// SPDX-License-Identifier: AGPL-3.0-only

//! The account's own deletion (1.9.1; App Store Review Guideline 5.1.1(v), the user's decision of
//! 2026-10-26): the app of a logged-in device deletes its account, at once and for good, with one
//! signed request.
//!
//! ```text
//! POST /api/hjv/account/delete  {"confirm":"delete-account"}     the app: token and X-HJV-Proof
//! 200 {"deleted":true,"pcs":1,"devices":2,"relays":0}
//! ```
//!
//! - An account path (api.rs `session`): the token with its account key's signature, in time (a
//!   signature whose time is off gets 409 `clock`: unlike a logout, a deletion takes no signature
//!   out of the window), counted with the account key's other calls (keys.rs, 30 a minute: 429
//!   `rate_limited`). The body must be that object, its other members ignored; anything else is
//!   400 `bad_request` and deletes nothing, so that a stray request (an empty POST, another path's
//!   body) never deletes an account, and the signature covers the body.
//! - What goes is what `hjv-api admin user delete` deletes (db.rs `delete_account`), in one
//!   transaction: the account's PCs leave it through `release_binding` (tombstones, so that each
//!   gets a signed `revoked`), its relays are revoked, its tokens go (every device is logged out:
//!   another device's next request gets the 401 of an unknown token), and with the account row
//!   its account keys with their nonces, key use and answers, its relay pre-approvals, its wake
//!   jobs with their commands, and its allowlist entry unless it is blocked. Besides, the open
//!   support codes its keys made end in memory at once (support.rs `end_keys`: their links show
//!   nothing more, and a held waiting room hears of it), and held relay polls look again.
//! - The App Review account deletes itself as any other (it may not make support codes, but this
//!   it may), and its allowlist entry stays (db.rs `delete_account_by_user`): its ID names no
//!   person, and the next review login needs the entry (docs/server-runbook.md 7-17); that login
//!   makes a new, empty account.
//! - The operator record (oplog.rs) takes a line before anything is deleted, under the service's
//!   own user ID and no sudo user, with the account's tag (`cmd="app account-delete"
//!   targets=<tag>`), and one after it with the counts (`done="ok: …"`). Without the record (the
//!   admin socket is off), or when the record does not take the first line, nothing is deleted
//!   (503 `not_recorded`): a restored backup brings deleted accounts back, and the record's tag is
//!   how the operator deletes them again (docs/server-runbook.md 7-14).
//! - The very same request again within HJV_API_RETRY_WINDOW gets the first answer, as every
//!   signed request that changes something does (api.rs `authenticate`). The deletion ends the
//!   token by which such a retry is found, so its answer is kept under the request's hash as well
//!   (`RetryCache::again`, asked before the request is authenticated: a request nobody signed
//!   leaves nothing in memory). Two different requests of one token: the second finds the account
//!   gone and gets `"deleted":true` with counts of 0.
//! - A deletion leaves the record `account deleted-by-user` with the account's tag (kept
//!   HJV_API_EVENT_DAYS, counted by `admin report` and the daily summary); the journal gets the
//!   tag and the counts, never a portal ID, name, address or device ID.

use crate::{
    api::{self, session, ApiRequest, Reply},
    db::{self, Listing, Session},
    keys::{CALLS_PER_MINUTE, TEXT_TOO_OFTEN},
    login::{self, Retry},
    oplog::{OpEntry, OpLog},
    proof, review,
    state::AppState,
    util::{hex, now, sha256},
};
use hyper::StatusCode;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

pub const DELETE_PATH: &str = "/api/hjv/account/delete";
/// The value of the body's `confirm`, the one member a deletion needs.
pub const CONFIRM: &str = "delete-account";
/// The command of a deletion's lines in the operator record.
pub const COMMAND: &str = "app account-delete";
/// The record did not take the deletion's first line, or there is none: nothing was deleted.
pub const TEXT_NOT_RECORDED: &str =
    "삭제 기록을 남기지 못해 계정을 지우지 않았습니다. 잠시 뒤 다시 시도해 주세요.";
const TEXT_BAD_REQUEST: &str = "요청을 이해하지 못했습니다.";

fn refusal(status: StatusCode, text: &str, code: &str) -> Reply {
    Reply::json(status, &json!({ "error": text, "hjv": code }))
}

/// Whether the body is the confirmation: a JSON object whose `confirm` is the string [`CONFIRM`]
/// (other members are ignored, room for later ones).
fn confirmed(body: &[u8]) -> bool {
    matches!(
        serde_json::from_slice::<Value>(body),
        Ok(Value::Object(members)) if members.get("confirm").and_then(Value::as_str) == Some(CONFIRM)
    )
}

/// The request's hash as api.rs `authenticate` makes it for a retry (proof.rs `request_hash`:
/// its proof, method, target, body and token); none without one of hjv-api's tokens.
fn request_hash(req: &ApiRequest) -> Option<[u8; 32]> {
    let token = api::bearer_hash(&req.headers)?;
    // a header that is not text counts as there, and wrong (as authenticate has it)
    let header = req
        .headers
        .get(proof::HEADER)
        .map_or("", |v| v.to_str().unwrap_or(""));
    let body = sha256(req.body.as_ref().map_or(&[][..], |b| b.as_ref()));
    Some(proof::request_hash(
        header,
        req.method.as_str(),
        &req.target,
        &body,
        &token,
    ))
}

/// What a deletion took.
struct Deleted {
    pcs: usize,
    /// the active account keys: the devices logged in
    devices: usize,
    tokens: usize,
    relays: usize,
    /// the allowlist entry as it stood
    listing: Listing,
}

impl Deleted {
    /// What became of the allowlist entry: `removed`, `kept` (the App Review account's),
    /// `stays blocked` or `none`.
    fn entry(&self, keep_listing: bool) -> &'static str {
        match self.listing {
            Listing::Registered if keep_listing => "kept",
            Listing::Registered => "removed",
            Listing::Blocked => "stays blocked",
            Listing::Absent => "none",
        }
    }

    /// `1 PCs, 2 account keys, 2 tokens, 0 relays, allowlist entry removed`
    fn counts(&self, keep_listing: bool) -> String {
        format!(
            "{} PCs, {} account keys, {} tokens, {} relays, allowlist entry {}",
            self.pcs,
            self.devices,
            self.tokens,
            self.relays,
            self.entry(keep_listing)
        )
    }

    /// The `done` of the record's line: the counts, and whose entry was kept.
    fn done(&self, keep_listing: bool) -> String {
        let whose = if self.entry(keep_listing) == "kept" {
            " (App Review)"
        } else {
            ""
        };
        format!("ok: {}{whose}", self.counts(keep_listing))
    }
}

/// Handles `POST /api/hjv/account/delete` (api.rs `route` checked the method, the body's type
/// and size, and that no `Origin` came).
pub async fn delete(state: &Arc<AppState>, req: &ApiRequest) -> Reply {
    let retry_key = request_hash(req).map(|sent| (format!("account-delete.{}", hex(&sent)), sent));
    // the very same request again, once its first run may have deleted the token (the module)
    if let Some((key, sent)) = &retry_key {
        if let Some(first) = state.request_retries.again(key, *sent) {
            return login::wait_for(first, Reply::server_error).await;
        }
    }
    let s = match session(state, req).await {
        Ok(s) => s,
        Err(reply) => return reply,
    };
    let Some(own) = s.key else {
        // session() lets no request without its key's proof through
        return Reply::server_error();
    };
    let tag = state.tags.account(&s.account.login_key);
    if !state.key_calls.try_take(own.ckid) {
        if state.first_notice(&format!("keys-calls:{}", own.ckid), Duration::from_secs(60)) {
            log_warn!(
                "account delete: more than {CALLS_PER_MINUTE} calls a minute from one key (acct={tag}, ckid={})",
                own.ckid
            );
        }
        return refusal(
            StatusCode::TOO_MANY_REQUESTS,
            TEXT_TOO_OFTEN,
            "rate_limited",
        );
    }
    let body = req.body.as_ref().map_or(&[][..], |b| b.as_ref());
    if !confirmed(body) {
        if state.first_notice(
            &format!("account-delete-request:{tag}"),
            Duration::from_secs(600),
        ) {
            log_info!(
                "account delete: a request not of the form (acct={tag}); the next ten minutes are not logged"
            );
        }
        return refusal(StatusCode::BAD_REQUEST, TEXT_BAD_REQUEST, "bad_request");
    }
    // a deletion that cannot be recorded is not made
    let Some((record, uid)) = state.operator_record.get().cloned() else {
        if state.first_notice("account-delete:no-record", Duration::from_secs(600)) {
            log_err!(
                "account delete: no operator record (the admin socket is off): a deletion is refused (acct={tag})"
            );
        }
        return refusal(
            StatusCode::SERVICE_UNAVAILABLE,
            TEXT_NOT_RECORDED,
            "not_recorded",
        );
    };
    // every answer from here on is kept under the request's hash too; its key is new to every
    // other request, so Again is a retry that came in between, and Reused does not happen
    let first = match retry_key.map(|(key, sent)| state.request_retries.begin(&key, sent)) {
        Some(Retry::Again(first)) => return login::wait_for(first, Reply::server_error).await,
        Some(Retry::First(answer)) => Some(answer),
        Some(Retry::Reused) | None => None,
    };
    let reply = delete_now(state, &s, &tag, record, uid).await;
    if let Some(first) = first {
        let _ = first.send(Some(reply.clone()));
    }
    reply
}

/// The deletion itself: the record's first line, the transaction, what memory holds of the
/// account, the record's result line, the record and the journal.
async fn delete_now(
    state: &Arc<AppState>,
    s: &Session,
    tag: &str,
    record: Arc<OpLog>,
    uid: u32,
) -> Reply {
    let written = {
        let (record, targets) = (record.clone(), vec![tag.to_owned()]);
        tokio::task::spawn_blocking(move || {
            record.append(&OpEntry {
                uid,
                sudo_user: "",
                command: COMMAND,
                targets: &targets,
                reveal: false,
            })
        })
        .await
    };
    if !matches!(written, Ok(Ok(()))) {
        log_err!(
            "account delete: the operator record did not take the line (acct={tag}); nothing was deleted"
        );
        return refusal(
            StatusCode::SERVICE_UNAVAILABLE,
            TEXT_NOT_RECORDED,
            "not_recorded",
        );
    }
    let (id, login_key, at) = (s.account.id, s.account.login_key.clone(), now());
    // only the App Review account's ID is outside the ID rule here: its tokens hold only while its
    // file names it (review.rs `admits`, checked by session())
    let keep_listing = review::is_review(&login_key);
    let deleted = state
        .db
        .call(move |c| {
            let deleted = db::in_tx(c, |tx| {
                // another request of the same token deleted it a moment ago
                if db::account(tx, id)?.is_none() {
                    return Ok(None);
                }
                let pcs = db::devices_of(tx, id)?.len();
                let devices = db::keys_of(tx, id)?.len();
                let relays = db::active_relays_of(tx, id)?.len();
                let tokens = db::revoke_tokens(tx, id)?;
                let listing = db::listing(tx, &login_key)?;
                db::delete_account_by_user(tx, id, keep_listing, at)?;
                Ok(Some(Deleted {
                    pcs,
                    devices,
                    tokens,
                    relays,
                    listing,
                }))
            })?;
            // the deleted rows must not linger in the WAL; the deletion stands either way
            if deleted.is_some() {
                if let Err(e) = db::checkpoint(c) {
                    log_err!("account delete: the WAL was not truncated after a deletion: {e}");
                }
            }
            Ok(deleted)
        })
        .await;
    match deleted {
        Ok(Some(d)) => {
            // what memory holds of it: the open support codes of its keys end, and the held
            // polls of its relays look again (relay.rs `admin_revoke`)
            let ckids = state.support.open_keys(id, at);
            state.support.end_keys(id, &ckids, at);
            if d.relays > 0 {
                state.relays.changed();
            }
            result_line(
                record,
                uid,
                d.done(keep_listing),
                tag,
                "the account is deleted",
            )
            .await;
            state
                .record(
                    "account",
                    Some(tag.to_owned()),
                    None,
                    None,
                    "deleted-by-user",
                )
                .await;
            let whose = if keep_listing {
                ", the App Review account"
            } else {
                ""
            };
            log_notice!(
                "account: deleted at its own request (acct={tag}{whose}): {}",
                d.counts(keep_listing)
            );
            Reply::ok(&json!({
                "deleted": true,
                "pcs": d.pcs,
                "devices": d.devices,
                "relays": d.relays,
            }))
        }
        Ok(None) => {
            result_line(
                record,
                uid,
                "ok: already deleted".to_owned(),
                tag,
                "it was deleted already",
            )
            .await;
            Reply::ok(&json!({ "deleted": true, "pcs": 0, "devices": 0, "relays": 0 }))
        }
        Err(e) => {
            log_err!("account delete: {e}");
            result_line(
                record,
                uid,
                "failed: database".to_owned(),
                tag,
                "nothing was deleted",
            )
            .await;
            Reply::server_error()
        }
    }
}

/// The record's line of what the deletion came to; one it does not take is the journal's, with
/// `how` the account stands.
async fn result_line(record: Arc<OpLog>, uid: u32, done: String, tag: &str, how: &str) {
    let written = tokio::task::spawn_blocking(move || {
        record.append_done(
            &OpEntry {
                uid,
                sudo_user: "",
                command: COMMAND,
                targets: &[],
                reveal: false,
            },
            &done,
        )
    })
    .await;
    if !matches!(written, Ok(Ok(()))) {
        log_err!(
            "account delete: the operator record did not take the result line (acct={tag}); {how}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_confirmation() {
        for (body, ok) in [
            (r#"{"confirm":"delete-account"}"#, true),
            (r#"{ "confirm" : "delete-account" }"#, true),
            (r#"{"confirm":"delete-account","from":"1.9.2"}"#, true),
            ("", false),
            ("{}", false),
            ("null", false),
            ("[]", false),
            (r#""delete-account""#, false),
            (r#"["confirm","delete-account"]"#, false),
            (r#"{"confirm":"delete"}"#, false),
            (r#"{"confirm":"Delete-Account"}"#, false),
            (r#"{"confirm":true}"#, false),
            (r#"{"confirm":null}"#, false),
            (r#"{"Confirm":"delete-account"}"#, false),
            (r#"{"confirm":"delete-account""#, false),
        ] {
            assert_eq!(confirmed(body.as_bytes()), ok, "{body}");
        }
    }

    #[test]
    fn what_the_lines_say() {
        let deleted = |listing| Deleted {
            pcs: 1,
            devices: 2,
            tokens: 2,
            relays: 0,
            listing,
        };
        let staff = deleted(Listing::Registered);
        assert_eq!(
            staff.counts(false),
            "1 PCs, 2 account keys, 2 tokens, 0 relays, allowlist entry removed"
        );
        assert_eq!(
            staff.done(false),
            "ok: 1 PCs, 2 account keys, 2 tokens, 0 relays, allowlist entry removed"
        );
        assert_eq!(
            staff.done(true),
            "ok: 1 PCs, 2 account keys, 2 tokens, 0 relays, allowlist entry kept (App Review)"
        );
        assert_eq!(deleted(Listing::Blocked).entry(true), "stays blocked");
        assert_eq!(deleted(Listing::Absent).entry(false), "none");
        // only what the record keeps as it is (oplog.rs `safe`), within its 160 characters
        for listing in [Listing::Registered, Listing::Blocked, Listing::Absent] {
            for keep in [false, true] {
                let done = deleted(listing).done(keep);
                assert!(done.len() <= 160, "{done}");
                assert!(
                    done.chars()
                        .all(|c| c.is_ascii_alphanumeric() || " ._:,()-".contains(c)),
                    "{done}"
                );
            }
        }
    }

    #[test]
    fn texts_never_say_disabled() {
        for text in [TEXT_NOT_RECORDED, TEXT_BAD_REQUEST] {
            assert!(!text.to_lowercase().contains("disabled"));
        }
    }
}

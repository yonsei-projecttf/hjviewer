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
//!   finds a new, empty account. From 1.9.2 (review.rs) the demo PCs it holds of its file's `PC=`
//!   line stay bound, to that new account, made in the same transaction, so that a reviewer who
//!   deletes the account (Apple tests it) still finds the demo PC online; the answer's `pcs` counts
//!   the PCs that left, and the record and the journal add `, <n> demo PCs kept`. A deletion from a
//!   demo PC's own login is refused before anything is recorded, with 403 `demo_pc` and
//!   [`TEXT_DEMO_PC`]: the app there would let its PC go with the account it deleted (it does so on
//!   `"deleted":true`, and on nothing else). The PC is found by the account's login key, which the
//!   new account keeps, so that a request that authenticated just before another device's deletion
//!   moved the PC is refused too.
//! - The operator record (oplog.rs) takes a line before anything is deleted, under the service's
//!   own user ID and no sudo user, with the account's tag (`cmd="app account-delete"
//!   targets=<tag>`), and one after it with the tag again and the counts (`acct=<tag> done="ok:
//!   …"`). Without the record (the admin socket is off), or when the record does not take the
//!   first line, nothing is deleted (503 `not_recorded`): a restored backup brings deleted
//!   accounts back, and the record's tag is how the operator deletes them again
//!   (docs/server-runbook.md 7-14).
//! - In the transaction the request's own token must still be there. It goes with its account,
//!   whose ID the next new account may take (`accounts.id` is reused), and that account is not
//!   the request's to delete: a request whose token went with its account a moment ago (another
//!   request of the account, the operator's `user delete`) gets `"deleted":true` with counts of 0,
//!   and one whose login ended another way meanwhile (a logout, the operator's revoke-all or
//!   block), the 401 of an unknown token; neither deletes anything.
//! - A request runs to its end in a task of its own, also when the client goes away (as a login
//!   does): a deletion is never left without its result line, its record, its journal line or
//!   what memory holds, nor its retry without the first answer.
//! - The very same request again within HJV_API_RETRY_WINDOW gets the first answer, as every
//!   signed request that changes something does (api.rs `authenticate`). The deletion ends the
//!   token by which such a retry is found, so its answer is kept under the request's hash as well
//!   (`RetryCache::again`, asked before the request is authenticated: a request nobody signed
//!   leaves nothing in memory).
//! - A deletion leaves the record `account deleted-by-user` with the account's tag (kept
//!   HJV_API_EVENT_DAYS, counted by `admin report` and the daily summary); the journal gets the
//!   tag and the counts, never a portal ID, name, address or device ID.

use crate::{
    api::{self, session, ApiRequest, Reply},
    db::{self, Keep, Listing, Session},
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
/// The answer to a deletion from a demo PC's own login of the App Review account (1.9.2).
pub const TEXT_DEMO_PC: &str = "이 PC는 App Review 계정의 데모 PC라서 여기서는 계정을 지울 수 없습니다. iPhone이나 iPad의 HJViewer에서 지우세요. (This PC is the App Review account's demo PC: delete the account in HJViewer on the iPhone or iPad.)";

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
    /// the PCs that left the account
    pcs: usize,
    /// the App Review account's demo PCs that stayed, bound to its new account (1.9.2)
    kept: usize,
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

    /// `1 PCs, 2 account keys, 2 tokens, 0 relays, allowlist entry removed`, and
    /// `, 1 demo PCs kept` when the App Review account kept some
    fn counts(&self, keep_listing: bool) -> String {
        let kept = if self.kept > 0 {
            format!(", {} demo PCs kept", self.kept)
        } else {
            String::new()
        };
        format!(
            "{} PCs, {} account keys, {} tokens, {} relays, allowlist entry {}{kept}",
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

/// What the transaction came to.
enum Outcome {
    Deleted(Deleted),
    /// the request's token went with its account a moment ago (another request of the account,
    /// the operator's `user delete`): it is deleted already
    Gone,
    /// the request's login ended another way while it waited (a logout, the operator's
    /// revoke-all or block): the account stays
    Ended,
}

/// Handles `POST /api/hjv/account/delete` (api.rs `route` checked the method, the body's type
/// and size, and that no `Origin` came). The request runs in a task of its own, to its end also
/// when the client goes away (nginx then closes the connection, and hyper drops this future), as
/// a login does (login.rs `handle`). The task works on its own copy of the request, and so it also
/// leaves the answer where [`api::authenticate`] keeps it for a retry under the key's nonce, as
/// `route` does for the other paths.
pub async fn delete(state: &Arc<AppState>, req: &ApiRequest) -> Reply {
    let task_state = state.clone();
    let own = ApiRequest {
        method: req.method.clone(),
        path: req.path.clone(),
        query: req.query.clone(),
        target: req.target.clone(),
        headers: req.headers.clone(),
        body: req.body.clone(),
        ip: req.ip.clone(),
        retry: Default::default(),
    };
    let task = tokio::spawn(async move {
        let reply = handle(&task_state, &own).await;
        if let Some(first) = own.retry.get() {
            let _ = first.send(Some(reply.clone()));
        }
        reply
    });
    match task.await {
        Ok(reply) => reply,
        Err(_) => {
            log_err!("account delete: the request's task failed");
            Reply::server_error()
        }
    }
}

/// The request, from its retry's answer to the deletion's.
async fn handle(state: &Arc<AppState>, req: &ApiRequest) -> Reply {
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
    // the App Review account's demo PC keeps its account (review.rs): the app on it lets the PC
    // go with an account it deleted, so its own login deletes nothing
    match on_demo_pc(state, &s).await {
        Ok(false) => {}
        Ok(true) => {
            if state.first_notice(
                &format!("account-delete-demo-pc:{tag}"),
                Duration::from_secs(600),
            ) {
                log_notice!(
                    "account delete: refused from a demo PC of the App Review account (acct={tag}); the next ten minutes are not logged"
                );
            }
            return refusal(StatusCode::FORBIDDEN, TEXT_DEMO_PC, "demo_pc");
        }
        Err(reply) => return reply,
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

/// Whether the request comes from a PC that the App Review account holds (review.rs): its
/// login's device is one of the account's PCs, its demo PCs, whether or not that login proved the
/// PC's key (a request that only claims the ID refuses its own deletion, nothing else). The PC is
/// looked for by the account's login key, not its id: the new row that the account's own deletion
/// makes for its demo PCs has the same key and another id (db.rs `delete_account_by_user`). A
/// request that authenticated just before another device's deletion moved the PC to that row
/// comes here after it; by the id it would find the PC in another account, go on, and find its
/// own account gone: `"deleted":true`, on which the app there lets its PC go (tests/review.rs).
async fn on_demo_pc(state: &Arc<AppState>, s: &Session) -> Result<bool, Reply> {
    if s.device_id.is_empty() || !review::is_account(&s.account) {
        return Ok(false);
    }
    let (id, key) = (s.device_id.clone(), s.account.login_key.clone());
    state
        .db
        .call(move |c| Ok(db::bound_key(c, &id, &key)?.is_some()))
        .await
        .map_err(|e| {
            log_err!("account delete: {e}");
            Reply::server_error()
        })
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
    let (id, login_key, created_at, at) = (
        s.account.id,
        s.account.login_key.clone(),
        s.account.created_at,
        now(),
    );
    let (token_hash, wal_tag) = (s.token_hash.clone(), tag.to_owned());
    // only the App Review account's ID is outside the ID rule here: its tokens hold only while its
    // file names it (review.rs `admits`, checked by session())
    let keep_listing = review::is_review(&login_key);
    // its demo PCs stay bound, to its new account (review.rs, db.rs `delete_account_by_user`)
    let demo_pcs = if review::is_account(&s.account) {
        review::active_for(state, &login_key, at)
            .await
            .map(|review| review.demo_pcs().to_vec())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let outcome = state
        .db
        .call(move |c| {
            let outcome = db::in_tx(c, |tx| {
                // The request's own token must still be the account's: it goes with the account,
                // whose ID the next new account may take (accounts.id is reused), and that one is
                // not this request's to delete.
                if !db::token_of(tx, &token_hash, id)? {
                    // the very account still there: its login ended another way meanwhile
                    let same = db::account(tx, id)?
                        .is_some_and(|a| a.login_key == login_key && a.created_at == created_at);
                    return Ok(if same { Outcome::Ended } else { Outcome::Gone });
                }
                let pcs = db::devices_of(tx, id)?.len();
                let devices = db::keys_of(tx, id)?.len();
                let relays = db::active_relays_of(tx, id)?.len();
                let tokens = db::revoke_tokens(tx, id)?;
                let listing = db::listing(tx, &login_key)?;
                let keep = Keep {
                    listing: keep_listing,
                    demo_pcs: &demo_pcs,
                };
                let kept = db::delete_account_by_user(tx, id, keep, at)?.unwrap_or(0);
                Ok(Outcome::Deleted(Deleted {
                    pcs: pcs - kept,
                    kept,
                    devices,
                    tokens,
                    relays,
                    listing,
                }))
            })?;
            // the deleted rows must not linger in the WAL; the deletion stands either way
            if matches!(outcome, Outcome::Deleted(_)) {
                if let Err(e) = db::checkpoint(c) {
                    log_err!(
                        "account delete: the WAL was not truncated after a deletion (acct={wal_tag}): {e}"
                    );
                }
            }
            Ok(outcome)
        })
        .await;
    match outcome {
        Ok(Outcome::Deleted(d)) => {
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
                tag,
                d.done(keep_listing),
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
        Ok(Outcome::Gone) => {
            result_line(
                record,
                uid,
                tag,
                DONE_GONE.to_owned(),
                "it was deleted already",
            )
            .await;
            Reply::ok(&json!({ "deleted": true, "pcs": 0, "devices": 0, "relays": 0 }))
        }
        Ok(Outcome::Ended) => {
            log_info!(
                "account delete: the login ended while the request waited (acct={tag}); nothing was deleted"
            );
            result_line(
                record,
                uid,
                tag,
                DONE_ENDED.to_owned(),
                "nothing was deleted",
            )
            .await;
            api::unauthorized()
        }
        Err(e) => {
            log_err!("account delete: {e} (acct={tag}); nothing was deleted");
            result_line(
                record,
                uid,
                tag,
                DONE_FAILED.to_owned(),
                "nothing was deleted",
            )
            .await;
            Reply::server_error()
        }
    }
}

/// The `done` of a request whose account was deleted already, of one whose login ended
/// meanwhile, and of one the database failed: none of them deleted anything.
const DONE_GONE: &str = "ok: already deleted";
const DONE_ENDED: &str = "failed: the login ended";
const DONE_FAILED: &str = "failed: database";

/// The record's line of what the deletion came to, with the account's tag (two deletions may
/// run at once); one it does not take is the journal's, with `how` the account stands.
async fn result_line(record: Arc<OpLog>, uid: u32, tag: &str, done: String, how: &str) {
    let who = tag.to_owned();
    let written = tokio::task::spawn_blocking(move || {
        record.append_done_for(
            &OpEntry {
                uid,
                sudo_user: "",
                command: COMMAND,
                targets: &[],
                reveal: false,
            },
            &who,
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
            kept: 0,
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
        // the App Review account's demo PCs that stayed (1.9.2), said only when there are some
        let review = Deleted {
            pcs: 0,
            kept: 3,
            devices: 20,
            tokens: 20,
            relays: 0,
            listing: Listing::Registered,
        };
        assert_eq!(
            review.counts(true),
            "0 PCs, 20 account keys, 20 tokens, 0 relays, allowlist entry kept, 3 demo PCs kept"
        );
        assert_eq!(
            review.done(true),
            "ok: 0 PCs, 20 account keys, 20 tokens, 0 relays, allowlist entry kept, 3 demo PCs kept (App Review)"
        );
        // only what the record keeps as it is (oplog.rs `safe`), within its 160 characters
        let mut dones = vec![
            DONE_GONE.to_owned(),
            DONE_ENDED.to_owned(),
            DONE_FAILED.to_owned(),
            review.done(true),
        ];
        for listing in [Listing::Registered, Listing::Blocked, Listing::Absent] {
            for keep in [false, true] {
                dones.push(deleted(listing).done(keep));
            }
        }
        for done in dones {
            assert!(done.len() <= 160, "{done}");
            assert!(
                done.chars()
                    .all(|c| c.is_ascii_alphanumeric() || " ._:,()-".contains(c)),
                "{done}"
            );
        }
    }

    #[test]
    fn texts_never_say_disabled() {
        for text in [TEXT_NOT_RECORDED, TEXT_BAD_REQUEST, TEXT_DEMO_PC] {
            assert!(!text.to_lowercase().contains("disabled"));
        }
    }
}

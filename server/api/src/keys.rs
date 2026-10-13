// SPDX-License-Identifier: AGPL-3.0-only

//! Account keys (design v3 §4-1, §5-2): the Ed25519 key a device makes for each password login,
//! which signs every account request after it (proof.rs) and, on the PCs' trust lists, the
//! same-account connection proofs.
//!
//! **Registration.** The sealed login payload (seal.rs) carries
//!
//! - `"ck"`: the account key's public key, standard base64 of 32 bytes;
//! - `"cp"`: standard base64 of the 64-byte Ed25519 signature by that key over the bytes
//!   `"hjv-ckey-v1" NUL n NUL id NUL ck` (parts joined by single NUL bytes, no NUL at the end),
//!   where `n` and `id` are the same payload's nonce and device ID and `ck` the text above, all
//!   exactly as sealed.
//!
//! After the SSO accepted the password (login.rs), hjv-api answers 400 with the failure text of
//! every failed login when `ck` or `cp` is missing or not that signature (checked strictly), and
//! when another row (of any account, revoked or not) has the same key. An account has at most
//! [`MAX_ACTIVE_KEYS`] active keys, counting the ones the login replaces as gone; past that the
//! login gets 403 [`TEXT_TOO_MANY`]. The key is stored with the server's own view of the login
//! (time, app version, platform, the device name of devmodel.rs, on or off campus; no address)
//! and the device's own name apart; the account's older keys of the same device (the same
//! `dev_tag`, the tag of the app's uuid) are revoked with their tokens; the change serial goes up
//! and the key keeps that value as its `ks`, which the login's answer carries.
//!
//! **Paths** (all with the token and its key's `X-HJV-Proof`, at most [`CALLS_PER_MINUTE`] a
//! minute per account):
//!
//! | path | answer |
//! |---|---|
//! | `GET /api/hjv/keys` | `{"total":n,"data":[{"ckid","name","app_name","platform","app_ver","net","created","last_login","last_used","last_used_pc","use":[{"pc_id","pc","last","n30"}],"current","new"}]}`: the account's active keys, newest first |
//! | `POST /api/hjv/keys/{ckid}/ack` | `{"ack":true}`: a device of the account confirmed a key made after its own ("내 기기 맞음") |
//! | `DELETE /api/hjv/keys/{ckid}` | `{"revoked":true}`: the key and its token are revoked ("로그아웃시키기") |
//! | `POST /api/hjv/keys/revoke-others` | `{"revoked":n}`: every other key of the account and its token |
//!
//! A key of another account, an unknown or revoked one: the same 404. `new` marks a key made
//! after the asking one that neither the asking key nor an older key of the account has
//! confirmed, for seven days. A key confirms only keys made after it (its own or an older one:
//! the same 404), and a confirmation counts for the device that gave it and the ones that logged
//! in after it: whoever logs in with a leaked portal password cannot take the notice off the
//! owner's devices, neither by confirming its own key nor through a second login of its own that
//! confirms the first (design v3 §0, §1-1). `name` is the server's device name, `app_name` the
//! device's own. `use` is what the PCs reported through their sync (U6, design v3 §5-2): per PC
//! the latest time the key opened it without its password (by the PC's clock, never after the
//! server got the report) and the count of the last 30 days (UTC); `pc` is the PC's host name. A
//! PC's lines go when it leaves the account (design v3 §10-1). `last_used` and `last_used_pc` are
//! its latest line, or null.

use crate::{
    api::{paged, session, ApiRequest, Reply},
    db::{self, Key, KeyUse},
    state::AppState,
    util::{b64, b64_decode, now, DAY},
};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use hyper::{Method, StatusCode};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub const CONTEXT: &[u8] = b"hjv-ckey-v1";
/// Active account keys per account (design v3 §4-1 step 4).
pub const MAX_ACTIVE_KEYS: usize = 20;
/// The answer past MAX_ACTIVE_KEYS (shown in the login dialog).
pub const TEXT_TOO_MANY: &str = "로그인한 기기가 너무 많습니다. 다른 기기에서 정리해 주세요.";
/// Calls of the keys paths per account and minute (design v3 §5-5).
pub const CALLS_PER_MINUTE: usize = 30;
const TEXT_TOO_OFTEN: &str = "요청이 너무 많습니다. 잠시 뒤 다시 시도해 주세요.";
/// How long a key no device has confirmed counts as new.
pub const NEW_FOR: i64 = 7 * DAY;

/// The bytes `cp` signs.
pub fn message(nonce: &str, device_id: &str, ck: &str) -> Vec<u8> {
    let fields: [&[u8]; 4] = [
        CONTEXT,
        nonce.as_bytes(),
        device_id.as_bytes(),
        ck.as_bytes(),
    ];
    fields.join(&0u8)
}

/// What is wrong with a login's account key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyError {
    /// no `ck` or no `cp`
    Missing,
    /// not a key or not its signature
    Bad,
}

/// A login's account key as the seal carried it.
#[derive(Debug, Clone, Default)]
pub struct KeyClaim {
    pub ck: Option<String>,
    pub cp: Option<String>,
    /// the bytes `cp` should sign
    pub message: Vec<u8>,
}

impl KeyClaim {
    pub fn new(ck: Option<String>, cp: Option<String>, nonce: &str, device_id: &str) -> KeyClaim {
        let message = message(nonce, device_id, ck.as_deref().unwrap_or(""));
        KeyClaim { ck, cp, message }
    }

    /// The public key, when `cp` is its strict signature of the login.
    pub fn verify(&self) -> Result<[u8; 32], KeyError> {
        let (Some(ck), Some(cp)) = (self.ck.as_deref(), self.cp.as_deref()) else {
            return Err(KeyError::Missing);
        };
        let public_key = b64_decode(ck)
            .and_then(|k| <[u8; 32]>::try_from(k).ok())
            .ok_or(KeyError::Bad)?;
        let signature = b64_decode(cp)
            .and_then(|s| <[u8; 64]>::try_from(s).ok())
            .ok_or(KeyError::Bad)?;
        let key = VerifyingKey::from_bytes(&public_key).map_err(|_| KeyError::Bad)?;
        key.verify_strict(&self.message, &Signature::from_bytes(&signature))
            .map_err(|_| KeyError::Bad)?;
        Ok(public_key)
    }
}

/// What the app does, for tests and tools: `ck` and `cp` of the account key with this 32-byte
/// seed, for a login sealed with nonce `nonce` from device `device_id`.
pub fn proof(seed: &[u8; 32], nonce: &str, device_id: &str) -> (String, String) {
    let key = SigningKey::from_bytes(seed);
    let ck = b64(&key.verifying_key().to_bytes());
    let cp = b64(&key.sign(&message(nonce, device_id, &ck)).to_bytes());
    (ck, cp)
}

/// Calls of the keys paths per account in the last minute, in memory.
#[derive(Default)]
pub struct CallLimiter {
    calls: Mutex<HashMap<i64, VecDeque<Instant>>>,
}

impl CallLimiter {
    /// Takes a call for the account if it has one left this minute.
    pub fn try_take(&self, account_id: i64) -> bool {
        let mut calls = self.calls.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let mine = calls.entry(account_id).or_default();
        while mine
            .front()
            .is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(60))
        {
            mine.pop_front();
        }
        if mine.len() >= CALLS_PER_MINUTE {
            return false;
        }
        mine.push_back(now);
        true
    }

    /// Forgets the accounts without a call in the last minute.
    pub fn purge(&self) {
        let mut calls = self.calls.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        calls.retain(|_, c| {
            c.back()
                .is_some_and(|t| now.duration_since(*t) < Duration::from_secs(60))
        });
    }
}

/// The methods of the keys paths, for 405 answers.
pub fn allowed_methods(path: &str) -> Option<&'static str> {
    let rest = path.strip_prefix("/api/hjv/keys")?;
    Some(match rest {
        "" => "GET",
        "/revoke-others" => "POST",
        r if r.ends_with("/ack") => "POST",
        r if r.starts_with('/') => "DELETE",
        _ => return None,
    })
}

/// A key number from a path: digits, no sign or leading zero.
fn ckid(text: &str) -> Option<i64> {
    (!text.is_empty()
        && text.len() <= 18
        && text.bytes().all(|b| b.is_ascii_digit())
        && !text.starts_with('0'))
    .then(|| text.parse().ok())
    .flatten()
}

/// A key as the app's list of logged-in devices reads it; `uses` are its lines of `key use`,
/// latest first.
fn key_json(k: &Key, current: Option<&Key>, at: i64, uses: &[&KeyUse]) -> Value {
    let current_ckid = current.map_or(0, |c| c.ckid);
    // a later key's confirmation does not count here: it may be the newcomer's own
    let new = k.ckid > current_ckid
        && k.ack_by.is_none_or(|by| by > current_ckid)
        && at - k.created_at < NEW_FOR;
    let latest = uses.first();
    json!({
        "ckid": k.ckid,
        "name": k.model,
        "app_name": k.app_name,
        "platform": k.platform,
        "app_ver": k.app_ver,
        "net": k.login_net,
        "created": k.created_at,
        "last_login": k.last_login_at,
        "last_used": latest.map(|u| u.last),
        "last_used_pc": latest.map(|u| u.pc.clone()),
        "use": uses
            .iter()
            .map(|u| json!({ "pc_id": u.device_id, "pc": u.pc, "last": u.last, "n30": u.n }))
            .collect::<Vec<Value>>(),
        "current": k.ckid == current_ckid,
        "new": new,
    })
}

/// Handles the keys paths (the methods are checked by the caller).
pub async fn route(state: &Arc<AppState>, req: &ApiRequest) -> Reply {
    let s = match session(state, req).await {
        Ok(s) => s,
        Err(reply) => return reply,
    };
    let Some(own) = s.key else {
        // session() lets no request without its key's proof through
        return Reply::server_error();
    };
    if !state.key_calls.try_take(s.account.id) {
        if state.first_notice(
            &format!("keys-calls:{}", s.account.id),
            Duration::from_secs(60),
        ) {
            log_notice!(
                "keys: more than {CALLS_PER_MINUTE} calls a minute (acct={})",
                state.tags.account(&s.account.login_key)
            );
        }
        return Reply::error(StatusCode::TOO_MANY_REQUESTS, TEXT_TOO_OFTEN);
    }
    let account_id = s.account.id;
    let at = now();
    let rest = req.path.strip_prefix("/api/hjv/keys").unwrap_or("");
    let record = |result: &'static str| {
        state.record(
            "key",
            Some(state.tags.account(&s.account.login_key)),
            None,
            None,
            result,
        )
    };
    match (rest, &req.method) {
        ("", &Method::GET) => match state
            .db
            .call(move |c| {
                Ok((
                    db::keys_of(c, account_id)?,
                    db::key_use_of(c, account_id, &db::use_since(at))?,
                ))
            })
            .await
        {
            Ok((keys, uses)) => {
                let current = keys.iter().find(|k| k.ckid == own.ckid);
                let items = keys
                    .iter()
                    .map(|k| {
                        let mine: Vec<&KeyUse> = uses.iter().filter(|u| u.ckid == k.ckid).collect();
                        key_json(k, current, at, &mine)
                    })
                    .collect();
                Reply::ok(&paged(items, &req.query))
            }
            Err(e) => {
                log_err!("keys: {e}");
                Reply::server_error()
            }
        },
        ("/revoke-others", &Method::POST) => {
            let done = state
                .db
                .call(move |c| {
                    db::in_tx(c, |tx| {
                        db::revoke_other_keys(tx, account_id, own.ckid, "others", at)
                    })
                })
                .await;
            match done {
                Ok(n) => {
                    if n > 0 {
                        record("revoked-others").await;
                    }
                    Reply::ok(&json!({ "revoked": n }))
                }
                Err(e) => {
                    log_err!("keys: {e}");
                    Reply::server_error()
                }
            }
        }
        (path, &Method::POST) if path.ends_with("/ack") => {
            let Some(ckid) = path
                .strip_prefix('/')
                .and_then(|p| p.strip_suffix("/ack"))
                .and_then(ckid)
            else {
                return Reply::not_found();
            };
            match state
                .db
                .call(move |c| db::ack_key(c, account_id, ckid, own.ckid, at))
                .await
            {
                Ok(true) => Reply::ok(&json!({ "ack": true })),
                Ok(false) => Reply::not_found(),
                Err(e) => {
                    log_err!("keys: {e}");
                    Reply::server_error()
                }
            }
        }
        (path, &Method::DELETE) => {
            let Some(ckid) = path.strip_prefix('/').and_then(ckid) else {
                return Reply::not_found();
            };
            let done = state
                .db
                .call(move |c| db::in_tx(c, |tx| db::revoke_key(tx, account_id, ckid, "user", at)))
                .await;
            match done {
                Ok(true) => {
                    record("revoked-user").await;
                    Reply::ok(&json!({ "revoked": true }))
                }
                // another account's key, or none: the same answer
                Ok(false) => Reply::not_found(),
                Err(e) => {
                    log_err!("keys: {e}");
                    Reply::server_error()
                }
            }
        }
        _ => Reply::not_found(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claims() {
        let seed = [3u8; 32];
        let (ck, cp) = proof(&seed, "bm9uY2U=", "987654321");
        let claim = |ck: Option<&str>, cp: Option<&str>, n: &str, id: &str| {
            KeyClaim::new(ck.map(str::to_owned), cp.map(str::to_owned), n, id).verify()
        };
        assert_eq!(
            claim(Some(&ck), Some(&cp), "bm9uY2U=", "987654321"),
            Ok(crate::devsig::public_key(&seed))
        );
        assert_eq!(
            claim(None, Some(&cp), "bm9uY2U=", "987654321"),
            Err(KeyError::Missing)
        );
        assert_eq!(
            claim(Some(&ck), None, "bm9uY2U=", "987654321"),
            Err(KeyError::Missing)
        );
        // another login's nonce or device ID, another key, not base64, the wrong lengths
        assert_eq!(
            claim(Some(&ck), Some(&cp), "bm9uY2U9", "987654321"),
            Err(KeyError::Bad)
        );
        assert_eq!(
            claim(Some(&ck), Some(&cp), "bm9uY2U=", "987654322"),
            Err(KeyError::Bad)
        );
        let (other, _) = proof(&[4; 32], "bm9uY2U=", "987654321");
        assert_eq!(
            claim(Some(&other), Some(&cp), "bm9uY2U=", "987654321"),
            Err(KeyError::Bad)
        );
        for (k, p) in [
            ("not base64", cp.as_str()),
            (ck.as_str(), "not base64"),
            (&ck[..40], cp.as_str()),
            (ck.as_str(), &cp[..80]),
        ] {
            assert_eq!(
                claim(Some(k), Some(p), "bm9uY2U=", "987654321"),
                Err(KeyError::Bad),
                "{k} {p}"
            );
        }
        // a small-order key with a signature that verify_strict refuses
        let weak = b64(&[0u8; 32]);
        let zero_sig = b64(&[0u8; 64]);
        assert_eq!(
            claim(Some(&weak), Some(&zero_sig), "bm9uY2U=", "987654321"),
            Err(KeyError::Bad)
        );
    }

    #[test]
    fn paths() {
        assert_eq!(allowed_methods("/api/hjv/keys"), Some("GET"));
        assert_eq!(allowed_methods("/api/hjv/keys/revoke-others"), Some("POST"));
        assert_eq!(allowed_methods("/api/hjv/keys/12/ack"), Some("POST"));
        assert_eq!(allowed_methods("/api/hjv/keys/12"), Some("DELETE"));
        assert_eq!(allowed_methods("/api/hjv/keysx"), None);
        assert_eq!(allowed_methods("/api/hjv/key"), None);
        assert_eq!(ckid("12"), Some(12));
        for bad in [
            "",
            "0",
            "012",
            "-1",
            "+1",
            "1a",
            "12/ack",
            "9999999999999999999",
        ] {
            assert_eq!(ckid(bad), None, "{bad}");
        }
    }

    #[test]
    fn calls_per_minute() {
        let l = CallLimiter::default();
        for _ in 0..CALLS_PER_MINUTE {
            assert!(l.try_take(1));
        }
        assert!(!l.try_take(1));
        assert!(l.try_take(2));
        l.purge();
        assert!(!l.try_take(1));
    }
}

// SPDX-License-Identifier: AGPL-3.0-only

//! The second-factor hook (D11, design v2 §9-2 step 8). D11 keeps it off for now; this module is
//! the place where TOTP plugs in later, without changes to the app (it already has the code
//! dialog) or to the order of the login checks.
//!
//! When a [`SecondFactor`] is required, every well-formed login that passes the limits gets the
//! code dialog, allowlisted or not, so the dialog does not tell who is on the list:
//!
//! ```text
//! 200 {"type":"email_check","tfa_type":"tfa_check","secret":"<challenge>","user":{"name":"<ID as typed>"}}
//! ```
//!
//! The app then sends `{"type":"email_code","secret":…,"verificationCode":<code>,"tfaCode":<code>,
//! "username":…,"id":…,"uuid":…}` without a password (client research notes §2-1: the type is
//! "email_code" even for TOTP). hjv-api keeps the sealed password's content in memory only, under
//! the challenge, for at most five minutes and one try; an ID off the list fails at the code
//! step with the same answer as a wrong code (login.rs).
//!
//! A code dialog counts against the address like a failed login until its code step succeeds
//! (an unanswered or expired one keeps counting), and an address or an ID may have three dialogs
//! open at most, so that no one can fill the store and keep everyone else from logging in.

use crate::login::DeviceClaim;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

pub trait SecondFactor: Send + Sync {
    /// Whether logins must pass a code step before the SSO call.
    fn required(&self) -> bool;
    /// Checks a code of the account with this login key.
    fn verify(&self, login_key: &str, code: &str) -> bool;
}

/// D11 as decided: no second factor.
pub struct Off;

impl SecondFactor for Off {
    fn required(&self) -> bool {
        false
    }

    fn verify(&self, _login_key: &str, _code: &str) -> bool {
        false
    }
}

/// The second factor that HJV_API_SECOND_FACTOR names.
pub fn from_setting(setting: &str) -> Result<Arc<dyn SecondFactor>, String> {
    match setting {
        "off" => Ok(Arc::new(Off)),
        other => Err(format!(
            "HJV_API_SECOND_FACTOR={other}: this version has only \"off\" (D11); TOTP plugs in at tfa.rs"
        )),
    }
}

pub const CHALLENGE_TTL: Duration = Duration::from_secs(300);
const MAX_PENDING: usize = 1000;
/// Code dialogs open at once per client address and per ID.
pub const MAX_PENDING_EACH: usize = 3;

/// A login waiting for its code.
pub struct Pending {
    pub username: String,
    pub login_key: String,
    pub password: Zeroizing<String>,
    pub allowed: bool,
    pub limit_scope: &'static str,
    pub limit_key: String,
    pub device: DeviceClaim,
    /// the tag of the client address (log.rs)
    pub ip_tag: String,
    /// the address's attempt of the first request (limits.rs), counting until the code step
    /// succeeds
    pub ip_attempt: Option<i64>,
    created: Instant,
}

impl Pending {
    pub fn new(
        username: String,
        login_key: String,
        password: Zeroizing<String>,
        allowed: bool,
        limit: (&'static str, String),
        device: DeviceClaim,
        ip: (String, Option<i64>),
    ) -> Self {
        Pending {
            username,
            login_key,
            password,
            allowed,
            limit_scope: limit.0,
            limit_key: limit.1,
            device,
            ip_tag: ip.0,
            ip_attempt: ip.1,
            created: Instant::now(),
        }
    }
}

/// Why a code dialog was not opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Busy {
    /// too many logins wait already
    Full,
    /// this address or this ID has MAX_PENDING_EACH dialogs open
    Each,
}

#[derive(Default)]
pub struct PendingStore {
    map: Mutex<HashMap<String, Pending>>,
}

impl PendingStore {
    /// Keeps `pending` under `challenge`, unless too many logins wait already, in all or from its
    /// address or for its ID.
    pub fn insert(&self, challenge: String, pending: Pending) -> Result<(), Busy> {
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        map.retain(|_, p| p.created.elapsed() < CHALLENGE_TTL);
        if map.len() >= MAX_PENDING {
            return Err(Busy::Full);
        }
        let same_address = map.values().filter(|p| p.ip_tag == pending.ip_tag).count();
        let same_id = map
            .values()
            .filter(|p| p.login_key == pending.login_key)
            .count();
        if same_address >= MAX_PENDING_EACH || same_id >= MAX_PENDING_EACH {
            return Err(Busy::Each);
        }
        map.insert(challenge, pending);
        Ok(())
    }

    /// Takes the login waiting under `challenge` (one try per challenge).
    pub fn take(&self, challenge: &str) -> Option<Pending> {
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        map.remove(challenge)
            .filter(|p| p.created.elapsed() < CHALLENGE_TTL)
    }

    pub fn purge(&self) {
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        map.retain(|_, p| p.created.elapsed() < CHALLENGE_TTL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings() {
        assert!(!from_setting("off").unwrap().required());
        assert!(from_setting("totp").is_err());
    }

    fn pending(id: &str, ip: &str) -> Pending {
        Pending::new(
            id.into(),
            id.into(),
            Zeroizing::new("pw".into()),
            true,
            ("id", "k".into()),
            DeviceClaim::default(),
            (ip.into(), Some(1)),
        )
    }

    #[test]
    fn one_try_per_challenge() {
        let store = PendingStore::default();
        assert_eq!(store.insert("c".into(), pending("u", "a")), Ok(()));
        assert!(store.take("c").is_some());
        assert!(store.take("c").is_none());
        assert!(store.take("other").is_none());
    }

    #[test]
    fn three_dialogs_per_address_and_id() {
        let store = PendingStore::default();
        for i in 0..MAX_PENDING_EACH {
            assert_eq!(
                store.insert(format!("a{i}"), pending(&format!("u{i}"), "a")),
                Ok(())
            );
        }
        assert_eq!(
            store.insert("a9".into(), pending("u9", "a")),
            Err(Busy::Each)
        );
        for i in 0..MAX_PENDING_EACH {
            assert_eq!(
                store.insert(format!("b{i}"), pending("v", &format!("ip{i}"))),
                Ok(())
            );
        }
        assert_eq!(
            store.insert("b9".into(), pending("v", "ip9")),
            Err(Busy::Each)
        );
        // a dialog answered makes room again
        assert!(store.take("a0").is_some());
        assert_eq!(store.insert("a9".into(), pending("u9", "a")), Ok(()));
    }
}

// SPDX-License-Identifier: AGPL-3.0-only

//! Log lines for the journal. hjv-api writes one line per entry to stderr, which systemd passes to
//! journald (the unit's `LogNamespace=hjviewer`); a `<N>` prefix sets the syslog priority.
//!
//! What may go into a line: counts, outcomes, error kinds, and people or devices as tags only
//! ([`Tagger`]: an HMAC with the server's log key). Never a password, token, request body, secret,
//! portal ID, name, device ID or client address (design v2 §9-2, §13-1).

use crate::util::{address_group, hex};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::{
    fmt,
    io::Write,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, OnceLock,
    },
};
use zeroize::Zeroizing;

pub const CRIT: u8 = 2;
pub const ERR: u8 = 3;
pub const WARNING: u8 = 4;
pub const NOTICE: u8 = 5;
pub const INFO: u8 = 6;

static CAPTURE: Mutex<Option<Vec<String>>> = Mutex::new(None);
static QUIET: AtomicBool = AtomicBool::new(false);
static JOURNAL: OnceLock<bool> = OnceLock::new();

/// Writes one entry. Use the macros (`log_warn!` …) rather than this.
pub fn emit(priority: u8, args: fmt::Arguments<'_>) {
    let line = args.to_string().replace(['\n', '\r'], " ");
    if let Ok(mut capture) = CAPTURE.lock() {
        if let Some(lines) = capture.as_mut() {
            lines.push(line.clone());
        }
    }
    if QUIET.load(Ordering::Relaxed) {
        return;
    }
    // journald reads the priority from the prefix; on a terminal a word reads better
    let journal = *JOURNAL.get_or_init(|| std::env::var_os("JOURNAL_STREAM").is_some());
    let mut err = std::io::stderr().lock();
    let _ = if journal {
        writeln!(err, "<{priority}>{line}")
    } else {
        let level = match priority {
            0..=CRIT => "critical",
            ERR => "error",
            WARNING => "warning",
            NOTICE => "notice",
            _ => "info",
        };
        writeln!(err, "{level}: {line}")
    };
}

/// For tests: keep every entry from now on in memory and stop writing to stderr.
pub fn capture() {
    if let Ok(mut capture) = CAPTURE.lock() {
        capture.get_or_insert_with(Vec::new);
    }
    QUIET.store(true, Ordering::Relaxed);
}

/// The entries kept since [`capture`].
pub fn captured() -> Vec<String> {
    CAPTURE
        .lock()
        .map(|c| c.clone().unwrap_or_default())
        .unwrap_or_default()
}

#[macro_export]
macro_rules! log_crit {
    ($($arg:tt)*) => { $crate::log::emit($crate::log::CRIT, format_args!($($arg)*)) };
}
#[macro_export]
macro_rules! log_err {
    ($($arg:tt)*) => { $crate::log::emit($crate::log::ERR, format_args!($($arg)*)) };
}
#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => { $crate::log::emit($crate::log::WARNING, format_args!($($arg)*)) };
}
#[macro_export]
macro_rules! log_notice {
    ($($arg:tt)*) => { $crate::log::emit($crate::log::NOTICE, format_args!($($arg)*)) };
}
#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => { $crate::log::emit($crate::log::INFO, format_args!($($arg)*)) };
}

/// Tags for log lines, the event table and the login-limit keys: the first 64 bits of an HMAC
/// with the server's log key (credential hjviewer-api-log-key). The operator can find out which
/// account or device a tag stands for through the admin commands, which compute the same tags; the
/// records themselves show no one.
pub struct Tagger {
    key: Zeroizing<Vec<u8>>,
}

impl Tagger {
    pub fn new(key: &[u8]) -> Self {
        Self {
            key: Zeroizing::new(key.to_vec()),
        }
    }

    pub fn tag(&self, kind: &str, value: &str) -> String {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.key).expect("HMAC takes keys of any length");
        mac.update(kind.as_bytes());
        mac.update(&[0]);
        mac.update(value.as_bytes());
        hex(&mac.finalize().into_bytes()[..8])
    }

    /// An account, by its login key (the case-folded portal ID).
    pub fn account(&self, login_key: &str) -> String {
        self.tag("account", login_key)
    }

    pub fn device(&self, device_id: &str) -> String {
        self.tag("device", device_id)
    }

    /// A client address, as the login limits count it: an IPv6 address by its /64
    /// ([`address_group`]), so that the addresses of one subscriber share one tag.
    pub fn ip(&self, ip: &str) -> String {
        self.tag("ip", &address_group(ip))
    }

    /// The login limit of the devices already in an account (design v2 §9-2 step 5), one for all
    /// of them: however many PCs an account has, the wrong passwords that reach the SSO for its ID
    /// stay within HJV_API_LIMIT_ID plus HJV_API_LIMIT_BOUND.
    pub fn bound(&self, login_key: &str) -> String {
        self.tag("bound", login_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_depend_on_key_kind_and_value() {
        let a = Tagger::new(b"key one");
        let b = Tagger::new(b"key two");
        assert_eq!(a.account("x").len(), 16);
        assert_eq!(a.account("x"), a.account("x"));
        assert_ne!(a.account("x"), b.account("x"));
        assert_ne!(a.account("x"), a.device("x"));
        assert_ne!(a.account("x"), a.account("y"));
        assert_ne!(a.bound("x"), a.account("x"));
        // the addresses of one IPv6 /64 share a tag, IPv4 addresses do not
        assert_eq!(a.ip("2001:db8::1"), a.ip("2001:db8::2:3"));
        assert_ne!(a.ip("2001:db8::1"), a.ip("2001:db8:0:1::1"));
        assert_ne!(a.ip("10.0.0.1"), a.ip("10.0.0.2"));
    }
}

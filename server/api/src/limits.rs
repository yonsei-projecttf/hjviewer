// SPDX-License-Identifier: AGPL-3.0-only

//! Login limits (design v2 §9-2) and what keeps a flood of logins small.
//!
//! Counted in the database, so that a restart does not reset them:
//!
//! - failed logins per client address (step 2), an IPv6 address by its /64;
//! - failed logins per portal ID, for every ID typed, allowlisted or not (step 5);
//! - failed logins of a portal ID from the PCs already in its account (step 5), one limit for all
//!   of them, so that the owner can log in from their PC while others use up the ID's limit;
//! - successful logins per account and per client address (step 7), so that a known password
//!   cannot keep calling the SSO or piling up tokens.
//!
//! An attempt is reserved before the work that may fail and counts while it runs: a burst of
//! concurrent requests cannot slip past a limit. When the login ends, login.rs settles each
//! reserved attempt: it counts from then on for the length of its windows, or it is taken back.
//! Requests refused by a full limit are not counted themselves, so hammering does not extend a
//! lock.
//!
//! In memory: the SSO calls of all accounts together, of which one account may hold a quarter
//! (step 7); the pause of SSO calls while the SSO refuses this server or keeps failing; how long
//! the SSO took lately (the failure delay follows it, step 10); and how many failed logins a
//! minute are written to the records.

use crate::{config::Window, sso::Failure};
use rusqlite::{params, Connection};
use std::{
    collections::VecDeque,
    sync::Mutex,
    time::{Duration, Instant},
};

pub const SCOPE_IP: &str = "ip";
pub const SCOPE_ID: &str = "id";
pub const SCOPE_BOUND: &str = "bound";
pub const SCOPE_OK_ID: &str = "ok-id";
pub const SCOPE_OK_IP: &str = "ok-ip";

/// Reserves an attempt under `scope`/`key` unless a window is full; returns its row id.
pub fn reserve(
    c: &Connection,
    scope: &str,
    key: &str,
    windows: &[Window],
    at: i64,
) -> rusqlite::Result<Option<i64>> {
    for w in windows {
        let used: u32 = c.query_row(
            "SELECT COUNT(*) FROM attempts WHERE scope = ?1 AND key = ?2 AND at > ?3",
            params![scope, key, at - w.secs],
            |r| r.get(0),
        )?;
        if used >= w.count {
            return Ok(None);
        }
    }
    c.execute(
        "INSERT INTO attempts (scope, key, at, counts) VALUES (?1, ?2, ?3, 0)",
        params![scope, key, at],
    )?;
    Ok(Some(c.last_insert_rowid()))
}

/// Settles reserved attempts: those in `keep` count from now on, those in `back` are taken back.
pub fn settle(c: &Connection, keep: &[i64], back: &[i64]) -> rusqlite::Result<()> {
    for id in keep {
        c.execute("UPDATE attempts SET counts = 1 WHERE id = ?1", [id])?;
    }
    for id in back {
        c.execute("DELETE FROM attempts WHERE id = ?1", [id])?;
    }
    Ok(())
}

/// Clears the counters under `keys`, in every scope (admin unlock): tags of different kinds never
/// collide (log.rs), so an account's tag clears its ID's counters and an address's tag the
/// address's. Returns how many attempts went.
pub fn clear(c: &Connection, keys: &[String]) -> rusqlite::Result<usize> {
    let mut n = 0;
    for key in keys {
        n += c.execute("DELETE FROM attempts WHERE key = ?1", [key])?;
    }
    Ok(n)
}

/// After a restart nothing is running: attempts left unsettled are taken back.
pub fn drop_unfinished(c: &Connection) -> rusqlite::Result<usize> {
    c.execute("DELETE FROM attempts WHERE counts = 0", [])
}

/// Counted attempts per scope since `since` (report).
pub fn counted_since(c: &Connection, since: i64) -> rusqlite::Result<Vec<(String, i64)>> {
    let mut stmt = c.prepare(
        "SELECT scope, COUNT(*) FROM attempts WHERE counts = 1 AND at > ?1 GROUP BY scope ORDER BY scope",
    )?;
    let rows = stmt.query_map([since], |r| Ok((r.get(0)?, r.get(1)?)))?;
    rows.collect()
}

/// What [`GlobalLimiter::try_take`] decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Take {
    Taken,
    /// the window is full
    Full,
    /// this account holds its share of the window
    Share,
}

/// The SSO calls of all accounts together (v2 §9-2 step 7), in memory. One account may hold a
/// quarter of a window (at least one call), so that one known password cannot use up the budget
/// of everyone.
pub struct GlobalLimiter {
    window: Window,
    calls: Mutex<VecDeque<(Instant, String)>>,
}

impl GlobalLimiter {
    pub fn new(window: Window) -> Self {
        GlobalLimiter {
            window,
            calls: Mutex::new(VecDeque::new()),
        }
    }

    /// The calls of a window one account may hold.
    pub fn share(&self) -> usize {
        (self.window.count as usize / 4).max(1)
    }

    /// Takes a call for the account tagged `who` if the window has room for it.
    pub fn try_take(&self, who: &str) -> Take {
        let now = Instant::now();
        let span = Duration::from_secs(self.window.secs as u64);
        let mut calls = self.calls.lock().unwrap_or_else(|e| e.into_inner());
        while calls
            .front()
            .is_some_and(|(t, _)| now.duration_since(*t) >= span)
        {
            calls.pop_front();
        }
        if calls.len() >= self.window.count as usize {
            return Take::Full;
        }
        if calls.iter().filter(|(_, w)| w == who).count() >= self.share() {
            return Take::Share;
        }
        calls.push_back((now, who.to_owned()));
        Take::Taken
    }
}

/// After the SSO refused this server's address or its client credentials, calls pause this long.
pub const ADDRESS_PAUSE: Duration = Duration::from_secs(300);
/// After this many upstream errors in a row …
pub const UPSTREAM_ERRORS: u32 = 5;
/// … calls pause this long.
pub const UPSTREAM_PAUSE: Duration = Duration::from_secs(120);

/// Pauses SSO calls while the SSO refuses this server or keeps failing (sso.rs sorts the
/// outcomes): logins get the same 503 at once then, before the allowlist and without a call
/// (login.rs, design v3 §4-1), and the operator hears of it once rather than with every login.
/// The first call after a pause tries the SSO again.
///
/// It also keeps when a failure last paused the calls, and why, past the pause and past the next
/// answer: the five-minute check (check.rs, design v3 §5-9) reports a pause of the last ten
/// minutes even when it ran out between two checks.
#[derive(Default)]
pub struct Breaker {
    state: Mutex<BreakerState>,
    last: Mutex<Option<(i64, &'static str)>>,
}

#[derive(Default)]
struct BreakerState {
    upstream_in_a_row: u32,
    paused_until: Option<Instant>,
    /// what started the pause, for the report
    reason: &'static str,
}

impl Breaker {
    /// Whether calls are paused now.
    pub fn paused(&self) -> bool {
        self.pause_reason().is_some()
    }

    /// Why calls are paused now ("address", "client" or "upstream"), if they are.
    pub fn pause_reason(&self) -> Option<&'static str> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state
            .paused_until
            .is_some_and(|t| Instant::now() < t)
            .then_some(state.reason)
    }

    /// Notes how a call ended (`None`: the SSO answered, with a user or refused credentials);
    /// returns the pause this outcome started, if it started one.
    pub fn note(&self, failure: Option<Failure>) -> Option<Duration> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let was_paused = state.paused_until.is_some_and(|t| now < t);
        let (pause, reason) = match failure {
            None | Some(Failure::Credentials) => {
                *state = BreakerState::default();
                return None;
            }
            Some(Failure::AddressRefused) => (ADDRESS_PAUSE, "address"),
            Some(Failure::ClientRefused) => (ADDRESS_PAUSE, "client"),
            Some(Failure::Upstream) => {
                state.upstream_in_a_row += 1;
                if state.upstream_in_a_row < UPSTREAM_ERRORS {
                    return None;
                }
                (UPSTREAM_PAUSE, "upstream")
            }
        };
        state.paused_until = Some(now + pause);
        state.reason = reason;
        *self.last.lock().unwrap_or_else(|e| e.into_inner()) = Some((crate::util::now(), reason));
        (!was_paused).then_some(pause)
    }

    /// When a failure last paused the calls (Unix time) and why ("address", "client" or
    /// "upstream"), whether or not the pause still runs.
    pub fn last_pause(&self) -> Option<(i64, &'static str)> {
        *self.last.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Ends a pause and starts the count over, forgetting the last one too (tests; in service a
    /// pause simply runs out).
    pub fn reset(&self) {
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = BreakerState::default();
        *self.last.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// How long recent SSO calls took: failures that make no call wait as long as the slowest of the
/// calls of the last ten minutes (at most the last 64), so that an ID's answer takes as long
/// whether it is on the list or not, also while the SSO is slow (v2 §9-2 step 10).
#[derive(Default)]
pub struct Latency {
    samples: Mutex<VecDeque<(Instant, Duration)>>,
}

const LATENCY_SPAN: Duration = Duration::from_secs(600);
const LATENCY_SAMPLES: usize = 64;

impl Latency {
    pub fn note(&self, took: Duration) {
        let mut samples = self.samples.lock().unwrap_or_else(|e| e.into_inner());
        if samples.len() >= LATENCY_SAMPLES {
            samples.pop_front();
        }
        samples.push_back((Instant::now(), took));
    }

    /// The longest call of the last ten minutes (zero without calls).
    pub fn recent(&self) -> Duration {
        let mut samples = self.samples.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        samples.retain(|(at, _)| now.duration_since(*at) < LATENCY_SPAN);
        samples
            .iter()
            .map(|(_, took)| *took)
            .max()
            .unwrap_or_default()
    }
}

/// Failed logins written to the records per minute; more are counted, not written, so that a
/// flood from many addresses cannot fill the disk the server shares (successes are always
/// written, and the limits bound them).
pub const FAILURE_RECORDS_PER_MINUTE: u32 = 20;

/// Whether a failure record may be written now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admit {
    Write,
    /// not written; `first` for the first one this minute (to say so once in the journal)
    Skip {
        first: bool,
    },
}

#[derive(Default)]
pub struct RecordBudget {
    state: Mutex<RecordState>,
}

#[derive(Default)]
struct RecordState {
    minute: Option<Instant>,
    written: u32,
    skipped_this_minute: u32,
    skipped: u64,
}

impl RecordBudget {
    pub fn admit(&self) -> Admit {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        if s.minute
            .is_none_or(|m| now.duration_since(m) >= Duration::from_secs(60))
        {
            s.minute = Some(now);
            s.written = 0;
            s.skipped_this_minute = 0;
        }
        if s.written < FAILURE_RECORDS_PER_MINUTE {
            s.written += 1;
            return Admit::Write;
        }
        s.skipped_this_minute += 1;
        s.skipped += 1;
        Admit::Skip {
            first: s.skipped_this_minute == 1,
        }
    }

    /// The failure records not written since the last call (the daily summary).
    pub fn take_skipped(&self) -> u64 {
        std::mem::take(&mut self.state.lock().unwrap_or_else(|e| e.into_inner()).skipped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    #[test]
    fn windows_and_settling() {
        let c = open_in_memory().unwrap();
        let w = [
            Window {
                count: 3,
                secs: 600,
            },
            Window {
                count: 5,
                secs: 86400,
            },
        ];
        let mut ids = Vec::new();
        for _ in 0..3 {
            ids.push(reserve(&c, SCOPE_ID, "k", &w, 1000).unwrap().unwrap());
        }
        // in-flight attempts count
        assert_eq!(reserve(&c, SCOPE_ID, "k", &w, 1000).unwrap(), None);
        // other keys and scopes are apart
        assert!(reserve(&c, SCOPE_ID, "other", &w, 1000).unwrap().is_some());
        assert!(reserve(&c, SCOPE_BOUND, "k", &w, 1000).unwrap().is_some());
        // one taken back, two kept
        settle(&c, &ids[1..], &ids[..1]).unwrap();
        let id = reserve(&c, SCOPE_ID, "k", &w, 1000).unwrap().unwrap();
        settle(&c, &[id], &[]).unwrap();
        assert_eq!(reserve(&c, SCOPE_ID, "k", &w, 1000).unwrap(), None);
        // after ten minutes the short window is free, the day window counts on
        let id = reserve(&c, SCOPE_ID, "k", &w, 1601).unwrap().unwrap();
        settle(&c, &[id], &[]).unwrap();
        let id = reserve(&c, SCOPE_ID, "k", &w, 1602).unwrap().unwrap();
        settle(&c, &[id], &[]).unwrap();
        assert_eq!(reserve(&c, SCOPE_ID, "k", &w, 2300).unwrap(), None);
        assert_eq!(counted_since(&c, 0).unwrap(), vec![("id".to_owned(), 5)]);
        // clearing a key clears it in every scope
        assert_eq!(clear(&c, &["k".to_owned()]).unwrap(), 6);
        assert!(reserve(&c, SCOPE_ID, "k", &w, 2300).unwrap().is_some());
        assert_eq!(drop_unfinished(&c).unwrap(), 2);
    }

    #[test]
    fn global_with_shares() {
        let g = GlobalLimiter::new(Window { count: 2, secs: 60 });
        assert_eq!(g.share(), 1);
        assert_eq!(g.try_take("a"), Take::Taken);
        assert_eq!(g.try_take("a"), Take::Share);
        assert_eq!(g.try_take("b"), Take::Taken);
        assert_eq!(g.try_take("c"), Take::Full);
        let g = GlobalLimiter::new(Window {
            count: 20,
            secs: 60,
        });
        assert_eq!(g.share(), 5);
        for _ in 0..5 {
            assert_eq!(g.try_take("a"), Take::Taken);
        }
        assert_eq!(g.try_take("a"), Take::Share);
        for i in 0..15 {
            assert_eq!(g.try_take(&format!("x{i}")), Take::Taken);
        }
        assert_eq!(g.try_take("y"), Take::Full);
    }

    #[test]
    fn breaker() {
        let b = Breaker::default();
        assert!(!b.paused());
        // upstream errors pause after five in a row; an answer in between starts the count over
        for _ in 0..UPSTREAM_ERRORS - 1 {
            assert_eq!(b.note(Some(Failure::Upstream)), None);
        }
        assert_eq!(b.note(Some(Failure::Credentials)), None);
        for _ in 0..UPSTREAM_ERRORS - 1 {
            assert_eq!(b.note(Some(Failure::Upstream)), None);
        }
        assert!(!b.paused());
        assert_eq!(b.note(Some(Failure::Upstream)), Some(UPSTREAM_PAUSE));
        assert!(b.paused());
        assert_eq!(b.pause_reason(), Some("upstream"));
        // a refused address pauses at once, reported once
        b.reset();
        assert_eq!(b.note(Some(Failure::AddressRefused)), Some(ADDRESS_PAUSE));
        assert_eq!(b.note(Some(Failure::AddressRefused)), None);
        assert!(b.paused());
        assert_eq!(b.pause_reason(), Some("address"));
        assert_eq!(b.note(None), None);
        assert!(!b.paused());
        assert_eq!(b.pause_reason(), None);
        // so do refused client credentials (invalid_client)
        assert_eq!(b.note(Some(Failure::ClientRefused)), Some(ADDRESS_PAUSE));
        assert_eq!(b.pause_reason(), Some("client"));
        // the last pause outlives the pause and the next answer, for the check
        let (at, reason) = b.last_pause().unwrap();
        assert_eq!(reason, "client");
        assert!((crate::util::now() - at).abs() <= 1);
        assert_eq!(b.note(None), None);
        assert!(!b.paused());
        assert_eq!(b.last_pause().map(|(_, r)| r), Some("client"));
        b.reset();
        assert_eq!(b.last_pause(), None);
        // a failure that does not pause leaves no mark
        assert_eq!(b.note(Some(Failure::Upstream)), None);
        assert_eq!(b.last_pause(), None);
    }

    #[test]
    fn latency() {
        let l = Latency::default();
        assert_eq!(l.recent(), Duration::ZERO);
        l.note(Duration::from_millis(300));
        l.note(Duration::from_millis(1800));
        l.note(Duration::from_millis(200));
        assert_eq!(l.recent(), Duration::from_millis(1800));
        for _ in 0..LATENCY_SAMPLES {
            l.note(Duration::from_millis(100));
        }
        assert_eq!(l.recent(), Duration::from_millis(100));
    }

    #[test]
    fn failure_records_per_minute() {
        let r = RecordBudget::default();
        for _ in 0..FAILURE_RECORDS_PER_MINUTE {
            assert_eq!(r.admit(), Admit::Write);
        }
        assert_eq!(r.admit(), Admit::Skip { first: true });
        assert_eq!(r.admit(), Admit::Skip { first: false });
        assert_eq!(r.take_skipped(), 2);
        assert_eq!(r.take_skipped(), 0);
    }
}

// SPDX-License-Identifier: AGPL-3.0-only

//! Support codes (지원 코드, docs/access-tickets.md 3-7, 1.8.2): a staff member who is logged in
//! makes a code and gives it to someone who is not; that person's PC trades it, once and within
//! [`CODE_SECS`], for a `guest`/`pc` ticket of its own that lives [`TICKET_SECS`]. The public
//! access password need not be handed out for it.
//!
//! ```text
//! POST /api/hjv/support-code  {"use":"pc"}                        a staff member's app: token and X-HJV-Proof
//! 200 {"code":"K7M2-Q9XD","use":"pc","expires_in":600}
//! POST /api/hjv/guest  {"use":"pc","id","pk","sig","code":"hjv-sealed-v1.<kid>.<sealed>"}   (guest.rs)
//! ```
//!
//! - Making one ([`handle`]): an account path (api.rs `session`), counted with the account key's
//!   other calls (keys.rs, 30 a minute); the App Review account gets 403 `no_code` ([`may_make`],
//!   which the login answer and currentUser tell the app as `"hjv":{"support_code":true}`); at most
//!   [`OPEN_PER_ACCOUNT`] open codes per account (429 `too_many_codes` with `retry_after`) and
//!   [`MAX_HELD`] in all (503 `busy`); without a ticket key 503 `unavailable`.
//! - The code: eight symbols of Crockford's base32 from 40 bits of the operating system's random
//!   source, shown `XXXX-XXXX`, read as a short link's code is (update.rs `typed_code`, without
//!   the URL's `%` escapes): at most 64 bytes, ASCII only, without regard to case, skipping `-`
//!   and the space U+0020 only, `O` as `0`, `I` and `L` as `1` ([`read`]); anything else (a tab,
//!   another space, a full-width letter, `%`) makes it a wrong code.
//! - Trading one is guest.rs's request with `code` in the place of `password`, sealed with the
//!   purpose [`PURPOSE`]; its checks are the access password's, with [`SupportCodes::find`] where
//!   the hash is, the maker looked at in the database (the account blocked, deleted or off the
//!   allowlist: `blocked`; the account key that made it ended, by a logout, a revoke, a block or
//!   a new login of the same device: `revoked`; either ends the code for good), and
//!   [`SupportCodes::take`] with the ticket, so that of two requests with one code only one gets
//!   it.
//! - Kept in memory only ([`SupportCodes`]): under an HMAC-SHA256 of the code with a key made at
//!   the start, with the maker (account number and tag, the account key's ckid), its use and its
//!   times, and from 1.9.0 its support link (by an HMAC too), its waiting number as it is, the
//!   downloads left, the PC that used it (ID, key, the ticket's `exp`) and, while the code is
//!   open, the code itself for the link's page: it is dropped (zeroized) once the code is used,
//!   ended or past its ten minutes, at the latest at the upkeep's next five-second round
//!   (jobs.rs runs [`SupportCodes::sweep`]); never in the database, a file, the journal or a record. A code that was used, ended
//!   or whose ten minutes are over stays [`ENDED_KEPT_SECS`] more, so that the records tell
//!   `used`, `revoked` and `expired` from `wrong`. A restart ends them all. The answer that
//!   carries a code is kept, as every signed request's that changes something, for its retry
//!   within HJV_API_RETRY_WINDOW (api.rs `authenticate`); a retry gets it as it was, its
//!   `expires_in` too.
//! - Records `code` (kept HJV_API_EVENT_DAYS): `made` with the maker's tag; each try to trade one
//!   with its result, the address's tag and, once the code was found, the maker's tag (guest.rs).
//!   Never the code or a tag of it, nor a link, a waiting number, a grant or the PC's ID; a
//!   support link's package download is an `update` record (update.rs, `support-windows`).
//!
//! - TV-W6 (1.9.0): the answer that makes a code also brings its support link's path
//!   (`/api/hjv/s/<22>`, a 128-bit value apart from the code: update.rs shows the code and hands
//!   out HJViewer through it while the code is open, [`LINK_DOWNLOADS`] package downloads) and its
//!   waiting number (`wait`, 128 bits too), both kept with the code under the same HMAC key. The
//!   app that made it holds [`WAIT_PATH`] ([`wait_handle`], at most [`WAIT_SECS`]) to hear of the
//!   PC that used it; only the account key that made a code sees it there. Trading a code also
//!   signs the support grant (grant.rs) for that PC with the list key.
//!
//! ```text
//! 200 {"code":"K7M2-Q9XD","use":"pc","expires_in":600,"link":"/api/hjv/s/<22>","wait":"<22>"}
//! GET /api/hjv/support-code/wait?rev=<n>                          the maker's app: token and X-HJV-Proof
//! 200 {"rev":18,"codes":[{"wait","state":"open","expires_in"},{"wait","state":"used","pc","pk","until_in"},{"wait","state":"over"}]}
//! ```
//!
//! The numbers are constants, not settings; the user may change [`TICKET_SECS`], which two texts
//! of the app say (docs/access-tickets.md 5-3, S2 and P3). The support grant lives the ticket's
//! time but at most two hours (grant.rs `MAX_LIFE`, the apps' MAX_GRANT_LIFE): a longer ticket
//! leaves the staff mark at two hours unless the apps' bound, the vector file's `max_life` and
//! `MAX_LIFE` change with a new app release. The `use` of both requests is where a controller's
//! code would go; 1.9.0 takes `pc` only.

use crate::{
    api::{session, ApiRequest, Reply},
    db,
    keys::{CALLS_PER_MINUTE, TEXT_TOO_OFTEN},
    review,
    state::AppState,
    ticket::Use,
    update::{code_of, shown, typed_code},
    util::{b64, b64url, now, random_bytes, DAY, HOUR},
};
use hmac::{Hmac, Mac};
use hyper::StatusCode;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Sha256;
use std::{
    collections::HashMap,
    fmt::Write as _,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard,
    },
    time::Duration,
};
use tokio::sync::Notify;
use zeroize::Zeroizing;

pub const PATH: &str = "/api/hjv/support-code";
/// The purpose of a support code's seal (seal.rs), which only `code` takes (guest.rs).
pub const PURPOSE: &str = "code";
/// How long a code can be traded after it was made.
pub const CODE_SECS: i64 = 600;
/// The life of the ticket a code is traded for (`exp − iat`; docs/access-tickets.md 2-4).
pub const TICKET_SECS: i64 = 2 * HOUR;
/// The open codes (made, neither used nor over) one account holds at most.
pub const OPEN_PER_ACCOUNT: usize = 5;
/// The codes held in memory at most, the ended ones with them.
pub const MAX_HELD: usize = 1000;
/// How long a used or expired code stays, marked so, for the records.
pub const ENDED_KEPT_SECS: i64 = 600;
/// The waiting room (TV-W6): the path, the longest hold, the holds of one account key at once
/// and of all.
pub const WAIT_PATH: &str = "/api/hjv/support-code/wait";
pub const WAIT_SECS: u64 = 20;
pub const HOLDS_PER_KEY: usize = 2;
pub const MAX_WAITS: usize = 200;
/// Where a support link lives (update.rs), and its package downloads.
pub const LINK_PATH: &str = "/api/hjv/s/";
pub const LINK_DOWNLOADS: u8 = 3;

/// The one answer to a code that is not there, not of the form, over, used, or whose account is
/// blocked or gone (docs/access-tickets.md 3-7, E6).
pub const TEXT_DENIED: &str = "지원 코드가 맞지 않거나, 10분이 지났거나, 이미 쓴 코드입니다.";
pub const TEXT_NO_CODE: &str = "이 계정으로는 지원 코드를 만들 수 없습니다.";
/// S6 without the minutes (the app words the wait from `retry_after`).
pub const TEXT_TOO_MANY: &str = "아직 쓰지 않은 지원 코드가 많습니다. 잠시 뒤 다시 시도해 주세요.";
pub const TEXT_CANNOT_MAKE: &str =
    "지금은 지원 코드를 만들 수 없습니다. 잠시 뒤 다시 시도해 주세요.";
/// Trading a code: hbbs or the ticket key is not there (E7).
pub const TEXT_UNAVAILABLE: &str =
    "지금은 지원 코드를 확인할 수 없습니다. 잠시 뒤 다시 시도해 주세요.";
/// Trading a code: hbbs holds another key for the PC (E8).
pub const TEXT_WRONG_PC: &str =
    "이 PC의 키가 서버에 있는 것과 달라 지원 코드로 연결받을 수 없습니다. 관리자에게 알려 주세요.";
const TEXT_BAD_REQUEST: &str = "요청을 이해하지 못했습니다.";

/// Whether a session of the account of `login_key` may make support codes: any staff account's,
/// never the App Review account's. The login answer and currentUser carry `"support_code":true`
/// in their `hjv` object for it (login.rs, api.rs), and the app shows S1 only then.
pub fn may_make(login_key: &str) -> bool {
    !review::is_review(login_key)
}

/// The eight symbols a person typed, as a short link's code is read but for the URL's `%`
/// escapes: at most 64 bytes, ASCII, case, `-` and U+0020 skipped, `O` as `0`, `I` and `L` as
/// `1`; `None` for anything else, or what is not eight symbols then. The app's own check of the
/// form (docs/access-tickets.md 3-7) is this function, ported as it is.
pub fn read(typed: &str) -> Option<String> {
    if typed.contains('%') {
        return None;
    }
    typed_code(typed)
}

/// A new code's eight symbols, from 40 random bits.
fn new_code() -> String {
    code_of(random_bytes::<5>())
}

/// Where a code is kept: the HMAC of its symbols.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CodeKey([u8; 32]);

/// A support link's value as hjv-api looks it up: its HMAC under the same key, apart from codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct LinkKey([u8; 32]);

/// A code's eight symbols, kept for its support link's page only; never in a `Debug`.
#[derive(Clone)]
struct Symbols(Zeroizing<String>);

impl std::fmt::Debug for Symbols {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Symbols(..)")
    }
}

/// The account, and its account key, that made a code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Maker {
    pub account_id: i64,
    /// its tag (log.rs `Tagger::account`)
    pub account_tag: String,
    /// the account key the request to make it was signed with: the code ends with it
    pub ckid: i64,
}

/// The PC that traded a code, once its ticket was signed (TV-W6): what the waiting room tells
/// the maker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsedBy {
    /// the trade's `id` and `pk`, which the PC's signature held for
    pub pc: String,
    pub pk: [u8; 32],
    /// the ticket's `exp`
    pub exp: i64,
}

#[derive(Debug, Clone)]
struct Entry {
    maker: Maker,
    use_: Use,
    expires: i64,
    used: Option<i64>,
    /// when it was found that its maker's key had ended ([`SupportCodes::end_keys`])
    revoked: Option<i64>,
    /// TV-W6: its support link, by the HMAC of the link's value
    link: LinkKey,
    /// the symbols, for the support link's page while the code is open: `None` (zeroized) once it
    /// is used, ended or over ([`Held::sweep`], [`SupportCodes::mark_used`],
    /// [`SupportCodes::end_keys`])
    symbols: Option<Symbols>,
    /// the waiting number by which the maker's app knows the code in the waiting room
    wait: String,
    /// the package downloads its support link has left
    downloads_left: u8,
    /// set with the ticket, after [`SupportCodes::take`]
    used_by: Option<UsedBy>,
    /// whether the waiting room heard that its ten minutes are over
    over_told: bool,
}

impl Entry {
    fn open(&self, at: i64) -> bool {
        self.used.is_none() && self.revoked.is_none() && at < self.expires
    }

    /// When it was used or ended, or else when its time is over.
    fn ended(&self) -> i64 {
        [self.used, self.revoked]
            .into_iter()
            .flatten()
            .fold(self.expires, i64::min)
    }

    fn kept(&self, at: i64) -> bool {
        self.open(at) || at < self.ended().saturating_add(ENDED_KEPT_SECS)
    }

    /// What the waiting room says of it at `at`. A code taken whose ticket is not signed yet is
    /// still `open` there: it may be given back.
    fn waiting(&self, at: i64) -> Value {
        match &self.used_by {
            Some(by) => json!({
                "wait": self.wait,
                "state": "used",
                "pc": by.pc,
                "pk": b64(&by.pk),
                "until_in": (by.exp - at).max(0),
            }),
            None if self.revoked.is_none() && at < self.expires => json!({
                "wait": self.wait,
                "state": "open",
                "expires_in": self.expires - at,
            }),
            None => json!({ "wait": self.wait, "state": "over" }),
        }
    }
}

/// What a code typed at a PC is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Looked {
    /// no such code (of the form or not), or one made for another use
    Wrong,
    Found {
        key: CodeKey,
        maker: Maker,
        state: CodeState,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeState {
    Open,
    Expired,
    Used,
    /// its maker's account key ended
    Revoked,
}

/// Why no code was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// the account holds its [`OPEN_PER_ACCOUNT`] open codes; the first ends in this many seconds
    TooMany { retry_after: u64 },
    /// [`MAX_HELD`] open codes are held
    Busy,
}

/// What [`SupportCodes::take`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Taken {
    Taken,
    Used,
    Revoked,
    Expired,
    Gone,
}

/// A code made: its symbols, its support link's value and its waiting number (each 16 random
/// bytes in base64url without padding, 22 characters).
#[derive(Clone, PartialEq, Eq)]
pub struct Made {
    pub code: String,
    pub link: String,
    pub wait: String,
}

impl std::fmt::Debug for Made {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Made(..)")
    }
}

/// An open code's support link, found by its value.
#[derive(Clone, PartialEq, Eq)]
pub struct LinkFound {
    pub key: CodeKey,
    pub maker: Maker,
    /// the code as shown, `XXXX-XXXX`
    pub code: String,
    pub downloads_left: u8,
}

impl std::fmt::Debug for LinkFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "LinkFound({:?}, {} left)",
            self.maker, self.downloads_left
        )
    }
}

/// The waiting room's answer for an account key at a moment.
#[derive(Debug, Clone, PartialEq)]
pub struct Waiting {
    pub rev: u64,
    pub codes: Vec<Value>,
    /// when the first of its open codes ends: a hold lasts no longer
    pub first_end: Option<i64>,
}

/// A waiting-room request held; the counts go down when it is dropped.
pub struct WaitHold<'a> {
    codes: &'a SupportCodes,
    ckid: i64,
}

impl Drop for WaitHold<'_> {
    fn drop(&mut self) {
        let mut holds = self.codes.holds();
        if let Some(n) = holds.get_mut(&self.ckid) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                holds.remove(&self.ckid);
            }
        }
    }
}

/// The codes and the revision of each account key's (the waiting room's `rev`), under one lock.
#[derive(Default)]
struct Held {
    codes: HashMap<CodeKey, Entry>,
    revs: HashMap<i64, u64>,
}

impl Held {
    fn bump(&mut self, ckid: i64) {
        *self.revs.entry(ckid).or_insert(0) += 1;
    }

    /// Forgets the codes past [`ENDED_KEPT_SECS`], and marks those whose ten minutes are over:
    /// the waiting room hears of both. Drops the symbols of every code that is not open but for
    /// one taken a moment ago whose ticket is not signed yet (it may be given back). Whether
    /// anything changed.
    fn sweep(&mut self, at: i64) -> bool {
        let mut changed: Vec<i64> = Vec::new();
        self.codes.retain(|_, e| {
            if !e.kept(at) {
                changed.push(e.maker.ckid);
                return false;
            }
            if e.revoked.is_some() || e.used_by.is_some() || at >= e.expires {
                e.symbols = None;
            }
            if e.used.is_none() && e.revoked.is_none() && at >= e.expires && !e.over_told {
                e.over_told = true;
                changed.push(e.maker.ckid);
            }
            true
        });
        for ckid in &changed {
            self.bump(*ckid);
        }
        !changed.is_empty()
    }
}

/// The support codes, in memory only, under an HMAC with a key made at the start; and the
/// waiting room's signal and holds (TV-W6).
pub struct SupportCodes {
    key: Zeroizing<[u8; 32]>,
    held: Mutex<Held>,
    /// wakes the held waiting-room requests: a code made, used or ended
    changed: Notify,
    /// the waiting-room requests held, per account key
    holds: Mutex<HashMap<i64, usize>>,
    /// the service is stopping: no waiting-room request is held any more
    stopping: AtomicBool,
}

impl Default for SupportCodes {
    fn default() -> Self {
        SupportCodes {
            key: Zeroizing::new(random_bytes::<32>()),
            held: Mutex::default(),
            changed: Notify::new(),
            holds: Mutex::default(),
            stopping: AtomicBool::new(false),
        }
    }
}

/// A support link's value or a waiting number: 16 random bytes, 22 characters.
fn new_value() -> String {
    b64url(&random_bytes::<16>())
}

/// Whether `text` has the form of a support link's value.
fn is_value(text: &str) -> bool {
    text.len() == 22
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

impl SupportCodes {
    fn held(&self) -> MutexGuard<'_, Held> {
        self.held.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn holds(&self) -> MutexGuard<'_, HashMap<i64, usize>> {
        self.holds.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn mac(&self, parts: &[&[u8]]) -> [u8; 32] {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&*self.key).expect("HMAC takes keys of any length");
        for part in parts {
            mac.update(part);
        }
        mac.finalize().into_bytes().into()
    }

    fn key_of(&self, code: &str) -> CodeKey {
        CodeKey(self.mac(&[code.as_bytes()]))
    }

    /// A support link's key: apart from the codes', whose eight symbols are never 22.
    fn link_key(&self, link: &str) -> LinkKey {
        LinkKey(self.mac(&[b"link\0", link.as_bytes()]))
    }

    /// The waiting room looks again.
    fn changed(&self) {
        self.changed.notify_waiters();
    }

    /// Makes a code for the account `maker` and `use_` at `at`: its eight symbols, its support
    /// link and its waiting number.
    pub fn make(&self, maker: Maker, use_: Use, at: i64) -> Result<Made, Refused> {
        let mut held = self.held();
        let swept = held.sweep(at);
        let mut evicted = false;
        let made = self.make_in(&mut held, maker, use_, at, &mut evicted);
        drop(held);
        // the waiting room hears of ended codes that made room, a code refused `busy` or not
        if swept || evicted || made.is_ok() {
            self.changed();
        }
        made
    }

    /// Runs the sweep at `at` (jobs.rs, every five seconds): an over code's symbols go, and the
    /// waiting room hears of it, without waiting for the next request.
    pub fn sweep(&self, at: i64) {
        let swept = self.held().sweep(at);
        if swept {
            self.changed();
        }
    }

    fn make_in(
        &self,
        held: &mut Held,
        maker: Maker,
        use_: Use,
        at: i64,
        evicted: &mut bool,
    ) -> Result<Made, Refused> {
        let codes = &mut held.codes;
        let first_end = codes
            .values()
            .filter(|e| e.maker.account_id == maker.account_id && e.open(at))
            .map(|e| e.expires);
        let (open, first_end) =
            first_end.fold((0, i64::MAX), |(n, first), exp| (n + 1, first.min(exp)));
        if open >= OPEN_PER_ACCOUNT {
            let retry_after = u64::try_from(first_end - at).unwrap_or(0).max(1);
            return Err(Refused::TooMany { retry_after });
        }
        if codes.len() >= MAX_HELD {
            // the ended ones go, the oldest first
            let mut ended: Vec<(i64, CodeKey)> = codes
                .iter()
                .filter(|(_, e)| !e.open(at))
                .map(|(k, e)| (e.ended(), *k))
                .collect();
            ended.sort_unstable_by_key(|(when, _)| *when);
            let excess = codes.len() + 1 - MAX_HELD;
            let mut gone = Vec::new();
            for (_, k) in ended.into_iter().take(excess) {
                if let Some(e) = codes.remove(&k) {
                    gone.push(e.maker.ckid);
                }
            }
            *evicted = !gone.is_empty();
            if codes.len() >= MAX_HELD {
                for ckid in gone {
                    held.bump(ckid);
                }
                return Err(Refused::Busy);
            }
            for ckid in gone {
                held.bump(ckid);
            }
        }
        let codes = &mut held.codes;
        let (link, wait) = loop {
            let (link, wait) = (new_value(), new_value());
            let key = self.link_key(&link);
            if link != wait && !codes.values().any(|e| e.link == key || e.wait == wait) {
                break (link, wait);
            }
        };
        let ckid = maker.ckid;
        loop {
            let code = new_code();
            let key = self.key_of(&code);
            if let std::collections::hash_map::Entry::Vacant(slot) = codes.entry(key) {
                slot.insert(Entry {
                    maker,
                    use_,
                    expires: at.saturating_add(CODE_SECS),
                    used: None,
                    revoked: None,
                    link: self.link_key(&link),
                    symbols: Some(Symbols(Zeroizing::new(code.clone()))),
                    wait: wait.clone(),
                    downloads_left: LINK_DOWNLOADS,
                    used_by: None,
                    over_told: false,
                });
                held.bump(ckid);
                return Ok(Made { code, link, wait });
            }
        }
    }

    /// What `typed` is at `at` for a ticket of `use_`.
    pub fn find(&self, typed: &str, use_: Use, at: i64) -> Looked {
        let Some(code) = read(typed) else {
            return Looked::Wrong;
        };
        let key = self.key_of(&code);
        let mut held = self.held();
        if held.sweep(at) {
            self.changed();
        }
        match held.codes.get(&key) {
            Some(e) if e.use_ == use_ => Looked::Found {
                key,
                maker: e.maker.clone(),
                state: match (e.used, e.revoked, e.open(at)) {
                    (Some(_), _, _) => CodeState::Used,
                    (None, Some(_), _) => CodeState::Revoked,
                    (None, None, true) => CodeState::Open,
                    (None, None, false) => CodeState::Expired,
                },
            },
            _ => Looked::Wrong,
        }
    }

    /// Uses the code of `key` at `at`, if it is still open: one call of all that come at once
    /// gets [`Taken::Taken`]. The waiting room hears of it with [`SupportCodes::mark_used`].
    pub fn take(&self, key: CodeKey, at: i64) -> Taken {
        let mut held = self.held();
        match held.codes.get_mut(&key) {
            None => Taken::Gone,
            Some(e) if e.used.is_some() => Taken::Used,
            Some(e) if e.revoked.is_some() => Taken::Revoked,
            Some(e) if !e.open(at) => Taken::Expired,
            Some(e) => {
                e.used = Some(at);
                Taken::Taken
            }
        }
    }

    /// Opens a code taken a moment ago again, when no ticket could be signed for it.
    pub fn give_back(&self, key: CodeKey) {
        if let Some(e) = self.held().codes.get_mut(&key) {
            e.used = None;
        }
    }

    /// The ticket of a code taken is signed: the waiting room tells its maker which PC it was.
    pub fn mark_used(&self, key: CodeKey, by: UsedBy) {
        let mut held = self.held();
        let Some(e) = held.codes.get_mut(&key) else {
            return;
        };
        if e.used.is_none() {
            return;
        }
        e.used_by = Some(by);
        e.symbols = None;
        let ckid = e.maker.ckid;
        held.bump(ckid);
        drop(held);
        self.changed();
    }

    /// The account keys that made the account `account_id`'s codes open at `at`, each once.
    pub fn open_keys(&self, account_id: i64, at: i64) -> Vec<i64> {
        let mut ckids: Vec<i64> = self
            .held()
            .codes
            .values()
            .filter(|e| e.maker.account_id == account_id && e.open(at))
            .map(|e| e.maker.ckid)
            .collect();
        ckids.sort_unstable();
        ckids.dedup();
        ckids
    }

    /// Ends at `at` the open codes of the account `account_id` made with the account keys
    /// `ckids`, which have ended: for good, as a used code; how many it ended.
    pub fn end_keys(&self, account_id: i64, ckids: &[i64], at: i64) -> usize {
        let mut held = self.held();
        let mut ended = Vec::new();
        for e in held.codes.values_mut() {
            if e.maker.account_id == account_id && ckids.contains(&e.maker.ckid) && e.open(at) {
                e.revoked = Some(at);
                e.symbols = None;
                ended.push(e.maker.ckid);
            }
        }
        for ckid in &ended {
            held.bump(*ckid);
        }
        drop(held);
        if !ended.is_empty() {
            self.changed();
        }
        ended.len()
    }

    /// The open code of the support link `link` at `at` (TV-W6): not used, not ended, within its
    /// ten minutes. `None` for any other value, of the form or not.
    pub fn find_link(&self, link: &str, at: i64) -> Option<LinkFound> {
        if !is_value(link) {
            return None;
        }
        let key = self.link_key(link);
        let held = self.held();
        // at most MAX_HELD, compared by their HMAC: the time says nothing of the value
        let (code_key, e) = held
            .codes
            .iter()
            .find(|(_, e)| e.link == key && e.open(at))?;
        Some(LinkFound {
            key: *code_key,
            maker: e.maker.clone(),
            code: shown(&e.symbols.as_ref()?.0),
            downloads_left: e.downloads_left,
        })
    }

    /// Takes one package download of the open code of `key`'s link: how many it has left then,
    /// `None` when the code is not open or the link has none left.
    pub fn take_download(&self, key: CodeKey, at: i64) -> Option<u8> {
        let mut held = self.held();
        let e = held
            .codes
            .get_mut(&key)
            .filter(|e| e.open(at) && e.downloads_left > 0)?;
        e.downloads_left -= 1;
        Some(e.downloads_left)
    }

    /// The waiting room of the account key `ckid` at `at`: its revision and the codes it made
    /// that are held, in the order they were made.
    pub fn waiting(&self, ckid: i64, at: i64) -> Waiting {
        let mut held = self.held();
        let swept = held.sweep(at);
        let mut mine: Vec<&Entry> = held
            .codes
            .values()
            .filter(|e| e.maker.ckid == ckid)
            .collect();
        mine.sort_by(|a, b| (a.expires, &a.wait).cmp(&(b.expires, &b.wait)));
        let first_end = mine
            .iter()
            .filter(|e| e.used_by.is_none() && e.revoked.is_none() && at < e.expires)
            .map(|e| e.expires)
            .min();
        let waiting = Waiting {
            rev: held.revs.get(&ckid).copied().unwrap_or(0),
            codes: mine.iter().map(|e| e.waiting(at)).collect(),
            first_end,
        };
        drop(held);
        if swept {
            self.changed();
        }
        waiting
    }

    /// Holds a waiting-room request of `ckid`, unless [`HOLDS_PER_KEY`] of it or [`MAX_WAITS`] in
    /// all are held already, or the service is stopping.
    pub fn hold(&self, ckid: i64) -> Option<WaitHold<'_>> {
        if self.stopping() {
            return None;
        }
        let mut holds = self.holds();
        let all: usize = holds.values().sum();
        let n = holds.entry(ckid).or_insert(0);
        if *n >= HOLDS_PER_KEY || all >= MAX_WAITS {
            if *n == 0 {
                holds.remove(&ckid);
            }
            return None;
        }
        *n += 1;
        Some(WaitHold { codes: self, ckid })
    }

    /// The waiting-room requests held now.
    pub fn held_waits(&self) -> usize {
        self.holds().values().sum()
    }

    /// The service stops: the held waiting-room requests are answered now.
    pub fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        self.changed();
    }

    fn stopping(&self) -> bool {
        self.stopping.load(Ordering::SeqCst)
    }

    /// The codes open at `at`, and those held (open or ended).
    pub fn counts(&self, at: i64) -> (usize, usize) {
        let held = self.held();
        let kept = held.codes.values().filter(|e| e.kept(at)).count();
        let open = held.codes.values().filter(|e| e.open(at)).count();
        (open, kept)
    }

    /// The support links of the codes open at `at` that have downloads left.
    pub fn links_left(&self, at: i64) -> usize {
        self.held()
            .codes
            .values()
            .filter(|e| e.open(at) && e.downloads_left > 0)
            .count()
    }

    /// Moves every code's times `secs` into the past (tests: a code's ten minutes without the
    /// wait).
    pub fn age_by(&self, secs: i64) {
        for e in self.held().codes.values_mut() {
            e.expires -= secs;
            for when in [&mut e.used, &mut e.revoked].into_iter().flatten() {
                *when -= secs;
            }
            if let Some(by) = &mut e.used_by {
                by.exp -= secs;
            }
        }
    }
}

fn refusal(status: StatusCode, text: &str, code: &str) -> Reply {
    Reply::json(status, &json!({ "error": text, "hjv": code }))
}

/// The body of a request to make one: exactly `{"use":"pc"}`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MakeBody {
    #[serde(rename = "use")]
    use_: String,
}

fn make_body(body: &[u8]) -> Result<Use, &'static str> {
    // an object of strings first: serde's derive takes a null as no member, and an array as the
    // members in their order; the derive refuses a member twice
    let strings = matches!(
        serde_json::from_slice::<Value>(body),
        Ok(Value::Object(members)) if members.values().all(Value::is_string)
    );
    let body: MakeBody = strings
        .then(|| serde_json::from_slice(body).ok())
        .flatten()
        .ok_or("not an object of use")?;
    match body.use_.as_str() {
        "pc" => Ok(Use::Pc),
        _ => Err("a use other than pc"),
    }
}

/// Handles `POST /api/hjv/support-code` (the caller checked the method).
pub async fn handle(state: &Arc<AppState>, req: &ApiRequest) -> Reply {
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
                "support code: more than {CALLS_PER_MINUTE} calls a minute from one key (acct={tag}, ckid={})",
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
    let use_ = match make_body(body) {
        Ok(use_) => use_,
        Err(why) => {
            if state.first_notice(&format!("support-request:{tag}"), Duration::from_secs(600)) {
                log_info!(
                    "support code: a request not of the form ({why}; acct={tag}); the next ten minutes are not logged"
                );
            }
            return refusal(StatusCode::BAD_REQUEST, TEXT_BAD_REQUEST, "bad_request");
        }
    };
    if !may_make(&s.account.login_key) {
        return refusal(StatusCode::FORBIDDEN, TEXT_NO_CODE, "no_code");
    }
    if state.ticket_key.is_none() {
        return refusal(
            StatusCode::SERVICE_UNAVAILABLE,
            TEXT_CANNOT_MAKE,
            "unavailable",
        );
    }
    end_ended_keys(state, s.account.id, own.ckid).await;
    let maker = Maker {
        account_id: s.account.id,
        account_tag: tag.clone(),
        ckid: own.ckid,
    };
    match state.support.make(maker, use_, now()) {
        Ok(made) => {
            state.record("code", Some(tag), None, None, "made").await;
            Reply::ok(&json!({
                "code": shown(&made.code),
                "use": use_.as_str(),
                "expires_in": CODE_SECS,
                "link": format!("{LINK_PATH}{}", made.link),
                "wait": made.wait,
            }))
        }
        Err(Refused::TooMany { retry_after }) => {
            if state.first_notice(&format!("support-open:{tag}"), Duration::from_secs(600)) {
                log_warn!(
                    "support code: an account holds its {OPEN_PER_ACCOUNT} open codes (acct={tag}); the next ten minutes are not logged"
                );
            }
            Reply::json(
                StatusCode::TOO_MANY_REQUESTS,
                &json!({ "error": TEXT_TOO_MANY, "hjv": "too_many_codes", "retry_after": retry_after }),
            )
        }
        Err(Refused::Busy) => {
            if state.first_notice("support-busy", Duration::from_secs(60)) {
                log_warn!("support code: {MAX_HELD} open codes are held, no more made");
            }
            refusal(StatusCode::SERVICE_UNAVAILABLE, TEXT_CANNOT_MAKE, "busy")
        }
    }
}

/// Before the account `account_id` makes a code with its key `own`: ends its open codes whose
/// account keys have ended since (a logout, a revoke), so that they hold no place of the
/// [`OPEN_PER_ACCOUNT`]. A trade would refuse them anyway (guest.rs `find_code`); on a database
/// error they stay until then.
async fn end_ended_keys(state: &Arc<AppState>, account_id: i64, own: i64) {
    let at = now();
    let others: Vec<i64> = state
        .support
        .open_keys(account_id, at)
        .into_iter()
        .filter(|ckid| *ckid != own)
        .collect();
    if others.is_empty() {
        return;
    }
    let looked = others.clone();
    let active = state
        .db
        .call(move |c| {
            let mut stmt = c.prepare(
                "SELECT EXISTS (SELECT 1 FROM ctl_keys
                                WHERE ckid = ?1 AND account_id = ?2 AND revoked_at IS NULL)",
            )?;
            looked
                .iter()
                .map(|ckid| stmt.query_row([*ckid, account_id], |r| r.get::<_, bool>(0)))
                .collect::<rusqlite::Result<Vec<bool>>>()
        })
        .await;
    match active {
        Ok(active) => {
            let ended: Vec<i64> = others
                .iter()
                .zip(active)
                .filter(|(_, active)| !active)
                .map(|(ckid, _)| *ckid)
                .collect();
            if !ended.is_empty() {
                state.support.end_keys(account_id, &ended, at);
            }
        }
        Err(e) => log_err!("support code: {e}"),
    }
}

/// The account key that made a code, as the database has it now: its public key and the device
/// its login came from, which the support grant names (grant.rs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MakerKey {
    pub pk: [u8; 32],
    pub ctl_id: String,
}

/// Whether a code's maker still stands: its account there, the same account, on the allowlist
/// and not blocked, and the account key that made it not ended (a logout, a revoke, a block).
/// `Err` with the result to record (`blocked`, `revoked`) ends the maker key's open codes for
/// good: neither an unblock nor a new login of the account opens them again. The outer `Err` is a
/// database error.
pub async fn standing(
    state: &Arc<AppState>,
    maker: &Maker,
) -> Result<Result<MakerKey, &'static str>, String> {
    let (id, ckid) = (maker.account_id, maker.ckid);
    let found = state
        .db
        .call(move |c| match db::account(c, id)? {
            Some(account) => {
                let allowed = db::allowed(c, &account.login_key)?;
                let key: Option<(Vec<u8>, String)> = {
                    let mut stmt = c.prepare(
                        "SELECT pk, ctl_id FROM ctl_keys
                         WHERE ckid = ?1 AND account_id = ?2 AND revoked_at IS NULL",
                    )?;
                    let mut rows = stmt.query([ckid, id])?;
                    match rows.next()? {
                        Some(row) => Some((row.get(0)?, row.get(1)?)),
                        None => None,
                    }
                };
                Ok(Some((account.login_key, allowed, key)))
            }
            None => Ok(None),
        })
        .await
        .map_err(|e| e.to_string())?;
    let standing = match found {
        Some((login_key, allowed, key)) => {
            if !allowed || state.tags.account(&login_key) != maker.account_tag {
                Err("blocked")
            } else {
                match key.and_then(|(pk, ctl_id)| Some((<[u8; 32]>::try_from(pk).ok()?, ctl_id))) {
                    Some((pk, ctl_id)) => Ok(MakerKey { pk, ctl_id }),
                    None => Err("revoked"),
                }
            }
        }
        None => Err("blocked"),
    };
    if standing.is_err() {
        state.support.end_keys(id, &[ckid], now());
    }
    Ok(standing)
}

/// `rev` of the waiting room's query, when it is there as a number.
fn asked_rev(query: &str) -> Option<u64> {
    form_urlencoded::parse(query.as_bytes())
        .find(|(name, _)| name == "rev")
        .and_then(|(_, value)| value.parse().ok())
}

fn waiting_reply(waiting: &Waiting) -> Reply {
    Reply::ok(&json!({ "rev": waiting.rev, "codes": waiting.codes }))
}

/// Handles `GET /api/hjv/support-code/wait?rev=<n>` (the caller checked the method): the codes
/// of the request's account key held in memory, at once when `rev` is not theirs now, else when
/// they change, at most [`WAIT_SECS`] and no later than the first of them ends. No record, no
/// nonce used (a GET), counted with the key's other calls.
pub async fn wait_handle(state: &Arc<AppState>, req: &ApiRequest) -> Reply {
    let s = match session(state, req).await {
        Ok(s) => s,
        Err(reply) => return reply,
    };
    let Some(own) = s.key else {
        return Reply::server_error();
    };
    if !may_make(&s.account.login_key) {
        return refusal(StatusCode::FORBIDDEN, TEXT_NO_CODE, "no_code");
    }
    if !state.key_calls.try_take(own.ckid) {
        if state.first_notice(&format!("keys-calls:{}", own.ckid), Duration::from_secs(60)) {
            log_warn!(
                "support code: more than {CALLS_PER_MINUTE} calls a minute from one key (acct={}, ckid={})",
                state.tags.account(&s.account.login_key),
                own.ckid
            );
        }
        return refusal(
            StatusCode::TOO_MANY_REQUESTS,
            TEXT_TOO_OFTEN,
            "rate_limited",
        );
    }
    let asked = asked_rev(&req.query);
    // armed before the codes are looked at, so that a change in between is not missed
    let notified = state.support.changed.notified();
    tokio::pin!(notified);
    notified.as_mut().enable();
    let at = now();
    let waiting = state.support.waiting(own.ckid, at);
    if asked != Some(waiting.rev) {
        return waiting_reply(&waiting);
    }
    let Some(_held) = state.support.hold(own.ckid) else {
        return waiting_reply(&waiting);
    };
    let secs = waiting.first_end.map_or(WAIT_SECS, |end| {
        u64::try_from(end - at).unwrap_or(0).min(WAIT_SECS)
    });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        tokio::select! {
            () = &mut notified => {}
            () = tokio::time::sleep_until(deadline) => break,
        }
        if state.support.stopping() {
            break;
        }
        notified.set(state.support.changed.notified());
        notified.as_mut().enable();
        let now_waiting = state.support.waiting(own.ckid, now());
        if now_waiting.rev != waiting.rev {
            return waiting_reply(&now_waiting);
        }
    }
    waiting_reply(&state.support.waiting(own.ckid, now()))
}

/// The lines `admin ticket status` adds: the codes open now and held, the waiting-room requests
/// held and the support links with downloads left, and the last day's `code` records by result;
/// no person, address, code or link.
pub async fn status(state: &Arc<AppState>) -> Result<String, String> {
    let at = now();
    let (open, held) = state.support.counts(at);
    let mut out = String::new();
    let _ = writeln!(
        out,
        "support codes: {open} open, {held} held in memory (at most {OPEN_PER_ACCOUNT} open per account and {MAX_HELD} held; {} min, one use; tickets {} h)",
        CODE_SECS / 60,
        TICKET_SECS / HOUR
    );
    let _ = writeln!(
        out,
        "support codes' waiting room: {} held (at most {HOLDS_PER_KEY} per key and {MAX_WAITS} in all, {WAIT_SECS} s); support links with downloads left: {}",
        state.support.held_waits(),
        state.support.links_left(at)
    );
    let since = at - DAY;
    let results = state
        .db
        .call(move |c| {
            let mut stmt = c.prepare(
                "SELECT result, COUNT(*) FROM events WHERE kind = 'code' AND at > ?1
                 GROUP BY result ORDER BY result",
            )?;
            let rows = stmt.query_map([since], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
        .await
        .map_err(|e| e.to_string())?;
    let tries: Vec<String> = results.iter().map(|(r, n)| format!("{r} {n}")).collect();
    let _ = writeln!(
        out,
        "support codes, the last day's records: {}",
        if tries.is_empty() {
            "none".to_owned()
        } else {
            tries.join(", ")
        }
    );
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: i64 = 1_760_000_000;
    const SYMBOLS: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

    fn maker(id: i64) -> Maker {
        Maker {
            account_id: id,
            account_tag: format!("{id:016x}"),
            ckid: 100 + id,
        }
    }

    fn found_state(codes: &SupportCodes, typed: &str, at: i64) -> Option<CodeState> {
        match codes.find(typed, Use::Pc, at) {
            Looked::Found { state, .. } => Some(state),
            Looked::Wrong => None,
        }
    }

    #[test]
    fn the_codes_form() {
        // eight symbols of Crockford's base32 from 40 bits, shown XXXX-XXXX
        assert_eq!(code_of([0; 5]), "00000000");
        assert_eq!(code_of([0xff; 5]), "ZZZZZZZZ");
        assert_eq!(code_of([0x08, 0x86, 0x42, 0x98, 0xe8]), "12345678");
        for _ in 0..200 {
            let code = new_code();
            assert_eq!(code.len(), 8);
            assert!(code.chars().all(|c| SYMBOLS.contains(c)), "{code}");
            let shown = shown(&code);
            assert_eq!((shown.len(), &shown[4..5]), (9, "-"));
            assert_eq!(read(&shown).as_deref(), Some(code.as_str()));
        }
        assert_ne!(new_code(), new_code());
    }

    #[test]
    fn reading_a_typed_code() {
        for (typed, read_as) in [
            ("K7M2-Q9XD", "K7M2Q9XD"),
            ("k7m2q9xd", "K7M2Q9XD"),
            ("K7M2 Q9XD", "K7M2Q9XD"),
            (" k7m2 - q9xd ", "K7M2Q9XD"),
            ("--K7M2Q9XD--", "K7M2Q9XD"),
            // O as 0, I and L as 1, in either case
            ("OoIi-LlAB", "001111AB"),
        ] {
            assert_eq!(read(typed).as_deref(), Some(read_as), "{typed:?}");
        }
        for typed in [
            "",
            "K7M2-Q9X",
            "K7M2-Q9XDE",
            "K7M2-Q9XU",
            "K7M2-Q9X!",
            "K7M2%2DQ9XD",
            "K7M2_Q9XD",
            "K7M2\tQ9XD",
            "K7M2\u{a0}Q9XD",
            "K7M2\u{3000}Q9XD",
            "K７M2Q9XD",
            "Ｋ7M2Q9XD",
            &"0".repeat(65),
            &format!("K7M2{}Q9XD", " ".repeat(57)),
        ] {
            assert_eq!(read(typed), None, "{typed:?}");
        }
    }

    #[test]
    fn a_code_works_once_within_its_ten_minutes() {
        let codes = SupportCodes::default();
        let code = codes.make(maker(1), Use::Pc, AT).unwrap().code;
        let Looked::Found {
            key,
            maker: who,
            state,
        } = codes.find(&shown(&code), Use::Pc, AT + 1)
        else {
            panic!("not found");
        };
        assert_eq!((who, state), (maker(1), CodeState::Open));
        // as typed in lower case, a moment before its end
        let lower = code.to_lowercase();
        assert_eq!(
            found_state(&codes, &lower, AT + CODE_SECS - 1),
            Some(CodeState::Open)
        );
        // over at ten minutes
        assert_eq!(
            found_state(&codes, &code, AT + CODE_SECS),
            Some(CodeState::Expired)
        );
        assert_eq!(codes.take(key, AT + CODE_SECS), Taken::Expired);
        // used once, then used
        assert_eq!(codes.take(key, AT + 2), Taken::Taken);
        assert_eq!(codes.take(key, AT + 3), Taken::Used);
        assert_eq!(found_state(&codes, &code, AT + 3), Some(CodeState::Used));
        // given back when no ticket was signed
        codes.give_back(key);
        assert_eq!(found_state(&codes, &code, AT + 3), Some(CodeState::Open));
        assert_eq!(codes.take(key, AT + 4), Taken::Taken);
        // another use's code is a wrong code; so is one never made
        assert_eq!(codes.find(&code, Use::Ctl, AT + 5), Looked::Wrong);
        let other = codes.make(maker(1), Use::Pc, AT).unwrap().code;
        assert_ne!(other, code);
        assert_eq!(codes.find("ZZZZ-ZZZZ", Use::Pc, AT), Looked::Wrong);
        // kept ten minutes after it ended, then gone: wrong
        assert_eq!(
            found_state(&codes, &code, AT + 4 + ENDED_KEPT_SECS - 1),
            Some(CodeState::Used)
        );
        assert_eq!(found_state(&codes, &code, AT + 4 + ENDED_KEPT_SECS), None);
        assert_eq!(codes.take(key, AT + 4 + ENDED_KEPT_SECS), Taken::Gone);
        assert_eq!(
            found_state(&codes, &other, AT + CODE_SECS + ENDED_KEPT_SECS - 1),
            Some(CodeState::Expired)
        );
        assert_eq!(
            found_state(&codes, &other, AT + CODE_SECS + ENDED_KEPT_SECS),
            None
        );
    }

    #[test]
    fn a_code_ends_with_its_makers_key() {
        let codes = SupportCodes::default();
        let one = codes.make(maker(1), Use::Pc, AT).unwrap().code;
        let mut other_key = maker(1);
        other_key.ckid = 7;
        let two = codes.make(other_key, Use::Pc, AT).unwrap().code;
        let used = codes.make(maker(1), Use::Pc, AT).unwrap().code;
        let Looked::Found { key, .. } = codes.find(&used, Use::Pc, AT) else {
            panic!("not found");
        };
        assert_eq!(codes.take(key, AT + 1), Taken::Taken);
        let theirs = codes.make(maker(2), Use::Pc, AT).unwrap().code;
        assert_eq!(codes.open_keys(1, AT + 2), vec![7, 101]);
        // the key 101 ended: its open code ends, the used one stays used, the others stay open
        assert_eq!(codes.end_keys(1, &[101], AT + 2), 1);
        assert_eq!(codes.end_keys(1, &[101], AT + 3), 0);
        assert_eq!(found_state(&codes, &one, AT + 3), Some(CodeState::Revoked));
        assert_eq!(found_state(&codes, &used, AT + 3), Some(CodeState::Used));
        assert_eq!(found_state(&codes, &two, AT + 3), Some(CodeState::Open));
        assert_eq!(found_state(&codes, &theirs, AT + 3), Some(CodeState::Open));
        assert_eq!(codes.open_keys(1, AT + 3), vec![7]);
        let Looked::Found { key, .. } = codes.find(&one, Use::Pc, AT + 3) else {
            panic!("not found");
        };
        assert_eq!(codes.take(key, AT + 3), Taken::Revoked);
        // kept ten minutes after it ended, then gone
        assert_eq!(
            found_state(&codes, &one, AT + 2 + ENDED_KEPT_SECS - 1),
            Some(CodeState::Revoked)
        );
        assert_eq!(found_state(&codes, &one, AT + 2 + ENDED_KEPT_SECS), None);
    }

    #[test]
    fn one_of_many_at_once_takes_a_code() {
        let codes = Arc::new(SupportCodes::default());
        let code = codes.make(maker(1), Use::Pc, AT).unwrap().code;
        let Looked::Found { key, .. } = codes.find(&code, Use::Pc, AT) else {
            panic!("not found");
        };
        let barrier = Arc::new(std::sync::Barrier::new(16));
        let takers: Vec<_> = (0..16)
            .map(|_| {
                let (codes, barrier) = (codes.clone(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    codes.take(key, AT + 1)
                })
            })
            .collect();
        let taken: Vec<Taken> = takers.into_iter().map(|t| t.join().unwrap()).collect();
        assert_eq!(taken.iter().filter(|t| **t == Taken::Taken).count(), 1);
        assert_eq!(taken.iter().filter(|t| **t == Taken::Used).count(), 15);
    }

    #[test]
    fn the_caps() {
        let codes = SupportCodes::default();
        // five open codes per account; the sixth waits for the first to end
        for i in 0..OPEN_PER_ACCOUNT as i64 {
            codes.make(maker(1), Use::Pc, AT + 10 * i).unwrap();
        }
        assert_eq!(
            codes.make(maker(1), Use::Pc, AT + 100),
            Err(Refused::TooMany {
                retry_after: (CODE_SECS - 100) as u64
            })
        );
        assert!(codes.make(maker(2), Use::Pc, AT + 100).is_ok());
        // one used: room again; at the last second, at least 1
        let first = codes.make(maker(3), Use::Pc, AT).unwrap().code;
        for _ in 1..OPEN_PER_ACCOUNT {
            codes.make(maker(3), Use::Pc, AT + 30).unwrap();
        }
        assert_eq!(
            codes.make(maker(3), Use::Pc, AT + CODE_SECS - 1),
            Err(Refused::TooMany { retry_after: 1 })
        );
        let Looked::Found { key, .. } = codes.find(&first, Use::Pc, AT + 1) else {
            panic!("not found");
        };
        assert_eq!(codes.take(key, AT + 1), Taken::Taken);
        assert!(codes.make(maker(3), Use::Pc, AT + 2).is_ok());
        // ten minutes on, the first account's first is over
        assert!(codes.make(maker(1), Use::Pc, AT + CODE_SECS).is_ok());

        // 1000 held in all: the ended ones go first, oldest first; 1000 open ones, no more
        let codes = SupportCodes::default();
        let old = codes.make(maker(0), Use::Pc, AT).unwrap().code;
        let newer = codes.make(maker(0), Use::Pc, AT + 60).unwrap().code;
        // both over, both held
        let t = AT + CODE_SECS + 60;
        let mut id = 1;
        while codes.counts(t).1 < MAX_HELD {
            for _ in 0..OPEN_PER_ACCOUNT {
                if codes.counts(t).1 < MAX_HELD {
                    codes.make(maker(id), Use::Pc, t).unwrap();
                }
            }
            id += 1;
        }
        assert_eq!(codes.counts(t), (MAX_HELD - 2, MAX_HELD));
        codes.make(maker(id), Use::Pc, t).unwrap();
        assert_eq!(found_state(&codes, &old, t), None);
        assert_eq!(found_state(&codes, &newer, t), Some(CodeState::Expired));
        codes.make(maker(id), Use::Pc, t).unwrap();
        assert_eq!(found_state(&codes, &newer, t), None);
        assert_eq!(codes.counts(t), (MAX_HELD, MAX_HELD));
        assert_eq!(codes.make(maker(id + 1), Use::Pc, t), Err(Refused::Busy));
    }

    #[test]
    fn the_key_is_this_starts_own() {
        // the same code under two starts' keys: two keys, neither the code
        let (a, b) = (SupportCodes::default(), SupportCodes::default());
        assert_ne!(a.key_of("K7M2Q9XD"), b.key_of("K7M2Q9XD"));
        assert_eq!(a.key_of("K7M2Q9XD"), a.key_of("K7M2Q9XD"));
        let code = a.make(maker(1), Use::Pc, AT).unwrap().code;
        assert_eq!(b.find(&code, Use::Pc, AT), Looked::Wrong);
        assert!(!format!("{:?}", a.held().codes).contains(&code));
    }

    #[test]
    fn the_waiting_room_hears_of_every_change() {
        let codes = SupportCodes::default();
        let me = maker(1);
        assert_eq!(
            codes.waiting(me.ckid, AT),
            Waiting {
                rev: 0,
                codes: vec![],
                first_end: None
            }
        );
        let made = codes.make(me.clone(), Use::Pc, AT).unwrap();
        let w = codes.waiting(me.ckid, AT + 1);
        assert_eq!((w.rev, w.first_end), (1, Some(AT + CODE_SECS)));
        assert_eq!(
            w.codes,
            vec![json!({"wait": made.wait, "state": "open", "expires_in": CODE_SECS - 1})]
        );
        // another key, of the same account or not, sees nothing of it
        let mut other = maker(1);
        other.ckid = 7;
        assert_eq!(codes.waiting(other.ckid, AT).codes, Vec::<Value>::new());
        assert_eq!(codes.waiting(maker(2).ckid, AT).rev, 0);
        // taken: still open until its ticket is signed, given back unseen
        let Looked::Found { key, .. } = codes.find(&made.code, Use::Pc, AT + 2) else {
            panic!("not found");
        };
        assert_eq!(codes.take(key, AT + 2), Taken::Taken);
        let w = codes.waiting(me.ckid, AT + 2);
        assert_eq!((w.rev, &w.codes[0]["state"]), (1, &json!("open")));
        codes.give_back(key);
        assert_eq!(codes.take(key, AT + 3), Taken::Taken);
        let by = UsedBy {
            pc: "123456789".into(),
            pk: [2; 32],
            exp: AT + 3 + TICKET_SECS,
        };
        codes.mark_used(key, by);
        let w = codes.waiting(me.ckid, AT + 10);
        assert_eq!(w.rev, 2);
        assert_eq!(
            w.codes,
            vec![
                json!({"wait": made.wait, "state": "used", "pc": "123456789",
                        "pk": b64(&[2; 32]), "until_in": TICKET_SECS - 7})
            ]
        );
        assert_eq!(w.first_end, None);
        // ended with its key: over
        let ended = codes.make(me.clone(), Use::Pc, AT + 20).unwrap();
        assert_eq!(codes.end_keys(1, &[me.ckid], AT + 21), 1);
        let w = codes.waiting(me.ckid, AT + 21);
        assert_eq!(w.rev, 4);
        assert_eq!(w.codes[1], json!({"wait": ended.wait, "state": "over"}));
        // its ten minutes over: noticed once
        let late = codes.make(me.clone(), Use::Pc, AT + 30).unwrap();
        assert_eq!(
            codes.waiting(me.ckid, AT + 31).first_end,
            Some(AT + 30 + CODE_SECS)
        );
        // gone from memory ten minutes after its end, out of the list: a change too
        let w = codes.waiting(me.ckid, AT + 3 + ENDED_KEPT_SECS);
        assert_eq!((w.rev, w.codes.len()), (6, 2));
        // the ended one gone, and the late one over: two more
        let w = codes.waiting(me.ckid, AT + 30 + CODE_SECS);
        assert_eq!(w.rev, 8);
        assert_eq!(w.codes, vec![json!({"wait": late.wait, "state": "over"})]);
        assert_eq!(codes.waiting(me.ckid, AT + 31 + CODE_SECS).rev, 8);
        let w = codes.waiting(me.ckid, AT + 30 + CODE_SECS + ENDED_KEPT_SECS);
        assert_eq!((w.rev, w.codes.len()), (9, 0));
        assert_eq!(codes.waiting(other.ckid, AT + 3 + ENDED_KEPT_SECS).rev, 0);
    }

    #[test]
    fn the_holds() {
        let codes = SupportCodes::default();
        let one = codes.hold(1).unwrap();
        let two = codes.hold(1).unwrap();
        assert!(codes.hold(1).is_none());
        drop(one);
        let three = codes.hold(1).unwrap();
        assert_eq!(codes.held_waits(), HOLDS_PER_KEY);
        let mut others: Vec<WaitHold<'_>> = (0..MAX_WAITS as i64 - 2)
            .map(|k| codes.hold(100 + k).unwrap())
            .collect();
        assert_eq!(codes.held_waits(), MAX_WAITS);
        assert!(codes.hold(5000).is_none());
        others.pop();
        assert!(codes.hold(5000).is_some());
        drop((two, three, others));
        assert_eq!(codes.held_waits(), 0);
        codes.stop();
        assert!(codes.hold(1).is_none());
    }

    #[test]
    fn a_link_is_its_codes_while_the_code_is_open() {
        let codes = SupportCodes::default();
        let made = codes.make(maker(1), Use::Pc, AT).unwrap();
        let other = codes.make(maker(1), Use::Pc, AT).unwrap();
        for value in [&made.link, &made.wait] {
            assert_eq!(value.len(), 22);
            assert_eq!(crate::util::b64url_decode(value).map(|b| b.len()), Some(16));
        }
        assert!(made.link != made.wait && made.link != other.link && made.wait != other.wait);
        let found = codes.find_link(&made.link, AT + 1).unwrap();
        assert_eq!(
            (&found.code, &found.maker, found.downloads_left),
            (&shown(&made.code), &maker(1), LINK_DOWNLOADS)
        );
        assert!(!format!("{found:?} {made:?}").contains(&made.code));
        // nor its waiting number, another value or the code
        for wrong in [
            made.wait.as_str(),
            "AAAAAAAAAAAAAAAAAAAAAA",
            &made.code,
            "",
            "%",
        ] {
            assert!(codes.find_link(wrong, AT + 1).is_none(), "{wrong}");
        }
        // three downloads
        for left in (0..LINK_DOWNLOADS).rev() {
            assert_eq!(codes.take_download(found.key, AT + 2), Some(left));
        }
        assert_eq!(codes.take_download(found.key, AT + 2), None);
        assert_eq!(codes.links_left(AT + 2), 1);
        assert_eq!(
            codes.find_link(&made.link, AT + 2).unwrap().downloads_left,
            0
        );
        // taken, the link ends; given back, it is there again; over at ten minutes
        let Looked::Found { key, .. } = codes.find(&other.code, Use::Pc, AT + 3) else {
            panic!("not found");
        };
        assert_eq!(codes.take(key, AT + 3), Taken::Taken);
        assert!(codes.find_link(&other.link, AT + 3).is_none());
        assert_eq!(codes.take_download(key, AT + 3), None);
        codes.give_back(key);
        assert!(codes.find_link(&other.link, AT + 3).is_some());
        assert!(codes.find_link(&other.link, AT + CODE_SECS).is_none());
    }

    /// The code itself is kept only while it is open: used (its ticket signed), ended or over,
    /// its symbols are dropped; taken a moment ago and given back, they stay.
    #[test]
    fn the_code_itself_goes_once_it_is_not_open() {
        let codes = SupportCodes::default();
        let shown_of = |codes: &SupportCodes| -> usize {
            codes
                .held()
                .codes
                .values()
                .filter(|e| e.symbols.is_some())
                .count()
        };
        let used = codes.make(maker(1), Use::Pc, AT).unwrap();
        let given_back = codes.make(maker(1), Use::Pc, AT).unwrap();
        codes.make(maker(2), Use::Pc, AT).unwrap();
        let later = codes.make(maker(3), Use::Pc, AT + 300).unwrap();
        assert_eq!(shown_of(&codes), 4);
        let key_of = |code: &str| match codes.find(code, Use::Pc, AT + 1) {
            Looked::Found { key, .. } => key,
            Looked::Wrong => panic!("not found"),
        };
        // taken and given back: still shown
        let key = key_of(&given_back.code);
        assert_eq!(codes.take(key, AT + 1), Taken::Taken);
        codes.sweep(AT + 1);
        codes.give_back(key);
        assert_eq!(shown_of(&codes), 4);
        assert!(codes.find_link(&given_back.link, AT + 1).is_some());
        // used: gone with the ticket
        let key = key_of(&used.code);
        assert_eq!(codes.take(key, AT + 2), Taken::Taken);
        codes.mark_used(
            key,
            UsedBy {
                pc: "123456789".into(),
                pk: [7; 32],
                exp: AT + 2 + TICKET_SECS,
            },
        );
        assert_eq!(shown_of(&codes), 3);
        // its maker's key ended: gone
        assert_eq!(codes.end_keys(2, &[maker(2).ckid], AT + 3), 1);
        assert_eq!(shown_of(&codes), 2);
        // over at ten minutes: gone at the next sweep, the later code still shown
        codes.sweep(AT + CODE_SECS);
        assert_eq!(shown_of(&codes), 1);
        assert!(codes.find_link(&later.link, AT + CODE_SECS).is_some());
        codes.sweep(AT + 300 + CODE_SECS);
        assert_eq!(shown_of(&codes), 0);
        // the ended entries stay their ten minutes for the records, without the code
        assert_eq!(codes.counts(AT + 300 + CODE_SECS), (0, 2));
    }

    #[test]
    fn the_body_of_a_request_to_make_one() {
        assert_eq!(make_body(br#"{"use":"pc"}"#), Ok(Use::Pc));
        for bad in [
            &b""[..],
            b"{}",
            b"[]",
            b"null",
            br#"{"use":"ctl"}"#,
            br#"{"use":"both"}"#,
            br#"{"use":null}"#,
            br#"{"use":"pc","to":"123456789"}"#,
            br#"{"use":"pc","use":"pc"}"#,
            br#"["pc"]"#,
            br#"{"use":["pc"]}"#,
        ] {
            assert!(make_body(bad).is_err(), "{}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn texts_never_say_disabled() {
        for text in [
            TEXT_DENIED,
            TEXT_NO_CODE,
            TEXT_TOO_MANY,
            TEXT_CANNOT_MAKE,
            TEXT_UNAVAILABLE,
            TEXT_WRONG_PC,
            TEXT_BAD_REQUEST,
        ] {
            assert!(!text.to_lowercase().contains("disabled"));
        }
    }
}

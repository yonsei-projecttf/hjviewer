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
//!   times; never in the database, a file, the journal or a record. A code that was used, ended
//!   or whose ten minutes are over stays [`ENDED_KEPT_SECS`] more, so that the records tell
//!   `used`, `revoked` and `expired` from `wrong`. A restart ends them all. The answer that
//!   carries a code is kept, as every signed request's that changes something, for its retry
//!   within HJV_API_RETRY_WINDOW (api.rs `authenticate`); a retry gets it as it was, its
//!   `expires_in` too.
//! - Records `code` (kept HJV_API_EVENT_DAYS): `made` with the maker's tag; each try to trade one
//!   with its result, the address's tag and, once the code was found, the maker's tag (guest.rs).
//!   Never the code or a tag of it.
//!
//! The numbers are constants, not settings; the user may change [`TICKET_SECS`], which two texts
//! of the app say (docs/access-tickets.md 5-3, S2 and P3). The `use` of both requests is where a
//! controller's code would go; 1.8.2 takes `pc` only.

use crate::{
    api::{session, ApiRequest, Reply},
    keys::{CALLS_PER_MINUTE, TEXT_TOO_OFTEN},
    review,
    state::AppState,
    ticket::Use,
    update::{code_of, shown, typed_code},
    util::{now, random_bytes, DAY, HOUR},
};
use hmac::{Hmac, Mac};
use hyper::StatusCode;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Sha256;
use std::{
    collections::HashMap,
    fmt::Write as _,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};
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

/// The account, and its account key, that made a code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Maker {
    pub account_id: i64,
    /// its tag (log.rs `Tagger::account`)
    pub account_tag: String,
    /// the account key the request to make it was signed with: the code ends with it
    pub ckid: i64,
}

#[derive(Debug, Clone)]
struct Entry {
    maker: Maker,
    use_: Use,
    expires: i64,
    used: Option<i64>,
    /// when it was found that its maker's key had ended ([`SupportCodes::end_keys`])
    revoked: Option<i64>,
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

/// The support codes, in memory only, under an HMAC with a key made at the start.
pub struct SupportCodes {
    key: Zeroizing<[u8; 32]>,
    codes: Mutex<HashMap<CodeKey, Entry>>,
}

impl Default for SupportCodes {
    fn default() -> Self {
        SupportCodes {
            key: Zeroizing::new(random_bytes::<32>()),
            codes: Mutex::default(),
        }
    }
}

impl SupportCodes {
    fn codes(&self) -> MutexGuard<'_, HashMap<CodeKey, Entry>> {
        self.codes.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn key_of(&self, code: &str) -> CodeKey {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&*self.key).expect("HMAC takes keys of any length");
        mac.update(code.as_bytes());
        CodeKey(mac.finalize().into_bytes().into())
    }

    /// Makes a code for the account `maker` and `use_` at `at`: its eight symbols.
    pub fn make(&self, maker: Maker, use_: Use, at: i64) -> Result<String, Refused> {
        let mut codes = self.codes();
        codes.retain(|_, e| e.kept(at));
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
            for (_, k) in ended.into_iter().take(excess) {
                codes.remove(&k);
            }
            if codes.len() >= MAX_HELD {
                return Err(Refused::Busy);
            }
        }
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
                });
                return Ok(code);
            }
        }
    }

    /// What `typed` is at `at` for a ticket of `use_`.
    pub fn find(&self, typed: &str, use_: Use, at: i64) -> Looked {
        let Some(code) = read(typed) else {
            return Looked::Wrong;
        };
        let key = self.key_of(&code);
        let mut codes = self.codes();
        codes.retain(|_, e| e.kept(at));
        match codes.get(&key) {
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
    /// gets [`Taken::Taken`].
    pub fn take(&self, key: CodeKey, at: i64) -> Taken {
        let mut codes = self.codes();
        match codes.get_mut(&key) {
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
        if let Some(e) = self.codes().get_mut(&key) {
            e.used = None;
        }
    }

    /// The account keys that made the account `account_id`'s codes open at `at`, each once.
    pub fn open_keys(&self, account_id: i64, at: i64) -> Vec<i64> {
        let mut ckids: Vec<i64> = self
            .codes()
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
        let mut ended = 0;
        for e in self.codes().values_mut() {
            if e.maker.account_id == account_id && ckids.contains(&e.maker.ckid) && e.open(at) {
                e.revoked = Some(at);
                ended += 1;
            }
        }
        ended
    }

    /// The codes open at `at`, and those held (open or ended).
    pub fn counts(&self, at: i64) -> (usize, usize) {
        let codes = self.codes();
        let held = codes.values().filter(|e| e.kept(at)).count();
        let open = codes.values().filter(|e| e.open(at)).count();
        (open, held)
    }

    /// Moves every code's times `secs` into the past (tests: a code's ten minutes without the
    /// wait).
    pub fn age_by(&self, secs: i64) {
        for e in self.codes().values_mut() {
            e.expires -= secs;
            for when in [&mut e.used, &mut e.revoked].into_iter().flatten() {
                *when -= secs;
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
        Ok(code) => {
            state.record("code", Some(tag), None, None, "made").await;
            Reply::ok(&json!({
                "code": shown(&code),
                "use": use_.as_str(),
                "expires_in": CODE_SECS,
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

/// The lines `admin ticket status` adds: the codes open now and held, and the last day's `code`
/// records by result; no person, address or code.
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
        let code = codes.make(maker(1), Use::Pc, AT).unwrap();
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
        let other = codes.make(maker(1), Use::Pc, AT).unwrap();
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
        let one = codes.make(maker(1), Use::Pc, AT).unwrap();
        let mut other_key = maker(1);
        other_key.ckid = 7;
        let two = codes.make(other_key, Use::Pc, AT).unwrap();
        let used = codes.make(maker(1), Use::Pc, AT).unwrap();
        let Looked::Found { key, .. } = codes.find(&used, Use::Pc, AT) else {
            panic!("not found");
        };
        assert_eq!(codes.take(key, AT + 1), Taken::Taken);
        let theirs = codes.make(maker(2), Use::Pc, AT).unwrap();
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
        let code = codes.make(maker(1), Use::Pc, AT).unwrap();
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
        let first = codes.make(maker(3), Use::Pc, AT).unwrap();
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
        let old = codes.make(maker(0), Use::Pc, AT).unwrap();
        let newer = codes.make(maker(0), Use::Pc, AT + 60).unwrap();
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
        let code = a.make(maker(1), Use::Pc, AT).unwrap();
        assert_eq!(b.find(&code, Use::Pc, AT), Looked::Wrong);
        assert!(!format!("{:?}", a.codes().keys().collect::<Vec<_>>()).contains(&code));
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

// SPDX-License-Identifier: AGPL-3.0-only

//! The access password (이용 비밀번호, docs/access-tickets.md 3-5): someone who is not logged in
//! types the one password the operator set, and gets one access ticket (ticket.rs) for one PC: a
//! controller's for the PC it connects to (`guest`/`ctl`, its `to` that one ID), or a PC's for
//! itself (`guest`/`pc`, its ID and device key), each for HJV_API_GUEST_TICKET_HOURS (12 h). The
//! app keeps neither the password nor the ticket beyond its process (the user's decisions of
//! 2026-10-16).
//!
//! `POST /api/hjv/guest`, without a token, as a login: JSON, at most 64 KiB, no `Origin`:
//!
//! ```text
//! {"use":"ctl","to":"<the PC's ID>","password":"hjv-sealed-v1.<kid>.<sealed>"}
//! {"use":"pc","id":"<its ID>","pk":"<its device key>","sig":"<signature>","password":"hjv-sealed-v1.…"}
//! sig: the device key's, over "hjv-guest-pc-v1" NUL id NUL pk NUL base64(SHA-256(sealed bytes))
//! ```
//!
//! The password is sealed as a login's (seal.rs), with the purpose `"p":"guest"`, the request's
//! `use`, and in `id` the PC the ticket reaches. The checks, in this order:
//!
//! 1. the body (400 `bad_request`, not counted) and the ticket key (503 `unavailable` without);
//!    the very same body again within HJV_API_RETRY_WINDOW gets the first answer here, before
//!    any limit, so that the app's resend of a try that issued a ticket (and used a code) gets
//!    that ticket even when the address's failures fill up meanwhile; a first answer of step 2's
//!    429 is not kept for it;
//! 2. the address's failures (HJV_API_LIMIT_GUEST_IP; an IPv6 address by its /64): 429;
//! 3. the seal and a PC's signature: 400 for a seal that is not the access password's for this
//!    request, a used nonce or a signature that does not hold, 409 `clock` for a seal's time,
//!    each counted as the address's failure;
//! 4. the address's tickets (HJV_API_LIMIT_GUEST_TICKETS) and the tries of all addresses
//!    together (HJV_API_LIMIT_GUEST_ALL, in memory; one address holds a quarter of it): 429;
//! 5. at most [`HASHES`] PBKDF2 at once (503 `busy`), on a blocking thread: hjv-api has one CPU
//!    and shares its blocking threads with every database call, so the limits come first;
//! 6. the file (HJV_API_GUEST_FILE): a wrong password and a closed file (none, refused, past its
//!    UNTIL) get the same 401 `denied`, counted as the address's failure;
//! 7. for a PC, the key hbbs holds for its ID (hbbs.rs `hjv-pk`): another key gets 403
//!    `wrong_pc`, a console that does not answer 503 `unavailable`; hbbs is asked about a PC
//!    once a minute at most, its answer serving the minute;
//! 8. the ticket, `{"ticket","use","exp","server_time"}` as every ticket (ticket.rs).
//!
//! Every answer but a ticket leaves no sooner than HJV_API_FAILURE_DELAY_MS ±
//! HJV_API_FAILURE_JITTER_MS after the request came, as a failed login does: a wrong password and
//! a closed file take as long. Each try leaves a record `guest` (its result, the time and the
//! address's tag; never the address, an ID, the password or the ticket), kept for
//! HJV_API_EVENT_DAYS; failures at most FAILURE_RECORDS_PER_MINUTE a minute.
//!
//! The file is root:hjviewer-api 0640, as the App Review account's (deploy/guest.conf.example;
//! deploy/guest-conf.py writes it in the operator's own terminal), with each line once and `#`
//! comments:
//!
//! ```text
//! HASH=pbkdf2-sha256$<iterations>$<salt, standard base64>$<key, standard base64>
//! UNTIL=<UTC time, 2026-11-29T15:00:00Z>
//! ```
//!
//! It is read again when it changes (update.rs `watched`); the journal and `admin ticket status`
//! tell files apart by a tag of the HASH line. A new password ends no ticket handed out before:
//! NOT_BEFORE in hbbs's access.conf does (docs/access-tickets.md 6-4).
//!
//! Since 1.8.2 a PC may bring a support code (support.rs, docs/access-tickets.md 3-7) in the place
//! of the password: `"code"` for `"password"` in a PC's request (with `use` `ctl`, or with both,
//! 400 before anything counts), sealed with the purpose `"p":"code"`. The checks are the same, in
//! the same order, with the code looked up in the place of steps 5 and 6 (no file, no PBKDF2):
//! a code that is not there, over, used, whose maker's account is blocked, deleted or off the
//! allowlist, or whose maker's account key has ended (a logout, a revoke) gets the one 401
//! `denied` of support.rs, counted as the address's failure. The code
//! is used only with the ticket (step 8), so that of two requests with it only one gets one; the
//! ticket lives support.rs `TICKET_SECS` (2 h). Its tries leave records `code`, with the maker's
//! tag once the code was found, in the same counters and within the same budget of failure
//! records.

use crate::{
    api::{is_json, ApiRequest, BodyError, Reply},
    config::Config,
    db::{self, Record},
    devsig,
    hbbs::{self, Lookup},
    limits::{self, Admit, Take, SCOPE_GUEST_IP, SCOPE_GUEST_TICKETS},
    login::{fresh, wait_for, Retry, TEXT_SEAL_CLOCK},
    review::hash_parts,
    seal::{Opened, SealError},
    state::AppState,
    support::{self, CodeState, Looked, Taken},
    ticket::{self, Claims, NoTicket, Use},
    update::{utc_time, watched, Watched},
    util::{b64, b64_decode, datetime, hex, now, random_below, sha256, valid_device_id, DAY},
};
use hyper::{header::ORIGIN, Method, StatusCode};
use ring::pbkdf2;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fmt::Write as _,
    num::NonZeroU32,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

pub const PATH: &str = "/api/hjv/guest";
/// What a PC's device key signs with its request ([`pc_message`]).
pub const PC_CONTEXT: &[u8] = b"hjv-guest-pc-v1";
/// The purpose of the access password's seal (seal.rs).
pub const PURPOSE: &str = "guest";
/// PBKDF2 of the access password at once; one more gets 503 `busy`.
pub const HASHES: usize = 2;
/// Requests in hand at once, each waiting out the failure delay at most; one more gets 503
/// `busy` at once.
pub const IN_FLIGHT: usize = 32;
/// How long hbbs's answer about a PC's key serves (docs/access-tickets.md 3-5).
const LOOKUP_SERVES: Duration = Duration::from_secs(60);
/// The PCs whose last answer is kept, at most.
const MAX_LOOKUPS: usize = 1024;
const MAX_FILE: u64 = 4096;

/// The one answer to a wrong password and to a closed file (docs/access-tickets.md 5-3, E1).
pub const TEXT_DENIED: &str =
    "이용 비밀번호가 맞지 않거나, 지금은 서버가 이용 비밀번호를 받지 않습니다.";
pub const TEXT_TOO_OFTEN: &str = "시도가 너무 많습니다. 잠시 뒤 다시 해 주세요.";
pub const TEXT_UNAVAILABLE: &str =
    "지금은 이용 비밀번호를 확인할 수 없습니다. 잠시 뒤 다시 해 주세요.";
pub const TEXT_WRONG_PC: &str =
    "이 PC의 키가 서버에 있는 것과 달라 이용 비밀번호로 연결받을 수 없습니다. 관리자에게 알려 주세요.";
const TEXT_BAD_REQUEST: &str = "요청을 이해하지 못했습니다.";
const TEXT_FORBIDDEN: &str = "접근이 차단되었습니다";
const TEXT_MEDIA_TYPE: &str = "JSON 요청만 받습니다";
const TEXT_TOO_LARGE: &str = "요청이 너무 큽니다";
const TEXT_SERVER: &str = "서버 오류입니다. 잠시 뒤 다시 시도해 주세요";
/// The in-flight gate's 503, before the body says whether a password or a code comes (the app
/// shows its own E3 or E7).
const TEXT_BUSY: &str = "지금은 확인할 수 없습니다. 잠시 뒤 다시 시도해 주세요.";

/// The file, as read.
pub struct Guest {
    iterations: NonZeroU32,
    salt: Vec<u8>,
    key: [u8; 32],
    until: i64,
    /// the first 4 bytes of the SHA-256 of its HASH, in hex: which file it is, not the password
    tag: String,
}

impl Guest {
    /// Reads the file: `HASH=` and `UNTIL=` once each, and `#` comments. The errors never show
    /// what the file holds.
    pub fn parse(file: &[u8]) -> Result<Guest, &'static str> {
        let text = std::str::from_utf8(file).map_err(|_| "the file is not text")?;
        let (mut hash, mut until) = (None, None);
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (field, value) = match line.split_once('=') {
                Some(("HASH", value)) => (&mut hash, value),
                Some(("UNTIL", value)) => (&mut until, value),
                _ => return Err("a line that is neither HASH=, UNTIL= nor a # comment"),
            };
            if field.replace(value.trim()).is_some() {
                return Err("HASH or UNTIL is there more than once");
            }
        }
        let (Some(hash), Some(until)) = (hash, until) else {
            return Err("HASH or UNTIL is missing");
        };
        let (iterations, salt, key) = hash_parts(hash)?;
        Ok(Guest {
            iterations,
            salt,
            key,
            until: utc_time(until).ok_or("UNTIL is not a time")?,
            tag: hex(&sha256(hash.as_bytes())[..4]),
        })
    }

    /// When the access password ends (UTC).
    pub fn until(&self) -> i64 {
        self.until
    }

    /// Which file it is.
    pub fn tag(&self) -> &str {
        &self.tag
    }

    /// Whether `password` is the file's: PBKDF2-HMAC-SHA256 with its salt and iterations,
    /// compared in constant time (ring). It takes as long as the iterations: not for an async
    /// thread.
    pub fn verify(&self, password: &str) -> bool {
        pbkdf2::verify(
            pbkdf2::PBKDF2_HMAC_SHA256,
            self.iterations,
            &self.salt,
            password.as_bytes(),
            &self.key,
        )
        .is_ok()
    }

    fn summary(&self) -> String {
        format!("until {} (hash {})", datetime(self.until), self.tag)
    }
}

/// The file as hjv-api last read it.
#[derive(Default)]
pub struct GuestFile(Mutex<Watched<Guest>>);

impl GuestFile {
    /// The file as it is now, whatever its UNTIL; `None` without HJV_API_GUEST_FILE, the file or
    /// one that is taken. It reads a file: not for an async thread.
    pub fn read(&self, config: &Config) -> Option<Arc<Guest>> {
        let path = config.guest_file.as_deref()?;
        watched(
            &self.0,
            path,
            MAX_FILE,
            "guest: the access password's file (HJV_API_GUEST_FILE)",
            Guest::parse,
            Guest::summary,
        )
    }

    /// The file while it takes the access password at `at`: there, taken, and before its UNTIL.
    pub fn active(&self, config: &Config, at: i64) -> Option<Arc<Guest>> {
        self.read(config).filter(|guest| at < guest.until)
    }
}

/// hbbs's last answer about the key of each PC that asked, which serves a minute.
#[derive(Default)]
pub struct KeyLookups(Mutex<HashMap<String, (Instant, Lookup)>>);

impl KeyLookups {
    fn get(&self, id: &str) -> Option<Lookup> {
        let lookups = self.0.lock().unwrap_or_else(|e| e.into_inner());
        lookups
            .get(id)
            .filter(|(at, _)| at.elapsed() < LOOKUP_SERVES)
            .map(|(_, lookup)| lookup.clone())
    }

    fn put(&self, id: &str, lookup: Lookup) {
        let mut lookups = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if lookups.len() >= MAX_LOOKUPS {
            lookups.retain(|_, (at, _)| at.elapsed() < LOOKUP_SERVES);
        }
        if lookups.len() < MAX_LOOKUPS {
            lookups.insert(id.to_owned(), (Instant::now(), lookup));
        }
    }
}

/// The bytes a PC's device key signs with its request: the context, its ID, its key as the request
/// has it (standard base64) and the standard base64 of SHA-256 of the sealed bytes, joined by
/// single NUL bytes. The seal's hash ties the signature to that one request.
pub fn pc_message(id: &str, pk: &str, sealed_hash: &[u8]) -> Vec<u8> {
    let hash = b64(sealed_hash);
    [PC_CONTEXT, id.as_bytes(), pk.as_bytes(), hash.as_bytes()].join(&0u8)
}

/// What a request brings: the access password, or a support code (support.rs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Password,
    Code,
}

impl Kind {
    /// The purpose its seal must have.
    fn purpose(self) -> &'static str {
        match self {
            Kind::Password => PURPOSE,
            Kind::Code => support::PURPOSE,
        }
    }

    /// The kind of its records, and the word of its counts in `admin ticket status`.
    fn record(self) -> &'static str {
        match self {
            Kind::Password => "guest",
            Kind::Code => "code",
        }
    }

    /// What the journal calls it.
    fn named(self) -> &'static str {
        match self {
            Kind::Password => "the access password",
            Kind::Code => "a support code",
        }
    }

    fn denied(self) -> Reply {
        match self {
            Kind::Password => refusal(StatusCode::UNAUTHORIZED, TEXT_DENIED, "denied"),
            Kind::Code => refusal(StatusCode::UNAUTHORIZED, support::TEXT_DENIED, "denied"),
        }
    }

    fn unavailable(self) -> Reply {
        let text = match self {
            Kind::Password => TEXT_UNAVAILABLE,
            Kind::Code => support::TEXT_UNAVAILABLE,
        };
        refusal(StatusCode::SERVICE_UNAVAILABLE, text, "unavailable")
    }

    fn wrong_pc(self) -> Reply {
        let text = match self {
            Kind::Password => TEXT_WRONG_PC,
            Kind::Code => support::TEXT_WRONG_PC,
        };
        refusal(StatusCode::FORBIDDEN, text, "wrong_pc")
    }
}

/// What a request asks for.
enum Asked {
    /// a controller's ticket for the PC `to`
    Ctl { to: String },
    /// a PC's ticket for itself: its ID, its key (and as sent) and its signature
    Pc {
        id: String,
        pk: [u8; 32],
        pk_text: String,
        sig: String,
    },
}

impl Asked {
    fn use_(&self) -> &'static str {
        match self {
            Asked::Ctl { .. } => "ctl",
            Asked::Pc { .. } => "pc",
        }
    }

    /// The PC the ticket reaches, which the seal names too.
    fn pc(&self) -> &str {
        match self {
            Asked::Ctl { to } => to,
            Asked::Pc { id, .. } => id,
        }
    }
}

/// The request's members: all strings, each once, nothing else.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    #[serde(rename = "use")]
    use_: String,
    to: Option<String>,
    id: Option<String>,
    pk: Option<String>,
    sig: Option<String>,
    password: Option<String>,
    code: Option<String>,
}

/// What a request asks for, what it brings and its sealed field; `Err` with why for anything but
/// the members of a `ctl` or a `pc` request with the access password, or of a `pc` request with a
/// support code.
fn parse_body(body: &[u8]) -> Result<(Asked, Kind, String), &'static str> {
    // an object of strings first: serde's derive takes a null as no member, and an array as the
    // members in their order
    let strings = matches!(
        serde_json::from_slice::<Value>(body),
        Ok(Value::Object(members)) if members.values().all(Value::is_string)
    );
    let body: Body = strings
        .then(|| serde_json::from_slice(body).ok())
        .flatten()
        .ok_or("not an object of the members of a request")?;
    let asked = match (body.use_.as_str(), body.to, body.id, body.pk, body.sig) {
        ("ctl", Some(to), None, None, None) => {
            if !valid_device_id(&to) {
                return Err("to is not a PC ID");
            }
            Asked::Ctl { to }
        }
        ("pc", None, Some(id), Some(pk_text), Some(sig)) => {
            if !valid_device_id(&id) {
                return Err("id is not a PC ID");
            }
            let pk = b64_decode(&pk_text)
                .and_then(|k| <[u8; 32]>::try_from(k).ok())
                .filter(|k| b64(k) == pk_text)
                .ok_or("pk is not the standard base64 of 32 bytes")?;
            Asked::Pc {
                id,
                pk,
                pk_text,
                sig,
            }
        }
        _ => return Err("not the members of a ctl or pc request"),
    };
    match (body.password, body.code, &asked) {
        (Some(password), None, _) => Ok((asked, Kind::Password, password)),
        (None, Some(code), Asked::Pc { .. }) => Ok((asked, Kind::Code, code)),
        (None, Some(_), Asked::Ctl { .. }) => Err("a support code for a controller"),
        _ => Err("neither the password nor a code, or both"),
    }
}

/// How a try ended, for its limit counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// the client's failure (a wrong password, a closed file, a bad seal): the address's failure
    /// counts
    Failed,
    /// refused for another reason (a limit, the server busy or in trouble): nothing counts
    Released,
    /// a ticket, or a right password whose PC hbbs knows by another key: the address's failure
    /// is taken back; the ticket counts with the ticket
    Passed,
}

/// One try's reserved limit counters and its record.
struct Run<'a> {
    state: &'a Arc<AppState>,
    kind: Kind,
    /// the client's address's tag (an IPv6 address by its /64)
    ip_tag: String,
    /// a support code's maker's tag, once the code was found
    account_tag: Option<String>,
    failure: Option<i64>,
    ticket: Option<i64>,
    /// holds a call of HJV_API_LIMIT_GUEST_ALL
    call: bool,
}

impl<'a> Run<'a> {
    fn new(state: &'a Arc<AppState>, kind: Kind, ip: &str) -> Run<'a> {
        Run {
            state,
            kind,
            ip_tag: state.tags.ip(ip),
            account_tag: None,
            failure: None,
            ticket: None,
            call: false,
        }
    }

    /// Reserves an attempt; `Ok(None)` when the limit is full.
    async fn reserve(
        &self,
        scope: &'static str,
        windows: Vec<crate::config::Window>,
    ) -> Result<Option<i64>, ()> {
        let (at, key) = (now(), self.ip_tag.clone());
        self.state
            .db
            .call(move |c| limits::reserve(c, scope, &key, &windows, at))
            .await
            .map_err(|e| log_err!("guest: {e}"))
    }

    /// Settles the reserved attempts as `outcome` says and writes the record `result` (that of
    /// a failure only within the minute's budget; none without one), in one transaction.
    async fn end(&mut self, outcome: Outcome, result: Option<&'static str>, issued: bool) {
        let (failure, ticket) = (self.failure.take(), self.ticket.take());
        let (mut keep, mut back) = (Vec::new(), Vec::new());
        match outcome {
            Outcome::Failed => {
                keep.extend(failure);
                back.extend(ticket);
            }
            Outcome::Released => back.extend(failure.into_iter().chain(ticket)),
            Outcome::Passed => {
                back.extend(failure);
                if issued {
                    keep.extend(ticket);
                } else {
                    back.extend(ticket);
                }
            }
        }
        if self.call && outcome == Outcome::Released {
            self.state.guest_calls.give_back(&[self.ip_tag.as_str()]);
        }
        self.call = false;
        let record = result
            .filter(|_| issued || self.admit_record())
            .map(|result| Record {
                at: now(),
                kind: self.kind.record(),
                account_tag: self.account_tag.clone(),
                device_tag: None,
                ip: Some(self.ip_tag.clone()),
                result,
            });
        if keep.is_empty() && back.is_empty() && record.is_none() {
            return;
        }
        let written = self
            .state
            .db
            .call(move |c| {
                let tx = c.transaction()?;
                limits::settle(&tx, &keep, &back)?;
                if let Some(record) = &record {
                    db::insert_record(&tx, record)?;
                }
                tx.commit()
            })
            .await;
        if let Err(e) = written {
            log_err!("guest: {e}");
        }
    }

    fn admit_record(&self) -> bool {
        match self.state.guest_records.admit() {
            Admit::Write => true,
            Admit::Skip { first } => {
                if first {
                    log_warn!(
                        "guest: more than {} failed tries of the access password and support codes this minute; the rest are not recorded",
                        limits::FAILURE_RECORDS_PER_MINUTE
                    );
                }
                false
            }
        }
    }

    async fn fail(&mut self, outcome: Outcome, result: &'static str, reply: Reply) -> Reply {
        self.end(outcome, Some(result), false).await;
        reply
    }
}

fn refusal(status: StatusCode, text: &str, code: &str) -> Reply {
    Reply::json(status, &json!({ "error": text, "hjv": code }))
}

fn bad_request() -> Reply {
    refusal(StatusCode::BAD_REQUEST, TEXT_BAD_REQUEST, "bad_request")
}

/// 429 `rate_limited`, with the seconds until a try could come again (`retry_after`).
fn too_often(retry_after: u64) -> Reply {
    Reply::json(
        StatusCode::TOO_MANY_REQUESTS,
        &json!({ "error": TEXT_TOO_OFTEN, "hjv": "rate_limited", "retry_after": retry_after }),
    )
}

fn busy() -> Reply {
    refusal(StatusCode::SERVICE_UNAVAILABLE, TEXT_UNAVAILABLE, "busy")
}

fn clock() -> Reply {
    Reply::json(
        StatusCode::CONFLICT,
        &json!({ "error": TEXT_SEAL_CLOCK, "hjv": "clock", "server_time": now() }),
    )
}

/// What a retry gets when the try it repeats ended without an answer.
fn unanswered() -> Reply {
    Reply::error(StatusCode::INTERNAL_SERVER_ERROR, TEXT_SERVER)
}

/// Handles `POST /api/hjv/guest` (api.rs routes it before the account paths' rules: it takes no
/// token, and checks its form itself).
pub async fn handle(state: &Arc<AppState>, req: ApiRequest) -> Reply {
    let started = Instant::now();
    let Ok(permit) = state.guest_slots.clone().try_acquire_owned() else {
        if state.first_notice("guest-slots", Duration::from_secs(60)) {
            log_warn!(
                "guest: more than {IN_FLIGHT} tries of the access password and support codes at once, refused"
            );
        }
        let reply = refusal(StatusCode::SERVICE_UNAVAILABLE, TEXT_BUSY, "busy");
        tokio::time::sleep_until((started + failure_delay(state)).into()).await;
        return reply;
    };
    // The try runs to its end even when the client goes away, so that its answer is there for the
    // app's retry of the same body; it holds its slot until then.
    let task_state = state.clone();
    let task = tokio::spawn(async move {
        let reply = attempt(&task_state, req).await;
        drop(permit);
        reply
    });
    let reply = match task.await {
        Ok(reply) => reply,
        Err(_) => {
            log_err!("guest: the try's task failed");
            unanswered()
        }
    };
    if reply.status != StatusCode::OK {
        tokio::time::sleep_until((started + failure_delay(state)).into()).await;
    }
    reply
}

/// How long after the request a refusal may leave: the login's failure delay ± its jitter.
fn failure_delay(state: &AppState) -> Duration {
    let c = &state.config;
    let jitter = c.failure_jitter.as_millis() as u64;
    c.failure_delay - c.failure_jitter + Duration::from_millis(random_below(2 * jitter + 1))
}

async fn attempt(state: &Arc<AppState>, req: ApiRequest) -> Reply {
    // 1. the form of the request, and the ticket key
    if req.headers.contains_key(ORIGIN) {
        return Reply::error(StatusCode::FORBIDDEN, TEXT_FORBIDDEN);
    }
    if req.method != Method::POST {
        return Reply {
            allow: Some("POST"),
            ..Reply::error(StatusCode::METHOD_NOT_ALLOWED, crate::api::TEXT_METHOD)
        };
    }
    if !is_json(&req.headers) {
        return Reply::error(StatusCode::UNSUPPORTED_MEDIA_TYPE, TEXT_MEDIA_TYPE);
    }
    let body = match &req.body {
        Ok(body) => body.clone(),
        Err(BodyError::TooLarge) => {
            return Reply::error(StatusCode::PAYLOAD_TOO_LARGE, TEXT_TOO_LARGE)
        }
        Err(BodyError::Unreadable) => return bad_request(),
    };
    let (asked, kind, field) = match parse_body(&body) {
        Ok(parsed) => parsed,
        Err(why) => {
            let tag = state.tags.ip(&req.ip);
            if state.first_notice(&format!("guest-form:{tag}"), Duration::from_secs(600)) {
                log_notice!("guest: a request not of the form ({why}; ip={tag})");
            }
            return bad_request();
        }
    };
    if state.ticket_key.is_none() {
        state
            .tickets
            .note(&format!("{} refused off", kind.record()));
        return kind.unavailable();
    }
    // the very same body again: the first answer, before any limit (a resend whose first try
    // issued the ticket and used the code is not refused by failures that came since)
    let body_hash = sha256(&body);
    let body_key = format!("guest-body:{}", hex(&body_hash));
    let tx = match state.request_retries.begin(&body_key, body_hash) {
        Retry::Again(answer) => return wait_for(answer, unanswered).await,
        // the key is the body's hash, so another body under it is SHA-256's collision
        Retry::Reused => return bad_request(),
        Retry::First(tx) => tx,
    };
    let (reply, keep) = steps(state, &req.ip, body_hash, asked, kind, &field).await;
    if !keep {
        // a 429 of the address's failures: a resend is looked at afresh
        state.request_retries.forget(&body_key);
    }
    let _ = tx.send(Some(reply.clone()));
    reply
}

/// Steps 2 to 8; whether the answer is kept for a resend of the same body (all but step 2's).
async fn steps(
    state: &Arc<AppState>,
    ip: &str,
    body_hash: [u8; 32],
    asked: Asked,
    kind: Kind,
    field: &str,
) -> (Reply, bool) {
    // 2. the address's failures
    let mut run = Run::new(state, kind, ip);
    match run
        .reserve(SCOPE_GUEST_IP, state.config.limit_guest_ip.clone())
        .await
    {
        Ok(Some(id)) => run.failure = Some(id),
        Ok(None) => return (limited(&mut run, "limited-ip").await, false),
        Err(()) => return (unanswered(), false),
    }
    (
        seal_and_exchange(&mut run, body_hash, asked, field).await,
        true,
    )
}

/// Steps 3 to 8, once the address's failure is reserved.
async fn seal_and_exchange(
    run: &mut Run<'_>,
    body_hash: [u8; 32],
    asked: Asked,
    field: &str,
) -> Reply {
    let (state, kind) = (run.state, run.kind);

    // 3. the seal, and a PC's signature
    let opened = match open_seal(state, &asked, kind, field) {
        Ok(opened) => opened,
        Err(result @ "seal-time") => return run.fail(Outcome::Failed, result, clock()).await,
        Err(result) => return run.fail(Outcome::Failed, result, bad_request()).await,
    };
    match state
        .request_retries
        .begin(&format!("guest:{}", opened.nonce), body_hash)
    {
        Retry::Again(answer) => {
            // the first request's record is the try's
            run.end(Outcome::Released, None, false).await;
            wait_for(answer, unanswered).await
        }
        Retry::Reused => run.fail(Outcome::Failed, "seal-nonce", bad_request()).await,
        Retry::First(tx) => {
            let nonce_hash = sha256(opened.nonce.as_bytes());
            let at = now();
            let expires = opened
                .time
                .saturating_add(state.config.seal_max_skew_secs)
                .saturating_add(60);
            let fresh = state
                .db
                .call(move |c| db::use_nonce(c, &nonce_hash, at, expires))
                .await;
            let reply = match fresh {
                Ok(true) => exchange(run, asked, opened).await,
                Ok(false) => run.fail(Outcome::Failed, "seal-nonce", bad_request()).await,
                Err(e) => {
                    log_err!("guest: {e}");
                    run.fail(Outcome::Released, "error", unanswered()).await
                }
            };
            let _ = tx.send(Some(reply.clone()));
            reply
        }
    }
}

/// Opens the sealed password or code and checks it against the request, its kind and the clock,
/// and a PC's signature against the seal (step 3, but the nonce); on failure, the record's result.
fn open_seal(
    state: &AppState,
    asked: &Asked,
    kind: Kind,
    field: &str,
) -> Result<Opened, &'static str> {
    let opened = state.seal.open(field).map_err(|e| match e {
        SealError::NotSealed => "seal-missing",
        SealError::UnknownKey => "seal-unknown-key",
        SealError::Corrupt => "seal-corrupt",
    })?;
    if opened.purpose.as_deref() != Some(kind.purpose()) {
        return Err("seal-purpose");
    }
    if opened.use_.as_deref() != Some(asked.use_()) || opened.device_id != asked.pc() {
        return Err("seal-request");
    }
    if !fresh(now(), opened.time, state.config.seal_max_skew_secs) {
        return Err("seal-time");
    }
    if let Asked::Pc {
        id,
        pk,
        pk_text,
        sig,
    } = asked
    {
        if !devsig::verify(pk, &pc_message(id, pk_text, &opened.sealed_hash), sig) {
            return Err("pc-signature");
        }
    }
    Ok(opened)
}

/// A try refused by a full limit of the address or of everyone: it counts nowhere, and the
/// record and the journal hear of it once per window, so that hammering writes nothing.
async fn limited(run: &mut Run<'_>, result: &'static str) -> Reply {
    let state = run.state;
    let window = match result {
        "limited-ip" => window_of(&state.config.limit_guest_ip),
        "limited-tickets" => window_of(&state.config.limit_guest_tickets),
        _ => Duration::from_secs(state.config.limit_guest_all.secs as u64),
    };
    let kind = run.kind.record();
    state.tickets.note(&format!("{kind} refused {result}"));
    let first = state.first_notice(&format!("{kind}:{result}:{}", run.ip_tag), window);
    if first {
        log_warn!(
            "guest: {result}, {} (ip={}); hjv-api admin unlock tag:{} lifts the address's",
            run.kind.named(),
            run.ip_tag,
            run.ip_tag
        );
    }
    let retry_after = retry_after(run, result).await;
    run.end(Outcome::Released, first.then_some(result), false)
        .await;
    too_often(retry_after)
}

/// When a try that the limit `result` refused could come again, in seconds: from the address's
/// counted tries, or from the tries of everyone in memory.
async fn retry_after(run: &Run<'_>, result: &'static str) -> u64 {
    let state = run.state;
    let (scope, windows) = match result {
        "limited-ip" => (SCOPE_GUEST_IP, state.config.limit_guest_ip.clone()),
        "limited-tickets" => (
            SCOPE_GUEST_TICKETS,
            state.config.limit_guest_tickets.clone(),
        ),
        _ => return state.guest_calls.retry_after(&[run.ip_tag.as_str()]),
    };
    let (key, at) = (run.ip_tag.clone(), now());
    state
        .db
        .call(move |c| limits::wait(c, scope, &key, &windows, at))
        .await
        .map_or(1, |secs| u64::try_from(secs).unwrap_or(1))
}

fn window_of(windows: &[crate::config::Window]) -> Duration {
    Duration::from_secs(windows.iter().map(|w| w.secs).max().unwrap_or(60) as u64)
}

/// Steps 4 to 8, for a request whose seal holds.
async fn exchange(run: &mut Run<'_>, asked: Asked, opened: Opened) -> Reply {
    let state = run.state;
    // 4. the address's tickets, and the tries of everyone
    match run
        .reserve(
            SCOPE_GUEST_TICKETS,
            state.config.limit_guest_tickets.clone(),
        )
        .await
    {
        Ok(Some(id)) => run.ticket = Some(id),
        Ok(None) => return limited(run, "limited-tickets").await,
        Err(()) => return run.fail(Outcome::Released, "error", unanswered()).await,
    }
    match state.guest_calls.try_take(&[run.ip_tag.as_str()]) {
        Take::Taken => run.call = true,
        Take::Full | Take::Share(_) => return limited(run, "limited-all").await,
    }
    // 5 and 6: the password, or the code
    let code = match run.kind {
        Kind::Password => {
            if let Err(reply) = check_password(run, opened).await {
                return reply;
            }
            None
        }
        Kind::Code => match find_code(run, &opened.password).await {
            Ok(key) => Some(key),
            Err(reply) => return reply,
        },
    };
    let kind = run.kind.record();
    // 7. a PC's key, as hbbs holds it
    let at = now();
    let (claims, what) = match &asked {
        Asked::Ctl { to } => (
            Claims::guest_ctl(to, at, state.config.guest_ticket_secs),
            "guest ctl",
        ),
        Asked::Pc { id, pk, .. } => {
            match known_key(state, id).await {
                Ok(Lookup::Known { public_key, .. }) if public_key != *pk => {
                    state.tickets.note(&format!("{kind} refused wrong-pc"));
                    let reply = run.kind.wrong_pc();
                    return run.fail(Outcome::Passed, "wrong-pc", reply).await;
                }
                Ok(_) => {}
                Err(e) => {
                    if state.first_notice("guest-hbbs", Duration::from_secs(600)) {
                        log_warn!("guest: the key hbbs holds for a PC is not known: {e}");
                    }
                    let reply = run.kind.unavailable();
                    return run.fail(Outcome::Released, "hbbs", reply).await;
                }
            }
            match run.kind {
                Kind::Password => (
                    Claims::guest_pc(id, *pk, at, state.config.guest_ticket_secs),
                    "guest pc",
                ),
                Kind::Code => (
                    Claims::guest_pc(id, *pk, at, support::TICKET_SECS),
                    "code pc",
                ),
            }
        }
    };
    // 8. the code is used with the ticket, and the ticket
    if let Some(key) = code {
        let result = match state.support.take(key, now()) {
            Taken::Taken => None,
            Taken::Used => Some("used"),
            Taken::Revoked => Some("revoked"),
            Taken::Expired => Some("expired"),
            Taken::Gone => Some("wrong"),
        };
        if let Some(result) = result {
            state.tickets.note(&format!("code refused {result}"));
            let reply = run.kind.denied();
            return run.fail(Outcome::Failed, result, reply).await;
        }
    }
    let issued = ticket::issue(state, &claims, what);
    if issued.is_err() {
        if let Some(key) = code {
            state.support.give_back(key);
        }
    }
    match issued {
        Ok(issued) => {
            let result = if claims.use_ == Use::Ctl {
                "ok-ctl"
            } else {
                "ok-pc"
            };
            run.end(Outcome::Passed, Some(result), true).await;
            Reply::ok(&issued.json(now()))
        }
        Err(NoTicket::Off) => {
            let reply = run.kind.unavailable();
            run.fail(Outcome::Released, "error", reply).await
        }
        Err(_) => run.fail(Outcome::Released, "error", unanswered()).await,
    }
}

/// Steps 5 and 6 for the access password: at most [`HASHES`] PBKDF2 at once, on a blocking
/// thread, against the file while it takes the password.
async fn check_password(run: &mut Run<'_>, opened: Opened) -> Result<(), Reply> {
    let state = run.state;
    // 5. the hash, on a blocking thread, two at a time
    let Ok(hash) = state.guest_hashes.clone().try_acquire_owned() else {
        state.tickets.note("guest refused busy");
        if state.first_notice("guest-hashes", Duration::from_secs(60)) {
            log_warn!("guest: {HASHES} access passwords are being checked already, refused");
        }
        return Err(run.fail(Outcome::Released, "busy", busy()).await);
    };
    // 6. the file and the password
    let (app, password, at) = (state.clone(), opened.password, now());
    let verdict = tokio::task::spawn_blocking(move || {
        let verdict = match app.guest.active(&app.config, at) {
            None => "closed",
            Some(guest) if guest.verify(&password) => "ok",
            Some(_) => "wrong",
        };
        drop(hash);
        drop::<Zeroizing<String>>(password);
        verdict
    })
    .await
    .unwrap_or("error");
    match verdict {
        "ok" => Ok(()),
        "closed" | "wrong" => {
            state.tickets.note(&format!("guest refused {verdict}"));
            Err(run
                .fail(Outcome::Failed, verdict, Kind::Password.denied())
                .await)
        }
        _ => {
            log_err!("guest: the check of the password failed");
            Err(run.fail(Outcome::Released, "error", unanswered()).await)
        }
    }
}

/// Steps 5 and 6 for a support code: the code in memory (support.rs), open, its maker's account
/// still on the allowlist and not blocked, and the account key that made it not ended; wrong,
/// over, used, blocked and revoked get the one 401. A code found blocked or revoked is ended for
/// good (support.rs `end_keys`): an unblock does not open it again. The code is not used yet:
/// that is step 8's.
async fn find_code(
    run: &mut Run<'_>,
    typed: &Zeroizing<String>,
) -> Result<support::CodeKey, Reply> {
    let state = run.state;
    let (key, maker, refused) = match state.support.find(typed, Use::Pc, now()) {
        Looked::Wrong => {
            state.tickets.note("code refused wrong");
            return Err(run
                .fail(Outcome::Failed, "wrong", Kind::Code.denied())
                .await);
        }
        Looked::Found { key, maker, state } => match state {
            CodeState::Open => (key, maker, None),
            CodeState::Expired => (key, maker, Some("expired")),
            CodeState::Used => (key, maker, Some("used")),
            CodeState::Revoked => (key, maker, Some("revoked")),
        },
    };
    run.account_tag = Some(maker.account_tag.clone());
    if let Some(result) = refused {
        state.tickets.note(&format!("code refused {result}"));
        return Err(run.fail(Outcome::Failed, result, Kind::Code.denied()).await);
    }
    // the maker's account: still there, the same account, on the allowlist and not blocked; and
    // the account key that made the code, not ended (a logout, a revoke, a block)
    let (id, ckid) = (maker.account_id, maker.ckid);
    let found = state
        .db
        .call(move |c| match db::account(c, id)? {
            Some(account) => {
                let allowed = db::allowed(c, &account.login_key)?;
                let key_active: bool = c.query_row(
                    "SELECT EXISTS (SELECT 1 FROM ctl_keys
                                    WHERE ckid = ?1 AND account_id = ?2 AND revoked_at IS NULL)",
                    [ckid, id],
                    |r| r.get(0),
                )?;
                Ok(Some((account.login_key, allowed, key_active)))
            }
            None => Ok(None),
        })
        .await;
    let refused = match found {
        Ok(Some((login_key, allowed, key_active))) => {
            if !allowed || state.tags.account(&login_key) != maker.account_tag {
                Some("blocked")
            } else if !key_active {
                Some("revoked")
            } else {
                None
            }
        }
        Ok(None) => Some("blocked"),
        Err(e) => {
            log_err!("guest: {e}");
            return Err(run.fail(Outcome::Released, "error", unanswered()).await);
        }
    };
    if let Some(result) = refused {
        // over for good: neither an unblock nor a new login of the account opens it again
        state.support.end_keys(id, &[ckid], now());
        state.tickets.note(&format!("code refused {result}"));
        return Err(run.fail(Outcome::Failed, result, Kind::Code.denied()).await);
    }
    Ok(key)
}

/// The key hbbs holds for the PC `id`: its last answer while that serves, else hbbs's now.
async fn known_key(state: &AppState, id: &str) -> Result<Lookup, hbbs::HbbsError> {
    if let Some(lookup) = state.guest_keys.get(id) {
        return Ok(lookup);
    }
    let lookup = hbbs::lookup(
        state.config.hbbs_console,
        state.config.hbbs_pubkey.as_ref(),
        id,
    )
    .await?;
    state.guest_keys.put(id, lookup.clone());
    Ok(lookup)
}

/// At the start: the file's own line says what was read or why it is not used; this one, that
/// no access password is taken.
pub fn note_at_start(state: &AppState) {
    if state.config.guest_file.is_some() && state.guest.active(&state.config, now()).is_none() {
        log_notice!(
            "guest: no access password (HJV_API_GUEST_FILE is not there, not used, or past its UNTIL)"
        );
    }
}

/// The lines `admin ticket status` adds: the file, the lifetime and the last day's tries by
/// result, never an address. It reads a file and the records.
pub async fn status(state: &Arc<AppState>) -> Result<String, String> {
    let mut out = String::new();
    let app = state.clone();
    let read = tokio::task::spawn_blocking(move || app.guest.read(&app.config))
        .await
        .ok()
        .flatten();
    let at = now();
    let file = match (&state.config.guest_file, read) {
        (None, _) => "off (HJV_API_GUEST_FILE is not set)".to_owned(),
        (Some(_), None) => {
            "closed: the file is not there or not used (the journal says why)".to_owned()
        }
        (Some(_), Some(guest)) if at >= guest.until => format!(
            "closed since {} (hash {})",
            datetime(guest.until),
            guest.tag
        ),
        (Some(_), Some(guest)) => {
            format!("on until {} (hash {})", datetime(guest.until), guest.tag)
        }
    };
    let _ = writeln!(out, "access password: {file}");
    let _ = writeln!(
        out,
        "access password's tickets: {} h (HJV_API_GUEST_TICKET_HOURS)",
        state.config.guest_ticket_secs / crate::util::HOUR
    );
    let since = at - DAY;
    let results = state
        .db
        .call(move |c| {
            let mut stmt = c.prepare(
                "SELECT result, COUNT(*) FROM events WHERE kind = 'guest' AND at > ?1
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
        "access password, the last day's tries: {}",
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

    const PASSWORD: &str = "correct horse battery staple";
    /// PBKDF2-HMAC-SHA256 of PASSWORD with the salt 00 01 … 0f and 100 000 iterations, as
    /// review.rs's vector.
    const HASH: &str =
        "pbkdf2-sha256$100000$AAECAwQFBgcICQoLDA0ODw==$SdScJfWXhGIJ8Nkud3CrZOHHXpS0zmxQkmXuZxddKh4=";

    fn file(hash: &str, until: &str) -> String {
        format!("HASH={hash}\nUNTIL={until}\n")
    }

    #[test]
    fn the_files_lines() {
        let until = "2026-11-29T15:00:00Z";
        let guest = Guest::parse(file(HASH, until).as_bytes()).unwrap();
        assert_eq!(guest.until(), 1_795_964_400);
        assert!(guest.verify(PASSWORD));
        assert!(!guest.verify("correct horse battery stapl"));
        assert_eq!(guest.tag(), &hex(&sha256(HASH.as_bytes())[..4]));
        // comments, blank lines, Windows line ends, blanks around a value, any order
        let guest = Guest::parse(
            format!("# the access password\r\n\r\nUNTIL= {until} \r\n  HASH={HASH}\r\n").as_bytes(),
        )
        .unwrap();
        assert!(guest.verify(PASSWORD));
        let refused = [
            (
                format!("{}HASH={HASH}\n", file(HASH, until)),
                "HASH or UNTIL is there more than once",
            ),
            (format!("HASH={HASH}\n"), "HASH or UNTIL is missing"),
            (String::new(), "HASH or UNTIL is missing"),
            (
                format!("{}ID=someone\n", file(HASH, until)),
                "a line that is neither HASH=, UNTIL= nor a # comment",
            ),
            (
                format!("{}PASSWORD={PASSWORD}\n", file(HASH, until)),
                "a line that is neither HASH=, UNTIL= nor a # comment",
            ),
            (
                file(PASSWORD, until),
                "HASH is not pbkdf2-sha256$<iterations>$<salt>$<key>",
            ),
            (
                file(&HASH.replace("$100000$", "$99999$"), until),
                "HASH's iterations are not 100000 to 10000000",
            ),
            (file(HASH, "2026-11-30"), "UNTIL is not a time"),
        ];
        for (text, why) in refused {
            let got = Guest::parse(text.as_bytes()).map(|_| ()).unwrap_err();
            assert_eq!(got, why, "{text:?}");
            assert!(!got.contains("AAECAw") && !got.contains("SdScJf") && !got.contains("horse"));
        }
        assert!(Guest::parse(&[0xff, 0xfe]).is_err());
    }

    #[test]
    fn the_bodies_of_the_two_requests() {
        let pk = b64(&devsig::public_key(&[2; 32]));
        let ctl = json!({"use": "ctl", "to": "123456789", "password": "x"}).to_string();
        let (asked, kind, password) = parse_body(ctl.as_bytes()).unwrap();
        assert_eq!(
            (asked.use_(), asked.pc(), kind, password.as_str()),
            ("ctl", "123456789", Kind::Password, "x")
        );
        let pc = json!({"use": "pc", "id": "Office-PC_7", "pk": pk, "sig": "s", "password": "x"})
            .to_string();
        let (asked, kind, _) = parse_body(pc.as_bytes()).unwrap();
        assert_eq!(
            (asked.use_(), asked.pc(), kind),
            ("pc", "Office-PC_7", Kind::Password)
        );
        // a PC's support code in the place of the password
        let code = json!({"use": "pc", "id": "Office-PC_7", "pk": pk, "sig": "s", "code": "c"})
            .to_string();
        let (asked, kind, field) = parse_body(code.as_bytes()).unwrap();
        assert_eq!(
            (asked.use_(), asked.pc(), kind, field.as_str()),
            ("pc", "Office-PC_7", Kind::Code, "c")
        );
        let bad = [
            json!({}).to_string(),
            json!([]).to_string(),
            "null".to_owned(),
            json!({"use": "ctl", "to": "123456789"}).to_string(),
            json!({"use": "ctl", "to": "123456789", "password": null}).to_string(),
            json!({"use": "ctl", "to": "123456789", "password": "x", "id": "1"}).to_string(),
            json!({"use": "ctl", "to": "12 3", "password": "x"}).to_string(),
            json!({"use": "ctl", "to": "", "password": "x"}).to_string(),
            json!({"use": "both", "to": "123456789", "password": "x"}).to_string(),
            json!({"use": "pc", "id": "1", "pk": pk, "password": "x"}).to_string(),
            json!({"use": "pc", "id": "1", "pk": &pk[..43], "sig": "s", "password": "x"})
                .to_string(),
            json!({"use": "pc", "id": "1", "pk": pk, "sig": "s", "password": "x", "x": "y"})
                .to_string(),
            json!({"use": "ctl", "to": "1", "password": 7}).to_string(),
            r#"{"use":"ctl","to":"1","to":"2","password":"x"}"#.to_owned(),
            // a support code: a PC's only, in the place of the password, once
            json!({"use": "ctl", "to": "123456789", "code": "c"}).to_string(),
            json!({"use": "pc", "id": "1", "pk": pk, "sig": "s", "password": "x", "code": "c"})
                .to_string(),
            json!({"use": "pc", "id": "1", "pk": pk, "sig": "s"}).to_string(),
            json!({"use": "pc", "id": "1", "pk": pk, "sig": "s", "code": null}).to_string(),
            format!(r#"{{"use":"pc","id":"1","pk":"{pk}","sig":"s","code":"c","code":"c"}}"#),
        ];
        for body in bad {
            assert!(parse_body(body.as_bytes()).is_err(), "{body}");
        }
    }

    #[test]
    fn what_a_pcs_key_signs() {
        // the layout of ticket.rs's PC part, with the seal's hash in place of the token's
        let message = pc_message("123456789", "PK", &[0u8; 32]);
        assert_eq!(
            message,
            [
                b"hjv-guest-pc-v1".as_slice(),
                b"123456789",
                b"PK",
                b64(&[0u8; 32]).as_bytes()
            ]
            .join(&0u8)
        );
    }
}

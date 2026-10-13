// SPDX-License-Identifier: AGPL-3.0-only

//! hjv-relay's side of hjv-api (relay/PROTOCOL.md, design v3 §8-3): `POST /api/hjv/relay/hello`
//! and `POST /api/hjv/relay/poll`, the relays' registration (pending, active, revoked), the
//! long-poll that hands an active relay its signed wake commands (wake.rs queues them), the
//! results it sends back, and the work of the admin commands `relay list | approve | revoke |
//! share`.
//!
//! **Requests** are the signed envelope of envelope.rs with the context `hjv-relay-v1` and the
//! operation `hello` or `poll`, at most 64 KiB, checked in PROTOCOL.md §4.3's order, the first
//! failure answering: 1 size (413 `too_large`), 2 shape (400 `bad_request`; for hello `pk` must be
//! there and `id` must be the relay ID of `pk`), 3 the key (hello: a relay kept under this ID keeps
//! its key, another one is 401 `bad_signature`; poll: the key kept, or 404 `unknown_relay`), 4 the
//! signature (strict, 401 `bad_signature`), 5 the time within 300 s (400 `clock` with
//! `server_time`), 6 the pair (`id`, `n`) not accepted in 600 s (409 `replay`; 503 `unavailable`
//! with `retry_after` while the store holds [`MAX_NONCES`]), 7 the payload (400 `bad_request`). A
//! request refused in steps 1-6 changes nothing; after them come the limits (429 `rate_limited`
//! with `retry_after`). Errors are `{"error":"<code>","server_time":<time>}`.
//!
//! **hello** (§5.1): a key never seen becomes a pending entry (its ID, key, time, egress address,
//! versions and interfaces), at most [`PENDING_PER_SOURCE`] new ones an hour per source address
//! (an IPv6 address by its /64) and [`PENDING_MAX`] in all, each for [`PENDING_LIFETIME`]; a key
//! whose code an admin approved ahead becomes active at once, whatever those limits. A known relay
//! updates what it reported (a revoked one nothing) and gets its state, at most
//! [`HELLOS_PER_HOUR`] an hour. The answer: `{"server_time","state","code"}`.
//!
//! **poll** (§5.2): applies the results (§8.2), stores what the relay reported, asks for its
//! interfaces when their summary changed (`send_ifaces`), and answers: a revoked relay at once; a
//! pending one when its state changes or after `min(wait, 20)` seconds; an active one at once
//! with its queued commands (§7.4: the exact text, signature and key number queued, oldest first,
//! at most [`MAX_COMMANDS`], until a result names the command or it expires), else when a
//! command is queued for it or after `min(wait, 20)` seconds. At most [`HOLDS_PER_RELAY`] polls of
//! a relay are held at once and [`POLLS_PER_MINUTE`] taken a minute, and the held ones are
//! answered when hjv-api stops, so that they do not keep it waiting. The time of its last poll
//! is its liveness: online within [`ONLINE_SECS`] seconds.
//!
//! **Results** (§8.2) are matched to their job by the command's SHA-256, idempotently: no error,
//! or `partial` with packets, makes the job `sent` at the relay's `sent_at`; `replayed` says
//! nothing new; any other error fails the job with that code (wake.rs shows which ones the apps
//! get). A result after the job failed for `no_result` still counts.

use crate::{
    api::{is_json, ApiRequest, BodyError, Reply},
    db::{self, NonceUse, RelayRow, RelaySeen, RelayState, Revoked},
    envelope::{self, Envelope, Refusal, Spec},
    state::AppState,
    util::{address_group, clean_text, date, now, sha256, valid_device_id, DAY, HOUR, MINUTE},
    wolinfo::{self, Egress, RelayIface},
};
use hyper::{header::ORIGIN, StatusCode};
use rusqlite::Connection;
use serde_json::{json, Map, Value};
use std::{
    collections::{HashMap, VecDeque},
    fmt::Write as _,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::Notify;

pub const HELLO_PATH: &str = "/api/hjv/relay/hello";
pub const POLL_PATH: &str = "/api/hjv/relay/poll";
/// The largest request body (PROTOCOL.md §1).
pub const MAX_BODY: usize = 65_536;
pub const HELLO: Spec = Spec {
    context: "hjv-relay-v1",
    op: "hello",
    max_body: MAX_BODY,
    id_ok: valid_relay_id,
    with_pk: true,
};
pub const POLL: Spec = Spec {
    op: "poll",
    with_pk: false,
    ..HELLO
};
/// How long a pending entry waits for its approval, and a pre-approval for its relay
/// (PROTOCOL.md §10.2).
pub const PENDING_LIFETIME: i64 = DAY;
pub const PREAPPROVAL_LIFETIME: i64 = 7 * DAY;
/// New pending entries per source address an hour, and pending entries in all.
pub const PENDING_PER_SOURCE: i64 = 5;
pub const PENDING_MAX: i64 = 20;
/// Hellos of a known relay an hour, and polls of a relay a minute (a relay that keeps to the
/// protocol polls at most every two seconds).
pub const HELLOS_PER_HOUR: usize = 60;
pub const POLLS_PER_MINUTE: usize = 120;
/// The longest hold of a poll, and the polls of one relay held at once.
pub const MAX_HOLD: u64 = 20;
pub const HOLDS_PER_RELAY: usize = 2;
/// The accepted (`id`, `n`) kept at most; past it, 503 (PROTOCOL.md §4.3).
pub const MAX_NONCES: i64 = 100_000;
const FULL_RETRY: u64 = 60;
/// A relay counts as online for this long after its last poll began (design v3 §8-4).
pub const ONLINE_SECS: i64 = 60;
/// Commands in one poll answer, results taken from one poll (PROTOCOL.md §10.2).
pub const MAX_COMMANDS: usize = 16;
pub const MAX_RESULTS: usize = 50;

// ---- identity ----

/// `r-` and 16 lowercase base32 letters (PROTOCOL.md §3.1).
pub fn valid_relay_id(id: &str) -> bool {
    id.len() == 18
        && id.starts_with("r-")
        && id[2..]
            .bytes()
            .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
}

/// RFC 4648 base32 of 10 bytes: 16 letters, no padding.
fn base32(bytes: &[u8; 10]) -> String {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let bits = bytes
        .iter()
        .fold(0u128, |acc, b| (acc << 8) | u128::from(*b));
    (0..16)
        .map(|i| char::from(ALPHABET[((bits >> (75 - 5 * i)) & 31) as usize]))
        .collect()
}

/// The relay ID of a public key: `r-` and base32 of the first 10 bytes of its SHA-256.
pub fn relay_id_of(pk: &[u8; 32]) -> String {
    let digest = sha256(pk);
    let mut first = [0u8; 10];
    first.copy_from_slice(&digest[..10]);
    format!("r-{}", base32(&first).to_ascii_lowercase())
}

/// The enrollment code of a relay ID: its 16 letters in upper case, four groups of four.
pub fn code_of(relay_id: &str) -> String {
    let letters = relay_id.trim_start_matches("r-").to_ascii_uppercase();
    letters
        .as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join("-")
}

/// The relay ID an admin means: an enrollment code in any case, with or without `-` and spaces
/// (PROTOCOL.md §3.1), or a relay ID.
pub fn parse_code(text: &str) -> Option<String> {
    let text = text.trim();
    let letters: String = match text.strip_prefix("r-") {
        Some(rest) => rest.to_owned(),
        None => text
            .chars()
            .filter(|c| *c != '-' && !c.is_whitespace())
            .collect(),
    };
    let id = format!("r-{}", letters.to_ascii_lowercase());
    valid_relay_id(&id).then_some(id)
}

/// What the apps call a relay unless the admin named it: `NAS` and the code's first group.
pub fn default_name(relay_id: &str) -> String {
    format!("NAS {}", short(&code_of(relay_id), 4))
}

// ---- the hub: long-polls and the per-relay limits ----

/// What the relays' requests share in memory: the signal that wakes held polls (a command queued,
/// a relay approved or revoked), the held polls per relay, and the per-relay limits.
#[derive(Default)]
pub struct RelayHub {
    changed: Notify,
    /// the service is stopping: no poll is held any more
    stopping: AtomicBool,
    holding: Mutex<HashMap<String, usize>>,
    hellos: Mutex<HashMap<String, VecDeque<Instant>>>,
    polls: Mutex<HashMap<String, VecDeque<Instant>>>,
}

/// A poll held; the count goes down when it is dropped.
pub struct Hold<'a> {
    hub: &'a RelayHub,
    relay_id: String,
}

impl Drop for Hold<'_> {
    fn drop(&mut self) {
        let mut holding = self.hub.holding.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = holding.get_mut(&self.relay_id) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                holding.remove(&self.relay_id);
            }
        }
    }
}

/// Takes a call of `key` from a window of `limit` calls per `window`; else the seconds until one
/// is free.
fn take(
    map: &Mutex<HashMap<String, VecDeque<Instant>>>,
    key: &str,
    limit: usize,
    window: Duration,
) -> Result<(), u64> {
    let mut map = map.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    let calls = map.entry(key.to_owned()).or_default();
    while calls
        .front()
        .is_some_and(|t| now.saturating_duration_since(*t) >= window)
    {
        calls.pop_front();
    }
    if calls.len() >= limit {
        let free = calls.front().map_or(window, |t| {
            window.saturating_sub(now.saturating_duration_since(*t))
        });
        return Err(free.as_secs().clamp(1, 3600));
    }
    calls.push_back(now);
    Ok(())
}

impl RelayHub {
    /// Something a held poll may wait for happened: they all look again.
    pub fn changed(&self) {
        self.changed.notify_waiters();
    }

    /// The service stops: the held polls are answered now, so that they do not keep it waiting.
    pub fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        self.changed.notify_waiters();
    }

    fn stopping(&self) -> bool {
        self.stopping.load(Ordering::SeqCst)
    }

    /// Holds a poll of `relay_id`, unless HOLDS_PER_RELAY are held already (or the service is
    /// stopping).
    pub fn hold(&self, relay_id: &str) -> Option<Hold<'_>> {
        if self.stopping() {
            return None;
        }
        let mut holding = self.holding.lock().unwrap_or_else(|e| e.into_inner());
        let n = holding.entry(relay_id.to_owned()).or_insert(0);
        if *n >= HOLDS_PER_RELAY {
            return None;
        }
        *n += 1;
        Some(Hold {
            hub: self,
            relay_id: relay_id.to_owned(),
        })
    }

    /// Whether a poll of the relay is held now (it is online).
    pub fn holding(&self, relay_id: &str) -> bool {
        self.holding
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(relay_id)
    }

    fn take_hello(&self, relay_id: &str) -> Result<(), u64> {
        take(
            &self.hellos,
            relay_id,
            HELLOS_PER_HOUR,
            Duration::from_secs(HOUR as u64),
        )
    }

    fn take_poll(&self, relay_id: &str) -> Result<(), u64> {
        take(
            &self.polls,
            relay_id,
            POLLS_PER_MINUTE,
            Duration::from_secs(MINUTE as u64),
        )
    }

    /// Forgets the calls past their windows.
    pub fn purge(&self) {
        let now = Instant::now();
        for (map, window) in [
            (&self.hellos, Duration::from_secs(HOUR as u64)),
            (&self.polls, Duration::from_secs(MINUTE as u64)),
        ] {
            map.lock()
                .unwrap_or_else(|e| e.into_inner())
                .retain(|_, calls| {
                    calls
                        .back()
                        .is_some_and(|t| now.saturating_duration_since(*t) < window)
                });
        }
    }
}

/// Whether a relay is online at `at`: it polled within ONLINE_SECS, or a poll of it is held.
pub fn online(state: &AppState, relay: &RelayRow, at: i64) -> bool {
    relay.last_poll_at.is_some_and(|t| t > at - ONLINE_SECS)
        || state.relays.holding(&relay.relay_id)
}

// ---- the payloads ----

/// What hello and poll payloads share (PROTOCOL.md §5).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Common {
    version: String,
    arch: String,
    os: String,
    kernel: String,
    ifaces_hash: String,
    ifaces: Option<Vec<RelayIface>>,
}

/// One entry of a poll's `results` (PROTOCOL.md §8.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayResult {
    pub cmd: Option<String>,
    pub sent_at: Option<i64>,
    pub packets: i64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Poll {
    common: Common,
    wait: u64,
    results: Vec<RelayResult>,
}

/// An `error` code's form (PROTOCOL.md §6): 1 to 32 lowercase letters, digits and `_`.
pub fn code_form(code: &str) -> bool {
    (1..=32).contains(&code.len())
        && code
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn common(object: &Map<String, Value>, ifaces_required: bool) -> Option<Common> {
    if object.get("v").and_then(Value::as_u64) != Some(1) {
        return None;
    }
    let text = |name: &str, max: usize| object.get(name)?.as_str().map(|t| clean_text(t, max));
    let ifaces = match object.get("ifaces") {
        Some(v) => Some(wolinfo::parse_ifaces(v)?),
        None if ifaces_required => return None,
        None => None,
    };
    Some(Common {
        version: text("version", 32)?,
        arch: text("arch", 32)?,
        os: text("os", 100)?,
        kernel: text("kernel", 100)?,
        ifaces_hash: text("ifaces_hash", 128)?,
        ifaces,
    })
}

fn parse_hello(p: &str) -> Option<Common> {
    let value: Value = serde_json::from_str(p).ok()?;
    common(value.as_object()?, true)
}

fn parse_poll(p: &str) -> Option<Poll> {
    let value: Value = serde_json::from_str(p).ok()?;
    let object = value.as_object()?;
    let common = common(object, false)?;
    let wait = object.get("wait")?.as_u64()?;
    // an entry not of the form is left out rather than refusing the poll: the relay would send it
    // again with every poll
    let results = object
        .get("results")?
        .as_array()?
        .iter()
        .take(MAX_RESULTS)
        .filter_map(result_entry)
        .collect();
    Some(Poll {
        common,
        wait: wait.min(MAX_HOLD),
        results,
    })
}

fn result_entry(value: &Value) -> Option<RelayResult> {
    let object = value.as_object()?;
    let cmd = match object.get("cmd")? {
        Value::Null => None,
        Value::String(c)
            if c.len() == 64
                && c.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) =>
        {
            Some(c.clone())
        }
        _ => return None,
    };
    let error = match object.get("error") {
        None | Some(Value::Null) => None,
        Some(Value::String(e)) if code_form(e) => Some(e.clone()),
        Some(_) => Some("bad_result".to_owned()),
    };
    Some(RelayResult {
        cmd,
        sent_at: object.get("sent_at").and_then(Value::as_i64),
        packets: object
            .get("packets")
            .and_then(Value::as_u64)
            .map_or(0, |n| i64::try_from(n).unwrap_or(i64::MAX)),
        error,
    })
}

// ---- the requests ----

/// What a relay request came to.
#[derive(Debug)]
enum Decided {
    Refused(Refusal),
    UnknownRelay,
    /// the nonce store is full
    Full,
    Limited(u64),
    Hello {
        state: RelayState,
        /// approved ahead: active at its first hello
        activated: bool,
    },
    Poll {
        state: RelayState,
        commands: Vec<(String, String, i64)>,
        send_ifaces: bool,
        wait: u64,
        /// result codes worth a line in the journal
        notes: Vec<String>,
    },
}

/// The interfaces as kept, with their summary.
fn ifaces_json(ifaces: &[RelayIface]) -> String {
    serde_json::to_string(ifaces).unwrap_or_else(|_| "[]".to_owned())
}

/// A pending entry past its lifetime counts as unknown.
fn live(row: Option<RelayRow>, at: i64) -> Option<RelayRow> {
    row.filter(|r| !(r.state == RelayState::Pending && r.created_at <= at - PENDING_LIFETIME))
}

/// Steps 3 to 7 of a hello and its work, in one transaction (a refusal rolls back).
fn hello(
    c: &mut Connection,
    env: &Envelope,
    at: i64,
    ip: &str,
    hub: &RelayHub,
) -> rusqlite::Result<Decided> {
    let tx = c.transaction()?;
    let Some(pk) = env.pk else {
        return Ok(Decided::Refused(Refusal::BadRequest));
    };
    // 3. the key: a relay kept under this ID keeps its key (an expired pending entry is gone)
    let row = live(db::relay(&tx, &env.id)?, at);
    if row.as_ref().is_some_and(|r| r.pk != pk) {
        return Ok(Decided::Refused(Refusal::BadSignature));
    }
    // 4. the signature, 5. the time, 6. the nonce
    if !env.verify(&HELLO, &pk) {
        return Ok(Decided::Refused(Refusal::BadSignature));
    }
    if !env.fresh(at) {
        return Ok(Decided::Refused(Refusal::Clock));
    }
    match db::use_relay_nonce(
        &tx,
        &env.id,
        &env.nonce,
        at,
        at + envelope::NONCE_TTL,
        MAX_NONCES,
    )? {
        NonceUse::Fresh => {}
        NonceUse::Replayed => return Ok(Decided::Refused(Refusal::Replay)),
        NonceUse::Full => return Ok(Decided::Full),
    }
    // 7. the payload (the nonce stays used)
    let Some(hello) = parse_hello(&env.p) else {
        tx.commit()?;
        return Ok(Decided::Refused(Refusal::BadRequest));
    };
    let ifaces = ifaces_json(hello.ifaces.as_deref().unwrap_or_default());
    let seen = RelaySeen {
        version: &hello.version,
        arch: &hello.arch,
        os: &hello.os,
        kernel: &hello.kernel,
        egress_ip: ip,
        ifaces: Some((&ifaces, &hello.ifaces_hash)),
        poll: false,
    };
    let decided = match row {
        Some(r) => match hub.take_hello(&env.id) {
            Err(retry) => Decided::Limited(retry),
            Ok(()) => {
                db::relay_seen(&tx, &env.id, &seen, at)?;
                Decided::Hello {
                    state: r.state,
                    activated: false,
                }
            }
        },
        None => {
            // an expired pending entry makes room for the new one, with the same code
            db::drop_expired_pending(&tx, at - PENDING_LIFETIME)?;
            match db::preapproval(&tx, &env.id, at)? {
                Some(pre) => {
                    db::insert_relay(
                        &tx,
                        &db::NewRelay {
                            relay_id: &env.id,
                            pk: &pk,
                            state: RelayState::Active,
                            owner_account: Some(pre.account_id),
                            name: &pre.name,
                            source: None,
                            at,
                        },
                    )?;
                    db::delete_preapproval(&tx, &env.id)?;
                    db::relay_seen(&tx, &env.id, &seen, at)?;
                    Decided::Hello {
                        state: RelayState::Active,
                        activated: true,
                    }
                }
                None => {
                    let source = address_group(ip);
                    let (total, from_source, first) = db::pending_counts(&tx, &source, at - HOUR)?;
                    if from_source >= PENDING_PER_SOURCE {
                        let free = first.map_or(HOUR, |f| f + HOUR - at);
                        Decided::Limited(free.clamp(1, 3600) as u64)
                    } else if total >= PENDING_MAX {
                        let free =
                            db::oldest_pending(&tx)?.map_or(HOUR, |o| o + PENDING_LIFETIME - at);
                        Decided::Limited(free.clamp(60, 3600) as u64)
                    } else {
                        db::insert_relay(
                            &tx,
                            &db::NewRelay {
                                relay_id: &env.id,
                                pk: &pk,
                                state: RelayState::Pending,
                                owner_account: None,
                                name: "",
                                source: Some(&source),
                                at,
                            },
                        )?;
                        db::relay_seen(&tx, &env.id, &seen, at)?;
                        Decided::Hello {
                            state: RelayState::Pending,
                            activated: false,
                        }
                    }
                }
            }
        }
    };
    tx.commit()?;
    Ok(decided)
}

/// Steps 3 to 7 of a poll and its work, in one transaction (a refusal rolls back).
fn poll(
    c: &mut Connection,
    env: &Envelope,
    at: i64,
    ip: &str,
    hub: &RelayHub,
) -> rusqlite::Result<Decided> {
    let tx = c.transaction()?;
    // 3. the key kept for the ID
    let Some(relay) = live(db::relay(&tx, &env.id)?, at) else {
        return Ok(Decided::UnknownRelay);
    };
    // 4. the signature, 5. the time, 6. the nonce
    if !env.verify(&POLL, &relay.pk) {
        return Ok(Decided::Refused(Refusal::BadSignature));
    }
    if !env.fresh(at) {
        return Ok(Decided::Refused(Refusal::Clock));
    }
    match db::use_relay_nonce(
        &tx,
        &env.id,
        &env.nonce,
        at,
        at + envelope::NONCE_TTL,
        MAX_NONCES,
    )? {
        NonceUse::Fresh => {}
        NonceUse::Replayed => return Ok(Decided::Refused(Refusal::Replay)),
        NonceUse::Full => return Ok(Decided::Full),
    }
    // 7. the payload (the nonce stays used)
    let Some(poll) = parse_poll(&env.p) else {
        tx.commit()?;
        return Ok(Decided::Refused(Refusal::BadRequest));
    };
    if let Err(retry) = hub.take_poll(&env.id) {
        tx.commit()?;
        return Ok(Decided::Limited(retry));
    }
    // 1. the results; then what the relay reported
    let notes = if relay.state == RelayState::Active {
        apply_results(&tx, &relay, &poll.results, at)?
    } else {
        Vec::new()
    };
    let ifaces = poll.common.ifaces.as_deref().map(ifaces_json);
    db::relay_seen(
        &tx,
        &env.id,
        &RelaySeen {
            version: &poll.common.version,
            arch: &poll.common.arch,
            os: &poll.common.os,
            kernel: &poll.common.kernel,
            egress_ip: ip,
            ifaces: ifaces
                .as_deref()
                .map(|json| (json, poll.common.ifaces_hash.as_str())),
            poll: true,
        },
        at,
    )?;
    // 2. the interfaces again when their summary is not the one kept
    let send_ifaces = relay.state != RelayState::Revoked
        && poll.common.ifaces.is_none()
        && relay.ifaces_hash.as_deref() != Some(poll.common.ifaces_hash.as_str());
    let commands = if relay.state == RelayState::Active {
        db::queued_commands(&tx, &env.id, at, MAX_COMMANDS)?
    } else {
        Vec::new()
    };
    tx.commit()?;
    Ok(Decided::Poll {
        state: relay.state,
        commands,
        send_ifaces,
        wait: poll.wait,
        notes,
    })
}

/// The results of a poll (PROTOCOL.md §8.2), matched by their command's hash to the relay's jobs;
/// returns the codes worth a journal line.
fn apply_results(
    c: &Connection,
    relay: &RelayRow,
    results: &[RelayResult],
    at: i64,
) -> rusqlite::Result<Vec<String>> {
    let mut notes = Vec::new();
    for r in results {
        let Some(cmd) = r.cmd.as_deref() else {
            notes.push(r.error.clone().unwrap_or_else(|| "no_cmd".to_owned()));
            continue;
        };
        let Some(job) = db::job_by_cmd(c, &relay.relay_id, cmd)? else {
            notes.push("unknown_cmd".to_owned());
            continue;
        };
        match r.error.as_deref() {
            None => sent(c, &job, r, at)?,
            Some("partial") if r.packets > 0 => sent(c, &job, r, at)?,
            // the relay ran it before: nothing new
            Some("replayed") => {}
            Some(code) => {
                if db::job_failed(c, &job.job, code, at)? {
                    log_info!(
                        "wake: failed, {code} (job {}, relay {})",
                        short(&job.job, 8),
                        relay_tag(relay)
                    );
                }
                db::delete_command(c, cmd)?;
                if !crate::wake::EXPECTED_ERRORS.contains(&code) {
                    notes.push(code.to_owned());
                }
            }
        }
    }
    Ok(notes)
}

fn sent(c: &Connection, job: &db::Job, r: &RelayResult, at: i64) -> rusqlite::Result<()> {
    // the relay's estimate of the server's time, within the job's own time
    let sent_at = r
        .sent_at
        .unwrap_or(at)
        .clamp(job.created_at, at.max(job.created_at));
    if db::job_sent(c, &job.job, sent_at, r.packets)? {
        log_info!(
            "wake: sent, {} packets (job {})",
            r.packets,
            short(&job.job, 8)
        );
    }
    db::delete_command(c, &job.cmd)
}

/// A relay or a job as the journal names it: the start of its ID (a relay ID is a digest of the
/// relay's key, nobody's name; `relay list` shows it whole).
pub fn short(id: &str, n: usize) -> &str {
    id.get(..n).unwrap_or(id)
}

fn relay_tag(relay: &RelayRow) -> &str {
    short(&relay.relay_id, 10)
}

/// An error answer of the relay protocol, with `retry_after` where it helps (PROTOCOL.md §6).
fn retry_reply(status: StatusCode, code: &str, retry_after: u64) -> Reply {
    Reply::json(
        status,
        &json!({ "error": code, "retry_after": retry_after, "server_time": now() }),
    )
}

/// Handles `POST /api/hjv/relay/hello` and `/poll` (the caller checked the method).
pub async fn handle(state: &Arc<AppState>, req: ApiRequest) -> Reply {
    if req.headers.contains_key(ORIGIN) {
        return envelope::error_reply(StatusCode::FORBIDDEN, "forbidden");
    }
    let body = match &req.body {
        Ok(body) => body,
        Err(BodyError::TooLarge) => return envelope::refusal_reply(Refusal::TooLarge),
        Err(BodyError::Unreadable) => {
            return envelope::error_reply(StatusCode::SERVICE_UNAVAILABLE, "unavailable")
        }
    };
    // 1. size and 2. shape, with the type of the body
    if body.len() > MAX_BODY {
        return envelope::refusal_reply(Refusal::TooLarge);
    }
    if !is_json(&req.headers) {
        return envelope::refusal_reply(Refusal::BadRequest);
    }
    let is_hello = req.path == HELLO_PATH;
    let spec = if is_hello { &HELLO } else { &POLL };
    let env = match Envelope::parse(body, spec) {
        Ok(env) => env,
        Err(refusal) => return envelope::refusal_reply(refusal),
    };
    if is_hello && env.pk.map(|pk| relay_id_of(&pk)).as_deref() != Some(env.id.as_str()) {
        return envelope::refusal_reply(Refusal::BadRequest);
    }
    let (at, ip, id) = (now(), req.ip.clone(), env.id.clone());
    // armed before the database is read, so that a command queued in between is not missed
    let notified = state.relays.changed.notified();
    tokio::pin!(notified);
    notified.as_mut().enable();
    let app = state.clone();
    let decided = state
        .db
        .call(move |c| {
            if is_hello {
                hello(c, &env, at, &ip, &app.relays)
            } else {
                poll(c, &env, at, &ip, &app.relays)
            }
        })
        .await;
    match decided {
        Ok(Decided::Refused(refusal)) => envelope::refusal_reply(refusal),
        Ok(Decided::UnknownRelay) => envelope::error_reply(StatusCode::NOT_FOUND, "unknown_relay"),
        Ok(Decided::Full) => {
            if state.first_notice("relay-nonces-full", Duration::from_secs(600)) {
                log_err!(
                    "relay: {MAX_NONCES} request nonces of the last ten minutes: relay requests get 503 until they expire (a flood?)"
                );
            }
            retry_reply(StatusCode::SERVICE_UNAVAILABLE, "unavailable", FULL_RETRY)
        }
        Ok(Decided::Limited(retry)) => {
            retry_reply(StatusCode::TOO_MANY_REQUESTS, "rate_limited", retry)
        }
        Ok(Decided::Hello {
            state: relay_state,
            activated,
        }) => {
            let tag = short(&id, 10);
            if activated {
                log_notice!("relay: {tag} said hello and is active (approved ahead)");
            } else if relay_state == RelayState::Pending
                && state.first_notice(
                    &format!("relay-pending:{id}"),
                    Duration::from_secs(HOUR as u64),
                )
            {
                log_notice!("relay: {tag} waits for approval (relay list)");
            }
            Reply::ok(&json!({
                "server_time": now(),
                "state": relay_state.as_str(),
                "code": code_of(&id),
            }))
        }
        Ok(Decided::Poll {
            state: relay_state,
            commands,
            send_ifaces,
            wait,
            notes,
        }) => {
            note_results(state, &id, &notes);
            let (relay_state, commands) =
                hold(state, &id, relay_state, commands, wait, notified).await;
            poll_reply(relay_state, &commands, send_ifaces)
        }
        Err(e) => {
            log_err!("relay: {e}");
            envelope::error_reply(StatusCode::SERVICE_UNAVAILABLE, "unavailable")
        }
    }
}

/// Holds a poll (PROTOCOL.md §5.2): an active relay with nothing to do until a command is
/// queued for it, a pending one until its state changes, at most `wait` seconds.
async fn hold<'a>(
    state: &'a Arc<AppState>,
    id: &str,
    relay_state: RelayState,
    commands: Vec<(String, String, i64)>,
    wait: u64,
    mut notified: std::pin::Pin<&mut tokio::sync::futures::Notified<'a>>,
) -> (RelayState, Vec<(String, String, i64)>) {
    let waits = match relay_state {
        RelayState::Active => commands.is_empty(),
        RelayState::Pending => true,
        RelayState::Revoked => false,
    };
    if !waits || wait == 0 {
        return (relay_state, commands);
    }
    let Some(_held) = state.relays.hold(id) else {
        return (relay_state, commands);
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(wait);
    loop {
        tokio::select! {
            () = &mut notified => {}
            () = tokio::time::sleep_until(deadline) => return (relay_state, commands),
        }
        if state.relays.stopping() {
            return (relay_state, commands);
        }
        notified.set(state.relays.changed.notified());
        notified.as_mut().enable();
        let (relay_id, at) = (id.to_owned(), now());
        let looked = state
            .db
            .call(move |c| {
                let Some(relay) = live(db::relay(c, &relay_id)?, at) else {
                    return Ok(None);
                };
                let commands = if relay.state == RelayState::Active {
                    db::queued_commands(c, &relay_id, at, MAX_COMMANDS)?
                } else {
                    Vec::new()
                };
                Ok(Some((relay.state, commands)))
            })
            .await;
        match looked {
            Ok(Some((now_state, now_commands))) => {
                if now_state != relay_state || !now_commands.is_empty() {
                    return (now_state, now_commands);
                }
            }
            Ok(None) => return (relay_state, commands),
            Err(e) => {
                log_err!("relay: {e}");
                return (relay_state, commands);
            }
        }
    }
}

fn poll_reply(state: RelayState, commands: &[(String, String, i64)], send_ifaces: bool) -> Reply {
    let mut body = json!({ "server_time": now(), "state": state.as_str() });
    if state == RelayState::Active {
        body["commands"] = commands
            .iter()
            .map(|(c, s, k)| json!({ "c": c, "s": s, "k": k }))
            .collect();
    }
    if send_ifaces {
        body["send_ifaces"] = json!(true);
    }
    Reply::ok(&body)
}

/// The refusals of PROTOCOL.md §8.1 that say the relay and hjv-api disagree, and hjv-api's own
/// names for results it cannot place: each has its line in the journal once an hour per relay.
const NOTED_CODES: &[&str] = &[
    "unknown_key",
    "bad_signature",
    "bad_command",
    "bad_version",
    "wrong_relay",
    "not_yet_valid",
    "bad_lifetime",
    "bad_macs",
    "bad_params",
    "bad_result",
    "unknown_cmd",
    "no_cmd",
];

/// Results that tell the operator something: a key the relays do not trust, a command they
/// refused as malformed, results of commands hjv-api does not know.
fn note_results(state: &AppState, id: &str, notes: &[String]) {
    let tag = short(id, 10);
    for code in notes {
        // codes the protocol does not name share one line an hour, so that a relay cannot fill
        // the notices (or the journal) with names of its own
        let named = if NOTED_CODES.contains(&code.as_str()) {
            code.as_str()
        } else {
            "other"
        };
        if !state.first_notice(
            &format!("relay-result:{id}:{named}"),
            Duration::from_secs(HOUR as u64),
        ) {
            continue;
        }
        match code.as_str() {
            "unknown_key" | "bad_signature" => log_err!(
                "relay: {tag} refused a wake command ({code}): it does not trust the command key in use (HJV_API_COMMAND_KID; hjv-api selfcheck, and the relay's hjv-relay version)"
            ),
            "unknown_cmd" | "no_cmd" => log_notice!(
                "relay: {tag} sent a result for a command hjv-api does not know ({code}); the next hour is not logged"
            ),
            _ => log_warn!(
                "relay: {tag} refused a wake command ({code}); the next hour is not logged"
            ),
        }
    }
}

// ---- the admin commands (admin.rs) ----

/// How the PC with the ID or tag `arg` is named in an admin command: its device row.
async fn named_pc(state: &Arc<AppState>, arg: &str) -> Result<db::Device, String> {
    let arg = arg.trim();
    let devices = state
        .db
        .call(|c| db::all_devices(c))
        .await
        .map_err(|e| e.to_string())?;
    let found = match arg.strip_prefix("tag:") {
        Some(tag) => devices
            .into_iter()
            .find(|d| state.tags.device(&d.device_id) == tag.to_ascii_lowercase()),
        None if valid_device_id(arg) => devices.into_iter().find(|d| d.device_id == arg),
        None => return Err("not a device ID".into()),
    };
    found.ok_or_else(|| "no account has that PC".to_owned())
}

fn ago(secs: i64) -> String {
    let secs = secs.max(0);
    if secs < MINUTE {
        format!("{secs} s")
    } else if secs < HOUR {
        format!("{} min", secs / MINUTE)
    } else if secs < DAY {
        format!("{} h", secs / HOUR)
    } else {
        format!("{} days", secs / DAY)
    }
}

/// The verdict lines of a relay and a PC (PROTOCOL.md §11): default output holds no address,
/// network or MAC; `reveal` adds them, MACs masked to their last two bytes.
fn verdict_lines(
    out: &mut String,
    relay: &RelayRow,
    info: Option<&db::WolInfo>,
    at: i64,
    reveal: bool,
    indent: &str,
) -> wolinfo::Verdict {
    let verdict = match info {
        Some(info) => wolinfo::judge(
            &relay.ifaces,
            relay.egress_ip.as_deref(),
            &info.report,
            Some(&info.egress_ip),
            &[],
        ),
        None => wolinfo::Verdict {
            grade: None,
            net: None,
            overlap: false,
            egress: Egress::Unknown,
            private: false,
        },
    };
    let network = match (info, verdict.grade) {
        (None, _) => "unknown (the PC has not reported how to wake it yet)".to_owned(),
        _ if relay.ifaces.is_empty() => {
            "unknown (the relay reported no network interface)".to_owned()
        }
        (_, Some(wolinfo::Grade::Exact)) => "yes (the same network and gateway)".to_owned(),
        (_, Some(_)) => "yes (overlapping networks, the same gateway)".to_owned(),
        _ if verdict.elsewhere() => {
            "no (a private network of another site: the egress addresses differ)".to_owned()
        }
        _ if verdict.overlap => {
            "not confirmed (overlapping networks, another gateway or none known)".to_owned()
        }
        _ => "no (no network in common)".to_owned(),
    };
    let egress = match verdict.egress {
        Egress::Same => "yes",
        Egress::Different => "no",
        Egress::Unknown => "unknown",
    };
    let _ = writeln!(out, "{indent}on the PC's network: {network}");
    let _ = writeln!(out, "{indent}the same egress address as the PC: {egress}");
    if let Some(info) = info {
        let _ = writeln!(
            out,
            "{indent}the PC reported how to wake it {} ago",
            ago(at - info.reported_at)
        );
    }
    if reveal {
        for iface in &relay.ifaces {
            let _ = writeln!(
                out,
                "{indent}relay {} {} gateway {} (gateway MAC {}){}",
                iface.name,
                if iface.ipv4.is_empty() {
                    "-".to_owned()
                } else {
                    iface.ipv4.join(" ")
                },
                iface.gateway.as_deref().unwrap_or("-"),
                iface
                    .gateway_mac
                    .as_deref()
                    .map_or_else(|| "-".to_owned(), wolinfo::mask_mac),
                if iface.running { "" } else { ", no carrier" }
            );
        }
        if let Some(info) = info {
            for a in &info.report.adapters {
                let _ = writeln!(
                    out,
                    "{indent}PC card {} {} gateway {} (gateway MAC {})",
                    wolinfo::mask_mac(&a.mac),
                    if a.ipv4.is_empty() {
                        "-".to_owned()
                    } else {
                        a.ipv4.join(" ")
                    },
                    a.gateway.as_deref().unwrap_or("-"),
                    a.gateway_mac
                        .as_deref()
                        .map_or_else(|| "-".to_owned(), wolinfo::mask_mac),
                );
            }
        }
        let _ = writeln!(
            out,
            "{indent}egress: relay {}, PC {}",
            relay.egress_ip.as_deref().unwrap_or("-"),
            info.map_or("-", |i| i.egress_ip.as_str())
        );
    }
    verdict
}

fn relay_line(relay: &RelayRow) -> String {
    format!(
        "hjv-relay {} ({}, {})",
        relay.version.as_deref().unwrap_or("-"),
        relay.os.as_deref().unwrap_or("-"),
        relay.arch.as_deref().unwrap_or("-")
    )
}

/// `relay list`: the relays with their state, account and last poll; for an active relay the
/// verdicts against each PC of its account (PROTOCOL.md §11), values with `--reveal`; the codes
/// approved ahead.
pub async fn admin_list(state: &Arc<AppState>, reveal: bool) -> Result<String, String> {
    let at = now();
    let (relays, preapprovals, accounts, devices, infos) = state
        .db
        .call(move |c| {
            let relays = db::all_relays(c)?;
            let mut infos = Vec::new();
            for relay in &relays {
                if let Some(account) = relay.owner_account {
                    infos.extend(db::wol_infos_of(c, account)?);
                }
            }
            Ok((
                relays,
                db::preapprovals(c, at)?,
                db::accounts(c)?,
                db::all_devices(c)?,
                infos,
            ))
        })
        .await
        .map_err(|e| e.to_string())?;
    let relays: Vec<RelayRow> = relays
        .into_iter()
        .filter(|r| !(r.state == RelayState::Pending && r.created_at <= at - PENDING_LIFETIME))
        .collect();
    let count = |s: RelayState| relays.iter().filter(|r| r.state == s).count();
    let account_tag = |id: Option<i64>| {
        id.and_then(|id| accounts.iter().find(|a| a.id == id))
            .map_or_else(|| "-".to_owned(), |a| state.tags.account(&a.login_key))
    };
    let mut out = format!(
        "relays: {} active, {} pending, {} revoked; codes approved ahead: {}\n",
        count(RelayState::Active),
        count(RelayState::Pending),
        count(RelayState::Revoked),
        preapprovals.len()
    );
    for relay in &relays {
        match relay.state {
            RelayState::Active => {
                let _ = writeln!(
                    out,
                    "  {}  active   account {}  \"{}\"  approved {}  last poll {}  {}",
                    relay.relay_id,
                    account_tag(relay.owner_account),
                    relay.name,
                    relay.approved_at.map_or_else(|| "-".to_owned(), date),
                    relay
                        .last_poll_at
                        .map_or_else(|| "never".to_owned(), |t| format!("{} ago", ago(at - t))),
                    relay_line(relay)
                );
                if relay.scope == "shared" {
                    let _ = writeln!(
                        out,
                        "      shared: on (kept for later; W2 wakes its account's PCs only)"
                    );
                }
                let pcs: Vec<&db::Device> = devices
                    .iter()
                    .filter(|d| Some(d.account_id) == relay.owner_account)
                    .collect();
                if pcs.is_empty() {
                    let _ = writeln!(out, "      the account has no PC");
                }
                for pc in pcs {
                    let _ = writeln!(
                        out,
                        "      PC {} {}:",
                        state.tags.device(&pc.device_id),
                        crate::util::mask(&pc.device_id)
                    );
                    let info = infos.iter().find(|i| i.device_id == pc.device_id);
                    verdict_lines(&mut out, relay, info, at, reveal, "        ");
                }
            }
            RelayState::Pending => {
                let _ = writeln!(
                    out,
                    "  {}  pending  code {}  waiting {}  from {}  {}",
                    relay.relay_id,
                    code_of(&relay.relay_id),
                    ago(at - relay.created_at),
                    relay
                        .source
                        .as_deref()
                        .map_or_else(|| "-".to_owned(), |s| format!("tag {}", state.tags.ip(s))),
                    relay_line(relay)
                );
                if reveal {
                    for iface in &relay.ifaces {
                        let _ = writeln!(
                            out,
                            "      {} {} gateway {}",
                            iface.name,
                            iface.ipv4.join(" "),
                            iface.gateway.as_deref().unwrap_or("-")
                        );
                    }
                    let _ = writeln!(
                        out,
                        "      egress {}",
                        relay.egress_ip.as_deref().unwrap_or("-")
                    );
                }
            }
            RelayState::Revoked => {
                let _ = writeln!(
                    out,
                    "  {}  revoked  {}",
                    relay.relay_id,
                    relay.revoked_at.map_or_else(|| "-".to_owned(), date)
                );
            }
        }
    }
    for p in &preapprovals {
        let _ = writeln!(
            out,
            "  approved ahead: code {}  account {}  \"{}\"  until {}",
            code_of(&p.relay_id),
            account_tag(Some(p.account_id)),
            p.name,
            date(p.expires_at)
        );
    }
    Ok(out)
}

/// `relay approve <code> --owner-of <PC ID> [--name <name>] [--force]` (PROTOCOL.md §11, design
/// v3 §8-3): a pending relay becomes active for the account that has the PC, once its verdict says
/// that it is on the PC's network (or `--force`, after the owner confirmed the code); a code
/// without a pending entry is approved ahead for PREAPPROVAL_LIFETIME.
pub async fn admin_approve(
    state: &Arc<AppState>,
    code: &str,
    pc: &str,
    name: Option<&str>,
    force: bool,
    reveal: bool,
) -> Result<String, String> {
    let relay_id = parse_code(code).ok_or(
        "an enrollment code is 16 letters A-Z and digits 2-7, such as KZDV-VJ2U-MNDU-YAUF",
    )?;
    let device = named_pc(state, pc).await?;
    let name = match name.map(|n| clean_text(n, 32)) {
        Some(n) if !n.is_empty() => n,
        Some(_) => return Err("--name takes a name of 1 to 32 characters".into()),
        None => default_name(&relay_id),
    };
    let (account_id, device_id, at) = (device.account_id, device.device_id.clone(), now());
    let (relay, info, account) = {
        let (id, dev) = (relay_id.clone(), device_id.clone());
        state
            .db
            .call(move |c| {
                Ok((
                    live(db::relay(c, &id)?, at),
                    db::wol_info(c, &dev)?,
                    db::account(c, account_id)?,
                ))
            })
            .await
            .map_err(|e| e.to_string())?
    };
    let account_tag = account.map_or_else(|| "-".to_owned(), |a| state.tags.account(&a.login_key));
    let mut out = String::new();
    let Some(relay) = relay else {
        // approved ahead: active at its first hello
        let p = db::Preapproval {
            relay_id: relay_id.clone(),
            account_id,
            name: name.clone(),
            created_at: at,
            expires_at: at + PREAPPROVAL_LIFETIME,
        };
        state
            .db
            .call(move |c| db::in_tx(c, |tx| db::preapprove(tx, &p)))
            .await
            .map_err(|e| e.to_string())?;
        let _ = writeln!(
            out,
            "approved ahead: code {} for the account {account_tag}, until {}",
            code_of(&relay_id),
            date(at + PREAPPROVAL_LIFETIME)
        );
        let _ = writeln!(out, "the apps will call it \"{name}\"");
        out.push_str("no relay has said hello with this code yet: it becomes active at its first hello; then compare its networks with the PC's (relay list) and revoke it if they do not match\n");
        return Ok(out);
    };
    match relay.state {
        RelayState::Revoked => {
            return Err(format!(
                "the relay {relay_id} was revoked: its key is not approved again (the NAS makes a new key, and so a new code, when its relay.key is deleted)"
            ))
        }
        RelayState::Active if relay.owner_account != Some(account_id) => {
            return Err(format!(
                "the relay {relay_id} is active for another account: revoke it first"
            ))
        }
        _ => {}
    }
    let _ = writeln!(
        out,
        "the relay {relay_id} (code {}): first hello {} ago, {}",
        code_of(&relay_id),
        ago(at - relay.created_at),
        relay_line(&relay)
    );
    let _ = writeln!(
        out,
        "the PC {} {} of the account {account_tag}:",
        state.tags.device(&device_id),
        crate::util::mask(&device_id)
    );
    let verdict = verdict_lines(&mut out, &relay, info.as_ref(), at, reveal, "  ");
    // the outcome first: the operator record keeps the first line
    if relay.state == RelayState::Active {
        return Ok(format!(
            "already active for that account (no change)\n{out}"
        ));
    }
    if !verdict.same_network() && !force {
        return Err(format!(
            "{out}nothing changed: the verdict is not \"on the PC's network\". Check with the PC's owner that the code is the one their NAS shows (its hjv-relay.log) and that the NAS is on the PC's network; approve anyway with --force"
        ));
    }
    let (id, n) = (relay_id.clone(), name.clone());
    let approved = state
        .db
        .call(move |c| db::in_tx(c, |tx| db::approve_relay(tx, &id, account_id, &n, at)))
        .await
        .map_err(|e| e.to_string())?;
    if !approved {
        return Err(format!(
            "{out}nothing changed: the relay is no longer pending"
        ));
    }
    state.relays.changed();
    // the name on a line of its own: the first line goes to the operator record
    Ok(format!(
        "approved: the relay {relay_id} wakes the PCs of the account {account_tag}{}\nthe apps call it \"{name}\"\n{out}",
        if verdict.same_network() {
            ""
        } else {
            " (--force)"
        }
    ))
}

/// `relay revoke <relay ID|code>`: the relay takes no commands any more and keeps only its ID,
/// key and times; a code approved ahead is withdrawn.
pub async fn admin_revoke(state: &Arc<AppState>, arg: &str) -> Result<String, String> {
    let relay_id = parse_code(arg).ok_or("relay revoke takes a relay ID (r-…) or its code")?;
    let at = now();
    let id = relay_id.clone();
    let (revoked, withdrawn) = state
        .db
        .call(move |c| {
            let done = db::in_tx(c, |tx| {
                Ok((
                    db::revoke_relay(tx, &id, at)?,
                    db::delete_preapproval(tx, &id)?,
                ))
            })?;
            db::checkpoint(c)?;
            Ok(done)
        })
        .await
        .map_err(|e| e.to_string())?;
    state.relays.changed();
    Ok(match (revoked, withdrawn) {
        (Revoked::Now(failed), _) => format!(
            "revoked the relay {relay_id}: it takes no commands ({failed} waiting wake jobs failed)"
        ),
        (Revoked::Already, _) => format!("the relay {relay_id} was revoked already (no change)"),
        (Revoked::NoSuchRelay, true) => {
            format!(
                "withdrew the approval ahead of the code {}",
                code_of(&relay_id)
            )
        }
        (Revoked::NoSuchRelay, false) => "no such relay".to_owned(),
    })
}

/// `relay share on|off <relay ID>`: kept for later; W2 wakes the owner's PCs only.
pub async fn admin_share(state: &Arc<AppState>, arg: &str, on: bool) -> Result<String, String> {
    let relay_id = parse_code(arg).ok_or("relay share takes a relay ID (r-…) or its code")?;
    let id = relay_id.clone();
    let changed = state
        .db
        .call(move |c| db::in_tx(c, |tx| db::set_relay_scope(tx, &id, on)))
        .await
        .map_err(|e| e.to_string())?;
    if !changed {
        return Err(format!("no active relay {relay_id}"));
    }
    Ok(format!(
        "the relay {relay_id}: shared {} (kept for later: in this version a relay wakes only its account's PCs)",
        if on { "on" } else { "off" }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::hex;

    /// relay/PROTOCOL.md §13: the relay key of the seed 00 01 … 1f
    const PK: &str = "A6EHv/POEL4dcN0Y50vAmWfk1jCbpQ1fHdyGZBJVMbg=";

    #[test]
    fn the_identity_of_protocol_md() {
        let pk: [u8; 32] = crate::util::b64_decode(PK).unwrap().try_into().unwrap();
        // the key digest: the first 10 bytes of SHA-256(pk)
        assert_eq!(hex(&sha256(&pk)[..10]), "56475aa75463474c0285");
        let id = relay_id_of(&pk);
        assert_eq!(id, "r-kzdvvj2umnduyauf");
        assert_eq!(code_of(&id), "KZDV-VJ2U-MNDU-YAUF");
        assert_eq!(default_name(&id), "NAS KZDV");
        for code in [
            "KZDV-VJ2U-MNDU-YAUF",
            "kzdv-vj2u-mndu-yauf",
            " KZDV VJ2U MNDU YAUF ",
            "KZDVVJ2UMNDUYAUF",
            "r-kzdvvj2umnduyauf",
        ] {
            assert_eq!(
                parse_code(code).as_deref(),
                Some("r-kzdvvj2umnduyauf"),
                "{code}"
            );
        }
        for bad in [
            "KZDV-VJ2U-MNDU-YAU",
            "KZDV-VJ2U-MNDU-YAU1",
            "r-KZDVVJ2UMNDUYAUF0",
            "",
            "r-",
        ] {
            assert_eq!(parse_code(bad), None, "{bad}");
        }
        assert!(valid_relay_id("r-kzdvvj2umnduyauf"));
        for bad in [
            "r-kzdvvj2umnduyau",
            "r-kzdvvj2umnduyau1",
            "R-kzdvvj2umnduyauf",
            "x-kzdvvj2umnduyauf",
        ] {
            assert!(!valid_relay_id(bad), "{bad}");
        }
    }

    #[test]
    fn payloads() {
        // relay/PROTOCOL.md §13's hello payload
        let p = r#"{"v":1,"version":"1.6.0","arch":"x86_64","os":"DSM 7.2.1-69057 Update 5","kernel":"5.10.55+","ifaces_hash":"4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945","ifaces":[]}"#;
        let hello = parse_hello(p).unwrap();
        assert_eq!(
            (
                hello.version.as_str(),
                hello.os.as_str(),
                hello.ifaces.as_deref()
            ),
            ("1.6.0", "DSM 7.2.1-69057 Update 5", Some(&[][..]))
        );
        for bad in [
            r#"{"v":2,"version":"1","arch":"a","os":"o","kernel":"k","ifaces_hash":"h","ifaces":[]}"#,
            r#"{"v":1,"arch":"a","os":"o","kernel":"k","ifaces_hash":"h","ifaces":[]}"#,
            r#"{"v":1,"version":"1","arch":"a","os":"o","kernel":"k","ifaces_hash":"h"}"#,
            r#"{"v":1,"version":1,"arch":"a","os":"o","kernel":"k","ifaces_hash":"h","ifaces":[]}"#,
            r#"{"v":1,"version":"1","arch":"a","os":"o","kernel":"k","ifaces_hash":"h","ifaces":{}}"#,
            r#"{"v":1.0,"version":"1","arch":"a","os":"o","kernel":"k","ifaces_hash":"h","ifaces":[]}"#,
            "[]",
            "",
        ] {
            assert_eq!(parse_hello(bad), None, "{bad}");
        }
        let poll = |extra: &str| {
            parse_poll(&format!(
                r#"{{"v":1,"version":"1.6.0","arch":"x86_64","os":"o","kernel":"k","ifaces_hash":"h"{extra}}}"#
            ))
        };
        let cmd = "e".repeat(64);
        let p = poll(&format!(
            r#","wait":20,"results":[{{"cmd":"{cmd}","job":"x","sent_at":5,"iface":"eth0","packets":12,"error":null}},
                {{"cmd":null,"job":null,"sent_at":null,"iface":null,"packets":0,"error":"bad_command"}},
                {{"cmd":"short","packets":0,"error":null}},
                {{"cmd":"{cmd}","packets":0,"error":"Not A Code"}}]"#
        ))
        .unwrap();
        assert_eq!((p.wait, p.common.ifaces), (20, None));
        assert_eq!(
            p.results,
            vec![
                RelayResult {
                    cmd: Some(cmd.clone()),
                    sent_at: Some(5),
                    packets: 12,
                    error: None
                },
                RelayResult {
                    cmd: None,
                    sent_at: None,
                    packets: 0,
                    error: Some("bad_command".into())
                },
                RelayResult {
                    cmd: Some(cmd),
                    sent_at: None,
                    packets: 0,
                    error: Some("bad_result".into())
                },
            ]
        );
        // wait is held to 20; an absent or mistyped one, or results that are not a list, refuse
        assert_eq!(poll(r#","wait":900,"results":[]"#).unwrap().wait, 20);
        for bad in [
            r#","results":[]"#,
            r#","wait":-1,"results":[]"#,
            r#","wait":20"#,
            r#","wait":20,"results":{}"#,
        ] {
            assert_eq!(poll(bad), None, "{bad}");
        }
        assert!(code_form("no_iface") && !code_form("No") && !code_form(&"a".repeat(33)));
    }

    #[tokio::test]
    async fn per_relay_limits() {
        let hub = RelayHub::default();
        for _ in 0..HELLOS_PER_HOUR {
            assert!(hub.take_hello("r-a").is_ok());
        }
        let retry = hub.take_hello("r-a").unwrap_err();
        assert!((1..=3600).contains(&retry));
        assert!(hub.take_hello("r-b").is_ok());
        let first = hub.hold("r-a").unwrap();
        let second = hub.hold("r-a").unwrap();
        assert!(hub.hold("r-a").is_none() && hub.holding("r-a"));
        drop((first, second));
        assert!(!hub.holding("r-a") && hub.hold("r-a").is_some());
        hub.purge();
    }
}

// SPDX-License-Identifier: AGPL-3.0-only

//! Waking a PC (design v3 §8-4, §8-5): the app's paths, the command key and its signed commands
//! (relay/PROTOCOL.md §7), the choice of relay, the jobs and their confirmation.
//!
//! | path | answer |
//! |---|---|
//! | `POST /api/hjv/devices/{id}/wake` | body `{}` or `{"any_relay":true}` ("모든 중계기로 시도"): 202 `{"job","state","relay":{"name","match"},"hints":[…]}`, the new job queued, or the job of this PC made within the last minute and still under way |
//! | `GET /api/hjv/wake/{job}` | `{"job","state","sent_at","confirmed_at","packets","error","relay":{"name","match"},"hints":[…]}` |
//! | `GET /api/hjv/devices/{id}/wol` | `{"ready","hints","reported_at","relay":{"name","match","online"}}`, no MAC |
//!
//! Each takes the token with its key's `X-HJV-Proof` (api.rs `session`); a PC of another account,
//! or a job, is the same 404 as one that does not exist. The wake request, in this order: the PC
//! of the account; already on (hbbs saw it within [`ONLINE_AGE`] seconds, or it synced as long
//! ago): 409 `online`; no wake report: 409 `no_wol_info`; the PC's limit, one job a minute: within
//! it the job under way (queued or sent) is the answer, 202, and one that is over 429; the
//! account's limit (20 an hour): 429; the relay (below): 409 `no_relay` or 503 `relay_offline`;
//! the relay's limit (10 a minute): 429; then the command, signed, queued for the relay, whose
//! held poll gets it at once (relay.rs). A minute after a job that still waits for its PC, the
//! user may send the packets again: that is a new job, and each is confirmed on its own. Errors
//! carry the text the account API's rules ask for and the code in `hjv`:
//!
//! ```text
//! 409 {"error":"이미 켜져 있습니다.","hjv":"online"}
//! 409 {"error":"…","hjv":"no_wol_info"}
//! 409 {"error":"…","hjv":"no_relay","reason":"no_relays|other_network|different_egress|unconfirmed","any_relay":false}
//! 503 {"error":"…","hjv":"relay_offline","last_seen":<time or null>}
//! 429 {"error":"…","hjv":"rate_limited","retry_after":<seconds>}
//! 503 {"error":"…","hjv":"unavailable"}   (no command key: hjv-api queues no command)
//! ```
//!
//! **The relay** (design v3 §8-4): of the account's active relays (W2: a relay wakes its
//! account's PCs only), the online ones (relay.rs [`crate::relay::online`]) with the best grade of
//! wolinfo.rs's [`wolinfo::judge`]: `exact`, `overlap`, `history`. None online that has a grade,
//! but one that is not online: 503 `relay_offline`, with when the best of them last polled. No
//! grade at all: 409 `no_relay` with its reason, and `any_relay` true when trying all relays may
//! help: an online relay shares a network with the PC but hjv-api could not confirm it (another
//! gateway, or no egress address to compare). With `{"any_relay":true}` such a relay is used,
//! grade `any`; a private network whose egress addresses differ is another site, so the app hides
//! the choice then (reason `different_egress`), though the request still takes it.
//!
//! **The command** (PROTOCOL.md §7.1): `{"v":1,"job","relay","iat","exp":iat+60,"net","macs",
//! "ports":[9,7],"repeat":3,"gap_ms":200}`, compact JSON in this order, `net` the PC's network
//! that matched, `macs` the PC's cards on it, from its signed report only. Its signature is the
//! command key's over `"hjv-wake-v1" NUL` and the text; the text, the signature and the key's
//! number are kept and sent as they are until a result names the command or it expires.
//!
//! **States**: `queued` → `sent` (the relay's result: its packets went out) → `confirmed` (hbbs
//! saw the PC register within [`ONLINE_AGE`] seconds under its key, after the packets, or the PC
//! synced after them) or `not_confirmed` ([`CONFIRM_FOR`] after the packets without that);
//! `queued` → `failed` with the relay's `error` (`no_iface`, `send_failed`, `expired`,
//! `not_active`, `rate_limited`, or a refusal that says the relay and hjv-api disagree, such as
//! `unknown_key`) or the server's `no_result` ([`RESULT_GRACE`] after the command expired
//! without a result). [`tick`], every five seconds (jobs.rs), moves them along. The jobs are the
//! wake record: the account's tag, the PC, the relay, the grade, the times and the result, kept
//! six months (db.rs `purge`).

use crate::{
    api::{session, ApiRequest, Reply},
    db::{self, Job, RelayRow, WolInfo},
    hbbs::{self, Lookup},
    relay,
    state::AppState,
    util::{b64, hex, now, random_bytes, sha256, valid_device_id},
    wolinfo::{self, Grade, Net4, Ready},
};
use ed25519_dalek::{Signer, SigningKey};
use hyper::{Method, StatusCode};
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::Arc;

/// The signature's context (PROTOCOL.md §7.2).
pub const CONTEXT: &[u8] = b"hjv-wake-v1";
/// A command's lifetime, `exp − iat` (PROTOCOL.md §7.1: the server uses 60 s).
pub const COMMAND_LIFETIME: i64 = 60;
/// How long after its command expired a job waits for a late result before `no_result` (the
/// relay gives up on a request 35 seconds after it began).
pub const RESULT_GRACE: i64 = 45;
/// How long after the packets a job waits for the PC (design v3 §8-4).
pub const CONFIRM_FOR: i64 = 300;
/// A PC is on when hbbs saw it register this recently (it registers every few seconds).
pub const ONLINE_AGE: u64 = 30;
/// The limits (design v3 §5-5): one job a minute per PC, 20 an hour per account, 10 a minute per
/// relay.
pub const PC_EVERY: i64 = 60;
pub const ACCOUNT_PER_HOUR: i64 = 20;
pub const RELAY_PER_MINUTE: i64 = 10;
/// The relay's errors the apps have texts for (design v3 §8-5) and `partial` without packets;
/// other refusals mean that the relay and hjv-api disagree, which the journal hears of.
pub const EXPECTED_ERRORS: &[&str] = &[
    "no_iface",
    "send_failed",
    "expired",
    "not_active",
    "rate_limited",
    "partial",
];

const TEXT_ONLINE: &str = "이미 켜져 있습니다.";
const TEXT_NO_WOL_INFO: &str =
    "이 PC가 아직 켜기 정보를 보내지 않았습니다. PC를 한 번 켜고 HJViewer가 설치되어 있는지 확인하세요.";
const TEXT_NO_RELAY: &str = "이 PC와 같은 네트워크에 켜져 있는 중계기가 없습니다.";
const TEXT_RELAY_OFFLINE: &str = "중계기(NAS)가 연결되어 있지 않습니다.";
const TEXT_TOO_OFTEN: &str = "잠시 뒤 다시 시도해 주세요.";
const TEXT_UNAVAILABLE: &str = "지금은 켜기 요청을 보낼 수 없습니다. 잠시 뒤 다시 시도해 주세요.";
const TEXT_BAD_BODY: &str = "요청을 이해하지 못했습니다.";

// ---- the command key and the commands ----

/// The command key in use, with its number (credential hjviewer-api-command-key,
/// HJV_API_COMMAND_KID).
pub struct CommandKey {
    kid: u32,
    key: SigningKey,
}

impl std::fmt::Debug for CommandKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "CommandKey({})",
            crate::trust::entry(self.kid, &self.public_key())
        )
    }
}

impl CommandKey {
    pub fn new(kid: u32, seed: &[u8; 32]) -> CommandKey {
        CommandKey {
            kid,
            key: SigningKey::from_bytes(seed),
        }
    }

    pub fn kid(&self) -> u32 {
        self.kid
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.key.verifying_key().to_bytes()
    }

    /// The signature of a command text (PROTOCOL.md §7.2).
    pub fn sign(&self, text: &str) -> [u8; 64] {
        self.key.sign(&message(text)).to_bytes()
    }
}

/// The bytes a command key signs.
pub fn message(text: &str) -> Vec<u8> {
    [CONTEXT, &[0u8], text.as_bytes()].concat()
}

/// The command object, its fields in PROTOCOL.md §7.1's order.
#[derive(Serialize)]
struct Command<'a> {
    v: u32,
    job: &'a str,
    relay: &'a str,
    iat: i64,
    exp: i64,
    net: &'a str,
    macs: &'a [String],
    ports: [u16; 2],
    repeat: u32,
    gap_ms: u32,
}

/// The text of a command for `relay` to wake the cards `macs` on `net` (canonical), issued at
/// `iat`: compact JSON, serialized once (PROTOCOL.md §7.2).
pub fn command_text(job: &str, relay: &str, iat: i64, net: &str, macs: &[String]) -> String {
    serde_json::to_string(&Command {
        v: 1,
        job,
        relay,
        iat,
        exp: iat + COMMAND_LIFETIME,
        net,
        macs,
        ports: [9, 7],
        repeat: 3,
        gap_ms: 200,
    })
    .unwrap_or_default()
}

/// `cmd`, the name results give a command: hex SHA-256 of its text.
pub fn command_hash(text: &str) -> String {
    hex(&sha256(text.as_bytes()))
}

// ---- choosing the relay ----

/// A relay that can take the job.
#[derive(Debug, Clone)]
pub struct Chosen {
    pub relay: RelayRow,
    pub grade: Grade,
    pub net: Net4,
    pub macs: Vec<String>,
}

/// What [`choose`] found.
#[derive(Debug, Clone)]
pub enum Choice {
    Relay(Box<Chosen>),
    /// 409 `no_relay`
    NoRelay {
        reason: &'static str,
        any_relay: bool,
    },
    /// 503 `relay_offline`: the relay that would do, or else any of the account's, last polled
    Offline {
        last_seen: Option<i64>,
    },
}

/// A relay as [`choose`] weighs it: its row, whether it is online, the networks on which it
/// woke this PC before.
pub struct Candidate {
    pub relay: RelayRow,
    pub online: bool,
    pub woken_on: Vec<String>,
}

/// The relay for a wake of the PC with `info` (design v3 §8-4); `any_relay`: the user asked to
/// try all relays.
pub fn choose(candidates: Vec<Candidate>, info: &WolInfo, any_relay: bool) -> Choice {
    if candidates.is_empty() {
        return Choice::NoRelay {
            reason: "no_relays",
            any_relay: false,
        };
    }
    let judged: Vec<(Candidate, wolinfo::Verdict)> = candidates
        .into_iter()
        .map(|c| {
            let v = wolinfo::judge(
                &c.relay.ifaces,
                c.relay.egress_ip.as_deref(),
                &info.report,
                Some(&info.egress_ip),
                &c.woken_on,
            );
            (c, v)
        })
        .collect();
    let latest = |c: &Candidate| c.relay.last_poll_at.unwrap_or(0);
    let chosen = |c: &Candidate, grade: Grade, net: Net4| {
        let macs = info.report.macs_on(&net);
        (!macs.is_empty()).then(|| {
            Choice::Relay(Box::new(Chosen {
                relay: c.relay.clone(),
                grade,
                net,
                macs,
            }))
        })
    };
    // the best grade online, the latest poll among equals
    let best = judged
        .iter()
        .filter(|(c, v)| c.online && v.grade.is_some())
        .min_by_key(|(c, v)| (v.grade, std::cmp::Reverse(latest(c))));
    if let Some((c, v)) = best {
        if let (Some(grade), Some(net)) = (v.grade, v.net) {
            if let Some(choice) = chosen(c, grade, net) {
                return choice;
            }
        }
    }
    let graded_offline = judged
        .iter()
        .filter(|(c, v)| !c.online && v.grade.is_some())
        .min_by_key(|(c, v)| (v.grade, std::cmp::Reverse(latest(c))));
    if let Some((c, _)) = graded_offline {
        return Choice::Offline {
            last_seen: c.relay.last_poll_at,
        };
    }
    let online: Vec<&(Candidate, wolinfo::Verdict)> =
        judged.iter().filter(|(c, _)| c.online).collect();
    if online.is_empty() {
        return Choice::Offline {
            last_seen: judged
                .iter()
                .filter_map(|(c, _)| c.relay.last_poll_at)
                .max(),
        };
    }
    if any_relay {
        let overlapping = online
            .iter()
            .filter(|(_, v)| v.overlap)
            .max_by_key(|(c, _)| latest(c));
        if let Some((c, v)) = overlapping {
            if let Some(choice) = v.net.and_then(|net| chosen(c, Grade::Any, net)) {
                return choice;
            }
        }
    }
    let unconfirmed = online.iter().any(|(_, v)| v.overlap && !v.elsewhere());
    let elsewhere = online.iter().any(|(_, v)| v.elsewhere());
    let reason = if unconfirmed {
        "unconfirmed"
    } else if elsewhere {
        "different_egress"
    } else {
        "other_network"
    };
    Choice::NoRelay {
        reason,
        any_relay: unconfirmed,
    }
}

// ---- the paths ----

/// The methods of the paths this module answers, for 405 answers; `None` for others.
pub fn allowed_methods(path: &str) -> Option<&'static str> {
    if let Some(rest) = path.strip_prefix("/api/hjv/wake/") {
        return (!rest.contains('/')).then_some("GET");
    }
    let rest = path.strip_prefix("/api/hjv/devices/")?;
    match rest.split_once('/') {
        Some((_, "wake")) => Some("POST"),
        Some((_, "wol")) => Some("GET"),
        _ => None,
    }
}

/// Dispatches the paths of [`allowed_methods`] (the caller checked the method).
pub async fn route(state: &Arc<AppState>, req: &ApiRequest) -> Reply {
    if let Some(job) = req.path.strip_prefix("/api/hjv/wake/") {
        return job_status(state, req, job).await;
    }
    let rest = req.path.strip_prefix("/api/hjv/devices/").unwrap_or("");
    match (rest.split_once('/'), &req.method) {
        (Some((id, "wake")), &Method::POST) => wake(state, req, id).await,
        (Some((id, "wol")), &Method::GET) => wol(state, req, id).await,
        _ => Reply::not_found(),
    }
}

fn error(status: StatusCode, text: &str, code: &str, extra: Value) -> Reply {
    let mut body = json!({ "error": text, "hjv": code });
    if let (Some(body), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
        body.extend(extra.clone());
    }
    Reply::json(status, &body)
}

fn too_often(retry_after: i64) -> Reply {
    error(
        StatusCode::TOO_MANY_REQUESTS,
        TEXT_TOO_OFTEN,
        "rate_limited",
        json!({ "retry_after": retry_after.clamp(1, 3600) }),
    )
}

/// What the body of a wake request asks: `{}` (or nothing) or `{"any_relay":true}`; `None` when
/// it is not that.
fn any_relay(req: &ApiRequest) -> Option<bool> {
    let body = req.body.as_ref().ok()?;
    if body.is_empty() {
        return Some(false);
    }
    let value: Value = serde_json::from_slice(body).ok()?;
    let object = value.as_object()?;
    match object.get("any_relay") {
        None => Some(false),
        Some(v) => v.as_bool(),
    }
}

/// The name of a job's relay as the apps show it.
fn relay_name(relay: Option<&RelayRow>, relay_id: &str) -> String {
    relay
        .map(|r| r.name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| relay::default_name(relay_id))
}

fn job_json(job: &Job, relay: Option<&RelayRow>, info: Option<&WolInfo>) -> Value {
    let (_, hints) = wolinfo::readiness(info.map(|i| &i.report));
    json!({
        "job": job.job,
        "state": job.state,
        "sent_at": job.sent_at,
        "confirmed_at": job.confirmed_at,
        "packets": job.packets,
        "error": if job.state == "failed" { job.error.clone() } else { None },
        "relay": { "name": relay_name(relay, &job.relay_id), "match": job.grade },
        "hints": wolinfo::hint_texts(&hints),
    })
}

/// Whether the PC is on now: it synced within ONLINE_AGE seconds, or hbbs saw it register as
/// recently (any key: the PC is on, whatever it holds).
async fn pc_online(state: &AppState, device: &db::Device, at: i64) -> bool {
    if device
        .last_sync_at
        .is_some_and(|t| t >= at - ONLINE_AGE as i64)
    {
        return true;
    }
    let Some(server_key) = state.config.hbbs_pubkey else {
        return false;
    };
    matches!(
        hbbs::lookup(state.config.hbbs_console, Some(&server_key), &device.device_id).await,
        Ok(Lookup::Known { seen_secs_ago: Some(age), .. }) if age <= ONLINE_AGE
    )
}

/// What a wake request reads first.
struct Found {
    device: db::Device,
    info: Option<WolInfo>,
    latest: Option<Job>,
    latest_relay: Option<RelayRow>,
    account_jobs: i64,
    first_of_hour: Option<i64>,
    candidates: Vec<(RelayRow, Vec<String>)>,
}

async fn wake(state: &Arc<AppState>, req: &ApiRequest, device_id: &str) -> Reply {
    let s = match session(state, req).await {
        Ok(s) => s,
        Err(reply) => return reply,
    };
    let Some(any_relay) = any_relay(req) else {
        return Reply::error(StatusCode::BAD_REQUEST, TEXT_BAD_BODY);
    };
    if !valid_device_id(device_id) {
        return Reply::not_found();
    }
    let (account_id, id, at) = (s.account.id, device_id.to_owned(), now());
    let found = state
        .db
        .call(move |c| {
            let Some(device) = db::device(c, &id)?.filter(|d| d.account_id == account_id) else {
                return Ok(None);
            };
            let latest = db::latest_job_of(c, &id, account_id)?;
            let latest_relay = match &latest {
                Some(job) => db::relay(c, &job.relay_id)?,
                None => None,
            };
            let mut candidates = Vec::new();
            for relay in db::active_relays_of(c, account_id)? {
                let woken = db::woken_on(c, &relay.relay_id, &id)?;
                candidates.push((relay, woken));
            }
            let (account_jobs, first_of_hour) = c.query_row(
                "SELECT COUNT(*), MIN(created_at) FROM wake_jobs WHERE account_id = ?1 AND created_at > ?2",
                rusqlite::params![account_id, at - 3600],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            Ok(Some(Found {
                info: db::wol_info(c, &id)?,
                device,
                latest,
                latest_relay,
                account_jobs,
                first_of_hour,
                candidates,
            }))
        })
        .await;
    let found = match found {
        Ok(Some(found)) => found,
        Ok(None) => return Reply::not_found(),
        Err(e) => {
            log_err!("wake: {e}");
            return Reply::server_error();
        }
    };
    let account_tag = state.tags.account(&s.account.login_key);
    let device_tag = state.tags.device(&found.device.device_id);
    // on already
    if pc_online(state, &found.device, at).await {
        return error(StatusCode::CONFLICT, TEXT_ONLINE, "online", json!({}));
    }
    // no report of how to wake it
    let Some(info) = found.info else {
        return error(
            StatusCode::CONFLICT,
            TEXT_NO_WOL_INFO,
            "no_wol_info",
            json!({}),
        );
    };
    // the PC's limit, one job a minute (design v3 §5-5): within it, the job under way is the
    // answer (a second tap, a request sent again), and a job that is over means waiting. After
    // it, a job still waiting for the PC does not keep the user from sending the packets again.
    if let Some(job) = found
        .latest
        .as_ref()
        .filter(|j| j.created_at > at - PC_EVERY)
    {
        if job.state == "queued" || job.state == "sent" {
            return Reply::json(
                StatusCode::ACCEPTED,
                &job_json(job, found.latest_relay.as_ref(), Some(&info)),
            );
        }
        return too_often(job.created_at + PC_EVERY - at);
    }
    // the account's limit
    if found.account_jobs >= ACCOUNT_PER_HOUR {
        if state.first_notice(
            &format!("wake-hour:{account_tag}"),
            std::time::Duration::from_secs(600),
        ) {
            log_notice!("wake: {ACCOUNT_PER_HOUR} wake requests in an hour (acct={account_tag}); the next ten minutes are not logged");
        }
        return too_often(found.first_of_hour.map_or(3600, |f| f + 3600 - at));
    }
    // the relay
    let candidates = found
        .candidates
        .into_iter()
        .map(|(relay, woken_on)| Candidate {
            online: relay::online(state, &relay, at),
            relay,
            woken_on,
        })
        .collect();
    let chosen = match choose(candidates, &info, any_relay) {
        Choice::Relay(chosen) => *chosen,
        Choice::NoRelay { reason, any_relay } => {
            log_info!("wake: no relay, {reason} (acct={account_tag}, dev={device_tag})");
            return error(
                StatusCode::CONFLICT,
                TEXT_NO_RELAY,
                "no_relay",
                json!({ "reason": reason, "any_relay": any_relay }),
            );
        }
        Choice::Offline { last_seen } => {
            log_info!("wake: the relay is offline (acct={account_tag}, dev={device_tag})");
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                TEXT_RELAY_OFFLINE,
                "relay_offline",
                json!({ "last_seen": last_seen }),
            );
        }
    };
    let Some(key) = state.command_key.as_ref() else {
        if state.first_notice("wake-no-key", std::time::Duration::from_secs(600)) {
            log_err!(
                "wake: no command key (credential hjviewer-api-command-key): no PC can be woken"
            );
        }
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            TEXT_UNAVAILABLE,
            "unavailable",
            json!({}),
        );
    };
    // the command, signed once and kept as it is
    let job_id = hex(&random_bytes::<16>());
    let net = chosen.net.canonical();
    let text = command_text(&job_id, &chosen.relay.relay_id, at, &net, &chosen.macs);
    let sig = b64(&key.sign(&text));
    let cmd = command_hash(&text);
    let job = Job {
        job: job_id,
        account_id,
        account_tag: account_tag.clone(),
        device_id: found.device.device_id.clone(),
        relay_id: chosen.relay.relay_id.clone(),
        grade: chosen.grade.as_str().to_owned(),
        net,
        cmd: cmd.clone(),
        created_at: at,
        exp: at + COMMAND_LIFETIME,
        state: "queued".to_owned(),
        sent_at: None,
        packets: None,
        confirmed_at: None,
        error: None,
        finished_at: None,
    };
    let kid = key.kid();
    let queued = state
        .db
        .call({
            let job = job.clone();
            move |c| {
                db::in_tx(c, |tx| {
                    // another request of this PC may have queued one meanwhile
                    if let Some(other) = db::latest_job_of(tx, &job.device_id, account_id)?
                        .filter(|j| j.created_at > at - PC_EVERY)
                    {
                        return Ok(Err(other));
                    }
                    if db::jobs_of_relay_since(tx, &job.relay_id, at - 60)? >= RELAY_PER_MINUTE {
                        return Ok(Ok(false));
                    }
                    db::insert_job(
                        tx,
                        &job,
                        &db::NewCommand {
                            cmd: &cmd,
                            text: &text,
                            sig: &sig,
                            kid,
                            iat: at,
                        },
                    )?;
                    Ok(Ok(true))
                })
            }
        })
        .await;
    match queued {
        Ok(Ok(true)) => {
            state.relays.changed();
            log_info!(
                "wake: queued, {} (acct={account_tag}, dev={device_tag}, relay {})",
                job.grade,
                relay::short(&job.relay_id, 10)
            );
            let (_, hints) = wolinfo::readiness(Some(&info.report));
            Reply::json(
                StatusCode::ACCEPTED,
                &json!({
                    "job": job.job,
                    "state": "queued",
                    "relay": { "name": relay_name(Some(&chosen.relay), &chosen.relay.relay_id), "match": job.grade },
                    "hints": wolinfo::hint_texts(&hints),
                }),
            )
        }
        Ok(Ok(false)) => too_often(60),
        Ok(Err(other)) if other.state == "queued" || other.state == "sent" => {
            let relay = (other.relay_id == chosen.relay.relay_id).then_some(&chosen.relay);
            Reply::json(StatusCode::ACCEPTED, &job_json(&other, relay, Some(&info)))
        }
        Ok(Err(other)) => too_often(other.created_at + PC_EVERY - at),
        Err(e) => {
            log_err!("wake: {e}");
            Reply::server_error()
        }
    }
}

async fn job_status(state: &Arc<AppState>, req: &ApiRequest, job_id: &str) -> Reply {
    let s = match session(state, req).await {
        Ok(s) => s,
        Err(reply) => return reply,
    };
    let well_formed = job_id.len() == 32
        && job_id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if !well_formed {
        return Reply::not_found();
    }
    let (account_id, job_id) = (s.account.id, job_id.to_owned());
    let found = state
        .db
        .call(move |c| {
            let Some(job) = db::job(c, &job_id)?.filter(|j| j.account_id == account_id) else {
                return Ok(None);
            };
            let relay = db::relay(c, &job.relay_id)?;
            let info = db::wol_info(c, &job.device_id)?;
            Ok(Some((job, relay, info)))
        })
        .await;
    match found {
        Ok(Some((job, relay, info))) => Reply::ok(&job_json(&job, relay.as_ref(), info.as_ref())),
        Ok(None) => Reply::not_found(),
        Err(e) => {
            log_err!("wake: {e}");
            Reply::server_error()
        }
    }
}

/// The verdict of a PC for the apps' device list (`hjv.wol`, design v3 §5-2) and the note's tail:
/// the report's verdict, but `no` while the account has no active relay, since nothing could
/// wake it; " · 켜기 가능" or " · 켜기 준비 안 됨" once it has one and the PC reported.
pub fn peer_wol(info: Option<&WolInfo>, has_relay: bool) -> (&'static str, &'static str) {
    let (ready, _) = wolinfo::readiness(info.map(|i| &i.report));
    if !has_relay {
        return (Ready::No.as_str(), "");
    }
    let tail = match ready {
        Ready::Ok | Ready::Warn => " · 켜기 가능",
        Ready::No => " · 켜기 준비 안 됨",
        Ready::Unknown => "",
    };
    (ready.as_str(), tail)
}

async fn wol(state: &Arc<AppState>, req: &ApiRequest, device_id: &str) -> Reply {
    let s = match session(state, req).await {
        Ok(s) => s,
        Err(reply) => return reply,
    };
    if !valid_device_id(device_id) {
        return Reply::not_found();
    }
    let (account_id, id) = (s.account.id, device_id.to_owned());
    let found = state
        .db
        .call(move |c| {
            if db::device(c, &id)?
                .filter(|d| d.account_id == account_id)
                .is_none()
            {
                return Ok(None);
            }
            let mut candidates = Vec::new();
            for relay in db::active_relays_of(c, account_id)? {
                let woken = db::woken_on(c, &relay.relay_id, &id)?;
                candidates.push((relay, woken));
            }
            Ok(Some((db::wol_info(c, &id)?, candidates)))
        })
        .await;
    let (info, candidates) = match found {
        Ok(Some(found)) => found,
        Ok(None) => return Reply::not_found(),
        Err(e) => {
            log_err!("wol: {e}");
            return Reply::server_error();
        }
    };
    let at = now();
    let (ready, hints) = wolinfo::readiness(info.as_ref().map(|i| &i.report));
    // the relay that would take a wake now, or the best that would once online
    let relay = info.as_ref().and_then(|info| {
        candidates
            .into_iter()
            .filter_map(|(relay, woken_on)| {
                let v = wolinfo::judge(
                    &relay.ifaces,
                    relay.egress_ip.as_deref(),
                    &info.report,
                    Some(&info.egress_ip),
                    &woken_on,
                );
                let online = relay::online(state, &relay, at);
                v.grade.map(|g| (std::cmp::Reverse(online), g, relay))
            })
            .min_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)))
            .map(|(online, grade, relay)| {
                json!({
                    "name": relay_name(Some(&relay), &relay.relay_id),
                    "match": grade.as_str(),
                    "online": online.0,
                })
            })
    });
    Reply::ok(&json!({
        "ready": ready.as_str(),
        "hints": wolinfo::hint_texts(&hints),
        "reported_at": info.as_ref().map(|i| i.reported_at),
        "relay": relay,
    }))
}

// ---- moving the jobs along ----

/// What one [`tick`] did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Ticked {
    pub confirmed: usize,
    pub not_confirmed: usize,
    pub no_result: usize,
}

/// Moves the open jobs along (every five seconds, jobs.rs): a command that expired
/// RESULT_GRACE ago without a result fails with `no_result`; a sent job is `confirmed` when the PC
/// synced after its packets, or hbbs saw it register within ONLINE_AGE seconds under its key and
/// after them, and `not_confirmed` CONFIRM_FOR after them.
pub async fn tick(state: &Arc<AppState>) -> Ticked {
    let mut done = Ticked::default();
    let at = now();
    let open = state
        .db
        .call(|c| {
            let mut open = Vec::new();
            for job in db::open_jobs(c)? {
                let device = db::device(c, &job.device_id)?;
                open.push((job, device));
            }
            Ok(open)
        })
        .await;
    let open = match open {
        Ok(open) => open,
        Err(e) => {
            log_err!("wake: {e}");
            return done;
        }
    };
    for (job, device) in open {
        let id = job.job.clone();
        if job.state == "queued" {
            if at > job.exp + RESULT_GRACE {
                let failed = state
                    .db
                    .call(move |c| {
                        db::in_tx(c, |tx| {
                            let failed = db::job_failed(tx, &id, "no_result", at)?;
                            db::delete_commands_of_job(tx, &id)?;
                            Ok(failed)
                        })
                    })
                    .await;
                if matches!(failed, Ok(true)) {
                    done.no_result += 1;
                    log_info!(
                        "wake: failed, no_result (job {})",
                        relay::short(&job.job, 8)
                    );
                }
            }
            continue;
        }
        let sent_at = job.sent_at.unwrap_or(job.created_at);
        let mut confirmed_at = device
            .as_ref()
            .and_then(|d| d.last_sync_at)
            .filter(|t| *t > sent_at);
        if confirmed_at.is_none() {
            if let Some(server_key) = state.config.hbbs_pubkey {
                let lookup =
                    hbbs::lookup(state.config.hbbs_console, Some(&server_key), &job.device_id)
                        .await;
                if let Ok(Lookup::Known {
                    public_key,
                    seen_secs_ago: Some(age),
                }) = lookup
                {
                    let same_key = device
                        .as_ref()
                        .is_none_or(|d| d.public_key.as_slice() == public_key.as_slice());
                    let registered = at - i64::try_from(age).unwrap_or(i64::MAX).min(at);
                    if same_key && age <= ONLINE_AGE && registered > sent_at {
                        confirmed_at = Some(registered);
                    }
                }
            }
        }
        let outcome = match confirmed_at {
            Some(t) => {
                let id = id.clone();
                state
                    .db
                    .call(move |c| db::in_tx(c, |tx| db::job_confirmed(tx, &id, t, at)))
                    .await
                    .map(|done| done.then_some(true))
            }
            None if at > sent_at + CONFIRM_FOR => state
                .db
                .call(move |c| db::in_tx(c, |tx| db::job_not_confirmed(tx, &id, at)))
                .await
                .map(|done| done.then_some(false)),
            None => Ok(None),
        };
        match outcome {
            Ok(Some(true)) => {
                done.confirmed += 1;
                log_info!("wake: confirmed (job {})", relay::short(&job.job, 8));
            }
            Ok(Some(false)) => {
                done.not_confirmed += 1;
                log_info!(
                    "wake: not confirmed in five minutes (job {})",
                    relay::short(&job.job, 8)
                );
            }
            Ok(None) => {}
            Err(e) => log_err!("wake: {e}"),
        }
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        db::RelayState,
        wolinfo::{Adapter, RelayIface, Report},
    };

    /// relay/PROTOCOL.md §13: the command key of the seed 20 21 … 3f and its command
    #[test]
    fn the_command_of_protocol_md() {
        let seed: [u8; 32] = std::array::from_fn(|i| (i + 32) as u8);
        let key = CommandKey::new(1, &seed);
        assert_eq!(
            crate::trust::entry(1, &key.public_key()),
            "1:Kay64UG8yvCyLhqU000LxzYeUm0L/hLIl5S8kyKWbdc="
        );
        assert_eq!(
            crate::trust::fingerprint(1, &key.public_key()),
            "1:24f6ed6acbfe1009c030d7ca567c33ca4830911498236b5561a6c82abec5de28"
        );
        let text = command_text(
            "0123456789abcdef0123456789abcdef",
            "r-kzdvvj2umnduyauf",
            1_760_000_000,
            "10.1.4.0/22",
            &["00:00:5e:00:53:01".to_owned()],
        );
        assert_eq!(
            text,
            r#"{"v":1,"job":"0123456789abcdef0123456789abcdef","relay":"r-kzdvvj2umnduyauf","iat":1760000000,"exp":1760000060,"net":"10.1.4.0/22","macs":["00:00:5e:00:53:01"],"ports":[9,7],"repeat":3,"gap_ms":200}"#
        );
        assert_eq!(
            b64(&key.sign(&text)),
            "91MXS5rMOB9k3ZP7XxPue9M++nXy9w4VIpjcqGJspnFI46U37TGC+st2hv6D8DxcUJOaN/2PcY7XeQ6wgfmLBw=="
        );
        assert_eq!(
            command_hash(&text),
            "eefb709bfc6dae46233f75d43c02d0dfb173218d3ed9010c40dc316b4c3d7abe"
        );
        // the entry of a poll answer, as the relay reads it
        let entry = json!({ "c": text, "s": b64(&key.sign(&text)), "k": 1 });
        assert_eq!(
            entry.to_string(),
            r#"{"c":"{\"v\":1,\"job\":\"0123456789abcdef0123456789abcdef\",\"relay\":\"r-kzdvvj2umnduyauf\",\"iat\":1760000000,\"exp\":1760000060,\"net\":\"10.1.4.0/22\",\"macs\":[\"00:00:5e:00:53:01\"],\"ports\":[9,7],\"repeat\":3,\"gap_ms\":200}","k":1,"s":"91MXS5rMOB9k3ZP7XxPue9M++nXy9w4VIpjcqGJspnFI46U37TGC+st2hv6D8DxcUJOaN/2PcY7XeQ6wgfmLBw=="}"#
        );
        assert!(!format!("{key:?}").contains(&b64(&seed)));
    }

    #[test]
    fn paths() {
        assert_eq!(
            allowed_methods("/api/hjv/devices/123456789/wake"),
            Some("POST")
        );
        assert_eq!(
            allowed_methods("/api/hjv/devices/123456789/wol"),
            Some("GET")
        );
        assert_eq!(
            allowed_methods(&format!("/api/hjv/wake/{}", "a".repeat(32))),
            Some("GET")
        );
        assert_eq!(allowed_methods("/api/hjv/devices/123456789"), None);
        assert_eq!(allowed_methods("/api/hjv/devices/123456789/other"), None);
        assert_eq!(allowed_methods("/api/hjv/wake/a/b"), None);
        for text in [
            TEXT_ONLINE,
            TEXT_NO_WOL_INFO,
            TEXT_NO_RELAY,
            TEXT_RELAY_OFFLINE,
            TEXT_TOO_OFTEN,
            TEXT_UNAVAILABLE,
            TEXT_BAD_BODY,
        ] {
            assert!(!text.to_lowercase().contains("disabled"));
        }
    }

    fn info(addr: &str, gateway: &str, egress: &str) -> WolInfo {
        WolInfo {
            device_id: "123456789".into(),
            reported_at: 0,
            report: Report {
                v: 1,
                adapters: vec![Adapter {
                    mac: "00:00:5e:00:53:10".into(),
                    ipv4: vec![addr.into()],
                    gateway: Some(gateway.into()),
                    wake: crate::wolinfo::Wake {
                        magic: Some("1".into()),
                        ..Default::default()
                    },
                    ..Adapter::default()
                }],
                power: Default::default(),
            },
            info_hash: String::new(),
            nets: String::new(),
            egress_ip: egress.into(),
        }
    }

    fn candidate(
        id: &str,
        addr: &str,
        gateway: Option<&str>,
        egress: &str,
        online: bool,
        last: i64,
    ) -> Candidate {
        Candidate {
            relay: RelayRow {
                relay_id: id.into(),
                pk: [0; 32],
                state: RelayState::Active,
                owner_account: Some(1),
                scope: "own".into(),
                name: String::new(),
                created_at: 0,
                source: None,
                approved_at: Some(0),
                revoked_at: None,
                last_hello_at: None,
                last_poll_at: Some(last),
                version: None,
                arch: None,
                os: None,
                kernel: None,
                ifaces: vec![RelayIface {
                    name: "eth0".into(),
                    ipv4: vec![addr.into()],
                    gateway: gateway.map(Into::into),
                    ..RelayIface::default()
                }],
                ifaces_hash: None,
                egress_ip: Some(egress.into()),
            },
            online,
            woken_on: Vec::new(),
        }
    }

    fn picked(choice: &Choice) -> (String, &'static str) {
        match choice {
            Choice::Relay(c) => (c.relay.relay_id.clone(), c.grade.as_str()),
            Choice::NoRelay { reason, any_relay } => (format!("no_relay {any_relay}"), reason),
            Choice::Offline { last_seen } => (format!("offline {last_seen:?}"), ""),
        }
    }

    #[test]
    fn the_relay_is_chosen() {
        let pc = info("192.0.2.67/24", "192.0.2.1", "198.51.100.7");
        // none at all
        assert_eq!(
            picked(&choose(vec![], &pc, false)),
            ("no_relay false".into(), "no_relays")
        );
        // the exact one online before an overlapping one, the latest poll among equals
        let exact = candidate(
            "r-exact",
            "192.0.2.10/24",
            Some("192.0.2.1"),
            "203.0.113.1",
            true,
            10,
        );
        let overlap = candidate(
            "r-overlap",
            "192.0.2.10/23",
            Some("192.0.2.1"),
            "203.0.113.1",
            true,
            20,
        );
        let choice = choose(vec![overlap, exact], &pc, false);
        assert_eq!(picked(&choice), ("r-exact".into(), "exact"));
        let Choice::Relay(c) = choice else {
            unreachable!()
        };
        assert_eq!(
            (c.net.canonical(), c.macs),
            (
                "192.0.2.0/24".to_owned(),
                vec!["00:00:5e:00:53:10".to_owned()]
            )
        );
        let older = candidate(
            "r-older",
            "192.0.2.11/24",
            Some("192.0.2.1"),
            "203.0.113.1",
            true,
            5,
        );
        let newer = candidate(
            "r-newer",
            "192.0.2.12/24",
            Some("192.0.2.1"),
            "203.0.113.1",
            true,
            50,
        );
        assert_eq!(picked(&choose(vec![older, newer], &pc, false)).0, "r-newer");
        // the only one that would do is offline
        let off = candidate(
            "r-off",
            "192.0.2.10/24",
            Some("192.0.2.1"),
            "203.0.113.1",
            false,
            42,
        );
        let other = candidate(
            "r-other",
            "198.51.100.10/24",
            Some("198.51.100.1"),
            "203.0.113.1",
            true,
            50,
        );
        assert_eq!(
            picked(&choose(vec![off, other], &pc, false)).0,
            "offline Some(42)"
        );
        // none would do, and none is online: offline, the latest poll of all
        let a = candidate("r-a", "198.51.100.10/24", None, "203.0.113.1", false, 7);
        let b = candidate("r-b", "198.51.100.11/24", None, "203.0.113.1", false, 9);
        assert_eq!(picked(&choose(vec![a, b], &pc, false)).0, "offline Some(9)");
        // online, another network
        let other = candidate(
            "r-other",
            "198.51.100.10/24",
            Some("198.51.100.1"),
            "203.0.113.1",
            true,
            50,
        );
        assert_eq!(
            picked(&choose(vec![other], &pc, false)),
            ("no_relay false".into(), "other_network")
        );
        // the same wire, another gateway: unconfirmed, and trying all relays uses it
        let unsure = || {
            candidate(
                "r-unsure",
                "192.0.2.10/24",
                Some("192.0.2.254"),
                "203.0.113.1",
                true,
                50,
            )
        };
        assert_eq!(
            picked(&choose(vec![unsure()], &pc, false)),
            ("no_relay true".into(), "unconfirmed")
        );
        assert_eq!(
            picked(&choose(vec![unsure()], &pc, true)),
            ("r-unsure".into(), "any")
        );
        // a private network of another site: the app hides "try all", the request may still ask
        let home = info("192.168.0.20/24", "192.168.0.1", "198.51.100.7");
        let site = || {
            candidate(
                "r-site",
                "192.168.0.5/24",
                Some("192.168.0.1"),
                "203.0.113.1",
                true,
                50,
            )
        };
        assert_eq!(
            picked(&choose(vec![site()], &home, false)),
            ("no_relay false".into(), "different_egress")
        );
        assert_eq!(
            picked(&choose(vec![site()], &home, true)),
            ("r-site".into(), "any")
        );
        // the same site
        let here = candidate(
            "r-here",
            "192.168.0.5/24",
            Some("192.168.0.1"),
            "198.51.100.7",
            true,
            50,
        );
        assert_eq!(
            picked(&choose(vec![here], &home, false)),
            ("r-here".into(), "exact")
        );
        // history: the relay woke this PC on this network before
        let mut known = candidate("r-known", "192.0.2.10/24", None, "203.0.113.1", true, 50);
        known.woken_on = vec!["192.0.2.0/24".into()];
        assert_eq!(
            picked(&choose(vec![known], &pc, false)),
            ("r-known".into(), "history")
        );
    }

    #[test]
    fn device_list_verdicts() {
        let ok = info("192.0.2.67/24", "192.0.2.1", "198.51.100.7");
        assert_eq!(peer_wol(Some(&ok), true), ("ok", " · 켜기 가능"));
        assert_eq!(peer_wol(Some(&ok), false), ("no", ""));
        assert_eq!(peer_wol(None, true), ("unknown", ""));
        let mut wireless = ok.clone();
        wireless.report.adapters.clear();
        assert_eq!(
            peer_wol(Some(&wireless), true),
            ("no", " · 켜기 준비 안 됨")
        );
        let mut slow = ok;
        slow.report.power.fast_startup = Some(true);
        assert_eq!(peer_wol(Some(&slow), true), ("warn", " · 켜기 가능"));
    }
}

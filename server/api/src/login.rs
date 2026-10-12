// SPDX-License-Identifier: AGPL-3.0-only

//! `POST /api/login` (design v2 §9-1, §9-2), with the checks in the design's order:
//!
//! 1. form: POST, `Content-Type: application/json`, at most 64 KiB, no `Origin` header;
//! 2. failures per client address (X-Real-IP, which nginx overwrites; an IPv6 address counts by
//!    its /64): 429 past the limit, except for a request signed by a PC already in the account it
//!    logs in to, which counts under that account's limit of step 5 instead: others behind the
//!    same address cannot lock the owner out of their PC;
//! 3. the sealed password (seal.rs): 400 unless sealed to a key of this server, fresh, from the
//!    device the request names and with a nonce not seen before; the very same body again within
//!    the retry window gets the first answer, without a second SSO call;
//! 4. the portal ID, trimmed and case-folded;
//! 5. failures per ID, the same for every ID typed; a request signed by a PC already in that
//!    account (devsig.rs) counts under a limit of its own, one for all the account's PCs, so that
//!    someone who knows only the ID cannot lock the owner out of their PC;
//! 6. the allowlist (D3): an ID off the list never reaches the SSO and fails like a wrong
//!    password, counted;
//! 7. successes per account and per address, the SSO calls of all accounts together and one
//!    account's share of them, and the pause of SSO calls while the SSO refuses this server or
//!    keeps failing (limits.rs);
//! 8. the second factor when it is on (tfa.rs; off by D11), before the SSO call;
//! 9. the SSO (sso.rs), sorted into refused credentials (counted), a refused server address and
//!    upstream errors (counted against the address only).
//!
//! Every failure has the same body and leaves no sooner than HJV_API_FAILURE_DELAY_MS (2 s) ±
//! HJV_API_FAILURE_JITTER_MS (0.5 s) after the request came, and while the SSO is slow no sooner
//! than its slowest recent call plus the jitter, so that failures with and without an SSO call
//! take as long (step 10). Every failure that depends on the ID is a 401, whatever stopped it: a
//! wrong password, an ID off the list, a limit of the ID or the account, the SSO-call limit, the
//! SSO's own trouble. The answer does not tell whether an ID is on the list (D3); the record and
//! the journal keep the real reason. The other statuses do not depend on the ID: 429 for the
//! address limit, 400 for sealing errors, 503 when too many logins run at once, 500 for the
//! server's own errors. (v2 §9-1 had 503 for the SSO-call limit and 502 for SSO errors; the app
//! shows the body's text whatever the status, research notes §9.)
//!
//! After a successful SSO check the account is created or updated, the device is added to it when
//! it is an installed Windows PC whose signature matches the key hbbs holds (D9, D10, v2 §5), and
//! a token is issued: 256 random bits, of which only the SHA-256 is stored. A new token replaces
//! the account's token from the same device, and an account keeps at most [`MAX_TOKENS`].

use crate::{
    api::{is_json, user_json, ApiRequest, BodyError, Reply},
    config::Window,
    db::{self, NewToken, Record},
    devsig, hbbs,
    limits::{self, Take, SCOPE_BOUND, SCOPE_ID, SCOPE_IP, SCOPE_OK_ID, SCOPE_OK_IP},
    seal::{Opened, SealError},
    sso::{Failure, SsoError, SsoUser},
    state::AppState,
    tfa::{Busy, Pending},
    util::{
        b64url, clean_text, login_key, now, random_below, random_bytes, sha256, valid_device_id,
        valid_portal_id,
    },
};
use hyper::{header::ORIGIN, Method, StatusCode};
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::watch;

/// The one answer to every failed login (design v2 §1-1).
pub const FAILURE_TEXT: &str =
    "로그인할 수 없습니다. ID·비밀번호를 확인하고, 계속 안 되면 잠시 뒤 다시 하거나 관리자에게 문의하세요.";

/// The tokens an account keeps: a login past it revokes the least recently used.
pub const MAX_TOKENS: usize = 20;

/// The login request as the app sends it (flutter/lib/common/hbbs/hbbs.dart `LoginRequest`), with
/// the `hjv` part that client patch 0008 adds. Other fields (uuid, autoLogin) are ignored.
#[derive(Deserialize)]
struct LoginBody {
    username: Option<String>,
    password: Option<String>,
    id: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(rename = "verificationCode")]
    verification_code: Option<String>,
    #[serde(rename = "tfaCode")]
    tfa_code: Option<String>,
    secret: Option<String>,
    #[serde(rename = "deviceInfo")]
    device_info: Option<DeviceInfo>,
    hjv: Option<HjvPart>,
}

/// `deviceInfo` (ui_interface.rs `get_login_device_info`: os, type, name).
#[derive(Deserialize)]
struct DeviceInfo {
    os: Option<String>,
    name: Option<String>,
}

/// `hjv` (devsig.rs).
#[derive(Deserialize)]
struct HjvPart {
    sig: Option<String>,
    installed: Option<bool>,
}

/// What a login says about the device it comes from.
#[derive(Debug, Clone, Default)]
pub struct DeviceClaim {
    /// the device ID of the sealed payload, when it is one ('' otherwise)
    pub id: String,
    pub os: String,
    pub hostname: String,
    pub installed: bool,
    pub signature: Option<String>,
    /// the bytes the signature should cover
    pub message: Vec<u8>,
}

impl DeviceClaim {
    fn new(body: &LoginBody, opened: &Opened) -> DeviceClaim {
        let info = body.device_info.as_ref();
        let os = info
            .and_then(|i| i.os.as_deref())
            .map(|os| clean_text(os, 32).to_ascii_lowercase())
            .unwrap_or_default();
        let hostname = info
            .and_then(|i| i.name.as_deref())
            .map(|n| clean_text(n, 64))
            .unwrap_or_default();
        let installed = body.hjv.as_ref().and_then(|h| h.installed).unwrap_or(false);
        let id = if valid_device_id(&opened.device_id) {
            opened.device_id.clone()
        } else {
            String::new()
        };
        DeviceClaim {
            message: devsig::login_message(
                &opened.device_id,
                opened.time,
                &opened.nonce,
                &opened.sealed_hash,
                installed,
                info.and_then(|i| i.os.as_deref()).unwrap_or(""),
            ),
            signature: body.hjv.as_ref().and_then(|h| h.sig.clone()),
            hostname: if hostname.is_empty() {
                id.clone()
            } else {
                hostname
            },
            id,
            os,
            installed,
        }
    }

    fn signed_by(&self, public_key: &[u8]) -> bool {
        self.signature
            .as_deref()
            .is_some_and(|sig| devsig::verify(public_key, &self.message, sig))
    }
}

/// What a sealed login claims: the portal ID as typed, its case-folded key when it is an ID at
/// all, and the device.
struct Claim {
    username: String,
    key: Option<String>,
    device: DeviceClaim,
}

impl Claim {
    fn new(body: &LoginBody, opened: &Opened) -> Claim {
        let username = body.username.as_deref().unwrap_or("").trim().to_owned();
        Claim {
            key: valid_portal_id(&username).then(|| login_key(&username)),
            username,
            device: DeviceClaim::new(body, opened),
        }
    }
}

/// What a login did with its device; GET /api/hjv/this-device reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindStatus {
    /// added to the account by this login
    Added,
    /// already in this account
    Already,
    /// in another account: refused (v2 §5)
    OtherAccount,
    /// not a device the list takes (a phone, a PC running without installing, no signature)
    NotEligible,
    /// hbbs does not know it, could not be asked, or the signature did not match
    CheckFailed,
    /// taken out of the account since
    Removed,
    /// no device in this login
    None,
}

impl BindStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            BindStatus::Added => "added",
            BindStatus::Already => "already",
            BindStatus::OtherAccount => "other_account",
            BindStatus::NotEligible => "not_eligible",
            BindStatus::CheckFailed => "check_failed",
            BindStatus::Removed => "removed",
            BindStatus::None => "none",
        }
    }

    pub fn parse(text: &str) -> BindStatus {
        match text {
            "added" => BindStatus::Added,
            "already" => BindStatus::Already,
            "other_account" => BindStatus::OtherAccount,
            "not_eligible" => BindStatus::NotEligible,
            "check_failed" => BindStatus::CheckFailed,
            "removed" => BindStatus::Removed,
            _ => BindStatus::None,
        }
    }
}

/// The answer to one login.
#[derive(Debug, Clone)]
pub struct LoginAnswer {
    pub status: StatusCode,
    pub body: Arc<String>,
    pub failure: bool,
}

impl LoginAnswer {
    fn failed(status: StatusCode) -> LoginAnswer {
        LoginAnswer {
            status,
            body: Arc::new(json!({ "error": FAILURE_TEXT }).to_string()),
            failure: true,
        }
    }

    fn ok(body: serde_json::Value) -> LoginAnswer {
        LoginAnswer {
            status: StatusCode::OK,
            body: Arc::new(body.to_string()),
            failure: false,
        }
    }

    fn into_reply(self) -> Reply {
        Reply {
            allow: (self.status == StatusCode::METHOD_NOT_ALLOWED).then_some("POST"),
            status: self.status,
            body: self.body.as_ref().clone(),
        }
    }
}

/// Answers kept for the app's retry of the same login (v2 §9-2 step 3): the app sends the very
/// same body again when its first TLS attempt failed, possibly after hjv-api got the request.
pub struct RetryCache {
    window: Duration,
    entries: Mutex<HashMap<String, RetryEntry>>,
}

struct RetryEntry {
    body_hash: [u8; 32],
    created: Instant,
    answer: watch::Receiver<Option<LoginAnswer>>,
}

pub enum Retry {
    /// the first request with this nonce: send its answer here
    First(watch::Sender<Option<LoginAnswer>>),
    /// the same body again: its answer comes here
    Again(watch::Receiver<Option<LoginAnswer>>),
    /// the nonce with another body
    Reused,
}

impl RetryCache {
    pub fn new(window_secs: i64) -> RetryCache {
        RetryCache {
            window: Duration::from_secs(window_secs as u64),
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub fn begin(&self, key: &str, body_hash: [u8; 32]) -> Retry {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.retain(|_, e| e.created.elapsed() < self.window);
        match entries.get(key) {
            Some(e) if e.body_hash == body_hash => Retry::Again(e.answer.clone()),
            Some(_) => Retry::Reused,
            None => {
                let (tx, rx) = watch::channel(None);
                entries.insert(
                    key.to_owned(),
                    RetryEntry {
                        body_hash,
                        created: Instant::now(),
                        answer: rx,
                    },
                );
                Retry::First(tx)
            }
        }
    }

    pub fn purge(&self) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.retain(|_, e| e.created.elapsed() < self.window);
    }
}

/// Handles `POST /api/login`.
pub async fn handle(state: &Arc<AppState>, req: ApiRequest) -> Reply {
    let started = Instant::now();
    let Ok(permit) = state.login_slots.clone().try_acquire_owned() else {
        if state.first_notice("login-slots", Duration::from_secs(60)) {
            log_warn!("login: too many logins at once, refused");
        }
        return LoginAnswer::failed(StatusCode::SERVICE_UNAVAILABLE).into_reply();
    };
    // The login runs to its end even when the client goes away, so that the answer is there for
    // the app's retry of the same body; it holds its slot until then.
    let task_state = state.clone();
    let task = tokio::spawn(async move {
        let answer = attempt(&task_state, req).await;
        drop(permit);
        answer
    });
    let answer = match task.await {
        Ok(answer) => answer,
        Err(_) => {
            log_err!("login: the login task failed");
            LoginAnswer::failed(StatusCode::INTERNAL_SERVER_ERROR)
        }
    };
    if answer.failure {
        tokio::time::sleep_until((started + failure_delay(state)).into()).await;
    }
    answer.into_reply()
}

/// How long after the request a failure may leave: the delay ± the jitter, uniformly. While the
/// SSO is slow the delay grows to its slowest recent call (at most the SSO time limit) plus the
/// jitter, so that a failure without an SSO call (an ID off the list) takes as long as one with
/// a call (v2 §9-2 step 10).
fn failure_delay(state: &AppState) -> Duration {
    let c = &state.config;
    let slow = state.sso_latency.recent().min(c.sso.total_timeout) + c.failure_jitter;
    let floor = c.failure_delay.max(slow);
    let jitter = c.failure_jitter.as_millis() as u64;
    floor - c.failure_jitter + Duration::from_millis(random_below(2 * jitter + 1))
}

/// Whether a seal made at `time` by the app's clock is fresh at `at`: at most `skew` seconds
/// apart either way, for any time a payload may hold.
pub fn fresh(at: i64, time: i64, skew: i64) -> bool {
    at.abs_diff(time) <= skew.unsigned_abs()
}

/// How a login ended, for its limit counters (limits.rs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// the client's failure (a wrong password, an ID off the list, a bad seal …): the address's
    /// and the ID's attempts count
    Failed,
    /// refused for a reason other than the password (a success limit, the SSO-call limit, the
    /// SSO's trouble): the address's attempt counts as for any failure, so that an address sees
    /// the same whether an ID is on the list or not (D3); the ID's is taken back
    Refused,
    /// the server's own fault, or a retry of a login already running: nothing counts
    Released,
    /// the code dialog of the second factor: the address's attempt counts until the code step
    /// succeeds (tfa.rs)
    Challenge,
    /// the success counters count; the failure counters, also the code dialog's, are taken back
    Success,
}

/// The limit a login's ID counts under (step 5).
struct Limit {
    scope: &'static str,
    key: String,
    windows: Vec<Window>,
}

/// One login's reserved limit counters and its record.
struct Run<'a> {
    state: &'a Arc<AppState>,
    /// the client's address, for the record
    ip: String,
    /// its tag, for the limits (an IPv6 address by its /64)
    ip_tag: String,
    ip_attempt: Option<i64>,
    id_attempt: Option<i64>,
    success_slots: Vec<i64>,
    /// the address's attempt of the code dialog this request answers
    challenge_attempt: Option<i64>,
    account_tag: Option<String>,
    device_tag: Option<String>,
}

impl<'a> Run<'a> {
    fn new(state: &'a Arc<AppState>, ip: &str) -> Run<'a> {
        Run {
            state,
            ip: ip.to_owned(),
            ip_tag: state.tags.ip(ip),
            ip_attempt: None,
            id_attempt: None,
            success_slots: Vec::new(),
            challenge_attempt: None,
            account_tag: None,
            device_tag: None,
        }
    }

    /// Reserves an attempt; `Ok(None)` when the limit is full.
    async fn reserve(
        &self,
        scope: &'static str,
        key: String,
        windows: Vec<Window>,
    ) -> Result<Option<i64>, ()> {
        let at = now();
        self.state
            .db
            .call(move |c| limits::reserve(c, scope, &key, &windows, at))
            .await
            .map_err(|e| log_err!("login: {e}"))
    }

    /// Settles the reserved attempts as `outcome` says and writes the record `result` (that of a
    /// failure only while the minute's failure records last), in one transaction.
    async fn end(&mut self, outcome: Outcome, result: Option<&'static str>) {
        let (ip, id) = (self.ip_attempt.take(), self.id_attempt.take());
        let slots = std::mem::take(&mut self.success_slots);
        let (mut keep, mut back) = (Vec::new(), Vec::new());
        match outcome {
            Outcome::Failed => {
                keep.extend(ip.into_iter().chain(id));
                back.extend(slots);
            }
            Outcome::Refused | Outcome::Challenge => {
                keep.extend(ip);
                back.extend(id.into_iter().chain(slots));
            }
            Outcome::Released => back.extend(ip.into_iter().chain(id).chain(slots)),
            Outcome::Success => {
                keep.extend(slots);
                back.extend(
                    ip.into_iter()
                        .chain(id)
                        .chain(self.challenge_attempt.take()),
                );
            }
        }
        let record = result
            .filter(|_| outcome == Outcome::Success || self.state.admit_failure_record())
            .map(|result| Record {
                at: now(),
                kind: "login",
                account_tag: self.account_tag.clone(),
                device_tag: self.device_tag.clone(),
                ip: Some(self.ip.clone()),
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
            log_err!("login: {e}");
        }
    }

    /// Takes the reserved attempts back without a record (a retry of a running login).
    async fn release(&mut self) {
        self.end(Outcome::Released, None).await;
    }

    async fn fail(
        &mut self,
        status: StatusCode,
        outcome: Outcome,
        result: &'static str,
    ) -> LoginAnswer {
        self.end(outcome, Some(result)).await;
        LoginAnswer::failed(status)
    }
}

async fn attempt(state: &Arc<AppState>, req: ApiRequest) -> LoginAnswer {
    // 1. the form of the request
    if req.headers.contains_key(ORIGIN) {
        return LoginAnswer::failed(StatusCode::FORBIDDEN);
    }
    if req.method != Method::POST {
        return LoginAnswer::failed(StatusCode::METHOD_NOT_ALLOWED);
    }
    if !is_json(&req.headers) {
        return LoginAnswer::failed(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    let body = match &req.body {
        Ok(body) => body.clone(),
        Err(BodyError::TooLarge) => return LoginAnswer::failed(StatusCode::PAYLOAD_TOO_LARGE),
        Err(BodyError::Unreadable) => return LoginAnswer::failed(StatusCode::BAD_REQUEST),
    };
    let Ok(parsed) = serde_json::from_slice::<LoginBody>(&body) else {
        return LoginAnswer::failed(StatusCode::BAD_REQUEST);
    };

    // 2. failures per client address
    let mut run = Run::new(state, &req.ip);
    let address_full = match run
        .reserve(SCOPE_IP, run.ip_tag.clone(), state.config.limit_ip.clone())
        .await
    {
        Ok(Some(id)) => {
            run.ip_attempt = Some(id);
            false
        }
        Ok(None) => true,
        Err(()) => return LoginAnswer::failed(StatusCode::INTERNAL_SERVER_ERROR),
    };
    let body_hash = sha256(&body);
    match parsed.kind.as_deref() {
        None | Some("account") => {}
        _ if address_full => return address_limited(&mut run).await,
        Some("email_code") => return second_step(&mut run, parsed, body_hash).await,
        Some(_) => {
            return run
                .fail(StatusCode::BAD_REQUEST, Outcome::Failed, "bad-type")
                .await
        }
    }

    // 3. the sealed password
    let opened = match open_seal(state, &parsed) {
        Ok(opened) => opened,
        Err(_) if address_full => return address_limited(&mut run).await,
        Err(result) => {
            return run
                .fail(StatusCode::BAD_REQUEST, Outcome::Failed, result)
                .await
        }
    };
    let claim = Claim::new(&parsed, &opened);
    // past a full address limit only a PC already in the account it logs in to (step 5)
    let mut bound = None;
    if address_full {
        match bound_limit(state, &claim).await {
            Some(limit) => bound = Some(limit),
            None => return address_limited(&mut run).await,
        }
    }
    match state
        .retries
        .begin(&format!("seal:{}", opened.nonce), body_hash)
    {
        Retry::Again(answer) => {
            run.release().await;
            wait_for(answer).await
        }
        Retry::Reused => {
            run.fail(StatusCode::BAD_REQUEST, Outcome::Failed, "seal-nonce")
                .await
        }
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
            let answer = match fresh {
                Ok(true) => sealed_login(&mut run, opened, claim, bound).await,
                Ok(false) => {
                    run.fail(StatusCode::BAD_REQUEST, Outcome::Failed, "seal-nonce")
                        .await
                }
                Err(e) => {
                    log_err!("login: {e}");
                    run.fail(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Outcome::Released,
                        "error",
                    )
                    .await
                }
            };
            let _ = tx.send(Some(answer.clone()));
            answer
        }
    }
}

/// Opens the sealed password and checks it against the request and the clock (step 3, but the
/// nonce); on failure, the record's result.
fn open_seal(state: &AppState, parsed: &LoginBody) -> Result<Opened, &'static str> {
    let opened = state
        .seal
        .open(parsed.password.as_deref().unwrap_or(""))
        .map_err(|e| match e {
            SealError::NotSealed => "seal-missing",
            SealError::UnknownKey => "seal-unknown-key",
            SealError::Corrupt => "seal-corrupt",
        })?;
    if parsed.id.as_deref().unwrap_or("") != opened.device_id {
        return Err("seal-device");
    }
    if !fresh(now(), opened.time, state.config.seal_max_skew_secs) {
        return Err("seal-time");
    }
    Ok(opened)
}

/// A login refused by a full address limit (step 2): it counts nowhere, and the record and the
/// journal hear of it once per window, so that hammering writes nothing.
async fn address_limited(run: &mut Run<'_>) -> LoginAnswer {
    let state = run.state;
    let key = format!("ip:{}", run.ip_tag);
    if state.first_notice(&key, window_of(&state.config.limit_ip)) {
        log_warn!(
            "login: address limit reached (ip={}); hjv-api admin unlock tag:{} lifts it",
            run.ip_tag,
            run.ip_tag
        );
        run.end(Outcome::Failed, Some("limited-ip")).await;
    }
    LoginAnswer::failed(StatusCode::TOO_MANY_REQUESTS)
}

async fn wait_for(mut answer: watch::Receiver<Option<LoginAnswer>>) -> LoginAnswer {
    match answer.wait_for(Option::is_some).await {
        Ok(a) => a.clone().expect("waited for an answer"),
        Err(_) => LoginAnswer::failed(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// The limit of a request signed by a PC already in the account it logs in to, if it is one
/// (v2 §9-2 step 5).
async fn bound_limit(state: &Arc<AppState>, claim: &Claim) -> Option<Limit> {
    let key = claim.key.as_deref()?;
    let device = &claim.device;
    if device.signature.is_none() || device.id.is_empty() {
        return None;
    }
    let (id, k) = (device.id.clone(), key.to_owned());
    let stored = state
        .db
        .call(move |c| db::bound_key(c, &id, &k))
        .await
        .ok()??;
    device.signed_by(&stored).then(|| Limit {
        scope: SCOPE_BOUND,
        key: state.tags.bound(key),
        windows: state.config.limit_bound.clone(),
    })
}

async fn sealed_login(
    run: &mut Run<'_>,
    opened: Opened,
    claim: Claim,
    bound: Option<Limit>,
) -> LoginAnswer {
    let state = run.state;
    // 4. the portal ID
    if !claim.device.id.is_empty() {
        run.device_tag = Some(state.tags.device(&claim.device.id));
    }
    let Some(key) = claim.key.clone() else {
        return run
            .fail(StatusCode::UNAUTHORIZED, Outcome::Failed, "bad-id")
            .await;
    };
    run.account_tag = Some(state.tags.account(&key));

    // 5. failures per ID
    let limit = match bound {
        Some(limit) => limit,
        None => match bound_limit(state, &claim).await {
            Some(limit) => limit,
            None => Limit {
                scope: SCOPE_ID,
                key: state.tags.account(&key),
                windows: state.config.limit_id.clone(),
            },
        },
    };
    match run
        .reserve(limit.scope, limit.key.clone(), limit.windows.clone())
        .await
    {
        Ok(Some(id)) => run.id_attempt = Some(id),
        Ok(None) => return id_limited(run, &limit).await,
        Err(()) => {
            return run
                .fail(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Outcome::Released,
                    "error",
                )
                .await
        }
    }

    // 6. the allowlist
    let allowed = {
        let key = key.clone();
        match state.db.call(move |c| db::allowed(c, &key)).await {
            Ok(allowed) => allowed,
            Err(e) => {
                log_err!("login: {e}");
                return run
                    .fail(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Outcome::Released,
                        "error",
                    )
                    .await;
            }
        }
    };

    let Claim {
        username, device, ..
    } = claim;
    // 8. the second factor comes before the allowlist shows: every ID gets the code dialog
    if state.second_factor.required() {
        let challenge = b64url(&random_bytes::<32>());
        let pending = Pending::new(
            username.clone(),
            key,
            opened.password,
            allowed,
            (limit.scope, limit.key),
            device,
            (run.ip_tag.clone(), run.ip_attempt),
        );
        match state.pending.insert(challenge.clone(), pending) {
            Ok(()) => {}
            Err(Busy::Full) => {
                if state.first_notice("tfa-full", Duration::from_secs(60)) {
                    log_warn!("login: too many logins wait for a code");
                }
                return run
                    .fail(StatusCode::SERVICE_UNAVAILABLE, Outcome::Refused, "busy")
                    .await;
            }
            Err(Busy::Each) => {
                return run
                    .fail(StatusCode::UNAUTHORIZED, Outcome::Failed, "tfa-busy")
                    .await
            }
        }
        run.end(Outcome::Challenge, Some("challenge")).await;
        return LoginAnswer::ok(json!({
            "type": "email_check",
            "tfa_type": "tfa_check",
            "secret": challenge,
            "user": { "name": username },
        }));
    }
    if !allowed {
        return run
            .fail(StatusCode::UNAUTHORIZED, Outcome::Failed, "not-allowed")
            .await;
    }
    sso_login(run, &username, &key, &opened.password, device).await
}

/// A login refused by a full ID limit: counted against the address only; the journal hears of
/// it once per window.
async fn id_limited(run: &mut Run<'_>, limit: &Limit) -> LoginAnswer {
    let state = run.state;
    let (result, from) = if limit.scope == SCOPE_BOUND {
        ("limited-bound", "from a PC in the account")
    } else {
        ("limited-id", "any device")
    };
    if state.first_notice(
        &format!("{}:{}", limit.scope, limit.key),
        window_of(&limit.windows),
    ) {
        log_warn!(
            "login: ID limit reached (acct={}, {from}); hjv-api admin unlock tag:{} lifts it",
            run.account_tag.as_deref().unwrap_or("-"),
            run.account_tag.as_deref().unwrap_or("-"),
        );
    }
    run.fail(StatusCode::UNAUTHORIZED, Outcome::Failed, result)
        .await
}

/// The shortest window of a limit.
fn window_of(windows: &[Window]) -> Duration {
    let secs = windows.iter().map(|w| w.secs).min().unwrap_or(600);
    Duration::from_secs(secs as u64)
}

/// The second request of a login with a second factor (tfa.rs).
async fn second_step(run: &mut Run<'_>, parsed: LoginBody, body_hash: [u8; 32]) -> LoginAnswer {
    let state = run.state;
    let challenge = parsed.secret.clone().unwrap_or_default();
    if !state.second_factor.required() || challenge.is_empty() {
        return run
            .fail(StatusCode::UNAUTHORIZED, Outcome::Failed, "no-challenge")
            .await;
    }
    let tx = match state.retries.begin(&format!("tfa:{challenge}"), body_hash) {
        Retry::Again(answer) => {
            run.release().await;
            return wait_for(answer).await;
        }
        Retry::Reused => {
            return run
                .fail(StatusCode::UNAUTHORIZED, Outcome::Failed, "no-challenge")
                .await
        }
        Retry::First(tx) => tx,
    };
    let answer = code_login(run, parsed, &challenge).await;
    let _ = tx.send(Some(answer.clone()));
    answer
}

async fn code_login(run: &mut Run<'_>, parsed: LoginBody, challenge: &str) -> LoginAnswer {
    let state = run.state;
    let Some(pending) = state.pending.take(challenge) else {
        return run
            .fail(StatusCode::UNAUTHORIZED, Outcome::Failed, "no-challenge")
            .await;
    };
    run.challenge_attempt = pending.ip_attempt;
    run.account_tag = Some(state.tags.account(&pending.login_key));
    if !pending.device.id.is_empty() {
        run.device_tag = Some(state.tags.device(&pending.device.id));
    }
    if parsed.username.as_deref().unwrap_or("").trim() != pending.username {
        return run
            .fail(StatusCode::UNAUTHORIZED, Outcome::Failed, "no-challenge")
            .await;
    }
    // 5. the ID limit of the first step
    let limit = Limit {
        scope: pending.limit_scope,
        key: pending.limit_key.clone(),
        windows: if pending.limit_scope == SCOPE_BOUND {
            state.config.limit_bound.clone()
        } else {
            state.config.limit_id.clone()
        },
    };
    match run
        .reserve(limit.scope, limit.key.clone(), limit.windows.clone())
        .await
    {
        Ok(Some(id)) => run.id_attempt = Some(id),
        Ok(None) => return id_limited(run, &limit).await,
        Err(()) => {
            return run
                .fail(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Outcome::Released,
                    "error",
                )
                .await
        }
    }
    // 6. and 8.: off the list or a wrong code, the same failure
    if !pending.allowed {
        return run
            .fail(StatusCode::UNAUTHORIZED, Outcome::Failed, "not-allowed")
            .await;
    }
    let code = parsed
        .tfa_code
        .or(parsed.verification_code)
        .unwrap_or_default();
    if !state.second_factor.verify(&pending.login_key, code.trim()) {
        return run
            .fail(StatusCode::UNAUTHORIZED, Outcome::Failed, "code-failed")
            .await;
    }
    sso_login(
        run,
        &pending.username,
        &pending.login_key,
        &pending.password,
        pending.device,
    )
    .await
}

async fn sso_login(
    run: &mut Run<'_>,
    username: &str,
    key: &str,
    password: &str,
    device: DeviceClaim,
) -> LoginAnswer {
    let state = run.state;
    let account_tag = run.account_tag.clone().unwrap_or_default();
    // 7. successes per account and per address
    for (scope, limit_key, windows, of) in [
        (
            SCOPE_OK_ID,
            account_tag.clone(),
            &state.config.limit_ok_id,
            "account",
        ),
        (
            SCOPE_OK_IP,
            run.ip_tag.clone(),
            &state.config.limit_ok_ip,
            "address",
        ),
    ] {
        match run.reserve(scope, limit_key.clone(), windows.clone()).await {
            Ok(Some(id)) => run.success_slots.push(id),
            Ok(None) => {
                if state.first_notice(&format!("{scope}:{limit_key}"), window_of(windows)) {
                    log_warn!(
                        "login: success limit of the {of} reached ({of}={limit_key}); hjv-api admin unlock tag:{limit_key} lifts it"
                    );
                }
                return run
                    .fail(StatusCode::UNAUTHORIZED, Outcome::Refused, "limited-ok")
                    .await;
            }
            Err(()) => {
                return run
                    .fail(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Outcome::Released,
                        "error",
                    )
                    .await
            }
        }
    }
    // the SSO calls are paused (limits::Breaker)
    if state.sso_breaker.paused() {
        return run
            .fail(StatusCode::UNAUTHORIZED, Outcome::Refused, "sso-paused")
            .await;
    }
    // the SSO calls of all accounts, and this account's share of them
    match state.sso_calls.try_take(&account_tag) {
        Take::Taken => {}
        Take::Full => {
            if state.first_notice("sso-calls", Duration::from_secs(60)) {
                log_warn!("login: SSO call limit reached");
            }
            return run
                .fail(StatusCode::UNAUTHORIZED, Outcome::Refused, "limited-sso")
                .await;
        }
        Take::Share => {
            if state.first_notice(&format!("sso-share:{account_tag}"), Duration::from_secs(60)) {
                log_warn!(
                    "login: one account's share of the SSO calls reached (acct={account_tag})"
                );
            }
            return run
                .fail(StatusCode::UNAUTHORIZED, Outcome::Refused, "limited-sso")
                .await;
        }
    }
    // 9. the SSO
    let started = Instant::now();
    let result = state.sso.authenticate(username, password).await;
    state.sso_latency.note(started.elapsed());
    if let Some(pause) = state
        .sso_breaker
        .note(result.as_ref().err().map(|e| e.failure))
    {
        log_err!(
            "login: SSO calls paused for {} s; logins fail meanwhile without calling it",
            pause.as_secs()
        );
    }
    match result {
        Ok(user) => success(run, key, user, device).await,
        Err(e) => {
            note_sso_message(state, &e, username);
            match e.failure {
                Failure::Credentials => {
                    run.fail(StatusCode::UNAUTHORIZED, Outcome::Failed, "credentials")
                        .await
                }
                Failure::AddressRefused => {
                    log_crit!(
                        "login: the SSO refuses this server's address ({}); no one can log in until it is registered again",
                        e.detail
                    );
                    run.fail(StatusCode::UNAUTHORIZED, Outcome::Refused, "sso-address")
                        .await
                }
                Failure::Upstream => {
                    log_warn!("login: SSO error: {}", e.detail);
                    run.fail(StatusCode::UNAUTHORIZED, Outcome::Refused, "upstream")
                        .await
                }
            }
        }
    }
}

/// Writes the SSO's message to the journal the first time it shows up, with the ID masked
/// (design v2 §4-1).
fn note_sso_message(state: &AppState, e: &SsoError, username: &str) {
    let Some(message) = e.message.as_deref().filter(|m| !m.is_empty()) else {
        return;
    };
    let masked = mask_id(message, username);
    let mut seen = state.sso_messages.lock().unwrap_or_else(|e| e.into_inner());
    if seen.len() < 100 && seen.insert(masked.clone()) {
        log_notice!("login: SSO message seen for the first time: {masked}");
    }
}

/// `message` with every occurrence of `id` (in any case) replaced by "***".
fn mask_id(message: &str, id: &str) -> String {
    if id.is_empty() {
        return message.to_owned();
    }
    let lower = message.to_ascii_lowercase();
    let needle = id.to_ascii_lowercase();
    let mut out = String::new();
    let mut rest = 0;
    while let Some(found) = lower[rest..].find(&needle) {
        out.push_str(&message[rest..rest + found]);
        out.push_str("***");
        rest += found + needle.len();
    }
    out.push_str(&message[rest..]);
    out
}

async fn success(run: &mut Run<'_>, key: &str, user: SsoUser, device: DeviceClaim) -> LoginAnswer {
    let state = run.state;
    let account_tag = run.account_tag.clone().unwrap_or_default();
    if login_key(&user.persl_no) != key {
        log_notice!("login: the portal ID typed and the SSO's perslNo differ (acct={account_tag})");
    }
    let at = now();
    let account_id = {
        let (key, user) = (key.to_owned(), user.clone());
        match state
            .db
            .call(move |c| db::upsert_account(c, &key, &user.persl_no, &user.name, at))
            .await
        {
            Ok(id) => id,
            Err(e) => {
                log_err!("login: {e}");
                return run
                    .fail(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Outcome::Released,
                        "error",
                    )
                    .await;
            }
        }
    };
    let (bind_status, verified) = bind_device(state, account_id, &device, &account_tag).await;
    let token = format!("hjv_{}", b64url(&random_bytes::<32>()));
    let hash = sha256(token.as_bytes());
    let device_id = device.id.clone();
    let stored = state
        .db
        .call(move |c| {
            let token = NewToken {
                hash: &hash,
                account_id,
                device_id: &device_id,
                device_verified: verified,
                bind_status: bind_status.as_str(),
            };
            db::insert_token(c, &token, at, MAX_TOKENS)
        })
        .await;
    if let Err(e) = stored {
        log_err!("login: {e}");
        return run
            .fail(
                StatusCode::INTERNAL_SERVER_ERROR,
                Outcome::Released,
                "error",
            )
            .await;
    }
    run.end(Outcome::Success, Some("ok")).await;
    LoginAnswer::ok(json!({
        "type": "access_token",
        "access_token": token,
        "user": user_json(&user.persl_no, &user.name),
    }))
}

/// Puts the login's device into the account when it qualifies (D9, D10, v2 §5). Returns what
/// happened and whether the device proved it holds the key hbbs has for its ID.
async fn bind_device(
    state: &Arc<AppState>,
    account_id: i64,
    device: &DeviceClaim,
    account_tag: &str,
) -> (BindStatus, bool) {
    if device.id.is_empty() {
        return (BindStatus::None, false);
    }
    if device.signature.is_none() || !device.installed || !state.config.bind_os.contains(&device.os)
    {
        return (BindStatus::NotEligible, false);
    }
    let device_tag = state.tags.device(&device.id);
    let record = |result: &'static str| {
        state.record(
            "device",
            Some(account_tag.to_owned()),
            Some(device_tag.clone()),
            None,
            result,
        )
    };
    let Some(server_key) = state.config.hbbs_pubkey else {
        record("no-hbbs-key").await;
        return (BindStatus::CheckFailed, false);
    };
    let lookup = hbbs::lookup(state.config.hbbs_console, Some(&server_key), &device.id).await;
    let public_key = match lookup {
        Ok(hbbs::Lookup::Known { public_key, .. }) => public_key,
        Ok(hbbs::Lookup::Unknown) => {
            record("unknown-to-hbbs").await;
            return (BindStatus::CheckFailed, false);
        }
        Err(e @ hbbs::HbbsError::BadSignature) => {
            log_err!("device check: {e} (dev={device_tag})");
            record("hbbs-bad-signature").await;
            return (BindStatus::CheckFailed, false);
        }
        Err(e) => {
            log_warn!("device check: {e}");
            record("hbbs-error").await;
            return (BindStatus::CheckFailed, false);
        }
    };
    if !device.signed_by(&public_key) {
        log_warn!("device check: the signature does not match hbbs's key (dev={device_tag})");
        record("bad-signature").await;
        return (BindStatus::CheckFailed, false);
    }
    let (id, os, hostname) = (
        device.id.clone(),
        device.os.clone(),
        device.hostname.clone(),
    );
    let at = now();
    let bound = state
        .db
        .call(move |c| db::bind(c, account_id, &id, &public_key, &os, &hostname, at))
        .await;
    match bound {
        Ok(db::Bound::Added) => {
            record("added").await;
            (BindStatus::Added, true)
        }
        Ok(db::Bound::Already) => (BindStatus::Already, true),
        Ok(db::Bound::OtherAccount) => {
            log_warn!(
                "device check: the device is in another account (dev={device_tag}, acct={account_tag}); admin: hjv-api admin device remove tag:{device_tag}"
            );
            record("conflict").await;
            (BindStatus::OtherAccount, true)
        }
        Err(e) => {
            log_err!("device check: {e}");
            (BindStatus::CheckFailed, false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_the_id_in_sso_messages() {
        assert_eq!(mask_id("user Y1234 locked", "y1234"), "user *** locked");
        assert_eq!(mask_id("y1234y1234", "Y1234"), "******");
        assert_eq!(mask_id("사용자 abc 없음", "ABC"), "사용자 *** 없음");
        assert_eq!(mask_id("nothing", "x9"), "nothing");
    }

    #[test]
    fn bind_status_names() {
        for s in [
            BindStatus::Added,
            BindStatus::Already,
            BindStatus::OtherAccount,
            BindStatus::NotEligible,
            BindStatus::CheckFailed,
            BindStatus::Removed,
            BindStatus::None,
        ] {
            assert_eq!(BindStatus::parse(s.as_str()), s);
        }
    }

    #[test]
    fn retry_cache() {
        let cache = RetryCache::new(30);
        let Retry::First(tx) = cache.begin("n", [1; 32]) else {
            panic!("first")
        };
        let Retry::Again(mut rx) = cache.begin("n", [1; 32]) else {
            panic!("again")
        };
        assert!(matches!(cache.begin("n", [2; 32]), Retry::Reused));
        tx.send(Some(LoginAnswer::failed(StatusCode::UNAUTHORIZED)))
            .unwrap();
        assert!(rx.borrow_and_update().is_some());
        assert!(matches!(cache.begin("m", [1; 32]), Retry::First(_)));
    }

    #[test]
    fn seal_times_far_off_never_pass() {
        let at = 1_760_000_000;
        assert!(fresh(at, at, 600));
        assert!(fresh(at, at - 600, 600));
        assert!(fresh(at, at + 600, 600));
        assert!(!fresh(at, at - 601, 600));
        assert!(!fresh(at, at + 601, 600));
        // times that overflow a subtraction
        for t in [i64::MIN, i64::MAX, at.wrapping_add(i64::MIN), -at, 0] {
            assert!(!fresh(at, t, 600), "{t}");
        }
        assert!(!fresh(i64::MIN, i64::MAX, 600));
        assert!(fresh(i64::MAX, i64::MAX - 1, 600));
    }
}

// SPDX-License-Identifier: AGPL-3.0-only

//! `POST /api/login` (design v2 §9-1, §9-2), with the checks in the design's order:
//!
//! 1. form: POST, `Content-Type: application/json`, at most 64 KiB, no `Origin` header;
//! 2. failures per client address (X-Real-IP, which nginx overwrites; an IPv6 address counts by
//!    its /64): 429 past the limit, except for a request signed by a PC already in the account it
//!    logs in to, which counts under that account's limit of step 5 instead: others behind the
//!    same address cannot lock the owner out of their PC here (nginx's own limit per address
//!    comes first and has no such exception: README.md, Run);
//! 3. the sealed password (seal.rs): 400 unless sealed to a key of this server, without a purpose
//!    (an access password's seal is not a login's, guest.rs), from the device the request names
//!    and with a nonce not seen before; 409 `clock` (with the server's time)
//!    when its time is more than ten minutes off (design v3 §4-1), so that the app can tell a
//!    wrong clock from a wrong password; the very same body again within the retry window gets
//!    the first answer, without a second SSO call;
//! 4. the portal ID, trimmed: HJViewer's ID rule (util.rs `staff_id`: uppercased, Z or Y and not
//!    ZA, the portal IDs of faculty and staff) before anything else is done with the ID. One that
//!    fails it gets 403 [`TEXT_ID_RULE`] at once: no SSO call, none of the SSO calls of step 8
//!    taken, counted against the address only (a client's failure, as a bad seal is), and the
//!    record keeps no tag of it. The rule is public, so the answer tells nothing about the
//!    allowlist or an account. The ID goes on case-folded (the account's key). The one ID outside
//!    the rule that goes on is the App Review account's, while its file names it (review.rs):
//!    steps 5, 7 and 8 as for any ID, then its file's hash in place of the SSO (no step 6, 9 or
//!    10), a wrong password failing as one the SSO refused, and the rest, into no account that
//!    is not its own;
//! 5. failures per ID, the same for every ID typed; a request signed by a PC already in that
//!    account (devsig.rs) counts under a limit of its own, one for all the account's PCs, so that
//!    someone who knows only the ID cannot lock the owner out of their PC. With
//!    HJV_API_LOGIN=open an ID that is not on the allowlist counts under the lower
//!    HJV_API_LIMIT_NEW_ID: there any portal ID reaches the SSO, and this keeps the login from
//!    becoming a way to try passwords on every Yonsei account or to lock it;
//! 6. while SSO calls are paused (the SSO refuses this server, its address or its client
//!    credentials, or keeps failing: limits.rs), every ID gets the same 503 [`TEXT_SSO_DOWN`]
//!    here, before the allowlist shows (design v3 §4-1), and nothing counts;
//! 7. the allowlist: with HJV_API_LOGIN=open (the user's decision of 2026-10-14) every ID goes on
//!    to the SSO; with `list` (D3) an ID off the list never reaches the SSO and fails like a
//!    wrong password, counted. A blocked entry goes on in either mode: what it gets is decided
//!    once the SSO accepted the password (step 12);
//! 8. successes per account and per address, the SSO calls of all accounts together and one
//!    account's share of them (limits.rs); in open mode an ID that is not on the allowlist first
//!    takes one of the few SSO calls of all such IDs (HJV_API_LIMIT_SSO_NEW, `limited-sso-new`),
//!    so that it cannot use up the calls of the people registered, and the five-minute check
//!    reports that window full (check.rs). One ID and one client address may each hold a quarter
//!    of it, so that one address cannot shut the first logins of everyone with a few wrong
//!    passwords, and a first login that goes through gives its call back;
//! 9. the second factor when it is on (tfa.rs; off by D11 and U1), before the SSO call;
//! 10. the SSO (sso.rs), with the ID typed uppercased, sorted into refused credentials (counted),
//!     a denied person (the user call refused, its pcode outside HJViewer's ID rule, or a pcode
//!     or name whose value the contract refuses: 403 [`TEXT_DENIED`], which only someone who
//!     knows the password sees, counted as a success, as a blocked account is in step 12; the
//!     record and the journal keep the reason), a refused server address or client and upstream
//!     errors (counted against the address only). The user call refusing several different
//!     accounts in a row alerts the operator (limits::UserRefusals);
//! 11. the account key of the sealed payload (keys.rs, design v3 §4-1): 400 without `ck` and
//!     `cp`, with a `cp` that is not its signature or with a key another row has. Past the
//!     account's active keys the least recently used one is logged out (keys.rs): a login is not
//!     refused for it;
//! 12. the allowlist once more (schema 5), before anything is looked up or written for the
//!     account and again inside the login's transaction: a blocked account (the entry of the ID
//!     typed, of the ID the SSO names for the person, its pcode, or of another account the SSO
//!     named so: db.rs `login_listing`) gets 403 [`TEXT_BLOCKED`] with `"hjv":"blocked"`, which
//!     only someone who knows its password sees (its successes count, its failures are taken
//!     back); in open mode an ID not on the list is put on it (`source` 'login', one journal line
//!     with the account's tag), in list mode it fails as in step 7.
//!
//! Every failure leaves no sooner than HJV_API_FAILURE_DELAY_MS (2 s) ± HJV_API_FAILURE_JITTER_MS
//! (0.5 s) after the request came, and while the SSO is slow no sooner than its slowest recent
//! call plus the jitter, so that failures with and without an SSO call take as long. Every failure
//! that depends on the ID is a 401 with the same body, whatever stopped it: a wrong password, an
//! ID off the list (list mode), a limit of the ID or the account, the SSO-call limit, a single SSO
//! error. The answer does not tell whether an ID is on the list (D3); the record and the journal
//! keep the real reason. The answers of their own are 403s: an ID outside HJViewer's ID rule
//! (step 4), which the rule makes public, and a denied person (step 10) or a blocked account
//! (step 12), given only after the SSO accepted the password. The other statuses do not depend
//! on the ID: 429 for the address limit, 400 for sealing errors, 409 for a seal's time, 503 while
//! SSO calls are paused or too many logins run at once, 500 for the server's own errors. The app
//! shows the body's text whatever the status (research notes §9).
//!
//! After the SSO accepted the password, in one transaction: the allowlist entry checked, or made
//! in open mode (step 12), the account created or updated, the account key registered (keys.rs),
//! the device added to the account when it is an installed Windows PC whose signature matches the
//! key hbbs holds (D9, D10, v2 §5), and a token issued: 256 random bits, of which only the SHA-256
//! is stored, bound to the account key. A new token replaces the account's token from the same
//! device, and that token's key with it, and an account keeps at most [`MAX_TOKENS`]. The answer
//! adds `"hjv":{"ks":<the key's serial>}`, and for a PC added by the login or already in the
//! account also `"binding":"<32 hex>","login_at":<server time>` (design v3 §4-1), from which the
//! PC's service takes up the binding (§4-4). While hjv-api has a ticket key, `hjv.access` is the
//! app's access ticket (ticket.rs): `both` for an installed PC whose login signature matched the
//! key hbbs holds for it, `ctl` for any other device, and for the App Review account `review`
//! for the demo PCs of its file, or none without them.

use crate::{
    api::{is_json, user_json, ApiRequest, BodyError, Reply},
    config::{LoginMode, Window},
    db::{self, Admission, Bound, KeyAdmission, Listing, NewKey, NewToken, Record},
    devmodel, devsig, hbbs,
    keys::{KeyClaim, KeyError, MAX_ACTIVE_KEYS},
    limits::{self, Take, SCOPE_BOUND, SCOPE_ID, SCOPE_IP, SCOPE_OK_ID, SCOPE_OK_IP},
    review::{self, Review},
    seal::{Opened, SealError},
    sso::{Denial, Failure, SsoError, SsoUser},
    state::AppState,
    tfa::{Busy, Pending},
    util::{
        b64url, clean_text, hex, login_key, now, random_below, random_bytes, sha256, staff_id,
        valid_device_id,
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
use zeroize::Zeroizing;

/// The one answer to every failed login (design v2 §1-1).
pub const FAILURE_TEXT: &str =
    "로그인할 수 없습니다. ID·비밀번호를 확인하고, 계속 안 되면 잠시 뒤 다시 하거나 관리자에게 문의하세요.";
/// The answer to a seal whose time is more than ten minutes off (design v3 §4-1).
pub const TEXT_SEAL_CLOCK: &str =
    "기기의 날짜와 시간이 맞지 않습니다. 날짜와 시간을 자동으로 맞추게 한 뒤 다시 해 주세요.";
/// The answer of every login while SSO calls are paused (design v3 §1-1, §4-1).
pub const TEXT_SSO_DOWN: &str = "연세 인증 서버에 지금 연결할 수 없습니다. 잠시 뒤 다시 해 주세요.";
/// The answer to the login of an account the operator blocked, once the SSO accepted its
/// password (step 12).
pub const TEXT_BLOCKED: &str =
    "이 계정은 관리자가 HJViewer 사용을 막았습니다. 다시 쓰려면 관리자에게 문의하세요.";
/// The answer to a portal ID outside HJViewer's ID rule (util.rs `staff_id`), before any SSO
/// call (step 4).
pub const TEXT_ID_RULE: &str =
    "연세대학교 교직원의 연세포탈 ID로만 로그인할 수 있습니다. 입력한 ID를 확인해 주세요.";
/// The answer to a person the SSO accepted the password of and HJViewer denies: the SSO's user
/// call refused, named a pcode outside HJViewer's ID rule, or a pcode or name whose value the
/// contract refuses (step 10).
pub const TEXT_DENIED: &str =
    "이 계정으로는 HJViewer를 쓸 수 없습니다. 연세대학교 교직원만 쓸 수 있으며, 교직원인데도 이 문구가 나오면 관리자에게 문의하세요.";

/// The tokens an account keeps: a login past it revokes the least recently used.
pub const MAX_TOKENS: usize = 20;

/// The login request as the app sends it (flutter/lib/common/hbbs/hbbs.dart `LoginRequest`), with
/// the `hjv` part that client patch 0030 adds. Other fields (autoLogin) are ignored.
#[derive(Deserialize)]
struct LoginBody {
    username: Option<String>,
    password: Option<String>,
    id: Option<String>,
    /// the app's uuid (`bind.mainGetUuid()`): the account key's device tag is made from it
    uuid: Option<String>,
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
    /// the app's version, such as "1.6.0" (design v3 §4-1 step 7)
    ver: Option<String>,
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
    /// the device's name as it sent it (deviceInfo.name; an iPhone's model code), '' if none
    pub name: String,
    /// the app's uuid as it sent it, '' if none; only its tag is stored
    pub uuid: String,
    /// the app's version as `hjv.ver` says it, '' if none or not a version
    pub version: String,
    /// the account key of the sealed payload (keys.rs), checked after the SSO
    pub key: KeyClaim,
}

/// "1.6.0": one to four numbers joined by dots.
fn app_version(text: &str) -> String {
    let text = text.trim();
    let ok = text.len() <= 32
        && text.split('.').count() <= 4
        && text
            .split('.')
            .all(|n| !n.is_empty() && n.len() <= 6 && n.bytes().all(|b| b.is_ascii_digit()));
    if ok {
        text.to_owned()
    } else {
        String::new()
    }
}

impl DeviceClaim {
    fn new(body: &LoginBody, opened: &Opened) -> DeviceClaim {
        let info = body.device_info.as_ref();
        let os = info
            .and_then(|i| i.os.as_deref())
            .map(|os| clean_text(os, 32).to_ascii_lowercase())
            .unwrap_or_default();
        let name = info
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
            hostname: if name.is_empty() {
                id.clone()
            } else {
                name.clone()
            },
            id,
            os,
            installed,
            name,
            uuid: body
                .uuid
                .as_deref()
                .map(|u| clean_text(u, 128))
                .unwrap_or_default(),
            version: body
                .hjv
                .as_ref()
                .and_then(|h| h.ver.as_deref())
                .map(app_version)
                .unwrap_or_default(),
            key: KeyClaim::new(
                opened.ck.clone(),
                opened.cp.clone(),
                &opened.nonce,
                &opened.device_id,
            ),
        }
    }

    fn signed_by(&self, public_key: &[u8]) -> bool {
        self.signature
            .as_deref()
            .is_some_and(|sig| devsig::verify(public_key, &self.message, sig))
    }
}

/// What a sealed login claims: the portal ID as typed (trimmed), its case-folded key when it
/// passes HJViewer's ID rule (step 4), and the device.
struct Claim {
    username: String,
    key: Option<String>,
    device: DeviceClaim,
}

impl Claim {
    fn new(body: &LoginBody, opened: &Opened) -> Claim {
        let username = body.username.as_deref().unwrap_or("").trim().to_owned();
        Claim {
            key: staff_id(&username).map(|_| login_key(&username)),
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
        LoginAnswer::failed_with(status, json!({ "error": FAILURE_TEXT }))
    }

    /// A failure with a body of its own (it waits as every failure does).
    fn failed_with(status: StatusCode, body: serde_json::Value) -> LoginAnswer {
        LoginAnswer {
            status,
            body: Arc::new(body.to_string()),
            failure: true,
        }
    }

    /// 409 `clock`: the seal's time is more than HJV_API_SEAL_MAX_SKEW off (design v3 §4-1).
    /// The server's time is public, and the seal is checked before the ID is looked at, so this
    /// tells nothing about the ID or the allowlist.
    fn clock() -> LoginAnswer {
        LoginAnswer::failed_with(
            StatusCode::CONFLICT,
            json!({ "error": TEXT_SEAL_CLOCK, "hjv": "clock", "server_time": now() }),
        )
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
            sent: None,
        }
    }
}

/// Answers kept for the app's retry of the very same request (v2 §9-2 step 3): the app sends a
/// request again, byte for byte, when its first TLS attempt failed, possibly after hjv-api got
/// it. A login is told by its body, under the seal's nonce; a signed account request that changes
/// something by its proof, under the key's nonce (api.rs `authenticate`, design v3 §4-2).
pub struct RetryCache<A> {
    window: Duration,
    entries: Mutex<Entries<A>>,
}

struct Entries<A> {
    map: HashMap<String, RetryEntry<A>>,
    /// when the entries past the window last went
    swept: Instant,
}

struct RetryEntry<A> {
    /// the SHA-256 of what was sent (a login's body, an account request's `proof::request_hash`)
    sent: [u8; 32],
    created: Instant,
    answer: watch::Receiver<Option<A>>,
}

pub enum Retry<A> {
    /// the first request with this nonce: send its answer here
    First(watch::Sender<Option<A>>),
    /// the very same request again: its answer comes here
    Again(watch::Receiver<Option<A>>),
    /// the nonce with another request
    Reused,
}

impl<A> RetryCache<A> {
    pub fn new(window_secs: i64) -> RetryCache<A> {
        RetryCache {
            window: Duration::from_secs(window_secs as u64),
            entries: Mutex::new(Entries {
                map: HashMap::new(),
                swept: Instant::now(),
            }),
        }
    }

    pub fn begin(&self, key: &str, sent: [u8; 32]) -> Retry<A> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        // the entries past the window go once a second at most: an account request calls this
        // with the database's lock held, and a sweep each time would cost one look per entry
        if entries.swept.elapsed() >= Duration::from_secs(1) {
            let window = self.window;
            entries.map.retain(|_, e| e.created.elapsed() < window);
            entries.swept = Instant::now();
        }
        match entries.map.get(key) {
            Some(e) if e.created.elapsed() < self.window && e.sent == sent => {
                Retry::Again(e.answer.clone())
            }
            Some(e) if e.created.elapsed() < self.window => Retry::Reused,
            _ => {
                let (tx, rx) = watch::channel(None);
                entries.map.insert(
                    key.to_owned(),
                    RetryEntry {
                        sent,
                        created: Instant::now(),
                        answer: rx,
                    },
                );
                Retry::First(tx)
            }
        }
    }

    /// Lets a request under `key` be looked at afresh: its answer is not kept for a retry (those
    /// waiting already get it).
    pub fn forget(&self, key: &str) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.map.remove(key);
    }

    pub fn purge(&self) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let window = self.window;
        entries.map.retain(|_, e| e.created.elapsed() < window);
        entries.swept = Instant::now();
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
    /// a blocked account whose password the SSO accepted (step 12), or a person it denies (step
    /// 10): counted as a success, so that a known password cannot keep calling the SSO, and
    /// answered as a failure
    Blocked,
}

/// The limit a login's ID counts under (step 5).
struct Limit {
    scope: &'static str,
    key: String,
    windows: Vec<Window>,
    /// an ID not on the allowlist, in open mode (HJV_API_LIMIT_NEW_ID)
    unlisted: bool,
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
            Outcome::Success | Outcome::Blocked => {
                keep.extend(slots);
                back.extend(
                    ip.into_iter()
                        .chain(id)
                        .chain(self.challenge_attempt.take()),
                );
            }
        }
        let record = result
            .filter(|_| {
                matches!(outcome, Outcome::Success | Outcome::Blocked)
                    || self.state.admit_failure_record()
            })
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
        self.fail_with(LoginAnswer::failed(status), outcome, result)
            .await
    }

    async fn fail_with(
        &mut self,
        answer: LoginAnswer,
        outcome: Outcome,
        result: &'static str,
    ) -> LoginAnswer {
        self.end(outcome, Some(result)).await;
        answer
    }

    /// The same 503 for every ID while SSO calls are paused (design v3 §4-1). Nothing counts:
    /// no SSO call is made, so no password is tried, and an outage must not use up the limits
    /// of everyone behind a campus address.
    async fn sso_down(&mut self) -> LoginAnswer {
        let answer = LoginAnswer::failed_with(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({ "error": TEXT_SSO_DOWN }),
        );
        self.fail_with(answer, Outcome::Released, "sso-paused")
            .await
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
        Err(result @ "seal-time") => {
            return run
                .fail_with(LoginAnswer::clock(), Outcome::Failed, result)
                .await
        }
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
            wait_for(answer, unanswered).await
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
    // the access password's seal (guest.rs) is never a login's
    if opened.purpose.is_some() || opened.use_.is_some() {
        return Err("seal-purpose");
    }
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

/// The first request's answer, once it is there ([`Retry::Again`]); `failed()` when that request
/// ended without one.
pub async fn wait_for<A: Clone>(
    mut answer: watch::Receiver<Option<A>>,
    failed: impl FnOnce() -> A,
) -> A {
    match answer.wait_for(Option::is_some).await {
        Ok(a) => a.clone().unwrap_or_else(failed),
        Err(_) => failed(),
    }
}

/// What a retry gets when the login it repeats ended without an answer.
fn unanswered() -> LoginAnswer {
    LoginAnswer::failed(StatusCode::INTERNAL_SERVER_ERROR)
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
        unlisted: false,
    })
}

/// Where the ID's entry stands with the allowlist; `Err` after the journal heard of the error.
async fn listing_of(state: &AppState, key: &str) -> Result<Listing, ()> {
    let key = key.to_owned();
    state
        .db
        .call(move |c| db::listing(c, &key))
        .await
        .map_err(|e| log_err!("login: {e}"))
}

/// Whether a login counts under the limits of the IDs not on the allowlist (steps 5 and 8): in
/// open mode only, as in list mode such an ID never reaches the SSO.
fn unlisted(state: &AppState, listing: Listing) -> bool {
    state.config.login == LoginMode::Open && listing == Listing::Absent
}

async fn sealed_login(
    run: &mut Run<'_>,
    opened: Opened,
    claim: Claim,
    bound: Option<Limit>,
) -> LoginAnswer {
    let state = run.state;
    // 4. the portal ID: one outside HJViewer's ID rule goes no further, before its tag is taken,
    // with an answer of its own; but for the App Review account's while its file names it
    // (review.rs), which goes on without the SSO
    if !claim.device.id.is_empty() {
        run.device_tag = Some(state.tags.device(&claim.device.id));
    }
    let review = if claim.key.is_none() {
        review::named(state, &claim.username).await
    } else {
        None
    };
    let Some(key) = claim
        .key
        .clone()
        .or_else(|| review.as_ref().map(|_| login_key(&claim.username)))
    else {
        let answer =
            LoginAnswer::failed_with(StatusCode::FORBIDDEN, json!({ "error": TEXT_ID_RULE }));
        return run.fail_with(answer, Outcome::Failed, "id-rule").await;
    };
    run.account_tag = Some(state.tags.account(&key));

    // the ID's allowlist entry, for the limits of steps 5 and 8 (an ID not on the list counts
    // under lower ones in open mode); what it says shows no sooner than step 7
    let Ok(listing) = listing_of(state, &key).await else {
        return run
            .fail(
                StatusCode::INTERNAL_SERVER_ERROR,
                Outcome::Released,
                "error",
            )
            .await;
    };
    let unlisted = unlisted(state, listing);

    // 5. failures per ID
    let limit = match bound {
        Some(limit) => limit,
        None => match bound_limit(state, &claim).await {
            Some(limit) => limit,
            None => Limit {
                scope: SCOPE_ID,
                key: state.tags.account(&key),
                windows: if unlisted {
                    state.config.limit_new_id.clone()
                } else {
                    state.config.limit_id.clone()
                },
                unlisted,
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
    if let Some(review) = review {
        return review_login(run, &key, listing, opened.password, claim.device, review).await;
    }

    // 6. the SSO's trouble shows before the allowlist does: the same 503 for every ID
    if state.sso_breaker.paused() {
        return run.sso_down().await;
    }

    // 7. the allowlist: in list mode an ID off it does not reach the SSO
    let allowed = listing != Listing::Absent || state.config.login == LoginMode::Open;

    let Claim {
        username, device, ..
    } = claim;
    // 9. the second factor comes before the allowlist shows: every ID gets the code dialog
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
    sso_login(run, &username, &key, &opened.password, device, unlisted).await
}

/// A login refused by a full ID limit: counted against the address only; the journal hears of
/// it once per window.
async fn id_limited(run: &mut Run<'_>, limit: &Limit) -> LoginAnswer {
    let state = run.state;
    let (result, from) = if limit.scope == SCOPE_BOUND {
        ("limited-bound", "from a PC in the account")
    } else if limit.unlisted {
        ("limited-id", "an ID not on the allowlist")
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
            return wait_for(answer, unanswered).await;
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
    // 5. the ID limit of the first step (an ID not on the allowlist under its own in open mode)
    let Ok(listing) = listing_of(state, &pending.login_key).await else {
        return run
            .fail(
                StatusCode::INTERNAL_SERVER_ERROR,
                Outcome::Released,
                "error",
            )
            .await;
    };
    let unlisted = pending.limit_scope != SCOPE_BOUND && unlisted(state, listing);
    let limit = Limit {
        scope: pending.limit_scope,
        key: pending.limit_key.clone(),
        windows: if pending.limit_scope == SCOPE_BOUND {
            state.config.limit_bound.clone()
        } else if unlisted {
            state.config.limit_new_id.clone()
        } else {
            state.config.limit_id.clone()
        },
        unlisted,
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
    // 6.: while SSO calls are paused, the same 503 for every ID, before the allowlist shows
    if state.sso_breaker.paused() {
        return run.sso_down().await;
    }
    // 7. and 9.: off the list or a wrong code, the same failure
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
        unlisted,
    )
    .await
}

/// 8. successes per account and per address; the answer to send when one is full.
async fn success_limits(run: &mut Run<'_>) -> Result<(), LoginAnswer> {
    let state = run.state;
    let account_tag = run.account_tag.clone().unwrap_or_default();
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
                return Err(run
                    .fail(StatusCode::UNAUTHORIZED, Outcome::Refused, "limited-ok")
                    .await);
            }
            Err(()) => {
                return Err(run
                    .fail(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Outcome::Released,
                        "error",
                    )
                    .await)
            }
        }
    }
    Ok(())
}

async fn sso_login(
    run: &mut Run<'_>,
    username: &str,
    key: &str,
    password: &str,
    device: DeviceClaim,
    unlisted: bool,
) -> LoginAnswer {
    let state = run.state;
    let account_tag = run.account_tag.clone().unwrap_or_default();
    // 8. successes per account and per address
    if let Err(answer) = success_limits(run).await {
        return answer;
    }
    // the SSO calls were paused meanwhile (limits::Breaker)
    if state.sso_breaker.paused() {
        return run.sso_down().await;
    }
    // in open mode the IDs not on the allowlist first take one of their own few calls, so that
    // trying passwords on many portal IDs cannot use up the calls of the people registered; the
    // ID and the client address hold it, each at most a quarter of them, so that one address
    // with a few wrong passwords cannot shut the first logins of everyone
    let ip_tag = run.ip_tag.clone();
    let new_holders = [account_tag.as_str(), ip_tag.as_str()];
    if unlisted {
        let refused = match state.sso_calls_new.try_take(&new_holders) {
            Take::Taken => false,
            Take::Full => {
                if state.first_notice("sso-calls-new", Duration::from_secs(60)) {
                    log_warn!(
                        "login: SSO call limit of the portal IDs not on the allowlist reached (HJV_API_LIMIT_SSO_NEW); their first logins fail meanwhile"
                    );
                }
                true
            }
            Take::Share(0) => {
                if state.first_notice(
                    &format!("sso-share-new:{account_tag}"),
                    Duration::from_secs(60),
                ) {
                    log_warn!(
                        "login: a portal ID not on the allowlist holds its share of their SSO calls (acct={account_tag}); hjv-api admin unlock tag:{account_tag} lifts it"
                    );
                }
                true
            }
            Take::Share(_) => {
                if state.first_notice(&format!("sso-share-new:{ip_tag}"), Duration::from_secs(60)) {
                    log_warn!(
                        "login: an address holds its share of the SSO calls of the portal IDs not on the allowlist (ip={ip_tag}); hjv-api admin unlock tag:{ip_tag} lifts it"
                    );
                }
                true
            }
        };
        if refused {
            return run
                .fail(
                    StatusCode::UNAUTHORIZED,
                    Outcome::Refused,
                    "limited-sso-new",
                )
                .await;
        }
    }
    // the SSO calls of all accounts, and this account's share of them
    match state.sso_calls.try_take(&[account_tag.as_str()]) {
        Take::Taken => {}
        Take::Full => {
            if state.first_notice("sso-calls", Duration::from_secs(60)) {
                log_warn!("login: SSO call limit reached");
            }
            // no call is made: a first login's call of the IDs not on the allowlist goes back
            if unlisted {
                state.sso_calls_new.give_back(&new_holders);
            }
            return run
                .fail(StatusCode::UNAUTHORIZED, Outcome::Refused, "limited-sso")
                .await;
        }
        Take::Share(_) => {
            if state.first_notice(&format!("sso-share:{account_tag}"), Duration::from_secs(60)) {
                log_warn!(
                    "login: one account's share of the SSO calls reached (acct={account_tag})"
                );
            }
            if unlisted {
                state.sso_calls_new.give_back(&new_holders);
            }
            return run
                .fail(StatusCode::UNAUTHORIZED, Outcome::Refused, "limited-sso")
                .await;
        }
    }
    // 10. the SSO
    let started = Instant::now();
    let result = state.sso.authenticate(username, password).await;
    state.sso_latency.note(started.elapsed());
    let failure = result.as_ref().err().map(|e| e.failure);
    if let Some(pause) = state.sso_breaker.note(failure) {
        log_err!(
            "login: SSO calls paused for {} s; logins fail meanwhile without calling it",
            pause.as_secs()
        );
    }
    if let Some(accounts) = state.sso_user_refusals.note(failure, &account_tag) {
        log_crit!(
            "login: the SSO's user call refused {accounts} different accounts in a row (access denied) and answered none between; if it refuses everyone, no one can log in until HJV_API_SSO_CLIENT_ID and the credential hjviewer-sso-api-uuid are right"
        );
    }
    match result {
        Ok(user) => {
            let answer = success(run, key, user, device).await;
            // a first login that went through gives its call back: the window keeps the calls
            // that logged no one in, so that people logging in for the first time one after
            // another behind one address (a campus NAT) do not use up its share
            if unlisted && !answer.failure {
                state.sso_calls_new.give_back(&new_holders);
            }
            answer
        }
        Err(e) => {
            note_sso_message(state, &e, username);
            match e.failure {
                Failure::Credentials => {
                    run.fail(StatusCode::UNAUTHORIZED, Outcome::Failed, "credentials")
                        .await
                }
                Failure::Denied(denial) => denied(run, &e, denial).await,
                Failure::AddressRefused => {
                    log_crit!(
                        "login: the SSO refuses this server's address ({}); no one can log in until it is registered again",
                        e.detail
                    );
                    run.fail(StatusCode::UNAUTHORIZED, Outcome::Refused, "sso-address")
                        .await
                }
                Failure::ClientRefused => {
                    log_crit!(
                        "login: the SSO refuses this server's client credentials ({}); no one can log in until the credential hjviewer-sso-client-secret is right",
                        e.detail
                    );
                    run.fail(StatusCode::UNAUTHORIZED, Outcome::Refused, "sso-client")
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

/// The App Review account's login (review.rs) after step 5, with nothing of the SSO's (its pause,
/// the second factor, its calls): steps 7 and 8, its file's hash in place of step 10 (a wrong
/// password failing as one the SSO refused), then the rest as for anyone, into no account but one
/// of its own.
async fn review_login(
    run: &mut Run<'_>,
    key: &str,
    listing: Listing,
    password: Zeroizing<String>,
    device: DeviceClaim,
    review: Arc<Review>,
) -> LoginAnswer {
    let state = run.state;
    let tag = run.account_tag.clone().unwrap_or_default();
    // 7. in list mode only once the operator imported its ID
    if listing == Listing::Absent && state.config.login == LoginMode::List {
        return run
            .fail(StatusCode::UNAUTHORIZED, Outcome::Failed, "not-allowed")
            .await;
    }
    // 8. before the hash is worked out, as before an SSO call
    if let Err(answer) = success_limits(run).await {
        return answer;
    }
    match tokio::task::spawn_blocking(move || review.verify(&password)).await {
        Ok(true) => {}
        Ok(false) => {
            log_notice!("login: a wrong password for the App Review account (acct={tag})");
            return run
                .fail(
                    StatusCode::UNAUTHORIZED,
                    Outcome::Failed,
                    "review-credentials",
                )
                .await;
        }
        Err(e) => {
            log_err!("login: the App Review account's password check: {e}");
            return run
                .fail(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Outcome::Released,
                    "error",
                )
                .await;
        }
    }
    // an account of its ID that this login did not make (another name or pcode, or a PC: one of
    // before the ID rule) is not its own. Checked outside the transaction: only this login makes
    // accounts of IDs outside the rule, and it binds no PC
    let own = {
        let key = key.to_owned();
        state
            .db
            .call(move |c| match db::account_by_key(c, &key)? {
                None => Ok(true),
                Some(a) => Ok(a.portal_id == key
                    && a.display_name == review::NAME
                    && db::devices_of(c, a.id)?.is_empty()),
            })
            .await
    };
    match own {
        Ok(true) => {}
        Ok(false) => {
            log_crit!(
                "login: the App Review file names the ID of an account that is not the App Review account's (acct={tag}); its logins fail until the file names another ID"
            );
            return run
                .fail(StatusCode::UNAUTHORIZED, Outcome::Failed, "review-existing")
                .await;
        }
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
    let user = SsoUser {
        pcode: key.to_owned(),
        name: review::NAME.to_owned(),
    };
    let answer = success(run, key, user, device).await;
    if !answer.failure {
        log_notice!("login: the App Review account logged in, without the SSO (acct={tag})");
    }
    answer
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

/// What the hbbs check found about the login's device (D9, D10, v2 §5), before anything is
/// written.
#[derive(Debug, Clone, Copy)]
enum DeviceCheck {
    /// nothing to put into the account: no device, not one the list takes, or not verified
    Skip(BindStatus),
    /// the key hbbs holds for the device, which signed this login
    Verified([u8; 32]),
}

/// What the login's transaction wrote.
enum Written {
    Done {
        status: BindStatus,
        binding: Option<db::Binding>,
        key_serial: i64,
        /// keys logged out to keep the account's MAX_ACTIVE_KEYS
        evicted: usize,
        /// the login put its portal ID on the allowlist (step 12, open mode)
        registered: bool,
    },
    KeyInUse,
    /// the allowlist refused it in the transaction (step 12): blocked, or not on the list in
    /// list mode
    Blocked,
    NotListed,
}

/// The answer to the login of a blocked account, once the SSO accepted its password (step 12):
/// 403 with [`TEXT_BLOCKED`]. The journal hears of it once in ten minutes per account.
async fn blocked(run: &mut Run<'_>) -> LoginAnswer {
    let state = run.state;
    let tag = run.account_tag.clone().unwrap_or_default();
    if state.first_notice(&format!("blocked:{tag}"), Duration::from_secs(600)) {
        log_notice!(
            "login: an account the operator blocked logged in with the right password and was refused (acct={tag}); hjv-api admin unblock tag:{tag} lets it in"
        );
    }
    let answer = LoginAnswer::failed_with(
        StatusCode::FORBIDDEN,
        json!({ "error": TEXT_BLOCKED, "hjv": "blocked" }),
    );
    run.fail_with(answer, Outcome::Blocked, "blocked").await
}

/// The answer to a person the SSO accepted the password of and HJViewer denies (step 10): 403
/// with [`TEXT_DENIED`] whatever the reason, counted as a blocked account's login is; the record
/// keeps the reason (`denied-user-refused`, `denied-pcode-rule`, `denied-bad-value`). The journal
/// hears of it once in ten minutes per account, with the reason.
async fn denied(run: &mut Run<'_>, e: &SsoError, denial: Denial) -> LoginAnswer {
    let state = run.state;
    let tag = run.account_tag.clone().unwrap_or_default();
    if state.first_notice(&format!("denied:{tag}"), Duration::from_secs(600)) {
        log_notice!(
            "login: the SSO accepted the password, but the person may not use HJViewer ({}; acct={tag})",
            e.detail
        );
    }
    let answer = LoginAnswer::failed_with(StatusCode::FORBIDDEN, json!({ "error": TEXT_DENIED }));
    run.fail_with(answer, Outcome::Blocked, denial.result())
        .await
}

async fn success(run: &mut Run<'_>, key: &str, user: SsoUser, device: DeviceClaim) -> LoginAnswer {
    let state = run.state;
    let account_tag = run.account_tag.clone().unwrap_or_default();
    if login_key(&user.pcode) != key {
        log_notice!(
            "login: the portal ID typed and the pcode the SSO names differ (acct={account_tag})"
        );
    }
    // 11. the account key the seal carried (design v3 §4-1 steps 1-2)
    let public_key = match device.key.verify() {
        Ok(public_key) => public_key,
        Err(e) => {
            let result = match e {
                KeyError::Missing => "key-missing",
                KeyError::Bad => "key-bad",
            };
            return run
                .fail(StatusCode::BAD_REQUEST, Outcome::Refused, result)
                .await;
        }
    };
    // 12. the allowlist once more: a blocked account goes no further (the ID typed or the one
    // the SSO names), nor (list mode) one whose entry went since step 7; the transaction below
    // asks again
    let open = state.config.login == LoginMode::Open;
    let pcode_key = login_key(&user.pcode);
    let listing = {
        let (key, pcode_key) = (key.to_owned(), pcode_key.clone());
        state
            .db
            .call(move |c| db::login_listing(c, &key, &pcode_key))
            .await
    };
    match listing {
        Ok(Listing::Registered) => {}
        Ok(Listing::Absent) if open => {}
        Ok(Listing::Blocked) => return blocked(run).await,
        Ok(Listing::Absent) => {
            return run
                .fail(StatusCode::UNAUTHORIZED, Outcome::Failed, "not-allowed")
                .await
        }
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
    let check = check_device(state, key, &device, &account_tag).await;
    let login_key = key.to_owned();
    let token = format!("hjv_{}", b64url(&random_bytes::<32>()));
    let hash = sha256(token.as_bytes());
    let at = now();
    // the server's own view of the login (step 7): no address is kept, only on or off campus
    let platform = devmodel::platform(&device.os);
    let model = devmodel::model_name(platform, &device.name);
    let login_net = state.config.login_net(&run.ip);
    let dev_tag = if device.uuid.is_empty() {
        String::new()
    } else {
        state.tags.tag("uuid", &device.uuid)
    };
    // the answer's user object, with the account's role when it has one (overview.rs)
    let user_object = crate::overview::with_role(state, key, user_json(&user.pcode, &user.name));
    let (key, user_db) = (key.to_owned(), user.clone());
    let (id, name, version, os, hostname) = (
        device.id.clone(),
        device.name.clone(),
        device.version.clone(),
        device.os.clone(),
        device.hostname.clone(),
    );
    let written = state
        .db
        .call(move |c| {
            let tx = c.transaction()?;
            // step 12 within the transaction: nothing is written for a refused login
            let registered = match db::admit_login(&tx, &key, &pcode_key, open, at)? {
                Admission::Listed => false,
                Admission::Registered => true,
                Admission::Blocked => return Ok(Written::Blocked),
                Admission::Refused => return Ok(Written::NotListed),
            };
            let account_id = db::upsert_account(&tx, &key, &user_db.pcode, &user_db.name, at)?;
            let new_key = NewKey {
                account_id,
                public_key: &public_key,
                dev_tag: &dev_tag,
                ctl_id: &id,
                platform,
                model,
                app_name: &name,
                app_ver: &version,
                login_net,
            };
            // steps 3-5: a key no row has, the active keys of the account, the same device's
            // older keys, the change serial (a refusal leaves everything as it was)
            let (ckid, key_serial, evicted) = match db::add_key(&tx, &new_key, at, MAX_ACTIVE_KEYS)?
            {
                KeyAdmission::Added {
                    ckid,
                    key_serial,
                    evicted,
                    ..
                } => (ckid, key_serial, evicted),
                KeyAdmission::InUse => return Ok(Written::KeyInUse),
            };
            let (status, verified, binding) = match check {
                DeviceCheck::Skip(status) => (status, false, None),
                DeviceCheck::Verified(device_key) => {
                    let binding = db::bind(&tx, account_id, &id, &device_key, &os, &hostname, at)?;
                    let status = match binding.bound {
                        Bound::Added => BindStatus::Added,
                        Bound::Already => BindStatus::Already,
                        Bound::OtherAccount => BindStatus::OtherAccount,
                    };
                    (status, true, Some(binding))
                }
            };
            // step 6: the token belongs to the key
            let new_token = NewToken {
                hash: &hash,
                account_id,
                device_id: &id,
                device_verified: verified,
                bind_status: status.as_str(),
                ckid: Some(ckid),
            };
            db::insert_token_tx(&tx, &new_token, at, MAX_TOKENS)?;
            tx.commit()?;
            Ok(Written::Done {
                status,
                binding,
                key_serial,
                evicted,
                registered,
            })
        })
        .await;
    let (status, binding, key_serial) = match written {
        Ok(Written::Done {
            status,
            binding,
            key_serial,
            evicted,
            registered,
        }) => {
            if registered {
                log_notice!(
                    "login: a portal ID that was not on the allowlist logged in and was registered (acct={account_tag})"
                );
            }
            if evicted > 0
                && state.first_notice(
                    &format!("keys-full:{account_tag}"),
                    Duration::from_secs(600),
                )
            {
                log_notice!(
                    "login: the account had {MAX_ACTIVE_KEYS} active keys; the least recently used one was logged out (acct={account_tag}); hjv-api admin key list tag:{account_tag}"
                );
            }
            (status, binding, key_serial)
        }
        Ok(Written::KeyInUse) => {
            return run
                .fail(StatusCode::BAD_REQUEST, Outcome::Refused, "key-in-use")
                .await
        }
        Ok(Written::Blocked) => return blocked(run).await,
        Ok(Written::NotListed) => {
            return run
                .fail(StatusCode::UNAUTHORIZED, Outcome::Failed, "not-allowed")
                .await
        }
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
    };
    let device_record = |result: &'static str| {
        state.record(
            "device",
            Some(account_tag.clone()),
            Some(state.tags.device(&device.id)),
            None,
            result,
        )
    };
    match status {
        BindStatus::Added => device_record("added").await,
        BindStatus::OtherAccount => {
            log_warn!(
                "device check: the device is in another account (dev={}, acct={account_tag}); admin: hjv-api admin binding revoke tag:{}",
                state.tags.device(&device.id),
                state.tags.device(&device.id)
            );
            device_record("conflict").await;
        }
        _ => {}
    }
    run.end(Outcome::Success, Some("ok")).await;
    let mut hjv = json!({ "ks": key_serial });
    if let Some(db::Binding {
        bound: Bound::Added | Bound::Already,
        binding_id: Some(binding_id),
        login_at: Some(login_at),
    }) = binding
    {
        hjv["binding"] = json!(hex(&binding_id));
        hjv["login_at"] = json!(login_at);
    }
    // the app's access ticket (ticket.rs): `both` for the PC whose login signature matched the
    // key hbbs holds for it, else `ctl`
    let pc = match check {
        DeviceCheck::Verified(pk) => Some((device.id.as_str(), pk)),
        DeviceCheck::Skip(_) => None,
    };
    if let Ok(issued) = crate::ticket::for_account(state, &login_key, pc, "login", at).await {
        hjv["access"] = issued.json(now());
    }
    LoginAnswer::ok(json!({
        "type": "access_token",
        "access_token": token,
        "user": user_object,
        "hjv": hjv,
    }))
}

/// Checks the login's device for the list when it qualifies (D9, D10, v2 §5): an installed
/// device of HJV_API_BIND_OS whose login signature matches the key hbbs holds for its ID, in an
/// account other than the App Review account (review.rs), which never holds a PC. Writes nothing
/// but the records of what went wrong.
async fn check_device(
    state: &Arc<AppState>,
    key: &str,
    device: &DeviceClaim,
    account_tag: &str,
) -> DeviceCheck {
    if device.id.is_empty() {
        return DeviceCheck::Skip(BindStatus::None);
    }
    if device.signature.is_none()
        || !device.installed
        || !state.config.bind_os.contains(&device.os)
        || review::is_review(key)
    {
        return DeviceCheck::Skip(BindStatus::NotEligible);
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
        return DeviceCheck::Skip(BindStatus::CheckFailed);
    };
    let lookup = hbbs::lookup(state.config.hbbs_console, Some(&server_key), &device.id).await;
    let public_key = match lookup {
        Ok(hbbs::Lookup::Known { public_key, .. }) => public_key,
        Ok(hbbs::Lookup::Unknown) => {
            record("unknown-to-hbbs").await;
            return DeviceCheck::Skip(BindStatus::CheckFailed);
        }
        Err(e @ hbbs::HbbsError::BadSignature) => {
            log_err!("device check: {e} (dev={device_tag})");
            record("hbbs-bad-signature").await;
            return DeviceCheck::Skip(BindStatus::CheckFailed);
        }
        Err(e) => {
            log_warn!("device check: {e}");
            record("hbbs-error").await;
            return DeviceCheck::Skip(BindStatus::CheckFailed);
        }
    };
    if !device.signed_by(&public_key) {
        log_warn!("device check: the signature does not match hbbs's key (dev={device_tag})");
        record("bad-signature").await;
        return DeviceCheck::Skip(BindStatus::CheckFailed);
    }
    DeviceCheck::Verified(public_key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_the_id_in_sso_messages() {
        assert_eq!(
            mask_id("user YZTEST02 locked", "yztest02"),
            "user *** locked"
        );
        assert_eq!(mask_id("yztest02YZTEST02", "YZTEST02"), "******");
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

    #[tokio::test]
    async fn a_retry_waits_for_the_first_answer() {
        let cache = RetryCache::<u8>::new(30);
        let Retry::First(tx) = cache.begin("n", [1; 32]) else {
            panic!("first")
        };
        let Retry::Again(rx) = cache.begin("n", [1; 32]) else {
            panic!("again")
        };
        // the first request still runs: the retry waits for its answer
        let waiting = tokio::spawn(wait_for(rx, || 0));
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());
        tx.send(Some(7)).unwrap();
        assert_eq!(waiting.await.unwrap(), 7);
        // and later ones get it at once, also when the first request is gone
        drop(tx);
        let Retry::Again(rx) = cache.begin("n", [1; 32]) else {
            panic!("again")
        };
        assert_eq!(wait_for(rx, || 0).await, 7);
        // a first request that ended without an answer
        let Retry::First(tx) = cache.begin("m", [1; 32]) else {
            panic!("first")
        };
        drop(tx);
        let Retry::Again(rx) = cache.begin("m", [1; 32]) else {
            panic!("again")
        };
        assert_eq!(wait_for(rx, || 0).await, 0);
        // past the window the cache no longer knows the nonce (the database still refuses it)
        let none = RetryCache::<u8>::new(0);
        assert!(matches!(none.begin("n", [1; 32]), Retry::First(_)));
        assert!(matches!(none.begin("n", [1; 32]), Retry::First(_)));
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

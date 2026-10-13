// SPDX-License-Identifier: AGPL-3.0-only

//! The relay loop (PROTOCOL.md, "States" and "Timing"). Without a known registration it sends
//! hello; then it long-polls: the server holds each poll up to 20 seconds and answers at once when
//! a command is queued. Results go back in the next poll, which follows immediately. Errors back
//! off from 5 to 300 seconds with jitter. A pending relay asks again every 30 seconds and a revoked
//! one every hour, unless the server says otherwise (`poll_after`).
//!
//! Everything that touches the system goes through [`Env`], so tests can run the loop against a
//! test server with fake interfaces and record what would be sent.
//!
//! Responses other than the commands are not signed, and whoever answers in the server's name
//! (PROTOCOL.md, "Security notes") chooses their text. The relay therefore never writes text from
//! a response to its log or status file, only short codes (`unknown_relay`, a state name) and
//! numbers; everything else it says about a bad response is its own words.

use crate::{
    clock::{system_now, ServerClock},
    command::{self, JobMemory, RateLimiter, Refusal},
    config::{self, CommandKey, VERSION},
    envelope::{self, Op},
    https::{Client, HttpError},
    identity::Identity,
    log, logln,
    netif::{self, Gateway, Iface},
    protocol::{
        CommandResult, ErrorBody, HelloPayload, HelloResponse, IfaceReport, PollPayload,
        PollResponse, PAYLOAD_VERSION,
    },
    system::{ExeWatch, StatusFile, StopMarker},
    wake::{self, SendOutcome, SendPlan},
};
use serde::Serialize;
use std::{
    collections::VecDeque,
    fmt, io,
    time::{Duration, Instant},
};

/// Results reported per poll, and kept at most while the server cannot be reached.
const MAX_RESULTS_PER_POLL: usize = 50;
const MAX_RESULTS_KEPT: usize = 100;
/// Commands handled per poll response; the server sends the rest again.
const MAX_COMMANDS_PER_RESPONSE: usize = 16;
/// The longest `poll_after` or `retry_after` the relay honours.
const MAX_SERVER_DELAY: u64 = 3600;
/// Polls that deliver results before the relay exits (a new executable or the stop marker).
const FINAL_POLLS: usize = 2;
/// The longest error text kept in the status file.
const MAX_STATUS_ERROR: usize = 200;

/// What the loop needs from the system.
pub trait Env {
    fn interfaces(&mut self) -> io::Result<Vec<Iface>>;
    fn gateways(&mut self) -> Vec<Gateway>;
    fn send(&mut self, plan: &SendPlan) -> SendOutcome;
    /// Why the relay's executable (or the file it watches instead, `run --watch`) changed, if it
    /// did.
    fn exe_changed(&mut self) -> Option<String>;
    /// Whether the stop marker is there (`run --stop`): the relay ends, and the boot-up task's
    /// loop ends on the marker too.
    fn stop_requested(&mut self) -> bool {
        false
    }
    /// Housekeeping between requests: the kept log loses its old lines once a day.
    fn tidy(&mut self) {}
    fn sleep(&mut self, d: Duration);
    /// The status JSON; `important` when something changed (always written), otherwise a
    /// heartbeat (written only where that is cheap).
    fn write_status(&mut self, _json: &str, _important: bool) {}
}

/// The real system.
pub struct SystemEnv {
    pub exe: Option<ExeWatch>,
    pub stop: Option<StopMarker>,
    pub status: Option<StatusFile>,
}

impl Env for SystemEnv {
    fn interfaces(&mut self) -> io::Result<Vec<Iface>> {
        netif::list()
    }

    fn gateways(&mut self) -> Vec<Gateway> {
        netif::system_gateways()
    }

    fn send(&mut self, plan: &SendPlan) -> SendOutcome {
        wake::send(plan)
    }

    fn exe_changed(&mut self) -> Option<String> {
        self.exe.as_ref().and_then(ExeWatch::changed)
    }

    fn stop_requested(&mut self) -> bool {
        self.stop.as_ref().is_some_and(StopMarker::present)
    }

    fn tidy(&mut self) {
        log::tidy_if_due();
    }

    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }

    fn write_status(&mut self, json: &str, important: bool) {
        if let Some(file) = &mut self.status {
            if important || file.frequent() {
                file.write(json);
            }
        }
    }
}

/// The loop's timings (the defaults are the protocol's; tests shorten them).
#[derive(Clone, Debug)]
pub struct Timing {
    /// How long the server may hold a poll (`wait`), in seconds; at most 20.
    pub wait: u32,
    /// The whole request, connect to last byte.
    pub request_timeout: Duration,
    pub pending_poll: Duration,
    pub revoked_poll: Duration,
    /// The least time between the starts of two polls, unless results are waiting.
    pub min_gap: Duration,
    pub backoff_first: Duration,
    pub backoff_max: Duration,
    /// Connection errors are logged at most once per this period.
    pub error_log_every: Duration,
    /// Long waits are cut into slices this long to notice a replaced executable or the stop
    /// marker.
    pub sleep_slice: Duration,
}

impl Default for Timing {
    fn default() -> Timing {
        Timing {
            wait: 20,
            request_timeout: Duration::from_secs(35),
            pending_poll: Duration::from_secs(30),
            revoked_poll: Duration::from_secs(3600),
            min_gap: Duration::from_secs(2),
            backoff_first: Duration::from_secs(5),
            backoff_max: Duration::from_secs(300),
            error_log_every: Duration::from_secs(600),
            sleep_slice: Duration::from_secs(15),
        }
    }
}

pub struct RelayConfig {
    pub identity: Identity,
    pub keys: Vec<CommandKey>,
    pub client: Client,
    pub os: String,
    pub kernel: String,
    pub timing: Timing,
}

/// The relay's registration as the server last described it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Not known yet, or the server does not know the relay: send hello.
    Unknown,
    Pending,
    Active,
    Revoked,
}

impl State {
    fn parse(s: &str) -> Option<State> {
        match s {
            "pending" => Some(State::Pending),
            "active" => Some(State::Active),
            "revoked" => Some(State::Revoked),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            State::Unknown => "unknown",
            State::Pending => "pending",
            State::Active => "active",
            State::Revoked => "revoked",
        }
    }
}

/// Why the loop ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Exit {
    /// The executable was replaced: exit 0 and let the boot-up task's loop start the new one.
    ExeChanged(String),
    /// The stop marker appeared: exit 0; the boot-up task's loop sees the marker and ends too.
    Stopped,
}

#[derive(Debug)]
enum Failure {
    Local(String),
    Http(HttpError),
    Server {
        status: u16,
        /// The response's `error` as sent; shown only when it [is a code](is_code).
        error: Option<String>,
        retry_after: Option<u64>,
    },
    /// The relay's own description of what was wrong with a 200 response.
    BadResponse(String),
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::Local(e) => write!(f, "{e}"),
            Failure::Http(e) => write!(f, "{e}"),
            Failure::Server { status, error, .. } => {
                match error.as_deref().filter(|e| is_code(e)) {
                    Some(e) => write!(f, "HTTP {status} {e}"),
                    None => write!(f, "HTTP {status}"),
                }
            }
            Failure::BadResponse(e) => write!(f, "bad response: {e}"),
        }
    }
}

/// Whether a string from the server is a short code (`unknown_relay`, `active`) that the relay
/// may log or keep in its status file. It shows no other text the server chose.
pub fn is_code(s: &str) -> bool {
    (1..=32).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// A 200 response body that is not the expected JSON, described without repeating any of it
/// (serde_json's messages quote the values they did not expect).
fn bad_json(e: &serde_json::Error) -> Failure {
    use serde_json::error::Category;
    let what = match e.classify() {
        Category::Io => "unreadable",
        Category::Syntax | Category::Eof => "not JSON",
        Category::Data => "not the expected JSON",
    };
    // a missing field is named by this relay's own types, so it may be shown
    let message = e.to_string();
    let missing = message
        .strip_prefix("missing field `")
        .and_then(|rest| rest.split('`').next())
        .filter(|field| is_code(field))
        .map(|field| format!(", no {field}"))
        .unwrap_or_default();
    Failure::BadResponse(format!(
        "{what}{missing} (line {} column {})",
        e.line(),
        e.column()
    ))
}

/// A `state` the relay does not know.
fn bad_state(state: &str) -> Failure {
    Failure::BadResponse(if is_code(state) {
        format!("unknown state {state}")
    } else {
        "unknown state".to_owned()
    })
}

struct Backoff {
    first: Duration,
    max: Duration,
    next: Duration,
}

impl Backoff {
    fn new(first: Duration, max: Duration) -> Backoff {
        Backoff {
            first,
            max,
            next: first,
        }
    }

    /// 5, 10, 20, ... 300 s, each within ±20%.
    fn next(&mut self) -> Duration {
        let base = self.next;
        self.next = (self.next * 2).min(self.max);
        let mut r = [0u8; 2];
        let unit = match getrandom::getrandom(&mut r) {
            Ok(()) => f64::from(u16::from_le_bytes(r)) / f64::from(u16::MAX),
            Err(_) => 0.5,
        };
        base.mul_f64(0.8 + 0.4 * unit)
    }

    fn reset(&mut self) {
        self.next = self.first;
    }
}

/// Connection errors, logged when they start, at most once per period while they last, and
/// when they end.
struct ErrorLog {
    every: Duration,
    streak: u32,
    logged_at: Option<Instant>,
    unlogged: u32,
}

impl ErrorLog {
    fn failed(&mut self, what: &str, retry_in: Duration) {
        self.streak += 1;
        match self.logged_at {
            None => {
                logln!(
                    "서버와 통신하지 못했습니다: {what} ({}초 뒤 다시 시도)",
                    retry_in.as_secs()
                );
                self.logged_at = Some(Instant::now());
                self.unlogged = 0;
            }
            Some(at) if at.elapsed() >= self.every => {
                logln!(
                    "서버와 통신하지 못하는 상태가 이어집니다: 지난 기록 뒤 {}번 더 실패, 마지막 오류: {what}",
                    self.unlogged + 1
                );
                self.logged_at = Some(Instant::now());
                self.unlogged = 0;
            }
            Some(_) => self.unlogged += 1,
        }
    }

    fn ok(&mut self) {
        if self.streak > 0 {
            logln!("서버와 다시 통신합니다 (연속 실패 {}번 뒤).", self.streak);
        }
        self.streak = 0;
        self.logged_at = None;
        self.unlogged = 0;
    }
}

#[derive(Clone, Debug, Default, Serialize)]
struct LastCommand {
    at: i64,
    job: Option<String>,
    iface: Option<String>,
    packets: u32,
    error: Option<&'static str>,
}

/// What `hjv-relay status` reads (local Unix times).
#[derive(Clone, Debug, Default, Serialize)]
struct Status {
    pid: u32,
    version: &'static str,
    id: String,
    server: String,
    state: &'static str,
    state_since: i64,
    last_ok: Option<i64>,
    last_error: Option<String>,
    last_error_at: Option<i64>,
    failures: u32,
    last_command: Option<LastCommand>,
    updated: i64,
}

pub struct Relay<E: Env> {
    cfg: RelayConfig,
    env: E,
    state: State,
    clock: ServerClock,
    jobs: JobMemory,
    limiter: RateLimiter,
    results: VecDeque<CommandResult>,
    sent_ifaces_hash: Option<String>,
    send_ifaces: bool,
    backoff: Backoff,
    errors: ErrorLog,
    noted_versions: (Option<String>, Option<String>),
    device_bind_warned: bool,
    /// Whether the last step sent packets: its results go back at once.
    sent_packets: bool,
    /// Polling only to deliver results before exiting: `wait: 0`.
    finishing: bool,
    refusal_log: RefusalLog,
    status: Status,
}

impl<E: Env> Relay<E> {
    pub fn new(cfg: RelayConfig, env: E) -> Relay<E> {
        let timing = cfg.timing.clone();
        let status = Status {
            pid: std::process::id(),
            version: VERSION,
            id: cfg.identity.id.clone(),
            server: cfg.client.server().authority(),
            state: State::Unknown.name(),
            state_since: system_now(),
            ..Status::default()
        };
        Relay {
            cfg,
            env,
            state: State::Unknown,
            clock: ServerClock::new(),
            jobs: JobMemory::default(),
            limiter: RateLimiter::default(),
            results: VecDeque::new(),
            sent_ifaces_hash: None,
            send_ifaces: false,
            backoff: Backoff::new(timing.backoff_first, timing.backoff_max),
            errors: ErrorLog {
                every: timing.error_log_every,
                streak: 0,
                logged_at: None,
                unlogged: 0,
            },
            noted_versions: (None, None),
            device_bind_warned: false,
            sent_packets: false,
            finishing: false,
            refusal_log: RefusalLog::default(),
            status,
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn env(&mut self) -> &mut E {
        &mut self.env
    }

    /// Results not yet delivered to the server.
    pub fn pending_results(&self) -> usize {
        self.results.len()
    }

    /// Runs until the executable changes or the stop marker appears.
    pub fn run(&mut self) -> Exit {
        loop {
            if let Some(exit) = self.exit_reason() {
                return self.finish(exit);
            }
            self.env.tidy();
            let started = Instant::now();
            let mut delay = self.step();
            if !self.sent_packets {
                delay = delay.max(self.cfg.timing.min_gap.saturating_sub(started.elapsed()));
            }
            if let Some(exit) = self.pause(delay) {
                return self.finish(exit);
            }
        }
    }

    fn exit_reason(&mut self) -> Option<Exit> {
        if let Some(why) = self.env.exe_changed() {
            return Some(Exit::ExeChanged(why));
        }
        self.env.stop_requested().then_some(Exit::Stopped)
    }

    /// Ends the loop. Results do not survive a restart, so results still waiting (a command sent
    /// while the executable was replaced) go to the server first, in at most [`FINAL_POLLS`]
    /// polls with `wait: 0`; a failed poll ends the attempt.
    fn finish(&mut self, exit: Exit) -> Exit {
        self.finishing = true;
        for _ in 0..FINAL_POLLS {
            if self.results.is_empty() || self.state == State::Unknown {
                break;
            }
            let failures = self.status.failures;
            self.step();
            if self.status.failures > failures {
                break;
            }
        }
        self.finishing = false;
        exit
    }

    fn pause(&mut self, total: Duration) -> Option<Exit> {
        let end = Instant::now() + total;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            self.env.sleep(left.min(self.cfg.timing.sleep_slice));
            if let Some(exit) = self.exit_reason() {
                return Some(exit);
            }
        }
    }

    /// One hello or poll; returns how long to wait before the next.
    pub fn step(&mut self) -> Duration {
        self.sent_packets = false;
        let (reports, hash) = self.interface_reports();
        let delay = match self.state {
            State::Unknown => self.hello(&reports, &hash),
            _ => self.poll(&reports, &hash),
        };
        self.write_status(false);
        delay
    }

    fn interface_reports(&mut self) -> (Vec<IfaceReport>, String) {
        let ifaces = self.env.interfaces().unwrap_or_default();
        let gateways = self.env.gateways();
        let reports = netif::reports(&ifaces, &gateways);
        let hash = netif::reports_hash(&reports);
        (reports, hash)
    }

    fn hello(&mut self, reports: &[IfaceReport], hash: &str) -> Duration {
        let payload = HelloPayload {
            v: PAYLOAD_VERSION,
            version: VERSION,
            arch: config::arch(),
            os: &self.cfg.os,
            kernel: &self.cfg.kernel,
            ifaces_hash: hash,
            ifaces: reports,
        };
        let body = match serde_json::to_string(&payload) {
            Ok(json) => self.call(Op::Hello, &json),
            Err(e) => Err(Failure::Local(e.to_string())),
        };
        let response = body.and_then(|body| {
            serde_json::from_slice::<HelloResponse>(&body).map_err(|e| bad_json(&e))
        });
        let response = match response {
            Ok(r) => r,
            Err(f) => return self.failed(f),
        };
        let Some(state) = State::parse(&response.state) else {
            return self.failed(bad_state(&response.state));
        };
        self.clock.update(response.server_time);
        self.succeeded();
        self.sent_ifaces_hash = Some(hash.to_owned());
        if let Some(code) = &response.code {
            // the server's code is not repeated: only the relay's own may look like one
            if normalize_code(code) != normalize_code(&self.cfg.identity.code) {
                logln!(
                    "서버가 알려 준 등록 코드가 이 중계기의 코드({})와 다릅니다. 서버 관리자에게 알리세요.",
                    self.cfg.identity.code
                );
            }
        }
        self.set_state(state);
        self.state_delay(state, response.poll_after)
    }

    fn poll(&mut self, reports: &[IfaceReport], hash: &str) -> Duration {
        let include_ifaces = self.send_ifaces || self.sent_ifaces_hash.as_deref() != Some(hash);
        let delivered = self.results.len().min(MAX_RESULTS_PER_POLL);
        let results: Vec<CommandResult> = self.results.iter().take(delivered).cloned().collect();
        let payload = PollPayload {
            v: PAYLOAD_VERSION,
            version: VERSION,
            arch: config::arch(),
            os: &self.cfg.os,
            kernel: &self.cfg.kernel,
            wait: if self.finishing {
                0
            } else {
                self.cfg.timing.wait
            },
            ifaces_hash: hash,
            ifaces: include_ifaces.then_some(reports),
            results: &results,
        };
        let body = match serde_json::to_string(&payload) {
            Ok(json) => self.call(Op::Poll, &json),
            Err(e) => Err(Failure::Local(e.to_string())),
        };
        let body = match body {
            Ok(body) => body,
            Err(Failure::Server {
                status: 404,
                error: Some(e),
                ..
            }) if e == "unknown_relay" => {
                // the server answered, it just does not know this relay (yet or any more)
                self.errors.ok();
                self.set_state(State::Unknown);
                return self.backoff.next();
            }
            Err(f) => return self.failed(f),
        };
        let response = match serde_json::from_slice::<PollResponse>(&body) {
            Ok(r) => r,
            Err(e) => return self.failed(bad_json(&e)),
        };
        let Some(state) = State::parse(&response.state) else {
            return self.failed(bad_state(&response.state));
        };
        self.clock.update(response.server_time);
        self.succeeded();
        self.results.drain(..delivered);
        if include_ifaces {
            self.sent_ifaces_hash = Some(hash.to_owned());
        }
        self.send_ifaces = response.send_ifaces;
        self.note_versions(
            response.latest_version.as_deref(),
            response.min_version.as_deref(),
        );
        self.set_state(state);
        let commands = response.commands.unwrap_or_default();
        if commands.len() > MAX_COMMANDS_PER_RESPONSE {
            logln!(
                "응답의 명령 {}개 중 {MAX_COMMANDS_PER_RESPONSE}개만 처리합니다.",
                commands.len()
            );
        }
        for entry in commands.iter().take(MAX_COMMANDS_PER_RESPONSE) {
            let result = if state == State::Active {
                self.execute(entry)
            } else {
                refused(entry, None, Refusal::NotActive)
            };
            self.record(result);
        }
        // results of commands that sent packets go back at once (the job is waiting for them);
        // refusals go with the next regular poll, so that bad commands cannot make it spin
        if self.sent_packets {
            return Duration::ZERO;
        }
        self.state_delay(state, response.poll_after)
    }

    /// Sends one request, re-signed once with the server's time if it says the clock is off.
    fn call(&mut self, op: Op, payload: &str) -> Result<Vec<u8>, Failure> {
        let mut retried = false;
        loop {
            let body = envelope::seal(&self.cfg.identity, op, self.clock.now(), payload)
                .map_err(|e| Failure::Local(e.to_string()))?;
            let response = self
                .cfg
                .client
                .post_json(op.path(), body.as_bytes(), self.cfg.timing.request_timeout)
                .map_err(Failure::Http)?;
            if response.status == 200 {
                return Ok(response.body);
            }
            let error: ErrorBody = serde_json::from_slice(&response.body).unwrap_or_default();
            if let Some(t) = error.server_time {
                self.clock.update(t);
            }
            if response.status == 400 && error.error.as_deref() == Some("clock") && !retried {
                retried = true;
                continue;
            }
            return Err(Failure::Server {
                status: response.status,
                error: error.error,
                retry_after: error.retry_after,
            });
        }
    }

    fn state_delay(&self, state: State, poll_after: Option<u64>) -> Duration {
        if let Some(s) = poll_after {
            return Duration::from_secs(s.min(MAX_SERVER_DELAY));
        }
        match state {
            State::Active | State::Unknown => Duration::ZERO,
            State::Pending => self.cfg.timing.pending_poll,
            State::Revoked => self.cfg.timing.revoked_poll,
        }
    }

    fn failed(&mut self, failure: Failure) -> Duration {
        let delay = match &failure {
            Failure::Server {
                status: 429 | 503,
                retry_after: Some(s),
                ..
            } => Duration::from_secs((*s).clamp(1, MAX_SERVER_DELAY)),
            _ => self.backoff.next(),
        };
        let text = log::clean(&failure.to_string(), MAX_STATUS_ERROR);
        self.errors.failed(&text, delay);
        let first = self.status.failures == 0;
        self.status.failures += 1;
        self.status.last_error = Some(text);
        self.status.last_error_at = Some(system_now());
        self.write_status(first);
        delay
    }

    fn succeeded(&mut self) {
        self.backoff.reset();
        self.errors.ok();
        let recovered = self.status.failures > 0;
        self.status.failures = 0;
        self.status.last_ok = Some(system_now());
        if recovered {
            self.write_status(true);
        }
    }

    fn set_state(&mut self, state: State) {
        if state == self.state {
            return;
        }
        match state {
            State::Pending => logln!(
                "등록 승인을 기다립니다. 등록 코드: {} (서버 관리자가 이 코드로 승인합니다)",
                self.cfg.identity.code
            ),
            State::Active => logln!("등록이 승인되어 켜기 명령을 기다립니다."),
            State::Revoked => logln!(
                "서버가 이 중계기를 폐기했습니다. 켜기 명령을 받지 않고 상태만 가끔 확인합니다."
            ),
            State::Unknown => logln!("서버에 이 중계기의 등록이 없습니다. 등록을 다시 요청합니다."),
        }
        self.state = state;
        self.status.state = state.name();
        self.status.state_since = system_now();
        self.write_status(true);
    }

    /// Logs once per value when the server names a newer or required version. Only versions
    /// that parse are noted, and they are logged as the relay reads them (numbers and dots).
    fn note_versions(&mut self, latest: Option<&str>, min: Option<&str>) {
        let latest = latest.and_then(parse_version);
        let min = min.and_then(parse_version);
        let mine = parse_version(VERSION);
        let older = |v: &Option<Vec<u64>>| match (&mine, v) {
            (Some(mine), Some(v)) => version_older(mine, v),
            _ => false,
        };
        let text = |v: &Option<Vec<u64>>| v.as_ref().map(|v| version_text(v));
        if text(&min).is_some() && self.noted_versions.1 != text(&min) {
            self.noted_versions.1 = text(&min);
            if older(&min) {
                logln!(
                    "이 hjv-relay({VERSION})는 서버가 요구하는 {}보다 오래되었습니다. 새 버전으로 바꾸세요.",
                    text(&min).unwrap_or_default()
                );
            }
        }
        if text(&latest).is_some() && self.noted_versions.0 != text(&latest) {
            self.noted_versions.0 = text(&latest);
            if older(&latest) && !older(&min) {
                logln!(
                    "새 hjv-relay {}가 나왔습니다 (지금 {VERSION}).",
                    text(&latest).unwrap_or_default()
                );
            }
        }
    }

    /// Checks 1-9 and the send, for one entry of `commands`.
    fn execute(&mut self, entry: &serde_json::Value) -> CommandResult {
        let Some(signed) = command::signed_entry(entry) else {
            let c = entry.get("c").and_then(|c| c.as_str());
            return refused(entry, c.map(command::command_hash), Refusal::BadCommand);
        };
        let hash = command::command_hash(signed.c);
        if let Err(r) = command::verify(&self.cfg.keys, &signed) {
            return refused(entry, Some(hash), r);
        }
        let cmd = match command::parse(signed.c) {
            Ok(cmd) => cmd,
            Err(r) => return refused(entry, Some(hash), r),
        };
        let job = Some(cmd.job.clone());
        let fail = |r: Refusal| CommandResult {
            cmd: Some(hash.clone()),
            job: job.clone(),
            sent_at: None,
            iface: None,
            packets: 0,
            error: Some(r.code()),
        };
        let admitted = match command::check(
            &cmd,
            &self.cfg.identity.id,
            self.clock.now(),
            &mut self.jobs,
        ) {
            Ok(a) => a,
            Err(r) => return fail(r),
        };
        let ifaces = self.env.interfaces().unwrap_or_default();
        let Some((target, matched)) = netif::select(&ifaces, &cmd.net) else {
            return fail(Refusal::NoIface);
        };
        let carrier = ifaces.iter().any(|i| i.name == target.iface && i.running());
        let plan = SendPlan::new(
            &target,
            &admitted.macs,
            &admitted.ports,
            admitted.repeat,
            admitted.gap,
        );
        if !self.limiter.admit(plan.packets(), Instant::now()) {
            return CommandResult {
                iface: Some(target.iface),
                ..fail(Refusal::RateLimited)
            };
        }
        let outcome = self.env.send(&plan);
        if let Some(e) = &outcome.device_error {
            if !self.device_bind_warned {
                self.device_bind_warned = true;
                logln!("SO_BINDTODEVICE를 쓸 수 없어({e}) 주소 bind만으로 랜카드를 고릅니다.");
            }
        }
        let error = if outcome.sent == 0 {
            Some(Refusal::SendFailed)
        } else if outcome.sent < outcome.planned {
            Some(Refusal::Partial)
        } else {
            None
        };
        let macs: Vec<String> = admitted.macs.iter().map(wake::mask_mac).collect();
        logln!(
            "켜기 {} ({}, 망 {} {}): {}{}: {}/{}개 보냄{}{}",
            short(&cmd.job),
            macs.join(", "),
            cmd.net,
            matched.name(),
            plan.describe(),
            if carrier { "" } else { " (연결 없음)" },
            outcome.sent,
            outcome.planned,
            error
                .map(|e| format!(" [{}]", e.code()))
                .unwrap_or_default(),
            outcome
                .errors
                .first()
                .map(|e| format!(" 오류: {e}"))
                .unwrap_or_default()
        );
        self.sent_packets |= outcome.sent > 0;
        CommandResult {
            cmd: Some(hash),
            job,
            sent_at: (outcome.sent > 0).then(|| self.clock.now()),
            iface: Some(target.iface),
            packets: outcome.sent,
            error: error.map(Refusal::code),
        }
    }

    fn record(&mut self, result: CommandResult) {
        if result.packets == 0 {
            if let Some(skipped) = self.refusal_log.admit(Instant::now()) {
                if skipped > 0 {
                    logln!("켜기 명령 거부 {skipped}건은 기록하지 않았습니다.");
                }
                let name = result
                    .job
                    .as_deref()
                    .or(result.cmd.as_deref())
                    .map(short)
                    .unwrap_or("?");
                logln!(
                    "켜기 명령 {name}을(를) 보내지 않았습니다: {}",
                    result.error.unwrap_or("?")
                );
            }
        }
        self.status.last_command = Some(LastCommand {
            at: system_now(),
            job: result.job.clone(),
            iface: result.iface.clone(),
            packets: result.packets,
            error: result.error,
        });
        self.results.push_back(result);
        if self.results.len() > MAX_RESULTS_KEPT {
            self.results.pop_front();
        }
        self.write_status(true);
    }

    fn write_status(&mut self, important: bool) {
        self.status.updated = system_now();
        if let Ok(json) = serde_json::to_string(&self.status) {
            self.env.write_status(&json, important);
        }
    }
}

/// Refused commands are logged at most 20 times per 10 minutes.
#[derive(Default)]
struct RefusalLog {
    start: Option<Instant>,
    count: u32,
    skipped: u32,
}

impl RefusalLog {
    const MAX: u32 = 20;
    const PERIOD: Duration = Duration::from_secs(600);

    /// `Some(lines skipped before)` when this refusal may be logged.
    fn admit(&mut self, now: Instant) -> Option<u32> {
        let mut skipped = 0;
        if self
            .start
            .is_none_or(|s| now.saturating_duration_since(s) >= Self::PERIOD)
        {
            skipped = self.skipped;
            *self = RefusalLog {
                start: Some(now),
                ..RefusalLog::default()
            };
        }
        self.count += 1;
        if self.count > Self::MAX {
            self.skipped += 1;
            return None;
        }
        Some(skipped)
    }
}

fn refused(entry: &serde_json::Value, hash: Option<String>, r: Refusal) -> CommandResult {
    let hash = hash.or_else(|| {
        entry
            .get("c")
            .and_then(|c| c.as_str())
            .map(command::command_hash)
    });
    CommandResult {
        cmd: hash,
        job: None,
        sent_at: None,
        iface: None,
        packets: 0,
        error: Some(r.code()),
    }
}

fn short(s: &str) -> &str {
    s.get(..8).unwrap_or(s)
}

fn normalize_code(code: &str) -> String {
    code.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// `MAJOR.MINOR.PATCH` (one to four parts of decimal digits, at most 9 digits each).
fn parse_version(v: &str) -> Option<Vec<u64>> {
    v.trim()
        .split('.')
        .map(|p| {
            (!p.is_empty() && p.len() <= 9 && p.bytes().all(|b| b.is_ascii_digit()))
                .then(|| p.parse::<u64>().ok())
                .flatten()
        })
        .collect::<Option<Vec<_>>>()
        .filter(|v| !v.is_empty() && v.len() <= 4)
}

fn version_text(v: &[u64]) -> String {
    v.iter().map(u64::to_string).collect::<Vec<_>>().join(".")
}

fn version_older(a: &[u64], b: &[u64]) -> bool {
    let n = a.len().max(b.len());
    let (mut a, mut b) = (a.to_vec(), b.to_vec());
    a.resize(n, 0);
    b.resize(n, 0);
    a < b
}

/// Whether version `a` is older than `b` (numeric dot-separated parts; anything else compares as
/// not older).
pub fn older(a: &str, b: &str) -> bool {
    match (parse_version(a), parse_version(b)) {
        (Some(a), Some(b)) => version_older(&a, &b),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(older("1.6.0", "1.6.1"));
        assert!(older("1.6.0", "1.10.0"));
        assert!(older("1.6", "1.6.1"));
        assert!(!older("1.6.0", "1.6"));
        assert!(!older("1.6.1", "1.6.0"));
        assert!(!older("1.6.0", "1.6.0-beta"));
        assert!(!older("1.6.0", ""));
    }

    #[test]
    fn backoff_doubles_to_the_cap_with_jitter() {
        let mut b = Backoff::new(Duration::from_secs(5), Duration::from_secs(300));
        let bases = [5u64, 10, 20, 40, 80, 160, 300, 300];
        for base in bases {
            let d = b.next().as_secs_f64();
            let base = base as f64;
            assert!(
                d >= base * 0.8 - 0.001 && d <= base * 1.2 + 0.001,
                "{d} vs {base}"
            );
        }
        b.reset();
        assert!(b.next() <= Duration::from_secs(6));
    }

    #[test]
    fn refusals_are_logged_at_most_20_times_in_10_minutes() {
        let mut log = RefusalLog::default();
        let t0 = Instant::now();
        for _ in 0..20 {
            assert_eq!(log.admit(t0), Some(0));
        }
        assert_eq!(log.admit(t0), None);
        assert_eq!(log.admit(t0 + Duration::from_secs(599)), None);
        assert_eq!(log.admit(t0 + Duration::from_secs(600)), Some(2));
        assert_eq!(log.admit(t0 + Duration::from_secs(601)), Some(0));
    }

    #[test]
    fn codes_compare_loosely() {
        assert_eq!(normalize_code("abcd-efgh ijkl-mnop"), "ABCDEFGHIJKLMNOP");
    }

    #[test]
    fn version_texts_are_numbers_and_dots() {
        assert_eq!(
            parse_version(" 1.6.10\n")
                .map(|v| version_text(&v))
                .as_deref(),
            Some("1.6.10")
        );
        for bad in [
            "+1.6",
            "1..6",
            "1.6.0.0.1",
            "1234567890.0",
            "1.6\n2026 등록 코드",
            "v1",
        ] {
            assert_eq!(parse_version(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn server_text_is_not_repeated() {
        assert!(is_code("unknown_relay") && is_code("active") && is_code("e2"));
        for bad in ["", "Active", "x\ny", "등록", &"a".repeat(33), "a b", "a-b"] {
            assert!(!is_code(bad), "{bad:?}");
        }
        let forged =
            "x\n2026-10-12 17:31:28 +0900 등록 승인을 기다립니다. 등록 코드: AAAA-BBBB-CCCC-DDDD";
        let shown = |f: Failure| f.to_string();
        let server = |error: &str| Failure::Server {
            status: 500,
            error: Some(error.to_owned()),
            retry_after: None,
        };
        assert_eq!(shown(server(forged)), "HTTP 500");
        assert_eq!(shown(server("unavailable")), "HTTP 500 unavailable");
        assert_eq!(shown(bad_state(forged)), "bad response: unknown state");
        assert_eq!(
            shown(bad_state("frozen")),
            "bad response: unknown state frozen"
        );
        // serde_json quotes values it did not expect; none of it reaches the text
        let body = format!(
            r#"{{"server_time":{},"state":"active"}}"#,
            serde_json::json!(forged)
        );
        let e = serde_json::from_str::<PollResponse>(&body).unwrap_err();
        let text = shown(bad_json(&e));
        assert!(
            text.starts_with("bad response: not the expected JSON (line 1 column"),
            "{text}"
        );
        let e = serde_json::from_str::<PollResponse>(r#"{"server_time":1}"#).unwrap_err();
        assert_eq!(
            shown(bad_json(&e)),
            "bad response: not the expected JSON, no state (line 1 column 17)"
        );
        let e = serde_json::from_str::<PollResponse>("<html>").unwrap_err();
        assert!(shown(bad_json(&e)).starts_with("bad response: not JSON"));
    }
}

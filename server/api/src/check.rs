// SPDX-License-Identifier: AGPL-3.0-only

//! The five-minute check (design v3 §5-9; deploy/systemd/hjviewer-api-check.timer).
//!
//! Why: while hjv-api is down, or only the PCs' syncs fail, password-less connections go on for
//! a day on the lists the PCs hold, and nobody notices until every PC falls back to its password
//! at once. So `hjv-api check`, run by a timer outside the service, looks at:
//!
//! - **socket**: `GET /api/hjv/source` on the socket nginx uses answers 200 within ten seconds;
//! - **admin**: the service answers `health` on its admin socket (the counts below);
//! - **sync**: of the PCs holding their binding (as `admin report` counts them) the ones that are
//!   on, by hbbs, which sees each PC register every few seconds: hbbs's console has seen the PC
//!   within [`ONLINE_SECS`], at each check of the last [`STEADY_SECS`] (so that a PC that has just
//!   woken has time for its first sync). When some are on and none of them synced in the last
//!   [`SYNC_WINDOW`] seconds, the syncs fail somewhere between the PCs and hjv-api. PCs that are
//!   off say nothing, so a night with every PC asleep is no alarm;
//! - **hbbs**: while PCs hold their binding, hbbs's console answers (without it the check cannot
//!   tell which PCs are on, and nobody can connect anyway);
//! - **sso_client**, **sso_address**, **sso**: SSO calls paused now or within the last
//!   [`SSO_RECENT`] seconds, because the SSO refused this server's client credentials
//!   (`invalid_client`: hjviewer-sso-client-secret must be renewed), refused its address, or kept
//!   failing (limits::Breaker);
//! - **relay**: once phase 2 has NAS relays, an active one that has not polled for
//!   [`RELAY_SILENT`] seconds.
//!
//! It prints one line when all is well, and a line per finding otherwise (errors in the journal),
//! writes the findings to a file the alert reads (`$RUNTIME_DIRECTORY/findings` under systemd:
//! /run/hjviewer-api-check/findings, `<code>: <text>` per line, counts only), and exits 1 when
//! there is one, which fails the unit and starts its `OnFailure=` alert (alert.rs).
//!
//! `health` is the admin command the check asks (admin.rs; it is not in the operator record): the
//! counts as JSON, [`Health`].

use crate::{
    admin::{self, AdminRequest},
    db,
    hbbs::{self, HbbsError, Lookup},
    state::AppState,
    util::now,
};
use http_body_util::{BodyExt, Empty, Limited};
use hyper::{body::Bytes, header::HOST, Request};
use hyper_util::rt::TokioIo;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fmt::Write as _,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};
use tokio::net::UnixStream;

/// A PC counts as on when hbbs saw it register within this many seconds.
pub const ONLINE_SECS: u64 = 90;
/// … at every check for this long (the timer runs every five minutes).
pub const STEADY_SECS: u64 = 240;
/// The syncs looked at: the last ten minutes (a PC syncs every two, at most every five).
pub const SYNC_WINDOW: i64 = 600;
/// An SSO pause this recent is reported, even when it has run out.
pub const SSO_RECENT: i64 = 600;
/// An active relay long-polls all the time; one silent for an hour is down (or its NAS is off).
pub const RELAY_SILENT: i64 = 3600;

pub const DEFAULT_API_SOCKET: &str = "/run/hjviewer-api.sock";
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(60);

pub const USAGE: &str =
    "hjv-api check [--api-socket <path>] [--admin-socket <path>] [--findings <file>]";

/// What `health` says.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct Health {
    pub v: u32,
    pub time: i64,
    /// PCs holding their binding (admin report's "PCs holding their binding")
    pub pcs_holding: i64,
    /// … of them synced in the last SYNC_WINDOW seconds
    pub pcs_synced: i64,
    /// … of them on, by hbbs (None when hbbs's console did not answer)
    pub pcs_on: Option<i64>,
    /// … of those synced in the last SYNC_WINDOW seconds
    pub pcs_on_synced: Option<i64>,
    /// "ok", "not asked" (no PC holds its binding), or what went wrong with hbbs's console
    pub hbbs: String,
    pub sso: Option<SsoPause>,
    pub relays: Option<Relays>,
}

/// The SSO calls' last pause (limits::Breaker).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct SsoPause {
    /// "client" (invalid_client), "address" or "upstream"
    pub reason: String,
    /// whether calls are paused now
    pub paused: bool,
    pub secs_ago: i64,
}

/// The NAS relays (phase 2).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Relays {
    pub active: i64,
    /// active relays without a poll for RELAY_SILENT seconds
    pub silent: i64,
}

pub const HBBS_OK: &str = "ok";
pub const HBBS_NOT_ASKED: &str = "not asked";

/// Since when hbbs has seen each PC on, check after check (in memory: a restart starts over).
#[derive(Default)]
pub struct OnlineSince {
    since: Mutex<HashMap<String, Instant>>,
}

impl OnlineSince {
    /// Notes the PCs on now (`on`), forgetting the others; returns those on for STEADY_SECS.
    pub fn observe(&self, on: &[String], at: Instant) -> Vec<String> {
        let mut since = self.since.lock().unwrap_or_else(|e| e.into_inner());
        since.retain(|id, _| on.contains(id));
        let mut steady = Vec::new();
        for id in on {
            let first = *since.entry(id.clone()).or_insert(at);
            if at.saturating_duration_since(first) >= Duration::from_secs(STEADY_SECS) {
                steady.push(id.clone());
            }
        }
        steady
    }

    /// Forgets every PC (hbbs did not answer: nobody knows who was on meanwhile).
    pub fn clear(&self) {
        self.since.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    /// Takes `id` as on since `ago` before now (tests).
    pub fn backdate(&self, id: &str, ago: Duration) {
        let at = Instant::now().checked_sub(ago).unwrap_or_else(Instant::now);
        self.since
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.to_owned(), at);
    }
}

fn hbbs_error(e: &HbbsError) -> &'static str {
    match e {
        HbbsError::Unreachable(_) => "unreachable",
        HbbsError::Unsupported => "no hjv-pk (patch 0004 missing)",
        HbbsError::Malformed => "answers in an unknown format",
        HbbsError::BadSignature => "answers not signed with HJV_API_HBBS_PUBKEY",
        HbbsError::NoServerKey => "HJV_API_HBBS_PUBKEY is not set",
        HbbsError::BadId => "bad ID",
    }
}

/// The service's side of `health`; a database that cannot be read is an error, which the check
/// reports.
pub async fn health(state: &AppState) -> Result<Health, String> {
    let at = now();
    let ttl = state.config.trust_ttl_secs;
    let mut h = Health {
        v: 1,
        time: at,
        hbbs: HBBS_NOT_ASKED.to_owned(),
        ..Health::default()
    };
    let (pcs, relays) = state
        .db
        .call(move |c| {
            Ok((
                db::holding_pcs(c, at, ttl)?,
                db::relay_health(c, at, RELAY_SILENT)?,
            ))
        })
        .await
        .map_err(|e| e.to_string())?;
    h.relays = relays.map(|(active, silent)| Relays { active, silent });
    let recent = |last: i64| last > at - SYNC_WINDOW;
    h.pcs_holding = pcs.len() as i64;
    h.pcs_synced = pcs.iter().filter(|(_, last)| recent(*last)).count() as i64;
    if pcs.is_empty() {
        state.online.clear();
        h.pcs_on = Some(0);
        h.pcs_on_synced = Some(0);
    } else {
        let mut on = Vec::new();
        let mut failed = None;
        for (id, _) in &pcs {
            match hbbs::lookup(
                state.config.hbbs_console,
                state.config.hbbs_pubkey.as_ref(),
                id,
            )
            .await
            {
                Ok(Lookup::Known {
                    seen_secs_ago: Some(age),
                    ..
                }) if age <= ONLINE_SECS => on.push(id.clone()),
                Ok(_) | Err(HbbsError::BadId) => {}
                Err(e) => {
                    failed = Some(e);
                    break;
                }
            }
        }
        match failed {
            Some(e) => {
                state.online.clear();
                h.hbbs = hbbs_error(&e).to_owned();
            }
            None => {
                let steady = state.online.observe(&on, Instant::now());
                h.hbbs = HBBS_OK.to_owned();
                h.pcs_on = Some(steady.len() as i64);
                h.pcs_on_synced = Some(
                    pcs.iter()
                        .filter(|(id, last)| steady.contains(id) && recent(*last))
                        .count() as i64,
                );
            }
        }
    }
    h.sso = state
        .sso_breaker
        .last_pause()
        .map(|(when, reason)| SsoPause {
            reason: reason.to_owned(),
            paused: state.sso_breaker.pause_reason().is_some(),
            secs_ago: (at - when).max(0),
        });
    Ok(h)
}

/// A finding: a short code (what the alert keys on) and a line, counts only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub code: &'static str,
    pub text: String,
}

fn finding(code: &'static str, text: String) -> Finding {
    Finding { code, text }
}

/// What the socket probe saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// 200 and the licence, in this many milliseconds
    Answered(u128),
    Failed(String),
}

/// The findings of a probe and of `health`'s answer (or why there was none).
pub fn evaluate(probe: &Probe, health: &Result<Health, String>) -> Vec<Finding> {
    let mut found = Vec::new();
    if let Probe::Failed(why) = probe {
        found.push(finding(
            "socket",
            format!("hjv-api does not answer on its socket ({why}): the apps get 503, and the PCs cannot renew their lists"),
        ));
    }
    let h = match health {
        Ok(h) => h,
        Err(why) => {
            found.push(finding(
                "admin",
                format!("hjv-api gave no counts on its admin socket ({why}): syncs, SSO and relays not checked"),
            ));
            return found;
        }
    };
    if let (Some(on), Some(0)) = (h.pcs_on, h.pcs_on_synced) {
        if on > 0 {
            found.push(finding(
                "sync",
                format!(
                    "none of the {on} PCs that are on (by hbbs) synced in the last {} minutes ({} of {} PCs holding their binding did): they keep their lists for a day, then ask for passwords",
                    SYNC_WINDOW / 60,
                    h.pcs_synced,
                    h.pcs_holding
                ),
            ));
        }
    }
    if h.pcs_on.is_none() && h.pcs_holding > 0 {
        found.push(finding(
            "hbbs",
            format!(
                "hbbs's console: {}; it is not known which PCs are on ({} of {} PCs holding their binding synced in the last {} minutes)",
                h.hbbs,
                h.pcs_synced,
                h.pcs_holding,
                SYNC_WINDOW / 60
            ),
        ));
    }
    if let Some(p) = &h.sso {
        if p.paused || p.secs_ago <= SSO_RECENT {
            let when = if p.paused {
                "paused now".to_owned()
            } else {
                format!("paused {} minutes ago", p.secs_ago / 60)
            };
            let (code, text) = match p.reason.as_str() {
                "client" => ("sso_client", format!("the SSO refuses this server's client credentials (invalid_client), SSO calls {when}: logins get 503 until the credential hjviewer-sso-client-secret is renewed")),
                "address" => ("sso_address", format!("the SSO refuses this server's address, SSO calls {when}: logins get 503")),
                _ => ("sso", format!("the SSO kept failing, SSO calls {when}: logins get 503 meanwhile")),
            };
            found.push(finding(code, text));
        }
    }
    if let Some(r) = &h.relays {
        if r.silent > 0 {
            found.push(finding(
                "relay",
                format!(
                    "{} of {} active NAS relays have not polled for an hour: switching PCs on fails",
                    r.silent, r.active
                ),
            ));
        }
    }
    found
}

/// One line of what was fine.
fn summary(probe: &Probe, h: &Health) -> String {
    let mut s = String::from("check: ok");
    if let Probe::Answered(ms) = probe {
        let _ = write!(s, "; socket {ms} ms");
    }
    let _ = write!(
        s,
        "; PCs holding their binding {}, synced in the last {} minutes {}",
        h.pcs_holding,
        SYNC_WINDOW / 60,
        h.pcs_synced
    );
    if let (Some(on), Some(synced)) = (h.pcs_on, h.pcs_on_synced) {
        let _ = write!(s, ", on {on}, of them synced {synced}");
    }
    let _ = write!(s, "; hbbs {}", h.hbbs);
    if let Some(r) = &h.relays {
        let _ = write!(s, "; relays {} active", r.active);
    }
    s
}

/// `GET /api/hjv/source` on the API socket.
pub async fn probe(socket: &Path) -> Probe {
    let started = Instant::now();
    let ask = async {
        let stream = UnixStream::connect(socket)
            .await
            .map_err(|e| format!("connect: {}", e.kind()))?;
        let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|e| format!("HTTP: {e}"))?;
        tokio::spawn(async move {
            let _ = conn.await;
        });
        let request = Request::get("/api/hjv/source")
            .header(HOST, "localhost")
            .body(Empty::<Bytes>::new())
            .map_err(|e| e.to_string())?;
        let response = sender
            .send_request(request)
            .await
            .map_err(|e| format!("HTTP: {e}"))?;
        let status = response.status().as_u16();
        let body = Limited::new(response.into_body(), 64 * 1024)
            .collect()
            .await
            .map_err(|_| "the answer's body".to_owned())?
            .to_bytes();
        let licence = serde_json::from_slice::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("license").and_then(|l| l.as_str()).map(str::to_owned));
        match (status, licence) {
            (200, Some(_)) => Ok(()),
            (200, None) => Err("200 without the licence".to_owned()),
            (s, _) => Err(format!("HTTP {s}")),
        }
    };
    match tokio::time::timeout(PROBE_TIMEOUT, ask).await {
        Ok(Ok(())) => Probe::Answered(started.elapsed().as_millis()),
        Ok(Err(e)) => Probe::Failed(e),
        Err(_) => Probe::Failed(format!("no answer in {} s", PROBE_TIMEOUT.as_secs())),
    }
}

/// `health` over the admin socket.
pub async fn ask_health(socket: &Path) -> Result<Health, String> {
    let request = AdminRequest {
        argv: vec!["health".into()],
        reveal: false,
        sudo_user: String::new(),
        input: None,
    };
    // no waiting for a socket that is not there: that is a finding
    let output = tokio::time::timeout(
        HEALTH_TIMEOUT,
        admin::send_within(socket, &request, Duration::ZERO),
    )
    .await
    .map_err(|_| format!("no answer in {} s", HEALTH_TIMEOUT.as_secs()))??;
    serde_json::from_str::<Health>(&output)
        .ok()
        .filter(|h| h.v == 1)
        .ok_or_else(|| "health: an answer in an unknown form".to_owned())
}

/// Writes the findings for the alert (none: the file goes).
fn write_findings(path: &Path, found: &[Finding]) -> Result<(), String> {
    if found.is_empty() {
        return match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(format!("{}: {e}", path.display()))
            }
            _ => Ok(()),
        };
    }
    let text: String = found
        .iter()
        .map(|f| format!("{}: {}\n", f.code, f.text))
        .collect();
    let temp = path.with_extension("new");
    std::fs::write(&temp, text)
        .and_then(|()| std::fs::rename(&temp, path))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Reads findings as `write_findings` wrote them: (code, text) per line.
pub fn read_findings(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let (code, rest) = line.split_once(": ")?;
            (!code.is_empty() && code.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'))
                .then(|| (code.to_owned(), rest.trim().to_owned()))
        })
        .collect()
}

/// `hjv-api check`: Ok when nothing is wrong.
pub async fn cli(args: &[String]) -> Result<(), String> {
    let mut api_socket = PathBuf::from(DEFAULT_API_SOCKET);
    let mut admin_socket = PathBuf::from(admin::DEFAULT_SOCKET);
    let mut findings = std::env::var_os("RUNTIME_DIRECTORY")
        .and_then(|d| {
            d.to_str()
                .and_then(|d| d.split(':').next())
                .map(PathBuf::from)
        })
        .filter(|d| !d.as_os_str().is_empty())
        .map(|d| d.join("findings"));
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let value = it
            .next()
            .map(PathBuf::from)
            .ok_or_else(|| format!("{arg} needs a value\n{USAGE}"));
        match arg.as_str() {
            "--api-socket" => api_socket = value?,
            "--admin-socket" => admin_socket = value?,
            "--findings" => findings = Some(value?),
            _ => return Err(USAGE.to_owned()),
        }
    }
    let probe = probe(&api_socket).await;
    let health = ask_health(&admin_socket).await;
    let found = evaluate(&probe, &health);
    if let Some(path) = &findings {
        if let Err(e) = write_findings(path, &found) {
            log_err!("check: {e}");
        }
    }
    if found.is_empty() {
        if let Ok(h) = &health {
            println!("{}", summary(&probe, h));
        }
        return Ok(());
    }
    for f in &found {
        log_err!("check: {}: {}", f.code, f.text);
    }
    Err(format!(
        "{} found: {}",
        if found.len() == 1 {
            "a problem".to_owned()
        } else {
            format!("{} problems", found.len())
        },
        found.iter().map(|f| f.code).collect::<Vec<_>>().join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn healthy() -> Health {
        Health {
            v: 1,
            time: 1_791_856_812,
            pcs_holding: 5,
            pcs_synced: 3,
            pcs_on: Some(3),
            pcs_on_synced: Some(3),
            hbbs: HBBS_OK.into(),
            sso: None,
            relays: None,
        }
    }

    fn codes(found: &[Finding]) -> Vec<&'static str> {
        found.iter().map(|f| f.code).collect()
    }

    #[test]
    fn a_fake_report_with_a_stopped_socket_and_no_syncs() {
        let ok = Probe::Answered(8);
        assert!(evaluate(&ok, &Ok(healthy())).is_empty());
        // the socket stopped, and the PCs that are on do not sync
        let stopped = Probe::Failed("connect: connection refused".into());
        let no_syncs = Health {
            pcs_synced: 0,
            pcs_on_synced: Some(0),
            ..healthy()
        };
        let found = evaluate(&stopped, &Ok(no_syncs));
        assert_eq!(codes(&found), ["socket", "sync"]);
        assert!(found[0].text.contains("connection refused"), "{found:?}");
        assert!(
            found[1].text.starts_with(
                "none of the 3 PCs that are on (by hbbs) synced in the last 10 minutes (0 of 5"
            ),
            "{found:?}"
        );
        // without an answer from the admin socket nothing else can be looked at
        let found = evaluate(&stopped, &Err("connection refused".into()));
        assert_eq!(codes(&found), ["socket", "admin"]);
        // a night with every PC asleep is no alarm: nothing is on
        let night = Health {
            pcs_synced: 0,
            pcs_on: Some(0),
            pcs_on_synced: Some(0),
            ..healthy()
        };
        assert!(evaluate(&ok, &Ok(night)).is_empty());
        // one of the PCs that are on syncing is enough
        let some = Health {
            pcs_on_synced: Some(1),
            ..healthy()
        };
        assert!(evaluate(&ok, &Ok(some)).is_empty());
    }

    #[test]
    fn hbbs_sso_and_relays() {
        let ok = Probe::Answered(8);
        let blind = Health {
            pcs_on: None,
            pcs_on_synced: None,
            hbbs: "unreachable".into(),
            ..healthy()
        };
        let found = evaluate(&ok, &Ok(blind.clone()));
        assert_eq!(codes(&found), ["hbbs"]);
        assert!(found[0].text.contains("hbbs's console: unreachable"));
        // without PCs holding a binding hbbs is not asked, and not missed
        let empty = Health {
            pcs_holding: 0,
            pcs_synced: 0,
            ..blind
        };
        assert!(evaluate(&ok, &Ok(empty)).is_empty());
        let sso = |reason: &str, paused, secs_ago| Health {
            sso: Some(SsoPause {
                reason: reason.into(),
                paused,
                secs_ago,
            }),
            ..healthy()
        };
        assert_eq!(
            codes(&evaluate(&ok, &Ok(sso("client", true, 5)))),
            ["sso_client"]
        );
        assert_eq!(
            codes(&evaluate(&ok, &Ok(sso("client", false, 400)))),
            ["sso_client"],
            "a pause that ran out between two checks"
        );
        assert!(evaluate(&ok, &Ok(sso("client", false, 700))).is_empty());
        assert_eq!(
            codes(&evaluate(&ok, &Ok(sso("address", true, 1)))),
            ["sso_address"]
        );
        assert_eq!(
            codes(&evaluate(&ok, &Ok(sso("upstream", true, 1)))),
            ["sso"]
        );
        let invalid = &evaluate(&ok, &Ok(sso("client", true, 5)))[0];
        assert!(invalid.text.contains("invalid_client"));
        assert!(invalid.text.contains("hjviewer-sso-client-secret"));
        let relays = |active, silent| Health {
            relays: Some(Relays { active, silent }),
            ..healthy()
        };
        assert!(evaluate(&ok, &Ok(relays(1, 0))).is_empty());
        assert_eq!(codes(&evaluate(&ok, &Ok(relays(2, 1)))), ["relay"]);
    }

    #[test]
    fn steady_means_on_at_every_check_for_four_minutes() {
        let o = OnlineSince::default();
        let t0 = Instant::now();
        let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(o.observe(&ids(&["a", "b"]), t0).is_empty());
        let t1 = t0 + Duration::from_secs(STEADY_SECS);
        assert_eq!(o.observe(&ids(&["a", "b"]), t1), ids(&["a", "b"]));
        // b was off at one check: it starts over
        let t2 = t1 + Duration::from_secs(300);
        assert_eq!(o.observe(&ids(&["a"]), t2), ids(&["a"]));
        let t3 = t2 + Duration::from_secs(300);
        assert_eq!(o.observe(&ids(&["a", "b"]), t3), ids(&["a"]));
        o.clear();
        assert!(o.observe(&ids(&["a"]), t3).is_empty());
    }

    #[test]
    fn findings_files() {
        let found = evaluate(
            &Probe::Failed("no answer in 10 s".into()),
            &Err("no answer".into()),
        );
        let dir = std::env::temp_dir().join(format!("hjv-findings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("findings");
        write_findings(&path, &found).unwrap();
        let read = read_findings(&std::fs::read_to_string(&path).unwrap());
        assert_eq!(
            read.iter().map(|(c, _)| c.as_str()).collect::<Vec<_>>(),
            ["socket", "admin"]
        );
        assert!(read[0].1.contains("no answer in 10 s"));
        // all well again: the file goes
        write_findings(&path, &[]).unwrap();
        assert!(!path.exists());
        write_findings(&path, &[]).unwrap();
        assert_eq!(
            read_findings("garbage\nX: y\n: z\nsync: ok"),
            [("sync".to_owned(), "ok".to_owned())]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

// SPDX-License-Identifier: AGPL-3.0-only

//! `hjv-api alert <unit>` (design v3 §5-9): what deploy/systemd/hjviewer-api-alert@.service runs
//! when hjviewer-api.service, its check or its backup fails (their `OnFailure=`).
//!
//! It always writes the alert to the journal: an error line in the namespace `hjviewer`, which the
//! operations agent's daily report counts (design v3 §5-9, question 12). When the user has put a
//! Telegram bot into the credential `hjviewer-api-telegram` (the unit imports it if it is there,
//! and without it nothing more happens), it also sends a message to that chat, as the user's other
//! services do: once in six hours for the same unit with the same findings, so that a check
//! failing every five minutes makes one message; new findings go at once. The message holds the
//! unit, the server's host name, the time and the check's findings: counts and states, no person,
//! device, address or secret. `hjv-api alert test` (`systemctl start
//! hjviewer-api-alert@test.service`) sends a test message whatever was sent before.
//!
//! The credential, made by the user on the server (README.md, Monitoring):
//!
//! ```text
//! TELEGRAM_BOT_TOKEN=<the bot's token, digits:letters>
//! TELEGRAM_CHAT_ID=<the chat's number>
//! ```
//!
//! The token never goes to the journal, the output or an error: Telegram's address carries it, so
//! no address is printed, and every error text is cleared of it.

use crate::{
    check::read_findings,
    sso::tls_config,
    util::{datetime, hex, now, sha256, HOUR},
};
use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    body::Bytes,
    header::{CONTENT_TYPE, USER_AGENT},
    Request, Uri,
};
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client},
    rt::TokioExecutor,
};
use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};
use zeroize::Zeroizing;

pub const CREDENTIAL: &str = "hjviewer-api-telegram";
pub const CHECK_UNIT: &str = "hjviewer-api-check.service";
pub const DEFAULT_FINDINGS: &str = "/run/hjviewer-api-check/findings";
/// The same alert is sent again after this long at the soonest.
pub const REALERT: i64 = 6 * HOUR;
/// The check's findings count for its alert when they are this fresh.
const FRESH: Duration = Duration::from_secs(900);
const TELEGRAM: &str = "https://api.telegram.org";
const SEND_TIMEOUT: Duration = Duration::from_secs(20);
const TEST: &str = "test";

pub const USAGE: &str = "hjv-api alert <unit>|test [--findings <file>] [--state-dir <dir>]";

/// The bot and the chat of the credential.
struct Telegram {
    token: Zeroizing<String>,
    chat: String,
}

/// Reads `TELEGRAM_BOT_TOKEN=` and `TELEGRAM_CHAT_ID=` lines; the error never shows a value.
fn parse_credential(text: &str) -> Result<Telegram, String> {
    let mut token = None;
    let mut chat = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match line.split_once('=').map(|(k, v)| (k.trim(), v.trim())) {
            Some(("TELEGRAM_BOT_TOKEN", v)) => token = Some(Zeroizing::new(v.to_owned())),
            Some(("TELEGRAM_CHAT_ID", v)) => chat = Some(v.to_owned()),
            _ => {}
        }
    }
    let token = token.filter(|t| {
        t.split_once(':').is_some_and(|(bot, secret)| {
            !bot.is_empty()
                && bot.bytes().all(|b| b.is_ascii_digit())
                && secret.len() >= 20
                && secret
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
    });
    let chat = chat.filter(|c| {
        let digits = c.strip_prefix('-').unwrap_or(c);
        !digits.is_empty() && digits.len() <= 20 && digits.bytes().all(|b| b.is_ascii_digit())
    });
    match (token, chat) {
        (Some(token), Some(chat)) => Ok(Telegram { token, chat }),
        _ => Err(format!(
            "the credential {CREDENTIAL} is not TELEGRAM_BOT_TOKEN=<digits>:<token> and TELEGRAM_CHAT_ID=<number> (README.md, Monitoring)"
        )),
    }
}

/// The credential, when the unit imported it.
fn read_credential() -> Result<Option<Telegram>, String> {
    let Some(dir) = std::env::var_os("CREDENTIALS_DIRECTORY") else {
        return Ok(None);
    };
    match std::fs::read_to_string(Path::new(&dir).join(CREDENTIAL)) {
        Ok(text) => parse_credential(&Zeroizing::new(text)).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("the credential {CREDENTIAL}: {e}")),
    }
}

/// What failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    pub unit: String,
    /// how the unit ended, as systemd tells the alert (`signal, killed KILL`), when it does
    pub ended: Option<String>,
    /// the check's findings: (code, text)
    pub findings: Vec<(String, String)>,
    pub host: String,
    pub at: i64,
}

/// How the failed unit ended, from what systemd passes to an `OnFailure=` unit (systemd 251 and
/// later): `MONITOR_SERVICE_RESULT`, with `MONITOR_EXIT_CODE` and `MONITOR_EXIT_STATUS`.
fn how_it_ended() -> Option<String> {
    let word = |name: &str| {
        std::env::var(name).ok().filter(|v| {
            !v.is_empty()
                && v.len() <= 32
                && v.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
    };
    let result = word("MONITOR_SERVICE_RESULT")?;
    Some(
        match (word("MONITOR_EXIT_CODE"), word("MONITOR_EXIT_STATUS")) {
            (Some(code), Some(status)) => format!("{result}, {code} {status}"),
            _ => result,
        },
    )
}

/// A failed unit's name as the alert unit's instance carries it: letters, digits and a few signs.
fn valid_unit(unit: &str) -> bool {
    !unit.is_empty()
        && unit.len() <= 128
        && unit
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"@._-:\\".contains(&b))
}

/// What a finding means, for a person (the check's text follows it).
fn meaning(code: &str) -> &'static str {
    match code {
        "socket" => "hjv-api가 소켓에서 답하지 않습니다. 앱 요청은 503을 받고, PC는 목록을 새로 받지 못합니다.",
        "admin" => "hjv-api가 관리 소켓에서 점검할 수치를 주지 않아 동기화, SSO, 중계기를 보지 못했습니다.",
        "sync" => "켜진 PC들이 10분 동안 동기화하지 못했습니다. 그대로면 하루 뒤 모든 PC가 비밀번호 창으로 돌아갑니다.",
        "hbbs" => "hbbs 콘솔이 답하지 않아 어느 PC가 켜졌는지 모릅니다.",
        "sso_client" => "연세 SSO가 이 서버의 client 자격 증명을 거절합니다(invalid_client). hjviewer-sso-client-secret을 새로 넣기 전까지 로그인이 안 됩니다.",
        "sso_address" => "연세 SSO가 이 서버의 주소를 거절합니다. 그동안 로그인이 안 됩니다.",
        "sso" => "연세 SSO가 계속 실패해 로그인을 잠시 멈췄습니다.",
        "sso_new" => "명단에 없는 포탈 ID들의 SSO 호출이 한도를 채웠습니다. 누군가 hjv-api로 여러 포탈 ID에 비밀번호를 시험하고 있을 수 있습니다(처음 로그인하는 사람이 몰렸을 수도 있음). 그동안 처음 로그인하는 사람은 실패합니다. 런북 7-13대로 봅니다.",
        "sso_user" => "연세 SSO의 사용자 정보 호출이 서로 다른 계정을 잇달아 거절했습니다. 모두를 거절하는 것이면 HJV_API_SSO_CLIENT_ID나 자격 증명 hjviewer-sso-api-uuid가 틀린 것이고, 그동안 맞는 비밀번호로 로그인한 교직원도 '이 계정으로는 HJViewer를 쓸 수 없습니다'를 받습니다. 런북 7-13대로 봅니다.",
        "relay" => "NAS 중계기가 1시간 넘게 연락하지 않아 PC 켜기가 안 됩니다.",
        _ => "알 수 없는 점검 결과입니다.",
    }
}

impl Alert {
    fn test(&self) -> bool {
        self.unit == TEST
    }

    /// The unit, how it ended and its findings' codes: the same key, the same alert (a crash and
    /// a start that fails are two alerts).
    fn key(&self) -> String {
        let mut codes: Vec<&str> = self.findings.iter().map(|(c, _)| c.as_str()).collect();
        codes.sort_unstable();
        codes.dedup();
        format!(
            "{} {} {}",
            self.unit,
            self.ended.as_deref().unwrap_or("-"),
            codes.join(",")
        )
    }

    /// The journal's line.
    fn line(&self) -> String {
        if self.test() {
            return "alert: a test alert".to_owned();
        }
        let mut line = format!("alert: {} failed", self.unit);
        if let Some(ended) = &self.ended {
            line.push_str(&format!(" [{ended}]"));
        }
        if !self.findings.is_empty() {
            let codes: Vec<&str> = self.findings.iter().map(|(c, _)| c.as_str()).collect();
            line.push_str(&format!(" ({})", codes.join(", ")));
        }
        line
    }

    /// The message for a person.
    pub fn message(&self) -> String {
        let when = datetime(self.at);
        if self.test() {
            return format!(
                "[HJViewer 경보 시험] {} {when}\n이 채팅으로 hjv-api의 경보를 받습니다.",
                self.host
            );
        }
        let mut text = format!("[HJViewer 경보] {} 실패\n{} {when}\n", self.unit, self.host);
        // how it ended tells what happened when there are no findings to tell it
        if let (Some(ended), true) = (&self.ended, self.findings.is_empty()) {
            text.push_str(&format!("종료: {ended}\n"));
        }
        if self.findings.is_empty() {
            text.push_str(match self.unit.as_str() {
                "hjviewer-api.service" => "hjv-api 서비스가 멈췄습니다. systemd가 다시 시작하지만, 계속 실패하면 앱 로그인과 PC 동기화가 안 됩니다.\n",
                "hjviewer-api-backup.service" => "오늘의 DB 백업이 실패했습니다.\n",
                CHECK_UNIT => "점검 자체가 실패했습니다(결과 없음).\n",
                _ => "유닛이 실패했습니다.\n",
            });
        }
        for (code, detail) in &self.findings {
            text.push_str(&format!("- {}\n  ({code}: {detail})\n", meaning(code)));
        }
        let unit = self.unit.trim_end_matches(".service");
        text.push_str(&format!(
            "확인: sudo journalctl --namespace=hjviewer -u {unit} -n 50\n같은 경보는 {}시간 동안 다시 보내지 않습니다.",
            REALERT / HOUR
        ));
        text
    }
}

/// The file that says when an alert of `key` was last sent.
fn stamp_path(dir: &Path, key: &str) -> PathBuf {
    dir.join(format!("sent-{}", &hex(&sha256(key.as_bytes()))[..16]))
}

/// When the alert of `key` was last sent, by its stamp.
fn last_sent(dir: &Path, key: &str) -> Option<i64> {
    std::fs::read_to_string(stamp_path(dir, key))
        .ok()
        .and_then(|t| t.trim().parse().ok())
}

/// Sends `text` to the chat; errors are cleared of the token.
async fn send(api: &str, telegram: &Telegram, text: &str) -> Result<(), String> {
    let clear = |e: String| e.replace(telegram.token.as_str(), "<token>");
    let uri: Uri = format!("{api}/bot{}/sendMessage", telegram.token.as_str())
        .parse()
        .map_err(|_| "the bot token does not fit an address".to_owned())?;
    let mut http = HttpConnector::new();
    http.enforce_http(false);
    http.set_connect_timeout(Some(Duration::from_secs(5)));
    let https = HttpsConnectorBuilder::new()
        .with_tls_config(tls_config()?)
        .https_or_http()
        .enable_http1()
        .wrap_connector(http);
    let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build(https);
    let body = form_urlencoded::Serializer::new(String::new())
        .append_pair("chat_id", &telegram.chat)
        .append_pair("text", text)
        .append_pair("disable_web_page_preview", "true")
        .finish();
    let request = Request::post(uri)
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(USER_AGENT, concat!("hjv-api/", env!("CARGO_PKG_VERSION")))
        .body(Full::new(Bytes::from(body)))
        .map_err(|e| clear(e.to_string()))?;
    let exchange = async {
        let response = client
            .request(request)
            .await
            .map_err(|e| clear(format!("Telegram: {e}")))?;
        let status = response.status().as_u16();
        let body = Limited::new(response.into_body(), 64 * 1024)
            .collect()
            .await
            .map_err(|_| "Telegram: the answer's body".to_owned())?
            .to_bytes();
        let answer: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        if status == 200 && answer["ok"] == true {
            return Ok(());
        }
        // cleared before it is cut, so that no part of the token is left
        let description = answer["description"]
            .as_str()
            .map(|d| crate::util::clean_text(&clear(d.to_owned()), 160))
            .unwrap_or_default();
        Err(clear(format!("Telegram: HTTP {status} {description}")))
    };
    tokio::time::timeout(SEND_TIMEOUT, exchange)
        .await
        .map_err(|_| format!("Telegram: no answer in {} s", SEND_TIMEOUT.as_secs()))?
}

/// The host name, for the message: the kernel's, or where the unit's sandbox hides /proc/sys
/// (ProcSubset=pid) the static one.
fn host_name() -> String {
    ["/proc/sys/kernel/hostname", "/etc/hostname"]
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .map(|h| crate::util::clean_text(&h, 64))
        .find(|h| !h.is_empty())
        .unwrap_or_else(|| "-".to_owned())
}

/// The check's findings, when `unit` is the check and they are fresh.
fn check_findings(unit: &str, path: &Path) -> Vec<(String, String)> {
    if unit != CHECK_UNIT {
        return Vec::new();
    }
    let fresh = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|m| SystemTime::now().duration_since(m).ok())
        .is_some_and(|age| age <= FRESH);
    if !fresh {
        return Vec::new();
    }
    std::fs::read_to_string(path)
        .map(|t| read_findings(&t))
        .unwrap_or_default()
}

/// `hjv-api alert <unit>|test [--findings <file>] [--state-dir <dir>]`; `--telegram-api
/// http://127.0.0.1:<port>` is for tests (a local stand-in for Telegram).
pub async fn cli(args: &[String]) -> Result<(), String> {
    let mut unit = None;
    let mut findings = PathBuf::from(DEFAULT_FINDINGS);
    let mut state_dir = std::env::var_os("STATE_DIRECTORY")
        .and_then(|d| {
            d.to_str()
                .and_then(|d| d.split(':').next())
                .map(PathBuf::from)
        })
        .filter(|d| !d.as_os_str().is_empty());
    let mut api = TELEGRAM.to_owned();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--findings" | "--state-dir" | "--telegram-api" => {
                let value = it
                    .next()
                    .ok_or_else(|| format!("{arg} needs a value\n{USAGE}"))?;
                match arg.as_str() {
                    "--findings" => findings = PathBuf::from(value),
                    "--state-dir" => state_dir = Some(PathBuf::from(value)),
                    _ => {
                        let local = value.strip_prefix("http://127.0.0.1:").is_some_and(|port| {
                            !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())
                        });
                        if !local {
                            return Err(
                                "--telegram-api takes http://127.0.0.1:<port> (tests)".into()
                            );
                        }
                        api = value.clone();
                    }
                }
            }
            a if unit.is_none() && valid_unit(a) => unit = Some(a.to_owned()),
            _ => return Err(USAGE.to_owned()),
        }
    }
    let unit = unit.ok_or(USAGE)?;
    let alert = Alert {
        findings: check_findings(&unit, &findings),
        ended: how_it_ended(),
        unit,
        host: host_name(),
        at: now(),
    };
    if alert.test() {
        log_warn!("{}", alert.line());
    } else {
        log_err!("{}", alert.line());
    }
    let telegram = match read_credential() {
        Ok(Some(t)) => t,
        Ok(None) => {
            log_notice!(
                "alert: no Telegram (credential {CREDENTIAL}); the journal line above is the alert"
            );
            return Ok(());
        }
        Err(e) => {
            log_err!("alert: {e}");
            return Err(e);
        }
    };
    let key = alert.key();
    let at = alert.at;
    if !alert.test() {
        if let Some(sent) = state_dir.as_deref().and_then(|d| last_sent(d, &key)) {
            if (0..REALERT).contains(&(at - sent)) {
                log_notice!(
                    "alert: sent to Telegram {} minutes ago; again after {} hours",
                    (at - sent) / 60,
                    REALERT / HOUR
                );
                return Ok(());
            }
        }
    }
    match send(&api, &telegram, &alert.message()).await {
        Ok(()) => {
            log_notice!("alert: sent to Telegram");
            if let (Some(dir), false) = (&state_dir, alert.test()) {
                if let Err(e) = std::fs::write(stamp_path(dir, &key), at.to_string()) {
                    log_warn!("alert: {}: {e}", dir.display());
                }
            }
            Ok(())
        }
        Err(e) => {
            log_err!("alert: {e}");
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "123456:TEST-not-a-real-token_xyz";

    #[test]
    fn credentials() {
        let t = parse_credential(&format!(
            "# bot\r\nTELEGRAM_BOT_TOKEN={TOKEN}\r\n TELEGRAM_CHAT_ID = -100200\n"
        ))
        .unwrap();
        assert_eq!((t.token.as_str(), t.chat.as_str()), (TOKEN, "-100200"));
        for bad in [
            format!("TELEGRAM_BOT_TOKEN={TOKEN}\n"),
            "TELEGRAM_CHAT_ID=42\n".to_owned(),
            "TELEGRAM_BOT_TOKEN=abc:defdefdefdefdefdefdefdef\nTELEGRAM_CHAT_ID=42\n".to_owned(),
            format!("TELEGRAM_BOT_TOKEN={TOKEN}\nTELEGRAM_CHAT_ID=@channel\n"),
            format!("TELEGRAM_BOT_TOKEN={TOKEN}/x\nTELEGRAM_CHAT_ID=42\n"),
        ] {
            let err = parse_credential(&bad).err().unwrap();
            assert!(!err.contains("TEST-not"), "{err}");
        }
    }

    #[test]
    fn every_finding_of_the_check_has_its_meaning() {
        let unknown = meaning("no such code");
        for code in [
            "socket",
            "admin",
            "sync",
            "hbbs",
            "sso_client",
            "sso_address",
            "sso",
            "sso_new",
            "sso_user",
            "relay",
        ] {
            assert_ne!(meaning(code), unknown, "{code}");
        }
        assert!(meaning("sso_new").contains("런북 7-13"));
        assert!(meaning("sso_user").contains("런북 7-13"));
        assert!(meaning("sso_user").contains("HJV_API_SSO_CLIENT_ID"));
        assert!(meaning("sso_user").contains("hjviewer-sso-api-uuid"));
    }

    #[test]
    fn messages_hold_no_values() {
        let check = Alert {
            unit: CHECK_UNIT.into(),
            ended: None,
            findings: vec![
                ("sync".into(), "none of the 3 PCs that are on synced".into()),
                ("sso_client".into(), "the SSO refuses …".into()),
            ],
            host: "project".into(),
            at: 1_791_856_812,
        };
        let text = check.message();
        assert!(
            text.starts_with(
                "[HJViewer 경보] hjviewer-api-check.service 실패\nproject 2026-10-13T02:00:12Z\n"
            ),
            "{text}"
        );
        assert!(text.contains("켜진 PC들이 10분 동안"));
        assert!(text.contains("(sync: none of the 3 PCs that are on synced)"));
        assert!(text.contains("invalid_client"));
        assert!(text.contains("journalctl --namespace=hjviewer -u hjviewer-api-check -n 50"));
        assert_eq!(
            check.line(),
            "alert: hjviewer-api-check.service failed (sync, sso_client)"
        );
        // the same findings in another order are the same alert; others are another
        let mut swapped = check.clone();
        swapped.findings.reverse();
        assert_eq!(swapped.key(), check.key());
        let mut other = check.clone();
        other.findings.pop();
        assert_ne!(other.key(), check.key());
        let service = Alert {
            unit: "hjviewer-api.service".into(),
            findings: vec![],
            ..check.clone()
        };
        assert!(service.message().contains("hjv-api 서비스가 멈췄습니다"));
        assert_eq!(service.line(), "alert: hjviewer-api.service failed");
        // how it ended, when systemd says: a crash and a start that fails are two alerts
        let crash = Alert {
            ended: Some("signal, killed KILL".into()),
            ..service.clone()
        };
        let start = Alert {
            ended: Some("exit-code, exited 1".into()),
            ..service.clone()
        };
        assert_eq!(
            crash.line(),
            "alert: hjviewer-api.service failed [signal, killed KILL]"
        );
        assert!(crash.message().contains("\n종료: signal, killed KILL\n"));
        assert_ne!(crash.key(), start.key());
        assert!(valid_unit("hjviewer-api-check.service"));
        assert!(valid_unit("test"));
        assert!(!valid_unit("a b") && !valid_unit("") && !valid_unit("x;y"));
    }

    #[test]
    fn stamps() {
        let dir = std::env::temp_dir().join(format!("hjv-alert-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(last_sent(&dir, "a"), None);
        std::fs::write(stamp_path(&dir, "a"), "100").unwrap();
        assert_eq!(last_sent(&dir, "a"), Some(100));
        assert_eq!(last_sent(&dir, "b"), None);
        assert_ne!(stamp_path(&dir, "a"), stamp_path(&dir, "b"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

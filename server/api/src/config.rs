// SPDX-License-Identifier: AGPL-3.0-only

//! Settings and secrets.
//!
//! Settings are not secret and come from the environment, which systemd fills from
//! /etc/hjviewer/hjviewer-api.env (`hjv-api serve --env-file` reads such a file directly);
//! hjviewer-api.env.example lists every name with its default. Secrets come only from systemd
//! credentials ($CREDENTIALS_DIRECTORY, or `--credentials <dir>`), never from the environment.

use crate::{
    trust,
    util::{b64_decode, IpNet, DAY},
};
use hyper::Uri;
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    path::Path,
    time::Duration,
};
use zeroize::Zeroizing;

/// `count` events within `secs` seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub count: u32,
    pub secs: i64,
}

#[derive(Debug, Clone)]
pub struct SsoSettings {
    pub token_url: Uri,
    pub userinfo_url: Uri,
    /// not secret (design v2 §10-2): sent as `clientId` with the user-info call
    pub client_id: String,
    pub connect_timeout: Duration,
    pub total_timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub sso: SsoSettings,
    /// hbbs's loopback console (patch 0004 answers `hjv-pk <id>` there)
    pub hbbs_console: SocketAddr,
    /// hbbs's public key (its id_ed25519.pub, the key the apps are built with), which signs the
    /// console's answers (hbbs.rs); without it no device is added to an account
    pub hbbs_pubkey: Option<[u8; 32]>,
    /// where /api/hjv/source sends users for the source (AGPL section 13)
    pub source_url: String,
    /// failed logins per client address (v2 §9-2 step 2)
    pub limit_ip: Vec<Window>,
    /// failed logins per portal ID, for every ID typed (step 5)
    pub limit_id: Vec<Window>,
    /// failed logins of a portal ID from the devices already in that account (step 5)
    pub limit_bound: Vec<Window>,
    /// successful logins per account and per client address, so that one known password cannot
    /// keep calling the SSO (step 7)
    pub limit_ok_id: Vec<Window>,
    pub limit_ok_ip: Vec<Window>,
    /// calls to the SSO, all accounts together (step 7)
    pub limit_sso: Window,
    pub token_idle_secs: i64,
    pub token_max_secs: i64,
    pub account_retention_secs: i64,
    pub device_unseen_secs: i64,
    pub event_retention_secs: i64,
    pub seal_max_skew_secs: i64,
    /// how long the very same login, or signed account request that changes something, sent
    /// again gets the first answer (login.rs `RetryCache`)
    pub retry_window_secs: i64,
    pub failure_delay: Duration,
    pub failure_jitter: Duration,
    /// deviceInfo.os values whose installed apps a login adds to the account (D10: windows)
    pub bind_os: Vec<String>,
    /// D11: "off" (the only value this version accepts; the hook is tfa.rs)
    pub second_factor: String,
    pub max_logins_in_flight: usize,
    /// the university's address networks: a login from one of them is "on-campus", from
    /// elsewhere "off-campus" (shown to the account's other devices, design v3 §4-1); empty: not
    /// told apart. The address itself is never stored.
    pub campus_nets: Vec<IpNet>,
    /// the connection kinds a same-account device may open without the PC's password (U3; the
    /// trust list's and the device list's `kinds`)
    pub passwordless_kinds: Vec<String>,
    /// the number of the list key in use (the credential hjviewer-api-list-key), design v3 §5-3
    pub list_kid: u32,
    /// the list keys the apps pin (their HJV_TRUST_PUBKEYS): selfcheck compares the key in use
    pub trust_pubkeys: Vec<(u32, [u8; 32])>,
    /// how often a bound PC asks for its trust list, and how long a list lives (U4)
    pub trust_refresh_secs: i64,
    pub trust_ttl_secs: i64,
}

/// The connection kinds of a same-account proof (design v3 §3-3): port forwarding and RDP never
/// go without a password in 1.6.0 (§3-4).
pub const KINDS: &[&str] = &["remote", "file", "camera", "terminal"];

pub const ENV_PREFIX: &str = "HJV_API_";

/// Every setting with its default; `None` marks the required ones.
pub const SETTINGS: &[(&str, Option<&str>)] = &[
    ("HJV_API_SSO_TOKEN_URL", None),
    ("HJV_API_SSO_USERINFO_URL", None),
    ("HJV_API_SSO_CLIENT_ID", None),
    ("HJV_API_SSO_CONNECT_TIMEOUT_MS", Some("3000")),
    ("HJV_API_SSO_TIMEOUT_MS", Some("8000")),
    ("HJV_API_HBBS_CONSOLE", Some("127.0.0.1:21115")),
    ("HJV_API_HBBS_PUBKEY", Some("")),
    (
        "HJV_API_SOURCE_URL",
        Some("https://github.com/yonsei-projecttf/hjviewer"),
    ),
    ("HJV_API_LIMIT_IP", Some("10/600")),
    ("HJV_API_LIMIT_ID", Some("3/600,5/86400")),
    ("HJV_API_LIMIT_BOUND", Some("3/600,5/86400")),
    ("HJV_API_LIMIT_OK_ID", Some("10/3600,30/86400")),
    ("HJV_API_LIMIT_OK_IP", Some("30/3600")),
    ("HJV_API_LIMIT_SSO", Some("20/60")),
    ("HJV_API_TOKEN_IDLE_DAYS", Some("90")),
    ("HJV_API_TOKEN_MAX_DAYS", Some("365")),
    ("HJV_API_ACCOUNT_DAYS", Some("365")),
    ("HJV_API_DEVICE_UNSEEN_DAYS", Some("180")),
    ("HJV_API_EVENT_DAYS", Some("183")),
    ("HJV_API_SEAL_MAX_SKEW", Some("600")),
    ("HJV_API_RETRY_WINDOW", Some("30")),
    ("HJV_API_FAILURE_DELAY_MS", Some("2000")),
    ("HJV_API_FAILURE_JITTER_MS", Some("500")),
    ("HJV_API_BIND_OS", Some("windows")),
    ("HJV_API_SECOND_FACTOR", Some("off")),
    ("HJV_API_MAX_LOGINS", Some("64")),
    ("HJV_API_CAMPUS_NETS", Some("")),
    ("HJV_API_PASSWORDLESS_KINDS", Some("remote,file,camera")),
    ("HJV_API_LIST_KID", Some("1")),
    ("HJV_API_TRUST_PUBKEYS", Some("")),
    ("HJV_API_TRUST_REFRESH", Some("120")),
    ("HJV_API_TRUST_TTL", Some("86400")),
];

impl Config {
    /// Reads the settings from the process environment.
    pub fn from_env() -> Result<Config, String> {
        let vars: HashMap<String, String> = std::env::vars()
            .filter(|(k, _)| k.starts_with(ENV_PREFIX))
            .collect();
        Config::from_vars(&vars)
    }

    /// Reads the settings from `vars`; names missing there take their defaults.
    pub fn from_vars(vars: &HashMap<String, String>) -> Result<Config, String> {
        for name in vars.keys() {
            if name.starts_with(ENV_PREFIX) && !SETTINGS.iter().any(|(n, _)| n == name) {
                return Err(format!("unknown setting {name}"));
            }
        }
        let get = |name: &str| -> Result<String, String> {
            let default = SETTINGS
                .iter()
                .find(|(n, _)| *n == name)
                .and_then(|(_, d)| *d);
            match vars.get(name).map(|v| v.trim()).filter(|v| !v.is_empty()) {
                Some(v) => Ok(v.to_owned()),
                None => default
                    .map(str::to_owned)
                    .ok_or_else(|| format!("{name} is not set")),
            }
        };
        let num = |name: &str| -> Result<i64, String> {
            get(name)?
                .parse::<i64>()
                .ok()
                .filter(|n| *n > 0)
                .ok_or_else(|| format!("{name} must be a positive whole number"))
        };
        let ms = |name: &str| -> Result<Duration, String> {
            Ok(Duration::from_millis(num(name)? as u64))
        };
        let windows = |name: &str| -> Result<Vec<Window>, String> {
            parse_windows(&get(name)?).map_err(|e| format!("{name}: {e}"))
        };

        let token_url = sso_url("HJV_API_SSO_TOKEN_URL", &get("HJV_API_SSO_TOKEN_URL")?)?;
        let userinfo_url = sso_url(
            "HJV_API_SSO_USERINFO_URL",
            &get("HJV_API_SSO_USERINFO_URL")?,
        )?;
        let hbbs_console: SocketAddr = get("HJV_API_HBBS_CONSOLE")?
            .parse()
            .map_err(|_| "HJV_API_HBBS_CONSOLE must be an address such as 127.0.0.1:21115")?;
        if !hbbs_console.ip().is_loopback() {
            return Err("HJV_API_HBBS_CONSOLE must be a loopback address".into());
        }
        let hbbs_pubkey = match get("HJV_API_HBBS_PUBKEY")?.as_str() {
            "" => None,
            text => Some(
                b64_decode(text)
                    .and_then(|k| <[u8; 32]>::try_from(k).ok())
                    .filter(|k| ed25519_dalek::VerifyingKey::from_bytes(k).is_ok())
                    .ok_or(
                        "HJV_API_HBBS_PUBKEY must be hbbs's public key: the base64 text of its id_ed25519.pub",
                    )?,
            ),
        };
        let limit_sso = windows("HJV_API_LIMIT_SSO")?;
        if limit_sso.len() != 1 {
            return Err("HJV_API_LIMIT_SSO takes one window, such as 20/60".into());
        }
        let failure_delay = Duration::from_millis(
            get("HJV_API_FAILURE_DELAY_MS")?
                .parse::<u64>()
                .map_err(|_| "HJV_API_FAILURE_DELAY_MS must be a whole number")?,
        );
        let failure_jitter = Duration::from_millis(
            get("HJV_API_FAILURE_JITTER_MS")?
                .parse::<u64>()
                .map_err(|_| "HJV_API_FAILURE_JITTER_MS must be a whole number")?,
        );
        if failure_jitter > failure_delay {
            return Err("HJV_API_FAILURE_JITTER_MS cannot exceed HJV_API_FAILURE_DELAY_MS".into());
        }
        let bind_os: Vec<String> = get("HJV_API_BIND_OS")?
            .split(',')
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        let mut campus_nets = Vec::new();
        for part in get("HJV_API_CAMPUS_NETS")?
            .split(',')
            .filter(|p| !p.trim().is_empty())
        {
            campus_nets.push(IpNet::parse(part).ok_or_else(|| {
                format!(
                    "HJV_API_CAMPUS_NETS: {} is not a network such as 192.0.2.0/24 or 2001:db8::/32",
                    part.trim()
                )
            })?);
        }
        let mut passwordless_kinds: Vec<String> = Vec::new();
        for kind in get("HJV_API_PASSWORDLESS_KINDS")?.split(',') {
            let kind = kind.trim().to_ascii_lowercase();
            if !KINDS.contains(&kind.as_str()) {
                return Err(format!(
                    "HJV_API_PASSWORDLESS_KINDS takes {} (comma-separated)",
                    KINDS.join(", ")
                ));
            }
            if !passwordless_kinds.contains(&kind) {
                passwordless_kinds.push(kind);
            }
        }
        let list_kid = u32::try_from(num("HJV_API_LIST_KID")?)
            .ok()
            .filter(|k| *k <= trust::MAX_KID)
            .ok_or("HJV_API_LIST_KID is a key number, 1 to 65535")?;
        let trust_pubkeys = trust::parse_pubkeys(&get("HJV_API_TRUST_PUBKEYS")?).map_err(|e| {
            format!("HJV_API_TRUST_PUBKEYS (the apps' HJV_TRUST_PUBKEYS, <kid>:<base64>,…): {e}")
        })?;
        let trust_refresh_secs = num("HJV_API_TRUST_REFRESH")?;
        if !(trust::MIN_REFRESH..=trust::MAX_REFRESH).contains(&trust_refresh_secs) {
            return Err(format!(
                "HJV_API_TRUST_REFRESH must be {} to {} seconds: the PCs hold it to that",
                trust::MIN_REFRESH,
                trust::MAX_REFRESH
            ));
        }
        let trust_ttl_secs = num("HJV_API_TRUST_TTL")?;
        if trust_ttl_secs > trust::MAX_TTL || trust_ttl_secs < trust_refresh_secs {
            return Err(format!(
                "HJV_API_TRUST_TTL must be at least HJV_API_TRUST_REFRESH and at most {} seconds (72 hours: the apps refuse longer lists)",
                trust::MAX_TTL
            ));
        }
        let config = Config {
            sso: SsoSettings {
                token_url,
                userinfo_url,
                client_id: get("HJV_API_SSO_CLIENT_ID")?,
                connect_timeout: ms("HJV_API_SSO_CONNECT_TIMEOUT_MS")?,
                total_timeout: ms("HJV_API_SSO_TIMEOUT_MS")?,
            },
            hbbs_console,
            hbbs_pubkey,
            source_url: get("HJV_API_SOURCE_URL")?,
            limit_ip: windows("HJV_API_LIMIT_IP")?,
            limit_id: windows("HJV_API_LIMIT_ID")?,
            limit_bound: windows("HJV_API_LIMIT_BOUND")?,
            limit_ok_id: windows("HJV_API_LIMIT_OK_ID")?,
            limit_ok_ip: windows("HJV_API_LIMIT_OK_IP")?,
            limit_sso: limit_sso[0],
            token_idle_secs: num("HJV_API_TOKEN_IDLE_DAYS")? * DAY,
            token_max_secs: num("HJV_API_TOKEN_MAX_DAYS")? * DAY,
            account_retention_secs: num("HJV_API_ACCOUNT_DAYS")? * DAY,
            device_unseen_secs: num("HJV_API_DEVICE_UNSEEN_DAYS")? * DAY,
            event_retention_secs: num("HJV_API_EVENT_DAYS")? * DAY,
            seal_max_skew_secs: num("HJV_API_SEAL_MAX_SKEW")?,
            retry_window_secs: num("HJV_API_RETRY_WINDOW")?,
            failure_delay,
            failure_jitter,
            bind_os,
            second_factor: get("HJV_API_SECOND_FACTOR")?,
            max_logins_in_flight: num("HJV_API_MAX_LOGINS")? as usize,
            campus_nets,
            passwordless_kinds,
            list_kid,
            trust_pubkeys,
            trust_refresh_secs,
            trust_ttl_secs,
        };
        if config.token_idle_secs > config.token_max_secs {
            return Err("HJV_API_TOKEN_IDLE_DAYS cannot exceed HJV_API_TOKEN_MAX_DAYS".into());
        }
        if config.retry_window_secs > config.seal_max_skew_secs {
            return Err("HJV_API_RETRY_WINDOW cannot exceed HJV_API_SEAL_MAX_SKEW".into());
        }
        Ok(config)
    }

    /// The longest window of the login limits kept in the database: older attempts count nowhere.
    pub fn longest_window(&self) -> i64 {
        [
            &self.limit_ip,
            &self.limit_id,
            &self.limit_bound,
            &self.limit_ok_id,
            &self.limit_ok_ip,
        ]
        .iter()
        .flat_map(|windows| windows.iter().map(|w| w.secs))
        .max()
        .unwrap_or(0)
    }

    /// The public key the apps pin under `kid`, if they pin one.
    pub fn pinned(&self, kid: u32) -> Option<[u8; 32]> {
        self.trust_pubkeys
            .iter()
            .find(|(k, _)| *k == kid)
            .map(|(_, key)| *key)
    }

    /// Where a login came from, as the account's other devices see it: "on-campus",
    /// "off-campus", or "" when HJV_API_CAMPUS_NETS is not set or the address is not one.
    pub fn login_net(&self, ip: &str) -> &'static str {
        if self.campus_nets.is_empty() {
            return "";
        }
        match ip.trim().parse::<IpAddr>() {
            Ok(ip) if self.campus_nets.iter().any(|n| n.contains(&ip)) => "on-campus",
            Ok(_) => "off-campus",
            Err(_) => "",
        }
    }
}

/// "3/600,5/86400": three within ten minutes and five within a day.
pub fn parse_windows(text: &str) -> Result<Vec<Window>, String> {
    let mut windows = Vec::new();
    for part in text.split(',') {
        let (count, secs) = part
            .trim()
            .split_once('/')
            .ok_or("expected <count>/<seconds>[,<count>/<seconds>]")?;
        let count: u32 = count
            .trim()
            .parse()
            .ok()
            .filter(|c| *c > 0)
            .ok_or("counts must be positive whole numbers")?;
        let secs: i64 = secs
            .trim()
            .parse()
            .ok()
            .filter(|s| *s > 0 && *s <= 7 * DAY)
            .ok_or("windows must be 1 to 604800 seconds")?;
        windows.push(Window { count, secs });
    }
    Ok(windows)
}

/// The SSO addresses: https, or plain http to this machine only (the fake SSO in tests).
fn sso_url(name: &str, value: &str) -> Result<Uri, String> {
    let uri: Uri = value.parse().map_err(|_| format!("{name} is not a URL"))?;
    let host = uri.host().ok_or_else(|| format!("{name} has no host"))?;
    match uri.scheme_str() {
        Some("https") => Ok(uri),
        Some("http") if is_loopback_host(host) => Ok(uri),
        _ => Err(format!(
            "{name} must be https:// (plain http only to 127.0.0.1, for tests)"
        )),
    }
}

pub fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Reads a KEY=VALUE file like systemd's EnvironmentFile: blank lines and lines starting with '#'
/// are skipped, and a value may be quoted.
pub fn read_env_file(path: &Path) -> Result<HashMap<String, String>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut vars = HashMap::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("{}:{}: expected KEY=VALUE", path.display(), n + 1))?;
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
            .unwrap_or(value);
        vars.insert(key.trim().to_owned(), value.to_owned());
    }
    Ok(vars)
}

/// systemd credential names (LoadCredentialEncrypted= in hjviewer-api.service).
pub const CRED_SSO_SECRET: &str = "hjviewer-sso-client-secret";
pub const CRED_SSO_API_UUID: &str = "hjviewer-sso-api-uuid";
pub const CRED_SEAL_KEY: &str = "hjviewer-api-login-key";
pub const CRED_LOG_KEY: &str = "hjviewer-api-log-key";
pub const CRED_LIST_KEY: &str = "hjviewer-api-list-key";

/// The secrets, read from credential files.
pub struct Secrets {
    /// what follows "Basic " in the token call, as the SSO issued it
    pub sso_basic: Zeroizing<String>,
    pub sso_api_uuid: Zeroizing<String>,
    /// X25519 secret keys that open sealed passwords: the current one and, during a change,
    /// the next or the previous one
    pub seal_keys: Vec<Zeroizing<[u8; 32]>>,
    /// the HMAC key of the tags in logs and records (log.rs)
    pub log_key: Zeroizing<Vec<u8>>,
    /// the 32-byte Ed25519 seed of the list key that signs the trust lists (design v3 §5-3); its
    /// number is HJV_API_LIST_KID
    pub list_key: Zeroizing<[u8; 32]>,
}

/// Reads the credential `name` of `dir`: text, trimmed, not empty.
fn read_credential(dir: &Path, name: &str) -> Result<Zeroizing<String>, String> {
    let bytes = Zeroizing::new(
        std::fs::read(dir.join(name))
            .map_err(|e| format!("credential {name}: {} ({})", e.kind(), dir.display()))?,
    );
    let text = std::str::from_utf8(&bytes).map_err(|_| format!("credential {name} is not text"))?;
    let text = Zeroizing::new(text.trim().to_owned());
    if text.is_empty() {
        return Err(format!("credential {name} is empty"));
    }
    Ok(text)
}

impl Secrets {
    pub fn load(dir: &Path) -> Result<Secrets, String> {
        let read = |name: &str| read_credential(dir, name);
        let seal_keys = parse_seal_keys(&read(CRED_SEAL_KEY)?)
            .map_err(|e| format!("credential {CRED_SEAL_KEY}: {e}"))?;
        let log_key = Zeroizing::new(
            b64_decode(read(CRED_LOG_KEY)?.as_str())
                .filter(|k| k.len() >= 32)
                .ok_or_else(|| {
                    format!("credential {CRED_LOG_KEY} must be base64 of 32 or more bytes")
                })?,
        );
        Ok(Secrets {
            sso_basic: read(CRED_SSO_SECRET)?,
            sso_api_uuid: read(CRED_SSO_API_UUID)?,
            seal_keys,
            log_key,
            list_key: load_list_key(dir)?,
        })
    }
}

/// The list key's seed from the credential hjviewer-api-list-key of `dir` (selfcheck reads it
/// alone, from $CREDENTIALS_DIRECTORY when it runs in the unit).
pub fn load_list_key(dir: &Path) -> Result<Zeroizing<[u8; 32]>, String> {
    parse_list_key(&read_credential(dir, CRED_LIST_KEY)?)
        .map_err(|e| format!("credential {CRED_LIST_KEY}: {e}"))
}

/// One standard-base64 32-byte seed, as `hjv-api keygen list` writes it ('#' comments allowed).
pub fn parse_list_key(text: &str) -> Result<Zeroizing<[u8; 32]>, String> {
    let mut lines = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'));
    let line = lines.next().ok_or("no key")?;
    if lines.next().is_some() {
        return Err("more than one key: the list key in use only".into());
    }
    let bytes = Zeroizing::new(b64_decode(line).ok_or("not base64")?);
    let seed: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| "a list key is a 32-byte seed")?;
    Ok(Zeroizing::new(seed))
}

/// One standard-base64 X25519 secret key per line ('#' comments allowed), as `hjv-api keygen
/// seal` writes it.
pub fn parse_seal_keys(text: &str) -> Result<Vec<Zeroizing<[u8; 32]>>, String> {
    let mut keys = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let bytes = Zeroizing::new(b64_decode(line).ok_or("a line is not base64")?);
        let key: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| "a key is not 32 bytes")?;
        keys.push(Zeroizing::new(key));
    }
    if keys.is_empty() {
        return Err("no key".into());
    }
    Ok(keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> HashMap<String, String> {
        [
            ("HJV_API_SSO_TOKEN_URL", "https://sso.example.ac.kr/token"),
            ("HJV_API_SSO_USERINFO_URL", "https://sso.example.ac.kr/user"),
            ("HJV_API_SSO_CLIENT_ID", "client"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
    }

    #[test]
    fn defaults_follow_the_design() {
        let c = Config::from_vars(&base()).unwrap();
        assert_eq!(
            c.limit_ip,
            vec![Window {
                count: 10,
                secs: 600
            }]
        );
        assert_eq!(
            c.limit_id,
            vec![
                Window {
                    count: 3,
                    secs: 600
                },
                Window {
                    count: 5,
                    secs: 86400
                }
            ]
        );
        assert_eq!(
            c.limit_sso,
            Window {
                count: 20,
                secs: 60
            }
        );
        assert_eq!(c.token_idle_secs, 90 * DAY);
        assert_eq!(c.token_max_secs, 365 * DAY);
        assert_eq!(c.device_unseen_secs, 180 * DAY);
        assert_eq!(c.seal_max_skew_secs, 600);
        assert_eq!(c.retry_window_secs, 30);
        assert_eq!(c.failure_delay, Duration::from_secs(2));
        assert_eq!(c.failure_jitter, Duration::from_millis(500));
        assert_eq!(c.sso.connect_timeout, Duration::from_secs(3));
        assert_eq!(c.sso.total_timeout, Duration::from_secs(8));
        assert_eq!(c.bind_os, vec!["windows"]);
        assert_eq!(c.second_factor, "off");
        assert_eq!(c.hbbs_console, "127.0.0.1:21115".parse().unwrap());
        assert_eq!(c.hbbs_pubkey, None);
        assert_eq!(
            c.limit_ok_id,
            vec![
                Window {
                    count: 10,
                    secs: 3600
                },
                Window {
                    count: 30,
                    secs: 86400
                }
            ]
        );
        assert_eq!(
            c.limit_ok_ip,
            vec![Window {
                count: 30,
                secs: 3600
            }]
        );
        assert_eq!(c.longest_window(), 86400);
        assert!(c.campus_nets.is_empty());
        assert_eq!(c.login_net("192.0.2.1"), "");
        assert_eq!(c.passwordless_kinds, vec!["remote", "file", "camera"]);
        // trust lists (design v3 §5-3, U4): asked every two minutes, a day's life
        assert_eq!(c.list_kid, 1);
        assert!(c.trust_pubkeys.is_empty());
        assert_eq!((c.trust_refresh_secs, c.trust_ttl_secs), (120, 86_400));
    }

    #[test]
    fn trust_list_settings() {
        let k1 = crate::devsig::public_key(&[1; 32]);
        let k2 = crate::devsig::public_key(&[9; 32]);
        let with = |pairs: &[(&str, String)]| {
            let mut vars = base();
            for (k, v) in pairs {
                vars.insert((*k).to_owned(), v.clone());
            }
            Config::from_vars(&vars)
        };
        let pins = format!("{},{}", trust::entry(1, &k1), trust::entry(2, &k2));
        let c = with(&[
            ("HJV_API_TRUST_PUBKEYS", pins.clone()),
            ("HJV_API_LIST_KID", "2".into()),
            ("HJV_API_TRUST_REFRESH", "5".into()),
            ("HJV_API_TRUST_TTL", "30".into()),
        ])
        .unwrap();
        assert_eq!(c.trust_pubkeys, vec![(1, k1), (2, k2)]);
        assert_eq!((c.list_kid, c.pinned(2), c.pinned(3)), (2, Some(k2), None));
        assert_eq!((c.trust_refresh_secs, c.trust_ttl_secs), (5, 30));
        for bad in [
            ("HJV_API_TRUST_PUBKEYS", format!("{pins},")),
            ("HJV_API_TRUST_PUBKEYS", trust::entry(0, &k1)),
            ("HJV_API_LIST_KID", "0".into()),
            ("HJV_API_LIST_KID", "65536".into()),
            ("HJV_API_TRUST_REFRESH", "4".into()),
            ("HJV_API_TRUST_REFRESH", "301".into()),
            ("HJV_API_TRUST_TTL", "259201".into()),
            ("HJV_API_TRUST_TTL", "119".into()),
        ] {
            assert!(with(std::slice::from_ref(&bad)).is_err(), "{bad:?}");
        }
        assert!(with(&[("HJV_API_TRUST_TTL", "259200".into())]).is_ok());
    }

    #[test]
    fn campus_networks_and_kinds() {
        let mut vars = base();
        vars.insert(
            "HJV_API_CAMPUS_NETS".into(),
            "192.0.2.0/24, 2001:db8::/32".into(),
        );
        vars.insert(
            "HJV_API_PASSWORDLESS_KINDS".into(),
            "Remote,file,terminal,file".into(),
        );
        let c = Config::from_vars(&vars).unwrap();
        assert_eq!(c.campus_nets.len(), 2);
        assert_eq!(c.login_net("192.0.2.9"), "on-campus");
        assert_eq!(c.login_net("2001:db8:1::5"), "on-campus");
        assert_eq!(c.login_net("198.51.100.1"), "off-campus");
        assert_eq!(c.login_net("unknown"), "");
        assert_eq!(c.passwordless_kinds, vec!["remote", "file", "terminal"]);
        for (k, v) in [
            ("HJV_API_CAMPUS_NETS", "192.0.2.0/33"),
            ("HJV_API_CAMPUS_NETS", "campus"),
            ("HJV_API_PASSWORDLESS_KINDS", "remote,port-forward"),
            ("HJV_API_PASSWORDLESS_KINDS", "remote,,file"),
        ] {
            let mut vars = base();
            vars.insert(k.into(), v.into());
            assert!(Config::from_vars(&vars).is_err(), "{k}={v}");
        }
    }

    #[test]
    fn hbbs_public_key() {
        let mut vars = base();
        let key = crate::devsig::public_key(&[4; 32]);
        vars.insert("HJV_API_HBBS_PUBKEY".into(), crate::util::b64(&key));
        assert_eq!(Config::from_vars(&vars).unwrap().hbbs_pubkey, Some(key));
        for bad in ["AAAA", "not base64", &crate::util::b64(&[0u8; 31])] {
            vars.insert("HJV_API_HBBS_PUBKEY".into(), bad.to_owned());
            assert!(Config::from_vars(&vars).is_err(), "{bad}");
        }
    }

    #[test]
    fn refuses_bad_settings() {
        let with = |k: &str, v: &str| {
            let mut vars = base();
            vars.insert(k.to_owned(), v.to_owned());
            Config::from_vars(&vars)
        };
        assert!(with("HJV_API_SSO_TOKEN_URL", "http://sso.example.ac.kr/token").is_err());
        assert!(with("HJV_API_SSO_TOKEN_URL", "http://127.0.0.1:18600/token").is_ok());
        assert!(with("HJV_API_SSO_TOKEN_URL", "http://localhost:18600/token").is_ok());
        assert!(with("HJV_API_HBBS_CONSOLE", "10.0.0.1:21115").is_err());
        assert!(with("HJV_API_LIMIT_ID", "3").is_err());
        assert!(with("HJV_API_LIMIT_ID", "0/600").is_err());
        assert!(with("HJV_API_LIMIT_SSO", "20/60,30/120").is_err());
        assert!(with("HJV_API_TOKEN_IDLE_DAYS", "400").is_err());
        assert!(with("HJV_API_TOKEN_MAX_DAYS", "-1").is_err());
        assert!(with("HJV_API_FAILURE_JITTER_MS", "3000").is_err());
        assert!(with("HJV_API_TYPO", "1").is_err());
        let mut vars = base();
        vars.remove("HJV_API_SSO_CLIENT_ID");
        assert!(Config::from_vars(&vars).is_err());
    }

    #[test]
    fn windows_parse() {
        assert_eq!(
            parse_windows(" 3/600 , 5/86400").unwrap(),
            vec![
                Window {
                    count: 3,
                    secs: 600
                },
                Window {
                    count: 5,
                    secs: 86400
                }
            ]
        );
        assert!(parse_windows("3/0").is_err());
        assert!(parse_windows("x/1").is_err());
    }

    #[test]
    fn env_file() {
        let dir = std::env::temp_dir().join(format!("hjv-api-env-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("api.env");
        std::fs::write(&path, "# comment\nA=1\nB = \"two words\"\n\nC='3'\n").unwrap();
        let vars = read_env_file(&path).unwrap();
        assert_eq!(vars["A"], "1");
        assert_eq!(vars["B"], "two words");
        assert_eq!(vars["C"], "3");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_example_file_names_every_setting() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/hjviewer-api.env.example");
        let text = std::fs::read_to_string(path).unwrap();
        for (name, _) in SETTINGS {
            assert!(
                text.contains(&format!("{name}=")),
                "{name} missing in the example"
            );
        }
        // the example works once the client ID is filled in, its commented values are the defaults
        let mut vars = read_env_file(Path::new(path)).unwrap();
        assert!(Config::from_vars(&vars).is_err());
        vars.insert("HJV_API_SSO_CLIENT_ID".into(), "client".into());
        Config::from_vars(&vars).unwrap();
        for line in text.lines().filter_map(|l| l.strip_prefix('#')) {
            if let Some((name, value)) = line.split_once('=') {
                if let Some((_, Some(default))) = SETTINGS.iter().find(|(n, _)| *n == name) {
                    assert_eq!(value, *default, "{name}");
                }
            }
        }
    }

    #[test]
    fn seal_key_lines() {
        let key = crate::util::b64(&[7u8; 32]);
        let keys = parse_seal_keys(&format!("# current\n{key}\n\n{key}\n")).unwrap();
        assert_eq!(keys.len(), 2);
        assert_eq!(*keys[0], [7u8; 32]);
        assert!(parse_seal_keys("").is_err());
        assert!(parse_seal_keys("AAAA").is_err());
    }

    #[test]
    fn list_key_lines() {
        let key = crate::util::b64(&[8u8; 32]);
        assert_eq!(
            *parse_list_key(&format!("# list key 1\n{key}\n")).unwrap(),
            [8u8; 32]
        );
        for bad in [
            String::new(),
            "AAAA".into(),
            format!("{key}\n{key}"),
            crate::util::b64(&[8u8; 64]),
        ] {
            assert!(parse_list_key(&bad).is_err(), "{bad}");
        }
    }
}

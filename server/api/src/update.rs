// SPDX-License-Identifier: AGPL-3.0-only

//! The packages of new app versions (the design of the minimum version and forced update, §4):
//! the Windows exe and the Android APK that the signed version policy names as `latest`. hjv-api
//! checks who asks and sends the file itself, so that neither nginx nor the web site on the same
//! host ever reads a package or one of the codes below.
//!
//! | path | answer |
//! |---|---|
//! | `GET, HEAD /api/hjv/update/{windows,android}/{x.y.z}` | door 1, for the apps of 1.6.0 and later: that platform's package, when `x.y.z` is its `latest` |
//! | `POST /api/hjv/update/{windows,android}/link` | body `{"installed":true}` or `{"installed":false}`: `{"url":"/api/hjv/u/K7M2-Q9XD","file":"HJViewer-1.6.1-install.exe","expires_in":600}`, a short link for the browser of a remote device |
//! | `GET, HEAD /api/hjv/u/{code}` | the short link: the package it was made for, under the name it was made with |
//! | `GET, HEAD /api/hjv/get/{code}/` and `…/{code}/{file}` | door 2, while 1.5.1 is being replaced: a page made from the policy, the packages and their notice files |
//!
//! **What is sent.** The policy is the file nginx hands to the apps (HJV_API_VERSION_POLICY, read
//! again when it changes; its signature is the apps' to check, not hjv-api's, which reads a file
//! only root writes). A package is the file of the policy's `package.file` in
//! HJV_API_PACKAGE_DIR: a plain file there, no link, of exactly the policy's `size`. No name or
//! path comes from a request. Without the policy, the entry, its package or the file the answer is
//! 404. The file is streamed (body.rs) with `Content-Type` by its extension, `Content-Length`,
//! `Content-Disposition: attachment`, `Cache-Control: private, no-store`, `X-Robots-Tag`,
//! `Referrer-Policy: no-referrer`, and for nginx `X-Accel-Buffering: no` (it passes the file on as
//! it comes and writes it to no file of its own) and `X-Accel-Limit-Rate`. hjv-api itself sends a
//! file no faster than 3 840 000 bytes a second, about 30 Mb/s: nginx does not slow down an answer
//! that it passes on as it comes. At most HJV_API_DOWNLOADS_MAX packages go out at once; one more
//! gets 503 with `Retry-After: 60`. A HEAD gets the same head, takes no slot and counts nowhere.
//!
//! **Door 1** takes one of two proofs:
//!
//! - a login: the token with its account key's `X-HJV-Proof`, as every account path (api.rs
//!   `session`, with its 401 and 409);
//! - a PC in an account, without a login (the service of an installed PC updates itself): the
//!   headers `X-HJV-Device` (the PC's ID), `X-HJV-Time` (Unix seconds, decimal) and `X-HJV-Sig`
//!   (standard base64 of an Ed25519 signature) by the PC's device key over
//!
//!   ```text
//!   "hjv-update-v1" 0 <id> 0 <t> 0 <platform> 0 <version>
//!   ```
//!
//!   (0 is a zero byte; `t` as the header has it, the platform and version as the path has
//!   them), checked strictly with the key stored with the PC's binding. The PC's account must
//!   still be on the allowlist, `|t − server time| ≤ 600 s`, and the platform asked for must be
//!   the PC's own (`os` of its binding), so that a PC's key gets the package of the app that PC
//!   runs and nothing else. Whatever of this fails, the answer is the same 403.
//!
//! A short link is made by a login only.
//!
//! **A short link** is eight symbols of Crockford's base32 (40 random bits), shown as
//! `XXXX-XXXX`; read without regard to case, `-` and spaces, with `O` for `0` and `I` or `L` for
//! `1`. It is bound to the account that made it, the platform, the version and the file name
//! (for Windows `HJViewer-<v>-install.exe` for an installed PC, whose packer then opens the
//! installer, else `HJViewer-<v>.exe`), lives ten minutes and three downloads (HEAD is free), and
//! ends when its package is no longer the latest or its account leaves the allowlist. The links
//! are kept in memory: a restart ends them.
//!
//! **Door 2** is open while HJV_API_DOOR2_FILE (`CODE=<32 hex>` and `UNTIL=<UTC time>`, read
//! again when it changes) is there and its time has not come. `…/{code}/` is a page in Korean,
//! made from the policy on each request; the files are `HJViewer-<v>-install.exe` and
//! `HJViewer-<v>.exe` (the one Windows package under two names), the APK under its own name, and
//! `HJViewer-<v>-LICENSE.txt`, `-NOTICE.txt`, `-SOURCE.txt`, `-THIRD_PARTY_NOTICES.txt` from
//! `notices-<v>/` of the package directory. A wrong code, a closed door and a name that is none
//! of these are 404.
//!
//! **Limits** (limits.rs): a request that fails for who asks (no or a wrong token, proof,
//! signature or code) counts against its client address, 30 a day (HJV_API_LIMIT_UPDATE_FAIL_IP);
//! past that, such a request gets 429 instead of its 401, 403 or 404. A request with the right
//! token, signature or code is never refused for its address: people behind one address (a campus
//! network) share it. The HEAD without any of the caller's headers that the app's HTTP client
//! sends first counts nowhere. Downloads that begin count 10 a day (HJV_API_LIMIT_UPDATE) per
//! account for its logins and per PC for its device key; an account makes 20 links a day. Door 2
//! has the download slots only.
//!
//! **Records**: every package sent and every link made is a record of kind `update` (the account
//! and the PC as tags, the address as its tag, never itself) and a line in the journal, where a
//! code shows as the first eight hex digits of its SHA-256. The first download by a PC's key from
//! an address that PC never used is a record of its own (`device-new-address`), a notice in the
//! journal and a line in the operator record (oplog.rs). Door 2's records stop at 20 a minute.

use crate::{
    api::{session, ApiRequest, Reply},
    body::{Body, Slot, Slots, Source, RATE},
    config::{Config, Window},
    db, devsig,
    limits::{
        self, Admit, RecordBudget, SCOPE_UPDATE_FAIL_IP, SCOPE_UPDATE_LINK,
        SCOPE_UPDATE_OK_ACCOUNT, SCOPE_UPDATE_OK_DEVICE,
    },
    oplog::OpEntry,
    state::AppState,
    util::{datetime, day_number, hex, now, random_bytes, sha256, valid_device_id, DAY},
};
use hyper::{
    body::Bytes,
    header::{
        HeaderName, HeaderValue, ACCEPT_RANGES, AUTHORIZATION, CACHE_CONTROL, CONTENT_DISPOSITION,
        CONTENT_LENGTH, CONTENT_SECURITY_POLICY, CONTENT_TYPE, LOCATION, REFERRER_POLICY,
        RETRY_AFTER,
    },
    Method, Response, StatusCode,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs::File,
    io::Read,
    os::unix::fs::MetadataExt,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

/// What a device key signs to download (the context of the signed bytes).
pub const CONTEXT: &[u8] = b"hjv-update-v1";
/// How far the time of a device's request may be off, either way.
pub const MAX_SKEW: i64 = 600;
/// The headers of a device's request.
pub const DEVICE_HEADERS: [&str; 3] = ["x-hjv-device", "x-hjv-time", "x-hjv-sig"];
/// A short link lives this long …
pub const LINK_SECS: i64 = 600;
/// … and this many downloads.
pub const LINK_DOWNLOADS: u8 = 3;
/// The short links an account makes in a day.
pub const LINKS_A_DAY: Window = Window {
    count: 20,
    secs: DAY,
};
/// The short links kept at once; more are not made.
const MAX_LINKS: usize = 1000;
/// What a slot's 503 names as `Retry-After`.
const BUSY_SECS: i64 = 60;
/// The largest package a policy names (the apps read no more).
pub const MAX_PACKAGE: u64 = 200 * 1024 * 1024;
const MAX_POLICY_FILE: u64 = 64 * 1024;
const MAX_DOOR2_FILE: u64 = 4096;
const MAX_NOTICE: u64 = 16 * 1024 * 1024;
/// The notice files of a package, in `notices-<version>/` as `<name>.txt`.
pub const NOTICES: [&str; 4] = ["LICENSE", "NOTICE", "SOURCE", "THIRD_PARTY_NOTICES"];
const NOTICE_EVERY: Duration = Duration::from_secs(600);

const TEXT_BAD_REQUEST: &str = "요청을 이해하지 못했습니다.";
const TEXT_REFUSED: &str = "이 기기는 새 버전을 받을 수 없습니다.";
const TEXT_NO_PACKAGE: &str = "새 버전을 준비하고 있습니다. 잠시 뒤 다시 시도해 주세요.";
const TEXT_NO_LINK: &str = "링크가 맞지 않거나 사용 기한이 지났습니다.";
const TEXT_TOO_OFTEN: &str = "요청이 너무 많습니다. 잠시 뒤 다시 시도해 주세요.";
const TEXT_BUSY: &str = "지금 내려받는 사람이 많습니다. 잠시 뒤 다시 시도해 주세요.";
const TEXT_MOVED: &str = "주소 끝에 /를 붙여 주세요.";

// ---- the policy and door 2's file ----

/// The platforms a package exists for; the others update through their stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Platform {
    Windows,
    Android,
}

impl Platform {
    pub const ALL: [Platform; 2] = [Platform::Windows, Platform::Android];

    pub fn parse(name: &str) -> Option<Platform> {
        Platform::ALL.into_iter().find(|p| p.name() == name)
    }

    /// As the policy, the paths and a binding's `os` name it.
    pub fn name(self) -> &'static str {
        match self {
            Platform::Windows => "windows",
            Platform::Android => "android",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Platform::Windows => ".exe",
            Platform::Android => ".apk",
        }
    }
}

/// An app version as the policy writes it: three numbers without leading zeros, the first two of
/// at most three digits and the last of at most two.
pub fn is_version(text: &str) -> bool {
    let mut fields = text.split('.');
    let mut field = |max: usize| {
        fields.next().is_some_and(|f| {
            !f.is_empty()
                && f.len() <= max
                && f.bytes().all(|b| b.is_ascii_digit())
                && (f == "0" || !f.starts_with('0'))
        })
    };
    field(3) && field(3) && field(2) && fields.next().is_none()
}

/// Letters, digits, `.`, `_` and `-`, at most `max` of them.
fn is_token(text: &str, max: usize) -> bool {
    !text.is_empty()
        && text.len() <= max
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// A package's name in the package directory, by the policy's rules: one path part of the
/// platform's kind, and for Windows a name on which the packer neither opens the installer
/// (`…install.exe`) nor runs as quick support (`-qs-`).
pub fn is_package_file(platform: Platform, name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    is_token(name, 100)
        && !name.starts_with('.')
        && !name.contains("..")
        && lower.ends_with(platform.extension())
        && name.len() > platform.extension().len()
        && !lower.contains("-qs-")
        && !lower.ends_with("install.exe")
}

fn is_sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// `2026-11-22T15:00:00Z`, the one form the policy's tool writes, as Unix seconds.
fn utc_time(text: &str) -> Option<i64> {
    let b = text.as_bytes();
    if b.len() != 20 || b[10] != b'T' || b[13] != b':' || b[16] != b':' || b[19] != b'Z' {
        return None;
    }
    let two = |at: usize| -> Option<i64> {
        (b[at].is_ascii_digit() && b[at + 1].is_ascii_digit())
            .then(|| i64::from(b[at] - b'0') * 10 + i64::from(b[at + 1] - b'0'))
    };
    let (hour, minute, second) = (two(11)?, two(14)?, two(17)?);
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    Some(day_number(&text[..10])? * DAY + hour * 3600 + minute * 60 + second)
}

/// A package as the policy names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    /// its name in the package directory
    pub file: String,
    pub size: u64,
    /// lowercase hex
    pub sha256: String,
}

/// What the policy says of a platform, as far as hjv-api reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub latest: String,
    /// the source tag of `latest`, such as `client-1.6.1`
    pub source: Option<String>,
    pub package: Option<Package>,
}

/// The signed version policy, as far as hjv-api reads it: which package is the latest of each
/// platform. Its signature is not checked here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub epoch: u64,
    pub seq: u64,
    pub windows: Option<Entry>,
    pub android: Option<Entry>,
}

impl Package {
    fn read(platform: Platform, value: &Value) -> Result<Package, &'static str> {
        let file = value
            .get("file")
            .and_then(Value::as_str)
            .filter(|f| is_package_file(platform, f))
            .ok_or("a package's file is not a name of the package directory")?;
        let size = value
            .get("size")
            .and_then(Value::as_u64)
            .filter(|s| (1..=MAX_PACKAGE).contains(s))
            .ok_or("a package's size is not 1 byte to 200 MiB")?;
        let sha256 = value
            .get("sha256")
            .and_then(Value::as_str)
            .filter(|s| is_sha256(s))
            .ok_or("a package's sha256 is not 64 lowercase hex digits")?;
        Ok(Package {
            file: file.to_owned(),
            size,
            sha256: sha256.to_owned(),
        })
    }
}

impl Entry {
    fn read(platform: Platform, value: &Value) -> Result<Entry, &'static str> {
        let latest = value
            .get("latest")
            .and_then(Value::as_str)
            .filter(|v| is_version(v))
            .ok_or("an entry's latest is not a version")?;
        let source = match value.get("source") {
            None => None,
            Some(source) => Some(
                source
                    .as_str()
                    .filter(|s| is_token(s, 64))
                    .ok_or("an entry's source is not a tag name")?
                    .to_owned(),
            ),
        };
        let package = match value.get("package") {
            None => None,
            Some(package) => Some(Package::read(platform, package)?),
        };
        Ok(Entry {
            latest: latest.to_owned(),
            source,
            package,
        })
    }
}

impl Policy {
    /// Reads the policy file: the envelope `{"v":1,"k":…,"p":"<policy text>","s":…}` and, of the
    /// policy text, what hjv-api needs. Anything of that which is not as the policy's rules have
    /// it makes the whole file no policy: nothing is handed out by half a policy.
    pub fn parse(file: &[u8]) -> Result<Policy, &'static str> {
        let envelope: Value = serde_json::from_slice(file).map_err(|_| "the file is not JSON")?;
        if envelope.get("v").and_then(Value::as_u64) != Some(1) {
            return Err("not an envelope of version 1");
        }
        let text = envelope
            .get("p")
            .and_then(Value::as_str)
            .ok_or("the envelope has no policy text")?;
        let policy: Value =
            serde_json::from_str(text).map_err(|_| "the policy text is not JSON")?;
        if policy.get("v").and_then(Value::as_u64) != Some(1) {
            return Err("not a policy of version 1");
        }
        let number = |name: &str| policy.get(name).and_then(Value::as_u64).filter(|n| *n >= 1);
        let (Some(epoch), Some(seq)) = (number("epoch"), number("seq")) else {
            return Err("the policy has no epoch and seq");
        };
        let platforms = policy
            .get("platforms")
            .and_then(Value::as_object)
            .ok_or("the policy has no platforms")?;
        let entry = |platform: Platform| match platforms.get(platform.name()) {
            None => Ok(None),
            Some(value) => Entry::read(platform, value).map(Some),
        };
        Ok(Policy {
            epoch,
            seq,
            windows: entry(Platform::Windows)?,
            android: entry(Platform::Android)?,
        })
    }

    pub fn entry(&self, platform: Platform) -> Option<&Entry> {
        match platform {
            Platform::Windows => self.windows.as_ref(),
            Platform::Android => self.android.as_ref(),
        }
    }

    /// The platform's entry with its package, when it has one.
    fn package(&self, platform: Platform) -> Option<(&Entry, &Package)> {
        let entry = self.entry(platform)?;
        Some((entry, entry.package.as_ref()?))
    }

    fn summary(&self) -> String {
        let latest = |platform| {
            self.package(platform)
                .map_or("no package", |(entry, _)| entry.latest.as_str())
        };
        format!(
            "epoch {}, seq {}; packages: windows {}, android {}",
            self.epoch,
            self.seq,
            latest(Platform::Windows),
            latest(Platform::Android)
        )
    }
}

/// Door 2's file: its code and the time it closes.
#[derive(Clone, PartialEq, Eq)]
pub struct Door2 {
    code: String,
    until: i64,
}

impl Door2 {
    /// Reads the file: a line `CODE=<32 lowercase hex digits>`, a line `UNTIL=<UTC time>`, and
    /// `#` comments. The errors never show what the file holds.
    pub fn parse(file: &[u8]) -> Result<Door2, &'static str> {
        let text = std::str::from_utf8(file).map_err(|_| "the file is not text")?;
        let (mut code, mut until) = (None, None);
        for line in text.lines().map(str::trim) {
            if let Some(value) = line.strip_prefix("CODE=") {
                code = Some(value.trim());
            } else if let Some(value) = line.strip_prefix("UNTIL=") {
                until = Some(utc_time(value.trim()).ok_or("UNTIL is not a time")?);
            } else if !(line.is_empty() || line.starts_with('#')) {
                return Err("a line that is neither CODE=, UNTIL= nor a # comment");
            }
        }
        let (Some(code), Some(until)) = (code, until) else {
            return Err("CODE or UNTIL is missing");
        };
        if code.len() != 32 || !code.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err("CODE is not 32 lowercase hex digits");
        }
        Ok(Door2 {
            code: code.to_owned(),
            until,
        })
    }

    fn summary(&self) -> String {
        format!(
            "open until {} (code={})",
            datetime(self.until),
            code_tag(&self.code)
        )
    }
}

/// How a code shows in the journal: the first eight hex digits of its SHA-256.
fn code_tag(code: &str) -> String {
    hex(&sha256(code.as_bytes())[..4])
}

/// Whether two byte strings are the same, in a time that does not tell where they differ.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A small file of the operator's as it was last read. It is read again when another file is in
/// its place or it was changed (its device, inode, size and time of change).
struct Watched<T> {
    seen: Option<(u64, u64, u64, i64, i64)>,
    read: Option<Arc<T>>,
}

impl<T> Default for Watched<T> {
    fn default() -> Self {
        Watched {
            seen: None,
            read: None,
        }
    }
}

/// What the file at `path` holds now, by `parse`; `None` without the file, and for a file that is
/// too large or not what `parse` takes, which the journal hears of once for each such file.
fn watched<T>(
    kept: &Mutex<Watched<T>>,
    path: &Path,
    max: u64,
    what: &str,
    parse: impl FnOnce(&[u8]) -> Result<T, &'static str>,
    summary: impl FnOnce(&T) -> String,
) -> Option<Arc<T>> {
    let meta = std::fs::metadata(path).ok().filter(|m| m.is_file());
    let seen = meta
        .as_ref()
        .map(|m| (m.dev(), m.ino(), m.len(), m.mtime(), m.mtime_nsec()));
    let mut kept = kept.lock().unwrap_or_else(|e| e.into_inner());
    if seen == kept.seen {
        return kept.read.clone();
    }
    let read = match meta {
        None => {
            log_notice!("update: {what} is gone");
            None
        }
        Some(_) => {
            let mut bytes = Vec::new();
            let read = File::open(path)
                .and_then(|file| file.take(max + 1).read_to_end(&mut bytes))
                .map_err(|_| "it cannot be read")
                .and_then(|_| match bytes.len() as u64 > max {
                    true => Err("it is too large"),
                    false => parse(&bytes),
                });
            match read {
                Ok(value) => {
                    log_notice!("update: {what} read: {}", summary(&value));
                    Some(Arc::new(value))
                }
                Err(why) => {
                    log_err!("update: {what} is not used: {why}");
                    None
                }
            }
        }
    };
    kept.seen = seen;
    kept.read = read.clone();
    read
}

/// Why a file of the package directory is not sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NoFile {
    Missing,
    NotPlain,
    Size,
}

impl NoFile {
    fn text(self) -> &'static str {
        match self {
            NoFile::Missing => "is not in the package directory, or cannot be read there",
            NoFile::NotPlain => "is not a plain file in the package directory",
            NoFile::Size => "has another size than the policy says",
        }
    }
}

/// Opens the plain file at `path`: nothing else, and no link to one. With `size`, a file of
/// exactly that size.
fn open_plain(path: &Path, size: Option<u64>) -> Result<(File, u64), NoFile> {
    let named = std::fs::symlink_metadata(path).map_err(|_| NoFile::Missing)?;
    if !named.is_file() {
        return Err(NoFile::NotPlain);
    }
    let file = File::open(path).map_err(|_| NoFile::Missing)?;
    let opened = file.metadata().map_err(|_| NoFile::Missing)?;
    // the file that was opened is the one that was looked at
    if !opened.is_file() || (opened.dev(), opened.ino()) != (named.dev(), named.ino()) {
        return Err(NoFile::NotPlain);
    }
    match size {
        Some(size) if opened.len() != size => Err(NoFile::Size),
        _ => Ok((file, opened.len())),
    }
}

/// Opens a notice file of the package of `version`: `notices-<version>/<name>.txt`, a plain file
/// in a directory that is no link.
fn open_notice(dir: &Path, version: &str, name: &str) -> Option<(File, u64)> {
    let notices = dir.join(format!("notices-{version}"));
    if !std::fs::symlink_metadata(&notices).is_ok_and(|m| m.is_dir()) {
        return None;
    }
    open_plain(&notices.join(format!("{name}.txt")), None)
        .ok()
        .filter(|(_, size)| *size <= MAX_NOTICE)
}

// ---- names ----

/// The two names a browser gets the Windows package under: the first for a PC with HJViewer
/// installed (the packer opens the installer for a name that ends in `install.exe`), the second
/// for running it without installing. The version is in the name because a browser that saves
/// the same name twice appends " (1)", and that name no longer ends in `install.exe`.
fn windows_names(version: &str) -> [String; 2] {
    [
        format!("HJViewer-{version}-install.exe"),
        format!("HJViewer-{version}.exe"),
    ]
}

/// The name a download of the package gets: for Windows by whether HJViewer is installed where
/// it goes, for Android the package's own.
fn download_name(platform: Platform, version: &str, stored: &str, installed: bool) -> String {
    match platform {
        Platform::Windows => {
            let [install, run] = windows_names(version);
            if installed {
                install
            } else {
                run
            }
        }
        Platform::Android => stored.to_owned(),
    }
}

/// Door 2's name of a notice file.
fn notice_name(version: &str, notice: &str) -> String {
    format!("HJViewer-{version}-{notice}.txt")
}

/// The `Content-Type` of a file, by the extension of its name.
pub fn content_type(name: &str) -> &'static str {
    match name.rsplit_once('.').map(|(_, extension)| extension) {
        Some("exe") => "application/vnd.microsoft.portable-executable",
        Some("apk") => "application/vnd.android.package-archive",
        Some("html") => "text/html; charset=utf-8",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

// ---- short links ----

/// Crockford's base32: no I, L, O or U.
const SYMBOLS: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// The eight symbols of 40 bits.
fn code_of(bits: [u8; 5]) -> String {
    let n = bits.iter().fold(0u64, |n, b| n << 8 | u64::from(*b));
    (0..8)
        .map(|i| char::from(SYMBOLS[(n >> (35 - 5 * i) & 31) as usize]))
        .collect()
}

/// A code as a link shows it: `XXXX-XXXX`.
fn shown(code: &str) -> String {
    format!("{}-{}", &code[..4], &code[4..])
}

/// The code a person typed, or a browser sent for it: without regard to case, `-` and spaces (a
/// browser sends a space as `%20`), with `O` read as `0` and `I` and `L` as `1`, as Crockford's
/// alphabet is read. `None` for what is not eight symbols then.
pub fn typed_code(typed: &str) -> Option<String> {
    if typed.len() > 64 {
        return None;
    }
    let mut code = String::with_capacity(8);
    let mut bytes = typed.bytes();
    while let Some(mut b) = bytes.next() {
        if b == b'%' {
            let digit = |b: Option<u8>| char::from(b?).to_digit(16);
            b = (digit(bytes.next())? * 16 + digit(bytes.next())?) as u8;
        }
        let symbol = match b.to_ascii_uppercase() {
            b'-' | b' ' => continue,
            b'O' => b'0',
            b'I' | b'L' => b'1',
            other => other,
        };
        if !SYMBOLS.contains(&symbol) {
            return None;
        }
        code.push(char::from(symbol));
    }
    (code.len() == 8).then_some(code)
}

/// A short link: who made it and what it hands out.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Link {
    account_id: i64,
    account_tag: String,
    platform: Platform,
    version: String,
    /// the package's name in the package directory
    stored: String,
    /// the name the download gets
    name: String,
    expires: i64,
    left: u8,
}

/// What the update paths keep between requests.
pub struct Updates {
    policy: Mutex<Watched<Policy>>,
    door2: Mutex<Watched<Door2>>,
    links: Mutex<HashMap<String, Link>>,
    /// the packages that may go out at once (HJV_API_DOWNLOADS_MAX)
    pub slots: Slots,
    /// door 2's downloads written to the records per minute: whoever has its link downloads as
    /// often as they like
    door2_records: RecordBudget,
    /// a public key whose secret key nobody has: what the signature of a PC that is in no
    /// account is checked against, so that its answer takes as long as a bound PC's
    nobody: [u8; 32],
}

impl Updates {
    pub fn new(config: &Config) -> Updates {
        Updates {
            policy: Mutex::default(),
            door2: Mutex::default(),
            links: Mutex::default(),
            slots: Slots::new(config.downloads_max),
            door2_records: RecordBudget::default(),
            nobody: devsig::public_key(&random_bytes::<32>()),
        }
    }

    /// The policy as the file has it now (it reads a file: not for an async thread).
    fn policy(&self, config: &Config) -> Option<Arc<Policy>> {
        watched(
            &self.policy,
            &config.version_policy,
            MAX_POLICY_FILE,
            "the version policy (HJV_API_VERSION_POLICY)",
            Policy::parse,
            Policy::summary,
        )
    }

    /// Door 2's file as it is now (it reads a file: not for an async thread).
    fn door2(&self, config: &Config) -> Option<Arc<Door2>> {
        watched(
            &self.door2,
            &config.door2_file,
            MAX_DOOR2_FILE,
            "door 2's file (HJV_API_DOOR2_FILE)",
            Door2::parse,
            Door2::summary,
        )
    }

    fn links(&self) -> std::sync::MutexGuard<'_, HashMap<String, Link>> {
        self.links.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Keeps a new link and returns its code; `None` while too many are kept.
    fn add_link(&self, link: Link, at: i64) -> Option<String> {
        let mut links = self.links();
        links.retain(|_, l| at < l.expires);
        if links.len() >= MAX_LINKS {
            return None;
        }
        loop {
            let code = code_of(random_bytes::<5>());
            if !links.contains_key(&code) {
                links.insert(code.clone(), link);
                return Some(code);
            }
        }
    }

    /// The link of `code`, while it lives.
    fn link(&self, code: &str, at: i64) -> Option<Link> {
        self.links()
            .get(code)
            .filter(|l| at < l.expires && l.left > 0)
            .cloned()
    }

    /// Takes one download of the link of `code`; how many it has left then, `None` when it does
    /// not live any more.
    fn use_link(&self, code: &str, at: i64) -> Option<u8> {
        let mut links = self.links();
        let link = links
            .get_mut(code)
            .filter(|l| at < l.expires && l.left > 0)?;
        link.left -= 1;
        let left = link.left;
        if left == 0 {
            links.remove(code);
        }
        Some(left)
    }

    fn drop_link(&self, code: &str) {
        self.links().remove(code);
    }

    /// The links kept now, past their time or not (tests).
    pub fn links_kept(&self) -> usize {
        self.links().len()
    }

    /// For tests: makes every link `secs` older (in service a link simply runs out).
    pub fn age_links(&self, secs: i64) {
        for link in self.links().values_mut() {
            link.expires -= secs;
        }
    }
}

// ---- answers ----

/// What an answer of this module sends with its JSON, or in its place: headers of its own, and a
/// file or door 2's page.
#[derive(Clone)]
pub struct Sent {
    headers: Vec<(HeaderName, HeaderValue)>,
    content: Option<Content>,
}

#[derive(Clone)]
enum Content {
    Page(Bytes),
    File(Arc<Source>),
}

impl std::fmt::Debug for Sent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let content = match &self.content {
            None => "the JSON".to_owned(),
            Some(Content::Page(page)) => format!("a page of {} bytes", page.len()),
            Some(Content::File(source)) => format!("a file of {} bytes", source.size()),
        };
        write!(f, "Sent({} headers, {content})", self.headers.len())
    }
}

impl Sent {
    /// Puts the headers over those of the JSON answer `response`, and the content, when there is
    /// some, in place of its body, with its length (the head alone for a HEAD).
    pub fn into_response(self, response: &mut Response<Body>, head: bool) {
        for (name, value) in self.headers {
            response.headers_mut().insert(name, value);
        }
        let Some(content) = self.content else {
            return;
        };
        let (size, body) = match content {
            Content::Page(page) => (page.len() as u64, Body::from(page)),
            Content::File(source) => (source.size(), Body::file(source)),
        };
        response
            .headers_mut()
            .insert(CONTENT_LENGTH, HeaderValue::from(size));
        *response.body_mut() = if head { Body::empty() } else { body };
    }
}

fn header(name: &'static str, value: &'static str) -> (HeaderName, HeaderValue) {
    (
        HeaderName::from_static(name),
        HeaderValue::from_static(value),
    )
}

/// The headers that keep an answer out of caches and search engines and its address out of
/// `Referer` headers.
fn private_headers() -> Vec<(HeaderName, HeaderValue)> {
    vec![
        (CACHE_CONTROL, HeaderValue::from_static("private, no-store")),
        header("x-robots-tag", "noindex, nofollow, noarchive"),
        (REFERRER_POLICY, HeaderValue::from_static("no-referrer")),
    ]
}

/// `reply` with `headers` over its own.
fn with_headers(mut reply: Reply, headers: Vec<(HeaderName, HeaderValue)>) -> Reply {
    match &mut reply.sent {
        Some(sent) => sent.headers.extend(headers),
        None => {
            reply.sent = Some(Sent {
                headers,
                content: None,
            })
        }
    }
    reply
}

/// An answer on a path that a browser opens: also an error there stays out of caches and search
/// engines.
fn browser(reply: Reply) -> Reply {
    with_headers(reply, private_headers())
}

/// A file as the answer: `name` is what a download is saved as; a notice file shows in the
/// browser.
fn file_reply(file: File, size: u64, slot: Option<Slot>, name: &str, attachment: bool) -> Reply {
    let kind = if attachment { "attachment" } else { "inline" };
    let disposition = HeaderValue::from_str(&format!("{kind}; filename=\"{name}\""))
        .unwrap_or(HeaderValue::from_static("attachment"));
    let mut headers = private_headers();
    headers.extend([
        (CONTENT_TYPE, HeaderValue::from_static(content_type(name))),
        (CONTENT_DISPOSITION, disposition),
        (ACCEPT_RANGES, HeaderValue::from_static("none")),
        // nginx passes the file on as it comes and writes it to no file of its own. hjv-api
        // sends it no faster than RATE (body.rs); nginx holds an answer to the rate named here
        // only where it does not follow the line above
        header("x-accel-buffering", "no"),
        (
            HeaderName::from_static("x-accel-limit-rate"),
            HeaderValue::from(RATE),
        ),
    ]);
    Reply {
        sent: Some(Sent {
            headers,
            content: Some(Content::File(Source::new(file, size, slot))),
        }),
        ..Reply::ok(&json!({}))
    }
}

fn bad_request() -> Reply {
    Reply::error(StatusCode::BAD_REQUEST, TEXT_BAD_REQUEST)
}

fn no_package() -> Reply {
    Reply::error(StatusCode::NOT_FOUND, TEXT_NO_PACKAGE)
}

fn no_link() -> Reply {
    Reply::error(StatusCode::NOT_FOUND, TEXT_NO_LINK)
}

/// 429 or 503 with `Retry-After`.
fn later(status: StatusCode, text: &str, code: &str, secs: i64) -> Reply {
    let secs = secs.clamp(1, DAY);
    with_headers(
        Reply::json(
            status,
            &json!({ "error": text, "hjv": code, "retry_after": secs }),
        ),
        vec![(RETRY_AFTER, HeaderValue::from(secs))],
    )
}

fn too_often(secs: i64) -> Reply {
    later(
        StatusCode::TOO_MANY_REQUESTS,
        TEXT_TOO_OFTEN,
        "rate_limited",
        secs,
    )
}

/// Every download slot is taken.
fn busy() -> Reply {
    later(
        StatusCode::SERVICE_UNAVAILABLE,
        TEXT_BUSY,
        "busy",
        BUSY_SECS,
    )
}

// ---- the paths ----

/// What a path of this module asks for.
enum Asked<'a> {
    /// door 1: the platform and the version
    Package(&'a str, &'a str),
    /// a short link for the platform
    Link(&'a str),
    /// a short link's code, as typed
    Short(&'a str),
    /// door 2: the code and, after its `/`, the file ("" for the page)
    Door2(&'a str, Option<&'a str>),
}

fn asked(path: &str) -> Option<Asked<'_>> {
    let one = |part: &str| !part.is_empty() && !part.contains('/');
    if let Some(rest) = path.strip_prefix("/api/hjv/update/") {
        let (platform, last) = rest.split_once('/')?;
        return (one(platform) && one(last)).then_some(match last {
            "link" => Asked::Link(platform),
            version => Asked::Package(platform, version),
        });
    }
    if let Some(code) = path.strip_prefix("/api/hjv/u/") {
        return one(code).then_some(Asked::Short(code));
    }
    let rest = path.strip_prefix("/api/hjv/get/")?;
    let (code, file) = match rest.split_once('/') {
        Some((code, file)) => (code, Some(file)),
        None => (rest, None),
    };
    (one(code) && file.is_none_or(|f| !f.contains('/'))).then_some(Asked::Door2(code, file))
}

/// The methods of the paths this module answers, for 405 answers; `None` for others.
pub fn allowed_methods(path: &str) -> Option<&'static str> {
    Some(match asked(path)? {
        Asked::Link(_) => "POST",
        Asked::Package(..) | Asked::Short(_) | Asked::Door2(..) => "GET, HEAD",
    })
}

/// Whether this module answers `method` on `path`.
pub fn takes(path: &str, method: &Method) -> bool {
    allowed_methods(path).is_some_and(|allowed| allowed.split(", ").any(|m| m == method.as_str()))
}

/// The paths a browser opens, whose key is the code in the path: a request with an `Origin` is
/// taken there too (api.rs `route`).
pub fn browser_path(path: &str) -> bool {
    matches!(asked(path), Some(Asked::Short(_) | Asked::Door2(..)))
}

/// Dispatches the paths of [`allowed_methods`] (the caller checked the method).
pub async fn route(state: &Arc<AppState>, req: &ApiRequest) -> Reply {
    match asked(&req.path) {
        Some(Asked::Package(platform, version)) => package(state, req, platform, version).await,
        Some(Asked::Link(platform)) => make_link(state, req, platform).await,
        Some(Asked::Short(code)) => browser(short_link(state, req, code).await),
        Some(Asked::Door2(code, file)) => browser(door2(state, req, code, file).await),
        None => Reply::not_found(),
    }
}

/// Runs what reads files on a thread for that; `None` when it did not end.
async fn off_thread<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    tokio::task::spawn_blocking(work).await.ok()
}

/// Counts one more under `scope` and `key` now, or says how many seconds a full limit takes; the
/// answer to send when the database fails.
async fn count(
    state: &Arc<AppState>,
    scope: &'static str,
    key: String,
    windows: Vec<Window>,
) -> Result<Result<(), i64>, Reply> {
    let at = now();
    state
        .db
        .call(move |c| db::in_tx(c, |tx| limits::count(tx, scope, &key, &windows, at)))
        .await
        .map_err(|e| {
            log_err!("update: {e}");
            Reply::server_error()
        })
}

/// A request that failed for who asked: `reply`, counted against its address; 429 instead once
/// the address has had its failures. `what` names for the journal what was missing or wrong.
async fn failed(state: &Arc<AppState>, req: &ApiRequest, reply: Reply, what: &str) -> Reply {
    let ip_tag = state.tags.ip(&req.ip);
    let windows = state.config.limit_update_fail_ip.clone();
    match count(state, SCOPE_UPDATE_FAIL_IP, ip_tag.clone(), windows).await {
        Ok(Ok(())) => {
            if state.first_notice(&format!("update:failed:{what}:{ip_tag}"), NOTICE_EVERY) {
                log_info!(
                    "update: refused, {what} (ip={ip_tag}); the next ten minutes are not logged"
                );
            }
            reply
        }
        Ok(Err(wait)) => {
            if state.first_notice(&format!("update:failures:{ip_tag}"), NOTICE_EVERY) {
                log_notice!(
                    "update: an address has had its failed requests (ip={ip_tag}): 429 for more of them, while a right token, signature or code still passes; the next ten minutes are not logged"
                );
            }
            too_often(wait)
        }
        Err(reply) => reply,
    }
}

/// A package ready to be sent.
struct Opened {
    file: File,
    size: u64,
    version: String,
    /// its name in the package directory
    stored: String,
}

/// The journal hears, once in ten minutes, of a package the policy names that cannot be sent.
fn complain(state: &AppState, platform: Platform, why: NoFile) {
    let name = platform.name();
    if state.first_notice(&format!("update:file:{name}"), NOTICE_EVERY) {
        log_err!(
            "update: the {name} package the policy names {}: nobody gets it (the next ten minutes are not logged)",
            why.text()
        );
    }
}

/// The package of the platform's latest version, opened: when the policy has one and its file is
/// there as the policy says, and, with `version`, when that is the latest.
async fn latest(
    state: &Arc<AppState>,
    platform: Platform,
    version: Option<String>,
) -> Option<Opened> {
    let state = state.clone();
    off_thread(move || {
        let policy = state.updates.policy(&state.config)?;
        let (entry, package) = policy.package(platform)?;
        if version.is_some_and(|version| version != entry.latest) {
            return None;
        }
        let path = state.config.package_dir.join(&package.file);
        match open_plain(&path, Some(package.size)) {
            Ok((file, size)) => Some(Opened {
                file,
                size,
                version: entry.latest.clone(),
                stored: package.file.clone(),
            }),
            Err(why) => {
                complain(&state, platform, why);
                None
            }
        }
    })
    .await
    .flatten()
}

/// Who asks at door 1, as tags.
enum Who {
    Account(String),
    /// a PC and its account
    Device(String, String),
}

/// Door 1.
async fn package(state: &Arc<AppState>, req: &ApiRequest, platform: &str, version: &str) -> Reply {
    let Some(platform) = Platform::parse(platform).filter(|_| is_version(version)) else {
        return bad_request();
    };
    let head = req.method == Method::HEAD;
    let has_token = req.headers.contains_key(AUTHORIZATION);
    let by_device = !has_token && DEVICE_HEADERS.iter().any(|h| req.headers.contains_key(*h));
    let who = if by_device {
        match device(state, req, platform, version).await {
            Ok((device_tag, account_tag)) => Who::Device(device_tag, account_tag),
            Err(reply) => return reply,
        }
    } else {
        match session(state, req).await {
            Ok(s) => Who::Account(state.tags.account(&s.account.login_key)),
            // the first HEAD of the app's HTTP client carries none of its caller's headers
            Err(reply) if head && !has_token => return reply,
            Err(reply) if reply.status == StatusCode::UNAUTHORIZED => {
                return failed(state, req, reply, "no login with its proof").await
            }
            Err(reply) => return reply,
        }
    };
    let Some(opened) = latest(state, platform, Some(version.to_owned())).await else {
        return no_package();
    };
    let name = download_name(platform, &opened.version, &opened.stored, false);
    if head {
        return file_reply(opened.file, opened.size, None, &name, true);
    }
    let Some(slot) = state.updates.slots.take() else {
        return busy();
    };
    // a download counts when it begins
    let (scope, key) = match &who {
        Who::Account(account) => (SCOPE_UPDATE_OK_ACCOUNT, account.clone()),
        Who::Device(device, _) => (SCOPE_UPDATE_OK_DEVICE, device.clone()),
    };
    match count(state, scope, key.clone(), state.config.limit_update.clone()).await {
        Ok(Ok(())) => {}
        Ok(Err(wait)) => {
            if state.first_notice(&format!("update:limit:{key}"), NOTICE_EVERY) {
                log_notice!(
                    "update: one account's or PC's downloads of a day reached ({scope} {key}); the next ten minutes are not logged"
                );
            }
            return too_often(wait);
        }
        Err(reply) => return reply,
    }
    let ip_tag = state.tags.ip(&req.ip);
    let (name_of, version_of) = (platform.name(), &opened.version);
    match who {
        Who::Account(account) => {
            log_info!("update: {name_of} {version_of} to a login (acct={account}, ip={ip_tag})");
            let result = match platform {
                Platform::Windows => "token-windows",
                Platform::Android => "token-android",
            };
            state
                .record("update", Some(account), None, Some(ip_tag), result)
                .await;
        }
        Who::Device(device, account) => {
            log_info!(
                "update: {name_of} {version_of} to a PC (acct={account}, dev={device}, ip={ip_tag})"
            );
            device_record(state, platform, device, account, ip_tag).await;
        }
    }
    file_reply(opened.file, opened.size, Some(slot), &name, true)
}

/// The bytes a device key signs to download.
pub fn device_message(id: &str, t: &str, platform: &str, version: &str) -> Vec<u8> {
    let fields: [&[u8]; 5] = [
        CONTEXT,
        id.as_bytes(),
        t.as_bytes(),
        platform.as_bytes(),
        version.as_bytes(),
    ];
    fields.join(&0u8)
}

/// The time of a device's request as its header has it: Unix seconds in decimal, without a sign
/// or zeros in front.
fn device_time(text: &str) -> Option<i64> {
    let decimal = !text.is_empty()
        && text.len() <= 18
        && text.bytes().all(|b| b.is_ascii_digit())
        && (text == "0" || !text.starts_with('0'));
    decimal.then(|| text.parse().ok()).flatten()
}

/// Whether a request signed at `t` is in time at `at`: within [`MAX_SKEW`] either way.
fn in_time(t: i64, at: i64) -> bool {
    at.abs_diff(t) <= MAX_SKEW.unsigned_abs()
}

/// The PC that signed the request, as its tag and its account's: a PC in an account that is on
/// the allowlist, whose stored key signed this platform and version at about this time, and whose
/// own platform it is. Else the 403 that is the same whatever was wrong, counted against the
/// address.
async fn device(
    state: &Arc<AppState>,
    req: &ApiRequest,
    platform: Platform,
    version: &str,
) -> Result<(String, String), Reply> {
    let text = |name: &str| req.headers.get(name).and_then(|v| v.to_str().ok());
    let id = text(DEVICE_HEADERS[0]).filter(|id| valid_device_id(id));
    let (id, t, sig) = (
        id.unwrap_or_default(),
        text(DEVICE_HEADERS[1]).unwrap_or_default(),
        text(DEVICE_HEADERS[2]).unwrap_or_default(),
    );
    let at = now();
    let found = state
        .db
        .call({
            let id = id.to_owned();
            move |c| {
                let Some(device) = db::device(c, &id)? else {
                    return Ok(None);
                };
                let account = db::account(c, device.account_id)?;
                let listed = match &account {
                    Some(account) => db::allowed(c, &account.login_key)?,
                    None => false,
                };
                Ok(Some((device, account, listed)))
            }
        })
        .await
        .map_err(|e| {
            log_err!("update: {e}");
            Reply::server_error()
        })?;
    // a PC in no account is checked against a key nobody has, so that it takes as long
    let key = found
        .as_ref()
        .map_or(&state.updates.nobody[..], |(device, ..)| &device.public_key);
    let signed = devsig::verify(key, &device_message(id, t, platform.name(), version), sig);
    let in_time = device_time(t).is_some_and(|t| in_time(t, at));
    let wrong = match &found {
        None => "a PC in no account",
        Some(_) if !signed => "a signature that is not the PC's",
        Some(_) if !in_time => "a signature of another time",
        Some((_, _, false)) => "a PC whose account is off the allowlist",
        Some((device, ..)) if device.os != platform.name() => "a PC asking for another platform",
        Some((device, Some(account), true)) => {
            return Ok((
                state.tags.device(&device.device_id),
                state.tags.account(&account.login_key),
            ))
        }
        Some(_) => "a PC without its account",
    };
    let refused = Reply::error(StatusCode::FORBIDDEN, TEXT_REFUSED);
    Err(failed(state, req, refused, wrong).await)
}

/// The record of a download by a PC's key. When the PC never downloaded from this address
/// before, one more record, a line in the journal and a line in the operator record (a PC's key
/// that someone took along shows as an address the PC was never at).
async fn device_record(
    state: &Arc<AppState>,
    platform: Platform,
    device: String,
    account: String,
    ip_tag: String,
) {
    let known = state
        .db
        .call({
            let (device, ip_tag) = (device.clone(), ip_tag.clone());
            move |c| {
                c.query_row(
                    "SELECT EXISTS (SELECT 1 FROM events
                                    WHERE kind = 'update' AND device_tag = ?1 AND ip = ?2)",
                    [device, ip_tag],
                    |r| r.get::<_, bool>(0),
                )
            }
        })
        .await;
    let result = match platform {
        Platform::Windows => "device-windows",
        Platform::Android => "device-android",
    };
    let record = |result| {
        state.record(
            "update",
            Some(account.clone()),
            Some(device.clone()),
            Some(ip_tag.clone()),
            result,
        )
    };
    record(result).await;
    if matches!(known, Ok(false)) {
        log_notice!(
            "update: a PC's key downloads from an address it did not use before (acct={account}, dev={device}, ip={ip_tag})"
        );
        record("device-new-address").await;
        if let Some((operator_record, uid)) = state.operator_record.get().cloned() {
            let targets = vec![device.clone(), ip_tag.clone()];
            let written = off_thread(move || {
                operator_record.append(&OpEntry {
                    uid,
                    sudo_user: "",
                    command: "update device-new-address",
                    targets: &targets,
                    reveal: false,
                })
            })
            .await;
            if !matches!(written, Some(Ok(()))) {
                log_err!("update: the operator record did not take the line of a PC's new address");
            }
        }
    }
}

/// What the body of a link request says of `installed`: `{}` (or nothing) is false; `None` when
/// the body is not that.
fn installed(req: &ApiRequest) -> Option<bool> {
    let body = req.body.as_ref().ok()?;
    if body.is_empty() {
        return Some(false);
    }
    let value: Value = serde_json::from_slice(body).ok()?;
    match value.as_object()?.get("installed") {
        None => Some(false),
        Some(installed) => installed.as_bool(),
    }
}

/// `POST /api/hjv/update/{platform}/link`.
async fn make_link(state: &Arc<AppState>, req: &ApiRequest, platform: &str) -> Reply {
    let Some(platform) = Platform::parse(platform) else {
        return bad_request();
    };
    let s = match session(state, req).await {
        Ok(s) => s,
        Err(reply) if reply.status == StatusCode::UNAUTHORIZED => {
            return failed(state, req, reply, "no login with its proof").await
        }
        Err(reply) => return reply,
    };
    let Some(installed) = installed(req) else {
        return bad_request();
    };
    let Some(opened) = latest(state, platform, None).await else {
        return no_package();
    };
    let account_tag = state.tags.account(&s.account.login_key);
    match count(
        state,
        SCOPE_UPDATE_LINK,
        account_tag.clone(),
        vec![LINKS_A_DAY],
    )
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(wait)) => return too_often(wait),
        Err(reply) => return reply,
    }
    let at = now();
    let name = download_name(platform, &opened.version, &opened.stored, installed);
    let link = Link {
        account_id: s.account.id,
        account_tag: account_tag.clone(),
        platform,
        version: opened.version.clone(),
        stored: opened.stored,
        name: name.clone(),
        expires: at + LINK_SECS,
        left: LINK_DOWNLOADS,
    };
    let Some(code) = state.updates.add_link(link, at) else {
        return busy();
    };
    log_info!(
        "update: a short link for {} {} (acct={account_tag}, code={})",
        platform.name(),
        opened.version,
        code_tag(&code)
    );
    let result = match platform {
        Platform::Windows => "link-made-windows",
        Platform::Android => "link-made-android",
    };
    let ip_tag = state.tags.ip(&req.ip);
    state
        .record("update", Some(account_tag), None, Some(ip_tag), result)
        .await;
    Reply::ok(&json!({
        "url": format!("/api/hjv/u/{}", shown(&code)),
        "file": name,
        "expires_in": LINK_SECS,
    }))
}

/// `GET, HEAD /api/hjv/u/{code}`.
async fn short_link(state: &Arc<AppState>, req: &ApiRequest, typed: &str) -> Reply {
    const WRONG: &str = "no short link of that code";
    let at = now();
    let found = typed_code(typed).and_then(|code| {
        let link = state.updates.link(&code, at)?;
        Some((code, link))
    });
    let Some((code, link)) = found else {
        return failed(state, req, no_link(), WRONG).await;
    };
    // the account that made it is still on the allowlist
    let account_id = link.account_id;
    let listed = state
        .db
        .call(move |c| {
            Ok(match db::account(c, account_id)? {
                Some(account) => db::allowed(c, &account.login_key)?,
                None => false,
            })
        })
        .await;
    match listed {
        Ok(true) => {}
        Ok(false) => {
            state.updates.drop_link(&code);
            return failed(state, req, no_link(), WRONG).await;
        }
        Err(e) => {
            log_err!("update: {e}");
            return Reply::server_error();
        }
    }
    // and its package is still the latest
    let opened = latest(state, link.platform, Some(link.version.clone()))
        .await
        .filter(|opened| opened.stored == link.stored);
    let Some(opened) = opened else {
        return no_link();
    };
    if req.method == Method::HEAD {
        return file_reply(opened.file, opened.size, None, &link.name, true);
    }
    let Some(slot) = state.updates.slots.take() else {
        return busy();
    };
    let Some(left) = state.updates.use_link(&code, at) else {
        return no_link();
    };
    let ip_tag = state.tags.ip(&req.ip);
    log_info!(
        "update: {} {} by a short link (acct={}, ip={ip_tag}, code={}, {left} left)",
        link.platform.name(),
        link.version,
        link.account_tag,
        code_tag(&code)
    );
    let result = match link.platform {
        Platform::Windows => "link-windows",
        Platform::Android => "link-android",
    };
    state
        .record("update", Some(link.account_tag), None, Some(ip_tag), result)
        .await;
    file_reply(opened.file, opened.size, Some(slot), &link.name, true)
}

/// What a file name of door 2 stands for.
enum Door2File {
    Package(Platform, Opened),
    Notice(File, u64),
}

/// `GET, HEAD /api/hjv/get/{code}`, `…/{code}/` and `…/{code}/{file}`.
async fn door2(state: &Arc<AppState>, req: &ApiRequest, code: &str, file: Option<&str>) -> Reply {
    let at = now();
    let door = {
        let state = state.clone();
        off_thread(move || state.updates.door2(&state.config))
            .await
            .flatten()
    };
    let open =
        door.is_some_and(|door| same(door.code.as_bytes(), code.as_bytes()) && at < door.until);
    if !open {
        return failed(
            state,
            req,
            no_link(),
            "not door 2's code, or door 2 is closed",
        )
        .await;
    }
    let name = match file {
        // the page's links are relative: it lives at the address that ends in /
        None => {
            let to = HeaderValue::from_str(&format!("/api/hjv/get/{code}/"));
            return match to {
                Ok(to) => with_headers(
                    Reply::error(StatusCode::TEMPORARY_REDIRECT, TEXT_MOVED),
                    vec![(LOCATION, to)],
                ),
                Err(_) => no_link(),
            };
        }
        Some("") => return page(state).await,
        Some(name) => name.to_owned(),
    };
    let found = {
        let (state, name) = (state.clone(), name.clone());
        off_thread(move || door2_file(&state, &name))
            .await
            .flatten()
    };
    let head = req.method == Method::HEAD;
    match found {
        None => no_link(),
        Some(Door2File::Notice(file, size)) => file_reply(file, size, None, &name, false),
        Some(Door2File::Package(_, opened)) if head => {
            file_reply(opened.file, opened.size, None, &name, true)
        }
        Some(Door2File::Package(platform, opened)) => {
            let Some(slot) = state.updates.slots.take() else {
                return busy();
            };
            match state.updates.door2_records.admit() {
                Admit::Write => {
                    let ip_tag = state.tags.ip(&req.ip);
                    log_info!(
                        "update: {} {} through door 2 (ip={ip_tag}, code={})",
                        platform.name(),
                        opened.version,
                        code_tag(code)
                    );
                    let result = match platform {
                        Platform::Windows => "door2-windows",
                        Platform::Android => "door2-android",
                    };
                    state
                        .record("update", None, None, Some(ip_tag), result)
                        .await;
                }
                Admit::Skip { first: true } => log_warn!(
                    "update: more than {} downloads through door 2 this minute; the rest of the minute is not recorded",
                    limits::FAILURE_RECORDS_PER_MINUTE
                ),
                Admit::Skip { first: false } => {}
            }
            file_reply(opened.file, opened.size, Some(slot), &name, true)
        }
    }
}

/// The file door 2 hands out under `name`, opened: a package under one of its names, or a notice
/// file of a package (it reads files: not for an async thread).
fn door2_file(state: &AppState, name: &str) -> Option<Door2File> {
    let policy = state.updates.policy(&state.config)?;
    let dir = &state.config.package_dir;
    for platform in Platform::ALL {
        let Some((entry, package)) = policy.package(platform) else {
            continue;
        };
        let version = &entry.latest;
        let is_package = match platform {
            Platform::Windows => windows_names(version).iter().any(|n| n == name),
            Platform::Android => package.file == name,
        };
        if is_package {
            return match open_plain(&dir.join(&package.file), Some(package.size)) {
                Ok((file, size)) => Some(Door2File::Package(
                    platform,
                    Opened {
                        file,
                        size,
                        version: version.clone(),
                        stored: package.file.clone(),
                    },
                )),
                Err(why) => {
                    complain(state, platform, why);
                    None
                }
            };
        }
        if let Some(notice) = NOTICES.iter().find(|n| notice_name(version, n) == name) {
            return open_notice(dir, version, notice)
                .map(|(file, size)| Door2File::Notice(file, size));
        }
    }
    None
}

// ---- door 2's page ----

/// Text for HTML.
fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// "26,214,400".
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let groups: Vec<&str> = digits
        .as_bytes()
        .rchunks(3)
        .rev()
        .map(|group| std::str::from_utf8(group).unwrap_or_default())
        .collect();
    groups.join(",")
}

/// A link to a file of the page's own directory.
fn file_link(name: &str) -> String {
    format!("<a href=\"{0}\">{0}</a>", esc(name))
}

/// The size and SHA-256 of a package, for comparing with what the operator sent.
fn facts(package: &Package) -> String {
    format!(
        "크기 {}바이트(약 {} MB) · SHA-256 <code>{}</code>",
        grouped(package.size),
        package.size.div_ceil(1024 * 1024),
        esc(&package.sha256)
    )
}

const PAGE_HEAD: &str = r#"<!doctype html>
<html lang="ko">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex, nofollow, noarchive">
<meta name="referrer" content="no-referrer">
<title>HJViewer 새 버전 받기</title>
<style>
body { font-family: sans-serif; line-height: 1.6; max-width: 44em; margin: 1.5em auto; padding: 0 1em; }
h1 { font-size: 1.4em; }
h2 { font-size: 1.15em; margin-top: 1.8em; }
code { word-break: break-all; }
.facts { font-size: 0.9em; color: #444; }
</style>
</head>
<body>
<h1>HJViewer 새 버전 받기</h1>
<p>이 페이지의 주소는 받은 사람만 쓰세요. 다른 사람에게 알리거나 인터넷에 올리지 마세요.</p>
"#;

/// Door 2's page (the design's text K2), from the policy as it is now. `notices` is the notice
/// files that are there, by version. Every value of the policy and the settings is escaped.
fn page_html(
    policy: Option<&Policy>,
    notices: &[(String, Vec<&'static str>)],
    source_url: &str,
) -> String {
    let package = |platform| policy.and_then(|policy| policy.package(platform));
    let mut html = String::from(PAGE_HEAD);
    html.push_str("<h2>Windows PC</h2>\n");
    match package(Platform::Windows) {
        Some((entry, package)) => {
            let [install, run] = windows_names(&entry.latest);
            html.push_str(&format!(
                "<p>새 버전은 HJViewer {version}입니다.</p>\n\
                 <ol>\n\
                 <li>설치 없이 실행만 했다면 먼저 HJViewer를 완전히 끝내세요(작업 표시줄 오른쪽 아래 아이콘의 [끝내기]).</li>\n\
                 <li>이 PC에 설치해서 쓰고 있었다면 {install_link}를, 설치 없이 쓰고 있었다면 {run_link}를 받아 실행하세요.</li>\n\
                 <li>'Windows의 PC 보호' 창이 뜨면 [추가 정보] → [실행]을 누르세요. 아래 SHA-256이 관리자가 알려 준 값과 같은지 볼 수 있습니다.</li>\n\
                 </ol>\n\
                 <p>같은 파일을 두 번 받으면 브라우저가 이름 끝에 (1) 같은 번호를 붙입니다. 이름이 install.exe로 끝나지 않으면 설치 화면이 열리지 않습니다. 번호가 붙은 파일은 지우고 {install}를 실행하세요.</p>\n\
                 <p class=\"facts\">두 파일은 이름만 다르고 내용은 같습니다. {facts}</p>\n",
                version = esc(&entry.latest),
                install_link = file_link(&install),
                run_link = file_link(&run),
                install = esc(&install),
                facts = facts(package),
            ));
        }
        None => html.push_str("<p>새 버전을 준비하고 있습니다. 잠시 뒤 다시 열어 주세요.</p>\n"),
    }
    html.push_str(
        "<h2>Android</h2>\n<p>Play 스토어에서 설치했다면 Play 스토어에서 업데이트하세요.</p>\n",
    );
    match package(Platform::Android) {
        Some((entry, package)) => html.push_str(&format!(
            "<p>그 밖에는 {link}를 받아 여세요(HJViewer {version}). 설치 화면이 뜨면 [업데이트]를 누르세요.</p>\n\
             <p class=\"facts\">{facts}</p>\n",
            version = esc(&entry.latest),
            link = file_link(&package.file),
            facts = facts(package),
        )),
        None => html.push_str(
            "<p>그 밖의 기기에 쓸 새 버전은 준비하고 있습니다. 잠시 뒤 다시 열어 주세요.</p>\n",
        ),
    }
    html.push_str(
        "<h2>iPhone</h2>\n<p>TestFlight 앱에서 HJViewer의 [업데이트]를 누르세요. <a href=\"itms-beta://\">TestFlight 열기</a></p>\n",
    );
    html.push_str(
        "<h2>소스 코드와 라이선스</h2>\n<p>HJViewer는 GNU AGPL-3.0 라이선스로 배포합니다.",
    );
    let source = match source_url.starts_with("https://") {
        true => format!(
            " 소스 코드: <a href=\"{0}\" rel=\"noreferrer noopener\">{0}</a>",
            esc(source_url)
        ),
        false => String::new(),
    };
    html.push_str(&source);
    let mut tags: Vec<&str> = Platform::ALL
        .into_iter()
        .filter_map(|platform| package(platform)?.0.source.as_deref())
        .collect();
    tags.dedup();
    if !tags.is_empty() {
        html.push_str(&format!(" (태그 {})", esc(&tags.join(", "))));
    }
    html.push_str("</p>\n");
    for (version, names) in notices {
        let links: Vec<String> = names
            .iter()
            .map(|notice| {
                format!(
                    "<a href=\"{}\">{}</a>",
                    esc(&notice_name(version, notice)),
                    esc(notice)
                )
            })
            .collect();
        html.push_str(&format!(
            "<p>HJViewer {}의 알림 파일: {}</p>\n",
            esc(version),
            links.join(" · ")
        ));
    }
    html.push_str("</body>\n</html>\n");
    html
}

/// Door 2's page as the answer.
async fn page(state: &Arc<AppState>) -> Reply {
    let html = {
        let state = state.clone();
        off_thread(move || {
            let policy = state.updates.policy(&state.config);
            let mut notices: Vec<(String, Vec<&'static str>)> = Vec::new();
            for platform in Platform::ALL {
                let Some((entry, _)) = policy.as_ref().and_then(|p| p.package(platform)) else {
                    continue;
                };
                if notices.iter().any(|(version, _)| *version == entry.latest) {
                    continue;
                }
                let there: Vec<&'static str> = NOTICES
                    .into_iter()
                    .filter(|n| open_notice(&state.config.package_dir, &entry.latest, n).is_some())
                    .collect();
                if !there.is_empty() {
                    notices.push((entry.latest.clone(), there));
                }
            }
            page_html(policy.as_deref(), &notices, &state.config.source_url)
        })
        .await
    };
    let Some(html) = html else {
        return Reply::server_error();
    };
    let mut headers = private_headers();
    headers.extend([
        (
            CONTENT_TYPE,
            HeaderValue::from_static(content_type(".html")),
        ),
        (CONTENT_DISPOSITION, HeaderValue::from_static("inline")),
        (
            CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'"),
        ),
    ]);
    Reply {
        sent: Some(Sent {
            headers,
            content: Some(Content::Page(Bytes::from(html))),
        }),
        ..Reply::ok(&json!({}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::b64_decode;
    use http_body_util::BodyExt;
    use std::path::PathBuf;

    /// tools/hjv-release/tests/vectors/accepted/e2-s2.json, byte for byte: a policy as the
    /// operator's tool writes and signs it (with the tool's test keys), its root declaration
    /// along.
    const TOOL_POLICY: &[u8] = include_bytes!("../tests/data/version-policy-e2-s2.json");

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "hjv-api-update-{}-{}",
            std::process::id(),
            hex(&random_bytes::<6>())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn config(dir: &Path) -> Config {
        let vars: HashMap<String, String> = [
            ("HJV_API_SSO_TOKEN_URL", "https://sso.example.ac.kr/token"),
            ("HJV_API_SSO_USERINFO_URL", "https://sso.example.ac.kr/user"),
            ("HJV_API_SSO_CLIENT_ID", "client"),
            ("HJV_API_DOWNLOADS_MAX", "2"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .chain([
            (
                "HJV_API_VERSION_POLICY".to_owned(),
                dir.join("version-policy.json").display().to_string(),
            ),
            (
                "HJV_API_PACKAGE_DIR".to_owned(),
                dir.join("pkg").display().to_string(),
            ),
            (
                "HJV_API_DOOR2_FILE".to_owned(),
                dir.join("door2.conf").display().to_string(),
            ),
        ])
        .collect();
        Config::from_vars(&vars).unwrap()
    }

    /// A change to a policy text.
    type Change<'a> = &'a dyn Fn(&mut Value);

    /// A policy text of version 1 with these `platforms`.
    fn policy_text(platforms: Value) -> Value {
        json!({"v": 1, "epoch": 1, "seq": 7, "issued": "2026-10-05T03:00:00Z",
               "server": "rd.example.com", "platforms": platforms})
    }

    /// The policy file of a policy text: the envelope around it (hjv-api does not read `s`).
    fn envelope(policy: &Value) -> Vec<u8> {
        json!({"v": 1, "k": 1, "p": policy.to_string(), "s": "AAAA"})
            .to_string()
            .into_bytes()
    }

    fn windows(file: &str, size: u64) -> Value {
        json!({"min": "1.6.0", "latest": "1.6.1", "source": "client-1.6.1",
               "package": {"file": file, "size": size, "sha256": "ab".repeat(32), "build": "1.6.1.41"}})
    }

    #[test]
    fn versions() {
        for ok in ["1.6.0", "0.0.0", "1.6.10", "999.999.99", "10.0.1"] {
            assert!(is_version(ok), "{ok}");
        }
        for bad in [
            "",
            "1",
            "1.6",
            "1.6.0.1",
            "v1.6.0",
            "1.6.0 ",
            " 1.6.0",
            "1.06.0",
            "01.6.0",
            "1.6.00",
            "1.6.100",
            "1000.0.0",
            "1..0",
            "1.6.",
            ".6.0",
            "1.6.x",
            "1.6.0-1",
            "1,6,0",
            "link",
            "１.６.０",
        ] {
            assert!(!is_version(bad), "{bad}");
        }
    }

    #[test]
    fn package_file_names() {
        let (w, a) = (Platform::Windows, Platform::Android);
        assert!(is_package_file(w, "HJViewer-1.6.1-windows-x86_64.exe"));
        assert!(is_package_file(a, "HJViewer-1.6.1-android-arm64.apk"));
        assert!(is_package_file(w, "a.exe"));
        assert!(is_package_file(w, &format!("{}.exe", "a".repeat(96))));
        for bad in [
            "",
            ".exe",
            "..exe",
            "a..exe",
            "../a.exe",
            "pkg/a.exe",
            "a b.exe",
            "a.exe ",
            "한글.exe",
            "a.apk",
            "a.exe.txt",
            "a",
            // the packer would open the installer, or run as quick support
            "HJViewer-1.6.1-install.exe",
            "HJViewer-INSTALL.EXE",
            "install.exe",
            "HJViewer-qs-1.exe",
            "HJViewer-QS-1.exe",
            &format!("{}.exe", "a".repeat(97)),
        ] {
            assert!(!is_package_file(w, bad), "{bad}");
        }
        for bad in ["a.exe", ".apk", "a/b.apk", "a.apk.exe"] {
            assert!(!is_package_file(a, bad), "{bad}");
        }
        assert_eq!(Platform::parse("windows"), Some(w));
        assert_eq!(Platform::parse("android"), Some(a));
        for other in ["", "Windows", "ios", "android_play", "linux", "windows "] {
            assert_eq!(Platform::parse(other), None, "{other}");
        }
    }

    #[test]
    fn the_policy_of_the_operators_tool() {
        // the copy is the tool's vector (compare with tools/hjv-release/tests/vectors)
        assert_eq!(
            hex(&sha256(TOOL_POLICY)),
            "3f76af335aacc32c7caefbc04832150b29c002e9e406c48d449ed0385a7f41f9"
        );
        let policy = Policy::parse(TOOL_POLICY).unwrap();
        assert_eq!((policy.epoch, policy.seq), (2, 2));
        let (entry, package) = policy.package(Platform::Windows).unwrap();
        assert_eq!(entry.latest, "1.6.1");
        assert_eq!(entry.source.as_deref(), Some("client-1.6.1"));
        assert_eq!(
            *package,
            Package {
                file: "HJViewer-1.6.1-windows-x86_64.exe".into(),
                size: 25_400_320,
                sha256: "9cbbb39735478859c75c6ba12439e589e0d383d398559347352dd48e61bc6ca9".into(),
            }
        );
        let (entry, package) = policy.package(Platform::Android).unwrap();
        assert_eq!(entry.latest, "1.6.1");
        assert_eq!(package.file, "HJViewer-1.6.1-android-arm64.apk");
        assert_eq!(package.size, 28_800_004);
        assert_eq!(
            policy.summary(),
            "epoch 2, seq 2; packages: windows 1.6.1, android 1.6.1"
        );
    }

    #[test]
    fn what_is_and_is_not_a_policy() {
        let good = policy_text(json!({
            "windows": windows("HJViewer-1.6.1-windows-x86_64.exe", 100),
            // entries hjv-api has no package for, and fields it does not know
            "ios": {"min": "1.6.0", "latest": "1.6.0", "testflight_build": "262740100"},
            "android_play": {"min": "1.6.0", "latest": "1.6.1", "version_code": 262782100},
            "later": {"anything": true},
        }));
        let policy = Policy::parse(&envelope(&good)).unwrap();
        assert_eq!(policy.seq, 7);
        assert!(policy.package(Platform::Windows).is_some());
        assert_eq!(policy.android, None);
        assert_eq!(
            policy.summary(),
            "epoch 1, seq 7; packages: windows 1.6.1, android no package"
        );
        // an entry without a package: known, nothing to hand out
        let bare = policy_text(json!({"android": {"min": "1.5.0", "latest": "1.6.0"}}));
        let policy = Policy::parse(&envelope(&bare)).unwrap();
        assert_eq!(policy.entry(Platform::Android).unwrap().latest, "1.6.0");
        assert!(policy.package(Platform::Android).is_none());
        assert!(Policy::parse(&envelope(&policy_text(json!({})))).is_ok());

        // the envelope
        for (file, why) in [
            (&b""[..], "the file is not JSON"),
            (b"version-policy", "the file is not JSON"),
            (
                br#"{"v":2,"k":1,"p":"{}","s":""}"#,
                "not an envelope of version 1",
            ),
            (
                br#"{"k":1,"p":"{}","s":""}"#,
                "not an envelope of version 1",
            ),
            (
                br#"{"v":1,"k":1,"s":""}"#,
                "the envelope has no policy text",
            ),
            (
                br#"{"v":1,"k":1,"p":{"v":1},"s":""}"#,
                "the envelope has no policy text",
            ),
            (
                br#"{"v":1,"k":1,"p":"{","s":""}"#,
                "the policy text is not JSON",
            ),
            (
                br#"{"v":1,"k":1,"p":"{\"v\":2}","s":""}"#,
                "not a policy of version 1",
            ),
        ] {
            assert_eq!(
                Policy::parse(file),
                Err(why),
                "{}",
                String::from_utf8_lossy(file)
            );
        }
        // the policy text: every part hjv-api uses keeps the policy's rules, or it is no policy
        let with = |change: Change| {
            let mut policy = good.clone();
            change(&mut policy);
            Policy::parse(&envelope(&policy))
        };
        let cases: [(Change, &str); 16] = [
            (
                &|p| p["epoch"] = json!(0),
                "the policy has no epoch and seq",
            ),
            (
                &|p| p["seq"] = json!("7"),
                "the policy has no epoch and seq",
            ),
            (
                &|p| p["platforms"] = json!([]),
                "the policy has no platforms",
            ),
            (
                &|p| p["platforms"]["windows"] = json!("1.6.1"),
                "an entry's latest is not a version",
            ),
            (
                &|p| p["platforms"]["windows"]["latest"] = json!("1.6"),
                "an entry's latest is not a version",
            ),
            (
                &|p| p["platforms"]["windows"]["latest"] = json!(161),
                "an entry's latest is not a version",
            ),
            (
                &|p| p["platforms"]["windows"]["source"] = json!("a tag"),
                "an entry's source is not a tag name",
            ),
            (
                &|p| p["platforms"]["windows"]["source"] = json!("<b>"),
                "an entry's source is not a tag name",
            ),
            (
                &|p| p["platforms"]["windows"]["package"]["file"] = json!("../../etc/passwd"),
                "a package's file is not a name of the package directory",
            ),
            (
                &|p| {
                    p["platforms"]["windows"]["package"]["file"] =
                        json!("HJViewer-1.6.1-install.exe")
                },
                "a package's file is not a name of the package directory",
            ),
            (
                &|p| {
                    p["platforms"]["windows"]["package"]["file"] =
                        json!("HJViewer-1.6.1-android-arm64.apk")
                },
                "a package's file is not a name of the package directory",
            ),
            (
                &|p| p["platforms"]["windows"]["package"]["size"] = json!(0),
                "a package's size is not 1 byte to 200 MiB",
            ),
            (
                &|p| p["platforms"]["windows"]["package"]["size"] = json!(MAX_PACKAGE + 1),
                "a package's size is not 1 byte to 200 MiB",
            ),
            (
                &|p| p["platforms"]["windows"]["package"]["size"] = json!("100"),
                "a package's size is not 1 byte to 200 MiB",
            ),
            (
                &|p| p["platforms"]["windows"]["package"]["sha256"] = json!("AB".repeat(32)),
                "a package's sha256 is not 64 lowercase hex digits",
            ),
            (
                &|p| p["platforms"]["android"] = windows("HJViewer-1.6.1-windows-x86_64.exe", 100),
                "a package's file is not a name of the package directory",
            ),
        ];
        for (i, (change, why)) in cases.iter().enumerate() {
            assert_eq!(with(change), Err(*why), "case {i}");
        }
        assert!(
            with(&|p| p["platforms"]["windows"]["package"]["size"] = json!(MAX_PACKAGE)).is_ok()
        );
    }

    #[test]
    fn door_2s_file() {
        let code = "0123456789abcdef0123456789abcdef";
        let door =
            Door2::parse(format!("CODE={code}\nUNTIL=2026-12-22T15:00:00Z\n").as_bytes()).unwrap();
        assert_eq!(door.code, code);
        assert_eq!(datetime(door.until), "2026-12-22T15:00:00Z");
        // the journal names a code by a tag, never itself
        let summary = door.summary();
        assert_eq!(
            summary,
            format!(
                "open until 2026-12-22T15:00:00Z (code={})",
                &hex(&sha256(code.as_bytes()))[..8]
            )
        );
        assert!(!summary.contains(code));
        let door = Door2::parse(
            format!("# door 2\r\n\r\n  UNTIL=2027-01-01T00:00:00Z  \r\nCODE= {code} \r\n")
                .as_bytes(),
        )
        .unwrap();
        assert_eq!((door.code.as_str(), door.until), (code, 1_798_761_600));
        for (text, why) in [
            (String::new(), "CODE or UNTIL is missing"),
            (format!("CODE={code}\n"), "CODE or UNTIL is missing"),
            (
                "UNTIL=2026-12-22T15:00:00Z\n".to_owned(),
                "CODE or UNTIL is missing",
            ),
            (
                format!("CODE={}\nUNTIL=2026-12-22T15:00:00Z\n", &code[..31]),
                "CODE is not 32 lowercase hex digits",
            ),
            (
                format!("CODE={code}0\nUNTIL=2026-12-22T15:00:00Z\n"),
                "CODE is not 32 lowercase hex digits",
            ),
            (
                format!("CODE={}\nUNTIL=2026-12-22T15:00:00Z\n", code.to_uppercase()),
                "CODE is not 32 lowercase hex digits",
            ),
            (format!("CODE={code}\nUNTIL=later\n"), "UNTIL is not a time"),
            (
                format!("CODE={code}\nUNTIL=2026-12-22\n"),
                "UNTIL is not a time",
            ),
            (
                format!("CODE={code}\nUNTIL=2026-12-22T15:00:00+09:00\n"),
                "UNTIL is not a time",
            ),
            (
                format!("CODE={code}\nUNTIL=2026-12-22T15:00:00Z\nextra\n"),
                "a line that is neither CODE=, UNTIL= nor a # comment",
            ),
        ] {
            let got = Door2::parse(text.as_bytes()).map(|_| ()).unwrap_err();
            assert_eq!(got, why);
            // no error shows what the file holds
            assert!(!got.contains(&code[..8]));
        }
        assert!(Door2::parse(&[0xff, 0xfe]).is_err());
    }

    #[test]
    fn times_of_the_policys_form() {
        for at in [
            0,
            1,
            86_399,
            951_782_400,
            1_760_000_000,
            1_798_761_599,
            4_102_444_800,
        ] {
            assert_eq!(utc_time(&datetime(at)), Some(at), "{at}");
        }
        assert_eq!(utc_time("2026-11-22T15:00:00Z"), Some(1_795_359_600));
        assert_eq!(utc_time("2028-02-29T23:59:59Z"), Some(1_835_481_599));
        for bad in [
            "",
            "2026-11-22",
            "2026-11-22T15:00:00",
            "2026-11-22T15:00:00z",
            "2026-11-22 15:00:00Z",
            "2026-11-22T15:00:00+0",
            "2026-11-22T24:00:00Z",
            "2026-11-22T15:60:00Z",
            "2026-11-22T15:00:60Z",
            "2026-02-30T00:00:00Z",
            "2026-13-01T00:00:00Z",
            "2026-11-22T1a:00:00Z",
            "2026-11-22T15-00-00Z",
            "2026-11-22T15:00:00Z ",
            "２026-11-22T15:00:0Z",
        ] {
            assert_eq!(utc_time(bad), None, "{bad}");
        }
    }

    #[test]
    fn codes_of_short_links() {
        assert_eq!(code_of([0; 5]), "00000000");
        assert_eq!(code_of([0xff; 5]), "ZZZZZZZZ");
        // 00001 00010 00011 00100 00101 00110 00111 01000
        assert_eq!(code_of([0x08, 0x86, 0x42, 0x98, 0xe8]), "12345678");
        assert_eq!(shown("K7M2Q9XD"), "K7M2-Q9XD");
        for _ in 0..200 {
            let code = code_of(random_bytes::<5>());
            assert_eq!(code.len(), 8);
            assert!(code.bytes().all(|b| SYMBOLS.contains(&b)), "{code}");
            assert_eq!(typed_code(&shown(&code)), Some(code.clone()));
            assert_eq!(typed_code(&shown(&code).to_lowercase()), Some(code));
        }
        for typed in [
            "K7M2-Q9XD",
            "K7M2Q9XD",
            "k7m2-q9xd",
            "k7m2q9xd",
            "K7M2%20Q9XD",
            "%20K7M2-Q9XD%20",
            "K7M2 Q9XD",
            "K-7-M-2-Q-9-X-D",
            "K7M2%2DQ9XD",
            "K7M2%2dQ9XD",
            "%4b7M2-Q9XD",
        ] {
            assert_eq!(typed_code(typed).as_deref(), Some("K7M2Q9XD"), "{typed}");
        }
        // letters that look like digits are read as the digits
        assert_eq!(typed_code("OoIi-LlO1").as_deref(), Some("00111101"));
        for bad in [
            "",
            "K7M2-Q9X",
            "K7M2-Q9XDD",
            "K7M2-Q9XU",
            "K7M2-Q9X!",
            "K7M2_Q9XD",
            "K7M2-Q9X%",
            "K7M2-Q9X%4",
            "K7M2-Q9X%zz",
            "K7M2-Q9X%00",
            "K7M2/Q9XD",
            "K7M2-Q9XD.exe",
            "K7M2+Q9XD",
            "Ｋ7M2-Q9XD",
            &"K7M2-Q9XD".repeat(8),
            &format!("{}K7M2Q9XD", "-".repeat(57)),
        ] {
            assert_eq!(typed_code(bad), None, "{bad}");
        }
        assert!(typed_code(&format!("{}K7M2Q9XD", "-".repeat(56))).is_some());
    }

    #[test]
    fn names_and_content_types() {
        assert_eq!(
            windows_names("1.6.1"),
            ["HJViewer-1.6.1-install.exe", "HJViewer-1.6.1.exe"]
        );
        let stored = "HJViewer-1.6.1-windows-x86_64.exe";
        let (w, a) = (Platform::Windows, Platform::Android);
        assert_eq!(
            download_name(w, "1.6.1", stored, true),
            "HJViewer-1.6.1-install.exe"
        );
        assert_eq!(
            download_name(w, "1.6.1", stored, false),
            "HJViewer-1.6.1.exe"
        );
        // what an app or a PC's service gets never opens the installer by its name
        assert!(!download_name(w, "1.6.1", stored, false).ends_with("install.exe"));
        let apk = "HJViewer-1.6.1-android-arm64.apk";
        assert_eq!(download_name(a, "1.6.1", apk, true), apk);
        assert_eq!(download_name(a, "1.6.1", apk, false), apk);
        assert_eq!(
            notice_name("1.6.1", "LICENSE"),
            "HJViewer-1.6.1-LICENSE.txt"
        );
        for (name, kind) in [
            (
                "HJViewer-1.6.1.exe",
                "application/vnd.microsoft.portable-executable",
            ),
            (
                "HJViewer-1.6.1-install.exe",
                "application/vnd.microsoft.portable-executable",
            ),
            (apk, "application/vnd.android.package-archive"),
            ("HJViewer-1.6.1-LICENSE.txt", "text/plain; charset=utf-8"),
            (".html", "text/html; charset=utf-8"),
            ("HJViewer", "application/octet-stream"),
            ("a.EXE", "application/octet-stream"),
        ] {
            assert_eq!(content_type(name), kind, "{name}");
        }
    }

    #[test]
    fn the_paths() {
        for (path, methods, browser) in [
            ("/api/hjv/update/windows/1.6.1", Some("GET, HEAD"), false),
            ("/api/hjv/update/android/1.6.1", Some("GET, HEAD"), false),
            ("/api/hjv/update/ios/x", Some("GET, HEAD"), false),
            ("/api/hjv/update/windows/link", Some("POST"), false),
            ("/api/hjv/u/K7M2-Q9XD", Some("GET, HEAD"), true),
            ("/api/hjv/u/x", Some("GET, HEAD"), true),
            (
                "/api/hjv/get/0123456789abcdef0123456789abcdef",
                Some("GET, HEAD"),
                true,
            ),
            (
                "/api/hjv/get/0123456789abcdef0123456789abcdef/",
                Some("GET, HEAD"),
                true,
            ),
            (
                "/api/hjv/get/0123456789abcdef0123456789abcdef/HJViewer-1.6.1.exe",
                Some("GET, HEAD"),
                true,
            ),
            ("/api/hjv/get/x/y", Some("GET, HEAD"), true),
            ("/api/hjv/update", None, false),
            ("/api/hjv/update/", None, false),
            ("/api/hjv/update/windows", None, false),
            ("/api/hjv/update/windows/", None, false),
            ("/api/hjv/update//1.6.1", None, false),
            ("/api/hjv/update/windows/1.6.1/", None, false),
            ("/api/hjv/update/windows/link/x", None, false),
            ("/api/hjv/u", None, false),
            ("/api/hjv/u/", None, false),
            ("/api/hjv/u/K7M2-Q9XD/", None, false),
            ("/api/hjv/get", None, false),
            ("/api/hjv/get/", None, false),
            ("/api/hjv/get//x", None, false),
            ("/api/hjv/get/x/y/z", None, false),
            ("/api/hjv/updates/windows/1.6.1", None, false),
            ("/api/hjv/user", None, false),
            ("/api/hjv/getx/y", None, false),
        ] {
            assert_eq!(allowed_methods(path), methods, "{path}");
            assert_eq!(browser_path(path), browser, "{path}");
            for method in [
                Method::GET,
                Method::HEAD,
                Method::POST,
                Method::DELETE,
                Method::PUT,
            ] {
                let takes_it = methods.is_some_and(|m| m.split(", ").any(|a| a == method.as_str()));
                assert_eq!(takes(path, &method), takes_it, "{method} {path}");
            }
        }
        assert!(takes("/api/hjv/update/windows/link", &Method::POST));
        assert!(!takes("/api/hjv/update/windows/link", &Method::GET));
        assert!(!takes("/api/hjv/u/K7M2-Q9XD", &Method::POST));
    }

    // The vector of the design's research notes (build/minver/policy-distribution, made with
    // Python's cryptography and checked with OpenSSL, ed25519-dalek and libsodium), which the
    // client checks its side against as well (hjviewer_update/vectors.rs): the device key of the
    // seed 60 61 … 7f.
    const DEVICE_PK: &str = "F0VTtFbd38aQjsqxwQH+arIeK6oGF3lbfUOmNIKZP9U=";
    const DEVICE_SIG: &str =
        "9SMz/B9qthKAtR/xDS6vk+fep97TTkCtFVTJRJ51ypTwmwxZMoH2EUoFrHlIN2LH5c+K5lTUfUoYMAQovYRLDg==";
    const DEVICE_SIGNED: &str = "686a762d7570646174652d76310031323334353637383900313739333537373630300077696e646f777300312e362e31";

    #[test]
    fn what_a_device_signs() {
        let message = device_message("123456789", "1793577600", "windows", "1.6.1");
        assert_eq!(hex(&message), DEVICE_SIGNED);
        let pk = b64_decode(DEVICE_PK).unwrap();
        let seed: [u8; 32] = std::array::from_fn(|i| 0x60 + i as u8);
        assert_eq!(devsig::public_key(&seed).to_vec(), pk);
        assert!(devsig::verify(&pk, &message, DEVICE_SIG));
        assert_eq!(devsig::sign(&seed, &message), DEVICE_SIG);
        // every field is part of what is signed
        for other in [
            device_message("123456788", "1793577600", "windows", "1.6.1"),
            device_message("123456789", "1793577601", "windows", "1.6.1"),
            device_message("123456789", "1793577600", "android", "1.6.1"),
            device_message("123456789", "1793577600", "windows", "1.6.2"),
            device_message("12345678", "91793577600", "windows", "1.6.1"),
        ] {
            assert!(!devsig::verify(&pk, &other, DEVICE_SIG));
        }
        // and the context sets it apart from a login's and a sync's signature
        assert_eq!(&message[..14], b"hjv-update-v1\0");
        assert_ne!(CONTEXT, devsig::CONTEXT);
    }

    #[test]
    fn the_time_of_a_devices_request() {
        // ten minutes either way, to the second
        let at = 1_793_577_600;
        for (t, ok) in [
            (at, true),
            (at - 1, true),
            (at + 1, true),
            (at - 600, true),
            (at + 600, true),
            (at - 601, false),
            (at + 601, false),
            (at - 86_400, false),
            (at + 86_400, false),
            (0, false),
            (i64::MAX, false),
        ] {
            assert_eq!(in_time(t, at), ok, "{}", t - at);
        }
        assert_eq!(MAX_SKEW, 600);
        // as the header has it: decimal, one spelling for each time
        assert_eq!(device_time("1793577600"), Some(1_793_577_600));
        assert_eq!(device_time("0"), Some(0));
        assert_eq!(
            device_time("999999999999999999"),
            Some(999_999_999_999_999_999)
        );
        for bad in [
            "",
            "+1793577600",
            "-1793577600",
            "01793577600",
            "00",
            "1793577600 ",
            " 1793577600",
            "1793577600.0",
            "1.7e9",
            "0x6ae7f400",
            "now",
            "１793577600",
            "1000000000000000000",
            "9223372036854775808",
        ] {
            assert_eq!(device_time(bad), None, "{bad}");
        }
    }

    fn link(account_id: i64, at: i64) -> Link {
        Link {
            account_id,
            account_tag: "tag".into(),
            platform: Platform::Windows,
            version: "1.6.1".into(),
            stored: "HJViewer-1.6.1-windows-x86_64.exe".into(),
            name: "HJViewer-1.6.1-install.exe".into(),
            expires: at + LINK_SECS,
            left: LINK_DOWNLOADS,
        }
    }

    #[test]
    fn links_live_ten_minutes_and_three_downloads() {
        let dir = temp_dir();
        let updates = Updates::new(&config(&dir));
        assert_eq!(updates.slots.free(), 2);
        let at = 1_760_000_000;
        let code = updates.add_link(link(1, at), at).unwrap();
        assert_eq!(typed_code(&shown(&code)), Some(code.clone()));
        assert_eq!(updates.link(&code, at), Some(link(1, at)));
        assert_eq!(updates.link(&code, at + LINK_SECS - 1), Some(link(1, at)));
        // looking at it takes nothing
        assert_eq!(updates.link(&code, at).unwrap().left, 3);
        assert_eq!(updates.use_link(&code, at), Some(2));
        assert_eq!(updates.use_link(&code, at + 1), Some(1));
        assert_eq!(updates.link(&code, at + 2).unwrap().left, 1);
        assert_eq!(updates.use_link(&code, at + 2), Some(0));
        assert_eq!(updates.use_link(&code, at + 3), None);
        assert_eq!(updates.link(&code, at + 3), None);
        assert_eq!(updates.links_kept(), 0);
        // ten minutes, not a second more
        let code = updates.add_link(link(2, at), at).unwrap();
        assert_eq!(updates.link(&code, at + LINK_SECS), None);
        assert_eq!(updates.use_link(&code, at + LINK_SECS), None);
        assert_eq!(updates.use_link(&code, at + LINK_SECS - 1), Some(2));
        updates.age_links(LINK_SECS);
        assert_eq!(updates.link(&code, at), None);
        // the next one made forgets those past their time
        assert_eq!(updates.links_kept(), 1);
        let other = updates.add_link(link(3, at), at).unwrap();
        assert_eq!(updates.links_kept(), 1);
        assert_ne!(other, code);
        updates.drop_link(&other);
        assert_eq!(updates.link(&other, at), None);
        // codes do not repeat, and no more than a thousand are kept at once
        let mut codes = std::collections::HashSet::new();
        for _ in 0..1000 {
            assert!(codes.insert(updates.add_link(link(4, at), at).unwrap()));
        }
        assert_eq!(updates.add_link(link(4, at), at), None);
        assert!(updates.add_link(link(4, at), at + LINK_SECS).is_some());
        assert_eq!(updates.links_kept(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_operators_files_are_read_again_when_they_change() {
        crate::log::capture();
        let dir = temp_dir();
        let config = config(&dir);
        let updates = Updates::new(&config);
        assert!(updates.policy(&config).is_none());
        assert!(updates.door2(&config).is_none());
        std::fs::write(&config.version_policy, TOOL_POLICY).unwrap();
        let first = updates.policy(&config).unwrap();
        assert_eq!(first.seq, 2);
        // not read again while it is the same file
        assert!(Arc::ptr_eq(&first, &updates.policy(&config).unwrap()));
        // replaced as the runbook replaces it: a new file moved over the old one
        let next = policy_text(json!({"windows": windows("HJViewer-1.6.2-windows-x86_64.exe", 9)}));
        let new = dir.join("version-policy.json.new");
        std::fs::write(&new, envelope(&next)).unwrap();
        std::fs::rename(&new, &config.version_policy).unwrap();
        let second = updates.policy(&config).unwrap();
        assert_eq!(second.seq, 7);
        assert_eq!(second.package(Platform::Windows).unwrap().1.size, 9);
        // what is no policy hands out nothing, and the old one is not kept in its place
        std::fs::write(&config.version_policy, b"{}").unwrap();
        assert!(updates.policy(&config).is_none());
        assert!(updates.policy(&config).is_none());
        std::fs::write(
            &config.version_policy,
            vec![b' '; MAX_POLICY_FILE as usize + 1],
        )
        .unwrap();
        assert!(updates.policy(&config).is_none());
        std::fs::write(&config.version_policy, TOOL_POLICY).unwrap();
        assert_eq!(updates.policy(&config).unwrap().seq, 2);
        std::fs::remove_file(&config.version_policy).unwrap();
        assert!(updates.policy(&config).is_none());
        // a directory in its place is no file
        std::fs::create_dir(&config.version_policy).unwrap();
        assert!(updates.policy(&config).is_none());

        let code = "0123456789abcdef0123456789abcdef";
        std::fs::write(
            &config.door2_file,
            format!("CODE={code}\nUNTIL=2026-12-22T15:00:00Z\n"),
        )
        .unwrap();
        let door = updates.door2(&config).unwrap();
        assert_eq!((door.code.as_str(), door.until), (code, 1_797_951_600));
        std::fs::write(
            &config.door2_file,
            format!("CODE={code}\nUNTIL=2027-01-01T00:00:00Z\n"),
        )
        .unwrap();
        assert_eq!(updates.door2(&config).unwrap().until, 1_798_761_600);
        std::fs::remove_file(&config.door2_file).unwrap();
        assert!(updates.door2(&config).is_none());
        // the journal heard of each change, and never of the code
        let lines = crate::log::captured();
        let mine: Vec<&String> = lines.iter().filter(|l| l.starts_with("update: ")).collect();
        assert!(mine.iter().any(|l| l.contains("the version policy (HJV_API_VERSION_POLICY) read: epoch 2, seq 2; packages: windows 1.6.1, android 1.6.1")));
        assert!(mine
            .iter()
            .any(|l| l.contains("is not used: not an envelope of version 1")));
        assert!(mine
            .iter()
            .any(|l| l.contains("is not used: it is too large")));
        assert!(mine
            .iter()
            .any(|l| l.contains("the version policy (HJV_API_VERSION_POLICY) is gone")));
        assert!(mine.iter().any(|l| l.contains(
            "door 2's file (HJV_API_DOOR2_FILE) read: open until 2026-12-22T15:00:00Z (code="
        )));
        assert!(mine
            .iter()
            .any(|l| l.contains("door 2's file (HJV_API_DOOR2_FILE) is gone")));
        assert!(!lines.iter().any(|l| l.contains(code)));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn only_plain_files_of_the_package_directory_are_opened() {
        let dir = temp_dir();
        let pkg = dir.join("pkg");
        std::fs::create_dir_all(pkg.join("notices-1.6.1")).unwrap();
        let file = pkg.join("HJViewer-1.6.1-windows-x86_64.exe");
        std::fs::write(&file, b"MZ package").unwrap();
        assert_eq!(open_plain(&file, Some(10)).unwrap().1, 10);
        assert_eq!(open_plain(&file, None).unwrap().1, 10);
        // the size the policy says, exactly
        assert_eq!(open_plain(&file, Some(9)).unwrap_err(), NoFile::Size);
        assert_eq!(open_plain(&file, Some(11)).unwrap_err(), NoFile::Size);
        assert_eq!(
            open_plain(&pkg.join("other.exe"), Some(10)).unwrap_err(),
            NoFile::Missing
        );
        // no link, wherever it points; no directory
        let secret = dir.join("secret");
        std::fs::write(&secret, b"0123456789").unwrap();
        let link = pkg.join("link.exe");
        std::os::unix::fs::symlink(&secret, &link).unwrap();
        assert_eq!(open_plain(&link, Some(10)).unwrap_err(), NoFile::NotPlain);
        std::os::unix::fs::symlink(&file, pkg.join("same.exe")).unwrap();
        assert_eq!(
            open_plain(&pkg.join("same.exe"), Some(10)).unwrap_err(),
            NoFile::NotPlain
        );
        std::os::unix::fs::symlink(dir.join("nowhere"), pkg.join("dangling.exe")).unwrap();
        assert_eq!(
            open_plain(&pkg.join("dangling.exe"), None).unwrap_err(),
            NoFile::NotPlain
        );
        assert_eq!(open_plain(&pkg, None).unwrap_err(), NoFile::NotPlain);
        // a file nobody may read is as good as missing
        let closed = pkg.join("closed.exe");
        std::fs::write(&closed, b"x").unwrap();
        std::fs::set_permissions(&closed, std::os::unix::fs::PermissionsExt::from_mode(0o000))
            .unwrap();
        if File::open(&closed).is_err() {
            assert_eq!(open_plain(&closed, None).unwrap_err(), NoFile::Missing);
        }

        // notice files: notices-<version>/<name>.txt, in a directory that is no link
        std::fs::write(pkg.join("notices-1.6.1/LICENSE.txt"), b"AGPL").unwrap();
        assert_eq!(open_notice(&pkg, "1.6.1", "LICENSE").unwrap().1, 4);
        assert!(open_notice(&pkg, "1.6.1", "NOTICE").is_none());
        assert!(open_notice(&pkg, "1.6.2", "LICENSE").is_none());
        std::os::unix::fs::symlink(pkg.join("notices-1.6.1"), pkg.join("notices-1.6.3")).unwrap();
        assert!(open_notice(&pkg, "1.6.3", "LICENSE").is_none());
        std::os::unix::fs::symlink(&secret, pkg.join("notices-1.6.1/SOURCE.txt")).unwrap();
        assert!(open_notice(&pkg, "1.6.1", "SOURCE").is_none());
        // a notice file is a text of a few megabytes at most: it goes out without a slot
        let large = File::create(pkg.join("notices-1.6.1/NOTICE.txt")).unwrap();
        large.set_len(MAX_NOTICE).unwrap();
        assert_eq!(open_notice(&pkg, "1.6.1", "NOTICE").unwrap().1, MAX_NOTICE);
        large.set_len(MAX_NOTICE + 1).unwrap();
        assert!(open_notice(&pkg, "1.6.1", "NOTICE").is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn comparing_codes() {
        assert!(same(b"", b""));
        assert!(same(b"0123456789abcdef", b"0123456789abcdef"));
        for (a, b) in [
            (&b"0123456789abcdef"[..], &b"0123456789abcdee"[..]),
            (b"0123456789abcdef", b"1123456789abcdef"),
            (b"0123456789abcdef", b"0123456789abcde"),
            (b"0123456789abcdef", b"0123456789abcdef0"),
            (b"", b"0"),
        ] {
            assert!(!same(a, b) && !same(b, a));
        }
        assert_eq!(code_tag("K7M2Q9XD").len(), 8);
        assert_ne!(code_tag("K7M2Q9XD"), code_tag("K7M2Q9XE"));
    }

    #[test]
    fn the_page_of_door_2() {
        let policy = Policy::parse(TOOL_POLICY).unwrap();
        let notices = vec![("1.6.1".to_owned(), vec!["LICENSE", "THIRD_PARTY_NOTICES"])];
        let html = page_html(
            Some(&policy),
            &notices,
            "https://github.com/yonsei-projecttf/hjviewer",
        );
        for part in [
            "<html lang=\"ko\">",
            "<meta name=\"robots\" content=\"noindex, nofollow, noarchive\">",
            "새 버전은 HJViewer 1.6.1입니다.",
            // the design's text K2, the installed PC's file and the other one apart
            "<li>설치 없이 실행만 했다면 먼저 HJViewer를 완전히 끝내세요(작업 표시줄 오른쪽 아래 아이콘의 [끝내기]).</li>",
            "<li>이 PC에 설치해서 쓰고 있었다면 <a href=\"HJViewer-1.6.1-install.exe\">HJViewer-1.6.1-install.exe</a>를, 설치 없이 쓰고 있었다면 <a href=\"HJViewer-1.6.1.exe\">HJViewer-1.6.1.exe</a>를 받아 실행하세요.</li>",
            "<li>'Windows의 PC 보호' 창이 뜨면 [추가 정보] → [실행]을 누르세요. 아래 SHA-256이 관리자가 알려 준 값과 같은지 볼 수 있습니다.</li>",
            "번호가 붙은 파일은 지우고 HJViewer-1.6.1-install.exe를 실행하세요.",
            "크기 25,400,320바이트(약 25 MB) · SHA-256 <code>9cbbb39735478859c75c6ba12439e589e0d383d398559347352dd48e61bc6ca9</code>",
            "Play 스토어에서 설치했다면 Play 스토어에서 업데이트하세요.",
            "<a href=\"HJViewer-1.6.1-android-arm64.apk\">HJViewer-1.6.1-android-arm64.apk</a>를 받아 여세요(HJViewer 1.6.1).",
            "크기 28,800,004바이트(약 28 MB) · SHA-256 <code>35e9781fea8aeddf96eb036b607e75b5f9fc1d26f947ff22a94220547c90a3c0</code>",
            "TestFlight 앱에서 HJViewer의 [업데이트]를 누르세요.",
            "<a href=\"https://github.com/yonsei-projecttf/hjviewer\" rel=\"noreferrer noopener\">https://github.com/yonsei-projecttf/hjviewer</a> (태그 client-1.6.1)",
            "HJViewer 1.6.1의 알림 파일: <a href=\"HJViewer-1.6.1-LICENSE.txt\">LICENSE</a> · <a href=\"HJViewer-1.6.1-THIRD_PARTY_NOTICES.txt\">THIRD_PARTY_NOTICES</a>",
        ] {
            assert!(html.contains(part), "{part}\n{html}");
        }
        // the links stay in the page's own directory, the one to the source apart: the code of
        // the address is nowhere in the page, and nothing is loaded from anywhere
        let links: Vec<&str> = html
            .split("href=\"")
            .skip(1)
            .map(|rest| rest.split('"').next().unwrap())
            .collect();
        assert_eq!(links.len(), 7);
        for link in &links {
            let own = link.starts_with("HJViewer-") && !link.contains('/');
            assert!(
                own || *link == "itms-beta://"
                    || *link == "https://github.com/yonsei-projecttf/hjviewer",
                "{link}"
            );
        }
        for none in [
            "<script", "<img", "<link", "<iframe", "<form", " src=", "url(", "@import",
        ] {
            assert!(!html.contains(none), "{none}");
        }

        // no policy, or one without packages: nothing to click but TestFlight
        for policy in [
            None,
            Some(Policy::parse(&envelope(&policy_text(json!({})))).unwrap()),
        ] {
            let html = page_html(policy.as_ref(), &[], "https://example.com/hjviewer");
            assert!(html.contains("<p>새 버전을 준비하고 있습니다. 잠시 뒤 다시 열어 주세요.</p>"));
            assert!(html.contains("그 밖의 기기에 쓸 새 버전은 준비하고 있습니다."));
            assert!(
                !html.contains(".exe") && !html.contains(".apk") && !html.contains("알림 파일")
            );
            assert!(!html.contains("태그"));
        }
        // what comes from the settings is text, and only an https address is a link
        let html = page_html(
            None,
            &[],
            "https://example.com/\"><script>alert(1)</script>&x='y'",
        );
        assert!(!html.contains("<script"));
        assert!(html.contains(
            "https://example.com/&quot;&gt;&lt;script&gt;alert(1)&lt;/script&gt;&amp;x=&#39;y&#39;"
        ));
        for not_a_link in [
            "javascript:alert(1)",
            "http://example.com/",
            "example.com",
            "",
        ] {
            let html = page_html(None, &[], not_a_link);
            assert!(!html.contains("소스 코드:"), "{not_a_link}");
            assert_eq!(html.matches("href=").count(), 1);
        }
        assert_eq!(
            esc("<a href=\"x\">&'</a>"),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#39;&lt;/a&gt;"
        );
        assert_eq!(
            [0, 7, 999, 1000, 25_400_320, 209_715_200].map(grouped),
            ["0", "7", "999", "1,000", "25,400,320", "209,715,200"]
        );
    }

    async fn sent_as(reply: Reply, head: bool) -> (Response<()>, Vec<u8>) {
        let (parts, body) = reply.into_response(head).into_parts();
        let bytes = body.collect().await.unwrap().to_bytes().to_vec();
        (Response::from_parts(parts, ()), bytes)
    }

    #[tokio::test]
    async fn the_heads_of_the_answers() {
        let dir = temp_dir();
        let path = dir.join("HJViewer-1.6.1-windows-x86_64.exe");
        let content: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &content).unwrap();
        let slots = Slots::new(1);
        let open = || open_plain(&path, Some(content.len() as u64)).unwrap();

        // a package
        let (file, size) = open();
        let reply = file_reply(file, size, slots.take(), "HJViewer-1.6.1-install.exe", true);
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(slots.free(), 0);
        let (head, body) = sent_as(reply, false).await;
        assert_eq!(body, content);
        assert_eq!(slots.free(), 1);
        let h = head.headers();
        for (name, value) in [
            (
                "content-type",
                "application/vnd.microsoft.portable-executable",
            ),
            ("content-length", "100000"),
            (
                "content-disposition",
                "attachment; filename=\"HJViewer-1.6.1-install.exe\"",
            ),
            ("cache-control", "private, no-store"),
            ("x-content-type-options", "nosniff"),
            ("x-robots-tag", "noindex, nofollow, noarchive"),
            ("referrer-policy", "no-referrer"),
            ("x-accel-buffering", "no"),
            ("x-accel-limit-rate", "3840000"),
            ("accept-ranges", "none"),
            ("content-security-policy", "default-src 'none'"),
        ] {
            assert_eq!(
                h.get(name).map(|v| v.to_str().unwrap()),
                Some(value),
                "{name}"
            );
            assert_eq!(h.get_all(name).iter().count(), 1, "{name}");
        }
        // HEAD: the same head, no body, no slot
        let (file, size) = open();
        let (head, body) = sent_as(
            file_reply(file, size, None, "HJViewer-1.6.1.exe", true),
            true,
        )
        .await;
        assert!(body.is_empty());
        assert_eq!(head.headers()["content-length"], "100000");
        assert_eq!(
            head.headers()["content-disposition"],
            "attachment; filename=\"HJViewer-1.6.1.exe\""
        );
        // a notice file shows in the browser
        let (file, size) = open();
        let (head, _) = sent_as(
            file_reply(file, size, None, "HJViewer-1.6.1-LICENSE.txt", false),
            false,
        )
        .await;
        assert_eq!(head.headers()["content-type"], "text/plain; charset=utf-8");
        assert_eq!(
            head.headers()["content-disposition"],
            "inline; filename=\"HJViewer-1.6.1-LICENSE.txt\""
        );
        let (file, size) = open();
        let (head, _) = sent_as(
            file_reply(file, size, None, "HJViewer-1.6.1-android-arm64.apk", true),
            true,
        )
        .await;
        assert_eq!(
            head.headers()["content-type"],
            "application/vnd.android.package-archive"
        );

        // 429 and 503 are JSON with Retry-After
        for (reply, status, code, secs) in [
            (too_often(4000), 429, "rate_limited", 4000),
            (too_often(0), 429, "rate_limited", 1),
            (too_often(10 * DAY), 429, "rate_limited", DAY),
            (busy(), 503, "busy", 60),
        ] {
            assert!(reply.body.contains(code));
            let (head, body) = sent_as(reply, false).await;
            assert_eq!(head.status().as_u16(), status);
            assert_eq!(head.headers()["retry-after"], secs.to_string().as_str());
            assert_eq!(
                head.headers()["content-type"],
                "application/json; charset=utf-8"
            );
            assert_eq!(head.headers()["cache-control"], "no-store");
            let json: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(
                (json["hjv"].as_str(), json["retry_after"].as_i64()),
                (Some(code), Some(secs))
            );
        }
        // an error on a path that a browser opens
        let (head, body) = sent_as(browser(no_link()), false).await;
        assert_eq!(head.status(), StatusCode::NOT_FOUND);
        assert_eq!(head.headers()["cache-control"], "private, no-store");
        assert_eq!(
            head.headers()["x-robots-tag"],
            "noindex, nofollow, noarchive"
        );
        assert_eq!(head.headers()["referrer-policy"], "no-referrer");
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["error"],
            TEXT_NO_LINK
        );
        // and a 429 there keeps its Retry-After
        let (head, _) = sent_as(browser(too_often(30)), false).await;
        assert_eq!(head.headers()["retry-after"], "30");
        assert_eq!(head.headers()["cache-control"], "private, no-store");
        assert_eq!(
            format!("{:?}", browser(too_often(30)).sent.unwrap()),
            "Sent(4 headers, the JSON)"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn texts_never_say_disabled() {
        // api.rs: the word makes the app empty its device list
        for text in [
            TEXT_BAD_REQUEST,
            TEXT_REFUSED,
            TEXT_NO_PACKAGE,
            TEXT_NO_LINK,
            TEXT_TOO_OFTEN,
            TEXT_BUSY,
            TEXT_MOVED,
        ] {
            assert!(!text.to_lowercase().contains("disabled"));
            assert!(!text.contains("://"));
        }
    }
}

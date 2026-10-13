// SPDX-License-Identifier: AGPL-3.0-only

//! The policy text (`p`) and the root declaration text (`root.p`), version 1 (design 1-1): their
//! fields, the rules each field keeps, and the one way the tool writes them.
//!
//! A policy never holds a URL or a secret: it is public, and an app builds every address itself
//! from its compiled-in server name. The tool reads only what it would write: every field known,
//! every rule kept, fields in the order below without spaces.

use crate::{
    json::{Fields, Json, Writer},
    time,
    version::Version,
};
use std::{collections::BTreeMap, fmt};

pub const POLICY_VERSION: u64 = 1;
pub const MAX_EPOCH: u64 = 1_000_000;
pub const MAX_SEQ: u64 = 1_000_000_000;
pub const MAX_PACKAGE_BYTES: u64 = 200 * 1024 * 1024;
pub const MAX_CONTACT_CHARS: usize = 200;
pub const MAX_NOTES_CHARS: usize = 2000;
/// Android's largest versionCode.
pub const MAX_VERSION_CODE: u64 = 2_100_000_000;
pub const MAX_REVOKED: usize = 64;

/// An entry of `platforms`. Android has two: the sideloaded APK and the Google Play install,
/// which cannot replace each other.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Platform {
    Windows,
    Android,
    AndroidPlay,
    Ios,
}

impl Platform {
    /// In the order the policy lists them.
    pub const ALL: [Platform; 4] = [
        Platform::Windows,
        Platform::Android,
        Platform::AndroidPlay,
        Platform::Ios,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Platform::Windows => "windows",
            Platform::Android => "android",
            Platform::AndroidPlay => "android_play",
            Platform::Ios => "ios",
        }
    }

    pub fn parse(s: &str) -> Option<Platform> {
        Platform::ALL.into_iter().find(|p| p.name() == s)
    }

    /// Whether the server hands out a package file for this entry.
    pub fn has_package(self) -> bool {
        matches!(self, Platform::Windows | Platform::Android)
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The file the server hands out for an entry, named as it lies in the server's package directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    pub file: String,
    pub size: u64,
    /// Lowercase hex.
    pub sha256: String,
    /// The Windows file version or the Android versionCode.
    pub build: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub min: Version,
    pub latest: Version,
    /// Until then an app below `min` only warns.
    pub grace_until: Option<i64>,
    /// The lowest OS `latest` runs on: an iOS version, an Android API level, a Windows major.
    pub os_min: Option<String>,
    pub notes_ko: Option<String>,
    /// The source tag of `latest`, such as `client-1.6.0`.
    pub source: Option<String>,
    /// Windows and Android.
    pub package: Option<Package>,
    /// Windows: from then on installed PCs install `latest` without a person, ring by ring.
    pub auto_install_after: Option<i64>,
    /// Android: sideloaded apps suggest moving to Google Play.
    pub move_to_play: bool,
    /// Google Play: the versionCode the operator saw released on the internal track.
    pub version_code: Option<u64>,
    /// iOS: the CFBundleVersion uploaded to TestFlight (`YYDDDHHMM`).
    pub testflight_build: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    /// Only the root key raises it.
    pub epoch: u64,
    /// Grows with every policy of an epoch.
    pub seq: u64,
    pub issued: i64,
    /// Must equal the app's compiled-in server name.
    pub server: String,
    pub contact_ko: Option<String>,
    pub platforms: BTreeMap<Platform, Entry>,
}

/// The root key's statement: the epoch, and every signing key revoked so far.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RootDecl {
    pub server: String,
    pub epoch: u64,
    /// Ascending, each number once.
    pub revoked: Vec<u32>,
    pub issued: i64,
}

/// A host name with an optional `:port`, as `HJV_SERVER` holds it.
pub fn check_server(server: &str) -> Result<(), String> {
    let (host, port) = match server.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (server, None),
    };
    let port_ok = port.is_none_or(|p| {
        !p.is_empty()
            && p.len() <= 5
            && p.bytes().all(|b| b.is_ascii_digit())
            && !p.starts_with('0')
            && p.parse::<u16>().is_ok()
    });
    let label_ok = |label: &str| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    };
    if port_ok && !host.is_empty() && host.len() <= 253 && host.split('.').all(label_ok) {
        Ok(())
    } else {
        Err(format!("서버 이름이 아닙니다: {server:?}"))
    }
}

/// Plain text for people, shown by the apps as it is: no URL (an app must never be led to
/// another site, and text is never made a link), no braces (the apps' translation layer reads
/// them as placeholders), no control characters.
fn check_text(what: &str, text: &str, max_chars: usize, line_feeds: bool) -> Result<(), String> {
    let chars = text.chars().count();
    if chars == 0 || chars > max_chars {
        return Err(format!(
            "{what}: 1자 이상 {max_chars}자 이하여야 합니다({chars}자)"
        ));
    }
    if text.trim() != text {
        return Err(format!("{what}: 앞뒤 공백을 빼 주세요"));
    }
    if text
        .chars()
        .any(|c| c.is_control() && !(line_feeds && c == '\n'))
    {
        return Err(format!("{what}: 제어 문자를 쓸 수 없습니다"));
    }
    // characters that show nothing or turn the text around (zero width, direction marks, BOM)
    let invisible = |c: char| matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2069}' | '\u{feff}');
    if text.chars().any(invisible) {
        return Err(format!("{what}: 보이지 않는 문자가 들어 있습니다"));
    }
    let lower = text.to_lowercase();
    if lower.contains("://") || lower.contains("www.") {
        return Err(format!(
            "{what}: URL을 넣을 수 없습니다(\"://\", \"www.\"). 앱은 문구를 링크로 만들지 않습니다"
        ));
    }
    if text.contains(['{', '}']) {
        return Err(format!("{what}: 중괄호를 쓸 수 없습니다"));
    }
    Ok(())
}

fn token(s: &str, max: usize) -> bool {
    !s.is_empty()
        && s.len() <= max
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// The packer of the Windows exe reads its own file name: one ending in `install.exe` opens the
/// installer and one with `-qs-` runs as quick support. A package an app or the service runs must
/// do neither (design 4-3).
pub fn check_package_file(platform: Platform, file: &str) -> Result<(), String> {
    let lower = file.to_ascii_lowercase();
    let extension = match platform {
        Platform::Windows => ".exe",
        _ => ".apk",
    };
    if !token(file, 100) || file.starts_with('.') || file.contains("..") {
        Err(format!(
            "패키지 파일 이름은 [A-Za-z0-9._-] 1-100자입니다: {file:?}"
        ))
    } else if lower.contains("-qs-") {
        Err(format!(
            "패키지 파일 이름에 -qs-를 넣을 수 없습니다: {file:?}"
        ))
    } else if lower.ends_with("install.exe") {
        Err(format!(
            "패키지 파일 이름은 install.exe로 끝날 수 없습니다: {file:?}"
        ))
    } else if !lower.ends_with(extension) || file.len() == extension.len() {
        Err(format!(
            "{platform} 패키지 파일 이름의 확장자는 {extension}입니다: {file:?}"
        ))
    } else {
        Ok(())
    }
}

pub fn check_sha256(s: &str) -> Result<(), String> {
    if s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        Ok(())
    } else {
        Err(format!("SHA-256은 소문자 hex 64자입니다: {s:?}"))
    }
}

/// Dotted numbers without leading zeros, 1-3 digits each: the fields of an `os_min`.
fn os_fields(s: &str) -> Option<Vec<u32>> {
    s.split('.')
        .map(|f| {
            let ok = !f.is_empty()
                && f.len() <= 3
                && f.bytes().all(|b| b.is_ascii_digit())
                && (f == "0" || !f.starts_with('0'));
            ok.then(|| f.parse().ok()).flatten()
        })
        .collect()
}

pub fn check_os_min(platform: Platform, s: &str) -> Result<(), String> {
    let fields = os_fields(s).unwrap_or_default();
    let ok = match platform {
        // the iOS version, compared field by field ("13.0")
        Platform::Ios => (1..=3).contains(&fields.len()),
        // the Android API level ("22") or the Windows major version ("10"), one number
        _ => fields.len() == 1,
    };
    if ok && fields[0] > 0 {
        Ok(())
    } else {
        Err(format!(
            "{platform}의 os_min이 맞지 않습니다(ios \"13.0\", android API 수준 \"22\", windows \"10\"): {s:?}"
        ))
    }
}

/// Whether `a` names a higher OS than `b`; both passed [`check_os_min`].
pub fn os_min_higher(a: &str, b: &str) -> bool {
    let (mut a, mut b) = (
        os_fields(a).unwrap_or_default(),
        os_fields(b).unwrap_or_default(),
    );
    let len = a.len().max(b.len());
    a.resize(len, 0);
    b.resize(len, 0);
    a > b
}

fn time_field(f: &mut Fields, name: &str) -> Result<Option<i64>, String> {
    f.opt_str(name)?
        .map(|s| time::parse_utc(s).map_err(|e| format!("{name}: {e}")))
        .transpose()
}

impl Package {
    fn read(mut f: Fields) -> Result<Package, String> {
        let package = Package {
            file: f.str("file")?.to_owned(),
            size: f.int("size")?,
            sha256: f.str("sha256")?.to_owned(),
            build: f.str("build")?.to_owned(),
        };
        f.done()?;
        Ok(package)
    }

    fn check(&self, platform: Platform) -> Result<(), String> {
        check_package_file(platform, &self.file)?;
        if self.size == 0 || self.size > MAX_PACKAGE_BYTES {
            return Err(format!(
                "패키지 크기는 1바이트 이상 200 MiB 이하입니다: {}",
                self.size
            ));
        }
        check_sha256(&self.sha256)?;
        let b = self.build.as_bytes();
        let build_ok = !b.is_empty()
            && b.len() <= 32
            && b.iter().all(|c| c.is_ascii_digit() || *c == b'.')
            && b[0].is_ascii_digit()
            && b[b.len() - 1].is_ascii_digit();
        if !build_ok {
            return Err(format!("패키지 build는 숫자와 점입니다: {:?}", self.build));
        }
        Ok(())
    }

    fn to_text(&self) -> String {
        let mut w = Writer::new();
        w.str("file", &self.file)
            .int("size", self.size)
            .str("sha256", &self.sha256)
            .str("build", &self.build);
        w.finish()
    }
}

impl Entry {
    /// An entry with the two versions and nothing else.
    pub fn new(min: Version, latest: Version) -> Entry {
        Entry {
            min,
            latest,
            grace_until: None,
            os_min: None,
            notes_ko: None,
            source: None,
            package: None,
            auto_install_after: None,
            move_to_play: false,
            version_code: None,
            testflight_build: None,
        }
    }

    fn read(mut f: Fields) -> Result<Entry, String> {
        let version = |f: &mut Fields, name: &str| -> Result<Version, String> {
            Version::parse(f.str(name)?).map_err(|e| format!("{name}: {e}"))
        };
        let entry = Entry {
            min: version(&mut f, "min")?,
            latest: version(&mut f, "latest")?,
            grace_until: time_field(&mut f, "grace_until")?,
            os_min: f.opt_str("os_min")?.map(str::to_owned),
            notes_ko: f.opt_str("notes_ko")?.map(str::to_owned),
            source: f.opt_str("source")?.map(str::to_owned),
            package: f
                .opt_object("package", "package")?
                .map(Package::read)
                .transpose()?,
            auto_install_after: time_field(&mut f, "auto_install_after")?,
            move_to_play: match f.opt_bool("move_to_play")? {
                None => false,
                Some(true) => true,
                Some(false) => return Err("move_to_play는 true이거나 없어야 합니다".to_owned()),
            },
            version_code: f.opt_int("version_code")?,
            testflight_build: f.opt_str("testflight_build")?.map(str::to_owned),
        };
        f.done()?;
        Ok(entry)
    }

    fn check(&self, platform: Platform) -> Result<(), String> {
        if self.min > self.latest {
            return Err(format!(
                "min({})이 latest({})보다 높습니다",
                self.min, self.latest
            ));
        }
        if let Some(os_min) = &self.os_min {
            check_os_min(platform, os_min)?;
        }
        if let Some(notes) = &self.notes_ko {
            check_text("notes_ko", notes, MAX_NOTES_CHARS, true)?;
        }
        if let Some(source) = &self.source {
            if !token(source, 64) {
                return Err(format!(
                    "source는 client-1.6.0 같은 태그 이름입니다: {source:?}"
                ));
            }
        }
        match (&self.package, platform.has_package()) {
            (Some(package), true) => package.check(platform)?,
            (None, true) => return Err("package가 없습니다".to_owned()),
            (Some(_), false) => return Err("package를 둘 수 없는 항목입니다".to_owned()),
            (None, false) => {}
        }
        if self.auto_install_after.is_some() && platform != Platform::Windows {
            return Err("auto_install_after는 windows에만 둡니다".to_owned());
        }
        if self.move_to_play && platform != Platform::Android {
            return Err("move_to_play는 android에만 둡니다".to_owned());
        }
        match (self.version_code, platform == Platform::AndroidPlay) {
            (Some(code), true) if (1..=MAX_VERSION_CODE).contains(&code) => {}
            (Some(code), true) => return Err(format!("version_code가 범위 밖입니다: {code}")),
            (None, true) => return Err("version_code가 없습니다".to_owned()),
            (Some(_), false) => return Err("version_code는 android_play에만 둡니다".to_owned()),
            (None, false) => {}
        }
        match (&self.testflight_build, platform == Platform::Ios) {
            (Some(build), true) => drop(time::build_number_time(build)?),
            (None, true) => return Err("testflight_build가 없습니다".to_owned()),
            (Some(_), false) => return Err("testflight_build는 ios에만 둡니다".to_owned()),
            (None, false) => {}
        }
        Ok(())
    }

    fn to_text(&self) -> String {
        let mut w = Writer::new();
        w.str("min", &self.min.to_string())
            .str("latest", &self.latest.to_string())
            .opt_str(
                "grace_until",
                self.grace_until.map(time::format_utc).as_deref(),
            )
            .opt_str("os_min", self.os_min.as_deref())
            .opt_str("notes_ko", self.notes_ko.as_deref())
            .opt_str("source", self.source.as_deref());
        if let Some(package) = &self.package {
            w.raw("package", &package.to_text());
        }
        w.opt_str(
            "auto_install_after",
            self.auto_install_after.map(time::format_utc).as_deref(),
        );
        if self.move_to_play {
            w.bool("move_to_play", true);
        }
        if let Some(code) = self.version_code {
            w.int("version_code", code);
        }
        w.opt_str("testflight_build", self.testflight_build.as_deref());
        w.finish()
    }
}

fn check_counters(epoch: u64, seq: Option<u64>) -> Result<(), String> {
    if !(1..=MAX_EPOCH).contains(&epoch) {
        return Err(format!(
            "세대(epoch)는 1 이상 {MAX_EPOCH} 이하입니다: {epoch}"
        ));
    }
    match seq {
        Some(seq) if !(1..=MAX_SEQ).contains(&seq) => {
            Err(format!("seq는 1 이상 {MAX_SEQ} 이하입니다: {seq}"))
        }
        _ => Ok(()),
    }
}

impl Policy {
    /// Reads a policy and checks every field rule; the fields may come in any order.
    pub fn read(text: &str) -> Result<Policy, String> {
        let value = Json::parse(text).map_err(|e| format!("정책이 JSON이 아닙니다({e})"))?;
        let mut f = value.fields("정책")?;
        let v = f.int("v")?;
        if v != POLICY_VERSION {
            return Err(format!("모르는 정책 버전 {v}"));
        }
        let mut policy = Policy {
            epoch: f.int("epoch")?,
            seq: f.int("seq")?,
            issued: time::parse_utc(f.str("issued")?).map_err(|e| format!("issued: {e}"))?,
            server: f.str("server")?.to_owned(),
            contact_ko: f.opt_str("contact_ko")?.map(str::to_owned),
            platforms: BTreeMap::new(),
        };
        let mut platforms = f.object("platforms", "platforms")?;
        f.done()?;
        for platform in Platform::ALL {
            if let Some(entry) = platforms.take(platform.name()) {
                let entry = entry
                    .fields(platform.name())
                    .and_then(Entry::read)
                    .map_err(|e| format!("{platform}: {e}"))?;
                policy.platforms.insert(platform, entry);
            }
        }
        platforms.done()?;
        policy.check()?;
        Ok(policy)
    }

    /// Reads a policy that must be exactly what [`Policy::to_text`] writes.
    pub fn parse(text: &str) -> Result<Policy, String> {
        let policy = Policy::read(text)?;
        if policy.to_text() != text {
            return Err(
                "정책 본문이 이 도구가 쓰는 형식(필드 순서, 공백 없음)과 다릅니다".to_owned(),
            );
        }
        Ok(policy)
    }

    /// The field rules of design 1-1 and 1-4 that one policy can be checked against by itself.
    pub fn check(&self) -> Result<(), String> {
        check_counters(self.epoch, Some(self.seq))?;
        check_server(&self.server)?;
        if let Some(contact) = &self.contact_ko {
            check_text("contact_ko", contact, MAX_CONTACT_CHARS, false)?;
        }
        if self.platforms.is_empty() {
            return Err("platforms에 항목이 없습니다".to_owned());
        }
        for (platform, entry) in &self.platforms {
            entry
                .check(*platform)
                .map_err(|e| format!("{platform}: {e}"))?;
        }
        Ok(())
    }

    /// The policy text: fields in the order of design 1-1, no spaces, Korean text as it is.
    pub fn to_text(&self) -> String {
        let mut platforms = Writer::new();
        for (platform, entry) in &self.platforms {
            platforms.raw(platform.name(), &entry.to_text());
        }
        let mut w = Writer::new();
        w.int("v", POLICY_VERSION)
            .int("epoch", self.epoch)
            .int("seq", self.seq)
            .str("issued", &time::format_utc(self.issued))
            .str("server", &self.server)
            .opt_str("contact_ko", self.contact_ko.as_deref())
            .raw("platforms", &platforms.finish());
        w.finish()
    }

    /// The lowest `min` of all entries: the one threshold hbbs can enforce, since a punch hole
    /// request names a version and no platform.
    pub fn lowest_min(&self) -> Option<Version> {
        self.platforms.values().map(|e| e.min).min()
    }
}

impl RootDecl {
    pub fn read(text: &str) -> Result<RootDecl, String> {
        let value = Json::parse(text).map_err(|e| format!("루트 선언이 JSON이 아닙니다({e})"))?;
        let mut f = value.fields("루트 선언")?;
        let v = f.int("v")?;
        if v != POLICY_VERSION {
            return Err(format!("모르는 루트 선언 버전 {v}"));
        }
        let server = f.str("server")?.to_owned();
        let epoch = f.int("epoch")?;
        let revoked = match f.take("revoked") {
            Some(Json::Array(items)) => items
                .iter()
                .map(|item| match item {
                    Json::Int(n) => u32::try_from(*n).ok().filter(|n| (1..=65535).contains(n)),
                    _ => None,
                })
                .collect::<Option<Vec<u32>>>()
                .ok_or("루트 선언: revoked는 키 번호(1-65535) 목록입니다")?,
            _ => return Err("루트 선언: revoked 목록이 없습니다".to_owned()),
        };
        let issued = time::parse_utc(f.str("issued")?).map_err(|e| format!("issued: {e}"))?;
        f.done()?;
        let decl = RootDecl {
            server,
            epoch,
            revoked,
            issued,
        };
        decl.check()?;
        Ok(decl)
    }

    /// Reads a root declaration that must be exactly what [`RootDecl::to_text`] writes.
    pub fn parse(text: &str) -> Result<RootDecl, String> {
        let decl = RootDecl::read(text)?;
        if decl.to_text() != text {
            return Err(
                "루트 선언이 이 도구가 쓰는 형식(필드 순서, 공백 없음)과 다릅니다".to_owned(),
            );
        }
        Ok(decl)
    }

    pub fn check(&self) -> Result<(), String> {
        check_counters(self.epoch, None)?;
        check_server(&self.server)?;
        if self.revoked.len() > MAX_REVOKED {
            return Err(format!("루트 선언: 폐기 목록은 {MAX_REVOKED}개까지입니다"));
        }
        if self.revoked.windows(2).any(|w| w[0] >= w[1]) {
            return Err("루트 선언: revoked는 오름차순으로, 번호마다 한 번만 씁니다".to_owned());
        }
        if self.revoked.iter().any(|n| !(1..=65535).contains(n)) {
            return Err("루트 선언: revoked는 키 번호(1-65535) 목록입니다".to_owned());
        }
        Ok(())
    }

    pub fn to_text(&self) -> String {
        let revoked: Vec<String> = self.revoked.iter().map(u32::to_string).collect();
        let mut w = Writer::new();
        w.int("v", POLICY_VERSION)
            .str("server", &self.server)
            .int("epoch", self.epoch)
            .raw("revoked", &format!("[{}]", revoked.join(",")))
            .str("issued", &time::format_utc(self.issued));
        w.finish()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    pub(crate) fn t(s: &str) -> i64 {
        time::parse_utc(s).unwrap()
    }

    /// A policy with all four entries and every field in use.
    pub(crate) fn sample() -> Policy {
        let mut platforms = BTreeMap::new();
        platforms.insert(
            Platform::Windows,
            Entry {
                grace_until: Some(t("2026-11-22T15:00:00Z")),
                os_min: Some("10".to_owned()),
                notes_ko: Some("연결 안정성 개선\n한글 입력 수정".to_owned()),
                source: Some("client-1.6.1".to_owned()),
                package: Some(Package {
                    file: "HJViewer-1.6.1-windows-x86_64.exe".to_owned(),
                    size: 26_214_400,
                    sha256: "e6a39ba1c067edf949f7e2906b801d919d1f36f4fd52987d149303e5f11a8607"
                        .to_owned(),
                    build: "1.6.1.7".to_owned(),
                }),
                auto_install_after: Some(t("2026-11-10T03:00:00Z")),
                ..Entry::new(v("1.6.0"), v("1.6.1"))
            },
        );
        platforms.insert(
            Platform::Android,
            Entry {
                os_min: Some("22".to_owned()),
                source: Some("client-1.6.1".to_owned()),
                package: Some(Package {
                    file: "HJViewer-1.6.1-android-arm64.apk".to_owned(),
                    size: 30_408_704,
                    sha256: "2183b8d680809f991816eb45ba8661c9f686111223fdecc35e1c0cbf56569d96"
                        .to_owned(),
                    build: "263122251".to_owned(),
                }),
                move_to_play: true,
                ..Entry::new(v("1.6.0"), v("1.6.1"))
            },
        );
        platforms.insert(
            Platform::AndroidPlay,
            Entry {
                version_code: Some(263_122_251),
                ..Entry::new(v("1.6.0"), v("1.6.1"))
            },
        );
        platforms.insert(
            Platform::Ios,
            Entry {
                os_min: Some("13.0".to_owned()),
                testflight_build: Some("263120249".to_owned()),
                ..Entry::new(v("1.6.0"), v("1.6.1"))
            },
        );
        Policy {
            epoch: 1,
            seq: 7,
            issued: t("2026-11-08T03:00:00Z"),
            server: "rd.example.com".to_owned(),
            contact_ko: Some("설치 파일은 관리자에게 받으세요.".to_owned()),
            platforms,
        }
    }

    #[test]
    fn the_text_is_fixed() {
        let text = sample().to_text();
        assert_eq!(
            text,
            concat!(
                r#"{"v":1,"epoch":1,"seq":7,"issued":"2026-11-08T03:00:00Z","server":"rd.example.com","#,
                r#""contact_ko":"설치 파일은 관리자에게 받으세요.","platforms":{"#,
                r#""windows":{"min":"1.6.0","latest":"1.6.1","grace_until":"2026-11-22T15:00:00Z","os_min":"10","#,
                r#""notes_ko":"연결 안정성 개선\n한글 입력 수정","source":"client-1.6.1","#,
                r#""package":{"file":"HJViewer-1.6.1-windows-x86_64.exe","size":26214400,"#,
                r#""sha256":"e6a39ba1c067edf949f7e2906b801d919d1f36f4fd52987d149303e5f11a8607","build":"1.6.1.7"},"#,
                r#""auto_install_after":"2026-11-10T03:00:00Z"},"#,
                r#""android":{"min":"1.6.0","latest":"1.6.1","os_min":"22","source":"client-1.6.1","#,
                r#""package":{"file":"HJViewer-1.6.1-android-arm64.apk","size":30408704,"#,
                r#""sha256":"2183b8d680809f991816eb45ba8661c9f686111223fdecc35e1c0cbf56569d96","build":"263122251"},"#,
                r#""move_to_play":true},"#,
                r#""android_play":{"min":"1.6.0","latest":"1.6.1","version_code":263122251},"#,
                r#""ios":{"min":"1.6.0","latest":"1.6.1","os_min":"13.0","testflight_build":"263120249"}}}"#
            )
        );
        assert_eq!(Policy::parse(&text).unwrap(), sample());
        assert_eq!(sample().lowest_min(), Some(v("1.6.0")));
    }

    #[test]
    fn reads_any_order_but_parses_one() {
        let text = r#"{ "platforms": {"ios": {"latest":"1.6.0","min":"1.5.0","testflight_build":"262840251"}},
            "server":"hjv.test", "issued":"2026-10-01T00:00:00Z", "seq":1, "epoch":1, "v":1 }"#;
        let policy = Policy::read(text).unwrap();
        assert_eq!(policy.platforms[&Platform::Ios].min, v("1.5.0"));
        assert!(Policy::parse(text).unwrap_err().contains("형식"));
        assert_eq!(Policy::parse(&policy.to_text()).unwrap(), policy);
    }

    fn change(f: impl FnOnce(&mut Policy)) -> Result<(), String> {
        let mut policy = sample();
        f(&mut policy);
        policy.check()?;
        // what passes the rules also survives writing and reading
        assert_eq!(Policy::parse(&policy.to_text()).unwrap(), policy);
        Ok(())
    }

    fn windows(p: &mut Policy) -> &mut Entry {
        p.platforms.get_mut(&Platform::Windows).unwrap()
    }

    #[test]
    fn field_rules() {
        assert!(change(|_| {}).is_ok());
        assert!(change(|p| p.epoch = 0).is_err());
        assert!(change(|p| p.seq = 0).is_err());
        assert!(change(|p| p.seq = MAX_SEQ + 1).is_err());
        assert!(change(|p| p.epoch = MAX_EPOCH + 1).is_err());
        assert!(change(|p| p.platforms.clear()).is_err());
        assert!(change(|p| windows(p).min = v("1.6.2")).is_err());
        assert!(change(|p| windows(p).min = v("1.6.1")).is_ok());
        // packages
        assert!(change(|p| windows(p).package = None).is_err());
        assert!(change(|p| windows(p).package.as_mut().unwrap().size = 0).is_err());
        assert!(change(|p| windows(p).package.as_mut().unwrap().size = MAX_PACKAGE_BYTES).is_ok());
        assert!(
            change(|p| windows(p).package.as_mut().unwrap().size = MAX_PACKAGE_BYTES + 1).is_err()
        );
        for sha in [
            "",
            "e6a3",
            &"E".repeat(64),
            &"g".repeat(64),
            &"0".repeat(65),
        ] {
            assert!(
                change(|p| windows(p).package.as_mut().unwrap().sha256 = sha.to_owned()).is_err()
            );
        }
        for build in ["", "1.6.1.", ".1", "1.6.1-7", "v1", &"1".repeat(33)] {
            assert!(
                change(|p| windows(p).package.as_mut().unwrap().build = build.to_owned()).is_err()
            );
        }
        assert!(change(
            |p| p.platforms.get_mut(&Platform::Ios).unwrap().package = windows(p).package.clone()
        )
        .is_err());
        // fields of one platform only
        assert!(change(|p| p
            .platforms
            .get_mut(&Platform::Android)
            .unwrap()
            .auto_install_after = Some(0))
        .is_err());
        assert!(change(|p| windows(p).move_to_play = true).is_err());
        assert!(change(|p| windows(p).version_code = Some(1)).is_err());
        assert!(change(|p| windows(p).testflight_build = Some("262840251".to_owned())).is_err());
        assert!(change(|p| p
            .platforms
            .get_mut(&Platform::AndroidPlay)
            .unwrap()
            .version_code = None)
        .is_err());
        assert!(change(|p| p
            .platforms
            .get_mut(&Platform::AndroidPlay)
            .unwrap()
            .version_code = Some(0))
        .is_err());
        assert!(change(|p| p
            .platforms
            .get_mut(&Platform::AndroidPlay)
            .unwrap()
            .version_code = Some(MAX_VERSION_CODE + 1))
        .is_err());
        assert!(change(|p| p
            .platforms
            .get_mut(&Platform::Ios)
            .unwrap()
            .testflight_build = None)
        .is_err());
        assert!(change(|p| p
            .platforms
            .get_mut(&Platform::Ios)
            .unwrap()
            .testflight_build = Some("26284025".to_owned()))
        .is_err());
        // an entry may be missing
        assert!(change(|p| drop(p.platforms.remove(&Platform::AndroidPlay))).is_ok());
        // source
        assert!(change(|p| windows(p).source = Some("client 1.6.1".to_owned())).is_err());
        assert!(change(|p| windows(p).source = Some(String::new())).is_err());
    }

    #[test]
    fn package_file_names() {
        for (platform, file) in [
            (Platform::Windows, "HJViewer-1.6.1-windows-x86_64.exe"),
            (Platform::Windows, "HJViewer-1.6.1.exe"),
            (Platform::Android, "HJViewer-1.6.1-android-arm64.apk"),
        ] {
            assert!(check_package_file(platform, file).is_ok(), "{file}");
        }
        for (platform, file) in [
            (Platform::Windows, "HJViewer-1.6.1-install.exe"),
            (Platform::Windows, "HJViewer-install.exe"),
            (Platform::Windows, "install.exe"),
            (Platform::Windows, "HJViewer-INSTALL.EXE"),
            (Platform::Windows, "HJViewer-qs-1.6.1.exe"),
            (Platform::Windows, "HJViewer-QS-1.6.1.exe"),
            (Platform::Windows, "HJViewer-1.6.1.apk"),
            (Platform::Android, "HJViewer-1.6.1.exe"),
            (Platform::Windows, ".exe"),
            (Platform::Windows, "..exe"),
            (Platform::Windows, "a..b.exe"),
            (Platform::Windows, ".hidden.exe"),
            (Platform::Windows, "dir/HJViewer.exe"),
            (Platform::Windows, "dir\\HJViewer.exe"),
            (Platform::Windows, "HJ Viewer.exe"),
            (Platform::Windows, "한글.exe"),
            (Platform::Windows, ""),
        ] {
            assert!(
                check_package_file(platform, file).is_err(),
                "{file:?} passed"
            );
        }
        let long = format!("{}.exe", "a".repeat(97));
        assert!(check_package_file(Platform::Windows, &long[1..]).is_ok());
        assert!(check_package_file(Platform::Windows, &long).is_err());
    }

    #[test]
    fn texts_have_no_urls() {
        let contact = |s: &str| change(|p| p.contact_ko = Some(s.to_owned()));
        assert!(contact("관리자(내선 1234)에게 받으세요.").is_ok());
        assert!(contact(&"가".repeat(MAX_CONTACT_CHARS)).is_ok());
        assert!(contact(&"가".repeat(MAX_CONTACT_CHARS + 1)).is_err());
        for bad in [
            "",
            " 앞 공백",
            "https://rd.example.com 에서 받으세요",
            "HTTPS://RD.EXAMPLE.COM",
            "ftp://x",
            "www.example.com 에서",
            "WWW.example.com",
            "줄\n바꿈",
            "탭\t문자",
            "중괄호 {0}",
            "닫는 }",
            "\u{1b}[31m색",
            "보이지 않는\u{200b} 문자",
            "방향을 \u{202e}바꾸는 문자",
            "\u{feff}BOM",
        ] {
            assert!(contact(bad).is_err(), "{bad:?} passed");
        }
        let notes = |s: &str| change(|p| windows(p).notes_ko = Some(s.to_owned()));
        assert!(notes("첫 줄\n둘째 줄").is_ok());
        assert!(notes(&"가".repeat(MAX_NOTES_CHARS)).is_ok());
        assert!(notes(&"가".repeat(MAX_NOTES_CHARS + 1)).is_err());
        assert!(notes("자세한 내용은 https://rd.example.com/notes").is_err());
        assert!(notes("자세한 내용은 www.example.com").is_err());
        assert!(notes("캐리지\r\n리턴").is_err());
    }

    #[test]
    fn os_minimums() {
        assert!(check_os_min(Platform::Ios, "13.0").is_ok());
        assert!(check_os_min(Platform::Ios, "15").is_ok());
        assert!(check_os_min(Platform::Ios, "17.5.1").is_ok());
        assert!(check_os_min(Platform::Android, "22").is_ok());
        assert!(check_os_min(Platform::AndroidPlay, "24").is_ok());
        assert!(check_os_min(Platform::Windows, "10").is_ok());
        for (platform, bad) in [
            (Platform::Ios, ""),
            (Platform::Ios, "13."),
            (Platform::Ios, "13.0.0.0"),
            (Platform::Ios, "013.0"),
            (Platform::Ios, "iOS 13"),
            (Platform::Ios, "0.1"),
            (Platform::Android, "5.1"),
            (Platform::Android, "API 22"),
            (Platform::Android, "0"),
            (Platform::Windows, "10.0"),
            (Platform::Windows, "1000"),
        ] {
            assert!(check_os_min(platform, bad).is_err(), "{bad:?} passed");
        }
        assert!(os_min_higher("15.0", "13.0"));
        assert!(os_min_higher("13.1", "13"));
        assert!(os_min_higher("24", "22"));
        assert!(os_min_higher("100", "22"));
        assert!(!os_min_higher("13.0", "13"));
        assert!(!os_min_higher("13", "13.0.0"));
        assert!(!os_min_higher("9", "10"));
    }

    #[test]
    fn servers() {
        for ok in [
            "rd.example.com",
            "hjv.test",
            "localhost",
            "rd.example.com:8443",
            "192.0.2.10",
        ] {
            assert!(check_server(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "https://rd.example.com",
            "rd.example.com/",
            "rd.example.com:0443",
            "rd.example.com:",
            "rd.example.com:65536",
            "-rd.example.com",
            "rd..example.com",
            "user@rd.example.com",
            "rd.example.com ",
            "rd_example.com",
            "한글.example.com",
        ] {
            assert!(check_server(bad).is_err(), "{bad:?} passed");
        }
    }

    #[test]
    fn refuses_other_shapes() {
        let good = sample().to_text();
        for (from, to) in [
            (r#""v":1,"epoch""#, r#""v":2,"epoch""#),
            (r#""epoch":1"#, r#""epoch":"1""#),
            (r#""seq":7"#, r#""seq":7.0"#),
            (r#""seq":7,"#, r#""seq":7,"seq":8,"#),
            (
                r#""seq":7,"#,
                r#""seq":7,"expires":"2027-01-01T00:00:00Z","#,
            ),
            (
                r#""issued":"2026-11-08T03:00:00Z""#,
                r#""issued":"2026-11-08 03:00""#,
            ),
            (
                r#""issued":"2026-11-08T03:00:00Z""#,
                r#""issued":1793934000"#,
            ),
            (
                r#""server":"rd.example.com""#,
                r#""server":"https://rd.example.com""#,
            ),
            (
                r#""min":"1.6.0","latest":"1.6.1","grace"#,
                r#""min":"1.6","latest":"1.6.1","grace"#,
            ),
            (r#""latest":"1.6.1","grace"#, r#""latest":"1.6.1+7","grace"#),
            (r#""size":26214400"#, r#""size":"26214400""#),
            (r#""move_to_play":true"#, r#""move_to_play":false"#),
            (r#""move_to_play":true"#, r#""move_to_play":1"#),
            (
                r#""version_code":263122251"#,
                r#""version_code":"263122251""#,
            ),
            (
                r#""testflight_build":"263120249""#,
                r#""testflight_build":263120249"#,
            ),
            (r#""os_min":"10""#, r#""os_min":10"#),
            (r#""build":"1.6.1.7"}"#, r#""build":"1.6.1.7","url":"x"}"#),
            (
                r#""ios":{"#,
                r#""linux":{"min":"1.6.0","latest":"1.6.1"},"ios":{"#,
            ),
            (r#""ios":{"#, r#""ios":{"url":"x","#),
            (r#"{"v":1,"#, r#"{ "v":1,"#),
        ] {
            assert!(good.contains(from), "{from} is not in the sample");
            let bad = good.replacen(from, to, 1);
            assert!(Policy::parse(&bad).is_err(), "{to} parsed");
        }
        // the research vector's first draft of the format has no epoch
        assert!(Policy::read(r#"{"v":1,"seq":7,"issued":"2026-11-02T00:00:00Z","server":"rd.example.com","platforms":{}}"#).is_err());
    }

    #[test]
    fn root_declarations() {
        let decl = RootDecl {
            server: "rd.example.com".to_owned(),
            epoch: 2,
            revoked: vec![1, 3],
            issued: t("2026-11-08T03:00:00Z"),
        };
        let text = decl.to_text();
        assert_eq!(
            text,
            r#"{"v":1,"server":"rd.example.com","epoch":2,"revoked":[1,3],"issued":"2026-11-08T03:00:00Z"}"#
        );
        assert_eq!(RootDecl::parse(&text).unwrap(), decl);
        let empty = RootDecl {
            revoked: Vec::new(),
            ..decl.clone()
        };
        assert!(empty.to_text().contains(r#""revoked":[]"#));
        assert_eq!(RootDecl::parse(&empty.to_text()).unwrap(), empty);
        for (from, to) in [
            (r#""v":1"#, r#""v":2"#),
            (r#""epoch":2"#, r#""epoch":0"#),
            (r#""revoked":[1,3]"#, r#""revoked":[3,1]"#),
            (r#""revoked":[1,3]"#, r#""revoked":[1,1]"#),
            (r#""revoked":[1,3]"#, r#""revoked":[0]"#),
            (r#""revoked":[1,3]"#, r#""revoked":[65536]"#),
            (r#""revoked":[1,3]"#, r#""revoked":["1"]"#),
            (r#""revoked":[1,3]"#, r#""revoked":1"#),
            (r#""revoked":[1,3],"#, ""),
            (r#""revoked":[1,3]"#, r#""revoked":[1, 3]"#),
            (r#""server":"rd.example.com""#, r#""server":"""#),
            (r#""epoch":2,"#, r#""epoch":2,"seq":1,"#),
        ] {
            assert!(text.contains(from));
            let bad = text.replacen(from, to, 1);
            assert!(RootDecl::parse(&bad).is_err(), "{to} parsed");
        }
    }
}

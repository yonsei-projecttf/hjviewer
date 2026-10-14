// SPDX-License-Identifier: AGPL-3.0-only

//! The policy text (`p`) and the declaration text (`root.p`), version 1 (trust rules 1-3): their
//! members, the rules each keeps, and the one way the tool writes them.
//!
//! A text is read in two steps, as the trust rules separate them:
//!
//! - its **form** ([`PolicyForm`], [`DeclForm`]): every member known, of its JSON type, none
//!   `null` or given twice. A text is **canonical** when it is also exactly what the tool writes
//!   from that form (member order of 1-3, no spaces, serde_json's escaping). The tool reads
//!   canonical texts only; the client alone reads the others (spec 4-1);
//! - its **values**, by the app's rules in the app's order, refused with the app's reason codes
//!   ([`PolicyForm::judge`], [`DeclForm::judge`]): what the tool's simulated app answers.
//!
//! The tool keeps rules of its own on top ([`Policy::check`], [`RootDecl::check`]): no URL in a
//! text, the package and platform rules of the design, key numbers 1-100 in `revoked`. A policy
//! never holds a URL or a secret: it is public, and an app builds every address itself from its
//! compiled-in server name.

use crate::{
    json::{Fields, Json, Writer},
    time,
    trust::Reason,
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

    /// The other entry of the Android pair (trust rules 1-1).
    pub fn sibling(self) -> Option<Platform> {
        match self {
            Platform::Android => Some(Platform::AndroidPlay),
            Platform::AndroidPlay => Some(Platform::Android),
            _ => None,
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The requirement part of an entry (trust rules 1-2): `min`, `latest`, and the pair
/// `grace_until` / `grace_min`, both or neither. It is what a declaration's base holds for an
/// entry and all that COVERS and the verdicts read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Requirement {
    pub min: Version,
    pub latest: Version,
    pub grace_until: Option<i64>,
    pub grace_min: Option<Version>,
}

impl Requirement {
    pub fn new(min: Version, latest: Version) -> Requirement {
        Requirement {
            min,
            latest,
            grace_until: None,
            grace_min: None,
        }
    }

    /// `lo(e)`: what is blocked before the deadline (1-2).
    pub fn lo(&self) -> i64 {
        match (self.grace_until, self.grace_min) {
            (Some(_), Some(grace_min)) => grace_min.number(),
            _ => self.min.number(),
        }
    }

    /// `hi(e)`: what is blocked from the deadline on.
    pub fn hi(&self) -> i64 {
        self.min.number()
    }

    /// `T(e)`: the deadline, when the entry has the pair.
    pub fn deadline(&self) -> Option<i64> {
        self.grace_until.filter(|_| self.grace_min.is_some())
    }

    /// `THRESHOLD(e, t)`: a version is blocked at `t` iff its number is below this.
    pub fn threshold(&self, t: i64) -> i64 {
        match self.deadline() {
            Some(deadline) if t < deadline => self.lo(),
            _ => self.hi(),
        }
    }

    /// The member whose rule of 1-3 this breaks first, in C3's order: `min` above `latest` is
    /// `min`; the pair rule and `grace_min` below `min` are `grace_min`.
    pub fn broken(&self) -> Option<&'static str> {
        if self.min > self.latest {
            return Some("min");
        }
        match (self.grace_until, self.grace_min) {
            (None, None) => None,
            (Some(_), Some(grace_min)) if grace_min < self.min => None,
            _ => Some("grace_min"),
        }
    }

    /// For people: what the entry blocks now and from its deadline on.
    pub fn describe(&self) -> String {
        match (self.grace_until, self.grace_min) {
            (Some(until), Some(grace_min)) => format!(
                "{} 미만을 막음, {}부터 {} 미만을 막음",
                grace_min,
                time::format_kst(until),
                self.min
            ),
            _ => format!("{} 미만을 막음", self.min),
        }
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
    /// From then on versions below `min` are blocked; before it, versions below `grace_min`.
    pub grace_until: Option<i64>,
    /// What is blocked until `grace_until`; present iff `grace_until` is.
    pub grace_min: Option<Version>,
    /// The lowest OS `latest` runs on: an iOS version, an Android API level, a Windows major.
    pub os_min: Option<String>,
    pub notes_ko: Option<String>,
    /// The source tag of `latest`, such as `client-1.6.0`.
    pub source: Option<String>,
    /// Windows and Android; withdrawn (`--withdraw-package`) until a higher `latest`.
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
    /// Moved only by a declaration.
    pub epoch: u64,
    /// Grows with every policy of an epoch.
    pub seq: u64,
    pub issued: i64,
    /// Must equal the app's compiled-in server name.
    pub server: String,
    pub contact_ko: Option<String>,
    pub platforms: BTreeMap<Platform, Entry>,
}

/// A root key's declaration (trust rules 1-3): a new epoch, every signing key revoked so far,
/// and the base, the requirement part of the epoch's first policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RootDecl {
    pub server: String,
    /// At least 2: epoch 1 never has a declaration.
    pub epoch: u64,
    /// Ascending, each number once, numbers 1-100 only (the tool's rule).
    pub revoked: Vec<u32>,
    pub issued: i64,
    pub base: BTreeMap<Platform, Requirement>,
}

// ---------------------------------------------------------------------------------------------
// The form of a text: members, JSON types, nothing judged yet
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageForm {
    pub file: String,
    pub size: u64,
    pub sha256: String,
    pub build: String,
}

/// An entry as written: versions and times are strings until they are judged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryForm {
    pub min: String,
    pub latest: String,
    pub grace_until: Option<String>,
    pub grace_min: Option<String>,
    pub os_min: Option<String>,
    pub notes_ko: Option<String>,
    pub source: Option<String>,
    pub package: Option<PackageForm>,
    pub auto_install_after: Option<String>,
    pub move_to_play: bool,
    pub version_code: Option<u64>,
    pub testflight_build: Option<String>,
}

/// A policy text read for its form (trust rules 1-3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyForm {
    pub v: u64,
    pub epoch: u64,
    pub seq: u64,
    pub issued: String,
    pub server: String,
    pub contact_ko: Option<String>,
    pub platforms: Vec<(Platform, EntryForm)>,
}

/// A base entry as written: every member may be missing (a missing one is the app's RootFormat).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaseForm {
    pub min: Option<String>,
    pub latest: Option<String>,
    pub grace_until: Option<String>,
    pub grace_min: Option<String>,
}

/// A declaration text read for its form. A missing member is not a matter of form here: the app
/// refuses it as RootFormat, as it does every other broken rule of a declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeclForm {
    pub v: Option<u64>,
    pub server: Option<String>,
    pub epoch: Option<u64>,
    pub revoked: Option<Vec<u64>>,
    pub issued: Option<String>,
    pub base: Option<Vec<(Platform, BaseForm)>>,
}

fn platforms_of<'a>(
    f: &mut Fields<'a>,
    what: &'a str,
) -> Result<Vec<(Platform, Fields<'a>)>, String> {
    let mut platforms = f.object(what, what)?;
    let mut out = Vec::new();
    for platform in Platform::ALL {
        if let Some(entry) = platforms.take(platform.name()) {
            out.push((platform, entry.fields(platform.name())?));
        }
    }
    platforms.done()?;
    Ok(out)
}

impl PackageForm {
    fn read(mut f: Fields) -> Result<PackageForm, String> {
        let package = PackageForm {
            file: f.str("file")?.to_owned(),
            size: f.int("size")?,
            sha256: f.str("sha256")?.to_owned(),
            build: f.str("build")?.to_owned(),
        };
        f.done()?;
        Ok(package)
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

impl EntryForm {
    fn read(mut f: Fields) -> Result<EntryForm, String> {
        let entry = EntryForm {
            min: f.str("min")?.to_owned(),
            latest: f.str("latest")?.to_owned(),
            grace_until: f.opt_str("grace_until")?.map(str::to_owned),
            grace_min: f.opt_str("grace_min")?.map(str::to_owned),
            os_min: f.opt_str("os_min")?.map(str::to_owned),
            notes_ko: f.opt_str("notes_ko")?.map(str::to_owned),
            source: f.opt_str("source")?.map(str::to_owned),
            package: f
                .opt_object("package", "package")?
                .map(PackageForm::read)
                .transpose()?,
            auto_install_after: f.opt_str("auto_install_after")?.map(str::to_owned),
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

    fn to_text(&self) -> String {
        let mut w = Writer::new();
        w.str("min", &self.min)
            .str("latest", &self.latest)
            .opt_str("grace_until", self.grace_until.as_deref())
            .opt_str("grace_min", self.grace_min.as_deref())
            .opt_str("os_min", self.os_min.as_deref())
            .opt_str("notes_ko", self.notes_ko.as_deref())
            .opt_str("source", self.source.as_deref());
        if let Some(package) = &self.package {
            w.raw("package", &package.to_text());
        }
        w.opt_str("auto_install_after", self.auto_install_after.as_deref());
        if self.move_to_play {
            w.bool("move_to_play", true);
        }
        if let Some(code) = self.version_code {
            w.int("version_code", code);
        }
        w.opt_str("testflight_build", self.testflight_build.as_deref());
        w.finish()
    }

    /// C3 for one entry, in the app's order (trust rules 2-2).
    fn judge(&self) -> Result<Entry, Reason> {
        let min = Version::parse(&self.min).map_err(|_| Reason::Field("min"))?;
        let latest = Version::parse(&self.latest).map_err(|_| Reason::Field("latest"))?;
        if min > latest {
            return Err(Reason::Field("min"));
        }
        let os_ok = self.os_min.as_ref().is_none_or(|os| {
            (1..=16).contains(&os.len()) && os.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        });
        if !os_ok {
            return Err(Reason::Field("os_min"));
        }
        if self
            .notes_ko
            .as_ref()
            .is_some_and(|n| n.chars().count() > MAX_NOTES_CHARS)
        {
            return Err(Reason::Field("notes_ko"));
        }
        if !self.source.as_deref().is_none_or(plain_name) {
            return Err(Reason::Field("source"));
        }
        let grace_until = self
            .grace_until
            .as_deref()
            .map(|t| time::parse_utc(t).map_err(|_| Reason::Field("grace_until")))
            .transpose()?;
        let grace_min = self
            .grace_min
            .as_deref()
            .map(|v| Version::parse(v).map_err(|_| Reason::Field("grace_min")))
            .transpose()?;
        let pair = Requirement {
            min,
            latest,
            grace_until,
            grace_min,
        };
        if pair.broken().is_some() {
            return Err(Reason::Field("grace_min"));
        }
        let package = match &self.package {
            Some(p) => {
                let lower = p.file.to_ascii_lowercase();
                if !plain_name(&p.file) || lower.contains("-qs-") || lower.ends_with("install.exe")
                {
                    return Err(Reason::Field("package.file"));
                }
                if p.size == 0 || p.size > MAX_PACKAGE_BYTES {
                    return Err(Reason::Field("package.size"));
                }
                if check_sha256(&p.sha256).is_err() {
                    return Err(Reason::Field("package.sha256"));
                }
                Some(Package {
                    file: p.file.clone(),
                    size: p.size,
                    sha256: p.sha256.clone(),
                    build: p.build.clone(),
                })
            }
            None => None,
        };
        let auto_install_after = self
            .auto_install_after
            .as_deref()
            .map(|t| time::parse_utc(t).map_err(|_| Reason::Field("auto_install_after")))
            .transpose()?;
        Ok(Entry {
            min,
            latest,
            grace_until,
            grace_min,
            os_min: self.os_min.clone(),
            notes_ko: self.notes_ko.clone(),
            source: self.source.clone(),
            package,
            auto_install_after,
            move_to_play: self.move_to_play,
            version_code: self.version_code,
            testflight_build: self.testflight_build.clone(),
        })
    }
}

/// `[A-Za-z0-9._-]{1,100}`: what the apps take as a source tag and a package name.
fn plain_name(name: &str) -> bool {
    (1..=100).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

impl PolicyForm {
    /// The form of a policy text (any member order: [`PolicyForm::canonical`] says whether it is
    /// the tool's). An error here is a text the tool does not read.
    pub fn read(text: &str) -> Result<PolicyForm, String> {
        let value = Json::parse(text).map_err(|e| format!("정책이 JSON이 아닙니다({e})"))?;
        let mut f = value.fields("정책")?;
        let v = f.int("v")?;
        let epoch = f.int("epoch")?;
        let seq = f.int("seq")?;
        let issued = f.str("issued")?.to_owned();
        let server = f.str("server")?.to_owned();
        let contact_ko = f.opt_str("contact_ko")?.map(str::to_owned);
        let platforms = platforms_of(&mut f, "platforms")?
            .into_iter()
            .map(|(p, e)| {
                EntryForm::read(e)
                    .map(|e| (p, e))
                    .map_err(|err| format!("{p}: {err}"))
            })
            .collect::<Result<Vec<_>, String>>()?;
        f.done()?;
        Ok(PolicyForm {
            v,
            epoch,
            seq,
            issued,
            server,
            contact_ko,
            platforms,
        })
    }

    pub fn to_text(&self) -> String {
        let mut platforms = Writer::new();
        for (platform, entry) in &self.platforms {
            platforms.raw(platform.name(), &entry.to_text());
        }
        let mut w = Writer::new();
        w.int("v", self.v)
            .int("epoch", self.epoch)
            .int("seq", self.seq)
            .str("issued", &self.issued)
            .str("server", &self.server)
            .opt_str("contact_ko", self.contact_ko.as_deref())
            .raw("platforms", &platforms.finish());
        w.finish()
    }

    /// The form of a canonical text, or why the text is not one.
    pub fn canonical(text: &str) -> Result<PolicyForm, String> {
        let form = PolicyForm::read(text)?;
        if form.to_text() != text {
            return Err(
                "정책 본문이 이 도구가 쓰는 형식(필드 순서, 공백 없음)과 다릅니다".to_owned(),
            );
        }
        Ok(form)
    }

    /// The app's reason for people: which entry, and for the versions what they say.
    pub fn explain(&self, reason: Reason) -> String {
        for (platform, e) in &self.platforms {
            if e.judge().err() != Some(reason) {
                continue;
            }
            let detail = match (reason, Version::parse(&e.min), Version::parse(&e.latest)) {
                (Reason::Field("min"), Ok(min), Ok(latest)) if min > latest => {
                    format!("min({min})이 latest({latest})보다 높습니다")
                }
                (Reason::Field("grace_min"), _, _) => {
                    "grace_until과 grace_min은 함께 있거나 함께 없어야 하고, grace_min은 min보다 낮아야 합니다"
                        .to_owned()
                }
                (Reason::Field(member), _, _) => format!("{member} 값이 규칙에 맞지 않습니다"),
                _ => continue,
            };
            return format!("정책: {platform}: {detail}");
        }
        format!("정책: {}", reason.korean())
    }

    /// The app's step C3 on this text (trust rules 2-2): `v`, then the members in the app's
    /// order, then the server when one is given.
    pub fn judge(&self, server: Option<&str>) -> Result<Policy, Reason> {
        if self.v != POLICY_VERSION {
            return Err(Reason::Version);
        }
        if self.epoch == 0 {
            return Err(Reason::Field("epoch"));
        }
        if self.seq == 0 {
            return Err(Reason::Field("seq"));
        }
        let issued = time::parse_utc(&self.issued).map_err(|_| Reason::Field("issued"))?;
        if self
            .contact_ko
            .as_ref()
            .is_some_and(|c| c.chars().count() > MAX_CONTACT_CHARS)
        {
            return Err(Reason::Field("contact_ko"));
        }
        let mut platforms = BTreeMap::new();
        for (platform, entry) in &self.platforms {
            platforms.insert(*platform, entry.judge()?);
        }
        if server.is_some_and(|s| s != self.server) {
            return Err(Reason::Server);
        }
        Ok(Policy {
            epoch: self.epoch,
            seq: self.seq,
            issued,
            server: self.server.clone(),
            contact_ko: self.contact_ko.clone(),
            platforms,
        })
    }
}

impl BaseForm {
    fn read(mut f: Fields) -> Result<BaseForm, String> {
        let base = BaseForm {
            min: f.opt_str("min")?.map(str::to_owned),
            latest: f.opt_str("latest")?.map(str::to_owned),
            grace_until: f.opt_str("grace_until")?.map(str::to_owned),
            grace_min: f.opt_str("grace_min")?.map(str::to_owned),
        };
        f.done()?;
        Ok(base)
    }

    fn to_text(&self) -> String {
        let mut w = Writer::new();
        w.opt_str("min", self.min.as_deref())
            .opt_str("latest", self.latest.as_deref())
            .opt_str("grace_until", self.grace_until.as_deref())
            .opt_str("grace_min", self.grace_min.as_deref());
        w.finish()
    }

    fn judge(&self) -> Option<Requirement> {
        let version = |v: &Option<String>| v.as_deref().map(Version::parse);
        let requirement = Requirement {
            min: version(&self.min)?.ok()?,
            latest: version(&self.latest)?.ok()?,
            grace_until: match self.grace_until.as_deref().map(time::parse_utc) {
                None => None,
                Some(t) => Some(t.ok()?),
            },
            grace_min: match version(&self.grace_min) {
                None => None,
                Some(v) => Some(v.ok()?),
            },
        };
        requirement.broken().is_none().then_some(requirement)
    }
}

impl DeclForm {
    /// The form of a declaration text (any member order).
    pub fn read(text: &str) -> Result<DeclForm, String> {
        let value = Json::parse(text).map_err(|e| format!("루트 선언이 JSON이 아닙니다({e})"))?;
        let mut f = value.fields("루트 선언")?;
        let v = f.opt_int("v")?;
        let server = f.opt_str("server")?.map(str::to_owned);
        let epoch = f.opt_int("epoch")?;
        let revoked = match f.take("revoked") {
            None => None,
            Some(Json::Array(items)) => Some(
                items
                    .iter()
                    .map(|item| match item {
                        Json::Int(n) => Ok(*n),
                        _ => Err("루트 선언: revoked는 정수 목록입니다".to_owned()),
                    })
                    .collect::<Result<Vec<u64>, String>>()?,
            ),
            Some(_) => return Err("루트 선언: revoked는 목록이어야 합니다".to_owned()),
        };
        let issued = f.opt_str("issued")?.map(str::to_owned);
        let base = match f.take("base") {
            None => None,
            Some(value) => {
                let mut entries = value.fields("base")?;
                let mut out = Vec::new();
                for platform in Platform::ALL {
                    if let Some(entry) = entries.take(platform.name()) {
                        let entry = BaseForm::read(entry.fields(platform.name())?)
                            .map_err(|e| format!("base의 {platform}: {e}"))?;
                        out.push((platform, entry));
                    }
                }
                entries.done()?;
                Some(out)
            }
        };
        f.done()?;
        Ok(DeclForm {
            v,
            server,
            epoch,
            revoked,
            issued,
            base,
        })
    }

    pub fn to_text(&self) -> String {
        let mut w = Writer::new();
        if let Some(v) = self.v {
            w.int("v", v);
        }
        w.opt_str("server", self.server.as_deref());
        if let Some(epoch) = self.epoch {
            w.int("epoch", epoch);
        }
        if let Some(revoked) = &self.revoked {
            let items: Vec<String> = revoked.iter().map(u64::to_string).collect();
            w.raw("revoked", &format!("[{}]", items.join(",")));
        }
        w.opt_str("issued", self.issued.as_deref());
        if let Some(base) = &self.base {
            let mut b = Writer::new();
            for (platform, entry) in base {
                b.raw(platform.name(), &entry.to_text());
            }
            w.raw("base", &b.finish());
        }
        w.finish()
    }

    /// The form of a canonical text, or why the text is not one.
    pub fn canonical(text: &str) -> Result<DeclForm, String> {
        let form = DeclForm::read(text)?;
        if form.to_text() != text {
            return Err(
                "루트 선언이 이 도구가 쓰는 형식(필드 순서, 공백 없음)과 다릅니다".to_owned(),
            );
        }
        Ok(form)
    }

    /// The app's step B3 on this text, without the pinned root keys (trust rules 2-2): `v`, every
    /// rule of a declaration (RootFormat), then the server when one is given (RootServer).
    pub fn judge(&self, server: Option<&str>) -> Result<RootDecl, Reason> {
        match self.v {
            None => return Err(Reason::RootFormat),
            Some(POLICY_VERSION) => {}
            Some(_) => return Err(Reason::Version),
        }
        let format = || Reason::RootFormat;
        let text_server = self.server.clone().ok_or_else(format)?;
        let epoch = self.epoch.filter(|e| *e >= 2).ok_or_else(format)?;
        let revoked = self
            .revoked
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|n| u32::try_from(n).ok().filter(|n| (1..=65535).contains(n)))
            .collect::<Option<Vec<u32>>>()
            .ok_or_else(format)?;
        let issued = self
            .issued
            .as_deref()
            .and_then(|t| time::parse_utc(t).ok())
            .ok_or_else(format)?;
        let mut base = BTreeMap::new();
        for (platform, entry) in self.base.as_ref().ok_or_else(format)? {
            base.insert(*platform, entry.judge().ok_or_else(format)?);
        }
        if server.is_some_and(|s| s != text_server) {
            return Err(Reason::RootServer);
        }
        Ok(RootDecl {
            server: text_server,
            epoch,
            revoked,
            issued,
            base,
        })
    }
}

// ---------------------------------------------------------------------------------------------
// The texts as the tool makes them, and the tool's own rules
// ---------------------------------------------------------------------------------------------

impl Package {
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

    fn form(&self) -> PackageForm {
        PackageForm {
            file: self.file.clone(),
            size: self.size,
            sha256: self.sha256.clone(),
            build: self.build.clone(),
        }
    }
}

impl Entry {
    /// An entry with the two versions and nothing else.
    pub fn new(min: Version, latest: Version) -> Entry {
        Entry {
            min,
            latest,
            grace_until: None,
            grace_min: None,
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

    /// The requirement part (min, latest, grace_until, grace_min).
    pub fn requirement(&self) -> Requirement {
        Requirement {
            min: self.min,
            latest: self.latest,
            grace_until: self.grace_until,
            grace_min: self.grace_min,
        }
    }

    /// The tool's rules for an entry. A Windows or Android entry may be without a package here:
    /// `policy` decides when one may (trust rules 4-2, the package rule).
    fn check(&self, platform: Platform) -> Result<(), String> {
        match self.requirement().broken() {
            Some("min") => {
                return Err(format!(
                    "min({})이 latest({})보다 높습니다",
                    self.min, self.latest
                ))
            }
            Some(_) => {
                return Err(
                    "grace_until과 grace_min은 함께 있거나 함께 없어야 하고, grace_min은 min보다 낮아야 합니다"
                        .to_owned(),
                )
            }
            None => {}
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
            (None, true) | (None, false) => {}
            (Some(_), false) => return Err("package를 둘 수 없는 항목입니다".to_owned()),
        }
        if self.auto_install_after.is_some() && platform != Platform::Windows {
            return Err("auto_install_after는 windows에만 둡니다".to_owned());
        }
        if self.auto_install_after.is_some() && self.package.is_none() {
            return Err("패키지가 없는 항목에 무인 설치 시각을 둘 수 없습니다".to_owned());
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

    fn form(&self) -> EntryForm {
        EntryForm {
            min: self.min.to_string(),
            latest: self.latest.to_string(),
            grace_until: self.grace_until.map(time::format_utc),
            grace_min: self.grace_min.map(|v| v.to_string()),
            os_min: self.os_min.clone(),
            notes_ko: self.notes_ko.clone(),
            source: self.source.clone(),
            package: self.package.as_ref().map(Package::form),
            auto_install_after: self.auto_install_after.map(time::format_utc),
            move_to_play: self.move_to_play,
            version_code: self.version_code,
            testflight_build: self.testflight_build.clone(),
        }
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
    /// Reads a policy and checks every rule, the app's and the tool's; the members may come in
    /// any order (test-sign reads hand-written texts with it).
    pub fn read(text: &str) -> Result<Policy, String> {
        let form = PolicyForm::read(text)?;
        let policy = form.judge(None).map_err(|reason| form.explain(reason))?;
        policy.check()?;
        Ok(policy)
    }

    /// Reads a policy that must be exactly what [`Policy::to_text`] writes.
    pub fn parse(text: &str) -> Result<Policy, String> {
        PolicyForm::canonical(text)?;
        Policy::read(text)
    }

    /// The tool's rules for a policy by itself (the app's are in [`PolicyForm::judge`]).
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

    pub fn form(&self) -> PolicyForm {
        PolicyForm {
            v: POLICY_VERSION,
            epoch: self.epoch,
            seq: self.seq,
            issued: time::format_utc(self.issued),
            server: self.server.clone(),
            contact_ko: self.contact_ko.clone(),
            platforms: self.platforms.iter().map(|(p, e)| (*p, e.form())).collect(),
        }
    }

    /// The policy text: members in the order of 1-3, no spaces, Korean text as it is.
    pub fn to_text(&self) -> String {
        self.form().to_text()
    }

    /// The requirement part of every entry: what a new declaration's base is (4-2).
    pub fn requirements(&self) -> BTreeMap<Platform, Requirement> {
        self.platforms
            .iter()
            .map(|(p, e)| (*p, e.requirement()))
            .collect()
    }

    /// The lowest `min` of all entries: the one threshold hbbs can enforce, since a punch hole
    /// request names a version and no platform.
    pub fn lowest_min(&self) -> Option<Version> {
        self.platforms.values().map(|e| e.min).min()
    }
}

impl RootDecl {
    /// Reads a declaration and checks every rule, the app's and the tool's; any member order.
    pub fn read(text: &str) -> Result<RootDecl, String> {
        let decl = DeclForm::read(text)?
            .judge(None)
            .map_err(|reason| format!("루트 선언: {}", reason.korean()))?;
        decl.check()?;
        Ok(decl)
    }

    /// Reads a declaration that must be exactly what [`RootDecl::to_text`] writes.
    pub fn parse(text: &str) -> Result<RootDecl, String> {
        DeclForm::canonical(text)?;
        RootDecl::read(text)
    }

    /// The tool's rules for a declaration by itself (trust rules 1-3, X2): numbers 1-100 in
    /// `revoked`, ascending and once each; a base with at least one entry.
    pub fn check(&self) -> Result<(), String> {
        check_counters(self.epoch, None)?;
        if self.epoch < 2 {
            return Err("루트 선언의 세대는 2 이상입니다(세대 1에는 선언이 없습니다)".to_owned());
        }
        check_server(&self.server)?;
        if self.revoked.len() > MAX_REVOKED {
            return Err(format!("루트 선언: 폐기 목록은 {MAX_REVOKED}개까지입니다"));
        }
        if self.revoked.windows(2).any(|w| w[0] >= w[1]) {
            return Err("루트 선언: revoked는 오름차순으로, 번호마다 한 번만 씁니다".to_owned());
        }
        if let Some(n) = self
            .revoked
            .iter()
            .find(|n| !crate::keys::SIGNING_IDS.contains(n))
        {
            return Err(format!(
                "루트 선언: 서명 키 번호(1-100)만 폐기합니다. {n}은 루트 키의 번호이거나 범위 밖입니다(루트 키는 폐기하지 않습니다)"
            ));
        }
        if self.base.is_empty() {
            return Err("루트 선언: base에 항목이 없습니다".to_owned());
        }
        for (platform, entry) in &self.base {
            if let Some(member) = entry.broken() {
                return Err(format!(
                    "루트 선언: base의 {platform}: {member} 규칙에 어긋납니다"
                ));
            }
        }
        Ok(())
    }

    pub fn form(&self) -> DeclForm {
        DeclForm {
            v: Some(POLICY_VERSION),
            server: Some(self.server.clone()),
            epoch: Some(self.epoch),
            revoked: Some(self.revoked.iter().map(|n| u64::from(*n)).collect()),
            issued: Some(time::format_utc(self.issued)),
            base: Some(
                self.base
                    .iter()
                    .map(|(p, r)| {
                        (
                            *p,
                            BaseForm {
                                min: Some(r.min.to_string()),
                                latest: Some(r.latest.to_string()),
                                grace_until: r.grace_until.map(time::format_utc),
                                grace_min: r.grace_min.map(|v| v.to_string()),
                            },
                        )
                    })
                    .collect(),
            ),
        }
    }

    /// The declaration text: `v, server, epoch, revoked, issued, base`.
    pub fn to_text(&self) -> String {
        self.form().to_text()
    }
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
                grace_min: Some(v("1.5.0")),
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
                r#""windows":{"min":"1.6.0","latest":"1.6.1","grace_until":"2026-11-22T15:00:00Z","grace_min":"1.5.0","os_min":"10","#,
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
        // the pair: both or neither, grace_min below min
        assert!(change(|p| windows(p).grace_min = None).is_err());
        assert!(change(|p| windows(p).grace_until = None).is_err());
        assert!(change(|p| windows(p).grace_min = Some(v("1.6.0"))).is_err());
        assert!(change(|p| windows(p).grace_min = Some(v("0.0.0"))).is_ok());
        assert!(change(|p| {
            windows(p).grace_until = None;
            windows(p).grace_min = None;
        })
        .is_ok());
        // packages: an entry may be without one (withdrawn: `policy` decides when), but then
        // without an unattended install date
        assert!(change(|p| windows(p).package = None).is_err());
        assert!(change(|p| {
            windows(p).package = None;
            windows(p).auto_install_after = None;
        })
        .is_ok());
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
        // `read` takes the fields in any order and with spaces (test-sign reads hand-written
        // texts with it), and is as strict about what the fields are: another version of the
        // format, an entry the format does not have
        let spaced = good.replacen(r#"{"v":1,"#, r#"{ "v":1,"#, 1);
        assert_eq!(Policy::read(&spaced).unwrap(), sample());
        let err = Policy::read(&spaced.replacen(r#""v":1,"epoch""#, r#""v":2,"epoch""#, 1));
        assert!(err.unwrap_err().contains("버전"));
        let err = Policy::read(&spaced.replacen(
            r#""ios":{"#,
            r#""linux":{"min":"1.6.0","latest":"1.6.1"},"ios":{"#,
            1,
        ));
        assert!(err.unwrap_err().contains("모르는 필드 \"linux\""));
        let decl = r#"{ "v":2,"server":"rd.example.com","epoch":2,"revoked":[],"issued":"2026-11-08T03:00:00Z","base":{"ios":{"min":"1.6.0","latest":"1.6.0"}}}"#;
        assert!(RootDecl::read(decl).unwrap_err().contains("버전"));
        assert!(RootDecl::read(&decl.replacen("\"v\":2", "\"v\":1", 1)).is_ok());
    }

    fn base() -> BTreeMap<Platform, Requirement> {
        sample().requirements()
    }

    fn decl() -> RootDecl {
        RootDecl {
            server: "rd.example.com".to_owned(),
            epoch: 2,
            revoked: vec![1, 3],
            issued: t("2026-11-08T03:00:00Z"),
            base: base(),
        }
    }

    #[test]
    fn root_declarations() {
        let decl = decl();
        let text = decl.to_text();
        assert_eq!(
            text,
            concat!(
                r#"{"v":1,"server":"rd.example.com","epoch":2,"revoked":[1,3],"issued":"2026-11-08T03:00:00Z","#,
                r#""base":{"windows":{"min":"1.6.0","latest":"1.6.1","grace_until":"2026-11-22T15:00:00Z","grace_min":"1.5.0"},"#,
                r#""android":{"min":"1.6.0","latest":"1.6.1"},"android_play":{"min":"1.6.0","latest":"1.6.1"},"#,
                r#""ios":{"min":"1.6.0","latest":"1.6.1"}}}"#
            )
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
            (r#""epoch":2"#, r#""epoch":1"#),
            (r#""revoked":[1,3]"#, r#""revoked":[3,1]"#),
            (r#""revoked":[1,3]"#, r#""revoked":[1,1]"#),
            (r#""revoked":[1,3]"#, r#""revoked":[0]"#),
            (r#""revoked":[1,3]"#, r#""revoked":[65536]"#),
            (r#""revoked":[1,3]"#, r#""revoked":[1,101]"#),
            (r#""revoked":[1,3]"#, r#""revoked":[1,103]"#),
            (r#""revoked":[1,3]"#, r#""revoked":["1"]"#),
            (r#""revoked":[1,3]"#, r#""revoked":1"#),
            (r#""revoked":[1,3]"#, r#""revoked":null"#),
            (r#""revoked":[1,3]"#, r#""revoked":[1, 3]"#),
            (r#""server":"rd.example.com""#, r#""server":"""#),
            (r#""epoch":2,"#, r#""epoch":2,"seq":1,"#),
            (r#","base":{"#, r#","base":null,"x":{"#),
            (
                r#""base":{"#,
                r#""base":{"linux":{"min":"1.6.0","latest":"1.6.0"},"#,
            ),
            (r#""ios":{"min":"1.6.0","#, r#""ios":{"min":"1.6","#),
            (r#""ios":{"min":"1.6.0","#, r#""ios":{"min":"1.6.2","#),
            (
                r#""ios":{"min":"1.6.0","#,
                r#""ios":{"os_min":"13.0","min":"1.6.0","#,
            ),
            (r#""grace_min":"1.5.0""#, r#""grace_min":"1.6.0""#),
            (r#","grace_min":"1.5.0""#, ""),
        ] {
            assert!(text.contains(from), "{from}");
            let bad = text.replacen(from, to, 1);
            assert!(RootDecl::parse(&bad).is_err(), "{to} parsed");
        }
        // a missing member is the app's RootFormat, not a matter of form
        let without = text.replacen(r#","revoked":[1,3]"#, "", 1);
        assert!(DeclForm::canonical(&without).is_ok());
        assert_eq!(
            DeclForm::read(&without)
                .unwrap()
                .judge(None)
                .unwrap()
                .revoked,
            Vec::<u32>::new()
        );
        let no_base = text.split(r#","base""#).next().unwrap().to_owned() + "}";
        assert!(DeclForm::canonical(&no_base).is_ok());
        assert_eq!(
            DeclForm::read(&no_base).unwrap().judge(None),
            Err(Reason::RootFormat)
        );
        // the app's codes: another version, a base entry that breaks a rule, another server
        let judge = |t: &str| DeclForm::read(t).unwrap().judge(Some("rd.example.com"));
        assert_eq!(
            judge(&text.replacen(r#""v":1"#, r#""v":2"#, 1)),
            Err(Reason::Version)
        );
        assert_eq!(
            judge(&text.replacen(r#""epoch":2"#, r#""epoch":1"#, 1)),
            Err(Reason::RootFormat)
        );
        // a key number outside 1-65535 in revoked breaks a rule of a declaration (the app's B3)
        for revoked in [
            r#""revoked":[0]"#,
            r#""revoked":[65536]"#,
            r#""revoked":[1,0]"#,
        ] {
            let bad = text.replacen(r#""revoked":[1,3]"#, revoked, 1);
            assert!(DeclForm::canonical(&bad).is_ok(), "{revoked}");
            assert_eq!(judge(&bad), Err(Reason::RootFormat), "{revoked}");
        }
        assert!(judge(&text.replacen(r#""revoked":[1,3]"#, r#""revoked":[65535]"#, 1)).is_ok());
        assert_eq!(
            judge(&text.replacen(r#""grace_min":"1.5.0""#, r#""grace_min":"1.6.0""#, 1)),
            Err(Reason::RootFormat)
        );
        assert_eq!(
            judge(&text.replacen("rd.example.com", "other.example.com", 1)),
            Err(Reason::RootServer)
        );
        // the tool's rule beyond the app's: a root key's number is never revoked (X2)
        let with_103 = text.replacen(r#""revoked":[1,3]"#, r#""revoked":[1,103]"#, 1);
        assert!(judge(&with_103).is_ok());
        assert!(RootDecl::parse(&with_103).unwrap_err().contains("루트 키"));
    }

    #[test]
    fn the_app_codes_of_a_policy() {
        let judge = |t: &str| {
            PolicyForm::canonical(t)
                .unwrap()
                .judge(Some("rd.example.com"))
        };
        let good = sample().to_text();
        assert_eq!(judge(&good), Ok(sample()));
        for (from, to, reason) in [
            (r#""v":1,"epoch""#, r#""v":2,"epoch""#, Reason::Version),
            (r#""epoch":1"#, r#""epoch":0"#, Reason::Field("epoch")),
            (r#""seq":7"#, r#""seq":0"#, Reason::Field("seq")),
            (
                r#""issued":"2026-11-08T03:00:00Z""#,
                r#""issued":"2026-11-08 03:00""#,
                Reason::Field("issued"),
            ),
            (
                r#""min":"1.6.0","latest":"1.6.1","grace"#,
                r#""min":"1.6","latest":"1.6.1","grace"#,
                Reason::Field("min"),
            ),
            (
                r#""latest":"1.6.1","grace"#,
                r#""latest":"1.6.1.0","grace"#,
                Reason::Field("latest"),
            ),
            (
                r#""min":"1.6.0","latest":"1.6.1","grace"#,
                r#""min":"1.6.2","latest":"1.6.1","grace"#,
                Reason::Field("min"),
            ),
            (
                r#""os_min":"10""#,
                r#""os_min":"ten""#,
                Reason::Field("os_min"),
            ),
            (
                r#""source":"client-1.6.1""#,
                r#""source":"client 1.6.1""#,
                Reason::Field("source"),
            ),
            (
                r#""grace_until":"2026-11-22T15:00:00Z""#,
                r#""grace_until":"2026-11-22""#,
                Reason::Field("grace_until"),
            ),
            (
                r#""grace_min":"1.5.0""#,
                r#""grace_min":"1.6.0""#,
                Reason::Field("grace_min"),
            ),
            (
                r#""grace_min":"1.5.0""#,
                r#""grace_min":"1.5""#,
                Reason::Field("grace_min"),
            ),
            (r#","grace_min":"1.5.0""#, "", Reason::Field("grace_min")),
            (
                r#""file":"HJViewer-1.6.1-windows-x86_64.exe""#,
                r#""file":"HJViewer-1.6.1-install.exe""#,
                Reason::Field("package.file"),
            ),
            (
                r#""size":26214400"#,
                r#""size":0"#,
                Reason::Field("package.size"),
            ),
            (
                r#""sha256":"e6a3"#,
                r#""sha256":"E6A3"#,
                Reason::Field("package.sha256"),
            ),
            (
                r#""auto_install_after":"2026-11-10T03:00:00Z""#,
                r#""auto_install_after":"soon""#,
                Reason::Field("auto_install_after"),
            ),
            (
                r#""server":"rd.example.com""#,
                r#""server":"other.example.com""#,
                Reason::Server,
            ),
        ] {
            assert!(good.contains(from), "{from}");
            assert_eq!(judge(&good.replacen(from, to, 1)), Err(reason), "{to}");
        }
        // the app's order: the first member that breaks a rule names the code
        let both = good
            .replacen(r#""issued":"2026-11-08T03:00:00Z""#, r#""issued":"x""#, 1)
            .replacen(
                r#""server":"rd.example.com""#,
                r#""server":"x.example.com""#,
                1,
            );
        assert_eq!(judge(&both), Err(Reason::Field("issued")));
        // ... and the server is judged after every entry
        let entry_and_server = good
            .replacen(
                r#""min":"1.6.0","latest":"1.6.1","grace"#,
                r#""min":"1.6","latest":"1.6.1","grace"#,
                1,
            )
            .replacen(
                r#""server":"rd.example.com""#,
                r#""server":"x.example.com""#,
                1,
            );
        assert_eq!(judge(&entry_and_server), Err(Reason::Field("min")));
        // what only the tool refuses passes the app's rules
        let url = Policy {
            contact_ko: Some("https://rd.example.com 에서 받으세요".to_owned()),
            ..sample()
        };
        assert!(judge(&url.to_text()).is_ok());
        assert!(url.check().is_err());
    }

    #[test]
    fn requirements() {
        let r = |min: &str, until: Option<&str>, grace_min: Option<&str>| Requirement {
            min: v(min),
            latest: v("1.6.1"),
            grace_until: until.map(t),
            grace_min: grace_min.map(v),
        };
        let raise = r("1.6.1", Some("2026-11-22T15:00:00Z"), Some("1.6.0"));
        let before = t("2026-11-22T14:59:59Z");
        let at = t("2026-11-22T15:00:00Z");
        assert_eq!(raise.threshold(before), v("1.6.0").number());
        assert_eq!(raise.threshold(at), v("1.6.1").number());
        assert_eq!(raise.deadline(), Some(at));
        let plain = r("1.6.0", None, None);
        assert_eq!(
            (plain.lo(), plain.hi(), plain.deadline()),
            (v("1.6.0").number(), v("1.6.0").number(), None)
        );
        assert_eq!(plain.threshold(0), v("1.6.0").number());
        assert_eq!(r("1.6.2", None, None).broken(), Some("min"));
        assert_eq!(
            r("1.6.0", Some("2026-11-22T15:00:00Z"), None).broken(),
            Some("grace_min")
        );
        assert_eq!(r("1.6.0", None, Some("1.5.0")).broken(), Some("grace_min"));
        assert_eq!(
            r("1.6.0", Some("2026-11-22T15:00:00Z"), Some("1.6.0")).broken(),
            Some("grace_min")
        );
        assert_eq!(
            r("1.6.0", Some("2026-11-22T15:00:00Z"), Some("0.0.0")).broken(),
            None
        );
    }
}

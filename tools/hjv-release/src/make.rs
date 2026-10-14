// SPDX-License-Identifier: AGPL-3.0-only

//! `hjv-release policy`: makes the next policy from the previous one, the builds of a release and
//! what the operator asks for (trust rules 4-2 to 4-5), and refuses what could not be undone:
//!
//! - the position: one above every position of the epoch the archive holds (normally the
//!   previous `seq` + 1); a new declaration opens the epoch above every epoch the archive holds;
//! - the requirements of an entry: a raise keeps the level in force until its deadline
//!   (`grace_min` = the minimum in force, derivations a-f); only a declaration asks less (X1);
//! - a new Windows or Android `latest` needs its package and the provenance of its build; the
//!   same version never comes with another hash, and a withdrawn package never comes back under
//!   the same version (the package rule, `--withdraw-package`);
//! - an iOS `latest` or a higher iOS `min` needs a TestFlight build the operator confirmed and
//!   that is younger than 80 days; a Google Play `latest` or a higher `min` needs the versionCode
//!   the operator saw released on the internal track;
//! - a policy that raises `os_min` keeps `min` at or below the last version the old OS runs;
//! - `--relax`, `--drop`, `--revoke-key` and `--new-epoch` make a new declaration (a root key)
//!   whose base is the new policy's requirement part, and only from the live file, which must be
//!   `--prev` and a file of the archive (X10);
//! - the signer: a signing key that every recorded key set pins (X12), or a root key when none
//!   does (`--root-signs-policy`, X9).
//!
//! The result is a draft (draft.rs); nothing is signed here and no key file is read. Every
//! envelope it describes is checked by the apps' rules (trust.rs) for a device holding `--prev`
//! and for a fresh install (X4) before the draft is written.

use crate::{
    archive::Archive,
    draft::Draft,
    envelope::{Envelope, Part, MAX_ENVELOPE_BYTES},
    keys::{KeySet, Role},
    policy::{
        check_os_min, check_server, os_min_higher, Entry, Package, Platform, Policy, Requirement,
        RootDecl, MAX_VERSION_CODE,
    },
    provenance::{self, Runs, Settings, Tools},
    release::{server_file_name, Artifact, Built},
    time::{self, DAY},
    trust::{self, check_envelope, loosenings, new_entries_below, peek_server, Checked, State},
    version::Version,
};
use std::collections::BTreeMap;

/// What `policy` says of a draft without the Google Play entry.
pub const NO_PLAY_ENTRY: &str =
    "android_play 항목이 없습니다: Google Play로 설치한 앱에는 이 정책의 \
     최소 버전이 걸리지 않습니다. Play 내부 트랙에 출시(completed)한 뒤 --play-live로 넣으세요";

/// A TestFlight build stops working 90 days after its upload; a policy may name one younger than
/// this.
pub const TESTFLIGHT_MAX_DAYS: i64 = 80;
/// `check` warns from this age on.
pub const TESTFLIGHT_WARN_DAYS: i64 = 75;
/// When installed Windows PCs start installing a new `latest` unattended, counted from the
/// policy's issue, when the operator names no time.
pub const DEFAULT_AUTO_INSTALL_DAYS: i64 = 2;
/// The grace period of a raised `min` when the operator names none.
pub const DEFAULT_GRACE_DAYS: i64 = 7;
/// A jump in `seq` larger than this needs the signer's typed consent.
pub const SEQ_JUMP_TO_CONFIRM: u64 = 10;
/// `--seq` may be at most this far above the previous policy (the apps' C5).
pub const MAX_SEQ_AHEAD: u64 = trust::MAX_SEQ_STEP;

/// What an option applies to: one entry, or every entry it can apply to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    All,
    One(Platform),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grace {
    /// Until 00:00 Korean time that many days from today; 0 means no grace period.
    Days(i64),
    Until(i64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoAfter {
    /// That many seconds after the policy is issued.
    In(i64),
    At(i64),
    /// No unattended installs.
    Off,
}

/// What the operator asked about an entry's grace period (after `all=` is applied).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraceAsk {
    /// No option: the derivation's default.
    Default,
    /// `--grace-days 0`: no pair, the minimum at once.
    None,
    /// A deadline.
    At(i64),
}

/// What `hjv-release policy` was asked for.
#[derive(Clone, Debug, Default)]
pub struct Request {
    /// The previous envelope file, or none for the first policy.
    pub prev: Option<String>,
    /// The live envelope as fetched from the server (`--live`), required for a new declaration.
    pub live: Option<String>,
    pub server: Option<String>,
    pub artifacts: Vec<Artifact>,
    pub min: Vec<(Target, Version)>,
    pub latest: Vec<(Platform, Version)>,
    /// Entries to keep exactly as they are.
    pub hold: Vec<Platform>,
    /// Entries to remove (a relaxation).
    pub drop: Vec<Platform>,
    pub grace: Vec<(Target, Grace)>,
    /// `--grace-min P=V`: the grace_min of an entry that has or gets a pair.
    pub grace_min: Vec<(Platform, Version)>,
    /// `--withdraw-package P`: the entry's package goes, until a higher latest.
    pub withdraw: Vec<Platform>,
    pub auto_after: Option<AutoAfter>,
    pub os_min: Vec<(Platform, String)>,
    /// An empty text removes the field.
    pub contact: Option<String>,
    pub notes: Vec<(Target, String)>,
    pub ios_valid: Option<String>,
    pub play_live: Option<u64>,
    pub move_to_play: Option<bool>,
    pub relax: bool,
    pub revoke: Vec<u32>,
    pub new_epoch: bool,
    /// A root key signs the policy: only when no signing key serves the records (X9).
    pub root_signs_policy: bool,
    pub seq: Option<u64>,
    pub now: i64,
}

impl Request {
    /// Whether this policy comes with a new declaration (4-2 Declaration).
    pub fn new_declaration(&self) -> bool {
        self.relax || !self.drop.is_empty() || !self.revoke.is_empty() || self.new_epoch
    }
}

/// The draft, and what to tell the operator.
#[derive(Clone, Debug)]
pub struct Made {
    pub draft: Draft,
    pub policy: Policy,
    pub report: Vec<String>,
}

struct Maker<'a> {
    req: &'a Request,
    /// The entries of `--prev`, or none for the first policy.
    prev: Option<&'a BTreeMap<Platform, Entry>>,
    report: Vec<String>,
    notes: Vec<String>,
    grace_for_all_used: bool,
    notes_for_all_used: bool,
    /// The runs whose artifacts were downloaded for the provenance checks.
    runs: Runs,
}

fn pick<T: Copy>(list: &[(Target, T)], platform: Platform) -> Option<(bool, T)> {
    let one = list.iter().find(|(t, _)| *t == Target::One(platform));
    let all = list.iter().find(|(t, _)| *t == Target::All);
    one.map(|(_, v)| (true, *v))
        .or(all.map(|(_, v)| (false, *v)))
}

/// The pair of an entry: `grace_until` and `grace_min`, both or neither.
type Pair = Option<(i64, Version)>;

fn pair_of(r: &Requirement) -> Pair {
    r.grace_until.zip(r.grace_min)
}

/// Derivations a-d of 4-2 for an entry `o` of `--prev` and the asked minimum `b`. The
/// derivations never relax `o`, except where `relax` lets them (a with a later or new deadline,
/// d). `grace_min` is `--grace-min` for this entry.
pub fn derive_existing(
    platform: Platform,
    o: &Requirement,
    b: Version,
    now: i64,
    grace: GraceAsk,
    relax: bool,
    grace_min: Option<Version>,
) -> Result<Pair, String> {
    let old = pair_of(o);
    if b == o.min {
        // a: the minimum stays
        let deadline = match grace {
            GraceAsk::None => return Ok(None),
            GraceAsk::Default => return Ok(old),
            GraceAsk::At(t) => t,
        };
        if let Some((until, gm)) = old {
            if deadline == until {
                return Ok(old);
            }
            if now < deadline && deadline < until {
                return Ok(Some((deadline, gm)));
            }
        }
        if !relax {
            return Err(format!(
                "{platform}: 같은 최소 버전({})의 유예 끝을 늦추거나 새로 두는 것은 완화입니다(X1). \
                 루트 키의 선언이 있어야 합니다: --relax --live <서버의 정책 파일>",
                o.min
            ));
        }
        let gm = grace_min.or(old.map(|(_, gm)| gm)).ok_or_else(|| {
            format!(
                "{platform}: 유예가 없던 항목에 유예를 주는 완화에는 --grace-min {platform}=<기다려 줄 가장 낮은 버전>이 \
                 필요합니다(유예 기간 동안 그 버전 미만은 계속 막습니다)"
            )
        })?;
        return Ok(Some((deadline, gm)));
    }
    if b > o.min {
        let pending = old.filter(|(until, _)| *until > now);
        return Ok(match (pending, grace) {
            (_, GraceAsk::None) => None,
            // b: no raise pending: until the deadline the minimum in force stays
            (None, GraceAsk::Default) => {
                Some((time::kst_midnight_after(now, DEFAULT_GRACE_DAYS), o.min))
            }
            (None, GraceAsk::At(t)) => Some((t, o.min)),
            // c: a raise is pending: the same deadline by default; a later one enforces the
            // pending minimum at once
            (Some((until, gm)), GraceAsk::Default) => Some((until, gm)),
            (Some((until, gm)), GraceAsk::At(t)) if t <= until => Some((t, gm)),
            (Some(_), GraceAsk::At(t)) => Some((t, o.min)),
        });
    }
    // d: a lowered minimum
    if !relax {
        return Err(format!(
            "{platform}: 최소 버전을 내리는 것({} → {b})은 완화입니다(X1). 루트 키의 선언이 있어야 합니다: \
             --relax --live <서버의 정책 파일>",
            o.min
        ));
    }
    if grace != GraceAsk::Default || grace_min.is_some() {
        return Err(format!(
            "{platform}: 최소 버전을 내리는 항목에는 유예 옵션을 줄 수 없습니다"
        ));
    }
    Ok(None)
}

/// Derivations e and f of 4-2 for an entry `--prev` does not have. `prev` is `--prev`'s
/// requirements, or none for the first policy. Below its sibling (a relaxation, with `relax`) an
/// Android entry gets no pair, and a grace option for it is refused, as in d.
pub fn derive_new(
    platform: Platform,
    b: Version,
    now: i64,
    grace: GraceAsk,
    prev: Option<&BTreeMap<Platform, Requirement>>,
    relax: bool,
    grace_min: Option<Version>,
) -> Result<Pair, String> {
    // f: the sibling of the Android pair is in --prev
    let sibling = platform
        .sibling()
        .and_then(|s| prev.and_then(|p| p.get(&s)).map(|x| (s, x)));
    if let Some((s, x)) = sibling {
        if b == x.min {
            // at the sibling's level: its pair as it is
            return Ok(pair_of(x));
        }
        if b > x.min {
            return derive_existing(platform, x, b, now, grace, relax, grace_min);
        }
        if !relax {
            return Err(format!(
                "{platform}: 새 항목의 최소 버전({b})이 {s}의 최소 버전({})보다 낮습니다. PC가 Android \
                 컨트롤러에 쓰는 기준이 내려가는 완화입니다(X1): --relax --live <서버의 정책 파일>",
                x.min
            ));
        }
        // below the sibling, as in d: no pair, and a grace option for it is refused
        if grace != GraceAsk::Default || grace_min.is_some() {
            return Err(format!(
                "{platform}: {s}의 최소 버전({})보다 낮은 새 항목에는 유예 옵션을 줄 수 없습니다",
                x.min
            ));
        }
        return Ok(None);
    }
    // e: a new entry: the asked grace or 7 days, grace_min 0.0.0; none in a first policy unless
    // asked for
    let zero = Version::new(0, 0, 0);
    let first = prev.is_none();
    if b == zero {
        return Ok(None);
    }
    Ok(match grace {
        GraceAsk::None => None,
        GraceAsk::Default if first => None,
        GraceAsk::Default => Some((time::kst_midnight_after(now, DEFAULT_GRACE_DAYS), zero)),
        GraceAsk::At(t) => Some((t, zero)),
    })
}

impl<'a> Maker<'a> {
    fn min_option(&self, platform: Platform) -> Option<Version> {
        pick(&self.req.min, platform).map(|(_, v)| v)
    }

    fn latest_option(&self, platform: Platform) -> Option<Version> {
        self.req
            .latest
            .iter()
            .find(|(p, _)| *p == platform)
            .map(|(_, v)| *v)
    }

    fn artifact(&self, built: Built) -> Option<&'a Artifact> {
        self.req.artifacts.iter().find(|a| a.info.platform == built)
    }

    /// Whether an option names this entry itself (`all` does not count).
    fn named(&self, platform: Platform) -> bool {
        let one = Target::One(platform);
        self.req.min.iter().any(|(t, _)| *t == one)
            || self.req.grace.iter().any(|(t, _)| *t == one)
            || self.req.notes.iter().any(|(t, _)| *t == one)
            || self.req.latest.iter().any(|(p, _)| *p == platform)
            || self.req.os_min.iter().any(|(p, _)| *p == platform)
            || self.req.grace_min.iter().any(|(p, _)| *p == platform)
            || self.req.withdraw.contains(&platform)
    }

    fn grace_ask(&mut self, platform: Platform, changed: bool) -> Result<GraceAsk, String> {
        let now = self.req.now;
        let grace = match pick(&self.req.grace, platform) {
            Some((true, grace)) => grace,
            Some((false, grace)) if changed => {
                self.grace_for_all_used = true;
                grace
            }
            _ => return Ok(GraceAsk::Default),
        };
        match grace {
            Grace::Days(0) => Ok(GraceAsk::None),
            Grace::Days(days) if (1..=365).contains(&days) => {
                Ok(GraceAsk::At(time::kst_midnight_after(now, days)))
            }
            Grace::Days(days) => Err(format!("유예 일수는 0-365입니다: {days}")),
            Grace::Until(t) if t > now => Ok(GraceAsk::At(t)),
            Grace::Until(t) => Err(format!(
                "{platform}: 유예 끝({})이 이미 지났습니다(바로 강제하려면 --grace-days 0)",
                time::format_utc(t)
            )),
        }
    }

    /// The members every entry has: `min` with its pair, `os_min` and the notes.
    fn finish(
        &mut self,
        platform: Platform,
        before: Option<&Entry>,
        mut entry: Entry,
        latest_changed: bool,
    ) -> Result<Entry, String> {
        let min = match (self.min_option(platform), before) {
            (Some(min), _) => min,
            (None, Some(before)) => before.min,
            (None, None) => {
                return Err(format!(
                    "{platform}: 새 항목에는 --min이 필요합니다(예: --min {platform}=1.5.0)"
                ))
            }
        };
        if min > entry.latest {
            return Err(format!(
                "{platform}: min({min})이 latest({})보다 높습니다. 그 버전이 준비된 뒤에 올리세요",
                entry.latest
            ));
        }
        let changed = before.is_none_or(|b| min != b.min);
        let grace = self.grace_ask(platform, changed)?;
        let at_sibling = before.is_none()
            && platform
                .sibling()
                .and_then(|s| self.prev.and_then(|p| p.get(&s)))
                .is_some_and(|x| x.min == min);
        if at_sibling
            && self
                .req
                .grace
                .iter()
                .any(|(t, _)| *t == Target::One(platform))
        {
            return Err(format!(
                "{platform}: 다른 Android 항목과 같은 최소 버전의 새 항목은 그 항목의 유예를 그대로 씁니다. \
                 유예 옵션을 주지 마세요(다른 기한은 완화이거나 새 올림입니다)"
            ));
        }
        let asked_gm = self
            .req
            .grace_min
            .iter()
            .find(|(p, _)| *p == platform)
            .map(|(_, v)| *v);
        let pair = match before {
            Some(b) => derive_existing(
                platform,
                &b.requirement(),
                min,
                self.req.now,
                grace,
                self.req.relax,
                asked_gm,
            )?,
            None => {
                let prev: Option<BTreeMap<Platform, Requirement>> = self
                    .prev
                    .map(|p| p.iter().map(|(k, e)| (*k, e.requirement())).collect());
                derive_new(
                    platform,
                    min,
                    self.req.now,
                    grace,
                    prev.as_ref(),
                    self.req.relax,
                    asked_gm,
                )?
            }
        };
        // --grace-min sets grace_min of an entry that has or gets a pair (a value that makes the
        // entry ask less than before is a relaxation, X1)
        let pair = match (pair, asked_gm) {
            (Some((until, _)), Some(gm)) => Some((until, gm)),
            (None, Some(_)) => {
                return Err(format!(
                    "{platform}: --grace-min은 유예가 있는 항목에만 줍니다(이 항목에는 유예가 없습니다)"
                ))
            }
            (pair, None) => pair,
        };
        let week = time::kst_midnight_after(self.req.now, DEFAULT_GRACE_DAYS);
        if grace == GraceAsk::Default && changed && pair.is_some_and(|(until, _)| until == week) {
            // the standing default (design 1-5), said aloud
            self.report.push(format!(
                "{platform}: 유예 옵션이 없어 기본 {DEFAULT_GRACE_DAYS}일을 둡니다({}까지). 바로 강제하려면 \
                 --grace-days 0, 다른 기간은 --grace-days N",
                time::format_kst(week)
            ));
        }
        entry.min = min;
        entry.grace_until = pair.map(|(until, _)| until);
        entry.grace_min = pair.map(|(_, gm)| gm);
        if let Some(member) = entry.requirement().broken() {
            return Err(format!(
                "{platform}: {member} 규칙에 맞지 않습니다(grace_min은 min보다 낮아야 합니다: --grace-min)"
            ));
        }
        if before.is_none_or(|b| min > b.min) && pair.is_none() && self.prev.is_some() {
            self.report.push(format!(
                "{platform}: 유예가 없습니다. 서버에 넣는 즉시 {min} 미만을 막습니다"
            ));
        }
        if let Some((until, gm)) = pair.filter(|_| before.is_none_or(|b| b.min != min)) {
            self.report.push(format!(
                "{platform}: {}까지는 {gm} 미만을, 그 뒤로는 {min} 미만을 막습니다",
                time::format_kst(until)
            ));
        }

        let req = self.req;
        let asked_os = req
            .os_min
            .iter()
            .find(|(p, _)| *p == platform)
            .map(|(_, v)| v.clone());
        entry.os_min = asked_os.or_else(|| before.and_then(|b| b.os_min.clone()));
        if let Some(os_min) = &entry.os_min {
            check_os_min(platform, os_min)?;
        }
        if let (Some(before), Some(new_os)) = (before, &entry.os_min) {
            let raised_os = before
                .os_min
                .as_ref()
                .filter(|old| os_min_higher(new_os, old));
            if let Some(old_os) = raised_os.filter(|_| min > before.latest) {
                return Err(format!(
                    "{platform}: os_min을 올리는 정책({old_os} → {new_os})에서는 min을 옛 OS에서 도는 \
                     마지막 버전 {} 이하로 둡니다(min {min}). 옛 OS 기기가 설치할 수 없는 버전을 강제하게 됩니다",
                    before.latest
                ));
            }
        }

        let kept = before.and_then(|b| b.notes_ko.clone());
        let text = |t: &str| (!t.is_empty()).then(|| t.to_owned());
        entry.notes_ko = match pick_ref(&req.notes, platform) {
            Some((true, notes)) => text(notes),
            Some((false, notes)) if latest_changed => {
                self.notes_for_all_used = true;
                text(notes)
            }
            // the old notes described the old version
            _ if latest_changed => None,
            _ => kept,
        };
        Ok(entry)
    }

    /// Windows and Android: `latest` comes with a package, and a new package with its provenance.
    fn packaged(
        &mut self,
        built: Built,
        before: Option<&Entry>,
        settings: &Settings,
        tools: &dyn Tools,
    ) -> Result<Option<(Entry, bool)>, String> {
        let platform = built.platform();
        let artifact = self.artifact(built);
        let wanted = self.latest_option(platform);
        let from_artifact = match (wanted, before, artifact) {
            (Some(v), Some(b), _) if v == b.latest => None,
            (Some(v), _, Some(a)) if a.info.version == v => Some(a),
            (Some(v), _, _) => {
                return Err(format!(
                    "{platform}: latest {v}의 패키지가 릴리스 디렉터리에 없습니다(--release-dir)"
                ))
            }
            (None, None, None) if self.named(platform) => {
                return Err(format!(
                    "{platform}: 패키지가 없어 항목을 만들 수 없습니다(--release-dir)"
                ))
            }
            (None, None, None) => return Ok(None),
            (None, Some(_), None) => None,
            (None, None, Some(a)) => Some(a),
            (None, Some(b), Some(a)) if a.info.version > b.latest => Some(a),
            (None, Some(b), Some(a)) if a.info.version == b.latest => None,
            (None, Some(b), Some(a)) => {
                return Err(format!(
                    "{platform}: 릴리스 디렉터리의 버전({})이 지금 latest({})보다 낮습니다\
                     (그대로 두려면 --hold {platform}, 내리려면 --relax --latest {platform}={})",
                    a.info.version, b.latest, a.info.version
                ))
            }
        };
        // a withdrawn package does not come back under the same version (one build per version):
        // the fix ships as a higher version (trust rules 4-2, the package rule)
        if let (Some(b), Some(a)) = (before, artifact) {
            if b.package.is_none() && a.info.version == b.latest {
                return Err(format!(
                    "{platform}: {}의 패키지는 거둬들였습니다. 같은 버전에 패키지를 다시 붙이지 않습니다\
                     (고친 빌드는 더 높은 버전으로 내세요)",
                    b.latest
                ));
            }
        }
        let withdraw = self.req.withdraw.contains(&platform);
        if withdraw
            && from_artifact.is_some_and(|a| before.is_none_or(|b| a.info.version != b.latest))
        {
            return Err(format!(
                "{platform}: --withdraw-package와 새 latest를 함께 줄 수 없습니다. 새 latest는 패키지와 함께 \
                 나갑니다"
            ));
        }
        // every build has a new version: the same version with another hash is a mistake
        if let (Some(b), Some(a)) = (before, artifact) {
            let other = b
                .package
                .as_ref()
                .is_some_and(|p| p.sha256 != a.info.sha256);
            if a.info.version == b.latest && other {
                return Err(format!(
                    "{platform}: 버전 {}의 패키지 해시가 지금 정책과 다릅니다. \
                     같은 버전을 다른 빌드로 낼 수 없습니다(버전을 올려 다시 빌드하세요)",
                    b.latest
                ));
            }
        }
        let (mut entry, latest_changed) = match (from_artifact, before) {
            (Some(a), _) => {
                let proof = provenance::check(a, settings, tools, &mut self.runs)?;
                self.notes.extend(proof.lines);
                let version = a.info.version;
                let file = server_file_name(built, version)
                    .ok_or_else(|| format!("{platform}: 패키지 이름을 정할 수 없습니다"))?;
                let mut entry = before
                    .cloned()
                    .unwrap_or_else(|| Entry::new(version, version));
                entry.latest = version;
                entry.package = Some(Package {
                    file,
                    size: a.info.size,
                    sha256: a.info.sha256.clone(),
                    build: a.info.build.clone(),
                });
                entry.source = Some(format!("client-{version}"));
                (entry, before.is_none_or(|b| b.latest != version))
            }
            (None, Some(b)) => {
                if let Some(a) = artifact.filter(|a| a.info.version != b.latest) {
                    self.report.push(format!(
                        "{platform}: 릴리스 디렉터리의 버전({})은 쓰지 않고 latest({})를 그대로 둡니다",
                        a.info.version, b.latest
                    ));
                }
                (b.clone(), false)
            }
            (None, None) => return Ok(None),
        };
        // the hint to move to Play goes with a withdrawn package as well (its fix may be Play)
        if platform == Platform::Android {
            if let Some(move_to_play) = self.req.move_to_play {
                entry.move_to_play = move_to_play;
            }
        }
        if withdraw {
            let Some(package) = entry.package.take() else {
                return Err(format!(
                    "{platform}: 거둬들일 패키지가 없습니다(이미 패키지 없는 항목입니다)"
                ));
            };
            self.report.push(format!(
                "{platform}: 패키지 {}(sha256 {})를 거둬들입니다. 앱 안 내려받기, 무인 설치, 강제 설치가 \
                 멈춥니다. 더 높은 latest가 나올 때까지 패키지 없이 둡니다",
                package.file, package.sha256
            ));
            if entry.auto_install_after.take().is_some() || self.req.auto_after.is_some() {
                self.report.push(format!(
                    "{platform}: 패키지가 없으니 무인 설치 시각도 없앱니다"
                ));
            }
            return Ok(Some((entry, latest_changed)));
        }
        if platform == Platform::Windows {
            let now = self.req.now;
            entry.auto_install_after = match self.req.auto_after {
                Some(AutoAfter::Off) => None,
                Some(AutoAfter::In(seconds)) => Some(now + seconds),
                Some(AutoAfter::At(t)) if t >= now => Some(t),
                Some(AutoAfter::At(t)) => {
                    return Err(format!(
                        "windows: 무인 설치 시작 시각({})이 이미 지났습니다. 지난 시각을 주면 고리가 한꺼번에 \
                         설치합니다(바로 시작하려면 --auto-after windows=+0d)",
                        time::format_utc(t)
                    ))
                }
                // the standing default for a new release (design 5-1): two days for the
                // operator to try it first; the rings follow day by day
                None if latest_changed => {
                    let after = now + DEFAULT_AUTO_INSTALL_DAYS * DAY;
                    self.report.push(format!(
                        "windows: 무인 설치는 기본값대로 게시 {DEFAULT_AUTO_INSTALL_DAYS}일 뒤({})부터 고리별로 \
                         시작합니다. 하지 않으려면 --auto-after windows=none",
                        time::format_kst(after)
                    ));
                    Some(after)
                }
                None => before.and_then(|b| b.auto_install_after),
            };
            if entry.package.is_none() {
                if self.req.auto_after.is_some_and(|a| a != AutoAfter::Off) {
                    return Err(
                        "windows: 패키지가 없는 항목에는 무인 설치 시각을 둘 수 없습니다"
                            .to_owned(),
                    );
                }
                entry.auto_install_after = None;
            }
        }
        Ok(Some((entry, latest_changed)))
    }

    /// iOS: the policy only names versions; TestFlight delivers. The operator confirms the build.
    fn ios(&mut self, before: Option<&Entry>) -> Result<Option<(Entry, bool)>, String> {
        let platform = Platform::Ios;
        let now = self.req.now;
        let artifact = self.artifact(Built::Ios);
        let wanted = self.latest_option(platform);
        let Some(build) = &self.req.ios_valid else {
            let Some(b) = before else {
                if self.named(platform) {
                    return Err(
                        "ios: 항목을 만들려면 --ios-valid <TestFlight 빌드 번호>가 필요합니다"
                            .to_owned(),
                    );
                }
                if let Some(a) = artifact {
                    self.report.push(format!(
                        "ios: {} 산출물이 있지만 --ios-valid가 없어 항목을 만들지 않았습니다",
                        a.info.version
                    ));
                }
                return Ok(None);
            };
            if wanted.is_some_and(|v| v != b.latest) {
                return Err(
                    "ios: latest를 바꾸려면 --ios-valid <빌드>로 TestFlight 빌드(VALID)를 확인해 주세요".to_owned(),
                );
            }
            if self.min_option(platform).is_some_and(|v| v > b.min) {
                return Err(
                    "ios: min을 올리려면 --ios-valid <빌드>로 그 TestFlight 빌드가 내부 그룹에서 \
                     시험 중(IN_BETA_TESTING)임을 확인해 주세요"
                        .to_owned(),
                );
            }
            if let Some(a) = artifact.filter(|a| a.info.version > b.latest) {
                self.report.push(format!(
                    "ios: {} 산출물이 있지만 --ios-valid가 없어 latest({})를 그대로 둡니다",
                    a.info.version, b.latest
                ));
            }
            if let Some(age) = b.testflight_build.as_deref().and_then(testflight_age(now)) {
                if age >= TESTFLIGHT_WARN_DAYS {
                    self.report.push(format!(
                        "경고: ios TestFlight 빌드를 올린 지 {age}일입니다(90일에 만료). 새 빌드를 올리세요"
                    ));
                }
            }
            return Ok(Some((b.clone(), false)));
        };
        let built_at = time::build_number_time(build).map_err(|e| format!("--ios-valid: {e}"))?;
        let age = (now - built_at).div_euclid(DAY);
        if now - built_at < -DAY {
            return Err(format!("ios: TestFlight 빌드 {build}의 시각이 미래입니다"));
        }
        if age >= TESTFLIGHT_MAX_DAYS {
            return Err(format!(
                "ios: TestFlight 빌드({build})를 올린 지 {age}일입니다(90일에 만료, {TESTFLIGHT_MAX_DAYS}일부터 거부). \
                 새 빌드를 올린 뒤 그 빌드로 정책을 만드세요"
            ));
        }
        let from_artifact = artifact
            .filter(|a| a.info.build == *build)
            .map(|a| a.info.version);
        let from_before = before
            .filter(|b| b.testflight_build.as_deref() == Some(build.as_str()))
            .map(|b| b.latest);
        let latest = match (wanted, from_artifact, from_before) {
            (Some(v), Some(a), _) if v != a => {
                return Err(format!(
                    "ios: --latest의 버전({v})이 릴리스 디렉터리에 있는 빌드 {build}의 버전({a})과 다릅니다"
                ))
            }
            (Some(v), _, Some(b)) if v != b => {
                return Err(format!("ios: 지금 정책에서 빌드 {build}의 버전은 {b}입니다(--latest {v})"))
            }
            (Some(v), _, _) => v,
            (None, Some(a), _) => a,
            (None, None, Some(b)) => b,
            (None, None, None) => {
                return Err(format!(
                    "ios: 빌드 {build}의 버전을 알 수 없습니다. 릴리스 디렉터리에 그 빌드의 release.json을 \
                     두거나 --latest ios=<버전>을 주세요"
                ))
            }
        };
        if let Some(old) = before.and_then(|b| b.testflight_build.as_deref()) {
            let newer = build.as_str() > old;
            let same_build = build == old;
            let same_version = before.is_some_and(|b| b.latest == latest);
            if (same_version && !same_build && !newer)
                || (!same_version && !newer && !self.req.relax)
            {
                return Err(format!(
                    "ios: 이 빌드({build})가 지금 정책의 빌드({old})보다 새롭지 않습니다"
                ));
            }
        }
        let latest_changed = before.is_none_or(|b| b.latest != latest);
        let mut entry = before
            .cloned()
            .unwrap_or_else(|| Entry::new(latest, latest));
        entry.latest = latest;
        entry.testflight_build = Some(build.clone());
        if latest_changed {
            entry.source = Some(format!("client-{latest}"));
        }
        self.notes.push(format!(
            "ios {latest}: 운영자가 TestFlight 빌드({build})를 확인함(--ios-valid, 올린 지 {age}일)"
        ));
        Ok(Some((entry, latest_changed)))
    }

    /// Google Play: the operator saw the versionCode released on the internal track.
    fn play(&mut self, before: Option<&Entry>) -> Result<Option<(Entry, bool)>, String> {
        let platform = Platform::AndroidPlay;
        let artifact = self.artifact(Built::Android);
        let wanted = self.latest_option(platform);
        let Some(code) = self.req.play_live else {
            let Some(b) = before else {
                if self.named(platform) {
                    return Err(
                        "android_play: 항목을 만들려면 --play-live <versionCode>가 필요합니다"
                            .to_owned(),
                    );
                }
                return Ok(None);
            };
            let confirm =
                "--play-live <versionCode>로 Play 내부 트랙의 출시(completed)를 확인해 주세요";
            if wanted.is_some_and(|v| v != b.latest) {
                return Err(format!("android_play: latest를 바꾸려면 {confirm}"));
            }
            if self.min_option(platform).is_some_and(|v| v > b.min) {
                return Err(format!("android_play: min을 올리려면 {confirm}"));
            }
            return Ok(Some((b.clone(), false)));
        };
        if !(1..=MAX_VERSION_CODE).contains(&code) {
            return Err(format!("--play-live: versionCode가 범위 밖입니다: {code}"));
        }
        let newer = artifact.filter(|a| before.is_none_or(|b| a.info.version > b.latest));
        let latest = match (wanted, before, newer) {
            (Some(v), Some(b), _) if v == b.latest => v,
            (Some(v), _, _) if artifact.is_some_and(|a| a.info.version == v) => v,
            (Some(v), _, _) => {
                return Err(format!(
                    "android_play: latest를 올리려면 릴리스 디렉터리에 Android {v} 산출물이 있어야 \
                     합니다(versionCode를 대조합니다)"
                ))
            }
            (None, _, Some(a)) => a.info.version,
            (None, Some(b), None) => b.latest,
            (None, None, None) => return Err(
                "android_play: 버전을 알 수 없습니다. 릴리스 디렉터리에 Android 산출물을 두세요"
                    .to_owned(),
            ),
        };
        // the versionCode that version has: the APK and the bundle of one build share it
        let needed = match artifact.filter(|a| a.info.version == latest) {
            Some(a) => a.info.build.parse::<u64>().map_err(|_| {
                format!(
                    "android_play: Android 산출물의 build 값({:?})이 versionCode가 아닙니다",
                    a.info.build
                )
            })?,
            None => before
                .and_then(|b| b.version_code)
                .ok_or("android_play: 지금 정책에 version_code가 없습니다")?,
        };
        let recorded = before
            .filter(|b| b.latest == latest)
            .and_then(|b| b.version_code);
        if code < needed || recorded.is_some_and(|r| code < r) {
            return Err(format!(
                "android_play: Play 내부 트랙의 versionCode({code})가 {latest}의 versionCode({})보다 낮습니다. \
                 트랙에 {latest} 버전이 출시(completed)된 뒤 다시 하세요",
                needed.max(recorded.unwrap_or(0))
            ));
        }
        let latest_changed = before.is_none_or(|b| b.latest != latest);
        let mut entry = before
            .cloned()
            .unwrap_or_else(|| Entry::new(latest, latest));
        entry.latest = latest;
        entry.version_code = Some(code);
        if latest_changed {
            entry.source = Some(format!("client-{latest}"));
        }
        self.notes.push(format!(
            "android_play {latest}: 운영자가 Play 내부 트랙의 versionCode({code})를 확인함(--play-live)"
        ));
        Ok(Some((entry, latest_changed)))
    }
}

fn pick_ref(list: &[(Target, String)], platform: Platform) -> Option<(bool, &str)> {
    let one = list.iter().find(|(t, _)| *t == Target::One(platform));
    let all = list.iter().find(|(t, _)| *t == Target::All);
    one.map(|(_, v)| (true, v.as_str()))
        .or(all.map(|(_, v)| (false, v.as_str())))
}

/// The age in days of a TestFlight build at `now`.
pub fn testflight_age(now: i64) -> impl Fn(&str) -> Option<i64> {
    move |build| {
        time::build_number_time(build)
            .ok()
            .map(|t| (now - t).div_euclid(DAY))
    }
}

fn no_duplicates<T: PartialEq + Copy>(
    items: impl Iterator<Item = T>,
    option: &str,
) -> Result<(), String> {
    let mut seen: Vec<T> = Vec::new();
    for item in items {
        if seen.contains(&item) {
            return Err(format!("{option}: 같은 항목에 두 번 줬습니다"));
        }
        seen.push(item);
    }
    Ok(())
}

/// What changes from `old` to `new`, for people.
pub fn describe_changes(old: Option<&Policy>, new: &Policy) -> Vec<String> {
    let Some(old) = old else {
        return vec!["첫 정책입니다(이전 정책 없음)".to_owned()];
    };
    let mut lines = Vec::new();
    if old.epoch != new.epoch {
        lines.push(format!("세대: {} → {}", old.epoch, new.epoch));
    }
    lines.push(format!("seq: {} → {}", old.seq, new.seq));
    if old.contact_ko != new.contact_ko {
        lines.push(format!(
            "안내 문구: {}",
            new.contact_ko.as_deref().unwrap_or("(없앰)")
        ));
    }
    let when = |t: Option<i64>| t.map_or("없음".to_owned(), time::format_kst);
    for platform in Platform::ALL {
        match (old.platforms.get(&platform), new.platforms.get(&platform)) {
            (None, None) => {}
            (None, Some(e)) => lines.push(format!(
                "{platform}: 새 항목(최소 {}, 최신 {}, 유예 끝 {}{})",
                e.min,
                e.latest,
                when(e.grace_until),
                e.grace_min
                    .map(|v| format!(", 그때까지 {v} 미만만 막음"))
                    .unwrap_or_default()
            )),
            (Some(_), None) => lines.push(format!("{platform}: 항목을 뺌")),
            (Some(a), Some(b)) => {
                let mut changes = Vec::new();
                if a.min != b.min {
                    changes.push(format!("최소 {} → {}", a.min, b.min));
                }
                if a.latest != b.latest {
                    changes.push(format!("최신 {} → {}", a.latest, b.latest));
                }
                if a.grace_until != b.grace_until {
                    changes.push(format!(
                        "유예 끝 {} → {}",
                        when(a.grace_until),
                        when(b.grace_until)
                    ));
                }
                if a.grace_min != b.grace_min {
                    let v = |v: Option<Version>| v.map_or("없음".to_owned(), |v| v.to_string());
                    changes.push(format!(
                        "유예 중 막는 버전(grace_min) {} → {}",
                        v(a.grace_min),
                        v(b.grace_min)
                    ));
                }
                if a.auto_install_after != b.auto_install_after {
                    changes.push(format!(
                        "무인 설치 시작 {} → {}",
                        when(a.auto_install_after),
                        when(b.auto_install_after)
                    ));
                }
                if a.os_min != b.os_min {
                    changes.push(format!(
                        "OS 최소 {} → {}",
                        a.os_min.as_deref().unwrap_or("없음"),
                        b.os_min.as_deref().unwrap_or("없음")
                    ));
                }
                if a.package != b.package {
                    match &b.package {
                        Some(p) => changes.push(format!("패키지 {} (sha256 {})", p.file, p.sha256)),
                        None => changes.push("패키지를 거둬들임".to_owned()),
                    }
                }
                if a.version_code != b.version_code {
                    changes.push(format!("Play versionCode {}", b.version_code.unwrap_or(0)));
                }
                if a.testflight_build != b.testflight_build {
                    changes.push(format!(
                        "TestFlight 빌드 {}",
                        b.testflight_build.as_deref().unwrap_or("없음")
                    ));
                }
                if a.move_to_play != b.move_to_play {
                    changes.push(format!(
                        "Play로 옮기기 안내 {}",
                        if b.move_to_play { "켬" } else { "끔" }
                    ));
                }
                if a.notes_ko != b.notes_ko {
                    changes.push("알림 글 바뀜".to_owned());
                }
                if changes.is_empty() {
                    lines.push(format!("{platform}: 그대로"));
                } else {
                    lines.push(format!("{platform}: {}", changes.join(", ")));
                }
            }
        }
    }
    lines
}

/// The size the signed envelope will have.
pub fn envelope_size(p: &str, root_p: Option<&str>) -> usize {
    unsigned_envelope(p, 65535, root_p.map(|p| (65535, p))).len()
}

/// An envelope as it will be signed, with signatures not made yet: what `policy` and `sign`
/// check by the apps' rules with the signatures taken as good ([`trust::accept_draft`]).
pub fn unsigned_envelope(p: &str, k: u32, root: Option<(u32, &str)>) -> String {
    let part = |k: u32, p: &str| Part {
        k,
        p: p.to_owned(),
        s: [0; 64],
    };
    Envelope {
        policy: part(k, p),
        root: root.map(|(k, p)| part(k, p)),
    }
    .to_file()
}

/// X4 (4-3) for an envelope that will be (or was) signed: taken by a device that holds `prev`
/// and by a fresh install at the key file's floor; a policy of epoch 2 or later asks no less
/// than its declaration's base. `signed` says whether the signatures are made (in `sign`) or
/// taken as good (in `policy`).
pub fn x4(
    file: &str,
    prev: Option<&str>,
    keys: &KeySet,
    server: &str,
    signed: bool,
) -> Result<(), String> {
    let run = |state: &State| {
        if signed {
            trust::accept(keys, server, state, file.as_bytes())
        } else {
            trust::accept_draft(keys, server, state, file.as_bytes())
        }
    };
    if let Some(prev) = prev {
        let holding = trust::holding(keys, server, prev);
        if holding.held.is_none() {
            return Err("X4: 이전 정책을 이 키 파일로 읽지 못합니다(--prev)".to_owned());
        }
        let (_, outcome) = run(&holding)?;
        if outcome != trust::Outcome::Taken {
            let why = outcome
                .reason()
                .map_or("같은 정책입니다".to_owned(), |r| r.code());
            return Err(format!(
                "X4: 이전 정책을 가진 앱이 이 정책을 받지 않습니다({why}: {})",
                outcome.reason().map(|r| r.korean()).unwrap_or_default()
            ));
        }
    }
    let (_, outcome) = run(&State::default())?;
    if outcome != trust::Outcome::Taken {
        let reason = outcome.reason();
        return Err(format!(
            "X4: 키 파일의 하한({})으로 새로 설치한 앱이 이 정책을 받지 않습니다({}: {})",
            keys.floor,
            reason.map(|r| r.code()).unwrap_or_default(),
            reason.map(|r| r.korean()).unwrap_or_default()
        ));
    }
    let checked = trust::canonical(file.as_bytes())?;
    if let (Some(form), true) = (&checked.root, checked.policy.epoch >= 2) {
        let decl = form
            .judge(Some(server))
            .map_err(|r| format!("X4: 루트 선언: {}", r.korean()))?;
        let policy = checked
            .policy
            .judge(Some(server))
            .map_err(|r| format!("X4: 정책: {}", r.korean()))?;
        if trust::relaxes(&decl.base, &policy.requirements()) {
            return Err("X4: 정책이 자기 루트 선언의 base보다 덜 막습니다".to_owned());
        }
    }
    Ok(())
}

/// What X1 says about entries that ask less: one line per entry and kind.
pub fn loosened_lines(old: &Policy, new: &Policy) -> Vec<String> {
    loosenings(&old.requirements(), &new.requirements())
        .into_iter()
        .map(|(p, kind)| format!("{p}: {}", kind.korean()))
        .collect()
}

/// The signer of a policy (4-2 Signer): a signing key of the key file that the (new or carried)
/// declaration does not revoke and every recorded key set pins; or, with `--root-signs-policy`
/// and only while no such key exists, a root key every recorded key set pins.
pub fn policy_signer(
    keys: &KeySet,
    archive: &Archive,
    revoked: &[u32],
    root_signs: bool,
) -> Result<(Role, u32), String> {
    let mut serving = Vec::new();
    for key in keys.signing.iter().filter(|k| !revoked.contains(&k.id)) {
        if archive.not_pinning(Role::Signing, key)?.is_empty() {
            serving.push(key.id);
        }
    }
    if root_signs {
        if let Some(k) = serving.first() {
            return Err(format!(
                "X9: 서명 키 {k}가 기록된 모든 빌드에 박혀 있습니다. 정책은 그 키로 서명합니다(--root-signs-policy를 빼세요)"
            ));
        }
        let root = serving_roots(keys, archive)?
            .first()
            .copied()
            .ok_or("X12: 기록된 모든 빌드가 박은 루트 키가 키 파일에 없습니다")?;
        return Ok((Role::Root, root));
    }
    match serving.first() {
        Some(k) => Ok((Role::Signing, *k)),
        None => {
            let mut unserved = Vec::new();
            for key in keys.signing.iter().filter(|k| !revoked.contains(&k.id)) {
                for record in archive.not_pinning(Role::Signing, key)? {
                    unserved.push(format!("서명 키 {}: {}", key.id, record.name()));
                }
            }
            Err(format!(
                "X12: 폐기되지 않은 서명 키 가운데 기록된 모든 빌드가 박은 키가 없습니다({}). 그 빌드가 쓰이는 동안은 \
                 루트 키가 정책에 서명합니다(--root-signs-policy, 런북 5-8 6번)",
                if unserved.is_empty() {
                    "키 파일에 쓸 서명 키가 없음".to_owned()
                } else {
                    unserved.join("; ")
                }
            ))
        }
    }
}

/// The root keys of the key file that every recorded key set pins (X12).
pub fn serving_roots(keys: &KeySet, archive: &Archive) -> Result<Vec<u32>, String> {
    let mut out = Vec::new();
    for key in &keys.roots {
        if archive.not_pinning(Role::Root, key)?.is_empty() {
            out.push(key.id);
        }
    }
    Ok(out)
}

/// Makes the next policy.
pub fn make(
    req: &Request,
    keys: &KeySet,
    archive: &Archive,
    settings: &Settings,
    tools: &dyn Tools,
) -> Result<Made, String> {
    let now = req.now;
    no_duplicates(req.min.iter().map(|(t, _)| *t), "--min")?;
    no_duplicates(
        req.grace.iter().map(|(t, _)| *t),
        "--grace-days/--grace-until",
    )?;
    no_duplicates(req.notes.iter().map(|(t, _)| *t), "--notes")?;
    no_duplicates(req.latest.iter().map(|(p, _)| *p), "--latest")?;
    no_duplicates(req.os_min.iter().map(|(p, _)| *p), "--os-min")?;
    no_duplicates(req.hold.iter().chain(&req.drop).copied(), "--hold/--drop")?;
    no_duplicates(req.revoke.iter().copied(), "--revoke-key")?;
    no_duplicates(req.grace_min.iter().map(|(p, _)| *p), "--grace-min")?;
    no_duplicates(req.withdraw.iter().copied(), "--withdraw-package")?;
    if let Some(p) = req.withdraw.iter().find(|p| !p.has_package()) {
        return Err(format!(
            "--withdraw-package {p}: 패키지는 windows와 android에만 있습니다"
        ));
    }
    let new_decl = req.new_declaration();

    let (server, prev): (String, Option<Checked>) = match &req.prev {
        Some(file) => {
            let server = peek_server(file).map_err(|e| format!("이전 정책(--prev): {e}"))?;
            if req.server.as_ref().is_some_and(|s| *s != server) {
                return Err(format!(
                    "--server의 값({:?})이 이전 정책의 서버 이름({server:?})과 다릅니다",
                    req.server.as_deref().unwrap_or("")
                ));
            }
            let checked = check_envelope(file, keys, &server)
                .map_err(|e| format!("이전 정책(--prev): {e}"))?;
            (server, Some(checked))
        }
        None => (
            req.server
                .clone()
                .ok_or("--prev none에는 --server <서버 이름>이 필요합니다")?,
            None,
        ),
    };
    check_server(&server)?;
    let old = prev.as_ref().map(|c| &c.policy);
    if old.is_some_and(|old| now < old.issued) {
        return Err(format!(
            "이 PC의 시계({})가 이전 정책의 발행 시각보다 이릅니다. 시계를 확인하세요",
            time::format_utc(now)
        ));
    }

    // the archive: a declaration only from the live file, which is --prev and a file of the
    // archive (X10); a policy only after the archive's highest file that is not voided (X11)
    let highest = archive.highest()?;
    if new_decl {
        let Some(prev_file) = req.prev.as_deref() else {
            return Err(
                "새 루트 선언은 서버에 있는 정책에서만 만듭니다: --prev와 --live에 서버의 정책 파일을 주세요(X10)"
                    .to_owned(),
            );
        };
        let live = req.live.as_deref().ok_or(
            "새 루트 선언(--relax, --drop, --revoke-key, --new-epoch)에는 --live <https://<서버>/api/hjv/version에서 \
             받은 파일>이 필요합니다(X10)",
        )?;
        if live != prev_file {
            return Err(
                "X10: --live의 파일이 --prev와 바이트 단위로 같지 않습니다. 서버의 정책 파일을 다시 받아 둘 다에 주세요"
                    .to_owned(),
            );
        }
        if archive.find(prev_file)?.is_none() {
            return Err(format!(
                "X10: 서버의 정책이 보관소({})에 없습니다. 다른 곳에서 서명된 것입니다: 서명 키를 도둑맞았거나(런북 5-8), \
                 깨끗한 컴퓨터에서 한 서명의 파일을 보관소로 옮기지 않았습니다(5-0). 서버의 파일을 보관소의 마지막 파일로 \
                 되돌리거나 파일을 옮긴 뒤 다시 하세요. archive adopt는 직접 서명한 파일에만 씁니다",
                archive.dir.display()
            ));
        }
    } else {
        match (&highest, &req.prev) {
            (None, None) => {}
            (None, Some(_)) => {
                return Err(format!(
                    "X11: 보관소({})에 서명한 정책이 없습니다. --prev가 직접 서명한 파일이면 먼저 archive adopt로 넣으세요",
                    archive.dir.display()
                ))
            }
            (Some(h), None) => {
                return Err(format!(
                    "X11: 보관소에 서명한 정책이 있습니다. --prev에 보관소의 마지막 파일을 주세요: {}",
                    h.path.display()
                ))
            }
            (Some(h), Some(p)) if h.file != *p => {
                return Err(format!(
                    "X11: --prev가 보관소의 마지막 파일(무효로 하지 않은 가장 높은 위치 {}:{})이 아닙니다: {}. 서명한 파일은 \
                     낸 것으로 칩니다. 잘못 서명했고 컴퓨터 밖으로 나가지 않았다면 void로 무효로 하세요(5-13)",
                    h.epoch,
                    h.seq,
                    h.path.display()
                ))
            }
            _ => {}
        }
    }
    if old.is_none() && keys.floor.epoch != 1 {
        return Err("첫 정책은 세대 1입니다. 키 파일의 하한 세대가 1이 아닙니다".to_owned());
    }
    let mut warnings = Vec::new();
    if old.is_none() && keys.floor.seq > 0 {
        warnings.push(format!(
            "경고: 키 파일의 하한이 {}입니다. 이전 정책이 있었다면 --prev로 줘야 합니다",
            keys.floor
        ));
    }

    let mut maker = Maker {
        req,
        prev: old.map(|p| &p.platforms),
        report: Vec::new(),
        notes: Vec::new(),
        grace_for_all_used: false,
        notes_for_all_used: false,
        runs: Runs::default(),
    };
    let mut platforms = BTreeMap::new();
    for platform in Platform::ALL {
        let before = old.and_then(|p| p.platforms.get(&platform));
        if req.drop.contains(&platform) || req.hold.contains(&platform) {
            if maker.named(platform) {
                return Err(format!(
                    "{platform}: --hold나 --drop을 준 항목에 다른 옵션을 함께 줄 수 없습니다"
                ));
            }
            match (before, req.hold.contains(&platform)) {
                (Some(before), true) => drop(platforms.insert(platform, before.clone())),
                (Some(_), false) => {}
                (None, true) => {}
                (None, false) => return Err(format!("{platform}: 뺄 항목이 지금 정책에 없습니다")),
            }
            continue;
        }
        let made = match platform {
            Platform::Windows => maker.packaged(Built::Windows, before, settings, tools)?,
            Platform::Android => maker.packaged(Built::Android, before, settings, tools)?,
            Platform::AndroidPlay => maker.play(before)?,
            Platform::Ios => maker.ios(before)?,
        };
        if let Some((entry, latest_changed)) = made {
            let entry = maker.finish(platform, before, entry, latest_changed)?;
            platforms.insert(platform, entry);
        } else if maker.named(platform) {
            return Err(format!("{platform}: 만들 항목이 없는데 옵션을 줬습니다"));
        }
    }
    if platforms.is_empty() {
        return Err(
            "정책에 항목이 하나도 없습니다(--release-dir, --ios-valid, --play-live)".to_owned(),
        );
    }
    let has = |p: Platform| platforms.contains_key(&p);
    if req.auto_after.is_some()
        && (!has(Platform::Windows) || req.hold.contains(&Platform::Windows))
    {
        return Err("--auto-after를 줬지만 바꿀 windows 항목이 없습니다".to_owned());
    }
    if req.move_to_play.is_some()
        && (!has(Platform::Android) || req.hold.contains(&Platform::Android))
    {
        return Err("--move-to-play를 줬지만 바꿀 android 항목이 없습니다".to_owned());
    }
    if req.grace.iter().any(|(t, _)| *t == Target::All) && !maker.grace_for_all_used {
        return Err(
            "유예 옵션을 줬지만 min이 바뀌는 항목이 없습니다. 항목을 정하려면 --grace-until <항목>=T \
             (같은 min의 유예를 늦추는 것은 완화라서 --relax와 루트 키가 필요합니다)"
                .to_owned(),
        );
    }
    if req.notes.iter().any(|(t, _)| *t == Target::All) && !maker.notes_for_all_used {
        return Err(
            "--notes를 줬지만 latest가 바뀌는 항목이 없습니다. 항목을 정하려면 --notes <항목>=FILE"
                .to_owned(),
        );
    }

    // the package rule (4-2, X7): a windows or android entry carries a package, unless it is
    // withdrawn in this policy or was without one at the same latest
    for (platform, entry) in platforms.iter().filter(|(p, _)| p.has_package()) {
        let before = old.and_then(|p| p.platforms.get(platform));
        let kept_without = before.is_some_and(|b| b.package.is_none() && b.latest == entry.latest);
        if entry.package.is_none() && !req.withdraw.contains(platform) && !kept_without {
            return Err(format!(
                "{platform}: 패키지가 없습니다(--release-dir). 거둬들이려면 --withdraw-package {platform}"
            ));
        }
    }

    // the position: one above every position of the epoch the archive holds; a new
    // declaration opens the epoch above every epoch the archive holds (4-2 Position)
    let (epoch, seq) = if new_decl {
        if req.seq.is_some() {
            return Err(
                "새 루트 선언과 함께 --seq를 줄 수 없습니다(새 세대는 seq 1부터)".to_owned(),
            );
        }
        let above = old.map_or(0, |p| p.epoch).max(archive.max_epoch()?);
        (above + 1, 1)
    } else {
        let epoch = old.map_or(keys.floor.epoch, |p| p.epoch);
        let last = old.map_or(keys.floor.seq, |p| p.seq);
        let above = archive.max_seq(epoch)?.unwrap_or(0).max(last);
        let seq = match req.seq {
            None => above + 1,
            Some(seq) if seq <= above => {
                return Err(format!(
                    "--seq의 값({seq})이 이 세대에서 보관소가 가진 가장 높은 seq({above})보다 높지 않습니다"
                ))
            }
            Some(seq) if seq > last + MAX_SEQ_AHEAD => {
                return Err(format!(
                    "--seq의 값({seq})이 이전 정책({last})에서 {MAX_SEQ_AHEAD} 넘게 뜹니다. 앱이 받지 않습니다"
                ))
            }
            Some(seq) => seq,
        };
        (epoch, seq)
    };

    let contact_ko = match &req.contact {
        Some(text) if text.is_empty() => None,
        Some(text) => Some(text.clone()),
        None => old.and_then(|p| p.contact_ko.clone()),
    };
    let policy = Policy {
        epoch,
        seq,
        issued: now,
        server: server.clone(),
        contact_ko,
        platforms,
    };
    policy.check()?;

    // X1 and X8: asking less needs a declaration
    let loosened = old
        .map(|old| loosened_lines(old, &policy))
        .unwrap_or_default();
    let below: Vec<String> = old
        .map(|old| new_entries_below(old, &policy))
        .unwrap_or_default()
        .into_iter()
        .map(|(p, min, lowest)| {
            format!("{p}: 새 항목의 최소 버전({min})이 지금 가장 낮은 최소 버전({lowest})보다 낮아 hbbs의 MIN이 내려감")
        })
        .collect();
    if !new_decl && !(loosened.is_empty() && below.is_empty()) {
        // X8 is the tool's own rule (hbbs's one MIN); its way out is X1's
        let rules = match (loosened.is_empty(), below.is_empty()) {
            (false, false) => "X1, X8",
            (false, true) => "X1",
            (true, _) => "X8",
        };
        return Err(format!(
            "{rules}: 이전 정책보다 덜 막습니다({}). 루트 키의 선언이 있어야 합니다: --relax --live <서버의 정책 파일>(새 세대)",
            loosened.iter().chain(&below).cloned().collect::<Vec<_>>().join("; ")
        ));
    }

    // the declaration: carried with the same bytes, or a new one whose base is this policy's
    // requirement part (4-2 Declaration, X2)
    let carried = prev
        .as_ref()
        .and_then(|c| c.envelope.root.clone())
        .filter(|_| !new_decl);
    let old_decl = prev.as_ref().and_then(|c| c.root.as_ref());
    let new_root = if new_decl {
        let mut revoked: Vec<u32> = old_decl.map(|d| d.revoked.clone()).unwrap_or_default();
        for id in &req.revoke {
            crate::keys::check_id(Role::Signing, *id).map_err(|e| {
                format!("X2: --revoke-key {id}: {e}. 루트 키는 폐기하지 않습니다(런북 5-10)")
            })?;
            if revoked.contains(id) {
                return Err(format!("--revoke-key {id}: 이미 폐기한 키입니다"));
            }
            revoked.push(*id);
        }
        revoked.sort_unstable();
        let decl = RootDecl {
            server: server.clone(),
            epoch,
            revoked,
            issued: now,
            base: policy.requirements(),
        };
        decl.check().map_err(|e| format!("X2: {e}"))?;
        Some(decl)
    } else {
        None
    };
    let revoked_now: Vec<u32> = new_root
        .as_ref()
        .or(old_decl)
        .map(|d| d.revoked.clone())
        .unwrap_or_default();

    // the signer (X9, X12)
    let (signer, k) = policy_signer(keys, archive, &revoked_now, req.root_signs_policy)?;
    let root_key = if new_decl {
        Some(*serving_roots(keys, archive)?.first().ok_or(
            "X12: 기록된 모든 빌드가 박은 루트 키가 키 파일에 없습니다. 새 루트 선언에 서명할 키가 없습니다",
        )?)
    } else {
        None
    };

    // X4: a device holding --prev and a fresh install take it
    let text = policy.to_text();
    let root_p = new_root.as_ref().map(RootDecl::to_text);
    let root_for_check: Option<(u32, String)> = match (&root_p, &carried) {
        (Some(p), _) => root_key.map(|k| (k, p.clone())),
        (None, Some(part)) => Some((part.k, part.p.clone())),
        (None, None) => None,
    };
    let unsigned = unsigned_envelope(
        &text,
        k,
        root_for_check.as_ref().map(|(k, p)| (*k, p.as_str())),
    );
    let size = envelope_size(&text, root_for_check.as_ref().map(|(_, p)| p.as_str()));
    if size > MAX_ENVELOPE_BYTES {
        return Err(format!(
            "X5: 봉투가 {size}바이트가 됩니다. 앱은 {MAX_ENVELOPE_BYTES}바이트까지만 읽습니다(알림 글을 줄이세요)"
        ));
    }
    x4(&unsigned, req.prev.as_deref(), keys, &server, false)?;

    let mut report = describe_changes(old, &policy);
    report.append(&mut maker.report);
    report.append(&mut warnings);
    // without the Google Play entry no minimum reaches the apps installed from Google Play: said,
    // not refused, since Play may not carry the version yet
    if !policy.platforms.contains_key(&Platform::AndroidPlay) {
        report.push(NO_PLAY_ENTRY.to_owned());
    }
    if req.auto_after == Some(AutoAfter::Off) {
        if let Some(line) = auto_off_warning(old, &policy, now) {
            report.push(line);
        }
    }
    if let Some(decl) = &new_root {
        report.push(format!(
            "새 루트 선언: 세대 {} · 폐기한 서명 키 {:?} · 루트 키 {}로 서명",
            decl.epoch,
            decl.revoked,
            root_key.unwrap_or_default()
        ));
        for line in trust::describe_declaration(decl).into_iter().skip(1) {
            report.push(line);
        }
        for line in &loosened {
            report.push(format!("완화: {line}"));
        }
        for line in &below {
            report.push(format!("완화(도구 규칙): {line}"));
        }
        let relaxed = old
            .map(|old| trust::relaxed(&old.requirements(), &policy.requirements()))
            .unwrap_or_default();
        report.push(relaxes_line(&relaxed));
    }
    report.push(format!(
        "서명할 키: 정책은 {} {k}{}",
        signer.korean(),
        match root_key {
            Some(r) => format!(", 루트 선언은 루트 키 {r}(깨끗한 컴퓨터에서, 런북 5-0)"),
            None => String::new(),
        }
    ));
    report.push(format!(
        "봉투 크기: 약 {size}바이트(한도 {MAX_ENVELOPE_BYTES})"
    ));

    let draft = Draft {
        p: text,
        signer,
        root: carried,
        root_p,
        prev: req.prev.clone(),
        notes: std::mem::take(&mut maker.notes),
    };
    // what `sign` will hold the draft to: better refused here than at the signer's terminal
    draft.check()?;
    Ok(Made {
        draft,
        policy,
        report,
    })
}

/// The server check's word for a new declaration (4-8 item 5): the entries the operator names.
pub fn relaxes_line(relaxed: &std::collections::BTreeSet<Platform>) -> String {
    if relaxed.is_empty() {
        "서버 확인: --relaxes 없이 설치합니다(완화하는 항목 없음)".to_owned()
    } else {
        let names: Vec<&str> = relaxed.iter().map(|p| p.name()).collect();
        format!(
            "서버 확인: --relaxes {} 로 설치합니다(완화하는 항목을 운영자가 이름으로 말합니다)",
            names.join(",")
        )
    }
}

/// `--auto-after windows=none` stops the rollout only: PCs below the Windows minimum still
/// install the latest by force (3-4). Said when the minimum was raised now or a raise is pending.
pub fn auto_off_warning(old: Option<&Policy>, new: &Policy, now: i64) -> Option<String> {
    let entry = new.platforms.get(&Platform::Windows)?;
    entry.package.as_ref()?;
    let before = old.and_then(|p| p.platforms.get(&Platform::Windows));
    let raised = before.is_some_and(|b| entry.min > b.min);
    let pending = entry.grace_until.is_some_and(|until| until > now);
    (raised || pending).then(|| {
        format!(
            "경고: --auto-after windows=none은 무인 설치(고리별)만 멈춥니다. 최소 버전({}) 미만인 PC는 {}을(를) \
             강제로 설치합니다{}. 그것도 멈추려면 --withdraw-package windows를 함께 주세요(런북 5-6)",
            entry.min,
            entry.latest,
            entry
                .grace_until
                .filter(|_| pending)
                .map(|t| format!("(유예 끝 {} 뒤)", time::format_kst(t)))
                .unwrap_or_default()
        )
    })
}

#[cfg(test)]
#[path = "make_tests.rs"]
mod tests;

// SPDX-License-Identifier: AGPL-3.0-only

//! `hjv-release policy`: makes the next policy from the previous one, the builds of a release and
//! what the operator asks for, and refuses what could not be undone later (design 1-4):
//!
//! - the position is the previous one plus one, and the last root declaration stays attached;
//! - a new Windows or Android `latest` needs its package and the provenance of its build; the
//!   same version never comes with another hash; `min` never exceeds `latest`;
//! - an iOS `latest` or a higher iOS `min` needs a TestFlight build the operator confirmed and
//!   that is younger than 80 days; a Google Play `latest` or a higher `min` needs the versionCode
//!   the operator saw released on the internal track;
//! - a higher `min` (or a new entry) comes with a grace period: the one asked for, or 7 days.
//!   `--grace-days 0` enforces at once. A grace period can be extended only with the root key;
//! - after a policy the root key signed, a policy for a signing key is checked against the
//!   policies from before it as well (`--held`): an app that was off while the root key's policy
//!   was served still holds an older one and compares the new policy with that. If the new policy
//!   relaxes what such an app holds (a `min` it knows with a later deadline, say), the app drops
//!   it and every later policy of a signing key;
//! - a policy that raises `os_min` keeps `min` at or below the last version the old OS runs;
//! - anything that relaxes, a new entry below what is enforced now, a revocation and a new epoch
//!   need `--relax`, `--revoke-key` or `--new-epoch`, and so the root key and the signer's typed
//!   consent.
//!
//! The result is a draft (draft.rs); nothing is signed here and no key file is read.

use crate::{
    draft::Draft,
    envelope::{Envelope, Part, MAX_ENVELOPE_BYTES},
    keys::{Floor, KeySet, Role},
    policy::{
        check_os_min, check_server, os_min_higher, Entry, Package, Platform, Policy, RootDecl,
        MAX_VERSION_CODE,
    },
    provenance::{self, Runs, Settings, Tools},
    release::{server_file_name, Artifact, Built},
    time::{self, DAY},
    trust::{
        self, check_envelope, new_entries_below, old_mins_that_wait, peek_server, relaxations,
        Checked, NewEntryBelow, Relaxation, State,
    },
    version::Version,
};
use std::collections::BTreeMap;

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

/// What `hjv-release policy` was asked for.
#[derive(Clone, Debug, Default)]
pub struct Request {
    /// The previous envelope file, or none for the first policy.
    pub prev: Option<String>,
    pub server: Option<String>,
    pub artifacts: Vec<Artifact>,
    pub min: Vec<(Target, Version)>,
    pub latest: Vec<(Platform, Version)>,
    /// Entries to keep exactly as they are.
    pub hold: Vec<Platform>,
    /// Entries to remove (a relaxation).
    pub drop: Vec<Platform>,
    pub grace: Vec<(Target, Grace)>,
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
    pub seq: Option<u64>,
    /// Envelope files from before the previous policy that apps may still hold (`--held`).
    pub held: Vec<String>,
    /// The operator says there is none (`--held none`).
    pub no_held: bool,
    pub now: i64,
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
    }

    fn grace_time(&self, platform: Platform, grace: Grace) -> Result<Option<i64>, String> {
        let now = self.req.now;
        match grace {
            Grace::Days(0) => Ok(None),
            Grace::Days(days) if (1..=365).contains(&days) => {
                Ok(Some(time::kst_midnight_after(now, days)))
            }
            Grace::Days(days) => Err(format!("유예 일수는 0-365입니다: {days}")),
            Grace::Until(t) if t > now => Ok(Some(t)),
            Grace::Until(t) => Err(format!(
                "{platform}: 유예 끝({})이 이미 지났습니다(바로 강제하려면 --grace-days 0)",
                time::format_utc(t)
            )),
        }
    }

    /// The fields every entry has: `min`, the grace period, `os_min` and the notes.
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
        entry.min = min;

        // a new entry counts as a raised min: it may block apps that nothing blocked before
        let raised = before.is_none_or(|b| min > b.min);
        let changed = before.is_none_or(|b| min != b.min);
        let kept = before.and_then(|b| b.grace_until);
        entry.grace_until = match pick(&self.req.grace, platform) {
            Some((true, grace)) => self.grace_time(platform, grace)?,
            Some((false, grace)) if changed => {
                self.grace_for_all_used = true;
                self.grace_time(platform, grace)?
            }
            Some((false, _)) => kept,
            // the standing default (design 1-5); said aloud, since a grace period can be cut
            // short later with a signing key but extended only with the root key
            None if raised => {
                let until = time::kst_midnight_after(self.req.now, DEFAULT_GRACE_DAYS);
                self.report.push(format!(
                    "{platform}: 유예 옵션이 없어 기본 {DEFAULT_GRACE_DAYS}일을 둡니다({}까지). \
                     바로 강제하려면 --grace-days 0, 다른 기간은 --grace-days N",
                    time::format_kst(until)
                ));
                Some(until)
            }
            // lowered: the old deadline belonged to the old min
            None if changed => None,
            None => kept,
        };
        // asked for or not, the operator reads it here and the signer is asked about it
        if raised && entry.grace_until.is_none() {
            self.report.push(format!(
                "{platform}: 유예가 없습니다. 서버에 넣는 즉시 {min} 미만을 막습니다"
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
        }
        if platform == Platform::Android {
            if let Some(move_to_play) = self.req.move_to_play {
                entry.move_to_play = move_to_play;
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
                "{platform}: 새 항목(최소 {}, 최신 {}, 유예 끝 {})",
                e.min,
                e.latest,
                when(e.grace_until)
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
                    if let Some(p) = &b.package {
                        changes.push(format!("패키지 {} (sha256 {})", p.file, p.sha256));
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
    let part = |p: &str, k: u32| Part {
        k,
        p: p.to_owned(),
        s: [0; 64],
    };
    Envelope {
        policy: part(p, 65535),
        root: root_p.map(|p| part(p, 65535)),
    }
    .to_file()
    .len()
}

/// Makes the next policy.
pub fn make(
    req: &Request,
    keys: &KeySet,
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

    let mut maker = Maker {
        req,
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

    // the position: the previous one plus one, or the first of a new epoch
    let old_root = prev.as_ref().and_then(|c| c.root.as_ref());
    let old_epoch = old.map_or(keys.floor.epoch, |p| p.epoch);
    let epoch = old_epoch + u64::from(req.new_epoch);
    let seq = match (req.seq, old) {
        (Some(seq), _) => seq,
        (None, _) if req.new_epoch => 1,
        (None, Some(old)) => old.seq + 1,
        (None, None) => keys.floor.seq + 1,
    };
    if !req.new_epoch && old.is_some_and(|old| seq <= old.seq) {
        return Err(format!(
            "--seq의 값({seq})이 이전 정책의 seq({})보다 높지 않습니다",
            old.map_or(0, |o| o.seq)
        ));
    }
    if old.is_none() && keys.floor.seq > 0 {
        maker.report.push(format!(
            "경고: 키 파일의 하한이 {}입니다. 이전 정책이 있었다면 --prev로 줘야 합니다",
            keys.floor
        ));
    }

    // the root declaration: the last one stays, or a new one for a revocation or a new epoch
    let mut root_p = None;
    let mut root_decl = old_root.cloned();
    if !req.revoke.is_empty() || req.new_epoch {
        let mut revoked: Vec<u32> = old_root.map(|r| r.revoked.clone()).unwrap_or_default();
        for id in &req.revoke {
            if *id == keys.root.id {
                return Err(format!(
                    "--revoke-key {id}: 루트 키 번호는 폐기할 수 없습니다"
                ));
            }
            if !(1..=65535).contains(id) {
                return Err(format!("--revoke-key {id}: 키 번호는 1-65535입니다"));
            }
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
        };
        decl.check()?;
        root_p = Some(decl.to_text());
        root_decl = Some(decl);
    }

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

    // who must sign: what an app would not take from a signing key, and what it would take
    // although it enforces less than now (a new entry below the present minimums)
    let relaxed = old.map(|old| relaxations(old, &policy)).unwrap_or_default();
    let lowered = old
        .map(|old| new_entries_below(old, &policy))
        .unwrap_or_default();
    let eased: Vec<String> = relaxed
        .iter()
        .map(Relaxation::describe)
        .chain(lowered.iter().map(NewEntryBelow::describe))
        .collect();
    if !eased.is_empty() && !req.relax {
        return Err(format!(
            "완화가 들어 있습니다: {}. 완화는 루트 키만 서명할 수 있습니다. 정말 필요하면 --relax를 주세요",
            eased.join("; ")
        ));
    }
    let signer = if req.relax { Role::Root } else { Role::Signing };
    let revoked: Vec<u32> = root_decl
        .as_ref()
        .map(|r| r.revoked.clone())
        .unwrap_or_default();
    let k = match signer {
        Role::Root => keys.root.id,
        Role::Signing => keys
            .signing
            .iter()
            .map(|k| k.id)
            .find(|id| !revoked.contains(id))
            .ok_or(
                "폐기되지 않은 서명 키가 키 파일에 없습니다. 새 서명 키를 박은 빌드가 나오기 전에는 \
                 루트 키로 서명합니다(--relax)",
            )?,
    };

    // an app that holds the previous policy must take this one
    let text = policy.to_text();
    let unsigned = Checked {
        envelope: Envelope {
            policy: Part {
                k,
                p: text.clone(),
                s: [0; 64],
            },
            root: None,
        },
        p_hash: trust::sha256(text.as_bytes()),
        policy: policy.clone(),
        root: root_decl.clone(),
        role: signer,
    };
    let state = prev
        .as_ref()
        .map_or_else(|| State::initial(keys), State::holding);
    state.accept(keys, &unsigned)?;

    // ... and so must an app that was off while the previous policy was served, when that was
    // a policy of the root key. Such an app holds an older policy and compares this one with
    // it: what the root key relaxed in between and this policy leaves relaxed is, to that app,
    // a relaxation by a signing key, and it drops this policy and every later one. Once one
    // signing-key policy passes for those apps, the later ones do (the rules of a signing key
    // only tighten).
    if req.no_held && !req.held.is_empty() {
        return Err("--held none과 --held <파일>을 함께 줄 수 없습니다".to_owned());
    }
    let prev_by_root = prev.as_ref().is_some_and(|c| c.role == Role::Root);
    if prev_by_root && signer == Role::Signing && req.held.is_empty() && !req.no_held {
        return Err(
            "직전 정책은 루트 키가 서명했습니다. 그 정책이 서버에 있는 동안 꺼져 있던 앱은 그 앞의 정책을 가진 \
             채로 새 정책을 비교합니다. 그 앱도 받는 정책인지 확인하도록 --held <루트 키의 정책 바로 앞의 \
             정책 파일>을 주세요(루트 키의 정책이 이어졌으면 그 사이의 것도 --held로, 그런 정책이 없으면 \
             --held none). 아직 완화를 되돌릴 수 없으면 이 정책도 루트 키로 서명합니다(--relax)"
                .to_owned(),
        );
    }
    for file in &req.held {
        let held = check_envelope(file, keys, &server).map_err(|e| format!("--held: {e}"))?;
        let at = Floor {
            epoch: held.policy.epoch,
            seq: held.policy.seq,
        };
        let before_prev = old.is_some_and(|old| {
            at < Floor {
                epoch: old.epoch,
                seq: old.seq,
            }
        });
        if !before_prev {
            return Err(format!(
                "--held: {at} 정책은 직전 정책보다 앞의 것이 아닙니다(앱이 아직 가지고 있을 수 있는 옛 정책을 줍니다)"
            ));
        }
        State::holding(&held)
            .accept(keys, &unsigned)
            .map_err(|e| {
                format!(
                    "{at} 정책을 가진 채 꺼져 있던 앱은 이 정책을 받지 않습니다: {e}. 그 정책보다 느슨한 데가 \
                     없게 하세요(같은 min이면 유예 끝을 그때의 기한보다 늦지 않게: --grace-until <그 시각> \
                     또는 --grace-days 0). 아직 그럴 수 없으면 이 정책도 루트 키로 서명합니다(--relax)"
                )
            })?;
        maker.notes.push(format!(
            "{at} 정책을 가진 채 꺼져 있던 앱도 이 정책을 받음을 확인함(--held)"
        ));
    }
    let position = Floor { epoch, seq };
    if position < keys.floor {
        return Err(format!(
            "정책 위치({position})가 키 파일의 하한({})보다 낮습니다. 이 키로 만든 빌드가 받지 않습니다",
            keys.floor
        ));
    }
    let carried = prev
        .as_ref()
        .and_then(|c| c.envelope.root.clone())
        .filter(|_| root_p.is_none());
    let root_text = root_p.as_deref().or(carried.as_ref().map(|r| r.p.as_str()));
    let size = envelope_size(&text, root_text);
    if size > MAX_ENVELOPE_BYTES {
        return Err(format!(
            "봉투가 {size}바이트가 됩니다. 앱은 {MAX_ENVELOPE_BYTES}바이트까지만 읽습니다(알림 글을 줄이세요)"
        ));
    }

    let mut report = describe_changes(old, &policy);
    report.append(&mut maker.report);
    for line in &eased {
        report.push(format!("완화: {line}"));
    }
    for waits in old
        .map(|old| old_mins_that_wait(old, &policy))
        .unwrap_or_default()
    {
        report.push(waits.describe());
    }
    if let Some(decl) = root_decl.as_ref().filter(|_| root_p.is_some()) {
        report.push(format!(
            "새 루트 선언: 세대 {}, 폐기한 서명 키 {:?}(루트 키로 서명)",
            decl.epoch, decl.revoked
        ));
    }
    if old.is_some_and(|old| old.epoch == epoch && seq > old.seq + SEQ_JUMP_TO_CONFIRM) {
        report.push(format!(
            "seq가 {SEQ_JUMP_TO_CONFIRM} 넘게 뜁니다: 서명할 때 확인을 묻습니다"
        ));
    }
    report.push(format!(
        "서명할 키: {}{}",
        signer.korean(),
        if root_p.is_some() && signer == Role::Signing {
            " + 루트 키(루트 선언)"
        } else {
            ""
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

#[cfg(test)]
#[path = "make_tests.rs"]
mod tests;

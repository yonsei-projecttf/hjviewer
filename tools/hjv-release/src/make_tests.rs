// SPDX-License-Identifier: AGPL-3.0-only

//! Tests of make.rs: a release history made with the test keys and recorded `gh` answers.

use super::*;
use crate::keys::Floor;
use crate::{
    keyfile::SecretKey,
    keys::{test_key, test_key_set},
    policy::tests::{t, v},
    provenance::tests::{run_json, Recorded},
    release::{
        scan,
        tests::{temp_dir, write_artifact},
    },
    signing,
    time::DAY,
};
use ed25519_dalek::SigningKey;
use std::{cell::RefCell, path::PathBuf};

thread_local! {
    /// The archive of the test running on this thread (each test has its own thread).
    static ARCHIVE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// The archive of this test's fixture.
fn archive() -> Archive {
    let dir = ARCHIVE
        .with(|a| a.borrow().clone())
        .expect("a Fixture first");
    Archive::open(&dir, true).unwrap()
}

const SERVER: &str = "rd.example.com";
/// November 8, 13:00 in Korea.
const NOW: &str = "2026-11-08T04:00:00Z";
// the iOS build numbers (UTC time of the build) and the Android versionCodes (the same + 2000)
const IOS_160: &str = "263100200";
const APK_160: &str = "263102200";
const IOS_161: &str = "263120249";
const APK_161: &str = "263122249";
const IOS_162: &str = "263130100";

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Fixture {
        let root = temp_dir(name);
        ARCHIVE.with(|a| *a.borrow_mut() = Some(root.join("archive")));
        Fixture { root }
    }

    /// A release directory with the three artifacts of `version`, as downloaded from a run.
    fn release(&self, version: &str, ios_build: &str, apk_build: &str) -> Vec<Artifact> {
        self.release_of(
            &format!("release-{version}"),
            version,
            ios_build,
            apk_build,
            "package",
        )
    }

    fn release_of(
        &self,
        dir: &str,
        version: &str,
        ios_build: &str,
        apk_build: &str,
        content: &str,
    ) -> Vec<Artifact> {
        let dir = self.root.join(dir);
        let bytes = |platform: &str| format!("{platform} {content} {version}").into_bytes();
        write_artifact(
            &dir,
            "windows",
            version,
            &format!("{version}.7"),
            &bytes("windows"),
        );
        write_artifact(&dir, "android", version, apk_build, &bytes("android"));
        write_artifact(&dir, "ios", version, ios_build, &bytes("ios"));
        scan(&[dir]).unwrap()
    }

    /// A release directory with the iOS artifact of `version` alone.
    fn ios_release(&self, version: &str, ios_build: &str) -> Vec<Artifact> {
        let dir = self.root.join(format!("release-ios-{version}"));
        write_artifact(
            &dir,
            "ios",
            version,
            ios_build,
            format!("ios {version}").as_bytes(),
        );
        scan(&[dir]).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn req(prev: Option<&str>, artifacts: Vec<Artifact>, now: &str) -> Request {
    Request {
        prev: prev.map(str::to_owned),
        server: prev.is_none().then(|| SERVER.to_owned()),
        artifacts,
        now: t(now),
        ..Request::default()
    }
}

/// Makes the policy of `request`. The build run of the recorded answers built exactly the
/// packages the request names.
fn made(request: &Request) -> Result<Made, String> {
    make(
        request,
        &test_key_set(),
        &archive(),
        &Settings::new(PathBuf::from(".")),
        &Recorded::good().building(&request.artifacts),
    )
}

fn secret(name: &str) -> SecretKey {
    let key = test_key(name).unwrap();
    SecretKey::from_seed(key.role, key.id, &key.seed())
}

/// Signs a draft with the test keys it needs, as the person at the terminal would; the live file
/// of a declaration is the draft's previous policy.
fn signed(made: &Made) -> String {
    sign_with(made, "root-101").unwrap()
}

fn sign_with(made: &Made, root: &str) -> Result<String, String> {
    let keys = test_key_set();
    let archive = archive();
    let live = made.draft.root_p.as_ref().and(made.draft.prev.as_deref());
    let plan = signing::plan(&made.draft, &keys, &archive, live, made.policy.issued)?;
    let revoked: Vec<u32> = plan
        .new_root
        .as_ref()
        .map(|r| r.revoked.clone())
        .or(plan
            .prev
            .as_ref()
            .and_then(|c| c.root.as_ref())
            .map(|r| r.revoked.clone()))
        .unwrap_or_default();
    let mut secrets = Vec::new();
    for role in plan.needed(&made.draft) {
        secrets.push(match role {
            Role::Root => secret(root),
            Role::Signing if revoked.contains(&1) => secret("signing-2"),
            Role::Signing => secret("signing-1"),
        });
    }
    Ok(signing::sign(&made.draft, &plan, &secrets, &keys, &archive, Some("1.6.0"))?.file)
}

/// seq 1 (design 1-6): 1.6.0 is out everywhere, nothing is enforced yet.
fn first(f: &Fixture) -> Request {
    let mut request = req(None, f.release("1.6.0", IOS_160, APK_160), NOW);
    request.min = vec![(Target::All, v("1.5.0"))];
    request.grace = vec![(Target::All, Grace::Days(0))];
    request.ios_valid = Some(IOS_160.to_owned());
    // the App Store has no version of the app yet (T3)
    request.no_appstore = true;
    request.os_min = vec![
        (Platform::Windows, "10".to_owned()),
        (Platform::Android, "22".to_owned()),
        (Platform::Ios, "13.0".to_owned()),
    ];
    request.contact = Some("설치 파일은 관리자에게 받으세요.".to_owned());
    request
}

/// seq 2: every entry's min to 1.6.0 with 14 days of grace.
fn enforce(file1: &str) -> Request {
    let mut request = req(Some(file1), Vec::new(), "2026-11-08T05:00:00Z");
    request.min = vec![(Target::All, v("1.6.0"))];
    request.grace = vec![(Target::All, Grace::Days(14))];
    request.ios_valid = Some(IOS_160.to_owned());
    // no Google Play entry yet (T2), no App Store version (T3)
    request.no_play = true;
    request.no_appstore = true;
    request
}

/// The envelopes of seq 1 and seq 2.
fn history(f: &Fixture) -> (String, String) {
    let file1 = signed(&made(&first(f)).unwrap());
    let file2 = signed(&made(&enforce(&file1)).unwrap());
    (file1, file2)
}

/// seq 3: 1.6.1 for Windows and Android, installed unattended from two days on.
fn release_161(f: &Fixture, file2: &str) -> Request {
    let mut request = req(
        Some(file2),
        f.release("1.6.1", IOS_161, APK_161),
        "2026-11-09T04:00:00Z",
    );
    request.auto_after = Some(AutoAfter::In(2 * DAY));
    request.notes = vec![(Target::All, "연결 안정성 개선".to_owned())];
    request
}

/// X4 for `file` after `prev`: what `verify --prev` says.
fn check_after(file: &str, prev: Option<&str>, keys: &KeySet, server: &str) -> Result<(), String> {
    x4(file, prev, keys, server, true)
}

fn error(request: &Request) -> String {
    match made(request) {
        Ok(made) => panic!("made a policy: {}", made.draft.p),
        Err(e) => e,
    }
}

#[test]
fn the_first_policy() {
    let f = Fixture::new("make-first");
    let made = made(&first(&f)).unwrap();
    let p = &made.policy;
    assert_eq!(
        (p.epoch, p.seq, p.issued, p.server.as_str()),
        (1, 1, t(NOW), SERVER)
    );
    assert_eq!(
        p.contact_ko.as_deref(),
        Some("설치 파일은 관리자에게 받으세요.")
    );
    assert_eq!(
        p.platforms.keys().copied().collect::<Vec<_>>(),
        [Platform::Windows, Platform::Android, Platform::Ios]
    );
    let windows = &p.platforms[&Platform::Windows];
    assert_eq!((windows.min, windows.latest), (v("1.5.0"), v("1.6.0")));
    // unattended installs start two days after the issue unless the operator says otherwise
    assert_eq!(
        (windows.grace_until, windows.auto_install_after),
        (None, Some(t(NOW) + 2 * DAY))
    );
    assert_eq!(windows.os_min.as_deref(), Some("10"));
    assert_eq!(windows.source.as_deref(), Some("client-1.6.0"));
    let package = windows.package.as_ref().unwrap();
    assert_eq!(package.file, "HJViewer-1.6.0-windows-x86_64.exe");
    assert_eq!(package.size, "windows package 1.6.0".len() as u64);
    assert_eq!(
        package.sha256,
        trust::hex(&trust::sha256(b"windows package 1.6.0"))
    );
    assert_eq!(package.build, "1.6.0.7");
    let android = &p.platforms[&Platform::Android];
    assert_eq!(
        android.package.as_ref().unwrap().file,
        "HJViewer-1.6.0-android-arm64.apk"
    );
    assert_eq!(android.package.as_ref().unwrap().build, APK_160);
    assert!(!android.move_to_play);
    let ios = &p.platforms[&Platform::Ios];
    assert_eq!(ios.testflight_build.as_deref(), Some(IOS_160));
    assert_eq!(
        (ios.latest, ios.os_min.as_deref()),
        (v("1.6.0"), Some("13.0"))
    );

    // the draft: the exact text, for a signing key, nothing attached, what was checked
    let draft = &made.draft;
    assert_eq!(draft.p, p.to_text());
    assert_eq!(
        (draft.signer, &draft.root, &draft.root_p, &draft.prev),
        (Role::Signing, &None, &None, &None)
    );
    assert_eq!(draft.notes.len(), 5);
    assert!(draft.notes[0].contains("windows 1.6.0: 실행 4242 성공"));
    assert!(
        draft.notes[1].contains("windows 1.6.0: 패키지와 release.json이 실행 4242의 산출물과 같음")
    );
    assert!(draft.notes[2].contains("android 1.6.0: 실행 4242 성공"));
    assert!(
        draft.notes[3].contains("android 1.6.0: 패키지와 release.json이 실행 4242의 산출물과 같음")
    );
    assert!(draft.notes[4].contains("TestFlight 빌드(263100200)를 확인함"));
    // the notes are lines `sign` may show, and the draft reads back from its file
    crate::draft::check_notes(&draft.notes).unwrap();
    assert_eq!(Draft::parse(&draft.to_text()).unwrap(), *draft);
    assert!(made.report.iter().any(|l| l.contains("첫 정책")));
    // the first policy has no Google Play entry (no --play-live): the report says what that leaves
    assert!(!p.platforms.contains_key(&Platform::AndroidPlay));
    assert!(made.report.iter().any(|l| l == crate::make::NO_PLAY_ENTRY));
    assert!(made.report.iter().any(|l| l
        .contains("windows: 무인 설치는 기본값대로 게시 2일 뒤(2026-11-10 13:00 한국 시각)부터")));
    let mut without = first(&f);
    without.auto_after = Some(AutoAfter::Off);
    assert_eq!(
        made_policy(&without).platforms[&Platform::Windows].auto_install_after,
        None
    );
    assert!(made
        .report
        .iter()
        .any(|l| l == "서명할 키: 정책은 서명 키 1"));

    // signed, an app with the test keys takes it
    let file = signed(&made);
    let checked = check_envelope(&file, &test_key_set(), SERVER).unwrap();
    assert_eq!(checked.policy, *p);
    assert!(file.len() < 2000, "{} bytes", file.len());
}

#[test]
fn what_the_first_policy_needs() {
    let f = Fixture::new("make-first-needs");
    let good = first(&f);
    let change = |change: &dyn Fn(&mut Request)| {
        let mut request = good.clone();
        change(&mut request);
        error(&request)
    };
    assert!(change(&|r| r.server = None).contains("--server"));
    assert!(
        change(&|r| r.server = Some("https://rd.example.com".to_owned()))
            .contains("서버 이름이 아닙니다")
    );
    assert!(change(&|r| r.min.clear()).contains("새 항목에는 --min이 필요합니다"));
    // in the first policy no entry gets a deadline unless one is asked for (derivation e):
    // nothing is blocked, 1.5.0 being the lowest version ever released
    let mut request = good.clone();
    request.grace.clear();
    let with_default = made(&request).unwrap();
    assert!(with_default
        .policy
        .platforms
        .values()
        .all(|e| e.grace_until.is_none() && e.grace_min.is_none()));
    assert!(!with_default.report.iter().any(|l| l.contains("기본 7일")));
    // asked for, a deadline comes with grace_min 0.0.0
    let mut request = good.clone();
    request.grace = vec![(Target::All, Grace::Days(7))];
    let week = t("2026-11-14T15:00:00Z");
    assert!(made_policy(&request)
        .platforms
        .values()
        .all(|e| e.grace_until == Some(week) && e.grace_min == Some(v("0.0.0"))));
    assert!(change(&|r| r.min = vec![(Target::All, v("1.6.1"))])
        .contains("min(1.6.1)이 latest(1.6.0)보다 높습니다"));
    assert!(
        change(&|r| r.artifacts.clear()).contains("windows: 패키지가 없어 항목을 만들 수 없습니다")
    );
    let nothing = |r: &mut Request| {
        r.artifacts.clear();
        r.os_min.clear();
        r.ios_valid = None;
        r.no_appstore = false;
    };
    assert!(change(&nothing).contains("항목이 하나도 없습니다"));
    // a platform nothing was built for cannot be named
    assert!(
        change(&|r| r.min.push((Target::One(Platform::AndroidPlay), v("1.5.0"))))
            .contains("--play-live")
    );
    assert!(change(&|r| r.os_min.push((Platform::Android, "5.1".to_owned()))).contains("두 번"));
    assert!(change(&|r| r.os_min[1].1 = "5.1".to_owned()).contains("os_min"));
    // without --ios-valid there is no iOS entry, and the report says why
    let mut request = good.clone();
    request.ios_valid = None;
    request.no_appstore = false;
    request
        .os_min
        .retain(|(platform, _)| *platform != Platform::Ios);
    let made = made(&request).unwrap();
    assert!(!made.policy.platforms.contains_key(&Platform::Ios));
    assert!(made
        .report
        .iter()
        .any(|l| l.contains("--ios-valid가 없어 항목을 만들지 않았습니다")));
    // texts
    assert!(
        change(&|r| r.contact = Some("https://rd.example.com/get 에서 받으세요".to_owned()))
            .contains("URL")
    );
    assert!(
        change(&|r| r.notes = vec![(Target::All, "www.example.com 참고".to_owned())])
            .contains("URL")
    );
    let mut request = good.clone();
    request.contact = Some(String::new());
    assert_eq!(made_policy(&request).contact_ko, None);
}

fn made_policy(request: &Request) -> Policy {
    made(request).unwrap().policy
}

#[test]
fn enforcing_a_minimum() {
    let f = Fixture::new("make-enforce");
    let file1 = signed(&made(&first(&f)).unwrap());
    let tools = Recorded::good();
    let made2 = make(
        &enforce(&file1),
        &test_key_set(),
        &archive(),
        &Settings::new(PathBuf::from(".")),
        &tools,
    )
    .unwrap();
    // nothing new was built, so nobody was asked about provenance; --no-appstore looks for an
    // App Store release in the store record (none: the build repository is there, without a
    // record yet)
    assert_eq!(
        *tools.asked.borrow(),
        [
            "gh api -H Accept: application/vnd.github.raw+json repos/yonsei-projecttf/hjviewer-private/contents/store-releases.txt",
            "gh api repos/yonsei-projecttf/hjviewer-private"
        ]
    );
    assert!(made2
        .report
        .iter()
        .any(|l| l.contains("출시 기록(store-releases.txt)이 아직 없습니다(404)")));
    assert_eq!(made2.draft.appstore_release, None);
    let p = &made2.policy;
    assert_eq!((p.epoch, p.seq), (1, 2));
    // 14 days from November 8 in Korea: the deadline is 00:00 on November 22 there
    let deadline = t("2026-11-21T15:00:00Z");
    for (platform, entry) in &p.platforms {
        assert_eq!(entry.min, v("1.6.0"), "{platform}");
        assert_eq!(entry.grace_until, Some(deadline), "{platform}");
    }
    // everything else is as it was
    let before = check_envelope(&file1, &test_key_set(), SERVER)
        .unwrap()
        .policy;
    assert_eq!(
        p.platforms[&Platform::Windows].package,
        before.platforms[&Platform::Windows].package
    );
    assert_eq!(p.contact_ko, before.contact_ko);
    assert_eq!(made2.draft.prev.as_deref(), Some(file1.as_str()));
    assert!(made2
        .report
        .iter()
        .any(|l| l
            .contains("windows: 최소 1.5.0 → 1.6.0, 유예 끝 없음 → 2026-11-22 00:00 한국 시각")));
    let change = |change: &dyn Fn(&mut Request)| {
        let mut request = enforce(&file1);
        change(&mut request);
        error(&request)
    };
    // without a grace option a raised min gets the default of 7 days, and the report says so
    let mut request = enforce(&file1);
    request.grace.clear();
    let with_default = made(&request).unwrap();
    let week = t("2026-11-14T15:00:00Z");
    assert!(with_default
        .policy
        .platforms
        .values()
        .all(|e| e.grace_until == Some(week)));
    let said = "windows: 유예 옵션이 없어 기본 7일을 둡니다(2026-11-15 00:00 한국 시각까지)";
    assert!(
        with_default.report.iter().any(|l| l.contains(said)),
        "{:?}",
        with_default.report
    );
    assert_eq!(with_default.draft.signer, Role::Signing);
    // 0 days is said, never assumed; and the report says what that means
    let mut request = enforce(&file1);
    request.grace = vec![(Target::All, Grace::Days(0))];
    let at_once = made(&request).unwrap();
    assert!(at_once
        .policy
        .platforms
        .values()
        .all(|e| e.grace_until.is_none()));
    for platform in ["windows", "android", "ios"] {
        let said = format!("{platform}: 유예가 없습니다. 서버에 넣는 즉시 1.6.0 미만을 막습니다");
        assert!(
            at_once.report.contains(&said),
            "{platform}: {:?}",
            at_once.report
        );
    }
    assert!(!made2.report.iter().any(|l| l.contains("유예가 없습니다")));
    // a higher iOS min needs the confirmed TestFlight build
    assert!(change(&|r| r.ios_valid = None).contains("ios: min을 올리려면 --ios-valid"));
    assert!(
        change(&|r| r.grace = vec![(Target::All, Grace::Until(t(NOW)))])
            .contains("이미 지났습니다")
    );
    assert!(change(&|r| r.min = vec![(Target::All, v("1.6.1"))]).contains("높습니다"));
    // a deadline given as a time
    let mut request = enforce(&file1);
    request.grace = vec![(Target::All, Grace::Until(t("2026-11-30T15:00:00Z")))];
    assert_eq!(
        made_policy(&request).platforms[&Platform::Ios].grace_until,
        Some(t("2026-11-30T15:00:00Z"))
    );
    // one entry only; a grace option for all applies to the entries whose min changes
    let mut request = enforce(&file1);
    request.min = vec![(Target::One(Platform::Windows), v("1.6.0"))];
    request.ios_valid = None;
    // what --no-play and --no-appstore would pass over does not happen: they are refused
    assert!(error(&request).contains("--no-appstore를 줬지만 ios의 latest와 min을 올리지 않습니다"));
    request.no_appstore = false;
    assert!(error(&request).contains("--no-play를 줬지만"));
    request.no_play = false;
    let p = made_policy(&request);
    assert_eq!(p.platforms[&Platform::Windows].grace_until, Some(deadline));
    assert_eq!(
        (
            p.platforms[&Platform::Ios].min,
            p.platforms[&Platform::Ios].grace_until
        ),
        (v("1.5.0"), None)
    );
    // options that would do nothing are mistakes
    let mut request = enforce(&file1);
    request.min.clear();
    request.no_appstore = false;
    assert!(error(&request).contains("min이 바뀌는 항목이 없습니다"));
    let mut request = enforce(&file1);
    request.notes = vec![(Target::All, "새 소식".to_owned())];
    assert!(error(&request).contains("latest가 바뀌는 항목이 없습니다"));
    // signed, a device holding seq 1 and a fresh install take it (X4)
    let file2 = signed(&made2);
    assert!(check_after(&file2, Some(&file1), &test_key_set(), SERVER).is_ok());
}

#[test]
fn a_new_release() {
    let f = Fixture::new("make-release");
    let (_, file2) = history(&f);
    let made3 = made(&release_161(&f, &file2)).unwrap();
    let p = &made3.policy;
    assert_eq!((p.epoch, p.seq), (1, 3));
    let now = t("2026-11-09T04:00:00Z");
    let windows = &p.platforms[&Platform::Windows];
    assert_eq!((windows.min, windows.latest), (v("1.6.0"), v("1.6.1")));
    assert_eq!(windows.auto_install_after, Some(now + 2 * DAY));
    assert_eq!(windows.notes_ko.as_deref(), Some("연결 안정성 개선"));
    assert_eq!(windows.source.as_deref(), Some("client-1.6.1"));
    assert_eq!(
        windows.package.as_ref().unwrap().file,
        "HJViewer-1.6.1-windows-x86_64.exe"
    );
    assert_eq!(
        windows.package.as_ref().unwrap().sha256,
        trust::hex(&trust::sha256(b"windows package 1.6.1"))
    );
    // the deadline of the min that did not change stays
    assert_eq!(windows.grace_until, Some(t("2026-11-21T15:00:00Z")));
    let android = &p.platforms[&Platform::Android];
    assert_eq!(
        (android.latest, android.auto_install_after),
        (v("1.6.1"), None)
    );
    assert_eq!(android.notes_ko.as_deref(), Some("연결 안정성 개선"));
    // iOS waits for its TestFlight build to be confirmed
    let ios = &p.platforms[&Platform::Ios];
    assert_eq!(
        (ios.latest, ios.testflight_build.as_deref(), &ios.notes_ko),
        (v("1.6.0"), Some(IOS_160), &None)
    );
    assert!(made3.report.iter().any(|l| l
        .contains("ios: 1.6.1 산출물이 있지만 --ios-valid가 없어 latest(1.6.0)를 그대로 둡니다")));
    assert_eq!(made3.draft.notes.len(), 4);

    // with the build confirmed, iOS follows, saying that the App Store has no version yet (T3)
    let mut request = release_161(&f, &file2);
    request.ios_valid = Some(IOS_161.to_owned());
    assert!(error(&request).contains("--appstore-live <App Store에 나온 버전>이나 --no-appstore"));
    request.no_appstore = true;
    let ios = made_policy(&request).platforms[&Platform::Ios].clone();
    assert_eq!(
        (ios.latest, ios.testflight_build.as_deref()),
        (v("1.6.1"), Some(IOS_161))
    );
    assert_eq!(
        (ios.source.as_deref(), ios.notes_ko.as_deref()),
        (Some("client-1.6.1"), Some("연결 안정성 개선"))
    );

    // without the option a new release starts two days after its issue as well, and says so
    let mut request = release_161(&f, &file2);
    request.auto_after = None;
    let by_default = made(&request).unwrap();
    assert_eq!(
        by_default.policy.platforms[&Platform::Windows].auto_install_after,
        Some(now + 2 * DAY)
    );
    assert!(by_default
        .report
        .iter()
        .any(|l| l.contains("--auto-after windows=none")));
    assert!(!made3.report.iter().any(|l| l.contains("기본값대로")));
    let file3 = signed(&made3);

    // the next policy changes nothing about the packages: no unattended install date is lost
    let mut request = req_after(&file3, "2026-11-10T04:00:00Z");
    request.contact = Some("내선 1234".to_owned());
    let p4 = made_policy(&request);
    assert_eq!(p4.seq, 4);
    assert_eq!(
        p4.platforms[&Platform::Windows],
        p.platforms[&Platform::Windows]
    );
    // the emergency stop: remove the date, with a signing key
    let mut request = req_after(&file3, "2026-11-10T04:00:00Z");
    request.auto_after = Some(AutoAfter::Off);
    let stopped = made(&request).unwrap();
    assert_eq!(
        stopped.policy.platforms[&Platform::Windows].auto_install_after,
        None
    );
    assert_eq!(stopped.draft.signer, Role::Signing);
    // a date in the past would install on every ring at once
    let mut request = req_after(&file3, "2026-11-10T04:00:00Z");
    request.auto_after = Some(AutoAfter::At(t("2026-11-10T03:59:59Z")));
    assert!(error(&request).contains("이미 지났습니다"));
    request.auto_after = Some(AutoAfter::At(t("2026-11-12T00:00:00Z")));
    assert_eq!(
        made_policy(&request).platforms[&Platform::Windows].auto_install_after,
        Some(t("2026-11-12T00:00:00Z"))
    );
    // held entries take no option
    request.hold = vec![Platform::Windows];
    assert!(error(&request).contains("바꿀 windows 항목이 없습니다"));
    for change in [
        (|r: &mut Request| r.min = vec![(Target::One(Platform::Windows), v("1.6.1"))])
            as fn(&mut Request),
        |r| r.grace = vec![(Target::One(Platform::Windows), Grace::Days(3))],
        |r| r.notes = vec![(Target::One(Platform::Windows), "새 소식".to_owned())],
        |r| r.os_min = vec![(Platform::Windows, "11".to_owned())],
        |r| r.latest = vec![(Platform::Windows, v("1.6.1"))],
    ] {
        let mut request = req_after(&file3, "2026-11-10T04:00:00Z");
        request.hold = vec![Platform::Windows];
        change(&mut request);
        let err = error(&request);
        assert!(
            err.contains("windows: --hold나 --drop을 준 항목에 다른 옵션을 함께 줄 수 없습니다"),
            "{err}"
        );
    }
}

fn req_after(file: &str, now: &str) -> Request {
    req(Some(file), Vec::new(), now)
}

#[test]
fn the_same_version_is_the_same_build() {
    let f = Fixture::new("make-same");
    let (_, file2) = history(&f);
    let file3 = signed(&made(&release_161(&f, &file2)).unwrap());
    // 1.6.1 again, built again: other bytes under the same version
    let rebuilt = f.release_of("rebuilt", "1.6.1", IOS_161, APK_161, "rebuilt package");
    let request = req(Some(&file3), rebuilt, "2026-11-10T04:00:00Z");
    assert!(error(&request).contains("같은 버전을 다른 빌드로 낼 수 없습니다"));
    // the very same build again changes nothing
    let same = f.release("1.6.1", IOS_161, APK_161);
    let p = made_policy(&req(Some(&file3), same, "2026-11-10T04:00:00Z"));
    assert_eq!(p.platforms[&Platform::Windows].latest, v("1.6.1"));
    // an older release directory by mistake
    let older = f.release("1.6.0", IOS_160, APK_160);
    let mut request = req(Some(&file3), older, "2026-11-10T04:00:00Z");
    assert!(
        error(&request).contains("릴리스 디렉터리의 버전(1.6.0)이 지금 latest(1.6.1)보다 낮습니다")
    );
    // ... is fine once the entries are held
    request.hold = vec![Platform::Windows, Platform::Android];
    assert_eq!(
        made_policy(&request).platforms[&Platform::Android].latest,
        v("1.6.1")
    );
    // a latest without its package
    let mut request = req_after(&file3, "2026-11-10T04:00:00Z");
    request.latest = vec![(Platform::Windows, v("1.6.2"))];
    assert!(error(&request).contains("latest 1.6.2의 패키지가 릴리스 디렉터리에 없습니다"));
}

#[test]
fn provenance_is_required_for_a_new_package() {
    let f = Fixture::new("make-provenance");
    let (_, file2) = history(&f);
    let request = release_161(&f, &file2);
    let keys = test_key_set();
    let settings = Settings::new(PathBuf::from("."));
    let refused =
        |tools: Recorded| make(&request, &keys, &archive(), &settings, &tools).unwrap_err();
    // a failed run, another workflow, another commit: no draft
    let failed = run_json(|r| r["conclusion"] = "failure".into());
    assert!(refused(Recorded {
        run: failed,
        ..Recorded::good()
    })
    .starts_with("출처 확인 실패(windows 1.6.1)"));
    let other = run_json(|r| r["path"] = ".github/workflows/other.yml".into());
    assert!(refused(Recorded {
        run: other,
        ..Recorded::good()
    })
    .contains("다른 워크플로"));
    let tag = Some("3333333333333333333333333333333333333333".to_owned());
    assert!(refused(Recorded {
        tag,
        ..Recorded::good()
    })
    .contains("태그 client-1.6.1의 커밋"));
    assert!(refused(Recorded {
        tag: None,
        ..Recorded::good()
    })
    .contains("태그가 없습니다: client-1.6.1"));
    // both new packages are checked, each against its own tag
    let tools = Recorded::good().building(&request.artifacts);
    let made = make(&request, &keys, &archive(), &settings, &tools).unwrap();
    let asked = tools.asked.borrow();
    assert_eq!(
        asked
            .iter()
            .filter(|a| a.starts_with("git rev-parse"))
            .count(),
        2
    );
    assert_eq!(
        asked
            .iter()
            .filter(|a| a.contains("actions/runs/4242"))
            .count(),
        2
    );
    assert!(asked.iter().all(|a| !a.contains("client-1.6.0")));
    // and each is a file of the run's artifacts, which are downloaded once for both
    assert_eq!(
        asked
            .iter()
            .filter(|a| a.starts_with("gh run download 4242 "))
            .count(),
        1
    );
    let sha = |platform: Platform| {
        let entry = &made.policy.platforms[&platform];
        entry.package.as_ref().unwrap().sha256.clone()
    };
    assert_eq!(made.draft.notes.len(), 4, "{:?}", made.draft.notes);
    assert_eq!(
        made.draft.notes[1],
        format!(
            "windows 1.6.1: 패키지와 release.json이 실행 4242의 산출물과 같음(패키지 sha256 {})",
            sha(Platform::Windows)
        )
    );
    assert_eq!(
        made.draft.notes[3],
        format!(
            "android 1.6.1: 패키지와 release.json이 실행 4242의 산출물과 같음(패키지 sha256 {})",
            sha(Platform::Android)
        )
    );
    // A release directory whose packages are not that run's: other bytes, with a release.json
    // that agrees with them and names the good run. No draft.
    let err = refused(Recorded::good());
    assert!(
        err.starts_with("출처 확인 실패(windows 1.6.1)") && err.contains("산출물에 없습니다"),
        "{err}"
    );
    let windows: Vec<Artifact> = request
        .artifacts
        .iter()
        .filter(|a| a.info.platform == Built::Windows)
        .cloned()
        .collect();
    assert_eq!(windows.len(), 1);
    let err = refused(Recorded::good().building(&windows));
    assert!(
        err.starts_with("출처 확인 실패(android 1.6.1)") && err.contains("산출물에 없습니다"),
        "{err}"
    );
    // the artifacts of the run are gone (kept for 7 days)
    let err = refused(Recorded {
        download: Err("no valid artifacts found to download".to_owned()),
        ..Recorded::good()
    });
    assert!(err.contains("산출물을 받지 못했습니다"), "{err}");
}

#[test]
fn os_minimum_binds_min() {
    let f = Fixture::new("make-os");
    let (_, file2) = history(&f);
    let file3 = signed(&made(&release_161(&f, &file2)).unwrap());
    let now = "2026-12-01T04:00:00Z";
    // 1.6.2 needs Android 7.0 (API 24): devices on the old OS can run 1.6.1 at most
    let release = |f: &Fixture| f.release("1.6.2", "263350100", "263352100");
    let mut request = req(Some(&file3), release(&f), now);
    request.os_min = vec![(Platform::Android, "24".to_owned())];
    let p = made_policy(&request);
    assert_eq!(
        p.platforms[&Platform::Android].os_min.as_deref(),
        Some("24")
    );
    assert_eq!(p.platforms[&Platform::Android].min, v("1.6.0"));
    // min up to the last version of the old OS is fine, above it is refused
    request.min = vec![(Target::One(Platform::Android), v("1.6.1"))];
    request.grace = vec![(Target::All, Grace::Days(7))];
    // no Google Play entry in this history (T2)
    request.no_play = true;
    assert_eq!(
        made_policy(&request).platforms[&Platform::Android].min,
        v("1.6.1")
    );
    request.min = vec![(Target::One(Platform::Android), v("1.6.2"))];
    let err = error(&request);
    assert!(
        err.contains("android: os_min을 올리는 정책(22 → 24)에서는 min을"),
        "{err}"
    );
    assert!(err.contains("마지막 버전 1.6.1 이하"), "{err}");
    // the same os_min, or a lower one, binds nothing
    request.os_min = vec![(Platform::Android, "22".to_owned())];
    assert_eq!(
        made_policy(&request).platforms[&Platform::Android].min,
        v("1.6.2")
    );
    // iOS versions compare by their fields
    let mut request = req(Some(&file3), release(&f), now);
    request.ios_valid = Some("263350100".to_owned());
    request.no_appstore = true;
    request.os_min = vec![(Platform::Ios, "15.0".to_owned())];
    request.min = vec![(Target::One(Platform::Ios), v("1.6.2"))];
    request.grace = vec![(Target::All, Grace::Days(7))];
    assert!(error(&request).contains("ios: os_min을 올리는 정책(13.0 → 15.0)"));
    request.os_min = vec![(Platform::Ios, "13".to_owned())];
    assert_eq!(
        made_policy(&request).platforms[&Platform::Ios].min,
        v("1.6.2")
    );
}

#[test]
fn testflight_builds() {
    let f = Fixture::new("make-ios");
    let (_, file2) = history(&f);
    let now = "2026-11-10T04:00:00Z";
    let with = |build: &str, latest: Option<&str>, now: &str, artifacts: Vec<Artifact>| {
        let mut request = req(Some(&file2), artifacts, now);
        request.hold = vec![Platform::Windows, Platform::Android];
        request.ios_valid = Some(build.to_owned());
        request.latest = latest.map(|l| (Platform::Ios, v(l))).into_iter().collect();
        // a new build of the same version raises nothing (T3)
        request.no_appstore = latest != Some("1.6.0");
        made(&request)
    };
    // the build of the release directory: version and build number agree
    let p = with(IOS_161, None, now, f.release("1.6.1", IOS_161, APK_161))
        .unwrap()
        .policy;
    assert_eq!(p.platforms[&Platform::Ios].latest, v("1.6.1"));
    // without the release directory the operator names the version
    assert!(with(IOS_161, None, now, Vec::new())
        .unwrap_err()
        .contains("버전을 알 수 없습니다"));
    let p = with(IOS_161, Some("1.6.1"), now, Vec::new())
        .unwrap()
        .policy;
    assert_eq!(
        p.platforms[&Platform::Ios].testflight_build.as_deref(),
        Some(IOS_161)
    );
    // the version and the build must agree with what is known
    let err = with(
        IOS_161,
        Some("1.6.2"),
        now,
        f.release("1.6.1", IOS_161, APK_161),
    )
    .unwrap_err();
    assert!(
        err.contains("--latest의 버전(1.6.2)이 릴리스 디렉터리에 있는 빌드"),
        "{err}"
    );
    let err = with(IOS_160, Some("1.6.1"), now, Vec::new()).unwrap_err();
    assert!(
        err.contains("지금 정책에서 빌드 263100200의 버전은 1.6.0입니다"),
        "{err}"
    );
    // the same version uploaded again (to renew the 90 days): a newer build number
    let p = with("263130500", Some("1.6.0"), now, Vec::new())
        .unwrap()
        .policy;
    assert_eq!(
        p.platforms[&Platform::Ios].testflight_build.as_deref(),
        Some("263130500")
    );
    assert_eq!(
        p.platforms[&Platform::Ios].source.as_deref(),
        Some("client-1.6.0")
    );
    assert!(with("263090500", Some("1.6.0"), now, Vec::new())
        .unwrap_err()
        .contains("새롭지 않습니다"));
    assert!(with("263090500", Some("1.6.1"), now, Vec::new())
        .unwrap_err()
        .contains("새롭지 않습니다"));
    // 80 days after its upload a build is too old to name, and a build cannot be from the future
    let day_79 = "2027-01-26T02:48:00Z";
    let day_80 = "2027-01-27T02:49:00Z";
    assert!(with(IOS_161, Some("1.6.1"), day_79, Vec::new()).is_ok());
    let err = with(IOS_161, Some("1.6.1"), day_80, Vec::new()).unwrap_err();
    assert!(err.contains("올린 지 80일"), "{err}");
    assert!(with("263200000", Some("1.6.1"), now, Vec::new())
        .unwrap_err()
        .contains("미래"));
    assert!(with("26312024", Some("1.6.1"), now, Vec::new())
        .unwrap_err()
        .contains("--ios-valid"));
    // an aging build is pointed out when a policy is made for something else
    let mut request = req_after(&file2, "2027-01-21T02:00:00Z");
    request.contact = Some("내선 1234".to_owned());
    let made = made(&request).unwrap();
    assert!(
        made.report
            .iter()
            .any(|l| l.starts_with("경고: ios TestFlight 빌드를 올린 지 76일")),
        "{:?}",
        made.report
    );
}

#[test]
fn google_play() {
    let f = Fixture::new("make-play");
    let (_, file2) = history(&f);
    let now = "2026-11-10T04:00:00Z";
    let code_160: u64 = APK_160.parse().unwrap();
    let code_161: u64 = APK_161.parse().unwrap();
    // the Play entry appears once the operator saw the release on the configured track (alpha)
    let add = |code: u64, artifacts: Vec<Artifact>| {
        let mut request = req(Some(&file2), artifacts, now);
        request.play_live = Some(code);
        request
            .min
            .push((Target::One(Platform::AndroidPlay), v("1.6.0")));
        made(&request)
    };
    let made3 = add(code_160, f.release("1.6.0", IOS_160, APK_160)).unwrap();
    let play = &made3.policy.platforms[&Platform::AndroidPlay];
    assert_eq!(
        (play.min, play.latest, play.version_code),
        (v("1.6.0"), v("1.6.0"), Some(code_160))
    );
    assert_eq!(play.source.as_deref(), Some("client-1.6.0"));
    assert_eq!(play.package, None);
    assert!(made3
        .draft
        .notes
        .iter()
        .any(|n| n.contains("Play alpha 트랙의 versionCode(263102200) 게시를 확인함")));
    assert!(!made3.report.iter().any(|l| l == crate::make::NO_PLAY_ENTRY));
    // a later build on the track is fine, an earlier one is not; the version needs its build
    assert!(add(code_160 + 5, f.release("1.6.0", IOS_160, APK_160)).is_ok());
    let err = add(code_160 - 1, f.release("1.6.0", IOS_160, APK_160)).unwrap_err();
    assert!(
        err.contains("versionCode(263102199)가 1.6.0의 versionCode(263102200)보다 낮습니다"),
        "{err}"
    );
    assert!(add(code_160, Vec::new())
        .unwrap_err()
        .contains("버전을 알 수 없습니다"));
    let file3 = signed(&made3);

    // 1.6.1 reaches Windows and sideloaded Android first; Play follows when its track has it
    let later = "2026-11-12T04:00:00Z";
    let mut request = req(Some(&file3), f.release("1.6.1", IOS_161, APK_161), later);
    let p = made_policy(&request);
    assert_eq!(p.platforms[&Platform::Android].latest, v("1.6.1"));
    assert_eq!(p.platforms[&Platform::AndroidPlay].latest, v("1.6.0"));
    request.play_live = Some(code_161);
    let p = made_policy(&request);
    assert_eq!(p.platforms[&Platform::AndroidPlay].latest, v("1.6.1"));
    assert_eq!(
        p.platforms[&Platform::AndroidPlay].version_code,
        Some(code_161)
    );
    request.play_live = Some(code_160);
    assert!(error(&request).contains("1.6.1의 versionCode(263122249)보다 낮습니다"));

    // min and latest of the Play entry move only with the confirmation
    let mut request = req_after(&file3, later);
    request.min = vec![(Target::One(Platform::AndroidPlay), v("1.6.0"))];
    assert!(made(&request).is_ok());
    let mut request = req(Some(&file3), f.release("1.6.1", IOS_161, APK_161), later);
    request.latest = vec![(Platform::AndroidPlay, v("1.6.1"))];
    assert!(error(&request).contains("android_play: latest를 바꾸려면 --play-live"));
    request.play_live = Some(code_161);
    request.min = vec![(Target::One(Platform::AndroidPlay), v("1.6.1"))];
    request.grace = vec![(Target::All, Grace::Days(7))];
    let made4 = made(&request).unwrap();
    // the min it has, before file4 is signed (the archive takes one policy at a position)
    let mut request = req_after(&file3, later);
    request.latest = vec![(Platform::AndroidPlay, v("1.6.0"))];
    request.min = vec![(Target::One(Platform::AndroidPlay), v("1.6.0"))];
    assert!(made(&request).is_ok(), "the min it has");
    let file4 = signed(&made4);
    let mut request = req_after(&file4, "2026-11-13T04:00:00Z");
    request.min = vec![(Target::One(Platform::AndroidPlay), v("1.6.1"))];
    assert!(made(&request).is_ok());
    // a version nobody built for Play, a versionCode below the recorded one, one out of range
    let mut request = req_after(&file4, "2026-11-13T04:00:00Z");
    request.play_live = Some(code_161);
    request.latest = vec![(Platform::AndroidPlay, v("1.6.2"))];
    assert!(error(&request).contains("Android 1.6.2 산출물이 있어야"));
    request.latest.clear();
    request.play_live = Some(code_161 - 1);
    assert!(error(&request).contains("보다 낮습니다"));
    request.play_live = Some(MAX_VERSION_CODE + 1);
    assert!(error(&request).contains("범위 밖"));
    // the sideloaded app's hint to move to Play
    let mut request = req_after(&file4, "2026-11-13T04:00:00Z");
    request.move_to_play = Some(true);
    let moved = made(&request).unwrap();
    assert!(moved.policy.platforms[&Platform::Android].move_to_play);
    assert!(moved
        .report
        .iter()
        .any(|l| l.contains("android: Play로 옮기기 안내 켬")));
    let file5 = signed(&moved);
    let mut request = req_after(&file5, "2026-11-14T04:00:00Z");
    request.move_to_play = Some(false);
    assert!(!made_policy(&request).platforms[&Platform::Android].move_to_play);
    // A higher min for Play installs without the operator's look at the track, once the
    // entry's latest is 1.6.1 already. Another branch from file3: a fresh archive with the same
    // history (the signatures are deterministic, the files the same).
    let g = Fixture::new("make-play-branch");
    let (_, again) = history(&g);
    assert_eq!(again, file2);
    let mut request = req(Some(&file2), g.release("1.6.0", IOS_160, APK_160), now);
    request.play_live = Some(code_160);
    request
        .min
        .push((Target::One(Platform::AndroidPlay), v("1.6.0")));
    assert_eq!(signed(&made(&request).unwrap()), file3);
    let play_at_161 = {
        let mut request = req(Some(&file3), g.release("1.6.1", IOS_161, APK_161), later);
        request.play_live = Some(code_161);
        signed(&made(&request).unwrap())
    };
    let mut request = req_after(&play_at_161, "2026-11-13T04:00:00Z");
    request.min = vec![(Target::One(Platform::AndroidPlay), v("1.6.1"))];
    request.grace = vec![(Target::All, Grace::Days(7))];
    let err = error(&request);
    assert!(
        err.contains("android_play: min을 올리려면 --play-live"),
        "{err}"
    );
    request.play_live = Some(code_161);
    assert_eq!(
        made_policy(&request).platforms[&Platform::AndroidPlay].min,
        v("1.6.1")
    );
}

#[test]
fn the_envelope_must_fit() {
    let f = Fixture::new("make-size");
    let (_, file2) = history(&f);
    let mut request = req_after(&file2, "2026-11-10T04:00:00Z");
    let long = "가".repeat(crate::policy::MAX_NOTES_CHARS);
    request.notes = Platform::ALL
        .into_iter()
        .filter(|p| *p != Platform::AndroidPlay)
        .map(|p| (Target::One(p), long.clone()))
        .collect();
    let err = error(&request);
    assert!(err.contains("앱은 16384바이트까지만 읽습니다"), "{err}");
    request.notes.truncate(2);
    let made = made(&request).unwrap();
    assert!(made.report.iter().any(|l| l.starts_with("봉투 크기: 약 ")));
    let file = signed(&made);
    assert!(file.len() <= MAX_ENVELOPE_BYTES);
    // the size named in the report is the size of the file
    let size: usize = made
        .report
        .iter()
        .find_map(|l| {
            l.strip_prefix("봉투 크기: 약 ")?
                .split("바이트")
                .next()?
                .parse()
                .ok()
        })
        .unwrap();
    // (an upper bound: the key numbers are not known before signing)
    let actual = file.len();
    assert!(size >= actual && size - actual <= 8, "{size} for {actual}");
    assert_eq!(
        envelope_size("{}", None),
        "{\"v\":1,\"k\":65535,\"p\":\"{}\",\"s\":\"\"}\n".len() + 88
    );
}

#[test]
fn relaxing_needs_a_declaration() {
    let f = Fixture::new("make-relax");
    let (_, file2) = history(&f);
    let file3 = signed(&made(&release_161(&f, &file2)).unwrap());
    let now = "2026-11-10T04:00:00Z";
    let deadline = t("2026-11-21T15:00:00Z");
    type Change = Box<dyn Fn(&mut Request)>;
    let cases: Vec<(&str, Change)> = vec![
        (
            "ios: 최소 버전을 내림",
            Box::new(|r| r.min = vec![(Target::One(Platform::Ios), v("1.5.0"))]),
        ),
        (
            "ios: 유예 끝을 늦추거나 새로 둠",
            Box::new(move |r| {
                r.grace = vec![(Target::One(Platform::Ios), Grace::Until(deadline + 1))]
            }),
        ),
        (
            "windows: latest를 내림",
            Box::new(|r| r.latest = vec![(Platform::Windows, v("1.6.0"))]),
        ),
    ];
    for (what, change) in &cases {
        let mut request = req_after(&file3, now);
        if what.starts_with("windows") {
            // lowering latest needs the older package again
            request.artifacts = f.release("1.6.0", IOS_160, APK_160);
            request.hold = vec![Platform::Android];
        }
        change(&mut request);
        let platform = what.split(':').next().unwrap();
        let err = error(&request);
        assert!(
            err.contains("X1") && err.contains(platform),
            "{what}: {err}"
        );
        // a declaration, made only from the live file (X10)
        request.relax = true;
        assert!(error(&request).contains("--live"), "{what}");
        request.live = Some(file3.clone());
        let made = made(&request).unwrap();
        assert_eq!(made.draft.signer, Role::Signing, "{what}");
        assert_eq!((made.policy.epoch, made.policy.seq), (2, 1), "{what}");
        let decl = RootDecl::parse(made.draft.root_p.as_ref().unwrap()).unwrap();
        assert_eq!(decl.base, made.policy.requirements());
        assert!(
            made.report
                .iter()
                .any(|l| l.contains(&format!("완화: {what}"))),
            "{what}: {:?}",
            made.report
        );
        assert!(made
            .report
            .iter()
            .any(|l| l.contains(&format!("--relaxes {platform}"))));
        // the signer is asked, at the clean machine
        let plan = signing::plan(
            &made.draft,
            &test_key_set(),
            &archive(),
            Some(&file3),
            made.policy.issued,
        )
        .unwrap();
        assert!(
            plan.dangers.iter().any(|d| d.contains(what)),
            "{what}: {:?}",
            plan.dangers
        );
        assert!(plan.dangers[0].contains("깨끗한 컴퓨터"));
        assert_eq!(
            plan.relaxed.iter().map(|p| p.name()).collect::<Vec<_>>(),
            [platform]
        );
    }
    // signed: a declaration by root key 101 and a policy by a signing key; a device holding
    // file3 takes it, the declaration first and then the policy against its base
    let mut request = req_after(&file3, now);
    request.min = vec![(Target::One(Platform::Ios), v("1.5.0"))];
    request.relax = true;
    request.live = Some(file3.clone());
    let file = signed(&made(&request).unwrap());
    let checked = check_envelope(&file, &test_key_set(), SERVER).unwrap();
    assert_eq!(checked.role, Role::Signing);
    assert_eq!(checked.envelope.root.as_ref().unwrap().k, 101);
    assert_eq!(checked.root.unwrap().epoch, 2);
    assert!(check_after(&file, Some(&file3), &test_key_set(), SERVER).is_ok());
    assert_eq!(archive().max_epoch().unwrap(), 2);
    assert!(archive().declaration(2).unwrap().is_some());
    // the next declaration is made from the new live file and opens epoch 3
    let mut request = req_after(&file, now);
    request.new_epoch = true;
    request.live = Some(file.clone());
    assert_eq!(made(&request).unwrap().policy.epoch, 3);
    // a lowered min loses its old deadline (made from the live file `file`)
    let file3 = file;
    let mut request = req_after(&file3, now);
    request.relax = true;
    request.live = Some(file3.clone());
    request.min = vec![(Target::All, v("1.5.0"))];
    let p = made_policy(&request);
    assert!(p
        .platforms
        .values()
        .all(|e| e.min == v("1.5.0") && e.grace_until.is_none() && e.grace_min.is_none()));
    // a grace option for a lowered minimum makes no sense
    request.grace = vec![(Target::One(Platform::Windows), Grace::Days(3))];
    assert!(error(&request).contains("내리는 항목에는 유예 옵션"));
    // tightening is no relaxation: an earlier deadline, by a signing key
    let mut request = req_after(&file3, now);
    request.grace = vec![(Target::One(Platform::Windows), Grace::Until(deadline - DAY))];
    let made_earlier = made(&request).unwrap();
    assert_eq!(made_earlier.draft.signer, Role::Signing);
    assert_eq!(made_earlier.draft.root_p, None);
    let windows = &made_earlier.policy.platforms[&Platform::Windows];
    assert_eq!(
        (windows.grace_until, windows.grace_min),
        (Some(deadline - DAY), Some(v("1.5.0")))
    );
    // a new epoch with nothing relaxed (a re-base) still makes a declaration
    let mut request = req_after(&file3, now);
    request.new_epoch = true;
    request.live = Some(file3.clone());
    let rebase = made(&request).unwrap();
    assert_eq!(
        (rebase.policy.epoch, rebase.draft.signer),
        (3, Role::Signing)
    );
    assert!(rebase.report.iter().any(|l| l.contains("--relaxes 없이")));
    // dropping what is not there, holding and dropping the same entry
    let mut request = req_after(&file3, now);
    request.live = Some(file3.clone());
    request.drop = vec![Platform::AndroidPlay];
    assert!(error(&request).contains("뺄 항목이 지금 정책에 없습니다"));
    request.drop = vec![Platform::Ios];
    request.hold = vec![Platform::Ios];
    assert!(error(&request).contains("두 번"));
    // dropping an entry is a relaxation by itself
    request.hold.clear();
    let dropped = made(&request).unwrap();
    assert!(!dropped.policy.platforms.contains_key(&Platform::Ios));
    assert!(dropped
        .report
        .iter()
        .any(|l| l.contains("완화: ios: 항목을 뺌")));
    assert!(dropped.report.iter().any(|l| l.contains("--relaxes ios")));
}

#[test]
fn revocation_and_epochs() {
    let f = Fixture::new("make-root");
    let (_, file2) = history(&f);
    let now = "2026-11-10T04:00:00Z";
    // key 1 leaked: a declaration revokes it and opens epoch 2 (runbook 5-8)
    let mut request = req_after(&file2, now);
    request.revoke = vec![1];
    assert!(error(&request).contains("--live"));
    request.live = Some(file2.clone());
    let made3 = made(&request).unwrap();
    assert_eq!((made3.policy.epoch, made3.policy.seq), (2, 1));
    assert_eq!(made3.draft.signer, Role::Signing);
    assert_eq!(made3.draft.root, None);
    let decl = RootDecl::parse(made3.draft.root_p.as_ref().unwrap()).unwrap();
    assert_eq!(
        (decl.epoch, decl.revoked.as_slice(), decl.issued),
        (2, &[1][..], t(now))
    );
    assert_eq!(decl.base, made3.policy.requirements());
    assert!(made3
        .report
        .iter()
        .any(|l| l.contains("새 루트 선언: 세대 2 · 폐기한 서명 키 [1]")));
    assert!(made3.report.iter().any(|l| l.contains("정책은 서명 키 2")));
    let plan = signing::plan(
        &made3.draft,
        &test_key_set(),
        &archive(),
        Some(&file2),
        t(now),
    )
    .unwrap();
    assert!(plan
        .dangers
        .iter()
        .any(|d| d.contains("서명 키(번호 1)를 폐기합니다")));
    assert!(plan
        .dangers
        .iter()
        .any(|d| d.contains("세대(epoch)를 올립니다(1 → 2)")));
    assert!(plan
        .summary
        .iter()
        .any(|l| l.contains("폐기와 함께 나가는 패키지")));
    let file3 = signed(&made3);
    let checked = check_envelope(&file3, &test_key_set(), SERVER).unwrap();
    assert_eq!(
        (
            checked.envelope.policy.k,
            checked.root.as_ref().unwrap().revoked.as_slice()
        ),
        (2, &[1][..])
    );
    assert!(check_after(&file3, Some(&file2), &test_key_set(), SERVER).is_ok());

    // from now on every envelope carries the declaration, as it was signed
    let made4 = made(&req_after(&file3, "2026-11-11T04:00:00Z")).unwrap();
    assert_eq!((made4.policy.epoch, made4.policy.seq), (2, 2));
    assert_eq!(made4.draft.root, Envelope::parse(&file3).unwrap().root);
    assert_eq!(made4.draft.root_p, None);
    let file4 = signed(&made4);
    assert!(check_after(&file4, Some(&file3), &test_key_set(), SERVER).is_ok());

    // revocations add up, each with a new epoch; a root key is never revoked; nothing twice
    let mut request = req_after(&file4, "2026-11-12T04:00:00Z");
    request.live = Some(file4.clone());
    request.revoke = vec![7];
    let made5 = made(&request).unwrap();
    let decl = RootDecl::parse(made5.draft.root_p.as_ref().unwrap()).unwrap();
    assert_eq!((decl.epoch, decl.revoked.as_slice()), (3, &[1, 7][..]));
    assert_eq!((made5.policy.epoch, made5.policy.seq), (3, 1));
    assert_eq!(made5.draft.root, None);
    assert_eq!(Draft::parse(&made5.draft.to_text()).unwrap(), made5.draft);
    request.revoke = vec![101];
    assert!(error(&request).contains("루트 키는 폐기하지 않습니다"));
    request.revoke = vec![150];
    assert!(error(&request).contains("X2"));
    request.revoke = vec![1];
    assert!(error(&request).contains("이미 폐기한 키"));
    request.revoke = vec![7, 7];
    assert!(error(&request).contains("두 번"));
    // with every signing key revoked, a root key signs the policy (X12, X9)
    request.revoke = vec![2];
    let err = error(&request);
    assert!(
        err.contains("X12") && err.contains("--root-signs-policy"),
        "{err}"
    );
    request.root_signs_policy = true;
    let by_root = made(&request).unwrap();
    assert_eq!(by_root.draft.signer, Role::Root);
    // ... and only then (X9)
    request.revoke = vec![7];
    assert!(error(&request).contains("X9"));

    // X10: the live file is --prev byte for byte, and a file of the archive
    let mut request = req_after(&file4, "2026-11-12T04:00:00Z");
    request.new_epoch = true;
    request.live = Some(file3.clone());
    let err = error(&request);
    assert!(err.contains("X10") && err.contains("--prev"), "{err}");
    // a new epoch without a revocation
    let mut request = req_after(&file4, "2026-11-12T04:00:00Z");
    request.live = Some(file4.clone());
    request.new_epoch = true;
    let m = made(&request).unwrap();
    let decl = RootDecl::parse(m.draft.root_p.as_ref().unwrap()).unwrap();
    assert_eq!(
        (
            m.policy.epoch,
            m.policy.seq,
            decl.epoch,
            decl.revoked.as_slice()
        ),
        (3, 1, 3, &[1][..])
    );
    // --seq has no place in a new epoch
    request.seq = Some(4);
    assert!(error(&request).contains("--seq"));
}

#[test]
fn seq_steps() {
    let f = Fixture::new("make-seq");
    let (_, file2) = history(&f);
    let now = "2026-11-10T04:00:00Z";
    let with_seq = |seq: u64| {
        let mut request = req_after(&file2, now);
        request.seq = Some(seq);
        made(&request)
    };
    assert_eq!(with_seq(3).unwrap().policy.seq, 3);
    let jump = with_seq(20).unwrap();
    let plan = signing::plan(&jump.draft, &test_key_set(), &archive(), None, t(now)).unwrap();
    assert!(plan
        .dangers
        .iter()
        .any(|l| l.contains("seq가 한 번에 뜁니다")));
    let small = with_seq(12).unwrap();
    let plan = signing::plan(&small.draft, &test_key_set(), &archive(), None, t(now)).unwrap();
    assert!(!plan.dangers.iter().any(|l| l.contains("뜁니다")));
    assert!(with_seq(2).unwrap_err().contains("높지 않습니다"));
    assert!(with_seq(1).unwrap_err().contains("높지 않습니다"));
    // a fresh install at the key file's floor (1:0) takes up to seq 1000 (X4 b)
    assert!(with_seq(1000).is_ok());
    let err = with_seq(1001).unwrap_err();
    assert!(err.contains("X4") && err.contains("Jump"), "{err}");
    assert!(with_seq(1003).unwrap_err().contains("1000"));
    // a root key's policy has no special rights: no further either
    let mut request = req_after(&file2, now);
    request.seq = Some(5000);
    request.root_signs_policy = true;
    assert!(error(&request).contains("1000"));
}

#[test]
fn the_previous_policy_is_checked() {
    let f = Fixture::new("make-prev");
    let (file1, file2) = history(&f);
    let now = "2026-11-10T04:00:00Z";
    // a changed byte, another server, a clock behind the previous policy
    let tampered = file2.replacen("\\\"seq\\\":2", "\\\"seq\\\":9", 1);
    assert_ne!(tampered, file2);
    assert!(error(&req_after(&tampered, now))
        .contains("이전 정책(--prev): 정책의 서명이 맞지 않습니다"));
    assert!(error(&req_after("not json", now)).contains("이전 정책(--prev)"));
    let mut request = req_after(&file2, now);
    request.server = Some("hjv.test".to_owned());
    assert!(error(&request).contains("--server"));
    assert!(error(&req_after(&file2, "2026-11-08T04:59:59Z")).contains("시계"));
    // X11: --prev is the archive's last file that is not voided
    let err = error(&req_after(&file1, now));
    assert!(
        err.contains("X11") && err.contains("policy-e1-s2.json"),
        "{err}"
    );
    // a build whose floor is above the next position does not take it (X4 b)
    let mut keys = test_key_set();
    keys.floor = Floor { epoch: 1, seq: 4 };
    let err = make(
        &req_after(&file2, now),
        &keys,
        &archive(),
        &Settings::new(PathBuf::from(".")),
        &Recorded::good(),
    )
    .unwrap_err();
    assert!(err.contains("X4") && err.contains("Rollback"), "{err}");
    keys.floor = Floor { epoch: 1, seq: 3 };
    assert!(make(
        &req_after(&file2, now),
        &keys,
        &archive(),
        &Settings::new(PathBuf::from(".")),
        &Recorded::good(),
    )
    .is_ok());
    // the first envelope is still what it was: the second one names it byte for byte
    assert_eq!(
        Draft::parse(&made(&req_after(&file2, now)).unwrap().draft.to_text())
            .unwrap()
            .prev
            .as_deref(),
        Some(file2.as_str())
    );
}

#[test]
fn a_first_policy_above_the_floor_and_the_archive() {
    let f = Fixture::new("make-first-floor");
    // the first policy of a key file whose floor is not zero starts above the floor
    let mut keys = test_key_set();
    keys.floor = Floor { epoch: 1, seq: 3 };
    let request = first(&f);
    let made = make(
        &request,
        &keys,
        &archive(),
        &Settings::new(PathBuf::from(".")),
        &Recorded::good().building(&request.artifacts),
    )
    .unwrap();
    assert_eq!(made.policy.seq, 4);
    assert!(made.report.iter().any(|l| l.contains("하한이 1:3입니다")));
    // once the archive holds a policy, a first policy is refused (X11)
    let _ = history(&f);
    let err = error(&first(&f));
    assert!(err.contains("X11") && err.contains("--prev"), "{err}");
}

#[test]
fn a_new_entry_below_the_present_minimums() {
    let f = Fixture::new("make-low-entry");
    // every entry at min 1.6.0 until November 22; no Google Play entry yet
    let (_, file2) = history(&f);
    let code: u64 = APK_160.parse().unwrap();
    let request_for = |min: &str, relax: bool| {
        let artifacts = f.release("1.6.0", IOS_160, APK_160);
        let mut request = req(Some(&file2), artifacts, "2026-11-10T04:00:00Z");
        request.play_live = Some(code);
        request.min = vec![(Target::One(Platform::AndroidPlay), v(min))];
        request.relax = relax;
        if relax {
            request.live = Some(file2.clone());
        }
        request
    };
    let add = |min: &str, relax: bool| made(&request_for(min, relax));
    // at android's level it copies android's deadline pair (derivation f), by a signing key
    let level = add("1.6.0", false).unwrap();
    assert_eq!(level.draft.signer, Role::Signing);
    let play = &level.policy.platforms[&Platform::AndroidPlay];
    let android = &level.policy.platforms[&Platform::Android];
    assert_eq!(
        (play.grace_until, play.grace_min),
        (android.grace_until, android.grace_min)
    );
    // a deadline of its own at android's level is refused: another deadline is a relaxation or
    // a new raise
    let mut request = request_for("1.6.0", false);
    request.grace = vec![(Target::One(Platform::AndroidPlay), Grace::Days(3))];
    assert!(error(&request).contains("유예 옵션을 주지 마세요"));
    // Below it the PC's rule for Android controllers would go down (problem (나)): a
    // relaxation of the Android pair, and hbbs's MIN would follow (X8)
    let err = add("1.5.0", false).unwrap_err();
    assert!(err.contains("X1"), "{err}");
    let made = add("1.5.0", true).unwrap();
    assert_eq!(made.draft.signer, Role::Signing);
    assert!(made.draft.root_p.is_some());
    let play = &made.policy.platforms[&Platform::AndroidPlay];
    assert_eq!((play.grace_until, play.grace_min), (None, None));
    // ... and a grace option for it is refused, not dropped (as for a lowered minimum)
    for (grace, grace_min) in [
        (
            vec![(Target::One(Platform::AndroidPlay), Grace::Days(3))],
            None,
        ),
        (vec![(Target::All, Grace::Days(3))], None),
        (Vec::new(), Some(v("1.4.0"))),
    ] {
        let mut request = request_for("1.5.0", true);
        request.grace = grace;
        request.grace_min = grace_min
            .map(|gm| (Platform::AndroidPlay, gm))
            .into_iter()
            .collect();
        let err = error(&request);
        assert!(err.contains("유예 옵션을 줄 수 없습니다"), "{err}");
    }
    assert!(
        made.report
            .iter()
            .any(|l| l.contains("완화: android_play: 다른 Android 항목보다 덜 막는 새 항목")),
        "{:?}",
        made.report
    );
    assert!(made
        .report
        .iter()
        .any(|l| l.contains("hbbs의 MIN이 내려감")));
    assert!(made
        .report
        .iter()
        .any(|l| l.contains("--relaxes android_play")));
    let file = signed(&made);
    assert!(check_after(&file, Some(&file2), &test_key_set(), SERVER).is_ok());
}

#[test]
fn raises_keep_the_level_in_force() {
    let f = Fixture::new("make-raise");
    // every min at 1.6.0 until November 22 (Korean time), 1.5.0 until then
    let (_, file2) = history(&f);
    let file3 = signed(&made(&release_161(&f, &file2)).unwrap());
    let deadline = t("2026-11-21T15:00:00Z");
    let raise = |now: &str, grace: Option<Grace>, grace_min: Option<&str>| {
        let mut request = req_after(&file3, now);
        request.min = vec![(Target::One(Platform::Windows), v("1.6.1"))];
        request.grace = grace
            .map(|g| (Target::One(Platform::Windows), g))
            .into_iter()
            .collect();
        request.grace_min = grace_min
            .map(|g| (Platform::Windows, v(g)))
            .into_iter()
            .collect();
        made(&request)
    };
    let pair = |m: &Made| {
        let w = &m.policy.platforms[&Platform::Windows];
        (w.min, w.grace_until, w.grace_min)
    };
    let soon = "2026-11-10T04:00:00Z";
    // c: a raise while one is pending gets the pending deadline and its grace_min
    let same = raise(soon, None, None).unwrap();
    assert_eq!(pair(&same), (v("1.6.1"), Some(deadline), Some(v("1.5.0"))));
    assert_eq!(same.draft.signer, Role::Signing);
    let plan = signing::plan(&same.draft, &test_key_set(), &archive(), None, t(soon)).unwrap();
    assert!(
        plan.dangers
            .iter()
            .any(|d| d.contains("1.6.0 이상 1.6.1 미만은 지금 유예와 같은")),
        "{:?}",
        plan.dangers
    );
    // c with an earlier deadline keeps the grace_min, and the signer is asked
    let earlier = raise(soon, Some(Grace::Until(deadline - DAY)), None).unwrap();
    assert_eq!(pair(&earlier).2, Some(v("1.5.0")));
    let plan = signing::plan(&earlier.draft, &test_key_set(), &archive(), None, t(soon)).unwrap();
    assert!(
        plan.dangers
            .iter()
            .any(|d| d.contains("유예 끝을 앞당깁니다") && d.contains("1.5.0 이상 1.6.1 미만")),
        "{:?}",
        plan.dangers
    );
    // c with a later deadline: the pending minimum is enforced at once (asked)
    let later = raise(soon, Some(Grace::Days(14)), None).unwrap();
    assert_eq!(
        pair(&later),
        (
            v("1.6.1"),
            Some(t("2026-11-23T15:00:00Z")),
            Some(v("1.6.0"))
        )
    );
    let plan = signing::plan(&later.draft, &test_key_set(), &archive(), None, t(soon)).unwrap();
    assert!(
        plan.dangers
            .iter()
            .any(|d| d.contains("유예 중이던 1.5.0 이상 1.6.0 미만이")),
        "{:?}",
        plan.dangers
    );
    // ... and a later deadline that lets the old minimum go is a relaxation (problem (가))
    let err = raise(soon, Some(Grace::Days(14)), Some("1.5.0")).unwrap_err();
    assert!(err.contains("X1"), "{err}");
    // b: after the deadline the minimum in force is the grace_min, 7 days by default
    let after = "2026-11-25T04:00:00Z";
    let b = raise(after, None, None).unwrap();
    assert_eq!(
        pair(&b),
        (
            v("1.6.1"),
            Some(t("2026-12-01T15:00:00Z")),
            Some(v("1.6.0"))
        )
    );
    assert!(b.report.iter().any(|l| l.contains("기본 7일")));
    // without a grace period: no pair, said as a danger
    let none = raise(after, Some(Grace::Days(0)), None).unwrap();
    assert_eq!(pair(&none), (v("1.6.1"), None, None));
    let plan = signing::plan(&none.draft, &test_key_set(), &archive(), None, t(after)).unwrap();
    assert!(plan.dangers.iter().any(|d| d.contains("유예가 없습니다")));
    // --grace-min below the level in force asks less: a relaxation
    let err = raise(after, None, Some("1.5.0")).unwrap_err();
    assert!(err.contains("X1"), "{err}");
    // --grace-min for an entry without a pair is a mistake
    let err = raise(after, Some(Grace::Days(0)), Some("1.6.0")).unwrap_err();
    assert!(err.contains("--grace-min"), "{err}");
}

#[test]
fn after_a_relaxing_declaration() {
    let f = Fixture::new("make-after-relax");
    // seq 2: every min at 1.6.0 until November 22 (Korean time)
    let (_, file2) = history(&f);
    // the update path is down: a declaration of epoch 2 lowers every min (runbook 5-12)
    let mut request = req_after(&file2, "2026-11-10T04:00:00Z");
    request.relax = true;
    request.live = Some(file2.clone());
    request.min = vec![(Target::All, v("1.5.0"))];
    let file3 = signed(&made(&request).unwrap());
    assert_eq!(
        check_envelope(&file3, &test_key_set(), SERVER)
            .unwrap()
            .role,
        Role::Signing
    );
    // afterwards a signing key raises them again, with a grace period of any length: a device
    // that missed the relaxation takes the declaration first and compares with its base
    // (problem (다))
    for now in ["2026-11-12T04:00:00Z", "2026-11-20T04:00:00Z"] {
        let mut request = req_after(&file3, now);
        request.min = vec![(Target::All, v("1.6.0"))];
        request.ios_valid = Some(IOS_160.to_owned());
        request.no_play = true;
        request.no_appstore = true;
        let made4 = made(&request).unwrap();
        assert_eq!(made4.draft.signer, Role::Signing);
        assert_eq!(made4.draft.root_p, None);
        let ios = &made4.policy.platforms[&Platform::Ios];
        assert_eq!(ios.grace_min, Some(v("1.5.0")));
        if now.starts_with("2026-11-20") {
            let file4 = signed(&made4);
            assert!(check_after(&file4, Some(&file2), &test_key_set(), SERVER).is_ok());
            assert!(check_after(&file4, Some(&file3), &test_key_set(), SERVER).is_ok());
        }
    }
}

#[test]
fn derivations_a_to_f() {
    let now = t("2026-11-10T04:00:00Z");
    let pending = t("2026-11-21T15:00:00Z");
    let r = |min: &str, until: Option<i64>, gm: Option<&str>| Requirement {
        min: v(min),
        latest: v("1.6.1"),
        grace_until: until,
        grace_min: gm.map(v),
    };
    let week = time::kst_midnight_after(now, DEFAULT_GRACE_DAYS);
    let w = Platform::Windows;
    let o = r("1.6.0", Some(pending), Some("1.5.0"));
    // a: the minimum stays
    assert_eq!(
        derive_existing(w, &o, v("1.6.0"), now, GraceAsk::Default, false, None),
        Ok(Some((pending, v("1.5.0"))))
    );
    assert_eq!(
        derive_existing(w, &o, v("1.6.0"), now, GraceAsk::None, false, None),
        Ok(None)
    );
    assert_eq!(
        derive_existing(
            w,
            &o,
            v("1.6.0"),
            now,
            GraceAsk::At(pending - DAY),
            false,
            None
        ),
        Ok(Some((pending - DAY, v("1.5.0"))))
    );
    assert!(derive_existing(
        w,
        &o,
        v("1.6.0"),
        now,
        GraceAsk::At(pending + DAY),
        false,
        None
    )
    .unwrap_err()
    .contains("X1"));
    assert_eq!(
        derive_existing(
            w,
            &o,
            v("1.6.0"),
            now,
            GraceAsk::At(pending + DAY),
            true,
            None
        ),
        Ok(Some((pending + DAY, v("1.5.0"))))
    );
    let plain = r("1.6.0", None, None);
    assert!(
        derive_existing(w, &plain, v("1.6.0"), now, GraceAsk::At(week), true, None)
            .unwrap_err()
            .contains("--grace-min")
    );
    assert_eq!(
        derive_existing(
            w,
            &plain,
            v("1.6.0"),
            now,
            GraceAsk::At(week),
            true,
            Some(v("1.5.0"))
        ),
        Ok(Some((week, v("1.5.0"))))
    );
    // b: no raise pending: the minimum in force until the deadline
    assert_eq!(
        derive_existing(w, &plain, v("1.6.1"), now, GraceAsk::Default, false, None),
        Ok(Some((week, v("1.6.0"))))
    );
    let passed = r("1.6.0", Some(now - DAY), Some("1.5.0"));
    assert_eq!(
        derive_existing(w, &passed, v("1.6.1"), now, GraceAsk::Default, false, None),
        Ok(Some((week, v("1.6.0"))))
    );
    // c: a raise pending
    assert_eq!(
        derive_existing(w, &o, v("1.6.1"), now, GraceAsk::Default, false, None),
        Ok(Some((pending, v("1.5.0"))))
    );
    assert_eq!(
        derive_existing(
            w,
            &o,
            v("1.6.1"),
            now,
            GraceAsk::At(pending + DAY),
            false,
            None
        ),
        Ok(Some((pending + DAY, v("1.6.0"))))
    );
    assert_eq!(
        derive_existing(w, &o, v("1.6.1"), now, GraceAsk::None, false, None),
        Ok(None)
    );
    // the pending deadline itself, asked for: the same deadline, so the same grace_min
    assert_eq!(
        derive_existing(w, &o, v("1.6.1"), now, GraceAsk::At(pending), false, None),
        Ok(Some((pending, v("1.5.0"))))
    );
    // d: lowered
    assert!(
        derive_existing(w, &o, v("1.5.0"), now, GraceAsk::Default, false, None)
            .unwrap_err()
            .contains("X1")
    );
    assert_eq!(
        derive_existing(w, &o, v("1.5.0"), now, GraceAsk::Default, true, None),
        Ok(None)
    );
    // e: a new entry: 7 days and 0.0.0, none in a first policy unless asked
    let prev: BTreeMap<Platform, Requirement> = [(Platform::Windows, o)].into_iter().collect();
    let zero = v("0.0.0");
    assert_eq!(
        derive_new(
            Platform::Ios,
            v("1.6.0"),
            now,
            GraceAsk::Default,
            Some(&prev),
            false,
            None
        ),
        Ok(Some((week, zero)))
    );
    assert_eq!(
        derive_new(
            Platform::Ios,
            v("1.6.0"),
            now,
            GraceAsk::Default,
            None,
            false,
            None
        ),
        Ok(None)
    );
    assert_eq!(
        derive_new(
            Platform::Ios,
            v("1.6.0"),
            now,
            GraceAsk::At(week),
            None,
            false,
            None
        ),
        Ok(Some((week, zero)))
    );
    assert_eq!(
        derive_new(
            Platform::Ios,
            zero,
            now,
            GraceAsk::Default,
            Some(&prev),
            false,
            None
        ),
        Ok(None)
    );
    // f: the sibling of the Android pair in --prev
    let android: BTreeMap<Platform, Requirement> = [(Platform::Android, o)].into_iter().collect();
    let p = Platform::AndroidPlay;
    assert_eq!(
        derive_new(
            p,
            v("1.6.0"),
            now,
            GraceAsk::Default,
            Some(&android),
            false,
            None
        ),
        Ok(Some((pending, v("1.5.0"))))
    );
    assert_eq!(
        derive_new(
            p,
            v("1.6.1"),
            now,
            GraceAsk::Default,
            Some(&android),
            false,
            None
        ),
        Ok(Some((pending, v("1.5.0"))))
    );
    assert!(derive_new(
        p,
        v("1.5.0"),
        now,
        GraceAsk::Default,
        Some(&android),
        false,
        None
    )
    .unwrap_err()
    .contains("X1"));
    // below the sibling with a declaration: no pair, and no grace option (as in d)
    assert_eq!(
        derive_new(
            p,
            v("1.5.0"),
            now,
            GraceAsk::Default,
            Some(&android),
            true,
            None
        ),
        Ok(None)
    );
    for (grace, grace_min) in [
        (GraceAsk::At(week), None),
        (GraceAsk::None, None),
        (GraceAsk::Default, Some(v("1.4.0"))),
    ] {
        assert!(
            derive_new(p, v("1.5.0"), now, grace, Some(&android), true, grace_min)
                .unwrap_err()
                .contains("유예 옵션을 줄 수 없습니다"),
            "{grace:?} {grace_min:?}"
        );
    }
    // INV-14: a raise covers the old entry, and blocks nothing new before its own deadline
    // unless it is c with a later deadline
    for o in [o, plain, passed] {
        for grace in [
            GraceAsk::Default,
            GraceAsk::None,
            GraceAsk::At(pending - DAY),
            GraceAsk::At(pending + 5 * DAY),
        ] {
            let pair = derive_existing(w, &o, v("1.6.1"), now, grace, false, None).unwrap();
            let n = Requirement {
                min: v("1.6.1"),
                latest: v("1.6.1"),
                grace_until: pair.map(|p| p.0),
                grace_min: pair.map(|p| p.1),
            };
            assert!(trust::covers(&n, &o), "{o:?} {grace:?}");
        }
    }
}

#[test]
fn the_package_rule_and_withdrawal() {
    let f = Fixture::new("make-withdraw");
    let (_, file2) = history(&f);
    let file3 = signed(&made(&release_161(&f, &file2)).unwrap());
    let now = "2026-11-10T04:00:00Z";
    // the emergency stop of runbook 5-6: no more unattended installs, and no forced installs
    // of the bad version either
    let mut request = req_after(&file3, now);
    request.auto_after = Some(AutoAfter::Off);
    let stopped = made(&request).unwrap();
    // the minimum is pending (November 22): PCs below it would still install by force
    assert!(
        stopped
            .report
            .iter()
            .any(|l| l.contains("--withdraw-package windows")),
        "{:?}",
        stopped.report
    );
    request.withdraw = vec![Platform::Windows];
    let withdrawn = made(&request).unwrap();
    let windows = &withdrawn.policy.platforms[&Platform::Windows];
    assert_eq!(
        (windows.package.as_ref(), windows.auto_install_after),
        (None, None)
    );
    assert_eq!(windows.latest, v("1.6.1"));
    assert_eq!(withdrawn.draft.signer, Role::Signing);
    assert_eq!(withdrawn.draft.root_p, None);
    assert!(withdrawn.report.iter().any(|l| l.contains("거둬들입니다")));
    let plan = signing::plan(&withdrawn.draft, &test_key_set(), &archive(), None, t(now)).unwrap();
    assert!(plan
        .dangers
        .iter()
        .any(|d| d.contains("패키지를 거둬들입니다")));
    // withdrawing with a new latest, or for an entry without packages
    let mut request = req(
        Some(&file3),
        f.release("1.6.2", "263150100", "263152100"),
        now,
    );
    request.withdraw = vec![Platform::Windows];
    assert!(error(&request).contains("새 latest"));
    let mut request = req_after(&file3, now);
    request.withdraw = vec![Platform::Ios];
    assert!(error(&request).contains("windows와 android"));
    // an Android withdrawal keeps the operator's word about the hint to move to Play
    let mut request = req_after(&file3, now);
    request.withdraw = vec![Platform::Android];
    request.move_to_play = Some(true);
    let moved = made(&request).unwrap();
    let android = &moved.policy.platforms[&Platform::Android];
    assert_eq!(
        (android.package.as_ref(), android.move_to_play),
        (None, true)
    );
    assert!(
        moved
            .report
            .iter()
            .any(|l| l == "android: 패키지를 거둬들임, Play로 옮기기 안내 켬"),
        "{:?}",
        moved.report
    );
    let file4 = signed(&withdrawn);
    assert!(check_after(&file4, Some(&file3), &test_key_set(), SERVER).is_ok());
    // the entry stays without a package while latest stays
    let next = made(&req_after(&file4, "2026-11-11T04:00:00Z")).unwrap();
    assert_eq!(next.policy.platforms[&Platform::Windows].package, None);
    // the same version never gets its package back
    let again = f.release("1.6.1", IOS_161, APK_161);
    let request = req(Some(&file4), again, "2026-11-11T04:00:00Z");
    assert!(error(&request).contains("거둬들였습니다"));
    // a higher version brings a package again
    let fixed = f.release("1.6.2", "263150100", "263152100");
    let mut request = req(Some(&file4), fixed, "2026-11-12T04:00:00Z");
    request.hold = vec![Platform::Android];
    let p = made_policy(&request);
    assert_eq!(p.platforms[&Platform::Windows].latest, v("1.6.2"));
    assert!(p.platforms[&Platform::Windows].package.is_some());
    // withdrawing what is not there
    let mut request = req_after(&file4, "2026-11-11T04:00:00Z");
    request.withdraw = vec![Platform::Windows];
    assert!(error(&request).contains("거둬들일 패키지가 없습니다"));
    // an unattended install needs a package
    let mut request = req_after(&file4, "2026-11-11T04:00:00Z");
    request.auto_after = Some(AutoAfter::In(DAY));
    assert!(error(&request).contains("패키지가 없는"));
}

#[test]
fn signers_serve_every_recorded_build() {
    let f = Fixture::new("make-records");
    let (_, file2) = history(&f);
    let now = "2026-11-10T04:00:00Z";
    // the first sign recorded the key file's key set
    let records = archive().keysets().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].versions, ["1.6.0"]);
    // a build that pins only signing key 2 (and the same root keys) is recorded: key 1 no
    // longer serves every build, key 2 does
    let keys = test_key_set();
    let only_2 = KeySet::from_values(&keys.signing[1].to_string(), &keys.root_line, "1:2").unwrap();
    archive()
        .record_keyset(&only_2, SERVER, Some("1.6.1"))
        .unwrap();
    let made3 = made(&req_after(&file2, now)).unwrap();
    assert!(made3.report.iter().any(|l| l.contains("정책은 서명 키 2")));
    let plan = signing::plan(&made3.draft, &keys, &archive(), None, t(now)).unwrap();
    let err = signing::sign(
        &made3.draft,
        &plan,
        &[secret("signing-1")],
        &keys,
        &archive(),
        None,
    )
    .unwrap_err();
    assert!(err.contains("X12"), "{err}");
    assert!(signing::sign(
        &made3.draft,
        &plan,
        &[secret("signing-2")],
        &keys,
        &archive(),
        None
    )
    .is_ok());
    // a build that pins neither signing key: a root key signs the policies (X9)
    let other = SigningKey::from_bytes(&[9; 32]).verifying_key();
    let none = KeySet::from_values(
        &format!(
            "3:{}",
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, other.to_bytes())
        ),
        &keys.root_line,
        "1:3",
    )
    .unwrap();
    archive()
        .record_keyset(&none, SERVER, Some("1.6.2"))
        .unwrap();
    let file3 = archive().highest().unwrap().unwrap().file;
    let err = error(&req_after(&file3, now));
    assert!(
        err.contains("X12") && err.contains("--root-signs-policy"),
        "{err}"
    );
    let mut request = req_after(&file3, now);
    request.root_signs_policy = true;
    let by_root = made(&request).unwrap();
    assert_eq!(by_root.draft.signer, Role::Root);
    let plan = signing::plan(&by_root.draft, &keys, &archive(), None, t(now)).unwrap();
    assert!(plan
        .dangers
        .iter()
        .any(|d| d.contains("루트 키로 서명합니다")));
    assert!(plan.dangers[0].contains("깨끗한 컴퓨터"));
    let signed = signing::sign(
        &by_root.draft,
        &plan,
        &[secret("root-102")],
        &keys,
        &archive(),
        None,
    )
    .unwrap();
    assert_eq!(Envelope::parse(&signed.file).unwrap().policy.k, 102);
    // a key file whose key set is not recorded does not sign (floor --live records a build's)
    let unrecorded =
        KeySet::from_values(&keys.signing_line, &keys.roots[0].to_string(), "1:0").unwrap();
    let err = signing::sign(
        &by_root.draft,
        &plan,
        &[secret("root-101")],
        &unrecorded,
        &archive(),
        None,
    )
    .unwrap_err();
    assert!(err.contains("floor --live"), "{err}");
}

/// X8, the tool's own rule: a new entry below the lowest minimum of `--prev` would let hbbs's one
/// MIN go down. Only a declaration does that.
#[test]
fn a_new_entry_below_hbbs_min_needs_a_declaration() {
    let f = Fixture::new("make-x8");
    // Windows and Android only, both at min 1.6.0 from seq 2 on
    let mut request = first(&f);
    request.ios_valid = None;
    request.no_appstore = false;
    request.os_min.retain(|(p, _)| *p != Platform::Ios);
    let file1 = signed(&made(&request).unwrap());
    let mut request = enforce(&file1);
    request.ios_valid = None;
    request.no_appstore = false;
    let file2 = signed(&made(&request).unwrap());
    let ios_at = |min: &str| {
        let mut request = req(
            Some(&file2),
            f.release("1.6.0", IOS_160, APK_160),
            "2026-11-08T06:00:00Z",
        );
        request.ios_valid = Some(IOS_160.to_owned());
        request.no_appstore = true;
        request.min = vec![(Target::One(Platform::Ios), v(min))];
        request
    };
    // a new iOS entry at the lowest minimum is a signing key's policy
    assert_eq!(made(&ios_at("1.6.0")).unwrap().draft.signer, Role::Signing);
    // below it: X8, though no entry of --prev asks less
    let err = error(&ios_at("1.5.0"));
    assert!(
        err.starts_with("X8: ")
            && err.contains(
                "ios: 새 항목의 최소 버전(1.5.0)이 지금 가장 낮은 최소 버전(1.6.0)보다 낮아 hbbs의 MIN이 내려감"
            ),
        "{err}"
    );
    // with a declaration it is made, and said
    let mut request = ios_at("1.5.0");
    request.relax = true;
    request.live = Some(file2.clone());
    let relaxed = made(&request).unwrap();
    assert!(relaxed.draft.root_p.is_some());
    assert!(
        relaxed
            .report
            .iter()
            .any(|l| l.starts_with("완화(도구 규칙): ios: 새 항목의 최소 버전(1.5.0)")),
        "{:?}",
        relaxed.report
    );
}

/// A new declaration and the archive: its epoch is above every epoch the archive holds (also one
/// signed and never published), its live file is the operator's own (X10), and its root key is one
/// every recorded build pins (X12).
#[test]
fn a_declaration_follows_the_archive() {
    let f = Fixture::new("make-decl-archive");
    let (_, file2) = history(&f);
    let now = "2026-11-10T04:00:00Z";
    let new_epoch = |file: &str| {
        let mut request = req_after(file, now);
        request.new_epoch = true;
        request.live = Some(file.to_owned());
        request
    };
    // a live file signed elsewhere (another text at 1:2, by a key the build trusts): no
    // declaration is made from it
    let mut stolen = Policy::parse(&Envelope::parse(&file2).unwrap().policy.p).unwrap();
    stolen.contact_ko = Some("다른 곳에서 서명된 정책".to_owned());
    let key = test_key("signing-1").unwrap();
    let elsewhere = Envelope {
        policy: Part::sign(
            &SigningKey::from_bytes(&key.seed()),
            1,
            crate::envelope::POLICY_DOMAIN,
            &stolen.to_text(),
        ),
        root: None,
    }
    .to_file();
    assert!(check_envelope(&elsewhere, &test_key_set(), SERVER).is_ok());
    let err = error(&new_epoch(&elsewhere));
    assert!(err.contains("X10") && err.contains("보관소"), "{err}");
    // a declaration of epoch 2 signed and never published: the next one from the live file opens
    // epoch 3, not 2 again
    let made3 = made(&new_epoch(&file2)).unwrap();
    assert_eq!(made3.policy.epoch, 2);
    signed(&made3);
    assert_eq!(archive().max_epoch().unwrap(), 2);
    let again = made(&new_epoch(&file2)).unwrap();
    assert_eq!((again.policy.epoch, again.policy.seq), (3, 1));
    let decl = RootDecl::parse(again.draft.root_p.as_ref().unwrap()).unwrap();
    assert_eq!(decl.epoch, 3);
    // a recorded build that pins root keys 102 and 103 only: root key 102 signs the declaration
    let keys = test_key_set();
    let pin = |name: &str| test_key(name).unwrap().public().to_string();
    let roots_102_103 = KeySet::from_values(
        &keys.signing_line,
        &format!("{},{}", pin("root-102"), pin("root-103")),
        "1:2",
    )
    .unwrap();
    archive()
        .record_keyset(&roots_102_103, SERVER, Some("1.6.1"))
        .unwrap();
    let by_102 = made(&new_epoch(&file2)).unwrap();
    assert!(
        by_102
            .report
            .iter()
            .any(|l| l.contains("루트 선언은 루트 키 102")),
        "{:?}",
        by_102.report
    );
    assert!(sign_with(&by_102, "root-101").unwrap_err().contains("X12"));
    assert_eq!(
        Envelope::parse(&sign_with(&by_102, "root-102").unwrap())
            .unwrap()
            .root
            .unwrap()
            .k,
        102
    );
}

/// The policy of `request` with the stores' release record `record` in the build repository.
fn made_with_record(request: &Request, record: &str) -> Result<Made, String> {
    let tools = Recorded {
        store_record: Some(record.to_owned()),
        ..Recorded::good()
    }
    .building(&request.artifacts);
    make(
        request,
        &test_key_set(),
        &archive(),
        &Settings::new(PathBuf::from(".")),
        &tools,
    )
}

/// iOS alone moves (the other entries are held): TestFlight build `build`, `latest` named.
fn ios_only(file: &str, now: &str, build: Option<&str>, latest: Option<&str>) -> Request {
    let mut request = req_after(file, now);
    request.hold = vec![Platform::Windows, Platform::Android];
    request.ios_valid = build.map(str::to_owned);
    request.latest = latest.map(|l| (Platform::Ios, v(l))).into_iter().collect();
    request
}

/// T3 (auto-deploy design 3-7): the App Store version in the iOS entry and what it gates.
#[test]
fn the_app_store_gate() {
    let f = Fixture::new("make-appstore");
    let (_, file2) = history(&f);
    let now = "2026-11-10T04:00:00Z";
    // no App Store version yet: a raise names it or says there is none
    let mut request = ios_only(&file2, now, Some(IOS_161), Some("1.6.1"));
    request.min = vec![(Target::All, v("1.6.1"))];
    request.grace = vec![(Target::All, Grace::Days(7))];
    let err = error(&request);
    assert!(
        err.contains("--appstore-live <App Store에 나온 버전>이나 --no-appstore"),
        "{err}"
    );
    request.min.clear();
    request.grace.clear();
    assert!(error(&request).contains("--no-appstore"));
    request.appstore_live = Some(v("1.6.1"));
    request.no_appstore = true;
    assert!(error(&request).contains("함께 줄 수 없습니다"));
    request.no_appstore = false;
    // the two name the iOS entry: not with --hold ios
    let mut held = req_after(&file2, now);
    held.hold = vec![Platform::Ios];
    held.no_appstore = true;
    assert!(error(&held).contains("--hold나 --drop"));
    // the App Store's version must hold latest
    request.appstore_live = Some(v("1.6.0"));
    let err = error(&request);
    assert!(
        err.contains("latest(1.6.1)가 App Store에 나온 버전(1.6.0)보다 높습니다"),
        "{err}"
    );
    // the first App Store policy: the field, the note, the report
    request.appstore_live = Some(v("1.6.1"));
    let made3 = made(&request).unwrap();
    let ios = &made3.policy.platforms[&Platform::Ios];
    assert_eq!(
        (ios.min, ios.latest, ios.appstore.as_deref()),
        (v("1.6.0"), v("1.6.1"), Some("1.6.1"))
    );
    assert!(made3
        .draft
        .p
        .contains(r#""testflight_build":"263120249","appstore":"1.6.1"}"#));
    assert!(
        made3
            .draft
            .notes
            .iter()
            .any(|n| n
                == "ios 1.6.1: 운영자가 App Store에 나온 버전(1.6.1)을 확인함(--appstore-live)")
    );
    crate::draft::check_notes(&made3.draft.notes).unwrap();
    assert!(made3
        .report
        .iter()
        .any(|l| l.contains("App Store 버전 없음 → 1.6.1")));
    assert_eq!(made3.draft.appstore_release, None);
    // without the App Store version (no field), an old TestFlight build is still refused
    let day_80 = "2027-01-27T02:49:00Z";
    let mut old = ios_only(&file2, day_80, Some(IOS_161), Some("1.6.1"));
    old.no_appstore = true;
    let err = error(&old);
    assert!(err.contains("80일부터 거부"), "{err}");
    // the signer compares a new App Store version with the store run's status
    let plan3 = signing::plan(
        &made3.draft,
        &test_key_set(),
        &archive(),
        None,
        made3.policy.issued,
    )
    .unwrap();
    assert!(
        plan3
            .dangers
            .iter()
            .any(|d| d.starts_with("ios: App Store 버전을 처음으로 1.6.1(으)로 적습니다")),
        "{:?}",
        plan3.dangers
    );
    let file3 = signed(&made3);
    let checked = check_envelope(&file3, &test_key_set(), SERVER).unwrap();
    assert_eq!(
        checked.policy.platforms[&Platform::Ios].appstore_version(),
        Some(v("1.6.1"))
    );

    // With the field the App Store is the gate. 80 days after the TestFlight upload the raise of
    // min is a warning, not a refusal (no new build of 1.6.1 can come).
    let mut request = ios_only(&file3, day_80, Some(IOS_161), None);
    request.min = vec![(Target::One(Platform::Ios), v("1.6.1"))];
    let err = error(&request);
    assert!(
        err.contains("appstore 1.6.1)이 있으므로 latest나 min을 올리려면 --appstore-live"),
        "{err}"
    );
    request.no_appstore = true;
    assert!(error(&request).contains("--no-appstore 대신"));
    request.no_appstore = false;
    request.appstore_live = Some(v("1.6.1"));
    let raised = made(&request).unwrap();
    assert_eq!(raised.policy.platforms[&Platform::Ios].min, v("1.6.1"));
    assert!(
        raised
            .report
            .iter()
            .any(|l| l.contains("올린 지 80일") && l.contains("거부하지 않습니다")),
        "{:?}",
        raised.report
    );
    // the TestFlight build is optional then
    request.ios_valid = None;
    let raised = made(&request).unwrap();
    let ios = &raised.policy.platforms[&Platform::Ios];
    assert_eq!(
        (ios.min, ios.testflight_build.as_deref()),
        (v("1.6.1"), Some(IOS_161))
    );

    // latest never goes above the App Store's version, which never goes down
    let ios_162 = f.ios_release("1.6.2", IOS_162);
    let mut request = ios_only(&file3, now, None, Some("1.6.2"));
    request.artifacts = ios_162.clone();
    assert!(error(&request).contains("--appstore-live"));
    request.appstore_live = Some(v("1.6.1"));
    assert!(error(&request).contains("latest(1.6.2)가 App Store에 나온 버전(1.6.1)보다 높습니다"));
    let mut request = ios_only(&file3, now, None, None);
    request.appstore_live = Some(v("1.6.0"));
    assert!(error(&request).contains("내려가지 않습니다"));
    // a new App Store version is the version of a build the tool knows: a mistyped one is no
    // ceiling for later policies
    request.appstore_live = Some(v("1.6.2"));
    let err = error(&request);
    assert!(
        err.contains("ios: latest를 1.6.2(으)로 바꾸려면 그 빌드를 알아야 합니다"),
        "{err}"
    );
    let mut typo = ios_only(&file3, now, Some(IOS_161), None);
    typo.appstore_live = Some(v("1.6.10"));
    let err = error(&typo);
    assert!(
        err.contains("--appstore-live 1.6.10이 ios의 latest(1.6.1)와 다릅니다"),
        "{err}"
    );
    typo.latest = vec![(Platform::Ios, v("1.6.1"))];
    typo.min = vec![(Target::One(Platform::Ios), v("1.6.1"))];
    assert!(error(&typo).contains("1.6.10이 ios의 latest(1.6.1)와 다릅니다"));
    // with the release directory's artifact of it, latest follows the App Store, and the
    // TestFlight build follows latest
    request.artifacts = ios_162.clone();
    let made4 = made(&request).unwrap();
    let ios = &made4.policy.platforms[&Platform::Ios];
    assert_eq!(
        (
            ios.latest,
            ios.source.as_deref(),
            ios.appstore.as_deref(),
            ios.testflight_build.as_deref()
        ),
        (
            v("1.6.2"),
            Some("client-1.6.2"),
            Some("1.6.2"),
            Some(IOS_162)
        )
    );
    assert!(made4
        .report
        .iter()
        .any(|l| l.contains("App Store 버전 1.6.1 → 1.6.2")));
    assert!(made4.draft.notes.iter().any(|n| n
        == &format!(
        "ios 1.6.2: 빌드({IOS_162})를 릴리스 디렉터리의 release.json에서 가져옴(--ios-valid 없음)"
    )));
    crate::draft::check_notes(&made4.draft.notes).unwrap();
    // and a higher one
    let keys = test_key_set();
    let dangers = |made: &Made| {
        signing::plan(&made.draft, &keys, &archive(), None, made.policy.issued)
            .unwrap()
            .dangers
    };
    assert!(dangers(&made4)
        .iter()
        .any(|d| d.starts_with("ios: App Store 버전을 1.6.1에서 1.6.2(으)로 적습니다")));
    // a policy for something else keeps the field as it is
    let mut request = req_after(&file3, now);
    request.contact = Some("내선 1234".to_owned());
    assert_eq!(
        made_policy(&request).platforms[&Platform::Ios]
            .appstore
            .as_deref(),
        Some("1.6.1")
    );
    // sign holds every draft to it: the field once there is never dropped or lowered
    for (from, to, why) in [
        (r#","appstore":"1.6.2""#, "", "빠졌습니다"),
        (r#""appstore":"1.6.2""#, r#""appstore":"1.6.0""#, "높습니다"),
    ] {
        let mut draft = made4.draft.clone();
        draft.p = draft.p.replacen(from, to, 1);
        let err = signing::plan(&draft, &keys, &archive(), None, made4.policy.issued).unwrap_err();
        assert!(err.contains(why), "{err}");
    }
    // lowered only with a root declaration, to mend a version written by mistake; never dropped
    let mut lowered = made4.policy.clone();
    let ios = lowered.platforms.get_mut(&Platform::Ios).unwrap();
    ios.appstore = Some("1.6.1".to_owned());
    let err = appstore_kept(Some(&made4.policy), &lowered, false).unwrap_err();
    assert!(
        err.contains("App Store 버전이 내려갑니다(1.6.2 → 1.6.1)"),
        "{err}"
    );
    appstore_kept(Some(&made4.policy), &lowered, true).unwrap();
    lowered.platforms.get_mut(&Platform::Ios).unwrap().appstore = None;
    let err = appstore_kept(Some(&made4.policy), &lowered, true).unwrap_err();
    assert!(err.contains("빠졌습니다"), "{err}");
    let file4 = signed(&made4);
    let later = "2026-11-11T04:00:00Z";
    let mut mend = ios_only(&file4, later, Some(IOS_161), Some("1.6.1"));
    mend.appstore_live = Some(v("1.6.1"));
    assert!(error(&mend).contains("--relax와 함께"));
    mend.relax = true;
    mend.live = Some(file4.clone());
    let mended = made(&mend).unwrap();
    let ios = &mended.policy.platforms[&Platform::Ios];
    assert_eq!(
        (
            ios.latest,
            ios.appstore.as_deref(),
            ios.testflight_build.as_deref()
        ),
        (v("1.6.1"), Some("1.6.1"), Some(IOS_161))
    );
    assert!(mended
        .report
        .iter()
        .any(|l| l.contains("--relax: App Store 버전을 내립니다(1.6.2 → 1.6.1)")));
    let plan5 = signing::plan(
        &mended.draft,
        &keys,
        &archive(),
        Some(&file4),
        mended.policy.issued,
    )
    .unwrap();
    assert!(
        plan5
            .dangers
            .iter()
            .any(|d| d.contains("루트 선언과 함께 App Store 버전을 내립니다(1.6.2 → 1.6.1)")),
        "{:?}",
        plan5.dangers
    );
}

/// T3: a change of iOS without the App Store while the store record has an App Store release
/// is said in the draft, and `sign` asks for a typed yes.
#[test]
fn no_app_store_after_a_release_asks_the_signer() {
    let f = Fixture::new("make-no-appstore");
    let (_, file2) = history(&f);
    let now = "2026-11-10T04:00:00Z";
    let mut request = ios_only(&file2, now, Some(IOS_161), Some("1.6.1"));
    request.no_appstore = true;
    let commit = "1111111111111111111111111111111111111111";
    let submitted = format!(
        "# store version build action track source-commit date(UTC) note\n\
         play 1.6.1 {APK_161} upload alpha {commit} 2026-11-09 store #1\n\
         appstore 1.6.1 {IOS_161} submit - {commit} 2026-11-09 store #2\n"
    );
    let keys = test_key_set();
    let dangers = |made: &Made| {
        signing::plan(&made.draft, &keys, &archive(), None, made.policy.issued)
            .unwrap()
            .dangers
    };
    // submitted, not released: nothing to ask
    let made3 = made_with_record(&request, &submitted).unwrap();
    assert_eq!(made3.draft.appstore_release, None);
    assert!(!dangers(&made3).iter().any(|d| d.contains("출시 기록")));
    // released: the draft says so, and the signer is asked
    let released =
        format!("{submitted}appstore 1.6.1 {IOS_161} release - {commit} 2026-11-10 store #3\n");
    let made3 = made_with_record(&request, &released).unwrap();
    assert_eq!(made3.draft.appstore_release.as_deref(), Some("1.6.1"));
    assert!(made3
        .report
        .iter()
        .any(|l| l.contains("출시 기록에 App Store 출시(1.6.1)가 있는데 --no-appstore")));
    let draft = Draft::parse(&made3.draft.to_text()).unwrap();
    assert_eq!(draft, made3.draft);
    assert!(
        dangers(&made3)
            .iter()
            .any(|d| d.contains("출시 기록에 App Store 출시(1.6.1)가 있는데")),
        "{:?}",
        dangers(&made3)
    );
    // with the App Store's version named, nobody reads the record and nothing is asked
    request.no_appstore = false;
    request.appstore_live = Some(v("1.6.1"));
    let named = made_with_record(&request, &released).unwrap();
    assert_eq!(named.draft.appstore_release, None);
    // a record the tool cannot read is not passed over
    request.appstore_live = None;
    request.no_appstore = true;
    let err = made_with_record(&request, "appstore 1.6.1 release\n").unwrap_err();
    assert!(err.contains("1번째 줄"), "{err}");
    // a draft whose iOS entry names the App Store carries no such word (policy makes none)
    let mut odd = named.draft.clone();
    odd.appstore_release = Some("1.6.1".to_owned());
    assert!(
        signing::plan(&odd, &keys, &archive(), None, named.policy.issued)
            .unwrap_err()
            .contains("appstore_release")
    );
}

/// T2 (auto-deploy design 3-7): the `android` minimum goes up only with an `android_play` entry,
/// or with `--no-play`.
#[test]
fn raising_android_without_the_play_entry() {
    let f = Fixture::new("make-no-play");
    let (_, file2) = history(&f);
    let mut request = release_161(&f, &file2);
    request.min = vec![(Target::One(Platform::Android), v("1.6.1"))];
    let err = error(&request);
    assert!(
        err.contains("android_play 항목 없이 android의 min을 올립니다(1.6.0 → 1.6.1)")
            && err.contains("--play-live")
            && err.contains("--no-play"),
        "{err}"
    );
    request.no_play = true;
    let made3 = made(&request).unwrap();
    assert_eq!(made3.policy.platforms[&Platform::Android].min, v("1.6.1"));
    assert!(made3
        .report
        .iter()
        .any(|l| l.contains("--no-play: android_play 없이")));
    assert!(made3.report.iter().any(|l| l == crate::make::NO_PLAY_ENTRY));
    // what --no-play passes over is the signer's to say yes to
    let plan3 = signing::plan(
        &made3.draft,
        &test_key_set(),
        &archive(),
        None,
        made3.policy.issued,
    )
    .unwrap();
    assert!(
        plan3.dangers.iter().any(|d| d
            .starts_with("android: android_play 항목 없이 android의 min을 올립니다(--no-play)")),
        "{:?}",
        plan3.dangers
    );
    // with the Play entry nothing is passed over, and --no-play is a mistake
    request.play_live = Some(APK_161.parse().unwrap());
    request
        .min
        .push((Target::One(Platform::AndroidPlay), v("1.6.1")));
    assert!(error(&request).contains("--no-play를 줬지만"));
    request.no_play = false;
    let p = made_policy(&request);
    assert_eq!(p.platforms[&Platform::AndroidPlay].min, v("1.6.1"));
    // the same minimum needs nothing
    let mut request = release_161(&f, &file2);
    request.no_play = true;
    assert!(error(&request).contains("--no-play를 줬지만"));
    // a new android entry is a raise as well
    let mut dropped = req_after(&file2, "2026-11-09T04:00:00Z");
    dropped.live = Some(file2.clone());
    dropped.drop = vec![Platform::Android];
    let file3 = signed(&made(&dropped).unwrap());
    let mut again = req(
        Some(&file3),
        f.release("1.6.1", IOS_161, APK_161),
        "2026-11-10T04:00:00Z",
    );
    again.hold = vec![Platform::Windows, Platform::Ios];
    again.min = vec![(Target::One(Platform::Android), v("1.6.0"))];
    let err = error(&again);
    assert!(
        err.contains("android_play 항목 없이 android의 min을 올립니다(없음 → "),
        "{err}"
    );
}

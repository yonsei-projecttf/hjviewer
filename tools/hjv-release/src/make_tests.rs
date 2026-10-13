// SPDX-License-Identifier: AGPL-3.0-only

//! Tests of make.rs: a release history made with the test keys and recorded `gh` answers.

use super::*;
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
};
use std::path::PathBuf;

const SERVER: &str = "rd.example.com";
/// November 8, 13:00 in Korea.
const NOW: &str = "2026-11-08T04:00:00Z";
// the iOS build numbers (UTC time of the build) and the Android versionCodes (the same + 2000)
const IOS_160: &str = "263100200";
const APK_160: &str = "263102200";
const IOS_161: &str = "263120249";
const APK_161: &str = "263122249";

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Fixture {
        Fixture {
            root: temp_dir(name),
        }
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

fn made(request: &Request) -> Result<Made, String> {
    make(
        request,
        &test_key_set(),
        &Settings::new(PathBuf::from(".")),
        &Recorded::good(),
    )
}

fn secret(name: &str) -> SecretKey {
    let key = test_key(name).unwrap();
    SecretKey::from_seed(key.role, key.id, &key.seed())
}

/// Signs a draft with the test keys it needs, as the person at the terminal would.
fn signed(made: &Made) -> String {
    let keys = test_key_set();
    let plan = signing::plan(&made.draft, &keys, made.policy.issued).unwrap();
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
    if plan.needs_root(&made.draft) {
        secrets.push(secret("root-101"));
    }
    if made.draft.signer == Role::Signing {
        secrets.push(secret(if revoked.contains(&1) {
            "signing-2"
        } else {
            "signing-1"
        }));
    }
    signing::sign(&made.draft, &plan, &secrets, &keys).unwrap()
}

/// seq 1 (design 1-6): 1.6.0 is out everywhere, nothing is enforced yet.
fn first(f: &Fixture) -> Request {
    let mut request = req(None, f.release("1.6.0", IOS_160, APK_160), NOW);
    request.min = vec![(Target::All, v("1.5.0"))];
    request.grace = vec![(Target::All, Grace::Days(0))];
    request.ios_valid = Some(IOS_160.to_owned());
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
    assert_eq!(draft.notes.len(), 3);
    assert!(draft.notes[0].contains("windows 1.6.0: 실행 4242 성공"));
    assert!(draft.notes[1].contains("android 1.6.0: 실행 4242 성공"));
    assert!(draft.notes[2].contains("TestFlight 빌드(263100200)를 확인함"));
    assert!(made.report.iter().any(|l| l.contains("첫 정책")));
    assert!(made.report.iter().any(|l| l
        .contains("windows: 무인 설치는 기본값대로 게시 2일 뒤(2026-11-10 13:00 한국 시각)부터")));
    let mut without = first(&f);
    without.auto_after = Some(AutoAfter::Off);
    assert_eq!(
        made_policy(&without).platforms[&Platform::Windows].auto_install_after,
        None
    );
    assert!(made.report.iter().any(|l| l == "서명할 키: 서명 키"));

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
    // new entries without a grace option get the default of 7 days as well
    let mut request = good.clone();
    request.grace.clear();
    let with_default = made(&request).unwrap();
    let week = t("2026-11-14T15:00:00Z");
    assert!(with_default
        .policy
        .platforms
        .values()
        .all(|e| e.grace_until == Some(week)));
    assert_eq!(
        with_default
            .report
            .iter()
            .filter(|l| l.contains("기본 7일"))
            .count(),
        3
    );
    assert!(change(&|r| r.min = vec![(Target::All, v("1.6.1"))])
        .contains("min(1.6.1)이 latest(1.6.0)보다 높습니다"));
    assert!(
        change(&|r| r.artifacts.clear()).contains("windows: 패키지가 없어 항목을 만들 수 없습니다")
    );
    let nothing = |r: &mut Request| {
        r.artifacts.clear();
        r.os_min.clear();
        r.ios_valid = None;
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
        &Settings::new(PathBuf::from(".")),
        &tools,
    )
    .unwrap();
    // nothing new was built, so nobody was asked about provenance
    assert_eq!(*tools.asked.borrow(), Vec::<String>::new());
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
    let file2 = signed(&made2);
    assert!(trust::check_after(&file2, Some(&file1), &test_key_set(), SERVER).is_ok());

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
    // 0 days is said, never assumed
    let mut request = enforce(&file1);
    request.grace = vec![(Target::All, Grace::Days(0))];
    assert!(made_policy(&request)
        .platforms
        .values()
        .all(|e| e.grace_until.is_none()));
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
    assert!(error(&request).contains("min이 바뀌는 항목이 없습니다"));
    let mut request = enforce(&file1);
    request.notes = vec![(Target::All, "새 소식".to_owned())];
    assert!(error(&request).contains("latest가 바뀌는 항목이 없습니다"));
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
    assert_eq!(made3.draft.notes.len(), 2);
    let file3 = signed(&made3);

    // with the build confirmed, iOS follows
    let mut request = release_161(&f, &file2);
    request.ios_valid = Some(IOS_161.to_owned());
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
    let refused = |tools: Recorded| make(&request, &keys, &settings, &tools).unwrap_err();
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
    let tools = Recorded::good();
    make(&request, &keys, &settings, &tools).unwrap();
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
}

#[test]
fn relaxing_needs_the_flag_and_the_root_key() {
    let f = Fixture::new("make-relax");
    let (_, file2) = history(&f);
    let file3 = signed(&made(&release_161(&f, &file2)).unwrap());
    let now = "2026-11-10T04:00:00Z";
    let deadline = t("2026-11-21T15:00:00Z");
    type Change = Box<dyn Fn(&mut Request)>;
    let cases: Vec<(&str, Change)> = vec![
        (
            "ios: min을 내림",
            Box::new(|r| r.min = vec![(Target::One(Platform::Ios), v("1.5.0"))]),
        ),
        (
            "ios: 같은 min의 유예를 늦추거나 새로 둠",
            Box::new(move |r| {
                r.grace = vec![(Target::One(Platform::Ios), Grace::Until(deadline + 1))]
            }),
        ),
        ("ios: 항목을 뺌", Box::new(|r| r.drop = vec![Platform::Ios])),
        (
            "windows: latest를 내림",
            Box::new(|r| r.latest = vec![(Platform::Windows, v("1.6.0"))]),
        ),
    ];
    for (what, change) in &cases {
        // lowering latest needs the older package again
        let mut request = req_after(&file3, now);
        if what.starts_with("windows") {
            request.artifacts = f.release("1.6.0", IOS_160, APK_160);
            request.hold = vec![Platform::Android];
        }
        change(&mut request);
        let err = error(&request);
        assert!(
            err.contains("완화가 들어 있습니다") && err.contains(what),
            "{what}: {err}"
        );
        request.relax = true;
        let made = made(&request).unwrap();
        assert_eq!(made.draft.signer, Role::Root, "{what}");
        assert!(
            made.report.iter().any(|l| l == &format!("완화: {what}")),
            "{what}: {:?}",
            made.report
        );
        assert!(made.report.iter().any(|l| l == "서명할 키: 루트 키"));
        // the signer is asked, and the root key's signature is what an app takes
        let plan = signing::plan(&made.draft, &test_key_set(), made.policy.issued).unwrap();
        assert!(
            plan.dangers.iter().any(|d| d.contains(what)),
            "{what}: {:?}",
            plan.dangers
        );
        let file = signed(&made);
        assert_eq!(
            check_envelope(&file, &test_key_set(), SERVER).unwrap().role,
            Role::Root
        );
    }
    // a lowered min loses its old deadline; a new one may be given
    let mut request = req_after(&file3, now);
    request.relax = true;
    request.min = vec![(Target::All, v("1.5.0"))];
    let p = made_policy(&request);
    assert!(p
        .platforms
        .values()
        .all(|e| e.min == v("1.5.0") && e.grace_until.is_none()));
    // tightening is no relaxation: an earlier deadline, by a signing key
    let mut request = req_after(&file3, now);
    request.grace = vec![(Target::One(Platform::Ios), Grace::Until(deadline - DAY))];
    let made_earlier = made(&request).unwrap();
    assert_eq!(made_earlier.draft.signer, Role::Signing);
    assert_eq!(
        made_earlier.policy.platforms[&Platform::Ios].grace_until,
        Some(deadline - DAY)
    );
    // --relax without anything to relax still asks for the root key: the operator may be
    // undoing a policy this tool has not seen (design 6-3, a leaked signing key)
    let mut request = req_after(&file3, now);
    request.relax = true;
    assert_eq!(made(&request).unwrap().draft.signer, Role::Root);
    // dropping what is not there, holding and dropping the same entry
    let mut request = req_after(&file3, now);
    request.relax = true;
    request.drop = vec![Platform::AndroidPlay];
    assert!(error(&request).contains("뺄 항목이 지금 정책에 없습니다"));
    request.drop = vec![Platform::Ios];
    request.hold = vec![Platform::Ios];
    assert!(error(&request).contains("두 번"));
}

#[test]
fn revocation_and_epochs() {
    let f = Fixture::new("make-root");
    let (_, file2) = history(&f);
    let now = "2026-11-10T04:00:00Z";
    // key 1 leaked: revoke it and start a new epoch (design 6-3)
    let mut request = req_after(&file2, now);
    request.revoke = vec![1];
    request.new_epoch = true;
    let made3 = made(&request).unwrap();
    assert_eq!((made3.policy.epoch, made3.policy.seq), (2, 1));
    assert_eq!(made3.draft.signer, Role::Signing);
    assert_eq!(made3.draft.root, None);
    let decl = RootDecl::parse(made3.draft.root_p.as_ref().unwrap()).unwrap();
    assert_eq!(
        (decl.epoch, decl.revoked.as_slice(), decl.issued),
        (2, &[1][..], t(now))
    );
    assert_eq!(decl.server, SERVER);
    assert!(made3
        .report
        .iter()
        .any(|l| l.contains("새 루트 선언: 세대 2, 폐기한 서명 키 [1]")));
    assert!(made3
        .report
        .iter()
        .any(|l| l == "서명할 키: 서명 키 + 루트 키(루트 선언)"));
    let plan = signing::plan(&made3.draft, &test_key_set(), t(now)).unwrap();
    assert_eq!(plan.dangers.len(), 2);
    let file3 = signed(&made3);
    let checked = check_envelope(&file3, &test_key_set(), SERVER).unwrap();
    assert_eq!(
        (
            checked.envelope.policy.k,
            checked.root.as_ref().unwrap().revoked.as_slice()
        ),
        (2, &[1][..])
    );
    assert!(trust::check_after(&file3, Some(&file2), &test_key_set(), SERVER).is_ok());

    // from now on every envelope carries the declaration, as it was signed
    let made4 = made(&req_after(&file3, "2026-11-11T04:00:00Z")).unwrap();
    assert_eq!((made4.policy.epoch, made4.policy.seq), (2, 2));
    assert_eq!(made4.draft.root, Envelope::parse(&file3).unwrap().root);
    assert_eq!(made4.draft.root_p, None);
    let file4 = signed(&made4);
    assert!(trust::check_after(&file4, Some(&file3), &test_key_set(), SERVER).is_ok());

    // revocations add up; the root key cannot be revoked; nothing is revoked twice
    let mut request = req_after(&file4, "2026-11-12T04:00:00Z");
    request.revoke = vec![7];
    let made5 = made(&request).unwrap();
    let decl = RootDecl::parse(made5.draft.root_p.as_ref().unwrap()).unwrap();
    assert_eq!((decl.epoch, decl.revoked.as_slice()), (2, &[1, 7][..]));
    assert_eq!(made5.policy.seq, 3);
    request.revoke = vec![101];
    assert!(error(&request).contains("루트 키 번호는 폐기할 수 없습니다"));
    request.revoke = vec![1];
    assert!(error(&request).contains("이미 폐기한 키"));
    request.revoke = vec![7, 7];
    assert!(error(&request).contains("두 번"));
    // with every signing key revoked, only the root key can sign until a build has new keys
    request.revoke = vec![2];
    assert!(error(&request).contains("폐기되지 않은 서명 키가 키 파일에 없습니다"));
    request.relax = true;
    assert_eq!(made(&request).unwrap().draft.signer, Role::Root);

    // a revocation without a new epoch, and a new epoch without a revocation
    let mut request = req_after(&file2, now);
    request.revoke = vec![2];
    let m = made(&request).unwrap();
    let decl = RootDecl::parse(m.draft.root_p.as_ref().unwrap()).unwrap();
    assert_eq!(
        (
            m.policy.epoch,
            m.policy.seq,
            decl.epoch,
            decl.revoked.as_slice()
        ),
        (1, 3, 1, &[2][..])
    );
    assert!(trust::check_after(&signed(&m), Some(&file2), &test_key_set(), SERVER).is_ok());
    let mut request = req_after(&file2, now);
    request.new_epoch = true;
    let m = made(&request).unwrap();
    let decl = RootDecl::parse(m.draft.root_p.as_ref().unwrap()).unwrap();
    assert_eq!(
        (m.policy.epoch, m.policy.seq, decl.epoch, decl.revoked.len()),
        (2, 1, 2, 0)
    );
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
    assert!(jump
        .report
        .iter()
        .any(|l| l.contains("seq가 10 넘게 뜁니다")));
    assert!(!with_seq(12)
        .unwrap()
        .report
        .iter()
        .any(|l| l.contains("넘게 뜁니다")));
    assert!(with_seq(2).unwrap_err().contains("높지 않습니다"));
    assert!(with_seq(1).unwrap_err().contains("높지 않습니다"));
    assert!(with_seq(1002).is_ok());
    assert!(with_seq(1003).unwrap_err().contains("1000"));
    // the root key may jump further
    let mut request = req_after(&file2, now);
    request.seq = Some(5000);
    request.relax = true;
    assert_eq!(made(&request).unwrap().policy.seq, 5000);
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
    // a build whose floor is above the previous policy does not take it either
    let mut keys = test_key_set();
    keys.floor = Floor { epoch: 1, seq: 3 };
    let err = make(
        &req_after(&file2, now),
        &keys,
        &Settings::new(PathBuf::from(".")),
        &Recorded::good(),
    )
    .unwrap_err();
    assert!(err.contains("하한"), "{err}");
    // the first policy of a key file whose floor is not zero starts above the floor
    let mut request = first(&f);
    request.now = t(now);
    let made = make(
        &request,
        &keys,
        &Settings::new(PathBuf::from(".")),
        &Recorded::good(),
    )
    .unwrap();
    assert_eq!(made.policy.seq, 4);
    assert!(made.report.iter().any(|l| l.contains("하한이 1:3입니다")));
    // the first envelope is still what it was: the second one names it byte for byte
    assert_eq!(
        Draft::parse(
            &crate::make::make(
                &enforce(&file1),
                &test_key_set(),
                &Settings::new(PathBuf::from(".")),
                &Recorded::good()
            )
            .unwrap()
            .draft
            .to_text()
        )
        .unwrap()
        .prev
        .as_deref(),
        Some(file1.as_str())
    );
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
    // the Play entry appears once the operator saw the release on the internal track
    let add = |code: u64, artifacts: Vec<Artifact>| {
        let mut request = req(Some(&file2), artifacts, now);
        request.play_live = Some(code);
        request
            .min
            .push((Target::One(Platform::AndroidPlay), v("1.6.0")));
        request.grace = vec![(Target::One(Platform::AndroidPlay), Grace::Days(7))];
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
        .any(|n| n.contains("Play 내부 트랙의 versionCode(263102200)를 확인함")));
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
    let file4 = signed(&made(&request).unwrap());
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
    assert!(signed(&made).len() <= MAX_ENVELOPE_BYTES);
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
    let actual = signed(&made).len();
    assert!(size >= actual && size - actual <= 8, "{size} for {actual}");
    assert_eq!(
        envelope_size("{}", None),
        "{\"v\":1,\"k\":65535,\"p\":\"{}\",\"s\":\"\"}\n".len() + 88
    );
}

// SPDX-License-Identifier: AGPL-3.0-only

//! `hjv-release check`: the weekly look at what goes stale without a release (design 5-5, 7):
//!
//! - the TestFlight build: after 90 days every iPhone app stops, whatever the minimum version
//!   is; the check warns from day 75;
//! - door 2's `UNTIL`;
//! - the policy's age and position, and whether the server hands out the policy this PC has
//!   (an older one: frozen or rolled back; a newer one nobody here made: a stolen signing key).

use crate::{
    hbbs::Door2,
    keys::KeySet,
    make::{testflight_age, TESTFLIGHT_WARN_DAYS},
    policy::Platform,
    time::{self, DAY},
    trust::{check_envelope, describe, Checked, MAX_SEQ_STEP},
};

/// A TestFlight build runs for this many days after its upload.
pub const TESTFLIGHT_DAYS: i64 = 90;
/// Door 2 closing within this many days is worth a warning.
pub const DOOR2_WARN_DAYS: i64 = 7;

#[derive(Debug, Default)]
pub struct Report {
    pub lines: Vec<String>,
    pub warnings: usize,
}

impl Report {
    fn info(&mut self, line: String) {
        self.lines.push(line);
    }

    fn warn(&mut self, line: String) {
        self.lines.push(format!("경고: {line}"));
        self.warnings += 1;
    }
}

/// Looks at the policy this PC has (`local`, already verified), and optionally at door 2's file
/// and at the envelope the server hands out now (`live`).
pub fn check(
    local: &Checked,
    local_file: &str,
    door2: Option<&Door2>,
    live: Option<&str>,
    keys: &KeySet,
    now: i64,
) -> Report {
    let mut report = Report::default();
    let policy = &local.policy;
    for line in describe(policy) {
        report.info(line);
    }
    report.info(format!(
        "정책을 낸 지 {}일(세대 {}, seq {})",
        (now - policy.issued).div_euclid(DAY).max(0),
        policy.epoch,
        policy.seq
    ));
    if keys.has_test_key() {
        report.warn(
            "키 파일에 이 저장소의 시험 키가 있습니다. 실서버 빌드에 쓰면 안 됩니다".to_owned(),
        );
    }
    // a fresh install takes a signing key's policy only up to 1000 above its build's floor
    let room = if policy.epoch == keys.floor.epoch {
        (keys.floor.seq + MAX_SEQ_STEP).saturating_sub(policy.seq)
    } else {
        MAX_SEQ_STEP.saturating_sub(policy.seq)
    };
    if room < 200 {
        report.warn(format!(
            "seq({})가 빌드 하한({})에서 {MAX_SEQ_STEP}에 가까워졌습니다. 그 뒤로는 새로 설치한 앱이 \
             정책을 받지 못합니다. 하한을 올린 빌드를 내거나 루트 키로 세대를 올리세요",
            policy.seq, keys.floor
        ));
    }

    if let Some(ios) = policy.platforms.get(&Platform::Ios) {
        let build = ios.testflight_build.as_deref().unwrap_or("");
        match testflight_age(now)(build) {
            Some(age) if age >= TESTFLIGHT_DAYS => report.warn(format!(
                "TestFlight 빌드({build})를 올린 지 {age}일입니다. 만료되어 아이폰 앱이 실행되지 않습니다. \
                 새 TestFlight 빌드를 올리세요"
            )),
            Some(age) if age >= TESTFLIGHT_WARN_DAYS => report.warn(format!(
                "TestFlight 빌드({build})를 올린 지 {age}일입니다({TESTFLIGHT_DAYS}일에 만료, {}일 남음). \
                 새 TestFlight 빌드를 올리세요",
                TESTFLIGHT_DAYS - age
            )),
            Some(age) => report.info(format!(
                "TestFlight 빌드 {build}: 올린 지 {age}일({TESTFLIGHT_DAYS}일에 만료)"
            )),
            None => report.warn(format!("TestFlight 빌드 번호를 읽을 수 없습니다({build:?})")),
        }
    }
    for (platform, entry) in &policy.platforms {
        if let Some(grace) = entry.grace_until.filter(|g| *g > now) {
            report.info(format!(
                "{platform}: 유예 끝({})까지 {}",
                time::format_kst(grace),
                time::format_span(grace - now)
            ));
        }
    }

    if let Some(door) = door2 {
        let left = door.until - now;
        if left <= 0 {
            report.warn(format!(
                "문 2의 UNTIL({})이 지났습니다. 문이 닫혀 있습니다. 1.5.x가 남아 있으면 UNTIL을 늦추고, \
                 아니면 door2.conf를 지우고 hbbs 문구를 S2로 바꾸세요",
                time::format_kst(door.until)
            ));
        } else if left <= DOOR2_WARN_DAYS * DAY {
            report.warn(format!(
                "문 2가 {} 뒤({})에 닫힙니다",
                time::format_span(left),
                time::format_kst(door.until)
            ));
        } else {
            report.info(format!("문 2: {}까지 열림", time::format_kst(door.until)));
        }
    }

    if let Some(live) = live {
        if live == local_file {
            report.info("서버가 내주는 정책이 이 PC의 정책과 같습니다".to_owned());
        } else {
            match check_envelope(live, keys, &policy.server) {
                Err(e) => report.warn(format!("서버가 내주는 정책이 검증되지 않습니다: {e}")),
                Ok(theirs) => {
                    let (ours, theirs) = (
                        (policy.epoch, policy.seq),
                        (theirs.policy.epoch, theirs.policy.seq),
                    );
                    let position = format!("세대 {}, seq {}", theirs.0, theirs.1);
                    if theirs < ours {
                        report.warn(format!(
                            "서버가 옛 정책({position})을 내주고 있습니다. 새 정책이 설치되지 않았거나 \
                             되돌려졌습니다"
                        ));
                    } else if theirs > ours {
                        report.warn(format!(
                            "서버의 정책({position})이 이 PC의 정책보다 새롭습니다. 이 PC에서 낸 정책이 \
                             아니면 서명 키가 새어 나갔을 수 있습니다(루트 키로 폐기)"
                        ));
                    } else {
                        report.warn(format!(
                            "서버의 정책이 같은 위치({position})인데 내용이 다릅니다. 서명 키가 새어 \
                             나갔을 수 있습니다"
                        ));
                    }
                }
            }
        }
    }
    report.info(
        "남은 1.5.x PC는 이 도구가 볼 수 없습니다. 서버의 hjv-api admin report와 운영자 PC 목록으로 확인하세요"
            .to_owned(),
    );
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        envelope::{Envelope, Part, POLICY_DOMAIN},
        keys::{test_key, test_key_set, Floor},
        policy::{
            tests::{sample, t},
            Policy,
        },
    };
    use ed25519_dalek::SigningKey;

    fn file(policy: &Policy, by: &str) -> String {
        let key = test_key(by).unwrap();
        Envelope {
            policy: Part::sign(
                &SigningKey::from_bytes(&key.seed()),
                key.id,
                POLICY_DOMAIN,
                &policy.to_text(),
            ),
            root: None,
        }
        .to_file()
    }

    fn run(policy: &Policy, door2: Option<&Door2>, live: Option<&str>, now: &str) -> Report {
        let keys = test_key_set();
        let local = file(policy, "signing-1");
        let checked = check_envelope(&local, &keys, "rd.example.com").unwrap();
        check(&checked, &local, door2, live, &keys, t(now))
    }

    fn warnings(report: &Report) -> Vec<&String> {
        report
            .lines
            .iter()
            .filter(|l| l.starts_with("경고: "))
            .collect()
    }

    #[test]
    fn testflight_expiry() {
        // the sample's build 263120249 was made on day 312 of 2026, November 8
        let fresh = run(&sample(), None, None, "2026-11-20T00:00:00Z");
        // the vectors' keys are test keys, which is always a warning; nothing else here
        assert_eq!(fresh.warnings, 1);
        assert!(warnings(&fresh)[0].contains("시험 키"));
        assert!(fresh
            .lines
            .iter()
            .any(|l| l.contains("TestFlight 빌드 263120249: 올린 지 11일")));
        assert!(fresh.lines.iter().any(|l| l.contains("1.5.x PC")));

        let day_74 = run(&sample(), None, None, "2027-01-21T02:50:00Z");
        assert_eq!(day_74.warnings, 1);
        let day_75 = run(&sample(), None, None, "2027-01-22T02:50:00Z");
        assert_eq!(day_75.warnings, 2);
        assert!(warnings(&day_75).iter().any(|l| l.contains("75일")
            && l.contains("15일 남음")
            && l.contains("새 TestFlight 빌드를 올리세요")));
        let day_90 = run(&sample(), None, None, "2027-02-06T02:50:00Z");
        assert!(warnings(&day_90).iter().any(|l| l.contains("만료되어")));
        // a policy without an iOS entry has nothing to expire
        let mut no_ios = sample();
        no_ios.platforms.remove(&Platform::Ios);
        assert_eq!(run(&no_ios, None, None, "2027-06-01T00:00:00Z").warnings, 1);
    }

    #[test]
    fn door_2() {
        let door = Door2 {
            code: "0123456789abcdef0123456789abcdef".to_owned(),
            until: t("2026-12-22T15:00:00Z"),
        };
        let open = run(&sample(), Some(&door), None, "2026-11-20T00:00:00Z");
        assert_eq!(open.warnings, 1);
        assert!(open
            .lines
            .iter()
            .any(|l| l.contains("문 2: 2026-12-23 00:00 한국 시각까지 열림")));
        let soon = run(&sample(), Some(&door), None, "2026-12-16T15:00:00Z");
        assert!(warnings(&soon).iter().any(|l| l.contains("문 2가 6일 뒤")));
        let closed = run(&sample(), Some(&door), None, "2026-12-22T15:00:00Z");
        assert!(warnings(&closed)
            .iter()
            .any(|l| l.contains("UNTIL") && l.contains("지났습니다")));
        // the code is never part of the report
        for report in [open, soon, closed] {
            assert!(!report.lines.iter().any(|l| l.contains("0123456789abcdef")));
        }
    }

    #[test]
    fn the_server_against_this_pc() {
        let now = "2026-11-20T00:00:00Z";
        let local = file(&sample(), "signing-1");
        let same = run(&sample(), None, Some(&local), now);
        assert_eq!(same.warnings, 1);
        assert!(same.lines.iter().any(|l| l.contains("같습니다")));

        let older = Policy { seq: 6, ..sample() };
        let report = run(&sample(), None, Some(&file(&older, "signing-1")), now);
        assert!(warnings(&report)
            .iter()
            .any(|l| l.contains("옛 정책(세대 1, seq 6)")));
        let newer = Policy { seq: 8, ..sample() };
        let report = run(&sample(), None, Some(&file(&newer, "signing-2")), now);
        assert!(warnings(&report)
            .iter()
            .any(|l| l.contains("새롭습니다") && l.contains("seq 8")));
        let mut other = sample();
        other.contact_ko = None;
        let report = run(&sample(), None, Some(&file(&other, "signing-1")), now);
        assert!(warnings(&report).iter().any(|l| l.contains("같은 위치")));
        // not a policy of these keys, or not a policy at all
        for bad in [
            file(&sample(), "signing-3"),
            "<html>502</html>".to_owned(),
            String::new(),
        ] {
            let report = run(&sample(), None, Some(&bad), now);
            assert!(
                warnings(&report)
                    .iter()
                    .any(|l| l.contains("검증되지 않습니다")),
                "{bad}"
            );
        }
    }

    #[test]
    fn seq_near_the_end_of_a_fresh_install() {
        let keys = test_key_set();
        let at = |seq: u64, floor: Floor| {
            let mut keys = keys.clone();
            keys.floor = floor;
            let policy = Policy { seq, ..sample() };
            let local = file(&policy, "signing-1");
            let checked = check_envelope(&local, &keys, "rd.example.com").unwrap();
            check(
                &checked,
                &local,
                None,
                None,
                &keys,
                t("2026-11-20T00:00:00Z"),
            )
            .warnings
        };
        let floor = |seq| Floor { epoch: 1, seq };
        assert_eq!(at(800, floor(0)), 1);
        assert_eq!(at(801, floor(0)), 2);
        assert_eq!(at(1500, floor(0)), 2);
        // a raised floor makes room again
        assert_eq!(at(1500, floor(1400)), 1);
    }
}

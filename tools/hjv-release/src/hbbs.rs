// SPDX-License-Identifier: AGPL-3.0-only

//! hbbs's side of a policy (design 1-5 and 2-4): the file `/etc/hjviewer/min-controller.conf`
//! with the one threshold hbbs enforces at punch hole, and door 2's `/etc/hjviewer/door2.conf`.
//!
//! ```text
//! MIN=1.6.0                        refuse controllers below this version
//! FROM=2026-11-22T15:00:00Z        from this time on
//! TEXT=…%MIN%… (version not allowed)   what a 1.5.1 app shows
//! ```
//!
//! A punch hole request names a version and no platform, so hbbs has one threshold for all:
//!
//! - `MIN` is the lowest `min` of all entries of the policy.
//! - `FROM` is set again only when `MIN` changes: to the latest `grace_until` of the entries that
//!   have just reached the new `MIN` (or the policy's `issued` time when none of them has a grace
//!   period). When one platform alone raises its `min`, `MIN` stays, and so does `FROM`: a
//!   refusal that has begun does not stop again.
//! - `FROM` never goes back: the new file's `FROM` is the later of the derived one and the present
//!   file's. Nobody is refused earlier than announced.
//! - hbbs has one threshold (it refuses below `MIN` from `FROM` on and nobody before), so during a
//!   raise the present file stays until the new one refuses, at every time from now on, at least
//!   what the present one refuses: a file with a higher `MIN` is written only when its `FROM` is
//!   not after max(now, the present `FROM`); otherwise the tool says when to run again (trust
//!   rules 3-5, 4-7, INV-16). With no present file (or one with an empty `MIN`) the new file is
//!   written at once (runbook 5-2).
//! - A `MIN` below the file's is always pointed out: hbbs would let in again what it refuses
//!   now. It comes from a root declaration that relaxes, or from a new entry with a lower `min`.
//!
//! The present file is read in the form this tool writes: `MIN` as `x.y.z` and `FROM` as
//! `2026-11-22T15:00:00Z`. A file edited by hand into another form (an offset, leading zeros) is
//! refused rather than guessed at; hbbs should refuse the same forms (the stricter of the two).
//!
//! The door 2 code is half a secret: it is written into the two files and never shown.

use crate::{
    policy::{Platform, Policy},
    time,
    version::Version,
};

/// The texts of design 2-5. `%MIN%` stays in the file; hbbs fills it in.
pub const S1: &str = "HJViewer를 %MIN% 이상으로 업데이트해야 연결할 수 있습니다. Play 스토어에서 설치했다면 Play 스토어에서, iPhone은 TestFlight 앱에서 업데이트하세요. 그 밖의 Windows·Android는 %LINK% 에서 새 버전을 받으세요. (version not allowed)";
pub const S2: &str = "HJViewer를 %MIN% 이상으로 업데이트해야 연결할 수 있습니다. 앱의 업데이트 화면을 쓰거나 관리자에게 새 버전을 받으세요. Play 스토어나 TestFlight로 설치했다면 그곳에서 업데이트하세요. (version not allowed)";

/// The `MIN` and `FROM` of a min-controller.conf; `None` where the line is empty or missing
/// (no `MIN`: hbbs refuses nobody; no `FROM`: at once).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Current {
    pub min: Option<Version>,
    pub from: Option<i64>,
}

/// Reads `MIN` and `FROM` of an existing file. `TEXT` is not looked at: it holds the door 2 link.
pub fn parse_conf(text: &str) -> Result<Current, String> {
    let (mut min, mut from) = (None, None);
    for line in text.strip_prefix('\u{feff}').unwrap_or(text).lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("MIN=") {
            if min.is_some() {
                return Err("MIN 줄이 둘입니다".to_owned());
            }
            let value = value.trim();
            min =
                Some(if value.is_empty() {
                    None
                } else {
                    Some(Version::parse(value).map_err(|e| {
                        format!("MIN: {e}. hjv-release가 쓰는 꼴(1.6.0)로 되돌리세요")
                    })?)
                });
        } else if let Some(value) = line.strip_prefix("FROM=") {
            if from.is_some() {
                return Err("FROM 줄이 둘입니다".to_owned());
            }
            let value = value.trim();
            from = Some(if value.is_empty() {
                None
            } else {
                Some(time::parse_utc(value).map_err(|e| {
                    format!(
                        "FROM: {e}. hjv-release가 쓰는 꼴(2026-11-22T15:00:00Z, UTC)만 읽습니다. 손으로 고친 파일이면 \
                         그 꼴로 되돌리세요"
                    )
                })?)
            });
        } else if !(line.is_empty() || line.starts_with('#') || line.starts_with("TEXT=")) {
            return Err("MIN, FROM, TEXT 줄과 # 주석만 있어야 합니다".to_owned());
        }
    }
    Ok(Current {
        min: min.flatten(),
        from: from.flatten(),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// `MIN` is what the file has: the file stays as it is.
    Unchanged {
        min: Version,
        from: Option<i64>,
        /// What the operator should know all the same.
        notes: Vec<String>,
    },
    Change {
        min: Version,
        from: i64,
        /// The entries whose deadline set `FROM`.
        basis: Vec<Platform>,
        /// What the operator should know before installing the file.
        notes: Vec<String>,
    },
    /// A higher `MIN` that would refuse less than the present file at some later time: keep the
    /// present file and run `hbbs-conf` again at or after `from` (INV-16).
    Later {
        min: Version,
        from: i64,
        notes: Vec<String>,
    },
}

/// What min-controller.conf should say for `policy`, which follows `previous`, on a server whose
/// file says `current` now.
pub fn decide(
    policy: &Policy,
    previous: Option<&Policy>,
    current: Option<&Current>,
    now: i64,
) -> Result<Decision, String> {
    let min = policy.lowest_min().ok_or("정책에 항목이 없습니다")?;
    let current = current.copied().unwrap_or(Current {
        min: None,
        from: None,
    });
    if current.min == Some(min) {
        // FROM stays. If the root key put off the deadline of the entries at MIN, the file is the
        // operator's to change: the tool only says so.
        let deadline = policy
            .platforms
            .values()
            .filter(|e| e.min == min)
            .filter_map(|e| e.grace_until)
            .max();
        let mut notes = Vec::new();
        if let (Some(deadline), Some(from)) = (deadline, current.from) {
            if deadline > from && deadline > now {
                notes.push(format!(
                    "참고: 정책의 유예 끝({})이 지금 FROM({})보다 늦습니다. 루트 키로 유예를 늦춘 것이라면 \
                     hbbs 파일의 FROM을 손으로 {} 값으로 고치세요",
                    time::format_kst(deadline),
                    time::format_kst(from),
                    time::format_utc(deadline)
                ));
            }
        }
        return Ok(Decision::Unchanged {
            min,
            from: current.from,
            notes,
        });
    }
    // the entries that have just reached the new MIN; if the file was off or by hand, all at MIN
    let at_min =
        |p: &Policy, platform: &Platform| p.platforms.get(platform).is_some_and(|e| e.min == min);
    let mut basis: Vec<Platform> = policy
        .platforms
        .keys()
        .filter(|platform| at_min(policy, platform))
        .filter(|platform| !previous.is_some_and(|prev| at_min(prev, platform)))
        .copied()
        .collect();
    if basis.is_empty() {
        basis = policy
            .platforms
            .keys()
            .filter(|p| at_min(policy, p))
            .copied()
            .collect();
    }
    let derived = basis
        .iter()
        .filter_map(|p| policy.platforms.get(p).and_then(|e| e.grace_until))
        .max()
        .unwrap_or(policy.issued);
    let mut notes = Vec::new();
    // FROM never goes back; with no present MIN the new file is written at once with its own
    let from = match (current.min, current.from) {
        (Some(_), Some(present)) => derived.max(present),
        _ => derived,
    };
    if let Some(current_min) = current.min {
        // said first, whatever happens to FROM: hbbs has one threshold, and it goes down
        if min < current_min {
            notes.push(format!(
                "경고: MIN을 {current_min}에서 {min}(으)로 내립니다. hbbs가 지금 거절하는 {min} 이상 \
                 {current_min} 미만의 컨트롤러를 다시 받게 됩니다. 완화하는 루트 선언이나 더 낮은 min으로 들어온 새 \
                 항목 때문인지 확인하세요"
            ));
        }
        // a raise waits until the new file refuses at every later time at least what the present
        // one refuses (an empty FROM counts as long past)
        let present_from = current.from.unwrap_or(i64::MIN);
        if min > current_min && from > now.max(present_from) {
            notes.push(format!(
                "지금 파일(MIN={current_min})을 그대로 두세요. 새 파일(MIN={min})은 {}부터 거절하므로, 지금 넣으면 그 전까지 \
                 {current_min} 미만도 받게 됩니다. {} 뒤에 hbbs-conf를 다시 실행하세요",
                time::format_kst(from),
                time::format_kst(from)
            ));
            return Ok(Decision::Later { min, from, notes });
        }
        if from > derived && from > now {
            notes.push(format!(
                "FROM은 지금 파일의 값({})을 그대로 둡니다. 이미 알린 시각보다 일찍 거절하지 않기 위해서입니다",
                time::format_kst(from)
            ));
        }
    }
    if from <= now {
        notes.push(format!(
            "FROM({})이 이미 지났습니다. 이 파일을 넣는 즉시 {min} 미만을 거절합니다",
            time::format_kst(from)
        ));
    }
    for (platform, entry) in &policy.platforms {
        if let Some(grace) = entry.grace_until.filter(|g| *g > from && *g > now) {
            notes.push(format!(
                "경고: {platform}의 유예는 {}까지인데 hbbs는 {}부터 {min} 미만을 모두 거절합니다\
                 (hbbs는 플랫폼을 모릅니다)",
                time::format_kst(grace),
                time::format_kst(from)
            ));
        }
    }
    Ok(Decision::Change {
        min,
        from,
        basis,
        notes,
    })
}

/// The rules a text sent to old apps keeps (design 2-5): a 1.5.1 app reconnects forever unless
/// the text has "not allowed", prefixes or mangles some texts, and makes every URL a link.
pub fn check_text(text: &str) -> Result<(), String> {
    let chars = text.chars().count();
    if chars == 0 || chars > 600 || text.chars().any(char::is_control) || text.trim() != text {
        return Err("TEXT는 한 줄, 1-600자여야 합니다".to_owned());
    }
    if !text.contains("version not allowed") {
        return Err(
            "TEXT에 \"version not allowed\"가 있어야 합니다(없으면 1.5.1 앱이 끝없이 다시 연결합니다)".to_owned(),
        );
    }
    if text.starts_with("Failed") {
        return Err("TEXT는 Failed로 시작할 수 없습니다(앱이 꼬리말을 붙입니다)".to_owned());
    }
    // the first word as hbbs and the apps split it: at the ASCII space (1.5.1's msgboxContent)
    if text
        .split(' ')
        .next()
        .is_some_and(|word| word.ends_with("_tip"))
    {
        return Err("TEXT의 첫 단어는 _tip으로 끝날 수 없습니다".to_owned());
    }
    if text.contains(['{', '}']) {
        return Err("TEXT에 중괄호를 쓸 수 없습니다(번역 자리표시자로 읽힙니다)".to_owned());
    }
    if text.to_lowercase().contains("rustdesk") {
        return Err("TEXT에 RustDesk를 쓸 수 없습니다".to_owned());
    }
    // hbbs fills in %MIN% and nothing else: another %NAME% would be shown as it is
    let filled = text.replace("%MIN%", "");
    let mut between: Vec<&str> = filled.split('%').skip(1).collect();
    between.pop();
    if between
        .iter()
        .any(|w| w.len() >= 3 && w.bytes().all(|b| b.is_ascii_alphabetic() || b == b'_'))
    {
        return Err("TEXT에 %MIN% 말고 다른 %이름%이 남아 있습니다".to_owned());
    }
    // the apps link `https?://` up to the next space: a particle glued to a URL breaks the link
    let mut rest = text;
    loop {
        let start = match (rest.find("http://"), rest.find("https://")) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) | (None, Some(a)) => a,
            (None, None) => break,
        };
        let url = &rest[start..];
        let (url, after) = url.split_at(url.find(char::is_whitespace).unwrap_or(url.len()));
        let url = url.trim_end_matches(['.', ',', ';', '!', '?']);
        let url_byte = |b: u8| b.is_ascii_alphanumeric() || b"-._~:/?#@!$&*+,;=%".contains(&b);
        if !url.bytes().all(url_byte) {
            return Err(
                "TEXT의 URL 뒤에는 공백을 둬야 합니다(조사가 붙으면 링크가 깨집니다)".to_owned(),
            );
        }
        rest = after;
    }
    Ok(())
}

/// S1 with the door 2 link for `server` and `code`.
pub fn s1(server: &str, code: &str) -> String {
    S1.replace("%LINK%", &format!("https://{server}/api/hjv/get/{code}/"))
}

/// The file's content.
pub fn conf_text(min: Version, from: Option<i64>, text: &str) -> Result<String, String> {
    check_text(text)?;
    let from_line = match from {
        Some(t) => format!(
            "# 이 시각부터(UTC, RFC 3339. 비우면 바로). {}\nFROM={}\n",
            time::format_kst(t),
            time::format_utc(t)
        ),
        None => "# 이 시각부터(UTC, RFC 3339. 비우면 바로)\nFROM=\n".to_owned(),
    };
    Ok(format!(
        "# hbbs가 10초마다 다시 읽습니다. 고친 뒤 재시작하지 않습니다.\n\
         # 이 버전보다 낮은 컨트롤러를 거절합니다(x.y.z, 비우면 끔)\n\
         MIN={min}\n\
         {from_line}\
         # 1.5.1 앱이 보는 문구. %MIN%은 MIN으로 바뀝니다. \"not allowed\"를 꼭 넣고, URL 뒤에는 공백을 둡니다.\n\
         TEXT={text}\n"
    ))
}

/// A door 2 code: 128 random bits as 32 hex digits.
pub fn new_code() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|_| "시스템에서 난수를 얻지 못했습니다".to_owned())?;
    Ok(crate::trust::hex(&bytes))
}

pub fn is_code(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Door 2's file: the code and the time it closes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Door2 {
    pub code: String,
    pub until: i64,
}

impl Door2 {
    pub fn to_text(&self) -> String {
        format!(
            "CODE={}\nUNTIL={}\n",
            self.code,
            time::format_utc(self.until)
        )
    }

    /// Reads a door2.conf. Errors never show the file's content.
    pub fn parse(text: &str) -> Result<Door2, String> {
        let (mut code, mut until) = (None, None);
        for line in text.lines() {
            let line = line.trim();
            if let Some(value) = line.strip_prefix("CODE=") {
                code = Some(value.trim().to_owned());
            } else if let Some(value) = line.strip_prefix("UNTIL=") {
                until = Some(
                    time::parse_utc(value.trim())
                        .map_err(|_| "door2.conf의 UNTIL이 시각이 아닙니다")?,
                );
            } else if !(line.is_empty() || line.starts_with('#')) {
                return Err("door2.conf에는 CODE, UNTIL 줄과 # 주석만 있어야 합니다".to_owned());
            }
        }
        match (code, until) {
            (Some(code), Some(until)) if is_code(&code) => Ok(Door2 { code, until }),
            (Some(_), Some(_)) => Err("door2.conf의 CODE가 32자리 hex가 아닙니다".to_owned()),
            _ => Err("door2.conf에 CODE나 UNTIL이 없습니다".to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::tests::{sample, t, v};

    const NOW: &str = "2026-11-08T04:00:00Z";

    /// A policy with the given `min` and deadline for each entry.
    fn policy(mins: [(&str, Option<&str>); 4]) -> Policy {
        let mut p = sample();
        for (platform, (min, grace)) in Platform::ALL.into_iter().zip(mins) {
            let e = p.platforms.get_mut(&platform).unwrap();
            e.min = v(min);
            e.latest = v("1.7.0");
            e.grace_until = grace.map(t);
            e.grace_min = grace.map(|_| v("1.0.0"));
        }
        p
    }

    fn cur(min: &str, from: &str) -> Current {
        Current {
            min: Some(v(min)),
            from: Some(t(from)),
        }
    }

    const G1: &str = "2026-11-22T15:00:00Z";
    const G2: &str = "2026-12-06T15:00:00Z";

    #[test]
    fn the_first_enforcement() {
        // seq 1: nothing enforced. seq 2: every entry to 1.6.0 with the same deadline
        let before = policy([("1.5.0", None); 4]);
        let after = policy([("1.6.0", Some(G1)); 4]);
        let d = decide(&after, Some(&before), None, t(NOW)).unwrap();
        assert_eq!(
            d,
            Decision::Change {
                min: v("1.6.0"),
                from: t(G1),
                basis: Platform::ALL.to_vec(),
                notes: Vec::new(),
            }
        );
        // the lowest min counts: one entry still at 1.5.0 keeps MIN there
        let partly = policy([
            ("1.6.0", Some(G1)),
            ("1.6.0", Some(G1)),
            ("1.5.0", None),
            ("1.6.0", Some(G1)),
        ]);
        match decide(&partly, Some(&before), None, t(NOW)).unwrap() {
            Decision::Change { min, .. } => assert_eq!(min, v("1.5.0")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn one_entry_alone_changes_nothing() {
        let before = policy([("1.6.0", Some(G1)); 4]);
        // Windows alone goes to 1.6.1 with a new, later deadline: MIN stays 1.6.0, FROM stays
        let after = policy([
            ("1.6.1", Some(G2)),
            ("1.6.0", Some(G1)),
            ("1.6.0", Some(G1)),
            ("1.6.0", Some(G1)),
        ]);
        let current = cur("1.6.0", G1);
        assert_eq!(
            decide(&after, Some(&before), Some(&current), t(NOW)).unwrap(),
            Decision::Unchanged {
                min: v("1.6.0"),
                from: Some(t(G1)),
                notes: Vec::new(),
            }
        );
        // the root key put off the deadline of the same min: MIN stays, and the tool points at FROM
        let put_off = policy([("1.6.0", Some(G2)); 4]);
        match decide(&put_off, Some(&before), Some(&current), t(NOW)).unwrap() {
            Decision::Unchanged { from, notes, .. } => {
                assert_eq!(from, Some(t(G1)));
                assert_eq!(notes.len(), 1);
                assert!(
                    notes[0].contains("FROM을 손으로 2026-12-06T15:00:00Z 값으로"),
                    "{notes:?}"
                );
            }
            other => panic!("{other:?}"),
        }
        // also after the refusal has begun
        assert!(matches!(
            decide(
                &after,
                Some(&before),
                Some(&current),
                t("2026-12-01T00:00:00Z")
            )
            .unwrap(),
            Decision::Unchanged { .. }
        ));
    }

    #[test]
    fn a_higher_min_sets_a_new_from() {
        let before = policy([
            ("1.6.1", Some(G1)),
            ("1.6.0", Some(G1)),
            ("1.6.0", Some(G1)),
            ("1.6.0", Some(G1)),
        ]);
        // the others follow to 1.6.1: MIN rises, FROM is the deadline of those that just arrived
        let after = policy([
            ("1.6.1", Some(G1)),
            ("1.6.1", Some(G2)),
            ("1.6.1", Some(G2)),
            ("1.6.1", Some(G2)),
        ]);
        let now = t("2026-11-25T00:00:00Z");
        // hbbs refuses below 1.6.0 now: the new file would refuse nobody until G2, so the
        // present one stays and the tool names the time to run again (INV-16)
        match decide(&after, Some(&before), Some(&cur("1.6.0", G1)), now).unwrap() {
            Decision::Later { min, from, notes } => {
                assert_eq!((min, from), (v("1.6.1"), t(G2)));
                assert!(
                    notes[0].contains("지금 파일(MIN=1.6.0)을 그대로 두세요"),
                    "{notes:?}"
                );
                assert!(
                    notes[0].contains("2026-12-07 00:00 한국 시각 뒤에"),
                    "{notes:?}"
                );
            }
            other => panic!("{other:?}"),
        }
        // run again at that time: the file is written
        match decide(&after, Some(&before), Some(&cur("1.6.0", G1)), t(G2)).unwrap() {
            Decision::Change {
                min, from, basis, ..
            } => {
                assert_eq!((min, from), (v("1.6.1"), t(G2)));
                assert_eq!(
                    basis,
                    [Platform::Android, Platform::AndroidPlay, Platform::Ios]
                );
            }
            other => panic!("{other:?}"),
        }
        // the latest deadline of the arriving entries counts
        let mixed = policy([
            ("1.6.1", Some(G1)),
            ("1.6.1", Some(G2)),
            ("1.6.1", Some("2026-11-30T15:00:00Z")),
            ("1.6.1", None),
        ]);
        match decide(&mixed, Some(&before), Some(&cur("1.6.0", G1)), t(G2)).unwrap() {
            Decision::Change { from, .. } => assert_eq!(from, t(G2)),
            other => panic!("{other:?}"),
        }
        // no deadline at all: from the policy's issue time, at once
        let mut at_once = policy([
            ("1.6.1", Some(G1)),
            ("1.6.1", None),
            ("1.6.1", None),
            ("1.6.1", None),
        ]);
        at_once.issued = now;
        match decide(&at_once, Some(&before), Some(&cur("1.6.0", G1)), now).unwrap() {
            Decision::Change { from, .. } => assert_eq!(from, at_once.issued),
            other => panic!("{other:?}"),
        }
        // with no present file the new one is written at once (runbook 5-2)
        assert!(matches!(
            decide(&after, Some(&before), None, now).unwrap(),
            Decision::Change { .. }
        ));
    }

    #[test]
    fn from_never_goes_back() {
        // the refusal of 1.5.x is announced for G2; a new policy must not pull it forward
        let before = policy([("1.6.0", Some(G2)); 4]);
        let after = policy([("1.6.1", Some(G1)); 4]);
        match decide(&after, Some(&before), Some(&cur("1.6.0", G2)), t(NOW)).unwrap() {
            Decision::Change {
                min, from, notes, ..
            } => {
                assert_eq!((min, from), (v("1.6.1"), t(G2)));
                assert!(
                    notes[0].starts_with(
                        "FROM은 지금 파일의 값(2026-12-07 00:00 한국 시각)을 그대로 둡니다"
                    ),
                    "{notes:?}"
                );
            }
            other => panic!("{other:?}"),
        }
        // after that time the same: the file's FROM stays, and it refuses at once
        let after_it = t("2026-12-07T00:00:00Z");
        match decide(&after, Some(&before), Some(&cur("1.6.0", G2)), after_it).unwrap() {
            Decision::Change {
                min, from, notes, ..
            } => {
                assert_eq!((min, from), (v("1.6.1"), t(G2)));
                assert_eq!(notes.len(), 1, "{notes:?}");
                assert!(notes[0].contains("넣는 즉시 1.6.1 미만을 거절합니다"));
            }
            other => panic!("{other:?}"),
        }
        // the same deadline is fine; with no file, or an empty MIN, there is nothing to go back
        // from: the new file has its own FROM
        let same = policy([("1.6.1", Some(G2)); 4]);
        assert!(matches!(
            decide(&same, Some(&before), Some(&cur("1.6.0", G2)), t(NOW)).unwrap(),
            Decision::Change { .. }
        ));
        match decide(&after, Some(&before), None, t(NOW)).unwrap() {
            Decision::Change { from, .. } => assert_eq!(from, t(G1)),
            other => panic!("{other:?}"),
        }
        let off = Current {
            min: None,
            from: Some(t(G2)),
        };
        match decide(&after, Some(&before), Some(&off), t(NOW)).unwrap() {
            Decision::Change { from, .. } => assert_eq!(from, t(G1)),
            other => panic!("{other:?}"),
        }
    }

    /// INV-16: a written file with a higher MIN refuses, at every time from now on, every
    /// version the present file refuses (hbbs refuses below MIN from FROM on, nobody before).
    #[test]
    fn a_written_raise_refuses_no_less_than_the_present_file() {
        let times = [
            "2026-11-01T00:00:00Z",
            G1,
            "2026-11-30T00:00:00Z",
            G2,
            "2026-12-20T00:00:00Z",
        ];
        let refuses = |min: Option<Version>, from: Option<i64>, at: i64, version: Version| {
            min.is_some_and(|m| version < m) && from.is_none_or(|f| at >= f)
        };
        let versions = ["1.5.0", "1.6.0", "1.6.1", "1.6.2"];
        let mut written = 0;
        for present_min in [None, Some("1.6.0"), Some("1.6.1")] {
            for present_from in [None].into_iter().chain(times.map(Some)) {
                for new_min in ["1.6.0", "1.6.1", "1.6.2"] {
                    for deadline in [None].into_iter().chain(times.map(Some)) {
                        for now in times {
                            let mut p = policy([(new_min, deadline); 4]);
                            p.issued = t(now);
                            let current = present_min.map(|m| Current {
                                min: Some(v(m)),
                                from: present_from.map(t),
                            });
                            let Decision::Change { min, from, .. } =
                                decide(&p, None, current.as_ref(), t(now)).unwrap()
                            else {
                                continue;
                            };
                            let Some(present) = current.filter(|c| c.min.is_some_and(|m| min > m))
                            else {
                                continue;
                            };
                            written += 1;
                            if let Some(f) = present.from {
                                assert!(from >= f, "FROM went back");
                            }
                            for at in times
                                .map(t)
                                .into_iter()
                                .chain([t(now)])
                                .filter(|a| *a >= t(now))
                            {
                                for version in versions.map(v) {
                                    assert!(
                                        !refuses(present.min, present.from, at, version)
                                            || refuses(Some(min), Some(from), at, version),
                                        "present {present:?}, wrote ({min}, {from}) at {now}: {version} at {at}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(written > 50, "{written} files written");
    }

    #[test]
    fn a_lowered_min_follows_the_policy() {
        // the root key eased min to 1.6.0 after hbbs had begun to refuse below 1.6.1
        let before = policy([("1.6.1", Some(G1)); 4]);
        let mut eased = policy([("1.6.0", None); 4]);
        eased.issued = t("2026-12-01T03:00:00Z");
        let now = t("2026-12-01T04:00:00Z");
        let lowering = "경고: MIN을 1.6.1에서 1.6.0(으)로 내립니다. hbbs가 지금 거절하는 1.6.0 이상 1.6.1 미만의 \
                        컨트롤러를 다시 받게 됩니다";
        match decide(&eased, Some(&before), Some(&cur("1.6.1", G1)), now).unwrap() {
            Decision::Change {
                min, from, notes, ..
            } => {
                assert_eq!((min, from), (v("1.6.0"), eased.issued));
                assert_eq!(notes.len(), 2, "{notes:?}");
                assert!(notes[0].starts_with(lowering), "{notes:?}");
                assert!(notes[1].contains("넣는 즉시 1.6.0 미만을 거절합니다"));
            }
            other => panic!("{other:?}"),
        }
        // eased while the refusal below 1.6.1 was only announced: the lower MIN takes the file's
        // place with the announced time, never an earlier one, and is not refused for that
        eased.issued = t("2026-11-10T03:00:00Z");
        let early = t("2026-11-10T04:00:00Z");
        match decide(&eased, Some(&before), Some(&cur("1.6.1", G1)), early).unwrap() {
            Decision::Change {
                min, from, notes, ..
            } => {
                assert_eq!((min, from), (v("1.6.0"), t(G1)));
                assert_eq!(notes.len(), 2, "{notes:?}");
                assert!(notes[0].starts_with(lowering), "{notes:?}");
                assert!(
                    notes[1].starts_with(
                        "FROM은 지금 파일의 값(2026-11-23 00:00 한국 시각)을 그대로 둡니다"
                    ),
                    "{notes:?}"
                );
            }
            other => panic!("{other:?}"),
        }
        // the same once that time has passed: the file's time stays, and it refuses at once
        let late = t("2026-11-25T00:00:00Z");
        match decide(&eased, Some(&before), Some(&cur("1.6.1", G1)), late).unwrap() {
            Decision::Change { from, notes, .. } => {
                assert_eq!(from, t(G1));
                assert_eq!(notes.len(), 2, "{notes:?}");
                assert!(notes[0].starts_with(lowering), "{notes:?}");
                assert!(notes[1].starts_with("FROM(2026-11-23 00:00 한국 시각)이 이미 지났습니다"));
            }
            other => panic!("{other:?}"),
        }
        // a file that was refusing at once goes on doing so
        let at_once = Current {
            min: Some(v("1.6.1")),
            from: None,
        };
        match decide(&eased, Some(&before), Some(&at_once), early).unwrap() {
            Decision::Change { from, notes, .. } => {
                assert_eq!(from, eased.issued);
                assert!(notes[0].starts_with(lowering), "{notes:?}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_lower_min_is_always_pointed_out() {
        // Every entry at 1.6.0 and hbbs refusing below it. A signing key adds the Google Play
        // entry with min 1.0.0, which the apps' rules do not call a relaxation (the tool asks
        // the root key for it, trust.rs): the lowest min of all, and so MIN, is 1.0.0 now.
        let before = policy([("1.6.0", None); 4]);
        let mut with_low_entry = policy([
            ("1.6.0", None),
            ("1.6.0", None),
            ("1.0.0", None),
            ("1.6.0", None),
        ]);
        with_low_entry.issued = t(NOW);
        let warned = |d: Decision| match d {
            Decision::Change { min, notes, .. } => {
                assert_eq!(min, v("1.0.0"));
                let warning = "경고: MIN을 1.6.0에서 1.0.0(으)로 내립니다";
                assert!(notes[0].starts_with(warning), "{notes:?}");
                assert!(notes[0].contains("새 항목"), "{notes:?}");
            }
            other => panic!("{other:?}"),
        };
        // whether the file's FROM has passed, is still ahead, or is empty
        for from in ["2026-01-01T00:00:00Z", G2] {
            warned(
                decide(
                    &with_low_entry,
                    Some(&before),
                    Some(&cur("1.6.0", from)),
                    t(NOW),
                )
                .unwrap(),
            );
        }
        let at_once = Current {
            min: Some(v("1.6.0")),
            from: None,
        };
        warned(decide(&with_low_entry, Some(&before), Some(&at_once), t(NOW)).unwrap());
        // a higher MIN, the same MIN and a server without a file lower nothing
        let no_warning = |notes: &[String]| !notes.iter().any(|n| n.contains("내립니다"));
        let higher = policy([("1.6.1", Some(G2)); 4]);
        for current in [Some(cur("1.6.0", G1)), None] {
            match decide(&higher, Some(&before), current.as_ref(), t(NOW)).unwrap() {
                Decision::Change { notes, .. } | Decision::Later { notes, .. } => {
                    assert!(no_warning(&notes), "{notes:?}")
                }
                other => panic!("{other:?}"),
            }
        }
        match decide(&before, None, Some(&cur("1.6.0", G1)), t(NOW)).unwrap() {
            Decision::Unchanged { notes, .. } => assert!(no_warning(&notes), "{notes:?}"),
            other => panic!("{other:?}"),
        }
        match decide(&with_low_entry, Some(&before), None, t(NOW)).unwrap() {
            Decision::Change { notes, .. } => assert!(no_warning(&notes), "{notes:?}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn other_deadlines_are_pointed_out() {
        // Windows went to 1.6.1 two days ago with a long grace; now the rest follows with a short one
        let before = policy([
            ("1.6.1", Some(G2)),
            ("1.6.0", None),
            ("1.6.0", None),
            ("1.6.0", None),
        ]);
        let after = policy([
            ("1.6.1", Some(G2)),
            ("1.6.1", Some(G1)),
            ("1.6.1", Some(G1)),
            ("1.6.1", Some(G1)),
        ]);
        // hbbs refuses below 1.6.0 now: the new file waits for its FROM (INV-16)
        let present = cur("1.6.0", "2026-10-01T00:00:00Z");
        assert!(matches!(
            decide(&after, Some(&before), Some(&present), t(NOW)).unwrap(),
            Decision::Later { .. }
        ));
        match decide(&after, Some(&before), Some(&present), t(G1)).unwrap() {
            Decision::Change { from, notes, .. } => {
                assert_eq!(from, t(G1));
                assert!(
                    notes.iter().any(|n| n.starts_with("경고: windows의 유예")),
                    "{notes:?}"
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn without_a_previous_policy_all_entries_at_min_count() {
        let after = policy([
            ("1.6.1", Some(G2)),
            ("1.6.0", Some(G1)),
            ("1.6.0", None),
            ("1.6.2", Some(G2)),
        ]);
        match decide(&after, None, None, t(NOW)).unwrap() {
            Decision::Change {
                min, from, basis, ..
            } => {
                assert_eq!((min, from), (v("1.6.0"), t(G1)));
                assert_eq!(basis, [Platform::Android, Platform::AndroidPlay]);
            }
            other => panic!("{other:?}"),
        }
        // hbbs was switched off by hand while the policy stayed: nobody has "just arrived"
        match decide(&after, Some(&after), None, t(NOW)).unwrap() {
            Decision::Change { from, basis, .. } => {
                assert_eq!(from, t(G1));
                assert_eq!(basis.len(), 2);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_files() {
        let text = conf_text(
            v("1.6.0"),
            Some(t(G1)),
            &s1("rd.example.com", &"0".repeat(32)),
        )
        .unwrap();
        assert!(text.contains("\nMIN=1.6.0\n"));
        assert!(text.contains("\nFROM=2026-11-22T15:00:00Z\n"));
        assert!(text.contains("2026-11-23 00:00 한국 시각"));
        assert!(text.contains("\nTEXT=HJViewer를 %MIN% 이상으로"));
        assert!(text.contains(
            " https://rd.example.com/api/hjv/get/00000000000000000000000000000000/ 에서"
        ));
        assert!(text.ends_with("(version not allowed)\n"));
        // the tool reads back what it writes, and never needs TEXT for that
        assert_eq!(parse_conf(&text).unwrap(), cur("1.6.0", G1));
        let off = conf_text(v("1.6.0"), None, S2).unwrap();
        assert_eq!(
            parse_conf(&off).unwrap(),
            Current {
                min: Some(v("1.6.0")),
                from: None
            }
        );
        assert_eq!(
            parse_conf(&format!("\u{feff}{text}")).unwrap(),
            cur("1.6.0", G1)
        );
        assert_eq!(
            parse_conf("").unwrap(),
            Current {
                min: None,
                from: None
            }
        );
        assert_eq!(
            parse_conf("MIN=\nFROM=\nTEXT=x\n").unwrap(),
            Current {
                min: None,
                from: None
            }
        );
        for bad in [
            "MIN=1.6\n",
            "MIN=1.6.0\nMIN=1.6.1\n",
            "FROM=soon\n",
            "MIN=1.6.0\nOTHER=1\n",
            "FROM=\nFROM=\n",
        ] {
            assert!(parse_conf(bad).is_err(), "{bad:?}");
        }
        // an error about a file with a link in it does not repeat the file
        let secret = "MIN=1.6.0\nTEXT=https://rd.example.com/api/hjv/get/0123456789abcdef0123456789abcdef/\nBAD\n";
        assert!(!parse_conf(secret).unwrap_err().contains("0123456789abcdef"));
    }

    #[test]
    fn the_present_file_is_read_in_the_tool_s_form() {
        // the forms hbbs's RFC 3339 reader also takes are refused, with the form the tool writes
        for (line, hint) in [
            ("FROM=2026-11-23T00:00:00+09:00", "2026-11-22T15:00:00Z"),
            ("FROM=2026-11-22T15:00:00.000Z", "2026-11-22T15:00:00Z"),
            ("FROM=2026-11-22t15:00:00z", "2026-11-22T15:00:00Z"),
            ("FROM=2026-11-22 15:00:00Z", "2026-11-22T15:00:00Z"),
            ("MIN=01.6.0", "1.6.0"),
            ("MIN=1.06.0", "1.6.0"),
        ] {
            let text = if line.starts_with("MIN") {
                format!("{line}\nFROM=2026-11-22T15:00:00Z\n")
            } else {
                format!("MIN=1.6.0\n{line}\n")
            };
            let err = parse_conf(&text).unwrap_err();
            assert!(err.contains(hint), "{line}: {err}");
        }
        assert_eq!(
            parse_conf("MIN=1.6.0\nFROM=2026-11-22T15:00:00Z\nTEXT=x\n").unwrap(),
            Current {
                min: Some(v("1.6.0")),
                from: Some(t("2026-11-22T15:00:00Z"))
            }
        );
    }

    #[test]
    fn text_rules() {
        check_text(S2).unwrap();
        check_text(&s1("rd.example.com", &"a".repeat(32))).unwrap();
        check_text(&s1("rd.example.com:8443", &"a".repeat(32))).unwrap();
        for bad in [
            "",
            "업데이트하세요.",
            "업데이트하세요. (version no longer allowed)",
            "Failed: 업데이트하세요. (version not allowed)",
            "update_tip 업데이트하세요. (version not allowed)",
            // the first word as hbbs and the apps split it: at the ASCII space only
            "x\u{3000}y_tip now (version not allowed)",
            "{0} 이상으로 업데이트하세요. (version not allowed)",
            "RustDesk를 업데이트하세요. (version not allowed)",
            "https://rd.example.com/x에서 받으세요. (version not allowed)",
            "http://rd.example.com/x를 여세요. (version not allowed)",
            "두 줄\n(version not allowed)",
            " 앞 공백 (version not allowed)",
        ] {
            assert!(check_text(bad).is_err(), "{bad:?} passed");
        }
        for good in [
            "https://rd.example.com/x 에서 받으세요. (version not allowed)",
            "https://rd.example.com/x. 받으세요. (version not allowed)",
            "https://rd.example.com/a%20b 에서 받으세요. (version not allowed)",
            "%MIN% 이상(100% 필요) (version not allowed)",
            "y_tip\u{3000}now (version not allowed)",
        ] {
            assert!(check_text(good).is_ok(), "{good:?} refused");
        }
        // a misspelt placeholder would reach the apps as it is
        assert!(check_text("%min% 이상으로 업데이트하세요. (version not allowed)").is_err());
        assert!(check_text("%MIN% 이상, %LINK% 에서 받으세요. (version not allowed)").is_err());
        assert!(check_text(&format!("{} (version not allowed)", "가".repeat(600))).is_err());
        assert!(conf_text(v("1.6.0"), None, "업데이트하세요.").is_err());
    }

    #[test]
    fn door_2() {
        let code = new_code().unwrap();
        assert!(is_code(&code));
        assert_ne!(code, new_code().unwrap());
        let door = Door2 {
            code: code.clone(),
            until: t("2026-12-22T15:00:00Z"),
        };
        assert_eq!(
            door.to_text(),
            format!("CODE={code}\nUNTIL=2026-12-22T15:00:00Z\n")
        );
        assert_eq!(Door2::parse(&door.to_text()).unwrap(), door);
        assert_eq!(
            Door2::parse(&format!("# door 2\n{}", door.to_text())).unwrap(),
            door
        );
        for bad in [
            String::new(),
            format!("CODE={code}\n"),
            "UNTIL=2026-12-22T15:00:00Z\n".to_owned(),
            format!("CODE={}\nUNTIL=2026-12-22T15:00:00Z\n", &code[..31]),
            format!(
                "CODE={}\nUNTIL=2026-12-22T15:00:00Z\n",
                code.to_uppercase()
                    .replace(|c: char| c.is_ascii_digit(), "A")
            ),
            format!("CODE={code}\nUNTIL=later\n"),
            format!("CODE={code}\nUNTIL=2026-12-22T15:00:00Z\nextra\n"),
        ] {
            let err = Door2::parse(&bad).unwrap_err();
            assert!(!err.contains(&code), "the error shows the code");
        }
        assert!(!is_code(&"g".repeat(32)) && !is_code(&"A".repeat(32)) && !is_code(""));
    }
}

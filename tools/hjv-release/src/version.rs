// SPDX-License-Identifier: AGPL-3.0-only

//! App versions as the policy writes them: `x.y.z`, three numbers and nothing else.
//!
//! The apps and hbbs compare versions with RustDesk's `get_version_number`, which reads "1.6" as
//! 1.0.60, "v1.6.0" as 0.6.0 and "1.5.100" as 1.6.0. So the tool takes only the form on which that
//! function and plain comparison agree: no leading zeros, the first two numbers below 1000 and the
//! last below 100.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

impl Version {
    pub const fn new(major: u16, minor: u16, patch: u16) -> Version {
        Version {
            major,
            minor,
            patch,
        }
    }

    pub fn parse(s: &str) -> Result<Version, String> {
        let bad = || {
            format!(
                "버전은 1.6.0처럼 숫자 셋이어야 합니다(앞의 0 없이, 마지막은 두 자리까지): {s:?}"
            )
        };
        let field = |text: &str, max_len: usize| -> Option<u16> {
            let canonical = !text.is_empty()
                && text.len() <= max_len
                && text.bytes().all(|b| b.is_ascii_digit())
                && (text == "0" || !text.starts_with('0'));
            canonical.then(|| text.parse().ok()).flatten()
        };
        let mut parts = s.split('.');
        let version = (|| {
            let version = Version {
                major: field(parts.next()?, 3)?,
                minor: field(parts.next()?, 3)?,
                patch: field(parts.next()?, 2)?,
            };
            parts.next().is_none().then_some(version)
        })();
        version.ok_or_else(bad)
    }

    /// What `hbb_common::get_version_number` makes of this version: the number hbbs and the apps
    /// compare.
    pub fn number(&self) -> i64 {
        (i64::from(self.major) * 1000 + i64::from(self.minor)) * 1000 + i64::from(self.patch) * 10
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `hbb_common::get_version_number` (upstream/rustdesk/libs/hbb_common/src/lib.rs), verbatim.
    fn get_version_number(v: &str) -> i64 {
        let mut versions = v.split('-');
        let mut n = 0;
        if let Some(v) = versions.next() {
            let mut last = 0;
            for x in v.split('.') {
                last = x.parse::<i64>().unwrap_or(0);
                n = n * 1000 + last;
            }
            n -= last;
            n += last * 10;
        }
        if let Some(v) = versions.next() {
            n += v.parse::<i64>().unwrap_or(0);
        }
        n
    }

    #[test]
    fn strict_form() {
        assert_eq!(Version::parse("1.6.0").unwrap(), Version::new(1, 6, 0));
        assert_eq!(Version::parse("0.0.0").unwrap(), Version::new(0, 0, 0));
        assert_eq!(
            Version::parse("999.999.99").unwrap(),
            Version::new(999, 999, 99)
        );
        assert_eq!(Version::parse("1.10.0").unwrap().to_string(), "1.10.0");
        for bad in [
            "", "1", "1.6", "v1.6.0", "1.6.0.1", "1.6.0+1", "1.6.0-1", "1.5.100", "1.06.0",
            "01.6.0", "1.6.00", "1.6.0 ", " 1.6.0", "1..0", "1.6.", ".6.0", "1.6.x", "1000.0.0",
            "1.1000.0", "1,6,0", "１.6.0", "+1.6.0", "1.6.-0",
        ] {
            assert!(Version::parse(bad).is_err(), "{bad:?} parsed");
        }
    }

    #[test]
    fn order_is_the_apps_order() {
        let ordered = [
            "0.0.0",
            "0.0.1",
            "1.4.3",
            "1.5.0",
            "1.5.1",
            "1.5.99",
            "1.6.0",
            "1.6.1",
            "1.9.9",
            "1.10.0",
            "2.0.0",
            "10.0.0",
            "999.999.99",
        ];
        for pair in ordered.windows(2) {
            let (a, b) = (
                Version::parse(pair[0]).unwrap(),
                Version::parse(pair[1]).unwrap(),
            );
            assert!(a < b, "{a} < {b}");
            assert!(a.number() < b.number(), "{a} < {b} as numbers");
        }
        for v in ordered {
            let version = Version::parse(v).unwrap();
            assert_eq!(version.to_string(), v);
            assert_eq!(version.number(), get_version_number(v), "{v}");
        }
    }
}

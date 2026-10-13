// SPDX-License-Identifier: AGPL-3.0-only

//! Command line arguments: `--name value`, `--name=value`, flags and positional arguments. Every
//! command names the options it knows; anything else is an error, never ignored.

use crate::{
    make::{AutoAfter, Grace, Target},
    policy::Platform,
    time::{self, DAY, HOUR},
    version::Version,
};

#[derive(Debug)]
pub struct Args {
    values: Vec<(String, String)>,
    flags: Vec<String>,
    pub positional: Vec<String>,
}

impl Args {
    /// Sorts `args` into the options that take a value, the flags and the rest.
    pub fn parse(args: &[String], with_value: &[&str], flags: &[&str]) -> Result<Args, String> {
        let mut parsed = Args {
            values: Vec::new(),
            flags: Vec::new(),
            positional: Vec::new(),
        };
        let mut rest = args.iter();
        while let Some(arg) = rest.next() {
            let (name, inline) = match arg.split_once('=') {
                Some((name, value)) if name.starts_with("--") => (name, Some(value)),
                _ => (arg.as_str(), None),
            };
            if with_value.contains(&name) {
                let value = match inline {
                    Some(value) => value.to_owned(),
                    None => rest
                        .next()
                        .cloned()
                        .ok_or_else(|| format!("{name}에는 값이 필요합니다"))?,
                };
                parsed.values.push((name.to_owned(), value));
            } else if flags.contains(&name) {
                if inline.is_some() {
                    return Err(format!("{name}에는 값을 주지 않습니다"));
                }
                if parsed.flags.iter().any(|f| f == name) {
                    return Err(format!("{name}: 두 번 줬습니다"));
                }
                parsed.flags.push(name.to_owned());
            } else if arg.starts_with('-') && arg != "-" {
                return Err(format!("모르는 옵션: {name}"));
            } else {
                parsed.positional.push(arg.clone());
            }
        }
        Ok(parsed)
    }

    pub fn flag(&self, name: &str) -> bool {
        self.flags.iter().any(|f| f == name)
    }

    /// Every value of an option that may repeat.
    pub fn all(&self, name: &str) -> Vec<&str> {
        self.values
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
            .collect()
    }

    /// The value of an option that may appear once.
    pub fn one(&self, name: &str) -> Result<Option<&str>, String> {
        match self.all(name).as_slice() {
            [] => Ok(None),
            [value] => Ok(Some(value)),
            _ => Err(format!("{name}: 두 번 줬습니다")),
        }
    }

    pub fn need(&self, name: &str) -> Result<&str, String> {
        self.one(name)?
            .ok_or_else(|| format!("{name} 옵션이 필요합니다"))
    }

    /// The one positional argument of a command.
    pub fn file(&self, what: &str) -> Result<&str, String> {
        match self.positional.as_slice() {
            [file] => Ok(file),
            [] => Err(format!("{what}: 하나를 주세요")),
            _ => Err(format!("{what}: 하나만 줍니다")),
        }
    }

    pub fn no_positional(&self) -> Result<(), String> {
        match self.positional.first() {
            None => Ok(()),
            Some(arg) => Err(format!("남는 인자: {arg}")),
        }
    }
}

pub fn platform(s: &str) -> Result<Platform, String> {
    Platform::parse(s)
        .ok_or_else(|| format!("항목은 windows, android, android_play, ios 중 하나입니다: {s:?}"))
}

/// `<platform>=<value>`.
pub fn platform_value<'a>(option: &str, s: &'a str) -> Result<(Platform, &'a str), String> {
    let (p, value) = s
        .split_once('=')
        .ok_or_else(|| format!("{option}: <항목>=<값> 형식으로 씁니다({s:?})"))?;
    Ok((platform(p)?, value))
}

/// `<platform>=<value>`, `all=<value>`, or just `<value>` for all (when `bare` allows it).
pub fn target_value<'a>(option: &str, s: &'a str, bare: bool) -> Result<(Target, &'a str), String> {
    match s.split_once('=') {
        Some(("all", value)) => Ok((Target::All, value)),
        Some((p, value)) if Platform::parse(p).is_some() => Ok((Target::One(platform(p)?), value)),
        _ if bare => Ok((Target::All, s)),
        _ => Err(format!(
            "{option}: <항목>=<값> 또는 all=<값> 형식으로 씁니다({s:?})"
        )),
    }
}

pub fn number(option: &str, s: &str) -> Result<u64, String> {
    let canonical = !s.is_empty()
        && s.len() <= 18
        && s.bytes().all(|b| b.is_ascii_digit())
        && (s == "0" || !s.starts_with('0'));
    canonical
        .then(|| s.parse().ok())
        .flatten()
        .ok_or_else(|| format!("{option}: 숫자를 주세요({s:?})"))
}

pub fn min(s: &str) -> Result<(Target, Version), String> {
    let (target, value) = target_value("--min", s, false)?;
    Ok((target, Version::parse(value)?))
}

pub fn latest(s: &str) -> Result<(Platform, Version), String> {
    let (platform, value) = platform_value("--latest", s)?;
    Ok((platform, Version::parse(value)?))
}

pub fn grace_days(s: &str) -> Result<(Target, Grace), String> {
    let (target, value) = target_value("--grace-days", s, true)?;
    let days = number("--grace-days", value)?;
    if days > 365 {
        return Err(format!("--grace-days는 0-365입니다: {days}"));
    }
    Ok((target, Grace::Days(days as i64)))
}

pub fn grace_until(s: &str) -> Result<(Target, Grace), String> {
    let (target, value) = target_value("--grace-until", s, true)?;
    Ok((target, Grace::Until(time::parse_operator_time(value)?)))
}

/// `windows=+2d`, `windows=+36h`, `windows=<time>` or `windows=none`.
pub fn auto_after(s: &str) -> Result<AutoAfter, String> {
    let (platform, value) = platform_value("--auto-after", s)?;
    if platform != Platform::Windows {
        return Err("--auto-after는 windows에만 씁니다(예: --auto-after windows=+2d)".to_owned());
    }
    if value == "none" {
        return Ok(AutoAfter::Off);
    }
    if let Some(span) = value.strip_prefix('+') {
        let (amount, unit) = span.split_at(span.len().saturating_sub(1));
        let seconds = match unit {
            "d" => DAY,
            "h" => HOUR,
            _ => {
                return Err(format!(
                    "--auto-after의 기간은 +2d나 +36h처럼 씁니다: {value:?}"
                ))
            }
        };
        let amount = number("--auto-after", amount)?;
        if amount > 2000 {
            return Err(format!("--auto-after의 기간이 너무 깁니다: {value:?}"));
        }
        return Ok(AutoAfter::In(amount as i64 * seconds));
    }
    Ok(AutoAfter::At(time::parse_operator_time(value)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn options_flags_and_files() {
        let args = Args::parse(
            &strings(&[
                "--key",
                "a.key",
                "draft.json",
                "--key=b.key",
                "--relax",
                "--out",
                "o",
            ]),
            &["--key", "--out"],
            &["--relax", "--new-epoch"],
        )
        .unwrap();
        assert_eq!(args.all("--key"), ["a.key", "b.key"]);
        assert_eq!(args.one("--out").unwrap(), Some("o"));
        assert_eq!(args.one("--missing").unwrap(), None);
        assert!(args.one("--key").is_err());
        assert!(args.need("--missing").is_err());
        assert!(args.flag("--relax") && !args.flag("--new-epoch"));
        assert_eq!(args.file("초안 파일").unwrap(), "draft.json");
        assert!(args.no_positional().is_err());

        let parse = |a: &[&str]| Args::parse(&strings(a), &["--key"], &["--relax"]);
        assert!(parse(&["--key"]).unwrap_err().contains("값이 필요"));
        assert!(parse(&["--unknown"]).unwrap_err().contains("모르는 옵션"));
        assert!(parse(&["--unknown=1"])
            .unwrap_err()
            .contains("모르는 옵션: --unknown"));
        assert!(parse(&["-k"]).unwrap_err().contains("모르는 옵션"));
        assert!(parse(&["--relax=yes"]).is_err());
        assert!(parse(&["--relax", "--relax"]).is_err());
        assert!(parse(&[]).unwrap().file("파일").is_err());
        assert!(parse(&["a", "b"]).unwrap().file("파일").is_err());
        // a value may start with a dash or hold an equals sign
        let args = parse(&["--key", "-", "--key", "a=b"]).unwrap();
        assert_eq!(args.all("--key"), ["-", "a=b"]);
    }

    #[test]
    fn typed_values() {
        let v = |s: &str| Version::parse(s).unwrap();
        assert_eq!(min("all=1.6.0").unwrap(), (Target::All, v("1.6.0")));
        assert_eq!(
            min("ios=1.6.1").unwrap(),
            (Target::One(Platform::Ios), v("1.6.1"))
        );
        assert!(min("1.6.0").is_err());
        assert!(min("linux=1.6.0").is_err());
        assert!(min("ios=1.6").is_err());
        assert_eq!(
            latest("android_play=1.6.1").unwrap(),
            (Platform::AndroidPlay, v("1.6.1"))
        );
        assert!(latest("all=1.6.1").is_err());

        assert_eq!(grace_days("14").unwrap(), (Target::All, Grace::Days(14)));
        assert_eq!(
            grace_days("windows=0").unwrap(),
            (Target::One(Platform::Windows), Grace::Days(0))
        );
        assert!(grace_days("366").is_err());
        assert!(grace_days("-1").is_err());
        assert!(grace_days("two").is_err());
        let midnight = time::parse_utc("2026-11-22T15:00:00Z").unwrap();
        assert_eq!(
            grace_until("2026-11-23").unwrap(),
            (Target::All, Grace::Until(midnight))
        );
        assert_eq!(
            grace_until("ios=2026-11-22T15:00:00Z").unwrap(),
            (Target::One(Platform::Ios), Grace::Until(midnight))
        );
        assert!(grace_until("next week").is_err());

        assert_eq!(auto_after("windows=+2d").unwrap(), AutoAfter::In(2 * DAY));
        assert_eq!(
            auto_after("windows=+36h").unwrap(),
            AutoAfter::In(36 * HOUR)
        );
        assert_eq!(auto_after("windows=+0d").unwrap(), AutoAfter::In(0));
        assert_eq!(auto_after("windows=none").unwrap(), AutoAfter::Off);
        assert_eq!(
            auto_after("windows=2026-11-23").unwrap(),
            AutoAfter::At(midnight)
        );
        for bad in [
            "+2d",
            "android=+2d",
            "windows=2d",
            "windows=+2w",
            "windows=+d",
            "windows=+-2d",
            "windows=",
            "windows=+9999d",
        ] {
            assert!(auto_after(bad).is_err(), "{bad:?} parsed");
        }
        assert_eq!(number("--seq", "12").unwrap(), 12);
        for bad in ["", "012", "+1", "1.0", "1e3", "１"] {
            assert!(number("--seq", bad).is_err(), "{bad:?} parsed");
        }
    }
}

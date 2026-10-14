// SPDX-License-Identifier: AGPL-3.0-only

//! Times of the policy: RFC 3339 UTC to the second (`2026-11-22T15:00:00Z`), Korean time for the
//! deadlines people read, and the build number CI gives iOS and Android builds (`YYDDDHHMM`, UTC).

pub const HOUR: i64 = 3600;
pub const DAY: i64 = 86_400;
/// Korea Standard Time: UTC+9 all year.
pub const KST: i64 = 9 * HOUR;

/// Days from 1970-01-01 to a date of the Gregorian calendar.
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (i64::from(month) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date `days` after 1970-01-01: (year, month, day).
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if leap(year) => 29,
        _ => 28,
    }
}

fn digits(s: &[u8]) -> Option<i64> {
    if s.is_empty() || !s.iter().all(u8::is_ascii_digit) {
        return None;
    }
    s.iter().try_fold(0i64, |n, b| {
        n.checked_mul(10)?.checked_add(i64::from(b - b'0'))
    })
}

/// `YYYY-MM-DD` as days from 1970-01-01; years 0001-9999, as an app reads them (trust rules 1-2).
fn parse_date(s: &[u8]) -> Option<i64> {
    if s.len() != 10 || s[4] != b'-' || s[7] != b'-' {
        return None;
    }
    let (year, month, day) = (digits(&s[..4])?, digits(&s[5..7])?, digits(&s[8..])?);
    let (month, day) = (u32::try_from(month).ok()?, u32::try_from(day).ok()?);
    if year < 1 || !(1..=12).contains(&month) {
        return None;
    }
    if day < 1 || day > days_in_month(year, month) {
        return None;
    }
    Some(days_from_civil(year, month, day))
}

/// `HH:MM:SS` as seconds of the day (no leap second).
fn parse_clock(s: &[u8]) -> Option<i64> {
    if s.len() != 8 || s[2] != b':' || s[5] != b':' {
        return None;
    }
    let (h, m, sec) = (digits(&s[..2])?, digits(&s[3..5])?, digits(&s[6..])?);
    (h < 24 && m < 60 && sec < 60).then_some(h * HOUR + m * 60 + sec)
}

/// The one form the policy uses and the tool writes: `YYYY-MM-DDTHH:MM:SSZ`, as Unix seconds. Years
/// 0001-9999, as an app reads them: the tool's own years are [`check_tool_years`].
pub fn parse_utc(s: &str) -> Result<i64, String> {
    let b = s.as_bytes();
    let parsed = (b.len() == 20 && b[10] == b'T' && b[19] == b'Z')
        .then(|| Some(parse_date(&b[..10])? * DAY + parse_clock(&b[11..19])?))
        .flatten();
    parsed.ok_or_else(|| format!("시각은 2026-11-22T15:00:00Z 형식이어야 합니다: {s:?}"))
}

/// The first second of 2000 and of 2200 (UTC).
const TOOL_YEARS: (i64, i64) = (946_684_800, 7_258_118_400);

/// The tool's own rule for a time it writes or signs (trust rules 1-2, `binding: tool`): years
/// 2000-2199. An app reads every year from 0001 to 9999 ([`parse_utc`]).
pub fn check_tool_years(what: &str, t: i64) -> Result<(), String> {
    if (TOOL_YEARS.0..TOOL_YEARS.1).contains(&t) {
        Ok(())
    } else {
        Err(format!(
            "{what}의 연도는 2000-2199년이어야 합니다(도구의 규칙): {}",
            format_utc(t)
        ))
    }
}

/// What an operator may type: the policy's form, the same with an offset (`+09:00`), or a bare
/// date, which means 00:00 Korean time on that day.
pub fn parse_operator_time(s: &str) -> Result<i64, String> {
    let b = s.as_bytes();
    let bad = || {
        format!("시각은 2026-11-23(한국 시각 0시), 2026-11-22T15:00:00Z, 2026-11-23T00:00:00+09:00 중 하나로 씁니다: {s:?}")
    };
    if b.len() == 10 {
        return parse_date(b).map(|d| d * DAY - KST).ok_or_else(bad);
    }
    if b.len() == 20 {
        return parse_utc(s).map_err(|_| bad());
    }
    if b.len() == 25 && b[10] == b'T' && (b[19] == b'+' || b[19] == b'-') && b[22] == b':' {
        let local = parse_date(&b[..10])
            .zip(parse_clock(&b[11..19]))
            .map(|(d, c)| d * DAY + c);
        let offset = digits(&b[20..22])
            .zip(digits(&b[23..]))
            .filter(|(h, m)| *h < 24 && *m < 60)
            .map(|(h, m)| h * HOUR + m * 60);
        if let (Some(local), Some(offset)) = (local, offset) {
            return Ok(if b[19] == b'+' {
                local - offset
            } else {
                local + offset
            });
        }
    }
    Err(bad())
}

/// `YYYY-MM-DDTHH:MM:SSZ`.
pub fn format_utc(t: i64) -> String {
    let (year, month, day) = civil_from_days(t.div_euclid(DAY));
    let s = t.rem_euclid(DAY);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        s / HOUR,
        s % HOUR / 60,
        s % 60
    )
}

/// Korean time for people: `2026-11-23 00:00 한국 시각`.
pub fn format_kst(t: i64) -> String {
    let local = t + KST;
    let (year, month, day) = civil_from_days(local.div_euclid(DAY));
    let s = local.rem_euclid(DAY);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} 한국 시각",
        s / HOUR,
        s % HOUR / 60
    )
}

/// 00:00 Korean time on the day `days` days after `now`'s Korean date: a grace period of 14 days
/// given on November 9 ends when November 23 begins in Korea.
pub fn kst_midnight_after(now: i64, days: i64) -> i64 {
    ((now + KST).div_euclid(DAY) + days) * DAY - KST
}

/// The time in a CI build number (`date -u +%y%j%H%M`: two-digit year, day of the year, hour and
/// minute, UTC), which is the iOS CFBundleVersion.
pub fn build_number_time(build: &str) -> Result<i64, String> {
    let b = build.as_bytes();
    let bad = || format!("빌드 번호는 YYDDDHHMM 숫자 9자리여야 합니다: {build:?}");
    if b.len() != 9 {
        return Err(bad());
    }
    let parts = (
        digits(&b[..2]),
        digits(&b[2..5]),
        digits(&b[5..7]),
        digits(&b[7..]),
    );
    let (Some(yy), Some(day), Some(hour), Some(minute)) = parts else {
        return Err(bad());
    };
    let year = 2000 + yy;
    let last_day = if leap(year) { 366 } else { 365 };
    if day < 1 || day > last_day || hour > 23 || minute > 59 {
        return Err(bad());
    }
    Ok((days_from_civil(year, 1, 1) + day - 1) * DAY + hour * HOUR + minute * 60)
}

/// The system clock, in Unix seconds.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A span for people: `13일 14시간`, `5시간`, `40분`.
pub fn format_span(seconds: i64) -> String {
    let s = seconds.abs();
    if s >= DAY {
        match s % DAY / HOUR {
            0 => format!("{}일", s / DAY),
            h => format!("{}일 {h}시간", s / DAY),
        }
    } else if s >= HOUR {
        format!("{}시간", s / HOUR)
    } else {
        format!("{}분", s / 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_round_trip() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        for days in (10_000..80_000).step_by(37) {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
            assert!((1..=12).contains(&m) && d >= 1 && d <= days_in_month(y, m));
        }
    }

    #[test]
    fn utc_text() {
        // date -u -d @1793577600
        assert_eq!(format_utc(1_793_577_600), "2026-11-02T00:00:00Z");
        assert_eq!(parse_utc("2026-11-02T00:00:00Z").unwrap(), 1_793_577_600);
        assert_eq!(parse_utc("2026-11-22T15:00:00Z").unwrap(), 1_795_359_600);
        assert_eq!(format_utc(1_795_359_600), "2026-11-22T15:00:00Z");
        assert_eq!(parse_utc("2028-02-29T23:59:59Z").unwrap() % DAY, DAY - 1);
        for bad in [
            "",
            "2026-11-02",
            "2026-11-02T00:00:00",
            "2026-11-02T00:00:00z",
            "2026-11-02 00:00:00Z",
            "2026-11-02T00:00:00+00:00",
            "2026-11-02T00:00:00.000Z",
            "2026-02-29T00:00:00Z",
            "2026-13-01T00:00:00Z",
            "2026-00-10T00:00:00Z",
            "2026-11-31T00:00:00Z",
            "2026-11-02T24:00:00Z",
            "2026-11-02T00:60:00Z",
            "2026-11-02T00:00:60Z",
            "0000-12-31T23:59:59Z",
            "2026-1-02T00:00:00ZZ",
            "２０２６-11-02T00:00:00Z",
            "2026-11-0２T00:00:00Z",
        ] {
            assert!(parse_utc(bad).is_err(), "{bad:?} parsed");
        }
    }

    /// An app reads the years 0001-9999 (trust rules 1-2); the tool writes and signs 2000-2199 only.
    #[test]
    fn the_apps_years_and_the_tools() {
        for (text, tool) in [
            ("0001-01-01T00:00:00Z", false),
            ("1999-12-31T23:59:59Z", false),
            ("2000-01-01T00:00:00Z", true),
            ("2199-12-31T23:59:59Z", true),
            ("2200-01-01T00:00:00Z", false),
            ("9999-12-31T23:59:59Z", false),
        ] {
            let t = parse_utc(text).unwrap();
            assert_eq!(format_utc(t), text);
            assert_eq!(check_tool_years("issued", t).is_ok(), tool, "{text}");
        }
        assert_eq!(
            check_tool_years("issued", parse_utc("1999-12-31T23:59:59Z").unwrap()).unwrap_err(),
            "issued의 연도는 2000-2199년이어야 합니다(도구의 규칙): 1999-12-31T23:59:59Z"
        );
        assert!(parse_utc("0000-01-01T00:00:00Z").is_err());
    }

    #[test]
    fn operator_times() {
        let midnight = parse_utc("2026-11-22T15:00:00Z").unwrap();
        assert_eq!(parse_operator_time("2026-11-23").unwrap(), midnight);
        assert_eq!(
            parse_operator_time("2026-11-22T15:00:00Z").unwrap(),
            midnight
        );
        assert_eq!(
            parse_operator_time("2026-11-23T00:00:00+09:00").unwrap(),
            midnight
        );
        assert_eq!(
            parse_operator_time("2026-11-22T10:00:00-05:00").unwrap(),
            midnight
        );
        for bad in [
            "2026-11-23T00:00:00+0900",
            "2026-11-23T00:00:00+24:00",
            "2026-11-23T00:00:00+09:60",
            "2026/11/23",
            "tomorrow",
            "",
        ] {
            assert!(parse_operator_time(bad).is_err(), "{bad:?} parsed");
        }
    }

    #[test]
    fn korean_time() {
        let midnight = parse_utc("2026-11-22T15:00:00Z").unwrap();
        assert_eq!(format_kst(midnight), "2026-11-23 00:00 한국 시각");
        // any time on November 9 in Korea, plus 14 days: the start of November 23 in Korea
        for now in [
            "2026-11-08T15:00:00Z",
            "2026-11-09T01:00:00Z",
            "2026-11-09T14:59:59Z",
        ] {
            assert_eq!(kst_midnight_after(parse_utc(now).unwrap(), 14), midnight);
        }
        assert_eq!(
            kst_midnight_after(parse_utc("2026-11-09T15:00:00Z").unwrap(), 14),
            midnight + DAY
        );
    }

    #[test]
    fn build_numbers() {
        // 1.5.1 was built with 262840251: day 284 of 2026 is October 11
        assert_eq!(
            format_utc(build_number_time("262840251").unwrap()),
            "2026-10-11T02:51:00Z"
        );
        assert_eq!(
            format_utc(build_number_time("280010000").unwrap()),
            "2028-01-01T00:00:00Z"
        );
        assert_eq!(
            format_utc(build_number_time("283662359").unwrap()),
            "2028-12-31T23:59:00Z"
        );
        for bad in [
            "26284025",
            "2628402510",
            "263660000",
            "260000000",
            "262842451",
            "262840260",
            "26284025x",
            "",
        ] {
            assert!(build_number_time(bad).is_err(), "{bad:?} parsed");
        }
    }

    #[test]
    fn spans() {
        assert_eq!(format_span(14 * DAY), "14일");
        assert_eq!(format_span(13 * DAY + 14 * HOUR + 5), "13일 14시간");
        assert_eq!(format_span(5 * HOUR + 59 * 60), "5시간");
        assert_eq!(format_span(40 * 60), "40분");
    }
}

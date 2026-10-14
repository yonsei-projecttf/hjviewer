// SPDX-License-Identifier: AGPL-3.0-only

//! Small helpers the modules share: the clock, random bytes, hashes, base64, address networks,
//! and the masking of admin output.

use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use crypto_box::aead::{rand_core::RngCore, OsRng};
use sha2::{Digest, Sha256};
use std::{
    net::{IpAddr, Ipv6Addr},
    time::{SystemTime, UNIX_EPOCH},
};

pub const MINUTE: i64 = 60;
pub const HOUR: i64 = 60 * MINUTE;
pub const DAY: i64 = 24 * HOUR;

/// Seconds since the Unix epoch.
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Bytes from the operating system's random source.
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    OsRng.fill_bytes(&mut bytes);
    bytes
}

/// A random number below `bound` (for delays; the slight modulo bias does not matter there).
pub fn random_below(bound: u64) -> u64 {
    if bound == 0 {
        return 0;
    }
    u64::from_le_bytes(random_bytes::<8>()) % bound
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// Standard base64 with padding, as the app's `encode64`.
pub fn b64(data: &[u8]) -> String {
    STANDARD.encode(data)
}

pub fn b64_decode(text: &str) -> Option<Vec<u8>> {
    STANDARD.decode(text).ok()
}

/// URL-safe base64 without padding (tokens, sealed passwords).
pub fn b64url(data: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(data)
}

pub fn b64url_decode(text: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(text).ok()
}

pub fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

/// What admin listings show of a portal or device ID: the first four characters, and no more than
/// half of a short one ("ZZTEST0001" → "ZZTE******").
pub fn mask(value: &str) -> String {
    let count = value.chars().count();
    let keep = (count / 2).min(4);
    value
        .chars()
        .take(keep)
        .chain(std::iter::repeat_n('*', count - keep))
        .collect()
}

/// Text from a client or the SSO made safe to store and show: trimmed, without control
/// characters, at most `max` characters.
pub fn clean_text(text: &str, max: usize) -> String {
    let kept: String = text
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(max)
        .collect();
    kept.trim().to_owned()
}

/// A portal ID as typed, or an entry of the allowlist: printable ASCII without spaces, 1 to 64
/// characters (after trimming).
pub fn valid_portal_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_graphic())
}

/// The form of a portal ID that the allowlist and accounts compare: trimmed and case-folded
/// (design v2 §9-2 step 4).
pub fn login_key(id: &str) -> String {
    id.trim().to_lowercase()
}

/// The first letters of the portal IDs [`staff_id`] takes: Z and Y (the 행정허브 plugin takes Z
/// and B; B only if the user says so later, as one more letter here).
const ID_FIRST_LETTERS: &[u8] = b"ZY";

/// HJViewer's ID rule (the user's decision of 2026-10-14), the one place it is written: a portal
/// ID, uppercased, is one of `ID_FIRST_LETTERS` and then 1 to 59 of A-Z, 0-9, '_' and '-'
/// (`^[ZY][A-Z0-9_-]{1,59}$`), and does not start with "ZA": the IDs of faculty and staff, with
/// the tail of the 행정허브 plugin, whose SSO registration hjv-api uses (D2). The ID typed at login
/// must pass it before any SSO call (login.rs), and the pcode the SSO names for the person after
/// the password (sso.rs). Returns the ID uppercased, the form the SSO takes and names, or `None`.
pub fn staff_id(id: &str) -> Option<String> {
    let id = id.to_ascii_uppercase();
    let b = id.as_bytes();
    let ok = (2..=60).contains(&b.len())
        && ID_FIRST_LETTERS.contains(&b[0])
        && !id.starts_with("ZA")
        && b[1..]
            .iter()
            .all(|&c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_' || c == b'-');
    ok.then_some(id)
}

/// The address a login limit counts under: an IPv4 address itself, an IPv6 address by its /64
/// network, which one subscriber usually holds whole and can pick addresses from at will
/// ("2001:db8:1:2::/64"). What is not an address ("invalid", "unknown") stays as it is.
pub fn address_group(ip: &str) -> String {
    match ip.trim().parse::<IpAddr>() {
        Ok(IpAddr::V6(v6)) => match v6.to_ipv4_mapped() {
            Some(v4) => v4.to_string(),
            None => format!(
                "{}/64",
                Ipv6Addr::from(u128::from(v6) & !u128::from(u64::MAX))
            ),
        },
        Ok(IpAddr::V4(v4)) => v4.to_string(),
        Err(_) => ip.to_owned(),
    }
}

/// An address network, such as `192.0.2.0/24` or `2001:db8::/32` (HJV_API_CAMPUS_NETS).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpNet {
    addr: IpAddr,
    prefix: u8,
}

impl IpNet {
    /// `<address>/<prefix>`, or an address alone (one host). Bits past the prefix are ignored.
    pub fn parse(text: &str) -> Option<IpNet> {
        let text = text.trim();
        let (addr, prefix) = match text.split_once('/') {
            Some((a, p)) => (a.trim(), Some(p.trim())),
            None => (text, None),
        };
        let addr: IpAddr = addr.parse().ok()?;
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(p) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => p.parse().ok()?,
            Some(_) => return None,
            None => max,
        };
        (prefix <= max).then_some(IpNet { addr, prefix })
    }

    /// Whether `ip` is in the network (an IPv4 network holds no IPv6 address and the other way
    /// round; the server already reads an IPv4-mapped IPv6 client as IPv4).
    pub fn contains(&self, ip: &IpAddr) -> bool {
        match (self.addr, ip) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = u32::MAX
                    .checked_shl(32 - u32::from(self.prefix))
                    .unwrap_or(0);
                u32::from(net) & mask == u32::from(*ip) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = u128::MAX
                    .checked_shl(128 - u32::from(self.prefix))
                    .unwrap_or(0);
                u128::from(net) & mask == u128::from(*ip) & mask
            }
            _ => false,
        }
    }
}

impl std::fmt::Display for IpNet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.addr, self.prefix)
    }
}

/// A RustDesk device ID: 1 to 64 letters, digits, '-' or '_' (numeric IDs and custom ones).
pub fn valid_device_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// "2026-10-12" for a Unix time (UTC).
pub fn date(ts: i64) -> String {
    let (y, m, d) = civil_from_days(ts.div_euclid(DAY));
    format!("{y:04}-{m:02}-{d:02}")
}

/// "2026-10-12T09:30:00Z" for a Unix time.
pub fn datetime(ts: i64) -> String {
    let secs = ts.rem_euclid(DAY);
    format!(
        "{}T{:02}:{:02}:{:02}Z",
        date(ts),
        secs / HOUR,
        secs % HOUR / MINUTE,
        secs % MINUTE
    )
}

/// The day number (days since 1970-01-01) of a UTC date written "2026-10-12", exactly in that
/// form; `None` for anything else, such as "2026-02-30" or "2026-1-9".
pub fn day_number(text: &str) -> Option<i64> {
    let b = text.as_bytes();
    let digits = |r: std::ops::Range<usize>| -> Option<i64> {
        b.get(r.clone())?
            .iter()
            .all(u8::is_ascii_digit)
            .then(|| text[r].parse().ok())
            .flatten()
    };
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let (y, m, d) = (digits(0..4)?, digits(5..7)?, digits(8..10)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let days = days_from_civil(y, m, d);
    (date(days * DAY) == text).then_some(days)
}

/// Days since 1970-01-01 of a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(datetime(0), "1970-01-01T00:00:00Z");
        assert_eq!(datetime(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(datetime(1_760_259_045), "2025-10-12T08:50:45Z");
        assert_eq!(date(-1), "1969-12-31");
        // and back
        for ts in [0, 951_782_400, 1_760_259_045, -DAY, 4_102_444_800] {
            assert_eq!(day_number(&date(ts)), Some(ts.div_euclid(DAY)), "{ts}");
        }
        assert_eq!(day_number("2025-10-09"), Some(20_370));
        for bad in [
            "2026-02-30",
            "2025-02-29",
            "2026-13-01",
            "2026-00-10",
            "2026-1-09",
            "2026-01-9",
            "20261009",
            "2026/10/09",
            "+026-10-09",
            "2026-10-09 ",
            "",
        ] {
            assert_eq!(day_number(bad), None, "{bad}");
        }
    }

    #[test]
    fn masking() {
        assert_eq!(mask("ZZTEST0001"), "ZZTE******");
        assert_eq!(mask("abcd"), "ab**");
        assert_eq!(mask("a"), "*");
        assert_eq!(mask(""), "");
        assert_eq!(mask("12345678"), "1234****");
    }

    #[test]
    fn ids() {
        assert!(valid_portal_id("ZZTEST01"));
        assert!(valid_portal_id("yztest02"));
        // a portal ID by its form, which HJViewer's rule does not take (staff_id)
        assert!(valid_portal_id("1234567890"));
        assert!(!valid_portal_id(""));
        assert!(!valid_portal_id("a b"));
        assert!(!valid_portal_id("홍길동"));
        assert!(!valid_portal_id(&"1".repeat(65)));
        assert_eq!(login_key("  YZTEST02 "), "yztest02");
        assert!(valid_device_id("123456789"));
        assert!(valid_device_id("my-pc_2"));
        assert!(!valid_device_id("12 3"));
        assert!(!valid_device_id("1\n2"));
        assert!(!valid_device_id(""));
    }

    /// HJViewer's ID rule: Z or Y, then 1 to 59 of A-Z, 0-9, '_' and '-', uppercased, not ZA.
    #[test]
    fn the_id_rule() {
        for (id, upper) in [
            ("ZZTEST01", "ZZTEST01"),
            ("zztest01", "ZZTEST01"),
            ("YZTEST02", "YZTEST02"),
            ("yZtest_02-x", "YZTEST_02-X"),
            ("Z1", "Z1"),
            ("Y-", "Y-"),
            ("YA_TEST", "YA_TEST"),
        ] {
            assert_eq!(staff_id(id).as_deref(), Some(upper), "{id}");
        }
        let longest = format!("Z{}", "9".repeat(59));
        assert_eq!(staff_id(&longest), Some(longest.clone()));
        for id in [
            // ZA, in any case
            "ZATEST01",
            "zatest01",
            "Za",
            // B, which the 행정허브 plugin takes and HJViewer does not (yet)
            "BZTEST01",
            // a student-like number, other letters, too short, too long
            "1234567890",
            "AZTEST01",
            "XZTEST01",
            "Z",
            "Y",
            &format!("Z{}", "9".repeat(60)),
            // characters outside A-Z, 0-9, '_' and '-', also those that uppercasing would not
            // turn into them, and spaces (login.rs trims before)
            "ZZ.TEST",
            "ZZ TEST",
            " ZZTEST01",
            "ZZTEST01\n",
            "ZZTEST01\u{0}",
            "ZZTEST\u{131}",
            "ＺＺTEST01",
            "Ｚ1",
            "",
        ] {
            assert_eq!(staff_id(id), None, "{id:?}");
        }
    }

    #[test]
    fn address_groups() {
        assert_eq!(address_group("10.1.2.3"), "10.1.2.3");
        assert_eq!(address_group("2001:db8:1:2:aaaa::1"), "2001:db8:1:2::/64");
        assert_eq!(
            address_group("2001:db8:1:2:ffff:ffff:ffff:ffff"),
            "2001:db8:1:2::/64"
        );
        assert_eq!(address_group("2001:db8:1:3::1"), "2001:db8:1:3::/64");
        assert_eq!(address_group("::ffff:10.1.2.3"), "10.1.2.3");
        assert_eq!(address_group("::1"), "::/64");
        assert_eq!(address_group("invalid"), "invalid");
        assert_eq!(address_group("unknown"), "unknown");
    }

    #[test]
    fn networks() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        let net = IpNet::parse("192.0.2.0/24").unwrap();
        assert!(net.contains(&ip("192.0.2.1")) && net.contains(&ip("192.0.2.255")));
        assert!(!net.contains(&ip("192.0.3.1")) && !net.contains(&ip("2001:db8::1")));
        // bits past the prefix do not matter; a lone address is one host; /0 holds everything
        assert!(IpNet::parse(" 192.0.2.77/24 ")
            .unwrap()
            .contains(&ip("192.0.2.5")));
        let host = IpNet::parse("198.51.100.7").unwrap();
        assert!(host.contains(&ip("198.51.100.7")) && !host.contains(&ip("198.51.100.8")));
        assert!(IpNet::parse("0.0.0.0/0")
            .unwrap()
            .contains(&ip("203.0.113.9")));
        let v6 = IpNet::parse("2001:db8::/32").unwrap();
        assert!(v6.contains(&ip("2001:db8:ffff::1")) && !v6.contains(&ip("2001:db9::1")));
        assert!(!v6.contains(&ip("192.0.2.1")));
        assert_eq!(v6.to_string(), "2001:db8::/32");
        for bad in [
            "192.0.2.0/33",
            "2001:db8::/129",
            "192.0.2.0/",
            "192.0.2.0/x",
            "192.0.2.0/+8",
            "example",
            "",
        ] {
            assert!(IpNet::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn cleaning() {
        assert_eq!(clean_text("  홍길동\u{0007}  ", 64), "홍길동");
        assert_eq!(clean_text("abcdef", 3), "abc");
        assert_eq!(clean_text("a\nb", 10), "ab");
    }

    #[test]
    fn encodings() {
        assert_eq!(b64(b"hi"), "aGk=");
        assert_eq!(b64_decode("aGk=").unwrap(), b"hi");
        assert_eq!(b64url(&[0xfb, 0xff]), "-_8");
        assert_eq!(b64url_decode("-_8").unwrap(), vec![0xfb, 0xff]);
        assert_eq!(hex(&[0, 255]), "00ff");
        assert_ne!(random_bytes::<16>(), random_bytes::<16>());
        assert!(random_below(10) < 10);
    }
}

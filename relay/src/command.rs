// SPDX-License-Identifier: AGPL-3.0-only

//! Wake commands and the relay's checks before it sends anything (PROTOCOL.md, "Commands" and
//! "Relay checks"). A command arrives as `{"c": <command JSON as a string>, "s": <signature>,
//! "k": <key id>}`; the signature covers `hjv-wake-v1\0` and the exact bytes of `c`, and is checked
//! before `c` is parsed. The checks run in this order and the first failure names the refusal:
//!
//! 1. `k` names a key of this build and the signature verifies (`unknown_key`, `bad_signature`)
//! 2. `c` is a version 1 command with every field well formed (`bad_version`, `bad_command`)
//! 3. it is addressed to this relay (`wrong_relay`)
//! 4. `exp` is after the estimated server time, `iat` at most 30 s ahead of it, the lifetime
//!    `exp - iat` 1-120 s, and `iat` at most 120 s before the newest `iat` of a command that got
//!    this far (`expired`, `not_yet_valid`, `bad_lifetime`, `expired`)
//! 5. the job was not seen since the relay started (`replayed`)
//! 6. 1-4 distinct individual MACs (`bad_macs`)
//! 7. ports a non-empty subset of {7, 9}, repeat 1-5, gap 100-1000 ms (`bad_params`)
//!
//! then, in the relay loop, 8 an own interface on the command's network (`no_iface`) and 9 the
//! rate limits (`rate_limited`).
//!
//! The estimated server time comes from `server_time`, which is not signed. The last part of
//! check 4 rests on signed values only: a command issued more than 120 s before another command
//! the server signed had expired before that one was issued, whatever `server_time` says, so
//! setting `server_time` back cannot bring back a command recorded earlier. It also bounds the
//! job memory: a job only has to be remembered while its command could still pass check 4.

use crate::{config::CommandKey, netif::Ipv4Net, wake};
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::Signature;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeSet, HashSet, VecDeque},
    time::{Duration, Instant},
};

/// Prefix of the signed bytes: domain separation from every other use of the command key.
pub const DOMAIN: &[u8] = b"hjv-wake-v1\0";
pub const COMMAND_VERSION: i64 = 1;
pub const MAX_LIFETIME: i64 = 120;
pub const MAX_AHEAD: i64 = 30;
pub const MAX_MACS: usize = 4;
pub const ALLOWED_PORTS: [u16; 2] = [9, 7];
pub const MAX_REPEAT: i64 = 5;
pub const GAP_MS: std::ops::RangeInclusive<i64> = 100..=1000;
pub const MIN_NET_PREFIX: u8 = 8;
/// The most jobs remembered at once.
pub const JOB_MEMORY_CAP: usize = 4096;
pub const RATE_WINDOW: Duration = Duration::from_secs(60);
pub const MAX_COMMANDS_PER_WINDOW: usize = 10;
pub const MAX_PACKETS_PER_WINDOW: u32 = 120;

/// Why a command was not (fully) carried out; `code()` is what the result reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    UnknownKey,
    BadSignature,
    BadCommand,
    BadVersion,
    WrongRelay,
    Expired,
    NotYetValid,
    BadLifetime,
    Replayed,
    BadMacs,
    BadParams,
    NoIface,
    RateLimited,
    /// The relay is pending or revoked and takes no commands.
    NotActive,
    /// Checks passed but no packet could be sent.
    SendFailed,
    /// Checks passed and some, not all, packets were sent.
    Partial,
}

impl Refusal {
    pub fn code(self) -> &'static str {
        match self {
            Refusal::UnknownKey => "unknown_key",
            Refusal::BadSignature => "bad_signature",
            Refusal::BadCommand => "bad_command",
            Refusal::BadVersion => "bad_version",
            Refusal::WrongRelay => "wrong_relay",
            Refusal::Expired => "expired",
            Refusal::NotYetValid => "not_yet_valid",
            Refusal::BadLifetime => "bad_lifetime",
            Refusal::Replayed => "replayed",
            Refusal::BadMacs => "bad_macs",
            Refusal::BadParams => "bad_params",
            Refusal::NoIface => "no_iface",
            Refusal::RateLimited => "rate_limited",
            Refusal::NotActive => "not_active",
            Refusal::SendFailed => "send_failed",
            Refusal::Partial => "partial",
        }
    }
}

/// One entry of a poll response's `commands`, as received.
#[derive(Clone, Copy, Debug)]
pub struct Signed<'a> {
    pub c: &'a str,
    pub s: &'a str,
    pub k: u64,
}

/// The entry's `c`, `s` and `k`, if it has them with the right JSON types.
pub fn signed_entry(entry: &Value) -> Option<Signed<'_>> {
    Some(Signed {
        c: entry.get("c")?.as_str()?,
        s: entry.get("s")?.as_str()?,
        k: entry.get("k")?.as_u64()?,
    })
}

/// Lowercase hex SHA-256 of the command string, which names the command in results.
pub fn command_hash(c: &str) -> String {
    crate::netif::hex(&Sha256::digest(c.as_bytes()))
}

/// Check 1: the key and the signature over the exact bytes received.
pub fn verify(keys: &[CommandKey], entry: &Signed<'_>) -> Result<(), Refusal> {
    let key = keys
        .iter()
        .find(|k| u64::from(k.id) == entry.k)
        .ok_or(Refusal::UnknownKey)?;
    let signature: [u8; 64] = STANDARD
        .decode(entry.s)
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or(Refusal::BadSignature)?;
    let mut msg = Vec::with_capacity(DOMAIN.len() + entry.c.len());
    msg.extend_from_slice(DOMAIN);
    msg.extend_from_slice(entry.c.as_bytes());
    key.key
        .verify_strict(&msg, &Signature::from_bytes(&signature))
        .map_err(|_| Refusal::BadSignature)
}

/// A version 1 command, well formed but not yet checked against this relay, the time or limits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub job: String,
    pub relay: String,
    pub iat: i64,
    pub exp: i64,
    pub net: Ipv4Net,
    pub macs: Vec<String>,
    pub ports: Vec<i64>,
    pub repeat: i64,
    pub gap_ms: i64,
}

#[derive(Deserialize)]
struct WireV1 {
    job: String,
    relay: String,
    iat: i64,
    exp: i64,
    net: String,
    macs: Vec<String>,
    ports: Vec<i64>,
    repeat: i64,
    gap_ms: i64,
}

/// Check 2: `c` is a JSON object with `"v": 1` and every field of the right type and format
/// (`job` 32 lowercase hex digits, `net` a canonical IPv4 network with prefix 8-32). Fields this
/// version does not know are ignored.
pub fn parse(c: &str) -> Result<Command, Refusal> {
    let value: Value = serde_json::from_str(c).map_err(|_| Refusal::BadCommand)?;
    let version = value
        .as_object()
        .and_then(|o| o.get("v"))
        .ok_or(Refusal::BadCommand)?;
    match version.as_i64() {
        Some(COMMAND_VERSION) => {}
        Some(_) => return Err(Refusal::BadVersion),
        None if version.is_number() => return Err(Refusal::BadVersion),
        None => return Err(Refusal::BadCommand),
    }
    let wire: WireV1 = serde_json::from_value(value).map_err(|_| Refusal::BadCommand)?;
    let job_ok = wire.job.len() == 32
        && wire
            .job
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    let net: Option<Ipv4Net> = wire.net.parse().ok();
    let net = net
        .filter(|n| n.prefix >= MIN_NET_PREFIX && n.is_network())
        .ok_or(Refusal::BadCommand)?;
    if !job_ok {
        return Err(Refusal::BadCommand);
    }
    Ok(Command {
        job: wire.job,
        relay: wire.relay,
        iat: wire.iat,
        exp: wire.exp,
        net,
        macs: wire.macs,
        ports: wire.ports,
        repeat: wire.repeat,
        gap_ms: wire.gap_ms,
    })
}

/// What a command that passed checks 3-7 asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Admitted {
    pub macs: Vec<[u8; 6]>,
    pub ports: Vec<u16>,
    pub repeat: u32,
    pub gap: Duration,
}

/// Checks 3-7. `now` is the estimated server time. A command that gets past check 4 uses up its
/// job id and raises the floor of check 4, whatever happens next.
pub fn check(
    cmd: &Command,
    relay_id: &str,
    now: i64,
    jobs: &mut JobMemory,
) -> Result<Admitted, Refusal> {
    if cmd.relay != relay_id {
        return Err(Refusal::WrongRelay);
    }
    if cmd.exp <= now {
        return Err(Refusal::Expired);
    }
    if cmd.iat > now.saturating_add(MAX_AHEAD) {
        return Err(Refusal::NotYetValid);
    }
    match cmd.exp.checked_sub(cmd.iat) {
        Some(lifetime) if (1..=MAX_LIFETIME).contains(&lifetime) => {}
        _ => return Err(Refusal::BadLifetime),
    }
    if jobs.superseded(cmd.iat) {
        return Err(Refusal::Expired);
    }
    if !jobs.first_time(&cmd.job, cmd.iat) {
        return Err(Refusal::Replayed);
    }
    let mut macs: Vec<[u8; 6]> = Vec::with_capacity(cmd.macs.len());
    for text in &cmd.macs {
        match wake::parse_mac(text) {
            Some(mac) if wake::wakeable(&mac) && !macs.contains(&mac) => macs.push(mac),
            _ => return Err(Refusal::BadMacs),
        }
    }
    if macs.is_empty() || macs.len() > MAX_MACS {
        return Err(Refusal::BadMacs);
    }
    let mut ports: Vec<u16> = Vec::with_capacity(cmd.ports.len());
    for port in &cmd.ports {
        match ALLOWED_PORTS.iter().find(|p| i64::from(**p) == *port) {
            Some(p) if !ports.contains(p) => ports.push(*p),
            _ => return Err(Refusal::BadParams),
        }
    }
    if ports.is_empty() || !(1..=MAX_REPEAT).contains(&cmd.repeat) || !GAP_MS.contains(&cmd.gap_ms)
    {
        return Err(Refusal::BadParams);
    }
    Ok(Admitted {
        macs,
        ports,
        repeat: u32::try_from(cmd.repeat).map_err(|_| Refusal::BadParams)?,
        gap: Duration::from_millis(u64::try_from(cmd.gap_ms).map_err(|_| Refusal::BadParams)?),
    })
}

/// The jobs of the commands that got past check 4 since the relay started, and the newest `iat`
/// among those commands (the floor of check 4).
#[derive(Debug, Default)]
pub struct JobMemory {
    ids: HashSet<String>,
    by_iat: BTreeSet<(i64, String)>,
    newest: Option<i64>,
    /// The newest `iat` of a job forgotten only because of [`JOB_MEMORY_CAP`]: a command issued
    /// at or before it may have run already.
    forgotten: Option<i64>,
}

impl JobMemory {
    /// Check 4's floor: whether a command issued at `iat` had expired before the newest command
    /// that got past check 4 was issued.
    pub fn superseded(&self, iat: i64) -> bool {
        self.newest
            .is_some_and(|newest| iat < newest.saturating_sub(MAX_LIFETIME))
    }

    /// Records the job of a command issued at `iat` that passed check 4, and says whether the
    /// job is new. Jobs whose commands check 4 refuses now anyway are forgotten.
    pub fn first_time(&mut self, job: &str, iat: i64) -> bool {
        let newest = self.newest.map_or(iat, |n| n.max(iat));
        self.newest = Some(newest);
        let floor = newest.saturating_sub(MAX_LIFETIME);
        while let Some((t, _)) = self.by_iat.first() {
            let t = *t;
            if t >= floor && self.by_iat.len() < JOB_MEMORY_CAP {
                break;
            }
            if let Some((_, id)) = self.by_iat.pop_first() {
                self.ids.remove(&id);
            }
            if t >= floor {
                self.forgotten = Some(self.forgotten.map_or(t, |f| f.max(t)));
            }
        }
        if self.ids.contains(job) || self.forgotten.is_some_and(|f| iat <= f) {
            return false;
        }
        self.ids.insert(job.to_owned());
        self.by_iat.insert((iat, job.to_owned()));
        true
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
}

/// At most 10 commands and 120 packets in any 60 seconds.
#[derive(Debug, Default)]
pub struct RateLimiter {
    events: VecDeque<(Instant, u32)>,
}

impl RateLimiter {
    /// Takes a command of `packets` packets if it fits, and says whether it did.
    pub fn admit(&mut self, packets: u32, now: Instant) -> bool {
        while let Some((t, _)) = self.events.front() {
            if now.saturating_duration_since(*t) < RATE_WINDOW {
                break;
            }
            self.events.pop_front();
        }
        let used: u32 = self.events.iter().map(|(_, p)| p).sum();
        if self.events.len() >= MAX_COMMANDS_PER_WINDOW
            || used.saturating_add(packets) > MAX_PACKETS_PER_WINDOW
        {
            return false;
        }
        self.events.push_back((now, packets));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    const RELAY: &str = "r-aaaaaaaaaaaaaaaa";
    const NOW: i64 = 1_760_000_000;

    fn key(seed: u8, id: u32) -> (SigningKey, CommandKey) {
        let sk = SigningKey::from_bytes(&[seed; 32]);
        let pk = CommandKey {
            id,
            key: sk.verifying_key(),
        };
        (sk, pk)
    }

    fn sign(sk: &SigningKey, c: &str) -> String {
        let mut msg = DOMAIN.to_vec();
        msg.extend_from_slice(c.as_bytes());
        STANDARD.encode(sk.sign(&msg).to_bytes())
    }

    fn command(patch: impl FnOnce(&mut serde_json::Map<String, Value>)) -> String {
        let mut v = serde_json::json!({
            "v": 1, "job": "0123456789abcdef0123456789abcdef", "relay": RELAY,
            "iat": NOW, "exp": NOW + 60, "net": "10.1.4.0/22",
            "macs": ["00:00:5e:00:53:01"], "ports": [9, 7], "repeat": 3, "gap_ms": 200
        });
        patch(v.as_object_mut().unwrap());
        v.to_string()
    }

    fn admit(c: &str, jobs: &mut JobMemory) -> Result<Admitted, Refusal> {
        admit_at(c, jobs, NOW)
    }

    fn admit_at(c: &str, jobs: &mut JobMemory, now: i64) -> Result<Admitted, Refusal> {
        let cmd = parse(c)?;
        check(&cmd, RELAY, now, jobs)
    }

    #[test]
    fn a_good_command_passes() {
        let (sk, pk) = key(9, 1);
        let c = command(|_| {});
        let s = sign(&sk, &c);
        let entry = serde_json::json!({"c": c, "s": s, "k": 1});
        let signed = signed_entry(&entry).unwrap();
        verify(&[pk], &signed).unwrap();
        let ok = admit(&c, &mut JobMemory::default()).unwrap();
        assert_eq!(ok.macs, [[0x00, 0x00, 0x5e, 0x00, 0x53, 0x01]]);
        assert_eq!(ok.ports, [9, 7]);
        assert_eq!((ok.repeat, ok.gap), (3, Duration::from_millis(200)));
        assert_eq!(command_hash(&c).len(), 64);
    }

    #[test]
    fn check_1_key_and_signature() {
        let (sk, pk) = key(9, 1);
        let (other, pk2) = key(8, 2);
        let c = command(|_| {});
        let good = sign(&sk, &c);
        fn entry<'a>(c: &'a str, s: &'a str, k: u64) -> Signed<'a> {
            Signed { c, s, k }
        }
        assert_eq!(
            verify(std::slice::from_ref(&pk), &entry(&c, &good, 2)),
            Err(Refusal::UnknownKey)
        );
        assert_eq!(
            verify(std::slice::from_ref(&pk), &entry(&c, &good, 0)),
            Err(Refusal::UnknownKey)
        );
        assert_eq!(
            verify(std::slice::from_ref(&pk), &entry(&c, &sign(&other, &c), 1)),
            Err(Refusal::BadSignature)
        );
        assert_eq!(
            verify(std::slice::from_ref(&pk), &entry(&c, "AAAA", 1)),
            Err(Refusal::BadSignature)
        );
        assert_eq!(
            verify(std::slice::from_ref(&pk), &entry(&c, "not base64!", 1)),
            Err(Refusal::BadSignature)
        );
        // any change to the bytes, even one that parses the same, breaks the signature
        let spaced = c.replacen(',', ", ", 1);
        assert_eq!(
            verify(std::slice::from_ref(&pk), &entry(&spaced, &good, 1)),
            Err(Refusal::BadSignature)
        );
        // without the domain prefix
        let bare = STANDARD.encode(sk.sign(c.as_bytes()).to_bytes());
        assert_eq!(
            verify(std::slice::from_ref(&pk), &entry(&c, &bare, 1)),
            Err(Refusal::BadSignature)
        );
        // the second key of a rotation
        assert_eq!(
            verify(&[pk.clone(), pk2.clone()], &entry(&c, &sign(&other, &c), 2)),
            Ok(())
        );
        // malformed entries
        assert!(signed_entry(&serde_json::json!({"c": c, "s": good, "k": "1"})).is_none());
        assert!(signed_entry(&serde_json::json!({"c": {}, "s": good, "k": 1})).is_none());
        assert!(signed_entry(&serde_json::json!({"s": good, "k": 1})).is_none());
    }

    #[test]
    fn check_2_format() {
        let mut jobs = JobMemory::default();
        assert_eq!(admit("not json", &mut jobs), Err(Refusal::BadCommand));
        assert_eq!(admit("[1]", &mut jobs), Err(Refusal::BadCommand));
        assert_eq!(
            admit(
                &command(|m| {
                    m.remove("v");
                }),
                &mut jobs
            ),
            Err(Refusal::BadCommand)
        );
        assert_eq!(
            admit(
                &command(|m| {
                    m.insert("v".into(), "1".into());
                }),
                &mut jobs
            ),
            Err(Refusal::BadCommand)
        );
        assert_eq!(
            admit(
                &command(|m| {
                    m.insert("v".into(), 2.into());
                }),
                &mut jobs
            ),
            Err(Refusal::BadVersion)
        );
        assert_eq!(
            admit(
                &command(|m| {
                    m.insert("v".into(), 1.5.into());
                }),
                &mut jobs
            ),
            Err(Refusal::BadVersion)
        );
        // a version-2 command is refused for its version even when its fields differ
        assert_eq!(
            admit(r#"{"v":2,"whatever":true}"#, &mut jobs),
            Err(Refusal::BadVersion)
        );
        for (field, value) in [
            ("job", Value::from("0123456789ABCDEF0123456789ABCDEF")),
            ("job", Value::from("0123")),
            ("job", Value::from(5)),
            ("relay", Value::from(5)),
            ("iat", Value::from(1.5)),
            ("exp", Value::from("soon")),
            ("net", Value::from("10.1.5.0/22")),
            ("net", Value::from("10.0.0.0/7")),
            ("net", Value::from("10.1.4.0")),
            ("macs", Value::from("00:00:5e:00:53:01")),
            ("ports", Value::from(9)),
            ("ports", serde_json::json!([9.5])),
            ("repeat", Value::from("3")),
            ("gap_ms", Value::Null),
        ] {
            let c = command(|m| {
                m.insert(field.into(), value.clone());
            });
            assert_eq!(
                admit(&c, &mut jobs),
                Err(Refusal::BadCommand),
                "{field}={value}"
            );
        }
        for field in [
            "job", "relay", "iat", "exp", "net", "macs", "ports", "repeat", "gap_ms",
        ] {
            let c = command(|m| {
                m.remove(field);
            });
            assert_eq!(
                admit(&c, &mut jobs),
                Err(Refusal::BadCommand),
                "without {field}"
            );
        }
        // unknown fields are ignored
        let c = command(|m| {
            m.insert("note".into(), "x".into());
        });
        assert!(admit(&c, &mut jobs).is_ok());
    }

    #[test]
    fn checks_3_to_7_in_order() {
        let mut jobs = JobMemory::default();
        let mut job = 0u32;
        let mut fresh = |patch: &dyn Fn(&mut serde_json::Map<String, Value>)| {
            job += 1;
            command(|m| {
                m.insert("job".into(), format!("{job:032x}").into());
                patch(m);
            })
        };
        let wrong = |m: &mut serde_json::Map<String, Value>| {
            m.insert("relay".into(), "r-bbbbbbbbbbbbbbbb".into());
        };
        assert_eq!(admit(&fresh(&wrong), &mut jobs), Err(Refusal::WrongRelay));
        // 3 comes before 4
        let wrong_and_old = |m: &mut serde_json::Map<String, Value>| {
            m.insert("relay".into(), "r-bbbbbbbbbbbbbbbb".into());
            m.insert("exp".into(), (NOW - 1).into());
        };
        assert_eq!(
            admit(&fresh(&wrong_and_old), &mut jobs),
            Err(Refusal::WrongRelay)
        );
        let t = |iat: i64, exp: i64| {
            move |m: &mut serde_json::Map<String, Value>| {
                m.insert("iat".into(), iat.into());
                m.insert("exp".into(), exp.into());
            }
        };
        // 4, each with a memory of its own (an accepted command raises the floor)
        let mut fresh_admit = |patch: &dyn Fn(&mut serde_json::Map<String, Value>)| {
            admit(&fresh(patch), &mut JobMemory::default())
        };
        assert_eq!(fresh_admit(&t(NOW - 60, NOW)), Err(Refusal::Expired));
        assert_eq!(fresh_admit(&t(NOW - 200, NOW - 100)), Err(Refusal::Expired));
        assert_eq!(
            fresh_admit(&t(NOW + 31, NOW + 90)),
            Err(Refusal::NotYetValid)
        );
        assert!(fresh_admit(&t(NOW + 30, NOW + 90)).is_ok());
        assert_eq!(
            fresh_admit(&t(NOW - 100, NOW + 21)),
            Err(Refusal::BadLifetime)
        );
        assert!(fresh_admit(&t(NOW - 100, NOW + 20)).is_ok());
        assert_eq!(
            fresh_admit(&t(NOW + 10, NOW + 10)),
            Err(Refusal::BadLifetime)
        );
        assert_eq!(
            fresh_admit(&t(NOW + 20, NOW + 10)),
            Err(Refusal::BadLifetime)
        );
        assert_eq!(
            fresh_admit(&t(i64::MIN, NOW + 10)),
            Err(Refusal::BadLifetime)
        );
        // 5: a job runs once, even if the first time failed a later check
        let c = command(|m| {
            m.insert("job".into(), "f".repeat(32).into());
            m.insert("ports".into(), serde_json::json!([80]));
        });
        assert_eq!(admit(&c, &mut jobs), Err(Refusal::BadParams));
        let c = command(|m| {
            m.insert("job".into(), "f".repeat(32).into());
        });
        assert_eq!(admit(&c, &mut jobs), Err(Refusal::Replayed));
        // 6
        let m = |macs: Value| {
            move |c: &mut serde_json::Map<String, Value>| {
                c.insert("macs".into(), macs.clone());
            }
        };
        for macs in [
            serde_json::json!([]),
            serde_json::json!([
                "00:00:5e:00:53:01",
                "00:00:5e:00:53:02",
                "00:00:5e:00:53:03",
                "00:00:5e:00:53:04",
                "00:00:5e:00:53:05"
            ]),
            serde_json::json!(["00:00:5e:00:53:01", "00:00:5e:00:53:01"]),
            serde_json::json!(["ff:ff:ff:ff:ff:ff"]),
            serde_json::json!(["01:00:5e:00:00:01"]),
            serde_json::json!(["00:00:00:00:00:00"]),
            serde_json::json!(["00-00-5e-00-53-01"]),
            serde_json::json!(["00:00:5e:00:53"]),
        ] {
            assert_eq!(
                admit(&fresh(&m(macs.clone())), &mut jobs),
                Err(Refusal::BadMacs),
                "{macs}"
            );
        }
        let four = serde_json::json!([
            "00:00:5e:00:53:01",
            "00:00:5e:00:53:02",
            "00:00:5e:00:53:03",
            "02:00:00:00:00:01"
        ]);
        assert_eq!(admit(&fresh(&m(four)), &mut jobs).unwrap().macs.len(), 4);
        // 6 comes before 7
        let both = |c: &mut serde_json::Map<String, Value>| {
            c.insert("macs".into(), serde_json::json!([]));
            c.insert("ports".into(), serde_json::json!([80]));
        };
        assert_eq!(admit(&fresh(&both), &mut jobs), Err(Refusal::BadMacs));
        // 7
        let p = |field: &'static str, v: Value| {
            move |c: &mut serde_json::Map<String, Value>| {
                c.insert(field.into(), v.clone());
            }
        };
        for (field, v) in [
            ("ports", serde_json::json!([])),
            ("ports", serde_json::json!([80])),
            ("ports", serde_json::json!([9, 9])),
            ("ports", serde_json::json!([9, 7, 9])),
            ("ports", serde_json::json!([-9])),
            ("repeat", Value::from(0)),
            ("repeat", Value::from(6)),
            ("gap_ms", Value::from(99)),
            ("gap_ms", Value::from(1001)),
        ] {
            assert_eq!(
                admit(&fresh(&p(field, v.clone())), &mut jobs),
                Err(Refusal::BadParams),
                "{field}={v}"
            );
        }
        let ok = admit(&fresh(&p("ports", serde_json::json!([7]))), &mut jobs).unwrap();
        assert_eq!(ok.ports, [7]);
        let ok = admit(
            &fresh(&|c| {
                c.insert("repeat".into(), 5.into());
                c.insert("gap_ms".into(), 1000.into());
            }),
            &mut jobs,
        )
        .unwrap();
        assert_eq!((ok.repeat, ok.gap), (5, Duration::from_secs(1)));
        let ok = admit(
            &fresh(&|c| {
                c.insert("repeat".into(), 1.into());
                c.insert("gap_ms".into(), 100.into());
            }),
            &mut jobs,
        )
        .unwrap();
        assert_eq!((ok.repeat, ok.gap), (1, Duration::from_millis(100)));
    }

    #[test]
    fn job_memory_and_the_floor_of_check_4() {
        let mut jobs = JobMemory::default();
        assert!(!jobs.superseded(i64::MIN));
        assert!(jobs.first_time("a", 1000));
        assert!(!jobs.first_time("a", 1000));
        // a job is remembered as long as its command could pass check 4
        assert!(jobs.first_time("b", 1120));
        assert!(!jobs.first_time("a", 1000));
        assert!(!jobs.superseded(1000) && jobs.superseded(999));
        // a newer command: "a" (iat 1000) is refused by check 4 now, and forgotten
        assert!(jobs.first_time("c", 1121));
        assert!(jobs.superseded(1000));
        assert_eq!(jobs.len(), 2);
        // capped: the oldest go first, and nothing issued up to them passes any more
        let mut jobs = JobMemory::default();
        for i in 0..JOB_MEMORY_CAP + 10 {
            assert!(jobs.first_time(&i.to_string(), 5000 + (i / 100) as i64));
        }
        assert_eq!(jobs.len(), JOB_MEMORY_CAP);
        assert!(!jobs.first_time("new, but issued with the forgotten ones", 5000));
        assert!(jobs.first_time("new", 5041));
    }

    #[test]
    fn a_rewound_clock_does_not_bring_back_old_commands() {
        // the estimated server time comes from unsigned responses: here it is set back a day
        // after a command of today, to replay one recorded yesterday that was never seen
        let mut jobs = JobMemory::default();
        let today = command(|m| {
            m.insert("job".into(), "a".repeat(32).into());
        });
        assert!(admit(&today, &mut jobs).is_ok());
        let yesterday = command(|m| {
            m.insert("job".into(), "b".repeat(32).into());
            m.insert("iat".into(), (NOW - 86_400).into());
            m.insert("exp".into(), (NOW - 86_400 + 60).into());
        });
        assert_eq!(
            admit_at(&yesterday, &mut jobs, NOW - 86_400 + 1),
            Err(Refusal::Expired)
        );
        // within the lifetime of the newest command, order does not matter
        let earlier = command(|m| {
            m.insert("job".into(), "c".repeat(32).into());
            m.insert("iat".into(), (NOW - 100).into());
            m.insert("exp".into(), (NOW + 20).into());
        });
        assert!(admit(&earlier, &mut jobs).is_ok());
    }

    #[test]
    fn rate_limits() {
        let mut limiter = RateLimiter::default();
        let t0 = Instant::now();
        for i in 0..10 {
            assert!(limiter.admit(1, t0 + Duration::from_secs(i)), "command {i}");
        }
        // the 11th command in a minute
        assert!(!limiter.admit(1, t0 + Duration::from_secs(30)));
        assert!(limiter.admit(1, t0 + Duration::from_secs(60)));
        let mut limiter = RateLimiter::default();
        assert!(limiter.admit(80, t0));
        assert!(limiter.admit(40, t0));
        assert!(!limiter.admit(1, t0 + Duration::from_secs(59)));
        assert!(limiter.admit(80, t0 + Duration::from_secs(60)));
    }
}

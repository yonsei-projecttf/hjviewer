// SPDX-License-Identifier: AGPL-3.0-only

//! The public side of the update keys: what a build compiles in, read from the public file
//! `client/update-keys.env`.
//!
//! ```text
//! HJV_UPDATE_PUBKEY=1:<base64>[,2:<base64>]           signing keys: the current one and the next
//! HJV_UPDATE_ROOT_PUBKEY=101:<base64>[,102:<base64>]  root keys: two, kept offline apart
//! HJV_POLICY_FLOOR=<epoch>:<seq>                      the live policy's position at the build
//! ```
//!
//! Key lists follow hjv-relay's `HJV_COMMAND_PUBKEY` (relay/PROTOCOL.md 3.2): standard base64 of
//! the 32-byte Ed25519 public key, no weak key, no id twice. Signing keys are numbered 1-100 and
//! root keys 101-199 (trust rules 1-4), so a root key's number is never one a declaration may
//! revoke. Also here: the same key may not serve under two ids or two roles, and a key is a point
//! of the prime-order
//! subgroup, as every honest key is. The apps refuse any other point (libsodium's conversion
//! check in the client core), and a build drops its whole key set over one refused key: a key
//! file this tool passes must be one the apps take.
//!
//! The file also knows the test keys of this repository, whose private halves are published:
//! a build for the real server must never trust one of them.

use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, fmt, sync::OnceLock};

pub const SIGNING_VAR: &str = "HJV_UPDATE_PUBKEY";
pub const ROOT_VAR: &str = "HJV_UPDATE_ROOT_PUBKEY";
pub const FLOOR_VAR: &str = "HJV_POLICY_FLOOR";
/// At most this many signing keys: the current one and the next, for a planned rotation.
pub const MAX_SIGNING_KEYS: usize = 2;
/// At most this many root keys: 1.6.0 pins two, each kept offline on its own media.
pub const MAX_ROOT_KEYS: usize = 2;
/// The numbers of signing keys and of root keys (trust rules 1-4).
pub const SIGNING_IDS: std::ops::RangeInclusive<u32> = 1..=100;
pub const ROOT_IDS: std::ops::RangeInclusive<u32> = 101..=199;
const FINGERPRINT_CONTEXT: &[u8] = b"hjv-update-keys-v1";

/// What a key may sign. A signing key signs the policies; a root key signs the declarations (a
/// relaxation, a revocation, a new epoch), and a policy only while no signing key serves every
/// recorded build (trust rules 4-2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Signing,
    Root,
}

impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Role::Signing => "signing",
            Role::Root => "root",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "signing" => Some(Role::Signing),
            "root" => Some(Role::Root),
            _ => None,
        }
    }

    /// For messages.
    pub fn korean(self) -> &'static str {
        match self {
            Role::Signing => "서명 키",
            Role::Root => "루트 키",
        }
    }

    /// The key numbers of this role.
    pub fn ids(self) -> std::ops::RangeInclusive<u32> {
        match self {
            Role::Signing => SIGNING_IDS,
            Role::Root => ROOT_IDS,
        }
    }

    /// The role a key number belongs to, if any.
    pub fn of_id(id: u32) -> Option<Role> {
        [Role::Signing, Role::Root]
            .into_iter()
            .find(|role| role.ids().contains(&id))
    }
}

/// A key number of a role: signing keys 1-100, root keys 101-199.
pub fn check_id(role: Role, id: u32) -> Result<(), String> {
    if role.ids().contains(&id) {
        Ok(())
    } else {
        Err(format!(
            "{}의 번호는 {}-{}입니다(서명 키 1-100, 루트 키 101-199): {id}",
            role.korean(),
            role.ids().start(),
            role.ids().end()
        ))
    }
}

/// A public key and the number envelopes name it by (`k`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicKey {
    pub id: u32,
    pub key: VerifyingKey,
}

impl fmt::Display for PublicKey {
    /// `<key id>:<base64 key>`, one entry of a key list.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.id, STANDARD.encode(self.key.to_bytes()))
    }
}

/// A key id in decimal, 1-65535, without sign or leading zeros.
pub fn parse_key_id(s: &str) -> Result<u32, String> {
    s.parse::<u32>()
        .ok()
        .filter(|n| (1..=65535).contains(n) && s.bytes().all(|b| b.is_ascii_digit()))
        .filter(|_| !s.starts_with('0'))
        .ok_or_else(|| format!("키 번호는 1-65535입니다: {s:?}"))
}

/// Parses `<key id>:<base64 key>[,<key id>:<base64 key>...]` with at most `max` keys.
pub fn parse_key_list(s: &str, max: usize, what: &str) -> Result<Vec<PublicKey>, String> {
    let mut keys: Vec<PublicKey> = Vec::new();
    for entry in s.split(',').map(str::trim) {
        let (id, key) = entry.split_once(':').ok_or_else(|| {
            format!("{what}: <키 번호>:<base64 공개키> 형식이 아닙니다({entry:?})")
        })?;
        let id = parse_key_id(id.trim()).map_err(|e| format!("{what}: {e}"))?;
        let bytes = STANDARD
            .decode(key.trim())
            .map_err(|_| format!("{what}: 키 번호 {id}의 키가 base64가 아닙니다"))?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| format!("{what}: 키 번호 {id}의 키가 32바이트가 아닙니다"))?;
        let key = VerifyingKey::from_bytes(&bytes)
            .map_err(|_| format!("{what}: 키 번호 {id}의 키가 Ed25519 공개키가 아닙니다"))?;
        if key.is_weak() {
            return Err(format!("{what}: 키 번호 {id}의 키가 약한 키입니다"));
        }
        // on the curve and not of small order, but with a small-order part: no key pair has
        // such a public key, and the apps refuse it
        if !key.to_edwards().is_torsion_free() {
            return Err(format!(
                "{what}: 키 번호 {id}의 키가 주 부분군 밖의 점입니다(앱이 받지 않는 키)"
            ));
        }
        if keys.iter().any(|k| k.id == id) {
            return Err(format!("{what}: 같은 키 번호가 두 번 나옵니다({id})"));
        }
        if keys.iter().any(|k| k.key == key) {
            return Err(format!("{what}: 같은 키가 두 번 나옵니다(번호 {id})"));
        }
        keys.push(PublicKey { id, key });
    }
    if keys.len() > max {
        return Err(format!("{what}: 키는 {max}개까지입니다"));
    }
    Ok(keys)
}

/// The oldest policy position a build accepts: a policy below it (by epoch, then seq) is ignored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Floor {
    pub epoch: u64,
    pub seq: u64,
}

impl Floor {
    pub fn parse(s: &str) -> Result<Floor, String> {
        let number = |t: &str| -> Option<u64> {
            let canonical = !t.is_empty()
                && t.len() <= 15
                && t.bytes().all(|b| b.is_ascii_digit())
                && (t == "0" || !t.starts_with('0'));
            canonical.then(|| t.parse().ok()).flatten()
        };
        s.split_once(':')
            .and_then(|(epoch, seq)| Some((number(epoch)?, number(seq)?)))
            .filter(|(epoch, _)| *epoch >= 1)
            .map(|(epoch, seq)| Floor { epoch, seq })
            .ok_or_else(|| format!("{FLOOR_VAR}는 <세대>:<seq>입니다(예: 1:0): {s:?}"))
    }
}

impl fmt::Display for Floor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.epoch, self.seq)
    }
}

/// The keys and the floor of a build.
#[derive(Clone, Debug)]
pub struct KeySet {
    pub signing: Vec<PublicKey>,
    /// One or two root keys.
    pub roots: Vec<PublicKey>,
    pub floor: Floor,
    /// The values as the file has them (trimmed).
    pub signing_line: String,
    pub root_line: String,
}

impl KeySet {
    /// Reads `client/update-keys.env`: `NAME=value` lines, `#` comments and empty lines.
    pub fn parse(text: &str) -> Result<KeySet, String> {
        let mut values: [Option<String>; 3] = [None, None, None];
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (name, value) = line
                .split_once('=')
                .ok_or_else(|| format!("키 파일 {}번째 줄이 이름=값 형식이 아닙니다", n + 1))?;
            let slot = [SIGNING_VAR, ROOT_VAR, FLOOR_VAR]
                .iter()
                .position(|known| *known == name)
                .ok_or_else(|| format!("키 파일 {}번째 줄: 모르는 이름 {name:?}", n + 1))?;
            if values[slot].is_some() {
                return Err(format!("키 파일에 {name} 줄이 둘입니다"));
            }
            values[slot] = Some(value.trim().to_owned());
        }
        let [signing, root, floor] = values.map(Option::unwrap_or_default);
        KeySet::from_values(&signing, &root, &floor)
    }

    /// From the three values, as a build gets them.
    pub fn from_values(signing: &str, root: &str, floor: &str) -> Result<KeySet, String> {
        let (signing, root, floor) = (signing.trim(), root.trim(), floor.trim());
        if signing.is_empty() && root.is_empty() {
            return Err(format!(
                "업데이트 공개키가 비어 있습니다({SIGNING_VAR}, {ROOT_VAR}). 이런 빌드는 정책을 확인하지 않습니다"
            ));
        }
        if signing.is_empty() || root.is_empty() || floor.is_empty() {
            return Err(format!(
                "{SIGNING_VAR}, {ROOT_VAR}, {FLOOR_VAR}는 셋 다 있어야 합니다"
            ));
        }
        let signing_keys = parse_key_list(signing, MAX_SIGNING_KEYS, SIGNING_VAR)?;
        let root_keys = parse_key_list(root, MAX_ROOT_KEYS, ROOT_VAR)?;
        for root_key in &root_keys {
            if signing_keys.iter().any(|k| k.id == root_key.id) {
                return Err(format!(
                    "루트 키 번호({})가 서명 키 번호와 같습니다. 서로 달라야 합니다",
                    root_key.id
                ));
            }
            if signing_keys.iter().any(|k| k.key == root_key.key) {
                return Err("루트 키와 서명 키가 같은 키입니다".to_owned());
            }
        }
        for key in &signing_keys {
            check_id(Role::Signing, key.id).map_err(|e| format!("{SIGNING_VAR}: {e}"))?;
        }
        for key in &root_keys {
            check_id(Role::Root, key.id).map_err(|e| format!("{ROOT_VAR}: {e}"))?;
        }
        Ok(KeySet {
            signing: signing_keys,
            roots: root_keys,
            floor: Floor::parse(floor)?,
            signing_line: signing.to_owned(),
            root_line: root.to_owned(),
        })
    }

    /// The key an envelope's `k` names, and its role.
    pub fn find(&self, id: u32) -> Option<(Role, &PublicKey)> {
        self.all().find(|(_, k)| k.id == id)
    }

    /// The pinned root key of that number.
    pub fn root(&self, id: u32) -> Option<&PublicKey> {
        self.roots.iter().find(|k| k.id == id)
    }

    pub fn is_root(&self, id: u32) -> bool {
        self.root(id).is_some()
    }

    pub fn all(&self) -> impl Iterator<Item = (Role, &PublicKey)> {
        self.signing
            .iter()
            .map(|k| (Role::Signing, k))
            .chain(self.roots.iter().map(|k| (Role::Root, k)))
    }

    /// Whether this key set pins `key` under its number and role.
    pub fn pins(&self, role: Role, key: &PublicKey) -> bool {
        self.find(key.id)
            .is_some_and(|(r, k)| r == role && k.key == key.key)
    }

    /// Whether any key is a test key of this repository (anyone can sign with those).
    pub fn has_test_key(&self) -> bool {
        self.all().any(|(_, k)| is_test_key(&k.key))
    }

    /// The name of a key set (trust rules 1-4): SHA-256("hjv-update-keys-v1" NUL server NUL
    /// signing keys NUL root keys), the keys written as `<id>:<base64>` in the order of their
    /// numbers with a comma between. So the same keys give the same fingerprint however the file
    /// spells or orders them.
    pub fn fingerprint(&self, server: &str) -> [u8; 32] {
        let line = |keys: &[PublicKey]| {
            let mut keys: Vec<&PublicKey> = keys.iter().collect();
            keys.sort_by_key(|k| k.id);
            keys.iter()
                .map(|k| k.to_string())
                .collect::<Vec<String>>()
                .join(",")
        };
        let mut h = Sha256::new();
        h.update(FINGERPRINT_CONTEXT);
        for part in [server, &line(&self.signing), &line(&self.roots)] {
            h.update([0u8]);
            h.update(part.as_bytes());
        }
        h.finalize().into()
    }

    /// The three lines of the key file.
    pub fn to_text(&self) -> String {
        format!(
            "{SIGNING_VAR}={}\n{ROOT_VAR}={}\n{FLOOR_VAR}={}\n",
            self.signing_line, self.root_line, self.floor
        )
    }
}

/// A key pair whose seed is written in this repository, for tests and vectors only.
pub struct TestKey {
    pub name: &'static str,
    pub role: Role,
    pub id: u32,
    /// The seed is the 32 bytes `first, first + 1, ...`.
    pub first: u8,
    pub about: &'static str,
}

impl TestKey {
    pub fn seed(&self) -> [u8; 32] {
        range_seed(self.first)
    }

    pub fn public(&self) -> PublicKey {
        PublicKey {
            id: self.id,
            key: SigningKey::from_bytes(&self.seed()).verifying_key(),
        }
    }
}

/// The update keys the tests, the vectors and the E2E use. Seeds follow the relay's vectors
/// (relay/PROTOCOL.md 13 uses 00.. and 20..) and the research vector (40.. and 60..).
pub const TEST_KEYS: [TestKey; 6] = [
    TestKey {
        name: "signing-1",
        role: Role::Signing,
        id: 1,
        first: 0x40,
        about: "signing key 1 of the vectors (seed 40 41 .. 5f)",
    },
    TestKey {
        name: "signing-2",
        role: Role::Signing,
        id: 2,
        first: 0x80,
        about: "signing key 2 of the vectors (seed 80 81 .. 9f)",
    },
    TestKey {
        name: "root-101",
        role: Role::Root,
        id: 101,
        first: 0xa0,
        about: "root key 101 of the vectors (seed a0 a1 .. bf)",
    },
    TestKey {
        name: "signing-3",
        role: Role::Signing,
        id: 3,
        first: 0xc0,
        about: "a signing key no vector build trusts (seed c0 c1 .. df)",
    },
    TestKey {
        name: "root-102",
        role: Role::Root,
        id: 102,
        first: 0xe0,
        about: "root key 102 of the vectors (seed e0 e1 .. ff)",
    },
    TestKey {
        name: "root-103",
        role: Role::Root,
        id: 103,
        first: 0x10,
        about: "a root key no vector build trusts (seed 10 11 .. 2f)",
    },
];

pub fn test_key(name: &str) -> Option<&'static TestKey> {
    TEST_KEYS.iter().find(|k| k.name == name)
}

/// The key set of the vectors and of E2E builds: signing keys 1 and 2, root keys 101 and 102,
/// floor 1:0.
pub fn test_key_set() -> KeySet {
    let line = |name: &str| {
        test_key(name)
            .map(|k| k.public().to_string())
            .unwrap_or_default()
    };
    KeySet::from_values(
        &format!("{},{}", line("signing-1"), line("signing-2")),
        &format!("{},{}", line("root-101"), line("root-102")),
        "1:0",
    )
    .expect("the test keys are valid")
}

fn range_seed(first: u8) -> [u8; 32] {
    let mut seed = [0u8; 32];
    for (i, b) in seed.iter_mut().enumerate() {
        *b = first.wrapping_add(i as u8);
    }
    seed
}

fn public_of(seed: &[u8; 32]) -> [u8; 32] {
    SigningKey::from_bytes(seed).verifying_key().to_bytes()
}

/// The Ed25519 seeds RFC 8032 prints in section 7.1 (TEST 1 to 3): examples that tests copy.
const RFC_8032_SEEDS: [&str; 3] = [
    "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
    "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
    "c5aa8df43f9f837bedb7442f31dcb7b166d38535076f094b85ce3a2e0b4458f7",
];

/// 32 bytes from 64 hex digits.
fn bytes_from_hex(hex: &str) -> Option<[u8; 32]> {
    let digits = hex.as_bytes();
    if digits.len() != 64 || !digits.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    let mut bytes = [0u8; 32];
    for (byte, pair) in bytes.iter_mut().zip(hex.as_bytes().chunks(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(bytes)
}

/// Every key pair a test in this repository could plausibly have published: seeds of 32 equal
/// bytes, seeds of 32 consecutive bytes and the examples of RFC 8032. The relay's vectors and
/// tests, the research vector (build/minver) and the keys above are all of the first two kinds.
fn published() -> &'static HashSet<[u8; 32]> {
    static KEYS: OnceLock<HashSet<[u8; 32]>> = OnceLock::new();
    KEYS.get_or_init(|| {
        let examples = RFC_8032_SEEDS.iter().filter_map(|hex| bytes_from_hex(hex));
        (0..=255u8)
            .flat_map(|b| [[b; 32], range_seed(b)])
            .chain(examples)
            .map(|seed| public_of(&seed))
            .collect()
    })
}

/// Whether the private half of `key` is published in this repository.
pub fn is_test_key(key: &VerifyingKey) -> bool {
    published().contains(&key.to_bytes())
}

/// The named test keys with their public keys in base64, for lists that refuse them elsewhere
/// (CI, packaging scripts).
pub fn named_test_keys() -> Vec<(String, String)> {
    let b64 = |seed: [u8; 32]| STANDARD.encode(public_of(&seed));
    let mut list: Vec<(String, String)> = TEST_KEYS
        .iter()
        .map(|k| (b64(k.seed()), k.about.to_owned()))
        .collect();
    list.push((
        b64(range_seed(0x60)),
        "the device key of the research vector (seed 60 61 .. 7f)".to_owned(),
    ));
    list.push((
        b64(range_seed(0x00)),
        "relay/PROTOCOL.md 13, relay key (seed 00 01 .. 1f)".to_owned(),
    ));
    list.push((
        b64(range_seed(0x20)),
        "relay/PROTOCOL.md 13, command key (seed 20 21 .. 3f)".to_owned(),
    ));
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64_key(seed: [u8; 32]) -> String {
        STANDARD.encode(public_of(&seed))
    }

    #[test]
    fn key_lists() {
        let (a, b) = (b64_key([1; 32]), b64_key([2; 32]));
        let keys = parse_key_list(&format!(" 1:{a} , 2:{b} "), 2, "t").unwrap();
        assert_eq!(keys.iter().map(|k| k.id).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(keys[0].to_string(), format!("1:{a}"));
        for bad in [
            String::new(),
            a.clone(),
            format!("0:{a}"),
            format!("x:{a}"),
            format!("+1:{a}"),
            format!("01:{a}"),
            format!("65536:{a}"),
            format!("1:{}", &a[..a.len() - 2]),
            "1:AAAA".to_owned(),
            format!("1:{a},1:{b}"),
            format!("1:{a},2:{a}"),
            format!("1:{a},2:{b},3:{}", b64_key([3; 32])),
            format!("1:{a},"),
            // the identity point, a weak key
            "1:AQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_owned(),
        ] {
            assert!(parse_key_list(&bad, 2, "t").is_err(), "{bad:?} parsed");
        }
    }

    #[test]
    fn keys_outside_the_prime_order_subgroup() {
        use curve25519_dalek::constants::EIGHT_TORSION;
        // An honest public key plus a point of order 2, 4 or 8: on the curve and not of small
        // order, so ed25519-dalek reads it and does not call it weak. The apps refuse it.
        let honest = SigningKey::from_bytes(&[1; 32]).verifying_key();
        assert!(parse_key_list(&format!("1:{}", b64_key([1; 32])), 1, "t").is_ok());
        for torsion in EIGHT_TORSION.iter().skip(1) {
            let mixed = (honest.to_edwards() + torsion).compress().to_bytes();
            let read = VerifyingKey::from_bytes(&mixed).unwrap();
            assert!(!read.is_weak() && !read.to_edwards().is_small_order());
            let err = parse_key_list(&format!("1:{}", STANDARD.encode(mixed)), 1, "t").unwrap_err();
            assert!(err.contains("주 부분군 밖"), "{err}");
        }
        // the small-order points themselves are weak keys, as before
        for torsion in EIGHT_TORSION {
            let line = format!("1:{}", STANDARD.encode(torsion.compress().to_bytes()));
            assert!(parse_key_list(&line, 1, "t")
                .unwrap_err()
                .contains("약한 키"));
        }
        // The same kind of point made another way (Python, from the curve's formulas): a key
        // plus a point of order 8, 4 and 2. And the honest key they were made from.
        for mixed in [
            "vsDdLZJCpofT3uginHmeFb7BOUIPi1yhbQz5QYrKBnc=",
            "C7jhjpJ98e17p5CPfRkxI5dM954qGv88gAzsUIpJMl8=",
            "+pkv4QnhqNXVi0y+/uF32tQb3eseQBW+FjGDYRMBlds=",
        ] {
            let err = parse_key_list(&format!("1:{mixed}"), 1, "t").unwrap_err();
            assert!(err.contains("주 부분군 밖"), "{mixed}: {err}");
        }
        let honest = "82XQHvYeVyoqdLNBAR6IJSvkIhThv+pB6c58nuz+aiQ=";
        assert!(parse_key_list(&format!("1:{honest}"), 1, "t").is_ok());
        // one such key anywhere makes the key file unusable, as it does for a build
        let good = test_key_set();
        let mixed = (honest_point() + EIGHT_TORSION[1]).compress().to_bytes();
        let line = format!("{},2:{}", good.signing[0], STANDARD.encode(mixed));
        assert!(KeySet::from_values(&line, &good.root_line, "1:0")
            .unwrap_err()
            .contains("주 부분군 밖"));
        let root = format!("101:{}", STANDARD.encode(mixed));
        assert!(KeySet::from_values(&good.signing_line, &root, "1:0")
            .unwrap_err()
            .contains("주 부분군 밖"));
    }

    fn honest_point() -> curve25519_dalek::edwards::EdwardsPoint {
        SigningKey::from_bytes(&[5; 32])
            .verifying_key()
            .to_edwards()
    }

    #[test]
    fn floors() {
        assert_eq!(Floor::parse("1:0").unwrap(), Floor { epoch: 1, seq: 0 });
        assert_eq!(Floor::parse("2:17").unwrap().to_string(), "2:17");
        assert!(Floor { epoch: 1, seq: 9 } < Floor { epoch: 2, seq: 0 });
        assert!(Floor { epoch: 1, seq: 9 } < Floor { epoch: 1, seq: 10 });
        for bad in [
            "", "1", "0:0", "1:", ":1", "1:-1", "1:01", "01:1", "1:1:1", "a:1", "1:+1", "1: 1",
        ] {
            assert!(Floor::parse(bad).is_err(), "{bad:?} parsed");
        }
    }

    #[test]
    fn key_files() {
        let (a, b, r) = (b64_key([1; 32]), b64_key([2; 32]), b64_key([3; 32]));
        let text = format!(
            "# update keys\n\n{SIGNING_VAR}=1:{a},2:{b}\n{ROOT_VAR}=101:{r}\n{FLOOR_VAR}=1:0\n"
        );
        let set = KeySet::parse(&text).unwrap();
        assert_eq!(set.signing.len(), 2);
        assert_eq!(set.roots.len(), 1);
        assert_eq!(set.roots[0].id, 101);
        assert_eq!(set.find(2).unwrap().0, Role::Signing);
        assert_eq!(set.find(101).unwrap().0, Role::Root);
        assert!(set.find(3).is_none());
        assert_eq!(
            set.to_text(),
            format!("{SIGNING_VAR}=1:{a},2:{b}\n{ROOT_VAR}=101:{r}\n{FLOOR_VAR}=1:0\n")
        );
        assert!(set.has_test_key());
        // one signing key is enough; two root keys are pinned by 1.6.0, in any order
        assert!(KeySet::from_values(&format!("1:{a}"), &format!("101:{r}"), "1:0").is_ok());
        let two =
            KeySet::from_values(&format!("1:{a}"), &format!("102:{b},101:{r}"), "1:0").unwrap();
        assert_eq!(two.find(102).unwrap().0, Role::Root);
        assert!(two.is_root(101) && two.is_root(102) && !two.is_root(1));
        // the number ranges: signing keys 1-100, root keys 101-199
        let c = b64_key([4; 32]);
        assert!(KeySet::from_values(&format!("100:{a}"), &format!("199:{r}"), "1:0").is_ok());
        for (signing, root) in [
            (format!("101:{a}"), format!("102:{r}")),
            (format!("1:{a}"), format!("100:{r}")),
            (format!("1:{a}"), format!("200:{r}")),
            (format!("1:{a}"), format!("101:{r},7:{b}")),
            (format!("1:{a}"), format!("101:{r},102:{b},103:{c}")),
        ] {
            assert!(
                KeySet::from_values(&signing, &root, "1:0").is_err(),
                "{signing} {root} parsed"
            );
        }
        assert!(check_id(Role::Signing, 100).is_ok() && check_id(Role::Signing, 101).is_err());
        assert!(check_id(Role::Root, 101).is_ok() && check_id(Role::Root, 200).is_err());
        assert_eq!(Role::of_id(55), Some(Role::Signing));
        assert_eq!(Role::of_id(150), Some(Role::Root));
        assert_eq!(Role::of_id(200), None);

        let empty = KeySet::parse(&format!("{SIGNING_VAR}=\n{ROOT_VAR}=\n{FLOOR_VAR}=\n"));
        assert!(empty.unwrap_err().contains("비어"));
        assert!(KeySet::parse("").unwrap_err().contains("비어"));
        for bad in [
            format!("{SIGNING_VAR}=1:{a}\n{FLOOR_VAR}=1:0\n"),
            format!("{ROOT_VAR}=101:{r}\n{FLOOR_VAR}=1:0\n"),
            format!("{SIGNING_VAR}=1:{a}\n{ROOT_VAR}=101:{r}\n"),
            format!("{SIGNING_VAR}=1:{a}\n{ROOT_VAR}=1:{r}\n{FLOOR_VAR}=1:0\n"),
            format!("{SIGNING_VAR}=1:{a}\n{ROOT_VAR}=101:{a}\n{FLOOR_VAR}=1:0\n"),
            format!("{SIGNING_VAR}=1:{a},2:{b}\n{ROOT_VAR}=101:{r},102:{b}\n{FLOOR_VAR}=1:0\n"),
            format!(
                "{SIGNING_VAR}=1:{a}\n{SIGNING_VAR}=2:{b}\n{ROOT_VAR}=101:{r}\n{FLOOR_VAR}=1:0\n"
            ),
            format!(
                "{SIGNING_VAR}=1:{a}\n{ROOT_VAR}=101:{r}\n{FLOOR_VAR}=1:0\nHJV_UPDATE_PUBKEYS=x\n"
            ),
            format!("{SIGNING_VAR}=1:{a}\n{ROOT_VAR}=101:{r}\n{FLOOR_VAR}=1:0\nnot a line\n"),
            format!("{SIGNING_VAR}=\"1:{a}\"\n{ROOT_VAR}=101:{r}\n{FLOOR_VAR}=1:0\n"),
        ] {
            assert!(KeySet::parse(&bad).is_err(), "{bad:?} parsed");
        }
    }

    #[test]
    fn the_test_keys() {
        // the research vector's update key (build/minver/policy-distribution, vector.json)
        let k1 = test_key("signing-1").unwrap();
        assert_eq!(
            k1.public().to_string(),
            "1:JUO5L/EJVRFHatyDadtt3JM2ZaEZeN2hQE7hBmypVZ0="
        );
        let set = test_key_set();
        assert_eq!(set.signing.len(), 2);
        assert_eq!(
            set.roots.iter().map(|k| k.id).collect::<Vec<_>>(),
            [101, 102]
        );
        assert_eq!(set.floor, Floor { epoch: 1, seq: 0 });
        assert!(set.has_test_key());
        for key in &TEST_KEYS {
            assert!(is_test_key(&key.public().key), "{}", key.name);
        }
        let named = named_test_keys();
        // the relay's vectors (scripts/package-relay.sh lists the same keys) and the device key
        for known in [
            "Kay64UG8yvCyLhqU000LxzYeUm0L/hLIl5S8kyKWbdc=",
            "A6EHv/POEL4dcN0Y50vAmWfk1jCbpQ1fHdyGZBJVMbg=",
            "F0VTtFbd38aQjsqxwQH+arIeK6oGF3lbfUOmNIKZP9U=",
            "JUO5L/EJVRFHatyDadtt3JM2ZaEZeN2hQE7hBmypVZ0=",
        ] {
            assert!(named.iter().any(|(k, _)| k == known), "{known}");
        }
        // the other test keys of scripts/package-relay.sh are seeds of equal bytes
        for known in [
            "GX9rI+FshTLGq8g4+s1ep4m+DHaykgM0A5v6iz02jWE=",
            "RQigeqlBcH8+stuUyIl6gLLBGXR2tt4hOsJz332GxP8=",
            "iojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1w=",
            "/RckOFqgx1tk+3jNYC+h2ZH96/drE8WO1wLqyDXp9hg=",
        ] {
            let bytes: [u8; 32] = STANDARD.decode(known).unwrap().try_into().unwrap();
            assert!(
                is_test_key(&VerifyingKey::from_bytes(&bytes).unwrap()),
                "{known}"
            );
        }
        // the public keys RFC 8032 7.1 prints for its TEST 1 to 3
        for known in [
            "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
            "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
            "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025",
        ] {
            let bytes = bytes_from_hex(known).unwrap();
            assert!(
                is_test_key(&VerifyingKey::from_bytes(&bytes).unwrap()),
                "{known}"
            );
        }
        assert_eq!(published().len(), 2 * 256 + 3);
        assert_eq!(bytes_from_hex("00"), None);
        assert_eq!(bytes_from_hex(&"0g".repeat(32)), None);
        // a fresh key is not a test key
        let mut seed = [0u8; 32];
        getrandom::getrandom(&mut seed).unwrap();
        assert!(!is_test_key(&SigningKey::from_bytes(&seed).verifying_key()));
    }

    #[test]
    fn fingerprints() {
        let set = test_key_set();
        let a = set.fingerprint("rd.example.com");
        assert_ne!(a, set.fingerprint("hjv.test"));
        // the client core's value for the same build (src/hjviewer_update/vectors.rs there)
        assert_eq!(
            crate::trust::hex(&a),
            "3d5fdc2ae12abd2dac598179aff56bdf121f60ea4089ba01dbef11f22399fb8b"
        );
        // the order and the spacing of the file do not matter, the keys do
        let (k1, k2) = (set.signing[0].to_string(), set.signing[1].to_string());
        let swapped =
            KeySet::from_values(&format!(" {k2} , {k1} "), &set.root_line, "1:0").unwrap();
        assert_eq!(swapped.fingerprint("rd.example.com"), a);
        let one = KeySet::from_values(&k1, &set.root_line, "1:0").unwrap();
        assert_eq!(
            crate::trust::hex(&one.fingerprint("rd.example.com")),
            "32192c380b4c670cd40007656d76066ce5c7c7239295388e535c93085e72f4e8"
        );
        let mut h = Sha256::new();
        h.update(b"hjv-update-keys-v1\0rd.example.com\0");
        h.update(set.signing_line.as_bytes());
        h.update(b"\0");
        h.update(set.root_line.as_bytes());
        let expected: [u8; 32] = h.finalize().into();
        assert_eq!(a, expected);
    }
}

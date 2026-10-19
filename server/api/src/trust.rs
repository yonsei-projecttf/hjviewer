// SPDX-License-Identifier: AGPL-3.0-only

//! Trust lists (design v3 §3-6): what hjv-api signs for a bound PC so that its service lets the
//! account's devices in without the PC's password. The PC asks for its list itself (pcsync.rs)
//! and takes only an answer to that very request, signed by a list key its build pins
//! (`HJV_TRUST_PUBKEYS`, the same value as hjv-api's `HJV_API_TRUST_PUBKEYS`).
//!
//! The list is UTF-8 JSON with its fields in this order (the bytes are what is signed, so the
//! order is fixed: [`TrustList`] serializes it):
//!
//! ```text
//! {"v":1,"typ":"hjv-trust","kid":1,
//!  "binding":"<32 hex>","pc_id":"123456789","pc_pk":"<base64 32>",
//!  "req":"<the request's n>",
//!  "serial":4182,"iat":1760250001,"exp":1760336401,
//!  "refresh":120,"strict":false,"state":"active","owner":"홍*동","login_at":1760249950,
//!  "keys":[{"pk":"<base64 32>","id":"987654321","name":"iPhone 14 Pro","platform":"ios",
//!           "kinds":["remote","file","camera"],"ks":4180}]}
//! ```
//!
//! and its signature is the list key's Ed25519 signature over `"hjv-trust-v1" NUL` and the list
//! bytes ([`message`]). `state` is `active` (with the account's keys that are not revoked),
//! `disabled` (switched off, or the account is off the allowlist or blocked: no keys) or
//! `revoked` (the binding the PC sent was released: no keys, `pc_pk` the key its request was
//! checked with, `binding` the value it sent). `exp − iat` is HJV_API_TRUST_TTL, never more than
//! the apps' bound of 72 hours.
//!
//! The list key is an Ed25519 key with a key number (1 on the server, 2 kept offline for the
//! change, design v3 §5-3): its 32-byte seed is the credential hjviewer-api-list-key (`hjv-api
//! keygen list --kid N`) and HJV_API_LIST_KID its number. Public keys are written `<kid>:<base64>`
//! wherever they appear, and shown by their fingerprint `<kid>:<hex SHA-256 of the 32 bytes>`, as
//! the app build's summary shows them (design v3 §6-4).

use crate::util::{b64, b64_decode, hex, sha256};
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use serde::Serialize;

pub const CONTEXT: &[u8] = b"hjv-trust-v1";
/// The longest a list may live, `exp − iat`: the apps refuse longer ones (MAX_LIST_TTL).
pub const MAX_TTL: i64 = 72 * 3600;
/// The PCs hold the server's `refresh` to these bounds; hjv-api takes no setting outside them.
pub const MIN_REFRESH: i64 = 5;
pub const MAX_REFRESH: i64 = 300;
/// Key numbers, as the apps and the build check take them.
pub const MAX_KID: u32 = 65_535;

/// The bytes a list key signs.
pub fn message(list: &[u8]) -> Vec<u8> {
    [CONTEXT, &[0u8], list].concat()
}

/// The list key in use, with its number.
pub struct ListKey {
    kid: u32,
    key: SigningKey,
}

impl std::fmt::Debug for ListKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ListKey({})", entry(self.kid, &self.public_key()))
    }
}

impl ListKey {
    pub fn new(kid: u32, seed: &[u8; 32]) -> ListKey {
        ListKey {
            kid,
            key: SigningKey::from_bytes(seed),
        }
    }

    pub fn kid(&self) -> u32 {
        self.kid
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.key.verifying_key().to_bytes()
    }

    /// The signature of a list's bytes.
    pub fn sign(&self, list: &[u8]) -> [u8; 64] {
        self.sign_message(&message(list))
    }

    /// The signature of bytes that begin with their own purpose string: a list's ([`message`])
    /// or, from 1.9.0, a support grant's (grant.rs, `hjv-support-v1`).
    pub fn sign_message(&self, signed: &[u8]) -> [u8; 64] {
        self.key.sign(signed).to_bytes()
    }
}

/// `<kid>:<base64 public key>`, as HJV_TRUST_PUBKEYS and HJV_API_TRUST_PUBKEYS hold it.
pub fn entry(kid: u32, public_key: &[u8; 32]) -> String {
    format!("{kid}:{}", b64(public_key))
}

/// A public key's fingerprint: `<kid>:<lowercase hex SHA-256 of its 32 bytes>`.
pub fn fingerprint(kid: u32, public_key: &[u8; 32]) -> String {
    format!("{kid}:{}", hex(&sha256(public_key)))
}

/// `<kid>:<base64>[,<kid>:<base64>…]`: key numbers 1 to 65535, each once, each key an Ed25519
/// public key that is not of small order and appears once. Empty text, no keys.
pub fn parse_pubkeys(text: &str) -> Result<Vec<(u32, [u8; 32])>, String> {
    let mut keys: Vec<(u32, [u8; 32])> = Vec::new();
    if text.trim().is_empty() {
        return Ok(keys);
    }
    for part in text.split(',') {
        let (kid, key) = part
            .trim()
            .split_once(':')
            .ok_or("an entry is not <key number>:<base64 public key>")?;
        let kid = kid
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|k| (1..=MAX_KID).contains(k) && !kid.trim().starts_with('0'))
            .ok_or("key numbers are 1 to 65535")?;
        let key: [u8; 32] = b64_decode(key.trim())
            .and_then(|k| k.try_into().ok())
            .ok_or_else(|| format!("key {kid} is not the standard base64 of 32 bytes"))?;
        let usable = VerifyingKey::from_bytes(&key).is_ok_and(|k| !k.is_weak());
        if !usable {
            return Err(format!(
                "key {kid} is not an Ed25519 public key one can rely on"
            ));
        }
        if keys.iter().any(|(k, raw)| *k == kid || *raw == key) {
            return Err(format!("key {kid}, or its key, appears twice"));
        }
        keys.push((kid, key));
    }
    Ok(keys)
}

/// The owner as a PC shows it next to a same-account connection (design v3 §1-5): the first and
/// the last character of the name and stars between ("홍길동" → "홍*동", "홍길" → "홍*").
pub fn mask_name(name: &str) -> String {
    let chars: Vec<char> = name.trim().chars().filter(|c| !c.is_control()).collect();
    match chars.len() {
        0 => String::new(),
        1 => "*".to_owned(),
        2 => format!("{}*", chars[0]),
        n => format!("{}{}{}", chars[0], "*".repeat(n - 2), chars[n - 1]),
    }
}

/// What a list says about the PC's binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Active,
    Disabled,
    Revoked,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Active => "active",
            State::Disabled => "disabled",
            State::Revoked => "revoked",
        }
    }
}

/// One account key on a list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListEntry {
    /// standard base64 of the 32-byte public key
    pub pk: String,
    /// the device ID of the key's login, which the proof's LoginRequest.my_id must equal
    pub id: String,
    /// the server's name of the device (devmodel.rs)
    pub name: String,
    pub platform: String,
    pub kinds: Vec<String>,
    /// the change serial the key's registration made
    pub ks: i64,
}

/// A trust list, its fields in the signed order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrustList {
    pub v: u8,
    pub typ: &'static str,
    pub kid: u32,
    pub binding: String,
    pub pc_id: String,
    pub pc_pk: String,
    pub req: String,
    pub serial: i64,
    pub iat: i64,
    pub exp: i64,
    pub refresh: i64,
    pub strict: bool,
    pub state: &'static str,
    pub owner: String,
    pub login_at: i64,
    pub keys: Vec<ListEntry>,
}

impl TrustList {
    /// A list of `state` for the PC `pc_id` with key `pc_pk`, answering the request `req`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        kid: u32,
        state: State,
        binding: &str,
        pc_id: &str,
        pc_pk: &[u8; 32],
        req: &str,
        serial: i64,
        iat: i64,
        ttl: i64,
        refresh: i64,
        strict: bool,
    ) -> TrustList {
        TrustList {
            v: 1,
            typ: "hjv-trust",
            kid,
            binding: binding.to_owned(),
            pc_id: pc_id.to_owned(),
            pc_pk: b64(pc_pk),
            req: req.to_owned(),
            serial,
            iat,
            exp: iat.saturating_add(ttl.clamp(1, MAX_TTL)),
            refresh,
            strict,
            state: state.as_str(),
            owner: String::new(),
            login_at: 0,
            keys: Vec::new(),
        }
    }

    /// The bytes that are signed and sent.
    pub fn bytes(&self) -> serde_json::Result<Vec<u8>> {
        serde_json::to_vec(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_masked() {
        assert_eq!(mask_name("홍길동"), "홍*동");
        assert_eq!(mask_name("남궁길동"), "남**동");
        assert_eq!(mask_name("홍길"), "홍*");
        assert_eq!(mask_name("홍"), "*");
        assert_eq!(mask_name("  "), "");
        assert_eq!(mask_name("Hong Gildong"), "H**********g");
    }

    #[test]
    fn public_keys() {
        let k1 = crate::devsig::public_key(&[1; 32]);
        let k2 = crate::devsig::public_key(&[9; 32]);
        let text = format!("{}, {}", entry(1, &k1), entry(2, &k2));
        assert_eq!(parse_pubkeys(&text).unwrap(), vec![(1, k1), (2, k2)]);
        assert_eq!(parse_pubkeys(" ").unwrap(), vec![]);
        for bad in [
            format!("0:{}", b64(&k1)),
            format!("01:{}", b64(&k1)),
            format!("65536:{}", b64(&k1)),
            format!("1:{}", &b64(&k1)[..40]),
            format!("{},{}", entry(1, &k1), entry(1, &k2)),
            format!("{},{}", entry(1, &k1), entry(2, &k1)),
            b64(&k1),
            format!("1:{},", b64(&k1)),
            // the identity point: of small order, anyone could sign with it
            format!(
                "1:{}",
                b64(&{
                    let mut p = [0u8; 32];
                    p[0] = 1;
                    p
                })
            ),
        ] {
            assert!(parse_pubkeys(&bad).is_err(), "{bad}");
        }
        assert_eq!(fingerprint(1, &k1), format!("1:{}", hex(&sha256(&k1))));
        let key = ListKey::new(1, &[1; 32]);
        assert_eq!(key.public_key(), k1);
        assert_eq!(format!("{key:?}"), format!("ListKey({})", entry(1, &k1)));
    }

    /// design-trust-list/trust-check's golden value: seed [1; 32] over "hjv-trust-v1" NUL "golden"
    #[test]
    fn the_golden_signature() {
        assert_eq!(
            b64(&ListKey::new(1, &[1; 32]).sign(b"golden")),
            "HVaqNR32XoAL6O7jrfbYV2eI54M1J3Ou2/Vhka373a0x1AIGEjFHWE6u59vl6YVCO69a+Bi5rA0BNt4WktwsBQ=="
        );
    }
}

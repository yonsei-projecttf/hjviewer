// SPDX-License-Identifier: AGPL-3.0-only

//! The sealed login password (design v2 §8-2, §9-2 step 3).
//!
//! The app does not send the portal password in the clear. Client patch 0008 replaces the
//! `password` field of `POST /api/login` with
//!
//! ```text
//! hjv-sealed-v1.<kid>.<sealed>
//! ```
//!
//! - `<sealed>`: URL-safe base64 without padding of `crypto_box_seal(payload, server public key)`,
//!   libsodium's sealed box (sodiumoxide's `sealedbox::seal` in the app; opened here with
//!   `crypto_box::SecretKey::unseal`, which the interop test below checks against libsodium).
//! - `<kid>`: [`key_id`] of the server key it was sealed to (16 lowercase hex digits).
//! - payload: the UTF-8 JSON object
//!   `{"v":1,"pw":"<password>","t":<unix seconds>,"n":"<nonce>","id":"<device id>","kid":"<kid>"}`
//!   where `n` is standard base64 of 16 to 64 random bytes, new for every login, and `id` is the
//!   device ID that the request also carries in its own `id` field.
//!
//! hjv-api answers 400 to a password that is not sealed, an unknown kid, a payload it cannot open
//! or read, a kid or device ID that differs from the outer one, a time more than
//! HJV_API_SEAL_MAX_SKEW (10 minutes) off its clock, and a nonce it has seen before, except that
//! the very same request body within HJV_API_RETRY_WINDOW (30 s) gets the first answer again
//! (login.rs). The 2FA follow-up request carries no password and is not sealed.
//!
//! The app builds get the public key as HJV_LOGIN_SEAL_PUBKEY: standard base64 of the 32-byte
//! X25519 key that `hjv-api keygen seal` prints; the app derives `<kid>` from it the same way.

use crate::util::{b64_decode, b64url_decode, hex, sha256};
use crypto_box::SecretKey;
use serde::Deserialize;
use zeroize::Zeroizing;

pub const FIELD_PREFIX: &str = "hjv-sealed-v1.";
/// Longest sealed field accepted (a password of up to 256 bytes fits several times over).
const MAX_FIELD_LEN: usize = 4096;
const MAX_PASSWORD_LEN: usize = 256;

/// The key ID of a seal public key: the first 8 bytes of its SHA-256, in hex.
pub fn key_id(public_key: &[u8; 32]) -> String {
    hex(&sha256(public_key)[..8])
}

/// The keys that open sealed passwords.
pub struct SealKeys {
    keys: Vec<(String, SecretKey)>,
}

/// An opened sealed password.
pub struct Opened {
    pub password: Zeroizing<String>,
    pub time: i64,
    /// the nonce exactly as the payload has it (the device signature covers this string)
    pub nonce: String,
    pub device_id: String,
    /// SHA-256 of the sealed bytes (the device signature covers it)
    pub sealed_hash: [u8; 32],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealError {
    /// no sealed value at all: a plain password, or none
    NotSealed,
    /// sealed to a key this server does not have
    UnknownKey,
    /// the box does not open or the payload does not follow the format
    Corrupt,
}

impl SealError {
    pub fn as_str(self) -> &'static str {
        match self {
            SealError::NotSealed => "not sealed",
            SealError::UnknownKey => "unknown key",
            SealError::Corrupt => "corrupt",
        }
    }
}

#[derive(Deserialize)]
struct Payload {
    v: u32,
    pw: String,
    t: i64,
    n: String,
    id: String,
    kid: String,
}

impl SealKeys {
    pub fn new(secret_keys: &[Zeroizing<[u8; 32]>]) -> SealKeys {
        let keys = secret_keys
            .iter()
            .map(|sk| {
                let sk = SecretKey::from_bytes(**sk);
                (key_id(sk.public_key().as_bytes()), sk)
            })
            .collect();
        SealKeys { keys }
    }

    /// The key IDs, current key first.
    pub fn key_ids(&self) -> Vec<String> {
        self.keys.iter().map(|(kid, _)| kid.clone()).collect()
    }

    /// Opens the `password` field of a login request. The checks against the request and the
    /// clock are the caller's (login.rs).
    pub fn open(&self, field: &str) -> Result<Opened, SealError> {
        let rest = field
            .strip_prefix(FIELD_PREFIX)
            .ok_or(SealError::NotSealed)?;
        if field.len() > MAX_FIELD_LEN {
            return Err(SealError::Corrupt);
        }
        let (kid, sealed) = rest.split_once('.').ok_or(SealError::Corrupt)?;
        let (_, key) = self
            .keys
            .iter()
            .find(|(k, _)| k == kid)
            .ok_or(SealError::UnknownKey)?;
        let sealed = b64url_decode(sealed).ok_or(SealError::Corrupt)?;
        let plain = Zeroizing::new(key.unseal(&sealed).map_err(|_| SealError::Corrupt)?);
        let payload: Payload = serde_json::from_slice(&plain).map_err(|_| SealError::Corrupt)?;
        let password = Zeroizing::new(payload.pw);
        let nonce_len = b64_decode(&payload.n).map_or(0, |n| n.len());
        if payload.v != 1
            || payload.kid != kid
            || password.is_empty()
            || password.len() > MAX_PASSWORD_LEN
            || !(16..=64).contains(&nonce_len)
            || payload.id.len() > 64
        {
            return Err(SealError::Corrupt);
        }
        Ok(Opened {
            password,
            time: payload.t,
            nonce: payload.n,
            device_id: payload.id,
            sealed_hash: sha256(&sealed),
        })
    }
}

/// What the app does, for tests and tools: seals `password` for a login from `device_id` at
/// `time` with `nonce` (standard base64).
pub fn seal_password(
    public_key: &[u8; 32],
    password: &str,
    time: i64,
    nonce: &str,
    device_id: &str,
) -> String {
    let kid = key_id(public_key);
    let payload = serde_json::json!({
        "v": 1, "pw": password, "t": time, "n": nonce, "id": device_id, "kid": kid,
    });
    seal_payload(public_key, &kid, &payload.to_string())
}

fn seal_payload(public_key: &[u8; 32], kid: &str, payload: &str) -> String {
    use crate::util::b64url;
    use crypto_box::aead::OsRng;
    let sealed = crypto_box::PublicKey::from(*public_key)
        .seal(&mut OsRng, payload.as_bytes())
        .expect("sealing cannot fail");
    format!("{FIELD_PREFIX}{kid}.{}", b64url(&sealed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::b64;

    // Made with libsodium 1.0.18 (crypto_box_seed_keypair, crypto_box_seal), the library behind
    // the app's sodiumoxide: the sealed payload must open here.
    const VECTOR_SK: &str = "d3iPGgzqABomMdrl0F29BiAI1bMPULnim+sqeCIokAQ=";
    const VECTOR_PK: &str = "SjgH0GTQdxgcwHCYnnaJHSDcpVWVSNwsd8GlAnOIKzg=";
    const VECTOR_KID: &str = "d9d90fbd70563cc4";
    const VECTOR_FIELD: &str = "hjv-sealed-v1.d9d90fbd70563cc4.fFFoEMjN6816rB0xp_9pEbKGTeY4-Nq_JAwDKVJZI2IIgU3i5bQZI8dHWCkgn6RbIfj3YFxaU0v_gFur5xg54TiQI0OkHOrs0HgSw1LYqXKGSquBeuGTFGH7uqD-r57iEyFhU5IWLlt3TiAoB3FzyhetnRAic83BmEX9JWkLtPnGFg3H4ZXVHWEnGVQHvZ0hoSMs0ZJkp1dI5dSDky5B_9f2i8CS3KNVCA7X_qHR1iUOPwA";
    const VECTOR_SEALED_HASH: &str = "qIDSw48Gkb5KSslffq/QEWRaClx5ihVLTE5+jqOHQ6Q=";

    fn vector_keys() -> SealKeys {
        let sk: [u8; 32] = b64_decode(VECTOR_SK).unwrap().try_into().unwrap();
        SealKeys::new(&[Zeroizing::new(sk)])
    }

    #[test]
    fn opens_what_libsodium_sealed() {
        let keys = vector_keys();
        let pk: [u8; 32] = b64_decode(VECTOR_PK).unwrap().try_into().unwrap();
        assert_eq!(key_id(&pk), VECTOR_KID);
        assert_eq!(keys.key_ids(), vec![VECTOR_KID.to_owned()]);
        let opened = keys.open(VECTOR_FIELD).unwrap();
        assert_eq!(opened.password.as_str(), "비밀번호 test \"quoted\"");
        assert_eq!(opened.time, 1_760_000_000);
        assert_eq!(opened.nonce, "AAECAwQFBgcICQoLDA0ODw==");
        assert_eq!(opened.device_id, "123456789");
        assert_eq!(b64(&opened.sealed_hash), VECTOR_SEALED_HASH);
    }

    #[test]
    fn round_trip_and_refusals() {
        let sk = [9u8; 32];
        let keys = SealKeys::new(&[Zeroizing::new(sk)]);
        let pk = *SecretKey::from_bytes(sk).public_key().as_bytes();
        let nonce = b64(&[1u8; 16]);
        let field = seal_password(&pk, "pw", 5, &nonce, "42");
        let opened = keys.open(&field).unwrap();
        assert_eq!(opened.password.as_str(), "pw");
        assert_eq!(opened.device_id, "42");

        assert_eq!(
            keys.open("plain password").err(),
            Some(SealError::NotSealed)
        );
        assert_eq!(keys.open("").err(), Some(SealError::NotSealed));
        assert_eq!(
            vector_keys().open(&field).err(),
            Some(SealError::UnknownKey)
        );
        let mut broken = field.clone();
        broken.push('A');
        assert_eq!(keys.open(&broken).err(), Some(SealError::Corrupt));
        assert_eq!(
            keys.open(&format!("{FIELD_PREFIX}{}", key_id(&pk))).err(),
            Some(SealError::Corrupt)
        );
        // a short nonce, an empty password
        for (pw, n) in [("pw", b64(&[1u8; 8])), ("", nonce.clone())] {
            let f = seal_password(&pk, pw, 5, &n, "42");
            assert_eq!(keys.open(&f).err(), Some(SealError::Corrupt));
        }
        let kid = key_id(&pk);
        let payload = |v: u32, kid_inside: &str| {
            serde_json::json!({"v": v, "pw": "pw", "t": 5, "n": nonce, "id": "42", "kid": kid_inside})
                .to_string()
        };
        // the inner kid must match the outer one, the version must be 1, and it must be JSON
        let wrong_kid = seal_payload(&pk, &kid, &payload(1, &key_id(&[0u8; 32])));
        assert_eq!(keys.open(&wrong_kid).err(), Some(SealError::Corrupt));
        let wrong_version = seal_payload(&pk, &kid, &payload(2, &kid));
        assert_eq!(keys.open(&wrong_version).err(), Some(SealError::Corrupt));
        let not_json = seal_payload(&pk, &kid, "password");
        assert_eq!(keys.open(&not_json).err(), Some(SealError::Corrupt));
        assert!(keys
            .open(&seal_payload(&pk, &kid, &payload(1, &kid)))
            .is_ok());
    }
}

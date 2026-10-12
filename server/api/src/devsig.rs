// SPDX-License-Identifier: AGPL-3.0-only

//! The device signature that ties a login to the device it comes from (design v2 §5).
//!
//! With the sealed password (seal.rs), client patch 0008 adds to the login body
//!
//! ```text
//! "hjv": {"v": 1, "sig": "<standard base64 of a 64-byte Ed25519 signature>", "installed": true}
//! ```
//!
//! signed with the device's key pair (`Config::get_existing_key_pair`, the key the device
//! registered with hbbs) over the bytes
//!
//! ```text
//! "hjv-login-v1" 0 <id> 0 <t> 0 <n> 0 <h> 0 <installed> 0 <os>
//! ```
//!
//! where 0 is a zero byte, `id`, `t` and `n` are the sealed payload's device ID, time (decimal)
//! and nonce exactly as sealed, `h` is standard base64 of SHA-256 of the sealed bytes, `installed`
//! is "1" or "0" (whether the app runs installed, D9) and `os` is the `deviceInfo.os` string of the
//! same request. The layout follows the client's switch-grant signature (zero-separated fields
//! after a context string).
//!
//! hjv-api checks it against the public key that hbbs holds for the ID (hbbs.rs) before it adds
//! the device to an account, and against the key stored with an existing binding when it picks the
//! separate login limit of a device already in the account (login.rs). Signatures are checked
//! strictly (`verify_strict`, as libsodium's `crypto_sign_verify_detached`).

use crate::util::{b64, b64_decode};
use ed25519_dalek::{Signature, VerifyingKey};

pub const CONTEXT: &[u8] = b"hjv-login-v1";

/// The bytes a login signature covers.
pub fn login_message(
    device_id: &str,
    time: i64,
    nonce: &str,
    sealed_hash: &[u8; 32],
    installed: bool,
    os: &str,
) -> Vec<u8> {
    let time = time.to_string();
    let hash = b64(sealed_hash);
    let fields: [&[u8]; 7] = [
        CONTEXT,
        device_id.as_bytes(),
        time.as_bytes(),
        nonce.as_bytes(),
        hash.as_bytes(),
        if installed { b"1" } else { b"0" },
        os.as_bytes(),
    ];
    fields.join(&0u8)
}

/// Whether `signature` (standard base64) is a valid signature of `message` by `public_key`.
pub fn verify(public_key: &[u8], message: &[u8], signature: &str) -> bool {
    let Ok(public_key) = <[u8; 32]>::try_from(public_key) else {
        return false;
    };
    let Ok(key) = VerifyingKey::from_bytes(&public_key) else {
        return false;
    };
    let Some(sig) = b64_decode(signature).and_then(|s| <[u8; 64]>::try_from(s).ok()) else {
        return false;
    };
    key.verify_strict(message, &Signature::from_bytes(&sig))
        .is_ok()
}

/// What the app does, for tests and tools: signs `message` with a 32-byte Ed25519 seed.
pub fn sign(seed: &[u8; 32], message: &[u8]) -> String {
    use ed25519_dalek::Signer;
    b64(&ed25519_dalek::SigningKey::from_bytes(seed)
        .sign(message)
        .to_bytes())
}

/// The public key of a 32-byte Ed25519 seed.
pub fn public_key(seed: &[u8; 32]) -> [u8; 32] {
    ed25519_dalek::SigningKey::from_bytes(seed)
        .verifying_key()
        .to_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::hex;

    // Made with libsodium 1.0.18 (crypto_sign_seed_keypair, crypto_sign_detached), as the app
    // signs with sodiumoxide; the sealed hash is that of the seal.rs vector.
    const DEVICE_PK: &str = "C7w0aldmfDgBIL2cf9flHSxf3+o3zS9b9AWyxr9vLXg=";
    const SIGNATURE: &str =
        "7r+N5cRWOL/V0AeRCN6d8fb0tTkBYW9YD+QzrxXazHwtLgmwFRz4EEtbx76AULkh/exqsd5bQ8bb3yOx6pStBg==";
    const SEALED_HASH: &str = "qIDSw48Gkb5KSslffq/QEWRaClx5ihVLTE5+jqOHQ6Q=";
    const MESSAGE_HEX: &str = "686a762d6c6f67696e2d76310031323334353637383900313736303030303030300041414543417751464267634943516f4c4441304f44773d3d0071494453773438476b62354b53736c6666712f5145575261436c78356968564c5445352b6a714f485136513d00310077696e646f7773";

    fn vector_message() -> Vec<u8> {
        let hash: [u8; 32] = b64_decode(SEALED_HASH).unwrap().try_into().unwrap();
        login_message(
            "123456789",
            1_760_000_000,
            "AAECAwQFBgcICQoLDA0ODw==",
            &hash,
            true,
            "windows",
        )
    }

    #[test]
    fn layout_and_libsodium_signature() {
        let message = vector_message();
        assert_eq!(hex(&message), MESSAGE_HEX);
        let pk = b64_decode(DEVICE_PK).unwrap();
        assert!(verify(&pk, &message, SIGNATURE));
        let mut other = message.clone();
        other.push(b'x');
        assert!(!verify(&pk, &other, SIGNATURE));
    }

    #[test]
    fn refuses_what_is_not_a_signature() {
        let message = vector_message();
        let pk = b64_decode(DEVICE_PK).unwrap();
        assert!(!verify(&pk, &message, ""));
        assert!(!verify(&pk, &message, "not base64"));
        assert!(!verify(&pk[..31], &message, SIGNATURE));
        assert!(!verify(&[0u8; 32], &message, SIGNATURE));
        let seed = [5u8; 32];
        let sig = sign(&seed, &message);
        assert!(verify(&public_key(&seed), &message, &sig));
        assert!(!verify(&pk, &message, &sig));
    }
}

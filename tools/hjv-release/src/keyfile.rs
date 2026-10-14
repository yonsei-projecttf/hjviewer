// SPDX-License-Identifier: AGPL-3.0-only

//! The private half of an update key, and the file that keeps it (design 6-3, decision D-K).
//!
//! A key file is an age file encrypted with a passphrase (age.rs). Inside is this text:
//!
//! ```text
//! hjv-update-key-v1
//! role=signing            (or root)
//! id=1
//! seed=<base64 of the 32-byte Ed25519 seed>
//! public=<base64 of the public key>
//! ```
//!
//! Nothing here prints, logs or formats a seed or a passphrase: the types have no `Display`,
//! their `Debug` shows the public side only, and every error is a fixed sentence. Secrets are
//! wiped from memory when they are dropped.

use crate::{
    age,
    keys::{parse_key_id, PublicKey, Role},
};
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use std::fmt;
use zeroize::Zeroizing;

const HEADER: &str = "hjv-update-key-v1";
/// A passphrase shorter than this is refused when a key is made.
pub const MIN_PASSPHRASE_CHARS: usize = 12;

pub struct SecretKey {
    pub role: Role,
    pub id: u32,
    key: SigningKey,
}

impl fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SecretKey({} {})", self.role.name(), self.public())
    }
}

/// Why a key file did not open. No variant carries content of the file.
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// Not a key file: the reason is about the file's outer form only.
    NotAKeyFile(&'static str),
    WrongPassphrase,
    Damaged,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotAKeyFile(why) => write!(f, "키 파일이 아닙니다({why})"),
            Error::WrongPassphrase => f.write_str("비밀번호가 맞지 않습니다"),
            Error::Damaged => f.write_str("키 파일이 손상되었습니다"),
        }
    }
}

impl SecretKey {
    /// A new key from the system's randomness.
    pub fn generate(role: Role, id: u32) -> Result<SecretKey, String> {
        let mut seed = Zeroizing::new([0u8; 32]);
        getrandom::getrandom(seed.as_mut())
            .map_err(|_| "시스템에서 난수를 얻지 못했습니다".to_owned())?;
        Ok(SecretKey::from_seed(role, id, &seed))
    }

    pub fn from_seed(role: Role, id: u32, seed: &[u8; 32]) -> SecretKey {
        SecretKey {
            role,
            id,
            key: SigningKey::from_bytes(seed),
        }
    }

    pub fn verifying_key(&self) -> VerifyingKey {
        self.key.verifying_key()
    }

    /// The public key and its number, as a key list names it.
    pub fn public(&self) -> PublicKey {
        PublicKey {
            id: self.id,
            key: self.verifying_key(),
        }
    }

    pub fn signing_key(&self) -> &SigningKey {
        &self.key
    }

    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.key.sign(message).to_bytes()
    }

    fn plaintext(&self) -> Zeroizing<String> {
        let seed = Zeroizing::new(self.key.to_bytes());
        let seed_b64 = Zeroizing::new(STANDARD.encode(seed.as_ref()));
        // one buffer that never grows, so that no copy of the seed is left behind in freed memory
        let mut text = Zeroizing::new(String::with_capacity(256));
        for part in [
            HEADER,
            "\nrole=",
            self.role.name(),
            "\nid=",
            &self.id.to_string(),
            "\nseed=",
            seed_b64.as_str(),
            "\npublic=",
            &STANDARD.encode(self.verifying_key().to_bytes()),
            "\n",
        ] {
            text.push_str(part);
        }
        text
    }

    /// The key file's bytes, encrypted under `passphrase`.
    pub fn encrypt(&self, passphrase: &str, work_factor: u8) -> Result<Vec<u8>, String> {
        age::encrypt(
            self.plaintext().as_bytes(),
            passphrase.as_bytes(),
            work_factor,
        )
        .map_err(|_| "키를 암호화하지 못했습니다".to_owned())
    }

    /// Opens a key file.
    pub fn decrypt(file: &[u8], passphrase: &str) -> Result<SecretKey, Error> {
        let plaintext = age::decrypt(file, passphrase.as_bytes()).map_err(|e| match e {
            age::Error::Format(why) => Error::NotAKeyFile(why),
            age::Error::Passphrase => Error::WrongPassphrase,
            age::Error::Damaged => Error::Damaged,
        })?;
        let text = std::str::from_utf8(&plaintext)
            .map_err(|_| Error::NotAKeyFile("the content is not an update key"))?;
        let mut lines = text.split('\n');
        if lines.next() != Some(HEADER) {
            return Err(Error::NotAKeyFile("the content is not an update key"));
        }
        let mut field = |name: &str| -> Result<&str, Error> {
            lines
                .next()
                .and_then(|line| line.strip_prefix(name))
                .and_then(|rest| rest.strip_prefix('='))
                .ok_or(Error::Damaged)
        };
        let role = Role::parse(field("role")?).ok_or(Error::Damaged)?;
        let id = parse_key_id(field("id")?).map_err(|_| Error::Damaged)?;
        let seed = Zeroizing::new(
            STANDARD
                .decode(field("seed")?)
                .map_err(|_| Error::Damaged)?,
        );
        let seed: Zeroizing<[u8; 32]> =
            Zeroizing::new(<[u8; 32]>::try_from(seed.as_slice()).map_err(|_| Error::Damaged)?);
        let public = STANDARD
            .decode(field("public")?)
            .map_err(|_| Error::Damaged)?;
        if lines.next() != Some("") || lines.next().is_some() {
            return Err(Error::Damaged);
        }
        let key = SecretKey::from_seed(role, id, &seed);
        if key.verifying_key().to_bytes()[..] != public[..] {
            return Err(Error::Damaged);
        }
        Ok(key)
    }
}

/// Whether a new key may be protected with this passphrase.
pub fn check_new_passphrase(passphrase: &str) -> Result<(), String> {
    if passphrase.chars().count() < MIN_PASSPHRASE_CHARS {
        return Err(format!(
            "비밀번호는 {MIN_PASSPHRASE_CHARS}자 이상이어야 합니다"
        ));
    }
    if passphrase.chars().any(char::is_control) {
        return Err("비밀번호에 제어 문자를 쓸 수 없습니다".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::test_key;

    const FAST: u8 = 6;

    #[test]
    fn round_trip() {
        let seed = test_key("root-101").unwrap().seed();
        let key = SecretKey::from_seed(Role::Root, 101, &seed);
        let file = key.encrypt("correct horse battery", FAST).unwrap();
        let back = SecretKey::decrypt(&file, "correct horse battery").unwrap();
        assert_eq!((back.role, back.id), (Role::Root, 101));
        assert_eq!(back.public(), key.public());
        assert_eq!(back.sign(b"m"), key.sign(b"m"));
        assert_eq!(
            SecretKey::decrypt(&file, "correct horse batterx").unwrap_err(),
            Error::WrongPassphrase
        );
        assert!(matches!(
            SecretKey::decrypt(b"not a key file", "x").unwrap_err(),
            Error::NotAKeyFile(_)
        ));
        let mut damaged = file.clone();
        *damaged.last_mut().unwrap() ^= 1;
        assert_eq!(
            SecretKey::decrypt(&damaged, "correct horse battery").unwrap_err(),
            Error::Damaged
        );
    }

    #[test]
    fn nothing_shows_the_seed() {
        let seed = test_key("signing-1").unwrap().seed();
        let key = SecretKey::from_seed(Role::Signing, 1, &seed);
        let shown = format!("{key:?} {:?}", key.public());
        let seed_b64 = STANDARD.encode(seed);
        let seed_hex: String = seed.iter().map(|b| format!("{b:02x}")).collect();
        assert!(!shown.contains(&seed_b64) && !shown.contains(&seed_hex));
        assert!(shown.contains("JUO5L/EJVRFHatyDadtt3JM2ZaEZeN2hQE7hBmypVZ0="));
        // the file holds no part of the seed or of the text around it
        let file = key.encrypt("a passphrase of words", FAST).unwrap();
        for needle in [
            &seed[..],
            seed_b64.as_bytes(),
            seed_hex.as_bytes(),
            b"role=",
            b"seed=",
            HEADER.as_bytes(),
        ] {
            for window in needle.windows(needle.len().min(8)) {
                assert!(!file.windows(window.len()).any(|w| w == window));
            }
        }
        for error in [
            Error::NotAKeyFile("x"),
            Error::WrongPassphrase,
            Error::Damaged,
        ] {
            let text = format!("{error} {error:?}");
            assert!(!text.contains(&seed_b64) && !text.contains("a passphrase of words"));
        }
    }

    #[test]
    fn a_generated_key_differs_every_time() {
        let a = SecretKey::generate(Role::Signing, 1).unwrap();
        let b = SecretKey::generate(Role::Signing, 1).unwrap();
        assert_ne!(a.public(), b.public());
        assert!(!crate::keys::is_test_key(&a.verifying_key()));
    }

    #[test]
    fn content_rules() {
        let open = |text: &str| {
            let file = age::encrypt(text.as_bytes(), b"p", FAST).unwrap();
            SecretKey::decrypt(&file, "p").map(|k| (k.role, k.id))
        };
        let seed = STANDARD.encode(test_key("signing-2").unwrap().seed());
        let public = test_key("signing-2").unwrap().public().to_string();
        let public = public.strip_prefix("2:").unwrap();
        let good = format!("{HEADER}\nrole=signing\nid=2\nseed={seed}\npublic={public}\n");
        assert_eq!(open(&good).unwrap(), (Role::Signing, 2));
        assert!(matches!(
            open("something else\n"),
            Err(Error::NotAKeyFile(_))
        ));
        for bad in [
            good.replacen("role=signing", "role=admin", 1),
            good.replacen("id=2", "id=0", 1),
            good.replacen("id=2", "id=70000", 1),
            good.replacen("id=2\n", "", 1),
            good.replacen(&seed, &seed[..40], 1),
            good.replacen(public, &STANDARD.encode([1u8; 32]), 1),
            format!("{good}extra\n"),
            good.trim_end().to_owned(),
        ] {
            assert_eq!(
                open(&bad).unwrap_err(),
                Error::Damaged,
                "{}",
                bad.replace(&seed, "<seed>")
            );
        }
    }

    #[test]
    fn new_passphrases() {
        assert!(check_new_passphrase("correct horse").is_ok());
        assert!(check_new_passphrase("한글로 된 긴 비밀번호 문장").is_ok());
        assert!(check_new_passphrase("short").is_err());
        assert!(check_new_passphrase("").is_err());
        assert!(check_new_passphrase("with a\ttab inside").is_err());
    }
}

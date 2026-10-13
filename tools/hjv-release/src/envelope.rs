// SPDX-License-Identifier: AGPL-3.0-only

//! The envelope: the file the server hands out at `/api/hjv/version` (design 1-1).
//!
//! ```text
//! {"v":1,"k":1,"p":"<policy text>","s":"<signature>",
//!  "root":{"k":101,"p":"<root declaration text>","s":"<signature>"}}
//! ```
//!
//! `p` is JSON text carried as a JSON string. A signature covers the exact UTF-8 bytes of that
//! text after a domain string and a zero byte: `"hjv-version-v1" 0 p` for the policy and
//! `"hjv-version-root-v1" 0 root.p` for the root declaration. Readers check the signature first
//! and parse `p` afterwards; nobody serializes `p` again. The domain strings differ from every
//! other signature in HJViewer (`hjv-wake-v1`, `hjv-login-v1`, `hjv-req-v1`, `hjv-update-v1`), so
//! a signature made for one purpose never passes for another. `s` is standard base64 of the
//! 64-byte Ed25519 signature (88 characters); `root` is optional.

use crate::json::{Json, Writer};
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

pub const POLICY_DOMAIN: &[u8] = b"hjv-version-v1";
pub const ROOT_DOMAIN: &[u8] = b"hjv-version-root-v1";
pub const ENVELOPE_VERSION: u64 = 1;
/// Apps read at most this much of the response (design 3-2).
pub const MAX_ENVELOPE_BYTES: usize = 16 * 1024;

/// A signed text: the policy or the root declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Part {
    /// The number of the key that signed.
    pub k: u32,
    pub p: String,
    pub s: [u8; 64],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Envelope {
    pub policy: Part,
    pub root: Option<Part>,
}

/// The bytes a signature covers.
pub fn message(domain: &[u8], p: &str) -> Vec<u8> {
    let mut m = Vec::with_capacity(domain.len() + 1 + p.len());
    m.extend_from_slice(domain);
    m.push(0);
    m.extend_from_slice(p.as_bytes());
    m
}

impl Part {
    pub fn sign(key: &SigningKey, k: u32, domain: &[u8], p: &str) -> Part {
        Part {
            k,
            p: p.to_owned(),
            s: key.sign(&message(domain, p)).to_bytes(),
        }
    }

    /// Strict verification, as hjv-api's and as libsodium's `crypto_sign_verify_detached`, which
    /// the apps use.
    pub fn verifies(&self, key: &VerifyingKey, domain: &[u8]) -> bool {
        key.verify_strict(&message(domain, &self.p), &Signature::from_bytes(&self.s))
            .is_ok()
    }

    fn read(
        value: &Json,
        what: &str,
        with_version: bool,
    ) -> Result<(Option<u64>, Part, Option<Part>), String> {
        let mut f = value.fields(what)?;
        let v = if with_version {
            Some(f.int("v")?)
        } else {
            None
        };
        let k = f.int("k")?;
        let k = u32::try_from(k)
            .ok()
            .filter(|k| (1..=65535).contains(k))
            .ok_or_else(|| format!("{what}: 키 번호 k는 1-65535입니다"))?;
        let p = f.str("p")?.to_owned();
        let s = STANDARD
            .decode(f.str("s")?)
            .ok()
            .and_then(|s| <[u8; 64]>::try_from(s).ok())
            .ok_or_else(|| format!("{what}: 서명 s는 64바이트의 표준 base64여야 합니다"))?;
        let root = match with_version.then(|| f.take("root")).flatten() {
            Some(root) => Some(Part::read(root, "루트 선언", false)?.1),
            None => None,
        };
        f.done()?;
        Ok((v, Part { k, p, s }, root))
    }

    fn write(&self, w: &mut Writer) {
        w.int("k", u64::from(self.k))
            .str("p", &self.p)
            .str("s", &STANDARD.encode(self.s));
    }

    /// `{"k":..,"p":..,"s":..}`: how a root declaration looks inside an envelope and on its own.
    pub fn to_text(&self) -> String {
        let mut w = Writer::new();
        self.write(&mut w);
        w.finish()
    }

    /// Reads `{"k":..,"p":..,"s":..}`.
    pub fn parse(text: &str, what: &str) -> Result<Part, String> {
        let value = Json::parse(text).map_err(|e| format!("{what}: JSON이 아닙니다({e})"))?;
        Ok(Part::read(&value, what, false)?.1)
    }
}

impl Envelope {
    /// Reads an envelope. Nothing is verified here.
    pub fn parse(text: &str) -> Result<Envelope, String> {
        if text.len() > MAX_ENVELOPE_BYTES {
            return Err(format!(
                "봉투가 {}바이트입니다. 앱은 {MAX_ENVELOPE_BYTES}바이트까지만 읽습니다",
                text.len()
            ));
        }
        let (v, envelope) = Envelope::read_form(text)?;
        if v != ENVELOPE_VERSION {
            return Err(format!("봉투: 모르는 봉투 버전 {v}"));
        }
        Ok(envelope)
    }

    /// The form of an envelope (trust rules 2-2, A2): its members and their types, a key number
    /// 1-65535 and a signature of 64 bytes in standard base64 (the base64 crate's engine takes
    /// the canonical form only). The version is returned, not judged.
    pub fn read_form(text: &str) -> Result<(u64, Envelope), String> {
        let value = Json::parse(text).map_err(|e| format!("봉투가 JSON이 아닙니다({e})"))?;
        let (v, policy, root) = Part::read(&value, "봉투", true)?;
        Ok((v.unwrap_or_default(), Envelope { policy, root }))
    }

    /// The envelope's text: one line without spaces, fields in the order v, k, p, s, root.
    pub fn to_text(&self) -> String {
        self.to_text_v(ENVELOPE_VERSION)
    }

    /// The same with another version number (the form of an envelope the tool does not write).
    pub fn to_text_v(&self, v: u64) -> String {
        let mut w = Writer::new();
        w.int("v", v);
        self.policy.write(&mut w);
        if let Some(root) = &self.root {
            w.raw("root", &root.to_text());
        }
        w.finish()
    }

    /// The file: the text and a line feed.
    pub fn to_file(&self) -> String {
        let mut text = self.to_text();
        text.push('\n');
        text
    }

    /// Whether `file` is exactly what [`Envelope::to_file`] writes (the line feed may be missing).
    pub fn is_canonical(&self, file: &str) -> bool {
        let text = self.to_text();
        file == text || file.strip_suffix('\n') == Some(text.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::test_key;

    fn key(name: &str) -> SigningKey {
        SigningKey::from_bytes(&test_key(name).unwrap().seed())
    }

    #[test]
    fn signs_and_verifies_by_domain() {
        let k1 = key("signing-1");
        let part = Part::sign(&k1, 1, POLICY_DOMAIN, "{\"v\":1}");
        assert!(part.verifies(&k1.verifying_key(), POLICY_DOMAIN));
        assert!(!part.verifies(&k1.verifying_key(), ROOT_DOMAIN));
        assert!(!part.verifies(&k1.verifying_key(), b"hjv-wake-v1"));
        assert!(!part.verifies(&key("signing-2").verifying_key(), POLICY_DOMAIN));
        let mut changed = part.clone();
        changed.p.push(' ');
        assert!(!changed.verifies(&k1.verifying_key(), POLICY_DOMAIN));
        assert_eq!(message(b"d", "p"), b"d\0p");
        // a domain is not a prefix trick: "hjv-version-v1" 0 p never equals "hjv-version-root-v1" 0 q
        assert_ne!(
            message(POLICY_DOMAIN, "x")[..POLICY_DOMAIN.len() + 1],
            message(ROOT_DOMAIN, "x")[..POLICY_DOMAIN.len() + 1]
        );
    }

    #[test]
    fn text_round_trip() {
        let policy = Part::sign(
            &key("signing-1"),
            1,
            POLICY_DOMAIN,
            "{\"v\":1,\"t\":\"한글\"}",
        );
        let root = Part::sign(&key("root-101"), 101, ROOT_DOMAIN, "{\"v\":1}");
        let plain = Envelope {
            policy: policy.clone(),
            root: None,
        };
        let text = plain.to_text();
        assert!(text.starts_with(
            "{\"v\":1,\"k\":1,\"p\":\"{\\\"v\\\":1,\\\"t\\\":\\\"한글\\\"}\",\"s\":\""
        ));
        assert!(!text.contains("root"));
        assert_eq!(Envelope::parse(&text).unwrap(), plain);
        assert_eq!(Envelope::parse(&plain.to_file()).unwrap(), plain);
        assert!(plain.is_canonical(&text) && plain.is_canonical(&plain.to_file()));
        assert!(!plain.is_canonical(&format!(" {text}")));
        assert!(!plain.is_canonical(&format!("{text}\n\n")));

        let with_root = Envelope {
            policy,
            root: Some(root.clone()),
        };
        let text = with_root.to_text();
        assert!(text.contains(",\"root\":{\"k\":101,\"p\":\"{\\\"v\\\":1}\",\"s\":\""));
        assert_eq!(Envelope::parse(&text).unwrap(), with_root);
        assert_eq!(Part::parse(&root.to_text(), "t").unwrap(), root);
    }

    #[test]
    fn refuses_other_shapes() {
        let part = Part::sign(&key("signing-1"), 1, POLICY_DOMAIN, "{}");
        let s = STANDARD.encode(part.s);
        let ok = format!(r#"{{"v":1,"k":1,"p":"{{}}","s":"{s}"}}"#);
        assert!(Envelope::parse(&ok).is_ok());
        for bad in [
            format!(r#"{{"v":2,"k":1,"p":"{{}}","s":"{s}"}}"#),
            format!(r#"{{"k":1,"p":"{{}}","s":"{s}"}}"#),
            format!(r#"{{"v":1,"k":0,"p":"{{}}","s":"{s}"}}"#),
            format!(r#"{{"v":1,"k":65536,"p":"{{}}","s":"{s}"}}"#),
            format!(r#"{{"v":1,"k":"1","p":"{{}}","s":"{s}"}}"#),
            format!(r#"{{"v":1,"k":1,"p":{{}},"s":"{s}"}}"#),
            format!(r#"{{"v":1,"k":1,"p":"{{}}","s":"{}"}}"#, &s[..s.len() - 4]),
            format!(
                r#"{{"v":1,"k":1,"p":"{{}}","s":"{}"}}"#,
                s.trim_end_matches('=')
            ),
            format!(r#"{{"v":1,"k":1,"p":"{{}}","s":"{s}","x":1}}"#),
            format!(r#"{{"v":1,"k":1,"k":1,"p":"{{}}","s":"{s}"}}"#),
            format!(r#"{{"v":1,"k":1,"p":"{{}}","s":"{s}","root":1}}"#),
            format!(
                r#"{{"v":1,"k":1,"p":"{{}}","s":"{s}","root":{{"v":1,"k":101,"p":"{{}}","s":"{s}"}}}}"#
            ),
            format!(
                r#"{{"v":1,"k":1,"p":"{{}}","s":"{s}","root":{{"k":101,"p":"{{}}","s":"{s}","root":{{}}}}}}"#
            ),
            format!(r#"[{ok}]"#),
            String::new(),
        ] {
            assert!(Envelope::parse(&bad).is_err(), "{bad} parsed");
        }
        let long = format!(
            r#"{{"v":1,"k":1,"p":"{}","s":"{s}"}}"#,
            "x".repeat(MAX_ENVELOPE_BYTES)
        );
        assert!(Envelope::parse(&long).unwrap_err().contains("바이트"));
    }

    #[test]
    fn the_size_limit_is_the_apps() {
        // an app reads 16384 bytes of the file and no more: a file of that size is read, one
        // byte more is not (the file's line feed counts)
        let with_p = |len: usize| Envelope {
            policy: Part {
                k: 1,
                p: "x".repeat(len),
                s: [0; 64],
            },
            root: None,
        };
        let empty = with_p(0).to_file().len();
        let at_the_limit = with_p(MAX_ENVELOPE_BYTES - empty).to_file();
        assert_eq!(at_the_limit.len(), 16 * 1024);
        assert!(Envelope::parse(&at_the_limit).is_ok());
        let one_over = with_p(MAX_ENVELOPE_BYTES - empty + 1).to_file();
        assert_eq!(one_over.len(), 16 * 1024 + 1);
        assert!(Envelope::parse(&one_over).unwrap_err().contains("바이트"));
    }

    /// A signature of `key` over `message` whose R is the identity point: with s = k * a (k the
    /// hash of R, A and the message, a the key's secret scalar) the plain equation
    /// [s]B = R + [k]A holds. No honest signer makes one; libsodium (the apps) refuses it.
    pub(crate) fn signature_with_identity_r(seed: &[u8; 32], message: &[u8]) -> [u8; 64] {
        use curve25519_dalek::{edwards::EdwardsPoint, scalar::Scalar, traits::Identity};
        use sha2::{Digest, Sha512};
        let public = SigningKey::from_bytes(seed).verifying_key();
        // the secret scalar (RFC 8032 5.1.5)
        let mut a = [0u8; 32];
        a.copy_from_slice(&Sha512::digest(seed)[..32]);
        a[0] &= 248;
        a[31] &= 127;
        a[31] |= 64;
        let a = Scalar::from_bytes_mod_order(a);
        let r = EdwardsPoint::identity().compress().to_bytes();
        let k = Scalar::from_hash(
            Sha512::new()
                .chain_update(r)
                .chain_update(public.to_bytes())
                .chain_update(message),
        );
        let mut signature = [0u8; 64];
        signature[..32].copy_from_slice(&r);
        signature[32..].copy_from_slice((k * a).as_bytes());
        signature
    }

    #[test]
    fn verification_is_strict() {
        use ed25519_dalek::Verifier;
        let seed = test_key("signing-1").unwrap().seed();
        let public = key("signing-1").verifying_key();
        let p = "{\"v\":1}";
        let s = signature_with_identity_r(&seed, &message(POLICY_DOMAIN, p));
        // the equation holds: a check that looks at nothing else takes the signature
        assert!(public
            .verify(&message(POLICY_DOMAIN, p), &Signature::from_bytes(&s))
            .is_ok());
        // this tool refuses it, as the apps do: what the tool verifies, an app must take
        let part = Part {
            k: 1,
            p: p.to_owned(),
            s,
        };
        assert!(!part.verifies(&public, POLICY_DOMAIN));
        // an honest signature of the same text passes, of course
        assert!(Part::sign(&key("signing-1"), 1, POLICY_DOMAIN, p).verifies(&public, POLICY_DOMAIN));
    }
}

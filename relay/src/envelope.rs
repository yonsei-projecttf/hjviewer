// SPDX-License-Identifier: AGPL-3.0-only

//! Signed request envelopes (PROTOCOL.md, "Request envelope"). Every request body is
//! `{"id","t","n","p","s"}` (and `"pk"` on hello): `p` is the payload as a JSON string, and `s` is
//! the relay key's Ed25519 signature over
//! `hjv-relay-v1 \0 <op> \0 <id> \0 <t> \0 <n> \0 base64(SHA-256(p))`.

use crate::identity::Identity;
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io;

pub const DOMAIN: &str = "hjv-relay-v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Hello,
    Poll,
}

impl Op {
    pub fn name(self) -> &'static str {
        match self {
            Op::Hello => "hello",
            Op::Poll => "poll",
        }
    }

    pub fn path(self) -> &'static str {
        match self {
            Op::Hello => "/api/hjv/relay/hello",
            Op::Poll => "/api/hjv/relay/poll",
        }
    }
}

/// The bytes the relay signs for one request.
pub fn signed_message(op: Op, id: &str, t: i64, n: &str, p: &str) -> Vec<u8> {
    let digest = STANDARD.encode(Sha256::digest(p.as_bytes()));
    let t = t.to_string();
    let parts = [DOMAIN, op.name(), id, &t, n, &digest];
    let mut msg = Vec::with_capacity(parts.iter().map(|p| p.len() + 1).sum());
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            msg.push(0);
        }
        msg.extend_from_slice(part.as_bytes());
    }
    msg
}

/// 16 random bytes in unpadded base64url (22 characters).
pub fn nonce() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

#[derive(Serialize)]
struct Envelope<'a> {
    id: &'a str,
    t: i64,
    n: &'a str,
    p: &'a str,
    s: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pk: Option<String>,
}

/// The request body for `payload` (a JSON object, already serialized), signed at time `t`.
pub fn seal(identity: &Identity, op: Op, t: i64, payload: &str) -> io::Result<String> {
    seal_with_nonce(identity, op, t, &nonce()?, payload)
}

pub fn seal_with_nonce(
    identity: &Identity,
    op: Op,
    t: i64,
    n: &str,
    payload: &str,
) -> io::Result<String> {
    let signature = identity.sign(&signed_message(op, &identity.id, t, n, payload));
    let envelope = Envelope {
        id: &identity.id,
        t,
        n,
        p: payload,
        s: STANDARD.encode(signature),
        pk: (op == Op::Hello).then(|| identity.pk_base64()),
    };
    serde_json::to_string(&envelope).map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signature, VerifyingKey};

    #[test]
    fn signed_message_layout() {
        let msg = signed_message(Op::Poll, "r-abc", 1760000000, "nnnn", "{}");
        // base64(SHA-256("{}"))
        let expected = b"hjv-relay-v1\0poll\0r-abc\x001760000000\0nnnn\0RBNvo1WzZ4oRRq0W9+hknpT7T8If536DEMBg9hyq/4o=";
        assert_eq!(msg, expected.to_vec());
    }

    #[test]
    fn envelopes_verify_and_carry_the_key_only_on_hello() {
        let identity = Identity::from_seed(&[3; 32]);
        for op in [Op::Hello, Op::Poll] {
            let body = seal(&identity, op, 1760000000, r#"{"v":1}"#).unwrap();
            let v: serde_json::Value = serde_json::from_str(&body).unwrap();
            assert_eq!(v["id"], identity.id.as_str());
            assert_eq!(v["t"], 1760000000);
            assert_eq!(v["p"], r#"{"v":1}"#);
            let n = v["n"].as_str().unwrap();
            assert_eq!(n.len(), 22);
            assert_eq!(URL_SAFE_NO_PAD.decode(n).unwrap().len(), 16);
            assert_eq!(v.get("pk").is_some(), op == Op::Hello);
            let sig: [u8; 64] = STANDARD
                .decode(v["s"].as_str().unwrap())
                .unwrap()
                .try_into()
                .unwrap();
            let key = VerifyingKey::from_bytes(&identity.pk).unwrap();
            let msg = signed_message(op, &identity.id, 1760000000, n, r#"{"v":1}"#);
            key.verify_strict(&msg, &Signature::from_bytes(&sig))
                .unwrap();
            // the other operation's message does not verify
            let other = if op == Op::Hello { Op::Poll } else { Op::Hello };
            let msg = signed_message(other, &identity.id, 1760000000, n, r#"{"v":1}"#);
            assert!(key
                .verify_strict(&msg, &Signature::from_bytes(&sig))
                .is_err());
        }
        assert_ne!(nonce().unwrap(), nonce().unwrap());
    }
}

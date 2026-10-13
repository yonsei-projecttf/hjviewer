// SPDX-License-Identifier: AGPL-3.0-only

//! The signed request envelope of the machines that talk to hjv-api without a user's token: the
//! PCs' sync (design v3 §3-6, pcsync.rs) and hjv-relay (relay/PROTOCOL.md §4, design v3 §8-3).
//! One body format, one signature layout and one order of checks for both; what differs is a
//! [`Spec`]: the signature's context string and operation, the largest body, and which IDs exist
//! (pcsync.rs `SPEC`; relay.rs `HELLO` and `POLL`).
//!
//! The body is one JSON object:
//!
//! ```text
//! {"id":"<ID>","t":<time>,"n":"<nonce>","p":"<payload text>","s":"<signature>"(,"pk":"<key>")}
//! ```
//!
//! - `id`: the PC's device ID, or the relay ID;
//! - `t`: the sender's idea of the server's time, a JSON integer greater than 0;
//! - `n`: base64url (no padding, 22 characters) of 16 random bytes, new for every request;
//! - `p`: the payload, a JSON object serialized to text and carried as a JSON string;
//! - `s`: standard base64 (88 characters) of the Ed25519 signature over the bytes of [`message`]:
//!
//! ```text
//! <context> NUL <op> NUL <id> NUL <t> NUL <n> NUL base64(SHA-256(p))
//! ```
//!
//!   with `t` in decimal and `p` the UTF-8 bytes of the payload text as received (never
//!   re-serialized), its digest in standard base64 (44 characters);
//! - `pk`: hjv-relay's hello only: standard base64 of the relay's 32-byte public key.
//!
//! Unknown fields are ignored. The server's checks, in this order, answer with the first failure
//! ([`Refusal`]): 1 size, 2 shape ([`Envelope::parse`]), 3 the key of `id` (the caller's: the
//! PC's bound key, or the relay's), 4 the signature ([`Envelope::verify`], strict), 5 the time,
//! `|t − server time| ≤ 300 s` ([`Envelope::fresh`]), 6 the pair (`id`, `n`) not accepted in the
//! last 600 seconds (the caller's store), 7 the payload (the caller's). A request refused in
//! steps 1-6 changes nothing; error answers are `{"error":"<code>","server_time":<time>}`
//! ([`refusal_reply`]).

use crate::{
    api::Reply,
    util::{b64, b64_decode, b64url, b64url_decode, now, sha256},
};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use hyper::StatusCode;
use serde_json::{json, Value};

/// How far `t` may be from the server's clock, either way (PROTOCOL.md §4.3 step 5).
pub const MAX_SKEW: i64 = 300;
/// How long an accepted (`id`, `n`) is refused again (step 6).
pub const NONCE_TTL: i64 = 600;

/// What one kind of envelope is.
#[derive(Debug, Clone, Copy)]
pub struct Spec {
    /// the signature's context string: "hjv-pc-v1" or "hjv-relay-v1"
    pub context: &'static str,
    /// the operation the signature names: "sync", "hello" or "poll"
    pub op: &'static str,
    /// the largest body taken (step 1)
    pub max_body: usize,
    /// whether a string can be an `id` of this kind (step 2)
    pub id_ok: fn(&str) -> bool,
    /// whether the envelope carries `pk` (hjv-relay's hello)
    pub with_pk: bool,
}

/// Why an envelope was refused, with its status and code (PROTOCOL.md §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// step 1: 413 `too_large`
    TooLarge,
    /// steps 2 and 7: 400 `bad_request`
    BadRequest,
    /// step 4: 401 `bad_signature`
    BadSignature,
    /// step 5: 400 `clock`, with `server_time`
    Clock,
    /// step 6: 409 `replay`
    Replay,
}

impl Refusal {
    pub fn status(self) -> StatusCode {
        match self {
            Refusal::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Refusal::BadRequest | Refusal::Clock => StatusCode::BAD_REQUEST,
            Refusal::BadSignature => StatusCode::UNAUTHORIZED,
            Refusal::Replay => StatusCode::CONFLICT,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Refusal::TooLarge => "too_large",
            Refusal::BadRequest => "bad_request",
            Refusal::BadSignature => "bad_signature",
            Refusal::Clock => "clock",
            Refusal::Replay => "replay",
        }
    }
}

/// An error answer of the envelope protocols: `{"error":"<code>","server_time":<now>}`.
pub fn error_reply(status: StatusCode, code: &str) -> Reply {
    Reply::json(status, &json!({ "error": code, "server_time": now() }))
}

pub fn refusal_reply(refusal: Refusal) -> Reply {
    error_reply(refusal.status(), refusal.code())
}

/// An envelope whose shape passed step 2.
#[derive(Debug, Clone)]
pub struct Envelope {
    pub id: String,
    pub t: i64,
    /// `n` as received, and its 16 bytes
    pub n: String,
    pub nonce: [u8; 16],
    /// the payload text as received (not yet parsed)
    pub p: String,
    pub signature: [u8; 64],
    /// `pk`, for a spec `with_pk`
    pub pk: Option<[u8; 32]>,
}

/// The bytes `s` signs (PROTOCOL.md §4.1).
pub fn message(spec: &Spec, id: &str, t: i64, n: &str, p: &str) -> Vec<u8> {
    let t = t.to_string();
    let digest = b64(&sha256(p.as_bytes()));
    let fields: [&[u8]; 6] = [
        spec.context.as_bytes(),
        spec.op.as_bytes(),
        id.as_bytes(),
        t.as_bytes(),
        n.as_bytes(),
        digest.as_bytes(),
    ];
    fields.join(&0u8)
}

impl Envelope {
    /// Steps 1 and 2: at most `max_body` bytes, a JSON object with the fields in their forms.
    pub fn parse(body: &[u8], spec: &Spec) -> Result<Envelope, Refusal> {
        if body.len() > spec.max_body {
            return Err(Refusal::TooLarge);
        }
        let value: Value = serde_json::from_slice(body).map_err(|_| Refusal::BadRequest)?;
        let object = value.as_object().ok_or(Refusal::BadRequest)?;
        let text = |name: &str| object.get(name).and_then(Value::as_str);
        let id = text("id")
            .filter(|id| (spec.id_ok)(id))
            .ok_or(Refusal::BadRequest)?;
        let t = object
            .get("t")
            .and_then(Value::as_i64)
            .filter(|t| *t > 0)
            .ok_or(Refusal::BadRequest)?;
        let n = text("n")
            .filter(|n| n.len() == 22)
            .ok_or(Refusal::BadRequest)?;
        let nonce = b64url_decode(n)
            .and_then(|b| <[u8; 16]>::try_from(b).ok())
            .ok_or(Refusal::BadRequest)?;
        let p = text("p").ok_or(Refusal::BadRequest)?;
        let signature = text("s")
            .and_then(b64_decode)
            .and_then(|b| <[u8; 64]>::try_from(b).ok())
            .ok_or(Refusal::BadRequest)?;
        let pk = if spec.with_pk {
            Some(
                text("pk")
                    .and_then(b64_decode)
                    .and_then(|b| <[u8; 32]>::try_from(b).ok())
                    .ok_or(Refusal::BadRequest)?,
            )
        } else {
            None
        };
        Ok(Envelope {
            id: id.to_owned(),
            t,
            n: n.to_owned(),
            nonce,
            p: p.to_owned(),
            signature,
            pk,
        })
    }

    /// The bytes this envelope's signature covers.
    pub fn signed_bytes(&self, spec: &Spec) -> Vec<u8> {
        message(spec, &self.id, self.t, &self.n, &self.p)
    }

    /// Step 4: the signature over the bytes as received, by `key`, in the strict form that refuses
    /// non-canonical signatures and small-order keys (as libsodium's verify).
    pub fn verify(&self, spec: &Spec, key: &[u8; 32]) -> bool {
        let Ok(key) = VerifyingKey::from_bytes(key) else {
            return false;
        };
        key.verify_strict(
            &self.signed_bytes(spec),
            &Signature::from_bytes(&self.signature),
        )
        .is_ok()
    }

    /// Step 5: `t` within MAX_SKEW of the server's clock `at`.
    pub fn fresh(&self, at: i64) -> bool {
        self.t
            .checked_sub(at)
            .is_some_and(|d| d.unsigned_abs() <= MAX_SKEW as u64)
    }
}

/// What a PC or a relay does, for tests and tools: the body of an envelope signed with the
/// 32-byte Ed25519 seed `seed`.
pub fn sign_body(
    spec: &Spec,
    seed: &[u8; 32],
    id: &str,
    t: i64,
    nonce: &[u8; 16],
    p: &str,
) -> String {
    let n = b64url(nonce);
    let key = SigningKey::from_bytes(seed);
    let s = b64(&key.sign(&message(spec, id, t, &n, p)).to_bytes());
    let mut body = json!({ "id": id, "t": t, "n": n, "p": p, "s": s });
    if spec.with_pk {
        body["pk"] = json!(b64(&key.verifying_key().to_bytes()));
    }
    body.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::hex;

    const PC: Spec = Spec {
        context: "hjv-pc-v1",
        op: "sync",
        max_body: 16 * 1024,
        id_ok: crate::util::valid_device_id,
        with_pk: false,
    };

    /// relay/PROTOCOL.md §13: the hello of the relay key of seed 00 01 … 1f, in the relays' spec
    /// (relay.rs).
    const HELLO: Spec = crate::relay::HELLO;
    const HELLO_BODY: &str = r#"{"id":"r-kzdvvj2umnduyauf","t":1760000000,"n":"AAECAwQFBgcICQoLDA0ODw","p":"{\"v\":1,\"version\":\"1.6.0\",\"arch\":\"x86_64\",\"os\":\"DSM 7.2.1-69057 Update 5\",\"kernel\":\"5.10.55+\",\"ifaces_hash\":\"4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945\",\"ifaces\":[]}","s":"xfOpDyAd0LYdAww0n+eYJg2DvHTymuHWYOZuGIy89rWYRqHcqEb0PVwTD5MUQIlzSNdUAmsGenMyLh8iqngcAQ==","pk":"A6EHv/POEL4dcN0Y50vAmWfk1jCbpQ1fHdyGZBJVMbg="}"#;
    const HELLO_SIGNED: &[u8] = b"hjv-relay-v1\x00hello\x00r-kzdvvj2umnduyauf\x001760000000\x00AAECAwQFBgcICQoLDA0ODw\x00PZ3ddx3dke+FIcRE2SqeSlomxD+MMZtr3gsC2ztzq0Q=";

    #[test]
    fn the_relay_hello_of_protocol_md() {
        let env = Envelope::parse(HELLO_BODY.as_bytes(), &HELLO).unwrap();
        assert_eq!(env.signed_bytes(&HELLO), HELLO_SIGNED);
        let pk = env.pk.unwrap();
        assert!(env.verify(&HELLO, &pk));
        assert!(env.fresh(1_760_000_300) && env.fresh(1_759_999_700));
        assert!(!env.fresh(1_760_000_301) && !env.fresh(1_759_999_699));
        // the same bytes signed for poll, or by another key, do not pass
        let poll = Spec {
            op: "poll",
            ..HELLO
        };
        assert!(!env.verify(&poll, &pk));
        assert!(!env.verify(&HELLO, &crate::devsig::public_key(&[1; 32])));
        // and this code signs the same body from the seed
        let seed: [u8; 32] = std::array::from_fn(|i| i as u8);
        let nonce: [u8; 16] = std::array::from_fn(|i| i as u8);
        let made = sign_body(&HELLO, &seed, &env.id, env.t, &nonce, &env.p);
        let made: Value = serde_json::from_str(&made).unwrap();
        assert_eq!(made, serde_json::from_str::<Value>(HELLO_BODY).unwrap());
    }

    #[test]
    fn shapes() {
        let seed = [2u8; 32];
        let good = sign_body(&PC, &seed, "123456789", 1_760_000_000, &[7; 16], "{}");
        let env = Envelope::parse(good.as_bytes(), &PC).unwrap();
        assert!(env.verify(&PC, &crate::devsig::public_key(&seed)));
        assert_eq!(env.pk, None);
        let mut value: Value = serde_json::from_str(&good).unwrap();
        // unknown fields are ignored
        value["extra"] = json!(1);
        assert!(Envelope::parse(value.to_string().as_bytes(), &PC).is_ok());
        let bad = |field: &str, v: Value| {
            let mut value: Value = serde_json::from_str(&good).unwrap();
            value[field] = v;
            Envelope::parse(value.to_string().as_bytes(), &PC).unwrap_err()
        };
        for (field, v) in [
            ("id", json!("12 34")),
            ("id", json!(123456789)),
            ("t", json!(0)),
            ("t", json!(-5)),
            ("t", json!(1_760_000_000.5)),
            ("t", json!("1760000000")),
            ("n", json!("AAECAwQFBgcICQoLDA0OD")),
            ("n", json!("AAECAwQFBgcICQoLDA0ODw==")),
            ("n", json!("AAECAwQFBgcICQoLDA0OD+")),
            ("p", json!({"v": 1})),
            ("s", json!("AAAA")),
            ("s", json!(hex(&[0u8; 64]))),
        ] {
            assert_eq!(bad(field, v.clone()), Refusal::BadRequest, "{field}: {v}");
        }
        for body in ["", "[]", "{}", "null", "not json"] {
            assert_eq!(
                Envelope::parse(body.as_bytes(), &PC).unwrap_err(),
                Refusal::BadRequest
            );
        }
        let big = vec![b' '; PC.max_body + 1];
        assert_eq!(Envelope::parse(&big, &PC).unwrap_err(), Refusal::TooLarge);
        // a hello needs its key
        let mut value: Value = serde_json::from_str(HELLO_BODY).unwrap();
        value.as_object_mut().unwrap().remove("pk");
        assert_eq!(
            Envelope::parse(value.to_string().as_bytes(), &HELLO).unwrap_err(),
            Refusal::BadRequest
        );
        // times far off never pass, whatever the overflow
        let mut env = env;
        for t in [i64::MAX, 1] {
            env.t = t;
            assert!(!env.fresh(i64::MIN + 1) && !env.fresh(1_760_000_000));
        }
    }

    #[test]
    fn a_payload_changed_on_the_way_fails() {
        let seed = [2u8; 32];
        let body = sign_body(
            &PC,
            &seed,
            "123456789",
            1_760_000_000,
            &[7; 16],
            r#"{"v":1}"#,
        );
        let mut value: Value = serde_json::from_str(&body).unwrap();
        // the same JSON, written differently: the signature covers the text
        value["p"] = json!(r#"{ "v":1}"#);
        let env = Envelope::parse(value.to_string().as_bytes(), &PC).unwrap();
        assert!(!env.verify(&PC, &crate::devsig::public_key(&seed)));
        let codes: Vec<(u16, &str)> = [
            Refusal::TooLarge,
            Refusal::BadRequest,
            Refusal::BadSignature,
            Refusal::Clock,
            Refusal::Replay,
        ]
        .iter()
        .map(|r| (r.status().as_u16(), r.code()))
        .collect();
        assert_eq!(
            codes,
            [
                (413, "too_large"),
                (400, "bad_request"),
                (401, "bad_signature"),
                (400, "clock"),
                (409, "replay")
            ]
        );
    }
}

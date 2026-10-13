// SPDX-License-Identifier: AGPL-3.0-only

//! The account key's signature on every account request (design v3 §4-2): the header
//!
//! ```text
//! X-HJV-Proof: v1.<t>.<n>.<sig>
//! ```
//!
//! - `<t>`: the device's clock in Unix seconds, decimal without sign or leading zeros (the app
//!   adds the offset it learned from the server's `server_time` and `Date`);
//! - `<n>`: 16 random bytes in base64url without padding (22 characters), new for every request;
//! - `<sig>`: base64url without padding (86 characters) of the 64-byte Ed25519 signature, by the
//!   account key registered at login (keys.rs), over the bytes
//!
//! ```text
//! "hjv-req-v1" NUL <method> NUL <path and query> NUL <t> NUL <n> NUL base64(SHA-256(body)) NUL base64(SHA-256(token))
//! ```
//!
//! with the parts joined by single NUL bytes and no NUL at the end: the method as sent ("GET"),
//! the request target exactly as sent (the path with `?` and the query when there is one, as the
//! app's URL has it: `/api/peers?current=1&pageSize=100&accessible=&status=1`), `t` and `n` as
//! the header has them, and the SHA-256 of the request body (of no bytes for a request without
//! one) and of the bearer token (`hjv_…`, as in the Authorization header), each in standard
//! base64 with padding (44 characters). The token's hash keeps a signature from being paired with
//! another token.
//!
//! hjv-api (api.rs `session`) finds the token, then checks the signature with the token's key
//! strictly (`verify_strict`), then the time (|t − server time| ≤ 600 s), then, for a request
//! that is not GET or HEAD, that the key used `n` not in the last 20 minutes. A missing or wrong
//! proof or a used nonce gets the same 401 as a wrong token; a right signature with a time out of
//! the window gets 409 `{"error":…,"hjv":"clock","server_time":…}`, which only the key's holder
//! can bring about and the app does not take for a logout (it logs out on 401 and 400 only).
//!
//! The one request that may use a nonce again is the very same request ([`request_hash`]) within
//! HJV_API_RETRY_WINDOW (30 s): it gets the first answer again and does not run a second time
//! (api.rs `authenticate`). The app sends a request again byte for byte when its first TLS
//! attempt failed, possibly after hjv-api got it and answered; a 401 there would log the app out.

use crate::util::{b64, b64url, b64url_decode, sha256};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

pub const DOMAIN: &[u8] = b"hjv-req-v1";
/// The header, as hyper names it.
pub const HEADER: &str = "x-hjv-proof";
/// How far the device's clock may be off, either way (the seal's window, design v3 §3-7).
pub const MAX_SKEW: i64 = 600;
/// How long a nonce of a request that changes something is remembered: as long as a request
/// signed at its `t` could pass the time check.
pub const NONCE_TTL: i64 = 2 * MAX_SKEW;

/// A parsed `X-HJV-Proof` value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub time: i64,
    /// `t` and `n` exactly as sent (the signature covers them as text)
    pub t: String,
    pub n: String,
    pub nonce: [u8; 16],
    pub signature: [u8; 64],
}

/// Reads a header value; `None` unless it is exactly `v1.<t>.<n>.<sig>` as above.
pub fn parse(value: &str) -> Option<Header> {
    let mut parts = value.trim().split('.');
    let (Some("v1"), Some(t), Some(n), Some(sig), None) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return None;
    };
    let canonical = !t.is_empty()
        && t.len() <= 18
        && t.bytes().all(|b| b.is_ascii_digit())
        && (t == "0" || !t.starts_with('0'));
    if !canonical {
        return None;
    }
    let time = t.parse::<i64>().ok()?;
    let nonce = <[u8; 16]>::try_from(b64url_decode(n)?).ok()?;
    let signature = <[u8; 64]>::try_from(b64url_decode(sig)?).ok()?;
    Some(Header {
        time,
        t: t.to_owned(),
        n: n.to_owned(),
        nonce,
        signature,
    })
}

/// The signed bytes, from the SHA-256 of the body and of the token.
pub fn message(
    method: &str,
    path_and_query: &str,
    t: &str,
    n: &str,
    body_hash: &[u8; 32],
    token_hash: &[u8; 32],
) -> Vec<u8> {
    let (body, token) = (b64(body_hash), b64(token_hash));
    let fields: [&[u8]; 7] = [
        DOMAIN,
        method.as_bytes(),
        path_and_query.as_bytes(),
        t.as_bytes(),
        n.as_bytes(),
        body.as_bytes(),
        token.as_bytes(),
    ];
    fields.join(&0u8)
}

/// What a request's proof is worth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// signed by the key, in time; for a request that changes something its nonce is still to be
    /// recorded (api.rs)
    Valid { nonce: [u8; 16] },
    /// signed by the key, but the device's clock is more than MAX_SKEW off
    Clock,
    /// no header
    Missing,
    /// not the format, or not a signature of this request by this key
    Invalid,
}

/// Checks a request's proof against the token's account key `public_key`, in the order of design
/// v3 §4-2: the signature first, then the time.
pub fn check(
    public_key: &[u8; 32],
    header: Option<&str>,
    method: &str,
    path_and_query: &str,
    body_hash: &[u8; 32],
    token_hash: &[u8; 32],
    now: i64,
) -> Check {
    let Some(value) = header else {
        return Check::Missing;
    };
    let Some(h) = parse(value) else {
        return Check::Invalid;
    };
    let Ok(key) = VerifyingKey::from_bytes(public_key) else {
        return Check::Invalid;
    };
    let signed = message(method, path_and_query, &h.t, &h.n, body_hash, token_hash);
    if key
        .verify_strict(&signed, &Signature::from_bytes(&h.signature))
        .is_err()
    {
        return Check::Invalid;
    }
    if now.abs_diff(h.time) > MAX_SKEW.unsigned_abs() {
        return Check::Clock;
    }
    Check::Valid { nonce: h.nonce }
}

/// What tells a signed request from every other one: the SHA-256 of its header value (`t`, `n`
/// and the signature, which [`parse`] takes in one spelling each) and of what else the signature
/// covers. A retry that sends the request again byte for byte has the same; a request that differs
/// in the method, the target, the body, the token, the time or the signature has another.
pub fn request_hash(
    value: &str,
    method: &str,
    path_and_query: &str,
    body_hash: &[u8; 32],
    token_hash: &[u8; 32],
) -> [u8; 32] {
    let fields: [&[u8]; 5] = [
        value.trim().as_bytes(),
        method.as_bytes(),
        path_and_query.as_bytes(),
        body_hash,
        token_hash,
    ];
    sha256(&fields.join(&0u8))
}

/// What the app does, for tests and tools: the header value for a request signed with the
/// account key's 32-byte seed.
pub fn header_value(
    seed: &[u8; 32],
    method: &str,
    path_and_query: &str,
    time: i64,
    nonce: &[u8; 16],
    body: &[u8],
    token: &str,
) -> String {
    let (t, n) = (time.to_string(), b64url(nonce));
    let signed = message(
        method,
        path_and_query,
        &t,
        &n,
        &sha256(body),
        &sha256(token.as_bytes()),
    );
    let signature = SigningKey::from_bytes(seed).sign(&signed).to_bytes();
    format!("v1.{t}.{n}.{}", b64url(&signature))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_760_000_000;

    fn key() -> ([u8; 32], [u8; 32]) {
        let seed = [3u8; 32];
        (seed, crate::devsig::public_key(&seed))
    }

    #[test]
    fn round_trip_and_order_of_checks() {
        let (seed, pk) = key();
        let token = sha256(b"hjv_token");
        let body = sha256(b"{}");
        let h = header_value(
            &seed,
            "POST",
            "/api/logout",
            NOW,
            &[1; 16],
            b"{}",
            "hjv_token",
        );
        let run = |header: &str, method: &str, path: &str, body: &[u8; 32], now: i64| {
            check(&pk, Some(header), method, path, body, &token, now)
        };
        assert_eq!(
            run(&h, "POST", "/api/logout", &body, NOW),
            Check::Valid { nonce: [1; 16] }
        );
        // in time at the edges of the window, a clock answer past them
        assert!(matches!(
            run(&h, "POST", "/api/logout", &body, NOW + 600),
            Check::Valid { .. }
        ));
        assert!(matches!(
            run(&h, "POST", "/api/logout", &body, NOW - 600),
            Check::Valid { .. }
        ));
        assert_eq!(
            run(&h, "POST", "/api/logout", &body, NOW + 601),
            Check::Clock
        );
        assert_eq!(
            run(&h, "POST", "/api/logout", &body, NOW - 601),
            Check::Clock
        );
        // another method, path, body, token or key: not this request's signature, whatever the time
        assert_eq!(
            run(&h, "GET", "/api/logout", &body, NOW + 9999),
            Check::Invalid
        );
        assert_eq!(run(&h, "POST", "/api/logout?x", &body, NOW), Check::Invalid);
        assert_eq!(
            run(&h, "POST", "/api/logout", &sha256(b"{ }"), NOW),
            Check::Invalid
        );
        assert_eq!(
            check(
                &pk,
                Some(&h),
                "POST",
                "/api/logout",
                &body,
                &sha256(b"hjv_other"),
                NOW
            ),
            Check::Invalid
        );
        let other = crate::devsig::public_key(&[4; 32]);
        assert_eq!(
            check(&other, Some(&h), "POST", "/api/logout", &body, &token, NOW),
            Check::Invalid
        );
        assert_eq!(
            check(&pk, None, "POST", "/api/logout", &body, &token, NOW),
            Check::Missing
        );
    }

    #[test]
    fn header_format() {
        let (seed, _) = key();
        let h = header_value(&seed, "GET", "/api/users", NOW, &[0; 16], b"", "hjv_t");
        let parsed = parse(&h).unwrap();
        assert_eq!((parsed.time, parsed.t.as_str()), (NOW, "1760000000"));
        assert_eq!(parsed.n.len(), 22);
        assert_eq!(h.len(), 3 + 10 + 1 + 22 + 1 + 86);
        let sig = h.rsplit('.').next().unwrap();
        for bad in [
            String::new(),
            "v1".to_owned(),
            h.replacen("v1.", "v2.", 1),
            h.replacen("1760000000", "01760000000", 1),
            h.replacen("1760000000", "+1760000000", 1),
            h.replacen("1760000000", "-1760000000", 1),
            h.replacen("1760000000", "17600000000000000000", 1),
            h.replacen(&parsed.n, "AAAA", 1),
            h.replacen(sig, &sig[..80], 1),
            format!("{h}.x"),
            h.replace('.', ","),
        ] {
            assert!(parse(&bad).is_none(), "{bad}");
        }
        // a nonce or signature in standard base64 is not the format
        let padded = h.replacen(&parsed.n, &format!("{}==", parsed.n), 1);
        assert!(parse(&padded).is_none());
    }

    #[test]
    fn the_very_same_request_and_every_other() {
        let (seed, _) = key();
        let (body, token) = (sha256(b"{}"), sha256(b"hjv_token"));
        let sign = |seed: &[u8; 32], t: i64| {
            header_value(seed, "POST", "/api/logout", t, &[1; 16], b"{}", "hjv_token")
        };
        let h = sign(&seed, NOW);
        let first = request_hash(&h, "POST", "/api/logout", &body, &token);
        // byte for byte again, also with the blanks HTTP allows around a header value
        assert_eq!(
            request_hash(&format!(" {h}\t"), "POST", "/api/logout", &body, &token),
            first
        );
        // the same nonce with another time, another signature (here another key's), method,
        // target, body or token
        for (what, other) in [
            (
                "time",
                request_hash(&sign(&seed, NOW + 1), "POST", "/api/logout", &body, &token),
            ),
            (
                "signature",
                request_hash(&sign(&[4; 32], NOW), "POST", "/api/logout", &body, &token),
            ),
            (
                "method",
                request_hash(&h, "DELETE", "/api/logout", &body, &token),
            ),
            (
                "target",
                request_hash(&h, "POST", "/api/logout?", &body, &token),
            ),
            (
                "body",
                request_hash(&h, "POST", "/api/logout", &sha256(b"{ }"), &token),
            ),
            (
                "token",
                request_hash(&h, "POST", "/api/logout", &body, &sha256(b"hjv_x")),
            ),
        ] {
            assert_ne!(other, first, "{what}");
        }
    }
}

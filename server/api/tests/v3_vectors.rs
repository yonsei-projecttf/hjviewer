// SPDX-License-Identifier: AGPL-3.0-only

//! The shared test vectors of the same-account logins (design v3 §3-3, §3-6, §4-1, §4-2) in
//! tests/data/v3-vectors.json, which the client renders from its src/hjviewer_account/vectors.rs
//! and checks there with libsodium. Here the same seeds and inputs go through ed25519-dalek, as
//! hjv-api signs and verifies: the layouts are rebuilt from the design, not from the file, and must
//! give the file's bytes and signatures. The private keys are public test values.

use hjv_api::{
    devsig, seal,
    util::{b64, b64_decode, b64url, hex, sha256},
};
use serde_json::Value;

const JSON: &str = include_str!("data/v3-vectors.json");
/// The client's vectors.rs JSON_SHA256: the file is its rendering, byte for byte.
const JSON_SHA256: &str = "2379784b5fd3e8590b1e7783341ea9c057eb40340d929d074e0b7dc4fc3ef112";

fn vectors() -> Value {
    serde_json::from_str(JSON).unwrap()
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key]
        .as_str()
        .unwrap_or_else(|| panic!("{key} is not a string"))
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

fn seed(key: &Value) -> [u8; 32] {
    unhex(s(key, "seed_hex")).try_into().unwrap()
}

fn joined(parts: &[&[u8]]) -> Vec<u8> {
    parts.join(&0u8)
}

fn sha256_b64(data: &[u8]) -> String {
    b64(&sha256(data))
}

/// `YYYY-MM-DD` of a Unix time in UTC (the civil-from-days algorithm).
fn utc_day(t: u64) -> String {
    let days = (t / 86_400) as i64 + 719_468;
    let era = days.div_euclid(146_097);
    let doe = days.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[test]
fn the_file_is_the_clients_rendering() {
    assert_eq!(hex(&sha256(JSON.as_bytes())), JSON_SHA256);
}

#[test]
fn keys_from_their_seeds() {
    let v = vectors();
    let keys = &v["keys"];
    for name in ["list_1", "list_2", "pc_device", "account"] {
        assert_eq!(
            b64(&devsig::public_key(&seed(&keys[name]))),
            s(&keys[name], "pk"),
            "{name}"
        );
    }
    assert_eq!(
        s(keys, "HJV_TRUST_PUBKEYS"),
        format!(
            "1:{},2:{}",
            s(&keys["list_1"], "pk"),
            s(&keys["list_2"], "pk")
        )
    );
    let seal_pk: [u8; 32] = b64_decode(s(&keys["seal"], "pk"))
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(seal::key_id(&seal_pk), s(&keys["seal"], "kid"));
    assert_eq!(s(keys, "HJV_LOGIN_SEAL_PUBKEY"), s(&keys["seal"], "pk"));
}

#[test]
fn trust_list_signature() {
    let v = vectors();
    let t = &v["trust_list"];
    let list = s(t, "list");
    assert_eq!(s(t, "l"), b64(list.as_bytes()));
    let mut signed = b"hjv-trust-v1\0".to_vec();
    signed.extend_from_slice(list.as_bytes());
    assert_eq!(hex(&signed), s(t, "signed_hex"));
    let key = seed(&v["keys"]["list_1"]);
    assert_eq!(devsig::sign(&key, &signed), s(t, "s"));
    assert!(devsig::verify(
        &devsig::public_key(&key),
        &signed,
        s(t, "s")
    ));
    assert!(!devsig::verify(
        &devsig::public_key(&seed(&v["keys"]["list_2"])),
        &signed,
        s(t, "s")
    ));
    // what the PC checks after the signature: the list's own key number, this PC, its binding,
    // this request, and a lifetime within the build's ceiling
    let l: Value = serde_json::from_str(list).unwrap();
    assert_eq!(
        (l["v"].as_u64(), l["typ"].as_str(), l["kid"].as_u64()),
        (Some(1), Some("hjv-trust"), t["kid"].as_u64())
    );
    for field in ["pc_id", "pc_pk", "binding", "req"] {
        assert_eq!(l[field], t[field], "{field}");
    }
    assert_eq!(l["req"], v["pc_sync"]["n"]);
    let life = l["exp"].as_u64().unwrap() - l["iat"].as_u64().unwrap();
    assert!(life > 0 && life <= t["max_list_ttl"].as_u64().unwrap());
}

#[test]
fn connection_proof() {
    let v = vectors();
    let c = &v["conn_proof"];
    let (pc_id, ctl_id) = (s(c, "pc_id"), s(c, "ctl_id"));
    let ctl_pk = b64_decode(s(c, "ctl_pk")).unwrap();
    let mut transcript = b"hjv-conn-v1\0".to_vec();
    transcript.push(pc_id.len() as u8);
    transcript.extend_from_slice(pc_id.as_bytes());
    transcript.extend_from_slice(&b64_decode(s(c, "pc_pk")).unwrap());
    transcript.extend_from_slice(&unhex(s(c, "nonce_hex")));
    transcript.push(ctl_id.len() as u8);
    transcript.extend_from_slice(ctl_id.as_bytes());
    transcript.push(c["kind"].as_u64().unwrap() as u8);
    transcript.extend_from_slice(&ctl_pk);
    assert_eq!(hex(&transcript), s(c, "transcript_hex"));
    let key = seed(&v["keys"]["account"]);
    assert_eq!(b64(&devsig::public_key(&key)), s(c, "ctl_pk"));
    assert_eq!(devsig::sign(&key, &transcript), s(c, "signature"));
    // HjvKeyProof { version = 1; pk = 2; signature = 3; key_serial = 4 } on the wire
    let mut proto = vec![0x08, 0x01, 0x12, 0x20];
    proto.extend_from_slice(&ctl_pk);
    proto.extend_from_slice(&[0x1a, 0x40]);
    proto.extend_from_slice(&b64_decode(s(c, "signature")).unwrap());
    proto.push(0x20);
    let mut serial = c["key_serial"].as_u64().unwrap();
    while serial >= 0x80 {
        proto.push((serial as u8 & 0x7f) | 0x80);
        serial >>= 7;
    }
    proto.push(serial as u8);
    assert_eq!(hex(&proto), s(c, "proto_hex"));
}

#[test]
fn request_proofs() {
    let v = vectors();
    let key = seed(&v["keys"]["account"]);
    for r in v["request_proofs"].as_array().unwrap() {
        let t = r["t"].as_u64().unwrap().to_string();
        let n = s(r, "n");
        assert_eq!(n, b64url(&unhex(s(r, "n_hex"))));
        let body = sha256_b64(s(r, "body").as_bytes());
        let token = sha256_b64(s(r, "token").as_bytes());
        let signed = joined(&[
            b"hjv-req-v1",
            s(r, "method").as_bytes(),
            s(r, "path_and_query").as_bytes(),
            t.as_bytes(),
            n.as_bytes(),
            body.as_bytes(),
            token.as_bytes(),
        ]);
        assert_eq!(hex(&signed), s(r, "signed_hex"));
        let sig = b64_decode(&devsig::sign(&key, &signed)).unwrap();
        assert_eq!(s(r, "header"), format!("v1.{t}.{n}.{}", b64url(&sig)));
    }
}

#[test]
fn pc_sync_envelope_and_use_report() {
    let v = vectors();
    let p = &v["pc_sync"];
    let (t, n, payload) = (p["t"].as_u64().unwrap(), s(p, "n"), s(p, "p"));
    assert_eq!(n, b64url(&unhex(s(p, "n_hex"))));
    assert_eq!(sha256_b64(payload.as_bytes()), s(p, "p_sha256"));
    let ts = t.to_string();
    let signed = joined(&[
        b"hjv-pc-v1",
        b"sync",
        s(p, "id").as_bytes(),
        ts.as_bytes(),
        n.as_bytes(),
        s(p, "p_sha256").as_bytes(),
    ]);
    assert_eq!(hex(&signed), s(p, "signed_hex"));
    let key = seed(&v["keys"]["pc_device"]);
    assert_eq!(devsig::sign(&key, &signed), s(p, "s"));
    // the request body, as the server reads its envelope
    let body: Value = serde_json::from_str(s(p, "body")).unwrap();
    assert_eq!(
        (
            body["id"].as_str(),
            body["t"].as_u64(),
            body["n"].as_str(),
            body["p"].as_str(),
            body["s"].as_str()
        ),
        (
            Some(s(p, "id")),
            Some(t),
            Some(n),
            Some(payload),
            Some(s(p, "s"))
        )
    );
    // `used`: a line per key and UTC day, `last` that day's latest acceptance by the PC's clock,
    // `n` that day's running count; only today and yesterday
    let used = serde_json::from_str::<Value>(payload).unwrap()["used"].clone();
    assert_eq!(used, p["used"]);
    let accepted: Vec<u64> = p["accepted"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_u64().unwrap())
        .collect();
    let days = [utc_day(t), utc_day(t - 86_400)];
    for line in used.as_array().unwrap() {
        let day = s(line, "day");
        assert!(days.iter().any(|d| d == day), "{day}");
        let that_day: Vec<u64> = accepted
            .iter()
            .copied()
            .filter(|at| utc_day(*at) == day)
            .collect();
        assert_eq!(line["n"].as_u64(), Some(that_day.len() as u64));
        assert_eq!(line["last"].as_u64(), that_day.iter().max().copied());
    }
    let reported: u64 = used
        .as_array()
        .unwrap()
        .iter()
        .map(|line| line["n"].as_u64().unwrap())
        .sum();
    assert!(
        reported < accepted.len() as u64,
        "an older day is not reported"
    );
}

#[test]
fn sealed_login_key() {
    let v = vectors();
    let c = &v["seal_ckey"];
    let signed = joined(&[
        b"hjv-ckey-v1",
        s(c, "n").as_bytes(),
        s(c, "id").as_bytes(),
        s(c, "ck").as_bytes(),
    ]);
    assert_eq!(hex(&signed), s(c, "signed_hex"));
    let key = seed(&v["keys"]["account"]);
    assert_eq!(s(c, "ck"), b64(&devsig::public_key(&key)));
    assert_eq!(devsig::sign(&key, &signed), s(c, "cp"));
    let ck = b64_decode(s(c, "ck")).unwrap();
    assert!(devsig::verify(&ck, &signed, s(c, "cp")));
    // the sealed payload carries them beside the fields seal.rs reads
    let payload: Value = serde_json::from_str(s(c, "payload")).unwrap();
    for field in ["n", "id", "ck", "cp"] {
        assert_eq!(payload[field], c[field], "{field}");
    }
    assert_eq!(payload["kid"], v["keys"]["seal"]["kid"]);
    assert_eq!(b64_decode(s(c, "n")).unwrap().len(), 16);
}

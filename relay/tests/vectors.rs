// SPDX-License-Identifier: AGPL-3.0-only

//! The test vectors of relay/PROTOCOL.md ("Test vectors"), checked against the relay's code and
//! against plain Ed25519/SHA-256, so the document and the implementation cannot drift apart.

use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use hjv_relay::{
    command,
    config::parse_command_keys,
    envelope::{self, Op},
    identity::Identity,
};
use sha2::{Digest, Sha256};

const RELAY_PK: &str = "A6EHv/POEL4dcN0Y50vAmWfk1jCbpQ1fHdyGZBJVMbg=";
const RELAY_ID: &str = "r-kzdvvj2umnduyauf";
const RELAY_CODE: &str = "KZDV-VJ2U-MNDU-YAUF";
const RELAY_KEY_DIGEST: &str = "56475aa75463474c0285";
const NONCE: &str = "AAECAwQFBgcICQoLDA0ODw";
const T: i64 = 1760000000;
const HELLO_P: &str = r#"{"v":1,"version":"1.6.0","arch":"x86_64","os":"DSM 7.2.1-69057 Update 5","kernel":"5.10.55+","ifaces_hash":"4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945","ifaces":[]}"#;
const HELLO_P_SHA256: &str = "PZ3ddx3dke+FIcRE2SqeSlomxD+MMZtr3gsC2ztzq0Q=";
const HELLO_SIGNED: &[u8] = b"hjv-relay-v1\0hello\0r-kzdvvj2umnduyauf\x001760000000\0AAECAwQFBgcICQoLDA0ODw\0PZ3ddx3dke+FIcRE2SqeSlomxD+MMZtr3gsC2ztzq0Q=";
const HELLO_S: &str =
    "xfOpDyAd0LYdAww0n+eYJg2DvHTymuHWYOZuGIy89rWYRqHcqEb0PVwTD5MUQIlzSNdUAmsGenMyLh8iqngcAQ==";
const HELLO_BODY: &str = r#"{"id":"r-kzdvvj2umnduyauf","t":1760000000,"n":"AAECAwQFBgcICQoLDA0ODw","p":"{\"v\":1,\"version\":\"1.6.0\",\"arch\":\"x86_64\",\"os\":\"DSM 7.2.1-69057 Update 5\",\"kernel\":\"5.10.55+\",\"ifaces_hash\":\"4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945\",\"ifaces\":[]}","s":"xfOpDyAd0LYdAww0n+eYJg2DvHTymuHWYOZuGIy89rWYRqHcqEb0PVwTD5MUQIlzSNdUAmsGenMyLh8iqngcAQ==","pk":"A6EHv/POEL4dcN0Y50vAmWfk1jCbpQ1fHdyGZBJVMbg="}"#;
const COMMAND_PUBKEY: &str = "1:Kay64UG8yvCyLhqU000LxzYeUm0L/hLIl5S8kyKWbdc=";
const C: &str = r#"{"v":1,"job":"0123456789abcdef0123456789abcdef","relay":"r-kzdvvj2umnduyauf","iat":1760000000,"exp":1760000060,"net":"10.1.4.0/22","macs":["00:00:5e:00:53:01"],"ports":[9,7],"repeat":3,"gap_ms":200}"#;
const C_S: &str =
    "91MXS5rMOB9k3ZP7XxPue9M++nXy9w4VIpjcqGJspnFI46U37TGC+st2hv6D8DxcUJOaN/2PcY7XeQ6wgfmLBw==";
const C_HASH: &str = "eefb709bfc6dae46233f75d43c02d0dfb173218d3ed9010c40dc316b4c3d7abe";
const COMMAND_FINGERPRINT: &str =
    "1:24f6ed6acbfe1009c030d7ca567c33ca4830911498236b5561a6c82abec5de28";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn relay_seed() -> [u8; 32] {
    std::array::from_fn(|i| i as u8)
}

fn command_seed() -> [u8; 32] {
    std::array::from_fn(|i| (i + 32) as u8)
}

#[test]
fn relay_identity() {
    let id = Identity::from_seed(&relay_seed());
    assert_eq!(id.pk_base64(), RELAY_PK);
    assert_eq!(id.id, RELAY_ID);
    assert_eq!(id.code, RELAY_CODE);
    // the key digest (section 3.1): the first 10 bytes of SHA-256(pk)
    let digest = &Sha256::digest(STANDARD.decode(RELAY_PK).unwrap())[..10];
    assert_eq!(hex(digest), RELAY_KEY_DIGEST);
    assert_eq!(
        hjv_relay::identity::key_digest(&STANDARD.decode(RELAY_PK).unwrap().try_into().unwrap()),
        digest
    );
    assert_eq!(hjv_relay::identity::base32(digest), "KZDVVJ2UMNDUYAUF");
}

#[test]
fn hello_envelope() {
    let id = Identity::from_seed(&relay_seed());
    assert_eq!(
        URL_SAFE_NO_PAD.encode(std::array::from_fn::<u8, 16, _>(|i| i as u8)),
        NONCE
    );
    assert_eq!(
        STANDARD.encode(Sha256::digest(HELLO_P.as_bytes())),
        HELLO_P_SHA256
    );
    let msg = envelope::signed_message(Op::Hello, RELAY_ID, T, NONCE, HELLO_P);
    assert_eq!(msg, HELLO_SIGNED);
    assert_eq!(STANDARD.encode(id.sign(&msg)), HELLO_S);
    assert_eq!(
        envelope::seal_with_nonce(&id, Op::Hello, T, NONCE, HELLO_P).unwrap(),
        HELLO_BODY
    );
    // what the server does: unescape p, hash it, rebuild the message, verify with pk
    let body: serde_json::Value = serde_json::from_str(HELLO_BODY).unwrap();
    let p = body["p"].as_str().unwrap();
    assert_eq!(p, HELLO_P);
    let pk: [u8; 32] = STANDARD.decode(RELAY_PK).unwrap().try_into().unwrap();
    let sig: [u8; 64] = STANDARD.decode(HELLO_S).unwrap().try_into().unwrap();
    VerifyingKey::from_bytes(&pk)
        .unwrap()
        .verify_strict(HELLO_SIGNED, &Signature::from_bytes(&sig))
        .unwrap();
}

#[test]
fn the_document_shows_these_vectors() {
    let doc = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/PROTOCOL.md")).unwrap();
    let signed = String::from_utf8_lossy(HELLO_SIGNED).replace('\0', "\\0");
    let entry = serde_json::json!({"c": C, "k": 1, "s": C_S}).to_string();
    for v in [
        RELAY_PK,
        RELAY_ID,
        RELAY_CODE,
        RELAY_KEY_DIGEST,
        NONCE,
        HELLO_P,
        HELLO_P_SHA256,
        &signed,
        HELLO_S,
        HELLO_BODY,
        COMMAND_PUBKEY,
        C,
        C_S,
        C_HASH,
        &entry,
        COMMAND_FINGERPRINT,
    ] {
        assert!(doc.contains(v), "PROTOCOL.md does not show {v}");
    }
    assert!(doc.contains(&format!("signed bytes ({} bytes", HELLO_SIGNED.len())));
    // one name for each: the relay key's digest (section 3.1) and a command key's fingerprint
    // (section 3.2), which a person compares with hjv-api selfcheck
    assert!(doc.contains(&format!("| key digest (hex) | `{RELAY_KEY_DIGEST}` |")));
    assert!(doc.contains("**command key fingerprint**"));
    assert!(
        !doc.contains("**fingerprint** ="),
        "section 3.1 names the key digest"
    );
}

#[test]
fn wake_command() {
    let key = SigningKey::from_bytes(&command_seed());
    let keys = parse_command_keys(COMMAND_PUBKEY).unwrap();
    assert_eq!(keys[0].key, key.verifying_key());
    // the one text form of a public key, and its fingerprint: hex SHA-256 of the key bytes
    assert_eq!(keys[0].to_string(), COMMAND_PUBKEY);
    assert_eq!(
        format!("{}:{}", keys[0].id, keys[0].fingerprint()),
        COMMAND_FINGERPRINT
    );
    let plain: String = Sha256::digest(key.verifying_key().to_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(COMMAND_FINGERPRINT, format!("1:{plain}"));
    let mut msg = b"hjv-wake-v1\0".to_vec();
    msg.extend_from_slice(C.as_bytes());
    assert_eq!(STANDARD.encode(key.sign(&msg).to_bytes()), C_S);
    assert_eq!(command::command_hash(C), C_HASH);
    let entry = serde_json::json!({"c": C, "s": C_S, "k": 1});
    let signed = command::signed_entry(&entry).unwrap();
    command::verify(&keys, &signed).unwrap();
    let cmd = command::parse(signed.c).unwrap();
    assert_eq!(cmd.relay, RELAY_ID);
    assert_eq!(cmd.exp - cmd.iat, 60);
}

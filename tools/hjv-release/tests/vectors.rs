// SPDX-License-Identifier: AGPL-3.0-only

//! The test vectors of the signed version policy (tests/vectors/, described in its README.md):
//! envelopes an app must take or refuse, each with the envelopes it has taken before. They are
//! made by tests/vectors/generate.py from the cases of build/minver/trust-rules/spec.md 7-4, with
//! the verdicts of the reference model (tests/vectors/model.py, a byte-identical copy of
//! build/minver/trust-rules/model.py). The client's implementation reads the same files.
//!
//!     python3 -I -B tests/vectors/generate.py
//!
//! writes them again.

use hjv_release::{
    envelope::{Envelope, POLICY_DOMAIN},
    keyfile::SecretKey,
    keys::{test_key, test_key_set, Role},
    trust::{check_envelope, hex, sha256},
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const SERVER: &str = "rd.example.com";
const KEYFILE_PASSPHRASE: &str = "test passphrase, not a secret";

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors")
}

fn signed(p: &str, by: &str, domain: &[u8]) -> hjv_release::envelope::Part {
    let key = test_key(by).unwrap();
    hjv_release::envelope::Part::sign(
        &ed25519_dalek::SigningKey::from_bytes(&key.seed()),
        key.id,
        domain,
        p,
    )
}

/// The files are what generate.py makes, and the model's verdict on every file on disk is what
/// manifest.json says. python3 is required: the model is the oracle of the vectors.
#[test]
fn the_model_agrees_with_every_vector() {
    let out = Command::new("python3")
        .args(["-I", "-B"])
        .arg(dir().join("generate.py"))
        .arg("--check")
        .output()
        .expect("python3 runs tests/vectors/generate.py --check (the reference model is Python)");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "generate.py --check failed:\n{text}");
    assert!(text.contains("the model agrees with manifest.json on all"), "{text}");
}

#[test]
fn the_research_vector() {
    // build/minver/policy-distribution/experiments/vector: the same key (seed 40..5f), the same
    // domain, checked there with OpenSSL, ed25519-dalek and libsodium
    let vector: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir().join("research/vector.json")).unwrap())
            .unwrap();
    let p = vector["policy"]["p"].as_str().unwrap();
    let signature = vector["policy"]["signature_b64"].as_str().unwrap();
    let seed = vector["update_key"]["seed_hex"].as_str().unwrap();
    let key1 = test_key("signing-1").unwrap();
    assert_eq!(hex(&key1.seed()), seed);
    assert_eq!(
        format!("HJV_UPDATE_PUBKEY={}", key1.public()),
        vector["update_key"]["build_variable"].as_str().unwrap()
    );

    // signing the same bytes gives the same signature, and the same envelope file
    let part = signed(p, "signing-1", POLICY_DOMAIN);
    let envelope = Envelope {
        policy: part.clone(),
        root: None,
    };
    assert!(envelope.to_text().contains(signature));
    assert_eq!(
        hex(&sha256(&hjv_release::envelope::message(POLICY_DOMAIN, p))),
        vector["policy"]["signed_bytes_sha256"].as_str().unwrap()
    );
    let on_disk = fs::read_to_string(dir().join("research/version-policy.json")).unwrap();
    assert_eq!(envelope.to_file(), on_disk);
    assert_eq!(
        on_disk.len() as u64,
        vector["policy"]["envelope_bytes"].as_u64().unwrap()
    );

    // the signature verifies; a changed byte and the wake-command domain do not
    let public = key1.public().key;
    let parsed = Envelope::parse(&on_disk).unwrap();
    assert!(parsed.policy.verifies(&public, POLICY_DOMAIN));
    assert!(!parsed.policy.verifies(&public, b"hjv-wake-v1"));
    let mut changed = parsed.policy.clone();
    changed.p = changed.p.replacen("\"seq\":7", "\"seq\":8", 1);
    assert!(!changed.verifies(&public, POLICY_DOMAIN));
    // as a policy it is refused: it is the first draft of the format, without an epoch
    let err = check_envelope(&on_disk, &test_key_set(), SERVER).unwrap_err();
    assert!(err.contains("epoch"), "{err}");
}

#[test]
fn a_key_file_made_by_age() {
    // made by the age crate, not by this tool: the tool reads what the age format says
    let file = fs::read(dir().join("keyfile/signing-1.age")).unwrap();
    let key = SecretKey::decrypt(&file, KEYFILE_PASSPHRASE).unwrap();
    assert_eq!((key.role, key.id), (Role::Signing, 1));
    assert_eq!(key.public(), test_key("signing-1").unwrap().public());
    assert!(SecretKey::decrypt(&file, "another passphrase").is_err());
    // and with it, the first vector's signature
    let e1_s1 = fs::read_to_string(dir().join("accepted/e1-s1.json")).unwrap();
    let envelope = Envelope::parse(&e1_s1).unwrap();
    assert_eq!(
        key.sign(&hjv_release::envelope::message(
            POLICY_DOMAIN,
            &envelope.policy.p
        )),
        envelope.policy.s
    );
}

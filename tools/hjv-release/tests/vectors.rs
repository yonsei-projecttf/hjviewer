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
    envelope::{Envelope, Part, POLICY_DOMAIN, ROOT_DOMAIN},
    keyfile::SecretKey,
    keys::{test_key, test_key_set, Floor, KeySet, Role},
    policy::{Requirement, RootDecl},
    time::parse_utc,
    trust::{self, check_envelope, hex, sha256, Outcome, State},
    version::Version,
};
use serde_json::{json, Value};
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
    assert!(
        text.contains("the model agrees with manifest.json on all"),
        "{text}"
    );
}

fn manifest() -> Value {
    serde_json::from_str(&fs::read_to_string(dir().join("manifest.json")).unwrap()).unwrap()
}

fn bytes(path: &str) -> Vec<u8> {
    fs::read(dir().join(path)).unwrap()
}

/// A state in the manifest's words (tests/vectors/README.md, "A case").
fn state_json(state: &State, keys: &KeySet) -> Value {
    json!({
        "epoch": state.epoch(keys),
        "seq": state.held.as_ref().map(|h| h.policy.seq),
        "revoked": state.revoked(),
        "root_sha256": state.decl.as_ref().map(|d| hex(&sha256(d.part.p.as_bytes()))),
        "root_k": state.decl.as_ref().map(|d| d.part.k),
        "p_sha256": state.held.as_ref().map(|h| hex(&h.hash)),
        "enforces": state.enforces(),
        "may_install": state.may_install(keys),
    })
}

fn keys_of(case: &Value) -> KeySet {
    let mut keys = test_key_set();
    if let Some(floor) = case["floor"].as_str() {
        keys.floor = Floor::parse(floor).unwrap();
    }
    keys
}

/// For every canonical case the tool's simulated app gives the manifest's outcome, reason and
/// state, and the same next envelope; every other case its strict layer refuses (spec 4-1,
/// INV-13). Every `binding: tool` case the tool refuses as well, and every envelope an app takes
/// with `binding: app` the tool takes by itself.
#[test]
fn the_tool_reads_every_vector_as_the_manifest_says() {
    let manifest = manifest();
    let cases = manifest["cases"].as_array().unwrap();
    assert!(cases.len() >= 130, "{} cases", cases.len());
    let mut canonical_cases = 0;
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let keys = keys_of(case);
        let mut state = State::default();
        for before in case["after"].as_array().unwrap() {
            let (next, outcome) =
                trust::accept(&keys, SERVER, &state, &bytes(before.as_str().unwrap())).unwrap();
            assert!(
                outcome.is_taken(),
                "{name}: {before} before it: {outcome:?}"
            );
            state = next;
        }
        let file = bytes(case["file"].as_str().unwrap());
        let canonical = case["canonical"].as_bool().unwrap();
        assert_eq!(
            trust::canonical(&file).is_ok(),
            canonical,
            "{name}: canonical by the tool's form"
        );
        let text = String::from_utf8_lossy(&file).into_owned();
        if case["binding"] == "tool" {
            assert!(
                check_envelope(&text, &keys, SERVER).is_err(),
                "{name}: the tool takes a binding tool case"
            );
            assert_eq!(case["app_expect"], case["expect"], "{name}");
        }
        if !canonical {
            assert!(
                trust::accept(&keys, SERVER, &state, &file).is_err(),
                "{name}: the strict layer lets a text through that is not canonical"
            );
            continue;
        }
        canonical_cases += 1;
        let (after, outcome) = trust::accept(&keys, SERVER, &state, &file).unwrap();
        match outcome {
            Outcome::Refused(reason) => {
                assert_eq!(case["expect"], "reject", "{name}: refused {reason:?}");
                assert_eq!(case["reason"], reason.code().as_str(), "{name}");
                match case.get("state_after_refusal") {
                    Some(want) => {
                        let got = state_json(&after, &keys);
                        for (key, value) in want.as_object().unwrap() {
                            assert_eq!(&got[key], value, "{name}: state_after_refusal.{key}");
                        }
                    }
                    None => assert_eq!(after, state, "{name}: the state changed"),
                }
            }
            taken => {
                assert_eq!(case["expect"], "accept", "{name}: {taken:?}");
                let word = if taken == Outcome::Taken {
                    "taken"
                } else {
                    "unchanged"
                };
                assert_eq!(case["outcome"], word, "{name}");
                assert_eq!(case["state_after"], state_json(&after, &keys), "{name}");
                if case["binding"] == "app" {
                    check_envelope(&text, &keys, SERVER)
                        .unwrap_or_else(|e| panic!("{name}: the tool refuses it by itself: {e}"));
                }
            }
        }
        if let Some(next) = case["then_accept"].as_str() {
            let (_, then) = trust::accept(&keys, SERVER, &after, &bytes(next)).unwrap();
            let word = match then {
                Outcome::Taken => "taken",
                Outcome::Unchanged => "unchanged",
                Outcome::Refused(_) => "refused",
            };
            assert_eq!(case["then_outcome"], word, "{name}: then {next}");
        }
    }
    assert!(canonical_cases >= 110, "{canonical_cases} canonical cases");
}

/// covers.json: COVERS of spec 2-3 for every pair of 16 entries.
#[test]
fn covers_as_the_vectors_say() {
    let rows: Vec<Value> =
        serde_json::from_str(&fs::read_to_string(dir().join("covers.json")).unwrap()).unwrap();
    assert_eq!(rows.len(), 256);
    let entry = |v: &Value| Requirement {
        min: Version::parse(v["min"].as_str().unwrap()).unwrap(),
        latest: Version::parse(v["latest"].as_str().unwrap()).unwrap(),
        grace_until: v["grace_until"].as_str().map(|t| parse_utc(t).unwrap()),
        grace_min: v["grace_min"].as_str().map(|g| Version::parse(g).unwrap()),
    };
    let mut both = (0, 0);
    for row in &rows {
        let covers = trust::covers(&entry(&row["new"]), &entry(&row["old"]));
        assert_eq!(row["covers"].as_bool().unwrap(), covers, "{row}");
        if covers {
            both.0 += 1;
        } else {
            both.1 += 1;
        }
    }
    assert!(both.0 > 50 && both.1 > 50, "{both:?}");
}

#[test]
fn the_key_file_of_the_vectors() {
    let text = fs::read_to_string(dir().join("update-keys.env")).unwrap();
    let keys = KeySet::parse(&text).unwrap();
    assert!(keys.has_test_key());
    assert_eq!(
        keys.signing.iter().map(|k| k.id).collect::<Vec<_>>(),
        [1, 2]
    );
    assert_eq!(
        keys.roots.iter().map(|k| k.id).collect::<Vec<_>>(),
        [101, 102]
    );
    assert_eq!(keys.to_text(), test_key_set().to_text());
    let manifest = manifest();
    assert_eq!(
        hex(&keys.fingerprint(SERVER)),
        manifest["key_file"]["fingerprint_sha256"].as_str().unwrap()
    );
    // every test key of the manifest is the tool's, with the same seed and number
    for key in manifest["test_keys"].as_array().unwrap() {
        let ours = test_key(key["name"].as_str().unwrap()).unwrap();
        assert_eq!(hex(&ours.seed()), key["seed_hex"].as_str().unwrap());
        assert_eq!(ours.public().to_string(), key["public"].as_str().unwrap());
        assert_eq!(
            keys.pins(ours.role, &ours.public()),
            key["in_the_key_file"].as_bool().unwrap()
        );
    }
    // the declaration of epoch 2 by itself
    let root = fs::read_to_string(dir().join("root-declaration-e2.json")).unwrap();
    let part = Part::parse(root.trim_end(), "t").unwrap();
    assert!(part.verifies(&keys.root(101).unwrap().key, ROOT_DOMAIN));
    let decl = RootDecl::parse(&part.p).unwrap();
    assert_eq!((decl.epoch, decl.revoked.clone()), (2, vec![1]));
    assert_eq!(decl.base.len(), 4);
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

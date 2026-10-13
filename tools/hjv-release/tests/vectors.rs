// SPDX-License-Identifier: AGPL-3.0-only

//! The test vectors of the signed version policy (tests/vectors/, described in its README.md):
//! envelopes an app must accept and envelopes it must refuse, each with the envelopes it has
//! taken before. The vectors are made here from the test keys, kept as files and checked
//! against the trust rules (src/trust.rs). The client's implementation reads the same files.
//!
//!     HJV_RELEASE_WRITE_VECTORS=1 cargo test --test vectors
//!
//! writes the files again; without the variable the test fails when a file differs from what
//! this code makes, so the files and the code cannot drift apart.

use ed25519_dalek::SigningKey;
use hjv_release::{
    envelope::{Envelope, Part, MAX_ENVELOPE_BYTES, POLICY_DOMAIN, ROOT_DOMAIN},
    keyfile::SecretKey,
    keys::{self, test_key, test_key_set, Floor, Role, TEST_KEYS},
    policy::{Entry, Package, Platform, Policy, RootDecl},
    time::parse_utc,
    trust::{check_envelope, hex, sha256, State},
    version::Version,
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

const SERVER: &str = "rd.example.com";
const KEYFILE_PASSPHRASE: &str = "test passphrase, not a secret";

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors")
}

fn v(s: &str) -> Version {
    Version::parse(s).unwrap()
}

fn t(s: &str) -> i64 {
    parse_utc(s).unwrap()
}

fn key(name: &str) -> (SigningKey, u32) {
    let key = test_key(name).unwrap();
    (SigningKey::from_bytes(&key.seed()), key.id)
}

/// `text` signed by the test key `by`, under its own number.
fn signed(text: &str, by: &str, domain: &[u8]) -> Part {
    let (key, k) = key(by);
    Part::sign(&key, k, domain, text)
}

/// The same, under another key's number.
fn signed_as(text: &str, by: &str, k: u32, domain: &[u8]) -> Part {
    Part::sign(&key(by).0, k, domain, text)
}

fn file(policy: Part, root: Option<Part>) -> String {
    Envelope { policy, root }.to_file()
}

/// A policy signed by `by`, with a root declaration signed by root key 101 if one is given.
fn envelope(policy: &Policy, by: &str, root: Option<&RootDecl>) -> String {
    file(
        signed(&policy.to_text(), by, POLICY_DOMAIN),
        root.map(|decl| signed(&decl.to_text(), "root-101", ROOT_DOMAIN)),
    )
}

fn package(platform: Platform, version: &str, build: &str, size: u64) -> Package {
    let suffix = match platform {
        Platform::Windows => "windows-x86_64.exe",
        _ => "android-arm64.apk",
    };
    Package {
        file: format!("HJViewer-{version}-{suffix}"),
        size,
        sha256: hex(&sha256(
            format!("HJViewer test package {platform} {version}").as_bytes(),
        )),
        build: build.to_owned(),
    }
}

/// seq 1: 1.6.0 is out, nothing is enforced.
fn p1() -> Policy {
    let entry = |os_min: &str| Entry {
        os_min: Some(os_min.to_owned()),
        source: Some("client-1.6.0".to_owned()),
        ..Entry::new(v("1.5.0"), v("1.6.0"))
    };
    let mut platforms = BTreeMap::new();
    platforms.insert(
        Platform::Windows,
        Entry {
            package: Some(package(Platform::Windows, "1.6.0", "1.6.0.37", 25_329_664)),
            ..entry("10")
        },
    );
    platforms.insert(
        Platform::Android,
        Entry {
            package: Some(package(Platform::Android, "1.6.0", "262742100", 28_717_196)),
            ..entry("22")
        },
    );
    platforms.insert(
        Platform::Ios,
        Entry {
            testflight_build: Some("262740100".to_owned()),
            ..entry("13.0")
        },
    );
    Policy {
        epoch: 1,
        seq: 1,
        issued: t("2026-10-01T03:00:00Z"),
        server: SERVER.to_owned(),
        contact_ko: Some("설치 파일은 관리자에게 받으세요.".to_owned()),
        platforms,
    }
}

const GRACE: &str = "2026-10-16T15:00:00Z";

/// seq 2: every min to 1.6.0, 14 days of grace.
fn p2() -> Policy {
    let mut p = p1();
    p.seq = 2;
    p.issued = t("2026-10-02T03:00:00Z");
    for entry in p.platforms.values_mut() {
        entry.min = v("1.6.0");
        entry.grace_until = Some(t(GRACE));
    }
    p
}

/// seq 3: 1.6.1 for Windows and Android, unattended installs, the Google Play entry.
fn p3() -> Policy {
    let mut p = p2();
    p.seq = 3;
    p.issued = t("2026-10-05T03:00:00Z");
    let notes = "연결이 끊긴 뒤 다시 붙지 않던 문제를 고쳤습니다.\n한글 입력을 고쳤습니다.";
    let windows = p.platforms.get_mut(&Platform::Windows).unwrap();
    windows.latest = v("1.6.1");
    windows.notes_ko = Some(notes.to_owned());
    windows.source = Some("client-1.6.1".to_owned());
    windows.package = Some(package(Platform::Windows, "1.6.1", "1.6.1.41", 25_400_320));
    windows.auto_install_after = Some(t("2026-10-07T03:00:00Z"));
    let android = p.platforms.get_mut(&Platform::Android).unwrap();
    android.latest = v("1.6.1");
    android.notes_ko = Some(notes.to_owned());
    android.source = Some("client-1.6.1".to_owned());
    android.package = Some(package(Platform::Android, "1.6.1", "262782100", 28_800_004));
    android.move_to_play = true;
    p.platforms.insert(
        Platform::AndroidPlay,
        Entry {
            source: Some("client-1.6.1".to_owned()),
            version_code: Some(262_782_100),
            ..Entry::new(v("1.6.0"), v("1.6.1"))
        },
    );
    p
}

/// Signing key 1 leaked: the root key revokes it and starts epoch 2.
fn r2() -> RootDecl {
    RootDecl {
        server: SERVER.to_owned(),
        epoch: 2,
        revoked: vec![1],
        issued: t("2026-10-06T03:00:00Z"),
    }
}

fn p4() -> Policy {
    Policy {
        epoch: 2,
        seq: 1,
        issued: t("2026-10-06T03:00:00Z"),
        ..p3()
    }
}

/// The emergency stop: no unattended installs.
fn p5() -> Policy {
    let mut p = p4();
    p.seq = 2;
    p.issued = t("2026-10-07T03:00:00Z");
    p.platforms
        .get_mut(&Platform::Windows)
        .unwrap()
        .auto_install_after = None;
    p
}

/// The update path is down: the root key lowers every min for a while.
fn p6() -> Policy {
    let mut p = p5();
    p.seq = 3;
    p.issued = t("2026-10-08T03:00:00Z");
    for entry in p.platforms.values_mut() {
        entry.min = v("1.5.0");
        entry.grace_until = None;
    }
    p
}

/// Afterwards a signing key raises them again.
fn p7() -> Policy {
    let mut p = p6();
    p.seq = 4;
    p.issued = t("2026-10-09T03:00:00Z");
    for (platform, entry) in p.platforms.iter_mut() {
        entry.min = if *platform == Platform::Ios {
            v("1.6.0")
        } else {
            v("1.6.1")
        };
        entry.grace_until = Some(t("2026-10-23T15:00:00Z"));
    }
    p
}

/// The other way back from the relaxation: every min as it was before it (e2-s2), with the
/// deadlines of then. An app that was off while the relaxation was served takes this one too.
fn p7_as_before() -> Policy {
    Policy {
        seq: 4,
        issued: t("2026-10-09T03:00:00Z"),
        ..p5()
    }
}

/// A signature of the test key `by` over `text` whose R is the identity point: with
/// s = k * a (k the hash of R, the public key and the message, a the secret scalar) the plain
/// equation [s]B = R + [k]A holds, and a strict check refuses the small-order R all the same.
fn signed_with_identity_r(text: &str, by: &str, domain: &[u8]) -> Part {
    use curve25519_dalek::{edwards::EdwardsPoint, scalar::Scalar, traits::Identity};
    use sha2::{Digest, Sha512};
    let key = test_key(by).unwrap();
    let public = key.public().key;
    let mut a = [0u8; 32];
    a.copy_from_slice(&Sha512::digest(key.seed())[..32]);
    a[0] &= 248;
    a[31] &= 127;
    a[31] |= 64;
    let a = Scalar::from_bytes_mod_order(a);
    let r = EdwardsPoint::identity().compress().to_bytes();
    let message = hjv_release::envelope::message(domain, text);
    let k = Scalar::from_hash(
        Sha512::new()
            .chain_update(r)
            .chain_update(public.to_bytes())
            .chain_update(&message),
    );
    let mut s = [0u8; 64];
    s[..32].copy_from_slice(&r);
    s[32..].copy_from_slice((k * a).as_bytes());
    // the equation does hold: a check that is not strict takes this signature
    {
        use ed25519_dalek::Verifier;
        assert!(public
            .verify(&message, &ed25519_dalek::Signature::from_bytes(&s))
            .is_ok());
    }
    Part {
        k: key.id,
        p: text.to_owned(),
        s,
    }
}

/// The first policy with notes that make its envelope file exactly `bytes` long.
fn first_policy_of_size(bytes: usize) -> String {
    let filled = |korean: usize, ascii: usize| {
        let mut p = p1();
        for platform in [Platform::Windows, Platform::Android] {
            p.platforms.get_mut(&platform).unwrap().notes_ko = Some("가".repeat(2000));
        }
        p.platforms.get_mut(&Platform::Ios).unwrap().notes_ko =
            Some(format!("{}{}", "가".repeat(korean), "a".repeat(ascii)));
        envelope(&p, "signing-1", None)
    };
    // "가" is three bytes in the file and "a" is one
    let need = bytes - (filled(1, 0).len() - 3);
    let korean = need / 3 - 1;
    let file = filled(korean, need - 3 * korean);
    assert_eq!(file.len(), bytes);
    file
}

struct Case {
    name: &'static str,
    /// Where the envelope is, relative to the vectors directory.
    path: String,
    accept: bool,
    content: String,
    /// The envelopes the app has taken before, in order.
    after: Vec<&'static str>,
    /// The build's floor when it is not the key file's.
    floor: Option<&'static str>,
    /// For a refused envelope: one that the same app must still take afterwards.
    then_accept: Option<&'static str>,
    /// Which rule decides.
    rule: &'static str,
    /// "app": the design says what an app does. "tool": only this tool is that strict.
    binding: &'static str,
    why: &'static str,
}

const MAIN: [&str; 7] = [
    "e1-s1",
    "e1-s2",
    "e1-s3",
    "e2-s1",
    "e2-s2",
    "e2-s3-root-relax",
    "e2-s4",
];

fn accepted(
    name: &'static str,
    content: String,
    after: &[&'static str],
    rule: &'static str,
    why: &'static str,
) -> Case {
    Case {
        name,
        path: format!("accepted/{name}.json"),
        accept: true,
        content,
        after: after.to_vec(),
        floor: None,
        then_accept: None,
        rule,
        binding: "app",
        why,
    }
}

fn refused(
    name: &'static str,
    content: String,
    after: &[&'static str],
    rule: &'static str,
    why: &'static str,
) -> Case {
    Case {
        name,
        path: format!("refused/{name}.json"),
        accept: false,
        content,
        after: after.to_vec(),
        floor: None,
        then_accept: None,
        rule,
        binding: "app",
        why,
    }
}

/// `text` with the first `from` replaced; the replacement must change something.
fn replaced(text: &str, from: &str, to: &str) -> String {
    assert!(text.contains(from), "{from:?} is not in the text");
    text.replacen(from, to, 1)
}

fn windows(p: &mut Policy) -> &mut Entry {
    p.platforms.get_mut(&Platform::Windows).unwrap()
}

fn cases() -> Vec<Case> {
    let mut cases = Vec::new();
    let policy_by = |p: &Policy, by: &str| envelope(p, by, None);
    let p1_text = p1().to_text();

    // ---- the history every other case starts from
    cases.push(accepted(
        "e1-s1",
        policy_by(&p1(), "signing-1"),
        &[],
        "first",
        "the first policy, by signing key 1: 1.6.0 is out, nothing is enforced",
    ));
    cases.push(accepted(
        "e1-s2",
        policy_by(&p2(), "signing-1"),
        &MAIN[..1],
        "seq",
        "every min raised to 1.6.0 with 14 days of grace",
    ));
    cases.push(accepted("e1-s3", policy_by(&p3(), "signing-2"), &MAIN[..2], "seq", "by signing key 2: 1.6.1 with packages, notes, an unattended install date, the Google Play entry"));
    cases.push(accepted(
        "e2-s1",
        envelope(&p4(), "signing-2", Some(&r2())),
        &MAIN[..3],
        "root-declaration",
        "the root key revokes signing key 1 and starts epoch 2; seq starts again at 1",
    ));
    cases.push(accepted("e2-s2", envelope(&p5(), "signing-2", Some(&r2())), &MAIN[..4], "seq", "the same declaration goes along; auto_install_after is removed (the emergency stop is no relaxation)"));
    cases.push(accepted(
        "e2-s3-root-relax",
        envelope(&p6(), "root-101", Some(&r2())),
        &MAIN[..5],
        "relaxation",
        "the root key signs a policy that lowers every min and drops the deadlines",
    ));
    cases.push(accepted(
        "e2-s4",
        envelope(&p7(), "signing-2", Some(&r2())),
        &MAIN[..6],
        "seq",
        "a signing key raises min again after the root key's relaxation",
    ));

    // ---- accepted side cases
    let again = |name, path: &str, after: &[&'static str], rule, why| Case {
        path: path.to_owned(),
        ..accepted(name, String::new(), after, rule, why)
    };
    cases.push(again(
        "e1-s2-again",
        "accepted/e1-s2.json",
        &MAIN[..2],
        "seq",
        "the same position with the same bytes: the app keeps what it has",
    ));
    cases.push(again("fresh-install-e2-s2", "accepted/e2-s2.json", &[], "root-declaration", "a fresh install takes the current policy and learns the epoch and the revocation from the declaration it carries"));
    cases.push(Case {
        floor: Some("2:2"),
        ..again(
            "floor-exact",
            "accepted/e2-s2.json",
            &[],
            "floor",
            "a build whose floor is 2:2 takes the policy at 2:2",
        )
    });
    let at = |seq: u64| Policy {
        seq,
        issued: t("2026-10-03T03:00:00Z"),
        ..p1()
    };
    cases.push(accepted(
        "seq-step-1000",
        policy_by(&at(1001), "signing-1"),
        &MAIN[..1],
        "seq",
        "a signing key's policy 1000 above the stored seq",
    ));
    cases.push(accepted(
        "fresh-install-seq-1000",
        policy_by(&at(1000), "signing-1"),
        &[],
        "seq",
        "a fresh install with floor 1:0 takes seq 1000",
    ));
    cases.push(accepted(
        "root-seq-jump",
        policy_by(&at(5000), "root-101"),
        &MAIN[..1],
        "seq",
        "the root key's policy may be any distance above the stored seq",
    ));
    let epoch2 = Policy {
        epoch: 2,
        seq: 1,
        issued: t("2026-10-03T03:00:00Z"),
        ..p1()
    };
    cases.push(accepted(
        "root-policy-raises-epoch",
        policy_by(&epoch2, "root-101"),
        &MAIN[..1],
        "epoch",
        "a policy signed by the root key may carry a higher epoch without a declaration",
    ));
    let revoke2 = RootDecl {
        epoch: 1,
        revoked: vec![2],
        issued: t("2026-10-03T03:00:00Z"),
        ..r2()
    };
    let p2_next = Policy {
        seq: 3,
        issued: t("2026-10-03T03:00:00Z"),
        ..p2()
    };
    cases.push(accepted(
        "revoke-in-the-same-epoch",
        envelope(&p2_next, "signing-1", Some(&revoke2)),
        &MAIN[..2],
        "root-declaration",
        "a declaration may revoke a key without a new epoch",
    ));

    // ---- what a signing key may not relax, and the root key may
    let mut relax = |name: &'static str,
                     root_name: &'static str,
                     after: &[&'static str],
                     policy: Policy,
                     why: &'static str| {
        cases.push(refused(
            name,
            policy_by(&policy, "signing-1"),
            after,
            "relaxation",
            why,
        ));
        cases.push(accepted(
            root_name,
            policy_by(&policy, "root-101"),
            after,
            "relaxation",
            "the same policy signed by the root key",
        ));
    };
    let mut p = p2_next.clone();
    p.platforms.get_mut(&Platform::Ios).unwrap().min = v("1.5.0");
    relax(
        "relax-min-lowered",
        "relax-min-lowered-root",
        &MAIN[..2],
        p,
        "a signing key lowers a min",
    );
    let mut p = p2_next.clone();
    windows(&mut p).grace_until = Some(t(GRACE) + 86_400);
    relax(
        "relax-grace-later",
        "relax-grace-later-root",
        &MAIN[..2],
        p,
        "a signing key delays the deadline of an unchanged min",
    );
    let mut p = Policy {
        seq: 2,
        issued: t("2026-10-03T03:00:00Z"),
        ..p1()
    };
    windows(&mut p).grace_until = Some(t(GRACE));
    relax(
        "relax-grace-added",
        "relax-grace-added-root",
        &MAIN[..1],
        p,
        "a signing key gives an unchanged min a deadline it did not have",
    );
    let mut p = Policy {
        seq: 4,
        issued: t("2026-10-06T03:00:00Z"),
        ..p3()
    };
    let old = p2().platforms[&Platform::Windows].clone();
    *windows(&mut p) = Entry {
        auto_install_after: None,
        ..old
    };
    relax(
        "relax-latest-lowered",
        "relax-latest-lowered-root",
        &MAIN[..3],
        p,
        "a signing key lowers latest",
    );
    let mut p = p2_next.clone();
    p.platforms.remove(&Platform::Ios);
    relax(
        "relax-entry-removed",
        "relax-entry-removed-root",
        &MAIN[..2],
        p,
        "a signing key removes an entry",
    );

    // ---- what is no relaxation: a signing key may do it
    let mut p = Policy {
        seq: 4,
        issued: t("2026-10-06T03:00:00Z"),
        ..p3()
    };
    windows(&mut p).auto_install_after = None;
    cases.push(accepted(
        "stop-auto-install",
        policy_by(&p, "signing-1"),
        &MAIN[..3],
        "relaxation",
        "removing auto_install_after is the emergency stop, not a relaxation",
    ));
    windows(&mut p).auto_install_after = Some(t("2026-11-07T03:00:00Z"));
    cases.push(accepted(
        "delay-auto-install",
        policy_by(&p, "signing-1"),
        &MAIN[..3],
        "relaxation",
        "delaying auto_install_after is not a relaxation",
    ));
    let mut p = Policy {
        seq: 4,
        issued: t("2026-10-06T03:00:00Z"),
        ..p3()
    };
    windows(&mut p).min = v("1.6.1");
    windows(&mut p).grace_until = Some(t("2026-11-30T15:00:00Z"));
    cases.push(accepted(
        "min-raised-with-new-grace",
        policy_by(&p, "signing-1"),
        &MAIN[..3],
        "relaxation",
        "a higher min may come with a new, later deadline",
    ));
    let mut p = p2_next.clone();
    for entry in p.platforms.values_mut() {
        entry.grace_until = Some(t(GRACE) - 2 * 86_400);
    }
    cases.push(accepted(
        "grace-earlier",
        policy_by(&p, "signing-1"),
        &MAIN[..2],
        "relaxation",
        "an earlier deadline tightens",
    ));
    let mut p = p2_next.clone();
    windows(&mut p).grace_until = None;
    cases.push(accepted(
        "grace-removed",
        policy_by(&p, "signing-1"),
        &MAIN[..2],
        "relaxation",
        "no deadline at all tightens",
    ));

    // ---- signatures and keys
    let e1_s1 = policy_by(&p1(), "signing-1");
    cases.push(refused(
        "tampered-policy",
        replaced(&e1_s1, "\\\"seq\\\":1,", "\\\"seq\\\":2,"),
        &[],
        "signature",
        "one byte of the policy text changed after signing",
    ));
    let s = &Envelope::parse(&e1_s1).unwrap().to_text();
    let at_signature = s.find("\"s\":\"").unwrap() + 5;
    let flipped = if &s[at_signature..at_signature + 1] == "A" {
        "B"
    } else {
        "A"
    };
    let mut tampered = s.clone();
    tampered.replace_range(at_signature..at_signature + 1, flipped);
    cases.push(refused(
        "tampered-signature",
        format!("{tampered}\n"),
        &[],
        "signature",
        "one character of the signature changed",
    ));
    cases.push(refused(
        "wake-domain",
        file(signed(&p1_text, "signing-1", b"hjv-wake-v1"), None),
        &[],
        "signature",
        "signed under the relay's wake-command domain hjv-wake-v1",
    ));
    cases.push(refused(
        "policy-under-root-domain",
        file(signed(&p1_text, "signing-1", ROOT_DOMAIN), None),
        &[],
        "signature",
        "the policy signed under the root declaration's domain",
    ));
    cases.push(refused(
        "unknown-key",
        policy_by(&p1(), "signing-3"),
        &[],
        "key",
        "signed by a key the build does not hold (k = 3)",
    ));
    cases.push(refused(
        "another-key-as-1",
        file(signed_as(&p1_text, "signing-3", 1, POLICY_DOMAIN), None),
        &[],
        "signature",
        "signed by another key under the number 1",
    ));
    cases.push(refused(
        "another-root-as-101",
        file(signed_as(&p1_text, "root-102", 101, POLICY_DOMAIN), None),
        &[],
        "signature",
        "signed by another root key under the number 101",
    ));
    cases.push(refused(
        "signature-with-identity-r",
        file(
            signed_with_identity_r(&p1_text, "signing-1", POLICY_DOMAIN),
            None,
        ),
        &[],
        "signature",
        "a signature whose R is the identity point: the equation of Ed25519 holds for it, and a strict check (libsodium, verify_strict) refuses a small-order R",
    ));

    // ---- envelope, version, server
    cases.push(refused(
        "envelope-v2",
        replaced(&e1_s1, "{\"v\":1,\"k\"", "{\"v\":2,\"k\""),
        &[],
        "version",
        "an envelope version the app does not know",
    ));
    let p_v2 = replaced(&p1_text, "{\"v\":1,\"epoch\"", "{\"v\":2,\"epoch\"");
    cases.push(refused(
        "policy-v2",
        file(signed(&p_v2, "signing-1", POLICY_DOMAIN), None),
        &[],
        "version",
        "a correctly signed policy of version 2: the app ignores it",
    ));
    let other = Policy {
        server: "other.example.com".to_owned(),
        ..p1()
    };
    cases.push(refused(
        "wrong-server",
        policy_by(&other, "signing-1"),
        &[],
        "server",
        "a correctly signed policy of another server",
    ));
    // the envelope's own fields: a key number 1-65535 as a JSON integer, a signature of 64 bytes
    cases.push(refused(
        "key-number-zero",
        replaced(&e1_s1, "{\"v\":1,\"k\":1,", "{\"v\":1,\"k\":0,"),
        &[],
        "envelope",
        "the key number 0",
    ));
    cases.push(refused(
        "key-number-as-text",
        replaced(&e1_s1, "{\"v\":1,\"k\":1,", "{\"v\":1,\"k\":\"1\","),
        &[],
        "envelope",
        "the key number as a JSON string",
    ));
    let short = {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let part = signed(&p1_text, "signing-1", POLICY_DOMAIN);
        replaced(
            &e1_s1,
            &STANDARD.encode(part.s),
            &STANDARD.encode(&part.s[..63]),
        )
    };
    cases.push(refused(
        "signature-63-bytes",
        short,
        &[],
        "envelope",
        "a signature of 63 bytes",
    ));
    let mut big = p1();
    for entry in big.platforms.values_mut() {
        entry.notes_ko = Some("가".repeat(2000));
    }
    let big = policy_by(&big, "signing-1");
    assert!(big.len() > MAX_ENVELOPE_BYTES);
    cases.push(refused(
        "too-large",
        big,
        &[],
        "size",
        "a correctly signed envelope over 16 KiB: the app does not read that much",
    ));
    cases.push(accepted(
        "size-at-the-limit",
        first_policy_of_size(MAX_ENVELOPE_BYTES),
        &[],
        "size",
        "an envelope file of exactly 16384 bytes, its line feed counted",
    ));
    cases.push(refused(
        "size-one-over",
        first_policy_of_size(MAX_ENVELOPE_BYTES + 1),
        &[],
        "size",
        "one byte more: 16385 bytes",
    ));

    // ---- root declarations
    let p4_part = || signed(&p4().to_text(), "signing-2", POLICY_DOMAIN);
    let r2_text = r2().to_text();
    cases.push(refused(
        "declaration-by-signing-key",
        file(p4_part(), Some(signed(&r2_text, "signing-2", ROOT_DOMAIN))),
        &[],
        "root-declaration",
        "the declaration is signed by signing key 2 under its own number",
    ));
    cases.push(refused(
        "declaration-signing-key-as-101",
        file(
            p4_part(),
            Some(signed_as(&r2_text, "signing-2", 101, ROOT_DOMAIN)),
        ),
        &[],
        "root-declaration",
        "the declaration is signed by signing key 2 under the root key's number",
    ));
    cases.push(refused(
        "declaration-under-policy-domain",
        file(p4_part(), Some(signed(&r2_text, "root-101", POLICY_DOMAIN))),
        &[],
        "root-declaration",
        "the declaration is signed by the root key under the policy's domain",
    ));
    let good = envelope(&p4(), "signing-2", Some(&r2()));
    cases.push(refused(
        "declaration-tampered",
        replaced(&good, "\\\"revoked\\\":[1]", "\\\"revoked\\\":[3]"),
        &[],
        "root-declaration",
        "the declaration's text changed after signing",
    ));
    let revokes_root = RootDecl {
        revoked: vec![1, 101],
        ..r2()
    };
    cases.push(refused(
        "declaration-revokes-root",
        envelope(&p4(), "signing-2", Some(&revokes_root)),
        &[],
        "root-declaration",
        "the declaration lists the root key's number as revoked",
    ));
    let other_server = RootDecl {
        server: "other.example.com".to_owned(),
        ..r2()
    };
    cases.push(refused(
        "declaration-wrong-server",
        envelope(&p4(), "signing-2", Some(&other_server)),
        &[],
        "root-declaration",
        "a declaration of another server",
    ));
    let decl_v2 = replaced(&r2_text, "{\"v\":1,", "{\"v\":2,");
    cases.push(refused(
        "declaration-v2",
        file(p4_part(), Some(signed(&decl_v2, "root-101", ROOT_DOMAIN))),
        &[],
        "root-declaration",
        "a declaration of version 2",
    ));
    cases.push(refused(
        "revoked-key-with-declaration",
        envelope(&p4(), "signing-1", Some(&r2())),
        &[],
        "revoked",
        "the policy is signed by the key its own declaration revokes",
    ));
    let p_2_2 = Policy {
        seq: 2,
        issued: t("2026-10-07T03:00:00Z"),
        ..p4()
    };
    cases.push(refused(
        "revoked-key-later",
        policy_by(&p_2_2, "signing-1"),
        &MAIN[..4],
        "revoked",
        "signing key 1 stays revoked for an envelope that carries no declaration",
    ));
    let back = Policy {
        seq: 4,
        issued: t("2026-10-07T03:00:00Z"),
        ..p3()
    };
    cases.push(refused(
        "old-epoch-after-new",
        policy_by(&back, "signing-2"),
        &MAIN[..4],
        "epoch",
        "a policy of epoch 1 after the app has moved to epoch 2",
    ));
    let r1 = RootDecl {
        epoch: 1,
        revoked: Vec::new(),
        ..r2()
    };
    cases.push(refused(
        "declaration-epoch-back",
        envelope(&back, "signing-2", Some(&r1)),
        &MAIN[..4],
        "epoch",
        "a declaration of epoch 1 after the app has moved to epoch 2",
    ));
    cases.push(refused(
        "signing-key-raises-epoch",
        policy_by(&p4(), "signing-2"),
        &MAIN[..1],
        "epoch",
        "a signing key's policy names epoch 2 without a declaration",
    ));
    let epoch3 = Policy { epoch: 3, ..p4() };
    cases.push(refused(
        "epoch-differs-from-declaration",
        envelope(&epoch3, "signing-2", Some(&r2())),
        &[],
        "epoch",
        "a signing key's policy of epoch 3 with a declaration of epoch 2",
    ));
    cases.push(Case {
        then_accept: Some("e1-s2"),
        ..refused(
            "good-declaration-bad-policy",
            file(signed_as(&p4().to_text(), "signing-3", 2, POLICY_DOMAIN), Some(signed(&r2_text, "root-101", ROOT_DOMAIN))),
            &MAIN[..1],
            "signature",
            "a valid declaration with a policy whose signature fails: nothing of the envelope is taken, so signing key 1 is still good",
        )
    });
    let after_revoke2: [&'static str; 3] = ["e1-s1", "e1-s2", "revoke-in-the-same-epoch"];
    let by_2 = Policy {
        seq: 4,
        issued: t("2026-10-04T03:00:00Z"),
        ..p2()
    };
    cases.push(refused(
        "revoked-in-the-same-epoch",
        policy_by(&by_2, "signing-2"),
        &after_revoke2,
        "revoked",
        "signing key 2 after a declaration of the same epoch revoked it",
    ));

    // ---- after a policy of the root key
    cases.push(Case {
        path: "accepted/root-seq-jump.json".to_owned(),
        ..refused(
            "root-policy-of-an-old-epoch",
            String::new(),
            &MAIN[..4],
            "epoch",
            "a policy the root key signed in epoch 1, sent again after the app has moved to epoch 2: the root key does not go back an epoch either, however high the seq",
        )
    });
    cases.push(Case {
        path: "accepted/e2-s4.json".to_owned(),
        ..refused(
            "e2-s4-missed-the-relaxation",
            String::new(),
            &MAIN[..5],
            "relaxation",
            "an app that was off while the root key's relaxation e2-s3 was served still holds e2-s2: to it e2-s4 puts off the deadline of a min it knows (ios), so it refuses e2-s4 and every later policy of a signing key that builds on it",
        )
    });
    cases.push(accepted(
        "restore-with-the-old-deadline",
        envelope(&p7_as_before(), "signing-2", Some(&r2())),
        &MAIN[..6],
        "relaxation",
        "the other way back from the relaxation: every min as before it, with the deadlines of then",
    ));
    cases.push(Case {
        path: "accepted/restore-with-the-old-deadline.json".to_owned(),
        ..accepted(
            "restore-with-the-old-deadline-missed-the-relaxation",
            String::new(),
            &MAIN[..5],
            "relaxation",
            "the same envelope for an app that still holds e2-s2: nothing is relaxed, so it follows",
        )
    });
    let mut low_entry = Policy {
        seq: 3,
        issued: t("2026-10-03T03:00:00Z"),
        ..p2()
    };
    low_entry.platforms.insert(
        Platform::AndroidPlay,
        Entry {
            source: Some("client-1.6.0".to_owned()),
            version_code: Some(262_742_100),
            ..Entry::new(v("1.0.0"), v("1.6.0"))
        },
    );
    cases.push(accepted(
        "new-entry-below-the-minimums",
        policy_by(&low_entry, "signing-1"),
        &MAIN[..2],
        "relaxation",
        "an entry the app did not have, with a min below every other: the app compares entry by entry, so this is no relaxation to it (the release tool asks the root key for such a policy all the same)",
    ));

    // ---- seq and floor
    cases.push(Case {
        path: "accepted/e1-s1.json".to_owned(),
        ..refused(
            "seq-lower",
            String::new(),
            &MAIN[..2],
            "seq",
            "an older policy after a newer one (rollback)",
        )
    });
    let other_bytes = Policy {
        contact_ko: None,
        ..p1()
    };
    cases.push(refused(
        "same-position-other-bytes",
        policy_by(&other_bytes, "signing-1"),
        &MAIN[..1],
        "seq",
        "another policy at the position the app already has",
    ));
    cases.push(refused(
        "seq-step-1001",
        policy_by(&at(1002), "signing-1"),
        &MAIN[..1],
        "seq",
        "a signing key's policy 1001 above the stored seq",
    ));
    cases.push(refused(
        "fresh-install-seq-1001",
        policy_by(&at(1001), "signing-1"),
        &[],
        "seq",
        "a fresh install with floor 1:0 does not take a signing key's seq 1001",
    ));
    cases.push(Case {
        floor: Some("1:5"),
        ..refused(
            "below-floor",
            policy_by(&at(4), "signing-1"),
            &[],
            "floor",
            "a policy at 1:4 for a build whose floor is 1:5",
        )
    });
    cases.push(Case {
        floor: Some("1:5"),
        ..refused(
            "below-floor-by-root",
            policy_by(&at(4), "root-101"),
            &[],
            "floor",
            "the same by the root key: the floor holds for every key",
        )
    });
    cases.push(Case {
        path: "accepted/e2-s1.json".to_owned(),
        floor: Some("2:2"),
        ..refused(
            "below-floor-in-epoch-2",
            String::new(),
            &[],
            "floor",
            "the policy at 2:1 for a build whose floor is 2:2",
        )
    });

    // ---- field rules an app checks (design 6-2, step 7)
    let field = |cases: &mut Vec<Case>, name: &'static str, text: String, why: &'static str| {
        cases.push(refused(
            name,
            file(signed(&text, "signing-1", POLICY_DOMAIN), None),
            &[],
            "fields",
            why,
        ));
    };
    let mut p = p1();
    windows(&mut p).min = v("1.6.1");
    field(
        &mut cases,
        "min-above-latest",
        p.to_text(),
        "min 1.6.1 above latest 1.6.0",
    );
    field(
        &mut cases,
        "version-two-numbers",
        replaced(&p1_text, "\"min\":\"1.5.0\"", "\"min\":\"1.6\""),
        "\"1.6\": the apps' version comparison reads it as 1.0.60",
    );
    field(
        &mut cases,
        "version-v-prefix",
        replaced(&p1_text, "\"min\":\"1.5.0\"", "\"min\":\"v1.6.0\""),
        "\"v1.6.0\": read as 0.6.0",
    );
    field(
        &mut cases,
        "version-three-digit-patch",
        replaced(&p1_text, "\"latest\":\"1.6.0\"", "\"latest\":\"1.5.100\""),
        "\"1.5.100\": read as 1.6.0",
    );
    field(
        &mut cases,
        "version-four-numbers",
        replaced(&p1_text, "\"latest\":\"1.6.0\"", "\"latest\":\"1.6.0.1\""),
        "\"1.6.0.1\": four numbers",
    );
    field(
        &mut cases,
        "version-plus-build",
        replaced(&p1_text, "\"latest\":\"1.6.0\"", "\"latest\":\"1.6.0+1\""),
        "\"1.6.0+1\": read as 1.6.0 with the build dropped, or as 1.5.x",
    );
    field(
        &mut cases,
        "issued-not-rfc3339",
        replaced(
            &p1_text,
            "\"issued\":\"2026-10-01T03:00:00Z\"",
            "\"issued\":\"2026-10-01 03:00:00\"",
        ),
        "issued is not of the form 2026-10-01T03:00:00Z",
    );
    let with_package = |change: &dyn Fn(&mut Package)| {
        let mut p = p1();
        change(windows(&mut p).package.as_mut().unwrap());
        p.to_text()
    };
    field(
        &mut cases,
        "package-file-install-exe",
        with_package(&|p| p.file = "HJViewer-1.6.0-install.exe".to_owned()),
        "a package name ending in install.exe would open the installer",
    );
    field(
        &mut cases,
        "package-file-qs",
        with_package(&|p| p.file = "HJViewer-qs-1.6.0.exe".to_owned()),
        "a package name with -qs- would run as quick support",
    );
    field(
        &mut cases,
        "package-file-path",
        with_package(&|p| p.file = "../HJViewer-1.6.0.exe".to_owned()),
        "a package name with a path",
    );
    field(
        &mut cases,
        "package-size-zero",
        with_package(&|p| p.size = 0),
        "a package of 0 bytes",
    );
    field(
        &mut cases,
        "package-size-over-200-mib",
        with_package(&|p| p.size = 200 * 1024 * 1024 + 1),
        "a package over 200 MiB",
    );
    field(
        &mut cases,
        "package-sha256-uppercase",
        with_package(&|p| p.sha256 = p.sha256.to_uppercase()),
        "a SHA-256 that is not 64 lowercase hex digits",
    );
    let long = Policy {
        contact_ko: Some("가".repeat(201)),
        ..p1()
    };
    field(
        &mut cases,
        "contact-201-characters",
        long.to_text(),
        "contact_ko over 200 characters",
    );
    let mut p = p1();
    windows(&mut p).notes_ko = Some("가".repeat(2001));
    field(
        &mut cases,
        "notes-2001-characters",
        p.to_text(),
        "notes_ko over 2000 characters",
    );

    // ---- stricter than the design asks of an app: this tool reads only what it writes
    let mut tool = |name: &'static str, text: String, why: &'static str| {
        cases.push(Case {
            binding: "tool",
            ..refused(
                name,
                file(signed(&text, "signing-1", POLICY_DOMAIN), None),
                &[],
                "fields",
                why,
            )
        });
    };
    let url = Policy {
        contact_ko: Some("https://rd.example.com/get 에서 받으세요.".to_owned()),
        ..p1()
    };
    tool(
        "text-with-url",
        url.to_text(),
        "a URL in contact_ko: the tool never signs one, and an app never makes text a link",
    );
    tool(
        "unknown-field",
        replaced(
            &p1_text,
            "\"seq\":1,",
            "\"seq\":1,\"expires\":\"2027-01-01T00:00:00Z\",",
        ),
        "a field the format does not have",
    );
    tool(
        "duplicate-field",
        replaced(&p1_text, "\"seq\":1,", "\"seq\":1,\"seq\":1,"),
        "a field twice",
    );
    tool(
        "not-canonical",
        replaced(&p1_text, "{\"v\":1,", "{ \"v\":1,"),
        "a space the tool does not write",
    );
    cases
}

/// What the manifest says about a case.
fn manifest_entry(case: &Case, outcome: &Result<State, String>) -> serde_json::Value {
    let mut entry = serde_json::json!({
        "name": case.name,
        "file": case.path,
        "expect": if case.accept { "accept" } else { "reject" },
        "after": case.after.iter().map(|name| format!("accepted/{name}.json")).collect::<Vec<_>>(),
        "rule": case.rule,
        "binding": case.binding,
        "why": case.why,
    });
    if let Some(floor) = case.floor {
        entry["floor"] = floor.into();
    }
    if let Some(next) = case.then_accept {
        entry["then_accept"] = format!("accepted/{next}.json").into();
    }
    if let Ok(state) = outcome {
        let last = state.last.as_ref().unwrap();
        entry["state_after"] = serde_json::json!({
            "epoch": state.epoch,
            "seq": last.policy.seq,
            "revoked": state.revoked.iter().collect::<Vec<_>>(),
            "p_sha256": hex(&last.p_hash),
        });
    }
    entry
}

/// Files (path and content) and the cases they belong to.
type Generated = (Vec<(String, String)>, Vec<Case>);

/// The files of the vectors directory that this test makes (path and content), and the cases.
/// Made once; with HJV_RELEASE_WRITE_VECTORS set, also written before any test reads a file.
fn generated() -> &'static Generated {
    static GENERATED: OnceLock<Generated> = OnceLock::new();
    GENERATED.get_or_init(|| {
        let made = generate();
        if std::env::var_os("HJV_RELEASE_WRITE_VECTORS").is_some() {
            for (path, content) in &made.0 {
                let full = dir().join(path);
                fs::create_dir_all(full.parent().unwrap()).unwrap();
                fs::write(&full, content).unwrap();
            }
        }
        made
    })
}

fn generate() -> Generated {
    let mut cases = cases();
    let by_name: BTreeMap<&str, String> = cases
        .iter()
        .filter(|c| !c.content.is_empty())
        .map(|c| (c.name, c.content.clone()))
        .collect();
    // a case that points at another case's file has that file's content
    for case in cases.iter_mut().filter(|c| c.content.is_empty()) {
        let stem = Path::new(&case.path).file_stem().unwrap().to_str().unwrap();
        case.content = by_name[stem].clone();
    }
    let keys = test_key_set();
    let mut files: Vec<(String, String)> = cases
        .iter()
        .filter(|c| c.path.ends_with(&format!("/{}.json", c.name)))
        .map(|c| (c.path.clone(), c.content.clone()))
        .collect();
    files.push((
        "update-keys.env".to_owned(),
        format!(
            "# The update keys of the test vectors. TEST KEYS: their private halves are in this\n\
         # repository (src/keys.rs, TEST_KEYS), so a real build must never hold them.\n{}",
            keys.to_text()
        ),
    ));
    files.push((
        "root-declaration-e2.json".to_owned(),
        format!(
            "{}\n",
            signed(&r2().to_text(), "root-101", ROOT_DOMAIN).to_text()
        ),
    ));

    let outcomes: Vec<Result<State, String>> = cases.iter().map(|c| run(c, &cases)).collect();
    let test_keys: Vec<serde_json::Value> = TEST_KEYS
        .iter()
        .map(|k| {
            serde_json::json!({
                "name": k.name,
                "role": k.role.name(),
                "id": k.id,
                "seed_hex": hex(&k.seed()),
                "public": k.public().to_string(),
                "in_the_key_file": keys.find(k.id).is_some_and(|(_, public)| *public == k.public()),
            })
        })
        .collect();
    let manifest = serde_json::json!({
        "about": "Test vectors of HJViewer's signed version policy (design 1-1, 1-4, 6-2). README.md explains the fields. Made and checked by tools/hjv-release/tests/vectors.rs.",
        "server": SERVER,
        "domains": {"policy": "hjv-version-v1", "root_declaration": "hjv-version-root-v1"},
        "key_file": {
            "file": "update-keys.env",
            "HJV_UPDATE_PUBKEY": keys.signing_line,
            "HJV_UPDATE_ROOT_PUBKEY": keys.root_line,
            "HJV_POLICY_FLOOR": keys.floor.to_string(),
            "fingerprint_sha256": hex(&keys.fingerprint(SERVER)),
        },
        "test_keys": test_keys,
        "root_declaration": {
            "file": "root-declaration-e2.json",
            "p": r2().to_text(),
            "signed_bytes_sha256": hex(&sha256(&hjv_release::envelope::message(ROOT_DOMAIN, &r2().to_text()))),
        },
        "key_file_example": {
            "file": "keyfile/signing-1.age",
            "passphrase": KEYFILE_PASSPHRASE,
            "made_by": "the age crate 0.11.5 (scrypt work factor 10), not by this tool",
            "holds": "test key signing-1",
        },
        "research_vector": {
            "file": "research/version-policy.json",
            "note": "the first draft of the format (build/minver/policy-distribution): the signature is good for signing key 1, the policy text has no epoch and is refused as a policy",
        },
        "cases": cases.iter().zip(&outcomes).map(|(c, o)| manifest_entry(c, o)).collect::<Vec<_>>(),
    });
    files.push((
        "manifest.json".to_owned(),
        format!("{}\n", serde_json::to_string_pretty(&manifest).unwrap()),
    ));
    (files, cases)
}

/// Applies a case to an app with the test keys: the state after an accepted envelope, or why
/// it was refused. Panics when the outcome is not the expected one.
fn run(case: &Case, all: &[Case]) -> Result<State, String> {
    let mut keys = test_key_set();
    if let Some(floor) = case.floor {
        keys.floor = Floor::parse(floor).unwrap();
    }
    let content = |name: &str| {
        let found = all.iter().find(|c| c.name == name && c.accept);
        found
            .unwrap_or_else(|| panic!("{}: no accepted case {name}", case.name))
            .content
            .clone()
    };
    let mut state = State::initial(&keys);
    for name in &case.after {
        let checked = check_envelope(&content(name), &keys, SERVER)
            .unwrap_or_else(|e| panic!("{}: {name} before it does not check: {e}", case.name));
        state = state
            .accept(&keys, &checked)
            .unwrap_or_else(|e| panic!("{}: {name} before it is not accepted: {e}", case.name));
    }
    let outcome =
        check_envelope(&case.content, &keys, SERVER).and_then(|c| state.accept(&keys, &c));
    match (&outcome, case.accept) {
        (Ok(_), true) | (Err(_), false) => {}
        (Ok(_), false) => panic!("{} was accepted", case.name),
        (Err(e), true) => panic!("{} was refused: {e}", case.name),
    }
    if let Some(next) = case.then_accept {
        // the refused envelope left nothing behind
        let checked = check_envelope(&content(next), &keys, SERVER).unwrap();
        state
            .accept(&keys, &checked)
            .unwrap_or_else(|e| panic!("{}: {next} is not accepted afterwards: {e}", case.name));
    }
    outcome
}

#[test]
fn the_files_are_what_the_code_makes() {
    let (files, _) = generated();
    let mut stale = Vec::new();
    for (path, content) in files {
        if fs::read_to_string(dir().join(path)).ok().as_deref() != Some(content.as_str()) {
            stale.push(path.clone());
        }
    }
    assert!(
        stale.is_empty(),
        "these vector files differ from what the code makes (HJV_RELEASE_WRITE_VECTORS=1 writes them): {stale:?}"
    );
    // and nothing lies around that the code does not make or the README does not name
    let known: Vec<&str> = files.iter().map(|(path, _)| path.as_str()).collect();
    let kept = [
        "README.md",
        "research/version-policy.json",
        "research/vector.json",
        "keyfile/signing-1.age",
    ];
    for sub in ["", "accepted", "refused", "research", "keyfile"] {
        for entry in fs::read_dir(dir().join(sub)).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                continue;
            }
            let name = entry.file_name().into_string().unwrap();
            let path = if sub.is_empty() {
                name
            } else {
                format!("{sub}/{name}")
            };
            assert!(
                known.contains(&path.as_str()) || kept.contains(&path.as_str()),
                "{path} is not a vector"
            );
        }
    }
}

#[test]
fn every_case_has_its_outcome() {
    let (_, cases) = generated();
    let (accepted, refused): (Vec<&Case>, Vec<&Case>) = cases.iter().partition(|c| c.accept);
    assert!(
        accepted.len() >= 20 && refused.len() >= 50,
        "{} accepted, {} refused",
        accepted.len(),
        refused.len()
    );
    // names are unique, and every rule has cases on both sides where it can
    let mut names: Vec<&str> = cases.iter().map(|c| c.name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), cases.len());
    for rule in [
        "signature",
        "key",
        "envelope",
        "version",
        "server",
        "size",
        "root-declaration",
        "revoked",
        "epoch",
        "seq",
        "floor",
        "relaxation",
        "fields",
    ] {
        assert!(
            refused.iter().any(|c| c.rule == rule),
            "no refused case for {rule}"
        );
    }
    // the outcomes hold for the files on disk as well as for the generated bytes
    for case in cases {
        let on_disk = fs::read_to_string(dir().join(&case.path)).unwrap();
        assert_eq!(on_disk, case.content, "{}", case.path);
    }
    // the size limit is pinned from both sides, to the byte
    let size = |name: &str| cases.iter().find(|c| c.name == name).unwrap().content.len();
    assert_eq!(
        (size("size-at-the-limit"), size("size-one-over")),
        (16384, 16385)
    );
}

#[test]
fn after_a_policy_of_the_root_key() {
    // The history has a relaxation by the root key (e2-s3) and two ways back from it. What an
    // app that was off during the relaxation makes of each is what the cases say, and why:
    let (_, cases) = generated();
    let case = |name: &str| cases.iter().find(|c| c.name == name).unwrap();
    let missed = run(case("e2-s4-missed-the-relaxation"), cases).unwrap_err();
    assert!(
        missed.contains("서명 키로는 완화할 수 없습니다")
            && missed.contains("ios: 같은 min의 유예를 늦추거나 새로 둠"),
        "{missed}"
    );
    // (the app that saw the relaxation takes the same envelope: the history's e2-s4)
    assert!(run(case("e2-s4"), cases).is_ok());
    for name in [
        "restore-with-the-old-deadline",
        "restore-with-the-old-deadline-missed-the-relaxation",
    ] {
        let state = run(case(name), cases).unwrap();
        let policy = &state.last.as_ref().unwrap().policy;
        assert_eq!((policy.epoch, policy.seq), (2, 4), "{name}");
        assert_eq!(policy.platforms[&Platform::Ios].min, v("1.6.0"));
        assert_eq!(
            policy.platforms[&Platform::Ios].grace_until,
            Some(t(GRACE)),
            "{name}"
        );
    }
    // a root-signed policy of the epoch before: refused for its epoch, not for its seq
    let old = run(case("root-policy-of-an-old-epoch"), cases).unwrap_err();
    assert!(
        old.contains("정책의 세대(1)가 지금 세대(2)보다 낮습니다"),
        "{old}"
    );
    assert!(run(case("root-seq-jump"), cases).is_ok());
    // the signature with a small-order R is refused as a signature, nothing else
    let small = run(case("signature-with-identity-r"), cases).unwrap_err();
    assert_eq!(small, "정책의 서명이 맞지 않습니다");
    // the tool's own, stricter rule about a new entry below the minimums is not the apps'
    let low = case("new-entry-below-the-minimums");
    assert!(run(low, cases).is_ok());
    let before = Policy::parse(&Envelope::parse(&case("e1-s2").content).unwrap().policy.p).unwrap();
    let after = Policy::parse(&Envelope::parse(&low.content).unwrap().policy.p).unwrap();
    let below = hjv_release::trust::new_entries_below(&before, &after);
    assert_eq!(below.len(), 1);
    assert_eq!(
        (below[0].platform, below[0].min, below[0].below_lowest),
        (Platform::AndroidPlay, v("1.0.0"), Some(v("1.6.0")))
    );
}

#[test]
fn the_history_ends_where_the_manifest_says() {
    let (_, cases) = generated();
    let last = cases.iter().find(|c| c.name == "e2-s4").unwrap();
    let state = run(last, cases).unwrap();
    assert_eq!(state.epoch, 2);
    assert_eq!(state.revoked.iter().copied().collect::<Vec<_>>(), [1]);
    let policy = &state.last.as_ref().unwrap().policy;
    assert_eq!((policy.epoch, policy.seq), (2, 4));
    assert_eq!(policy.platforms[&Platform::Windows].min, v("1.6.1"));
    // the manifest on disk parses and names every case file
    let manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir().join("manifest.json")).unwrap()).unwrap();
    let listed = manifest["cases"].as_array().unwrap();
    assert_eq!(listed.len(), cases.len());
    for entry in listed {
        assert!(
            dir().join(entry["file"].as_str().unwrap()).is_file(),
            "{entry}"
        );
        for before in entry["after"].as_array().unwrap() {
            assert!(dir().join(before.as_str().unwrap()).is_file(), "{entry}");
        }
    }
    let e2_s4 = listed.iter().find(|e| e["name"] == "e2-s4").unwrap();
    assert_eq!(e2_s4["state_after"]["epoch"], 2);
    assert_eq!(e2_s4["state_after"]["seq"], 4);
    assert_eq!(e2_s4["state_after"]["revoked"], serde_json::json!([1]));
}

#[test]
fn the_key_file_of_the_vectors() {
    generated();
    let text = fs::read_to_string(dir().join("update-keys.env")).unwrap();
    let keys = keys::KeySet::parse(&text).unwrap();
    assert!(keys.has_test_key());
    assert_eq!(
        keys.signing.iter().map(|k| k.id).collect::<Vec<_>>(),
        [1, 2]
    );
    assert_eq!(keys.root.id, 101);
    assert_eq!(keys.floor, Floor { epoch: 1, seq: 0 });
    assert_eq!(keys.to_text(), test_key_set().to_text());
    // the fingerprint a build ties its stored state to (design 1-4), spelled out
    let mut bytes = b"hjv-update-keys-v1\0rd.example.com\0".to_vec();
    bytes.extend_from_slice(keys.signing_line.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(keys.root_line.as_bytes());
    assert_eq!(keys.fingerprint(SERVER), sha256(&bytes));
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
    generated();
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

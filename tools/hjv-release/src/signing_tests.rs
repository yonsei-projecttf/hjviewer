// SPDX-License-Identifier: AGPL-3.0-only

//! Tests of signing.rs with the shared vectors' files: what `sign` refuses in a draft whoever
//! made it (the base recomputed, X2, X10, X11), whom it lets sign (X9, X12), and what it says.
//! make_tests.rs signs every policy it makes through the same functions.

use super::*;
use crate::{
    archive::Archive,
    keyfile::SecretKey,
    keys::{test_key, test_key_set},
    policy::tests::t,
    release::tests::temp_dir,
};
use std::path::Path;

fn vector(path: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/vectors")
            .join(path),
    )
    .unwrap()
}

fn secret(name: &str) -> SecretKey {
    let key = test_key(name).unwrap();
    SecretKey::from_seed(key.role, key.id, &key.seed())
}

/// An archive that holds the history's files up to `upto`, as `sign` would have left it.
fn archive_with(name: &str, upto: &[&str]) -> Archive {
    let archive = Archive::open(&temp_dir(name), true).unwrap();
    for file in upto {
        let text = vector(&format!("accepted/{file}.json"));
        let (epoch, seq) = crate::archive::position_of(&text).unwrap();
        archive.add_policy(epoch, seq, &text).unwrap();
        if let Some(part) = Envelope::parse(&text).unwrap().root {
            let _ = archive.add_declaration(epoch, &part);
        }
    }
    archive
}

fn p_of(file: &str) -> String {
    Envelope::parse(&vector(&format!("accepted/{file}.json")))
        .unwrap()
        .policy
        .p
}

fn root_p_of(file: &str) -> String {
    Envelope::parse(&vector(&format!("accepted/{file}.json")))
        .unwrap()
        .root
        .unwrap()
        .p
}

/// The declaration of epoch 2 with the policy e2-s1, after e1-s3 (the history's own draft).
fn declaration_draft() -> Draft {
    Draft {
        p: p_of("e2-s1"),
        signer: Role::Signing,
        root: None,
        root_p: Some(root_p_of("e2-s1")),
        prev: Some(vector("accepted/e1-s3.json")),
        notes: Vec::new(),
    }
}

#[test]
fn a_declaration_is_checked_from_its_texts() {
    let keys = test_key_set();
    let archive = archive_with("sign-decl", &["e1-s1", "e1-s2", "e1-s3"]);
    let now = t("2026-10-06T03:00:00Z");
    let live = vector("accepted/e1-s3.json");
    let draft = declaration_draft();
    let plan = plan(&draft, &keys, &archive, Some(&live), now).unwrap();
    assert_eq!(plan.new_root.as_ref().unwrap().epoch, 2);
    assert_eq!(plan.needed(&draft), [Role::Root, Role::Signing]);
    assert!(plan.dangers[0].contains("깨끗한 컴퓨터"));
    assert!(plan
        .dangers
        .iter()
        .any(|d| d.contains("서명 키(번호 1)를 폐기합니다")));
    assert!(plan.dangers.iter().any(|d| d.contains("--relaxes 없이")));
    assert!(plan
        .summary
        .iter()
        .any(|l| l.contains("폐기와 함께 나가는 패키지 windows 1.6.1")));
    // X10: the live file is --prev and a file of the archive
    assert!(plan_err(&draft, &keys, &archive, None, now).contains("--live"));
    let other_live = vector("accepted/e1-s2.json");
    assert!(plan_err(&draft, &keys, &archive, Some(&other_live), now).contains("X10"));
    let empty = Archive::open(&temp_dir("sign-decl-empty"), true).unwrap();
    assert!(plan_err(&draft, &keys, &empty, Some(&live), now).contains("X10"));
    // the base is recomputed: a declaration whose base is not the policy's requirements
    let mut decl = RootDecl::parse(&root_p_of("e2-s1")).unwrap();
    decl.base = Policy::parse(&p_of("e1-s1")).unwrap().requirements();
    let wrong_base = Draft {
        root_p: Some(decl.to_text()),
        ..declaration_draft()
    };
    assert!(plan_err(&wrong_base, &keys, &archive, Some(&live), now).contains("base"));
    // X2: an epoch the archive holds already, a dropped revocation
    let _ = archive.add_declaration(
        2,
        &Part {
            k: 101,
            p: root_p_of("e3-s1-relax"),
            s: [1; 64],
        },
    );
    let err = plan_err(&draft, &keys, &archive, Some(&live), now);
    assert!(err.contains("X2"), "{err}");
    let archive = archive_with(
        "sign-decl-revoked",
        &["e1-s1", "e1-s2", "e1-s3", "e2-s1", "e2-s2"],
    );
    let mut d3 = RootDecl::parse(&root_p_of("e3-s1-relax")).unwrap();
    d3.revoked.clear();
    let forgets = Draft {
        p: p_of("e3-s1-relax"),
        root_p: Some(d3.to_text()),
        prev: Some(vector("accepted/e2-s2.json")),
        ..declaration_draft()
    };
    let live = vector("accepted/e2-s2.json");
    let err = plan_err(
        &forgets,
        &keys,
        &archive,
        Some(&live),
        t("2026-10-08T03:00:00Z"),
    );
    assert!(err.contains("X2") && err.contains("빼먹었습니다"), "{err}");
}

fn plan_err(
    draft: &Draft,
    keys: &KeySet,
    archive: &Archive,
    live: Option<&str>,
    now: i64,
) -> String {
    match plan(draft, keys, archive, live, now) {
        Ok(_) => panic!("planned"),
        Err(e) => e,
    }
}

#[test]
fn a_policy_without_a_declaration_follows_the_archive() {
    let keys = test_key_set();
    let archive = archive_with("sign-x11", &["e1-s1", "e1-s2"]);
    let now = t("2026-10-05T03:00:00Z");
    let next = Draft {
        p: p_of("e1-s3"),
        signer: Role::Signing,
        root: None,
        root_p: None,
        prev: Some(vector("accepted/e1-s2.json")),
        notes: Vec::new(),
    };
    assert!(plan(&next, &keys, &archive, None, now).is_ok());
    // X11: --prev is the archive's last file
    let stale = Draft {
        prev: Some(vector("accepted/e1-s1.json")),
        ..next.clone()
    };
    assert!(plan_err(&stale, &keys, &archive, None, now).contains("X11"));
    // a position the archive holds already
    archive
        .add_policy(1, 3, &vector("accepted/e1-s3.json"))
        .unwrap();
    assert!(plan_err(&next, &keys, &archive, None, now).contains("X11"));
    // the declaration goes along unchanged
    let archive = archive_with("sign-carried", &["e1-s1", "e1-s2", "e1-s3", "e2-s1"]);
    let carried = Draft {
        p: p_of("e2-s2"),
        signer: Role::Signing,
        root: Envelope::parse(&vector("accepted/e3-s2.json"))
            .unwrap()
            .root,
        root_p: None,
        prev: Some(vector("accepted/e2-s1.json")),
        notes: Vec::new(),
    };
    let err = plan_err(&carried, &keys, &archive, None, t("2026-10-07T03:00:00Z"));
    assert!(
        err.contains("루트 선언이 이전 정책의 것과 다릅니다"),
        "{err}"
    );
}

#[test]
fn who_may_sign() {
    let keys = test_key_set();
    // a policy a root key signs while a signing key serves every recorded build (X9)
    let archive = archive_with("sign-x9", &["e1-s1"]);
    let by_root = Draft {
        p: p_of("e1-s2"),
        signer: Role::Root,
        root: None,
        root_p: None,
        prev: Some(vector("accepted/e1-s1.json")),
        notes: Vec::new(),
    };
    let now = t("2026-10-02T03:00:00Z");
    let plan_root = plan(&by_root, &keys, &archive, None, now).unwrap();
    assert_eq!(plan_root.needed(&by_root), [Role::Root]);
    assert!(plan_root
        .dangers
        .iter()
        .any(|d| d.contains("루트 키로 서명합니다")));
    let err = sign(
        &by_root,
        &plan_root,
        &[secret("root-101")],
        &keys,
        &archive,
        None,
    )
    .unwrap_err();
    assert!(err.contains("X9"), "{err}");
    // a declaration by a root key that a recorded build does not pin (X12)
    let archive = archive_with("sign-x12", &["e1-s1", "e1-s2", "e1-s3"]);
    archive
        .record_keyset(&keys, "rd.example.com", Some("1.6.0"))
        .unwrap();
    let pin = |name: &str| test_key(name).unwrap().public().to_string();
    let newer = KeySet::from_values(
        &keys.signing_line,
        &format!("{},{}", pin("root-102"), pin("root-103")),
        "1:3",
    )
    .unwrap();
    archive
        .record_keyset(&newer, "rd.example.com", Some("1.6.1"))
        .unwrap();
    let draft = declaration_draft();
    let live = vector("accepted/e1-s3.json");
    let now = t("2026-10-06T03:00:00Z");
    let planned = plan(&draft, &keys, &archive, Some(&live), now).unwrap();
    let err = sign(
        &draft,
        &planned,
        &[secret("root-101"), secret("signing-2")],
        &keys,
        &archive,
        None,
    )
    .unwrap_err();
    assert!(err.contains("X12") && err.contains("루트 키 101"), "{err}");
    // the other root key serves both builds
    let signed = sign(
        &draft,
        &planned,
        &[secret("root-102"), secret("signing-2")],
        &keys,
        &archive,
        None,
    )
    .unwrap();
    let envelope = Envelope::parse(&signed.file).unwrap();
    assert_eq!((envelope.policy.k, envelope.root.unwrap().k), (2, 102));
    assert_eq!(signed.archived.len(), 2, "{:?}", signed.archived);
    // a key the declaration revokes signs nothing
    let archive = archive_with("sign-revoked", &["e1-s1", "e1-s2", "e1-s3"]);
    let planned = plan(&draft, &keys, &archive, Some(&live), now).unwrap();
    let err = sign(
        &draft,
        &planned,
        &[secret("root-101"), secret("signing-1")],
        &keys,
        &archive,
        None,
    )
    .unwrap_err();
    assert!(err.contains("폐기한 키"), "{err}");
}

#[test]
fn what_an_entry_blocks_is_said() {
    let keys = test_key_set();
    let archive = archive_with("sign-said", &["e1-s1"]);
    let next = Draft {
        p: p_of("e1-s2"),
        signer: Role::Signing,
        root: None,
        root_p: None,
        prev: Some(vector("accepted/e1-s1.json")),
        notes: Vec::new(),
    };
    let plan = plan(&next, &keys, &archive, None, t("2026-10-02T03:00:00Z")).unwrap();
    assert!(
        plan.summary.iter().any(|l| l.contains(
            "windows: 지금은 1.5.0 미만을 막고, 2026-10-17 00:00 한국 시각부터는 1.6.0 미만을 막습니다"
        )),
        "{:?}",
        plan.summary
    );
    assert!(plan.summary.iter().any(|l| l.contains("유예 끝까지")));
    assert!(plan.dangers.is_empty(), "{:?}", plan.dangers);
    // signed a week later, the deadline is still ahead; signed after it, it is a danger
    let late = super::plan(&next, &keys, &archive, None, t("2026-10-17T00:00:00Z")).unwrap();
    assert!(
        late.dangers.iter().any(|d| d.contains("이미 지났습니다")),
        "{:?}",
        late.dangers
    );
}

/// What a declaration from a rolled-back or abandoned state asks the signer about (the
/// archive's newer files, revocations of archived declarations it does not carry).
#[test]
fn a_declaration_after_a_rolled_back_live_file_is_asked_about() {
    let keys = test_key_set();
    let archive = archive_with("sign-rolled-back", &["e1-s1", "e1-s2", "e1-s3"]);
    let live = vector("accepted/e1-s2.json");
    let rolled_back = Draft {
        prev: Some(live.clone()),
        ..declaration_draft()
    };
    let plan = plan(
        &rolled_back,
        &keys,
        &archive,
        Some(&live),
        t("2026-10-06T03:00:00Z"),
    )
    .unwrap();
    assert!(
        plan.dangers
            .iter()
            .any(|d| d.contains("보관소에 서버의 정책(1:2)보다 높은 파일(1:3)이 있습니다")),
        "{:?}",
        plan.dangers
    );
    // an archived declaration of epoch 2 that revoked key 1 and was never published: the next
    // declaration (epoch 3) without key 1 is asked about
    let _ = archive.add_declaration(
        2,
        &Envelope::parse(&vector("accepted/e2-s1.json"))
            .unwrap()
            .root
            .unwrap(),
    );
    let mut d3 = RootDecl::parse(&root_p_of("e2-s1")).unwrap();
    d3.epoch = 3;
    d3.revoked.clear();
    let mut p = Policy::parse(&p_of("e2-s1")).unwrap();
    p.epoch = 3;
    let draft = Draft {
        p: p.to_text(),
        root_p: Some(d3.to_text()),
        prev: Some(vector("accepted/e1-s3.json")),
        ..declaration_draft()
    };
    let live = vector("accepted/e1-s3.json");
    let plan = super::plan(
        &draft,
        &keys,
        &archive,
        Some(&live),
        t("2026-10-06T03:00:00Z"),
    )
    .unwrap();
    assert!(
        plan.dangers
            .iter()
            .any(|d| d.contains("보관소의 루트 선언(세대 2)이 폐기한 서명 키 1")),
        "{:?}",
        plan.dangers
    );
}

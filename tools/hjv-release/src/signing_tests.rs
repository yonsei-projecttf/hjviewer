// SPDX-License-Identifier: AGPL-3.0-only

//! Tests of signing.rs with the shared vectors' files: what `sign` refuses in a draft whoever
//! made it (the base recomputed, X1, X2, X10, X11, the times, the first position, X4 on the
//! signed bytes), whom it lets sign (X9, X12 by key, not by number), and what it says.
//! make_tests.rs signs every policy it makes through the same functions.

use super::*;
use crate::{
    archive::Archive,
    keyfile::SecretKey,
    keys::{test_key, test_key_set},
    policy::tests::{t, v},
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
        appstore_release: None,
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
        appstore_release: None,
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
        appstore_release: None,
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
        appstore_release: None,
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
        appstore_release: None,
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
    // the vectors' history has no Google Play entry: the raise of android is T2's danger
    assert_eq!(plan.dangers.len(), 1, "{:?}", plan.dangers);
    assert!(
        plan.dangers[0]
            .starts_with("android: android_play 항목 없이 android의 min을 올립니다(--no-play)"),
        "{:?}",
        plan.dangers
    );
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

/// The draft `e1-s3` after `e1-s2` with its policy changed by hand.
fn edited_next(change: impl FnOnce(&mut Policy)) -> Draft {
    let mut p = Policy::parse(&p_of("e1-s3")).unwrap();
    change(&mut p);
    Draft {
        p: p.to_text(),
        signer: Role::Signing,
        root: None,
        root_p: None,
        prev: Some(vector("accepted/e1-s2.json")),
        appstore_release: None,
        notes: Vec::new(),
    }
}

/// An envelope of `p` alone, signed with the test key `by`.
fn envelope_of(p: &str, by: &str) -> String {
    let key = test_key(by).unwrap();
    Envelope {
        policy: Part::sign(
            &ed25519_dalek::SigningKey::from_bytes(&key.seed()),
            key.id,
            POLICY_DOMAIN,
            p,
        ),
        root: None,
    }
    .to_file()
}

/// What `policy` never makes, refused by `sign` all the same: the draft is a file an agent wrote
/// and `sign` is the last check before a key is used. The times, the positions and X4 on the
/// signed bytes, each in a draft edited by hand.
#[test]
fn hand_edited_drafts() {
    let keys = test_key_set();
    let now = t("2026-10-05T03:00:00Z");
    let archive = archive_with("sign-edited", &["e1-s1", "e1-s2"]);
    assert!(plan(&edited_next(|_| {}), &keys, &archive, None, now).is_ok());
    // issued ahead of this PC's clock (300 s are allowed), or before the previous policy
    let ahead = |seconds: i64| edited_next(|p| p.issued = now + seconds);
    assert!(plan(&ahead(CLOCK_SLACK), &keys, &archive, None, now).is_ok());
    assert!(plan_err(&ahead(CLOCK_SLACK + 1), &keys, &archive, None, now).contains("미래"));
    assert!(plan_err(&ahead(DAY), &keys, &archive, None, now).contains("미래"));
    let before_prev = edited_next(|p| p.issued = t("2026-10-02T02:59:59Z"));
    assert!(plan_err(&before_prev, &keys, &archive, None, now).contains("이전 정책보다 이릅니다"));
    // a minimum lowered without a declaration (X1)
    let lowered = edited_next(|p| {
        let windows = p.platforms.get_mut(&Platform::Windows).unwrap();
        windows.min = v("1.5.0");
        windows.grace_until = None;
        windows.grace_min = None;
    });
    let err = plan_err(&lowered, &keys, &archive, None, now);
    assert!(err.contains("X1") && err.contains("windows"), "{err}");
    // a seq 1001 above the previous one: asked about, and then refused on the signed bytes (X4);
    // nothing reaches the archive
    let jump = edited_next(|p| p.seq = 2 + 1001);
    let planned = plan(&jump, &keys, &archive, None, now).unwrap();
    assert!(planned
        .dangers
        .iter()
        .any(|d| d.contains("seq가 한 번에 뜁니다(2 → 1003")));
    let err = sign(
        &jump,
        &planned,
        &[secret("signing-1")],
        &keys,
        &archive,
        None,
    )
    .unwrap_err();
    assert!(
        err.contains("서명한 정책을 앱이 받지 않으므로 쓰지 않습니다")
            && err.contains("X4")
            && err.contains("Jump"),
        "{err}"
    );
    assert!(archive.at(1, 1003).unwrap().is_none());
    assert!(archive.keysets().unwrap().is_empty());
    // a key file whose floor is above the draft: the plan has nothing against it, X4 has
    let mut high_floor = test_key_set();
    high_floor.floor = crate::keys::Floor { epoch: 1, seq: 9 };
    let next = edited_next(|_| {});
    let planned = plan(&next, &high_floor, &archive, None, now).unwrap();
    let err = sign(
        &next,
        &planned,
        &[secret("signing-2")],
        &high_floor,
        &archive,
        None,
    )
    .unwrap_err();
    assert!(err.contains("X4"), "{err}");
    assert!(archive.at(1, 3).unwrap().is_none());
    // a seq not above a voided file of the epoch, though its own position is free (X11)
    let voided = archive_with("sign-edited-void", &["e1-s1", "e1-s2"]);
    let at_5 = Policy::parse(&p_of("e1-s3"))
        .map(|p| Policy { seq: 5, ..p })
        .unwrap();
    voided
        .add_policy(1, 5, &envelope_of(&at_5.to_text(), "signing-2"))
        .unwrap();
    voided.add_void(1, 5, now).unwrap();
    for seq in [3, 4, 5] {
        let err = plan_err(&edited_next(|p| p.seq = seq), &keys, &voided, None, now);
        assert!(err.contains("X11") && err.contains("(5)"), "{seq}: {err}");
    }
    assert!(plan(&edited_next(|p| p.seq = 6), &keys, &voided, None, now).is_ok());
    // the first policy lies just above the key file's floor
    let empty = Archive::open(&temp_dir("sign-edited-first"), true).unwrap();
    let first = |seq: u64| {
        let mut p = Policy::parse(&p_of("e1-s1")).unwrap();
        p.seq = seq;
        Draft {
            p: p.to_text(),
            signer: Role::Signing,
            root: None,
            root_p: None,
            prev: None,
            appstore_release: None,
            notes: Vec::new(),
        }
    };
    let first_now = t("2026-10-01T03:00:00Z");
    assert!(plan(&first(1), &keys, &empty, None, first_now).is_ok());
    let err = plan_err(&first(5), &keys, &empty, None, first_now);
    assert!(
        err.contains("첫 정책의 위치") && err.contains("1:1"),
        "{err}"
    );
    let mut floor_4 = test_key_set();
    floor_4.floor = crate::keys::Floor { epoch: 1, seq: 4 };
    assert!(plan(&first(5), &floor_4, &empty, None, first_now).is_ok());
    assert!(plan_err(&first(1), &floor_4, &empty, None, first_now).contains("1:5"));
}

/// A declaration's draft edited by hand: its policy is seq 1 of the new epoch, issued with the
/// declaration, for the same server.
#[test]
fn a_hand_edited_declaration() {
    let keys = test_key_set();
    let archive = archive_with("sign-edited-decl", &["e1-s1", "e1-s2", "e1-s3"]);
    let now = t("2026-10-06T03:00:00Z");
    let live = vector("accepted/e1-s3.json");
    assert!(plan(&declaration_draft(), &keys, &archive, Some(&live), now).is_ok());
    let mut p = Policy::parse(&p_of("e2-s1")).unwrap();
    p.seq = 2;
    let second = Draft {
        p: p.to_text(),
        ..declaration_draft()
    };
    let err = plan_err(&second, &keys, &archive, Some(&live), now);
    assert!(err.contains("seq 1"), "{err}");
    let decl = RootDecl::parse(&root_p_of("e2-s1")).unwrap();
    for (changed, why) in [
        (
            RootDecl {
                issued: decl.issued + 1,
                ..decl.clone()
            },
            "발행 시각이 정책의 발행 시각과 다릅니다",
        ),
        (
            RootDecl {
                server: "hjv.test".to_owned(),
                ..decl.clone()
            },
            "서버가 정책의 서버와 다릅니다",
        ),
    ] {
        let draft = Draft {
            root_p: Some(changed.to_text()),
            ..declaration_draft()
        };
        let err = plan_err(&draft, &keys, &archive, Some(&live), now);
        assert!(err.contains(why), "{why}: {err}");
    }
}

/// A record pins keys, not numbers (X12): a build that pins another key under a number is not
/// served by this key file's key of that number, signing key or root key.
#[test]
fn a_record_pins_keys_not_numbers() {
    let keys = test_key_set();
    let as_number = |id: u32, name: &str| {
        crate::keys::PublicKey {
            id,
            key: test_key(name).unwrap().public().key,
        }
        .to_string()
    };
    let pin = |name: &str| test_key(name).unwrap().public().to_string();
    // the archive records the key file's key set (1.6.0) and the build of 1.6.1
    let recorded = |name: &str, upto: &[&str], signing: String, roots: String| {
        let archive = archive_with(name, upto);
        archive
            .record_keyset(&keys, "rd.example.com", Some("1.6.0"))
            .unwrap();
        let other = KeySet::from_values(&signing, &roots, "1:2").unwrap();
        archive
            .record_keyset(&other, "rd.example.com", Some("1.6.1"))
            .unwrap();
        archive
    };
    // 1.6.1 pins signing key 1 and another key as 2
    let archive = recorded(
        "sign-x12-number",
        &["e1-s1", "e1-s2"],
        format!("{},{}", pin("signing-1"), as_number(2, "signing-3")),
        keys.root_line.clone(),
    );
    let next = Draft {
        p: p_of("e1-s3"),
        signer: Role::Signing,
        root: None,
        root_p: None,
        prev: Some(vector("accepted/e1-s2.json")),
        appstore_release: None,
        notes: Vec::new(),
    };
    let now = t("2026-10-05T03:00:00Z");
    let planned = plan(&next, &keys, &archive, None, now).unwrap();
    let err = sign(
        &next,
        &planned,
        &[secret("signing-2")],
        &keys,
        &archive,
        None,
    )
    .unwrap_err();
    assert!(err.contains("X12") && err.contains("서명 키 2"), "{err}");
    assert!(sign(
        &next,
        &planned,
        &[secret("signing-1")],
        &keys,
        &archive,
        None
    )
    .is_ok());
    // 1.6.1 pins another key as root key 101: a declaration is signed by 102
    let archive = recorded(
        "sign-x12-root-number",
        &["e1-s1", "e1-s2", "e1-s3"],
        keys.signing_line.clone(),
        format!("{},{}", as_number(101, "root-103"), pin("root-102")),
    );
    let draft = declaration_draft();
    let live = vector("accepted/e1-s3.json");
    let planned = plan(
        &draft,
        &keys,
        &archive,
        Some(&live),
        t("2026-10-06T03:00:00Z"),
    )
    .unwrap();
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
    let signed = sign(
        &draft,
        &planned,
        &[secret("root-102"), secret("signing-2")],
        &keys,
        &archive,
        None,
    )
    .unwrap();
    assert_eq!(Envelope::parse(&signed.file).unwrap().root.unwrap().k, 102);
}

// SPDX-License-Identifier: AGPL-3.0-only

//! Tests of trust.rs beyond the shared vectors (tests/vectors.rs runs every one of those): COVERS
//! against the thresholds it stands for, the kinds of RELAXED, LOAD part by part (trust rules
//! 1-6 and 7-6), the floor of a newer build, the draft check and the tool's strict layer.

use super::*;
use crate::{
    keys::{test_key_set, Floor},
    policy::tests::{t, v},
};
use std::path::Path;

const SERVER: &str = "rd.example.com";

fn vector(path: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/vectors")
            .join(path),
    )
    .unwrap()
}

fn root_of(file: &str) -> Option<Part> {
    Envelope::parse(file).unwrap().root
}

fn requirement(min: &str, until: Option<i64>, grace_min: Option<&str>) -> Requirement {
    Requirement {
        min: v(min),
        latest: v(min),
        grace_until: until,
        grace_min: grace_min.map(v),
    }
}

/// INV-1: COVERS(n, o) iff THRESHOLD(n, t) >= THRESHOLD(o, t) at every t, for every pair of
/// entries over a small domain (the cut times are enough: thresholds change only at deadlines).
#[test]
fn covers_is_the_comparison_at_every_time() {
    let (t1, t2) = (t("2026-10-16T15:00:00Z"), t("2026-10-23T15:00:00Z"));
    let versions = ["0.0.0", "1.5.0", "1.6.0", "1.6.1"];
    let mut entries = Vec::new();
    for deadline in [None, Some(t1), Some(t2)] {
        for (i, min) in versions.iter().enumerate() {
            match deadline {
                None => entries.push(requirement(min, None, None)),
                Some(d) => {
                    for gm in &versions[..i] {
                        entries.push(requirement(min, Some(d), Some(gm)));
                    }
                }
            }
        }
    }
    assert_eq!(entries.len(), 16);
    let times = [t1 - 1, t1, t2 - 1, t2, t2 + 1, 0];
    for n in &entries {
        for o in &entries {
            let pointwise = times.iter().all(|at| n.threshold(*at) >= o.threshold(*at));
            assert_eq!(covers(n, o), pointwise, "{n:?} {o:?}");
        }
    }
}

#[test]
fn the_kinds_of_relaxed() {
    let (t1, t2) = (t("2026-10-16T15:00:00Z"), t("2026-10-23T15:00:00Z"));
    // every entry with the same latest, so that only what the case changes changes
    let requirement = |min: &str, until: Option<i64>, grace_min: Option<&str>| Requirement {
        latest: v("1.6.1"),
        ..requirement(min, until, grace_min)
    };
    let map = |items: &[(Platform, Requirement)]| items.iter().copied().collect::<BTreeMap<_, _>>();
    let w = Platform::Windows;
    let pending = requirement("1.6.0", Some(t1), Some("1.5.0"));
    let cases = [
        (requirement("1.5.0", None, None), Loosening::MinLowered),
        (
            requirement("1.6.0", Some(t2), Some("1.5.0")),
            Loosening::DeadlineLater,
        ),
        (
            requirement("1.6.0", Some(t1), Some("1.4.0")),
            Loosening::GraceMinLowered,
        ),
        (
            requirement("1.6.1", Some(t2), Some("1.5.0")),
            Loosening::RaiseLetsOldGo,
        ),
    ];
    for (n, kind) in cases {
        assert_eq!(
            loosenings(&map(&[(w, pending)]), &map(&[(w, n)])),
            [(w, kind)],
            "{n:?}"
        );
    }
    let lower_latest = Requirement {
        latest: v("1.6.0"),
        ..requirement("1.5.0", None, None)
    };
    assert_eq!(
        loosenings(
            &map(&[(w, requirement("1.5.0", None, None))]),
            &map(&[(w, lower_latest)])
        ),
        [(w, Loosening::LatestLowered)]
    );
    assert_eq!(
        loosenings(&map(&[(w, pending)]), &map(&[])),
        [(w, Loosening::Dropped)]
    );
    // never a relaxation: an earlier deadline, no deadline, a raise that keeps the level in
    // force, the pending minimum at once with a later deadline, a new windows or ios entry
    for n in [
        requirement("1.6.0", Some(t1 - 1), Some("1.5.0")),
        requirement("1.6.0", None, None),
        requirement("1.6.1", Some(t1), Some("1.5.0")),
        requirement("1.6.1", Some(t2), Some("1.6.0")),
    ] {
        assert!(!relaxes(&map(&[(w, pending)]), &map(&[(w, n)])), "{n:?}");
    }
    let new_entry = map(&[
        (w, pending),
        (Platform::Ios, requirement("0.0.0", None, None)),
    ]);
    assert!(!relaxes(&map(&[(w, pending)]), &new_entry));
    // clause 3: a new Android entry must cover its sibling
    let (a, p) = (Platform::Android, Platform::AndroidPlay);
    let android = map(&[(a, pending)]);
    for (play, relaxed) in [
        (pending, false),
        (requirement("1.6.0", Some(t2), Some("1.5.0")), true),
        (requirement("1.5.0", None, None), true),
        (requirement("1.6.1", None, None), false),
    ] {
        let both = map(&[(a, pending), (p, play)]);
        assert_eq!(relaxes(&android, &both), relaxed, "{play:?}");
        if relaxed {
            assert_eq!(super::relaxed(&android, &both), [p].into_iter().collect());
            assert_eq!(loosenings(&android, &both), [(p, Loosening::BelowSibling)]);
        }
    }
    // the first entry of the pair is free: no sibling to cover
    assert!(!relaxes(
        &map(&[(w, pending)]),
        &map(&[(w, pending), (p, requirement("1.0.0", None, None))])
    ));
}

/// LOAD keeps exactly the signed parts that verify under the reading build (1-6, 7-6).
#[test]
fn load_keeps_what_verifies_part_by_part() {
    let keys = test_key_set();
    let (e1s3, e2s2, e3s2) = (
        vector("accepted/e1-s3.json"),
        vector("accepted/e2-s2.json"),
        vector("accepted/e3-s2.json"),
    );
    let (d2, d3) = (root_of(&e2s2).unwrap(), root_of(&e3s2).unwrap());
    // what SAVE wrote, read back by the same build
    let state = load(&keys, SERVER, Some(&d2), Some(&e2s2));
    assert_eq!(
        (
            state.declared_epoch(),
            state.held.as_ref().unwrap().policy.seq
        ),
        (2, 2)
    );
    assert_eq!(state.enforces(), "policy");
    assert!(state.may_install(&keys));
    // a build of other signing keys that pins key 2 keeps both; one that does not keeps the
    // declaration and enforces its base
    let pin = |id: &str| crate::keys::test_key(id).unwrap().public().to_string();
    let keys_23 = KeySet::from_values(
        &format!("{},{}", pin("signing-2"), pin("signing-3")),
        &keys.root_line,
        "2:2",
    )
    .unwrap();
    let kept = load(&keys_23, SERVER, Some(&d2), Some(&e2s2));
    assert!(kept.decl.is_some() && kept.held.is_some());
    let keys_3 = KeySet::from_values(&pin("signing-3"), &keys.root_line, "1:0").unwrap();
    let base_only = load(&keys_3, SERVER, Some(&d2), Some(&e2s2));
    assert!(base_only.decl.is_some() && base_only.held.is_none());
    assert_eq!(base_only.enforces(), "base");
    // a build that does not pin root key 101: the declaration goes, and with it its epoch's policy
    let roots_102_103 = KeySet::from_values(
        &keys.signing_line,
        &format!("{},{}", pin("root-102"), pin("root-103")),
        "1:0",
    )
    .unwrap();
    assert_eq!(
        load(&roots_102_103, SERVER, Some(&d2), Some(&e2s2)),
        State::default()
    );
    // ... and an epoch 1 policy without a root part stays
    let epoch_1 = load(&roots_102_103, SERVER, None, Some(&e1s3));
    assert!(epoch_1.held.is_some() && epoch_1.decl.is_none());
    // a declaration newer than the held policy: the policy is kept, the base enforced (later wins)
    let newer = load(&keys, SERVER, Some(&d3), Some(&e2s2));
    assert!(newer.held.is_some());
    assert_eq!(newer.enforces(), "base");
    assert!(!newer.may_install(&keys));
    // L4: a policy newer than the declaration, or of another text of its epoch, is dropped
    let older = load(&keys, SERVER, Some(&d2), Some(&e3s2));
    assert!(older.held.is_none() && older.decl.is_some());
    let fork = root_of(&vector("refused/declaration-fork.json")).unwrap();
    let forked = load(&keys, SERVER, Some(&fork), Some(&e2s2));
    assert_eq!(
        (
            forked.decl.as_ref().unwrap().part.clone(),
            forked.held.clone()
        ),
        (fork, None)
    );
    // an epoch 1 policy with a root part, an epoch 2 policy without one
    let without_root = vector("refused/epoch-2-without-declaration.json");
    assert!(load(&keys, SERVER, Some(&d2), Some(&without_root))
        .held
        .is_none());
    // a crafted record whose held policy is by a key its declaration revokes: kept by L3 (no
    // revocation is checked there), never enforced and never installed from
    let revoked = vector("refused/revoked-key-later.json");
    let crafted = load(&keys, SERVER, Some(&d2), Some(&revoked));
    assert!(crafted.held.is_some());
    assert_eq!(crafted.enforces(), "base");
    assert!(!crafted.may_install(&keys));
    // a declaration that revokes a pinned root key: nothing is kept, a fresh start
    let revokes_root = root_of(&vector("refused/declaration-revokes-second-root.json")).unwrap();
    assert_eq!(
        load(&keys, SERVER, Some(&revokes_root), None),
        State::default()
    );
    // a record of nothing, and texts that are not canonical
    assert_eq!(load(&keys, SERVER, None, None), State::default());
    let spaced = e1s3.replacen("{\"v\":1,", "{ \"v\":1,", 1);
    assert_eq!(load(&keys, SERVER, None, Some(&spaced)), State::default());
}

/// A newer build whose floor is later keeps the held policy in force, compares with it while it
/// is of the device epoch, and installs nothing until it holds a policy at or above the floor.
#[test]
fn a_later_floor_keeps_the_held_policy() {
    let mut keys = test_key_set();
    keys.floor = Floor { epoch: 2, seq: 5 };
    let e2s2 = vector("accepted/e2-s2.json");
    let state = holding(&keys, SERVER, &e2s2);
    assert_eq!(state.enforces(), "policy");
    assert_eq!(state.epoch(&keys), 2);
    assert_eq!(state.low_same(&keys), (4, None));
    assert!(!state.may_install(&keys));
    assert!(state.basis(&keys).is_some());
    let (_, again) = accept(&keys, SERVER, &state, e2s2.as_bytes()).unwrap();
    assert_eq!(again, Outcome::Refused(Reason::Rollback));
    // a floor in a later epoch: the held policy is enforced but no basis
    keys.floor = Floor { epoch: 3, seq: 0 };
    let state = holding(&keys, SERVER, &e2s2);
    assert_eq!((state.epoch(&keys), state.enforces()), (3, "policy"));
    assert!(state.basis(&keys).is_none() && !state.may_install(&keys));
    // a fresh install of that build takes the newest envelope with its declaration of epoch 3
    let e3s2 = vector("accepted/e3-s2.json");
    let (taken, outcome) = accept(&keys, SERVER, &State::default(), e3s2.as_bytes()).unwrap();
    assert_eq!(outcome, Outcome::Taken);
    assert!(taken.may_install(&keys));
}

/// A draft is checked with its signatures taken as good, and nothing else changes.
#[test]
fn a_draft_is_checked_without_its_signatures() {
    let keys = test_key_set();
    let e1s1 = Envelope::parse(&vector("accepted/e1-s1.json")).unwrap();
    let unsigned = |k: u32| {
        Envelope {
            policy: Part {
                k,
                p: e1s1.policy.p.clone(),
                s: [0; 64],
            },
            root: None,
        }
        .to_file()
    };
    let fresh = State::default();
    let signed = accept(&keys, SERVER, &fresh, unsigned(1).as_bytes()).unwrap();
    assert_eq!(signed.1, Outcome::Refused(Reason::BadSignature));
    let draft = accept_draft(&keys, SERVER, &fresh, unsigned(1).as_bytes()).unwrap();
    assert_eq!(draft.1, Outcome::Taken);
    // the key numbers are still the build's
    let unknown = accept_draft(&keys, SERVER, &fresh, unsigned(3).as_bytes()).unwrap();
    assert_eq!(unknown.1, Outcome::Refused(Reason::UnknownKey));
}

/// The tool's strict layer: canonical texts only, and its own rules on top of the app's.
#[test]
fn the_strict_layer() {
    let keys = test_key_set();
    assert!(check_envelope(&vector("accepted/e2-s2.json"), &keys, SERVER).is_ok());
    for (file, why) in [
        ("tool/not-canonical.json", "형식"),
        ("tool/text-with-url.json", "URL"),
        ("tool/declaration-revokes-old-root-number.json", "루트 키"),
        ("refused/epoch-2-without-declaration.json", "루트 선언"),
        ("refused/revoked-key-with-declaration.json", "폐기된 키"),
        ("refused/wrong-server.json", "서버"),
    ] {
        let err = check_envelope(&vector(file), &keys, SERVER).unwrap_err();
        assert!(err.contains(why), "{file}: {err}");
    }
    // the app's reason is the app's, a text that is not canonical is no app's matter here
    assert!(accept(
        &keys,
        SERVER,
        &State::default(),
        vector("tool/unknown-field.json").as_bytes()
    )
    .is_err());
    assert!(
        canonical(vector("accepted/e1-s1.json").trim_end().as_bytes()).is_err(),
        "the line feed"
    );
    // the reason codes are written as the vectors write them
    assert_eq!(Reason::Field("grace_min").code(), "Field:grace_min");
    assert_eq!(Reason::RootFork.code(), "RootFork");
}

// SPDX-License-Identifier: AGPL-3.0-only

//! The trust rules of design 1-4, as an app applies them: which envelope a build with a given key
//! set accepts, alone and after the policy it has stored.
//!
//! The tool runs the same rules on its own output before it writes a file, so that it never
//! publishes a policy an app would drop. In short:
//!
//! - A root declaration is signed by the root key, names this server, has an epoch that is not
//!   below the stored one and never revokes the root key. Its revocations add to the stored ones.
//!   A higher epoch starts `seq` again.
//! - A policy signed by a signing key: the key is not revoked, the policy's epoch is the current
//!   one, `seq` is above the stored one by at most 1000, and nothing is relaxed.
//! - A policy signed by the root key: its epoch is not below the current one, and within the same
//!   epoch `seq` is above the stored one. It may relax.
//! - The same (epoch, seq) is the same bytes. A policy below the build's floor is dropped.
//! - If anything fails, nothing of the envelope is taken, the root declaration included.

use crate::{
    envelope::{Envelope, Part, POLICY_DOMAIN, ROOT_DOMAIN},
    keys::{Floor, KeySet, Role},
    policy::{Platform, Policy, RootDecl},
    time,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

/// How far above the stored `seq` a signing key's policy may be: a thief of a signing key cannot
/// run `seq` to its end and so shut out every later policy.
pub const MAX_SEQ_STEP: u64 = 1000;

/// An envelope that passed every check an app can make without stored state.
#[derive(Clone, Debug)]
pub struct Checked {
    pub envelope: Envelope,
    pub policy: Policy,
    pub root: Option<RootDecl>,
    /// The role of the key that signed the policy.
    pub role: Role,
    pub p_hash: [u8; 32],
}

pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The server an envelope names, read before anything is verified: only to know which server's
/// rules to check the envelope against.
pub fn peek_server(file: &str) -> Result<String, String> {
    let envelope = Envelope::parse(file)?;
    Ok(Policy::read(&envelope.policy.p)?.server)
}

/// Checks a root declaration by itself: the root key's signature first, then the text.
pub fn check_root(part: &Part, keys: &KeySet, server: &str) -> Result<RootDecl, String> {
    if part.k != keys.root.id {
        return Err(format!(
            "루트 선언의 키 번호({})가 이 빌드의 루트 키 번호({})와 다릅니다",
            part.k, keys.root.id
        ));
    }
    if !part.verifies(&keys.root.key, ROOT_DOMAIN) {
        return Err("루트 선언의 서명이 맞지 않습니다".to_owned());
    }
    let decl = RootDecl::parse(&part.p)?;
    if decl.server != server {
        return Err(format!(
            "루트 선언의 서버 이름({:?})이 이 빌드의 서버 이름({server:?})과 다릅니다",
            decl.server
        ));
    }
    if decl.revoked.contains(&keys.root.id) {
        return Err("루트 선언이 루트 키 번호를 폐기합니다".to_owned());
    }
    Ok(decl)
}

/// Checks an envelope as an app does before it looks at its stored state (design 6-2, steps
/// 2-5 and 7, and the floor): signatures before parsing, the root declaration before the policy.
pub fn check_envelope(file: &str, keys: &KeySet, server: &str) -> Result<Checked, String> {
    let envelope = Envelope::parse(file)?;
    if !envelope.is_canonical(file) {
        return Err("봉투가 이 도구가 쓰는 형식(한 줄, 공백 없음)과 다릅니다".to_owned());
    }
    let k = envelope.policy.k;
    let (role, key) = keys
        .find(k)
        .ok_or_else(|| format!("이 빌드가 믿는 키가 아닙니다(키 번호 {k})"))?;
    let root = envelope
        .root
        .as_ref()
        .map(|part| check_root(part, keys, server))
        .transpose()?;
    if root.as_ref().is_some_and(|r| r.revoked.contains(&k)) {
        return Err(format!("폐기된 키입니다(키 번호 {k})"));
    }
    if !envelope.policy.verifies(&key.key, POLICY_DOMAIN) {
        return Err("정책의 서명이 맞지 않습니다".to_owned());
    }
    let policy = Policy::parse(&envelope.policy.p)?;
    if policy.server != server {
        return Err(format!(
            "정책의 서버 이름({:?})이 이 빌드의 서버 이름({server:?})과 다릅니다",
            policy.server
        ));
    }
    if let Some(root) = &root {
        let fits = match role {
            Role::Signing => policy.epoch == root.epoch,
            Role::Root => policy.epoch >= root.epoch,
        };
        if !fits {
            return Err(format!(
                "정책의 세대({})와 루트 선언의 세대({})가 맞지 않습니다",
                policy.epoch, root.epoch
            ));
        }
    }
    let position = Floor {
        epoch: policy.epoch,
        seq: policy.seq,
    };
    if position < keys.floor {
        return Err(format!(
            "정책 위치({position})가 빌드 하한({})보다 낮습니다",
            keys.floor
        ));
    }
    Ok(Checked {
        p_hash: sha256(envelope.policy.p.as_bytes()),
        envelope,
        policy,
        root,
        role,
    })
}

/// What a signing key may not do to the stored policy (design 1-4). Removing or delaying
/// `auto_install_after` is not on the list: that is the emergency stop for unattended installs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Relaxation {
    MinLowered(Platform),
    /// The same `min` with a later `grace_until`, or with one where there was none.
    GraceLater(Platform),
    LatestLowered(Platform),
    Removed(Platform),
}

impl Relaxation {
    pub fn describe(&self) -> String {
        match self {
            Relaxation::MinLowered(p) => format!("{p}: min을 내림"),
            Relaxation::GraceLater(p) => format!("{p}: 같은 min의 유예를 늦추거나 새로 둠"),
            Relaxation::LatestLowered(p) => format!("{p}: latest를 내림"),
            Relaxation::Removed(p) => format!("{p}: 항목을 뺌"),
        }
    }
}

/// Every way `new` relaxes `old`.
pub fn relaxations(old: &Policy, new: &Policy) -> Vec<Relaxation> {
    let mut found = Vec::new();
    for (platform, before) in &old.platforms {
        let Some(after) = new.platforms.get(platform) else {
            found.push(Relaxation::Removed(*platform));
            continue;
        };
        if after.min < before.min {
            found.push(Relaxation::MinLowered(*platform));
        }
        let later = match (before.grace_until, after.grace_until) {
            (None, Some(_)) => true,
            (Some(a), Some(b)) => b > a,
            (_, None) => false,
        };
        if after.min == before.min && later {
            found.push(Relaxation::GraceLater(*platform));
        }
        if after.latest < before.latest {
            found.push(Relaxation::LatestLowered(*platform));
        }
    }
    found
}

/// The last policy an app verified and stored.
#[derive(Clone, Debug)]
pub struct Stored {
    pub policy: Policy,
    pub p_hash: [u8; 32],
}

/// What an app keeps, tied to its key set: the epoch, the revoked signing keys and the last
/// policy. A build with other keys starts again from its floor.
#[derive(Clone, Debug)]
pub struct State {
    pub epoch: u64,
    pub revoked: BTreeSet<u32>,
    pub last: Option<Stored>,
}

impl State {
    /// A fresh install: the floor's epoch, nothing revoked, no policy.
    pub fn initial(keys: &KeySet) -> State {
        State {
            epoch: keys.floor.epoch,
            revoked: BTreeSet::new(),
            last: None,
        }
    }

    /// An app that has taken `checked`, whatever it had before: its epoch and its last policy
    /// are the envelope's, and it has revoked at least what the envelope's declaration lists.
    pub fn holding(checked: &Checked) -> State {
        let root = checked.root.as_ref();
        State {
            epoch: checked.policy.epoch,
            revoked: root
                .map(|r| r.revoked.iter().copied().collect())
                .unwrap_or_default(),
            last: Some(Stored {
                policy: checked.policy.clone(),
                p_hash: checked.p_hash,
            }),
        }
    }

    /// The state after `checked`, or why an app in this state drops the envelope.
    pub fn accept(&self, keys: &KeySet, checked: &Checked) -> Result<State, String> {
        let policy = &checked.policy;
        let (mut epoch, mut revoked) = (self.epoch, self.revoked.clone());
        if let Some(root) = &checked.root {
            if root.epoch < self.epoch {
                return Err(format!(
                    "루트 선언의 세대({})가 저장된 세대({})보다 낮습니다",
                    root.epoch, self.epoch
                ));
            }
            epoch = root.epoch;
            revoked.extend(root.revoked.iter().copied());
        }
        let k = checked.envelope.policy.k;
        if revoked.contains(&k) {
            return Err(format!("폐기된 키입니다(키 번호 {k})"));
        }
        match checked.role {
            Role::Signing if policy.epoch != epoch => {
                return Err(format!(
                    "서명 키의 정책은 지금 세대의 것이어야 합니다(지금 세대 {epoch}, 정책의 세대 {})",
                    policy.epoch
                ));
            }
            Role::Root if policy.epoch < epoch => {
                return Err(format!(
                    "정책의 세대({})가 지금 세대({epoch})보다 낮습니다",
                    policy.epoch
                ));
            }
            _ => {}
        }
        // the same position is the same bytes: an app that has the policy keeps it
        let same_position = self
            .last
            .as_ref()
            .filter(|s| s.policy.epoch == policy.epoch && s.policy.seq == policy.seq);
        if let Some(stored) = same_position {
            if stored.p_hash != checked.p_hash {
                return Err(format!(
                    "같은 위치 {}:{}에 다른 내용의 정책입니다",
                    policy.epoch, policy.seq
                ));
            }
        } else {
            // the highest seq this app has taken in the policy's epoch; a fresh install counts
            // from just below its floor, so that it takes the policy the floor names
            let base = match &self.last {
                Some(stored) if stored.policy.epoch == policy.epoch => stored.policy.seq,
                Some(_) => 0,
                None if keys.floor.epoch == policy.epoch => keys.floor.seq.saturating_sub(1),
                None => 0,
            };
            if policy.seq <= base {
                return Err(format!(
                    "seq({})가 저장된 seq({base})보다 높지 않습니다",
                    policy.seq
                ));
            }
            if checked.role == Role::Signing && policy.seq > base + MAX_SEQ_STEP {
                return Err(format!(
                    "seq({})가 저장된 seq({base})에서 {MAX_SEQ_STEP} 넘게 뜁니다",
                    policy.seq
                ));
            }
        }
        if checked.role == Role::Signing {
            if let Some(stored) = &self.last {
                let relaxed = relaxations(&stored.policy, policy);
                if !relaxed.is_empty() {
                    let list: Vec<String> = relaxed.iter().map(Relaxation::describe).collect();
                    return Err(format!(
                        "서명 키로는 완화할 수 없습니다({}). 루트 키가 필요합니다",
                        list.join("; ")
                    ));
                }
            }
        }
        Ok(State {
            epoch: policy.epoch.max(epoch),
            revoked,
            last: Some(Stored {
                policy: policy.clone(),
                p_hash: checked.p_hash,
            }),
        })
    }
}

/// Checks `file` for an app that has stored `previous` (an envelope file), or for a fresh
/// install. `previous` is checked by itself first.
pub fn check_after(
    file: &str,
    previous: Option<&str>,
    keys: &KeySet,
    server: &str,
) -> Result<(Checked, State), String> {
    let state = match previous {
        Some(previous) => State::holding(
            &check_envelope(previous, keys, server).map_err(|e| format!("이전 정책: {e}"))?,
        ),
        None => State::initial(keys),
    };
    let checked = check_envelope(file, keys, server)?;
    let state = state.accept(keys, &checked)?;
    Ok((checked, state))
}

/// One line per entry, for people: versions, the deadline in Korean time, the package.
pub fn describe(policy: &Policy) -> Vec<String> {
    let mut lines = vec![format!(
        "서버 {} · 세대 {} · seq {} · 발행 {} ({})",
        policy.server,
        policy.epoch,
        policy.seq,
        time::format_utc(policy.issued),
        time::format_kst(policy.issued)
    )];
    if let Some(contact) = &policy.contact_ko {
        lines.push(format!("안내 문구: {contact}"));
    }
    for (platform, e) in &policy.platforms {
        let mut line = format!("{platform}: 최소 {} · 최신 {}", e.min, e.latest);
        if let Some(t) = e.grace_until {
            line.push_str(&format!(" · 유예 끝 {}", time::format_kst(t)));
        }
        if let Some(os) = &e.os_min {
            line.push_str(&format!(" · OS 최소 {os}"));
        }
        if let Some(t) = e.auto_install_after {
            line.push_str(&format!(" · 무인 설치 시작 {}", time::format_kst(t)));
        }
        if let Some(code) = e.version_code {
            line.push_str(&format!(" · Play versionCode {code}"));
        }
        if let Some(build) = &e.testflight_build {
            line.push_str(&format!(" · TestFlight 빌드 {build}"));
        }
        if e.move_to_play {
            line.push_str(" · Play로 옮기기 안내");
        }
        lines.push(line);
        if let Some(p) = &e.package {
            lines.push(format!(
                "  패키지 {} · {}바이트 · build {} · sha256 {}",
                p.file, p.size, p.build, p.sha256
            ));
        }
        if let Some(notes) = &e.notes_ko {
            lines.push(format!("  알림: {}", notes.replace('\n', " / ")));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        keys::{test_key, test_key_set},
        policy::tests::{sample, t, v},
    };
    use ed25519_dalek::SigningKey;

    const SERVER: &str = "rd.example.com";

    fn signer(name: &str) -> (SigningKey, u32) {
        let key = test_key(name).unwrap();
        (SigningKey::from_bytes(&key.seed()), key.id)
    }

    fn envelope(policy: &Policy, by: &str, root: Option<(&RootDecl, &str)>) -> String {
        let (key, k) = signer(by);
        let root = root.map(|(decl, by)| {
            let (key, k) = signer(by);
            Part::sign(&key, k, ROOT_DOMAIN, &decl.to_text())
        });
        Envelope {
            policy: Part::sign(&key, k, POLICY_DOMAIN, &policy.to_text()),
            root,
        }
        .to_file()
    }

    fn policy(epoch: u64, seq: u64) -> Policy {
        Policy {
            epoch,
            seq,
            ..sample()
        }
    }

    fn decl(epoch: u64, revoked: &[u32]) -> RootDecl {
        RootDecl {
            server: SERVER.to_owned(),
            epoch,
            revoked: revoked.to_vec(),
            issued: t("2026-11-08T03:00:00Z"),
        }
    }

    fn after(previous: &[&str], file: &str) -> Result<State, String> {
        let keys = test_key_set();
        let mut state = State::initial(&keys);
        for p in previous {
            let checked = check_envelope(p, &keys, SERVER).unwrap();
            state = state.accept(&keys, &checked).unwrap();
        }
        let checked = check_envelope(file, &keys, SERVER)?;
        state.accept(&keys, &checked)
    }

    #[test]
    fn a_plain_policy_passes() {
        let keys = test_key_set();
        let file = envelope(&policy(1, 1), "signing-1", None);
        let checked = check_envelope(&file, &keys, SERVER).unwrap();
        assert_eq!(checked.role, Role::Signing);
        assert_eq!(checked.policy, policy(1, 1));
        let state = State::initial(&keys).accept(&keys, &checked).unwrap();
        assert_eq!((state.epoch, state.last.unwrap().policy.seq), (1, 1));
        // the next key signs as well
        assert!(after(&[&file], &envelope(&policy(1, 2), "signing-2", None)).is_ok());
        assert!(!describe(&policy(1, 1)).is_empty());
    }

    #[test]
    fn signatures_and_keys() {
        let keys = test_key_set();
        let good = envelope(&policy(1, 1), "signing-1", None);
        // one byte of the policy text
        let tampered = good.replacen("\\\"seq\\\":1", "\\\"seq\\\":2", 1);
        assert_ne!(tampered, good);
        assert!(check_envelope(&tampered, &keys, SERVER)
            .unwrap_err()
            .contains("서명"));
        // signed under another domain: the relay's wake commands, the root declaration
        for domain in [&b"hjv-wake-v1"[..], ROOT_DOMAIN, b"hjv-version-v2", b""] {
            let (key, k) = signer("signing-1");
            let file = Envelope {
                policy: Part::sign(&key, k, domain, &policy(1, 1).to_text()),
                root: None,
            }
            .to_file();
            assert!(check_envelope(&file, &keys, SERVER)
                .unwrap_err()
                .contains("서명"));
        }
        // a key the build does not trust, under its own number and under a trusted one
        let unknown = envelope(&policy(1, 1), "signing-3", None);
        assert!(check_envelope(&unknown, &keys, SERVER)
            .unwrap_err()
            .contains("믿는 키"));
        let (key, _) = signer("signing-3");
        let stolen_number = Envelope {
            policy: Part::sign(&key, 1, POLICY_DOMAIN, &policy(1, 1).to_text()),
            root: None,
        }
        .to_file();
        assert!(check_envelope(&stolen_number, &keys, SERVER)
            .unwrap_err()
            .contains("서명"));
        // another server's policy
        assert!(check_envelope(&good, &keys, "hjv.test")
            .unwrap_err()
            .contains("서버"));
        // not the tool's bytes
        assert!(check_envelope(&format!(" {good}"), &keys, SERVER).is_err());
        assert!(check_envelope(&format!("{good}\n"), &keys, SERVER).is_err());
    }

    #[test]
    fn root_declarations() {
        let keys = test_key_set();
        let d2 = decl(2, &[1]);
        // key 2 signs the new epoch; key 1 is revoked
        let good = envelope(&policy(2, 1), "signing-2", Some((&d2, "root-101")));
        let checked = check_envelope(&good, &keys, SERVER).unwrap();
        assert_eq!(checked.root.as_ref().unwrap().revoked, [1]);
        let state = State::initial(&keys).accept(&keys, &checked).unwrap();
        assert_eq!(state.epoch, 2);
        assert!(state.revoked.contains(&1));

        let revoked = envelope(&policy(2, 1), "signing-1", Some((&d2, "root-101")));
        assert!(check_envelope(&revoked, &keys, SERVER)
            .unwrap_err()
            .contains("폐기"));
        // a declaration a signing key made, or another root
        for by in ["signing-1", "signing-2", "root-102"] {
            let file = envelope(&policy(2, 1), "signing-2", Some((&d2, by)));
            assert!(check_envelope(&file, &keys, SERVER).is_err(), "{by}");
        }
        // the declaration signed under the policy's domain
        let (root_key, root_id) = signer("root-101");
        let (key, k) = signer("signing-2");
        let wrong_domain = Envelope {
            policy: Part::sign(&key, k, POLICY_DOMAIN, &policy(2, 1).to_text()),
            root: Some(Part::sign(&root_key, root_id, POLICY_DOMAIN, &d2.to_text())),
        }
        .to_file();
        assert!(check_envelope(&wrong_domain, &keys, SERVER)
            .unwrap_err()
            .contains("루트 선언의 서명"));
        // the root key's declaration under another key number
        let misnumbered = Envelope {
            policy: Part::sign(&key, k, POLICY_DOMAIN, &policy(2, 1).to_text()),
            root: Some(Part::sign(&root_key, 2, ROOT_DOMAIN, &d2.to_text())),
        }
        .to_file();
        assert!(check_envelope(&misnumbered, &keys, SERVER)
            .unwrap_err()
            .contains("루트 키 번호(101)와 다릅니다"));
        // revoking the root key itself
        let file = envelope(
            &policy(2, 1),
            "signing-2",
            Some((&decl(2, &[1, 101]), "root-101")),
        );
        assert!(check_envelope(&file, &keys, SERVER)
            .unwrap_err()
            .contains("루트 키 번호"));
        // another server's declaration
        let other = RootDecl {
            server: "hjv.test".to_owned(),
            ..d2.clone()
        };
        let file = envelope(&policy(2, 1), "signing-2", Some((&other, "root-101")));
        assert!(check_envelope(&file, &keys, SERVER)
            .unwrap_err()
            .contains("서버"));
        // a signing key's policy must be of the declaration's epoch
        for epoch in [1, 3] {
            let file = envelope(&policy(epoch, 1), "signing-2", Some((&d2, "root-101")));
            assert!(check_envelope(&file, &keys, SERVER)
                .unwrap_err()
                .contains("세대"));
        }
    }

    #[test]
    fn epochs_and_revocations_are_kept() {
        let s1 = envelope(&policy(1, 5), "signing-1", None);
        let d2 = decl(2, &[1]);
        let e2 = envelope(&policy(2, 1), "signing-2", Some((&d2, "root-101")));
        // a higher epoch starts seq again
        let state = after(&[&s1], &e2).unwrap();
        assert_eq!(
            (state.epoch, state.last.as_ref().unwrap().policy.seq),
            (2, 1)
        );
        // key 1 stays revoked without the declaration, and the old epoch is gone
        let old = envelope(&policy(2, 2), "signing-1", None);
        assert!(after(&[&s1, &e2], &old).unwrap_err().contains("폐기"));
        let old_epoch = envelope(&policy(1, 6), "signing-2", None);
        assert!(after(&[&s1, &e2], &old_epoch).unwrap_err().contains("세대"));
        // an older declaration after a newer one
        let d1 = decl(1, &[]);
        let back = envelope(&policy(1, 6), "signing-2", Some((&d1, "root-101")));
        assert!(after(&[&s1, &e2], &back).unwrap_err().contains("세대"));
        // revocations add up: a later declaration that lists less does not bring key 1 back
        let d3 = decl(3, &[]);
        let e3 = envelope(&policy(3, 1), "signing-2", Some((&d3, "root-101")));
        let state = after(&[&s1, &e2], &e3).unwrap();
        assert!(state.revoked.contains(&1));
        let by_1 = envelope(&policy(3, 2), "signing-1", Some((&d3, "root-101")));
        assert!(after(&[&s1, &e2, &e3], &by_1).unwrap_err().contains("폐기"));
        // a revocation in the same epoch
        let d1r = decl(1, &[2]);
        let e = envelope(&policy(1, 6), "signing-1", Some((&d1r, "root-101")));
        let state = after(&[&s1], &e).unwrap();
        assert_eq!(state.epoch, 1);
        assert!(state.revoked.contains(&2));
    }

    #[test]
    fn seq_rules() {
        let s5 = envelope(&policy(1, 5), "signing-1", None);
        // the same position: the same bytes pass, other bytes do not
        assert!(after(&[&s5], &s5).is_ok());
        let mut other = policy(1, 5);
        other.contact_ko = None;
        assert!(after(&[&s5], &envelope(&other, "signing-1", None))
            .unwrap_err()
            .contains("같은 위치"));
        // lower, and too far ahead for a signing key
        assert!(after(&[&s5], &envelope(&policy(1, 4), "signing-1", None)).is_err());
        assert!(after(&[&s5], &envelope(&policy(1, 1005), "signing-1", None)).is_ok());
        assert!(
            after(&[&s5], &envelope(&policy(1, 1006), "signing-1", None))
                .unwrap_err()
                .contains("1000")
        );
        // the root key may jump
        assert!(after(&[&s5], &envelope(&policy(1, 1006), "root-101", None)).is_ok());
        assert!(after(&[&s5], &envelope(&policy(1, 5_000_000), "root-101", None)).is_ok());
        assert!(after(&[&s5], &envelope(&policy(1, 4), "root-101", None)).is_err());
        // a fresh install: up to 1000 above the floor
        assert!(after(&[], &envelope(&policy(1, 1000), "signing-1", None)).is_ok());
        assert!(after(&[], &envelope(&policy(1, 1001), "signing-1", None)).is_err());
        // the root key's policy may carry a higher epoch by itself
        let state = after(&[&s5], &envelope(&policy(2, 1), "root-101", None)).unwrap();
        assert_eq!(state.epoch, 2);
        assert!(after(&[&s5], &envelope(&policy(2, 1), "signing-1", None)).is_err());
    }

    #[test]
    fn the_floor() {
        let mut keys = test_key_set();
        keys.floor = Floor { epoch: 2, seq: 7 };
        let check = |p: &Policy, by: &str, root: Option<(&RootDecl, &str)>| {
            let file = envelope(p, by, root);
            let checked = check_envelope(&file, &keys, SERVER)?;
            State::initial(&keys).accept(&keys, &checked)
        };
        let d2 = decl(2, &[]);
        // the policy the floor names passes; older ones do not
        assert!(check(&policy(2, 7), "signing-1", Some((&d2, "root-101"))).is_ok());
        assert!(check(&policy(2, 7), "signing-1", None).is_ok());
        assert!(check(&policy(2, 6), "signing-1", None)
            .unwrap_err()
            .contains("하한"));
        assert!(check(&policy(1, 900), "signing-1", None)
            .unwrap_err()
            .contains("하한"));
        assert!(check(&policy(1, 900), "root-101", None)
            .unwrap_err()
            .contains("하한"));
        assert!(check(&policy(2, 1006), "signing-1", None).is_ok());
        assert!(check(&policy(2, 1007), "signing-1", None).is_err());
        // an epoch above the floor's starts from 1
        let d3 = decl(3, &[]);
        assert!(check(&policy(3, 1), "signing-1", Some((&d3, "root-101"))).is_ok());
        assert!(check(&policy(3, 1001), "signing-1", Some((&d3, "root-101"))).is_err());
    }

    fn relaxed(change: impl Fn(&mut Policy)) -> (String, String, Vec<Relaxation>) {
        let before = policy(1, 5);
        let mut after = policy(1, 6);
        change(&mut after);
        after.check().unwrap();
        let list = relaxations(&before, &after);
        (
            envelope(&after, "signing-1", None),
            envelope(&after, "root-101", None),
            list,
        )
    }

    #[test]
    fn only_the_root_key_relaxes() {
        let s5 = envelope(&policy(1, 5), "signing-1", None);
        type Change = Box<dyn Fn(&mut Policy)>;
        let cases: Vec<(Change, Relaxation)> = vec![
            (
                Box::new(|p| p.platforms.get_mut(&Platform::Ios).unwrap().min = v("1.5.1")),
                Relaxation::MinLowered(Platform::Ios),
            ),
            (
                Box::new(|p| {
                    p.platforms.get_mut(&Platform::Windows).unwrap().grace_until =
                        Some(t("2026-11-22T15:00:01Z"))
                }),
                Relaxation::GraceLater(Platform::Windows),
            ),
            (
                // a grace period where there was none
                Box::new(|p| {
                    p.platforms.get_mut(&Platform::Ios).unwrap().grace_until =
                        Some(t("2026-11-01T00:00:00Z"))
                }),
                Relaxation::GraceLater(Platform::Ios),
            ),
            (
                Box::new(|p| {
                    let e = p.platforms.get_mut(&Platform::Ios).unwrap();
                    e.min = v("1.5.9");
                    e.latest = v("1.6.0");
                }),
                Relaxation::MinLowered(Platform::Ios),
            ),
            (
                Box::new(|p| drop(p.platforms.remove(&Platform::AndroidPlay))),
                Relaxation::Removed(Platform::AndroidPlay),
            ),
        ];
        for (change, expected) in &cases {
            let (by_signing, by_root, list) = relaxed(change);
            assert!(list.contains(expected), "{expected:?} in {list:?}");
            let err = after(&[&s5], &by_signing).unwrap_err();
            assert!(err.contains("루트 키가 필요"), "{err}");
            assert!(
                after(&[&s5], &by_root).is_ok(),
                "{expected:?} by the root key"
            );
        }
        // latest lowered (with min following it down)
        let (by_signing, by_root, list) = relaxed(|p| {
            let e = p.platforms.get_mut(&Platform::Ios).unwrap();
            e.latest = v("1.6.0");
        });
        assert_eq!(list, [Relaxation::LatestLowered(Platform::Ios)]);
        assert!(after(&[&s5], &by_signing).is_err());
        assert!(after(&[&s5], &by_root).is_ok());
    }

    #[test]
    fn what_is_not_a_relaxation() {
        let s5 = envelope(&policy(1, 5), "signing-1", None);
        let ok = |change: &dyn Fn(&mut Policy)| {
            let (by_signing, _, list) = relaxed(change);
            assert_eq!(list, []);
            after(&[&s5], &by_signing).unwrap();
        };
        fn windows(p: &mut Policy) -> &mut crate::policy::Entry {
            p.platforms.get_mut(&Platform::Windows).unwrap()
        }
        // the emergency stop for unattended installs, and delaying them
        ok(&|p| windows(p).auto_install_after = None);
        ok(&|p| windows(p).auto_install_after = Some(t("2027-01-01T00:00:00Z")));
        // tightening: an earlier deadline or none, a higher min (even with a new grace period)
        ok(&|p| windows(p).grace_until = Some(t("2026-11-20T15:00:00Z")));
        ok(&|p| windows(p).grace_until = None);
        ok(&|p| {
            let e = windows(p);
            e.min = v("1.6.1");
            e.grace_until = Some(t("2027-01-01T00:00:00Z"));
        });
        // a higher latest, new text, a new entry
        ok(&|p| windows(p).latest = v("1.6.2"));
        ok(&|p| p.contact_ko = None);
        ok(&|p| windows(p).notes_ko = None);
        // an entry that was not there before
        let mut before = policy(1, 5);
        before.platforms.remove(&Platform::AndroidPlay);
        assert_eq!(relaxations(&before, &policy(1, 6)), []);
    }

    #[test]
    fn a_root_policy_lifts_a_required_update() {
        // design 7: the update path is down, so the root key lowers min for a while
        let mut strict = policy(1, 5);
        for e in strict.platforms.values_mut() {
            e.min = e.latest;
            e.grace_until = None;
        }
        let strict_file = envelope(&strict, "signing-1", None);
        let mut eased = policy(1, 6);
        for e in eased.platforms.values_mut() {
            e.min = v("1.5.0");
            e.grace_until = None;
        }
        assert!(after(&[&strict_file], &envelope(&eased, "signing-1", None)).is_err());
        let state = after(&[&strict_file], &envelope(&eased, "root-101", None)).unwrap();
        assert_eq!(state.last.unwrap().policy.lowest_min(), Some(v("1.5.0")));
        // and a signing key raises it again afterwards
        let eased_file = envelope(&eased, "root-101", None);
        let mut again = strict.clone();
        again.seq = 7;
        assert!(after(
            &[&strict_file, &eased_file],
            &envelope(&again, "signing-1", None)
        )
        .is_ok());
    }

    #[test]
    fn check_after_names_the_previous_policy() {
        let keys = test_key_set();
        let s5 = envelope(&policy(1, 5), "signing-1", None);
        let s6 = envelope(&policy(1, 6), "signing-1", None);
        assert!(check_after(&s6, Some(&s5), &keys, SERVER).is_ok());
        assert!(check_after(&s5, Some(&s6), &keys, SERVER).is_err());
        assert!(check_after(&s6, Some("{}"), &keys, SERVER)
            .unwrap_err()
            .contains("이전 정책"));
        assert!(check_after(&s6, None, &keys, SERVER).is_ok());
    }

    #[test]
    fn too_large_for_the_apps() {
        let keys = test_key_set();
        let mut big = policy(1, 1);
        for e in big.platforms.values_mut() {
            e.notes_ko = Some("가".repeat(crate::policy::MAX_NOTES_CHARS));
        }
        big.check().unwrap();
        let file = envelope(&big, "signing-1", None);
        assert!(file.len() > crate::envelope::MAX_ENVELOPE_BYTES);
        assert!(check_envelope(&file, &keys, SERVER)
            .unwrap_err()
            .contains("바이트"));
    }
}

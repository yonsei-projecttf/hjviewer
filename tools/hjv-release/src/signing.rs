// SPDX-License-Identifier: AGPL-3.0-only

//! `hjv-release sign`, without the terminal: what a draft would change (shown to the person
//! before any key is opened) and the signing itself.
//!
//! The draft is checked again from its texts; nothing an agent wrote next to them is believed.
//! Key revocations, relaxations, a new epoch and a jump in `seq` are listed as dangers: the
//! command asks the person to type `yes` to each list (design 6-3). So is a new deadline that
//! has passed since the draft was made, because the grace period or the head start it promised
//! would be gone when the policy arrives. After signing, the envelope goes through the apps' own
//! checks (trust.rs) against the previous policy, with the public keys of
//! `client/update-keys.env`; a file is written only if an app would take it.

use crate::{
    draft::Draft,
    envelope::{Envelope, Part, POLICY_DOMAIN, ROOT_DOMAIN},
    keyfile::SecretKey,
    keys::{KeySet, Role},
    make::{describe_changes, SEQ_JUMP_TO_CONFIRM},
    policy::{Policy, RootDecl},
    time,
    trust::{check_envelope, check_root, describe, relaxations, Checked, State},
};

/// How far ahead of this PC's clock a draft's times may be.
const CLOCK_SLACK: i64 = 300;

#[derive(Debug)]
pub struct Plan {
    pub policy: Policy,
    pub prev: Option<Checked>,
    /// A root declaration to sign now.
    pub new_root: Option<RootDecl>,
    /// What needs typed consent.
    pub dangers: Vec<String>,
    /// What the signed policy says and how it differs from the previous one.
    pub summary: Vec<String>,
}

impl Plan {
    /// Whether the root key is needed, for the declaration or for the policy.
    pub fn needs_root(&self, draft: &Draft) -> bool {
        self.new_root.is_some() || draft.signer == Role::Root
    }
}

/// Checks a draft and says what signing it would do.
pub fn plan(draft: &Draft, keys: &KeySet, now: i64) -> Result<Plan, String> {
    let policy = Policy::parse(&draft.p).map_err(|e| format!("초안의 정책: {e}"))?;
    let server = policy.server.clone();
    let prev = draft
        .prev
        .as_deref()
        .map(|file| check_envelope(file, keys, &server))
        .transpose()
        .map_err(|e| format!("초안의 이전 정책: {e}"))?;
    let old = prev.as_ref().map(|c| &c.policy);
    let old_root = prev.as_ref().and_then(|c| c.root.as_ref());

    if policy.issued > now + CLOCK_SLACK {
        return Err(format!(
            "정책의 발행 시각({})이 이 PC의 시계보다 미래입니다. 앱은 발행 시각을 '지금'의 하한으로 \
             쓰므로 서명하지 않습니다",
            time::format_utc(policy.issued)
        ));
    }
    if old.is_some_and(|old| policy.issued < old.issued) {
        return Err("정책의 발행 시각이 이전 정책보다 이릅니다".to_owned());
    }

    let mut dangers = Vec::new();
    let new_root = match &draft.root_p {
        Some(text) => {
            let decl = RootDecl::parse(text).map_err(|e| format!("초안의 새 루트 선언: {e}"))?;
            if decl.server != server {
                return Err("새 루트 선언의 서버가 정책의 서버와 다릅니다".to_owned());
            }
            if decl.revoked.contains(&keys.root.id) {
                return Err("새 루트 선언이 루트 키 번호를 폐기합니다".to_owned());
            }
            if decl.issued > now + CLOCK_SLACK {
                return Err("새 루트 선언의 발행 시각이 미래입니다".to_owned());
            }
            let before: &[u32] = old_root.map_or(&[], |r| &r.revoked);
            if before.iter().any(|id| !decl.revoked.contains(id)) {
                return Err(
                    "새 루트 선언이 이전에 폐기한 키를 빼먹었습니다(폐기 목록은 쌓이기만 합니다)"
                        .to_owned(),
                );
            }
            let old_epoch = old.map_or(keys.floor.epoch, |old| old.epoch);
            if decl.epoch < old_epoch {
                return Err("새 루트 선언의 세대가 이전보다 낮습니다".to_owned());
            }
            for id in decl.revoked.iter().filter(|id| !before.contains(id)) {
                dangers.push(format!(
                    "서명 키(번호 {id})를 폐기합니다. 지금 키를 박은 앱은 이 키를 다시 믿지 않습니다(되돌릴 수 없음)"
                ));
            }
            if decl.epoch > old_epoch {
                dangers.push(format!(
                    "세대(epoch)를 올립니다({old_epoch} → {}). seq는 1부터 다시 시작합니다",
                    decl.epoch
                ));
            }
            Some(decl)
        }
        None => {
            // the last declaration goes along unchanged, so that a device that was off learns it
            let carried = prev.as_ref().and_then(|c| c.envelope.root.as_ref());
            if draft.root.as_ref() != carried {
                return Err("초안에 붙은 루트 선언이 이전 정책의 것과 다릅니다".to_owned());
            }
            if let Some(part) = &draft.root {
                check_root(part, keys, &server)?;
            }
            None
        }
    };
    let root_epoch = new_root.as_ref().or(old_root).map(|r| r.epoch);
    if draft.signer == Role::Signing && root_epoch.is_some_and(|e| e != policy.epoch) {
        return Err("정책의 세대가 루트 선언의 세대와 다릅니다".to_owned());
    }
    if old.is_some_and(|old| policy.epoch > old.epoch) && new_root.is_none() {
        return Err("세대를 올리려면 새 루트 선언이 있어야 합니다(policy --new-epoch)".to_owned());
    }

    // a new policy has a new position, even where an app would take the same bytes again
    if let Some(old) = old {
        if policy.epoch < old.epoch || (policy.epoch == old.epoch && policy.seq <= old.seq) {
            return Err(format!(
                "정책 위치({}:{})가 이전 정책({}:{})보다 높지 않습니다",
                policy.epoch, policy.seq, old.epoch, old.seq
            ));
        }
    }

    let relaxed = old.map(|old| relaxations(old, &policy)).unwrap_or_default();
    if !relaxed.is_empty() && draft.signer != Role::Root {
        return Err(
            "초안이 완화를 담고 있는데 서명 키로 서명하게 되어 있습니다. policy --relax로 다시 만드세요".to_owned(),
        );
    }
    for relaxation in &relaxed {
        dangers.push(format!("완화: {}", relaxation.describe()));
    }
    if draft.signer == Role::Root && relaxed.is_empty() {
        dangers.push(
            "정책을 루트 키로 서명합니다. 앱은 루트 키의 정책을 완화인지 따지지 않고 받습니다"
                .to_owned(),
        );
    }
    let base = match old {
        Some(old) if old.epoch == policy.epoch => Some(old.seq),
        Some(_) => None,
        None => Some(keys.floor.seq),
    };
    if let Some(base) = base.filter(|base| policy.seq > base + SEQ_JUMP_TO_CONFIRM) {
        dangers.push(format!(
            "seq가 한 번에 뜁니다({base} → {}, {}만큼). 보통은 1씩 오릅니다",
            policy.seq,
            policy.seq - base
        ));
    }

    // A draft is signed later than it was made, by another person. What was a grace period or a
    // head start for the operator then may be shorter now, or gone: the signer sees how long each
    // new deadline still is from this moment, and one that has passed needs consent.
    let mut remaining = Vec::new();
    for (platform, entry) in &policy.platforms {
        let before = old.and_then(|old| old.platforms.get(platform));
        let new_deadline = entry
            .grace_until
            .filter(|g| before.is_none_or(|b| b.min != entry.min || b.grace_until != Some(*g)));
        match new_deadline {
            Some(deadline) if deadline <= now => dangers.push(format!(
                "{platform}: 유예 끝({})이 이미 지났습니다. 서버에 넣는 즉시 {} 미만을 막습니다. \
                 초안을 만든 뒤 시간이 지났다면 policy로 다시 만드세요",
                time::format_kst(deadline),
                entry.min
            )),
            Some(deadline) => remaining.push(format!(
                "{platform}: 유예 끝까지 {}",
                time::format_span(deadline - now)
            )),
            None => {}
        }
        let new_start = entry.auto_install_after.filter(|a| {
            before.is_none_or(|b| b.latest != entry.latest || b.auto_install_after != Some(*a))
        });
        match new_start {
            // a start at the issue time itself was asked for (`--auto-after windows=+0d`)
            Some(start) if start <= now && start > policy.issued => dangers.push(format!(
                "{platform}: 무인 설치 시작({})이 이미 지났습니다. 서버에 넣는 즉시 설치된 PC가 고리 0부터 \
                 설치합니다. 초안을 만든 뒤 시간이 지났다면 policy로 다시 만드세요",
                time::format_kst(start)
            )),
            Some(start) if start > now => remaining.push(format!(
                "{platform}: 무인 설치 시작까지 {}",
                time::format_span(start - now)
            )),
            _ => {}
        }
    }

    let mut summary = describe(&policy);
    summary.push("바뀌는 것:".to_owned());
    summary.extend(
        describe_changes(old, &policy)
            .into_iter()
            .map(|l| format!("  {l}")),
    );
    if !remaining.is_empty() {
        summary.push("지금부터 남은 시간:".to_owned());
        summary.extend(remaining.into_iter().map(|l| format!("  {l}")));
    }
    if !draft.notes.is_empty() {
        summary.push("policy가 확인한 것(초안에 적힌 대로):".to_owned());
        summary.extend(draft.notes.iter().map(|l| format!("  {l}")));
    }
    Ok(Plan {
        policy,
        prev,
        new_root,
        dangers,
        summary,
    })
}

/// Signs the draft with the opened keys and returns the envelope file, after an app's checks.
pub fn sign(
    draft: &Draft,
    plan: &Plan,
    secrets: &[SecretKey],
    keys: &KeySet,
) -> Result<String, String> {
    // a key the builds do not hold under that number and role would sign for nobody
    for secret in secrets {
        match keys.find(secret.id) {
            Some((role, public)) if role == secret.role && public.key == secret.verifying_key() => {
            }
            Some(_) => {
                return Err(format!(
                    "키 파일의 키({} {})가 update-keys.env에 있는 번호 {}의 키와 다릅니다",
                    secret.role.korean(),
                    secret.id,
                    secret.id
                ))
            }
            None => {
                return Err(format!(
                    "이 키({} {})는 update-keys.env에 없습니다. 이 키로 서명한 정책은 앱이 받지 않습니다",
                    secret.role.korean(),
                    secret.id
                ))
            }
        }
    }
    let of = |role: Role| -> Result<Option<&SecretKey>, String> {
        let mut found = secrets.iter().filter(|s| s.role == role);
        match (found.next(), found.next()) {
            (first, None) => Ok(first),
            _ => Err(format!("{}: 하나만 주세요", role.korean())),
        }
    };
    let (signing_key, root_key) = (of(Role::Signing)?, of(Role::Root)?);

    let root = match &draft.root_p {
        Some(text) => {
            let key =
                root_key.ok_or("새 루트 선언은 루트 키로 서명합니다: --key <루트 키 파일>")?;
            Some(Part::sign(key.signing_key(), key.id, ROOT_DOMAIN, text))
        }
        None => draft.root.clone(),
    };
    let key = match draft.signer {
        Role::Root => root_key.ok_or("이 정책은 루트 키로 서명합니다(완화): --key <루트 키 파일>")?,
        Role::Signing => signing_key.ok_or(
            "이 정책은 서명 키로 서명합니다: --key <서명 키 파일>(루트 키는 루트 선언과 완화에만 씁니다)",
        )?,
    };
    let file = Envelope {
        policy: Part::sign(key.signing_key(), key.id, POLICY_DOMAIN, &draft.p),
        root,
    }
    .to_file();

    let rejected = |e: String| format!("서명한 정책을 앱이 받지 않으므로 쓰지 않습니다: {e}");
    let checked = check_envelope(&file, keys, &plan.policy.server).map_err(rejected)?;
    let state = plan
        .prev
        .as_ref()
        .map_or_else(|| State::initial(keys), State::holding);
    state.accept(keys, &checked).map_err(rejected)?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        keys::{test_key, test_key_set, Floor},
        policy::tests::{sample, t, v},
        policy::Platform,
    };

    const NOW: &str = "2026-11-08T04:00:00Z";

    fn secret(name: &str) -> SecretKey {
        let key = test_key(name).unwrap();
        SecretKey::from_seed(key.role, key.id, &key.seed())
    }

    fn draft(policy: &Policy, prev: Option<&str>) -> Draft {
        Draft {
            p: policy.to_text(),
            signer: Role::Signing,
            root: prev.and_then(|p| Envelope::parse(p).unwrap().root),
            root_p: None,
            prev: prev.map(str::to_owned),
            notes: vec!["windows 1.6.1: 실행 4242 성공".to_owned()],
        }
    }

    fn policy(seq: u64) -> Policy {
        Policy { seq, ..sample() }
    }

    fn signed(draft: &Draft, with: &[&str]) -> Result<String, String> {
        let keys = test_key_set();
        let plan = plan(draft, &keys, t(NOW))?;
        let secrets: Vec<SecretKey> = with.iter().map(|n| secret(n)).collect();
        sign(draft, &plan, &secrets, &keys)
    }

    #[test]
    fn a_first_policy_and_the_next() {
        let keys = test_key_set();
        let first = draft(&policy(1), None);
        let p = plan(&first, &keys, t(NOW)).unwrap();
        assert_eq!(p.dangers, Vec::<String>::new());
        assert!(!p.needs_root(&first));
        assert!(p.summary.iter().any(|l| l.contains("첫 정책")));
        assert!(p.summary.iter().any(|l| l.contains("실행 4242")));
        let file1 = signed(&first, &["signing-1"]).unwrap();
        let checked = check_envelope(&file1, &keys, "rd.example.com").unwrap();
        assert_eq!(checked.envelope.policy.k, 1);
        // Ed25519 signatures are deterministic: signing again gives the same file
        assert_eq!(signed(&first, &["signing-1"]).unwrap(), file1);

        let mut next = policy(2);
        next.platforms.get_mut(&Platform::Windows).unwrap().latest = v("1.6.2");
        let second = draft(&next, Some(&file1));
        let p = plan(&second, &keys, t(NOW)).unwrap();
        assert_eq!(p.dangers, Vec::<String>::new());
        assert!(p.summary.iter().any(|l| l.contains("최신 1.6.1 → 1.6.2")));
        let file2 = signed(&second, &["signing-2"]).unwrap();
        assert_eq!(
            check_envelope(&file2, &keys, "rd.example.com")
                .unwrap()
                .envelope
                .policy
                .k,
            2
        );
    }

    #[test]
    fn the_right_key_for_the_job() {
        let first = draft(&policy(1), None);
        // the root key does not sign an ordinary policy, and a policy needs some key
        assert!(signed(&first, &["root-101"])
            .unwrap_err()
            .contains("서명 키로 서명합니다"));
        assert!(signed(&first, &[])
            .unwrap_err()
            .contains("서명 키로 서명합니다"));
        assert!(signed(&first, &["signing-1", "signing-2"])
            .unwrap_err()
            .contains("하나만"));
        // keys the builds do not hold
        assert!(signed(&first, &["signing-3"])
            .unwrap_err()
            .contains("update-keys.env에 없습니다"));
        assert!(signed(&first, &["root-102"])
            .unwrap_err()
            .contains("update-keys.env에 없습니다"));
        // the right number with another key behind it
        let keys = test_key_set();
        let plan = plan(&first, &keys, t(NOW)).unwrap();
        let wrong = SecretKey::from_seed(Role::Signing, 1, &test_key("signing-3").unwrap().seed());
        assert!(sign(&first, &plan, &[wrong], &keys)
            .unwrap_err()
            .contains("키와 다릅니다"));
        let wrong_role =
            SecretKey::from_seed(Role::Root, 1, &test_key("signing-1").unwrap().seed());
        assert!(sign(&first, &plan, &[wrong_role], &keys)
            .unwrap_err()
            .contains("키와 다릅니다"));
    }

    #[test]
    fn relaxing_needs_the_root_key_and_consent() {
        let keys = test_key_set();
        let file1 = signed(&draft(&policy(1), None), &["signing-1"]).unwrap();
        let mut eased = policy(2);
        eased.platforms.get_mut(&Platform::Ios).unwrap().min = v("1.5.0");
        // marked for a signing key: refused before any key is asked for
        let as_signing = draft(&eased, Some(&file1));
        assert!(plan(&as_signing, &keys, t(NOW))
            .unwrap_err()
            .contains("완화"));
        let as_root = Draft {
            signer: Role::Root,
            ..as_signing
        };
        let p = plan(&as_root, &keys, t(NOW)).unwrap();
        assert_eq!(p.dangers.len(), 1);
        assert!(p.dangers[0].contains("ios: min을 내림"));
        assert!(p.needs_root(&as_root));
        assert!(signed(&as_root, &["signing-1"])
            .unwrap_err()
            .contains("루트 키로 서명합니다"));
        let file2 = signed(&as_root, &["root-101"]).unwrap();
        assert_eq!(
            check_envelope(&file2, &keys, "rd.example.com")
                .unwrap()
                .role,
            Role::Root
        );
        // a root-signed policy that relaxes nothing still asks
        let plain_root = Draft {
            signer: Role::Root,
            ..draft(&policy(2), Some(&file1))
        };
        let p = plan(&plain_root, &keys, t(NOW)).unwrap();
        assert!(p.dangers[0].contains("루트 키로 서명"));
    }

    #[test]
    fn revocation_and_a_new_epoch() {
        let keys = test_key_set();
        let file1 = signed(&draft(&policy(5), None), &["signing-1"]).unwrap();
        let decl = RootDecl {
            server: "rd.example.com".to_owned(),
            epoch: 2,
            revoked: vec![1],
            issued: t(NOW),
        };
        let next = Policy {
            epoch: 2,
            seq: 1,
            ..sample()
        };
        let d = Draft {
            root_p: Some(decl.to_text()),
            ..draft(&next, Some(&file1))
        };
        let p = plan(&d, &keys, t(NOW)).unwrap();
        assert_eq!(p.dangers.len(), 2);
        assert!(p.dangers[0].contains("서명 키(번호 1)를 폐기"));
        assert!(p.dangers[1].contains("세대(epoch)를 올립니다(1 → 2)"));
        assert!(p.needs_root(&d));
        // both keys are needed, and the revoked key cannot sign the policy
        assert!(signed(&d, &["signing-2"])
            .unwrap_err()
            .contains("루트 선언은 루트 키"));
        assert!(signed(&d, &["root-101"])
            .unwrap_err()
            .contains("서명 키로 서명합니다"));
        assert!(signed(&d, &["root-101", "signing-1"])
            .unwrap_err()
            .contains("폐기"));
        let file2 = signed(&d, &["root-101", "signing-2"]).unwrap();
        let checked = check_envelope(&file2, &keys, "rd.example.com").unwrap();
        assert_eq!(checked.root.as_ref().unwrap().revoked, [1]);

        // from now on the declaration goes along, untouched
        let third = Policy {
            epoch: 2,
            seq: 2,
            ..sample()
        };
        let carried = draft(&third, Some(&file2));
        assert!(carried.root.is_some());
        let file3 = signed(&carried, &["signing-2"]).unwrap();
        assert_eq!(
            Envelope::parse(&file3).unwrap().root,
            Envelope::parse(&file2).unwrap().root
        );
        // a draft that drops it, or carries another one, is refused
        let dropped = Draft {
            root: None,
            ..carried.clone()
        };
        assert!(plan(&dropped, &keys, t(NOW))
            .unwrap_err()
            .contains("루트 선언"));
        // a new declaration may not forget a revocation or go back an epoch
        for (epoch, revoked) in [(2, vec![]), (2, vec![2]), (1, vec![1])] {
            let bad = RootDecl {
                epoch,
                revoked,
                ..decl.clone()
            };
            let d = Draft {
                root: None,
                root_p: Some(bad.to_text()),
                ..carried.clone()
            };
            assert!(plan(&d, &keys, t(NOW)).is_err(), "epoch {epoch}");
        }
        // nor revoke the root key
        let bad = RootDecl {
            revoked: vec![1, 101],
            ..decl.clone()
        };
        let d = Draft {
            root: None,
            root_p: Some(bad.to_text()),
            ..carried
        };
        assert!(plan(&d, &keys, t(NOW))
            .unwrap_err()
            .contains("루트 키 번호"));
    }

    #[test]
    fn positions_and_times() {
        let keys = test_key_set();
        let file1 = signed(&draft(&policy(5), None), &["signing-1"]).unwrap();
        // a jump of more than 10 asks; more than 1000 no signing key may make
        let jump = draft(&policy(16), Some(&file1));
        let p = plan(&jump, &keys, t(NOW)).unwrap();
        assert!(p.dangers[0].contains("seq가 한 번에 뜁니다(5 → 16, 11만큼)"));
        assert!(plan(&draft(&policy(15), Some(&file1)), &keys, t(NOW))
            .unwrap()
            .dangers
            .is_empty());
        assert!(signed(&draft(&policy(1006), Some(&file1)), &["signing-1"])
            .unwrap_err()
            .contains("1000"));
        // not above the previous one
        assert!(signed(&draft(&policy(5), Some(&file1)), &["signing-1"]).is_err());
        assert!(signed(&draft(&policy(4), Some(&file1)), &["signing-1"]).is_err());
        // a higher epoch without a declaration
        let higher = Policy {
            epoch: 2,
            seq: 1,
            ..sample()
        };
        assert!(plan(&draft(&higher, Some(&file1)), &keys, t(NOW))
            .unwrap_err()
            .contains("새 루트 선언"));
        // issued in the future, or before the previous policy
        let now = t("2026-11-08T02:00:00Z");
        assert!(plan(&draft(&policy(6), Some(&file1)), &keys, now)
            .unwrap_err()
            .contains("미래"));
        assert!(plan(
            &draft(&policy(6), Some(&file1)),
            &keys,
            t("2026-11-08T02:56:00Z")
        )
        .is_ok());
        let mut earlier = policy(6);
        earlier.issued -= 1;
        assert!(plan(&draft(&earlier, Some(&file1)), &keys, t(NOW))
            .unwrap_err()
            .contains("이전 정책보다"));
        // below the floor of the key file
        let mut high_floor = test_key_set();
        high_floor.floor = Floor { epoch: 1, seq: 9 };
        let p = plan(&draft(&policy(8), None), &high_floor, t(NOW)).unwrap();
        let err = sign(
            &draft(&policy(8), None),
            &p,
            &[secret("signing-1")],
            &high_floor,
        )
        .unwrap_err();
        assert!(err.contains("하한"), "{err}");
    }

    #[test]
    fn a_draft_that_waited() {
        let keys = test_key_set();
        let lines = |p: &Plan| -> Vec<String> {
            let at = p.summary.iter().position(|l| l == "지금부터 남은 시간:");
            at.map(|at| {
                let rest = p.summary[at + 1..].iter();
                rest.take_while(|l| l.starts_with("  ")).cloned().collect()
            })
            .unwrap_or_default()
        };
        // a first policy: every deadline is new. Signed an hour after it was made, the signer
        // reads how long each of them still is
        let first = draft(&policy(1), None);
        let p = plan(&first, &keys, t(NOW)).unwrap();
        assert_eq!(p.dangers, Vec::<String>::new());
        assert_eq!(
            lines(&p),
            [
                "  windows: 유예 끝까지 14일 11시간",
                "  windows: 무인 설치 시작까지 1일 23시간"
            ]
        );
        // three days later the operator's head start is gone: the signer must say yes
        let p = plan(&first, &keys, t("2026-11-11T04:00:00Z")).unwrap();
        assert_eq!(p.dangers.len(), 1, "{:?}", p.dangers);
        assert!(p.dangers[0]
            .contains("windows: 무인 설치 시작(2026-11-10 12:00 한국 시각)이 이미 지났습니다"));
        assert_eq!(lines(&p), ["  windows: 유예 끝까지 11일 11시간"]);
        // after the grace period the minimum would be enforced on arrival
        let later = t("2026-11-23T00:00:00Z");
        let p = plan(&first, &keys, later).unwrap();
        assert_eq!(p.dangers.len(), 2, "{:?}", p.dangers);
        assert!(p.dangers[0].contains(
            "windows: 유예 끝(2026-11-23 00:00 한국 시각)이 이미 지났습니다. 서버에 넣는 즉시 1.6.0 미만을 막습니다"
        ));
        assert!(p.dangers[1].contains("무인 설치 시작"));
        assert_eq!(lines(&p), Vec::<String>::new());

        // deadlines the apps have already are not new: the next policy is signed without a word
        let file1 = signed(&first, &["signing-1"]).unwrap();
        let mut next = policy(2);
        next.issued = later;
        let p = plan(&draft(&next, Some(&file1)), &keys, later).unwrap();
        assert_eq!(p.dangers, Vec::<String>::new());
        assert_eq!(lines(&p), Vec::<String>::new());
        // the old deadline under a higher min is a new one; a start at the issue time itself
        // was asked for and is none
        let windows = next.platforms.get_mut(&Platform::Windows).unwrap();
        windows.min = v("1.6.1");
        windows.auto_install_after = Some(later);
        let p = plan(&draft(&next, Some(&file1)), &keys, later + 60).unwrap();
        assert_eq!(p.dangers.len(), 1, "{:?}", p.dangers);
        assert!(p.dangers[0].contains("서버에 넣는 즉시 1.6.1 미만을 막습니다"));
        // ahead again, both are only shown
        let windows = next.platforms.get_mut(&Platform::Windows).unwrap();
        windows.grace_until = Some(later + 7 * 86_400);
        windows.auto_install_after = Some(later + 2 * 86_400);
        let p = plan(&draft(&next, Some(&file1)), &keys, later + 60).unwrap();
        assert_eq!(p.dangers, Vec::<String>::new());
        assert_eq!(
            lines(&p),
            [
                "  windows: 유예 끝까지 6일 23시간",
                "  windows: 무인 설치 시작까지 1일 23시간"
            ]
        );
    }

    #[test]
    fn a_changed_draft_is_noticed() {
        let keys = test_key_set();
        let file1 = signed(&draft(&policy(1), None), &["signing-1"]).unwrap();
        let good = draft(&policy(2), Some(&file1));
        // the policy text is not the tool's
        let spaced = Draft {
            p: good.p.replacen("{\"v\":1,", "{ \"v\":1,", 1),
            ..good.clone()
        };
        assert!(plan(&spaced, &keys, t(NOW))
            .unwrap_err()
            .contains("초안의 정책"));
        let with_url = Draft {
            p: good.p.replacen(
                "관리자에게 받으세요",
                "https://evil.example.com 에서 받으세요",
                1,
            ),
            ..good.clone()
        };
        assert!(plan(&with_url, &keys, t(NOW)).unwrap_err().contains("URL"));
        // the previous envelope was touched
        let touched = Draft {
            prev: Some(file1.replacen("\\\"seq\\\":1", "\\\"seq\\\":0", 1)),
            ..good.clone()
        };
        assert!(plan(&touched, &keys, t(NOW))
            .unwrap_err()
            .contains("이전 정책"));
        // a declaration nobody signed is attached
        let key = test_key("signing-3").unwrap();
        let forged = Part::sign(
            &ed25519_dalek::SigningKey::from_bytes(&key.seed()),
            101,
            ROOT_DOMAIN,
            "{\"v\":1,\"server\":\"rd.example.com\",\"epoch\":1,\"revoked\":[2],\"issued\":\"2026-11-08T03:00:00Z\"}",
        );
        let attached = Draft {
            root: Some(forged),
            ..good
        };
        assert!(plan(&attached, &keys, t(NOW))
            .unwrap_err()
            .contains("루트 선언"));
    }
}

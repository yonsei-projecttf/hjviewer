// SPDX-License-Identifier: AGPL-3.0-only

//! `hjv-release sign`, without the terminal: what a draft would change (shown to the person
//! before any key is opened) and the signing itself.
//!
//! The draft is checked again from its texts; nothing an agent wrote next to them is believed.
//! Key revocations, relaxations, a new epoch and a jump in `seq` are listed as dangers: the
//! command asks the person to type `yes` to each list (design 6-3). So is whatever would be
//! enforced the moment the policy arrives: a raised `min` or a new entry without a grace period,
//! a new deadline that has passed since the draft was made, and unattended installs that start
//! at once. After signing, the envelope goes through the apps' own checks (trust.rs) against the
//! previous policy, with the public keys of `client/update-keys.env`; a file is written only if
//! an app would take it.
//!
//! `sign` is the last check before a key is used, and the draft may come from an agent that
//! went wrong. What `policy` never makes is therefore refused here as well: the same version
//! with another package, unattended installs dated before the policy itself. What `sign` cannot
//! check, it says: it has neither the builds nor the network, so the package hashes of a draft
//! are the signer's to compare with the build run's summary.

use crate::{
    draft::Draft,
    envelope::{Envelope, Part, POLICY_DOMAIN, ROOT_DOMAIN},
    keyfile::SecretKey,
    keys::{KeySet, Role},
    make::{describe_changes, SEQ_JUMP_TO_CONFIRM},
    policy::{Policy, RootDecl},
    time::{self, DAY},
    trust::{
        check_envelope, check_root, describe, new_entries_below, old_mins_that_wait, relaxations,
        Checked, State,
    },
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

    // what an app would not take from a signing key, and what it would take although it
    // enforces less than now (a new entry below the present minimums): both are the root key's
    let relaxed = old.map(|old| relaxations(old, &policy)).unwrap_or_default();
    let lowered = old
        .map(|old| new_entries_below(old, &policy))
        .unwrap_or_default();
    let eased: Vec<String> = relaxed
        .iter()
        .map(|r| r.describe())
        .chain(lowered.iter().map(|l| l.describe()))
        .collect();
    if !eased.is_empty() && draft.signer != Role::Root {
        return Err(format!(
            "초안이 완화를 담고 있는데 서명 키로 서명하게 되어 있습니다({}). policy --relax로 다시 만드세요",
            eased.join("; ")
        ));
    }
    for line in &eased {
        dangers.push(format!("완화: {line}"));
    }
    if draft.signer == Role::Root && eased.is_empty() {
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
    // new deadline still is from this moment. Whatever is enforced the moment the policy arrives
    // needs consent: a deadline that has passed, a raised minimum with no deadline at all,
    // unattended installs that start at once.
    let mut remaining = Vec::new();
    let mut effects = Vec::new();
    let mut new_packages = Vec::new();
    for (platform, entry) in &policy.platforms {
        let before = old.and_then(|old| old.platforms.get(platform));

        // every build has a new version (design 1-4): `policy` never names another package for
        // a version an app may have installed already
        if let (Some(b), Some(package)) = (before, &entry.package) {
            if b.latest == entry.latest && b.package.as_ref().is_some_and(|was| was != package) {
                return Err(format!(
                    "{platform}: 버전 {}의 패키지가 이전 정책의 것과 다릅니다. 같은 버전을 다른 빌드로 낼 수 \
                     없습니다(policy는 이런 초안을 만들지 않습니다)",
                    entry.latest
                ));
            }
        }
        let new_package = entry
            .package
            .as_ref()
            .filter(|package| before.is_none_or(|b| b.package.as_ref() != Some(*package)));
        if let Some(package) = new_package {
            new_packages.push(format!(
                "{platform} {}: {} · {}바이트 · sha256 {}",
                entry.latest, package.file, package.size, package.sha256
            ));
        }

        let new_deadline = entry
            .grace_until
            .filter(|g| before.is_none_or(|b| b.min != entry.min || b.grace_until != Some(*g)));
        match (new_deadline, entry.grace_until) {
            (Some(deadline), _) if deadline <= now => dangers.push(format!(
                "{platform}: 유예 끝({})이 이미 지났습니다. 서버에 넣는 즉시 {} 미만을 막습니다. \
                 초안을 만든 뒤 시간이 지났다면 policy로 다시 만드세요",
                time::format_kst(deadline),
                entry.min
            )),
            (Some(deadline), _) => remaining.push(format!(
                "{platform}: 유예 끝까지 {}",
                time::format_span(deadline - now)
            )),
            // a deadline the apps have already
            (None, Some(_)) => {}
            // no deadline at all: a raised min, and the min of a new entry, are enforced on
            // arrival, and only the root key could give them a grace period afterwards
            (None, None) => match (before, old) {
                (Some(b), _) if entry.min > b.min => dangers.push(format!(
                    "{platform}: 최소 버전을 올리는데({} → {}) 유예가 없습니다. 서버에 넣는 즉시 {} 미만을 \
                     막습니다. 유예는 나중에 루트 키로만 둘 수 있습니다",
                    b.min, entry.min, entry.min
                )),
                // the grace period of an unchanged min taken away while it was still running
                (Some(b), _) if entry.min == b.min && b.grace_until.is_some_and(|g| g > now) => {
                    dangers.push(format!(
                        "{platform}: 유예를 없앱니다(앱에 알린 기한 {}). 서버에 넣는 즉시 {} 미만을 \
                         막습니다. 유예는 나중에 루트 키로만 다시 둘 수 있습니다",
                        b.grace_until.map(time::format_kst).unwrap_or_default(),
                        entry.min
                    ))
                }
                // an unchanged min that is enforced already, or a min the root key lowers
                (Some(_), _) => {}
                (None, Some(_)) => dangers.push(format!(
                    "{platform}: 새 항목에 유예가 없습니다. 서버에 넣는 즉시 {} 미만을 막습니다. 유예는 \
                     나중에 루트 키로만 둘 수 있습니다",
                    entry.min
                )),
                // the first policy of all: said, not asked (README: seq 1 names the lowest
                // version ever released, without a deadline)
                (None, None) => effects.push(format!(
                    "{platform}: 유예가 없습니다. 서버에 넣는 즉시 {} 미만을 막습니다",
                    entry.min
                )),
            },
        }

        let new_start = entry.auto_install_after.filter(|a| {
            before.is_none_or(|b| b.latest != entry.latest || b.auto_install_after != Some(*a))
        });
        match new_start {
            // `policy` dates a start at the policy's issue or later
            Some(start) if start < policy.issued => {
                return Err(format!(
                    "{platform}: 무인 설치 시작({})이 정책의 발행 시각보다 이릅니다. 설치된 PC가 고리 차례 없이 \
                     한꺼번에 설치하게 되므로 서명하지 않습니다(policy는 이런 초안을 만들지 않습니다)",
                    time::format_kst(start)
                ))
            }
            // passed while the draft waited, or asked for at once (`--auto-after windows=+0d`):
            // either way PCs install without a person as soon as the policy arrives
            Some(start) if start <= now => {
                let late = now - start;
                let rings = if late >= DAY {
                    format!(
                        " 시작이 {} 전이라 뒤 고리의 PC도 차례를 기다리지 않고 함께 설치합니다.",
                        time::format_span(late)
                    )
                } else {
                    String::new()
                };
                let (when, again) = if start == policy.issued {
                    ("정책의 발행 시각입니다(바로 시작)", "")
                } else {
                    (
                        "이미 지났습니다",
                        " 초안을 만든 뒤 시간이 지났다면 policy로 다시 만드세요",
                    )
                };
                dangers.push(format!(
                    "{platform}: 무인 설치 시작({})이 {when}. 서버에 넣는 즉시 설치된 PC가 고리 0부터 \
                     설치합니다.{rings}{again}",
                    time::format_kst(start)
                ))
            }
            Some(start) => remaining.push(format!(
                "{platform}: 무인 설치 시작까지 {}",
                time::format_span(start - now)
            )),
            None => {}
        }
    }
    // what the apps' rules allow a signing key and the signer should still know
    for waits in old
        .map(|old| old_mins_that_wait(old, &policy))
        .unwrap_or_default()
    {
        if waits.until > now {
            effects.push(waits.describe());
        }
    }
    let prev_by_root = prev.as_ref().is_some_and(|c| c.role == Role::Root);
    if prev_by_root && draft.signer == Role::Signing {
        effects.push(
            "직전 정책은 루트 키가 서명했습니다. 그 정책을 받지 못한(꺼져 있던) 앱은 그 앞의 정책과 비교하므로, \
             그 정책보다 느슨한 데가 남아 있으면 이 정책과 그 뒤의 정책을 받지 않습니다(policy --held가 확인합니다)"
                .to_owned(),
        );
    }

    let mut summary = describe(&policy);
    summary.push("바뀌는 것:".to_owned());
    summary.extend(
        describe_changes(old, &policy)
            .into_iter()
            .map(|l| format!("  {l}")),
    );
    if !effects.is_empty() {
        summary.push("알아 둘 것:".to_owned());
        summary.extend(effects.into_iter().map(|l| format!("  {l}")));
    }
    if !remaining.is_empty() {
        summary.push("지금부터 남은 시간:".to_owned());
        summary.extend(remaining.into_iter().map(|l| format!("  {l}")));
    }
    if !new_packages.is_empty() {
        summary.push(
            "새 패키지(sign은 빌드를 다시 확인하지 못합니다. sha256을 빌드 실행 요약의 값과 대조하세요):"
                .to_owned(),
        );
        summary.extend(new_packages.into_iter().map(|l| format!("  {l}")));
    }
    if !draft.notes.is_empty() {
        summary.push(
            "초안에 적힌 확인 기록(policy가 썼다고 되어 있을 뿐, sign은 다시 확인하지 않습니다):"
                .to_owned(),
        );
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
        // the old deadline under a higher min is a new one. So is a start at the issue time
        // itself (`--auto-after windows=+0d`): the operator asked for it, and the signer is asked
        let windows = next.platforms.get_mut(&Platform::Windows).unwrap();
        windows.min = v("1.6.1");
        windows.auto_install_after = Some(later);
        let p = plan(&draft(&next, Some(&file1)), &keys, later + 60).unwrap();
        assert_eq!(p.dangers.len(), 2, "{:?}", p.dangers);
        assert!(p.dangers[0].contains("서버에 넣는 즉시 1.6.1 미만을 막습니다"));
        assert_eq!(
            p.dangers[1],
            "windows: 무인 설치 시작(2026-11-23 09:00 한국 시각)이 정책의 발행 시각입니다(바로 시작). \
             서버에 넣는 즉시 설치된 PC가 고리 0부터 설치합니다."
        );
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

    /// The envelope of `policy`, signed with the test key `by`.
    fn file_of(policy: &Policy, prev: Option<&str>, by: &str) -> String {
        let d = Draft {
            signer: test_key(by).unwrap().role,
            ..draft(policy, prev)
        };
        signed(&d, &[by]).unwrap()
    }

    fn entry(p: &mut Policy, platform: Platform) -> &mut crate::policy::Entry {
        p.platforms.get_mut(&platform).unwrap()
    }

    /// The lines of the summary under `heading`.
    fn section(p: &Plan, heading: &str) -> Vec<String> {
        let at = p.summary.iter().position(|l| l.starts_with(heading));
        at.map(|at| {
            let rest = p.summary[at + 1..].iter();
            rest.take_while(|l| l.starts_with("  ")).cloned().collect()
        })
        .unwrap_or_default()
    }

    #[test]
    fn a_minimum_enforced_on_arrival_asks_first() {
        let keys = test_key_set();
        let now = t("2026-11-09T04:00:00Z");
        // the sample: iOS at min 1.6.0 with no deadline; here with 1.6.2 out
        let mut first = policy(1);
        entry(&mut first, Platform::Ios).latest = v("1.6.2");
        let file1 = file_of(&first, None, "signing-1");
        let at = |change: &dyn Fn(&mut Policy)| {
            let mut next = Policy {
                seq: 2,
                issued: now,
                ..first.clone()
            };
            change(&mut next);
            plan(&draft(&next, Some(&file1)), &keys, now + 60).unwrap()
        };
        // nothing changes: nothing to ask
        assert_eq!(at(&|_| {}).dangers, Vec::<String>::new());
        // a higher min without a grace period blocks the moment the policy is on the server,
        // and no signing key can give it a grace period afterwards
        let p = at(&|p| entry(p, Platform::Ios).min = v("1.6.1"));
        assert_eq!(
            p.dangers,
            ["ios: 최소 버전을 올리는데(1.6.0 → 1.6.1) 유예가 없습니다. 서버에 넣는 즉시 1.6.1 미만을 \
              막습니다. 유예는 나중에 루트 키로만 둘 수 있습니다"]
        );
        // with a grace period ahead it is shown and not asked; one that has passed is asked
        let p = at(&|p| {
            let e = entry(p, Platform::Ios);
            e.min = v("1.6.1");
            e.grace_until = Some(now + 7 * DAY);
        });
        assert_eq!(p.dangers, Vec::<String>::new());
        assert_eq!(
            section(&p, "지금부터 남은 시간:"),
            ["  ios: 유예 끝까지 6일 23시간"]
        );
        let p = at(&|p| {
            let e = entry(p, Platform::Ios);
            e.min = v("1.6.1");
            e.grace_until = Some(now);
        });
        assert!(p.dangers[0].contains("ios: 유예 끝("), "{:?}", p.dangers);
        // The grace period of an unchanged min taken away while it runs (the sample's Windows
        // entry has until November 23): what the apps were promised for later happens at once.
        let p = at(&|p| entry(p, Platform::Windows).grace_until = None);
        assert_eq!(
            p.dangers,
            ["windows: 유예를 없앱니다(앱에 알린 기한 2026-11-23 00:00 한국 시각). 서버에 넣는 즉시 1.6.0 \
              미만을 막습니다. 유예는 나중에 루트 키로만 다시 둘 수 있습니다"]
        );
        // cut short to a time that is still ahead, it is shown with what is left of it
        let p = at(&|p| entry(p, Platform::Windows).grace_until = Some(now + 3 * DAY));
        assert_eq!(p.dangers, Vec::<String>::new());
        assert_eq!(
            section(&p, "지금부터 남은 시간:"),
            ["  windows: 유예 끝까지 2일 23시간"]
        );
        // and once the old deadline has passed by itself, dropping it changes nothing
        let mut later = Policy {
            seq: 2,
            issued: t("2026-11-23T00:00:00Z"),
            ..first.clone()
        };
        entry(&mut later, Platform::Windows).grace_until = None;
        let p = plan(
            &draft(&later, Some(&file1)),
            &keys,
            t("2026-11-23T00:00:00Z"),
        )
        .unwrap();
        assert_eq!(p.dangers, Vec::<String>::new());

        // a new entry in a later policy counts as a raised min
        let mut without = first.clone();
        without.platforms.remove(&Platform::AndroidPlay);
        let file1 = file_of(&without, None, "signing-1");
        let with_play = |grace: Option<i64>| {
            let mut next = Policy {
                seq: 2,
                issued: now,
                ..first.clone()
            };
            entry(&mut next, Platform::AndroidPlay).grace_until = grace;
            plan(&draft(&next, Some(&file1)), &keys, now + 60).unwrap()
        };
        assert_eq!(
            with_play(None).dangers,
            ["android_play: 새 항목에 유예가 없습니다. 서버에 넣는 즉시 1.6.0 미만을 막습니다. 유예는 \
              나중에 루트 키로만 둘 수 있습니다"]
        );
        assert_eq!(with_play(Some(now + DAY)).dangers, Vec::<String>::new());

        // the first policy of all is said, not asked: nobody held a policy before it
        let p = plan(&draft(&first, None), &keys, t(NOW)).unwrap();
        assert_eq!(p.dangers, Vec::<String>::new());
        assert_eq!(
            section(&p, "알아 둘 것:"),
            [
                "  android: 유예가 없습니다. 서버에 넣는 즉시 1.6.0 미만을 막습니다",
                "  android_play: 유예가 없습니다. 서버에 넣는 즉시 1.6.0 미만을 막습니다",
                "  ios: 유예가 없습니다. 서버에 넣는 즉시 1.6.0 미만을 막습니다",
            ]
        );
    }

    #[test]
    fn unattended_installs_that_start_at_once_ask_first() {
        let keys = test_key_set();
        let first = policy(1);
        let file1 = file_of(&first, None, "signing-1");
        let issued = t("2026-11-20T03:00:00Z");
        let with_start = |start: i64, now: i64| {
            let mut next = Policy {
                seq: 2,
                issued,
                ..first.clone()
            };
            entry(&mut next, Platform::Windows).auto_install_after = Some(start);
            plan(&draft(&next, Some(&file1)), &keys, now)
        };
        // ahead: shown with the time that is left
        let p = with_start(issued + 2 * DAY, issued + 60).unwrap();
        assert_eq!(p.dangers, Vec::<String>::new());
        assert_eq!(
            section(&p, "지금부터 남은 시간:"),
            ["  windows: 무인 설치 시작까지 1일 23시간"]
        );
        // at the issue time (`+0d`), signed a minute later: ring 0 installs on arrival
        let p = with_start(issued, issued + 60).unwrap();
        assert_eq!(
            p.dangers,
            ["windows: 무인 설치 시작(2026-11-20 12:00 한국 시각)이 정책의 발행 시각입니다(바로 시작). \
              서버에 넣는 즉시 설치된 PC가 고리 0부터 설치합니다."]
        );
        // the same draft three days later: the rings that were a day apart install together
        let p = with_start(issued, issued + 3 * DAY).unwrap();
        assert!(
            p.dangers[0].contains(
                "고리 0부터 설치합니다. 시작이 3일 전이라 뒤 고리의 PC도 차례를 기다리지 않고 함께 설치합니다."
            ),
            "{:?}",
            p.dangers
        );
        // a start after the issue that has passed since: the draft waited
        let p = with_start(issued + DAY, issued + 2 * DAY - 1).unwrap();
        assert_eq!(
            p.dangers,
            ["windows: 무인 설치 시작(2026-11-21 12:00 한국 시각)이 이미 지났습니다. 서버에 넣는 즉시 \
              설치된 PC가 고리 0부터 설치합니다. 초안을 만든 뒤 시간이 지났다면 policy로 다시 만드세요"]
        );
        let p = with_start(issued + DAY, issued + 2 * DAY).unwrap();
        assert!(
            p.dangers[0].contains("시작이 1일 전이라"),
            "{:?}",
            p.dangers
        );
        // dated before the policy itself: `policy` makes no such draft, and none is signed
        for before in [1, 30 * DAY] {
            let err = with_start(issued - before, issued + 60).unwrap_err();
            assert!(
                err.contains("windows: 무인 설치 시작(")
                    && err.contains("정책의 발행 시각보다 이릅니다")
                    && err.contains("서명하지 않습니다"),
                "{err}"
            );
        }
        // a start the apps have already is no news, however long ago it was
        let mut next = Policy {
            seq: 2,
            issued,
            ..first.clone()
        };
        next.contact_ko = None;
        let p = plan(&draft(&next, Some(&file1)), &keys, issued + 60).unwrap();
        assert_eq!(p.dangers, Vec::<String>::new());
        // ... unless it comes with a new version: then the old date is a new start, and it lies
        // before this policy
        entry(&mut next, Platform::Windows).latest = v("1.6.2");
        assert!(plan(&draft(&next, Some(&file1)), &keys, issued + 60)
            .unwrap_err()
            .contains("발행 시각보다 이릅니다"));
        // the first policy of all is held to the same
        let mut alone = Policy {
            issued,
            ..policy(1)
        };
        entry(&mut alone, Platform::Windows).auto_install_after = Some(issued - DAY);
        assert!(plan(&draft(&alone, None), &keys, issued + 60).is_err());
    }

    #[test]
    fn the_same_version_is_the_same_package() {
        let keys = test_key_set();
        let first = policy(1);
        let file1 = file_of(&first, None, "signing-1");
        let next = |change: &dyn Fn(&mut crate::policy::Package)| {
            let mut next = policy(2);
            change(
                entry(&mut next, Platform::Windows)
                    .package
                    .as_mut()
                    .unwrap(),
            );
            next
        };
        // another hash, another size, another name or build under the version the apps know
        type Change = Box<dyn Fn(&mut crate::policy::Package)>;
        let changes: Vec<Change> = vec![
            Box::new(|p| p.sha256 = "0".repeat(64)),
            Box::new(|p| p.size += 1),
            Box::new(|p| p.build = "1.6.1.8".to_owned()),
            Box::new(|p| p.file = "HJViewer-1.6.1-windows-x86_64-b.exe".to_owned()),
        ];
        for change in &changes {
            let changed = next(change);
            for signer in [Role::Signing, Role::Root] {
                let d = Draft {
                    signer,
                    ..draft(&changed, Some(&file1))
                };
                let err = plan(&d, &keys, t(NOW)).unwrap_err();
                assert!(
                    err.contains("windows: 버전 1.6.1의 패키지가 이전 정책의 것과 다릅니다"),
                    "{err}"
                );
            }
        }
        // the same package again is nothing new; a new version brings a new package, which the
        // signer is told to compare, since `sign` cannot look at the build
        let p = plan(&draft(&next(&|_| {}), Some(&file1)), &keys, t(NOW)).unwrap();
        assert_eq!(section(&p, "새 패키지"), Vec::<String>::new());
        let mut newer = next(&|p| {
            p.sha256 = "1".repeat(64);
            p.file = "HJViewer-1.6.2-windows-x86_64.exe".to_owned();
        });
        entry(&mut newer, Platform::Windows).latest = v("1.6.2");
        let p = plan(&draft(&newer, Some(&file1)), &keys, t(NOW)).unwrap();
        let heading = p.summary.iter().find(|l| l.starts_with("새 패키지"));
        assert!(
            heading.is_some_and(|l| l.contains("sha256을 빌드 실행 요약의 값과 대조하세요")),
            "{:?}",
            p.summary
        );
        assert_eq!(
            section(&p, "새 패키지"),
            [format!(
                "  windows 1.6.2: HJViewer-1.6.2-windows-x86_64.exe · 26214400바이트 · sha256 {}",
                "1".repeat(64)
            )]
        );
        // in the first policy every package is new
        let p = plan(&draft(&first, None), &keys, t(NOW)).unwrap();
        assert_eq!(section(&p, "새 패키지").len(), 2);
    }

    #[test]
    fn what_enforces_less_without_being_a_relaxation() {
        let keys = test_key_set();
        // every entry at 1.6.0, no Google Play entry yet
        let mut first = policy(1);
        first.platforms.remove(&Platform::AndroidPlay);
        let file1 = file_of(&first, None, "signing-1");
        // A new entry below what is enforced now: an app would take it from a signing key (it
        // has no old entry to compare with), and hbbs's MIN and the PC's threshold for Android
        // controllers would go down. The tool asks the root key and the signer's yes.
        let mut low = policy(2);
        let play = entry(&mut low, Platform::AndroidPlay);
        play.min = v("1.0.0");
        play.grace_until = Some(t(NOW) + 7 * DAY);
        let as_signing = draft(&low, Some(&file1));
        let err = plan(&as_signing, &keys, t(NOW)).unwrap_err();
        assert!(
            err.contains("완화") && err.contains("android_play: 새 항목의 min(1.0.0)"),
            "{err}"
        );
        let as_root = Draft {
            signer: Role::Root,
            ..as_signing
        };
        let p = plan(&as_root, &keys, t(NOW)).unwrap();
        assert_eq!(p.dangers.len(), 1, "{:?}", p.dangers);
        assert!(
            p.dangers[0].starts_with(
                "완화: android_play: 새 항목의 min(1.0.0)이 지금 강제하는 값보다 낮음"
            ),
            "{:?}",
            p.dangers
        );
        assert!(signed(&as_root, &["root-101"]).is_ok());
        // at the present minimum it is an ordinary policy
        let mut same = policy(2);
        entry(&mut same, Platform::AndroidPlay).grace_until = Some(t(NOW) + 7 * DAY);
        let p = plan(&draft(&same, Some(&file1)), &keys, t(NOW)).unwrap();
        assert_eq!(p.dangers, Vec::<String>::new());

        // A raised min with a later deadline than the old min had: until then apps below the
        // old min are let through as well. The apps take that from a signing key; the signer is
        // told.
        let file1 = file_of(&policy(1), None, "signing-1");
        let mut raised = policy(2);
        let windows = entry(&mut raised, Platform::Windows);
        windows.min = v("1.6.1");
        windows.grace_until = Some(t("2026-12-06T15:00:00Z"));
        let p = plan(&draft(&raised, Some(&file1)), &keys, t(NOW)).unwrap();
        assert_eq!(p.dangers, Vec::<String>::new());
        assert_eq!(
            section(&p, "알아 둘 것:"),
            ["  windows: 유예 끝(2026-12-07 00:00 한국 시각)까지는 옛 최소 1.6.0 미만도 막지 않습니다\
              (앱은 새 최소의 유예만 봅니다)"]
        );
        // once that deadline has passed there is nothing left to say but the danger itself
        let p = plan(
            &draft(&raised, Some(&file1)),
            &keys,
            t("2026-12-07T00:00:00Z"),
        )
        .unwrap();
        assert_eq!(section(&p, "알아 둘 것:"), Vec::<String>::new());
        assert_eq!(p.dangers.len(), 1);

        // after a policy of the root key, the signer of a signing-key policy is reminded of the
        // apps that never saw it
        let mut eased = policy(2);
        entry(&mut eased, Platform::Ios).min = v("1.5.0");
        let file2 = file_of(&eased, Some(&file1), "root-101");
        let mut back = policy(3);
        back.issued = t(NOW);
        let p = plan(&draft(&back, Some(&file2)), &keys, t(NOW)).unwrap();
        let said = section(&p, "알아 둘 것:");
        assert!(
            said.iter()
                .any(|l| l.contains("직전 정책은 루트 키가 서명했습니다")),
            "{said:?}"
        );
        let by_root = Draft {
            signer: Role::Root,
            ..draft(&back, Some(&file2))
        };
        let p = plan(&by_root, &keys, t(NOW)).unwrap();
        assert!(!p
            .summary
            .iter()
            .any(|l| l.contains("직전 정책은 루트 키가")));
    }

    #[test]
    fn a_new_declaration_is_checked_before_any_key() {
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
        let file2 = signed(
            &Draft {
                root_p: Some(decl.to_text()),
                ..draft(&next, Some(&file1))
            },
            &["root-101", "signing-2"],
        )
        .unwrap();
        // the next policy with another new declaration, changed in one way at a time; each is
        // refused by the plan, before a key file is asked for, with its own reason
        let third = |epoch: u64| Policy {
            epoch,
            seq: if epoch == 2 { 2 } else { 1 },
            ..sample()
        };
        let refused = |policy: &Policy, decl: RootDecl| {
            let d = Draft {
                root: None,
                root_p: Some(decl.to_text()),
                ..draft(policy, Some(&file2))
            };
            plan(&d, &keys, t(NOW)).unwrap_err()
        };
        let good = RootDecl {
            epoch: 3,
            ..decl.clone()
        };
        let d = Draft {
            root: None,
            root_p: Some(good.to_text()),
            ..draft(&third(3), Some(&file2))
        };
        assert!(plan(&d, &keys, t(NOW)).is_ok());
        // an epoch before the app's
        let back = RootDecl {
            epoch: 1,
            ..decl.clone()
        };
        let err = refused(&third(2), back);
        assert!(
            err.contains("새 루트 선언의 세대가 이전보다 낮습니다"),
            "{err}"
        );
        // a signing key's policy of another epoch than its declaration
        let err = refused(&third(2), good.clone());
        assert!(
            err.contains("정책의 세대가 루트 선언의 세대와 다릅니다"),
            "{err}"
        );
        // a declaration of another server, and one dated ahead of this PC's clock
        let elsewhere = RootDecl {
            server: "hjv.test".to_owned(),
            ..good.clone()
        };
        let err = refused(&third(3), elsewhere);
        assert!(
            err.contains("새 루트 선언의 서버가 정책의 서버와 다릅니다"),
            "{err}"
        );
        let ahead = RootDecl {
            issued: t(NOW) + CLOCK_SLACK + 1,
            ..good.clone()
        };
        let err = refused(&third(3), ahead);
        assert!(
            err.contains("새 루트 선언의 발행 시각이 미래입니다"),
            "{err}"
        );
        let on_time = RootDecl {
            issued: t(NOW) + CLOCK_SLACK,
            ..good
        };
        let d = Draft {
            root: None,
            root_p: Some(on_time.to_text()),
            ..draft(&third(3), Some(&file2))
        };
        assert!(plan(&d, &keys, t(NOW)).is_ok());
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

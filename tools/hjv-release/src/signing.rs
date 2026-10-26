// SPDX-License-Identifier: AGPL-3.0-only

//! `hjv-release sign`, without the terminal: what a draft would change (shown to the person
//! before any key is opened) and the signing itself (trust rules 4-4, 4-6).
//!
//! The draft is checked again from its texts; nothing an agent wrote next to them is believed. A
//! new declaration's base is recomputed from the policy, its epoch and revocations checked
//! against the previous declaration and the archive (X2), the live file against `--prev` and
//! the archive (X10), a policy without a declaration against the archive's last file (X11), and
//! every signer against the key sets the archive records (X12, X9). Revocations, a new epoch,
//! a policy a root key signs, opening a root key, a jump in `seq`, and whatever would be enforced
//! the moment the policy arrives are listed as dangers: the command asks the person to type `yes`.
//! After signing, the envelope goes through the apps' own steps (trust.rs) for a device holding
//! the previous policy and for a fresh install (X4); a file is written only if both take it. Then
//! the archive gets the signed files.
//!
//! `sign` is the last check before a key is used, and the draft may come from an agent that went
//! wrong. What `policy` never makes is therefore refused here as well. What `sign` cannot check,
//! it says: it has neither the builds nor the network, so the package hashes of a draft are the
//! signer's to compare with the build run's summary.

use crate::{
    archive::Archive,
    draft::Draft,
    envelope::{Envelope, Part, POLICY_DOMAIN, ROOT_DOMAIN},
    keyfile::SecretKey,
    keys::{KeySet, Role},
    make::{
        appstore_kept, auto_off_warning, describe_changes, loosened_lines, relaxes_line, x4,
        SEQ_JUMP_TO_CONFIRM,
    },
    policy::{Platform, Policy, RootDecl},
    time::{self, DAY},
    trust::{self, check_envelope, describe, describe_declaration, new_entries_below, Checked},
};
use std::{collections::BTreeSet, path::PathBuf};

/// How far ahead of this PC's clock a draft's times may be.
const CLOCK_SLACK: i64 = 300;

#[derive(Debug)]
pub struct Plan {
    pub policy: Policy,
    pub prev: Option<Checked>,
    /// A root declaration to sign now.
    pub new_root: Option<RootDecl>,
    /// `RELAXED(--prev, new)`: what the server check's `--relaxes` names.
    pub relaxed: BTreeSet<Platform>,
    /// What needs typed consent.
    pub dangers: Vec<String>,
    /// What the signed policy says and how it differs from the previous one.
    pub summary: Vec<String>,
}

impl Plan {
    /// The keys the draft needs, before any key file is opened.
    pub fn needed(&self, draft: &Draft) -> Vec<Role> {
        match (draft.signer, self.new_root.is_some()) {
            (Role::Signing, false) => vec![Role::Signing],
            (Role::Signing, true) => vec![Role::Root, Role::Signing],
            (Role::Root, _) => vec![Role::Root],
        }
    }

    /// Whether a root key is opened.
    pub fn opens_root(&self, draft: &Draft) -> bool {
        self.needed(draft).contains(&Role::Root)
    }
}

/// What a policy does on arrival and over time, compared with the previous one (4-4): the
/// dangers to consent to, what is said without asking, and how long is left of each new time.
pub fn effects(
    old: Option<&Policy>,
    policy: &Policy,
    new_declaration: bool,
    now: i64,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let (mut dangers, mut said, mut remaining) = (Vec::new(), Vec::new(), Vec::new());
    for (platform, entry) in &policy.platforms {
        let before = old.and_then(|old| old.platforms.get(platform));
        // what the entry blocks now and from its deadline on
        if let (Some(until), Some(grace_min)) = (entry.grace_until, entry.grace_min) {
            said.push(format!(
                "{platform}: 지금은 {grace_min} 미만을 막고, {}부터는 {} 미만을 막습니다",
                time::format_kst(until),
                entry.min
            ));
            let new_deadline =
                before.is_none_or(|b| b.min != entry.min || b.grace_until != Some(until));
            if new_deadline && until <= now {
                dangers.push(format!(
                    "{platform}: 유예 끝({})이 이미 지났습니다. 서버에 넣는 즉시 {} 미만을 막습니다. 초안을 만든 \
                     뒤 시간이 지났다면 policy로 다시 만드세요",
                    time::format_kst(until),
                    entry.min
                ));
            } else if new_deadline {
                remaining.push(format!(
                    "{platform}: 유예 끝까지 {}",
                    time::format_span(until - now)
                ));
            }
        }
        match before {
            Some(b) => {
                let pending = b.grace_until.filter(|u| *u > now).zip(b.grace_min);
                // a raise while a deadline is pending (derivation c)
                if let Some((until, gm)) = pending.filter(|_| entry.min > b.min) {
                    match (entry.grace_until, entry.grace_min) {
                        (Some(new_until), Some(new_gm)) if new_until > until && new_gm >= b.min => {
                            dangers.push(format!(
                                "{platform}: 유예 중이던 {gm} 이상 {} 미만이 {} 대신 서버에 넣는 즉시 막힙니다(더 늦은 \
                                 유예 끝 {}을 줬기 때문입니다)",
                                b.min,
                                time::format_kst(until),
                                time::format_kst(new_until)
                            ))
                        }
                        (Some(new_until), Some(_)) if new_until < until => dangers.push(format!(
                            "{platform}: 유예 끝을 앞당깁니다({} → {}). {gm} 이상 {} 미만은 그때까지만 씁니다",
                            time::format_kst(until),
                            time::format_kst(new_until),
                            entry.min
                        )),
                        (Some(new_until), Some(_)) => dangers.push(format!(
                            "{platform}: {} 이상 {} 미만은 지금 유예와 같은 {}까지만 씁니다",
                            b.min,
                            entry.min,
                            time::format_kst(new_until)
                        )),
                        _ => {}
                    }
                }
                // enforced on arrival
                if entry.grace_until.is_none() && entry.min > b.min {
                    dangers.push(format!(
                        "{platform}: 최소 버전을 올리는데({} → {}) 유예가 없습니다. 서버에 넣는 즉시 {} 미만을 막습니다",
                        b.min, entry.min, entry.min
                    ));
                }
                if let Some((until, _)) = pending.filter(|_| entry.min == b.min) {
                    match entry.grace_until {
                        None => dangers.push(format!(
                            "{platform}: 남은 유예(앱에 알린 기한 {})를 없앱니다. 서버에 넣는 즉시 {} 미만을 막습니다",
                            time::format_kst(until),
                            entry.min
                        )),
                        Some(new_until) if new_until < until => dangers.push(format!(
                            "{platform}: 유예 끝을 앞당깁니다({} → {})",
                            time::format_kst(until),
                            time::format_kst(new_until)
                        )),
                        _ => {}
                    }
                }
                // a relaxation that puts off or gives a deadline (with a declaration)
                if new_declaration && entry.min == b.min {
                    if let (Some(until), Some(gm)) = (entry.grace_until, entry.grace_min) {
                        if b.grace_until.is_none_or(|was| until > was) {
                            dangers.push(format!(
                                "{platform}: 지금부터 {}까지 {gm} 이상 {} 미만을 막지 않습니다",
                                time::format_kst(until),
                                entry.min
                            ));
                        }
                    }
                }
            }
            // a new entry without a deadline, except in the very first policy
            None if old.is_some() && entry.grace_until.is_none() => dangers.push(format!(
                "{platform}: 새 항목에 유예가 없습니다. 서버에 넣는 즉시 {} 미만을 막습니다",
                entry.min
            )),
            None if entry.grace_until.is_none() => said.push(format!(
                "{platform}: 유예가 없습니다. 서버에 넣는 즉시 {} 미만을 막습니다",
                entry.min
            )),
            None => {}
        }
        // unattended installs that start at once
        let new_start = entry.auto_install_after.filter(|a| {
            before.is_none_or(|b| b.latest != entry.latest || b.auto_install_after != Some(*a))
        });
        match new_start {
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
                dangers.push(format!(
                    "{platform}: 무인 설치 시작({})이 지났거나 지금입니다. 서버에 넣는 즉시 설치된 PC가 고리 0부터 \
                     설치합니다.{rings}",
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
    (dangers, said, remaining)
}

/// Checks a draft against the key file, the archive and the live file, and says what signing
/// it would do.
pub fn plan(
    draft: &Draft,
    keys: &KeySet,
    archive: &Archive,
    live: Option<&str>,
    now: i64,
) -> Result<Plan, String> {
    let policy = Policy::parse(&draft.p).map_err(|e| format!("초안의 정책: {e}"))?;
    let server = policy.server.clone();
    let prev = draft
        .prev
        .as_deref()
        .map(|file| check_envelope(file, keys, &server))
        .transpose()
        .map_err(|e| format!("초안의 이전 정책: {e}"))?;
    let old = prev.as_ref().map(|c| &c.policy);
    let old_decl = prev.as_ref().and_then(|c| c.root.as_ref());

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
    let mut said = Vec::new();
    let mut relaxed = BTreeSet::new();
    let new_root = match &draft.root_p {
        Some(text) => {
            let decl = RootDecl::parse(text).map_err(|e| format!("초안의 새 루트 선언: {e}"))?;
            if decl.server != server {
                return Err("새 루트 선언의 서버가 정책의 서버와 다릅니다".to_owned());
            }
            if decl.issued != policy.issued {
                return Err("새 루트 선언의 발행 시각이 정책의 발행 시각과 다릅니다".to_owned());
            }
            if decl.base != policy.requirements() {
                return Err(
                    "새 루트 선언의 base가 이 정책의 요구 부분(min, latest, grace_until, grace_min)과 다릅니다. \
                     policy로 다시 만드세요"
                        .to_owned(),
                );
            }
            // X10: from the live file, which is --prev and a file of the archive
            let prev_file = draft
                .prev
                .as_deref()
                .ok_or("새 루트 선언에는 이전 정책(서버의 정책)이 있어야 합니다")?;
            let live = live.ok_or(
                "새 루트 선언에 서명하려면 --live <https://<서버>/api/hjv/version에서 방금 받은 파일>이 필요합니다(X10)",
            )?;
            if live != prev_file {
                return Err(
                    "X10: 서버의 정책(--live)이 초안의 이전 정책과 다릅니다. 서버가 바뀌었으면 policy로 다시 만드세요"
                        .to_owned(),
                );
            }
            if archive.find(prev_file)?.is_none() {
                return Err(
                    "X10: 초안의 이전 정책(서버의 정책)이 보관소에 없습니다. 다른 곳에서 서명된 것입니다(도둑맞은 키: \
                     런북 5-8, 또는 옮기지 않은 서명: 5-0)"
                        .to_owned(),
                );
            }
            // X2: an epoch above every one, revocations that only grow
            let old_epoch = old.map_or(1, |o| o.epoch);
            let archived = archive.max_epoch()?;
            if decl.epoch <= old_epoch || decl.epoch <= archived {
                return Err(format!(
                    "X2: 새 루트 선언의 세대({})가 이전 정책({old_epoch})과 보관소의 모든 세대({archived})보다 높아야 합니다",
                    decl.epoch
                ));
            }
            let before: &[u32] = old_decl.map_or(&[], |d| &d.revoked);
            if before.iter().any(|id| !decl.revoked.contains(id)) {
                return Err(
                    "X2: 새 루트 선언이 이전에 폐기한 키를 빼먹었습니다(폐기 목록은 쌓이기만 합니다)".to_owned(),
                );
            }
            if (policy.epoch, policy.seq) != (decl.epoch, 1) {
                return Err("새 루트 선언과 함께 가는 정책은 그 세대의 seq 1입니다".to_owned());
            }
            // the live file should be the archive's newest: a server file rolled back to an older
            // archived one would make a base of old requirements (said and asked)
            if let (Some(h), Some(old)) = (archive.highest()?, old) {
                if (h.epoch, h.seq) > (old.epoch, old.seq) {
                    dangers.push(format!(
                        "보관소에 서버의 정책({}:{})보다 높은 파일({}:{})이 있습니다. 서버의 파일이 되돌려졌다면 먼저 \
                         보관소의 마지막 파일로 바로잡으세요(런북 5-8 2번). 서명하고 내지 않은 파일이면(5-13) 그대로 \
                         진행합니다",
                        old.epoch, old.seq, h.epoch, h.seq
                    ));
                }
            }
            // every revocation any archived declaration made, not only the previous one's
            for (epoch, part) in archive.declarations()? {
                if let Ok(earlier) = RootDecl::read(&part.p) {
                    for id in earlier
                        .revoked
                        .iter()
                        .filter(|id| !decl.revoked.contains(id))
                    {
                        dangers.push(format!(
                            "보관소의 루트 선언(세대 {epoch})이 폐기한 서명 키 {id}가 새 선언의 폐기 목록에 없습니다. \
                             그 선언을 내지 않았다면 그대로 진행하고, 냈다면 --revoke-key {id}로 다시 만드세요"
                        ));
                    }
                }
            }
            let newly: Vec<u32> = decl
                .revoked
                .iter()
                .copied()
                .filter(|id| !before.contains(id))
                .collect();
            for id in &newly {
                dangers.push(format!(
                    "서명 키(번호 {id})를 폐기합니다. 지금 키를 박은 앱은 이 키를 다시 믿지 않습니다(되돌릴 수 없음)"
                ));
            }
            relaxed = old
                .map(|o| trust::relaxed(&o.requirements(), &policy.requirements()))
                .unwrap_or_default();
            let mut epoch_line = format!(
                "세대(epoch)를 올립니다({old_epoch} → {}). 모든 앱이 이 base를 앞으로 서명 키가 요구할 수 있는 \
                 바닥으로 받습니다:",
                decl.epoch
            );
            for line in describe_declaration(&decl).into_iter().skip(1) {
                epoch_line.push_str(&format!(" [{}]", line.trim()));
            }
            dangers.push(epoch_line);
            if let Some(old) = old {
                for line in loosened_lines(old, &policy) {
                    dangers.push(format!("완화: {line}"));
                }
                for (p, min, lowest) in new_entries_below(old, &policy) {
                    dangers.push(format!(
                        "완화(도구 규칙): {p}: 새 항목의 최소 버전({min})이 지금 가장 낮은 최소 버전({lowest})보다 낮아 \
                         hbbs의 MIN이 내려감"
                    ));
                }
            }
            dangers.push(relaxes_line(&relaxed));
            if !newly.is_empty() {
                for (platform, entry) in &policy.platforms {
                    if let Some(p) = &entry.package {
                        said.push(format!(
                            "폐기와 함께 나가는 패키지 {platform} {}: {} · {}바이트 · sha256 {}(태그 노트와 대조)",
                            entry.latest, p.file, p.size, p.sha256
                        ));
                    }
                }
            }
            Some(decl)
        }
        None => {
            // the last declaration goes along unchanged, so that a device that was off learns it
            let carried = prev.as_ref().and_then(|c| c.envelope.root.as_ref());
            if draft.root.as_ref() != carried {
                return Err("초안에 붙은 루트 선언이 이전 정책의 것과 다릅니다".to_owned());
            }
            // X11: after the archive's last file, above every position of the epoch it holds
            let highest = archive.highest()?;
            match (&highest, &draft.prev) {
                (None, None) => {}
                (Some(h), Some(p)) if h.file == *p => {}
                (Some(h), _) => {
                    return Err(format!(
                        "X11: 초안의 이전 정책이 보관소의 마지막 파일({}:{})이 아닙니다: {}",
                        h.epoch,
                        h.seq,
                        h.path.display()
                    ))
                }
                (None, Some(_)) => return Err(
                    "X11: 초안의 이전 정책이 보관소에 없습니다(보관소가 비었습니다: archive adopt)"
                        .to_owned(),
                ),
            }
            if let Some(old) = old {
                if policy.epoch != old.epoch {
                    return Err(
                        "세대를 올리려면 새 루트 선언이 있어야 합니다(policy --new-epoch)"
                            .to_owned(),
                    );
                }
                let above = archive.max_seq(old.epoch)?.unwrap_or(0).max(old.seq);
                if policy.seq <= above {
                    return Err(format!(
                        "X11: seq({})가 이 세대에서 보관소가 가진 가장 높은 seq({above})보다 높지 않습니다",
                        policy.seq
                    ));
                }
                // X1, X8: asking less needs a declaration
                let eased: Vec<String> =
                    loosened_lines(old, &policy)
                        .into_iter()
                        .chain(new_entries_below(old, &policy).into_iter().map(
                            |(p, min, lowest)| format!("{p}: 새 항목의 최소 버전 {min} < {lowest}"),
                        ))
                        .collect();
                if !eased.is_empty() {
                    return Err(format!(
                        "X1: 초안이 이전 정책보다 덜 막는데 루트 선언이 없습니다({}). policy --relax로 다시 만드세요",
                        eased.join("; ")
                    ));
                }
                if policy.seq > old.seq + SEQ_JUMP_TO_CONFIRM {
                    dangers.push(format!(
                        "seq가 한 번에 뜁니다({} → {}, {}만큼). 보통은 1씩 오릅니다",
                        old.seq,
                        policy.seq,
                        policy.seq - old.seq
                    ));
                }
            } else if (policy.epoch, policy.seq) != (keys.floor.epoch, keys.floor.seq + 1) {
                return Err(format!(
                    "첫 정책의 위치는 키 파일의 하한 바로 위({}:{})입니다",
                    keys.floor.epoch,
                    keys.floor.seq + 1
                ));
            }
            None
        }
    };
    if let Some(a) = archive.at(policy.epoch, policy.seq)? {
        return Err(format!(
            "X11: 보관소에 같은 위치({}:{})의 정책이 이미 있습니다: {}",
            policy.epoch,
            policy.seq,
            a.path.display()
        ));
    }

    // the App Store (design 3-7 T3): once named, never dropped or lowered; a change of iOS
    // without it while the store record has an App Store release is asked about
    appstore_kept(old, &policy)?;
    if let Some(released) = &draft.appstore_release {
        if policy
            .platforms
            .get(&Platform::Ios)
            .is_none_or(|ios| ios.appstore.is_some())
        {
            return Err(
                "초안: appstore_release는 App Store 버전 없이 ios를 바꾸는 초안에만 있습니다(policy는 이런 초안을 \
                 만들지 않습니다)"
                    .to_owned(),
            );
        }
        dangers.push(format!(
            "ios: 출시 기록에 App Store 출시({released})가 있는데 App Store 버전 없이(--no-appstore) ios를 바꿉니다. \
             App Store 설치본도 이 항목을 따릅니다. --appstore-live를 빠뜨린 것이 아닐 때만 진행하세요"
        ));
    }

    // the package rule and one build per version (4-2, X7)
    let mut new_packages = Vec::new();
    for (platform, entry) in &policy.platforms {
        let before = old.and_then(|old| old.platforms.get(platform));
        if let (Some(b), Some(package)) = (before, &entry.package) {
            if b.latest == entry.latest && b.package.as_ref() != Some(package) {
                return Err(format!(
                    "{platform}: 버전 {}의 패키지가 이전 정책의 것과 다릅니다(거둬들인 패키지를 다시 붙이거나 같은 버전을 \
                     다른 빌드로 냄). policy는 이런 초안을 만들지 않습니다",
                    entry.latest
                ));
            }
        }
        if platform.has_package() && entry.package.is_none() {
            match before {
                Some(b) if b.package.is_some() && b.latest == entry.latest => dangers.push(format!(
                    "{platform}: 패키지를 거둬들입니다(앱 안 내려받기, 무인 설치, 강제 설치가 멈춤). 같은 버전에는 \
                     다시 붙이지 않습니다"
                )),
                Some(b) if b.package.is_none() && b.latest == entry.latest => {}
                _ => {
                    return Err(format!(
                        "{platform}: 패키지가 없습니다(새 latest는 패키지와 함께 나갑니다)"
                    ))
                }
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
        let new_start = entry.auto_install_after.filter(|a| {
            before.is_none_or(|b| b.latest != entry.latest || b.auto_install_after != Some(*a))
        });
        if let Some(start) = new_start.filter(|s| *s < policy.issued) {
            return Err(format!(
                "{platform}: 무인 설치 시작({})이 정책의 발행 시각보다 이릅니다. 설치된 PC가 고리 차례 없이 \
                 한꺼번에 설치하게 되므로 서명하지 않습니다(policy는 이런 초안을 만들지 않습니다)",
                time::format_kst(start)
            ));
        }
    }

    if draft.signer == Role::Root {
        // the recorded builds that pin none of the key file's usable signing keys
        let revoked: Vec<u32> = new_root
            .as_ref()
            .or(old_decl)
            .map(|d| d.revoked.clone())
            .unwrap_or_default();
        let unserved: Vec<String> = archive
            .keysets()?
            .into_iter()
            .filter(|record| {
                !keys
                    .signing
                    .iter()
                    .filter(|k| !revoked.contains(&k.id))
                    .any(|k| record.pins(Role::Signing, k))
            })
            .map(|record| record.name())
            .collect();
        dangers.push(format!(
            "정책을 루트 키로 서명합니다: 쓸 수 있는 서명 키를 박지 않은 빌드({})의 기기는 루트 키가 서명한 것만 받습니다",
            if unserved.is_empty() {
                "기록된 모든 빌드에 쓸 서명 키가 없음".to_owned()
            } else {
                unserved.join(", ")
            }
        ));
        for (platform, entry) in &policy.platforms {
            dangers.push(format!(
                "  그 기기가 설치하게 될 빌드: {platform} {}",
                entry.latest
            ));
        }
    }
    let (arrival, more_said, remaining) = effects(old, &policy, new_root.is_some(), now);
    dangers.extend(arrival);
    said.extend(more_said);
    if let Some(line) = old
        .filter(|o| {
            o.platforms
                .get(&Platform::Windows)
                .is_some_and(|w| w.auto_install_after.is_some())
        })
        .filter(|_| {
            policy
                .platforms
                .get(&Platform::Windows)
                .is_some_and(|w| w.auto_install_after.is_none())
        })
        .and_then(|o| auto_off_warning(Some(o), &policy, now))
    {
        said.push(line);
    }

    let mut summary = describe(&policy);
    summary.push("바뀌는 것:".to_owned());
    summary.extend(
        describe_changes(old, &policy)
            .into_iter()
            .map(|l| format!("  {l}")),
    );
    if !said.is_empty() {
        summary.push("알아 둘 것:".to_owned());
        summary.extend(said.into_iter().map(|l| format!("  {l}")));
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
    let plan = Plan {
        policy,
        prev,
        new_root,
        relaxed,
        dangers,
        summary,
    };
    if plan.opens_root(draft) {
        plan_root_question(plan)
    } else {
        Ok(plan)
    }
}

/// Every `sign` that opens a root key asks about the machine it runs on (runbook 5-0).
fn plan_root_question(mut plan: Plan) -> Result<Plan, String> {
    plan.dangers.insert(
        0,
        "루트 키를 엽니다: 이 컴퓨터는 런북 5-0의 깨끗한 컴퓨터입니다(개발 PC가 아님, 공개 저장소의 릴리스 태그에서 \
         빌드한 도구)"
            .to_owned(),
    );
    Ok(plan)
}

/// The files `sign` wrote: the envelope, and what went to the archive.
#[derive(Debug)]
pub struct Signed {
    pub file: String,
    pub archived: Vec<PathBuf>,
}

/// Signs the draft with the opened keys, checks the envelope by the apps' steps (X4) and adds
/// it to the archive.
pub fn sign(
    draft: &Draft,
    plan: &Plan,
    secrets: &[SecretKey],
    keys: &KeySet,
    archive: &Archive,
    version: Option<&str>,
) -> Result<Signed, String> {
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
    let server = &plan.policy.server;
    // key sets: only an archive without records takes the key file's key set by itself; any
    // other key set is recorded by `floor --live` for the build made with it
    let records = archive.keysets()?;
    if !records.is_empty() && !archive.records(keys, server)? {
        return Err(format!(
            "키 파일의 키 집합이 보관소에 기록되어 있지 않습니다({}). 새 빌드의 키 파일이면 floor --live로 먼저 \
             기록하고(빌드를 만들 때), 그렇지 않으면 맞는 --keys를 주세요",
            Archive::keyset_name(keys, server)
        ));
    }
    let serves = |role: Role, id: u32| -> Result<(), String> {
        let (_, public) = keys
            .find(id)
            .ok_or_else(|| format!("키 번호 {id}가 키 파일에 없습니다"))?;
        let unserved = archive.not_pinning(role, public)?;
        if unserved.is_empty() {
            return Ok(());
        }
        let names: Vec<String> = unserved.iter().map(|r| r.name()).collect();
        Err(format!(
            "X12: {} {id}를 박지 않은 빌드가 기록되어 있습니다({}). 그 빌드가 쓰이는 동안은 이 키로 서명하지 않습니다",
            role.korean(),
            names.join(", ")
        ))
    };
    let revoked: Vec<u32> = plan
        .new_root
        .as_ref()
        .or(plan.prev.as_ref().and_then(|c| c.root.as_ref()))
        .map(|d| d.revoked.clone())
        .unwrap_or_default();

    let root = match &draft.root_p {
        Some(text) => {
            let key =
                root_key.ok_or("새 루트 선언은 루트 키로 서명합니다: --key <루트 키 파일>")?;
            serves(Role::Root, key.id)?;
            Some(Part::sign(key.signing_key(), key.id, ROOT_DOMAIN, text))
        }
        None => draft.root.clone(),
    };
    let key = match draft.signer {
        Role::Root => {
            let key = root_key.ok_or(
                "이 정책은 루트 키로 서명합니다(--root-signs-policy): --key <루트 키 파일>",
            )?;
            // X9: only while no signing key of the key file serves every recorded build
            for signing in keys.signing.iter().filter(|k| !revoked.contains(&k.id)) {
                if archive.not_pinning(Role::Signing, signing)?.is_empty() {
                    return Err(format!(
                        "X9: 서명 키 {}가 기록된 모든 빌드에 박혀 있습니다. 정책은 그 키로 서명합니다(policy를 \
                         --root-signs-policy 없이 다시 만드세요)",
                        signing.id
                    ));
                }
            }
            serves(Role::Root, key.id)?;
            key
        }
        Role::Signing => {
            let key = signing_key.ok_or(
                "이 정책은 서명 키로 서명합니다: --key <서명 키 파일>(루트 키는 루트 선언에만 씁니다)",
            )?;
            if revoked.contains(&key.id) {
                return Err(format!(
                    "서명 키 {}는 (새) 루트 선언이 폐기한 키입니다. 다른 서명 키로 서명하세요",
                    key.id
                ));
            }
            serves(Role::Signing, key.id)?;
            key
        }
    };
    let file = Envelope {
        policy: Part::sign(key.signing_key(), key.id, POLICY_DOMAIN, &draft.p),
        root,
    }
    .to_file();

    // X4 on the signed bytes
    x4(&file, draft.prev.as_deref(), keys, server, true)
        .map_err(|e| format!("서명한 정책을 앱이 받지 않으므로 쓰지 않습니다: {e}"))?;
    check_envelope(&file, keys, server)
        .map_err(|e| format!("서명한 정책을 이 도구가 받지 않으므로 쓰지 않습니다: {e}"))?;

    // the archive: the envelope, a new declaration, and the first key set
    let mut archived = Vec::new();
    if let Some(path) = archive.add_policy(plan.policy.epoch, plan.policy.seq, &file)? {
        archived.push(path);
    }
    if let (Some(decl), Some(part)) = (&plan.new_root, &Envelope::parse(&file)?.root) {
        if let Some(path) = archive.add_declaration(decl.epoch, part)? {
            archived.push(path);
        }
    }
    if records.is_empty() {
        if let Some(path) = archive.record_keyset(keys, server, version)? {
            archived.push(path);
        }
    }
    Ok(Signed { file, archived })
}

#[cfg(test)]
#[path = "signing_tests.rs"]
mod tests;

// SPDX-License-Identifier: AGPL-3.0-only

//! The trust rules of the version policy (build/minver/trust-rules/spec.md, 1.6.0) as an app
//! applies them: what a device holds (1-5), how it takes or refuses an envelope (2), what LOAD
//! keeps of a stored record (1-6), COVERS and RELAXES (2-3). The same steps, outcomes and reason
//! codes as the client core, so that the tool never signs what an app would refuse, and the shared
//! vectors (tests/vectors) hold for both.
//!
//! In short:
//!
//! - An entry blocks below `grace_min` until `grace_until` and below `min` from then on. A new
//!   policy is taken only if it asks no less than the device's basis at every time (COVERS), entry
//!   by entry and for the Android pair.
//! - Only a root key's **declaration** asks less: every declaration opens a new epoch and carries
//!   the revoked keys and a base. A device takes a declaration first and keeps it whatever happens
//!   to the policy in the same envelope; of the two signed parts it holds, the later wins.
//! - A policy signed by a root key has no special rights; every policy of epoch 2 or later comes
//!   with its epoch's declaration.
//!
//! The tool reads canonical texts only ([`canonical`]): an envelope whose form is not exactly what
//! the tool writes is refused before the app's steps, by the tool's strict layer. The client
//! alone decides such texts (spec 4-1). The tool's own rules beyond the app's (no URL in a text,
//! no root key number in `revoked`, ...) are in [`check_envelope`], which also refuses what the
//! app would take.

use crate::{
    envelope::{Envelope, Part, ENVELOPE_VERSION, MAX_ENVELOPE_BYTES, POLICY_DOMAIN, ROOT_DOMAIN},
    keys::{Floor, KeySet, Role},
    policy::{DeclForm, Platform, Policy, PolicyForm, Requirement, RootDecl},
    time,
    version::Version,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// How far above the position a device compares with a policy may be (C5), for every signer: a
/// thief of a signing key cannot run `seq` to its end and so shut out every later policy.
pub const MAX_SEQ_STEP: u64 = 1000;

pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Why an app refuses an envelope (trust rules 2-4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    TooLarge,
    Envelope,
    Version,
    RootKey,
    RootSignature,
    RootFormat,
    RootServer,
    RootRevokesRoot,
    RootEpoch,
    RootFork,
    UnknownKey,
    RevokedKey,
    BadSignature,
    /// A member of the policy text breaks its rule (`json` for its form).
    Field(&'static str),
    Server,
    Epoch,
    Fork,
    Rollback,
    Jump,
    BelowFloor,
    Issued,
    Relaxation,
}

impl Reason {
    /// The code as the vectors write it: `Rollback`, `Field:grace_min`.
    pub fn code(&self) -> String {
        match self {
            Reason::Field(member) => format!("Field:{member}"),
            other => format!("{other:?}"),
        }
    }

    pub fn korean(&self) -> String {
        let text = match self {
            Reason::TooLarge => "봉투가 16384바이트보다 큽니다",
            Reason::Envelope => "봉투의 형식이 맞지 않습니다",
            Reason::Version => "모르는 버전입니다(v)",
            Reason::RootKey => "루트 선언의 키가 이 빌드가 박은 루트 키가 아닙니다",
            Reason::RootSignature => "루트 선언의 서명이 맞지 않습니다",
            Reason::RootFormat => "루트 선언의 형식이나 규칙이 맞지 않습니다(세대 2 이상, base)",
            Reason::RootServer => "루트 선언의 서버 이름이 이 빌드의 서버가 아닙니다",
            Reason::RootRevokesRoot => "루트 선언이 이 빌드가 박은 루트 키 번호를 폐기합니다",
            Reason::RootEpoch => "루트 선언의 세대가 기기의 세대보다 낮습니다",
            Reason::RootFork => "기기의 세대에 다른 루트 선언이 이미 있습니다",
            Reason::UnknownKey => "정책의 키가 이 빌드가 박은 키가 아닙니다",
            Reason::RevokedKey => "정책의 키가 폐기된 키입니다",
            Reason::BadSignature => "정책의 서명이 맞지 않습니다",
            Reason::Field(member) => return format!("정책의 {member} 값이 규칙에 맞지 않습니다"),
            Reason::Server => "정책의 서버 이름이 이 빌드의 서버가 아닙니다",
            Reason::Epoch => {
                "정책의 세대가 기기의 세대가 아니거나, 세대 2 이상의 정책에 그 세대의 루트 선언이 없습니다"
            }
            Reason::Fork => "같은 위치에 다른 내용의 정책입니다",
            Reason::Rollback => "기기가 가진 위치보다 높지 않은 정책입니다(되돌림)",
            Reason::Jump => "seq가 기기가 가진 위치에서 1000 넘게 뜁니다",
            Reason::BelowFloor => "빌드 하한보다 낮은 정책입니다",
            Reason::Issued => "기기가 비교하는 정책(또는 base)보다 일찍 발행된 정책입니다",
            Reason::Relaxation => "기기가 비교하는 정책(또는 base)보다 덜 막는 정책입니다(완화)",
        };
        text.to_owned()
    }
}

/// What `accept` did with an envelope (2-1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Taken,
    Unchanged,
    Refused(Reason),
}

impl Outcome {
    pub fn is_taken(&self) -> bool {
        matches!(self, Outcome::Taken | Outcome::Unchanged)
    }

    pub fn reason(&self) -> Option<Reason> {
        match self {
            Outcome::Refused(reason) => Some(*reason),
            _ => None,
        }
    }
}

/// The held declaration: its root part as received and its text read (1-5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Declared {
    pub part: Part,
    pub decl: RootDecl,
}

/// The held policy: its whole envelope as received, the policy, the signer's number, SHA-256(p)
/// and the root part that envelope carried (1-5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Held {
    pub file: String,
    pub policy: Policy,
    pub k: u32,
    pub hash: [u8; 32],
    pub root: Option<Part>,
}

/// One device's state besides its build (1-5). A fresh install holds nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub decl: Option<Declared>,
    pub held: Option<Held>,
}

/// What a device enforces (`EFF`): its held policy, or its declaration's base read as a policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Effective {
    pub entries: BTreeMap<Platform, Requirement>,
    pub issued: i64,
    pub epoch: u64,
    pub seq: u64,
    /// Whether this is the base of the held declaration.
    pub base: bool,
}

/// What a new policy is compared with (`BASIS`): entries and `issued`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Basis {
    pub entries: BTreeMap<Platform, Requirement>,
    pub issued: i64,
}

impl State {
    /// `DE`: the held declaration's epoch, or 1.
    pub fn declared_epoch(&self) -> u64 {
        self.decl.as_ref().map_or(1, |d| d.decl.epoch)
    }

    /// `E`: the device epoch, `max(floor.epoch, DE)`.
    pub fn epoch(&self, keys: &KeySet) -> u64 {
        keys.floor.epoch.max(self.declared_epoch())
    }

    /// `REVOKED`: the held declaration's list.
    pub fn revoked(&self) -> &[u32] {
        self.decl.as_ref().map_or(&[], |d| &d.decl.revoked)
    }

    /// `CUR`: the held policy if it is of the device epoch.
    pub fn current(&self, keys: &KeySet) -> Option<&Held> {
        let e = self.epoch(keys);
        self.held.as_ref().filter(|h| h.policy.epoch == e)
    }

    /// `DCUR`: the held declaration if it is of the device epoch.
    pub fn current_declaration(&self, keys: &KeySet) -> Option<&Declared> {
        let e = self.epoch(keys);
        self.decl.as_ref().filter(|d| d.decl.epoch == e)
    }

    /// `LOW` and `SAME` (the seq and hash of a current policy at or above the floor).
    pub fn low_same(&self, keys: &KeySet) -> (u64, Option<(u64, [u8; 32])>) {
        let e = self.epoch(keys);
        if let Some(cur) = self.current(keys) {
            let at = Floor {
                epoch: e,
                seq: cur.policy.seq,
            };
            if at >= keys.floor {
                return (cur.policy.seq, Some((cur.policy.seq, cur.hash)));
            }
        }
        let low = if keys.floor.epoch == e && keys.floor.seq >= 1 {
            keys.floor.seq - 1
        } else {
            0
        };
        (low, None)
    }

    /// `BASIS`: the current policy, else the current declaration's base, else none.
    pub fn basis(&self, keys: &KeySet) -> Option<Basis> {
        if let Some(cur) = self.current(keys) {
            return Some(Basis {
                entries: cur.policy.requirements(),
                issued: cur.policy.issued,
            });
        }
        self.current_declaration(keys).map(|d| Basis {
            entries: d.decl.base.clone(),
            issued: d.decl.issued,
        })
    }

    /// `EFF`: the later of the two signed parts wins.
    pub fn effective(&self) -> Option<Effective> {
        let de = self.declared_epoch();
        if let Some(held) = self
            .held
            .as_ref()
            .filter(|h| h.policy.epoch == de && !self.revoked().contains(&h.k))
        {
            return Some(Effective {
                entries: held.policy.requirements(),
                issued: held.policy.issued,
                epoch: held.policy.epoch,
                seq: held.policy.seq,
                base: false,
            });
        }
        self.decl.as_ref().map(|d| Effective {
            entries: d.decl.base.clone(),
            issued: d.decl.issued,
            epoch: d.decl.epoch,
            seq: 0,
            base: true,
        })
    }

    /// `MAY_INSTALL`: `SAME` exists and the held policy's signer is not revoked.
    pub fn may_install(&self, keys: &KeySet) -> bool {
        self.low_same(keys).1.is_some()
            && self
                .held
                .as_ref()
                .is_some_and(|h| !self.revoked().contains(&h.k))
    }

    /// What the vectors call `enforces`.
    pub fn enforces(&self) -> &'static str {
        match self.effective() {
            None => "nothing",
            Some(e) if e.base => "base",
            Some(_) => "policy",
        }
    }
}

// ---------------------------------------------------------------------------------------------
// COVERS, RELAXES, RELAXED (2-3)
// ---------------------------------------------------------------------------------------------

/// `EARLIER(a, b)`: a is none, or both are times and a <= b.
fn earlier(a: Option<i64>, b: Option<i64>) -> bool {
    match (a, b) {
        (None, _) => true,
        (Some(a), Some(b)) => a <= b,
        (Some(_), None) => false,
    }
}

/// `COVERS(n, o)`: entry `n` asks at least what `o` asks, at every time, past included.
pub fn covers(n: &Requirement, o: &Requirement) -> bool {
    n.lo() >= o.hi()
        || (n.lo() >= o.lo() && n.hi() >= o.hi() && earlier(n.deadline(), o.deadline()))
}

/// How an entry asks less than before (the words of X1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Loosening {
    Dropped,
    LatestLowered,
    MinLowered,
    /// The same minimum with a later deadline, or with one where there was none.
    DeadlineLater,
    /// The same minimum and deadline, with a lower `grace_min`.
    GraceMinLowered,
    /// A higher minimum whose grace period lets the old one go before its deadline (problem (가)).
    RaiseLetsOldGo,
    /// A new Android entry that asks less than its sibling (problem (나)).
    BelowSibling,
}

impl Loosening {
    pub fn korean(self) -> &'static str {
        match self {
            Loosening::Dropped => "항목을 뺌",
            Loosening::LatestLowered => "latest를 내림",
            Loosening::MinLowered => "최소 버전을 내림",
            Loosening::DeadlineLater => "유예 끝을 늦추거나 새로 둠",
            Loosening::GraceMinLowered => "유예 중에 막는 버전(grace_min)을 내림",
            Loosening::RaiseLetsOldGo => {
                "최소 버전을 올리면서 지금 막는 버전을 유예 끝까지 풀어 줌"
            }
            Loosening::BelowSibling => "다른 Android 항목보다 덜 막는 새 항목",
        }
    }
}

/// Every way `p` asks less than `b`, entry by entry (clauses 1-3 of RELAXES).
pub fn loosenings(
    b: &BTreeMap<Platform, Requirement>,
    p: &BTreeMap<Platform, Requirement>,
) -> Vec<(Platform, Loosening)> {
    let mut out = Vec::new();
    for (c, o) in b {
        let Some(n) = p.get(c) else {
            out.push((*c, Loosening::Dropped));
            continue;
        };
        if !covers(n, o) {
            let kind = if n.hi() < o.hi() {
                Loosening::MinLowered
            } else if n.hi() > o.hi() {
                Loosening::RaiseLetsOldGo
            } else if !earlier(n.deadline(), o.deadline()) {
                Loosening::DeadlineLater
            } else {
                Loosening::GraceMinLowered
            };
            out.push((*c, kind));
        }
        if n.latest < o.latest {
            out.push((*c, Loosening::LatestLowered));
        }
    }
    for c in [Platform::Android, Platform::AndroidPlay] {
        let Some(sibling) = c.sibling() else { continue };
        if let (Some(n), None, Some(o)) = (p.get(&c), b.get(&c), b.get(&sibling)) {
            if !covers(n, o) {
                out.push((c, Loosening::BelowSibling));
            }
        }
    }
    out.sort();
    out
}

/// `RELAXED(b, p)`: the entries for which a clause holds.
pub fn relaxed(
    b: &BTreeMap<Platform, Requirement>,
    p: &BTreeMap<Platform, Requirement>,
) -> BTreeSet<Platform> {
    loosenings(b, p).into_iter().map(|(c, _)| c).collect()
}

/// `RELAXES(b, p)`.
pub fn relaxes(b: &BTreeMap<Platform, Requirement>, p: &BTreeMap<Platform, Requirement>) -> bool {
    !loosenings(b, p).is_empty()
}

// ---------------------------------------------------------------------------------------------
// Reading an envelope: the tool's form first, then the app's steps
// ---------------------------------------------------------------------------------------------

/// An envelope whose texts are canonical (the form the tool writes), read for its form.
#[derive(Clone, Debug)]
pub struct Canonical {
    pub v: u64,
    pub envelope: Envelope,
    pub policy: PolicyForm,
    pub root: Option<DeclForm>,
}

/// Whether `file` is exactly what the tool writes (trust rules 7-4 `canonical`): one line and a
/// line feed; members known, of their types, in their order, none twice, none `null`; a key
/// number 1-65535 and a canonical 64-byte signature in every part; a policy with its required
/// members. The values are not judged here.
pub fn canonical(file: &[u8]) -> Result<Canonical, String> {
    let text = std::str::from_utf8(file).map_err(|_| "봉투가 UTF-8이 아닙니다".to_owned())?;
    let (v, envelope) = Envelope::read_form(text)?;
    if text != format!("{}\n", envelope.to_text_v(v)) {
        return Err("봉투가 이 도구가 쓰는 형식(한 줄과 줄바꿈, 공백 없음)과 다릅니다".to_owned());
    }
    let policy = PolicyForm::canonical(&envelope.policy.p)?;
    let root = envelope
        .root
        .as_ref()
        .map(|part| DeclForm::canonical(&part.p))
        .transpose()?;
    Ok(Canonical {
        v,
        envelope,
        policy,
        root,
    })
}

/// B1-B3 for a root part: a pinned root key's number, its signature, the text's rules, the
/// server, no pinned root key revoked.
fn check_root_part(
    keys: &KeySet,
    server: &str,
    part: &Part,
    form: &DeclForm,
    signatures: bool,
) -> Result<RootDecl, Reason> {
    let key = keys.root(part.k).ok_or(Reason::RootKey)?;
    if signatures && !part.verifies(&key.key, ROOT_DOMAIN) {
        return Err(Reason::RootSignature);
    }
    let decl = form.judge(Some(server))?;
    if decl.revoked.iter().any(|n| keys.is_root(*n)) {
        return Err(Reason::RootRevokesRoot);
    }
    Ok(decl)
}

/// The key a policy part names (C1 without the revocation): a pinned root key, else a pinned
/// signing key.
fn policy_key(keys: &KeySet, k: u32) -> Option<(Role, &crate::keys::PublicKey)> {
    keys.root(k)
        .map(|key| (Role::Root, key))
        .or_else(|| keys.find(k).filter(|(role, _)| *role == Role::Signing))
}

/// `accept(build, state, body)` of trust rules 2-1 and 2-2 for a canonical body: steps A, B and
/// C in order, every refusal with the app's reason, a declaration taken in step B kept on a
/// later refusal. An envelope that is not canonical is refused before the app's steps
/// (`Err`): the tool does not read it.
pub fn accept(
    keys: &KeySet,
    server: &str,
    state: &State,
    body: &[u8],
) -> Result<(State, Outcome), String> {
    steps(keys, server, state, body, true)
}

/// The same for an envelope whose signatures are not made yet (a draft): every step but the
/// two signature checks (B2, C2), which are taken as good. The key numbers must still be pinned.
pub fn accept_draft(
    keys: &KeySet,
    server: &str,
    state: &State,
    body: &[u8],
) -> Result<(State, Outcome), String> {
    steps(keys, server, state, body, false)
}

fn steps(
    keys: &KeySet,
    server: &str,
    state: &State,
    body: &[u8],
    signatures: bool,
) -> Result<(State, Outcome), String> {
    // A1
    if body.len() > MAX_ENVELOPE_BYTES {
        return Ok((state.clone(), Outcome::Refused(Reason::TooLarge)));
    }
    // A2, by the tool's form
    let read = canonical(body)?;
    if read.v != ENVELOPE_VERSION {
        return Ok((state.clone(), Outcome::Refused(Reason::Version)));
    }
    let envelope = &read.envelope;
    // B
    let mut s1 = state.clone();
    let mut declared: Option<RootDecl> = None;
    if let (Some(part), Some(form)) = (&envelope.root, &read.root) {
        let decl = match check_root_part(keys, server, part, form, signatures) {
            Ok(decl) => decl,
            Err(reason) => return Ok((state.clone(), Outcome::Refused(reason))),
        };
        let e = state.epoch(keys);
        if decl.epoch < e {
            return Ok((state.clone(), Outcome::Refused(Reason::RootEpoch)));
        }
        match state.current_declaration(keys) {
            Some(dcur) if decl.epoch == e => {
                if dcur.part.p != part.p {
                    return Ok((state.clone(), Outcome::Refused(Reason::RootFork)));
                }
            }
            _ => {
                s1.decl = Some(Declared {
                    part: part.clone(),
                    decl: decl.clone(),
                });
            }
        }
        declared = Some(decl);
    }
    let refuse = |s: &State, reason| Ok((s.clone(), Outcome::Refused(reason)));
    // C1
    let k = envelope.policy.k;
    let Some((_, key)) = policy_key(keys, k) else {
        return refuse(&s1, Reason::UnknownKey);
    };
    if s1.revoked().contains(&k) {
        return refuse(&s1, Reason::RevokedKey);
    }
    // C2
    if signatures && !envelope.policy.verifies(&key.key, POLICY_DOMAIN) {
        return refuse(&s1, Reason::BadSignature);
    }
    // C3
    let policy = match read.policy.judge(Some(server)) {
        Ok(policy) => policy,
        Err(reason) => return refuse(&s1, reason),
    };
    // C4
    let e = s1.epoch(keys);
    if policy.epoch != e {
        return refuse(&s1, Reason::Epoch);
    }
    if policy.epoch >= 2 && declared.as_ref().map(|d| d.epoch) != Some(policy.epoch) {
        return refuse(&s1, Reason::Epoch);
    }
    // C5
    let hash = sha256(envelope.policy.p.as_bytes());
    let (low, same) = s1.low_same(keys);
    if let Some((_, same_hash)) = same.filter(|(seq, _)| *seq == policy.seq) {
        if hash == same_hash {
            return Ok((s1, Outcome::Unchanged));
        }
        return refuse(&s1, Reason::Fork);
    }
    if policy.seq <= low {
        return refuse(&s1, Reason::Rollback);
    }
    if policy.seq - low > MAX_SEQ_STEP {
        return refuse(&s1, Reason::Jump);
    }
    // C6
    let at = Floor {
        epoch: e,
        seq: policy.seq,
    };
    if at < keys.floor {
        return refuse(&s1, Reason::BelowFloor);
    }
    // C7, C8
    if let Some(basis) = s1.basis(keys) {
        if policy.issued < basis.issued {
            return refuse(&s1, Reason::Issued);
        }
        if relaxes(&basis.entries, &policy.requirements()) {
            return refuse(&s1, Reason::Relaxation);
        }
    }
    // C9
    let file = std::str::from_utf8(body).unwrap_or_default().to_owned();
    s1.held = Some(Held {
        file,
        policy,
        k,
        hash,
        root: envelope.root.clone(),
    });
    Ok((s1, Outcome::Taken))
}

/// LOAD of a stored record (1-6, L2-L5) under the build `keys`: the declaration `root` and the
/// held envelope `envelope`, each kept only if it verifies under this build, the held policy
/// only if it agrees with the declaration kept. The tool reads canonical texts only.
pub fn load(keys: &KeySet, server: &str, root: Option<&Part>, envelope: Option<&str>) -> State {
    // L2
    let decl = root.and_then(|part| {
        let form = DeclForm::canonical(&part.p).ok()?;
        let decl = check_root_part(keys, server, part, &form, true).ok()?;
        Some(Declared {
            part: part.clone(),
            decl,
        })
    });
    // L3
    let held = envelope.and_then(|file| {
        if file.len() > MAX_ENVELOPE_BYTES {
            return None;
        }
        let read = canonical(file.as_bytes()).ok()?;
        if read.v != ENVELOPE_VERSION {
            return None;
        }
        let envelope = read.envelope;
        let inner = match (&envelope.root, &read.root) {
            (Some(part), Some(form)) => Some(check_root_part(keys, server, part, form, true).ok()?),
            _ => None,
        };
        let (_, key) = policy_key(keys, envelope.policy.k)?;
        if !envelope.policy.verifies(&key.key, POLICY_DOMAIN) {
            return None;
        }
        let policy = read.policy.judge(Some(server)).ok()?;
        // L4
        let consistent = if policy.epoch == 1 {
            envelope.root.is_none()
        } else {
            inner.as_ref().is_some_and(|d| d.epoch == policy.epoch)
                && decl.as_ref().is_some_and(|d| {
                    d.decl.epoch > policy.epoch
                        || (d.decl.epoch == policy.epoch
                            && envelope.root.as_ref().is_some_and(|r| r.p == d.part.p))
                })
        };
        consistent.then(|| Held {
            file: file.to_owned(),
            hash: sha256(envelope.policy.p.as_bytes()),
            k: envelope.policy.k,
            root: envelope.root.clone(),
            policy,
        })
    });
    // L5: none and none is a fresh start
    State { decl, held }
}

/// The state a device has after it took `file` and nothing later: LOAD of a record holding
/// `file` and its root part (X4 a, `floor --live`, the server check's item 4).
pub fn holding(keys: &KeySet, server: &str, file: &str) -> State {
    let root = Envelope::read_form(file).ok().and_then(|(_, e)| e.root);
    load(keys, server, root.as_ref(), Some(file))
}

// ---------------------------------------------------------------------------------------------
// One envelope by itself, as the tool reads its own files
// ---------------------------------------------------------------------------------------------

/// An envelope that passed every check the tool makes without a device's state.
#[derive(Clone, Debug)]
pub struct Checked {
    pub envelope: Envelope,
    pub policy: Policy,
    pub root: Option<RootDecl>,
    /// The role of the key that signed the policy.
    pub role: Role,
    pub p_hash: [u8; 32],
}

/// The server an envelope names, read before anything is verified: only to know which server's
/// rules to check the envelope against.
pub fn peek_server(file: &str) -> Result<String, String> {
    let (_, envelope) = Envelope::read_form(file)?;
    Ok(PolicyForm::read(&envelope.policy.p)?.server)
}

/// Checks an envelope by itself with the tool's strict layer: canonical, every signature by a
/// pinned key, the app's rules for both texts (steps A-C without a device's state), a policy of
/// epoch 2 or later with its epoch's declaration and not signed by a key it revokes, and the
/// tool's own rules ([`Policy::check`], [`RootDecl::check`]).
pub fn check_envelope(file: &str, keys: &KeySet, server: &str) -> Result<Checked, String> {
    if file.len() > MAX_ENVELOPE_BYTES {
        return Err(format!(
            "봉투가 {}바이트입니다. 앱은 {MAX_ENVELOPE_BYTES}바이트까지만 읽습니다",
            file.len()
        ));
    }
    let read = canonical(file.as_bytes())?;
    if read.v != ENVELOPE_VERSION {
        return Err(format!("봉투: {}", Reason::Version.korean()));
    }
    let envelope = read.envelope;
    let root = match (&envelope.root, &read.root) {
        (Some(part), Some(form)) => Some(
            check_root_part(keys, server, part, form, true)
                .map_err(|r| format!("루트 선언: {}", r.korean()))?,
        ),
        _ => None,
    };
    let k = envelope.policy.k;
    let (role, key) =
        policy_key(keys, k).ok_or_else(|| format!("이 빌드가 믿는 키가 아닙니다(키 번호 {k})"))?;
    if root.as_ref().is_some_and(|r| r.revoked.contains(&k)) {
        return Err(format!("폐기된 키입니다(키 번호 {k})"));
    }
    if !envelope.policy.verifies(&key.key, POLICY_DOMAIN) {
        return Err(Reason::BadSignature.korean());
    }
    let policy = read
        .policy
        .judge(Some(server))
        .map_err(|r| format!("정책: {}", r.korean()))?;
    match (&root, policy.epoch) {
        (None, 1) => {}
        (Some(r), epoch) if epoch >= 2 && r.epoch == epoch => {}
        _ => {
            return Err(format!(
                "정책의 세대({})와 루트 선언이 맞지 않습니다(세대 1은 선언 없이, 세대 2 이상은 그 세대의 선언과 함께)",
                policy.epoch
            ))
        }
    }
    policy.check()?;
    if let Some(r) = &root {
        r.check()?;
    }
    Ok(Checked {
        p_hash: sha256(envelope.policy.p.as_bytes()),
        envelope,
        policy,
        root,
        role,
    })
}

/// The new entries whose `min` is below the lowest `min` of `old`: hbbs's one threshold would go
/// down (X8, the tool's own rule).
pub fn new_entries_below(old: &Policy, new: &Policy) -> Vec<(Platform, Version, Version)> {
    let Some(lowest) = old.lowest_min() else {
        return Vec::new();
    };
    new.platforms
        .iter()
        .filter(|(p, e)| !old.platforms.contains_key(p) && e.min < lowest)
        .map(|(p, e)| (*p, e.min, lowest))
        .collect()
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
        if let (Some(t), Some(grace_min)) = (e.grace_until, e.grace_min) {
            line.push_str(&format!(
                " · 유예 끝 {} (그때까지는 {grace_min} 미만만 막음)",
                time::format_kst(t)
            ));
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
        match &e.package {
            Some(p) => lines.push(format!(
                "  패키지 {} · {}바이트 · build {} · sha256 {}",
                p.file, p.size, p.build, p.sha256
            )),
            None if platform.has_package() => lines.push("  패키지 없음(거둬들임)".to_owned()),
            None => {}
        }
        if let Some(notes) = &e.notes_ko {
            lines.push(format!("  알림: {}", notes.replace('\n', " / ")));
        }
    }
    lines
}

/// A declaration for people: epoch, revoked keys, and the base per entry.
pub fn describe_declaration(decl: &RootDecl) -> Vec<String> {
    let mut lines = vec![format!(
        "루트 선언: 세대 {} · 폐기한 서명 키 {:?} · 발행 {}",
        decl.epoch,
        decl.revoked,
        time::format_kst(decl.issued)
    )];
    for (platform, r) in &decl.base {
        lines.push(format!(
            "  base {platform}: 최신 {} · {}",
            r.latest,
            r.describe()
        ));
    }
    lines
}

#[cfg(test)]
#[path = "trust_tests.rs"]
mod tests;

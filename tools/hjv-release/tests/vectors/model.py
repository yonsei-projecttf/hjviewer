#!/usr/bin/env python3
"""HJViewer version policy: executable reference model of the trust rules, and a brute-force checker.

Round 3 (2026-10-14). Transcribes build/minver/trust-rules/spec.md as revised in rounds 1 and 2: EFF
"the later signed part wins", MAY_INSTALL with the floor, two pinned root keys, LOAD part by part
without a fingerprint, SAVE by RANK under LOAD, the archive with its key set records (X12, X9 by the
records, X2's root range, X10 and X11 with `void`), `floor --live` from the archive only, and the
runbook of section 5. Round 2's checker model and the judge's round-2 patch of it are merged into
this one file and checked again against the text. Pure standard library, Python 3.8 or later.

New in round 3: the state keeps the held declaration's root part (its signer), so LOAD under other
root keys and two signers of one text are exact; appendix A's `load` is transcribed and compared
with LOAD on every record and restart; `floor --live` verifies the live file with the key file
(4-7) for every build, so runbook 5-10 (a root key lost) is searched with declarations by either
root key before the loss, alone and with a theft; INV-16 (hbbs-conf) and INV-17 (the signers, also
of the root part an envelope carries unchanged) are checked; the two-process check lets either
process save first (round 2 only let the higher RANK save last, so SAVE's "write nothing" branch
was never exercised there); 4-2's package rule (a new latest brings its package back) and 5-8's
revocation of every retired key are followed; the 7-4 cases of round 2 are added.

Parts 1-7 are the reference: every rule is written the way the spec states it, step by step and with
the spec's names, so that these parts can serve as the oracle for the shared test vectors: accept()
with its reason codes, the derived values (EFF, MAY_INSTALL, ...), load()/save()/rank(), the
verdicts, hbbs-conf's decision, the tool's derivations and refusals and the server check. Parts 8-11
generate histories and check the invariants of spec 6-1 and the goals G1-G6 of the task:

  * operator histories (an honest operator following runbook 5; its named mistakes; a theft with the
    recovery of 5-8; a theft during the rotation of 5-9; both signing keys stolen; a root key lost,
    5-10, alone and with a theft), each with the closure of every device state reachable by any
    order of the envelopes N and S can serve (devices that see everything, skip some, are off for a
    stretch, restart under any build, update to builds of another key set, are installed fresh at
    any point), two processes on one record, and the liveness probe (INV-11; G2 after a recovery:
    every state takes the live envelope, is then "back" and stays back);
  * random operator histories of ten steps mixing all of the above;
  * a fuzzer of arbitrary signed envelopes (not only what a correct tool makes) for the app-side
    rules; every transition and every LOAD is also decided by a literal transcription of the spec's
    appendix A, which must agree with sections 1-6 and 2;
  * broken variants of the rules, each of which must be caught.

    nice -n 10 python3 -I model.py cases          # spec 7-4 and the store checks of 7-6
    nice -n 10 python3 -I model.py cases --json   # the 7-4 cases, one JSON line each (vector oracle)
    nice -n 10 python3 -I model.py covers         # covers.json of 7-4 (the 256 entry pairs)
    nice -n 10 python3 -I model.py pure           # INV-1, INV-2 (with the verdicts), INV-3, INV-14, INV-16
    nice -n 10 python3 -I model.py search         # the history searches (INV-4..INV-17, G1-G6)
    nice -n 10 python3 -I model.py variants       # each broken variant must be caught
    nice -n 10 python3 -I model.py all            # all of it (the default), about 10 minutes of CPU
  options: --budget S   CPU seconds for the history searches (default 350; several end earlier)
           --quick      a tenth of every budget
           --only TEXT  only the searches whose name contains TEXT

Layout (each part names the spec section it transcribes):
  1  numbers, times, entries: THRESHOLD, COVERS, RELAXED, RELAXES, LIMIT   (1-2, 2-3, 3-3)
  2  texts: policies, declarations, signed parts, envelopes                (1-3)
  3  builds, the device state, derived values                             (1-4, 1-5)
  4  accept(): steps A, B, C and the reason codes                         (2-1 .. 2-5)
  5  the stored record: LOAD (part by part), SAVE, RANK                   (1-6)
  6  verdicts: clock, app state, the PC's rule, installs, hbbs-conf        (3, 4-7)
  7  the tool: derivations, archive, records, signer, X1-X12, server check (4)
  8  the world: the operator (runbook 5 and named mistakes), N, S, O      (5; the task's threats)
  9  invariants and goals; appendix A transcribed                         (6-1; G1-G6; appendix A)
  10 the fixed cases of spec 7-4 and the store checks of 7-6              (7-4, 7-6)
  11 search drivers, the fuzzer and the report

What stands for what:
  * versions are num() values (small integers), times are integers, None is "absent";
  * a signature is a flag: a part is `valid` when it was made with the key it names; who can make
    which part is the attacker model's business (N: none; S: a stolen signing key; O: S once that key
    is revoked); the root keys are never stolen (out of scope), but one may be lost (5-10);
  * texts are interned objects, so `is` stands for comparing the bytes (C5's SHA-256(p), B4b's and
    L4's "p byte for byte"); members no rule reads are folded into `notes`, windows'
    auto_install_after into `auto` and the packages into `pkg`; a text that the JSON rules of 2-2 or
    a member the model does not represent would refuse carries that code in its part's `form`;
  * the state keeps the held declaration's root part (signer and text), as the record does (1-5,
    1-6), so that LOAD under a build of other root keys is exact; a device is one process under one
    build; "two processes on one record" pairs two states;
  * the server name is fixed, so a build's key set (the fingerprint of 1-4) is (signing keys, root
    keys);
  * accept() reads no clock; the verdicts do, and G1 is judged at every device clock
    (now = max(clock, EFF.issued), part 9's clamped()).
Not modelled: the JSON reading rules of 2-2 (only their codes, through `form`), sizes, Ed25519,
texts, the Date pull-back of 3-1, the limiter and checked_at, packages beyond present/withdrawn/
forged, the Windows service's timing (only MAY_INSTALL and the install target), iOS/Play specifics,
the server check's items 1, 2, 6 and 8 (bytes, signatures, the clock, package files), hbbs beyond
hbbs-conf's decision (INV-16 is checked on its own), the operator's clean machine and archive
copies (5-0) other than as "the archive", and a root key that is stolen.

Kinds the report uses: "INV-n" and "G1".."G6" are violations of spec 6-1 and of the task's goals;
"G4 (runbook ...)" is a runbook step that cannot be carried out as written; kinds that start with
"note" are what 6-1 and 6-2 allow (INV-11's exceptions, R1, R2/R3, R10, ...) and are counted, with the
smallest trace, so that their reach can be judged.
"""

from __future__ import annotations

import argparse
import collections
import itertools
import json
import random
import sys
import time
from typing import NamedTuple, Optional


# ================================================================================================
# 1. Numbers, times, entries                                             (spec 1-2, 2-3, 3-3)
# ================================================================================================

STEP = 1000                     # C5: how far one policy may move seq (Jump)
NAMES = ("windows", "android", "android_play", "ios")
ANDROID = ("android", "android_play")
FAMILIES = {"windows": ("windows",), "android": ANDROID, "ios": ("ios",)}

# Broken variants switch one rule off on purpose (part 9 lists them); empty: the rules as written.
V: set = set()


def sibling(c):
    return ANDROID[1] if c == ANDROID[0] else ANDROID[0]


class Entry(NamedTuple):
    """The requirement part of an entry of a policy or of a base (1-3): `min`, `latest`, and the
    pair `gu` (grace_until) / `gm` (grace_min), both None or both set."""
    min: int
    latest: int
    gu: Optional[int] = None
    gm: Optional[int] = None


def entry_error(e) -> Optional[str]:
    """The member whose rule of 1-3 `e` breaks first, in C3's order, or None: num(min) <=
    num(latest) is Field(min); the pair rule and num(grace_min) < num(min) are Field(grace_min).
    (B3 refuses a base entry that breaks one of them as RootFormat.)"""
    if e.min > e.latest:
        return "min"
    if (e.gu is None) != (e.gm is None) or (e.gm is not None and not e.gm < e.min):
        return "grace_min"
    return None


def lht(e):
    """1-2: (lo, hi, T) = (num(grace_min), num(min), grace_until) with the pair, else
    (num(min), num(min), NONE)."""
    return (e.gm, e.min, e.gu) if e.gu is not None else (e.min, e.min, None)


def threshold(e, t):
    """THRESHOLD(e, t) of 1-2. A version v is blocked by e at time t iff num(v) < THRESHOLD(e, t);
    at t == grace_until the minimum holds."""
    lo, hi, T = lht(e)
    return hi if T is None or t >= T else lo


def earlier(a, b):
    """EARLIER(a, b) of 2-3: a is NONE, or a and b are times and a <= b."""
    return a is None or (b is not None and a <= b)


def covers(n, o):
    """COVERS(n, o) of 2-3: entry n asks at least what entry o asks, at every time, past included."""
    nlo, nhi, nT = lht(n)
    olo, ohi, oT = lht(o)
    return nlo >= ohi or (nlo >= olo and nhi >= ohi and earlier(nT, oT))


def relaxed(B, P) -> frozenset:
    """RELAXED(B, P) of 2-3 (B and P map entry names to entries): the names c for which
      1. c is in B and missing in P, or
      2. c is in both and not COVERS(P.c, B.c) or num(P.c.latest) < num(B.c.latest), or
      3. c of the Android pair is in P, not in B, its sibling o is in B, and not COVERS(P.c, B.o)."""
    out = set()
    for c, o in B.items():
        n = P.get(c)
        if n is None or not covers(n, o) or n.latest < o.latest:
            out.add(c)
    for c in ANDROID:
        o = sibling(c)
        if c in P and c not in B and o in B and not covers(P[c], B[o]):
            out.add(c)
    return frozenset(out)


def relaxes_ref(B, P):
    """RELAXES(B, P) of 2-3: RELAXED(B, P) is not empty."""
    return bool(relaxed(B, P))


def relaxes(B, P):
    """RELAXES as the app and the tool apply it in this run: the spec's, or a broken variant's.
    The checks of part 9 use relaxes_ref, which no variant touches."""
    if "today" in V:
        return relaxes_today(B, P)
    if "no_android_clause" in V:
        return any(c not in P or not covers(P[c], o) or P[c].latest < o.latest for c, o in B.items())
    return relaxes_ref(B, P)


def relaxes_today(B, P):
    """Broken variant 'today' (today's entry_relaxed/relaxes in trust.rs): a lowered min or latest,
    the same min with a later deadline or a new one, a dropped entry; a new entry never counts;
    grace_min does not exist."""
    for c, o in B.items():
        n = P.get(c)
        if n is None or n.min < o.min or n.latest < o.latest:
            return True
        if n.min == o.min and n.gu is not None and (o.gu is None or n.gu > o.gu):
            return True
    return False


def family_entries(ents, family):
    return [ents[c] for c in FAMILIES[family] if c in ents]


def family_limit(ents, family, t):
    """LIMIT(t) of 3-3: the lowest THRESHOLD(e, t) over the family's entries present; None when
    none is (the family is not checked)."""
    es = family_entries(ents, family)
    return min(threshold(e, t) for e in es) if es else None


def cut_times(*entry_maps):
    """Times between which every threshold of these entries is constant: one before every deadline
    and, for every deadline, the deadline and the time just before it."""
    ts = {-(10 ** 9)}
    for m in entry_maps:
        for e in m.values():
            if e.gu is not None:
                ts.update((e.gu - 1, e.gu))
    return sorted(ts)


def asks_less_somewhere(old, new):
    """INV-2's right-hand side by brute force over time: an entry of `old` missing in `new`, a lower
    latest, or an entry's THRESHOLD or a family's LIMIT lower at some time."""
    for c, o in old.items():
        n = new.get(c)
        if n is None or n.latest < o.latest:
            return True
    for t in cut_times(old, new):
        for c, o in old.items():
            if threshold(new[c], t) < threshold(o, t):
                return True
        for fam in FAMILIES:
            lo = family_limit(old, fam, t)
            if lo is not None and family_limit(new, fam, t) < lo:
                return True
    return False


# ================================================================================================
# 2. Texts: policies, declarations, signed parts, envelopes                            (spec 1-3)
# ================================================================================================

class Policy:
    """A policy text p, reduced to what the rules read. Interned by policy(): one object per text,
    so `is` means "the same bytes"."""
    __slots__ = ("epoch", "seq", "issued", "ents", "auto", "pkg", "notes", "label", "serial")

    def __hash__(self):
        return self.serial

    def __repr__(self):
        return self.label or "P(e%d s%d i%s %s)" % (self.epoch, self.seq, self.issued, fmt_ents(self.ents))


class Decl:
    """A declaration text root.p (1-3): epoch, revoked, issued, base. Interned by declaration();
    `variant` makes another text with the same members (what RootFork refuses)."""
    __slots__ = ("epoch", "revoked", "issued", "base", "variant", "label", "serial")

    def __hash__(self):
        return self.serial

    def __repr__(self):
        return self.label or "D%d(rev%s i%s base %s%s)" % (
            self.epoch, sorted(self.revoked), self.issued, fmt_ents(self.base),
            " " + str(self.variant) if self.variant else "")


_POLICIES: dict = {}
_DECLS: dict = {}


def canon(ents):
    return tuple((c, ents[c]) for c in NAMES if c in ents)


def policy(epoch, seq, issued, ents, auto=False, pkg=True, notes="", label=""):
    """The policy text with these members (interned). pkg: True (packages), False (withdrawn), 'S'
    (a thief's package); auto: windows.auto_install_after present (and due)."""
    key = (epoch, seq, issued, canon(ents), auto, pkg, notes)
    p = _POLICIES.get(key)
    if p is None:
        p = Policy()
        p.epoch, p.seq, p.issued, p.auto, p.pkg, p.notes = epoch, seq, issued, auto, pkg, notes
        p.ents = dict(canon(ents))
        p.label = label
        p.serial = 2 * len(_POLICIES) + 1
        _POLICIES[key] = p
    elif label and not p.label:
        p.label = label
    return p


def declaration(epoch, revoked, issued, base, variant="", label=""):
    """The declaration text with these members (interned)."""
    key = (epoch, frozenset(revoked), issued, canon(base), variant)
    d = _DECLS.get(key)
    if d is None:
        d = Decl()
        d.epoch, d.revoked, d.issued, d.variant = epoch, frozenset(revoked), issued, variant
        d.base = dict(canon(base))
        d.label = label
        d.serial = 2 * len(_DECLS) + 2
        _DECLS[key] = d
    elif label and not d.label:
        d.label = label
    return d


def requirements(ents):
    """The requirement part of entries (what a base holds): min, latest, grace_until, grace_min."""
    return {c: Entry(e.min, e.latest, e.gu, e.gm) for c, e in ents.items()}


def fmt_entry(e):
    s = "%d..%d" % (e.min, e.latest)
    if e.gu is not None:
        s += "(gm%d<t%d)" % (e.gm, e.gu)
    return s


def fmt_ents(ents):
    short = {"windows": "w", "android": "a", "android_play": "p", "ios": "i"}
    return "{" + " ".join("%s:%s" % (short[c], fmt_entry(e)) for c, e in ents.items()) + "}"


class Part:
    """A signed part {k, p, s}: the policy part or the root part. `valid`: the signature holds for
    key k (B2, C2). `form`: 'ok', or the code its text gets once its signature holds for what the
    model does not represent: policy 'Version', 'Field:json' (2-2's shape errors), 'Field:<member>'
    (a member the model leaves out), 'Server'; declaration 'Version', 'RootFormat', 'RootServer'.
    A part with a `form` other than 'ok' has a text that keeps every rule the model represents."""
    __slots__ = ("k", "text", "valid", "form", "_key", "_h")

    def __init__(self, k, text, valid=True, form="ok"):
        self.k, self.text, self.valid, self.form = k, text, valid, form
        self._key = (k, text, valid, form)
        self._h = hash((k, text, valid, form == "ok", len(form)))

    def __hash__(self):
        return self._h

    def __eq__(self, other):
        return isinstance(other, Part) and self._key == other._key

    def __repr__(self):
        return "%r/k%d%s%s" % (self.text, self.k, "" if self.valid else " (bad signature)",
                               "" if self.form == "ok" else " (" + self.form + ")")


class Env:
    """An envelope as served at /api/hjv/version: the policy part, the root part or None. `form`:
    'ok' or step A's code ('TooLarge', 'Envelope', 'Version')."""
    __slots__ = ("pol", "root", "form", "_key", "_h")

    def __init__(self, pol, root=None, form="ok"):
        self.pol, self.root, self.form = pol, root, form
        self._key = (pol, root, form)
        self._h = hash((pol, root, form == "ok", len(form)))

    def __hash__(self):
        return self._h

    def __eq__(self, other):
        return isinstance(other, Env) and self._key == other._key

    def __repr__(self):
        return "Env(%r, %r%s)" % (self.pol, self.root, "" if self.form == "ok" else ", " + self.form)


def decl_of(env):
    return env.root.text if env.root is not None else None


ENV_LABEL: dict = {}


def label_env(env, label):
    ENV_LABEL.setdefault(env, label)
    return env


def env_name(env):
    if env in ENV_LABEL:
        return ENV_LABEL[env]
    s = "%r" % env.pol
    if env.root is not None:
        s += " + %r" % env.root
    return s


# ================================================================================================
# 3. Builds, the device state, derived values                                    (spec 1-4, 1-5)
# ================================================================================================

ROOT1, ROOT2 = 101, 102
ROOTS = frozenset({ROOT1, ROOT2})       # 1.6.0 pins two root keys (1-4)


def is_root_number(k):
    """1-4: signing keys are numbered 1-100, root keys 101-199."""
    return 101 <= k <= 199


class Build:
    """What a build pins (1-4): signing keys (HJV_UPDATE_PUBKEY), root keys (HJV_UPDATE_ROOT_PUBKEY)
    and the floor (HJV_POLICY_FLOOR = (epoch, seq)). Its key set, the fingerprint of 1-4 with the
    server fixed, is (pins, roots)."""
    __slots__ = ("name", "pins", "roots", "floor", "keyset", "_key", "_h")

    def __init__(self, name, pins, roots, floor):
        self.name, self.pins, self.roots, self.floor = name, frozenset(pins), frozenset(roots), tuple(floor)
        self.keyset = (self.pins, self.roots)
        self._key = (name, self.pins, self.roots, self.floor)
        self._h = hash((self.floor, tuple(sorted(self.pins)), tuple(sorted(self.roots)),
                        sum(map(ord, name)), len(name)))

    def __hash__(self):
        return self._h

    def __eq__(self, other):
        return isinstance(other, Build) and self._key == other._key

    def __repr__(self):
        return "Build(%s pins %s roots %s floor %d:%d)" % (self.name, sorted(self.pins), sorted(self.roots),
                                                           self.floor[0], self.floor[1])


class Held(NamedTuple):
    """The held policy (1-5). Of its envelope as received the model keeps the policy text, the
    signer's key number and the root part inside that envelope (L3 and L4 read it). Its signature
    holds (a crafted record with a bad one is a record whose signer the build does not pin)."""
    pol: Policy
    k: int
    root: Optional[Part]


class State(NamedTuple):
    """One process's state (1-5): its build (the constants), the held declaration's root part as
    received (`root`; the declaration is root.text) and the held policy. checked_at and
    server_offset are left out: no rule here reads them."""
    build: Build
    root: Optional[Part]
    held: Optional[Held]

    @property
    def decl(self):
        return None if self.root is None else self.root.text


def fresh(build):
    """A fresh install: decl none, held none: E = floor.epoch, EFF none (1-5)."""
    return State(build, None, None)


class Eff(NamedTuple):
    """EFF (1-5): a policy, or a base read as a policy (only the four requirement members;
    epoch = decl.epoch, seq = 0, issued = decl.issued)."""
    ents: dict
    issued: int
    epoch: int
    seq: int
    source: str             # 'policy' or 'base'
    obj: object             # the Policy or the Decl


class Basis(NamedTuple):
    ents: dict
    issued: int


class Derived(NamedTuple):
    DE: int
    E: int
    REVOKED: frozenset
    CUR: Optional[Held]
    DCUR: Optional[Decl]
    LOW: int
    SAME: Optional[tuple]   # (seq, the policy text: `is` compares h)
    BASIS: Optional[Basis]
    EFF: Optional[Eff]
    MAY_INSTALL: bool


_DERIVED: dict = {}


def derived(st) -> Derived:
    d = _DERIVED.get(st)
    if d is None:
        d = _derive(st)
        if len(_DERIVED) > 1_500_000:
            _DERIVED.clear()
        _DERIVED[st] = d
    return d


def eff_of_base(D):
    return Eff(D.base, D.issued, D.epoch, 0, "base", D)


def _derive(st) -> Derived:
    """The table of 1-5, line by line."""
    fe, fs = st.build.floor
    h, D = st.held, st.decl
    DE = D.epoch if D else 1
    E = max(fe, DE)
    REVOKED = D.revoked if D else frozenset()
    CUR = h if h and h.pol.epoch == E else None
    DCUR = D if D and D.epoch == E else None
    if CUR and (E, CUR.pol.seq) >= (fe, fs):
        LOW, SAME = CUR.pol.seq, (CUR.pol.seq, CUR.pol)
    else:
        SAME = None
        LOW = fs - 1 if fe == E and fs >= 1 else 0
    if "basis_held" in V and h:                                   # broken variant
        BASIS = Basis(h.pol.ents, h.pol.issued)
    elif CUR:
        BASIS = Basis(CUR.pol.ents, CUR.pol.issued)
    elif DCUR:
        BASIS = Basis(DCUR.base, DCUR.issued)
    else:
        BASIS = None
    # EFF: held.P if held, held.P.epoch == DE and held.k not in REVOKED; else decl's base; else none
    if h and (h.pol.epoch == DE or "eff_old" in V) and (h.k not in REVOKED or "eff_revoked" in V):
        EFF = Eff(h.pol.ents, h.pol.issued, h.pol.epoch, h.pol.seq, "policy", h.pol)
    elif D:
        EFF = eff_of_base(D)
    else:
        EFF = None
    if "may_install_no_floor" in V:                               # broken variant
        MAY_INSTALL = bool(h) and h.pol.epoch == E and h.k not in REVOKED
    else:
        MAY_INSTALL = SAME is not None and h.k not in REVOKED
    return Derived(DE, E, REVOKED, CUR, DCUR, LOW, SAME, BASIS, EFF, MAY_INSTALL)


# ================================================================================================
# 4. accept(): taking or refusing an envelope                              (spec 2-1 .. 2-5)
# ================================================================================================

class Outcome(NamedTuple):
    kind: str                       # 'Taken', 'Unchanged', 'Refused'
    reason: Optional[str] = None    # the code of 2-4 when refused ('Field:<member>' for Field)

    def __repr__(self):
        return self.kind if self.reason is None else "%s(%s)" % (self.kind, self.reason)


TAKEN = Outcome("Taken")
UNCHANGED = Outcome("Unchanged")


def refused(reason):
    return Outcome("Refused", reason)


_TEXT_ERR: dict = {}


def decl_text_error(D) -> Optional[str]:
    """The rules of B3 the model keeps for a declaration text: epoch >= 2, every base entry keeps
    the rules of 1-3."""
    err = _TEXT_ERR.get(D, 0)
    if err == 0:
        err = "RootFormat" if D.epoch < 2 or any(entry_error(e) for e in D.base.values()) else None
        _TEXT_ERR[D] = err
    return err


def policy_text_error(P) -> Optional[str]:
    """The rules of C3 the model keeps for a policy text, in C3's order: epoch >= 1, seq >= 1, then
    the entries (windows, android, android_play, ios), each min, latest, num(min) <= num(latest),
    grace_min."""
    err = _TEXT_ERR.get(P, 0)
    if err == 0:
        err = None
        if P.epoch < 1:
            err = "Field:epoch"
        elif P.seq < 1:
            err = "Field:seq"
        else:
            for c in NAMES:
                if c in P.ents and entry_error(P.ents[c]):
                    err = "Field:" + entry_error(P.ents[c])
                    break
        _TEXT_ERR[P] = err
    return err


def root_part_error(roots, part) -> Optional[str]:
    """B3 for a root part whose key is pinned and whose signature holds: the JSON rules and v
    (`form`), the text rules (epoch >= 2, base), the server, no pinned root key revoked."""
    if part.form in ("Version", "RootFormat"):
        return part.form
    if decl_text_error(part.text):
        return "RootFormat"
    if part.form == "RootServer":
        return "RootServer"
    if roots & part.text.revoked:
        return "RootRevokesRoot"
    return None


def policy_part_error(part) -> Optional[str]:
    """C3 for a policy part whose signature holds: the JSON rules and v (`form`), then the members
    in today's order (epoch, seq, ..., the entries), then the server."""
    if part.form in ("Version", "Field:json"):
        return part.form
    err = policy_text_error(part.text)
    if err:
        return err
    return None if part.form == "ok" else part.form          # Field:<a member left out>, Server


def step_b(st, env):
    """Step B of 2-2. Returns (S1, None), or (st, code) when B1-B4 refuse the declaration (the state
    is then unchanged). `held` is never touched; B4c and B4d replace the declaration only (2-5)."""
    root = env.root
    if root is None:
        return st, None
    roots = st.build.roots
    if root.k not in roots:                               # B1: a pinned root key's number
        return st, "RootKey"
    if not root.valid:                                    # B2: strict Ed25519 with that key
        return st, "RootSignature"
    err = root_part_error(roots, root)                    # B3
    if err:
        return st, err
    D = root.text
    d = derived(st)
    if D.epoch < d.E:                                     # B4a
        return st, "RootEpoch"
    if D.epoch == d.E and d.DCUR is not None:             # B4b: the same text p (any signer), or a fork
        return (st, None) if D is d.DCUR else (st, "RootFork")
    if D.epoch == d.E and "b4c_off" in V:                 # broken variant: no B4c
        return st, "RootEpoch"
    return State(st.build, root, st.held), None           # B4c (same epoch, no DCUR), B4d (later)


def accept(st: State, env: Env):
    """accept(build, state, body) -> (state', outcome) of 2-1 and 2-2, with A, B, C in order.

    Up to and including B4 a refusal returns the state unchanged; after B4 it returns S1, the state
    with the declaration B4 took, if any. No text is read before its signature holds (here: `valid`
    is looked at before `form` and the text)."""
    if env.form != "ok":                                  # A1 TooLarge; A2 Envelope (UTF-8 first), Version
        return st, refused(env.form)
    S1, why = step_b(st, env)
    if why:
        return st, refused(why)
    out = st if "all_or_nothing" in V else S1             # broken variant: drop the declaration
    d = derived(S1)
    k = env.pol.k
    if k not in S1.build.roots and k not in S1.build.pins:   # C1
        return out, refused("UnknownKey")
    if k in d.REVOKED:                                    # C1
        return out, refused("RevokedKey")
    if not env.pol.valid:                                 # C2
        return out, refused("BadSignature")
    err = policy_part_error(env.pol)                      # C3
    if err:
        return out, refused(err)
    P = env.pol.text
    if P.epoch != d.E:                                    # C4: any signer's policy of another epoch
        return out, refused("Epoch")
    if P.epoch >= 2 and (env.root is None or env.root.text.epoch != P.epoch) and "c4_no_root" not in V:
        return out, refused("Epoch")                      # C4: an epoch 2+ policy travels with its declaration
    if d.SAME is not None and P.seq == d.SAME[0]:         # C5
        if P is d.SAME[1]:
            return S1, UNCHANGED                          # S1 is the incoming state here (2-5)
        return out, refused("Fork")
    if P.seq <= d.LOW:
        return out, refused("Rollback")
    if P.seq - d.LOW > STEP and not ("jump_root_exempt" in V and k in S1.build.roots):
        return out, refused("Jump")                       # every signer, the root keys included
    if (d.E, P.seq) < S1.build.floor:                     # C6, a guard
        return out, refused("BelowFloor")
    if d.BASIS is not None:
        if P.issued < d.BASIS.issued and "no_issued" not in V:   # C7
            return out, refused("Issued")
        if relaxes(d.BASIS.ents, P.ents):                 # C8
            return out, refused("Relaxation")
    return State(S1.build, S1.root, Held(P, k, env.root)), TAKEN   # C9


# ================================================================================================
# 5. The stored record: LOAD (part by part), SAVE, RANK                                (spec 1-6)
# ================================================================================================

class Record(NamedTuple):
    """HJViewer_update.json v 2 (1-6): `root` (the held declaration's root part as received) and
    `envelope` (the held envelope: policy text, signer, its own root part). No fingerprint;
    checked_at and server_offset are left out. A record that fails L1 is None (no file)."""
    root: Optional[Part]
    held: Optional[Held]


def record_of(st):
    return Record(st.root, st.held)


def root_part_holds(build, part):
    """B1-B3 for a root part under `build` (L2, L3): a pinned root key's number, its signature, the
    text rules, the server, no pinned root key revoked."""
    return part.k in build.roots and part.valid and root_part_error(build.roots, part) is None


def load(build, rec):
    """LOAD of 1-6 under `build`, part by part: each part that fails is dropped alone; nothing kept
    is a fresh start."""
    if rec is None:
        return fresh(build)                                                    # L1
    root = rec.root if rec.root is not None and root_part_holds(build, rec.root) else None   # L2
    h = rec.held
    if h is not None:
        ok = ((h.root is None or root_part_holds(build, h.root))              # L3: its root part,
              and (h.k in build.roots or h.k in build.pins)                   # its signer (not revoked: not checked)
              and policy_text_error(h.pol) is None)                           # and C3
        if ok:                                                                 # L4
            P = h.pol
            if P.epoch == 1:
                ok = h.root is None
            else:
                D = root.text if root is not None else None
                ok = (h.root is not None and h.root.text.epoch == P.epoch and D is not None
                      and D.epoch >= P.epoch and (D.epoch > P.epoch or D is h.root.text))
        if ok and "load_drops_held" in V and (h.pol.epoch, h.pol.seq) < build.floor:   # broken variant
            ok = False
        if not ok:
            h = None
    if "load_whole" in V and ((rec.root is not None and root is None) or (rec.held is not None and h is None)):
        return fresh(build)                                                    # broken variant: round 1's LOAD
    if root is None and h is None:
        return fresh(build)                                                    # L5
    return State(build, root, h)


def rank(x):
    """RANK of 1-6 for a state or a record: (e, s, x) with e = DE, s = held.P.seq if the held policy
    is of epoch e (else 0), x = 1 if a policy is held."""
    r = x.root
    e = r.text.epoch if r is not None else 1
    H = x.held
    s = H.pol.seq if H and H.pol.epoch == e else 0
    return (e, s, 1 if H else 0)


def save(file_rec, st):
    """SAVE of 1-6: what the file holds afterwards (the process's memory never changes). The file is
    read by LOAD under the process's build, whoever wrote it; a state of greater RANK there stays."""
    if file_rec is not None and "save_no_rank" not in V:
        if rank(load(st.build, file_rec)) > rank(st):
            return file_rec
    return record_of(st)


# ================================================================================================
# 6. Verdicts                                                                         (spec 3, 4-7)
# ================================================================================================

def policy_now(st, clock):
    """3-1: now = max(device clock, EFF.issued) (the pull-back to the server's Date left out)."""
    eff = derived(st).EFF
    return clock if eff is None else max(clock, eff.issued)


def app_state_of(eff, entry_name, own, now):
    """3-2: unknown, ok, available, required or grace for the app's own entry."""
    if eff is None or entry_name not in eff.ents:
        return "unknown"
    e = eff.ents[entry_name]
    if own >= e.latest:
        return "ok"
    if own >= e.min:
        return "available"
    if own < threshold(e, now):
        return "required"
    return "grace"


def app_state(st, entry_name, own, clock):
    return app_state_of(derived(st).EFF, entry_name, own, policy_now(st, clock))


class ControllerRule(NamedTuple):
    min_text: int       # the lowest min among the family's entries (the target L1/L2 name)
    min_number: int     # LIMIT(now)
    enforced: bool      # LIMIT(now) > 0


def controller_rule_of(eff, family, now):
    """3-3: None when EFF is none or has no entry of the family (not refused)."""
    if eff is None:
        return None
    es = family_entries(eff.ents, family)
    if not es:
        return None
    lim = min(threshold(e, now) for e in es)
    return ControllerRule(min(e.min for e in es), lim, lim > 0)


def controller_refused(st, family, version, clock):
    r = controller_rule_of(derived(st).EFF, family, policy_now(st, clock))
    return r is not None and version < r.min_number


def install_target(st, entry_name, own):
    """3-4: nothing unless MAY_INSTALL; then the held policy's own entry: its latest, if above the
    own version and the policy carries the package."""
    if not derived(st).MAY_INSTALL:
        return None
    e = st.held.pol.ents.get(entry_name)
    if e is None or not st.held.pol.pkg or e.latest <= own:
        return None
    return e.latest


def forced_anchor(st, entry_name, own):
    """3-4: a forced install when own < min: anchor = grace_until if the entry has the pair and
    own >= grace_min, else the held policy's issued (the PC is blocked already)."""
    if install_target(st, entry_name, own) is None:
        return None
    e = st.held.pol.ents[entry_name]
    if own >= e.min:
        return None
    if e.gu is not None and own >= e.gm:
        return e.gu
    return st.held.pol.issued


def screen_package(st, entry_name):
    """3-6: the screens' `package` is null unless MAY_INSTALL and the entry has one."""
    d = derived(st)
    return bool(d.MAY_INSTALL and st.held.pol.pkg and entry_name in st.held.pol.ents)


NEVER = -(10 ** 9)


def hbbs_conf(min_n, from_basis, current, now):
    """`hbbs-conf` (3-5, 4-7) for a policy whose MIN (the lowest min of all entries) is min_n and
    whose derived FROM is from_basis, given the present file `current` = (MIN, FROM or None) or None
    (`--current none`): ('keep',) when MIN is unchanged, ('later', FROM) for "keep the present file;
    run hbbs-conf again at or after FROM", or ('write', MIN, FROM). FROM never goes back; a lower
    MIN is always pointed out (written, with a warning)."""
    if current is None or current[0] is None:
        return ("write", min_n, from_basis)               # 5-2: no present file: at once
    cur_min, cur_from = current
    if cur_min == min_n:
        return ("keep",)
    frm = from_basis if cur_from is None else max(from_basis, cur_from)     # FROM never goes back
    if min_n > cur_min:
        cf = NEVER if cur_from is None else cur_from      # an empty FROM counts as long past
        if frm > max(now, cf) and "hbbs_no_wait" not in V:
            return ("later", frm)
    return ("write", min_n, frm)


def hbbs_refuses(conf, t, v):
    """hbbs with (MIN, FROM): refuses a version below MIN from FROM on (an empty FROM: always),
    nobody before; no file refuses nobody."""
    if conf is None or conf[0] is None:
        return False
    mn, frm = conf
    return v < mn and (frm is None or t >= frm)


# ================================================================================================
# 7. The tool (tools/hjv-release)                                                        (spec 4)
# ================================================================================================

GRACE = 7          # the tool's default grace ("00:00 KST + 7 days") in model time
WAIT = 5           # one step of the operator's clock
VTOP = 3           # the highest version the operator releases in the search


class ToolRefusal(Exception):
    def __init__(self, code, why):
        super().__init__("%s: %s" % (code, why))
        self.code, self.why = code, why


def derive_existing(o, B, now, grace, relax=False, grace_min=None):
    """4-2 a-d for an entry `o` of --prev and the asked minimum B. `grace`: 'def' (no option),
    'none' (--grace-days 0) or an asked grace_until (a time). `grace_min`: --grace-min (a with
    --relax). latest is the caller's."""
    if B == o.min:                                                     # a
        if grace == "none":
            return Entry(o.min, o.latest)                              # the pair removed
        if grace == "def" or (o.gu is not None and grace == o.gu):
            return o                                                   # min and the pair stay
        if o.gu is not None and now < grace < o.gu:
            return Entry(o.min, o.latest, grace, o.gm)                 # brought forward (yes)
        if not relax:
            raise ToolRefusal("X1", "a deadline put off or given")
        gm = grace_min if grace_min is not None else (o.gm if o.gu is not None else None)
        if gm is None:
            raise ToolRefusal("option", "--relax with a deadline where there was none needs --grace-min")
        return Entry(o.min, o.latest, grace, gm)
    if B > o.min:
        if o.gu is None or o.gu <= now:                                # b: no raise pending
            if grace == "none":
                return Entry(B, o.latest)
            T = now + GRACE if grace == "def" else grace
            return Entry(B, o.latest, T, o.min)
        if grace == "none":                                            # c: a raise is pending
            return Entry(B, o.latest)
        T = o.gu if grace == "def" else grace
        if T > o.gu:
            return Entry(B, o.latest, T, o.min)                        # c, later: o.min at once (yes)
        return Entry(B, o.latest, T, o.gm)                             # c, same or earlier
    if not relax:                                                      # d
        raise ToolRefusal("X1", "minimum lowered")
    return Entry(B, o.latest)


def derive_new(name, B, latest, now, grace, prev_ents, first=False, relax=False):
    """4-2 e-f for a new entry."""
    if name in ANDROID and sibling(name) in prev_ents:                 # f
        x = prev_ents[sibling(name)]
        if B == x.min:
            return Entry(B, latest, x.gu, x.gm)
        if B > x.min:
            e = derive_existing(x, B, now, grace, relax)
            return Entry(e.min, latest, e.gu, e.gm)
        if not relax:
            raise ToolRefusal("X1", "an Android entry that asks less than its sibling")
        return Entry(B, latest)
    if B == 0 or grace == "none" or (first and grace == "def"):        # e (no pair)
        return Entry(B, latest)
    T = now + GRACE if grace == "def" else grace
    return Entry(B, latest, T, 0)                                      # e: grace_min "0.0.0"


class OpState(NamedTuple):
    """The operator's side. `prev`: the archive's highest signed envelope that is not voided
    (--prev, X11); `live`: the server's file; `published`, `leaked` (signed, not published, N has
    it), `voided`: the archive's files; `archive_max`: the highest declaration epoch the archive
    holds; `keys`/`roots`: the key file's signing and root keys in the order the tool picks them;
    `builds`: builds made (the last one's key file is the tool's); `recorded`: the key sets the
    archive records (4-2); `server_kb`: the server's key file; `stolen`: what the thief holds;
    `lost_roots`: root keys whose private half or passphrase is lost (5-10); `phase` of an incident;
    `ops`: the history; `root_sigs`: root signatures made; `destroyed`: retired signing keys whose
    private half was destroyed (5-9)."""
    now: int
    prev: Env
    live: Env
    archive_max: int
    keys: tuple
    roots: tuple
    builds: tuple
    server_kb: Build
    published: tuple
    leaked: tuple
    voided: tuple
    recorded: frozenset
    stolen: frozenset
    lost_roots: frozenset
    phase: str          # 'normal', 'stolen', 'recovered'
    ops: tuple
    root_sigs: int
    destroyed: frozenset


def keyfile_build(o):
    """The build whose update-keys.env the tool reads: the newest build."""
    return o.builds[-1]


def archived(o, env):
    return env in o.published or env in o.leaked


def serving_keys(o, revoked):
    """4-2 Signer: the key file's signing keys not revoked by the (new or carried) declaration that
    serve the records (every recorded key set pins them)."""
    if "x12_off" in V:
        return [k for k in o.keys if k not in revoked]
    return [k for k in o.keys if k not in revoked and all(k in pins for pins, _ in o.recorded)]


def serving_roots(o):
    """The key file's root keys that serve the records."""
    if "x12_off" in V:
        return list(o.roots)
    return [r for r in o.roots if all(r in roots for _, roots in o.recorded)]


def holding(build, env):
    """The state LOAD gives under `build` for a record holding `env` and its root part (X4 a,
    `floor --live`, 4-8 item 4)."""
    return load(build, Record(env.root, Held(env.pol.text, env.pol.k, env.root)))


def x4(env, prev_env, kbuild):
    """X4 (4-3): refuse an envelope that is not Taken by (a) a device holding --prev, (b) a fresh
    install at the key file's floor; or, for epoch 2 and later, a policy that relaxes its
    declaration's base."""
    if prev_env is not None:
        _, out = accept(holding(kbuild, prev_env), env)
        if out.kind != "Taken":
            return "X4a:%s" % out.reason
    _, out = accept(fresh(kbuild), env)
    if out.kind != "Taken":
        return "X4b:%s" % out.reason
    P = env.pol.text
    if P.epoch >= 2 and relaxes(env.root.text.base, P.ents):
        return "X4c:Relaxation"
    return None


def server_check(env, live_env, kbuild, declared=frozenset()):
    """The server-side check of 4-8, items 3, 4, 5 and 7 (1, 2, 6 and 8 are about bytes, signatures,
    clocks and packages). `kbuild` holds the server's key file; `declared` is the operator's
    --relaxes."""
    _, out = accept(fresh(kbuild), env)                                # 3
    if out.kind != "Taken":
        return "S3:%s" % out.reason
    if live_env is None:
        return None
    lst = holding(kbuild, live_env)                                    # 4
    if lst.held is None:
        return "S4:LiveUnreadable"
    _, out = accept(lst, env)
    if out.kind not in ("Taken", "Unchanged"):
        return "S4:%s" % out.reason
    lr, nr = decl_of(live_env), decl_of(env)
    if nr is not None and nr is not lr:                                # 5, 7
        lP, nP = live_env.pol.text, env.pol.text
        if nP.issued < lP.issued:
            return "S5:LiveIssued"
        if relaxed(lP.ents, nP.ents) != frozenset(declared):
            return "S5:LiveRelaxes"
        if nr.epoch <= lP.epoch:
            return "S7:DeclarationEpoch"
        if (lr is not None and not lr.revoked <= nr.revoked) or any(is_root_number(k) for k in nr.revoked):
            return "S7:DeclarationRevoked"
    return None


def server_build(o, live):
    """The server's key file (4-8): it follows the newest build once the live envelope LOADs under
    it with its policy part (the live file is signed by keys it pins); until then the older one."""
    kb = keyfile_build(o)
    return kb if holding(kb, live).held is not None else o.server_kb


def floor_live(o, build):
    """`floor --live F` (4-7, 5-15) for a build to be made with key file `build`: F, the live
    envelope as fetched, must be byte for byte a file of the archive and verify with the key file
    (LOAD keeps its policy part); X6: the key file pins no key F's declaration revokes. The floor is
    F's position; the caller records the build's key set."""
    F = o.live
    if not archived(o, F):
        raise ToolRefusal("floor", "the live file is not a file of the archive")
    if "floor_unverified" not in V and holding(build, F).held is None:
        raise ToolRefusal("floor", "the live file does not verify with the key file (signer %d, root part %s)"
                          % (F.pol.k, "none" if F.root is None else "by %d" % F.root.k))
    D = decl_of(F)
    if D is not None and D.revoked & build.pins:
        raise ToolRefusal("X6", "the key file pins a key the live declaration revokes")
    return (F.pol.text.epoch, F.pol.text.seq)


def tool_make(o, ents, *, new_decl=False, revoke=frozenset(), auto=None, pkg=None, label="",
              declared=frozenset(), live=None, x10=True, root_signs=False, prev=None, droot=None):
    """`policy` + `sign` (4-2, 4-3, 4-6) for the derived entries `ents`: position (above every file
    of the epoch the archive holds, voided ones included), declaration (carried with the same bytes,
    or a new one with its base from the live file), signer (X9, X12 by the key set records), X1,
    X2, X4, X7, X8, X10, X11, then the server check (4-8). Returns the signed envelope or raises
    ToolRefusal."""
    T = ToolRefusal
    live = o.live if live is None else live
    if prev is None:
        prev = live if new_decl else o.prev       # 4-2: a declaration only from the live file (X10)
    prevP, prevD = prev.pol.text, decl_of(prev)
    for c, e in ents.items():                                          # X7: the rules of 1-3
        m = entry_error(e)
        if m:
            raise T("X7", "%s: Field(%s)" % (c, m))
    if not new_decl:
        if prev is not o.prev:                                         # X11
            raise T("X11", "--prev is not the archive's highest position that is not voided")
        if relaxes(prevP.ents, ents):                                  # X1
            raise T("X1", "asks less than --prev (%s) without a new declaration"
                    % ",".join(sorted(relaxed(prevP.ents, ents))))
        lowest = min(e.min for e in prevP.ents.values())
        if any(c not in prevP.ents and e.min < lowest for c, e in ents.items()):
            raise T("X8", "a new entry below the lowest min (tool only)")
        epoch, root, D = prevP.epoch, prev.root, prevD
        seq = 1 + max(x.pol.text.seq for x in o.published + o.leaked + o.voided
                      if x.pol.text.epoch == epoch)                    # 4-2 Position, X11
    else:
        if x10 and live is not prev:                                   # X10
            raise T("X10", "the live envelope is not --prev")
        if x10 and not archived(o, prev):
            raise T("X10", "--prev is not a file of the archive")
        epoch, seq = 1 + max(prevP.epoch, o.archive_max), 1
        revoked = (prevD.revoked if prevD else frozenset()) | frozenset(revoke)
        if any(not 1 <= k <= 100 for k in revoke):                     # X2
            raise T("X2", "--revoke-key outside 1-100")
        D = declaration(epoch, revoked, o.now, requirements(ents), label="D%d" % epoch)
        if prevD is not None and not prevD.revoked <= D.revoked:
            raise T("X2", "a declaration that forgets a revoked key")
        if any(is_root_number(k) for k in D.revoked):
            raise T("X2", "a declaration that revokes a number 101-199")
        holdable = [r for r in serving_roots(o) if r not in o.lost_roots]
        rk = droot if droot is not None else (holdable[0] if holdable else None)
        if rk is None or rk not in serving_roots(o):                   # X12
            raise T("X12", "the declaration's root key %s does not serve every recorded key set" % rk)
        if rk in o.lost_roots:
            raise T("lost", "root key %d is lost" % rk)
        root = Part(rk, D)
    if o.now < prevP.issued:                                           # X7
        raise T("X7", "issued before the previous policy's")
    revoked_now = D.revoked if D else frozenset()
    serving = serving_keys(o, revoked_now)
    if root_signs:                                                     # X9
        if serving:
            raise T("X9", "--root-signs-policy while key %d serves every recorded key set" % serving[0])
        rs = [r for r in serving_roots(o) if r not in o.lost_roots]
        if not rs:
            raise T("X12", "no root key the operator holds serves every recorded key set")
        signer = rs[0]
    else:
        usable = [k for k in serving if k not in o.destroyed]
        if not usable:                                                 # X12
            raise T("X12", "no signing key the operator holds serves every recorded key set")
        signer = usable[0]
    if pkg is None:                         # 4-2 Packages: a new windows/android latest brings its package
        pkg = True if any(c in ("windows", "android") and (c not in prevP.ents or e.latest != prevP.ents[c].latest)
                          for c, e in ents.items()) else prevP.pkg
    P = policy(epoch, seq, o.now, ents, auto=prevP.auto if auto is None else auto, pkg=pkg,
               label="e%d-s%d" % (epoch, seq))
    env = Env(Part(signer, P), root)
    why = x4(env, prev, keyfile_build(o))                              # X4 (sign, on the signed bytes)
    if why:
        raise T("X4", why)
    why = server_check(env, live, server_build(o, live), declared)     # 4-8
    if why:
        raise T("SERVER", why)
    return label_env(env, label or "e%d-s%d" % (epoch, seq))


# ================================================================================================
# 8. The world: the operator (runbook 5 and named mistakes), attackers N, S, O
# ================================================================================================

B0 = Build("b0", frozenset({1, 2}), ROOTS, (1, 0))      # 1.6.0: signing keys 1, 2; roots 101, 102; floor 1:0


def start(with_play=False):
    """5-1: the first policy (seq 1, signing key 1, no grace pair): every entry min 1, latest 2. The
    first `sign` starts the archive and records 1.6.0's key set."""
    ents = {"windows": Entry(1, 2), "android": Entry(1, 2)}
    if with_play:
        ents["android_play"] = Entry(1, 2)
    env = label_env(Env(Part(1, policy(1, 1, 0, ents, auto=True, label="e1-s1"))), "e1-s1")
    return OpState(now=0, prev=env, live=env, archive_max=1, keys=(1, 2), roots=(ROOT1, ROOT2),
                   builds=(B0,), server_kb=B0, published=(env,), leaked=(), voided=(),
                   recorded=frozenset({B0.keyset}), stolen=frozenset(), lost_roots=frozenset(),
                   phase="normal", ops=("e1-s1",), root_sigs=0, destroyed=frozenset())


def publish(o, env, op):
    """The operator publishes `env` (it passed `sign` and the server check); `sign` recorded the
    key file's key set."""
    new_root = env.root is not None and (o.live.root is None or env.root.text is not o.live.root.text)
    o2 = o._replace(prev=env, live=env, published=o.published + (env,),
                    archive_max=max(o.archive_max, env.root.text.epoch if env.root else 1),
                    recorded=o.recorded | {keyfile_build(o).keyset},
                    ops=o.ops + ("%s:%s" % (op, ENV_LABEL.get(env, "?")),),
                    root_sigs=o.root_sigs + int(new_root) + int(is_root_number(env.pol.k)))
    return o2._replace(server_kb=server_build(o2, env))


def entries_of(o, live=False):
    """The entries of --prev: the archive's highest file, or the live file for a declaration."""
    return dict((o.live if live else o.prev).pol.text.ents)


def raised_entries(o, names="all", grace="def"):
    ents = entries_of(o)
    changed = False
    for c in list(ents):
        if names != "all" and c not in names:
            continue
        e = ents[c]
        if e.min + 1 > VTOP:
            continue
        g = grace if grace in ("def", "none") else o.now + grace
        n = derive_existing(e, e.min + 1, o.now, g)
        ents[c] = Entry(n.min, max(e.latest, n.min), n.gu, n.gm)
        changed = True
    return ents if changed else None


def op_raise(names, grace):
    """5-2/5-3: raise the minimum of the entries by one, with the default grace, none
    (--grace-days 0) or an asked deadline `now + grace` (earlier or later than the default)."""
    def run(o):
        ents = raised_entries(o, names, grace)
        return ("publish", tool_make(o, ents)) if ents is not None else None
    return run


def op_latest(o):
    """5-4: a new latest (every entry's latest + 1)."""
    ents = entries_of(o)
    if all(e.latest >= VTOP for e in ents.values()):
        return None
    return ("publish", tool_make(o, {c: e._replace(latest=min(VTOP, e.latest + 1)) for c, e in ents.items()}))


def op_add_play(offset):
    """5-5: add android_play at android's level (offset 0, the tool copies the pair) or above."""
    def run(o):
        ents = entries_of(o)
        if "android_play" in ents or "android" not in ents:
            return None
        a = ents["android"]
        B = a.min + offset
        if B > VTOP:
            return None
        ents["android_play"] = derive_new("android_play", B, max(a.latest, B), o.now, "def", ents)
        return ("publish", tool_make(o, ents))
    return run


def op_add_ios(o):
    """5-5: a new ios entry at the others' level gets its own grace period (derivation e)."""
    ents = entries_of(o)
    if "ios" in ents:
        return None
    lvl = max(e.min for e in ents.values())
    ents["ios"] = derive_new("ios", lvl, max(lvl, max(e.latest for e in ents.values())), o.now, "def", ents)
    return ("publish", tool_make(o, ents))


def op_stop(o):
    """5-6: stop unattended installs and withdraw the packages (no relaxation)."""
    P = o.prev.pol.text
    if not P.auto and not P.pkg:
        return None
    return ("publish", tool_make(o, entries_of(o), auto=False, pkg=False))


def op_relax(kind):
    """5-7, 5-12: a declaration that relaxes ('min' every min down by one; 'late' every running
    deadline put off, a deadline given where there was none with --grace-min = min - 1; 'drop'
    android_play or windows dropped; 'latest' every latest down by one), installed with --relaxes
    RELAXED(live, new)."""
    def run(o):
        ents = entries_of(o, live=True)
        if kind == "min":
            if all(e.min == 0 for e in ents.values()):
                return None
            ents = {c: derive_existing(e, e.min - 1, o.now, "def", relax=True) if e.min else e
                    for c, e in ents.items()}
        elif kind == "late":
            new = {}
            for c, e in ents.items():
                if e.gu is not None and e.gu > o.now:
                    new[c] = derive_existing(e, e.min, o.now, e.gu + 2 * GRACE, relax=True)
                elif e.gu is None and e.min > 0:
                    new[c] = derive_existing(e, e.min, o.now, o.now + GRACE, relax=True, grace_min=e.min - 1)
                else:
                    new[c] = e
            if new == ents:
                return None
            ents = new
        elif kind == "latest":
            new = {c: e._replace(latest=e.latest - 1) if e.latest > e.min else e for c, e in ents.items()}
            if new == ents:
                return None
            ents = new
        elif kind == "drop":
            victim = "android_play" if "android_play" in ents else "windows"
            if victim not in ents or len(ents) == 1:
                return None
            del ents[victim]
        return ("publish", tool_make(o, ents, new_decl=True, declared=relaxed(o.live.pol.text.ents, ents)))
    return run


def op_new_epoch(droot=None):
    """5-14 (the rehearsal) or a re-base: a new declaration and no other change, signed by the first
    root key that serves the records and is not lost, or by `droot`."""
    def run(o):
        return ("publish", tool_make(o, entries_of(o, live=True), new_decl=True, droot=droot))
    return run


def op_rotate(o):
    """5-9 step 1: sign with the next key (2) from now on; every build in use pins it."""
    if o.keys[0] != 1 or 2 not in keyfile_build(o).pins:
        return None
    return ("state", o._replace(keys=(2, 1), ops=o.ops + ("rotate: sign with key 2",)))


def op_destroy_retired(o):
    """5-9 step 1: destroy the retired key's private half (key 1 once key 2 signs)."""
    if o.keys[0] != 2 or 1 in o.destroyed or 1 in o.stolen:
        return None
    return ("state", o._replace(destroyed=o.destroyed | {1}, ops=o.ops + ("destroy key 1",)))


def op_revoke_retired(o):
    """5-9 step 5: revoke the retired key 1 in a declaration (a re-base) once key 2 signs."""
    D = decl_of(o.live)
    if o.keys[0] != 2 or (D and 1 in D.revoked):
        return None
    return ("publish", tool_make(o, entries_of(o, live=True), new_decl=True, revoke={1}))


def op_wait(o):
    return ("state", o._replace(now=o.now + WAIT, ops=o.ops + ("wait",)))


def made(o, b, what):
    """A build made with `floor --live` (which records its key set)."""
    return ("state", o._replace(builds=o.builds + (b,), recorded=o.recorded | {b.keyset},
                                ops=o.ops + ("%s %s" % (what, b.name),)))


def op_build(o):
    """5-15: a new build of the same key set; its floor is the live position (`floor --live`)."""
    kb = keyfile_build(o)
    P = o.live.pol.text
    floor = (P.epoch, P.seq)
    if any(b.keyset == kb.keyset and b.floor == floor for b in o.builds):
        return None
    b = Build("b%d:%d" % floor, kb.pins, kb.roots, floor)
    floor_live(o, b)
    return made(o, b, "build")


def op_build_ahead(o):
    """A mistake 5-15 warns about: a build whose floor is the position of a policy that is not live
    yet (the live one + 1), written by hand."""
    kb = keyfile_build(o)
    P = o.live.pol.text
    floor = (P.epoch, P.seq + 1)
    if any(b.floor == floor for b in o.builds):
        return None
    b = Build("ahead%d:%d" % floor, kb.pins, kb.roots, floor)
    return ("state", o._replace(builds=o.builds + (b,), recorded=o.recorded | {b.keyset},
                                ops=o.ops + ("build %s (floor ahead of live, by hand)" % b.name,)))


def op_build_thief_floor(o):
    """While S's file is live on the server, the operator runs `floor --live` for a build: since
    round 2 it takes only a file of the archive, so S's file is refused."""
    if o.phase != "stolen":
        return None
    raise ToolRefusal("floor", "floor --live: the live file is not a file of the archive")


def op_build_newkeys(o):
    """5-8 step 7: after a revocation, a build with the unrevoked keys and a new key 3 (another key
    set; X6 keeps revoked keys out); the key file follows it; `floor --live`."""
    D = decl_of(o.live)
    kb = keyfile_build(o)
    if D is None or not (D.revoked & kb.pins) or 3 in kb.pins:
        return None
    keep = tuple(k for k in o.keys if k not in D.revoked and k not in o.destroyed)
    pins = frozenset(keep) | {3}
    P = o.live.pol.text
    b = Build("n%d:%d" % (P.epoch, P.seq), pins, kb.roots, (P.epoch, P.seq))
    floor_live(o, b)
    r = made(o, b, "build-newkeys pins %s" % sorted(pins))
    return ("state", r[1]._replace(keys=keep + (3,)))


def op_build_next(o):
    """5-9 step 2: once the live file is signed by key 2, the following build pins (2, 3) (another
    key set); the key file follows it; `floor --live`."""
    kb = keyfile_build(o)
    if o.keys[0] != 2 or o.live.pol.k != 2 or 3 in kb.pins or (decl_of(o.live) and 2 in decl_of(o.live).revoked):
        return None
    P = o.live.pol.text
    b = Build("x%d:%d" % (P.epoch, P.seq), frozenset({2, 3}), kb.roots, (P.epoch, P.seq))
    floor_live(o, b)
    r = made(o, b, "build-next pins [2, 3]")
    return ("state", r[1]._replace(keys=(2, 3)))


def op_lose_root(k):
    """5-10: root key k's medium or passphrase is lost (one root key at most in these searches; both
    lost is R16's manual cut-over)."""
    def run(o):
        if o.lost_roots or k not in o.roots:
            return None
        return ("state", o._replace(lost_roots=frozenset({k}), ops=o.ops + ("root key %d lost" % k,)))
    return run


def op_build_newroot(o):
    """5-10 as written: "The next build pins (the remaining one, a new one), for example (102, 103)";
    its floor by `floor --live` (5-15). Signing keys the live declaration revokes are left out and
    key 3 added (5-8 step 7), so that the step also follows a recovery."""
    kb = keyfile_build(o)
    if not o.lost_roots or 103 in kb.roots:
        return None
    remaining = [r for r in kb.roots if r not in o.lost_roots]
    D = decl_of(o.live)
    pins, keys = kb.pins, o.keys
    if D is not None and D.revoked & pins:
        pins = frozenset(k for k in pins if k not in D.revoked) | {3}
        keys = tuple(k for k in o.keys if k in pins and k not in o.destroyed) + ((3,) if 3 not in o.keys else ())
    P = o.live.pol.text
    b = Build("r%d:%d" % (P.epoch, P.seq), pins, frozenset(remaining) | {103}, (P.epoch, P.seq))
    floor_live(o, b)
    r = made(o, b, "build-newroot pins %s roots %s" % (sorted(b.pins), sorted(b.roots)))
    return ("state", r[1]._replace(roots=tuple(remaining) + (103,), keys=keys))


def op_decl_new_root(o):
    """A mistake 5-10 warns about: a re-base declaration signed with the new root key 103 before the
    older build's key set record is deleted (X12 refuses it)."""
    if 103 not in o.roots:
        return None
    return ("publish", tool_make(o, entries_of(o, live=True), new_decl=True, droot=103))


def op_revoke_lost_root(o):
    """A mistake X2 refuses: a declaration that lists the lost root key's number as revoked."""
    if not o.lost_roots:
        return None
    return ("publish", tool_make(o, entries_of(o, live=True), new_decl=True, revoke=set(o.lost_roots)))


def op_leak(o):
    """A mistake (5-13, R10): a declaration (a re-base) and its policy signed and not published; N
    has the file. The archive records the epoch."""
    try:
        env = tool_make(o, entries_of(o, live=True), new_decl=True)
    except ToolRefusal:
        return None
    D = env.root.text
    return ("state", o._replace(leaked=o.leaked + (env,), archive_max=max(o.archive_max, D.epoch),
                                root_sigs=o.root_sigs + 1,
                                ops=o.ops + ("leak e%d-s1+D%d" % (D.epoch, D.epoch),)))


def op_reuse_epoch(o):
    """A mistake the archive prevents (5-13): after a leaked declaration of epoch e the archive is
    lost and the next declaration is signed with epoch e again (another text)."""
    if not o.leaked:
        return None
    e = max(env.root.text.epoch for env in o.leaked if env.root is not None)
    if any(env.root is not None and env.root.text.epoch == e for env in o.published):
        return None
    try:
        env = tool_make(o._replace(archive_max=e - 1), entries_of(o, live=True), new_decl=True)
    except ToolRefusal:
        return None
    return ("publish", env)


def op_stale_decl(o):
    """A mistake (round 1, problem 6): a re-base declaration made from an older own policy than the
    live one, X10 bypassed, to see that the server check's item 5 alone stops every such declaration
    that would relax what devices holding the live file enforce. Never publishes."""
    live = o.live
    for old in o.published[:-1]:
        if old is live or old.pol.text.epoch != live.pol.text.epoch:
            continue
        try:
            env = tool_make(o, dict(old.pol.text.ents), new_decl=True, live=live, x10=False, prev=old)
        except ToolRefusal as e:
            STATS["stale --prev declaration: refused by %s" % e.why.split(":")[0]] += 1
            continue
        rel = relaxed(live.pol.text.ents, env.pol.text.ents)
        STATS["stale --prev declaration: PASSED%s" % (" although it relaxes" if rel else ", relaxes nothing")] += 1
    return None


def op_leak_policy(o):
    """A mistake (5-13): a raise signed and not published; N has the file; a signed file counts as
    published, so the next policy is made from it (X11)."""
    ents = raised_entries(o)
    if ents is None:
        return None
    env = tool_make(o, ents)
    return ("state", o._replace(prev=env, leaked=o.leaked + (env,),
                                ops=o.ops + ("leak-policy %r" % env.pol.text,)))


def op_void(o):
    """5-13 (round 2): `void --live L FILE` for a signed policy that was not published (N got the
    file all the same): FILE is the archive's highest not voided, opens no epoch; the next policy is
    made from the archive's highest file that is not voided, at a seq above FILE's."""
    P = o.prev
    if P is o.live or P not in o.leaked or P in o.published:
        return None
    if P.root is not None and (o.live.root is None or P.root.text is not o.live.root.text):
        return None
    voided = o.voided + (P,)
    rest = [e for e in o.published + o.leaked if e not in voided]
    prev = max(rest, key=lambda e: (e.pol.text.epoch, e.pol.text.seq))     # X11: the highest not voided
    return ("state", o._replace(prev=prev, voided=voided, ops=o.ops + ("void %r" % P.pol.text,)))


def op_theft(which):
    """S steals a signing key: 'cur' the key in use, 'other' the other one, 'both' both."""
    def run(o):
        if o.stolen:
            return None
        D = decl_of(o.live)
        live = [k for k in o.keys if not (D and k in D.revoked) and k not in o.destroyed]
        if which == "cur":
            ks = live[:1]
        elif which == "other":
            ks = live[1:2]
        else:
            ks = live[:2] if len(live) >= 2 else []
        if not ks:
            return None
        return ("state", o._replace(stolen=frozenset(ks), phase="stolen",
                                    ops=o.ops + ("theft of key %s" % ",".join(map(str, ks)),)))
    return run


def op_recover(o):
    """5-8: --prev <the last own file> --live <fetched> --revoke-key <stolen> [--revoke-key <each
    retired key not revoked yet>]; the policy is signed by a signing key that serves every recorded
    key set, else by a root key (--root-signs-policy); the declaration by a root key that serves."""
    if o.phase != "stolen":
        return None
    D = decl_of(o.live)
    old_rev = D.revoked if D else frozenset()
    revoke = (set(o.stolen) | set(o.destroyed)) - set(old_rev)      # the stolen and every retired key
    new_rev = frozenset(revoke) | old_rev
    serving = [k for k in serving_keys(o, new_rev) if k not in o.destroyed]
    try:
        env = tool_make(o, entries_of(o, live=True), new_decl=True, revoke=revoke, root_signs=not serving)
    except ToolRefusal as e:
        if e.code in ("X9", "X12"):
            raise ToolRefusal("signer", "%s %s" % (e.code, e.why))     # the checker reports it (G4)
        raise
    o2 = publish(o, env, "recover(revoke %s%s)" % (",".join(map(str, sorted(revoke))),
                                                   "" if serving else ", root-signed"))
    return ("state", o2._replace(phase="recovered",
                                 keys=tuple(x for x in o.keys if x not in o.stolen) + tuple(sorted(o.stolen))))


def op_root_policy(o):
    """5-8 step 6: while no signing key serves every recorded build, the next policy (a new latest)
    is signed by a root key (--root-signs-policy; X9 refuses it otherwise)."""
    if o.phase != "recovered":
        return None
    ents = entries_of(o)
    new = {c: e._replace(latest=min(VTOP, e.latest + 1)) for c, e in ents.items()}
    if new == ents:
        return None
    return ("publish", tool_make(o, new, root_signs=True))


def op_root_raise(o):
    """5-8 step 6: a raise signed by a root key while no signing key serves every recorded build."""
    if o.phase != "recovered":
        return None
    ents = raised_entries(o)
    if ents is None:
        return None
    return ("publish", tool_make(o, ents, root_signs=True))


def op_retire(o):
    """5-8 step 6, 5-9 step 4, 5-10: the admin report shows no device on an older build: the
    operator deletes the records of every key set but the key file's."""
    keep = frozenset({keyfile_build(o).keyset})
    if o.recorded == keep:
        return None
    gone = sorted(sorted(p) + sorted(r) for p, r in o.recorded - keep)
    return ("state", o._replace(recorded=keep, ops=o.ops + ("retire records %s" % gone,)))


STEPS = {
    "raise:def": op_raise("all", "def"),
    "raise:none": op_raise("all", "none"),
    "raise:early": op_raise("all", 2),
    "raise:late": op_raise("all", 12),
    "raise-w:def": op_raise(("windows",), "def"),
    "raise-a:def": op_raise(("android",), "def"),
    "raise-p:def": op_raise(("android_play",), "def"),
    "latest": op_latest,
    "add-play": op_add_play(0),
    "add-play+1": op_add_play(1),
    "add-ios": op_add_ios,
    "stop": op_stop,
    "relax:min": op_relax("min"),
    "relax:late": op_relax("late"),
    "relax:drop": op_relax("drop"),
    "relax:latest": op_relax("latest"),
    "new-epoch": op_new_epoch(),
    "new-epoch:102": op_new_epoch(ROOT2),
    "rotate": op_rotate,
    "destroy-retired": op_destroy_retired,
    "revoke-retired": op_revoke_retired,
    "wait": op_wait,
    "build": op_build,
    "build-newkeys": op_build_newkeys,
    "build-thief-floor": op_build_thief_floor,
    "build-next": op_build_next,
    "lose-root:101": op_lose_root(ROOT1),
    "lose-root:102": op_lose_root(ROOT2),
    "build-newroot": op_build_newroot,
    "decl-new-root": op_decl_new_root,
    "revoke-lost-root": op_revoke_lost_root,
    "leak": op_leak,
    "leak-policy": op_leak_policy,
    "void": op_void,
    "build-ahead": op_build_ahead,
    "reuse-epoch": op_reuse_epoch,
    "stale-decl": op_stale_decl,
    "theft:cur": op_theft("cur"),
    "theft:other": op_theft("other"),
    "theft:both": op_theft("both"),
    "recover": op_recover,
    "root-policy": op_root_policy,
    "root-raise": op_root_raise,
    "retire": op_retire,
}

# Steps that are mistakes (a refusal of the tool is what should happen) and steps of runbook 5 (a
# refusal that the runbook does not expect is reported): the dfs reads these.
MISTAKES = {"decl-new-root", "revoke-lost-root", "leak", "reuse-epoch", "stale-decl", "build-ahead",
            "build-thief-floor", "leak-policy", "void"}


def apply_step(o, name):
    """Runs one step: (OpState, the envelope it published or None, the tool's refusal or None)."""
    try:
        r = STEPS[name](o)
    except ToolRefusal as e:
        return None, None, "%s %s" % (e.code, e.why)
    if r is None:
        return None, None, None
    kind, x = r
    if kind == "publish":
        return publish(o, x, name), x, None
    return x, (x.live if x.live is not o.live else None), None


def genuine_decls(o):
    out = []
    for env in o.published + o.leaked:
        if env.root is not None and env.root.text not in out:
            out.append(env.root.text)
    return out


def n_envelopes(o):
    """N (no keys): every genuine envelope as it was (published, or leaked by a mistake); each root
    part with a policy part whose signature fails (the declaration alone); every genuine policy
    part stripped of its root part, and with every other genuine root part."""
    out = list(o.published + o.leaked)
    parts = [env.pol for env in o.published + o.leaked]
    roots = []
    for env in o.published + o.leaked:
        if env.root is not None and env.root not in roots:
            roots.append(env.root)
    for r in roots:
        out.append(label_env(Env(Part(parts[-1].k, parts[-1].text, valid=False), r), "D%d-alone" % r.text.epoch))
    for p in parts:
        out.append(label_env(Env(p, None), "%s-stripped" % p.text))
        for r in roots:
            out.append(label_env(Env(p, r), "%s+D%d" % (p.text, r.text.epoch)))
    return out


FUTURE = 999      # S dates a policy far ahead
FAR = 990         # S's far deadline (problem (가))
VLOCK = 4         # S's lock-out version (min 99.0.0)


def s_envelopes(o):
    """S (O once its key is revoked): policies signed with a stolen key, in every epoch whose
    declaration does not revoke it (epoch 1 needs none), carrying that declaration's genuine root
    part. Contents:
      lock   every entry min = latest = 99.0.0 (a lock-out);
      base   the epoch's base (epoch 1: its first policy): lagging devices brought down to it;
      copy   the epoch's newest genuine requirements (a fork at the same seq, or the next one);
      grace  every entry raised by one with a far deadline and grace_min 0 (problem (가));
      addp   the newest requirements plus android_play 0 (problem (나));
      loose  every entry min 0; drop: windows dropped (taken only where there is no basis);
      pkg    the newest requirements with the thief's package and an unattended install;
      the newest genuine text itself, re-signed with the stolen key.
    seq: g (the epoch's newest genuine seq), g + 1, g + STEP; issued: 0, the clock, FUTURE."""
    out = []
    for k in sorted(o.stolen):
        epochs = {1: None}
        for D in genuine_decls(o):
            if k not in D.revoked:
                epochs[D.epoch] = D
        for e, D in sorted(epochs.items()):
            gen = [env.pol.text for env in o.published + o.leaked if env.pol.text.epoch == e]
            if not gen:
                continue
            newestP = max(gen, key=lambda p: p.seq)
            g, newest = newestP.seq, newestP.ents
            base = D.base if D else min(gen, key=lambda p: p.seq).ents
            lock = {c: Entry(VLOCK, VLOCK) for c in ("windows", "android", "android_play")}
            grace = {c: Entry(min(VLOCK, x.min + 1), max(x.latest, min(VLOCK, x.min + 1)), FAR, 0)
                     for c, x in newest.items()}
            addp = dict(newest)
            addp.setdefault("android_play", Entry(0, max(x.latest for x in newest.values())))
            loose = {c: Entry(0, 0) for c in newest}
            drop = {c: x for c, x in newest.items() if c != "windows"} or None
            menu = [("lock", lock, (g + 1, g + STEP), (o.now, FUTURE), True),
                    ("base", base, (g + 1,), (o.now,), True),
                    ("copy", newest, (g, g + 1), (0, o.now, FUTURE), True),
                    ("grace", grace, (g + 1,), (o.now,), True),
                    ("addp", addp, (g + 1,), (o.now,), True),
                    ("loose", loose, (g + 1,), (o.now,), True),
                    ("pkg", newest, (g + 1,), (o.now,), "S")]
            if drop:
                menu.append(("drop", drop, (g + 1,), (o.now,), True))
            root = None
            if D is not None:
                root = next(env.root for env in o.published + o.leaked
                            if env.root is not None and env.root.text is D)
            for name, ents, seqs, issueds, pkg in menu:
                for s in seqs:
                    for i in issueds:
                        P = policy(e, s, i, ents, auto=(pkg == "S"), pkg=pkg, notes="S",
                                   label="S:%s(e%d s%d i%d)" % (name, e, s, i))
                        out.append(label_env(Env(Part(k, P), root), "%r/k%d" % (P, k)))
            out.append(label_env(Env(Part(k, newestP), root), "%r re-signed/k%d" % (newestP, k)))
    return out


# ================================================================================================
# 9. Invariants and goals                                                   (spec 6-1; G1-G6)
# ================================================================================================

VARIANTS = {
    "today": "today's comparison (no grace_min, no Android clause)",
    "no_android_clause": "RELAXES without clause 3",
    "basis_held": "comparing with the held policy across epochs",
    "no_issued": "no Issued rule (C7)",
    "all_or_nothing": "a refusal after B4 drops the declaration too",
    "eff_revoked": "enforcing a revoked key's policy",
    "load_drops_held": "LOAD drops the held policy under a later floor",
    "eff_old": "round 0's EFF (an older-epoch held policy over a later base)",
    "may_install_no_floor": "MAY_INSTALL without the floor",
    "b4c_off": "no B4c (a declaration of the device epoch without DCUR refused)",
    "c4_no_root": "an epoch 2+ policy taken without its declaration",
    "jump_root_exempt": "Jump not applied to root-signed policies",
    "load_whole": "round 1's LOAD (any failing part drops the whole record)",
    "save_no_rank": "SAVE overwrites without comparing RANK",
    "x12_off": "the tool ignores the key set records (round 1's X9/X12)",
    "floor_unverified": "floor --live without verifying the live file with the key file",
    "hbbs_no_wait": "hbbs-conf writes a raise at once (round 1's rule)",
}


def clamped(ents, issued):
    """What entries block at every device clock c, given now = max(c, issued) (3-1): a deadline at
    or before `issued` has passed for every clock. THRESHOLD(clamped(e), t) ==
    THRESHOLD(e, max(t, issued)) for every t."""
    return {c: (Entry(e.min, e.latest) if e.gu is not None and e.gu <= issued else e)
            for c, e in ents.items()}


def asks_less(new, old):
    """`new` asks less than `old` (Effs or None) at some device clock: RELAXES on the clamped
    entries (exact by INV-2): an entry dropped, a latest lowered, a threshold or LIMIT lower."""
    if old is None:
        return False
    if new is None:
        return True
    return relaxes_ref(clamped(old.ents, old.issued), clamped(new.ents, new.issued))


def blocks_less(new, old):
    """G1 in what is blocked: at some device clock, a version the app or the PC's rule blocks under
    `old` is let through under `new` (a missing entry blocks nothing)."""
    if old is None:
        return False
    on = clamped(old.ents, old.issued)
    nn = clamped(new.ents, new.issued) if new is not None else {}
    for t in cut_times(on, nn):
        for c, o in on.items():
            n = nn.get(c)
            if (threshold(n, t) if n is not None else 0) < threshold(o, t):
                return True
        for fam in FAMILIES:
            lo = family_limit(on, fam, t)
            if lo is not None:
                ln = family_limit(nn, fam, t)
                if (ln if ln is not None else 0) < lo:
                    return True
    return False


def g1_violation(st_after, eff_before):
    """G1 for a device that enforced `eff_before` and is now in `st_after`: None, or the class of a
    loosening that no declaration it holds allows: 'blocking' (min, deadline, the PC's rule) or
    'latest/entry' (a latest lowered, or an entry that blocked nothing dropped)."""
    eff_after = derived(st_after).EFF
    if not asks_less(eff_after, eff_before):
        return None
    D = st_after.decl
    base = eff_of_base(D) if D is not None else None
    if blocks_less(eff_after, eff_before):
        if base is None or blocks_less(eff_after, base):
            return "blocking"
        return None
    if base is None or asks_less(eff_after, base):
        return "latest/entry"
    return None


def eff_asks_no_less(new, old):
    """INV-4 as written: RELAXES on the entries, and issued never goes down."""
    if old is None:
        return True
    if new is None:
        return False
    return not relaxes_ref(old.ents, new.ents) and new.issued >= old.issued


def inv5b_ok(st, st2):
    """INV-5 (b): if EFF(S') asks less than EFF(S) somewhere, S' holds a declaration D and EFF(S')
    asks no less than D.base."""
    e, e2 = derived(st).EFF, derived(st2).EFF
    if e is None or (e2 is not None and not relaxes_ref(e.ents, e2.ents)):
        return True
    return st2.decl is not None and e2 is not None and not relaxes_ref(st2.decl.base, e2.ents)


def verified_decl(env, build):
    """The declaration of `env` if steps A and B1-B3 hold for it under `build`, else None."""
    r = env.root
    if env.form == "ok" and r is not None and root_part_holds(build, r):
        return r.text
    return None


def check_transition(st, env, st2, out, report, ctx):
    """2-1, 2-5 and INV-4 .. INV-9 on one transition; G1 in its two classes; G5."""
    d, d2 = derived(st), derived(st2)
    if d2.E < d.E:                                                              # INV-6
        report("INV-6", ctx, "E went down %d -> %d" % (d.E, d2.E))
    if not d.REVOKED <= d2.REVOKED:
        report("INV-6", ctx, "revocations shrank")
    if st2.root != st.root and st2.root != env.root:
        report("INV-6", ctx, "the declaration changed to something that is not the envelope's root part")
    if out.kind == "Taken" and st2.held.k in d2.REVOKED:                        # INV-7
        report("INV-7", ctx, "took a policy of revoked key %d" % st2.held.k)
    if d2.EFF is not None and d2.EFF.source == "policy" and st2.held.k in d2.REVOKED:
        report("INV-7", ctx, "enforces a policy of revoked key %d" % st2.held.k)
    if out.kind == "Taken" and (d2.E, st2.held.pol.seq) < st2.build.floor:      # INV-8, G5
        report("INV-8", ctx, "took %r below the floor %s" % (st2.held.pol, st2.build.floor))
    vd = verified_decl(env, st.build)                                           # INV-9, G3
    if vd is not None and vd.epoch > d.E and (st2.decl is None or st2.decl.epoch < vd.epoch):
        report("INV-9", ctx, "a verified declaration of epoch %d was not kept (%r)" % (vd.epoch, out))
    if vd is not None and st.decl is None and st.held is None and vd.epoch < st.build.floor[0] \
            and st2.decl is vd:                                                 # G5
        report("G5", ctx, "a fresh install took a declaration below its floor's epoch")
    if out.kind != "Taken" and st2 != st and st2 != State(st.build, env.root, st.held):
        report("2-1", ctx, "a refusal changed more than the declaration")
    if out.kind == "Unchanged" and st2 != st:
        report("2-5", ctx, "Unchanged with another state")
    if d.EFF is not None and d2.EFF is not None and d.EFF.obj is d2.EFF.obj and d.EFF.source == d2.EFF.source:
        return
    if d.CUR is not None and d2.E == d.E and not eff_asks_no_less(d2.EFF, d.EFF):  # INV-4
        report("INV-4", ctx, "EFF %s -> %s" % (fmt_eff(d.EFF), fmt_eff(d2.EFF)))
    if not inv5b_ok(st, st2):                                                   # INV-5 (b)
        report("INV-5", ctx, "EFF %s -> %s, no declaration allows it" % (fmt_eff(d.EFF), fmt_eff(d2.EFF)))
    g = g1_violation(st2, d.EFF)                                                # G1
    if g:
        report("G1 (%s)" % g, ctx, "EFF %s -> %s" % (fmt_eff(d.EFF), fmt_eff(d2.EFF)))


def check_state(st, report, ctx):
    """Per reachable state: INV-5 (a), INV-7, INV-10, INV-12's LOAD(SAVE(S)) = S (also by appendix
    A's load), 1-5's held.P.epoch <= DE."""
    d = derived(st)
    h = st.held
    if d.MAY_INSTALL and (d.EFF is None or d.EFF.obj is not h.pol or h.pol.epoch != d.E
                          or h.k in d.REVOKED or (d.E, h.pol.seq) < st.build.floor):
        report("INV-10", ctx, "MAY_INSTALL while EFF is %s" % fmt_eff(d.EFF))
    if load(st.build, record_of(st)) != st:
        report("INV-12", ctx, "LOAD(SAVE(S)) != S")
    if not V:
        diff = appendix_a_load_agrees(st.build, record_of(st))
        STATS["appendix A's load compared with 1-6 (records)"] += 1
        if diff:
            report("appendix A's load differs from 1-6", ctx, diff)
    if st.decl is not None and (d.EFF is None or relaxes_ref(st.decl.base, d.EFF.ents)):
        report("INV-5", ctx, "EFF %s asks less than the held declaration's base" % fmt_eff(d.EFF))
    if h is not None and h.pol.epoch > d.DE:
        report("1-5", ctx, "held.P.epoch > DE")
    if d.EFF is not None and d.EFF.source == "policy" and h.k in d.REVOKED:
        report("INV-7", ctx, "enforces a policy of a revoked key")


def fmt_eff(e):
    if e is None:
        return "nothing"
    return "%s %s i%d" % (e.source, fmt_ents(e.ents), e.issued)


def fmt_state(st):
    h, d = st.held, derived(st)
    held = ("%r/k%d%s" % (h.pol, h.k, "" if h.root is None else "+D%d" % h.root.text.epoch)) if h else "-"
    decl = "-" if st.root is None else "%r/k%d" % (st.root.text, st.root.k)
    return "[%s E%d decl=%s held=%s EFF=%s%s]" % (st.build.name, d.E, decl, held, fmt_eff(d.EFF),
                                                  " MAY_INSTALL" if d.MAY_INSTALL else "")


# ---- appendix A of the spec, transcribed literally, and a differential check against parts 3-5 ---

class AppendixA:
    """The reference functions of spec appendix A, as printed (entries as dicts with min, latest,
    gu, gm; texts compared by id; decl_ok/policy_ok stand for the text rules). Used only to check
    that appendix A and sections 1-5, 1-6 and 2 (parts 3-5 here) decide alike: the spec says that
    where they disagree, appendix A is a bug."""
    STEP = 1000
    ANDROID = ("android", "android_play")

    @staticmethod
    def lht(e):
        return (e["gm"], e["min"], e["gu"]) if "gu" in e else (e["min"], e["min"], None)

    @classmethod
    def covers(cls, n, o):
        nlo, nhi, nT = cls.lht(n)
        olo, ohi, oT = cls.lht(o)
        if nlo >= ohi:
            return True
        earlier_ = nT is None or (oT is not None and nT <= oT)
        return nlo >= olo and nhi >= ohi and earlier_

    @classmethod
    def relaxed(cls, B, P):
        out = set()
        for c, o in B.items():
            if c not in P or not cls.covers(P[c], o) or P[c]["latest"] < o["latest"]:
                out.add(c)
        for c in cls.ANDROID:
            sib = cls.ANDROID[1] if c == cls.ANDROID[0] else cls.ANDROID[0]
            if c in P and c not in B and sib in B and not cls.covers(P[c], B[sib]):
                out.add(c)
        return out

    @classmethod
    def relaxes(cls, B, P):
        return bool(cls.relaxed(B, P))

    @staticmethod
    def DE(d):
        return d["decl"]["epoch"] if d["decl"] else 1

    @classmethod
    def E(cls, d):
        return max(d["floor"][0], cls.DE(d))

    @staticmethod
    def REV(d):
        return d["decl"]["revoked"] if d["decl"] else frozenset()

    @classmethod
    def CUR(cls, d):
        h = d["held"]
        return h if h and h["pol"]["epoch"] == cls.E(d) else None

    @classmethod
    def DCUR(cls, d):
        x = d["decl"]
        return x if x and x["epoch"] == cls.E(d) else None

    @classmethod
    def low_same(cls, d):
        cur, (fe, fs), e = cls.CUR(d), d["floor"], cls.E(d)
        if cur and (e, cur["pol"]["seq"]) >= (fe, fs):
            return cur["pol"]["seq"], (cur["pol"]["seq"], cur["pol"]["id"])
        return (fs - 1 if fe == e and fs >= 1 else 0), None

    @classmethod
    def accept(cls, d, env, decl_ok, policy_ok):
        root, p, d1 = env["root"], env["pol"], d
        E = cls.E
        if root is not None:
            if root["k"] not in d["roots"]: return d, "RootKey"
            if not root["valid"]: return d, "RootSignature"
            D = root["text"]
            if not decl_ok(D): return d, "RootFormat"           # epoch >= 2, base rules
            if d["roots"] & D["revoked"]: return d, "RootRevokesRoot"
            if D["epoch"] < E(d): return d, "RootEpoch"
            if D["epoch"] == E(d) and cls.DCUR(d) is not None:
                if D["id"] != cls.DCUR(d)["id"]: return d, "RootFork"
            else:
                d1 = dict(d, decl=D)
        k = p["k"]
        if k not in d1["roots"] and k not in d1["pins"]: return d1, "UnknownKey"
        if k in cls.REV(d1): return d1, "RevokedKey"
        if not p["valid"]: return d1, "BadSignature"
        P = p["text"]
        if not policy_ok(P): return d1, "Field"
        if P["epoch"] != E(d1): return d1, "Epoch"
        if P["epoch"] >= 2 and (root is None or root["text"]["epoch"] != P["epoch"]): return d1, "Epoch"
        low, same = cls.low_same(d1)
        if same and P["seq"] == same[0]:
            return (d1, "Unchanged") if P["id"] == same[1] else (d1, "Fork")
        if P["seq"] <= low: return d1, "Rollback"
        if P["seq"] - low > cls.STEP: return d1, "Jump"
        if (E(d1), P["seq"]) < d1["floor"]: return d1, "BelowFloor"
        cur, dc = cls.CUR(d1), cls.DCUR(d1)
        B, Bi = ((cur["pol"]["plat"], cur["pol"]["issued"]) if cur else
                 (dc["base"], dc["issued"]) if dc else (None, None))
        if B is not None:
            if P["issued"] < Bi: return d1, "Issued"
            if cls.relaxes(B, P["plat"]): return d1, "Relaxation"
        held = {"pol": P, "k": k, "root_id": root["text"]["id"] if root else None}
        return dict(d1, held=held), "Taken"

    @classmethod
    def eff(cls, d):
        h = d["held"]
        if h and h["pol"]["epoch"] == cls.DE(d) and h["k"] not in cls.REV(d): return h["pol"]["plat"]
        return d["decl"]["base"] if d["decl"] else None

    @classmethod
    def may_install(cls, d):
        low, same = cls.low_same(d)
        return same is not None and d["held"]["k"] not in cls.REV(d)

    @staticmethod
    def rank(rec):
        D, H = rec["decl"], rec["held"]
        e = D["epoch"] if D else 1
        s = H["pol"]["seq"] if H and H["pol"]["epoch"] == e else 0
        return (e, s, 1 if H else 0)

    @staticmethod
    def load(build, rec, decl_ok, policy_ok):
        def root_ok(r):
            return (r is not None and r["k"] in build["roots"] and r["valid"] and decl_ok(r["text"])
                    and not (build["roots"] & r["text"]["revoked"]))
        decl = rec["root"]["text"] if root_ok(rec["root"]) else None                      # L2
        held, env = None, rec["env"]
        if env is not None:
            p, r = env["pol"], env["root"]
            P = p["text"]
            ok = ((r is None or root_ok(r)) and (p["k"] in build["roots"] or p["k"] in build["pins"])
                  and p["valid"] and policy_ok(P))                                       # L3
            if ok and P["epoch"] == 1:                                                    # L4
                ok = r is None
            elif ok:
                ok = (r is not None and r["text"]["epoch"] == P["epoch"] and decl is not None
                      and decl["epoch"] >= P["epoch"]
                      and (decl["epoch"] > P["epoch"] or decl["id"] == r["text"]["id"]))
            if ok:
                held = {"pol": P, "k": p["k"], "root_id": r["text"]["id"] if r else None}
        return dict(build, decl=decl, held=held)                                          # L5


_A_CACHE: dict = {}


def to_a(x):
    """The model's objects in appendix A's shapes (cached per object)."""
    if x is None:
        return None
    r = _A_CACHE.get(id(x))
    if r is not None and r[0] is x:
        return r[1]
    if isinstance(x, Policy):
        v = {"epoch": x.epoch, "seq": x.seq, "issued": x.issued, "id": x.serial, "plat": a_ents(x.ents), "_obj": x}
    elif isinstance(x, Decl):
        v = {"epoch": x.epoch, "revoked": x.revoked, "issued": x.issued, "id": x.serial, "base": a_ents(x.base),
             "_obj": x}
    else:
        raise TypeError(x)
    _A_CACHE[id(x)] = (x, v)
    return v


def a_ents(ents):
    out = {}
    for c, e in ents.items():
        d = {"min": e.min, "latest": e.latest}
        if e.gu is not None:
            d["gu"], d["gm"] = e.gu, e.gm
        out[c] = d
    return out


def a_build(b):
    return {"roots": b.roots, "pins": b.pins, "floor": b.floor}


def a_state(st):
    h = st.held
    return dict(a_build(st.build), decl=to_a(st.decl),
                held=None if h is None else {"pol": to_a(h.pol), "k": h.k,
                                             "root_id": h.root.text.serial if h.root else None})


def a_part(x):
    return None if x is None else {"k": x.k, "text": to_a(x.text), "valid": x.valid}


def a_env(env):
    return {"root": a_part(env.root), "pol": a_part(env.pol)}


def a_decl_ok(D):
    return decl_text_error(D["_obj"]) is None


def a_policy_ok(P):
    return policy_text_error(P["_obj"]) is None


def appendix_a_agrees(st, env):
    """None if appendix A and part 4 decide `env` alike for `st` (outcome, kept declaration, held
    policy, EFF, MAY_INSTALL, RANK), else a description of the difference. Envelopes whose text
    rules beyond decl_ok/policy_ok matter (JSON, v, server) are skipped, as appendix A leaves them
    out."""
    if env.form != "ok" or env.pol.form != "ok" or (env.root is not None and env.root.form != "ok"):
        return None
    st2, out = accept(st, env)
    d2, code = AppendixA.accept(a_state(st), a_env(env), a_decl_ok, a_policy_ok)
    mine = out.kind if out.kind != "Refused" else out.reason.split(":")[0]
    if code != mine:
        return "outcome: appendix A %s, section 2 %r" % (code, out)
    if (d2["decl"]["id"] if d2["decl"] else None) != (st2.decl.serial if st2.decl else None):
        return "declaration kept differs"
    h2 = d2["held"]
    a_held = (h2["pol"]["id"], h2["k"]) if h2 else None
    m_held = (st2.held.pol.serial, st2.held.k) if st2.held else None
    if a_held != m_held:
        return "held policy differs"
    a_eff = AppendixA.eff(d2)
    m_eff = derived(st2).EFF
    if (a_eff is None) != (m_eff is None) or (a_eff is not None and a_eff != a_ents(m_eff.ents)):
        return "EFF differs"
    if (AppendixA.may_install(d2) if d2["held"] else False) != derived(st2).MAY_INSTALL:
        return "MAY_INSTALL differs"
    if AppendixA.rank(d2) != rank(st2):
        return "RANK differs"
    return None


def appendix_a_load_agrees(build, rec):
    """None if appendix A's load and LOAD of 1-6 keep the same declaration and held policy."""
    st = load(build, rec)
    a_rec = {"root": a_part(rec.root),
             "env": None if rec.held is None else {"pol": {"k": rec.held.k, "text": to_a(rec.held.pol), "valid": True},
                                                   "root": a_part(rec.held.root)}}
    d = AppendixA.load(a_build(build), a_rec, a_decl_ok, a_policy_ok)
    if (d["decl"]["id"] if d["decl"] else None) != (st.decl.serial if st.decl else None):
        return "declaration kept differs"
    a_held = (d["held"]["pol"]["id"], d["held"]["k"]) if d["held"] else None
    m_held = (st.held.pol.serial, st.held.k) if st.held else None
    if a_held != m_held:
        return "held policy differs"
    return None


# ================================================================================================
# 10. The fixed cases of spec 7-4                                                     (spec 7-4)
# ================================================================================================
#
# Versions: 0.0.0=0, 1.0.0=1, 1.4.0=2, 1.5.0=3, 1.6.0=4, 1.6.1=5, 99.0.0=9.
# Times: hours since 2026-10-01T00:00Z; H(16, 15) is 2026-10-16T15:00Z.

def H(day, hour=3, month=10):
    return ((day - 1) if month == 10 else (31 + day - 1)) * 24 + hour


T14, T16, T17, T23, T30, N30, Y2099 = H(14, 15), H(16, 15), H(17, 15), H(23, 15), H(30, 15), \
    H(30, 15, 11), 10 ** 6
v00, v10, v14, v15, v16, v161, v99 = 0, 1, 2, 3, 4, 5, 9
VECTOR_BUILD = Build("vectors", frozenset({1, 2}), ROOTS, (1, 0))
VERSION_TEXT = {0: "0.0.0", 1: "1.0.0", 2: "1.4.0", 3: "1.5.0", 4: "1.6.0", 5: "1.6.1", 9: "99.0.0"}


def vector_history():
    """The history of 7-4: e1-s1 .. e3-s2, and the declarations D2, D3."""
    e1 = {"windows": Entry(v15, v16), "android": Entry(v15, v16), "ios": Entry(v15, v16)}
    g = Entry(v16, v16, T16, v15)
    g3 = Entry(v16, v161, T16, v15)
    ents3 = {"windows": g3, "android": g3, "android_play": Entry(v16, v161), "ios": Entry(v16, v16, T16, v15)}
    D2 = declaration(2, {1}, H(6), requirements(ents3), label="D2")
    ents31 = {"windows": Entry(v15, v161), "android": Entry(v15, v161), "android_play": Entry(v15, v161),
              "ios": Entry(v15, v16)}
    D3 = declaration(3, {1}, H(8), ents31, label="D3")
    r = Entry(v161, v161, T23, v15)
    ents32 = {"windows": r, "android": r, "android_play": r, "ios": Entry(v16, v16, T23, v15)}
    R2, R3 = Part(ROOT1, D2), Part(ROOT1, D3)
    hist = {
        "e1-s1": Env(Part(1, policy(1, 1, H(1), e1, label="e1-s1"))),
        "e1-s2": Env(Part(1, policy(1, 2, H(2), {"windows": g, "android": g, "ios": g}, label="e1-s2"))),
        "e1-s3": Env(Part(2, policy(1, 3, H(5), ents3, auto=True, label="e1-s3"))),
        "e2-s1": Env(Part(2, policy(2, 1, H(6), ents3, auto=True, label="e2-s1")), R2),
        "e2-s2": Env(Part(2, policy(2, 2, H(7), ents3, label="e2-s2")), R2),
        "e3-s1-relax": Env(Part(2, policy(3, 1, H(8), ents31, label="e3-s1-relax")), R3),
        "e3-s2": Env(Part(2, policy(3, 2, H(9), ents32, label="e3-s2")), R3),
    }
    for n, env in hist.items():
        label_env(env, n)
    return hist, D2, D3


def run_after(after, hist, build=VECTOR_BUILD):
    st = fresh(build)
    for n in after:
        st, out = accept(st, hist[n] if isinstance(n, str) else n)
        assert out.kind in ("Taken", "Unchanged"), (n, out)
    return st


class Case(NamedTuple):
    name: str
    after: list
    env: Env
    build: Build
    expect: str            # 'Taken', 'Unchanged' or 'Refused(<code>)'
    kept: Optional[dict]   # state_after_refusal as 7-4 states it (None: the state must not change)
    then: Optional[tuple]  # (envelope, expectation) for then_accept
    binding: str = "app"
    canonical: bool = True
    after_state: Optional[dict] = None   # state_after as 7-4 states it (only the members it names)


def summary(st):
    """A state in the manifest's words (7-4): the device epoch E, revoked, the held declaration
    (its text and signer; the vectors use root_sha256), the held policy (p_sha256), enforces,
    may_install."""
    d = derived(st)
    return {"epoch": d.E, "revoked": sorted(d.REVOKED),
            "root": None if st.decl is None else repr(st.decl),
            "root_k": None if st.root is None else st.root.k,
            "held": None if st.held is None else repr(st.held.pol),
            "enforces": "nothing" if d.EFF is None else d.EFF.source,
            "may_install": d.MAY_INSTALL}


def fixed_cases():
    """The cases of 7-4, every one the model can express (the JSON-level ones through `form`)."""
    hist, D2, D3 = vector_history()
    order = list(hist)
    upto = lambda last: order[: order.index(last) + 1]
    P11, P12, P13 = hist["e1-s1"].pol.text, hist["e1-s2"].pol.text, hist["e1-s3"].pol.text
    P21, P22 = hist["e2-s1"].pol.text, hist["e2-s2"].pol.text
    R2, R3 = hist["e2-s1"].root, hist["e3-s1-relax"].root
    F22 = Build("floor 2:2", frozenset({1, 2}), ROOTS, (2, 2))
    F15 = Build("floor 1:5", frozenset({1, 2}), ROOTS, (1, 5))
    cases = []

    def case(name, after, env, expect, build=VECTOR_BUILD, kept=None, then=None, binding="app",
             canonical=True, after_state=None):
        cases.append(Case(name, after, env, build, expect, kept, then, binding, canonical, after_state))

    base_kept = lambda D, held=None, may=False: {"root": repr(D), "held": held, "enforces": "base",
                                                 "may_install": may, "revoked": sorted(D.revoked),
                                                 "epoch": D.epoch}
    for n in order:
        case(n, upto(n)[:-1], hist[n], "Taken")
    case("e1-s2-again", ["e1-s1", "e1-s2"], hist["e1-s2"], "Unchanged")
    case("fresh-install-e2-s2", [], hist["e2-s2"], "Taken")
    case("e3-s2-missed-the-relaxation", upto("e2-s2"), hist["e3-s2"], "Taken")
    case("floor-exact", [], hist["e2-s2"], "Taken", build=F22)
    case("seq-step-1000", ["e1-s1"], Env(Part(1, policy(1, 1001, H(3), P11.ents))), "Taken")
    case("fresh-install-seq-1000", [], Env(Part(1, policy(1, 1000, H(3), P11.ents))), "Taken")
    case("stop-auto-install", upto("e1-s3"), Env(Part(2, policy(1, 4, H(6), P13.ents, auto=False))), "Taken")
    case("delay-auto-install", upto("e1-s3"), Env(Part(2, policy(1, 4, H(6), P13.ents, auto=True, notes="later"))),
         "Taken")
    case("grace-earlier", ["e1-s1", "e1-s2"],
         Env(Part(1, policy(1, 3, H(3), {c: Entry(v16, v16, T14, v15) for c in P12.ents}))), "Taken")
    case("grace-removed", ["e1-s1", "e1-s2"],
         Env(Part(1, policy(1, 3, H(3), dict(P12.ents, windows=Entry(v16, v16))))), "Taken")
    case("size-at-the-limit", ["e1-s1"], Env(Part(1, policy(1, 2, H(2), P11.ents, notes="long"))), "Taken")
    relax = {
        "relax-min-lowered": (["e1-s1", "e1-s2"], dict(P12.ents, ios=Entry(v15, v16, T16, v14))),
        "relax-grace-later": (["e1-s1", "e1-s2"], dict(P12.ents, windows=Entry(v16, v16, T17, v15))),
        "relax-grace-added": (["e1-s1"], dict(P11.ents, windows=Entry(v15, v16, T16, v14))),
        "relax-latest-lowered": (upto("e1-s3"), dict(P13.ents, windows=Entry(v16, v16, T16, v15))),
        "relax-entry-removed": (["e1-s1", "e1-s2"], {c: e for c, e in P12.ents.items() if c != "ios"}),
    }
    for name, (after, ents) in relax.items():
        last = hist[after[-1]].pol.text
        P = policy(1, last.seq + 1, last.issued, ents)
        case(name, after, Env(Part(1, P)), "Refused(Relaxation)")
        case(name + "-root", after, Env(Part(ROOT1, P)), "Refused(Relaxation)")
        Dx = declaration(2, set(), last.issued, requirements(ents), label="D2(%s)" % name)
        case(name + "-by-declaration", after, Env(Part(2, policy(2, 1, last.issued, ents)), Part(ROOT1, Dx)), "Taken")
    case("tampered-policy", [], Env(Part(1, P11, valid=False)), "Refused(BadSignature)")
    for name in ("tampered-signature", "wake-domain", "policy-under-root-domain", "another-key-as-1",
                 "signature-with-identity-r"):
        case(name, [], Env(Part(1, P11, valid=False)), "Refused(BadSignature)")
    case("another-root-as-101", ["e1-s1"], Env(Part(ROOT1, policy(1, 2, H(2), P11.ents), valid=False)),
         "Refused(BadSignature)")
    case("unknown-key", [], Env(Part(3, P11)), "Refused(UnknownKey)")
    case("envelope-v2", [], Env(Part(1, P11), form="Version"), "Refused(Version)")
    case("policy-v2", [], Env(Part(1, P11, form="Version")), "Refused(Version)")
    case("declaration-v2", [], Env(hist["e2-s1"].pol, Part(ROOT1, D2, form="Version")), "Refused(Version)")
    case("wrong-server", [], Env(Part(1, P11, form="Server")), "Refused(Server)")
    for name in ("key-number-zero", "key-number-as-text", "signature-63-bytes"):
        case(name, [], Env(Part(1, P11), form="Envelope"), "Refused(Envelope)", canonical=False)
    for name in ("too-large", "size-one-over"):
        case(name, [], Env(Part(1, P11), form="TooLarge"), "Refused(TooLarge)")
    case("declaration-by-signing-key", [], Env(hist["e2-s1"].pol, Part(2, D2)), "Refused(RootKey)")
    for name in ("declaration-signing-key-as-101", "declaration-under-policy-domain", "declaration-tampered"):
        case(name, [], Env(hist["e2-s1"].pol, Part(ROOT1, D2, valid=False)), "Refused(RootSignature)")
    case("declaration-revokes-root", [],
         Env(hist["e2-s1"].pol, Part(ROOT1, declaration(2, {1, ROOT1}, H(6), D2.base))), "Refused(RootRevokesRoot)")
    case("declaration-wrong-server", [], Env(hist["e2-s1"].pol, Part(ROOT1, D2, form="RootServer")),
         "Refused(RootServer)")
    case("revoked-key-later", upto("e2-s1"), Env(Part(1, policy(2, 2, H(7), P13.ents)), R2), "Refused(RevokedKey)")
    case("old-epoch-after-new", upto("e2-s1"), Env(Part(2, policy(1, 4, H(7), P13.ents))), "Refused(Epoch)")
    case("signing-key-raises-epoch", ["e1-s1"], Env(Part(2, policy(2, 1, H(6), P13.ents))), "Refused(Epoch)")
    case("root-policy-of-an-old-epoch", upto("e2-s1"), Env(Part(ROOT1, policy(1, 5000, H(7), P11.ents))),
         "Refused(Epoch)")
    case("seq-lower", ["e1-s1", "e1-s2"], hist["e1-s1"], "Refused(Rollback)")
    case("below-floor", [], Env(Part(1, policy(1, 4, H(3), P11.ents))), "Refused(Rollback)", build=F15)
    case("below-floor-by-root", [], Env(Part(ROOT1, policy(1, 4, H(3), P11.ents))), "Refused(Rollback)", build=F15)
    case("same-position-other-bytes", ["e1-s1"], Env(Part(1, policy(1, 1, H(1), P11.ents, notes="other"))),
         "Refused(Fork)")
    case("seq-step-1001", ["e1-s1"], Env(Part(1, policy(1, 1002, H(3), P11.ents))), "Refused(Jump)")
    case("fresh-install-seq-1001", [], Env(Part(1, policy(1, 1001, H(3), P11.ents))), "Refused(Jump)")
    case("min-above-latest", ["e1-s1"], Env(Part(1, policy(1, 2, H(2), dict(P11.ents, windows=Entry(v16, v15))))),
         "Refused(Field:min)")
    for name in ("version-*", "issued-not-rfc3339", "package-*", "contact-201-characters", "notes-2001-characters"):
        case(name, ["e1-s1"], Env(Part(1, P12, form="Field:<member>")), "Refused(Field:<member>)")
    for name in ("text-with-url", "unknown-field", "not-canonical"):
        case(name, [], Env(Part(1, policy(1, 1, H(1), P11.ents, notes=name))), "Taken", binding="tool",
             canonical=(name == "text-with-url"))
    # verdict changed
    case("root-seq-jump", ["e1-s1"], Env(Part(ROOT1, policy(1, 5000, H(3), P11.ents))), "Refused(Jump)")
    case("root-policy-raises-epoch", ["e1-s1"], Env(Part(ROOT1, policy(2, 1, H(3), P11.ents))), "Refused(Epoch)")
    case("revoke-in-the-same-epoch", ["e1-s1", "e1-s2"],
         Env(Part(1, policy(1, 3, H(3), P12.ents)), Part(ROOT1, declaration(1, {2}, H(3), requirements(P12.ents)))),
         "Refused(RootFormat)")
    case("min-raised-with-new-grace", upto("e1-s3"),
         Env(Part(2, policy(1, 4, H(6), dict(P13.ents, windows=Entry(v161, v161, N30, v15))))), "Refused(Relaxation)")
    case("new-entry-below-the-minimums", ["e1-s1", "e1-s2"],
         Env(Part(1, policy(1, 3, H(3), dict(P12.ents, android_play=Entry(v10, v16))))), "Refused(Relaxation)")
    case("revoked-key-with-declaration", [], Env(Part(1, P21), R2), "Refused(RevokedKey)", kept=base_kept(D2))
    case("epoch-differs-from-declaration", [], Env(Part(2, policy(3, 1, H(6), P13.ents)), R2),
         "Refused(Epoch)", kept=base_kept(D2))
    case("good-declaration-bad-policy", ["e1-s1"], Env(Part(2, P21, valid=False), R2),
         "Refused(BadSignature)", kept=base_kept(D2, held="e1-s1"), then=(hist["e2-s2"], "Taken"))
    case("below-floor-in-epoch-2", [], hist["e2-s1"], "Refused(Rollback)", build=F22,
         kept=dict(base_kept(D2), epoch=2))
    case("declaration-epoch-back", upto("e3-s1-relax"), hist["e2-s2"], "Refused(RootEpoch)")
    case("declaration-of-epoch-1", upto("e2-s1"),
         Env(Part(2, policy(1, 4, H(7), P13.ents)), Part(ROOT1, declaration(1, set(), H(6), {}))),
         "Refused(RootFormat)")
    case("duplicate-field", [], Env(Part(1, P11, form="Field:json")), "Refused(Field:json)", canonical=False)
    # new in round 0/1
    case("relaxation-declaration-alone", upto("e2-s2"), Env(Part(2, hist["e3-s1-relax"].pol.text, valid=False), R3),
         "Refused(BadSignature)", kept=base_kept(D3, held="e2-s2"), then=(hist["e3-s2"], "Taken"))
    case("raise-keeps-the-old-min", upto("e1-s3"),
         Env(Part(2, policy(1, 4, H(6), dict(P13.ents, windows=Entry(v161, v161, N30, v16))))), "Taken")
    case("raise-far-deadline-level-kept", upto("e1-s3"),
         Env(Part(2, policy(1, 4, H(6), {c: Entry(v161, v161, Y2099, v16) for c in P13.ents}))), "Taken")
    case("raise-while-pending-same-deadline", ["e1-s1", "e1-s2"],
         Env(Part(1, policy(1, 3, H(3), dict(P12.ents, windows=Entry(v161, v161, T16, v15))))), "Taken")
    case("raise-while-pending-later-deadline", ["e1-s1", "e1-s2"],
         Env(Part(1, policy(1, 3, H(3), dict(P12.ents, windows=Entry(v161, v161, T23, v15))))), "Refused(Relaxation)")
    case("raise-while-pending-later-deadline-pending-min-now", ["e1-s1", "e1-s2"],
         Env(Part(1, policy(1, 3, H(3), dict(P12.ents, windows=Entry(v161, v161, T23, v16))))), "Taken")
    case("android-play-same-min-later-deadline", ["e1-s1", "e1-s2"],
         Env(Part(1, policy(1, 3, H(3), dict(P12.ents, android_play=Entry(v16, v16, T30, v15))))),
         "Refused(Relaxation)")
    case("android-play-at-level", ["e1-s1", "e1-s2"],
         Env(Part(1, policy(1, 3, H(3), dict(P12.ents, android_play=Entry(v16, v16, T16, v15))))), "Taken")
    for name, e in (("grace-min-without-grace-until", Entry(v15, v16, None, v14)),
                    ("grace-until-without-grace-min", Entry(v15, v16, T16, None)),
                    ("grace-min-not-below-min", Entry(v15, v16, T16, v15))):
        case(name, ["e1-s1"], Env(Part(1, policy(1, 2, H(2), dict(P11.ents, windows=e)))), "Refused(Field:grace_min)")
    case("grace-min-as-number", ["e1-s1"], Env(Part(1, P12, form="Field:json")), "Refused(Field:json)",
         canonical=False)
    case("policy-without-epoch", [], Env(Part(1, P11, form="Field:json")), "Refused(Field:json)", canonical=False)
    case("root-signed-policy-taken", ["e1-s1", "e1-s2"], Env(Part(ROOT1, policy(1, 3, H(2), P12.ents))), "Taken")
    case("declaration-without-base", [], Env(hist["e2-s1"].pol, Part(ROOT1, D2, form="RootFormat")),
         "Refused(RootFormat)")
    case("declaration-base-bad-entry", [],
         Env(hist["e2-s1"].pol, Part(ROOT1, declaration(2, {1}, H(6), {"windows": Entry(v16, v14)}))),
         "Refused(RootFormat)")
    case("declaration-base-grace-min-not-below-min", [],
         Env(hist["e2-s1"].pol, Part(ROOT1, declaration(2, {1}, H(6), {"windows": Entry(v16, v161, T16, v16)}))),
         "Refused(RootFormat)")
    for name in ("declaration-base-extra-member", "declaration-unknown-member", "declaration-revoked-null",
                 "entry-null"):
        case(name, [], hist["e2-s1"], "Taken", binding="tool", canonical=False)
    case("declaration-fork", upto("e2-s1"),
         Env(hist["e2-s2"].pol, Part(ROOT1, declaration(2, {1}, H(6), D2.base, variant="fork"))),
         "Refused(RootFork)")
    case("declaration-by-second-root", [], Env(hist["e2-s1"].pol, Part(ROOT2, D2)), "Taken",
         after_state={"epoch": 2, "revoked": [1], "root_k": ROOT2})
    case("declaration-by-unpinned-root", [], Env(hist["e2-s1"].pol, Part(103, D2)), "Refused(RootKey)")
    case("declaration-revokes-second-root", [],
         Env(hist["e2-s1"].pol, Part(ROOT1, declaration(2, {1, ROOT2}, H(6), D2.base))), "Refused(RootRevokesRoot)")
    case("epoch-2-without-declaration", [], Env(hist["e2-s2"].pol), "Refused(Epoch)")
    case("e2-s2-stripped", upto("e2-s1"), Env(hist["e2-s2"].pol), "Refused(Epoch)")
    case("unknown-key-with-declaration", [], Env(Part(3, P21), R2), "Refused(UnknownKey)", kept=base_kept(D2))
    forged = label_env(Env(Part(1, policy(1, 4, H(6), {c: Entry(v99, v99) for c in P13.ents}))), "forged-minimum")
    case("forged-minimum", upto("e1-s3"), forged, "Taken")
    case("forgery-healed-by-the-declaration", upto("e1-s3") + [forged], hist["e2-s1"], "Taken")
    case("forgery-then-the-newest", upto("e1-s3") + [forged], hist["e3-s2"], "Taken")
    case("forgery-then-declaration-alone", upto("e1-s3") + [forged], Env(Part(2, P21, valid=False), R2),
         "Refused(BadSignature)", kept=base_kept(D2, held=repr(forged.pol.text)))
    Db = declaration(2, set(), H(6), {"windows": Entry(v161, v161)}, label="D2(base windows 1.6.1)")
    case("policy-below-its-base", [], Env(Part(2, policy(2, 1, H(6), {"windows": Entry(v16, v161)})), Part(ROOT1, Db)),
         "Refused(Relaxation)", kept=base_kept(Db))
    case("issued-back", upto("e1-s3"), Env(Part(1, policy(1, 4, H(4), P13.ents))), "Refused(Issued)")
    case("issued-before-declaration", [], Env(Part(2, policy(2, 1, H(5), P13.ents)), R2),
         "Refused(Issued)", kept=base_kept(D2))
    # new in round 2
    case("signature-trailing-bits", [], Env(Part(1, P11), form="Envelope"), "Refused(Envelope)", canonical=False)
    case("body-not-utf8", upto("e2-s2"), Env(hist["e3-s2"].pol, R3, form="Envelope"), "Refused(Envelope)",
         canonical=False)
    case("lone-surrogate-in-known-member", [], Env(Part(1, P11, form="Field:json")), "Refused(Field:json)",
         canonical=False)
    case("lone-surrogate-in-unknown-member", [], Env(Part(1, policy(1, 1, H(1), P11.ents, notes="\\ud800"))), "Taken",
         binding="tool", canonical=False)
    case("package-withdrawn", upto("e1-s3"), Env(Part(2, policy(1, 4, H(6), P13.ents, auto=False, pkg=False))),
         "Taken", after_state={"may_install": True})
    case("declaration-revokes-old-root-number", [],
         Env(hist["e2-s1"].pol, Part(ROOT1, declaration(2, {1, 103}, H(6), D2.base, label="D2(revoked [1, 103])"))),
         "Taken", binding="tool")
    return cases, hist


def case_result(c, hist):
    """Runs one case: (state before, state after, outcome, then-outcome, problems)."""
    st = run_after(c.after, hist, c.build)
    st2, out = accept(st, c.env)
    problems = []
    if repr(out) != c.expect:
        problems.append("spec says " + c.expect)
    if out.kind == "Refused":
        if c.kept is None and st2 != st:
            problems.append("the state changed: " + fmt_state(st2))
        if c.kept is not None:
            s = summary(st2)
            for key, want in c.kept.items():
                if s.get(key) != want:
                    problems.append("kept %s %s, spec %s" % (key, s.get(key), want))
    elif c.after_state is not None:
        s = summary(st2)
        for key, want in c.after_state.items():
            if s.get(key) != want:
                problems.append("after %s %s, spec %s" % (key, s.get(key), want))
    then_out = None
    if c.then is not None:
        _, then_out = accept(st2, c.then[0])
        if repr(then_out) != c.then[1]:
            problems.append("then: spec says " + c.then[1])
    return st, st2, out, then_out, problems


def store_cases():
    """The store checks of 7-6 on the vector history: (name, detail, ok); a check that cannot run
    (under a broken variant) counts as a mismatch."""
    try:
        return _store_cases()
    except Exception as e:                      # noqa: BLE001 - a broken variant may crash a check
        return [("store checks", "could not run: %r" % (e,), False)]


def _store_cases():
    hist, D2, D3 = vector_history()
    order = list(hist)
    R2, R3 = hist["e2-s1"].root, hist["e3-s1-relax"].root
    out = []
    b = VECTOR_BUILD
    states = [fresh(b)]
    st = fresh(b)
    for n in order:
        st, _ = accept(st, hist[n])
        states.append(st)
    for x in states:                                                   # LOAD(SAVE(S)) = S
        out.append(("load-save %s" % (repr(x.held.pol) if x.held else "fresh"), "", load(b, save(None, x)) == x))
    later = Build("floor 2:5", b.pins, b.roots, (2, 5))                # a later floor keeps the held policy
    st22 = run_after(order[:5], hist)
    y = load(later, record_of(st22))
    out.append(("later-floor-keeps-held", fmt_state(y),
                y.held == st22.held and derived(y).EFF == derived(st22).EFF and not derived(y).MAY_INSTALL))
    st_alone, _ = accept(run_after(order[:5], hist), Env(Part(2, hist["e3-s1-relax"].pol.text, valid=False), R3))
    z = load(b, record_of(st_alone))                                   # a declaration newer than the policy
    out.append(("record-declaration-newer-than-policy", fmt_state(z),
                z == st_alone and derived(z).EFF.source == "base" and not derived(z).MAY_INSTALL))
    w, _ = accept(fresh(b), Env(Part(2, hist["e2-s1"].pol.text, valid=False), R2))
    out.append(("record-declaration-without-policy", fmt_state(w),
                load(b, record_of(w)) == w and w.held is None and derived(w).EFF.source == "base"))
    crafted = Record(R2, Held(policy(2, 5, H(7), D2.base, label="crafted"), 1, R2))
    c = load(b, crafted)                                               # a crafted record: revoked signer
    out.append(("crafted-record-revoked-signer-enforces-base", fmt_state(c),
                c.held is not None and derived(c).EFF.source == "base" and not derived(c).MAY_INSTALL))
    # round 2: an L2 failure keeps nothing (with no held policy: a fresh start); L3/L4 failures keep the declaration
    out.append(("L2-declaration-revokes-root: fresh", "",
                load(b, Record(Part(ROOT1, declaration(2, {1, ROOT2}, H(6), D2.base)), None)) == fresh(b)))
    keeps_decl = {
        "L3-unpinned-signer": Record(R2, Held(st22.held.pol, 3, R2)),
        "L3-held-text-breaks-C3": Record(R2, Held(policy(2, 3, H(7), {"windows": Entry(3, 1)}), 2, R2)),
        "L4-epoch1-policy-with-root-part": Record(R2, Held(hist["e1-s3"].pol.text, 2, R2)),
        "L4-root-older-than-policy": Record(R2, Held(hist["e3-s2"].pol.text, 2, R3)),
        "L4-policy-without-root-part": Record(R2, Held(st22.held.pol, 2, None)),
    }
    for name, rec in keeps_decl.items():
        s = load(b, rec)
        out.append((name + ": declaration kept, held dropped", fmt_state(s),
                    s.decl is D2 and s.held is None and derived(s).EFF.source == "base"))
    fork = Part(ROOT1, declaration(2, {1}, H(6), D2.base, variant="fork"))
    s = load(b, Record(fork, st22.held))
    out.append(("L4-same-epoch-other-text: that declaration kept, held dropped", fmt_state(s),
                s.root == fork and s.held is None))
    nb = Build("keys 2,3", frozenset({2, 3}), b.roots, (2, 2))     # another key set (1-6, round 2)
    s = load(nb, record_of(st22))
    out.append(("another-key-set-keeps-what-it-verifies (key 2's policy)", fmt_state(s),
                s.decl is D2 and s.held == st22.held))
    st12 = run_after(order[:2], hist)
    s = load(nb, record_of(st12))
    out.append(("another-key-set-drops-an-unpinned-signer (key 1's policy)", fmt_state(s), s == fresh(nb)))
    rb = Build("roots 102,103", b.pins, frozenset({ROOT2, 103}), (1, 0))
    st32 = run_after(order, hist)                                    # D3 is signed by 101 alone
    s = load(rb, record_of(st32))
    out.append(("another-root-set-drops-101's-declaration-and-its-epoch's-policy", fmt_state(s), s == fresh(rb)))
    st13 = run_after(order[:3], hist)                                # an epoch-1 policy of key 2 under roots 102, 103
    s = load(rb, record_of(st13))
    out.append(("another-root-set-keeps-an-epoch-1-policy", fmt_state(s), s.held == st13.held and s.decl is None))
    a = run_after(order[:5], hist)                                   # two processes: RANK
    c2 = st_alone
    for first, second in ((a, c2), (c2, a)):
        rec = save(save(None, first), second)
        back = load(b, rec)
        out.append(("two-processes-%s-then-%s" % (rank(first), rank(second)), fmt_state(back),
                    rank(back) == max(rank(a), rank(c2)) and derived(back).EFF.obj is D3))
    for x in states + [y, z, w, c, st_alone]:                       # appendix A's load agrees
        diff = appendix_a_load_agrees(x.build, record_of(x))
        if diff:
            out.append(("appendix-A-load %s" % fmt_state(x), diff, False))
    return out


def run_cases(verbose=True):
    cs, hist = fixed_cases()
    bad = 0
    for c in cs:
        st, st2, out, then_out, problems = case_result(c, hist)
        bad += bool(problems)
        if verbose or problems:
            extra = "" if then_out is None else "; then %s: %r" % (ENV_LABEL.get(c.then[0]), then_out)
            print("  %-52s %-26s %s%s" % (c.name, repr(out), "MISMATCH: " + "; ".join(problems)
                                          if problems else "ok", extra))
            if verbose and st2 != st and out.kind == "Refused":
                print("  %52s kept: %s" % ("", fmt_state(st2)))
    print("cases: %d, mismatches with spec 7-4: %d" % (len(cs), bad))
    sc = store_cases()
    for name, detail, ok in sc:
        if verbose or not ok:
            print("  store %-46s %s %s" % (name, "ok" if ok else "MISMATCH", detail))
    print("store cases (7-6): %d, mismatches: %d" % (len(sc), sum(1 for c in sc if not c[2])))
    return bad


def cases_json():
    """The oracle's output: one JSON line per case in the manifest's terms (7-4). `root` and `held`
    name the texts (the vectors use root_sha256 and p_sha256 of them)."""
    cs, hist = fixed_cases()
    for c in cs:
        st, st2, out, then_out, problems = case_result(c, hist)
        row = {"name": c.name, "binding": c.binding, "canonical": c.canonical,
               "after": [a if isinstance(a, str) else ENV_LABEL.get(a, "?") for a in c.after],
               "floor": "%d:%d" % c.build.floor, "spec": c.expect, "model_agrees": not problems}
        if c.binding == "tool":
            row["app_expect"] = "accept" if out.kind != "Refused" else "reject"
        if out.kind == "Refused":
            row.update(expect="reject", reason=out.reason)
            if st2 != st:
                row["state_after_refusal"] = summary(st2)
        else:
            row.update(expect="accept", outcome=out.kind.lower(), state_after=summary(st2))
        if then_out is not None:
            row["then_accept"] = {"envelope": ENV_LABEL.get(c.then[0]), "outcome": repr(then_out)}
        print(json.dumps(row))


def covers_json():
    """covers.json of 7-4: all 256 pairs of entries over versions {0.0.0, 1.5.0, 1.6.0, 1.6.1} and
    deadlines {none, T1, T2} with the expected COVERS."""
    vs = (v00, v15, v16, v161)
    ents = entry_domain(versions=vs, deadlines=(None, T16, T23))
    tx = {None: None, T16: "2026-10-16T15:00:00Z", T23: "2026-10-23T15:00:00Z"}

    def js(e):
        d = {"min": VERSION_TEXT[e.min]}
        if e.gu is not None:
            d.update(grace_until=tx[e.gu], grace_min=VERSION_TEXT[e.gm])
        return d
    rows = [{"new": js(n), "old": js(o), "covers": covers(n, o)} for n in ents for o in ents]
    print(json.dumps(rows, indent=0))


# ================================================================================================
# 11. Search drivers and the report
# ================================================================================================

STATS = collections.Counter()


class Report:
    """Violations and notes by kind: a count and the smallest trace (fewest operator steps, then
    fewest device events) of each kind."""

    def __init__(self):
        self.best, self.count = {}, collections.Counter()

    def __call__(self, kind, ctx, detail):
        self.count[kind] += 1
        best = self.best.get(kind)
        if best is not None and isinstance(ctx, Ctx) and len(ctx.node.o.ops) > best[0][0]:
            return
        size = ctx.size() if hasattr(ctx, "size") else (len(ctx[0].split(" | ")) if ctx else 0, 0)
        if best is None or size < best[0]:
            self.best[kind] = (size, ctx() if callable(ctx) else list(ctx), detail)

    def show(self, title):
        viol = sorted(k for k in self.count if not k.startswith("note"))
        notes = sorted(k for k in self.count if k.startswith("note"))
        print("\n== %s: %s" % (title, "no violation" if not viol else "VIOLATIONS: " +
                                 ", ".join("%s x%d" % (k, self.count[k]) for k in viol)))
        if notes:
            print("   notes: " + ", ".join("%s x%d" % (k, self.count[k]) for k in notes))
        for kind in viol + notes:
            size, lines, detail = self.best[kind]
            print("-- %s: %s" % (kind, detail))
            for line in lines:
                print("     " + line)


class Node:
    """One operator history and every device state reachable in it (by any envelope of the pool in
    any order: devices that see every envelope, skip some, are off for a stretch; restarts and
    updates under every build; fresh installs of every build). parent_of gives traces."""

    def __init__(self, o, pool, reach, parent_of):
        self.o, self.pool, self.reach, self.parent_of = o, pool, reach, parent_of


_ACCEPT: dict = {}
_SEEN_STATES: set = set()


def step(st, env):
    """accept() with a memo; `new` is True the first time a pair is evaluated."""
    key = (st, env)
    r = _ACCEPT.get(key)
    if r is not None:
        return r, False
    r = accept(st, env)
    if len(_ACCEPT) > 3_000_000:
        _ACCEPT.clear()
    _ACCEPT[key] = r
    STATS["transitions evaluated"] += 1
    return r, True


def device_trace(node, st):
    out = []
    while True:
        parent, ev = node.parent_of[st]
        out.append(ev if isinstance(ev, str) else env_name(ev))
        if parent is None:
            return list(reversed(out))
        st = parent


class Ctx:
    """A lazily formatted trace."""

    def __init__(self, node, st, extra=()):
        self.node, self.st, self.extra = node, st, extra

    def size(self):
        return (len(self.node.o.ops), len(device_trace(self.node, self.st)))

    def __call__(self):
        lines = ["operator: " + " | ".join(self.node.o.ops),
                 "device:   " + " -> ".join(device_trace(self.node, self.st)),
                 "state:    " + fmt_state(self.st)]
        return lines + [x() if callable(x) else x for x in self.extra]


def pool_of(o):
    seen, out = set(), []
    for env in n_envelopes(o) + s_envelopes(o):
        if env not in seen:
            seen.add(env)
            out.append(env)
    return out


def expand(parent, o, report):
    """The node for operator state `o` below `parent`: its pool and the closure of the device
    states, checking every transition and state the first time it is met."""
    pool = pool_of(o)
    if parent is None:
        reach, parent_of, old_pool, old_builds = set(), {}, set(), ()
    else:
        reach, parent_of = set(parent.reach), dict(parent.parent_of)
        old_pool, old_builds = set(parent.pool), parent.o.builds
    node = Node(o, pool, reach, parent_of)
    new_envs = [e for e in pool if e not in old_pool]
    new_builds = [b for b in o.builds if b not in old_builds]
    frontier = collections.deque()

    def add(st, parent_st, ev):
        if st in reach:
            return
        reach.add(st)
        parent_of[st] = (parent_st, ev)
        frontier.append(st)
        if st not in _SEEN_STATES:
            _SEEN_STATES.add(st)
            check_state(st, report, Ctx(node, st))

    for b in new_builds:
        add(fresh(b), None, "fresh install of %s" % b.name)
    fresh_states = set(frontier)
    for st in sorted(reach, key=state_key):
        if st in fresh_states:
            continue
        for env in new_envs:
            transit(node, st, env, add, report)
        if new_builds:
            restarts(node, st, add, report, new_builds)
    while frontier:
        st = frontier.popleft()
        for env in pool:
            transit(node, st, env, add, report)
        restarts(node, st, add, report, o.builds)
    STATS["closures computed"] += 1
    return node


def transit(node, st, env, add, report):
    (st2, out), new = step(st, env)
    if new:
        ctx = Ctx(node, st, [lambda: "envelope: %s -> %r" % (env_name(env), out),
                             lambda: "after:    " + fmt_state(st2)])
        check_transition(st, env, st2, out, report, ctx)
        if not V:
            diff = appendix_a_agrees(st, env)
            STATS["appendix A compared with section 2 (transitions)"] += 1
            if diff:
                report("appendix A differs from section 2", ctx, diff)
    add(st2, st, env)


_SEEN_RESTARTS: set = set()


def restarts(node, st, add, report, builds):
    """A restart under another build: of the same key set (newer or older; INV-12: decl, held and
    EFF are kept; G1), or of another key set (an update, 1-4, 1-6: LOAD part by part keeps what the
    build verifies; dropping a part is R1, a note). The state joins the closure either way."""
    for b in builds:
        if b == st.build:
            continue
        st2 = load(b, record_of(st))
        other = b.keyset != st.build.keyset
        key = (st, b)
        if key not in _SEEN_RESTARTS:
            _SEEN_RESTARTS.add(key)
            dropped = st2.held != st.held or st2.root != st.root
            if not other and dropped:
                report("INV-12", Ctx(node, st, ["restart under %s: %s" % (b.name, fmt_state(st2))]),
                       "a build of the same keys lost the held policy or declaration")
            if other:
                STATS["updates to another key set: %s" % ("dropped a part" if dropped else "kept every part")] += 1
            g = g1_violation(st2, derived(st).EFF)
            if g:
                if other and dropped:
                    report("note R1: an update to another key set dropped a part it cannot verify (%s)" % g,
                           Ctx(node, st, ["update to %s (pins %s, roots %s): %s"
                                          % (b.name, sorted(b.pins), sorted(b.roots), fmt_state(st2))]),
                           "fails open until the first check (R1)")
                else:
                    report("G1 (%s) at a restart%s" % (g, " (another key set, nothing dropped)" if other else ""),
                           Ctx(node, st, ["restart under %s: %s" % (b.name, fmt_state(st2))]),
                           "a restart under another build loosened what is enforced")
            if not V:
                diff = appendix_a_load_agrees(b, record_of(st))
                STATS["appendix A's load compared with 1-6 (restarts)"] += 1
                if diff:
                    report("appendix A's load differs from 1-6", Ctx(node, st, ["under %s" % b.name]), diff)
        add(st2, st, "%s %s" % ("update to build" if other else "restart under build", b.name))


def state_key(st):
    h = st.held
    return (rank(st), st.decl.serial if st.decl else 0, st.root.k if st.root else 0,
            h.pol.serial if h else 0, h.k if h else 0, st.build.floor, tuple(sorted(st.build.pins)),
            tuple(sorted(st.build.roots)), st.build.name)


def made_by_thief(st):
    return st.held is not None and st.held.pol.notes == "S"


def newest_declaration(o):
    ds = [env.root.text for env in o.published if env.root is not None]
    return max(ds, key=lambda d: d.epoch) if ds else None


def check_liveness(node, report, g2, kind="INV-11"):
    """INV-11 for the newest genuine envelope (the live file): every reachable state takes it (Taken
    or Unchanged) unless (a) a key that signed it is not pinned by the device's build, which X12
    allows only for a build whose key set record the operator deleted (R9, R16), (b) it is more than
    STEP ahead of LOW, (c) CUR is a policy the operator did not sign, (d) the device holds a
    declaration that was not published or another text of a published declaration's epoch, (e) CUR
    is a signed, unpublished policy above the live one, or a voided one (until the next
    declaration).

    With g2 (after the runbook's recovery, G2): (c) is no excuse; (a) for a recorded build is
    stranding; a device that took it must be back: EFF the newest policy, the stolen keys revoked,
    installs allowed where its floor allows, and no envelope of the pool moves it off again.
    Also notes (not violations) states that enforce less than the newest published declaration's
    base without holding it: the reach of R2 (S) and R3 (N)."""
    o = node.o
    newest = o.live
    signed = {env.pol.text for env in o.published + o.leaked}
    leaked_pols = {env.pol.text for env in o.leaked}
    published_pols = {env.pol.text for env in o.published}
    voided_pols = {env.pol.text for env in o.voided}
    published_decls = {env.root.text for env in o.published if env.root is not None}
    Dn = newest_declaration(o)
    base_n = eff_of_base(Dn) if Dn is not None else None
    back = set()
    for st in node.reach:
        if base_n is not None and (st.decl is None or st.decl.epoch < Dn.epoch) and st.build.keyset == B0.keyset:
            e = derived(st).EFF
            if e is not None and asks_less(e, base_n) and blocks_less(e, base_n):
                lagging = "by a declaration of an older epoch" if st.decl is not None else "in epoch 1"
                report("note R2/R3: below the newest declaration's base, %s [%s]"
                       % (lagging, "S" if made_by_thief(st) else "N"),
                       Ctx(node, st, ["newest declaration: %r" % Dn]), fmt_eff(e))
        (st2, out), _ = step(st, newest)
        STATS["liveness probes"] += 1
        if out.kind in ("Taken", "Unchanged"):
            if g2:
                back.add(st2)
            continue
        S1, _ = step_b(st, newest)
        d1 = derived(S1)
        k = newest.pol.k
        recorded = st.build.keyset in o.recorded
        if out.reason == "RootKey" or (k not in st.build.roots and k not in st.build.pins):
            if recorded:
                report("%s: the newest envelope is signed by a key a recorded build does not pin (%s)"
                       % ("G2: stranded" if g2 else "INV-11 (a) for a recorded build", out.reason),
                       Ctx(node, st, ["newest:   %s -> %r" % (env_name(newest), out)]),
                       "X12 should keep every key that signs to keys every recorded build pins")
            else:
                STATS["liveness excused: (a) a key the build does not pin, its record deleted (R9, R16)"] += 1
            continue
        if newest.pol.text.seq - d1.LOW > STEP:
            STATS["liveness excused: (b) jump"] += 1
            continue
        cur = d1.CUR
        if cur is not None and cur.pol not in signed and not g2:
            STATS["liveness excused: (c) a policy the operator did not sign"] += 1
            continue
        k2 = "G2: not back" if g2 else kind
        if cur is not None and cur.pol in voided_pols:
            if cur.pol.epoch == newest.pol.text.epoch:
                STATS["liveness excused: (e) holds a voided policy (until the next declaration)"] += 1
                continue
            k2 = kind + " (e) after the next declaration"
        elif (st.decl is None or st.decl in published_decls) and cur is not None \
                and cur.pol in leaked_pols and cur.pol not in published_pols:
            if (cur.pol.epoch, cur.pol.seq) > (newest.pol.text.epoch, newest.pol.text.seq):
                STATS["liveness excused: (e) holds a signed policy above the live one (until the next publish)"] += 1
                continue
            k2 = kind + " after the next publish"
        if st.decl is not None and st.decl not in published_decls:
            k2 = "note INV-11 (d): a declaration not published / another text"
            if Dn is not None and st.decl.epoch < Dn.epoch and newest.root is not None:
                k2 = "INV-11 after the cure of R10 (d)"
        if kind == "floor ahead":
            k2 = "note 5-15's mistake: a build's floor ahead of the live policy"
            if (newest.pol.text.epoch, newest.pol.text.seq) >= st.build.floor:
                k2 = "INV-11 after the live policy reached the floor"
        report(k2 + " (%s)" % out.reason, Ctx(node, st, ["newest:   %s -> %r" % (env_name(newest), out)]),
               "the newest genuine envelope is refused: %r" % (out,))
    if not g2:
        return
    for st2 in back:
        STATS["G2 recovered states checked"] += 1
        d2 = derived(st2)
        ctx = Ctx(node, st2)
        if d2.EFF is None or d2.EFF.obj is not newest.pol.text:
            report("G2: took the newest but enforces something else", ctx, fmt_eff(d2.EFF))
        if not o.stolen <= d2.REVOKED:
            report("G2: took the newest but the stolen key is not revoked", ctx, str(sorted(d2.REVOKED)))
        if (d2.E, newest.pol.text.seq) >= st2.build.floor and not d2.MAY_INSTALL:
            report("G2: took the newest but installs nothing", ctx, fmt_state(st2))
        for env in node.pool:
            (st3, out3), _ = step(st2, env)
            if st3 == st2 or derived(st3).EFF == d2.EFF:
                continue
            e3 = derived(st3).EFF
            if (st3.held is not None and st3.held.pol not in signed) or asks_less(e3, d2.EFF):
                report("G2: a recovered device is moved off by %s" % ("S/O" if env.pol.k in o.stolen else "N"),
                       Ctx(node, st2, ["envelope: %s -> %r" % (env_name(env), out3),
                                       "after:    " + fmt_state(st3)]), "moved to %s" % fmt_eff(e3))


def _pair_tag(x, y, back, o):
    voided = {e.pol.text for e in o.voided}
    if (x.held is not None and x.held.pol in voided) or (y.held is not None and y.held.pol in voided):
        return "a voided policy (R10)"
    if made_by_thief(x) or made_by_thief(y):
        return "S, no declaration: R1" if back.decl is None else "S: R13"
    if x.held is not None and y.held is not None and \
            (x.held.pol.epoch, x.held.pol.seq) == (y.held.pol.epoch, y.held.pol.seq):
        return "two texts at one position"
    return "N"


def check_two_processes(node, report, limit=14):
    """INV-12 with two processes on one record, and G1 across the restart that follows: pairs of
    states of one key set (one build, or an older and a newer build during an update), and pairs
    across key sets (an update while a process of the older build still runs). Both save, in either
    order (a process that holds nothing never saves: SAVE follows a take, an "unchanged" or a kept
    declaration); a process restarts under its build: within a key set the file's RANK is the
    higher one, its revocations hold both lists, and what is enforced asks no less than either
    process did, unless a declaration it holds allows it (INV-5). Loosening with a thief's policy
    held by either process (R1, R13), a voided policy (R10) or a part one build cannot verify (R1)
    is a note."""
    def saving(st):
        return st.root is not None or st.held is not None

    by_keyset = collections.defaultdict(list)
    for st in node.reach:
        by_keyset[st.build.keyset].append(st)
    groups = list(by_keyset.values())
    if len(by_keyset) > 1:
        groups.append(None)                          # across key sets
    for sts in groups:
        across = sts is None
        if across:
            sts = sorted(node.reach, key=state_key)
            rng = random.Random(len(sts) * 13 + len(node.o.ops))
        else:
            sts = sorted(sts, key=state_key)
            rng = random.Random(len(sts) * 7 + len(node.o.ops))
        if len(sts) > limit:
            sts = rng.sample(sts, limit)
        for a, c in itertools.permutations(sts, 2):          # a saves first, then c (both orders)
            if across and a.build.keyset == c.build.keyset:
                continue
            if not (saving(a) and saving(c)):
                continue
            first = save(None, a)
            unread = load(c.build, first)
            overwrote_unreadable = unread.root != first.root or unread.held != first.held
            rec = save(first, c)
            for b in {a.build, c.build}:
                back = load(b, rec)
                STATS["two-process pairs" + (" across key sets" if across else "")] += 1
                if not across:
                    if rank(back) < max(rank(a), rank(c)):
                        report("INV-12", Ctx(node, a, ["other process: " + fmt_state(c)]), "RANK went down")
                    if not (derived(a).REVOKED | derived(c).REVOKED) <= derived(back).REVOKED:
                        report("INV-12", Ctx(node, a, ["other process: " + fmt_state(c)]), "revocations shrank")
                dropped = back.root != rec.root or back.held != rec.held
                for x, y in ((a, c), (c, a)):
                    if across and x.build is not b:
                        continue
                    g = g1_violation(back, derived(x).EFF)
                    if not g:
                        continue
                    tag = _pair_tag(x, y, back, node.o)
                    kind = "G1"
                    if across and (dropped or overwrote_unreadable):
                        kind, tag = "note R1", "a part one build cannot verify: R1"
                    elif tag.endswith("(R10)"):
                        kind = "note R10"
                    elif tag.startswith("S, no declaration"):      # INV-12: a thief's epoch-1 policy (R1, R13)
                        kind = "note R1/R13"
                    report("%s (%s), two processes%s [%s]" % (kind, g, " across key sets" if across else "", tag),
                           Ctx(node, x, ["other process: " + fmt_state(y),
                                         "both saved, restart under %s: %s" % (b.name, fmt_state(back))]),
                           "the restarted device enforces %s instead of %s"
                           % (fmt_eff(derived(back).EFF), fmt_eff(derived(x).EFF)))


def stranded_builds(o):
    """Recorded builds that pin none of the signing keys the operator can still use (not revoked by
    the live declaration, private half not destroyed)."""
    D = decl_of(o.live)
    usable = {k for k in o.keys if k not in o.destroyed and not (D and k in D.revoked)}
    return [b.name for b in o.builds if b.keyset in o.recorded and not (b.pins & usable)]


def root_sigs_after_theft(o):
    """Root signatures the incident needed from the theft on: the recovery's declaration and every
    root-signed policy (later relaxations and re-bases are root decisions of their own and are not
    counted)."""
    i = next(i for i, x in enumerate(o.ops) if x.startswith("theft"))
    return sum(1 for x in o.ops[i:] if x.startswith(("recover", "root-policy", "root-raise")))


def inv17_problems(o, env):
    """INV-17 for an envelope the tool signed in operator state `o` (`sign` records the key file's
    key set first): the policy's signer and a new declaration's root key are pinned by every
    recorded key set; a signing key is not revoked by the declaration; a root key signs the policy
    only when no signing key serves. Also, beyond INV-17's text: the root part carried unchanged
    from --prev is signed by a key every recorded key set pins (else INV-17's conclusion, "no
    device of a recorded build is ever in INV-11 (a)", fails)."""
    recs = o.recorded | {keyfile_build(o).keyset}

    def pinned_by_all(k):
        return all(k in (roots if is_root_number(k) else pins) for pins, roots in recs)
    out = []
    k = env.pol.k
    D = decl_of(env)
    revoked = D.revoked if D else frozenset()
    if not pinned_by_all(k):
        out.append(("INV-17", "the policy's signer %d is not pinned by every recorded key set" % k))
    if not is_root_number(k) and k in revoked:
        out.append(("INV-17", "the policy's signer %d is revoked by its declaration" % k))
    if is_root_number(k) and any(all(s in pins for pins, _ in recs) for s in o.keys if s not in revoked):
        out.append(("INV-17", "a root key signs the policy while signing key serves"))
    if env.root is not None and not pinned_by_all(env.root.k):
        new = o.live.root is None or env.root.text is not o.live.root.text
        out.append(("INV-17" if new else "INV-17 (the carried root part)",
                    "root part by %d, not pinned by every recorded key set %s"
                    % (env.root.k, sorted(sorted(r) for _, r in recs))))
    return out


def runbook_refusal(o, name, refusal):
    """A refusal of a runbook step that the runbook does not expect, as a report kind, or None."""
    if name in MISTAKES:
        return None
    if refusal.startswith(("X4", "SERVER")):
        return "G4/G6: the tool refuses a runbook step (%s)" % name
    if refusal.startswith("signer") and name == "recover":
        return "G2/G4: the recovery cannot be signed (%s)" % name
    if refusal.startswith("X9") and stranded_builds(o):
        return "G4: X9 refuses the root-signed policy 5-8 asks for (%s)" % name
    D = decl_of(o.live)
    usable = [k for k in o.keys if k not in o.destroyed and not (D and k in D.revoked)]
    if refusal.startswith("X12") and not name.startswith(("root-", "decl-", "new-epoch")) \
            and o.recorded <= {keyfile_build(o).keyset} and usable:
        return "G2/G4: no signing key may sign although the key file's is the only recorded key set (%s)" % name
    if refusal.startswith("floor") and name in ("build", "build-newkeys", "build-next", "build-newroot"):
        return "G4 (runbook %s): floor --live refuses the next build" % {
            "build": "5-15", "build-newkeys": "5-8 step 7", "build-next": "5-9 step 2", "build-newroot": "5-10"}[name]
    return None


def dfs(root_o, steps_at, depth, report, budget, *, pairs_depth=2, tool_log=None, kind="INV-11",
        iterative=True):
    """Over operator histories (each level one more step). Per node: the device closure (every
    transition and state checked once), the liveness probe of the live envelope (G2 after a
    recovery), and (up to pairs_depth) pairs of processes. With `iterative`, iterative deepening: a
    level is complete or partly covered when the budget runs out; without it one depth-first pass.
    Returns (CPU s, deepest complete level, histories of the first incomplete level)."""
    t0 = time.process_time()
    root = expand(None, root_o, report)
    check_liveness(root, report, False, kind)
    complete, partial = 0, 0
    for limit in (range(1, depth + 1) if iterative else (depth,)):
        partial = 0
        stack = [(root, 0)]
        out_of_time = False
        while stack:
            if time.process_time() - t0 > budget:
                out_of_time = True
                break
            node, d = stack.pop()
            if d >= limit:
                continue
            for name in steps_at(node.o, d):
                o2, env, refusal = apply_step(node.o, name)
                last = d + 1 == limit or not iterative
                if refusal is not None and last:
                    if tool_log is not None:
                        code = refusal.split(" ")[0]
                        if code in ("X4", "SERVER"):
                            code += " " + refusal.split(" ")[1]
                        tool_log[(name, code)] += 1
                    rk = runbook_refusal(node.o, name, refusal)
                    if rk:
                        report(rk, [" | ".join(node.o.ops) + " | " + name,
                                    "recorded builds that pin no usable signing key: %s" % stranded_builds(node.o)],
                               refusal)
                if o2 is None:
                    continue
                if env is not None and last:
                    for kind_, why in inv17_problems(node.o, env):
                        report(kind_, [" | ".join(o2.ops)], why)
                child = expand(node, o2, report)
                if last:
                    partial += 1
                    STATS["operator histories"] += 1
                    check_liveness(child, report, o2.phase == "recovered", kind)
                    if d + 1 <= pairs_depth:
                        check_two_processes(child, report)
                    if o2.phase == "recovered":
                        key = "root signatures an incident needed (recovery + root-signed policies), max"
                        STATS[key] = max(STATS[key], root_sigs_after_theft(o2))
                if not last or not iterative:
                    stack.append((child, d + 1))
        if out_of_time:
            return time.process_time() - t0, (complete if iterative else -1), partial
        complete = limit
    return time.process_time() - t0, complete, 0


HONEST_ALL = ["raise:def", "raise:none", "raise:early", "raise:late", "raise-w:def", "raise-a:def",
              "raise-p:def", "latest", "add-play", "add-play+1", "add-ios", "stop", "relax:min",
              "relax:late", "relax:drop", "relax:latest", "new-epoch", "rotate", "revoke-retired",
              "wait", "build", "build-newkeys", "build-next", "retire"]
HONEST_CORE = ["raise:def", "raise:none", "raise-a:def", "add-play", "relax:min", "relax:late",
               "new-epoch", "wait", "build"]
THEFT_PRE = ["raise:def", "relax:min", "new-epoch", "add-play", "build"]
THEFT_UNAWARE = ["raise:def", "new-epoch", "build", "build-thief-floor"]
THEFT_POST = ["raise:def", "raise:none", "latest", "add-play", "relax:min", "wait", "build",
              "build-newkeys", "new-epoch", "stop", "root-raise", "root-policy", "retire"]
THEFT_POST2 = ["raise:def", "latest", "relax:min", "build"]


def theft_steps(o, d):
    """Histories around a theft: up to two steps before (d < 2), the theft, up to one step while the
    operator does not know, the recovery of 5-8, up to two steps after it (the second from a
    shorter menu)."""
    if o.phase == "normal":
        return (THEFT_PRE if d < 2 else []) + ["theft:cur", "theft:other"]
    i = next(i for i, x in enumerate(o.ops) if x.startswith("theft"))
    if o.phase == "stolen":
        return ["recover"] + (THEFT_UNAWARE if len(o.ops) - i - 1 == 0 else [])
    j = next(i for i, x in enumerate(o.ops) if x.startswith("recover"))
    after = len(o.ops) - j - 1
    return THEFT_POST if after == 0 else THEFT_POST2 if after == 1 else []


def rotation_steps(o, d):
    """5-9's planned rotation with a theft in it: sign with key 2, destroy key 1, the following
    build (2, 3), a raise; the theft; the recovery; up to three steps after it (root-signed policies
    while no signing key serves, a build with new keys, deleting the old records)."""
    if o.phase == "normal":
        return (["rotate", "destroy-retired", "build-next", "raise:def"] if d < 4 else []) + \
            ["theft:cur", "theft:other"]
    if o.phase == "stolen":
        return ["recover"]
    j = next(i for i, x in enumerate(o.ops) if x.startswith("recover"))
    return ["raise:def", "root-raise", "latest", "root-policy", "build-newkeys", "retire", "wait"] \
        if len(o.ops) - j - 1 < 3 else []


def both_steps(o, d):
    """5-8's last case: both signing keys stolen; revoke both, --root-signs-policy; a build with new
    keys; deleting the old record; then the signing key signs again."""
    if o.phase == "normal":
        return (["raise:def", "new-epoch"] if d < 1 else []) + ["theft:both"]
    if o.phase == "stolen":
        return ["recover"]
    j = next(i for i, x in enumerate(o.ops) if x.startswith("recover"))
    return ["root-policy", "root-raise", "build-newkeys", "retire", "raise:def", "latest"] \
        if len(o.ops) - j - 1 < 3 else []


def root_lost_steps(o, d):
    """5-10: a root key lost, after declarations by either root key (the rehearsal of 5-14 signs
    epoch 2 with 101 and epoch 3 with 102); then the runbook's next build (remaining, 103), a
    re-base by the remaining key, normal steps, deleting the old record, and two mistakes (a
    declaration by 103 too early, one that revokes the lost root's number)."""
    if not o.lost_roots:
        return (["raise:def", "new-epoch", "new-epoch:102"] if d < 2 else []) + ["lose-root:101", "lose-root:102"]
    return ["build-newroot", "new-epoch", "raise:def", "latest", "decl-new-root", "revoke-lost-root", "retire"]


def root_lost_theft_steps(o, d):
    """A root key lost, then a theft and the recovery of 5-8 with the remaining root key (G2 with
    one root key)."""
    if o.phase == "normal" and not o.lost_roots:
        return (["new-epoch", "new-epoch:102"] if d < 1 else []) + ["lose-root:101", "lose-root:102"]
    if o.phase == "normal":
        return (["build-newroot", "new-epoch"] if d < 3 else []) + ["theft:cur"]
    if o.phase == "stolen":
        return ["recover"]
    j = next(i for i, x in enumerate(o.ops) if x.startswith("recover"))
    return ["raise:def", "build-newroot", "build-newkeys", "retire", "latest"] if len(o.ops) - j - 1 < 2 else []


SEARCHES = [
    # name, start, steps, depth, CPU seconds (at the default --budget 350), pairs depth, liveness kind, iterative
    ("honest: every runbook step", lambda: start(False), lambda o, d: HONEST_ALL, 5, 70, 3, "INV-11", True),
    ("honest: android_play from seq 1", lambda: start(True), lambda o, d: HONEST_ALL, 4, 18, 3, "INV-11", True),
    ("honest: core steps, deeper", lambda: start(False), lambda o, d: HONEST_CORE, 6, 60, 3, "INV-11", True),
    ("mistake: a declaration signed, not published; a lost archive", lambda: start(False),
     lambda o, d: ["leak", "raise:def", "new-epoch", "relax:min", "wait", "reuse-epoch"], 4, 10, 2, "INV-11", True),
    ("mistakes: a stale --prev declaration; a signed policy not published", lambda: start(False),
     lambda o, d: ["raise:def", "stale-decl", "leak-policy", "new-epoch", "wait", "latest"], 4, 8, 2, "INV-11", True),
    ("mistake: a signed policy voided (N has the file)", lambda: start(False),
     lambda o, d: ["leak-policy", "void", "raise:def", "latest", "new-epoch", "wait"], 5, 12, 2, "INV-11", True),
    ("mistake: a build's floor ahead of the live policy", lambda: start(False),
     lambda o, d: ["build-ahead", "raise:def", "new-epoch", "wait"], 3, 3, 1, "floor ahead", True),
    ("theft (S, then O) and the recovery of 5-8 (G2)", lambda: start(False), theft_steps, 7, 140, 7, "INV-11", False),
    ("theft with the Android pair from seq 1 (G2)", lambda: start(True), theft_steps, 6, 40, 6, "INV-11", False),
    ("theft during 5-9's rotation (G2)", lambda: start(False), rotation_steps, 8, 25, 8, "INV-11", False),
    ("both signing keys stolen (G2, 5-8's last case)", lambda: start(False), both_steps, 6, 12, 6, "INV-11", False),
    ("a root key lost (5-10) and two mistakes", lambda: start(False), root_lost_steps, 5, 15, 3, "INV-11", True),
    ("a root key lost, then a theft (G2 with one root key)", lambda: start(False), root_lost_theft_steps, 7, 12, 7,
     "INV-11", False),
]


RANDOM_MENU = ["raise:def", "raise:none", "raise:early", "raise:late", "raise-a:def", "raise-p:def", "latest",
               "add-play", "add-ios", "stop", "relax:min", "relax:late", "relax:drop", "new-epoch", "new-epoch:102",
               "wait", "wait", "build", "rotate", "build-next", "revoke-retired", "build-newkeys", "leak-policy",
               "void", "theft:cur", "theft:other", "recover", "recover", "build-thief-floor", "root-raise",
               "root-policy", "retire", "lose-root:101", "lose-root:102", "build-newroot"]


def random_histories(report, budget, length=10, seed=20261014, tool_log=None):
    """Random operator histories of `length` steps from RANDOM_MENU (a theft is followed by its
    recovery within three steps), each node with its device closure, the liveness probe (G2 after
    a recovery) and, every third step, pairs of processes. Returns (CPU s, histories, nodes)."""
    t0 = time.process_time()
    rng = random.Random(seed)
    histories = nodes = 0
    while time.process_time() - t0 < budget:
        o = start(rng.random() < 0.3)
        node = expand(None, o, report)
        since_theft = None
        for i in range(length):
            menu = RANDOM_MENU if since_theft is None or since_theft < 3 else ["recover"]
            for _ in range(12):
                name = rng.choice(menu)
                o2, env, refusal = apply_step(node.o, name)
                if refusal is not None:
                    if tool_log is not None:
                        tool_log[(name, refusal.split(" ")[0])] += 1
                    rk = runbook_refusal(node.o, name, refusal)
                    if rk:
                        report(rk, [" | ".join(node.o.ops) + " | " + name], refusal)
                if o2 is not None:
                    break
            else:
                break
            if env is not None:
                for kind_, why in inv17_problems(node.o, env):
                    report(kind_, [" | ".join(o2.ops)], why)
            node = expand(node, o2, report)
            nodes += 1
            if o2.phase == "stolen":
                since_theft = 0 if since_theft is None else since_theft + 1
            elif o2.phase == "recovered":
                since_theft = None
            check_liveness(node, report, o2.phase == "recovered", "INV-11")
            if i % 3 == 2:
                check_two_processes(node, report, limit=10)
        histories += 1
        STATS["random long histories"] += 1
    return time.process_time() - t0, histories, nodes


class FuzzCtx:
    """A trace of the fuzzer: the build and the events so far."""

    def __init__(self, build, events, extra=()):
        self.build, self.events, self.extra = build, list(events), extra

    def size(self):
        return (0, len(self.events))

    def __call__(self):
        return ["device:   fresh install of %s -> %s" % (self.build.name, " -> ".join(self.events))] + \
            [x() if callable(x) else x for x in self.extra]


def fuzz(report, budget, seed=7):
    """Arbitrary signed envelopes, not only the ones a correct tool makes: declarations of epochs
    2-4 (two texts each, revocations cumulative by epoch, random bases, some breaking B3), each
    signed by 101, 102 or 103; policies of epochs 1-4 and seq near LOW with random entries (some
    breaking C3), signers 1, 2, 3, 101, valid or not, a root part or none; replays; builds with
    floors 1:0, 1:2, 2:1, 3:2, another signing key set and another root key set; restarts and
    updates under every build (LOAD part by part). Checks the app-side rules that do not depend on
    the operator: INV-4 .. INV-10, INV-12's LOAD(SAVE(S)) = S and restarts, 2-1, 2-5, G1 as
    INV-5 (b), and appendix A (accept and load) against sections 1-6 and 2."""
    t0 = time.process_time()
    rng = random.Random(seed)
    builds = [Build("f1:0", {1, 2}, ROOTS, (1, 0)), Build("f1:2", {1, 2}, ROOTS, (1, 2)),
              Build("f2:1", {1, 2}, ROOTS, (2, 1)), Build("f3:2", {1, 2}, ROOTS, (3, 2)),
              Build("g1:0", {2, 3}, ROOTS, (1, 0)), Build("r2:1", {1, 2}, {ROOT2, 103}, (2, 1))]

    def r_entry():
        mn = rng.randint(0, 3)
        latest = rng.randint(mn, 3)
        if rng.random() < 0.4 and mn > 0:
            return Entry(mn, latest, rng.choice((10, 20)), rng.randint(0, mn - 1))
        if rng.random() < 0.03:
            return Entry(mn, latest, 10, mn)                          # breaks grace_min < min
        return Entry(mn, latest)

    def r_ents():
        names = [c for c in NAMES if rng.random() < 0.6] or ["windows"]
        return {c: r_entry() for c in names}

    decls = []
    rev = set()
    for e in (2, 3, 4):
        rev = rev | ({1} if rng.random() < 0.5 else set())
        for variant in ("", "other text"):
            base = r_ents() if rng.random() < 0.95 else {"windows": Entry(3, 1)}   # some break B3
            decls.append(declaration(e, frozenset(rev), rng.randint(0, 3), base, variant=variant,
                                     label="FD%d%s(rev%s)" % (e, "'" if variant else "", sorted(rev))))
    decls.append(declaration(1, frozenset(), 0, r_ents(), label="FD1(epoch 1: RootFormat)"))
    decls.append(declaration(3, frozenset({1}), 1, {"windows": Entry(2, 3, 10, 2)}, variant="bad base",
                             label="FD3(base grace_min = min: RootFormat)"))
    decls.append(declaration(4, frozenset({1, ROOT2}), 1, r_ents(), label="FD4(revokes 102: RootRevokesRoot)"))

    def raised(ents):
        """Entries that ask at least as much as `ents` (a plausible next policy), or random ones."""
        if not ents or rng.random() < 0.3:
            return r_ents()
        out = {}
        for c, e in ents.items():
            if rng.random() < 0.5:
                out[c] = e
            else:
                mn = min(3, e.min + 1)
                out[c] = Entry(mn, max(e.latest, mn), *((rng.choice((10, 20)), e.min) if rng.random() < 0.5 and
                                                         e.min < mn else (None, None)))
        if rng.random() < 0.2:
            c = rng.choice(NAMES)
            out.setdefault(c, r_entry())
        return out

    n = 0
    while time.process_time() - t0 < budget:
        b = rng.choice(builds)
        st = fresh(b)
        events = []
        sent = []
        for i in range(10):
            if rng.random() < 0.12:
                b2 = rng.choice(builds)
                st2 = load(b2, record_of(st))
                ev = "%s %s" % ("restart under" if b2.keyset == st.build.keyset else "update to", b2.name)
                ctx = FuzzCtx(b, events + [ev], ["after:    " + fmt_state(st2)])
                if b2.keyset == st.build.keyset and (st2.held != st.held or st2.root != st.root):
                    report("fuzz INV-12", ctx, "a restart under a build of the same keys lost a part")
                g = g1_violation(st2, derived(st).EFF)
                if g and b2.keyset == st.build.keyset:
                    report("fuzz G1 (%s) at a restart" % g, ctx, "a restart loosened what is enforced")
                diff = appendix_a_load_agrees(b2, record_of(st))
                STATS["fuzz: appendix A's load compared with 1-6"] += 1
                if diff:
                    report("fuzz: appendix A's load differs from 1-6", ctx, diff)
            else:
                if sent and rng.random() < 0.2:
                    env = rng.choice(sent)                                   # a replay
                    root, P = env.root, env.pol.text
                else:
                    root = None
                    if rng.random() < 0.7:
                        rk = rng.choices((101, 102, 103, 2), (45, 40, 10, 5))[0]
                        root = Part(rk, rng.choice(decls), valid=rng.random() < 0.95)
                    d = derived(st)
                    pe = root.text.epoch if root is not None and rng.random() < 0.85 else rng.choice((1, d.E, d.E + 1))
                    basis = d.BASIS.ents if d.BASIS is not None else (d.EFF.ents if d.EFF else {})
                    if root is not None and root.text.epoch > d.E:
                        basis = root.text.base
                    P = policy(pe, rng.choice((d.LOW, d.LOW + 1, d.LOW + 1, d.LOW + 2, 1)) or 1,
                               rng.randint(0, 3), raised(basis), notes="fuzz")
                    env = Env(Part(rng.choice((1, 2, 2, 3, 101)), P, valid=rng.random() < 0.95), root)
                    sent.append(env)
                st2, out = accept(st, env)
                ev = "%s%s -> %r" % ("" if root is None else "%r + " % (root,), P, out)
                ctx = FuzzCtx(b, events + [ev], ["after:    " + fmt_state(st2)])
                check_transition(st, env, st2, out, lambda kind, c, detail: report("fuzz " + kind, c, detail), ctx)
                diff = appendix_a_agrees(st, env)
                STATS["fuzz: appendix A compared with section 2"] += 1
                if diff:
                    report("fuzz: appendix A differs from section 2", ctx, diff)
            check_state(st2, lambda kind, c, detail: report("fuzz " + kind, c, detail), FuzzCtx(b, events + [ev]))
            events.append(ev)
            st = st2
            n += 1
    STATS["fuzz: steps"] += n
    return time.process_time() - t0, n


def run_search(budget, tool_log, verbose=True, only=None, depth_cap=None):
    """Runs SEARCHES (CPU seconds scaled to budget/350); one Report per search."""
    reports = []
    for name, mk, steps, depth, secs, pairs, kind, iterative in SEARCHES:
        if only is not None and only not in name:
            continue
        report = Report()
        _ACCEPT.clear()                     # the memo tables only speed up one search; each starts empty
        _DERIVED.clear()
        before = STATS["operator histories"]
        d = depth if depth_cap is None else min(depth, depth_cap)
        STATS.pop("root signatures an incident needed (recovery + root-signed policies), max", None)
        used, complete, partial = dfs(mk(), steps, d, report, secs * budget / 350.0, pairs_depth=pairs,
                                      tool_log=tool_log, kind=kind, iterative=iterative)
        if verbose:
            if complete < 0:
                where = "budget hit before the depth-first pass ended"
            else:
                where = "complete to depth %d%s" % (complete, "" if complete == d else
                                                    " (+%d at depth %d)" % (partial, complete + 1))
            sigs = STATS.get("root signatures an incident needed (recovery + root-signed policies), max")
            print("   %-66s %7d histories %6.1f s  %s%s" % (name, STATS["operator histories"] - before, used, where,
                                                         "" if sigs is None else "; root signatures per incident <= %d"
                                                         % sigs))
        reports.append((name, report))
    name = "fuzz: arbitrary signed envelopes, app-side rules"
    if (only is None or only in name) and depth_cap is None:
        report = Report()
        used, n = fuzz(report, 20.0 * budget / 350.0)
        if verbose:
            print("   %-66s %7d steps     %6.1f s" % (name, n, used))
        reports.append((name, report))
    name = "random histories of 10 steps (runbook, theft and recovery, builds, root keys, mistakes)"
    if (only is None or only in name) and depth_cap is None:
        report = Report()
        used, n, nodes = random_histories(report, 45.0 * budget / 350.0, tool_log=tool_log)
        if verbose:
            print("   %-66s %7d histories %6.1f s  (%d operator steps)" % (name, n, used, nodes))
        reports.append((name, report))
    return reports


# ---- pure-function checks -------------------------------------------------------------------------

def entry_domain(versions=(0, 1, 2, 3), deadlines=(None, 10, 20), latests=None):
    out = []
    for T in deadlines:
        for mn in versions:
            for gm in ([None] if T is None else [g for g in versions if g < mn]):
                for lt in ([mn] if latests is None else [x for x in latests if x >= mn]):
                    out.append(Entry(mn, lt, T, gm))
    return out


def check_pure(report, budget=30.0):
    """INV-1 on the 256 pairs of covers.json; INV-2 exhaustively for the Android family and for a
    single entry (RELAXES against the brute-force semantics, plus the verdicts of 3-2/3-3), then on
    random three-entry policies; INV-3 exhaustively on those domains; INV-14 on the derivations;
    INV-16 on hbbs-conf."""
    t0 = time.process_time()
    ents16 = entry_domain()
    for n, o in itertools.product(ents16, ents16):
        pointwise = all(threshold(n, t) >= threshold(o, t) for t in cut_times({"n": n, "o": o}))
        STATS["INV-1 entry pairs"] += 1
        if covers(n, o) != pointwise:
            report("INV-1", ["n=%s o=%s" % (fmt_entry(n), fmt_entry(o))], "COVERS %s, pointwise %s" % (covers(n, o), pointwise))
    small = entry_domain(versions=(0, 1, 2), latests=(0, 1, 2))
    fam = []
    for a in [None] + small:
        for p in [None] + small:
            d = {}
            if a is not None:
                d["android"] = a
            if p is not None:
                d["android_play"] = p
            fam.append(d)
    singles = [{}] + [{"windows": e} for e in small]
    succ = {}
    for dom in (fam, singles):
        for i, B in enumerate(dom):
            succ_i = []
            for j, P in enumerate(dom):
                STATS["INV-2 policy pairs"] += 1
                r = relaxes(B, P)
                if r != asks_less_somewhere(B, P):
                    report("INV-2", ["B=%s P=%s" % (fmt_ents(B), fmt_ents(P))], "RELAXES %s, brute force %s" % (r, not r))
                if not r:
                    succ_i.append(j)
                    verdicts_monotone(B, P, report)
            succ[(id(dom), i)] = succ_i
    for dom in (fam, singles):
        for i, A in enumerate(dom):
            for j in succ[(id(dom), i)]:
                for k in succ[(id(dom), j)]:
                    STATS["INV-3 chains"] += 1
                    if relaxes(A, dom[k]):
                        report("INV-3", ["A=%s B=%s C=%s" % (fmt_ents(A), fmt_ents(dom[j]), fmt_ents(dom[k]))],
                               "not transitive")
    rng = random.Random(7)
    three = [dict(w, **a) for w in singles for a in fam]
    while time.process_time() - t0 < budget * 0.6:
        B, P = rng.choice(three), rng.choice(three)
        STATS["INV-2 random 3-entry pairs"] += 1
        if relaxes(B, P) != asks_less_somewhere(B, P):
            report("INV-2", ["B=%s P=%s" % (fmt_ents(B), fmt_ents(P))], "three entries")
    for o in entry_domain(latests=(3,)):
        for now in (0, 5, 9, 10, 15, 25):
            for B in range(o.min, 4):
                for grace in ("def", "none", 2, 8, 12, 30):
                    g = grace if grace in ("def", "none") else now + grace
                    try:
                        n = derive_existing(o, B, now, g)
                    except ToolRefusal:
                        continue
                    STATS["INV-14 derivations"] += 1
                    where = ["o=%s now=%d asked=%d grace=%s -> %s" % (fmt_entry(o), now, B, grace, fmt_entry(n))]
                    if not covers(n, o):
                        report("INV-14", where, "the derived entry does not cover the old one")
                    if B > o.min:
                        end = n.gu if n.gu is not None else now
                        if o.gu is not None and o.gu > now and n.gu is not None and n.gu > o.gu:
                            STATS["INV-14 derivations c with a later deadline"] += 1
                            if any(threshold(n, t) != o.min for t in range(now, end)):
                                report("INV-14", where, "c, later: not the pending min until its own deadline")
                        elif any(threshold(n, t) != threshold(o, t) for t in range(now, end)):
                            report("INV-14", where, "blocks something new before its own deadline")
    for x in entry_domain(latests=(3,)):                         # derivation f (a new Android entry)
        for now in (0, 5, 15, 25):
            for B in range(x.min, 4):
                try:
                    n = derive_new("android_play", B, 3, now, "def", {"android": x})
                except ToolRefusal:
                    continue
                STATS["INV-14 derivations f"] += 1
                if relaxes_ref({"android": x}, {"android": x, "android_play": n}):
                    report("INV-14", ["x=%s now=%d B=%d -> %s" % (fmt_entry(x), now, B, fmt_entry(n))],
                           "derivation f gives an Android entry that relaxes its sibling")
    check_inv16(report)
    return time.process_time() - t0


def check_inv16(report):
    """INV-16: for every present file (MIN_p, FROM_p) (or none), new MIN, derived FROM and now, a
    written file with a higher MIN refuses, at every t >= now, every version the present file
    refuses at t."""
    T = range(0, 7)
    for cur_min, cur_from, min_n, from_basis, now in itertools.product((None, 1, 2), (None,) + tuple(T),
                                                                       (1, 2, 3), T, T):
        cur = None if cur_min is None else (cur_min, cur_from)
        dec = hbbs_conf(min_n, from_basis, cur, now)
        if dec[0] != "write":
            continue
        _, mn, frm = dec
        STATS["INV-16 hbbs-conf files written"] += 1
        if cur is not None and cur_from is not None and frm < cur_from:
            report("INV-16", ["present %s new MIN %d FROM %d now %d" % (cur, min_n, from_basis, now)],
                   "FROM went back")
        if cur is None or mn <= cur_min:
            continue
        for t in range(now, 10):
            if any(hbbs_refuses(cur, t, v) and not hbbs_refuses((mn, frm), t, v) for v in range(4)):
                report("INV-16", ["present %s new MIN %d FROM %d now %d -> wrote (%d, %d)"
                                  % (cur, min_n, from_basis, now, mn, frm)],
                       "the written file refuses less than the present one at t=%d" % t)
                break


def verdicts_monotone(B, P, report):
    """With not RELAXES(B, P), at every version and time: what the app blocks under B it blocks
    under P, what is 'ok' under P is 'ok' under B, and a controller the PC refuses under B it refuses
    under P (3-2, 3-3)."""
    eB, eP = Eff(B, 0, 1, 1, "policy", None), Eff(P, 0, 1, 1, "policy", None)
    for t in cut_times(B, P):
        for v in range(0, 4):
            for c in B:
                sb, sp = app_state_of(eB, c, v, t), app_state_of(eP, c, v, t)
                if (sb == "required" and sp != "required") or (sp == "ok" and sb != "ok"):
                    report("INV-2-verdicts", ["B=%s P=%s %s v%d t%d" % (fmt_ents(B), fmt_ents(P), c, v, t)],
                           "app %s -> %s" % (sb, sp))
            for fam in ("windows", "android"):
                rb, rp = controller_rule_of(eB, fam, t), controller_rule_of(eP, fam, t)
                if rb is not None and v < rb.min_number and not (rp is not None and v < rp.min_number):
                    report("INV-2-verdicts", ["B=%s P=%s %s v%d t%d" % (fmt_ents(B), fmt_ents(P), fam, v, t)],
                           "the PC lets a controller in")


# ---- main -----------------------------------------------------------------------------------------

def reset_caches():
    _ACCEPT.clear()
    _DERIVED.clear()
    _SEEN_STATES.clear()
    _SEEN_RESTARTS.clear()
    _TEXT_ERR.clear()


def case_ok(c, hist):
    try:
        return not case_result(c, hist)[4]
    except Exception:                           # noqa: BLE001 - a broken variant may crash a case
        return False


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("what", nargs="?", default="all", choices=("cases", "covers", "pure", "search", "variants", "all"))
    ap.add_argument("--budget", type=float, default=350.0, help="CPU seconds for the history searches")
    ap.add_argument("--quick", action="store_true", help="a tenth of every budget")
    ap.add_argument("--json", action="store_true", help="with `cases`: one JSON line per case")
    ap.add_argument("--only", default=None, help="only the searches whose name contains this")
    args = ap.parse_args(argv)
    sys.stdout.reconfigure(line_buffering=True)
    scale = 0.1 if args.quick else 1.0
    t0 = time.process_time()
    if args.what == "cases" and args.json:
        cases_json()
        return
    if args.what == "covers":
        covers_json()
        return
    if args.what in ("cases", "all"):
        print("== spec 7-4 cases")
        run_cases(verbose=args.what == "cases")
    if args.what in ("pure", "all"):
        rep = Report()
        used = check_pure(rep, 20.0 * scale)
        rep.show("pure functions (INV-1, INV-2 with the verdicts, INV-3, INV-14, INV-16), %.0f s" % used)
    violations = collections.Counter()
    if args.what in ("search", "all"):
        print("\n== history searches, the rules as written")
        tool_log = collections.Counter()
        for name, report in run_search(args.budget * scale, tool_log, only=args.only):
            report.show(name)
            for k, v in report.count.items():
                if not k.startswith("note"):
                    violations[k] += v
        print("\n== the tool's refusals met by the operator's steps (step, code): count")
        for (stepname, code), n in sorted(tool_log.items()):
            print("   %-16s %-40s %d" % (stepname, code, n))
        print("\n== coverage of the history searches")
        for k, v in sorted(STATS.items()):
            if not k.startswith("INV-"):
                print("   %-74s %d" % (k, v))
        print("   %-74s %d" % ("distinct device states", len(_SEEN_STATES)))
        print("   %-74s %d / %d" % ("distinct policy texts / declaration texts", len(_POLICIES), len(_DECLS)))
        print("\n== violations over every search: %s" % (", ".join("%s x%d" % kv for kv in sorted(violations.items()))
                                                        or "none"))
    if args.what in ("variants", "all"):
        print("\n== broken variants (each must be caught)")

        def short_run():
            counts = collections.Counter()
            cs, hist = fixed_cases()
            bad = sum(1 for c in cs if not case_ok(c, hist)) + sum(1 for c in store_cases() if not c[2])
            rep = Report()
            check_pure(rep, 2.0 * scale + 0.5)
            counts.update(rep.count)
            for _, r in run_search(5.0 * scale + 1.0, collections.Counter(), verbose=False, depth_cap=3):
                counts.update(r.count)
            return bad, counts
        reset_caches()
        V.clear()
        _, base_counts = short_run()
        for name, what in VARIANTS.items():
            reset_caches()
            V.clear()
            V.add(name)
            bad_cases, counts = short_run()
            caught = ["%s x%d" % kv for kv in sorted(counts.items())
                      if kv[0] not in base_counts and not kv[0].startswith("note")]
            if bad_cases:
                caught.insert(0, "7-4/7-6 cases x%d" % bad_cases)
            print("   %-20s %-62s caught by: %s" % (name, what, ", ".join(caught) or "NOTHING"))
        V.clear()
        reset_caches()
    print("\n== totals over every part run")
    for k, v in sorted(STATS.items()):
        print("   %-74s %d" % (k, v))
    print("   %-74s %.0f s" % ("total CPU", time.process_time() - t0))


if __name__ == "__main__":
    main()

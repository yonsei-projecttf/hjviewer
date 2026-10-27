#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Checks the public keys an HJViewer build pins (design v3 6-4; docs/ci.md, "계정 기능의 공개키").

client-build.yml runs it after the checkout, with the build variables in the environment as cargo
sees them:

    python3 -I scripts/check-build-keys.py [--version-file client/version.env]
                                           [--update-keys client/update-keys.env]

    HJV_LOGIN_SEAL_PUBKEY  standard base64 of the 32-byte X25519 key `hjv-api keygen seal` prints
    HJV_TRUST_PUBKEYS      <key id>:<base64>,<key id>:<base64>: the two Ed25519 trust-list keys
    HJV_SERVER_KEY         hbbs's public key, whose fingerprint is shown (the workflow's "Check
                           server variables" step checks that it is set)
    CALLED                 non-empty when the private build repository called the workflow
    CHECK_ONLY             "true" for a check build, which hands nothing out

A called run of client/version.env 1.6.0 or later fails when HJV_LOGIN_SEAL_PUBKEY or
HJV_TRUST_PUBKEYS is missing (not with CHECK_ONLY), when a key given is malformed or weak, and when
a key is a public test key, whose private key anyone has: keys made from a seed of one repeated byte
or of 32 consecutive byte values, the RFC 8032 and RFC 7748 examples, the keys in
scripts/public-test-keys.txt, and the Ed25519 and X25519 keys of every 32-byte value that the files
git tracks here write in base64 or hex (test vectors: their private keys are in this public
repository). In other runs these are warnings: without the account keys an app is built like
1.5.1, with no login. The SHA-256 fingerprints of the pinned keys go to the run summary
($GITHUB_STEP_SUMMARY, else stdout), for comparing with `hjv-api selfcheck` before the release tag
(design v3 11 step 8).

The update keys of the minimum-version work come from the file client/update-keys.env of the
checked-out source and from nowhere else (minimum version design 6-4): a secret or a variable of
the repository that builds the apps cannot change whom they believe about versions.

    HJV_UPDATE_PUBKEY=<key id>:<base64>[,<key id>:<base64>]   the signing keys, two at most
    HJV_UPDATE_ROOT_PUBKEY=<key id>:<base64>                  the root key
    HJV_POLICY_FLOOR=<epoch>:<seq>                            the oldest policy the build takes

Every run fails when that file is malformed: another form than NAME=value lines, '#' comments
and empty lines, a key that the apps would refuse (they drop the whole set over one such key), a
key or key id twice, one key line without the other, a floor that is not <epoch>:<seq>, or one
of the three names set to another value in the environment. A called run of 1.6.0 or later also
fails when the file holds a public test key, and when it holds no keys at all (not with
CHECK_ONLY): apps built from it would never check the version policy. In other runs these two
are warnings.

    python3 -I scripts/check-build-keys.py --pin-update-keys [--version-file ...] [--update-keys ...]

checks that file in the same way and hands its three values to the later steps of a job
($GITHUB_ENV), where cargo reads them (option_env!): the "Pin the update keys" step of every
platform job. It prints them when there is no $GITHUB_ENV.

    python3 -I scripts/check-build-keys.py --relay

does the same for HJV_COMMAND_PUBKEY (<key id>:<base64>[,<key id>:<base64>]) in the hjv-relay
job; scripts/package-relay.sh runs it again, strictly, before and after its build; and

    python3 -I scripts/check-build-keys.py pubkey ed25519|x25519 <64 hex digits>

prints the public key of an Ed25519 seed or X25519 secret key, for making test keys.

Standard library only: the GitHub runners' python3 has no crypto package.
"""
import argparse
import base64
import binascii
import hashlib
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TEST_KEYS_FILE = os.path.join(ROOT, "scripts", "public-test-keys.txt")
UPDATE_KEYS_FILE = os.path.join(ROOT, "client", "update-keys.env")
UPDATE_NAMES = ("HJV_UPDATE_PUBKEY", "HJV_UPDATE_ROOT_PUBKEY", "HJV_POLICY_FLOOR")
# the first version with the account features, and with the version policy
ACCOUNT_VERSION = (1, 6, 0)

# --- Edwards25519 and Curve25519 (RFC 8032, RFC 7748), just enough to derive and check keys ---

P = 2**255 - 19
D = -121665 * pow(121666, P - 2, P) % P
SQRT_M1 = pow(2, (P - 1) // 4, P)
_BX = 15112221349535400772501151409588531511454012693041857206046113283949847762202
_BY = 46316835694926478169428394003475163141307993866256225615783033603165251855960
BASE = (_BX, _BY, 1, _BX * _BY % P)
IDENTITY = (0, 1, 1, 0)
# the order of the base point: the size of the subgroup every honest public key is in
L = 2**252 + 27742317777372353535851937790883648493


def _add(p, q):
    """Point addition in extended coordinates (RFC 8032 5.1.4); also doubles."""
    x1, y1, z1, t1 = p
    x2, y2, z2, t2 = q
    a = (y1 - x1) * (y2 - x2) % P
    b = (y1 + x1) * (y2 + x2) % P
    c = 2 * D * t1 * t2 % P
    d = 2 * z1 * z2 % P
    e, f, g, h = b - a, d - c, d + c, b + a
    return (e * f % P, g * h % P, f * g % P, e * h % P)


def _mul(s, p):
    q = IDENTITY
    while s:
        if s & 1:
            q = _add(q, p)
        p = _add(p, p)
        s >>= 1
    return q


def _encode(p):
    x, y, z, _ = p
    zi = pow(z, P - 2, P)
    x, y = x * zi % P, y * zi % P
    return (y | (x & 1) << 255).to_bytes(32, "little")


def _decode(b):
    """The point a 32-byte Ed25519 public key encodes (RFC 8032 5.1.3), or None. Strict: a
    non-canonical encoding (y >= p) is no key here, which no key generator makes."""
    if len(b) != 32:
        return None
    y = int.from_bytes(b, "little")
    sign = y >> 255
    y &= (1 << 255) - 1
    if y >= P:
        return None
    u = (y * y - 1) % P
    v = (D * y * y + 1) % P
    x = u * pow(v, 3, P) * pow(u * pow(v, 7, P), (P - 5) // 8, P) % P
    vxx = v * x * x % P
    if vxx == (-u) % P:
        x = x * SQRT_M1 % P
    elif vxx != u:
        return None
    if x == 0 and sign:
        return None
    if x & 1 != sign:
        x = P - x
    return (x, y, 1, x * y % P)


def ed25519_public(seed):
    h = hashlib.sha512(seed).digest()
    a = int.from_bytes(h[:32], "little")
    a = (a & ((1 << 254) - 8)) | (1 << 254)
    return _encode(_mul(a, BASE))


def ed25519_weak(point):
    """Whether the point has a small order, as ed25519-dalek's VerifyingKey::is_weak says."""
    x, y, z, _ = _mul(8, point)
    return x % P == 0 and (y - z) % P == 0


def ed25519_in_subgroup(point):
    """Whether the point is in the prime-order subgroup. A point outside it has a small-order
    part; no key pair has such a public key, and the apps refuse one where they pin the update
    keys (libsodium's crypto_sign_ed25519_pk_to_curve25519)."""
    x, y, z, _ = _mul(L, point)
    return x % P == 0 and (y - z) % P == 0


def x25519(k, u):
    """RFC 7748 section 5."""
    k = bytearray(k)
    k[0] &= 248
    k[31] &= 127
    k[31] |= 64
    k = int.from_bytes(k, "little")
    x1 = int.from_bytes(u, "little") & ((1 << 255) - 1)
    x2, z2, x3, z3, swap = 1, 0, x1, 1, 0
    for t in range(254, -1, -1):
        bit = (k >> t) & 1
        if swap ^ bit:
            x2, x3, z2, z3 = x3, x2, z3, z2
        swap = bit
        a, b, c, d = x2 + z2, x2 - z2, x3 + z3, x3 - z3
        aa, bb = a * a % P, b * b % P
        e = aa - bb
        da, cb = d * a % P, c * b % P
        x3 = (da + cb) ** 2 % P
        z3 = x1 * (da - cb) ** 2 % P
        x2 = aa * bb % P
        z2 = e * (aa + 121665 * e) % P
    if swap:
        x2, z2 = x3, z3
    return (x2 * pow(z2, P - 2, P) % P).to_bytes(32, "little")


def x25519_public(secret):
    return x25519(secret, (9).to_bytes(32, "little"))


def x25519_weak(u):
    """Whether u (an int below p) has a small order on Curve25519 or its twist: eight times the
    point is the point at infinity. A sealed box to such a key is open to anyone."""
    x, z = u, 1
    for _ in range(3):
        aa, bb = (x + z) ** 2 % P, (x - z) ** 2 % P
        e = aa - bb
        x, z = aa * bb % P, e * (aa + 121665 * e) % P
    return z == 0


# --- the public test keys ---

# Secret keys published as examples: RFC 8032 7.1 TEST 1-3 (Ed25519 seeds) and RFC 7748 6.1
# (Alice's and Bob's X25519 keys). Both kinds of public key are derived from each.
RFC_SECRETS = (
    "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
    "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
    "c5aa8df43f9f837bedb7442f31dcb7b166d38535076f094b85ce3a2e0b4458f7",
    "77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a",
    "5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb",
)
# {tree: {public key: where its private key is published}}
_test_keys = {}

# A 32-byte value written out on its own: base64, standard or URL-safe, with or without its '='
# (43 characters), or hex (64 digits)
_B64_32 = re.compile(rb"(?<![A-Za-z0-9+/=_-])([A-Za-z0-9+/_-]{43})=?(?![A-Za-z0-9+/=_-])")
_HEX_32 = re.compile(rb"(?<![0-9A-Za-z])(?:0[xX])?([0-9a-fA-F]{64})(?![0-9A-Za-z])")


def test_seeds():
    """The secrets tests write down: one repeated byte, 32 consecutive byte values, RFC examples,
    as [(seed, what it is)]."""
    seeds = [(bytes([b]) * 32, f"seed [{b:#04x}; 32]") for b in range(256)]
    seeds += [(bytes((b + i) % 256 for i in range(32)), f"seed {b:02x} {(b + 1) % 256:02x} ..")
              for b in range(256)]
    seeds += [(bytes.fromhex(h), "an RFC 8032 / RFC 7748 example") for h in RFC_SECRETS]
    return seeds


def read_key_file(path):
    """{key: "path:line"} of a list file: one base64 key per line, '#' starts a comment. The list
    must be there: without it the check would pass the keys it names."""
    keys = {}
    try:
        with open(path, encoding="utf-8-sig") as f:
            lines = f.readlines()
    except FileNotFoundError:
        raise SystemExit(f"{path} is missing: the check needs its list of public test keys")
    except UnicodeDecodeError:
        raise SystemExit(f"{path} is not UTF-8 text")
    for n, line in enumerate(lines, 1):
        text = line.split("#", 1)[0].strip()
        if not text:
            continue
        raw = b64_32(text)
        if raw is None:
            raise SystemExit(f"{path}:{n}: not the base64 of a 32-byte key")
        keys.setdefault(raw, f"{os.path.relpath(path, ROOT)}:{n}")
    return keys


def tracked_files(tree):
    """The files git tracks in <tree>; outside a git work tree, the files there."""
    try:
        done = subprocess.run(["git", "-C", tree, "ls-files", "-z"], capture_output=True, check=True)
    except (OSError, subprocess.CalledProcessError):
        found = []
        for top, dirs, names in os.walk(tree):
            dirs[:] = [d for d in dirs if d not in (".git", "build", "target", "upstream")]
            found += [os.path.join(top, n) for n in names]
        return sorted(found)
    return [os.path.join(tree, p.decode(errors="surrogateescape")) for p in done.stdout.split(b"\0") if p]


def tree_values(tree):
    """{32 bytes: "path:line"}: the 32-byte values the files of <tree> write in base64 or hex."""
    values = {}
    for path in tracked_files(tree):
        if os.path.islink(path) or not os.path.isfile(path):
            continue
        with open(path, "rb") as f:
            data = f.read()
        if path.endswith(".patch"):
            # the lines a patch adds (client/patches): "+<value>" as the patched file has it
            data = data.replace(b"\n+", b"\n ")
        rel = os.path.relpath(path, tree)
        found = []
        for m in _B64_32.finditer(data):
            try:
                raw = base64.b64decode(m.group(1).replace(b"-", b"+").replace(b"_", b"/") + b"=", validate=True)
            except binascii.Error:
                continue
            found.append((m.start(), raw))
        found += [(m.start(), bytes.fromhex(m.group(1).decode())) for m in _HEX_32.finditer(data)]
        for start, raw in found:
            if len(raw) == 32 and raw not in values:
                line = data.count(b"\n", 0, start) + 1
                values[raw] = f"{rel}:{line}"
    return values


def public_test_keys(tree=ROOT):
    """{public key: where its private key is published}: the keys of test_seeds(), the list file,
    and, as Ed25519 seed and as X25519 secret key, every 32-byte value of <tree>'s files."""
    if tree not in _test_keys:
        keys = {raw: f"listed in {where}" for raw, where in read_key_file(TEST_KEYS_FILE).items()}
        for seed, what in test_seeds():
            keys.setdefault(ed25519_public(seed), what)
            keys.setdefault(x25519_public(seed), what)
        for raw, where in tree_values(tree).items():
            keys.setdefault(ed25519_public(raw), f"its Ed25519 seed is at {where}")
            keys.setdefault(x25519_public(raw), f"its X25519 secret key is at {where}")
        _test_keys[tree] = keys
    return _test_keys[tree]


# --- parsing the variables ---


def b64_32(text):
    """The 32 bytes of standard base64 text, or None. Canonical only: Rust's base64 crate, which
    the apps and hjv-relay decode with, refuses a last character with stray low bits."""
    if not re.fullmatch(r"[A-Za-z0-9+/]{43}=", text):
        return None
    try:
        raw = base64.b64decode(text, validate=True)
    except binascii.Error:
        return None
    if len(raw) != 32 or base64.b64encode(raw).decode() != text:
        return None
    return raw


def fingerprint(raw):
    return hashlib.sha256(raw).hexdigest()


def check_ed25519(name, kid, raw, subgroup=False):
    """Problems of an Ed25519 public key, as ed25519-dalek and hjv-relay judge it; with
    subgroup, as the apps judge an update key."""
    what = f"{name} key {kid}" if kid is not None else name
    point = _decode(raw)
    if point is None:
        return [f"{what} is not an Ed25519 public key"]
    if ed25519_weak(point):
        return [f"{what} is a weak key (small order): anyone could sign with it"]
    if subgroup and not ed25519_in_subgroup(point):
        return [f"{what} is outside the prime-order subgroup: no key pair has such a public key, "
                "and the apps refuse it"]
    return []


def parse_seal(name, text):
    """(keys, problems): HJV_LOGIN_SEAL_PUBKEY, one X25519 key."""
    raw = b64_32(text)
    if raw is None:
        return [], [f"{name} is not the standard base64 of a 32-byte key (what hjv-api keygen seal prints)"]
    u = int.from_bytes(raw, "little")
    if u >= P:
        return [], [f"{name} is not a canonical X25519 public key"]
    if x25519_weak(u):
        return [], [f"{name} is a weak key (small order): anyone could open the sealed passwords"]
    return [(None, raw)], []


def parse_list(name, text, exactly=None, most=2, subgroup=False):
    """(keys, problems): <key id>:<base64>[,...], ids 1-65535, distinct ids and keys."""
    keys, problems = [], []
    entries = [e.strip() for e in text.split(",")]
    if any(not e for e in entries):
        return [], [f"{name} has an empty entry: <key id>:<base64>,<key id>:<base64>"]
    for entry in entries:
        kid, sep, key = entry.partition(":")
        kid, key = kid.strip(), key.strip()
        if not sep or not re.fullmatch(r"[1-9][0-9]{0,4}", kid) or int(kid) > 65535:
            problems.append(f"{name}: an entry is not <key id>:<base64> with a key id 1-65535")
            continue
        kid = int(kid)
        raw = b64_32(key)
        if raw is None:
            problems.append(f"{name} key {kid} is not the standard base64 of a 32-byte key")
            continue
        found = check_ed25519(name, kid, raw, subgroup)
        if found:
            problems += found
            continue
        if any(k == kid for k, _ in keys):
            problems.append(f"{name}: key id {kid} appears twice")
            continue
        if any(r == raw for _, r in keys):
            problems.append(f"{name}: the same key appears twice")
            continue
        keys.append((kid, raw))
    if not problems:
        if exactly is not None and len(keys) != exactly:
            problems.append(f"{name} must have {exactly} keys (the current and the next), has {len(keys)}")
        elif len(keys) > most:
            problems.append(f"{name} has more than {most} key{'s' if most > 1 else ''}")
    return keys, problems


def read_version(path):
    with open(path, encoding="utf-8") as f:
        for line in f:
            m = re.fullmatch(r"\s*HJV_VERSION=(\d+)\.(\d+)\.(\d+)\s*", line)
            if m:
                return tuple(int(n) for n in m.groups())
    raise SystemExit(f"{path}: no HJV_VERSION=<x.y.z> line")


# One line of client/update-keys.env that sets a value. The value's characters are those of key
# lists and of the floor, so that a shell, tools/hjv-release, the server's check script and this
# script all read the same value from the line, and $GITHUB_ENV gets nothing else.
_UPDATE_LINE = re.compile(r"(HJV_UPDATE_PUBKEY|HJV_UPDATE_ROOT_PUBKEY|HJV_POLICY_FLOOR)=([0-9A-Za-z+/=:,]*)")
# <epoch>:<seq> as the apps read it: plain numbers of 15 digits at most, an epoch from 1
_FLOOR = re.compile(r"[1-9][0-9]{0,14}:(0|[1-9][0-9]{0,14})")
# What str.splitlines(), editors and browsers end a line at besides LF and CR. Refused in comments
# too, so that every reader and every reviewer sees the lines this script reads.
_OTHER_LINE_BREAK = re.compile("[\x0b\x0c\x1c\x1d\x1e\x85\u2028\u2029]")


def read_update_keys(path):
    """(values, problems): the three values of client/update-keys.env by name, or None with the
    reasons why the file is not in its one form: NAME=value from the first column, each of the
    three names once, besides them only empty lines and lines that start with '#', LF line
    endings and no other character that a reader may end a line at. A problem names a line by
    its number: what the line holds is never repeated, in case something else than a public key
    was pasted there."""
    try:
        if os.path.islink(path) or not os.path.isfile(path):
            return None, [f"{path} is not a file: the builds take the update keys from it "
                          "(minimum version design 6-4)"]
        with open(path, "rb") as f:
            data = f.read(65537)
    except OSError as e:
        return None, [f"{path} cannot be read ({e.strerror})"]
    if len(data) > 65536:
        return None, [f"{path} is larger than 64 KiB"]
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        return None, [f"{path} is not UTF-8 text"]
    problems = []
    if text.startswith("\ufeff"):
        problems.append(f"{path} starts with a byte order mark; save it as UTF-8 without one")
        text = text[1:]
    if "\r" in text:
        problems.append(f"{path} has CR line endings; save it with LF line endings")
        text = text.replace("\r\n", "\n").replace("\r", "\n")
    values = {}
    for n, line in enumerate(text.split("\n"), 1):
        other = _OTHER_LINE_BREAK.search(line)
        if other:
            problems.append(f"{path}:{n}: holds U+{ord(other.group()):04X}, which other programs take for "
                            "a line break; only LF may end a line")
            continue
        if not line or line.startswith("#"):
            continue
        m = _UPDATE_LINE.fullmatch(line)
        if m is None:
            problems.append(
                f"{path}:{n}: neither a comment (# in the first column) nor one of "
                f"{', '.join(name + '=' for name in UPDATE_NAMES)} followed by its value and nothing "
                "else (no spaces, quotes or export)")
        elif m.group(1) in values:
            problems.append(f"{path}:{n}: {m.group(1)} is set a second time")
        else:
            values[m.group(1)] = m.group(2)
    problems += [f"{path} has no {name}= line" for name in UPDATE_NAMES if name not in values]
    return (None, problems) if problems else (values, [])


def parse_update_keys(values):
    """(signing, root, problems) of a key file's values: the signing keys and the root keys as
    [(key id, key)], both empty for a file without keys. The rules are the apps' (the client
    core's pinned(), which uses none of the keys when one of them breaks a rule) and those of
    tools/hjv-release, which wants the floor too: at most two signing keys numbered 1-100 and two
    root keys numbered 101-199 (the trust rules' 1-4: a declaration revokes signing key numbers
    only, and 1.6.0 pins two root keys, each kept offline on its own media)."""
    signing_text, root_text, floor = (values[name] for name in UPDATE_NAMES)
    problems = []
    if floor and not _FLOOR.fullmatch(floor):
        problems.append("HJV_POLICY_FLOOR is not <epoch>:<seq>, such as 1:0 (plain numbers, an epoch from 1)")
    if not signing_text and not root_text:
        return [], [], problems
    if not signing_text or not root_text:
        missing = UPDATE_NAMES[0] if not signing_text else UPDATE_NAMES[1]
        return [], [], problems + [
            f"{missing} is empty while the other key line is set: the apps need the signing keys "
            "and the root key, and use none without both"]
    signing, found = parse_list(UPDATE_NAMES[0], signing_text, most=2, subgroup=True)
    problems += found
    root, found = parse_list(UPDATE_NAMES[1], root_text, most=2, subgroup=True)
    problems += found
    if any(not 1 <= kid <= 100 for kid, _ in signing):
        problems.append(f"{UPDATE_NAMES[0]}: a signing key's id is not 1-100")
    if any(not 101 <= kid <= 199 for kid, _ in root):
        problems.append(f"{UPDATE_NAMES[1]}: a root key's id is not 101-199")
    if not floor:
        problems.append("HJV_POLICY_FLOOR is empty: with keys the file names the floor too, 1:0 before the first policy")
    for kid, raw in root:
        if any(k == kid for k, _ in signing):
            problems.append(f"key id {kid} is the root key's and a signing key's: the ids must differ")
        if any(r == raw for _, r in signing):
            problems.append("the root key is also a signing key: they must be different keys")
    if problems:
        return [], [], problems
    return signing, root, []


# --- the check ---


class Report:
    def __init__(self, warn=True):
        self.errors = 0
        self.rows = []
        self.notes = []
        self.actions = os.environ.get("GITHUB_ACTIONS") == "true"
        # False where another step of the run has given the same warnings already
        self.warn = warn

    def say(self, level, message):
        if level == "error":
            self.errors += 1
        if level == "warning" and not self.warn:
            print("note: " + message, flush=True)
            return
        prefix = f"::{level}::" if self.actions else f"{level}: "
        print(prefix + message, flush=True)

    def key(self, name, kid, raw):
        self.rows.append((name, kid, fingerprint(raw)))

    def summary(self, title):
        lines = [f"### {title}", ""]
        if self.rows:
            lines += [
                "| Variable | Key ID | SHA-256 of the key |",
                "|---|---|---|",
            ]
            lines += [f"| `{n}` | {k} | `{f}` |" for n, k, f in self.rows]
            note = ("Compare with the fingerprints of `hjv-api selfcheck` on the server before tagging "
                    "the release (design v3 11 step 8).")
            if any(n == "HJV_LOGIN_SEAL_PUBKEY" for n, _, _ in self.rows):
                note += (" The seal key's ID is the first 16 digits of its fingerprint, as "
                         "`hjv-api keygen seal` prints it.")
            lines += ["", note]
        else:
            lines.append("No pinned public key in this build.")
        lines += [""] + self.notes
        text = "\n".join(lines).rstrip() + "\n"
        path = os.environ.get("GITHUB_STEP_SUMMARY")
        if path:
            with open(path, "a", encoding="utf-8") as f:
                f.write(text + "\n")
        print(text, end="")


def spaced(name, raw_value):
    """A problem when the value holds whitespace: the builds take it byte for byte (option_env!),
    so a space after a comma or a line break from a file would reach the apps' parsers."""
    if re.search(r"\s", raw_value):
        return [f"{name} has spaces or line breaks; set it as one line without them"]
    return []


def judge(report, name, keys, problems, strict):
    """Reports a variable's problems and test keys; strict makes them errors."""
    level = "error" if strict else "warning"
    for problem in problems:
        report.say(level, problem)
    if problems:
        return
    test_keys = public_test_keys()
    for kid, raw in keys:
        if raw in test_keys:
            what = f"{name} key {kid}" if kid is not None else name
            report.say(
                level,
                f"{what} is a public test key of this repository or an RFC example "
                f"({test_keys[raw]}): its private key is public, so anyone could forge what this "
                "build trusts",
            )


def check_update_keys(report, path, strict, check_only, wanted):
    """Checks the key file client/update-keys.env and returns its three values by name for the
    builds, or None when no build may take them. A malformed file is an error in every run. A
    public test key, and a file without keys, are errors where strict (a called run of 1.6.0 or
    later; a file without keys not with check_only) and warnings elsewhere. `wanted` is false
    for a version before 1.6.0, which looked for no update keys."""
    before = report.errors
    values, problems = read_update_keys(path)
    signing, root = [], []
    if values is not None:
        signing, root, problems = parse_update_keys(values)
        for name in UPDATE_NAMES:
            other = os.environ.get(name, "")
            if other.strip() and other != values[name]:
                problems.append(
                    f"{name} is set in the environment to another value than in {path}: the builds "
                    "take the update keys from that file alone, never from a secret or a variable")
    for problem in problems:
        report.say("error", problem)
    if problems:
        return None
    if not signing:
        what = (f"{path} holds no update keys: apps built from it never check the version policy, "
                "so they learn of no minimum version and of no update")
        if strict and not check_only:
            report.say(
                "error",
                f"{what} (minimum version design 6-4). The user makes the keys "
                "(tools/hjv-release/README.md) and their public lines go into that file; or run "
                "with check_only for a check build.",
            )
        elif wanted and not check_only:
            report.say("warning", f"{what} (like 1.5.1).")
        report.notes.append(f"`{path}` holds no update keys: these apps check no version policy (like 1.5.1).")
        return None if report.errors > before else values
    for name, keys in ((UPDATE_NAMES[0], signing), (UPDATE_NAMES[1], root)):
        judge(report, name, keys, [], strict)
        for kid, raw in keys:
            report.key(name, kid, raw)
    report.notes.append(
        f"The update keys and `HJV_POLICY_FLOOR` ({values['HJV_POLICY_FLOOR']}) are those of `{path}` at "
        "the source commit, the file `hjv-release keys --production` reads.")
    return None if report.errors > before else values


def flags(version_file):
    """(called, check_only, wanted): what kind of run this is, and whether its version is one
    that pins the account keys and the update keys."""
    called = bool(os.environ.get("CALLED", "").strip())
    check_only = os.environ.get("CHECK_ONLY", "").strip() == "true"
    return called, check_only, read_version(version_file) >= ACCOUNT_VERSION


def check_app(version_file, update_file):
    report = Report()
    called, check_only, account = flags(version_file)
    strict = called and account
    env = {k: os.environ.get(k, "").strip() for k in (
        "HJV_SERVER_KEY", "HJV_LOGIN_SEAL_PUBKEY", "HJV_TRUST_PUBKEYS")}

    raw = b64_32(env["HJV_SERVER_KEY"])
    if raw is not None:
        report.key("HJV_SERVER_KEY", "", raw)
    elif env["HJV_SERVER_KEY"]:
        report.say("warning", "HJV_SERVER_KEY is not the base64 of a 32-byte key (hbbs's id_ed25519.pub)")

    missing = []
    for name, parse, required in (
        ("HJV_LOGIN_SEAL_PUBKEY", parse_seal, True),
        ("HJV_TRUST_PUBKEYS", lambda n, t: parse_list(n, t, exactly=2), True),
    ):
        text = env[name]
        if not text:
            if required:
                missing.append(name)
            continue
        keys, problems = parse(name, text)
        judge(report, name, keys, spaced(name, os.environ.get(name, "")) + problems, strict)
        for kid, raw in keys:
            report.key(name, kid if kid is not None else fingerprint(raw)[:16], raw)

    if missing and account:
        what = " and ".join(missing)
        if strict and not check_only:
            report.say(
                "error",
                f"{what} did not arrive: builds of version 1.6.0 and later pin the account keys "
                "(design v3 6-4). Set them as repository secrets of the build repository "
                "(docs/ci.md), or run with check_only for a check build. Without them the apps "
                "have no login and no password-less connections.",
            )
        else:
            report.say("warning", f"{what} not set: this build has no account features (like 1.5.1).")
    check_update_keys(report, update_file, strict, check_only, account)
    if check_only:
        report.notes.append(
            "**Check build** (`check_only`): not for distribution. Its artifacts end in `-check`, are "
            "kept one day, and nothing goes to TestFlight or Google Play.")
    report.summary("Pinned public keys")
    return 1 if report.errors else 0


def pin_update_keys(version_file, update_file):
    """The "Pin the update keys" step of a platform job: after the same check as check_app's,
    the key file's three values go to the later steps of the job ($GITHUB_ENV), where cargo
    reads them. Nothing is handed on when the check fails. The Checks job has shown this
    run's warnings and written its summary, so warnings are plain lines here."""
    report = Report(warn=False)
    called, check_only, wanted = flags(version_file)
    values = check_update_keys(report, update_file, called and wanted, check_only, wanted)
    if values is None:
        return 1
    lines = "".join(f"{name}={values[name]}\n" for name in UPDATE_NAMES)
    target = os.environ.get("GITHUB_ENV")
    if target:
        with open(target, "a", encoding="utf-8", newline="\n") as f:
            f.write(lines)
    else:
        print(lines, end="", flush=True)
    for name, kid, digest in report.rows:
        print(f"pinned {name} key {kid}, SHA-256 {digest}", file=sys.stderr)
    if not report.rows:
        print("no update keys to pin: the builds of this job check no version policy", file=sys.stderr)
    print(f"pinned HJV_POLICY_FLOOR={values['HJV_POLICY_FLOOR']}", file=sys.stderr)
    return 0


def check_relay():
    report = Report()
    called = bool(os.environ.get("CALLED", "").strip())
    check_only = os.environ.get("CHECK_ONLY", "").strip() == "true"
    strict = called and not check_only
    text = os.environ.get("HJV_COMMAND_PUBKEY", "").strip()
    if text:
        keys, problems = parse_list("HJV_COMMAND_PUBKEY", text)
        problems = spaced("HJV_COMMAND_PUBKEY", os.environ.get("HJV_COMMAND_PUBKEY", "")) + problems
        judge(report, "HJV_COMMAND_PUBKEY", keys, problems, strict)
        for kid, raw in keys:
            report.key("HJV_COMMAND_PUBKEY", kid, raw)
    elif strict:
        report.say(
            "error",
            "HJV_COMMAND_PUBKEY did not arrive: hjv-relay trusts the wake commands signed with it "
            "(relay/PROTOCOL.md 3.2). Set it as a repository secret of the build repository "
            "(docs/ci.md), or run with check_only.",
        )
    report.summary("hjv-relay command keys")
    return 1 if report.errors else 0


def main(argv):
    if argv[:1] == ["pubkey"]:
        if len(argv) != 3 or argv[1] not in ("ed25519", "x25519") or not re.fullmatch(r"[0-9a-fA-F]{64}", argv[2]):
            print("usage: check-build-keys.py pubkey ed25519|x25519 <64 hex digits>", file=sys.stderr)
            return 2
        secret = bytes.fromhex(argv[2])
        pub = ed25519_public(secret) if argv[1] == "ed25519" else x25519_public(secret)
        print(base64.b64encode(pub).decode())
        return 0
    parser = argparse.ArgumentParser(description="Checks the public keys an HJViewer build pins.")
    parser.add_argument("--version-file", default=os.path.join(ROOT, "client", "version.env"))
    parser.add_argument("--update-keys", default=None, metavar="FILE",
                        help="the update keys' file (default: client/update-keys.env of this tree)")
    parser.add_argument("--relay", action="store_true", help="check HJV_COMMAND_PUBKEY for the hjv-relay job")
    parser.add_argument("--pin-update-keys", action="store_true",
                        help="check the update keys' file and hand its values to the job's later steps")
    args = parser.parse_args(argv)
    if args.relay and (args.pin_update_keys or args.update_keys):
        parser.error("--relay checks the command key only")
    if args.relay:
        return check_relay()
    update_file = args.update_keys
    if update_file is None:
        try:
            update_file = os.path.relpath(UPDATE_KEYS_FILE)
        except ValueError:  # another drive on Windows
            update_file = UPDATE_KEYS_FILE
    if args.pin_update_keys:
        return pin_update_keys(args.version_file, update_file)
    return check_app(args.version_file, update_file)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

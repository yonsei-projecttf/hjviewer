#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Checks the public keys an HJViewer build pins (design v3 6-4; docs/ci.md, "계정 기능의 공개키").

client-build.yml runs it after the checkout, with the build variables in the environment as cargo
sees them:

    python3 -I scripts/check-build-keys.py [--version-file client/version.env]

    HJV_LOGIN_SEAL_PUBKEY  standard base64 of the 32-byte X25519 key `hjv-api keygen seal` prints
    HJV_TRUST_PUBKEYS      <key id>:<base64>,<key id>:<base64>: the two Ed25519 trust-list keys
    HJV_UPDATE_PUBKEY      <key id>:<base64>[,<key id>:<base64>]: the update keys of the
                           minimum-version work, optional for now
    HJV_SERVER_KEY         hbbs's public key, whose fingerprint is shown (the workflow's "Check
                           server variables" step checks that it is set)
    CALLED                 non-empty when the private build repository called the workflow
    CHECK_ONLY             "true" for a check build, which hands nothing out

A called run of client/version.env 1.6.0 or later fails when HJV_LOGIN_SEAL_PUBKEY or
HJV_TRUST_PUBKEYS is missing (not with CHECK_ONLY), when a key given is malformed or weak, and when
a key is a public test key, whose private key anyone has: keys made from a seed of one repeated byte
or of 32 consecutive byte values, the RFC 8032 and RFC 7748 examples, and the keys in
scripts/public-test-keys.txt. In other runs these are warnings: without the account keys an app is
built like 1.5.1, with no login. The SHA-256 fingerprints of the pinned keys go to the run summary
($GITHUB_STEP_SUMMARY, else stdout), for comparing with `hjv-api selfcheck` before the release tag
(design v3 11 step 8).

    python3 -I scripts/check-build-keys.py --relay

does the same for HJV_COMMAND_PUBKEY (<key id>:<base64>[,<key id>:<base64>]) in the hjv-relay
job, which scripts/package-relay.sh then checks again; and

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
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TEST_KEYS_FILE = os.path.join(ROOT, "scripts", "public-test-keys.txt")
ACCOUNT_VERSION = (1, 6, 0)

# --- Edwards25519 and Curve25519 (RFC 8032, RFC 7748), just enough to derive and check keys ---

P = 2**255 - 19
D = -121665 * pow(121666, P - 2, P) % P
SQRT_M1 = pow(2, (P - 1) // 4, P)
_BX = 15112221349535400772501151409588531511454012693041857206046113283949847762202
_BY = 46316835694926478169428394003475163141307993866256225615783033603165251855960
BASE = (_BX, _BY, 1, _BX * _BY % P)
IDENTITY = (0, 1, 1, 0)


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
_test_keys = None


def test_seeds():
    """The secrets tests write down: one repeated byte, 32 consecutive byte values, RFC examples."""
    seeds = [bytes([b]) * 32 for b in range(256)]
    seeds += [bytes((b + i) % 256 for i in range(32)) for b in range(256)]
    seeds += [bytes.fromhex(h) for h in RFC_SECRETS]
    return seeds


def read_key_file(path):
    """The base64 keys of a list file: one per line, '#' starts a comment."""
    keys = set()
    try:
        with open(path, encoding="utf-8") as f:
            lines = f.readlines()
    except FileNotFoundError:
        return keys
    for n, line in enumerate(lines, 1):
        text = line.split("#", 1)[0].strip()
        if not text:
            continue
        raw = b64_32(text)
        if raw is None:
            raise SystemExit(f"{path}:{n}: not the base64 of a 32-byte key")
        keys.add(raw)
    return keys


def public_test_keys():
    global _test_keys
    if _test_keys is None:
        keys = read_key_file(TEST_KEYS_FILE)
        for seed in test_seeds():
            keys.add(ed25519_public(seed))
            keys.add(x25519_public(seed))
        _test_keys = keys
    return _test_keys


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


def check_ed25519(name, kid, raw):
    """Problems of an Ed25519 public key, as ed25519-dalek and hjv-relay judge it."""
    what = f"{name} key {kid}" if kid is not None else name
    point = _decode(raw)
    if point is None:
        return [f"{what} is not an Ed25519 public key"]
    if ed25519_weak(point):
        return [f"{what} is a weak key (small order): anyone could sign with it"]
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


def parse_list(name, text, exactly=None, most=2):
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
        found = check_ed25519(name, kid, raw)
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
            problems.append(f"{name} has more than {most} keys")
    return keys, problems


def read_version(path):
    with open(path, encoding="utf-8") as f:
        for line in f:
            m = re.fullmatch(r"\s*HJV_VERSION=(\d+)\.(\d+)\.(\d+)\s*", line)
            if m:
                return tuple(int(n) for n in m.groups())
    raise SystemExit(f"{path}: no HJV_VERSION=<x.y.z> line")


# --- the check ---


class Report:
    def __init__(self):
        self.errors = 0
        self.rows = []
        self.notes = []
        self.actions = os.environ.get("GITHUB_ACTIONS") == "true"

    def say(self, level, message):
        if level == "error":
            self.errors += 1
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
            lines += [
                "",
                "Compare with the fingerprints of `hjv-api selfcheck` on the server before tagging the "
                "release (design v3 11 step 8). The seal key's ID is the first 16 digits of its "
                "fingerprint, as `hjv-api keygen seal` prints it.",
            ]
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
                f"{what} is a public test key of this repository or an RFC example: its private "
                "key is public, so anyone could forge what this build trusts",
            )


def check_app(version_file):
    report = Report()
    called = bool(os.environ.get("CALLED", "").strip())
    check_only = os.environ.get("CHECK_ONLY", "").strip() == "true"
    version = read_version(version_file)
    account = version >= ACCOUNT_VERSION
    strict = called and account
    env = {k: os.environ.get(k, "").strip() for k in (
        "HJV_SERVER_KEY", "HJV_LOGIN_SEAL_PUBKEY", "HJV_TRUST_PUBKEYS", "HJV_UPDATE_PUBKEY")}

    raw = b64_32(env["HJV_SERVER_KEY"])
    if raw is not None:
        report.key("HJV_SERVER_KEY", "", raw)
    elif env["HJV_SERVER_KEY"]:
        report.say("warning", "HJV_SERVER_KEY is not the base64 of a 32-byte key (hbbs's id_ed25519.pub)")

    missing = []
    for name, parse, required in (
        ("HJV_LOGIN_SEAL_PUBKEY", parse_seal, True),
        ("HJV_TRUST_PUBKEYS", lambda n, t: parse_list(n, t, exactly=2), True),
        ("HJV_UPDATE_PUBKEY", parse_list, False),
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
    if not env["HJV_UPDATE_PUBKEY"]:
        report.notes.append("`HJV_UPDATE_PUBKEY` is not set (the minimum-version work adds it).")
    if check_only:
        report.notes.append(
            "**Check build** (`check_only`): not for distribution. Its artifacts end in `-check`, are "
            "kept one day, and nothing goes to TestFlight or Google Play.")
    report.summary("Pinned public keys")
    return 1 if report.errors else 0


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
    parser.add_argument("--relay", action="store_true", help="check HJV_COMMAND_PUBKEY for the hjv-relay job")
    args = parser.parse_args(argv)
    return check_relay() if args.relay else check_app(args.version_file)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

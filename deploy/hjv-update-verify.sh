#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# The server's own check of a signed version policy before it becomes /etc/hjviewer/version-policy.json,
# the file nginx hands to every app at https://<domain>/api/hjv/version (docs/server-runbook.md 8;
# minimum version design 4-5; the trust rules' specification, 4-8):
#
#   hjv-update-verify.sh [options] <new envelope>
#     --keys FILE      the update keys of the newest build: client/update-keys.env of the public
#                      repository's release tag (default /etc/hjviewer/update-keys.env)
#     --live FILE      the envelope served now (default /etc/hjviewer/version-policy.json; none
#                      there is the first install)
#     --server NAME    the server name the apps are built with (default: HJV_DOMAIN of
#                      /etc/hjviewer/hjviewer.env)
#     --relaxes LIST   the entries the operator means to relax with this envelope's new
#                      declaration, comma-separated (windows,android,android_play,ios); default none
#     --packages DIR   where the packages the policy names are looked for, in the order given
#                      (default: the pkg/ beside the new envelope, then /srv/hjviewer-updates/pkg)
#   hjv-update-verify.sh --vectors DIR   the shared test vectors (tools/hjv-release/tests/vectors)
#   hjv-update-verify.sh --self-test     whether this machine's python3 and openssl can check at all
#
# It decides like the apps (libsodium, the client core): the envelope must be what a fresh install
# of the newest build and a device that holds the live policy both take, every signature strict
# (OpenSSL alone would take a signature whose R has a small order), hjv-api must read its Windows
# and Android entries, a new declaration must relax exactly the entries named, nothing may be dated
# ahead of this server's clock, and every package named must be there with its size and SHA-256.
# It reads only texts exactly as hjv-release writes them. Exit 0: install it; 1: refused, with the
# reason; 2: the check could not run. A refused new file that lies beside the live one is removed,
# so that no "mv" can make it live by mistake (the operator's archive keeps it).
#
# Only bash, python3's standard library and openssl: the server never runs the operator's tool, so
# that one machine cannot vouch for itself.
set -euo pipefail
export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin LC_ALL=C.UTF-8
for tool in python3 openssl; do
  command -v "$tool" > /dev/null || { echo "hjv-update-verify: $tool is missing" >&2; exit 2; }
done
exec python3 -I -B - "$0" "$@" << 'PYTHON'
"""The check itself; the shell part above has what it does and how it is called."""

import base64
import binascii
import hashlib
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import time

POLICY_CONTEXT = b"hjv-version-v1"
ROOT_CONTEXT = b"hjv-version-root-v1"
# A1: an app reads no more of the answer (the file's line feed counts)
MAX_ENVELOPE = 16384
# what this script reads of any file
MAX_READ = 1 << 20
MAX_PACKAGE = 200 * 1024 * 1024
MAX_CONTACT = 200
MAX_NOTES = 2000
# C5: how far one policy may move the sequence number
STEP = 1000
# item 6: how far ahead of this server's clock a policy or a new declaration may be dated
CLOCK_SLACK = 300
NAMES = ("windows", "android", "android_play", "ios")
ANDROID = ("android", "android_play")
U64 = (1 << 64) - 1
UPDATE_NAMES = ("HJV_UPDATE_PUBKEY", "HJV_UPDATE_ROOT_PUBKEY", "HJV_POLICY_FLOOR")
DEFAULT_KEYS = "/etc/hjviewer/update-keys.env"
DEFAULT_LIVE = "/etc/hjviewer/version-policy.json"
DEFAULT_ENV = "/etc/hjviewer/hjviewer.env"
DEFAULT_PKG = "/srv/hjviewer-updates/pkg"

# Ed25519: the field, the group order L, the curve constant, the base point
P = 2**255 - 19
L = 2**252 + 27742317777372353535851937790883648493
D = -121665 * pow(121666, P - 2, P) % P
_BX = 15112221349535400772501151409588531511454012693041857206046113283949847762202
_BY = 46316835694926478169428394003475163141307993866256225615783033603165251855960
BASE = (_BX, _BY, 1, _BX * _BY % P)
IDENTITY = (0, 1, 1, 0)
# The encodings libsodium refuses as a signature's R and as a public key (ge25519_has_small_order),
# compared with the sign bit cleared: the eight points of small order and the two non-canonical
# encodings y = p and y = p + 1. OpenSSL's Ed25519 checks only the equation, which a signature
# whose R is the identity satisfies (vector signature-with-identity-r).
SMALL_ORDER = frozenset(bytes.fromhex(h) for h in (
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0100000000000000000000000000000000000000000000000000000000000000",
    "26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc05",
    "c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac037a",
    "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
    "edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
    "eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
))
# RFC 8032 7.1 TEST 2: OpenSSL must take it, and refuse it with one bit changed, before this
# script believes OpenSSL about anything
SELF_TEST = (
    "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
    "72",
    "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da"
    "085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00",
)

WHY = {
    "TooLarge": "the envelope is larger than the 16384 bytes an app reads",
    "Envelope": "the envelope is not of the envelope's form",
    "Version": "a format version (v) this app version does not read",
    "RootKey": "the declaration is signed by a key that is not a pinned root key",
    "RootSignature": "the declaration's signature does not hold (strictly) for that root key",
    "RootFormat": "the declaration breaks a rule of its text (epoch 2 or later, a base, ...)",
    "RootServer": "the declaration is for another server",
    "RootRevokesRoot": "the declaration revokes a pinned root key",
    "RootEpoch": "the declaration is of an epoch before the device's",
    "RootFork": "another declaration text for the device's epoch",
    "UnknownKey": "the policy is signed by a key this build does not pin",
    "RevokedKey": "the policy's signing key is revoked",
    "BadSignature": "the policy's signature does not hold (strictly) for its key",
    "Server": "the policy is for another server",
    "Epoch": "the policy is not of the device's epoch, or comes without its epoch's declaration",
    "Fork": "the position the device holds, with other bytes",
    "Rollback": "a sequence number the device is past",
    "Jump": "the sequence number moves more than 1000 at once",
    "BelowFloor": "older than the build's floor",
    "Issued": "issued before the policy (or declaration) the device compares it with",
    "Relaxation": "it asks less than the policy (or base) the device holds",
}


class Refused(Exception):
    """An envelope refused: `code` is the apps' reason (spec 2-4) or one of the install check's
    (spec 4-8)."""

    def __init__(self, code, detail=""):
        super().__init__(code)
        self.code = code
        self.detail = detail

    def text(self):
        why = WHY.get(self.code.split(":", 1)[0], "")
        if self.code.startswith("Field:"):
            why = "the policy's %s breaks its rule" % self.code[6:]
        parts = [p for p in (why, self.detail) if p]
        return self.code + (": " + "; ".join(parts) if parts else "")


class CannotRun(Exception):
    """The check cannot run (exit 2): missing or broken input that is not the envelope's fault."""


# ---- numbers, versions, times (spec 1-2) ---------------------------------------------------------


def ascii_digits(s):
    return s != "" and all("0" <= c <= "9" for c in s)


def version_number(text):
    """num() of a strict x.y.z (today's Version::parse: no leading zeros, parts of at most 3, 3
    and 2 digits), else None. hbb_common::get_version_number of that form is x·10⁶ + y·10³ + 10·z."""
    parts = text.split(".")
    if len(parts) != 3:
        return None
    for part, most in zip(parts, (3, 3, 2)):
        if not ascii_digits(part) or len(part) > most or (part != "0" and part[0] == "0"):
            return None
    x, y, z = (int(part) for part in parts)
    return x * 1_000_000 + y * 1000 + 10 * z


def days_from_civil(y, m, d):
    """Days since 1970-01-01 of a proleptic Gregorian date (any year 0-9999)."""
    y -= m <= 2
    era = y // 400
    yoe = y - era * 400
    doy = (153 * (m + (-3 if m > 2 else 9)) + 2) // 5 + d - 1
    return era * 146097 + yoe * 365 + yoe // 4 - yoe // 100 + doy - 719468


def parse_utc(text):
    """Unix seconds of the one form of a time, YYYY-MM-DDTHH:MM:SSZ with a year 0001-9999 (spec 1-2,
    decided 2026-10-14: no implementation reads the year 0000), else None."""
    if not text.isascii() or len(text) != 20 or [text[i] for i in (4, 7, 10, 13, 16, 19)] != list("--T::Z"):
        return None
    fields = (text[0:4], text[5:7], text[8:10], text[11:13], text[14:16], text[17:19])
    if not all(ascii_digits(f) for f in fields):
        return None
    y, mo, d, h, mi, s = (int(f) for f in fields)
    leap = y % 4 == 0 and (y % 100 != 0 or y % 400 == 0)
    days = (31, 29 if leap else 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31)
    if y < 1 or not 1 <= mo <= 12 or not 1 <= d <= days[mo - 1] or h > 23 or mi > 59 or s > 59:
        return None
    return days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + s


def format_utc(t):
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(t))


def version_text(n):
    return "%d.%d.%d" % (n // 1_000_000, n // 1000 % 1000, n % 1000 // 10)


# ---- base64, keys, signatures --------------------------------------------------------------------


def b64_exact(text, length):
    """The bytes of `text` if it is the canonical standard base64 (padded, no stray low bits) of
    exactly `length` bytes, as the base64 crate's STANDARD engine takes it, else None."""
    if not isinstance(text, str) or not text.isascii():
        return None
    try:
        raw = base64.b64decode(text, validate=True)
    except (binascii.Error, ValueError):
        return None
    if len(raw) != length or base64.b64encode(raw).decode("ascii") != text:
        return None
    return raw


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


def _is_identity(p):
    x, y, z, _ = p
    return x % P == 0 and (y - z) % P == 0


def decode_point(b):
    """The point a 32-byte encoding names (RFC 8032 5.1.3), strictly (y < p), or None."""
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
        x = x * pow(2, (P - 1) // 4, P) % P
    elif vxx != u:
        return None
    if x == 0 and sign:
        return None
    if x & 1 != sign:
        x = P - x
    return (x, y, 1, x * y % P)


def small_order(encoding):
    return encoding[:31] + bytes([encoding[31] & 0x7F]) in SMALL_ORDER


def key_problem(raw):
    """Why the apps would not pin `raw` as an update key (libsodium's
    crypto_sign_ed25519_pk_to_curve25519: on the curve, not of small order, in the prime-order
    subgroup), or None."""
    point = decode_point(raw)
    if point is None:
        return "not an Ed25519 public key"
    if small_order(raw) or _is_identity(_mul(8, point)):
        return "a weak key (small order)"
    if not _is_identity(_mul(L, point)):
        return "outside the prime-order subgroup"
    return None


def weak_signature(sig):
    """Why a 64-byte signature is one that libsodium and ed25519-dalek's verify_strict refuse
    whatever it signs, or None: S not below the group order, or an R of small order."""
    if int.from_bytes(sig[32:], "little") >= L:
        return "its S is not below the group order"
    if small_order(sig[:32]):
        return "its R is a point of small order"
    return None


class Verifier:
    """Strict Ed25519 (spec B2, C2): the weak signatures of weak_signature() are refused here, the
    equation is OpenSSL's (openssl pkeyutl, the raw key wrapped as SubjectPublicKeyInfo)."""

    SPKI = bytes.fromhex("302a300506032b6570032100")

    def __init__(self):
        self.dir = None
        self.cache = {}
        self.calls = 0

    def close(self):
        if self.dir is not None:
            shutil.rmtree(self.dir, ignore_errors=True)
            self.dir = None

    def openssl(self, pub, message, sig):
        if self.dir is None:
            self.dir = tempfile.mkdtemp(prefix="hjv-update-verify-")
        self.calls += 1
        base = os.path.join(self.dir, str(self.calls))
        for ext, data in ((".key", self.SPKI + pub), (".msg", message), (".sig", sig)):
            with open(base + ext, "wb") as f:
                f.write(data)
        try:
            done = subprocess.run(
                ["openssl", "pkeyutl", "-verify", "-pubin", "-inkey", base + ".key", "-keyform", "DER",
                 "-rawin", "-in", base + ".msg", "-sigfile", base + ".sig"],
                stdin=subprocess.DEVNULL, capture_output=True, check=False)
        except OSError as e:
            raise CannotRun("openssl could not be started: %s" % e.strerror)
        finally:
            for ext in (".key", ".msg", ".sig"):
                try:
                    os.unlink(base + ext)
                except OSError:
                    pass
        out = done.stdout.decode("utf-8", "replace")
        if done.returncode == 0 and "Signature Verified Successfully" in out:
            return True
        if "Signature Verification Failure" in out:
            return False
        raise CannotRun("openssl pkeyutl gave no verdict (exit %d): %s" % (
            done.returncode, done.stderr.decode("utf-8", "replace").strip()[:200]))

    def self_test(self):
        pub, message, sig = (bytes.fromhex(h) for h in SELF_TEST)
        bad = bytes([sig[0] ^ 1]) + sig[1:]
        if not self.openssl(pub, message, sig) or self.openssl(pub, message, bad):
            raise CannotRun("this openssl does not verify Ed25519 as RFC 8032 says (openssl version: %s)"
                            % openssl_version())

    def verify(self, pub, context, text, sig):
        if weak_signature(sig):
            return False
        message = context + b"\0" + text.encode("utf-8")
        key = (pub, sig, hashlib.sha256(message).digest())
        if key not in self.cache:
            self.cache[key] = self.openssl(pub, message, sig)
        return self.cache[key]


def openssl_version():
    try:
        return subprocess.run(["openssl", "version"], capture_output=True, check=False).stdout.decode().strip()
    except OSError:
        return "-"


# ---- item 1: the canonical form (spec 1-3, 4-8) --------------------------------------------------


class NotCanonical(Exception):
    pass


class Obj(list):
    """A JSON object as its (name, value) pairs, in order, repeats kept."""


def _constant(name):
    raise NotCanonical("%s is not JSON's" % name)


def json_value(text, what):
    try:
        return json.loads(text, object_pairs_hook=Obj, parse_constant=_constant)
    except NotCanonical:
        raise
    except (ValueError, RecursionError) as e:
        raise NotCanonical("%s is not JSON (%s)" % (what, e))


# The member order, type and presence of every text, exactly as hjv-release writes it (spec 1-3).
# A member a text does not name is not canonical, and so is a text without a member the tool always
# writes: a declaration has all six (the term `canonical`, decided 2026-10-14; the vectors
# declaration-without-base and declaration-without-revoked).
PACKAGE = (("file", "str", True), ("size", "int", True), ("sha256", "str", True), ("build", "str", True))
ENTRY = (("min", "str", True), ("latest", "str", True), ("grace_until", "str", False),
         ("grace_min", "str", False), ("os_min", "str", False), ("notes_ko", "str", False),
         ("source", "str", False), ("package", PACKAGE, False), ("auto_install_after", "str", False),
         ("move_to_play", "true", False), ("version_code", "int", False),
         ("testflight_build", "str", False))
POLICY = (("v", "int", True), ("epoch", "int", True), ("seq", "int", True), ("issued", "str", True),
          ("server", "str", True), ("contact_ko", "str", False),
          ("platforms", tuple((name, ENTRY, False) for name in NAMES), True))
BASE_ENTRY = (("min", "str", True), ("latest", "str", True), ("grace_until", "str", False),
              ("grace_min", "str", False))
DECLARATION = (("v", "int", True), ("server", "str", True), ("epoch", "int", True),
               ("revoked", "ints", True), ("issued", "str", True),
               ("base", tuple((name, BASE_ENTRY, False) for name in NAMES), True))
PART = (("k", "key", True), ("p", "str", True), ("s", "sig", True))
ENVELOPE = (("v", "int", True), ("k", "key", True), ("p", "str", True), ("s", "sig", True),
            ("root", PART, False))


def _write(value, kind, where):
    if isinstance(kind, tuple):
        return _write_object(value, kind, where)
    if kind in ("int", "key"):
        if type(value) is not int or value < 0 or value > U64:
            raise NotCanonical("%s is not a JSON integer of 0 to 2^64-1" % where)
        if kind == "key" and not 1 <= value <= 65535:
            raise NotCanonical("%s is not a key number 1-65535" % where)
        return str(value)
    if kind == "ints":
        if not isinstance(value, list) or isinstance(value, Obj):
            raise NotCanonical("%s is not a list" % where)
        return "[" + ",".join(_write(v, "int", where + "[]") for v in value) + "]"
    if kind == "true":
        if value is not True:
            raise NotCanonical("%s is not true (the tool leaves it out otherwise)" % where)
        return "true"
    if type(value) is not str:
        raise NotCanonical("%s is not a string" % where)
    try:
        value.encode("utf-8")
    except UnicodeEncodeError:
        raise NotCanonical("%s holds a lone surrogate" % where)
    if kind == "sig" and b64_exact(value, 64) is None:
        raise NotCanonical("%s is not the canonical base64 of 64 bytes" % where)
    # escapes exactly as serde_json::to_string, which the tool writes strings with
    return json.dumps(value, ensure_ascii=False)


def _write_object(value, schema, where):
    if not isinstance(value, Obj):
        raise NotCanonical("%s is not an object" % where)
    seen = {}
    for name, member in value:
        if name in seen:
            raise NotCanonical("%s has %s twice" % (where, name))
        seen[name] = member
    known = {name for name, _, _ in schema}
    for name in seen:
        if name not in known:
            raise NotCanonical("%s has a member the tool does not write: %s" % (where, json.dumps(name)))
    out = []
    for name, kind, required in schema:
        if name not in seen:
            if required:
                raise NotCanonical("%s has no %s" % (where, name))
            continue
        out.append(json.dumps(name) + ":" + _write(seen[name], kind, where + "." + name))
    return "{" + ",".join(out) + "}"


def to_dict(value):
    """Obj → dict, all the way down (a canonical text has no repeated member)."""
    if isinstance(value, Obj):
        return {k: to_dict(v) for k, v in value}
    if isinstance(value, list):
        return [to_dict(v) for v in value]
    return value


def canonical_text(text, schema, what):
    """The text's members as a dict, if `text` (a str) is exactly the tool's writing of them."""
    value = json_value(text, what)
    if _write_object(value, schema, what) != text:
        raise NotCanonical("%s is not written as the tool writes it (member order, spaces, escapes)" % what)
    return to_dict(value)


def canonical_envelope(data):
    """Item 1 for a file's bytes: one line in the tool's canonical form and a line feed; its p and
    root.p canonical too. Returns the envelope's members, with p and root.p parsed under "P" and
    "D" (D absent without a root part); raises NotCanonical."""
    if not data.endswith(b"\n") or b"\n" in data[:-1] or data.endswith(b"\r\n"):
        raise NotCanonical("the file is not one line and a line feed")
    try:
        text = data[:-1].decode("utf-8")
    except UnicodeDecodeError:
        raise NotCanonical("the file is not UTF-8")
    env = canonical_text(text, ENVELOPE, "the envelope")
    env["P"] = canonical_text(env["p"], POLICY, "the policy text p")
    if "root" in env:
        env["D"] = canonical_text(env["root"]["p"], DECLARATION, "the declaration text root.p")
    return env


# ---- texts (spec 1-3) and their rules (A2, B3, C3) -----------------------------------------------


class Entry:
    """An entry of a policy or a base: num() of min, latest and grace_min; grace_until in seconds."""

    __slots__ = ("min", "latest", "gu", "gm", "min_text", "package")

    def __init__(self, mn, latest, gu=None, gm=None, min_text="", package=None):
        self.min, self.latest, self.gu, self.gm = mn, latest, gu, gm
        self.min_text = min_text or version_text(mn)
        self.package = package

    def describe(self):
        s = "min %s, latest %s" % (self.min_text, version_text(self.latest))
        if self.gu is not None:
            s += ", until %s below %s blocked" % (format_utc(self.gu), version_text(self.gm))
        return s


class Policy:
    __slots__ = ("epoch", "seq", "issued", "server", "entries", "p", "h")


class Decl:
    __slots__ = ("epoch", "revoked", "issued", "base", "server", "p", "k", "s")


class Part:
    __slots__ = ("k", "p", "s")

    def __init__(self, k, p, s):
        self.k, self.p, self.s = k, p, s


class Envelope:
    """An envelope that passed item 1 and A2 (its v is 1)."""

    __slots__ = ("pol", "root", "data", "P", "D")


def token(text, most):
    """Letters, digits, ".", "_" and "-" (ASCII), at least one and at most `most` of them."""
    return 1 <= len(text) <= most and all(c.isascii() and (c.isalnum() or c in "._-") for c in text)


def plain_name(name):
    return token(name, 100)


def _entry(e, where):
    """C3's rules of an entry, in today's order with grace_min after grace_until. `where` is
    "Field:" for a policy and "RootFormat" for a base entry."""
    def bad(member):
        raise Refused("RootFormat", "base entry: " + member) if where == "RootFormat" else Refused("Field:" + member)

    mn = version_number(e["min"])
    if mn is None:
        bad("min")
    latest = version_number(e["latest"])
    if latest is None:
        bad("latest")
    if mn > latest:
        bad("min")
    if where != "RootFormat":
        os_min = e.get("os_min")
        if os_min is not None and not (1 <= len(os_min) <= 16 and all(c in "0123456789." for c in os_min)):
            bad("os_min")
        notes = e.get("notes_ko")
        if notes is not None and len(notes) > MAX_NOTES:
            bad("notes_ko")
        source = e.get("source")
        if source is not None and not plain_name(source):
            bad("source")
    gu = None
    if "grace_until" in e:
        gu = parse_utc(e["grace_until"])
        if gu is None:
            bad("grace_until")
    gm = None
    if "grace_min" in e:
        gm = version_number(e["grace_min"])
        if gm is None:
            bad("grace_min")
    if (gu is None) != (gm is None) or (gm is not None and gm >= mn):
        bad("grace_min")
    package = None
    if where != "RootFormat":
        pkg = e.get("package")
        if pkg is not None:
            name = pkg["file"]
            lower = name.lower()
            if not plain_name(name) or "-qs-" in lower or lower.endswith("install.exe"):
                bad("package.file")
            if not 1 <= pkg["size"] <= MAX_PACKAGE:
                bad("package.size")
            if not re.fullmatch(r"[0-9a-f]{64}", pkg["sha256"]):
                bad("package.sha256")
            package = (name, pkg["size"], pkg["sha256"])
        if "auto_install_after" in e and parse_utc(e["auto_install_after"]) is None:
            bad("auto_install_after")
    return Entry(mn, latest, gu, gm, e["min"], package)


# What hjv-api reads of a policy (server/api/src/update.rs, Policy::parse): of the windows and
# android entries the latest, the source and the package. A text that breaks one of its rules there
# is no policy at all for hjv-api, so that door 1, door 2 and the short links answer 404, while the
# apps' rules (C3) may take it: they allow a source of up to 100 characters and a package file of
# any plain name. hjv-release writes no such text; a stolen key or a fault of the tool could.
API_ENTRIES = (("windows", ".exe"), ("android", ".apk"))
API_MAX_SOURCE = 64


def api_package_file(name, extension):
    """hjv-api's is_package_file(): a name of its package directory for an entry of `extension`."""
    lower = name.lower()
    return (token(name, 100) and not name.startswith(".") and ".." not in name and lower.endswith(extension)
            and len(name) > len(extension) and "-qs-" not in lower and not lower.endswith("install.exe"))


def api_problem(platforms):
    """Why hjv-api would read a policy text with these `platforms` as no policy, or None."""
    for name, extension in API_ENTRIES:
        e = platforms.get(name)
        if e is None:
            continue
        source = e.get("source")
        if source is not None and not token(source, API_MAX_SOURCE):
            return "the %s entry's source is not a tag name of at most %d letters, digits, \".\", \"_\" and \"-\"" % (
                name, API_MAX_SOURCE)
        package = e.get("package")
        if package is not None and not api_package_file(package["file"], extension):
            return ("the %s entry's package file %s is not a name hjv-api hands out (a plain name ending in %s, "
                    "no leading \".\", no \"..\")" % (name, json.dumps(package["file"]), extension))
    return None


def parse_policy(p, d, server):
    """C3 for a canonical policy text `p` with members `d`."""
    if d["v"] != 1:
        raise Refused("Version", "policy v %d" % d["v"])
    if d["epoch"] < 1:
        raise Refused("Field:epoch")
    if d["seq"] < 1:
        raise Refused("Field:seq")
    issued = parse_utc(d["issued"])
    if issued is None:
        raise Refused("Field:issued")
    if len(d.get("contact_ko", "")) > MAX_CONTACT:
        raise Refused("Field:contact_ko")
    entries = {}
    for name in NAMES:
        if name in d["platforms"]:
            entries[name] = _entry(d["platforms"][name], "Field")
    if d["server"] != server:
        raise Refused("Server", "the policy names %s, the apps are built for %s" % (json.dumps(d["server"]), server))
    pol = Policy()
    pol.epoch, pol.seq, pol.issued, pol.server, pol.entries, pol.p = d["epoch"], d["seq"], issued, d["server"], entries, p
    pol.h = hashlib.sha256(p.encode("utf-8")).digest()
    return pol


def parse_declaration(part, d, server, roots):
    """B3 for a canonical declaration text with members `d`."""
    if d["v"] != 1:
        raise Refused("Version", "declaration v %d" % d["v"])
    if d["epoch"] < 2:
        raise Refused("RootFormat", "epoch %d (a declaration opens epoch 2 or later)" % d["epoch"])
    if any(not 1 <= k <= 65535 for k in d["revoked"]):
        raise Refused("RootFormat", "revoked is not a list of key numbers 1-65535")
    issued = parse_utc(d["issued"])
    if issued is None:
        raise Refused("RootFormat", "issued is not a time")
    if "base" not in d:
        raise Refused("RootFormat", "no base")
    base = {name: _entry(e, "RootFormat") for name, e in d["base"].items()}
    if d["server"] != server:
        raise Refused("RootServer", "the declaration names %s, the apps are built for %s"
                      % (json.dumps(d["server"]), server))
    if any(k in roots for k in d["revoked"]):
        raise Refused("RootRevokesRoot")
    decl = Decl()
    decl.epoch, decl.revoked, decl.issued, decl.base = d["epoch"], frozenset(d["revoked"]), issued, base
    decl.server, decl.p, decl.k, decl.s = d["server"], part.p, part.k, part.s
    return decl


def read_envelope(data):
    """Item 1 and A1-A2 for the bytes of an envelope file. Raises NotCanonical (item 1) or Refused
    (TooLarge, Version)."""
    if len(data) > MAX_READ:
        raise Refused("TooLarge", "%d bytes" % len(data))
    env = canonical_envelope(data)
    if len(data) > MAX_ENVELOPE:
        raise Refused("TooLarge", "%d bytes" % len(data))
    if env["v"] != 1:
        raise Refused("Version", "envelope v %d" % env["v"])
    e = Envelope()
    e.pol = Part(env["k"], env["p"], b64_exact(env["s"], 64))
    e.root = Part(env["root"]["k"], env["root"]["p"], b64_exact(env["root"]["s"], 64)) if "root" in env else None
    e.data, e.P, e.D = data, env["P"], env.get("D")
    return e


# ---- builds, state, derived values (spec 1-4, 1-5) -----------------------------------------------


class Build:
    """What a build pins: the server name, signing keys and root keys by number, the floor."""

    def __init__(self, server, signing, roots, floor):
        self.server, self.signing, self.roots, self.floor = server, dict(signing), dict(roots), tuple(floor)

    def with_floor(self, floor):
        return Build(self.server, self.signing, self.roots, floor)

    def fingerprint(self):
        """The key set's fingerprint (spec 1-4): SHA-256 of "hjv-update-keys-v1", the server, the
        signing keys and the root keys, each key line sorted, NUL between them."""
        line = lambda keys: ",".join("%d:%s" % (k, base64.b64encode(v).decode()) for k, v in sorted(keys.items()))
        return hashlib.sha256(b"\0".join(x.encode() for x in (
            "hjv-update-keys-v1", self.server, line(self.signing), line(self.roots)))).hexdigest()


class Held:
    """The held policy: its envelope's policy part and root part, the parsed policy."""

    __slots__ = ("P", "k", "root", "decl_text")

    def __init__(self, P, k, root, decl_text):
        self.P, self.k, self.root, self.decl_text = P, k, root, decl_text


class State:
    __slots__ = ("decl", "held")

    def __init__(self, decl=None, held=None):
        self.decl, self.held = decl, held

    def __eq__(self, other):
        def key(st):
            return (None if st.decl is None else (st.decl.p, st.decl.k, st.decl.s),
                    None if st.held is None else (st.held.P.p, st.held.k))
        return isinstance(other, State) and key(self) == key(other)


class Derived:
    pass


def derived(build, st):
    """The table of 1-5."""
    d = Derived()
    fe, fs = build.floor
    D_, h = st.decl, st.held
    d.DE = D_.epoch if D_ else 1
    d.E = max(fe, d.DE)
    d.REVOKED = D_.revoked if D_ else frozenset()
    d.CUR = h if h and h.P.epoch == d.E else None
    d.DCUR = D_ if D_ and D_.epoch == d.E else None
    if d.CUR and (d.E, d.CUR.P.seq) >= (fe, fs):
        d.LOW, d.SAME = d.CUR.P.seq, (d.CUR.P.seq, d.CUR.P.h)
    else:
        d.SAME = None
        d.LOW = fs - 1 if fe == d.E and fs >= 1 else 0
    if d.CUR:
        d.BASIS = (d.CUR.P.entries, d.CUR.P.issued)
    elif d.DCUR:
        d.BASIS = (d.DCUR.base, d.DCUR.issued)
    else:
        d.BASIS = None
    if h and h.P.epoch == d.DE and h.k not in d.REVOKED:
        d.EFF = ("policy", h.P)
    elif D_:
        d.EFF = ("base", D_)
    else:
        d.EFF = None
    d.MAY_INSTALL = d.SAME is not None and h.k not in d.REVOKED
    return d


# ---- COVERS, RELAXES, RELAXED (spec 2-3) ---------------------------------------------------------


def lht(e):
    return (e.gm, e.min, e.gu) if e.gu is not None else (e.min, e.min, None)


def covers(n, o):
    """Entry n asks at least what entry o asks, at every time."""
    nlo, nhi, nT = lht(n)
    olo, ohi, oT = lht(o)
    return nlo >= ohi or (nlo >= olo and nhi >= ohi and (nT is None or (oT is not None and nT <= oT)))


def relaxed(B, Pe):
    """RELAXED(B, P): the entries for which P asks less than B (clauses 1-3)."""
    out = set()
    for c, o in B.items():
        n = Pe.get(c)
        if n is None or not covers(n, o) or n.latest < o.latest:
            out.add(c)
    for c in ANDROID:
        sib = ANDROID[1] if c == ANDROID[0] else ANDROID[0]
        if c in Pe and c not in B and sib in B and not covers(Pe[c], B[sib]):
            out.add(c)
    return out


# ---- accept (spec 2-2) and LOAD of an envelope (1-6) ---------------------------------------------


def check_root(build, verifier, part, d):
    """B1-B3 for a root part."""
    if part.k not in build.roots:
        raise Refused("RootKey", "key %d" % part.k)
    if not verifier.verify(build.roots[part.k], ROOT_CONTEXT, part.p, part.s):
        raise Refused("RootSignature", "root key %d" % part.k)
    return parse_declaration(part, d, build.server, build.roots)


def accept(build, st, env, verifier):
    """accept(build, state, envelope) -> (state', outcome) with outcome "taken", "unchanged" or a
    Refused. `env` passed item 1 and A1-A2; the strict checks of item 2 are inside B2 and C2."""
    S1 = st
    D2 = None
    if env.root is not None:
        try:
            D2 = check_root(build, verifier, env.root, env.D)
        except Refused as r:
            return st, r
        d = derived(build, st)
        if D2.epoch < d.E:
            return st, Refused("RootEpoch", "the declaration's epoch %d, the device's %d" % (D2.epoch, d.E))
        if D2.epoch == d.E and d.DCUR is not None:
            if D2.p != d.DCUR.p:
                return st, Refused("RootFork", "epoch %d" % D2.epoch)
        else:
            S1 = State(D2, st.held)
    d = derived(build, S1)
    k = env.pol.k
    key = build.roots.get(k) or build.signing.get(k)
    if key is None:
        return S1, Refused("UnknownKey", "key %d" % k)
    if k in d.REVOKED:
        return S1, Refused("RevokedKey", "key %d" % k)
    if not verifier.verify(key, POLICY_CONTEXT, env.pol.p, env.pol.s):
        return S1, Refused("BadSignature", "key %d" % k)
    try:
        Pn = parse_policy(env.pol.p, env.P, build.server)
    except Refused as r:
        return S1, r
    if Pn.epoch != d.E:
        return S1, Refused("Epoch", "the policy's epoch %d, the device's %d" % (Pn.epoch, d.E))
    if Pn.epoch >= 2 and (env.root is None or D2.epoch != Pn.epoch):
        return S1, Refused("Epoch", "a policy of epoch %d comes with its epoch's declaration" % Pn.epoch)
    if d.SAME is not None and Pn.seq == d.SAME[0]:
        if Pn.h == d.SAME[1]:
            return S1, "unchanged"
        return S1, Refused("Fork", "seq %d" % Pn.seq)
    if Pn.seq <= d.LOW:
        return S1, Refused("Rollback", "seq %d, the device is at %d" % (Pn.seq, d.LOW))
    if Pn.seq - d.LOW > STEP:
        return S1, Refused("Jump", "seq %d, the device is at %d" % (Pn.seq, d.LOW))
    if (d.E, Pn.seq) < build.floor:
        return S1, Refused("BelowFloor")
    if d.BASIS is not None:
        if Pn.issued < d.BASIS[1]:
            return S1, Refused("Issued", "%s, before %s" % (format_utc(Pn.issued), format_utc(d.BASIS[1])))
        names = relaxed(d.BASIS[0], Pn.entries)
        if names:
            return S1, Refused("Relaxation", "entries " + ",".join(n for n in NAMES if n in names))
    return State(S1.decl, Held(Pn, k, env.root, None if env.root is None else env.root.p)), "taken"


def holding(build, env, verifier):
    """LOAD (1-6, L2-L5) of a record whose declaration is `env`'s root part and whose envelope is
    `env`: the state of a device that holds `env`. Item 4 and the vectors' state checks use it."""
    return load(build, env, verifier)[0]


def load(build, env, verifier):
    """holding(), and when it holds no policy, why: (text, keys), `keys` true when the key file pins
    no key that signed the envelope (the key file is replaced too early: runbook 8-5), false when
    the envelope is not one the apps hold whatever the keys (it did not pass this check)."""
    decl, why = None, None
    if env.root is not None:
        if env.root.k not in build.roots:
            why = ("it pins no root key %d, which signed the declaration" % env.root.k, True)
        else:
            try:
                decl = check_root(build, verifier, env.root, env.D)
            except Refused as r:
                why = ("its declaration: %s" % r.text(), False)
    held = None
    key = build.roots.get(env.pol.k) or build.signing.get(env.pol.k)
    if why is not None:
        pass
    elif key is None:
        why = ("it pins no key %d, which signed the policy" % env.pol.k, True)
    elif not verifier.verify(key, POLICY_CONTEXT, env.pol.p, env.pol.s):
        why = ("the policy's signature does not hold (strictly) for key %d" % env.pol.k, False)
    else:
        try:
            Pn = parse_policy(env.pol.p, env.P, build.server)
        except Refused as r:
            why = ("its policy: %s" % r.text(), False)
        else:
            # L4: a policy of epoch 1 comes alone, one of a later epoch with that epoch's declaration
            if Pn.epoch == 1 and env.root is not None:
                why = ("a policy of epoch 1 with a declaration (of epoch %d)" % decl.epoch, False)
            elif Pn.epoch >= 2 and (decl is None or decl.epoch != Pn.epoch):
                why = ("a policy of epoch %d without that epoch's declaration%s" % (
                    Pn.epoch, "" if decl is None else " (it comes with epoch %d's)" % decl.epoch), False)
            else:
                held = Held(Pn, env.pol.k, env.root, None if env.root is None else env.root.p)
    return State(decl, held), why


def summary(build, st):
    """A state in the vectors' words (spec 7-4)."""
    d = derived(build, st)
    sha = lambda text: hashlib.sha256(text.encode("utf-8")).hexdigest()
    return {
        "epoch": d.E,
        "seq": None if st.held is None else st.held.P.seq,
        "revoked": sorted(d.REVOKED),
        "p_sha256": None if st.held is None else sha(st.held.P.p),
        "root_sha256": None if st.decl is None else sha(st.decl.p),
        "root_k": None if st.decl is None else st.decl.k,
        "enforces": "nothing" if d.EFF is None else d.EFF[0],
        "may_install": d.MAY_INSTALL,
    }


# ---- the key file -----------------------------------------------------------------------------------

_UPDATE_LINE = re.compile(r"(HJV_UPDATE_PUBKEY|HJV_UPDATE_ROOT_PUBKEY|HJV_POLICY_FLOOR)=([0-9A-Za-z+/=:,]*)")
_OTHER_LINE_BREAK = re.compile("[\x0b\x0c\x1c\x1d\x1e\x85\u2028\u2029]")


def read_file(path, most=MAX_READ):
    with open(path, "rb") as f:
        return f.read(most + 1)


def _key_list(name, text):
    keys = {}
    for entry in text.split(","):
        kid, sep, b64 = entry.partition(":")
        if not sep or not re.fullmatch(r"[1-9][0-9]{0,4}", kid) or int(kid) > 65535:
            raise CannotRun("%s: an entry is not <key number>:<base64> with a key number 1-65535" % name)
        raw = b64_exact(b64, 32)
        if raw is None:
            raise CannotRun("%s key %s is not the standard base64 of 32 bytes" % (name, kid))
        problem = key_problem(raw)
        if problem:
            raise CannotRun("%s key %s is %s: the apps pin no key of this file" % (name, kid, problem))
        if int(kid) in keys:
            raise CannotRun("%s: key number %s twice" % (name, kid))
        keys[int(kid)] = raw
    if len(keys) > 2:
        raise CannotRun("%s holds more than two keys" % name)
    return keys


def read_keys(path, server):
    """The build of client/update-keys.env (the apps' pinned() rules; the form of the file as
    scripts/check-build-keys.py reads it)."""
    try:
        if os.path.islink(path) or not os.path.isfile(path):
            raise CannotRun("%s is not a file: install client/update-keys.env of the release tag there" % path)
        data = read_file(path, 65536)
    except OSError as e:
        raise CannotRun("%s cannot be read (%s)" % (path, e.strerror))
    if len(data) > 65536:
        raise CannotRun("%s is larger than 64 KiB" % path)
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        raise CannotRun("%s is not UTF-8 text" % path)
    if text.startswith("\ufeff") or "\r" in text:
        raise CannotRun("%s has a byte order mark or CR line endings; it must be the tag's file as it is" % path)
    values = {}
    for n, line in enumerate(text.split("\n"), 1):
        if _OTHER_LINE_BREAK.search(line):
            raise CannotRun("%s:%d holds a character that other programs take for a line break" % (path, n))
        if not line or line.startswith("#"):
            continue
        m = _UPDATE_LINE.fullmatch(line)
        if m is None:
            raise CannotRun("%s:%d is neither a # comment nor one of %s with its value" % (
                path, n, ", ".join(name + "=" for name in UPDATE_NAMES)))
        if m.group(1) in values:
            raise CannotRun("%s:%d sets %s a second time" % (path, n, m.group(1)))
        values[m.group(1)] = m.group(2)
    for name in UPDATE_NAMES:
        if name not in values:
            raise CannotRun("%s has no %s= line" % (path, name))
    if not values["HJV_UPDATE_PUBKEY"] or not values["HJV_UPDATE_ROOT_PUBKEY"]:
        raise CannotRun("%s pins no signing key or no root key: apps built from it check no policy, and "
                        "nothing can be verified with it" % path)
    signing = _key_list("HJV_UPDATE_PUBKEY", values["HJV_UPDATE_PUBKEY"])
    roots = _key_list("HJV_UPDATE_ROOT_PUBKEY", values["HJV_UPDATE_ROOT_PUBKEY"])
    numbers = list(signing) + list(roots)
    keys = list(signing.values()) + list(roots.values())
    if len(set(numbers)) != len(numbers) or len(set(keys)) != len(keys):
        raise CannotRun("%s: a key number or a key appears twice among the signing and root keys" % path)
    floor = values["HJV_POLICY_FLOOR"] or "1:0"
    m = re.fullmatch(r"([1-9][0-9]{0,14}):(0|[1-9][0-9]{0,14})", floor)
    if m is None:
        raise CannotRun("%s: HJV_POLICY_FLOOR is not <epoch>:<seq>" % path)
    return Build(server, signing, roots, (int(m.group(1)), int(m.group(2))))


def server_from_env(path):
    try:
        text = read_file(path, 65536).decode("utf-8")
    except (OSError, UnicodeDecodeError):
        return None
    for line in text.splitlines():
        m = re.fullmatch(r"HJV_DOMAIN=(\S+)", line.strip())
        if m:
            return m.group(1)
    return None


# ---- the install check (spec 4-8) ------------------------------------------------------------------


def describe_env(env):
    s = "epoch %d, seq %d, signed by key %d" % (env.P["epoch"], env.P["seq"], env.pol.k)
    if env.root is None:
        return s + ", no declaration"
    return s + ", with the declaration of epoch %s by root key %d" % (env.D.get("epoch"), env.root.k)


def find_package(dirs, name, size, sha256):
    """Item 8 for one package: where it lies, or why not."""
    if name.startswith("."):
        raise Refused("Package", "%s: a package name that starts with a dot" % name)
    for directory in dirs:
        path = os.path.join(directory, name)
        try:
            st = os.lstat(path)
        except FileNotFoundError:
            continue
        except OSError as e:
            raise Refused("Package", "%s cannot be read (%s)" % (path, e.strerror))
        if not stat.S_ISREG(st.st_mode):
            raise Refused("Package", "%s is not a plain file (hjv-api hands out no link or directory)" % path)
        if st.st_size != size:
            raise Refused("Package", "%s has %d bytes, the policy says %d" % (path, st.st_size, size))
        digest = hashlib.sha256()
        with open(path, "rb") as f:
            for chunk in iter(lambda: f.read(1 << 20), b""):
                digest.update(chunk)
        if digest.hexdigest() != sha256:
            raise Refused("Package", "%s has another SHA-256 than the policy names" % path)
        return path
    raise Refused("Package", "%s is in none of %s" % (name, ", ".join(dirs)))


def install_check(build, new_data, live_data, relaxes, package_dirs, now, verifier, say):
    """Items 1-8 of spec 4-8 for the bytes of the new envelope, and of the live one (None: the
    first install), with hjv-api's reading in item 3. Raises Refused; returns "install" or
    "unchanged"."""

    def unreadable(why):
        return Refused("LiveUnreadable", "the live file is not one this check reads (%s). Leave it where it is "
                       "and check the new file against the last one installed, with --live (runbook 8-5)" % why)

    # 1. the canonical form, before anything else is read
    try:
        new = read_envelope(new_data)
    except NotCanonical as e:
        raise Refused("Canonical", "%s; this check reads only files exactly as hjv-release writes them" % e)
    say("new:   %s" % describe_env(new))
    # 2. strict signatures
    for what, part in (("the policy part", new.pol), ("the root part", new.root)):
        if part is not None and weak_signature(part.s):
            why = weak_signature(part.s)
            raise Refused("WeakSignature", "the signature of %s: %s. The apps (libsodium) refuse it whatever "
                          "it signs%s" % (what, why, "; OpenSSL alone would take it" if "R" in why else ""))
    # 3. a fresh install of the newest build
    fresh, out = accept(build, State(), new, verifier)
    if isinstance(out, Refused):
        out.detail = "a fresh install of the newest build (floor %d:%d) refuses it: %s" % (
            build.floor[0], build.floor[1], out.detail)
        raise out
    say("a fresh install of the newest build (floor %d:%d) takes it" % build.floor)
    problem = api_problem(new.P["platforms"])
    if problem:
        raise Refused("HjvApi", "%s: hjv-api would take the whole file for no policy, and door 1, door 2 and "
                      "the short links would answer 404" % problem)
    say("hjv-api reads its windows and android entries")
    nP = fresh.held.P
    new_decl = None if new.root is None else new.root.p
    live, live_outcome = None, None
    if live_data is not None:
        # 4. a device that holds the live envelope
        try:
            live = read_envelope(live_data)
        except (NotCanonical, Refused) as e:
            raise unreadable(e if isinstance(e, NotCanonical) else e.text())
        say("live:  %s" % describe_env(live))
        holder, missing = load(build, live, verifier)
        if holder.held is None:
            text, keys = missing
            if not keys:
                raise unreadable(text)
            raise Refused("LiveUnreadable", "the key file does not verify the live file (%s): replace the key "
                          "file only after the live policy is signed by a key it pins (runbook 8-5)" % text)
        _, live_outcome = accept(build, holder, new, verifier)
        if isinstance(live_outcome, Refused):
            hint = ""
            if live_outcome.code in ("Rollback", "Fork", "Relaxation"):
                hint = ". If the live file is not one you signed, a signing key may be stolen (runbook 8-5)"
            live_outcome.detail = "a device that holds the live policy refuses it: %s%s" % (
                live_outcome.detail, hint)
            raise live_outcome
        say("a device that holds the live policy %s" % (
            "has it already: it is the live file" if live_outcome == "unchanged" else "takes it"))
    live_decl = None if live is None or live.root is None else live.root.p
    differs = new_decl is not None and new_decl != live_decl
    named = set(relaxes)
    if named and (live is None or not differs):
        raise Refused("LiveRelaxes", "--relaxes names %s, but %s" % (
            ",".join(n for n in NAMES if n in named),
            "there is no live policy to relax" if live is None
            else "this envelope carries no new declaration: is it the file you meant?"))
    if differs and live is not None:
        # 5. a new declaration resets every device's basis: compare with the live policy
        lP = holder.held.P
        if nP.issued < lP.issued:
            raise Refused("LiveIssued", "the new policy is issued %s, before the live one (%s)" % (
                format_utc(nP.issued), format_utc(lP.issued)))
        found = relaxed(lP.entries, nP.entries)
        if found != named:
            raise Refused("LiveRelaxes", "against the live policy it relaxes {%s}; --relaxes names {%s}. A "
                          "declaration made from a stale or forged --prev shows here; if the live file is not "
                          "one you signed, a signing key may be stolen (runbook 8-5)" % (
                              ",".join(n for n in NAMES if n in found), ",".join(n for n in NAMES if n in named)))
        say("the new declaration relaxes {%s} against the live policy, as --relaxes names"
            % ",".join(n for n in NAMES if n in found))
    # 6. nothing dated ahead of this server's clock
    if nP.issued > now + CLOCK_SLACK:
        raise Refused("Clock", "the policy is issued %s, more than %d s after this server's clock (%s)" % (
            format_utc(nP.issued), CLOCK_SLACK, format_utc(now)))
    if differs and fresh.decl.issued > now + CLOCK_SLACK:
        raise Refused("Clock", "the new declaration is issued %s, more than %d s after this server's clock "
                      "(%s)" % (format_utc(fresh.decl.issued), CLOCK_SLACK, format_utc(now)))
    if differs and live is not None:
        # 7. a new declaration: an epoch above the live policy's, every revocation kept, no root number
        if fresh.decl.epoch <= live.P["epoch"]:
            raise Refused("DeclarationEpoch", "the new declaration's epoch %d is not above the live policy's %d"
                          % (fresh.decl.epoch, live.P["epoch"]))
        kept = frozenset() if live.D is None else frozenset(live.D["revoked"])
        if not kept <= fresh.decl.revoked:
            raise Refused("DeclarationRevoked", "it no longer revokes key(s) %s, which the live declaration "
                          "revokes" % ",".join(str(k) for k in sorted(kept - fresh.decl.revoked)))
        if any(101 <= k <= 199 for k in fresh.decl.revoked):
            raise Refused("DeclarationRevoked", "it revokes a number 101-199: a root key is never revoked, "
                          "since a build that pins it would refuse that declaration and every later one")
    # 8. the packages
    for name in NAMES:
        e = new.P["platforms"].get(name)
        if e is not None and "package" in e:
            pkg = e["package"]
            where = find_package(package_dirs, pkg["file"], pkg["size"], pkg["sha256"])
            say("package of %s: %s, %d bytes, the SHA-256 the policy names" % (name, where, pkg["size"]))
    return "unchanged" if live_outcome == "unchanged" else "install"


# ---- the shared test vectors (spec 7-4) ------------------------------------------------------------


def vectors(directory, verifier, say):
    """Runs every case of <directory>/manifest.json and covers.json; returns the number of failures."""
    try:
        with open(os.path.join(directory, "manifest.json"), encoding="utf-8") as f:
            manifest = json.load(f)
        cases = manifest["cases"]
    except (OSError, ValueError, KeyError, TypeError) as e:
        raise CannotRun("%s/manifest.json is not a manifest of test vectors (%s)" % (directory, e))
    server = manifest.get("server", "rd.example.com")
    build = read_keys(os.path.join(directory, "update-keys.env"), server)
    failures = 0
    cache = {}

    def envelope(path):
        if path not in cache:
            cache[path] = read_file(os.path.join(directory, path))
        return cache[path]

    def run(st, b, path):
        try:
            data = envelope(path)
        except OSError as e:
            return st, Refused("Unreadable", "%s: %s" % (path, e.strerror))
        try:
            env = read_envelope(data)
        except NotCanonical as e:
            return st, Refused("Canonical", str(e))
        except Refused as r:
            return st, r
        return accept(b, st, env, verifier)

    def fail(name, why):
        nonlocal failures
        failures += 1
        say("FAIL %s: %s" % (name, why))

    def compare(name, what, want, st, b):
        got = summary(b, st)
        for key, value in want.items():
            if key not in got:
                fail(name, "%s: no member %s in this check's state" % (what, key))
            elif got[key] != value:
                fail(name, "%s: %s is %s, the manifest says %s" % (what, key, json.dumps(got[key]), json.dumps(value)))

    def outcome_text(out):
        return out.code if isinstance(out, Refused) else out

    counts = {"canonical": 0, "not canonical": 0}
    for case in cases:
        name = case["name"]
        if "canonical" not in case:
            fail(name, "the case has no `canonical` (the vectors of the trust rules' round 2 are needed)")
            continue
        b = build
        if case.get("floor"):
            epoch, seq = case["floor"].split(":")
            b = build.with_floor((int(epoch), int(seq)))
        st = State()
        broken = False
        for path in case.get("after", []):
            st, out = run(st, b, path)
            if isinstance(out, Refused):
                fail(name, "its history refuses %s: %s" % (path, out.text()))
                broken = True
                break
        if broken:
            continue
        before = st
        st2, out = run(st, b, case["file"])
        if not case["canonical"]:
            counts["not canonical"] += 1
            if not (isinstance(out, Refused) and out.code == "Canonical"):
                fail(name, "not canonical, but this check reads it: %s" % outcome_text(out))
            continue
        counts["canonical"] += 1
        if isinstance(out, Refused) and out.code == "Canonical":
            fail(name, "canonical, but this check does not read it: %s" % out.detail)
            continue
        # what the apps do: the manifest's outcome, reason and state, or app_expect for binding tool
        want_state = case.get("state_after")
        if case.get("binding", "app") == "tool":
            # the tool alone refuses it; app_expect is what the apps, and so this check, do
            expect = case.get("app_expect", "accept")
            if isinstance(expect, dict):
                want_state = expect.get("state_after", want_state)
                expect = expect.get("expect", expect.get("outcome", "accept"))
            if expect == "reject":
                want_state = None
            want_refused, want_reason = expect == "reject", None
            want_outcome = expect if expect in ("taken", "unchanged") else None
        else:
            want_reason, want_outcome = case.get("reason"), case.get("outcome")
            if want_reason is None and want_outcome is None:
                if case.get("expect") not in ("accept", "reject"):
                    fail(name, "the case names no outcome, reason or expect")
                    continue
                want_refused = case["expect"] == "reject"
            else:
                want_refused = want_reason is not None
        if isinstance(out, Refused) != want_refused:
            fail(name, "this check gives %s, the manifest says %s" % (
                outcome_text(out), want_reason or want_outcome or case.get("expect") or case.get("app_expect")))
            continue
        if isinstance(out, Refused):
            if want_reason is not None and out.code != want_reason:
                fail(name, "refused as %s, the manifest says %s" % (out.code, want_reason))
                continue
            if "state_after_refusal" in case:
                compare(name, "state_after_refusal", case["state_after_refusal"], st2, b)
            elif st2 != before:
                fail(name, "the refusal changed the state, the manifest says it does not")
        else:
            if want_outcome is not None and out != want_outcome.lower():
                fail(name, "%s, the manifest says %s" % (out, want_outcome))
                continue
            if want_state:
                compare(name, "state_after", want_state, st2, b)
        then = case.get("then_accept")
        if then:
            path = then if isinstance(then, str) else then.get("file") or then.get("envelope")
            _, out2 = run(st2, b, path)
            want = case.get("then_outcome") if isinstance(then, str) else then.get("outcome")
            if isinstance(out2, Refused):
                fail(name, "then_accept %s is refused: %s" % (path, out2.text()))
            elif want is not None and out2 != str(want).lower():
                fail(name, "then_accept %s: %s, the manifest says %s" % (path, out2, want))
    say("vectors: %d cases (%d canonical, %d not canonical)" % (len(cases), counts["canonical"], counts["not canonical"]))
    covers_path = os.path.join(directory, "covers.json")
    if os.path.exists(covers_path):
        with open(covers_path, encoding="utf-8") as f:
            rows = json.load(f)
        bad = 0
        for row in rows:
            n, o = (Entry(version_number(e["min"]), version_number(e.get("latest", e["min"])),
                          None if "grace_until" not in e else parse_utc(e["grace_until"]),
                          None if "grace_min" not in e else version_number(e["grace_min"]))
                    for e in (row["new"], row["old"]))
            if covers(n, o) != row["covers"]:
                bad += 1
                if bad <= 10:
                    say("FAIL covers.json: COVERS(%s; %s) is %s here" % (json.dumps(row["new"]), json.dumps(row["old"]),
                                                                       covers(n, o)))
        failures += bad
        say("covers.json: %d pairs, %d disagree" % (len(rows), bad))
    else:
        fail("covers.json", "missing")
    return failures


# ---- the command ---------------------------------------------------------------------------------


USAGE = """usage: %(prog)s [--keys FILE] [--live FILE] [--server NAME] [--relaxes LIST]
          [--packages DIR]... <new envelope>
       %(prog)s --vectors DIR
       %(prog)s --self-test
Checks a signed version policy before it is installed (docs/server-runbook.md 8)."""


def main(argv):
    prog = os.path.basename(argv[1]) if len(argv) > 1 else "hjv-update-verify.sh"
    args = argv[2:]
    opts = {"keys": DEFAULT_KEYS, "live": DEFAULT_LIVE, "server": None, "relaxes": "", "packages": [],
            "vectors": None, "now": None}
    files = []
    i = 0
    while i < len(args):
        a = args[i]
        if a in ("-h", "--help"):
            print(USAGE % {"prog": prog})
            return 0
        if a == "--self-test":
            opts["self_test"] = True
            i += 1
            continue
        name = a[2:].split("=", 1)[0] if a.startswith("--") else None
        if name in ("keys", "live", "server", "relaxes", "packages", "vectors", "now"):
            if "=" in a:
                value = a.split("=", 1)[1]
            elif i + 1 < len(args):
                i += 1
                value = args[i]
            else:
                print("%s: %s needs a value\n%s" % (prog, a, USAGE % {"prog": prog}), file=sys.stderr)
                return 2
            if name == "packages":
                opts["packages"].append(value)
            else:
                opts[name] = value
        elif a.startswith("-") and a != "-":
            print("%s: unknown option %s\n%s" % (prog, a, USAGE % {"prog": prog}), file=sys.stderr)
            return 2
        else:
            files.append(a)
        i += 1
    say = lambda line: print(line, flush=True)
    verifier = Verifier()
    try:
        verifier.self_test()
        if opts.get("self_test"):
            if files or opts["vectors"]:
                print("%s: --self-test takes nothing else" % prog, file=sys.stderr)
                return 2
            say("%s: python3 %s and %s verify Ed25519 as the apps need" % (
                prog, sys.version.split()[0], openssl_version()))
            return 0
        if opts["vectors"]:
            if files:
                print("%s: --vectors takes no envelope" % prog, file=sys.stderr)
                return 2
            failures = vectors(opts["vectors"], verifier, say)
            say("%s: %s" % (prog, "the vectors agree" if failures == 0 else "%d failure(s)" % failures))
            return 0 if failures == 0 else 1
        if len(files) != 1:
            print(USAGE % {"prog": prog}, file=sys.stderr)
            return 2
        relaxes = [n for n in opts["relaxes"].split(",") if n]
        for n in relaxes:
            if n not in NAMES:
                print("%s: --relaxes names %s, not one of %s" % (prog, n, ",".join(NAMES)), file=sys.stderr)
                return 2
        server = opts["server"] or server_from_env(DEFAULT_ENV)
        if not server:
            raise CannotRun("no server name: give --server, or HJV_DOMAIN in %s" % DEFAULT_ENV)
        if opts["now"] is None:
            now = int(time.time())
        else:
            now = parse_utc(opts["now"]) if not opts["now"].isdigit() else int(opts["now"])
            if now is None:
                raise CannotRun("--now is not a time")
        build = read_keys(opts["keys"], server)
        new_path = files[0]
        try:
            new_data = read_file(new_path)
        except OSError as e:
            raise CannotRun("%s cannot be read (%s)" % (new_path, e.strerror))
        live_data = None
        try:
            live_data = read_file(opts["live"])
        except FileNotFoundError:
            live_data = None
        except OSError as e:
            raise CannotRun("%s cannot be read (%s)" % (opts["live"], e.strerror))
        dirs = opts["packages"] or [os.path.join(os.path.dirname(os.path.abspath(new_path)), "pkg"), DEFAULT_PKG]
        say("%s: %s" % (prog, new_path))
        say("file SHA-256: %s, %d bytes" % (hashlib.sha256(new_data).hexdigest(), len(new_data)))
        say("key set %s: signing keys %s, root keys %s, floor %d:%d, server %s" % (
            build.fingerprint(), ",".join(map(str, sorted(build.signing))), ",".join(map(str, sorted(build.roots))),
            build.floor[0], build.floor[1], server))
        say("live file: %s" % (opts["live"] if live_data is not None else "none (the first install)"))
        try:
            result = install_check(build, new_data, live_data, relaxes, dirs, now, verifier, say)
        except Refused as r:
            say("REFUSED %s" % r.text())
            live_dir = os.path.dirname(os.path.abspath(opts["live"]))
            if (os.path.dirname(os.path.abspath(new_path)) == live_dir
                    and os.path.abspath(new_path) != os.path.abspath(opts["live"])):
                try:
                    os.unlink(new_path)
                    say("removed %s: a refused file does not stay beside the live one" % new_path)
                except OSError as e:
                    say("could not remove %s (%s): remove it by hand" % (new_path, e.strerror))
            return 1
        if result == "unchanged":
            say("OK, nothing to install: it is the live file")
        else:
            say("OK: install it, the packages first and the policy last (runbook 8-2)")
        return 0
    except CannotRun as e:
        print("%s: cannot check: %s" % (prog, e), file=sys.stderr)
        return 2
    except Exception as e:  # a fault of this script: never mistaken for a refusal (exit 1)
        print("%s: cannot check: an internal error (%s: %s)" % (prog, type(e).__name__, e), file=sys.stderr)
        return 2
    finally:
        verifier.close()


if __name__ == "__main__":
    sys.exit(main(sys.argv))
PYTHON

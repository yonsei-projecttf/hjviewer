#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Makes the shared test vectors of HJViewer's access tickets, and checks them.

The formats are docs/access-tickets.md: hjv-api signs a ticket, hbbs checks it at a punch hole
request, a relay request and a registration, and the app only carries it. This script writes
access-tickets.json next to it: the test keys, the rule the cases are judged under, and for every
case the ticket and what hbbs decides at each check point; then RegisterPk messages that carry a
ticket in field 1001, and access.conf files with what hbbs reads from them.

    python3 -I -B access-tickets.py          # writes access-tickets.json
    python3 -I -B access-tickets.py --check  # makes it in memory and compares it with the file

Every expected decision is written out in the case tables below and checked against this
script's reading of the document (decide(), parse_conf(), register_ticket()): the two must agree,
or the script stops. Ed25519 (RFC 8032) comes from PyNaCl when it is installed, otherwise from the
openssl command line (OpenSSL 3, pkeyutl -rawin). Its signatures are deterministic, so either
writes the same file. Test keys only: every seed here is a public test value
(scripts/public-test-keys.txt).
"""

import argparse
import base64
import calendar
import datetime
import hashlib
import hmac
import json
import os
import subprocess
import sys
import tempfile
import unicodedata

sys.dont_write_bytecode = True          # no __pycache__ in the repository

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "access-tickets.json")

# ------------------------------------------------------------------------------------------------
# The format (docs/access-tickets.md 2, 4 and 6)
# ------------------------------------------------------------------------------------------------

DOMAIN = b"hjv-ticket-v1"
PREFIX = "t1"
MAX_TICKET_BYTES = 1024
MAX_KID = 65535
SKEW = 60
MAX_LIFETIME = 7 * 24 * 3600
MAX_TO = 3
U64_MAX = 2 ** 64 - 1
MEMBERS = ("v", "who", "use", "sub", "id", "pk", "to", "iat", "exp")
USES = {"staff": ("ctl", "pc", "both"), "review": ("ctl",), "guest": ("ctl", "pc")}
# how many entries `to` has, by (who, use)
TO_COUNT = {("staff", "ctl"): (0, 0), ("staff", "pc"): (0, 0), ("staff", "both"): (0, 0),
            ("review", "ctl"): (1, MAX_TO), ("guest", "ctl"): (1, 1), ("guest", "pc"): (0, 0)}
POINTS = {"punch_hole": ("ctl", "both"), "request_relay": ("ctl", "both"),
          "register_pk": ("pc", "both")}
REASONS = ("ok", "missing", "oversize", "malformed", "unknown_kid", "bad_signature", "wrong_use",
           "expired", "not_yet_valid", "revoked", "wrong_pc", "wrong_target")
FIELD_HJV_TICKET = 1001

MARK = "access not allowed"
VERSION_MARK = "version not allowed"
MAX_TEXT_CHARS = 2000
MAX_CONF_BYTES = 64 * 1024
CONF_KEYS = {"MODE": 1, "KEY": 2, "NOT_BEFORE": 1, "TEXT": 1}
BUILTIN_TEXT = ("HJViewer는 연세대학교 교직원용입니다. 연세포탈로 로그인하거나 이용 비밀번호를 넣어야 "
                "연결할 수 있습니다. HJViewer is for Yonsei University staff: sign in with a Yonsei "
                "portal account or enter the access password to connect. (access not allowed)")

STD = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
URL = STD[:62] + "-_"
ID_CHARS = set(STD[:62] + "-_")
HEX = set("0123456789abcdef")
# char::is_whitespace of Rust (Unicode White_Space), which hbbs trims lines with
WHITE = set("\t\n\x0b\x0c\r \x85\xa0\u1680\u2028\u2029\u202f\u205f\u3000"
            + "".join(chr(c) for c in range(0x2000, 0x200b)))

# ------------------------------------------------------------------------------------------------
# The values of the vectors
# ------------------------------------------------------------------------------------------------

HOUR = 3600
IAT = 1760000000                      # 2025-10-09T08:53:20Z, when the valid tickets were issued
NOW = IAT + HOUR                      # when hbbs judges them
NOT_BEFORE_TEXT = "2025-10-09T00:00:00Z"
NOT_BEFORE = 1759968000
STAFF_LIFE, PC_LIFE, GUEST_LIFE = 24 * HOUR, 72 * HOUR, 12 * HOUR

PC_ID = "123456789"                   # the PC that registers, and the usual target
OTHER_ID = "222333444"
DEMO = ("100200300", "100200301")     # the App Review account's demo PCs (review.conf PC=)
CUSTOM_ID = "Office-PC_7"
UUID = b"vector-uuid-0001"

SEEDS = {
    "ticket_1": bytes([0x74]) * 32,   # KEY=1
    "ticket_2": bytes([0x75]) * 32,   # KEY=2
    "ticket_3": bytes([0x76]) * 32,   # in no KEY line
    "pc": bytes([0x02]) * 32,         # the PC's device key (pc_device of tests/data/v3-vectors.json)
    "other_pc": bytes([0x77]) * 32,
}
KIDS = {"ticket_1": 1, "ticket_2": 2, "ticket_3": 3}
LOG_KEY = bytes([0x6c]) * 32          # the log key whose tags stand in `sub`
STAFF_LOGIN = "zztest0001"
REVIEW_LOGIN = "review-demo"

# ------------------------------------------------------------------------------------------------
# Ed25519
# ------------------------------------------------------------------------------------------------


class Nacl:
    name = "PyNaCl"

    def __init__(self, nacl):
        self.nacl = nacl

    def public_key(self, seed):
        return bytes(self.nacl.signing.SigningKey(seed).verify_key)

    def sign(self, seed, message):
        return bytes(self.nacl.signing.SigningKey(seed).sign(message).signature)

    def verify(self, pk, message, sig):
        try:
            self.nacl.signing.VerifyKey(pk).verify(message, sig)
        except (self.nacl.exceptions.CryptoError, ValueError):
            return False
        return True


class Openssl:
    name = "openssl"
    PKCS8 = bytes.fromhex("302e020100300506032b657004220420")   # an Ed25519 private key, then the seed
    SPKI = bytes.fromhex("302a300506032b6570032100")             # an Ed25519 public key, then the key

    def __init__(self, tmp):
        self.tmp = tmp
        self.files = 0

    def file(self, data):
        self.files += 1
        path = os.path.join(self.tmp, "f%d" % self.files)
        with open(path, "wb") as f:
            f.write(data)
        return path

    @staticmethod
    def run(*args):
        try:
            return subprocess.run(["openssl", *args], capture_output=True, check=False)
        except FileNotFoundError:
            sys.exit("neither PyNaCl nor the openssl command is there")

    def public_key(self, seed):
        done = self.run("pkey", "-inform", "DER", "-in", self.file(self.PKCS8 + seed), "-pubout",
                        "-outform", "DER")
        if done.returncode != 0 or len(done.stdout) != 44 or not done.stdout.startswith(self.SPKI):
            sys.exit("openssl pkey failed: %s" % done.stderr.decode(errors="replace").strip())
        return done.stdout[len(self.SPKI):]

    def sign(self, seed, message):
        # OpenSSL 3.0 cannot sign an empty input with -rawin; no message here is empty
        done = self.run("pkeyutl", "-sign", "-inkey", self.file(self.PKCS8 + seed), "-keyform",
                        "DER", "-rawin", "-in", self.file(message))
        if done.returncode != 0 or len(done.stdout) != 64:
            sys.exit("openssl pkeyutl -sign failed: %s" % done.stderr.decode(errors="replace").strip())
        return done.stdout

    def verify(self, pk, message, sig):
        done = self.run("pkeyutl", "-verify", "-pubin", "-inkey", self.file(self.SPKI + pk),
                        "-keyform", "DER", "-rawin", "-in", self.file(message), "-sigfile",
                        self.file(sig))
        return done.returncode == 0 and b"Verified Successfully" in done.stdout


def ed25519(tmp):
    try:
        import nacl.exceptions
        import nacl.signing
    except ImportError:
        ed = Openssl(tmp)
    else:
        ed = Nacl(nacl)
    # RFC 8032 7.1, TEST 2
    seed = bytes.fromhex("4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb")
    pk = bytes.fromhex("3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c")
    sig = bytes.fromhex("92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da"
                        "085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00")
    if (ed.public_key(seed) != pk or ed.sign(seed, b"\x72") != sig or not ed.verify(pk, b"\x72", sig)
            or ed.verify(pk, b"\x73", sig)):
        sys.exit("%s does not give RFC 8032's test vector" % ed.name)
    return ed


# ------------------------------------------------------------------------------------------------
# Encodings
# ------------------------------------------------------------------------------------------------


def b64(data):
    return base64.b64encode(data).decode()


def b64url(data):
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()


def b64url_decode(text):
    """Unpadded base64url read strictly: only its 64 letters, and the text it encodes back to."""
    if not text or any(c not in URL for c in text) or len(text) % 4 == 1:
        return None
    raw = base64.urlsafe_b64decode(text + "=" * (-len(text) % 4))
    return raw if b64url(raw) == text else None


def key_32(text):
    """The 32 bytes of a key written in standard base64 with padding (44 characters), or None."""
    if len(text) != 44 or text[43] != "=" or any(c not in STD for c in text[:43]):
        return None
    raw = base64.b64decode(text)
    return raw if b64(raw) == text else None


def valid_id(text):
    return 1 <= len(text) <= 64 and all(c in ID_CHARS for c in text)


def kid_number(text):
    if not 1 <= len(text) <= 5 or not all("0" <= c <= "9" for c in text) or text[0] == "0":
        return None
    kid = int(text)
    return kid if kid <= MAX_KID else None


def tag(kind, value):
    """hjv-api's Tagger::tag (server/api/src/log.rs) with the vectors' log key."""
    mac = hmac.new(LOG_KEY, kind.encode() + b"\0" + value.encode(), hashlib.sha256)
    return mac.hexdigest()[:16]


def canonical(t):
    """The payload text of the values `t`: the members in their order, no spaces (2-3)."""
    to = ",".join('"%s"' % entry for entry in t["to"])
    return ('{"v":%d,"who":"%s","use":"%s","sub":"%s","id":"%s","pk":"%s","to":[%s],"iat":%d,"exp":%d}'
            % (t["v"], t["who"], t["use"], t["sub"], t["id"], t["pk"], to, t["iat"], t["exp"]))


def fields(who, use, sub="", id="", pk="", to=(), iat=IAT, life=STAFF_LIFE, exp=None):
    return {"v": 1, "who": who, "use": use, "sub": sub, "id": id, "pk": pk, "to": list(to),
            "iat": iat, "exp": iat + life if exp is None else exp}


def ticket_text(kid, payload, sig):
    return "%s.%s.%s.%s" % (PREFIX, kid, b64url(payload), b64url(sig))


def varint(n):
    out = bytearray()
    while True:
        byte, n = n & 0x7F, n >> 7
        if not n:
            out.append(byte)
            return bytes(out)
        out.append(byte | 0x80)


def ld(field, data):
    """A length-delimited protobuf field."""
    return varint(field << 3 | 2) + varint(len(data)) + data


def register_pk(id, pk, *tail):
    """A RegisterPk of the vectors' uuid, as the app's generated code writes it: fields 1, 2, 3, the
    empty old_id (4) and the false no_register_device (5) left out, then `tail`."""
    return ld(1, id.encode()) + ld(2, UUID) + ld(3, pk) + b"".join(tail)


# ------------------------------------------------------------------------------------------------
# The rules: what hbbs decides (docs/access-tickets.md 4) and reads (3-2, 6)
# ------------------------------------------------------------------------------------------------


class _Object(list):
    pass


class _NotAnInteger(Exception):
    pass


def _refuse_number(text):
    raise _NotAnInteger(text)


def parse_payload(raw):
    """The members of a payload that keeps every rule of 2-3, or None (malformed)."""
    try:
        text = raw.decode("utf-8")
        top = json.loads(text, object_pairs_hook=_Object, parse_float=_refuse_number,
                         parse_constant=_refuse_number)
    except (UnicodeDecodeError, ValueError, _NotAnInteger):
        return None
    if not isinstance(top, _Object) or [name for name, _ in top] != list(MEMBERS):
        return None
    t = dict(top)
    if any(type(t[k]) is not int for k in ("v", "iat", "exp")):
        return None
    if any(type(t[k]) is not str for k in ("who", "use", "sub", "id", "pk")):
        return None
    if type(t["to"]) is not list or any(type(entry) is not str for entry in t["to"]):
        return None
    if canonical(t) != text:
        return None
    if t["v"] != 1 or t["who"] not in USES or t["use"] not in USES[t["who"]]:
        return None
    if t["sub"] and (len(t["sub"]) != 16 or not set(t["sub"]) <= HEX):
        return None
    if t["use"] in ("pc", "both"):
        if not valid_id(t["id"]) or key_32(t["pk"]) is None:
            return None
    elif t["id"] or t["pk"]:
        return None
    least, most = TO_COUNT[(t["who"], t["use"])]
    to = t["to"]
    if not least <= len(to) <= most or len(set(to)) != len(to) or not all(map(valid_id, to)):
        return None
    if not 0 <= t["iat"] < t["exp"] <= U64_MAX or t["exp"] - t["iat"] > MAX_LIFETIME:
        return None
    return t


def parse_wire(text):
    parts = text.split(".")
    if len(parts) != 4 or parts[0] != PREFIX:
        return None
    kid = kid_number(parts[1])
    payload = b64url_decode(parts[2])
    sig = b64url_decode(parts[3]) if len(parts[3]) == 86 else None
    if kid is None or payload is None or sig is None:
        return None
    return kid, payload, sig


class Judge:
    def __init__(self, ed, keys, not_before):
        self.ed = ed
        self.keys = keys
        self.not_before = not_before
        self.seen = {}

    def verify(self, pk, message, sig):
        key = (pk, message, sig)
        if key not in self.seen:
            self.seen[key] = self.ed.verify(pk, message, sig)
        return self.seen[key]

    def decide(self, ticket, at, ctx, now=NOW):
        """The reason of 4-1 for `ticket` (text, or the bytes of field 1001) at the check point `at`."""
        raw = ticket.encode("utf-8") if isinstance(ticket, str) else ticket
        if not raw:
            return "missing"
        if len(raw) > MAX_TICKET_BYTES:
            return "oversize"
        try:
            wire = parse_wire(raw.decode("ascii"))
        except UnicodeDecodeError:
            wire = None
        if wire is None:
            return "malformed"
        kid, payload, sig = wire
        if kid not in self.keys:
            return "unknown_kid"
        if not self.verify(self.keys[kid], DOMAIN + b"\0" + payload, sig):
            return "bad_signature"
        t = parse_payload(payload)
        if t is None:
            return "malformed"
        if t["use"] not in POINTS[at]:
            return "wrong_use"
        if now >= t["exp"]:
            return "expired"
        if t["iat"] > now + SKEW:
            return "not_yet_valid"
        if self.not_before is not None and t["iat"] < self.not_before:
            return "revoked"
        if at == "register_pk":
            if t["id"] != ctx["id"] or t["pk"] != ctx["pk"]:
                return "wrong_pc"
        elif t["to"] and ctx["target"] not in t["to"]:
            return "wrong_target"
        return "ok"


def read_message(data):
    """[(field, wire type, value)] of a protobuf message; None if it does not parse."""
    out, i = [], 0

    def number():
        nonlocal i
        n = shift = 0
        while True:
            if i >= len(data) or shift > 63:
                raise ValueError
            byte = data[i]
            i += 1
            n |= (byte & 0x7F) << shift
            shift += 7
            if not byte & 0x80:
                return n

    try:
        while i < len(data):
            key = number()
            field, wire = key >> 3, key & 7
            if wire == 0:
                value = number()
            elif wire in (1, 5):
                size = 8 if wire == 1 else 4
                value, i = data[i:i + size], i + size
            elif wire == 2:
                size = number()
                value, i = data[i:i + size], i + size
            else:
                raise ValueError
            if i > len(data):
                raise ValueError
            out.append((field, wire, value))
    except ValueError:
        return None
    return out


def register_ticket(message):
    """(id, pk, ticket) of a RegisterPk as hbbs reads field 1001 (3-2): the last length-delimited
    value; "malformed" when the field also comes in another wire type; b"" when it is not there."""
    fields_in = read_message(message)
    if fields_in is None:
        sys.exit("a RegisterPk of the vectors does not parse")
    known = {f: v for f, w, v in fields_in if f in (1, 3) and w == 2}
    ours = [(w, v) for f, w, v in fields_in if f == FIELD_HJV_TICKET]
    if any(w != 2 for w, _ in ours):
        ticket = "malformed"
    else:
        ticket = ours[-1][1] if ours else b""
    return known[1].decode(), b64(known[3]), ticket


def rust_trim(text, start=True, end=True):
    a, b = 0, len(text)
    while start and a < b and text[a] in WHITE:
        a += 1
    while end and b > a and text[b - 1] in WHITE:
        b -= 1
    return text[a:b]


def parse_time(text):
    """YYYY-MM-DDTHH:MM:SSZ in UTC, years 0001-9999, as hbbs 0005 reads FROM; Unix seconds or None."""
    b = text
    if len(b) != 20 or b[4] != "-" or b[7] != "-" or b[10] != "T" or b[13] != ":" or b[16] != ":" \
            or b[19] != "Z":
        return None
    parts = (b[0:4], b[5:7], b[8:10], b[11:13], b[14:16], b[17:19])
    if not all(all("0" <= c <= "9" for c in p) for p in parts):
        return None
    year, month, day, hour, minute, second = map(int, parts)
    try:
        when = datetime.datetime(year, month, day, hour, minute, second)
    except ValueError:
        return None
    return calendar.timegm(when.timetuple())


def has_placeholder(text):
    between = text.split("%")[1:-1]
    return any(len(w) >= 3 and all(c.isascii() and (c.isalpha() or c == "_") for c in w)
               for w in between)


def urls_end_at_spaces(text):
    url_byte = set(STD[:62] + "-._~:/?#@!$&*+,;=%")
    rest = text
    while True:
        starts = [i for i in (rest.find("http://"), rest.find("https://")) if i >= 0]
        if not starts:
            return True
        url = rest[min(starts):]
        end = next((i for i, c in enumerate(url) if c in WHITE), len(url))
        if not all(c in url_byte for c in url[:end]):
            return False
        rest = url[end:]


def text_problem(text):
    """hbbs 0005's text_problem with the access marks (5-2); None for a TEXT that may be used."""
    if not text:
        return "is empty"
    if len(text) > MAX_TEXT_CHARS:
        return "is longer than 2000 characters"
    if any(unicodedata.category(c) == "Cc" for c in text):
        return "has a control character"
    if MARK not in text:
        return "does not contain \"access not allowed\""
    if VERSION_MARK in text.lower():
        return "contains \"version not allowed\""
    if text.startswith("Failed"):
        return "starts with \"Failed\""
    if text.split(" ")[0].endswith("_tip"):
        return "starts with a word ending in \"_tip\""
    if "{" in text or "}" in text:
        return "has { or }"
    if "RustDesk" in text:
        return "names RustDesk"
    if has_placeholder(text):
        return "has a %NAME%"
    if not urls_end_at_spaces(text):
        return "has a URL without a space after it"
    return None


def parse_conf(data):
    """The rule of an access.conf (6-2) as the vectors write it, or ("error", why)."""
    if len(data) > MAX_CONF_BYTES:
        return "error", "larger than 64 KiB"
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        return "error", "not UTF-8"
    if text.startswith("\ufeff"):
        text = text[1:]
    if text and not text.endswith("\n"):
        return "error", "the last line has no line break"
    seen = {name: [] for name in CONF_KEYS}
    lines = text.split("\n")[:-1] if text else []
    for n, line in enumerate(lines, 1):
        if line.endswith("\r"):
            line = line[:-1]
        line = rust_trim(line)
        if not line or line.startswith("#"):
            continue
        name, equals, value = line.partition("=")
        if not equals:
            # 6-2: every other line is NAME=value; a bare KEY is no KEY= with an empty value
            return "error", "line %d is not a comment, MODE=, KEY=, NOT_BEFORE= or TEXT=" % n
        if name not in CONF_KEYS:
            if rust_trim(name, start=False) in CONF_KEYS:
                return "error", "line %d has a space before its =" % n
            return "error", "line %d is not a comment, MODE=, KEY=, NOT_BEFORE= or TEXT=" % n
        if len(seen[name]) == CONF_KEYS[name]:
            return "error", "line %d sets %s once too often" % (n, name)
        seen[name].append(rust_trim(value, end=False))
    if not seen["MODE"]:
        return "error", "no MODE= line"
    mode = seen["MODE"][0]
    if mode == "off":
        return {"mode": "off"}
    if mode not in ("count", "enforce"):
        return "error", "MODE is not off, count or enforce"
    if not seen["KEY"]:
        return "error", "no KEY= line"
    keys = {}
    for value in seen["KEY"]:
        kid_text, _, key_text = value.partition(":")
        kid, key = kid_number(kid_text), key_32(key_text)
        if kid is None or key is None:
            return "error", "a KEY is not <1-65535>:<standard base64 of 32 bytes>"
        if kid in keys or key in keys.values():
            return "error", "a key number or a key appears twice"
        keys[kid] = key
    if not seen["NOT_BEFORE"]:
        return "error", "no NOT_BEFORE= line (NOT_BEFORE= alone means none)"
    not_before = None
    if seen["NOT_BEFORE"][0]:
        not_before = parse_time(seen["NOT_BEFORE"][0])
        if not_before is None:
            return "error", "NOT_BEFORE is not a UTC time written as 2026-11-02T00:00:00Z"
    text_value = None
    if seen["TEXT"]:
        text_value = seen["TEXT"][0]
        problem = text_problem(text_value)
        if problem:
            return "error", "TEXT " + problem
    return {"mode": mode, "keys": {str(k): b64(v) for k, v in sorted(keys.items())},
            "not_before": not_before, "text": text_value}


# ------------------------------------------------------------------------------------------------
# The cases
# ------------------------------------------------------------------------------------------------


def ph(target, expect):
    return {"at": "punch_hole", "target": target, "expect": expect}


def rr(target, expect):
    return {"at": "request_relay", "target": target, "expect": expect}


def reg(id, pk, expect):
    return {"at": "register_pk", "id": id, "pk": pk, "expect": expect}


class Vectors:
    def __init__(self, ed):
        self.ed = ed
        self.pub = {name: ed.public_key(seed) for name, seed in SEEDS.items()}
        self.judge = Judge(ed, {1: self.pub["ticket_1"], 2: self.pub["ticket_2"]}, NOT_BEFORE)
        self.pcpk = b64(self.pub["pc"])
        self.otherpk = b64(self.pub["other_pc"])
        self.staff = tag("account", STAFF_LOGIN)
        self.review = tag("account", REVIEW_LOGIN)
        self.tickets = []
        self.by_name = {}

    def sign_bytes(self, key, message):
        return self.ed.sign(SEEDS[key], message)

    def sign(self, key, payload):
        """The signature of 2-2 over `payload`."""
        return self.sign_bytes(key, DOMAIN + b"\0" + payload)

    def make(self, payload, key="ticket_1", kid=None):
        payload = payload.encode() if isinstance(payload, str) else payload
        return ticket_text(KIDS[key] if kid is None else kid, payload, self.sign(key, payload))

    def all3(self, expect):
        return [ph(PC_ID, expect), rr(PC_ID, expect), reg(PC_ID, self.pcpk, expect)]

    def add(self, name, why, ticket, checks, values=None, key=None):
        record = {"name": name, "why": why, "ticket": ticket, "bytes": len(ticket.encode())}
        wire = parse_wire(ticket)
        if wire is not None:
            record["kid"] = wire[0]
            try:
                record["payload"] = wire[1].decode("utf-8")
            except UnicodeDecodeError:
                record["payload_hex"] = wire[1].hex()
        if values is not None:
            record["fields"] = values
            record["signed_by"] = key
            record["signed_hex"] = (DOMAIN + b"\0" + canonical(values).encode()).hex()
        record["checks"] = checks
        for check in checks:
            got = self.judge.decide(ticket, check["at"], check)
            if got != check["expect"]:
                sys.exit("case %s: %s gives %s" % (name, json.dumps(check), got))
        self.tickets.append(record)
        self.by_name[name] = record

    def valid(self, name, why, values, checks, key="ticket_1"):
        if parse_payload(canonical(values).encode()) is None:
            sys.exit("case %s: its fields break a rule of 2-3" % name)
        self.add(name, why, self.make(canonical(values), key), checks, values, key)

    def signed(self, name, why, payload, checks=None):
        """A ticket signed with key 1 over a payload that breaks a rule of 2-3."""
        self.add(name, why, self.make(payload), checks or self.all3("malformed"))

    def build(self):
        s, r, pcpk, otherpk = self.staff, self.review, self.pcpk, self.otherpk
        pk31 = b64(self.pub["pc"][:31])
        longest = tuple(c * 64 for c in "ABC")

        # --- tickets that hbbs takes where their use allows ---
        staff_ctl = fields("staff", "ctl", sub=s)
        self.valid("staff-ctl", "a staff phone's ticket (24 h): any target, no registration",
                   staff_ctl,
                   [ph(PC_ID, "ok"), ph(OTHER_ID, "ok"), rr(PC_ID, "ok"), reg(PC_ID, pcpk, "wrong_use")])
        self.valid("staff-ctl-key-2", "the same ticket signed with key 2: both KEY lines are good",
                   staff_ctl, [ph(PC_ID, "ok"), rr(OTHER_ID, "ok"), reg(PC_ID, pcpk, "wrong_use")],
                   key="ticket_2")
        staff_both = fields("staff", "both", sub=s, id=PC_ID, pk=pcpk)
        self.valid("staff-both", "the ticket of a staff member's Windows app (24 h): it connects "
                   "anywhere and registers only its own ID and device key", staff_both,
                   [ph(OTHER_ID, "ok"), rr(OTHER_ID, "ok"), reg(PC_ID, pcpk, "ok"),
                    reg(OTHER_ID, pcpk, "wrong_pc"), reg(PC_ID, otherpk, "wrong_pc"),
                    reg(PC_ID, pk31, "wrong_pc")])
        self.valid("bound-pc", "a bound PC's ticket from its sync (72 h)",
                   fields("staff", "both", sub=s, id=PC_ID, pk=pcpk, life=PC_LIFE),
                   [reg(PC_ID, pcpk, "ok"), ph(OTHER_ID, "ok"), rr(PC_ID, "ok")])
        self.valid("staff-pc", "staff with use pc: a form hjv-api does not issue now, valid all the "
                   "same", fields("staff", "pc", sub=s, id=PC_ID, pk=pcpk, life=PC_LIFE),
                   [reg(PC_ID, pcpk, "ok"), ph(PC_ID, "wrong_use"), rr(PC_ID, "wrong_use")])
        self.valid("guest-ctl", "a controller's ticket for the access password (12 h): one target",
                   fields("guest", "ctl", to=[PC_ID], life=GUEST_LIFE),
                   [ph(PC_ID, "ok"), ph(OTHER_ID, "wrong_target"), rr(PC_ID, "ok"),
                    rr(OTHER_ID, "wrong_target"), reg(PC_ID, pcpk, "wrong_use")])
        self.valid("guest-ctl-custom-id", "a target is compared byte for byte: a custom ID with "
                   "- and _, and no case folding",
                   fields("guest", "ctl", to=[CUSTOM_ID], life=GUEST_LIFE),
                   [ph(CUSTOM_ID, "ok"), ph(CUSTOM_ID.lower(), "wrong_target"),
                    rr(CUSTOM_ID.upper(), "wrong_target")])
        self.valid("guest-pc", "a PC's ticket for the access password (12 h): its own ID and key",
                   fields("guest", "pc", id=PC_ID, pk=pcpk, life=GUEST_LIFE),
                   [reg(PC_ID, pcpk, "ok"), reg(PC_ID, otherpk, "wrong_pc"),
                    reg(OTHER_ID, pcpk, "wrong_pc"), ph(PC_ID, "wrong_use"), rr(PC_ID, "wrong_use")])
        self.valid("review-ctl", "the App Review account's ticket: only the demo PCs of review.conf",
                   fields("review", "ctl", sub=r, to=DEMO),
                   [ph(DEMO[0], "ok"), ph(DEMO[1], "ok"), ph(PC_ID, "wrong_target"),
                    rr(DEMO[1], "ok"), rr(PC_ID, "wrong_target"), reg(PC_ID, pcpk, "wrong_use")])
        self.valid("longest", "the longest ticket the rules allow: three targets of 64 characters",
                   fields("review", "ctl", sub=r, to=longest),
                   [ph(longest[0], "ok"), ph(longest[2], "ok"), ph(PC_ID, "wrong_target")])
        self.valid("expires-in-a-second", "exp one second after now: still valid",
                   fields("staff", "both", sub=s, id=PC_ID, pk=pcpk, exp=NOW + 1),
                   [ph(PC_ID, "ok"), reg(PC_ID, pcpk, "ok")])
        self.valid("issued-60-s-ahead", "iat 60 s after now, the skew hbbs allows",
                   fields("staff", "both", sub=s, id=PC_ID, pk=pcpk, iat=NOW + SKEW),
                   [ph(PC_ID, "ok"), reg(PC_ID, pcpk, "ok")])
        self.valid("issued-at-not-before", "iat exactly NOT_BEFORE: not revoked",
                   fields("staff", "both", sub=s, id=PC_ID, pk=pcpk, iat=NOT_BEFORE),
                   [ph(PC_ID, "ok"), reg(PC_ID, pcpk, "ok")])
        self.valid("lifetime-7-days", "exp - iat = 604800, the longest lifetime",
                   fields("staff", "ctl", sub=s, life=MAX_LIFETIME), [ph(PC_ID, "ok")])

        # --- refused before the signature ---
        self.add("missing", "no ticket (an empty token, no field 1001): every app before 1.8.0",
                 "", self.all3("missing"))
        self.at_size("oversize", 1026, "oversize", "1026 bytes, a good signature: the size comes "
                     "first")
        self.at_size("at-the-limit", 1024, "malformed", "exactly 1024 bytes passes the size check; "
                     "its long sub breaks a rule after the signature")
        self.add("unknown-kid", "a good signature by key 3, which no KEY line names",
                 self.make(canonical(staff_ctl), "ticket_3"), self.all3("unknown_kid"))

        # --- refused at the signature ---
        payload = canonical(staff_both).encode()
        sig = self.sign("ticket_1", payload)
        flipped = bytes([sig[0] ^ 1]) + sig[1:]
        self.add("bad-signature", "a bit of the signature flipped", ticket_text(1, payload, flipped),
                 self.all3("bad_signature"))
        later = dict(staff_both, exp=staff_both["exp"] + STAFF_LIFE)
        self.add("bad-signature-payload-changed", "exp made a day later after signing",
                 ticket_text(1, canonical(later).encode(), sig), self.all3("bad_signature"))
        self.add("bad-signature-no-domain", "signed over the payload alone, without "
                 "\"hjv-ticket-v1\" NUL", ticket_text(1, payload, self.sign_bytes("ticket_1", payload)),
                 self.all3("bad_signature"))
        over_text = DOMAIN + b"\0" + b64url(payload).encode()
        self.add("bad-signature-over-text", "signed over the base64url text instead of the payload "
                 "bytes", ticket_text(1, payload, self.sign_bytes("ticket_1", over_text)),
                 self.all3("bad_signature"))
        self.add("bad-signature-key-2-as-kid-1", "signed with key 2 and named kid 1: hbbs checks "
                 "only the key that kid names", ticket_text(1, payload, self.sign("ticket_2", payload)),
                 self.all3("bad_signature"))
        order = 2 ** 252 + 27742317777372353535851937790883648493
        s_high = sig[:32] + (int.from_bytes(sig[32:], "little") + order).to_bytes(32, "little")
        self.add("bad-signature-s-not-reduced", "S + L in place of S: refused by libsodium, "
                 "ed25519-dalek and OpenSSL", ticket_text(1, payload, s_high),
                 self.all3("bad_signature"))

        # --- refused after the signature: use, time, binding ---
        both = dict(sub=s, id=PC_ID, pk=pcpk)
        self.add("expired", "exp = now", self.make(canonical(
            fields("staff", "both", iat=NOT_BEFORE, exp=NOW, **both))), self.all3("expired"))
        self.add("not-yet-valid", "iat 61 s after now", self.make(canonical(
            fields("staff", "both", iat=NOW + SKEW + 1, **both))), self.all3("not_yet_valid"))
        self.add("revoked", "iat one second before NOT_BEFORE", self.make(canonical(
            fields("staff", "both", iat=NOT_BEFORE - 1, **both))), self.all3("revoked"))

        # --- malformed outside: the wire form ---
        t = self.by_name["staff-both"]["ticket"]
        prefix, kid, body, tail = t.split(".")
        self.add("prefix-t2", "another version", ".".join(["t2", kid, body, tail]),
                 self.all3("malformed"))
        self.add("prefix-upper-case", "T1", ".".join(["T1", kid, body, tail]), self.all3("malformed"))
        self.add("three-parts", "no signature part", ".".join([prefix, kid, body]),
                 self.all3("malformed"))
        self.add("five-parts", "a part more", t + "." + tail, self.all3("malformed"))
        for name, bad_kid in (("kid-leading-zero", "01"), ("kid-zero", "0"), ("kid-65536", "65536"),
                              ("kid-letter", "a"), ("kid-arabic-digit", "\u0661")):
            self.add(name, "kid %r" % bad_kid, ".".join([prefix, bad_kid, body, tail]),
                     self.all3("malformed"))
        self.add("empty-payload-part", "t1.1..<signature>", ".".join([prefix, kid, "", tail]),
                 self.all3("malformed"))
        self.padded()
        self.std_alphabet()
        last = URL.index(tail[-1])
        self.add("signature-trailing-bits", "the signature's last letter with unused bits set: a "
                 "lenient decoder reads the same 64 bytes",
                 ".".join([prefix, kid, body, tail[:-1] + URL[last + 1]]), self.all3("malformed"))
        sig64 = b64url_decode(tail)
        self.add("signature-63-bytes", "the signature cut to 63 bytes",
                 ".".join([prefix, kid, body, b64url(sig64[:63])]), self.all3("malformed"))
        self.add("signature-65-bytes", "a zero byte after the signature",
                 ".".join([prefix, kid, body, b64url(sig64 + b"\0")]), self.all3("malformed"))
        self.add("trailing-newline", "the ticket and a line feed", t + "\n", self.all3("malformed"))
        self.add("leading-space", "a space and the ticket", " " + t, self.all3("malformed"))

        # --- malformed inside: payloads signed with key 1 that break a rule of 2-3 ---
        good = canonical(staff_both)
        exp = staff_both["exp"]
        self.signed("payload-not-json", "the payload is text, not JSON", "hjv")
        self.signed("payload-not-utf8", "the payload is not UTF-8", b"\xff\xfe{}")
        self.signed("payload-array", "a JSON array", "[1]")
        self.signed("v-2", "v is 2", good.replace('"v":1', '"v":2'))
        self.signed("v-true", "v is true", good.replace('"v":1', '"v":true'))
        self.signed("member-order", "who and use swapped",
                    good.replace('"who":"staff","use":"both"', '"use":"both","who":"staff"'))
        self.signed("member-extra", "a tenth member", good[:-1] + ',"x":1}')
        self.signed("member-missing", "no to", good.replace('"to":[],', ""))
        self.signed("member-twice", "exp twice, the second a day later",
                    good[:-1] + ',"exp":%d}' % (exp + STAFF_LIFE))
        self.signed("spaces", "the same members with spaces", json.dumps(staff_both))
        self.signed("escaped-letter", "who written \\u0073taff",
                    good.replace('"who":"staff"', '"who":"\\u0073taff"'))
        self.signed("who-unknown", "who admin", good.replace('"who":"staff"', '"who":"admin"'))
        self.signed("use-unknown", "use all", good.replace('"use":"both"', '"use":"all"'))
        self.signed("guest-both", "guest with use both", canonical(
            fields("guest", "both", id=PC_ID, pk=pcpk, life=GUEST_LIFE)))
        self.signed("review-pc", "review with use pc", canonical(
            fields("review", "pc", sub=r, id=PC_ID, pk=pcpk)))
        self.signed("guest-ctl-without-to", "a guest controller's ticket for any target", canonical(
            fields("guest", "ctl", life=GUEST_LIFE)))
        self.signed("staff-with-to", "staff with a target", canonical(
            fields("staff", "ctl", sub=s, to=[PC_ID])))
        self.signed("guest-ctl-two-targets", "a guest controller's ticket for two targets", canonical(
            fields("guest", "ctl", to=[PC_ID, OTHER_ID], life=GUEST_LIFE)))
        self.signed("review-four-targets", "review with four targets", canonical(
            fields("review", "ctl", sub=r, to=list(DEMO) + [PC_ID, OTHER_ID])))
        self.signed("review-without-to", "review for any target", canonical(
            fields("review", "ctl", sub=r)))
        self.signed("to-twice", "the same target twice", canonical(
            fields("review", "ctl", sub=r, to=[DEMO[0], DEMO[0]])))
        self.signed("to-empty-entry", "a target that is empty", canonical(
            fields("guest", "ctl", to=[""], life=GUEST_LIFE)))
        self.signed("to-not-array", "to a string", canonical(
            fields("guest", "ctl", to=[PC_ID], life=GUEST_LIFE)).replace(
            '"to":["%s"]' % PC_ID, '"to":"%s"' % PC_ID))
        self.signed("ctl-with-id", "use ctl with an ID and a key", canonical(
            fields("staff", "ctl", sub=s, id=PC_ID, pk=pcpk)))
        self.signed("both-without-pk", "use both without a key", canonical(
            fields("staff", "both", sub=s, id=PC_ID)))
        self.signed("pk-31-bytes", "a key of 31 bytes", good.replace(pcpk, pk31))
        self.signed("pk-url-safe", "the key in base64url letters",
                    good.replace(pcpk, pcpk.replace("+", "-").replace("/", "_")))
        self.signed("pk-unpadded", "the key without its =", good.replace(pcpk, pcpk[:-1]))
        self.signed("id-with-space", "an ID with a space", good.replace(PC_ID, "123 456789"))
        self.signed("id-65-characters", "an ID of 65 characters", good.replace(PC_ID, "1" * 65))
        self.signed("sub-upper-case", "sub in upper case hex", good.replace(s, s.upper()))
        self.signed("sub-15-characters", "sub of 15 hex digits", good.replace(s, s[:15]))
        self.signed("sub-null", "sub null", good.replace('"sub":"%s"' % s, '"sub":null'))
        self.signed("iat-equals-exp", "iat = exp", canonical(dict(staff_both, exp=IAT)))
        self.signed("iat-negative", "iat -1", canonical(dict(staff_both, iat=-1, exp=STAFF_LIFE)))
        self.signed("iat-string", "iat a string", good.replace('"iat":%d' % IAT, '"iat":"%d"' % IAT))
        self.signed("exp-fraction", "exp written with .0", good.replace(str(exp), "%d.0" % exp))
        self.signed("exp-exponent", "exp written with an exponent", good.replace(
            str(exp), "%se%d" % (str(exp)[0] + "." + str(exp)[1:].rstrip("0"), len(str(exp)) - 1)))
        self.signed("iat-leading-zero", "iat written with a 0 in front",
                    good.replace('"iat":%d' % IAT, '"iat":0%d' % IAT))
        self.signed("lifetime-over-7-days", "exp - iat = 604801", canonical(
            dict(staff_both, exp=IAT + MAX_LIFETIME + 1)))

    def at_size(self, name, size, expect, why):
        """A ticket of exactly `size` bytes, signed with key 1, whose sub is as long as it must be."""
        for n in range(size):
            payload = canonical(fields("staff", "ctl", sub="a" * n)).encode()
            length = len("%s.1.." % PREFIX) + len(b64url(payload)) + 86
            if length == size:
                self.add(name, why, self.make(payload), self.all3(expect))
                return
            if length > size:
                break
        sys.exit("no ticket of %d bytes" % size)

    def padded(self):
        for record in list(self.tickets):
            if "fields" in record and len(canonical(record["fields"])) % 3:
                prefix, kid, body, tail = record["ticket"].split(".")
                body += "=" * (-len(body) % 4)
                self.add("payload-padded", "the payload part of %s with its = padding" %
                         record["name"], ".".join([prefix, kid, body, tail]),
                         self.all3("malformed"))
                return
        sys.exit("no valid ticket whose payload needs padding")

    def std_alphabet(self):
        for record in list(self.tickets):
            if "fields" in record and record["fields"]["use"] == "both":
                prefix, kid, rest = record["ticket"].split(".", 2)
                if "-" in rest or "_" in rest:
                    rest = rest.replace("-", "+").replace("_", "/")
                    self.add("standard-alphabet", "%s with + and / in place of - and _" %
                             record["name"], ".".join([prefix, kid, rest]), self.all3("malformed"))
                    return
        sys.exit("no valid ticket with - or _")

    # --- field 1001 of RegisterPk ---

    def register_cases(self):
        t = self.by_name["staff-both"]["ticket"].encode()
        pc = self.pub["pc"]
        varint_1001 = varint(FIELD_HJV_TICKET << 3 | 0) + varint(1)
        cases = [
            ("ticket", "the RegisterPk of a 1.8.0 PC: the app's generated code writes these bytes",
             register_pk(PC_ID, pc, ld(FIELD_HJV_TICKET, t)), "ok"),
            ("no-field", "the RegisterPk of an app before 1.8.0", register_pk(PC_ID, pc), "missing"),
            ("empty", "field 1001 there and empty: no ticket, as in proto3",
             register_pk(PC_ID, pc, ld(FIELD_HJV_TICKET, b"")), "missing"),
            ("twice", "field 1001 twice: the last value counts, as protobuf reads a string",
             register_pk(PC_ID, pc, ld(FIELD_HJV_TICKET, b"t1.0.x.y"), ld(FIELD_HJV_TICKET, t)), "ok"),
            ("varint", "field 1001 also as a varint: malformed, whatever else is there",
             register_pk(PC_ID, pc, varint_1001, ld(FIELD_HJV_TICKET, t)), "malformed"),
            ("not-utf8", "field 1001 is not text", register_pk(PC_ID, pc, ld(FIELD_HJV_TICKET,
                                                                            b"\xff\xfe")), "malformed"),
            ("other-pc", "the ticket of 123456789 in the registration of 222333444",
             register_pk(OTHER_ID, pc, ld(FIELD_HJV_TICKET, t)), "wrong_pc"),
        ]
        out = []
        for name, why, message, expect in cases:
            id, pk, ticket = register_ticket(message)
            record = {"name": name, "why": why, "register_pk_hex": message.hex(),
                      "rendezvous_message_hex": ld(15, message).hex(), "id": id, "pk": pk}
            if ticket == "malformed":
                got = "malformed"
                record["ticket"] = None
            else:
                got = self.judge.decide(ticket, "register_pk", {"id": id, "pk": pk})
                try:
                    record["ticket"] = ticket.decode("utf-8")
                except UnicodeDecodeError:
                    record["ticket"] = None
                    record["ticket_hex"] = ticket.hex()
            if got != expect:
                sys.exit("register_pk %s gives %s" % (name, got))
            record["expect"] = expect
            out.append(record)
        return out

    # --- access.conf ---

    def conf_text(self, mode="enforce", keys=(1, 2), not_before=NOT_BEFORE_TEXT, text=BUILTIN_TEXT):
        lines = ["MODE=%s" % mode]
        lines += ["KEY=%d:%s" % (kid, b64(self.pub["ticket_%d" % kid])) for kid in keys]
        if not_before is not None:
            lines.append("NOT_BEFORE=%s" % not_before)
        if text is not None:
            lines.append("TEXT=%s" % text)
        return "".join(line + "\n" for line in lines)

    def conf_cases(self):
        k1, k2 = b64(self.pub["ticket_1"]), b64(self.pub["ticket_2"])
        main = self.conf_text()
        both_keys = {"1": k1, "2": k2}
        mark = " (access not allowed)"
        long_text = "x" * (MAX_TEXT_CHARS - len(mark)) + mark
        cases = [
            ("vectors", "the file the ticket cases are judged under", main,
             {"mode": "enforce", "keys": both_keys, "not_before": NOT_BEFORE, "text": BUILTIN_TEXT}),
            ("off", "MODE=off alone", "MODE=off\n", {"mode": "off"}),
            ("off-other-values-unread", "with MODE=off the values of the other lines are not read",
             "MODE=off\nKEY=garbage\nNOT_BEFORE=tomorrow\nTEXT=hello\n", {"mode": "off"}),
            ("count-one-key", "count with one key, no NOT_BEFORE and no TEXT (the built-in text)",
             "MODE=count\nKEY=1:%s\nNOT_BEFORE=\n" % k1,
             {"mode": "count", "keys": {"1": k1}, "not_before": None, "text": None}),
            ("bom-comments-crlf", "a BOM, comments, blank lines, CR LF, spaces around a line and "
             "after =", "\ufeff# access\r\n\r\n  MODE= enforce  \r\nKEY=2:%s\r\n\tNOT_BEFORE=\r\n" % k2,
             {"mode": "enforce", "keys": {"2": k2}, "not_before": None, "text": None}),
            ("text-2000-characters", "a TEXT of 2000 characters",
             self.conf_text(text=long_text),
             {"mode": "enforce", "keys": both_keys, "not_before": NOT_BEFORE, "text": long_text}),
            ("text-with-url", "a URL with a space after it",
             self.conf_text(text="https://rd.example.com/help 를 보세요." + mark),
             {"mode": "enforce", "keys": both_keys, "not_before": NOT_BEFORE,
              "text": "https://rd.example.com/help 를 보세요." + mark}),
            ("empty", "an empty file", "", "error"),
            ("no-line-break-at-the-end", "MODE=off without its line break", "MODE=off", "error"),
            ("not-utf8", "a byte that is not UTF-8", b"MODE=off\n#\xff\n", "error"),
            ("no-mode", "no MODE line", "KEY=1:%s\nNOT_BEFORE=\n" % k1, "error"),
            ("mode-twice", "MODE twice", "MODE=off\nMODE=off\n", "error"),
            ("mode-capital", "MODE=Enforce", self.conf_text(mode="Enforce"), "error"),
            ("space-before-equals", "MODE =enforce", main.replace("MODE=", "MODE ="), "error"),
            ("bare-key-under-off", "a KEY line without its = is refused also with MODE=off",
             "MODE=off\nKEY\n", "error"),
            ("bare-not-before", "a NOT_BEFORE line without its = is never NOT_BEFORE= (none)",
             main.replace("NOT_BEFORE=%s\n" % NOT_BEFORE_TEXT, "NOT_BEFORE\n"), "error"),
            ("unknown-line", "a line of another file", main + "MIN=1.8.0\n", "error"),
            ("three-keys-even-when-off", "a third KEY line is refused also with MODE=off",
             "MODE=off\nKEY=1:%s\nKEY=2:%s\nKEY=3:%s\n" % (k1, k2, b64(self.pub["ticket_3"])), "error"),
            ("enforce-without-key", "no KEY line", self.conf_text(keys=()), "error"),
            ("same-kid-twice", "two keys under kid 1",
             main.replace("KEY=2:", "KEY=1:"), "error"),
            ("same-key-twice", "one key under two kids", main.replace(k2, k1), "error"),
            ("kid-leading-zero", "KEY=01:", main.replace("KEY=1:", "KEY=01:"), "error"),
            ("key-31-bytes", "a key of 31 bytes",
             main.replace(k1, b64(self.pub["ticket_1"][:31])), "error"),
            ("key-unpadded", "a key without its =", main.replace(k1, k1[:-1]), "error"),
            ("key-with-space", "a space after the colon", main.replace("KEY=1:", "KEY=1: "),
             "error"),
            ("comment-after-a-value", "a # after a value is part of the value",
             main.replace(k2 + "\n", k2 + "  # the next key\n"), "error"),
            ("enforce-without-not-before", "no NOT_BEFORE line: a file cut short is never a "
             "shorter rule", self.conf_text(not_before=None), "error"),
            ("not-before-with-space", "a time with a space",
             self.conf_text(not_before="2025-10-09 00:00:00"), "error"),
            ("not-before-with-offset", "a time with an offset",
             self.conf_text(not_before="2025-10-09T09:00:00+09:00"), "error"),
            ("not-before-february-30", "a day that does not exist",
             self.conf_text(not_before="2026-02-30T00:00:00Z"), "error"),
            ("text-empty", "TEXT= with nothing after it", self.conf_text(text=""), "error"),
            ("text-without-mark", "no \"access not allowed\"",
             self.conf_text(text="HJViewer는 교직원용입니다."), "error"),
            ("text-with-version-mark", "\"version not allowed\" too",
             self.conf_text(text="(access not allowed) (Version not allowed)"), "error"),
            ("text-failed", "starts with Failed", self.conf_text(text="Failed" + mark), "error"),
            ("text-tip", "starts with a _tip word", self.conf_text(text="login_tip" + mark),
             "error"),
            ("text-braces", "a brace", self.conf_text(text="{x}" + mark), "error"),
            ("text-rustdesk", "names RustDesk", self.conf_text(text="RustDesk" + mark), "error"),
            ("text-placeholder", "a %NAME%", self.conf_text(text="%MIN%" + mark), "error"),
            ("text-url-without-space", "a URL that runs into the text",
             self.conf_text(text="https://rd.example.com/help(access not allowed)"), "error"),
            ("text-control-character", "a tab inside TEXT", self.conf_text(text="a\tb" + mark),
             "error"),
            ("text-2001-characters", "a TEXT of 2001 characters",
             self.conf_text(text="x" + long_text), "error"),
        ]
        out = []
        for name, why, text, expect in cases:
            data = text if isinstance(text, bytes) else text.encode("utf-8")
            got = parse_conf(data)
            if expect == "error" and isinstance(got, tuple):
                result = "error"
            elif got == expect:
                result = expect
            else:
                sys.exit("access.conf %s gives %s" % (name, got))
            record = {"name": name, "why": why}
            if isinstance(text, bytes):
                record["hex"] = data.hex()
            else:
                record["text"] = text
            record["expect"] = result
            out.append(record)
        return out

    def render(self):
        self.build()
        for record in self.tickets:
            for check in record["checks"]:
                assert check["expect"] in REASONS
        used = {check["expect"] for record in self.tickets for check in record["checks"]}
        if used != set(REASONS):
            sys.exit("reasons without a case: %s" % sorted(set(REASONS) - used))
        keys = {}
        for name, seed in SEEDS.items():
            entry = {"seed_hex": seed.hex(), "pk": b64(self.pub[name])}
            if name in KIDS:
                entry = {"kid": KIDS[name], **entry}
            keys[name] = entry
        main = self.conf_text()
        if parse_conf(main.encode())["text"] != BUILTIN_TEXT or text_problem(BUILTIN_TEXT):
            sys.exit("the built-in text breaks a rule")
        doc = {
            "about": "Shared test vectors of HJViewer's access tickets (docs/access-tickets.md), made "
                     "by access-tickets.py next to this file: hjv-api signs a ticket, hbbs checks it at "
                     "the punch hole request, the relay request and the registration, the app only "
                     "carries it. The private keys are public test values: never use them for "
                     "anything else.",
            "use": {
                "hjv-api": "for every ticket with fields: the payload of 2-3 made from the fields and "
                           "signed with the key signed_by gives exactly ticket (signed_hex: the bytes "
                           "signed); sub is Tagger::account of the login key under accounts.log_key_hex",
                "hbbs": "for every ticket and check: under rule, at now, the reason of 4-1 is expect; "
                        "for every register_pk: the ticket read from field 1001 of the message and the "
                        "reason at register_pk with its id and pk; for every access_conf: the rule "
                        "read from the file, or error",
                "app": "register_pk 'ticket': a RegisterPk with these id, uuid, pk and hjv_ticket "
                       "encodes to register_pk_hex",
            },
            "encodings": "base64: standard with padding; base64url: URL-safe without padding; hex: "
                         "lowercase; times: Unix seconds; signed_hex: the exact bytes the signature "
                         "covers",
            "domain": DOMAIN.decode(),
            "limits": {"ticket_bytes": MAX_TICKET_BYTES, "kid": [1, MAX_KID], "skew_seconds": SKEW,
                       "max_lifetime_seconds": MAX_LIFETIME, "max_to": MAX_TO,
                       "text_characters": MAX_TEXT_CHARS, "access_conf_bytes": MAX_CONF_BYTES},
            "keys": keys,
            "accounts": {"log_key_hex": LOG_KEY.hex(),
                         "staff": {"login_key": STAFF_LOGIN, "sub": self.staff},
                         "review": {"login_key": REVIEW_LOGIN, "sub": self.review}},
            "lifetimes": {"staff": STAFF_LIFE, "bound_pc": PC_LIFE, "guest": GUEST_LIFE,
                          "review": STAFF_LIFE},
            "now": NOW,
            "access_conf": main,
            "rule": {"mode": "enforce",
                     "keys": {"1": b64(self.pub["ticket_1"]), "2": b64(self.pub["ticket_2"])},
                     "not_before": NOT_BEFORE},
            "builtin_text": BUILTIN_TEXT,
            "checks": "at: punch_hole (target: PunchHoleRequest.id), request_relay (target: "
                      "RequestRelay.id) or register_pk (id: RegisterPk.id; pk: the standard base64 "
                      "of RegisterPk.pk); expect: the reason of 4-1. Under MODE=enforce every reason "
                      "but ok is refused with the answer of its point; under MODE=count every "
                      "request goes on and the reason is counted.",
            "answers": {"punch_hole": "PunchHoleResponse { other_failure: TEXT }",
                        "request_relay": "RelayResponse { refuse_reason: TEXT }",
                        "register_pk": "RegisterPkResponse { result: NOT_DEPLOYED, keep_alive: 30 }"},
            "reasons": list(REASONS),
            "tickets": self.tickets,
            "register_pk": self.register_cases(),
            "access_conf_cases": self.conf_cases(),
        }
        self.counts = (len(doc["tickets"]), len(doc["register_pk"]), len(doc["access_conf_cases"]))
        return dump(doc) + "\n"


def visible(text):
    """JSON text with the characters one cannot see (a BOM, separators) written as escapes."""
    return "".join("\\u%04x" % ord(c) if ord(c) > 0x7F and unicodedata.category(c) in
                   ("Cf", "Zl", "Zp", "Zs") else c for c in text)


def dump(value, indent=0, prefix=""):
    """JSON in two-space indents, where an object or array that fits in 100 columns keeps to one
    line."""
    pad = " " * indent
    one = visible(json.dumps(value, ensure_ascii=False, separators=(", ", ": ")))
    if not isinstance(value, (dict, list)) or not value or indent + len(prefix) + len(one) <= 100:
        return pad + prefix + one
    if isinstance(value, dict):
        items = [dump(v, indent + 2, json.dumps(k, ensure_ascii=False) + ": ")
                 for k, v in value.items()]
        return pad + prefix + "{\n" + ",\n".join(items) + "\n" + pad + "}"
    items = [dump(v, indent + 2) for v in value]
    return pad + prefix + "[\n" + ",\n".join(items) + "\n" + pad + "]"


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--check", action="store_true",
                        help="compare access-tickets.json with what this script makes")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory() as tmp:
        ed = ed25519(tmp)
        vectors = Vectors(ed)
        text = vectors.render()
    if args.check:
        try:
            with open(OUT, encoding="utf-8") as f:
                on_disk = f.read()
        except FileNotFoundError:
            sys.exit("no %s: run this script without --check" % OUT)
        if on_disk != text:
            sys.exit("access-tickets.json is not what access-tickets.py makes (%s): run it again" %
                     ed.name)
        print("access-tickets.json is what it makes: %d tickets, %d registrations, %d access.conf "
              "files (%s)" % (*vectors.counts, ed.name))
        return
    with open(OUT, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)
    print("wrote %s: %d tickets, %d registrations, %d access.conf files (%s)" %
          (OUT, *vectors.counts, ed.name))


if __name__ == "__main__":
    main()

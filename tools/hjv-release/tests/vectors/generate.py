#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Makes the shared test vectors of HJViewer's signed version policy, and checks them.

The rules are build/minver/trust-rules/spec.md (1.6.0). Their executable reference model is
model.py next to this file: a byte-identical copy of build/minver/trust-rules/model.py, so that
the repository carries the oracle (generate.py --check fails when the copy changes without the
vectors). This script turns the model's cases of spec 7-4 into envelope files signed with the test
keys, and writes manifest.json with what an app does with each one. The verdicts are the model's:

  1. every envelope file is read back from its bytes, as an app reads it (spec 2-2 and steps A-C:
     size, UTF-8, the JSON rules, strict Ed25519 with the pinned test keys, the field rules) into
     the model's terms (versions as num(), times as Unix seconds, texts interned by their bytes);
  2. the model's accept() runs over the case's history and the case's file; its outcome, reason
     and the state it leaves are what the manifest says;
  3. that result must equal what the model says for its own (abstract) case of 7-4, which the
     model has already checked against the specification; a difference stops the script.

    python3 -I -B generate.py          # writes the files
    python3 -I -B generate.py --check  # makes them in memory, compares with the files, and runs
                                       # the model over every file on disk against manifest.json

Pure standard library (Python 3.8 or later); Ed25519 is written out below (RFC 8032) because the
vectors need strict verification exactly as libsodium and ed25519-dalek's verify_strict do it.
Test keys only: every private key here is a published test key (src/keys.rs, TEST_KEYS).
"""

import argparse
import base64
import calendar
import hashlib
import importlib.util
import json
import math
import os
import re
import sys

sys.dont_write_bytecode = True          # no __pycache__ in the repository

HERE = os.path.dirname(os.path.abspath(__file__))
MODEL_SHA256 = "fb864c1df4b0e1f034442fbdba377ff15e6a6422b59028220317fe5bbfdc8af5"


def load_model():
    path = os.path.join(HERE, "model.py")
    with open(path, "rb") as f:
        digest = hashlib.sha256(f.read()).hexdigest()
    if digest != MODEL_SHA256:
        sys.exit("model.py is not the copy these vectors were made with (sha256 %s, expected %s): "
                 "copy build/minver/trust-rules/model.py again and regenerate" % (digest, MODEL_SHA256))
    spec = importlib.util.spec_from_file_location("hjv_trust_model", path)
    module = importlib.util.module_from_spec(spec)
    sys.modules["hjv_trust_model"] = module
    spec.loader.exec_module(module)
    return module


M = load_model()

SERVER = "rd.example.com"
POLICY_DOMAIN = b"hjv-version-v1"
ROOT_DOMAIN = b"hjv-version-root-v1"
MAX_ENVELOPE_BYTES = 16384
FINGERPRINT_CONTEXT = b"hjv-update-keys-v1"

# ------------------------------------------------------------------------------------------------
# Ed25519 (RFC 8032 5.1), with the strict verification of libsodium / verify_strict
# ------------------------------------------------------------------------------------------------

_P = 2 ** 255 - 19
_L = 2 ** 252 + 27742317777372353535851937790883648493
_D = -121665 * pow(121666, _P - 2, _P) % _P
_SQRT_M1 = pow(2, (_P - 1) // 4, _P)


def _inv(x):
    return pow(x, _P - 2, _P)


def _recover_x(y, sign):
    if y >= _P:
        return None
    x2 = (y * y - 1) * _inv(_D * y * y + 1) % _P
    if x2 == 0:
        return None if sign else 0
    x = pow(x2, (_P + 3) // 8, _P)
    if (x * x - x2) % _P:
        x = x * _SQRT_M1 % _P
    if (x * x - x2) % _P:
        return None
    if (x & 1) != sign:
        x = _P - x
    return x


_GY = 4 * _inv(5) % _P
_GX = _recover_x(_GY, 0)
_G = (_GX, _GY, 1, _GX * _GY % _P)
_O = (0, 1, 1, 0)


def _add(p, q):
    a = (p[1] - p[0]) * (q[1] - q[0]) % _P
    b = (p[1] + p[0]) * (q[1] + q[0]) % _P
    c = 2 * p[3] * q[3] * _D % _P
    d = 2 * p[2] * q[2] % _P
    e, f, g, h = b - a, d - c, d + c, b + a
    return (e * f % _P, g * h % _P, f * g % _P, e * h % _P)


def _mul(s, p):
    q = _O
    while s:
        if s & 1:
            q = _add(q, p)
        p = _add(p, p)
        s >>= 1
    return q


def _compress(p):
    zi = _inv(p[2])
    x, y = p[0] * zi % _P, p[1] * zi % _P
    return (y | ((x & 1) << 255)).to_bytes(32, "little")


def _decompress(b):
    y = int.from_bytes(b, "little")
    sign, y = y >> 255, y & ((1 << 255) - 1)
    x = _recover_x(y, sign)
    return None if x is None else (x, y, 1, x * y % _P)


def _is_small_order(p):
    q = _mul(8, p)
    return q[0] % _P == 0 and (q[1] - q[2]) % _P == 0


def _hint(b):
    return int.from_bytes(hashlib.sha512(b).digest(), "little")


def _expand(seed):
    h = hashlib.sha512(seed).digest()
    a = int.from_bytes(h[:32], "little")
    a &= (1 << 254) - 8
    a |= 1 << 254
    return a, h[32:]


def public_key(seed):
    return _compress(_mul(_expand(seed)[0], _G))


def sign(seed, message):
    a, prefix = _expand(seed)
    pub = _compress(_mul(a, _G))
    r = _hint(prefix + message) % _L
    rb = _compress(_mul(r, _G))
    s = (r + _hint(rb + pub + message) % _L * a) % _L
    return rb + s.to_bytes(32, "little")


def sign_with_identity_r(seed, message):
    """A signature whose R is the identity point: with s = k * a the plain equation holds, and a
    strict check refuses the small-order R all the same."""
    a, _ = _expand(seed)
    pub = _compress(_mul(a, _G))
    rb = _compress(_O)
    k = _hint(rb + pub + message) % _L
    return rb + (k * a % _L).to_bytes(32, "little")


def verify_plain(pub, message, sig):
    """The bare equation [s]B = R + [k]A, as OpenSSL checks it (for the identity-R case only)."""
    a, r = _decompress(pub), _decompress(sig[:32])
    s = int.from_bytes(sig[32:], "little")
    if a is None or r is None or s >= _L:
        return False
    k = _hint(sig[:32] + pub + message) % _L
    lhs, rhs = _mul(s, _G), _add(r, _mul(k, a))
    return all((lhs[i] * rhs[2] - rhs[i] * lhs[2]) % _P == 0 for i in (0, 1))


def verify_strict(pub, message, sig):
    if len(sig) != 64 or len(pub) != 32:
        return False
    a = _decompress(pub)
    if a is None or _is_small_order(a):
        return False
    r = _decompress(sig[:32])
    if r is None or _is_small_order(r):
        return False
    s = int.from_bytes(sig[32:], "little")
    if s >= _L:
        return False
    k = _hint(sig[:32] + pub + message) % _L
    ka = _mul(k, a)
    neg = ((_P - ka[0]) % _P, ka[1], ka[2], (_P - ka[3]) % _P)
    return _compress(_add(_mul(s, _G), neg)) == sig[:32]


# ------------------------------------------------------------------------------------------------
# Test keys (src/keys.rs TEST_KEYS): seeds of 32 consecutive bytes
# ------------------------------------------------------------------------------------------------

TEST_KEYS = [
    # name, role, id, first seed byte, in the vectors' key file, about
    ("signing-1", "signing", 1, 0x40, True, "signing key 1 of the vectors (seed 40 41 .. 5f)"),
    ("signing-2", "signing", 2, 0x80, True, "signing key 2 of the vectors (seed 80 81 .. 9f)"),
    ("root-101", "root", 101, 0xA0, True, "root key 101 of the vectors (seed a0 a1 .. bf)"),
    ("root-102", "root", 102, 0xE0, True, "root key 102 of the vectors (seed e0 e1 .. ff)"),
    ("signing-3", "signing", 3, 0xC0, False, "a signing key no vector build trusts (seed c0 c1 .. df)"),
    ("root-103", "root", 103, 0x10, False, "a root key no vector build trusts (seed 10 11 .. 2f)"),
]
KEY = {name: (k, bytes((first + i) % 256 for i in range(32))) for name, _, k, first, _, _ in TEST_KEYS}
PUBLIC = {name: public_key(seed) for name, (_, seed) in KEY.items()}


def key_line(names):
    return ",".join("%d:%s" % (KEY[n][0], base64.b64encode(PUBLIC[n]).decode())
                    for n in sorted(names, key=lambda n: KEY[n][0]))


SIGNING_LINE = key_line(["signing-1", "signing-2"])
ROOT_LINE = key_line(["root-101", "root-102"])
PINS = {KEY[n][0]: PUBLIC[n] for n in ("signing-1", "signing-2")}
ROOTS = {KEY[n][0]: PUBLIC[n] for n in ("root-101", "root-102")}


def fingerprint():
    h = hashlib.sha256(FINGERPRINT_CONTEXT)
    for part in (SERVER, SIGNING_LINE, ROOT_LINE):
        h.update(b"\0" + part.encode())
    return h.hexdigest()


# ------------------------------------------------------------------------------------------------
# The texts, exactly as hjv-release writes them (spec 1-3)
# ------------------------------------------------------------------------------------------------

PLATFORMS = ("windows", "android", "android_play", "ios")
POLICY_ORDER = ("v", "epoch", "seq", "issued", "server", "contact_ko", "platforms")
ENTRY_ORDER = ("min", "latest", "grace_until", "grace_min", "os_min", "notes_ko", "source", "package",
               "auto_install_after", "move_to_play", "version_code", "testflight_build")
PACKAGE_ORDER = ("file", "size", "sha256", "build")
DECL_ORDER = ("v", "server", "epoch", "revoked", "issued", "base")
BASE_ORDER = ("min", "latest", "grace_until", "grace_min")


def dumps(value):
    """serde_json::to_string's output for these values: no spaces, members in insertion order,
    strings escaped as serde_json does (quote, backslash, control characters; nothing else)."""
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"), allow_nan=False)


def ordered(obj, order):
    unknown = set(obj) - set(order)
    assert not unknown, unknown
    return {k: obj[k] for k in order if k in obj}


def policy_text(p):
    p = dict(p)
    p.setdefault("v", 1)
    plats = {}
    for name in PLATFORMS:
        if name in p["platforms"]:
            e = dict(p["platforms"][name])
            if "package" in e:
                e["package"] = ordered(e["package"], PACKAGE_ORDER)
            plats[name] = ordered(e, ENTRY_ORDER)
    p["platforms"] = plats
    return dumps(ordered(p, POLICY_ORDER))


def base_of(platforms):
    """The requirement part (min, latest, grace_until, grace_min) of every entry (4-2)."""
    return {n: {k: platforms[n][k] for k in BASE_ORDER if k in platforms[n]}
            for n in PLATFORMS if n in platforms}


def decl_text(d):
    d = dict(d)
    d.setdefault("v", 1)
    d.setdefault("server", SERVER)
    if "base" in d:
        d["base"] = {n: ordered(d["base"][n], BASE_ORDER) for n in PLATFORMS if n in d["base"]}
    return dumps(ordered(d, DECL_ORDER))


def message(domain, text):
    return domain + b"\0" + text.encode()


def part(text, by, domain=POLICY_DOMAIN, as_k=None):
    k, seed = KEY[by]
    return {"k": k if as_k is None else as_k, "p": text, "s": base64.b64encode(sign(seed, message(domain, text))).decode()}


def envelope_text(pol, root=None):
    env = {"v": 1, "k": pol["k"], "p": pol["p"], "s": pol["s"]}
    if root is not None:
        env["root"] = {"k": root["k"], "p": root["p"], "s": root["s"]}
    return dumps(env)


def envelope(p, by, decl=None, root_by="root-101"):
    """The file of policy p signed by `by`, with declaration `decl` signed by `root_by`."""
    root = None if decl is None else part(decl_text(decl), root_by, ROOT_DOMAIN)
    return (envelope_text(part(policy_text(p), by), root) + "\n").encode()


def raw_file(pol, root=None):
    return (envelope_text(pol, root) + "\n").encode()


def replaced(data, old, new):
    """`data` (bytes or str) with the first `old` replaced; it must be there."""
    if isinstance(data, bytes):
        old, new = old.encode() if isinstance(old, str) else old, new.encode() if isinstance(new, str) else new
    assert old in data, old
    return data.replace(old, new, 1)


def sha256hex(b):
    return hashlib.sha256(b).hexdigest()


# ------------------------------------------------------------------------------------------------
# The history and its texts (spec 7-4), as model.vector_history() has them
# ------------------------------------------------------------------------------------------------

GRACE = "2026-10-16T15:00:00Z"            # T16 of the model
CONTACT = "설치 파일은 관리자에게 받으세요."
NOTES = "연결이 끊긴 뒤 다시 붙지 않던 문제를 고쳤습니다.\n한글 입력을 고쳤습니다."


def package(platform, version, build, size):
    suffix = "windows-x86_64.exe" if platform == "windows" else "android-arm64.apk"
    return {"file": "HJViewer-%s-%s" % (version, suffix), "size": size,
            "sha256": sha256hex(("HJViewer test package %s %s" % (platform, version)).encode()), "build": build}


PKG = {
    ("windows", "1.6.0"): package("windows", "1.6.0", "1.6.0.37", 25329664),
    ("android", "1.6.0"): package("android", "1.6.0", "262742100", 28717196),
    ("windows", "1.6.1"): package("windows", "1.6.1", "1.6.1.41", 25400320),
    ("android", "1.6.1"): package("android", "1.6.1", "262782100", 28800004),
    ("windows", "99.0.0"): package("windows", "99.0.0", "99.0.0.1", 25400320),
    ("android", "99.0.0"): package("android", "99.0.0", "999999999", 28800004),
}


def copy(p):
    return json.loads(json.dumps(p))


def p1():
    """seq 1: 1.6.0 is out, nothing is enforced."""
    def entry(os_min, **more):
        e = {"min": "1.5.0", "latest": "1.6.0", "os_min": os_min, "source": "client-1.6.0"}
        e.update(more)
        return e
    return {"epoch": 1, "seq": 1, "issued": "2026-10-01T03:00:00Z", "server": SERVER, "contact_ko": CONTACT,
            "platforms": {"windows": entry("10", package=dict(PKG["windows", "1.6.0"])),
                          "android": entry("22", package=dict(PKG["android", "1.6.0"])),
                          "ios": entry("13.0", testflight_build="262740100")}}


def raise_all(p, minimum, until, grace_min):
    for e in p["platforms"].values():
        e["min"] = minimum
        e["grace_until"] = until
        e["grace_min"] = grace_min


def p2():
    """seq 2: every min 1.6.0 until 10-16, grace_min 1.5.0 (the first enforcement, 5-2)."""
    p = p1()
    p.update(seq=2, issued="2026-10-02T03:00:00Z")
    raise_all(p, "1.6.0", GRACE, "1.5.0")
    return p


def p3():
    """seq 3 (key 2): 1.6.1 for Windows and Android with packages, an unattended install date,
    the Google Play entry at 1.6.0 without a deadline."""
    p = p2()
    p.update(seq=3, issued="2026-10-05T03:00:00Z")
    w, a = p["platforms"]["windows"], p["platforms"]["android"]
    w.update(latest="1.6.1", notes_ko=NOTES, source="client-1.6.1", package=dict(PKG["windows", "1.6.1"]),
             auto_install_after="2026-10-07T03:00:00Z")
    a.update(latest="1.6.1", notes_ko=NOTES, source="client-1.6.1", package=dict(PKG["android", "1.6.1"]),
             move_to_play=True)
    p["platforms"]["android_play"] = {"min": "1.6.0", "latest": "1.6.1", "source": "client-1.6.1",
                                      "version_code": 262782100}
    return p


def d2(**more):
    """The declaration of epoch 2: signing key 1 revoked; base = e1-s3's requirements."""
    d = {"epoch": 2, "revoked": [1], "issued": "2026-10-06T03:00:00Z", "base": base_of(p3()["platforms"])}
    d.update(more)
    return d


def p4():
    p = p3()
    p.update(epoch=2, seq=1, issued="2026-10-06T03:00:00Z")
    return p


def p5():
    """e2-s2: the emergency stop, auto_install_after removed."""
    p = p4()
    p.update(seq=2, issued="2026-10-07T03:00:00Z")
    del p["platforms"]["windows"]["auto_install_after"]
    return p


def p6():
    """e3-s1-relax: every min 1.5.0 without deadlines (the update path is down, 5-12)."""
    p = p5()
    p.update(epoch=3, seq=1, issued="2026-10-08T03:00:00Z")
    for e in p["platforms"].values():
        e["min"] = "1.5.0"
        e.pop("grace_until", None)
        e.pop("grace_min", None)
    return p


def d3():
    """The declaration of epoch 3 by root key 101: revoked [1], base = e3-s1-relax's requirements."""
    return {"epoch": 3, "revoked": [1], "issued": "2026-10-08T03:00:00Z", "base": base_of(p6()["platforms"])}


def p7():
    """e3-s2 (key 2): 1.6.1 (iOS 1.6.0) until 10-23, grace_min 1.5.0."""
    p = p6()
    p.update(seq=2, issued="2026-10-09T03:00:00Z")
    for name, e in p["platforms"].items():
        e.update(min="1.6.0" if name == "ios" else "1.6.1", grace_until="2026-10-23T15:00:00Z",
                 grace_min="1.5.0")
    return p


def at(p, **more):
    p = copy(p)
    p.update(more)
    return p


def with_entry(p, name, **change):
    """p with entry `name` changed: a value None removes the member."""
    p = copy(p)
    e = p["platforms"].setdefault(name, {})
    for k, v in change.items():
        if v is None:
            e.pop(k, None)
        else:
            e[k] = v
    return p


def first_policy_of_size(size):
    """e1-s1's entries at seq 2 with notes that make the envelope file exactly `size` bytes."""
    def filled(korean, ascii_):
        p = at(p1(), seq=2, issued="2026-10-02T03:00:00Z")
        for name in ("windows", "android"):
            p["platforms"][name]["notes_ko"] = "가" * 2000
        p["platforms"]["ios"]["notes_ko"] = "가" * korean + "a" * ascii_
        return envelope(p, "signing-1")
    need = size - (len(filled(1, 0)) - 3)
    korean = need // 3 - 1
    out = filled(korean, need - 3 * korean)
    assert len(out) == size, (len(out), size)
    return out


# ------------------------------------------------------------------------------------------------
# The concrete envelope of every case. A recipe returns the file's bytes, or the name of the case
# whose file it uses. Names, histories, floors, bindings and expectations are the model's.
# ------------------------------------------------------------------------------------------------

def bad_part(text, domain=POLICY_DOMAIN, k=2):
    """A part whose signature fails: signed by signing-3 under the number k."""
    return part(text, "signing-3", domain, as_k=k)


def declared(p, decl, by="signing-2", root_by="root-101"):
    return envelope(p, by, decl, root_by)


def recipes():
    e1s1 = envelope(p1(), "signing-1")
    p1t = policy_text(p1())
    r = {}
    # the history
    r["e1-s1"] = e1s1
    r["e1-s2"] = envelope(p2(), "signing-1")
    r["e1-s3"] = envelope(p3(), "signing-2")
    r["e2-s1"] = declared(p4(), d2())
    r["e2-s2"] = declared(p5(), d2())
    r["e3-s1-relax"] = declared(p6(), d3())
    r["e3-s2"] = declared(p7(), d3())
    # accepted around it
    r["e1-s2-again"] = "e1-s2"
    r["fresh-install-e2-s2"] = "e2-s2"
    r["e3-s2-missed-the-relaxation"] = "e3-s2"
    r["floor-exact"] = "e2-s2"
    r["seq-step-1000"] = envelope(at(p1(), seq=1001, issued="2026-10-03T03:00:00Z"), "signing-1")
    r["fresh-install-seq-1000"] = envelope(at(p1(), seq=1000, issued="2026-10-03T03:00:00Z"), "signing-1")
    p3_4 = at(p3(), seq=4, issued="2026-10-06T03:00:00Z")
    r["stop-auto-install"] = envelope(with_entry(p3_4, "windows", auto_install_after=None), "signing-2")
    r["delay-auto-install"] = envelope(with_entry(p3_4, "windows", auto_install_after="2026-11-07T03:00:00Z"),
                                       "signing-2")
    p2_3 = at(p2(), seq=3, issued="2026-10-03T03:00:00Z")
    earlier = copy(p2_3)
    for e in earlier["platforms"].values():
        e["grace_until"] = "2026-10-14T15:00:00Z"
    r["grace-earlier"] = envelope(earlier, "signing-1")
    r["grace-removed"] = envelope(with_entry(p2_3, "windows", grace_until=None, grace_min=None), "signing-1")
    r["size-at-the-limit"] = first_policy_of_size(MAX_ENVELOPE_BYTES)
    # relaxations: refused by a signing key, refused by a root key, taken with a declaration
    relax = {
        "relax-min-lowered": with_entry(at(p2(), seq=3), "ios", min="1.5.0", grace_min="1.4.0"),
        "relax-grace-later": with_entry(at(p2(), seq=3), "windows", grace_until="2026-10-17T15:00:00Z"),
        "relax-grace-added": with_entry(at(p1(), seq=2), "windows", grace_until=GRACE, grace_min="1.4.0"),
        "relax-latest-lowered": with_entry(at(p3(), seq=4), "windows", latest="1.6.0", notes_ko=None,
                                           source="client-1.6.0", package=dict(PKG["windows", "1.6.0"]),
                                           auto_install_after=None),
        "relax-entry-removed": at(p2(), seq=3),
    }
    del relax["relax-entry-removed"]["platforms"]["ios"]
    for name, p in relax.items():
        r[name] = envelope(p, "signing-1")
        r[name + "-root"] = envelope(p, "root-101")
        q = at(p, epoch=2, seq=1)
        r[name + "-by-declaration"] = declared(q, {"epoch": 2, "revoked": [], "issued": q["issued"],
                                                   "base": base_of(q["platforms"])})
    # signatures, keys, envelopes
    r["tampered-policy"] = replaced(e1s1, '\\"seq\\":1,', '\\"seq\\":2,')
    text = e1s1.decode()
    i = text.index('"s":"') + 5
    r["tampered-signature"] = (text[:i] + ("B" if text[i] == "A" else "A") + text[i + 1:]).encode()
    r["wake-domain"] = raw_file(part(p1t, "signing-1", b"hjv-wake-v1"))
    r["policy-under-root-domain"] = raw_file(part(p1t, "signing-1", ROOT_DOMAIN))
    r["another-key-as-1"] = raw_file(part(p1t, "signing-3", as_k=1))
    r["signature-with-identity-r"] = raw_file(
        {"k": 1, "p": p1t, "s": base64.b64encode(sign_with_identity_r(KEY["signing-1"][1],
                                                                     message(POLICY_DOMAIN, p1t))).decode()})
    r["another-root-as-101"] = raw_file(part(policy_text(at(p1(), seq=2, issued="2026-10-02T03:00:00Z")),
                                             "root-102", as_k=101))
    r["unknown-key"] = envelope(p1(), "signing-3")
    r["envelope-v2"] = replaced(e1s1, '{"v":1,"k"', '{"v":2,"k"')
    r["policy-v2"] = raw_file(part(replaced(p1t, '{"v":1,"epoch"', '{"v":2,"epoch"'), "signing-1"))
    r["declaration-v2"] = raw_file(part(policy_text(p4()), "signing-2"),
                                   part(replaced(decl_text(d2()), '{"v":1,', '{"v":2,'), "root-101", ROOT_DOMAIN))
    r["wrong-server"] = envelope(at(p1(), server="other.example.com"), "signing-1")
    r["key-number-zero"] = replaced(e1s1, '{"v":1,"k":1,', '{"v":1,"k":0,')
    r["key-number-as-text"] = replaced(e1s1, '{"v":1,"k":1,', '{"v":1,"k":"1",')
    s64 = json.loads(e1s1)["s"]
    r["signature-63-bytes"] = replaced(e1s1, s64, base64.b64encode(base64.b64decode(s64)[:63]).decode())
    big = p1()
    for e in big["platforms"].values():
        e["notes_ko"] = "가" * 2000
    r["too-large"] = envelope(big, "signing-1")
    assert len(r["too-large"]) > MAX_ENVELOPE_BYTES
    r["size-one-over"] = first_policy_of_size(MAX_ENVELOPE_BYTES + 1)
    # declarations
    p4t, d2t = policy_text(p4()), decl_text(d2())
    r["declaration-by-signing-key"] = raw_file(part(p4t, "signing-2"), part(d2t, "signing-2", ROOT_DOMAIN))
    r["declaration-signing-key-as-101"] = raw_file(part(p4t, "signing-2"),
                                                   part(d2t, "signing-2", ROOT_DOMAIN, as_k=101))
    r["declaration-under-policy-domain"] = raw_file(part(p4t, "signing-2"), part(d2t, "root-101", POLICY_DOMAIN))
    r["declaration-tampered"] = replaced(r["e2-s1"], '\\"revoked\\":[1]', '\\"revoked\\":[3]')
    r["declaration-revokes-root"] = declared(p4(), d2(revoked=[1, 101]))
    r["declaration-wrong-server"] = declared(p4(), d2(server="other.example.com"))
    r["revoked-key-later"] = declared(with_entry(at(p3(), epoch=2, seq=2, issued="2026-10-07T03:00:00Z"),
                                                 "windows", auto_install_after=None), d2(), by="signing-1")
    p3_old = with_entry(p3(), "windows", auto_install_after=None)
    r["old-epoch-after-new"] = envelope(at(p3_old, seq=4, issued="2026-10-07T03:00:00Z"), "signing-2")
    r["signing-key-raises-epoch"] = envelope(at(p3_old, epoch=2, seq=1, issued="2026-10-06T03:00:00Z"),
                                             "signing-2")
    r["root-policy-of-an-old-epoch"] = envelope(at(p1(), seq=5000, issued="2026-10-07T03:00:00Z"), "root-101")
    r["seq-lower"] = "e1-s1"
    at4 = at(p1(), seq=4, issued="2026-10-03T03:00:00Z")
    r["below-floor"] = envelope(at4, "signing-1")
    r["below-floor-by-root"] = envelope(at4, "root-101")
    other = p1()
    del other["contact_ko"]
    r["same-position-other-bytes"] = envelope(other, "signing-1")
    r["seq-step-1001"] = envelope(at(p1(), seq=1002, issued="2026-10-03T03:00:00Z"), "signing-1")
    r["fresh-install-seq-1001"] = envelope(at(p1(), seq=1001, issued="2026-10-03T03:00:00Z"), "signing-1")
    p1_2 = at(p1(), seq=2, issued="2026-10-02T03:00:00Z")
    r["min-above-latest"] = envelope(with_entry(p1_2, "windows", min="1.6.0", latest="1.5.0"), "signing-1")
    # the member rules an app checks (C3), each on e1-s2's text
    p2t = policy_text(p2())
    fields = {
        "version-two-numbers": ('"min":"1.6.0"', '"min":"1.6"'),
        "version-v-prefix": ('"min":"1.6.0"', '"min":"v1.6.0"'),
        "version-three-digit-patch": ('"latest":"1.6.0"', '"latest":"1.5.100"'),
        "version-four-numbers": ('"latest":"1.6.0"', '"latest":"1.6.0.1"'),
        "version-plus-build": ('"latest":"1.6.0"', '"latest":"1.6.0+1"'),
        "issued-not-rfc3339": ('"issued":"2026-10-02T03:00:00Z"', '"issued":"2026-10-02 03:00:00"'),
    }
    for name, (old, new) in fields.items():
        r[name] = raw_file(part(replaced(p2t, old, new), "signing-1"))

    def windows_package(**change):
        p = p2()
        p["platforms"]["windows"]["package"].update(change)
        return envelope(p, "signing-1")
    wp = dict(PKG["windows", "1.6.0"])
    r["package-file-install-exe"] = windows_package(file="HJViewer-1.6.0-install.exe")
    r["package-file-qs"] = windows_package(file="HJViewer-qs-1.6.0.exe")
    r["package-file-path"] = windows_package(file="../HJViewer-1.6.0.exe")
    r["package-size-zero"] = windows_package(size=0)
    r["package-size-over-200-mib"] = windows_package(size=200 * 1024 * 1024 + 1)
    r["package-sha256-uppercase"] = windows_package(sha256=wp["sha256"].upper())
    r["contact-201-characters"] = envelope(at(p2(), contact_ko="가" * 201), "signing-1")
    r["notes-2001-characters"] = envelope(with_entry(p2(), "windows", notes_ko="가" * 2001), "signing-1")
    # binding tool: an app takes them, the tool and the server check do not
    r["text-with-url"] = envelope(at(p1(), contact_ko="https://rd.example.com/get 에서 받으세요."), "signing-1")
    r["unknown-field"] = raw_file(part(replaced(p1t, '"seq":1,', '"seq":1,"expires":"2027-01-01T00:00:00Z",'),
                                       "signing-1"))
    r["not-canonical"] = raw_file(part(replaced(p1t, '{"v":1,', '{ "v":1,'), "signing-1"))
    # verdicts changed by the 2026-10-13 rules
    r["root-seq-jump"] = envelope(at(p1(), seq=5000, issued="2026-10-03T03:00:00Z"), "root-101")
    r["root-policy-raises-epoch"] = envelope(at(p1(), epoch=2, seq=1, issued="2026-10-03T03:00:00Z"), "root-101")
    r["revoke-in-the-same-epoch"] = declared(p2_3, {"epoch": 1, "revoked": [2], "issued": "2026-10-03T03:00:00Z",
                                                    "base": base_of(p2_3["platforms"])}, by="signing-1")
    p3_4n = with_entry(p3_4, "windows", auto_install_after=None)
    r["min-raised-with-new-grace"] = envelope(with_entry(p3_4n, "windows", min="1.6.1",
                                                         grace_until="2026-11-30T15:00:00Z"), "signing-2")
    r["new-entry-below-the-minimums"] = envelope(
        with_entry(p2_3, "android_play", min="1.0.0", latest="1.6.0", source="client-1.6.0",
                   version_code=262742100), "signing-1")
    r["revoked-key-with-declaration"] = declared(p4(), d2(), by="signing-1")
    r["epoch-differs-from-declaration"] = declared(at(p3_old, epoch=3, seq=1, issued="2026-10-06T03:00:00Z"), d2())
    r["good-declaration-bad-policy"] = raw_file(bad_part(p4t), part(d2t, "root-101", ROOT_DOMAIN))
    r["below-floor-in-epoch-2"] = "e2-s1"
    r["declaration-epoch-back"] = "e2-s2"
    r["declaration-of-epoch-1"] = declared(at(p3_old, seq=4, issued="2026-10-07T03:00:00Z"),
                                           {"epoch": 1, "revoked": [], "issued": "2026-10-06T03:00:00Z", "base": {}})
    r["duplicate-field"] = raw_file(part(replaced(p1t, '"seq":1,', '"seq":1,"seq":1,'), "signing-1"))
    # new in rounds 0 and 1
    r["relaxation-declaration-alone"] = raw_file(bad_part(policy_text(p6())),
                                                 part(decl_text(d3()), "root-101", ROOT_DOMAIN))
    r["raise-keeps-the-old-min"] = envelope(with_entry(p3_4n, "windows", min="1.6.1", grace_until="2026-11-30T15:00:00Z",
                                                       grace_min="1.6.0"), "signing-2")
    far = copy(p3_4n)
    for e in far["platforms"].values():
        e.update(min="1.6.1", latest="1.6.1", grace_until="2099-10-16T15:00:00Z", grace_min="1.6.0")
    r["raise-far-deadline-level-kept"] = envelope(far, "signing-2")
    w161 = dict(latest="1.6.1", notes_ko=NOTES, source="client-1.6.1", package=dict(PKG["windows", "1.6.1"]))
    r["raise-while-pending-same-deadline"] = envelope(with_entry(p2_3, "windows", min="1.6.1", **w161), "signing-1")
    r["raise-while-pending-later-deadline"] = envelope(
        with_entry(p2_3, "windows", min="1.6.1", grace_until="2026-10-23T15:00:00Z", **w161), "signing-1")
    r["raise-while-pending-later-deadline-pending-min-now"] = envelope(
        with_entry(p2_3, "windows", min="1.6.1", grace_until="2026-10-23T15:00:00Z", grace_min="1.6.0", **w161),
        "signing-1")
    play = dict(min="1.6.0", latest="1.6.0", grace_min="1.5.0", source="client-1.6.0", version_code=262742100)
    r["android-play-same-min-later-deadline"] = envelope(
        with_entry(p2_3, "android_play", grace_until="2026-10-30T15:00:00Z", **play), "signing-1")
    r["android-play-at-level"] = envelope(with_entry(p2_3, "android_play", grace_until=GRACE, **play), "signing-1")
    r["grace-min-without-grace-until"] = envelope(with_entry(p1_2, "windows", grace_min="1.4.0"), "signing-1")
    r["grace-until-without-grace-min"] = envelope(with_entry(p1_2, "windows", grace_until=GRACE), "signing-1")
    r["grace-min-not-below-min"] = envelope(with_entry(p1_2, "windows", grace_until=GRACE, grace_min="1.5.0"),
                                            "signing-1")
    r["grace-min-as-number"] = raw_file(part(replaced(p2t, '"grace_min":"1.5.0"', '"grace_min":150'), "signing-1"))
    r["policy-without-epoch"] = raw_file(part(replaced(p1t, '"epoch":1,', ''), "signing-1"))
    r["root-signed-policy-taken"] = envelope(at(p2(), seq=3), "root-101")
    r["declaration-without-base"] = raw_file(part(p4t, "signing-2"),
                                             part(decl_text({k: v for k, v in d2().items() if k != "base"}),
                                                  "root-101", ROOT_DOMAIN))
    r["declaration-base-bad-entry"] = raw_file(
        part(p4t, "signing-2"),
        part(replaced(d2t, '"base":{"windows":{"min":"1.6.0"', '"base":{"windows":{"min":"1.6"'), "root-101",
             ROOT_DOMAIN))
    r["declaration-base-grace-min-not-below-min"] = declared(p4(), d2(base={"windows": {
        "min": "1.6.0", "latest": "1.6.1", "grace_until": GRACE, "grace_min": "1.6.0"}}))
    # binding tool on the declaration: an app reads past them
    r["declaration-base-extra-member"] = raw_file(
        part(p4t, "signing-2"),
        part(replaced(d2t, '"grace_min":"1.5.0"}', '"grace_min":"1.5.0","os_min":"10"}'), "root-101", ROOT_DOMAIN))
    r["declaration-unknown-member"] = raw_file(part(p4t, "signing-2"),
                                               part(replaced(d2t, '"epoch":2,', '"epoch":2,"note":"x",'), "root-101",
                                                    ROOT_DOMAIN))
    r["declaration-revoked-null"] = raw_file(part(p4t, "signing-2"),
                                             part(replaced(d2t, '"revoked":[1]', '"revoked":null'), "root-101",
                                                  ROOT_DOMAIN))
    r["entry-null"] = raw_file(part(replaced(p1t, '"ios":{', '"android_play":null,"ios":{'), "signing-1"))
    r["declaration-fork"] = declared(p5(), d2(issued="2026-10-06T04:00:00Z"))
    r["declaration-by-second-root"] = declared(p4(), d2(), root_by="root-102")
    r["declaration-by-unpinned-root"] = declared(p4(), d2(), root_by="root-103")
    r["declaration-revokes-second-root"] = declared(p4(), d2(revoked=[1, 102]))
    r["epoch-2-without-declaration"] = envelope(p5(), "signing-2")
    r["e2-s2-stripped"] = "epoch-2-without-declaration"
    r["unknown-key-with-declaration"] = declared(p4(), d2(), by="signing-3")
    forged = copy(p3_4n)
    for name, e in forged["platforms"].items():
        e.update(min="99.0.0", latest="99.0.0")
        e.pop("grace_until", None)
        e.pop("grace_min", None)
        if name in ("windows", "android"):
            e["package"] = dict(PKG[name, "99.0.0"])
    r["forged-minimum"] = envelope(forged, "signing-1")
    r["forgery-healed-by-the-declaration"] = "e2-s1"
    r["forgery-then-the-newest"] = "e3-s2"
    r["forgery-then-declaration-alone"] = "good-declaration-bad-policy"
    only_windows = {"epoch": 2, "seq": 1, "issued": "2026-10-06T03:00:00Z", "server": SERVER, "contact_ko": CONTACT,
                    "platforms": {"windows": dict(p3()["platforms"]["windows"], min="1.6.0")}}
    for k in ("grace_until", "grace_min", "auto_install_after"):
        only_windows["platforms"]["windows"].pop(k)
    r["policy-below-its-base"] = declared(only_windows, {"epoch": 2, "revoked": [], "issued": "2026-10-06T03:00:00Z",
                                                         "base": {"windows": {"min": "1.6.1", "latest": "1.6.1"}}})
    r["issued-back"] = envelope(at(p3_old, seq=4, issued="2026-10-04T03:00:00Z"), "signing-1")
    r["issued-before-declaration"] = declared(at(p3_old, epoch=2, seq=1, issued="2026-10-05T03:00:00Z"), d2())
    # new in round 2
    s = json.loads(e1s1)["s"]
    alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
    last = alphabet.index(s[-3])
    assert last % 16 == 0
    r["signature-trailing-bits"] = replaced(e1s1, s, s[:-3] + alphabet[last + 1] + "==")
    r["body-not-utf8"] = r["e3-s2"][:-2] + b',"y":"\xff"}\n'
    r["lone-surrogate-in-known-member"] = raw_file(part(replaced(p1t, '"contact_ko":"%s"' % CONTACT,
                                                                 '"contact_ko":"\\ud800"'), "signing-1"))
    r["lone-surrogate-in-unknown-member"] = raw_file(part(replaced(p1t, '"seq":1,', '"seq":1,"x":"\\ud800",'),
                                                          "signing-1"))
    r["package-withdrawn"] = envelope(with_entry(p3_4, "windows", package=None, auto_install_after=None), "signing-2")
    r["declaration-revokes-old-root-number"] = declared(p4(), d2(revoked=[1, 103]))
    # decided on 2026-10-14 (spec 1-2, 1-3, the term `canonical`)
    r["issued-1999"] = envelope(at(p1(), issued="1999-12-31T23:59:59Z"), "signing-1")
    r["issued-year-0000"] = envelope(at(p2(), issued="0000-01-01T00:00:00Z"), "signing-1")
    r["package-build-1e400"] = raw_file(part(replaced(p2t, '"build":"1.6.0.37"', '"build":1e400'), "signing-1"))
    r["declaration-without-revoked"] = raw_file(part(p4t, "signing-2"),
                                                part(decl_text({k: v for k, v in d2().items() if k != "revoked"}),
                                                     "root-101", ROOT_DOMAIN))
    return r


# The model writes one abstract case for families of concrete ones: their concrete names and the
# member each one's Field reason names.
EXPANDED = {
    "version-*": [("version-two-numbers", "Field:min"), ("version-v-prefix", "Field:min"),
                  ("version-three-digit-patch", "Field:latest"), ("version-four-numbers", "Field:latest"),
                  ("version-plus-build", "Field:latest")],
    "package-*": [("package-file-install-exe", "Field:package.file"), ("package-file-qs", "Field:package.file"),
                  ("package-file-path", "Field:package.file"), ("package-size-zero", "Field:package.size"),
                  ("package-size-over-200-mib", "Field:package.size"),
                  ("package-sha256-uppercase", "Field:package.sha256")],
    "issued-not-rfc3339": [("issued-not-rfc3339", "Field:issued"), ("issued-year-0000", "Field:issued")],
    "contact-201-characters": [("contact-201-characters", "Field:contact_ko")],
    "notes-2001-characters": [("notes-2001-characters", "Field:notes_ko")],
}

# Cases where the model's abstract case stands for its concrete envelope only coarsely (the model
# reuses e2-s1 for them): only the outcome is compared with the model's case; the state is the
# model's run over the concrete bytes, as for every case.
COARSE = {
    "declaration-revoked-null": "the app reads revoked null as no revocation: revoked [] (spec 7-4)",
    "declaration-without-revoked": "the app reads no revoked as no revocation: revoked [] (the model reuses e2-s1)",
    "entry-null": "e1-s1 with android_play null: epoch 1, no declaration (spec 7-4)",
    "declaration-base-extra-member": "the model reuses e2-s1",
    "declaration-unknown-member": "the model reuses e2-s1",
}

WHY = {
    "e1-s1": "the first policy, by signing key 1: 1.6.0 is out, nothing is enforced",
    "e1-s2": "every min raised to 1.6.0 until 10-16 with grace_min 1.5.0 (spec 5-2)",
    "e1-s3": "by signing key 2: 1.6.1 with packages, an unattended install date, the Google Play entry",
    "e2-s1": "the declaration of epoch 2 by root key 101 revokes signing key 1; its base is e1-s3's requirements",
    "e2-s2": "the same declaration goes along; auto_install_after is removed (the emergency stop is no relaxation)",
    "e3-s1-relax": "the declaration of epoch 3 relaxes: every min 1.5.0 without deadlines (spec 5-12)",
    "e3-s2": "a signing key raises min again after the relaxation, grace_min 1.5.0",
    "e1-s2-again": "the same position with the same bytes: the app keeps what it has (Unchanged)",
    "fresh-install-e2-s2": "a fresh install takes the current policy and its declaration",
    "e3-s2-missed-the-relaxation": "an app that was off during the relaxation takes the declaration first and compares e3-s2 with its base (problems (다), (바))",
    "floor-exact": "a build whose floor is 2:2 takes the policy at 2:2",
    "seq-step-1000": "a policy 1000 above the stored seq",
    "fresh-install-seq-1000": "a fresh install with floor 1:0 takes seq 1000",
    "stop-auto-install": "removing auto_install_after is the emergency stop, not a relaxation",
    "delay-auto-install": "delaying auto_install_after is not a relaxation",
    "grace-earlier": "an earlier deadline asks more",
    "grace-removed": "no deadline at all asks more",
    "size-at-the-limit": "an envelope file of exactly 16384 bytes, its line feed counted",
    "relax-min-lowered": "a signing key lowers a min",
    "relax-grace-later": "a signing key puts off the deadline of an unchanged min",
    "relax-grace-added": "a signing key gives an unchanged min a deadline it did not have",
    "relax-latest-lowered": "a signing key lowers latest",
    "relax-entry-removed": "a signing key removes an entry",
    "tampered-policy": "one byte of the policy text changed after signing",
    "tampered-signature": "one character of the signature changed",
    "wake-domain": "signed under the relay's wake-command domain hjv-wake-v1",
    "policy-under-root-domain": "the policy signed under the declaration's domain",
    "another-key-as-1": "signed by another key under the number 1",
    "signature-with-identity-r": "R is the identity point: the equation of Ed25519 holds, a strict check (libsodium, verify_strict) refuses the small-order R",
    "another-root-as-101": "signed by root key 102 under the number 101",
    "unknown-key": "signed by a key the build does not pin (k = 3)",
    "envelope-v2": "an envelope version the app does not know",
    "policy-v2": "a correctly signed policy of version 2",
    "declaration-v2": "a correctly signed declaration of version 2",
    "wrong-server": "a correctly signed policy of another server",
    "key-number-zero": "the key number 0",
    "key-number-as-text": "the key number as a JSON string",
    "signature-63-bytes": "a signature of 63 bytes",
    "too-large": "a correctly signed envelope over 16 KiB",
    "size-one-over": "16385 bytes",
    "declaration-by-signing-key": "the declaration is signed by signing key 2 under its own number",
    "declaration-signing-key-as-101": "the declaration is signed by signing key 2 under the number 101",
    "declaration-under-policy-domain": "the declaration is signed by root key 101 under the policy's domain",
    "declaration-tampered": "the declaration's text changed after signing",
    "declaration-revokes-root": "the declaration lists a pinned root key (101) as revoked",
    "declaration-wrong-server": "a declaration of another server",
    "revoked-key-later": "signing key 1's policy of epoch 2 with the declaration that revokes it",
    "old-epoch-after-new": "a policy of epoch 1 after the app has moved to epoch 2",
    "signing-key-raises-epoch": "a policy of epoch 2 without a declaration",
    "root-policy-of-an-old-epoch": "a root key's policy of epoch 1 after epoch 2: refused for its epoch, whoever signed",
    "seq-lower": "an older policy after a newer one",
    "below-floor": "a policy at 1:4 for a build whose floor is 1:5",
    "below-floor-by-root": "the same by root key 101: the floor holds for every key",
    "same-position-other-bytes": "another policy at the position the app holds",
    "seq-step-1001": "a policy 1001 above the stored seq",
    "fresh-install-seq-1001": "a fresh install with floor 1:0 does not take seq 1001",
    "min-above-latest": "min 1.6.0 above latest 1.5.0",
    "version-two-numbers": "\"1.6\": the apps' version comparison reads it as 1.0.60",
    "version-v-prefix": "\"v1.6.0\"",
    "version-three-digit-patch": "\"1.5.100\": read as 1.6.0",
    "version-four-numbers": "\"1.6.0.1\": four numbers",
    "version-plus-build": "\"1.6.0+1\"",
    "issued-not-rfc3339": "issued is not of the form 2026-10-02T03:00:00Z",
    "package-file-install-exe": "a package name ending in install.exe would open the installer",
    "package-file-qs": "a package name with -qs- would run as quick support",
    "package-file-path": "a package name with a path",
    "package-size-zero": "a package of 0 bytes",
    "package-size-over-200-mib": "a package over 200 MiB",
    "package-sha256-uppercase": "a SHA-256 that is not 64 lowercase hex digits",
    "contact-201-characters": "contact_ko over 200 characters",
    "notes-2001-characters": "notes_ko over 2000 characters",
    "text-with-url": "a URL in contact_ko: the tool never signs one; an app shows the text as it is",
    "unknown-field": "a member the format does not have: the app skips it",
    "not-canonical": "a space the tool does not write",
    "root-seq-jump": "a root key's policy 4999 above the stored seq: no special rights (Jump)",
    "root-policy-raises-epoch": "only a declaration moves the epoch",
    "revoke-in-the-same-epoch": "a declaration of epoch 1: declarations start at epoch 2",
    "min-raised-with-new-grace": "a raise with a later deadline whose grace_min lets the old minimum go (problem (가))",
    "new-entry-below-the-minimums": "an android_play entry below its sibling android (problem (나))",
    "revoked-key-with-declaration": "the policy is signed by the key its own declaration revokes; the declaration is kept",
    "epoch-differs-from-declaration": "a policy of epoch 3 with the declaration of epoch 2; the declaration is kept",
    "good-declaration-bad-policy": "a good declaration with a policy part whose signature fails: the declaration is kept, key 1's policy is no longer enforced",
    "below-floor-in-epoch-2": "the policy at 2:1 for a build whose floor is 2:2; the declaration is kept",
    "declaration-epoch-back": "the declaration of epoch 2 after the app took epoch 3",
    "declaration-of-epoch-1": "a declaration of epoch 1 (epoch 1 never has one)",
    "duplicate-field": "a known member twice is a shape error",
    "relaxation-declaration-alone": "the relaxing declaration of epoch 3 with a policy part that fails: kept and enforced",
    "raise-keeps-the-old-min": "a raise with a later deadline that keeps the level in force (grace_min 1.6.0)",
    "raise-far-deadline-level-kept": "every min 1.6.1 until 2099 with grace_min 1.6.0",
    "raise-while-pending-same-deadline": "a second raise before the pending deadline, the same deadline",
    "raise-while-pending-later-deadline": "a second raise with a later deadline keeping grace_min 1.5.0 lets the pending minimum go",
    "raise-while-pending-later-deadline-pending-min-now": "derivation c: a later deadline with grace_min = the pending minimum",
    "android-play-same-min-later-deadline": "a new android_play entry whose deadline is later than its sibling's",
    "android-play-at-level": "a new android_play entry at its sibling's level",
    "grace-min-without-grace-until": "grace_min without grace_until",
    "grace-until-without-grace-min": "grace_until without grace_min",
    "grace-min-not-below-min": "grace_min equal to min",
    "grace-min-as-number": "grace_min as a JSON number",
    "policy-without-epoch": "a policy text without its required epoch",
    "relax-min-lowered-root": "the relaxation signed by root key 101: a root key relaxes by declaration only",
    "relax-grace-later-root": "the relaxation signed by root key 101",
    "relax-grace-added-root": "the relaxation signed by root key 101",
    "relax-latest-lowered-root": "the relaxation signed by root key 101",
    "relax-entry-removed-root": "the relaxation signed by root key 101",
    "relax-min-lowered-by-declaration": "the relaxed policy by key 2 with a declaration of epoch 2 whose base it is",
    "relax-grace-later-by-declaration": "the relaxed policy by key 2 with a declaration of epoch 2 whose base it is",
    "relax-grace-added-by-declaration": "the relaxed policy by key 2 with a declaration of epoch 2 whose base it is",
    "relax-latest-lowered-by-declaration": "the relaxed policy by key 2 with a declaration of epoch 2 whose base it is",
    "relax-entry-removed-by-declaration": "the relaxed policy by key 2 with a declaration of epoch 2 whose base it is",
    "root-signed-policy-taken": "a root key may sign a policy that asks no less (spec 4-2 Signer, X9)",
    "declaration-without-base": "a declaration without base",
    "declaration-base-bad-entry": "a base entry with min \"1.6\"",
    "declaration-base-grace-min-not-below-min": "a base entry whose grace_min is not below min",
    "declaration-base-extra-member": "a base entry with os_min: the app skips it, the tool never writes one",
    "declaration-unknown-member": "a declaration with an unknown member: the app skips it",
    "declaration-revoked-null": "revoked null: the app reads no revocation, the tool writes a list",
    "entry-null": "an entry that is null: the app reads no entry, the tool writes none",
    "declaration-fork": "another text of the declaration of the app's epoch",
    "declaration-by-second-root": "the declaration of epoch 2 signed by the second pinned root key 102",
    "declaration-by-unpinned-root": "a declaration signed by root key 103, which no vector build pins",
    "declaration-revokes-second-root": "the declaration revokes pinned root key 102",
    "epoch-2-without-declaration": "a policy of epoch 2 without its declaration",
    "e2-s2-stripped": "e2-s2 without its declaration",
    "unknown-key-with-declaration": "the declaration of epoch 2 with a policy of key 3: the declaration is kept",
    "forged-minimum": "a thief's policy (key 1, before its revocation): every min and latest 99.0.0",
    "forgery-healed-by-the-declaration": "the declaration that revokes the stolen key lifts the forgery",
    "forgery-then-the-newest": "the newest envelope reaches the device that took the forgery",
    "forgery-then-declaration-alone": "the declaration alone: the forgery is no longer enforced, the base is",
    "policy-below-its-base": "a policy of epoch 2 that asks less than its declaration's base",
    "issued-back": "a policy issued before the one the app holds",
    "issued-before-declaration": "a policy issued before its declaration",
    "signature-trailing-bits": "the character before == in s has non-zero unused bits (a lenient decoder reads the same bytes)",
    "body-not-utf8": "e3-s2 with a member whose string holds the byte 0xFF: refused before the declaration is read",
    "lone-surrogate-in-known-member": "contact_ko is the escape \\ud800: a shape error in a member that is read",
    "lone-surrogate-in-unknown-member": "an unknown member's value is the escape \\ud800: skipped",
    "package-withdrawn": "the windows package and auto_install_after removed (--withdraw-package): no relaxation",
    "declaration-revokes-old-root-number": "a declaration revoking [1, 103]: no vector build pins 103 (the tool refuses any 101-199, X2)",
    "issued-1999": "issued in 1999: an app reads the years 0001-9999, the tool writes and signs only 2000-2199 (spec 1-2)",
    "issued-year-0000": "issued in the year 0000, which no implementation reads (spec 1-2)",
    "package-build-1e400": "package.build is the number 1e400, beyond a 64-bit float: a shape error, as serde_json reads it (spec 1-3)",
    "declaration-without-revoked": "a declaration without revoked: the app reads no revocation; not canonical, the tool always writes the list",
}


# ------------------------------------------------------------------------------------------------
# Reading an envelope as an app does (spec 2-2, steps A, B, C), into the model's terms
# ------------------------------------------------------------------------------------------------

class Shape(Exception):
    """A shape error of 2-2."""


class Obj:
    def __init__(self, pairs):
        self.pairs = pairs


class Int:
    def __init__(self, literal):
        self.literal = literal
        self.value = int(literal)


class Flt:
    def __init__(self, literal):
        self.literal = literal


def _constant(name):
    raise Shape("%s is not JSON" % name)


def jload(text):
    try:
        return json.loads(text, object_pairs_hook=Obj, parse_int=Int, parse_float=Flt, parse_constant=_constant)
    except ValueError as e:
        raise Shape(str(e))


def surrogate(s):
    return any(0xD800 <= ord(c) <= 0xDFFF for c in s)


def members(obj, known):
    """The known members of a JSON object that is read: a name with a lone surrogate or a known
    member given twice is a shape error, a null member is absent, unknown members are skipped."""
    if not isinstance(obj, Obj):
        raise Shape("not an object")
    out, seen = {}, set()
    for name, value in obj.pairs:
        if surrogate(name):
            raise Shape("lone surrogate in a member name")
        if name in known:
            if name in seen:
                raise Shape("%s twice" % name)
            seen.add(name)
            if value is not None:
                out[name] = value
    return out


def as_int(value, lo=0, hi=2 ** 64 - 1):
    if not isinstance(value, Int) or value.literal.startswith("-") or not lo <= value.value <= hi:
        raise Shape("not an integer")
    return value.value


def as_str(value):
    if not isinstance(value, str) or surrogate(value):
        raise Shape("not a string")
    return value


def need(m, name, read):
    if name not in m:
        raise Shape("%s missing" % name)
    return read(m[name])


def opt(m, name, read):
    return read(m[name]) if name in m else None


def read_v(obj):
    """The pass that reads `v` alone (2-2)."""
    if not isinstance(obj, Obj):
        raise Shape("not an object")
    vs = []
    for name, value in obj.pairs:
        if surrogate(name):
            raise Shape("lone surrogate in a member name")
        if name == "v":
            vs.append(value)
    if len(vs) != 1 or vs[0] is None:
        raise Shape("v")
    return as_int(vs[0])


VERSION = re.compile(r"(0|[1-9][0-9]{0,2})\.(0|[1-9][0-9]{0,2})\.(0|[1-9][0-9]?)\Z")
UTC = re.compile(r"([0-9]{4})-([0-9]{2})-([0-9]{2})T([0-9]{2}):([0-9]{2}):([0-9]{2})Z\Z")


def num(text):
    """hbb_common::get_version_number of a strict x.y.z, or None."""
    m = VERSION.match(text)
    return None if m is None else int(m.group(1)) * 1000000 + int(m.group(2)) * 1000 + int(m.group(3)) * 10


def parse_utc(text):
    m = UTC.match(text)
    if m is None:
        return None
    y, mo, d, h, mi, s = (int(g) for g in m.groups())
    if not (1 <= mo <= 12 and 1 <= d <= calendar.monthrange(y, mo)[1] and h < 24 and mi < 60 and s < 60) or y < 1:
        return None
    return calendar.timegm((y, mo, d, h, mi, s))


def canonical_signature(s):
    """`s` as the canonical standard base64 of exactly 64 bytes (1-3), or None."""
    if len(s) != 88 or not re.fullmatch(r"[A-Za-z0-9+/]{86}==", s):
        return None
    sig = base64.b64decode(s)
    return sig if len(sig) == 64 and base64.b64encode(sig).decode() == s else None


def plain_name(text):
    return 1 <= len(text.encode()) <= 100 and re.fullmatch(r"[A-Za-z0-9._-]+", text) is not None


ENTRY_READ = ("min", "latest", "grace_until", "grace_min", "os_min", "notes_ko", "source", "package",
              "auto_install_after", "move_to_play")

PLACEHOLDER_POLICY = M.policy(1, 1, 0, {}, notes="placeholder: never read")
PLACEHOLDER_DECL = M.declaration(2, set(), 0, {}, variant="placeholder: never read")


def read_policy(text):
    """C3: (form, the model's policy text). The form is 'ok', 'Version', 'Server' or 'Field:<member>'
    in C3's order; a text that is not 'ok' is a placeholder unless the form is 'Server'."""
    try:
        if read_v(jload(text)) != 1:
            return "Version", PLACEHOLDER_POLICY
    except Shape:
        return "Field:json", PLACEHOLDER_POLICY
    try:
        m = members(jload(text), POLICY_ORDER)
        epoch, seq = need(m, "epoch", as_int), need(m, "seq", as_int)
        issued, server = need(m, "issued", as_str), need(m, "server", as_str)
        contact = opt(m, "contact_ko", as_str)
        plats = members(need(m, "platforms", lambda x: x), PLATFORMS)
        raw = {}
        for name in PLATFORMS:
            if name not in plats:
                continue
            em = members(plats[name], ENTRY_READ)
            e = {k: need(em, k, as_str) for k in ("min", "latest")}
            for k in ("grace_until", "grace_min", "os_min", "notes_ko", "source", "auto_install_after"):
                e[k] = opt(em, k, as_str)
            if "move_to_play" in em and not isinstance(em["move_to_play"], bool):
                raise Shape("move_to_play")
            if "package" in em:
                pm = members(em["package"], PACKAGE_ORDER)
                e["package"] = {"file": need(pm, "file", as_str), "size": need(pm, "size", as_int),
                                "sha256": need(pm, "sha256", as_str)}
                build_value(pm.get("build"))
            raw[name] = e
    except Shape:
        return "Field:json", PLACEHOLDER_POLICY
    if epoch < 1:
        return "Field:epoch", PLACEHOLDER_POLICY
    if seq < 1:
        return "Field:seq", PLACEHOLDER_POLICY
    issued_t = parse_utc(issued)
    if issued_t is None:
        return "Field:issued", PLACEHOLDER_POLICY
    if contact is not None and len(contact) > 200:
        return "Field:contact_ko", PLACEHOLDER_POLICY
    ents = {}
    for name, e in raw.items():
        lo, hi = num(e["min"]), num(e["latest"])
        if lo is None:
            return "Field:min", PLACEHOLDER_POLICY
        if hi is None:
            return "Field:latest", PLACEHOLDER_POLICY
        if lo > hi:
            return "Field:min", PLACEHOLDER_POLICY
        if e["os_min"] is not None and not re.fullmatch(r"[0-9.]{1,16}", e["os_min"]):
            return "Field:os_min", PLACEHOLDER_POLICY
        if e["notes_ko"] is not None and len(e["notes_ko"]) > 2000:
            return "Field:notes_ko", PLACEHOLDER_POLICY
        if e["source"] is not None and not plain_name(e["source"]):
            return "Field:source", PLACEHOLDER_POLICY
        gu = None if e["grace_until"] is None else parse_utc(e["grace_until"])
        if e["grace_until"] is not None and gu is None:
            return "Field:grace_until", PLACEHOLDER_POLICY
        gm = None if e["grace_min"] is None else num(e["grace_min"])
        if (e["grace_min"] is not None and gm is None) or (gu is None) != (gm is None) or \
                (gm is not None and not gm < lo):
            return "Field:grace_min", PLACEHOLDER_POLICY
        pk = e.get("package")
        if pk is not None:
            lower = pk["file"].lower()
            if not plain_name(pk["file"]) or "-qs-" in lower or lower.endswith("install.exe"):
                return "Field:package.file", PLACEHOLDER_POLICY
            if not 1 <= pk["size"] <= 200 * 1024 * 1024:
                return "Field:package.size", PLACEHOLDER_POLICY
            if not re.fullmatch(r"[0-9a-f]{64}", pk["sha256"]):
                return "Field:package.sha256", PLACEHOLDER_POLICY
        if e["auto_install_after"] is not None and parse_utc(e["auto_install_after"]) is None:
            return "Field:auto_install_after", PLACEHOLDER_POLICY
        ents[name] = M.Entry(lo, hi, gu, gm)
    auto = raw.get("windows", {}).get("auto_install_after") is not None
    pkg = all(raw[n].get("package") is not None for n in ("windows", "android") if n in raw)
    text_obj = M.policy(epoch, seq, issued_t, ents, auto=auto, pkg=pkg, notes="p sha256 " + sha256hex(text.encode()))
    return ("ok" if server == SERVER else "Server"), text_obj


def build_value(value):
    """package.build is read as any JSON value, as serde_json reads one: its strings must hold no
    lone surrogate, and a number must be within the range of a 64-bit float (spec 1-3, decided
    2026-10-14: `1e400` is a shape error, as the client already has it)."""
    if isinstance(value, str):
        as_str(value)
    elif isinstance(value, list):
        for v in value:
            build_value(v)
    elif isinstance(value, Obj):
        for name, v in value.pairs:
            as_str(name)
            build_value(v)
    elif isinstance(value, Flt):
        if value.literal.lstrip("-").lower() in ("inf", "nan") or math.isinf(float(value.literal)):
            raise Shape("a number beyond a 64-bit float")
    elif isinstance(value, Int):
        try:
            float(value.value)
        except OverflowError:
            raise Shape("a number beyond a 64-bit float")


def read_declaration(text):
    """B3: (form, the model's declaration text). 'Version', 'RootFormat' (with a placeholder for
    what the model cannot hold), 'RootServer' or 'ok'; epoch >= 2 and the base entries' rules are
    left to the model's own text rules when the versions are strict."""
    try:
        if read_v(jload(text)) != 1:
            return "Version", PLACEHOLDER_DECL
    except Shape:
        return "RootFormat", PLACEHOLDER_DECL
    try:
        m = members(jload(text), DECL_ORDER)
        server, epoch = need(m, "server", as_str), need(m, "epoch", as_int)
        revoked = opt(m, "revoked", lambda x: x)
        if revoked is not None and not isinstance(revoked, list):
            raise Shape("revoked")
        revoked = [as_int(x) for x in revoked or []]
        issued = need(m, "issued", as_str)
        bm = members(need(m, "base", lambda x: x), PLATFORMS)
        base = {}
        for name in PLATFORMS:
            if name in bm:
                em = members(bm[name], BASE_ORDER)
                base[name] = ({k: need(em, k, as_str) for k in ("min", "latest")},
                              {k: opt(em, k, as_str) for k in ("grace_until", "grace_min")})
    except Shape:
        return "RootFormat", PLACEHOLDER_DECL
    issued_t = parse_utc(issued)
    if issued_t is None or any(not 1 <= k <= 65535 for k in revoked):
        return "RootFormat", PLACEHOLDER_DECL
    ents = {}
    for name, (req, pair) in base.items():
        lo, hi = num(req["min"]), num(req["latest"])
        gu = None if pair["grace_until"] is None else parse_utc(pair["grace_until"])
        gm = None if pair["grace_min"] is None else num(pair["grace_min"])
        if lo is None or hi is None or (pair["grace_until"] is not None and gu is None) or \
                (pair["grace_min"] is not None and gm is None):
            return "RootFormat", PLACEHOLDER_DECL
        ents[name] = M.Entry(lo, hi, gu, gm)
    decl = M.declaration(epoch, set(revoked), issued_t, ents, variant="root.p sha256 " + sha256hex(text.encode()))
    return ("ok" if server == SERVER else "RootServer"), decl


def read_part(value):
    m = members(value, ("k", "p", "s"))
    k = need(m, "k", lambda x: as_int(x, 1, 65535))
    p = need(m, "p", as_str)
    sig = canonical_signature(need(m, "s", as_str))
    if sig is None:
        raise Shape("s")
    return k, p, sig


def abstract(body, build):
    """The model's Env for an envelope file, read under `build` (the pinned keys of the vectors)."""
    if len(body) > MAX_ENVELOPE_BYTES:
        return M.Env(M.Part(0, PLACEHOLDER_POLICY, False), None, form="TooLarge")
    try:
        text = body.decode("utf-8")
    except UnicodeDecodeError:
        return M.Env(M.Part(0, PLACEHOLDER_POLICY, False), None, form="Envelope")
    try:
        top = jload(text)
        if read_v(top) != 1:
            return M.Env(M.Part(0, PLACEHOLDER_POLICY, False), None, form="Version")
    except Shape:
        return M.Env(M.Part(0, PLACEHOLDER_POLICY, False), None, form="Envelope")
    try:
        m = members(top, ("v", "k", "p", "s", "root"))
        k, p, sig = read_part(Obj([(n, m[n]) for n in ("k", "p", "s") if n in m]))
        root = None if "root" not in m else read_part(m["root"])
    except Shape:
        return M.Env(M.Part(0, PLACEHOLDER_POLICY, False), None, form="Envelope")
    root_part = None
    if root is not None:
        rk, rp, rsig = root
        valid = rk in build.roots and verify_strict(ROOTS[rk], message(ROOT_DOMAIN, rp), rsig)
        form, decl = read_declaration(rp) if valid else ("ok", PLACEHOLDER_DECL)
        root_part = M.Part(rk, decl, valid, form)
    key = ROOTS.get(k) if k in build.roots else PINS.get(k) if k in build.pins else None
    valid = key is not None and verify_strict(key, message(POLICY_DOMAIN, p), sig)
    form, pol = read_policy(p) if valid else ("ok", PLACEHOLDER_POLICY)
    return M.Env(M.Part(k, pol, valid, form), root_part)


# ------------------------------------------------------------------------------------------------
# Canonical form (the manifest's `canonical`, spec 7-4 round 2, 4-1, 4-8 item 1)
# ------------------------------------------------------------------------------------------------

def strict_load(text):
    """JSON with no duplicate member and no null anywhere, numbers as written."""
    def pairs(ps):
        names = [n for n, _ in ps]
        if len(set(names)) != len(names) or any(v is None for _, v in ps):
            raise Shape("duplicate or null")
        return dict(ps)
    try:
        return json.loads(text, object_pairs_hook=pairs, parse_int=Int, parse_float=Flt, parse_constant=_constant)
    except ValueError as e:
        raise Shape(str(e))


def typed(value, kind):
    if kind == "int":
        # an unsigned 64-bit integer, as the tool's reader keeps it (u64)
        if not isinstance(value, Int) or value.literal.startswith("-") or value.value > 2 ** 64 - 1:
            raise Shape("int")
        return value.value
    if kind == "str":
        if not isinstance(value, str):
            raise Shape("str")
        return value
    if kind == "true":
        if value is not True:
            raise Shape("true")
        return value
    raise AssertionError(kind)


def rewrite(obj, schema, required=()):
    """`obj` written again member by member in the schema's order with the schema's types."""
    if not isinstance(obj, dict) or set(obj) - set(schema):
        raise Shape("members")
    if any(r not in obj for r in required):
        raise Shape("required")
    out = {}
    for name, kind in schema.items():
        if name in obj:
            out[name] = kind(obj[name]) if callable(kind) else typed(obj[name], kind)
    return out


ENTRY_SCHEMA = {"min": "str", "latest": "str", "grace_until": "str", "grace_min": "str", "os_min": "str",
                "notes_ko": "str", "source": "str",
                "package": lambda x: rewrite(x, {"file": "str", "size": "int", "sha256": "str", "build": "str"},
                                             PACKAGE_ORDER),
                "auto_install_after": "str", "move_to_play": "true", "version_code": "int",
                "testflight_build": "str"}
POLICY_SCHEMA = {"v": "int", "epoch": "int", "seq": "int", "issued": "str", "server": "str", "contact_ko": "str",
                 "platforms": lambda x: rewrite(x, {n: (lambda e: rewrite(e, ENTRY_SCHEMA, ("min", "latest")))
                                                    for n in PLATFORMS})}
BASE_SCHEMA = {n: (lambda e: rewrite(e, {k: "str" for k in BASE_ORDER})) for n in PLATFORMS}
def int_list(value):
    if not isinstance(value, list):
        raise Shape("list")
    return [typed(i, "int") for i in value]


DECL_SCHEMA = {"v": "int", "server": "str", "epoch": "int", "revoked": int_list, "issued": "str",
               "base": lambda x: rewrite(x, BASE_SCHEMA)}


def canonical(body):
    """Whether the file is exactly what hjv-release writes, by the form the tool's strict reader
    keeps: one line and a line feed; no duplicate member and no null; only the members of 1-3, in
    1-3's order, with their JSON types (unsigned 64-bit integers, `move_to_play` only true,
    package.build a string); serde_json's escaping; the envelope's k 1-65535 and every s the
    canonical base64 of 64 bytes; a policy text with its required members, a declaration text
    with all six of its members (the term `canonical`, decided 2026-10-14). Value rules (v,
    versions, times, lengths, the server, epoch >= 2) are not part of it: a canonical text that
    breaks one gets the app's reason from the tool."""
    try:
        text = body.decode("utf-8")
        if not text.endswith("\n") or "\n" in text[:-1]:
            return False
        text = text[:-1]
        env = strict_load(text)
        part_schema = {"k": "int", "p": "str", "s": "str"}
        e = rewrite(env, dict(v="int", **part_schema, root=lambda x: rewrite(x, part_schema, ("k", "p", "s"))),
                    ("v", "k", "p", "s"))
        for prt in [e] + ([e["root"]] if "root" in e else []):
            if not 1 <= prt["k"] <= 65535 or canonical_signature(prt["s"]) is None:
                return False
        if dumps(e) != text:
            return False
        p = rewrite(strict_load(e["p"]), POLICY_SCHEMA, ("v", "epoch", "seq", "issued", "server", "platforms"))
        if dumps(p) != e["p"]:
            return False
        if "root" in e:
            d = rewrite(strict_load(e["root"]["p"]), DECL_SCHEMA, DECL_ORDER)
            if dumps(d) != e["root"]["p"]:
                return False
        return True
    except (Shape, UnicodeDecodeError):
        return False


# ------------------------------------------------------------------------------------------------
# Cases: the model's list of 7-4, its verdicts on the concrete files, the manifest
# ------------------------------------------------------------------------------------------------

def cases_of_2026_10_14(hist):
    """The cases the main session decided on 2026-10-14, after the model's list of 7-4 (spec 1-2,
    1-3 and the term `canonical`), written with the model's own constructors: model.py stays the
    byte-identical copy of the model."""
    def case(name, after, env, expect, binding="app", canonical=True):
        return M.Case(name, after, env, M.VECTOR_BUILD, expect, None, None, binding, canonical)
    p11 = hist["e1-s1"].pol.text
    return [
        # an app reads the years 0001-9999; the tool writes and signs 2000-2199 only (spec 1-2)
        case("issued-1999", [], M.Env(M.Part(1, M.policy(1, 1, M.H(1), p11.ents, notes="issued 1999"))), "Taken",
             binding="tool"),
        # a number serde_json cannot read is a shape error, in package.build as anywhere (spec 1-3)
        case("package-build-1e400", ["e1-s1"], M.Env(M.Part(1, hist["e1-s2"].pol.text, form="Field:json")),
             "Refused(Field:json)", canonical=False),
        # a declaration without one of the members the tool always writes is not canonical
        case("declaration-without-revoked", [], hist["e2-s1"], "Taken", binding="tool", canonical=False),
    ]


# The model's cases whose `canonical` the definition of 2026-10-14 changes: a declaration without
# one of its six members is not canonical.
CANONICAL_2026_10_14 = {"declaration-without-base": False}


def concrete_cases():
    """[(name, model case, expected reason or None)] in the model's order, families expanded, then
    the cases of 2026-10-14."""
    cases, hist = M.fixed_cases()
    out = []
    for c in cases + cases_of_2026_10_14(hist):
        if c.name in CANONICAL_2026_10_14:
            c = c._replace(canonical=CANONICAL_2026_10_14[c.name])
        for name, reason in EXPANDED.get(c.name, [(c.name, None)]):
            out.append((name, c, reason))
    return out


def build_of(c):
    fe, fs = c.build.floor
    return M.Build("vectors floor %d:%d" % (fe, fs), c.build.pins, c.build.roots, (fe, fs))


def after_names(c):
    return [a if isinstance(a, str) else M.ENV_LABEL[a] for a in c.after]


def state_json(st, texts):
    """The manifest's state (7-4): the device epoch E, revoked, the held declaration's and policy's
    SHA-256, the held policy's seq, what is enforced, MAY_INSTALL."""
    d = M.derived(st)
    return {"epoch": d.E,
            "seq": None if st.held is None else st.held.pol.seq,
            "revoked": sorted(d.REVOKED),
            "root_sha256": None if st.decl is None else texts[id(st.decl)],
            "root_k": None if st.root is None else st.root.k,
            "p_sha256": None if st.held is None else texts[id(st.held.pol)],
            "enforces": "nothing" if d.EFF is None else d.EFF.source,
            "may_install": d.MAY_INSTALL}


def remember_texts(body, env, texts):
    """SHA-256 of the texts behind the model's objects (the manifest names texts by them)."""
    try:
        e = json.loads(body)
    except ValueError:
        return
    if not isinstance(e, dict):
        return
    if isinstance(e.get("p"), str):
        texts.setdefault(id(env.pol.text), sha256hex(e["p"].encode()))
    if env.root is not None and isinstance(e.get("root"), dict) and isinstance(e["root"].get("p"), str):
        texts.setdefault(id(env.root.text), sha256hex(e["root"]["p"].encode()))


def run_case(c, files, name):
    """The model over the concrete files: (state before, state after, outcome, then-outcome, texts)."""
    build = build_of(c)
    texts = {}
    st = M.fresh(build)
    for a in after_names(c):
        env = abstract(files[a], build)
        remember_texts(files[a], env, texts)
        st, out = M.accept(st, env)
        if out.kind not in ("Taken", "Unchanged"):
            raise SystemExit("%s: %s before it is %r" % (name, a, out))
    env = abstract(files[name], build)
    remember_texts(files[name], env, texts)
    st2, out = M.accept(st, env)
    then = None
    if c.then is not None:
        then_name = M.ENV_LABEL[c.then[0]]
        tenv = abstract(files[then_name], build)
        remember_texts(files[then_name], tenv, texts)
        _, then = M.accept(st2, tenv)
    return st, st2, out, then, texts


LABEL_FREE = ("epoch", "revoked", "enforces", "may_install", "root_k")


def compare_with_model(name, c, reason, result, hist):
    """The model's verdict on the concrete files must be its verdict on its own case."""
    st, st2, out, then, _ = result
    ast, ast2, aout, athen, problems = M.case_result(c, hist)
    if problems:
        raise SystemExit("%s: the model disagrees with spec 7-4 itself: %s" % (name, problems))
    want = repr(aout)
    if reason is not None:
        want = "Refused(%s)" % reason
    if repr(out) != want:
        raise SystemExit("%s: the concrete file gives %r, the model's case %s" % (name, out, want))
    if name not in COARSE:
        a, b = M.summary(ast2), M.summary(st2)
        for key in LABEL_FREE:
            if a[key] != b[key]:
                raise SystemExit("%s: %s is %r on the files and %r in the model's case" % (name, key, b[key], a[key]))
        for key in ("root", "held"):
            if (a[key] is None) != (b[key] is None):
                raise SystemExit("%s: %s differs from the model's case" % (name, key))
        if (st2 == st) != (ast2 == ast):
            raise SystemExit("%s: the state changed in one and not in the other" % name)
    s = M.summary(st2)
    for key, val in list((c.kept or {}).items()) + list((c.after_state or {}).items()):
        if key in LABEL_FREE and s[key] != val:
            raise SystemExit("%s: %s is %r, spec 7-4 says %r" % (name, key, s[key], val))
    if c.then is not None and repr(then) != c.then[1]:
        raise SystemExit("%s: then %r, spec 7-4 says %s" % (name, then, c.then[1]))


def where(name, body_of, binding, accepted):
    if isinstance(body_of[name], str):
        return where(body_of[name], body_of, binding, accepted)
    return ("tool" if binding[name] == "tool" else "accepted" if accepted[name] else "refused") + "/%s.json" % name


def generate():
    """Every file of the vectors (path -> bytes) and the manifest's cases."""
    made = recipes()
    cases = concrete_cases()
    names = [n for n, _, _ in cases]
    assert len(set(names)) == len(names)
    missing = set(names) ^ set(made)
    if missing:
        raise SystemExit("cases without a recipe or recipes without a case: %s" % sorted(missing))
    files = {}
    for n in names:
        target = made[n]
        while isinstance(target, str):
            target = made[target]
        files[n] = target
    _, hist = M.fixed_cases()
    results, rows = {}, []
    binding = {n: c.binding for n, c, _ in cases}
    accepted = {}
    for name, c, reason in cases:
        res = run_case(c, files, name)
        compare_with_model(name, c, reason, res, hist)
        results[name] = res
        accepted[name] = res[2].kind != "Refused"
    for name, c, reason in cases:
        st, st2, out, then, texts = results[name]
        can = canonical(files[name])
        if can != c.canonical:
            raise SystemExit("%s: canonical by the form is %s, the model's case says %s" % (name, can, c.canonical))
        row = {"name": name, "file": where(name, made, binding, accepted),
               "after": [where(a, made, binding, accepted) for a in after_names(c)]}
        if c.build.floor != (1, 0):
            row["floor"] = "%d:%d" % c.build.floor
        row["binding"] = c.binding
        row["canonical"] = can
        if out.kind == "Refused":
            row["expect"] = "reject"
            row["reason"] = out.reason
            if st2 != st:
                s = state_json(st2, texts)
                row["state_after_refusal"] = {k: s[k] for k in ("epoch", "revoked", "root_sha256", "root_k",
                                                                "p_sha256", "enforces", "may_install")}
        else:
            row["expect"] = "accept"
            row["outcome"] = out.kind.lower()
            row["state_after"] = state_json(st2, texts)
        if c.binding == "tool":
            row["app_expect"] = row["expect"]
        if c.then is not None:
            row["then_accept"] = where(M.ENV_LABEL[c.then[0]], made, binding, accepted)
            row["then_outcome"] = then.kind.lower()
        row["why"] = WHY[name]
        rows.append(row)
    out_files = {}
    for row in rows:
        out_files.setdefault(row["file"], files[row["name"]])
    keyfile = ("# The update keys of the test vectors. TEST KEYS: their private halves are in this\n"
               "# repository (src/keys.rs, TEST_KEYS), so a real build must never hold them.\n"
               "HJV_UPDATE_PUBKEY=%s\nHJV_UPDATE_ROOT_PUBKEY=%s\nHJV_POLICY_FLOOR=1:0\n" % (SIGNING_LINE, ROOT_LINE))
    out_files["update-keys.env"] = keyfile.encode()
    d2_part = part(decl_text(d2()), "root-101", ROOT_DOMAIN)
    out_files["root-declaration-e2.json"] = (dumps(d2_part) + "\n").encode()
    out_files["covers.json"] = covers_file()
    manifest = {
        "about": "Test vectors of HJViewer's signed version policy (the trust rules of 1.6.0, whose executable "
                 "form is model.py). README.md explains the fields. Made by generate.py from the model; checked by "
                 "generate.py --check and by tools/hjv-release/tests/vectors.rs.",
        "model_sha256": MODEL_SHA256,
        "server": SERVER,
        "domains": {"policy": POLICY_DOMAIN.decode(), "root_declaration": ROOT_DOMAIN.decode()},
        "key_file": {"file": "update-keys.env", "HJV_UPDATE_PUBKEY": SIGNING_LINE, "HJV_UPDATE_ROOT_PUBKEY": ROOT_LINE,
                     "HJV_POLICY_FLOOR": "1:0", "fingerprint_sha256": fingerprint()},
        "test_keys": [{"name": n, "role": role, "id": k, "seed_hex": KEY[n][1].hex(),
                       "public": "%d:%s" % (k, base64.b64encode(PUBLIC[n]).decode()), "in_the_key_file": pinned,
                       "about": about}
                      for n, role, k, _, pinned, about in TEST_KEYS],
        "root_declaration": {"file": "root-declaration-e2.json", "p": d2_part["p"],
                             "signed_bytes_sha256": sha256hex(message(ROOT_DOMAIN, d2_part["p"]))},
        "covers": {"file": "covers.json", "about": "COVERS(new, old) of spec 2-3 for every pair of 16 entries"},
        "key_file_example": {"file": "keyfile/signing-1.age", "passphrase": "test passphrase, not a secret",
                             "made_by": "the age crate 0.11.5 (scrypt work factor 10), not by this tool",
                             "holds": "test key signing-1"},
        "research_vector": {"file": "research/version-policy.json",
                            "note": "the first draft of the format (build/minver/policy-distribution): the signature "
                                    "is good for signing key 1, the policy text has no epoch and is refused as a "
                                    "policy"},
        "cases": rows,
    }
    out_files["manifest.json"] = (json.dumps(manifest, ensure_ascii=False, indent=2) + "\n").encode()
    return out_files


def covers_file():
    vs = (M.v00, M.v15, M.v16, M.v161)
    tx = {None: None, M.T16: "2026-10-16T15:00:00Z", M.T23: "2026-10-23T15:00:00Z"}

    def js(e):
        d = {"min": M.VERSION_TEXT[e.min], "latest": M.VERSION_TEXT[e.latest]}
        if e.gu is not None:
            d.update(grace_until=tx[e.gu], grace_min=M.VERSION_TEXT[e.gm])
        return d
    ents = M.entry_domain(versions=vs, deadlines=(None, M.T16, M.T23))
    rows = [{"new": js(n), "old": js(o), "covers": M.covers(n, o)} for n in ents for o in ents]
    assert len(rows) == 256
    return ("[\n" + ",\n".join(dumps(r) for r in rows) + "\n]\n").encode()


# ------------------------------------------------------------------------------------------------
# --check: the files on disk against what this script makes, and the model over every file
# ------------------------------------------------------------------------------------------------

KEPT = {"README.md", "generate.py", "model.py", "research/version-policy.json", "research/vector.json",
        "keyfile/signing-1.age"}


def check_disk(made):
    bad = []
    for path, data in sorted(made.items()):
        try:
            with open(os.path.join(HERE, path), "rb") as f:
                if f.read() != data:
                    bad.append(path + " differs")
        except OSError:
            bad.append(path + " is missing")
    for root, _, names in os.walk(HERE):
        for n in names:
            rel = os.path.relpath(os.path.join(root, n), HERE).replace(os.sep, "/")
            if rel not in made and rel not in KEPT and "__pycache__" not in rel:
                bad.append(rel + " is not a vector")
    return bad


def canonical_edges(made):
    """What canonical() decides that no vector shows: an integer is unsigned and 64-bit, as the
    tool's strict reader keeps it (seq of the first vector, changed)."""
    first = made["accepted/e1-s1.json"]
    bad = []
    for literal, want in [(b"18446744073709551615", True), (b"18446744073709551616", False),
                          (b"-0", False), (b"1.0", False)]:
        body = first.replace(b'\\"seq\\":1,', b'\\"seq\\":' + literal + b",", 1)
        if body == first or canonical(body) != want:
            bad.append("canonical() of seq %s is not %s" % (literal.decode(), want))
    # a declaration without one of its six members (the term `canonical`, decided 2026-10-14)
    declared_file = made["accepted/e2-s1.json"]
    env = json.loads(declared_file)
    if not canonical(declared_file):
        bad.append("canonical() of e2-s1 is not True")
    for member in DECL_ORDER:
        root = dict(env["root"], p=dumps({k: v for k, v in json.loads(env["root"]["p"]).items() if k != member}))
        if canonical((dumps(dict(env, root=root)) + "\n").encode()):
            bad.append("canonical() of a declaration without %s is True" % member)
    return bad


def model_over_disk():
    """The model's verdict on every file on disk, compared with manifest.json on disk: a check that
    does not use the recipes above."""
    with open(os.path.join(HERE, "manifest.json"), "rb") as f:
        manifest = json.load(f)
    cases = {n: c for n, c, _ in concrete_cases()}
    bad = []
    if manifest.get("model_sha256") != MODEL_SHA256:
        bad.append("manifest.json names another model")
    for row in manifest["cases"]:
        c = cases.get(row["name"])
        if c is None:
            bad.append("%s is no case of the model" % row["name"])
            continue
        build = M.Build("disk", c.build.pins, c.build.roots,
                        tuple(int(x) for x in row.get("floor", "1:0").split(":")))
        read = lambda path: open(os.path.join(HERE, path), "rb").read()  # noqa: E731
        texts = {}
        st = M.fresh(build)
        for path in row["after"]:
            body = read(path)
            env = abstract(body, build)
            remember_texts(body, env, texts)
            st, out = M.accept(st, env)
            if out.kind == "Refused":
                bad.append("%s: %s before it is %r" % (row["name"], path, out))
        body = read(row["file"])
        env = abstract(body, build)
        remember_texts(body, env, texts)
        st2, out = M.accept(st, env)
        got = {"expect": "reject" if out.kind == "Refused" else "accept"}
        if out.kind == "Refused":
            got["reason"] = out.reason
        else:
            got["outcome"] = out.kind.lower()
            got["state_after"] = state_json(st2, texts)
        if out.kind == "Refused" and st2 != st:
            s = state_json(st2, texts)
            got["state_after_refusal"] = {k: s[k] for k in row.get("state_after_refusal", {})}
        for key, val in got.items():
            if row.get(key) != val:
                bad.append("%s: %s is %r by the model, %r in the manifest" % (row["name"], key, val, row.get(key)))
        if out.kind == "Refused" and st2 == st and "state_after_refusal" in row:
            bad.append("%s: the manifest has a state after the refusal, the model none" % row["name"])
        if canonical(body) != row["canonical"]:
            bad.append("%s: canonical %s, the manifest %s" % (row["name"], canonical(body), row["canonical"]))
        if "then_accept" in row:
            tbody = read(row["then_accept"])
            tenv = abstract(tbody, build)
            _, tout = M.accept(st2, tenv)
            if tout.kind.lower() != row.get("then_outcome"):
                bad.append("%s: then %r" % (row["name"], tout))
    return bad, len(manifest["cases"])


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--check", action="store_true", help="compare with the files instead of writing them")
    args = ap.parse_args(argv)
    if not verify_strict(PUBLIC["signing-1"], b"x", sign(KEY["signing-1"][1], b"x")):
        raise SystemExit("Ed25519 does not verify its own signature")
    made = generate()
    if args.check:
        bad = check_disk(made) + canonical_edges(made)
        disk_bad, n = model_over_disk() if not bad else ([], 0)
        for line in bad + disk_bad:
            print("vectors: " + line)
        if bad or disk_bad:
            return 1
        print("vectors: %d files as generate.py makes them; the model agrees with manifest.json on all %d cases"
              % (len(made), n))
        return 0
    for path, data in made.items():
        full = os.path.join(HERE, path)
        os.makedirs(os.path.dirname(full), exist_ok=True)
        with open(full, "wb") as f:
            f.write(data)
    print("vectors: wrote %d files" % len(made))
    return 0


if __name__ == "__main__":
    sys.exit(main())

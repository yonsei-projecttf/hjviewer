# SPDX-License-Identifier: AGPL-3.0-only
"""Tests of deploy/hjv-update-verify.sh, the server's check of a signed version policy (the trust
rules' specification 4-8; docs/server-runbook.md 8). Offline: the envelopes are made here, signed
with the published test keys of tools/hjv-release (seeds of 32 consecutive bytes) by an Ed25519 of
this file's own, so that the script's way through OpenSSL is checked by an independent signer.

    python3 -I -B -m unittest discover -s scripts -p 'test_hjv_update_verify.py'

The script's Python part is loaded as a module for the rules; the install check and the vector
runner are run as the command a server runs. scripts/ci.sh deploy runs this file.
"""
import base64
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import types
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SCRIPT = os.path.join(ROOT, "deploy", "hjv-update-verify.sh")
SERVER = "rd.example.com"
NAMES = ("windows", "android", "android_play", "ios")


def load_script():
    """The script's Python part as a module, with the script's line numbers in tracebacks."""
    with open(SCRIPT, encoding="utf-8") as f:
        text = f.read()
    marker = "<< 'PYTHON'\n"
    start = text.index(marker) + len(marker)
    end = text.index("\nPYTHON\n", start)
    module = types.ModuleType("hjv_update_verify")
    module.__file__ = SCRIPT
    code = "\n" * text.count("\n", 0, start) + text[start:end + 1]
    sys.modules["hjv_update_verify"] = module
    exec(compile(code, SCRIPT, "exec"), module.__dict__)
    return module


H = load_script()

# ---- an Ed25519 signer of these tests (RFC 8032 5.1.6) ---------------------------------------------

P = 2**255 - 19
L = 2**252 + 27742317777372353535851937790883648493


def _scalar(seed):
    h = hashlib.sha512(seed).digest()
    a = int.from_bytes(h[:32], "little")
    return (a & ((1 << 254) - 8)) | (1 << 254), h[32:]


def _encode(p):
    x, y, z, _ = p
    zi = pow(z, P - 2, P)
    x, y = x * zi % P, y * zi % P
    return (y | (x & 1) << 255).to_bytes(32, "little")


def public(seed):
    return _encode(H._mul(_scalar(seed)[0], H.BASE))


def sign(seed, message):
    a, prefix = _scalar(seed)
    r = int.from_bytes(hashlib.sha512(prefix + message).digest(), "little") % L
    R = _encode(H._mul(r, H.BASE))
    k = int.from_bytes(hashlib.sha512(R + public(seed) + message).digest(), "little") % L
    return R + ((r + k * a) % L).to_bytes(32, "little")


def sign_identity_r(seed, message):
    """A signature whose R is the identity: [s]B = R + [k]A holds with s = k·a. No honest signer
    makes one; libsodium refuses it, OpenSSL takes it."""
    a, _ = _scalar(seed)
    R = (1).to_bytes(32, "little")
    k = int.from_bytes(hashlib.sha512(R + public(seed) + message).digest(), "little") % L
    return R + (k * a % L).to_bytes(32, "little")


def seed_from(first):
    return bytes((first + i) % 256 for i in range(32))


# tools/hjv-release's test keys (src/keys.rs TEST_KEYS), and one more root key no build pins
SEEDS = {1: seed_from(0x40), 2: seed_from(0x80), 3: seed_from(0xC0), 101: seed_from(0xA0), 102: seed_from(0xE0),
         103: seed_from(0x10)}


def b64(raw):
    return base64.b64encode(raw).decode()


def key_file(signing=(1, 2), roots=(101, 102), floor="1:0", keys=None):
    keys = keys or {k: public(SEEDS[k]) for k in (*signing, *roots)}
    return ("# test keys\nHJV_UPDATE_PUBKEY=%s\nHJV_UPDATE_ROOT_PUBKEY=%s\nHJV_POLICY_FLOOR=%s\n" % (
        ",".join("%d:%s" % (k, b64(keys[k])) for k in signing),
        ",".join("%d:%s" % (k, b64(keys[k])) for k in roots), floor))


# ---- texts, written the tool's way by this file's own writer -------------------------------------

ENTRY_ORDER = ("min", "latest", "grace_until", "grace_min", "os_min", "notes_ko", "source", "package",
               "auto_install_after", "move_to_play", "version_code", "testflight_build")


def J(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def day(d, hour=3, month=10):
    return "2026-%02d-%02dT%02d:00:00Z" % (month, d, hour)


T14, T16, T17, T23, T30 = day(14, 15), day(16, 15), day(17, 15), day(23, 15), day(30, 15)
NOV30 = day(30, 15, 11)


def entry(mn, latest, gu=None, gm=None, **more):
    e = dict(more, min=mn, latest=latest)
    if gu is not None:
        e["grace_until"] = gu
    if gm is not None:
        e["grace_min"] = gm
    return {k: e[k] for k in ENTRY_ORDER if k in e}


def package(name, size, sha256, build):
    return {"file": name, "size": size, "sha256": sha256, "build": build}


def policy(epoch, seq, issued, entries, contact="설치 파일은 관리자에게 받으세요.", server=SERVER, v=1):
    p = {"v": v, "epoch": epoch, "seq": seq, "issued": issued, "server": server}
    if contact is not None:
        p["contact_ko"] = contact
    p["platforms"] = {n: {k: entries[n][k] for k in ENTRY_ORDER if k in entries[n]} for n in NAMES if n in entries}
    return J(p)


def requirements(entries):
    return {n: {k: e[k] for k in ("min", "latest", "grace_until", "grace_min") if k in e} for n, e in entries.items()}


def declaration(epoch, revoked, issued, base, server=SERVER):
    return J({"v": 1, "server": server, "epoch": epoch, "revoked": sorted(revoked), "issued": issued,
              "base": {n: base[n] for n in NAMES if n in base}})


def part(k, text, context=H.POLICY_CONTEXT, seed=None, signature=None):
    message = context + b"\0" + text.encode("utf-8")
    sig = signature(seed or SEEDS[k], message) if callable(signature) else signature
    if sig is None:
        sig = sign(seed or SEEDS[k], message)
    return {"k": k, "p": text, "s": b64(sig)}


def root_part(k, text, context=H.ROOT_CONTEXT, **kw):
    return part(k, text, context, **kw)


def envelope(pol, root=None, v=1):
    e = {"v": v, "k": pol["k"], "p": pol["p"], "s": pol["s"]}
    if root is not None:
        e["root"] = root
    return (J(e) + "\n").encode("utf-8")


# The history of spec 7-4, in texts: e1-s1 .. e3-s2 and the declarations of epochs 2 and 3.
WIN16 = package("HJViewer-1.6.0-windows-x86_64.exe", 4096, hashlib.sha256(b"w" * 4096).hexdigest(), "1.6.0.37")
APK16 = package("HJViewer-1.6.0-android-arm64.apk", 3000, hashlib.sha256(b"a" * 3000).hexdigest(), "262740100")
WIN161 = package("HJViewer-1.6.1-windows-x86_64.exe", 5000, hashlib.sha256(b"W" * 5000).hexdigest(), "1.6.1.41")
APK161 = package("HJViewer-1.6.1-android-arm64.apk", 3100, hashlib.sha256(b"A" * 3100).hexdigest(), "262782100")
PACKAGE_BYTES = {WIN16["file"]: b"w" * 4096, APK16["file"]: b"a" * 3000, WIN161["file"]: b"W" * 5000,
                 APK161["file"]: b"A" * 3100}

E11 = {"windows": entry("1.5.0", "1.6.0", os_min="10", source="client-1.6.0", package=WIN16),
       "android": entry("1.5.0", "1.6.0", os_min="22", source="client-1.6.0", package=APK16),
       "ios": entry("1.5.0", "1.6.0", os_min="13.0", source="client-1.6.0", testflight_build="262740100")}
E12 = {"windows": entry("1.6.0", "1.6.0", T16, "1.5.0", source="client-1.6.0", package=WIN16),
       "android": entry("1.6.0", "1.6.0", T16, "1.5.0", source="client-1.6.0", package=APK16),
       "ios": entry("1.6.0", "1.6.0", T16, "1.5.0", source="client-1.6.0", testflight_build="262740100")}
E13 = {"windows": entry("1.6.0", "1.6.1", T16, "1.5.0", notes_ko="연결이 끊긴 뒤 다시 붙지 않던 문제를 고쳤습니다.\n한글 입력을 고쳤습니다.",
                        source="client-1.6.1", package=WIN161, auto_install_after=day(7)),
       "android": entry("1.6.0", "1.6.1", T16, "1.5.0", source="client-1.6.1", package=APK161, move_to_play=True),
       "android_play": entry("1.6.0", "1.6.1", source="client-1.6.1", version_code=262782100),
       "ios": entry("1.6.0", "1.6.0", T16, "1.5.0", source="client-1.6.0", testflight_build="262740100")}
E22 = {n: {k: v for k, v in e.items() if k != "auto_install_after"} for n, e in E13.items()}
E31 = {"windows": entry("1.5.0", "1.6.1", source="client-1.6.1", package=WIN161),
       "android": entry("1.5.0", "1.6.1", source="client-1.6.1", package=APK161),
       "android_play": entry("1.5.0", "1.6.1", source="client-1.6.1", version_code=262782100),
       "ios": entry("1.5.0", "1.6.0", source="client-1.6.0", testflight_build="262740100")}
E32 = {"windows": entry("1.6.1", "1.6.1", T23, "1.5.0", source="client-1.6.1", package=WIN161),
       "android": entry("1.6.1", "1.6.1", T23, "1.5.0", source="client-1.6.1", package=APK161),
       "android_play": entry("1.6.1", "1.6.1", T23, "1.5.0", source="client-1.6.1", version_code=262782100),
       "ios": entry("1.6.0", "1.6.0", T23, "1.5.0", source="client-1.6.0", testflight_build="262740100")}
D2 = declaration(2, [1], day(6), requirements(E13))
D3 = declaration(3, [1], day(8), requirements(E31))
R2, R3 = root_part(101, D2), root_part(101, D3)
P11, P12, P13 = policy(1, 1, day(1), E11), policy(1, 2, day(2), E12), policy(1, 3, day(5), E13)
P21, P22 = policy(2, 1, day(6), E13), policy(2, 2, day(7), E22)
P31, P32 = policy(3, 1, day(8), E31), policy(3, 2, day(9), E32)
HISTORY = {
    "e1-s1": envelope(part(1, P11)),
    "e1-s2": envelope(part(1, P12)),
    "e1-s3": envelope(part(2, P13)),
    "e2-s1": envelope(part(2, P21), R2),
    "e2-s2": envelope(part(2, P22), R2),
    "e3-s1-relax": envelope(part(2, P31), R3),
    "e3-s2": envelope(part(2, P32), R3),
}
ORDER = list(HISTORY)


def upto(name):
    return ORDER[: ORDER.index(name) + 1]


def sha(text):
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def base_kept(D, held=None, may=False, epoch=None):
    return {"root_sha256": sha(D), "p_sha256": None if held is None else sha(held), "enforces": "base",
            "may_install": may, "revoked": sorted(json.loads(D)["revoked"]), "epoch": epoch or json.loads(D)["epoch"]}


def sized(target):
    """e1-s2's position with notes that make the envelope exactly `target` bytes (Korean letters
    are three bytes each; an entry's notes stay within 2000 characters)."""
    def made(notes):
        ents = dict(E11, windows=dict(E11["windows"], notes_ko="가" * 2000),
                    android=dict(E11["android"], notes_ko="가" * 2000), ios=dict(E11["ios"], notes_ko=notes))
        return envelope(part(1, policy(1, 2, day(2), ents)))
    rest = target - len(made(""))
    data = made("가" * (rest // 3) + "x" * (rest % 3))
    assert len(data) == target and rest // 3 + rest % 3 <= 2000
    return data


def cases():
    """The cases of spec 7-4 that texts of this file can make, with the expectations of the spec
    (not of the script): (name, after, envelope bytes, floor, expectation, kept, then, binding,
    canonical, state_after)."""
    out = []

    def case(name, after, data, expect, floor="1:0", kept=None, then=None, binding="app", canonical=True,
             state=None):
        out.append((name, list(after), data, floor, expect, kept, then, binding, canonical, state))

    for n in ORDER:
        case(n, upto(n)[:-1], HISTORY[n], "taken")
    case("e1-s2-again", ["e1-s1", "e1-s2"], HISTORY["e1-s2"], "unchanged")
    case("fresh-install-e2-s2", [], HISTORY["e2-s2"], "taken",
         state={"epoch": 2, "revoked": [1], "p_sha256": sha(P22), "root_sha256": sha(D2), "root_k": 101,
                "enforces": "policy", "may_install": True})
    case("e3-s2-missed-the-relaxation", upto("e2-s2"), HISTORY["e3-s2"], "taken")
    case("floor-exact", [], HISTORY["e2-s2"], "taken", floor="2:2")
    case("seq-step-1000", ["e1-s1"], envelope(part(1, policy(1, 1001, day(3), E11))), "taken")
    case("fresh-install-seq-1000", [], envelope(part(1, policy(1, 1000, day(3), E11))), "taken")
    case("stop-auto-install", upto("e1-s3"), envelope(part(2, policy(1, 4, day(6), E22))), "taken")
    case("grace-earlier", ["e1-s1", "e1-s2"],
         envelope(part(1, policy(1, 3, day(3), {n: dict(e, grace_until=T14) for n, e in E12.items()}))), "taken")
    case("grace-removed", ["e1-s1", "e1-s2"],
         envelope(part(1, policy(1, 3, day(3), dict(E12, windows=entry("1.6.0", "1.6.0", package=WIN16))))), "taken")
    relax = {
        "relax-min-lowered": (["e1-s1", "e1-s2"], dict(E12, ios=entry("1.5.0", "1.6.0", T16, "1.4.0"))),
        "relax-grace-later": (["e1-s1", "e1-s2"], dict(E12, windows=entry("1.6.0", "1.6.0", T17, "1.5.0", package=WIN16))),
        "relax-grace-added": (["e1-s1"], dict(E11, windows=entry("1.5.0", "1.6.0", T16, "1.4.0", package=WIN16))),
        "relax-latest-lowered": (upto("e1-s3"), dict(E13, windows=entry("1.6.0", "1.6.0", T16, "1.5.0", package=WIN16))),
        "relax-entry-removed": (["e1-s1", "e1-s2"], {n: e for n, e in E12.items() if n != "ios"}),
    }
    for name, (after, ents) in relax.items():
        last = json.loads(json.loads(HISTORY[after[-1]])["p"])
        text = policy(1, last["seq"] + 1, last["issued"], ents)
        case(name, after, envelope(part(1, text)), "Relaxation")
        case(name + "-root", after, envelope(part(101, text)), "Relaxation")
        Dx = declaration(2, [], last["issued"], requirements(ents))
        case(name + "-by-declaration", after, envelope(part(2, policy(2, 1, last["issued"], ents)), root_part(101, Dx)),
             "taken")
    tampered = json.loads(HISTORY["e1-s1"])
    tampered["s"] = b64(bytes([base64.b64decode(tampered["s"])[0] ^ 1]) + base64.b64decode(tampered["s"])[1:])
    case("tampered-signature", [], (J(tampered) + "\n").encode(), "BadSignature")
    case("tampered-policy", [], envelope(dict(part(1, P11), p=P11.replace("1.5.0", "1.4.0", 1))), "BadSignature")
    case("wake-domain", [], envelope(part(1, P11, context=b"hjv-wake-v1")), "BadSignature")
    case("policy-under-root-domain", [], envelope(part(1, P11, context=H.ROOT_CONTEXT)), "BadSignature")
    case("another-key-as-1", [], envelope(dict(part(3, P11), k=1)), "BadSignature")
    case("signature-with-identity-r", [], envelope(part(1, P11, signature=sign_identity_r)), "BadSignature")
    case("another-root-as-101", ["e1-s1"], envelope(dict(part(102, policy(1, 2, day(2), E11)), k=101)), "BadSignature")
    case("unknown-key", [], envelope(part(3, P11)), "UnknownKey")
    case("envelope-v2", [], envelope(part(1, P11), v=2), "Version")
    case("policy-v2", [], envelope(part(1, policy(1, 1, day(1), E11, v=2))), "Version")
    case("declaration-v2", [], envelope(part(2, P21), root_part(101, D2.replace('{"v":1,', '{"v":2,', 1))), "Version")
    case("wrong-server", [], envelope(part(1, policy(1, 1, day(1), E11, server="rd.example.org"))), "Server")
    case("declaration-wrong-server", [], envelope(part(2, P21), root_part(101, declaration(
        2, [1], day(6), requirements(E13), server="rd.example.org"))), "RootServer")
    case("declaration-by-signing-key", [], envelope(part(2, P21), root_part(2, D2)), "RootKey")
    case("declaration-signing-key-as-101", [], envelope(part(2, P21), dict(root_part(2, D2), k=101)), "RootSignature")
    case("declaration-under-policy-domain", [], envelope(part(2, P21), root_part(101, D2, context=H.POLICY_CONTEXT)),
         "RootSignature")
    case("declaration-revokes-root", [], envelope(part(2, P21), root_part(101, declaration(
        2, [1, 101], day(6), requirements(E13)))), "RootRevokesRoot")
    case("declaration-revokes-second-root", [], envelope(part(2, P21), root_part(101, declaration(
        2, [1, 102], day(6), requirements(E13)))), "RootRevokesRoot")
    case("revoked-key-later", upto("e2-s1"), envelope(part(1, policy(2, 2, day(7), E13)), R2), "RevokedKey")
    case("old-epoch-after-new", upto("e2-s1"), envelope(part(2, policy(1, 4, day(7), E13))), "Epoch")
    case("signing-key-raises-epoch", ["e1-s1"], envelope(part(2, policy(2, 1, day(6), E13))), "Epoch")
    case("root-policy-of-an-old-epoch", upto("e2-s1"), envelope(part(101, policy(1, 5000, day(7), E11))), "Epoch")
    case("seq-lower", ["e1-s1", "e1-s2"], HISTORY["e1-s1"], "Rollback")
    case("below-floor", [], envelope(part(1, policy(1, 4, day(3), E11))), "Rollback", floor="1:5")
    case("below-floor-by-root", [], envelope(part(101, policy(1, 4, day(3), E11))), "Rollback", floor="1:5")
    case("same-position-other-bytes", ["e1-s1"], envelope(part(1, policy(1, 1, day(1), E11, contact="다른 문구"))),
         "Fork")
    case("seq-step-1001", ["e1-s1"], envelope(part(1, policy(1, 1002, day(3), E11))), "Jump")
    case("fresh-install-seq-1001", [], envelope(part(1, policy(1, 1001, day(3), E11))), "Jump")
    case("root-seq-jump", ["e1-s1"], envelope(part(101, policy(1, 5000, day(3), E11))), "Jump")
    case("root-policy-raises-epoch", ["e1-s1"], envelope(part(101, policy(2, 1, day(3), E11))), "Epoch")
    case("min-above-latest", ["e1-s1"], envelope(part(1, policy(1, 2, day(2), dict(E11, windows=entry(
        "1.6.0", "1.5.0", package=WIN16))))), "Field:min")
    for name, change, member in (
            ("version-four-numbers", ("windows", "min", "1.6.0.1"), "min"),
            ("version-two-numbers", ("android", "latest", "1.6"), "latest"),
            ("version-v-prefix", ("ios", "min", "v1.6.0"), "min"),
            ("version-three-digit-patch", ("windows", "latest", "1.6.100"), "latest"),
            ("issued-not-rfc3339", None, "issued"),
            ("package-file-install-exe", ("windows", "package", dict(WIN16, file="HJViewer-1.6.0-install.exe")),
             "package.file"),
            ("package-file-path", ("windows", "package", dict(WIN16, file="../HJViewer.exe")), "package.file"),
            ("package-file-qs", ("windows", "package", dict(WIN16, file="HJViewer-qs-1.6.0.exe")), "package.file"),
            ("package-sha256-uppercase", ("android", "package", dict(APK16, sha256=APK16["sha256"].upper())),
             "package.sha256"),
            ("package-size-zero", ("android", "package", dict(APK16, size=0)), "package.size"),
            ("package-size-over-200-mib", ("android", "package", dict(APK16, size=200 * 1024 * 1024 + 1)),
             "package.size"),
            ("os-min-text", ("ios", "os_min", "iOS 13"), "os_min"),
            ("source-with-space", ("windows", "source", "client 1.6.0"), "source"),
            ("auto-install-not-a-time", ("windows", "auto_install_after", "soon"), "auto_install_after"),
            ("grace-until-with-offset", ("ios", "grace_until", "2026-10-16T15:00:00+09:00"), "grace_until")):
        ents = {n: dict(e) for n, e in E12.items()}
        issued = day(2)
        if change is None:
            issued = "2026-10-02 03:00:00Z"
        elif change[1] == "grace_until":
            ents[change[0]]["grace_until"] = change[2]
        else:
            ents[change[0]][change[1]] = change[2]
        case(name, ["e1-s1"], envelope(part(1, policy(1, 2, issued, ents))), "Field:" + member)
    case("contact-201-characters", [], envelope(part(1, policy(1, 1, day(1), E11, contact="가" * 201))),
         "Field:contact_ko")
    case("notes-2001-characters", [], envelope(part(1, policy(1, 1, day(1), dict(E11, ios=entry(
        "1.5.0", "1.6.0", notes_ko="가" * 2001))))), "Field:notes_ko")
    case("contact-200-characters", [], envelope(part(1, policy(1, 1, day(1), E11, contact="가" * 200))), "taken")
    case("text-with-url", [], envelope(part(1, policy(1, 1, day(1), E11, contact="https://rd.example.com/ 에서"))),
         "taken", binding="tool")
    case("revoke-in-the-same-epoch", ["e1-s1", "e1-s2"], envelope(part(1, policy(1, 3, day(3), E12)), root_part(
        101, declaration(1, [2], day(3), requirements(E12)))), "RootFormat")
    case("min-raised-with-new-grace", upto("e1-s3"), envelope(part(2, policy(1, 4, day(6), dict(
        E13, windows=entry("1.6.1", "1.6.1", NOV30, "1.5.0", package=WIN161))))), "Relaxation")
    case("new-entry-below-the-minimums", ["e1-s1", "e1-s2"], envelope(part(1, policy(1, 3, day(3), dict(
        E12, android_play=entry("1.0.0", "1.6.0", version_code=262740100))))), "Relaxation")
    case("revoked-key-with-declaration", [], envelope(part(1, P21), R2), "RevokedKey", kept=base_kept(D2))
    case("epoch-differs-from-declaration", [], envelope(part(2, policy(3, 1, day(6), E13)), R2), "Epoch",
         kept=base_kept(D2))
    case("good-declaration-bad-policy", ["e1-s1"], envelope(dict(part(2, P21), p=P21.replace("1.6.1", "1.6.2", 1)), R2),
         "BadSignature", kept=base_kept(D2, held=P11), then="e2-s2")
    case("below-floor-in-epoch-2", [], HISTORY["e2-s1"], "Rollback", floor="2:2", kept=base_kept(D2))
    case("declaration-epoch-back", upto("e3-s1-relax"), HISTORY["e2-s2"], "RootEpoch")
    case("declaration-of-epoch-1", upto("e2-s1"), envelope(part(2, policy(1, 4, day(7), E13)), root_part(
        101, J({"v": 1, "server": SERVER, "epoch": 1, "revoked": [], "issued": day(6), "base": {}}))), "RootFormat")
    case("relaxation-declaration-alone", upto("e2-s2"), envelope(dict(part(2, P31), p=P31.replace(
        "1.5.0", "1.4.0", 1)), R3), "BadSignature", kept=base_kept(D3, held=P22), then="e3-s2")
    case("raise-keeps-the-old-min", upto("e1-s3"), envelope(part(2, policy(1, 4, day(6), dict(
        E13, windows=entry("1.6.1", "1.6.1", NOV30, "1.6.0", package=WIN161))))), "taken")
    case("raise-far-deadline-level-kept", upto("e1-s3"), envelope(part(2, policy(1, 4, day(6), {
        n: dict(e, min="1.6.1", latest="1.6.1", grace_until="2099-01-01T00:00:00Z", grace_min="1.6.0")
        for n, e in E13.items()}))), "taken")
    case("raise-while-pending-same-deadline", ["e1-s1", "e1-s2"], envelope(part(1, policy(1, 3, day(3), dict(
        E12, windows=entry("1.6.1", "1.6.1", T16, "1.5.0", package=WIN161))))), "taken")
    case("raise-while-pending-later-deadline", ["e1-s1", "e1-s2"], envelope(part(1, policy(1, 3, day(3), dict(
        E12, windows=entry("1.6.1", "1.6.1", T23, "1.5.0", package=WIN161))))), "Relaxation")
    case("raise-while-pending-later-deadline-pending-min-now", ["e1-s1", "e1-s2"], envelope(part(1, policy(
        1, 3, day(3), dict(E12, windows=entry("1.6.1", "1.6.1", T23, "1.6.0", package=WIN161))))), "taken")
    case("android-play-same-min-later-deadline", ["e1-s1", "e1-s2"], envelope(part(1, policy(1, 3, day(3), dict(
        E12, android_play=entry("1.6.0", "1.6.0", T30, "1.5.0", version_code=262740100))))), "Relaxation")
    case("android-play-at-level", ["e1-s1", "e1-s2"], envelope(part(1, policy(1, 3, day(3), dict(
        E12, android_play=entry("1.6.0", "1.6.0", T16, "1.5.0", version_code=262740100))))), "taken")
    for name, e in (("grace-min-without-grace-until", entry("1.5.0", "1.6.0", None, "1.4.0", package=WIN16)),
                    ("grace-until-without-grace-min", entry("1.5.0", "1.6.0", T16, None, package=WIN16)),
                    ("grace-min-not-below-min", entry("1.5.0", "1.6.0", T16, "1.5.0", package=WIN16))):
        case(name, ["e1-s1"], envelope(part(1, policy(1, 2, day(2), dict(E11, windows=e)))), "Field:grace_min")
    case("root-signed-policy-taken", ["e1-s1", "e1-s2"], envelope(part(101, policy(1, 3, day(2), E12))), "taken")
    case("declaration-without-base", [], envelope(part(2, P21), root_part(101, J(
        {"v": 1, "server": SERVER, "epoch": 2, "revoked": [1], "issued": day(6)}))), "RootFormat",
         canonical=False)
    case("declaration-without-revoked", [], envelope(part(2, P21), root_part(101, J(
        {"v": 1, "server": SERVER, "epoch": 2, "issued": day(6), "base": requirements(E13)}))), "taken",
         binding="tool", canonical=False)
    case("declaration-base-bad-entry", [], envelope(part(2, P21), root_part(101, declaration(
        2, [1], day(6), {"windows": {"min": "1.6", "latest": "1.6.1"}}))), "RootFormat")
    case("declaration-base-grace-min-not-below-min", [], envelope(part(2, P21), root_part(101, declaration(
        2, [1], day(6), {"windows": {"min": "1.6.0", "latest": "1.6.1", "grace_until": T16,
                                     "grace_min": "1.6.0"}}))), "RootFormat")
    case("declaration-fork", upto("e2-s1"), envelope(part(2, P22), root_part(101, declaration(
        2, [1], day(6, 4), requirements(E13)))), "RootFork")
    case("declaration-by-second-root", [], envelope(part(2, P21), root_part(102, D2)), "taken",
         state={"epoch": 2, "revoked": [1], "root_k": 102})
    case("declaration-by-unpinned-root", [], envelope(part(2, P21), root_part(103, D2)), "RootKey")
    case("epoch-2-without-declaration", [], envelope(part(2, P22)), "Epoch")
    case("e2-s2-stripped", upto("e2-s1"), envelope(part(2, P22)), "Epoch")
    case("unknown-key-with-declaration", [], envelope(part(3, P21), R2), "UnknownKey", kept=base_kept(D2))
    forged = policy(1, 4, day(6), {n: entry("99.0.0", "99.0.0") for n in E13})
    HISTORY["forged-minimum"] = envelope(part(1, forged))
    case("forged-minimum", upto("e1-s3"), HISTORY["forged-minimum"], "taken")
    case("forgery-healed-by-the-declaration", upto("e1-s3") + ["forged-minimum"], HISTORY["e2-s1"], "taken")
    case("forgery-then-the-newest", upto("e1-s3") + ["forged-minimum"], HISTORY["e3-s2"], "taken")
    case("forgery-then-declaration-alone", upto("e1-s3") + ["forged-minimum"],
         envelope(dict(part(2, P21), p=P21.replace("1.6.1", "1.6.2", 1)), R2), "BadSignature",
         kept=base_kept(D2, held=forged))
    Db = declaration(2, [], day(6), {"windows": {"min": "1.6.1", "latest": "1.6.1"}})
    case("policy-below-its-base", [], envelope(part(2, policy(2, 1, day(6), {"windows": entry(
        "1.6.0", "1.6.1", package=WIN161)})), root_part(101, Db)), "Relaxation", kept=base_kept(Db))
    case("issued-back", upto("e1-s3"), envelope(part(1, policy(1, 4, day(4), E13))), "Issued")
    case("issued-before-declaration", [], envelope(part(2, policy(2, 1, day(5), E13)), R2), "Issued",
         kept=base_kept(D2))
    case("package-withdrawn", upto("e1-s3"), envelope(part(2, policy(1, 4, day(6), dict(
        E22, windows={k: v for k, v in E22["windows"].items() if k != "package"})))), "taken",
         state={"may_install": True})
    case("declaration-revokes-old-root-number", [], envelope(part(2, P21), root_part(101, declaration(
        2, [1, 103], day(6), requirements(E13)))), "taken", binding="tool")
    # not canonical: refused by item 1 before anything else is read
    e11 = json.loads(HISTORY["e1-s1"])

    def raw(text):
        return text if isinstance(text, bytes) else text.encode("utf-8")

    text = J(e11)
    s = e11["s"]
    case("key-number-zero", [], raw(text.replace('"k":1', '"k":0', 1) + "\n"), "Envelope", canonical=False)
    case("key-number-as-text", [], raw(text.replace('"k":1', '"k":"1"', 1) + "\n"), "Envelope", canonical=False)
    case("signature-63-bytes", [], raw(text.replace(s, b64(base64.b64decode(s)[:63])) + "\n"), "Envelope",
         canonical=False)
    last = s[-3]
    stray = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
    twin = stray[stray.index(last) | 1]
    case("signature-trailing-bits", [], raw(text.replace(s, s[:-3] + twin + "==") + "\n"), "Envelope",
         canonical=False)
    case("duplicate-field", [], envelope(part(1, P11.replace('"seq":1,', '"seq":1,"seq":1,', 1))), "Field:json",
         canonical=False)
    case("grace-min-as-number", ["e1-s1"], envelope(part(1, P12.replace('"grace_min":"1.5.0"', '"grace_min":150', 1))),
         "Field:json", canonical=False)
    case("policy-without-epoch", [], envelope(part(1, P11.replace('"epoch":1,', "", 1))), "Field:json",
         canonical=False)
    case("lone-surrogate-in-known-member", [], envelope(part(1, P11.replace(
        "설치 파일은 관리자에게 받으세요.", "\\ud800", 1))), "Field:json", canonical=False)
    case("lone-surrogate-in-unknown-member", [], envelope(part(1, P11[:-1] + ',"x":"\\ud800"}')), "taken",
         binding="tool", canonical=False)
    case("unknown-field", [], envelope(part(1, P11[:-1] + ',"later":1}')), "taken", binding="tool", canonical=False)
    case("not-canonical", [], envelope(part(1, P11.replace(",", ", "))), "taken", binding="tool", canonical=False)
    case("entry-null", [], envelope(part(1, P11.replace('"platforms":{', '"platforms":{"android_play":null,', 1))),
         "taken", binding="tool", canonical=False)
    case("declaration-unknown-member", [], envelope(part(2, P21), root_part(101, D2[:-1] + ',"note":"x"}')), "taken",
         binding="tool", canonical=False)
    case("declaration-revoked-null", [], envelope(part(2, P21), root_part(101, D2.replace('"revoked":[1]',
                                                                                        '"revoked":null'))),
         "taken", binding="tool", canonical=False)
    case("declaration-base-extra-member", [], envelope(part(2, P21), root_part(101, D2.replace(
        '"windows":{"min":"1.6.0","latest":"1.6.1"', '"windows":{"min":"1.6.0","latest":"1.6.1","os_min":"10"', 1))),
         "taken", binding="tool", canonical=False)
    body = json.loads(HISTORY["e3-s2"])
    body["y"] = "x"
    case("body-not-utf8", upto("e2-s2"), (J(body).replace('"y":"x"', '"y":"\udcff"', 1)
                                          .encode("utf-8", "surrogateescape") + b"\n"), "Envelope", canonical=False)
    case("size-at-the-limit", ["e1-s1"], sized(16384), "taken")
    case("size-one-over", ["e1-s1"], sized(16385), "TooLarge")
    return out


CASES = cases()


def write_vectors(directory, rows=None):
    """The cases above as shared vectors (the manifest of spec 7-4, as the trust rules' model prints
    it), with update-keys.env and covers.json."""
    os.makedirs(os.path.join(directory, "envelopes"), exist_ok=True)
    paths = {}

    def put(name, data):
        path = "envelopes/%s.json" % name
        with open(os.path.join(directory, path), "wb") as f:
            f.write(data)
        paths[name] = path
        return path

    for name, data in HISTORY.items():
        put(name, data)
    manifest = {"server": SERVER, "cases": []}
    for name, after, data, floor, expect, kept, then, binding, canonical, state in CASES:
        row = {"name": name, "file": put(name, data), "after": [paths[a] for a in after], "floor": floor,
               "binding": binding, "canonical": canonical}
        if binding == "tool":
            row["app_expect"] = "accept" if expect in ("taken", "unchanged") else "reject"
        elif expect in ("taken", "unchanged"):
            row.update(expect="accept", outcome=expect)
            if state:
                row["state_after"] = state
        else:
            row.update(expect="reject", reason=expect)
            if kept:
                row["state_after_refusal"] = kept
        if then:
            row.update(then_accept=paths[then], then_outcome="taken")
        manifest["cases"].append(row)
    for row in rows or []:
        manifest["cases"].append(row)
    with open(os.path.join(directory, "manifest.json"), "w", encoding="utf-8") as f:
        json.dump(manifest, f, ensure_ascii=False, indent=1)
    with open(os.path.join(directory, "update-keys.env"), "w", encoding="utf-8") as f:
        f.write(key_file())
    with open(os.path.join(directory, "covers.json"), "w", encoding="utf-8") as f:
        json.dump(covers_rows(), f)
    return manifest


def entry_domain():
    """Entries over versions {0.0.0, 1.5.0, 1.6.0, 1.6.1} and deadlines {none, T1, T2} that keep the
    rules of 1-3 (spec 7-4's covers.json)."""
    versions = ("0.0.0", "1.5.0", "1.6.0", "1.6.1")
    out = []
    for mn in versions:
        out.append({"min": mn})
        for gu in (T16, T23):
            for gm in versions:
                if H.version_number(gm) < H.version_number(mn):
                    out.append({"min": mn, "grace_until": gu, "grace_min": gm})
    return out


def threshold(e, t):
    hi = H.version_number(e["min"])
    if "grace_until" not in e or t >= H.parse_utc(e["grace_until"]):
        return hi
    return H.version_number(e["grace_min"])


def brute_covers(n, o):
    times = {0}
    for e in (n, o):
        if "grace_until" in e:
            t = H.parse_utc(e["grace_until"])
            times.update((t - 1, t))
    return all(threshold(n, t) >= threshold(o, t) for t in times)


def covers_rows():
    ents = entry_domain()
    return [{"new": n, "old": o, "covers": brute_covers(n, o)} for n in ents for o in ents]


def entry_of(e):
    return H.Entry(H.version_number(e["min"]), H.version_number(e.get("latest", e["min"])),
                   H.parse_utc(e["grace_until"]) if "grace_until" in e else None,
                   H.version_number(e["grace_min"]) if "grace_min" in e else None)


def run_script(*args, cwd=None):
    done = subprocess.run(["bash", SCRIPT, *args], capture_output=True, cwd=cwd, timeout=300)
    return done.returncode, done.stdout.decode("utf-8", "replace"), done.stderr.decode("utf-8", "replace")


class Common(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.verifier = H.Verifier()
        cls.build = H.Build(SERVER, {k: public(SEEDS[k]) for k in (1, 2)}, {k: public(SEEDS[k]) for k in (101, 102)},
                            (1, 0))

    @classmethod
    def tearDownClass(cls):
        cls.verifier.close()

    def take(self, names, build=None):
        st = H.State()
        for n in names:
            st, out = H.accept(build or self.build, st, H.read_envelope(HISTORY[n]), self.verifier)
            self.assertIn(out, ("taken", "unchanged"), n)
        return st


class Texts(Common):
    def test_versions_and_times(self):
        for good, n in (("1.6.0", 1_006_000), ("0.0.0", 0), ("999.999.99", 999_999_990), ("1.5.99", 1_005_990)):
            self.assertEqual(H.version_number(good), n)
        for bad in ("", "1", "1.6", "v1.6.0", "1.5.100", "1.6.0.1", "1.6.0+1", "1.06.0", "01.6.0", "1.6.00",
                    " 1.6.0", "1.6.x", "1000.0.0", "１.6.0", "1..0"):
            self.assertIsNone(H.version_number(bad), bad)
        self.assertEqual(H.parse_utc("1970-01-01T00:00:00Z"), 0)
        self.assertEqual(H.parse_utc("2026-11-22T15:00:00Z"), 1_795_359_600)
        self.assertEqual(H.parse_utc("2028-02-29T23:59:59Z"), 1_835_481_599)
        self.assertEqual(H.parse_utc("0001-01-01T00:00:00Z"), -62_135_596_800)
        self.assertEqual(H.parse_utc("9999-12-31T23:59:59Z"), 253_402_300_799)
        self.assertIsNone(H.parse_utc("0000-01-01T00:00:00Z"))
        for bad in ("", "2026-11-22", "2026-11-22T15:00:00+00:00", "2026-11-22T15:00:00.000Z", "2026-13-01T00:00:00Z",
                    "2026-02-29T00:00:00Z", "2026-11-22T24:00:00Z", "2026-11-22T15:60:00Z", "2026-11-22T15:00:60Z",
                    "２026-11-22T15:00:00Z", "2026-11-22t15:00:00Z", "+026-11-22T15:00:00Z"):
            self.assertIsNone(H.parse_utc(bad), bad)

    def test_the_history_is_canonical(self):
        for name, data in HISTORY.items():
            H.canonical_envelope(data)

    def test_what_is_not_canonical(self):
        good = HISTORY["e2-s1"]
        text = good.decode()
        env = json.loads(text)
        changes = {
            "no line feed": good[:-1],
            "two line feeds": good + b"\n",
            "CRLF": good[:-1] + b"\r\n",
            "a byte order mark": b"\xef\xbb\xbf" + good,
            "a space": text.replace('"v":1,', '"v": 1,', 1).encode(),
            "member order": (J({"k": env["k"], "v": 1, "p": env["p"], "s": env["s"], "root": env["root"]}) + "\n").encode(),
            "an unknown member": (text[:-2] + ',"x":1}\n').encode(),
            "a null root": (J(dict(env, root=None)) + "\n").encode(),
            "k twice": text.replace('"k":2,', '"k":2,"k":2,', 1).encode(),
            "a float": text.replace('"v":1,', '"v":1.0,', 1).encode(),
            "an exponent": text.replace('"v":1,', '"v":1e0,', 1).encode(),
            "minus zero": (J(dict(env, v=1)).replace('"v":1', '"v":-0', 1) + "\n").encode(),
            "k 65536": text.replace('"k":2,', '"k":65536,', 1).encode(),
            "an escaped slash in p": (J(dict(env, p=env["p"].replace("받으세요.", "받으세요\\/"))) + "\n").encode(),
            "p with a space": (J(dict(env, p=env["p"].replace(",", ", ", 1))) + "\n").encode(),
            "p with an escaped Korean letter": text.replace("설", "\\uc124", 1).encode(),
            "p's members out of order": (J(dict(env, p=env["p"].replace('"v":1,"epoch":2', '"epoch":2,"v":1', 1)))
                                         + "\n").encode(),
            "move_to_play false": (J(dict(env, p=env["p"].replace('"move_to_play":true', '"move_to_play":false')))
                                   + "\n").encode(),
            "build as a number": (J(dict(env, p=env["p"].replace('"build":"262782100"', '"build":262782100')))
                                  + "\n").encode(),
            "a package without build": (J(dict(env, p=env["p"].replace(',"build":"262782100"', ""))) + "\n").encode(),
            "build beyond a 64-bit float": (J(dict(env, p=env["p"].replace('"build":"262782100"', '"build":1e400')))
                                            + "\n").encode(),
            "a declaration without revoked": (J(dict(env, root=dict(env["root"], p=env["root"]["p"].replace(
                '"revoked":[1],', "")))) + "\n").encode(),
            "a declaration without base": (J(dict(env, root=dict(env["root"], p=env["root"]["p"].split(',"base"')[0]
                                                                + "}"))) + "\n").encode(),
            "a declaration without v": (J(dict(env, root=dict(env["root"], p=env["root"]["p"].replace(
                '"v":1,', "", 1)))) + "\n").encode(),
            "a base entry out of order": (J(dict(env, root=dict(env["root"], p=env["root"]["p"].replace(
                '"min":"1.6.0","latest":"1.6.1"', '"latest":"1.6.1","min":"1.6.0"', 1)))) + "\n").encode(),
            "a negative revoked number": (J(dict(env, root=dict(env["root"], p=env["root"]["p"].replace(
                '"revoked":[1]', '"revoked":[-1]')))) + "\n").encode(),
            "s unpadded": text.replace(env["s"], env["s"].rstrip("="), 1).encode(),
            "not UTF-8": good.replace(b"\xec\x84\xa4", b"\xff", 1),
            "NaN": text.replace('"v":1,', '"v":NaN,', 1).encode(),
            "a list": b"[" + good[:-1] + b"]\n",
        }
        for why, data in changes.items():
            with self.assertRaises(H.NotCanonical, msg=why):
                H.canonical_envelope(data)
        with self.assertRaisesRegex(H.NotCanonical, 'a member the tool does not write: "x"'):
            H.canonical_envelope(changes["an unknown member"])
        # what the tool writes but the apps refuse is canonical: the apps' codes say why
        for name in ("envelope-v2", "policy-v2", "declaration-v2", "wrong-server", "min-above-latest",
                     "declaration-of-epoch-1", "contact-201-characters"):
            data = [c for c in CASES if c[0] == name][0][2]
            H.canonical_envelope(data)

    def test_field_rules(self):
        base = json.loads(P12)

        def refused(change):
            d = json.loads(json.dumps(base))
            change(d)
            text = J(d)
            try:
                H.parse_policy(text, H.to_dict(H.json_value(text, "p")), SERVER)
            except H.Refused as r:
                return r.code
            return None

        def w(member, value):
            return lambda d: d["platforms"]["windows"].__setitem__(member, value)

        self.assertIsNone(refused(lambda d: None))
        self.assertEqual(refused(lambda d: d.__setitem__("epoch", 0)), "Field:epoch")
        self.assertEqual(refused(lambda d: d.__setitem__("seq", 0)), "Field:seq")
        self.assertEqual(refused(lambda d: d.__setitem__("v", 2)), "Version")
        self.assertEqual(refused(lambda d: d.__setitem__("server", "rd.example.org")), "Server")
        self.assertEqual(refused(w("min", "1.6")), "Field:min")
        self.assertEqual(refused(w("latest", "1.5.9")), "Field:min")
        self.assertEqual(refused(w("grace_min", "1.6.0")), "Field:grace_min")
        self.assertEqual(refused(w("grace_min", "1.6")), "Field:grace_min")
        self.assertEqual(refused(w("grace_until", "2026-10-16")), "Field:grace_until")
        self.assertEqual(refused(w("os_min", "")), "Field:os_min")
        self.assertEqual(refused(w("os_min", "1" * 17)), "Field:os_min")
        self.assertEqual(refused(w("notes_ko", "가" * 2001)), "Field:notes_ko")
        self.assertIsNone(refused(w("notes_ko", "가" * 2000)))
        self.assertEqual(refused(w("source", "a" * 101)), "Field:source")
        self.assertEqual(refused(w("package", dict(WIN16, file="HJViewer-1.6.0-INSTALL.EXE"))), "Field:package.file")
        self.assertEqual(refused(w("package", dict(WIN16, file="a" * 101))), "Field:package.file")
        self.assertEqual(refused(w("package", dict(WIN16, size=200 * 1024 * 1024))), None)
        self.assertEqual(refused(w("package", dict(WIN16, sha256=WIN16["sha256"][:63]))), "Field:package.sha256")
        self.assertEqual(refused(w("auto_install_after", "2026-10-07T03:00:00+00:00")), "Field:auto_install_after")
        # the order of the rules: min before os_min before grace_min, windows before android
        both = lambda d: (w("os_min", "x")(d), w("min", "x")(d))
        self.assertEqual(refused(both), "Field:min")
        self.assertEqual(refused(lambda d: (d["platforms"]["android"].__setitem__("min", "x"),
                                            w("os_min", "x")(d))), "Field:os_min")


class Signatures(Common):
    def test_the_small_order_list_is_the_eight_points_and_two_more_encodings(self):
        points = set()
        for i in range(256):
            point = H.decode_point(hashlib.sha256(bytes([i])).digest())
            if point is not None:
                points.add(_encode(H._mul(L, point)))
        self.assertEqual(len(points), 8)
        for enc in points:
            self.assertTrue(H.small_order(enc))
        cleared = {enc[:31] + bytes([enc[31] & 0x7F]) for enc in points}
        p_and_p1 = {(P).to_bytes(32, "little"), (P + 1).to_bytes(32, "little")}
        self.assertEqual(cleared | p_and_p1, set(H.SMALL_ORDER))

    def test_weak_signatures_are_refused_and_openssl_alone_takes_one(self):
        message = H.POLICY_CONTEXT + b"\0" + P11.encode()
        pub = public(SEEDS[1])
        good = sign(SEEDS[1], message)
        identity = sign_identity_r(SEEDS[1], message)
        self.assertTrue(self.verifier.openssl(pub, message, identity))
        self.assertFalse(self.verifier.verify(pub, H.POLICY_CONTEXT, P11, identity))
        self.assertEqual(H.weak_signature(identity), "its R is a point of small order")
        self.assertTrue(self.verifier.verify(pub, H.POLICY_CONTEXT, P11, good))
        s_plus_l = good[:32] + (int.from_bytes(good[32:], "little") + L).to_bytes(32, "little")
        self.assertEqual(H.weak_signature(s_plus_l), "its S is not below the group order")
        self.assertFalse(self.verifier.verify(pub, H.POLICY_CONTEXT, P11, s_plus_l))
        # the same R with its sign bit set
        flipped = identity[:31] + bytes([identity[31] | 0x80]) + identity[32:]
        self.assertIsNotNone(H.weak_signature(flipped))
        # S = L exactly, and the largest S below it
        self.assertEqual(H.weak_signature(good[:32] + L.to_bytes(32, "little")), "its S is not below the group order")
        self.assertIsNone(H.weak_signature(good[:32] + (L - 1).to_bytes(32, "little")))
        self.verifier.self_test()

    def test_keys_the_apps_would_not_pin(self):
        self.assertIsNone(H.key_problem(public(SEEDS[1])))
        self.assertEqual(H.key_problem((1).to_bytes(32, "little")), "a weak key (small order)")
        self.assertEqual(H.key_problem((P + 5).to_bytes(32, "little")), "not an Ed25519 public key")
        torsion = None
        for i in range(256):
            point = H.decode_point(hashlib.sha256(bytes([i])).digest())
            if point is not None and not H._is_identity(H._mul(4, H._mul(L, point))):
                torsion = H._mul(L, point)
                break
        mixed = _encode(H._add(H._mul(7, H.BASE), torsion))
        self.assertEqual(H.key_problem(mixed), "outside the prime-order subgroup")


class Rules(Common):
    def test_covers_is_threshold_at_every_time(self):
        rows = covers_rows()
        self.assertEqual(len(rows), 256)
        for row in rows:
            self.assertEqual(H.covers(entry_of(row["new"]), entry_of(row["old"])), row["covers"], row)

    def test_relaxed_names_the_entries(self):
        B = {n: entry_of(e) for n, e in E12.items()}
        self.assertEqual(H.relaxed(B, B), set())
        self.assertEqual(H.relaxed(B, {n: e for n, e in B.items() if n != "ios"}), {"ios"})
        lower = dict(B, windows=entry_of(entry("1.5.0", "1.6.0")))
        self.assertEqual(H.relaxed(B, lower), {"windows"})
        play = dict(B, android_play=entry_of(entry("1.6.0", "1.6.0", T30, "1.5.0")))
        self.assertEqual(H.relaxed(B, play), {"android_play"})
        self.assertEqual(H.relaxed(B, dict(B, android_play=B["android"])), set())

    def test_every_case(self):
        for name, after, data, floor, expect, kept, then, binding, canonical, state in CASES:
            with self.subTest(name):
                build = self.build.with_floor(tuple(int(x) for x in floor.split(":")))
                st = self.take(after, build) if all(a in HISTORY for a in after) else None
                try:
                    env = H.read_envelope(data)
                except H.NotCanonical:
                    self.assertFalse(canonical)
                    continue
                except H.Refused as r:
                    self.assertEqual(r.code, expect)
                    continue
                self.assertTrue(canonical)
                st2, out = H.accept(build, st, env, self.verifier)
                got = out.code if isinstance(out, H.Refused) else out
                if binding == "tool":
                    self.assertIn(got, ("taken", "unchanged"))
                    continue
                self.assertEqual(got, expect)
                if isinstance(out, H.Refused):
                    if kept:
                        summary = H.summary(build, st2)
                        for key, value in kept.items():
                            self.assertEqual(summary[key], value, key)
                    else:
                        self.assertEqual(st2, st, "the state changed")
                    if then:
                        _, out2 = H.accept(build, st2, H.read_envelope(HISTORY[then]), self.verifier)
                        self.assertEqual(out2, "taken")
                elif state:
                    summary = H.summary(build, st2)
                    for key, value in state.items():
                        self.assertEqual(summary[key], value, key)

    def test_the_later_signed_part_wins_and_what_may_be_installed(self):
        # a newer build whose floor is in epoch 2 holds an epoch-1 policy: enforced (it is of the
        # declared epoch, 1), not a basis, nothing installed from it (spec 1-5, state 2)
        later = self.build.with_floor((2, 0))
        st = H.holding(later, H.read_envelope(HISTORY["e1-s3"]), self.verifier)
        summary = H.summary(later, st)
        self.assertEqual((summary["epoch"], summary["enforces"], summary["may_install"]), (2, "policy", False))
        # a held policy whose signer its own declaration revokes (only a crafted record holds one):
        # the base is enforced and nothing is installed
        st = H.holding(self.build, H.read_envelope(envelope(part(1, P21), R2)), self.verifier)
        self.assertEqual(st.held.k, 1)
        summary = H.summary(self.build, st)
        self.assertEqual((summary["enforces"], summary["may_install"]), ("base", False))
        # the same policy by key 2: installed from
        st = H.holding(self.build, H.read_envelope(HISTORY["e2-s1"]), self.verifier)
        self.assertEqual(H.summary(self.build, st)["may_install"], True)

    def test_load_keeps_what_the_build_verifies(self):
        def load(data, build=None):
            return H.load(build or self.build, H.read_envelope(data), self.verifier)

        # L4: a policy of epoch 2 or later comes with its own epoch's declaration, one of epoch 1
        # with none; the declaration stays. The reason sends the operator to runbook 8-5 (바)
        for data, why, decl in (
                (envelope(part(2, P22)), "a policy of epoch 2 without that epoch's declaration", None),
                (envelope(part(2, policy(3, 1, day(8), E22)), R2),
                 "a policy of epoch 3 without that epoch's declaration (it comes with epoch 2's)", 2),
                (envelope(part(2, P22), R3), "a policy of epoch 2 without that epoch's declaration (it comes with "
                                             "epoch 3's)", 3),
                (envelope(part(1, P13), R2), "a policy of epoch 1 with a declaration (of epoch 2)", 2)):
            st, missing = load(data)
            self.assertIsNone(st.held, why)
            self.assertEqual(None if st.decl is None else st.decl.epoch, decl, why)
            self.assertEqual(missing, (why, False))
            self.assertEqual(H.holding(self.build, H.read_envelope(data), self.verifier), st)
        st, missing = load(HISTORY["e3-s2"])
        self.assertEqual((st.decl.epoch, st.held.P.seq, missing), (3, 2, None))
        # a build that pins neither signing key 2 nor root key 101: the key file's reason (8-5 (마))
        other = H.Build(SERVER, {1: public(SEEDS[1]), 3: public(SEEDS[3])}, {102: public(SEEDS[102])}, (1, 0))
        st, missing = load(HISTORY["e3-s2"], other)
        self.assertIsNone(st.held)
        self.assertIsNone(st.decl)
        self.assertEqual(missing, ("it pins no root key 101, which signed the declaration", True))
        # root key 101 pinned, signing key 2 not: the declaration stays, its base is enforced
        half = H.Build(SERVER, {1: public(SEEDS[1])}, {101: public(SEEDS[101])}, (1, 0))
        st, missing = load(HISTORY["e3-s2"], half)
        self.assertIsNone(st.held)
        self.assertEqual(H.summary(half, st)["enforces"], "base")
        self.assertEqual(missing, ("it pins no key 2, which signed the policy", True))
        # what no key file mends: a signature that does not hold, a declaration broken, a policy
        # that breaks a rule
        st, missing = load(envelope(dict(part(2, P21), p=P21.replace("1.6.1", "1.6.2", 1)), R2))
        self.assertEqual((st.held, st.decl.epoch), (None, 2))
        self.assertEqual(missing, ("the policy's signature does not hold (strictly) for key 2", False))
        st, missing = load(envelope(part(2, P21), dict(R2, p=D2.replace('"revoked":[1]', '"revoked":[2]'))))
        self.assertEqual((st.held, st.decl), (None, None))
        self.assertTrue(missing[0].startswith("its declaration: RootSignature"), missing)
        self.assertFalse(missing[1])
        st, missing = load(envelope(part(1, policy(1, 1, day(1), E11, server="rd.example.org"))))
        self.assertIsNone(st.held)
        self.assertTrue(missing[0].startswith("its policy: Server"), missing)
        self.assertFalse(missing[1])

    def test_what_hjv_api_reads(self):
        # hjv-api's is_package_file (server/api/src/update.rs) for the two entries it reads
        for name, extension, ok in (
                ("HJViewer-1.6.1-windows-x86_64.exe", ".exe", True), ("HJViewer-1.6.1-android-arm64.apk", ".apk", True),
                ("HJViewer.EXE", ".exe", True), ("x.exe", ".exe", True), ("a" * 96 + ".exe", ".exe", True),
                ("a" * 97 + ".exe", ".exe", False), ("HJViewer-1.6.1-windows-x86_64.apk", ".exe", False),
                ("HJViewer-1.6.1-android-arm64.exe", ".apk", False), (".exe", ".exe", False), ("exe", ".exe", False),
                (".HJViewer.exe", ".exe", False), ("HJViewer..1.6.0.exe", ".exe", False), ("HJViewer..exe", ".exe", False),
                ("HJViewer-qs-1.exe", ".exe", False), ("HJViewer-INSTALL.exe", ".exe", False),
                ("HJViewer 1.exe", ".exe", False), ("HJViewer/1.exe", ".exe", False), ("HJViewer-한.exe", ".exe", False)):
            self.assertEqual(H.api_package_file(name, extension), ok, name)
        for platforms in (json.loads(P13)["platforms"], json.loads(P32)["platforms"],
                          {"windows": {"source": "c" * 64}}, {"ios": {"source": "c" * 65}},
                          {"ios": {"package": {"file": ".x.ipa"}}}, {"android_play": {"source": "c" * 100}}):
            self.assertIsNone(H.api_problem(platforms), platforms)
        for platforms, why in (
                ({"windows": {"source": "c" * 65}}, "the windows entry's source is not a tag name of at most 64"),
                ({"android": {"source": "client 1.6.1"}}, "the android entry's source"),
                ({"windows": {"package": {"file": "HJViewer-1.6.1-windows-x86_64.apk"}}},
                 'the windows entry\'s package file "HJViewer-1.6.1-windows-x86_64.apk" is not a name hjv-api hands out'),
                ({"android": {"package": {"file": "HJViewer-1.6.1-android-arm64.exe"}}}, "the android entry's package"),
                ({"windows": {"package": {"file": "HJViewer..1.6.1.exe"}}}, "no \"..\""),
                ({"android": {"package": {"file": ".HJViewer.apk"}}}, "no leading \".\"")):
            self.assertIn(why, H.api_problem(platforms) or "")
        # the package directory never hands out a file whose name starts with a dot (item 8)
        with self.assertRaisesRegex(H.Refused, "Package"):
            H.find_package([os.path.dirname(SCRIPT)], ".x.ipa", 1, "0" * 64)


class Command(unittest.TestCase):
    """The check as 허브-운영 runs it on the server (runbook 8-2)."""

    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="hjv-update-verify-test-")
        self.keys = self.file("update-keys.env", key_file())
        self.pkg = os.path.join(self.dir, "srv-pkg")
        os.makedirs(self.pkg)
        for name, data in PACKAGE_BYTES.items():
            self.file(os.path.join("srv-pkg", name), data)
        self.live = os.path.join(self.dir, "etc", "version-policy.json")
        os.makedirs(os.path.dirname(self.live))

    def tearDown(self):
        shutil.rmtree(self.dir)

    def file(self, name, data):
        path = os.path.join(self.dir, name)
        with open(path, "wb") as f:
            f.write(data.encode("utf-8") if isinstance(data, str) else data)
        return path

    def set_live(self, data):
        with open(self.live, "wb") as f:
            f.write(data)

    def check(self, data, *more, now="2026-10-09T12:00:00Z", relaxes=None, keys=None):
        new = self.file("new.json", data)
        args = ["--keys", keys or self.keys, "--live", self.live, "--server", SERVER, "--now", now,
                "--packages", self.pkg]
        if relaxes is not None:
            args += ["--relaxes", relaxes]
        return run_script(*args, *more, new)

    def refused(self, code, data, **kw):
        status, out, err = self.check(data, **kw)
        self.assertEqual(status, 1, out + err)
        self.assertIn("REFUSED %s" % code, out)
        return out

    def passes(self, data, **kw):
        status, out, err = self.check(data, **kw)
        self.assertEqual(status, 0, out + err)
        self.assertIn("OK", out)
        return out

    def test_the_first_install_and_then_each_policy(self):
        out = self.passes(HISTORY["e1-s1"])
        self.assertIn("the first install", out)
        self.assertIn("package of windows: %s" % os.path.join(self.pkg, WIN16["file"]), out)
        self.assertIn("file SHA-256: %s" % hashlib.sha256(HISTORY["e1-s1"]).hexdigest(), out)
        previous = None
        for name in ("e1-s1", "e1-s2", "e1-s3", "e2-s1", "e2-s2"):
            if previous:
                self.set_live(HISTORY[previous])
            self.passes(HISTORY[name])
            previous = name
        self.set_live(HISTORY["e2-s2"])
        # the relaxation of epoch 3, with the entries it relaxes named
        self.refused("LiveRelaxes", HISTORY["e3-s1-relax"])
        self.refused("LiveRelaxes", HISTORY["e3-s1-relax"], relaxes="windows,android")
        out = self.passes(HISTORY["e3-s1-relax"], relaxes="windows,android,android_play,ios")
        self.assertIn("relaxes {windows,android,android_play,ios}", out)
        self.set_live(HISTORY["e3-s1-relax"])
        self.passes(HISTORY["e3-s2"])
        # the same file again: nothing to install
        self.set_live(HISTORY["e3-s2"])
        self.assertIn("nothing to install", self.passes(HISTORY["e3-s2"]))

    def test_a_declaration_from_a_stale_prev(self):
        # live: e1-s3 (windows 1.6.1 latest); a declaration made from e1-s2 (latest 1.6.0)
        self.set_live(HISTORY["e1-s3"])
        stale = envelope(part(2, policy(2, 1, day(6), E12)), root_part(101, declaration(2, [], day(6), requirements(E12))))
        out = self.refused("LiveRelaxes", stale)
        self.assertIn("relaxes {windows,android,android_play}", out)
        # even when the operator names what the stale file relaxes, other items hold: here it passes
        self.passes(stale, relaxes="windows,android,android_play")

    def test_relaxes_without_a_new_declaration(self):
        self.set_live(HISTORY["e1-s1"])
        self.refused("LiveRelaxes", HISTORY["e1-s2"], relaxes="ios")
        os.unlink(self.live)
        self.refused("LiveRelaxes", HISTORY["e1-s1"], relaxes="ios")

    def test_a_live_file_this_check_cannot_read(self):
        # signed by a key the key file does not pin: the key file was replaced too early (8-5 (마))
        self.set_live(envelope(part(3, P11)))
        out = self.refused("LiveUnreadable", HISTORY["e1-s2"])
        self.assertIn("the key file does not verify the live file (it pins no key 3, which signed the policy)", out)
        self.assertNotIn("with --live", out)
        # not canonical: a file that did not pass this check. It stays where it is, and the new file
        # is checked against the last file installed (runbook 8-5 (바)); no old file is put back
        self.set_live(HISTORY["e1-s1"].replace(b'"v":1,', b'"v": 1,', 1))
        out = self.refused("LiveUnreadable", HISTORY["e1-s2"])
        self.assertIn("the live file is not one this check reads", out)
        self.assertIn("with --live", out)
        self.assertNotIn("put back", out)
        last = self.file("last-installed.json", HISTORY["e1-s1"])
        status, out, err = run_script("--keys", self.keys, "--live", last, "--server", SERVER, "--now",
                                      "2026-10-09T12:00:00Z", "--packages", self.pkg,
                                      self.file("new.json", HISTORY["e1-s2"]))
        self.assertEqual(status, 0, out + err)
        self.assertIn("a device that holds the live policy takes it", out)
        # what the apps hold under no key file (L4, a signature broken by hand): 8-5 (바) too
        for live, new, why in (
                (envelope(part(2, P22)), HISTORY["e2-s2"], "a policy of epoch 2 without that epoch's declaration"),
                (envelope(part(1, P13), R2), HISTORY["e2-s1"], "a policy of epoch 1 with a declaration (of epoch 2)"),
                (envelope(part(2, P22), R3), HISTORY["e3-s2"], "(it comes with epoch 3's)"),
                (envelope(dict(part(1, P12), p=P12.replace("1.5.0", "1.4.0", 1))), HISTORY["e1-s3"],
                 "the policy's signature does not hold (strictly) for key 1")):
            self.set_live(live)
            out = self.refused("LiveUnreadable", new)
            self.assertIn("the live file is not one this check reads (", out)
            self.assertIn(why, out)
            self.assertIn("with --live", out)
        # an epoch-2 policy whose declaration a root key signed that the key file does not pin
        self.set_live(envelope(part(2, P21), root_part(103, D2)))
        out = self.refused("LiveUnreadable", HISTORY["e2-s2"])
        self.assertIn("the key file does not verify the live file (it pins no root key 103", out)
        # a key file of the next build that pins key 2 and 3 only reads key 2's live files
        keys = self.file("next-keys.env", key_file(signing=(2, 3)))
        self.set_live(HISTORY["e1-s2"])
        self.assertIn("it pins no key 1, which signed the policy",
                      self.refused("LiveUnreadable", HISTORY["e1-s3"], keys=keys))
        self.set_live(HISTORY["e1-s3"])
        self.passes(envelope(part(2, policy(1, 4, day(6), E22))), keys=keys)

    def test_a_stolen_signing_key(self):
        # runbook 8-5 (라): after the user's e1-s3, a policy signed with the stolen key 1 went live
        # (min 99.0.0 everywhere). The user's recovery is the declaration of epoch 2 that revokes
        # key 1, made from the archive's last own file (e2-s1: its base is e1-s3's requirements)
        self.set_live(HISTORY["forged-minimum"])
        out = self.refused("LiveRelaxes", HISTORY["e2-s1"])
        self.assertIn("relaxes {windows,android,android_play,ios}; --relaxes names {}", out)
        self.assertIn("a signing key may be stolen (runbook 8-5)", out)
        # and the user's own file is what a device that holds the thief's refuses
        self.assertIn("a signing key may be stolen", self.refused("Rollback", HISTORY["e1-s3"]))
        # step 2: the user's last own file is checked on its own, against itself, then put back
        own = self.file("own.json", HISTORY["e1-s3"])
        status, out, err = run_script("--keys", self.keys, "--live", own, "--server", SERVER, "--now",
                                      "2026-10-09T12:00:00Z", "--packages", self.pkg, own)
        self.assertEqual(status, 0, out + err)
        self.assertIn("a fresh install of the newest build (floor 1:0) takes it", out)
        self.assertTrue(out.rstrip().endswith("OK, nothing to install: it is the live file"), out)
        self.assertTrue(os.path.exists(own))
        # what is not a file of the apps does not pass that way either
        broken = self.file("broken.json", envelope(dict(part(2, P13), p=P13.replace("1.6.1", "1.6.2", 1))))
        status, out, _ = run_script("--keys", self.keys, "--live", broken, "--server", SERVER, "--now",
                                    "2026-10-09T12:00:00Z", "--packages", self.pkg, broken)
        self.assertEqual(status, 1, out)
        self.assertIn("REFUSED BadSignature", out)
        self.assertTrue(os.path.exists(broken))
        # step 3: the declaration against the file now live, with no --relaxes
        self.set_live(HISTORY["e1-s3"])
        self.assertIn("relaxes {} against the live policy", self.passes(HISTORY["e2-s1"]))

    def test_what_a_device_that_holds_the_live_file_refuses(self):
        self.set_live(HISTORY["e1-s2"])
        self.refused("Rollback", HISTORY["e1-s1"])
        out = self.refused("Fork", envelope(part(1, policy(1, 2, day(2), E12, contact="다른 문구"))))
        self.assertIn("a signing key may be stolen", out)
        self.refused("Relaxation", [c for c in CASES if c[0] == "relax-grace-later"][0][2])
        self.refused("Issued", envelope(part(1, policy(1, 3, day(1), E12))))

    def test_what_a_fresh_install_refuses(self):
        out = self.refused("BadSignature", [c for c in CASES if c[0] == "tampered-signature"][0][2])
        self.assertIn("a fresh install of the newest build (floor 1:0)", out)
        self.refused("Epoch", envelope(part(2, P22)))
        self.refused("Jump", envelope(part(1, policy(1, 1001, day(3), E11))))
        # the newest build's floor
        keys = self.file("floor-keys.env", key_file(floor="1:2"))
        self.refused("Rollback", HISTORY["e1-s1"], keys=keys)
        self.passes(HISTORY["e1-s2"], keys=keys)

    def test_canonical_and_weak_signatures(self):
        out = self.refused("Canonical", HISTORY["e1-s1"].replace(b'"v":1,', b'"v": 1,', 1))
        self.assertIn("as hjv-release writes them", out)
        out = self.refused("WeakSignature", envelope(part(1, P11, signature=sign_identity_r)))
        self.assertIn("OpenSSL alone would take it", out)
        good = sign(SEEDS[1], H.POLICY_CONTEXT + b"\0" + P11.encode())
        s_plus_l = good[:32] + (int.from_bytes(good[32:], "little") + L).to_bytes(32, "little")
        self.refused("WeakSignature", envelope(part(1, P11, signature=s_plus_l)))
        identity_root = root_part(101, D2, signature=sign_identity_r)
        self.refused("WeakSignature", envelope(part(2, P21), identity_root))

    def test_the_clock(self):
        self.refused("Clock", HISTORY["e1-s1"], now="2026-10-01T02:54:59Z")
        self.passes(HISTORY["e1-s1"], now="2026-10-01T02:55:00Z")
        # a declaration and its policy dated ahead (a fresh install refuses a policy issued before
        # its declaration, C7, so the declaration is never ahead alone)
        D = declaration(2, [1], day(6, 10), requirements(E13))
        ahead = envelope(part(2, policy(2, 1, day(6, 10), E13)), root_part(101, D))
        self.assertIn("the policy is issued 2026-10-06T10:00:00Z", self.refused("Clock", ahead, now=day(6, 9)))
        self.passes(ahead, now="2026-10-06T09:55:00Z")

    def test_a_new_declaration_against_the_live_one(self):
        self.set_live(HISTORY["e2-s2"])
        # the epoch is not above the live policy's: a fork of epoch 2, which the device refuses first
        self.refused("RootFork", envelope(part(2, policy(2, 3, day(8), E22)), root_part(101, declaration(
            2, [1], day(8), requirements(E22)))))
        # drops a revocation
        drop = declaration(3, [], day(8), requirements(E22))
        self.refused("DeclarationRevoked", envelope(part(2, policy(3, 1, day(8), E22)), root_part(101, drop)))
        # names a root number another build may pin
        D = declaration(3, [1, 103], day(8), requirements(E22))
        out = self.refused("DeclarationRevoked", envelope(part(2, policy(3, 1, day(8), E22)), root_part(101, D)))
        self.assertIn("101-199", out)
        # revokes key 2 too: the next build pins 2 and 3, and key 3 signs
        out = self.passes(envelope(part(3, policy(3, 1, day(8), E22)), root_part(101, declaration(
            3, [1, 2], day(8), requirements(E22)))), keys=self.file("k.env", key_file(signing=(2, 3))))
        self.assertIn("relaxes {}", out)
        # issued before the live policy (e2-s2, 7 October): every device takes it, its basis reset
        # by the declaration, so only this item sees it
        early = declaration(3, [1], day(6, 4), requirements(E22))
        out = self.refused("LiveIssued", envelope(part(2, policy(3, 1, day(6, 4), E22)), root_part(101, early)))
        self.assertIn("the new policy is issued 2026-10-06T04:00:00Z, before the live one (2026-10-07T03:00:00Z)", out)
        # --relaxes names exactly what the declaration relaxes, no more
        same = envelope(part(2, policy(3, 1, day(8), E22)), root_part(101, declaration(3, [1], day(8), requirements(E22))))
        self.assertIn("relaxes {}; --relaxes names {ios}", self.refused("LiveRelaxes", same, relaxes="ios"))
        self.passes(same)
        lower = dict(E22, windows=entry("1.5.0", "1.6.1", source="client-1.6.1", package=WIN161))
        windows = envelope(part(2, policy(3, 1, day(8), lower)), root_part(101, declaration(
            3, [1], day(8), requirements(lower))))
        self.assertIn("relaxes {windows}; --relaxes names {windows,ios}",
                      self.refused("LiveRelaxes", windows, relaxes="windows,ios"))
        self.refused("LiveRelaxes", windows)
        self.assertIn("relaxes {windows} against the live policy", self.passes(windows, relaxes="windows"))

    def test_what_hjv_api_reads_as_no_policy(self):
        def first(**change):
            ents = dict(E11)
            for name, members in change.items():
                ents[name] = dict(E11[name], **members)
            return envelope(part(1, policy(1, 1, day(1), ents)))

        out = self.refused("HjvApi", first(windows={"source": "c" * 65}))
        self.assertIn("the windows entry's source is not a tag name of at most 64", out)
        self.assertIn("door 1, door 2 and the short links would answer 404", out)
        self.assertIn("hjv-api reads its windows and android entries", self.passes(first(windows={"source": "c" * 64})))
        for name in ("HJViewer-1.6.0-windows-x86_64.apk", "HJViewer..1.6.0.exe", ".HJViewer-1.6.0-windows-x86_64.exe",
                     ".exe"):
            out = self.refused("HjvApi", first(windows={"package": dict(WIN16, file=name)}))
            self.assertIn("the windows entry's package file %s" % json.dumps(name), out)
        out = self.refused("HjvApi", first(android={"package": dict(APK16, file="HJViewer-1.6.0-android-arm64.exe")}))
        self.assertIn("the android entry's package file", out)
        # hjv-api reads no other entry; the package directory hands out no name with a leading dot
        out = self.refused("Package", first(ios={"package": dict(WIN16, file=".HJViewer-1.6.0-ios.ipa")}))
        self.assertIn("a package name that starts with a dot", out)
        # hjv-api compares the extension in lower case
        upper = dict(WIN16, file="HJViewer-1.6.0-windows-x86_64.EXE")
        self.file(os.path.join("srv-pkg", upper["file"]), PACKAGE_BYTES[WIN16["file"]])
        self.passes(first(windows={"package": upper}))

    def test_the_packages(self):
        self.set_live(HISTORY["e1-s2"])
        os.unlink(os.path.join(self.pkg, WIN161["file"]))
        out = self.refused("Package", HISTORY["e1-s3"])
        self.assertIn(WIN161["file"], out)
        self.file(os.path.join("srv-pkg", WIN161["file"]), b"W" * 4999)
        self.assertIn("4999 bytes", self.refused("Package", HISTORY["e1-s3"]))
        self.file(os.path.join("srv-pkg", WIN161["file"]), b"V" * 5000)
        self.assertIn("another SHA-256", self.refused("Package", HISTORY["e1-s3"]))
        os.unlink(os.path.join(self.pkg, WIN161["file"]))
        os.symlink(os.path.join(self.dir, "elsewhere"), os.path.join(self.pkg, WIN161["file"]))
        self.file("elsewhere", PACKAGE_BYTES[WIN161["file"]])
        self.assertIn("not a plain file", self.refused("Package", HISTORY["e1-s3"]))
        os.unlink(os.path.join(self.pkg, WIN161["file"]))
        # the default places: the pkg/ beside the new file
        incoming = os.path.join(self.dir, "incoming", "1.6.1")
        os.makedirs(os.path.join(incoming, "pkg"))
        for name, data in PACKAGE_BYTES.items():
            with open(os.path.join(incoming, "pkg", name), "wb") as f:
                f.write(data)
        new = os.path.join(incoming, "version-policy.json")
        with open(new, "wb") as f:
            f.write(HISTORY["e1-s3"])
        status, out, err = run_script("--keys", self.keys, "--live", self.live, "--server", SERVER, "--now",
                                      "2026-10-09T12:00:00Z", new)
        self.assertEqual(status, 0, out + err)
        self.assertIn(os.path.join(incoming, "pkg", WIN161["file"]), out)

    def test_a_refused_file_beside_the_live_one_is_removed(self):
        self.set_live(HISTORY["e1-s2"])
        beside = self.live + ".new"
        with open(beside, "wb") as f:
            f.write(HISTORY["e1-s1"])
        status, out, _ = run_script("--keys", self.keys, "--live", self.live, "--server", SERVER, "--now",
                                    "2026-10-09T12:00:00Z", "--packages", self.pkg, beside)
        self.assertEqual(status, 1)
        self.assertIn("removed", out)
        self.assertFalse(os.path.exists(beside))
        self.assertTrue(os.path.exists(self.live))
        # the live file itself is never removed
        status, out, _ = run_script("--keys", self.keys, "--live", self.live, "--server", SERVER, "--now",
                                    "2026-10-09T12:00:00Z", "--packages", self.pkg, self.live)
        self.assertEqual(status, 0, out)
        self.assertTrue(os.path.exists(self.live))
        # a refused file elsewhere stays: in incoming/, or beside the live one while --live names
        # another file (runbook 8-5 (바) removes that .new by hand)
        elsewhere = self.file("elsewhere.json", HISTORY["e1-s1"])
        last = self.file("last-installed.json", HISTORY["e1-s2"])
        with open(beside, "wb") as f:
            f.write(HISTORY["e1-s1"])
        for live, new in ((self.live, elsewhere), (last, beside)):
            status, out, _ = run_script("--keys", self.keys, "--live", live, "--server", SERVER, "--now",
                                        "2026-10-09T12:00:00Z", "--packages", self.pkg, new)
            self.assertEqual(status, 1, out)
            self.assertIn("REFUSED Rollback", out)
            self.assertNotIn("removed", out)
            self.assertTrue(os.path.exists(new), new)

    def test_what_stops_the_check_itself(self):
        for name, text in (("no keys", "HJV_UPDATE_PUBKEY=\nHJV_UPDATE_ROOT_PUBKEY=\nHJV_POLICY_FLOOR=1:0\n"),
                           ("a weak key", key_file().replace(b64(public(SEEDS[1])), b64((1).to_bytes(32, "little")))),
                           ("CRLF", key_file().replace("\n", "\r\n")),
                           ("a byte order mark", "\ufeff" + key_file()),
                           ("a line twice", key_file() + "HJV_POLICY_FLOOR=1:0\n"),
                           ("an export", "export " + key_file()),
                           ("the same key twice", key_file(signing=(1, 2), roots=(101, 102)).replace(
                               b64(public(SEEDS[2])), b64(public(SEEDS[1])))),
                           ("three signing keys", key_file(signing=(1, 2, 3)))):
            keys = self.file("bad.env", text)
            status, out, err = self.check(HISTORY["e1-s1"], keys=keys)
            self.assertEqual(status, 2, name + out + err)
            self.assertIn("cannot check", err, name)
        status, _, err = run_script("--keys", self.keys, "--live", self.live, "--now", "2026-10-09T12:00:00Z",
                                    "--packages", self.pkg, self.file("n.json", HISTORY["e1-s1"]))
        if not os.path.exists(H.DEFAULT_ENV):
            self.assertEqual(status, 2)
            self.assertIn("no server name", err)
        status, _, err = run_script("--relaxes", "macos", self.file("n.json", HISTORY["e1-s1"]))
        self.assertEqual(status, 2)
        status, _, _ = run_script()
        self.assertEqual(status, 2)
        self.assertEqual(run_script("--help")[0], 0)
        status, out, _ = run_script("--self-test")
        self.assertEqual(status, 0)
        self.assertIn("verify Ed25519 as the apps need", out)
        self.assertEqual(run_script("--self-test", "x.json")[0], 2)


class Vectors(unittest.TestCase):
    """--vectors, the shared vectors' runner, on the cases above written as the manifest of spec 7-4."""

    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="hjv-update-verify-vectors-")

    def tearDown(self):
        shutil.rmtree(self.dir)

    def run_vectors(self):
        return run_script("--vectors", self.dir)

    def test_the_cases_agree(self):
        write_vectors(self.dir)
        status, out, err = self.run_vectors()
        self.assertEqual(status, 0, out + err)
        self.assertIn("the vectors agree", out)
        self.assertIn("covers.json: 256 pairs, 0 disagree", out)

    def test_a_wrong_expectation_fails(self):
        manifest = write_vectors(self.dir)
        for change in (
                lambda c: c["name"] == "seq-lower" and c.update(reason="Fork"),
                lambda c: c["name"] == "e1-s2-again" and c.update(outcome="taken"),
                lambda c: c["name"] == "revoked-key-with-declaration" and c["state_after_refusal"].update(epoch=1),
                lambda c: c["name"] == "fresh-install-e2-s2" and c["state_after"].update(may_install=False),
                lambda c: c["name"] == "unknown-field" and c.update(canonical=True),
                lambda c: c["name"] == "e1-s2" and c.update(canonical=False),
                lambda c: c["name"] == "text-with-url" and c.update(app_expect="reject"),
                lambda c: c["name"] == "good-declaration-bad-policy" and c.update(then_accept=c["after"][0]),
                lambda c: c["name"] == "good-declaration-bad-policy" and c.update(then_outcome="unchanged"),
                lambda c: c["name"] == "e1-s1" and c.pop("canonical"),
                lambda c: c["name"] == "seq-lower" and c.update(state_after_refusal={"epoch": 2})):
            rows = json.loads(json.dumps(manifest))
            for c in rows["cases"]:
                change(c)
            with open(os.path.join(self.dir, "manifest.json"), "w", encoding="utf-8") as f:
                json.dump(rows, f)
            status, out, _ = self.run_vectors()
            self.assertEqual(status, 1, out)
            self.assertIn("FAIL", out)
        # a case whose file is missing fails; a manifest that is none stops the run
        rows = json.loads(json.dumps(manifest))
        rows["cases"][0]["file"] = "envelopes/missing.json"
        with open(os.path.join(self.dir, "manifest.json"), "w", encoding="utf-8") as f:
            json.dump(rows, f)
        status, out, _ = self.run_vectors()
        self.assertEqual(status, 1, out)
        self.assertIn("FAIL e1-s1", out)
        with open(os.path.join(self.dir, "manifest.json"), "w", encoding="utf-8") as f:
            f.write("[]")
        status, _, err = self.run_vectors()
        self.assertEqual(status, 2)
        self.assertIn("not a manifest", err)
        with open(os.path.join(self.dir, "manifest.json"), "w", encoding="utf-8") as f:
            json.dump(manifest, f)
        rows = covers_rows()
        rows[5]["covers"] = not rows[5]["covers"]
        with open(os.path.join(self.dir, "covers.json"), "w", encoding="utf-8") as f:
            json.dump(rows, f)
        status, out, _ = self.run_vectors()
        self.assertEqual(status, 1, out)
        self.assertIn("covers.json: 256 pairs, 1 disagree", out)


if __name__ == "__main__":
    unittest.main()

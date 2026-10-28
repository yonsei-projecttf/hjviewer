# SPDX-License-Identifier: AGPL-3.0-only
"""Offline tests of scripts/release-info.py and of the client-build.yml steps of the
minimum-version work that are not about keys: the release record of every platform job with the
Android manifest check, the Android job's APK signing, the iOS export method, the list of Dart tests
and the Windows smoke tests (scripts/ci.sh client):

    python3 -I -B -m unittest discover -s scripts -p 'test_release_info.py'

More tests run when asked for:

    HJV_RELEASE_BIN=<the built tools/hjv-release>      the release tool takes what the script
                                                       writes, with a `gh` that replays answers
    HJV_RELEASE_SAMPLES=<a directory of artifacts>     the real APKs and IPAs of a download of a
                                                       run (such as build/release-1.9.1) are read,
                                                       the APKs' manifests too, in place of the
                                                       samples in scripts/testdata (SAMPLES)
    HJV_PATCHED_FLUTTER=<the patched tree's flutter>   the Dart tests that the Dart tests job
                                                       requires are all in that tree
"""
import ast
import contextlib
import fnmatch
import hashlib
import importlib.util
import io
import json
import os
import plistlib
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import time
import unittest
import zipfile
from unittest import mock

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
SCRIPT = os.path.join(HERE, "release-info.py")
WORKFLOW = os.path.join(ROOT, ".github", "workflows", "client-build.yml")
# Real packages of a release build, as small as the reader allows: the AndroidManifest.xml of
# 1.9.1's signed APK and the Info.plist of its IPA (build b389ea2), byte for byte, each alone in its
# archive, stored (aapt2 writes the manifest's strings in UTF-16, and they were checked decoded
# against the leak list when they were taken, 2026-10-27)
SAMPLES = os.path.join(HERE, "testdata", "release-samples")
NOTICES = ("LICENSE.txt", "NOTICE.txt", "SOURCE.txt", "THIRD_PARTY_NOTICES.txt")
BUILD_REPO = "yonsei-projecttf/hjviewer-private"
# what GitHub gives a step of a workflow that the build repository's build.yml called
CALLED = {
    "GITHUB_RUN_ID": "4242",
    "GITHUB_REPOSITORY": BUILD_REPO,
    "GITHUB_WORKFLOW_REF": BUILD_REPO + "/.github/workflows/build.yml@refs/heads/main",
}
GITHUB_VARS = ("GITHUB_RUN_ID", "GITHUB_REPOSITORY", "GITHUB_WORKFLOW_REF", "GITHUB_STEP_SUMMARY",
               "GITHUB_ACTIONS", "GITHUB_WORKSPACE", "CHECK_ONLY")

_spec = importlib.util.spec_from_file_location("release_info", SCRIPT)
ri = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(ri)


def sha256(data):
    return hashlib.sha256(data).hexdigest()


# the android: attributes the fixtures write, by their resource IDs (as aapt2 puts them first)
ATTRIBUTE_IDS = {"versionCode": 0x0101021B, "versionName": 0x0101021C, "compileSdkVersion": 0x01010572,
                 "name": 0x01010003, "permission": 0x01010006, "foregroundServiceType": 0x01010599}
# What an Android build of 1.6.0 declares, cut down: the in-app update's permission and provider,
# and nothing of the controlled side. (depth, element, {android attribute: value}) in document
# order below <manifest>.
APP = "com.carriez.flutter_hbb."
MANIFEST_1_6 = (
    (1, "uses-permission", {"name": "android.permission.INTERNET"}),
    (1, "uses-permission", {"name": "android.permission.REQUEST_INSTALL_PACKAGES"}),
    (1, "uses-permission", {"name": "android.permission.RECORD_AUDIO"}),
    (1, "uses-permission", {"name": "android.permission.WAKE_LOCK"}),
    (1, "application", {"name": APP + "MainApplication"}),
    (2, "activity", {"name": APP + "MainActivity"}),
    (2, "provider", {"name": APP + "HjvUpdateFileProvider"}),
    (3, "meta-data", {"name": "android.support.FILE_PROVIDER_PATHS"}),
    (2, "provider", {"name": "androidx.startup.InitializationProvider"}),
)
# and what 1.5.1 declared of the controlled side, without the in-app update
MANIFEST_1_5 = (
    (1, "uses-permission", {"name": "android.permission.INTERNET"}),
    (1, "uses-permission", {"name": "android.permission.FOREGROUND_SERVICE"}),
    (1, "uses-permission", {"name": "android.permission.FOREGROUND_SERVICE_SPECIAL_USE"}),
    (1, "uses-permission", {"name": "android.permission.SYSTEM_ALERT_WINDOW"}),
    (1, "uses-permission", {"name": "android.permission.RECEIVE_BOOT_COMPLETED"}),
    (1, "uses-permission", {"name": "android.permission.REQUEST_IGNORE_BATTERY_OPTIMIZATIONS"}),
    (1, "application", {"name": APP + "MainApplication"}),
    (2, "receiver", {"name": APP + "BootReceiver"}),
    (2, "service", {"name": APP + "InputService", "permission": "android.permission.BIND_ACCESSIBILITY_SERVICE"}),
    (3, "meta-data", {"name": "android.accessibilityservice"}),
    (2, "activity", {"name": APP + "MainActivity"}),
    (2, "service", {"name": APP + "MainService", "foregroundServiceType": 0x400000A0}),
    (2, "service", {"name": APP + "FloatingWindowService"}),
)


def binary_manifest(code, name, utf8=False, children=()):
    """A binary AndroidManifest.xml as aapt2 writes one: its <manifest> element, and <children>
    below it ((depth, element, {android attribute: value}), a str value a string, an int one a
    number)."""
    filler = "x" * 300  # a string whose length takes two length units
    android = "http://schemas.android.com/apk/res/android"
    strings = [*ATTRIBUTE_IDS, "android", android, "package", "manifest", filler, name, "kr.ac.yonsei.hjviewer"]

    def index(s):
        if s not in strings:
            strings.append(s)
        return strings.index(s)

    none = 0xFFFFFFFF
    node = struct.pack("<II", 1, none)  # line number, comment

    def chunk(kind, header, body):
        return struct.pack("<HHI", kind, 8 + len(header), 8 + len(header) + len(body)) + header + body

    def start(element, attributes):  # attributes: (namespace, name, raw value, type, data)
        body = struct.pack("<IIHHHHHH", none, index(element), 20, 20, len(attributes), 0, 0, 0)
        for ns, attr, raw, kind, value in attributes:
            body += struct.pack("<IIIHBBI", ns, attr, raw, 8, 0, kind, value)
        return chunk(0x0102, node, body)

    def end(element):
        return chunk(0x0103, node, struct.pack("<II", none, index(element)))

    def typed(attribute, value):
        if isinstance(value, str):
            return (index(android), index(attribute), index(value), 0x03, index(value))
        return (index(android), index(attribute), none, 0x10, value)

    elements = start("manifest", [
        typed("compileSdkVersion", 36),
        typed("versionCode", code),
        typed("versionName", name),
        (none, index("package"), index("kr.ac.yonsei.hjviewer"), 0x03, index("kr.ac.yonsei.hjviewer")),
    ])
    open_ = ["manifest"]
    for depth, element, attributes in children:
        while len(open_) > depth:
            elements += end(open_.pop())
        elements += start(element, [typed(a, v) for a, v in attributes.items()])
        open_.append(element)
    while open_:
        elements += end(open_.pop())

    def length8(n):
        return bytes([n]) if n < 0x80 else bytes([0x80 | n >> 8, n & 0xFF])

    data, offsets = b"", []
    for s in strings:
        offsets.append(len(data))
        if utf8:
            raw = s.encode("utf-8")
            data += length8(len(s)) + length8(len(raw)) + raw + b"\0"
        else:
            data += struct.pack("<H", len(s)) + s.encode("utf-16-le") + b"\0\0"
    data += b"\0" * (-len(data) % 4)
    header = struct.pack("<IIIII", len(strings), 0, 0x100 if utf8 else 0, 28 + 4 * len(strings), 0)
    pool = chunk(0x0001, header, struct.pack("<%dI" % len(offsets), *offsets) + data)
    ids = list(ATTRIBUTE_IDS.values())  # of the first strings, the attribute names
    resource_map = chunk(0x0180, b"", struct.pack("<%dI" % len(ids), *ids))
    namespace = chunk(0x0100, node, struct.pack("<II", index("android"), index(android)))
    rest = pool + resource_map + namespace + elements
    return struct.pack("<HHI", 0x0003, 8, 8 + len(rest)) + rest


def apk(code=262872251, name="1.6.0", utf8=False, children=MANIFEST_1_6):
    out = io.BytesIO()
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("AndroidManifest.xml", binary_manifest(code, name, utf8, children))
        z.writestr("classes.dex", b"dex\n035\0")
    return out.getvalue()


def ipa(build="262870101", version="1.6.0", apps=("Runner",), fmt=plistlib.FMT_BINARY):
    out = io.BytesIO()
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        for app in apps:
            info = {"CFBundleIdentifier": "kr.ac.yonsei.hjviewer", "CFBundleShortVersionString": version,
                    "CFBundleVersion": build}
            z.writestr(f"Payload/{app}.app/Info.plist", plistlib.dumps(info, fmt=fmt))
            z.writestr(f"Payload/{app}.app/Frameworks/App.framework/Info.plist", plistlib.dumps({"CFBundleVersion": "1"}))
    return out.getvalue()


def git(tree, *args):
    env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": tree, "GIT_CONFIG_NOSYSTEM": "1"}
    done = subprocess.run(["git", "-C", tree, "-c", "user.name=test", "-c", "user.email=test@example.com", *args],
                          env=env, capture_output=True, text=True, check=True)
    return done.stdout.strip()


class Tree:
    """A checkout as a platform job has it: this repository's script and client/version.env at
    one commit, and the directory the job uploads, <tree>/out, with the notice files in it."""

    def __init__(self, test, version="1.6.0"):
        self.root = tempfile.mkdtemp()
        test.addCleanup(shutil.rmtree, self.root)
        os.makedirs(os.path.join(self.root, "scripts"))
        os.makedirs(os.path.join(self.root, "client"))
        shutil.copy(SCRIPT, os.path.join(self.root, "scripts", "release-info.py"))
        with open(os.path.join(self.root, "client", "version.env"), "w") as f:
            f.write(f"# the app's version\nHJV_VERSION={version}\n")
        git(self.root, "init", "-q")
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "-m", "the reviewed commit")
        self.commit = git(self.root, "rev-parse", "HEAD")
        self.out = os.path.join(self.root, "out")
        os.makedirs(self.out)
        for name in NOTICES:
            self.put(name, f"{name} of this build\n".encode())

    def put(self, name, data, directory=None):
        path = os.path.join(directory or self.out, name)
        with open(path, "wb") as f:
            f.write(data)
        return path

    def read(self, name):
        with open(os.path.join(self.out, name), "rb") as f:
            return f.read()

    def has(self, name):
        return os.path.lexists(os.path.join(self.out, name))

    def run(self, *args, env=None, directory=None):
        """release-info.py's main() in this process, as a step of a called run unless `env`
        says otherwise: (exit code, output, summary)."""
        summary = os.path.join(self.root, "summary.md")
        clean = {k: v for k, v in os.environ.items() if k not in GITHUB_VARS}
        full = {**clean, **CALLED, "GITHUB_STEP_SUMMARY": summary, **(env or {})}
        full = {k: v for k, v in full.items() if v is not None}
        out = io.StringIO()
        with mock.patch.dict(os.environ, full, clear=True), mock.patch.object(ri, "ROOT", self.root), \
                contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
            try:
                code = ri.main(["--dir", directory or self.out, *args])
            except SystemExit as e:  # argparse
                code = e.code
        text = ""
        if os.path.exists(summary):
            with open(summary) as f:
                text = f.read()
            os.remove(summary)
        return code, out.getvalue(), text


class RecordTest(unittest.TestCase):
    """release.json and SHA256SUMS."""

    def check_contract(self, text, tree):
        """What tools/hjv-release reads (ReleaseInfo::parse in its src/release.rs), here in
        Python: the form the two sides agreed on."""
        self.assertTrue(text.endswith("\n") and "\r" not in text)
        record = json.loads(text)
        self.assertEqual(list(record), ["platform", "version", "build", "file", "size", "sha256", "source_sha",
                                        "run_id", "workflow"])
        self.assertIn(record["platform"], ("windows", "android", "ios"))
        self.assertRegex(record["version"], r"^(0|[1-9]\d{0,2})\.(0|[1-9]\d{0,2})\.(0|[1-9]\d?)$")
        self.assertRegex(str(record["build"]), r"^[0-9.]{1,32}$")
        self.assertRegex(record["file"], r"^[A-Za-z0-9_-][A-Za-z0-9._-]{0,199}$")
        self.assertIs(type(record["size"]), int)
        self.assertRegex(record["sha256"], r"^[0-9a-f]{64}$")
        self.assertRegex(record["source_sha"], r"^[0-9a-f]{40}$")
        self.assertRegex(str(record["run_id"]), r"^[0-9]{1,20}$")
        self.assertTrue(record["workflow"] == ".github/workflows/build.yml"
                        or record["workflow"].startswith(BUILD_REPO + "/.github/workflows/build.yml@"))
        # and what it says is so
        data = tree.read(record["file"])
        self.assertEqual((record["size"], record["sha256"]), (len(data), sha256(data)))
        self.assertEqual(record["source_sha"], tree.commit)
        self.assertEqual(record["run_id"], CALLED["GITHUB_RUN_ID"])
        return record

    def check_sums(self, tree, names):
        """SHA256SUMS lists exactly these files, in this order, and sha256sum agrees."""
        text = tree.read("SHA256SUMS").decode()
        self.assertEqual(text, "".join(f"{sha256(tree.read(n))}  {n}\n" for n in names))
        done = subprocess.run(["sha256sum", "-c", "--strict", "SHA256SUMS"], cwd=tree.out, capture_output=True)
        self.assertEqual(done.returncode, 0, done.stderr)

    def test_windows(self):
        tree = Tree(self)
        packer = b"MZ" + os.urandom(2000)
        tree.put("HJViewer.exe", packer)
        tree.put("HJViewer-install.exe", packer)
        code, out, summary = tree.run("--platform", "windows", "--file", "HJViewer.exe", "--also",
                                      "HJViewer-install.exe", "--build", "1.6.0.41")
        self.assertEqual(code, 0, out)
        record = self.check_contract(tree.read("release.json").decode(), tree)
        self.assertEqual({k: record[k] for k in ("platform", "version", "build", "file", "workflow")},
                         {"platform": "windows", "version": "1.6.0", "build": "1.6.0.41", "file": "HJViewer.exe",
                          "workflow": ".github/workflows/build.yml"})
        self.check_sums(tree, ["HJViewer.exe", "HJViewer-install.exe", *NOTICES, "release.json"])
        # the hashes of what is handed out, for the person who signs the policy
        self.assertIn(f"{sha256(packer)}  HJViewer.exe\n{sha256(packer)}  HJViewer-install.exe\n", summary)
        self.assertIn("HJViewer 1.6.0 for windows (build 1.6.0.41)", summary)
        self.assertNotIn("LICENSE.txt", summary)
        self.assertNotIn("Check build", summary)
        self.assertIn(tree.read("release.json").decode(), out)
        # the run number is any 16-bit field of a Windows file version
        for number in ("0", "7", "65535"):
            self.assertEqual(tree.run("--platform", "windows", "--file", "HJViewer.exe", "--build", f"1.6.0.{number}")[0], 0)
            self.assertEqual(json.loads(tree.read("release.json"))["build"], f"1.6.0.{number}")

    def test_android_takes_the_build_number_from_the_apk(self):
        for utf8 in (False, True):
            tree = Tree(self)
            name = "HJViewer-1.6.0-0123abc-android-aarch64-signed.apk"
            tree.put(name, apk(262872251, "1.6.0", utf8))
            # what else the signing step leaves in the directory is not uploaded, and not listed
            tree.put("HJViewer-1.6.0-0123abc-android-aarch64.apk", apk(262872251, "1.6.0"))
            tree.put("HJViewer-1.6.0-0123abc-android-aarch64-signed.apk.idsig", b"a v4 signature")
            code, out, summary = tree.run("--platform", "android", "--file", name)
            self.assertEqual(code, 0, out)
            record = self.check_contract(tree.read("release.json").decode(), tree)
            self.assertEqual((record["build"], record["version"], record["file"]), ("262872251", "1.6.0", name))
            self.check_sums(tree, [name, *NOTICES, "release.json"])
            self.assertIn("(build 262872251)", summary)
            # the same number may be given; another one, or another version, is a mistake of the build
            self.assertEqual(tree.run("--platform", "android", "--file", name, "--build", "262872251")[0], 0)
        # the largest versionCode Google Play takes, and the smallest
        for code in (1, 2100000000):
            tree = Tree(self)
            tree.put("app.apk", apk(code, "1.6.0"))
            self.assertEqual(tree.run("--platform", "android", "--file", "app.apk")[0], 0)
            self.assertEqual(json.loads(tree.read("release.json"))["build"], str(code))
        for args, package in (
            (["--build", "262870251"], apk(262872251, "1.6.0")),
            ([], apk(262872251, "1.5.1")),
            ([], apk(0, "1.6.0")),
            ([], apk(2100000001, "1.6.0")),
            ([], b"not a zip"),
            ([], ipa()),
        ):
            tree = Tree(self)
            tree.put("app.apk", package)
            code, out, _ = tree.run("--platform", "android", "--file", "app.apk", *args)
            self.assertEqual(code, 1, out)
            self.assertIn("error: ", out)
            self.assertFalse(tree.has("release.json") or tree.has("SHA256SUMS"))

    def test_an_android_build_of_1_6_0_only_controls_and_updates_itself(self):
        """From 1.6.0 the APK's manifest has the in-app update's permission and provider and
        nothing of the controlled side, in a check build and a debug-signed one too (docs/ci.md,
        "Android 매니페스트 확인"): the step fails when a client patch went missing."""
        def without(name):
            return tuple(c for c in MANIFEST_1_6 if c[2]["name"] != name)

        def renamed(old, new):
            return tuple((d, e, {**a, "name": new}) if a["name"] == old else (d, e, a) for d, e, a in MANIFEST_1_6)

        install, provider = "android.permission.REQUEST_INSTALL_PACKAGES", APP + "HjvUpdateFileProvider"
        refused = [
            # the two entries of the in-app update, of this app and where Android reads them
            (without(install), [f"lacks permission {install}"]),
            (without(provider), [f"lacks provider {provider}"]),
            (without(install) + ((2, "uses-permission", {"name": install}),), [f"lacks permission {install}"]),
            (renamed(provider, "kr.ac.yonsei.hjviewer.HjvUpdateFileProvider"), [f"lacks provider {provider}"]),
            (tuple(c for c in MANIFEST_1_6 if c[2]["name"] not in (install, provider)),
             [f"lacks permission {install}, provider {provider}"]),
            # what the controlled side declares, also the 1.5.1 manifest at 1.6.0
            (MANIFEST_1_5, [f"lacks permission {install}, provider {provider}", "permission android.permission.FOREGROUND_SERVICE,",
                            "service " + APP + "MainService", "accessibility-service " + APP + "InputService"]),
        ]
        for permission in ("FOREGROUND_SERVICE", "FOREGROUND_SERVICE_MEDIA_PROJECTION", "FOREGROUND_SERVICE_MICROPHONE",
                           "FOREGROUND_SERVICE_SPECIAL_USE", "FOREGROUND_SERVICE_DATA_SYNC", "SYSTEM_ALERT_WINDOW",
                           "RECEIVE_BOOT_COMPLETED", "REQUEST_IGNORE_BATTERY_OPTIMIZATIONS"):
            for element in ("uses-permission", "uses-permission-sdk-23"):
                refused.append((MANIFEST_1_6 + ((1, element, {"name": "android.permission." + permission}),),
                                [f"declares permission android.permission.{permission}, of the controlled side"]))
        for element, name in (("service", "InputService"), ("service", "MainService"),
                              ("service", "FloatingWindowService"), ("receiver", "BootReceiver")):
            refused.append((MANIFEST_1_6 + ((2, element, {"name": APP + name}),), [f"declares {element} {APP}{name},"]))
        # a library's foreground service or accessibility service, whatever its class
        refused.append((MANIFEST_1_6 + ((2, "service", {"name": "androidx.work.impl.foreground.SystemForegroundService",
                                                        "foregroundServiceType": 1}),),
                        ["declares foreground-service-type of service androidx.work.impl.foreground.SystemForegroundService,"]))
        refused.append((MANIFEST_1_6 + ((2, "service", {"name": "com.example.Reader",
                                                        "permission": "android.permission.BIND_ACCESSIBILITY_SERVICE"}),),
                        ["declares accessibility-service com.example.Reader,"]))
        for children, said in refused:
            for env, more in (({}, []), ({"CHECK_ONLY": "true"}, []), ({}, ["--unreleased", "debug-signed"])):
                tree = Tree(self)
                tree.put("app.apk", apk(children=children))
                code, out, _ = tree.run("--platform", "android", "--file", "app.apk", *more, env=env)
                self.assertEqual(code, 1, out)
                self.assertIn("error: app.apk is not an Android build of 1.6.0: it ", out)
                for text in said:
                    self.assertIn(text, out)
                self.assertFalse(tree.has("release.json") or tree.has("SHA256SUMS"))
        taken = [
            MANIFEST_1_6,
            # the install permission in its other form; what is not the controlled side
            without(install) + ((1, "uses-permission-sdk-23", {"name": install}),),
            MANIFEST_1_6 + ((2, "provider", {"name": "com.example.MainService"}),
                            (2, "receiver", {"name": "androidx.profileinstaller.ProfileInstallReceiver",
                                             "permission": "android.permission.DUMP"}),
                            (1, "uses-permission", {"name": "android.permission.CAMERA"}),
                            (1, "uses-permission", {"name": "android.permission.POST_NOTIFICATIONS"})),
            # the app lock's (1.9.0: local_auth_android and androidx.biometric) are not the controlled side
            MANIFEST_1_6 + ((1, "uses-permission", {"name": "android.permission.USE_BIOMETRIC"}),
                            (1, "uses-permission", {"name": "android.permission.USE_FINGERPRINT"})),
        ]
        for children in taken:
            tree = Tree(self)
            tree.put("app.apk", apk(children=children))
            code, out, _ = tree.run("--platform", "android", "--file", "app.apk")
            self.assertEqual(code, 0, out)
            self.assertIn(f"the manifest has permission {install} and provider {provider}, and nothing of the "
                          "controlled side", out)

    def test_ios_takes_the_build_number_from_the_ipa(self):
        for fmt in (plistlib.FMT_BINARY, plistlib.FMT_XML):
            tree = Tree(self)
            name = "HJViewer-1.6.0-0123abc-ios.ipa"
            tree.put(name, ipa("262870101", "1.6.0", fmt=fmt))
            code, out, _ = tree.run("--platform", "ios", "--file", name)
            self.assertEqual(code, 0, out)
            record = self.check_contract(tree.read("release.json").decode(), tree)
            self.assertEqual((record["platform"], record["build"], record["file"]), ("ios", "262870101", name))
            self.check_sums(tree, [name, *NOTICES, "release.json"])
        for args, package in (
            (["--build", "262870102"], ipa()),
            ([], ipa(version="1.6.1")),
            ([], ipa(build="1.6.0")),
            ([], ipa(apps=("Runner", "Other"))),
            ([], ipa(apps=())),
            ([], apk()),
            ([], b""),
        ):
            tree = Tree(self)
            tree.put("app.ipa", package)
            code, out, _ = tree.run("--platform", "ios", "--file", "app.ipa", *args)
            self.assertEqual(code, 1, out)
            self.assertFalse(tree.has("release.json") or tree.has("SHA256SUMS"))

    def test_a_check_build_gets_no_release_json(self):
        """tools/hjv-release takes no directory without one for a release."""
        tree = Tree(self)
        tree.put("HJViewer.exe", b"MZ check")
        tree.put("release.json", b"{}")  # one that was there is removed
        code, out, summary = tree.run("--platform", "windows", "--file", "HJViewer.exe", "--build", "1.6.0.7",
                                      env={"CHECK_ONLY": "true"})
        self.assertEqual(code, 0, out)
        self.assertFalse(tree.has("release.json"))
        self.check_sums(tree, ["HJViewer.exe", *NOTICES])
        self.assertIn("**Not for release** (check build)", summary)
        self.assertIn("no release.json", out)
        # the step still does all its checks in a check build
        code, out, _ = tree.run("--platform", "windows", "--file", "HJViewer.exe", "--build", "1.6.1.7",
                                env={"CHECK_ONLY": "true"})
        self.assertEqual(code, 1, out)
        # only "true" makes a check build, as in scripts/check-build-keys.py
        for value in ("false", "", "1", "TRUE"):
            self.assertEqual(tree.run("--platform", "windows", "--file", "HJViewer.exe", "--build", "1.6.0.7",
                                      env={"CHECK_ONLY": value})[0], 0)
            self.assertTrue(tree.has("release.json"), value)

    def test_a_package_not_for_release_gets_no_release_json(self):
        """A debug-signed APK installs over no user's app, an unsigned IPA nowhere."""
        tree = Tree(self)
        tree.put("app.apk", apk())
        self.assertEqual(tree.run("--platform", "android", "--file", "app.apk")[0], 0)
        self.assertTrue(tree.has("release.json"))
        code, out, summary = tree.run("--platform", "android", "--file", "app.apk", "--unreleased", "debug-signed")
        self.assertEqual(code, 0, out)
        self.assertFalse(tree.has("release.json"))  # the one of the run before is gone too
        self.check_sums(tree, ["app.apk", *NOTICES])
        self.assertIn("**Not for release** (debug-signed)", summary)
        # the checks of a release are made all the same
        self.assertEqual(tree.run("--platform", "android", "--file", "app.apk", "--unreleased", "debug-signed",
                                  "--build", "1")[0], 1)
        # an empty reason is no reason: a release
        self.assertEqual(tree.run("--platform", "android", "--file", "app.apk", "--unreleased", "")[0], 0)
        self.assertTrue(tree.has("release.json"))
        # the reason goes to the log and the summary as it is: plain words only
        for why in ("::error::x", "Debug", "a" * 41, "debug\nsigned"):
            code, out, summary = tree.run("--platform", "android", "--file", "app.apk", "--unreleased", why)
            self.assertEqual((code, summary), (1, ""), why)

    def test_the_run_and_its_workflow(self):
        tree = Tree(self)
        tree.put("HJViewer.exe", b"MZ")
        args = ("--platform", "windows", "--file", "HJViewer.exe", "--build", "1.6.0.7")

        def workflow(**env):
            code, out, _ = tree.run(*args, env=env)
            return json.loads(tree.read("release.json"))["workflow"] if code == 0 else out

        self.assertEqual(workflow(), ".github/workflows/build.yml")
        # a run of this repository itself; a branch with an @ in its name
        self.assertEqual(workflow(GITHUB_REPOSITORY="yonsei-projecttf/hjviewer",
                                  GITHUB_WORKFLOW_REF="yonsei-projecttf/hjviewer/.github/workflows/client-build.yml@refs/heads/a@b"),
                         ".github/workflows/client-build.yml")
        self.assertEqual(workflow(GITHUB_WORKFLOW_REF=BUILD_REPO + "/.github/workflows/build.yaml@refs/tags/v1"),
                         ".github/workflows/build.yaml")
        os.remove(os.path.join(tree.out, "release.json"))
        os.remove(os.path.join(tree.out, "SHA256SUMS"))
        # outside a run, or with names that are no run's and no workflow's, nothing is written
        for env in (
            {"GITHUB_RUN_ID": None},
            {"GITHUB_RUN_ID": ""},
            {"GITHUB_RUN_ID": "42a"},
            {"GITHUB_RUN_ID": "4242/../1"},
            {"GITHUB_RUN_ID": "1" * 21},
            {"GITHUB_WORKFLOW_REF": None},
            {"GITHUB_WORKFLOW_REF": ""},
            {"GITHUB_WORKFLOW_REF": BUILD_REPO + "/.github/workflows/a b.yml@refs/heads/main"},
            {"GITHUB_WORKFLOW_REF": BUILD_REPO + "/.github/workflows/build.yml@refs/heads/\x1b[2J"},
            {"GITHUB_WORKFLOW_REF": BUILD_REPO + "/.github/workflows/build.yml@" + "r" * 300},
        ):
            self.assertIn("error: GITHUB_", workflow(**env), env)
            self.assertFalse(tree.has("release.json") or tree.has("SHA256SUMS"), env)
        # A workflow reference of another form than this repository's file loses no build: the
        # record carries what GitHub said, with a warning, and tools/hjv-release judges it.
        for ref in ("someone/else/.github/workflows/build.yml@refs/heads/main",
                    BUILD_REPO + "-2/.github/workflows/build.yml@refs/heads/main",
                    BUILD_REPO + "/build.yml@refs/heads/main",
                    BUILD_REPO + "/.github/workflows/../../x.yml@refs/heads/main",
                    BUILD_REPO + "/.github/workflows/build.sh@refs/heads/main"):
            code, out, _ = tree.run(*args, env={"GITHUB_WORKFLOW_REF": ref})
            self.assertEqual(code, 0, out)
            self.assertIn("warning: GITHUB_WORKFLOW_REF", out)
            self.assertEqual(json.loads(tree.read("release.json"))["workflow"], ref)
        code, out, _ = tree.run(*args, env={"GITHUB_REPOSITORY": None})
        self.assertEqual((code, json.loads(tree.read("release.json"))["workflow"]), (0, CALLED["GITHUB_WORKFLOW_REF"]), out)
        self.assertNotIn("warning", tree.run(*args)[1])

    def test_the_source_commit_is_the_checkout(self):
        tree = Tree(self)
        tree.put("HJViewer.exe", b"MZ")
        args = ("--platform", "windows", "--file", "HJViewer.exe", "--build", "1.6.0.7")
        self.assertEqual(tree.run(*args)[0], 0)
        self.assertEqual(json.loads(tree.read("release.json"))["source_sha"], tree.commit)
        # outside a checkout nothing is written
        shutil.rmtree(os.path.join(tree.root, ".git"))
        os.remove(os.path.join(tree.out, "release.json"))
        os.remove(os.path.join(tree.out, "SHA256SUMS"))
        with mock.patch.dict(os.environ, {"GIT_CEILING_DIRECTORIES": os.path.dirname(tree.root)}):
            code, out, _ = tree.run(*args)
        self.assertEqual(code, 1, out)
        self.assertFalse(tree.has("release.json") or tree.has("SHA256SUMS"))

    def test_what_is_refused(self):
        tree = Tree(self)
        tree.put("HJViewer.exe", b"MZ")
        tree.put("HJViewer-install.exe", b"MZ")
        tree.put("empty.exe", b"")
        tree.put(".hidden.exe", b"MZ")
        os.makedirs(os.path.join(tree.out, "sub"))
        tree.put("inner.exe", b"MZ", os.path.join(tree.out, "sub"))
        os.symlink(os.path.join(tree.out, "HJViewer.exe"), os.path.join(tree.out, "link.exe"))
        good = ["--platform", "windows", "--file", "HJViewer.exe", "--build", "1.6.0.7"]
        self.assertEqual(tree.run(*good)[0], 0)
        os.remove(os.path.join(tree.out, "release.json"))
        os.remove(os.path.join(tree.out, "SHA256SUMS"))

        def file(name):
            return ["--platform", "windows", "--file", name, "--build", "1.6.0.7"]

        def build(number):
            return ["--platform", "windows", "--file", "HJViewer.exe", "--build", number]

        cases = [
            file("missing.exe"), file("empty.exe"), file(".hidden.exe"), file("sub/inner.exe"), file("../out/HJViewer.exe"),
            file("link.exe"), file("sub"), file(""), file("HJViewer.apk"), file("LICENSE.txt"),
            ["--platform", "android", "--file", "HJViewer.exe"],
            ["--platform", "windows", "--file", "HJViewer.exe"],  # no build number
            build("1.6.0"), build("1.6.0.x"), build("1.6.1.7"), build("1.6.0.65536"), build("1.6.0.07"), build("1.6.0.7.1"),
            build("1.6.0.7\n1.6.0.8"), build("11.6.0.7"),
            good + ["--also", "missing.exe"], good + ["--also", "HJViewer.exe"], good + ["--also", "release.json"],
            good + ["--also", "HJViewer-install.exe", "--also", "HJViewer-install.exe"], good + ["--also", "link.exe"],
            good + ["--also", "sub/inner.exe"],
        ]
        for args in cases:
            code, out, summary = tree.run(*args)
            self.assertEqual(code, 1, args)
            self.assertIn("error: ", out, args)
            self.assertEqual(summary, "", args)
            self.assertFalse(tree.has("release.json") or tree.has("SHA256SUMS"), args)
        code, out, _ = tree.run(*good, directory=os.path.join(tree.root, "nowhere"))
        self.assertEqual(code, 1, out)
        # in a run, the problem is an annotation
        code, out, _ = tree.run(*file("missing.exe"), env={"GITHUB_ACTIONS": "true"})
        self.assertIn("::error::", out)

    def test_versions_the_release_tool_would_not_take(self):
        """Its policies name versions on which the apps' version numbers and plain comparison
        agree; a build of another version fails here, in the check build already."""
        for version, ok in (("1.6.0", True), ("0.0.0", True), ("999.999.99", True), ("1.6", False), ("1.6.100", False),
                            ("1.06.0", False), ("1.6.0.1", False), ("1000.0.0", False), ("v1.6.0", False),
                            ("1.6.0-rc1", False)):
            tree = Tree(self, version)
            tree.put("HJViewer.exe", b"MZ")
            code, out, _ = tree.run("--platform", "windows", "--file", "HJViewer.exe", "--build", f"{version}.7")
            self.assertEqual(code, 0 if ok else 1, f"{version}: {out}")
        tree = Tree(self)
        tree.put("HJViewer.exe", b"MZ")
        for text in ("", "HJV_VERSION=1.6.0\nHJV_VERSION=1.6.1\n", "VERSION=1.6.0\n"):
            with open(os.path.join(tree.root, "client", "version.env"), "w") as f:
                f.write(text)
            self.assertEqual(tree.run("--platform", "windows", "--file", "HJViewer.exe", "--build", "1.6.0.7")[0], 1)

    def test_manifests(self):
        for utf8 in (False, True):
            self.assertEqual(ri.manifest_version(binary_manifest(262872251, "1.6.0", utf8)), (262872251, "1.6.0"))
            self.assertEqual(ri.manifest_version(binary_manifest(1, "10.20.30-한글", utf8)), (1, "10.20.30-한글"))
        good = binary_manifest(7, "1.6.0")
        broken = [b"", b"PK\x03\x04", good[:40], good[:-60], b"\x03\x00\x08\x00" + good[4:8] + b"\0" * 64,
                  good.replace(b"m\0a\0n\0i\0f\0e\0s\0t\0", b"m\0a\0n\0i\0f\0e\0s\0u\0"),
                  good.replace(struct.pack("<I", 0x0101021B), struct.pack("<I", 0x0101021D)),
                  good[:4] + struct.pack("<I", len(good) + 1) + good[8:]]
        for data in broken:
            with self.assertRaises(ri.Problem):
                ri.manifest_version(data)
        # cut anywhere, it is refused, never misread
        for cut in range(0, len(good), 7):
            with self.assertRaises(ri.Problem):
                ri.manifest_version(good[:cut])

    def test_the_elements_of_a_manifest(self):
        application = ("manifest", "application")
        for utf8 in (False, True):
            found = ri.manifest_elements(binary_manifest(7, "1.6.0", utf8, MANIFEST_1_5))
            self.assertEqual([p for p, _ in found], [("manifest",)] + [("manifest", "uses-permission")] * 6 + [
                application, application + ("receiver",), application + ("service",),
                application + ("service", "meta-data"), application + ("activity",), application + ("service",),
                application + ("service",)])
            self.assertEqual((found[0][1][0x0101021B], found[0][1][0x0101021C]), (7, "1.6.0"))
            self.assertEqual(found[9][1], {0x01010003: APP + "InputService",
                                           0x01010006: "android.permission.BIND_ACCESSIBILITY_SERVICE"})
            self.assertEqual(found[12][1], {0x01010003: APP + "MainService", 0x01010599: 0x400000A0})
            self.assertEqual(ri.android_manifest_problems(binary_manifest(7, "1.6.0", utf8, MANIFEST_1_5)), (
                ["permission android.permission.REQUEST_INSTALL_PACKAGES", "provider " + APP + "HjvUpdateFileProvider"],
                ["accessibility-service " + APP + "InputService", "foreground-service-type of service " + APP + "MainService",
                 "permission android.permission.FOREGROUND_SERVICE",
                 "permission android.permission.FOREGROUND_SERVICE_SPECIAL_USE",
                 "permission android.permission.RECEIVE_BOOT_COMPLETED",
                 "permission android.permission.REQUEST_IGNORE_BATTERY_OPTIMIZATIONS",
                 "permission android.permission.SYSTEM_ALERT_WINDOW", "receiver " + APP + "BootReceiver",
                 "service " + APP + "FloatingWindowService", "service " + APP + "InputService",
                 "service " + APP + "MainService"]))
            self.assertEqual(ri.android_manifest_problems(binary_manifest(7, "1.6.0", utf8, MANIFEST_1_6)), ([], []))
        good = binary_manifest(7, "1.6.0", children=MANIFEST_1_6)
        start = b"\x02\x01\x10\x00"  # an element's start: its chunk type and header size
        first_child = good.index(start, good.index(start) + 1)
        size = struct.unpack_from("<I", good, first_child + 4)[0]

        def resized(data):
            return data[:4] + struct.pack("<I", len(data)) + data[8:]

        broken = {
            "an element ends that did not start": resized(good[:first_child] + good[first_child + size:]),
            "elements that do not end": resized(good[:-24]),
            "a second root element": binary_manifest(7, "1.6.0", children=((0, "manifest", {}),)),
            "no element": resized(good[:good.index(start)]),
        }
        for why, data in broken.items():
            with self.assertRaisesRegex(ri.Problem, why):
                ri.manifest_elements(data)
        # whatever byte goes wrong, the reader refuses with a Problem or reads what is there
        for at in range(len(good)):
            data = good[:at] + bytes([good[at] ^ 0xFF]) + good[at + 1:]
            try:
                ri.android_manifest_problems(data)
            except ri.Problem:
                pass

    def test_real_packages(self):
        """Real packages (SAMPLES, or a download of a run in HJV_RELEASE_SAMPLES): every APK and IPA
        says the version that its artifact's name does, and carries a plausible build number."""
        samples = os.environ.get("HJV_RELEASE_SAMPLES") or SAMPLES
        seen = set()
        for top, _, names in os.walk(samples):
            named = re.search(r"HJViewer-(\d+\.\d+\.\d+)-", os.path.basename(top))
            for name in names:
                path = os.path.join(top, name)
                if name.endswith(".apk"):
                    code, version = ri.apk_version(path)
                    self.assertTrue(2000 < code <= 2100000000, path)
                    # Supported release samples are controllers with in-app updates.
                    missing, controlled = ri.android_manifest_problems(ri.apk_manifest(path))
                    self.assertEqual((missing, controlled), ([], []), path)
                elif name.endswith(".ipa"):
                    build, version = ri.ipa_version(path)
                    self.assertRegex(build, r"^\d{9}$", path)
                else:
                    continue
                seen.add(name.rsplit(".", 1)[1])
                if named:
                    self.assertEqual(version, named.group(1), path)
        self.assertEqual(seen, {"apk", "ipa"}, "the samples hold no APK or no IPA")

    def test_the_runners_pythons_can_read_the_scripts(self):
        """The Windows and macOS runners' python3 may be 3.9: nothing newer in the syntax of the
        two scripts the platform jobs run."""
        for script in (SCRIPT, os.path.join(HERE, "check-build-keys.py")):
            with open(script, encoding="utf-8") as f:
                ast.parse(f.read(), filename=script, feature_version=(3, 8))


class ReleaseToolTest(unittest.TestCase):
    """tools/hjv-release takes what the script writes (HJV_RELEASE_BIN): `policy` finds the
    packages and their release.json among the run's artifacts and makes a draft with test keys."""

    GH = """#!/bin/sh
# replays what GitHub answers about run 4242 (as tools/hjv-release/tests/cli.rs does)
case "$*" in
  "api repos/yonsei-projecttf/hjviewer-private/actions/runs/4242")
    exec cat "$HJV_TEST_GH/run.json" ;;
  "api -H Accept: application/vnd.github.raw+json repos/yonsei-projecttf/hjviewer-private/contents/.github/workflows/build.yml?ref=2222222222222222222222222222222222222222")
    exec cat "$HJV_TEST_GH/build.yml" ;;
  "run download 4242 -R yonsei-projecttf/hjviewer-private -D "*)
    exec cp -R "$HJV_TEST_GH/artifacts/." "$7" ;;
  "api repos/yonsei-projecttf/hjviewer-private")
    # the build repository is there (it has no store record yet: --no-appstore reads a 404)
    echo '{"full_name":"yonsei-projecttf/hjviewer-private","private":true}'
    exit 0 ;;
esac
echo "gh: Not Found (HTTP 404)" >&2
exit 1
"""

    def setUp(self):
        self.tool = os.environ.get("HJV_RELEASE_BIN")
        if not self.tool:
            self.skipTest("HJV_RELEASE_BIN is not set")

    def documented_build_yml(self, commit):
        """The build repository's build.yml as docs/ci.md gives it, pinned to <commit>."""
        with open(os.path.join(ROOT, "docs", "ci.md"), encoding="utf-8") as f:
            found = re.search(r"```yaml\n(name: build\n.*?)```", f.read(), re.S)
        self.assertEqual(found.group(1).count("<커밋>"), 2)
        return found.group(1).replace("<커밋>", commit)

    def world(self, check_only=False):
        """A run's three artifacts as the jobs' steps leave them, GitHub's answers about the
        run, and a clone with the release tag."""
        tree = Tree(self)
        git(tree.root, "tag", "-a", "client-1.6.0", "-m", "HJViewer 1.6.0")
        ios_build = time.strftime("%y%j%H%M", time.gmtime(time.time() - 86400))
        suffix = "-check" if check_only else ""
        artifacts = os.path.join(tree.root, "gh", "artifacts")
        env = {"CHECK_ONLY": "true"} if check_only else {}
        for platform, directory, name, data, args in (
            ("windows", "HJViewer-1.6.0-0123abc-windows-x86_64", "HJViewer.exe", b"MZ" + os.urandom(999),
             ["--build", "1.6.0.41"]),
            ("android", "HJViewer-1.6.0-0123abc-android-aarch64", "HJViewer-1.6.0-0123abc-android-aarch64-signed.apk",
             apk(int(ios_build) + 2000, "1.6.0"), []),
            ("ios", "HJViewer-1.6.0-0123abc-ios", "HJViewer-1.6.0-0123abc-ios.ipa", ipa(ios_build, "1.6.0"), []),
        ):
            out = os.path.join(artifacts, directory + suffix)
            os.makedirs(out)
            for notice in NOTICES:
                tree.put(notice, f"{notice} of this build\n".encode(), out)
            tree.put(name, data, out)
            code, text, _ = tree.run("--platform", platform, "--file", name, *args, env=env, directory=out)
            self.assertEqual(code, 0, text)
        gh = os.path.join(tree.root, "gh")
        run = {
            "id": 4242, "name": "build", "path": ".github/workflows/build.yml", "event": "workflow_dispatch",
            "status": "completed", "conclusion": "success", "head_branch": "main", "head_sha": "2" * 40,
            "referenced_workflows": [{
                "path": f"yonsei-projecttf/hjviewer/.github/workflows/client-build.yml@{tree.commit}",
                "sha": tree.commit}],
            "repository": {"full_name": BUILD_REPO, "private": True},
        }
        with open(os.path.join(gh, "run.json"), "w") as f:
            json.dump(run, f)
        with open(os.path.join(gh, "build.yml"), "w", encoding="utf-8") as f:
            f.write(self.documented_build_yml(tree.commit))
        os.makedirs(os.path.join(tree.root, "bin"))
        with open(os.path.join(tree.root, "bin", "gh"), "w") as f:
            f.write(self.GH)
        os.chmod(os.path.join(tree.root, "bin", "gh"), 0o755)
        # the development PC's download of the run
        shutil.copytree(artifacts, os.path.join(tree.root, "release"))
        keys = subprocess.run([self.tool, "test-keys", "--env"], capture_output=True, text=True, check=True).stdout
        with open(os.path.join(tree.root, "keys.env"), "w") as f:
            f.write(keys)
        return tree, ios_build

    def policy(self, tree, ios_build, *more):
        env = {**os.environ, "PATH": os.path.join(tree.root, "bin") + os.pathsep + os.environ["PATH"],
               "HJV_TEST_GH": os.path.join(tree.root, "gh")}
        # The documented build.yml passes testflight_run (1.9.2), which an older tool takes only
        # allowed (tools/hjv-release/README.md): HJV_RELEASE_BIN is the server stage's build, of
        # another tree until that stage has built this one's. That this tree's tool knows the input
        # is the tool's own test (src/provenance.rs, the_documented_build_yml).
        return subprocess.run(
            [self.tool, "policy", "--keys", "keys.env", "--allow-test-keys", "--prev", "none", "--server", "hjv.test",
             "--release-dir", "release", "--repo", ".", "--min", "all=1.5.0", "--grace-days", "0",
             "--auto-after", "windows=none", "--ios-valid", ios_build, "--no-appstore", "--out", "draft.json",
             "--allow-input", "testflight_run", *more],
            cwd=tree.root, env=env, capture_output=True, text=True)

    def test_the_tool_makes_a_draft_from_a_runs_artifacts(self):
        tree, ios_build = self.world()
        # check_only is an input of the documented build.yml that the tool may not know yet
        done = self.policy(tree, ios_build, "--allow-input", "check_only")
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        with open(os.path.join(tree.root, "draft.json"), encoding="utf-8") as f:
            draft = json.load(f)
        policy = json.loads(draft["p"])["platforms"]
        for platform, directory, name in (
            ("windows", "HJViewer-1.6.0-0123abc-windows-x86_64", "HJViewer.exe"),
            ("android", "HJViewer-1.6.0-0123abc-android-aarch64", "HJViewer-1.6.0-0123abc-android-aarch64-signed.apk"),
        ):
            with open(os.path.join(tree.root, "release", directory, name), "rb") as f:
                data = f.read()
            package = policy[platform]["package"]
            self.assertEqual((package["size"], package["sha256"]), (len(data), sha256(data)))
            self.assertEqual(policy[platform]["latest"], "1.6.0")
        self.assertEqual(policy["windows"]["package"]["build"], "1.6.0.41")
        self.assertEqual(policy["android"]["package"]["build"], str(int(ios_build) + 2000))
        self.assertEqual(policy["ios"]["testflight_build"], ios_build)
        # a package that is not the run's, under the run's release.json, is refused
        with open(os.path.join(tree.root, "release", "HJViewer-1.6.0-0123abc-windows-x86_64", "HJViewer.exe"), "ab") as f:
            f.write(b"!")
        self.assertNotEqual(self.policy(tree, ios_build, "--allow-input", "check_only").returncode, 0)

    def test_the_tool_takes_no_check_build(self):
        tree, ios_build = self.world(check_only=True)
        done = self.policy(tree, ios_build, "--allow-input", "check_only")
        self.assertEqual(done.returncode, 1, done.stdout + done.stderr)
        self.assertIn("release.json", done.stderr)
        self.assertFalse(os.path.exists(os.path.join(tree.root, "draft.json")))


class WorkflowTest(unittest.TestCase):
    """client-build.yml: the steps of the minimum-version work besides the keys."""

    @classmethod
    def setUpClass(cls):
        try:
            import yaml
        except ImportError:
            raise unittest.SkipTest("PyYAML is not installed")
        with open(WORKFLOW) as f:
            cls.wf = yaml.safe_load(f)

    def steps(self, job):
        return self.wf["jobs"][job]["steps"]

    def step(self, job, name):
        for s in self.steps(job):
            if s.get("name") == name:
                return s
        self.fail(f"{job} has no step '{name}'")

    def index(self, job, name):
        return [s.get("name") for s in self.steps(job)].index(name)

    # job: (the directory it uploads, its upload steps)
    UPLOADS = {
        "windows": ("upstream/rustdesk/SignOutput", ["Upload Windows build (unsigned)"]),
        "android": ("upstream/rustdesk/signed-apk", ["Upload signed APK", "Upload debug-signed APK"]),
        "ios": ("upstream/rustdesk/ios-out", ["Upload IPA"]),
    }
    RECORD = "Write release.json and SHA256SUMS"

    # what each platforms value builds: (windows, android, ios, relay)
    PLANS = {"windows": (1, 0, 0, 0), "android": (0, 1, 0, 0), "ios": (0, 0, 1, 0), "relay": (0, 0, 0, 1),
             "all": (1, 1, 1, 1)}

    def plan(self, env):
        """The plan step's script as the runner runs it: (exit code, output, its outputs or None)."""
        step = self.step("preflight", "Plan build targets")
        with tempfile.TemporaryDirectory() as tmp:
            output = os.path.join(tmp, "outputs")
            done = subprocess.run(["bash", "--noprofile", "--norc", "-eo", "pipefail", "-c", step["run"]],
                                  cwd=tmp, env={**os.environ, "GITHUB_OUTPUT": output, "TESTFLIGHT_RUN": "", **env},
                                  capture_output=True, text=True)
            self.assertFalse(os.path.exists(os.path.join(tmp, "injected")))
            outputs = None
            if os.path.exists(output):
                with open(output) as f:
                    outputs = dict(line.split("=", 1) for line in f.read().splitlines())
            return done.returncode, done.stdout + done.stderr, outputs

    def test_build_plan_rejects_unknown_platforms_and_uploads_without_ios(self):
        step = self.step("preflight", "Plan build targets")
        self.assertEqual(step["env"], {
            "PLATFORMS": "${{ github.event_name == 'push' && 'all' || inputs.platforms }}",
            "TESTFLIGHT": "${{ inputs.testflight }}",
            "TESTFLIGHT_RUN": "${{ inputs.testflight_run }}",
        })
        # first, before anything is checked out
        self.assertEqual(self.index("preflight", "Plan build targets"), 0)
        triggers = self.wf.get("on", self.wf.get(True))  # PyYAML's YAML 1.1 spelling of `on`
        for trigger in ("workflow_dispatch", "workflow_call"):
            self.assertIs(triggers[trigger]["inputs"]["testflight"]["default"], False)
            self.assertEqual(triggers[trigger]["inputs"]["testflight_run"]["type"], "string")
        for platform in ("windows", "android", "ios", "relay", "all", "", "unknown", "$(touch injected)"):
            for testflight, run in (("false", ""), ("true", ""), ("true", "40363130281"), ("false", "40363130281"),
                                    ("true", "4036313028x"), ("true", "$(touch injected)")):
                with self.subTest(platform=platform, testflight=testflight, run=run):
                    code, out, outputs = self.plan({"PLATFORMS": platform, "TESTFLIGHT": testflight,
                                                    "TESTFLIGHT_RUN": run})
                    upload = testflight == "true"
                    valid = platform in self.PLANS and (
                        (not upload and not run) or (upload and platform == "ios" and run.isdigit()))
                    self.assertEqual(code, 0 if valid else 1, out)
                    if not valid:
                        self.assertIsNone(outputs)
                        continue
                    # a TestFlight upload builds nothing: it sends a finished build run's IPA
                    builds = dict(zip(("windows", "android", "ios", "relay"), (0, 0, 0, 0) if upload else self.PLANS[platform]))
                    want = {job: "true" if on else "false" for job, on in builds.items()}
                    want["build_apps"] = "false" if upload or platform == "relay" else "true"
                    want["testflight"] = "true" if upload else "false"
                    self.assertEqual(outputs, want)

    def test_the_jobs_build_what_the_plan_says_from_one_source_commit(self):
        preflight = self.wf["jobs"]["preflight"]
        self.assertNotIn("if", preflight)  # relay still needs a successful preflight and a source SHA
        self.assertNotIn("needs", preflight)
        self.assertEqual(preflight["outputs"]["source_sha"], "${{ steps.source.outputs.sha }}")
        for name in ("build_apps", "windows", "android", "ios", "relay", "testflight"):
            self.assertEqual(preflight["outputs"][name], f"${{{{ steps.plan.outputs.{name} }}}}")
        for name in ("Check out hjviewer", "Record the source commit", "Check the pinned public keys"):
            self.assertNotIn("if", self.step("preflight", name), name)
        # no job builds before the checks have passed, and each reads its part of the plan
        conditions = {"generate-bridge": "build_apps", "dart-tests": "build_apps", "windows": "windows",
                      "android": "android", "ios": "ios", "relay": "relay", "testflight": "testflight"}
        for job, output in conditions.items():
            self.assertEqual(self.wf["jobs"][job]["if"], f"needs.preflight.outputs.{output} == 'true'", job)
            needs = self.wf["jobs"][job]["needs"]
            self.assertIn("preflight", [needs] if isinstance(needs, str) else needs, job)
        # a relay-only run needs no bridge
        self.assertEqual(self.wf["jobs"]["relay"]["needs"], "preflight")
        self.assertNotIn("bridge-artifact", json.dumps(self.wf["jobs"]["relay"]))
        for job in ("generate-bridge", "dart-tests", "windows", "android", "ios", "relay"):
            self.assertEqual(self.step(job, "Check out hjviewer")["with"]["ref"],
                             "${{ needs.preflight.outputs.source_sha }}", job)
        self.assertNotIn("needs.generate-bridge.outputs", json.dumps(self.wf["jobs"]))

    def test_windows_and_android_wait_for_the_bridge_files_right_before_cargo(self):
        """GHA-11: they start beside the Bridge codegen job; iOS still waits for it (macOS minutes)."""
        for job in ("windows", "android"):
            self.assertEqual(self.wf["jobs"][job]["needs"], "preflight", job)
            self.assertEqual(self.wf["jobs"][job]["permissions"], {"contents": "read", "actions": "read"}, job)
            steps = self.steps(job)
            wait, restore = self.index(job, "Wait for the bridge files"), self.index(job, "Restore bridge files")
            self.assertEqual(restore, wait + 1, job)
            # the bridge files are needed by cargo's build only: before them cargo downloads the
            # crates of the build's target (after the Rust cache's restore, whose key it does not
            # change), and nothing builds Rust or the app
            fetch = self.index(job, "Fetch the crates")
            self.assertEqual(wait, fetch + 1, job)
            self.assertLess([s.get("id") for s in steps].index("rust-cache"), fetch, job)
            target = {"windows": "x86_64-pc-windows-msvc", "android": "aarch64-linux-android"}[job]
            self.assertEqual(steps[fetch]["run"].strip(), f"cargo fetch --locked --target {target}", job)
            self.assertNotIn("env", steps[fetch], job)
            # the target the job's toolchain has (Windows) or adds before cargo ndk (Android)
            built = self.step(job, "Install Rust toolchain")["with"].get("targets", "") + self.step(
                job, "Build rustdesk lib" if job == "android" else "Build rustdesk")["run"]
            self.assertIn(target, built, job)
            for step in steps[:restore]:
                if step is steps[fetch]:
                    continue
                for builder in ("cargo", "build.py", "flutter build", "ndk_arm64"):
                    self.assertNotIn(builder, step.get("run", ""), f"{job}: {step.get('name')}")
            # and they are, a few steps (the toolchain, the cache) later
            later = " ".join(step.get("run", "") for step in steps[restore + 1:restore + 5])
            self.assertTrue(any(builder in later for builder in ("build.py", "ndk_arm64")), job)
        self.assertEqual(self.step("windows", "Wait for the bridge files"), self.step("android", "Wait for the bridge files"))
        wait = self.step("windows", "Wait for the bridge files")
        self.assertEqual(wait["env"], {"GH_TOKEN": "${{ github.token }}"})
        self.assertEqual(wait["shell"], "bash")
        self.assertLessEqual(wait["timeout-minutes"], self.wf["jobs"]["generate-bridge"]["timeout-minutes"])
        for needed in ('artifacts?name=bridge-artifact', 'endswith("Bridge codegen")', "exit 1", "sleep"):
            self.assertIn(needed, wait["run"])
        self.assertIn("generate-bridge", self.wf["jobs"]["ios"]["needs"])

    def wait_for_bridge(self, answers):
        """The wait step's script against a gh that gives <answers> in turn: (exit code, output)."""
        wait = self.step("windows", "Wait for the bridge files")
        with tempfile.TemporaryDirectory() as tmp:
            os.makedirs(os.path.join(tmp, "bin"))
            with open(os.path.join(tmp, "answers"), "w") as f:
                f.write("".join(line + "\n" for line in answers))
            gh = os.path.join(tmp, "bin", "gh")
            with open(gh, "w") as f:
                # each call takes the next line: "fail" fails, anything else is the --jq result
                f.write('#!/bin/sh\nn=$(cat "$T/n" 2>/dev/null || echo 0); n=$((n + 1)); echo "$n" > "$T/n"\n'
                        'line=$(sed -n "${n}p" "$T/answers")\n[ "$line" != fail ] || exit 1\necho "$line"\n')
            os.chmod(gh, 0o755)
            sleep = os.path.join(tmp, "bin", "sleep")
            with open(sleep, "w") as f:
                f.write("#!/bin/sh\n:\n")
            os.chmod(sleep, 0o755)
            done = subprocess.run(["bash", "--noprofile", "--norc", "-eo", "pipefail", "-c", wait["run"]], cwd=tmp,
                                  env={**os.environ, "PATH": os.path.join(tmp, "bin") + os.pathsep + os.environ["PATH"],
                                       "T": tmp, "GITHUB_REPOSITORY": "o/r", "GITHUB_RUN_ID": "1"},
                                  capture_output=True, text=True, timeout=60)
            return done.returncode, done.stdout + done.stderr

    def test_the_wait_ends_with_the_bridge_files_or_with_the_bridge_job(self):
        # pairs of answers: the artifact count, then the Bridge codegen job's status and conclusion
        self.assertEqual(self.wait_for_bridge(["0", "queued ", "0", "in_progress ", "1", "completed success"])[0], 0)
        code, out = self.wait_for_bridge(["0", "in_progress ", "0", "completed failure"])
        self.assertEqual(code, 1)
        self.assertIn("::error::Bridge codegen ended (completed failure)", out)
        self.assertEqual(self.wait_for_bridge(["0", "completed cancelled"])[0], 1)
        # a moment between the upload and the list is waited out, but not for ever
        self.assertEqual(self.wait_for_bridge(["0", "completed success", "1", "completed success"])[0], 0)
        code, out = self.wait_for_bridge(["0", "completed success"] * 4)
        self.assertEqual(code, 1)
        self.assertIn("no bridge-artifact", out)
        # the API failing now and then is waited out, ten times in a row is not
        self.assertEqual(self.wait_for_bridge(["fail", "fail", "0", "queued ", "1", "in_progress "])[0], 0)
        code, out = self.wait_for_bridge(["fail"] * 10)
        self.assertEqual(code, 1)
        self.assertIn("could not be read ten times", out)

    def test_every_app_job_writes_the_record_between_the_notices_and_the_upload(self):
        for job, (directory, uploads) in self.UPLOADS.items():
            at = self.index(job, self.RECORD)
            step = self.steps(job)[at]
            self.assertEqual(self.index(job, "Add license, source and third-party notices"), at - 1, job)
            for upload in uploads:
                self.assertGreater(self.index(job, upload), at, job)
                paths = self.step(job, upload)["with"]["path"].splitlines()
                self.assertIn(directory + "/release.json", paths, upload)
                self.assertIn(directory + "/SHA256SUMS", paths, upload)
            # always, in a check build too (which gets no release.json from the script)
            self.assertNotIn("if", step, job)
            self.assertNotIn("continue-on-error", step, job)
            self.assertEqual(step["env"]["CHECK_ONLY"], "${{ inputs.check_only }}", job)
            self.assertIn(f'scripts/release-info.py" --platform {job} --dir {os.path.basename(directory)} ', step["run"])
        # the Play bundle is the same Android build: one record of it, with the APK
        for name in ("Upload Play bundle",):
            self.assertNotIn("release.json", self.step("android", name)["with"]["path"])
        # 1.9.1: no Google Play job (the store run uploads the bundle, store-release.yml)
        self.assertNotIn("play", self.wf["jobs"])

    def test_the_bridge_files_are_kept_as_long_as_the_artifacts(self):
        """A job that "Re-run failed jobs" runs again restores the generated bridge of its run:
        kept for as long as the packages, which the release tool takes from a successful run
        only (docs/ci.md, "릴리스 기록")."""
        upload = self.step("generate-bridge", "Upload Artifact")["with"]
        self.assertEqual((upload["name"], upload["retention-days"]), ("bridge-artifact", "${{ env.ARTIFACT_RETENTION_DAYS }}"))
        for job in ("dart-tests", "windows", "android", "ios"):
            self.assertEqual(self.step(job, "Restore bridge files")["with"]["name"], "bridge-artifact", job)

    def test_the_bridge_files_of_a_commit_are_generated_once(self):
        """GHA-03: the first run of a commit saves the generated files under an exact key of the
        commit and the generator's tools; a later run takes them and uploads them all the same."""
        steps = self.steps("generate-bridge")
        restore = self.step("generate-bridge", "Restore the generated bridge")
        save = self.step("generate-bridge", "Save the generated bridge")
        self.assertTrue(restore["uses"].startswith("actions/cache/restore@"))
        self.assertTrue(save["uses"].startswith("actions/cache/save@"))
        self.assertNotIn("restore-keys", restore["with"])
        key = restore["with"]["key"]
        for part in ("needs.preflight.outputs.source_sha", "env.FLUTTER_RUST_BRIDGE_VERSION",
                     "env.CARGO_EXPAND_VERSION", "env.BRIDGE_FLUTTER_VERSION", "env.RUST_VERSION"):
            self.assertIn(part, key)
        self.assertEqual(save["with"]["key"], "${{ steps.bridge-out.outputs.cache-primary-key }}")
        upload = self.step("generate-bridge", "Upload Artifact")["with"]["path"]
        self.assertEqual(restore["with"]["path"], upload)
        self.assertEqual(save["with"]["path"], upload)
        # between the restore and the upload everything is skipped on a hit, and the upload is not
        first, last = self.index("generate-bridge", "Restore the generated bridge"), self.index("generate-bridge", "Upload Artifact")
        self.assertEqual(self.index("generate-bridge", "Check out hjviewer"), first - 1)
        for step in steps[first + 1:last]:
            self.assertEqual(step.get("if"), "steps.bridge-out.outputs.cache-hit != 'true'", step.get("name", step.get("uses")))
        self.assertNotIn("if", steps[last])
        self.assertEqual(last, len(steps) - 1)

    def test_artifact_names_and_retention_are_as_before(self):
        """The acceptance of the work package; tools/hjv-release counts on the 7 days."""
        self.assertEqual(self.wf["env"]["ARTIFACT_RETENTION_DAYS"], "${{ inputs.check_only && 1 || 7 }}")
        names = {
            ("windows", "Upload Windows build (unsigned)"): "${{ env.BUILD_NAME }}-windows-x86_64",
            ("android", "Upload signed APK"): "${{ env.BUILD_NAME }}-android-aarch64",
            ("android", "Upload debug-signed APK"): "${{ env.BUILD_NAME }}-android-aarch64-debugsigned",
            ("ios", "Upload IPA"): "${{ env.BUILD_NAME }}-ios${{ env.IOS_SIGNING != 'true' && '-unsigned' || '' }}",
        }
        for (job, name), artifact in names.items():
            step = self.step(job, name)["with"]
            self.assertEqual(step["name"], artifact)
            self.assertEqual(step["retention-days"], "${{ env.ARTIFACT_RETENTION_DAYS }}")
            self.assertEqual(step["if-no-files-found"], "error")

    def run_record_step(self, job, tree, env, cwd):
        """The step's script as the runner runs it (bash -eo pipefail), in <cwd>."""
        step = self.step(job, self.RECORD)
        clean = {k: v for k, v in os.environ.items() if k not in GITHUB_VARS}
        full = {**clean, **CALLED, "GITHUB_WORKSPACE": tree.root, "GITHUB_ACTIONS": "true",
                "GITHUB_STEP_SUMMARY": os.path.join(tree.root, "summary.md"), "CHECK_ONLY": "false", **env}
        done = subprocess.run(["bash", "--noprofile", "--norc", "-eo", "pipefail", "-c", step["run"]],
                              cwd=cwd, env=full, capture_output=True, text=True)
        return done.returncode, done.stdout + done.stderr

    def uploaded(self, job, upload, directory, substitutions):
        """The files of <directory> that the upload step's paths take."""
        patterns = [p.strip() for p in self.step(job, upload)["with"]["path"].splitlines() if p.strip()]
        for old, new in substitutions.items():
            patterns = [p.replace(old, new) for p in patterns]
        prefix = self.UPLOADS[job][0] + "/"
        self.assertTrue(all(p.startswith(prefix) for p in patterns), patterns)
        return sorted(n for n in os.listdir(directory) if any(fnmatch.fnmatchcase(prefix + n, p) for p in patterns))

    def listed(self, directory):
        with open(os.path.join(directory, "SHA256SUMS")) as f:
            return sorted(line.split("  ", 1)[1].rstrip("\n") for line in f)

    def workspace(self, job):
        """A tree with the job's upload directory under upstream/rustdesk, where its steps run."""
        tree = Tree(self)
        cwd = os.path.join(tree.root, "upstream", "rustdesk")
        out = os.path.join(tree.root, *self.UPLOADS[job][0].split("/"))
        os.makedirs(out)
        for name in NOTICES:
            tree.put(name, b"a notice\n", out)
        return tree, cwd, out

    def test_the_windows_step(self):
        for ending in ("\n", "\r\n"):  # upstream is checked out with CRLF on the Windows runner
            tree, cwd, out = self.workspace("windows")
            os.makedirs(os.path.join(cwd, "flutter"))
            pubspec = ["name: flutter_hbb", "description: HJViewer", "version: 1.6.0+41", "dependencies:",
                       "  some_package:", "    version: 2.0.0+3", ""]
            tree.put("pubspec.yaml", ending.join(pubspec).encode(), os.path.join(cwd, "flutter"))
            packer = b"MZ" + os.urandom(500)
            tree.put("HJViewer.exe", packer, out)
            tree.put("HJViewer-install.exe", packer, out)
            code, text = self.run_record_step("windows", tree, {}, cwd)
            self.assertEqual(code, 0, text)
            with open(os.path.join(out, "release.json")) as f:
                record = json.load(f)
            self.assertEqual((record["build"], record["file"], record["source_sha"]),
                             ("1.6.0.41", "HJViewer.exe", tree.commit))
            # the hashes cover what is uploaded, and nothing else
            self.assertEqual(self.uploaded("windows", "Upload Windows build (unsigned)", out, {}),
                             sorted(self.listed(out) + ["SHA256SUMS"]))
        # a check build: the same step, no release.json
        os.remove(os.path.join(out, "release.json"))
        code, text = self.run_record_step("windows", tree, {"CHECK_ONLY": "true"}, cwd)
        self.assertEqual(code, 0, text)
        self.assertFalse(os.path.exists(os.path.join(out, "release.json")))
        self.assertNotIn("release.json", self.listed(out))
        # a pubspec without a build number fails the step, and so the job
        tree.put("pubspec.yaml", b"version: 1.6.0\n", os.path.join(cwd, "flutter"))
        code, text = self.run_record_step("windows", tree, {}, cwd)
        self.assertEqual(code, 1, text)
        self.assertIn("::error::", text)

    def test_what_the_record_steps_read_is_there(self):
        """The variables the three steps' scripts read, which the tests below give them, come
        from where the workflow sets them."""
        self.assertEqual(self.step("android", self.RECORD)["env"]["SIGNED_APK"],
                         "${{ steps.sign-apk.outputs.signedReleaseFile }}")
        self.assertEqual(self.step("android", "Sign app APK")["id"], "sign-apk")
        self.assertIn("ANDROID_SIGNING", self.wf["jobs"]["android"]["env"])
        self.assertIn("IOS_SIGNING", self.wf["jobs"]["ios"]["env"])
        for job in ("android", "ios"):
            fetch = self.step(job, "Fetch upstream and apply hjviewer patches")["run"]
            self.assertIn('echo "BUILD_NAME=', fetch)
            self.assertLess(self.index(job, "Fetch upstream and apply hjviewer patches"), self.index(job, self.RECORD))
        self.assertLess(self.index("android", "Sign app APK"), self.index("android", self.RECORD))
        self.assertLess(self.index("ios", "Set up code signing"), self.index("ios", self.RECORD))

    def test_the_android_step(self):
        build = "HJViewer-1.6.0-0123abc"
        for signing in ("true", "false"):
            tree, cwd, out = self.workspace("android")
            # what the build and the signing step leave there
            tree.put(f"{build}-android-aarch64.apk", apk(), out)
            env = {"BUILD_NAME": build, "ANDROID_SIGNING": signing, "SIGNED_APK": ""}
            want, upload, artifact = f"{build}-android-aarch64.apk", "Upload debug-signed APK", {}
            if signing == "true":
                want, upload = f"{build}-android-aarch64-signed.apk", "Upload signed APK"
                tree.put(f"{build}-android-aarch64-aligned.apk", apk(), out)
                tree.put(want, apk() + b"signed", out)
                tree.put(want + ".idsig", b"apksigner's v4 signature", out)
                env["SIGNED_APK"] = f"upstream/rustdesk/signed-apk/{want}"
                artifact = {"${{ steps.sign-apk.outputs.signedReleaseFile }}": env["SIGNED_APK"]}
            artifact["${{ env.BUILD_NAME }}"] = build
            code, text = self.run_record_step("android", tree, env, cwd)
            self.assertEqual(code, 0, text)
            self.assertEqual(self.uploaded("android", upload, out, artifact), sorted(self.listed(out) + ["SHA256SUMS"]))
            self.assertFalse([n for n in self.listed(out) if n.endswith((".idsig", "-aligned.apk"))])
            self.assertEqual(self.listed(out)[0], want)
            if signing == "true":
                with open(os.path.join(out, "release.json")) as f:
                    record = json.load(f)
                self.assertEqual((record["platform"], record["file"], record["build"]), ("android", want, "262872251"))
            else:
                # a debug-signed APK is no release
                self.assertFalse(os.path.exists(os.path.join(out, "release.json")))
                self.assertIn("not for release (debug-signed)", text)
        # a signed build whose signing step named no file fails here
        tree, cwd, out = self.workspace("android")
        tree.put(f"{build}-android-aarch64.apk", apk(), out)
        code, text = self.run_record_step("android", tree, {"BUILD_NAME": build, "ANDROID_SIGNING": "true", "SIGNED_APK": ""}, cwd)
        self.assertEqual(code, 1, text)

    # Stand-ins for build-tools' zipalign and apksigner: each call goes to $T/calls; apksigner sign
    # notes the keystore's mode and bytes and writes the signed APK with its .idsig, as apksigner
    # does; verify --print-certs prints a certificate's name, which the step must not show
    ZIPALIGN = '#!/bin/bash\necho "zipalign $*" >> "$T/calls"\n[ -z "${MISALIGNED:-}" ] || ' \
               '{ echo "Verification FAILED"; exit 1; }\necho "Verification succesful"\n'
    APKSIGNER = '''#!/bin/bash
echo "apksigner $*" >> "$T/calls"
[ "$ANDROID_KEYSTORE_PASSWORD" = store-secret ] && [ "$HJV_KEY_PASS" = key-secret ] || exit 9
if [ "$1" = sign ]; then
  ks= out=
  while [ $# -gt 1 ]; do
    case $1 in --ks) ks=$2 ;; --out) out=$2 ;; esac
    shift
  done
  stat -c '%a' "$ks" >> "$T/keystore"
  cat "$ks" >> "$T/keystore"
  [ -z "${SIGN_FAILS:-}" ] || exit 1
  { cat "$1"; echo signed; } > "$out"
  echo v4 > "$out.idsig"
elif [ "$2" = --verbose ]; then
  echo "Verifies"
  echo "Verified using v2 scheme (APK Signature Scheme v2): true"
  echo "Number of signers: 1"
  echo "Signer #1 certificate DN: CN=Somebody Real, O=Somewhere"
  echo "Signer #1 certificate SHA-256 digest: 00ff"
  echo "WARNING: a warning that names nothing" >&2
fi
'''

    def sign_apk(self, apks, **env):
        """"Sign app APK" as the runner runs it, against the stand-ins: (exit code, output, the
        step's outputs, its GITHUB_ENV, the calls, what apksigner saw of the keystore, the
        keystore's path, the workspace's signed-apk folder)."""
        step = self.step("android", "Sign app APK")
        tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, tmp)
        tools = os.path.join(tmp, "sdk", "build-tools", "36.1.0")
        os.makedirs(tools)
        for name, text in (("zipalign", self.ZIPALIGN), ("apksigner", self.APKSIGNER)):
            with open(os.path.join(tools, name), "w") as f:
                f.write(text)
            os.chmod(os.path.join(tools, name), 0o755)
        work, temp = os.path.join(tmp, "work"), os.path.join(tmp, "temp")
        folder = os.path.join(work, "upstream", "rustdesk", "signed-apk")
        os.makedirs(folder)
        os.makedirs(temp)
        for name in apks:
            with open(os.path.join(folder, name), "wb") as f:
                f.write(b"an APK " + name.encode())
        files = {name: os.path.join(tmp, name) for name in ("output", "env", "calls", "keystore")}
        for path in files.values():
            open(path, "w").close()
        secrets = {"KEYSTORE_BASE64": "dGhlIGtleXN0b3Jl", "ANDROID_KEYSTORE_PASSWORD": "store-secret",
                   "ANDROID_KEY_ALIAS": "the alias", "HJV_KEY_PASS": "key-secret"}
        self.assertEqual(sorted(step["env"]), sorted(secrets))
        done = subprocess.run(["bash", "--noprofile", "--norc", "-eo", "pipefail", "-c", step["run"]], cwd=work,
                              env={**os.environ, **secrets, "ANDROID_HOME": os.path.join(tmp, "sdk"),
                                   "ANDROID_SIGN_TOOL_VERSION": "36.1.0", "RUNNER_TEMP": temp, "T": tmp,
                                   "GITHUB_OUTPUT": files["output"], "GITHUB_ENV": files["env"], **env},
                              capture_output=True, text=True, timeout=60)
        read = {}
        for name, path in files.items():
            with open(path) as f:
                read[name] = f.read()
        return (done.returncode, done.stdout + done.stderr, read["output"], read["env"], read["calls"],
                read["keystore"], os.path.join(temp, "apk-release.keystore"), folder)

    def test_the_apk_signing_step(self):
        """What r0adkll/sign-android-release ran at 349ebde, without it (node12, set-output): the
        same tools and flags, the passwords by name only, the keystore outside the workspace and
        gone after the step, and the output its four readers take."""
        step = self.step("android", "Sign app APK")
        self.assertNotIn("uses", step)
        self.assertEqual((step["if"], step["shell"], step["working-directory"]),
                         ("env.ANDROID_SIGNING == 'true'", "bash", "${{ github.workspace }}"))
        self.assertEqual(step["env"]["HJV_KEY_PASS"],
                         "${{ secrets.ANDROID_KEY_PASSWORD || secrets.ANDROID_KEYSTORE_PASSWORD }}")
        self.assertNotIn("${{", step["run"])
        self.assertNotIn("r0adkll", json.dumps(self.wf["jobs"]))
        name = "HJViewer-1.9.2-0123abc-android-aarch64"
        signed = f"upstream/rustdesk/signed-apk/{name}-signed.apk"
        code, out, output, env, calls, keystore, ks, folder = self.sign_apk([name + ".apk"])
        self.assertEqual(code, 0, out)
        # the path as the action wrote it, relative to the workspace, for the output's readers
        self.assertEqual(output, f"signedReleaseFile={signed}\n")
        self.assertEqual(env, f"SIGNED_RELEASE_FILE={signed}\n")
        apk_path = f"upstream/rustdesk/signed-apk/{name}.apk"
        self.assertEqual(calls.splitlines(), [
            f"zipalign -c -v 4 {apk_path}",
            f"apksigner sign --ks {ks} --ks-key-alias the alias --ks-pass env:ANDROID_KEYSTORE_PASSWORD "
            f"--out {signed} --key-pass env:HJV_KEY_PASS upstream/rustdesk/signed-apk/{name}-aligned.apk",
            f"apksigner verify {signed}",
            f"apksigner verify --verbose --print-certs {signed}"])
        # the keystore: decoded, mode 600, outside the workspace, and gone after the step
        self.assertEqual(keystore, "600\nthe keystore")
        self.assertFalse(os.path.exists(ks))
        self.assertEqual(sorted(os.listdir(folder)),
                         sorted([name + ".apk", name + "-aligned.apk", name + "-signed.apk", name + "-signed.apk.idsig"]))
        with open(os.path.join(folder, name + "-aligned.apk"), "rb") as f:
            self.assertEqual(f.read(), b"an APK " + name.encode() + b".apk")
        # the schemes and the digest of the certificate, never its name, warnings or a password
        self.assertIn("Signer #1 certificate SHA-256 digest: 00ff", out)
        self.assertIn("Verified using v2 scheme (APK Signature Scheme v2): true", out)
        for hidden in ("Somebody Real", "DN:", "WARNING", "store-secret", "key-secret"):
            self.assertNotIn(hidden, out + calls)
        # an APK that is not aligned is not signed, and no keystore is written for it
        code, out, output, env, calls, keystore, ks, _ = self.sign_apk([name + ".apk"], MISALIGNED="1")
        self.assertEqual((code, output, env, keystore), (1, "", "", ""))
        self.assertIn("::error::zipalign -c -v 4", out)
        self.assertNotIn("apksigner", calls)
        # a failed signing leaves no keystore and no output
        code, out, output, env, calls, keystore, ks, _ = self.sign_apk([name + ".apk"], SIGN_FAILS="1")
        self.assertNotEqual(code, 0)
        self.assertEqual((output, env, keystore), ("", "", "600\nthe keystore"))
        self.assertFalse(os.path.exists(ks))
        # one APK, the build's: none or two are a mistake of the build
        for apks in ([], [name + ".apk", "other.apk"]):
            code, out, output, *_ = self.sign_apk(apks)
            self.assertEqual((code, output), (1, ""), apks)
            self.assertIn("::error::upstream/rustdesk/signed-apk holds", out)
        # the next step removes the keystore also when this one failed or the run was cancelled
        steps = [s.get("name") for s in self.steps("android")]
        remove = self.step("android", "Remove the APK keystore")
        self.assertEqual(steps.index("Remove the APK keystore"), steps.index("Sign app APK") + 1)
        self.assertEqual(remove["if"], "always() && env.ANDROID_SIGNING == 'true'")
        self.assertEqual(remove["run"], 'rm -f "$RUNNER_TEMP/apk-release.keystore"')
        self.assertIn('ks="$RUNNER_TEMP/apk-release.keystore"', step["run"])
        # the build-tools of the step that also gives "Sign and check Play bundle" its aapt2
        self.assertLess(steps.index("Setup sign tool version variable"), steps.index("Sign app APK"))
        self.assertIn('tools="$ANDROID_HOME/build-tools/$ANDROID_SIGN_TOOL_VERSION"', step["run"])

    def test_the_ios_step(self):
        for name, signing in (("HJViewer-1.6.0-0123abc-ios.ipa", "true"), ("HJViewer-1.6.0-0123abc-ios-unsigned.ipa", "false")):
            tree, cwd, out = self.workspace("ios")
            tree.put(name, ipa(), out)
            code, text = self.run_record_step("ios", tree, {"IOS_SIGNING": signing}, cwd)
            self.assertEqual(code, 0, text)
            self.assertEqual(self.uploaded("ios", "Upload IPA", out, {}), sorted(self.listed(out) + ["SHA256SUMS"]))
            if signing == "true":
                with open(os.path.join(out, "release.json")) as f:
                    record = json.load(f)
                self.assertEqual((record["platform"], record["file"], record["build"]), ("ios", name, "262870101"))
            else:
                # an unsigned IPA is no release
                self.assertFalse(os.path.exists(os.path.join(out, "release.json")))
                self.assertIn("not for release (unsigned)", text)
        # no IPA, or two: the upload would take what nobody checked
        for names in ((), ("a.ipa", "b.ipa")):
            tree, cwd, out = self.workspace("ios")
            for name in names:
                tree.put(name, ipa(), out)
            code, text = self.run_record_step("ios", tree, {"IOS_SIGNING": "true"}, cwd)
            self.assertEqual(code, 1, text)
            self.assertIn("::error::", text)
            self.assertFalse(os.path.exists(os.path.join(out, "release.json")))

    def test_the_ios_app_is_told_how_it_is_handed_out(self):
        """Only an App Store Connect build updates through TestFlight (design 5-5)."""
        signed = self.step("ios", "Build rustdesk (signed IPA)")["run"]
        self.assertIn('--dart-define=HJV_IOS_EXPORT="$IOS_EXPORT_METHOD"', signed)
        self.assertEqual(signed.count("flutter build ipa"), 1)
        # the value is the one the signing step chose from the profile; the TestFlight upload, a
        # later run's, reads the profile in the IPA by the same rules
        signing = self.step("ios", "Set up code signing")["run"]
        self.assertIn('echo "IOS_EXPORT_METHOD=$method" >> "$GITHUB_ENV"', signing)
        upload = self.step("testflight", "Upload to TestFlight")["run"]
        for rule in ("ProvisionsAllDevices", "Entitlements:get-task-allow", "ProvisionedDevices"):
            self.assertIn(rule, signing)
            self.assertIn(rule, upload)
        self.assertLess(self.index("ios", "Set up code signing"), self.index("ios", "Build rustdesk (signed IPA)"))
        # an unsigned build is exported by nobody: the app sees no method
        self.assertNotIn("HJV_IOS_EXPORT", self.step("ios", "Build rustdesk (unsigned)")["run"])

    def upload_check(self, out, env):
        """The release.json check of the TestFlight upload, as the step runs it in the downloaded
        artifact <out>: (exit code, output)."""
        run = self.step("testflight", "Upload to TestFlight")["run"]
        code = re.search(r"python3 -I -c '\n(.*?)\n' \"\$ipa\"", run, re.S).group(1)
        ipa_name = next(n for n in os.listdir(out) if n.endswith(".ipa"))
        done = subprocess.run([sys.executable, "-I", "-c", code, ipa_name], cwd=out, capture_output=True, text=True,
                              env={**os.environ, **env})
        return done.returncode, done.stdout + done.stderr

    def test_the_upload_takes_the_ipa_its_build_run_recorded(self):
        """What the iOS job writes (release-info.py) is what the later run's upload checks."""
        tree = Tree(self)
        name = "HJViewer-1.6.0-0123abc-ios.ipa"
        tree.put(name, ipa())
        code, text, _ = tree.run("--platform", "ios", "--file", name)
        self.assertEqual(code, 0, text)
        good = {"VERSION": "1.6.0", "SOURCE_SHA": tree.commit, "TESTFLIGHT_RUN": CALLED["GITHUB_RUN_ID"]}
        self.assertEqual(self.upload_check(tree.out, good), (0, ""))
        for change, why in (({"VERSION": "1.6.1"}, "version"), ({"SOURCE_SHA": "3" * 40}, "source_sha"),
                            ({"TESTFLIGHT_RUN": "4243"}, "run_id")):
            code, out = self.upload_check(tree.out, {**good, **change})
            self.assertEqual(code, 1, change)
            self.assertIn(f"::error::release.json does not name this IPA of build run", out)
            self.assertIn(why, out)
        # other bytes under the build's release.json
        tree.put(name, ipa() + b"!")
        code, out = self.upload_check(tree.out, good)
        self.assertEqual(code, 1)
        self.assertIn("sha256", out)

    def required_dart_tests(self):
        """The test files the Dart tests job fails without."""
        run = self.step("dart-tests", "Run the tests")["run"]
        return re.search(r"for t in (.*?); do", run, re.S).group(1).replace("\\\n", " ").split(), run

    def test_the_update_screens_dart_tests_must_be_in_the_tree(self):
        listed, run = self.required_dart_tests()
        for test in ("test/hjv_update_test.dart", "test/hjv_update_gate_test.dart"):
            self.assertIn(test, listed)
        for earlier in ("test/soft_keyboard_input_test.dart", "test/common/widgets/gestures_test.dart",
                        "test/hjviewer_account_test.dart"):
            self.assertIn(earlier, listed)
        self.assertEqual(len(listed), len(set(listed)))
        # the whole folder is run, so the list only says what must not be missing
        self.assertRegex(run, r"(?m)^flutter test$")

    def test_the_required_dart_tests_are_in_the_patched_tree(self):
        """HJV_PATCHED_FLUTTER=<the flutter directory of the tree with client/patches applied>:
        a test the job requires and no patch brings fails here, not in a run on GitHub."""
        flutter = os.environ.get("HJV_PATCHED_FLUTTER")
        if not flutter:
            self.skipTest("HJV_PATCHED_FLUTTER is not set")
        listed, _ = self.required_dart_tests()
        self.assertEqual([t for t in listed if not os.path.isfile(os.path.join(flutter, t))], [])

    def test_the_windows_smoke_tests(self):
        """Text only: what the steps do on a Windows runner is a device test (docs/ci.md)."""
        names = [s.get("name") for s in self.steps("windows")]
        install, update, guard = (names.index(n) for n in ("Install smoke test", "Update smoke test",
                                                           "Packer guard smoke test"))
        self.assertEqual((update, guard), (install + 1, install + 2))
        self.assertEqual(guard, len(names) - 1)
        self.assertGreater(install, names.index("Upload Windows build (unsigned)"))
        self.assertEqual(self.steps("windows")[install]["id"], "install-smoke")
        for at in (install, update, guard):
            step = self.steps("windows")[at]
            # advisory for now, never without the server values, never for long, and in pwsh
            self.assertIs(step["continue-on-error"], True)
            self.assertIn("env.HJV_SERVER != ''", step["if"])
            self.assertLessEqual(step["timeout-minutes"], 12)
            self.assertEqual(step["shell"], "pwsh")
            self.assertIn("exit 1", step["run"])
        update, guard = self.steps("windows")[update], self.steps("windows")[guard]
        # the update needs the install; the guard does not
        self.assertEqual(update["if"], "env.HJV_SERVER != '' && steps.install-smoke.outcome == 'success'")
        self.assertEqual(guard["if"], "env.HJV_SERVER != ''")
        for needed in (
            '"127.0.0.1 $env:HJV_SERVER"',          # the runner never reaches the real server
            "'NT AUTHORITY\\SYSTEM'", "-Argument '--update'", 'HJViewer-$version.exe',
            "Unregister-ScheduledTask", "'Update completed\\.'", "DisplayVersion", "LastWriteTimeUtc",
            "Name='HJViewer'",
        ):
            self.assertIn(needed, update["run"], needed)
        for needed in ("$env:LOCALAPPDATA 'hjviewer'", "[System.IO.FileShare]::Read", "-ArgumentList '--update'",
                       "$p.ExitCode -ne 32", "$hold.Dispose()"):
            self.assertIn(needed, guard["run"], needed)
        for step in (update, guard):
            # --silent-install reinstalls; nothing here shows a window that waits for a person
            self.assertNotIn("--silent-install", step["run"])
            self.assertNotIn("-Wait", step["run"])


if __name__ == "__main__":
    unittest.main()

#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Writes release.json and SHA256SUMS beside a built app, in the directory a job of
client-build.yml uploads as its artifact (minimum version design 4-5 and 6-4; docs/ci.md,
"업데이트 키와 릴리스 기록").

Every platform job runs it once the package and its notice files are in place:

    python3 -I scripts/release-info.py --platform windows|android|ios --dir <directory>
        --file <package> [--also <file>]... [--build <build number>] [--unreleased <why>]

release.json is what the build says about its package:

    {"platform": "windows", "version": "1.6.0", "build": "1.6.0.41", "file": "HJViewer.exe",
     "size": 25329664, "sha256": "<64 hex digits>", "source_sha": "<40 hex digits>",
     "run_id": "38106290530", "workflow": ".github/workflows/build.yml"}

    version     client/version.env. An APK's versionName and an IPA's CFBundleShortVersionString
                must say the same.
    build       Windows: --build, the file version <version>.<run number> that
                scripts/set-client-version.sh gave the app. Android: the APK's versionCode.
                iOS: the IPA's CFBundleVersion. Both are read from the package itself.
    file, size, sha256
                the package in <directory>
    source_sha  the commit this checkout is at, the source that was built
    run_id      GITHUB_RUN_ID: the run, the build repository's when it called the workflow
    workflow    the file of the workflow that run belongs to, from GITHUB_WORKFLOW_REF: the
                build repository's build.yml when it called the workflow

tools/hjv-release reads it before a version policy may name the package, and believes none of
it until it has found the package and this very file among the artifacts of the run it names
(its README, "출처 확인"). So the file must be uploaded with the package, unchanged.

SHA256SUMS lists, as sha256sum writes them, what the job uploads from <directory>: the package,
the files given with --also, the notice files (*.txt) and release.json.
`sha256sum -c SHA256SUMS` checks a download. The package's lines also go to the run summary
($GITHUB_STEP_SUMMARY): the person who signs a policy compares the hash there.

A build that is not for release gets no release.json, and one that is there is removed: a check
build (CHECK_ONLY=true), which may lack the public keys a release needs and is never handed out,
and a package that no user can install over the app they have (--unreleased: a debug-signed APK,
an unsigned IPA). Without that file hjv-release takes no directory for a release. SHA256SUMS and
the summary are written all the same, with every check of a release, so that such builds run
this step too.

Standard library only, for the Python of the Windows, Linux and macOS runners (3.8 or later).
"""
import argparse
import hashlib
import json
import os
import plistlib
import re
import struct
import subprocess
import sys
import zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RELEASE_JSON = "release.json"
SHA256SUMS = "SHA256SUMS"
EXTENSIONS = {"windows": ".exe", "android": ".apk", "ios": ".ipa"}
# a file name as tools/hjv-release takes it: no directory, no leading dot
_NAME = re.compile(r"[A-Za-z0-9_-][A-Za-z0-9._-]{0,199}")
# a version as tools/hjv-release takes it (src/version.rs): the form on which the apps' version
# numbers and plain comparison agree
_VERSION = re.compile(r"(0|[1-9][0-9]{0,2})\.(0|[1-9][0-9]{0,2})\.(0|[1-9][0-9]?)")


class Problem(Exception):
    """Why no record can be written; the step fails with this message."""


# --- what a package says about itself ---

# Android's binary XML (frameworks/base ResourceTypes.h)
_RES_STRING_POOL = 0x0001
_RES_XML = 0x0003
_RES_XML_START_ELEMENT = 0x0102
_RES_XML_RESOURCE_MAP = 0x0180
_UTF8_FLAG = 0x100
_ATTR_VERSION_CODE = 0x0101021B  # android:versionCode
_ATTR_VERSION_NAME = 0x0101021C  # android:versionName
_TYPE_STRING = 0x03
_TYPE_INT_DEC = 0x10
_TYPE_INT_HEX = 0x11


def _pool_string(data, pool, index):
    """String <index> of the string pool chunk at <pool>."""
    header = struct.unpack_from("<H", data, pool + 2)[0]
    count, _, flags, strings_start, _ = struct.unpack_from("<IIIII", data, pool + 8)
    if index >= count:
        raise ValueError("no such string")
    at = pool + strings_start + struct.unpack_from("<I", data, pool + header + 4 * index)[0]
    if flags & _UTF8_FLAG:
        for _ in range(2):  # the length in characters, then the length in bytes
            two = data[at] & 0x80
            size = ((data[at] & 0x7F) << 8 | data[at + 1]) if two else data[at]
            at += 2 if two else 1
        return data[at:at + size].decode("utf-8")
    size = struct.unpack_from("<H", data, at)[0]
    at += 2
    if size & 0x8000:
        size = (size & 0x7FFF) << 16 | struct.unpack_from("<H", data, at)[0]
        at += 2
    return data[at:at + 2 * size].decode("utf-16-le")


def manifest_version(data):
    """(versionCode, versionName) of a binary AndroidManifest.xml: the two attributes of its
    <manifest> element, found by their resource IDs."""
    try:
        kind, header, size = struct.unpack_from("<HHI", data, 0)
        if kind != _RES_XML or header < 8 or size > len(data):
            raise ValueError("not binary XML")
        pool, ids, at = None, (), header
        while at + 8 <= size:
            kind, header, chunk = struct.unpack_from("<HHI", data, at)
            if header < 8 or chunk < header or at + chunk > size:
                raise ValueError("a chunk past the end")
            if kind == _RES_STRING_POOL and pool is None:
                pool = at
            elif kind == _RES_XML_RESOURCE_MAP:
                ids = struct.unpack_from("<%dI" % ((chunk - header) // 4), data, at + header)
            elif kind == _RES_XML_START_ELEMENT:
                # the first element; its attributes follow the node's and the element's headers
                body = at + header
                _, name, start, each, count = struct.unpack_from("<IIHHH", data, body)
                if pool is None or _pool_string(data, pool, name) != "manifest" or each < 20:
                    raise ValueError("no <manifest> element")
                code = text = None
                for i in range(count):
                    # namespace, name, raw value; then the typed value: size, 0, type, data
                    _, attr, _, _, _, typed, value = struct.unpack_from("<IIIHBBI", data, body + start + i * each)
                    resource = ids[attr] if attr < len(ids) else None
                    if resource == _ATTR_VERSION_CODE and typed in (_TYPE_INT_DEC, _TYPE_INT_HEX):
                        code = value
                    elif resource == _ATTR_VERSION_NAME and typed == _TYPE_STRING:
                        text = _pool_string(data, pool, value)
                if code is None or text is None:
                    raise ValueError("no versionCode or versionName")
                return code, text
            at += chunk
        raise ValueError("no element")
    except (struct.error, IndexError, ValueError, UnicodeDecodeError) as e:
        raise Problem(f"the APK's AndroidManifest.xml cannot be read ({e})")


def apk_version(path):
    """(versionCode, versionName) of an APK."""
    try:
        with zipfile.ZipFile(path) as z:
            data = z.read("AndroidManifest.xml")
    except Exception as e:  # zipfile and zlib have errors of their own
        raise Problem(f"{os.path.basename(path)} is not an APK ({e})")
    return manifest_version(data)


def ipa_version(path):
    """(CFBundleVersion, CFBundleShortVersionString) of an IPA's app."""
    try:
        with zipfile.ZipFile(path) as z:
            names = [n for n in z.namelist() if re.fullmatch(r"Payload/[^/]+\.app/Info\.plist", n)]
            if len(names) != 1:
                raise Problem(f"{os.path.basename(path)} has {len(names)} apps under Payload/, not one")
            info = plistlib.loads(z.read(names[0]))
        build, version = info.get("CFBundleVersion"), info.get("CFBundleShortVersionString")
    except Problem:
        raise
    except Exception as e:  # zipfile, zlib, plistlib and expat have errors of their own
        raise Problem(f"{os.path.basename(path)} is not an IPA ({e})")
    if not isinstance(build, str) or not isinstance(version, str):
        raise Problem(f"{os.path.basename(path)}: Info.plist has no CFBundleVersion or CFBundleShortVersionString")
    return build, version


# --- the record ---


def read_version(path):
    try:
        with open(path, encoding="utf-8") as f:
            found = [m.group(1) for m in (re.fullmatch(r"\s*HJV_VERSION=(\S+)\s*", line) for line in f) if m]
    except OSError as e:
        raise Problem(f"{path} cannot be read ({e.strerror})")
    if len(found) != 1 or not _VERSION.fullmatch(found[0]):
        raise Problem(
            f"{path} must have one HJV_VERSION=<x.y.z> line: three numbers without leading zeros, "
            "the first two below 1000 and the last below 100 (hjv-release takes no other version)")
    return found[0]


def source_commit():
    """The commit of this checkout: the source the job built."""
    try:
        done = subprocess.run(["git", "-C", ROOT, "rev-parse", "HEAD"], capture_output=True, check=True)
    except (OSError, subprocess.CalledProcessError):
        raise Problem(f"git cannot name the commit of {ROOT}")
    commit = done.stdout.decode("ascii", "replace").strip()
    if not re.fullmatch(r"[0-9a-f]{40}", commit):
        raise Problem("git rev-parse HEAD did not print a 40-digit commit")
    return commit


def run_of_the_build():
    """(run_id, workflow, note): the run this step is part of and its workflow's file. In a
    workflow that another one calls, GitHub's variables name the caller's run and the caller's
    file. A GITHUB_WORKFLOW_REF of another form than <repository>/<file>@<ref> is written as it
    is, with a note: what the record says is for tools/hjv-release to judge, and no reason to
    lose the build."""
    run_id = os.environ.get("GITHUB_RUN_ID", "")
    if not re.fullmatch(r"[0-9]{1,20}", run_id):
        raise Problem("GITHUB_RUN_ID is not a run's number: release.json names the run that built the "
                      "package, so this is a step of client-build.yml")
    repository = os.environ.get("GITHUB_REPOSITORY", "")
    ref = os.environ.get("GITHUB_WORKFLOW_REF", "")
    if not ref or len(ref) > 300 or not re.fullmatch(r"[\x21-\x7e]+", ref):
        raise Problem("GITHUB_WORKFLOW_REF does not name the run's workflow: this is a step of client-build.yml")
    # <owner>/<repository>/.github/workflows/<file>@<ref>
    path = ref[len(repository) + 1:].split("@", 1)[0] if repository and ref.startswith(repository + "/") else ""
    if re.fullmatch(r"\.github/workflows/[A-Za-z0-9._-]{1,100}\.ya?ml", path):
        return run_id, path, None
    return run_id, ref, ("GITHUB_WORKFLOW_REF is not a workflow file of GITHUB_REPOSITORY: release.json's "
                         "workflow is its value as it is, which hjv-release may not take")


def file_in(directory, name, what):
    """The path of a plain file of <directory> that the job uploads."""
    if not _NAME.fullmatch(name or ""):
        raise Problem(f"{what} must be a file name of letters, digits, '.', '_' and '-', not starting "
                      f"with a dot: {name!r}")
    path = os.path.join(directory, name)
    if os.path.islink(path) or not os.path.isfile(path):
        raise Problem(f"{name} is not a file in {directory}")
    return path


def digest(path):
    """(size, SHA-256 in lowercase hex) of a file."""
    h, size = hashlib.sha256(), 0
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
            size += len(block)
    return size, h.hexdigest()


def make_record(platform, directory, name, build, version):
    """(release.json's fields in the order they are written, a note for the log or None)."""
    if not name.lower().endswith(EXTENSIONS[platform]):
        raise Problem(f"a {platform} package ends in {EXTENSIONS[platform]}: {name!r}")
    path = file_in(directory, name, "--file")
    if platform == "windows":
        if not build or not re.fullmatch(re.escape(version) + r"\.(0|[1-9][0-9]{0,4})", build) \
                or int(build.rsplit(".", 1)[1]) > 65535:
            raise Problem(f"--build must be the Windows file version {version}.<0-65535>: {build!r}")
    else:
        if platform == "android":
            code, in_package = apk_version(path)
            found = str(code)
            if not 0 < code <= 2100000000:
                raise Problem(f"the APK's versionCode {code} is not one Google Play takes (1-2100000000)")
        else:
            found, in_package = ipa_version(path)
            if not re.fullmatch(r"[0-9]{1,18}", found):
                raise Problem(f"the IPA's CFBundleVersion is not a number: {found!r}")
        if in_package != version:
            raise Problem(f"{name} says it is version {in_package!r}, client/version.env says {version}")
        if build and build != found:
            raise Problem(f"{name} carries build number {found}, not {build}")
        build = found
    size, sha256 = digest(path)
    if size == 0:
        raise Problem(f"{name} is empty")
    run_id, workflow, note = run_of_the_build()
    return {
        "platform": platform,
        "version": version,
        "build": build,
        "file": name,
        "size": size,
        "sha256": sha256,
        "source_sha": source_commit(),
        "run_id": run_id,
        "workflow": workflow,
    }, note


def write(path, text):
    # LF on every platform: tools/hjv-release looks for these very bytes
    with open(path, "w", encoding="ascii", newline="\n") as f:
        f.write(text)


def main(argv):
    parser = argparse.ArgumentParser(description="Writes release.json and SHA256SUMS beside a built app.")
    parser.add_argument("--platform", required=True, choices=sorted(EXTENSIONS))
    parser.add_argument("--dir", required=True, help="the directory the job uploads")
    parser.add_argument("--file", required=True, help="the package's file name in that directory")
    parser.add_argument("--also", action="append", default=[], metavar="FILE",
                        help="another file of the directory that the job uploads")
    parser.add_argument("--build", default="", help="the build number (required for windows)")
    parser.add_argument("--unreleased", default="", metavar="WHY",
                        help="the package is not for release (e.g. debug-signed): no release.json")
    parser.add_argument("--version-file", default=os.path.join(ROOT, "client", "version.env"))
    args = parser.parse_args(argv)
    check_only = os.environ.get("CHECK_ONLY", "").strip() == "true"
    unreleased = "check build" if check_only else args.unreleased.strip()
    actions = os.environ.get("GITHUB_ACTIONS") == "true"
    try:
        if unreleased and not re.fullmatch(r"[a-z][a-z -]{0,39}", unreleased):
            raise Problem("--unreleased takes a few plain words, such as debug-signed")
        if not os.path.isdir(args.dir):
            raise Problem(f"{args.dir} is not a directory")
        record, note = make_record(args.platform, args.dir, args.file, args.build, read_version(args.version_file))
        listed = [args.file]
        for name in args.also:
            file_in(args.dir, name, "--also")
            if name in listed or name in (RELEASE_JSON, SHA256SUMS):
                raise Problem(f"--also {name}: named twice, or a file this script writes")
            listed.append(name)
        handed_out = len(listed)
        # the notice files, as the upload's *.txt takes them
        listed += sorted(n for n in os.listdir(args.dir)
                         if n.endswith(".txt") and n not in listed and _NAME.fullmatch(n)
                         and os.path.isfile(os.path.join(args.dir, n)) and not os.path.islink(os.path.join(args.dir, n)))
        text = json.dumps(record) + "\n"
        target = os.path.join(args.dir, RELEASE_JSON)
        if unreleased:
            if os.path.lexists(target):
                os.remove(target)
        else:
            write(target, text)
            listed.append(RELEASE_JSON)
        sums = [f"{digest(os.path.join(args.dir, name))[1]}  {name}" for name in listed]
        write(os.path.join(args.dir, SHA256SUMS), "\n".join(sums) + "\n")
    except Problem as e:
        print(("::error::" if actions else "error: ") + str(e), flush=True)
        return 1

    if note:
        print(("::warning::" if actions else "warning: ") + note, flush=True)
    print(text, end="")
    print("\n".join(sums), flush=True)
    summary = [
        f"### HJViewer {record['version']} for {record['platform']} (build {record['build']})",
        "",
        "```",
        *sums[:handed_out],
        "```",
        "",
        f"`{record['file']}`: {record['size']} bytes, built from `{record['source_sha']}`.",
    ]
    if unreleased:
        summary.append(f"**Not for release** ({unreleased}): no `release.json`, so `hjv-release` takes none of it.")
        print(f"not for release ({unreleased}): no release.json", flush=True)
    path = os.environ.get("GITHUB_STEP_SUMMARY")
    if path:
        with open(path, "a", encoding="utf-8", newline="\n") as f:
            f.write("\n".join(summary) + "\n\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

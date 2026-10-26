#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""The release record `store-releases.txt` of the build repository: what went to which store
(auto-deploy design 3-8). One line per store action, never changed or removed:

    # store version build action track source-commit date(UTC) note
    play 1.9.1 263122249 upload alpha <40-digit commit> 2026-10-28 store #12 build-run 4242
    appstore 1.9.1 263120249 submit - <40-digit commit> 2026-10-28 store #13
    appstore 1.9.1 263120249 release - <40-digit commit> 2026-11-02 store #15

tools/hjv-release (src/store.rs) reads the same lines: every line but comments and blank lines is
`store version build action ...` with store play|appstore, version X.Y.Z, a numeric build and the
action upload|submit|release.

    store-record.py check --store S --version V --build B --action A   (the store run's checks job)
    store-record.py check-build --store S --version V                  (a build run's first step)
    store-record.py add --store S --version V --build B --action A --source SHA [--track T] [--note N]
                                                                         (the store run's record job)

The record comes from --file PATH, or from --repo owner/name through the contents API with
GITHUB_TOKEN (GITHUB_API_URL, default https://api.github.com). A missing file is an empty record,
but only when the repository itself answers: a wrong name or a token that cannot see it is an error.

Rules, within one store only (the stores' versions and build numbers differ, design 3-8):
  - the same version, build and action again passes (a re-run)
  - the same version with another build is refused (the owner's rule: a new build gets a higher
    version), and so is a version below the highest one the store has
  - an App Store submit of a version that has a release line is refused
  - check-build refuses a version the store has already, or a lower one: that build could never go
    to the store
add writes with the file's sha and tries again, up to 5 times, when another run wrote first; its
commit message carries [skip ci]. The script prints no token.

Exit status: 0 passed or written, 1 refused or failed, 2 bad usage.
Offline tests: python3 -I -B -m unittest discover -s scripts -p 'test_store_record.py'
"""
from __future__ import annotations

import argparse
import base64
import datetime
import json
import os
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

RECORD_PATH = "store-releases.txt"
HEADER = "# store version build action track source-commit date(UTC) note\n"
STORES = ("play", "appstore")
ACTIONS = {"play": ("upload",), "appstore": ("submit", "release")}
VERSION = re.compile(r"(0|[1-9][0-9]{0,4})\.(0|[1-9][0-9]{0,4})\.(0|[1-9][0-9]{0,4})")
BUILD = re.compile(r"[0-9]{1,18}")
SHA = re.compile(r"[0-9a-f]{40}")
REPO = re.compile(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+")
MAX_BYTES = 1024 * 1024  # as tools/hjv-release reads it
ATTEMPTS = 5
CALL_TIMEOUT = 30


class RecordError(Exception):
    def __init__(self, message: str, usage: bool = False):
        super().__init__(message)
        self.usage = usage


def version_key(text: str) -> tuple[int, int, int]:
    m = VERSION.fullmatch(text)
    if not m:
        raise RecordError(f"not a version X.Y.Z: {text!r}", usage=True)
    return tuple(int(part) for part in m.groups())


def parse(record: str) -> list[dict]:
    """The record's lines as dicts; a line in no known form stops everything (src/store.rs does too:
    a line written by hand with a typo must not pass for no line)."""
    entries = []
    for n, line in enumerate(record.splitlines(), 1):
        fields = line.split()
        if not fields or fields[0].startswith("#"):
            continue
        bad = RecordError(f"{RECORD_PATH} line {n} is not 'store version build action ...'")
        if len(fields) < 4:
            raise bad
        store, version, build, action = fields[:4]
        if store not in STORES or action not in ("upload", "submit", "release") \
                or not VERSION.fullmatch(version) or not BUILD.fullmatch(build):
            raise bad
        entries.append({"store": store, "version": version, "build": build, "action": action,
                        "line": n, "key": version_key(version)})
    return entries


def check(record: str, store: str, version: str, build: str, action: str) -> str:
    """'recorded' when the same line is there already, 'new' when it may be added; raises otherwise."""
    entries = [e for e in parse(record) if e["store"] == store]
    key = version_key(version)
    if store == "appstore" and action == "submit":
        for e in entries:
            if e["version"] == version and e["action"] == "release":
                raise RecordError(f"appstore {version} is released (line {e['line']}): a released App Store "
                                  "version takes no new submission")
    if any(e["version"] == version and e["build"] == build and e["action"] == action for e in entries):
        return "recorded"
    for e in entries:
        if e["version"] == version and e["build"] != build:
            raise RecordError(f"{store} {version} is recorded with build {e['build']} (line {e['line']}): "
                              f"a new build gets a higher version, so build {build} cannot go to the store")
    if entries:
        highest = max(entries, key=lambda e: e["key"])
        if key < highest["key"]:
            raise RecordError(f"{store} has {highest['version']} already (line {highest['line']}): "
                              f"{version} is lower")
    return "new"


def check_build(record: str, store: str, version: str) -> None:
    entries = [e for e in parse(record) if e["store"] == store]
    key = version_key(version)
    at_or_above = [e for e in entries if e["key"] >= key]
    if at_or_above:
        e = max(at_or_above, key=lambda e: (e["key"], e["line"]))
        same = "has" if e["key"] == key else "is past it with"
        raise RecordError(f"the release record {same} {store} {e['version']} build {e['build']} "
                              f"{e['action']} (line {e['line']}): raise client/version.env before "
                              "building for the store again")


def line_of(args, today: str) -> str:
    note = " ".join(args.note.split()) if args.note else ""
    fields = [args.store, args.version, args.build, args.action, args.track or "-", args.source, today]
    return " ".join(fields + ([note] if note else [])) + "\n"


# ---- the contents API ----------------------------------------------------------------------------

def send(method: str, url: str, headers: dict, data: bytes | None) -> tuple[int, bytes]:
    """The one network seam (tests replace it). Returns (HTTP status, body)."""
    request = urllib.request.Request(url, data=data, method=method, headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=CALL_TIMEOUT) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as error:
        return error.code, error.read()


class Repo:
    def __init__(self, repo: str, env):
        if not REPO.fullmatch(repo) or ".." in repo:
            raise RecordError(f"not a repository owner/name: {repo!r}", usage=True)
        self.repo = repo
        self.api = env.get("GITHUB_API_URL", "https://api.github.com").rstrip("/")
        self.token = env.get("GITHUB_TOKEN", "") or env.get("GH_TOKEN", "")
        if not self.token:
            raise RecordError("set GITHUB_TOKEN (the run's token) to read the record", usage=True)

    def call(self, method: str, path: str, body=None) -> tuple[int, dict]:
        headers = {"Authorization": "Bearer " + self.token, "Accept": "application/vnd.github+json",
                   "X-GitHub-Api-Version": "2022-11-28", "User-Agent": "hjviewer-store-record"}
        data = None
        if body is not None:
            data, headers["Content-Type"] = json.dumps(body).encode(), "application/json"
        for attempt in range(3):
            try:
                status, raw = send(method, f"{self.api}/{path}", headers, data)
            except OSError as error:
                status, raw = 0, str(getattr(error, "reason", error)).encode()
            if (status == 0 or status >= 500) and attempt < 2 and method == "GET":
                time.sleep(3 * (attempt + 1))
                continue
            break
        try:
            answer = json.loads(raw) if raw.strip() else {}
        except ValueError:
            answer = {}
        return status, answer if isinstance(answer, dict) else {}

    def read(self) -> tuple[str, str | None]:
        """(record text, its blob sha or None when there is no file yet)."""
        status, answer = self.call("GET", f"repos/{self.repo}/contents/{RECORD_PATH}")
        if status == 404:
            repo_status, _ = self.call("GET", f"repos/{self.repo}")
            if repo_status != 200:
                raise RecordError(f"{self.repo}: no {RECORD_PATH} and the repository does not answer either "
                                  f"(HTTP {repo_status}): check the name and the token's access")
            return "", None
        if status != 200:
            raise RecordError(f"could not read {self.repo}/{RECORD_PATH}: HTTP {status} {answer.get('message', '')}")
        if answer.get("encoding") != "base64" or not isinstance(answer.get("content"), str) \
                or not isinstance(answer.get("sha"), str):
            raise RecordError(f"{self.repo}/{RECORD_PATH}: the contents API's answer has no base64 content")
        raw = base64.b64decode(answer["content"])
        if len(raw) > MAX_BYTES:
            raise RecordError(f"{RECORD_PATH} is larger than {MAX_BYTES} bytes")
        try:
            return raw.decode("utf-8"), answer["sha"]
        except UnicodeDecodeError:
            raise RecordError(f"{RECORD_PATH} is not UTF-8 text") from None

    def write(self, text: str, sha: str | None, message: str) -> int:
        body = {"message": message, "content": base64.b64encode(text.encode()).decode()}
        if sha:
            body["sha"] = sha
        status, answer = self.call("PUT", f"repos/{self.repo}/contents/{RECORD_PATH}", body)
        if status in (200, 201):
            return status
        # another run wrote first (409: the sha is old; 422: the file appeared meanwhile): read again
        if status == 409 or (status == 422 and sha is None):
            return 409
        raise RecordError(f"could not write {self.repo}/{RECORD_PATH}: HTTP {status} {answer.get('message', '')}")


def read_record(args, env) -> str:
    if args.file:
        try:
            with open(args.file, encoding="utf-8") as handle:
                return handle.read()
        except FileNotFoundError:
            return ""
        except UnicodeDecodeError:
            raise RecordError(f"{args.file} is not UTF-8 text") from None
    return Repo(args.repo, env).read()[0]


def add(args, env, today: str) -> int:
    line = line_of(args, today)
    message = f"store: record {args.store} {args.version} {args.build} {args.action} [skip ci]"
    if args.file:
        record = read_record(args, env)
        if check(record, args.store, args.version, args.build, args.action) == "recorded":
            print(f"{RECORD_PATH}: {args.store} {args.version} {args.build} {args.action} is recorded already")
            return 0
        with open(args.file, "a", encoding="utf-8") as handle:
            handle.write(("" if record else HEADER) + ("" if not record or record.endswith("\n") else "\n") + line)
        print(f"{RECORD_PATH}: added {line.strip()}")
        return 0
    repo = Repo(args.repo, env)
    for attempt in range(1, ATTEMPTS + 1):
        record, sha = repo.read()
        if check(record, args.store, args.version, args.build, args.action) == "recorded":
            print(f"{args.repo}/{RECORD_PATH}: {args.store} {args.version} {args.build} {args.action} "
                  "is recorded already")
            return 0
        text = (record if record else HEADER) + ("" if not record or record.endswith("\n") else "\n") + line
        if repo.write(text, sha, message) != 409:
            summary(f"Release record: added `{line.strip()}` to {args.repo}/{RECORD_PATH}.")
            return 0
        print(f"{RECORD_PATH} changed meanwhile (attempt {attempt} of {ATTEMPTS}); reading it again")
        time.sleep(2 * attempt)
    raise RecordError(f"{RECORD_PATH} kept changing: gave up after {ATTEMPTS} attempts; run the record job again")


def summary(line: str) -> None:
    print(line)
    path = os.environ.get("GITHUB_STEP_SUMMARY")
    if path:
        with open(path, "a", encoding="utf-8") as handle:
            handle.write(line + "\n")


def validate(args) -> None:
    version_key(args.version)
    if args.command in ("check", "add"):
        if args.action not in ACTIONS[args.store]:
            raise RecordError(f"{args.store} records {' or '.join(ACTIONS[args.store])}, not {args.action}",
                              usage=True)
        if not BUILD.fullmatch(args.build or ""):
            raise RecordError(f"not a build number: {args.build!r}", usage=True)
    if args.command == "add":
        if not SHA.fullmatch(args.source or ""):
            raise RecordError("--source takes the 40-digit source commit", usage=True)
        if args.track and not re.fullmatch(r"[a-z0-9_-]+", args.track):
            raise RecordError(f"not a track name: {args.track!r}", usage=True)
        if args.store == "play" and not args.track:
            raise RecordError("a Play line names its track (--track alpha)", usage=True)
        if args.note and ("\n" in args.note or "\r" in args.note or len(args.note) > 200):
            raise RecordError("--note is one line of up to 200 characters", usage=True)


def main(argv: list[str], env=os.environ) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("command", choices=("check", "check-build", "add"))
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--repo", help="the build repository, owner/name (contents API)")
    source.add_argument("--file", help="a local copy of the record")
    parser.add_argument("--store", choices=STORES, required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--build")
    parser.add_argument("--action", choices=("upload", "submit", "release"))
    parser.add_argument("--track")
    parser.add_argument("--source", help="add: the 40-digit source commit")
    parser.add_argument("--note", default="")
    args = parser.parse_args(argv)
    try:
        if args.command in ("check", "add") and not args.action:
            raise RecordError(f"{args.command} needs --action", usage=True)
        validate(args)
        if args.command == "add":
            return add(args, env, datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d"))
        record = read_record(args, env)
        if args.command == "check-build":
            check_build(record, args.store, args.version)
            print(f"release record: no {args.store} line at {args.version} or above; this build may go to the store")
            return 0
        state = check(record, args.store, args.version, args.build, args.action)
        print(f"release record: {args.store} {args.version} build {args.build} {args.action}: "
              + ("recorded already (a re-run)" if state == "recorded" else "allowed"))
        return 0
    except RecordError as error:
        print(f"::error::release record: {error}")
        return 2 if error.usage else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

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
    store-record.py check-run --repo R --store play --version V --run-id N --source SHA
                                                  (the store run's checks job, before an upload)

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

check-run is the provenance of the bundle an upload sends, as tools/hjv-release checks a package
(src/provenance.rs steps 1-3): run N of the build repository is a successful run of build.yml on
main, started by hand; it called exactly one workflow, the public client-build.yml pinned to the
tag's commit SHA; build.yml at the run's commit has the documented shape (one job that only calls
that workflow with source_ref at the same SHA, check_build_yml below); and the run kept the
artifact HJViewer-<version>-<7 digits of SHA>-android-aarch64-play, not expired. Its name goes to
GITHUB_OUTPUT as artifact=.

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
# tools/hjv-release's Version::parse (src/version.rs): no leading zeros, 3, 3 and 2 digits at most
VERSION = re.compile(r"(0|[1-9][0-9]{0,2})\.(0|[1-9][0-9]{0,2})\.(0|[1-9][0-9]?)")
BUILD = re.compile(r"[0-9]{1,18}")
SHA = re.compile(r"[0-9a-f]{40}")
REPO = re.compile(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+")
MAX_BYTES = 1024 * 1024  # as tools/hjv-release reads it
ATTEMPTS = 5
CALL_TIMEOUT = 30
PUBLIC_REPOSITORY = "yonsei-projecttf/hjviewer"
CALLED_WORKFLOW = ".github/workflows/client-build.yml"
BUILD_WORKFLOW = ".github/workflows/build.yml"
# client-build.yml's inputs that build.yml may pass (provenance.rs KNOWN_INPUTS and check_only)
BUILD_INPUTS = ("platforms", "testflight", "play", "play_release", "check_only", "source_ref")


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

class _NoRedirect(urllib.request.HTTPRedirectHandler):
    """urllib's default opener follows a redirect with the Authorization header, to any host: a
    3xx answer is returned as it is instead."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


_OPENER = urllib.request.build_opener(_NoRedirect)


def send(method: str, url: str, headers: dict, data: bytes | None) -> tuple[int, bytes]:
    """The one network seam (tests replace it). Returns (HTTP status, body)."""
    request = urllib.request.Request(url, data=data, method=method, headers=headers)
    try:
        with _OPENER.open(request, timeout=CALL_TIMEOUT) as response:
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


# ---- the build run of an upload (check-run) -------------------------------------------------------

def _content(raw: str) -> str:
    line = raw.rstrip("\r")
    if line.lstrip().startswith("#"):
        return ""
    at = line.find(" #")
    return (line[:at] if at >= 0 else line).rstrip()


_KEY_VALUE = re.compile(r"([A-Za-z0-9_-]+):(?:| (.*))")
_PLAIN = re.compile(r"[A-Za-z0-9_.-]+")
_INPUT = re.compile(r"\$\{\{\s*inputs\.[A-Za-z0-9_]+\s*\}\}")
_ALLOWED = re.compile(r"[A-Za-z0-9 _.,:@/${}\[\]-]*")


def check_build_yml(text: str, source: str) -> None:
    """build.yml is the documented one (docs/ci.md, provenance.rs check_build_yml): top-level keys
    name, run-name, on, permissions, concurrency and jobs; one job, which only calls the public
    client-build.yml at `source` with source_ref `source`, known inputs, secrets: inherit. No lists,
    no quotes, no block scalars: whatever else could run a step is refused."""
    uses = f"{PUBLIC_REPOSITORY}/{CALLED_WORKFLOW}@{source}"
    lines = []
    for n, raw in enumerate(text.splitlines(), 1):
        content = _content(raw)
        if not content:
            continue
        if not _ALLOWED.fullmatch(content):
            raise RecordError(f"build.yml line {n}: a character the documented build.yml has not")
        if content.count("{") != content.count("}") or content.count("[") != content.count("]"):
            raise RecordError(f"build.yml line {n}: a bracket that is not closed on its line")
        body = content.lstrip()
        if body.startswith(("---", "...", "- ")) or body == "-":
            raise RecordError(f"build.yml line {n}: lists and document markers are not used")
        lines.append((n, len(content) - len(body), body))
    section, jobs_seen, job_lines = "", False, []
    for n, indent, body in lines:
        m = _KEY_VALUE.fullmatch(body)
        if indent == 0:
            if not m or m.group(1) not in ("name", "run-name", "on", "permissions", "concurrency", "jobs"):
                raise RecordError(f"build.yml line {n}: not a top-level key of the documented build.yml")
            if m.group(1) == "jobs":
                if jobs_seen or (m.group(2) or "").strip():
                    raise RecordError(f"build.yml line {n}: jobs comes once, without a value")
                jobs_seen = True
            section = m.group(1)
        elif section == "jobs":
            if not m:
                raise RecordError(f"build.yml line {n}: not 'key: value'")
            job_lines.append((n, indent, m.group(1), (m.group(2) or "").strip()))
        elif not section:
            raise RecordError(f"build.yml line {n}: starts indented")
    if len(job_lines) < 2:
        raise RecordError("build.yml has no job, or an empty one")
    job_indent, key_indent = job_lines[0][1], job_lines[1][1]
    if job_lines[0][3] or key_indent <= job_indent:
        raise RecordError("build.yml must have exactly one job")
    keys, parent, uses_seen, source_ref_seen = [], "", False, False
    for n, indent, k, value in job_lines[1:]:
        if indent <= job_indent:
            raise RecordError(f"build.yml line {n}: a second job (it could run anything)")
        if indent == key_indent:
            if k in keys:
                raise RecordError(f"build.yml line {n}: {k} comes twice")
            keys.append(k)
            parent = k
            if k == "uses":
                if value != uses:
                    raise RecordError(f"build.yml line {n}: uses is not {uses}")
                uses_seen = True
            elif k in ("with", "permissions") and not value:
                pass
            elif k == "secrets" and value == "inherit":
                pass
            elif k in ("name", "permissions") and _PLAIN.fullmatch(value):
                pass
            else:
                raise RecordError(f"build.yml line {n}: a job key the documented build.yml has not: {k}")
        elif parent == "with":
            if k not in BUILD_INPUTS:
                raise RecordError(f"build.yml line {n}: client-build.yml takes no input {k}")
            if k == "source_ref":
                if value != source or source_ref_seen:
                    raise RecordError(f"build.yml line {n}: source_ref is not {source}")
                source_ref_seen = True
            elif not (_INPUT.fullmatch(value) or _PLAIN.fullmatch(value)):
                raise RecordError(f"build.yml line {n}: the value of input {k} is not understood")
        elif parent == "permissions" and _PLAIN.fullmatch(value):
            pass
        else:
            raise RecordError(f"build.yml line {n}: an indentation the documented build.yml has not")
    if not uses_seen:
        raise RecordError(f"build.yml's job does not call {uses}")
    if not source_ref_seen:
        raise RecordError("build.yml's job passes no source_ref")


def check_run(repo: Repo, run_id: str, version: str, source: str) -> str:
    """The -play artifact's name, once run_id is shown to be the reviewed build of `source`."""
    status, run = repo.call("GET", f"repos/{repo.repo}/actions/runs/{run_id}")
    if status != 200:
        raise RecordError(f"could not read build run {run_id} of {repo.repo}: HTTP {status}")
    seen = {k: run.get(k) for k in ("path", "event", "status", "conclusion", "head_branch")}
    seen["repository"] = (run.get("repository") or {}).get("full_name")
    wanted = {"path": BUILD_WORKFLOW, "event": "workflow_dispatch", "status": "completed",
              "conclusion": "success", "head_branch": "main", "repository": repo.repo}
    wrong = [f"{k} {seen[k]!r}" for k, v in wanted.items() if seen[k] != v]
    if wrong:
        raise RecordError(f"run {run_id} is not a successful run of build.yml on main started by hand in "
                          f"{repo.repo} ({', '.join(wrong)})")
    pinned = f"{PUBLIC_REPOSITORY}/{CALLED_WORKFLOW}@{source}"
    referenced = run.get("referenced_workflows")
    referenced = referenced if isinstance(referenced, list) else []
    if len(referenced) != 1 or not isinstance(referenced[0], dict) \
            or referenced[0].get("path") != pinned or referenced[0].get("sha") != source:
        paths = [r.get("path") if isinstance(r, dict) else None for r in referenced]
        raise RecordError(f"run {run_id} did not call exactly {pinned} (it called {paths!r})")
    head = run.get("head_sha")
    if not isinstance(head, str) or not SHA.fullmatch(head):
        raise RecordError(f"run {run_id} has no readable head_sha")
    status, answer = repo.call("GET", f"repos/{repo.repo}/contents/{BUILD_WORKFLOW}?ref={head}")
    if status != 200 or answer.get("encoding") != "base64" or not isinstance(answer.get("content"), str):
        raise RecordError(f"could not read {BUILD_WORKFLOW} at {head}: HTTP {status}")
    try:
        text = base64.b64decode(answer["content"]).decode("utf-8")
    except (ValueError, UnicodeDecodeError):
        raise RecordError(f"{BUILD_WORKFLOW} at {head} is not UTF-8 text") from None
    check_build_yml(text, source)
    artifact = f"HJViewer-{version}-{source[:7]}-android-aarch64-play"
    status, answer = repo.call("GET", f"repos/{repo.repo}/actions/runs/{run_id}/artifacts?per_page=100")
    if status != 200:
        raise RecordError(f"could not list the artifacts of run {run_id}: HTTP {status}")
    found = [a for a in answer.get("artifacts") or [] if isinstance(a, dict) and a.get("name") == artifact]
    if not found or found[0].get("expired") is not False:
        gone = " (it has expired: bundles are kept 30 days)" if found else ""
        raise RecordError(f"run {run_id} has no {artifact}{gone}: the bundle must be of client-{version}'s "
                          "commit. A gone bundle takes a new build with a higher version.")
    return artifact


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
    if args.command == "check-run":
        if args.store != "play" or not args.repo:
            raise RecordError("check-run takes --store play and --repo (the build run of an upload)", usage=True)
        if not re.fullmatch(r"[0-9]{1,20}", args.run_id or ""):
            raise RecordError("check-run needs --run-id, the build run's ID", usage=True)
        if not SHA.fullmatch(args.source or ""):
            raise RecordError("--source takes the 40-digit source commit", usage=True)
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
    parser.add_argument("command", choices=("check", "check-build", "add", "check-run"))
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--repo", help="the build repository, owner/name (contents API)")
    source.add_argument("--file", help="a local copy of the record")
    parser.add_argument("--store", choices=STORES, required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--build")
    parser.add_argument("--action", choices=("upload", "submit", "release"))
    parser.add_argument("--track")
    parser.add_argument("--source", help="add, check-run: the 40-digit source commit")
    parser.add_argument("--note", default="")
    parser.add_argument("--run-id", help="check-run: the build run's ID (…/actions/runs/<ID>)")
    args = parser.parse_args(argv)
    try:
        if args.command in ("check", "add") and not args.action:
            raise RecordError(f"{args.command} needs --action", usage=True)
        validate(args)
        if args.command == "add":
            return add(args, env, datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d"))
        if args.command == "check-run":
            artifact = check_run(Repo(args.repo, env), args.run_id, args.version, args.source)
            output = env.get("GITHUB_OUTPUT")
            if output:
                with open(output, "a", encoding="utf-8") as handle:
                    handle.write(f"artifact={artifact}\n")
            summary(f"Bundle: {artifact} of build run {args.run_id} (build.yml on main, client-build.yml "
                    f"@{args.source[:12]}, build.yml's shape checked)")
            return 0
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

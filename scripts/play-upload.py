#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Upload a signed Android App Bundle to a Google Play testing track (Play Developer API v3).

Before any edit, applications.tracks.releases.list (no edit) reads the target track (alpha, the
closed testing track: auto-deploy design 7-2) and the internal track:
  - a release of this versionCode that Play has (in review, approved or published) ends the run
    as done: a re-run after a commit whose answer was lost changes nothing
  - a release IN_REVIEW stops the run unless --in-review cancel; NOT_APPROVED and
    NOT_SENT_FOR_REVIEW always stop it, with what to do in Play Console
Then one edit: insert -> tracks.list + bundles.list -> bundles.upload (skipped when Play already
holds the bundle, same sha256) -> tracks.update -> commit (or validate + delete with --mode
validate). The commit always names changesInReviewBehavior (Google's default would cancel a
review): ERROR_IF_IN_REVIEW, or CANCEL_IN_REVIEW_AND_SUBMIT with --in-review cancel. Any failure
deletes the edit, so nothing half-lands. A bundle with a lower versionCode than the track already
serves is refused before it is uploaded.
Python standard library only; the JWT is signed by the runner's openssl.

Credentials come from the environment only, never from arguments or files:
  PLAY_ACCESS_TOKEN                    an OAuth access token with the androidpublisher scope, e.g. from
                                       Workload Identity Federation (google-github-actions/auth), or
  PLAY_SA_EMAIL + PLAY_SA_PRIVATE_KEY  the service account's client_email and its PEM private_key
                                       (the two fields of the JSON key). The key reaches openssl through
                                       a pipe and never touches the disk.

Modes:
  dry-run   no network, no credentials needed: checks the arguments and the bundle, then prints the
            calls it would make (with credentials set, also proves the key can sign)
  validate  everything up to the commit, then asks Play to validate the edit and deletes it: nothing
            is released, and a bundle uploaded to a deleted edit is discarded
  commit    the real upload (default)
  status    reads the target track's and the internal track's releases (releases.list, no edit, no
            bundle): one summary line, and with --json a file for the run's artifact

Exit status: 0 done (or already done), 1 failed, 2 bad usage.
Used by the Google Play job of .github/workflows/store-release.yml (the store run); docs/store-release.md
and docs/ci.md, "Google Play 업로드".
Offline tests: python3 -I -B -m unittest discover -s scripts -p 'test_play_upload.py'
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import http.client
import json
import os
import re
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
import zipfile

API = "https://androidpublisher.googleapis.com/androidpublisher/v3/applications"
UPLOAD_API = "https://androidpublisher.googleapis.com/upload/androidpublisher/v3/applications"
TOKEN_URL = "https://oauth2.googleapis.com/token"
SCOPE = "https://www.googleapis.com/auth/androidpublisher"
NOTES_LIMIT = 500  # Play Console Help: "up to 500 Unicode characters per language"
PEM_LIMIT = 16384  # an RSA private key in PEM is a few KB; this also stays far below a pipe's buffer
CALL_TIMEOUT = 60
UPLOAD_TIMEOUT = 900  # the discovery document asks for a longer timeout on bundles.upload
STATUSES = ("completed", "draft")  # inProgress/halted (staged rollouts) are not for a testing track here
TRACK = "alpha"  # the closed testing track Alpha (the owner's decision of 2026-10-26)
# edits.commit's changesInReviewBehavior, always sent: Google's default cancels a review in progress
COMMIT_BEHAVIOUR = {"error": "ERROR_IF_IN_REVIEW", "cancel": "CANCEL_IN_REVIEW_AND_SUBMIT"}
# ReleaseSummary.releaseLifecycleState, without the RELEASE_LIFECYCLE_STATE_ prefix
STATE_PREFIX = "RELEASE_LIFECYCLE_STATE_"
HELD = ("IN_REVIEW", "APPROVED_NOT_PUBLISHED", "PUBLISHED")  # states in which Play has the release


class PlayError(Exception):
    """An API or token error: HTTP status, Google's message and its reason/status words.

    words == "usage" marks a problem with the input (exit status 2); advice, when set, replaces
    the hint that main() would print.
    """

    def __init__(self, status: int, message: str, words: str = "", advice: str = ""):
        super().__init__(message)
        self.status = status
        self.message = message
        self.words = words
        self.advice = advice

    @classmethod
    def from_body(cls, status: int, raw: bytes) -> "PlayError":
        try:
            body = json.loads(raw or b"{}")
        except ValueError:
            return cls(status, raw[:300].decode("utf-8", "replace"))
        err = body.get("error") if isinstance(body, dict) else None
        if isinstance(err, dict):  # Google API error
            reasons = " ".join(e.get("reason", "") for e in err.get("errors", []) if isinstance(e, dict))
            return cls(status, str(err.get("message", "")), f"{err.get('status', '')} {reasons}".strip())
        if isinstance(err, str):  # OAuth token endpoint error
            return cls(status, str(body.get("error_description", err)), err)
        return cls(status, raw[:300].decode("utf-8", "replace"))


def send(method: str, url: str, headers: dict, data: bytes | None, timeout: int) -> tuple[int, bytes]:
    """The one network seam (tests replace it). Returns (HTTP status, body)."""
    request = urllib.request.Request(url, data=data, method=method, headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as error:
        return error.code, error.read()


def call(method: str, url: str, token: str | None = None, *, body=None, data: bytes | None = None,
         content_type: str | None = None, timeout: int = CALL_TIMEOUT, retries: int = 0) -> dict:
    headers = {}
    if token:
        headers["Authorization"] = "Bearer " + token
    if body is not None:
        data, content_type = json.dumps(body).encode(), "application/json"
    if content_type:
        headers["Content-Type"] = content_type
    for attempt in range(retries + 1):
        try:
            status, raw = send(method, url, headers, data, timeout)
        except (OSError, http.client.HTTPException) as error:
            # no proper answer: DNS, TLS, reset, timeout (URLError is an OSError), a cut-off reply
            status, raw = 0, str(getattr(error, "reason", error) or type(error).__name__).encode()
        if status in (0, 429) or status >= 500:
            if attempt < retries:
                time.sleep(5 * (attempt + 1))
                continue
        if 200 <= status < 300:
            try:
                answer = json.loads(raw) if raw.strip() else {}
            except ValueError:
                answer = None
            if not isinstance(answer, dict):
                # the body is not repeated: a token endpoint answer may hold a token
                raise PlayError(status, f"Google answered {method} {url.split('?')[0]} with a body "
                                        "that is not a JSON object")
            return answer
        raise PlayError.from_body(status, raw)
    raise AssertionError("unreachable")


# ---- credentials -----------------------------------------------------------------------------------

def b64url(raw: bytes) -> str:
    return base64.urlsafe_b64encode(raw).rstrip(b"=").decode()


def rs256(signing_input: bytes, pem: str) -> bytes:
    """RSASSA-PKCS1-v1_5 with SHA-256 by openssl; the key goes through a pipe, never a file."""
    read_end, write_end = os.pipe()
    try:
        # The whole key goes into the pipe before openssl starts, so it must fit the pipe's buffer
        # (64 KB on Linux); credentials() refuses anything longer than PEM_LIMIT.
        pending = memoryview(pem.encode())
        while pending:
            pending = pending[os.write(write_end, pending):]
    finally:
        os.close(write_end)
    try:
        done = subprocess.run(["openssl", "dgst", "-sha256", "-sign", f"/dev/fd/{read_end}"],
                              input=signing_input, capture_output=True, pass_fds=(read_end,))
    finally:
        os.close(read_end)
    if done.returncode != 0 or not done.stdout:
        # openssl's own message names the file descriptor, never the key
        raise PlayError(0, "openssl could not sign with PLAY_SA_PRIVATE_KEY: it must be the PEM "
                           "private_key field of the service account's JSON key", "usage")
    return done.stdout


def assertion(email: str, pem: str, now: int) -> str:
    header = b64url(json.dumps({"alg": "RS256", "typ": "JWT"}).encode())
    claims = b64url(json.dumps({"iss": email, "scope": SCOPE, "aud": TOKEN_URL,
                                "iat": now, "exp": now + 600}).encode())
    signing_input = f"{header}.{claims}".encode()
    return f"{header}.{claims}.{b64url(rs256(signing_input, pem))}"


def credentials(env) -> tuple[str, str] | None:
    """('token', '') or ('key', email) when credentials are set, else None."""
    if env.get("PLAY_ACCESS_TOKEN"):
        return "token", ""
    email, pem = env.get("PLAY_SA_EMAIL", "").strip(), env.get("PLAY_SA_PRIVATE_KEY", "")
    if email or pem:
        if not email or not pem:
            raise PlayError(0, "set both PLAY_SA_EMAIL and PLAY_SA_PRIVATE_KEY", "usage")
        if not re.fullmatch(r"[^@\s]+@[^@\s]+\.iam\.gserviceaccount\.com", email):
            raise PlayError(0, "PLAY_SA_EMAIL is not a service account address "
                               "(…@<project>.iam.gserviceaccount.com)", "usage")
        if "PRIVATE KEY-----" not in pem or len(pem) > PEM_LIMIT:
            raise PlayError(0, "PLAY_SA_PRIVATE_KEY is not a PEM private key", "usage")
        return "key", email
    return None


def pem_of(env) -> str:
    """The PEM key; also takes it pasted from the JSON file with literal \\n escapes."""
    pem = env["PLAY_SA_PRIVATE_KEY"]
    return pem.replace("\\n", "\n") if "\\n" in pem and "\n" not in pem.strip() else pem


def usable_token(token, origin: str, words: str = "") -> str:
    """The token, checked to fit an HTTP header: http.client would otherwise reject it in an error
    message that repeats the whole header. The token itself is never part of a message."""
    token = token.strip() if isinstance(token, str) else ""
    if not re.fullmatch(r"[\x21-\x7e]+", token):
        raise PlayError(0, f"{origin} is not a usable access token", words)
    return token


def access_token(env) -> str:
    if env.get("PLAY_ACCESS_TOKEN"):
        return usable_token(env["PLAY_ACCESS_TOKEN"], "PLAY_ACCESS_TOKEN", "usage")
    kind = credentials(env)
    if kind is None:
        raise PlayError(0, "no credentials: set PLAY_SA_EMAIL and PLAY_SA_PRIVATE_KEY (or "
                           "PLAY_ACCESS_TOKEN)", "usage")
    jwt = assertion(env["PLAY_SA_EMAIL"].strip(), pem_of(env), int(time.time()))
    form = urllib.parse.urlencode({"grant_type": "urn:ietf:params:oauth:grant-type:jwt-bearer",
                                   "assertion": jwt}).encode()
    reply = call("POST", TOKEN_URL, data=form, content_type="application/x-www-form-urlencoded", retries=2)
    return usable_token(reply.get("access_token"), "the token endpoint's access_token")


# ---- checks that need no network -------------------------------------------------------------------

def check_args(args) -> str:
    """Validates the arguments; returns the release notes (possibly empty)."""
    if not re.fullmatch(r"[A-Za-z][A-Za-z0-9_]*(\.[A-Za-z][A-Za-z0-9_]*)+", args.package):
        raise PlayError(0, f"not an Android package name: {args.package!r}", "usage")
    if not re.fullmatch(r"[A-Za-z0-9_:.-]+", args.track):
        raise PlayError(0, f"not a track name: {args.track!r}", "usage")
    if not re.fullmatch(r"[a-z]{2,3}(-[A-Za-z0-9]{2,8})*", args.notes_language):
        raise PlayError(0, f"not a BCP-47 language tag: {args.notes_language!r}", "usage")
    notes = ""
    if args.notes_file:
        try:
            with open(args.notes_file, encoding="utf-8") as handle:
                notes = handle.read().strip()
        except UnicodeDecodeError:
            raise PlayError(0, f"{args.notes_file} is not UTF-8 text", "usage") from None
    if len(notes) > NOTES_LIMIT:
        raise PlayError(0, f"release notes are {len(notes)} characters; Play takes {NOTES_LIMIT} per "
                           "language", "usage")
    if len(args.name) > 50:
        raise PlayError(0, "keep the release name to 50 characters", "usage")
    if args.mode != "status" and not args.aab:
        raise PlayError(0, f"--mode {args.mode} needs --aab", "usage")
    return notes


def inspect_bundle(path: str) -> tuple[bytes, str]:
    """Reads the bundle and checks it looks like a signed AAB; returns (bytes, sha256)."""
    with open(path, "rb") as handle:
        data = handle.read()
    try:
        names = set(zipfile.ZipFile(path).namelist())
    except zipfile.BadZipFile:
        raise PlayError(0, f"{path} is not an Android App Bundle (not a zip file)", "usage") from None
    missing = [n for n in ("BundleConfig.pb", "base/manifest/AndroidManifest.xml") if n not in names]
    if missing:
        raise PlayError(0, f"{path} is not an Android App Bundle (missing {', '.join(missing)})", "usage")
    signed = any(re.fullmatch(r"META-INF/[^/]+\.(RSA|EC|DSA)", n) for n in names) and \
        any(re.fullmatch(r"META-INF/[^/]+\.SF", n) for n in names)
    if not signed:
        raise PlayError(0, f"{path} is not signed: Play only takes bundles signed with the upload key", "usage")
    return data, hashlib.sha256(data).hexdigest()


# ---- the edit --------------------------------------------------------------------------------------

# What Play's answers about an unfinished declaration or policy item contain. They come as 403, and
# they name permissions, so they must be recognised before the service account's own 401/403.
DECLARATION_WORDS = ("foreground service", "declar", "let us know", "privacy policy", "app content",
                     "accessibility")
# The answers that do mean the service account lacks access (docs/ci.md, "오류와 대처")
NO_ACCESS_WORDS = ("does not have permission", "caller does not have", "insufficient permission")


def hint(error: PlayError, args) -> str:
    """One actionable sentence for the errors setting up Play produces most."""
    text = f"{error.message} {error.words}".lower()
    if "invalid_grant" in text or "invalid jwt" in text:
        return ("Google refused the service account key (deleted, disabled or the wrong one): create a new "
                "JSON key and set PLAY_SA_EMAIL and PLAY_SA_PRIVATE_KEY again.")
    if "has not been used in project" in text or "service_disabled" in text or "accessnotconfigured" in text:
        return "Enable the Google Play Android Developer API in the service account's Google Cloud project."
    # before the 404 rule below: Play may answer a lost edit with 404 too
    if "edit" in text and ("deleted" in text or "expired" in text or "conflict" in text):
        return "Another edit was committed meanwhile (another run or a change in Play Console): run it again."
    if "package not found" in text or "applicationnotfound" in text or error.status == 404:
        return (f"Play has no app {args.package} that this account can see: create it in Play Console and "
                "upload the first bundle there by hand (the API cannot create apps), then retry.")
    if "already been used" in text or "versionconflict" in text:
        return ("This versionCode is taken in Play (uploaded before): build again so the bundle gets a new "
                "build number.")
    if "existing users to upgrade" in text or "shadowed" in text or "higher version code" in text:
        return ("Play already serves a newer versionCode than this bundle's, so it would reach no tester: "
                "start a new build (its versionCode will be higher). Nothing was committed.")
    if "draft app" in text:
        return ("The Play app is still a draft: upload with release status 'draft' and roll it out in Play "
                "Console, or finish the app's first rollout there.")
    if "changesnotsentforreview" in text or "in review" in text:
        return ("Play is holding changes (in review, or after a rejection): resolve them under Publishing "
                "overview in Play Console, then retry. Nothing was committed.")
    if "signed with" in text or "certificate" in text or "upload key" in text:
        return ("The bundle is not signed with the upload key Play expects (the message names its "
                "fingerprint): sign with that keystore, or request an upload key reset in Play Console > "
                "Protected with Play > Play Store protection > Manage Play app signing (Upload key certificate "
                "> Request upload key reset).")
    if "language" in text or "releasenotes" in text:
        return (f"Release notes in {args.notes_language} were refused: the language must be one of the app's "
                "store listing languages.")
    if any(word in text for word in DECLARATION_WORDS):
        return ("Play wants a declaration or policy item finished first, the one Google's message above "
                "names: complete it in Play Console > App content, then use Re-run failed jobs (the bundle "
                "is kept). Nothing was committed.")
    # Not every 403: Play also answers 403 for reasons that have nothing to do with the account.
    if error.status == 401 or any(word in text for word in NO_ACCESS_WORDS):
        return (f"The service account (PLAY_SA_EMAIL) has no release permission for {args.package}: in Play "
                "Console > Users and permissions, invite it with 'Release apps to testing tracks' for this "
                "app (new permissions can take a while to apply).")
    if error.status == 0 or error.status >= 500 or error.status == 429:
        return "Google Play did not answer properly; run it again later. Nothing was committed."
    return "See Google's message above."


def summary(line: str) -> None:
    print(line)
    path = os.environ.get("GITHUB_STEP_SUMMARY")
    if path:
        with open(path, "a", encoding="utf-8") as handle:
            handle.write(line + "\n")


def live_version(tracks: list, track: str) -> int:
    """The highest versionCode the track serves: its releases that are not drafts (0: none)."""
    return max((int(code) for entry in tracks if entry.get("track") == track
                for release in entry.get("releases") or [] if release.get("status") != "draft"
                for code in release.get("versionCodes") or []), default=0)


def refuse_older(version_code: int, live: int, track: str) -> None:
    """Play jobs run in the order they arrive, not in versionCode order. A bundle below what the
    track serves would reach no tester, and Play may accept such a release all the same."""
    if version_code < live:
        raise PlayError(0, f"Play already serves versionCode {live} on {track}; this run's bundle "
                           f"({version_code}) is older", "",
                        "A bundle of an older run cannot replace a newer release: nothing was released. "
                        "Start a new build (its versionCode will be higher).")


def release_body(track: str, version_code: int, args, notes: str) -> dict:
    release = {"name": args.name or None, "versionCodes": [str(version_code)], "status": args.status}
    if not release["name"]:
        del release["name"]  # Play then names it after the bundle's versionName
    if notes:
        release["releaseNotes"] = [{"language": args.notes_language, "text": notes}]
    return {"track": track, "releases": [release]}


def releases_url(package: str, track: str) -> str:
    return f"{API}/{package}/tracks/{urllib.parse.quote(track, safe=':')}/releases"


def state_of(release: dict) -> str:
    return str(release.get("releaseLifecycleState", "")).removeprefix(STATE_PREFIX) or "UNSPECIFIED"


def codes_of(release: dict) -> list[int]:
    return sorted(int(a["versionCode"]) for a in release.get("activeArtifacts") or []
                  if isinstance(a, dict) and str(a.get("versionCode", "")).isdigit())


def list_releases(token: str, args) -> dict:
    """{track: [ReleaseSummary, ...]} for the target track and internal; no edit is opened. A track
    the app does not have is None (internal may be called qa, or not be set up)."""
    found = {}
    for track in dict.fromkeys((args.track, "internal")):
        try:
            found[track] = call("GET", releases_url(args.package, track), token, retries=2).get("releases") or []
        except PlayError as error:
            if error.status != 404:
                raise
            if track == args.track:
                raise PlayError(404, f"Play has no track {track!r} for {args.package}", "usage") from None
            found[track] = None
    return found


def describe(found: dict) -> list[str]:
    lines = []
    for track, entries in found.items():
        if entries is None:
            lines.append(f"{track}: no such track")
        elif not entries:
            lines.append(f"{track}: no release")
        for release in entries or []:
            codes = ", ".join(str(c) for c in codes_of(release)) or "no bundle"
            lines.append(f"{track}: {release.get('releaseName') or '(no name)'}, versionCode {codes}, "
                         f"{state_of(release)}")
    return lines


def precheck(found: dict, args) -> str | None:
    """Before an edit: a done-message when Play has this versionCode on the track already, None to go
    on; raises when a review or a rejection stands in the way (design 7-2)."""
    if args.version_code:
        for release in found.get(args.track) or []:
            if args.version_code in codes_of(release) and state_of(release) in HELD:
                return (f"Google Play {args.track}: versionCode {args.version_code} is there already "
                        f"({state_of(release)}); nothing to do.")
    for track, entries in found.items():
        for release in entries or []:
            state, name = state_of(release), release.get("releaseName") or "a release"
            if state in ("NOT_APPROVED", "NOT_SENT_FOR_REVIEW"):
                raise PlayError(0, f"{track} has {name} {state}: Play holds changes after a rejection or "
                                   "changes not sent for review", "",
                                "Resolve them in Play Console > Publishing overview (send them for review by "
                                "hand, or remove them), then run the upload again. No edit was opened. A code "
                                "problem takes a new build with a higher version.")
            if state == "IN_REVIEW":
                if args.in_review == "cancel":
                    print(f"note: {track} has {name} IN_REVIEW; --in-review cancel: the commit cancels that "
                          "review and sends this release (the review starts over)")
                    continue
                raise PlayError(0, f"{track} has {name} IN_REVIEW: Google is reviewing an earlier change", "",
                                "Wait for the review to end (store run action=status), or, if the owner decides "
                                "so, run the upload with in_review=cancel: that cancels the review in progress "
                                "and sends this release instead. No edit was opened.")
    return None


def status(args, env) -> int:
    check_args(args)
    token = access_token(env)
    found = list_releases(token, args)
    for line in describe(found):
        print(line)
    report = {"package": args.package, "checked_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
              "tracks": {track: None if entries is None else
                         [{"name": r.get("releaseName"), "versionCodes": codes_of(r), "state": state_of(r)}
                          for r in entries] for track, entries in found.items()}}
    target = found.get(args.track) or []
    if args.version_code:
        states = [state_of(r) for r in target if args.version_code in codes_of(r)]
        report["versionCode"] = {"code": args.version_code, "states": states}
        summary(f"Google Play {args.track}: versionCode {args.version_code} "
                + (", ".join(states) if states else "not on the track"))
    else:
        summary(f"Google Play {args.track}: " + ("; ".join(
            f"{', '.join(map(str, codes_of(r))) or '-'} {state_of(r)}" for r in target) or "no release"))
    if args.json:
        with open(args.json, "w", encoding="utf-8") as handle:
            json.dump(report, handle, indent=2)
            handle.write("\n")
    return 0


def upload(args, env) -> int:
    if args.mode == "status":
        return status(args, env)
    notes = check_args(args)
    data, sha256 = inspect_bundle(args.aab)
    print(f"bundle {args.aab}: {len(data)} bytes, sha256 {sha256}")
    kind = credentials(env)

    if args.mode == "dry-run":
        if kind and kind[0] == "key":
            assertion(env["PLAY_SA_EMAIL"].strip(), pem_of(env), int(time.time()))
            print("credentials: service account key set and usable by openssl (not sent anywhere)")
        else:
            print(f"credentials: {'access token set' if kind else 'none (fine for a dry run)'}")
        base = f"{API}/{args.package}/edits"
        planned = release_body(args.track, args.version_code or 0, args, notes)
        for step in (f"POST {TOKEN_URL} (jwt-bearer)" if not kind or kind[0] == "key" else None,
                     f"GET  {releases_url(args.package, args.track)} (no edit: stop on IN_REVIEW unless "
                     f"--in-review cancel, on NOT_APPROVED and NOT_SENT_FOR_REVIEW)",
                     f"GET  {releases_url(args.package, 'internal')}" if args.track != "internal" else None,
                     f"POST {base}", f"GET  {base}/<edit>/tracks", f"GET  {base}/<edit>/bundles",
                     f"POST {UPLOAD_API}/{args.package}/edits/<edit>/bundles?uploadType=media "
                     "(skipped if Play already holds this sha256)",
                     f"PUT  {base}/<edit>/tracks/{args.track} {json.dumps(planned, ensure_ascii=False)}",
                     f"POST {base}/<edit>:commit?changesInReviewBehavior={COMMIT_BEHAVIOUR[args.in_review]}"
                     if args.mode != "validate" else f"POST {base}/<edit>:validate, then DELETE the edit"):
            if step:
                print("would call: " + step)
        print("dry run: nothing was sent")
        return 0

    token = access_token(env)
    done = precheck(list_releases(token, args), args)
    if done:
        summary(done)
        return 0
    base = f"{API}/{args.package}/edits"
    edit = None
    left = "nothing committed"  # said when the edit is deleted; not known after an unanswered commit
    try:
        edit = call("POST", base, token, body={}, retries=2)["id"]
        url = f"{base}/{edit}"
        tracks = call("GET", f"{url}/tracks", token, retries=2).get("tracks", [])
        names = [t.get("track") for t in tracks]
        track = args.track
        if track not in names:
            alias = {"internal": "qa", "qa": "internal"}.get(track)
            if alias not in names:
                raise PlayError(0, f"Play has no track {track!r} for {args.package}; it has {names}", "usage")
            print(f"track {track!r} is called {alias!r} in this app")
            track = alias
        live = live_version(tracks, track)

        held = {b.get("sha256"): int(b["versionCode"])
                for b in call("GET", f"{url}/bundles", token, retries=2).get("bundles", [])}
        if sha256 in held:
            version_code = held[sha256]
            print(f"Play already holds this bundle as versionCode {version_code}; not uploading it again")
            for entry in tracks:
                for release in entry.get("releases", []) if entry.get("track") == track else []:
                    if str(version_code) in release.get("versionCodes", []) and release.get("status") == args.status:
                        summary(f"Google Play {track}: versionCode {version_code} is already released "
                                f"({args.status}); nothing to do.")
                        return 0
        else:
            if args.version_code:
                refuse_older(args.version_code, live, track)  # before the upload: nothing is sent
            uploaded = call("POST", f"{UPLOAD_API}/{args.package}/edits/{edit}/bundles?uploadType=media",
                            token, data=data, content_type="application/octet-stream", timeout=UPLOAD_TIMEOUT)
            if uploaded.get("sha256") and uploaded["sha256"].lower() != sha256:
                raise PlayError(0, "Play received different bytes than the bundle (sha256 mismatch)")
            version_code = int(uploaded["versionCode"])
            print(f"uploaded versionCode {version_code}")
        if args.version_code and version_code != args.version_code:
            raise PlayError(0, f"the bundle's versionCode is {version_code}, the build expected "
                               f"{args.version_code}", "usage",
                            "The bundle is not the one the build reported: check the bundle artifact of this "
                            "run, then run the workflow again.")
        refuse_older(version_code, live, track)

        call("PUT", f"{url}/tracks/{urllib.parse.quote(track, safe=':')}", token,
             body=release_body(track, version_code, args, notes))
        if args.mode == "validate":
            call("POST", f"{url}:validate", token)
            summary(f"Google Play {track}: versionCode {version_code} validated ({args.status}); "
                    "nothing was released (validate mode).")
            return 0
        try:
            call("POST", f"{url}:commit?changesInReviewBehavior={COMMIT_BEHAVIOUR[args.in_review]}", token)
        except PlayError as error:
            if error.status == 0 or error.status >= 500:
                # no refusal, only no proper answer: Play may have committed and its reply got lost
                left = "it was still open"
                error.advice = ("Google Play did not answer the commit properly, so the release may or may "
                                "not have gone through: run the same upload again (or Re-run failed jobs). A "
                                "bundle that Play has already is detected and left as it is.")
            raise
        edit = None  # committed: nothing to delete
        summary(f"Uploaded {args.package} versionCode {version_code} to Google Play {track} ({args.status}). "
                + (("Internal testers get it from the Play Store once Play has processed it." if track in
                    ("internal", "qa") else "Google reviews it first (hours to 7 days); the store run's status "
                    "shows when it is PUBLISHED.")
                   if args.status == "completed" else "Roll the draft out in Play Console."))
        return 0
    finally:
        if edit:
            try:
                call("DELETE", f"{base}/{edit}", token)
                print(f"deleted edit {edit} ({left})")
            except PlayError:
                pass  # an edit left open expires on its own; the next insert invalidates it anyway


def main(argv: list[str], env=os.environ) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--package", required=True)
    parser.add_argument("--aab", help="the signed bundle (every mode but status)")
    parser.add_argument("--track", default=TRACK)
    parser.add_argument("--status", choices=STATUSES, default="completed")
    parser.add_argument("--name", default="", help="release name, e.g. '1.6.0 (262851234)'")
    parser.add_argument("--notes-file", help="release notes, UTF-8 text, up to 500 characters")
    parser.add_argument("--notes-language", default="ko-KR")
    parser.add_argument("--version-code", type=int, help="fail unless the bundle has this versionCode")
    parser.add_argument("--mode", choices=("dry-run", "validate", "commit", "status"), default="commit")
    parser.add_argument("--in-review", choices=tuple(COMMIT_BEHAVIOUR), default="error",
                        help="a release in review: error (stop, default) or cancel (cancel it and send this one)")
    parser.add_argument("--json", help="status: write the releases to this file")
    args = parser.parse_args(argv)
    try:
        return upload(args, env)
    except PlayError as error:
        status = f"HTTP {error.status}: " if error.status else ""
        advice = error.advice or (hint(error, args) if error.words != "usage" else "Fix the input above.")
        # workflow commands go to stdout, like the rest of client-build.yml
        print(f"::error::Google Play upload failed: {status}{error.message}")
        print(f"::error::{advice}")
        return 2 if error.words == "usage" else 1
    except OSError as error:
        print(f"::error::{error}")
        return 2
    except (KeyError, TypeError, ValueError) as error:
        # An answer without the fields the API reference promises (the edit was deleted above). Only
        # the type or the missing field is printed: some library messages repeat request headers.
        detail = f"no field {error}" if isinstance(error, KeyError) else type(error).__name__
        print(f"::error::Google Play upload failed: unexpected answer from Play ({detail})")
        print("::error::Run it again; if it repeats, compare the answer with the Play Developer API reference.")
        return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

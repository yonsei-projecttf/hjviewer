# SPDX-License-Identifier: AGPL-3.0-only
# Offline tests for scripts/play-upload.py: no network and no Google credentials (PlayStoreRunTest:
# the store run's alpha upload and status). The fake Play API
# below answers the calls; the key tests sign with throwaway RSA keys that openssl makes in a temporary
# directory or in memory (they belong to no Google account and are gone at the end).
# PlayBundleListsTest, at the end, reads no script: it holds the Play bundle check of client-build.yml
# and the Play manifest overlay (client/play/AndroidManifest.xml) to each other, and the bundle steps'
# conditions and order to what docs/ci.md says a failed bundle does.
# Run: python3 -I -B -m unittest discover -s scripts -p 'test_play_upload.py'  (scripts/ci.sh client)
import argparse
import contextlib
import hashlib
import http.client
import importlib.util
import io
import json
import pathlib
import re
import subprocess
import tempfile
import unittest
import urllib.parse
import xml.etree.ElementTree
import zipfile
from unittest import mock

HERE = pathlib.Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("play_upload", HERE / "play-upload.py")
play = importlib.util.module_from_spec(spec)
spec.loader.exec_module(play)

PKG = "kr.ac.yonsei.hjviewer"
EDITS = f"{play.API}/{PKG}/edits"
TOKEN = "ya29.fake-access-token-value"
SA_EMAIL = "play-upload@example-project.iam.gserviceaccount.com"


def make_aab(path, signed=True):
    with zipfile.ZipFile(path, "w") as z:
        z.writestr("BundleConfig.pb", b"\x0a\x00")
        z.writestr("base/manifest/AndroidManifest.xml", b"\x00proto")
        z.writestr("base/dex/classes.dex", b"dex\n035")
        if signed:
            z.writestr("META-INF/MANIFEST.MF", "Manifest-Version: 1.0\n")
            z.writestr("META-INF/HJVIEWER.SF", "Signature-Version: 1.0\n")
            z.writestr("META-INF/HJVIEWER.RSA", b"\x30\x82")


def throwaway_key():
    """A PEM RSA key made in memory for one test; it signs nothing outside the test."""
    return subprocess.run(["openssl", "genpkey", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048"],
                          check=True, capture_output=True, text=True).stdout


def no_network(*args):
    raise AssertionError("the script touched the network")


class FakePlay:
    """Answers the Play API calls the script makes; records them."""

    def __init__(self, tracks=None, bundles=None, fail=None, upload_sha=None, version_code=262851234,
                 releases=None):
        self.calls = []
        self.listed = []  # applications.tracks.releases.list (no edit), kept apart from the edit's calls
        self.releases = releases or {}  # track -> [ReleaseSummary]; a track missing here answers 404
        self.tracks = tracks if tracks is not None else [{"track": "production"}, {"track": "internal"}]
        self.bundles = bundles or []
        self.fail = fail or {}  # (method, url suffix) -> (status, body)
        self.upload_sha = upload_sha
        self.version_code = version_code
        self.token_check = None

    def __call__(self, method, url, headers, data, timeout):
        self.calls.append((method, url, headers, data))
        key = next((k for k in self.fail if k[0] == method and url.split("?")[0].endswith(k[1])), None)
        if key:
            status, body = self.fail[key]
            if isinstance(status, list):  # a sequence of answers for retries
                status, body = status.pop(0) if len(status) > 1 else status[0]
            if status:
                return status, json.dumps(body).encode()
        if url == play.TOKEN_URL:
            return 200, json.dumps(self.token_check(data)).encode()
        path = url.split("?")[0]
        if method == "GET" and path.startswith(f"{play.API}/{PKG}/tracks/") and path.endswith("/releases"):
            self.calls.pop()
            track = path.rsplit("/", 2)[1]
            self.listed.append(track)
            if track not in self.releases and track != "internal":
                return 404, json.dumps({"error": {"code": 404, "message": "Track not found."}}).encode()
            return 200, json.dumps({"releases": self.releases.get(track, [])}).encode()
        if method == "POST" and path == EDITS:
            return 200, b'{"id": "edit-1", "expiryTimeSeconds": "9999999999"}'
        if method == "GET" and path.endswith("/tracks"):
            return 200, json.dumps({"tracks": self.tracks}).encode()
        if method == "GET" and path.endswith("/bundles"):
            return 200, json.dumps({"bundles": self.bundles}).encode()
        if method == "POST" and path.endswith("/bundles"):
            sha = self.upload_sha or hashlib.sha256(data).hexdigest()
            return 200, json.dumps({"versionCode": self.version_code, "sha256": sha}).encode()
        if method == "PUT" and "/tracks/" in path:
            return 200, data
        if method == "POST" and (path.endswith(":commit") or path.endswith(":validate")):
            return 200, b'{"id": "edit-1"}'
        if method == "DELETE":
            return 204, b""
        raise AssertionError(f"unexpected call {method} {url}")

    def names(self):
        return [f"{m} {u.split('?')[0].rsplit('/', 1)[-1]}" for m, u, _, _ in self.calls]


class PlayUploadTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = pathlib.Path(self.tmp.name)
        self.aab = self.dir / "app-release.aab"
        make_aab(self.aab)
        self.sha = hashlib.sha256(self.aab.read_bytes()).hexdigest()
        self.notes = self.dir / "notes.txt"
        self.notes.write_text("HJViewer 1.6.0 (262851234)\n소스: https://github.com/yonsei-projecttf/hjviewer\n",
                              encoding="utf-8")
        sleep = mock.patch.object(play.time, "sleep", lambda s: None)
        sleep.start()
        self.addCleanup(sleep.stop)
        self.addCleanup(self.tmp.cleanup)

    def run_script(self, fake, extra=(), env=None, mode="commit"):
        # the tests from before the store run use the internal track; extra may name another
        argv = ["--package", PKG, "--aab", str(self.aab), "--name", "1.6.0 (262851234)",
                "--notes-file", str(self.notes), "--mode", mode, "--track", "internal", *extra]
        env = {"PLAY_ACCESS_TOKEN": TOKEN} if env is None else env
        out = io.StringIO()
        with mock.patch.object(play, "send", fake), contextlib.redirect_stdout(out), \
                contextlib.redirect_stderr(out):
            code = play.main(argv, env)
        text = out.getvalue()
        self.assertNotIn(TOKEN, text)  # the token is never printed
        for value in env.values():
            if len(value) > 20:
                self.assertNotIn(value.strip()[:40], text)
        return code, text

    def test_dry_run_needs_no_network_or_credentials(self):
        code, text = self.run_script(no_network, env={}, mode="dry-run")
        self.assertEqual(code, 0, text)
        self.assertIn("nothing was sent", text)
        self.assertIn('"status": "completed"', text)
        self.assertIn("ko-KR", text)

    def test_dry_run_with_a_key_proves_it_signs_offline(self):
        code, text = self.run_script(no_network, mode="dry-run",
                                     env={"PLAY_SA_EMAIL": SA_EMAIL, "PLAY_SA_PRIVATE_KEY": throwaway_key()})
        self.assertEqual(code, 0, text)
        self.assertIn("usable by openssl", text)
        self.assertNotIn("PRIVATE KEY", text)

    def test_key_pasted_with_escaped_line_breaks(self):
        escaped = throwaway_key().strip().replace("\n", "\\n")  # the JSON file's form, pasted as is
        code, text = self.run_script(no_network, mode="dry-run",
                                     env={"PLAY_SA_EMAIL": SA_EMAIL, "PLAY_SA_PRIVATE_KEY": escaped})
        self.assertEqual(code, 0, text)
        self.assertIn("usable by openssl", text)

    def test_key_that_is_not_pem_is_refused(self):
        marker = "PRIVATE" + " KEY-----"  # split, so no secret scanner takes the fixture for a key
        big = f"-----BEGIN {marker}\n" + "A" * 20000 + f"\n-----END {marker}\n"
        code, text = self.run_script(no_network, mode="dry-run",
                                     env={"PLAY_SA_EMAIL": SA_EMAIL, "PLAY_SA_PRIVATE_KEY": big})
        self.assertEqual(code, 2)
        self.assertIn("not a PEM private key", text)

    def test_unsigned_bundle_is_refused_before_any_call(self):
        make_aab(self.aab, signed=False)
        code, text = self.run_script(FakePlay(), mode="dry-run")
        self.assertEqual(code, 2)
        self.assertIn("not signed", text)

    def test_long_notes_are_refused(self):
        self.notes.write_text("가" * 501, encoding="utf-8")
        code, text = self.run_script(FakePlay(), mode="dry-run")
        self.assertEqual(code, 2)
        self.assertIn("501 characters", text)

    def test_notes_must_be_utf8(self):
        self.notes.write_bytes("소스 코드".encode("euc-kr"))
        code, text = self.run_script(FakePlay(), mode="dry-run")
        self.assertEqual(code, 2)
        self.assertIn("not UTF-8", text)

    def test_upload_releases_and_commits(self):
        fake = FakePlay()
        code, text = self.run_script(fake, extra=["--version-code", "262851234"])
        self.assertEqual(code, 0, text)
        self.assertEqual(fake.names(), ["POST edits", "GET tracks", "GET bundles", "POST bundles",
                                        "PUT internal", "POST edit-1:commit"])
        method, url, headers, data = fake.calls[3]
        self.assertIn("/upload/androidpublisher/v3/", url)
        self.assertIn("uploadType=media", url)
        self.assertEqual(headers["Content-Type"], "application/octet-stream")
        self.assertEqual(headers["Authorization"], "Bearer " + TOKEN)
        body = json.loads(fake.calls[4][3])
        self.assertEqual(body["releases"], [{
            "name": "1.6.0 (262851234)", "versionCodes": ["262851234"], "status": "completed",
            "releaseNotes": [{"language": "ko-KR", "text": self.notes.read_text(encoding="utf-8").strip()}]}])
        self.assertIn("changesInReviewBehavior=ERROR_IF_IN_REVIEW", fake.calls[5][1])
        self.assertIn("Uploaded kr.ac.yonsei.hjviewer versionCode 262851234", text)

    def test_rerun_of_a_released_bundle_changes_nothing(self):
        fake = FakePlay(bundles=[{"versionCode": 262851234, "sha256": self.sha}],
                        tracks=[{"track": "internal", "releases": [
                            {"versionCodes": ["262851234"], "status": "completed"}]}])
        code, text = self.run_script(fake)
        self.assertEqual(code, 0, text)
        self.assertEqual(fake.names(), ["POST edits", "GET tracks", "GET bundles", "DELETE edit-1"])
        self.assertIn("already released", text)

    def test_uploaded_but_unreleased_bundle_is_released_without_uploading_again(self):
        fake = FakePlay(bundles=[{"versionCode": 262851234, "sha256": self.sha}])
        code, text = self.run_script(fake)
        self.assertEqual(code, 0, text)
        self.assertNotIn("POST bundles", fake.names())
        self.assertIn("PUT internal", fake.names())

    def test_version_code_taken(self):
        fake = FakePlay(fail={("POST", "/bundles"): (403, {"error": {
            "code": 403, "message": "APK specifies a version code that has already been used.",
            "status": "PERMISSION_DENIED", "errors": [{"reason": "apkUpgradeVersionConflict"}]}})})
        code, text = self.run_script(fake)
        self.assertEqual(code, 1)
        self.assertIn("versionCode is taken", text)
        self.assertEqual(fake.names()[-1], "DELETE edit-1")

    def test_bundle_older_than_the_track_serves_is_refused_before_the_upload(self):
        # "Re-run failed jobs" on an older run after a newer run has released: the jobs run in the
        # order they arrive, and Play may accept a release that no tester can install
        fake = FakePlay(tracks=[{"track": "internal", "releases": [
            {"versionCodes": ["262851000"], "status": "halted"},
            {"versionCodes": ["262851300"], "status": "completed"}]}])
        code, text = self.run_script(fake, extra=["--version-code", "262851234"])
        self.assertEqual(code, 1)
        self.assertIn("Play already serves versionCode 262851300 on internal", text)
        self.assertIn("(262851234) is older", text)
        self.assertIn("Start a new build", text)
        self.assertEqual(fake.names(), ["POST edits", "GET tracks", "GET bundles", "DELETE edit-1"])

    def test_older_bundle_is_refused_after_the_upload_when_no_version_code_was_given(self):
        fake = FakePlay(tracks=[{"track": "internal", "releases": [
            {"versionCodes": ["262851300"], "status": "completed"}]}])
        code, text = self.run_script(fake)  # Play's answer is the first to say 262851234
        self.assertEqual(code, 1)
        self.assertIn("Play already serves versionCode 262851300 on internal", text)
        self.assertEqual(fake.names(), ["POST edits", "GET tracks", "GET bundles", "POST bundles",
                                        "DELETE edit-1"])

    def test_older_bundle_that_play_still_holds_is_not_released_again(self):
        fake = FakePlay(bundles=[{"versionCode": 262851234, "sha256": self.sha}],
                        tracks=[{"track": "internal", "releases": [
                            {"versionCodes": ["262851300"], "status": "completed"}]}])
        for mode in ("commit", "validate"):
            fake.calls.clear()
            code, text = self.run_script(fake, extra=["--version-code", "262851234"], mode=mode)
            self.assertEqual(code, 1, mode)
            self.assertIn("is older", text)
            self.assertEqual(fake.names(), ["POST edits", "GET tracks", "GET bundles", "DELETE edit-1"])

    def test_only_what_the_track_serves_counts_as_newer(self):
        # a higher draft on the track, a higher release on another track and an older release here
        fake = FakePlay(tracks=[
            {"track": "production", "releases": [{"versionCodes": ["999999999"], "status": "completed"}]},
            {"track": "internal", "releases": [{"versionCodes": ["262851300"], "status": "draft"},
                                               {"versionCodes": ["262851000", "262850000"],
                                                "status": "completed"}]}])
        code, text = self.run_script(fake, extra=["--version-code", "262851234"])
        self.assertEqual(code, 0, text)
        self.assertIn("POST edit-1:commit", fake.names())

    def test_the_renamed_track_is_the_one_compared(self):
        fake = FakePlay(tracks=[{"track": "qa", "releases": [
            {"versionCodes": ["262851300"], "status": "inProgress"}]}])
        code, text = self.run_script(fake, extra=["--version-code", "262851234"])
        self.assertEqual(code, 1)
        self.assertIn("Play already serves versionCode 262851300 on qa", text)

    def test_same_version_code_is_left_to_play(self):
        # another bundle with this versionCode: Play refuses the upload itself (test_version_code_taken)
        fake = FakePlay(tracks=[{"track": "internal", "releases": [
            {"versionCodes": ["262851234"], "status": "completed"}]}])
        code, text = self.run_script(fake, extra=["--version-code", "262851234"])
        self.assertEqual(code, 0, text)
        self.assertIn("POST bundles", fake.names())

    def test_app_not_created(self):
        fake = FakePlay(fail={("POST", "/edits"): (404, {"error": {
            "code": 404, "message": "Package not found: kr.ac.yonsei.hjviewer.", "status": "NOT_FOUND"}})})
        code, text = self.run_script(fake)
        self.assertEqual(code, 1)
        self.assertIn("upload the first bundle there by hand", text)

    def test_lost_edit_is_not_taken_for_a_missing_app(self):
        fake = FakePlay(fail={("POST", ":commit"): (404, {"error": {
            "code": 404, "message": "This Edit has been deleted.", "status": "NOT_FOUND"}})})
        code, text = self.run_script(fake)
        self.assertEqual(code, 1)
        self.assertIn("run it again", text)
        self.assertNotIn("by hand", text)

    def test_no_permission(self):
        fake = FakePlay(fail={("POST", "/edits"): (403, {"error": {
            "code": 403, "message": "The caller does not have permission", "status": "PERMISSION_DENIED"}})})
        code, text = self.run_script(fake)
        self.assertEqual(code, 1)
        self.assertIn("Release apps to testing tracks", text)

    def advice(self, status, message, words=""):
        return play.hint(play.PlayError(status, message, words),
                         argparse.Namespace(package=PKG, notes_language="ko-KR"))

    def test_declaration_answer_is_not_taken_for_a_missing_permission(self):
        # what a missing foreground service declaration produced on an internal track (403, and the
        # message says "permissions"): the service account is fine, App content is not finished
        fake = FakePlay(fail={("POST", ":commit"): (403, {"error": {
            "code": 403, "message": "You must let us know whether your app uses any Foreground Service "
                                    "permissions.",
            "status": "PERMISSION_DENIED", "errors": [{"reason": "forbidden"}]}})})
        code, text = self.run_script(fake)
        self.assertEqual(code, 1)
        self.assertIn("Play Console > App content", text)
        self.assertIn("Nothing was committed", text)
        self.assertNotIn("PLAY_SA_EMAIL", text)
        self.assertNotIn("Release apps to testing tracks", text)
        self.assertEqual(fake.names()[-1], "DELETE edit-1")

    def test_policy_answers_point_at_app_content(self):
        # worded after reports of what Play answers for a bundle with the camera permission and no
        # privacy policy; what the rule reads are the words of DECLARATION_WORDS, each tried below
        policy = ("Your app has an APK with version code 262851234 using permissions that require a privacy "
                  "policy: (android.permission.CAMERA).")
        for status, message in [(403, policy), (400, policy)] + [
                (403, f"Play refused the release: {word} is not finished.") for word in play.DECLARATION_WORDS]:
            advice = self.advice(status, message, "PERMISSION_DENIED forbidden")
            self.assertIn("Play Console > App content", advice, message)
            self.assertNotIn("PLAY_SA_EMAIL", advice, message)

    def test_release_older_than_the_track_has_its_own_advice(self):
        # wordings Play has for a release that no tester would get, should one get past
        # refuse_older (which compares only this track's releases that are not drafts)
        for message in ("You can't rollout this release because it doesn't allow any existing users to "
                        "upgrade to the newly added app bundles.",
                        "Version 262851234 of this app can not be downloaded by any devices as they will all "
                        "receive APKs with higher version codes.",
                        "This app bundle is fully shadowed by one or more app bundles of the release."):
            advice = self.advice(403, message, "PERMISSION_DENIED forbidden")
            self.assertIn("already serves a newer versionCode", advice, message)
            self.assertNotIn("PLAY_SA_EMAIL", advice, message)

    def test_permission_advice_only_for_answers_about_the_account(self):
        account = "Release apps to testing tracks"
        self.assertIn(account, self.advice(403, "The caller does not have permission", "PERMISSION_DENIED"))
        self.assertIn(account, self.advice(401, "The current user has insufficient permissions to perform "
                                                "the requested operation.", "permissionDenied"))
        self.assertIn(account, self.advice(403, "The current user has insufficient permissions to perform "
                                                "the requested operation.", "forbidden"))
        self.assertIn(account, self.advice(401, "Request had invalid authentication credentials.",
                                           "UNAUTHENTICATED"))
        # a 403 that says nothing about the account gets no guess
        for message in ("Something Play has not said before.", "This release is not compliant with a permission "
                                                               "policy."):
            self.assertEqual(self.advice(403, message, "PERMISSION_DENIED forbidden"),
                             "See Google's message above.", message)

    def test_earlier_advice_keeps_its_place(self):
        # answers that also arrive as 403 PERMISSION_DENIED and were recognised before
        denied = "PERMISSION_DENIED"
        self.assertIn("versionCode is taken", self.advice(
            403, "APK specifies a version code that has already been used.", denied + " apkUpgradeVersionConflict"))
        self.assertIn("still a draft", self.advice(
            403, "Only releases with status draft may be created on draft app.", denied))
        self.assertIn("Publishing overview", self.advice(403, "Changes are in review.", denied))
        self.assertIn("upload key", self.advice(
            403, "The Android App Bundle was signed with the wrong key.", denied))
        self.assertIn("store listing languages", self.advice(
            400, "Release notes are in a language the store listing does not have.", "INVALID_ARGUMENT"))
        self.assertIn("Developer API", self.advice(
            403, "Google Play Android Developer API has not been used in project 123 before or it is disabled.",
            denied + " accessNotConfigured"))

    def test_unanswered_commit_may_have_gone_through(self):
        for failure in (TimeoutError("timed out"), (503, {"error": {"code": 503, "message": "Backend Error"}})):
            fake = FakePlay()

            def commit_fails(method, url, headers, data, timeout, fake=fake, failure=failure):
                if method == "POST" and url.split("?")[0].endswith(":commit"):
                    fake.calls.append((method, url, headers, data))
                    if isinstance(failure, Exception):
                        raise failure
                    return failure[0], json.dumps(failure[1]).encode()
                return fake(method, url, headers, data, timeout)

            code, text = self.run_script(commit_fails)
            self.assertEqual(code, 1)
            self.assertIn("may or may not have gone through", text)
            self.assertIn("run the same upload again", text)
            self.assertNotIn("othing was committed", text)
            self.assertNotIn("nothing committed", text)
            self.assertIn("deleted edit edit-1 (it was still open)", text)
            self.assertEqual(fake.names()[-2:], ["POST edit-1:commit", "DELETE edit-1"])  # no second commit

    def test_refused_commit_committed_nothing(self):
        fake = FakePlay(fail={("POST", ":commit"): (403, {"error": {
            "code": 403, "message": "Changes cannot be sent for review automatically.",
            "status": "PERMISSION_DENIED", "errors": [{"reason": "changesNotSentForReview"}]}})})
        code, text = self.run_script(fake)
        self.assertEqual(code, 1)
        self.assertIn("Nothing was committed", text)
        self.assertIn("deleted edit edit-1 (nothing committed)", text)
        self.assertNotIn("may or may not", text)

    def test_unanswered_call_before_the_commit_committed_nothing(self):
        fake = FakePlay(fail={("PUT", "/internal"): (503, {"error": {"code": 503, "message": "Backend Error"}})})
        code, text = self.run_script(fake)
        self.assertEqual(code, 1)
        self.assertIn("run it again later. Nothing was committed", text)
        self.assertIn("deleted edit edit-1 (nothing committed)", text)

    def test_qa_track_name(self):
        fake = FakePlay(tracks=[{"track": "production"}, {"track": "qa"}])
        code, text = self.run_script(fake)
        self.assertEqual(code, 0, text)
        self.assertIn("PUT qa", fake.names())

    def test_missing_track(self):
        fake = FakePlay(tracks=[{"track": "production"}])
        code, text = self.run_script(fake)
        self.assertEqual(code, 2)
        self.assertIn("has no track", text)
        self.assertEqual(fake.names()[-1], "DELETE edit-1")

    def test_draft_app(self):
        fake = FakePlay(fail={("POST", ":commit"): (400, {"error": {
            "code": 400, "message": "Only releases with status draft may be created on draft app.",
            "status": "INVALID_ARGUMENT"}})})
        code, text = self.run_script(fake)
        self.assertEqual(code, 1)
        self.assertIn("still a draft", text)
        self.assertEqual(fake.names()[-1], "DELETE edit-1")

    def test_draft_status_is_sent(self):
        fake = FakePlay()
        code, text = self.run_script(fake, extra=["--status", "draft"])
        self.assertEqual(code, 0, text)
        self.assertEqual(json.loads(fake.calls[4][3])["releases"][0]["status"], "draft")
        self.assertIn("Roll the draft out in Play Console", text)

    def test_validate_mode_releases_nothing(self):
        fake = FakePlay()
        code, text = self.run_script(fake, mode="validate")
        self.assertEqual(code, 0, text)
        self.assertEqual(fake.names()[-2:], ["POST edit-1:validate", "DELETE edit-1"])
        self.assertNotIn("POST edit-1:commit", fake.names())

    def test_sha_mismatch(self):
        fake = FakePlay(upload_sha="0" * 64)
        code, text = self.run_script(fake)
        self.assertEqual(code, 1)
        self.assertIn("sha256 mismatch", text)
        self.assertEqual(fake.names()[-1], "DELETE edit-1")

    def test_expected_version_code(self):
        fake = FakePlay(version_code=262851235)
        code, text = self.run_script(fake, extra=["--version-code", "262851234"])
        self.assertEqual(code, 2)
        self.assertIn("expected 262851234", text)
        self.assertIn("not the one the build reported", text)
        self.assertEqual(fake.names()[-1], "DELETE edit-1")

    def test_transient_error_is_retried(self):
        fake = FakePlay(fail={("POST", "/edits"): ([(503, {"error": {"code": 503, "message": "busy"}}),
                                                    (0, None)], None)})
        code, text = self.run_script(fake)
        self.assertEqual(code, 0, text)
        self.assertEqual(fake.names()[:2], ["POST edits", "POST edits"])

    def test_cut_off_answer_is_retried(self):
        fake = FakePlay()
        failures = [http.client.RemoteDisconnected("Remote end closed connection without response"),
                    http.client.IncompleteRead(b"{", 10)]

        def flaky(method, url, headers, data, timeout):
            if method == "POST" and url == EDITS and failures:
                fake.calls.append((method, url, headers, data))
                raise failures.pop(0)
            return fake(method, url, headers, data, timeout)

        code, text = self.run_script(flaky)
        self.assertEqual(code, 0, text)
        self.assertEqual(fake.names()[:4], ["POST edits", "POST edits", "POST edits", "GET tracks"])

    def test_answer_without_a_version_code(self):
        fake = FakePlay()

        def no_version(method, url, headers, data, timeout):
            if method == "POST" and url.split("?")[0].endswith("/bundles"):
                fake.calls.append((method, url, headers, data))
                return 200, json.dumps({"sha256": self.sha}).encode()
            return fake(method, url, headers, data, timeout)

        code, text = self.run_script(no_version)
        self.assertEqual(code, 1)
        self.assertIn("no field 'versionCode'", text)
        self.assertEqual(fake.names()[-1], "DELETE edit-1")

    def test_answer_that_is_not_json(self):
        fake = FakePlay()

        def html(method, url, headers, data, timeout):
            if method == "GET" and url.endswith("/tracks"):
                fake.calls.append((method, url, headers, data))
                return 200, b"<html>maintenance page</html>"
            return fake(method, url, headers, data, timeout)

        code, text = self.run_script(html)
        self.assertEqual(code, 1)
        self.assertIn("not a JSON object", text)
        self.assertNotIn("maintenance", text)  # an answer's body is not repeated
        self.assertEqual(fake.names()[-1], "DELETE edit-1")

    def test_token_with_a_line_break_is_refused_without_printing_it(self):
        code, text = self.run_script(no_network, env={"PLAY_ACCESS_TOKEN": "ya29.secret-value\nX-Extra: 1"})
        self.assertEqual(code, 2)
        self.assertIn("PLAY_ACCESS_TOKEN is not a usable access token", text)
        self.assertNotIn("secret-value", text)

    def test_token_with_a_trailing_line_break_works(self):
        fake = FakePlay()
        code, text = self.run_script(fake, env={"PLAY_ACCESS_TOKEN": TOKEN + "\n"})
        self.assertEqual(code, 0, text)
        self.assertEqual(fake.calls[0][2]["Authorization"], "Bearer " + TOKEN)

    def test_service_account_key_signs_a_valid_jwt(self):
        key, pub = self.dir / "throwaway.pem", self.dir / "throwaway.pub"
        subprocess.run(["openssl", "genpkey", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048",
                        "-out", str(key)], check=True, capture_output=True)
        subprocess.run(["openssl", "pkey", "-in", str(key), "-pubout", "-out", str(pub)], check=True,
                       capture_output=True)
        pem = key.read_text()
        key.unlink()  # from here on only the environment holds it, as in CI

        def token_check(form):
            fields = urllib.parse.parse_qs(form.decode())
            self.assertEqual(fields["grant_type"], ["urn:ietf:params:oauth:grant-type:jwt-bearer"])
            header, claims, signature = fields["assertion"][0].split(".")
            pad = lambda s: s + "=" * (-len(s) % 4)
            claims_json = json.loads(play.base64.urlsafe_b64decode(pad(claims)))
            self.assertEqual(claims_json["iss"], SA_EMAIL)
            self.assertEqual(claims_json["aud"], play.TOKEN_URL)
            self.assertEqual(claims_json["scope"], play.SCOPE)
            self.assertLessEqual(claims_json["exp"] - claims_json["iat"], 3600)
            (self.dir / "sig").write_bytes(play.base64.urlsafe_b64decode(pad(signature)))
            (self.dir / "msg").write_bytes(f"{header}.{claims}".encode())
            verified = subprocess.run(["openssl", "dgst", "-sha256", "-verify", str(pub), "-signature",
                                       str(self.dir / "sig"), str(self.dir / "msg")], capture_output=True)
            self.assertEqual(verified.returncode, 0, verified.stdout)
            return {"access_token": TOKEN, "expires_in": 3599, "token_type": "Bearer"}

        fake = FakePlay()
        fake.token_check = token_check
        code, text = self.run_script(fake, env={"PLAY_SA_EMAIL": SA_EMAIL, "PLAY_SA_PRIVATE_KEY": pem})
        self.assertEqual(code, 0, text)
        self.assertNotIn("PRIVATE KEY", text)
        self.assertEqual(fake.calls[1][2]["Authorization"], "Bearer " + TOKEN)

    def test_rejected_key(self):
        fake = FakePlay(fail={("POST", "/token"): (400, {"error": "invalid_grant",
                                                         "error_description": "Invalid JWT Signature."})})
        fake.token_check = lambda form: {}
        code, text = self.run_script(fake, env={"PLAY_SA_EMAIL": SA_EMAIL, "PLAY_SA_PRIVATE_KEY": throwaway_key()})
        self.assertEqual(code, 1)
        self.assertIn("refused the service account key", text)


def update_entries(manifest: str) -> list:
    """The in-app update's entries in manifest text, as "<kind> <name>" lines of PLAY_FORBIDDEN:
    a provider whose element names the update's paths (res/xml/hjv_update_paths.xml, minimum
    version design 5-4), with a short class name completed by the app's namespace, and the
    install permission."""
    entries = []
    # a provider element: self-closing, or up to its end tag (its meta-data may come in any order)
    for match in re.finditer(r"<provider\b(?:[^>]*/>|.*?</provider>)", manifest, re.S):
        block = match.group(0)
        if "hjv_update" in block:
            name = re.search(r'android:name="([^"]+)"', block).group(1)
            entries.append("provider " + ("com.carriez.flutter_hbb" + name if name.startswith(".") else name))
    if re.search(r'<uses-permission(?:-sdk-23)?\s+android:name="android\.permission\.REQUEST_INSTALL_PACKAGES"',
                 manifest):
        entries.append("permission android.permission.REQUEST_INSTALL_PACKAGES")
    return entries


def summary_of(state, *codes, name="1.9.0"):
    """A ReleaseSummary as applications.tracks.releases.list gives it."""
    return {"releaseName": name, "track": "alpha", "releaseLifecycleState": "RELEASE_LIFECYCLE_STATE_" + state,
            "activeArtifacts": [{"versionCode": code} for code in codes]}


class PlayStoreRunTest(unittest.TestCase):
    """The store run's upload (auto-deploy design 7-2): the closed track alpha, the release states read
    with releases.list before any edit, and the commit's changesInReviewBehavior always named."""

    setUp = PlayUploadTest.setUp
    run_script = PlayUploadTest.run_script

    def run_alpha(self, fake, extra=(), mode="commit"):
        return self.run_script(fake, extra=["--track", "alpha", "--version-code", "262851234", *extra], mode=mode)

    def alpha(self, releases, **kw):
        return FakePlay(tracks=[{"track": "alpha", "releases": []}, {"track": "internal"}],
                        releases={"alpha": releases}, **kw)

    def test_the_default_track_is_alpha(self):
        self.assertEqual(play.TRACK, "alpha")
        args = argparse.Namespace()
        with mock.patch.object(play, "upload", lambda a, env: setattr(args, "track", a.track) or 0):
            play.main(["--package", PKG, "--aab", "x.aab"], {})
        self.assertEqual(args.track, "alpha")

    def test_upload_to_alpha_reads_the_releases_before_the_edit(self):
        fake = self.alpha([summary_of("PUBLISHED", 262932441)])
        code, text = self.run_alpha(fake)
        self.assertEqual(code, 0, text)
        self.assertEqual(fake.listed, ["alpha", "internal"])
        self.assertEqual(fake.names(), ["POST edits", "GET tracks", "GET bundles", "POST bundles",
                                        "PUT alpha", "POST edit-1:commit"])
        self.assertIn("changesInReviewBehavior=ERROR_IF_IN_REVIEW", fake.calls[-1][1])
        self.assertIn("Google reviews it first", text)

    def test_a_review_in_progress_stops_before_any_edit(self):
        for track in ("alpha", "internal"):
            releases = {"alpha": [summary_of("PUBLISHED", 262932441)]}
            releases[track] = releases.get(track, []) + [summary_of("IN_REVIEW", 262851000, name="1.8.9")]
            fake = FakePlay(tracks=[{"track": "alpha"}, {"track": "internal"}], releases=releases)
            code, text = self.run_alpha(fake)
            self.assertEqual(code, 1, text)
            self.assertIn(f"{track} has 1.8.9 IN_REVIEW", text)
            self.assertIn("in_review=cancel", text)
            self.assertEqual(fake.names(), [], "an edit was opened")

    def test_in_review_cancel_goes_on_and_says_so_in_the_commit(self):
        fake = self.alpha([summary_of("IN_REVIEW", 262851000, name="1.8.9")])
        code, text = self.run_alpha(fake, extra=["--in-review", "cancel"])
        self.assertEqual(code, 0, text)
        self.assertIn("cancels that review", text)
        self.assertIn("changesInReviewBehavior=CANCEL_IN_REVIEW_AND_SUBMIT", fake.calls[-1][1])

    def test_rejected_or_unsent_changes_stop_with_a_console_hint(self):
        for state in ("NOT_APPROVED", "NOT_SENT_FOR_REVIEW"):
            for in_review in ("error", "cancel"):
                fake = self.alpha([summary_of(state, 262851000, name="1.8.9")])
                code, text = self.run_alpha(fake, extra=["--in-review", in_review])
                self.assertEqual(code, 1, text)
                self.assertIn(f"1.8.9 {state}", text)
                self.assertIn("Publishing overview", text)
                self.assertEqual(fake.names(), [])

    def test_a_version_code_play_has_already_ends_the_run(self):
        # a re-run after a commit whose answer was lost: in review, approved or published
        for state in ("IN_REVIEW", "APPROVED_NOT_PUBLISHED", "PUBLISHED"):
            fake = self.alpha([summary_of(state, 262851234, name="1.9.1")])
            code, text = self.run_alpha(fake)
            self.assertEqual(code, 0, text)
            self.assertIn(f"versionCode 262851234 is there already ({state})", text)
            self.assertEqual(fake.names(), [])
        # a draft of it is not a release: the upload goes on
        fake = self.alpha([summary_of("DRAFT", 262851234, name="1.9.1")])
        code, text = self.run_alpha(fake)
        self.assertEqual(code, 0, text)
        self.assertIn("POST edit-1:commit", fake.names())

    def test_a_track_the_app_lacks(self):
        fake = FakePlay(releases={})
        code, text = self.run_alpha(fake)
        self.assertEqual(code, 2, text)
        self.assertIn("no track 'alpha'", text)
        # internal may be missing (or be called qa): that is no reason to stop
        fake = FakePlay(tracks=[{"track": "alpha"}, {"track": "qa"}], releases={"alpha": []},
                        fail={("GET", "/tracks/internal/releases"): (404, {"error": {"code": 404}})})
        code, text = self.run_alpha(fake)
        self.assertEqual(code, 0, text)

    def test_every_commit_names_its_behaviour(self):
        for extra, behaviour in (((), "ERROR_IF_IN_REVIEW"), (("--in-review", "cancel"), "CANCEL_IN_REVIEW_AND_SUBMIT")):
            fake = self.alpha([])
            self.assertEqual(self.run_alpha(fake, extra=extra)[0], 0)
            (commit,) = [u for m, u, _, _ in fake.calls if u.split("?")[0].endswith(":commit")]
            self.assertEqual(urllib.parse.parse_qs(urllib.parse.urlsplit(commit).query),
                             {"changesInReviewBehavior": [behaviour]})

    def test_a_bundle_play_holds_is_not_uploaded_again(self):
        fake = self.alpha([], bundles=[{"versionCode": 262851234, "sha256": self.sha}])
        code, text = self.run_alpha(fake)
        self.assertEqual(code, 0, text)
        self.assertNotIn("POST bundles", fake.names())
        self.assertIn("PUT alpha", fake.names())

    def test_dry_run_plans_the_release_list_and_the_commit(self):
        code, text = self.run_script(no_network, env={}, mode="dry-run",
                                     extra=["--track", "alpha", "--in-review", "cancel"])
        self.assertEqual(code, 0, text)
        self.assertIn("tracks/alpha/releases", text)
        self.assertIn("tracks/internal/releases", text)
        self.assertIn("changesInReviewBehavior=CANCEL_IN_REVIEW_AND_SUBMIT", text)
        code, text = self.run_script(no_network, env={}, mode="dry-run", extra=["--track", "alpha"])
        self.assertIn("changesInReviewBehavior=ERROR_IF_IN_REVIEW", text)

    def test_status_reads_the_releases_without_an_edit_or_a_bundle(self):
        fake = self.alpha([summary_of("PUBLISHED", 262932441), summary_of("IN_REVIEW", 262851234, name="1.9.1")])
        report = self.dir / "status.json"
        out = io.StringIO()
        with mock.patch.object(play, "send", fake), contextlib.redirect_stdout(out):
            code = play.main(["--package", PKG, "--mode", "status", "--version-code", "262851234",
                              "--json", str(report)], {"PLAY_ACCESS_TOKEN": TOKEN})
        text = out.getvalue()
        self.assertEqual(code, 0, text)
        self.assertEqual(fake.names(), [])
        self.assertEqual(fake.listed, ["alpha", "internal"])
        self.assertIn("Google Play alpha: versionCode 262851234 IN_REVIEW", text)
        self.assertIn("alpha: 1.9.0, versionCode 262932441, PUBLISHED", text)
        self.assertNotIn(TOKEN, text)
        data = json.loads(report.read_text(encoding="utf-8"))
        self.assertEqual(data["tracks"]["alpha"][0], {"name": "1.9.0", "versionCodes": [262932441], "state": "PUBLISHED"})
        self.assertEqual(data["tracks"]["internal"], [])
        self.assertEqual(data["versionCode"], {"code": 262851234, "states": ["IN_REVIEW"]})

    def test_other_modes_need_the_bundle(self):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = play.main(["--package", PKG, "--mode", "commit"], {"PLAY_ACCESS_TOKEN": TOKEN})
        self.assertEqual(code, 2)
        self.assertIn("needs --aab", out.getvalue())


class PlayBundleListsTest(unittest.TestCase):
    """What a run can only show after a 20-minute build: the lists of "Sign and check Play bundle"
    and the Play-only manifest overlay must say the same thing, the overlay must never reach the
    side-loaded APK, and a bundle that failed a step must go nowhere."""

    WORKFLOW = HERE.parent / ".github" / "workflows" / "client-build.yml"
    OVERLAY = HERE.parent / "client" / "play" / "AndroidManifest.xml"
    ANDROID = "{http://schemas.android.com/apk/res/android}"
    TOOLS = "{http://schemas.android.com/tools}"
    # the in-app update of the side-loaded APK installs an APK: an app from Google Play must not
    SIDE_LOAD_ONLY = {"permission android.permission.REQUEST_INSTALL_PACKAGES",
                      "provider com.carriez.flutter_hbb.HjvUpdateFileProvider"}
    # The lines that decide what a failed bundle step does (docs/ci.md, the Google Play section, step
    # 1), which only a failing run on GitHub shows at work. Without `play` and outside a check build a
    # failed bundle step leaves the job green and "Report a failed Play bundle" says that the run has
    # no bundle; with either it fails the job. Only a bundle that passed every check becomes the
    # -play artifact, which the store run uploads (store-release.yml) and which the first
    # upload in Play Console takes by hand.
    BUNDLE_STEPS = {
        "Build Play bundle": {
            "if": "env.ANDROID_SIGNING == 'true' && github.event.repository.private",
            "id": "play-build",
            "continue-on-error": "${{ !inputs.play && !inputs.check_only }}"},
        "Sign and check Play bundle": {
            "if": "steps.play-build.outcome == 'success'",
            "id": "play-bundle",
            "continue-on-error": "${{ !inputs.play && !inputs.check_only }}"},
        "Upload Play bundle": {"if": "steps.play-bundle.outcome == 'success'"},
        # no status function: GitHub adds success(), which is false once a failed step failed the job
        "Report a failed Play bundle": {
            "if": "steps.play-build.outcome == 'failure' || steps.play-bundle.outcome == 'failure'"},
        # the keystore that "Sign and check Play bundle" decodes, also when a step before failed
        "Remove the upload keystore": {"if": "always() && env.ANDROID_SIGNING == 'true'"},
    }

    def setUp(self):
        self.workflow = self.WORKFLOW.read_text(encoding="utf-8")
        self.overlay = xml.etree.ElementTree.parse(self.OVERLAY).getroot()

    def block(self, name):
        """The lines of the step's `NAME: |` environment value."""
        lines = self.workflow.splitlines()
        (start,) = [i for i, line in enumerate(lines) if line.strip() == f"{name}: |"]
        indent = len(lines[start]) - len(lines[start].lstrip())
        value = []
        for line in lines[start + 1:]:
            if line.strip() and len(line) - len(line.lstrip()) <= indent:
                break
            if line.strip():
                value.append(line.strip())
        return value

    def job_steps(self, job):
        """The steps of the job in their order, each as {key: the text after the colon on the key's
        line} for the step's own keys (a block value such as `run: |` gives "|"), read as text like
        block()."""
        lines = self.workflow.splitlines()
        (start,) = [i for i, line in enumerate(lines) if line == f"  {job}:"]
        steps, in_steps = [], False
        for line in lines[start + 1:]:
            text, indent = line.strip(), len(line) - len(line.lstrip())
            if not text or text.startswith("#"):
                continue
            if indent <= 2:  # the next job
                break
            if indent == 4:
                in_steps = text == "steps:"
            elif in_steps and indent in (6, 8):
                if indent == 6:
                    self.assertTrue(text.startswith("- "), line)
                    steps.append({})
                    text = text[2:]
                key, colon, value = text.partition(":")
                self.assertTrue(colon and re.fullmatch(r"[a-z-]+", key), line)
                self.assertNotIn(key, steps[-1], f"a step of the {job} job has {key} twice")
                steps[-1][key] = value.strip()
        return steps

    def removed_by_the_overlay(self):
        kinds = {"uses-permission": "permission", "uses-permission-sdk-23": "permission", "provider": "provider"}
        removed = set()
        for element in self.overlay.iter():
            if element.get(self.TOOLS + "node") is not None:
                self.assertEqual(element.get(self.TOOLS + "node"), "remove", element.tag)
                self.assertIn(element.tag, kinds, "the bundle check has no list for this kind of entry")
                removed.add(f"{kinds[element.tag]} {element.get(self.ANDROID + 'name')}")
        return removed

    def test_side_load_only_entries_are_forbidden_and_removed(self):
        forbidden = self.block("PLAY_FORBIDDEN")
        self.assertEqual(len(forbidden), len(set(forbidden)))
        self.assertLessEqual(self.SIDE_LOAD_ONLY, set(forbidden))
        # the overlay takes out what the check forbids, name for name: a name misspelt in one of the
        # two would leave the entry in the bundle or leave it unchecked
        self.assertEqual(self.removed_by_the_overlay(), set(forbidden))
        # a permission can be asked for with either element, and removing one leaves the other
        asked = {(e.tag, e.get(self.ANDROID + "name")) for e in self.overlay}
        for line in forbidden:
            kind, name = line.split()
            if kind == "permission":
                self.assertIn(("uses-permission", name), asked)
                self.assertIn(("uses-permission-sdk-23", name), asked)

    def test_a_forbidden_entry_is_never_an_expected_one(self):
        # adding it to PLAY_MANIFEST is the quick way to make a red check green, and would ship it
        expected = self.block("PLAY_MANIFEST")
        self.assertEqual(expected, sorted(set(expected)))
        self.assertFalse(set(expected) & set(self.block("PLAY_FORBIDDEN")))
        self.assertFalse(set(expected) & self.SIDE_LOAD_ONLY)

    def test_expected_entries_are_the_controller_only_ones(self):
        # every Android build is controller-only: an entry of the controlled side here means the
        # app got one back, which Play wants declared (accessibility, foreground services)
        controlled = ("accessibility-service ", "foreground-service-type ", "permission android.permission."
                      "FOREGROUND_SERVICE", "permission android.permission.SYSTEM_ALERT_WINDOW",
                      "permission android.permission.REQUEST_IGNORE_BATTERY_OPTIMIZATIONS",
                      "permission android.permission.RECEIVE_BOOT_COMPLETED")
        for line in self.block("PLAY_MANIFEST"):
            self.assertRegex(line, r"^permission [A-Za-z0-9_.]+$")
            self.assertFalse(line.startswith(controlled), line)

    def test_marker_of_the_overlay_is_the_one_the_step_checks(self):
        (application,) = self.overlay.findall("application")
        (marker,) = application.findall("meta-data")
        name, value = marker.get(self.ANDROID + "name"), marker.get(self.ANDROID + "value")
        self.assertIn(f"overlay=$(x \"/manifest/application/meta-data[@android:name='{name}']/@android:value\")",
                      self.workflow)
        self.assertIn(f'[ "$overlay" = {value} ] || fail ', self.workflow)
        # a string that no resource compiler turns into a number or a boolean
        self.assertRegex(value, r"^[a-z]+$")
        self.assertNotIn(value, ("true", "false"))

    def test_overlay_has_no_package_and_full_class_names(self):
        self.assertEqual(self.overlay.tag, "manifest")
        self.assertIsNone(self.overlay.get("package"))  # nothing would complete a short class name
        components = [e for e in self.overlay.iter() if e.tag in ("provider", "service", "receiver", "activity")]
        self.assertTrue(components)
        for element in components:
            self.assertRegex(element.get(self.ANDROID + "name"), r"^[a-z][a-z0-9_]*(\.[A-Za-z0-9_]+)+$")

    def test_entries_that_client_patches_add_for_the_update_are_the_removed_ones(self):
        # The in-app update's entries come from a client patch: what that patch names must be what
        # the overlay removes, or the bundle keeps them. Skipped until a patch adds them.
        added = []
        for patch in sorted((HERE.parent / "client" / "patches" / "rustdesk").glob("*.patch")):
            in_manifest = False
            for line in patch.read_text(encoding="utf-8", errors="replace").splitlines():
                if line.startswith("diff --git "):
                    in_manifest = line.endswith(" b/flutter/android/app/src/main/AndroidManifest.xml")
                elif in_manifest and line.startswith("+") and not line.startswith("+++"):
                    added.append(line[1:])
        found = set(update_entries("\n".join(added)))
        if not found:
            self.skipTest("no client patch adds the in-app update's manifest entries yet")
        self.assertLessEqual(found, set(self.block("PLAY_FORBIDDEN")))

    def test_update_entries_are_recognised_in_a_manifest(self):
        # update_entries() as the test above uses it, on the forms a patch may write
        manifest = """
            <uses-permission android:name="android.permission.REQUEST_INSTALL_PACKAGES" />
            <provider
                android:name=".HjvUpdateFileProvider"
                android:authorities="${applicationId}.fileprovider"
                android:exported="false">
                <meta-data
                    android:name="android.support.FILE_PROVIDER_PATHS"
                    android:resource="@xml/hjv_update_paths" />
            </provider>
            <provider android:name="io.flutter.plugins.imagepicker.ImagePickerFileProvider"
                android:authorities="${applicationId}.flutter.image_provider" />
            <provider android:authorities="x" android:resource="@xml/hjv_update_paths"
                android:name="com.example.Other"/>
            <provider android:name="com.example.Third">
                <meta-data android:name="first" android:value="1" />
                <meta-data android:name="android.support.FILE_PROVIDER_PATHS"
                    android:resource="@xml/hjv_update_paths" />
            </provider>
            <provider android:name="com.example.Unrelated">
                <meta-data android:name="android.support.FILE_PROVIDER_PATHS" android:resource="@xml/paths" />
            </provider>"""
        self.assertEqual(update_entries(manifest), [
            "provider com.carriez.flutter_hbb.HjvUpdateFileProvider", "provider com.example.Other",
            "provider com.example.Third", "permission android.permission.REQUEST_INSTALL_PACKAGES"])
        self.assertEqual(update_entries('<uses-permission android:name="android.permission.CAMERA" />'), [])

    def test_overlay_is_copied_in_between_the_two_builds(self):
        # before the APK build it would take the update out of the side-loaded APK as well
        apk = self.workflow.index("flutter build apk ")
        copy = self.workflow.index('cp "$overlay" "$release"')
        bundle = self.workflow.index("flutter build appbundle ")
        self.assertLess(apk, copy)
        self.assertLess(copy, bundle)
        self.assertIn('overlay="$GITHUB_WORKSPACE/client/play/AndroidManifest.xml"', self.workflow)
        self.assertIn("release=flutter/android/app/src/release/AndroidManifest.xml", self.workflow)
        # client/overlay is copied over the tree when it is fetched, before every build
        self.assertFalse((HERE.parent / "client" / "overlay" / "rustdesk" / "flutter" / "android" / "app" / "src"
                          / "release").exists())

    def test_bundle_steps_keep_their_conditions_and_order(self):
        # The shell of these steps can be dry-run on the development machine, their conditions work
        # only on GitHub, and actionlint (when installed) checks no more than the step ids they read
        steps = self.job_steps("android")
        names = [step.get("name") for step in steps]
        for name, conditions in self.BUNDLE_STEPS.items():
            self.assertEqual(names.count(name), 1, name)
            step = steps[names.index(name)]
            # a change that is meant goes into BUNDLE_STEPS and docs/ci.md as well
            self.assertEqual({key: step[key] for key in ("if", "id", "continue-on-error") if key in step},
                             conditions, name)
        at = {name: names.index(name) for name in self.BUNDLE_STEPS}
        # the APK is uploaded before the first bundle step: a failed bundle never costs it, also where
        # the failure fails the job
        self.assertLess(max(names.index("Upload signed APK"), names.index("Upload debug-signed APK")),
                        at["Build Play bundle"], "an APK upload comes after the first bundle step")
        # a condition reads only steps that ran before it: a step yet to run has no outcome
        ids = {step["id"]: i for i, step in enumerate(steps) if "id" in step}
        for i, step in enumerate(steps):
            for read in re.findall(r"\bsteps\.([A-Za-z0-9_-]+)\.", step.get("if", "")):
                self.assertIn(read, ids, step.get("name"))
                self.assertLess(ids[read], i, f"{step.get('name')} reads steps.{read}")
        # the keystore is removed after the step that decodes it
        self.assertLess(at["Sign and check Play bundle"], at["Remove the upload keystore"],
                        "the upload keystore is removed before the step that decodes it")


if __name__ == "__main__":
    unittest.main()

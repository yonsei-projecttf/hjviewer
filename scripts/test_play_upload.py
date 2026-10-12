# SPDX-License-Identifier: AGPL-3.0-only
# Offline tests for scripts/play-upload.py: no network and no Google credentials. The fake Play API
# below answers the calls; the key tests sign with throwaway RSA keys that openssl makes in a temporary
# directory or in memory (they belong to no Google account and are gone at the end).
# Run: python3 -I -B -m unittest discover -s scripts -p 'test_play_upload.py'  (scripts/ci.sh client)
import contextlib
import hashlib
import http.client
import importlib.util
import io
import json
import pathlib
import subprocess
import tempfile
import unittest
import urllib.parse
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

    def __init__(self, tracks=None, bundles=None, fail=None, upload_sha=None, version_code=262851234):
        self.calls = []
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
        argv = ["--package", PKG, "--aab", str(self.aab), "--name", "1.6.0 (262851234)",
                "--notes-file", str(self.notes), "--mode", mode, *extra]
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


if __name__ == "__main__":
    unittest.main()

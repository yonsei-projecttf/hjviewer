# SPDX-License-Identifier: AGPL-3.0-only
"""Offline tests of scripts/store-record.py, the release record of the build repository (auto-deploy
design 3-8), against a fake GitHub contents API and throwaway files:

    python3 -I -B -m unittest discover -s scripts -p 'test_store_record.py'
"""
import base64
import contextlib
import importlib.util
import io
import json
import os
import tempfile
import unittest
from unittest import mock

HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location("store_record", os.path.join(HERE, "store-record.py"))
sr = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(sr)

REPO = "example-org/build-repo"
TOKEN = "ghs_made-up-token-for-the-tests"
ENV = {"GITHUB_TOKEN": TOKEN, "GITHUB_API_URL": "https://api.example.test"}
A = "a" * 40
B = "b" * 40
RECORD = (sr.HEADER
          + f"play 1.9.0 262932441 upload alpha {A} 2026-10-21 by hand in Play Console\n"
          + f"play 1.9.1 263122249 upload alpha {B} 2026-10-28 store #12 build-run 4242\n"
          + f"appstore 1.9.1 263120249 submit - {B} 2026-10-28 store #13\n")


class FakeGitHub:
    """The contents API of one repository; records the calls."""

    def __init__(self, record=None, repo_status=200, conflicts=0, put_status=None):
        self.record = record
        self.sha = "sha-1"
        self.repo_status = repo_status
        self.conflicts = conflicts
        self.put_status = put_status
        self.calls = []

    def __call__(self, method, url, headers, data):
        self.calls.append((method, url, headers, json.loads(data) if data else None))
        assert headers["Authorization"] == "Bearer " + TOKEN
        path = url.removeprefix(ENV["GITHUB_API_URL"] + "/")
        if path == f"repos/{REPO}":
            return self.repo_status, b"{}"
        assert path == f"repos/{REPO}/contents/{sr.RECORD_PATH}", path
        if method == "GET":
            if self.record is None:
                return 404, b'{"message": "Not Found"}'
            return 200, json.dumps({"sha": self.sha, "encoding": "base64",
                                    "content": base64.b64encode(self.record.encode()).decode()}).encode()
        assert method == "PUT"
        body = json.loads(data)
        if self.put_status:
            return self.put_status, b'{"message": "no"}'
        if self.conflicts:
            self.conflicts -= 1
            # another run's line lands meanwhile
            self.record = (self.record or sr.HEADER) + f"appstore 1.9.2 263200000 submit - {A} 2026-11-01 store #20\n"
            self.sha = f"sha-{len(self.calls)}"
            return 409, b'{"message": "is at sha-x but expected sha-1"}'
        if body.get("sha") != (self.sha if self.record is not None else None):
            return 409, b"{}"
        self.record = base64.b64decode(body["content"]).decode()
        self.sha = f"sha-{len(self.calls)}"
        return (200 if "sha" in body else 201), b"{}"


class StoreRecordTest(unittest.TestCase):
    def setUp(self):
        sleep = mock.patch.object(sr.time, "sleep", lambda s: None)
        sleep.start()
        self.addCleanup(sleep.stop)

    def run_script(self, argv, fake=None):
        out = io.StringIO()
        with mock.patch.object(sr, "send", fake or (lambda *a: (_ for _ in ()).throw(AssertionError("network")))), \
                contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
            code = sr.main(argv, ENV)
        self.assertNotIn(TOKEN, out.getvalue())
        return code, out.getvalue()

    # the rules (store, version)

    def test_a_new_version_passes(self):
        self.assertEqual(sr.check(RECORD, "play", "1.9.2", "263200000", "upload"), "new")
        self.assertEqual(sr.check(RECORD, "appstore", "1.9.1", "263120249", "release"), "new")

    def test_the_same_line_again_passes(self):
        self.assertEqual(sr.check(RECORD, "play", "1.9.1", "263122249", "upload"), "recorded")
        self.assertEqual(sr.check(RECORD, "appstore", "1.9.1", "263120249", "submit"), "recorded")

    def test_the_same_version_with_another_build_is_refused(self):
        with self.assertRaisesRegex(sr.RecordError, "recorded with build 263122249"):
            sr.check(RECORD, "play", "1.9.1", "263122999", "upload")
        with self.assertRaisesRegex(sr.RecordError, "recorded with build 263120249"):
            sr.check(RECORD, "appstore", "1.9.1", "263120999", "release")

    def test_a_lower_version_is_refused(self):
        with self.assertRaisesRegex(sr.RecordError, "has 1.9.1 already .* 1.8.9 is lower"):
            sr.check(RECORD, "play", "1.8.9", "262000000", "upload")
        # numbers, not text: 1.10.0 is above 1.9.1
        self.assertEqual(sr.check(RECORD, "play", "1.10.0", "263300000", "upload"), "new")

    def test_the_other_store_does_not_count(self):
        # the same 1.9.0 had different numbers in the two stores (version-policy-5.json)
        play_only = "".join(line for line in RECORD.splitlines(True) if not line.startswith("appstore"))
        self.assertEqual(sr.check(play_only, "appstore", "1.9.0", "262930544", "submit"), "new")
        record = RECORD + f"appstore 1.9.3 263300100 submit - {A} 2026-11-20 store #30\n"
        self.assertEqual(sr.check(record, "play", "1.9.2", "263200000", "upload"), "new")

    def test_a_released_app_store_version_takes_no_new_submission(self):
        record = RECORD + f"appstore 1.9.1 263120249 release - {B} 2026-11-02 store #15\n"
        with self.assertRaisesRegex(sr.RecordError, "released .* no new submission"):
            sr.check(record, "appstore", "1.9.1", "263120249", "submit")
        self.assertEqual(sr.check(record, "appstore", "1.9.1", "263120249", "release"), "recorded")

    def test_lines_in_no_known_form_stop_everything(self):
        for bad in ("appstore 1.9.1 263120249\n", "appstore 1.9 263120249 release -\n",
                    "appstore 1.9.1 263120249 Release - x\n", "app-store 1.9.1 263120249 release - x\n",
                    "play 1.9.0 <alpha의 versionCode> upload alpha x\n"):
            with self.assertRaisesRegex(sr.RecordError, "line 2 is not"):
                sr.check(sr.HEADER + bad, "play", "1.9.2", "1", "upload")

    def test_the_build_run_refuses_a_version_the_store_has(self):
        sr.check_build(RECORD, "play", "1.9.2")
        sr.check_build(RECORD, "appstore", "1.9.2")
        with self.assertRaisesRegex(sr.RecordError, "has play 1.9.1 build 263122249"):
            sr.check_build(RECORD, "play", "1.9.1")
        with self.assertRaisesRegex(sr.RecordError, "is past it with play 1.9.1"):
            sr.check_build(RECORD, "play", "1.9.0")
        with self.assertRaisesRegex(sr.RecordError, "has appstore 1.9.1"):
            sr.check_build(RECORD, "appstore", "1.9.1")
        # another store's line is no reason: Windows and Android 1.6.1 went out beside iOS 1.6.2
        sr.check_build(sr.HEADER + f"appstore 1.9.3 1 submit - {A} 2026-11-20 x\n", "play", "1.9.2")
        sr.check_build("", "play", "1.9.1")

    # the command line and the contents API

    def test_check_against_the_repository(self):
        fake = FakeGitHub(RECORD)
        code, out = self.run_script(["check", "--repo", REPO, "--store", "play", "--version", "1.9.2",
                                     "--build", "263200000", "--action", "upload"], fake)
        self.assertEqual(code, 0, out)
        self.assertIn("allowed", out)
        self.assertEqual([c[0] for c in fake.calls], ["GET"])
        code, out = self.run_script(["check", "--repo", REPO, "--store", "play", "--version", "1.9.1",
                                     "--build", "1", "--action", "upload"], FakeGitHub(RECORD))
        self.assertEqual(code, 1)
        self.assertIn("::error::release record:", out)

    def test_a_missing_record_counts_as_empty_only_when_the_repository_answers(self):
        code, out = self.run_script(["check-build", "--repo", REPO, "--store", "appstore", "--version", "1.9.1"],
                                    FakeGitHub(None))
        self.assertEqual(code, 0, out)
        code, out = self.run_script(["check-build", "--repo", REPO, "--store", "appstore", "--version", "1.9.1"],
                                    FakeGitHub(None, repo_status=404))
        self.assertEqual(code, 1)
        self.assertIn("repository does not answer", out)

    def test_add_appends_a_line_with_skip_ci(self):
        fake = FakeGitHub(RECORD)
        argv = ["add", "--repo", REPO, "--store", "appstore", "--version", "1.9.1", "--build", "263120249",
                "--action", "release", "--source", B, "--note", "store #15"]
        code, out = self.run_script(argv, fake)
        self.assertEqual(code, 0, out)
        put = [c for c in fake.calls if c[0] == "PUT"]
        self.assertEqual(len(put), 1)
        self.assertIn("[skip ci]", put[0][3]["message"])
        self.assertEqual(put[0][3]["sha"], "sha-1")
        last = fake.record.splitlines()[-1].split()
        self.assertEqual(last[:6], ["appstore", "1.9.1", "263120249", "release", "-", B])
        self.assertRegex(last[6], r"^\d{4}-\d\d-\d\d$")
        self.assertEqual(last[7:], ["store", "#15"])
        self.assertTrue(fake.record.startswith(RECORD))
        # what it wrote, hjv-release's reader takes too: every line in the record's form
        sr.parse(fake.record)
        # a re-run writes nothing
        code, out = self.run_script(argv, fake)
        self.assertEqual(code, 0, out)
        self.assertIn("recorded already", out)
        self.assertEqual(len([c for c in fake.calls if c[0] == "PUT"]), 1)

    def test_add_creates_the_record_with_its_header(self):
        fake = FakeGitHub(None)
        code, out = self.run_script(["add", "--repo", REPO, "--store", "play", "--version", "1.9.1", "--build",
                                     "263122249", "--action", "upload", "--track", "alpha", "--source", B], fake)
        self.assertEqual(code, 0, out)
        put = [c for c in fake.calls if c[0] == "PUT"][0][3]
        self.assertNotIn("sha", put)
        self.assertTrue(fake.record.startswith(sr.HEADER))
        self.assertEqual(fake.record.count("\n"), 2)

    def test_add_reads_again_after_a_conflict(self):
        fake = FakeGitHub(RECORD, conflicts=2)
        code, out = self.run_script(["add", "--repo", REPO, "--store", "play", "--version", "1.9.2", "--build",
                                     "263200000", "--action", "upload", "--track", "alpha", "--source", B], fake)
        self.assertEqual(code, 0, out)
        self.assertEqual(out.count("changed meanwhile"), 2)
        self.assertEqual([c[0] for c in fake.calls], ["GET", "PUT"] * 3)
        # the other run's line stays, ours comes after it
        self.assertIn("appstore 1.9.2 263200000 submit", fake.record)
        self.assertTrue(fake.record.splitlines()[-1].startswith("play 1.9.2 263200000 upload alpha"))

    def test_add_gives_up_after_five_conflicts(self):
        fake = FakeGitHub(RECORD, conflicts=9)
        code, out = self.run_script(["add", "--repo", REPO, "--store", "play", "--version", "1.9.2", "--build",
                                     "263200000", "--action", "upload", "--track", "alpha", "--source", B], fake)
        self.assertEqual(code, 1)
        self.assertIn("gave up after 5 attempts", out)
        self.assertEqual(len([c for c in fake.calls if c[0] == "PUT"]), 5)

    def test_add_checks_the_rules_on_the_fresh_record(self):
        # a line written meanwhile can make ours wrong: the check runs on each read
        fake = FakeGitHub(RECORD + f"play 1.9.2 263200001 upload alpha {A} 2026-11-01 store #21\n")
        code, out = self.run_script(["add", "--repo", REPO, "--store", "play", "--version", "1.9.2", "--build",
                                     "263200000", "--action", "upload", "--track", "alpha", "--source", B], fake)
        self.assertEqual(code, 1)
        self.assertIn("recorded with build 263200001", out)
        self.assertNotIn("PUT", [c[0] for c in fake.calls])

    def test_a_refused_write_is_an_error(self):
        code, out = self.run_script(["add", "--repo", REPO, "--store", "play", "--version", "1.9.2", "--build",
                                     "263200000", "--action", "upload", "--track", "alpha", "--source", B],
                                    FakeGitHub(RECORD, put_status=403))
        self.assertEqual(code, 1)
        self.assertIn("HTTP 403", out)

    def test_add_to_a_local_file(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "store-releases.txt")
            argv = ["add", "--file", path, "--store", "appstore", "--version", "1.9.1", "--build", "263120249",
                    "--action", "submit", "--source", B]
            self.assertEqual(self.run_script(argv)[0], 0)
            self.assertEqual(self.run_script(argv)[0], 0)
            with open(path, encoding="utf-8") as handle:
                text = handle.read()
            self.assertTrue(text.startswith(sr.HEADER))
            self.assertEqual(len(sr.parse(text)), 1)

    def test_bad_usage(self):
        for argv, why in [
            (["check", "--file", "x", "--store", "play", "--version", "1.9", "--build", "1", "--action", "upload"],
             "not a version"),
            (["check", "--file", "x", "--store", "play", "--version", "1.9.1", "--build", "1", "--action", "submit"],
             "play records upload"),
            (["check", "--file", "x", "--store", "appstore", "--version", "1.9.1", "--build", "12a",
              "--action", "submit"], "not a build number"),
            (["add", "--file", "x", "--store", "play", "--version", "1.9.1", "--build", "1", "--action", "upload",
              "--source", B], "names its track"),
            (["add", "--file", "x", "--store", "appstore", "--version", "1.9.1", "--build", "1",
              "--action", "submit", "--source", "abc"], "40-digit"),
            (["add", "--file", "x", "--store", "appstore", "--version", "1.9.1", "--build", "1",
              "--action", "submit", "--source", B, "--note", "two\nlines"], "one line"),
            (["check", "--repo", "a/b/../c", "--store", "play", "--version", "1.9.1", "--build", "1",
              "--action", "upload"], "not a repository"),
        ]:
            code, out = self.run_script(argv)
            self.assertEqual(code, 2, argv)
            self.assertIn(why, out)
        out = io.StringIO()
        with mock.patch.object(sr, "send", lambda *a: (200, b"{}")), contextlib.redirect_stdout(out):
            self.assertEqual(sr.main(["check-build", "--repo", REPO, "--store", "play", "--version", "1.9.1"], {}), 2)
        self.assertIn("GITHUB_TOKEN", out.getvalue())


if __name__ == "__main__":
    unittest.main()

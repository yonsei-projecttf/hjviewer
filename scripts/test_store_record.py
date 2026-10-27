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
import http.server
import tempfile
import threading
import types
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

    def test_a_local_file_is_read_as_the_contents_api_gives_it(self):
        # a lone \r stays one (open()'s newline translation made it a line break)
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "store-releases.txt")
            with open(path, "w", encoding="utf-8", newline="") as handle:
                handle.write(sr.HEADER + f"appstore 1.9.1 263120249 release - {B} 2026-11-02 x\r")
            code, out = self.run_script(["check-build", "--file", path, "--store", "appstore", "--version", "1.9.2"])
            self.assertEqual(code, 1, out)
            self.assertIn("line 2 has a control character", out)

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
            (["add", "--file", "x", "--store", "appstore", "--version", "1.9.1", "--build", "1",
              "--action", "submit", "--source", B, "--note", "store \x1b[31m#13"], "without control characters"),
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


PUBLIC = "yonsei-projecttf/hjviewer"
HEAD = "c" * 40
RUN = "4242"


def build_yml(sha, extra=""):
    """The build repository's build.yml as docs/ci.md shows it, pinned to `sha` in both places."""
    return ("# Builds the HJViewer apps.\n"
            "name: build\n"
            "on:\n"
            "  workflow_dispatch:\n"
            "    inputs:\n"
            "      platforms: { type: choice, options: [windows, android, ios, relay, all], default: all }\n"
            "      testflight: { type: boolean, default: false }\n"
            "      testflight_run: { type: string, required: false }\n"
            "      play: { type: boolean, default: false }\n"
            "      play_release: { type: choice, options: [completed, draft, validate], default: completed }\n"
            "      check_only: { type: boolean, default: false }\n"
            "permissions:\n"
            "  contents: read\n"
            "jobs:\n"
            "  client:\n"
            "    # the reviewed public commit\n"
            "    permissions:\n"
            "      contents: write\n"
            "      actions: read\n"
            f"    uses: {PUBLIC}/.github/workflows/client-build.yml@{sha}\n"
            "    with:\n"
            "      platforms: ${{ inputs.platforms }}\n"
            "      testflight: ${{ inputs.testflight }}\n"
            "      testflight_run: ${{ inputs.testflight_run }}\n"
            "      play: ${{ inputs.play }}\n"
            "      play_release: ${{ inputs.play_release }}\n"
            "      check_only: ${{ inputs.check_only }}\n"
            f"      source_ref: {sha}\n"
            "    secrets: inherit\n" + extra)


class FakeRuns:
    """The actions runs, contents and artifacts API of the build repository for check-run."""

    def __init__(self, **change):
        self.run = {"id": int(RUN), "path": ".github/workflows/build.yml", "event": "workflow_dispatch",
                    "status": "completed", "conclusion": "success", "head_branch": "main", "head_sha": HEAD,
                    "repository": {"full_name": REPO},
                    "referenced_workflows": [{"path": f"{PUBLIC}/.github/workflows/client-build.yml@{B}",
                                              "sha": B, "ref": B}]}
        self.run.update(change)
        self.build_yml = build_yml(B)
        self.artifacts = [{"name": "HJViewer-1.9.1-bbbbbbb-android-aarch64", "expired": False},
                          {"name": "HJViewer-1.9.1-bbbbbbb-android-aarch64-play", "expired": False},
                          {"name": "HJViewer-1.9.1-bbbbbbb-ios", "expired": False}]
        self.calls = []

    def __call__(self, method, url, headers, data):
        assert method == "GET" and headers["Authorization"] == "Bearer " + TOKEN
        path = url.removeprefix(ENV["GITHUB_API_URL"] + "/")
        self.calls.append(path)
        if path == f"repos/{REPO}/actions/runs/{RUN}":
            return 200, json.dumps(self.run).encode()
        if path == f"repos/{REPO}/contents/.github/workflows/build.yml?ref={HEAD}":
            return 200, json.dumps({"encoding": "base64", "content": base64.b64encode(self.build_yml.encode()).decode()}).encode()
        if path == f"repos/{REPO}/actions/runs/{RUN}/artifacts?per_page=100":
            return 200, json.dumps({"artifacts": self.artifacts}).encode()
        return 404, b"{}"


class CheckRunTest(unittest.TestCase):
    ARGV = ["check-run", "--repo", REPO, "--store", "play", "--version", "1.9.1", "--run-id", RUN, "--source", B]

    def run_script(self, fake, env=ENV, argv=None):
        out = io.StringIO()
        with mock.patch.object(sr, "send", fake), contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
            code = sr.main(argv or self.ARGV, env)
        self.assertNotIn(TOKEN, out.getvalue())
        return code, out.getvalue()

    def test_the_reviewed_build_run_passes_and_names_the_artifact(self):
        with tempfile.TemporaryDirectory() as tmp:
            output = os.path.join(tmp, "output")
            code, out = self.run_script(FakeRuns(), {**ENV, "GITHUB_OUTPUT": output})
            self.assertEqual(code, 0, out)
            with open(output, encoding="utf-8") as handle:
                self.assertEqual(handle.read(), "artifact=HJViewer-1.9.1-bbbbbbb-android-aarch64-play\n")

    def test_testflight_takes_the_ipa_of_the_reviewed_build_run(self):
        """client-build.yml's TestFlight upload job: the same provenance, for the run's IPA."""
        argv = [a if a != "play" else "appstore" for a in self.ARGV]
        with tempfile.TemporaryDirectory() as tmp:
            output = os.path.join(tmp, "output")
            code, out = self.run_script(FakeRuns(), {**ENV, "GITHUB_OUTPUT": output}, argv)
            self.assertEqual(code, 0, out)
            with open(output, encoding="utf-8") as handle:
                self.assertEqual(handle.read(), "artifact=HJViewer-1.9.1-bbbbbbb-ios\n")
        self.assertIn("IPA: HJViewer-1.9.1-bbbbbbb-ios of build run 4242", out)
        # an unsigned IPA (-ios-unsigned), a check build's or an expired one is not that artifact
        for names, why in [(["HJViewer-1.9.1-bbbbbbb-ios-unsigned"], "has no HJViewer-1.9.1-bbbbbbb-ios:"),
                           (["HJViewer-1.9.1-bbbbbbb-check-ios"], "has no HJViewer-1.9.1-bbbbbbb-ios:")]:
            fake = FakeRuns()
            fake.artifacts = [{"name": n, "expired": False} for n in names]
            code, out = self.run_script(fake, argv=argv)
            self.assertEqual(code, 1, names)
            self.assertIn(why, out)
        fake = FakeRuns()
        fake.artifacts[2]["expired"] = True
        code, out = self.run_script(fake, argv=argv)
        self.assertEqual((code, "IPAs are kept 7 days" in out), (1, True), out)
        # and the run is held to the same rules as for a Play upload
        code, out = self.run_script(FakeRuns(conclusion="failure"), argv=argv)
        self.assertEqual(code, 1)
        self.assertIn("conclusion 'failure'", out)

    def test_another_run_is_refused(self):
        other = {"path": f"{PUBLIC}/.github/workflows/client-build.yml@{A}", "sha": A, "ref": A}
        pinned = {"path": f"{PUBLIC}/.github/workflows/client-build.yml@{B}", "sha": B, "ref": B}
        for change, why in [
            ({"head_branch": "other"}, "head_branch 'other'"),
            ({"event": "push"}, "event 'push'"),
            ({"conclusion": "failure"}, "conclusion 'failure'"),
            ({"path": ".github/workflows/store.yml"}, "path"),
            ({"repository": {"full_name": "someone/fork"}}, "repository"),
            ({"referenced_workflows": [other]}, "did not call exactly"),
            ({"referenced_workflows": [pinned, other]}, "did not call exactly"),
            ({"referenced_workflows": [{**pinned, "sha": A}]}, "did not call exactly"),
            # the same sha in another file or another repository: only the path tells them apart
            ({"referenced_workflows": [{**pinned, "path": f"{PUBLIC}/.github/workflows/other.yml@{B}"}]},
             "did not call exactly"),
            ({"referenced_workflows": [{**pinned, "path": f"someone/fork/.github/workflows/client-build.yml@{B}"}]},
             "did not call exactly"),
            ({"referenced_workflows": []}, "did not call exactly"),
            ({"head_sha": None}, "head_sha"),
        ]:
            code, out = self.run_script(FakeRuns(**change))
            self.assertEqual(code, 1, change)
            self.assertIn(why, out, change)

    def test_build_yml_must_have_the_documented_shape(self):
        for text, why in [
            (build_yml(B, "  other:\n    runs-on: ubuntu-24.04\n"), "second job"),
            (build_yml(B).replace("    secrets: inherit\n", "    steps:\n      run: echo\n"), "job key"),
            (build_yml(B).replace(f"@{B}", f"@{A}"), "uses is not"),
            (build_yml(B).replace(f"source_ref: {B}", f"source_ref: {A}"), "source_ref is not"),
            (build_yml(B).replace("      play: ${{ inputs.play }}\n", "      script: ${{ inputs.play }}\n"), "no input script"),
            (build_yml(B) + "  extra:\n    - run: echo\n", "lists"),
            (build_yml(B).replace("name: build", "name: 'build'"), "character"),
            (build_yml(B).replace("jobs:\n", "env:\n  A: b\njobs:\n"), "top-level"),
        ]:
            fake = FakeRuns()
            fake.build_yml = text
            code, out = self.run_script(fake)
            self.assertEqual(code, 1, why)
            self.assertIn(why, out)

    def test_the_artifact_must_be_there_and_not_expired(self):
        fake = FakeRuns()
        fake.artifacts[1]["expired"] = True
        code, out = self.run_script(fake)
        self.assertEqual(code, 1)
        self.assertIn("it has expired", out)
        fake.artifacts = fake.artifacts[:1]
        code, out = self.run_script(fake)
        self.assertEqual(code, 1)
        self.assertIn("has no HJViewer-1.9.1-bbbbbbb-android-aarch64-play", out)

    def test_bad_usage(self):
        for argv, why in [
            (["check-run", "--repo", REPO, "--store", "play", "--version", "1.9.1", "--run-id", "12a", "--source", B], "--run-id"),
            (["check-run", "--repo", REPO, "--store", "play", "--version", "1.9.1", "--run-id", RUN, "--source", "abc"], "40-digit"),
            (["check-run", "--file", "x", "--store", "play", "--version", "1.9.1", "--run-id", RUN, "--source", B], "--repo"),
        ]:
            out = io.StringIO()
            with mock.patch.object(sr, "send", lambda *a: (_ for _ in ()).throw(AssertionError("network"))), \
                    contextlib.redirect_stdout(out):
                self.assertEqual(sr.main(argv, ENV), 2, argv)
            self.assertIn(why, out.getvalue())


class VersionLimitTest(unittest.TestCase):
    def test_the_tools_version_limits(self):
        for good in ("1.9.1", "0.0.0", "999.999.99"):
            sr.version_key(good)
        for bad in ("1.9.100", "1000.0.0", "01.9.1", "1.09.1", "1.9"):
            with self.assertRaises(sr.RecordError, msg=bad):
                sr.version_key(bad)


# One table in the tests of both readers of the record (tools/hjv-release src/store.rs,
# the_two_readers_take_one_line_grammar): a record, then what reading it gives, the highest App Store
# release (None: none), or the number of the line that is refused.
C = "1" * 40
WRITTEN = (sr.HEADER
           + f"play 1.9.1 263122249 upload alpha {C} 2026-10-28 store #12 build-run 4242\n"
           + f"appstore 1.9.1 263120249 submit - {C} 2026-10-28 store #13\n"
           + f"appstore 1.9.1 263120249 release - {C} 2026-11-02 store #15\n")
BY_HAND = f"play 1.9.0 262932441 upload alpha {'3' * 40} 2026-10-21 by hand\n"
LINE_GRAMMAR = [
    # read by both, as before: the record job's lines, one written by hand, CRLF lines, tabs and
    # runs of spaces, comments and blank lines, no newline at the end, builds of 1 and 18 digits,
    # a Unicode space after the fourth field, no lines
    (WRITTEN, "1.9.1"),
    (BY_HAND, None),
    ("# note\r\n\r\nappstore 1.9.1 263120249 release - x 2026-11-02 store #15\r\n", "1.9.1"),
    ("\tappstore  1.9.1\t263120249 \t release\n", "1.9.1"),
    ("  # a note\n\t# another\n \t \n\n", None),
    ("appstore 1.9.1 263120249 release - x", "1.9.1"),
    ("appstore 1.9.1 7 release - x\nappstore 1.9.2 123456789012345678 release - x\n", "1.9.2"),
    ("appstore 1.9.1 263120249 release - x by\xa0hand\n", "1.9.1"),
    ("", None),
    # refused by both: a build of 19 digits (hjv-release read it) or with a digit that is not ASCII
    ("appstore 1.9.1 1234567890123456789 release - x\n", 1),
    ("appstore 1.9.1 26312024\u0663 release - x\n", 1),
    # a \r other than the one of a CRLF line (store-record.py read the first as two lines and
    # hjv-release as one; both read the other two)
    ("appstore 1.9.1 1 release - x\rappstore 1.9.3 1 release - x\n", 1),
    ("# note\nappstore 1.9.1 1 release - x\r", 2),
    ("appstore 1.9.1 1 release - x\r\r\n", 1),
    # the other line breaks of str.splitlines(), and \x1f, a space to str.split() only: the two
    # read some of these differently (two lines as one, a release after a comment)
    ("appstore 1.9.1 1 release - x\x0bappstore 1.9.3 1 release - x\n", 1),
    ("appstore 1.9.1 1 release - x\x0c\n", 1),
    ("# note\x1cappstore 1.9.3 1 release - x\n", 1),
    ("appstore\x1f1.9.1 1 release - x\n", 1),
    ("# note\x85appstore 1.9.3 1 release - x\n", 1),
    ("# note\u2028appstore 1.9.3 1 release - x\n", 1),
    ("appstore 1.9.1 1 release - x\u2029\n", 1),
    # any other control character, in a comment too
    ("appstore 1.9.1 1 release - x\x00\n", 1),
    ("# \x1b[31mnote\x1b[0m\n", 1),
    ("appstore 1.9.1 1 release - x\x7f\n", 1),
    ("appstore 1.9.1 1 release - x\x9b\n", 1),
    # fields are separated by spaces and tabs only (both split at Unicode spaces before)
    ("appstore\u30001.9.1 1 release - x\n", 1),
    ("appstore 1.9.1\xa01 release - x\n", 1),
    ("\xa0\n", 1),
    # lines are counted at \n
    ("# header\n\nappstore 1.9.1 1 release - x\nappstore 1.9 1 release - x\n", 4),
]


class LineGrammarTest(unittest.TestCase):
    def test_the_two_readers_take_one_line_grammar(self):
        for record, expected in LINE_GRAMMAR:
            with self.subTest(record=record):
                if isinstance(expected, int):
                    with self.assertRaisesRegex(sr.RecordError, rf"^store-releases\.txt line {expected} "):
                        sr.parse(record)
                else:
                    keys = [e["key"] for e in sr.parse(record) if e["store"] == "appstore" and e["action"] == "release"]
                    self.assertEqual(".".join(map(str, max(keys))) if keys else None, expected)

    def test_the_table_has_the_lines_the_record_job_writes(self):
        lines = [sr.line_of(types.SimpleNamespace(store=store, version="1.9.1", build=build, action=action,
                                                  track=track, source=C, note=note), day)
                 for store, build, action, track, note, day in [
                     ("play", "263122249", "upload", "alpha", "store #12 build-run 4242", "2026-10-28"),
                     ("appstore", "263120249", "submit", None, "store #13", "2026-10-28"),
                     ("appstore", "263120249", "release", None, "store #15", "2026-11-02")]]
        self.assertEqual(sr.HEADER + "".join(lines), WRITTEN)


class NoRedirectTest(unittest.TestCase):
    """A 3xx answer is returned, not followed: the Authorization header goes nowhere else."""

    def test_send_does_not_follow_a_redirect(self):
        spec = importlib.util.spec_from_file_location("play_upload_for_redirects", os.path.join(HERE, "play-upload.py"))
        play = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(play)
        reached = []

        class Target(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                reached.append(self.headers.get("Authorization"))
                self.send_response(200)
                self.end_headers()

            def log_message(self, *args):
                pass

        target = http.server.HTTPServer(("127.0.0.1", 0), Target)

        class Redirect(Target):
            def do_GET(self):
                self.send_response(302)
                self.send_header("Location", f"http://127.0.0.1:{target.server_port}/elsewhere")
                self.send_header("Content-Length", "0")
                self.end_headers()

        source = http.server.HTTPServer(("127.0.0.1", 0), Redirect)
        servers = [target, source]
        for server in servers:
            threading.Thread(target=server.serve_forever, daemon=True).start()
        try:
            url = f"http://127.0.0.1:{source.server_port}/"
            headers = {"Authorization": "Bearer " + TOKEN}
            self.assertEqual(sr.send("GET", url, headers, None)[0], 302)
            self.assertEqual(play.send("GET", url, headers, None, 5)[0], 302)
            self.assertEqual(reached, [])
        finally:
            for server in servers:
                server.shutdown()
                server.server_close()


if __name__ == "__main__":
    unittest.main()

# SPDX-License-Identifier: AGPL-3.0-only
"""Offline tests of the store run's rules in the workflow files (auto-deploy design 2-1, 2-5, 7-2),
read as text like test_play_upload.py's PlayBundleListsTest: what only a run on GitHub would show.

    python3 -I -B -m unittest discover -s scripts -p 'test_store_workflows.py'

store-release.yml: called only (workflow_call), Linux only, read permissions but the record job's,
every checkout of this file's own repository and commit without credentials, the environment
`store` and the secrets in the play and appstore jobs alone (by name, never inherit), status out
of the editing jobs' concurrency groups. client-build.yml: no Google Play job, play refused, the
-play bundle kept 30 days, the release checks before any third-party build step, and the TestFlight
upload a job of its own that builds nothing and takes the IPA of a checked, finished build run. The
run summary (scripts/run-summary.py, RunSummaryTest) never fails a run.
"""
import importlib.util
import io
import json
import os
import re
import tempfile
import unittest
from contextlib import redirect_stdout

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
WORKFLOWS = os.path.join(ROOT, ".github", "workflows")

_spec = importlib.util.spec_from_file_location("run_summary", os.path.join(ROOT, "scripts", "run-summary.py"))
rs = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(rs)
SHA_PIN = re.compile(r"uses: [\w./-]+@[0-9a-f]{40}(?: #.*)?$")


def read(name):
    """The workflow without its comment lines (the comments may name what the code must not do)."""
    with open(os.path.join(WORKFLOWS, name), encoding="utf-8") as handle:
        return "".join(line for line in handle if not line.lstrip().startswith("#"))


def top_block(text, key):
    """The lines under a top-level `key:`."""
    lines = text.splitlines()
    (start,) = [i for i, line in enumerate(lines) if line == f"{key}:"]
    block = []
    for line in lines[start + 1:]:
        if line and not line.startswith((" ", "#")):
            break
        block.append(line)
    return block


def jobs(text):
    """{job id: its lines} of the `jobs:` block."""
    out, current = {}, None
    for line in top_block(text, "jobs"):
        m = re.fullmatch(r"  ([a-z][a-z0-9_-]*):", line)
        if m:
            current = out.setdefault(m.group(1), [])
        elif current is not None:
            current.append(line)
    return out


def steps(lines):
    """The steps of a job, each as its text (the `- ` line and the lines under it)."""
    out, inside = [], False
    for line in lines:
        if line.startswith("    ") and not line.startswith("     "):
            inside = line.strip() == "steps:"
            continue
        if inside and line.startswith("      - "):
            out.append(line + "\n")
        elif inside and out and (line.startswith("        ") or not line.strip()):
            out[-1] += line + "\n"
    return out


def name_of(step):
    m = re.search(r"^      - name: (.+)$", step, re.M) or re.search(r"^        name: (.+)$", step, re.M)
    return m.group(1).strip() if m else ""


def key(lines, name):
    """The value of a job's own `name:` key (indent 4), or None."""
    for line in lines:
        m = re.fullmatch(rf"    {name}: ?(.*)", line)
        if m:
            return m.group(1)
    return None


def block(lines, name):
    """The lines under a job's own `name:` key (indent 4), stripped, blank lines left out."""
    out, inside = [], False
    for line in lines:
        if re.fullmatch(r"    \S.*", line):
            inside = line == f"    {name}:"
        elif inside and line.strip():
            out.append(line.strip())
    return out


class StoreReleaseWorkflowTest(unittest.TestCase):
    def setUp(self):
        self.text = read("store-release.yml")
        self.jobs = jobs(self.text)

    def test_the_jobs(self):
        self.assertEqual(list(self.jobs), ["checks", "play", "appstore", "record"])

    def test_it_is_called_only(self):
        triggers = [line.strip() for line in top_block(self.text, "on") if re.fullmatch(r"  [a-z_]+:", line)]
        self.assertEqual(triggers, ["workflow_call:"])

    def test_linux_only(self):
        for job, lines in self.jobs.items():
            self.assertEqual(key(lines, "runs-on"), "ubuntu-24.04", job)
            self.assertIsNotNone(key(lines, "timeout-minutes"), job)

    def test_permissions_read_but_the_record_job(self):
        self.assertEqual([line.strip() for line in top_block(self.text, "permissions") if line.strip()],
                         ["contents: read"])
        for job, lines in self.jobs.items():
            text = "\n".join(lines)
            self.assertIn("    permissions:\n", text + "\n", f"{job} names its permissions")
            if job == "record":
                # the one write, and nothing beside it (actions: write could change other runs)
                self.assertEqual(block(lines, "permissions"), ["contents: write"])
            else:
                self.assertNotIn("write", text.split("    steps:")[0], job)
        self.assertNotIn("contents: write", "\n".join(l for j, ls in self.jobs.items() if j != "record" for l in ls))

    def test_every_checkout_is_this_files_own_source_without_credentials(self):
        checkouts = 0
        for job, lines in self.jobs.items():
            for step in steps(lines):
                if "uses: actions/checkout@" in step:
                    checkouts += 1
                    self.assertIn("repository: ${{ job.workflow_repository }}\n", step, job)
                    self.assertIn("ref: ${{ job.workflow_sha }}\n", step, job)
                    self.assertIn("persist-credentials: false\n", step, job)
            # the source is checked before the first checkout of every job
            first = steps(lines)[0]
            self.assertIn('"$WORKFLOW_REPOSITORY" != "$PUBLIC_REPOSITORY"', first, job)
        self.assertEqual(checkouts, 4)
        self.assertIn("  PUBLIC_REPOSITORY: yonsei-projecttf/hjviewer\n", self.text)
        self.assertNotRegex(self.text, r"github\.sha|ref: \$\{\{ github\.")

    def test_actions_are_pinned_by_sha(self):
        for line in self.text.splitlines():
            if line.strip().startswith(("uses:", "- uses:")):
                self.assertRegex(line.strip().removeprefix("- "), SHA_PIN, line)

    def test_environment_and_secrets_only_where_they_are_needed(self):
        self.assertNotIn("secrets: inherit", self.text)
        expected = {
            "play": {"PLAY_SA_EMAIL", "PLAY_SA_PRIVATE_KEY"},
            "appstore": {"ASC_APP_ID", "ASC_ISSUER_ID", "ASC_STORE_KEY_ID", "ASC_STORE_PRIVATE_KEY",
                         "REVIEW_DEMO_USER", "REVIEW_DEMO_PASSWORD", "REVIEW_DEMO_PC_ID", "REVIEW_DEMO_PC_PASSWORD",
                         "REVIEW_CONTACT_FIRST_NAME", "REVIEW_CONTACT_LAST_NAME", "REVIEW_CONTACT_PHONE",
                         "REVIEW_CONTACT_EMAIL"},
        }
        for job, lines in self.jobs.items():
            text = "\n".join(lines)
            used = set(re.findall(r"secrets\.([A-Z0-9_]+)", text))
            self.assertEqual(used, expected.get(job, set()), job)
            self.assertEqual(key(lines, "environment"), "store" if job in expected else None, job)
        declared = {m for m in re.findall(r"^      ([A-Z0-9_]+):$", "\n".join(top_block(self.text, "on")), re.M)}
        self.assertEqual(declared, set().union(*expected.values()))
        # the ASC key of the store run is the App Manager key, under the script's names
        appstore = "\n".join(self.jobs["appstore"])
        self.assertIn("ASC_KEY_ID: ${{ secrets.ASC_STORE_KEY_ID }}", appstore)
        self.assertIn("ASC_PRIVATE_KEY: ${{ secrets.ASC_STORE_PRIVATE_KEY }}", appstore)

    def test_status_stays_out_of_the_editing_groups(self):
        groups = {"play": "google-play", "appstore": "app-store"}
        for job, prefix in groups.items():
            group = key([l.replace("      group:", "    group:") for l in self.jobs[job]], "group")
            self.assertEqual(group, "${{ inputs.action == 'status' && format('store-status-{0}', github.run_id) "
                                    f"|| format('{prefix}-{{0}}', github.repository) }}}}", job)
            self.assertIn("      cancel-in-progress: false", self.jobs[job])
        for job in ("checks", "record"):
            self.assertIsNone(key(self.jobs[job], "concurrency"), job)
        self.assertEqual(key(self.jobs["checks"], "if"), "inputs.action != 'status'")
        for job in ("play", "appstore"):
            self.assertIn("(inputs.action == 'status' || needs.checks.result == 'success')", key(self.jobs[job], "if"))

    def test_the_record_job_writes_only_after_a_real_store_action(self):
        condition = key(self.jobs["record"], "if")
        self.assertIn("!inputs.dry_run", condition)
        self.assertIn("needs.checks.result == 'success'", condition)
        play = "\n".join(self.jobs["play"])
        # an upload that only validated is no release
        self.assertIn('if [ "$mode" = commit ]; then echo "done=true"', play)
        self.assertIn('mode=commit\n          if [ "$DRY_RUN" = true ]; then mode=validate; fi', play)

    def test_the_upload_goes_to_alpha_with_the_source_line(self):
        play = "\n".join(self.jobs["play"])
        self.assertIn("--track alpha", play)
        self.assertNotIn("--track internal", self.text)
        self.assertIn("--in-review \"$IN_REVIEW\"", play)
        self.assertIn("cat store/play/ko-KR/release_notes.txt", play)
        self.assertIn("SOURCE_REPOSITORY: ${{ job.workflow_repository }}", play)

    def test_no_expression_inside_a_run_block(self):
        # inputs reach the shell through env only: an expression in a script is a script injection
        lines = self.text.splitlines()
        blocks = 0
        for i, line in enumerate(lines):
            m = re.match(r"^(\s*)(?:- )?run: ?(.*)$", line)
            if not m:
                continue
            blocks += 1
            indent = len(m.group(1))
            body = [m.group(2)]
            for later in lines[i + 1:]:
                if later.strip() and len(later) - len(later.lstrip()) <= indent:
                    break
                body.append(later)
            self.assertNotIn("${{", "\n".join(body), line)
        self.assertGreater(blocks, 10)

    def test_the_review_values_reach_prepare_and_submit_only(self):
        (asc,) = [s for s in steps(self.jobs["appstore"]) if name_of(s) == "App Store Connect"]
        for name in ("REVIEW_DEMO_USER", "REVIEW_DEMO_PASSWORD", "REVIEW_DEMO_PC_ID", "REVIEW_DEMO_PC_PASSWORD",
                     "REVIEW_CONTACT_FIRST_NAME", "REVIEW_CONTACT_LAST_NAME", "REVIEW_CONTACT_PHONE", "REVIEW_CONTACT_EMAIL"):
            self.assertIn(f"          {name}: ${{{{ (inputs.action == 'prepare' || inputs.action == 'submit') "
                          f"&& secrets.{name} || '' }}}}\n", asc, name)
        (notes,) = [s for s in steps(self.jobs["appstore"]) if name_of(s) == "Check the filled review notes"]
        self.assertIn("if: inputs.action == 'prepare' || inputs.action == 'submit'\n", notes)
        # the free-text inputs of a status run are checked before they reach the script
        first = steps(self.jobs["appstore"])[0]
        self.assertIn('[[ "$VERSION" =~', first)
        self.assertIn('[[ "$BUILD" =~ ^[0-9]{1,18}$ ]]', first)

    def test_the_bundle_provenance_is_hjv_releases(self):
        (step,) = [s for s in steps(self.jobs["checks"]) if name_of(s) == "Check the build run"]
        self.assertIn("scripts/store-record.py check-run --repo \"$GITHUB_REPOSITORY\" --store play", step)
        self.assertIn('--run-id "$RUN_ID" --source "$SOURCE_SHA"', step)
        self.assertIn("SOURCE_SHA: ${{ steps.inputs.outputs.source_sha }}", step)

    def test_release_sends_no_texts_and_the_demo_account_ends_in_kst(self):
        (step,) = [s for s in steps(self.jobs["checks"]) if name_of(s) == "Check the inputs, the tag and the store texts"]
        self.assertIn('[ "$ACTION" != release ] && [ "$(cat "store/$store/version.txt")" != "$VERSION" ]', step)
        self.assertIn('if [ "$ACTION" != release ]; then python3 -I scripts/check-store-text.py', step)
        (until,) = [s for s in steps(self.jobs["checks"]) if name_of(s) == "Check the demo account's end date"]
        self.assertIn('date -d "$UNTIL 00:00 +0900" +%s', until)
        self.assertIn('if [ "$now" -ge "$end" ]; then', until)


class ClientBuildWorkflowTest(unittest.TestCase):
    def setUp(self):
        self.text = read("client-build.yml")
        self.jobs = jobs(self.text)

    def test_no_google_play_job(self):
        self.assertNotIn("play", self.jobs)
        for word in ("PLAY_SA_", "play-upload.py", "Google Play internal", "google-play-"):
            self.assertNotIn(word, self.text)

    def test_play_is_refused_before_any_build(self):
        (step,) = [s for s in steps(self.jobs["preflight"]) if name_of(s) == "Check the Google Play input"]
        self.assertIn("        if: inputs.play\n", step)
        self.assertIn("exit 1", step)
        self.assertIn("store.yml", step)
        # the input stays declared: the build repository's build.yml passes it (provenance's shape)
        for trigger in ("workflow_dispatch", "workflow_call"):
            self.assertIn("      play:\n", self.text.split(trigger, 1)[1])

    def test_the_play_bundle_is_kept_30_days(self):
        (step,) = [s for s in steps(self.jobs["android"]) if name_of(s) == "Upload Play bundle"]
        self.assertIn("retention-days: ${{ inputs.check_only && 1 || 30 }}\n", step)
        others = [s for job in self.jobs.values() for s in steps(job)
                  if "retention-days:" in s and name_of(s) != "Upload Play bundle"]
        self.assertTrue(others)
        for s in others:
            self.assertIn("retention-days: ${{ env.ARTIFACT_RETENTION_DAYS }}\n", s, name_of(s))

    def test_the_release_checks_come_first(self):
        expected = {
            "android": ["Check out the release record check", "Check the release record"],
            "ios": ["Check out the release record check", "Check the release record"],
            # the upload: before the IPA is fetched, and in a run that builds nothing (no check build)
            "testflight": ["Check out the release record check", "Check the release record and the build run",
                           "Check that TestFlight has no build of this version", "Remove the release record check"],
        }
        for job, names in expected.items():
            found = steps(self.jobs[job])
            self.assertEqual([name_of(s) for s in found[:len(names)]], names, job)
            for s in found[:len(names)]:
                if job != "testflight":
                    self.assertIn("!inputs.check_only", s, name_of(s))
                uses = re.findall(r"uses: ([\w./-]+)@", s)
                self.assertTrue(set(uses) <= {"actions/checkout"}, uses)
            checkout = found[0]
            self.assertIn("path: release-checks\n", checkout)
            self.assertIn("persist-credentials: false\n", checkout)
            self.assertIn("sparse-checkout-cone-mode: false\n", checkout)
            self.assertEqual("scripts/asc-store.mjs" in checkout, job == "testflight", job)
            store = {"android": "play", "ios": "appstore", "testflight": "appstore"}[job]
            self.assertIn(f"check-build --repo \"$GITHUB_REPOSITORY\" --store {store} --version \"$HJV_VERSION\"", found[1])
            self.assertNotIn("secrets.", found[1])
            # the checked-out scripts are gone before any build or download
            self.assertIn("rm -rf release-checks", found[len(names) - 1])

    def test_build_names_carry_exactly_seven_digits_of_the_commit(self):
        # the store run looks for HJViewer-<version>-<7 digits>-android-aarch64-play; --short=7
        # gives more digits when 7 are ambiguous
        self.assertNotIn("--short", self.text)
        self.assertEqual(self.text.count('name="$HJV_VERSION-$(git rev-parse HEAD | cut -c1-7)"'), 3)

    def test_the_upload_key_reaches_two_steps_of_the_upload_job_only(self):
        holders = [name_of(s) for s in steps(self.jobs["testflight"]) if "secrets.ASC_" in s]
        self.assertEqual(holders, ["Check that TestFlight has no build of this version", "Upload to TestFlight"])
        (check,) = [s for s in steps(self.jobs["testflight"]) if name_of(s) == holders[0]]
        self.assertIn("asc-store.mjs check-new --version", check)
        for job in set(self.jobs) - {"testflight"}:
            self.assertNotIn("secrets.ASC_", "\n".join(self.jobs[job]), job)
        # the job that holds it signs nothing and builds nothing
        text = "\n".join(self.jobs["testflight"])
        for word in ("secrets.IOS_", "flutter build", "cargo", "xcodebuild"):
            self.assertNotIn(word, text)

    def test_testflight_takes_the_ipa_of_a_checked_build_run(self):
        """The build run uploads nothing; a later run with testflight and testflight_run sends its IPA,
        after the build run's provenance check (store-record.py check-run, hjv-release's rules)."""
        job = self.jobs["testflight"]
        self.assertEqual(key(job, "needs"), "preflight")
        self.assertEqual(key(job, "if"), "needs.preflight.outputs.testflight == 'true'")
        self.assertEqual(block(job, "permissions"), ["contents: read", "actions: read"])
        (check,) = [s for s in steps(job) if name_of(s) == "Check the release record and the build run"]
        self.assertIn('check-run --repo "$GITHUB_REPOSITORY" --store appstore', check)
        self.assertIn('--run-id "$TESTFLIGHT_RUN" --source "$SOURCE_SHA"', check)
        self.assertIn("TESTFLIGHT_RUN: ${{ inputs.testflight_run }}", check)
        (download,) = [s for s in steps(job) if name_of(s) == "Download the IPA"]
        self.assertIn("name: ${{ steps.build-run.outputs.artifact }}\n", download)
        self.assertIn("run-id: ${{ inputs.testflight_run }}\n", download)
        names = [name_of(s) for s in steps(job)]
        self.assertEqual(names[-3:], ["Download the IPA", "Upload to TestFlight", "Remove the API key"])
        (upload,) = [s for s in steps(job) if name_of(s) == "Upload to TestFlight"]
        for needed in ("shasum -a 256 -c SHA256SUMS", "release.json", "ProvisionedDevices", "get-task-allow",
                       "ProvisionsAllDevices", "xcrun altool --upload-package", '"product-errors"'):
            self.assertIn(needed, upload)
        # the build jobs hand nothing out: no altool, no App Store Connect key
        for build in ("windows", "android", "ios"):
            self.assertNotIn("altool", "\n".join(self.jobs[build]), build)
        self.assertEqual(key(self.jobs["ios"], "needs"), "[preflight, generate-bridge]")


    def test_the_run_summary_never_fails_the_run(self):
        """GHA-09: the release tool takes successful runs only, so the summary may not fail one."""
        job = self.jobs["summary"]
        self.assertEqual(key(job, "continue-on-error"), "true")
        self.assertEqual(key(job, "if"), "${{ !cancelled() }}")
        self.assertEqual(block(job, "permissions"), ["contents: read", "actions: read"])
        needs = key(job, "needs").strip("[]").split(", ")
        self.assertEqual(sorted(needs), sorted(set(self.jobs) - {"summary"}))
        (write,) = [s for s in steps(job) if name_of(s) == "Write the run summary"]
        self.assertIn("NEEDS: ${{ toJSON(needs) }}", write)
        self.assertIn("scripts/run-summary.py", write)
        self.assertIn(">> \"$GITHUB_STEP_SUMMARY\"", write)
        # what the summary reads: every Rust job's image and cache, the vcpkg jobs' packages
        for build in ("generate-bridge", "windows", "android", "ios"):
            outputs = block(self.jobs[build], "outputs")
            self.assertIn("image: ${{ steps.image.outputs.image }}", outputs, build)
            self.assertIn("rust_cache: ${{ steps.rust-cache.outputs.cache-hit }}", outputs, build)
            if build != "generate-bridge":
                self.assertIn("vcpkg_restored: ${{ steps.vcpkg-fp.outputs.restored }}", outputs, build)
                self.assertIn("vcpkg_packages: ${{ steps.vcpkg-fp.outputs.packages }}", outputs, build)
            (image,) = [s for s in steps(self.jobs[build]) if name_of(s) == "Runner image and rustup directory"]
            # actions/cache/restore leaves cache-hit unset on a miss, which the summary reads as generated
            if build == "generate-bridge":
                self.assertIn("bridge_cache: ${{ steps.bridge-out.outputs.cache-hit || 'false' }}", outputs)
            self.assertIn('echo "RUSTUP_HOME=$RUNNER_TEMP/rustup" >> "$GITHUB_ENV"', image)
            names = [name_of(s) for s in steps(self.jobs[build])]
            self.assertEqual(names.index("Install Rust toolchain"), names.index("Runner image and rustup directory") + 1, build)


class RunSummaryTest(unittest.TestCase):
    """scripts/run-summary.py on answers of the jobs API, cut down to the fields it reads."""

    JOBS = [
        {"name": "client / Checks", "status": "completed", "conclusion": "success", "labels": ["ubuntu-24.04"],
         "created_at": "2026-10-26T15:03:30Z", "started_at": "2026-10-26T15:03:40Z",
         "completed_at": "2026-10-26T15:04:10Z",
         "steps": [{"name": "Set up job", "status": "completed", "conclusion": "success",
                    "started_at": "2026-10-26T15:03:40Z", "completed_at": "2026-10-26T15:03:43Z"},
                   {"name": "Check the Google Play input", "status": "completed", "conclusion": "skipped",
                    "started_at": "2026-10-26T15:03:44Z", "completed_at": "2026-10-26T15:03:44Z"}]},
        {"name": "client / Windows x64", "status": "completed", "conclusion": "success", "labels": ["windows-2022"],
         "created_at": "2026-10-26T15:04:12Z", "started_at": "2026-10-26T15:04:18Z",
         "completed_at": "2026-10-26T15:31:18Z",
         "steps": [{"name": "Build rustdesk", "status": "completed", "conclusion": "success",
                    "started_at": "2026-10-26T15:12:00Z", "completed_at": "2026-10-26T15:24:30Z"},
                   {"name": "Build rustdesk lib | x", "status": "completed", "conclusion": "success",
                    "started_at": "2026-10-26T15:24:30Z", "completed_at": "2026-10-26T15:25:00Z"}]},
        {"name": "client / Run summary", "status": "in_progress", "conclusion": None, "labels": ["ubuntu-24.04"],
         "created_at": "2026-10-26T15:31:20Z", "started_at": "2026-10-26T15:31:25Z", "completed_at": None,
         "steps": []},
    ]
    NEEDS = {
        "windows": {"result": "success", "outputs": {"image": "win22 20261022.344", "rust_cache": "true",
                                                     "vcpkg_restored": "16", "vcpkg_packages": "16"}},
        "generate-bridge": {"result": "success", "outputs": {"image": "", "rust_cache": "", "bridge_cache": "true"}},
        "android": {"result": "success", "outputs": {"image": "ubuntu24 20261012.334", "rust_cache": "false",
                                                     "vcpkg_restored": "0", "vcpkg_packages": "26",
                                                     "disk_free": "21G"}},
        "relay": {"result": "skipped", "outputs": {}},
    }

    def summary(self, text, needs=None):
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "jobs.json")
            with open(path, "w", encoding="utf-8") as handle:
                handle.write(text)
            out = io.StringIO()
            env = {} if needs is None else {"NEEDS": json.dumps(needs)}
            with redirect_stdout(out):
                code = rs.main([path], env)
        return code, out.getvalue()

    def test_times_of_the_jobs_and_their_steps(self):
        # gh --jq prints one job after another; the API's pages work as well
        for text in ("\n".join(json.dumps(j, indent=1) for j in self.JOBS), json.dumps({"jobs": self.JOBS})):
            code, out = self.summary(text)
            self.assertEqual(code, 0)
            self.assertIn("From the first job's start to the last job's end: 27.8 minutes.", out)
            self.assertIn("| client / Checks | success | 0.2 | 0.5 | ubuntu-24.04 |", out)
            self.assertIn("| client / Windows x64 | success | 0.8 | 27.0 | windows-2022 |", out)
            self.assertIn("| client / Run summary | in progress | 27.9 |  | ubuntu-24.04 |", out)
            self.assertIn("<details><summary>Steps of client / Windows x64 (27.0 minutes)</summary>", out)
            self.assertIn("| Build rustdesk | success | 12.5 |", out)
            # names as they are, never matched by prefix; a separator in one stays in its cell
            self.assertIn("| Build rustdesk lib \\| x | success | 0.5 |", out)
            # skipped steps make no line
            self.assertNotIn("Check the Google Play input", out)
            self.assertNotIn("Runner images and caches", out)

    def test_images_and_caches_from_the_jobs_outputs(self):
        code, out = self.summary(json.dumps(self.JOBS[0]), self.NEEDS)
        self.assertEqual(code, 0)
        self.assertIn("| windows | win22 20261022.344 | hit | 16 of 16 |  |", out)
        self.assertIn("| android | ubuntu24 20261012.334 | miss or fallback | 0 of 26 | disk left 21G |", out)
        self.assertIn("| generate-bridge |  |  |  | bridge files from the cache |", out)
        self.assertNotIn("| relay |", out)
        # a miss, which the Bridge codegen job's output gives as 'false', with its Rust cache
        missed = {"generate-bridge": {"result": "success", "outputs": {
            "image": "ubuntu22 20261023.322.3", "rust_cache": "true", "bridge_cache": "false"}}}
        code, out = self.summary(json.dumps(self.JOBS[0]), missed)
        self.assertEqual(code, 0)
        self.assertIn("| generate-bridge | ubuntu22 20261023.322.3 | hit |  | bridge files generated |", out)

    def test_what_it_cannot_read_becomes_a_line(self):
        code, out = self.summary("{not json", {"windows": {"outputs": {"image": "x"}}})
        self.assertEqual(code, 0)
        self.assertIn("The run's jobs could not be read", out)
        self.assertIn("| windows | x |", out)
        with tempfile.TemporaryDirectory() as tmp, redirect_stdout(io.StringIO()):
            self.assertEqual(rs.main([os.path.join(tmp, "missing.json")], {"NEEDS": "not json"}), 0)


if __name__ == "__main__":
    unittest.main()

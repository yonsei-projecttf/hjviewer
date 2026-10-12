# SPDX-License-Identifier: AGPL-3.0-only
"""Tests of scripts/leak-check.py (scripts/ci.sh leak) on throwaway repositories, with made-up
values from the documentation ranges, never real ones:

    python3 -I -B -m unittest discover -s scripts -p 'test_leak_check.py'
"""
import os
import shutil
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, "leak-check.py")

LIST = """# made-up values for the tests
leak-canary.example.net

198.51.100.
00:00:5E:00:53:77
부서-시험-공유폴더
Test-Portal-0001
"""
# the list's line numbers of the values above
DOMAIN, PREFIX, MAC, FOLDER, PORTAL = 2, 4, 5, 6, 7


class Repo:
    """A clone of a bare origin whose main has one pushed commit, and a list in build/."""

    def __init__(self):
        self.tmp = tempfile.mkdtemp()
        self.env = {
            "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
            "HOME": self.tmp,
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_AUTHOR_NAME": "tester", "GIT_AUTHOR_EMAIL": "tester@example.org",
            "GIT_COMMITTER_NAME": "tester", "GIT_COMMITTER_EMAIL": "tester@example.org",
        }
        self.origin = os.path.join(self.tmp, "origin.git")
        self.dir = os.path.join(self.tmp, "work")
        self.git("init", "-q", "--bare", "-b", "main", self.origin, cwd=self.tmp)
        self.git("clone", "-q", self.origin, self.dir, cwd=self.tmp)
        self.write(".gitignore", "/build/\n")
        self.write("README.md", "# test\n")
        self.commit("first")
        self.git("push", "-q", "origin", "HEAD:main")
        self.git("fetch", "-q", "origin")
        os.makedirs(os.path.join(self.dir, "build"))
        self.write("build/leak-patterns.txt", LIST)

    def git(self, *args, cwd=None):
        return subprocess.run(["git", *args], cwd=cwd or self.dir, env=self.env, check=True,
                              capture_output=True, text=True).stdout

    def write(self, rel, text, mode="w"):
        path = os.path.join(self.dir, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, mode) as f:
            f.write(text)

    def commit(self, message, *paths):
        self.git("add", *(paths or ["-A"]))
        self.git("commit", "-q", "-m", message)

    def check(self, *args):
        done = subprocess.run(["python3", "-I", SCRIPT, "--repo", self.dir, *args], env=self.env,
                              capture_output=True, text=True)
        return done.returncode, done.stdout + done.stderr

    def close(self):
        shutil.rmtree(self.tmp)


class LeakCheckTest(unittest.TestCase):
    def setUp(self):
        self.repo = Repo()

    def tearDown(self):
        self.repo.close()

    def assertNoValue(self, out):
        low = out.lower()
        for value in ("leak-canary", "198.51.100", "5e:00:53", "5e-00-53", "5e0053", "부서-시험", "portal-0001"):
            self.assertNotIn(value, low)

    def test_a_clean_repository_passes(self):
        code, out = self.repo.check()
        self.assertEqual(code, 0, out)
        self.assertIn("5 values", out)

    def test_a_tracked_file_fails_with_place_and_list_line_only(self):
        self.repo.write("docs/setup.md", "line one\nserver: LEAK-CANARY.example.NET:443\n")
        self.repo.write("client/patches/0099-x.patch", "+const NAS: &str = \"198.51.100.20\";\n")
        self.repo.commit("docs: setup")
        code, out = self.repo.check()
        self.assertEqual(code, 1, out)
        self.assertIn(f"docs/setup.md:2: list line {DOMAIN}", out)
        self.assertIn(f"client/patches/0099-x.patch:1: list line {PREFIX}", out)
        self.assertNoValue(out)

    def test_an_uncommitted_tracked_file_counts(self):
        self.repo.write("notes.txt", "old\n")
        self.repo.commit("notes")
        self.repo.write("notes.txt", "old\nTest-PORTAL-0001\n")
        code, out = self.repo.check()
        self.assertEqual(code, 1)
        self.assertIn(f"notes.txt:2: list line {PORTAL}", out)

    def test_a_commit_message_fails(self):
        self.repo.write("a.txt", "a\n")
        self.repo.commit("fix: the relay\n\nTested against leak-canary.example.net from the lab.")
        sha = self.repo.git("rev-parse", "HEAD").strip()[:10]
        code, out = self.repo.check()
        self.assertEqual(code, 1, out)
        self.assertIn(f"commit {sha}: message line 3: list line {DOMAIN}", out)
        self.assertNoValue(out)

    def test_a_value_removed_again_is_still_in_the_unpushed_history(self):
        self.repo.write("cfg.env", "HOST=198.51.100.7\n")
        self.repo.commit("add cfg")
        self.repo.write("cfg.env", "HOST=192.0.2.7\n")
        self.repo.commit("fix cfg")
        code, out = self.repo.check()
        self.assertEqual(code, 1, out)
        self.assertIn(f"cfg.env:1: list line {PREFIX}", out)
        self.assertEqual(out.count("cfg.env"), 1, out)

    def test_pushed_commits_are_not_checked_again(self):
        self.repo.write("a.txt", "a\n")
        self.repo.commit("mentions leak-canary.example.net")
        self.repo.git("push", "-q", "origin", "HEAD:main")
        self.repo.git("fetch", "-q", "origin")
        self.assertEqual(self.repo.check()[0], 0)

    def test_without_origin_main_every_commit_is_checked(self):
        self.repo.git("update-ref", "-d", "refs/remotes/origin/main")
        self.repo.git("commit", "-q", "--amend", "-m", "first, from leak-canary.example.net")
        code, out = self.repo.check()
        self.assertEqual(code, 1)
        self.assertIn("checking every commit", out)

    def test_staged_changes_count_even_when_the_working_tree_dropped_them(self):
        self.repo.write("b.txt", "folder: 부서-시험-공유폴더\n")
        self.repo.git("add", "b.txt")
        self.repo.write("b.txt", "folder: (removed)\n")
        code, out = self.repo.check()
        self.assertEqual(code, 1, out)
        self.assertIn(f"staged b.txt:1: list line {FOLDER}", out)
        self.assertNoValue(out)

    def test_mac_addresses_in_every_form(self):
        for i, form in enumerate(("00-00-5e-00-53-77", "00005E005377", "0000.5e00.5377", "00:00:5e:00:53:77")):
            self.repo.write(f"m{i}.txt", f"wol {form}\n")
        self.repo.commit("macs")
        code, out = self.repo.check()
        self.assertEqual(code, 1)
        for i in range(4):
            self.assertIn(f"m{i}.txt:1: list line {MAC}", out)
        self.assertNoValue(out)

    def test_binary_files(self):
        self.repo.write("icon.bin", b"\x89PNG\0\0leak-canary.example.net\0".decode("latin-1"), mode="w")
        self.repo.commit("icon")
        code, out = self.repo.check()
        self.assertEqual(code, 1)
        self.assertIn(f"icon.bin (binary): list line {DOMAIN}", out)

    def test_authors_count(self):
        self.repo.write("c.txt", "c\n")
        self.repo.git("add", "c.txt")
        self.repo.git("-c", "user.name=x", "commit", "-q", "-m", "c", "--author", "x <ops@leak-canary.example.net>")
        code, out = self.repo.check()
        self.assertEqual(code, 1)
        self.assertIn(f": author: list line {DOMAIN}", out)

    def test_the_list_is_needed_and_must_hold_values(self):
        os.remove(os.path.join(self.repo.dir, "build", "leak-patterns.txt"))
        code, out = self.repo.check()
        self.assertNotEqual(code, 0)
        self.assertIn("no list", out)
        self.repo.write("build/leak-patterns.txt", "# only comments\n\n")
        code, out = self.repo.check()
        self.assertNotEqual(code, 0)
        self.assertIn("lists no value", out)

    def test_the_list_must_not_be_tracked(self):
        self.repo.write("leaks.txt", "leak-canary.example.net\n")
        self.repo.commit("oops")
        code, out = self.repo.check("--patterns", os.path.join(self.repo.dir, "leaks.txt"))
        self.assertNotEqual(code, 0)
        self.assertIn("tracked by git", out)
        self.assertNoValue(out)

    def test_a_linked_worktree_uses_the_main_worktrees_list(self):
        wt = os.path.join(self.repo.tmp, "wt")
        self.repo.git("worktree", "add", "-q", "-b", "side", wt)
        with open(os.path.join(wt, "d.txt"), "w") as f:
            f.write("Test-Portal-0001\n")
        done = subprocess.run(["python3", "-I", SCRIPT, "--repo", wt], env=self.repo.env,
                              capture_output=True, text=True)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)  # d.txt is not tracked
        subprocess.run(["git", "add", "d.txt"], cwd=wt, env=self.repo.env, check=True)
        done = subprocess.run(["python3", "-I", SCRIPT, "--repo", wt], env=self.repo.env,
                              capture_output=True, text=True)
        self.assertEqual(done.returncode, 1)
        self.assertIn(os.path.join(self.repo.dir, "build", "leak-patterns.txt"), done.stdout)


if __name__ == "__main__":
    unittest.main()

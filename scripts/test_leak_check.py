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

    def test_mac_addresses_listed_in_any_form(self):
        for listed in ("00005E005377", "0000.5e00.5377", "00-00-5E-00-53-77"):
            self.repo.write("build/leak-patterns.txt", f"{listed}\n")
            for i, form in enumerate(("00-00-5e-00-53-77", "00005E005377", "0000.5e00.5377", "00:00:5e:00:53:77")):
                self.repo.write(f"m{i}.txt", f"wol {form}\n")
            self.repo.git("add", "-A")
            code, out = self.repo.check()
            self.assertEqual(code, 1, listed)
            for i in range(4):
                self.assertIn(f"m{i}.txt:1: list line 1", out, listed)
            self.assertNoValue(out)

    def test_lists_saved_by_windows_editors(self):
        """Notepad's "UTF-8 with BOM" and PowerShell 5.1's Set-Content -Encoding UTF8 start the file
        with a BOM; CRLF line ends; a note after the value."""
        self.repo.write("docs/setup.md", "server: leak-canary.example.net\nid: Test-Portal-0001\n")
        self.repo.commit("docs")
        lists = {
            "BOM": "﻿leak-canary.example.net\r\nTest-Portal-0001\r\n",
            "a note after the value": "leak-canary.example.net   # 운영 도메인\nTest-Portal-0001\t# 시험 ID\n",
            "a note without a space": "leak-canary.example.net# 운영\nTest-Portal-0001\n",
            "old Mac line ends": "leak-canary.example.net\rTest-Portal-0001\r",
        }
        for what, text in lists.items():
            self.repo.write("build/leak-patterns.txt", text.encode("utf-8"), mode="wb")
            code, out = self.repo.check()
            self.assertEqual(code, 1, f"{what}: {out}")
            self.assertIn("docs/setup.md:1: list line 1", out, what)
            self.assertIn("docs/setup.md:2: list line 2", out, what)
            self.assertNoValue(out)

    def test_lists_that_would_match_nothing_stop_the_check(self):
        """UTF-16 (PowerShell 5.1's '>' and Out-File), other encodings, invisible characters: such a
        value finds nothing, so the check stops instead of passing."""
        self.repo.write("docs/setup.md", "server: leak-canary.example.net\n")
        self.repo.commit("docs")
        lists = {
            "UTF-16 with BOM": "leak-canary.example.net\r\n".encode("utf-16"),
            "UTF-16 without BOM": "leak-canary.example.net\r\n".encode("utf-16-le"),
            "EUC-KR": "부서-시험-공유폴더\n".encode("euc-kr"),
            "a zero-width space": "leak-canary​.example.net\n".encode("utf-8"),
            "a no-break space": "부서 시험\n".encode("utf-8"),
        }
        for what, data in lists.items():
            self.repo.write("build/leak-patterns.txt", data, mode="wb")
            code, out = self.repo.check()
            self.assertNotEqual(code, 0, what)
            self.assertNotIn("leak: none", out, what)
            self.assertIn("leak-patterns.txt", out, what)
            self.assertNoValue(out)

    def test_korean_text_in_either_unicode_form(self):
        import unicodedata
        self.repo.write("a.md", "폴더: " + unicodedata.normalize("NFD", "부서-시험-공유폴더") + "\n")
        self.repo.commit("nfd")
        code, out = self.repo.check()
        self.assertEqual(code, 1, out)
        self.assertIn(f"a.md:1: list line {FOLDER}", out)

    def test_paths_count_and_are_printed_without_the_value(self):
        self.repo.write("docs/leak-canary.example.net.md", "nothing here\n")
        self.repo.write("cfg/Test-Portal-0001/a.txt", "198.51.100.9\n")
        self.repo.commit("paths")
        code, out = self.repo.check()
        self.assertEqual(code, 1, out)
        self.assertIn(f"docs/<listed value>.md (in the path): list line {DOMAIN}", out)
        self.assertIn(f"cfg/<listed value>/a.txt (in the path): list line {PORTAL}", out)
        # a hit in the contents of such a file names it the same way
        self.assertIn(f"cfg/<listed value>/a.txt:1: list line {PREFIX}", out)
        self.assertNoValue(out)

    def test_a_path_renamed_again_is_still_in_the_unpushed_history(self):
        self.repo.write("notes/Test-Portal-0001.txt", "x\n")
        self.repo.commit("add notes")
        sha = self.repo.git("rev-parse", "HEAD").strip()[:10]
        self.repo.git("mv", "notes/Test-Portal-0001.txt", "notes/x.txt")
        self.repo.write("empty-leak-canary.example.net", "")
        self.repo.commit("rename")
        self.repo.git("rm", "-q", "empty-leak-canary.example.net")
        self.repo.commit("remove")
        code, out = self.repo.check()
        self.assertEqual(code, 1, out)
        self.assertIn(f"commit {sha}: notes/<listed value>.txt (in the path): list line {PORTAL}", out)
        self.assertIn(f"empty-<listed value> (in the path): list line {DOMAIN}", out)
        self.assertNotIn("notes/x.txt", out)
        self.assertNoValue(out)

    def test_binary_files(self):
        self.repo.write("icon.bin", b"\x89PNG\0\0leak-canary.example.net\0".decode("latin-1"), mode="w")
        self.repo.commit("icon")
        code, out = self.repo.check()
        self.assertEqual(code, 1)
        self.assertIn(f"icon.bin (binary): list line {DOMAIN}", out)

    def test_values_in_utf16_in_binary_files(self):
        """An APK's manifest (Android's binary XML) keeps its strings in UTF-16LE, a binary property
        list (an IPA's Info.plist) and a font's names in UTF-16BE."""
        manifest = b"\x03\x00\x08\x00" + "LEAK-canary.example.net".encode("utf-16-le") + b"\0\0"
        self.repo.write("AndroidManifest.xml", manifest, mode="wb")
        self.repo.write("Info.plist", b"bplist00\x6a" + "부서-시험-공유폴더".encode("utf-16-be"), mode="wb")
        self.repo.commit("samples")
        code, out = self.repo.check()
        self.assertEqual(code, 1, out)
        self.assertIn(f"AndroidManifest.xml (binary): list line {DOMAIN}", out)
        self.assertIn(f"Info.plist (binary): list line {FOLDER}", out)
        self.assertNoValue(out)

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

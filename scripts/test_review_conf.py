# SPDX-License-Identifier: AGPL-3.0-only
"""Offline tests of deploy/review-conf.py, which writes the file of hjv-api's App Review account,
with made-up values only:

    python3 -I -B -m unittest discover -s scripts -p 'test_review_conf.py'

The ID and the password are asked through stand-ins for input and getpass, on a stand-in
terminal; the command itself runs once more as a server would run it, with a standard input that is
no terminal.
"""
import datetime
import getpass
import hashlib
import importlib.util
import io
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import types
import unittest
import warnings
from contextlib import redirect_stderr, redirect_stdout
from unittest import mock

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SCRIPT = os.path.join(ROOT, "deploy", "review-conf.py")

_spec = importlib.util.spec_from_file_location("review_conf", SCRIPT)
rc = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(rc)

ID = "APPREVIEW-TEST"
PASSWORD = "correct horse battery staple"
# 2026-10-16 12:00 KST
NOW = datetime.datetime(2026, 10, 16, 3, 0, tzinfo=datetime.timezone.utc)


class Terminal(io.StringIO):
    def __init__(self, tty=True):
        super().__init__()
        self.tty = tty

    def isatty(self):
        return self.tty


def answers(*typed):
    """A stand-in for getpass.getpass that gives <typed> one after another."""
    asked = []

    def ask(prompt):
        asked.append(prompt)
        return typed[len(asked) - 1]
    ask.asked = asked
    return ask


class ReviewConfTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.tmp)
        self.out = os.path.join(self.tmp, "review.conf")

    def run_main(self, argv, ask, tty=True, typed_id=ID):
        """main() on a stand-in terminal where <typed_id> is typed for the ID; the questions asked
        for it go to self.id_asked."""
        self.id_asked = []

        def read(prompt):
            self.id_asked.append(prompt)
            return typed_id
        stdout, stderr = io.StringIO(), io.StringIO()
        with redirect_stdout(stdout), redirect_stderr(stderr):
            code = rc.main(argv, stdin=Terminal(tty), ask=ask, now=NOW, read=read)
        return code, stdout.getvalue(), stderr.getvalue()

    def args(self, until="2026-11-30"):
        return ["--until", until, "--out", self.out]

    def test_the_file(self):
        code, out, err = self.run_main(self.args(), answers(PASSWORD, PASSWORD),
                                       typed_id=f" {ID} ")
        self.assertEqual(code, 0)
        # without --pc, one line on stderr says the file has no PC= line (runbook 7-17)
        self.assertEqual(err.count("\n"), 1)
        self.assertIn("note: no --pc, so no PC= line", err)
        self.assertEqual(self.id_asked, ["App Review ID: "])
        # only the path and UNTIL are printed
        self.assertEqual(out, f"{self.out}\nUNTIL=2026-11-29T15:00:00Z\n")
        self.assertEqual(stat.S_IMODE(os.stat(self.out).st_mode), 0o600)
        with open(self.out, encoding="ascii") as f:
            lines = [line for line in f.read().splitlines() if not line.startswith("#")]
        self.assertEqual(lines[0], f"ID={ID}")
        self.assertEqual(lines[2], "UNTIL=2026-11-29T15:00:00Z")
        scheme, iterations, salt, key = lines[1].removeprefix("HASH=").split("$")
        self.assertEqual((scheme, iterations), ("pbkdf2-sha256", "600000"))
        salt = rc.base64.b64decode(salt, validate=True)
        self.assertEqual(len(salt), 16)
        self.assertEqual(rc.base64.b64decode(key, validate=True),
                         hashlib.pbkdf2_hmac("sha256", PASSWORD.encode(), salt, 600000, 32))
        self.assertEqual(len(lines), 3)
        with open(self.out, encoding="ascii") as f:
            first = f.read()
        self.assertNotIn(PASSWORD, out + err + first)
        # a new salt each time
        other = os.path.join(self.tmp, "other.conf")
        code, _, _ = self.run_main(["--until", "2026-11-30", "--out", other],
                                   answers(PASSWORD, PASSWORD))
        self.assertEqual(code, 0)
        with open(other, encoding="ascii") as f:
            self.assertNotEqual(f.read(), first)

    def test_the_demo_pcs(self):
        code, out, err = self.run_main(self.args() + ["--pc", "100200301", "--pc", "Office-PC_7"],
                                       answers(PASSWORD, PASSWORD))
        self.assertEqual((code, err), (0, ""))
        self.assertEqual(out, f"{self.out}\nUNTIL=2026-11-29T15:00:00Z\nPC=100200301,Office-PC_7\n")
        with open(self.out, encoding="ascii") as f:
            lines = [line for line in f.read().splitlines() if not line.startswith("#")]
        # the line hjv-api reads (server/api/src/review.rs): the IDs in their order
        self.assertEqual(lines[3], "PC=100200301,Office-PC_7")
        self.assertEqual(len(lines), 4)
        for pcs, why in ((["1", "2", "3", "4"], "at most 3"),
                         (["100200300", "100200300"], "twice"),
                         (["a.b"], "a PC's ID"),
                         (["1 2"], "a PC's ID"),
                         ([""], "a PC's ID"),
                         (["1" * 65], "a PC's ID")):
            if os.path.lexists(self.out):
                os.remove(self.out)
            ask = answers(PASSWORD, PASSWORD)
            argv = self.args()
            for pc in pcs:
                argv += ["--pc", pc]
            code, out, err = self.run_main(argv, ask)
            self.assertEqual((code, out), (1, ""), pcs)
            self.assertIn(why, err)
            # refused before anything is asked
            self.assertEqual((self.id_asked, ask.asked), ([], []))
            self.assertFalse(os.path.lexists(self.out))

    def test_the_hash_is_the_servers(self):
        # server/api/src/review.rs and tests/review.rs check the same line with ring
        self.assertEqual(
            rc.hash_of(PASSWORD, bytes(range(16)), 100000),
            "pbkdf2-sha256$100000$AAECAwQFBgcICQoLDA0ODw==$SdScJfWXhGIJ8Nkud3CrZOHHXpS0zmxQkmXuZxddKh4=")
        # and deploy/review.conf.example shows what the tool writes
        with open(os.path.join(ROOT, "deploy", "review.conf.example"), encoding="utf-8") as f:
            example = f.read().splitlines()
        self.assertIn(f"#   HASH={rc.hash_of(PASSWORD, bytes(range(16)))}", example)
        self.assertIn(f"#   ID={ID}", example)
        self.assertIn(f"#   UNTIL={rc.until_of('2026-11-30', NOW)}", example)
        self.assertFalse([line for line in example if line and not line.startswith("#")])

    def test_ids(self):
        for good in (ID, "appreview-test", "ZATEST01", "BZTEST01", "Z", "z.test", "1234567890",
                     "A" * 64):
            self.assertEqual(rc.check_id(good), good)
        # HJViewer's ID rule (server/api/src/util.rs staff_id): a staff account's ID never
        for staff in ("ZZTEST01", "yztest03", "Z1", "zz_demo-01", "Y" + "9" * 59):
            with self.assertRaisesRegex(rc.Refused, "passes HJViewer's ID rule"):
                rc.check_id(staff)
        for bad in ("", "two words", "A" * 65, "홍길동", "ab#cd", "ab=cd", "a\tb"):
            with self.assertRaisesRegex(rc.Refused, "1 to 64 letters"):
                rc.check_id(bad)
        # typed: refused before the password is asked, and the ID is not shown back
        ask = answers(PASSWORD, PASSWORD)
        code, out, err = self.run_main(self.args(), ask, typed_id="ZZTEST01")
        self.assertEqual((code, out), (1, ""))
        self.assertIn("ID rule", err)
        self.assertNotIn("ZZTEST01", err)
        self.assertEqual(ask.asked, [])
        self.assertFalse(os.path.lexists(self.out))

    def test_until(self):
        self.assertEqual(rc.until_of("2026-11-30", NOW), "2026-11-29T15:00:00Z")
        self.assertEqual(rc.until_of("2027-01-01", NOW), "2026-12-31T15:00:00Z")
        # 0:00 KST of 2026-10-17 is after NOW, of 2026-10-16 not
        self.assertEqual(rc.until_of("2026-10-17", NOW), "2026-10-16T15:00:00Z")
        for bad in ("2026-10-16", "2026-02-30", "20261130", "2026-11-30T00:00", "2026-1-30",
                    "２０２６-11-30", ""):
            with self.assertRaises(rc.Refused, msg=bad):
                rc.until_of(bad, NOW)
        # at most 90 days ahead: NOW is 2026-10-16 03:00 UTC, 90 days later 2027-01-14 03:00 UTC
        self.assertEqual(rc.until_of("2027-01-14", NOW), "2027-01-13T15:00:00Z")
        for far in ("2027-01-15", "2027-11-30", "9999-12-31"):
            with self.assertRaisesRegex(rc.Refused, "at most 90 days", msg=far):
                rc.until_of(far, NOW)

    def test_passwords(self):
        for typed, why in (((PASSWORD, "correct horse battery stapler"), "differ"),
                           (("too short", "too short"), "12 to 256"),
                           (("x" * 257, "x" * 257), "12 to 256"),
                           (("비밀번호를 길게 적었습니다", "비밀번호를 길게 적었습니다"), "printable ASCII")):
            code, out, err = self.run_main(self.args(), answers(*typed))
            self.assertEqual((code, out), (1, ""), why)
            self.assertIn(why, err)
            self.assertFalse(os.path.lexists(self.out))
            for t in typed:
                self.assertNotIn(t, err)

    def test_only_from_a_terminal(self):
        ask = answers(PASSWORD, PASSWORD)
        code, out, err = self.run_main(self.args(), ask, tty=False)
        self.assertEqual((code, out), (1, ""))
        self.assertIn("terminal", err)
        self.assertEqual((self.id_asked, ask.asked), ([], []))
        # where getpass cannot hide what is typed it warns before it reads: that refuses
        def echoing(prompt):
            warnings.warn("Can not control echo on the terminal.", getpass.GetPassWarning)
            return PASSWORD
        code, out, err = self.run_main(self.args(), echoing)
        self.assertEqual((code, out), (1, ""))
        self.assertIn("cannot hide", err)
        self.assertFalse(os.path.lexists(self.out))
        # no option takes the password or the ID
        for option in (["--password", PASSWORD], ["--id", ID]):
            with redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                rc.main(self.args() + option, stdin=Terminal(), ask=ask, now=NOW,
                        read=lambda prompt: ID)
        # where Python lacks O_NOFOLLOW (Windows), nothing is asked
        ask = answers(PASSWORD, PASSWORD)
        with mock.patch.object(rc, "os", types.SimpleNamespace(path=os.path)):
            code, out, err = self.run_main(self.args(), ask)
        self.assertEqual((code, out), (1, ""))
        self.assertIn("Linux, macOS or WSL", err)
        self.assertEqual((self.id_asked, ask.asked), ([], []))

    def test_never_over_a_file(self):
        for there in ("file", "link"):
            if there == "file":
                with open(self.out, "w", encoding="ascii") as f:
                    f.write("kept\n")
            else:
                os.remove(self.out)
                os.symlink(os.path.join(self.tmp, "nowhere"), self.out)
            ask = answers(PASSWORD, PASSWORD)
            code, out, err = self.run_main(self.args(), ask)
            self.assertEqual((code, out), (1, ""), there)
            self.assertIn("is there already", err)
            self.assertEqual((self.id_asked, ask.asked), ([], []))
        self.assertFalse(os.path.exists(os.path.join(self.tmp, "nowhere")))
        os.remove(self.out)
        # the file comes between the question and the writing: still not written over
        def racing(prompt):
            if not os.path.lexists(self.out):
                with open(self.out, "w", encoding="ascii") as f:
                    f.write("kept\n")
            return PASSWORD
        code, out, err = self.run_main(self.args(), racing)
        self.assertEqual((code, out), (1, ""))
        with open(self.out, encoding="ascii") as f:
            self.assertEqual(f.read(), "kept\n")

    def test_the_command_without_a_terminal(self):
        # a date a month ahead of today, which the command takes
        until = (datetime.date.today() + datetime.timedelta(days=30)).isoformat()
        p = subprocess.run([sys.executable, "-I", "-B", SCRIPT] + self.args(until),
                           input=ID + "\n" + PASSWORD + "\n" + PASSWORD + "\n",
                           capture_output=True, text=True, timeout=60, start_new_session=True)
        self.assertEqual(p.returncode, 1, p.stderr)
        self.assertEqual(p.stdout, "")
        self.assertIn("terminal", p.stderr)
        self.assertFalse(os.path.lexists(self.out))


if __name__ == "__main__":
    unittest.main()

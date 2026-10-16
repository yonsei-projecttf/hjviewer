# SPDX-License-Identifier: AGPL-3.0-only
"""Offline tests of deploy/guest-conf.py, which writes the file of hjv-api's access password, with
made-up values only:

    python3 -I -B -m unittest discover -s scripts -p 'test_guest_conf.py'

The password is asked through a stand-in for getpass, on a stand-in terminal; the command itself
runs once more as a server would run it, with a standard input that is no terminal.
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
from contextlib import redirect_stderr
from unittest import mock

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SCRIPT = os.path.join(ROOT, "deploy", "guest-conf.py")

_spec = importlib.util.spec_from_file_location("guest_conf", SCRIPT)
gc = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(gc)

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


class GuestConfTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.tmp)
        self.out = os.path.join(self.tmp, "guest.conf")

    def run_main(self, argv, ask, tty=True, out_tty=True, make=gc.random_password):
        stdout, stderr = Terminal(out_tty), io.StringIO()
        with redirect_stderr(stderr):
            code = gc.main(argv, stdin=Terminal(tty), stdout=stdout, ask=ask, now=NOW, make=make)
        return code, stdout.getvalue(), stderr.getvalue()

    def args(self, until="2026-11-30"):
        return ["--until", until, "--out", self.out]

    def lines(self):
        with open(self.out, encoding="ascii") as f:
            return [line for line in f.read().splitlines() if not line.startswith("#")]

    def check_hash(self, line, password):
        scheme, iterations, salt, key = line.removeprefix("HASH=").split("$")
        self.assertEqual((scheme, iterations), ("pbkdf2-sha256", "600000"))
        salt = gc.base64.b64decode(salt, validate=True)
        self.assertEqual(len(salt), 16)
        self.assertEqual(gc.base64.b64decode(key, validate=True),
                         hashlib.pbkdf2_hmac("sha256", password.encode(), salt, 600000, 32))

    def test_the_file(self):
        ask = answers(PASSWORD, PASSWORD)
        code, out, err = self.run_main(self.args(), ask)
        self.assertEqual((code, err), (0, ""))
        self.assertEqual(ask.asked, ["Access password: ", "The same again: "])
        # only the path and UNTIL are printed
        self.assertEqual(out, f"{self.out}\nUNTIL=2026-11-29T15:00:00Z\n")
        self.assertEqual(stat.S_IMODE(os.stat(self.out).st_mode), 0o600)
        lines = self.lines()
        self.assertEqual(len(lines), 2)
        self.check_hash(lines[0], PASSWORD)
        self.assertEqual(lines[1], "UNTIL=2026-11-29T15:00:00Z")
        with open(self.out, encoding="ascii") as f:
            self.assertNotIn(PASSWORD, out + err + f.read())

    def test_a_random_password_is_shown_once(self):
        code, out, err = self.run_main(self.args() + ["--random"], answers(),
                                       make=lambda: "abcde-fghjk-mnpqr-stuvw")
        self.assertEqual((code, err), (0, ""))
        self.assertEqual(out, f"{self.out}\nUNTIL=2026-11-29T15:00:00Z\n"
                              "The access password, shown this once: abcde-fghjk-mnpqr-stuvw\n")
        self.check_hash(self.lines()[0], "abcde-fghjk-mnpqr-stuvw")
        # only to a terminal: never into a pipe or a file
        os.remove(self.out)
        code, out, err = self.run_main(self.args() + ["--random"], answers(), out_tty=False)
        self.assertEqual((code, out), (1, ""))
        self.assertIn("terminal", err)
        self.assertFalse(os.path.lexists(self.out))
        # what it makes: 4 groups of 5 of the alphabet, none alike
        made = {gc.random_password() for _ in range(50)}
        self.assertEqual(len(made), 50)
        for password in made:
            groups = password.split("-")
            self.assertEqual([len(g) for g in groups], [5, 5, 5, 5])
            self.assertTrue(set("".join(groups)) <= set(gc.ALPHABET))
            gc.check_password(password)
        self.assertFalse(set("01ilo") & set(gc.ALPHABET))

    def test_the_hash_is_the_servers(self):
        # server/api/src/guest.rs and tests/guest.rs check the same line with ring
        self.assertEqual(
            gc.hash_of(PASSWORD, bytes(range(16)), 100000),
            "pbkdf2-sha256$100000$AAECAwQFBgcICQoLDA0ODw==$SdScJfWXhGIJ8Nkud3CrZOHHXpS0zmxQkmXuZxddKh4=")
        # and deploy/guest.conf.example shows what the tool writes
        with open(os.path.join(ROOT, "deploy", "guest.conf.example"), encoding="utf-8") as f:
            example = f.read().splitlines()
        self.assertIn(f"#   HASH={gc.hash_of(PASSWORD, bytes(range(16)))}", example)
        self.assertIn(f"#   UNTIL={gc.until_of('2026-11-30', NOW)}", example)
        self.assertFalse([line for line in example if line and not line.startswith("#")])

    def test_until(self):
        self.assertEqual(gc.until_of("2026-11-30", NOW), "2026-11-29T15:00:00Z")
        self.assertEqual(gc.until_of("2026-10-17", NOW), "2026-10-16T15:00:00Z")
        for bad in ("2026-10-16", "2026-02-30", "20261130", "2026-11-30T00:00", ""):
            with self.assertRaises(gc.Refused, msg=bad):
                gc.until_of(bad, NOW)
        self.assertEqual(gc.until_of("2027-01-14", NOW), "2027-01-13T15:00:00Z")
        for far in ("2027-01-15", "9999-12-31"):
            with self.assertRaisesRegex(gc.Refused, "at most 90 days", msg=far):
                gc.until_of(far, NOW)

    def test_passwords(self):
        for typed, why in (((PASSWORD, "correct horse battery stapler"), "differ"),
                           (("too short", "too short"), "12 to 256"),
                           (("x" * 257, "x" * 257), "12 to 256"),
                           (("이용 비밀번호를 길게 적었습니다", "이용 비밀번호를 길게 적었습니다"),
                            "printable ASCII"),
                           ((" " + PASSWORD, " " + PASSWORD), "start or end with a space"),
                           ((PASSWORD + " ", PASSWORD + " "), "start or end with a space")):
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
        self.assertEqual(ask.asked, [])

        def echoing(prompt):
            warnings.warn("Can not control echo on the terminal.", getpass.GetPassWarning)
            return PASSWORD
        code, out, err = self.run_main(self.args(), echoing)
        self.assertEqual((code, out), (1, ""))
        self.assertIn("cannot hide", err)
        self.assertFalse(os.path.lexists(self.out))
        # no option takes the password
        with redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            gc.main(self.args() + ["--password", PASSWORD], stdin=Terminal(), stdout=Terminal(),
                    ask=ask, now=NOW)
        # where Python lacks O_NOFOLLOW (Windows), nothing is asked
        ask = answers(PASSWORD, PASSWORD)
        with mock.patch.object(gc, "os", types.SimpleNamespace(path=os.path)):
            code, out, err = self.run_main(self.args(), ask)
        self.assertEqual((code, out), (1, ""))
        self.assertIn("Linux, macOS or WSL", err)
        self.assertEqual(ask.asked, [])

    def test_never_over_a_file(self):
        with open(self.out, "w", encoding="ascii") as f:
            f.write("kept\n")
        ask = answers(PASSWORD, PASSWORD)
        code, out, err = self.run_main(self.args(), ask)
        self.assertEqual((code, out), (1, ""))
        self.assertIn("is there already", err)
        self.assertEqual(ask.asked, [])
        os.remove(self.out)
        os.symlink(os.path.join(self.tmp, "nowhere"), self.out)
        code, out, err = self.run_main(self.args(), answers(PASSWORD, PASSWORD))
        self.assertEqual((code, out), (1, ""))
        self.assertFalse(os.path.exists(os.path.join(self.tmp, "nowhere")))

    def test_the_command_without_a_terminal(self):
        until = (datetime.date.today() + datetime.timedelta(days=30)).isoformat()
        p = subprocess.run([sys.executable, "-I", "-B", SCRIPT] + self.args(until),
                           input=PASSWORD + "\n" + PASSWORD + "\n",
                           capture_output=True, text=True, timeout=60, start_new_session=True)
        self.assertEqual(p.returncode, 1, p.stderr)
        self.assertEqual(p.stdout, "")
        self.assertIn("terminal", p.stderr)
        self.assertFalse(os.path.lexists(self.out))


if __name__ == "__main__":
    unittest.main()

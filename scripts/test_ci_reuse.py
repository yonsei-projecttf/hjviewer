# SPDX-License-Identifier: AGPL-3.0-only
"""Tests of scripts/ci.sh's lock, on throwaway CI directories (scripts/ci.sh leak runs them):

    python3 -I -B -m unittest discover -s scripts -p 'test_ci_reuse.py'
"""
import os
import re
import shutil
import subprocess
import tempfile
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)


def clean_env(ci, **more):
    """This environment without the variables of a CI run (the leak stage runs inside prepush), with
    <ci> as the CI directory."""
    env = {k: v for k, v in os.environ.items() if not k.startswith(("HJV_", "E2E_"))}
    env.update(HJV_CI_DIR=ci, GIT_CONFIG_NOSYSTEM="1", GIT_AUTHOR_NAME="tester", GIT_AUTHOR_EMAIL="tester@example.org",
               GIT_COMMITTER_NAME="tester", GIT_COMMITTER_EMAIL="tester@example.org")
    env.update(more)
    return env


def bash(script, *args, env, cwd=None, check=True):
    done = subprocess.run(["bash", "-c", script, "bash", *args], env=env, cwd=cwd, capture_output=True, text=True)
    if check and done.returncode != 0:
        raise AssertionError(f"bash failed ({done.returncode}):\n{done.stdout}\n{done.stderr}")
    return done


class LockTest(unittest.TestCase):
    """lock_run, lock_holder and lock_held of scripts/ci.sh on a CI directory of their own"""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="hjv-lock-")
        self.ci = os.path.join(self.tmp, "ci")
        os.makedirs(self.ci)
        self.stub = os.path.join(self.tmp, "stub.sh")
        with open(self.stub, "w") as f:
            f.write('#!/usr/bin/env bash\necho "under the lock of $HJV_CI_LOCK: $*"\n')
        os.chmod(self.stub, 0o755)

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def hold(self, seconds):
        """A process that holds the lock <seconds> and says who it is, as a run of ci.sh does"""
        holder = subprocess.Popen(["flock", os.path.join(self.ci, "ci.lock"), "sh", "-c",
                                   'echo "$$ tree T, commit C, scripts/ci.sh test, since now" > "$1/ci.lock.holder"; '
                                   'sleep "$2"', "sh", self.ci, str(seconds)])
        for _ in range(50):
            if os.path.exists(os.path.join(self.ci, "ci.lock.holder")):
                break
            time.sleep(0.1)
        return holder

    def lock_run(self, wait=None):
        env = clean_env(self.ci, **({"HJV_CI_WAIT": wait} if wait is not None else {}))
        start = time.monotonic()
        done = bash('source "$1"; SELF=$2; lock_run a b', os.path.join(ROOT, "scripts", "ci.sh"), self.stub, env=env,
                    check=False)
        return done, time.monotonic() - start

    def test_a_held_lock_stops_a_run_at_once_or_after_its_wait(self):
        holder = self.hold(1)
        try:
            done, took = self.lock_run()
            self.assertEqual(done.returncode, 1)
            self.assertLess(took, 1.5)
            self.assertRegex(done.stderr, rf"FAIL: another scripts/ci.sh run holds .*/ci.lock: pid \d+, tree T, commit C, "
                                          r"scripts/ci.sh test, since now \(HJV_CI_WAIT=<seconds> waits for it\)")
            done, took = self.lock_run(wait="10")
            self.assertEqual(done.returncode, 0, done.stderr)
            self.assertIn("waiting up to 10 s for", done.stdout)
            self.assertIn(f"under the lock of {self.ci}: a b", done.stdout)
            self.assertGreater(took, 0.3)
        finally:
            holder.wait()
        holder = self.hold(2)
        try:
            done, took = self.lock_run(wait="1")
            self.assertEqual(done.returncode, 1)
            self.assertRegex(done.stderr, r"FAIL: .*/ci.lock is still held after 1 s, by pid \d+, tree T")
            self.assertLess(took, 2.5)
        finally:
            holder.wait()
        done, _ = self.lock_run(wait="x")
        self.assertIn("HJV_CI_WAIT=x is no number of seconds", done.stderr)

    def test_the_run_that_holds_the_lock_says_who_it_is(self):
        env = clean_env(self.ci)
        ci = os.path.join(ROOT, "scripts", "ci.sh")
        out = bash('source "$1"; lock_held prepush --drill; cat "$CI/ci.lock.holder"; lock_holder; lock_held other; '
                   'cat "$CI/ci.lock.holder"', ci, env=env).stdout.splitlines()
        self.assertRegex(out[0], rf"^\d+ tree {re.escape(ROOT)}, commit [0-9a-f]+, scripts/ci.sh prepush --drill, since 20")
        self.assertEqual(out[1], "pid " + out[0].replace(" tree ", ", tree ", 1))
        # the run's later processes are its: they leave the file as it is
        self.assertEqual(out[2], out[0])
        # once its process has ended, a holder that does not say who it is
        self.assertRegex(bash('source "$1"; lock_holder', ci, env=env).stdout,
                         r"^a run that does not say who it is \(no live process in .*ci.lock.holder\)\n$")


if __name__ == "__main__":
    unittest.main()

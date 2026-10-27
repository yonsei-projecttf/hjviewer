# SPDX-License-Identifier: AGPL-3.0-only
"""Tests of scripts/ci.sh's reuse of strict passes (prepush's plan, its inputs, records and the
evidence they take) and of its lock, on throwaway copies of this tree and CI directories
(scripts/ci.sh leak runs them):

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
    """This environment without the variables of a CI run (the leak stage runs inside prepush) and of
    the plan's switches, with <ci> as the CI directory."""
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


class Tree:
    """A throwaway repository with this tree's files (those git tracks or does not ignore, as they are
    now) in one commit, or a copy of another such tree at another path."""

    def __init__(self, tmp, name, like=None):
        self.dir = os.path.join(tmp, name)
        self.env = clean_env(os.path.join(tmp, "ci"))
        if like:
            shutil.copytree(like.dir, self.dir, symlinks=True)
            return
        os.makedirs(self.dir)
        files = subprocess.run(["git", "-C", ROOT, "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
                               capture_output=True, check=True).stdout
        subprocess.run(["tar", "--null", "--ignore-failed-read", "-T", "-", "-cf", os.path.join(tmp, name + ".tar")],
                       input=files, cwd=ROOT, check=True, capture_output=True)
        subprocess.run(["tar", "-xf", os.path.join(tmp, name + ".tar"), "-C", self.dir], check=True)
        self.git("init", "-q", "-b", "main")
        self.commit("the tree")

    def git(self, *args):
        return subprocess.run(["git", *args], cwd=self.dir, env=self.env, check=True, capture_output=True,
                              text=True).stdout

    def commit(self, message):
        self.git("add", "-A")
        self.git("commit", "-q", "-m", message)
        return self.git("rev-parse", "HEAD").strip()

    def write(self, rel, text, mode="w"):
        with open(os.path.join(self.dir, rel), mode, encoding="utf-8") as f:
            f.write(text)

    def ci(self, *args, env=None, check=True):
        done = subprocess.run(["bash", os.path.join(self.dir, "scripts", "ci.sh"), *args], env=env or self.env,
                              cwd=self.dir, capture_output=True, text=True)
        if check and done.returncode != 0:
            raise AssertionError(f"scripts/ci.sh {' '.join(args)} ({done.returncode}):\n{done.stdout}\n{done.stderr}")
        return done

    def inputs(self, *units, env=None):
        """{unit: its inputs} by scripts/ci.sh inputs."""
        out = tempfile.mkdtemp(dir=os.path.dirname(self.dir))
        self.ci("inputs", out, *units, env=env)
        found = {}
        for unit in units:
            with open(os.path.join(out, unit + ".txt"), encoding="utf-8") as f:
                found[unit] = f.read()
        return found

    def plan(self, *args, env=None):
        """{unit: (run|reuse, the rest of its line)} of prepush --plan."""
        lines = self.ci("prepush", "--plan", *args, env=env).stdout.splitlines()
        return {m.group(2): (m.group(1), m.group(3)) for m in
                (re.match(r"^  (run|reuse) +(\S+) +(.*)$", line) for line in lines) if m}

    def source(self, script, *args, env=None, check=True):
        """<script> in a bash that sourced this tree's scripts/ci.sh, in this tree."""
        return bash('set -euo pipefail\nsource scripts/ci.sh\nreuse_rules\n' + script, *args, env=env or self.env,
                    cwd=self.dir, check=check)


class ReuseTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.mkdtemp(prefix="hjv-reuse-")
        cls.tree = Tree(cls.tmp, "tree")

    @classmethod
    def tearDownClass(cls):
        shutil.rmtree(cls.tmp, ignore_errors=True)

    def copy(self, name):
        tree = Tree(self.tmp, name, like=self.tree)
        tree.env = clean_env(tempfile.mkdtemp(dir=self.tmp))
        return tree

    def test_another_tree_with_the_same_files_has_the_same_inputs(self):
        other = self.copy("other")
        a = self.tree.inputs("server:release", "e2e", "client")
        b = other.inputs("server:release", "e2e", "client")
        self.assertEqual(a, b)
        self.assertEqual(self.tree.inputs("server:release"), {"server:release": a["server:release"]})
        self.assertNotIn(self.tree.dir, a["e2e"])

    def test_a_file_counts_for_the_units_that_read_it(self):
        tree = self.copy("files")
        before = tree.inputs("server:release", "server:hbbs", "client", "e2e")
        tree.write("tools/hjv-release/src/lib.rs", "// an edit\n", "a")
        tree.write("docs/privacy.md", "an edit\n", "a")
        tree.commit("an edit")
        after = tree.inputs("server:release", "server:hbbs", "client", "e2e")
        self.assertNotEqual(before["server:release"], after["server:release"])
        self.assertNotEqual(before["client"], after["client"])
        self.assertNotEqual(before["e2e"], after["e2e"])
        self.assertEqual(before["server:hbbs"], after["server:hbbs"])
        self.assertIn("tools/hjv-release/src/lib.rs", after["server:release"])
        for unit in after:
            self.assertNotIn("docs/privacy.md", after[unit], unit)
        # a file nobody thought of counts for every unit
        tree.write("new-top-file.txt", "a\n")
        tree.commit("a new file")
        newest = tree.inputs("server:release", "server:hbbs")
        self.assertIn(" new-top-file.txt\n", newest["server:release"])
        self.assertIn(" new-top-file.txt\n", newest["server:hbbs"])

    def test_the_tools_the_environment_and_the_test_keys_count(self):
        tree = self.copy("outside")
        before = tree.inputs("server:release", "e2e", "client")
        stubs = tempfile.mkdtemp(dir=self.tmp)
        with open(os.path.join(stubs, "iptables"), "w") as f:
            f.write("#!/bin/sh\necho 'iptables v0.0.0 (a stand-in)'\n")
        os.chmod(os.path.join(stubs, "iptables"), 0o755)
        env = dict(tree.env, PATH=stubs + ":" + tree.env["PATH"])
        tools = tree.inputs("server:release", "e2e", env=env)
        self.assertEqual(before["server:release"], tools["server:release"])
        self.assertNotEqual(before["e2e"], tools["e2e"])
        self.assertIn("tool iptables v0.0.0 (a stand-in)\n", tools["e2e"])
        moved = tree.inputs("server:release", env=dict(tree.env, HJV_RELAY_NETNS="", HJV_CI_FRESH="1"))
        self.assertNotEqual(before["server:release"], moved["server:release"])
        self.assertIn("env HJV_RELAY_NETNS=''\n", moved["server:release"])
        self.assertNotIn("HJV_CI_FRESH", moved["server:release"])
        # a 32-byte value in a file that the client stage does not otherwise read: its key test does
        tree.write("docs/privacy.md", "\n" + "ab" * 32 + "\n", "a")
        tree.commit("a value")
        after = tree.inputs("server:release", "client")
        self.assertEqual(before["server:release"], after["server:release"])
        self.assertNotEqual(before["client"], after["client"])
        self.assertEqual([line for line in before["client"].splitlines() if not line.startswith("values ")],
                         [line for line in after["client"].splitlines() if not line.startswith("values ")])

    def test_an_edit_of_a_units_code_counts_for_it_alone(self):
        # Every function of scripts/ci.sh, edited by one statement more: server:release's key changes
        # exactly for the functions of its code. The floors: each changes the units that name it.
        # (each function in a subshell of its own, side by side: the edit stays there)
        out = self.tree.source(r'''
key() { reuse_code "$@" | sha256sum | cut -d' ' -f1; }
read -ra release <<< "${REUSE_CODE[server:release]}"
base=$(key "${release[@]}" finish)
closure=" $(reuse_closure "${release[@]}" finish | sed -n 's/^f //p' | paste -sd' ') "
for f in $(declare -F | sed 's/^declare -f //'); do
  (
    eval "$(declare -f "$f" | sed '2a\    : an edit;')"
    now=$(key "${release[@]}" finish)
    if [[ $closure == *" $f "* ]]; then
      if [ "$now" != "$base" ]; then echo "in $f"; else echo "UNCHANGED $f"; fi
    elif [ "$now" = "$base" ]; then echo "out $f"; else echo "CHANGED $f"; fi
  ) &
  while [ "$(jobs -pr | wc -l)" -ge "$(nproc)" ]; do wait -n; done
done
declare -A keys=()
for u in server:release server:hbbs client deploy e2e wol drill:full; do
  read -ra fns <<< "${REUSE_CODE[$u]}"
  keys[$u]=$(key "${fns[@]}" finish)
done
for v in "VECTORS_LEAST client" "E2E_LEAST_STRICT e2e" "E2E_LEAST_OPEN e2e" "WOL_LEAST wol" "UNIT_LEAST e2e wol" \
  "RELEASE_INPUTS server:release client e2e" "HBBS_INPUTS server:hbbs deploy e2e wol"; do
  read -r name users <<< "$v"
  for u in "${!keys[@]}"; do
    (
      read -ra fns <<< "${REUSE_CODE[$u]}"
      eval "$(declare -p "$name" | sed 's/="/="1/')"
      now=$(key "${fns[@]}" finish)
      if [[ " $users " == *" $u "* ]]; then
        if [ "$now" != "${keys[$u]}" ]; then echo "uses $name $u"; else echo "NOT USED $name $u"; fi
      elif [ "$now" = "${keys[$u]}" ]; then echo "leaves $name $u"; else echo "MOVED $name $u"; fi
    ) &
    while [ "$(jobs -pr | wc -l)" -ge "$(nproc)" ]; do wait -n; done
  done
done
wait
echo "functions $(declare -F | wc -l)"
''').stdout
        self.assertNotRegex(out, r"(?m)^(UNCHANGED|CHANGED|NOT USED|MOVED) ")
        self.assertRegex(out, r"(?m)^in server_release$")
        self.assertRegex(out, r"(?m)^in finish$")
        self.assertRegex(out, r"(?m)^out deploy_stage$")
        self.assertRegex(out, r"(?m)^out prepush_run$")
        self.assertEqual(len(re.findall(r"(?m)^(in|out) ", out)), int(re.search(r"(?m)^functions (\d+)$", out).group(1)))

    def test_the_top_level_code_counts_for_every_unit_and_comments_and_values_do_not(self):
        tree = self.copy("top")
        before = tree.inputs("server:release", "wol")
        path = os.path.join(tree.dir, "scripts", "ci.sh")
        with open(path, encoding="utf-8") as f:
            text = f.read()
        # a comment and a value that no unit's code names (prepush's own)
        edited = text.replace("\nset -euo pipefail\n", "\n# a comment\nset -euo pipefail\n", 1)
        edited = edited.replace('PREPUSH_STAGES="leak ', 'PREPUSH_STAGES="leak leak ', 1)
        with open(path, "w", encoding="utf-8") as f:
            f.write(edited)
        tree.commit("a comment and prepush's stages")
        self.assertEqual(before, tree.inputs("server:release", "wol"))
        with open(path, "w", encoding="utf-8") as f:
            f.write(edited.replace("\nset -euo pipefail\n", "\nset -euo pipefail\n: an edit\n", 1))
        tree.commit("top-level code")
        after = tree.inputs("server:release", "wol")
        for unit in after:
            self.assertNotEqual(before[unit], after[unit], unit)

    def pass_log(self, tree, unit, name=None, status=0, skipped=0, how="prepush, HJV_CI_STRICT=1", lines=0, more=""):
        """A log of <unit> as prepush keeps it, in the tree's CI directory."""
        head = tree.git("rev-parse", "HEAD").strip()
        top = tree.git("rev-parse", "HEAD^{tree}").strip()
        path = os.path.join(tree.env["HJV_CI_DIR"], (name or unit.replace(":", "-")) + ".log")
        with open(path, "w", encoding="utf-8") as f:
            f.write(f"LOG scripts/ci.sh {unit} ({how}, CI directory {tree.env['HJV_CI_DIR']})\n"
                    f"hjviewer {head} tree {top} status-lines {lines} ({tree.dir})\n"
                    "TOOLCHAIN rustc / cargo\nSTART 2026-10-27 10:00:00 +0900\n" + more +
                    f"\n== CI passed: {unit} ({skipped} skipped)\nEXIT {status} 2026-10-27 10:01:00 +0900\n")
        return path

    def record(self, tree, unit, log, env=None):
        inputs = tempfile.mkdtemp(dir=self.tmp)
        tree.ci("inputs", inputs, unit, env=env)
        out = tree.source('reuse_record "$1" "$2" "$3" "$(git rev-parse HEAD)"', unit, log,
                          os.path.join(inputs, unit + ".txt"), env=env).stdout
        return out

    def test_only_a_strict_clean_pass_without_skips_is_recorded(self):
        tree = self.copy("records")
        passes = os.path.join(tree.env["HJV_CI_DIR"], "passes", "server:release")
        for kind, log in (("skipped", self.pass_log(tree, "server:release", "a", skipped=1)),
                          ("a skipped line", self.pass_log(tree, "server:release", "b", more="skipped: x (y)\n")),
                          ("partial", self.pass_log(tree, "server:release", "c", more="PARTIAL (access)\n")),
                          ("not strict", self.pass_log(tree, "server:release", "d", how="on its own, HJV_CI_STRICT=0")),
                          ("a changed tree", self.pass_log(tree, "server:release", "e", lines=2)),
                          ("exit 1", self.pass_log(tree, "server:release", "f", status=1))):
            self.assertEqual(self.record(tree, "server:release", log), "", kind)
            self.assertFalse(os.path.isdir(passes) and os.listdir(passes), kind)
        # a part of the server stage, in the log of the stage whose other part failed: with its own
        # exit status
        log = self.pass_log(tree, "server", "g", status=1, more="== CI passed: server:release (0 skipped)\n")
        self.assertEqual(self.record(tree, "server:release", log), "")
        log = self.pass_log(tree, "server", "g", status=1, more="== CI passed: server:release (0 skipped)\n"
                            "server: release: exit 0, 8 s, 0 skipped line(s), 0 retried test run(s)\n")
        self.assertIn("recorded the pass of server:release", self.record(tree, "server:release", log))
        self.assertEqual(len([f for f in os.listdir(passes) if f.endswith(".pass")]), 1)
        # a tree with changes since
        tree.write("README.md", "an edit\n", "a")
        self.assertEqual(self.record(tree, "drill:full", self.pass_log(tree, "drill:full")), "")
        tree.git("checkout", "README.md")
        # a variable that names files outside the tree
        self.assertEqual(self.record(tree, "drill:full", self.pass_log(tree, "drill:full"),
                                     env=dict(tree.env, HJV_UPDATE_VECTORS="/elsewhere")), "")

    def test_the_plan_reuses_a_recent_pass_with_the_same_inputs(self):
        tree = self.copy("plan")
        self.assertEqual(tree.plan("server:release")["server:release"], ("run", "no pass of it recorded"))
        head = tree.git("rev-parse", "--short=7", "HEAD").strip()
        log = self.pass_log(tree, "server:release")
        self.record(tree, "server:release", log)
        how, said = tree.plan("server:release")["server:release"]
        self.assertEqual(how, "reuse")
        self.assertIn(f"its strict pass of {head} at ", said)
        self.assertIn(f"log {log}", said)
        when = said.split(" at ")[1][:16]
        # the switches and the age limit
        for name, value, why in (("HJV_CI_FRESH", "1", "HJV_CI_FRESH=1"),
                                 ("HJV_CI_DEPLOY_ALL", "1", "HJV_CI_DEPLOY_ALL=1, a fresh deploy: no unit is reused"),
                                 ("HJV_API_ENV_FILE", "/elsewhere", "HJV_API_ENV_FILE names files outside the tree")):
            self.assertEqual(tree.plan("server:release", env=dict(tree.env, **{name: value}))["server:release"],
                             ("run", why))
        record = [os.path.join(dp, f) for dp, _, fs in os.walk(os.path.join(tree.env["HJV_CI_DIR"], "passes"))
                  for f in fs if f.endswith(".pass")][0]
        with open(record, encoding="utf-8") as f:
            text = f.read()
        with open(record, "w", encoding="utf-8") as f:
            f.write(re.sub(r"(?m)^time \d+$", f"time {int(time.time()) - 25 * 3600}", text))
        how, said = tree.plan("server:release")["server:release"]
        self.assertEqual(how, "run")
        self.assertRegex(said, r"^its pass with these inputs, of .*, is 25 h old \(HJV_CI_REUSE_HOURS=24\)$")
        self.assertEqual(tree.plan("server:release", env=dict(tree.env, HJV_CI_REUSE_HOURS="48"))["server:release"][0],
                         "reuse")
        with open(record, "w", encoding="utf-8") as f:
            f.write(text)
        # its log is its evidence
        os.rename(log, log + ".gone")
        self.assertEqual(tree.plan("server:release")["server:release"],
                         ("run", f"its pass with these inputs has no log that shows it any more ({log})"))
        os.rename(log + ".gone", log)
        # a change it reads, and which: the plan names it and the pass it compares with
        tree.write("tools/hjv-release/src/lib.rs", "// an edit\n", "a")
        tree.commit("an edit")
        self.assertEqual(tree.plan("server:release")["server:release"],
                         ("run", f"tools/hjv-release/src/lib.rs changed since its pass of {head} ({when})"))

    def test_a_unit_that_runs_has_its_binaries_built_from_this_trees_inputs(self):
        tree = self.copy("producer")
        ci = tree.env["HJV_CI_DIR"]
        # e2e runs (no pass of it) and takes hbbs and hbbr of server:hbbs, which are not built (as
        # deploy and wol do, and deploy hjv-api of server:api)
        plan = tree.plan("e2e")
        self.assertEqual(plan["e2e"][0], "run")
        self.assertEqual(plan["server:hbbs"], ("run", "e2e runs and needs this tree's hbbs: it is not built"))
        # built from these inputs, with a pass of server:hbbs to reuse: reused
        os.makedirs(os.path.join(ci, "target", "release"))
        tree.source('''for t in hbbs hbbr; do printf '#!/bin/sh\\n' > "$CI/target/release/$t"; chmod 755 "$CI/target/release/$t"
          stamp_binary "$CI/target/release/$t" "${HBBS_INPUTS[@]}" > /dev/null; done''')
        self.record(tree, "server:hbbs", self.pass_log(tree, "server:hbbs"))
        plan = tree.plan("server:hbbs", "e2e")
        self.assertEqual(plan["server:hbbs"][0], "reuse")
        self.assertEqual(plan["e2e"][0], "run")
        # another tree's hbbs since (other server/patches): server:hbbs runs again, though its pass
        # could be reused
        tree.source('''sed -i 's/ inputs [0-9a-f]* / inputs 0000 /' "$CI/target/release/hbbs.stamp"''')
        plan = tree.plan("server:hbbs", "e2e")
        self.assertEqual(plan["server:hbbs"], ("run", "e2e runs and needs this tree's hbbs: this tree's "
                                               "server/patches scripts/upstream.env scripts/fetch-upstream.sh or the "
                                               "toolchain are not what it was built from"))
        # a unit that is reused takes nothing
        self.assertNotIn("server:hbbs", tree.plan("server:release"))


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

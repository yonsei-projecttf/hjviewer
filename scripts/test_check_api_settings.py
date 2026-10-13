# SPDX-License-Identifier: AGPL-3.0-only
"""Offline tests of scripts/check-api-settings.py (scripts/ci.sh deploy), with stand-ins for the
hjv-api binary that know some setting names:

    python3 -I -B -m unittest discover -s scripts -p 'test_check_api_settings.py'
"""
import importlib.util
import os
import shutil
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
SCRIPT = os.path.join(HERE, "check-api-settings.py")
PACKAGED_EXAMPLE = os.path.join(ROOT, "server", "api", "hjviewer-api.env.example")

_spec = importlib.util.spec_from_file_location("check_api_settings", SCRIPT)
cas = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(cas)

# the names of the settings hjv-api had before the trust lists, and those the trust lists add
BEFORE = ("HJV_API_BIND_OS", "HJV_API_SECOND_FACTOR")
TRUST = ("HJV_API_TRUST_REFRESH", "HJV_API_TRUST_TTL")
EXAMPLE = """# settings
#HJV_API_BIND_OS=windows
#HJV_API_SECOND_FACTOR=off
"""


class SettingsTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.tmp)

    def file(self, name, text):
        path = os.path.join(self.tmp, name)
        with open(path, "w", encoding="utf-8") as f:
            f.write(text)
        return path

    def binary(self, *names):
        # string literals packed together, as rustc lays them out
        return self.file("hjv-api", "\0ELF" + "".join(names) + "\0")

    def run_check(self, env, example=EXAMPLE, names=BEFORE):
        example_path = example if os.path.isabs(example) else self.file("example", example)
        env_path = self.file("env", env)
        results = cas.check(self.binary(*names), example_path, env_path)
        return {m.split(" ", 1)[0].split("=", 1)[0].rstrip(":"): (level, m) for level, m in results}

    def assertLevel(self, results, name, level):
        self.assertEqual(results[name][0], level, results[name][1])

    def test_the_packaged_example(self):
        with open(PACKAGED_EXAMPLE, encoding="utf-8") as f:
            r = self.run_check(f.read(), PACKAGED_EXAMPLE)
        self.assertLevel(r, "HJV_API_BIND_OS", "ok")
        self.assertLevel(r, "HJV_API_SECOND_FACTOR", "ok")
        self.assertLevel(r, "HJV_API_TRUST_REFRESH", "skipped")

    def test_defaults_and_values(self):
        r = self.run_check("HJV_API_SSO_CLIENT_ID=x\n")
        self.assertIn("documented default", r["HJV_API_BIND_OS"][1])
        self.assertLevel(r, "HJV_API_BIND_OS", "ok")
        for env in ("HJV_API_BIND_OS=windows,linux\n", "HJV_API_BIND_OS=\n#x\nHJV_API_BIND_OS=linux\n",
                    "HJV_API_SECOND_FACTOR=totp\n"):
            r = self.run_check(env)
            self.assertTrue(any(level == "fail" for level, _ in r.values()), env)
        # what hjv-api reads as the same: case, blanks and empty entries; an empty value is the default
        for env in ("HJV_API_BIND_OS= Windows , \n", "HJV_API_BIND_OS=\n", "HJV_API_BIND_OS=linux\nHJV_API_BIND_OS=windows\n"):
            self.assertLevel(self.run_check(env), "HJV_API_BIND_OS", "ok")

    def test_lines_as_systemd_reads_them(self):
        """Leading blanks, blanks around '=', quotes: systemd applies these lines."""
        for env in ("  HJV_API_BIND_OS=windows,linux\n", "\tHJV_API_BIND_OS = linux\n",
                    'HJV_API_BIND_OS="windows,linux"\n', "HJV_API_SECOND_FACTOR='totp'\n",
                    "HJV_API_SECOND_FACTOR=\"to\"'tp'\n"):
            r = self.run_check(env)
            self.assertTrue(any(level == "fail" for level, _ in r.values()), env)
        # comments, also indented or with ';', are not applied
        for env in ("  # HJV_API_BIND_OS=linux\n", "; HJV_API_BIND_OS=linux\n", "export HJV_API_BIND_OS=linux\n"):
            self.assertLevel(self.run_check(env), "HJV_API_BIND_OS", "ok")
        self.assertEqual(cas.unquote('"a b"'), "a b")
        self.assertEqual(cas.unquote(r'"a\"b"'), 'a"b')

    def test_the_trust_settings_once_hjv_api_knows_them(self):
        names = BEFORE + TRUST
        # no documented default and none set: what hjv-api uses is unknown
        r = self.run_check("", names=names)
        self.assertLevel(r, "HJV_API_TRUST_REFRESH", "fail")
        self.assertIn("documents no default", r["HJV_API_TRUST_REFRESH"][1])
        # a default documented with a blank after '#'
        example = EXAMPLE + "# HJV_API_TRUST_REFRESH=120\n#HJV_API_TRUST_TTL=86400\n"
        r = self.run_check("", example, names)
        self.assertLevel(r, "HJV_API_TRUST_REFRESH", "ok")
        self.assertLevel(r, "HJV_API_TRUST_TTL", "ok")
        for env, name in (("HJV_API_TRUST_REFRESH=30\n", "HJV_API_TRUST_REFRESH"),
                          ("HJV_API_TRUST_REFRESH=301\n", "HJV_API_TRUST_REFRESH"),
                          ("HJV_API_TRUST_REFRESH=2m\n", "HJV_API_TRUST_REFRESH"),
                          ("HJV_API_TRUST_TTL=259201\n", "HJV_API_TRUST_TTL"),
                          ("HJV_API_TRUST_TTL=0\n", "HJV_API_TRUST_TTL")):
            self.assertLevel(self.run_check(env, example, names), name, "fail")
        for env in ("HJV_API_TRUST_REFRESH=60\nHJV_API_TRUST_TTL=259200\n", "HJV_API_TRUST_REFRESH=300\n"):
            r = self.run_check(env, example, names)
            self.assertLevel(r, "HJV_API_TRUST_REFRESH", "ok")
            self.assertLevel(r, "HJV_API_TRUST_TTL", "ok")

    def test_the_command(self):
        example = self.file("example", EXAMPLE)
        binary = self.binary(*BEFORE)
        run = lambda env: subprocess.run(["python3", "-I", SCRIPT, "--binary", binary, "--example", example,
                                          self.file("env", env)], capture_output=True, text=True)
        done = run("HJV_API_BIND_OS=windows\n")
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertIn("HJV_API_BIND_OS=windows (", done.stdout)
        self.assertIn("skipped: HJV_API_TRUST_TTL", done.stdout)
        done = run("  HJV_API_BIND_OS=windows,linux\n")
        self.assertEqual(done.returncode, 1)
        self.assertIn("FAIL: HJV_API_BIND_OS is 'windows,linux'", done.stdout)


if __name__ == "__main__":
    unittest.main()

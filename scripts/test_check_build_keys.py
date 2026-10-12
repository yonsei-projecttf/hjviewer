# SPDX-License-Identifier: AGPL-3.0-only
"""Offline tests of scripts/check-build-keys.py and of the client-build.yml steps that run it
(scripts/ci.sh client):

    python3 -I -B -m unittest discover -s scripts -p 'test_check_build_keys.py'
"""
import base64
import contextlib
import importlib.util
import io
import os
import shutil
import subprocess
import tempfile
import unittest
from unittest import mock

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
SCRIPT = os.path.join(HERE, "check-build-keys.py")
WORKFLOW = os.path.join(ROOT, ".github", "workflows", "client-build.yml")

_spec = importlib.util.spec_from_file_location("check_build_keys", SCRIPT)
cbk = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(cbk)

KEY_VARS = ("HJV_SERVER_KEY", "HJV_LOGIN_SEAL_PUBKEY", "HJV_TRUST_PUBKEYS", "HJV_UPDATE_PUBKEY",
            "HJV_COMMAND_PUBKEY", "CALLED", "CHECK_ONLY", "GITHUB_ACTIONS", "GITHUB_STEP_SUMMARY")
# server/api/src/seal.rs: the sealed-login test vector's key pair (VECTOR_PK, VECTOR_SK)
SEAL_VECTOR_PK = "SjgH0GTQdxgcwHCYnnaJHSDcpVWVSNwsd8GlAnOIKzg="
SEAL_VECTOR_SK = "d3iPGgzqABomMdrl0F29BiAI1bMPULnim+sqeCIokAQ="


def b64(raw):
    return base64.b64encode(raw).decode()


def ed(seed):
    return b64(cbk.ed25519_public(seed))


def x(secret):
    return b64(cbk.x25519_public(secret))


def fresh():
    return os.urandom(32)


def stray_bits(text):
    """The same 32 bytes in base64 whose last character has a low bit set, which Python's decoder
    takes and Rust's base64 crate refuses."""
    alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
    return text[:42] + alphabet[alphabet.index(text[42]) | 1] + "="


def account_keys():
    """Well-formed keys nobody published."""
    return {
        "HJV_SERVER_KEY": ed(fresh()),
        "HJV_LOGIN_SEAL_PUBKEY": x(fresh()),
        "HJV_TRUST_PUBKEYS": f"1:{ed(fresh())},2:{ed(fresh())}",
    }


class Run:
    """check-build-keys.py's main() in this process, with a version file and an environment."""

    def __init__(self, version="1.6.0", argv=(), **env):
        self.tmp = tempfile.mkdtemp()
        self.version_file = os.path.join(self.tmp, "version.env")
        with open(self.version_file, "w") as f:
            f.write(f"# comment\nHJV_VERSION={version}\n")
        self.summary_file = os.path.join(self.tmp, "summary.md")
        env = {k: v for k, v in env.items() if v is not None}
        env.setdefault("GITHUB_STEP_SUMMARY", self.summary_file)
        out = io.StringIO()
        clean = {k: v for k, v in os.environ.items() if k not in KEY_VARS}
        with mock.patch.dict(os.environ, {**clean, **env}, clear=True), contextlib.redirect_stdout(out):
            self.code = cbk.main(list(argv) or ["--version-file", self.version_file])
        self.out = out.getvalue()
        try:
            with open(self.summary_file) as f:
                self.summary = f.read()
        except FileNotFoundError:
            self.summary = ""
        shutil.rmtree(self.tmp)

    def errors(self):
        return [line for line in self.out.splitlines() if line.startswith("error: ")]

    def warnings(self):
        return [line for line in self.out.splitlines() if line.startswith("warning: ")]


class CurveTest(unittest.TestCase):
    def test_rfc_examples(self):
        self.assertEqual(cbk.ed25519_public(bytes.fromhex(cbk.RFC_SECRETS[0])).hex(),
                         "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a")
        self.assertEqual(cbk.x25519_public(bytes.fromhex(cbk.RFC_SECRETS[3])).hex(),
                         "8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a")

    def test_relay_protocol_vectors(self):
        # relay/PROTOCOL.md section 13
        self.assertEqual(ed(bytes(range(32))), "A6EHv/POEL4dcN0Y50vAmWfk1jCbpQ1fHdyGZBJVMbg=")
        self.assertEqual(ed(bytes(range(32, 64))), "Kay64UG8yvCyLhqU000LxzYeUm0L/hLIl5S8kyKWbdc=")

    def test_against_cryptography(self):
        try:
            from cryptography.hazmat.primitives import serialization
            from cryptography.hazmat.primitives.asymmetric import ed25519, x25519
        except ImportError:
            self.skipTest("the cryptography package is not installed")
        raw = (serialization.Encoding.Raw, serialization.PublicFormat.Raw)
        for _ in range(20):
            seed = fresh()
            self.assertEqual(cbk.ed25519_public(seed),
                             ed25519.Ed25519PrivateKey.from_private_bytes(seed).public_key().public_bytes(*raw))
            self.assertEqual(cbk.x25519_public(seed),
                             x25519.X25519PrivateKey.from_private_bytes(seed).public_key().public_bytes(*raw))

    def test_small_order(self):
        for h in ("0100000000000000000000000000000000000000000000000000000000000000",
                  "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
                  "26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc05"):
            self.assertTrue(cbk.ed25519_weak(cbk._decode(bytes.fromhex(h))), h)
        self.assertFalse(cbk.ed25519_weak(cbk._decode(cbk.ed25519_public(fresh()))))
        for u in (0, 1, cbk.P - 1,
                  325606250916557431795983626356110631294008115727848805560023387167927233504):
            self.assertTrue(cbk.x25519_weak(u), u)
        self.assertFalse(cbk.x25519_weak(int.from_bytes(cbk.x25519_public(fresh()), "little")))


class CalledReleaseTest(unittest.TestCase):
    """A run the private build repository started, client/version.env 1.6.0 or later."""

    def test_well_formed_keys_pass_and_fingerprints_go_to_the_summary(self):
        keys = account_keys()
        r = Run(CALLED="main", **keys)
        self.assertEqual(r.code, 0, r.out)
        self.assertEqual(r.errors(), [])
        seal = base64.b64decode(keys["HJV_LOGIN_SEAL_PUBKEY"])
        self.assertIn(cbk.fingerprint(seal), r.summary)
        self.assertIn(f"| {cbk.fingerprint(seal)[:16]} |", r.summary)  # hjv-api keygen seal's key ID
        for entry in keys["HJV_TRUST_PUBKEYS"].split(","):
            self.assertIn(cbk.fingerprint(base64.b64decode(entry.split(":")[1])), r.summary)
        self.assertIn(cbk.fingerprint(base64.b64decode(keys["HJV_SERVER_KEY"])), r.summary)

    def test_a_missing_account_key_fails(self):
        for name in ("HJV_LOGIN_SEAL_PUBKEY", "HJV_TRUST_PUBKEYS"):
            keys = account_keys()
            del keys[name]
            r = Run(CALLED="main", **keys)
            self.assertEqual(r.code, 1, name)
            self.assertTrue(any(name in e and "did not arrive" in e for e in r.errors()), r.out)

    def test_later_versions_need_them_too(self):
        r = Run(version="1.10.0", CALLED="main")
        self.assertEqual(r.code, 1)

    def test_the_update_key_is_optional_but_checked(self):
        keys = account_keys()
        self.assertEqual(Run(CALLED="main", **keys).code, 0)
        self.assertEqual(Run(CALLED="main", HJV_UPDATE_PUBKEY=f"1:{ed(fresh())}", **keys).code, 0)
        self.assertEqual(Run(CALLED="main", HJV_UPDATE_PUBKEY=f"1:{ed(bytes([3]) * 32)}", **keys).code, 1)
        three = ",".join(f"{i}:{ed(fresh())}" for i in (1, 2, 3))
        self.assertEqual(Run(CALLED="main", HJV_UPDATE_PUBKEY=three, **keys).code, 1)

    def test_public_test_keys_fail(self):
        cases = {
            "seed [1; 32]": ed(bytes([1]) * 32),
            "seed [0; 32]": ed(bytes(32)),
            "seed 00 01 .. 1f": ed(bytes(range(32))),
            "relay command key": "Kay64UG8yvCyLhqU000LxzYeUm0L/hLIl5S8kyKWbdc=",
            "RFC 8032 TEST 1": ed(bytes.fromhex(cbk.RFC_SECRETS[0])),
        }
        for what, key in cases.items():
            keys = account_keys()
            keys["HJV_TRUST_PUBKEYS"] = f"1:{ed(fresh())},2:{key}"
            r = Run(CALLED="main", **keys)
            self.assertEqual(r.code, 1, what)
            self.assertTrue(any("public test key" in e for e in r.errors()), what)
        seal_cases = {
            "X25519 [1; 32]": x(bytes([1]) * 32),
            "X25519 [0xe2; 32]": x(bytes([0xE2]) * 32),
            "RFC 7748 Alice": x(bytes.fromhex(cbk.RFC_SECRETS[3])),
            # server/api/src/seal.rs VECTOR_PK, whose secret key VECTOR_SK is published next to it;
            # made with crypto_box_seed_keypair, so no seed pattern covers it
            "the sealed-login test vector": SEAL_VECTOR_PK,
        }
        for what, key in seal_cases.items():
            keys = account_keys()
            keys["HJV_LOGIN_SEAL_PUBKEY"] = key
            r = Run(CALLED="main", **keys)
            self.assertEqual(r.code, 1, what)
            self.assertTrue(any("public test key" in e for e in r.errors()), what)

    def test_the_test_key_file_counts(self):
        key = ed(fresh())
        tmp = tempfile.mkdtemp()
        try:
            path = os.path.join(tmp, "keys.txt")
            with open(path, "w", encoding="utf-8-sig") as f:  # a BOM, as Windows editors write
                f.write(f"# a fixture's key\n{key}  # trailing comment\n\n")
            with mock.patch.object(cbk, "TEST_KEYS_FILE", path), mock.patch.object(cbk, "_test_keys", {}):
                keys = account_keys()
                keys["HJV_TRUST_PUBKEYS"] = f"1:{key},2:{ed(fresh())}"
                r = Run(CALLED="main", **keys)
                self.assertEqual(r.code, 1)
                self.assertTrue(any("listed in" in e and "keys.txt:2" in e for e in r.errors()), r.out)
        finally:
            shutil.rmtree(tmp)

    def test_the_test_key_file_is_needed(self):
        """Without its list the check would pass the listed keys: it stops instead."""
        tmp = tempfile.mkdtemp()
        try:
            with mock.patch.object(cbk, "TEST_KEYS_FILE", os.path.join(tmp, "missing.txt")), \
                    mock.patch.object(cbk, "_test_keys", {}):
                with self.assertRaises(SystemExit) as stop:
                    Run(CALLED="main", **account_keys())
                self.assertIn("missing", str(stop.exception))
                with self.assertRaises(SystemExit):
                    Run(CALLED="", **account_keys())
        finally:
            shutil.rmtree(tmp)

    def test_malformed_values_fail(self):
        good = [ed(fresh()) for _ in range(3)]
        trust_cases = {
            "one key": f"1:{good[0]}",
            "three keys": f"1:{good[0]},2:{good[1]},3:{good[2]}",
            "same key id": f"1:{good[0]},1:{good[1]}",
            "same key": f"1:{good[0]},2:{good[0]}",
            "no key id": f"{good[0]},2:{good[1]}",
            "key id 0": f"0:{good[0]},2:{good[1]}",
            "key id 65536": f"65536:{good[0]},2:{good[1]}",
            "key id 01": f"01:{good[0]},2:{good[1]}",
            "empty entry": f"1:{good[0]},,2:{good[1]}",
            "not base64": f"1:{good[0][:-2]}!=,2:{good[1]}",
            "31 bytes": f"1:{b64(bytes(31))},2:{good[1]}",
            "no padding": f"1:{good[0].rstrip('=')},2:{good[1]}",
            "url-safe alphabet": "1:" + good[0].replace("+", "-").replace("/", "_") + f",2:{good[1]}",
            "weak (identity)": f"1:{b64(bytes([1]) + bytes(31))},2:{good[1]}",
            "not on the curve": f"1:{b64(bytes([2]) + bytes(31))},2:{good[1]}",
            "non-canonical": f"1:{b64((cbk.P + 1).to_bytes(32, 'little'))},2:{good[1]}",
            "base64 with stray bits": f"1:{stray_bits(good[0])},2:{good[1]}",
            "a space after the comma": f"1:{good[0]}, 2:{good[1]}",
            "a line break at the end": f"1:{good[0]},2:{good[1]}\n",
        }
        for what, value in trust_cases.items():
            if what == "url-safe alphabet" and "-" not in value and "_" not in value:
                continue
            keys = account_keys()
            keys["HJV_TRUST_PUBKEYS"] = value
            r = Run(CALLED="main", **keys)
            self.assertEqual(r.code, 1, f"{what}: {r.out}")
        seal_cases = {
            "zero (small order)": b64(bytes(32)),
            "one (small order)": b64(bytes([1]) + bytes(31)),
            "non-canonical": b64((cbk.P + 5).to_bytes(32, "little")),
            "33 bytes": b64(bytes(range(33))),
            "key id prefix": "1:" + x(fresh()),
            "base64 with stray bits": stray_bits(x(fresh())),
            "a line break at the end": x(fresh()) + "\n",
        }
        for what, value in seal_cases.items():
            keys = account_keys()
            keys["HJV_LOGIN_SEAL_PUBKEY"] = value
            self.assertEqual(Run(CALLED="main", **keys).code, 1, what)

    def test_annotations_in_actions(self):
        r = Run(CALLED="main", GITHUB_ACTIONS="true")
        self.assertEqual(r.code, 1)
        self.assertIn("::error::", r.out)


class PublishedPrivateKeysTest(unittest.TestCase):
    """Keys whose private key a tracked file writes out are test keys, listed or not."""

    def git_tree(self, files, untracked=None):
        tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, tmp)
        env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": tmp, "GIT_CONFIG_NOSYSTEM": "1"}
        for rel, text in files.items():
            os.makedirs(os.path.dirname(os.path.join(tmp, rel)), exist_ok=True)
            with open(os.path.join(tmp, rel), "w") as f:
                f.write(text)
        subprocess.run(["git", "init", "-q", tmp], env=env, check=True)
        subprocess.run(["git", "-C", tmp, "add", "-A"], env=env, check=True)
        for rel, text in (untracked or {}).items():
            with open(os.path.join(tmp, rel), "w") as f:
                f.write(text)
        return tmp

    def test_values_in_tracked_files(self):
        seeds = [fresh() for _ in range(4)]
        url_safe = base64.urlsafe_b64encode(seeds[2]).decode().rstrip("=")
        tree = self.git_tree({
            "src/vectors.rs": f'const SK: &str = "{b64(seeds[0])}";\n',
            "tests/data/v.json": f'{{\n  "seed_hex": "{seeds[1].hex()}",\n  "k": "{url_safe}"\n}}\n',
        }, untracked={"notes.txt": b64(seeds[3]) + "\n"})
        keys = cbk.public_test_keys(tree)
        self.assertIn(cbk.ed25519_public(seeds[0]), keys)
        self.assertIn(cbk.x25519_public(seeds[0]), keys)
        self.assertEqual(keys[cbk.x25519_public(seeds[0])], "its X25519 secret key is at src/vectors.rs:1")
        self.assertEqual(keys[cbk.ed25519_public(seeds[1])], "its Ed25519 seed is at tests/data/v.json:2")
        self.assertIn(cbk.ed25519_public(seeds[2]), keys)
        # what git does not track is not published
        self.assertNotIn(cbk.ed25519_public(seeds[3]), keys)
        # longer base64 and hex hold no 32-byte value
        longer = self.git_tree({"a.txt": b64(fresh() + b"x") + "\n" + (fresh() + b"x").hex() + "\n"})
        self.assertEqual(cbk.tree_values(longer), {})

    def test_every_published_key_pair_of_this_tree_is_refused(self):
        """Any key pair of the tracked files (a public key next to its private key, as a test
        vector has them), whether or not scripts/public-test-keys.txt names it."""
        self.assertEqual(b64(cbk.x25519_public(base64.b64decode(SEAL_VECTOR_SK))), SEAL_VECTOR_PK)
        values = cbk.tree_values(ROOT)
        keys = cbk.public_test_keys()
        pairs, missing = [], []
        for raw, where in values.items():
            for kind, derive in (("Ed25519", cbk.ed25519_public), ("X25519", cbk.x25519_public)):
                pub = derive(raw)
                if pub in values:
                    pairs.append(b64(pub))
                    if pub not in keys:
                        missing.append(f"{kind} {b64(pub)} at {values[pub]}, private key at {where}")
        self.assertEqual(missing, [])
        if os.path.exists(os.path.join(ROOT, "server", "api", "src", "seal.rs")):
            self.assertIn(SEAL_VECTOR_PK, pairs)


class OtherRunsTest(unittest.TestCase):
    def test_check_only_skips_the_required_keys_but_not_test_keys(self):
        r = Run(CALLED="main", CHECK_ONLY="true")
        self.assertEqual(r.code, 0, r.out)
        self.assertIn("Check build", r.summary)
        keys = account_keys()
        keys["HJV_TRUST_PUBKEYS"] = f"1:{ed(bytes([1]) * 32)},2:{ed(fresh())}"
        self.assertEqual(Run(CALLED="main", CHECK_ONLY="true", **keys).code, 1)

    def test_before_160_nothing_is_required(self):
        r = Run(version="1.5.1", CALLED="main")
        self.assertEqual(r.code, 0)
        self.assertEqual(r.warnings(), [])

    def test_runs_of_this_repository_only_warn(self):
        r = Run(CALLED="")
        self.assertEqual(r.code, 0)
        self.assertTrue(any("no account features" in w for w in r.warnings()), r.out)
        keys = account_keys()
        keys["HJV_TRUST_PUBKEYS"] = f"1:{ed(bytes([1]) * 32)},2:{ed(fresh())}"
        r = Run(CALLED="", **keys)
        self.assertEqual(r.code, 0)
        self.assertTrue(any("public test key" in w for w in r.warnings()), r.out)

    def test_relay_command_keys(self):
        relay = ["--relay"]
        self.assertEqual(Run(argv=relay, CALLED="main").code, 1)
        self.assertEqual(Run(argv=relay, CALLED="main", CHECK_ONLY="true").code, 0)
        self.assertEqual(Run(argv=relay, CALLED="").code, 0)
        key = ed(fresh())
        r = Run(argv=relay, CALLED="main", HJV_COMMAND_PUBKEY=f"1:{key}")
        self.assertEqual(r.code, 0, r.out)
        self.assertIn(cbk.fingerprint(base64.b64decode(key)), r.summary)
        self.assertEqual(Run(argv=relay, CALLED="main", HJV_COMMAND_PUBKEY="1:" + ed(bytes([42]) * 32)).code, 1)
        self.assertEqual(Run(argv=relay, CALLED="main", HJV_COMMAND_PUBKEY=f"1:{key},1:{ed(fresh())}").code, 1)
        self.assertEqual(Run(argv=relay, CALLED="main", HJV_COMMAND_PUBKEY=f"1:{key}\n").code, 1)

    def test_pubkey_command(self):
        seed = bytes([0x11]) * 32
        for kind, derive in (("ed25519", cbk.ed25519_public), ("x25519", cbk.x25519_public)):
            out = subprocess.run(["python3", "-I", SCRIPT, "pubkey", kind, seed.hex()],
                                 capture_output=True, text=True, check=True).stdout.strip()
            self.assertEqual(out, b64(derive(seed)))


class WorkflowTest(unittest.TestCase):
    """client-build.yml: the variables reach the builds, and its steps run this check."""

    @classmethod
    def setUpClass(cls):
        try:
            import yaml
        except ImportError:
            raise unittest.SkipTest("PyYAML is not installed")
        with open(WORKFLOW) as f:
            cls.wf = yaml.safe_load(f)
        # PyYAML reads the key "on" as True
        cls.on = cls.wf.get("on", cls.wf.get(True))

    def step(self, job, name):
        for s in self.wf["jobs"][job]["steps"]:
            if s.get("name") == name:
                return s
        self.fail(f"{job} has no step '{name}'")

    def test_variables_come_from_secrets_or_variables(self):
        env = self.wf["env"]
        for name in ("HJV_LOGIN_SEAL_PUBKEY", "HJV_TRUST_PUBKEYS", "HJV_UPDATE_PUBKEY"):
            self.assertEqual(env[name], f"${{{{ secrets.{name} || vars.{name} }}}}")
        self.assertEqual(self.wf["jobs"]["relay"]["env"]["HJV_COMMAND_PUBKEY"],
                         "${{ secrets.HJV_COMMAND_PUBKEY || vars.HJV_COMMAND_PUBKEY }}")
        # the command key reaches the relay job only, never the apps' builds
        self.assertNotIn("HJV_COMMAND_PUBKEY", env)

    def test_check_only_input(self):
        for trigger in ("workflow_dispatch", "workflow_call"):
            spec = self.on[trigger]["inputs"]["check_only"]
            self.assertEqual(spec["type"], "boolean")
            self.assertIs(spec["default"], False)
        self.assertIn("!inputs.check_only", self.step("ios", "Upload to TestFlight")["if"])
        self.assertIn("!inputs.check_only", self.wf["jobs"]["play"]["if"])
        self.assertIn("!inputs.check_only", self.wf["jobs"]["relay-release"]["if"])
        refuse = self.step("generate-bridge", "Check the check_only input")
        self.assertIn("inputs.check_only", refuse["if"])
        self.assertNotEqual(subprocess.run(["bash", "-c", refuse["run"]], capture_output=True).returncode, 0)

    def run_step(self, job, name, version, env):
        """The step's run script in a copy of what it needs, as the runner runs it."""
        step = self.step(job, name)
        tmp = tempfile.mkdtemp()
        try:
            os.makedirs(os.path.join(tmp, "scripts"))
            os.makedirs(os.path.join(tmp, "client"))
            for f in ("check-build-keys.py", "public-test-keys.txt"):
                shutil.copy(os.path.join(HERE, f), os.path.join(tmp, "scripts", f))
            with open(os.path.join(tmp, "client", "version.env"), "w") as f:
                f.write(f"HJV_VERSION={version}\n")
            clean = {k: v for k, v in os.environ.items() if k not in KEY_VARS}
            full = {**clean, "GITHUB_ACTIONS": "true", "GITHUB_STEP_SUMMARY": os.path.join(tmp, "summary.md"),
                    **{k: v for k, v in env.items() if v is not None}}
            done = subprocess.run(["bash", "-e", "-c", step["run"]], cwd=tmp, env=full,
                                  capture_output=True, text=True)
            return done.returncode, done.stdout + done.stderr
        finally:
            shutil.rmtree(tmp)

    def test_the_step_fails_a_called_160_run_without_a_key_or_with_a_test_key(self):
        name = "Check the pinned public keys"
        self.assertEqual(self.step("generate-bridge", name)["env"],
                         {"CALLED": "${{ inputs.source_ref }}", "CHECK_ONLY": "${{ inputs.check_only }}"})
        keys = account_keys()
        code, out = self.run_step("generate-bridge", name, "1.6.0", {"CALLED": "main", "CHECK_ONLY": "false", **keys})
        self.assertEqual(code, 0, out)
        self.assertNotIn("::error::", out)
        for missing in ("HJV_LOGIN_SEAL_PUBKEY", "HJV_TRUST_PUBKEYS"):
            env = {"CALLED": "main", "CHECK_ONLY": "false", **keys, missing: ""}
            code, out = self.run_step("generate-bridge", name, "1.6.0", env)
            self.assertEqual(code, 1, missing)
            self.assertIn(f"::error::{missing} did not arrive", out)
        env = {"CALLED": "main", "CHECK_ONLY": "false", **keys,
               "HJV_TRUST_PUBKEYS": f"1:{ed(bytes([1]) * 32)},2:{ed(fresh())}"}
        self.assertEqual(self.run_step("generate-bridge", name, "1.6.0", env)[0], 1)
        # check_only: no key needed; a run of this repository (no source_ref): warnings only
        self.assertEqual(self.run_step("generate-bridge", name, "1.6.0", {"CALLED": "", "CHECK_ONLY": "false"})[0], 0)
        self.assertEqual(self.run_step("generate-bridge", name, "1.6.0", {"CALLED": "main", "CHECK_ONLY": "true"})[0], 0)
        self.assertEqual(self.run_step("generate-bridge", name, "1.5.1", {"CALLED": "main", "CHECK_ONLY": "false"})[0], 0)

    def test_the_relay_job_checks_its_command_key(self):
        name = "Check the command keys"
        self.assertEqual(self.run_step("relay", name, "1.6.0", {"CALLED": "main", "CHECK_ONLY": "false"})[0], 1)
        self.assertEqual(self.run_step("relay", name, "1.6.0", {
            "CALLED": "main", "CHECK_ONLY": "false", "HJV_COMMAND_PUBKEY": f"1:{ed(fresh())}"})[0], 0)


if __name__ == "__main__":
    unittest.main()

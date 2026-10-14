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
            "HJV_UPDATE_ROOT_PUBKEY", "HJV_POLICY_FLOOR", "HJV_COMMAND_PUBKEY", "CALLED", "CHECK_ONLY",
            "GITHUB_ACTIONS", "GITHUB_STEP_SUMMARY", "GITHUB_ENV")
KEY_FILE = os.path.join(ROOT, "client", "update-keys.env")
# client/update-keys.env as it is before the user has made the keys
NO_KEYS = "# no keys yet\nHJV_UPDATE_PUBKEY=\nHJV_UPDATE_ROOT_PUBKEY=\nHJV_POLICY_FLOOR=1:0\n"
# `hjv-release test-keys --env` (tools/hjv-release): the keys of the vectors and the E2E builds
TEST_KEYS = ("HJV_UPDATE_PUBKEY=1:JUO5L/EJVRFHatyDadtt3JM2ZaEZeN2hQE7hBmypVZ0=,"
             "2:zRSzf5VulTGU/3+3Oz2B3MVh1hp1OAlLfD4aZD7l86o=\n"
             "HJV_UPDATE_ROOT_PUBKEY=101:T9CZzNR9eJPf6ewkQU7LDZtUICMqrTDZHEZb4zy+ZcQ=\n"
             "HJV_POLICY_FLOOR=1:0\n")
# server/api/src/seal.rs: the sealed-login test vector's key pair (VECTOR_PK, VECTOR_SK)
SEAL_VECTOR_PK = "SjgH0GTQdxgcwHCYnnaJHSDcpVWVSNwsd8GlAnOIKzg="
SEAL_VECTOR_SK = "d3iPGgzqABomMdrl0F29BiAI1bMPULnim+sqeCIokAQ="
# what str.splitlines() ends a line at besides LF and CR
LINE_BREAKS = ("\x0b", "\x0c", "\x1c", "\x1d", "\x1e", "\x85", "\u2028", "\u2029")


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


def key_file(signing=None, root=None, floor="1:0"):
    """The text of client/update-keys.env; by default with well-formed keys nobody published."""
    if signing is None:
        signing = f"1:{ed(fresh())},2:{ed(fresh())}"
    if root is None:
        root = f"101:{ed(fresh())}"
    return f"# a comment\n\nHJV_UPDATE_PUBKEY={signing}\nHJV_UPDATE_ROOT_PUBKEY={root}\nHJV_POLICY_FLOOR={floor}\n"


def with_torsion(key):
    """The base64 of an honest public key plus a point of order 8: on the curve and not of small
    order, yet outside the prime-order subgroup."""
    order8 = cbk._decode(bytes.fromhex("26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc05"))
    return b64(cbk._encode(cbk._add(cbk._decode(base64.b64decode(key)), order8)))


class Run:
    """check-build-keys.py's main() in this process, with a version file, a key file
    (client/update-keys.env: `update` is its text, False for no file, by default one with
    well-formed keys) and an environment. `pin` runs --pin-update-keys instead of the check."""

    def __init__(self, version="1.6.0", argv=(), update=None, pin=False, **env):
        self.tmp = tempfile.mkdtemp()
        self.version_file = os.path.join(self.tmp, "version.env")
        with open(self.version_file, "w") as f:
            f.write(f"# comment\nHJV_VERSION={version}\n")
        self.update_file = os.path.join(self.tmp, "update-keys.env")
        if update is not False:
            with open(self.update_file, "wb") as f:
                text = key_file() if update is None else update
                f.write(text if isinstance(text, bytes) else text.encode())
        self.summary_file = os.path.join(self.tmp, "summary.md")
        self.env_file = os.path.join(self.tmp, "github-env")
        env = {k: v for k, v in env.items() if v is not None}
        env.setdefault("GITHUB_STEP_SUMMARY", self.summary_file)
        if pin:
            env.setdefault("GITHUB_ENV", self.env_file)
        out, err = io.StringIO(), io.StringIO()
        clean = {k: v for k, v in os.environ.items() if k not in KEY_VARS}
        files = ["--version-file", self.version_file, "--update-keys", self.update_file]
        with mock.patch.dict(os.environ, {**clean, **env}, clear=True), \
                contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            self.code = cbk.main(list(argv) or (["--pin-update-keys"] if pin else []) + files)
        self.out, self.err = out.getvalue(), err.getvalue()
        self.summary = self.read(self.summary_file)
        # what the job's later steps get; None when nothing was handed on
        self.pinned = self.read(self.env_file) if os.path.exists(self.env_file) else None
        shutil.rmtree(self.tmp)

    @staticmethod
    def read(path):
        try:
            with open(path) as f:
                return f.read()
        except FileNotFoundError:
            return ""

    def errors(self):
        return [line for line in self.out.splitlines() if line.startswith("error: ")]

    def warnings(self):
        return [line for line in self.out.splitlines() if line.startswith("warning: ")]

    def notes(self):
        return [line for line in self.out.splitlines() if line.startswith("note: ")]


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

    def test_values_a_patch_adds_and_0x_hex(self):
        """A client patch adds a line "+<value>" (a key list or fixture in the client tree), and a
        test vector may write hex with 0x: both are published (WP-B verification)."""
        seeds = [fresh() for _ in range(3)]
        tree = self.git_tree({
            "client/patches/rustdesk/0040-x.patch": (
                "--- a/src/keys.txt\n+++ b/src/keys.txt\n@@ -0,0 +1,2 @@\n"
                f"+{b64(seeds[0])}\n+{seeds[1].hex()}\n"),
            "src/v.rs": f"const SEED: [u8; 32] = hex!(\"0x{seeds[2].hex()}\");\n",
        })
        keys = cbk.public_test_keys(tree)
        self.assertEqual(keys[cbk.ed25519_public(seeds[0])],
                         "its Ed25519 seed is at client/patches/rustdesk/0040-x.patch:4")
        self.assertIn(cbk.x25519_public(seeds[1]), keys)
        self.assertEqual(keys[cbk.ed25519_public(seeds[2])], "its Ed25519 seed is at src/v.rs:1")

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


class UpdateKeysTest(unittest.TestCase):
    """client/update-keys.env, the one place the update keys of a build come from."""

    # a release build of the build repository, its check build, and a run of this repository
    RELEASE = {"CALLED": "main"}
    CHECK = {"CALLED": "main", "CHECK_ONLY": "true"}
    PUBLIC = {"CALLED": ""}

    def run_kinds(self, update):
        """The runs of every kind with this key file and good account keys."""
        return [Run(update=update, **kind, **account_keys()) for kind in (self.RELEASE, self.CHECK, self.PUBLIC)]

    def test_the_committed_file_is_well_formed_and_holds_no_test_key(self):
        """Whatever the file holds when this runs: no keys yet, or the user's."""
        values, problems = cbk.read_update_keys(KEY_FILE)
        self.assertEqual(problems, [])
        signing, root, problems = cbk.parse_update_keys(values)
        self.assertEqual(problems, [])
        self.assertEqual(bool(signing), bool(root))
        published = cbk.public_test_keys()
        self.assertEqual([kid for kid, raw in signing + root if raw in published], [])
        self.assertRegex(values["HJV_POLICY_FLOOR"], r"^[1-9][0-9]*:[0-9]+$")

    def test_well_formed_keys_pass_and_show_in_the_summary(self):
        signing = [ed(fresh()), ed(fresh())]
        roots = [ed(fresh()), ed(fresh())]
        text = key_file(f"1:{signing[0]},2:{signing[1]}", f"101:{roots[0]},102:{roots[1]}", "2:17")
        for r in self.run_kinds(text):
            self.assertEqual(r.code, 0, r.out)
            self.assertEqual(r.errors() + r.warnings(), [])
            for kid, key in ((1, signing[0]), (2, signing[1])):
                self.assertIn(f"| `HJV_UPDATE_PUBKEY` | {kid} | `{cbk.fingerprint(base64.b64decode(key))}` |", r.summary)
            # both root keys (the trust rules' 1-4: 1.6.0 pins two)
            for kid, key in ((101, roots[0]), (102, roots[1])):
                self.assertIn(f"| `HJV_UPDATE_ROOT_PUBKEY` | {kid} | `{cbk.fingerprint(base64.b64decode(key))}` |", r.summary)
            self.assertIn("`HJV_POLICY_FLOOR` (2:17)", r.summary)
        # one signing key and one root key are enough; the largest numbers of each and of the floor
        one = key_file(f"100:{signing[0]}", f"199:{roots[0]}", "999999999999999:999999999999999")
        self.assertEqual([r.code for r in self.run_kinds(one)], [0, 0, 0])

    def test_a_file_without_keys(self):
        release, check, public = self.run_kinds(NO_KEYS)
        self.assertEqual(release.code, 1)
        self.assertTrue(any("holds no update keys" in e and "check_only" in e for e in release.errors()), release.out)
        self.assertEqual(check.code, 0, check.out)
        self.assertEqual(check.errors() + check.warnings(), [])
        self.assertIn("holds no update keys", check.summary)
        self.assertEqual(public.code, 0, public.out)
        self.assertTrue(any("holds no update keys" in w for w in public.warnings()), public.out)
        # the floor may be empty too while there are no keys
        bare = "HJV_UPDATE_PUBKEY=\nHJV_UPDATE_ROOT_PUBKEY=\nHJV_POLICY_FLOOR=\n"
        self.assertEqual([r.code for r in self.run_kinds(bare)], [1, 0, 0])
        # before 1.6.0 no build looked for update keys
        old = Run(version="1.5.1", CALLED="main", update=NO_KEYS)
        self.assertEqual((old.code, old.warnings()), (0, []))

    def test_public_test_keys(self):
        cases = {
            "the release tool's test keys": TEST_KEYS,
            "a signing key of one repeated byte": key_file(signing=f"1:{ed(bytes([7]) * 32)}"),
            "a signing key of consecutive bytes": key_file(signing=f"1:{ed(fresh())},2:{ed(bytes(range(0xc0, 0xe0)))}"),
            "a root key of consecutive bytes": key_file(root=f"101:{ed(bytes(range(0xa0, 0xc0)))}"),
            "an RFC 8032 example as root key": key_file(root=f"101:{ed(bytes.fromhex(cbk.RFC_SECRETS[1]))}"),
        }
        for what, text in cases.items():
            release, check, public = self.run_kinds(text)
            for r in (release, check):
                self.assertEqual(r.code, 1, what)
                self.assertTrue(any("public test key" in e for e in r.errors()), what)
            self.assertEqual(public.code, 0, what)
            self.assertTrue(any("public test key" in w for w in public.warnings()), what)
        # all three of the tool's keys are named, each by its place in the list file
        r = Run(update=TEST_KEYS, **self.RELEASE, **account_keys())
        self.assertEqual(len([e for e in r.errors() if "public-test-keys.txt" in e]), 3, r.out)

    def test_a_malformed_file_fails_every_run(self):
        good = key_file()
        lines = good.splitlines()
        cases = {
            "no file": False,
            "an empty file": "",
            "comments only": "# nothing here\n",
            "a byte order mark": "﻿" + good,
            "CR LF line endings": good.replace("\n", "\r\n"),
            "not UTF-8": good.encode() + b"# \xff\n",
            "an unknown name": good + "HJV_UPDATE_KEY=1:x\n",
            "a name twice": good + lines[-1] + "\n",
            "no floor line": "\n".join(lines[:-1]) + "\n",
            "no root line": "\n".join(lines[:-2] + lines[-1:]) + "\n",
            "export": good.replace("HJV_POLICY_FLOOR", "export HJV_POLICY_FLOOR"),
            "a space before the name": good.replace("HJV_POLICY_FLOOR", " HJV_POLICY_FLOOR"),
            "spaces around =": good.replace("HJV_POLICY_FLOOR=", "HJV_POLICY_FLOOR = "),
            "a quoted value": good.replace("HJV_POLICY_FLOOR=1:0", 'HJV_POLICY_FLOOR="1:0"'),
            "a comment after a value": good.replace("HJV_POLICY_FLOOR=1:0", "HJV_POLICY_FLOOR=1:0 # first"),
            "a space at the end of a line": good.replace("HJV_POLICY_FLOOR=1:0", "HJV_POLICY_FLOOR=1:0 "),
            "a space after the comma": good.replace(",2:", ", 2:"),
            "a tab in a value": good.replace(",2:", ",\t2:"),
            "an indented comment": good + "  # indented\n",
            "a shell line": good + "echo pwned > /tmp/x\n",
            "a NUL": good + "\0\n",
            "larger than 64 KiB": good + "#" * 70000 + "\n",
        }
        # LF alone ends a line, as tools/hjv-release reads the file; a character that str.splitlines(),
        # an editor or a browser ends a line at is refused wherever it is
        for sep in LINE_BREAKS:
            cases[f"{sep!r} between two lines"] = good.replace("\nHJV_UPDATE_ROOT_PUBKEY=", f"{sep}HJV_UPDATE_ROOT_PUBKEY=")
            cases[f"{sep!r} at the end of a line"] = good.replace("HJV_POLICY_FLOOR=1:0\n", f"HJV_POLICY_FLOOR=1:0{sep}\n")
            cases[f"{sep!r} in a comment"] = good.replace("# a comment\n", f"# a comment{sep}HJV_POLICY_FLOOR=1:1\n")
        for what, text in cases.items():
            runs = self.run_kinds(text) + [Run(version="1.5.1", update=text, CALLED="main")]
            self.assertEqual([r.code for r in runs], [1, 1, 1, 1], what)
            for r in runs:
                self.assertTrue(r.errors(), what)
                self.assertEqual(r.pinned, None)
        # the message says what to do about the file
        for what, said in (("a byte order mark", "byte order mark"), ("CR LF line endings", "CR line endings"),
                           ("no file", "is not a file"), ("not UTF-8", "not UTF-8"), ("a name twice", "a second time"),
                           ("no floor line", "has no HJV_POLICY_FLOOR= line"), ("export", ":5: neither a comment"),
                           ("larger than 64 KiB", "larger than 64 KiB"),
                           (r"'\x85' between two lines", ":3: holds U+0085, which other programs take for a line break"),
                           (r"'\x85' between two lines", "has no HJV_UPDATE_ROOT_PUBKEY= line"),
                           (r"'\u2028' in a comment", ":1: holds U+2028")):
            r = Run(update=cases[what], CALLED="")
            self.assertTrue(any(said in e for e in r.errors()), f"{what}: {r.out}")
        # a link is not the file either
        tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, tmp)
        with open(os.path.join(tmp, "real.env"), "w") as f:
            f.write(good)
        os.symlink(os.path.join(tmp, "real.env"), os.path.join(tmp, "link.env"))
        self.assertEqual(cbk.read_update_keys(os.path.join(tmp, "real.env"))[1], [])
        self.assertEqual(cbk.read_update_keys(os.path.join(tmp, "link.env"))[0], None)
        self.assertEqual(cbk.read_update_keys(tmp)[0], None)

    def test_the_refused_line_breaks_are_those_of_str_splitlines(self):
        """Every character besides LF and CR that str.splitlines() ends a line at, and no other."""
        breaks, refused = [], []
        for c in map(chr, range(0x110000)):
            if c not in "\n\r" and len(f"a{c}b".splitlines()) > 1:
                breaks.append(c)
            if cbk._OTHER_LINE_BREAK.search(c):
                refused.append(c)
        self.assertEqual(breaks, list(LINE_BREAKS))
        self.assertEqual(refused, list(LINE_BREAKS))

    def test_keys_and_floors_the_apps_would_refuse_fail_every_run(self):
        a, b, c = (ed(fresh()) for _ in range(3))
        root = ed(fresh())
        cases = {
            "three signing keys": key_file(signing=f"1:{a},2:{b},3:{c}"),
            "three root keys": key_file(root=f"101:{a},102:{b},103:{c}"),
            "a signing key numbered 101": key_file(signing=f"1:{a},101:{b}"),
            "a root key numbered 100": key_file(root=f"100:{root}"),
            "a root key numbered 200": key_file(root=f"101:{root},200:{c}"),
            "the root key's id is a signing key's": key_file(signing=f"1:{a},2:{b}", root=f"2:{root}"),
            "the root key is a signing key": key_file(signing=f"1:{a},2:{b}", root=f"101:{b}"),
            "a signing key twice": key_file(signing=f"1:{a},2:{a}"),
            "a signing key id twice": key_file(signing=f"1:{a},1:{b}"),
            "signing keys without a root key": key_file(root=""),
            "a root key without signing keys": key_file(signing=""),
            "keys without a floor": key_file(floor=""),
            "key id 0": key_file(signing=f"0:{a}"),
            "key id 65536": key_file(root=f"65536:{root}"),
            "key id 01": key_file(signing=f"01:{a}"),
            "no key id": key_file(root=root),
            "an empty entry": key_file(signing=f"1:{a},"),
            "31 bytes": key_file(signing=f"1:{b64(bytes(31))}"),
            "no padding": key_file(signing=f"1:{a.rstrip('=')}"),
            "base64 with stray bits": key_file(root=f"101:{stray_bits(root)}"),
            "the identity, a weak key": key_file(signing=f"1:{b64(bytes([1]) + bytes(31))}"),
            "a weak root key": key_file(root="101:" + b64(bytes.fromhex(
                "26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc05"))),
            "not on the curve": key_file(signing=f"1:{b64(bytes([2]) + bytes(31))}"),
            "a non-canonical encoding": key_file(signing=f"1:{b64((cbk.P + 1).to_bytes(32, 'little'))}"),
            "a signing key outside the subgroup": key_file(signing=f"1:{a},2:{with_torsion(b)}"),
            "a root key outside the subgroup": key_file(root=f"101:{with_torsion(root)}"),
        }
        for floor in ("0:0", "1", "1:", ":1", "1:01", "01:0", "1:-1", "+1:0", "1:0:0", "1,0", "1:" + "9" * 16,
                      "1" * 16 + ":0", "a:b"):
            cases[f"floor {floor}"] = key_file(floor=floor)
            cases[f"floor {floor} without keys"] = NO_KEYS.replace("1:0", floor)
        for what, text in cases.items():
            runs = self.run_kinds(text)
            self.assertEqual([r.code for r in runs], [1, 1, 1], what)
            for r in runs:
                self.assertTrue(r.errors(), what)
        for what, said in (
            ("three signing keys", "HJV_UPDATE_PUBKEY has more than 2 keys"),
            ("three root keys", "HJV_UPDATE_ROOT_PUBKEY has more than 2 keys"),
            ("a signing key numbered 101", "HJV_UPDATE_PUBKEY: a signing key's id is not 1-100"),
            ("a root key numbered 100", "HJV_UPDATE_ROOT_PUBKEY: a root key's id is not 101-199"),
            ("a root key numbered 200", "HJV_UPDATE_ROOT_PUBKEY: a root key's id is not 101-199"),
            ("the root key's id is a signing key's", "key id 2 is the root key's and a signing key's"),
            ("the root key is a signing key", "the root key is also a signing key"),
            ("signing keys without a root key", "HJV_UPDATE_ROOT_PUBKEY is empty while the other key line is set"),
            ("a root key without signing keys", "HJV_UPDATE_PUBKEY is empty while the other key line is set"),
            ("keys without a floor", "HJV_POLICY_FLOOR is empty"),
            ("floor 0:0", "HJV_POLICY_FLOOR is not <epoch>:<seq>"),
            ("the identity, a weak key", "weak key"),
        ):
            r = Run(update=cases[what], CALLED="")
            self.assertTrue(any(said in e for e in r.errors()), f"{what}: {r.out}")
        # the tests above must not pass for another reason than the one they name
        self.assertTrue(cbk.ed25519_in_subgroup(cbk._decode(base64.b64decode(b))))
        mixed = cbk._decode(base64.b64decode(with_torsion(b)))
        self.assertFalse(cbk.ed25519_weak(mixed))
        self.assertFalse(cbk.ed25519_in_subgroup(mixed))
        r = Run(update=key_file(root=f"101:{with_torsion(root)}"), **self.RELEASE, **account_keys())
        self.assertTrue(any("outside the prime-order subgroup" in e for e in r.errors()), r.out)

    def test_the_account_keys_are_judged_as_before(self):
        """A point outside the subgroup is refused for the update keys only: the trust-list and
        command keys keep the rules they had."""
        keys = account_keys()
        keys["HJV_TRUST_PUBKEYS"] = f"1:{with_torsion(ed(fresh()))},2:{ed(fresh())}"
        self.assertEqual(Run(CALLED="main", **keys).code, 0)

    def test_what_a_line_holds_is_never_repeated(self):
        """Something else than a public key may have been pasted into the file."""
        pasted = "AGE-SECRET-KEY-1QQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQ"
        seed = b64(fresh())
        for text in (key_file() + pasted + "\n", key_file() + f"HJV_UPDATE_SEED={seed}\n",
                     key_file(signing=f"1:{seed[:-4]}"), key_file(root=f"x{seed}")):
            for r in self.run_kinds(text):
                self.assertEqual(r.code, 1)
                for shown in (r.out, r.err, r.summary):
                    self.assertNotIn(pasted, shown)
                    self.assertNotIn(seed[:20], shown)

    def test_the_environment_cannot_say_otherwise(self):
        """A secret or variable that reached the environment under one of the names."""
        text = key_file()
        values = dict(line.split("=", 1) for line in text.splitlines() if line.startswith("HJV_"))
        other = {
            "HJV_UPDATE_PUBKEY": f"1:{ed(fresh())}",
            "HJV_UPDATE_ROOT_PUBKEY": f"101:{ed(fresh())}",
            "HJV_POLICY_FLOOR": "1:1",
        }
        for name in cbk.UPDATE_NAMES:
            for kind in (self.RELEASE, self.CHECK, self.PUBLIC):
                r = Run(update=text, **kind, **account_keys(), **{name: other[name]})
                self.assertEqual(r.code, 1, name)
                self.assertTrue(any(name in e and "environment" in e for e in r.errors()), r.out)
                # with a file without keys too: the build would take the file's empty value
                self.assertEqual(Run(update=NO_KEYS, **kind, **account_keys(), **{name: other[name]}).code, 1)
        # the same values, as after the pin step, and empty ones are no contradiction
        self.assertEqual(Run(update=text, **self.RELEASE, **account_keys(), **values).code, 0)
        empty = {name: "" for name in cbk.UPDATE_NAMES}
        self.assertEqual(Run(update=text, **self.RELEASE, **account_keys(), **empty).code, 0)

    def test_the_release_tool_reads_the_file_alike(self):
        """`hjv-release keys --production` (tools/hjv-release) passes exactly the files a release
        build passes here, on the cases both name; set HJV_RELEASE_BIN to the built tool."""
        tool = os.environ.get("HJV_RELEASE_BIN")
        if not tool:
            self.skipTest("HJV_RELEASE_BIN is not set")
        a, b = ed(fresh()), ed(fresh())
        root = ed(fresh())
        cases = {
            "good keys": key_file(),
            "one signing key": key_file(signing=f"7:{a}"),
            "no keys": NO_KEYS,
            "the tool's test keys": TEST_KEYS,
            "a repeated-byte seed": key_file(signing=f"1:{ed(bytes([9]) * 32)}"),
            "three signing keys": key_file(signing=f"1:{a},2:{b},3:{ed(fresh())}"),
            "two root keys": key_file(root=f"101:{a},102:{b}"),
            "shared id": key_file(signing=f"1:{a}", root=f"1:{root}"),
            "shared key": key_file(signing=f"1:{a}", root=f"101:{a}"),
            "no root": key_file(root=""),
            "no signing": key_file(signing=""),
            "no floor": key_file(floor=""),
            "floor 0:0": key_file(floor="0:0"),
            "floor 1:01": key_file(floor="1:01"),
            "weak": key_file(signing=f"1:{b64(bytes([1]) + bytes(31))}"),
            "outside the subgroup": key_file(signing=f"1:{with_torsion(a)}"),
            "stray bits": key_file(signing=f"1:{stray_bits(a)}"),
            "key id 01": key_file(signing=f"01:{a}"),
            "unknown name": key_file() + "HJV_OTHER=1\n",
            "a name twice": key_file() + "HJV_POLICY_FLOOR=1:0\n",
        }
        # the tool too reads two lines joined by another line break as one line
        for sep in LINE_BREAKS:
            cases[f"{sep!r} between two lines"] = key_file().replace("\nHJV_UPDATE_ROOT_PUBKEY=", f"{sep}HJV_UPDATE_ROOT_PUBKEY=")
        tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, tmp)
        for what, text in cases.items():
            path = os.path.join(tmp, "keys.env")
            with open(path, "w", encoding="utf-8") as f:
                f.write(text)
            theirs = subprocess.run([tool, "keys", "--production", "--keys", path], capture_output=True).returncode
            ours = Run(update=text, **self.RELEASE, **account_keys()).code
            self.assertEqual(ours, theirs, what)


class PinUpdateKeysTest(unittest.TestCase):
    """--pin-update-keys: what the platform jobs' builds get."""

    def test_the_values_of_the_file_reach_the_job(self):
        text = key_file(floor="3:12")
        lines = [line for line in text.splitlines() if line.startswith("HJV_")]
        for kind in ({"CALLED": "main"}, {"CALLED": "main", "CHECK_ONLY": "true"}, {"CALLED": ""}):
            r = Run(pin=True, update=text, **kind)
            self.assertEqual(r.code, 0, r.out)
            self.assertEqual(r.pinned, "\n".join(lines) + "\n")
            self.assertEqual([line.split("=")[0] for line in lines], list(cbk.UPDATE_NAMES))
            self.assertIn("pinned HJV_POLICY_FLOOR=3:12", r.err)
            self.assertEqual(r.summary, "")  # the Bridge codegen job writes the summary

    def test_a_file_without_keys_pins_empty_values(self):
        """So that nothing else in the job's environment can stand in for them."""
        want = "HJV_UPDATE_PUBKEY=\nHJV_UPDATE_ROOT_PUBKEY=\nHJV_POLICY_FLOOR=1:0\n"
        for kind in ({"CALLED": "main", "CHECK_ONLY": "true"}, {"CALLED": ""}):
            r = Run(pin=True, update=NO_KEYS, **kind)
            self.assertEqual((r.code, r.pinned), (0, want), r.out)

    def test_nothing_is_pinned_when_the_check_fails(self):
        r = Run(pin=True, update=NO_KEYS, CALLED="main")
        self.assertEqual((r.code, r.pinned), (1, None), r.out)
        for check_only in ("false", "true"):
            r = Run(pin=True, update=TEST_KEYS, CALLED="main", CHECK_ONLY=check_only)
            self.assertEqual((r.code, r.pinned), (1, None), r.out)
        for kind in ({"CALLED": "main"}, {"CALLED": "main", "CHECK_ONLY": "true"}, {"CALLED": ""}):
            for text in (False, "", key_file(floor="0:0"), key_file(root=""), key_file() + "X=1\n"):
                r = Run(pin=True, update=text, **kind)
                self.assertEqual((r.code, r.pinned), (1, None), r.out)
            r = Run(pin=True, HJV_UPDATE_PUBKEY=f"1:{ed(fresh())}", **kind)
            self.assertEqual((r.code, r.pinned), (1, None), r.out)

    def test_without_github_env_the_values_are_printed(self):
        text = key_file()
        r = Run(pin=True, update=text, GITHUB_ENV="")
        self.assertEqual(r.code, 0)
        self.assertEqual(r.pinned, None)
        self.assertEqual(r.out, "".join(line + "\n" for line in text.splitlines() if line.startswith("HJV_")))

    def test_what_earlier_steps_handed_on_stays(self):
        """$GITHUB_ENV is appended to: the build name and number of other steps are kept."""
        tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, tmp)
        path = os.path.join(tmp, "env")
        with open(path, "w") as f:
            f.write("BUILD_NUMBER=262870101\n")
        self.assertEqual(Run(pin=True, update=NO_KEYS, CALLED="", GITHUB_ENV=path).code, 0)
        with open(path) as f:
            self.assertEqual(f.read(), "BUILD_NUMBER=262870101\nHJV_UPDATE_PUBKEY=\nHJV_UPDATE_ROOT_PUBKEY=\n"
                                       "HJV_POLICY_FLOOR=1:0\n")

    def test_the_file_of_this_tree_is_the_default(self):
        """Without --update-keys, client/update-keys.env beside the script, shown by its
        relative path: whatever it holds now, a run of this repository pins it."""
        clean = {k: v for k, v in os.environ.items() if k not in KEY_VARS}
        done = subprocess.run(["python3", "-I", SCRIPT, "--pin-update-keys"], cwd=ROOT, env={**clean, "CALLED": ""},
                              capture_output=True, text=True)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertEqual([line.split("=")[0] for line in done.stdout.splitlines() if line.startswith("HJV_")],
                         list(cbk.UPDATE_NAMES))
        self.assertNotIn(ROOT, done.stdout + done.stderr)

    def test_warnings_are_plain_lines(self):
        """The Bridge codegen job has given the run's warnings; three more jobs do not repeat them."""
        for text in (NO_KEYS, TEST_KEYS):
            r = Run(pin=True, update=text, CALLED="", GITHUB_ACTIONS="true")
            self.assertEqual(r.code, 0)
            self.assertNotIn("::warning::", r.out)
            self.assertTrue(r.notes(), r.out)
        r = Run(pin=True, update=TEST_KEYS, CALLED="main", GITHUB_ACTIONS="true")
        self.assertIn("::error::", r.out)

    def test_the_command_key_check_takes_no_key_file(self):
        with self.assertRaises(SystemExit), contextlib.redirect_stderr(io.StringIO()):
            cbk.main(["--relay", "--pin-update-keys"])


class OtherRunsTest(unittest.TestCase):
    def test_check_only_skips_the_required_keys_but_not_test_keys(self):
        r = Run(CALLED="main", CHECK_ONLY="true", update=NO_KEYS)
        self.assertEqual(r.code, 0, r.out)
        self.assertIn("Check build", r.summary)
        keys = account_keys()
        keys["HJV_TRUST_PUBKEYS"] = f"1:{ed(bytes([1]) * 32)},2:{ed(fresh())}"
        self.assertEqual(Run(CALLED="main", CHECK_ONLY="true", **keys).code, 1)

    def test_before_160_nothing_is_required(self):
        r = Run(version="1.5.1", CALLED="main", update=NO_KEYS)
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
        for name in ("HJV_LOGIN_SEAL_PUBKEY", "HJV_TRUST_PUBKEYS"):
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

    def run_step(self, job, name, version, env, update=None):
        """The step's run script in a copy of what it needs, as the runner runs it: (exit code,
        output, what the step handed to the job's later steps through $GITHUB_ENV). `update` is
        the text of client/update-keys.env, False for no such file, by default well-formed keys."""
        step = self.step(job, name)
        tmp = tempfile.mkdtemp()
        try:
            os.makedirs(os.path.join(tmp, "scripts"))
            os.makedirs(os.path.join(tmp, "client"))
            for f in ("check-build-keys.py", "public-test-keys.txt"):
                shutil.copy(os.path.join(HERE, f), os.path.join(tmp, "scripts", f))
            with open(os.path.join(tmp, "client", "version.env"), "w") as f:
                f.write(f"HJV_VERSION={version}\n")
            if update is not False:
                with open(os.path.join(tmp, "client", "update-keys.env"), "w") as f:
                    f.write(key_file() if update is None else update)
            clean = {k: v for k, v in os.environ.items() if k not in KEY_VARS}
            full = {**clean, "GITHUB_ACTIONS": "true", "GITHUB_STEP_SUMMARY": os.path.join(tmp, "summary.md"),
                    "GITHUB_ENV": os.path.join(tmp, "github-env"),
                    **{k: v for k, v in env.items() if v is not None}}
            done = subprocess.run(["bash", "-e", "-c", step["run"]], cwd=tmp, env=full,
                                  capture_output=True, text=True)
            return done.returncode, done.stdout + done.stderr, Run.read(os.path.join(tmp, "github-env"))
        finally:
            shutil.rmtree(tmp)

    def test_the_step_fails_a_called_160_run_without_a_key_or_with_a_test_key(self):
        name = "Check the pinned public keys"
        self.assertEqual(self.step("generate-bridge", name)["env"],
                         {"CALLED": "${{ inputs.source_ref }}", "CHECK_ONLY": "${{ inputs.check_only }}"})
        keys = account_keys()
        code, out, _ = self.run_step("generate-bridge", name, "1.6.0", {"CALLED": "main", "CHECK_ONLY": "false", **keys})
        self.assertEqual(code, 0, out)
        self.assertNotIn("::error::", out)
        for missing in ("HJV_LOGIN_SEAL_PUBKEY", "HJV_TRUST_PUBKEYS"):
            env = {"CALLED": "main", "CHECK_ONLY": "false", **keys, missing: ""}
            code, out, _ = self.run_step("generate-bridge", name, "1.6.0", env)
            self.assertEqual(code, 1, missing)
            self.assertIn(f"::error::{missing} did not arrive", out)
        env = {"CALLED": "main", "CHECK_ONLY": "false", **keys,
               "HJV_TRUST_PUBKEYS": f"1:{ed(bytes([1]) * 32)},2:{ed(fresh())}"}
        self.assertEqual(self.run_step("generate-bridge", name, "1.6.0", env)[0], 1)
        # check_only: no key needed; a run of this repository (no source_ref): warnings only
        none = {"update": NO_KEYS}
        self.assertEqual(self.run_step("generate-bridge", name, "1.6.0", {"CALLED": "", "CHECK_ONLY": "false"}, **none)[0], 0)
        self.assertEqual(self.run_step("generate-bridge", name, "1.6.0", {"CALLED": "main", "CHECK_ONLY": "true"}, **none)[0], 0)
        self.assertEqual(self.run_step("generate-bridge", name, "1.5.1", {"CALLED": "main", "CHECK_ONLY": "false"}, **none)[0], 0)

    def test_the_relay_job_checks_its_command_key(self):
        name = "Check the command keys"
        self.assertEqual(self.run_step("relay", name, "1.6.0", {"CALLED": "main", "CHECK_ONLY": "false"})[0], 1)
        self.assertEqual(self.run_step("relay", name, "1.6.0", {
            "CALLED": "main", "CHECK_ONLY": "false", "HJV_COMMAND_PUBKEY": f"1:{ed(fresh())}"})[0], 0)

    # --- the update keys: client/update-keys.env and nothing else (minimum version design 6-4) ---

    APP_JOBS = ("windows", "android", "ios")
    KINDS = {"CALLED": "main", "CHECK_ONLY": "false"}, {"CALLED": "main", "CHECK_ONLY": "true"}, \
        {"CALLED": "", "CHECK_ONLY": "false"}

    def test_no_secret_or_variable_can_set_an_update_key(self):
        """The acceptance of the work package: a variable of the build repository cannot change
        the update keys, and there is no new secret."""
        with open(WORKFLOW) as f:
            text = f.read()
        for name in cbk.UPDATE_NAMES:
            self.assertNotRegex(text, r"(secrets|vars|inputs)\." + name)
        envs = [("the workflow", self.wf.get("env", {}))]
        for job_name, job in self.wf["jobs"].items():
            envs.append((job_name, job.get("env", {})))
            for step in job.get("steps", []):
                where = f"{job_name}: {step.get('name')}"
                envs.append((where, step.get("env", {})))
                # nor does a step write one of them by itself
                for name in cbk.UPDATE_NAMES:
                    self.assertNotIn(name, step.get("run", ""), where)
        for where, env in envs:
            for name in cbk.UPDATE_NAMES:
                self.assertNotIn(name, env, where)

    def test_the_check_step_reads_the_key_file(self):
        """The step as the runner runs it, with a key file in each state (UpdateKeysTest has the
        cases; here the account keys are left out where a run needs none, which is quicker)."""
        name = "Check the pinned public keys"
        self.assertIn("--update-keys client/update-keys.env", self.step("generate-bridge", name)["run"])
        release, check, public = self.KINDS
        # good keys: a release build passes
        code, out, _ = self.run_step("generate-bridge", name, "1.6.0", {**release, **account_keys()})
        self.assertEqual(code, 0, out)
        # no keys yet: only a check build, or a run of this repository
        code, out, _ = self.run_step("generate-bridge", name, "1.6.0", release, update=NO_KEYS)
        self.assertEqual(code, 1)
        self.assertIn("::error::client/update-keys.env holds no update keys", out)
        code, out, _ = self.run_step("generate-bridge", name, "1.6.0", check, update=NO_KEYS)
        self.assertEqual(code, 0, out)
        self.assertNotIn("::error::", out)
        code, out, _ = self.run_step("generate-bridge", name, "1.6.0", public, update=NO_KEYS)
        self.assertEqual(code, 0, out)
        self.assertIn("::warning::client/update-keys.env holds no update keys", out)
        # this repository's test keys: never in a run of the build repository
        code, out, _ = self.run_step("generate-bridge", name, "1.6.0", check, update=TEST_KEYS)
        self.assertEqual(code, 1)
        self.assertIn("::error::HJV_UPDATE_PUBKEY key 1 is a public test key", out)
        self.assertIn("::error::HJV_UPDATE_PUBKEY key 2 is a public test key", out)
        self.assertIn("::error::HJV_UPDATE_ROOT_PUBKEY key 101 is a public test key", out)
        code, out, _ = self.run_step("generate-bridge", name, "1.6.0", public, update=TEST_KEYS)
        self.assertEqual(code, 0, out)
        self.assertIn("::warning::HJV_UPDATE_PUBKEY key 1 is a public test key", out)
        # a malformed file: never, not even in a check build or a run of this repository
        for text in (False, key_file() + "export A=1\n", key_file(root="")):
            for env in (check, public):
                code, out, _ = self.run_step("generate-bridge", name, "1.6.0", env, update=text)
                self.assertEqual(code, 1, out)
                self.assertIn("::error::", out)

    def test_every_app_job_pins_the_update_keys_before_it_builds(self):
        want = {
            "name": "Pin the update keys",
            "shell": "bash",
            "working-directory": "${{ github.workspace }}",
            "env": {"CALLED": "${{ inputs.source_ref }}", "CHECK_ONLY": "${{ inputs.check_only }}"},
            "run": "python3 -I scripts/check-build-keys.py --pin-update-keys --version-file client/version.env "
                   "--update-keys client/update-keys.env",
        }
        for job in self.APP_JOBS:
            steps = self.wf["jobs"][job]["steps"]
            names = [s.get("name") for s in steps]
            at = names.index("Pin the update keys")
            # unconditional, and what the check step of the Bridge codegen job is given
            self.assertEqual(steps[at], want, job)
            self.assertEqual(want["env"], self.step("generate-bridge", "Check the pinned public keys")["env"])
            # of this checkout (on Windows the one with LF line endings), before anything is built
            self.assertGreater(at, names.index("Check out hjviewer"), job)
            if job == "windows":
                self.assertGreater(at, names.index("Re-check out hjviewer with LF line endings"))
            self.assertLess(at, names.index("Fetch upstream and apply hjviewer patches"), job)
            for step in steps[:at]:
                for builder in ("cargo", "flutter build", "build.py", "ndk_arm64"):
                    self.assertNotIn(builder, step.get("run", ""), f"{job}: {step.get('name')}")
        # the jobs that build no app need no update key
        for job in set(self.wf["jobs"]) - set(self.APP_JOBS):
            self.assertNotIn("Pin the update keys", [s.get("name") for s in self.wf["jobs"][job]["steps"]])

    def test_the_pin_step_hands_on_the_file_or_fails(self):
        """The step as the runner runs it; the three jobs have the same step (the test above)."""
        job, name = "windows", "Pin the update keys"
        release, check, public = self.KINDS
        text = key_file(floor="1:4")
        want = "".join(line + "\n" for line in text.splitlines() if line.startswith("HJV_"))
        code, out, pinned = self.run_step(job, name, "1.6.0", release, update=text)
        self.assertEqual((code, pinned), (0, want), out)
        self.assertEqual(self.run_step(job, name, "1.6.0", release, update=NO_KEYS)[::2], (1, ""))
        for env in (check, public):
            code, out, pinned = self.run_step(job, name, "1.6.0", env, update=NO_KEYS)
            self.assertEqual((code, pinned), (0, "HJV_UPDATE_PUBKEY=\nHJV_UPDATE_ROOT_PUBKEY=\nHJV_POLICY_FLOOR=1:0\n"), out)
            self.assertNotIn("::warning::", out)
        self.assertEqual(self.run_step(job, name, "1.6.0", check, update=TEST_KEYS)[::2], (1, ""))
        for env in (check, public):
            self.assertEqual(self.run_step(job, name, "1.6.0", env, update=key_file(signing=""))[::2], (1, ""))
            self.assertEqual(self.run_step(job, name, "1.6.0", env, update=False)[::2], (1, ""))


if __name__ == "__main__":
    unittest.main()

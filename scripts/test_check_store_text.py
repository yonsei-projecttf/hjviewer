# SPDX-License-Identifier: AGPL-3.0-only
"""Offline tests of scripts/check-store-text.py on the repository's store/ and on changed copies of
it (store/README.md):

    python3 -I -B -m unittest discover -s scripts -p 'test_check_store_text.py'
"""
import contextlib
import glob
import hashlib
import importlib.util
import io
import os
import re
import shutil
import subprocess
import sys
import tempfile
import unicodedata
import unittest
from unittest import mock

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
SCRIPT = os.path.join(HERE, "check-store-text.py")
STORE = os.path.join(ROOT, "store")

_spec = importlib.util.spec_from_file_location("check_store_text", SCRIPT)
cst = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(cst)

UNLISTED = "This app is intended for unlisted distribution."
# made-up demo PC values for the --filled tests (never real ones)
FAKE_ID = "987654321"
FAKE_PASSWORD = "Made-up-Test-Password-7f3!"


class StoreCopy:
    """A throwaway copy of store/ to change one thing in."""

    def __init__(self, case):
        self.dir = tempfile.mkdtemp(prefix="hjv-store-")
        case.addCleanup(shutil.rmtree, self.dir, True)
        self.store = os.path.join(self.dir, "store")
        shutil.copytree(STORE, self.store)

    def path(self, rel):
        return os.path.join(self.store, rel)

    def read(self, rel):
        with open(self.path(rel), encoding="utf-8") as f:
            return f.read()

    def write(self, rel, text, raw=False):
        os.makedirs(os.path.dirname(self.path(rel)), exist_ok=True)
        with open(self.path(rel), "wb") as f:
            f.write(text if raw else text.encode("utf-8"))

    def value(self, rel, value):
        self.write(rel, value + "\n")

    def run(self, *args, env=None):
        out = io.StringIO()
        with mock.patch.dict(os.environ, env or {}, clear=False), contextlib.redirect_stdout(out):
            code = cst.main(["--store", self.store, *args])
        return code, out.getvalue()


class StoreTextTest(unittest.TestCase):
    def check(self, copy, *args, fails=None, env=None):
        code, out = copy.run(*args, env=env)
        if fails is None:
            self.assertEqual(code, 0, out)
        else:
            self.assertEqual(code, 1, out)
            self.assertRegex(out, fails)
        return out

    def test_the_repository_store(self):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = cst.main(["--store", STORE])
        self.assertEqual(code, 0, out.getvalue())
        with open(os.path.join(ROOT, "client", "version.env"), encoding="utf-8") as f:
            version = re.search(r"^HJV_VERSION=(\S+)$", f.read(), re.M).group(1)
        with open(os.path.join(STORE, "appstore", "version.txt"), encoding="utf-8") as f:
            self.assertEqual(f.read().strip(), version, "store/ is written for the app's version")
        with open(os.path.join(STORE, "appstore", "distribution.txt"), encoding="utf-8") as f:
            self.assertEqual(f.read(), "unlisted\n")
        with open(os.path.join(STORE, "appstore", "locales.txt"), encoding="utf-8") as f:
            self.assertEqual(f.read(), "ko\nen-GB\n")

    def test_the_limits(self):
        for rel, kind, limit in (("appstore/ko/name.txt", "chars", 30), ("appstore/en-GB/subtitle.txt", "chars", 30),
                                 ("appstore/ko/promotional_text.txt", "chars", 170),
                                 ("appstore/en-GB/description.txt", "chars", 4000),
                                 ("appstore/ko/whats_new.txt", "chars", 4000),
                                 ("appstore/ko/keywords.txt", "bytes", 100)):
            with self.subTest(rel=rel):
                copy = StoreCopy(self)
                if kind == "bytes":
                    # Hangul takes 3 bytes: 33 syllables and a letter is 100 bytes, one more is 103
                    copy.value(rel, "가" * 33 + "a")
                    self.check(copy)
                    copy.value(rel, "가" * 34)
                else:
                    char = "가" if "/ko/" in rel else "a"  # characters, not bytes
                    copy.value(rel, char * limit)
                    self.check(copy)
                    copy.value(rel, char * (limit + 1))
                self.check(copy, fails=re.escape(rel) + r": \d+ of " + str(limit))
        copy = StoreCopy(self)
        copy.value("appstore/ko/name.txt", "H")
        self.check(copy, fails=r"needs at least 2")
        copy = StoreCopy(self)
        copy.value("appstore/ko/name.txt", "HJViewer\nPC")
        self.check(copy, fails=r"more than one line")

    def test_keywords(self):
        for value, rule in (("원격제어, 컴퓨터", "blank around a comma"), ("원격제어,,컴퓨터", "empty keyword"),
                            ("wake,Wake", "repeated keyword")):
            with self.subTest(value=value):
                copy = StoreCopy(self)
                copy.value("appstore/en-GB/keywords.txt" if value.isascii() else "appstore/ko/keywords.txt", value)
                self.check(copy, fails=rule)

    def notes(self, copy):
        return copy.read("appstore/review_notes.txt")[:-1]

    def test_the_review_notes_bytes_once_filled(self):
        copy = StoreCopy(self)
        head = UNLISTED + "\n\nID {{DEMO_PC_ID}}, password {{DEMO_PC_PASSWORD}}.\n\n"
        worst = cst.fill(head, {"DEMO_PC_ID": "x" * cst.PC_ID_MAX, "DEMO_PC_PASSWORD": "x" * cst.PC_PASSWORD_MAX})
        pad = cst.REVIEW_NOTES_BYTES - cst.notes_bytes(worst)
        copy.value("appstore/review_notes.txt", head + "a" * pad)
        out = self.check(copy)
        self.assertIn(f"4000 of 4000 bytes filled with the worst case", out)
        copy.value("appstore/review_notes.txt", head + "a" * (pad + 1))
        self.check(copy, fails=r"4001 of 4000 bytes")
        # each line break counts as CRLF, and a longer password limit takes more room
        copy.value("appstore/review_notes.txt", head + "a" * (pad - 1) + "\n")
        self.check(copy, fails=r"must end with exactly one newline")
        copy.value("appstore/review_notes.txt", head + "a" * (pad - 2) + "\na")
        self.check(copy, fails=r"4001 of 4000 bytes")
        copy.value("appstore/review_notes.txt", head + "a" * pad)
        self.check(copy, "--pc-password-max", str(cst.PC_PASSWORD_MAX + 1), fails=r"4001 of 4000 bytes")

    def test_the_review_notes_room_for_every_conditional_sentence(self):
        # store/README.md's conditional sentences, all at once, still fit in the worst case
        with open(os.path.join(STORE, "README.md"), encoding="utf-8") as f:
            guide = f.read()
        copy = StoreCopy(self)
        notes = self.notes(copy)
        permission = "The university's written permission to use its name and its SSO login is attached."
        login = "Login uses their university portal accounts; the app has no sign-up."
        login_48 = ("Login uses their existing university portal accounts (education accounts, guideline 4.8); "
                    "the app has no sign-up.")
        supported = "The app supports iPhone and iPad with iOS 13.0 or later."
        tested_bytes = 80  # store/README.md: " Tested on …" up to 80 bytes
        for text in (permission, login, login_48, supported, "**80바이트까지**",
                     "Screenshots of the My PCs list and the wake offer, and the attached video, show"):
            self.assertIn(text, guide + notes, text)
        for text in (login, supported):
            self.assertEqual(notes.count(text), 1, text)
        paragraphs = notes.split("\n\n")
        paragraphs[1] += " " + permission
        every = "\n\n".join(paragraphs).replace(login, login_48)
        every = every.replace(supported, supported + " Tested on " + "x" * (tested_bytes - len(" Tested on ")))
        copy.value("appstore/review_notes.txt", every)
        out = self.check(copy)
        size = int(re.search(r"review_notes.txt: (\d+) of 4000 bytes filled", out).group(1))
        self.assertLessEqual(size, cst.REVIEW_NOTES_BYTES)
        self.assertEqual(cst.PC_PASSWORD_MAX, 32, "store/README.md and the store run say 32 bytes")

    def test_the_review_notes_placeholders(self):
        for change, rule in ((lambda t: t + " {{DEMO_ACCOUNT}}", r"\{\{DEMO_ACCOUNT\}\} is not one of"),
                             (lambda t: t.replace("{{DEMO_PC_PASSWORD}}", "x"), r"\{\{DEMO_PC_PASSWORD\}\} appears 0 times"),
                             (lambda t: t + " {{DEMO_PC_ID}}", r"\{\{DEMO_PC_ID\}\} appears 2 times"),
                             (lambda t: t + " {DEMO_PC_ID}", r"a brace that is not part of a placeholder")):
            with self.subTest(rule=rule):
                copy = StoreCopy(self)
                copy.value("appstore/review_notes.txt", change(self.notes(copy)))
                self.check(copy, fails=rule)
        copy = StoreCopy(self)
        copy.value("appstore/en-GB/description.txt", copy.read("appstore/en-GB/description.txt")[:-1] + " {{DEMO_PC_ID}}")
        self.check(copy, fails=r"description.txt: placeholders belong in appstore/review_notes.txt only")

    def test_the_unlisted_sentence(self):
        copy = StoreCopy(self)
        notes = self.notes(copy)
        self.assertTrue(notes.startswith(UNLISTED + "\n\n"))
        without = notes.replace(UNLISTED + "\n\n", "")
        copy.value("appstore/review_notes.txt", without)
        self.check(copy, fails=r"distribution is unlisted but the notes lack")
        copy.value("appstore/distribution.txt", "public")
        self.check(copy)
        copy.value("appstore/review_notes.txt", notes)
        self.check(copy, fails=r"distribution is public but the notes still say")
        copy.value("appstore/distribution.txt", "private")
        self.check(copy, fails=r"neither of unlisted, public")
        copy.value("appstore/distribution.txt", "unlisted")
        copy.value("appstore/review_notes.txt", without.replace("\n\n", " " + UNLISTED + "\n\n", 1))
        self.check(copy, fails=r"must be a paragraph of its own")

    def test_the_words(self):
        cases = (
            ("appstore/ko/description.txt", " Android 앱도 있습니다.", r"other platform \(2.3.10\)"),
            ("appstore/ko/whats_new.txt", " 구글 계정은 쓰지 않습니다.", r"other platform"),
            ("appstore/en-GB/whats_new.txt", " Get it on Google Play.", r"other platform"),
            ("appstore/review_notes.txt", " The Play build is the same.", r"other platform"),
            ("appstore/en-GB/promotional_text.txt", " Beta.", r"beta word"),
            ("appstore/ko/promotional_text.txt", " 무료.", r"price"),
            ("appstore/ko/description.txt", " 시범 서비스입니다.", r"pilot notice"),
            ("play/ko-KR/release_notes.txt", " 시범 구현입니다.", r"pilot notice"),
            ("play/ko-KR/release_notes.txt", " iPhone도 같습니다.", r"Apple's platform"),
            ("appstore/en-GB/description.txt", " Open rustdesk:// links.", r"rustdesk:// link"),
            ("appstore/ko/whats_new.txt", " QR 코드로 서버를 정합니다.", r"QR"),
            ("appstore/en-GB/description.txt", " ID <PC ID>.", r"leftover <...>"),
            ("appstore/review_notes.txt", " TODO.", r"marker"),
            ("appstore/en-GB/description.txt", " Write to someone@example.org.", r"e-mail address"),
            ("play/ko-KR/release_notes.txt", " 192.0.2.1", r"IPv4 address"),
            ("appstore/en-GB/description.txt", " The colors follow the theme.", r"American spelling 'colors'"),
            ("appstore/en-GB/whats_new.txt", " 한글.", r"Hangul in an English text"),
            ("appstore/en-GB/description.txt", " Trial version, for testing only.", r"beta word"),
            ("appstore/en-GB/description.txt", " This is a demo build.", r"beta word"),
            ("appstore/en-GB/promotional_text.txt", " Early access.", r"beta word"),
            ("appstore/ko/description.txt", " 체험판입니다.", r"beta word"),
            ("appstore/ko/description.txt", " 아직 정식 서비스가 아닙니다.", r"beta word"),
            ("appstore/ko/description.txt", " 갤럭시에서도 됩니다.", r"other platform"),
            ("appstore/en-GB/description.txt", " Samsung tablets too.", r"other platform"),
            ("appstore/en-GB/description.txt", " It costs nothing.", r"price"),
            ("appstore/ko/description.txt", " 공짜입니다.", r"price"),
            ("appstore/ko/support_url.txt", "/192.0.2.10", r"IPv4 address"),
            ("appstore/en-GB/marketing_url.txt", "?to=someone@example.org", r"e-mail address"),
            ("appstore/ko/name.txt", " RustDesk", r"another app's name \(4.1\)"),
            ("appstore/en-GB/keywords.txt", ",teamviewer", r"another app's name"),
            ("appstore/ko/promotional_text.txt", " 애니데스크처럼.", r"another app's name"),
            ("appstore/en-GB/whats_new.txt", " Like AnyDesk.", r"another app's name"),
            ("appstore/ko/description.txt", " RustDesk를 바탕으로 합니다.", r"another app's name"),
            ("appstore/en-GB/description.txt", " Based on RustDesk.", r"another app's name"),
            ("appstore/en-GB/description.txt", " The license is AGPL.", r"American spelling 'license'"),
            ("appstore/en-GB/description.txt", " The catalog lists them.", r"American spelling 'catalog'"),
            ("appstore/en-GB/description.txt", " A dialog opens.", r"American spelling 'dialog'"),
            ("appstore/en-GB/description.txt", " Buttons are labeled.", r"American spelling 'labeled'"),
            ("appstore/en-GB/description.txt", " Prioritize them.", r"American spelling 'Prioritize'"),
            ("appstore/en-GB/description.txt", " Realized at once.", r"American spelling 'Realized'"),
            ("appstore/en-GB/description.txt", " Paralyzed.", r"American spelling 'Paralyzed'"),
            ("appstore/review_notes.txt", " 데모.", r"Hangul in an English text"),
        )
        for rel, extra, rule in cases:
            with self.subTest(rel=rel, extra=extra):
                copy = StoreCopy(self)
                copy.value(rel, copy.read(rel)[:-1] + extra)
                self.check(copy, fails=re.escape(rel) + r":\d+: " + rule)
        # what the rules let through: "display" (and 디스플레이), the team's Korean name, the notes'
        # words "beta" and "demo account", British -ise and the -ize words that are British too,
        # "licensed", "dialogue", "democratic"
        copy = StoreCopy(self)
        copy.value("appstore/en-GB/whats_new.txt", copy.read("appstore/en-GB/whats_new.txt")[:-1] +
                   " The display (프로젝트TF) has colours. Organise, resize and prize; seize the size."
                   " Licensed, with a dialogue and a licence, democratic.")
        copy.value("appstore/ko/description.txt", copy.read("appstore/ko/description.txt")[:-1] +
                   " 디스플레이 배율을 따릅니다.")
        copy.value("appstore/review_notes.txt", self.notes(copy) + " The terminal is no beta.")
        self.assertNotIn("RustDesk", copy.read("appstore/en-GB/description.txt"))
        self.check(copy)

    def test_the_file_form(self):
        rel = "appstore/ko/subtitle.txt"
        for raw, rule in ((b"\xef\xbb\xbfabc\n", "starts with a BOM"), (b"ab\tc\n", "tab"),
                          (b"abc \n", "trailing blank"), (b"abc\r\n", r"CR \(CRLF line ends\)"),
                          (b"abc", "must end with exactly one newline"), (b"abc\n\n", "must end with exactly one newline"),
                          (b"\xff\n", r"not UTF-8"), (b"a\x00c\n", "control or invisible"),
                          (b"a\xe2\x80\x8bc\n", "control or invisible"), (b"\nabc\n", "blank first line"),
                          (unicodedata.normalize("NFD", "원격 제어\n").encode("utf-8"), "not in NFC")):
            with self.subTest(raw=raw):
                copy = StoreCopy(self)
                copy.write(rel, raw, raw=True)
                self.check(copy, fails=re.escape(rel) + r"(:\d+)?: " + rule)
        copy = StoreCopy(self)
        os.remove(copy.path("appstore/en-GB/privacy_url.txt"))
        self.check(copy, fails=r"appstore/en-GB/privacy_url.txt: missing")
        copy = StoreCopy(self)
        copy.value("appstore/ko/whatsnew.txt", "새 기능")
        self.check(copy, fails=r"appstore/ko/whatsnew.txt: not a store file")
        copy = StoreCopy(self)
        copy.value("appstore/locales.txt", "ko\nen-GB\nen-UK")
        self.check(copy, fails=r"'en-UK' is not an App Store Connect locale code")

    def test_the_urls_and_copyright(self):
        copy = StoreCopy(self)
        copy.value("appstore/ko/marketing_url.txt", "")
        self.check(copy)
        for rel, value, rule in (("appstore/ko/support_url.txt", "", "not one https:// URL"),
                                 ("appstore/en-GB/privacy_url.txt", "http://example.org/privacy", "not one https://"),
                                 ("appstore/copyright.txt", "© 2026 프로젝트TF", "the © sign is Apple's to add"),
                                 ("appstore/copyright.txt", "프로젝트TF", "not 'YYYY holder'")):
            with self.subTest(rel=rel, value=value):
                copy = StoreCopy(self)
                copy.value(rel, value)
                self.check(copy, fails=re.escape(rel) + ": " + rule)

    def test_the_versions(self):
        copy = StoreCopy(self)
        copy.value("play/version.txt", "1.9.2")
        self.check(copy, fails=r"differ \(--allow-version-split")
        self.check(copy, "--allow-version-split")
        self.check(copy, "--allow-version-split", "--version", "1.9.2", fails=r"appstore/version.txt: is 1.9.1, not")
        copy.value("appstore/version.txt", "1.9.2")
        self.check(copy, "--version", "1.9.2")
        copy.value("appstore/version.txt", "1.9")
        self.check(copy, fails=r"not a version X.Y.Z")

    def test_the_play_notes_with_the_source_lines(self):
        copy = StoreCopy(self)
        room = cst.PLAY_NOTES_CHARS - cst.play_notes_chars("", "1.9.1")
        copy.value("play/ko-KR/release_notes.txt", "가" * room)
        self.assertIn("500 of 500 characters", self.check(copy))
        copy.value("play/ko-KR/release_notes.txt", "가" * (room + 1))
        self.check(copy, fails=r"501 of 500 characters with the CI's source lines")
        # a longer version takes from the notes' room too
        copy.value("play/ko-KR/release_notes.txt", "가" * room)
        copy.value("play/version.txt", "1.9.10")
        self.check(copy, "--allow-version-split", fails=r"501 of 500")

    def test_the_source_lines_are_the_workflows(self):
        # the Play upload step's printf: the checker counts the lines this format writes
        fmt = "\\n".join(re.sub(r"\{[a-z]+\}", "%s", line) for line in cst.PLAY_SOURCE_LINES) + "\\n"
        texts = []
        for path in sorted(glob.glob(os.path.join(ROOT, ".github", "workflows", "*.yml"))):
            with open(path, encoding="utf-8") as f:
                texts.append(f.read())
        self.assertTrue(any(f"printf '{fmt}'" in t for t in texts),
                        "no workflow writes the Play notes' source lines as PLAY_SOURCE_LINES has them")
        self.assertEqual(len(cst.PLAY_WORST["code"]), 10)  # versionCode YYDDDHHMM + 2000 (docs/ci.md)
        self.assertEqual(len(cst.PLAY_WORST["sha"]), 40)

    def test_filled_notes_print_counts_only(self):
        copy = StoreCopy(self)
        env = {"REVIEW_DEMO_PC_ID": FAKE_ID, "REVIEW_DEMO_PC_PASSWORD": FAKE_PASSWORD}
        out = self.check(copy, "--filled", env=env)
        filled = cst.fill(self.notes(copy), {"DEMO_PC_ID": FAKE_ID, "DEMO_PC_PASSWORD": FAKE_PASSWORD})
        self.assertIn(f"{cst.notes_bytes(filled)} of 4000 bytes as filled from the environment", out)
        self.assert_nothing_of(out, filled)
        # a too long value fails, and its error does not show it either
        long_password = FAKE_PASSWORD * 3
        out = self.check(copy, "--filled", env=dict(env, REVIEW_DEMO_PC_PASSWORD=long_password),
                         fails=rf"REVIEW_DEMO_PC_PASSWORD is longer than {cst.PC_PASSWORD_MAX} bytes")
        self.assert_nothing_of(out, cst.fill(self.notes(copy), {"DEMO_PC_ID": FAKE_ID,
                                                                "DEMO_PC_PASSWORD": long_password}))
        self.assertNotIn(long_password, out)
        with mock.patch.dict(os.environ, {}, clear=False):
            os.environ.pop("REVIEW_DEMO_PC_PASSWORD", None)
            os.environ.pop("REVIEW_DEMO_PC_ID", None)
            self.check(copy, "--filled", fails=r"--filled needs REVIEW_DEMO_PC_ID")

    def assert_nothing_of(self, out, filled):
        self.assertNotIn(FAKE_ID, out)
        self.assertNotIn(FAKE_PASSWORD, out)
        for line in filled.split("\n"):
            if FAKE_ID in line or FAKE_PASSWORD in line:
                self.assertNotIn(line, out)
        for algo in ("md5", "sha1", "sha256", "sha512", "blake2b"):
            for text in (filled, filled.replace("\n", "\r\n")):
                self.assertNotIn(hashlib.new(algo, text.encode("utf-8")).hexdigest(), out)
        self.assertIsNone(re.search(r"[0-9a-f]{32,}", out), "nothing that looks like a hash")

    def test_testflight_texts_when_external_testing_is_on(self):
        copy = StoreCopy(self)
        copy.value("appstore/testflight/what_to_test.ko.txt", "계정 삭제를 봐 주세요.")
        self.check(copy)
        copy.value("appstore/testflight/what_to_test.ko.txt", "Android도 봐 주세요.")
        self.check(copy, fails=r"what_to_test.ko.txt:1: other platform")
        copy = StoreCopy(self)
        copy.value("appstore/testflight/what_to_test.fr.txt", "Bonjour.")
        self.check(copy, fails=r"what_to_test.fr.txt: not a store file")

    def test_the_command(self):
        copy = StoreCopy(self)
        run = subprocess.run([sys.executable, "-I", SCRIPT, "--store", copy.store], capture_output=True, text=True)
        self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
        self.assertTrue(run.stdout.rstrip().endswith("store texts: ok"))
        copy.value("appstore/ko/name.txt", "가" * 31)
        run = subprocess.run([sys.executable, "-I", SCRIPT, "--store", copy.store], capture_output=True, text=True)
        self.assertEqual(run.returncode, 1, run.stdout + run.stderr)
        self.assertIn("store texts: 1 failures", run.stdout)
        run = subprocess.run([sys.executable, "-I", SCRIPT, "--store", os.path.join(copy.dir, "none")],
                             capture_output=True, text=True)
        self.assertEqual(run.returncode, 1)


if __name__ == "__main__":
    unittest.main()

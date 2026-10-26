#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""The store texts of store/ (store/README.md; auto-deploy design 2-3), as the App Store and
Google Play show them, checked before anything sends them:

    python3 -I scripts/check-store-text.py [--store DIR] [--version V] [--allow-version-split]
                                           [--pc-id-max N] [--pc-password-max N] [--filled]

  files       the files of store/README.md and no others; UTF-8 without a BOM, in NFC, without tabs,
              CR, control characters, trailing blanks, blank first or last lines; one final newline
  versions    appstore/version.txt and play/version.txt are X.Y.Z and the same (--allow-version-split
              lets them differ once the stores' versions part); --version V: each must be V
  App Store   per locale of appstore/locales.txt (App Store Connect codes): name 2-30 characters,
              subtitle, promotional text 170, description 4000, keywords 100 UTF-8 bytes (commas, no
              empty or repeated keyword, no blank around a comma), What's New 4000; https URLs, the
              marketing URL may be empty; copyright "YYYY holder" without the sign Apple adds
  review      appstore/review_notes.txt: no placeholders but {{DEMO_PC_ID}} and {{DEMO_PC_PASSWORD}},
              each once; 4000 bytes once filled with the worst case (ASCII values of --pc-id-max and
              --pc-password-max bytes, every line break counted as CRLF); with appstore/distribution.txt
              "unlisted" it holds Apple's sentence "This app is intended for unlisted distribution." as a
              paragraph, and with "public" it does not
  Play        play/ko-KR/release_notes.txt with the lines the CI adds after it (PLAY_SOURCE_LINES:
              the version, the versionCode and the source link of the build) in 500 characters
  words       iOS texts never name the other platform's store (guideline 2.3.10: Android, Google,
              Play); Play texts never name Apple's; no text has a pilot notice, rustdesk:// links,
              QR, a leftover <...> or TODO, an e-mail or IPv4 address (URLs included); the App Store's
              own texts no beta, trial or demo words and no prices, and their name, subtitle,
              keywords, promotional text and What's New no other app's name (4.1); the English
              (en-GB) texts British spelling (-ise) and no Hangul but 프로젝트TF

--filled (the store run): also fills the notes with REVIEW_DEMO_PC_ID and REVIEW_DEMO_PC_PASSWORD
from the environment and counts those bytes. Whatever it checks, the script prints file names,
line numbers, rule names and counts, never a line of the filled notes, a value or a hash of them:
the template is public, so a hash would let a reader of the log guess a short demo PC password.
"""
import argparse
import os
import re
import sys
import unicodedata

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
STORE = os.path.join(ROOT, "store")

# App Store Connect's limits (characters unless bytes); the name needs at least 2
APPSTORE_LOCALE_FILES = {
    "name.txt": ("chars", 30),
    "subtitle.txt": ("chars", 30),
    "promotional_text.txt": ("chars", 170),
    "description.txt": ("chars", 4000),
    "keywords.txt": ("bytes", 100),
    "whats_new.txt": ("chars", 4000),
    "support_url.txt": ("url", None),
    "marketing_url.txt": ("url-or-empty", None),
    "privacy_url.txt": ("url", None),
}
REVIEW_NOTES_BYTES = 4000
PLAY_NOTES_CHARS = 500
PLAY_LOCALES = ("ko-KR",)
# the lines the CI puts after play/ko-KR/release_notes.txt: the printf of the Play upload step in
# .github/workflows (client-build.yml: 'HJViewer %s (%s)\n소스 코드(AGPL-3.0): https://github.com/%s/tree/%s\n'),
# counted with the longest values it can take (scripts/test_check_store_text.py pins the format)
PLAY_SOURCE_LINES = ("HJViewer {version} ({code})", "소스 코드(AGPL-3.0): https://github.com/{repo}/tree/{sha}")
# the repository is the public one with a source_ref, else the calling (private build) repository:
# the longer of the two
PLAY_REPOS = ("yonsei-projecttf/hjviewer", "yonsei-projecttf/hjviewer-private")
PLAY_WORST = {"code": "9" * 10, "repo": max(PLAY_REPOS, key=len), "sha": "f" * 40}
# worst cases of the filled notes: a RustDesk ID has 6-16 characters (hbbs), and the demo PC's
# permanent password is long but bounded (design 4-3: 16 or more; store/README.md: 16-32); the
# store run refuses longer values (--filled)
PC_ID_MAX = 16
PC_PASSWORD_MAX = 32
PLACEHOLDERS = ("DEMO_PC_ID", "DEMO_PC_PASSWORD")
FILL_ENV = {"DEMO_PC_ID": "REVIEW_DEMO_PC_ID", "DEMO_PC_PASSWORD": "REVIEW_DEMO_PC_PASSWORD"}
UNLISTED_SENTENCE = "This app is intended for unlisted distribution."
DISTRIBUTIONS = ("unlisted", "public")
# App Store Connect's localization codes
ASC_LOCALES = frozenset((
    "ar-SA", "bn-BD", "ca", "cs", "da", "de-DE", "el", "en-AU", "en-CA", "en-GB", "en-US", "es-ES",
    "es-MX", "fi", "fr-CA", "fr-FR", "gu-IN", "he", "hi", "hr", "hu", "id", "it", "ja", "kn-IN", "ko",
    "ml-IN", "mr-IN", "ms", "nl-NL", "no", "or-IN", "pa-IN", "pl", "pt-BR", "pt-PT", "ro", "ru", "sk",
    "sl-SI", "sv", "ta-IN", "te-IN", "th", "tr", "uk", "ur-PK", "vi", "zh-Hans", "zh-Hant"))
TESTFLIGHT = re.compile(r"appstore/testflight/(what_to_test|beta_description)\.([A-Za-z-]+)\.txt")
VERSION = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+")

# every text: no pilot notice (1.9.1 is the official service), no rustdesk:// (1.9.1 takes
# hjviewer:// alone), no QR page (gone in 1.9.1), nothing left to fill, no addresses
ALL_RULES = {
    "pilot notice": re.compile(r"시범|pilot", re.I),
    "rustdesk:// link": re.compile(r"rustdesk://", re.I),
    "QR": re.compile(r"\bQR\b|QR ?코드", re.I),
    "leftover <...>": re.compile(r"<[^<>\n]{0,80}>"),
    "marker": re.compile(r"\b(?:TODO|TBD|FIXME|XXX)\b|lorem|ipsum|\?\?\?"),
    "e-mail address": re.compile(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+"),
    "IPv4 address": re.compile(r"\b(?:\d{1,3}\.){3}\d{1,3}\b"),
}
# guideline 2.3.10: the App Store's texts (review notes included) never name another platform
# ("플레이" but not "디스플레이", display)
IOS_RULES = {"other platform (2.3.10)": re.compile(
    r"android|google|\bplay\b|galaxy|samsung|안드로이드|구글|(?<!디스)플레이|갤럭시|삼성", re.I)}
PLAY_RULES = {"Apple's platform": re.compile(r"iphone|ipad|\bios\b|app ?store|testflight|아이폰|아이패드|애플", re.I)}
# what customers read in the App Store (guideline 2.2: no beta, trial or demo; 2.3.7: no prices)
BETA = re.compile(r"베타|\bbeta\b|preview|미리 ?보기|test build|testflight|\btrial\b|\bdemos?\b(?! account)|"
                  r"experimental|early access|체험판|시험판|테스트 ?버전|데모|정식 (?:서비스|버전|출시)\S* ?아[니닙닌님]", re.I)
PRICE = re.compile(r"무료|유료|공짜|\bfree\b|가격|\bprices?\b|costs? nothing|no (?:charge|cost)", re.I)
# guideline 4.1 (metadata): no other app's name in these fields; the description's open-source
# paragraph names RustDesk until the owner decides SUBMIT-1.9.0 0-2 #25 (store/README.md)
OTHER_APPS = re.compile(r"rust ?desk|team ?viewer|any ?desk|chrome remote desktop|splashtop|parsec|"
                        r"러스트 ?데스크|팀 ?뷰어|애니 ?데스크", re.I)
OTHER_APPS_IN = ("name.txt", "subtitle.txt", "keywords.txt", "promotional_text.txt", "whats_new.txt")
CUSTOMER_TEXTS = ("name.txt", "subtitle.txt", "promotional_text.txt", "description.txt", "keywords.txt",
                  "whats_new.txt")
# en-GB: SUBMIT-1.9.0 0-2 #4 (the Korean App Store's English is English (U.K.))
# (-ise, not -ize, as in SUBMIT's English: organise, recognise; licence the noun, license the verb)
AMERICAN = re.compile(r"\b(colors?|colored|favorites?|favorited|centers?|centered|behaviors?|gray|"
                      r"canceled|canceling|licenses?|catalogs?|dialogs?|labeled|labeling|traveled|"
                      r"traveling|modeled|modeling|enrolls?|enrollment|fulfills?|fulfillment|"
                      r"[a-z]{3,}iz(?:e|ed|es|ing|ation|ations)|[a-z]{3,}yz(?:e|ed|es|ing))\b", re.I)
# -ize words that are British too (size, prize, seize, …)
AMERICAN_OK = re.compile(r"(?:size|sized|sizes|sizing|prize|prized|prizes|seize|seized|seizes|seizing|"
                         r"maize|baize)$", re.I)
HANGUL = re.compile(r"[\u1100-\u11ff\u3130-\u318f\uac00-\ud7a3]+")
ENGLISH_HANGUL_ALLOWED = ("프로젝트TF",)  # the team's own name once, in parentheses
CONTROL = re.compile(r"[\x00-\x08\x0b-\x1f\x7f\u200b-\u200f\u2028\u2029\u202a-\u202e\u2060\ufeff]")


class Report:
    def __init__(self, quiet=False):
        self.failures = 0
        self.quiet = quiet  # for a second read of a file whose own check reports it

    def fail(self, where, what):
        self.failures += 1
        if not self.quiet:
            print(f"  FAIL {where}: {what}")

    @staticmethod
    def ok(what):
        print(f"  ok   {what}")


def expected_files(store):
    """The paths (relative to store/) that store/README.md lists, from its locales."""
    paths = ["README.md", "appstore/version.txt", "appstore/distribution.txt", "appstore/locales.txt",
             "appstore/copyright.txt", "appstore/review_notes.txt", "play/version.txt"]
    paths += [f"play/{loc}/release_notes.txt" for loc in PLAY_LOCALES]
    for loc in read_locales(store) or ():
        paths += [f"appstore/{loc}/{name}" for name in APPSTORE_LOCALE_FILES]
    return paths


def read_locales(store):
    try:
        with open(os.path.join(store, "appstore", "locales.txt"), encoding="utf-8") as f:
            return [line.strip() for line in f.read().split("\n") if line.strip()]
    except (OSError, UnicodeDecodeError):
        return None


def present_files(store):
    out = []
    for top, dirs, files in os.walk(store):
        dirs.sort()
        for name in sorted(files):
            out.append(os.path.relpath(os.path.join(top, name), store).replace(os.sep, "/"))
    return out


def load(store, rel, report):
    """The value of a store file: its text without the one final newline, after the file checks."""
    path = os.path.join(store, rel)
    try:
        with open(path, "rb") as f:
            raw = f.read()
    except OSError as e:
        report.fail(rel, f"cannot read it ({e.strerror})")
        return None
    if raw.startswith(b"\xef\xbb\xbf"):
        report.fail(rel, "starts with a BOM")
        raw = raw[3:]
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError as e:
        report.fail(rel, f"not UTF-8 (byte {e.start})")
        return None
    if unicodedata.normalize("NFC", text) != text:
        report.fail(rel, "not in NFC (Korean text decomposed into jamo, as macOS can save it)")
    if not text.endswith("\n") or text.endswith("\n\n"):
        report.fail(rel, "must end with exactly one newline")
    lines = text.split("\n")
    if lines and lines[-1] == "":
        lines.pop()
    for n, line in enumerate(lines, 1):
        if "\t" in line:
            report.fail(f"{rel}:{n}", "tab")
        if "\r" in line:
            report.fail(f"{rel}:{n}", "CR (CRLF line ends)")
        elif CONTROL.search(line):
            report.fail(f"{rel}:{n}", "control or invisible character")
        if line != line.rstrip():
            report.fail(f"{rel}:{n}", "trailing blank")
    if len(lines) > 1 and not lines[0].strip() and rel != "README.md":
        report.fail(rel, "blank first line")
    return text[:-1] if text.endswith("\n") else text


def check_words(rel, value, rules, report):
    for n, line in enumerate(value.split("\n"), 1):
        for rule, rx in rules.items():
            if rx.search(line):
                report.fail(f"{rel}:{n}", rule)


def check_english(rel, value, report):
    for n, line in enumerate(value.split("\n"), 1):
        for m in AMERICAN.finditer(line):
            if AMERICAN_OK.search(m.group(0)):
                continue
            report.fail(f"{rel}:{n}", f"American spelling '{m.group(0)}' in an en-GB text")
        rest = line
        for allowed in ENGLISH_HANGUL_ALLOWED:
            rest = rest.replace(allowed, "")
        if HANGUL.search(rest):
            report.fail(f"{rel}:{n}", "Hangul in an English text")


def check_url(rel, value, may_be_empty, report):
    if value == "" and may_be_empty:
        return
    if not re.fullmatch(r"https://[^\s<>\"']+", value):
        report.fail(rel, "not one https:// URL")


def check_keywords(rel, value, report):
    words = value.split(",")
    if any(w != w.strip() for w in words):
        report.fail(rel, "blank around a comma (it costs bytes)")
    stripped = [w.strip() for w in words]
    if any(not w for w in stripped):
        report.fail(rel, "empty keyword")
    lowered = [w.casefold() for w in stripped if w]
    if len(set(lowered)) != len(lowered):
        report.fail(rel, "repeated keyword")


def fill(template, values):
    out = template
    for name, value in values.items():
        out = out.replace("{{" + name + "}}", value)
    return out


def notes_bytes(text):
    """Bytes in App Store Connect's Notes field, counting every line break as CRLF."""
    return len(text.encode("utf-8")) + text.count("\n")


def check_review_notes(store, args, distribution, report):
    rel = "appstore/review_notes.txt"
    template = load(store, rel, report)
    if template is None:
        return
    check_words(rel, template, {**ALL_RULES, **IOS_RULES}, report)
    check_english(rel, template, report)
    if not template.isascii():
        print(f"  note {rel}: has characters outside ASCII (2-3 bytes each)")
    names = re.findall(r"\{\{([^{}]*)\}\}", template)
    for name in sorted(set(names) - set(PLACEHOLDERS)):
        report.fail(rel, f"placeholder {{{{{name}}}}} is not one of {', '.join(PLACEHOLDERS)}")
    for name in PLACEHOLDERS:
        if names.count(name) != 1:
            report.fail(rel, f"{{{{{name}}}}} appears {names.count(name)} times, not once")
    rest = re.sub(r"\{\{[^{}]*\}\}", "", template)
    if "{" in rest or "}" in rest:
        report.fail(rel, "a brace that is not part of a placeholder")
    worst = fill(template, {"DEMO_PC_ID": "x" * args.pc_id_max, "DEMO_PC_PASSWORD": "x" * args.pc_password_max})
    size = notes_bytes(worst)
    line = (f"{rel}: {size} of {REVIEW_NOTES_BYTES} bytes filled with the worst case "
            f"(ID {args.pc_id_max}, password {args.pc_password_max} bytes, CRLF line breaks)")
    report.ok(line) if size <= REVIEW_NOTES_BYTES else report.fail(rel, line)
    if distribution == "unlisted":
        paragraphs = [p.strip() for p in template.split("\n\n")]
        if UNLISTED_SENTENCE in paragraphs:
            report.ok(f"{rel}: Apple's unlisted sentence is a paragraph of its own")
        elif UNLISTED_SENTENCE in template:
            report.fail(rel, "Apple's unlisted sentence must be a paragraph of its own")
        else:
            report.fail(rel, f"distribution is unlisted but the notes lack \"{UNLISTED_SENTENCE}\"")
    elif UNLISTED_SENTENCE in template:
        report.fail(rel, f"distribution is {distribution} but the notes still say \"{UNLISTED_SENTENCE}\"")
    if args.filled:
        check_filled(rel, template, args, report)


def check_filled(rel, template, args, report):
    """The notes as the store run sends them. Prints the byte count only (module docstring)."""
    values = {}
    for name, env in FILL_ENV.items():
        value = os.environ.get(env, "")
        if not value:
            report.fail(rel, f"--filled needs {env} in the environment")
            return
        limit = args.pc_id_max if name == "DEMO_PC_ID" else args.pc_password_max
        if len(value.encode("utf-8")) > limit:
            report.fail(rel, f"{env} is longer than {limit} bytes")
        if not value.isascii() or not value.isprintable() or "{" in value or "}" in value:
            report.fail(rel, f"{env} must be printable ASCII without braces")
        values[name] = value
    size = notes_bytes(fill(template, values))
    line = f"{rel}: {size} of {REVIEW_NOTES_BYTES} bytes as filled from the environment"
    report.ok(line) if size <= REVIEW_NOTES_BYTES else report.fail(rel, line)


def check_versions(store, args, report):
    found = {}
    for which in ("appstore", "play"):
        rel = f"{which}/version.txt"
        value = load(store, rel, report)
        if value is None:
            continue
        if not VERSION.fullmatch(value):
            report.fail(rel, "not a version X.Y.Z")
            continue
        found[which] = value
        if args.version and value != args.version:
            report.fail(rel, f"is {value}, not --version {args.version}")
    if len(found) == 2:
        if found["appstore"] == found["play"]:
            report.ok(f"versions: both {found['appstore']}")
        elif args.allow_version_split:
            report.ok(f"versions: App Store {found['appstore']}, Play {found['play']} (split allowed)")
        else:
            report.fail("appstore/version.txt, play/version.txt", "differ (--allow-version-split when they should)")
    return found.get("play")


def check_appstore(store, args, report):
    print("App Store")
    distribution = load(store, "appstore/distribution.txt", report)
    if distribution is not None and distribution not in DISTRIBUTIONS:
        report.fail("appstore/distribution.txt", f"neither of {', '.join(DISTRIBUTIONS)}")
    locales = read_locales(store)
    if not locales:
        report.fail("appstore/locales.txt", "no locale")
        locales = []
    load(store, "appstore/locales.txt", report)
    for loc in locales:
        if loc not in ASC_LOCALES:
            report.fail("appstore/locales.txt", f"'{loc}' is not an App Store Connect locale code")
    if len(set(locales)) != len(locales):
        report.fail("appstore/locales.txt", "a locale twice")
    copyright_ = load(store, "appstore/copyright.txt", report)
    if copyright_ is not None:
        check_words("appstore/copyright.txt", copyright_, {**ALL_RULES, **IOS_RULES}, report)
        if "©" in copyright_ or "(c)" in copyright_.lower():
            report.fail("appstore/copyright.txt", "the © sign is Apple's to add")
        if not re.fullmatch(r"[0-9]{4} \S.*", copyright_):
            report.fail("appstore/copyright.txt", "not 'YYYY holder'")
    for loc in locales:
        for name, (kind, limit) in APPSTORE_LOCALE_FILES.items():
            rel = f"appstore/{loc}/{name}"
            value = load(store, rel, report)
            if value is None:
                continue
            if kind.startswith("url"):
                check_url(rel, value, kind == "url-or-empty", report)
                check_words(rel, value, {k: ALL_RULES[k] for k in ("e-mail address", "IPv4 address")}, report)
                continue
            check_words(rel, value, {**ALL_RULES, **IOS_RULES}, report)
            if name in CUSTOMER_TEXTS:
                check_words(rel, value, {"beta word (2.2)": BETA, "price (2.3.7)": PRICE}, report)
            if name in OTHER_APPS_IN:
                check_words(rel, value, {"another app's name (4.1)": OTHER_APPS}, report)
            if loc.startswith("en-GB"):
                check_english(rel, value, report)
            if name in ("name.txt", "subtitle.txt", "keywords.txt") and "\n" in value:
                report.fail(rel, "more than one line")
            if name == "keywords.txt":
                check_keywords(rel, value, report)
            size = len(value.encode("utf-8")) if kind == "bytes" else len(value)
            unit = "bytes" if kind == "bytes" else "characters"
            least = 2 if name == "name.txt" else (1 if name in ("description.txt", "keywords.txt") else 0)
            line = f"{rel}: {size} of {limit} {unit}"
            if size > limit:
                report.fail(rel, line)
            elif size < least:
                report.fail(rel, f"{size} {unit}, needs at least {least}")
            else:
                report.ok(line)
    check_review_notes(store, args, distribution, report)


def play_notes_chars(notes, version):
    worst = dict(PLAY_WORST, version=version)
    return len(notes) + sum(1 + len(line.format(**worst)) for line in PLAY_SOURCE_LINES)


def check_play(store, version, report):
    print("Google Play")
    for loc in PLAY_LOCALES:
        rel = f"play/{loc}/release_notes.txt"
        value = load(store, rel, report)
        if value is None:
            continue
        check_words(rel, value, {**ALL_RULES, **PLAY_RULES}, report)
        if not value.strip():
            report.fail(rel, "empty")
        size = play_notes_chars(value, version or "999.999.999")
        line = f"{rel}: {size} of {PLAY_NOTES_CHARS} characters with the CI's source lines"
        report.ok(line) if size <= PLAY_NOTES_CHARS else report.fail(rel, line)


def check(store, args):
    report = Report()
    print(f"store texts in {store}")
    print("files")
    expected = expected_files(store)
    present = present_files(store)
    for rel in expected:
        if rel not in present:
            report.fail(rel, "missing")
    languages = {loc.split("-")[0] for loc in read_locales(store) or ()}
    for rel in present:
        if rel in expected:
            continue
        m = TESTFLIGHT.fullmatch(rel)
        if m and m.group(2) in languages:
            value = load(store, rel, report)  # external TestFlight's texts (README.md), when it is on
            if value is not None:
                check_words(rel, value, {**ALL_RULES, **IOS_RULES}, report)
            continue
        report.fail(rel, "not a store file (store/README.md lists them)")
    for rel in present:
        value = load(store, rel, report if rel == "README.md" else Report(quiet=True))
        if value is None:
            continue
        if rel == "README.md":
            # the guide names the notes' two placeholders; addresses never go in store/
            check_words(rel, value, {k: ALL_RULES[k] for k in ("e-mail address", "IPv4 address")}, report)
            value = value.replace("{{DEMO_PC_ID}}", "").replace("{{DEMO_PC_PASSWORD}}", "")
        if rel != "appstore/review_notes.txt" and ("{{" in value or "}}" in value):
            report.fail(rel, "placeholders belong in appstore/review_notes.txt only")
    print("versions")
    version = check_versions(store, args, report)
    check_appstore(store, args, report)
    check_play(store, version, report)
    if report.failures:
        print(f"store texts: {report.failures} failures")
        return 1
    print("store texts: ok")
    return 0


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--store", default=STORE, help="the store/ directory (default: the repository's)")
    ap.add_argument("--version", help="the version the store run sends; both version files must be it")
    ap.add_argument("--allow-version-split", action="store_true",
                    help="appstore/version.txt and play/version.txt may differ")
    ap.add_argument("--pc-id-max", type=int, default=PC_ID_MAX, help="bytes of the longest demo PC ID")
    ap.add_argument("--pc-password-max", type=int, default=PC_PASSWORD_MAX,
                    help="bytes of the longest demo PC password")
    ap.add_argument("--filled", action="store_true",
                    help="also count the notes filled from REVIEW_DEMO_PC_ID and REVIEW_DEMO_PC_PASSWORD")
    args = ap.parse_args(argv)
    if args.version is not None and not VERSION.fullmatch(args.version):
        ap.error("--version takes X.Y.Z")
    if not os.path.isdir(args.store):
        print(f"store texts: no directory {args.store}")
        return 1
    return check(os.path.abspath(args.store), args)


if __name__ == "__main__":
    sys.exit(main())

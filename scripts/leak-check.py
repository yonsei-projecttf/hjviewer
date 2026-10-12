#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""The push gate of design v3 11 steps 1-2 (scripts/ci.sh leak): finds the values of a local list
in what a push would publish, without ever printing them.

    python3 -I scripts/leak-check.py [--patterns FILE] [--repo DIR] [--base REF]

The list (default build/leak-patterns.txt, which git ignores; a linked worktree without one uses
its main worktree's; HJV_LEAK_PATTERNS names another file) is UTF-8 text, with or without a BOM,
with one fixed string per line: the production domain, the university's public address prefix,
the NAS and PC addresses, the PC's MAC address, the department's shared folder name, real portal
IDs. '#' starts a comment, also after a value; blank lines are skipped, and so are the spaces
around a value. A MAC address (12 hex digits, with ':', '-' or '.' between them or none) also
matches in the other three forms. Matching ignores ASCII case, and Korean text matches in both of
its Unicode forms (NFC and NFD). A list that is not UTF-8 (UTF-16, as Windows PowerShell 5.1
writes with '>' or Out-File), or whose values hold control or invisible characters, stops the
check: such a value would match nothing.

Searched: every file git tracks, as it is in the working tree (client/patches and server/patches
included), and its path; the staged version of staged files; and of the commits a push would
publish (REF..HEAD, default origin/main..HEAD; every commit when REF does not exist) their
messages, authors, committers, the paths of the files they add or change, and their added lines.
A hit prints where it is and the list's line number, never the value or the line (a path that
holds a value is printed with "<listed value>" in its place), and the check fails. It also fails
when the list is missing, has no value, or is itself tracked by git.
"""
import argparse
import os
import re
import subprocess
import sys
import unicodedata

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
# a MAC address: six pairs of hex digits with one separator, three dotted quads, or 12 digits
MAC = re.compile(r"[0-9a-f]{2}([:.-])[0-9a-f]{2}(?:\1[0-9a-f]{2}){4}|[0-9a-f]{4}([:.-])[0-9a-f]{4}\2[0-9a-f]{4}"
                 r"|[0-9a-f]{12}")
LIST_HELP = "docs/ci.md, 'push 전 검사'"


def git(repo, *args, check=True):
    done = subprocess.run(["git", "-C", repo, *args], capture_output=True, check=False)
    if check and done.returncode != 0:
        raise SystemExit(f"leak: git {args[0]} failed: {done.stderr.decode(errors='replace').strip()}")
    return done


def default_patterns(repo):
    env = os.environ.get("HJV_LEAK_PATTERNS")
    if env:
        return env
    own = os.path.join(repo, "build", "leak-patterns.txt")
    if os.path.exists(own):
        return own
    # a linked worktree (build/wt/...): the main worktree's list
    common = git(repo, "rev-parse", "--path-format=absolute", "--git-common-dir", check=False)
    if common.returncode == 0:
        main = os.path.dirname(common.stdout.decode().strip())
        other = os.path.join(main, "build", "leak-patterns.txt")
        if os.path.exists(other):
            return other
    return own


def mac_forms(value):
    """The four ways of writing the MAC address <value> (lowercase), or []."""
    if not MAC.fullmatch(value):
        return []
    digits = re.sub(r"[:.-]", "", value)
    pairs = [digits[i:i + 2] for i in range(0, 12, 2)]
    return [":".join(pairs), "-".join(pairs), digits, ".".join(digits[i:i + 4] for i in (0, 4, 8))]


def read_patterns(path):
    """[(list line number, [lowercased byte strings to find])]"""
    try:
        with open(path, "rb") as f:
            data = f.read()
    except FileNotFoundError:
        raise SystemExit(f"leak: no list at {path}: create it ({LIST_HELP})")
    # UTF-16 (PowerShell 5.1's '>' and Out-File) puts a NUL next to every ASCII character: as
    # UTF-8 its values would match nothing
    if data.startswith((b"\xff\xfe", b"\xfe\xff")) or b"\0" in data:
        raise SystemExit(f"leak: {path} is UTF-16 or holds NUL bytes: save it as UTF-8 ({LIST_HELP})")
    try:
        text = data.decode("utf-8-sig")  # a BOM (Notepad's "UTF-8 with BOM") is no part of a value
    except UnicodeDecodeError as e:
        raise SystemExit(f"leak: {path} is not UTF-8 (byte {e.start}): save it as UTF-8 ({LIST_HELP})")
    patterns = []
    for n, line in enumerate(text.splitlines(), 1):
        value = line.split("#", 1)[0].strip()
        if not value:
            continue
        odd = [c for c in value if c != " " and (c.isspace() or unicodedata.category(c)[0] in "CZ")]
        if odd:
            raise SystemExit(f"leak: {path} line {n}: the value holds U+{ord(odd[0]):04X}, a control, "
                             f"invisible or unusual space character, and would match nothing: retype it")
        forms = {value, unicodedata.normalize("NFC", value), unicodedata.normalize("NFD", value)}
        forms.update(mac_forms(value.lower()))
        # ASCII case only, as in the files searched (bytes.lower)
        patterns.append((n, sorted({f.encode("utf-8").lower() for f in forms})))
    if not patterns:
        raise SystemExit(f"leak: {path} lists no value")
    return patterns


def hits_in(data, patterns):
    """[(line number, list line)] of the patterns in data (bytes)."""
    low = data.lower()
    found = []
    for n, forms in patterns:
        if not any(f in low for f in forms):
            continue
        for i, line in enumerate(low.split(b"\n"), 1):
            if any(f in line for f in forms):
                found.append((i, n))
    return found


def is_binary(data):
    return b"\0" in data[:8000]


class Scan:
    def __init__(self, repo, patterns):
        self.repo = repo
        self.patterns = patterns
        self.hits = 0

    def report(self, where, list_line):
        self.hits += 1
        print(f"{where}: list line {list_line}", flush=True)

    def shown(self, raw):
        """A path (bytes) for printing: every listed value in it replaced by "<listed value>"."""
        low = raw.lower()
        covered = bytearray(len(raw))
        for _, forms in self.patterns:
            for form in forms:
                at = low.find(form)
                while at >= 0:
                    covered[at:at + len(form)] = b"\1" * len(form)
                    at = low.find(form, at + 1)
        out, start = [], 0
        for i in range(1, len(raw) + 1):
            if i == len(raw) or covered[i] != covered[start]:
                out.append("<listed value>" if covered[start] else raw[start:i].decode(errors="replace"))
                start = i
        return "".join(out)

    def path(self, label, raw):
        """A path that a push publishes: the values in its name."""
        for n in sorted({n for _, n in hits_in(raw, self.patterns)}):
            self.report(f"{label}{self.shown(raw)} (in the path)", n)

    def blob(self, label, data):
        found = hits_in(data, self.patterns)
        if found and is_binary(data):
            for n in sorted({n for _, n in found}):
                self.report(f"{label} (binary)", n)
            return
        for line, n in found:
            self.report(f"{label}:{line}", n)

    def tracked(self):
        files = [p for p in git(self.repo, "ls-files", "-z").stdout.split(b"\0") if p]
        for raw in files:
            self.path("", raw)
            rel = raw.decode(errors="surrogateescape")
            path = os.path.join(self.repo, rel)
            if os.path.islink(path):
                data = os.readlink(path).encode(errors="surrogateescape")
            elif os.path.isfile(path):
                with open(path, "rb") as f:
                    data = f.read()
            else:
                continue  # deleted in the working tree, or a submodule
            self.blob(self.shown(raw), data)
        return len(files)

    def staged(self):
        names = git(self.repo, "diff", "--cached", "--name-only", "-z", "--diff-filter=ACMRT").stdout
        for raw in [p for p in names.split(b"\0") if p]:
            rel = raw.decode(errors="surrogateescape")
            self.blob(f"staged {self.shown(raw)}", git(self.repo, "show", f":{rel}").stdout)

    def commits(self, base):
        if git(self.repo, "rev-parse", "--verify", "--quiet", "HEAD", check=False).returncode != 0:
            return 0  # no commit yet
        if git(self.repo, "rev-parse", "--verify", "--quiet", f"{base}^{{commit}}", check=False).returncode == 0:
            span = [f"{base}..HEAD"]
        else:
            print(f"leak: {base} does not exist: checking every commit of HEAD", flush=True)
            span = ["HEAD"]
        log = git(self.repo, "log", "--format=%H%x00%an <%ae>%x00%cn <%ce>%x00%B%x00%x01", *span).stdout
        count = 0
        for entry in log.split(b"\x01"):
            entry = entry.lstrip(b"\n")
            if not entry:
                continue
            sha, author, committer, message = entry.split(b"\0")[:4]
            short = sha.decode()[:10]
            count += 1
            for line, n in hits_in(message, self.patterns):
                self.report(f"commit {short}: message line {line}", n)
            for who, value in (("author", author), ("committer", committer)):
                for _, n in hits_in(value, self.patterns):
                    self.report(f"commit {short}: {who}", n)
            # the paths it adds or changes, also those of files a later commit removes or renames
            names = git(self.repo, "show", "--format=", "--name-only", "-z", "--no-renames",
                        "--diff-merges=first-parent", "--diff-filter=d", sha.decode()).stdout
            for raw in [p for p in names.split(b"\0") if p]:
                self.path(f"commit {short}: ", raw)
            self.added_lines(sha.decode(), short)
        return count

    def added_lines(self, sha, short):
        """The lines a commit adds against its first parent, binary files as text."""
        diff = git(self.repo, "-c", "core.quotepath=off", "show", "--format=", "--unified=0", "--text",
                   "--no-color", "--no-ext-diff", "--diff-merges=first-parent", "--no-renames", sha).stdout
        path, line, header = None, 0, False
        for raw in diff.split(b"\n"):
            if raw.startswith(b"diff --git "):
                path, header = None, True
            elif header and raw.startswith(b"+++ "):
                target = raw[4:]
                path = None if target == b"/dev/null" else self.shown(target[2:])
            elif raw.startswith(b"@@"):
                header = False
                m = re.match(rb"@@ -\d+(?:,\d+)? \+(\d+)", raw)
                line = int(m.group(1)) if m else 0
            elif not header and raw.startswith(b"+") and path is not None:
                for _, n in hits_in(raw[1:], self.patterns):
                    self.report(f"commit {short} {path}:{line}", n)
                line += 1


def main(argv):
    parser = argparse.ArgumentParser(description="Finds the values of a local list in what a push publishes.")
    parser.add_argument("--repo", default=ROOT)
    parser.add_argument("--patterns")
    parser.add_argument("--base", default="origin/main")
    args = parser.parse_args(argv)
    repo = os.path.abspath(args.repo)
    path = os.path.abspath(args.patterns or default_patterns(repo))
    patterns = read_patterns(path)
    rel = os.path.relpath(path, repo)
    if not rel.startswith(".."):
        if git(repo, "ls-files", "--error-unmatch", "--", rel, check=False).returncode == 0:
            raise SystemExit(f"leak: the list {rel} is tracked by git: remove it from the index (git rm --cached)")
    print(f"leak: {len(patterns)} values from {path}", flush=True)
    scan = Scan(repo, patterns)
    files = scan.tracked()
    scan.staged()
    commits = scan.commits(args.base)
    if scan.hits:
        print(f"leak: FAIL, {scan.hits} hits above (where, and the list line; values are not shown)", flush=True)
        return 1
    print(f"leak: none in {files} tracked files (contents and paths), the staged changes and {commits} "
          "unpushed commits", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

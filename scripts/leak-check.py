#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""The push gate of design v3 11 steps 1-2 (scripts/ci.sh leak): finds the values of a local list
in what a push would publish, without ever printing them.

    python3 -I scripts/leak-check.py [--patterns FILE] [--repo DIR] [--base REF]

The list (default build/leak-patterns.txt, which git ignores; a linked worktree without one uses
its main worktree's; HJV_LEAK_PATTERNS names another file) has one fixed string per line: the
production domain, the university's public address prefix, the NAS and PC addresses, the PC's MAC
address, the department's shared folder name, real portal IDs. Blank lines and lines starting with
'#' are skipped. A MAC address also matches with the other separator, without separators, and in
dotted form. Matching ignores ASCII case.

Searched: every file git tracks, as it is in the working tree (client/patches and server/patches
included), the staged version of staged files, and of the commits a push would publish (REF..HEAD,
default origin/main..HEAD; every commit when REF does not exist) their messages, authors,
committers and added lines. A hit prints where it is and the list's line number, never the value
or the line, and the check fails. It also fails when the list is missing, has no value, or is
itself tracked by git.
"""
import argparse
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MAC = re.compile(r"[0-9a-f]{2}([:-])[0-9a-f]{2}(?:\1[0-9a-f]{2}){4}")


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


def read_patterns(path):
    """[(list line number, [lowercased byte strings to find])]"""
    try:
        with open(path, "rb") as f:
            lines = f.read().split(b"\n")
    except FileNotFoundError:
        raise SystemExit(f"leak: no list at {path}: create it (docs/ci.md, 'push 전 검사')")
    patterns = []
    for n, line in enumerate(lines, 1):
        text = line.strip()
        if not text or text.startswith(b"#"):
            continue
        value = text.lower()
        forms = [value]
        m = MAC.fullmatch(value.decode("ascii", errors="replace"))
        if m:
            octets = re.split(r"[:-]", value.decode())
            forms += [":".join(octets).encode(), "-".join(octets).encode(), "".join(octets).encode(),
                      ".".join("".join(octets[i:i + 2]) for i in (0, 2, 4)).encode()]
        patterns.append((n, sorted(set(forms))))
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
            rel = raw.decode(errors="surrogateescape")
            path = os.path.join(self.repo, rel)
            if os.path.islink(path):
                data = os.readlink(path).encode(errors="surrogateescape")
            elif os.path.isfile(path):
                with open(path, "rb") as f:
                    data = f.read()
            else:
                continue  # deleted in the working tree, or a submodule
            self.blob(rel, data)
        return len(files)

    def staged(self):
        names = git(self.repo, "diff", "--cached", "--name-only", "-z", "--diff-filter=ACMRT").stdout
        for raw in [p for p in names.split(b"\0") if p]:
            rel = raw.decode(errors="surrogateescape")
            self.blob(f"staged {rel}", git(self.repo, "show", f":{rel}").stdout)

    def commits(self, base):
        if git(self.repo, "rev-parse", "--verify", "--quiet", "HEAD", check=False).returncode != 0:
            return 0  # no commit yet
        if git(self.repo, "rev-parse", "--verify", "--quiet", f"{base}^{{commit}}", check=False).returncode == 0:
            span = [f"{base}..HEAD"]
        else:
            print(f"leak: {base} does not exist: checking every commit of HEAD", flush=True)
            span = ["HEAD"]
        log =git(self.repo, "log", "--format=%H%x00%an <%ae>%x00%cn <%ce>%x00%B%x00%x01", *span).stdout
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
                path = None if target == b"/dev/null" else target[2:].decode(errors="surrogateescape")
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
    print(f"leak: none in {files} tracked files, the staged changes and {commits} unpushed commits", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

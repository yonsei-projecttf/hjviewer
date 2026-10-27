#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""What the units of scripts/ci.sh read of the tree, for prepush's reuse of their strict passes
(docs/ci.md, "통과 재사용"). scripts/ci.sh holds the rules: for each unit the paths it provably does
not read (REUSE_SKIP), as patterns that fnmatch matches against a path of HEAD's tree (* also takes
/), the last matching pattern deciding and a pattern after "!" taking paths back. The unit reads all
the rest, so that a new file counts until a rule says otherwise. Rules come as <unit>=<patterns>,
the patterns separated by spaces.

  ci-reuse.py files <repo> <out dir> <unit>=<patterns>...
      for each unit <out dir>/<unit>.files: a line "file <mode> <blob> <path>" for each file of
      HEAD's tree that it reads
  ci-reuse.py values <repo>
      the SHA-256 of the 32-byte values that the tracked files write in base64 or hex, as
      scripts/check-build-keys.py finds them (its public test keys come from every such value)
  ci-reuse.py diff <old inputs> <new inputs>
      what differs between two input lists of scripts/ci.sh's reuse_inputs, on one line
"""

import fnmatch
import hashlib
import importlib.util
import os
import subprocess
import sys

def head_tree(repo):
    """[(mode, blob, path)] of HEAD's tree."""
    out = subprocess.run(["git", "-C", repo, "ls-tree", "-r", "-z", "--full-tree", "HEAD"],
                         capture_output=True, check=True).stdout
    files = []
    for entry in out.split(b"\0"):
        if not entry:
            continue
        meta, path = entry.split(b"\t", 1)
        mode, _, blob = meta.decode().split(" ")
        files.append((mode, blob, path.decode(errors="surrogateescape")))
    return files


def parse_rules(args):
    """{unit: [pattern...]} of <unit>=<patterns> arguments, in their order."""
    rules = {}
    for arg in args:
        unit, _, patterns = arg.partition("=")
        if not unit:
            raise SystemExit(f"ci-reuse.py: no unit in {arg!r}")
        rules[unit] = patterns.split()
    return rules


def skips(path, patterns):
    """Whether the unit of <patterns> does not read <path>: the last pattern that matches says."""
    out = False
    for pattern in patterns:
        back = pattern.startswith("!")
        if fnmatch.fnmatchcase(path, pattern[1:] if back else pattern):
            out = not back
    return out


def files_mode(repo, out_dir, args):
    rules = parse_rules(args)
    tree = head_tree(repo)
    for unit, patterns in rules.items():
        with open(os.path.join(out_dir, unit + ".files"), "w", encoding="utf-8") as f:
            for mode, blob, path in tree:
                if not skips(path, patterns):
                    f.write(f"file {mode} {blob} {path}\n")


def values_mode(repo):
    spec = importlib.util.spec_from_file_location("check_build_keys", os.path.join(repo, "scripts", "check-build-keys.py"))
    cbk = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(cbk)
    digest = hashlib.sha256()
    for raw in sorted(cbk.tree_values(repo)):
        digest.update(raw)
    print(digest.hexdigest())


def sections(path):
    """An input list of reuse_inputs by kind: {kind: {name: value}}."""
    out = {}
    with open(path, encoding="utf-8", errors="surrogateescape") as f:
        for line in f:
            line = line.rstrip("\n")
            kind, _, rest = line.partition(" ")
            if kind == "file":
                mode, blob, name = rest.split(" ", 2)
                out.setdefault(kind, {})[name] = mode + " " + blob
            elif kind in ("code", "global"):
                name, _, value = rest.partition(" ")
                out.setdefault(kind, {})[name] = value
            elif kind in ("tool", "env"):
                out.setdefault(kind, {})[rest] = ""
            else:
                out.setdefault(kind, {})[""] = rest
    return out


def some(names, most=3):
    names = sorted(names)
    return ", ".join(names[:most]) + (f" and {len(names) - most} more" if len(names) > most else "")


def diff_mode(old_path, new_path):
    old, new = sections(old_path), sections(new_path)
    said = []
    for kind, what in (("file", "files"), ("code", "scripts/ci.sh's"), ("global", "scripts/ci.sh's values")):
        a, b = old.get(kind, {}), new.get(kind, {})
        changed = [n for n in set(a) | set(b) if a.get(n) != b.get(n)]
        if changed:
            said.append(f"{what} {some(changed)}" if kind != "file" else some(changed))
    if old.get("top") != new.get("top"):
        said.append("scripts/ci.sh's code outside its functions")
    for kind, what in (("tool", "the tools"), ("env", "the environment")):
        a, b = set(old.get(kind, {})), set(new.get(kind, {}))
        if a != b:
            gone, came = sorted(a - b), sorted(b - a)
            said.append(f"{what}: {(gone[0] if gone else '(none)')} -> {(came[0] if came else '(none)')}"
                        + (f" and {max(len(gone), len(came)) - 1} more" if max(len(gone), len(came)) > 1 else ""))
    if old.get("values") != new.get("values"):
        said.append("a 32-byte value of a tracked file (scripts/check-build-keys.py's test keys)")
    if old.get("unit") != new.get("unit"):
        said.append("another unit")
    print("; ".join(said) if said else "nothing")


def main(argv):
    if len(argv) >= 3 and argv[0] == "files":
        files_mode(argv[1], argv[2], argv[3:])
    elif len(argv) == 2 and argv[0] == "values":
        values_mode(argv[1])
    elif len(argv) == 3 and argv[0] == "diff":
        diff_mode(argv[1], argv[2])
    else:
        print(__doc__, file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

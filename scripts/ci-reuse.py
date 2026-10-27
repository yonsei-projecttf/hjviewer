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
  ci-reuse.py table <repo> <doc> <unit>=<patterns>...
      checks the re-run table of <doc> ("고친 뒤 다시 돌릴 단위"): each row's paths (column 1) are
      read by the units its column 2 names, every tracked file is in a row
  ci-reuse.py reads <repo> <dir>
      checks that the code a unit runs reads no path it leaves out: the Rust, shell, Python and
      JavaScript module (.mjs) files it reads and <dir>/<unit>.code (its code in scripts/ci.sh), by
      include_str!, include_bytes!, #[path], CARGO_MANIFEST_DIR joins and repository paths in Rust;
      $ROOT/<path>, $<name>/<path> of a variable set to a path of the tree, the paths of the tree
      that the words after a cd to one name, unittest discover's -s and -p, and the *_INPUTS lists in
      shell; os.path.join and pathlib joins on the repository's or the file's folder (and the
      folders above them, .parent) in Python; join(<name>, '<path>'...) on a module's folder
      (import.meta.url) or a name joined to it in JavaScript. <dir>/rules has a line
      <unit>=<patterns> for each unit, <dir>/all the patterns of what no unit reads (which a
      reference to a folder does not count), <dir>/tests "<unit> <crate>" lines (the crates whose
      own unit tests, #[cfg(test)] in src/, the unit compiles: what they read counts for it alone),
      <dir>/not-read "<unit> <file> <path>" lines, references that the unit does not follow (a
      branch it does not take), each with its reason in scripts/ci.sh; a path there takes the paths
      under it too.
"""

import fnmatch
import hashlib
import importlib.util
import os
import posixpath
import re
import subprocess
import sys

# the repository's top-level folders, by which a bare path in a Rust string is one of its paths
TOP = ("client/", "deploy/", "docs/", "relay/", "scripts/", "server/", "store/", "tools/", ".github/")


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


# --- the re-run table ---


def table_rows(doc):
    """[(line, [path pattern...], units text)] of the re-run table of <doc>."""
    rows, inside = [], False
    with open(doc, encoding="utf-8") as f:
        for n, line in enumerate(f, 1):
            if line.startswith("### "):
                inside = line.startswith("### 고친 뒤 다시 돌릴 단위")
                continue
            if not inside or not line.startswith("| `"):
                continue
            cells = [c.strip() for c in line.strip().strip("|").split("|")]
            paths = re.findall(r"`([^`]+)`", cells[0])
            if cells[0] != ", ".join(f"`{p}`" for p in paths):
                raise SystemExit(f"{doc}:{n}: column 1 must be paths in backticks and nothing else")
            rows.append((n, paths, cells[1]))
    return rows


def path_matches(pattern, path):
    """Whether a table path names <path>: a folder (ending in /) everything under it, else fnmatch."""
    if pattern.endswith("/"):
        return path.startswith(pattern)
    return fnmatch.fnmatchcase(path, pattern)


def table_mode(repo, doc, args):
    rules = parse_rules(args)
    units = list(rules)
    tree = [path for _, _, path in head_tree(repo)]
    readers = {path: [u for u in units if not skips(path, rules[u])] for path in tree}
    rows = table_rows(doc)
    problems = []
    taken = set()
    for n, patterns, said in rows:
        # a file belongs to the first row that names it: rows of single files go above their folder's
        mine = [p for p in tree if p not in taken and any(path_matches(q, p) for q in patterns)]
        if not mine:
            problems.append(f"{doc}:{n}: no tracked file left for {', '.join(patterns)}")
            continue
        taken.update(mine)
        if patterns == ["scripts/ci.sh"]:
            continue
        sets = {}
        for p in mine:
            sets.setdefault(", ".join(readers[p]) or "-", []).append(p)
        if len(sets) > 1:
            problems.append(f"{doc}:{n}: {', '.join(patterns)} has files that different units read: "
                            + "; ".join(f"{s} ({some(ps, 2)})" for s, ps in sorted(sets.items())) + " (split the row)")
            continue
        (want,) = sets
        if said != want:
            problems.append(f"{doc}:{n}: {', '.join(patterns)}: column 2 says {said!r}, the rules give {want!r}")
    left = [p for p in tree if p not in taken]
    if left:
        problems.append(f"{doc}: no row of the re-run table names {some(left, 5)}")
    for p in problems:
        print(p)
    if problems:
        raise SystemExit(1)
    print(f"the re-run table of {doc}: {len(rows)} rows, every tracked file, as the rules of scripts/ci.sh give them")


# --- the reads of the code a unit runs ---


def crate_of(path, tracked):
    """The folder of the Cargo.toml nearest above <path>, or ""."""
    d = posixpath.dirname(path)
    while d:
        if d + "/Cargo.toml" in tracked:
            return d
        d = posixpath.dirname(d)
    return ""


def norm(path):
    """A repository path from a joined one; None when it leaves the repository."""
    p = posixpath.normpath(path)
    return None if p.startswith("..") or p.startswith("/") else ("" if p == "." else p)


def up(path, levels):
    """The folder <levels> above a repository path; None above the repository."""
    for _ in range(levels):
        if not path:
            return None
        path = posixpath.dirname(path)
    return path


RUST_REL = re.compile(r'(?:include_str!|include_bytes!)\(\s*"([^"]+)"|#\[path\s*=\s*"([^"]+)"\]')
RUST_MOD = re.compile(r'^\s*(?:pub(?:\([a-z]+\))?\s+)?mod\s+([a-z_][a-z0-9_]*)\s*;')
RUST_STR = re.compile(r'"((?:\.\./|/\.\./|[a-z.][a-z0-9_.-]*/)[^"\s]*)"')


def rust_refs(path, text, tracked):
    """(line, path, test) of the references of a Rust file; test when the line is in code that only
    the crate's own unit tests compile: a src/ file's #[cfg(test)] module (at its end, by this
    repository's custom) or a src/*_tests.rs file."""
    here, crate = posixpath.dirname(path), crate_of(path, tracked)
    test = crate != "" and path.startswith(crate + "/src/") and path.endswith("_tests.rs")
    lines = text.splitlines()
    for n, line in enumerate(lines, 1):
        if line.strip() == "#[cfg(test)]" and n < len(lines) and lines[n].lstrip().startswith("mod "):
            test = test or path.startswith(crate + "/src/")
        code = line.split("//", 1)[0] if not line.lstrip().startswith("//") else ""
        m = RUST_MOD.match(code)
        if m and (n < 2 or not lines[n - 2].lstrip().startswith("#[path")):
            # a module of its own file: beside a crate root, mod.rs or an integration test, else in
            # the folder of the file's own name
            root = posixpath.basename(path) in ("lib.rs", "main.rs", "mod.rs") or posixpath.dirname(path) == crate + "/tests"
            base = here if root else posixpath.join(here, posixpath.basename(path)[:-3])
            for p in (posixpath.join(base, m.group(1) + ".rs"), posixpath.join(base, m.group(1), "mod.rs")):
                if p in tracked:
                    yield n, p, test
        for m in RUST_REL.finditer(code):
            yield n, norm(posixpath.join(here, m.group(1) or m.group(2))), test
        for m in RUST_STR.finditer(code):
            s = m.group(1)
            if s.startswith(TOP):
                yield n, norm(s), test
            elif s.startswith(("../", "/../")):
                # a join on the crate's folder (CARGO_MANIFEST_DIR, or the tests' working folder)
                yield n, norm(posixpath.join(crate, s.lstrip("/"))), test


# $ROOT/<path>, $root/<path>, and $<name>/<path> of a variable set to a path of the tree before
SHELL_REF = re.compile(r'\$\{?([A-Za-z_][A-Za-z0-9_]*)\}?"?/([A-Za-z0-9_.@*{},/-]+)')
SHELL_ROOTS = ("ROOT", "root")
# a variable set on the line, and a value that is a path of the tree: $ROOT, or a variable of one,
# and what follows it
SHELL_SET = re.compile(r'(?:^|(?<=[\s;(&|]))([A-Za-z_][A-Za-z0-9_]*)=("[^"]*"|\'[^\']*\'|[^\s;&|)]*)')
SHELL_PATH = re.compile(r'\$\{?([A-Za-z_][A-Za-z0-9_]*)\}?((?:/[A-Za-z0-9_.@-]+)*)/?')
# cd to a folder of the tree, after which the words of the command line are paths in it
SHELL_CD = re.compile(r'\bcd "?\$\{?([A-Za-z_][A-Za-z0-9_]*)\}?((?:/[A-Za-z0-9_.@-]+)*)/?"? *&& *')
DISCOVER = re.compile(r'unittest discover -s "\$ROOT/([A-Za-z0-9_./-]+)" -p \'([^\']+)\'')
SHELL_DIR = re.compile(r'\$\(dirname "\$(?:0|\{BASH_SOURCE\[0\]\})"\)/([A-Za-z0-9_.@/-]+)')
INPUTS = re.compile(r'declare -a ([A-Z_]+_INPUTS)=\((.*)\)')


def braces(path):
    m = re.search(r"\{([^{}]*)\}", path)
    if not m:
        return [path]
    return [x for alt in m.group(1).split(",") for x in braces(path[:m.start()] + alt + path[m.end():])]


CARGO_TEST = re.compile(r"\bcargo(?:_tests| test)\b")
TEST_TARGET = re.compile(r"--test[ =]([A-Za-z0-9_-]+)")


def shell_lines(text):
    """(number, line) of a shell file's lines, a line that ends with a backslash joined with the next"""
    lines, first, held = [], None, []
    for n, line in enumerate(text.splitlines(), 1):
        if first is None:
            first = n
        if line.endswith("\\") and not line.lstrip().startswith("#"):
            held.append(line[:-1])
            continue
        lines.append((first, " ".join(held + [line])))
        first, held = None, []
    if held:
        lines.append((first, " ".join(held)))
    return lines


def shell_base(name, rest, folders):
    """The path of the tree that $<name><rest> names: $ROOT's, or a variable's of folders; or None."""
    base = "" if name in SHELL_ROOTS else folders.get(name)
    return None if base is None else norm(posixpath.join(base, rest.lstrip("/")))


def cd_words(rest):
    """The words of a command line that follows a cd's &&, up to the parenthesis that ends its
    subshell: without quotes, but those that hold a variable, an assignment, an option or a sign of
    the shell."""
    depth, quote, end = 0, None, len(rest)
    for i, c in enumerate(rest):
        if quote:
            quote = None if c == quote else quote
        elif c in "\"'":
            quote = c
        elif c == "(":
            depth += 1
        elif c == ")":
            depth -= 1
            if depth < 0:
                end = i
                break
    for word in re.findall(r'"[^"]*"|\'[^\']*\'|[^\s"\']+', rest[:end]):
        word = word.strip("\"'")
        if word and not word.startswith("-") and not any(c in word for c in "$=`|&;<>()"):
            yield word


def shell_refs(path, text, tracked=()):
    """(line, path, test) of the references of a shell file; in a unit's code of scripts/ci.sh
    (declare -f and declare -p output, a command on one line) the line is the function's name or
    the value's, and a cargo test there with a crate's folder takes that crate's tests (with
    --test <name>, that test's file). A variable set to a path of the tree is followed in the lines
    after it, and so are the paths of the tree that the words after a cd to it name."""
    here = posixpath.dirname(path)
    where = None
    folders = {}
    for n, line in shell_lines(text):
        if line.lstrip().startswith("#"):
            continue
        m = re.match(r"^([a-z_][a-z0-9_]*) \(\) $|^declare -[-aAgx]+ ([A-Za-z_][A-Za-z0-9_]*)=", line)
        if m:
            where = m.group(1) or m.group(2)
        if where is not None:
            n = where
        for m in SHELL_SET.finditer(line):
            if m.group(1) in SHELL_ROOTS:
                continue
            value = SHELL_PATH.fullmatch(m.group(2).strip("\"'"))
            folder = shell_base(value.group(1), value.group(2), folders) if value else None
            if folder is None:
                folders.pop(m.group(1), None)
            else:
                folders[m.group(1)] = folder
        for m in SHELL_CD.finditer(line):
            folder = shell_base(m.group(1), m.group(2), folders)
            if folder is not None:
                for word in cd_words(line[m.end():]):
                    p = norm(posixpath.join(folder, word))
                    if p and matching(p, tracked):
                        yield n, p, False
        discovered = [(m.span(), m.group(1) + "/" + m.group(2)) for m in DISCOVER.finditer(line)]
        for m in SHELL_REF.finditer(line):
            # a path that goes on with a variable names what this cannot know; unittest's -s folder
            # is read by its -p pattern
            if line[m.end():m.end() + 1] == "$" or any(a <= m.start() < b for (a, b), _ in discovered):
                continue
            base = "" if m.group(1) in SHELL_ROOTS else folders.get(m.group(1))
            if base is None:
                continue
            for p in braces(m.group(2).rstrip("}")):
                p = posixpath.join(base, p)
                yield n, norm(p), False
                crate = (norm(p) or "").rstrip("/")
                if path == "scripts/ci.sh" and CARGO_TEST.search(line) and crate + "/Cargo.toml" in tracked:
                    names = TEST_TARGET.findall(line)
                    for name in names or [None]:
                        yield n, crate + ("/tests/" + name + ".rs" if name else "/tests"), False
        for _, p in discovered:
            yield n, norm(p), False
        for m in SHELL_DIR.finditer(line):
            yield n, norm(posixpath.join(here, m.group(1))), False
        for m in INPUTS.finditer(line):
            for p in re.findall(r'"([^"]+)"', m.group(2)):
                yield n, norm(p), False


PY_BASE = {
    "os.path.dirname(os.path.abspath(__file__))": 1,
    "os.path.dirname(os.path.dirname(os.path.abspath(__file__)))": 2,
    "pathlib.Path(__file__).resolve().parent": 1,
    "pathlib.Path(__file__).resolve().parent.parent": 2,
    "Path(__file__).resolve().parent": 1,
    "Path(__file__).resolve().parent.parent": 2,
}
PY_ASSIGN = re.compile(r"^([A-Z_][A-Z0-9_]*) = (.+)$")
# a module-level name with the folders above it (pathlib's .parent), and the strings joined to it
PY_NAME = r'([A-Z_][A-Z0-9_]*)((?:\.parent)*)'
PY_STRINGS = r'((?:\s*{}\s*(?:"[^"]*"|\'[^\']*\'))+)'
PY_JOIN = re.compile(r'os\.path\.join\(\s*' + PY_NAME + PY_STRINGS.format(","))
PY_SLASH = re.compile(r'\b' + PY_NAME + PY_STRINGS.format("/"))


def strings(text):
    """The literal strings of a piece of Python or JavaScript, in either quotes."""
    return [a or b for a, b in re.findall(r'"([^"]*)"|\'([^\']*)\'', text)]


def py_base(name, parents, names):
    """The repository path of a name of <names> with .parent after it <parents>, or None."""
    return up(names[name], parents.count(".parent")) if name in names else None


def py_value(value, here, names):
    """The repository path that a module-level value of a Python file names, or None."""
    if value in PY_BASE:
        return up(here, PY_BASE[value] - 1)
    m = re.fullmatch(r"os\.path\.dirname\(" + PY_NAME + r"\)", value)
    if m:
        return up(py_base(m.group(1), m.group(2), names), 1)
    m = re.fullmatch(PY_NAME, value)
    if m:
        return py_base(m.group(1), m.group(2), names)
    for pattern in (PY_JOIN, PY_SLASH):
        m = pattern.fullmatch(value)
        d = py_base(m.group(1), m.group(2), names) if m else None
        if d is not None:
            return norm(posixpath.join(d, *strings(m.group(3))))
    return None


def py_refs(path, text):
    here = posixpath.dirname(path)
    names = {}
    for n, line in enumerate(text.splitlines(), 1):
        if line.lstrip().startswith("#"):
            continue
        for pattern in (PY_JOIN, PY_SLASH):
            for m in pattern.finditer(line):
                d = py_base(m.group(1), m.group(2), names)
                if d is not None:
                    yield n, norm(posixpath.join(d, *strings(m.group(3)))), False
        m = PY_ASSIGN.match(line)
        if m:
            value = py_value(m.group(2).strip(), here, names)
            if value is not None:
                names[m.group(1)] = value


# a JavaScript module's folder (import.meta.url) and the folders above it, a value it sets, and the
# strings that path.join joins to a name
MJS_BASE = {
    "dirname(fileURLToPath(import.meta.url))": 1,
    "dirname(dirname(fileURLToPath(import.meta.url)))": 2,
}
MJS_ASSIGN = re.compile(r"^(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=\s*(.+?);?$")
MJS_JOIN = re.compile(r'\b(?:path\.)?join\(\s*([A-Za-z_$][\w$]*)((?:\s*,\s*(?:"[^"]*"|\'[^\']*\'))+)\s*\)')


def mjs_refs(path, text):
    """(line, path, test) of the references of a JavaScript module: join(<name>, '<path>'...) on a
    name that it set to its folder or to such a join before"""
    here = posixpath.dirname(path)
    names = {}
    for n, line in enumerate(text.splitlines(), 1):
        if line.lstrip().startswith(("//", "*", "/*")):
            continue
        for m in MJS_JOIN.finditer(line):
            if m.group(1) in names:
                yield n, norm(posixpath.join(names[m.group(1)], *strings(m.group(2)))), False
        m = MJS_ASSIGN.match(line.strip())
        if not m:
            continue
        value, j = m.group(2).strip(), MJS_JOIN.fullmatch(m.group(2).strip())
        d = (up(here, MJS_BASE[value] - 1) if value in MJS_BASE
             else norm(posixpath.join(names[j.group(1)], *strings(j.group(2)))) if j and j.group(1) in names else None)
        if d is None:
            names.pop(m.group(1), None)
        else:
            names[m.group(1)] = d


def matching(ref, tracked):
    """The tracked files that a reference names: the file, the files under the folder, or a glob's."""
    if ref is None or ref == "":
        return []
    if any(c in ref for c in "*?["):
        return [p for p in tracked if fnmatch.fnmatchcase(p, ref)]
    if ref in tracked:
        return [ref]
    return [p for p in tracked if p.startswith(ref.rstrip("/") + "/")]


def reads_mode(repo, work):
    with open(os.path.join(work, "rules"), encoding="utf-8") as f:
        rules = parse_rules([line for line in f.read().split("\n") if line.strip()])
    every = open(os.path.join(work, "all"), encoding="utf-8").read().split()
    tests = {}
    excused = []
    with open(os.path.join(work, "tests"), encoding="utf-8") as f:
        for line in f:
            if line.strip():
                unit, crate = line.split()
                tests.setdefault(unit, set()).add(crate)
    with open(os.path.join(work, "not-read"), encoding="utf-8") as f:
        for line in f:
            if line.strip():
                unit, file, ref = line.split()
                excused.append((unit, file, ref.rstrip("/")))
    tracked = [p for _, _, p in head_tree(repo)]
    tracked_set = set(tracked)
    problems, checked = [], 0
    for unit, patterns in rules.items():
        own = [p for p in patterns]
        whole = own + every
        sources = [(p, None) for p in tracked if not skips(p, whole) and p.endswith((".rs", ".sh", ".py", ".mjs"))
                   and not p.startswith(("client/", "server/patches/"))]
        sources.append(("scripts/ci.sh", os.path.join(work, unit + ".code")))
        for path, code in sources:
            with open(code or os.path.join(repo, path), encoding="utf-8", errors="replace") as f:
                text = f.read()
            refs = (rust_refs(path, text, tracked_set) if path.endswith(".rs")
                    else py_refs(path, text) if path.endswith(".py")
                    else mjs_refs(path, text) if path.endswith(".mjs") else shell_refs(path, text, tracked_set))
            for n, ref, test in refs:
                if ref is None or ref == "scripts/ci.sh":
                    continue
                if test and crate_of(path, tracked_set) not in tests.get(unit, ()):
                    continue
                checked += 1
                named = matching(ref, tracked)
                # a folder's files that no unit reads do not count: the folder is where a build or a
                # test runs; of a crate's folder a build takes its manifest and src/, and its tests
                # only a unit that compiles them (<dir>/tests, a cargo test on its line)
                folder = ref not in tracked_set and not any(c in ref for c in "*?[")
                crate = ref.rstrip("/")
                if folder and crate + "/Cargo.toml" in tracked_set and crate not in tests.get(unit, ()):
                    named = [p for p in named if p in (crate + "/Cargo.toml", crate + "/Cargo.lock", crate + "/build.rs")
                             or p.startswith(crate + "/src/")]
                left = [p for p in named if skips(p, own if folder else whole)]
                file = path if code is None else f"scripts/ci.sh:{n}"
                if left and not any(u == unit and f == file and (ref == r or ref.startswith(r + "/"))
                                    for u, f, r in excused):
                    where = f"{path}:{n}" if code is None else f"scripts/ci.sh's {n}"
                    problems.append(f"{unit}: {where} reads {ref}, which its rules leave out ({some(left, 2)})")
    for p in problems:
        print(p)
    if problems:
        raise SystemExit(1)
    print(f"the code that {len(rules)} units run: {checked} references to files, none into a path its unit leaves out")


def main(argv):
    if len(argv) >= 3 and argv[0] == "files":
        files_mode(argv[1], argv[2], argv[3:])
    elif len(argv) == 2 and argv[0] == "values":
        values_mode(argv[1])
    elif len(argv) == 3 and argv[0] == "diff":
        diff_mode(argv[1], argv[2])
    elif len(argv) >= 3 and argv[0] == "table":
        table_mode(argv[1], argv[2], argv[3:])
    elif len(argv) == 3 and argv[0] == "reads":
        reads_mode(argv[1], argv[2])
    else:
        print(__doc__, file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

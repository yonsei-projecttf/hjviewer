#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""The known advisories of the packages that HJViewer's lock files pin, from OSV
(https://api.osv.dev), which receives the packages' names and versions and nothing else:

    python3 -I scripts/advisories.py [--upstream <dir>] [--known <file>] [--update-known]

GitHub's dependency graph sees no manifest of this repository, and the lock files of the patched
upstream trees are not in it at all, so a release runs this by hand (docs/ci.md says when). It is
no stage of scripts/ci.sh: it needs the network, and its answer changes with time.

Our own lock files, server/api, relay and tools/*/Cargo.lock, must be clean: every advisory there
is a failure (FAIL), except RustSec's informational kinds "unmaintained" and "notice", which are
listed (INFO). The lock files of the upstream trees that scripts/fetch-upstream.sh checked out
(<upstream>/rustdesk-server/Cargo.lock, <upstream>/rustdesk/Cargo.lock and
<upstream>/rustdesk/flutter/pubspec.lock; a tree not fetched is skipped and said so) are compared
with the recorded list scripts/advisories-known.txt (lock file, package, version, advisory ID): an
advisory not in it is a failure (NEW), an informational one is listed (INFO); one recorded that is
no longer reported is listed (GONE). --update-known rewrites the list from this run's answer for
the upstream lock files it read, and keeps the lines of those it did not.

Only packages from the public registries are asked about: crates.io's for Cargo.lock (git and path
sources are skipped) and pub.dev's for pubspec.lock (hosted there; git, path and sdk are skipped).
An advisory that OSV holds under several IDs (RUSTSEC, GHSA, CVE) is one line, named by its
RUSTSEC ID where it has one, with the others in brackets and the versions that fix it.

Exit status: 0 nothing to do, 1 a failure (above), 2 a lock file that cannot be read or a wrong
argument, 3 OSV could not be asked or gave an answer this script cannot read: then nothing was
checked, and a later run asks again.
"""
import argparse
import concurrent.futures
import datetime
import glob
import json
import os
import re
import sys
import tomllib
import urllib.error
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OSV = "https://api.osv.dev"
# OSV takes at most 1000 queries in one batch
BATCH = 1000
TIMEOUT = 60
CRATES = "crates.io"
PUB = "Pub"
CRATES_SOURCES = ("registry+https://github.com/rust-lang/crates.io-index", "sparse+https://index.crates.io/")
PUB_HOSTS = ("https://pub.dev", "https://pub.dartlang.org")
# RustSec's kinds of advisory that report no vulnerability
INFORMATIONAL = ("unmaintained", "notice")
OWN = ("server/api/Cargo.lock", "relay/Cargo.lock", "tools/*/Cargo.lock")
UPSTREAM = ("rustdesk-server/Cargo.lock", "rustdesk/Cargo.lock", "rustdesk/flutter/pubspec.lock")
KNOWN = os.path.join(ROOT, "scripts", "advisories-known.txt")

OK, FINDINGS, BAD_INPUT, NETWORK = 0, 1, 2, 3


class InputError(Exception):
    """A lock file or the known list that cannot be read (exit status 2)."""


class NetworkError(Exception):
    """OSV could not be asked, or its answer cannot be read (exit status 3)."""


# ---- lock files ----

def cargo_packages(text, name="Cargo.lock"):
    """The (name, version) pairs of crates.io packages in a Cargo.lock (versions 3 and 4, and the
    older ones, which have the same [[package]] tables): git and path sources are skipped."""
    try:
        lock = tomllib.loads(text)
    except tomllib.TOMLDecodeError as e:
        raise InputError(f"{name}: not a Cargo.lock ({e})") from None
    out = set()
    for p in lock.get("package", []):
        if not isinstance(p, dict) or not isinstance(p.get("name"), str) or not isinstance(p.get("version"), str):
            raise InputError(f"{name}: a [[package]] without name or version")
        source = p.get("source")
        if isinstance(source, str) and source.startswith(CRATES_SOURCES):
            out.add((p["name"], p["version"]))
    return sorted(out)


def _scalar(value):
    value = value.strip()
    if len(value) >= 2 and value[0] == value[-1] and value[0] in "\"'":
        return value[1:-1]
    return value


def pub_packages(text, name="pubspec.lock"):
    """The (name, version) pairs of the packages hosted on pub.dev in a pubspec.lock, read as pub
    writes it (two spaces a level): git, path and sdk packages and other hosts are skipped."""
    out, packages, current, seen = set(), False, None, False

    def done(p):
        if p and p.get("source") == "hosted" and p.get("url", "https://pub.dev").rstrip("/") in PUB_HOSTS:
            if not p.get("version"):
                raise InputError(f"{name}: the package {p['key']} has no version")
            out.add((p.get("name") or p["key"], p["version"]))

    for line in text.splitlines():
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        indent = len(line) - len(line.lstrip(" "))
        key, sep, value = line.strip().partition(":")
        if not sep:
            raise InputError(f"{name}: cannot read the line {line.strip()!r}")
        if indent == 0:
            done(current)
            current = None
            packages = key == "packages"
            seen = seen or packages
        elif not packages:
            continue
        elif indent == 2:
            done(current)
            current = {"key": key}
        elif indent == 4 and current is not None:
            if key in ("source", "version"):
                current[key] = _scalar(value)
            elif key == "description" and value.strip():
                current["description"] = _scalar(value)
        elif indent == 6 and current is not None and key in ("name", "url"):
            current[key] = _scalar(value)
    done(current)
    if not seen:
        raise InputError(f"{name}: no packages: section")
    return sorted(out)


def read_lock(path, name):
    """(ecosystem, [(package, version)]) of a lock file."""
    try:
        with open(path, encoding="utf-8") as f:
            text = f.read()
    except OSError as e:
        raise InputError(f"{name}: {e.strerror}") from None
    if path.endswith("pubspec.lock"):
        return PUB, pub_packages(text, name)
    return CRATES, cargo_packages(text, name)


def lock_files(root, upstream):
    """([(name, path)] of our own lock files, [(name, path)] of the upstream ones,
    [names of upstream lock files not fetched]). The upstream ones are named upstream/...,
    wherever <upstream> is."""
    own = []
    for pattern in OWN:
        for path in sorted(glob.glob(os.path.join(root, pattern))):
            own.append((os.path.relpath(path, root).replace(os.sep, "/"), path))
    found, missing = [], []
    for rel in UPSTREAM:
        path = os.path.join(upstream, rel)
        (found if os.path.isfile(path) else missing).append(("upstream/" + rel, path))
    return own, found, [n for n, _ in missing]


# ---- OSV ----

class Osv:
    """OSV's API: the batch query (IDs per package and version) and each advisory's record."""

    def __init__(self, base=OSV, timeout=TIMEOUT):
        self.base = base
        self.timeout = timeout

    def _call(self, path, body=None):
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(self.base + path, data=data, method="GET" if body is None else "POST",
                                     headers={"Content-Type": "application/json", "User-Agent": "hjviewer-advisories"})
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as r:
                return json.load(r)
        except urllib.error.HTTPError as e:
            raise NetworkError(f"{self.base}{path}: HTTP {e.code}") from None
        except (urllib.error.URLError, OSError) as e:
            raise NetworkError(f"{self.base}{path}: {getattr(e, 'reason', e)}") from None
        except ValueError as e:
            raise NetworkError(f"{self.base}{path}: the answer is no JSON ({e})") from None

    def querybatch(self, queries):
        return self._call("/v1/querybatch", {"queries": queries})

    def vuln(self, vid):
        return self._call("/v1/vulns/" + vid)


def ask(osv, packages):
    """{(ecosystem, name, version): [advisory IDs]} for the set of (ecosystem, name, version)."""
    keys = sorted(packages)
    ids = {}
    for start in range(0, len(keys), BATCH):
        part = keys[start:start + BATCH]
        queries = [{"package": {"name": n, "ecosystem": e}, "version": v} for e, n, v in part]
        answer = osv.querybatch(queries)
        results = answer.get("results") if isinstance(answer, dict) else None
        if not isinstance(results, list) or len(results) != len(part):
            raise NetworkError("OSV's batch answer does not have one result per package")
        for key, query, result in zip(part, queries, results):
            found = []
            while True:
                if not isinstance(result, dict):
                    raise NetworkError("OSV's batch answer has a result that is no object")
                for v in result.get("vulns") or []:
                    # an ID goes into the next request's path
                    if not isinstance(v, dict) or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._:-]{0,99}", str(v.get("id"))):
                        raise NetworkError("OSV's batch answer has an advisory without a readable ID")
                    found.append(v["id"])
                token = result.get("next_page_token")
                if not token:
                    break
                again = osv.querybatch([dict(query, page_token=token)])
                result = (again.get("results") or [None])[0] if isinstance(again, dict) else None
            if found:
                ids[key] = sorted(set(found))
    return ids


def records(osv, vids):
    """{ID: record} of the advisories, asked side by side."""
    with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
        got = dict(zip(vids, pool.map(osv.vuln, vids)))
    for vid, rec in got.items():
        if not isinstance(rec, dict) or rec.get("id") != vid:
            raise NetworkError(f"OSV's record of {vid} cannot be read")
    return got


# ---- advisories ----

def version_key(version):
    """For sorting versions: numbers as numbers, ahead of words."""
    return [(0, int(x), "") if x.isdigit() else (1, 0, x) for x in re.split(r"[.+-]", version)]


def _prerelease(version):
    return "-" in version


def fixed_versions(rec, ecosystem, name, version):
    """The versions that fix <rec> for the package, pre-releases only for a pre-release."""
    fixed = set()
    for a in rec.get("affected") or []:
        p = a.get("package") or {}
        if p.get("ecosystem") != ecosystem or p.get("name") != name:
            continue
        for r in a.get("ranges") or []:
            for e in r.get("events") or []:
                if isinstance(e.get("fixed"), str):
                    fixed.add(e["fixed"])
    if not _prerelease(version) and any(not _prerelease(f) for f in fixed):
        fixed = {f for f in fixed if not _prerelease(f)}
    return sorted(fixed, key=version_key)


def informational(rec, ecosystem, name):
    """RustSec's informational kind of <rec> for the package, or None."""
    for a in rec.get("affected") or []:
        p = a.get("package") or {}
        if p.get("ecosystem") == ecosystem and p.get("name") == name:
            kind = (a.get("database_specific") or {}).get("informational")
            if isinstance(kind, str) and kind:
                return kind
    return None


def _rank(vid):
    return (0 if vid.startswith("RUSTSEC-") else 1 if vid.startswith("GHSA-") else 2, vid)


def group(ecosystem, name, version, vids, recs):
    """The advisories of one package version, an advisory under several IDs once: a list of dicts
    with id (the one that names it), ids (all of them), kind (informational or None), fixed and
    summary. Withdrawn records count for nothing."""
    parent = {}

    def find(x):
        while parent.setdefault(x, x) != x:
            x = parent[x]
        return x

    live = [v for v in vids if not recs[v].get("withdrawn")]
    for v in live:
        for alias in recs[v].get("aliases") or []:
            parent[find(alias)] = find(v)
    groups = {}
    for v in live:
        groups.setdefault(find(v), []).append(v)
    out = []
    for members in groups.values():
        names = set(members)
        for v in members:
            names.update(a for a in recs[v].get("aliases") or [] if isinstance(a, str))
        main = min(members, key=_rank)
        # shown beside it: the IDs OSV gave for this package, and the GHSA and CVE names; an alias
        # of another ecosystem or another crate's advisory (one CVE for several) still matches the
        # known list
        shown = {n for n in names if n in members or n.startswith(("GHSA-", "CVE-"))}
        kinds = [informational(recs[v], ecosystem, name) for v in members]
        fixed = sorted({f for v in members for f in fixed_versions(recs[v], ecosystem, name, version)},
                       key=version_key)
        summary = next((recs[v].get("summary") for v in sorted(members, key=_rank) if recs[v].get("summary")), "")
        out.append({
            "id": main,
            "ids": names,
            "shown": shown,
            # informational only when no record of it reports a vulnerability
            "kind": kinds[0] if kinds and all(k and k == kinds[0] for k in kinds) else None,
            "fixed": fixed,
            "summary": " ".join(str(summary).split()),
        })
    return sorted(out, key=lambda g: _rank(g["id"]))


def line(tag, lock, name, version, adv):
    others = sorted(adv["shown"] - {adv["id"]}, key=_rank)
    text = f"{tag:4} {lock} {name} {version} {adv['id']}"
    if others:
        text += f" ({', '.join(others)})"
    if adv["kind"]:
        text += f" {adv['kind']}"
    text += "; fixed in " + ", ".join(adv["fixed"]) if adv["fixed"] else "; no fixed version"
    summary = adv["summary"]
    if summary:
        text += ": " + (summary if len(summary) <= 100 else summary[:99] + "…")
    return text


# ---- the known list ----

def read_known(path):
    """{(lock file, package, version, advisory ID)} of the known list (absent: empty)."""
    known = set()
    try:
        with open(path, encoding="utf-8") as f:
            lines = f.read().splitlines()
    except FileNotFoundError:
        return known
    except OSError as e:
        raise InputError(f"{path}: {e.strerror}") from None
    for n, raw in enumerate(lines, 1):
        text = raw.split("#", 1)[0].strip()
        if not text:
            continue
        parts = text.split()
        if len(parts) != 4 or not parts[0].startswith("upstream/"):
            raise InputError(f"{path}:{n}: not <upstream/... lock file> <package> <version> <advisory ID>")
        known.add(tuple(parts))
    return known


def write_known(path, entries, upstream_env):
    head = [
        "# The advisories of the upstream trees' lock files that a release has seen and left as they",
        "# are, written by python3 -I scripts/advisories.py --update-known (docs/ci.md): lock file,",
        "# package, version, advisory ID. The script reports an advisory of these lock files only",
        "# when it is not here.",
    ]
    head.append(f"# Written {datetime.datetime.now(datetime.timezone.utc):%Y-%m-%d} (UTC) from the upstream trees of"
                " scripts/upstream.env with server/patches")
    commits = ", ".join(f"{k} {v}" for k, v in upstream_env)
    head.append("# and client/patches" + (": " + commits if commits else ""))
    rows = sorted(entries, key=lambda e: (e[0], e[1], e[2], _rank(e[3])))
    width = [max([len(r[i]) for r in rows] + [0]) for i in range(3)]
    body = [f"{r[0]:<{width[0]}}  {r[1]:<{width[1]}}  {r[2]:<{width[2]}}  {r[3]}" for r in rows]
    tmp = path + ".new"
    with open(tmp, "w", encoding="utf-8") as f:
        f.write("\n".join(head + body) + "\n")
    os.replace(tmp, path)


def upstream_commits(root):
    """[(name, commit)] of scripts/upstream.env, for the known list's head."""
    out = []
    try:
        with open(os.path.join(root, "scripts", "upstream.env"), encoding="utf-8") as f:
            for raw in f:
                m = re.fullmatch(r"(RUSTDESK_SERVER_COMMIT|RUSTDESK_COMMIT)=([0-9a-f]{40})", raw.strip())
                if m:
                    out.append((m.group(1), m.group(2)[:12]))
    except OSError:
        pass
    return out


# ---- the check ----

def check(root, upstream, known_path, update, osv, out=None):
    out = out or sys.stdout
    own, ups, missing = lock_files(root, upstream)
    if not own:
        raise InputError(f"no lock file of our own under {root} ({', '.join(OWN)})")
    locks = []
    for name, path in own + ups:
        eco, pkgs = read_lock(path, name)
        locks.append((name, eco, pkgs, name.startswith("upstream/")))
    known = read_known(known_path)
    wanted = {(eco, n, v) for _, eco, pkgs, _ in locks for n, v in pkgs}
    ids = ask(osv, wanted)
    recs = records(osv, sorted({v for vs in ids.values() for v in vs}))

    print(f"OSV ({OSV}), {datetime.datetime.now(datetime.timezone.utc):%Y-%m-%d %H:%M} UTC: "
          f"{len(wanted)} package versions of {len(locks)} lock files", file=out)
    for name, eco, pkgs, _ in locks:
        print(f"read {name}: {len(pkgs)} {'pub.dev' if eco == PUB else 'crates.io'} packages", file=out)
    for name in missing:
        print(f"skipped {name}: not fetched (scripts/fetch-upstream.sh)", file=out)

    fails = new = 0
    # the known list's lines still reported, and the advisories of the upstream lock files it lacks
    still, unknown = set(), set()
    read_upstream = {name for name, _, _, is_upstream in locks if is_upstream}
    for name, eco, pkgs, is_upstream in locks:
        for pkg, version in pkgs:
            for adv in group(eco, pkg, version, ids.get((eco, pkg, version), []), recs):
                if not is_upstream:
                    tag = "INFO" if adv["kind"] in INFORMATIONAL else "FAIL"
                    fails += tag == "FAIL"
                    print(line(tag, name, pkg, version, adv), file=out)
                    continue
                # recorded under any of its IDs
                matched = {k for k in known if k[:3] == (name, pkg, version) and k[3] in adv["ids"]}
                if matched:
                    still.update(matched)
                    continue
                unknown.add((name, pkg, version, adv["id"]))
                tag = "INFO" if adv["kind"] in INFORMATIONAL else "NEW"
                new += tag == "NEW"
                print(line(tag, name, pkg, version, adv), file=out)
    for k in sorted(k for k in known if k[0] in read_upstream and k not in still):
        print(f"GONE {' '.join(k)}: recorded, no longer reported" + ("" if update else " (--update-known drops it)"),
              file=out)
    print(f"known: {len(still)} recorded advisories of the upstream lock files still reported", file=out)
    if update:
        entries = {k for k in known if k[0] not in read_upstream} | still | unknown
        write_known(known_path, entries, upstream_commits(root))
        print(f"wrote {os.path.relpath(known_path, root)}: {len(entries)} advisories", file=out)
        new = 0
    status = FINDINGS if fails or new else OK
    print(f"result: {fails} advisories in our lock files, {new} new in the upstream ones (exit {status})", file=out)
    return status


def main(argv=None, osv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--root", default=ROOT, help="the tree whose own lock files are read (this one)")
    ap.add_argument("--upstream", help="the fetched upstream trees (default: <root>/upstream)")
    ap.add_argument("--known", help="the known list (default: <root>/scripts/advisories-known.txt)")
    ap.add_argument("--update-known", action="store_true",
                    help="rewrite the known list from this run's answer for the upstream lock files read")
    args = ap.parse_args(argv)
    root = os.path.abspath(args.root)
    upstream = os.path.abspath(args.upstream or os.path.join(root, "upstream"))
    known = os.path.abspath(args.known or os.path.join(root, "scripts", "advisories-known.txt"))
    try:
        return check(root, upstream, known, args.update_known, osv or Osv())
    except InputError as e:
        print(f"advisories: {e}", file=sys.stderr)
        return BAD_INPUT
    except NetworkError as e:
        print(f"advisories: OSV could not be asked, nothing was checked ({e}); run it again later",
              file=sys.stderr)
        return NETWORK


if __name__ == "__main__":
    sys.exit(main())

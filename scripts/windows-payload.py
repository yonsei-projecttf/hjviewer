#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Lists every file under a directory with its SHA-256, as sha256sum writes it, in a fixed order.

    python3 -I scripts/windows-payload.py <dir> > list.sha256

The Windows build job (client-build.yml) lists the app folder before it hands the folder to the
signing job as an artifact; the signing job lists what it downloaded and compares the two byte for
byte, so a file that the artifact dropped (a hidden one, say), added or changed stops the job
before anything is signed or packed. Paths use "/" and line ends are "\n" on every system.
"""
import hashlib
import os
import sys


def listing(root):
    lines = []
    for top, dirs, files in os.walk(root):
        dirs.sort()
        if os.path.islink(top) or any(os.path.islink(os.path.join(top, d)) for d in dirs):
            raise SystemExit(f"error: a link in {top}: the payload holds plain files only")
        for name in sorted(files):
            path = os.path.join(top, name)
            if os.path.islink(path) or not os.path.isfile(path):
                raise SystemExit(f"error: not a plain file: {path}")
            digest = hashlib.sha256()
            with open(path, "rb") as f:
                for block in iter(lambda: f.read(1 << 20), b""):
                    digest.update(block)
            relative = os.path.relpath(path, root).replace(os.sep, "/")
            if "\n" in relative or "\r" in relative:
                raise SystemExit(f"error: a line break in a file name under {root}")
            lines.append(f"{digest.hexdigest()}  {relative}\n")
    if not lines:
        raise SystemExit(f"error: no files under {root}")
    return "".join(lines)


def main(argv):
    if len(argv) != 2 or not os.path.isdir(argv[1]):
        print("usage: windows-payload.py <dir>", file=sys.stderr)
        return 2
    sys.stdout.buffer.write(listing(argv[1]).encode("utf-8"))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

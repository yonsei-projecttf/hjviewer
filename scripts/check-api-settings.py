#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""The production settings of hjv-api (design v3 5-6 and 3-6, decisions U1 and U4), as
scripts/ci.sh deploy checks them in the packaged example and in a server's own env file:

    python3 -I scripts/check-api-settings.py --binary <hjv-api> --example <env example> <env file>

    HJV_API_BIND_OS         windows: only installed Windows PCs join an account (5-6)
    HJV_API_SECOND_FACTOR   off (U1)
    HJV_API_TRUST_REFRESH   60-300 seconds (3-6)
    HJV_API_TRUST_TTL       at most 72 hours, the apps' bound (U4: 86400)

A setting's value is the one systemd hands hjv-api from the env file (EnvironmentFile=: lines
starting with '#' or ';' are comments, blanks around the name and the value and the quotes around
the value are dropped, the last assignment counts) and, where the file leaves it out or empty
(hjv-api then takes its default), the default that the example documents in a comment line
("#NAME=value" or "# NAME=value"). A setting the binary does not know, because the work package
that adds it is not in yet, is skipped; one it knows whose value neither file gives fails, since
the check cannot tell what hjv-api would use: document its default in the example.
"""
import argparse
import re
import sys

WINDOWS_ONLY = "only installed Windows PCs join an account (design v3 5-6)"
CHECKS = (
    # hjv-api splits the list at commas, trims and lowercases it, and drops empty entries
    ("HJV_API_BIND_OS", "windows, " + WINDOWS_ONLY,
     lambda v: [p.strip().lower() for p in v.split(",") if p.strip()] == ["windows"]),
    ("HJV_API_SECOND_FACTOR", "off (decision U1)", lambda v: v == "off"),
    ("HJV_API_TRUST_REFRESH", "60-300 seconds (design v3 3-6)",
     lambda v: re.fullmatch(r"[0-9]+", v) is not None and 60 <= int(v) <= 300),
    ("HJV_API_TRUST_TTL", "at most 259200 seconds (72 hours), the apps' bound (decision U4: 86400)",
     lambda v: re.fullmatch(r"[0-9]+", v) is not None and 0 < int(v) <= 259200),
)


def unquote(raw):
    """A value (without the blanks around it) as systemd reads it: quoted parts lose their quotes,
    '\\' escapes the next character in "..." and outside quotes, blanks after a closing quote go."""
    out, i = [], 0
    while i < len(raw):
        c = raw[i]
        if c in "'\"":
            i += 1
            while i < len(raw) and raw[i] != c:
                if c == '"' and raw[i] == "\\" and i + 1 < len(raw):
                    i += 1
                out.append(raw[i])
                i += 1
            i += 1
            while i < len(raw) and raw[i] in " \t":
                i += 1
        elif c == "\\" and i + 1 < len(raw):
            out.append(raw[i + 1])
            i += 2
        else:
            out.append(c)
            i += 1
    return "".join(out)


def lines(path):
    with open(path, encoding="utf-8", errors="replace") as f:
        return f.read().splitlines()


def assigned(path, name):
    """The value the env file gives <name>, or None."""
    value = None
    for line in lines(path):
        text = line.strip()
        if not text or text[0] in "#;":
            continue
        key, sep, rest = text.partition("=")
        if sep and key.strip() == name:
            value = unquote(rest.strip())
    return value


def documented(path, name):
    """The default the example documents for <name> in a "#NAME=value" line, or None."""
    value = None
    for line in lines(path):
        m = re.fullmatch(r"\s*[#;]\s*" + re.escape(name) + r"\s*=(.*)", line)
        if m:
            value = unquote(m.group(1).strip())
    return value


def check(binary, example, env):
    """[(level, message)] with level "ok", "skipped" or "fail"."""
    with open(binary, "rb") as f:
        known = f.read()
    results = []
    for name, rule, good in CHECKS:
        if name.encode() not in known:
            results.append(("skipped", f"{name}: this hjv-api does not know it yet"))
            continue
        # hjv-api trims a value and takes its default for an empty one
        value, source = (assigned(env, name) or "").strip(), env
        if not value:
            value, source = documented(example, name), f"{example}, documented default"
            value = value.strip() if value is not None else None
        if value is None:
            results.append(("fail", f"{name}: hjv-api knows it, but {env} does not set it and {example} "
                                    f"documents no default (a '#{name}=<value>' line): must be {rule}"))
        elif good(value):
            results.append(("ok", f"{name}={value} ({source})"))
        else:
            results.append(("fail", f"{name} is '{value}' ({source}): must be {rule}"))
    return results


def main(argv):
    parser = argparse.ArgumentParser(description="Checks hjv-api's production settings.")
    parser.add_argument("--binary", required=True, help="the packaged hjv-api")
    parser.add_argument("--example", required=True, help="the packaged hjviewer-api.env.example")
    parser.add_argument("env", help="the env file to check (the example itself, or a server's copy)")
    args = parser.parse_args(argv)
    results = check(args.binary, args.example, args.env)
    for level, message in results:
        print({"ok": "", "skipped": "skipped: ", "fail": "FAIL: "}[level] + message, flush=True)
    return 1 if any(level == "fail" for level, _ in results) else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

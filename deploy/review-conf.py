#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Writes the file of hjv-api's App Review account (HJV_API_REVIEW_FILE: server/api/README.md, The
App Review account; docs/server-runbook.md 7-17). Run it in a terminal of your own (Linux, macOS or
WSL), not in an agent's session, with FILE outside the repository:

    python3 -I deploy/review-conf.py --until YYYY-MM-DD --out FILE

It asks for the ID, and then for the password twice without showing it, on the terminal only:
neither comes from the command line, which a shell's history and the process list keep, the
environment or a standard input that is not a terminal. It writes FILE (0600, never over a file
that is there) and prints only its path and UNTIL:

    ID=<the ID as typed>
    HASH=pbkdf2-sha256$600000$<salt, base64>$<key, base64>
    UNTIL=<0:00 KST at the start of the date, in UTC: 2026-11-29T15:00:00Z for 2026-11-30>

HASH is PBKDF2-HMAC-SHA256 with a random salt of 16 bytes and a key of 32. The ID is 1 to 64
letters, digits, '.', '_' and '-', and must not pass HJViewer's ID rule (Z or Y and then 1 to 59 of
A-Z, 0-9, '_' and '-', not ZA: the faculty's and staff's portal IDs), so that the account can never
be a real one; hjv-api refuses such a file too. The password is 12 to 256 printable ASCII
characters, which a reviewer can type on any keyboard. The date is at most 90 days ahead, so that
a slip in it cannot keep the account for a year. Python 3's standard library only.
"""
import argparse
import base64
import datetime
import getpass
import hashlib
import os
import re
import secrets
import sys
import warnings

ITERATIONS = 600000
SALT_BYTES = 16
MAX_DAYS = 90
KST = datetime.timezone(datetime.timedelta(hours=9))
# HJViewer's ID rule (server/api/src/util.rs staff_id), on the ID uppercased
STAFF_ID = re.compile(r"[ZY][A-Z0-9_-]{1,59}")
ID_FORM = re.compile(r"[A-Za-z0-9._-]{1,64}")
DATE = re.compile(r"([0-9]{4})-([0-9]{2})-([0-9]{2})")


class Refused(Exception):
    """What the command refuses, in one line."""


def staff_id(text):
    """Whether <text> passes HJViewer's ID rule."""
    upper = text.upper()
    return STAFF_ID.fullmatch(upper) is not None and not upper.startswith("ZA")


def check_id(text):
    if not ID_FORM.fullmatch(text):
        raise Refused("the ID is 1 to 64 letters, digits, '.', '_' or '-'")
    if staff_id(text):
        raise Refused("the ID passes HJViewer's ID rule (Z or Y, not ZA), so it could be a staff "
                      "account's: choose another")
    return text


def until_of(date, now):
    """UNTIL of the date YYYY-MM-DD: 0:00 KST at its start, in UTC; after <now>, and at most
    MAX_DAYS after it."""
    m = DATE.fullmatch(date)
    if not m:
        raise Refused("--until is a date, YYYY-MM-DD")
    try:
        start = datetime.datetime(int(m[1]), int(m[2]), int(m[3]), tzinfo=KST)
    except ValueError:
        raise Refused("--until is not a day of the calendar") from None
    until = start.astimezone(datetime.timezone.utc)
    if until <= now:
        raise Refused(f"{until:%Y-%m-%dT%H:%M:%SZ} has come already: give a later date")
    if until > now + datetime.timedelta(days=MAX_DAYS):
        raise Refused(f"--until is at most {MAX_DAYS} days ahead: the account ends")
    return f"{until:%Y-%m-%dT%H:%M:%SZ}"


def check_password(text):
    if not 12 <= len(text) <= 256 or any(not " " <= c <= "~" for c in text):
        raise Refused("the password is 12 to 256 printable ASCII characters")
    return text


def ask_password(ask):
    """The password, asked twice on the terminal, never shown."""
    with warnings.catch_warnings():
        # where getpass cannot hide what is typed it warns and shows it: refuse instead
        warnings.simplefilter("error", getpass.GetPassWarning)
        try:
            first = ask("App Review password: ")
            second = ask("The same again: ")
        except getpass.GetPassWarning:
            raise Refused("this terminal cannot hide the password") from None
    if first != second:
        raise Refused("the two passwords differ")
    return check_password(first)


def hash_of(password, salt, iterations=ITERATIONS):
    """HASH: pbkdf2-sha256$<iterations>$<salt>$<key>, in standard base64."""
    key = hashlib.pbkdf2_hmac("sha256", password.encode("ascii"), salt, iterations, 32)
    b64 = base64.b64encode
    return f"pbkdf2-sha256${iterations}${b64(salt).decode()}${b64(key).decode()}"


def write_new(path, text):
    """Writes a new file of mode 0600; never over a file, nor through a link."""
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, "w", encoding="ascii") as f:
        os.fchmod(f.fileno(), 0o600)
        f.write(text)


def main(argv, stdin=sys.stdin, ask=getpass.getpass, now=None, read=input):
    parser = argparse.ArgumentParser(description="Writes the file of hjv-api's App Review account.")
    parser.add_argument("--until", required=True, help="the day it ends at 0:00 KST, YYYY-MM-DD")
    parser.add_argument("--out", required=True, help="the file to write (it must not be there)")
    args = parser.parse_args(argv)
    try:
        # write_new needs O_NOFOLLOW, which Windows' Python lacks: refuse before anything is asked
        if not hasattr(os, "O_NOFOLLOW"):
            raise Refused("run this in a Linux, macOS or WSL terminal")
        until = until_of(args.until, now or datetime.datetime.now(datetime.timezone.utc))
        if os.path.lexists(args.out):
            raise Refused(f"{args.out} is there already: this writes no file over another")
        if not stdin.isatty():
            raise Refused("the ID and the password are asked in a terminal: run this in one, with "
                          "nothing piped in")
        review_id = check_id(read("App Review ID: ").strip())
        password = ask_password(ask)
        text = (f"# hjv-api's App Review account (HJV_API_REVIEW_FILE), by deploy/review-conf.py\n"
                f"ID={review_id}\n"
                f"HASH={hash_of(password, secrets.token_bytes(SALT_BYTES))}\n"
                f"UNTIL={until}\n")
        write_new(args.out, text)
    except Refused as e:
        print(f"review-conf.py: {e}", file=sys.stderr)
        return 1
    except OSError as e:
        print(f"review-conf.py: {args.out}: {e.strerror}", file=sys.stderr)
        return 1
    print(os.path.abspath(args.out))
    print(f"UNTIL={until}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

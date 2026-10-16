#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Writes the file of hjv-api's access password, 이용 비밀번호 (HJV_API_GUEST_FILE:
server/api/README.md, The access password; docs/access-tickets.md 3-5). Run it in a terminal of your
own (Linux, macOS or WSL), not in an agent's session, with FILE outside the repository:

    python3 -I deploy/guest-conf.py --until YYYY-MM-DD --out FILE            # asks for it
    python3 -I deploy/guest-conf.py --until YYYY-MM-DD --out FILE --random   # makes one

It asks for the password twice without showing it, on the terminal only: it never comes from the
command line, which a shell's history and the process list keep, the environment or a standard
input that is not a terminal. With --random it makes one instead (four groups of five letters and
digits that cannot be mistaken for one another) and shows it once, on a terminal only. It writes
FILE (0600, never over a file that is there) and prints its path and UNTIL:

    HASH=pbkdf2-sha256$600000$<salt, base64>$<key, base64>
    UNTIL=<0:00 KST at the start of the date, in UTC: 2026-11-29T15:00:00Z for 2026-11-30>

HASH is PBKDF2-HMAC-SHA256 with a random salt of 16 bytes and a key of 32, as the App Review
account's (deploy/review-conf.py). The password is 12 to 256 printable ASCII characters, which
anyone can type on any keyboard, a phone's too. The date is at most 90 days ahead, so that a slip in
it cannot keep the password for a year. Python 3's standard library only.
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
DATE = re.compile(r"([0-9]{4})-([0-9]{2})-([0-9]{2})")
# no 0/o, 1/l/i: a password read out or copied by hand
ALPHABET = "abcdefghjkmnpqrstuvwxyz23456789"
GROUPS, GROUP = 4, 5


class Refused(Exception):
    """What the command refuses, in one line."""


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
        raise Refused(f"--until is at most {MAX_DAYS} days ahead: the access password ends")
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
            first = ask("Access password: ")
            second = ask("The same again: ")
        except getpass.GetPassWarning:
            raise Refused("this terminal cannot hide the password") from None
    if first != second:
        raise Refused("the two passwords differ")
    return check_password(first)


def random_password(choice=secrets.choice):
    """GROUPS groups of GROUP characters of ALPHABET, joined by '-' (23 characters, about 99 bits)."""
    return "-".join("".join(choice(ALPHABET) for _ in range(GROUP)) for _ in range(GROUPS))


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


def main(argv, stdin=sys.stdin, stdout=sys.stdout, ask=getpass.getpass, now=None,
         make=random_password):
    parser = argparse.ArgumentParser(description="Writes the file of hjv-api's access password.")
    parser.add_argument("--until", required=True, help="the day it ends at 0:00 KST, YYYY-MM-DD")
    parser.add_argument("--out", required=True, help="the file to write (it must not be there)")
    parser.add_argument("--random", action="store_true",
                        help="make a password and show it once, instead of asking for one")
    args = parser.parse_args(argv)
    try:
        # write_new needs O_NOFOLLOW, which Windows' Python lacks: refuse before anything is asked
        if not hasattr(os, "O_NOFOLLOW"):
            raise Refused("run this in a Linux, macOS or WSL terminal")
        until = until_of(args.until, now or datetime.datetime.now(datetime.timezone.utc))
        if os.path.lexists(args.out):
            raise Refused(f"{args.out} is there already: this writes no file over another")
        if not stdin.isatty():
            raise Refused("the password is asked or shown in a terminal: run this in one, with "
                          "nothing piped in")
        if args.random:
            # shown only where a person reads it, never into a file or a pipe
            if not stdout.isatty():
                raise Refused("--random shows the password on the terminal: run this with its "
                              "output going to one")
            password = check_password(make())
        else:
            password = ask_password(ask)
        text = (f"# hjv-api's access password (HJV_API_GUEST_FILE), by deploy/guest-conf.py\n"
                f"HASH={hash_of(password, secrets.token_bytes(SALT_BYTES))}\n"
                f"UNTIL={until}\n")
        write_new(args.out, text)
    except Refused as e:
        print(f"guest-conf.py: {e}", file=sys.stderr)
        return 1
    except OSError as e:
        print(f"guest-conf.py: {args.out}: {e.strerror}", file=sys.stderr)
        return 1
    print(os.path.abspath(args.out), file=stdout)
    print(f"UNTIL={until}", file=stdout)
    if args.random:
        print(f"The access password, shown this once: {password}", file=stdout)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# The operations drill of hjv-api (design v3 §5-9, WP-O; the record is docs/hjv-api-drill.md): a
# test server in a container with systemd as PID 1 (Ubuntu 24.04, the production server's
# release), with hjv-api installed as docs/server-runbook.md section 7 installs it, from the files
# the server package carries (bin/hjv-api; systemd/ with the hjviewer-api* units, the env example
# and journald@hjviewer.conf), the fake SSO, a fake hbbs console and the test keys of
# server/api/tests/common. Then, in order:
#   1. install, keys as credentials (systemd-creds; command key 1 made on the server with keygen,
#      command key 2 offline), settings, start, the login mode (open: two IDs imported ahead, the
#      third registered at its first login), timers (7-3 to 7-5)
#   2. a day of use (logins, PCs bound and syncing; server/api/tests/drill.rs), a NAS relay
#      approved by its code and a PC woken through it (Wake-on-LAN, WP-W2), the check, the daily
#      backup, the journal namespace and its 30 days
#   3. alerts through a stand-in for Telegram: the test message, the SSO's invalid_client, a crash
#      of hjviewer-api.service, the socket stopped, PCs that are on but stop syncing (waits about
#      12 minutes; HJV_DRILL_QUICK=1 skips it), PCs that are off
#   4. the restore of the morning's backup after changes (a PC that syncs right after the start
#      gets a disabled list: a restored backup starts off; an account blocked since is blocked
#      again by the operator record's tag), then switching back on and lifting that block
#   5. the snapshot before an upgrade, an upgrade the old binary cannot go back from, going back
#      (the snapshot restored, the restore's steps after it)
#   6. old backups going after 14 days
#   7. the host rebuilt: a new server with the seal key from its offline copy, list key 2 and
#      command key 2 instead of keys 1, a new log key, and the last backup restored; the relay of
#      the backup takes a wake command signed with command key 2
# Nothing calls the real SSO or Telegram, nothing leaves this machine, and the containers go at the
# end (the image hjv-drill:local stays for the next run). The output is build/drill/drill.log.
#   tools/e2e/hjv-api-drill.sh
# Needs cargo, Docker through sudo (as scripts/ci.sh), and Ubuntu's apt sources for the image.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
WORK="${HJV_DRILL_DIR:-$ROOT/build/drill}"
TARGET="${CARGO_TARGET_DIR:-$WORK/target}"
IMAGE=hjv-drill:local
A=hjv-drill-a
B=hjv-drill-b
REGISTRY="${HJV_REGISTRY:-public.ecr.aws/docker/library}"
export PATH="$HOME/.cargo/bin:$PATH"
mkdir -p "$WORK"
exec > >(tee "$WORK/drill.log") 2>&1

step() { printf '\n== %s\n' "$*"; }
fail() {
  echo "FAIL: $*"
  exit 1
}
d() { sudo docker "$@"; }
# a script on a host, as root, with these variables
on() { # <host> [NAME=value...] <<'EOF' script EOF
  local host=$1 envs=()
  shift
  for e in "$@"; do envs+=(-e "$e"); done
  d exec -i "${envs[@]}" "$host" bash -s
}
# a phase of the drill's client on a host
client() { # <host> <phase> [NAME=value...]
  local host=$1 phase=$2 envs=()
  shift 2
  for e in "$@"; do envs+=(-e "$e"); done
  d exec "${envs[@]}" "$host" /pkg/drill/drill-client --ignored --exact "$phase" --nocapture --test-threads 1 2>&1 |
    grep -vE '^(running [0-9]+ test|test result: |$)' || fail "the drill client's $phase"
}
# a credential from a file on the removable medium, through standard input (runbook 7-4)
cred_from() { # <host> <credential> <file>
  d exec -i "$1" systemd-creds encrypt -q --name="$2" - "/etc/credstore.encrypted/$2" < "$3"
}
cleanup() { d rm -f "$A" "$B" > /dev/null 2>&1 || true; }
trap cleanup EXIT

step "build: hjv-api, the fake SSO and the drill's client"
api="$ROOT/server/api"
(cd "$api" && HJV_API_COMMIT=$(git -C "$ROOT" rev-parse HEAD) CARGO_TARGET_DIR="$TARGET" \
  cargo build -q --release --locked --bin hjv-api)
(cd "$api" && CARGO_TARGET_DIR="$TARGET" cargo build -q --release --locked --features fake-sso --bin hjv-fake-sso)
drill_client=$(cd "$api" && CARGO_TARGET_DIR="$TARGET" cargo test -q --release --locked --test drill --no-run \
  --message-format=json 2> /dev/null | python3 -I -c '
import json, sys
for line in sys.stdin:
    m = json.loads(line)
    if m.get("reason") == "compiler-artifact" and m["target"]["name"] == "drill" and m.get("executable"):
        print(m["executable"])')
[ -x "$drill_client" ] || fail "the drill client was not built"

step "the files of the server package (scripts/package-server.sh lays them out so)"
stage="$WORK/stage"
rm -rf "$stage"
mkdir -p "$stage/bin" "$stage/systemd" "$stage/drill"
install -m 755 "$TARGET/release/hjv-api" "$stage/bin/"
install -m 644 "$ROOT"/deploy/systemd/hjviewer-api*.service "$ROOT"/deploy/systemd/hjviewer-api*.socket \
  "$ROOT"/deploy/systemd/hjviewer-api*.timer "$ROOT/deploy/journald@hjviewer.conf" \
  "$api/hjviewer-api.env.example" "$stage/systemd/"
install -m 755 "$TARGET/release/hjv-fake-sso" "$stage/drill/"
install -m 755 "$drill_client" "$stage/drill/drill-client"
ls -l "$stage"/*
# the test keys, and the "removable medium" with the offline copies (seal key, list key 2)
eval "$("$stage/drill/drill-client" --ignored --exact drill_values --nocapture --test-threads 1 2> /dev/null | grep -E '^[A-Z_0-9]+=')"
offline="$WORK/offline"
rm -rf "$offline"
install -d -m 700 "$offline"
printf '%s\n' "$SEAL_KEY" > "$offline/hjviewer-seal.key"
printf '%s\n' "$LIST_KEY_2" > "$offline/hjviewer-list-2.key"
# command key 2, made offline as the runbook makes it (design v3 §5-3). The screen shows two
# lines: its public key, the entry the relays are built with next to key 1's, and its fingerprint,
# which the one who made it compares with selfcheck's later
screen=$("$stage/bin/hjv-api" keygen command --kid 2 --out "$offline/hjviewer-command-2.key" 2>&1) ||
  fail "keygen command --kid 2: $screen"
COMMAND_PUBKEY_2=$(printf '%s\n' "$screen" | sed -n '1p')
COMMAND_FINGERPRINT_2=$(printf '%s\n' "$screen" | sed -n 's/^fingerprint: //p')
case $COMMAND_PUBKEY_2 in 2:*) ;; *) fail "keygen command --kid 2: $screen" ;; esac
case $COMMAND_FINGERPRINT_2 in 2:*) ;; *) fail "keygen command --kid 2 showed no fingerprint" ;; esac
TOKEN=123456:TEST-not-a-real-bot-token_x

step "image: Ubuntu 24.04 with systemd ($IMAGE)"
if ! d image inspect "$IMAGE" > /dev/null 2>&1; then
  printf '%s\n' "FROM $REGISTRY/ubuntu:24.04" \
    'RUN apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends systemd systemd-sysv dbus sqlite3 python3 ca-certificates e2fsprogs > /dev/null && rm -rf /var/lib/apt/lists/*' \
    'STOPSIGNAL SIGRTMIN+3' 'CMD ["/sbin/init"]' | d build -q -t "$IMAGE" - > /dev/null
fi

start_host() { # <name>
  d rm -f "$1" > /dev/null 2>&1 || true
  d run -d --name "$1" --hostname "$1" --privileged --cgroupns=private --tmpfs /run --tmpfs /run/lock \
    -v "$stage:/pkg:ro" "$IMAGE" > /dev/null
  local state=""
  for _ in $(seq 100); do
    state=$(d exec "$1" systemctl is-system-running 2> /dev/null || true)
    case $state in running | degraded) break ;; esac
    sleep 0.2
  done
  echo "$1: systemd $state"
  # systemd sets up a unit's credentials in a mount namespace and moves them over: they reach the
  # service only when / propagates mounts, as on a server (Docker makes it private)
  d exec "$1" mount --make-rshared /
  # the stand-ins: the SSO, hbbs's console, Telegram (it keeps what it was sent)
  on "$1" << 'EOF'
set -euo pipefail
mkdir -p /var/tmp/hjv-drill
systemd-run -q --unit=drill-sso /pkg/drill/hjv-fake-sso --listen 127.0.0.1:18600
systemd-run -q --unit=drill-hbbs /pkg/drill/drill-client --ignored --exact drill_fake_hbbs --test-threads 1
systemd-run -q --unit=drill-telegram python3 -I -c '
import http.server, json
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers["Content-Length"]))
        with open("/var/tmp/hjv-drill/telegram.log", "ab") as f:
            f.write(self.path.encode() + b" " + body + b"\n")
        self.send_response(200)
        self.end_headers()
        self.wfile.write(json.dumps({"ok": True, "result": {}}).encode())
    def log_message(self, *a):
        pass
http.server.HTTPServer(("127.0.0.1", 18650), H).serve_forever()'
EOF
}

# runbook 7-3: the binary, the units and the settings, nothing started
install_api() { # <host>
  on "$1" << 'EOF'
set -euo pipefail
cd /pkg
getent passwd hjviewer-api > /dev/null || useradd --system --user-group --home-dir /var/lib/hjviewer-api --no-create-home --shell /usr/sbin/nologin hjviewer-api
install -d -m 755 /opt/hjviewer/bin /etc/hjviewer
install -m 755 bin/hjv-api /opt/hjviewer/bin/
install -m 644 systemd/hjviewer-api.env.example /etc/hjviewer/hjviewer-api.env
units=()
for u in systemd/hjviewer-api*.socket systemd/hjviewer-api*.service systemd/hjviewer-api*.timer; do
  [ -e "$u" ] || continue
  install -m 644 "$u" /etc/systemd/system/
  case $u in *@.*) units+=("$(basename "${u/@./@check.}")") ;; *) units+=("/etc/systemd/system/$(basename "$u")") ;; esac
done
install -m 644 systemd/journald@hjviewer.conf /etc/systemd/
systemctl daemon-reload
systemd-analyze verify "${units[@]}" && echo UNITS_OK
# the drill's Telegram is the stand-in on this host (the shipped unit sends to api.telegram.org)
install -d /etc/systemd/system/hjviewer-api-alert@.service.d
printf '[Service]\nExecStart=\nExecStart=/opt/hjviewer/bin/hjv-api alert %%i --telegram-api http://127.0.0.1:18650\n' \
  > /etc/systemd/system/hjviewer-api-alert@.service.d/drill.conf
systemctl daemon-reload
EOF
}

# runbook 7-3 and 7-5: the settings of the fake SSO, hbbs's public key, the pinned list keys and
# command keys, and the numbers of the keys in use
settings() { # <host> <list kid> <command kid>
  on "$1" "HBBS_PUBKEY=$HBBS_PUBKEY" "TRUST=$TRUST_PUBKEYS" "CLIENT_ID=$SSO_CLIENT_ID" "KID=$2" \
    "COMMAND=$COMMAND_PUBKEYS" "COMMAND_KID=$3" << 'EOF'
set -euo pipefail
E=/etc/hjviewer/hjviewer-api.env
sed -i -e "s|^HJV_API_SSO_TOKEN_URL=.*|HJV_API_SSO_TOKEN_URL=http://127.0.0.1:18600/oauth/token.do|" \
  -e "s|^HJV_API_SSO_USERINFO_URL=.*|HJV_API_SSO_USERINFO_URL=http://127.0.0.1:18600/api/findByApiUuid.do|" \
  -e "s|^HJV_API_SSO_CLIENT_ID=.*|HJV_API_SSO_CLIENT_ID=$CLIENT_ID|" \
  -e "s|^HJV_API_HBBS_PUBKEY=.*|HJV_API_HBBS_PUBKEY=$HBBS_PUBKEY|" \
  -e "s|^#\?HJV_API_TRUST_PUBKEYS=.*|HJV_API_TRUST_PUBKEYS=$TRUST|" \
  -e "s|^#\?HJV_API_LIST_KID=.*|HJV_API_LIST_KID=$KID|" \
  -e "s|^#\?HJV_API_COMMAND_PUBKEYS=.*|HJV_API_COMMAND_PUBKEYS=$COMMAND|" \
  -e "s|^#\?HJV_API_COMMAND_KID=.*|HJV_API_COMMAND_KID=$COMMAND_KID|" "$E"
grep -cE '^HJV_API_(SSO_TOKEN_URL|SSO_USERINFO_URL|SSO_CLIENT_ID|HBBS_PUBKEY|TRUST_PUBKEYS|LIST_KID|COMMAND_PUBKEYS|COMMAND_KID)=.+' "$E"
EOF
}

# runbook 7-5: start, the files' modes, the switch, selfcheck, the login mode, the timers; two IDs
# imported ahead (the runbook needs none in open mode: Y9001 goes on the list at its first login)
start_api() { # <host>
  on "$1" << 'EOF'
set -euo pipefail
systemctl enable -q --now hjviewer-api.socket hjviewer-api.service
sleep 3
systemctl is-active hjviewer-api.socket hjviewer-api.service
stat -c '%a %U:%G %n' /run/hjviewer-api.sock /var/lib/hjviewer-api /var/lib/hjviewer-api/hjv-api.sqlite3 /var/log/hjviewer-api /var/log/hjviewer-api/operator.log
chattr +a /var/log/hjviewer-api/operator.log 2>&1 || echo "(chattr: not on this container's file system)"
A="/opt/hjviewer/bin/hjv-api admin"
$A passwordless status | head -1
$A selfcheck | grep -E '^\[' | cut -c1-100 || true
$A report | sed -n 2p
printf '2026000001\n2026000002\nsso-invalid-client\n' > /root/hjviewer-allow.txt
chmod 600 /root/hjviewer-allow.txt
$A allow import /root/hjviewer-allow.txt
rm /root/hjviewer-allow.txt
for t in /etc/systemd/system/hjviewer-api-*.timer; do [ -e "$t" ] && systemctl enable -q --now "$(basename "$t")"; done
systemctl list-timers 'hjviewer-api-*' --no-legend | awk '{print $(NF-1), $NF}'
EOF
}

telegram_count() { d exec "$1" sh -c 'wc -l < /var/tmp/hjv-drill/telegram.log 2> /dev/null || echo 0'; }
last_telegram() { # <host>: the text of the last message sent
  on "$1" << 'EOF'
python3 -I -c '
import urllib.parse
line = open("/var/tmp/hjv-drill/telegram.log", "rb").read().decode().splitlines()[-1]
path, body = line.split(" ", 1)
form = dict(urllib.parse.parse_qsl(body))
print("to", path.replace(path.split("/")[1], "bot<token>"), "chat", form["chat_id"])
print(form["text"])'
EOF
}
check_now() { # <host>: runs the check now; prints its result
  on "$1" << 'EOF'
systemctl start hjviewer-api-check.service && echo "check: passed" || echo "check: FAILED (OnFailure= alert)"
sleep 2
journalctl -q --namespace=hjviewer -u hjviewer-api-check -n 4 -o cat --no-pager | tail -n 4
EOF
}

step "host A: a new server ($A)"
start_host "$A"
step "A: runbook 7-3, install"
install_api "$A"
step "A: runbook 7-4, the keys as credentials (systemd-creds; the user's step)"
on "$A" "SSO_BASIC=$SSO_BASIC" "SSO_API_UUID=$SSO_API_UUID" "LOG_KEY=$LOG_KEY" "LIST_KEY_1=$LIST_KEY_1" "TOKEN=$TOKEN" << 'EOF'
set -euo pipefail
install -d -m 700 /etc/credstore.encrypted
c() { systemd-creds encrypt -q --name="$1" - "/etc/credstore.encrypted/$1"; }
printf '%s' "$SSO_BASIC" | c hjviewer-sso-client-secret
printf '%s' "$SSO_API_UUID" | c hjviewer-sso-api-uuid
printf '%s\n' "$LOG_KEY" | c hjviewer-api-log-key
printf '%s\n' "$LIST_KEY_1" | c hjviewer-api-list-key
# the user's Telegram bot (here the stand-in's): README.md, Monitoring
printf 'TELEGRAM_BOT_TOKEN=%s\nTELEGRAM_CHAT_ID=%s\n' "$TOKEN" -1001234 | c hjviewer-api-telegram
EOF
# the seal key from the removable medium
cred_from "$A" hjviewer-api-login-key "$offline/hjviewer-seal.key"
# command key 1, made on the server: its secret goes straight into systemd-creds, its public key
# (the one line on the screen) next to key 2's into the settings and the relay build
screen=$(d exec "$A" /opt/hjviewer/bin/hjv-api keygen command --kid 1 \
  --encrypt-to /etc/credstore.encrypted/hjviewer-api-command-key 2>&1 || true)
COMMAND_PUBKEY_1=$(printf '%s\n' "$screen" | sed -n '/^1:/p')
COMMAND_FINGERPRINT_1=$(printf '%s\n' "$screen" | sed -n 's/^fingerprint: //p')
case $COMMAND_PUBKEY_1 in 1:*) ;; *) fail "keygen command --kid 1 showed no public key" ;; esac
case $COMMAND_FINGERPRINT_1 in 1:*) ;; *) fail "keygen command --kid 1 showed no fingerprint" ;; esac
COMMAND_PUBKEYS="$COMMAND_PUBKEY_1,$COMMAND_PUBKEY_2"
echo "HJV_API_COMMAND_PUBKEYS=$COMMAND_PUBKEYS"
d exec "$A" ls /etc/credstore.encrypted
settings "$A" 1 1
step "A: runbook 7-5, start"
start_api "$A"
# the fingerprint that keygen showed for command key 1 is the one selfcheck shows (the comparison
# a person makes again with the relay build's summary)
case $(d exec "$A" /opt/hjviewer/bin/hjv-api admin selfcheck || true) in
  *"command key in use (HJV_API_COMMAND_KID 1): the pinned key 1, $COMMAND_FINGERPRINT_1"*) echo "command key 1: selfcheck shows the fingerprint keygen showed" ;;
  *) fail "selfcheck does not show the fingerprint keygen showed for command key 1" ;;
esac
check_now "$A"

step "A: a day of use"
client "$A" drill_populate
step "A: Wake-on-LAN: a NAS relay approved by its code, a PC woken through it (WP-W2)"
client "$A" drill_relay
d exec "$A" /opt/hjviewer/bin/hjv-api admin health
echo
check_now "$A"
step "A: the daily backup (the timer's service, now)"
on "$A" << 'EOF'
set -euo pipefail
systemctl start hjviewer-api-backup.service
journalctl -q --namespace=hjviewer -u hjviewer-api-backup -n 5 -o cat --no-pager
stat -c '%a %U:%G %n' /var/lib/hjviewer-api /var/lib/hjviewer-api/backup /var/lib/hjviewer-api/backup/*
f=$(ls /var/lib/hjviewer-api/backup/*.sqlite3 | head -1)
cp "$f" /var/tmp/hjv-drill/morning.sqlite3
echo "rows of the backup:"
sqlite3 -readonly "$f" "SELECT 'accounts', COUNT(*) FROM accounts UNION ALL SELECT 'tokens', COUNT(*) FROM tokens UNION ALL SELECT 'ctl_keys', COUNT(*) FROM ctl_keys UNION ALL SELECT 'devices', COUNT(*) FROM devices UNION ALL SELECT 'binding_tombstones', COUNT(*) FROM binding_tombstones UNION ALL SELECT 'events', COUNT(*) FROM events" | tr '\n' ' '
echo
tail -n 2 /var/log/hjviewer-api/operator.log | sed -E 's/ prev=[0-9a-f]+//'
EOF

step "A: the journal namespace hjviewer and its retention"
on "$A" << 'EOF'
set -euo pipefail
systemd-analyze cat-config systemd/journald@hjviewer.conf | grep -E '^(Storage|MaxRetentionSec|MaxFileSec|SystemMaxUse)='
ls -d /var/log/journal/*.hjviewer
journalctl -q --namespace=hjviewer -o cat --no-pager | wc -l
journalctl -q --namespace=hjviewer --no-pager -o json | python3 -I -c '
import json, sys, collections
units = collections.Counter(json.loads(l).get("_SYSTEMD_UNIT", "?") for l in sys.stdin)
print(dict(units))'
EOF

step "A: alerts: a test message"
on "$A" << 'EOF'
systemctl start hjviewer-api-alert@test.service
journalctl -q --namespace=hjviewer -u hjviewer-api-alert@test.service -o cat --no-pager | tail -n 3
EOF
[ "$(telegram_count "$A")" = 1 ] || fail "no test message"
last_telegram "$A"

step "A: alerts: the SSO refuses this server's client credentials (invalid_client)"
client "$A" drill_invalid_client
check_now "$A"
sleep 3
[ "$(telegram_count "$A")" = 2 ] || fail "no alert for invalid_client"
last_telegram "$A"
check_now "$A"
sleep 3
[ "$(telegram_count "$A")" = 2 ] || fail "the same alert again within six hours"
on "$A" << 'EOF'
journalctl -q --namespace=hjviewer -u 'hjviewer-api-alert@*' -o cat --no-pager | tail -n 3
systemctl restart hjviewer-api.service
EOF

step "A: alerts: hjviewer-api.service crashes (systemd restarts it)"
on "$A" << 'EOF'
systemctl kill -s SIGKILL hjviewer-api.service
sleep 8
systemctl is-active hjviewer-api.service
journalctl -q --namespace=hjviewer -u 'hjviewer-api-alert@hjviewer-api.service.service' -o cat --no-pager | tail -n 3
EOF
[ "$(telegram_count "$A")" = 3 ] || fail "no alert for the crash"
last_telegram "$A"

step "A: alerts: the socket stopped"
on "$A" << 'EOF'
systemctl stop hjviewer-api.socket hjviewer-api.service
EOF
check_now "$A"
sleep 3
[ "$(telegram_count "$A")" = 4 ] || fail "no alert for the stopped socket"
last_telegram "$A"
on "$A" << 'EOF'
systemctl start hjviewer-api.socket hjviewer-api.service
EOF
check_now "$A"

if [ -z "${HJV_DRILL_QUICK:-}" ]; then
  step "A: PCs that are on but stop syncing (their last sync 10 minutes old, on by hbbs for 4)"
  client "$A" drill_sync
  synced=$(date +%s)
  sleep 300
  check_now "$A"
  left=$((synced + 620 - $(date +%s)))
  if [ "$left" -gt 0 ]; then sleep "$left"; fi
  check_now "$A"
  sleep 3
  [ "$(telegram_count "$A")" = 5 ] || fail "no alert for PCs that stopped syncing"
  last_telegram "$A"
  step "A: PCs that are off say nothing"
  d exec "$A" sh -c 'printf "700000001\n700000002\n" > /var/tmp/hjv-drill/off'
  check_now "$A"
  d exec "$A" rm /var/tmp/hjv-drill/off
  client "$A" drill_sync
  check_now "$A"
else
  step "A: (HJV_DRILL_QUICK: no wait for the syncs to age)"
fi

step "A: after the backup: a new login, KIM's PC released by its owner, HONG's keys revoked (a lost phone), LEE blocked"
client "$A" drill_after_backup

step "A: restore of the morning's backup (README.md, Backup and restore; runbook 7-14)"
on "$A" << 'EOF'
set -euo pipefail
B=/var/tmp/hjv-drill/morning.sqlite3   # in the drill: the copy of the morning's backup
A="/opt/hjviewer/bin/hjv-api admin"
D=/var/lib/hjviewer-api/hjv-api.sqlite3
test -s "$B" || { echo 'STOP: no backup file'; exit 1; }
$A passwordless off
systemctl stop hjviewer-api-check.timer
# the database as it is now, kept as a backup that goes after 14 days with the others
$A backup "/var/lib/hjviewer-api/backup/before-restore-$(date -u +%Y%m%d%H%M).sqlite3" | head -2
systemctl stop hjviewer-api.socket hjviewer-api.service
rm -f "$D" "$D-wal" "$D-shm"
install -o hjviewer-api -g hjviewer-api -m 600 "$B" "$D"
systemctl start hjviewer-api.socket hjviewer-api.service
EOF
# the PCs sync every two minutes: one comes before the operator's next command. The backup's
# switch was on (and its keys are those revoked since), but a restored backup starts off
client "$A" drill_sync HJV_DRILL_EXPECT=disabled
on "$A" << 'EOF'
set -euo pipefail
A="/opt/hjviewer/bin/hjv-api admin"
# two lines: every account off, and the relays that came back with the backup
journalctl -q --namespace=hjviewer -u hjviewer-api -o cat --no-pager | grep 'restored backup' | tail -n 2 ||
  echo '(no journal line about the restored backup yet)'
off=$($A passwordless off)
echo "$off"
case $off in *'(no change)'*) ;; *) echo 'FAIL: the restored backup was not off from its start'; exit 1 ;; esac
$A user revoke-all --all
# the blocks made after the backup are lifted by it: made again by the operator record's tags
# (runbook 7-14; here every block of the record came after the backup)
$A allow list | head -1
tags=$(grep -a 'cmd="block" targets=' /var/log/hjviewer-api/operator.log | sed -n 's/.* targets=\([0-9a-f]\{16\}\) .*/\1/p' | sort -u)
[ -n "$tags" ] || { echo 'FAIL: no block in the operator record'; exit 1; }
for t in $tags; do $A block "tag:$t"; done
$A passwordless status | head -1
$A allow list | head -1
$A report | head -7
# the relays are the backup's too: one revoked since would be revoked again here
$A relay list | head -1
EOF
client "$A" drill_after_restore
on "$A" << 'EOF'
set -euo pipefail
/opt/hjviewer/bin/hjv-api admin passwordless on
systemctl start hjviewer-api-check.timer
EOF
client "$A" drill_after_on
check_now "$A"

step "A: before an upgrade: the snapshot; an upgrade the old binary cannot go back from; going back"
sent_before=$(telegram_count "$A")
on "$A" << 'EOF'
set -euo pipefail
A="/opt/hjviewer/bin/hjv-api admin"
D=/var/lib/hjviewer-api/hjv-api.sqlite3
V=9.9.9
$A backup /var/lib/hjviewer-api/backup/before-$V.sqlite3 | head -2
install -d -m 700 /root/hjv-backup
cp -p /etc/hjviewer/hjviewer-api.env /root/hjv-backup/hjviewer-api.env.before-$V
cp -p /opt/hjviewer/bin/hjv-api /root/hjv-backup/hjv-api.before-$V
before=$($A report | head -1)
# the new version's first start upgrades the schema (one past the schemas the old binary knows)
systemctl stop hjviewer-api-check.timer hjviewer-api.socket hjviewer-api.service
next=$(($(sqlite3 "$D" 'PRAGMA user_version;') + 1))
sqlite3 "$D" "PRAGMA user_version = $next;"
systemctl start hjviewer-api.socket hjviewer-api.service || true
sleep 7
systemctl is-active hjviewer-api.service || true
journalctl -q --namespace=hjviewer -u hjviewer-api -n 30 -o cat --no-pager | grep -m1 "schema $next" || true
# going back: the old binary, the snapshot, the settings, together
systemctl stop hjviewer-api.socket hjviewer-api.service
systemctl reset-failed hjviewer-api.service hjviewer-api.socket 2> /dev/null || true
install -m 755 /root/hjv-backup/hjv-api.before-$V /opt/hjviewer/bin/hjv-api
install -m 644 /root/hjv-backup/hjviewer-api.env.before-$V /etc/hjviewer/hjviewer-api.env
rm -f "$D" "$D-wal" "$D-shm"
install -o hjviewer-api -g hjviewer-api -m 600 /var/lib/hjviewer-api/backup/before-$V.sqlite3 "$D"
systemctl start hjviewer-api.socket hjviewer-api.service hjviewer-api-check.timer
sleep 2
systemctl is-active hjviewer-api.service
after=$($A report | head -1)
echo "before: $before"
echo "after:  $after"
[ "$before" = "$after" ] && echo "SNAPSHOT_OK"
# the snapshot is a restored backup: off from its start; then the restore's steps
$A passwordless status | head -1
$A passwordless off
$A user revoke-all --all
$A passwordless on
EOF
sleep 3
# the failing start is an alert of its own, not the crash's again (how it ended differs)
[ "$(telegram_count "$A")" -gt "$sent_before" ] || fail "no alert for the start that failed"
last_telegram "$A"

step "A: backups older than 14 days go, other files stay"
on "$A" << 'EOF'
set -euo pipefail
cd /var/lib/hjviewer-api/backup
cp before-9.9.9.sqlite3 hjv-api-20260901-033000Z.sqlite3
touch -d '20 days ago' hjv-api-20260901-033000Z.sqlite3
printf 'x' > notes.txt
touch -d '20 days ago' notes.txt
systemctl start hjviewer-api-backup.service
journalctl -q --namespace=hjviewer -u hjviewer-api-backup -n 2 -o cat --no-pager
ls
EOF
on "$A" << 'EOF'
f=$(ls -t /var/lib/hjviewer-api/backup/hjv-api-*.sqlite3 | head -1)
cp "$f" /var/tmp/hjv-drill/last.sqlite3
EOF
d cp "$A:/var/tmp/hjv-drill/last.sqlite3" "$WORK/last.sqlite3"
d cp "$A:/var/tmp/hjv-drill/state.json" "$WORK/state.json"

step "host B: the server rebuilt ($B): seal key from the offline copy, list key 2, command key 2, a new log key"
start_host "$B"
install_api "$B"
on "$B" "SSO_BASIC=$SSO_BASIC" "SSO_API_UUID=$SSO_API_UUID" << 'EOF'
set -euo pipefail
install -d -m 700 /etc/credstore.encrypted
c() { systemd-creds encrypt -q --name="$1" - "/etc/credstore.encrypted/$1"; }
printf '%s' "$SSO_BASIC" | c hjviewer-sso-client-secret
printf '%s' "$SSO_API_UUID" | c hjviewer-sso-api-uuid
/opt/hjviewer/bin/hjv-api keygen log-key --encrypt-to /etc/credstore.encrypted/hjviewer-api-log-key
EOF
cred_from "$B" hjviewer-api-login-key "$offline/hjviewer-seal.key"
cred_from "$B" hjviewer-api-list-key "$offline/hjviewer-list-2.key"
cred_from "$B" hjviewer-api-command-key "$offline/hjviewer-command-2.key"
d exec "$B" ls /etc/credstore.encrypted
settings "$B" 2 2
d cp "$WORK/last.sqlite3" "$B:/var/tmp/hjv-drill/last.sqlite3"
d cp "$WORK/state.json" "$B:/var/tmp/hjv-drill/state.json"
on "$B" << 'EOF'
set -euo pipefail
# the database from the backup, before the first start
install -d -o hjviewer-api -g hjviewer-api -m 700 /var/lib/hjviewer-api
install -o hjviewer-api -g hjviewer-api -m 600 /var/tmp/hjv-drill/last.sqlite3 /var/lib/hjviewer-api/hjv-api.sqlite3
systemctl enable -q --now hjviewer-api.socket hjviewer-api.service
sleep 3
A="/opt/hjviewer/bin/hjv-api admin"
$A passwordless off
$A user revoke-all --all
$A selfcheck | grep -E 'list key|command key' | cut -c1-120
$A passwordless on
for t in hjviewer-api-check.timer hjviewer-api-backup.timer; do systemctl enable -q --now "$t"; done
EOF
# the offline key's fingerprint, as its maker saw it, is the one selfcheck shows on the new host
case $(d exec "$B" /opt/hjviewer/bin/hjv-api admin selfcheck || true) in
  *"command key in use (HJV_API_COMMAND_KID 2): the pinned key 2, $COMMAND_FINGERPRINT_2"*) echo "command key 2: selfcheck shows the fingerprint keygen showed" ;;
  *) fail "selfcheck does not show the fingerprint keygen showed for command key 2" ;;
esac
client "$B" drill_rebuilt
check_now "$B"

step "the drill passed"

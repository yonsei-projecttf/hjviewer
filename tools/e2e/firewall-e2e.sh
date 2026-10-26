#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# End-to-end check of the whole connection path behind firewalls, on one Linux host.
#
# Two network namespaces stand in for a customer's PC (hjv-pc) and the controlling device
# (hjv-ctl). Their firewalls let nothing out but TCP 443 to the server, like a strict corporate
# network. The server (Caddy on 443 -> hbbs/hbbr, the deploy/Caddyfile) runs on the host. The
# controlled side registers and serves a test page on its loopback; the controller logs in with the
# password through the server and fetches that page over a port forward, so the bytes cross the
# whole path a session takes: rendezvous, then relay (strict) or WebRTC P2P (open: UDP allowed
# between the peers), then the encrypted, password-authenticated session. Last, it opens a remote
# desktop session and checks that the decoded screen is the PC's (a virtual X display).
#
#   tools/e2e/firewall-e2e.sh [strict|open]
#   HJV_E2E_BITRATE=1 tools/e2e/firewall-e2e.sh strict   # also measures the session's video bitrate
#
# Runs as a normal user and uses sudo for namespaces and iptables. Needs:
#   HJV_E2E_WORK  scratch directory; $HJV_E2E_WORK/srv/id_ed25519(.pub) is the server key
#   CADDY         caddy binary
#   the patched hbbs/hbbr (scripts/fetch-upstream.sh server, cargo build --release), and a client
#   built with the test feature against this server:
#     HJV_SERVER=hjv.test HJV_SERVER_KEY=$(cat $HJV_E2E_WORK/srv/id_ed25519.pub) \
#       cargo build --release --features linux-pkg-config,hjv-e2e
#   PC_CLIENT     optional: the client the PC runs, e.g. the same build installed as
#                 /usr/local/lib/hjviewer-e2e/rustdesk (Linux counts programs under /usr as
#                 installed); the controllers run CLIENT
#
# HJV_E2E_ACCOUNT=1 adds the same-account logins (design v3 7-4, docs/e2e.md): hjv-api and the fake
# Yonsei SSO on the host's loopback behind Caddy's account paths, a second controller (hjv-ctl2)
# and a second PC (hjv-pc2), and the account scenarios in strict mode (S21 in open
# mode). It also needs:
#   HJV_API, HJV_FAKE_SSO  hjv-api and hjv-fake-sso (server/api, cargo build --features fake-sso)
#   HJV_E2E_KEYS  the E2E's keys (scripts/ci.sh e2e_keys: seal.secret, list-1.secret, log.secret,
#                 trust.pub), the client built with HJV_LOGIN_SEAL_PUBKEY and HJV_TRUST_PUBKEYS
#                 from them, and PC_CLIENT installed under /usr
#   FAKETIME_LIB  optional: libfaketime.so.1 (S27; skipped without it)
#
# The minimum version (its design 2-2, 2-4; docs/e2e.md), in strict mode after the parts above:
# HJV_E2E_MIN_CONTROLLER=1 gives hbbs (server patch 0005) the file $RUN/min-controller.conf, which
# V1, V2 and V4 write as an operator would: a controller below MIN is refused with the file's text and
# connects again once MIN is emptied. HJV_E2E_UPDATE_KEYS=1 says that CLIENT and PC_CLIENT were
# built with the update test keys (hjv-release test-keys --env): such an app shows its own text R1
# for a refusal for its version, and with HJV_RELEASE, tools/hjv-release, which signs the test
# policies (test-sign, the test key 1), Caddy serves the policy at /api/hjv/version (a site block of
# that path in the run's Caddyfile; the public deploy/Caddyfile serves none). The PC's service
# takes it by itself; V5-V7 check that it lets a controller below the minimum in before the
# deadline and refuses its login once the deadline has passed. V9 serves a policy signed with the
# test key 3, which no E2E build pins, and starts the PC's service again: the PC refuses the login
# by the policy it stored, refuses the new one and takes nothing from it. It does the same with a
# policy of the test key 1 that carries the signature of another one, which only the signature
# itself gives away.
#
# The staff-only gate (docs/access-tickets.md; docs/e2e.md, the access stage), in strict mode with
# the account part, after it and before the minimum version's parts (whose V7 leaves the PC
# refusing the E2E's controllers by their version): HJV_E2E_ACCESS=1 gives hbbs (server patches
# 0006-0008) the file $RUN/access.conf, which A1-A11 and A13-A14 write as an operator would (MODE off, count
# and enforce, NOT_BEFORE, a broken file), and hjv-api the ticket key of HJV_E2E_KEYS
# (ticket.secret, ticket.pub), the access password's file and the App Review account's, whose demo
# PC is the PC. A second PC, PC B, runs CLIENT (not installed, logged in nowhere) in hjv-pc2's
# network. Between A10 and A11, C1-C7 run the support codes (access-tickets.md 3-7): alice makes
# one, PC B trades it for its ticket and registers, and the code works once.
#
# 1.9.0 (docs/e2e.md, the scenarios marked TV-W2, W3, W5, W6, W9 and UX-G1): the connection
# manager's mark and the controller's recording (S5′, S6′, S9′, X1′, H-hide), the end-of-session
# lock (EL1-EL4, with HJV_E2E_LOGON=1 on the PC's service from S15 on), the cut of open
# password-less sessions (S33-S38, background streams beside S17, S12, S19, S25, S26 and X6), the
# incoming connections (I1-I5), and in the access part W6-E1 to W6-E7: the support link (its page
# and package from hjv-api's version policy and package folder, which Caddy's policy serves too),
# the waiting room, the staff mark on PC B and the input it drops before [수락].
#
# 1.9.1 (docs/e2e.md, DEL1-DEL4), in the access part's network after A14: alice's controller
# deletes her account with the client's own signed request (client patch 0161's
# --hjv-e2e-delete-account), her other devices get 401, the PC her login bound takes the signed
# revoked and needs its password again, her support codes end; the very same request again gets
# the first answer; the App Review account deletes itself and its next login makes a new, empty
# account; and neither door 2's page (HJV_API_DOOR2_FILE, written here) nor the support link page
# has a pilot sentence any more.
#
# HJV_E2E_HOLD=<seconds> (at most 3600): a run that fails keeps its namespaces, servers and
# clients up that long, or until $RUN/release exists, before it cleans up, so that the state can
# be looked at (logs, --hjv-e2e-option through in_ns) without running it again.
set -euo pipefail

MODE=${1:-strict}
case "$MODE" in strict | open) ;; *) echo "usage: $0 [strict|open]" >&2; exit 2 ;; esac
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
WORK=${HJV_E2E_WORK:?set HJV_E2E_WORK to a scratch directory}
CADDY=${CADDY:?set CADDY to a caddy binary}
HBBS=${HBBS:-$ROOT/upstream/rustdesk-server/target/release/hbbs}
HBBR=${HBBR:-$ROOT/upstream/rustdesk-server/target/release/hbbr}
CLIENT=${CLIENT:-$ROOT/upstream/rustdesk/target/release/rustdesk}
PC_CLIENT=${PC_CLIENT:-$CLIENT}
PROBE=${PROBE:-$ROOT/tools/hjv-probe/target/release/hjv-probe}
DOMAIN=hjv.test
HTTPS_PORT=18443 # Caddy's unprivileged port; the namespaces DNAT 443 to it
HBBS_PORT=41116  # hbbs/hbbr ports stay off the namespaces' allow list
HBBR_PORT=41117
USER_ID=$(id -u)
GROUP_ID=$(id -g)
PASSWORD="Hjv-e2e-$RANDOM$RANDOM"
TOKEN="hjviewer-e2e-$RANDOM$RANDOM"
RUN="$WORK/run-$MODE"
OLD_FORWARD=$(sysctl -n net.ipv4.ip_forward)
SERVER_PIDS=()
ACCOUNT=${HJV_E2E_ACCOUNT:-}
NAMESPACES=(hjv-pc hjv-ctl)
if [ -n "$ACCOUNT" ]; then
  HJV_API=${HJV_API:?set HJV_API to hjv-api for HJV_E2E_ACCOUNT=1}
  HJV_FAKE_SSO=${HJV_FAKE_SSO:?set HJV_FAKE_SSO to hjv-fake-sso for HJV_E2E_ACCOUNT=1}
  KEYS=${HJV_E2E_KEYS:?set HJV_E2E_KEYS to the E2E keys for HJV_E2E_ACCOUNT=1}
  case "$PC_CLIENT" in
    /usr/*) ;;
    *) echo "HJV_E2E_ACCOUNT=1: PC_CLIENT must be the client installed under /usr (hjv-api binds installed PCs only)" >&2; exit 2 ;;
  esac
  [ "$MODE" = strict ] && NAMESPACES+=(hjv-ctl2 hjv-pc2)
fi
# the minimum version's parts run in strict mode only
MIN_HBBS="" KEYED="" POLICY=""
if [ "$MODE" = strict ]; then
  MIN_HBBS=${HJV_E2E_MIN_CONTROLLER:-} KEYED=${HJV_E2E_UPDATE_KEYS:-}
  if [ -n "$KEYED" ] && [ -n "${HJV_RELEASE:-}" ]; then POLICY=1; fi
fi
# the staff-only gate's parts run in strict mode only, with hjv-api, which signs the tickets
ACCESS=""
if [ "$MODE" = strict ] && [ -n "${HJV_E2E_ACCESS:-}" ]; then
  [ -n "$ACCOUNT" ] || { echo "HJV_E2E_ACCESS=1 needs HJV_E2E_ACCOUNT=1: hjv-api signs the tickets" >&2; exit 2; }
  ACCESS=1
  GUEST_PW="hjv-e2e-guest-$RANDOM$RANDOM" WRONG_GUEST_PW="hjv-e2e-wrong-$RANDOM$RANDOM"
  REVIEW_ID=E2EREVIEW-TEST REVIEW_PW="hjv-e2e-review-$RANDOM$RANDOM"
fi

log() { printf '\n== %s\n' "$*"; }

# <seq> <deadline> [<test key> [<seq of an earlier policy>]]: signs a policy of epoch 1 and that seq
# with the test key 1 (hjv-release test-sign), or with <test key> as it is (--raw, as the tool signs
# for a key that no E2E build pins), keeps the envelope as envelope-<seq>.json and serves it at
# https://hjv.test/api/hjv/version, replaced at once as the server's file is. With <seq of an
# earlier policy> of the same key, the envelope served carries that policy's signature in place of
# its own. Its one entry, windows, is the one an E2E build on Linux is held to: NEXT_MINOR from
# <deadline> on, and nothing before it (grace_until with grace_min 0.0.0, the pair of the trust
# rules). Its package is made up: Linux downloads nothing.
serve_policy() {
  local text="$RUN/policy/policy-$1.json" signed="$RUN/policy/envelope-$1.json" key=()
  [ -z "${3:-}" ] || key=(--key "$3" --raw)
  printf '{"v":1,"epoch":1,"seq":%s,"issued":"%s","server":"%s","platforms":{"windows":{"min":"%s","latest":"%s","grace_until":"%s","grace_min":"0.0.0","package":{"file":"HJViewer-%s-windows-x86_64.exe","size":1,"sha256":"%s","build":"%s.0"}}}}\n' \
    "$1" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$DOMAIN" "$NEXT_MINOR" "$NEXT_MINOR" "$2" "$NEXT_MINOR" \
    "$(printf 'e2e0%.0s' $(seq 16))" "$NEXT_MINOR" >"$text"
  if ! timeout 30 "$HJV_RELEASE" test-sign "${key[@]}" --policy "$text" --out "$signed" \
    >>"$RUN/hjv-release.log" 2>&1; then
    cat "$RUN/hjv-release.log"
    echo "FAIL: hjv-release test-sign $text"
    exit 1
  fi
  if [ -z "${4:-}" ]; then
    cp "$signed" "$RUN/policy/version-policy.json.new"
  elif ! python3 -I -c '
import json, sys
envelope, other = (json.load(open(path, encoding="utf-8")) for path in sys.argv[1:3])
if envelope["k"] != other["k"] or envelope["p"] == other["p"] or envelope["s"] == other["s"]:
    sys.exit("not a signature of the same key over another text")
envelope["s"] = other["s"]
print(json.dumps(envelope, ensure_ascii=False, separators=(",", ":")))' "$signed" "$RUN/policy/envelope-$4.json" \
    >"$RUN/policy/version-policy.json.new"; then
    echo "FAIL: the policy $1 with the signature of the policy $4"
    exit 1
  fi
  mv -f "$RUN/policy/version-policy.json.new" "$RUN/policy/version-policy.json"
}

# <MIN> [<FROM> <TEXT>]: hbbs's file of the lowest controller version, written next to it and moved
# over it as an operator does; MIN alone, empty, turns the minimum off
min_conf() {
  {
    echo "# the E2E's lowest controller version (server patch 0005), read again every 10 seconds"
    echo "MIN=$1"
    if [ -n "$1" ]; then
      echo "FROM=$2"
      echo "TEXT=$3"
    fi
  } >"$RUN/min-controller.conf.new"
  mv -f "$RUN/min-controller.conf.new" "$RUN/min-controller.conf"
}

# <MODE> [<NOT_BEFORE>]: hbbs's access file (server patch 0006) with the E2E's ticket key 1 and no
# TEXT line, written next to it and moved over it as an operator does (docs/server-runbook.md 9-3)
access_conf() {
  access_file "MODE=$1" "KEY=1:$(cat "$KEYS/ticket.pub")" "NOT_BEFORE=${2:-}"
}
access_file() { # <line>...: the access file of these lines
  printf '%s\n' "# the E2E's access check (docs/access-tickets.md 6), read again every 10 seconds" "$@" \
    >"$RUN/access.conf.new"
  mv -f "$RUN/access.conf.new" "$RUN/access.conf"
}
# <password>: its HASH= line's value as deploy/guest-conf.py writes it (the tool's function: the
# tool itself asks in a terminal only)
pbkdf2_of() {
  python3 -I -c 'import importlib.util, os, sys
spec = importlib.util.spec_from_file_location("guest_conf", sys.argv[1])
conf = importlib.util.module_from_spec(spec)
spec.loader.exec_module(conf)
print(conf.hash_of(sys.argv[2], os.urandom(16)))' "$ROOT/deploy/guest-conf.py" "$1"
}

# <-I|-D>: the host forwards UDP between hjv-pc and hjv-ctl (open mode)
forward_rules() {
  sudo iptables "$1" FORWARD -i veth-hjvpc -o veth-hjvctl -p udp -j ACCEPT
  sudo iptables "$1" FORWARD -i veth-hjvctl -o veth-hjvpc -p udp -j ACCEPT
}

cleanup() {
  local rc=$? end hold=${HJV_E2E_HOLD:-}
  set +e
  if [ "$rc" != 0 ] && [[ $hold =~ ^[0-9]+$ ]] && [ "$hold" -gt 0 ]; then
    [ "$hold" -le 3600 ] || hold=3600
    echo "HOLD: exit $rc: the test bed stays up for $hold s, or until $RUN/release exists (logs in $RUN)"
    end=$(($(date +%s) + hold))
    while [ ! -e "$RUN/release" ] && [ "$(date +%s)" -lt "$end" ]; do sleep 2; done
  fi
  [ "$MODE" = open ] && forward_rules -D 2>/dev/null
  # a sudo of in_ns that a stopped command stopped (R3, or a run that failed between its stop and
  # go on) would stay stopped: it goes on, and ends with its command below
  sudo pkill -CONT -f '^sudo ip netns exec hjv-' 2>/dev/null
  for ns in "${NAMESPACES[@]}"; do
    sudo ip netns pids "$ns" 2>/dev/null | xargs -r sudo kill 2>/dev/null
  done
  [ ${#SERVER_PIDS[@]} -gt 0 ] && kill "${SERVER_PIDS[@]}" 2>/dev/null
  [ -n "${API_PID:-}" ] && kill -CONT "$API_PID" 2>/dev/null && kill "$API_PID" 2>/dev/null
  sleep 0.5
  for ns in "${NAMESPACES[@]}"; do
    sudo ip netns pids "$ns" 2>/dev/null | xargs -r sudo kill -9 2>/dev/null
    sudo ip netns del "$ns" 2>/dev/null
    sudo rm -rf "/etc/netns/$ns"
  done
  sudo sysctl -qw net.ipv4.ip_forward="$OLD_FORWARD"
}
trap cleanup EXIT

# <ns> <host veth> <subnet, e.g. 10.77.1> <the other peer's address>
setup_ns() {
  local ns=$1 veth=$2 net=$3 peer=$4
  sudo ip netns add "$ns"
  sudo ip link add "$veth" type veth peer name eth0 netns "$ns"
  sudo ip addr add "$net.1/24" dev "$veth"
  sudo ip link set "$veth" up
  sudo ip netns exec "$ns" ip addr add "$net.2/24" dev eth0
  sudo ip netns exec "$ns" ip link set eth0 up
  sudo ip netns exec "$ns" ip link set lo up
  sudo ip netns exec "$ns" ip route add default via "$net.1"
  sudo mkdir -p "/etc/netns/$ns"
  printf '127.0.0.1 localhost\n%s.1 %s\n' "$net" "$DOMAIN" | sudo tee "/etc/netns/$ns/hosts" >/dev/null
  # a machine ID of its own (the app's uuid): hjv-api tells one account's devices apart by it
  head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n' | sudo tee "/etc/netns/$ns/machine-id" >/dev/null
  local ipt=(sudo ip netns exec "$ns" iptables)
  "${ipt[@]}" -t nat -A OUTPUT -p tcp -d "$net.1" --dport 443 -j DNAT --to-destination "$net.1:$HTTPS_PORT"
  "${ipt[@]}" -P INPUT DROP
  "${ipt[@]}" -P OUTPUT DROP
  "${ipt[@]}" -P FORWARD DROP
  "${ipt[@]}" -A INPUT -i lo -j ACCEPT
  "${ipt[@]}" -A OUTPUT -o lo -j ACCEPT
  "${ipt[@]}" -A INPUT -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
  "${ipt[@]}" -A OUTPUT -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
  "${ipt[@]}" -A OUTPUT -p tcp -d "$net.1" --dport "$HTTPS_PORT" -j ACCEPT
  if [ "$MODE" = open ]; then
    "${ipt[@]}" -A OUTPUT -p udp -d "$peer" -j ACCEPT
    "${ipt[@]}" -A INPUT -p udp -s "$peer" -j ACCEPT
  fi
}

# <ns> <side> <cmd...>: runs cmd as this user inside the namespace, with the side's own HOME, IPC
# directory (both sides are the same user, and the app's IPC lives in /tmp/HJViewer-<uid>) and host
# name (hjv-<side>, so the real one never shows up in output). Without a controlling terminal: from
# a terminal, sudo's use_pty (Ubuntu's default) stops the process group that called it when its
# command stops, so R3's stop of the PC's service would stop this script too.
in_ns() {
  local ns=$1 side=$2
  shift 2
  setsid -w sudo ip netns exec "$ns" unshare --uts bash -c '
    hostname "hjv-$7"
    mount --bind "$1" "/tmp/HJViewer-$2" || exit 1
    mkdir -p /tmp/.X11-unix && mount -t tmpfs -o mode=1777 tmpfs /tmp/.X11-unix || exit 1
    exec setpriv --reuid="$2" --regid="$3" --init-groups \
      env -i HOME="$4" USER="$5" LOGNAME="$5" PATH=/usr/local/bin:/usr/bin:/bin SSL_CERT_FILE="$6" "${@:8}"
  ' _ "$RUN/$side/ipc" "$USER_ID" "$GROUP_ID" "$RUN/$side/home" "$(id -un)" "$RUN/ca-bundle.pem" "$side" "$@"
}

rm -rf "$RUN"
mkdir -p "$RUN"/{pc,ctl}/{home,ipc} "$RUN/srv" "$RUN/site" "/tmp/HJViewer-$USER_ID"
cp "$WORK/srv/id_ed25519" "$WORK/srv/id_ed25519.pub" "$RUN/srv/"
echo "$TOKEN" >"$RUN/site/index.html"
# the PC's screen: a full-screen image, so the controller's decoded screen can be compared with it
convert -size 1280x720 xc:'#1e6fd9' \( -size 760x220 xc:white -fill '#1e6fd9' -pointsize 54 \
  -gravity center -annotate +0+0 "HJViewer E2E\n$MODE firewall" \) -gravity center -composite \
  "$RUN/pc/desktop.png"
# The minimum version: the E2E's clients send the version of the upstream checkout, as every build
# whose version scripts/set-client-version.sh did not stamp; hbbs's MIN is the next patch version
# above it, the policy's minimum the next minor one (1.5.1 and 1.6.0 for 1.5.0). The first policy
# is served from the start, so that the PC's service takes it 30 s after its start, by itself: it
# holds the PC's controllers to NEXT_MINOR from tomorrow on.
if [ -n "$MIN_HBBS$POLICY" ]; then
  mkdir -p "$RUN/version" "$RUN/policy"
  CUR=$(env -i HOME="$RUN/version" PATH=/usr/bin:/bin timeout 10 "$CLIENT" --version) || { echo "FAIL: $CLIENT --version"; exit 1; }
  [[ $CUR =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]] || { echo "FAIL: the client's version '$CUR' is not x.y.z"; exit 1; }
  NEXT_MINOR="${BASH_REMATCH[1]}.$((BASH_REMATCH[2] + 1)).0"
  NEXT_PATCH="${BASH_REMATCH[1]}.${BASH_REMATCH[2]}.$((BASH_REMATCH[3] + 1))"
  [ "${BASH_REMATCH[3]}" -lt 99 ] || NEXT_PATCH=$NEXT_MINOR
  [ -z "$POLICY" ] || serve_policy 1 "$(date -u -d '+1 day' +%Y-%m-%dT%H:%M:%SZ)"
fi

log "network: ${#NAMESPACES[@]} namespaces, firewall '$MODE' (out: TCP 443 to the server$([ "$MODE" = open ] && echo ', UDP between peers'))"
setup_ns hjv-pc veth-hjvpc 10.77.1 10.77.2.2
setup_ns hjv-ctl veth-hjvctl 10.77.2 10.77.1.2
if [ "${#NAMESPACES[@]}" -gt 2 ]; then
  mkdir -p "$RUN"/{ctl2,pc2}/{home,ipc}
  setup_ns hjv-ctl2 veth-hjvctl2 10.77.3 10.77.1.2
  setup_ns hjv-pc2 veth-hjvpc2 10.77.4 10.77.1.2
fi
if [ "$MODE" = open ]; then
  sudo sysctl -qw net.ipv4.ip_forward=1
  # the host routes the peers' UDP; where Docker runs, FORWARD drops what no rule accepts
  forward_rules -I
fi

# The account API (design v3 7-4): the fake Yonsei SSO and hjv-api on the host's loopback, with the
# E2E's test settings: Linux PCs bind, a PC asks every 5 seconds and keeps a list 30 seconds, the
# password-less switch is off as after an install, the first two test accounts are imported ahead
# (open login, the default: the third goes on the allowlist at its first login), and the limits of
# successful logins and SSO calls fit the two dozen logins of one account in a few minutes
API_PORT=41120
SSO_PORT=41600
API_PID=""
start_api() {
  (cd "$RUN/api" && exec env -i RUST_LOG=info "$HJV_API" serve --listen "127.0.0.1:$API_PORT" \
    --env-file "$RUN/api/hjviewer-api.env" --credentials "$RUN/api/creds" --db "$RUN/api/hjv-api.sqlite3" \
    --admin-socket "$RUN/api/admin.sock" --operator-log "$RUN/api/operator.log" >>"$RUN/hjv-api.log" 2>&1) &
  API_PID=$!
  for _ in $(seq 50); do
    curl -s -o /dev/null "http://127.0.0.1:$API_PORT/api/hjv/source" && [ -S "$RUN/api/admin.sock" ] && return 0
    sleep 0.1
  done
  echo "FAIL: hjv-api did not start"
  tail -n 20 "$RUN/hjv-api.log"
  exit 1
}
admin() { "$HJV_API" admin --socket "$RUN/api/admin.sock" "$@"; }
# the access password's file with a day to go (A9 deletes it, W6 writes it again)
guest_conf() {
  printf 'HASH=%s\nUNTIL=%s\n' "$(pbkdf2_of "$GUEST_PW")" "$(date -u -d '+1 day' +%Y-%m-%dT%H:%M:%SZ)" \
    >"$RUN/api/guest.conf.new"
  mv -f "$RUN/api/guest.conf.new" "$RUN/api/guest.conf"
}
if [ -n "$ACCOUNT" ]; then
  log "account API: hjv-api on 127.0.0.1:$API_PORT and the fake Yonsei SSO on 127.0.0.1:$SSO_PORT"
  mkdir -p "$RUN/api"
  install -d -m 700 "$RUN/api/creds"
  # the fake SSO's registration (server/api/src/fake_sso.rs) and the E2E's keys
  printf '%s' aGp2LWZha2UtY2xpZW50Omhqdi1mYWtlLXNlY3JldA== >"$RUN/api/creds/hjviewer-sso-client-secret"
  printf '%s' hjv-fake-api-uuid >"$RUN/api/creds/hjviewer-sso-api-uuid"
  cp "$KEYS/seal.secret" "$RUN/api/creds/hjviewer-api-login-key"
  cp "$KEYS/log.secret" "$RUN/api/creds/hjviewer-api-log-key"
  cp "$KEYS/list-1.secret" "$RUN/api/creds/hjviewer-api-list-key"
  [ -z "$ACCESS" ] || cp "$KEYS/ticket.secret" "$RUN/api/creds/hjviewer-api-ticket-key"
  chmod 600 "$RUN/api/creds"/*
  cat >"$RUN/api/hjviewer-api.env" <<EOF
HJV_API_SSO_TOKEN_URL=http://127.0.0.1:$SSO_PORT/oauth/token.do
HJV_API_SSO_USERINFO_URL=http://127.0.0.1:$SSO_PORT/api/findByApiUuid.do
HJV_API_SSO_CLIENT_ID=hjv-fake-client
HJV_API_HBBS_CONSOLE=127.0.0.1:$((HBBS_PORT - 1))
HJV_API_HBBS_PUBKEY=$(cat "$RUN/srv/id_ed25519.pub")
HJV_API_BIND_OS=windows,linux
HJV_API_TRUST_PUBKEYS=$(cat "$KEYS/trust.pub")
HJV_API_LIST_KID=1
HJV_API_TRUST_REFRESH=5
HJV_API_TRUST_TTL=30
HJV_API_LIMIT_OK_ID=200/3600
HJV_API_LIMIT_OK_IP=200/3600
HJV_API_LIMIT_SSO=400/60
EOF
  if [ -n "$ACCESS" ]; then
    # the access tickets from the start, while hbbs checks none (MODE=off until A2): the E2E's
    # ticket key, the access password's file with a day to go, and the App Review account's,
    # which A10 writes once the PC has its ID; the failures of one address are not what is tried
    cat >>"$RUN/api/hjviewer-api.env" <<EOF
HJV_API_TICKET_PUBKEYS=1:$(cat "$KEYS/ticket.pub")
HJV_API_GUEST_FILE=$RUN/api/guest.conf
HJV_API_REVIEW_FILE=$RUN/api/review.conf
HJV_API_DOOR2_FILE=$RUN/api/door2.conf
HJV_API_LIMIT_GUEST_IP=50/600
EOF
    guest_conf
    # W6-E2's support link hands out the Windows package of the policy Caddy serves: hjv-api reads
    # the same file, and its package folder holds that made-up package (one byte, as the policy
    # says)
    if [ -n "$POLICY" ]; then
      mkdir -p "$RUN/pkg"
      printf x >"$RUN/pkg/HJViewer-$NEXT_MINOR-windows-x86_64.exe"
      printf 'HJV_API_VERSION_POLICY=%s\nHJV_API_PACKAGE_DIR=%s\n' "$RUN/policy/version-policy.json" "$RUN/pkg" \
        >>"$RUN/api/hjviewer-api.env"
    fi
  fi
  "$HJV_FAKE_SSO" --listen "127.0.0.1:$SSO_PORT" >"$RUN/fake-sso.log" 2>&1 &
  SERVER_PIDS+=($!)
  start_api
  printf 'ZZTEST01\nZZTEST02\n' >"$RUN/api/allow.txt"
  admin allow import "$RUN/api/allow.txt"
fi

log "server: hbbs + hbbr behind Caddy (deploy/Caddyfile) on :443"
# (with the minimum version, hbbs reads the lowest controller version from a file, while there is
# one; with the staff-only gate, its access file, MODE=off at first)
HBBS_PID=""
start_hbbs() { # its log appended to $RUN/hbbs.log (A13 starts it again)
  (cd "$RUN/srv" && exec env -i TEST_HBBS=no RUST_LOG=debug ${MIN_HBBS:+"HJV_MIN_CONTROLLER_FILE=$RUN/min-controller.conf"} \
    ${ACCESS:+"HJV_ACCESS_FILE=$RUN/access.conf"} "$HBBS" -p "$HBBS_PORT" -r "$DOMAIN" >>"$RUN/hbbs.log" 2>&1) &
  HBBS_PID=$!
  SERVER_PIDS+=($!)
}
[ -z "$ACCESS" ] || access_conf off
start_hbbs
(cd "$RUN/srv" && exec env -i RUST_LOG=info "$HBBR" -p "$HBBR_PORT" -k _ >"$RUN/hbbr.log" 2>&1) &
SERVER_PIDS+=($!)
# The version policy, which the server's nginx serves from a file (minimum version design 4-2): a
# site block of that path alone, which Caddy puts before deploy/Caddyfile's site of the same host
# (a path in a site address is deprecated in Caddy 2.11, and works)
POLICY_SITE=""
if [ -n "$POLICY" ]; then
  POLICY_SITE="$DOMAIN/api/hjv/version {
	root * $RUN/policy
	rewrite * /version-policy.json
	header Cache-Control no-cache
	header X-Content-Type-Options nosniff
	file_server
}"
fi
# (debug, for the account part: Caddy's log then has every request it passes to hjv-api with the
# status that came back; it leaves the Authorization header's value out)
cat >"$RUN/Caddyfile" <<EOF
{
	${ACCOUNT:+debug}
	local_certs
	skip_install_trust
	admin off
	http_port 18080
	https_port $HTTPS_PORT
	storage file_system $RUN/caddy
}
$POLICY_SITE
import $ROOT/deploy/Caddyfile
EOF
env -i HOME="$RUN" HJV_DOMAIN="$DOMAIN" HJV_HBBS_WS="127.0.0.1:$((HBBS_PORT + 2))" \
  HJV_HBBR_WS="127.0.0.1:$((HBBR_PORT + 2))" HJV_API="127.0.0.1:$API_PORT" \
  "$CADDY" run --config "$RUN/Caddyfile" --adapter caddyfile >"$RUN/caddy.log" 2>&1 &
SERVER_PIDS+=($!)
CA_ROOT="$RUN/caddy/pki/authorities/local/root.crt"
for _ in $(seq 50); do [ -s "$CA_ROOT" ] && break; sleep 0.2; done
cat /etc/ssl/certs/ca-certificates.crt "$CA_ROOT" >"$RUN/ca-bundle.pem"

# The PC's desktop on a virtual display, and its service
start_pc() {
  in_ns hjv-pc pc bash -c '
    Xvfb :99 -screen 0 1280x720x24 -nolisten tcp >"$1/xvfb.log" 2>&1 &
    for _ in $(seq 50); do [ -e /tmp/.X11-unix/X99 ] && break; sleep 0.1; done
    export DISPLAY=:99
    display -geometry +0+0 "$1/desktop.png" &
    exec "$2" --server
  ' _ "$RUN/pc" "$PC_CLIENT" >>"$RUN/pc-server.log" 2>&1 &
}

log "controlled PC (hjv-pc): password, register over wss, serve a test page on its loopback"
ID=$(in_ns hjv-pc pc "$PC_CLIENT" --hjv-e2e-set-password "$PASSWORD" | tail -1)
echo "device ID: $ID"
in_ns hjv-pc pc python3 -m http.server 8099 --bind 127.0.0.1 --directory "$RUN/site" >"$RUN/pc-site.log" 2>&1 &
if [ -n "$ACCOUNT" ]; then
  # the connection manager the service reports each login to, which prints them
  in_ns hjv-pc pc "$PC_CLIENT" --hjv-e2e-cm >"$RUN/pc-cm.log" 2>&1 &
  for _ in $(seq 50); do grep -q 'cm listening' "$RUN/pc-cm.log" && break; sleep 0.1; done
fi
start_pc

log "controller (hjv-ctl): the firewall lets nothing but HTTPS to the server out"
BLOCKED=()
blocked() { # <description> <cmd...>: each waits out its timeout, so they run side by side
  (
    if in_ns hjv-ctl ctl "${@:2}" >/dev/null 2>&1; then
      echo "FAIL: $1 is reachable"
      exit 1
    fi
    echo "blocked as expected: $1"
  ) &
  BLOCKED+=($!)
}
blocked "hbbs TCP $HBBS_PORT (RustDesk's native port)" curl -s --max-time 3 "http://10.77.2.1:$HBBS_PORT/"
blocked "hbbs WebSocket port $((HBBS_PORT + 2)) directly" curl -s --max-time 3 "http://10.77.2.1:$((HBBS_PORT + 2))/"
blocked "UDP to hbbs" bash -c "echo x >/dev/udp/10.77.2.1/$HBBS_PORT"
blocked "the internet" curl -s --max-time 3 https://1.1.1.1/
if [ -n "$ACCOUNT" ]; then
  blocked "hbbs's console port $((HBBS_PORT - 1))" curl -s --max-time 3 "http://10.77.2.1:$((HBBS_PORT - 1))/"
  blocked "hjv-api port $API_PORT directly" curl -s --max-time 3 "http://10.77.2.1:$API_PORT/api/hjv/source"
  blocked "the fake SSO port $SSO_PORT" curl -s --max-time 3 "http://10.77.2.1:$SSO_PORT/_stats"
fi
for pid in "${BLOCKED[@]}"; do wait "$pid" || exit 1; done

log "probe from the controller's network, waiting for the PC to come online"
online=
for _ in $(seq 40); do
  if in_ns hjv-ctl ctl "$PROBE" "$DOMAIN" --ca "$CA_ROOT" --peer "$ID" >"$RUN/probe.log" 2>&1 \
    && grep -q "$ID=online" "$RUN/probe.log"; then
    online=1
    break
  fi
  sleep 1
done
cat "$RUN/probe.log"
[ -n "$online" ] || { echo "FAIL: the PC did not come online"; exit 1; }

log "controller logs in with the password and forwards 127.0.0.1:9099 to the PC's 127.0.0.1:8099"
in_ns hjv-ctl ctl "$CLIENT" --hjv-e2e-forward "$ID" "$PASSWORD" 9099 127.0.0.1 8099 >"$RUN/ctl.log" 2>&1 &
got=
for _ in $(seq 30); do
  got=$(in_ns hjv-ctl ctl curl -s --max-time 20 http://127.0.0.1:9099/index.html || true)
  [ "$got" = "$TOKEN" ] && break
  sleep 1
done
cat "$RUN/ctl.log"
if [ "$got" != "$TOKEN" ]; then
  echo "FAIL: did not get the PC's page through the session (got: '${got}')"
  exit 1
fi
echo "page from the PC, fetched through the session: $got"
# the forward stays up until the end otherwise; the account part opens sessions of its own
sudo ip netns pids hjv-ctl | xargs -r ps -o pid=,args= -p | awk '/--hjv-e2e-forward/ {print $1}' | xargs -r sudo kill 2>/dev/null || true

log "controller opens a remote desktop session and decodes the PC's screen"
in_ns hjv-ctl ctl "$CLIENT" --hjv-e2e-screenshot "$ID" "$PASSWORD" "$RUN/screen.ppm" >"$RUN/ctl-screen.log" 2>&1 || true
cat "$RUN/ctl-screen.log"
[ -s "$RUN/screen.ppm" ] || { echo "FAIL: no screen arrived"; exit 1; }
convert "$RUN/screen.ppm" "$RUN/screen.png"
# normalized RMSE against what the PC shows: ~0.08 for a match (VP9 is lossy), ~0.5 for another image
rmse=$(compare -metric RMSE "$RUN/screen.png" "$RUN/pc/desktop.png" null: 2>&1 | sed -n 's/.*(\(.*\)).*/\1/p' || true)
echo "screen: $RUN/screen.png, RMSE against the PC's desktop: ${rmse:-?}"
if ! awk -v e="${rmse:-1}" 'BEGIN { exit !(e < 0.2) }'; then
  echo "FAIL: the decoded screen is not the PC's desktop"
  exit 1
fi

if [ -n "${HJV_E2E_BITRATE:-}" ]; then
  log "video bitrate of a desktop session (1280x720) for three kinds of screen"
  mkdir -p "$RUN/anim/scroll" "$RUN/anim/noise"
  # a document scrolling at 20 fps, and the whole screen changing at 20 fps (the codec's worst case)
  doc=(convert -size 1280x2400 xc:white -fill black -font DejaVu-Sans -pointsize 24)
  for i in $(seq 60); do
    doc+=(-annotate "+30+$((i * 38))" "Line $i of a long document: remote sessions mostly show text, windows and the odd image.")
  done
  "${doc[@]}" "$RUN/anim/doc.png"
  for i in $(seq 0 39); do
    convert "$RUN/anim/doc.png" -crop "1280x720+0+$((i * 30))" +repage "$RUN/anim/scroll/$(printf %02d "$i").png"
    convert -size 1280x720 xc: +noise Random -seed "$i" "$RUN/anim/noise/$(printf %02d "$i").png"
  done
  for kind in static scroll noise; do
    if [ "$kind" != static ]; then
      in_ns hjv-pc pc env DISPLAY=:99 animate -geometry +0+0 -delay 5 -loop 0 "$RUN"/anim/"$kind"/*.png >/dev/null 2>&1 &
      sleep 3
    fi
    printf '%-7s ' "$kind"
    in_ns hjv-ctl ctl "$CLIENT" --hjv-e2e-stream "$ID" "$PASSWORD" 15 2>/dev/null | grep -o '[0-9]* frames.*' || echo "no video"
    sudo ip netns pids hjv-pc | xargs -r ps -o pid=,args= -p | awk '/animate/ {print $1}' | xargs -r sudo kill 2>/dev/null || true
  done
fi

# ---------------------------------------------------------------------------------------------
# The same-account logins (design v3 7-4). Each step prints PASS or FAIL with what it saw.
# ---------------------------------------------------------------------------------------------
PC_ID=$ID
# the fake SSO's test accounts (server/api/src/fake_sso.rs), fake IDs of HJViewer's ID rule (Z or
# Y, not ZA); CAROL, a Y code, is not imported: her first login puts her on the allowlist (S28)
ALICE=ZZTEST01 ALICE_PW=hjv-test-pass-1 ALICE_NAME=홍길동
BOB=ZZTEST02 BOB_PW=hjv-test-pass-2
CAROL=YZTEST03 CAROL_PW=hjv-test-pass-3
WRONG_PW=hjv-wrong-pass
SECRETS=() # the tokens and account keys the clients held, for S20
LAST_ASK=0 # when a proof last made the PC sync at once

pass() { echo "PASS: $*"; }
fail() {
  echo "FAIL: $*"
  exit 1
}
client_of() { case $1 in pc | pc2) echo "$PC_CLIENT" ;; *) echo "$CLIENT" ;; esac }

# <side> [NAME=value...] <args...>: the side's client in its namespace (hjv-<side>). Its standard
# output is $OUT, its exit code $RC and its run time $MS (ms); both outputs go to $RUN/<side>.out,
# with the portal passwords masked. Prints the hjv-e2e lines, indented.
run() {
  local s=$1 t0 envs=() shown=()
  shift
  while [[ ${1:-} =~ ^[A-Z_][A-Z0-9_]*= ]]; do
    envs+=("$1")
    shift
  done
  for a in "$@"; do
    case "$a" in "$ALICE_PW" | "$BOB_PW" | "$CAROL_PW" | "$WRONG_PW") shown+=("***") ;; *) shown+=("$a") ;; esac
  done
  t0=$(date +%s%N)
  RC=0
  OUT=$(in_ns "hjv-$s" "$s" env "${envs[@]}" "$(client_of "$s")" "$@" 2>"$RUN/$s.err") || RC=$?
  MS=$((($(date +%s%N) - t0) / 1000000))
  {
    echo "== ${envs[*]} ${shown[*]} (exit $RC, $MS ms)"
    printf '%s\n' "$OUT"
    cat "$RUN/$s.err"
  } >>"$RUN/$s.out"
  grep '^hjv-e2e:' <<<"$OUT" | sed 's/^/    /' || true
}

# <side> <tag> [<PC ID>]: the PC's remote screen from <side> without a password ("" as the
# password), written to $RUN/<tag>.ppm
pwless() {
  run "$1" --hjv-e2e-screenshot "${3:-$PC_ID}" "" "$RUN/$2.ppm"
}
has() { grep -q -- "$1" <<<"$OUT"; }
want() { # <exit code> <what>: of the last run
  [ "$RC" = "$1" ] || fail "$2: exit $RC, expected $1"
}
# the decoded screen <tag> is the PC's desktop
screen_is_pc() {
  local e
  [ -s "$RUN/$1.ppm" ] || return 1
  convert "$RUN/$1.ppm" "$RUN/$1.png"
  e=$(compare -metric RMSE "$RUN/$1.png" "$RUN/pc/desktop.png" null: 2>&1 | sed -n 's/.*(\(.*\)).*/\1/p' || true)
  echo "    RMSE against the PC's desktop: ${e:-?}"
  awk -v e="${e:-1}" 'BEGIN { exit !(e < 0.2) }'
}
pwless_ok() { # <side> <tag> <scenario>: a password-less session with the PC's screen
  pwless "$1" "$2"
  want 0 "$3"
  has 'password required' && fail "$3: a password box"
  screen_is_pc "$2" || fail "$3: the decoded screen is not the PC's desktop"
}
pwless_refused() { # <side> <tag> <scenario>: the PC refused the proof: the refusal box (exit 3)
  pwless "$1" "$2"
  want 3 "$3"
  has 'password required Could not connect without a password' ||
    fail "$3: not the refusal box ($(grep 'password required' <<<"$OUT"))"
}
until_ok() { # <seconds> <what> <command...>: retries the command until it succeeds
  local end=$(($(date +%s) + $1))
  until "${@:3}"; do
    [ "$(date +%s)" -lt "$end" ] || fail "$2 (not within $1 s)"
    sleep 0.5
  done
}
# <side> <password> <local port>: a port forwarding to the PC's test page, which logs in when the
# first connection to the local port comes; $OUT and $RC as run's. A forwarding that got in stays
# up, and is ended here (RC 0).
forward_try() {
  local s=$1 port=$3 fpid
  in_ns "hjv-$s" "$s" "$(client_of "$s")" --hjv-e2e-forward "$PC_ID" "$2" "$port" 127.0.0.1 8099 \
    >"$RUN/$s-forward.out" 2>&1 &
  fpid=$!
  sleep 1
  in_ns "hjv-$s" "$s" curl -s --max-time 15 -o /dev/null "http://127.0.0.1:$port/index.html" || true
  # (sudo's process: its owner is root, so its /proc entry says whether it is still there)
  for _ in $(seq 50); do
    [ -e "/proc/$fpid" ] || break
    sleep 0.1
  done
  if [ -e "/proc/$fpid" ]; then
    sudo ip netns pids "hjv-$s" | xargs -r ps -o pid=,args= -p | awk '/--hjv-e2e-forward/ {print $1}' | xargs -r sudo kill
  fi
  RC=0
  wait "$fpid" || RC=$?
  OUT=$(cat "$RUN/$s-forward.out")
  cat "$RUN/$s-forward.out" >>"$RUN/$s.out"
  grep '^hjv-e2e:' <<<"$OUT" | sed 's/^/    /' || true
}
try_rc() { # <side> <tag> <exit code>: one password-less attempt ends with <exit code>
  pwless "$1" "$2"
  [ "$RC" = "$3" ]
}
# The PC's service
pc_log() { cat "$RUN/pc-server.log" "$RUN"/pc/home/.local/share/logs/HJViewer/server/*.log 2>/dev/null || true; }
# (no "… | grep -q" here: with pipefail, grep leaving early fails the pipeline)
cm_since() { # <line count> <pattern>: a line of the connection manager's after the first <count>
  local text
  text=$(tail -n "+$(($1 + 1))" "$RUN/pc-cm.log")
  grep -q -- "$2" <<<"$text"
}
pc_logins() { pc_log | grep -c "hjv: same-account login (" || true; }
pc_log_has() {
  local text
  text=$(pc_log)
  grep -q -- "$1" <<<"$text"
}
pc_log_count() { pc_log | grep -c -- "$1" || true; }
pc_log_count_above() { [ "$(pc_log_count "$1")" -gt "$2" ]; }
# the PC's first sync after its last start ended with a list (its log line)
first_sync_done() {
  local text
  text=$(pc_log | awk '/hjv: same-account logins: sync task started/ { n = NR } { l[NR] = $0 }
    END { for (i = n + 1; i <= NR; i++) if (n && l[i] ~ /hjv: trust list/) { print "done"; exit } }')
  [ "$text" = "done" ]
}
# ms from the last start of the PC's sync task to the list of its first sync, from the log's times
first_sync_ms() {
  pc_log | python3 -I -c '
import datetime, re, sys
lines = sys.stdin.read().splitlines()
starts = [i for i, l in enumerate(lines) if "hjv: same-account logins: sync task started" in l]
def at(line):
    m = re.match(r"\[(\d{4}-\d\d-\d\d)[ T](\d\d:\d\d:\d\d(?:\.\d+)?)", line)
    return datetime.datetime.fromisoformat(m.group(1) + " " + m.group(2)) if m else None
if starts:
    t0 = at(lines[starts[-1]])
    for line in lines[starts[-1] + 1:]:
        if "hjv: trust list" in line and t0 and at(line):
            print(int((at(line) - t0).total_seconds() * 1000))
            break'
}
pc_state() { # hjv-pwless-state, what the account settings show about this PC
  run pc --hjv-e2e-option hjv-pwless-state >/dev/null
  STATE=$OUT
}
state_is() {
  pc_state
  [[ $STATE == "$1"* ]]
}
# <time, date +%s.%N>: the statuses hjv-api gave the PC's sync requests since then, in order (from
# Caddy's debug log of what it passed on)
pc_syncs_since() {
  python3 -I -c 'import json, sys
statuses = []
for line in open(sys.argv[1], encoding="utf-8", errors="replace"):
    if "upstream roundtrip" not in line:
        continue
    try:
        entry = json.loads(line)
    except ValueError:
        continue
    request = entry.get("request", {})
    if (entry.get("ts", 0) >= float(sys.argv[2]) and request.get("uri") == "/api/hjv/pc/sync"
            and request.get("remote_ip") == "10.77.1.2"):
        statuses.append(str(entry.get("status")))
print(" ".join(statuses))' "$RUN/caddy.log" "$1"
}
pc_syncs_match() { # <time> <regex>
  [[ $(pc_syncs_since "$1") =~ $2 ]]
}
binding() { # <name>: a value the service keeps in HJViewer_binding (HKLM on Windows)
  python3 -I -c 'import json, sys
try:
    print(json.load(open(sys.argv[1])).get(sys.argv[2], ""))
except FileNotFoundError:
    print("")' "$RUN/pc/home/.config/hjviewer/HJViewer_binding" "$1"
}
pc_unbound() { [ -z "$(binding Binding)" ]; }
pc_off() { [ -n "$(binding Off)" ]; }
pc_binding_written() { # <value before>: the PC user's LocalConfig hjv-pc-binding changed and is set
  local now
  now=$(local_option pc hjv-pc-binding)
  [ -n "$now" ] && [ "$now" != "$1" ]
}
within_ms() { # <ms> <command...>: retries the command for up to <ms> ms; WITHIN_MS is how long it took
  local t0 end
  t0=$(date +%s%N)
  end=$((t0 + $1 * 1000000))
  until "${@:2}"; do
    [ "$(date +%s%N)" -lt "$end" ] || return 1
    sleep 0.1
  done
  WITHIN_MS=$(ms_since "$t0")
}
set_binding() { # <binding ID>: writes it into HJViewer_binding, as only the service should
  python3 -I -c 'import json, sys
values = json.load(open(sys.argv[1]))
values["Binding"] = sys.argv[2]
json.dump(values, open(sys.argv[1], "w"))' "$RUN/pc/home/.config/hjviewer/HJViewer_binding" "$1"
}
random_binding() { head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n'; } # 32 hex digits
pc_login() { # <account> <password>: the PC's login, which binds it
  run pc --hjv-e2e-login "$1" "$2"
  want 0 "the PC's login"
  BIND_REQ=$(sed -n 's/^hjv-e2e: bind request //p' <<<"$OUT")
  keep_secrets pc
}
pc_bound() { # the PC's login confirmed by hjv-api and on: active
  pc_login "$ALICE" "$ALICE_PW"
  until_ok 10 "the PC's binding confirmed" state_is active
}
stop_pc_service() {
  local pids
  pids=$(sudo ip netns pids hjv-pc | xargs -r ps -o pid=,args= -p | awk '$0 ~ / --server$/ {print $1}')
  [ -n "$pids" ] || fail "the PC's service is not running"
  sudo kill $pids
  for _ in $(seq 50); do
    sudo kill -0 $pids 2>/dev/null || return 0
    sleep 0.1
  done
  sudo kill -9 $pids 2>/dev/null || true
}
start_pc_service() { # on the display the PC still has (X's abstract socket is per network namespace)
  in_ns hjv-pc pc env DISPLAY=:99 "${PC_SERVICE_ENV[@]}" "$PC_CLIENT" --server >>"$RUN/pc-server.log" 2>&1 &
}
# the service's environment from S15 on: HJV_E2E_LOGON=1 (hjv-e2e builds) reads each start as a
# locked PC and logs the end lock instead of locking (TV-W3, EL1-EL4)
PC_SERVICE_ENV=()
online() { # <ID> <side>: hbbs has the device online, asked from the side's network
  in_ns "hjv-$2" "$2" "$PROBE" "$DOMAIN" --ca "$CA_ROOT" --peer "$1" >"$RUN/probe-$1.log" 2>&1 &&
    grep -q "$1=online" "$RUN/probe-$1.log"
}
# <what>: a PC whose binding went starts its rendezvous connection again within a second (its access
# ticket changed: hjviewer_access.rs registration_tick), and hbbs has it offline until it registers
# again; a controller that asks in that gap gets "Remote desktop is offline" (exit 1), not the
# refusal. Wait for the restart's tick, then for hbbs to have the PC online.
revoked_registered_again() {
  sleep 2
  until_ok 20 "$1: the PC online again after its rendezvous restart" online "$PC_ID" ctl
}
relogin_marked() { # after a knock for the PC's sync, the account's list has the PC needs-relogin
  knock
  sleep 2
  ctl_lists needs-relogin
}
pc2_id() { # the second PC's ID, from its service
  run pc2 --hjv-e2e-id >/dev/null
  PC2_ID=${OUT%% *}
  [ -n "$PC2_ID" ] && [ "${OUT#* }" != "-" ]
}
hbbs_new_key() { [ "$(grep -c "update_pk $PC_ID " "$RUN/hbbs.log" || true)" -gt "$1" ]; }
sso_calls() { curl -s "http://127.0.0.1:$SSO_PORT/_stats" | python3 -I -c 'import json, sys; print(json.load(sys.stdin)["token_calls"])'; }
local_option() { # <side> <name>: a value of the side's LocalConfig, read from its file (never printed)
  python3 -I -c 'import re, sys
try:
    text = open(sys.argv[1], encoding="utf-8").read()
except FileNotFoundError:
    sys.exit(0)
m = re.search(r"^" + re.escape(sys.argv[2]) + r" = (\x27|\")(.*?)\1$", text, re.M)
print(m.group(2) if m else "")' "$RUN/$1/home/.config/hjviewer/HJViewer_local.toml" "$2"
}
keep_secrets() { # <side>: remembers the side's token and account key for S20
  local v k
  for k in access_token hjv-ctl-key; do
    v=$(local_option "$1" "$k")
    if [ -n "$v" ] && [[ " ${SECRETS[*]} " != *" $v "* ]]; then SECRETS+=("$v"); fi
  done
}
# The PC syncs at once for a proof its list cannot decide (a new key, no list), but for proofs at
# most once in 10 s (design 3-5): `asked` after a proof that made it sync, `gap` before the next
# one that needs a sync of its own
asked() { LAST_ASK=$(date +%s); }
gap() {
  local wait=$((LAST_ASK + 11 - $(date +%s)))
  if [ "$wait" -gt 0 ]; then sleep "$wait"; fi
}
knock() { run pc --hjv-e2e-option hjv-pc-sync "$RANDOM$RANDOM" >/dev/null; } # the PC syncs soon
ms_since() { echo $((($(date +%s%N) - $1) / 1000000)); }

# Client calls in the background (1.9: TV-W5's streams, W6-E3's waiting room, W6-E5's probe):
# <tag> <ns> <side> <client> [NAME=value...] <args...>; its output goes to $RUN/<tag>.bg, and it
# ends by itself within BG_LIMIT s. bg_wait <tag> <seconds> waits for it: BG_RC, BG_OUT, BG_MS
# (ms the wait took); a call still running then fails the run.
declare -A BG_PID=()
BG_LIMIT=150
bg_start() {
  local tag=$1 ns=$2 side=$3 client=$4 envs=()
  shift 4
  while [[ ${1:-} =~ ^[A-Z_][A-Z0-9_]*= ]]; do
    envs+=("$1")
    shift
  done
  mkdir -p "$RUN/$side/home" "$RUN/$side/ipc"
  in_ns "$ns" "$side" timeout -k 5 "$BG_LIMIT" env "${envs[@]}" "$client" "$@" >"$RUN/$tag.bg" 2>&1 &
  BG_PID[$tag]=$!
}
# (the subshell's /proc entry: bash reaps its children as they end, as forward_try relies on)
bg_running() { [ -e "/proc/${BG_PID[$1]}" ]; }
bg_has() { grep -qF -- "$2" "$RUN/$1.bg" 2>/dev/null; } # <tag> <text>
bg_wait() {
  local t0 end
  t0=$(date +%s%N)
  end=$(($(date +%s) + $2))
  while bg_running "$1"; do
    [ "$(date +%s)" -lt "$end" ] || fail "$1: still running $2 s later ($(grep '^hjv-e2e:' "$RUN/$1.bg" | tail -n 1))"
    sleep 0.1
  done
  BG_MS=$(ms_since "$t0")
  BG_RC=0
  wait "${BG_PID[$1]}" || BG_RC=$?
  BG_OUT=$(cat "$RUN/$1.bg")
  grep '^hjv-e2e:' <<<"$BG_OUT" | sed "s/^/    [$1] /" || true
}
bg_in() { until_ok 25 "$1: the session started" bg_has "$1" 'hjv-e2e: logged in to '; } # <tag>

# TV-W5's close reasons (src/hjviewer_account/mod.rs; the mark "not allowed" keeps every app,
# 1.6.0-1.8.x too, from connecting again by itself)
CUT_LOGIN="이 기기의 로그인이 끝나 연결을 끊었습니다. (login ended, not allowed)"
CUT_PC="이 PC가 비밀번호 없는 연결을 더 받지 않아 연결을 끊었습니다. (password-less ended, not allowed)"
# <scenario> <close text> <log reason> <controller ID>: the last bg_wait's stream was cut by the PC
# with that close reason, and the PC logged the session's end for that reason once
cut_by_pc() {
  [ "$BG_RC" = 7 ] || fail "$1: the stream ended with exit $BG_RC, not 7 (closed with a reason): $(grep '^hjv-e2e:' <<<"$BG_OUT" | tail -n 1)"
  grep -qxF -- "hjv-e2e: closed: $2" <<<"$BG_OUT" || fail "$1: not the close reason '$2': $(grep '^hjv-e2e: closed' <<<"$BG_OUT")"
  until_ok 5 "$1: the PC's log line of the end ($3)" pc_log_has "(remote) of .* from $4 ended: $3"
  pc_log_has 'Connection closed: hjv: account key no longer trusted' || fail "$1: no 'Connection closed: hjv: account key no longer trusted' in the PC's log"
}
sessions_ended() { pc_log_count 'hjv: same-account session #'; } # the PC's lines of sessions it cut

# The PC's incoming connections (TV-W9), as --hjv-e2e-incoming prints the service's answer: $INC
incoming() { # <scenario>
  run pc --hjv-e2e-incoming >/dev/null
  [ "$RC" = 0 ] || fail "$1: --hjv-e2e-incoming: exit $RC"
  INC=$OUT
  jq -e '.on == true and (.rows | type == "array")' <<<"$INC" >/dev/null 2>&1 ||
    fail "$1: not the list of a PC that records: $(head -c 300 <<<"$INC")"
}
inc_rows() { jq '.rows | length' <<<"$INC"; }
# <scenario> <jq condition>: the newest row (rows come newest first) meets it
inc_newest() {
  jq -e ".rows[0] | $2" <<<"$INC" >/dev/null || fail "$1: the newest row is $(jq -c '.rows[0]' <<<"$INC")"
}
inc_newest_ended() { incoming "$1" && [ "$(jq -r '.rows[0].end' <<<"$INC")" != open ]; } # <scenario>
# a row of <ID> still open: its t0 in I5_T0
inc_open_from() {
  incoming I5
  I5_T0=$(jq -r --arg id "$1" '[.rows[] | select(.id == $id and .kind == "remote" and .end == "open")][0].t0 // empty' <<<"$INC")
  [ -n "$I5_T0" ]
}
end_locks() { pc_log_count 'hjv: end lock:'; } # the PC's end-of-session lock lines (TV-W3)
end_locks_above() { [ "$(pc_log_count "$1")" -gt "$2" ]; } # <text> <count before>
# the connection manager's lines of <file> after its first <count>: one matches <pattern>
cm_file_since() {
  local text
  text=$(tail -n "+$(($2 + 1))" "$1")
  grep -q -- "$3" <<<"$text"
}
# an ID as the connection manager shows a staff mark's controller: groups of three, the first the rest
spaced_id() { sed ':a;s/\B[0-9]\{3\}\>/ &/;ta' <<<"$1"; }

account_common() { # hjv-pc and hjv-ctl logged in as alice, the PC bound
  run pc --hjv-e2e-login "$ALICE" "$ALICE_PW"
  want 0 "S1: the PC's login"
  has 'this device: added' || fail "S1: the PC was not added to the account"
  BIND_REQ=$(sed -n 's/^hjv-e2e: bind request //p' <<<"$OUT")
  keep_secrets pc
  run ctl --hjv-e2e-login "$ALICE" "$ALICE_PW"
  want 0 "S2: the controller's login"
  has "logged in as $ALICE_NAME ($ALICE)" || fail "S2: the account's name in Korean did not come"
  CTL_ID=$(sed -n 's/^hjv-e2e: logged in as .* on \([0-9]*\)$/\1/p' <<<"$OUT")
  [ -n "$CTL_ID" ] || fail "S2: the controller's login did not say its ID"
  keep_secrets ctl
}

account_open() {
  log "account (open): alice's PC and controller, the switch on for everyone"
  admin passwordless on
  account_common
  until_ok 10 "the PC's binding active" state_is active
  run ctl --hjv-e2e-devices
  has "device $PC_ID linux pwless=active" || fail "the controller's list has no active PC"
  log "S21: a password-less remote screen over WebRTC P2P (UDP between the peers)"
  pwless_ok ctl s21 "S21"
  has 'transport: direct (P2P)' || fail "S21: not direct ($(grep -o 'transport: .*' <<<"$OUT"))"
  pass "S21: password-less, $(grep -o 'transport: .*' <<<"$OUT"), $MS ms"
}

account_strict() {
  local t0 n0 n1 cm0 calls e_bob e_carol tok b0 old off os_user code first_sync_ms recv ckid
  local bind_s19b t_key spid lpid b3 r3_before s25_ms ts ft=() ended row n2 locks
  log "S24: the switch is off after the install; alice logs in on the PC and the controller"
  admin passwordless status | tee "$RUN/switch.txt" | sed 's/^/    /'
  grep -qi 'off' "$RUN/switch.txt" || fail "S24: the switch is not off after the install"
  account_common
  pass "S1: this device: added (the PC's login bound it)"
  pass "S2: logged in as $ALICE_NAME ($ALICE) on the controller $CTL_ID"
  until_ok 10 "S24: the PC took its binding (paused by the administrator)" state_is disabled
  pass "S24: the PC shows 'disabled' (관리자가 일시 중지함): $STATE"
  run ctl --hjv-e2e-devices
  has "device $PC_ID linux pwless=disabled" || fail "S24: the controller's list does not have the PC paused"
  pwless_refused ctl s24a "S24: password-less while the switch is off"
  pass "S24: refused while off (exit 3, the refusal box)"
  admin passwordless on --owner-of "$PC_ID" | sed 's/^/    /'
  t0=$(date +%s%N)
  until_ok 15 "S24: password-less within the refresh after 'passwordless on --owner-of'" try_rc ctl s24b 0
  screen_is_pc s24b || fail "S24: the decoded screen is not the PC's desktop"
  pass "S24: password-less $(ms_since "$t0") ms after the switch went on for the PC's owner"
  admin passwordless on | sed 's/^/    /'

  log "S1, S3: the PC's trust list, the controller's device list"
  until_ok 10 "S1: the PC's log says 'trust list active'" pc_log_has 'hjv: trust list active'
  pass "S1: $(pc_log | grep -o 'hjv: trust list active.*' | tail -1)"
  run ctl --hjv-e2e-devices
  has "device $PC_ID linux pwless=active pk=[A-Za-z0-9+/]\{43\}=" || fail "S3: the PC is not in the list, active with its key"
  has "device $CTL_ID " && fail "S3: the controller lists itself"
  pass "S3: the PC is listed active with its key; the controller is not"

  log "T0, I1: a password connection (hjv-ctl2, not logged in), and its row in the PC's incoming connections"
  incoming I1
  n0=$(inc_rows)
  run ctl2 --hjv-e2e-screenshot "$PC_ID" "$PASSWORD" "$RUN/t0.ppm"
  want 0 "T0: the password connection"
  screen_is_pc t0 || fail "T0: the decoded screen is not the PC's desktop"
  T0=$MS B=$((MS + 2000))
  pass "T0 = $T0 ms (B = T0 + 2 s = $B ms)"
  until_ok 5 "I1: the row of T0 ended" inc_newest_ended I1
  [ "$(inc_rows)" = $((n0 + 1)) ] || fail "I1: $(($(inc_rows) - n0)) new rows for one session"
  inc_newest I1 '.kind == "remote" and .auth == "permanent" and .ver == "" and .end == "peer" and .t1 >= .t0'
  pass "I1: one new row, newest first: $(jq -c '.rows[0] | {kind, auth, ver, end, re}' <<<"$INC") (the PC records: installed, an account build)"

  log "S4-S6, I2: alice's controller without a password"
  n0=$(pc_logins)
  cm0=$(wc -l <"$RUN/pc-cm.log")
  pwless_ok ctl s4 "S4"
  pass "S4: no password box, the PC's screen ($MS ms; T0 $T0 ms)"
  n1=$(pc_logins)
  [ "$((n1 - n0))" = 1 ] || fail "S6: $((n1 - n0)) same-account login lines for one session"
  pc_log_has "hjv: same-account login (remote) by .* from $CTL_ID" ||
    fail "S6: no 'same-account login … from $CTL_ID' in the PC's log"
  pass "S6: $(pc_log | grep -o 'hjv: same-account login (remote) .*' | tail -1)"
  # S6′ (TV-W2): the device's name alone, and the mark on a line of its own; the proof's record
  # comes before the logon response, so a PC without 2FA shows it at once (critic 2-3)
  cm_since "$cm0" "cm login from $CTL_ID named \"Linux PC\", authorized true, mark same-account(홍\*동)" ||
    fail "S6′: the connection manager did not show the account's device with its mark ($(tail -n 1 "$RUN/pc-cm.log"))"
  cm_since "$cm0" "named \"[^\"]*same account" && fail "S6′: the name still carries 'same account'"
  pass "S6′: the connection manager shows $(tail -n "+$((cm0 + 1))" "$RUN/pc-cm.log" | grep -o 'named ".*' | head -1)"
  until_ok 5 "I2: the row of S4 ended" inc_newest_ended I2
  inc_newest I2 ".id == \"$CTL_ID\" and .ver == \"same-account\" and .dev == \"Linux PC\" and .own == \"홍*동\" and .auth == \"account\" and .kind == \"remote\""
  pass "I2: alice's row: $(jq -c '.rows[0] | {ver, dev, own, auth, kind, end}' <<<"$INC")"
  # S5′ (TV-W2): the controller's recording reaches the connection manager
  cm0=$(wc -l <"$RUN/pc-cm.log")
  run ctl --hjv-e2e-stream "$PC_ID" "" 5 record
  want 0 "S5"
  has 'password required' && fail "S5: a password box"
  has 'hjv-e2e: [1-9][0-9]* frames' || fail "S5: no video frames"
  has 'hjv-e2e: sent recording true' && has 'hjv-e2e: sent recording false' || fail "S5′: the stream did not report its recording"
  until_ok 5 "S5′: the connection manager's 'cm recording false'" cm_since "$cm0" 'cm recording false'
  [ "$(tail -n "+$((cm0 + 1))" "$RUN/pc-cm.log" | grep -o 'cm recording [a-z]*' | tr '\n' ' ')" = "cm recording true cm recording false " ] ||
    fail "S5′: the connection manager's recording lines: $(tail -n "+$((cm0 + 1))" "$RUN/pc-cm.log" | grep 'cm recording' | tr '\n' ' ')"
  pc_log_has 'hjv: peer recording true' || fail "S5′: no 'hjv: peer recording true' in the PC's log"
  pass "S5: $(grep -o '[0-9]* frames.*' <<<"$OUT"); S5′: the recording it reported reached the connection manager (cm recording true, then false) and the PC's log"
  # EL2 (TV-W3): without HJV_E2E_LOGON the PC was not locked when these sessions came in, so the
  # automatic lock of a same-account session leaves it as it was
  [ "$(end_locks)" = 0 ] || fail "EL2: the PC logged an end lock: $(pc_log | grep -o 'hjv: end lock:.*' | head -1)"
  pass "EL2: no 'hjv: end lock:' line after $(pc_logins) same-account remote screen sessions of a PC that was not locked when they came in"

  log "S7-S10: bob's controller (hjv-ctl2)"
  run ctl2 --hjv-e2e-login "$BOB" "$BOB_PW"
  want 0 "S7: bob's login"
  keep_secrets ctl2
  run ctl2 --hjv-e2e-devices
  want 0 "S7: bob's device list"
  has "device $PC_ID" && fail "S7: bob's list has alice's PC"
  pass "S7: bob's list has no PC ($(grep -o '[0-9]* device(s)' <<<"$OUT"))"
  incoming I3
  n0=$(inc_rows)
  pwless ctl2 s8
  want 3 "S8: bob without a password"
  has 'password required Password Required' || fail "S8: not the password box"
  [ "$MS" -le "$B" ] || fail "S8: the password box after $MS ms (B = $B ms)"
  pass "S8: no proof, the password box at once ($MS ms <= B $B ms)"
  cm0=$(wc -l <"$RUN/pc-cm.log")
  run ctl2 --hjv-e2e-screenshot "$PC_ID" "$PASSWORD" "$RUN/s9.ppm"
  want 0 "S9: bob with the PC's password"
  screen_is_pc s9 || fail "S9: the decoded screen is not the PC's desktop"
  pass "S9: bob gets in with the PC's password (D6)"
  # H21: other PCs see the OS user name as 1.5.1 sent it (capitalized), never the login's name or
  # portal ID
  os_user=$(id -un)
  # (S9′, TV-W2: a password session carries no mark)
  cm_since "$cm0" "cm login from .* named \"${os_user^}\", authorized true, mark none()" ||
    fail "S9: the PC was not given the OS user name without a mark ($(tail -n 1 "$RUN/pc-cm.log"))"
  cm_since "$cm0" "김연세\|$BOB" && fail "S9: the PC was given bob's name or portal ID"
  pass "S9: the PC's connection manager got the OS user name (${os_user^}), not bob's name or portal ID (H21), and no mark (S9′: mark none())"
  run ctl2 --hjv-e2e-screenshot "$PC_ID" "$WRONG_PW" "$RUN/s10.ppm"
  want 3 "S10: a wrong password"
  has 'password required Wrong Password' || fail "S10: not the box to enter it again"
  pass "S10: a wrong password asks again (exit 3)"
  incoming I3
  [ "$(inc_rows)" = $((n0 + 1)) ] || fail "I3: $(($(inc_rows) - n0)) new rows for bob's three tries (one got in)"
  inc_newest I3 ".name == \"${os_user^}\" and .ver == \"\" and .auth == \"permanent\" and .kind == \"remote\" and .dev == \"\""
  pass "I3: one row for bob's success ($(jq -c '.rows[0] | {name, ver, auth, end}' <<<"$INC")); none for the refused proof (S8) or the wrong password (S10)"

  log "S22, I4: port forwarding never goes by proof (U3)"
  incoming I4
  n0=$(inc_rows)
  forward_try ctl "" 9098
  want 3 "S22: port forwarding without a password"
  has 'password required Password Required' || fail "S22: not the password box"
  pass "S22: port forwarding without a password: the password box, no proof (exit 3)"
  incoming I4
  [ "$(inc_rows)" = "$n0" ] || fail "I4: the refused port forwarding has a row"
  jq -e 'any(.rows[]; .kind == "forward" and .auth == "permanent" and .end != "open")' <<<"$INC" >/dev/null ||
    fail "I4: no row of the port forwarding the first part opened with the password"
  pass "I4: no row for the refused port forwarding; the one opened with the password at the start is kind forward ($(jq -c '[.rows[] | select(.kind == "forward")][0] | {auth, end}' <<<"$INC"))"

  log "H-hide: a pinned build keeps the connection window shown (TV-W2)"
  run pc --hjv-e2e-option allow-hide-cm Y
  run pc --hjv-e2e-option allow-hide-cm
  [ "$OUT" = N ] || fail "H-hide: the service reads allow-hide-cm '$OUT' after Y was written, not N"
  run pc --hjv-e2e-option allow-hide-cm ""
  pass "H-hide: allow-hide-cm written Y, read back N from the service (the pinned build's overwrite)"

  log "S23: a token copied to another device is no use without its account key"
  tok=$(local_option ctl access_token)
  [ -n "$tok" ] || fail "S23: the controller keeps no token"
  code=$(in_ns hjv-ctl2 ctl2 curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $tok" \
    "https://$DOMAIN/api/peers?current=1&pageSize=100&accessible=&status=1" || true)
  [ "$code" = 401 ] || fail "S23: the copied token got $code"
  pass "S23: the copied token alone gets 401"

  log "X1, X5, X8: the PC's own settings"
  run pc --hjv-e2e-option approve-mode click
  cm0=$(wc -l <"$RUN/pc-cm.log")
  pwless ctl x1
  has 'No Password Access' || fail "X1: the click-approval PC did not answer 'No Password Access'"
  until_ok 5 "X1: the PC's user asked to accept" cm_since "$cm0" "cm login from $CTL_ID .*authorized false"
  # X1′ (TV-W2, critic X-21): the click-approval branch judges no proof, so no same-account mark
  cm_since "$cm0" "cm login from $CTL_ID .*authorized false, mark none()" ||
    fail "X1′: the click-approval request is not mark none() ($(grep "cm login from $CTL_ID" "$RUN/pc-cm.log" | tail -n 1))"
  pass "X1: approve-mode=click: 'No Password Access', the PC's user is asked; X1′: with mark none()"
  run pc --hjv-e2e-option approve-mode ""
  run pc --hjv-e2e-option verification-method use-temporary-password
  pwless_refused ctl x5 "X5: a PC that takes only its temporary password"
  pass "X5: verification-method=use-temporary-password refuses the proof (exit 3)"
  run pc --hjv-e2e-option verification-method ""
  run pc --hjv-e2e-option approve-mode password
  run pc --hjv-e2e-option verification-method use-permanent-password
  run pc --hjv-e2e-service-password ""
  want 0 "X8: clearing the PC's password"
  pwless_ok ctl x8a "X8: alice on a PC without a password"
  run ctl2 --hjv-e2e-screenshot "$PC_ID" "$PASSWORD" "$RUN/x8b.ppm"
  want 3 "X8: bob with the old password"
  pass "X8: 'my devices only' (password approval, permanent only, no password): alice gets in, bob does not"
  run pc --hjv-e2e-service-password "$PASSWORD"
  want 0 "X8: the PC's password back"
  run pc --hjv-e2e-option approve-mode ""
  run pc --hjv-e2e-option verification-method ""

  log "S16: requests and files a local user could write"
  b0=$(binding Binding)
  [ -n "$b0" ] || fail "S16: the PC keeps no binding"
  run pc --hjv-e2e-option hjv-bind-req "$(random_binding).$(date +%s)"
  sleep 3
  [ "$(binding Binding)" = "$b0" ] || fail "S16: a bind request of another binding changed the PC's binding"
  pwless_ok ctl s16a "S16: after a bind request of another binding"
  pass "S16: a bind request of another binding leaves the binding as it was"
  gap
  sed -i 's/"state":"active"/"state":"disabled"/' "$RUN/pc/home/.config/hjviewer/HJViewer_trust.json"
  pwless_ok ctl s16b "S16: after the cached list was altered"
  asked
  [ "$(sha256sum <"$RUN/pc/home/.config/hjviewer/HJViewer_trust.json" | cut -d' ' -f1)" = "$(binding ListHash)" ] ||
    fail "S16: the cached list does not match its hash again"
  pass "S16: after the cached list was altered the PC lets in and keeps a list that matches its hash (fetched again for the mismatch, or by a periodic sync; the unit tests pin which)"
  set_binding "$(random_binding)"
  pwless_refused ctl s16c "S16: a binding file with another binding"
  asked
  set_binding "$b0"
  pass "S16: a binding the server does not confirm opens nothing (exit 3)"
  until_ok 15 "S16: the PC back to its own binding" state_is active

  log "S12, S13, S13b, S35: hjv-api stopped, then gone for longer than a list lives (30 s), with a password-less stream open"
  # S35 (TV-W5): a list that only expired, and syncs that fail, cut nothing (the 24-hour rule);
  # the stream outlasts hjv-api's absence and the next list
  bg_start s35 hjv-ctl ctl "$CLIENT" --hjv-e2e-stream "$PC_ID" "" 85
  bg_in s35
  ended=$(sessions_ended)
  kill -STOP "$API_PID"
  pwless_ok ctl s12 "S12: hjv-api stopped (kill -STOP)"
  pass "S12: hjv-api stopped: password-less from the PC's cached list and the controller's hjv-my-devices"
  kill -CONT "$API_PID"
  kill "$API_PID"
  wait "$API_PID" 2>/dev/null || true
  code=$(in_ns hjv-ctl ctl curl -s -o /dev/null -w '%{http_code}' "https://$DOMAIN/api/hjv/source" || true)
  [ "$code" = 503 ] || fail "S13: with hjv-api gone the account paths answer $code, not Caddy's 503"
  pwless_ok ctl s13 "S13: hjv-api gone"
  pass "S13: hjv-api gone (the account paths answer $code): password-less still"
  recv=$(binding Recv)
  sleep $((recv + 31 - $(date +%s) > 0 ? recv + 31 - $(date +%s) : 0))
  pwless_refused ctl s13b "S13b: hjv-api gone longer than the list's 30 s"
  asked
  [ "$MS" -le "$B" ] || fail "S13b: the refusal after $MS ms (B = $B ms)"
  pass "S13b: the list expired, the PC's sync failed: refused ($MS ms)"
  start_api
  knock
  until_ok 20 "S13b: the PC's list again with hjv-api back" state_is active
  # one more sync with the list, which still has the stream's key
  sleep 6
  bg_running s35 || fail "S35: the stream ended before hjv-api's list came back: $(grep '^hjv-e2e:' "$RUN/s35.bg" | tail -n 2 | tr '\n' ' ')"
  bg_wait s35 100
  [ "$BG_RC" = 0 ] && grep -q 'hjv-e2e: [1-9][0-9]* frames' <<<"$BG_OUT" ||
    fail "S35: the stream did not run to its end (exit $BG_RC): $(grep '^hjv-e2e:' <<<"$BG_OUT" | tail -n 1)"
  [ "$(sessions_ended)" = "$ended" ] || fail "S35: the PC cut a session: $(pc_log | grep -o 'hjv: same-account session #.*' | tail -1)"
  pass "S35: the password-less stream ran its 85 s through hjv-api stopped, gone past the list's 30 s and back (exit 0, $(grep -o '[0-9]* frames' <<<"$BG_OUT")); no session cut"

  log "S14: hjv-ctl2 logs out and in as alice (a new key); its first connection right after"
  run ctl2 --hjv-e2e-logout
  gap
  run ctl2 --hjv-e2e-login "$ALICE" "$ALICE_PW"
  want 0 "S14: alice's login on hjv-ctl2"
  CTL2_ID=$(sed -n 's/^hjv-e2e: logged in as .* on \([0-9]*\)$/\1/p' <<<"$OUT")
  [ -n "$CTL2_ID" ] || fail "S14: the login on hjv-ctl2 did not say its ID"
  keep_secrets ctl2
  run ctl2 --hjv-e2e-devices
  pwless_ok ctl2 s14 "S14: the new key's first connection"
  asked
  pass "S14: the new key's first connection gets in ($MS ms): the PC's list had the key (its sync for the key's serial, or a periodic one; the unit tests pin which)"

  log "S15, I5: the PC's service starts again with a session open; its user is logged out (the token gone)"
  bg_start i5 hjv-ctl ctl "$CLIENT" --hjv-e2e-stream "$PC_ID" "" 60
  bg_in i5
  until_ok 10 "I5: the open session's row" inc_open_from "$CTL_ID"
  n2=$(inc_rows)
  stop_pc_service
  # the service went: the stream ends with it
  bg_wait i5 40
  run pc --hjv-e2e-local-option access_token ""
  n0=$(pc_log_count 'hjv: same-account logins: sync task started')
  n1=$(pc_log_count 'keep_alive: ')
  # from here on the service reads each session's start as a locked PC (TV-W3's EL1-EL4)
  PC_SERVICE_ENV=(HJV_E2E_LOGON=1)
  start_pc_service
  until_ok 20 "S15: the PC's sync task started again" pc_log_count_above 'hjv: same-account logins: sync task started' "$n0"
  until_ok 10 "S15: the PC's first sync after the start" first_sync_done
  until_ok 20 "S15: the PC registered with hbbs again" pc_log_count_above 'keep_alive: ' "$n1"
  locks=$(pc_log_count 'hjv: end lock: auto, locked at start true')
  pwless_ok ctl s15 "S15: after the service started again"
  pass "S15: password-less right after the restart, with the user logged out (binding + cache)"
  first_sync_ms=$(first_sync_ms)
  [ -n "$first_sync_ms" ] || fail "S15: no first sync in the PC's log"
  [ "$first_sync_ms" -le 1500 ] || fail "S15: the first sync took $first_sync_ms ms (more than 1.5 s)"
  pass "S15: the first sync after the start took $first_sync_ms ms (<= 1.5 s)"
  until_ok 5 "I5: the row of S15's session ended" inc_newest_ended I5
  row=$(jq -c --arg id "$CTL_ID" --argjson t0 "$I5_T0" '[.rows[] | select(.id == $id and .kind == "remote" and .t0 == $t0)][0]' <<<"$INC")
  [[ $(jq -r '.end' <<<"$row") =~ ^(unknown|service)$ ]] || fail "I5: the row of the session the restart ended is $row"
  [ "$(inc_rows)" = $((n2 + 1)) ] || fail "I5: $(($(inc_rows) - n2)) new rows after the restart, for one session"
  inc_newest I5 ".id == \"$CTL_ID\" and .t0 >= $I5_T0 and .end != \"open\" and .ver == \"same-account\""
  pass "I5: the session open across the restart ends $(jq -r '.end' <<<"$row") (the new process has another run); S15's session is a row of its own"
  until_ok 5 "EL1: the PC's line 'hjv: end lock: auto, locked at start true'" \
    end_locks_above 'hjv: end lock: auto, locked at start true' "$locks"
  pass "EL1: HJV_E2E_LOGON=1 on the service: after S15's same-account remote screen 'hjv: end lock: auto, locked at start true' (the automatic lock; a 1.8.x controller sends no hjv_end_lock either, D-9)"

  log "EL3, EL4: the controller's 'never' and 'always' for the PC, and a password session"
  run ctl --hjv-e2e-peer-option "$PC_ID" hjv-end-lock never
  [ "$OUT" = "hjv-e2e: peer option hjv-end-lock: never" ] || fail "EL3: the controller's PeerConfig says '$OUT'"
  locks=$(end_locks)
  pwless_ok ctl el3 "EL3: with hjv-end-lock never"
  sleep 2
  [ "$(end_locks)" = "$locks" ] || fail "EL3: an end lock with 'never': $(pc_log | grep -o 'hjv: end lock:.*' | tail -1)"
  pass "EL3: the controller's PeerConfig hjv-end-lock=never went with its login (OptionMessage.hjv_end_lock): no lock line"
  run ctl --hjv-e2e-peer-option "$PC_ID" hjv-end-lock always
  locks=$(pc_log_count 'hjv: end lock: always, locked at start true')
  pwless_ok ctl el3b "EL3: with hjv-end-lock always"
  until_ok 5 "EL3: the PC's line 'hjv: end lock: always'" end_locks_above 'hjv: end lock: always, locked at start true' "$locks"
  run ctl --hjv-e2e-peer-option "$PC_ID" hjv-end-lock ""
  [ "$OUT" = "hjv-e2e: peer option hjv-end-lock: " ] || fail "EL3: the option did not go: '$OUT'"
  pass "EL3: with hjv-end-lock=always 'hjv: end lock: always, locked at start true'; the option removed again"
  locks=$(end_locks)
  ascreen el4 "$PC_ID" el4
  screen_ok "EL4: a password session of a controller that is not logged in" el4
  sleep 2
  [ "$(end_locks)" = "$locks" ] || fail "EL4: an end lock after a password session: $(pc_log | grep -o 'hjv: end lock:.*' | tail -1)"
  pass "EL4: a password session (no proof) on the PC that reads as locked: no lock line (upstream's default)"

  log "S17, S33, S34: hjv-ctl2 (alice) logs out the controller's key while two streams run"
  run ctl2 --hjv-e2e-keys
  ckid=$(sed -n 's/^hjv-e2e: key \([0-9]*\) current=false app_name=hjv-ctl .*/\1/p' <<<"$OUT")
  [ -n "$ckid" ] || fail "S17: the controller's key is not in the list"
  # S33: hjv-ctl's password-less stream; S34: a password stream of a controller that is not logged
  # in (a logged-in controller sends its proof with the password too, and the PC takes the proof)
  bg_start s33 hjv-ctl ctl "$CLIENT" --hjv-e2e-stream "$PC_ID" "" 60
  bg_start s34 hjv-ctl s34 "$CLIENT" --hjv-e2e-stream "$PC_ID" "$PASSWORD" 25
  bg_in s33
  bg_in s34
  locks=$(pc_log_count 'hjv: end lock: forced, locked at start true')
  run ctl2 --hjv-e2e-revoke-key "$ckid"
  want 0 "S17: revoking the controller's key"
  t0=$(date +%s%N)
  bg_wait s33 15
  cut_by_pc S33 "$CUT_LOGIN" "its key is no longer listed" "$CTL_ID"
  until_ok 5 "S33: the PC's lock line of the cut" end_locks_above 'hjv: end lock: forced, locked at start true' "$locks"
  pass "S33: the stream ended $(ms_since "$t0") ms after the revoke (exit 7, 'closed: $CUT_LOGIN'); the PC logged '… from $CTL_ID ended: its key is no longer listed', closed it as 'account key no longer trusted' and locks (forced)"
  until_ok 15 "S17: the revoked key refused" try_rc ctl s17 3
  has 'password required Could not connect without a password' || fail "S17: not the refusal box"
  pass "S17: the revoked key is refused $(ms_since "$t0") ms after the revoke (refresh 5 s); S33: so the controller cut off gets the refusal box, not the session again"
  bg_wait s34 40
  [ "$BG_RC" = 0 ] && grep -q 'hjv-e2e: [1-9][0-9]* frames' <<<"$BG_OUT" ||
    fail "S34: the password stream did not run to its end (exit $BG_RC): $(grep '^hjv-e2e:' <<<"$BG_OUT" | tail -n 1)"
  pass "S34: the password stream beside it ran its 25 s (exit 0, $(grep -o '[0-9]* frames' <<<"$BG_OUT")): sessions opened by a password are not held to the list"

  log "S18: hjv-ctl2 takes the PC out of the list"
  run ctl2 --hjv-e2e-remove "$PC_ID"
  want 0 "S18: removing the PC"
  until_ok 15 "S18: the PC took the signed revoked" state_is revoked
  [ -z "$(binding Binding)" ] || fail "S18: the PC kept its binding"
  pwless_refused ctl2 s18 "S18: password-less to a removed PC"
  pass "S18: the PC dropped its binding on a signed revoked ($STATE); refused"

  log "S19, S38: the PC's user logs out, with a password-less stream open"
  pc_bound
  pwless_ok ctl2 s19a "S19: the PC bound again"
  bg_start s38 hjv-ctl2 ctl2 "$CLIENT" --hjv-e2e-stream "$PC_ID" "" 40
  bg_in s38
  run pc --hjv-e2e-logout
  t0=$(date +%s%N)
  within_ms 1500 pc_unbound || fail "S19: the PC still held its binding 1.5 s after the logout"
  # S38 (D-6, critic S-01): the PC's own logout ends its proven sessions
  bg_wait s38 5
  cut_by_pc S38 "$CUT_PC" "this PC was logged out" "$CTL2_ID"
  pass "S38: the PC's logout ended the password-less stream $(ms_since "$t0") ms after the logout returned (exit 7, 'closed: $CUT_PC'; the PC logged '… ended: this PC was logged out')"
  pwless_refused ctl2 s19b "S19: password-less after the PC's logout"
  admin --reveal binding list >"$RUN/bindings.txt"
  grep -q " $PC_ID " "$RUN/bindings.txt" && fail "S19: hjv-api kept the PC's binding after its logout"
  pass "S19: the PC dropped its binding $WITHIN_MS ms after the logout returned; refused, and hjv-api released it"

  log "S19b: the PC's user logs out while hjv-api is gone"
  pc_bound
  bind_s19b=$BIND_REQ
  pwless_ok ctl2 s19c "S19b: the PC bound again"
  kill "$API_PID"
  wait "$API_PID" 2>/dev/null || true
  run pc --hjv-e2e-logout
  within_ms 1500 pc_unbound || fail "S19b: the PC still held its binding 1.5 s after the offline logout"
  pwless_refused ctl2 s19d "S19b: password-less after an offline logout"
  pass "S19b: with hjv-api gone the PC dropped its binding $WITHIN_MS ms after the logout returned; refused"
  start_api
  knock
  until_ok 20 "S19b: the account lists the PC as 'listed'" ctl2_lists listed
  pass "S19b: with hjv-api back, the PC reported its empty binding: listed ($(grep "device $PC_ID" <<<"$OUT" | grep -o 'pwless=[a-z-]*'))"

  log "S16b: the old bind request again, without a login (within ten minutes of it)"
  run pc --hjv-e2e-option hjv-bind-req "$bind_s19b"
  sleep 3
  [ -z "$(binding Binding)" ] || fail "S16b: the old bind request bound the PC again"
  [ "$(binding UnboundBinding)" = "${bind_s19b%.*}" ] || fail "S16b: the released binding is not recorded"
  pwless_refused ctl2 s16b2 "S16b: password-less after the old bind request"
  pass "S16b: the released binding's old request binds nothing (UnboundLoginAt); refused"

  log "R3: a bind answer that comes after the logout binds nothing"
  # hjv-api forgets the listed binding, so the next login makes a new one, while the PC's record
  # of the released one guards its ten minutes. The PC's user logs in and out at once, with the
  # bind request's sync still out: the service is held until the login has its answer, hjv-api
  # from then until the unbind (the logout's one write of the options) is done.
  admin binding revoke "$PC_ID" | sed 's/^/    /'
  b3=$(binding UnboundBinding)
  spid=$(sudo ip netns pids hjv-pc | xargs -r ps -o pid=,args= -p | awk '$0 ~ / --server$/ {print $1}')
  sudo kill -STOP $spid
  r3_before=$(local_option pc hjv-pc-binding)
  in_ns hjv-pc pc "$PC_CLIENT" --hjv-e2e-login "$ALICE" "$ALICE_PW" >"$RUN/r3-login.out" 2>>"$RUN/pc.err" &
  lpid=$!
  # the login has its answer once its hook wrote the user's hjv-pc-binding, which it does before
  # it writes the stopped service's options
  until_ok 15 "R3: the login's answer (hjv-pc-binding written)" pc_binding_written "$r3_before"
  kill -STOP "$API_PID"
  # The sudo that runs the service in the namespace stopped itself when the service stopped (sudo
  # follows its command's stop): it goes on too, or it stays stopped after the run, holding the
  # service as a zombie
  for p in $spid; do sudo kill -CONT "$p" "$(ps -o ppid= -p "$p" | tr -d ' ')"; done
  # the service sends the bind request's sync on its second tick after it goes on (about 1 s);
  # hjv-api holds it, which an HTTP timeout of 5 s allows for some seconds more
  sleep 2
  run pc --hjv-e2e-option hjv-bind-req >/dev/null
  BIND_REQ=$OUT
  [ -n "$BIND_REQ" ] && [ "${BIND_REQ%.*}" != "$b3" ] || fail "R3: the login left no bind request of a new binding"
  run pc --hjv-e2e-option hjv-bind-req "" hjv-unbind-req "$BIND_REQ" hjv-pc-sync "$RANDOM$RANDOM"
  sleep 2
  kill -CONT "$API_PID"
  wait "$lpid" || fail "R3: the PC's login"
  keep_secrets pc
  sleep 1
  admin --reveal binding list >"$RUN/bindings.txt"
  grep -q " $PC_ID .* present " "$RUN/bindings.txt" || fail "R3: hjv-api did not answer the bind request's sync"
  [ -z "$(binding Binding)" ] || fail "R3: the bind answer that came after the unbind bound the PC"
  [ "$(binding UnboundBinding)" = "$b3" ] || fail "R3: the other binding's record did not stay"
  pwless_refused ctl2 r3 "R3: password-less after the unbind"
  pass "R3: hjv-api confirmed the new binding after its unbind; the PC took nothing (exit 3)"

  log "S25, S38: [끄기] on the PC with a password-less stream open, the option written again, then a login on the PC"
  pc_bound
  pwless_ok ctl2 s25a "S25: bound"
  bg_start s38off hjv-ctl2 ctl2 "$CLIENT" --hjv-e2e-stream "$PC_ID" "" 12
  bg_in s38off
  ended=$(sessions_ended)
  run pc --hjv-e2e-option hjv-pwless-off Y
  within_ms 1500 pc_off || fail "S25: the PC did not keep Off within 1.5 s of [끄기]"
  s25_ms=$WITHIN_MS
  pwless_refused ctl2 s25b "S25: after [끄기]"
  # S38's other half (D-6): [끄기] stops new password-less logins; its sync goes on, and the open
  # session stays
  bg_wait s38off 30
  [ "$BG_RC" = 0 ] && grep -q 'hjv-e2e: [1-9][0-9]* frames' <<<"$BG_OUT" ||
    fail "S38: the stream open at [끄기] did not run to its end (exit $BG_RC): $(grep '^hjv-e2e:' <<<"$BG_OUT" | tail -n 1)"
  [ "$(sessions_ended)" = "$ended" ] || fail "S38: [끄기] cut a session: $(pc_log | grep -o 'hjv: same-account session #.*' | tail -1)"
  pass "S38: [끄기] on the PC refused new password-less logins and left the open stream to its end (exit 0, $(grep -o '[0-9]* frames' <<<"$BG_OUT"))"
  run pc --hjv-e2e-option hjv-pwless-off N
  sleep 2
  pwless_refused ctl2 s25c "S25: with Config2's request written again"
  pc_bound
  pwless_ok ctl2 s25d "S25: after a login on the PC"
  pass "S25: off $s25_ms ms after [끄기], kept off when Config2 is written again (Off), on again only by a login on the PC"

  log "S26, S36: the administrator releases the PC (binding revoke, then device remove), with a password-less stream open"
  bg_start s36 hjv-ctl2 ctl2 "$CLIENT" --hjv-e2e-stream "$PC_ID" "" 40
  bg_in s36
  ts=$(date +%s.%N)
  admin binding revoke "$PC_ID" | sed 's/^/    /'
  t0=$(date +%s%N)
  bg_wait s36 20
  cut_by_pc S36 "$CUT_PC" "this PC's binding was revoked" "$CTL2_ID"
  pass "S36: the stream ended $(ms_since "$t0") ms after 'binding revoke' (exit 7, 'closed: $CUT_PC'; the PC logged '… ended: this PC's binding was revoked')"
  until_ok 15 "S26: the PC took the signed revoked" state_is revoked
  revoked_registered_again "S26"
  pwless_refused ctl2 s26a "S26: after binding revoke"
  pass "S26: binding revoke: a signed revoked, the binding gone ($STATE), refused"
  # then the PC reports that it holds no binding: hjv-api, which has only the released one, answers
  # 404 'unbound' without a list, and the PC goes on showing why its binding went
  until_ok 15 "S26: the PC's report of its empty binding answered 404" pc_syncs_match "$ts" '^(200 )+404( 404)*$'
  sleep 1
  state_is revoked || fail "S26: the unsigned answer changed what the PC shows ($STATE)"
  pass "S26: the PC's report of its empty binding got the unsigned 'unbound' (its syncs since the revoke: $(pc_syncs_since "$ts")); it still shows $STATE"
  pc_bound
  pwless_ok ctl2 s26b "S26: bound again"
  admin device remove "$PC_ID" | sed 's/^/    /'
  until_ok 15 "S26: the PC took the signed revoked after device remove" state_is revoked
  revoked_registered_again "S26 after device remove"
  pwless_refused ctl2 s26c "S26: after device remove"
  pass "S26: device remove (the alias) does the same"

  log "X6, S37: the switch off for everyone, with a password-less stream open"
  pc_bound
  pwless_ok ctl2 x6a "X6: bound"
  bg_start s37 hjv-ctl2 ctl2 "$CLIENT" --hjv-e2e-stream "$PC_ID" "" 40
  bg_in s37
  admin passwordless off | sed 's/^/    /'
  t0=$(date +%s%N)
  # S37 (D-5): a signed 'disabled' list also ends the open password-less sessions
  bg_wait s37 20
  cut_by_pc S37 "$CUT_PC" "password-less logins are paused (disabled)" "$CTL2_ID"
  pass "S37: the stream ended $(ms_since "$t0") ms after 'passwordless off' (exit 7, 'closed: $CUT_PC'; the PC logged '… ended: password-less logins are paused (disabled)')"
  until_ok 15 "X6: refused after 'passwordless off'" try_rc ctl2 x6b 3
  pass "X6: refused $(ms_since "$t0") ms after 'passwordless off'"
  admin passwordless on | sed 's/^/    /'
  until_ok 15 "X6: on again" state_is active

  log "X7: the PC's device key changes (a reset): the key warning"
  run ctl --hjv-e2e-login "$ALICE" "$ALICE_PW"
  want 0 "X7: the controller's login"
  keep_secrets ctl
  CTL_ID=$(sed -n 's/^hjv-e2e: logged in as .* on \([0-9]*\)$/\1/p' <<<"$OUT")
  run ctl --hjv-e2e-devices
  old=$(sed -n "s/^hjv-e2e: device $PC_ID .* pk=\([^ ]*\) .*/\1/p" <<<"$OUT")
  stop_pc_service
  # the key pair alone goes (a multi-line array); the ID, the password and the binding stay
  python3 -I -c 'import re, sys
p = sys.argv[1]
text = open(p).read()
cut = re.sub(r"(?ms)^key_pair = \[.*?^\]\n", "", text)
assert cut != text, "no key_pair in the PC config"
open(p, "w").write(cut)' "$RUN/pc/home/.config/hjviewer/HJViewer.toml"
  n0=$(grep -c "update_pk $PC_ID " "$RUN/hbbs.log" || true)
  start_pc_service
  until_ok 30 "X7: hbbs took the PC's new key" hbbs_new_key "$n0"
  t_key=$(date +%s)
  cm0=$(wc -l <"$RUN/pc-cm.log")
  pwless ctl x7
  want 4 "X7: the controller's first connection to the PC with a new key"
  has "error Security warning: that PC's key has changed" || fail "X7: not the key warning"
  forward_try ctl "$PASSWORD" 9097
  want 4 "X7: port forwarding to the PC with a new key"
  has "error Security warning: that PC's key has changed" || fail "X7: port forwarding: not the key warning"
  [ "$(wc -l <"$RUN/pc-cm.log")" = "$cm0" ] || fail "X7: a login reached the PC"
  pass "X7: the key warning (exit 4) for the remote screen and port forwarding; neither a proof nor the password went out"

  # The PC's first sync with its new key came before hbbs had that key, so hjv-api found nothing to
  # mark; it asks hbbs again at the PC's first sync a minute later (KEY_CHECK_EVERY). Meanwhile:
  log "K1, K2: the account key out of reach, and a certificate the device does not trust"
  run ctl HJV_E2E_KEY_LOCKED=1 --hjv-e2e-devices
  want 1 "K1: a request while the account key cannot be read"
  has 'devices failed: hjv: account key unavailable' || fail "K1: not 'hjv: account key unavailable'"
  run ctl --hjv-e2e-devices
  want 0 "K1: the next request"
  pass "K1: no key, no request ('hjv: account key unavailable'), and no logout"
  # the setting that lets other requests go on past a certificate that fails ("Allow insecure TLS
  # fallback") must not let an account request: no test CA in this run's trust store
  run ctl --hjv-e2e-option allow-insecure-tls-fallback Y
  calls=$(sso_calls)
  run ctl SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt --hjv-e2e-login "$ALICE" "$ALICE_PW"
  want 1 "K2: a login through a certificate the device does not trust"
  grep -qiE 'certificate|UnknownIssuer' <<<"$OUT" || fail "K2: not a certificate failure ($(grep 'login failed' <<<"$OUT"))"
  [ "$(sso_calls)" = "$calls" ] || fail "K2: the login reached the SSO"
  run ctl --hjv-e2e-option allow-insecure-tls-fallback ""
  pass "K2: with the insecure TLS fallback allowed, the account request still fails on the certificate (H15)"

  log "S11: a wrong password gets the same answer for an imported ID and one never imported"
  calls=$(sso_calls)
  run ctl2 --hjv-e2e-login "$BOB" "$WRONG_PW"
  want 1 "S11: bob's wrong password"
  e_bob=$(sed -n 's/^hjv-e2e: login failed: //p' <<<"$OUT")
  [ "$(sso_calls)" = $((calls + 1)) ] || fail "S11: a wrong password did not reach the fake SSO once"
  calls=$(sso_calls)
  run ctl2 --hjv-e2e-login "$CAROL" "$WRONG_PW"
  want 1 "S11: a wrong password of a portal ID never imported"
  e_carol=$(sed -n 's/^hjv-e2e: login failed: //p' <<<"$OUT")
  [ "$e_carol" = "$e_bob" ] || fail "S11: '$e_carol' is not the wrong password's '$e_bob'"
  [ "$(sso_calls)" = $((calls + 1)) ] || fail "S11: the ID never imported did not reach the fake SSO once (open login)"
  admin allow list >"$RUN/allow.txt"
  [ "$(head -1 "$RUN/allow.txt" | cut -d, -f1)" = "allowlist: 2 IDs" ] || fail "S11: a failed login changed the allowlist"
  pass "S11: the same failure ($e_carol), one SSO call each; a failed login puts nobody on the allowlist"
  # the failed logins leave the login of hjv-ctl2 (alice's) as it was
  run ctl2 --hjv-e2e-devices
  want 0 "S11: the login of hjv-ctl2 after the failed ones"

  log "D1: the device list answered after a logout keeps nothing"
  kill -STOP "$API_PID"
  (sleep 1.5 && kill -CONT "$API_PID") &
  lpid=$!
  run ctl2 --hjv-e2e-logout --during-devices
  wait "$lpid"
  has 'device list answered after the logout (HTTP 200' ||
    fail "D1: the device list was not answered 200 after the logout, so the key check was not reached"
  has 'hjv-my-devices empty' || fail "D1: the device list came back after the logout"
  pass "D1: $(grep -o 'device list answered after the logout.*' <<<"$OUT")"

  log "S27: the controller's clock 15 minutes ahead"
  if [ -n "${FAKETIME_LIB:-}" ] && [ -f "$FAKETIME_LIB" ]; then
    ft=(LD_PRELOAD="$FAKETIME_LIB" FAKETIME=+15m FAKETIME_DONT_FAKE_MONOTONIC=1)
    tok=$(local_option ctl access_token)
    run ctl "${ft[@]}" --hjv-e2e-devices
    want 1 "S27: the first account request 15 minutes ahead"
    has 'devices failed: HTTP 409 clock' || fail "S27: not the clock answer"
    [ -n "$tok" ] && [ "$(local_option ctl access_token)" = "$tok" ] || fail "S27: the 409 logged the controller out"
    [ -n "$(local_option ctl hjv-my-devices)" ] || fail "S27: the 409 dropped hjv-my-devices"
    off=$(local_option ctl hjv-clock-offset)
    [ "${off:-0}" -le -895 ] && [ "$off" -ge -905 ] || fail "S27: hjv-clock-offset is '$off', not about -900"
    pass "S27: 409 clock ($(sed -n 's/^hjv-e2e: devices failed: HTTP 409 clock: //p' <<<"$OUT")), no logout, hjv-clock-offset $off"
    run ctl "${ft[@]}" --hjv-e2e-devices
    want 0 "S27: the device list in the next process"
    pass "S27: the next process gets through with the learned offset"
    run ctl --hjv-e2e-local-option hjv-clock-offset 0
    run ctl "${ft[@]}" --hjv-e2e-login "$ALICE" "$ALICE_PW"
    want 1 "S27: a login 15 minutes ahead"
    has 'login failed: HTTP 409 clock' || fail "S27: the login: not the clock answer"
    run ctl "${ft[@]}" --hjv-e2e-login "$ALICE" "$ALICE_PW"
    want 0 "S27: the login again"
    keep_secrets ctl
    run ctl --hjv-e2e-local-option hjv-clock-offset 0
    pass "S27: the login 15 minutes ahead gets the clock answer, and goes through when done again"
  else
    echo "skipped: S27 (no libfaketime: FAKETIME_LIB)"
  fi

  log "X7b: hjv-api asks hbbs again and marks the PC needs-relogin; then a login on the PC"
  # with a knock for the PC's sync: a PC started again has no list yet, so it would wait its
  # default 120 s (the refresh of hjv-api's lists, 5 s here, comes with a list)
  sleep $((t_key + 61 - $(date +%s) > 0 ? t_key + 61 - $(date +%s) : 0))
  until_ok 40 "X7b: the account marks the PC needs-relogin" relogin_marked
  pass "X7b: needs-relogin $(($(date +%s) - t_key)) s after the PC came back with its new key"
  pwless ctl x7b
  want 3 "X7b: needs-relogin"
  has 'password required Password Required' || fail "X7b: not the plain password box"
  pass "X7b: needs-relogin: no warning, the password box (exit 3)"
  pc_bound
  run ctl --hjv-e2e-devices
  has "device $PC_ID linux pwless=active" || fail "X7b: the PC is not active again"
  grep -q "pk=$old " <<<"$OUT" && fail "X7b: the list still has the old key"
  pwless_ok ctl x7c "X7b: after the login on the PC"
  pass "X7b: the login on the PC stored its new key: password-less again"
  if [ ${#ft[@]} -gt 0 ]; then
    run ctl "${ft[@]}" --hjv-e2e-screenshot "$PC_ID" "" "$RUN/s27.ppm"
    want 0 "S27: password-less 15 minutes ahead"
    screen_is_pc s27 || fail "S27: the decoded screen is not the PC's desktop"
    pass "S27: a password-less session with the clock 15 minutes ahead"
  fi

  log "X4: a second PC with the current build (alice) connects to the PC"
  in_ns hjv-pc2 pc2 bash -c 'Xvfb :98 -screen 0 1280x720x24 -nolisten tcp >/dev/null 2>&1 &
    for _ in $(seq 50); do [ -e /tmp/.X11-unix/X98 ] && break; sleep 0.1; done
    DISPLAY=:98 exec "$1" --server' _ "$PC_CLIENT" >>"$RUN/pc2-server.log" 2>&1 &
  until_ok 20 "X4: the second PC's service" pc2_id
  until_ok 40 "X4: the second PC online" online "$PC2_ID" ctl
  gap
  run pc2 --hjv-e2e-login "$ALICE" "$ALICE_PW"
  want 0 "X4: alice's login on the second PC"
  keep_secrets pc2
  run pc2 --hjv-e2e-devices
  has "device $PC_ID linux pwless=active" || fail "X4: the second PC's list has no active PC"
  has "device $PC2_ID " && fail "X4: the second PC lists itself"
  pwless_ok pc2 x4 "X4: PC to PC"
  asked
  pass "X4: the second PC (alice), not in its own list, connects to the PC without a password"

  account_allowlist

  log "S20: the portal passwords, tokens and account keys in $RUN"
  keep_secrets ctl
  keep_secrets ctl2
  keep_secrets pc
  keep_secrets pc2
  local found=0 f secret key keys=0
  for secret in "$ALICE_PW" "$BOB_PW" "$CAROL_PW" "$WRONG_PW" "${SECRETS[@]}"; do
    while IFS= read -r f; do
      case "$f" in */home/.config/hjviewer/HJViewer_local.toml) ;; *)
        echo "FAIL: S20: a secret in $f"
        found=1
        ;;
      esac
    done < <(grep -rlF -- "$secret" "$RUN" 2>/dev/null || true)
  done
  # hjv-api's own keys (they open the sealed passwords, sign the lists and tag its log) stay in its
  # credentials
  for key in "$RUN"/api/creds/hjviewer-api-*-key; do
    [ -s "$key" ] || fail "S20: hjv-api's keys are not in $RUN/api/creds"
    keys=$((keys + 1))
    while IFS= read -r f; do
      case "$f" in "$RUN"/api/creds/*) ;; *)
        echo "FAIL: S20: one of hjv-api's keys in $f"
        found=1
        ;;
      esac
    done < <(grep -rlF -- "$(<"$key")" "$RUN" 2>/dev/null || true)
  done
  [ "$found" = 0 ] || exit 1
  pass "S20: ${#SECRETS[@]} tokens and keys and 4 portal passwords: none outside the clients' own LocalConfig; hjv-api's $keys keys: none outside its credentials"
}

# The open login of 2026-10-14 (server/api/README.md, Admin): an ID the SSO accepts and nobody
# imported logs in and is registered, a block logs its devices out and refuses its next login
# after the SSO, unblock lets it in again, and in list mode an ID off the list is refused without
# an SSO call. Then HJViewer's ID rule of the same day (README.md, hbbs and the SSO): an ID that is
# not a faculty or staff one gets its own answer without an SSO call, and a person the SSO
# accepts and HJViewer denies gets its own after it. On hjv-ctl2, free since D1; hjv-api starts
# again for list mode, after X7b.
account_allowlist() {
  local calls e_wrong blocked_text="이 계정은 관리자가 HJViewer 사용을 막았습니다. 다시 쓰려면 관리자에게 문의하세요."
  local id_rule_text="연세대학교 교직원의 연세포탈 ID로만 로그인할 수 있습니다. 입력한 ID를 확인해 주세요."
  local denied_text="이 계정으로는 HJViewer를 쓸 수 없습니다. 연세대학교 교직원만 쓸 수 있으며, 교직원인데도 이 문구가 나오면 관리자에게 문의하세요."
  log "S28: a portal ID never imported logs in and is registered"
  calls=$(sso_calls)
  run ctl2 --hjv-e2e-login "$CAROL" "$CAROL_PW"
  want 0 "S28: carol's login"
  keep_secrets ctl2
  [ "$(sso_calls)" = $((calls + 1)) ] || fail "S28: carol's login did not reach the fake SSO once"
  admin allow list >"$RUN/allow.txt"
  head -1 "$RUN/allow.txt" | grep -q '^allowlist: 3 IDs, 3 registered (1 by a login), 0 blocked; login open$' ||
    fail "S28: the allowlist after carol's login: $(head -1 "$RUN/allow.txt")"
  grep -q " registered  from login " "$RUN/allow.txt" || fail "S28: no entry from a login"
  grep -q "logged in and was registered (acct=" "$RUN/hjv-api.log" || fail "S28: no journal line of the registration"
  # (in any case: hjv-api keeps and shows portal IDs case-folded)
  grep -qi "$CAROL" "$RUN/hjv-api.log" "$RUN/allow.txt" && fail "S28: carol's portal ID in hjv-api's log or the masked list"
  run ctl2 --hjv-e2e-devices
  want 0 "S28: carol's device list"
  pass "S28: carol logged in (one SSO call) and is on the allowlist as a login's entry; the journal has her tag, not her ID"

  log "S29: carol blocked: her device is logged out, her next login refused after the SSO"
  admin block "$CAROL" | tee "$RUN/block.txt" | sed 's/^/    /'
  grep -q '^blocked: 1 blocked (0 of them not on the allowlist before), 0 blocked already, 1 tokens and 1 account keys revoked;' "$RUN/block.txt" ||
    fail "S29: block said: $(cat "$RUN/block.txt")"
  run ctl2 --hjv-e2e-devices
  want 1 "S29: carol's device list after the block"
  has 'devices failed: HTTP 401' || fail "S29: not a 401 for the blocked account's device ($(grep 'devices failed' <<<"$OUT"))"
  calls=$(sso_calls)
  run ctl2 --hjv-e2e-login "$CAROL" "$WRONG_PW"
  want 1 "S29: a wrong password of the blocked account"
  e_wrong=$(sed -n 's/^hjv-e2e: login failed: //p' <<<"$OUT")
  [ "$e_wrong" = "$e_bob" ] || fail "S29: the blocked account's wrong password got '$e_wrong', not '$e_bob'"
  run ctl2 --hjv-e2e-login "$CAROL" "$CAROL_PW"
  want 1 "S29: the blocked account's login"
  has "login failed: HTTP 403: $blocked_text" || fail "S29: not the blocked text ($(grep 'login failed' <<<"$OUT"))"
  [ "$(sso_calls)" = $((calls + 2)) ] || fail "S29: the blocked account's logins did not reach the fake SSO once each"
  pass "S29: the device got 401 at once; a wrong password the usual failure; the right one, after the SSO, 403 '$blocked_text'"

  log "S30: carol unblocked logs in again"
  admin unblock "$CAROL" | sed 's/^/    /'
  run ctl2 --hjv-e2e-login "$CAROL" "$CAROL_PW"
  want 0 "S30: carol's login after unblock"
  keep_secrets ctl2
  run ctl2 --hjv-e2e-devices
  want 0 "S30: carol's device list after unblock"
  pass "S30: unblock lets carol in again"

  log "S31: list mode (HJV_API_LOGIN=list): an ID off the list is refused without an SSO call"
  admin user delete "$CAROL" | tee "$RUN/delete.txt" | sed 's/^/    /'
  grep -q '(and its allowlist entry)$' "$RUN/delete.txt" || fail "S31: the deleted account's entry stayed"
  kill "$API_PID"
  wait "$API_PID" 2>/dev/null || true
  echo "HJV_API_LOGIN=list" >>"$RUN/api/hjviewer-api.env"
  start_api
  admin report >"$RUN/report.txt"
  sed -n 2p "$RUN/report.txt" | grep -q '^login: list (' || fail "S31: the report does not say list mode: $(sed -n 2p "$RUN/report.txt")"
  calls=$(sso_calls)
  run ctl2 --hjv-e2e-login "$CAROL" "$CAROL_PW"
  want 1 "S31: carol, off the list, in list mode"
  e_wrong=$(sed -n 's/^hjv-e2e: login failed: //p' <<<"$OUT")
  [ "$e_wrong" = "$e_bob" ] || fail "S31: carol off the list got '$e_wrong', not the wrong password's '$e_bob'"
  [ "$(sso_calls)" = "$calls" ] || fail "S31: the fake SSO was called for an ID off the list"
  run ctl2 --hjv-e2e-login "$BOB" "$BOB_PW"
  want 0 "S31: bob, imported, in list mode"
  keep_secrets ctl2
  # back to the default for what runs after the account part
  sed -i '/^HJV_API_LOGIN=list$/d' "$RUN/api/hjviewer-api.env"
  kill "$API_PID"
  wait "$API_PID" 2>/dev/null || true
  start_api
  admin report >"$RUN/report.txt"
  sed -n 2p "$RUN/report.txt" | grep -q '^login: open (' || fail "S31: hjv-api is not in open mode again: $(sed -n 2p "$RUN/report.txt")"
  pass "S31: in list mode carol (deleted, off the list) gets the wrong password's failure without an SSO call; bob (imported) logs in; open mode again after"

  log "S32: HJViewer's ID rule: IDs that are not faculty or staff ones are refused before the SSO; a person the SSO accepts and HJViewer denies is told so after it"
  calls=$(sso_calls)
  for id in 1234567890 zatest01 BZTEST01; do
    run ctl2 --hjv-e2e-login "$id" "$WRONG_PW"
    want 1 "S32: $id"
    has "login failed: HTTP 403: $id_rule_text" || fail "S32: $id did not get the ID rule's text ($(grep 'login failed' <<<"$OUT"))"
  done
  [ "$(sso_calls)" = "$calls" ] || fail "S32: an ID outside the rule reached the fake SSO"
  run ctl2 --hjv-e2e-login ZZSSO-DENIED "$WRONG_PW"
  want 1 "S32: a person the SSO's user call refuses"
  has "login failed: HTTP 403: $denied_text" || fail "S32: not the denial's text ($(grep 'login failed' <<<"$OUT"))"
  run ctl2 --hjv-e2e-login ZZSSO-ZA "$WRONG_PW"
  want 1 "S32: a person whose pcode starts with ZA"
  has "login failed: HTTP 403: $denied_text" || fail "S32: a ZA pcode did not get the denial's text ($(grep 'login failed' <<<"$OUT"))"
  [ "$(sso_calls)" = $((calls + 2)) ] || fail "S32: the denied logins did not reach the fake SSO once each"
  admin report >"$RUN/report.txt"
  # the denials by reason: the user call refused, a pcode outside the rule
  grep -q 'login id-rule' "$RUN/report.txt" && grep -q 'login denied-user-refused' "$RUN/report.txt" &&
    grep -q 'login denied-pcode-rule' "$RUN/report.txt" ||
    fail "S32: the report does not count the refusals by reason: $(grep -c . "$RUN/report.txt") lines"
  grep -qiE '1234567890|zatest01|bztest01|zzsso-denied|zzsso-za' "$RUN/hjv-api.log" && fail "S32: an ID typed in hjv-api's log"
  run ctl2 --hjv-e2e-devices
  want 0 "S32: the login of hjv-ctl2 (bob's) after the refused ones"
  pass "S32: a student-like number, ZA and B: 403 '$id_rule_text' without an SSO call; the user call refused and a ZA pcode: 403 '$denied_text' after one SSO call each"
}

# the device list of hjv-ctl (hjv-ctl2) has the PC in state <state>
ctl_lists() {
  run ctl --hjv-e2e-devices >/dev/null
  has "device $PC_ID linux pwless=$1"
}
ctl2_lists() {
  run ctl2 --hjv-e2e-devices >/dev/null
  has "device $PC_ID linux pwless=$1"
}

# ---------------------------------------------------------------------------------------------
# The staff-only gate (docs/access-tickets.md; docs/e2e.md, A1-A11, C1-C7 and A13-A14): hbbs's
# access file in its modes, the tickets of a login, of the bound PC, of the access password and of
# the App Review account, and the refusals as the apps show them. Its controllers are sides of
# their own in the controller's network: actl never logs in, staff is alice's, rev the App Review
# account's. PC B (pcb) runs CLIENT, which Linux does not count as installed, in the second PC's
# network, logged in nowhere; the second PC of X4 stops for it. Every call of a client ends within
# 30 s or fails the run.
# ---------------------------------------------------------------------------------------------
# the texts of a refusal for access: the app's core's (src/hjviewer_access.rs A1, A2), and hbbs's
# own (server patch 0006, docs/access-tickets.md 5-3), each with hbbs's mark
ACC_A1="HJViewer는 연세대학교 교직원용입니다. 연세포탈로 로그인하거나 이용 비밀번호를 넣으세요. (access not allowed)"
ACC_A2="서버가 이 연결을 받지 않았습니다. 잠시 뒤 다시 시도하고, 계속되면 관리자에게 알려 주세요. (access not allowed)"

# <ns> <side> <client> [NAME=value...] <args...>: as vrun, in the network <ns> and with NAME=value
# in the client's environment; the passwords are masked in $RUN/<side>.out
arun() {
  local ns=$1 side=$2 client=$3 t0 p envs=() line
  shift 3
  while [[ ${1:-} =~ ^[A-Z_][A-Z0-9_]*= ]]; do
    envs+=("$1")
    shift
  done
  line="${envs[*]} $*"
  # (EL4 calls it in the account part, which runs without the access part's passwords too)
  for p in "$PASSWORD" "$ALICE_PW" "${GUEST_PW:-}" "${WRONG_GUEST_PW:-}" "${REVIEW_PW:-}"; do line=${line//"$p"/***}; done
  mkdir -p "$RUN/$side/home" "$RUN/$side/ipc"
  t0=$(date +%s%N)
  RC=0
  OUT=$(in_ns "$ns" "$side" timeout -k 5 30 env "${envs[@]}" "$client" "$@" 2>"$RUN/$side.err") || RC=$?
  MS=$(ms_since "$t0")
  ERR=$(grep '^hjv-e2e:' "$RUN/$side.err" || true)
  {
    echo "== $(basename "$client") $line (exit $RC, $MS ms)"
    printf '%s\n' "$OUT"
    cat "$RUN/$side.err"
  } >>"$RUN/$side.out"
  case $RC in 124 | 137) fail "$side: '$line' did not end within 30 s" ;; esac
}
ahas() { grep -qF -- "$1" <<<"$OUT"$'\n'"$ERR"; } # <text>: in the last call's hjv-e2e lines or output
# <side> <PC ID> <tag> [NAME=value...]: that PC's remote screen from the controller's network with
# the PCs' password, into $RUN/<tag>.ppm
ascreen() { arun hjv-ctl "$1" "$CLIENT" "${@:4}" --hjv-e2e-screenshot "$2" "$PASSWORD" "$RUN/$3.ppm"; }
# <scenario> <tag>: the last call was refused for access with the core's text <text>: exit 5, no screen
refused_with() {
  [ "$RC" = 5 ] || fail "$1: exit $RC, expected 5 (a refusal for access): $(grep '^hjv-e2e:' <<<"$OUT" | tail -n 1)"
  ahas "hjv-e2e: access refused" || fail "$1: no 'access refused' line"
  ahas "$3" || fail "$1: not the text '$3': $(grep '^hjv-e2e: hjv-access-error' <<<"$OUT")"
  if ahas 'logged in to' || [ -e "$RUN/$2.ppm" ]; then fail "$1: a session started"; fi
}
screen_ok() { # <scenario> <tag>: the last call got the PC's screen (both PCs show the same desktop)
  [ "$RC" = 0 ] || fail "$1: exit $RC ($(grep '^hjv-e2e:' <<<"$OUT" | tail -n 1))"
  screen_is_pc "$2" || fail "$1: the decoded screen is not the PC's desktop"
}
access_console() { # hbbs's answer to hjv-access on its loopback console (server patch 0006)
  timeout 3 bash -c 'exec 3<>"/dev/tcp/127.0.0.1/$1" && printf hjv-access >&3 && cat <&3' _ "$((HBBS_PORT - 1))" || true
}
console_count() { # <point.reason>: its count in the console's line, 0 without one
  local n
  n=$(access_console | tr ' ' '\n' | sed -n "s/^${1//./\\.}=//p")
  echo "${n:-0}"
}
console_has() { # <point.reason>...: each counted at least once
  local c
  for c in "$@"; do [ "$(console_count "$c")" -gt 0 ] || return 1; done
}
gate_is() { # <mode>: GET /api/hjv/access, without a token, from the controller's network; with
  # or without "codes":true (access-tickets.md 3-6: count and enforce while hjv-api has its ticket key)
  GATE=$(in_ns hjv-ctl actl curl -s --max-time 5 --cacert "$CA_ROOT" "https://$DOMAIN/api/hjv/access" || true)
  [ "$GATE" = "{\"mode\":\"$1\"}" ] || [ "$GATE" = "{\"codes\":true,\"mode\":\"$1\"}" ]
}
hbbs_count() { grep -cF -- "$1" "$RUN/hbbs.log" || true; }
hbbs_above() { [ "$(hbbs_count "$1")" -gt "$2" ]; } # <text> <count before>
PCB_STATE="" GATE=""
pcb_is() { # <state>: PC B's hjv-access-pc, from its service: ok or refused
  arun hjv-pc2 pcb "$CLIENT" --hjv-e2e-option hjv-access-pc
  PCB_STATE=$OUT
  [ "$OUT" = "$1" ]
}
start_pcb() { # PC B's desktop on the display :97, and its service
  in_ns hjv-pc2 pcb bash -c '
    Xvfb :97 -screen 0 1280x720x24 -nolisten tcp >/dev/null 2>&1 &
    for _ in $(seq 50); do [ -e /tmp/.X11-unix/X97 ] && break; sleep 0.1; done
    export DISPLAY=:97
    display -geometry +0+0 "$1" &
    exec "$2" --server
  ' _ "$RUN/pc/desktop.png" "$CLIENT" >>"$RUN/pcb-server.log" 2>&1 &
}
restart_pcb_service() { # on the display PC B still has, as start_pc_service
  local pids
  pids=$(sudo ip netns pids hjv-pc2 | xargs -r ps -o pid=,args= -p | awk '$0 ~ / --server$/ {print $1}')
  [ -z "$pids" ] || sudo kill $pids
  sleep 1
  in_ns hjv-pc2 pcb env DISPLAY=:97 "$CLIENT" --server >>"$RUN/pcb-server.log" 2>&1 &
}
guest_tries() { # the access password's tries of the last day, as admin ticket status counts them
  admin ticket status | sed -n "s/^access password, the last day's tries: //p"
}
guest_tried() { [[ ", $(guest_tries), " == *", $1, "* ]]; } # <result> <n>, e.g. "wrong 1"

code_records() { # the support codes' records of the last day, as admin ticket status counts them
  admin ticket status | sed -n "s/^support codes, the last day's records: //p"
}
code_recorded() { [[ ", $(code_records), " == *", $1, "* ]]; } # <result> <n>, e.g. "used 1"
gate_has_codes() { # <mode>: GET /api/hjv/access says the mode and "codes":true
  gate_is "$1" && [ "$GATE" = "{\"codes\":true,\"mode\":\"$1\"}" ]
}
pcb_offline() { ! online "$PCB_ID" ctl; }
# the form hjv-api shows a code in: two times four of Crockford base32's symbols
CODE_FORM='^[0-9A-HJKMNP-TV-Z]{4}-[0-9A-HJKMNP-TV-Z]{4}$'

# The support codes (docs/access-tickets.md 3-7; docs/e2e.md, C1-C7), in enforce after A10 and
# before A11: alice's controller (staff) makes a code, PC B, logged in nowhere and started again
# without the access password's ticket of A8, trades it for its own ticket and registers, and
# alice connects to it. The access password's file is gone since A9: codes do not read it. No
# expiry run: the ten minutes and the ticket's two hours stay as they are (hjv-api's tests).
support_code_part() {
  local code n0 ms ms2 fake
  log "C1: MODE=enforce and hjv-api's ticket key: GET /api/hjv/access says \"codes\":true"
  until_ok 5 "C1: GET /api/hjv/access with \"codes\":true" gate_has_codes enforce
  pass "C1: GET /api/hjv/access $GATE (count or enforce, with the ticket key; access-tickets.md 3-6)"

  log "C2: alice's login may make support codes; the App Review account's may not"
  [ "$(local_option staff hjv-support-code)" = Y ] ||
    fail "C2: alice's login answer did not carry \"support_code\":true (hjv-support-code '$(local_option staff hjv-support-code)')"
  [ -z "$(local_option rev hjv-support-code)" ] || fail "C2: the App Review account's login answer carried \"support_code\":true"
  arun hjv-ctl rev "$CLIENT" --hjv-e2e-support-code
  [ "$RC" = 6 ] && ahas "hjv-e2e: support code: no_code" ||
    fail "C2: the App Review account's make: exit $RC ($(grep '^hjv-e2e: support code' <<<"$OUT")), expected 6 and no_code"
  pass "C2: alice's login answer has hjv.support_code true (the app keeps hjv-support-code Y), the App Review account's none, and its make gets 403 no_code (exit 6); currentUser's flag is hjv-api's tests' (tests/support.rs, account.rs): no headless command asks it"

  log "C3: alice makes a support code (POST /api/hjv/support-code, signed)"
  arun hjv-ctl staff "$CLIENT" --hjv-e2e-support-code
  [ "$RC" = 0 ] || fail "C3: alice's make: exit $RC ($(grep '^hjv-e2e: support code' <<<"$OUT"))"
  code=$(sed -n 's/^hjv-e2e: support code: //p' <<<"$OUT" | tail -n 1)
  [[ $code =~ $CODE_FORM ]] || fail "C3: the code is not of the form XXXX-XXXX: '$code'"
  until_ok 5 "C3: admin ticket status counts 'made 1'" code_recorded "made 1"
  pass "C3: exit 0 after $MS ms with a code of the form XXXX-XXXX; admin ticket status: $(admin ticket status | sed -n 's/^support codes: \([0-9]* open\).*/\1/p'), records $(code_records)"

  log "C4: PC B, logged in nowhere and without a ticket, trades the code and registers"
  # A8's access password ticket lived in PC B's service: started again, it registers with none
  restart_pcb_service
  until_ok 45 "C4: PC B's hjv-access-pc refused after its service started again" pcb_is refused
  until_ok 35 "C4: hbbs has PC B offline before the code" pcb_offline
  n0=$(console_count reg.ok)
  arun hjv-pc2 pcb "$CLIENT" --hjv-e2e-guest-pc-code "$code"
  [ "$RC" = 0 ] && ahas "hjv-e2e: guest pc code: ok" ||
    fail "C4: exit $RC ($(grep '^hjv-e2e: guest' <<<"$OUT")), expected 0 and 'guest pc code: ok'"
  ms=$MS
  until_ok 10 "C4: PC B's hjv-access-pc ok" pcb_is ok
  [ "$(console_count reg.ok)" -gt "$n0" ] || fail "C4: the console's reg.ok did not grow"
  until_ok 20 "C4: PC B online" online "$PCB_ID" ctl
  until_ok 5 "C4: admin ticket status counts 'ok-pc 1'" code_recorded "ok-pc 1"
  [ ! -e "$RUN/pcb/home/.config/hjviewer/HJViewer_ticket.json" ] || fail "C4: PC B keeps a ticket file"
  pass "C4: refused without a ticket, then 'guest pc code: ok' ($ms ms, no access password file); within 10 s hjv-access-pc ok, reg.ok up, PC B online; records $(code_records)"

  log "C5: alice's controller connects to PC B"
  ascreen staff "$PCB_ID" c5
  screen_ok "C5: alice's controller to PC B" c5
  pass "C5: the PC's screen ($MS ms) with alice's ticket and PC B's password"

  log "C6: the same code again: the one answer for a code that does not work"
  arun hjv-pc2 pcb "$CLIENT" --hjv-e2e-guest-pc-code "$code"
  [ "$RC" = 6 ] && ahas "hjv-e2e: guest pc code: denied" ||
    fail "C6: exit $RC ($(grep '^hjv-e2e: guest' <<<"$OUT")), expected 6 and denied"
  [ "$MS" -ge 1500 ] || fail "C6: the refusal came after $MS ms, before hjv-api's failure delay"
  ms2=$MS
  until_ok 5 "C6: admin ticket status counts 'used 1'" code_recorded "used 1"
  pcb_is ok || fail "C6: PC B lost its ticket: hjv-access-pc '$PCB_STATE'"
  pass "C6: exit 6 after $ms2 ms ('guest pc code: denied', 401); PC B keeps the ticket it has; records $(code_records)"

  log "C7: a made-up code gets the same answer"
  fake=$code
  while [ "$fake" = "$code" ]; do
    fake=$(python3 -I -c 'import secrets
s = "".join(secrets.choice("0123456789ABCDEFGHJKMNPQRSTVWXYZ") for _ in range(8))
print(s[:4] + "-" + s[4:])')
  done
  arun hjv-pc2 pcb "$CLIENT" --hjv-e2e-guest-pc-code "$fake"
  [ "$RC" = 6 ] && ahas "hjv-e2e: guest pc code: denied" ||
    fail "C7: exit $RC ($(grep '^hjv-e2e: guest' <<<"$OUT")), expected 6 and denied"
  [ "$MS" -ge 1500 ] || fail "C7: the refusal came after $MS ms, before hjv-api's failure delay"
  until_ok 5 "C7: admin ticket status counts 'wrong 1'" code_recorded "wrong 1"
  pass "C7: exit 6 after $MS ms ('guest pc code: denied', as C6 after $ms2 ms): the answer does not tell a used code from a made-up one; records $(code_records)"
}

# TV-W6 (docs/e2e.md W6-E1 to W6-E7, docs/access-tickets.md 3-7): after C1-C7, in enforce, with
# PC B registered by C4's code. The support link and its page and package (E1, E2), the waiting
# room (E3), the staff mark on PC B's connection manager and the input PC B drops before [수락]
# (E4, E4b, E5), a second profile of the same account (E6), and the mark's end with PC B's ticket
# (E7). PC B gets a connection manager of its own here (its log $RUN/pcb-cm.log), and the access
# password's file is back for E4's controller without a login and E7's PC B.
pcb_log() { cat "$RUN/pcb-server.log" "$RUN"/pcb/home/.local/share/logs/HJViewer/server/*.log 2>/dev/null || true; }
pcb_log_count() { pcb_log | grep -cF -- "$1" || true; }
start_pcb_cm() { # [NAME=value]: PC B's connection manager, the one before it gone
  local n
  sudo ip netns pids hjv-pc2 | xargs -r ps -o pid=,args= -p | awk '/--hjv-e2e-cm/ {print $1}' | xargs -r sudo kill 2>/dev/null || true
  n=$(grep -c 'cm listening' "$RUN/pcb-cm.log" 2>/dev/null || true)
  in_ns hjv-pc2 pcb env "$@" "$CLIENT" --hjv-e2e-cm >>"$RUN/pcb-cm.log" 2>&1 &
  until_ok 10 "PC B's connection manager" pcb_cm_listening "${n:-0}"
}
pcb_cm_listening() { [ "$(grep -c 'cm listening' "$RUN/pcb-cm.log" 2>/dev/null || true)" -gt "$1" ]; }
pcb_cm_lines() { wc -l <"$RUN/pcb-cm.log"; }
pcb_dropped() { # the input PC B's service dropped before an accept, counted since its start (hjv-e2e)
  arun hjv-pc2 pcb "$CLIENT" --hjv-e2e-option hjv_e2e_dropped_input
  echo "${OUT:-0}"
}
pcb_dropped_at_least() { [ "$(pcb_dropped)" -ge "$1" ]; }
pcb_pointer() { in_ns hjv-pc2 pcb env DISPLAY=:97 xdotool getmouselocation 2>/dev/null | grep -o 'x:[0-9]* y:[0-9]*' || true; }
# <URL>: from PC B's network, the status and the address a redirect points to; the body in $RUN/w6.body
link_get() { in_ns hjv-pc2 pcb curl -s --max-time 10 -o "$RUN/w6.body" -w '%{http_code} %{redirect_url}' "$1" || true; }
LINK_FORM='^https://hjv\.test/api/hjv/s/[A-Za-z0-9_-]{22}/$'

support_link_part() {
  local code link slot base got i pkg wrong ms n0 cm0 d0 d1 d2 p0 p1 p2 staff_id staff_sp actl_id s2_id marks
  guest_conf
  start_pcb_cm
  arun hjv-ctl staff "$CLIENT" --hjv-e2e-id
  staff_id=${OUT%% *}
  staff_sp=$(spaced_id "$staff_id")

  log "W6-E1: alice makes a support code; hjv-api gives its support link and waiting number"
  arun hjv-ctl staff "$CLIENT" --hjv-e2e-support-code
  [ "$RC" = 0 ] || fail "W6-E1: exit $RC ($(grep '^hjv-e2e: support' <<<"$OUT"))"
  code=$(sed -n 's/^hjv-e2e: support code: //p' <<<"$OUT")
  link=$(sed -n 's/^hjv-e2e: support link: //p' <<<"$OUT")
  slot=$(sed -n 's/^hjv-e2e: support slot: //p' <<<"$OUT")
  [[ $code =~ $CODE_FORM ]] || fail "W6-E1: the code is not of the form XXXX-XXXX: '$code'"
  [[ $link =~ $LINK_FORM ]] || fail "W6-E1: the support link is not https://hjv.test/api/hjv/s/<22 base64url>/: '$link'"
  [[ $slot =~ ^[1-9][0-9]*$ ]] || fail "W6-E1: the slot is '$slot'"
  pass "W6-E1: a code, its support link (/api/hjv/s/ and 22 base64url characters, the whole address ending in /) and slot $slot"

  log "W6-E2: the support link from PC B's network: its page and the package, three times"
  if [ -z "$POLICY" ]; then
    echo "skipped: W6-E2 (no version policy, so hjv-api has no Windows package to offer: HJV_RELEASE)"
  else
    base=${link%/}
    got=$(link_get "$base")
    [ "$got" = "307 $link" ] || fail "W6-E2: the link without its / answers '$got', not 307 to $link"
    got=$(link_get "$link")
    [ "$got" = "200 " ] || fail "W6-E2: the page answers '$got'"
    grep -qF -- "$code" "$RUN/w6.body" || fail "W6-E2: the page does not show the code"
    pkg="HJViewer-$NEXT_MINOR.exe"
    grep -qF -- "href=\"$pkg\"" "$RUN/w6.body" || fail "W6-E2: the page offers no $pkg"
    for i in 1 2 3; do
      got=$(link_get "$link$pkg")
      [ "$got" = "200 " ] || fail "W6-E2: download $i of $pkg answers '$got'"
    done
    got=$(link_get "$link$pkg")
    [ "${got%% *}" = 404 ] || fail "W6-E2: the fourth download answers '$got', not 404"
    wrong="https://$DOMAIN/api/hjv/s/$(head -c 16 /dev/urandom | base64 | tr '+/' '-_' | tr -d '=')/"
    got=$(link_get "$wrong")
    [ "${got%% *}" = 404 ] || fail "W6-E2: a made-up link answers '$got', not 404"
    pass "W6-E2: 307 to the address with /, the page (200) with the code and $pkg, three downloads 200 and the fourth 404, a made-up link 404"
  fi

  log "W6-E3: the waiting room: alice waits, PC B trades the code, alice learns PC B's ID"
  bg_start e3 hjv-ctl staff "$CLIENT" --hjv-e2e-support-wait 25
  until_ok 15 "W6-E3: the code made in the waiting process" bg_has e3 'hjv-e2e: support slot: '
  code=$(sed -n 's/^hjv-e2e: support code: //p' "$RUN/e3.bg")
  link=$(sed -n 's/^hjv-e2e: support link: //p' "$RUN/e3.bg")
  [[ $code =~ $CODE_FORM ]] && [[ $link =~ $LINK_FORM ]] || fail "W6-E3: the waiting process made '$code' and '$link'"
  n0=$(pcb_log_count 'a support grant refused')
  arun hjv-pc2 pcb "$CLIENT" --hjv-e2e-guest-pc-code "$code"
  [ "$RC" = 0 ] && ahas "hjv-e2e: guest pc code: ok" ||
    fail "W6-E3: PC B's trade: exit $RC ($(grep '^hjv-e2e: guest' <<<"$OUT"))"
  bg_wait e3 10
  [ "$BG_RC" = 0 ] && grep -qxF "hjv-e2e: support arrived: $PCB_ID" <<<"$BG_OUT" ||
    fail "W6-E3: the waiting process: exit $BG_RC, $(grep '^hjv-e2e: support arrived' <<<"$BG_OUT")"
  [ "$BG_MS" -le 5000 ] || fail "W6-E3: PC B's ID came $BG_MS ms after the trade"
  ms=$BG_MS
  until_ok 10 "W6-E3: PC B's hjv-access-pc ok" pcb_is ok
  [ "$(pcb_log_count 'a support grant refused')" = "$n0" ] || fail "W6-E3: PC B refused the support grant: $(pcb_log | grep -o 'a support grant refused.*' | tail -1)"
  got=$(link_get "$link")
  [ "${got%% *}" = 404 ] || fail "W6-E3: the used code's link answers '$got', not 404"
  pass "W6-E3: 'guest pc code: ok'; the waiting process printed PC B's ID $ms ms after the trade (exit 0); PC B took the support grant; the used code's link is 404"

  # A headless command is a process of its own, and the app keeps its support PCs in its memory
  # only (src/hjviewer_account/support.rs): E4, E4b and E5 need the support PC that E3's process
  # learned in the next ones, which an hjv-e2e build hands on in LocalConfig hjv-e2e-support-pc
  if grep -aqF hjv-e2e-support-pc "$CLIENT"; then
    log "W6-E4: alice's controller to PC B without a password: the staff mark, and the wait for [수락]"
    cm0=$(pcb_cm_lines)
    marks=$(pcb_log_count "hjv: support mark for $staff_id")
    arun hjv-ctl staff "$CLIENT" --hjv-e2e-screenshot "$PCB_ID" "" "$RUN/e4.ppm"
    [ "$RC" = 1 ] && ahas 'login error: No Password Access' && ahas 'wait-remote-accept-nook' ||
      fail "W6-E4: exit $RC, not the wait for [수락]: $(grep '^hjv-e2e:' <<<"$OUT" | tail -n 2 | tr '\n' ' ')"
    if ahas 'Could not connect without a password' || ahas '비밀번호 없이 연결하지 못했습니다'; then fail "W6-E4: the same-account failure box"; fi
    until_ok 5 "W6-E4: PC B's connection manager shows the staff mark" \
      cm_file_since "$RUN/pcb-cm.log" "$cm0" "cm login from $staff_id named \"$staff_sp\", authorized false, mark staff()"
    [ "$(pcb_log_count "hjv: support mark for $staff_id")" -gt "$marks" ] || fail "W6-E4: no 'hjv: support mark for $staff_id' in PC B's log"
    arun hjv-ctl actl "$CLIENT" --hjv-e2e-id
    actl_id=${OUT%% *}
    cm0=$(pcb_cm_lines)
    ascreen actl "$PCB_ID" e4a HJV_E2E_ACCESS_PASSWORD="$GUEST_PW"
    screen_ok "W6-E4: the controller without a login, with the access password and PC B's password" e4a
    cm_file_since "$RUN/pcb-cm.log" "$cm0" "cm login from $actl_id named \"[^\"]*\", authorized true, mark none()" ||
      fail "W6-E4: the access password's session is not mark none(): $(tail -n 1 "$RUN/pcb-cm.log")"
    pass "W6-E4: PC B's connection manager: named \"$staff_sp\", authorized false, mark staff() ('hjv: support mark for $staff_id'); the controller waits for [수락] (No Password Access, no failure box); the access password's session: mark none()"

    log "W6-E4b: the same at PC B set to 'password only'"
    arun hjv-pc2 pcb "$CLIENT" --hjv-e2e-option approve-mode password
    cm0=$(pcb_cm_lines)
    arun hjv-ctl staff "$CLIENT" --hjv-e2e-screenshot "$PCB_ID" "" "$RUN/e4b.ppm"
    [ "$RC" = 3 ] && ahas 'login error: Empty Password' && ahas 'password required' ||
      fail "W6-E4b: exit $RC, not 'Empty Password' and the password box: $(grep '^hjv-e2e:' <<<"$OUT" | tail -n 2 | tr '\n' ' ')"
    if ahas 'Could not connect without a password'; then fail "W6-E4b: the same-account failure box"; fi
    until_ok 5 "W6-E4b: the staff mark, not accepted" \
      cm_file_since "$RUN/pcb-cm.log" "$cm0" "cm login from $staff_id named \"$staff_sp\", authorized false, mark staff()"
    arun hjv-pc2 pcb "$CLIENT" --hjv-e2e-option approve-mode ""
    pass "W6-E4b: 'password only': 'Empty Password', the plain password box (exit 3, no failure box); the connection manager: authorized false, mark staff()"

    log "W6-E5: no input before [수락]: PC B's connection manager accepts 10 s after the request"
    start_pcb_cm HJV_E2E_CM_ACCEPT_AFTER=10
    cm0=$(pcb_cm_lines)
    d0=$(pcb_dropped)
    p0=$(pcb_pointer)
    [ -n "$p0" ] || fail "W6-E5: xdotool does not read PC B's pointer"
    bg_start e5 hjv-ctl staff "$CLIENT" --hjv-e2e-input-probe "$PCB_ID" 20
    until_ok 20 "W6-E5: the probe's moves before the accept" bg_has e5 'hjv-e2e: input probe: 20 moves before accept'
    until_ok 5 "W6-E5: PC B dropped the 20 moves (hjv_e2e_dropped_input from $d0)" pcb_dropped_at_least $((d0 + 20))
    d1=$(pcb_dropped)
    p1=$(pcb_pointer)
    if cm_file_since "$RUN/pcb-cm.log" "$cm0" 'cm accepts'; then fail "W6-E5: the connection manager accepted before the count was read"; fi
    [ "$p1" = "$p0" ] || fail "W6-E5: PC B's pointer moved before [수락]: $p0 -> $p1"
    bg_wait e5 30
    [ "$BG_RC" = 0 ] && grep -qF 'hjv-e2e: input probe: 20 moves after accept' <<<"$BG_OUT" ||
      fail "W6-E5: the probe: exit $BG_RC, $(grep '^hjv-e2e:' <<<"$BG_OUT" | tail -n 1)"
    cm_file_since "$RUN/pcb-cm.log" "$cm0" 'cm accepts' || fail "W6-E5: the connection manager did not accept"
    d2=$(pcb_dropped)
    p2=$(pcb_pointer)
    [ "$d2" = "$d1" ] || fail "W6-E5: PC B dropped input after the accept: $d1 -> $d2"
    [ "$p2" != "$p0" ] || fail "W6-E5: PC B's pointer did not move after [수락] ($p2)"
    pass "W6-E5: before [수락] PC B dropped $((d1 - d0)) inputs (hjv_e2e_dropped_input $d0 -> $d1) and its pointer stayed at $p0; after the connection manager's accept the probe's moves took it to $p2 and nothing more was dropped"
  else
    echo "skipped: W6-E4, W6-E4b, W6-E5 (the client keeps the support PC that the waiting room named in the waiting process only; no hjv-e2e-support-pc in $CLIENT)"
  fi

  log "W6-E6: a second profile of alice's account (it made no code) to PC B"
  arun hjv-ctl2 staff2 "$CLIENT" --hjv-e2e-login "$ALICE" "$ALICE_PW"
  [ "$RC" = 0 ] || fail "W6-E6: alice's login on the second profile: exit $RC"
  s2_id=$(sed -n 's/^hjv-e2e: logged in as .* on \([0-9]*\)$/\1/p' <<<"$OUT")
  [ -n "$s2_id" ] || fail "W6-E6: the second profile's login did not say its ID"
  cm0=$(pcb_cm_lines)
  arun hjv-ctl2 staff2 "$CLIENT" --hjv-e2e-screenshot "$PCB_ID" "$PASSWORD" "$RUN/e6.ppm"
  screen_ok "W6-E6: the second profile with PC B's password" e6
  cm_file_since "$RUN/pcb-cm.log" "$cm0" "cm login from $s2_id named \"[^\"]*\", authorized true, mark none()" ||
    fail "W6-E6: the second profile's session is not mark none(): $(tail -n 1 "$RUN/pcb-cm.log")"
  [ "$(pcb_log_count "hjv: support mark for $s2_id")" = 0 ] || fail "W6-E6: PC B marked the second profile"
  pass "W6-E6: the same account's other profile (another account key, no code) gets PC B's screen with its password: mark none(), no support mark"

  log "W6-E7: PC B ends its ticket and comes back with the access password: the mark goes with the ticket"
  arun hjv-pc2 pcb "$CLIENT" --hjv-e2e-guest-pc-end
  [ "$RC" = 0 ] && ahas 'hjv-e2e: guest pc end' || fail "W6-E7: --hjv-e2e-guest-pc-end: exit $RC"
  arun hjv-pc2 pcb "$CLIENT" --hjv-e2e-guest-pc "$GUEST_PW"
  [ "$RC" = 0 ] && ahas "hjv-e2e: guest pc: ok" || fail "W6-E7: PC B's access password: exit $RC ($(grep '^hjv-e2e: guest' <<<"$OUT"))"
  until_ok 10 "W6-E7: PC B's hjv-access-pc ok" pcb_is ok
  until_ok 20 "W6-E7: PC B online" online "$PCB_ID" ctl
  cm0=$(pcb_cm_lines)
  marks=$(pcb_log_count "hjv: support mark for $staff_id")
  ascreen staff "$PCB_ID" e7
  screen_ok "W6-E7: alice's controller to PC B after its ticket ended" e7
  cm_file_since "$RUN/pcb-cm.log" "$cm0" "cm login from $staff_id named \"[^\"]*\", authorized true, mark none()" ||
    fail "W6-E7: alice's session is not mark none(): $(tail -n 1 "$RUN/pcb-cm.log")"
  [ "$(pcb_log_count "hjv: support mark for $staff_id")" = "$marks" ] || fail "W6-E7: PC B marked alice's controller after its ticket ended"
  pass "W6-E7: after 'guest pc end' and the access password, alice's controller gets PC B's screen with its password and no mark (mark none()): the support grant ended with the code's ticket"
}

access_part() {
  local t0 n0 n1 nb before ms
  mkdir -p "$RUN"/{actl,staff,rev,pcb}/{home,ipc}
  log "A1: MODE=off: hbbs checks nothing (a PC with no ticket registers); the apps read the mode"
  hbbs_has "access off" || fail "A1: hbbs did not log 'access off' (HJV_ACCESS_FILE, server patch 0006)"
  [[ $(access_console) == "HJVACCESS1 mode=off"* ]] || fail "A1: the console's hjv-access: '$(access_console)'"
  # PC B in the second PC's network, where X4's PC stops
  sudo ip netns pids hjv-pc2 | xargs -r sudo kill 2>/dev/null || true
  sleep 0.5
  PCB_ID=$(in_ns hjv-pc2 pcb "$CLIENT" --hjv-e2e-set-password "$PASSWORD" | tail -1)
  [[ $PCB_ID =~ ^[0-9]+$ ]] || fail "A1: PC B's ID: '$PCB_ID'"
  start_pcb
  until_ok 40 "A1: PC B online" online "$PCB_ID" ctl
  ascreen actl "$PCB_ID" a1
  screen_ok "A1: the controller without a login, to PC B with its password" a1
  ms=$MS
  until_ok 10 "A1: PC B's hjv-access-pc ok" pcb_is ok
  until_ok 5 "A1: GET /api/hjv/access" gate_is off
  pass "A1: MODE=off ('access off', console 'HJVACCESS1 mode=off'): PC B ($PCB_ID), with no ticket, registered (hjv-access-pc ok) and the controller without a login got its screen with the password ($ms ms); GET /api/hjv/access {\"mode\":\"off\"}, without counts"

  log "A2: MODE=count: hbbs judges and counts, and refuses nothing"
  access_conf count
  until_ok 13 "A2: hbbs's line 'access count: keys 1'" hbbs_has "access count: keys 1"
  # the PCs register again, now judged: PC A with its bound PC's ticket, PC B with none
  stop_pc_service
  start_pc_service
  restart_pcb_service
  until_ok 40 "A2: the PC online again" online "$PC_ID" ctl
  until_ok 40 "A2: PC B online again" online "$PCB_ID" ctl
  ascreen actl "$PCB_ID" a2
  screen_ok "A2: the controller without a login, to PC B" a2
  arun hjv-ctl staff "$CLIENT" --hjv-e2e-login "$ALICE" "$ALICE_PW"
  [ "$RC" = 0 ] || fail "A2: alice's login on the controller staff: exit $RC"
  ascreen staff "$PC_ID" a2s
  screen_ok "A2: alice's controller to the PC" a2s
  until_ok 15 "A2: the console's ph.missing, ph.ok, reg.ok and reg.missing" \
    console_has ph.missing ph.ok reg.ok reg.missing
  [ "$(hbbs_count 'access refused')" = 0 ] || fail "A2: hbbs refused in count mode: $(grep -m1 -F 'access refused' "$RUN/hbbs.log")"
  pass "A2: MODE=count: both connections went through; hbbs counted ph.missing (no login), ph.ok (alice's ticket), reg.ok (the PC's bound PC ticket) and reg.missing (PC B), and refused nothing"

  log "A3: MODE=enforce: the controller without a login is refused once, with the app's A1"
  n0=$(hbbs_count 'access refused ph ')
  access_conf enforce
  until_ok 13 "A3: hbbs's line 'access enforce: keys 1'" hbbs_has "access enforce: keys 1"
  ascreen actl "$PC_ID" a3
  refused_with A3 a3 "$ACC_A1"
  until_ok 5 "A3: hbbs's line 'access refused ph … : missing'" hbbs_above 'access refused ph ' "$n0"
  # a second request of the same attempt would be in by now
  sleep 2
  [ "$(hbbs_count 'access refused ph ')" = $((n0 + 1)) ] ||
    fail "A3: hbbs refused $(($(hbbs_count 'access refused ph ') - n0)) punch hole requests of one attempt"
  grep -F 'access refused ph ' "$RUN/hbbs.log" | tail -n 1 | grep -q ': missing$' || fail "A3: the refusal is not for a missing ticket"
  until_ok 35 "A3: GET /api/hjv/access" gate_is enforce
  pass "A3: exit 5 after $MS ms with the app's A1 ('… (access not allowed)'), one refusal in hbbs's log (missing); GET /api/hjv/access $GATE within its 30 s"

  log "A4: alice's controller connects to the PC with her login's ticket"
  ascreen staff "$PC_ID" a4
  screen_ok "A4: alice's controller to the PC" a4
  ms=$MS
  arun hjv-ctl staff "$CLIENT" --hjv-e2e-ticket refresh
  [ "$RC" = 0 ] || fail "A4: --hjv-e2e-ticket refresh: exit $RC"
  # a controller's: this side never ran as a PC, so it has no device key to ask for both with
  ahas '"use":"ctl"' && ahas '"valid":true' || fail "A4: not a valid controller's ticket: $(grep '^hjv-e2e: ticket {' <<<"$OUT")"
  pass "A4: the PC's screen ($ms ms); the login's ticket, asked again (POST /api/hjv/ticket), is valid and a controller's (ctl: no device key on this side)"

  log "A5: PC B, with no ticket, is refused: offline; the PC stays online with its bound PC's ticket"
  until_ok 30 "A5: PC B's hjv-access-pc refused" pcb_is refused
  if online "$PCB_ID" ctl; then fail "A5: hbbs has PC B online"; fi
  online "$PC_ID" ctl || fail "A5: the PC is not online"
  pass "A5: PC B's registration refused (hjv-access-pc refused), hbbs has it offline; the PC online (reg.ok $(console_count reg.ok))"

  log "A6: the access password: the controller without a login gets a ticket for the PC and connects"
  ascreen actl "$PC_ID" a6 HJV_E2E_ACCESS_PASSWORD="$GUEST_PW"
  ahas "hjv-e2e: guest ctl: ok" || fail "A6: no 'guest ctl: ok' ($(grep '^hjv-e2e: guest' <<<"$OUT"))"
  screen_ok "A6: with the access password" a6
  # A6′ (UX-G1): what A0 reads, hjv-guest-held:<PC A>, is Y while the ticket is held for the
  # session, and empty once its last session closed (A0 then asks again)
  [ "$(grep -o '^hjv-e2e: \(guest held: .\|screen \)' <<<"$OUT" | tr '\n' '|')" = "hjv-e2e: guest held: Y|hjv-e2e: screen |hjv-e2e: guest held: -|" ] ||
    fail "A6′: not 'guest held: Y' before the screen and 'guest held: -' after it: $(grep '^hjv-e2e: \(guest held\|screen\)' <<<"$OUT" | tr '\n' ' ')"
  # the ticket was that process's: a new one without the password is refused again
  ascreen actl "$PC_ID" a6b
  refused_with "A6: a new process without the access password" a6b "$ACC_A1"
  pass "A6: 'guest ctl: ok', then the PC's screen; a new process without the password is refused as in A3 (the ticket lived in the first one's memory); A6′: 'guest held: Y' before the session, 'guest held: -' after it closed"

  log "A7: a wrong access password: denied after the failure delay, no session"
  ascreen actl "$PC_ID" a7 HJV_E2E_ACCESS_PASSWORD="$WRONG_GUEST_PW"
  [ "$RC" = 6 ] || fail "A7: exit $RC, expected 6 (no ticket)"
  ahas "hjv-e2e: guest ctl: denied" || fail "A7: not 'guest ctl: denied' ($(grep '^hjv-e2e: guest' <<<"$OUT"))"
  [ "$MS" -ge 1500 ] || fail "A7: the refusal came after $MS ms, before hjv-api's failure delay (2 s ± 0.5 s)"
  [ ! -e "$RUN/a7.ppm" ] || fail "A7: a session started"
  until_ok 5 "A7: admin ticket status counts 'wrong 1'" guest_tried "wrong 1"
  pass "A7: exit 6 after $MS ms ('guest ctl: denied'); admin ticket status: $(guest_tries)"

  log "A8: PC B takes the access password: its service registers with the PC's ticket"
  n0=$(console_count reg.ok)
  arun hjv-pc2 pcb "$CLIENT" --hjv-e2e-guest-pc "$GUEST_PW"
  [ "$RC" = 0 ] && ahas "hjv-e2e: guest pc: ok" || fail "A8: exit $RC ($(grep '^hjv-e2e: guest' <<<"$OUT"))"
  until_ok 10 "A8: PC B's hjv-access-pc ok" pcb_is ok
  [ "$(console_count reg.ok)" -gt "$n0" ] || fail "A8: the console's reg.ok did not grow"
  until_ok 20 "A8: PC B online" online "$PCB_ID" ctl
  ascreen staff "$PCB_ID" a8
  screen_ok "A8: alice's controller to PC B" a8
  [ ! -e "$RUN/pcb/home/.config/hjviewer/HJViewer_ticket.json" ] || fail "A8: PC B keeps a ticket file"
  pass "A8: 'guest pc: ok'; PC B registered within 10 s (hjv-access-pc ok, reg.ok up), alice's controller got its screen; no ticket file in PC B's configuration"

  log "A9: no access password file: the right password gets the wrong one's answer"
  rm -f "$RUN/api/guest.conf"
  ascreen actl "$PC_ID" a9 HJV_E2E_ACCESS_PASSWORD="$GUEST_PW"
  [ "$RC" = 6 ] && ahas "hjv-e2e: guest ctl: denied" || fail "A9: exit $RC ($(grep '^hjv-e2e: guest' <<<"$OUT")), expected 6 and denied"
  [ "$MS" -ge 1500 ] || fail "A9: the refusal came after $MS ms, before hjv-api's failure delay"
  until_ok 5 "A9: admin ticket status counts 'closed 1'" guest_tried "closed 1"
  pass "A9: the file gone, the right password: exit 6 after $MS ms ('guest ctl: denied', as A7); admin ticket status: $(guest_tries)"

  log "A10: the App Review account reaches its demo PC, the PC, and nothing else"
  printf 'ID=%s\nHASH=%s\nUNTIL=%s\nPC=%s\n' "$REVIEW_ID" "$(pbkdf2_of "$REVIEW_PW")" \
    "$(date -u -d '+1 day' +%Y-%m-%dT%H:%M:%SZ)" "$PC_ID" >"$RUN/api/review.conf.new"
  mv -f "$RUN/api/review.conf.new" "$RUN/api/review.conf"
  arun hjv-ctl rev "$CLIENT" --hjv-e2e-login "$REVIEW_ID" "$REVIEW_PW"
  [ "$RC" = 0 ] || fail "A10: the App Review account's login: exit $RC ($(grep '^hjv-e2e:' <<<"$OUT" | tail -n 1))"
  arun hjv-ctl rev "$CLIENT" --hjv-e2e-ticket refresh
  ahas '"use":"ctl"' && ahas '"valid":true' || fail "A10: not a valid ticket for ctl: $(grep '^hjv-e2e: ticket' <<<"$OUT")"
  ascreen rev "$PC_ID" a10
  screen_ok "A10: the App Review account to its demo PC" a10
  ascreen rev "$PCB_ID" a10b
  refused_with "A10: the App Review account to PC B" a10b "$ACC_A2"
  pass "A10: the App Review account's ticket is for ctl; the PC's screen; PC B refused after one new ticket (exit 5, the app's A2)"

  support_code_part
  support_link_part

  log "A11: NOT_BEFORE=now ends every ticket handed out before: the registrations go, the logins get new ones"
  nb=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  n0=$(hbbs_count 'access closed reg ')
  sleep 1
  access_conf enforce "$nb"
  until_ok 13 "A11: hbbs's line 'access enforce: keys 1, not before $nb'" hbbs_has "access enforce: keys 1, not before $nb"
  until_ok 15 "A11: both PCs' registrations closed" hbbs_above 'access closed reg ' $((n0 + 1))
  # the PC registers again with its next sync's new ticket (a sync every 5 s in the E2E)
  until_ok 60 "A11: the PC online again" online "$PC_ID" ctl
  # alice's login ticket is from before: refused once, a new one, then in, in the same call
  n1=$(hbbs_count 'access refused ph ')
  ascreen staff "$PC_ID" a11
  screen_ok "A11: alice's controller after NOT_BEFORE" a11
  ms=$MS
  [ "$(hbbs_count 'access refused ph ')" = $((n1 + 1)) ] ||
    fail "A11: hbbs refused $(($(hbbs_count 'access refused ph ') - n1)) of alice's punch hole requests, not her old ticket's one"
  until_ok 30 "A11: PC B refused again" pcb_is refused
  pass "A11: hbbs closed both registrations ('access closed reg'); the PC came back with its next sync's ticket; alice's controller was refused once with her old ticket and got the screen with a new one in the same call ($ms ms); PC B stays refused until the access password comes again"

  log "A13: a broken file keeps the rule in force; hbbs started again with it refuses everything"
  before=$(hbbs_count "access file $RUN/access.conf: ")
  access_file "MODE=on" "KEY=1:$(cat "$KEYS/ticket.pub")" "NOT_BEFORE=$nb"
  until_ok 13 "A13: hbbs's ERROR line of the file" hbbs_above "access file $RUN/access.conf: " "$before"
  grep -F "access file $RUN/access.conf: " "$RUN/hbbs.log" | tail -n 1 | grep -q "access enforce: keys 1, not before $nb stays in force" ||
    fail "A13: not 'stays in force': $(grep -F "access file $RUN/access.conf: " "$RUN/hbbs.log" | tail -n 1)"
  ascreen staff "$PC_ID" a13
  screen_ok "A13: alice's controller while the broken file is there" a13
  n1=$(hbbs_count "; access closed: every request is refused")
  kill "$HBBS_PID" 2>/dev/null || true
  wait "$HBBS_PID" 2>/dev/null || true
  start_hbbs
  until_ok 15 "A13: hbbs's 'access closed' after its start" hbbs_above "; access closed: every request is refused" "$n1"
  until_ok 40 "A13: hbbs's console mode=closed" access_console_closed
  ascreen staff "$PC_ID" a13b
  refused_with "A13: alice's controller to a closed hbbs" a13b "$ACC_A2"
  until_ok 5 "A13: the console's ph.closed" console_has ph.closed
  until_ok 35 "A13: GET /api/hjv/access" gate_is closed
  pass "A13: the broken file left 'access enforce … stays in force' and alice connected; started again with it, hbbs refuses everything ('access closed'): exit 5 with the app's A2, ph.closed, GET /api/hjv/access {\"mode\":\"closed\"}"

  log "A14: MODE=off put right: hbbs takes it without a restart; PC B registers again"
  n0=$(hbbs_count "access off")
  access_conf off
  until_ok 13 "A14: hbbs's line 'access off'" hbbs_above "access off" "$n0"
  # refused, PC B sends RegisterPk again 20 s after its last one (1.9.0, hjviewer_access's
  # REFUSED_RETRY), before hbbs drops a connection quiet for 30 s (upstream sent every 30 s, and
  # waited a second round when that drop met the send): 20 s and the file's 10 s, with room
  t0=$(date +%s%N)
  until_ok 45 "A14: PC B's hjv-access-pc ok" pcb_is ok
  ms=$(ms_since "$t0")
  until_ok 40 "A14: PC B online" online "$PCB_ID" ctl
  ascreen actl "$PCB_ID" a14
  screen_ok "A14: the controller without a login, to PC B" a14
  until_ok 60 "A14: the PC online" online "$PC_ID" ctl
  pass "A14: 'access off' within 10 s, no restart; PC B registered again (hjv-access-pc ok $ms ms after hbbs's 'access off') and the controller without a login got its screen with the password"
  sudo ip netns pids hjv-pc2 | xargs -r sudo kill 2>/dev/null || true
}
# hbbs's console answers mode=closed
access_console_closed() { [[ $(access_console) == "HJVACCESS1 mode=closed"* ]]; }

# ---------------------------------------------------------------------------------------------
# 1.9.1's in-app account deletion (server/api/src/account.rs, POST /api/hjv/account/delete;
# docs/e2e.md, DEL1-DEL4), after the access part (MODE=off again since A14, PC B stopped). The
# deleting apps are the access part's sides in the controller's network: staff is alice's, rev the
# App Review account's. Their other devices are in hjv-ctl2's network (W6-E6's staff2, alice's, and
# rev2 here), because a login in the same network is the same device to hjv-api (one machine-id),
# which ends the earlier login's key there (that is why ctl's login ended at A2). DEL1-DEL3 need the client's --hjv-e2e-delete-account (client
# patch 0161), which sends the app's request as http_request does: signed by the account key
# (H13), its answer observed (H14). Every client call ends within 30 s or fails the run.
# ---------------------------------------------------------------------------------------------
oplog_count() { grep -cF -- "$1" "$RUN/api/operator.log" 2>/dev/null || true; } # <text>
api_log_count() { grep -cF -- "$1" "$RUN/hjv-api.log" || true; }               # <text>
allow_ids() { admin allow list | head -n 1 | sed -n 's/^allowlist: \([0-9]*\) IDs.*/\1/p'; }
codes_open() { admin ticket status | sed -n 's/^support codes: \([0-9]*\) open.*/\1/p'; }
codes_open_is() { [ "$(codes_open)" = "$1" ]; } # <n>
key_lines() { grep -c '^hjv-e2e: key ' <<<"$OUT" || true; } # of the last --hjv-e2e-keys
pc_count() { sed -n 's/^hjv-e2e: \([0-9]*\) device(s)$/\1/p' <<<"$OUT"; } # of the last --hjv-e2e-devices
# <scenario> <pcs> <devices>: the last --hjv-e2e-delete-account's first answer is a 200 with these
# counts and no relay, and its device logged out on it; DEL_FIRST and DEL_AGAIN are its answer lines
deleted_with() {
  DEL_FIRST=$(sed -n 's/^hjv-e2e: delete-account: //p' <<<"$OUT")
  DEL_AGAIN=$(sed -n 's/^hjv-e2e: delete-account again: //p' <<<"$OUT")
  [ "$RC" = 0 ] && [[ $DEL_FIRST == "HTTP 200 "* ]] || fail "$1: exit $RC, '$DEL_FIRST'"
  jq -e --argjson pcs "$2" --argjson devices "$3" \
    '. == {"deleted": true, "pcs": $pcs, "devices": $devices, "relays": 0}' <<<"${DEL_FIRST#HTTP 200 }" >/dev/null ||
    fail "$1: '$DEL_FIRST', expected {\"deleted\":true} with pcs $2, devices $3 and relays 0"
  ahas "hjv-e2e: after delete-account: access_token empty, user_info empty, hjv-my-devices empty" ||
    fail "$1: the device did not log out on the answer: $(grep '^hjv-e2e: after delete-account' <<<"$OUT")"
}
# <file> <what>: the page has neither the pilot notice's Korean word nor "pilot"
no_pilot() {
  if grep -qiE '시범|pilot' "$1"; then fail "$2: a pilot sentence: $(grep -oiE '.{0,30}(시범|pilot).{0,30}' "$1" | head -n 1)"; fi
}
door2_page() { [ "$(link_get "https://$DOMAIN/api/hjv/get/$1/")" = "200 " ]; } # <code>

delete_part() {
  local code link got pcs keys ids0 ops0 done0 log0 n0 door tag
  mkdir -p "$RUN"/rev2/{home,ipc}
  # a support code of alice's, whose page DEL4 reads and which DEL1 ends
  arun hjv-ctl staff "$CLIENT" --hjv-e2e-support-code
  [ "$RC" = 0 ] || fail "DEL: alice's support code: exit $RC ($(grep '^hjv-e2e: support' <<<"$OUT"))"
  code=$(sed -n 's/^hjv-e2e: support code: //p' <<<"$OUT")
  link=$(sed -n 's/^hjv-e2e: support link: //p' <<<"$OUT")
  [[ $code =~ $CODE_FORM ]] && [[ $link =~ $LINK_FORM ]] || fail "DEL: alice made '$code' and '$link'"
  got=$(link_get "$link")
  [ "$got" = "200 " ] && grep -qF -- "$code" "$RUN/w6.body" || fail "DEL: the code's support link answers '$got'"
  cp "$RUN/w6.body" "$RUN/del-support-page.html"

  if ! grep -aqF 'after delete-account' "$CLIENT"; then
    echo "skipped: DEL1-DEL3, the account's deletion from the app (no --hjv-e2e-delete-account in $CLIENT: client patch 0161)"
  else
    log "DEL1: alice deletes her account in the app (POST /api/hjv/account/delete, signed with her account key)"
    # before: the PC her login bound is active and lets her other device in without a password
    until_ok 15 "DEL1: the PC bound by alice's login (active)" state_is active
    arun hjv-ctl2 staff2 "$CLIENT" --hjv-e2e-devices
    [ "$RC" = 0 ] && ahas "hjv-e2e: device $PC_ID linux pwless=active" || fail "DEL1: staff2's device list (exit $RC) has the PC not active"
    arun hjv-ctl2 staff2 "$CLIENT" --hjv-e2e-screenshot "$PC_ID" "" "$RUN/del1a.ppm"
    [ "$RC" = 0 ] && screen_is_pc del1a || fail "DEL1: staff2's password-less connection before the deletion: exit $RC"
    n0=$(codes_open)
    [ "${n0:-0}" -ge 1 ] || fail "DEL1: admin ticket status has no support code open before the deletion"
    arun hjv-ctl staff "$CLIENT" --hjv-e2e-devices
    [ "$RC" = 0 ] && ahas "hjv-e2e: device $PC_ID " || fail "DEL1: alice's device list (exit $RC) has no PC $PC_ID"
    pcs=$(pc_count)
    arun hjv-ctl staff "$CLIENT" --hjv-e2e-keys
    keys=$(key_lines)
    [ "$RC" = 0 ] && [ "$keys" -ge 3 ] || fail "DEL1: alice's key list (exit $RC) has $keys keys, not at least staff's, staff2's and the PC's"
    ids0=$(allow_ids)
    ops0=$(oplog_count 'cmd="app account-delete" targets=')
    done0=$(oplog_count 'cmd="app account-delete" acct=')
    log0=$(api_log_count 'account: deleted at its own request (acct=')
    arun hjv-ctl staff "$CLIENT" --hjv-e2e-delete-account again
    deleted_with DEL1 "$pcs" "$keys"
    [ -z "$(local_option staff hjv-ctl-key)" ] || fail "DEL1: staff kept its account key"
    pass "DEL1: 200 ${DEL_FIRST#HTTP 200 } (alice's $pcs PCs and $keys keys, as her lists had them just before); staff logged out on the answer (token, user, device list, account key)"

    log "DEL2: the very same request again gets the first answer"
    [ -n "$DEL_AGAIN" ] && [ "$DEL_AGAIN" = "$DEL_FIRST" ] ||
      fail "DEL2: the same bytes again got '$DEL_AGAIN', not the first answer '$DEL_FIRST'"
    [ "$(oplog_count 'cmd="app account-delete" targets=')" = $((ops0 + 1)) ] &&
      [ "$(oplog_count 'cmd="app account-delete" acct=')" = $((done0 + 1)) ] ||
      fail "DEL2: the operator record has $(($(oplog_count 'cmd="app account-delete" targets=') - ops0)) first and $(($(oplog_count 'cmd="app account-delete" acct=') - done0)) result lines of the deletion, not one each"
    tag=$(grep -F 'cmd="app account-delete" targets=' "$RUN/api/operator.log" | tail -n 1 | sed -n 's/.* targets=\([^ ]*\) .*/\1/p')
    grep -F 'cmd="app account-delete" acct=' "$RUN/api/operator.log" | tail -n 1 | grep -qF "acct=$tag done=\"ok: " ||
      fail "DEL2: the result line is not 'acct=$tag done=\"ok: …\"': $(grep -F 'cmd="app account-delete" acct=' "$RUN/api/operator.log" | tail -n 1 | sed 's/ prev=.*//')"
    [ "$(api_log_count 'account: deleted at its own request (acct=')" = $((log0 + 1)) ] ||
      fail "DEL2: the journal has $(($(api_log_count 'account: deleted at its own request (acct=') - log0)) deletion lines, not one"
    pass "DEL2: the same bytes again (http_request_sync's second try) got the first answer although the token was gone; one deletion: the operator record's 'cmd=\"app account-delete\" targets=<tag>' and 'acct=<tag> done=\"ok: …\"', one journal line"

    log "DEL1: alice's other devices: 401 at their next account request"
    arun hjv-ctl2 staff2 "$CLIENT" --hjv-e2e-devices
    [ "$RC" = 1 ] && ahas 'devices failed: HTTP 401' || fail "DEL1: staff2 after the deletion: exit $RC ($(grep 'devices failed' <<<"$OUT"))"
    run pc --hjv-e2e-devices
    want 1 "DEL1: the PC's own device list after the deletion"
    has 'devices failed: HTTP 401' || fail "DEL1: the PC: not a 401 ($(grep 'devices failed' <<<"$OUT"))"
    pass "DEL1: alice's other devices, the controller staff2 and the PC, get 401 at their next account request (the app's UserModel.reset logs out on it, L4)"

    log "DEL1: the PC takes the signed revoked at its next sync and needs its password again"
    until_ok 15 "DEL1: the PC took the signed revoked" state_is revoked
    pc_unbound || fail "DEL1: the PC kept its binding"
    # staff2 still holds its key and device list (UserModel.reset is the app's): it sends a proof
    arun hjv-ctl2 staff2 "$CLIENT" --hjv-e2e-screenshot "$PC_ID" "" "$RUN/del1b.ppm"
    [ "$RC" = 3 ] && ahas 'password required Could not connect without a password' ||
      fail "DEL1: staff2's password-less connection after the deletion: exit $RC, not the refusal box (exit 3)"
    arun hjv-ctl2 staff2 "$CLIENT" --hjv-e2e-screenshot "$PC_ID" "$PASSWORD" "$RUN/del1c.ppm"
    [ "$RC" = 0 ] && screen_is_pc del1c || fail "DEL1: staff2 with the PC's password after the deletion: exit $RC"
    pass "DEL1: the PC dropped its binding on a signed revoked ($STATE); password-less refused (the refusal box), the PC's password lets the controller in"

    log "DEL1: alice's support codes end with her account; her allowlist entry goes"
    until_ok 5 "DEL1: admin ticket status: 0 support codes open" codes_open_is 0
    got=$(link_get "$link")
    [ "${got%% *}" = 404 ] || fail "DEL1: the ended code's support link answers '$got', not 404"
    [ "$(allow_ids)" = $((ids0 - 1)) ] || fail "DEL1: the allowlist has $(allow_ids) IDs, not $((ids0 - 1))"
    grep -F 'account: deleted at its own request (acct=' "$RUN/hjv-api.log" | tail -n 1 | grep -qiF "$ALICE" &&
      fail "DEL1: alice's portal ID in the journal's deletion line"
    pass "DEL1: admin ticket status 0 support codes open ($n0 before), the code's support link 404; the allowlist $ids0 -> $((ids0 - 1)) IDs; the journal's line has her tag, not her ID"

    log "DEL3: the App Review account deletes itself; its next login makes a new, empty account"
    arun hjv-ctl2 rev2 "$CLIENT" --hjv-e2e-login "$REVIEW_ID" "$REVIEW_PW"
    [ "$RC" = 0 ] || fail "DEL3: the App Review account's second login: exit $RC ($(grep '^hjv-e2e:' <<<"$OUT" | tail -n 1))"
    arun hjv-ctl rev "$CLIENT" --hjv-e2e-devices
    [ "$RC" = 0 ] || fail "DEL3: the App Review account's device list: exit $RC"
    pcs=$(pc_count)
    arun hjv-ctl rev "$CLIENT" --hjv-e2e-keys
    keys=$(key_lines)
    [ "$RC" = 0 ] && [ "$keys" -ge 2 ] || fail "DEL3: the App Review account's key list (exit $RC) has $keys keys, not rev's and rev2's"
    ids0=$(allow_ids)
    arun hjv-ctl rev "$CLIENT" --hjv-e2e-delete-account
    deleted_with DEL3 "$pcs" "$keys"
    grep -F 'account: deleted at its own request (acct=' "$RUN/hjv-api.log" | tail -n 1 | grep -qF ', the App Review account)' ||
      fail "DEL3: the journal's line does not name the App Review account"
    arun hjv-ctl2 rev2 "$CLIENT" --hjv-e2e-devices
    [ "$RC" = 1 ] && ahas 'devices failed: HTTP 401' || fail "DEL3: rev2 after the deletion: exit $RC ($(grep 'devices failed' <<<"$OUT"))"
    [ "$(allow_ids)" = "$ids0" ] || fail "DEL3: the allowlist has $(allow_ids) IDs, not $ids0: the App Review account's entry went"
    pass "DEL3: 200 ${DEL_FIRST#HTTP 200 } (its $keys keys); rev logged out, rev2 gets 401; its allowlist entry stays ($ids0 IDs)"
    arun hjv-ctl rev "$CLIENT" --hjv-e2e-login "$REVIEW_ID" "$REVIEW_PW"
    [ "$RC" = 0 ] || fail "DEL3: the next review login: exit $RC ($(grep '^hjv-e2e:' <<<"$OUT" | tail -n 1))"
    arun hjv-ctl rev "$CLIENT" --hjv-e2e-keys
    [ "$RC" = 0 ] && [ "$(key_lines)" = 1 ] && ahas 'current=true' ||
      fail "DEL3: the new account's key list (exit $RC): $(key_lines) keys, not this login's one"
    arun hjv-ctl rev "$CLIENT" --hjv-e2e-devices
    [ "$RC" = 0 ] && [ "$(pc_count)" = 0 ] || fail "DEL3: the new account's device list (exit $RC): $(pc_count) PCs"
    arun hjv-ctl rev "$CLIENT" --hjv-e2e-ticket refresh
    ahas '"use":"ctl"' && ahas '"valid":true' || fail "DEL3: the new account's ticket: $(grep '^hjv-e2e: ticket' <<<"$OUT")"
    pass "DEL3: the next review login made a new, empty account: one key (this login's), no PC, and a valid ticket for ctl (its demo PC)"
  fi

  log "DEL4: no pilot sentence on door 2's page and the support link page"
  door=$(random_binding)
  printf 'CODE=%s\nUNTIL=%s\n' "$door" "$(date -u -d '+1 day' +%Y-%m-%dT%H:%M:%SZ)" >"$RUN/api/door2.conf.new"
  mv -f "$RUN/api/door2.conf.new" "$RUN/api/door2.conf"
  until_ok 10 "DEL4: door 2's page (200)" door2_page "$door"
  grep -qF '<h1>HJViewer 새 버전 받기</h1>' "$RUN/w6.body" || fail "DEL4: not door 2's page: $(head -c 200 "$RUN/w6.body")"
  no_pilot "$RUN/w6.body" "DEL4: door 2's page"
  rm -f "$RUN/api/door2.conf"
  pass "DEL4: door 2's page (/api/hjv/get/<code>/, 200, '새 버전 받기') has neither '시범' nor 'pilot'"
  grep -qF '<h1>HJViewer 원격 지원</h1>' "$RUN/del-support-page.html" && grep -qF '<h1>HJViewer remote support</h1>' "$RUN/del-support-page.html" ||
    fail "DEL4: not the support link page: $(head -c 200 "$RUN/del-support-page.html")"
  no_pilot "$RUN/del-support-page.html" "DEL4: the support link page"
  pass "DEL4: the support link page (Korean and English, with its code) has neither '시범' nor 'pilot'"
}

# ---------------------------------------------------------------------------------------------
# The minimum version (its design 2-2, 2-4, 2-5): hbbs's lowest controller version (V1, V2, V4),
# and the signed policy refusing a controller below the minimum (V5-V7, V9). The ver side runs
# the current CLIENT in its own profile without an account login; the test sets a minimum above
# that build's version. Every client call ends within 30 s or fails the run.
# ---------------------------------------------------------------------------------------------
# the text an app with the update keys shows in place of a refusal for its version
R1="이 버전으로는 연결할 수 없다는 답을 받았습니다. 업데이트를 확인하고 있습니다. (version not allowed)"
# <version>: what the PC answers the login of a controller of that version (L1; the policy's
# minimum is NEXT_MINOR)
l1() {
  echo "이 PC는 HJViewer $NEXT_MINOR 이상에서만 연결을 받습니다. 이 기기의 HJViewer를 업데이트한 뒤 다시 연결하세요(지금 버전 $1). (version not allowed)"
}
# hbbs's text: the design's S1, with the test server and a made-up code of door 2
HBBS_TEXT="HJViewer를 %MIN% 이상으로 업데이트해야 연결할 수 있습니다. Play 스토어에서 설치했다면 Play 스토어에서, iPhone은 TestFlight 앱에서 업데이트하세요. 그 밖의 Windows·Android는 https://$DOMAIN/api/hjv/get/0123456789abcdef0123456789abcdef/ 에서 새 버전을 받으세요. (version not allowed)"

# <side> <client> <args...>: one call of a client in the controller's network as <side>: its exit
# code $RC, standard output $OUT, the hjv-e2e lines of its standard error $ERR and its time $MS
# (ms), all also in $RUN/<side>.out with the PC's password masked. A call that did not end within
# 30 s fails the run.
vrun() {
  local side=$1 client=$2 t0 a shown=()
  shift 2
  for a in "$@"; do
    if [ "$a" = "$PASSWORD" ]; then shown+=("***"); else shown+=("$a"); fi
  done
  mkdir -p "$RUN/$side/home" "$RUN/$side/ipc"
  t0=$(date +%s%N)
  RC=0
  OUT=$(in_ns hjv-ctl "$side" timeout -k 5 30 "$client" "$@" 2>"$RUN/$side.err") || RC=$?
  MS=$(ms_since "$t0")
  ERR=$(grep '^hjv-e2e:' "$RUN/$side.err" || true)
  {
    echo "== $(basename "$client") ${shown[*]} (exit $RC, $MS ms)"
    printf '%s\n' "$OUT"
    cat "$RUN/$side.err"
  } >>"$RUN/$side.out"
  case $RC in 124 | 137) fail "$side: '${shown[*]}' did not end within 30 s" ;; esac
}
# <side> <client> <tag>: the PC's remote screen with its password, into $RUN/<tag>.ppm
vscreen() { vrun "$1" "$2" --hjv-e2e-screenshot "$PC_ID" "$PASSWORD" "$RUN/$3.ppm"; }
hbbs_has() { grep -qF -- "$1" "$RUN/hbbs.log"; }
# <version>: hbbs's lines of the punch hole requests for the PC that it refused for that version
hbbs_refusals() { grep -cF -- "for \"$PC_ID\": version \"$1\"" "$RUN/hbbs.log" || true; }
hbbs_refused_more() { [ "$(hbbs_refusals "$1")" -gt "$2" ]; }
# the hjv-e2e lines of $OUT from the login error on
from_login_error() { sed -n '/^hjv-e2e: login error: /,$p' <<<"$OUT" | grep '^hjv-e2e:' || true; }
# <scenario> <epoch:seq>: the PC's log says that it took that policy; a policy that it refused, or
# a build that takes none, fails the run at once
policy_taken() {
  if pc_log_has 'version policy: off in this build' || pc_log_has 'version policy: refused ('; then
    fail "$1: $(pc_log | grep -o 'version policy: \(off in this build\|refused (\).*' | head -1)"
  fi
  pc_log_has "version policy: now at $2"
}

# V9: <epoch:seq> <reason> [<reason before>]: the PC refused the policy <epoch:seq> for <reason>
# (UnknownKey, its key; BadSignature, its signature). A PC that took it, or refused a policy for
# another reason than these (<reason before>: what it refused in V9 before), fails the run at once.
v9_refused() {
  local text other
  text=$(pc_log)
  if grep -q "version policy: now at $1" <<<"$text"; then fail "V9: the PC took the policy $1"; fi
  other=$(grep -o 'version policy: refused (.*' <<<"$text" | grep -vE "^version policy: refused \(($2|${3:-$2})\)" | head -1 || true)
  [ -z "$other" ] || fail "V9: the PC refused the policy $1, but not for $2: $other"
  grep -qF "version policy: refused ($2)" <<<"$text"
}

min_controller_part() {
  local conf="$RUN/min-controller.conf" from=2026-01-01T00:00:00Z text n0 t0 ms
  text=${HBBS_TEXT//%MIN%/$NEXT_PATCH}
  log "V1: hbbs takes MIN=$NEXT_PATCH from its file within its 10 s, without a restart"
  hbbs_has "min-controller off: no file $conf" ||
    fail "V1: hbbs did not look for $conf (HJV_MIN_CONTROLLER_FILE, server patch 0005)"
  t0=$(date +%s%N)
  min_conf "$NEXT_PATCH" "$from" "$HBBS_TEXT"
  until_ok 13 "V1: hbbs's line 'min-controller $NEXT_PATCH from $from'" hbbs_has "min-controller $NEXT_PATCH from $from"
  pass "V1: hbbs took MIN=$NEXT_PATCH, FROM=$from $(ms_since "$t0") ms after the file was replaced (it reads it every 10 s)"

  log "V2: a controller of $CUR is refused by hbbs$([ -n "$KEYED" ] && echo "; with the update keys it shows its own R1")"
  n0=$(hbbs_refusals "$CUR")
  vscreen ver "$CLIENT" v2
  [ "$RC" = 1 ] || fail "V2: exit $RC, expected 1 (no session)"
  if [ -n "$KEYED" ]; then
    [ "$ERR" = "hjv-e2e: $R1" ] || fail "V2: not R1 alone in place of hbbs's text: '$ERR'"
  else
    [ "$ERR" = "hjv-e2e: $text" ] || fail "V2: not hbbs's text: '$ERR'"
  fi
  if has 'connected over' || [ -e "$RUN/v2.ppm" ]; then fail "V2: a session started"; fi
  until_ok 5 "V2: hbbs's line of the refusal" hbbs_refused_more "$CUR" "$n0"
  # the log is written a moment later: a second request of the same attempt would be in by now
  sleep 2
  [ "$(hbbs_refusals "$CUR")" = $((n0 + 1)) ] ||
    fail "V2: hbbs refused $(($(hbbs_refusals "$CUR") - n0)) requests of one attempt"
  pass "V2: exit 1 after $MS ms with $([ -n "$KEYED" ] && echo "R1 (no link)" || echo "hbbs's text"), one refusal in hbbs's log (version \"$CUR\"); the text ends with '(version not allowed)', which keeps an app from connecting again by itself"

  log "V4: MIN emptied: the controller of $CUR gets through hbbs again within its 10 s"
  t0=$(date +%s%N)
  min_conf ""
  until_ok 13 "V4: hbbs's line 'min-controller off: MIN is empty in $conf'" hbbs_has "min-controller off: MIN is empty in $conf"
  ms=$(ms_since "$t0")
  n0=$(hbbs_refusals "$CUR")
  vscreen ver "$CLIENT" v4
  [ "$RC" = 0 ] || fail "V4: exit $RC ($(tail -n 1 <<<"$ERR"))"
  screen_is_pc v4 || fail "V4: the decoded screen is not the PC's desktop"
  [ "$(hbbs_refusals "$CUR")" = "$n0" ] || fail "V4: hbbs refused the controller again"
  pass "V4: hbbs dropped its minimum $ms ms after MIN was emptied; the controller got the PC's screen ($MS ms)"
}

policy_part() {
  local t0 cm0="" want deadline line n0 n1 after
  log "V5: the PC's service took the first policy by itself (it asks 30 s after its start)"
  until_ok 60 "V5: the PC's log line 'version policy: now at 1:1'" policy_taken V5 1:1
  pass "V5: the PC holds the policy 1:1 that it fetched from https://$DOMAIN/api/hjv/version and checked against the build's update keys"

  log "V6: before the deadline (tomorrow) the PC lets a controller below the minimum ($CUR < $NEXT_MINOR) in"
  vscreen ver "$CLIENT" v6
  [ "$RC" = 0 ] || fail "V6: exit $RC ($(grep '^hjv-e2e:' <<<"$OUT" | tail -n 1))"
  screen_is_pc v6 || fail "V6: the decoded screen is not the PC's desktop"
  if pc_log_has 'version policy: refused the login'; then fail "V6: the PC refused a login before the deadline"; fi
  pass "V6: the PC's screen ($MS ms): a deadline that has not passed lets the controller below the minimum in"

  log "V7: the minimum in force (seq 2, its deadline an hour ago): the PC takes it and refuses the login"
  deadline=$(date -u -d '-1 hour' +%Y-%m-%dT%H:%M:%SZ)
  serve_policy 2 "$deadline"
  # the service asks by itself 30 s after its start, then every six hours: it starts again
  stop_pc_service
  start_pc_service
  t0=$(date +%s)
  until_ok 75 "V7: the PC's log line 'version policy: now at 1:2'" policy_taken V7 1:2
  pass "V7: the PC's service took the policy 1:2 by itself, $(($(date +%s) - t0)) s after it started"
  until_ok 40 "V7: the PC online again" online "$PC_ID" ctl
  if [ -f "$RUN/pc-cm.log" ]; then cm0=$(wc -l <"$RUN/pc-cm.log"); fi
  vscreen ver "$CLIENT" v7
  want=$(printf '%s\n' "hjv-e2e: login error: $(l1 "$CUR")" "hjv-e2e: error: Login Error: $R1" "hjv-e2e: error Login Error")
  [ "$RC" = 4 ] || fail "V7: exit $RC, expected 4 (an 'error' box)"
  [ "$(from_login_error)" = "$want" ] || fail "V7: not the PC's L1, then R1 in a 'Login Error' box: $(from_login_error)"
  if has 'logged in to' || [ -e "$RUN/v7.ppm" ]; then fail "V7: the controller got in"; fi
  until_ok 5 "V7: the PC's log line of the refusal" \
    pc_log_has "version policy: refused the login of a controller (\"Linux\", version \"$CUR\")"
  if [ -n "$cm0" ] && [ "$(wc -l <"$RUN/pc-cm.log")" != "$cm0" ]; then
    fail "V7: the refused login reached the PC's connection manager"
  fi
  pass "V7: exit 4 after $MS ms: the PC's L1 ('… $NEXT_MINOR 이상 … (지금 버전 $CUR)'), then R1 in a 'Login Error' box, which no app reconnects from by itself$([ -n "$cm0" ] && echo "; the login never reached the PC's connection manager")"

  # (last: from here on the PC's log has policies that it refused, which policy_taken fails on)
  log "V9: a policy signed with the test key 3, which the build does not pin (seq 3, as 1:2): the PC's service starts again"
  # (with 1:2's deadline: a later one puts the deadline off, which even a PC that took any key
  # would refuse, as a relaxation)
  serve_policy 3 "$deadline" signing-3
  line="version policy: refused the login of a controller (\"Linux\", version \"$CUR\")"
  n0=$(pc_log_count "$line")
  n1=$(pc_log_count 'keep_alive: ')
  cm0=""
  if [ -f "$RUN/pc-cm.log" ]; then cm0=$(wc -l <"$RUN/pc-cm.log"); fi
  stop_pc_service
  t0=$(date +%s)
  start_pc_service
  until_ok 20 "V9: the PC registered with hbbs again" pc_log_count_above 'keep_alive: ' "$n1"
  vscreen ver "$CLIENT" v9
  after=$(($(date +%s) - t0))
  # The service asks the server 30 s after its start, or once it has refused a login: a login
  # refused sooner was refused by the policy it read from its store, before it asked anything.
  [ "$after" -lt 30 ] || fail "V9: the login ended $after s after the PC's start, when the PC may have asked the server already"
  want=$(printf '%s\n' "hjv-e2e: login error: $(l1 "$CUR")" "hjv-e2e: error: Login Error: $R1" "hjv-e2e: error Login Error")
  [ "$RC" = 4 ] || fail "V9: exit $RC, expected 4: the PC did not refuse by the policy 1:2 it stored"
  [ "$(from_login_error)" = "$want" ] || fail "V9: not the PC's L1, then R1 in a 'Login Error' box: $(from_login_error)"
  if has 'logged in to' || [ -e "$RUN/v9.ppm" ]; then fail "V9: the controller got in"; fi
  until_ok 5 "V9: the PC's log line of the refusal" pc_log_count_above "$line" "$n0"
  pass "V9: the PC's service, started again, refused the login by the policy 1:2 it stored, $after s after its start, before it asked the server (exit 4, the PC's L1, then R1)"
  # the refused login has the service ask at once
  until_ok 40 "V9: the PC's log line 'version policy: refused (UnknownKey)'" v9_refused 1:3 UnknownKey
  vscreen ver "$CLIENT" v9b
  if [ "$RC" != 4 ] || [ "$(from_login_error)" != "$want" ]; then
    fail "V9: after the policy of the test key 3, exit $RC, not 4 with the PC's L1, then R1: $(from_login_error)"
  fi
  if has 'logged in to' || [ -e "$RUN/v9b.ppm" ]; then fail "V9: the controller got in after the policy of the test key 3"; fi
  if [ -n "$cm0" ] && [ "$(wc -l <"$RUN/pc-cm.log")" != "$cm0" ]; then
    fail "V9: a refused login reached the PC's connection manager"
  fi
  pass "V9: the PC refused the policy 1:3 of the test key 3 (UnknownKey), took nothing from it and still refuses the controller by 1:2 ($MS ms)"

  log "V9: the policy 1:4 of the test key 1 with the signature of 1:2 in place of its own: the PC's service starts again"
  # A signature of a pinned key over another text, which only the signature itself gives away: as
  # 1:2 otherwise, so that a PC that does not check the signature takes it. A service that refused a
  # policy asks again only after half an hour, also for a refused login, and logs one refused policy
  # in that time: started again, it asks at its first refused login, and logs.
  serve_policy 4 "$deadline" "" 2
  n0=$(pc_log_count "$line")
  n1=$(pc_log_count 'keep_alive: ')
  stop_pc_service
  start_pc_service
  until_ok 20 "V9: the PC registered with hbbs again" pc_log_count_above 'keep_alive: ' "$n1"
  # refused by the 1:2 it stored, which the policy of the test key 3 left as it was
  vscreen ver "$CLIENT" v9c
  if [ "$RC" != 4 ] || [ "$(from_login_error)" != "$want" ]; then
    fail "V9: after a new start, exit $RC, not 4 with the PC's L1, then R1: $(from_login_error)"
  fi
  if has 'logged in to' || [ -e "$RUN/v9c.ppm" ]; then fail "V9: the controller got in after a new start"; fi
  until_ok 5 "V9: the PC's log line of the refusal" pc_log_count_above "$line" "$n0"
  until_ok 40 "V9: the PC's log line 'version policy: refused (BadSignature)'" v9_refused 1:4 BadSignature UnknownKey
  if [ -n "$cm0" ] && [ "$(wc -l <"$RUN/pc-cm.log")" != "$cm0" ]; then
    fail "V9: a refused login reached the PC's connection manager"
  fi
  pass "V9: the PC refused the policy 1:4 with the signature of 1:2 (BadSignature) and took nothing from it; started again, it refused the controller by the 1:2 it stored ($MS ms)"
}

if [ -n "$ACCOUNT" ]; then
  if [ "$MODE" = strict ]; then account_strict; else account_open; fi
fi
if [ -n "$ACCESS" ]; then
  access_part
  delete_part
fi
if [ -n "$MIN_HBBS" ]; then min_controller_part; fi
if [ -n "$POLICY" ]; then
  policy_part
elif [ -n "$KEYED" ]; then
  echo "skipped: V5-V7 and V9, the version policy (no HJV_RELEASE, tools/hjv-release to sign its test policies)"
fi

log "what hbbs saw"
# (|| true: with pipefail, head leaving early fails the pipeline once the log is long)
grep -E "update_pk|Punch hole|Tcp connection from 10\.77" "$RUN/hbbs.log" | sed 's/^\[[^]]*\] //' | head -8 || true
echo
echo "PASS ($MODE): $(grep -o 'transport: .*' "$RUN/ctl.log" | tail -1)$([ -n "$ACCOUNT" ] && echo ", same-account logins")$(
  [ -n "$ACCESS" ] && echo ", the staff-only gate")$([ -n "$MIN_HBBS" ] && echo ", hbbs's minimum")$(
  [ -n "$POLICY" ] && echo ", the version policy")"

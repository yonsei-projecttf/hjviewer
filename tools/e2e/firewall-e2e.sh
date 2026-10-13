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
# and a second PC (hjv-pc2), and the scenarios S0-S27 and X1-X8 in strict mode (S21 in open
# mode). It also needs:
#   HJV_API, HJV_FAKE_SSO  hjv-api and hjv-fake-sso (server/api, cargo build --features fake-sso)
#   HJV_E2E_KEYS  the E2E's keys (scripts/ci.sh e2e_keys: seal.secret, list-1.secret, log.secret,
#                 trust.pub), the client built with HJV_LOGIN_SEAL_PUBKEY and HJV_TRUST_PUBKEYS
#                 from them, and PC_CLIENT installed under /usr
#   CLIENT_OLD    optional: a 1.5.1 client built for this server (X2, X3; skipped without it)
#   FAKETIME_LIB  optional: libfaketime.so.1 (S27; skipped without it)
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

log() { printf '\n== %s\n' "$*"; }

# <-I|-D>: the host forwards UDP between hjv-pc and hjv-ctl (open mode)
forward_rules() {
  sudo iptables "$1" FORWARD -i veth-hjvpc -o veth-hjvctl -p udp -j ACCEPT
  sudo iptables "$1" FORWARD -i veth-hjvctl -o veth-hjvpc -p udp -j ACCEPT
}

cleanup() {
  set +e
  [ "$MODE" = open ] && forward_rules -D 2>/dev/null
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
# name (hjv-<side>, so the real one never shows up in output)
in_ns() {
  local ns=$1 side=$2
  shift 2
  sudo ip netns exec "$ns" unshare --uts bash -c '
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
# password-less switch is off as after an install, only the first two test accounts are on the
# allowlist, and the limits of successful logins and SSO calls fit the two dozen logins of one
# account in a few minutes
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
  "$HJV_FAKE_SSO" --listen "127.0.0.1:$SSO_PORT" >"$RUN/fake-sso.log" 2>&1 &
  SERVER_PIDS+=($!)
  start_api
  printf '2026000001\n2026000002\n' >"$RUN/api/allow.txt"
  admin allow import "$RUN/api/allow.txt"
fi

log "server: hbbs + hbbr behind Caddy (deploy/Caddyfile) on :443"
(cd "$RUN/srv" && exec env -i TEST_HBBS=no RUST_LOG=debug "$HBBS" -p "$HBBS_PORT" -r "$DOMAIN" >"$RUN/hbbs.log" 2>&1) &
SERVER_PIDS+=($!)
(cd "$RUN/srv" && exec env -i RUST_LOG=info "$HBBR" -p "$HBBR_PORT" -k _ >"$RUN/hbbr.log" 2>&1) &
SERVER_PIDS+=($!)
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
# the fake SSO's test accounts (server/api/src/fake_sso.rs); CAROL is not on the allowlist
ALICE=2026000001 ALICE_PW=hjv-test-pass-1 ALICE_NAME=홍길동
BOB=2026000002 BOB_PW=hjv-test-pass-2
CAROL=Y9001 CAROL_PW=hjv-test-pass-3
WRONG_PW=hjv-wrong-pass
SECRETS=() # the tokens and account keys the clients held, for S20
LAST_ASK=0 # when a proof last made the PC sync at once

pass() { echo "PASS: $*"; }
fail() {
  echo "FAIL: $*"
  exit 1
}
client_of() { case $1 in pc | pc2) echo "${SIDE_CLIENT[$1]:-$PC_CLIENT}" ;; *) echo "${SIDE_CLIENT[$1]:-$CLIENT}" ;; esac }
declare -A SIDE_CLIENT

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
  in_ns hjv-pc pc env DISPLAY=:99 "$PC_CLIENT" --server >>"$RUN/pc-server.log" 2>&1 &
}
online() { # <ID> <side>: hbbs has the device online, asked from the side's network
  in_ns "hjv-$2" "$2" "$PROBE" "$DOMAIN" --ca "$CA_ROOT" --peer "$1" >"$RUN/probe-$1.log" 2>&1 &&
    grep -q "$1=online" "$RUN/probe-$1.log"
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
  local bind_s19b t_key spid lpid b3 ts ft=()
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

  log "T0: a password connection (hjv-ctl2, not logged in)"
  run ctl2 --hjv-e2e-screenshot "$PC_ID" "$PASSWORD" "$RUN/t0.ppm"
  want 0 "T0: the password connection"
  screen_is_pc t0 || fail "T0: the decoded screen is not the PC's desktop"
  T0=$MS B=$((MS + 2000))
  pass "T0 = $T0 ms (B = T0 + 2 s = $B ms)"

  log "S4-S6: alice's controller without a password"
  n0=$(pc_logins)
  cm0=$(wc -l <"$RUN/pc-cm.log")
  pwless_ok ctl s4 "S4"
  pass "S4: no password box, the PC's screen ($MS ms; T0 $T0 ms)"
  n1=$(pc_logins)
  [ "$((n1 - n0))" = 1 ] || fail "S6: $((n1 - n0)) same-account login lines for one session"
  pc_log_has "hjv: same-account login (remote) by .* from $CTL_ID" ||
    fail "S6: no 'same-account login … from $CTL_ID' in the PC's log"
  pass "S6: $(pc_log | grep -o 'hjv: same-account login (remote) .*' | tail -1)"
  cm_since "$cm0" "cm login from $CTL_ID named \".* · same account(홍\*동)\", authorized true" ||
    fail "S6: the connection manager did not name the account's device ($(tail -n 1 "$RUN/pc-cm.log"))"
  pass "S6: the connection manager shows $(tail -n "+$((cm0 + 1))" "$RUN/pc-cm.log" | grep -o 'named ".*"' | head -1)"
  run ctl --hjv-e2e-stream "$PC_ID" "" 5
  want 0 "S5"
  has 'password required' && fail "S5: a password box"
  has 'hjv-e2e: [1-9][0-9]* frames' || fail "S5: no video frames"
  pass "S5: $(grep -o '[0-9]* frames.*' <<<"$OUT")"

  log "S7-S10: bob's controller (hjv-ctl2)"
  run ctl2 --hjv-e2e-login "$BOB" "$BOB_PW"
  want 0 "S7: bob's login"
  keep_secrets ctl2
  run ctl2 --hjv-e2e-devices
  want 0 "S7: bob's device list"
  has "device $PC_ID" && fail "S7: bob's list has alice's PC"
  pass "S7: bob's list has no PC ($(grep -o '[0-9]* device(s)' <<<"$OUT"))"
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
  cm_since "$cm0" "cm login from .* named \"${os_user^}\", authorized true" ||
    fail "S9: the PC was not given the OS user name ($(tail -n 1 "$RUN/pc-cm.log"))"
  cm_since "$cm0" "김연세\|$BOB" && fail "S9: the PC was given bob's name or portal ID"
  pass "S9: the PC's connection manager got the OS user name (${os_user^}), not bob's name or portal ID (H21)"
  run ctl2 --hjv-e2e-screenshot "$PC_ID" "$WRONG_PW" "$RUN/s10.ppm"
  want 3 "S10: a wrong password"
  has 'password required Wrong Password' || fail "S10: not the box to enter it again"
  pass "S10: a wrong password asks again (exit 3)"

  log "S22: port forwarding never goes by proof (U3)"
  forward_try ctl "" 9098
  want 3 "S22: port forwarding without a password"
  has 'password required Password Required' || fail "S22: not the password box"
  pass "S22: port forwarding without a password: the password box, no proof (exit 3)"

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
  pass "X1: approve-mode=click: 'No Password Access', the PC's user is asked"
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
  pass "S16: an altered cached list does not match its hash: the PC fetched it again"
  set_binding "$(random_binding)"
  pwless_refused ctl s16c "S16: a binding file with another binding"
  asked
  set_binding "$b0"
  pass "S16: a binding the server does not confirm opens nothing (exit 3)"
  until_ok 15 "S16: the PC back to its own binding" state_is active

  log "S12, S13, S13b: hjv-api stopped, then gone for longer than a list lives (30 s)"
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

  log "S14: hjv-ctl2 logs out and in as alice (a new key); its first connection right after"
  run ctl2 --hjv-e2e-logout
  gap
  run ctl2 --hjv-e2e-login "$ALICE" "$ALICE_PW"
  want 0 "S14: alice's login on hjv-ctl2"
  keep_secrets ctl2
  run ctl2 --hjv-e2e-devices
  pwless_ok ctl2 s14 "S14: the new key's first connection"
  asked
  pass "S14: the new key's first connection gets in ($MS ms): the PC fetched its list for the key's serial"

  log "S15: the PC's service starts again; its user is logged out (the token gone)"
  stop_pc_service
  run pc --hjv-e2e-local-option access_token ""
  n0=$(pc_log_count 'hjv: same-account logins: sync task started')
  n1=$(pc_log_count 'keep_alive: ')
  start_pc_service
  until_ok 20 "S15: the PC's sync task started again" pc_log_count_above 'hjv: same-account logins: sync task started' "$n0"
  until_ok 10 "S15: the PC's first sync after the start" first_sync_done
  until_ok 20 "S15: the PC registered with hbbs again" pc_log_count_above 'keep_alive: ' "$n1"
  pwless_ok ctl s15 "S15: after the service started again"
  pass "S15: password-less right after the restart, with the user logged out (binding + cache)"
  first_sync_ms=$(first_sync_ms)
  [ -n "$first_sync_ms" ] || fail "S15: no first sync in the PC's log"
  [ "$first_sync_ms" -le 1500 ] || fail "S15: the first sync took $first_sync_ms ms (more than 1.5 s)"
  pass "S15: the first sync after the start took $first_sync_ms ms (<= 1.5 s)"

  log "S17: hjv-ctl2 (alice) logs out the controller's key"
  run ctl2 --hjv-e2e-keys
  ckid=$(sed -n 's/^hjv-e2e: key \([0-9]*\) current=false app_name=hjv-ctl .*/\1/p' <<<"$OUT")
  [ -n "$ckid" ] || fail "S17: the controller's key is not in the list"
  run ctl2 --hjv-e2e-revoke-key "$ckid"
  want 0 "S17: revoking the controller's key"
  t0=$(date +%s%N)
  until_ok 15 "S17: the revoked key refused" try_rc ctl s17 3
  has 'password required Could not connect without a password' || fail "S17: not the refusal box"
  pass "S17: the revoked key is refused $(ms_since "$t0") ms after the revoke (refresh 5 s)"

  log "S18: hjv-ctl2 takes the PC out of the list"
  run ctl2 --hjv-e2e-remove "$PC_ID"
  want 0 "S18: removing the PC"
  until_ok 15 "S18: the PC took the signed revoked" state_is revoked
  [ -z "$(binding Binding)" ] || fail "S18: the PC kept its binding"
  pwless_refused ctl2 s18 "S18: password-less to a removed PC"
  pass "S18: the PC dropped its binding on a signed revoked ($STATE); refused"

  log "S19: the PC's user logs out"
  pc_bound
  pwless_ok ctl2 s19a "S19: the PC bound again"
  run pc --hjv-e2e-logout
  sleep 1
  pwless_refused ctl2 s19b "S19: password-less 1 s after the PC's logout"
  [ -z "$(binding Binding)" ] || fail "S19: the PC kept its binding"
  admin --reveal binding list >"$RUN/bindings.txt"
  grep -q " $PC_ID " "$RUN/bindings.txt" && fail "S19: hjv-api kept the PC's binding after its logout"
  pass "S19: refused 1 s after the logout; the PC dropped its binding, and hjv-api released it"

  log "S19b: the PC's user logs out while hjv-api is gone"
  pc_bound
  bind_s19b=$BIND_REQ
  pwless_ok ctl2 s19c "S19b: the PC bound again"
  kill "$API_PID"
  wait "$API_PID" 2>/dev/null || true
  run pc --hjv-e2e-logout
  sleep 1
  pwless_refused ctl2 s19d "S19b: password-less 1 s after an offline logout"
  pass "S19b: refused 1 s after the logout, with hjv-api gone"
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
  in_ns hjv-pc pc "$PC_CLIENT" --hjv-e2e-login "$ALICE" "$ALICE_PW" >"$RUN/r3-login.out" 2>>"$RUN/pc.err" &
  lpid=$!
  sleep 1.5
  kill -STOP "$API_PID"
  sudo kill -CONT $spid
  sleep 1
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

  log "S25: [끄기] on the PC, the option written again, then a login on the PC"
  pc_bound
  pwless_ok ctl2 s25a "S25: bound"
  run pc --hjv-e2e-option hjv-pwless-off Y
  sleep 1
  pwless_refused ctl2 s25b "S25: 1 s after [끄기]"
  [ -n "$(binding Off)" ] || fail "S25: the PC did not keep Off"
  run pc --hjv-e2e-option hjv-pwless-off N
  sleep 2
  pwless_refused ctl2 s25c "S25: with Config2's request written again"
  pc_bound
  pwless_ok ctl2 s25d "S25: after a login on the PC"
  pass "S25: off within 1 s, kept off when Config2 is written again (Off), on again only by a login on the PC"

  log "S26: the administrator releases the PC (binding revoke, then device remove)"
  ts=$(date +%s.%N)
  admin binding revoke "$PC_ID" | sed 's/^/    /'
  until_ok 15 "S26: the PC took the signed revoked" state_is revoked
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
  pwless_refused ctl2 s26c "S26: after device remove"
  pass "S26: device remove (the alias) does the same"

  log "X6: the switch off for everyone"
  pc_bound
  pwless_ok ctl2 x6a "X6: bound"
  admin passwordless off | sed 's/^/    /'
  t0=$(date +%s%N)
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
  has "error Security warning: this PC's key has changed" || fail "X7: not the key warning"
  forward_try ctl "$PASSWORD" 9097
  want 4 "X7: port forwarding to the PC with a new key"
  has "error Security warning: this PC's key has changed" || fail "X7: port forwarding: not the key warning"
  [ "$(wc -l <"$RUN/pc-cm.log")" = "$cm0" ] || fail "X7: a login reached the PC"
  pass "X7: the key warning (exit 4) for the remote screen and port forwarding; neither a proof nor the password went out"

  # The PC's first sync with its new key came before hbbs had that key, so hjv-api found nothing to
  # mark; it asks hbbs again at the PC's first sync a minute later (KEY_CHECK_EVERY). Meanwhile:
  log "X2, X3: a 1.5.1 PC and a 1.5.1 controller (X2 to S27 run while hjv-api waits to ask hbbs again)"
  if [ -n "${CLIENT_OLD:-}" ]; then
    SIDE_CLIENT[pc2]=$CLIENT_OLD
    run pc2 --hjv-e2e-set-password "$PASSWORD"
    OLD_ID=$(tail -1 <<<"$OUT")
    in_ns hjv-pc2 pc2 bash -c 'Xvfb :98 -screen 0 1280x720x24 -nolisten tcp >/dev/null 2>&1 &
      for _ in $(seq 50); do [ -e /tmp/.X11-unix/X98 ] && break; sleep 0.1; done
      DISPLAY=:98 exec "$1" --server' _ "$CLIENT_OLD" >>"$RUN/pc2-server.log" 2>&1 &
    until_ok 40 "X2: the 1.5.1 PC online" online "$OLD_ID" ctl
    pwless ctl x2 "$OLD_ID"
    want 3 "X2: alice to a 1.5.1 PC"
    has 'password required Password Required' || fail "X2: not the password box"
    [ "$MS" -le "$B" ] || fail "X2: the password box after $MS ms (B = $B ms)"
    pass "X2: a 1.5.1 PC sends no nonce: the password box at once ($MS ms <= B $B ms)"
    run pc2 --hjv-e2e-screenshot "$PC_ID" "$PASSWORD" "$RUN/x3.ppm"
    has 'written to' || fail "X3: the 1.5.1 controller got no screen"
    screen_is_pc x3 || fail "X3: the decoded screen is not the PC's desktop"
    pass "X3: a 1.5.1 controller gets in with the PC's password"
    sudo ip netns pids hjv-pc2 | xargs -r sudo kill 2>/dev/null || true
    unset 'SIDE_CLIENT[pc2]'
  else
    echo "skipped: X2, X3 (no CLIENT_OLD, a 1.5.1 client built for this server)"
  fi

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

  log "S11: a portal ID off the allowlist fails as a wrong password does, without an SSO call"
  calls=$(sso_calls)
  run ctl2 --hjv-e2e-login "$BOB" "$WRONG_PW"
  want 1 "S11: bob's wrong password"
  e_bob=$(sed -n 's/^hjv-e2e: login failed: //p' <<<"$OUT")
  [ "$(sso_calls)" = $((calls + 1)) ] || fail "S11: a wrong password did not reach the fake SSO once"
  calls=$(sso_calls)
  run ctl2 --hjv-e2e-login "$CAROL" "$CAROL_PW"
  want 1 "S11: a portal ID off the allowlist"
  e_carol=$(sed -n 's/^hjv-e2e: login failed: //p' <<<"$OUT")
  [ "$e_carol" = "$e_bob" ] || fail "S11: '$e_carol' is not the wrong password's '$e_bob'"
  [ "$(sso_calls)" = "$calls" ] || fail "S11: the fake SSO was called for an ID off the allowlist"
  pass "S11: the same failure ($e_carol), no SSO call"
  # the failed logins leave the login of hjv-ctl2 (alice's) as it was
  run ctl2 --hjv-e2e-devices
  want 0 "S11: the login of hjv-ctl2 after the failed ones"

  log "D1: the device list answered after a logout keeps nothing"
  kill -STOP "$API_PID"
  (sleep 1.5 && kill -CONT "$API_PID") &
  lpid=$!
  run ctl2 --hjv-e2e-logout --during-devices
  wait "$lpid"
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

  log "X4: a second 1.6.0 PC (alice) connects to the PC"
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
  pwless_ok pc2 x4 "X4: PC to PC"
  asked
  pass "X4: the second PC (alice) connects to the PC without a password"

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

# the device list of hjv-ctl (hjv-ctl2) has the PC in state <state>
ctl_lists() {
  run ctl --hjv-e2e-devices >/dev/null
  has "device $PC_ID linux pwless=$1"
}
ctl2_lists() {
  run ctl2 --hjv-e2e-devices >/dev/null
  has "device $PC_ID linux pwless=$1"
}

if [ -n "$ACCOUNT" ]; then
  if [ "$MODE" = strict ]; then account_strict; else account_open; fi
fi

log "what hbbs saw"
# (|| true: with pipefail, head leaving early fails the pipeline once the log is long)
grep -E "update_pk|Punch hole|Tcp connection from 10\.77" "$RUN/hbbs.log" | sed 's/^\[[^]]*\] //' | head -8 || true
echo
echo "PASS ($MODE): $(grep -o 'transport: .*' "$RUN/ctl.log" | tail -1)$([ -n "$ACCOUNT" ] && echo ", same-account logins")"

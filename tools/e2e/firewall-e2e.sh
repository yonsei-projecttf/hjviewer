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
#
# The minimum version (its design 2-2, 2-4; docs/e2e.md), in strict mode after the parts above:
# HJV_E2E_MIN_CONTROLLER=1 gives hbbs (server patch 0005) the file $RUN/min-controller.conf, which
# V1-V4 write as an operator would: a controller below MIN is refused with the file's text and
# connects again once MIN is emptied. HJV_E2E_UPDATE_KEYS=1 says that CLIENT and PC_CLIENT were
# built with the update test keys (hjv-release test-keys --env): such an app shows its own text R1
# for a refusal for its version, and with HJV_RELEASE, tools/hjv-release, which signs the test
# policies (test-sign, the test key 1), Caddy serves the policy at /api/hjv/version (a site block of
# that path in the run's Caddyfile; the public deploy/Caddyfile serves none). The PC's service
# takes it by itself; V5-V8 check that it lets a controller below the minimum in before the
# deadline and refuses its login once the deadline has passed. V9 serves a policy signed with the
# test key 3, which no E2E build pins, and starts the PC's service again: the PC refuses the login
# by the policy it stored, refuses the new one and takes nothing from it. It does the same with a
# policy of the test key 1 that carries the signature of another one, which only the signature
# itself gives away. CLIENT_OLD adds the 1.5.1 controller to V3 and V8.
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

log() { printf '\n== %s\n' "$*"; }

# <seq> <deadline> [<test key> [<seq of an earlier policy>]]: signs a policy of epoch 1 and that seq
# with the test key 1 (hjv-release test-sign), or with <test key> as it is (--raw, as the tool signs
# for a key that no E2E build pins), keeps the envelope as envelope-<seq>.json and serves it at
# https://hjv.test/api/hjv/version, replaced at once as the server's file is. With <seq of an
# earlier policy> of the same key, the envelope served carries that policy's signature in place of
# its own. Its one entry, windows, is the one an E2E build on Linux is held to: NEXT_MINOR, until
# <deadline> only for apps below it (grace_until). Its package is made up: Linux downloads nothing.
serve_policy() {
  local text="$RUN/policy/policy-$1.json" signed="$RUN/policy/envelope-$1.json" key=()
  [ -z "${3:-}" ] || key=(--key "$3" --raw)
  printf '{"v":1,"epoch":1,"seq":%s,"issued":"%s","server":"%s","platforms":{"windows":{"min":"%s","latest":"%s","grace_until":"%s","package":{"file":"HJViewer-%s-windows-x86_64.exe","size":1,"sha256":"%s","build":"%s.0"}}}}\n' \
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

# <-I|-D>: the host forwards UDP between hjv-pc and hjv-ctl (open mode)
forward_rules() {
  sudo iptables "$1" FORWARD -i veth-hjvpc -o veth-hjvctl -p udp -j ACCEPT
  sudo iptables "$1" FORWARD -i veth-hjvctl -o veth-hjvpc -p udp -j ACCEPT
}

cleanup() {
  set +e
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
# (with the minimum version, hbbs reads the lowest controller version from a file, while there is one)
(cd "$RUN/srv" && exec env -i TEST_HBBS=no RUST_LOG=debug ${MIN_HBBS:+"HJV_MIN_CONTROLLER_FILE=$RUN/min-controller.conf"} \
  "$HBBS" -p "$HBBS_PORT" -r "$DOMAIN" >"$RUN/hbbs.log" 2>&1) &
SERVER_PIDS+=($!)
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
# the fake SSO's test accounts (server/api/src/fake_sso.rs); CAROL is not imported: her first login
# puts her on the allowlist (S28)
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
  local bind_s19b t_key spid lpid b3 r3_before s25_ms ts ft=()
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
  pass "S16: after the cached list was altered the PC lets in and keeps a list that matches its hash (fetched again for the mismatch, or by a periodic sync; the unit tests pin which)"
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
  pass "S14: the new key's first connection gets in ($MS ms): the PC's list had the key (its sync for the key's serial, or a periodic one; the unit tests pin which)"

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
  within_ms 1500 pc_unbound || fail "S19: the PC still held its binding 1.5 s after the logout"
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

  log "S25: [끄기] on the PC, the option written again, then a login on the PC"
  pc_bound
  pwless_ok ctl2 s25a "S25: bound"
  run pc --hjv-e2e-option hjv-pwless-off Y
  within_ms 1500 pc_off || fail "S25: the PC did not keep Off within 1.5 s of [끄기]"
  s25_ms=$WITHIN_MS
  pwless_refused ctl2 s25b "S25: after [끄기]"
  run pc --hjv-e2e-option hjv-pwless-off N
  sleep 2
  pwless_refused ctl2 s25c "S25: with Config2's request written again"
  pc_bound
  pwless_ok ctl2 s25d "S25: after a login on the PC"
  pass "S25: off $s25_ms ms after [끄기], kept off when Config2 is written again (Off), on again only by a login on the PC"

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
# an SSO call. On hjv-ctl2, free since D1; hjv-api starts again for list mode, after X7b.
account_allowlist() {
  local calls e_wrong blocked_text="이 계정은 관리자가 HJViewer 사용을 막았습니다. 다시 쓰려면 관리자에게 문의하세요."
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
# The minimum version (its design 2-2, 2-4, 2-5): hbbs's lowest controller version (V1-V4), and the
# signed policy by which the PC refuses the login of an old controller (V5-V9). The controllers are
# sides of their own in the controller's network that never logged in: ver runs CLIENT, old the
# 1.5.1 client. Every call of a client ends within 30 s or fails the run.
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
version_of() { env -i HOME="$RUN/version" PATH=/usr/bin:/bin timeout 10 "$1" --version; } # <client>
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
  local conf="$RUN/min-controller.conf" from=2026-01-01T00:00:00Z text n0 t0 ms old_version
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

  log "V3: a 1.5.1 controller shows hbbs's text as it came, with its link"
  if [ -n "${CLIENT_OLD:-}" ]; then
    old_version=$(version_of "$CLIENT_OLD") || fail "V3: $CLIENT_OLD --version"
    n0=$(hbbs_refusals "$old_version")
    vscreen old "$CLIENT_OLD" v3
    # (the 1.5.1 client's headless commands end with 0 whatever happened)
    [ "$RC" = 0 ] || fail "V3: exit $RC, expected 0"
    [ "$ERR" = "hjv-e2e: $text" ] || fail "V3: not hbbs's text with MIN filled in: '$ERR'"
    if has 'connected over' || [ -e "$RUN/v3.ppm" ]; then fail "V3: a session started"; fi
    until_ok 5 "V3: hbbs's line of the refusal" hbbs_refused_more "$old_version" "$n0"
    pass "V3: the 1.5.1 controller ($old_version) shows hbbs's text with MIN filled in and the link of door 2 ($MS ms)"
  else
    echo "skipped: V3 (no CLIENT_OLD, a 1.5.1 client built for this server)"
  fi

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
  local t0 cm0="" old_version want deadline line n0 n1 after
  log "V5: the PC's service took the first policy by itself (it asks 30 s after its start)"
  until_ok 60 "V5: the PC's log line 'version policy: now at 1:1'" policy_taken V5 1:1
  pass "V5: the PC holds the policy 1:1 that it fetched from https://$DOMAIN/api/hjv/version and checked against the build's update keys"

  log "V6: before the deadline (tomorrow) the PC lets a controller below the minimum ($CUR < $NEXT_MINOR) in"
  vscreen ver "$CLIENT" v6
  [ "$RC" = 0 ] || fail "V6: exit $RC ($(grep '^hjv-e2e:' <<<"$OUT" | tail -n 1))"
  screen_is_pc v6 || fail "V6: the decoded screen is not the PC's desktop"
  if pc_log_has 'version policy: refused the login'; then fail "V6: the PC refused a login before the deadline"; fi
  pass "V6: the PC's screen ($MS ms): a deadline that has not passed lets the old controller in"

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

  log "V8: a 1.5.1 controller shows the PC's L1 as it came"
  if [ -n "${CLIENT_OLD:-}" ]; then
    old_version=$(version_of "$CLIENT_OLD") || fail "V8: $CLIENT_OLD --version"
    vscreen old "$CLIENT_OLD" v8
    want=$(printf '%s\n' "hjv-e2e: login error: $(l1 "$old_version")" "hjv-e2e: error: Login Error: $(l1 "$old_version")")
    [ "$RC" = 0 ] || fail "V8: exit $RC, expected 0"
    [ "$(from_login_error)" = "$want" ] || fail "V8: not the PC's L1 in a 'Login Error' box: $(from_login_error)"
    if has 'logged in to' || [ -e "$RUN/v8.ppm" ]; then fail "V8: the controller got in"; fi
    pass "V8: the 1.5.1 controller ($old_version) shows the PC's L1 in its 'Login Error' box ($MS ms)"
  else
    echo "skipped: V8 (no CLIENT_OLD, a 1.5.1 client built for this server)"
  fi

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
if [ -n "$MIN_HBBS" ]; then min_controller_part; fi
if [ -n "$POLICY" ]; then
  policy_part
elif [ -n "$KEYED" ]; then
  echo "skipped: V5-V9, the version policy (no HJV_RELEASE, tools/hjv-release to sign its test policies)"
fi

log "what hbbs saw"
# (|| true: with pipefail, head leaving early fails the pipeline once the log is long)
grep -E "update_pk|Punch hole|Tcp connection from 10\.77" "$RUN/hbbs.log" | sed 's/^\[[^]]*\] //' | head -8 || true
echo
echo "PASS ($MODE): $(grep -o 'transport: .*' "$RUN/ctl.log" | tail -1)$([ -n "$ACCOUNT" ] && echo ", same-account logins")$(
  [ -n "$MIN_HBBS" ] && echo ", hbbs's minimum")$([ -n "$POLICY" ] && echo ", the version policy")"

#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# End-to-end check of waking a PC that sleeps (design v3 8-10, docs/e2e.md), on one Linux host and
# without sudo: everything runs in user, network, mount and PID namespaces of this user, and is
# gone when the script ends.
#
#   the internet 203.0.113.0/24   hjv.test (.10): Caddy on 443 -> hbbs, hbbr, hjv-api (-> the fake
#                                 Yonsei SSO); the phones of alice (.50) and bob (.51)
#   LAN A 192.0.2.0/24            the PC (.67, MAC 00:00:5e:00:53:67) and the NAS's first card (.20)
#   LAN B 198.51.100.0/24         the NAS's second card (.20), which has the NAS's default route,
#                                 and another host (.30)
#
# The namespace the script runs in is the switch of each network and the router between them; the
# PC, the NAS, the other host and the phones each have a network namespace. The PC's client runs
# as an installed one: Linux counts a program under /usr as installed, and hjv-api adds only
# installed PCs to an account, so the PC's commands see a folder of the test in place of
# /usr/local/lib (a bind mount in their own mount namespace; the host's /usr stays as it is).
#
# alice logs in on the PC, which binds it, and the PC reports how to wake it. hjv-relay runs on the
# NAS and waits for its approval. The PC is shut down: its service ends and its card loses its
# address, and a packet socket on the card sees what reaches it. The administrator approves the
# relay's code for the owner of that PC. alice's phone asks hjv-api to wake the PC; the relay gets
# the signed command and the card gets 12 frames, to 255.255.255.255 and LAN A's broadcast address,
# ports 9 and 7, three times, while the other LAN gets none. The job is `sent`; the PC wakes
# (address back, service started) and the job is `confirmed`. Around that:
# - the wake API's other answers: a PC that is on, another account, an account without a relay, a
#   second request, a request too soon after a job, a relay that is switched off, a revoked relay;
# - a second relay on the NAS before a server that stands in hjv-api's place (the impostor of
#   tools/e2e/wol-helper.py) and hands it commands hjv-api never signs: each is refused with its
#   reason and puts no frame on the wire, and of eleven good commands in a minute ten are sent;
# - a second wake, of the PC in standby (its service frozen, not ended) through a NAS whose card
#   has a narrower netmask than the PC's: the relay uses its own broadcast address, and the PC's
#   service, running on, sees its clock jump and tells hjv-api at once.
#
#   tools/e2e/wol-e2e.sh
#
# Needs unprivileged user namespaces (Ubuntu 24.04 restricts them through AppArmor: sysctl
# kernel.apparmor_restrict_unprivileged_userns=0), unshare, ip, python3, curl, and, none of them
# under /run (the test has its own inside its namespaces):
#   HJV_E2E_WORK  scratch directory; $HJV_E2E_WORK/srv/id_ed25519(.pub) is the server key
#   CADDY         caddy binary
#   HBBS, HBBR, PROBE  the patched hbbs and hbbr (server/patches) and hjv-probe
#   CLIENT        a client with the account code and the WOL report, built with the test feature
#                 for this server and the E2E's keys (HJV_SERVER=hjv.test, HJV_SERVER_KEY,
#                 HJV_LOGIN_SEAL_PUBKEY, HJV_TRUST_PUBKEYS, --features linux-pkg-config,hjv-e2e)
#   HJV_API, HJV_FAKE_SSO  hjv-api and hjv-fake-sso (server/api, --features fake-sso)
#   HJV_RELAY     hjv-relay built for this server with the E2E's command key
#                 (HJV_SERVER=hjv.test HJV_COMMAND_PUBKEY=$(cat $HJV_E2E_KEYS/command.pub))
#   HJV_E2E_KEYS  the E2E's keys (scripts/ci.sh e2e_keys)
# scripts/ci.sh wol builds all of them and runs this.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
# (as absolute paths: the servers start in folders of their own)
WORK=$(realpath -m -- "${HJV_E2E_WORK:?set HJV_E2E_WORK to a scratch directory}")
CADDY=$(realpath -m -- "${CADDY:?set CADDY to a caddy binary}")
HBBS=$(realpath -m -- "${HBBS:-$ROOT/upstream/rustdesk-server/target/release/hbbs}")
HBBR=$(realpath -m -- "${HBBR:-$ROOT/upstream/rustdesk-server/target/release/hbbr}")
CLIENT=$(realpath -m -- "${CLIENT:-$ROOT/upstream/rustdesk/target/release/rustdesk}")
PROBE=$(realpath -m -- "${PROBE:-$ROOT/tools/hjv-probe/target/release/hjv-probe}")
HJV_API=$(realpath -m -- "${HJV_API:?set HJV_API to hjv-api}")
HJV_FAKE_SSO=$(realpath -m -- "${HJV_FAKE_SSO:?set HJV_FAKE_SSO to hjv-fake-sso}")
HJV_RELAY=$(realpath -m -- "${HJV_RELAY:?set HJV_RELAY to hjv-relay built for hjv.test}")
KEYS=$(realpath -m -- "${HJV_E2E_KEYS:?set HJV_E2E_KEYS to the E2E keys}")
HELPER="$ROOT/tools/e2e/wol-helper.py"
CAPTURE="$ROOT/tools/e2e/capture.py"
RUN="$WORK/run-wol"

if [ "${1:-}" != --inside ]; then
  [ $# -eq 0 ] || { echo "usage: $0 (no arguments; its header says what it needs)" >&2; exit 2; }
  # the clients and the relay run as the user who starts this, and behave otherwise as root
  [ "$(id -u)" != 0 ] || { echo "FAIL: run it as a normal user, not as root (it needs no privileges)" >&2; exit 1; }
  for tool in unshare ip python3 curl timeout; do
    command -v "$tool" >/dev/null || { echo "FAIL: $tool is not installed" >&2; exit 1; }
  done
  for program in "$CADDY" "$HBBS" "$HBBR" "$CLIENT" "$PROBE" "$HJV_API" "$HJV_FAKE_SSO" "$HJV_RELAY"; do
    [ -x "$program" ] || { echo "FAIL: $program is not there (the header of $0 says what it needs)" >&2; exit 1; }
  done
  [ -s "$WORK/srv/id_ed25519.pub" ] && [ -s "$KEYS/command-1.secret" ] ||
    { echo "FAIL: no server key in $WORK/srv, or no keys in $KEYS" >&2; exit 1; }
  # inside, /run is the test's own (ip keeps its network namespaces there): what is there now is hidden
  for path in "$WORK" "$KEYS" "$CADDY" "$HBBS" "$HBBR" "$CLIENT" "$PROBE" "$HJV_API" "$HJV_FAKE_SSO" "$HJV_RELAY" "$ROOT"; do
    case "$path" in
      /run | /run/*) echo "FAIL: $path is under /run, which the test replaces inside its namespaces: use another place" >&2; exit 1 ;;
    esac
  done
  NAMESPACES=(unshare --user --map-root-user --net --mount --pid --fork --mount-proc --kill-child)
  if ! "${NAMESPACES[@]}" true 2>/dev/null; then
    echo "FAIL: this user cannot make user, network, mount and PID namespaces" >&2
    echo "(Ubuntu 24.04: sudo sysctl kernel.apparmor_restrict_unprivileged_userns=0)" >&2
    exit 1
  fi
  rm -rf "$RUN"
  mkdir -p "$RUN"
  # in there this user is root of its own namespaces, and the script is their first process: when
  # it ends, or is ended, every process it started goes with it. A run that hangs is ended after
  # ten minutes (it takes two), with SIGKILL: unshare waits for its child with the others blocked.
  exec timeout --foreground --signal=KILL 600 "${NAMESPACES[@]}" "$0" --inside "$(id -u)" "$(id -g)" "$(id -un)"
fi
USER_ID=$2 GROUP_ID=$3 USER_NAME=$4

DOMAIN=hjv.test
SERVER_IP=203.0.113.10
LAN_A=192.0.2 LAN_B=198.51.100
PC_IP=$LAN_A.67 NAS_IP=$LAN_A.20 GATEWAY=$LAN_A.1
PC_MAC=00:00:5e:00:53:67
HBBS_PORT=41116 HBBR_PORT=41117 API_PORT=41120 SSO_PORT=41600 IMPOSTOR_PORT=41700
PC=/usr/local/lib/hjviewer-e2e/rustdesk # where the PC's commands see the client
PASSWORD="Hjv-e2e-$RANDOM$RANDOM"
# the fake SSO's test accounts (server/api/src/fake_sso.rs)
ALICE=2026000001 ALICE_PW=hjv-test-pass-1
BOB=2026000002 BOB_PW=hjv-test-pass-2
STARTED=$(date +%s)

log() { printf '\n== [%3d s] %s\n' "$(($(date +%s) - STARTED))" "$*"; }
pass() { echo "PASS: $*"; }
fail() { # with what the servers, the relay and the PC last said
  local f
  echo "FAIL: $*"
  for f in hjv-api.log nas/relay.log nas/forged.log pc-server.log; do
    if [ -s "$RUN/$f" ]; then
      echo "--- $RUN/$f"
      tail -n 8 "$RUN/$f"
    fi
  done
  exit 1
}
until_ok() { # <seconds> <what> <command...>: retries the command until it succeeds
  local end=$(($(date +%s) + $1))
  until "${@:3}"; do
    [ "$(date +%s)" -lt "$end" ] || fail "$2 (not within $1 s)"
    sleep 0.2
  done
}
sleep_until() { # <time, date +%s>
  local wait=$(($1 - $(date +%s)))
  if [ "$wait" -gt 0 ]; then
    echo "    (waiting $wait s)"
    sleep "$wait"
  fi
}

# ---------------------------------------------------------------------------------------------
# The networks
# ---------------------------------------------------------------------------------------------
log "networks: the internet, LAN A (the PC, the NAS) and LAN B (the NAS's second card, another host)"
mount -t tmpfs tmpfs /run # ip keeps its network namespaces there; nothing of the host's /run is used
mkdir -p /run/netns
ADMIN_SOCKET=/run/hjv-api-admin.sock # there too: a socket's path is short, whatever the run folder's is
mkdir /run/hjv-run
mount --bind "$RUN" /run/hjv-run # the run folder by a name that a side's own /tmp cannot hide
printf '127.0.0.1 localhost\n%s %s\n' "$SERVER_IP" "$DOMAIN" >"$RUN/hosts"
mount --bind "$RUN/hosts" /etc/hosts
ip link set lo up
for net in wan lan-a lan-b; do
  ip link add "$net" type bridge
  ip link set "$net" up
done
ip addr add "${SERVER_IP%.*}.1/24" dev wan
ip addr add "$SERVER_IP/24" dev wan
ip addr add "$GATEWAY/24" dev lan-a
ip addr add "$LAN_B.1/24" dev lan-b
echo 1 >/proc/sys/net/ipv4/ip_forward
PORTS=()
attach() { # <namespace> <its interface> <network> [<MAC>]: a cable from the network's switch
  local ns=$1 ifname=$2 net=$3 mac=${4:-}
  if [ ! -e "/run/netns/$ns" ]; then
    ip netns add "$ns"
    ip -n "$ns" link set lo up
  fi
  ip link add "$ns-$ifname" type veth peer name "$ifname" netns "$ns"
  ip link set "$ns-$ifname" master "$net" up
  if [ -n "$mac" ]; then ip -n "$ns" link set "$ifname" address "$mac"; fi
  ip -n "$ns" link set "$ifname" up
  PORTS+=("$ns-$ifname")
}
attach pc eth0 lan-a "$PC_MAC"
attach nas eth0 lan-a
attach nas eth1 lan-b
attach other eth0 lan-b
attach phone eth0 wan
attach phone2 eth0 wan
# The NAS reaches the internet through LAN B (the lower metric): a packet that is not sent on the
# card of the PC's network on purpose leaves on the wrong one. Its card on LAN A has the PC's
# gateway.
ip -n nas addr add "$NAS_IP/24" dev eth0
ip -n nas addr add "$LAN_B.20/24" dev eth1
ip -n nas route add default via "$LAN_B.1" dev eth1 metric 100
ip -n nas route add default via "$GATEWAY" dev eth0 metric 200
ip -n other addr add "$LAN_B.30/24" dev eth0
ip -n other route add default via "$LAN_B.1"
ip -n phone addr add "${SERVER_IP%.*}.50/24" dev eth0
ip -n phone2 addr add "${SERVER_IP%.*}.51/24" dev eth0
pc_address() { # add | del: the PC's card with or without its address (a sleeping card has none)
  if [ "$1" = add ]; then
    ip -n pc addr add "$PC_IP/24" dev eth0
    ip -n pc route add default via "$GATEWAY"
  else
    ip -n pc addr del "$PC_IP/24" dev eth0
  fi
}
pc_address add
forwarding() { [[ $(ip -d link show dev "$1") == *"state forwarding"* ]]; }
for port in "${PORTS[@]}"; do until_ok 10 "the switch port $port" forwarding "$port"; done

# <namespace> <side> <command...>: the command as this user (not root, no capabilities) in the
# namespace, with the side's own home, /tmp (the app's IPC lives there), machine ID (the app's
# uuid, by which hjv-api tells an account's devices apart) and host name, and, for a side with an
# `installed` folder, that folder as /usr/local/lib. (A run folder under /tmp, which the side's
# /tmp hides, is put back at its place: what the sides run and read is all in that folder.)
in_side() {
  local ns=$1 side=$2
  shift 2
  ip netns exec "$ns" unshare --uts bash -c '
    hostname "hjv-$1" || exit 1
    mount --bind "$2/tmp" /tmp || exit 1
    if [ ! -d "$2/home" ]; then mkdir -p "$7" && mount --bind /run/hjv-run "$7" || exit 1; fi
    if [ -e /etc/machine-id ]; then mount --bind "$2/machine-id" /etc/machine-id || exit 1; fi
    if [ -d "$2/installed" ]; then mount --bind "$2/installed" /usr/local/lib || exit 1; fi
    exec unshare --user --map-user="$3" --map-group="$4" \
      env -i HOME="$2/home" USER="$5" LOGNAME="$5" PATH=/usr/local/bin:/usr/bin:/bin SSL_CERT_FILE="$6" "${@:8}"
  ' _ "$side" "$RUN/$side" "$USER_ID" "$GROUP_ID" "$USER_NAME" "$RUN/ca-bundle.pem" "$RUN" "$@"
}
for side in pc ctl ctl2 nas; do
  mkdir -p "$RUN/$side/home" "$RUN/$side/tmp"
  head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n' >"$RUN/$side/machine-id"
done
# the run's own copies of the programs its sides run: the client, which the PC sees under /usr and
# the phones where it is (not installed), and hjv-relay
install -D -m 755 "$CLIENT" "$RUN/pc/installed/hjviewer-e2e/rustdesk"
install -m 755 "$HJV_RELAY" "$RUN/nas/hjv-relay"
CLIENT="$RUN/pc/installed/hjviewer-e2e/rustdesk" HJV_RELAY="$RUN/nas/hjv-relay"

# ---------------------------------------------------------------------------------------------
# The server: hbbs and hbbr, hjv-api with the fake Yonsei SSO, and Caddy (deploy/Caddyfile) on 443
# ---------------------------------------------------------------------------------------------
log "server: hbbs, hbbr, hjv-api and the fake Yonsei SSO behind Caddy (deploy/Caddyfile) on $DOMAIN:443"
python3 -I "$HELPER" selftest
mkdir -p "$RUN/srv" "$RUN/api"
install -d -m 700 "$RUN/api/creds"
cp "$WORK/srv/id_ed25519" "$WORK/srv/id_ed25519.pub" "$RUN/srv/"
(cd "$RUN/srv" && exec env -i TEST_HBBS=no RUST_LOG=debug "$HBBS" -p "$HBBS_PORT" -r "$DOMAIN" >"$RUN/hbbs.log" 2>&1) &
(cd "$RUN/srv" && exec env -i RUST_LOG=info "$HBBR" -p "$HBBR_PORT" -k _ >"$RUN/hbbr.log" 2>&1) &
# the fake SSO's registration (server/api/src/fake_sso.rs) and the E2E's keys
printf '%s' aGp2LWZha2UtY2xpZW50Omhqdi1mYWtlLXNlY3JldA== >"$RUN/api/creds/hjviewer-sso-client-secret"
printf '%s' hjv-fake-api-uuid >"$RUN/api/creds/hjviewer-sso-api-uuid"
cp "$KEYS/seal.secret" "$RUN/api/creds/hjviewer-api-login-key"
cp "$KEYS/log.secret" "$RUN/api/creds/hjviewer-api-log-key"
cp "$KEYS/list-1.secret" "$RUN/api/creds/hjviewer-api-list-key"
cp "$KEYS/command-1.secret" "$RUN/api/creds/hjviewer-api-command-key"
chmod 600 "$RUN/api/creds"/*
# the settings a server has, but for the fake SSO and for Linux PCs, which bind here as Windows
# PCs do
cat >"$RUN/api/hjviewer-api.env" <<EOF
HJV_API_SSO_TOKEN_URL=http://127.0.0.1:$SSO_PORT/oauth/token.do
HJV_API_SSO_USERINFO_URL=http://127.0.0.1:$SSO_PORT/api/findByApiUuid.do
HJV_API_SSO_CLIENT_ID=hjv-fake-client
HJV_API_HBBS_CONSOLE=127.0.0.1:$((HBBS_PORT - 1))
HJV_API_HBBS_PUBKEY=$(cat "$RUN/srv/id_ed25519.pub")
HJV_API_BIND_OS=windows,linux
HJV_API_TRUST_PUBKEYS=$(cat "$KEYS/trust.pub")
HJV_API_LIST_KID=1
HJV_API_COMMAND_PUBKEYS=$(cat "$KEYS/command.pub")
HJV_API_COMMAND_KID=1
EOF
"$HJV_FAKE_SSO" --listen "127.0.0.1:$SSO_PORT" >"$RUN/fake-sso.log" 2>&1 &
(cd "$RUN/api" && exec env -i RUST_LOG=info "$HJV_API" serve --listen "127.0.0.1:$API_PORT" \
  --env-file "$RUN/api/hjviewer-api.env" --credentials "$RUN/api/creds" --db "$RUN/api/hjv-api.sqlite3" \
  --admin-socket "$ADMIN_SOCKET" --operator-log "$RUN/api/operator.log" >>"$RUN/hjv-api.log" 2>&1) &
admin() { "$HJV_API" admin --socket "$ADMIN_SOCKET" "$@"; }
admin_to() { # <name> <arguments...>: an admin command whose output is shown and kept as $RUN/<name>.txt
  admin "${@:2}" >"$RUN/$1.txt" 2>&1 || true
  sed 's/^/    /' "$RUN/$1.txt"
}
api_up() { curl -s -o /dev/null "http://127.0.0.1:$API_PORT/api/hjv/source" && [ -S "$ADMIN_SOCKET" ]; }
until_ok 10 "hjv-api's start (see $RUN/hjv-api.log)" api_up
# the second site is the impostor's: it answers in the server's name, with a certificate the
# relays trust, as someone who took over the server's name or its web server would
# (debug: Caddy's log then has every request it passes to hjv-api with the status that came back,
# where the steps see the PC's syncs)
cat >"$RUN/Caddyfile" <<EOF
{
	debug
	local_certs
	skip_install_trust
	admin off
	storage file_system $RUN/caddy
}
import $ROOT/deploy/Caddyfile
$DOMAIN:8443 {
	reverse_proxy 127.0.0.1:$IMPOSTOR_PORT
}
EOF
env -i HOME="$RUN/caddy" HJV_DOMAIN="$DOMAIN" HJV_HBBS_WS="127.0.0.1:$((HBBS_PORT + 2))" \
  HJV_HBBR_WS="127.0.0.1:$((HBBR_PORT + 2))" HJV_API="127.0.0.1:$API_PORT" \
  "$CADDY" run --config "$RUN/Caddyfile" --adapter caddyfile >"$RUN/caddy.log" 2>&1 &
CA_ROOT="$RUN/caddy/pki/authorities/local/root.crt"
# (its certificates come a moment after its authority: a relay that starts before them backs off)
serving() {
  [ -s "$CA_ROOT" ] && curl -s -o /dev/null --cacert "$CA_ROOT" "https://$DOMAIN/" &&
    curl -s -o /dev/null --cacert "$CA_ROOT" "https://$DOMAIN:8443/"
}
until_ok 15 "Caddy's certificates (see $RUN/caddy.log)" serving
cat /etc/ssl/certs/ca-certificates.crt "$CA_ROOT" >"$RUN/ca-bundle.pem"
printf '%s\n%s\n' "$ALICE" "$BOB" >"$RUN/api/allow.txt"
admin allow import "$RUN/api/allow.txt" || fail "hjv-api's allowlist"
# as after the rollout (design v3 11): the same-account logins are on
admin passwordless on || fail "hjv-api's password-less switch"

# ---------------------------------------------------------------------------------------------
# What the steps use
# ---------------------------------------------------------------------------------------------
declare -A SIDE_NS=([pc]=pc [ctl]=phone [ctl2]=phone2)
# <side> <args...>: the side's client. Its standard output is $OUT and its exit code $RC; both
# outputs go to $RUN/<side>.out, with the portal passwords masked. Prints the hjv-e2e lines.
run() {
  local s=$1 client=$CLIENT shown=() a
  shift
  if [ "$s" = pc ]; then client=$PC; fi
  for a in "$@"; do
    case "$a" in "$ALICE_PW" | "$BOB_PW" | "$PASSWORD") shown+=("***") ;; *) shown+=("$a") ;; esac
  done
  RC=0
  OUT=$(in_side "${SIDE_NS[$s]}" "$s" "$client" "$@" 2>"$RUN/$s.err") || RC=$?
  {
    echo "== ${shown[*]} (exit $RC)"
    printf '%s\n' "$OUT"
    cat "$RUN/$s.err"
  } >>"$RUN/$s.out"
  grep '^hjv-e2e:' <<<"$OUT" | sed 's/^/    /' || true
}
has() { grep -q -- "$1" <<<"$OUT"; }
# <side> <METHOD> <path> [<body>]: an account request of the side's login from its phone, signed
# as the app signs it (tools/e2e/wol-helper.py). The answer's status is $STATUS, its body $BODY.
app() {
  local side=$1 line
  shift
  line=$(ip netns exec "${SIDE_NS[$side]}" python3 -I "$HELPER" request "$RUN/$side/home/.config/hjviewer/HJViewer_local.toml" \
    "$CA_ROOT" "$1" "https://$DOMAIN$2" "${@:3}") || fail "no answer to $1 $2"
  STATUS=${line%% *} BODY=${line#* }
  echo "    $1 $2 ${3:-} -> $line"
}
field() { # <name>[.<name>...]: a value of the JSON in $BODY ("" for null or none)
  python3 -I -c 'import json, sys
try:
    value = json.loads(sys.argv[1])
except ValueError:
    value = None
for name in sys.argv[2].split("."):
    value = value.get(name) if isinstance(value, dict) else None
print("" if value is None else value if isinstance(value, str) else json.dumps(value))' "$BODY" "$1"
}
answered() { # <status> <hjv code> <what>: the last answer of app
  [ "$STATUS" = "$1" ] && [ "$(field hjv)" = "$2" ] || fail "$3: $STATUS $BODY, expected $1 $2"
}
job_is() { # <job> <state>: alice's phone asks for the job
  app ctl GET "/api/hjv/wake/$1" >/dev/null
  [ "$STATUS" = 200 ] && [ "$(field state)" = "$2" ]
}
pc_relay_is() { # <match>: hjv-api has a relay for the PC that is connected now, with this match
  app ctl GET "/api/hjv/devices/$ID/wol" >/dev/null
  [ "$STATUS" = 200 ] && [ "$(field relay.match)" = "$1" ] && [ "$(field relay.online)" = true ]
}
pc_state_is() { # hjv-pwless-state, what the account settings show about this PC
  run pc --hjv-e2e-option hjv-pwless-state >/dev/null
  [[ $OUT == "$1"* ]]
}
online() { # hbbs has the PC online, asked from the phones' network
  ip netns exec phone "$PROBE" "$DOMAIN" --ca "$CA_ROOT" --peer "$ID" >"$RUN/probe.log" 2>&1 &&
    grep -q "$ID=online" "$RUN/probe.log"
}
# <time, date +%s>: hjv-api took a sync of the PC (200) at or after that time, by Caddy's log of
# what it passed on
pc_synced_since() {
  python3 -I -c 'import json, sys
for line in open(sys.argv[1], encoding="utf-8", errors="replace"):
    if "upstream roundtrip" not in line:
        continue
    try:
        entry = json.loads(line)
    except ValueError:
        continue
    request = entry.get("request", {})
    if (entry.get("ts", 0) >= float(sys.argv[2]) and request.get("uri") == "/api/hjv/pc/sync"
            and request.get("remote_ip") == sys.argv[3] and entry.get("status") == 200):
        sys.exit(0)
sys.exit(1)' "$RUN/caddy.log" "$1" "$PC_IP"
}
start_pc_service() { in_side pc pc "$PC" --server >>"$RUN/pc-server.log" 2>&1 & }
# hjv-relay on the NAS, as the DSM boot-up task runs it (docs/relay-dsm.md: --state and --log; the
# task's --watch and --stop are left to the relay's own tests), with a status file in the run's
# folder and the test's certificate authority; --server only for the relay before the impostor
start_relay() { # <name of its state> [<server>]
  in_side nas nas "$HJV_RELAY" run --state "$RUN/nas/$1" --log "$RUN/nas/$1.log" \
    --status-file "$RUN/nas/$1.status" --ca "$CA_ROOT" ${2:+--server "$2"} >>"$RUN/nas/relay.out" 2>&1 &
}
stop_relay() { ip netns pids nas | xargs -r ps -o pid=,args= -p | awk -v s="/$1 " '$0 ~ s {print $1}' | xargs -r kill; }
relay_is() { grep -q "\"state\":\"$1\"" "$RUN/nas/${2:-relay}.status" 2>/dev/null; } # <state> [<name>]
# <time, date +%s>: the relay knows that it is active, or has asked hjv-api since that time (relay
# list: "last poll N s ago"): its request is held, and a command reaches it at once
relay_ready_since() {
  local ago
  relay_is active && return 0
  ago=$(admin relay list | sed -n "s/^  $RELAY_ID  active .* last poll \([0-9]*\) s ago .*/\1/p")
  [ -n "$ago" ] && [ $(($(date +%s) - ago)) -gt "$1" ]
}
# The PC goes to sleep: its card keeps its link and loses its address, and its service ends (off:
# a PC that is shut down, whose service starts anew when it wakes) or stops running where it is
# (standby: the service's processes are frozen and run on when the PC wakes, their clock having
# jumped)
FROZEN=()
pc_sleeps() { # off | standby
  local pids
  mapfile -t pids < <(ip netns pids pc | xargs -r ps -o pid=,args= -p | awk '/ --(server|tray)$/ {print $1}')
  [ ${#pids[@]} -gt 0 ] || fail "the PC's service is not running"
  if [ "$1" = standby ]; then
    kill -STOP "${pids[@]}"
    FROZEN=("${pids[@]}")
  else
    kill "${pids[@]}"
    for _ in $(seq 50); do
      kill -0 "${pids[@]}" 2>/dev/null || break
      sleep 0.1
    done
    kill -9 "${pids[@]}" 2>/dev/null || true
  fi
  pc_address del
  ASLEEP=$(date +%s)
}
pc_wakes() {
  WOKE=$(date +%s)
  pc_address add
  if [ ${#FROZEN[@]} -gt 0 ]; then
    kill -CONT "${FROZEN[@]}"
    FROZEN=()
  else
    start_pc_service
  fi
}
# <tag>: packet sockets on the sleeping PC's card and on the other LAN's host, from now on
CAPTURES=()
capture_start() {
  local where
  CAPTURES=()
  for where in pc other; do
    ip netns exec "$where" python3 -I "$CAPTURE" eth0 "$PC_MAC" >"$RUN/capture-$1-$where.txt" 2>&1 &
    CAPTURES+=($!)
    until_ok 5 "the packet socket in $where" grep -q '^listening' "$RUN/capture-$1-$where.txt"
  done
}
capture_stop() {
  sleep 0.5 # what is still on its way
  kill "${CAPTURES[@]}"
  wait "${CAPTURES[@]}" 2>/dev/null || true
}
frames() { grep -c ' magic eth_dst=' "$RUN/capture-$1-$2.txt" || true; } # <tag> <pc|other>
# <step> <tag> <rounds> <broadcast address>: the PC's card got, from the NAS's card on its LAN, to
# the Ethernet broadcast address, <rounds> datagrams of 102 bytes (the magic packet and nothing
# else) for each of 255.255.255.255 and the NAS's broadcast address on each of the ports 9 and 7,
# and nothing else; the other LAN got nothing
frames_are() {
  local step=$1 tag=$2 rounds=$3 file="$RUN/capture-$2-pc.txt" dst port n
  for dst in 255.255.255.255 "$4"; do
    for port in 9 7; do
      n=$(grep -c " magic eth_dst=ff:ff:ff:ff:ff:ff src=$NAS_IP:[0-9]* dst=$dst:$port udp_len=102\$" "$file" || true)
      [ "$n" = "$rounds" ] || fail "$step: $n frames to $dst:$port on the PC's card, expected $rounds ($(frames "$tag" pc) in all)"
    done
  done
  [ "$(frames "$tag" pc)" = $((rounds * 4)) ] || fail "$step: $(frames "$tag" pc) frames on the PC's card, expected $((rounds * 4))"
  [ "$(frames "$tag" other)" = 0 ] || fail "$step: $(frames "$tag" other) frames on the other LAN"
}

# ---------------------------------------------------------------------------------------------
# The steps. Each prints PASS or FAIL with what it saw.
# ---------------------------------------------------------------------------------------------
log "the NAS: hjv-relay starts, as its boot-up task starts it"
start_relay relay

log "W1: the PC (hjv-pc, installed): its service registers; alice logs in on it"
run pc --hjv-e2e-set-password "$PASSWORD"
ID=$(tail -n 1 <<<"$OUT")
[[ $ID =~ ^[0-9]{6,}$ ]] || fail "W1: no device ID ($(tail -n 3 "$RUN/pc.err"))"
echo "device ID: $ID"
start_pc_service
until_ok 40 "W1: the PC online at hbbs" online
run pc --hjv-e2e-login "$ALICE" "$ALICE_PW"
[ "$RC" = 0 ] || fail "W1: the PC's login (exit $RC)"
has 'this device: added' || fail "W1: the PC was not added to the account as an installed one"
until_ok 10 "W1: the PC's binding" pc_state_is active
pass "W1: this device: added; the service holds its binding ($OUT)"

log "W2: the phones of alice and bob log in; the PC reports how to wake it"
run ctl --hjv-e2e-login "$ALICE" "$ALICE_PW"
[ "$RC" = 0 ] || fail "W2: alice's login on her phone"
run ctl2 --hjv-e2e-login "$BOB" "$BOB_PW"
[ "$RC" = 0 ] || fail "W2: bob's login on his phone"
app ctl GET "/api/hjv/devices/$ID/wol"
[ "$STATUS" = 200 ] && [ "$(field ready)" = unknown ] || fail "W2: the PC's verdict before its report is not 'unknown'"
run pc --hjv-e2e-wol-report "$PC_MAC" "$PC_IP/24" "$GATEWAY"
[ "$RC" = 0 ] && has 'wol report taken' || fail "W2: hjv-api did not take the PC's report ($(cat "$RUN/pc.err"))"
app ctl GET "/api/hjv/devices/$ID/wol"
[ "$(field ready)" = ok ] && [ "$(field hints)" = "[]" ] || fail "W2: the PC's verdict is not 'ok' without hints"
[ -z "$(field relay)" ] || fail "W2: a relay before any was approved"
if grep -qi "$PC_MAC" <<<"$BODY"; then fail "W2: the PC's MAC went to the app"; fi
run ctl --hjv-e2e-devices
has "device $ID linux pwless=active" || fail "W2: alice's list has no active PC"
if has '켜기'; then fail "W2: the list says the PC of an account without a relay can be woken"; fi
pass "W2: wol report taken; the verdict is ok, without the MAC; no relay yet"

log "W3: hjv-relay on the NAS waits for its approval"
until_ok 10 "W3: the relay's hello" relay_is pending
RELAY_ID=$(sed -n 's/.*중계기 ID \(r-[a-z2-7]\{16\}\), 등록 코드 .*, 새 키 .*/\1/p' "$RUN/nas/relay.log")
CODE=$(sed -n 's/.*등록 승인을 기다립니다. 등록 코드: \([A-Z2-7-]\{19\}\) .*/\1/p' "$RUN/nas/relay.log")
[ -n "$RELAY_ID" ] && [ -n "$CODE" ] || fail "W3: the relay's log has no ID of a new key, or no enrollment code"
admin_to relay-list relay list
grep -q "^  $RELAY_ID  pending  code $CODE " "$RUN/relay-list.txt" || fail "W3: hjv-api's pending relay is not the one of the relay's log"
grep -q 'root가 아니어서' "$RUN/nas/relay.log" || fail "W3: the relay does not say that it runs without root"
pass "W3: pending, with the code of the relay's log ($CODE)"

log "W4: the PC is on: nothing to wake"
app ctl POST "/api/hjv/devices/$ID/wake" '{}'
answered 409 online "W4: a wake request for a PC that is on"
pass "W4: 409 online ($(field error))"

log "W5: another account (bob) gets 404 for alice's PC"
app ctl2 POST "/api/hjv/devices/$ID/wake" '{}'
[ "$STATUS" = 404 ] || fail "W5: bob's wake request got $STATUS"
app ctl2 GET "/api/hjv/devices/$ID/wol"
[ "$STATUS" = 404 ] || fail "W5: bob's question about the PC got $STATUS"
pass "W5: 404 for the wake request and for the PC's verdict"

log "the PC is shut down: its service ends, its card keeps the link and loses its address"
pc_sleeps off

# ---------------------------------------------------------------------------------------------
# Meanwhile (hjv-api counts a PC as on for 30 seconds after it last saw it): a second relay on
# the NAS before the impostor, which hands it commands hjv-api never signs
# ---------------------------------------------------------------------------------------------
log "F0: a second relay on the NAS, before a server that answers in hjv-api's name ($DOMAIN:8443)"
python3 -I "$HELPER" impostor "127.0.0.1:$IMPOSTOR_PORT" "$RUN/impostor" "$KEYS/command-1.secret" "$PC_MAC" "$LAN_A.0/24" \
  >"$RUN/impostor.log" 2>&1 &
until_ok 5 "the impostor" grep -q '^impostor listening' "$RUN/impostor.log"
start_relay forged "$DOMAIN:8443"
until_ok 10 "F0: the second relay active before the impostor" relay_is active forged
results_are() { [ -f "$RUN/impostor/results.jsonl" ] && [ "$(grep -c '"case"' "$RUN/impostor/results.jsonl")" -ge "$1" ]; }
# <case> <error> <packets>: the relay's result for a forged command
result_is() {
  grep -q "^{\"case\": \"$1\", \"error\": $2, \"packets\": $3, " "$RUN/impostor/results.jsonl" ||
    fail "the relay's result for '$1' is not $2 with $3 packets: $(grep "\"case\": \"$1\"," "$RUN/impostor/results.jsonl" || echo none)"
  echo "    $1: $2, $3 packets"
}
pass "F0: active (a relay believes the state the server's name tells it; commands are another matter)"

log "F1: commands without the command key's signature, old ones, another relay's, and ones outside a command's bounds"
capture_start forged
python3 -I "$HELPER" forge "$RUN/impostor" no-signature bad-signature other-key unknown-key tampered \
  expired other-relay five-macs broadcast-mac unknown-net port-80
until_ok 15 "F1: the relay's results" results_are 11
capture_stop
result_is no-signature '"bad_command"' 0
result_is bad-signature '"bad_signature"' 0
result_is other-key '"bad_signature"' 0
result_is unknown-key '"unknown_key"' 0
result_is tampered '"bad_signature"' 0
result_is expired '"expired"' 0
result_is other-relay '"wrong_relay"' 0
result_is five-macs '"bad_macs"' 0
result_is broadcast-mac '"bad_macs"' 0
result_is unknown-net '"no_iface"' 0
result_is port-80 '"bad_params"' 0
[ "$(frames forged pc)" = 0 ] && [ "$(frames forged other)" = 0 ] ||
  fail "F1: $(frames forged pc) frames on the PC's card and $(frames forged other) on the other LAN for refused commands"
# the NAS's administrator reads the same reasons in the relay's log
for reason in bad_command:1 bad_signature:3 unknown_key:1 expired:1 wrong_relay:1 bad_macs:2 no_iface:1 bad_params:1; do
  [ "$(grep -c "보내지 않았습니다: ${reason%:*}\$" "$RUN/nas/forged.log")" = "${reason#*:}" ] ||
    fail "F1: the relay's log does not name ${reason%:*} ${reason#*:} time(s)"
done
pass "F1: 11 forged commands refused, each with its reason in the result and in the relay's log; no frame on either LAN"

log "F2: eleven good commands within a minute, and the first of them once more"
capture_start limit
python3 -I "$HELPER" forge "$RUN/impostor" valid valid valid valid valid valid valid valid valid valid valid again:valid
until_ok 15 "F2: the relay's results" results_are 23
capture_stop
for n in "" -2 -3 -4 -5 -6 -7 -8 -9 -10; do
  grep -q "^{\"case\": \"valid$n\", \"error\": null, \"packets\": 4, \"iface\": \"eth0\"" "$RUN/impostor/results.jsonl" ||
    fail "F2: the command valid$n did not send its 4 packets on eth0"
done
result_is valid-11 '"rate_limited"' 0
result_is again:valid '"replayed"' 0
frames_are F2 limit 10 "$LAN_A.255"
pass "F2: ten commands sent (40 frames on the PC's card, none on the other LAN); the eleventh rate_limited, the repeated job replayed"
stop_relay forged

# ---------------------------------------------------------------------------------------------
# The wake
# ---------------------------------------------------------------------------------------------
log "W6: the PC is off, and its account has no approved relay yet"
sleep_until $((ASLEEP + 32))
app ctl POST "/api/hjv/devices/$ID/wake" '{}'
answered 409 no_relay "W6: a wake request of an account without a relay"
[ "$(field reason)" = no_relays ] && [ "$(field any_relay)" = false ] || fail "W6: not no_relays, without the choice of any relay"
pass "W6: 409 no_relay, no_relays ($(field error))"

log "W7: the administrator approves the relay's code for the owner of the PC"
admin_to relay-approve relay approve "$CODE" --owner-of "$ID"
APPROVED=$(date +%s)
grep -q "^approved: the relay $RELAY_ID " "$RUN/relay-approve.txt" || fail "W7: not approved"
grep -q "on the PC's network: yes (the same network and gateway)" "$RUN/relay-approve.txt" ||
  fail "W7: the verdict is not 'the same network and gateway'"
if grep -q "$LAN_A\.\|$LAN_B\.\|${SERVER_IP%.*}\.\|${PC_MAC:0:8}" "$RUN/relay-approve.txt" "$RUN/relay-list.txt"; then
  fail "W7: an address or a MAC in the default output of relay approve or relay list"
fi
admin --reveal relay list >"$RUN/relay-list-reveal.txt" || fail "W7: relay list --reveal"
grep -q "relay eth0 $NAS_IP/24 gateway $GATEWAY " "$RUN/relay-list-reveal.txt" ||
  fail "W7: --reveal does not show the relay's card on LAN A"
grep -q "PC card \*\*:\*\*:\*\*:\*\*:${PC_MAC:12} $PC_IP/24 gateway $GATEWAY " "$RUN/relay-list-reveal.txt" ||
  fail "W7: --reveal does not show the PC's card with its MAC masked"
grep -q "egress: relay $LAN_B.20, PC $PC_IP\$" "$RUN/relay-list-reveal.txt" ||
  fail "W7: --reveal does not show the egress addresses of the NAS (through LAN B) and the PC"
# A relay that waits for its approval asks every 30 seconds, and hjv-api holds its request for
# 20: approved meanwhile, it hears at once. Approved while it pauses, it asks again within 30
# seconds, as an approved relay now: that request is held for commands, and the relay hears of its
# approval with the answer (with the first command, or after 20 seconds). Until it has asked, a
# command would wait for it.
until_ok 40 "W7: the relay's first request as an approved one" relay_ready_since "$APPROVED"
if relay_is active; then
  HEARD="the relay, whose request hjv-api held, heard within $(($(date +%s) - APPROVED)) s"
else
  HEARD="the relay asked again $(($(date +%s) - APPROVED)) s later, as an approved one"
fi
until_ok 5 "W7: the PC's relay, connected and an exact match" pc_relay_is exact
RELAY_NAME=$(field relay.name)
[ "$RELAY_NAME" = "NAS ${CODE%%-*}" ] || fail "W7: the relay's name is '$RELAY_NAME'"
run ctl --hjv-e2e-devices
has "device $ID linux .* · 켜기 가능" || fail "W7: alice's list does not say that the PC can be woken"
pass "W7: approved (the same network and gateway), no address in the default output; $HEARD;" \
  "the app shows \"$RELAY_NAME\" and 켜기 가능"

log "W8: alice's phone wakes the PC"
capture_start wake
REQUESTED=$(date +%s)
app ctl POST "/api/hjv/devices/$ID/wake" '{}'
QUEUED=$(date +%s)
[ "$STATUS" = 202 ] && [ "$(field state)" = queued ] || fail "W8: the wake request got $STATUS $BODY"
[ "$(field relay.name)" = "$RELAY_NAME" ] && [ "$(field relay.match)" = exact ] || fail "W8: not the relay, as an exact match"
JOB=$(field job)
until_ok 5 "W8: the job sent" job_is "$JOB" sent
[ "$(field packets)" = 12 ] || fail "W8: the job has $(field packets) packets"
capture_stop
frames_are W8 wake 3 "$LAN_A.255"
grep -q '등록이 승인되어 켜기 명령을 기다립니다' "$RUN/nas/relay.log" || fail "W8: the relay's log does not say that it was approved"
grep -q "켜기 ${JOB:0:8} (\*\*:\*\*:\*\*:\*\*:${PC_MAC:12}, 망 $LAN_A.0/24 exact): eth0 $NAS_IP/24 .*: 12/12개 보냄" "$RUN/nas/relay.log" ||
  fail "W8: the relay's log has no line of the 12 packets, with the MAC masked"
# what the NAS's administrator sees (docs/relay-dsm.md, the status task)
in_side nas nas "$HJV_RELAY" status --state "$RUN/nas/relay" --status-file "$RUN/nas/relay.status" \
  --log "$RUN/nas/relay.log" >"$RUN/nas/status.txt" 2>&1 || fail "W8: hjv-relay status"
grep -q '등록 상태: active' "$RUN/nas/status.txt" && grep -q "job $JOB 랜카드 eth0 패킷 12 결과 ok" "$RUN/nas/status.txt" ||
  fail "W8: hjv-relay status does not show the registration and the command ($RUN/nas/status.txt)"
pass "W8: 202 queued (\"$RELAY_NAME\", exact) -> 12 frames on the PC's card (255.255.255.255 and $LAN_A.255," \
  "ports 9 and 7, three times), none on the other LAN -> sent"

log "the NAS's relay is switched off (hjv-api counts a relay as connected for a minute after its last request)"
stop_relay relay
RELAY_OFF=$(date +%s)

log "W9: a second request for the PC within the minute is the same job"
app ctl POST "/api/hjv/devices/$ID/wake" '{}'
[ "$STATUS" = 202 ] && [ "$(field job)" = "$JOB" ] && [ "$(field state)" = sent ] || fail "W9: not the job under way"
app ctl2 GET "/api/hjv/wake/$JOB"
[ "$STATUS" = 404 ] || fail "W9: bob can read alice's job ($STATUS)"
pass "W9: 202 with the job under way; bob gets 404 for it"

log "W10: the PC wakes: its card has its address again and its service starts"
pc_wakes
until_ok 30 "W10: the job confirmed" job_is "$JOB" confirmed
app ctl GET "/api/hjv/wake/$JOB"
[ "$(field confirmed_at)" -ge "$(field sent_at)" ] || fail "W10: confirmed before the packets"
CONFIRMED=$(($(date +%s) - WOKE))
pc_synced_since "$WOKE" || fail "W10: no sync of the PC since it woke"
until_ok 30 "W10: the PC online at hbbs again" online
pass "W10: confirmed $CONFIRMED s after the PC woke ($(($(date +%s) - REQUESTED)) s after the request); hbbs has it online again"

log "the PC goes into standby: its service stops where it is, its card loses its address"
pc_sleeps standby

log "W11: within the minute of a job that is over, the next request for the PC waits"
if [ $((ASLEEP + 32)) -lt $((REQUESTED + 57)) ]; then
  sleep_until $((ASLEEP + 32))
  app ctl POST "/api/hjv/devices/$ID/wake" '{}'
  answered 429 rate_limited "W11: a request within the minute of a job that is over"
  [ "$(field retry_after)" -ge 1 ] && [ "$(field retry_after)" -le 60 ] || fail "W11: retry_after is '$(field retry_after)'"
  pass "W11: 429 rate_limited, retry_after $(field retry_after)"
else
  echo "skipped: W11 (the job's minute was over before the PC had slept for 30 seconds: a slow run)"
fi

log "W12: the relay is off: 503 with the time it last asked"
sleep_until $((QUEUED + 61))
sleep_until $((RELAY_OFF + 61))
sleep_until $((ASLEEP + 32))
app ctl POST "/api/hjv/devices/$ID/wake" '{}'
answered 503 relay_offline "W12: a wake request while the relay is off"
SEEN=$(field last_seen)
[ -n "$SEEN" ] && [ "$SEEN" -ge "$REQUESTED" ] && [ "$SEEN" -le "$RELAY_OFF" ] || fail "W12: last_seen is '$SEEN'"
pass "W12: 503 relay_offline ($(field error)), last_seen $((RELAY_OFF - SEEN)) s before the relay was switched off"

log "W13: the NAS's card gets a /25 on the PC's /24 wire; the relay starts again with its key and wakes the PC in standby"
ip -n nas addr del "$NAS_IP/24" dev eth0
ip -n nas addr add "$NAS_IP/25" dev eth0
ip -n nas route add default via "$GATEWAY" dev eth0 metric 200
rm -f "$RUN/nas/relay.status"
start_relay relay
until_ok 10 "W13: the relay active again, without a new approval" relay_is active
[ "$(grep -c "중계기 ID $RELAY_ID, 등록 코드 $CODE" "$RUN/nas/relay.log")" = 2 ] || fail "W13: the relay did not start again with its ID and code"
[ "$(grep -c '새 키' "$RUN/nas/relay.log")" = 1 ] || fail "W13: the relay made a new key"
until_ok 10 "W13: the PC's relay, connected and an overlap match" pc_relay_is overlap
capture_start overlap
app ctl POST "/api/hjv/devices/$ID/wake" '{}'
[ "$STATUS" = 202 ] && [ "$(field relay.match)" = overlap ] || fail "W13: the wake request got $STATUS $BODY, expected an overlap match"
JOB2=$(field job)
until_ok 5 "W13: the job sent" job_is "$JOB2" sent
capture_stop
frames_are W13 overlap 3 "$LAN_A.127"
grep -q "켜기 ${JOB2:0:8} (.*, 망 $LAN_A.0/24 within): eth0 $NAS_IP/25 -> 255.255.255.255, $LAN_A.127 " "$RUN/nas/relay.log" ||
  fail "W13: the relay's log does not show its own /25 for the PC's /24"
pass "W13: active at once after the restart; 202 (overlap) -> 12 frames on the PC's card, to 255.255.255.255" \
  "and the NAS's own broadcast address $LAN_A.127, none to the PC's $LAN_A.255 -> sent"

log "W14: the PC resumes: its card has its address again and its service runs on"
pc_wakes
until_ok 15 "W14: a sync of the PC right after the resume (its service saw its clock jump)" pc_synced_since "$WOKE"
SYNCED=$(($(date +%s) - WOKE))
until_ok 30 "W14: the second job confirmed" job_is "$JOB2" confirmed
CONFIRMED=$(($(date +%s) - WOKE))
until_ok 30 "W14: the PC online at hbbs again" online
pass "W14: the service synced within $SYNCED s of the resume, the job was confirmed after $CONFIRMED s, and hbbs has the PC online again"

log "W15: the administrator revokes the relay"
admin_to relay-revoke relay revoke "$RELAY_ID"
grep -q "^revoked the relay $RELAY_ID: it takes no commands" "$RUN/relay-revoke.txt" || fail "W15: not revoked"
until_ok 10 "W15: the relay told that it is revoked" relay_is revoked
grep -q '서버가 이 중계기를 폐기했습니다' "$RUN/nas/relay.log" || fail "W15: the relay's log does not say that it was revoked"
app ctl GET "/api/hjv/devices/$ID/wol"
[ "$STATUS" = 200 ] && [ -z "$(field relay)" ] || fail "W15: the PC still has a relay"
run ctl --hjv-e2e-devices
if has '켜기'; then fail "W15: the list still says the PC can be woken"; fi
admin relay list >"$RUN/relay-list-revoked.txt" || fail "W15: relay list"
grep -q '^relays: 0 active, 0 pending, 1 revoked' "$RUN/relay-list-revoked.txt" || fail "W15: relay list does not have it revoked"
pass "W15: revoked: the relay is told at once and says so in its log; the PC has no relay and the list no 켜기"

log "L1: what the logs and the operator's report keep"
admin report >"$RUN/report.txt" || fail "L1: hjv-api's report"
sed -n '/^relays:/,/^records/p' "$RUN/report.txt" | sed '$d; s/^/    /'
grep -q '^PCs that reported how to wake them: 1$' "$RUN/report.txt" && grep -q '^  confirmed  *2 / 2$' "$RUN/report.txt" ||
  fail "L1: the report does not have the PC's wake report and the two confirmed jobs"
for f in "$RUN/hjv-api.log" "$RUN/api/operator.log" "$RUN/nas/relay.log" "$RUN/nas/forged.log"; do
  [ -s "$f" ] || fail "L1: no $f"
  if grep -qi "$PC_MAC" "$f"; then fail "L1: the PC's MAC in $f"; fi
done
if grep -q "$LAN_A\.\|$LAN_B\.\|$ID" "$RUN/hjv-api.log" "$RUN/api/operator.log"; then
  fail "L1: an address of the LANs or the PC's ID in hjv-api's log or its operator record"
fi
SECRETS=()
for side in pc ctl ctl2; do
  for name in access_token hjv-ctl-key; do
    secret=$(sed -n "s/^$name = ['\"]\(.*\)['\"]\$/\1/p" "$RUN/$side/home/.config/hjviewer/HJViewer_local.toml")
    [ -n "$secret" ] || fail "L1: $side keeps no $name"
    SECRETS+=("$secret")
  done
done
# (the files are listed first: with pipefail, a grep -q that leaves early fails the pipeline)
for secret in "$ALICE_PW" "$BOB_PW" "${SECRETS[@]}"; do
  found=$(grep -rlF -- "$secret" "$RUN" | grep -v '/home/.config/hjviewer/HJViewer_local.toml$' || true)
  [ -z "$found" ] || fail "L1: a portal password, a token or an account key outside the clients' LocalConfig: $found"
done
KEYS_KEPT=0
for key in "$RUN"/api/creds/hjviewer-api-*-key; do
  [ -s "$key" ] || fail "L1: hjv-api's keys are not in $RUN/api/creds"
  KEYS_KEPT=$((KEYS_KEPT + 1))
  found=$(grep -rlF -- "$(<"$key")" "$RUN" | grep -v "^$RUN/api/creds/" || true)
  [ -z "$found" ] || fail "L1: one of hjv-api's keys outside its credentials: $found"
done
pass "L1: no MAC, LAN address or PC ID in hjv-api's log and operator record, the MAC masked in the relays' logs;" \
  "2 portal passwords, ${#SECRETS[@]} tokens and account keys and hjv-api's $KEYS_KEPT keys nowhere else in the run's folder"

echo
echo "PASS (wol): the PC woken through the NAS's relay when shut down and when in standby, and confirmed each time; $(($(date +%s) - STARTED)) s"

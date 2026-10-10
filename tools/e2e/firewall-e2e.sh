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
set -euo pipefail

MODE=${1:-strict}
case "$MODE" in strict | open) ;; *) echo "usage: $0 [strict|open]" >&2; exit 2 ;; esac
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
WORK=${HJV_E2E_WORK:?set HJV_E2E_WORK to a scratch directory}
CADDY=${CADDY:?set CADDY to a caddy binary}
HBBS=${HBBS:-$ROOT/upstream/rustdesk-server/target/release/hbbs}
HBBR=${HBBR:-$ROOT/upstream/rustdesk-server/target/release/hbbr}
CLIENT=${CLIENT:-$ROOT/upstream/rustdesk/target/release/rustdesk}
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

log() { printf '\n== %s\n' "$*"; }

cleanup() {
  set +e
  for ns in hjv-pc hjv-ctl; do
    sudo ip netns pids "$ns" 2>/dev/null | xargs -r sudo kill 2>/dev/null
  done
  [ ${#SERVER_PIDS[@]} -gt 0 ] && kill "${SERVER_PIDS[@]}" 2>/dev/null
  sleep 0.5
  for ns in hjv-pc hjv-ctl; do
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

log "network: two namespaces, firewall '$MODE' (out: TCP 443 to the server$([ "$MODE" = open ] && echo ', UDP between peers'))"
setup_ns hjv-pc veth-hjvpc 10.77.1 10.77.2.2
setup_ns hjv-ctl veth-hjvctl 10.77.2 10.77.1.2
if [ "$MODE" = open ]; then
  sudo sysctl -qw net.ipv4.ip_forward=1
fi

log "server: hbbs + hbbr behind Caddy (deploy/Caddyfile) on :443"
(cd "$RUN/srv" && exec env -i TEST_HBBS=no RUST_LOG=debug "$HBBS" -p "$HBBS_PORT" -r "$DOMAIN" >"$RUN/hbbs.log" 2>&1) &
SERVER_PIDS+=($!)
(cd "$RUN/srv" && exec env -i RUST_LOG=info "$HBBR" -p "$HBBR_PORT" -k _ >"$RUN/hbbr.log" 2>&1) &
SERVER_PIDS+=($!)
cat >"$RUN/Caddyfile" <<EOF
{
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
  HJV_HBBR_WS="127.0.0.1:$((HBBR_PORT + 2))" "$CADDY" run --config "$RUN/Caddyfile" --adapter caddyfile >"$RUN/caddy.log" 2>&1 &
SERVER_PIDS+=($!)
CA_ROOT="$RUN/caddy/pki/authorities/local/root.crt"
for _ in $(seq 50); do [ -s "$CA_ROOT" ] && break; sleep 0.2; done
cat /etc/ssl/certs/ca-certificates.crt "$CA_ROOT" >"$RUN/ca-bundle.pem"

log "controlled PC (hjv-pc): password, register over wss, serve a test page on its loopback"
ID=$(in_ns hjv-pc pc "$CLIENT" --hjv-e2e-set-password "$PASSWORD" | tail -1)
echo "device ID: $ID"
in_ns hjv-pc pc python3 -m http.server 8099 --bind 127.0.0.1 --directory "$RUN/site" >"$RUN/pc-site.log" 2>&1 &
# a desktop to share, on a virtual display
in_ns hjv-pc pc bash -c '
  Xvfb :99 -screen 0 1280x720x24 -nolisten tcp >"$1/xvfb.log" 2>&1 &
  for _ in $(seq 50); do [ -e /tmp/.X11-unix/X99 ] && break; sleep 0.1; done
  export DISPLAY=:99
  display -geometry +0+0 "$1/desktop.png" &
  exec "$2" --server
' _ "$RUN/pc" "$CLIENT" >"$RUN/pc-server.log" 2>&1 &

log "controller (hjv-ctl): the firewall lets nothing but HTTPS to the server out"
blocked() { # <description> <cmd...>
  if in_ns hjv-ctl ctl "${@:2}" >/dev/null 2>&1; then
    echo "FAIL: $1 is reachable"
    exit 1
  fi
  echo "blocked as expected: $1"
}
blocked "hbbs TCP $HBBS_PORT (RustDesk's native port)" curl -s --max-time 3 "http://10.77.2.1:$HBBS_PORT/"
blocked "hbbs WebSocket port $((HBBS_PORT + 2)) directly" curl -s --max-time 3 "http://10.77.2.1:$((HBBS_PORT + 2))/"
blocked "UDP to hbbs" bash -c "echo x >/dev/udp/10.77.2.1/$HBBS_PORT"
blocked "the internet" curl -s --max-time 3 https://1.1.1.1/

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
    sudo ip netns pids hjv-pc | xargs -r ps -o pid=,comm= -p | awk '$2 == "animate" {print $1}' | xargs -r sudo kill 2>/dev/null || true
  done
fi

log "what hbbs saw"
grep -E "update_pk|Punch hole|Tcp connection from 10\.77" "$RUN/hbbs.log" | sed 's/^\[[^]]*\] //' | head -8
echo
echo "PASS ($MODE): $(grep -o 'transport: .*' "$RUN/ctl.log" | tail -1)"

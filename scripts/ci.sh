#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Server-side CI, run on the development machine (GitHub CI is paused; docs/ci.md).
#   scripts/ci.sh [all]    both stages
#   scripts/ci.sh server   a fresh upstream checkout with server/patches, the signaling and relay
#                          tests, hjv-probe and hjv-load
#   scripts/ci.sh deploy   docs/server-runbook.md's deployment with Docker, as on the server, from a
#                          copy of this working tree: path B (hbbs/hbbr on 127.0.0.1) behind
#                          deploy/nginx-hjviewer.conf and deploy/apache-hjviewer.conf, then path A
#                          (Caddy). Everything listens on 127.0.0.1; ports 80/443 stay untouched.
# Needs cargo, curl, openssl, Docker with compose v2 (used through sudo, as the runbook does), and
# apt-get for Ubuntu's nginx and Apache packages, which are unpacked under build/ci, not installed.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CI="${HJV_CI_DIR:-$ROOT/build/ci}"
PROJECT=hjv-ci                     # compose project of the test stack, apart from a real deployment
PROBE="$ROOT/tools/hjv-probe/target/release/hjv-probe"
LOAD="$ROOT/tools/hjv-load/target/release/hjv-load"
export PATH="$HOME/.cargo/bin:$PATH"
mkdir -p "$CI"

SRC="$CI/src" PKGS="$CI/pkgs" CERTS="$CI/certs"
NGINX_PID="" APACHE_PID=""

step() { printf '\n== %s\n' "$*"; }
dc() { (cd "$SRC/deploy" && sudo docker compose -p "$PROJECT" "$@"); }

fail() {
  echo "FAIL: $*" >&2
  if [ -d "$SRC/deploy" ]; then
    dc logs --tail 40 >&2 2>/dev/null || true
    for f in "$CI"/nginx/error.log "$CI"/apache/error.log; do
      [ -f "$f" ] && { echo "--- $f" >&2; tail -n 20 "$f" >&2; }
    done
  fi
  exit 1
}

stop_web() {
  [ -n "$NGINX_PID" ] && kill "$NGINX_PID" 2>/dev/null && wait "$NGINX_PID" 2>/dev/null
  [ -n "$APACHE_PID" ] && kill "$APACHE_PID" 2>/dev/null && wait "$APACHE_PID" 2>/dev/null
  NGINX_PID="" APACHE_PID=""
  return 0
}

cleanup() {
  stop_web
  # only this script's own project: its containers, network and Caddy volume
  [ -d "$SRC/deploy" ] && dc down -v --remove-orphans > /dev/null 2>&1
  return 0
}

wait_for() { # <seconds> <command...>
  local n=$1; shift
  for _ in $(seq "$n"); do "$@" > /dev/null 2>&1 && return 0; sleep 1; done
  return 1
}

server_stage() {
  step "server: upstream checkout with server/patches"
  rm -rf "$CI/upstream/rustdesk-server"
  UPSTREAM_DIR="$CI/upstream" "$ROOT/scripts/fetch-upstream.sh" server
  step "server: signaling and relay tests"
  (cd "$CI/upstream/rustdesk-server" &&
    CARGO_TARGET_DIR="$CI/target" cargo test --release --locked --test tcp_signaling --test relay_pairing)
  step "server: hjv-probe and hjv-load"
  for t in hjv-probe hjv-load; do (cd "$ROOT/tools/$t" && cargo build --release --locked); done
}

# The runbook's checks against one web server: WebSocket upgrades to hbbs and hbbr, the /api/
# answer, the source notice, hjv-probe, three PCs registering (hbbs must see their ports), a relay
# session with the server key, and the same key in hbbs, hbbr and data/id_ed25519.pub.
check_site() { # <port> <CA file> <status of /> <name>
  local port=$1 ca=$2 root_status=$3 name=$4 code p before
  local url="https://localhost:$port" curl_tls=(--cacert "$ca" --resolve "localhost:$port:127.0.0.1")
  step "deploy: checks through $name on 127.0.0.1:$port"
  for p in /ws/id /ws/relay; do
    code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 --http1.1 "${curl_tls[@]}" \
      -H 'Connection: Upgrade' -H 'Upgrade: websocket' -H 'Sec-WebSocket-Version: 13' \
      -H 'Sec-WebSocket-Key: SGVsbG9Xb3JsZDEyMzQ1Ng==' "$url$p" || true)
    echo "$p $code"
    [ "$code" = 101 ] || fail "$name: $p answered $code, not 101"
  done
  code=$(curl -s -o /dev/null -w '%{http_code}' "${curl_tls[@]}" -X POST -d '{}' "$url/api/heartbeat" || true)
  echo "/api/heartbeat $code"
  [ "$code" = 404 ] || fail "$name: /api/heartbeat answered $code, not 404"
  code=$(curl -s -o "$CI/root.txt" -w '%{http_code}' "${curl_tls[@]}" "$url/" || true)
  echo "/ $code $(head -c 80 "$CI/root.txt")"
  if [ "$code" != "$root_status" ] || ! grep -q 'github.com/ysfaculty/hjviewer' "$CI/root.txt"; then
    fail "$name: / answered $code without the source notice"
  fi
  "$PROBE" localhost --port "$port" --ca "$ca" || fail "$name: hjv-probe"
  before=$(dc logs --no-log-prefix hbbs 2>/dev/null | grep -c 'update_pk ' || true)
  "$LOAD" peers localhost --port "$port" --ca "$ca" -n 3 --secs 12 || fail "$name: hjv-load peers"
  { dc logs --no-log-prefix hbbs | grep -oE 'update_pk [0-9]+ [^ ]+' || true; } | tail -n "+$((before + 1))" > "$CI/pk.txt"
  cat "$CI/pk.txt"
  [ "$(wc -l < "$CI/pk.txt")" = 3 ] || fail "$name: hbbs did not register 3 PCs"
  if grep -q ':0$' "$CI/pk.txt"; then fail "$name: X-Real-Port did not reach hbbs (port 0)"; fi
  "$LOAD" relay localhost --port "$port" --ca "$ca" -n 1 --mbps 2 --secs 5 --key "$(sudo cat "$SRC/deploy/data/id_ed25519.pub")" ||
    fail "$name: hjv-load relay"
}

check_stack() { # keys, key file permissions, container hardening
  local key s k
  key=$(sudo cat "$SRC/deploy/data/id_ed25519.pub")
  for s in hbbs hbbr; do
    k=$({ dc logs --no-log-prefix "$s" | grep -o 'Key: [A-Za-z0-9+/=]*' || true; } | tail -1 | cut -d' ' -f2)
    echo "$s key $k"
    [ "$k" = "$key" ] || fail "$s runs with a key other than data/id_ed25519.pub"
    [ "$(sudo docker inspect -f '{{.HostConfig.CapDrop}}' "$(dc ps -q "$s")")" = "[ALL]" ] ||
      fail "$s keeps capabilities"
  done
  sudo chmod 600 "$SRC/deploy/data/id_ed25519"
  [ "$(sudo stat -c '%a %U' "$SRC/deploy/data")" = "700 root" ] || fail "data/ is not 700 root"
  [ "$(sudo stat -c '%a %U' "$SRC/deploy/data/id_ed25519")" = "600 root" ] || fail "id_ed25519 is not 600 root"
}

deploy_stage() {
  if [ ! -x "$PROBE" ] || [ ! -x "$LOAD" ]; then fail "hjv-probe/hjv-load are not built: scripts/ci.sh server"; fi
  sudo docker compose version > /dev/null 2>&1 || fail "Docker with compose v2 is needed (docs/server-runbook.md 3-1)"
  [ -z "$(ss -Hltnu '( sport >= :21115 and sport <= :21119 )')" ] ||
    fail "ports 21115-21119 are in use (a running hjviewer server?); the test stack needs them"
  [ -z "$(ss -Hltn '( sport >= :18080 and sport <= :18082 ) or ( sport >= :18443 and sport <= :18445 )')" ] ||
    fail "test ports 18080-18082 / 18443-18445 are in use"
  trap cleanup EXIT

  step "deploy: copy of this working tree"
  if [ -d "$SRC/deploy" ]; then dc down -v --remove-orphans > /dev/null 2>&1 || true; fi
  sudo rm -rf "$SRC"
  mkdir -p "$SRC"
  (cd "$ROOT" && git ls-files -z --cached --others --exclude-standard |
    tar --null --ignore-failed-read -T - -cf -) | tar -xf - -C "$SRC"

  step "deploy: Ubuntu's nginx and Apache, unpacked"
  if [ ! -x "$PKGS/root/usr/sbin/nginx" ] || [ ! -x "$PKGS/root/usr/sbin/apache2" ]; then
    rm -rf "$PKGS"
    mkdir -p "$PKGS/root"
    (cd "$PKGS" && apt-get download nginx apache2-bin libapr1t64 libaprutil1t64 libjansson4 liblua5.4-0 > /dev/null)
    for d in "$PKGS"/*.deb; do dpkg-deb -x "$d" "$PKGS/root"; done
  fi
  "$PKGS/root/usr/sbin/nginx" -v
  LD_LIBRARY_PATH="$PKGS/root/usr/lib/x86_64-linux-gnu" "$PKGS/root/usr/sbin/apache2" -v | head -1
  mkdir -p "$CERTS"
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 2 -subj /CN=localhost \
    -addext subjectAltName=DNS:localhost -keyout "$CERTS/key.pem" -out "$CERTS/cert.pem" 2> /dev/null

  step "deploy B: .env, image build, hbbs and hbbr (docker-compose.behind-proxy.yml)"
  (cd "$SRC/deploy" &&
    sudo install -d -m 700 data &&
    cp .env.example .env &&
    sed -i 's/^HJV_DOMAIN=.*/HJV_DOMAIN=localhost/' .env &&
    printf 'COMPOSE_FILE=docker-compose.behind-proxy.yml\nHJV_TOTAL_BANDWIDTH=100\nHJV_BUILD_JOBS=%s\n' "$(nproc)" >> .env)
  [ "$(dc config --services | sort | paste -sd' ')" = "hbbr hbbs" ] || fail "COMPOSE_FILE in .env did not select the behind-proxy file"
  dc build > "$CI/build.log" 2>&1 || { tail -n 40 "$CI/build.log"; fail "image build (build/ci/build.log)"; }
  dc up -d
  wait_for 30 sudo test -s "$SRC/deploy/data/id_ed25519.pub" || fail "hbbs wrote no key"
  sleep 2
  ss -Hltnu '( sport >= :21115 and sport <= :21119 )' | awk '{print $1, $5}' | sort -u | tee "$CI/listen.txt"
  [ "$(wc -l < "$CI/listen.txt")" = 6 ] || fail "expected TCP 21115-21119 and UDP 21116"
  if grep -v ' 127\.0\.0\.1:' "$CI/listen.txt"; then fail "hbbs/hbbr listen beyond 127.0.0.1"; fi
  check_stack
  dc logs --no-log-prefix hbbr | grep -q 'TOTAL_BANDWIDTH: 100Mb/s' || fail "HJV_TOTAL_BANDWIDTH did not reach hbbr"
  dc logs --no-log-prefix hbbr | grep -q 'SINGLE_BANDWIDTH: 128Mb/s' || fail "an empty HJV_SINGLE_BANDWIDTH changed hbbr's default"

  step "deploy B: nginx with deploy/nginx-hjviewer.conf"
  local ng="$CI/nginx"
  rm -rf "$ng"
  mkdir -p "$ng/tmp"
  sed -e 's/DOMAIN/localhost/g' \
      -e "s#/etc/letsencrypt/live/localhost/fullchain.pem#$CERTS/cert.pem#" \
      -e "s#/etc/letsencrypt/live/localhost/privkey.pem#$CERTS/key.pem#" \
      -e 's/listen 443 ssl;/listen 127.0.0.1:18443 ssl;/' -e 's/listen 80;/listen 127.0.0.1:18080;/' \
      "$SRC/deploy/nginx-hjviewer.conf" > "$ng/site.conf"
  grep -q 'listen 127.0.0.1:18443 ssl;' "$ng/site.conf" || fail "nginx-hjviewer.conf: no 'listen 443 ssl;' line"
  cat > "$ng/nginx.conf" << EOF
pid $ng/nginx.pid;
error_log $ng/error.log info;
events {}
http {
  # as in Ubuntu's nginx.conf
  default_type application/octet-stream;
  access_log $ng/access.log;
  client_body_temp_path $ng/tmp; proxy_temp_path $ng/tmp; fastcgi_temp_path $ng/tmp;
  uwsgi_temp_path $ng/tmp; scgi_temp_path $ng/tmp;
  include $ng/site.conf;
}
EOF
  "$PKGS/root/usr/sbin/nginx" -e "$ng/error.log" -p "$ng" -c "$ng/nginx.conf" -t || fail "nginx -t"
  "$PKGS/root/usr/sbin/nginx" -e "$ng/error.log" -p "$ng" -c "$ng/nginx.conf" -g 'daemon off;' > "$ng/out.log" 2>&1 &
  NGINX_PID=$!
  wait_for 10 curl -s -o /dev/null --cacert "$CERTS/cert.pem" https://localhost:18443/ || fail "nginx did not start"
  check_site 18443 "$CERTS/cert.pem" 200 nginx
  [ "$(curl -s -o /dev/null -w '%{http_code} %{redirect_url}' http://localhost:18080/x)" = "301 https://localhost/x" ] ||
    fail "nginx: port 80 does not redirect to https"
  if grep -E '\[(emerg|alert|crit|error)\]' "$ng/error.log"; then fail "nginx logged errors"; fi
  stop_web

  step "deploy B: Apache with deploy/apache-hjviewer.conf"
  local ap="$CI/apache" m
  rm -rf "$ap"
  mkdir -p "$ap"
  sed -e 's/DOMAIN/localhost/g' \
      -e "s#/etc/letsencrypt/live/localhost/fullchain.pem#$CERTS/cert.pem#" \
      -e "s#/etc/letsencrypt/live/localhost/privkey.pem#$CERTS/key.pem#" \
      -e 's/<VirtualHost \*:443>/<VirtualHost *:18444>/' -e 's/<VirtualHost \*:80>/<VirtualHost *:18081>/' \
      "$SRC/deploy/apache-hjviewer.conf" > "$ap/site.conf"
  grep -q '<VirtualHost \*:18444>' "$ap/site.conf" || fail "apache-hjviewer.conf: no '<VirtualHost *:443>'"
  {
    echo "ServerRoot \"$ap\""
    echo "DefaultRuntimeDir \"$ap\""
    echo "PidFile \"$ap/apache.pid\""
    echo "Mutex file:$ap"
    echo "Listen 127.0.0.1:18444"
    echo "Listen 127.0.0.1:18081"
    echo "ErrorLog \"$ap/error.log\""
    echo "LogLevel warn"
    echo "ServerName localhost"
    echo "Timeout 300"   # Ubuntu's apache2.conf
    for m in mpm_event authz_core socache_shmcb ssl headers proxy proxy_http proxy_wstunnel alias; do
      echo "LoadModule ${m}_module $PKGS/root/usr/lib/apache2/modules/mod_$m.so"
    done
    echo "Include \"$ap/site.conf\""
  } > "$ap/apache.conf"
  export LD_LIBRARY_PATH="$PKGS/root/usr/lib/x86_64-linux-gnu"
  "$PKGS/root/usr/sbin/apache2" -t -f "$ap/apache.conf" || fail "apache2 -t"
  "$PKGS/root/usr/sbin/apache2" -f "$ap/apache.conf" -DFOREGROUND > "$ap/out.log" 2>&1 &
  APACHE_PID=$!
  unset LD_LIBRARY_PATH
  wait_for 10 curl -s -o /dev/null --cacert "$CERTS/cert.pem" https://localhost:18444/ || fail "Apache did not start"
  check_site 18444 "$CERTS/cert.pem" 404 Apache
  [ "$(curl -s -o /dev/null -w '%{http_code} %{redirect_url}' http://localhost:18081/x)" = "301 https://localhost/x" ] ||
    fail "Apache: port 80 does not redirect to https"
  # "client denied by server configuration" would feed fail2ban's apache-auth filter
  if grep -E 'AH01630|:(error|crit|alert|emerg)\]' "$ap/error.log"; then fail "Apache logged errors"; fi
  stop_web

  step "deploy A: Caddy (docker-compose.yml) with the same image and key, on 127.0.0.1:18082/18445"
  dc down
  cat > "$SRC/deploy/docker-compose.ci.yml" << 'EOF'
# scripts/ci.sh: Caddy on high ports of 127.0.0.1 instead of 80/443
services:
  caddy:
    ports: !override
      - "127.0.0.1:18082:80"
      - "127.0.0.1:18445:443"
EOF
  sed -i '/^COMPOSE_FILE=/d' "$SRC/deploy/.env"
  echo 'COMPOSE_FILE=docker-compose.yml:docker-compose.ci.yml' >> "$SRC/deploy/.env"
  dc up -d
  # Caddy issues "localhost" from its own CA; the checks trust that CA's root
  wait_for 60 dc exec -T caddy test -s /data/caddy/pki/authorities/local/root.crt || fail "Caddy made no CA"
  dc exec -T caddy cat /data/caddy/pki/authorities/local/root.crt > "$CI/caddy-root.crt"
  wait_for 30 curl -s -o /dev/null --cacert "$CI/caddy-root.crt" https://localhost:18445/ || fail "Caddy does not serve https"
  check_stack
  check_site 18445 "$CI/caddy-root.crt" 200 Caddy

  cleanup
  trap - EXIT
}

case "${1:-all}" in
  server) server_stage ;;
  deploy) deploy_stage ;;
  all) server_stage; deploy_stage ;;
  *) echo "usage: $0 [all|server|deploy]" >&2; exit 2 ;;
esac
step "CI passed: ${1:-all}"

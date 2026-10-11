#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# CI run on the development machine (GitHub CI is paused; docs/ci.md).
#   scripts/ci.sh [all]    the server and deploy stages
#   scripts/ci.sh server   a fresh upstream checkout with server/patches, the signaling and relay
#                          tests, hjv-probe and hjv-load
#   scripts/ci.sh client   a fresh upstream checkout with client/patches, hbb_common's WebSocket
#                          tests and the client's pure Dart tests, with the Flutter SDK the app
#                          builds use (downloaded once, checked against Flutter's release index).
#                          Tests that need the generated bridge run in client-build.yml.
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
NGINX_PID="" APACHE_PID="" ECHO_PID=""
# the stack under test: the compose files (docker) or the release package's systemd units
STACK=docker SYSTEMD_SINCE=""

step() { printf '\n== %s\n' "$*"; }
dc() { (cd "$SRC/deploy" && sudo docker compose -p "$PROJECT" "$@"); }
hbbs_logs() {
  if [ "$STACK" = systemd ]; then
    sudo journalctl -q -u hjv-ci-hbbs --since "$SYSTEMD_SINCE" -o cat
  else
    dc logs --no-log-prefix hbbs
  fi
}
key_file() {
  if [ "$STACK" = systemd ]; then echo /var/lib/hjv-ci/id_ed25519.pub; else echo "$SRC/deploy/data/id_ed25519.pub"; fi
}

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
  local pid
  # a process ended by the signal has status 143, which must not end this script under set -e
  for pid in $NGINX_PID $APACHE_PID $ECHO_PID; do
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  done
  NGINX_PID="" APACHE_PID="" ECHO_PID=""
}

cleanup_systemd() {
  if [ -e /run/systemd/system/hjv-ci-hbbs.service ] || [ -e /run/systemd/system/hjv-ci-hbbr.service ]; then
    sudo systemctl stop hjv-ci-hbbr hjv-ci-hbbs 2> /dev/null || true
    sudo rm -f /run/systemd/system/hjv-ci-hbbs.service /run/systemd/system/hjv-ci-hbbr.service
    sudo systemctl daemon-reload
    sudo systemctl reset-failed hjv-ci-hbbs hjv-ci-hbbr 2> /dev/null || true
  fi
  sudo rm -rf /var/lib/private/hjv-ci /run/hjv-ci /usr/local/lib/hjv-ci
  sudo rm -f /var/lib/hjv-ci
}

cleanup() {
  stop_web
  cleanup_systemd
  # only this script's own project: its containers, network and Caddy volume
  [ -d "$SRC/deploy" ] && dc down -v --remove-orphans > /dev/null 2>&1
  return 0
}

wait_for() { # <seconds> <command...>
  local n=$1; shift
  for _ in $(seq "$n"); do "$@" > /dev/null 2>&1 && return 0; sleep 1; done
  return 1
}

# A server on 127.0.0.1:18599 that answers with the address headers it got, for check_headers
start_echo() {
  python3 -I -c '
import http.server
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        body = "".join("%s: %s\n" % (k, self.headers.get(k, "")) for k in ("x-real-ip", "x-real-port", "x-forwarded-for")).encode()
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *args):
        pass
http.server.HTTPServer(("127.0.0.1", 18599), H).serve_forever()
' &
  ECHO_PID=$!
  wait_for 10 curl -s -o /dev/null http://127.0.0.1:18599/ || fail "the header echo server did not start"
}

# hbbs and hbbr trust X-Real-IP / X-Real-Port / X-Forwarded-For: the web server must replace what a
# client sends. <port> serves the HJViewer locations in front of start_echo's server.
check_headers() { # <port> <name>
  local port=$1 name=$2 path got
  step "deploy: through $name, a client's own X-Real-IP / X-Forwarded-For / X-Real-Port never reach hbbs or hbbr"
  for path in /ws/id /ws/relay; do
    got=$(curl -s --cacert "$CERTS/cert.pem" -H 'X-Real-IP: 203.0.113.66' -H 'X-Forwarded-For: 203.0.113.66' \
      -H 'X-Real-Port: 1' "https://localhost:$port$path" | tr '\n' ' ')
    echo "$path -> $got"
    case "$got" in
      "x-real-ip: 127.0.0.1 x-real-port: "[1-9]*" x-forwarded-for: 127.0.0.1 ") ;;
      *) fail "$name passes client-chosen address headers on $path: $got" ;;
    esac
    case "$got" in *"x-real-port: 1 "*) fail "$name passes the client's X-Real-Port on $path" ;; esac
  done
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

FLUTTER_VERSION=3.24.5 # client-build.yml FLUTTER_VERSION
# The pure Dart tests of the client patches, which run with the bridge's web stand-in. The gesture
# and quality monitor tests need the generated bridge and run in client-build.yml.
CLIENT_DART_TESTS="test/soft_keyboard_input_test.dart test/soft_keyboard_log_test.dart"

client_stage() {
  local fl="$CI/flutter-$FLUTTER_VERSION" app="$CI/upstream/rustdesk/flutter" sum t tests=()
  local index=https://storage.googleapis.com/flutter_infra_release/releases
  if [ ! -x "$fl/bin/flutter" ]; then
    step "client: Flutter $FLUTTER_VERSION, checked against releases_linux.json"
    curl -fsSL "$index/releases_linux.json" -o "$CI/releases_linux.json"
    sum=$(python3 -I -c 'import json, sys
for r in json.load(open(sys.argv[1]))["releases"]:
    if r["version"] == sys.argv[2] and r["channel"] == "stable":
        print(r["sha256"]); break' "$CI/releases_linux.json" "$FLUTTER_VERSION")
    [ -n "$sum" ] || fail "Flutter $FLUTTER_VERSION is not in releases_linux.json"
    curl -fsSL "$index/stable/linux/flutter_linux_$FLUTTER_VERSION-stable.tar.xz" -o "$CI/flutter.tar.xz"
    echo "$sum  $CI/flutter.tar.xz" | sha256sum -c --quiet - || fail "the Flutter archive does not match releases_linux.json"
    rm -rf "$fl.tmp" && mkdir -p "$fl.tmp" && tar -xJf "$CI/flutter.tar.xz" -C "$fl.tmp"
    mv "$fl.tmp/flutter" "$fl" && rm -rf "$fl.tmp" "$CI/flutter.tar.xz"
  fi
  step "client: upstream checkout with client/patches"
  rm -rf "$CI/upstream/rustdesk"
  UPSTREAM_DIR="$CI/upstream" "$ROOT/scripts/fetch-upstream.sh" client
  step "client: hbb_common WebSocket tests"
  (cd "$CI/upstream/rustdesk" && CARGO_TARGET_DIR="$CI/target-client" cargo test --locked -p hbb_common --lib websocket)
  step "client: pure Dart tests"
  for t in $CLIENT_DART_TESTS; do
    [ -f "$app/$t" ] || fail "client: $t is not in the patched tree"
    tests+=("$t")
  done
  # The bridge is generated in client-build.yml; these tests only need its web stand-in to resolve.
  printf "export 'web/bridge.dart';\n" > "$app/lib/generated_bridge.dart"
  (cd "$app" && export PUB_CACHE="$CI/pub-cache" FLUTTER_SUPPRESS_ANALYTICS=true &&
    { "$fl/bin/flutter" pub get > "$CI/pub-get.log" 2>&1 || { tail -n 20 "$CI/pub-get.log"; fail "client: flutter pub get"; }; } &&
    "$fl/bin/flutter" test --no-pub "${tests[@]}") || fail "client: Dart tests"
}

# The runbook's checks against one web server: WebSocket upgrades to hbbs and hbbr, the /api/
# answer, the source notice, hjv-probe, three PCs registering (hbbs must see their ports), a relay
# session with the server key, and the same key in hbbs, hbbr and data/id_ed25519.pub.
check_site() { # <port> <CA file> <status of />, or "site" when / belongs to another site <name>
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
  if [ "$root_status" != site ]; then
    code=$(curl -s -o "$CI/root.txt" -w '%{http_code}' "${curl_tls[@]}" "$url/" || true)
    echo "/ $code $(head -c 80 "$CI/root.txt")"
    if [ "$code" != "$root_status" ] || ! grep -q 'github.com/yonsei-projecttf/hjviewer' "$CI/root.txt"; then
      fail "$name: / answered $code without the source notice"
    fi
  fi
  "$PROBE" localhost --port "$port" --ca "$ca" || fail "$name: hjv-probe"
  before=$(hbbs_logs 2>/dev/null | grep -c 'update_pk ' || true)
  "$LOAD" peers localhost --port "$port" --ca "$ca" -n 3 --secs 12 || fail "$name: hjv-load peers"
  { hbbs_logs | grep -oE 'update_pk [0-9]+ [^ ]+' || true; } | tail -n "+$((before + 1))" > "$CI/pk.txt"
  cat "$CI/pk.txt"
  [ "$(wc -l < "$CI/pk.txt")" = 3 ] || fail "$name: hbbs did not register 3 PCs"
  if grep -q ':0$' "$CI/pk.txt"; then fail "$name: X-Real-Port did not reach hbbs (port 0)"; fi
  "$LOAD" relay localhost --port "$port" --ca "$ca" -n 1 --mbps 2 --secs 5 --key "$(sudo cat "$(key_file)")" ||
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

check_systemd() { # keys, file permissions, sandbox, settings of the systemd units
  local key s k
  key=$(sudo cat /var/lib/hjv-ci/id_ed25519.pub)
  for s in hbbs hbbr; do
    k=$({ sudo journalctl -q -u "hjv-ci-$s" --since "$SYSTEMD_SINCE" -o cat | grep -o 'Key: [A-Za-z0-9+/=]*' || true; } | tail -1 | cut -d' ' -f2)
    echo "$s key $k"
    [ "$k" = "$key" ] || fail "systemd: $s runs with a key other than id_ed25519.pub"
    [ "$(systemctl show -p NoNewPrivileges --value "hjv-ci-$s")" = yes ] || fail "systemd: $s may gain privileges"
    [ -z "$(systemctl show -p CapabilityBoundingSet --value "hjv-ci-$s")" ] || fail "systemd: $s keeps capabilities"
  done
  [ "$(sudo stat -c '%a' /var/lib/private/hjv-ci)" = 700 ] || fail "systemd: the state directory is not 700"
  [ "$(sudo stat -c '%a' /var/lib/hjv-ci/id_ed25519)" = 600 ] || fail "systemd: id_ed25519 is not created 600"
  if sudo journalctl -q -u 'hjv-ci-*' --since "$SYSTEMD_SINCE" -o cat | grep -E 'Failed to store|panicked|Permission denied|Operation not permitted'; then
    fail "systemd: the sandbox gets in the way (journal above)"
  fi
  sudo journalctl -q -u hjv-ci-hbbr --since "$SYSTEMD_SINCE" -o cat | grep -q 'TOTAL_BANDWIDTH: 100Mb/s' ||
    fail "systemd: TOTAL_BANDWIDTH from the env file did not reach hbbr"
}

deploy_stage() {
  if [ ! -x "$PROBE" ] || [ ! -x "$LOAD" ]; then fail "hjv-probe/hjv-load are not built: scripts/ci.sh server"; fi
  sudo docker compose version > /dev/null 2>&1 || fail "Docker with compose v2 is needed (docs/server-runbook.md 3-1)"
  [ -z "$(ss -Hltnu '( sport >= :21115 and sport <= :21119 )')" ] ||
    fail "ports 21115-21119 are in use (a running hjviewer server?); the test stack needs them"
  [ -z "$(ss -Hltn '( sport >= :18080 and sport <= :18082 ) or ( sport >= :18443 and sport <= :18448 ) or sport = :18599')" ] ||
    fail "test ports 18080-18082 / 18443-18448 / 18599 are in use"
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

  step "deploy B: nginx with deploy/nginx-hjviewer.conf, and added to an existing site"
  local ng="$CI/nginx"
  rm -rf "$ng"
  mkdir -p "$ng/tmp" "$ng/snippets"
  cp "$SRC/deploy/nginx-hjviewer-locations.conf" "$ng/snippets/hjviewer-locations.conf"
  sed -e 's/DOMAIN/localhost/g' \
      -e "s#/etc/letsencrypt/live/localhost/fullchain.pem#$CERTS/cert.pem#" \
      -e "s#/etc/letsencrypt/live/localhost/privkey.pem#$CERTS/key.pem#" \
      -e 's/listen 443 ssl;/listen 127.0.0.1:18443 ssl;/' -e 's/listen 80;/listen 127.0.0.1:18080;/' \
      "$SRC/deploy/nginx-hjviewer.conf" > "$ng/site.conf"
  grep -q 'listen 127.0.0.1:18443 ssl;' "$ng/site.conf" || fail "nginx-hjviewer.conf: no 'listen 443 ssl;' line"
  # An existing site that shares the domain, like a login-only WordPress: HTTP/2 on the listener,
  # every path sent to its login page, and HJViewer added with the runbook's one include line
  cat > "$ng/shared.conf" << EOF
server {
    listen 127.0.0.1:18446 ssl http2;
    server_name localhost;
    ssl_certificate     $CERTS/cert.pem;
    ssl_certificate_key $CERTS/key.pem;
    include snippets/hjviewer-locations.conf;
    location / {
        return 302 https://\$host/login/?redirect_to=\$request_uri;
    }
    location ~ \.php\$ {
        return 302 https://\$host/login/;
    }
}
EOF
  # The same locations in front of a server that echoes the headers it gets, to check that a
  # client cannot choose the address hbbs/hbbr trust
  sed 's#http://127\.0\.0\.1:2111[89]#http://127.0.0.1:18599#' "$SRC/deploy/nginx-hjviewer-locations.conf" \
    > "$ng/snippets/hjviewer-locations-echo.conf"
  cat > "$ng/echo.conf" << EOF
server {
    listen 127.0.0.1:18447 ssl http2;
    server_name localhost;
    ssl_certificate     $CERTS/cert.pem;
    ssl_certificate_key $CERTS/key.pem;
    include snippets/hjviewer-locations-echo.conf;
}
EOF
  start_echo
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
  include $ng/shared.conf;
  include $ng/echo.conf;
}
EOF
  "$PKGS/root/usr/sbin/nginx" -e "$ng/error.log" -p "$ng" -c "$ng/nginx.conf" -t || fail "nginx -t"
  "$PKGS/root/usr/sbin/nginx" -e "$ng/error.log" -p "$ng" -c "$ng/nginx.conf" -g 'daemon off;' > "$ng/out.log" 2>&1 &
  NGINX_PID=$!
  wait_for 10 curl -s -o /dev/null --cacert "$CERTS/cert.pem" https://localhost:18443/ || fail "nginx did not start"
  check_site 18443 "$CERTS/cert.pem" 200 nginx
  [ "$(curl -s -o /dev/null -w '%{http_code} %{redirect_url}' http://localhost:18080/x)" = "301 https://localhost/x" ] ||
    fail "nginx: port 80 does not redirect to https"
  check_site 18446 "$CERTS/cert.pem" site "nginx, existing site with HTTP/2"
  # the existing site still answers everything else, over HTTP/2 for browsers
  [ "$(curl -s -o /dev/null -w '%{http_version} %{http_code} %{redirect_url}' --http2 --cacert "$CERTS/cert.pem" https://localhost:18446/)" = \
    "2 302 https://localhost/login/?redirect_to=/" ] || fail "nginx: the existing site's / changed"
  [ "$(curl -s -o /dev/null -w '%{http_code}' --cacert "$CERTS/cert.pem" https://localhost:18446/wp-json/)" = 302 ] ||
    fail "nginx: the existing site's other paths changed"
  check_headers 18447 nginx
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
  # the 443 VirtualHost again, in front of the header echo server (see the nginx check)
  awk '/<VirtualHost \*:18444>/{on=1} on{print} on&&/<\/VirtualHost>/{exit}' "$ap/site.conf" |
    sed -e 's/<VirtualHost \*:18444>/<VirtualHost *:18448>/' -e 's#ws://127\.0\.0\.1:2111[89]#ws://127.0.0.1:18599#' > "$ap/echo.conf"
  {
    echo "ServerRoot \"$ap\""
    echo "DefaultRuntimeDir \"$ap\""
    echo "PidFile \"$ap/apache.pid\""
    echo "Mutex file:$ap"
    echo "Listen 127.0.0.1:18444"
    echo "Listen 127.0.0.1:18081"
    echo "Listen 127.0.0.1:18448"
    echo "ErrorLog \"$ap/error.log\""
    echo "LogLevel warn"
    echo "ServerName localhost"
    echo "Timeout 300"   # Ubuntu's apache2.conf
    for m in mpm_event authz_core socache_shmcb ssl headers proxy proxy_http proxy_wstunnel alias; do
      echo "LoadModule ${m}_module $PKGS/root/usr/lib/apache2/modules/mod_$m.so"
    done
    echo "Include \"$ap/site.conf\""
    echo "Include \"$ap/echo.conf\""
  } > "$ap/apache.conf"
  export LD_LIBRARY_PATH="$PKGS/root/usr/lib/x86_64-linux-gnu"
  "$PKGS/root/usr/sbin/apache2" -t -f "$ap/apache.conf" || fail "apache2 -t"
  "$PKGS/root/usr/sbin/apache2" -f "$ap/apache.conf" -DFOREGROUND > "$ap/out.log" 2>&1 &
  APACHE_PID=$!
  unset LD_LIBRARY_PATH
  wait_for 10 curl -s -o /dev/null --cacert "$CERTS/cert.pem" https://localhost:18444/ || fail "Apache did not start"
  check_site 18444 "$CERTS/cert.pem" 404 Apache
  start_echo
  check_headers 18448 Apache
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

  step "deploy B, systemd: the release package (scripts/package-server.sh) without Docker"
  dc down
  local rel="$CI/release" pkg unit f
  rm -rf "$rel"
  HJV_SKIP_SOURCE_CHECK=1 HJV_SERVER_BIN_DIR="$CI/target/release" "$ROOT/scripts/package-server.sh" ci "$rel" > "$CI/package.log" 2>&1 ||
    { tail -n 30 "$CI/package.log"; fail "scripts/package-server.sh"; }
  tar -xzf "$rel/hjviewer-server-ci-linux-x86_64.tar.gz" -C "$rel"
  pkg="$rel/hjviewer-server-ci-linux-x86_64"
  for f in bin/hbbs bin/hbbr systemd/hjviewer-hbbs.service systemd/hjviewer-hbbr.service systemd/hjviewer.env.example \
    web/nginx-hjviewer-locations.conf web/nginx-hjviewer.conf web/apache-hjviewer.conf \
    LICENSE.txt NOTICE.txt SOURCE.txt THIRD_PARTY_NOTICES.txt; do
    [ -s "$pkg/$f" ] || fail "release package: no $f"
  done
  grep -q 'server/patches' "$pkg/SOURCE.txt" || fail "release package: SOURCE.txt does not describe the server build"
  # Installed as the runbook does, apart from the names that keep it away from a real installation:
  # runtime units hjv-ci-* (in /run, gone at reboot), a dynamic user and directories of its own. The
  # sandbox, limits and commands are those of the shipped units.
  sudo install -d -m 755 /usr/local/lib/hjv-ci/bin /run/hjv-ci
  sudo install -m 755 "$pkg/bin/hbbs" "$pkg/bin/hbbr" /usr/local/lib/hjv-ci/bin/
  sed -e 's/^HJV_DOMAIN=.*/HJV_DOMAIN=localhost/' -e 's/^TOTAL_BANDWIDTH=.*/TOTAL_BANDWIDTH=100/' \
    "$pkg/systemd/hjviewer.env.example" | sudo tee /run/hjv-ci/hjviewer.env > /dev/null
  for unit in hbbs hbbr; do
    sed -e 's/^User=hjviewer$/DynamicUser=yes\nUser=hjv-ci/' -e '/^Group=hjviewer$/d' \
      -e 's#/var/lib/hjviewer#/var/lib/hjv-ci#g' -e 's/^StateDirectory=hjviewer$/StateDirectory=hjv-ci/' \
      -e 's#/opt/hjviewer/bin/#/usr/local/lib/hjv-ci/bin/#' -e 's#/etc/hjviewer/hjviewer.env#/run/hjv-ci/hjviewer.env#' \
      -e 's/hjviewer-hbbs\.service/hjv-ci-hbbs.service/g' \
      "$pkg/systemd/hjviewer-$unit.service" | sudo tee "/run/systemd/system/hjv-ci-$unit.service" > /dev/null
  done
  STACK=systemd
  SYSTEMD_SINCE=$(date '+%Y-%m-%d %H:%M:%S')
  sudo systemctl daemon-reload
  sudo systemctl start hjv-ci-hbbs hjv-ci-hbbr || fail "systemd: the units did not start"
  wait_for 15 sudo test -s /var/lib/hjv-ci/id_ed25519.pub || fail "systemd: hbbs wrote no key"
  wait_for 15 systemctl is-active --quiet hjv-ci-hbbr || fail "systemd: hbbr is not running"
  sleep 2
  ss -Hltnu '( sport >= :21115 and sport <= :21119 )' | awk '{print $1, $5}' | sort -u | tee "$CI/listen.txt"
  [ "$(wc -l < "$CI/listen.txt")" = 6 ] || fail "systemd: expected TCP 21115-21119 and UDP 21116"
  if grep -v ' 127\.0\.0\.1:' "$CI/listen.txt"; then fail "systemd: hbbs/hbbr listen beyond 127.0.0.1"; fi
  check_systemd
  "$PKGS/root/usr/sbin/nginx" -e "$ng/error.log" -p "$ng" -c "$ng/nginx.conf" -g 'daemon off;' > "$ng/out.log" 2>&1 &
  NGINX_PID=$!
  wait_for 10 curl -s -o /dev/null --cacert "$CERTS/cert.pem" https://localhost:18446/ || fail "nginx did not start"
  check_site 18446 "$CERTS/cert.pem" site "nginx, existing site with HTTP/2, to the systemd units"
  stop_web
  STACK=docker

  cleanup
  trap - EXIT
}

case "${1:-all}" in
  server) server_stage ;;
  client) client_stage ;;
  deploy) deploy_stage ;;
  all) server_stage; deploy_stage ;;
  *) echo "usage: $0 [all|server|client|deploy]" >&2; exit 2 ;;
esac
step "CI passed: ${1:-all}"

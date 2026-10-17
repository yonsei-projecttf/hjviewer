#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# CI run on the development machine (GitHub CI is paused; docs/ci.md).
#   scripts/ci.sh [all]    the server and deploy stages only (prepush runs every stage)
#   scripts/ci.sh prepush [<stage>...]
#                          the gate before a push (design v3 11 step 1): on a clean tree, with
#                          HJV_CI_STRICT=1, the stages server, client, deploy, e2e, wol and leak one
#                          after another, each with its own log (build/ci/prepush/<time>-<commit>/;
#                          first the commits, the toolchain and the start, last the exit status),
#                          stopping at the first that fails. With stages named, only those, in this
#                          order: after a fix, the stages that read what it changed (docs/ci.md).
#   scripts/ci.sh server   a fresh upstream checkout with server/patches and all of its tests in
#                          release (the library's and binaries' unit tests, the signaling, relay,
#                          protobuf, hbbs hjv-pk, lowest controller version and access ticket tests,
#                          against hjv-api's shared vectors), hjv-probe and
#                          hjv-load; hjv-api's tests and release build (server/api); hjv-relay's
#                          tests (relay/); tools/hjv-release's format, lints, tests (its shared test
#                          vectors among them) and release build
#   scripts/ci.sh client   the offline tests of the Google Play upload script and of the build-key
#                          check (scripts/test_*.py, the key files also against hjv-release), actionlint
#                          on the workflows when it is installed, then a fresh upstream checkout with
#                          client/patches, the release records' tests (scripts/release-info.py, on
#                          the patched tree and build/release-1.5.1), hbb_common's WebSocket tests,
#                          cargo +1.75.0 check (the app builds' toolchain), the client core's
#                          agreement with the shared vectors of tools/hjv-release (the trust rules)
#                          and the client's pure Dart tests, with the Flutter SDK the app builds use
#                          (downloaded once, checked against Flutter's release index). Tests that
#                          need the generated bridge run in client-build.yml.
#   scripts/ci.sh deploy   docs/server-runbook.md's deployment with Docker, as on the server, from a
#                          copy of this working tree: path B (hbbs/hbbr on 127.0.0.1) behind
#                          deploy/nginx-hjviewer.conf and deploy/apache-hjviewer.conf, then path A
#                          (Caddy). Everything listens on 127.0.0.1; ports 80/443 stay untouched.
#                          Then the release package (scripts/package-server.sh): its systemd units
#                          on this machine (hjv-api's too), the whole package installed in a clean
#                          Ubuntu container (systemd-analyze verify, nginx -t, requests through
#                          nginx to hjv-api), hjv-api's production settings
#                          (scripts/check-api-settings.py and its offline tests), and the account
#                          API's nginx limits (an IPv6 /64 and a dual-stack listener too, in an
#                          unprivileged network namespace). Before the Docker part, hjv-relay's
#                          packages (scripts/package-relay.sh, relay/tests/package.rs --ignored: a
#                          static build for x86_64-unknown-linux-musl, which needs that Rust target
#                          and musl-gcc).
#   scripts/ci.sh e2e      tools/e2e/firewall-e2e.sh strict and open (sudo: namespaces, iptables)
#                          with a client built for its test server, Caddy pinned by SHA-256, and the
#                          PC's client installed as /usr/local/lib/hjviewer-e2e/rustdesk; with the
#                          account code in the patches, also the unit tests of the client's
#                          HJViewer code (--lib hjv) in that build and the account scenarios
#                          (docs/e2e.md) against hjv-api and the fake SSO, with a 1.5.1 client
#                          built once from the tag client-1.5.1 and Ubuntu's libfaketime, unpacked
#                          under build/ci; with server patch 0005, hbbs's lowest controller
#                          version, and with the update code in the patches (src/hjviewer_update),
#                          a client with the update test keys and the signed version policy that
#                          tools/hjv-release signs for it (the minimum version's V1-V9), and the
#                          update code's unit tests once more in a build without those keys
#   scripts/ci.sh wol      tools/e2e/wol-e2e.sh, the Wake-on-LAN E2E (docs/e2e.md), without sudo: in
#                          this user's own namespaces a PC that sleeps is woken through hjv-api
#                          and hjv-relay, with the E2E's client, Caddy, hjv-api and fake SSO, and
#                          hjv-relay built for the test server with the E2E's command key; before
#                          it, the unit tests of the client's wake report (src/hjviewer_wol)
#   scripts/ci.sh leak     the push gate (design v3 11 steps 1-2): no value of the local list
#                          build/leak-patterns.txt in tracked files, unpushed commits or staged
#                          changes (scripts/leak-check.py)
# A stage skips, with a line "skipped: ..." saying so, what needs a work package that is not in the
# tree yet or a tool this machine lacks. Every skip is counted, those that the E2E scripts and the
# other checks print too, and the last line says how many: "CI passed: <stage> (<n> skipped)". With
# HJV_CI_STRICT=1 (prepush) a run with a skip fails at its end with the list: a pass with skipped
# lines is no pass before a push. hjv-api's and hjv-relay's tests run with the output of the tests
# that pass, so that a test that skips itself is counted too. A run that skipped nothing fails
# when an E2E has fewer PASS lines, or a client unit test filter fewer passed tests, than the last
# full run (E2E_LEAST_*, WOL_LEAST, UNIT_LEAST below).
# One run at a time on a CI directory (HJV_CI_DIR, default build/ci): a run holds the lock
# <CI directory>/ci.lock, and a run started meanwhile stops at once.
# Needs cargo, curl, openssl, Docker with compose v2 (used through sudo, as the runbook does), and
# apt-get for Ubuntu's nginx and Apache packages, which are unpacked under build/ci, not installed.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SELF="$ROOT/scripts/$(basename "$0")"
CI="${HJV_CI_DIR:-$ROOT/build/ci}"
mkdir -p "$CI"
# The runs on a CI directory share its checkouts, builds and E2E folders. flock holds the lock for
# as long as the run lasts and keeps it from the programs the run starts (-o), so that nothing the
# run leaves behind holds it; prepush's stages run under prepush's lock (HJV_CI_LOCK).
if [ "${HJV_CI_LOCK:-}" != "$CI" ]; then
  flock -n "$CI/ci.lock" true || { echo "FAIL: another scripts/ci.sh run holds $CI/ci.lock" >&2; exit 1; }
  exec env HJV_CI_LOCK="$CI" flock -n -o "$CI/ci.lock" "$SELF" "$@"
fi
TARGET=${1:-all}
PROJECT=hjv-ci                     # compose project of the test stack, apart from a real deployment
PROBE="$ROOT/tools/hjv-probe/target/release/hjv-probe"
LOAD="$ROOT/tools/hjv-load/target/release/hjv-load"
API="$CI/target-api/release/hjv-api"  # the server stage's release build of hjv-api
# Caddy for the E2E: the release archive, pinned by its SHA-256 (taken when the release's SHA-512
# list was checked, 2026-10-12)
CADDY_VERSION=2.11.7
CADDY_SHA256=727b91701a392de6ebc5027509f548bf39979e5216340d0faed8fa5e69c84f8b
# where the E2E installs the PC's client: Linux counts only programs under /usr as installed, and
# hjv-api adds only installed PCs to an account (design v3 7-4)
E2E_PC_DIR=/usr/local/lib/hjviewer-e2e
# The E2E's clients are test builds (feature hjv-e2e) of a fresh checkout, whose crates cargo
# compiles anew each time. Without link-time optimization and in 16 units the client and its unit
# tests take under a minute each once their dependencies are built, instead of three and a half
# minutes each with the release profile as it is.
E2E_PROFILE=(CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16)
# the 1.5.1 client the account E2E compares with (X2, X3)
E2E_OLD_TAG=client-1.5.1
# the stages of prepush, in their order
PREPUSH_STAGES="server client deploy e2e wol leak"
# The fewest PASS lines of the E2E scripts and the fewest client unit tests that the last full run
# passed with (2026-10-13). A run that skipped nothing and has fewer lost a scenario or a test (one
# moved under a condition that does not hold, a test filter that picks nothing), and the stage
# fails (at_least). Raise them as scenarios and tests come; lower one only with what was taken out.
E2E_LEAST_STRICT=60 E2E_LEAST_OPEN=2 WOL_LEAST=20
# strict: and the open login's S28-S31 (2026-10-14)
E2E_LEAST_STRICT=$((E2E_LEAST_STRICT + 4))
# strict: and the minimum version's V1-V9 (2026-10-14)
E2E_LEAST_STRICT=$((E2E_LEAST_STRICT + 12))
# strict: and HJViewer's ID rule's S32 (2026-10-14)
E2E_LEAST_STRICT=$((E2E_LEAST_STRICT + 1))
# strict: and the staff-only gate's A1-A14 (2026-10-17)
E2E_LEAST_STRICT=$((E2E_LEAST_STRICT + 14))
# by the filter of build_e2e_client's cargo test --lib <filter>; hjv as the staff-only gate's E2E
# counted it (260, 2026-10-17, the series with src/hjviewer_access)
declare -A UNIT_LEAST=([hjv]=260 [hjviewer_wol]=6)
# and the update code's tests in the e2e stage's build without the update keys (2026-10-14)
UNIT_LEAST[hjviewer_update-nokeys]=141
# and the cases of the shared vectors the client core runs in the client stage (tools/hjv-release,
# 2026-10-14)
VECTORS_LEAST=141
export PATH="$HOME/.cargo/bin:$PATH"

SRC="$CI/src" PKGS="$CI/pkgs" CERTS="$CI/certs"
NGINX_PID="" APACHE_PID="" ECHO_PID=""
# set by the deploy stage, whose failures show its containers' and web servers' logs
DEPLOY_LOGS=""
# set by build_e2e_client when the E2E has its account part
E2E_ACCOUNT=""
# and when its client has the update test keys (the minimum version's policy)
E2E_KEYED=""
# set by e2e_minimum_version
E2E_MINVER=()
# a client given by hand (E2E_CLIENT=<binary>, the wol stage only), and the E2E's client in use
E2E_CLIENT_GIVEN=${E2E_CLIENT:-} E2E_CLIENT=""
# the stack under test: the compose files (docker) or the release package's systemd units
STACK=docker SYSTEMD_SINCE=""
# what this run skipped (skip, count_skips)
SKIPPED=()

step() { printf '\n== %s\n' "$*"; }
# Every skip goes through skip, or through count_skips for the skipped lines another program prints
skip() { # <what is skipped (why)>
  echo "skipped: $*"
  SKIPPED+=("$*")
}
count_skips() { # <file>: its lines that start with "skipped: "
  local line
  while IFS= read -r line; do SKIPPED+=("${line#skipped: }"); done < <(grep -a '^skipped: ' "$1" || true)
}
# Runs <command...> with its output in <file>, shown as it comes, and counts its skipped lines. It
# writes to a file, not to a pipe that a process it leaves behind could keep open; started as a job,
# it gets back SIGINT and SIGQUIT, which the background jobs of a script ignore.
with_skips() { # <file> <command...>
  local log=$1 pid status=0
  shift
  : > "$log"
  env --default-signal=INT,QUIT "$@" < /dev/null > "$log" 2>&1 &
  pid=$!
  tail -n +1 -f --pid="$pid" "$log" || true
  wait "$pid" || status=$?
  count_skips "$log"
  return "$status"
}
# A count against its least (E2E_LEAST_*, WOL_LEAST, UNIT_LEAST): fewer fails the stage when this run
# skipped nothing. A run that skipped a part may pass with fewer, and with HJV_CI_STRICT=1 it fails
# at its end anyway.
at_least() { # <what> <count> <least>
  local n=$2
  [[ $n =~ ^[0-9]+$ ]] || n=0
  echo "$1: $n (at least $3)"
  [ "$n" -lt "$3" ] || return 0
  if [ ${#SKIPPED[@]} -gt 0 ]; then
    echo "$1: fewer than $3, in a run that skipped ${#SKIPPED[@]} part(s)"
  else
    fail "$1: $n, fewer than the $3 of the last full run: a scenario or a test went missing"
  fi
}
# The tests that passed by a cargo test log's "test result" lines, all of its test binaries
passed_tests() { # <file>
  sed -n -E 's/^test result: ok\. ([0-9]+) passed;.*/\1/p' "$1" | awk '{ n += $1 } END { print n + 0 }'
}
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
  if [ -n "$DEPLOY_LOGS" ] && [ -d "$SRC/deploy" ]; then
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
  if [ -e /run/systemd/system/hjv-ci-hbbs.service ] || [ -e /run/systemd/system/hjv-ci-hbbr.service ] ||
    [ -e /run/systemd/system/hjv-ci-api.service ] || [ -e /run/systemd/system/hjv-ci-api.socket ]; then
    sudo systemctl stop hjv-ci-api.service hjv-ci-api.socket hjv-ci-hbbr hjv-ci-hbbs 2> /dev/null || true
    sudo rm -f /run/systemd/system/hjv-ci-hbbs.service /run/systemd/system/hjv-ci-hbbr.service \
      /run/systemd/system/hjv-ci-api.service /run/systemd/system/hjv-ci-api.socket
    sudo systemctl daemon-reload
    sudo systemctl reset-failed hjv-ci-hbbs hjv-ci-hbbr hjv-ci-api.service hjv-ci-api.socket 2> /dev/null || true
  fi
  sudo rm -rf /var/lib/private/hjv-ci /var/lib/private/hjv-ci-api /var/log/private/hjv-ci-api /run/hjv-ci \
    /usr/local/lib/hjv-ci
  # stopping a socket unit leaves its socket file without RemoveOnStop=
  sudo rm -f /var/lib/hjv-ci /var/lib/hjv-ci-api /var/log/hjv-ci-api /run/hjv-ci-api.sock
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

# A server on 127.0.0.1:18599 that answers with the address headers it got, for check_headers, and
# for an /api/ path also with the path and the cookie, for check_account_proxy
start_echo() {
  python3 -I -c '
import http.server
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        body = "".join("%s: %s\n" % (k, self.headers.get(k, "")) for k in ("x-real-ip", "x-real-port", "x-forwarded-for"))
        if self.path.startswith("/api/"):
            body += "path: %s\ncookie: %s\n" % (self.path, self.headers.get("cookie", ""))
        body = body.encode()
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

# The account API's paths (design v3 5-8) through a web server whose hjv-api is start_echo's
# server: hjv-api counts logins per address from X-Real-IP, so the path and query must arrive as
# sent and the client's own address headers and the site's cookie must not; the rest of /api/
# keeps its 404.
check_account_proxy() { # <port> <name>
  local port=$1 name=$2 p got code
  step "deploy: through $name, the account paths reach hjv-api with the address $name saw"
  for p in '/api/peers?current=2&pageSize=100&accessible=&status=1' /api/login-options /api/hjv/keys/12; do
    got=$(curl -s --cacert "$CERTS/cert.pem" -H 'X-Real-IP: 203.0.113.66' -H 'X-Forwarded-For: 203.0.113.66' \
      -H 'Cookie: site=1' "https://localhost:$port$p" | tr '\n' ' ')
    echo "$p -> $got"
    [ "$got" = "x-real-ip: 127.0.0.1 x-real-port:  x-forwarded-for: 127.0.0.1 path: $p cookie:  " ] ||
      fail "$name does not pass $p on to hjv-api as it should"
  done
  for p in /api/heartbeat /api/loginx /api/hjv /api/peers/x; do
    code=$(curl -s -o /dev/null -w '%{http_code}' --cacert "$CERTS/cert.pem" "https://localhost:$port$p" || true)
    [ "$code" = 404 ] || fail "$name: $p answered $code, not 404"
  done
}

# With hjv-api stopped, an account path answers 503 with the text the app shows (JSON).
check_account_down() { # <port> <CA file> <name>
  local got
  got=$(curl -s -w ' %{http_code} %{content_type}' --cacert "$2" "https://localhost:$1/api/peers" || true)
  echo "/api/peers without hjv-api -> $got"
  case "$got" in
    '{"error":"HJViewer 서버를 지금 쓸 수 없습니다.'*'"} 503 application/json; charset=utf-8') ;;
    *) fail "$3: an account path without hjv-api does not answer the app's 503" ;;
  esac
}

# The vector that tests/hjv_pk.rs (server/patches/0004) checks the patched hbbs against is a
# copy of hjv-api's (server/api/src/hbbs.rs, reads_what_libsodium_signed): its four values, named.
hjv_pk_vector() { # <file>
  sed -n -E 's/^ *const (SERVER_SEED|SERVER_PK|DEVICE_PK|PEER_PK|SIGNED): &str = "([^"]*)";$/\1 \2/p' "$1" |
    sed 's/^DEVICE_PK /PEER_PK /'
}

server_stage() {
  # all of the patched server's tests, its upstream unit tests too: patches 0001-0004 change the
  # code they test
  local tests=(--lib --bins --tests) netns
  step "server: upstream checkout with server/patches"
  rm -rf "$CI/upstream/rustdesk-server"
  UPSTREAM_DIR="$CI/upstream" "$ROOT/scripts/fetch-upstream.sh" server
  echo "the patched tree: $(git -C "$CI/upstream/rustdesk-server" rev-parse 'HEAD^{tree}')"
  # hbbs's signed hjv-pk answer for hjv-api (server/patches/0004, design v3 5-7): required, so that
  # a dropped patch fails
  [ -f "$CI/upstream/rustdesk-server/tests/hjv_pk.rs" ] ||
    fail "server: tests/hjv_pk.rs is not in the patched tree (server/patches/0004)"
  # hjv-api reads what that test has hbbs answer only while both tests use one vector
  if [ -f "$ROOT/server/api/src/hbbs.rs" ]; then
    local vector
    vector=$(hjv_pk_vector "$ROOT/server/api/src/hbbs.rs")
    [ "$(printf '%s\n' "$vector" | grep -c .)" = 4 ] &&
      [ "$vector" = "$(hjv_pk_vector "$CI/upstream/rustdesk-server/tests/hjv_pk.rs")" ] ||
      fail "server: the vector of tests/hjv_pk.rs (server/patches/0004) is not hjv-api's (server/api/src/hbbs.rs)"
  fi
  # hbbs's access tickets (server/patches/0006, docs/access-tickets.md): required too; its tests read
  # a copy of hjv-api's shared vectors, which must be that very file (8), and its unit tests read
  # hjv-api's own through HJV_ACCESS_VECTORS against the SHA-256 they pin
  local vectors="$ROOT/server/api/tests/vectors/access-tickets.json"
  [ -f "$CI/upstream/rustdesk-server/tests/access.rs" ] ||
    fail "server: tests/access.rs is not in the patched tree (server/patches/0006)"
  cmp -s "$vectors" "$CI/upstream/rustdesk-server/tests/vectors/access-tickets.json" ||
    fail "server: hbbs's tests/vectors/access-tickets.json (server/patches) is not hjv-api's (server/api/tests/vectors)"
  step "server: hbbs and hbbr's tests (unit, signaling, relay, protobuf, hjv-pk, access tickets)"
  (cd "$CI/upstream/rustdesk-server" &&
    HJV_ACCESS_VECTORS="$vectors" CARGO_TARGET_DIR="$CI/target" cargo test --release --locked "${tests[@]}")
  step "server: hjv-probe and hjv-load"
  for t in hjv-probe hjv-load; do (cd "$ROOT/tools/$t" && cargo build --release --locked); done
  if [ -d "$ROOT/server/api" ]; then
    step "server: hjv-api tests and release build (server/api)"
    # with the output of the tests that pass: a test that cannot run here (no systemd tools) passes
    # with a line "skipped: ...", which only then is seen and counted
    with_skips "$CI/api-tests.log" env -C "$ROOT/server/api" CARGO_TARGET_DIR="$CI/target-api" \
      cargo test --locked -- --show-output || fail "server: hjv-api tests"
    (cd "$ROOT/server/api" && HJV_API_COMMIT=$(git -C "$ROOT" rev-parse HEAD) \
      CARGO_TARGET_DIR="$CI/target-api" cargo build --release --locked --bin hjv-api)
  else
    skip "hjv-api (no server/api)"
  fi
  if [ -d "$ROOT/relay" ]; then
    step "server: hjv-relay tests (relay/)"
    # Here the netns test fails instead of skipping when user and network namespaces are missing;
    # GitHub's runners (server.yml) restrict them, so it may skip there. HJV_RELAY_NETNS= lets it
    # skip anywhere. Its own line does not start with "skipped:", so that is counted here. The
    # tests run with the output of those that pass, as hjv-api's do: a test that cannot run with
    # this build passes with a line "skipped: ...".
    netns=require
    if [ -n "${GITHUB_ACTIONS:-}" ]; then netns=; fi
    netns=${HJV_RELAY_NETNS-$netns}
    [ "$netns" = require ] || skip "hjv-relay's network namespace test, where it cannot run (HJV_RELAY_NETNS=$netns)"
    with_skips "$CI/relay-tests.log" env -C "$ROOT/relay" HJV_RELAY_NETNS="$netns" CARGO_TARGET_DIR="$CI/target-relay" \
      cargo test --locked -- --show-output || fail "server: hjv-relay tests"
  else
    skip "hjv-relay (no relay/)"
  fi
  if [ -d "$ROOT/tools/hjv-release" ]; then
    # the operator's tool of the signed version policy: its tests hold the shared test vectors to
    # the reference model (python3 tests/vectors/generate.py --check) and read every one; the release
    # build is the one the client stage's key-file test and the E2E use
    step "server: hjv-release format, lints, tests and release build (tools/hjv-release)"
    (cd "$ROOT/tools/hjv-release" && cargo fmt --check) || fail "server: hjv-release is not formatted (cargo fmt)"
    (cd "$ROOT/tools/hjv-release" && CARGO_TARGET_DIR="$CI/target-hjv-release" cargo clippy --locked --all-targets -- -D warnings) ||
      fail "server: hjv-release lints"
    with_skips "$CI/hjv-release-tests.log" env -C "$ROOT/tools/hjv-release" CARGO_TARGET_DIR="$CI/target-hjv-release" \
      cargo test --locked || fail "server: hjv-release tests"
    (cd "$ROOT/tools/hjv-release" && CARGO_TARGET_DIR="$CI/target-hjv-release" cargo build --release --locked) ||
      fail "server: hjv-release release build"
  else
    skip "hjv-release (no tools/hjv-release)"
  fi
}

FLUTTER_VERSION=3.24.5 # client-build.yml FLUTTER_VERSION
# The pure Dart tests of the client patches, which run with the bridge's web stand-in. The gesture
# and quality monitor tests need the generated bridge and run in client-build.yml.
CLIENT_DART_TESTS="test/soft_keyboard_input_test.dart test/soft_keyboard_log_test.dart test/hjviewer_account_test.dart"
# the update screens' (minimum version design 6-4), the controller-only Android app's (Play design
# 3-3) and the super operator's view's pure tests (2026-10-14)
CLIENT_DART_TESTS+=" test/hjv_update_test.dart test/hjviewer_android_test.dart test/hjviewer_admin_test.dart"
# the access screens' (staff-only gate, docs/access-tickets.md 5-3; client patches 0061-0062)
CLIENT_DART_TESTS+=" test/hjviewer_access_test.dart"
# Pure Dart tests that a work package not in client/patches yet adds: run when the patched tree
# has them, else skipped with a note. Once the patch is in, move them to CLIENT_DART_TESTS, so that
# a dropped patch fails (the account screens' tests, design v3 7-5, moved with client patch 0033).
CLIENT_DART_TESTS_LATER=""

# Ubuntu has no libyuv.pc, which the client's linux-pkg-config build looks up (docs/e2e.md)
pkgconfig() {
  mkdir -p "$CI/pkgconfig"
  printf '%s\n' 'libdir=/usr/lib/x86_64-linux-gnu' 'includedir=/usr/include' 'Name: libyuv' \
    'Description: libyuv' 'Version: 0' 'Libs: -L${libdir} -lyuv' 'Cflags: -I${includedir}' > "$CI/pkgconfig/libyuv.pc"
}

client_stage() {
  local fl="$CI/flutter-$FLUTTER_VERSION" app="$CI/upstream/rustdesk/flutter" sum t tests=() lint
  local index=https://storage.googleapis.com/flutter_infra_release/releases
  # the server stage's release build of tools/hjv-release, against which the key files and the
  # release records are read too (its tests skip that part without it)
  local release_bin="$CI/target-hjv-release/release/hjv-release" samples="$ROOT/build/release-1.5.1"
  local vectors="$ROOT/tools/hjv-release/tests/vectors" ran cases
  if [ ! -x "$release_bin" ]; then
    release_bin=""
    skip "the key files and release records against hjv-release (no $CI/target-hjv-release/release/hjv-release: scripts/ci.sh server builds it)"
  fi
  # no network, no Google credentials (-B: no __pycache__ in scripts/)
  step "client: Google Play upload script, offline tests"
  python3 -I -B -m unittest discover -s "$ROOT/scripts" -p 'test_play_upload.py' ||
    fail "client: Google Play upload script tests"
  step "client: the pinned public keys' check (scripts/check-build-keys.py, client-build.yml), offline tests"
  HJV_RELEASE_BIN="$release_bin" python3 -I -B -m unittest discover -s "$ROOT/scripts" -p 'test_check_build_keys.py' ||
    fail "client: build-key check tests"
  step "client: actionlint on .github/workflows"
  if lint=$(command -v actionlint || ls "$HOME/.local/bin/actionlint" 2> /dev/null); then
    "$lint" "$ROOT"/.github/workflows/*.yml || fail "client: actionlint"
  else
    skip "actionlint is not installed (github.com/rhysd/actionlint)"
  fi
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
  echo "the patched tree: $(git -C "$CI/upstream/rustdesk" rev-parse 'HEAD^{tree}') (and the overlay files)"
  # release.json, SHA256SUMS and the workflow's minimum-version steps, also on the patched tree (the
  # Dart tests the workflow requires) and on the 1.5.1 artifacts kept under build/ (the manifest
  # gate on a real APK)
  step "client: the release records (scripts/release-info.py, client-build.yml), offline tests"
  [ -d "$samples" ] || { samples=""; skip "the release records on real artifacts (no build/release-1.5.1)"; }
  HJV_RELEASE_BIN="$release_bin" HJV_PATCHED_FLUTTER="$app" HJV_RELEASE_SAMPLES="$samples" \
    python3 -I -B -m unittest discover -s "$ROOT/scripts" -p 'test_release_info.py' || fail "client: release record tests"
  step "client: hbb_common WebSocket tests"
  # One at a time: the proxy test sets the process's proxy option, and a test running beside it
  # would connect through that test's one-shot proxy
  (cd "$CI/upstream/rustdesk" && CARGO_TARGET_DIR="$CI/target-client" cargo test --locked -p hbb_common --lib websocket -- --test-threads=1)
  # The apps build with Rust 1.75 (client-build.yml RUST_VERSION); the development machine's
  # stable toolchain would let newer language features through (design v3 6-6)
  step "client: cargo +1.75.0 check --features linux-pkg-config,hjv-e2e"
  rustup toolchain list | grep '^1\.75\.0' > /dev/null ||
    fail "client: no Rust 1.75.0 toolchain: rustup toolchain install 1.75.0 --profile minimal"
  pkgconfig
  (cd "$CI/upstream/rustdesk" && PKG_CONFIG_PATH="$CI/pkgconfig" CARGO_TARGET_DIR="$CI/target-175" \
    cargo +1.75.0 check --locked --features linux-pkg-config,hjv-e2e) || fail "client: cargo +1.75.0 check"
  # INV-13 of the trust rules: the client core takes and refuses every envelope of the shared vectors
  # as the reference model says (src/hjviewer_update/vectors.rs, a test run with --ignored only)
  if [ -d "$CI/upstream/rustdesk/src/hjviewer_update" ]; then
    step "client: the client core against the shared vectors of tools/hjv-release (output also in build/ci/client-vectors.log)"
    with_skips "$CI/client-vectors.log" env -C "$CI/upstream/rustdesk" PKG_CONFIG_PATH="$CI/pkgconfig" \
      CARGO_TARGET_DIR="$CI/target-client" HJV_UPDATE_VECTORS="$vectors" \
      cargo test --locked --features linux-pkg-config --lib hjviewer_update::vectors -- --ignored --nocapture ||
      fail "client: the client core disagrees with the shared vectors"
    at_least "client: the vector test that passed" "$(passed_tests "$CI/client-vectors.log")" 1
    # the cases it ran, by its counts of canonical and non-canonical cases, against the manifest's
    ran=$(sed -n -E 's/.*"canonical false": ([0-9]+), "canonical true": ([0-9]+).*/\1 \2/p' "$CI/client-vectors.log" |
      awk '{ print $1 + $2; exit }')
    cases=$(python3 -I -c 'import json, sys; print(len(json.load(open(sys.argv[1]))["cases"]))' "$vectors/manifest.json")
    [ "${ran:-0}" = "$cases" ] || fail "client: the vector test ran ${ran:-no} cases, the manifest has $cases"
    at_least "client: the vector cases the client core ran" "$ran" "$VECTORS_LEAST"
  else
    skip "the client core against the shared vectors (no src/hjviewer_update in the patched tree)"
  fi
  step "client: pure Dart tests"
  for t in $CLIENT_DART_TESTS; do
    [ -f "$app/$t" ] || fail "client: $t is not in the patched tree"
    tests+=("$t")
  done
  for t in $CLIENT_DART_TESTS_LATER; do
    if [ -f "$app/$t" ]; then tests+=("$t"); else skip "$t (not in client/patches yet)"; fi
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

# The signed version policy at /api/hjv/version (deploy/nginx-hjviewer-locations.conf, runbook 8),
# through an nginx whose location reads <file> instead of /etc/hjviewer/version-policy.json:
# without the file a 404 of its own in JSON (not the site's error page), with it the file's bytes
# as JSON, not cached, nosniff, a 304 for its ETag and 405 for a POST. The access and error logs
# are looked at after the last site.
check_version_policy() { # <port> <CA file> <name> <file>
  local url="https://localhost:$1/api/hjv/version" ca=$2 name=$3 file=$4 got etag h
  step "deploy: through $name, the version policy at /api/hjv/version"
  rm -f "$file"
  got=$(curl -s --cacert "$ca" -D "$CI/policy-head" -o "$CI/policy-body" -w '%{http_code}' "$url" || true)
  echo "no policy file: $got $(head -c 60 "$CI/policy-body")"
  [ "$got" = 404 ] && [ "$(cat "$CI/policy-body")" = '{"error":"no version policy"}' ] ||
    fail "$name: /api/hjv/version without a policy file answered $got, not its own 404"
  # a stand-in: nginx hands the file out without reading it (deploy/hjv-update-verify.sh checks it)
  printf '%s\n' '{"v":1,"k":1,"p":"{\"contact_ko\":\"설치 파일은 관리자에게 받으세요.\"}","s":"ci"}' > "$file"
  got=$(curl -s --cacert "$ca" -D "$CI/policy-head" -o "$CI/policy-body" -w '%{http_code}' "$url" || true)
  echo "with the file: $got, $(wc -c < "$CI/policy-body") bytes"
  [ "$got" = 200 ] && cmp -s "$CI/policy-body" "$file" || fail "$name: /api/hjv/version answered $got, not the file's bytes"
  for h in 'content-type: application/json; charset=utf-8' 'cache-control: no-cache' 'x-content-type-options: nosniff'; do
    tr -d '\r' < "$CI/policy-head" | grep -qix "$h" || fail "$name: /api/hjv/version without the header $h"
  done
  etag=$(tr -d '\r' < "$CI/policy-head" | sed -n 's/^[Ee][Tt][Aa][Gg]: //p')
  [ -n "$etag" ] || fail "$name: /api/hjv/version without an ETag"
  got=$(curl -s -o /dev/null --cacert "$ca" -H "If-None-Match: $etag" -w '%{http_code}' "$url" || true)
  echo "If-None-Match $etag: $got"
  [ "$got" = 304 ] || fail "$name: /api/hjv/version with its ETag answered $got, not 304"
  got=$(curl -s -o /dev/null --cacert "$ca" -X POST -d '{}' -w '%{http_code}' "$url" || true)
  echo "POST: $got"
  [ "$got" = 405 ] || fail "$name: a POST to /api/hjv/version answered $got, not 405"
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

# Throwaway credentials for a test run of hjv-api: one file in <dir> for each
# LoadCredentialEncrypted= line of <unit>, made as docs/server-runbook.md makes the real ones
# (hjv-api keygen), written with <write> (e.g. "sudo tee").
api_test_credentials() { # <hjv-api> <unit> <dir> <write...>
  local api=$1 unit=$2 dir=$3 name value
  shift 3
  for name in $(sed -n 's/^LoadCredentialEncrypted=//p' "$unit"); do
    case $name in
      hjviewer-sso-client-secret | hjviewer-sso-api-uuid) value="hjv-ci-$name" ;;
      hjviewer-api-login-key) value=$("$api" keygen seal 2> /dev/null) || fail "hjv-api keygen seal" ;;
      hjviewer-api-log-key) value=$("$api" keygen log-key 2> /dev/null) || fail "hjv-api keygen log-key" ;;
      hjviewer-api-list-key) value=$("$api" keygen list --kid 1 2> /dev/null) || fail "hjv-api keygen list --kid 1" ;;
      hjviewer-api-command-key) value=$("$api" keygen command --kid 1 2> /dev/null) || fail "hjv-api keygen command --kid 1" ;;
      hjviewer-api-ticket-key) value=$("$api" keygen ticket --kid 1 2> /dev/null) || fail "hjv-api keygen ticket --kid 1" ;;
      *) fail "scripts/ci.sh makes no test value for the credential $name of $(basename "$unit")" ;;
    esac
    printf '%s\n' "$value" | "$@" "$dir/$name" > /dev/null
  done
}

# The settings design v3 5-6 fixes for production (BIND_OS windows, SECOND_FACTOR off, the trust
# lists' refresh and lifetime) in <env file>, read as systemd reads it, with the defaults <packaged
# example> documents; skipped only for the names that <hjv-api> does not know yet
# (scripts/check-api-settings.py)
check_api_env() { # <env file> <packaged example> <hjv-api>
  step "deploy: hjv-api's production settings in $1"
  with_skips "$CI/api-settings.log" python3 -I -B "$ROOT/scripts/check-api-settings.py" --binary "$3" --example "$2" "$1" ||
    fail "hjv-api's production settings in $1 (above)"
}

# Whether hjv-api's passwordless switch is off, as it must be after an install (design v3 5-6, 11
# step 4): the first line of "admin passwordless status" says off and not on. Skipped only while
# the binary has no word of it; once it has, the command must answer.
check_switch_off() { # <hjv-api> <admin socket> <run...>
  local api=$1 socket=$2 status first
  shift 2
  if ! grep -aq passwordless "$api"; then
    skip "the passwordless switch (hjv-api has no such admin command yet)"
    return 0
  fi
  status=$("$@" "$api" admin --socket "$socket" passwordless status) || fail "hjv-api admin passwordless status"
  echo "$status"
  first=$(printf '%s\n' "$status" | head -n 1 | tr '[:upper:]' '[:lower:]')
  if ! [[ $first =~ (^|[^a-z])off([^a-z]|$) ]] || [[ $first =~ (^|[^a-z])on([^a-z]|$) ]]; then
    fail "hjv-api: the passwordless switch is not off after the install"
  fi
}

# hjv-api's socket and service from the package, run here like hbbs and hbbr: the shipped
# sandbox, limits and command, with this test's names, a dynamic user, throwaway credentials from
# plain files (systemd-creds would need this machine's credential key) and the main journal (no
# journal namespace left behind)
start_systemd_api() { # <unpacked package>
  local pkg=$1
  step "deploy B, systemd: hjv-api's socket and service from the package"
  sudo install -m 755 "$pkg/bin/hjv-api" /usr/local/lib/hjv-ci/bin/
  sudo install -d -m 700 /run/hjv-ci/creds
  api_test_credentials "$pkg/bin/hjv-api" "$pkg/systemd/hjviewer-api.service" /run/hjv-ci/creds sudo tee
  sed -e 's/^HJV_API_SSO_CLIENT_ID=.*/HJV_API_SSO_CLIENT_ID=hjv-ci/' \
    -e "s#^HJV_API_HBBS_PUBKEY=.*#HJV_API_HBBS_PUBKEY=$(sudo cat /var/lib/hjv-ci/id_ed25519.pub)#" \
    "$pkg/systemd/hjviewer-api.env.example" | sudo tee /run/hjv-ci/hjviewer-api.env > /dev/null
  sudo chmod 600 /run/hjv-ci/hjviewer-api.env  # root only, as docs/server-runbook.md 7-3 installs it
  sed -e 's/^User=hjviewer-api$/DynamicUser=yes\nUser=hjv-ci-api/' -e '/^Group=hjviewer-api$/d' \
    -e 's#/etc/hjviewer/hjviewer-api.env#/run/hjv-ci/hjviewer-api.env#' \
    -e 's#/var/lib/hjviewer-api#/var/lib/hjv-ci-api#g' -e 's/^StateDirectory=hjviewer-api$/StateDirectory=hjv-ci-api/' \
    -e 's/^RuntimeDirectory=hjviewer-api-admin$/RuntimeDirectory=hjv-ci-api-admin/' \
    -e 's/^LogsDirectory=hjviewer-api$/LogsDirectory=hjv-ci-api/' \
    -e 's#^InaccessiblePaths=-/var/lib/hjviewer$#InaccessiblePaths=-/var/lib/hjv-ci#' \
    -e 's#/opt/hjviewer/bin/#/usr/local/lib/hjv-ci/bin/#g' \
    -e 's#^LoadCredentialEncrypted=\(.*\)$#LoadCredential=\1:/run/hjv-ci/creds/\1#' -e '/^LogNamespace=/d' \
    -e 's/hjviewer-api\.socket/hjv-ci-api.socket/g' -e 's/hjviewer-hbbs\.service/hjv-ci-hbbs.service/g' \
    "$pkg/systemd/hjviewer-api.service" | sudo tee /run/systemd/system/hjv-ci-api.service > /dev/null
  sed 's#^ListenStream=/run/hjviewer-api.sock$#ListenStream=/run/hjv-ci-api.sock#' "$pkg/systemd/hjviewer-api.socket" |
    sudo tee /run/systemd/system/hjv-ci-api.socket > /dev/null
  grep -q '^ListenStream=/run/hjv-ci-api.sock$' /run/systemd/system/hjv-ci-api.socket || fail "hjviewer-api.socket: no ListenStream=/run/hjviewer-api.sock"
  sudo systemctl daemon-reload
  sudo systemctl start hjv-ci-api.socket hjv-ci-api.service || fail "systemd: hjv-api's units did not start"
}

check_systemd_api() {
  local bin=/usr/local/lib/hjv-ci/bin/hjv-api got
  wait_for 15 sh -c 'sudo journalctl -q -u hjv-ci-api.service --since "$1" -o cat | grep -q " ready: "' _ "$SYSTEMD_SINCE" ||
    fail "systemd: hjv-api is not ready ($(sudo journalctl -q -u hjv-ci-api.service --since "$SYSTEMD_SINCE" -o cat | tail -n 5))"
  sudo journalctl -q -u hjv-ci-api.service --since "$SYSTEMD_SINCE" -o cat | grep ' ready: '
  [ "$(sudo stat -c '%a %U %G' /run/hjv-ci-api.sock)" = "660 root www-data" ] ||
    fail "systemd: hjv-api's socket is not 660 root:www-data"
  got=$(sudo curl -s --max-time 5 --unix-socket /run/hjv-ci-api.sock http://localhost/api/hjv/source || true)
  echo "/api/hjv/source $got"
  [[ $got == *'"license":"AGPL-3.0-only"'* ]] || fail "systemd: hjv-api does not answer on its socket"
  [ "$(systemctl show -p NoNewPrivileges --value hjv-ci-api.service)" = yes ] || fail "systemd: hjv-api may gain privileges"
  [ -z "$(systemctl show -p CapabilityBoundingSet --value hjv-ci-api.service)" ] || fail "systemd: hjv-api keeps capabilities"
  [ "$(sudo stat -c '%a' /var/lib/private/hjv-ci-api)" = 700 ] || fail "systemd: hjv-api's state directory is not 700"
  [ "$(sudo stat -c '%a' /var/lib/private/hjv-ci-api/hjv-api.sqlite3)" = 600 ] || fail "systemd: hjv-api's database is not 600"
  sudo "$bin" admin --socket /run/hjv-ci-api-admin/admin.sock report > /dev/null || fail "systemd: hjv-api admin report"
  check_switch_off "$bin" /run/hjv-ci-api-admin/admin.sock sudo
  if sudo journalctl -q -u hjv-ci-api.service --since "$SYSTEMD_SINCE" -o cat |
    grep -E '^error: |panicked|Permission denied|Operation not permitted|Read-only file system'; then
    fail "systemd: hjv-api's sandbox gets in the way (journal above)"
  fi
}

# The account API's nginx limits from the package (zones file and locations), with Ubuntu's nginx
# in an unprivileged user and network namespace that has an IPv6 /64 of its own: hjv-relay's
# registrations are counted per IPv4 address and per IPv6 /64 (relay/PROTOCOL.md 4.3), also on a
# dual-stack listener (ipv6only=off), where IPv4 clients arrive as ::ffff:a.b.c.d; logins per
# address. No hjv-api runs, so what passes the limits gets the "server unavailable" answer.
check_api_limits() { # <unpacked package>
  local pkg=$1 ng="$CI/nginx-api"
  step "deploy: the account API's nginx limits, per IPv4 address and IPv6 /64"
  if ! unshare -rn true 2> /dev/null; then
    skip "the account API's nginx limits (no unprivileged user and network namespaces here)"
    return 0
  fi
  rm -rf "$ng"
  mkdir -p "$ng/tmp"
  # in the namespace this user is root, and its "nobody" does not exist: no worker processes and
  # no switch of user
  cat > "$ng/nginx.conf" << EOF
user root root;
master_process off;
daemon off;
pid $ng/nginx.pid;
error_log $ng/error.log info;
events {}
http {
  default_type application/octet-stream;
  access_log off;
  client_body_temp_path $ng/tmp; proxy_temp_path $ng/tmp; fastcgi_temp_path $ng/tmp;
  uwsgi_temp_path $ng/tmp; scgi_temp_path $ng/tmp;
  include $pkg/web/nginx-hjviewer-api-zones.conf;
  server {
    listen 127.0.0.1:18450;
    listen [::]:18450;
    include $pkg/web/nginx-hjviewer-api.conf;
  }
  # one dual-stack socket for IPv4 and IPv6, as some sites listen
  server {
    listen [::]:18451 ipv6only=off;
    include $pkg/web/nginx-hjviewer-api.conf;
  }
}
EOF
  unshare -rn bash -s -- "$PKGS/root/usr/sbin/nginx" "$ng" << 'EOF' || fail "the account API's nginx limits (above)"
set -euo pipefail
nginx=$1 dir=$2
ip link set lo up
for a in 2001:db8:1:2::1 2001:db8:1:2::2 2001:db8:1:2::3 2001:db8:1:3::1; do ip -6 addr add "$a/64" dev lo nodad; done
"$nginx" -p "$dir" -c "$dir/nginx.conf" -t 2> /dev/null
"$nginx" -p "$dir" -c "$dir/nginx.conf" &
trap 'kill $! 2> /dev/null; wait 2> /dev/null' EXIT
for _ in $(seq 50); do curl -s -o /dev/null http://127.0.0.1:18450/ && break; sleep 0.1; done
post() { # <source address> <URL>
  curl -s -o "$dir/body" -w '%{http_code}' --interface "$1" -X POST -d '{}' "$2"
}
check() { # <name> <URL> <429 body> <source addresses of seven requests at once...>
  local name=$1 url=$2 body=$3 codes="" src last
  shift 3
  for src in "${@:1:6}"; do codes+="$(post "$src" "$url") "; done
  last=$(post "$7" "$url")
  echo "$name: $codes-> $last $(cat "$dir/body")"
  case $codes in *429*) echo "FAIL: $name refused within the burst"; exit 1 ;; esac
  [ "$last" = 429 ] && [ "$(cat "$dir/body")" = "$body" ] || { echo "FAIL: $name: no 429 with $body"; exit 1; }
}
relay='{"error":"rate_limited","retry_after":60}'
a=2001:db8:1:2::2 b=2001:db8:1:2::3
check "relay hello, two addresses of one IPv6 /64" "http://[2001:db8:1:2::1]:18450/api/hjv/relay/hello" "$relay" \
  $a $b $a $b $a $b $a
code=$(post 2001:db8:1:3::1 "http://[2001:db8:1:2::1]:18450/api/hjv/relay/hello")
echo "relay hello from another /64: $code"
[ "$code" != 429 ] || { echo "FAIL: another /64 shares the limit"; exit 1; }
v4=(127.0.0.1 127.0.0.1 127.0.0.1 127.0.0.1 127.0.0.1 127.0.0.1 127.0.0.1)
check "relay hello, one IPv4 address" http://127.0.0.1:18450/api/hjv/relay/hello "$relay" "${v4[@]}"
v4=(127.0.0.3 127.0.0.3 127.0.0.3 127.0.0.3 127.0.0.3 127.0.0.3 127.0.0.3)
check "relay hello, one IPv4 address on a dual-stack listener" http://127.0.0.1:18451/api/hjv/relay/hello "$relay" \
  "${v4[@]}"
code=$(post 127.0.0.4 http://127.0.0.1:18451/api/hjv/relay/hello)
echo "relay hello from another IPv4 address on the dual-stack listener: $code"
[ "$code" != 429 ] || { echo "FAIL: the IPv4 clients of a dual-stack listener share one limit"; exit 1; }
v4=(127.0.0.1 127.0.0.1 127.0.0.1 127.0.0.1 127.0.0.1 127.0.0.1 127.0.0.1)
check "login, one IPv4 address" http://127.0.0.1:18450/api/login '{"error":"잠시 뒤 다시 시도해 주세요"}' "${v4[@]}"
check "access password, one IPv4 address" http://127.0.0.1:18450/api/hjv/guest '{"error":"잠시 뒤 다시 시도해 주세요"}' \
  "${v4[@]}"
EOF
}

# The release package installed the runbook's way in a clean Ubuntu 24.04 (the server's release)
# with Ubuntu's nginx: binaries and their libraries, the units (systemd-analyze verify), the
# account API's nginx files in place of the old /api/ block (nginx -t), and hjv-api behind that
# nginx, started on its socket by systemd-socket-activate as hjviewer-api.socket would.
check_package_container() { # <release dir> <package name>
  local image="${HJV_REGISTRY:-public.ecr.aws/docker/library}/ubuntu:24.04"
  step "deploy: the release package installed in a clean container ($image)"
  cat > "$CI/container-test.sh" << 'EOF'
set -euo pipefail
name=$1
step() { printf '\n== container: %s\n' "$*"; }
step "Ubuntu's nginx, systemd (systemd-analyze, systemd-socket-activate) and curl"
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq > /dev/null
apt-get install -y -qq --no-install-recommends nginx systemd curl openssl ca-certificates python3 > /dev/null 2>&1
grep PRETTY_NAME /etc/os-release
nginx -v
step "unpack; the binaries' libraries"
cd "$(mktemp -d)"
tar -xzf "/pkg/$name.tar.gz"
cd "$name"
if ldd bin/* | grep 'not found'; then echo "FAIL: libraries missing"; exit 1; fi
bin/hjv-api version
step "install as docs/server-runbook.md does"
useradd --system --user-group --home-dir /var/lib/hjviewer --no-create-home --shell /usr/sbin/nologin hjviewer
useradd --system --user-group --home-dir /var/lib/hjviewer-api --no-create-home --shell /usr/sbin/nologin hjviewer-api
install -d -m 755 /opt/hjviewer/bin /opt/hjviewer/share/web /etc/hjviewer
install -m 755 bin/* /opt/hjviewer/bin/
install -m 644 LICENSE.txt NOTICE.txt SOURCE.txt THIRD_PARTY_NOTICES.txt /opt/hjviewer/share/
install -m 644 web/* /opt/hjviewer/share/web/
sed 's/^HJV_DOMAIN=.*/HJV_DOMAIN=rd.example.com/' systemd/hjviewer.env.example > /etc/hjviewer/hjviewer.env
install -m 600 systemd/hjviewer-api.env.example /etc/hjviewer/hjviewer-api.env
units=()
for u in systemd/*.service systemd/*.socket systemd/*.timer; do
  [ -e "$u" ] || continue
  install -m 644 "$u" /etc/systemd/system/
  units+=("$(basename "$u")")
done
if [ -f systemd/journald@hjviewer.conf ]; then install -m 644 systemd/journald@hjviewer.conf /etc/systemd/; fi
step "systemd-analyze verify ${units[*]}"
verify=()
for u in "${units[@]}"; do
  case $u in
    *@.*) verify+=("${u/@./@ci.}") ;;  # a template, through an instance
    *) verify+=("/etc/systemd/system/$u") ;;
  esac
done
systemd-analyze verify "${verify[@]}"
step "nginx: the zones at the http level, the account API's locations in place of the /api/ block, nginx -t"
install -m 644 web/nginx-hjviewer-api-zones.conf /etc/nginx/conf.d/hjviewer-api-zones.conf
install -m 644 web/nginx-hjviewer-api.conf /etc/nginx/snippets/hjviewer-api.conf
awk '/^location \^~ \/api\/ \{/ { print "include snippets/hjviewer-api.conf;"; skip = 1; next }
  skip { if ($0 ~ /^\}/) skip = 0; next } { print }' web/nginx-hjviewer-locations.conf > /etc/nginx/snippets/hjviewer-locations.conf
grep -qx 'include snippets/hjviewer-api.conf;' /etc/nginx/snippets/hjviewer-locations.conf
if grep -q 'return 404;' /etc/nginx/snippets/hjviewer-locations.conf; then echo "FAIL: the old /api/ block is left"; exit 1; fi
d=/etc/letsencrypt/live/rd.example.com
mkdir -p "$d"
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 2 -subj /CN=rd.example.com \
  -addext subjectAltName=DNS:rd.example.com -keyout "$d/privkey.pem" -out "$d/fullchain.pem" 2> /dev/null
sed 's/DOMAIN/rd.example.com/g' web/nginx-hjviewer.conf > /etc/nginx/sites-available/hjviewer
ln -s /etc/nginx/sites-available/hjviewer /etc/nginx/sites-enabled/hjviewer
nginx -t
step "hjv-api behind nginx, on /run/hjviewer-api.sock"
api=/opt/hjviewer/bin/hjv-api
creds=$(mktemp -d)
printf 'hjv-ci-secret\n' > "$creds/hjviewer-sso-client-secret"
printf 'hjv-ci-uuid\n' > "$creds/hjviewer-sso-api-uuid"
for c in $(sed -n 's/^LoadCredentialEncrypted=hjviewer-api-//p' /etc/systemd/system/hjviewer-api.service); do
  case $c in
    login-key) kind=(seal) ;;
    log-key) kind=(log-key) ;;
    list-key) kind=(list --kid 1) ;;
    command-key) kind=(command --kid 1) ;;
    ticket-key) kind=(ticket --kid 1) ;;
    *) echo "FAIL: no test value for the credential hjviewer-api-$c"; exit 1 ;;
  esac
  "$api" keygen "${kind[@]}" > "$creds/hjviewer-api-$c" 2> /dev/null || { echo "FAIL: hjv-api keygen ${kind[*]}"; exit 1; }
done
chown -R hjviewer-api: "$creds"
chmod 700 "$creds"
sed 's/^HJV_API_SSO_CLIENT_ID=.*/HJV_API_SSO_CLIENT_ID=hjv-ci/' /etc/hjviewer/hjviewer-api.env > /tmp/api.env
install -d -m 700 -o hjviewer-api -g hjviewer-api /var/lib/hjviewer-api /var/log/hjviewer-api /run/hjviewer-api-admin
systemd-socket-activate -l /run/hjviewer-api.sock setpriv --reuid=hjviewer-api --regid=hjviewer-api \
  --init-groups -- sh -c 'umask 077; exec "$0" serve --env-file /tmp/api.env --credentials "$1" \
  --db /var/lib/hjviewer-api/hjv-api.sqlite3 --admin-socket /run/hjviewer-api-admin/admin.sock \
  --operator-log /var/log/hjviewer-api/operator.log' "$api" "$creds" > /tmp/api.log 2>&1 &
# the owner and mode hjviewer-api.socket gives the socket (nginx's workers are in its group)
for _ in $(seq 50); do [ -S /run/hjviewer-api.sock ] && break; sleep 0.1; done
chown "$(sed -n 's/^SocketUser=//p' /etc/systemd/system/hjviewer-api.socket):$(sed -n 's/^SocketGroup=//p' /etc/systemd/system/hjviewer-api.socket)" /run/hjviewer-api.sock
chmod "$(sed -n 's/^SocketMode=//p' /etc/systemd/system/hjviewer-api.socket)" /run/hjviewer-api.sock
ls -l /run/hjviewer-api.sock
nginx
c() { curl -sk --max-time 10 --resolve rd.example.com:443:127.0.0.1 -o /tmp/body -w '%{http_code}' "$@" || true; }
expect() { # <what> <status> <body pattern> <curl options...>
  local what=$1 status=$2 pattern=$3 code
  shift 3
  code=$(c "$@")
  echo "$what: $code $(head -c 120 /tmp/body)"
  [ "$code" = "$status" ] && grep -Eq -- "$pattern" /tmp/body || { echo "FAIL: $what"; cat /tmp/api.log; exit 1; }
}
expect "/api/hjv/source through nginx to hjv-api" 200 '"license":"AGPL-3.0-only"' https://rd.example.com/api/hjv/source
expect "/api/login-options (nginx)" 200 '^\[\]$' https://rd.example.com/api/login-options
expect "/api/heartbeat (nginx)" 404 'not found' -X POST https://rd.example.com/api/heartbeat
expect "/ (the source notice)" 200 'github.com/yonsei-projecttf/hjviewer' https://rd.example.com/
step "the version policy at /api/hjv/version: /etc/hjviewer/version-policy.json, read by nginx's workers"
expect "/api/hjv/version before a policy is installed" 404 '^\{"error":"no version policy"\}$' https://rd.example.com/api/hjv/version
printf '%s\n' '{"v":1,"k":1,"p":"{}","s":"ci"}' > /etc/hjviewer/version-policy.json.new
chmod 644 /etc/hjviewer/version-policy.json.new && mv -f /etc/hjviewer/version-policy.json.new /etc/hjviewer/version-policy.json
expect "/api/hjv/version (nginx, not hjv-api)" 200 '^\{"v":1,"k":1,"p":"\{\}","s":"ci"\}$' https://rd.example.com/api/hjv/version
step "the server's check of a version policy (bin/hjv-update-verify.sh) on this Ubuntu's python3 and openssl"
/opt/hjviewer/bin/hjv-update-verify.sh --self-test || { echo "FAIL: hjv-update-verify.sh --self-test"; exit 1; }
for i in 1 2 3 4 5 6; do c -X POST -d '{}' https://rd.example.com/api/hjv/relay/hello > /dev/null; done
expect "the 7th relay hello at once" 429 '^\{"error":"rate_limited","retry_after":60\}$' -X POST -d '{}' https://rd.example.com/api/hjv/relay/hello
step "hjv-api's passwordless switch after the install"
if grep -aq passwordless "$api"; then
  status=$("$api" admin --socket /run/hjviewer-api-admin/admin.sock passwordless status)
  echo "$status"
  first=$(printf '%s\n' "$status" | head -n 1 | tr '[:upper:]' '[:lower:]')
  if ! [[ $first =~ (^|[^a-z])off([^a-z]|$) ]] || [[ $first =~ (^|[^a-z])on([^a-z]|$) ]]; then
    echo "FAIL: the passwordless switch is not off after the install"; exit 1
  fi
else
  # counted as a skip of the run from container.log (check_package_container)
  echo "skipped: hjv-api has no passwordless switch yet"
fi
nginx -s quit
echo CONTAINER_OK
EOF
  sudo docker run --rm -v "$1:/pkg:ro" -v "$CI/container-test.sh:/test.sh:ro" "$image" bash /test.sh "$2" 2>&1 |
    tee "$CI/container.log" || fail "the release package in a clean container (build/ci/container.log)"
  [ "$(tail -n 1 "$CI/container.log")" = CONTAINER_OK ] || fail "the release package in a clean container (build/ci/container.log)"
  count_skips "$CI/container.log"
}

deploy_stage() {
  if [ ! -x "$PROBE" ] || [ ! -x "$LOAD" ]; then fail "hjv-probe/hjv-load are not built: scripts/ci.sh server"; fi
  if [ -d "$ROOT/server/api" ] && [ ! -x "$API" ]; then fail "hjv-api is not built: scripts/ci.sh server"; fi
  sudo docker compose version > /dev/null 2>&1 || fail "Docker with compose v2 is needed (docs/server-runbook.md 3-1)"
  step "deploy: the production settings check (scripts/check-api-settings.py), offline tests"
  python3 -I -B -m unittest discover -s "$ROOT/scripts" -p 'test_check_api_settings.py' ||
    fail "deploy: tests of the settings check"
  step "deploy: the App Review account's file (deploy/review-conf.py), offline tests"
  python3 -I -B -m unittest discover -s "$ROOT/scripts" -p 'test_review_conf.py' ||
    fail "deploy: tests of deploy/review-conf.py"
  step "deploy: the access password's file (deploy/guest-conf.py), offline tests"
  python3 -I -B -m unittest discover -s "$ROOT/scripts" -p 'test_guest_conf.py' ||
    fail "deploy: tests of deploy/guest-conf.py"
  # the server's own check of a signed version policy before it is installed (runbook 8-2): its
  # tests, then the shared test vectors of tools/hjv-release that the apps and the tool agree on
  step "deploy: the server's check of a version policy (deploy/hjv-update-verify.sh), offline tests"
  python3 -I -B -m unittest discover -s "$ROOT/scripts" -p 'test_hjv_update_verify.py' ||
    fail "deploy: tests of deploy/hjv-update-verify.sh"
  local vectors=${HJV_UPDATE_VECTORS:-$ROOT/tools/hjv-release/tests/vectors}
  if [ -f "$vectors/manifest.json" ]; then
    step "deploy: deploy/hjv-update-verify.sh against the shared test vectors ($vectors)"
    "$ROOT/deploy/hjv-update-verify.sh" --vectors "$vectors" ||
      fail "deploy: deploy/hjv-update-verify.sh and the shared test vectors disagree (above)"
  else
    skip "deploy/hjv-update-verify.sh against the shared test vectors (no $vectors/manifest.json: tools/hjv-release is not in this tree yet)"
  fi
  if [ -d "$ROOT/relay" ]; then
    # relay/tests/package.rs's packages run only with --ignored, which no other stage gives
    step "deploy: hjv-relay's packages (scripts/package-relay.sh): the static build, its SHA256SUMS, the field test package"
    [ -n "$(rustup target list --installed 2>/dev/null | grep -x x86_64-unknown-linux-musl)" ] && command -v musl-gcc > /dev/null ||
      fail "deploy: hjv-relay's packages need the x86_64-unknown-linux-musl target and musl-gcc (rustup target add x86_64-unknown-linux-musl; Ubuntu: apt-get install musl-tools)"
    # with the output of the tests that pass: scripts/package-relay.sh says "skipped: ..." where it
    # cannot run the binary in an empty root
    with_skips "$CI/relay-package.log" env -C "$ROOT/relay" CARGO_TARGET_DIR="$CI/target-relay" \
      cargo test --locked --test package -- --ignored --show-output ||
      fail "deploy: hjv-relay's packages (relay/tests/package.rs --ignored)"
  else
    skip "hjv-relay's packages (no relay/)"
  fi
  [ -z "$(ss -Hltnu '( sport >= :21115 and sport <= :21119 )')" ] ||
    fail "ports 21115-21119 are in use (a running hjviewer server?); the test stack needs them"
  [ -z "$(ss -Hltn '( sport >= :18080 and sport <= :18082 ) or ( sport >= :18443 and sport <= :18448 ) or sport = :18599')" ] ||
    fail "test ports 18080-18082 / 18443-18448 / 18599 are in use"
  trap cleanup EXIT
  DEPLOY_LOGS=1

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
  # the version policy from a test file under build/ci instead of /etc/hjviewer/version-policy.json
  sed "s#alias /etc/hjviewer/version-policy.json;#alias $ng/version-policy.json;#" \
    "$SRC/deploy/nginx-hjviewer-locations.conf" > "$ng/snippets/hjviewer-locations.conf"
  grep -q "alias $ng/version-policy.json;" "$ng/snippets/hjviewer-locations.conf" ||
    fail "nginx-hjviewer-locations.conf: no 'alias /etc/hjviewer/version-policy.json;' line"
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
  # client cannot choose the address hbbs/hbbr trust. This site sends every 404 to a PHP page, as
  # some WordPress sites do: the version policy's own 404 must not go there.
  sed 's#http://127\.0\.0\.1:2111[89]#http://127.0.0.1:18599#' "$ng/snippets/hjviewer-locations.conf" \
    > "$ng/snippets/hjviewer-locations-echo.conf"
  cat > "$ng/echo.conf" << EOF
server {
    listen 127.0.0.1:18447 ssl http2;
    server_name localhost;
    ssl_certificate     $CERTS/cert.pem;
    ssl_certificate_key $CERTS/key.pem;
    error_page 404 /index.php;
    include snippets/hjviewer-locations-echo.conf;
    location ~ \.php\$ {
        return 302 https://\$host/404/;
    }
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
  check_version_policy 18443 "$CERTS/cert.pem" nginx "$ng/version-policy.json"
  [ "$(curl -s -o /dev/null -w '%{http_code} %{redirect_url}' http://localhost:18080/x)" = "301 https://localhost/x" ] ||
    fail "nginx: port 80 does not redirect to https"
  check_site 18446 "$CERTS/cert.pem" site "nginx, existing site with HTTP/2"
  check_version_policy 18446 "$CERTS/cert.pem" "nginx, existing site with HTTP/2" "$ng/version-policy.json"
  # the existing site still answers everything else, over HTTP/2 for browsers
  [ "$(curl -s -o /dev/null -w '%{http_version} %{http_code} %{redirect_url}' --http2 --cacert "$CERTS/cert.pem" https://localhost:18446/)" = \
    "2 302 https://localhost/login/?redirect_to=/" ] || fail "nginx: the existing site's / changed"
  [ "$(curl -s -o /dev/null -w '%{http_code}' --cacert "$CERTS/cert.pem" https://localhost:18446/wp-json/)" = 302 ] ||
    fail "nginx: the existing site's other paths changed"
  check_headers 18447 nginx
  check_version_policy 18447 "$CERTS/cert.pem" "nginx, a site whose 404s go to a PHP page" "$ng/version-policy.json"
  if grep ' /api/hjv/version ' "$ng/access.log"; then fail "nginx: /api/hjv/version is in the access log"; fi
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
    sed -e 's/<VirtualHost \*:18444>/<VirtualHost *:18448>/' -e 's#ws://127\.0\.0\.1:2111[89]#ws://127.0.0.1:18599#' \
      -e 's#unix:/run/hjviewer-api\.sock|http://localhost#http://127.0.0.1:18599#' > "$ap/echo.conf"
  grep -q 'http://127.0.0.1:18599/api/' "$ap/echo.conf" || fail "apache-hjviewer.conf: no account paths to hjv-api's socket"
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
  check_account_proxy 18448 Apache
  [ "$(curl -s -o /dev/null -w '%{http_code} %{redirect_url}' http://localhost:18081/x)" = "301 https://localhost/x" ] ||
    fail "Apache: port 80 does not redirect to https"
  # "client denied by server configuration" would feed fail2ban's apache-auth filter
  if grep -E 'AH01630|:(error|crit|alert|emerg)\]' "$ap/error.log"; then fail "Apache logged errors"; fi
  # last, since Apache logs the upstream it cannot reach: the site as shipped, without hjv-api
  check_account_down 18444 "$CERTS/cert.pem" Apache
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
  # the account paths of deploy/Caddyfile in a stack without hjv-api: what Caddy answers itself
  # (the E2E has them in front of hjv-api)
  [ "$(curl -s --cacert "$CI/caddy-root.crt" https://localhost:18445/api/login-options)" = '[]' ] ||
    fail "Caddy: /api/login-options is not []"
  [ "$(curl -s --cacert "$CI/caddy-root.crt" https://localhost:18445/api/device-group/accessible)" = '{"total":0,"data":[]}' ] ||
    fail "Caddy: /api/device-group/accessible is not the empty list"
  [ "$(curl -s -w ' %{http_code}' --cacert "$CI/caddy-root.crt" -X POST -d '{}' https://localhost:18445/api/heartbeat)" = '{"error":"not found"} 404' ] ||
    fail "Caddy: the rest of /api/ does not answer 404 in JSON"
  check_account_down 18445 "$CI/caddy-root.crt" Caddy

  step "deploy B, systemd: the release package (scripts/package-server.sh) without Docker"
  dc down
  local rel="$CI/release" pkg unit f api_files=()
  rm -rf "$rel"
  HJV_SKIP_SOURCE_CHECK=1 HJV_SERVER_BIN_DIR="$CI/target/release" HJV_API_BIN="$API" \
    "$ROOT/scripts/package-server.sh" ci "$rel" > "$CI/package.log" 2>&1 ||
    { tail -n 30 "$CI/package.log"; fail "scripts/package-server.sh"; }
  [ "$(cat "$rel/SHA256SUMS")" = "$(cd "$rel" && sha256sum hjviewer-server-ci-linux-x86_64.tar.gz)" ] ||
    fail "release package: SHA256SUMS does not name the package alone"
  tar -xzf "$rel/hjviewer-server-ci-linux-x86_64.tar.gz" -C "$rel"
  pkg="$rel/hjviewer-server-ci-linux-x86_64"
  if [ -d "$ROOT/server/api" ]; then
    # hjv-api, its units, settings and nginx files; the watch and backup units and the journal
    # namespace's retention once deploy/ has them
    api_files=(bin/hjv-api systemd/hjviewer-api.service systemd/hjviewer-api.socket systemd/hjviewer-api.env.example
      web/nginx-hjviewer-api.conf web/nginx-hjviewer-api-zones.conf)
    for f in "$ROOT"/deploy/systemd/hjviewer-api-*.service "$ROOT"/deploy/systemd/hjviewer-api-*.timer \
      "$ROOT/deploy/journald@hjviewer.conf"; do
      if [ -e "$f" ]; then api_files+=("systemd/$(basename "$f")"); fi
    done
  else
    skip "hjv-api in the release package (no server/api)"
  fi
  for f in bin/hbbs bin/hbbr bin/hjv-update-verify.sh systemd/hjviewer-hbbs.service systemd/hjviewer-hbbr.service systemd/hjviewer.env.example \
    web/nginx-hjviewer-locations.conf web/nginx-hjviewer.conf web/apache-hjviewer.conf \
    LICENSE.txt NOTICE.txt SOURCE.txt THIRD_PARTY_NOTICES.txt "${api_files[@]}"; do
    [ -s "$pkg/$f" ] || fail "release package: no $f"
  done
  grep -q 'server/patches' "$pkg/SOURCE.txt" || fail "release package: SOURCE.txt does not describe the server build"
  if [ ${#api_files[@]} -gt 0 ]; then
    grep -q 'server/api' "$pkg/SOURCE.txt" || fail "release package: SOURCE.txt does not describe hjv-api's build"
    # a crate of hjv-api's that hbbs and hbbr do not use
    grep -q '^crypto_box ' "$pkg/THIRD_PARTY_NOTICES.txt" || fail "release package: hjv-api's crates are not in THIRD_PARTY_NOTICES"
  fi
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
  if [ ${#api_files[@]} -gt 0 ]; then
    start_systemd_api "$pkg"
    check_systemd_api
  fi
  "$PKGS/root/usr/sbin/nginx" -e "$ng/error.log" -p "$ng" -c "$ng/nginx.conf" -g 'daemon off;' > "$ng/out.log" 2>&1 &
  NGINX_PID=$!
  wait_for 10 curl -s -o /dev/null --cacert "$CERTS/cert.pem" https://localhost:18446/ || fail "nginx did not start"
  check_site 18446 "$CERTS/cert.pem" site "nginx, existing site with HTTP/2, to the systemd units"
  stop_web
  STACK=docker
  cleanup_systemd

  if [ ${#api_files[@]} -gt 0 ]; then
    check_api_env "$pkg/systemd/hjviewer-api.env.example" "$pkg/systemd/hjviewer-api.env.example" "$pkg/bin/hjv-api"
    # a server's own settings, e.g. a copy of /etc/hjviewer/hjviewer-api.env: no secret in it, but
    # the copy leaves out the HJV_API_OPERATORS and HJV_API_CAMPUS_NETS lines (portal IDs and
    # campus ranges, why only root reads the server's file: docs/server-runbook.md 7-3)
    if [ -n "${HJV_API_ENV_FILE:-}" ]; then
      check_api_env "$HJV_API_ENV_FILE" "$pkg/systemd/hjviewer-api.env.example" "$pkg/bin/hjv-api"
    fi
    check_api_limits "$pkg"
    check_package_container "$rel" hjviewer-server-ci-linux-x86_64
  fi

  cleanup
  trap - EXIT
}

# Caddy for the E2E, once: the release archive, checked against CADDY_SHA256
fetch_caddy() {
  local dir="$CI/caddy-$CADDY_VERSION" tgz="$CI/caddy.tar.gz"
  CADDY_BIN="$dir/caddy"
  [ -x "$CADDY_BIN" ] && return 0
  step "Caddy $CADDY_VERSION, checked against its pinned SHA-256"
  curl -fsSL --max-time 300 -o "$tgz" \
    "https://github.com/caddyserver/caddy/releases/download/v$CADDY_VERSION/caddy_${CADDY_VERSION}_linux_amd64.tar.gz" ||
    fail "Caddy download"
  echo "$CADDY_SHA256  $tgz" | sha256sum -c --quiet - || fail "the Caddy archive is not the one CADDY_SHA256 pins"
  rm -rf "$dir.tmp" && mkdir -p "$dir.tmp" && tar -xzf "$tgz" -C "$dir.tmp" caddy
  mv "$dir.tmp" "$dir" && rm -f "$tgz"
  "$CADDY_BIN" version
}

# The E2E's keys, from seeds of one repeated byte, the kind scripts/check-build-keys.py refuses in
# a release build: seal key e2, trust-list keys 1 and 2 e3 and e4, command key 1 e5, log key e6.
# In <dir>: <name>.secret the secret halves (base64, as the credentials take them), seal.pub,
# trust.pub, command.pub and build.env the public values.
e2e_keys() { # <dir>
  local dir=$1 name kind byte hex
  mkdir -p "$dir"
  chmod 700 "$dir"
  # (ticket: hjv-api's access ticket key, whose public key is hbbs's KEY= line: the access stage)
  for spec in "seal x25519 e2" "list-1 ed25519 e3" "list-2 ed25519 e4" "command-1 ed25519 e5" "log - e6" \
    "ticket ed25519 e7"; do
    read -r name kind byte <<< "$spec"
    hex=""
    for _ in $(seq 32); do hex+=$byte; done
    python3 -I -c 'import base64, sys; print(base64.b64encode(bytes.fromhex(sys.argv[1])).decode())' "$hex" > "$dir/$name.secret"
    if [ "$kind" != - ]; then python3 -I "$ROOT/scripts/check-build-keys.py" pubkey "$kind" "$hex" > "$dir/$name.pub"; fi
  done
  echo "1:$(cat "$dir/list-1.pub"),2:$(cat "$dir/list-2.pub")" > "$dir/trust.pub"
  echo "1:$(cat "$dir/command-1.pub")" > "$dir/command.pub"
  {
    echo "HJV_LOGIN_SEAL_PUBKEY=$(cat "$dir/seal.pub")"
    echo "HJV_TRUST_PUBKEYS=$(cat "$dir/trust.pub")"
    echo "HJV_COMMAND_PUBKEY=$(cat "$dir/command.pub")"
  } > "$dir/build.env"
  chmod 600 "$dir"/*
}

# The test server's key pair, which hbbs makes on its first start (docs/e2e.md)
e2e_server_key() { # <work dir>
  mkdir -p "$1/srv"
  if [ ! -s "$1/srv/id_ed25519.pub" ]; then
    (cd "$1/srv" && { timeout 2 "$CI/target/release/hbbs" -p 41116 > /dev/null 2>&1 || true; })
  fi
  [ -s "$1/srv/id_ed25519.pub" ] || fail "e2e: hbbs made no key in $1/srv"
}

# The update keys of the E2E's clients (minimum version design 1-7): the test keys of
# tools/hjv-release (hjv-release test-keys --env), from seeds of 32 consecutive bytes that start at
# 40, 80, a0 and e0 (signing keys 1 and 2, root keys 101 and 102: two, as a release build pins),
# and the floor 1:0. A build with the E2E's commands (hjv-e2e) takes part in the version policy
# with these keys and the server hjv.test only.
e2e_update_keys() {
  local first seed key pub=()
  for first in 40 80 a0 e0; do
    seed=$(python3 -I -c 'import sys; f = int(sys.argv[1], 16); print(bytes(range(f, f + 32)).hex())' "$first") || return 1
    key=$(python3 -I "$ROOT/scripts/check-build-keys.py" pubkey ed25519 "$seed") || return 1
    [ ${#key} = 44 ] || return 1
    pub+=("$key")
  done
  printf '%s\n' "HJV_UPDATE_PUBKEY=1:${pub[0]},2:${pub[1]}" "HJV_UPDATE_ROOT_PUBKEY=101:${pub[2]},102:${pub[3]}" HJV_POLICY_FLOOR=1:0
}

# The client of the E2E (docs/e2e.md): a fresh upstream checkout with client/patches, built for
# the test server hjv.test with the test feature hjv-e2e. Once the patches have the account code
# (src/hjviewer_account) and firewall-e2e.sh its account part (HJV_E2E_ACCOUNT), it also pins the
# E2E's public keys and runs unit tests in that build, when the patched tree has <module>: those
# that <filter> picks (cargo test --lib <filter>, by default the module's). The e2e stage runs all
# of the client's HJViewer tests (hjv: the account code's, design v3 7-3, its hooks' in the
# controller, hjviewer.rs's and the wake report's), the wol stage the wake report's (hjviewer_wol,
# design v3 8-10). Before the account code, it is built like 1.5.1. Once the patches have the update
# code (src/hjviewer_update) and firewall-e2e.sh its part (HJV_E2E_UPDATE_KEYS), the build and its
# unit tests also get the update test keys (e2e_update_keys; E2E_KEYED), so that the update code's
# tests run as in a build that takes part; when <filter> picks those tests, they run once more
# without the keys (e2e_unit_without_update_keys). A client given by hand
# (E2E_CLIENT, which only the wol stage takes) is used as it is, and that is a skip.
# The staff-only gate's A1-A14 (docs/e2e.md, the access stage): with hbbs's access file (server
# patch 0006), the client's access password command (client patch 0061) and the account E2E,
# whose hjv-api signs the tickets. Like e2e_minimum_version: a patch whose build lacks its part
# fails, no patch skips.
e2e_access() {
  local help
  E2E_ACCESS=()
  help=$(cd "$CI" && "$CI/target/release/hbbs" --help 2>&1 || true)
  if ! grep -q -- '--hjv-access-file' <<< "$help"; then
    grep -q -- 'hjv-access-file' "$ROOT"/server/patches/*.patch &&
      fail "e2e: hbbs has no --hjv-access-file, though server/patches has it (patch 0006): scripts/ci.sh server builds it again"
    skip "the staff-only gate's A1-A14 (hbbs has no --hjv-access-file: no server patch 0006)"
    return 0
  fi
  # (the name of its variable: a release build compares the commands' names without keeping them)
  if ! grep -aqF HJV_E2E_ACCESS_PASSWORD "$E2E_CLIENT"; then
    grep -q HJV_E2E_ACCESS_PASSWORD "$ROOT"/client/patches/rustdesk/*.patch &&
      fail "e2e: the client has no HJV_E2E_ACCESS_PASSWORD, though client/patches has it (patch 0061)"
    skip "the staff-only gate's A1-A14 (the client has no HJV_E2E_ACCESS_PASSWORD: no client patch 0061)"
    return 0
  fi
  if [ -z "$E2E_ACCOUNT" ] || ! grep -q HJV_E2E_ACCESS "$ROOT/tools/e2e/firewall-e2e.sh"; then
    skip "the staff-only gate's A1-A14 (no account E2E, or no HJV_E2E_ACCESS part in tools/e2e/firewall-e2e.sh)"
    return 0
  fi
  E2E_ACCESS=(HJV_E2E_ACCESS=1)
}
build_e2e_client() { # <work dir> [<module of src/> [<test filter>]]
  local work=$1 unit=${2:-hjviewer_account} src="$CI/upstream-e2e/rustdesk" keys=() update_keys
  local filter=${3:-$unit}
  if [ -n "$E2E_CLIENT_GIVEN" ]; then
    E2E_CLIENT=$E2E_CLIENT_GIVEN
    skip "the client build and its unit tests (E2E_CLIENT=$E2E_CLIENT is used as it is)"
    return 0
  fi
  # what a build of the e2e stage leaves for the wol stage of its prepush run: gone until it passes
  rm -f "$CI/target-e2e/e2e-client.stamp"
  step "e2e: upstream checkout with client/patches"
  rm -rf "$CI/upstream-e2e"
  UPSTREAM_DIR="$CI/upstream-e2e" "$ROOT/scripts/fetch-upstream.sh" client
  echo "the patched tree: $(git -C "$src" rev-parse 'HEAD^{tree}') (and the overlay files)"
  E2E_ACCOUNT=
  if [ -d "$src/src/hjviewer_account" ] && grep -q HJV_E2E_ACCOUNT "$ROOT/tools/e2e/firewall-e2e.sh"; then
    E2E_ACCOUNT=1
    keys=(HJV_LOGIN_SEAL_PUBKEY="$(cat "$work/api/seal.pub")" HJV_TRUST_PUBKEYS="$(cat "$work/api/trust.pub")")
  else
    skip "the account E2E (no src/hjviewer_account in the patched tree, or no HJV_E2E_ACCOUNT part in tools/e2e/firewall-e2e.sh): the client is built without the account keys, like 1.5.1"
  fi
  # the update test keys, once the patches have the update code: the e2e stage says what it skips
  E2E_KEYED=
  if [ -d "$src/src/hjviewer_update" ] && grep -q HJV_E2E_UPDATE_KEYS "$ROOT/tools/e2e/firewall-e2e.sh"; then
    E2E_KEYED=1
    update_keys=$(e2e_update_keys) || fail "e2e: the update test keys (scripts/check-build-keys.py pubkey)"
    readarray -t -O "${#keys[@]}" keys <<< "$update_keys"
  fi
  step "e2e: the client for the test server hjv.test (--features linux-pkg-config,hjv-e2e)"
  pkgconfig
  (cd "$src" && env PKG_CONFIG_PATH="$CI/pkgconfig" HJV_SERVER=hjv.test HJV_SERVER_KEY="$(cat "$work/srv/id_ed25519.pub")" \
    "${keys[@]}" "${E2E_PROFILE[@]}" CARGO_TARGET_DIR="$CI/target-e2e" \
    cargo build --release --locked --features linux-pkg-config,hjv-e2e) || fail "e2e: the client build"
  E2E_CLIENT="$CI/target-e2e/release/rustdesk"
  if [ -e "$src/src/$unit" ] || [ -e "$src/src/$unit.rs" ]; then
    step "e2e: the client's unit tests (--lib $filter, output also in build/ci/e2e-unit-$filter.log)"
    with_skips "$CI/e2e-unit-$filter.log" env -C "$src" PKG_CONFIG_PATH="$CI/pkgconfig" HJV_SERVER=hjv.test \
      HJV_SERVER_KEY="$(cat "$work/srv/id_ed25519.pub")" "${keys[@]}" "${E2E_PROFILE[@]}" CARGO_TARGET_DIR="$CI/target-e2e" \
      cargo test --release --locked --features linux-pkg-config,hjv-e2e --lib "$filter" ||
      fail "e2e: the client's unit tests (--lib $filter)"
    at_least "e2e: the client's unit tests that passed (--lib $filter)" "$(passed_tests "$CI/e2e-unit-$filter.log")" \
      "${UNIT_LEAST[$filter]:-1}"
    if [ -n "$E2E_KEYED" ] && [[ hjviewer_update == *"$filter"* ]]; then e2e_unit_without_update_keys "$work" "${keys[@]}"; fi
  else
    skip "the client's unit tests of src/$unit (not in the patched tree)"
  fi
}

# The update code's unit tests once more, in a build without the update keys (minimum version
# design 1-7): such a build takes no part and does what 1.5.1 did, and the tests that pin this
# return at once in the keyed build. The build variables of build_e2e_client without the update
# keys, which are taken out of this shell's environment too, in the same target directory: cargo
# compiles the client crate's tests again (about a minute), which shows that the keys are gone, and
# leaves the client built there as it is.
e2e_unit_without_update_keys() { # <work dir> <build variables...>
  local work=$1 v vars=() log="$CI/e2e-unit-hjviewer_update-nokeys.log"
  shift
  for v in "$@"; do
    case $v in HJV_UPDATE_PUBKEY=* | HJV_UPDATE_ROOT_PUBKEY=* | HJV_POLICY_FLOOR=*) ;; *) vars+=("$v") ;; esac
  done
  step "e2e: the update code's unit tests without the update keys (--lib hjviewer_update, output also in build/ci/e2e-unit-hjviewer_update-nokeys.log)"
  with_skips "$log" env -C "$CI/upstream-e2e/rustdesk" -u HJV_UPDATE_PUBKEY -u HJV_UPDATE_ROOT_PUBKEY -u HJV_POLICY_FLOOR \
    PKG_CONFIG_PATH="$CI/pkgconfig" HJV_SERVER=hjv.test HJV_SERVER_KEY="$(cat "$work/srv/id_ed25519.pub")" "${vars[@]}" \
    "${E2E_PROFILE[@]}" CARGO_TARGET_DIR="$CI/target-e2e" \
    cargo test --release --locked --features linux-pkg-config,hjv-e2e --lib hjviewer_update ||
    fail "e2e: the update code's unit tests without the update keys (--lib hjviewer_update)"
  grep -q '^ *Compiling rustdesk v' "$log" ||
    fail "e2e: cargo did not compile the client's tests again without the update keys: they ran with the keys"
  at_least "e2e: the update code's unit tests that passed without the update keys" "$(passed_tests "$log")" \
    "${UNIT_LEAST[hjviewer_update-nokeys]}"
}

# What the E2E's client is built from and with: client/ (the patches, the overlay), the upstream
# pins, the test server's key, the E2E's public keys, the profile and the toolchain. With the
# prepush run and the binary itself, the e2e stage leaves it for the wol stage of that run, which
# then takes the e2e stage's build and the unit tests it ran (hjviewer_wol's among hjv's).
e2e_client_stamp() { # <work dir> <binary>
  local inputs
  inputs=$({
    (cd "$ROOT" && find client scripts/upstream.env scripts/fetch-upstream.sh -type f -print0 | sort -z | xargs -0 sha256sum)
    cat "$1/srv/id_ed25519.pub" "$1/api/seal.pub" "$1/api/trust.pub"
    echo "${E2E_PROFILE[*]} $(cargo --version)"
  } | sha256sum | cut -d' ' -f1)
  echo "run ${HJV_CI_RUN:-none} inputs $inputs binary $(sha256sum < "$2" | cut -d' ' -f1)"
}

# The 1.5.1 client of the account E2E's X2 and X3 (design v3 7-4): the patches and the upstream
# pin of the tag client-1.5.1, built for the test server like the client above. Built once for a
# tag commit and server key, and kept as build/ci/e2e-old/rustdesk; the scenarios are skipped
# where the repository has no such tag (a shallow checkout).
build_e2e_client_old() { # <work dir>
  local work=$1 dir="$CI/e2e-old" stamp
  E2E_CLIENT_OLD=""
  if ! git -C "$ROOT" rev-parse -q --verify "refs/tags/$E2E_OLD_TAG^{commit}" > /dev/null; then
    skip "the 1.5.1 client of X2 and X3 (this repository has no tag $E2E_OLD_TAG)"
    return 0
  fi
  stamp="$(git -C "$ROOT" rev-parse "refs/tags/$E2E_OLD_TAG^{commit}") $(cat "$work/srv/id_ed25519.pub")"
  if [ ! -x "$dir/rustdesk" ] || [ "$(cat "$dir/stamp" 2> /dev/null)" != "$stamp" ]; then
    step "e2e: the 1.5.1 client (tag $E2E_OLD_TAG) for the test server, once"
    rm -rf "$dir" "$CI/target-e2e-old"
    mkdir -p "$dir/repo"
    git -C "$ROOT" archive "refs/tags/$E2E_OLD_TAG" scripts client | tar -x -C "$dir/repo"
    UPSTREAM_DIR="$dir/upstream" "$dir/repo/scripts/fetch-upstream.sh" client
    pkgconfig
    (cd "$dir/upstream/rustdesk" && env PKG_CONFIG_PATH="$CI/pkgconfig" HJV_SERVER=hjv.test \
      HJV_SERVER_KEY="$(cat "$work/srv/id_ed25519.pub")" "${E2E_PROFILE[@]}" CARGO_TARGET_DIR="$CI/target-e2e-old" \
      cargo build --release --locked --features linux-pkg-config,hjv-e2e) || fail "e2e: the 1.5.1 client build"
    cp "$CI/target-e2e-old/release/rustdesk" "$dir/rustdesk"
    echo "$stamp" > "$dir/stamp"
    # the binary is what is kept
    rm -rf "$dir/repo" "$dir/upstream" "$CI/target-e2e-old"
  fi
  E2E_CLIENT_OLD="$dir/rustdesk"
}

# libfaketime for the account E2E's S27 (a controller's clock 15 minutes ahead): the system's if
# it is installed, else Ubuntu's package unpacked under build/ci, as nginx and Apache are for the
# deploy stage. Without either, S27 is skipped with a line saying so.
fetch_faketime() {
  local dir="$CI/faketime" lib=usr/lib/x86_64-linux-gnu/faketime/libfaketime.so.1
  E2E_FAKETIME=""
  if [ -f "/$lib" ]; then
    E2E_FAKETIME="/$lib"
  elif [ -f "$dir/root/$lib" ]; then
    E2E_FAKETIME="$dir/root/$lib"
  else
    step "e2e: Ubuntu's libfaketime, unpacked"
    rm -rf "$dir"
    mkdir -p "$dir/root"
    if (cd "$dir" && apt-get download libfaketime > /dev/null 2>&1) && dpkg-deb -x "$dir"/libfaketime_*.deb "$dir/root" &&
      [ -f "$dir/root/$lib" ]; then
      E2E_FAKETIME="$dir/root/$lib"
    else
      skip "libfaketime (apt-get download libfaketime failed): the E2E skips S27"
    fi
  fi
}

# hjv-api and the fake SSO for the E2Es (server/api, feature fake-sso), apart from the build the
# server package takes
build_e2e_api() {
  step "e2e: hjv-api and hjv-fake-sso (server/api)"
  (cd "$ROOT/server/api" && CARGO_TARGET_DIR="$CI/target-api-e2e" cargo build --release --locked --features fake-sso) ||
    fail "e2e: hjv-api build"
  E2E_API="$CI/target-api-e2e/release/hjv-api" E2E_FAKE_SSO="$CI/target-api-e2e/release/hjv-fake-sso"
}

cleanup_e2e() {
  sudo rm -rf "$E2E_PC_DIR"
}

# The minimum version's parts of the E2E (its design 2-4, 1-7; docs/e2e.md), as values for
# firewall-e2e.sh in E2E_MINVER: hbbs's lowest controller version (V1-V4) once hbbs has server patch
# 0005; once server/patches has that patch, an hbbs without it (built by an earlier server stage)
# fails the stage instead of skipping the part. And the signed version policy (V5-V9) once the
# client has the update test keys, with tools/hjv-release, which signs its test policies; without
# the tool, firewall-e2e.sh says that it skips them.
e2e_minimum_version() {
  local help
  E2E_MINVER=()
  help=$(cd "$CI" && "$CI/target/release/hbbs" --help 2>&1 || true)
  if grep -q -- '--hjv-min-controller-file' <<< "$help"; then
    E2E_MINVER+=(HJV_E2E_MIN_CONTROLLER=1)
  elif grep -q -- 'hjv-min-controller-file' "$ROOT"/server/patches/*.patch; then
    fail "e2e: hbbs has no --hjv-min-controller-file, though server/patches has it (patch 0005): scripts/ci.sh server builds it again"
  else
    skip "the minimum version's V1-V4 (hbbs has no --hjv-min-controller-file: no server patch 0005)"
  fi
  if [ -z "$E2E_KEYED" ]; then
    skip "the minimum version's V5-V9 (no src/hjviewer_update in the patched tree, or no HJV_E2E_UPDATE_KEYS part in tools/e2e/firewall-e2e.sh): the client is built without the update keys, like 1.5.1"
    return 0
  fi
  E2E_MINVER+=(HJV_E2E_UPDATE_KEYS=1)
  if [ -f "$ROOT/tools/hjv-release/Cargo.toml" ]; then
    step "e2e: tools/hjv-release, which signs the E2E's version policies with the test keys"
    (cd "$ROOT/tools/hjv-release" && CARGO_TARGET_DIR="$CI/target-hjv-release" cargo build --release --locked) ||
      fail "e2e: hjv-release build"
    E2E_MINVER+=(HJV_RELEASE="$CI/target-hjv-release/release/hjv-release")
  fi
}

e2e_stage() {
  local work="$CI/e2e" mode extra=()
  # a client given by hand would go without the build's unit tests and, built without the E2E's
  # keys, without the account E2E
  [ -z "$E2E_CLIENT_GIVEN" ] ||
    fail "e2e: E2E_CLIENT is not taken here: the stage builds the client from client/patches, runs its unit tests and the account E2E with it"
  sudo -n true 2> /dev/null || fail "e2e: needs sudo without a password prompt (network namespaces, iptables)"
  if [ ! -x "$CI/target/release/hbbs" ] || [ ! -x "$CI/target/release/hbbr" ] || [ ! -x "$PROBE" ]; then
    fail "e2e: hbbs, hbbr and hjv-probe are not built: scripts/ci.sh server"
  fi
  fetch_caddy
  e2e_server_key "$work"
  e2e_keys "$work/api"
  build_e2e_client "$work" hjviewer_account hjv
  if [ -n "${HJV_CI_RUN:-}" ] && [ -n "$E2E_ACCOUNT" ]; then
    e2e_client_stamp "$work" "$E2E_CLIENT" > "$CI/target-e2e/e2e-client.stamp"
  fi
  # the PC's client where Linux counts it as installed, as its account binding needs; the
  # controllers keep running the build output
  step "e2e: the PC's client as $E2E_PC_DIR/rustdesk"
  trap cleanup_e2e EXIT
  sudo install -D -m 755 "$E2E_CLIENT" "$E2E_PC_DIR/rustdesk"
  if [ -n "$E2E_ACCOUNT" ]; then
    build_e2e_api
    build_e2e_client_old "$work"
    fetch_faketime
    extra=(HJV_E2E_ACCOUNT=1 HJV_API="$E2E_API" HJV_FAKE_SSO="$E2E_FAKE_SSO" HJV_E2E_KEYS="$work/api"
      CLIENT_OLD="$E2E_CLIENT_OLD" FAKETIME_LIB="$E2E_FAKETIME")
  fi
  e2e_minimum_version
  extra+=("${E2E_MINVER[@]}")
  e2e_access
  extra+=("${E2E_ACCESS[@]}")
  for mode in strict open; do
    step "e2e: tools/e2e/firewall-e2e.sh $mode (output also in build/ci/e2e-$mode.log)"
    with_skips "$CI/e2e-$mode.log" env HJV_E2E_WORK="$work" CADDY="$CADDY_BIN" HBBS="$CI/target/release/hbbs" \
      HBBR="$CI/target/release/hbbr" PROBE="$PROBE" CLIENT="$E2E_CLIENT" PC_CLIENT="$E2E_PC_DIR/rustdesk" "${extra[@]}" \
      "$ROOT/tools/e2e/firewall-e2e.sh" "$mode" || fail "e2e: firewall-e2e.sh $mode"
  done
  at_least "e2e: PASS lines (strict)" "$(grep -c '^PASS' "$CI/e2e-strict.log" || true)" "$E2E_LEAST_STRICT"
  at_least "e2e: PASS lines (open)" "$(grep -c '^PASS' "$CI/e2e-open.log" || true)" "$E2E_LEAST_OPEN"
  cleanup_e2e
  trap - EXIT
}

# Without privileges: tools/e2e/wol-e2e.sh runs its server, PC, NAS and networks in namespaces of
# this user (design v3 8-10, docs/e2e.md). It gets the same programs as the E2E, and hjv-relay
# built for hjv.test with the E2E's command key. The client's unit tests are those of its wake
# report here; the E2E runs once the client has the report's command (client patch 0035), and
# fails without it once client/patches has that patch. In a prepush run it takes the e2e stage's
# client when that was built from the same inputs (e2e_client_stamp).
wol_stage() {
  local work="$CI/e2e" built="$CI/target-e2e/release/rustdesk"
  if [ ! -x "$CI/target/release/hbbs" ] || [ ! -x "$CI/target/release/hbbr" ] || [ ! -x "$PROBE" ]; then
    fail "wol: hbbs, hbbr and hjv-probe are not built: scripts/ci.sh server"
  fi
  fetch_caddy
  e2e_server_key "$work"
  e2e_keys "$work/api"
  if [ -z "$E2E_CLIENT_GIVEN" ] && [ -n "${HJV_CI_RUN:-}" ] && [ -x "$built" ] &&
    [ "$(cat "$CI/target-e2e/e2e-client.stamp" 2> /dev/null)" = "$(e2e_client_stamp "$work" "$built")" ]; then
    E2E_CLIENT=$built
    echo "wol: the client of this prepush run's e2e stage, built from the same client/patches and keys" \
      "(its unit tests there, --lib hjv, had the wake report's)"
  else
    build_e2e_client "$work" hjviewer_wol
  fi
  if ! grep -q -a -- '--hjv-e2e-wol-report' "$E2E_CLIENT"; then
    # once client/patches has the wake report (0035), its command is required: a dropped patch, a
    # build without hjv-e2e or another E2E_CLIENT would otherwise pass without the E2E
    if grep -q -- '--hjv-e2e-wol-report' "$ROOT"/client/patches/rustdesk/*.patch; then
      fail "wol: the client has no --hjv-e2e-wol-report, though client/patches has the wake report"
    fi
    skip "the WOL E2E (the client has no --hjv-e2e-wol-report: client/patches has no wake report yet)"
    return 0
  fi
  build_e2e_api
  step "wol: hjv-relay for hjv.test with the E2E's command key"
  (cd "$ROOT/relay" && HJV_SERVER=hjv.test HJV_COMMAND_PUBKEY="$(cat "$work/api/command.pub")" \
    CARGO_TARGET_DIR="$CI/target-relay" cargo build --release --locked) || fail "wol: hjv-relay build"
  step "wol: tools/e2e/wol-e2e.sh (output also in build/ci/wol-e2e.log)"
  with_skips "$CI/wol-e2e.log" env HJV_E2E_WORK="$work" CADDY="$CADDY_BIN" HBBS="$CI/target/release/hbbs" \
    HBBR="$CI/target/release/hbbr" PROBE="$PROBE" CLIENT="$E2E_CLIENT" HJV_API="$E2E_API" HJV_FAKE_SSO="$E2E_FAKE_SSO" \
    HJV_RELAY="$CI/target-relay/release/hjv-relay" HJV_E2E_KEYS="$work/api" \
    "$ROOT/tools/e2e/wol-e2e.sh" || fail "wol: wol-e2e.sh"
  at_least "wol: PASS lines" "$(grep -c '^PASS' "$CI/wol-e2e.log" || true)" "$WOL_LEAST"
}

# The push gate of design v3 11 steps 1-2: none of the values of build/leak-patterns.txt (a local
# list git ignores; docs/ci.md says what goes in) in tracked files, staged changes and the commits
# a push would publish. Prints places and list line numbers, never the values.
leak_stage() {
  step "leak: tests of scripts/leak-check.py (made-up values, throwaway repositories)"
  python3 -I -B -m unittest discover -s "$ROOT/scripts" -p 'test_leak_check.py' || fail "leak: tests of the leak check"
  step "leak: the listed values in what a push would publish"
  python3 -I -B "$ROOT/scripts/leak-check.py" --repo "$ROOT" || fail "leak: see above; push nothing until this passes"
}

# The head of a stage's log: the commits it ran on (this repository; the client series and hbbs
# checkouts under upstream/, which client/patches and server/patches are exported from), the
# toolchain and the start
log_head() { # <stage>
  local name dir
  echo "LOG scripts/ci.sh $1 (prepush, HJV_CI_STRICT=1, CI directory $CI)"
  for name in hjviewer client-series hbbs; do
    case $name in
      hjviewer) dir=$ROOT ;;
      client-series) dir=$ROOT/upstream/rustdesk ;;
      hbbs) dir=$ROOT/upstream/rustdesk-server ;;
    esac
    if git -C "$dir" rev-parse -q --verify HEAD > /dev/null 2>&1; then
      echo "$name $(git -C "$dir" rev-parse HEAD) tree $(git -C "$dir" rev-parse 'HEAD^{tree}')" \
        "status-lines $(git -C "$dir" --no-optional-locks status --porcelain | wc -l) ($dir)"
    else
      echo "$name - (no checkout in $dir)"
    fi
  done
  echo "TOOLCHAIN $(rustc --version) / $(cargo --version) / $(rustc +1.75.0 --version 2> /dev/null || echo 'no rustc 1.75.0')" \
    "/ $(python3 --version 2>&1) / bash $BASH_VERSION"
  echo "START $(date '+%Y-%m-%d %H:%M:%S %z')"
}

# The gate before a push (design v3 11 step 1): the stages one after another on a clean tree, under
# this run's lock, with HJV_CI_STRICT=1, each with its own log, stopping at the first that fails.
# Named stages run alone, in this order: after a fix, those that read what it changed (docs/ci.md).
prepush() { # [<stage>...]
  local stages=() s log logs head start status passed list
  for s in "$@"; do
    [[ " $PREPUSH_STAGES " == *" $s "* ]] || fail "prepush: no stage $s ($PREPUSH_STAGES)"
  done
  for s in $PREPUSH_STAGES; do
    if [ $# -eq 0 ] || [[ " $* " == *" $s "* ]]; then stages+=("$s"); fi
  done
  [ -z "$(git -C "$ROOT" status --porcelain)" ] ||
    fail "prepush: the working tree is not clean (git status): a push publishes commits, and they are what is checked"
  [ -z "$E2E_CLIENT_GIVEN" ] || fail "prepush: E2E_CLIENT is set: prepush builds the E2E's client from client/patches"
  if [[ " ${stages[*]} " == *" leak "* ]]; then
    # the leak stage comes last; without its list it would fail only then
    list=${HJV_LEAK_PATTERNS:-$ROOT/build/leak-patterns.txt}
    if [ -z "${HJV_LEAK_PATTERNS:-}" ] && [ ! -f "$list" ]; then
      list="$(dirname "$(git -C "$ROOT" rev-parse --path-format=absolute --git-common-dir)")/build/leak-patterns.txt"
    fi
    [ -f "$list" ] || fail "prepush: no list of values for the leak stage ($list; HJV_LEAK_PATTERNS, docs/ci.md)"
  fi
  head=$(git -C "$ROOT" rev-parse HEAD)
  logs=${HJV_CI_LOGS:-$CI/prepush/$(date '+%Y%m%d-%H%M%S')-$(git -C "$ROOT" rev-parse --short HEAD)}
  mkdir -p "$logs"
  HJV_CI_RUN="$(date +%s)-$$"
  export HJV_CI_RUN
  step "prepush: ${stages[*]} on $head, logs in $logs"
  for s in "${stages[@]}"; do
    log="$logs/$s.log" start=$(date +%s) status=0
    (
      trap 'echo "EXIT $? $(date "+%Y-%m-%d %H:%M:%S %z")"' EXIT
      log_head "$s"
      HJV_CI_STRICT=1 "$SELF" "$s"
    ) > "$log" 2>&1 < /dev/null || status=$?
    passed=$(grep -x "== CI passed: $s (0 skipped)" "$log" || true)
    echo "prepush: $s: exit $status, $(($(date +%s) - start)) s, $(grep -c '^skipped:' "$log" || true) skipped line(s), log $log"
    [ "$status" = 0 ] && [ -n "$passed" ] || fail "prepush: $s did not pass ($log)"
    if grep '^skipped:' "$log"; then fail "prepush: $s printed the skipped lines above ($log)"; fi
    [ "$(git -C "$ROOT" rev-parse HEAD)" = "$head" ] && [ -z "$(git -C "$ROOT" status --porcelain)" ] ||
      fail "prepush: the tree changed during $s; its log is not of $head"
  done
}

# The last lines of a run: what it skipped, and with HJV_CI_STRICT=1 a failure if anything
finish() { # <target>
  local s
  if [ ${#SKIPPED[@]} -gt 0 ]; then
    printf '\n%s skipped in this run:\n' "${#SKIPPED[@]}"
    for s in "${SKIPPED[@]}"; do echo "  - $s"; done
    if [ "${HJV_CI_STRICT:-}" = 1 ]; then
      echo "FAIL: HJV_CI_STRICT=1 and $1 skipped ${#SKIPPED[@]} part(s), listed above: a pass with skipped lines is no pass before a push" >&2
      exit 1
    fi
  fi
  step "CI passed: $1 (${#SKIPPED[@]} skipped)"
}

case "$TARGET" in
  server) server_stage ;;
  client) client_stage ;;
  deploy) deploy_stage ;;
  e2e) e2e_stage ;;
  wol) wol_stage ;;
  leak) leak_stage ;;
  all) server_stage; deploy_stage ;;
  prepush) shift; prepush "$@" ;;
  *)
    echo "usage: $0 [all|prepush [<stage>...]|server|client|deploy|e2e|wol|leak]" >&2
    echo "  all is the server and deploy stages only; prepush runs every stage ($PREPUSH_STAGES) with HJV_CI_STRICT=1" >&2
    exit 2
    ;;
esac
finish "$TARGET"

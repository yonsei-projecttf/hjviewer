#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# CI run on the development machine (GitHub CI is paused; docs/ci.md).
#   scripts/ci.sh [all]    the server and deploy stages only (prepush runs every stage)
#   scripts/ci.sh prepush [--plan] [--drill[=full|quick]] [<stage>...]
#                          the gate before a push (design v3 11 step 1), on a clean tree, each unit
#                          with HJV_CI_STRICT=1 and its own log (build/ci/prepush/<time>-<commit>/;
#                          first the commits, the toolchain and the start, last the exit status):
#                          leak and what the stages need of this machine (preflight), both without
#                          the CI lock; the server stage, its parts side by side; the deploy stage
#                          with the client stage, e2e:build and the drill's build beside it; the e2e
#                          stage with the drill beside it; the wol stage. A failing unit stops what
#                          has not started. A unit (each stage, the server stage's parts, the drill)
#                          whose strict pass of the last day had the same inputs (the files of the
#                          tree it may read, its code in this script, its tools) is reused instead
#                          (prepush_run, reuse_plan; HJV_CI_FRESH=1 reuses nothing,
#                          HJV_CI_REUSE_HOURS=<hours> sets the age); --plan says what would run or
#                          be reused and why, and runs nothing; --drill adds the operations drill
#                          (tools/e2e/hjv-api-drill.sh, full or quick). summary.txt there says what
#                          each unit took and how it ended, times.tsv holds each stage's steps by
#                          time (scripts/ci-times.sh adds them up over runs). With stages named,
#                          only those (docs/ci.md). A stage run on its own starts with the same head.
#   scripts/ci.sh server   four parts side by side, each with its own log (server:<part>[,<part>]
#                          runs some of them, also as a stage of prepush): hbbs, a fresh upstream
#                          checkout with server/patches (the one there before kept when it has the
#                          same patched tree) and all of its tests in release (the library's and
#                          binaries' unit tests, the signaling, relay, protobuf, hbbs hjv-pk, lowest
#                          controller version and access ticket tests, against hjv-api's shared
#                          vectors), each test binary in a network namespace of its own, hjv-probe
#                          and hjv-load; api, hjv-api's tests (in namespaces of their own too) and
#                          release build (server/api); relay, hjv-relay's tests (relay/); release,
#                          tools/hjv-release's format, lints, tests (its shared test vectors among
#                          them) and release build
#   scripts/ci.sh client   the offline tests of the Google Play upload script and of the build-key
#                          check (scripts/test_*.py, the key files also against hjv-release), the store
#                          run's: the store texts (scripts/check-store-text.py and its tests), the
#                          release record, the workflows' store rules and, with Node 20 or later,
#                          the App Store Connect script (node --test), actionlint on the workflows
#                          when it is installed, then a fresh upstream checkout with
#                          client/patches, the release records' tests (scripts/release-info.py, on
#                          the patched tree and 1.9.1's APK and IPA in scripts/testdata),
#                          hbb_common's WebSocket tests, cargo +1.75.0 check (the app builds'
#                          toolchain), the client core's agreement with the shared vectors of
#                          tools/hjv-release (the trust rules) and the client's pure Dart tests,
#                          with the Flutter SDK the app builds use (downloaded once, checked
#                          against Flutter's release index). Tests that need the generated bridge
#                          run in client-build.yml.
#   scripts/ci.sh deploy   docs/server-runbook.md's deployment, as on the server, from a copy of
#                          this working tree: the release package (scripts/package-server.sh), its
#                          systemd units on this machine (hjv-api's too) behind
#                          deploy/nginx-hjviewer.conf, also added to an existing site, the whole
#                          package installed in a clean Ubuntu container (systemd-analyze verify,
#                          nginx -t, requests through nginx to hjv-api), hjv-api's production
#                          settings (scripts/check-api-settings.py and its offline tests), and the
#                          account API's nginx limits (an IPv6 /64 and a dual-stack listener too,
#                          in an unprivileged network namespace). What production does not use,
#                          Apache (deploy/apache-hjviewer.conf) in front of those units, Docker's
#                          hbbs/hbbr on 127.0.0.1 (docker-compose.behind-proxy.yml) and path A
#                          (Docker and Caddy), runs only when what it reads changed since it last
#                          passed (deploy_part; HJV_CI_DEPLOY_ALL=1 runs it anyway). Everything
#                          listens on 127.0.0.1; ports 80/443 stay untouched. Beside the checks,
#                          hjv-relay's packages (scripts/package-relay.sh, relay/tests/package.rs
#                          --ignored: a static build for x86_64-unknown-linux-musl, which needs
#                          that Rust target and musl-gcc).
#   scripts/ci.sh e2e[:build]
#                          tools/e2e/firewall-e2e.sh strict and open (sudo: namespaces, iptables)
#                          with a client built for its test server, Caddy pinned by SHA-256, and the
#                          PC's client installed as /usr/local/lib/hjviewer-e2e/rustdesk; with the
#                          account code in the patches, also the unit tests of the client's
#                          HJViewer code (--lib hjv) in that build and the account scenarios
#                          (docs/e2e.md) against hjv-api and the fake SSO, with Ubuntu's libfaketime
#                          unpacked under build/ci; with server patch 0005, hbbs's lowest controller
#                          version, and with the update code in the patches (src/hjviewer_update),
#                          a client with the update test keys and the signed version policy that
#                          tools/hjv-release signs for it (V1, V2, V4-V7 and V9), and the
#                          update code's unit tests once more in a build without those keys. With
#                          HJV_E2E_PARTS=<part>[,<part>...] only those parts of strict mode run, a
#                          partial run for fix loops that is never a pass (HJV_CI_STRICT=1 refuses it).
#                          e2e:build makes only what needs no port and no sudo: the client with its
#                          unit tests, hjv-api and the fake SSO, Caddy and libfaketime
#   scripts/ci.sh wol      tools/e2e/wol-e2e.sh, the Wake-on-LAN E2E (docs/e2e.md), without sudo: in
#                          this user's own namespaces a PC that sleeps is woken through hjv-api
#                          and hjv-relay, with the E2E's client, Caddy, hjv-api and fake SSO, and
#                          hjv-relay built for the test server with the E2E's command key; before
#                          it, the unit tests of the client's wake report (src/hjviewer_wol)
#   scripts/ci.sh drill:full|drill:quick|drill:build
#                          the operations drill of hjv-api (tools/e2e/hjv-api-drill.sh,
#                          docs/hjv-api-drill.md), full or quick, on what drill:build builds and
#                          stages (in build/ci/target-drill; built first when it is not this tree's)
#   scripts/ci.sh leak     the push gate (design v3 11 steps 1-2): no value of the local list
#                          build/leak-patterns.txt in tracked files, unpushed commits or staged
#                          changes (scripts/leak-check.py); and the checks of prepush's reuse: its
#                          rules against the code the units run and docs/ci.md's re-run table
#                          (reuse_check), and the tests of the reuse and of the lock
#   scripts/ci.sh inputs <dir> <unit>...
#                          what prepush's units read, the list whose SHA-256 keys their passes
#                          (reuse_inputs), in <dir>/<unit>.txt
# A stage skips, with a line "skipped: ..." saying so, what needs a work package that is not in the
# tree yet or a tool this machine lacks. Every skip is counted, those that the E2E scripts and the
# other checks print too, and the last line says how many: "CI passed: <stage> (<n> skipped)". A run
# is strict (HJV_CI_STRICT=1, unless HJV_CI_STRICT=0 says otherwise or the run is a partial E2E): a
# run with a skip fails at its end with the list, since a pass with skipped lines is no pass before
# a push, and a run of its own first looks at what the stage needs of this machine (preflight).
# hjv-api's and hjv-relay's tests run with the output of the tests that pass, so that a test that
# skips itself is counted too. A run that skipped nothing fails when an E2E has fewer PASS lines, or
# a client unit test filter fewer passed tests, than the last full run (E2E_LEAST_*, WOL_LEAST,
# UNIT_LEAST below).
# Under each step's title a line says when it began, how far into the run, the machine's load and
# how long the step before took; before its last line a run prints its steps by time.
# One run at a time on a CI directory (HJV_CI_DIR, default build/ci): a run holds the lock
# <CI directory>/ci.lock and says who it is in ci.lock.holder; a run started meanwhile stops at once
# with what that says, or with HJV_CI_WAIT=<seconds> waits that long for it (lock_run). What reads
# only the tree and writes nothing under the CI directory runs without it: the leak stage, inputs,
# prepush --plan and the first part of prepush (its log folder, leak, the preflight).
# Needs cargo, curl, openssl, Docker with compose v2 (used through sudo, as the runbook does), and
# apt-get for Ubuntu's nginx and Apache packages, which are unpacked under build/ci, not installed.
set -euo pipefail

# the tree's physical path, as cargo and the tests it builds give theirs (CARGO_MANIFEST_DIR); the
# tests of this script source it (scripts/test_ci_reuse.py)
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
SELF="$ROOT/scripts/$(basename "${BASH_SOURCE[0]}")"
CI="${HJV_CI_DIR:-$ROOT/build/ci}"
mkdir -p "$CI"
TARGET=${1:-all}
PROJECT=hjv-ci                     # compose project of the test stack, apart from a real deployment
PROBE="$ROOT/tools/hjv-probe/target/release/hjv-probe"
LOAD="$ROOT/tools/hjv-load/target/release/hjv-load"
API="$CI/target-api/release/hjv-api"  # the server stage's release build of hjv-api
# what the server stage builds hbbs and hbbr, hjv-api and hjv-release from (stamp_binary): the upstream
# pins and server/patches, and the crates' sources (not their tests and docs)
HBBS_INPUTS=(server/patches scripts/upstream.env scripts/fetch-upstream.sh)
API_INPUTS=(server/api/Cargo.toml server/api/Cargo.lock server/api/src)
RELEASE_INPUTS=(tools/hjv-release/Cargo.toml tools/hjv-release/Cargo.lock tools/hjv-release/src)
# Caddy for the E2E: the release archive, pinned by its SHA-256 (taken when the release's SHA-512
# list was checked, 2026-10-12)
CADDY_VERSION=2.11.7
CADDY_SHA256=727b91701a392de6ebc5027509f548bf39979e5216340d0faed8fa5e69c84f8b
# where the E2E installs the PC's client: Linux counts only programs under /usr as installed, and
# hjv-api adds only installed PCs to an account (design v3 7-4)
E2E_PC_DIR=/usr/local/lib/hjviewer-e2e
# The E2E's clients are test builds (feature hjv-e2e) of a checkout whose crates cargo compiles
# anew each time client/patches gives another tree (build_e2e_client keeps it otherwise). Without
# link-time optimization and in 16 units the client takes under a minute once its dependencies are
# built, instead of three and a half minutes with the release profile as it is. Its unit tests
# build in the test profile, with rustc's incremental cache, as the client stage's do.
E2E_PROFILE=(CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16)
# the stages of prepush (prepush_run says in which order and beside what they run)
PREPUSH_STAGES="leak server client deploy e2e wol"
# the side logs in the CI directory that prepush keeps with a stage's log, those the stage wrote:
# the image build's and the server package's are in no stage log, and the next run writes over all
declare -A SIDE_LOGS=([deploy]="build.log package.log container.log api-settings.log relay-package.log deploy-offline.log")
# The fewest PASS lines of the E2E scripts and the fewest client unit tests that the last full run
# passed with (2026-10-13). A run that skipped nothing and has fewer lost a scenario or a test (one
# moved under a condition that does not hold, a test filter that picks nothing), and the stage
# fails (at_least). Raise them as scenarios and tests come; lower one only with what was taken out.
# The historical-client comparisons X2 and X3 were removed (2026-10-18): 60 - 2.
E2E_LEAST_STRICT=58 E2E_LEAST_OPEN=2 WOL_LEAST=20
# strict: and the open login's S28-S31 (2026-10-14)
E2E_LEAST_STRICT=$((E2E_LEAST_STRICT + 4))
# strict: the minimum version's V1, V2, V4-V7 and V9 (12 - V3 - V8, 2026-10-18)
E2E_LEAST_STRICT=$((E2E_LEAST_STRICT + 10))
# strict: and HJViewer's ID rule's S32 (2026-10-14)
E2E_LEAST_STRICT=$((E2E_LEAST_STRICT + 1))
# strict: the staff-only gate's A1-A11, A13-A14 and C1-C7 (14 - A12, 2026-10-18)
E2E_LEAST_STRICT=$((E2E_LEAST_STRICT + 13))
# strict: and the support codes' C1-C7 (2026-10-20)
E2E_LEAST_STRICT=$((E2E_LEAST_STRICT + 7))
# strict: and 1.9.0's lines (2026-10-20): TV-W2's H-hide 1, TV-W3's EL1-EL4 with EL3's 'always' 5,
# TV-W5's S33-S38 with S38's [끄기] 7, TV-W9's I1-I5 5, TV-W6's W6-E1-E7 with E4b 8 (S5′, S6′, S9′,
# X1′ and A6′ change lines that were there)
E2E_LEAST_STRICT=$((E2E_LEAST_STRICT + 26))
# strict: and 1.9.1's in-app account deletion (2026-10-26): DEL1 4, DEL2 1, DEL3 2, DEL4 2
E2E_LEAST_STRICT=$((E2E_LEAST_STRICT + 9))
# strict: and the relay window's line, the default hold on the base path (2026-10-27)
E2E_LEAST_STRICT=$((E2E_LEAST_STRICT + 1))
# by the filter of build_e2e_client's cargo test --lib <filter>; hjv as the staff-only gate's E2E
# counted it (260, 2026-10-17, the series with src/hjviewer_access); 345 with 1.9.1's series
# (2026-10-26: 1.9.0's run counted 344, client patch 0160 adds one)
declare -A UNIT_LEAST=([hjv]=345 [hjviewer_wol]=6)
# and the update code's tests in the e2e stage's build without the update keys (2026-10-14; 159 as
# 1.9.0's run and 1.9.1's series count them, 2026-10-26)
UNIT_LEAST[hjviewer_update-nokeys]=159
# and the cases of the shared vectors the client core runs in the client stage (tools/hjv-release,
# 2026-10-14; 143 with 1.9.1's App Store field, 2026-10-26)
VECTORS_LEAST=143
export PATH="$HOME/.cargo/bin:$PATH"

SRC="$CI/src" PKGS="$CI/pkgs" CERTS="$CI/certs"
NGINX_PID="" APACHE_PID="" ECHO_PID=""
# set by the deploy stage, whose failures show its containers' and web servers' logs
DEPLOY_LOGS=""
# the deploy stage's jobs beside its checks, by the file they write (deploy_job)
declare -A DEPLOY_JOBS=()
# set by build_e2e_client when the E2E has its account part
E2E_ACCOUNT=""
# and when its client has the update test keys (the minimum version's policy); the build variables
# of both (e2e_client_kind)
E2E_KEYED="" E2E_BUILD_KEYS=()
# set by e2e_minimum_version
E2E_MINVER=()
# a client given by hand (E2E_CLIENT=<binary>, the wol stage only), and the E2E's client in use
E2E_CLIENT_GIVEN=${E2E_CLIENT:-} E2E_CLIENT=""
# the stack under test: the compose files (docker) or the release package's systemd units
STACK=docker SYSTEMD_SINCE=""
# what this run skipped (skip, count_skips)
SKIPPED=()
# the step under way and when it began, and the steps before it with their seconds (step_times)
STEP="" STEP_SINCE=0 STEP_TIMES=()
# the target directories this run builds in (own_target)
OWNED=()

# A step's title, and under it the time, the seconds since the run began, the machine's load (a
# slow step on a busy machine) and how long the step before took
step() {
  local took=""
  if [ -n "$STEP" ]; then
    took=$((SECONDS - STEP_SINCE))
    STEP_TIMES+=("$took $STEP")
  fi
  STEP=$* STEP_SINCE=$SECONDS
  printf '\n== %s\n' "$*"
  echo "-- $(date +%T), $SECONDS s in, load $(cut -d' ' -f1 /proc/loadavg)${took:+; the step before: $took s}"
}
# The steps by their time, the last one's until now; finish prints it (a table of one says nothing)
step_times() {
  if [ -n "$STEP" ]; then
    STEP_TIMES+=("$((SECONDS - STEP_SINCE)) $STEP")
    STEP=""
  fi
  [ ${#STEP_TIMES[@]} -gt 1 ] || return 0
  printf '\n-- time by step, %s s in all:\n' "$SECONDS"
  printf '%s\n' "${STEP_TIMES[@]}" | sort -s -rn -k1,1 | while read -r took what; do printf '%6d s  %s\n' "$took" "$what"; done
}
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
# The target directories of a CI directory serve every tree that runs on it (HJV_CI_DIR), and cargo
# judges freshness by file times, with unit names that do not depend on the tree: a tree whose files
# are older takes what another tree built there for its own, test executables with that tree's paths
# compiled in (CARGO_MANIFEST_DIR) among them. So each directory names the tree that built there last
# (.hjv-tree), and the end of a run that built there touches that mark (owned_built). Before another
# tree builds there, or this one after anything else did (units compiled after the mark: a
# scripts/ci.sh that has no marks yet, cargo run by hand), the packages that cargo builds from a path
# in <workspace> (with any of its features) are built again in both profiles, whatever tree built
# them; the crates.io and git dependencies stay built. This tree's crates are cleaned, and
# cargo clean -p also removes what other trees left there under names of their own (hjv-api's tests:
# hjv-relay lies outside server/api, so the tree goes into their names). An upstream checkout made
# from this tree's patches is made anew in the same place for every tree, so its crates' units have
# the same names whatever tree built them, and it is enough to remove those units' fingerprints and
# build script folders: cargo builds them again, and their incremental caches stay (cleaning them
# cost the client stage 43 s). The checkouts that a run keeps while a fresh one has the same tree
# (server_checkout, build_e2e_client) have no mark: what another tree built from them is this
# tree's too. A directory is this run's (OWNED) once its mark names this tree.
own_target() { # <target dir> <workspace dir>
  local dir=$1 ws=$2 last name names=() specs=() units=() unit start=$SECONDS
  [[ " ${OWNED[*]} " != *" $dir "* ]] || return 0
  last=$(cat "$dir/.hjv-tree" 2> /dev/null || true)
  if [ "$last" = "$ROOT" ]; then
    if [ -z "$(find "$dir" -mindepth 4 -maxdepth 4 -path '*/.fingerprint/*/invoked.timestamp' -newer "$dir/.hjv-tree" -print -quit)" ]; then
      OWNED+=("$dir")
      return 0
    fi
    last="$ROOT, then something else"
  fi
  while read -r name; do names+=("$name") specs+=(-p "$name"); done < <(cd "$ws" &&
    cargo metadata --format-version 1 --locked --all-features --filter-platform "$(rustc -vV | sed -n 's/^host: //p')" |
    python3 -I -c 'import json, sys
for p in json.load(sys.stdin)["packages"]:
    if p["source"] is None:
        print(p["name"])')
  [ ${#names[@]} -gt 0 ] || fail "cargo metadata named no packages of $ws"
  if [[ $ws == "$CI"/* ]]; then
    # <package>-<16 hex digits>, a unit's folder
    for unit in "$dir"/{debug,release}/{.fingerprint,build}/*; do
      name=${unit##*/}
      if [[ ${name##*-} =~ ^[0-9a-f]{16}$ ]] && [[ " ${names[*]} " == *" ${name%-*} "* ]]; then units+=("$unit"); fi
    done
    [ ${#units[@]} -eq 0 ] || rm -rf "${units[@]}"
  else
    (cd "$ws" && cargo clean --locked "${specs[@]}" --target-dir "$dir" &&
      cargo clean --locked --release "${specs[@]}" --target-dir "$dir") || fail "cargo clean in $dir"
  fi
  mkdir -p "$dir"
  echo "$ROOT" > "$dir/.hjv-tree"
  OWNED+=("$dir")
  echo "$(basename "$dir"): built last from ${last:-a tree it does not name}; this tree's packages" \
    "(${names[*]}) cleaned in $((SECONDS - start)) s${units[*]:+ (${#units[@]} fingerprints and build script folders)}"
}
# At the end of a run: what it built in its target directories is older than their marks
owned_built() {
  local dir
  for dir in "${OWNED[@]}"; do touch "$dir/.hjv-tree"; done
}
# What a binary that one stage leaves for others is built from: hbbs and hbbr (HBBS_INPUTS), hjv-api
# (API_INPUTS), and hjv-release (RELEASE_INPUTS), which the owner may sign version policies with too.
# <binary>.stamp names the commit and its changed files, a hash of the files the binary is built from
# (as git sees them: tracked, or not ignored) and of the toolchain, the binary's hash, and last the
# tree (a path may hold spaces).
inputs_hash() { # <path in this tree...>
  {
    git -C "$ROOT" ls-files -z --cached --others --exclude-standard -- "$@" | LC_ALL=C sort -z -u |
      { env -C "$ROOT" xargs -0 -r sha256sum 2>&1 || true; }
    cargo --version
  } | sha256sum | cut -d' ' -f1
}
stamp_binary() { # <binary> <path in this tree...>
  local bin=$1
  shift
  echo "commit $(git -C "$ROOT" rev-parse HEAD) changed-files $(git -C "$ROOT" status --porcelain | wc -l)" \
    "inputs $(inputs_hash "$@") binary $(sha256sum < "$bin" | cut -d' ' -f1) tree $ROOT" > "$bin.stamp"
  echo "$(basename "$bin").stamp: $(cat "$bin.stamp")"
}
# Why <binary> is not built from what this tree has of <path...> (another tree's or commit's files,
# another toolchain, no stamp, or a binary that changed since), on one line; nothing when it is
stamp_mismatch() { # <binary> <path in this tree...>
  local bin=$1 inputs sum
  shift
  if [ ! -x "$bin" ]; then
    echo "it is not built"
  elif ! read -r _ _ _ _ _ inputs _ sum _ 2> /dev/null < "$bin.stamp"; then
    echo "it has no stamp ($(basename "$bin").stamp)"
  elif [ "$sum" != "$(sha256sum < "$bin" | cut -d' ' -f1)" ]; then
    echo "it changed since its stamp"
  elif [ "$inputs" != "$(inputs_hash "$@")" ]; then
    echo "this tree's $* or the toolchain are not what it was built from"
  fi
}
# Says where a binary that another stage built comes from. One that is not built from what this tree
# has of <path...> (stamp_mismatch) fails the stage with HJV_CI_STRICT=1, and is a skip otherwise.
check_stamp() { # <binary> <stage that builds it> <path in this tree...>
  local bin=$1 stage=$2 name tree="" commit="" changed why
  shift 2
  name=$(basename "$bin")
  [ -x "$bin" ] || fail "$name is not built: scripts/ci.sh $stage"
  why=$(stamp_mismatch "$bin" "$@")
  read -r _ commit _ changed _ _ _ _ _ tree 2> /dev/null < "$bin.stamp" || true
  commit=${commit:0:7}
  if [ -z "$why" ]; then
    echo "$name: built from what this tree has of $* (in $tree at $commit, $changed changed files)"
    return 0
  fi
  [ "${HJV_CI_STRICT:-}" != 1 ] ||
    fail "$name is not this tree's: $why (built in ${tree:-?} at ${commit:-?}); scripts/ci.sh $stage builds it again"
  skip "this tree's own $name ($why; the one there was built in ${tree:-?} at ${commit:-?}): scripts/ci.sh $stage builds it again"
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
  owned_built
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
  local pid
  # the jobs still running when the stage stops early, and the clean container they may have left
  for pid in "${DEPLOY_JOBS[@]}"; do end_tree "$pid"; done
  DEPLOY_JOBS=()
  sudo docker rm -f hjv-ci-pkgtest > /dev/null 2>&1 || true
  stop_web
  cleanup_systemd
  # only this script's own project: its containers, network and Caddy volume
  [ -d "$SRC/deploy" ] && dc down -v --remove-orphans > /dev/null 2>&1
  return 0
}

# Ends <pid> and the processes it started. Those of another user cannot be signalled, but sudo, run
# by this user, passes the signal on to its command (e.g. the image build).
end_tree() { # <pid>
  local children child
  children=$(pgrep -P "$1" || true)
  kill "$1" 2> /dev/null || true
  for child in $children; do end_tree "$child"; done
}

# Runs <command...> beside the checks that follow, with its output in <file>: deploy_wait waits for
# it, and cleanup ends it when the stage stops before
deploy_job() { # <file> <command...>
  local log=$1
  shift
  "$@" < /dev/null > "$log" 2>&1 &
  DEPLOY_JOBS[$log]=$!
}

# The exit status of the job of deploy_job that writes <file>, once it ends
deploy_wait() { # <file>
  local status=0
  wait "${DEPLOY_JOBS[$1]}" || status=$?
  unset "DEPLOY_JOBS[$1]"
  return "$status"
}

# The output of that job once it ends, with its skipped lines counted; its failure fails the stage
show_job() { # <file> <what failed>
  local status=0
  deploy_wait "$1" || status=$?
  cat "$1"
  [ "$status" = 0 ] || fail "$2"
  count_skips "$1"
}

# Runs <command...> until it succeeds, five times a second for up to <seconds>: a server that is up
# a moment later costs that moment, not a whole second
wait_for() { # <seconds> <command...>
  local n=$1; shift
  for _ in $(seq $((n * 5))); do "$@" > /dev/null 2>&1 && return 0; sleep 0.2; done
  return 1
}

# hbbs's and hbbr's listeners, each once: TCP 21115-21119 and UDP 21116
listeners() { ss -Hltnu '( sport >= :21115 and sport <= :21119 )' | awk '{print $1, $5}' | sort -u; }
listening() { [ "$(listeners | wc -l)" -ge 6 ]; }
# Waits for all six, and half a second more, in which a late one would still show; the list goes to
# build/ci/listen.txt for the checks (exactly six, all on 127.0.0.1)
wait_listeners() {
  wait_for 10 listening || true
  sleep 0.5
  listeners | tee "$CI/listen.txt"
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

# hjv-probe and hjv-load of this tree, each built in its own folder (tools/<tool>/target): by the
# server stage, and by every stage that runs it (deploy both, e2e and wol hjv-probe); a second when
# it is built already
build_tools() { # <tool...>
  local t
  for t in "$@"; do (cd "$ROOT/tools/$t" && cargo build --release --locked) || fail "$t build"; done
}

# The vector that tests/hjv_pk.rs (server/patches/0004) checks the patched hbbs against is a
# copy of hjv-api's (server/api/src/hbbs.rs, reads_what_libsodium_signed): its four values, named.
hjv_pk_vector() { # <file>
  sed -n -E 's/^ *const (SERVER_SEED|SERVER_PK|DEVICE_PK|PEER_PK|SIGNED): &str = "([^"]*)";$/\1 \2/p' "$1" |
    sed 's/^DEVICE_PK /PEER_PK /'
}

# The server stage's parts, which server:<part>[,<part>] names, as a target and as a stage of
# prepush; server alone is all of them, in this order (server_stage)
SERVER_PARTS="hbbs api relay release"
# The one test that runs once more when it alone failed (the user's decision of 2026-10-27;
# docs/ci.md, "알려진 불안정한 시험"), by its package, test target and name: hbbs's signaling over
# WebSocket right after hbbs listens. Alone means that its test binary came to its end, with
# libtest's "test result" line of one failure: a binary that crashed or was killed after that test
# failed is no flake of it. A line "RETRY: ..." says so, and the stage's last lines count the
# retried runs.
KNOWN_FLAKY=("$CI/upstream/rustdesk-server" tcp_signaling peer_registered_over_websocket_receives_signaling)
# the test runs that this run retried (cargo_tests; server_stage adds those of its parts' runs)
RETRIED=0

# The parts that <stage> names, in the order of SERVER_PARTS: all of them for server, those it lists
# for server:<part>[,<part>]; nothing, and a failure, for any other name
server_parts() { # <stage>
  local want part picked=()
  case $1 in
    server) want=$SERVER_PARTS ;;
    server:?*) want=${1#server:}; want=${want//,/ } ;;
    *) return 1 ;;
  esac
  for part in $want; do
    [[ " $SERVER_PARTS " == *" $part "* ]] || return 1
  done
  for part in $SERVER_PARTS; do
    if [[ " $want " == *" $part "* ]]; then picked+=("$part"); fi
  done
  echo "${picked[*]}"
}

# Runs <command...> in a network namespace of its own, whose loopback is up and which reaches no
# other network, as this user: in a user namespace inside the one whose root sets the loopback up.
# The hbbs tests pick their servers' ports with bind(127.0.0.1:0) and let them go for hbbs to bind;
# in a namespace of their own nothing else can take them in between.
own_net() { # <command...>
  unshare -rn sh -c 'u=$1 g=$2; shift 2; ip link set lo up && exec unshare -U --map-user="$u" --map-group="$g" -- "$@"' \
    own_net "$(id -u)" "$(id -g)" "$@"
}

# The tests of the cargo package in <dir> as cargo test <options> [-- <test options>] runs them,
# but each test target (--lib, --bin <name>, --test <name>, and --doc where <options> pick no
# target) in a cargo test of its own, once cargo test <options> --no-run has built them all: at most
# <most> at a time, each under <wrapper> (own_net, or env for none), with its output in <log without
# .log>/<n>.log; where own_net cannot make its namespaces, one after another under env. Only the
# build shows the compiler's warnings, which every run would show again. Then all of it goes to
# <log> and stdout in cargo's order, and its skipped lines are counted. Unlike one cargo test, a
# target that fails stops none of the others: the failure comes at the end.
# The known flaky test (KNOWN_FLAKY) runs once more when it alone failed in a run of its binary
# that came to its end.
cargo_tests() { # <dir> <log> <most> <wrapper> <options...> [-- <test options...>]
  local pkg=$1 log=$2 most=$3 wrapper=$4 dir=${2%.log} opts=() pick=() args=() targets=() started=() i target status failed=()
  shift 4
  # the options that pick targets apart from the others, which every target's run takes
  while [ $# -gt 0 ] && [ "$1" != -- ]; do
    case $1 in
      --lib | --bins | --tests | --examples | --benches | --all-targets | --doc | --bin=* | --test=* | --example=* | --bench=*) pick+=("$1") ;;
      --bin | --test | --example | --bench) pick+=("$1" "$2"); shift ;;
      *) opts+=("$1") ;;
    esac
    shift
  done
  if [ $# -gt 0 ]; then shift; args=("$@"); fi
  if [ "$wrapper" = own_net ] && ! own_net true 2> /dev/null; then
    most=1 wrapper=env
    echo "no unprivileged user and network namespaces here: the test binaries run one after another"
  fi
  rm -rf "$dir"
  mkdir -p "$dir"
  env -C "$pkg" cargo test "${opts[@]}" "${pick[@]}" --no-run --message-format=json-render-diagnostics > "$dir/build.json" ||
    return
  # the test executables it built, by target, in the order cargo test runs them
  readarray -t targets < <(python3 -I -c 'import json, sys
kinds = ("lib", "bin", "test", "example", "bench")
found = set()
for line in open(sys.argv[1]):
    m = json.loads(line) if line.startswith("{") else {}
    if m.get("reason") == "compiler-artifact" and m["profile"]["test"] and m["executable"]:
        t = m["target"]
        kind = next((k for k in kinds[1:] if k in t["kind"]), "lib")
        found.add((kinds.index(kind), t["name"], t.get("doctest", False)))
for n, name, _ in sorted(found):
    print("--lib" if n == 0 else "--%s %s" % (kinds[n], name))
if sys.argv[2] == "0" and any(n == 0 and doctest for n, _, doctest in found):
    print("--doc")' "$dir/build.json" "${#pick[@]}")
  [ ${#targets[@]} -gt 0 ] || { echo "no test targets in $pkg"; return 1; }
  echo "$(IFS=,; echo "${targets[*]}"), at most $most at a time"
  for i in "${!targets[@]}"; do
    while [ "$(jobs -pr | wc -l)" -ge "$most" ]; do wait -n || true; done
    (
      read -ra target <<< "${targets[$i]}"
      status=0
      "$wrapper" env -C "$pkg" CARGO_BUILD_WARNINGS=allow cargo test "${opts[@]}" "${target[@]}" -- "${args[@]}" \
        > "$dir/$i.log" 2>&1 || status=$?
      if [ "$status" != 0 ] && [ "$pkg ${target[*]}" = "${KNOWN_FLAKY[0]} --test ${KNOWN_FLAKY[1]}" ] &&
        grep -qE '^test result: FAILED\. [0-9]+ passed; 1 failed;' "$dir/$i.log" &&
        [ "$(sed -n 's/^test \(.*\) \.\.\. FAILED$/\1/p' "$dir/$i.log")" = "${KNOWN_FLAKY[2]}" ]; then
        echo "RETRY: ${KNOWN_FLAKY[2]} (tests/${KNOWN_FLAKY[1]}.rs) failed alone; the known flaky test runs once more" \
          >> "$dir/$i.log"
        status=0
        "$wrapper" env -C "$pkg" CARGO_BUILD_WARNINGS=allow cargo test "${opts[@]}" "${target[@]}" -- "${args[@]}" \
          --exact "${KNOWN_FLAKY[2]}" >> "$dir/$i.log" 2>&1 || status=$?
      fi
      echo "$status" > "$dir/$i.status"
    ) &
    started+=($!)
  done
  wait "${started[@]}" || true
  : > "$log"
  for i in "${!targets[@]}"; do
    cat "$dir/$i.log" >> "$log"
    count_skips "$dir/$i.log"
    RETRIED=$((RETRIED + $(grep -c '^RETRY: ' "$dir/$i.log" || true)))
    [ "$(cat "$dir/$i.status")" = 0 ] || failed+=("${targets[$i]}")
  done
  cat "$log"
  [ ${#failed[@]} -eq 0 ] || { echo "the test targets that failed in $pkg: $(IFS=,; echo "${failed[*]}")"; return 1; }
}

# The upstream checkout with server/patches in $CI/upstream/rustdesk-server: fetched anew every run,
# which shows that the pinned commits and the patches still apply, but the checkout there before is
# kept when it has the new one's patched tree and nothing else (git status, hbb_common's too: only
# the version file that hbbs's build writes, once the tests' database is gone). Its files keep
# their modification times, so cargo does not compile hbbs again (fat LTO, a minute or more) for
# the same sources.
server_checkout() {
  local dir="$CI/upstream/rustdesk-server" new="$CI/upstream-new/rustdesk-server" tree left
  rm -rf "$CI/upstream-new"
  UPSTREAM_DIR="$CI/upstream-new" "$ROOT/scripts/fetch-upstream.sh" server
  tree=$(git -C "$new" rev-parse 'HEAD^{tree}')
  # what the tests leave behind: test_insert's database, whose unique IDs a second run would add again
  rm -f "$dir/test.sqlite3" "$dir/test.sqlite3-shm" "$dir/test.sqlite3-wal"
  left=$(git -C "$dir" status --porcelain --ignored --untracked-files=all 2> /dev/null || echo "no checkout")
  if [ "$(git -C "$dir" rev-parse -q --verify 'HEAD^{tree}' 2> /dev/null)" = "$tree" ] &&
    { [ -z "$left" ] || [ "$left" = "!! src/version.rs" ]; } &&
    [ "$(git -C "$dir/libs/hbb_common" rev-parse HEAD 2> /dev/null)" = "$(git -C "$dir" rev-parse HEAD:libs/hbb_common)" ] &&
    [ -z "$(git -C "$dir/libs/hbb_common" status --porcelain --ignored --untracked-files=all)" ]; then
    echo "kept $dir of an earlier run: the new checkout has the same patched tree"
  else
    rm -rf "$dir"
    mkdir -p "$CI/upstream"
    mv "$new" "$dir"
  fi
  rm -rf "$CI/upstream-new"
  echo "the patched tree: $tree"
}

# hbbs: the upstream checkout with server/patches and all of its tests, hjv-probe and hjv-load
server_hbbs() {
  # all of the patched server's tests, its upstream unit tests too: patches 0001-0004 change the
  # code they test
  local tests=(--lib --bins --tests) t
  step "server: upstream checkout with server/patches"
  server_checkout
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
  # Each test binary in a network namespace of its own, all of them side by side: they mostly wait
  # (hbbs's ten-second periods, test_insert's 10 000 synced inserts), they name their directories
  # by process, and no binary's hbbs can take the ports another has picked (own_net). (No mark on
  # $CI/target, own_target: only this checkout builds there, and server_checkout keeps it only while
  # it has the same patched tree, so what another tree built there is this one's; a mark would have
  # hbbs compiled again after every switch of trees.)
  HJV_ACCESS_VECTORS="$vectors" CARGO_TARGET_DIR="$CI/target" cargo_tests "$CI/upstream/rustdesk-server" \
    "$CI/hbbs-tests.log" "$(nproc)" own_net --release --locked "${tests[@]}" || fail "server: hbbs and hbbr's tests"
  for t in hbbs hbbr; do stamp_binary "$CI/target/release/$t" "${HBBS_INPUTS[@]}"; done
  step "server: hjv-probe and hjv-load"
  build_tools hjv-probe hjv-load
}

# api: hjv-api's tests and release build (server/api)
server_api() {
  local commit
  if [ -d "$ROOT/server/api" ]; then
    step "server: hjv-api tests and release build (server/api)"
    own_target "$CI/target-api" "$ROOT/server/api"
    # with the output of the tests that pass: a test that cannot run here (no systemd tools) passes
    # with a line "skipped: ...", which only then is seen and counted. Four test binaries at a time,
    # each in a network namespace of its own: update_stream.rs starts hjv-api on a port it picked
    # with bind(127.0.0.1:0) and let go, which a binary beside it could take meanwhile (own_net).
    # All at once end barely sooner (support.rs's 20-second long poll is the longest) and slow the
    # timed requests (login.rs's 700 ms) two- to threefold.
    CARGO_TARGET_DIR="$CI/target-api" cargo_tests "$ROOT/server/api" "$CI/api-tests.log" 4 own_net --locked -- --show-output ||
      fail "server: hjv-api tests"
    # The commit that /api/hjv/source and hjv-api version report: the last one that changed
    # server/api, the binary's source, which is the same in HEAD. HEAD itself would have cargo
    # compile hjv-api again for every commit. The binary must report it.
    commit=$(git -C "$ROOT" log -1 --format=%H -- server/api)
    (cd "$ROOT/server/api" && HJV_API_COMMIT=$commit \
      CARGO_TARGET_DIR="$CI/target-api" cargo build --release --locked --bin hjv-api) || fail "server: hjv-api release build"
    "$API" version | grep -qF "($commit)" || fail "server: $API does not report the commit of its source, $commit"
    stamp_binary "$API" "${API_INPUTS[@]}"
  else
    skip "hjv-api (no server/api)"
  fi
}

# relay: hjv-relay's tests (relay/)
server_relay() {
  local netns
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
    own_target "$CI/target-relay" "$ROOT/relay"
    with_skips "$CI/relay-tests.log" env -C "$ROOT/relay" HJV_RELAY_NETNS="$netns" CARGO_TARGET_DIR="$CI/target-relay" \
      cargo test --locked -- --show-output || fail "server: hjv-relay tests"
  else
    skip "hjv-relay (no relay/)"
  fi
}

# release: tools/hjv-release's format, lints, tests and release build
server_release() {
  if [ -d "$ROOT/tools/hjv-release" ]; then
    # the operator's tool of the signed version policy: its tests hold the shared test vectors to
    # the reference model (python3 tests/vectors/generate.py --check) and read every one; the release
    # build is the one the client stage's key-file test and the E2E use
    step "server: hjv-release format, lints, tests and release build (tools/hjv-release)"
    own_target "$CI/target-hjv-release" "$ROOT/tools/hjv-release"
    (cd "$ROOT/tools/hjv-release" && cargo fmt --check) || fail "server: hjv-release is not formatted (cargo fmt)"
    (cd "$ROOT/tools/hjv-release" && CARGO_TARGET_DIR="$CI/target-hjv-release" cargo clippy --locked --all-targets -- -D warnings) ||
      fail "server: hjv-release lints"
    with_skips "$CI/hjv-release-tests.log" env -C "$ROOT/tools/hjv-release" CARGO_TARGET_DIR="$CI/target-hjv-release" \
      cargo test --locked || fail "server: hjv-release tests"
    (cd "$ROOT/tools/hjv-release" && CARGO_TARGET_DIR="$CI/target-hjv-release" cargo build --release --locked) ||
      fail "server: hjv-release release build"
    stamp_binary "$CI/target-hjv-release/release/hjv-release" "${RELEASE_INPUTS[@]}"
  else
    skip "hjv-release (no tools/hjv-release)"
  fi
}

# The server stage: the parts that <stage> names (server_parts). One runs here. Several run side by
# side, each as a run of this script (server:<part>, under this run's lock) with its output in
# build/ci/server-<part>.log, shown in their order once it ended: they share no target directory,
# port or file, as the hbbs and hjv-api tests run in network namespaces of their own
# (cargo_tests). Side by side, a part that fails stops none of the others: the stage fails once all
# of them ended. Where this user cannot make those namespaces, the parts run here one after
# another, and the first that fails ends the stage. Its last lines say which parts ran and how many
# test runs were retried (KNOWN_FLAKY).
server_stage() { # <stage>
  local parts=() part rest=() log status failed=() n
  local -A pids=() starts=()
  read -ra parts <<< "$(server_parts "$1")"
  [ ${#parts[@]} -gt 0 ] || fail "no stage $1: server:<part>[,<part>] names parts of the server stage ($SERVER_PARTS)"
  if [ ${#parts[@]} -eq 1 ] || ! own_net true 2> /dev/null; then
    [ ${#parts[@]} -eq 1 ] || echo "no unprivileged user and network namespaces here: the server stage's parts run one after another"
    for part in "${parts[@]}"; do "server_$part"; done
  else
    step "server: ${parts[*]} side by side, the output of each also in build/ci/server-<part>.log"
    for part in "${parts[@]}"; do
      # as a job of its own, which gets back SIGINT and SIGQUIT (with_skips); a part of this run, whose
      # head is this run's (HJV_CI_PART)
      env --default-signal=INT,QUIT HJV_CI_PART=1 "$SELF" "server:$part" < /dev/null > "$CI/server-$part.log" 2>&1 &
      pids[$part]=$! starts[$part]=$(date +%s)
    done
    for part in "${parts[@]}"; do
      log="$CI/server-$part.log" status=0
      wait "${pids[$part]}" || status=$?
      printf '\n---- server:%s (%s)\n' "$part" "$log"
      cat "$log"
      count_skips "$log"
      n=$(grep -c '^RETRY: ' "$log" || true)
      RETRIED=$((RETRIED + n))
      if [ "$status" = 0 ] && ! grep -q "^== CI passed: server:$part (" "$log"; then status=1; fi
      echo "server: $part: exit $status, $(($(stat -c %Y "$log") - ${starts[$part]})) s," \
        "$(grep -c '^skipped:' "$log" || true) skipped line(s), $n retried test run(s)"
      [ "$status" = 0 ] || failed+=("$part")
    done
    [ ${#failed[@]} -eq 0 ] || fail "server: ${failed[*]} failed (above, and in build/ci/server-<part>.log)"
  fi
  for part in $SERVER_PARTS; do
    if [[ " ${parts[*]} " != *" $part "* ]]; then rest+=("$part"); fi
  done
  echo "server: the parts that ran: ${parts[*]}${rest[*]:+ (not ${rest[*]}: $1)}"
  echo "server: test runs retried: $RETRIED (only the known flaky test, once: the RETRY lines above)"
}

FLUTTER_VERSION=3.24.5 # client-build.yml FLUTTER_VERSION
# The pure Dart tests of the client patches, which run with the bridge's web stand-in. The gesture
# and quality monitor tests, and the other tests that need the generated bridge (the account, admin
# and update gate widgets, 1.9's dialog frame), run in client-build.yml.
CLIENT_DART_TESTS="test/soft_keyboard_input_test.dart test/soft_keyboard_log_test.dart test/hjviewer_account_test.dart"
# the update screens' (minimum version design 6-4), the controller-only Android app's (Play design
# 3-3) and the super operator's view's pure tests (2026-10-14)
CLIENT_DART_TESTS+=" test/hjv_update_test.dart test/hjviewer_android_test.dart test/hjviewer_admin_test.dart"
# the access screens' (staff-only gate, docs/access-tickets.md 5-3; client patches 0061-0062)
CLIENT_DART_TESTS+=" test/hjviewer_access_test.dart"
# 1.8.2 (ships in 1.9.0): the toast, the ID suggestions, the phone bar and targets, what the
# settings leave out for what a build pins, the look's contrast and line breaks, and the phone app
# lock (W13) (client patches 0065-0085)
CLIENT_DART_TESTS+=" test/hjviewer_toast_test.dart test/hjviewer_suggestions_test.dart test/hjviewer_phone_test.dart"
CLIENT_DART_TESTS+=" test/hjviewer_pins_test.dart test/hjviewer_style_test.dart test/hjviewer_applock_test.dart"
# 1.9.0: the PC connection window's marks, cautions and recording rule and its card's fit with
# Pretendard (TV-W2), the staff member's support link and waiting room (TV-W6), the session end's
# lock rule (TV-W3), the remote readiness texts (TV-W4), the phone session's gesture guide and switch
# (TV-W7), the incoming connections list (TV-W9), the home's account line (UX-H1), the desktop's
# keyboard and what screen readers hear (UX-K1, UX-K2), the session windows' colours (U15), the
# devices list's title line (D16), the Windows 11 maximise button (U44) and the phones' text size
# up to 130 % (UX-K3) (client patches 0086-0147)
CLIENT_DART_TESTS+=" test/hjviewer_cm_test.dart test/hjviewer_support_test.dart test/hjviewer_cm_fit_test.dart"
CLIENT_DART_TESTS+=" test/hjviewer_end_lock_test.dart test/hjviewer_ready_test.dart test/hjviewer_session_test.dart"
CLIENT_DART_TESTS+=" test/hjviewer_incoming_test.dart test/hjviewer_home_test.dart"
CLIENT_DART_TESTS+=" test/hjviewer_keys_test.dart test/hjviewer_semantics_test.dart"
CLIENT_DART_TESTS+=" test/hjviewer_session_look_test.dart test/hjviewer_title_line_test.dart"
CLIENT_DART_TESTS+=" test/hjv_caption_test.dart test/hjviewer_text_scale_test.dart"
# 1.9.1: the desktop settings split into the everyday tabs and Advanced settings (client patches
# 0149-0152)
CLIENT_DART_TESTS+=" test/hjviewer_settings_split_test.dart"
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
  local fl="$CI/flutter-$FLUTTER_VERSION" app="$CI/upstream/rustdesk/flutter" sum t tests=() offline status=0
  local index=https://storage.googleapis.com/flutter_infra_release/releases
  # the server stage's release build of tools/hjv-release, against which the key files and the
  # release records are read too (its tests skip that part without it); built here when it is not
  # there or not this tree's, so that the stage needs no server stage before it
  local release_bin="$CI/target-hjv-release/release/hjv-release"
  local vectors="$ROOT/tools/hjv-release/tests/vectors" ran cases why
  if [ ! -d "$ROOT/tools/hjv-release" ]; then
    release_bin=""
    skip "the key files and release records against hjv-release (no tools/hjv-release)"
  else
    why=$(stamp_mismatch "$release_bin" "${RELEASE_INPUTS[@]}")
    if [ -n "$why" ]; then
      step "client: tools/hjv-release's release build ($why)"
      own_target "$CI/target-hjv-release" "$ROOT/tools/hjv-release"
      (cd "$ROOT/tools/hjv-release" && CARGO_TARGET_DIR="$CI/target-hjv-release" cargo build --release --locked) ||
        fail "client: hjv-release release build"
      stamp_binary "$release_bin" "${RELEASE_INPUTS[@]}"
    fi
    check_stamp "$release_bin" server "${RELEASE_INPUTS[@]}"
  fi
  # The offline tests beside the rest, with their output in build/ci/client-offline.log, shown at
  # the end. A stage that fails before then waits for them, so that nothing of it runs on, and
  # shows them too, as it did when they ran first.
  client_offline "$release_bin" > "$CI/client-offline.log" 2>&1 < /dev/null &
  offline=$!
  trap 'client_offline_end "$offline" || true' EXIT
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
  # U52: a new raw font size in the app's own Dart files is told, not failed (the baseline holds
  # the deliberate ones); only the check's own test can fail the stage
  step "client: raw font sizes in hjv_*.dart and hjviewer_*.dart against scripts/hjv-fontsize-baseline.txt (warns only)"
  "$ROOT/scripts/hjv-fontsize-check.sh" --self-test || fail "client: the font size check's self-test"
  "$ROOT/scripts/hjv-fontsize-check.sh" "$app/lib" "$ROOT/scripts/hjv-fontsize-baseline.txt"
  # release.json, SHA256SUMS and the workflow's minimum-version steps, also on the patched tree (the
  # Dart tests the workflow requires) and on real packages (the manifest gate on a real APK): the
  # manifest and Info.plist of 1.9.1's APK and IPA in scripts/testdata, or the download of a run that
  # HJV_RELEASE_SAMPLES names
  step "client: the release records (scripts/release-info.py, client-build.yml), offline tests"
  HJV_RELEASE_BIN="$release_bin" HJV_PATCHED_FLUTTER="$app" \
    python3 -I -B -m unittest discover -s "$ROOT/scripts" -p 'test_release_info.py' || fail "client: release record tests"
  step "client: hbb_common WebSocket tests"
  own_target "$CI/target-client" "$CI/upstream/rustdesk"
  # One at a time: the proxy test sets the process's proxy option, and a test running beside it
  # would connect through that test's one-shot proxy
  (cd "$CI/upstream/rustdesk" && CARGO_TARGET_DIR="$CI/target-client" cargo test --locked -p hbb_common --lib websocket -- --test-threads=1) ||
    fail "client: hbb_common WebSocket tests"
  # The apps build with Rust 1.75 (client-build.yml RUST_VERSION); the development machine's
  # stable toolchain would let newer language features through (design v3 6-6)
  step "client: cargo +1.75.0 check --features linux-pkg-config,hjv-e2e"
  rustup toolchain list | grep '^1\.75\.0' > /dev/null ||
    fail "client: no Rust 1.75.0 toolchain: rustup toolchain install 1.75.0 --profile minimal"
  pkgconfig
  own_target "$CI/target-175" "$CI/upstream/rustdesk"
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
  trap - EXIT
  client_offline_end "$offline" || status=$?
  count_skips "$CI/client-offline.log"
  [ "$status" = 0 ] || fail "client: the offline tests (above)"
}

# The client stage's offline tests, which read this tree and the hjv-release build, not the upstream
# checkout, so client_stage runs them beside its checkout, cargo and Dart steps.
client_offline() { # <hjv-release's release build, or nothing>
  local lint
  # no network, no Google credentials (-B: no __pycache__ in scripts/)
  step "client: Google Play upload script, offline tests"
  python3 -I -B -m unittest discover -s "$ROOT/scripts" -p 'test_play_upload.py' ||
    fail "client: Google Play upload script tests"
  # the store run (store-release.yml, docs/store-release.md): no network, no store key
  step "client: the store texts (scripts/check-store-text.py on store/, and its tests)"
  python3 -I -B "$ROOT/scripts/check-store-text.py" || fail "client: the store texts"
  python3 -I -B -m unittest discover -s "$ROOT/scripts" -p 'test_check_store_text.py' ||
    fail "client: store text check tests"
  step "client: the release record (scripts/store-record.py) and the workflows' store rules, offline tests"
  python3 -I -B -m unittest discover -s "$ROOT/scripts" -p 'test_store_record.py' || fail "client: release record tests"
  python3 -I -B -m unittest discover -s "$ROOT/scripts" -p 'test_store_workflows.py' ||
    fail "client: the workflows' store rules"
  step "client: the App Store Connect script (scripts/asc-store.mjs), offline tests with a fake App Store Connect"
  if command -v node > /dev/null && [ "$(node -p 'process.versions.node.split(".")[0]')" -ge 20 ]; then
    node --test "$ROOT/scripts/asc-store.test.mjs" || fail "client: App Store Connect script tests"
  else
    skip "the App Store Connect script's tests (Node 20 or later is not installed)"
  fi
  step "client: the pinned public keys' check (scripts/check-build-keys.py, client-build.yml), offline tests"
  HJV_RELEASE_BIN="$1" python3 -I -B -m unittest discover -s "$ROOT/scripts" -p 'test_check_build_keys.py' ||
    fail "client: build-key check tests"
  step "client: actionlint on .github/workflows"
  if lint=$(command -v actionlint || ls "$HOME/.local/bin/actionlint" 2> /dev/null); then
    "$lint" "$ROOT"/.github/workflows/*.yml || fail "client: actionlint"
  else
    skip "actionlint is not installed (github.com/rhysd/actionlint)"
  fi
}

# Waits for client_offline's job and shows its output; its status is the job's
client_offline_end() { # <job>
  local status=0
  step "client: the offline tests, run beside the steps above (output also in build/ci/client-offline.log)"
  wait "$1" || status=$?
  cat "$CI/client-offline.log"
  return "$status"
}

# The runbook's checks against one web server: WebSocket upgrades to hbbs and hbbr, the /api/
# answer, the source notice, hjv-probe, three PCs registering (hbbs must see their ports), a relay
# session with the server key, and the same key in hbbs, hbbr and data/id_ed25519.pub.
# The checks that wait run side by side: each WebSocket upgrade waits out curl's --max-time after
# its 101 (the runbook's command), hjv-load holds the PCs for 12 s and the relay session for 7. None
# of them changes what another sees (only the PCs register with hbbs); their output goes to files
# under build/ci, shown and checked afterwards in this order.
check_site() { # <port> <CA file> <status of />, or "site" when / belongs to another site <name>
  local port=$1 ca=$2 root_status=$3 name=$4 code p before heartbeat root probe=0 peers=0 relay=0
  local url="https://localhost:$port" curl_tls=(--cacert "$ca" --resolve "localhost:$port:127.0.0.1") out="$CI/site"
  step "deploy: checks through $name on 127.0.0.1:$port"
  for p in id relay; do
    deploy_job "$out-ws-$p.code" curl -s -o /dev/null -w '%{http_code}' --max-time 5 --http1.1 "${curl_tls[@]}" \
      -H 'Connection: Upgrade' -H 'Upgrade: websocket' -H 'Sec-WebSocket-Version: 13' \
      -H 'Sec-WebSocket-Key: SGVsbG9Xb3JsZDEyMzQ1Ng==' "$url/ws/$p"
  done
  deploy_job "$out-relay.txt" "$LOAD" relay localhost --port "$port" --ca "$ca" -n 1 --mbps 2 --secs 5 \
    --key "$(sudo cat "$(key_file)")"
  heartbeat=$(curl -s -o /dev/null -w '%{http_code}' "${curl_tls[@]}" -X POST -d '{}' "$url/api/heartbeat" || true)
  if [ "$root_status" != site ]; then
    root=$(curl -s -o "$CI/root.txt" -w '%{http_code}' "${curl_tls[@]}" "$url/" || true)
  fi
  "$PROBE" localhost --port "$port" --ca "$ca" > "$out-probe.txt" 2>&1 || probe=$?
  before=$(hbbs_logs 2>/dev/null | grep -c 'update_pk ' || true)
  "$LOAD" peers localhost --port "$port" --ca "$ca" -n 3 --secs 12 > "$out-peers.txt" 2>&1 || peers=$?
  # (curl ends with 28 once the upgraded connection outlasts --max-time; its code is what counts)
  for p in id relay; do deploy_wait "$out-ws-$p.code" || true; done
  deploy_wait "$out-relay.txt" || relay=$?
  for p in id relay; do
    code=$(cat "$out-ws-$p.code")
    echo "/ws/$p $code"
    [ "$code" = 101 ] || fail "$name: /ws/$p answered $code, not 101"
  done
  echo "/api/heartbeat $heartbeat"
  [ "$heartbeat" = 404 ] || fail "$name: /api/heartbeat answered $heartbeat, not 404"
  if [ "$root_status" != site ]; then
    echo "/ $root $(head -c 80 "$CI/root.txt")"
    if [ "$root" != "$root_status" ] || ! grep -q 'github.com/yonsei-projecttf/hjviewer' "$CI/root.txt"; then
      fail "$name: / answered $root without the source notice"
    fi
  fi
  cat "$out-probe.txt"
  [ "$probe" = 0 ] || fail "$name: hjv-probe"
  cat "$out-peers.txt"
  [ "$peers" = 0 ] || fail "$name: hjv-load peers"
  # Each PC echoed a heartbeat in its 12 s: the patched hbbs sends an empty frame every 10 s
  # (TCP_HEARTBEAT_MS, server/patches/0002) and drops a PC after 30 s without a frame
  # (TCP_IDLE_TIMEOUT_MS), and the web server's 120-s timeouts count on those frames
  # (deploy/nginx-hjviewer-locations.conf): one that drops or mangles them knocks PCs offline
  [[ $(cat "$out-peers.txt") =~ at\ least\ ([0-9]+)\ by\ each\ PC ]] && [ "${BASH_REMATCH[1]}" -ge 1 ] ||
    fail "$name: a PC echoed no heartbeat of hbbs's in 12 s"
  { hbbs_logs | grep -oE 'update_pk [0-9]+ [^ ]+' || true; } | tail -n "+$((before + 1))" > "$CI/pk.txt"
  cat "$CI/pk.txt"
  [ "$(wc -l < "$CI/pk.txt")" = 3 ] || fail "$name: hbbs did not register 3 PCs"
  if grep -q ':0$' "$CI/pk.txt"; then fail "$name: X-Real-Port did not reach hbbs (port 0)"; fi
  cat "$out-relay.txt"
  [ "$relay" = 0 ] || fail "$name: hjv-load relay"
  # what one half of the session sent, the other half received, every byte
  [[ $(cat "$out-relay.txt") =~ ([0-9]+)\ bytes\ received\ of\ ([0-9]+)\ sent ]] && [ "${BASH_REMATCH[2]}" -gt 0 ] &&
    [ "${BASH_REMATCH[1]}" = "${BASH_REMATCH[2]}" ] || fail "$name: the relay did not carry every byte sent"
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
# nginx, started on its socket by systemd-socket-activate as hjviewer-api.socket would. The
# container shares nothing with the deploy stage's other checks (its own network, the package
# read-only): it runs beside them as hjv-ci-pkgtest, which cleanup removes, and
# check_package_container shows and checks its output at the end.
start_package_container() { # <release dir> <package name>
  local image="${HJV_REGISTRY:-public.ecr.aws/docker/library}/ubuntu:24.04"
  echo "the release package installed in a clean container ($image), beside the checks below (build/ci/container.log)"
  cat > "$CI/container-test.sh" << 'EOF'
set -euo pipefail
name=$1
step() { printf '\n== container: %s\n-- %s, %s s in\n' "$*" "$(date '+%T %Z')" "$SECONDS"; }
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
  sudo docker rm -f hjv-ci-pkgtest > /dev/null 2>&1 || true
  deploy_job "$CI/container.log" sudo docker run --rm --name hjv-ci-pkgtest -v "$1:/pkg:ro" \
    -v "$CI/container-test.sh:/test.sh:ro" "$image" bash /test.sh "$2"
}
check_package_container() {
  step "deploy: the release package installed in a clean container (build/ci/container.log)"
  show_job "$CI/container.log" "the release package in a clean container (build/ci/container.log)"
  [ "$(tail -n 1 "$CI/container.log")" = CONTAINER_OK ] || fail "the release package in a clean container (build/ci/container.log)"
}

# The deploy stage's offline tests, a job beside its checks (deploy_job): the production settings
# check, deploy/'s scripts for the server's own files, and the server's check of a version policy,
# also against the shared test vectors of tools/hjv-release
deploy_offline() {
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
}

# What production does not use, Apache in front of the systemd units, Docker's hbbs and hbbr behind
# a web server (docker-compose.behind-proxy.yml) and path A (Docker and Caddy), runs only when what
# it reads changed since it last passed (the user's decision, 2026-10-27; docs/ci.md). <inputs>
# prints what <run> reads besides the deploy stage's own code: its code, files, packages, images
# and tools. A pass that skipped nothing keeps the SHA-256 of all that in
# build/ci/stamps/deploy-<part>.stamp, and the next run with the same prints one line instead of
# running the part again, not a skip: the part passed with exactly that. HJV_CI_DEPLOY_ALL=1 runs
# every part.
deploy_part() { # <part> <title> <inputs> <run> [<argument>...]
  local part=$1 title=$2 inputs=$3 run=$4 stamp="$CI/stamps/deploy-$1.stamp" hash old="" since=""
  local skips=${#SKIPPED[@]}
  shift 4
  step "$title"
  hash=$(part_hash "$inputs" "$@") || fail "deploy: what the part $part reads (above)"
  read -r old since 2> /dev/null < "$stamp" || true
  if [ "${HJV_CI_DEPLOY_ALL:-}" = 1 ]; then
    echo "runs: HJV_CI_DEPLOY_ALL=1"
  elif [ "$old" = "$hash" ]; then
    echo "unchanged since $since: $part (build/ci/stamps/deploy-$part.stamp; HJV_CI_DEPLOY_ALL=1 runs it)"
    return 0
  elif [ -n "$old" ]; then
    echo "runs: what it reads changed since its pass of $since"
  else
    echo "runs: no pass of it recorded (build/ci/stamps/deploy-$part.stamp)"
  fi
  "$run" "$@"
  # a run that skipped a check is no pass (with HJV_CI_STRICT=1 it fails at the end), and an older
  # record of the same inputs (HJV_CI_DEPLOY_ALL=1) no longer says what a run of the part does here
  if [ ${#SKIPPED[@]} != "$skips" ]; then rm -f "$stamp"; return 0; fi
  # what it read in the end: path A pulls the Caddy image when it is not here
  hash=$(part_hash "$inputs" "$@") || fail "deploy: what the part $part reads (above)"
  mkdir -p "$CI/stamps"
  echo "$hash $(date '+%Y-%m-%d %H:%M') ($(git -C "$ROOT" rev-parse --short HEAD))" > "$stamp"
}
part_hash() { # <inputs> [<argument>...]
  { declare -f deploy_stage; echo "PROJECT=$PROJECT"; "$@"; } | sha256sum | cut -d' ' -f1
}

# The code of <function...> and of every function it names, as bash read it (without comments)
code_of() { # <function...>
  local todo=("$@") seen=" " f w
  while [ ${#todo[@]} -gt 0 ]; do
    f=${todo[0]}
    todo=("${todo[@]:1}")
    [[ $seen == *" $f "* ]] && continue
    seen+="$f "
    declare -f "$f"
    for w in $(declare -f "$f" | grep -oE '[a-z_][a-z0-9_]*' | sort -u); do
      if declare -F "$w" > /dev/null; then todo+=("$w"); fi
    done
  done
}

# hjv-probe and hjv-load, and the programs that check_site and the checks around it run
site_tools() {
  sha256sum < "$PROBE"
  sha256sum < "$LOAD"
  curl --version
  openssl version
  python3 --version
}

# What Apache's part reads: its site file, Ubuntu's packages, and the release package's hbbs and
# hbbr with their units, which it is in front of
apache_inputs() { # <unpacked package>
  code_of deploy_apache
  (cd "$SRC" && sha256sum deploy/apache-hjviewer.conf)
  (cd "$PKGS" && sha256sum ./*.deb)
  (cd "$1" && sha256sum bin/hbbs bin/hbbr systemd/hjviewer-hbbs.service systemd/hjviewer-hbbr.service \
    systemd/hjviewer.env.example)
  site_tools
}

# What the Docker parts read: the image's build (deploy/Dockerfile and what it copies), <compose
# file>, Docker's versions, and the images that file names as they are here, layers and settings
# (the image of hbbs and hbbr as built above, which a new base image changes)
docker_inputs() { # <compose file>
  local image
  (cd "$SRC" && find "deploy/$1" deploy/.env.example deploy/Dockerfile scripts/fetch-upstream.sh scripts/upstream.env \
    server/patches LICENSE NOTICE -type f -print0 | sort -z | xargs -0 sha256sum)
  sudo docker version --format '{{.Server.Version}} {{.Client.Version}}'
  sudo docker compose version --short
  sudo docker buildx version
  for image in $(dc -f "$1" config --images | sort -u); do
    echo "$image $(sudo docker image inspect -f '{{json .RootFS.Layers}} {{json .Config}}' "$image" 2> /dev/null || echo missing)"
  done
}
# and Docker's hbbs and hbbr behind a web server: the nginx in front, as configured for the checks
# before, with Ubuntu's package
behind_proxy_inputs() {
  code_of deploy_behind_proxy
  docker_inputs docker-compose.behind-proxy.yml
  (cd "$CI/nginx" && cat ./*.conf snippets/*.conf)
  (cd "$PKGS" && sha256sum ./nginx_*.deb)
  site_tools
}
# and path A: the Caddyfile
caddy_inputs() {
  code_of deploy_caddy
  docker_inputs docker-compose.yml
  (cd "$SRC" && sha256sum deploy/Caddyfile)
  site_tools
}

# Apache with deploy/apache-hjviewer.conf in front of the release package's systemd units, path B as
# the runbook has it for Apache (4-3); deploy_part
deploy_apache() {
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
}

# Docker's hbbs and hbbr on 127.0.0.1 (docker-compose.behind-proxy.yml, the image built before), with
# the nginx of the checks before in front; deploy_part
deploy_behind_proxy() {
  local ng="$CI/nginx"
  dc up -d
  wait_for 30 sudo test -s "$SRC/deploy/data/id_ed25519.pub" || fail "hbbs wrote no key"
  wait_listeners
  [ "$(wc -l < "$CI/listen.txt")" = 6 ] || fail "expected TCP 21115-21119 and UDP 21116"
  if grep -v ' 127\.0\.0\.1:' "$CI/listen.txt"; then fail "hbbs/hbbr listen beyond 127.0.0.1"; fi
  check_stack
  dc logs --no-log-prefix hbbr | grep -q 'TOTAL_BANDWIDTH: 100Mb/s' || fail "HJV_TOTAL_BANDWIDTH did not reach hbbr"
  dc logs --no-log-prefix hbbr | grep -q 'SINGLE_BANDWIDTH: 128Mb/s' || fail "an empty HJV_SINGLE_BANDWIDTH changed hbbr's default"
  "$PKGS/root/usr/sbin/nginx" -e "$ng/error.log" -p "$ng" -c "$ng/nginx.conf" -g 'daemon off;' > "$ng/out.log" 2>&1 &
  NGINX_PID=$!
  wait_for 10 curl -s -o /dev/null --cacert "$CERTS/cert.pem" https://localhost:18443/ || fail "nginx did not start"
  check_site 18443 "$CERTS/cert.pem" 200 "nginx, to Docker's hbbs and hbbr"
  if grep -E '\[(emerg|alert|crit|error)\]' "$ng/error.log"; then fail "nginx logged errors"; fi
  stop_web
  dc down
}

# Path A: Caddy in front of Docker's hbbs and hbbr (docker-compose.yml, the image built before), on
# 127.0.0.1:18082/18445; deploy_part
deploy_caddy() {
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
  wait_for 30 sudo test -s "$SRC/deploy/data/id_ed25519.pub" || fail "hbbs wrote no key"
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
  dc down
}

deploy_stage() {
  local t
  # the server stage's builds that the release package takes
  for t in hbbs hbbr; do check_stamp "$CI/target/release/$t" server "${HBBS_INPUTS[@]}"; done
  if [ -d "$ROOT/server/api" ]; then check_stamp "$API" server "${API_INPUTS[@]}"; fi
  sudo docker compose version > /dev/null 2>&1 || fail "Docker with compose v2 is needed (docs/server-runbook.md 3-1)"
  if [ -d "$ROOT/relay" ]; then
    [ -n "$(rustup target list --installed 2>/dev/null | grep -x x86_64-unknown-linux-musl)" ] && command -v musl-gcc > /dev/null ||
      fail "deploy: hjv-relay's packages need the x86_64-unknown-linux-musl target and musl-gcc (rustup target add x86_64-unknown-linux-musl; Ubuntu: apt-get install musl-tools)"
  fi
  [ -z "$(ss -Hltnu '( sport >= :21115 and sport <= :21119 )')" ] ||
    fail "ports 21115-21119 are in use (a running hjviewer server?); the test stack needs them"
  [ -z "$(ss -Hltn '( sport >= :18080 and sport <= :18082 ) or ( sport >= :18443 and sport <= :18448 ) or sport = :18599')" ] ||
    fail "test ports 18080-18082 / 18443-18448 / 18599 are in use"
  # before the jobs below: the checks and the parts' records (site_tools) take this tree's builds
  step "deploy: hjv-probe and hjv-load (tools/)"
  build_tools hjv-probe hjv-load
  trap cleanup EXIT

  # The offline tests and hjv-relay's packages share nothing with the checks below, which mostly
  # wait: they run beside them, and their output comes at the end
  step "deploy: the offline tests and hjv-relay's packages, beside the checks below (build/ci/deploy-offline.log, relay-package.log)"
  deploy_job "$CI/deploy-offline.log" deploy_offline
  if [ -d "$ROOT/relay" ]; then
    # relay/tests/package.rs's packages run only with --ignored, which no other stage gives; with
    # the output of the tests that pass: scripts/package-relay.sh says "skipped: ..." where it
    # cannot run the binary in an empty root. The mark of its target directory is this run's
    # (own_target here, not in the job, whose OWNED the end of the run would not see).
    own_target "$CI/target-relay" "$ROOT/relay"
    deploy_job "$CI/relay-package.log" env -C "$ROOT/relay" CARGO_TARGET_DIR="$CI/target-relay" \
      cargo test --locked --test package -- --ignored --show-output
  else
    skip "hjv-relay's packages (no relay/)"
  fi
  # the web servers' error logs that fail shows are this run's, not those of a run that had Apache
  rm -rf "$CI/nginx" "$CI/apache"
  DEPLOY_LOGS=1

  step "deploy B, systemd: the release package (scripts/package-server.sh) without Docker"
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
    # and names the commit that the package's hjv-api reports (the last one that changed server/api,
    # server_api)
    local api_commit
    api_commit=$("$pkg/bin/hjv-api" version | sed -n 's/^hjv-api [^ ]* (\([0-9a-f]\{40\}\))$/\1/p')
    [ -n "$api_commit" ] || fail "release package: its hjv-api reports no commit ($("$pkg/bin/hjv-api" version))"
    grep -qF "$api_commit" "$pkg/SOURCE.txt" || fail "release package: SOURCE.txt does not name $api_commit, hjv-api's commit"
    # a crate of hjv-api's that hbbs and hbbr do not use
    grep -q '^crypto_box ' "$pkg/THIRD_PARTY_NOTICES.txt" || fail "release package: hjv-api's crates are not in THIRD_PARTY_NOTICES"
    start_package_container "$rel" hjviewer-server-ci-linux-x86_64
  fi

  step "deploy: copy of this working tree"
  if [ -d "$SRC/deploy" ]; then dc down -v --remove-orphans > /dev/null 2>&1 || true; fi
  sudo rm -rf "$SRC"
  mkdir -p "$SRC"
  (cd "$ROOT" && git ls-files -z --cached --others --exclude-standard |
    tar --null --ignore-failed-read -T - -cf -) | tar -xf - -C "$SRC"

  step "deploy: Ubuntu's nginx and Apache, unpacked"
  # again when apt has another version of one of them (a security update): the checks run the
  # nginx and Apache that a server gets
  local debs=(nginx apache2-bin libapr1t64 libaprutil1t64 libjansson4 liblua5.4-0) d stale=""
  for d in "${debs[@]}"; do
    [ "$(dpkg-deb -f "$PKGS/$d"_*.deb Version 2> /dev/null)" = "$(apt-cache policy "$d" | sed -n 's/^ *Candidate: //p')" ] ||
      stale=1
  done
  if [ -n "$stale" ] || [ ! -x "$PKGS/root/usr/sbin/nginx" ] || [ ! -x "$PKGS/root/usr/sbin/apache2" ]; then
    rm -rf "$PKGS"
    mkdir -p "$PKGS/root"
    (cd "$PKGS" && apt-get download "${debs[@]}" > /dev/null)
    for d in "$PKGS"/*.deb; do dpkg-deb -x "$d" "$PKGS/root"; done
  fi
  "$PKGS/root/usr/sbin/nginx" -v
  LD_LIBRARY_PATH="$PKGS/root/usr/lib/x86_64-linux-gnu" "$PKGS/root/usr/sbin/apache2" -v | head -1
  mkdir -p "$CERTS"
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 2 -subj /CN=localhost \
    -addext subjectAltName=DNS:localhost -keyout "$CERTS/key.pem" -out "$CERTS/cert.pem" 2> /dev/null

  # The image of the Docker parts below, built beside the checks: when those parts decide whether
  # they run, it has taken in a new base image too
  step "deploy: Docker's .env and image (deploy/Dockerfile), built beside the checks below (build/ci/build.log)"
  (cd "$SRC/deploy" &&
    sudo install -d -m 700 data &&
    cp .env.example .env &&
    sed -i 's/^HJV_DOMAIN=.*/HJV_DOMAIN=localhost/' .env &&
    printf 'COMPOSE_FILE=docker-compose.behind-proxy.yml\nHJV_TOTAL_BANDWIDTH=100\nHJV_BUILD_JOBS=%s\n' "$(nproc)" >> .env)
  [ "$(dc config --services | sort | paste -sd' ')" = "hbbr hbbs" ] || fail "COMPOSE_FILE in .env did not select the behind-proxy file"
  local built
  built=$(date +%s)
  deploy_job "$CI/build.log" dc build

  # Installed as the runbook does, apart from the names that keep it away from a real installation:
  # runtime units hjv-ci-* (in /run, gone at reboot), a dynamic user and directories of its own. The
  # sandbox, limits and commands are those of the shipped units.
  step "deploy B, systemd: hbbs and hbbr from the package"
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
  wait_listeners
  [ "$(wc -l < "$CI/listen.txt")" = 6 ] || fail "systemd: expected TCP 21115-21119 and UDP 21116"
  if grep -v ' 127\.0\.0\.1:' "$CI/listen.txt"; then fail "systemd: hbbs/hbbr listen beyond 127.0.0.1"; fi
  check_systemd
  if [ ${#api_files[@]} -gt 0 ]; then
    start_systemd_api "$pkg"
    check_systemd_api
  fi

  step "deploy B: nginx with deploy/nginx-hjviewer.conf, and added to an existing site, to the systemd units"
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

  deploy_part apache "deploy B: Apache with deploy/apache-hjviewer.conf, to the systemd units" apache_inputs deploy_apache "$pkg"
  STACK=docker
  cleanup_systemd

  step "deploy: Docker's image (build/ci/build.log)"
  deploy_wait "$CI/build.log" || { tail -n 40 "$CI/build.log"; fail "image build (build/ci/build.log)"; }
  # What the image build took from Docker's cache (building the image again costs minutes; a FROM is
  # no step of its own to take from there) and the base images it resolved. It ran beside the
  # checks: it took until its log's last line, not until this wait.
  echo "image build: $(($(stat -c %Y "$CI/build.log") - built)) s, $(grep -E '^#[0-9]+ CACHED$' "$CI/build.log" | sort -u | wc -l) of" \
    "$(grep -E '^#[0-9]+ \[[^]]* [0-9]+/[0-9]+\] ' "$CI/build.log" | grep -v '\] FROM ' | cut -d' ' -f1 | sort -u | wc -l)" \
    "steps from the cache"
  sed -n -E 's/^#[0-9]+ \[[^]]*\] FROM (.*@sha256:[0-9a-f]+)$/image build: FROM \1/p' "$CI/build.log" | sort -u
  deploy_part behind-proxy "deploy B: Docker's hbbs and hbbr on 127.0.0.1 (docker-compose.behind-proxy.yml), behind nginx" \
    behind_proxy_inputs deploy_behind_proxy
  deploy_part caddy "deploy A: Caddy (docker-compose.yml) with the same image, on 127.0.0.1:18082/18445" caddy_inputs deploy_caddy

  if [ ${#api_files[@]} -gt 0 ]; then
    check_api_env "$pkg/systemd/hjviewer-api.env.example" "$pkg/systemd/hjviewer-api.env.example" "$pkg/bin/hjv-api"
    # a server's own settings, e.g. a copy of /etc/hjviewer/hjviewer-api.env: no secret in it, but
    # the copy leaves out the HJV_API_OPERATORS and HJV_API_CAMPUS_NETS lines (portal IDs and
    # campus ranges, why only root reads the server's file: docs/server-runbook.md 7-3)
    if [ -n "${HJV_API_ENV_FILE:-}" ]; then
      check_api_env "$HJV_API_ENV_FILE" "$pkg/systemd/hjviewer-api.env.example" "$pkg/bin/hjv-api"
    fi
    check_api_limits "$pkg"
  fi

  # the jobs beside the checks, in the order they started
  step "deploy: the offline tests, run beside the checks above (build/ci/deploy-offline.log)"
  show_job "$CI/deploy-offline.log" "deploy: the offline tests (build/ci/deploy-offline.log, above)"
  if [ -d "$ROOT/relay" ]; then
    step "deploy: hjv-relay's packages (scripts/package-relay.sh): the static build, its SHA256SUMS, the field test package"
    show_job "$CI/relay-package.log" "deploy: hjv-relay's packages (relay/tests/package.rs --ignored)"
    # The test runs scripts/package-relay.sh of the tree it was compiled in (CARGO_MANIFEST_DIR),
    # which packages that tree and says its commit: it must have been this tree, at this commit
    local head made
    head=$(git -C "$ROOT" rev-parse HEAD)
    made=$(sed -n -e 's/^commit: //p' -e 's#^wrote .* to \(.*\)/build/package-relay/.*#\1#p' "$CI/relay-package.log" | sort -u)
    [ "$made" = "$(printf '%s\n' "$head" "$ROOT" | sort -u)" ] ||
      fail "deploy: hjv-relay's package test packaged $(echo "$made" | paste -sd' '), not this tree at $head (build/ci/relay-package.log)"
  fi
  if [ ${#api_files[@]} -gt 0 ]; then check_package_container; fi

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

# What a checkout of fetch-upstream.sh holds, whatever its path: the trees of its commits
# (rustdesk's and hbb_common's), its status (the overlay files over them) and the mode, size and
# content of every file there that differs from its commit or is not in it, those that git ignores
# too (an overlay file can be one, as an icon under upstream's *png rule), except the src/version.rs
# that the client's build writes. A build keeps its checkout while a fresh fetch holds the same, so
# that cargo, which judges path crates by their files' times, compiles nothing again; another tree
# replaces it, with new times.
checkout_state() { # <rustdesk checkout>
  local dir
  for dir in "$1" "$1/libs/hbb_common"; do
    git -C "$dir" rev-parse 'HEAD^{tree}'
    git -C "$dir" --no-optional-locks status --porcelain --untracked-files=all
    (cd "$dir" && git ls-files -z -m -o -- . ':!libs/hbb_common' ':!src/version.rs' | sort -z |
      xargs -0 -r sh -c 'for f; do [ ! -e "$f" ] || echo "$f $(stat -c "%a %s" "$f") $(sha256sum < "$f")"; done' _)
  done | sha256sum
}

# Whether a cargo test log's test executable (its "Running unittests src/lib.rs" line) was compiled
# with the build variable <NAME> set or unset, by rustc's dep-info next to it ("# env-dep:" lines);
# nothing when it does not read that variable
test_env() { # <cargo test log> <NAME>
  local exe
  exe=$(sed -n 's/^ *Running unittests src\/lib\.rs (\(.*\))$/\1/p' "$1" | head -n 1)
  [ -n "$exe" ] && [ -f "$exe.d" ] || return 0
  if grep -q "^# env-dep:$2=" "$exe.d"; then
    echo set
  elif grep -qx "# env-dep:$2" "$exe.d"; then
    echo unset
  fi
}

# The client of the E2E (docs/e2e.md): an upstream checkout with client/patches, built for
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
# The staff-only gate's A1-A11, A13-A14, C1-C7 and W6-E1-E7 (docs/e2e.md, the access stage): with hbbs's access file (server
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
    skip "the staff-only gate's A1-A11, A13-A14, C1-C7 and W6-E1-E7 (hbbs has no --hjv-access-file: no server patch 0006)"
    return 0
  fi
  # (the name of its variable: a release build compares the commands' names without keeping them)
  if ! grep -aqF HJV_E2E_ACCESS_PASSWORD "$E2E_CLIENT"; then
    grep -q HJV_E2E_ACCESS_PASSWORD "$ROOT"/client/patches/rustdesk/*.patch &&
      fail "e2e: the client has no HJV_E2E_ACCESS_PASSWORD, though client/patches has it (patch 0061)"
    skip "the staff-only gate's A1-A11, A13-A14, C1-C7 and W6-E1-E7 (the client has no HJV_E2E_ACCESS_PASSWORD: no client patch 0061)"
    return 0
  fi
  if [ -z "$E2E_ACCOUNT" ] || ! grep -q HJV_E2E_ACCESS "$ROOT/tools/e2e/firewall-e2e.sh"; then
    skip "the staff-only gate's A1-A11, A13-A14, C1-C7 and W6-E1-E7 (no account E2E, or no HJV_E2E_ACCESS part in tools/e2e/firewall-e2e.sh)"
    return 0
  fi
  E2E_ACCESS=(HJV_E2E_ACCESS=1)
}
# What the E2E's client is built for, by the checkout <src> and tools/e2e/firewall-e2e.sh: the account
# E2E with the E2E's public keys (E2E_ACCOUNT), once the patches have the account code, and the update
# test keys (E2E_KEYED) once they have the update code; the build variables of both in E2E_BUILD_KEYS
e2e_client_kind() { # <work dir> <checkout>
  local work=$1 src=$2 update_keys
  E2E_ACCOUNT="" E2E_KEYED="" E2E_BUILD_KEYS=()
  if [ -d "$src/src/hjviewer_account" ] && grep -q HJV_E2E_ACCOUNT "$ROOT/tools/e2e/firewall-e2e.sh"; then
    E2E_ACCOUNT=1
    E2E_BUILD_KEYS=(HJV_LOGIN_SEAL_PUBKEY="$(cat "$work/api/seal.pub")" HJV_TRUST_PUBKEYS="$(cat "$work/api/trust.pub")")
  else
    skip "the account E2E (no src/hjviewer_account in the patched tree, or no HJV_E2E_ACCOUNT part in tools/e2e/firewall-e2e.sh): the client is built without the account keys, like 1.5.1"
  fi
  # the update test keys, once the patches have the update code: the e2e stage says what it skips
  if [ -d "$src/src/hjviewer_update" ] && grep -q HJV_E2E_UPDATE_KEYS "$ROOT/tools/e2e/firewall-e2e.sh"; then
    E2E_KEYED=1
    update_keys=$(e2e_update_keys) || fail "e2e: the update test keys (scripts/check-build-keys.py pubkey)"
    readarray -t -O "${#E2E_BUILD_KEYS[@]}" E2E_BUILD_KEYS <<< "$update_keys"
  fi
}
build_e2e_client() { # <work dir> [<module of src/> [<test filter>]]
  local work=$1 unit=${2:-hjviewer_account} src="$CI/upstream-e2e/rustdesk" keys=()
  local filter=${3:-$unit}
  if [ -n "$E2E_CLIENT_GIVEN" ]; then
    E2E_CLIENT=$E2E_CLIENT_GIVEN
    skip "the client build and its unit tests (E2E_CLIENT=$E2E_CLIENT is used as it is)"
    return 0
  fi
  # what a build of the e2e stage leaves for the e2e and wol stages of its prepush run: gone until it
  # passes
  rm -f "$CI/target-e2e/e2e-client.stamp"
  step "e2e: upstream checkout with client/patches"
  rm -rf "$CI/upstream-e2e.new"
  UPSTREAM_DIR="$CI/upstream-e2e.new" "$ROOT/scripts/fetch-upstream.sh" client
  if [ -d "$src" ] && [ "$(checkout_state "$src")" = "$(checkout_state "$CI/upstream-e2e.new/rustdesk")" ]; then
    rm -rf "$CI/upstream-e2e.new"
    echo "the same as the checkout of the last build, which stays with its files' times: cargo has nothing to compile again"
  else
    rm -rf "$CI/upstream-e2e"
    mv "$CI/upstream-e2e.new" "$CI/upstream-e2e"
  fi
  echo "the patched tree: $(git -C "$src" rev-parse 'HEAD^{tree}') (and the overlay files)"
  e2e_client_kind "$work" "$src"
  keys=("${E2E_BUILD_KEYS[@]}")
  step "e2e: the client for the test server hjv.test (--features linux-pkg-config,hjv-e2e)"
  pkgconfig
  # (No mark on target-e2e and target-e2e-nokeys, own_target: only this checkout builds into them,
  # and it stays only while client/patches gives the same tree, so what another tree built there is
  # this one's; a mark would have the client compiled again after every switch of trees.)
  (cd "$src" && env PKG_CONFIG_PATH="$CI/pkgconfig" HJV_SERVER=hjv.test HJV_SERVER_KEY="$(cat "$work/srv/id_ed25519.pub")" \
    "${keys[@]}" "${E2E_PROFILE[@]}" CARGO_TARGET_DIR="$CI/target-e2e" \
    cargo build --release --locked --features linux-pkg-config,hjv-e2e) || fail "e2e: the client build"
  E2E_CLIENT="$CI/target-e2e/release/rustdesk"
  if [ -e "$src/src/$unit" ] || [ -e "$src/src/$unit.rs" ]; then
    # (the debug side of target-e2e, which only this checkout builds into: cargo hashes another
    # checkout's path crates alike, and would take their builds as fresh for the kept one)
    step "e2e: the client's unit tests (--lib $filter, output also in build/ci/e2e-unit-$filter.log)"
    with_skips "$CI/e2e-unit-$filter.log" env -C "$src" PKG_CONFIG_PATH="$CI/pkgconfig" HJV_SERVER=hjv.test \
      HJV_SERVER_KEY="$(cat "$work/srv/id_ed25519.pub")" "${keys[@]}" CARGO_TARGET_DIR="$CI/target-e2e" \
      cargo test --locked --features linux-pkg-config,hjv-e2e --lib "$filter" ||
      fail "e2e: the client's unit tests (--lib $filter)"
    at_least "e2e: the client's unit tests that passed (--lib $filter)" "$(passed_tests "$CI/e2e-unit-$filter.log")" \
      "${UNIT_LEAST[$filter]:-1}"
    if [ -n "$E2E_KEYED" ] && [ "$(test_env "$CI/e2e-unit-$filter.log" HJV_UPDATE_PUBKEY)" != set ]; then
      fail "e2e: the client's unit tests ran an executable built without the update keys (rustc's dep-info)"
    fi
    if [ -n "$E2E_KEYED" ] && [[ hjviewer_update == *"$filter"* ]]; then e2e_unit_without_update_keys "$work" "${keys[@]}"; fi
  else
    skip "the client's unit tests of src/$unit (not in the patched tree)"
  fi
  # The client's build script writes src/version.rs (its version and the minute of the build) in
  # each target directory's first build of a checkout, which leaves the builds before the last one
  # older than it: in the next run of the kept checkout cargo would compile the client crate again
  # for that minute. Back at the checkout's time, the file leaves every build as it is.
  if [ -e "$src/src/version.rs" ]; then touch -r "$src/Cargo.toml" "$src/src/version.rs"; fi
}

# The update code's unit tests once more, in a build without the update keys (minimum version
# design 1-7): such a build takes no part and does what 1.5.1 did, and the tests that pin this
# return at once in the keyed build. The build variables of build_e2e_client without the update
# keys, which are taken out of this shell's environment too, in the test profile and in a target
# directory of their own: in build_e2e_client's, cargo would compile the client crate's tests
# twice a run, for the keys going and coming back. That the keys are gone shows in rustc's dep-info
# of the test executable that ran (test_env), not in what cargo compiled.
e2e_unit_without_update_keys() { # <work dir> <build variables...>
  local work=$1 v vars=() log="$CI/e2e-unit-hjviewer_update-nokeys.log"
  shift
  for v in "$@"; do
    case $v in HJV_UPDATE_PUBKEY=* | HJV_UPDATE_ROOT_PUBKEY=* | HJV_POLICY_FLOOR=*) ;; *) vars+=("$v") ;; esac
  done
  step "e2e: the update code's unit tests without the update keys (--lib hjviewer_update, output also in build/ci/e2e-unit-hjviewer_update-nokeys.log)"
  with_skips "$log" env -C "$CI/upstream-e2e/rustdesk" -u HJV_UPDATE_PUBKEY -u HJV_UPDATE_ROOT_PUBKEY -u HJV_POLICY_FLOOR \
    PKG_CONFIG_PATH="$CI/pkgconfig" HJV_SERVER=hjv.test HJV_SERVER_KEY="$(cat "$work/srv/id_ed25519.pub")" "${vars[@]}" \
    CARGO_TARGET_DIR="$CI/target-e2e-nokeys" \
    cargo test --locked --features linux-pkg-config,hjv-e2e --lib hjviewer_update ||
    fail "e2e: the update code's unit tests without the update keys (--lib hjviewer_update)"
  for v in HJV_UPDATE_PUBKEY HJV_UPDATE_ROOT_PUBKEY HJV_POLICY_FLOOR; do
    [ "$(test_env "$log" "$v")" = unset ] ||
      fail "e2e: the update code's tests without the update keys ran an executable built with $v (rustc's dep-info)"
  done
  at_least "e2e: the update code's unit tests that passed without the update keys" "$(passed_tests "$log")" \
    "${UNIT_LEAST[hjviewer_update-nokeys]}"
}

# What the E2E's client is built from and with: client/ (the patches, the overlay), the upstream
# pins, the test server's key, the E2E's public keys, the profile and the toolchain. With the
# prepush run and the binary itself, e2e:build leaves it for the e2e and wol stages of that run,
# which then take its build and the unit tests it ran (hjviewer_wol's among hjv's).
e2e_client_stamp() { # <work dir> <binary>
  local inputs
  inputs=$({
    (cd "$ROOT" && find client scripts/upstream.env scripts/fetch-upstream.sh -type f -print0 | sort -z | xargs -0 sha256sum)
    cat "$1/srv/id_ed25519.pub" "$1/api/seal.pub" "$1/api/trust.pub"
    echo "${E2E_PROFILE[*]} $(cargo --version)"
  } | sha256sum | cut -d' ' -f1)
  echo "run ${HJV_CI_RUN:-none} inputs $inputs binary $(sha256sum < "$2" | cut -d' ' -f1)"
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
  own_target "$CI/target-api-e2e" "$ROOT/server/api"
  (cd "$ROOT/server/api" && CARGO_TARGET_DIR="$CI/target-api-e2e" cargo build --release --locked --features fake-sso) ||
    fail "e2e: hjv-api build"
  E2E_API="$CI/target-api-e2e/release/hjv-api" E2E_FAKE_SSO="$CI/target-api-e2e/release/hjv-fake-sso"
}

cleanup_e2e() {
  sudo rm -rf "$E2E_PC_DIR"
}

# The minimum version's parts of the E2E (its design 2-4, 1-7; docs/e2e.md), as values for
# firewall-e2e.sh in E2E_MINVER: hbbs's lowest controller version (V1, V2 and V4) once hbbs has server patch
# 0005; once server/patches has that patch, an hbbs without it (built by an earlier server stage)
# fails the stage instead of skipping the part. And the signed version policy (V5-V7 and V9) once the
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
    skip "the minimum version's V1, V2 and V4 (hbbs has no --hjv-min-controller-file: no server patch 0005)"
  fi
  if [ -z "$E2E_KEYED" ]; then
    skip "the minimum version's V5-V7 and V9 (no src/hjviewer_update in the patched tree, or no HJV_E2E_UPDATE_KEYS part in tools/e2e/firewall-e2e.sh): the client is built without the update keys, like 1.5.1"
    return 0
  fi
  E2E_MINVER+=(HJV_E2E_UPDATE_KEYS=1)
  if [ -f "$ROOT/tools/hjv-release/Cargo.toml" ]; then
    step "e2e: tools/hjv-release, which signs the E2E's version policies with the test keys"
    own_target "$CI/target-hjv-release" "$ROOT/tools/hjv-release"
    (cd "$ROOT/tools/hjv-release" && CARGO_TARGET_DIR="$CI/target-hjv-release" cargo build --release --locked) ||
      fail "e2e: hjv-release build"
    stamp_binary "$CI/target-hjv-release/release/hjv-release" "${RELEASE_INPUTS[@]}"
    E2E_MINVER+=(HJV_RELEASE="$CI/target-hjv-release/release/hjv-release")
  fi
}

# The e2e stage's builds (e2e:build), which take no port, no network namespace and no sudo: the E2E's
# client with its unit tests (build_e2e_client), hjv-api and the fake SSO, Caddy and libfaketime.
# prepush runs it beside the deploy stage; the e2e stage of that run then takes its client and the
# unit tests it ran (e2e_client_stamp), and builds again only what this left undone.
e2e_build_stage() {
  local work="$CI/e2e" t
  [ -z "$E2E_CLIENT_GIVEN" ] ||
    fail "e2e: E2E_CLIENT is not taken here: the stage builds the client from client/patches, runs its unit tests and the account E2E with it"
  # (hbbs makes the test server's key)
  for t in hbbs hbbr; do check_stamp "$CI/target/release/$t" server "${HBBS_INPUTS[@]}"; done
  fetch_caddy
  e2e_server_key "$work"
  e2e_keys "$work/api"
  build_e2e_client "$work" hjviewer_account hjv
  if [ -n "${HJV_CI_RUN:-}" ] && [ -n "$E2E_ACCOUNT" ]; then
    e2e_client_stamp "$work" "$E2E_CLIENT" > "$CI/target-e2e/e2e-client.stamp"
  fi
  if [ -n "$E2E_ACCOUNT" ]; then
    build_e2e_api
    fetch_faketime
  fi
}

e2e_stage() {
  local work="$CI/e2e" mode extra=() modes=(strict open) t built="$CI/target-e2e/release/rustdesk"
  # a client given by hand would go without the build's unit tests and, built without the E2E's
  # keys, without the account E2E
  [ -z "$E2E_CLIENT_GIVEN" ] ||
    fail "e2e: E2E_CLIENT is not taken here: the stage builds the client from client/patches, runs its unit tests and the account E2E with it"
  # a partial run of strict mode (HJV_E2E_PARTS, docs/e2e.md) is for a fix loop and never a pass
  if [ -n "${HJV_E2E_PARTS:-}" ]; then
    [ "${HJV_CI_STRICT:-}" != 1 ] ||
      fail "e2e: HJV_E2E_PARTS=$HJV_E2E_PARTS runs a part of the E2E, which is never a pass: not with HJV_CI_STRICT=1"
    modes=(strict)
    skip "firewall-e2e.sh open (HJV_E2E_PARTS: a partial run of strict mode)"
  fi
  sudo -n true 2> /dev/null || fail "e2e: needs sudo without a password prompt (network namespaces, iptables)"
  for t in hbbs hbbr; do check_stamp "$CI/target/release/$t" server "${HBBS_INPUTS[@]}"; done
  step "e2e: hjv-probe (tools/)"
  build_tools hjv-probe
  fetch_caddy
  e2e_server_key "$work"
  e2e_keys "$work/api"
  if [ -n "${HJV_CI_RUN:-}" ] && [ -x "$built" ] &&
    [ "$(cat "$CI/target-e2e/e2e-client.stamp" 2> /dev/null)" = "$(e2e_client_stamp "$work" "$built")" ]; then
    E2E_CLIENT=$built
    e2e_client_kind "$work" "$CI/upstream-e2e/rustdesk"
    echo "e2e: the client of this prepush run's e2e:build, built from the same client/patches and keys, with its unit tests there"
  else
    build_e2e_client "$work" hjviewer_account hjv
    if [ -n "${HJV_CI_RUN:-}" ] && [ -n "$E2E_ACCOUNT" ]; then
      e2e_client_stamp "$work" "$E2E_CLIENT" > "$CI/target-e2e/e2e-client.stamp"
    fi
  fi
  # the PC's client where Linux counts it as installed, as its account binding needs; the
  # controllers keep running the build output
  step "e2e: the PC's client as $E2E_PC_DIR/rustdesk"
  trap cleanup_e2e EXIT
  sudo install -D -m 755 "$E2E_CLIENT" "$E2E_PC_DIR/rustdesk"
  if [ -n "$E2E_ACCOUNT" ]; then
    build_e2e_api
    fetch_faketime
    extra=(HJV_E2E_ACCOUNT=1 HJV_API="$E2E_API" HJV_FAKE_SSO="$E2E_FAKE_SSO" HJV_E2E_KEYS="$work/api"
      FAKETIME_LIB="$E2E_FAKETIME")
  fi
  e2e_minimum_version
  extra+=("${E2E_MINVER[@]}")
  e2e_access
  extra+=("${E2E_ACCESS[@]}")
  # In strict mode a skip fails the stage at its end: what was skipped so far fails it now, and
  # firewall-e2e.sh fails at its start on what it knows it would skip (HJV_E2E_NO_SKIP), not after
  # twelve minutes
  if [ "${HJV_CI_STRICT:-}" = 1 ]; then
    if [ ${#SKIPPED[@]} -gt 0 ]; then
      printf '  - %s\n' "${SKIPPED[@]}"
      fail "e2e: HJV_CI_STRICT=1, and the stage has skipped the ${#SKIPPED[@]} part(s) above before the E2E"
    fi
    extra+=(HJV_E2E_NO_SKIP=1)
  fi
  for mode in "${modes[@]}"; do
    step "e2e: tools/e2e/firewall-e2e.sh $mode (output also in build/ci/e2e-$mode.log)"
    with_skips "$CI/e2e-$mode.log" env HJV_E2E_WORK="$work" CADDY="$CADDY_BIN" HBBS="$CI/target/release/hbbs" \
      HBBR="$CI/target/release/hbbr" PROBE="$PROBE" CLIENT="$E2E_CLIENT" PC_CLIENT="$E2E_PC_DIR/rustdesk" "${extra[@]}" \
      "$ROOT/tools/e2e/firewall-e2e.sh" "$mode" || fail "e2e: firewall-e2e.sh $mode"
  done
  at_least "e2e: PASS lines (strict)" "$(grep -c '^PASS' "$CI/e2e-strict.log" || true)" "$E2E_LEAST_STRICT"
  if [ ${#modes[@]} = 2 ]; then
    at_least "e2e: PASS lines (open)" "$(grep -c '^PASS' "$CI/e2e-open.log" || true)" "$E2E_LEAST_OPEN"
  fi
  cleanup_e2e
  trap - EXIT
}

# Without privileges: tools/e2e/wol-e2e.sh runs its server, PC, NAS and networks in namespaces of
# this user (design v3 8-10, docs/e2e.md). It gets the same programs as the E2E, and hjv-relay
# built for hjv.test with the E2E's command key. The client's unit tests are those of its wake
# report here; the E2E runs once the client has the report's command (client patch 0035), and
# fails without it once client/patches has that patch. In a prepush run it takes the client of
# e2e:build when that was built from the same inputs (e2e_client_stamp).
wol_stage() {
  local work="$CI/e2e" built="$CI/target-e2e/release/rustdesk" t
  for t in hbbs hbbr; do check_stamp "$CI/target/release/$t" server "${HBBS_INPUTS[@]}"; done
  step "wol: hjv-probe (tools/)"
  build_tools hjv-probe
  fetch_caddy
  e2e_server_key "$work"
  e2e_keys "$work/api"
  if [ -z "$E2E_CLIENT_GIVEN" ] && [ -n "${HJV_CI_RUN:-}" ] && [ -x "$built" ] &&
    [ "$(cat "$CI/target-e2e/e2e-client.stamp" 2> /dev/null)" = "$(e2e_client_stamp "$work" "$built")" ]; then
    E2E_CLIENT=$built
    echo "wol: the client of this prepush run's e2e:build, built from the same client/patches and keys" \
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
  own_target "$CI/target-relay" "$ROOT/relay"
  (cd "$ROOT/relay" && HJV_SERVER=hjv.test HJV_COMMAND_PUBKEY="$(cat "$work/api/command.pub")" \
    CARGO_TARGET_DIR="$CI/target-relay" cargo build --release --locked) || fail "wol: hjv-relay build"
  step "wol: tools/e2e/wol-e2e.sh (output also in build/ci/wol-e2e.log)"
  with_skips "$CI/wol-e2e.log" env HJV_E2E_WORK="$work" CADDY="$CADDY_BIN" HBBS="$CI/target/release/hbbs" \
    HBBR="$CI/target/release/hbbr" PROBE="$PROBE" CLIENT="$E2E_CLIENT" HJV_API="$E2E_API" HJV_FAKE_SSO="$E2E_FAKE_SSO" \
    HJV_RELAY="$CI/target-relay/release/hjv-relay" HJV_E2E_KEYS="$work/api" \
    "$ROOT/tools/e2e/wol-e2e.sh" || fail "wol: wol-e2e.sh"
  at_least "wol: PASS lines" "$(grep -c '^PASS' "$CI/wol-e2e.log" || true)" "$WOL_LEAST"
}

# The operations drill (tools/e2e/hjv-api-drill.sh, docs/hjv-api-drill.md) of this tree. drill:build
# builds and stages what it installs, into build/ci/target-drill, which every tree's prepush builds
# into (own_target marks it); drill:full and drill:quick run it on that stage, built first when it is
# not of this tree's inputs (the script's staged). prepush --drill runs the build beside the deploy
# stage and the drill beside the e2e stage. The drill's own logs are in build/drill/.
drill_stage() { # <drill:build|drill:full|drill:quick>
  local part=${1#drill:} quick=()
  case $part in
    build | full) ;;
    quick) quick=(HJV_DRILL_QUICK=1) ;;
    *) fail "no stage $1: drill:build, drill:full or drill:quick" ;;
  esac
  if [ "$part" = build ] || ! "$ROOT/tools/e2e/hjv-api-drill.sh" staged; then
    step "drill: hjv-api, the fake SSO and the drill's client, staged (tools/e2e/hjv-api-drill.sh build, into build/ci/target-drill)"
    own_target "$CI/target-drill" "$ROOT/server/api"
    with_skips "$CI/drill-build.log" env CARGO_TARGET_DIR="$CI/target-drill" "$ROOT/tools/e2e/hjv-api-drill.sh" build ||
      fail "drill: the build (above)"
  fi
  [ "$part" != build ] || return 0
  sudo -n true 2> /dev/null || fail "drill: needs sudo without a password prompt (Docker)"
  step "drill: tools/e2e/hjv-api-drill.sh run, the $part drill (output also in build/drill/drill.log)"
  with_skips "$CI/drill-run.log" env -u HJV_DRILL_QUICK "${quick[@]}" "$ROOT/tools/e2e/hjv-api-drill.sh" run ||
    fail "drill: the $part drill (above)"
}

# The push gate of design v3 11 steps 1-2: none of the values of build/leak-patterns.txt (a local
# list git ignores; docs/ci.md says what goes in) in tracked files, staged changes and the commits
# a push would publish. Prints places and list line numbers, never the values. Then the checks of
# prepush's reuse, which every prepush runs first, since they hold no unit's pass: its rules against
# the code the units run and the re-run table of docs/ci.md (reuse_check), and the tests of the reuse
# and of the lock.
leak_stage() {
  step "leak: tests of scripts/leak-check.py (made-up values, throwaway repositories)"
  python3 -I -B -m unittest discover -s "$ROOT/scripts" -p 'test_leak_check.py' || fail "leak: tests of the leak check"
  step "leak: the listed values in what a push would publish"
  python3 -I -B "$ROOT/scripts/leak-check.py" --repo "$ROOT" || fail "leak: see above; push nothing until this passes"
  step "leak: what the units of prepush's reuse read, against their rules (REUSE_SKIP), and the re-run table of docs/ci.md"
  reuse_check
  step "leak: tests of scripts/ci.sh's reuse and lock (scripts/test_ci_reuse.py, throwaway repositories)"
  python3 -I -B -m unittest discover -s "$ROOT/scripts" -p 'test_ci_reuse.py' || fail "leak: tests of scripts/ci.sh's reuse and lock"
}

# The head of a stage's log: the commits it ran on (this repository; the client series and hbbs
# checkouts under upstream/, which client/patches and server/patches are exported from), the
# toolchain and the start. A run of its own starts with it too, saying so in its first line.
log_head() { # <stage> [<how it runs>]
  local name dir
  echo "LOG scripts/ci.sh $1 (${2:-prepush, HJV_CI_STRICT=1}, CI directory $CI)"
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

# --- the lock ---

# The runs on a CI directory share its checkouts, builds and E2E folders: a run that uses them holds
# the lock <CI directory>/ci.lock. flock holds it for as long as the run lasts and keeps it from the
# programs the run starts (-o), so that nothing the run leaves behind holds it. The runs that prepush
# starts are under prepush's lock (HJV_CI_LOCK), as are those of a caller that holds it itself
# (flock -o <lock> env HJV_CI_LOCK=<CI directory> scripts/ci.sh ...). A run that finds it held says
# by whom (lock_holder) and stops at once, or with HJV_CI_WAIT=<seconds> waits that long for it.
lock_run() { # <argument...>: this script again with them, under the lock
  local wait=${HJV_CI_WAIT:-} how=(-n) status=0
  [[ $wait =~ ^[0-9]*$ ]] || fail "HJV_CI_WAIT=$wait is no number of seconds"
  if ! flock -n "$CI/ci.lock" true; then
    [ -n "$wait" ] || fail "another scripts/ci.sh run holds $CI/ci.lock: $(lock_holder) (HJV_CI_WAIT=<seconds> waits for it)"
    echo "waiting up to $wait s for $CI/ci.lock, held by $(lock_holder)"
  fi
  [ -z "$wait" ] || how=(-w "$wait")
  # (75, which no run of this script exits with, is flock's own: the lock stayed held)
  env HJV_CI_LOCK="$CI" flock "${how[@]}" -E 75 -o "$CI/ci.lock" "$SELF" "$@" || status=$?
  [ "$status" != 75 ] || fail "$CI/ci.lock is still held${wait:+ after $wait s}, by $(lock_holder)"
  exit "$status"
}
# Who holds the lock, by what the run that holds it wrote at its start (lock_held)
lock_holder() {
  local pid rest
  if read -r pid rest 2> /dev/null < "$CI/ci.lock.holder" && kill -0 "$pid" 2> /dev/null; then
    echo "pid $pid, $rest"
  else
    echo "a run that does not say who it is (no live process in $CI/ci.lock.holder)"
  fi
}
# At the start of a run under the lock, in its first process only (the stages that prepush starts
# are its): who holds the lock, for lock_holder
lock_held() { # <argument...>
  [ -z "${HJV_CI_HOLDER:-}" ] || return 0
  export HJV_CI_HOLDER=$$
  echo "$$ tree $ROOT, commit $(git -C "$ROOT" rev-parse --short HEAD), scripts/ci.sh $*, since $(date '+%Y-%m-%d %H:%M:%S %z')" \
    > "$CI/ci.lock.holder.$$"
  mv -f "$CI/ci.lock.holder.$$" "$CI/ci.lock.holder"
}

# --- the preflight ---

# What <stage...> need of this machine, looked at before any of them starts: what a stage would
# otherwise skip (which fails a strict run, but only at its end) or fail on minutes in. In a strict
# run what is missing stops it at once; else it is told. The ports, which a run holding the lock may
# be using, are looked at under the lock (preflight_ports).
preflight() { # <stage...>
  local s missing=() node
  for s in "$@"; do
    case $s in
      server | server:* | all)
        own_net true 2> /dev/null ||
          missing+=("$s: unprivileged user and network namespaces (unshare -rn), for hjv-relay's netns test and the test binaries' own networks") ;;&
      client)
        rustup toolchain list 2> /dev/null | grep -q '^1\.75\.0' ||
          missing+=("client: Rust 1.75.0, the app builds' (rustup toolchain install 1.75.0 --profile minimal)")
        node=$(node -p 'process.versions.node.split(".")[0]' 2> /dev/null || echo 0)
        [ "$node" -ge 20 ] 2> /dev/null || missing+=("client: Node 20 or later, for the App Store Connect script's tests")
        command -v actionlint > /dev/null || [ -x "$HOME/.local/bin/actionlint" ] ||
          missing+=("client: actionlint (github.com/rhysd/actionlint), for the workflows")
        ;;
      deploy | all)
        sudo -n true 2> /dev/null || missing+=("deploy: sudo without a password prompt")
        sudo -n docker compose version > /dev/null 2>&1 ||
          missing+=("deploy: Docker with compose v2, through sudo (docs/server-runbook.md 3-1)")
        if [ -d "$ROOT/relay" ] && ! { rustup target list --installed 2> /dev/null | grep -qx x86_64-unknown-linux-musl &&
          command -v musl-gcc > /dev/null; }; then
          missing+=("deploy: the x86_64-unknown-linux-musl target and musl-gcc, for hjv-relay's packages")
        fi
        unshare -rn true 2> /dev/null ||
          missing+=("deploy: unprivileged user and network namespaces, for the account API's nginx limits")
        ;;
      e2e)
        sudo -n true 2> /dev/null || missing+=("e2e: sudo without a password prompt (network namespaces, iptables)")
        [ -f /usr/lib/x86_64-linux-gnu/faketime/libfaketime.so.1 ] ||
          [ -f "$CI/faketime/root/usr/lib/x86_64-linux-gnu/faketime/libfaketime.so.1" ] ||
          apt-cache show libfaketime > /dev/null 2>&1 ||
          missing+=("e2e: libfaketime (installed, unpacked under build/ci/faketime, or in apt's lists), for S27")
        ;;
      wol) unshare -rn true 2> /dev/null || missing+=("wol: unprivileged user and network namespaces") ;;
      drill:full | drill:quick)
        sudo -n docker version > /dev/null 2>&1 || missing+=("$s: Docker through sudo without a password prompt") ;;
    esac
  done
  if [ ${#missing[@]} -eq 0 ]; then
    echo "preflight: what $* need is here"
    return 0
  fi
  printf 'preflight: missing: %s\n' "${missing[@]}"
  [ "${HJV_CI_STRICT:-}" != 1 ] || fail "preflight: $* would skip or fail on the ${#missing[@]} missing above (HJV_CI_STRICT=1)"
  echo "preflight: not strict (HJV_CI_STRICT=${HJV_CI_STRICT:-}): the stages skip what needs it, or fail"
}
# The host ports of the deploy and e2e stages, free before prepush starts its first stage
preflight_ports() { # <stage...>
  local busy
  if [[ " $* " == *" deploy "* ]]; then
    busy=$(ss -Hltnu '( sport >= :21115 and sport <= :21119 ) or ( sport >= :18080 and sport <= :18082 ) or ( sport >= :18443 and sport <= :18448 ) or sport = :18599' |
      awk '{print $5}' | sort -u | paste -sd' ')
    [ -z "$busy" ] || fail "preflight: the deploy stage's ports are in use: $busy (a running hjviewer server?)"
  fi
  if [[ " $* " == *" e2e "* ]]; then
    busy=$(ss -Hltnu '( sport >= :41115 and sport <= :41119 ) or sport = :18443' | awk '{print $5}' | sort -u | paste -sd' ')
    [ -z "$busy" ] || fail "preflight: the e2e stage's ports are in use: $busy"
  fi
  return 0
}

# --- reusing strict passes (docs/ci.md, "통과 재사용") ---

# The rules of the reuse (docs/ci.md, "통과 재사용"), set by what uses them: in a function, so that
# their lines are no top-level code that every unit runs (reuse_top)
reuse_rules() {
  [ -z "${REUSE_UNITS:-}" ] || return 0
  # The units whose strict passes prepush may reuse: the server stage's parts, the client, deploy,
  # e2e and wol stages, and the operations drill (prepush --drill). leak reads everything and always
  # runs.
  REUSE_UNITS="server:hbbs server:api server:relay server:release client deploy e2e wol drill:full drill:quick"
  # What of this script each runs: these functions, finish, and every function that they name
  # (reuse_closure)
  declare -gA REUSE_CODE=([server:hbbs]="server_stage server_hbbs" [server:api]="server_stage server_api"
    [server:relay]="server_stage server_relay" [server:release]="server_stage server_release" [client]=client_stage
    [deploy]=deploy_stage [e2e]="e2e_build_stage e2e_stage" [wol]=wol_stage [drill:full]=drill_stage
    [drill:quick]=drill_stage)
  # What each unit provably does not read of HEAD's tree, by path pattern (scripts/ci-reuse.py: fnmatch,
  # whose * also takes /; the last pattern that matches decides, and one after ! takes paths back). Its
  # input files are all the rest, so that a file nobody thought of counts until a rule leaves it out;
  # every prepush's leak stage holds the rules to the code that the units run (reuse_check).
  declare -gA REUSE_SKIP=(
    # the upstream checkout with server/patches, the vectors of server/api that it compares and reads,
    # hjv-probe and hjv-load
    [server:hbbs]="client/* store/* .github/* docs/* deploy/* relay/* tools/e2e/* tools/hjv-release/* LICENSE NOTICE
      server/api/* !server/api/src/hbbs.rs !server/api/tests/vectors/access-tickets.json
      scripts/* !scripts/fetch-upstream.sh !scripts/upstream.env"
    # hjv-api's tests: hjv-relay, a dev-dependency, and its tests/common (relay_loop.rs, which builds
    # it), deploy/systemd and the journal's retention (ops.rs), the vectors of tools/hjv-release
    # (update.rs)
    [server:api]="client/* store/* .github/* docs/* server/patches/* tools/e2e/* tools/hjv-probe/* tools/hjv-load/*
      LICENSE NOTICE scripts/* deploy/* !deploy/systemd/* !deploy/journald@hjviewer.conf
      tools/hjv-release/* !tools/hjv-release/tests/vectors/* relay/tests/* !relay/tests/common/* relay/PROTOCOL.md"
    # hjv-relay's tests: docs/relay-dsm.md (watchdog.rs, package.rs), and package-relay.sh's refusals
    # with check-build-keys.py, whose test keys come from every tracked file (REUSE_VALUES)
    [server:relay]="client/* store/* .github/* docs/* !docs/relay-dsm.md server/* deploy/* tools/* LICENSE NOTICE
      scripts/* !scripts/package-relay.sh !scripts/check-build-keys.py !scripts/public-test-keys.txt"
    [server:release]="client/* store/* .github/* docs/* server/* relay/* deploy/* scripts/* LICENSE NOTICE
      tools/* !tools/hjv-release/*"
    # the store run, the release records (docs/ci.md's build.yml among them), the key files and the
    # workflows, the upstream checkout with client/patches, hjv-release and its vectors, the seal key of
    # server/api/src/seal.rs (test_check_build_keys.py) and the 32-byte values of every tracked file
    [client]="server/* !server/api/src/seal.rs relay/* deploy/* tools/e2e/* tools/hjv-probe/* tools/hjv-load/* LICENSE NOTICE
      docs/* !docs/ci.md scripts/package-server.sh scripts/package-relay.sh scripts/package-notices.sh
      scripts/check-api-settings.py scripts/test_check_api_settings.py scripts/test_review_conf.py
      scripts/test_guest_conf.py scripts/test_hjv_update_verify.py"
    # the server package, its units and web server files, Docker's image (server/patches, the pins,
    # LICENSE, NOTICE), hjv-relay's packages (docs/relay-dsm.md is their README-DSM.md), the server's
    # check of a version policy against the vectors of tools/hjv-release, the settings, offline tests
    [deploy]="client/* store/* .github/* tools/e2e/* docs/* !docs/relay-dsm.md tools/hjv-release/* !tools/hjv-release/tests/vectors/*
      scripts/release-info.py scripts/test_release_info.py scripts/testdata/* scripts/play-upload.py
      scripts/test_play_upload.py scripts/check-store-text.py scripts/test_check_store_text.py scripts/store-record.py
      scripts/test_store_record.py scripts/test_store_workflows.py scripts/run-summary.py scripts/asc-store.mjs
      scripts/asc-store.test.mjs scripts/test_check_build_keys.py scripts/hjv-fontsize-check.sh
      scripts/hjv-fontsize-baseline.txt scripts/set-client-version.sh"
    # the E2E's client (client/, the pins, the keys of check-build-keys.py), hbbs and hjv-api's release
    # build (server/, which resolves hjv-relay's Cargo.toml, a dev-dependency, under --locked),
    # hjv-probe, hjv-release for the test policies, firewall-e2e.sh with the Caddyfile and guest-conf.py
    [e2e]="store/* .github/* docs/* LICENSE NOTICE tools/hjv-load/* tools/e2e/* !tools/e2e/firewall-e2e.sh
      deploy/* !deploy/Caddyfile !deploy/guest-conf.py relay/* !relay/Cargo.toml server/api/tests/* server/api/README.md
      scripts/* !scripts/fetch-upstream.sh !scripts/upstream.env !scripts/check-build-keys.py !scripts/public-test-keys.txt"
    # the E2E's client, hbbs, the release builds of hjv-api and hjv-relay, hjv-probe, wol-e2e.sh with
    # its helpers, firewall-e2e.sh (whose parts the client's build follows) and the Caddyfile
    [wol]="store/* .github/* docs/* LICENSE NOTICE tools/hjv-release/* tools/hjv-load/* tools/e2e/hjv-api-drill.sh
      deploy/* !deploy/Caddyfile server/api/tests/* server/api/README.md relay/tests/* relay/PROTOCOL.md
      scripts/* !scripts/fetch-upstream.sh !scripts/upstream.env !scripts/check-build-keys.py !scripts/public-test-keys.txt"
    # hjv-api, the fake SSO and the drill's client (server/api's tests/drill.rs with tests/common, and
    # hjv-relay, a dev-dependency), the units, the journal's retention and the drill script
    [drill:full]="client/* store/* .github/* docs/* LICENSE NOTICE server/patches/* scripts/* tools/*
      !tools/e2e/hjv-api-drill.sh deploy/* !deploy/systemd/* !deploy/journald@hjviewer.conf
      server/api/tests/* !server/api/tests/drill.rs !server/api/tests/common/* server/api/README.md relay/tests/*
      relay/PROTOCOL.md")
  REUSE_SKIP[drill:quick]=${REUSE_SKIP[drill:full]}
  # Last, for every unit: this script, whose code each unit takes by function instead (reuse_code), the
  # files that only prepush and the leak stage read, the hand tools, and the READMEs that no code reads
  REUSE_SKIP_ALL="scripts/ci.sh scripts/ci-reuse.py scripts/ci-times.sh scripts/test_ci_reuse.py scripts/build-signer.sh
    scripts/leak-check.py scripts/test_leak_check.py scripts/export-patches.sh scripts/make-icons.sh
    scripts/make-splash-text.py README.md tools/hjv-release/README.md"
  # The units whose inputs hold the hash of the 32-byte values of every tracked file: check-build-keys.py
  # takes them all for public test keys (its key tests, hjv-relay's refusals)
  REUSE_VALUES="server:relay client deploy"
  # Variables that name files outside the tree a run reads: a run with one neither reuses nor records
  REUSE_FILES_FROM="HJV_API_ENV_FILE HJV_RELEASE_SAMPLES HJV_UPDATE_VECTORS"
  # The crates whose own unit tests (#[cfg(test)] in src/) a unit compiles, which alone take what that
  # code reads (reuse_check); the others' builds leave that code out
  declare -gA REUSE_TESTS=([server:api]=server/api [server:relay]=relay [server:release]=tools/hjv-release)
  # References that a unit's code makes and does not follow, which reuse_check lets pass: "<unit> <file>
  # <path>", the file a tracked one or scripts/ci.sh:<function of this script>, the path with what is
  # under it
  REUSE_NOT_READ=(
    # fetch-upstream.sh server applies server/patches alone (Docker's image and the server package
    # fetch that too), fetch-upstream.sh client client/patches and the overlay alone
    "server:hbbs scripts/fetch-upstream.sh client"
    "deploy scripts/fetch-upstream.sh client"
    "client scripts/fetch-upstream.sh server/patches"
    # the refusals of relay/tests/package.rs stop before the notices; check-build-keys.py --relay
    # checks HJV_COMMAND_PUBKEY alone, not the app's key and version files
    "server:relay scripts/package-relay.sh scripts/package-notices.sh"
    "server:relay scripts/check-build-keys.py client"
    "deploy scripts/check-build-keys.py client"
    # hjv-release's default --keys, relative to where the operator runs it (its tests give theirs),
    # and the names of the workflows that its provenance check compares with GitHub's answers
    "server:release tools/hjv-release/src/commands.rs client/update-keys.env"
    "server:release tools/hjv-release/src/provenance.rs .github/workflows"
    "e2e tools/hjv-release/src/provenance.rs .github/workflows"
    # the wol stage reads firewall-e2e.sh for its parts (build_e2e_client) and does not run it
    "wol tools/e2e/firewall-e2e.sh deploy/guest-conf.py"
  )
}

# The top level of this script outside its functions (each ends at a line "}", here-documents aside):
# code, what every run of a unit runs of it (without comments, blank lines and the lines that only
# set values); values, the names of those values; functions, each function's name and lines
reuse_top() { # code|values|functions
  awk -v want="$1" '
    doc != "" { if ($0 == doc) doc = ""; next }
    fn != "" {
      if ($0 !~ /<<</ && match($0, /<<-? *'\''?[A-Za-z_]+'\''?/)) {
        doc = substr($0, RSTART, RLENGTH)
        gsub(/[<'\'' -]/, "", doc)
      }
      if ($0 == "}") {
        if (want == "functions") print fn, start, NR
        fn = ""
      }
      next
    }
    /^[a-z_][a-z0-9_]*\(\) \{/ {
      name = substr($0, 1, index($0, "(") - 1)
      if ($0 ~ /; \}$/) { if (want == "functions") print name, NR, NR }
      else { fn = name; start = NR }
      next
    }
    /^[[:space:]]*(#|$)/ { next }
    /^(declare -[aA]+ )?[A-Za-z_][A-Za-z0-9_]*(\[[^]]*\])?\+?=/ {
      if (want == "values") {
        line = $0
        sub(/^declare -[aA]+ /, "", line)
        while (match(line, /^[A-Za-z_][A-Za-z0-9_]*/)) {
          print substr(line, 1, RLENGTH)
          if (!match(line, /[[:space:]]+[A-Za-z_][A-Za-z0-9_]*(\[[^]]*\])?\+?=/)) break
          line = substr(line, RSTART)
          sub(/^[[:space:]]+/, "", line)
        }
      }
      next
    }
    want == "code" { print }
  ' "$SELF" | if [ "$1" = values ]; then LC_ALL=C sort -u; else cat; fi
}

# The code of this script that <function...> run: "f <name>" for them and every function they name,
# then "v <name>" for every value of its top level (reuse_top values) that one of them names
reuse_closure() { # <function...>
  local todo=("$@") seen=" " f w words=" " v
  while [ ${#todo[@]} -gt 0 ]; do
    f=${todo[0]}
    todo=("${todo[@]:1}")
    [[ $seen == *" $f "* ]] && continue
    seen+="$f "
    echo "f $f"
    for w in $(declare -f "$f" | grep -oE '[A-Za-z_][A-Za-z0-9_]*' | LC_ALL=C sort -u); do
      if declare -F "$w" > /dev/null; then todo+=("$w"); else words+="$w "; fi
    done
  done
  for v in $(reuse_top values); do
    if [[ $words == *" $v "* ]] && declare -p "$v" > /dev/null 2>&1; then echo "v $v"; fi
  done
}
# That code by line: "code <function> <SHA-256 of it as bash read it>" (without comments, so that a
# comment changes no unit) and "global <name> <SHA-256 of its value>", the CI directory as $CI and
# the path of this tree as $ROOT (another tree with the same inputs reuses the pass); with text, the
# code itself
reuse_code() { # [text] <function...>
  local kind name value text=""
  if [ "$1" = text ]; then text=1; shift; fi
  reuse_closure "$@" | while read -r kind name; do
    if [ "$kind" = f ]; then
      value=$(declare -f "$name")
    else
      value=$(declare -p "$name")
      value=${value//"$CI"/\$CI}
      value=${value//"$ROOT"/\$ROOT}
    fi
    if [ -n "$text" ]; then
      printf '%s\n' "$value"
    elif [ "$kind" = f ]; then
      echo "code $name $(printf '%s\n' "$value" | sha256sum | cut -d' ' -f1)"
    else
      echo "global $name $(printf '%s\n' "$value" | sha256sum | cut -d' ' -f1)"
    fi
  done | if [ -n "$text" ]; then cat; else LC_ALL=C sort; fi
}

# What <unit> runs that is not in the tree, by version: the Rust toolchain, Python, bash and git
# for all, beside them each stage's own (the deploy stage's packages as apt has them and its images
# as Docker has them here; a newer base image that Docker pulls later is the age limit's,
# HJV_CI_REUSE_HOURS)
reuse_tools() { # <unit>
  local lint lib image registry=${HJV_REGISTRY:-public.ecr.aws/docker/library}
  rustc -vV 2>&1 | paste -sd' '
  cargo -V 2>&1
  python3 --version 2>&1
  echo "bash $BASH_VERSION"
  git --version
  case $1 in
    server:hbbs | server:api | server:relay | e2e | wol | deploy)
      unshare --version 2>&1 | head -n 1
      ip -V 2>&1 ;;&
    server:api) systemd-analyze --version 2>&1 | head -n 1 ;;
    client)
      rustc +1.75.0 -vV 2>&1 | paste -sd' '
      echo "node $(node --version 2>&1 || echo none)"
      lint=$(command -v actionlint || ls "$HOME/.local/bin/actionlint" 2> /dev/null || true)
      if [ -n "$lint" ]; then echo "actionlint $("$lint" --version 2>&1 | head -n 1)"; else echo "actionlint none"; fi
      ;;
    deploy)
      echo "docker $(sudo -n docker version --format '{{.Server.Version}} {{.Client.Version}}' 2>&1)"
      echo "compose $(sudo -n docker compose version --short 2>&1)"
      echo "buildx $(sudo -n docker buildx version 2>&1)"
      for image in "$registry/ubuntu:24.04" "$registry/rust:1-bookworm" "$registry/debian:bookworm-slim" "$registry/caddy:2"; do
        reuse_image "$image"
      done
      apt-cache policy nginx apache2-bin libapr1t64 libaprutil1t64 libjansson4 liblua5.4-0 2> /dev/null |
        awk '/^[^ ]/ { p = $1 } /Candidate:/ { print "apt " p " " $2 }'
      curl --version 2>&1 | head -n 1
      openssl version 2>&1
      systemctl --version 2>&1 | head -n 1
      musl-gcc --version 2>&1 | head -n 1
      rustup target list --installed 2>&1 | paste -sd' '
      ;;
    e2e)
      sudo --version 2>&1 | head -n 1
      iptables --version 2>&1
      for lib in /usr/lib/x86_64-linux-gnu/faketime/libfaketime.so.1 "$CI/faketime/root/usr/lib/x86_64-linux-gnu/faketime/libfaketime.so.1"; do
        if [ -f "$lib" ]; then echo "libfaketime ${lib#"$CI/"} $(sha256sum < "$lib" | cut -d' ' -f1)"; fi
      done
      ;;&
    e2e | wol | drill:full | drill:quick) uname -r ;;&
    drill:full | drill:quick)
      echo "docker $(sudo -n docker version --format '{{.Server.Version}} {{.Client.Version}}' 2>&1)"
      reuse_image "$registry/ubuntu:24.04"
      ;;
  esac
}
# An image as Docker has it here, by its ID; none when it is not here
reuse_image() { # <image>
  local id
  id=$(sudo -n docker image inspect -f '{{.Id}}' "$1" 2> /dev/null || true)
  echo "image $1 ${id:-none}"
}
# The variables of the environment that a unit's run may read, as NAME=value: HJV_*, E2E_*, CARGO*,
# RUST* and GITHUB_ACTIONS, but HJV_CI_*, which say how prepush and the lock run
reuse_env() {
  local name
  for name in $(compgen -e | LC_ALL=C sort); do
    case $name in
      HJV_CI_*) ;;
      HJV_* | E2E_* | CARGO* | RUST* | GITHUB_ACTIONS) printf '%s=%q\n' "$name" "${!name}" ;;
    esac
  done
}
# The inputs of <unit...>, each in <dir>/<unit>.txt, whose SHA-256 is the key of its records: "file
# <mode> <blob> <path>" for the files of HEAD's tree that it reads (REUSE_SKIP), "code" and "global"
# lines for its code in this script (reuse_code), "top" for the code of this script's top level that
# every run runs, "tool" lines for what it runs outside the tree (reuse_tools), "env" lines for the
# environment (reuse_env), and "values" for those that read the 32-byte values of every tracked file
# (REUSE_VALUES). In a process of its own (scripts/ci.sh inputs): this script's values as they are
# at its start.
reuse_inputs() { # <dir> <unit...>
  local dir=$1 u rules=() fns top values=""
  shift
  reuse_rules
  mkdir -p "$dir"
  for u in "$@"; do
    [ -n "${REUSE_CODE[$u]:-}" ] || fail "no unit $u ($REUSE_UNITS)"
    rules+=("$u=${REUSE_SKIP[$u]} $REUSE_SKIP_ALL")
  done
  python3 -I -B "$ROOT/scripts/ci-reuse.py" files "$ROOT" "$dir" "${rules[@]}" || fail "the files that $* read (scripts/ci-reuse.py)"
  top=$(reuse_top code | sha256sum | cut -d' ' -f1)
  for u in "$@"; do
    if [[ " $REUSE_VALUES " == *" $u "* ]] && [ -z "$values" ]; then
      values=$(python3 -I -B "$ROOT/scripts/ci-reuse.py" values "$ROOT") || fail "the 32-byte values of the tree (scripts/ci-reuse.py)"
    fi
    read -ra fns <<< "${REUSE_CODE[$u]}"
    {
      echo "unit $u"
      cat "$dir/$u.files"
      reuse_code "${fns[@]}" finish
      echo "top $top"
      reuse_tools "$u" | sed 's/^/tool /'
      reuse_env | sed 's/^/env /'
      if [[ " $REUSE_VALUES " == *" $u "* ]]; then echo "values $values"; fi
    } > "$dir/$u.txt"
    rm -f "$dir/$u.files"
  done
}

# Whether <log> shows a strict pass of <unit> by prepush on a clean tree, with nothing skipped: its
# head is prepush's with HJV_CI_STRICT=1 on a tree without changes, it has no skipped line and no
# partial E2E, it has the unit's pass line of 0 skipped, and it ends with exit status 0; a part of the
# server stage that ran beside others has instead its own exit status in its stage's log (another
# part may have failed)
reuse_evidence() { # <unit> <log>
  local target
  [ -f "$2" ] || return 1
  target=$(head -n 1 "$2" | sed -n 's/^LOG scripts\/ci\.sh \([^ ]*\) (prepush, HJV_CI_STRICT=1, .*/\1/p')
  [ -n "$target" ] || return 1
  sed -n 2p "$2" | grep -q '^hjviewer [0-9a-f]\{40\} tree [0-9a-f]\{40\} status-lines 0 ' || return 1
  grep -qx "== CI passed: $1 (0 skipped)" "$2" || return 1
  if grep -q -e '^skipped:' -e '^PARTIAL' "$2"; then return 1; fi
  if [[ $1 == server:* ]] && [ "$target" != "$1" ]; then
    grep -q "^server: ${1#server:}: exit 0, " "$2"
  else
    tail -n 1 "$2" | grep -q '^EXIT 0 '
  fi
}
# A field of a record (reuse_record)
reuse_field() { # <record> <field>
  sed -n "s/^$2 //p" "$1"
}
# Records the pass of <unit> that <log> shows (reuse_evidence), on the inputs <inputs file>, while the
# tree is still <commit> and clean: build/ci/passes/<unit>/<key>.pass, and the inputs beside it as
# <key>.inputs for what a later plan says changed. A record of the same key is the newer pass's;
# records older than a week go.
reuse_record() { # <unit> <log> <inputs file> <commit>
  local unit=$1 log=$2 inputs=$3 head=$4 key dir="$CI/passes/$1" name
  reuse_rules
  for name in $REUSE_FILES_FROM; do [ -z "${!name:-}" ] || return 0; done
  reuse_evidence "$unit" "$log" || return 0
  [ "$(git -C "$ROOT" rev-parse HEAD)" = "$head" ] && [ -z "$(git -C "$ROOT" status --porcelain)" ] || return 0
  key=$(sha256sum < "$inputs" | cut -d' ' -f1)
  mkdir -p "$dir"
  cp "$inputs" "$dir/$key.inputs.$$"
  mv -f "$dir/$key.inputs.$$" "$dir/$key.inputs"
  printf '%s\n' "unit $unit" "key $key" "commit $head" "tree $(git -C "$ROOT" rev-parse 'HEAD^{tree}')" "root $ROOT" \
    "time $(date +%s)" "date $(date '+%Y-%m-%d %H:%M:%S %z')" "log $log" > "$dir/$key.pass.$$"
  mv -f "$dir/$key.pass.$$" "$dir/$key.pass"
  find "$dir" -type f -mtime +7 -delete
  echo "prepush: recorded the pass of $unit, inputs ${key:0:12} ($dir/$key.pass)"
}
# Whether <unit> is reused, by its inputs <inputs file>: "reuse <record>" for its recorded strict pass
# with the same inputs (key) of the last HJV_CI_REUSE_HOURS hours (24 by default: some tests depend
# on the date), whose log still shows it; else "run <why>". HJV_CI_FRESH=1 runs every unit, and so
# does HJV_CI_DEPLOY_ALL=1, a fresh deploy.
reuse_decide() { # <unit> <inputs file>
  local unit=$1 inputs=$2 key rec hours=${HJV_CI_REUSE_HOURS:-24} age newest name
  reuse_rules
  [[ $hours =~ ^[0-9]+$ ]] || fail "HJV_CI_REUSE_HOURS=$hours is no number of hours"
  if [ "${HJV_CI_FRESH:-}" = 1 ]; then echo "run HJV_CI_FRESH=1"; return 0; fi
  if [ "${HJV_CI_DEPLOY_ALL:-}" = 1 ]; then echo "run HJV_CI_DEPLOY_ALL=1, a fresh deploy: no unit is reused"; return 0; fi
  for name in $REUSE_FILES_FROM; do
    if [ -n "${!name:-}" ]; then echo "run $name names files outside the tree"; return 0; fi
  done
  key=$(sha256sum < "$inputs" | cut -d' ' -f1)
  rec="$CI/passes/$unit/$key.pass"
  if [ -f "$rec" ]; then
    age=$(($(date +%s) - $(reuse_field "$rec" time)))
    if [ "$age" -ge $((hours * 3600)) ]; then
      echo "run its pass with these inputs, of $(reuse_field "$rec" date | cut -c1-16), is $((age / 3600)) h old (HJV_CI_REUSE_HOURS=$hours)"
    elif ! reuse_evidence "$unit" "$(reuse_field "$rec" log)"; then
      echo "run its pass with these inputs has no log that shows it any more ($(reuse_field "$rec" log))"
    else
      echo "reuse $rec"
    fi
    return 0
  fi
  newest=$(find "$CI/passes/$unit" -name '*.pass' -printf '%T@ %p\n' 2> /dev/null | sort -rn | sed -n '1s/^[^ ]* //p')
  if [ -z "$newest" ]; then
    echo "run no pass of it recorded"
  else
    echo "run $(python3 -I -B "$ROOT/scripts/ci-reuse.py" diff "${newest%.pass}.inputs" "$inputs") changed since its pass" \
      "of $(reuse_field "$newest" commit | cut -c1-7) ($(reuse_field "$newest" date | cut -c1-16))"
  fi
}
# The plan of a prepush run for <unit...>: their inputs in <dir> (scripts/ci.sh inputs, a process of
# its own) and REUSE_PLAN[<unit>], "reuse <record>" or "run <why>" (reuse_decide); a full drill's
# pass serves a quick one. A unit that runs and takes a binary another unit builds, deploy, e2e and
# wol hbbs and hbbr of server:hbbs and deploy hjv-api of server:api, needs it from this tree's
# inputs: when its stamp says otherwise (stamp_mismatch), the unit that builds it runs too, though
# its own pass could be reused or prepush was not asked for it.
declare -A REUSE_PLAN=()
reuse_plan() { # <dir> <unit...>
  local dir=$1 u want c p bin why
  shift
  want=("$@")
  if [[ " $* " == *" drill:quick "* ]]; then want+=(drill:full); fi
  reuse_rules
  "$SELF" inputs "$dir" "${want[@]}" || fail "prepush: the units' inputs (scripts/ci.sh inputs)"
  for u in "${want[@]}"; do REUSE_PLAN[$u]=$(reuse_decide "$u" "$dir/$u.txt"); done
  if [[ " $* " == *" drill:quick "* ]] && [[ ${REUSE_PLAN[drill:full]} == reuse* ]]; then
    REUSE_PLAN[drill:quick]=${REUSE_PLAN[drill:full]}
  fi
  for c in deploy e2e wol; do
    if [[ " $* " != *" $c "* ]] || [[ ${REUSE_PLAN[$c]} != run* ]]; then continue; fi
    for p in "server:hbbs $CI/target/release/hbbs HBBS_INPUTS" "server:hbbs $CI/target/release/hbbr HBBS_INPUTS" \
      "server:api $API API_INPUTS"; do
      read -r u bin p <<< "$p"
      [ "$u" != server:api ] || { [ "$c" = deploy ] && [ -d "$ROOT/server/api" ]; } || continue
      [[ ${REUSE_PLAN[$u]:-} != run* ]] || continue
      case $p in
        HBBS_INPUTS) why=$(stamp_mismatch "$bin" "${HBBS_INPUTS[@]}") ;;
        API_INPUTS) why=$(stamp_mismatch "$bin" "${API_INPUTS[@]}") ;;
      esac
      [ -n "$why" ] || continue
      if [ -z "${REUSE_PLAN[$u]:-}" ]; then
        "$SELF" inputs "$dir" "$u" || fail "prepush: the inputs of $u (scripts/ci.sh inputs)"
      fi
      REUSE_PLAN[$u]="run $c runs and needs this tree's $(basename "$bin"): $why"
    done
  done
}
# The plan, a line for each unit of REUSE_PLAN in prepush's order
reuse_plan_lines() {
  local u rec
  reuse_rules
  for u in $REUSE_UNITS; do
    [ -n "${REUSE_PLAN[$u]:-}" ] || continue
    [ "$u" != drill:full ] || [ -z "${REUSE_PLAN[drill:quick]:-}" ] || continue
    if [[ ${REUSE_PLAN[$u]} == reuse* ]]; then
      rec=${REUSE_PLAN[$u]#reuse }
      printf '  %-6s %-15s %s\n' reuse "$u" "$(reuse_said "$rec")"
    else
      printf '  %-6s %-15s %s\n' run "$u" "${REUSE_PLAN[$u]#run }"
    fi
  done
}
# A record as the plan and a reused unit's log say it
reuse_said() { # <record>
  local age=$(($(date +%s) - $(reuse_field "$1" time)))
  echo "its strict pass of $(reuse_field "$1" commit | cut -c1-7) at $(reuse_field "$1" date | cut -c1-16)" \
    "($((age / 3600)) h $((age % 3600 / 60)) min ago$([ "$(reuse_field "$1" unit)" = drill:full ] && echo ', a full drill')), log $(reuse_field "$1" log)"
}

# The rules of the reuse held to the code (the leak stage): that what each unit's code reads lies in
# its inputs (scripts/ci-reuse.py reads, but for REUSE_NOT_READ), that the re-run table of docs/ci.md
# names the units the rules give (table), that no unit's code is the code of prepush, its plan or
# the lock (which a unit's inputs leave out), and that reuse_top finds the functions bash finds
reuse_check() {
  local dir u fns rules=() table=() names
  reuse_rules
  dir=$(mktemp -d)
  for u in $REUSE_UNITS; do
    rules+=("$u=${REUSE_SKIP[$u]//$'\n'/ }")
    [ "$u" = drill:quick ] || table+=("${u%:full}=${REUSE_SKIP[$u]} $REUSE_SKIP_ALL")
    read -ra fns <<< "${REUSE_CODE[$u]}"
    reuse_code text "${fns[@]}" finish > "$dir/$u.code"
    names=$(reuse_closure "${fns[@]}" finish | sed -n 's/^f //p' | grep -E '^(prepush_|reuse_|lock_|preflight|log_head$)' | paste -sd' ' || true)
    [ -z "$names" ] || fail "reuse: the code of $u takes the code of prepush, its plan or the lock ($names), which no unit's inputs have"
  done
  printf '%s\n' "${rules[@]}" > "$dir/rules"
  echo "$REUSE_SKIP_ALL" > "$dir/all"
  for u in "${!REUSE_TESTS[@]}"; do echo "$u ${REUSE_TESTS[$u]}"; done > "$dir/tests"
  printf '%s\n' "${REUSE_NOT_READ[@]}" > "$dir/not-read"
  python3 -I -B "$ROOT/scripts/ci-reuse.py" reads "$ROOT" "$dir" ||
    fail "reuse: a unit's code reads a path its rules leave out (above): REUSE_SKIP, or REUSE_NOT_READ with the reason"
  python3 -I -B "$ROOT/scripts/ci-reuse.py" table "$ROOT" "$ROOT/docs/ci.md" "${table[@]}" ||
    fail "reuse: docs/ci.md's re-run table and the rules of scripts/ci.sh disagree (above)"
  # the functions as reuse_top finds them (where each ends: a code line of a function would be top
  # level code of every unit) are bash's, each with its own code
  [ "$(reuse_top functions | cut -d' ' -f1 | LC_ALL=C sort)" = "$(declare -F | sed 's/^declare -f //' | LC_ALL=C sort)" ] ||
    fail "reuse: reuse_top does not find the functions that bash finds in scripts/ci.sh"
  names=$(reuse_top functions | bash -c 'mapfile -t l < "$1"
    while read -r f a b; do eval "$(printf "%s\n" "${l[@]:a-1:b-a+1}")" && declare -f "$f"; done' _ "$SELF" |
    diff - <(reuse_top functions | while read -r u _; do declare -f "$u"; done) | head -n 5 || true)
  [ -z "$names" ] || fail "reuse: the lines that reuse_top takes for a function of scripts/ci.sh are not its code: $names"
  echo "reuse: $(reuse_top functions | wc -l) functions, $(reuse_top code | wc -l) lines of top-level code"
  rm -rf "$dir"
}

# --- prepush ---

# The jobs of a prepush run: their process, when they started, the lines that their log has after
# its head, and the run's log folder
declare -A PREPUSH_PID=() PREPUSH_START=() PREPUSH_HEAD=()
PREPUSH_LOGS=""
# Starts <target> of this script as a job of this prepush run, with HJV_CI_STRICT=1 and its log
# <log folder>/<name>.log: prepush's head first, then PREPUSH_HEAD[<name>], its output, last its
# exit status. With nice it runs with a lower priority, for CPU and disk, than the jobs beside it.
prepush_start() { # <name> <target> [nice]
  local name=$1 target=$2 pre=()
  [ "${3:-}" != nice ] || pre=(nice -n 10 ionice -c 2 -n 7)
  PREPUSH_START[$name]=$(date +%s)
  (
    trap 'echo "EXIT $? $(date "+%Y-%m-%d %H:%M:%S %z")"' EXIT
    log_head "$target"
    if [ -n "${PREPUSH_HEAD[$name]:-}" ]; then printf '%s\n' "${PREPUSH_HEAD[$name]}"; fi
    env --default-signal=INT,QUIT HJV_CI_STRICT=1 "${pre[@]}" "$SELF" "$target"
  ) > "$PREPUSH_LOGS/$name.log" 2>&1 < /dev/null &
  PREPUSH_PID[$name]=$!
}
# Waits for the job <name> of prepush_start, keeps the side logs that it wrote, says how it ended
# and how long it took (summary.txt: by its log's EXIT line, since the jobs beside it are waited for
# one after another), puts its steps' times in times.tsv, and records the passes of <unit...> that
# its log shows (reuse_record, a server stage's parts also when another failed). Its status is 1
# unless the job ended with 0 and its log with the pass line of <target> and no skipped line; then
# the last lines of its log come too.
prepush_end() { # <name> <target> <commit> [<unit>...]
  local name=$1 target=$2 head=$3 log="$PREPUSH_LOGS/$1.log" status=0 f u base ended
  shift 3
  wait "${PREPUSH_PID[$name]}" || status=$?
  unset "PREPUSH_PID[$name]"
  ended=$(date -d "$(sed -n 's/^EXIT [0-9]* //p' "$log" | tail -n 1)" +%s 2> /dev/null || date +%s)
  base=${target%%:*}
  for f in ${SIDE_LOGS[$base]:-}; do
    if [ "$(stat -c %Y "$CI/$f" 2> /dev/null || echo 0)" -ge "${PREPUSH_START[$name]}" ]; then
      cp -p "$CI/$f" "$PREPUSH_LOGS/$base-${f#"$base"-}"
    fi
  done
  sed -n "/^-- time by step/,/^\$/s/^ *\([0-9][0-9]*\) s  /$name\t\1\t/p" "$log" >> "$PREPUSH_LOGS/times.tsv"
  echo "prepush: $name: exit $status, $((ended - PREPUSH_START[$name])) s, $(grep -c '^skipped:' "$log" || true)" \
    "skipped line(s), log $log" | tee -a "$PREPUSH_LOGS/summary.txt"
  for u in "$@"; do reuse_record "$u" "$log" "$PREPUSH_LOGS/inputs/$u.txt" "$head" | tee -a "$PREPUSH_LOGS/summary.txt"; done
  [ "$(git -C "$ROOT" rev-parse HEAD)" = "$head" ] && [ -z "$(git -C "$ROOT" status --porcelain)" ] ||
    fail "prepush: the tree changed during $name; its log is not of $head"
  if [ "$status" = 0 ] && grep -qx "== CI passed: $target (0 skipped)" "$log" && ! grep -q '^skipped:' "$log"; then
    return 0
  fi
  echo "prepush: $name did not pass; the end of $log:"
  tail -n 25 "$log"
  return 1
}
# Stops the job <name> while it runs, its processes with it; their exit traps clean up after them
prepush_stop_all() {
  local name
  for name in "${!PREPUSH_PID[@]}"; do prepush_stop "$name"; done
}
prepush_stop() { # <name>
  [ -n "${PREPUSH_PID[$1]:-}" ] || return 0
  echo "prepush: stopping $1, which a failure before makes pointless" | tee -a "$PREPUSH_LOGS/summary.txt"
  end_tree "${PREPUSH_PID[$1]}"
  wait "${PREPUSH_PID[$1]}" 2> /dev/null || true
  unset "PREPUSH_PID[$1]"
}
# The log of a stage whose units are all reused: prepush's head, the passes it reuses, and a last
# line "== CI reused: <name> (...)" before the exit status
prepush_reused() { # <name> <unit...>
  local name=$1 log="$PREPUSH_LOGS/$1.log" u
  shift
  {
    log_head "$name" "prepush, reused"
    for u in "$@"; do reuse_lines "$u"; done
    printf '\n== CI reused: %s (%s)\n' "$name" "$*"
    echo "EXIT 0 $(date '+%Y-%m-%d %H:%M:%S %z')"
  } > "$log"
  echo "prepush: $name: reused ($*), log $log" | tee -a "$PREPUSH_LOGS/summary.txt"
}
# What a log says of a reused unit
reuse_lines() { # <unit>
  local rec=${REUSE_PLAN[$1]#reuse }
  echo "REUSED $1: $(reuse_said "$rec")"
  echo "REUSED $1: on $(reuse_field "$rec" commit) in $(reuse_field "$rec" root), inputs $(reuse_field "$rec" key)" \
    "($rec), the same as this run's ($PREPUSH_LOGS/inputs/$1.txt)"
}

# The gate before a push (design v3 11 step 1), on a clean tree, each unit with HJV_CI_STRICT=1 and
# a log of its own (prepush_start), in this order: the leak stage and the preflight, without the
# lock; then under it the server stage, its parts side by side; then the deploy stage, with the
# client stage (in a process of its own, as its exit trap needs), e2e:build and the drill's build
# beside it, those three with a lower priority than the deploy stage, whose checks wait on timers
# with seconds to spare; then the e2e stage, with the drill (--drill) beside it; then the wol stage,
# while the drill ends. A unit whose strict pass on the same inputs can be reused (reuse_plan) does
# not run, and its log says so; --plan says what would run or be reused and why, and runs nothing.
# A unit that fails stops what has not started; the jobs beside it end as they would (those beside
# the deploy stage within a minute or two; the deploy stage holds systemd units and containers that
# its cleanup takes down), but the drill, which a failure of the e2e or wol stage stops (its exit
# trap removes its containers). Named stages run alone: after a fix, those that read what it changed
# (docs/ci.md), with --drill also the drill.
prepush_run() { # [--plan] [--drill[=full|quick]] [<stage>...]
  local a plan="" drill="" stages=() units=() s u parts=() head list run=() reused=() n failed=() started=() t
  reuse_rules
  for a in "$@"; do
    case $a in
      --plan) plan=1 ;;
      --drill | --drill=full) drill=full ;;
      --drill=quick) drill=quick ;;
      -*) fail "prepush: no option $a (--plan, --drill[=full|quick])" ;;
      *)
        [[ " $PREPUSH_STAGES " == *" $a "* ]] || [ -n "$(server_parts "$a")" ] ||
          fail "prepush: no stage $a ($PREPUSH_STAGES; server:<part>[,<part>] of $SERVER_PARTS)"
        stages+=("$a")
        ;;
    esac
  done
  [ ${#stages[@]} -gt 0 ] || read -ra stages <<< "$PREPUSH_STAGES"
  for s in "${stages[@]}"; do
    case $s in
      leak) ;;
      server | server:*)
        read -ra parts <<< "$(server_parts "$s")"
        for u in "${parts[@]}"; do [[ " ${units[*]} " == *" server:$u "* ]] || units+=("server:$u"); done
        ;;
      *) [[ " ${units[*]} " == *" $s "* ]] || units+=("$s") ;;
    esac
  done
  [ -z "$drill" ] || units+=("drill:$drill")
  # a partial run of strict mode (docs/e2e.md) is for a fix loop and never a pass
  [ -z "${HJV_E2E_PARTS:-}" ] || fail "prepush: HJV_E2E_PARTS=$HJV_E2E_PARTS runs a part of the E2E, which is never a pass"
  [ -z "$E2E_CLIENT_GIVEN" ] || fail "prepush: E2E_CLIENT is set: prepush builds the E2E's client from client/patches"
  if [ -n "$plan" ]; then
    [ -z "$(git -C "$ROOT" status --porcelain)" ] ||
      echo "prepush --plan: the working tree has changes, which prepush does not take: the plan of HEAD"
    PREPUSH_LOGS=$(mktemp -d)
    reuse_plan "$PREPUSH_LOGS/inputs" "${units[@]}"
    echo "prepush plan of $(git -C "$ROOT" rev-parse --short HEAD) (records of the last ${HJV_CI_REUSE_HOURS:-24} h in $CI/passes):"
    reuse_plan_lines
    rm -rf "$PREPUSH_LOGS"
    exit 0
  fi
  [ -z "$(git -C "$ROOT" status --porcelain)" ] ||
    fail "prepush: the working tree is not clean (git status): a push publishes commits, and they are what is checked"
  head=$(git -C "$ROOT" rev-parse HEAD)
  if [ -z "${HJV_CI_PREPUSH:-}" ]; then
    # What needs no lock: the log folder, the leak stage and the preflight. The leak stage comes
    # first: a hit or a broken rule of the reuse costs seconds, not the run.
    if [[ " ${stages[*]} " == *" leak "* ]]; then
      list=${HJV_LEAK_PATTERNS:-$ROOT/build/leak-patterns.txt}
      if [ -z "${HJV_LEAK_PATTERNS:-}" ] && [ ! -f "$list" ]; then
        list="$(dirname "$(git -C "$ROOT" rev-parse --path-format=absolute --git-common-dir)")/build/leak-patterns.txt"
      fi
      [ -f "$list" ] || fail "prepush: no list of values for the leak stage ($list; HJV_LEAK_PATTERNS, docs/ci.md)"
    fi
    PREPUSH_LOGS=${HJV_CI_LOGS:-$CI/prepush/$(date '+%Y%m%d-%H%M%S')-$(git -C "$ROOT" rev-parse --short HEAD)}
    mkdir -p "$PREPUSH_LOGS"
    HJV_CI_RUN="$(date +%s)-$$"
    export HJV_CI_RUN
    step "prepush: ${stages[*]}${drill:+ and the $drill drill} on $head, logs in $PREPUSH_LOGS"
    # summary.txt: what prepush says of each unit; times.tsv: each stage's steps by time
    echo "prepush: ${stages[*]}${drill:+ --drill=$drill} on $head in $ROOT, $(date '+%Y-%m-%d %H:%M:%S %z')" > "$PREPUSH_LOGS/summary.txt"
    : > "$PREPUSH_LOGS/times.tsv"
    if [[ " ${stages[*]} " == *" leak "* ]]; then
      step "prepush: leak"
      prepush_start leak leak
      prepush_end leak leak "$head" || fail "prepush: leak did not pass ($PREPUSH_LOGS/leak.log)"
    fi
    step "prepush: what the stages need of this machine"
    preflight "${units[@]}"
    [ ${#units[@]} -gt 0 ] || prepush_passed
    if [ "${HJV_CI_LOCK:-}" != "$CI" ]; then
      export HJV_CI_PREPUSH=$PREPUSH_LOGS
      step "prepush: the lock of $CI"
      lock_run prepush "$@"
    fi
    export HJV_CI_PREPUSH=$PREPUSH_LOGS
  fi
  PREPUSH_LOGS=$HJV_CI_PREPUSH
  lock_held prepush "$@"
  # a job still running when prepush stops early (set -e, a signal) is stopped with it
  trap prepush_stop_all EXIT
  preflight_ports "${units[@]}"
  step "prepush: the plan (the records of the last ${HJV_CI_REUSE_HOURS:-24} h in $CI/passes)"
  reuse_plan "$PREPUSH_LOGS/inputs" "${units[@]}"
  # the server stage's parts that the plan added, to build what a unit that runs takes
  for u in "${!REUSE_PLAN[@]}"; do
    if [[ $u == server:* ]] && [[ " ${units[*]} " != *" $u "* ]]; then units=("$u" "${units[@]}"); fi
  done
  reuse_plan_lines | tee -a "$PREPUSH_LOGS/summary.txt"
  for u in "${units[@]}"; do
    if [[ ${REUSE_PLAN[$u]} == reuse* ]]; then reused+=("$u"); else run+=("$u"); fi
  done
  # the server stage: the parts that run side by side, those reused named in its log's head
  parts=()
  for u in $REUSE_UNITS; do
    if [[ $u == server:* ]] && [[ " ${run[*]} " == *" $u "* ]]; then parts+=("${u#server:}"); fi
  done
  if [ ${#parts[@]} -gt 0 ]; then
    t="server:$(IFS=,; echo "${parts[*]}")"
    [ ${#parts[@]} != 4 ] || t=server
    PREPUSH_HEAD[server]=$(for u in "${reused[@]}"; do [[ $u != server:* ]] || reuse_lines "$u"; done)
    step "prepush: $t"
    prepush_start server "$t"
    prepush_end server "$t" "$head" "${parts[@]/#/server:}" || fail "prepush: server did not pass ($PREPUSH_LOGS/server.log)"
  else
    parts=()
    for u in "${reused[@]}"; do if [[ $u == server:* ]]; then parts+=("$u"); fi; done
    [ ${#parts[@]} -eq 0 ] || prepush_reused server "${parts[@]}"
  fi
  # beside the deploy stage
  for u in client deploy e2e wol "drill:${drill:-}"; do
    if [[ " ${reused[*]} " == *" $u "* ]]; then prepush_reused "${u%%:*}" "$u"; fi
  done
  started=()
  if [[ " ${run[*]} " == *" deploy "* ]]; then prepush_start deploy deploy; started+=(deploy); fi
  if [[ " ${run[*]} " == *" client "* ]]; then prepush_start client client nice; started+=(client); fi
  if [[ " ${run[*]} " == *" e2e "* ]]; then prepush_start e2e-build e2e:build nice; started+=(e2e-build); fi
  if [[ " ${run[*]} " == *" drill:"* ]]; then prepush_start drill-build drill:build nice; started+=(drill-build); fi
  if [ ${#started[@]} -gt 0 ]; then
    step "prepush: ${started[*]} side by side"
    for n in "${started[@]}"; do
      case $n in
        deploy | client) prepush_end "$n" "$n" "$head" "$n" || failed+=("$n") ;;
        e2e-build) prepush_end "$n" e2e:build "$head" || failed+=("$n") ;;
        drill-build) prepush_end "$n" drill:build "$head" || failed+=("$n") ;;
      esac
    done
    [ ${#failed[@]} -eq 0 ] || fail "prepush: ${failed[*]} did not pass (above, and in $PREPUSH_LOGS)"
  fi
  # the e2e stage with the drill beside it, then the wol stage while the drill ends
  if [[ " ${run[*]} " == *" drill:"* ]]; then prepush_start drill "drill:$drill"; fi
  if [[ " ${run[*]} " == *" e2e "* ]]; then
    step "prepush: e2e${PREPUSH_PID[drill]:+, the $drill drill beside it}"
    prepush_start e2e e2e
    prepush_end e2e e2e "$head" e2e || { prepush_stop drill; fail "prepush: e2e did not pass ($PREPUSH_LOGS/e2e.log)"; }
  fi
  if [[ " ${run[*]} " == *" wol "* ]]; then
    step "prepush: wol${PREPUSH_PID[drill]:+, while the $drill drill ends}"
    prepush_start wol wol
    prepush_end wol wol "$head" wol || { prepush_stop drill; fail "prepush: wol did not pass ($PREPUSH_LOGS/wol.log)"; }
  fi
  if [ -n "${PREPUSH_PID[drill]:-}" ]; then
    step "prepush: the $drill drill"
    prepush_end drill "drill:$drill" "$head" "drill:$drill" || fail "prepush: the $drill drill did not pass ($PREPUSH_LOGS/drill.log)"
  fi
  trap - EXIT
  prepush_passed "${#units[@]}" "${reused[@]}"
}
# The last lines of a prepush run that passed: how long it took since its start, with the units of
# <count> that it reused, and its pass line, which counts them
prepush_passed() { # [<count of units> <reused unit>...]
  local count=${1:-0}
  shift || true
  echo "prepush: passed, $(($(date +%s) - ${HJV_CI_RUN%%-*})) s, $# of $count units reused${*:+ ($*)}" |
    tee -a "$PREPUSH_LOGS/summary.txt"
  owned_built
  step_times
  printf '\n== %s\n' "CI passed: prepush (0 skipped, $# reused)"
  exit 0
}

# The last lines of a run: its steps by time, what it skipped, and with HJV_CI_STRICT=1 a failure if
# anything; the line "== CI passed: ..." comes last, with no time line under it
finish() { # <target>
  local s
  owned_built
  step_times
  if [ ${#SKIPPED[@]} -gt 0 ]; then
    printf '\n%s skipped in this run:\n' "${#SKIPPED[@]}"
    for s in "${SKIPPED[@]}"; do echo "  - $s"; done
    if [ "${HJV_CI_STRICT:-}" = 1 ]; then
      echo "FAIL: HJV_CI_STRICT=1 and $1 skipped ${#SKIPPED[@]} part(s), listed above: a pass with skipped lines is no pass before a push" >&2
      exit 1
    fi
  fi
  printf '\n== %s\n' "CI passed: $1 (${#SKIPPED[@]} skipped)"
}

# Sourced (scripts/test_ci_reuse.py), this file only defines what is above
[ "${BASH_SOURCE[0]}" = "$0" ] || return 0
# Every run is strict unless HJV_CI_STRICT=0 says otherwise, but for a partial E2E (HJV_E2E_PARTS),
# which is never a pass anyway
if [ -z "${HJV_CI_STRICT+set}" ]; then
  HJV_CI_STRICT=1
  [ -z "${HJV_E2E_PARTS:-}" ] || HJV_CI_STRICT=0
fi
export HJV_CI_STRICT
reuse_rules
case "$TARGET" in
  # what reads only the tree and writes nothing under the CI directory takes no lock
  leak | inputs) ;;
  # prepush takes it itself, after the leak stage and the preflight
  prepush) shift; prepush_run "$@" ;;
  *)
    if [ "${HJV_CI_LOCK:-}" != "$CI" ]; then lock_run "$@"; fi
    lock_held "$@"
    ;;
esac
# the head of a run of its own and what it needs (a stage that prepush runs, with HJV_CI_RUN, has the
# head in its log, and a part that a server stage runs beside the others, with HJV_CI_PART, in that
# stage's); such a run ends with "== CI passed: ..." or "FAIL: ...", and only prepush's logs with an
# EXIT line
case "$TARGET" in
  all | server | server:* | client | deploy | e2e | e2e:build | wol | leak | drill:*)
    if [ -z "${HJV_CI_RUN:-}${HJV_CI_PART:-}" ]; then
      log_head "$TARGET" "on its own, HJV_CI_STRICT=$HJV_CI_STRICT"
      preflight "$TARGET"
    fi
    ;;
esac
case "$TARGET" in
  server | server:*) server_stage "$TARGET" ;;
  client) client_stage ;;
  deploy) deploy_stage ;;
  e2e:build) e2e_build_stage ;;
  e2e) e2e_stage ;;
  wol) wol_stage ;;
  leak) leak_stage ;;
  drill:*) drill_stage "$TARGET" ;;
  all) server_stage server; deploy_stage ;;
  inputs) shift; reuse_inputs "$@"; exit 0 ;;
  *)
    echo "usage: $0 [all|prepush [--plan] [--drill[=full|quick]] [<stage>...]|server[:<part>[,<part>]]|client|deploy|e2e[:build]|wol|leak|drill:build|drill:full|drill:quick]" >&2
    echo "  all is the server and deploy stages only; prepush runs every stage ($PREPUSH_STAGES) with HJV_CI_STRICT=1" >&2
    echo "  server:<part>[,<part>] (also a stage of prepush) runs those of the server stage's parts: $SERVER_PARTS" >&2
    echo "  inputs <dir> <unit>... writes what prepush's units read ($REUSE_UNITS) into <dir>" >&2
    exit 2
    ;;
esac
finish "$TARGET"

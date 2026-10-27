#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Packages the server for servers that run it as systemd services, without Docker
# (docs/server-runbook.md, path B), as a GitHub release asset:
#   scripts/package-server.sh <version> [<out dir>]
# writes <out dir>/hjviewer-server-<version>-linux-x86_64.tar.gz and <out dir>/SHA256SUMS with that
# file's line only (hjv-relay's package goes to build/release-relay). The archive holds
#   bin/      hbbs and hbbr, built on Ubuntu 24.04 from a fresh upstream checkout with server/patches
#             (so they need glibc 2.39 or later), hjv-api, the account API (server/api), and
#             hjv-update-verify.sh, the server's check of a signed version policy (python3, openssl)
#   systemd/  the units of deploy/systemd: hbbs, hbbr, hjv-api's socket and service, and the
#             hjviewer-api-* services and timers that watch and back it up; hjviewer.env.example
#             and hjviewer-api.env.example; journald@hjviewer.conf, hjv-api's journal namespace
#   web/      the nginx and Apache files of deploy/, among them nginx-hjviewer-api.conf (the /api/
#             locations once the account API is on) and nginx-hjviewer-api-zones.conf (their zones,
#             at the http level)
#   LICENSE, NOTICE, SOURCE and THIRD_PARTY_NOTICES (hjv-api's crates too)
# Without server/api (design v3 10-2: kept out of the public repository) the hjv-api parts are left
# out.
#
# Binaries to hand out must be of a commit that is in the public repository, since their notices
# point there: the script stops on uncommitted changes or a commit that origin/main does not
# contain. HJV_SKIP_SOURCE_CHECK=1 skips that for local tests. HJV_SERVER_BIN_DIR=<dir> packages
# hbbs/hbbr built already and HJV_API_BIN=<file> an hjv-api built already (scripts/ci.sh server
# leaves them in build/ci/target/release and build/ci/target-api/release).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
version=${1:?usage: $0 <version> [<out dir>]}
out=${2:-$ROOT/build/release}
work="$ROOT/build/package-server"
name="hjviewer-server-$version-linux-x86_64"
export PATH="$HOME/.cargo/bin:$PATH"

if [ -z "${HJV_SKIP_SOURCE_CHECK:-}" ]; then
  if [ -n "$(git -C "$ROOT" status --porcelain)" ]; then
    echo "uncommitted changes: commit and push them first" >&2
    exit 1
  fi
  git -C "$ROOT" fetch -q origin main
  if ! git -C "$ROOT" merge-base --is-ancestor HEAD origin/main; then
    echo "$(git -C "$ROOT" rev-parse --short HEAD) is not in origin/main: push it first" >&2
    exit 1
  fi
fi

rm -rf "$work/upstream"
UPSTREAM_DIR="$work/upstream" "$ROOT/scripts/fetch-upstream.sh" server
bin=${HJV_SERVER_BIN_DIR:-}
if [ -z "$bin" ]; then
  (cd "$work/upstream/rustdesk-server" &&
    CARGO_TARGET_DIR="$work/target" cargo build --release --locked --bin hbbs --bin hbbr)
  bin="$work/target/release"
fi
api="" api_commit=""
if [ -d "$ROOT/server/api" ]; then
  api=${HJV_API_BIN:-}
  if [ -z "$api" ]; then
    api_commit=$(git -C "$ROOT" rev-parse HEAD)
    (cd "$ROOT/server/api" && HJV_API_COMMIT=$api_commit \
      CARGO_TARGET_DIR="$work/target-api" cargo build --release --locked --bin hjv-api)
    api="$work/target-api/release/hjv-api"
  else
    # the commit it was built with, as it reports it (scripts/ci.sh server builds it with the last
    # commit that changed server/api)
    api_commit=$("$api" version | sed -n 's/^hjv-api [^ ]* (\(.*\))$/\1/p')
  fi
else
  echo "no server/api: the package has no hjv-api"
fi

stage="$work/$name"
rm -rf "$stage"
mkdir -p "$stage/bin" "$stage/systemd" "$stage/web"
install -m 755 "$bin/hbbs" "$bin/hbbr" "$stage/bin/"
install -m 755 "$ROOT/deploy/hjv-update-verify.sh" "$stage/bin/"
install -m 644 "$ROOT/deploy/systemd/hjviewer.env.example" "$stage/systemd/"
install -m 644 "$ROOT/deploy/nginx-hjviewer-locations.conf" "$ROOT/deploy/nginx-hjviewer.conf" \
  "$ROOT/deploy/apache-hjviewer.conf" "$stage/web/"
shopt -s nullglob
for unit in "$ROOT"/deploy/systemd/*.service "$ROOT"/deploy/systemd/*.socket "$ROOT"/deploy/systemd/*.timer; do
  case $(basename "$unit") in
    hjviewer-api*) [ -n "$api" ] || continue ;;
  esac
  install -m 644 "$unit" "$stage/systemd/"
done
shopt -u nullglob
extra=()
if [ -n "$api" ]; then
  install -m 755 "$api" "$stage/bin/hjv-api"
  install -m 644 "$ROOT/server/api/hjviewer-api.env.example" "$stage/systemd/"
  if [ -f "$ROOT/deploy/journald@hjviewer.conf" ]; then
    install -m 644 "$ROOT/deploy/journald@hjviewer.conf" "$stage/systemd/"
  fi
  install -m 644 "$ROOT/deploy/nginx-hjviewer-api.conf" "$ROOT/deploy/nginx-hjviewer-api-zones.conf" "$stage/web/"
  extra=("$ROOT/server/api")
fi
NOTICES_COMPONENT=server NOTICES_TARGET=x86_64-unknown-linux-gnu NOTICES_EXTRA_WORKSPACES="${extra[*]}" \
  "$ROOT/scripts/package-notices.sh" "$stage" "$work/upstream/rustdesk-server"
if [ -n "$api" ]; then
  {
    echo "(server/api holds the whole source of hjv-api; it was built there with"
    if [ "$api_commit" = "$(git -C "$ROOT" rev-parse HEAD)" ]; then
      echo "cargo build --release --locked --bin hjv-api and HJV_API_COMMIT set to the commit above)"
    elif git -C "$ROOT" diff --quiet "$api_commit" HEAD -- server/api 2> /dev/null; then
      echo "cargo build --release --locked --bin hjv-api and HJV_API_COMMIT set to"
      echo "$api_commit, whose server/api is that of the commit above)"
    else
      echo "cargo build --release --locked --bin hjv-api and HJV_API_COMMIT set to"
      echo "$api_commit)"
    fi
  } >> "$stage/SOURCE.txt"
fi

mkdir -p "$out"
tar -C "$work" --owner=0 --group=0 --numeric-owner --sort=name -czf "$out/$name.tar.gz" "$name"
(cd "$out" && sha256sum "$name.tar.gz" > SHA256SUMS && cat SHA256SUMS)
tar -tzvf "$out/$name.tar.gz" | awk '{print $1, $3, $6}'

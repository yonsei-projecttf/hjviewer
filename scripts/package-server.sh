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
# leaves them in build/ci/target/release and build/ci/target-api/release). Only the binaries it
# builds itself are free of the build machine's paths (remap_flags below): a release builds them
# here, without those two.
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

# The RUSTFLAGS of this script's own builds, which keep the build machine's paths out of the
# binaries (panic messages and debug info name source files): the cargo home's crates become
# /cargo/registry/src/..., the toolchain's sources (where rust-src is installed) /rustc/<the
# toolchain's commit>/library/..., as rustc names them without rust-src, and this tree, the
# upstream checkout under build/ among it, /hjviewer/.... The same tree and toolchain give the
# same bytes in any directory and home. Binaries passed in (HJV_SERVER_BIN_DIR, HJV_API_BIN) are
# packaged as they are.
remap_flags() { # <the directory cargo builds in, for its toolchain>
  local sysroot hash
  sysroot=$(cd "$1" && rustc --print sysroot)
  hash=$(cd "$1" && rustc -vV | sed -n 's/^commit-hash: //p')
  [ -n "$hash" ] || { echo "rustc -vV names no commit-hash" >&2; return 1; }
  # rustc takes the last of the prefixes that match a path
  echo "${RUSTFLAGS:-} --remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo" \
    "--remap-path-prefix=$sysroot/lib/rustlib/src/rust=/rustc/$hash --remap-path-prefix=$ROOT=/hjviewer"
}
# Fails when a binary this script built names the build machine's home directory or this tree
no_build_paths() { # <binary...>
  local f
  for f in "$@"; do
    if grep -q -a -F -e "$HOME/" -e "$ROOT/" "$f"; then
      echo "$f names a path of this machine ($HOME/ or $ROOT/): --remap-path-prefix missed it" >&2
      return 1
    fi
  done
}

rm -rf "$work/upstream"
UPSTREAM_DIR="$work/upstream" "$ROOT/scripts/fetch-upstream.sh" server
bin=${HJV_SERVER_BIN_DIR:-}
# the rustc of each of this script's own builds, for SOURCE.txt (empty: a binary passed in)
server_rustc="" api_rustc=""
if [ -z "$bin" ]; then
  server_rustc=$(cd "$work/upstream/rustdesk-server" && rustc -V)
  flags=$(remap_flags "$work/upstream/rustdesk-server")
  (cd "$work/upstream/rustdesk-server" &&
    RUSTFLAGS=$flags CARGO_TARGET_DIR="$work/target" cargo build --release --locked --bin hbbs --bin hbbr)
  bin="$work/target/release"
  no_build_paths "$bin/hbbs" "$bin/hbbr"
fi
api="" api_commit=""
if [ -d "$ROOT/server/api" ]; then
  api=${HJV_API_BIN:-}
  if [ -z "$api" ]; then
    api_commit=$(git -C "$ROOT" rev-parse HEAD)
    api_rustc=$(cd "$ROOT/server/api" && rustc -V)
    flags=$(remap_flags "$ROOT/server/api")
    (cd "$ROOT/server/api" && HJV_API_COMMIT=$api_commit RUSTFLAGS=$flags \
      CARGO_TARGET_DIR="$work/target-api" cargo build --release --locked --bin hjv-api)
    api="$work/target-api/release/hjv-api"
    no_build_paths "$api"
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
# How the binaries were built, so that a rebuild can compare bytes: those this script built carry
# remap_flags' RUSTFLAGS, and a build without them gives other bytes (the paths in them differ)
remapped=("the RUSTFLAGS of scripts/package-server.sh's remap_flags (--remap-path-prefix of the"
  "cargo home, the toolchain's library sources and the tree): a build without them gives other bytes")
if [ -n "$server_rustc" ]; then
  {
    echo "(hbbs and hbbr were built in that tree with"
    echo "cargo build --release --locked --bin hbbs --bin hbbr, $server_rustc and"
    printf '%s\n' "${remapped[0]}" "${remapped[1]})"
  } >> "$stage/SOURCE.txt"
fi
if [ -n "$api" ]; then
  how=("(server/api holds the whole source of hjv-api; it was built there with")
  if [ "$api_commit" = "$(git -C "$ROOT" rev-parse HEAD)" ]; then
    how+=("cargo build --release --locked --bin hjv-api and HJV_API_COMMIT set to the commit above")
  elif git -C "$ROOT" diff --quiet "$api_commit" HEAD -- server/api 2> /dev/null; then
    how+=("cargo build --release --locked --bin hjv-api and HJV_API_COMMIT set to"
      "$api_commit, whose server/api is that of the commit above")
  else
    how+=("cargo build --release --locked --bin hjv-api and HJV_API_COMMIT set to" "$api_commit")
  fi
  if [ -n "$api_rustc" ]; then
    how[-1]+=","
    how+=("$api_rustc and" "${remapped[@]}")
  fi
  how[-1]+=")"
  printf '%s\n' "${how[@]}" >> "$stage/SOURCE.txt"
fi

mkdir -p "$out"
tar -C "$work" --owner=0 --group=0 --numeric-owner --sort=name -czf "$out/$name.tar.gz" "$name"
(cd "$out" && sha256sum "$name.tar.gz" > SHA256SUMS && cat SHA256SUMS)
tar -tzvf "$out/$name.tar.gz" | awk '{print $1, $3, $6}'

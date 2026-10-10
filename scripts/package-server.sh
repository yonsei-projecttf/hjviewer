#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Packages hbbs/hbbr for servers that run them as systemd services, without Docker
# (docs/server-runbook.md, path B), as a GitHub release asset:
#   scripts/package-server.sh <version> [<out dir>]
# writes <out dir>/hjviewer-server-<version>-linux-x86_64.tar.gz and <out dir>/SHA256SUMS. The
# archive holds bin/hbbs and bin/hbbr (built on Ubuntu 24.04 from a fresh upstream checkout with
# server/patches, so they need glibc 2.39 or later), systemd/ (the units and hjviewer.env.example),
# web/ (the nginx and Apache files of deploy/), and LICENSE, NOTICE, SOURCE and THIRD_PARTY_NOTICES.
#
# Binaries to hand out must be of a commit that is in the public repository, since their notices
# point there: the script stops on uncommitted changes or a commit that origin/main does not
# contain. HJV_SKIP_SOURCE_CHECK=1 skips that for local tests, and HJV_SERVER_BIN_DIR=<dir> packages
# binaries built already (scripts/ci.sh server leaves them in build/ci/target/release).
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

stage="$work/$name"
rm -rf "$stage"
mkdir -p "$stage/bin" "$stage/systemd" "$stage/web"
install -m 755 "$bin/hbbs" "$bin/hbbr" "$stage/bin/"
install -m 644 "$ROOT"/deploy/systemd/*.service "$ROOT/deploy/systemd/hjviewer.env.example" "$stage/systemd/"
install -m 644 "$ROOT/deploy/nginx-hjviewer-locations.conf" "$ROOT/deploy/nginx-hjviewer.conf" \
  "$ROOT/deploy/apache-hjviewer.conf" "$stage/web/"
NOTICES_COMPONENT=server NOTICES_TARGET=x86_64-unknown-linux-gnu \
  "$ROOT/scripts/package-notices.sh" "$stage" "$work/upstream/rustdesk-server"

mkdir -p "$out"
tar -C "$work" --owner=0 --group=0 --numeric-owner --sort=name -czf "$out/$name.tar.gz" "$name"
(cd "$out" && sha256sum "$name.tar.gz" > SHA256SUMS && cat SHA256SUMS)
tar -tzvf "$out/$name.tar.gz" | awk '{print $1, $3, $6}'

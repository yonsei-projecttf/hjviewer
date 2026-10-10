#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Checks out the pinned upstream sources under upstream/ and applies hjviewer's patches on a
# `hjviewer` branch. Usage: scripts/fetch-upstream.sh [server|client|all]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT/scripts/upstream.env"
UPSTREAM="${UPSTREAM_DIR:-$ROOT/upstream}"
mkdir -p "$UPSTREAM"

git_am() {
  # patches carry their own authorship; the committer identity only has to exist
  git -c user.name="${GIT_COMMITTER_NAME:-hjviewer}" -c user.email="${GIT_COMMITTER_EMAIL:-build@hjviewer.invalid}" \
    am --quiet --keep-cr --3way "$@"
}

checkout() { # <repo> <commit> <dir>
  local repo=$1 commit=$2 dir=$3
  if [ -d "$dir/.git" ]; then
    echo "$dir exists; remove it to fetch again" >&2
    return 1
  fi
  git init -q "$dir"
  git -C "$dir" remote add origin "$repo"
  git -C "$dir" fetch -q --depth 1 origin "$commit"
  git -C "$dir" checkout -q -b hjviewer FETCH_HEAD
}

apply_patches() { # <dir> <patch dir>
  local dir=$1 patches=$2
  shopt -s nullglob
  local files=("$patches"/*.patch)
  shopt -u nullglob
  if [ ${#files[@]} -gt 0 ]; then
    (cd "$dir" && git_am "${files[@]}")
  fi
  echo "applied ${#files[@]} patch(es) from ${patches#$ROOT/} to ${dir#$ROOT/}"
}

fetch_server() {
  local dir="$UPSTREAM/rustdesk-server"
  checkout "$RUSTDESK_SERVER_REPO" "$RUSTDESK_SERVER_COMMIT" "$dir"
  git -C "$dir" submodule update --init --depth 1 -q
  apply_patches "$dir" "$ROOT/server/patches"
  # the first patch moves the hbb_common pin
  git -C "$dir" submodule update --init -q
}

fetch_client() {
  local dir="$UPSTREAM/rustdesk"
  checkout "$RUSTDESK_REPO" "$RUSTDESK_COMMIT" "$dir"
  git -C "$dir" submodule update --init --depth 1 -q
  (cd "$dir/libs/hbb_common" && git checkout -q -b hjviewer)
  apply_patches "$dir/libs/hbb_common" "$ROOT/client/patches/hbb_common"
  apply_patches "$dir" "$ROOT/client/patches/rustdesk"
  # Binary assets (icons) are plain files over the tree, not patches: a binary patch would also
  # carry the upstream file it replaces. They stay uncommitted in the working copy.
  if [ -d "$ROOT/client/overlay/rustdesk" ]; then
    cp -R "$ROOT/client/overlay/rustdesk/." "$dir/"
    echo "copied $(find "$ROOT/client/overlay/rustdesk" -type f | wc -l) overlay file(s) to ${dir#$ROOT/}"
  fi
}

case "${1:-all}" in
  server) fetch_server ;;
  client) fetch_client ;;
  all) fetch_server; fetch_client ;;
  *) echo "usage: $0 [server|client|all]" >&2; exit 2 ;;
esac

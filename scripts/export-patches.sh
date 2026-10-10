#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Regenerates the patch files from the commits on the `hjviewer` branches under upstream/.
# Commit your changes there first. Usage: scripts/export-patches.sh [server|client|all]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT/scripts/upstream.env"

export_series() { # <dir> <base commit> <patch dir> [-- <pathspec>...]
  local dir=$1 base=$2 out=$3
  shift 3
  mkdir -p "$out"
  rm -f "$out"/*.patch
  git -C "$dir" format-patch --quiet --zero-commit --no-signature "$base..hjviewer" -o "$out" "$@"
  echo "exported $(ls "$out"/*.patch 2>/dev/null | wc -l) patch(es) to ${out#$ROOT/}"
}

export_server() {
  export_series "$ROOT/upstream/rustdesk-server" "$RUSTDESK_SERVER_COMMIT" "$ROOT/server/patches"
}

export_client() {
  local dir="$ROOT/upstream/rustdesk"
  local hbb_base
  hbb_base=$(git -C "$dir" ls-tree "$RUSTDESK_COMMIT" libs/hbb_common | awk '{print $3}')
  export_series "$dir/libs/hbb_common" "$hbb_base" "$ROOT/client/patches/hbb_common"
  # the submodule's own changes are the hbb_common series; its pointer here would name a commit that
  # exists only in this working copy
  export_series "$dir" "$RUSTDESK_COMMIT" "$ROOT/client/patches/rustdesk" -- . ':(exclude)libs/hbb_common'
  if grep -l -E '^(index [0-9a-f]+\.\.[0-9a-f]+ 160000|\+Subproject commit )' "$ROOT"/client/patches/rustdesk/*.patch; then
    echo "error: the patches above move the libs/hbb_common submodule" >&2
    exit 1
  fi
  # overlay files must not end up in a patch (see fetch-upstream.sh)
  local overlay="$ROOT/client/overlay/rustdesk" leaked=0 f
  if [ -d "$overlay" ]; then
    while IFS= read -r f; do
      if grep -lF "diff --git a/${f#$overlay/} " "$ROOT"/client/patches/rustdesk/*.patch; then leaked=1; fi
    done < <(find "$overlay" -type f)
  fi
  if [ "$leaked" = 1 ]; then
    echo "error: the patches above change overlay files; keep those changes out of the commits" >&2
    exit 1
  fi
}

case "${1:-all}" in
  server) export_server ;;
  client) export_client ;;
  all) export_server; export_client ;;
  *) echo "usage: $0 [server|client|all]" >&2; exit 2 ;;
esac

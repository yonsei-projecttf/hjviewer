#!/usr/bin/env bash
# Regenerates the patch files from the commits on the `hjviewer` branches under upstream/.
# Commit your changes there first. Usage: scripts/export-patches.sh [server|client|all]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT/scripts/upstream.env"

export_series() { # <dir> <base commit> <patch dir>
  local dir=$1 base=$2 out=$3
  mkdir -p "$out"
  rm -f "$out"/*.patch
  git -C "$dir" format-patch --quiet --zero-commit --no-signature "$base..hjviewer" -o "$out"
  echo "exported $(ls "$out" | wc -l) patch(es) to ${out#$ROOT/}"
}

export_server() {
  export_series "$ROOT/upstream/rustdesk-server" "$RUSTDESK_SERVER_COMMIT" "$ROOT/server/patches"
}

export_client() {
  local dir="$ROOT/upstream/rustdesk"
  local hbb_base
  hbb_base=$(git -C "$dir" ls-tree "$RUSTDESK_COMMIT" libs/hbb_common | awk '{print $3}')
  export_series "$dir/libs/hbb_common" "$hbb_base" "$ROOT/client/patches/hbb_common"
  export_series "$dir" "$RUSTDESK_COMMIT" "$ROOT/client/patches/rustdesk"
}

case "${1:-all}" in
  server) export_server ;;
  client) export_client ;;
  all) export_server; export_client ;;
  *) echo "usage: $0 [server|client|all]" >&2; exit 2 ;;
esac

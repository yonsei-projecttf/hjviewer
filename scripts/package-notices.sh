#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Writes the notices that ship next to a client build, into <out dir>:
#   LICENSE.txt              the GNU AGPL-3.0
#   NOTICE.txt               upstream projects, copyright holders and what HJViewer changed
#   SOURCE.txt               where the Corresponding Source of this exact build is
#   THIRD_PARTY_NOTICES.txt  every Rust crate in the dependency graph (name, version, license,
#                            source), the copyright files of the vcpkg ports under $VCPKG_ROOT,
#                            and the extra license files given as <file> <title> pairs
#   scripts/package-notices.sh <out dir> <rust workspace dir> [<license file> <title>]...
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
out=$1
ws=$2
shift 2
mkdir -p "$out"
cp "$ROOT/LICENSE" "$out/LICENSE.txt"
cp "$ROOT/NOTICE" "$out/NOTICE.txt"

server=${GITHUB_SERVER_URL:-https://github.com}
repo=${GITHUB_REPOSITORY:-ysfaculty/hjviewer}
commit=${GITHUB_SHA:-$(git -C "$ROOT" rev-parse HEAD)}
{
  echo "HJViewer is free software under the GNU Affero General Public License v3.0 (LICENSE.txt)"
  echo "and comes with ABSOLUTELY NO WARRANTY."
  echo
  echo "Corresponding Source of this build: $server/$repo/tree/$commit"
  echo "(scripts/upstream.env pins the upstream RustDesk commits; client/patches and client/overlay"
  echo "hold every change; scripts/fetch-upstream.sh client reproduces the tree that was built)"
  if [ -n "${GITHUB_RUN_ID:-}" ]; then
    echo "Built by: $server/$repo/actions/runs/$GITHUB_RUN_ID"
  fi
} >"$out/SOURCE.txt"

{
  echo "HJViewer third-party notices"
  echo
  echo "HJViewer includes the components below, each under its own license. The full license texts of"
  echo "the Rust crates are in their sources at the listed versions."
  echo
  echo "== Rust crates =="
  cargo metadata --format-version 1 --locked --manifest-path "$ws/Cargo.toml" |
    jq -r '.workspace_members as $own | .packages[] | select(.id as $id | $own | index($id) | not)
      | "\(.name) \(.version) | \(.license // "see its source") | \(.repository // .source // "")"' |
    sort -u
  shopt -s nullglob
  for f in "${VCPKG_ROOT:-/nonexistent}"/installed/*/share/*/copyright; do
    printf '\n== %s (vcpkg port) ==\n' "$(basename "$(dirname "$f")")"
    cat "$f"
  done
  while [ $# -ge 2 ]; do
    if [ -f "$1" ]; then
      printf '\n== %s ==\n' "$2"
      cat "$1"
    else
      echo "warning: $1 not found, '$2' is missing from the notices" >&2
    fi
    shift 2
  done
} >"$out/THIRD_PARTY_NOTICES.txt"
echo "wrote LICENSE.txt NOTICE.txt SOURCE.txt THIRD_PARTY_NOTICES.txt to $out"

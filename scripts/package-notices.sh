#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Writes the notices that ship next to a client build (or, with NOTICES_COMPONENT=server, next to
# the hbbs/hbbr binaries of scripts/package-server.sh), into <out dir>:
#   LICENSE.txt              the GNU AGPL-3.0
#   NOTICE.txt               upstream projects, copyright holders and what HJViewer changes
#   SOURCE.txt               where the Corresponding Source of this exact build is
#   THIRD_PARTY_NOTICES.txt  the Rust crates of this build with their license files, the copyright
#                            files of the vcpkg ports under $VCPKG_ROOT, and the extra license files
#                            given as <file> <title> pairs (a title starting with "?" is optional)
#
#   scripts/package-notices.sh <out dir> <rust workspace dir> [<license file> <title>]...
#
# NOTICES_FEATURES and NOTICES_TARGET name the build's cargo features and target triple, so the
# crate list is the one that build links. The source location is always the public repository
# (HJV_SOURCE_REPO): builds made elsewhere, e.g. by the private build repository, must be of a
# commit that is public there, which is checked in CI (set HJV_SKIP_SOURCE_CHECK=1 for local test
# builds). The commit is the checkout's HEAD: in a run the private repository starts, GITHUB_SHA
# names that repository's commit, not the source.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
out=$1
ws=$2
shift 2
mkdir -p "$out"
cp "$ROOT/LICENSE" "$out/LICENSE.txt"
cp "$ROOT/NOTICE" "$out/NOTICE.txt"

repo=${HJV_SOURCE_REPO:-yonsei-projecttf/hjviewer}
commit=$(git -C "$ROOT" rev-parse HEAD)
if [ -z "${HJV_SKIP_SOURCE_CHECK:-}" ] && [ -n "${GITHUB_ACTIONS:-}" ]; then
  probe=$(mktemp -d)
  if ! git -C "$probe" init -q || ! git -C "$probe" fetch -q --depth 1 "https://github.com/$repo" "$commit"; then
    echo "::error::$commit is not in the public repository $repo; push it there before building apps to hand out"
    exit 1
  fi
  rm -rf "$probe"
fi
{
  echo "HJViewer is free software under the GNU Affero General Public License v3.0 (LICENSE.txt)"
  echo "and comes with ABSOLUTELY NO WARRANTY."
  echo
  echo "Corresponding Source of this build: https://github.com/$repo/tree/$commit"
  if [ "${NOTICES_COMPONENT:-client}" = server ]; then
    echo "(scripts/upstream.env pins the upstream rustdesk-server commit; server/patches holds every"
    echo "change; scripts/fetch-upstream.sh server reproduces the tree that was built)"
  else
    echo "(scripts/upstream.env pins the upstream RustDesk commits; client/patches and client/overlay"
    echo "hold every change; scripts/fetch-upstream.sh client reproduces the tree that was built)"
  fi
  if [ "${GITHUB_REPOSITORY:-}" = "$repo" ] && [ -n "${GITHUB_RUN_ID:-}" ]; then
    echo "Built by: https://github.com/$repo/actions/runs/$GITHUB_RUN_ID"
  fi
} >"$out/SOURCE.txt"

notices="$out/THIRD_PARTY_NOTICES.txt"
missing=0
{
  echo "HJViewer third-party notices"
  echo
  echo "HJViewer includes the components below, each under its own license, reproduced here."
  echo "Build: features '${NOTICES_FEATURES:-default}', target '${NOTICES_TARGET:-host}'."
  echo
  echo "== Rust crates =="
  # every crate of the build except the AGPL workspace ones, with the license files in its source
  cargo metadata --format-version 1 --locked --manifest-path "$ws/Cargo.toml" \
    ${NOTICES_FEATURES:+--features "$NOTICES_FEATURES"} \
    ${NOTICES_TARGET:+--filter-platform "$NOTICES_TARGET"} |
    jq -r '(.workspace_members) as $own | (.resolve.nodes | map(.id)) as $used | .packages[]
      | select(.id as $id | $used | index($id))
      | select((.id as $id | $own | index($id) | not) or ((.license // "AGPL") | test("AGPL") | not))
      | "\(.name) \(.version)\t\(.license // "see its license files")\t\(.manifest_path)"' |
    tr -d '\r' | sort -u >"$out/.crates"
  awk -F'\t' '{ print $1 " | " $2 }' "$out/.crates"
  while IFS=$'\t' read -r crate license manifest; do
    dir=$(dirname "${manifest//\\//}")
    shopt -s nullglob nocaseglob
    found=0
    for f in "$dir"/LICEN[CS]E* "$dir"/COPYING* "$dir"/NOTICE* "$dir"/UNLICENSE*; do
      [ -f "$f" ] || continue
      printf '\n== %s: %s ==\n' "$crate" "$(basename "$f")"
      cat "$f"
      found=1
    done
    for f in "$dir"/LICENSES/*; do
      [ -f "$f" ] || continue
      printf '\n== %s: LICENSES/%s ==\n' "$crate" "$(basename "$f")"
      cat "$f"
      found=1
    done
    shopt -u nullglob nocaseglob
    if [ "$found" = 0 ]; then
      echo "$crate" >>"$out/.unlicensed"
    fi
  done <"$out/.crates"
  rm -f "$out/.crates"
  if [ -s "$out/.unlicensed" ]; then
    # the crate ships no license file: its license is the SPDX expression listed above
    printf '\n== Crates without a license file in their source (license as listed above) ==\n'
    cat "$out/.unlicensed"
  fi
  rm -f "$out/.unlicensed"
  shopt -s nullglob
  for f in "${VCPKG_ROOT:-/nonexistent}"/installed/*/share/*/copyright; do
    printf '\n== %s (vcpkg port) ==\n' "$(basename "$(dirname "$f")")"
    cat "$f"
  done
  shopt -u nullglob
  while [ $# -ge 2 ]; do
    file=$1 title=${2#\?}
    if [ -f "$file" ]; then
      printf '\n== %s ==\n' "$title"
      cat "$file"
    elif [ "${2:0:1}" = "?" ]; then
      echo "warning: $file not found, '$title' is not in the notices" >&2
    else
      echo "::error::$file not found, '$title' would be missing from the notices" >&2
      missing=1
    fi
    shift 2
  done
} >"$notices"
if [ "$missing" = 1 ]; then
  exit 1
fi
echo "wrote LICENSE.txt NOTICE.txt SOURCE.txt THIRD_PARTY_NOTICES.txt ($(wc -c <"$notices") bytes) to $out"

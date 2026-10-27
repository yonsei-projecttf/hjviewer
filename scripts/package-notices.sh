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
# crate list is the one that build links. NOTICES_EXTRA_WORKSPACES names more Rust workspaces
# whose binaries ship in the same package (space-separated directories, default features, the
# same target): the server package's hjv-api (server/api). The source location is always the public repository
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
    echo "hold every change; scripts/fetch-upstream.sh client, then scripts/set-client-version.sh"
    echo "and the steps of .github/workflows/client-build.yml, reproduce the tree that was built)"
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
  if [ -n "${NOTICES_EXTRA_WORKSPACES:-}" ]; then
    echo "Also the crates of: $(for w in $NOTICES_EXTRA_WORKSPACES; do printf '%s ' "${w#"$ROOT"/}"; done)"
  fi
  echo
  echo "== Rust crates =="
  # every crate of the build except the AGPL workspace ones, with the license files in its source
  crates='(.workspace_members) as $own | (.resolve.nodes | map(.id)) as $used | .packages[]
      | select(.id as $id | $used | index($id))
      | select((.id as $id | $own | index($id) | not) or ((.license // "AGPL") | test("AGPL") | not))
      | "\(.name) \(.version)\t\(.license // "see its license files")\t\(.manifest_path)"'
  {
    cargo metadata --format-version 1 --locked --manifest-path "$ws/Cargo.toml" \
      ${NOTICES_FEATURES:+--features "$NOTICES_FEATURES"} \
      ${NOTICES_TARGET:+--filter-platform "$NOTICES_TARGET"} | jq -r "$crates"
    for extra in ${NOTICES_EXTRA_WORKSPACES:-}; do
      cargo metadata --format-version 1 --locked --manifest-path "$extra/Cargo.toml" \
        ${NOTICES_TARGET:+--filter-platform "$NOTICES_TARGET"} | jq -r "$crates"
    done
  } | tr -d '\r' | sort -u >"$out/.crates"
  awk -F'\t' '{ print $1 " | " $2 }' "$out/.crates"
  # The loop finds each crate's license files with builtins only, and one python3 writes them all,
  # each after its header, byte for byte as cat would: a dirname, a basename and a cat for each of
  # some 950 files took up to 2.7 minutes in Git Bash on Windows (GHA-07 of the 2026-10-27 CI
  # audit). The headers and the paths go to it as NUL-separated pairs. cargo metadata gives
  # absolute manifest paths, so the crate's directory is the path up to its last slash.
  parts=()
  while IFS=$'\t' read -r crate license manifest; do
    dir=${manifest//\\//}
    dir=${dir%/*}
    shopt -s nullglob nocaseglob
    found=0
    for f in "$dir"/LICEN[CS]E* "$dir"/COPYING* "$dir"/NOTICE* "$dir"/UNLICENSE*; do
      [ -f "$f" ] || continue
      printf -v header '\n== %s: %s ==\n' "$crate" "${f##*/}"
      parts+=("$header" "$f")
      found=1
    done
    for f in "$dir"/LICENSES/*; do
      [ -f "$f" ] || continue
      printf -v header '\n== %s: LICENSES/%s ==\n' "$crate" "${f##*/}"
      parts+=("$header" "$f")
      found=1
    done
    shopt -u nullglob nocaseglob
    if [ "$found" = 0 ]; then
      echo "$crate" >>"$out/.unlicensed"
    fi
  done <"$out/.crates"
  rm -f "$out/.crates"
  if [ ${#parts[@]} -gt 0 ]; then
    printf '%s\0' "${parts[@]}" | python3 -I -c '
import sys
parts = sys.stdin.buffer.read().split(b"\0")[:-1]
out = sys.stdout.buffer
for header, path in zip(parts[0::2], parts[1::2]):
    out.write(header)
    with open(path, "rb") as f:
        out.write(f.read())
out.flush()'
  fi
  if [ -s "$out/.unlicensed" ]; then
    # the crate ships no license file: its license is the SPDX expression listed above
    printf '\n== Crates without a license file in their source (license as listed above) ==\n'
    cat "$out/.unlicensed"
  fi
  rm -f "$out/.unlicensed"
  shopt -s nullglob
  for f in "${VCPKG_ROOT:-/nonexistent}"/installed/*/share/*/copyright; do
    port=${f%/copyright}
    printf '\n== %s (vcpkg port) ==\n' "${port##*/}"
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

#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# The owner's hjv-release, which signs the version policies (docs/server-runbook.md 8-2,
# tools/hjv-release/README.md), built from an export of a commit: not from a working tree, whose
# changes that no commit has would reach the signer, and not taken from the CI directory, whose
# build/ci/target-hjv-release is the last tree's to run scripts/ci.sh there (its stages' tests use it).
#   scripts/build-signer.sh [<commit>]   origin/main, after a fetch, when no commit is named
# The output folder is build/signer of the main checkout (the folder that holds the repository's
# common git directory), whichever worktree runs this, so that the owner finds the signer in one
# place: build/signer/hjv-release, and beside it hjv-release.record, which says what it is built from
# (the commit and its tree, the toolchain, the cargo settings from outside the commit, the binary's
# SHA-256 and when it was built) and which the script prints at its end. HJV_SIGNER_DIR=<folder>
# builds into another folder, for a trial.
# The commit is baked in (HJV_BUILD_COMMIT): hjv-release version and sign, before it asks anything,
# say which commit's tool it is. The script stops on a commit it cannot resolve or an export that is
# not the commit's files, signs nothing and touches no key.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
export PATH="$HOME/.cargo/bin:$PATH"
fail() {
  echo "build-signer: $*" >&2
  exit 1
}
main="$(dirname "$(git -C "$ROOT" rev-parse --path-format=absolute --git-common-dir)")"
out=${HJV_SIGNER_DIR:-$main/build/signer}
# absolute: cargo runs inside the export
case $out in /*) ;; *) out=$PWD/$out ;; esac
# Nothing of the caller's environment changes what cargo builds: the compiler, its wrappers and
# flags, the toolchain, and cargo's build, target and profile settings are unset here (the output
# names them)
dropped=()
for name in $(compgen -e); do
  case $name in
    RUSTC | RUSTC_WRAPPER | RUSTC_WORKSPACE_WRAPPER | RUSTC_BOOTSTRAP | RUSTFLAGS | CARGO_ENCODED_RUSTFLAGS | \
      RUSTUP_TOOLCHAIN | CARGO_INCREMENTAL | CARGO_BUILD_* | CARGO_TARGET_* | CARGO_PROFILE_* | CARGO_HOST_* | \
      CARGO_UNSTABLE_*)
      dropped+=("$name")
      unset "$name"
      ;;
  esac
done
ref=${1:-}
if [ -z "$ref" ]; then
  git -C "$ROOT" fetch -q origin main || fail "git fetch origin main"
  ref=origin/main
fi
commit=$(git -C "$ROOT" rev-parse -q --verify "$ref^{commit}") || fail "no commit $ref"
tree=$(git -C "$ROOT" rev-parse "$commit^{tree}")
git -C "$ROOT" cat-file -e "$commit:tools/hjv-release/Cargo.toml" 2> /dev/null || fail "$commit has no tools/hjv-release"
# A new folder in the owner's cache for each build, for the export and cargo's target both: cargo and
# rustup read settings from the folders above the build (.cargo/config.toml, rust-toolchain.toml),
# which anyone can make under /tmp, and a target kept from an earlier build would be taken as this
# one (the export's files are as old as the commit, so cargo sees nothing changed)
cache=${XDG_CACHE_HOME:-}
case $cache in /*) ;; *) cache=$HOME/.cache ;; esac
mkdir -p "$cache"
work=$(mktemp -d "$cache/hjv-signer.XXXXXX")
trap 'rm -rf "$work"' EXIT
echo "== the files of $commit ($ref)"
git -C "$ROOT" archive --format=tar "$commit" | tar -x -C "$work"
# The export holds the commit's files of tools/hjv-release, byte for byte, and no other there (an
# attribute such as export-subst or export-ignore, or a line ending conversion, would change them)
want=$(git -C "$ROOT" ls-tree -r "$commit" -- tools/hjv-release | awk -F'\t' '{ split($1, m, " "); print $2, m[3] }' |
  LC_ALL=C sort)
got=$(cd "$work" && find tools/hjv-release -type f -print | while IFS= read -r f; do
  echo "$f $(git hash-object --no-filters -- "$f")"
done | LC_ALL=C sort)
if [ -z "$want" ] || [ "$got" != "$want" ]; then fail "the export of tools/hjv-release is not the files of $commit"; fi
# The cargo settings that the build takes from outside the commit: .cargo/config and config.toml in
# the folders above the export, and CARGO_HOME's
config=$(
  d=$work
  {
    while [ "$d" != / ]; do
      d=$(dirname "$d")
      echo "${d%/}/.cargo"
    done
    echo "${CARGO_HOME:-$HOME/.cargo}"
  } | LC_ALL=C sort -u | while IFS= read -r d; do
    for f in "$d/config" "$d/config.toml"; do
      if [ -e "$f" ]; then echo "$f"; fi
    done
  done | paste -sd ' ' -
)
if [ ${#dropped[@]} -gt 0 ]; then echo "== left out of the build's environment: ${dropped[*]}"; fi
echo "== hjv-release of $commit, in $work"
(cd "$work/tools/hjv-release" && HJV_BUILD_COMMIT=$commit CARGO_TARGET_DIR="$work/target" cargo build --release --locked) ||
  fail "cargo build"
mkdir -p "$out"
install -m 755 "$work/target/release/hjv-release" "$out/hjv-release.new"
version=$("$out/hjv-release.new" version)
[ "$version" = "hjv-release $(sed -n 's/^version = "\(.*\)"$/\1/p' "$work/tools/hjv-release/Cargo.toml" | head -n 1) (commit $commit)" ] ||
  fail "the build says $version, not its commit $commit"
mv -f "$out/hjv-release.new" "$out/hjv-release"
{
  echo "commit $commit ($ref)"
  echo "tree $tree"
  echo "version $version"
  echo "toolchain $(rustc -V) / $(cargo -V)"
  echo "cargo config ${config:-none from outside the commit}"
  echo "sha256 $(sha256sum < "$out/hjv-release" | cut -d' ' -f1)"
  echo "built $(date '+%Y-%m-%d %H:%M:%S %z') from an export of the commit, by scripts/build-signer.sh"
} > "$out/hjv-release.record"
echo "== $out/hjv-release"
cat "$out/hjv-release.record"

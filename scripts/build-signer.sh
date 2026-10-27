#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# The owner's hjv-release, which signs the version policies with the signing key on the dev PC
# (docs/server-runbook.md 8-2, tools/hjv-release/README.md; the root key's ceremony builds its own at
# the clean computer, README 3장 5-0), built from an export of a commit of pushed main: not from a
# working tree, whose changes that no commit has would reach the signer, and not taken from the CI
# directory, whose build/ci/target-hjv-release is the last tree's to run scripts/ci.sh there (its
# stages' tests use it).
#   scripts/build-signer.sh [<commit>]   origin/main when no commit is named
# It first fetches main from origin (into refs/remotes/origin/main, which origin/main in a named
# commit, origin/main~1 say, stands for too), and a named commit must be on it.
# The output folder is build/signer of the main checkout (the folder that holds the repository's
# common git directory), whichever worktree runs this, so that the owner finds the signer in one
# place: build/signer/hjv-release, and beside it hjv-release.record, which says what it is built from
# (the commit and the origin/main it was checked against, its tree, the toolchain, the cargo settings
# from outside the commit, the binary's SHA-256 and when it was built) and which the script prints at
# its end. HJV_SIGNER_DIR=<folder> builds into another folder, for a trial, which also builds a commit
# that origin/main does not have and says so.
# The commit is baked in (HJV_BUILD_COMMIT): hjv-release version and sign, before it asks anything,
# say which commit's tool it is. The script stops on a commit it cannot resolve, a commit that is not
# on origin/main (outside a trial) or an export that is not the commit's files, and then says which
# commit's tool stays in the output folder. It signs nothing and touches no key.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
export PATH="$HOME/.cargo/bin:$PATH"
main="$(dirname "$(git -C "$ROOT" rev-parse --path-format=absolute --git-common-dir)")"
out=${HJV_SIGNER_DIR:-$main/build/signer}
# absolute: cargo runs inside the export
case $out in /*) ;; *) out=$PWD/$out ;; esac
# a trial: HJV_SIGNER_DIR names a folder other than build/signer
trial=
if [ -n "${HJV_SIGNER_DIR:-}" ]; then
  mkdir -p "$out"
  [ "$out" -ef "$main/build/signer" ] || trial=1
fi
# What the output folder holds: a run that stops leaves the tool that was there, and one that fails
# only after it has moved its own in (in printing the record) says so
replaced=
kept() {
  if [ ! -e "$out/hjv-release" ]; then
    echo "there is no $out/hjv-release"
  elif [ -n "$replaced" ]; then
    echo "$out/hjv-release is this run's: $(head -n 1 "$out/hjv-release.record")"
  elif [ -f "$out/hjv-release.record" ]; then
    echo "$out/hjv-release stays as it was: $(head -n 1 "$out/hjv-release.record")"
  else
    echo "$out/hjv-release stays as it was (no record beside it)"
  fi
}
work=
finish() {
  local status=$?
  # neither another signal (a second Ctrl-C) nor a step that fails cuts the clean-up and the message
  # short
  trap '' INT TERM HUP PIPE
  set +e
  # After a Ctrl-C during cargo build, rustc stops a moment later and a linker that it started then
  # runs on, writing in the build folder: a removal that fails is tried again, for up to 5 s
  if [ -n "$work" ]; then
    for _ in 1 2 3 4 5 6 7 8 9 10; do
      if rm -rf "$work" 2> /dev/null; then break; fi
      sleep 0.5
    done
    if [ -e "$work" ]; then echo "build-signer: $work stays: it could not be removed" >&2; fi
  fi
  rm -f "$out/hjv-release.new" "$out/hjv-release.record.new"
  if [ "$status" != 0 ]; then echo "build-signer: $(kept)" >&2; fi
}
trap finish EXIT
# a stop by a signal is a failure too (the exit trap sees the status), PIPE's too: a reader of the
# output that went first (| head) would else end the script with no word of the tool that stays
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP
trap 'exit 141' PIPE
fail() {
  echo "build-signer: $*" >&2
  exit 1
}
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
# main as origin has it now, by its full name (a branch or a tag named origin/main comes first for
# the short one), and the commit, which must be on it
git -C "$ROOT" fetch -q origin +refs/heads/main:refs/remotes/origin/main || fail "git fetch origin main"
origin_main=$(git -C "$ROOT" rev-parse -q --verify 'refs/remotes/origin/main^{commit}') || fail "no origin/main after the fetch"
ref=${1:-origin/main}
commit=$origin_main
if [ -n "${1:-}" ]; then
  # origin/main in a named commit (origin/main itself, origin/main~1) is that full name too
  rev=$1
  case $rev in origin/main | origin/main[~^@]*) rev=refs/remotes/$rev ;; esac
  commit=$(git -C "$ROOT" rev-parse -q --verify "$rev^{commit}") || fail "no commit $1"
fi
if git -C "$ROOT" merge-base --is-ancestor "$commit" "$origin_main"; then
  checked="origin/main $origin_main, which has the commit"
elif [ -n "$trial" ]; then
  checked="origin/main $origin_main, which does not have the commit: a trial (HJV_SIGNER_DIR), not a tool to sign with"
  echo "== $commit is not on origin/main $origin_main: a trial build (HJV_SIGNER_DIR), not a tool to sign with"
else
  fail "$commit ($ref) is not on origin/main $origin_main: the signer is built from pushed main" \
    "(HJV_SIGNER_DIR=<folder> builds it elsewhere, for a trial)"
fi
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
# What else the build takes: the toolchain that rustup picks there, and the cargo settings from outside
# the commit (.cargo/config and config.toml in the folders above the export, and CARGO_HOME's)
toolchain=$(cd "$work/tools/hjv-release" && echo "$(rustc -V) / $(cargo -V)")
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
version=$("$out/hjv-release.new" version) || fail "the build does not run: hjv-release version"
case $version in
  "hjv-release "*" (commit $commit)") ;;
  *"(commit "*) fail "the build says \"$version\", not that it is commit $commit's" ;;
  *) fail "the build says \"$version\", not that it is commit $commit's (a commit before 2b5d294 bakes in none)" ;;
esac
sha=$(sha256sum < "$out/hjv-release.new" | cut -d' ' -f1)
# The record is written beside the tool first, and moved in after it
{
  echo "commit $commit ($ref)"
  echo "$checked"
  echo "tree $tree"
  echo "version $version"
  echo "toolchain $toolchain"
  echo "cargo config ${config:-none from outside the commit}"
  echo "sha256 $sha"
  echo "built $(date '+%Y-%m-%d %H:%M:%S %z') from an export of the commit, by scripts/build-signer.sh"
} > "$out/hjv-release.record.new"
# From here on INT, TERM, HUP and PIPE are ignored: a stop between the two moves would leave the new
# tool beside the old record, which the exit trap would then name as the tool that stays, and what is
# left only prints the record
trap '' INT TERM HUP PIPE
mv -f "$out/hjv-release.new" "$out/hjv-release"
mv -f "$out/hjv-release.record.new" "$out/hjv-release.record"
replaced=1
echo "== $out/hjv-release"
cat "$out/hjv-release.record"

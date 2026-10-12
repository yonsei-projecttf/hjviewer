#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Packages hjv-relay, the Wake-on-LAN relay for a Synology NAS (docs/relay-dsm.md), as a release
# asset:
#   HJV_SERVER=<server> HJV_COMMAND_PUBKEY=<key id>:<base64 key>[,...] scripts/package-relay.sh [<out dir>]
# writes <out dir>/hjviewer-relay-<version>-linux-x86_64.tar.gz (default out dir build/release-relay,
# apart from the server package's build/release) and <out dir>/SHA256SUMS with this archive's line
# only, so that each package's checksum file names its own files alone (design v3 8-6).
# <version> is relay/Cargo.toml's. The archive holds hjv-relay,
# built for x86_64-unknown-linux-musl and statically linked so that it runs on any x86_64 DSM 7
# NAS; README-DSM.md (docs/relay-dsm.md); and LICENSE, NOTICE, SOURCE and THIRD_PARTY_NOTICES
# (scripts/package-notices.sh). Tag: relay-<version>.
#
# The relay is built for one server: HJV_SERVER and HJV_COMMAND_PUBKEY (relay/PROTOCOL.md,
# section 3.2) are compiled in, like the apps' HJV_SERVER, so the archive names the server and
# belongs with the private build repository's releases, not the public repository's. The script
# refuses a placeholder or test server name (localhost, *.example.com, *.test, an IP address, ...)
# and the public test keys of this repository, whose private keys are published, unless
# HJV_RELAY_TEST_BUILD=1 says that the build is a test build.
# HJV_RELAY_UNCONFIGURED=1 packages a build without them, which refuses 'run' (for tests).
#
# Binaries to hand out must be of a commit that is in the public repository, since their notices
# point there: the script stops on uncommitted changes or a commit that origin/main does not
# contain. HJV_SKIP_SOURCE_CHECK=1 skips that for local tests. A build with HJV_SKIP_SOURCE_CHECK
# or HJV_RELAY_TEST_BUILD is a test build: its archive is named ...-linux-x86_64-test.tar.gz and
# its SOURCE.txt says so in its first line.
# Needs the x86_64-unknown-linux-musl Rust target (rustup target add x86_64-unknown-linux-musl),
# musl-gcc for ring's C code (Ubuntu: musl-tools), file, jq, python3, sha256sum and gzip.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
out=${1:-$ROOT/build/release-relay}
target=x86_64-unknown-linux-musl
work="$ROOT/build/package-relay"
export PATH="$HOME/.cargo/bin:$PATH"

# Whether <server> (host or host:port) is a placeholder, a reserved test name or an IP address.
placeholder_server() {
  local host
  host=$(printf '%s' "$1" | tr -d '[:space:]' | tr '[:upper:]' '[:lower:]')
  host=${host%:*}
  host=${host%.}
  case "$host" in
    "" | localhost | *.localhost | example.com | *.example.com | example.net | *.example.net | \
      example.org | *.example.org | *.example | *.test | *.invalid | *.local)
      return 0
      ;;
  esac
  [[ $host =~ ^[0-9.]+$ ]]
}

# Whether <keys> (<key id>:<base64 key>[,...]) is no list a relay to hand out may trust, as
# scripts/check-build-keys.py --relay judges it in the release builds: malformed, a weak key, or a
# public test key, whose private key anyone has (seeds of one repeated byte or of 32 consecutive
# bytes such as the tests' and the E2E's, the RFC examples, scripts/public-test-keys.txt, and every
# key whose private key a tracked file of this repository writes out). It says what it refuses.
has_test_key() {
  ! HJV_COMMAND_PUBKEY=$1 CALLED=package-relay.sh CHECK_ONLY='' GITHUB_STEP_SUMMARY='' \
    python3 -I "$ROOT/scripts/check-build-keys.py" --relay >&2
}

version=$(sed -n 's/^version = "\([^"]*\)"$/\1/p' "$ROOT/relay/Cargo.toml" | head -n 1)
if [ -z "$version" ]; then
  echo "no version in relay/Cargo.toml" >&2
  exit 1
fi
test_build=
if [ -n "${HJV_SKIP_SOURCE_CHECK:-}" ] || [ -n "${HJV_RELAY_TEST_BUILD:-}" ]; then
  test_build=1
  echo "warning: a test build (HJV_SKIP_SOURCE_CHECK or HJV_RELAY_TEST_BUILD): not to be handed out" >&2
fi
name="hjviewer-relay-$version-linux-x86_64${test_build:+-test}"

if [ -z "${HJV_SKIP_SOURCE_CHECK:-}" ]; then
  if [ -n "$(git -C "$ROOT" status --porcelain)" ]; then
    echo "uncommitted changes: commit and push them first" >&2
    exit 1
  fi
  git -C "$ROOT" fetch -q origin main
  if ! git -C "$ROOT" merge-base --is-ancestor HEAD origin/main &&
    [ "$(git -C "$ROOT" rev-parse --is-shallow-repository)" = true ]; then
    # a shallow CI checkout may lack the history that connects HEAD to origin/main
    git -C "$ROOT" fetch -q --unshallow origin main
  fi
  if ! git -C "$ROOT" merge-base --is-ancestor HEAD origin/main; then
    echo "$(git -C "$ROOT" rev-parse --short HEAD) is not in origin/main: push it first" >&2
    exit 1
  fi
fi

if [ -n "${HJV_RELAY_UNCONFIGURED:-}" ]; then
  unset HJV_SERVER HJV_COMMAND_PUBKEY
elif [ -z "${HJV_SERVER:-}" ] || [ -z "${HJV_COMMAND_PUBKEY:-}" ]; then
  echo "set HJV_SERVER and HJV_COMMAND_PUBKEY (relay/PROTOCOL.md, section 3.2): a relay without them cannot run" >&2
  echo "(HJV_RELAY_UNCONFIGURED=1 packages such a build anyway, for tests)" >&2
  exit 1
elif [ -z "${HJV_RELAY_TEST_BUILD:-}" ]; then
  if placeholder_server "$HJV_SERVER"; then
    echo "HJV_SERVER=$HJV_SERVER is a placeholder, test name or address, not the server (HJV_RELAY_TEST_BUILD=1 for a test build)" >&2
    exit 1
  fi
  if has_test_key "$HJV_COMMAND_PUBKEY"; then
    echo "HJV_COMMAND_PUBKEY is malformed or has a public test key, whose private key anyone has (see above; HJV_RELAY_TEST_BUILD=1 for a test build)" >&2
    exit 1
  fi
fi

if ! rustup target list --installed 2>/dev/null | grep -qx "$target"; then
  echo "the $target target is missing: rustup target add $target" >&2
  exit 1
fi
if [ -z "${CC_x86_64_unknown_linux_musl:-}" ]; then
  if ! command -v musl-gcc > /dev/null; then
    echo "musl-gcc is missing (Ubuntu: apt-get install musl-tools)" >&2
    exit 1
  fi
  export CC_x86_64_unknown_linux_musl=musl-gcc
fi

commit=$(git -C "$ROOT" rev-parse HEAD)
mkdir -p "$work"
# keep the build machine's paths out of the binary
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$ROOT=/hjviewer --remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo"
(cd "$ROOT/relay" &&
  HJV_BUILD_COMMIT=$commit CARGO_TARGET_DIR="$work/target" cargo build --release --locked --target "$target")
bin="$work/target/$target/release/hjv-relay"

# the binary needs nothing from the NAS: statically linked, and it runs without environment and,
# where this machine allows unprivileged namespaces, in an empty root directory
kind=$(file -b "$bin")
case "$kind" in
  *"statically linked"* | *"static-pie linked"*) ;;
  *)
    echo "hjv-relay is not statically linked: $kind" >&2
    exit 1
    ;;
esac
env -i "$bin" version > "$work/version.txt"
cat "$work/version.txt"
if unshare --user --map-root-user --mount true 2> /dev/null; then
  empty=$(mktemp -d "$work/empty.XXXXXX")
  cp "$bin" "$empty/hjv-relay"
  unshare --user --map-root-user --mount env -i "$(command -v chroot)" "$empty" /hjv-relay version > /dev/null
  rm -r -- "$empty"
  echo "hjv-relay runs in an empty root directory"
else
  echo "warning: no unprivileged namespaces here; skipped the empty-root check" >&2
fi
grep -qx "commit: $commit" "$work/version.txt"
if [ -z "${HJV_RELAY_UNCONFIGURED:-}" ] && grep -Eq '^(server|command keys): (none|invalid)' "$work/version.txt"; then
  echo "the build did not take HJV_SERVER or HJV_COMMAND_PUBKEY (see above)" >&2
  exit 1
fi
# what the binary has, checked again (it prints the keys the way HJV_COMMAND_PUBKEY gives them)
if [ -z "${HJV_RELAY_UNCONFIGURED:-}" ] && [ -z "${HJV_RELAY_TEST_BUILD:-}" ]; then
  built_server=$(sed -n 's/^server: //p' "$work/version.txt")
  built_keys=$(sed -n 's/^command keys: //p' "$work/version.txt")
  if placeholder_server "$built_server" || has_test_key "$built_keys"; then
    echo "the build has a placeholder server or a public test key (see above)" >&2
    exit 1
  fi
fi

stage="$work/$name"
rm -rf -- "$stage"
mkdir -p "$stage"
install -m 755 "$bin" "$stage/hjv-relay"
install -m 644 "$ROOT/docs/relay-dsm.md" "$stage/README-DSM.md"
NOTICES_COMPONENT=relay NOTICES_TARGET=$target "$ROOT/scripts/package-notices.sh" "$stage" "$ROOT/relay"
# package-notices.sh says how to rebuild the client or the server; say how to rebuild the relay
source_txt="$stage/SOURCE.txt"
if ! sed -n 4p "$source_txt" | grep -q '^Corresponding Source of this build: '; then
  echo "unexpected SOURCE.txt from package-notices.sh" >&2
  exit 1
fi
{
  if [ -n "${HJV_SKIP_SOURCE_CHECK:-}" ]; then
    echo "UNVERIFIED TEST BUILD, not built from a public commit: the link below may not hold its source."
  fi
  if [ -n "${HJV_RELAY_TEST_BUILD:-}" ]; then
    echo "TEST BUILD: it may trust a test server or a public test key. Not for use."
  fi
  if [ -n "$test_build" ]; then
    echo
  fi
  sed -n 1,4p "$source_txt"
  echo "(relay/ holds the whole source of hjv-relay; scripts/package-relay.sh built it in relay/"
  echo "with cargo build --release --locked --target $target and HJV_SERVER, HJV_COMMAND_PUBKEY"
  echo "and HJV_BUILD_COMMIT set to what 'hjv-relay version' prints as server, command keys and"
  echo "commit)"
  grep '^Built by: ' "$source_txt" || true
} > "$source_txt.new"
mv "$source_txt.new" "$source_txt"

mkdir -p "$out"
mtime=$(git -C "$ROOT" log -1 --format=%ct)
tar -C "$work" --owner=0 --group=0 --numeric-owner --sort=name --mtime="@$mtime" -cf - "$name" |
  gzip -9 -n > "$out/$name.tar.gz"
(cd "$out" && sha256sum "$name.tar.gz" > SHA256SUMS && cat SHA256SUMS)
tar -tzvf "$out/$name.tar.gz" | awk '{print $1, $3, $6}'

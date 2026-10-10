#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Writes HJViewer's app version (client/version.env) into the client tree that
# scripts/fetch-upstream.sh prepared: the rustdesk and portable-packer crates (Cargo.toml and their
# Cargo.lock entries, because builds use --locked) and the Flutter app (pubspec.yaml).
# Usage: scripts/set-client-version.sh [<build number>]
#   <build number> becomes pubspec's "+N", the fourth field of the Windows file version, so it must
#   be 0-65535. Android and iOS builds pass their own --build-number to flutter instead.
# Uses perl, which the Linux, macOS and Git for Windows shells all have; the Windows job checks
# upstream out with CRLF line endings.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT/client/version.env"
dir="${UPSTREAM_DIR:-$ROOT/upstream}/rustdesk"
build=${1:-}

if ! [[ $HJV_VERSION =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "error: HJV_VERSION must be three numbers such as 1.5.1 (iOS allows no more): $HJV_VERSION" >&2
  exit 1
fi
if [ -n "$build" ] && { ! [[ $build =~ ^[0-9]+$ ]] || [ "$build" -gt 65535 ]; }; then
  echo "error: the build number must be 0-65535 (a Windows version field): $build" >&2
  exit 1
fi

upstream=$(perl -ne 'if (/^version = "([^"]*)"/) { print $1; exit }' "$dir/Cargo.toml")
if ! perl -e '@a = split /\./, $ARGV[0]; @b = split /\./, $ARGV[1];
    for $i (0 .. 2) { exit 0 if $a[$i] > $b[$i]; exit 1 if $a[$i] < $b[$i] } exit 0' \
    "$HJV_VERSION" "$upstream"; then
  echo "error: HJV_VERSION $HJV_VERSION is below upstream's $upstream; peers compare versions to enable features" >&2
  exit 1
fi

export HJV_VERSION build
for f in Cargo.toml libs/portable/Cargo.toml; do
  perl -0777 -pi -e '$n = s/^version = "[^"]*"/version = "$ENV{HJV_VERSION}"/m;
    END { exit($n == 1 ? 0 : 1) }' "$dir/$f" || { echo "error: no package version in $f" >&2; exit 1; }
done
perl -0777 -pi -e '$n = s/(\r?\nname = "(?:rustdesk|rustdesk-portable-packer)"\r?\nversion = )"[^"]*"/$1"$ENV{HJV_VERSION}"/g;
  END { exit($n == 2 ? 0 : 1) }' "$dir/Cargo.lock" || { echo "error: Cargo.lock lacks the rustdesk or packer entry" >&2; exit 1; }
perl -0777 -pi -e '$n = s/^version: [^+\r\n]*(\+[^\r\n]*)?/"version: $ENV{HJV_VERSION}" . ($ENV{build} ne "" ? "+$ENV{build}" : ($1 \/\/ ""))/me;
  END { exit($n == 1 ? 0 : 1) }' "$dir/flutter/pubspec.yaml" || { echo "error: no version in pubspec.yaml" >&2; exit 1; }

echo "HJViewer version $HJV_VERSION${build:+ (Windows build $build)} (upstream $upstream)"

#!/usr/bin/env bash
# The type scale's watch (U52): counts the lines with a raw font size (`fontSize: <number>`) in the
# app's own Dart files (flutter/lib/**/hjv_*.dart and hjviewer_*.dart) and warns, without failing,
# for each file whose count grew past the baseline ("<count> <path under flutter/lib>" lines; a file
# that is not listed counts from 0). A new size belongs in HjvText (common/hjv_style.dart); a
# deliberate one goes in the baseline.
#   hjv-fontsize-check.sh LIB BASELINE   the warnings, or one line saying there are none; exit 0
#   hjv-fontsize-check.sh --baseline LIB the baseline for LIB as it is now
#   hjv-fontsize-check.sh --self-test    exit 1 when the check misses a grown or a new file
set -eu

count() { # <lib dir>
  (cd "$1" && find . -type f \( -name 'hjv_*.dart' -o -name 'hjviewer_*.dart' \) | LC_ALL=C sort |
    while read -r f; do
      n=$(grep -cE 'fontSize: *[0-9]' "$f" || true)
      if [ "$n" -gt 0 ]; then echo "$n ${f#./}"; fi
    done)
}

check() { # <lib dir> <baseline>
  local n f old warned=0
  while read -r n f; do
    old=$(awk -v f="$f" '$2 == f { print $1 }' "$2")
    if [ "$n" -gt "${old:-0}" ]; then
      echo "warning: $f has $n lines with a raw font size (fontSize: <number>), the baseline ${old:-0}: use a HjvText role (common/hjv_style.dart) or raise the baseline"
      warned=1
    fi
  done < <(count "$1")
  if [ "$warned" = 0 ]; then echo "no new raw font sizes in the app's own Dart files"; fi
}

self_test() {
  local t out
  t=$(mktemp -d)
  trap 'rm -rf "$t"' RETURN
  mkdir -p "$t/lib/common/widgets"
  printf 'a(fontSize: 13);\nb(fontSize: 12, height: 1.4);\nc(fontSize: size);\n' > "$t/lib/common/hjviewer_a.dart"
  printf 'd(fontSize: 14);\n' > "$t/lib/common/widgets/hjv_b.dart"
  printf 'e(fontSize: 15);\n' > "$t/lib/common/other.dart"
  count "$t/lib" > "$t/baseline"
  [ "$(cat "$t/baseline")" = "$(printf '2 common/hjviewer_a.dart\n1 common/widgets/hjv_b.dart')" ] ||
    { echo "self-test: the count is wrong: $(cat "$t/baseline")"; return 1; }
  out=$(check "$t/lib" "$t/baseline")
  [ "$out" = "no new raw font sizes in the app's own Dart files" ] || { echo "self-test: a warning at the baseline: $out"; return 1; }
  # one more than the baseline, and a new file
  printf 'f(fontSize: 11);\n' >> "$t/lib/common/hjviewer_a.dart"
  printf 'g(fontSize: 18);\n' > "$t/lib/common/hjv_new.dart"
  out=$(check "$t/lib" "$t/baseline")
  case "$out" in *"warning: common/hjviewer_a.dart has 3 "*"baseline 2"*) ;; *) echo "self-test: no warning for one more: $out"; return 1 ;; esac
  case "$out" in *"warning: common/hjv_new.dart has 1 "*"baseline 0"*) ;; *) echo "self-test: no warning for a new file: $out"; return 1 ;; esac
  [ "$(printf '%s\n' "$out" | grep -c '^warning: ')" = 2 ] || { echo "self-test: other warnings: $out"; return 1; }
  echo "self-test passed"
}

case "${1:-}" in
  --self-test) self_test ;;
  --baseline) count "$2" ;;
  *) check "$1" "$2" ;;
esac

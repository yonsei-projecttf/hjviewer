#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Regenerates every file in client/overlay/rustdesk (the icons scripts/fetch-upstream.sh copies over
# the client tree) from the SVG masters in client/icons, with ImageMagick 6 `convert` and its own SVG
# renderer (msvg:), so no librsvg or Inkscape is involved.
# Usage: scripts/make-icons.sh [<out dir>]   (default: client/overlay/rustdesk)
#   icon.svg         rounded tile: Windows ICOs, Android legacy icons, res/ PNGs, the SVG logo copies
#   icon-square.svg  the same design full-bleed and opaque: iOS, which applies its own mask
#   glyph.svg        the white foreground alone: Android adaptive foreground (also the monochrome
#                    layer), notification icon, macOS tray template, Android floating-window vector
#   small.svg        optional redraw on the 16 px grid for the 16 and 32 px ICO entries
#   small-24.svg     optional redraw on the 24 px grid for the 24 px ICO entries
# The PNGs keep the formats the overlay has always had (16-bit RGBA; iOS 16-bit RGB without alpha;
# 8-bit gray+alpha for single-colour glyphs) and carry no timestamp, so unchanged masters give
# byte-identical files.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/client/icons"
OUT="${1:-$ROOT/client/overlay/rustdesk}"
BLUE='#1E6FD9' # the tile colour, also Android's adaptive background (ic_launcher_background)
ANDROID=flutter/android/app/src/main/res
IOS=flutter/ios/Runner/Assets.xcassets/AppIcon.appiconset

version=$(convert -version 2>/dev/null || true)
if [[ $version != "Version: ImageMagick 6."* ]]; then
  echo "error: needs ImageMagick 6 (convert)" >&2
  exit 1
fi
masters=(icon.svg icon-square.svg glyph.svg)
for f in small.svg small-24.svg; do
  if [ -f "$SRC/$f" ]; then masters+=("$f"); fi
done
for f in "${masters[@]}"; do
  size=$(identify -density 96 -format '%wx%h' "msvg:$SRC/$f" 2>/dev/null || true)
  if [ "$size" != 1024x1024 ]; then
    echo "error: $SRC/$f must render at 1024x1024 at 96 dpi (got '${size:-nothing}')" >&2
    exit 1
  fi
done
# glyph.svg's path data goes into an Android VectorDrawable as it is
tags=$(grep -oE '<[A-Za-z][A-Za-z0-9:-]*' "$SRC/glyph.svg" | sort -u | tr '\n' ' ')
mapfile -t glyph_paths < <(grep -oE '[[:space:]]d="[^"]*"' "$SRC/glyph.svg" |
  sed -E 's/^[[:space:]]d="(.*)"$/\1/')
if [ "$tags" != "<path <svg " ] || [ ${#glyph_paths[@]} = 0 ] ||
  grep -qE 'transform|stroke|fill-rule|clip|mask' "$SRC/glyph.svg"; then
  echo "error: glyph.svg must hold only fill-only <path d=...> elements without transforms" >&2
  exit 1
fi
for d in "${glyph_paths[@]}"; do
  if ! [[ $d =~ ^[MLHVCSQTAZmlhvcsqtaz0-9eE.,\ +-]+$ ]]; then
    echo "error: glyph.svg has path data a VectorDrawable cannot take: $d" >&2
    exit 1
  fi
done

PNG=(-define png:exclude-chunk=time)
count=0

src() { # <master> <size>: sets $args to read the master at 1024 px and scale it to <size>
  args=(-background none -density 96 "msvg:$SRC/$1")
  # the hinted redraws are averaged per pixel block so their grid-aligned edges stay sharp
  case $1 in small*) args+=(-filter box) ;; esac
  args+=(-resize "$2x$2")
}

master_for() { # <size>: the master for an ICO entry; a hinted redraw is sharp only on its own grid
  if [ "$1" -le 32 ] && [ $(($1 % 16)) = 0 ] && [ -f "$SRC/small.svg" ]; then echo small.svg
  elif [ "$1" -le 32 ] && [ $(($1 % 24)) = 0 ] && [ -f "$SRC/small-24.svg" ]; then echo small-24.svg
  else echo icon.svg; fi
}

rgba() { # <master> <size> <out>: 16-bit RGBA
  src "$1" "$2"
  convert "${args[@]}" -define png:color-type=6 -define png:bit-depth=16 "${PNG[@]}" "$OUT/$3"
  count=$((count + 1))
}

opaque() { # <size> <out>: icon-square.svg as 16-bit RGB without alpha (iOS rejects transparency)
  src icon-square.svg "$1"
  convert "${args[@]}" -background "$BLUE" -alpha remove -alpha off \
    -define png:color-type=2 -define png:bit-depth=16 "${PNG[@]}" "$OUT/$2"
  count=$((count + 1))
}

circle() { # <size> <out>: icon-square.svg cut to its inscribed circle, 16-bit RGBA
  convert -background none -density 96 "msvg:$SRC/icon-square.svg" -alpha set \
    \( -size 1024x1024 xc:none -fill white -draw 'circle 511.5,511.5 511.5,-0.5' \) \
    -compose DstIn -composite -compose Over -resize "$1x$1" \
    -define png:color-type=6 -define png:bit-depth=16 "${PNG[@]}" "$OUT/$2"
  count=$((count + 1))
}

glyph() { # <size> <out> [trim]: glyph.svg in white on transparent, 8-bit gray+alpha. As it is, it
  # fills the adaptive-icon canvas; trimmed, its box is centred with 8% of the size free on every
  # side (notification and menu-bar icons, which get no safe zone around them).
  local size=$1 fit
  local read=(-background none -density 96 "msvg:$SRC/glyph.svg")
  if [ "${3:-}" = trim ]; then
    fit=$(((size * 84 + 50) / 100))
    read+=(-trim +repage -resize "${fit}x$fit" -gravity center -extent "${size}x$size")
  else
    read+=(-resize "${size}x$size")
  fi
  # no cHRM either: primaries mean nothing to a gray image
  convert "${read[@]}" -channel RGB -evaluate set 100% +channel \
    -define png:color-type=4 -define png:bit-depth=8 -define png:exclude-chunk=time,cHRM "$OUT/$2"
  count=$((count + 1))
}

ico() { # <out> <size>...: one entry per size, ascending; ImageMagick stores 256 px entries as PNG
  local out=$1 s all=()
  shift
  for s in "$@"; do
    src "$(master_for "$s")" "$s"
    all+=(\( "${args[@]}" \))
  done
  convert -respect-parentheses "${all[@]}" "$OUT/$out"
  count=$((count + 1))
}

copy() { # <master> <out>
  cp "$SRC/$1" "$OUT/$2"
  count=$((count + 1))
}

mkdir -p "$OUT/res" "$OUT/flutter/assets" "$OUT/$IOS" "$OUT/flutter/windows/runner/resources" \
  "$OUT/$ANDROID/drawable"

# Android: legacy launcher icons (48dp), adaptive foreground (108dp), notification icon (24dp)
for d in mdpi:2 hdpi:3 xhdpi:4 xxhdpi:6 xxxhdpi:8; do # density:scale x2
  dir=$ANDROID/mipmap-${d%:*} q=${d#*:}
  mkdir -p "$OUT/$dir"
  rgba icon.svg $((48 * q / 2)) "$dir/ic_launcher.png"
  circle $((48 * q / 2)) "$dir/ic_launcher_round.png"
  glyph $((108 * q / 2)) "$dir/ic_launcher_foreground.png"
  glyph $((24 * q / 2)) "$dir/ic_stat_logo.png" trim
done

# Android floating window: a #1E6FD9 circle with glyph.svg's paths on it in white
{
  cat <<'EOF'
<?xml version="1.0" encoding="utf-8"?>
<!-- HJ monogram for the floating window (the same shapes as the app icon) -->
<vector xmlns:android="http://schemas.android.com/apk/res/android"
    android:width="320dp"
    android:height="320dp"
    android:viewportWidth="1024"
    android:viewportHeight="1024">
    <path
        android:fillColor="#1E6FD9"
        android:pathData="M0,512 A512,512 0 1,0 1024,512 A512,512 0 1,0 0,512 Z" />
EOF
  for d in "${glyph_paths[@]}"; do
    printf '    <path\n        android:fillColor="#FFFFFF"\n        android:pathData="%s" />\n' "$d"
  done
  echo '</vector>'
} > "$OUT/$ANDROID/drawable/floating_window.xml"
count=$((count + 1))

# iOS app icon set: <name> <pixels>
while read -r name px; do
  opaque "$px" "$IOS/Icon-App-$name.png"
done <<'EOF'
20x20@1x 20
20x20@2x 40
20x20@3x 60
29x29@1x 29
29x29@2x 58
29x29@3x 87
40x40@1x 40
40x40@2x 80
40x40@3x 120
60x60@2x 120
60x60@3x 180
76x76@1x 76
76x76@2x 152
83.5x83.5@2x 167
1024x1024@1x 1024
EOF

# desktop: the Rust binaries and packages (res/), the Flutter Windows runner, the tray icons
rgba icon.svg 1024 res/icon.png
cp "$OUT/res/icon.png" "$OUT/res/mac-icon.png"
count=$((count + 1))
for s in 32 64 128; do rgba icon.svg $s "res/${s}x$s.png"; done
rgba icon.svg 256 res/128x128@2x.png
ico res/icon.ico 16 32 48 64 128
ico flutter/windows/runner/resources/app_icon.ico 16 24 32 48 64 128 256
ico res/tray-icon.ico 16 24 32
glyph 60 res/mac-tray-dark-x2.png trim # macOS uses it as a template: only its alpha counts
glyph 48 res/mac-tray-light-x2.png trim
for f in res/scalable.svg res/logo.svg flutter/assets/icon.svg; do copy icon.svg "$f"; done

echo "wrote $count file(s) under ${OUT#"$ROOT"/} from ${SRC#"$ROOT"/}"

#!/usr/bin/env bash
# Render the brand SVGs (from build.py) to PNGs with headless Chrome, which
# draws their blend modes and grain exactly as the browser does.
#   CHROME=/path/to/chrome scripts/brand/render.sh
set -euo pipefail

cd "$(dirname "$0")/../.."
CHROME="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
PAGE="file://$PWD/scripts/brand/render.html"
PROFILE="$(mktemp -d)"
trap 'rm -rf "$PROFILE"' EXIT

# out, canvas width, canvas height, asset, image width, background
shot() {
  "$CHROME" --headless=new --user-data-dir="$PROFILE" --disable-gpu --hide-scrollbars \
    --force-device-scale-factor=1 --default-background-color=00000000 \
    --window-size="$2,$3" --screenshot="$PWD/public/$1" \
    "$PAGE?cw=$2&ch=$3&a=$4&w=$5&bg=$6" >/dev/null 2>&1
  echo "public/$1"
}

shot brand/imf-tag-512.png    512 369 brand/imf-tag.svg        512 none
shot brand/imf-mark-256.png   256 184 brand/imf-mark.svg       256 none
shot brand/og-image.png      1200 630 brand/imf-tag.svg        600 '%23f1f1ef'
shot apple-touch-icon.png     180 180 brand/imf-mark.svg       152 '%23f6f6f6'
shot favicon-48.png            48  48 brand/imf-mark-small.svg  48 none
shot favicon-32.png            32  32 brand/imf-mark-small.svg  32 none
shot favicon-16.png            16  16 favicon.svg               16 none

#!/usr/bin/env bash
# Render the HTML snapshots produced by `ppm --snapshot` into PNG images using
# a headless Chromium/Chrome.  Usage: scripts/render-screenshots.sh [DIR]
set -euo pipefail
dir="${1:-screenshots}"
browser="${BROWSER_BIN:-chromium}"
cols=132
rows=40
font_size=16
cell_w=9.6
line_h=18
pad=20
width=$(python3 -c "import math; print(math.ceil($cols*$cell_w + 2*$pad))")
height=$(python3 -c "import math; print(math.ceil($rows*$line_h + 2*$pad))")

shopt -s nullglob
for html in "$dir"/*.html; do
  png="${html%.html}.png"
  "$browser" --headless=new --no-sandbox --disable-gpu --hide-scrollbars \
    --user-data-dir="$(realpath .chrome)" \
    --force-device-scale-factor=2 --window-size="${width},${height}" \
    --screenshot="$png" "file://$(realpath "$html")" >/dev/null 2>&1
  echo "rendered $png"
done

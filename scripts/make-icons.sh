#!/usr/bin/env bash
# Regenerates the platform icons from packaging/icon.svg. The outputs are committed, so
# builds and CI don't need these tools: rsvg-convert and magick (Homebrew), iconutil (macOS).
set -euo pipefail
cd "$(dirname "$0")/.."
SVG=packaging/icon.svg
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# macOS draws icons with the grid margin; Windows icons fill their canvas, so the same art
# would look a size smaller next to other apps there.
sed 's/viewBox="0 0 1024 1024"/viewBox="92 92 840 840"/' "$SVG" >"$TMP/windows.svg"

ICONSET="$TMP/Viewer.iconset"
mkdir "$ICONSET"
for size in 16 32 128 256 512; do
  rsvg-convert -w $size -h $size "$SVG" -o "$ICONSET/icon_${size}x${size}.png"
  rsvg-convert -w $((size * 2)) -h $((size * 2)) "$SVG" -o "$ICONSET/icon_${size}x${size}@2x.png"
done
iconutil -c icns "$ICONSET" -o packaging/macos/Viewer.icns

PNGS=()
for size in 16 20 24 32 40 48 64 256; do
  rsvg-convert -w $size -h $size "$TMP/windows.svg" -o "$TMP/win-$size.png"
  PNGS+=("$TMP/win-$size.png")
done
# PNG-compressed entries (Windows Vista and later). ImageMagick stores BMPs: 313 KB vs ~20 KB.
python3 -I - packaging/windows/viewer.ico "${PNGS[@]}" <<'PY'
import struct, sys
out, pngs = sys.argv[1], [open(p, "rb").read() for p in sys.argv[2:]]
offset = 6 + 16 * len(pngs)
header = struct.pack("<HHH", 0, 1, len(pngs))
for png in pngs:
    w, h = struct.unpack(">II", png[16:24])
    header += struct.pack("<BBBBHHII", w % 256, h % 256, 0, 0, 1, 32, len(png), offset)
    offset += len(png)
open(out, "wb").write(header + b"".join(pngs))
PY

# The window icon (title bar, taskbar). macOS ignores it and uses the bundle's icon.
rsvg-convert -w 256 -h 256 "$TMP/windows.svg" -o ui/icon.png

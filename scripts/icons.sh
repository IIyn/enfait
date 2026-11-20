#!/usr/bin/env bash
# Regenerates the PNG and ICO icons from assets/icon/enfait.svg (48 px and
# up) and assets/icon/enfait-small.svg (32 px and under).
# Needs rsvg-convert (librsvg) and python3.
set -euo pipefail
cd "$(dirname "$0")/../assets/icon"

for size in 16 22 32 48 64 128 256 512; do
    if [ "$size" -le 32 ]; then source=enfait-small.svg; else source=enfait.svg; fi
    rsvg-convert -w "$size" -h "$size" "$source" -o "enfait-$size.png"
done

# ICO whose entries are the PNG files themselves (Windows Vista and later):
# a few kilobytes instead of uncompressed bitmaps.
python3 - enfait-16.png enfait-32.png enfait-48.png enfait-64.png enfait-128.png enfait-256.png <<'PY'
import struct
import sys

images = []
for name in sys.argv[1:]:
    with open(name, "rb") as f:
        data = f.read()
    width, height = struct.unpack(">II", data[16:24])
    images.append((width, height, data))

out = struct.pack("<HHH", 0, 1, len(images))
offset = 6 + 16 * len(images)
for width, height, data in images:
    # 256 is written as 0 in the directory.
    out += struct.pack("<BBBBHHII", width % 256, height % 256, 0, 0, 1, 32, len(data), offset)
    offset += len(data)
out += b"".join(data for _, _, data in images)

with open("enfait.ico", "wb") as f:
    f.write(out)
PY

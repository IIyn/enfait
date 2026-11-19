#!/usr/bin/env bash
# Builds enfait.app and a .dmg from target/release.
#   packaging/macos/bundle.sh <version> <out-dir>
set -euo pipefail
cd "$(dirname "$0")/../.."
version="$1"
out="$2"
arch="$(uname -m)"

app="$out/enfait.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/enfait target/release/enfait-cli "$app/Contents/MacOS/"
sed "s/@VERSION@/$version/g" packaging/macos/Info.plist > "$app/Contents/Info.plist"

iconset="$out/enfait.iconset"
rm -rf "$iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
    cp "assets/icon/enfait-$size.png" "$iconset/icon_${size}x${size}.png"
done
cp assets/icon/enfait-32.png "$iconset/icon_16x16@2x.png"
cp assets/icon/enfait-64.png "$iconset/icon_32x32@2x.png"
cp assets/icon/enfait-256.png "$iconset/icon_128x128@2x.png"
cp assets/icon/enfait-512.png "$iconset/icon_256x256@2x.png"
iconutil -c icns "$iconset" -o "$app/Contents/Resources/enfait.icns"
rm -rf "$iconset"

# Ad-hoc signature: required to launch on Apple Silicon. Not notarised, so
# Gatekeeper asks for confirmation on first launch.
codesign --force --deep --sign - "$app"

stage="$out/dmg"
rm -rf "$stage"
mkdir -p "$stage"
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
hdiutil create -volname enfait -srcfolder "$stage" -ov -format UDZO \
    "$out/enfait-$version-$arch-macos.dmg"
rm -rf "$stage"

#!/usr/bin/env bash
# Builds target/release/Viewer.app from the release binary.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release
VERSION=$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json, sys; print(json.load(sys.stdin)["packages"][0]["version"])')
APP=target/release/Viewer.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp -f target/release/viewer "$APP/Contents/MacOS/viewer"
cp -f packaging/macos/Info.plist "$APP/Contents/Info.plist"
cp -f packaging/macos/Viewer.icns "$APP/Contents/Resources/Viewer.icns"
plutil -insert CFBundleShortVersionString -string "$VERSION" "$APP/Contents/Info.plist"
plutil -insert CFBundleVersion -string "$VERSION" "$APP/Contents/Info.plist"
# Ad-hoc signature: Apple Silicon refuses to run unsigned code, and the linker's own
# signature doesn't cover the bundle.
codesign --force --sign - "$APP"
echo "$APP"

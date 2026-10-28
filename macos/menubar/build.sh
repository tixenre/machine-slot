#!/bin/bash
# Builds «Machine Slot.app», a menu bar view of the machine-slot queue.
#   ./build.sh             build into build/ and open it
#   ./build.sh --install   also copy it to ~/Applications
#   ./build.sh --no-open   only build (for releases)
#   Starting at login is a switch in its menu: Open at Login.
set -euo pipefail
cd "$(dirname "$0")"
APP="build/Machine Slot.app"
ID=dev.mancino.machine-slot.menubar

mkdir -p "$APP/Contents/MacOS"
# One app for Apple silicon and Intel Macs.
VERSION="$(sed -nE 's/^version = "(.*)"/\1/p' ../../Cargo.toml | head -1)"
for arch in arm64 x86_64; do
    swiftc -O -target "$arch-apple-macos13" main.swift -o "build/bar-$arch"
done
lipo -create build/bar-arm64 build/bar-x86_64 -output "$APP/Contents/MacOS/machine-slot-bar"
rm build/bar-arm64 build/bar-x86_64
cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>$ID</string>
  <key>CFBundleName</key><string>Machine Slot</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleExecutable</key><string>machine-slot-bar</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>LSUIElement</key><true/>
</dict></plist>
EOF
codesign --force --sign - "$APP" >/dev/null

if [ "${1:-}" = --install ]; then
    mkdir -p "$HOME/Applications"
    rm -rf "$HOME/Applications/Machine Slot.app"
    cp -R "$APP" "$HOME/Applications/"
    APP="$HOME/Applications/Machine Slot.app"
fi
[ "${1:-}" = --no-open ] && { echo "Machine Slot: $APP"; exit 0; }
pkill -x machine-slot-bar 2>/dev/null || true
open "$APP"
echo "Machine Slot: $APP"

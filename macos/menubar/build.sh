#!/bin/bash
# Builds «Machine Slot.app», a menu bar view of the machine-slot queue.
#   ./build.sh             build into build/ and open it
#   ./build.sh --install   also copy it to ~/Applications
#   Starting at login is a switch in its menu: Open at Login.
set -euo pipefail
cd "$(dirname "$0")"
APP="build/Machine Slot.app"
ID=dev.mancino.machine-slot.menubar

mkdir -p "$APP/Contents/MacOS"
swiftc -O main.swift -o "$APP/Contents/MacOS/machine-slot-bar"
cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>$ID</string>
  <key>CFBundleName</key><string>Machine Slot</string>
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
pkill -x machine-slot-bar 2>/dev/null || true
open "$APP"
echo "Machine Slot: $APP"

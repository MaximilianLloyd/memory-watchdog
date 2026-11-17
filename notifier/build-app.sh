#!/bin/bash
# Build "Memory Watchdog.app" (notification helper + icon) into target/.
set -euo pipefail
cd "$(dirname "$0")/.."
APP="target/Memory Watchdog.app"
WORK=target/icon
rm -rf "$APP" "$WORK"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$WORK/AppIcon.iconset"

swiftc -O -o "$APP/Contents/MacOS/memory-watchdog-notify" notifier/main.swift
cp notifier/Info.plist "$APP/Contents/Info.plist"

swift icon/make-icon.swift "$WORK/icon-1024.png"
for s in 16 32 128 256 512; do
  sips -z $s $s "$WORK/icon-1024.png" --out "$WORK/AppIcon.iconset/icon_${s}x${s}.png" >/dev/null
  d=$((s * 2))
  sips -z $d $d "$WORK/icon-1024.png" --out "$WORK/AppIcon.iconset/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$WORK/AppIcon.iconset" -o "$APP/Contents/Resources/AppIcon.icns"

codesign --force --sign - "$APP" >/dev/null 2>&1
echo "built $APP"

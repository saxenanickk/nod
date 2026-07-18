#!/usr/bin/env bash
# Builds a distributable nod.app bundle for macOS:
#   1. renders the icon at every iconset size and packs a .icns
#   2. builds the release binary
#   3. assembles dist/nod.app (Info.plist + icon + binary)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

APP_NAME="nod"
BUNDLE_ID="com.saxenanickk.nod"
VERSION="0.1.0"

ICONSET="$(mktemp -d)/${APP_NAME}.iconset"
mkdir -p "$ICONSET"

echo "==> Rendering icon…"
# name:size pairs for a macOS iconset.
render() { swift scripts/render_icon.swift "$2" "$ICONSET/$1" >/dev/null; }
render icon_16x16.png 16
render icon_16x16@2x.png 32
render icon_32x32.png 32
render icon_32x32@2x.png 64
render icon_128x128.png 128
render icon_128x128@2x.png 256
render icon_256x256.png 256
render icon_256x256@2x.png 512
render icon_512x512.png 512
render icon_512x512@2x.png 1024

mkdir -p assets
iconutil -c icns "$ICONSET" -o "assets/${APP_NAME}.icns"
swift scripts/render_icon.swift 1024 "assets/icon-1024.png" >/dev/null
echo "    assets/${APP_NAME}.icns"

echo "==> Building release binary… (this can take a few minutes)"
cargo build --release --bin "$APP_NAME"

APP="dist/${APP_NAME}.app"
echo "==> Assembling ${APP}…"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "target/release/${APP_NAME}" "$APP/Contents/MacOS/${APP_NAME}"
cp "assets/${APP_NAME}.icns" "$APP/Contents/Resources/${APP_NAME}.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>               <string>${APP_NAME}</string>
    <key>CFBundleDisplayName</key>        <string>${APP_NAME}</string>
    <key>CFBundleIdentifier</key>         <string>${BUNDLE_ID}</string>
    <key>CFBundleExecutable</key>         <string>${APP_NAME}</string>
    <key>CFBundleIconFile</key>           <string>${APP_NAME}</string>
    <key>CFBundlePackageType</key>        <string>APPL</string>
    <key>CFBundleShortVersionString</key> <string>${VERSION}</string>
    <key>CFBundleVersion</key>            <string>${VERSION}</string>
    <key>CFBundleInfoDictionaryVersion</key> <string>6.0</string>
    <key>LSMinimumSystemVersion</key>     <string>11.0</string>
    <key>LSApplicationCategoryType</key>  <string>public.app-category.developer-tools</string>
    <key>NSHighResolutionCapable</key>    <true/>
</dict>
</plist>
PLIST

# Refresh Finder/Dock icon cache for the new bundle.
touch "$APP"
echo "==> Done: ${APP}"
echo "    Drag it to /Applications, or run: open '${APP}'"

#!/bin/bash
# Build Mnemonic.app — a draggable macOS application bundle around the
# MnemonicApp Swift Package executable.
#
# Usage:
#   ./scripts/build-app.sh                  # build into ./build/
#   ./scripts/build-app.sh --install        # also copy to ~/Applications/
#
# Idempotent. No code signing — for personal use only.

set -euo pipefail

PKG_DIR="$(cd "$(dirname "$0")/.." && pwd)"
BUILD_DIR="$PKG_DIR/build"
APP_NAME="Mnemonic.app"
APP_PATH="$BUILD_DIR/$APP_NAME"
INSTALL=false

for arg in "$@"; do
  case "$arg" in
    --install) INSTALL=true ;;
    *) echo "Unknown arg: $arg" >&2; exit 2 ;;
  esac
done

echo "==> Building MnemonicApp (release)"
cd "$PKG_DIR"
swift build -c release --product MnemonicApp

BIN="$PKG_DIR/.build/release/MnemonicApp"
if [[ ! -f "$BIN" ]]; then
  echo "Build failed: $BIN not found" >&2
  exit 1
fi

echo "==> Assembling $APP_NAME"
rm -rf "$APP_PATH"
mkdir -p "$APP_PATH/Contents/MacOS"
mkdir -p "$APP_PATH/Contents/Resources"

cp "$BIN" "$APP_PATH/Contents/MacOS/MnemonicApp"
chmod +x "$APP_PATH/Contents/MacOS/MnemonicApp"

cat > "$APP_PATH/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleDisplayName</key>
    <string>Mnemonic</string>
    <key>CFBundleExecutable</key>
    <string>MnemonicApp</string>
    <key>CFBundleIdentifier</key>
    <string>com.kossvat.mnemonic.app</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>Mnemonic</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>0.1.0</string>
    <key>CFBundleVersion</key>
    <string>1</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.productivity</string>
    <key>LSMinimumSystemVersion</key>
    <string>14.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSAppTransportSecurity</key>
    <dict>
        <key>NSAllowsLocalNetworking</key>
        <true/>
    </dict>
    <key>NSSupportsAutomaticTermination</key>
    <false/>
    <key>NSSupportsSuddenTermination</key>
    <false/>
</dict>
</plist>
PLIST

# Optional icon — if assets/logo.png exists in the parent mnemonic dir,
# convert to .icns. Skipped silently if `iconutil` isn't around.
ICON_SOURCE="$PKG_DIR/../../assets/logo.png"
if [[ -f "$ICON_SOURCE" ]] && command -v sips >/dev/null && command -v iconutil >/dev/null; then
  ICONSET="$BUILD_DIR/Mnemonic.iconset"
  rm -rf "$ICONSET"
  mkdir -p "$ICONSET"
  for SZ in 16 32 64 128 256 512; do
    sips -z "$SZ" "$SZ" "$ICON_SOURCE" --out "$ICONSET/icon_${SZ}x${SZ}.png" >/dev/null
    DBL=$((SZ * 2))
    sips -z "$DBL" "$DBL" "$ICON_SOURCE" --out "$ICONSET/icon_${SZ}x${SZ}@2x.png" >/dev/null
  done
  iconutil -c icns "$ICONSET" -o "$APP_PATH/Contents/Resources/Mnemonic.icns" 2>/dev/null || true
  if [[ -f "$APP_PATH/Contents/Resources/Mnemonic.icns" ]]; then
    /usr/libexec/PlistBuddy -c "Add :CFBundleIconFile string Mnemonic" \
      "$APP_PATH/Contents/Info.plist" 2>/dev/null || true
  fi
  rm -rf "$ICONSET"
fi

# Strip the quarantine attribute so first launch doesn't get blocked by
# Gatekeeper when this was built locally.
xattr -dr com.apple.quarantine "$APP_PATH" 2>/dev/null || true

# Ad-hoc signature so macOS treats the bundle as a real signed app
# (no Developer ID, no notarization — just enough to skip the "damaged
# / can't be verified" Gatekeeper popup on first launch). Silent on
# failure: if `codesign` is missing this still works as an unsigned bundle.
codesign --force --deep --sign - "$APP_PATH" 2>/dev/null || true

echo "==> Bundle ready: $APP_PATH"

if [[ "$INSTALL" == "true" ]]; then
  TARGET="$HOME/Applications/$APP_NAME"
  mkdir -p "$HOME/Applications"
  rm -rf "$TARGET"
  cp -R "$APP_PATH" "$TARGET"
  echo "==> Installed to: $TARGET"
fi

echo ""
echo "Open with:  open '$APP_PATH'"
[[ "$INSTALL" == "true" ]] && echo "Or from ~/Applications via Launchpad / Spotlight."

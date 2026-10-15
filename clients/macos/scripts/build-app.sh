#!/bin/bash
# Build Mnemonic.app — a draggable macOS application bundle around the
# MnemonicApp Swift Package executable.
#
# Usage:
#   ./scripts/build-app.sh                  # stage in ./build/Mnemonic.noindex/
#   ./scripts/build-app.sh --install        # also copy to ~/Applications/
#   ./scripts/build-app.sh --icon FILE      # make the icon from FILE
#
# Idempotent. No code signing — for personal use only.

set -euo pipefail

PKG_DIR="$(cd "$(dirname "$0")/.." && pwd)"
INVOKED_FROM="$PWD"
BUILD_DIR="$PKG_DIR/build"
APP_NAME="Mnemonic.app"
ICON_SOURCE="$PKG_DIR/../../assets/app-icon.png"
INSTALL=false

# The value of an option: there, not empty, and not another option.
need_value() {
  [[ $# -ge 2 && -n "${2}" && "${2}" != --* ]] || { echo "${1} needs a value" >&2; exit 2; }
}

while (($#)); do
  case "${1}" in
    --install) INSTALL=true ;;
    --icon)
      need_value "$@"
      # Relative to where the script was called from, not to the package.
      case "${2}" in
        /*) ICON_SOURCE="${2}" ;;
        *) ICON_SOURCE="$INVOKED_FROM/${2}" ;;
      esac
      shift ;;
    *) echo "Unknown arg: ${1}" >&2; exit 2 ;;
  esac
  shift
done

# The search of the system does not index a folder whose name ends in
# .noindex, so the staged bundle is never found beside an installed one.
STAGE_DIR="$BUILD_DIR/Mnemonic.noindex"
APP_PATH="$STAGE_DIR/$APP_NAME"

# The script stages and removes only inside the build folder of the
# package. A link standing in for that folder, or for what the script
# removes there, could lead anywhere, so the script stops instead.
for OWNED in "$BUILD_DIR" "$STAGE_DIR" "$APP_PATH" "$BUILD_DIR/$APP_NAME"; do
  if [[ -L "$OWNED" ]]; then
    echo "Refusing to build: $OWNED is a link" >&2
    exit 2
  fi
done

# The icon is not optional. Its source and its tools are checked before
# the build, so a bundle without an icon is never made.
if [[ ! -f "$ICON_SOURCE" ]]; then
  echo "Icon source not found: $ICON_SOURCE" >&2
  exit 1
fi
for TOOL in sips iconutil; do
  command -v "$TOOL" >/dev/null || { echo "Tool not found: $TOOL" >&2; exit 1; }
done

echo "==> Building MnemonicApp (release)"
cd "$PKG_DIR"
swift build -c release --product MnemonicApp

BIN="$PKG_DIR/.build/release/MnemonicApp"
if [[ ! -f "$BIN" ]]; then
  echo "Build failed: $BIN not found" >&2
  exit 1
fi

# A raw executable has no bundle manifest. Bind its route capability to
# these exact built bytes so a later rebuild cannot reuse a stale receipt.
ROUTE_DIGEST="$(/usr/bin/shasum -a 256 "$BIN" | /usr/bin/cut -d ' ' -f 1)"
[[ "$ROUTE_DIGEST" =~ ^[0-9a-f]{64}$ ]] || exit 1
ROUTE_RECEIPT="$(mktemp "${BIN}.route-capability.XXXXXX")"
trap 'rm -f "$ROUTE_RECEIPT"' EXIT
cat > "$ROUTE_RECEIPT" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
    <key>MnemonicRouteProtocolVersion</key>
    <integer>1</integer>
    <key>MnemonicExecutableSHA256</key>
    <string>$ROUTE_DIGEST</string>
</dict>
</plist>
PLIST
mv -f "$ROUTE_RECEIPT" "${BIN}.route-capability.plist"
trap - EXIT

echo "==> Assembling $APP_NAME"
# Earlier versions of this script staged the bundle here, where the search
# of the system indexed it as a second app.
rm -rf "$BUILD_DIR/$APP_NAME"
rm -rf "$APP_PATH"
# A bundle that was not finished is removed, whatever stopped the script.
FINISHED=false
trap '[[ "$FINISHED" == true ]] || rm -rf "$APP_PATH"' EXIT
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
    <key>CFBundleIconFile</key>
    <string>Mnemonic</string>
    <key>CFBundleIdentifier</key>
    <string>com.kossvat.mnemonic.app</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>Mnemonic</string>
    <key>MnemonicRouteProtocolVersion</key>
    <integer>1</integer>
    <key>CFBundleURLTypes</key>
    <array>
        <dict>
            <key>CFBundleTypeRole</key>
            <string>Viewer</string>
            <key>CFBundleURLSchemes</key>
            <array>
                <string>mnemonic</string>
            </array>
        </dict>
    </array>
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

# The icon, in the representations the system asks of one: 16, 32, 128,
# 256 and 512 points, each at 1x and at 2x.
ICONSET="$STAGE_DIR/Mnemonic.iconset"
rm -rf "$ICONSET"
mkdir -p "$ICONSET"
for SZ in 16 32 128 256 512; do
  DBL=$((SZ * 2))
  sips -s format png -z "$SZ" "$SZ" "$ICON_SOURCE" --out "$ICONSET/icon_${SZ}x${SZ}.png" >/dev/null
  sips -s format png -z "$DBL" "$DBL" "$ICON_SOURCE" --out "$ICONSET/icon_${SZ}x${SZ}@2x.png" >/dev/null
  [[ -s "$ICONSET/icon_${SZ}x${SZ}.png" && -s "$ICONSET/icon_${SZ}x${SZ}@2x.png" ]] || {
    echo "The icon could not be drawn from $ICON_SOURCE at $SZ points" >&2
    exit 1
  }
done
iconutil -c icns "$ICONSET" -o "$APP_PATH/Contents/Resources/Mnemonic.icns"
rm -rf "$ICONSET"

# Strip the quarantine attribute so first launch doesn't get blocked by
# Gatekeeper when this was built locally.
xattr -dr com.apple.quarantine "$APP_PATH" 2>/dev/null || true

# Ad-hoc signature so macOS treats the bundle as a real signed app
# (no Developer ID, no notarization — just enough to skip the "damaged
# / can't be verified" Gatekeeper popup on first launch). Silent on
# failure: if `codesign` is missing this still works as an unsigned bundle.
codesign --force --deep --sign - "$APP_PATH" 2>/dev/null || true

FINISHED=true
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
if [[ "$INSTALL" == "true" ]]; then
  echo "Or from ~/Applications via Launchpad / Spotlight."
fi

#!/bin/sh
# Builds PilotHouse.app and PilotHouse-macOS-universal.dmg into dist/ (with
# the name in brand/brand.toml: <name>.app, <name without spaces>-macOS-…).
#
#   scripts/package/macos.sh            # universal (Apple silicon + Intel)
#   scripts/package/macos.sh --native   # this Mac's architecture only (quick local check)
#
# The app is signed ad hoc (no Apple Developer certificate): Apple silicon
# requires a signature to run, and Gatekeeper asks once on first open.
set -eu
cd "$(dirname "$0")/../.."
VERSION=$(scripts/package/version.sh)
BRAND_NAME=$(scripts/package/brand.sh app name)
BRAND_ID=$(scripts/package/brand.sh app id)
STEM=$(scripts/package/brand.sh stem)
DIST=dist
APP="$DIST/$BRAND_NAME.app"

if [ "${1:-}" = "--native" ]; then
  cargo build --release -p pilothouse
  BINARY=target/release/pilothouse
  NAME="$STEM-macOS-$(uname -m).dmg"
else
  rustup target add aarch64-apple-darwin x86_64-apple-darwin
  cargo build --release -p pilothouse --target aarch64-apple-darwin
  cargo build --release -p pilothouse --target x86_64-apple-darwin
  mkdir -p target/universal-apple-darwin/release
  BINARY=target/universal-apple-darwin/release/pilothouse
  lipo -create -output "$BINARY" \
    target/aarch64-apple-darwin/release/pilothouse \
    target/x86_64-apple-darwin/release/pilothouse
  NAME="$STEM-macOS-universal.dmg"
fi

rm -rf "$APP" && mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BINARY" "$APP/Contents/MacOS/pilothouse"
cp brand/icons/app.icns "$APP/Contents/Resources/AppIcon.icns"
# macOS takes only numbers there: 1.0.0 for 1.0.0-beta. The name and id
# come from the brand (checked by the app's build: no quotes or slashes).
sed -e "s/@VERSION@/${VERSION%%-*}/g" -e "s|@NAME@|$BRAND_NAME|g" -e "s|@ID@|$BRAND_ID|g" \
  scripts/package/Info.plist > "$APP/Contents/Info.plist"
cp LICENSE NOTICE "$APP/Contents/Resources/"
codesign --force --deep --sign - "$APP"

# The app zipped, for the app's updater (it replaces the app with this).
ditto -c -k --keepParent "$APP" "$DIST/${NAME%.dmg}.zip"

# The disk image: the app and a shortcut to Applications, to drag it onto.
STAGE=$(mktemp -d)
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
rm -f "$DIST/$NAME"
# The image's size is given, not guessed: hdiutil's own estimate can come
# out too small ("create failed - No space left on device" with the disk
# nearly empty, as on CI for 0.3.0). A failed try is retried once or twice,
# since hdiutil also fails now and then when the system holds the files.
SIZE_MB=$(( $(du -sk "$STAGE" | cut -f1) / 1024 * 12 / 10 + 32 ))
for try in 1 2 3; do
  if hdiutil create -volname "$BRAND_NAME" -srcfolder "$STAGE" -fs HFS+ -size "${SIZE_MB}m" \
    -ov -format UDZO "$DIST/$NAME" >/dev/null; then
    break
  fi
  [ "$try" = 3 ] && exit 1
  sleep 5
done
rm -rf "$STAGE"
echo "$DIST/$NAME"

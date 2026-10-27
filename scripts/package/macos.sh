#!/bin/sh
# Builds PilotHouse.app and PilotHouse-macOS-universal.dmg into dist/.
#
#   scripts/package/macos.sh            # universal (Apple silicon + Intel)
#   scripts/package/macos.sh --native   # this Mac's architecture only (quick local check)
#
# The app is signed ad hoc (no Apple Developer certificate): Apple silicon
# requires a signature to run, and Gatekeeper asks once on first open.
set -eu
cd "$(dirname "$0")/../.."
VERSION=$(scripts/package/version.sh)
DIST=dist
APP="$DIST/PilotHouse.app"

if [ "${1:-}" = "--native" ]; then
  cargo build --release -p pilothouse
  BINARY=target/release/pilothouse
  NAME="PilotHouse-macOS-$(uname -m).dmg"
else
  rustup target add aarch64-apple-darwin x86_64-apple-darwin
  cargo build --release -p pilothouse --target aarch64-apple-darwin
  cargo build --release -p pilothouse --target x86_64-apple-darwin
  mkdir -p target/universal-apple-darwin/release
  BINARY=target/universal-apple-darwin/release/pilothouse
  lipo -create -output "$BINARY" \
    target/aarch64-apple-darwin/release/pilothouse \
    target/x86_64-apple-darwin/release/pilothouse
  NAME=PilotHouse-macOS-universal.dmg
  # A CI runner's disk is small: two release builds leave too little room
  # for the disk image, so their intermediates go first.
  if [ -n "${CI:-}" ]; then
    for arch in aarch64-apple-darwin x86_64-apple-darwin; do
      rm -rf "target/$arch/release/deps" "target/$arch/release/build" \
        "target/$arch/release/incremental"
    done
  fi
fi

rm -rf "$APP" && mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BINARY" "$APP/Contents/MacOS/pilothouse"
cp crates/app/assets/icon/PilotHouse.icns "$APP/Contents/Resources/PilotHouse.icns"
# macOS takes only numbers there: 1.0.0 for 1.0.0-beta.
sed "s/@VERSION@/${VERSION%%-*}/g" scripts/package/Info.plist > "$APP/Contents/Info.plist"
cp LICENSE NOTICE "$APP/Contents/Resources/"
codesign --force --deep --sign - "$APP"

# The app zipped, for PilotHouse's updater (it replaces the app with this).
ditto -c -k --keepParent "$APP" "$DIST/${NAME%.dmg}.zip"

# The disk image: the app and a shortcut to Applications, to drag it onto.
STAGE=$(mktemp -d)
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
rm -f "$DIST/$NAME"
hdiutil create -volname PilotHouse -srcfolder "$STAGE" -ov -format UDZO "$DIST/$NAME" >/dev/null
rm -rf "$STAGE"
echo "$DIST/$NAME"

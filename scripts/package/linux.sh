#!/bin/sh
# Builds the Linux packages into dist/: a tarball, a .deb and an AppImage.
#
#   scripts/package/linux.sh
#
# Build on the oldest system you support (CI uses Ubuntu 22.04), since the
# binary needs that glibc or newer. The AppImage is made with appimagetool,
# downloaded at a pinned version and checked against its SHA-256; it checks
# the desktop entry with `desktop-file-validate` (package desktop-file-utils).
set -eu
cd "$(dirname "$0")/../.."
command -v desktop-file-validate >/dev/null ||
  { echo "desktop-file-validate is missing: install desktop-file-utils" >&2; exit 1; }
VERSION=$(scripts/package/version.sh)
DIST=dist
mkdir -p "$DIST"
cargo build --release -p pilothouse
BINARY=target/release/pilothouse
ICON=crates/app/assets/icon/PilotHouse-1024.png

# The desktop entry, shared by every package.
desktop() {
  cat <<DESKTOP
[Desktop Entry]
Type=Application
Name=PilotHouse
Comment=Coding agents, code, Git, devices and planning in one app
Exec=$1 %F
Icon=pilothouse
Terminal=false
Categories=Development;IDE;
MimeType=inode/directory;text/plain;
StartupWMClass=pilothouse
DESKTOP
}

# 1. Tarball: the binary, its icon and desktop entry, the licences.
TAR=$(mktemp -d)/pilothouse
mkdir -p "$TAR"
cp "$BINARY" "$ICON" LICENSE NOTICE "$TAR/"
desktop pilothouse > "$TAR/pilothouse.desktop"
tar -czf "$DIST/PilotHouse-Linux-x86_64.tar.gz" -C "$(dirname "$TAR")" pilothouse

# 2. Debian package.
DEB=$(mktemp -d)
mkdir -p "$DEB/DEBIAN" "$DEB/usr/bin" "$DEB/usr/share/applications" \
  "$DEB/usr/share/icons/hicolor/512x512/apps" "$DEB/usr/share/doc/pilothouse"
cp "$BINARY" "$DEB/usr/bin/pilothouse"
desktop /usr/bin/pilothouse > "$DEB/usr/share/applications/pilothouse.desktop"
cp "$ICON" "$DEB/usr/share/icons/hicolor/512x512/apps/pilothouse.png"
cp LICENSE NOTICE "$DEB/usr/share/doc/pilothouse/"
cat > "$DEB/DEBIAN/control" <<CONTROL
Package: pilothouse
Version: $(echo "$VERSION" | tr - '~')
Section: devel
Priority: optional
Architecture: amd64
Maintainer: LibreGuild <noreply@github.com>
Homepage: https://github.com/LibreGuild/PilotHouse
Depends: libc6 (>= 2.35), libxkbcommon0, libxkbcommon-x11-0, libwayland-client0, libvulkan1, libfontconfig1, libfreetype6, libx11-xcb1, libxcb1, libdbus-1-3, git
Description: One desktop app for coding agents
 Chat with Claude Code, Codex, Gemini CLI or Antigravity beside your code,
 Git, a board, terminals, a browser and Android devices. On-device.
CONTROL
dpkg-deb --root-owner-group --build "$DEB" "$DIST/PilotHouse-Linux-amd64.deb" >/dev/null

# 3. AppImage.
TOOL_VERSION=1.9.0
TOOL_SHA256=46fdd785094c7f6e545b61afcfb0f3d98d8eab243f644b4b17698c01d06083d1
TOOL=$(mktemp -d)/appimagetool
curl -fsSL -o "$TOOL" \
  "https://github.com/AppImage/appimagetool/releases/download/$TOOL_VERSION/appimagetool-x86_64.AppImage"
echo "$TOOL_SHA256  $TOOL" | sha256sum -c -
chmod +x "$TOOL"
APPDIR=$(mktemp -d)/PilotHouse.AppDir
mkdir -p "$APPDIR/usr/bin"
cp "$BINARY" "$APPDIR/usr/bin/pilothouse"
cp "$ICON" "$APPDIR/pilothouse.png"
desktop pilothouse > "$APPDIR/pilothouse.desktop"
cat > "$APPDIR/AppRun" <<'APPRUN'
#!/bin/sh
here=$(dirname "$(readlink -f "$0")")
exec "$here/usr/bin/pilothouse" "$@"
APPRUN
chmod +x "$APPDIR/AppRun"
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$TOOL" "$APPDIR" "$DIST/PilotHouse-Linux-x86_64.AppImage" >/dev/null
ls -1 "$DIST"

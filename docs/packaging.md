# Packaging and releases

Installers are built by `scripts/package/` and published by the **Release** workflow (`.github/workflows/release.yml`).

| System | Files | Script |
|---|---|---|
| macOS 11+ | `PilotHouse-macOS-universal.dmg` (Apple silicon and Intel in one app) | `scripts/package/macos.sh` (`--native` for this Mac only, quicker) |
| Windows 10/11 x64 | `PilotHouse-Windows-Setup-x64.exe` (NSIS, per user, no admin), `PilotHouse-Windows-Portable-x64.zip` | `scripts/package/windows.ps1` (needs NSIS) |
| Linux x86-64 | `PilotHouse-Linux-x86_64.AppImage`, `PilotHouse-Linux-amd64.deb`, `PilotHouse-Linux-x86_64.tar.gz` | `scripts/package/linux.sh` (build on Ubuntu 22.04 or older) |

Everything lands in `dist/` (git-ignored). The version comes from `Cargo.toml` (`scripts/package/version.sh`), now `0.2.0`. A pre-release version such as `1.0.0-beta` is shown as "1.0.0 beta"; where only numbers are allowed the suffix is dropped (the macOS bundle version and Windows file version read `1.0.0`), and the `.deb` is `1.0.0~beta`, which Debian sorts before `1.0.0`.

- **macOS:** the script builds both architectures, joins them with `lipo`, makes `PilotHouse.app` (`Info.plist` with the folder and text document types that Finder's *Open With* uses, the icon), signs it ad hoc and wraps it in a DMG with a shortcut to Applications.
- **Windows:** the icon is built into `pilothouse.exe` by `crates/app/build.rs` (resource 1, which GPUI shows for the window and the taskbar). The installer puts the app in `%LOCALAPPDATA%\Programs\PilotHouse`, adds a Start menu entry (a desktop shortcut is optional) and an uninstaller; uninstalling keeps your data.
- **Linux:** one desktop entry for all three packages (`Categories=Development;IDE;`, folders and text files). The AppImage is made with `appimagetool`, pinned and checked against its SHA-256; it needs `desktop-file-validate` (`desktop-file-utils`), which checks the desktop entry.

## Releases

- **Release:** push a tag `vX.Y.Z` matching `Cargo.toml` (`v0.2.0` now). Only maintainers can create `v*` tags. The release becomes the latest; a version with a suffix (`-beta`) is published as a pre-release instead. There are no nightly builds.
- **Check a branch:** start the Release workflow by hand on any other branch (`gh workflow run release.yml --ref <branch>`): it builds every installer and keeps them as run artifacts, without publishing.

Each build job attests its files ([build provenance](https://docs.github.com/actions/security-for-github-actions/using-artifact-attestations)), and the release carries `SHA256SUMS`. Check a download with `gh attestation verify <file> -R LibreGuild/PilotHouse`.

## Signing

The builds are not code-signed: that needs a paid Apple Developer ID and a Windows code-signing certificate. Until then, macOS asks once (*Open Anyway* in Privacy & Security) and Windows SmartScreen may warn (*More info → Run anyway*); the release notes say so. The checksums and attestations are how a download is verified.

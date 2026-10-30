# Packaging and releases

Installers are built by `scripts/package/` and published by the **Release** workflow (`.github/workflows/release.yml`).

| System | Files | Script |
|---|---|---|
| macOS 11+ | `PilotHouse-macOS-universal.dmg` (Apple silicon and Intel in one app) | `scripts/package/macos.sh` (`--native` for this Mac only, quicker) |
| Windows 10/11 x64 | `PilotHouse-Windows-Setup-x64.exe` (NSIS, per user, no admin), `PilotHouse-Windows-Portable-x64.zip` | `scripts/package/windows.ps1` (needs NSIS) |
| Linux x86-64 | `PilotHouse-Linux-x86_64.AppImage`, `PilotHouse-Linux-amd64.deb`, `PilotHouse-Linux-x86_64.tar.gz` | `scripts/package/linux.sh` (build on Ubuntu 22.04 or older) |

Everything lands in `dist/` (git-ignored). The version comes from `Cargo.toml` (`scripts/package/version.sh`), now `0.4.1`. A pre-release version such as `1.0.0-beta` is shown as "1.0.0 beta"; where only numbers are allowed the suffix is dropped (the macOS bundle version and Windows file version read `1.0.0`), and the `.deb` is `1.0.0~beta`, which Debian sorts before `1.0.0`.

- **macOS:** the script builds both architectures, joins them with `lipo`, makes `PilotHouse.app` (`Info.plist` with the folder and text document types that Finder's *Open With* uses, the icon), signs it ad hoc and wraps it in a DMG with a shortcut to Applications.
- **Windows:** the icon is built into `pilothouse.exe` by `crates/app/build.rs` (resource 1, which GPUI shows for the window and the taskbar). The installer puts the app in `%LOCALAPPDATA%\Programs\PilotHouse`, adds a Start menu entry (a desktop shortcut is optional) and an uninstaller; uninstalling keeps your data.
- **Linux:** one desktop entry for all three packages (`Categories=Development;IDE;`, folders and text files). The AppImage is made with `appimagetool`, pinned and checked against its SHA-256; it needs `desktop-file-validate` (`desktop-file-utils`), which checks the desktop entry.

## Releases

- **Release:** push a tag `vX.Y.Z` matching `Cargo.toml` (`v0.4.1` now). Only maintainers can create `v*` tags. The release becomes the latest; a version with a suffix (`-beta`) is published as a pre-release instead. There are no nightly builds.
- **Check a branch:** start the Release workflow by hand on any other branch (`gh workflow run release.yml --ref <branch>`): it builds every installer and keeps them as run artifacts, without publishing.

Each build job attests its files ([build provenance](https://docs.github.com/actions/security-for-github-actions/using-artifact-attestations)), and the release carries `SHA256SUMS`. Check a download with `gh attestation verify <file> -R LibreGuild/PilotHouse`.

## Updates

Each release also carries `latest.json` (written by `scripts/package/latest-json.py` in the publish job) and `PilotHouse-macOS-universal.zip` (the app, for macOS updates). `latest.json` names the version and, per kind of install, the file to download with its SHA-256 and size:

```json
{ "version": "0.3.0",
  "platforms": { "macos-universal": { "url": "https://github.com/LibreGuild/PilotHouse/releases/download/v0.3.0/PilotHouse-macOS-universal.zip", "sha256": "…", "size": 123 } } }
```

PilotHouse (`ph_workspace::updates`) reads `releases/latest/download/latest.json` at start and every six hours (*Settings → About → Updates* turns it off), downloads the file for how it was installed, refuses it unless it comes from this repository's release downloads and matches its SHA-256, unpacks it into `<data dir>/updates/<version>/`, and puts it in place when the app quits, or at once with **Restart to update** (title bar and Settings):

| Installed as | Detected by | Update |
|---|---|---|
| macOS `PilotHouse.app` | the program is inside an `.app` | the app is moved aside, the new one moved in (the old one comes back if that fails) |
| Windows installer | `Uninstall.exe` beside the program | the new installer runs silently (`/S`) |
| Windows portable | `LICENSE` beside the program, folder writable | the zip is unpacked over the folder |
| Linux AppImage | `$APPIMAGE` | the new AppImage replaces it |
| Linux tarball | `pilothouse.desktop` beside the program, folder writable | the new binary replaces it |
| `.deb`, a build from source | — | the title bar says a version is available and links to the release |

A small script (`sh`, PowerShell on Windows) waits for PilotHouse to quit, then replaces it and, after *Restart to update*, starts it again. Updates are not signed beyond the checksum: they are as trustworthy as the release page itself (HTTPS from GitHub, this repository only). Debug builds can be pointed at a test release with `PILOTHOUSE_UPDATE_MANIFEST` (and `PILOTHOUSE_UPDATE_TEST_BUNDLE` for a stand-in app).

## Signing

The builds are not code-signed: that needs a paid Apple Developer ID and a Windows code-signing certificate. Until then, macOS asks once (*Open Anyway* in Privacy & Security) and Windows SmartScreen may warn (*More info → Run anyway*); the release notes say so. The checksums and attestations are how a download is verified.

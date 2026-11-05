# Packaging and releases

Installers are built by `scripts/package/` and published by the **Release** workflow (`.github/workflows/release.yml`).

| System | Files | Script |
|---|---|---|
| macOS 11+ | `PilotHouse-macOS-universal.dmg` (Apple silicon and Intel in one app) | `scripts/package/macos.sh` (`--native` for this Mac only, quicker) |
| Windows 10/11 x64 | `PilotHouse-Windows-Setup-x64.exe` (NSIS, per user, no admin), `PilotHouse-Windows-Portable-x64.zip` | `scripts/package/windows.ps1` (needs NSIS) |
| Linux x86-64 | `PilotHouse-Linux-x86_64.AppImage`, `PilotHouse-Linux-amd64.deb`, `PilotHouse-Linux-x86_64.tar.gz` | `scripts/package/linux.sh` (build on Ubuntu 22.04 or older) |

Everything lands in `dist/` (git-ignored). The version comes from `Cargo.toml` (`scripts/package/version.sh`), now `0.6.0`. A pre-release version such as `1.0.0-beta` is shown as "1.0.0 beta"; where only numbers are allowed the suffix is dropped (the macOS bundle version and Windows file version read `1.0.0`), and the `.deb` is `1.0.0~beta`, which Debian sorts before `1.0.0`.

- **macOS:** the script builds both architectures, joins them with `lipo`, makes `PilotHouse.app` (`Info.plist` with the folder and text document types that Finder's *Open With* uses, the icon), signs it ad hoc and wraps it in a DMG with a shortcut to Applications.
- **Windows:** the icon is built into `pilothouse.exe` by `crates/app/build.rs` (resource 1, which GPUI shows for the window and the taskbar). The installer puts the app in `%LOCALAPPDATA%\Programs\PilotHouse`, adds a Start menu entry (a desktop shortcut is optional) and an uninstaller; uninstalling keeps your data.
- **Linux:** one desktop entry for all three packages (`Categories=Development;IDE;`, folders and text files). The AppImage is made with `appimagetool`, pinned and checked against its SHA-256; it needs `desktop-file-validate` (`desktop-file-utils`), which checks the desktop entry.

## Releases

- **Before a release:** run `scripts/update-prices.py` to refresh the model prices the usage dashboard ships with (`crates/workspace/src/usage_view/prices.json`) and commit the result.
- **Release:** push a tag `vX.Y.Z` matching `Cargo.toml` (`v0.6.0` now). Only maintainers can create `v*` tags. The release becomes the latest; a version with a suffix (`-beta`) is published as a pre-release instead. There are no nightly builds.
- **Check a branch:** start the Release workflow by hand on any other branch (`gh workflow run release.yml --ref <branch>`): it builds every installer and keeps them as run artifacts, without publishing.

Each build job attests its files ([build provenance](https://docs.github.com/actions/security-for-github-actions/using-artifact-attestations)), and the release carries `SHA256SUMS`. Check a download with `gh attestation verify <file> -R LibreGuild/PilotHouse`.

### Mirrors

Some networks block GitHub, so the **Mirror to R2** job copies every published release to the Cloudflare R2 bucket `libreguild`, served at two addresses:

| Address | |
|---|---|
| `https://libreguild.skatech.org/pilothouse/` | the bucket's custom domain (TLS 1.2 or newer) |
| `https://pub-6c540d377bd4404f87ff33fb14c18c21.r2.dev/pilothouse/` | the bucket's public development address, a backup (rate-limited by Cloudflare) |

Under each, `releases/<tag>/` keeps every release's files (cached for good: they never change), and `latest/` the newest release's (cached five minutes; `latest.json` not at all), which is what the site's mirror links and the updater read. A pre-release goes to `releases/<tag>/` only. The job downloads the release from GitHub, checks it against `SHA256SUMS`, uploads the files and `latest.json` last, then checks both addresses answer. A failed mirror run leaves the GitHub release as it is: re-run the job.

### The `release` environment

The `publish` and `mirror` jobs run in the repository's `release` environment, which only `v*` tags may use (a workflow started on a branch cannot read its secrets):

| Secret | What |
|---|---|
| `UPDATE_SIGNING_KEY` | the Ed25519 private key (PEM) that signs `latest.json`; its public half is `crates/workspace/src/update_key.pub` |
| `R2_ACCOUNT_ID` | the Cloudflare account that holds the bucket |
| `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY` | an R2 API token with *Object Read & Write* on the `libreguild` bucket only |

Keep a copy of the signing key outside GitHub (a password manager): every copy of PilotHouse checks updates against its public half, so a lost key means a new one has to ship in an app release before updates can be signed again, and copies older than that release would no longer update by themselves. To replace it: `openssl genpkey -algorithm ed25519 -out key.pem`, put its public half in `update_key.pub` (`openssl pkey -in key.pem -pubout -outform DER | tail -c 32 | base64`), and store the PEM as the secret. `scripts/package/sign-manifest.sh` refuses to sign with a key whose public half is not the one in the source.

## Updates

Each release also carries `latest.json` (written by `scripts/package/latest-json.py` in the publish job), its signature `latest.json.sig` (`scripts/package/sign-manifest.sh`, with the release key), and `PilotHouse-macOS-universal.zip` (the app, for macOS updates). `latest.json` names the version and, per kind of install, the file to download (by its GitHub address) with its SHA-256 and size:

```json
{ "version": "0.3.0",
  "platforms": { "macos-universal": { "url": "https://github.com/LibreGuild/PilotHouse/releases/download/v0.3.0/PilotHouse-macOS-universal.zip", "sha256": "…", "size": 123 } } }
```

PilotHouse (`ph_workspace::updates`) looks at start and every six hours (*Settings → About → Updates* turns it off). It tries its sources in order, GitHub (`releases/latest/download/latest.json`), the mirror, then the backup mirror, and uses the first whose `latest.json` carries the release key's Ed25519 signature. It downloads the file for how it was installed from that same source (the GitHub address's `<tag>/<name>`, under the mirror's `releases/`); should the download fail, it moves to the next source, reads its `latest.json` (signed, and naming the same version) and downloads from there. A file must match its SHA-256 before it is unpacked into `<data dir>/updates/<version>/`; it is put in place when the app quits, or at once with **Restart to update** (title bar and Settings):

| Installed as | Detected by | Update |
|---|---|---|
| macOS `PilotHouse.app` | the program is inside an `.app` | the app is moved aside, the new one moved in (the old one comes back if that fails) |
| Windows installer | `Uninstall.exe` beside the program | the new installer runs silently (`/S`) |
| Windows portable | `LICENSE` beside the program, folder writable | the zip is unpacked over the folder |
| Linux AppImage | `$APPIMAGE` | the new AppImage replaces it |
| Linux tarball | `pilothouse.desktop` beside the program, folder writable | the new binary replaces it |
| `.deb`, a build from source | — | the title bar says a version is available and links to the release |

A small script (`sh`, PowerShell on Windows) waits for PilotHouse to quit, then replaces it and, after *Restart to update*, starts it again. On Windows it first stops PilotHouse's helpers still running from the install folder (copies of `pilothouse.exe` started as `mcp`, `agy-acp`, `agent-hook` or `chrome-host`: an agent's MCP server, the Antigravity bridge, a Claude Code hook, the Chrome extension's host) and waits until none does, since Windows keeps a running program's file open and the installer would stop at *Abort, Retry, Ignore*. Another PilotHouse window you have open (a second data folder) is left alone; close it before updating. The manifest is trusted for its signature, not for where it came from: a mirror, or whoever controls one, can withhold an update but not change one. Debug builds can be pointed at a test release with `PILOTHOUSE_UPDATE_MANIFEST` (unsigned; and `PILOTHOUSE_UPDATE_TEST_BUNDLE` for a stand-in app).

## Signing

The builds are not code-signed: that needs a paid Apple Developer ID and a Windows code-signing certificate. Until then, macOS asks once (*Open Anyway* in Privacy & Security) and Windows SmartScreen may warn (*More info → Run anyway*); the release notes say so. The checksums and attestations are how a download is verified.

//! Updates from the project's GitHub releases.
//!
//! Each release carries `latest.json`: the version, and per kind of install
//! the file to download with its SHA-256. PilotHouse reads it (only that, and
//! only from this repository's releases, over HTTPS) at start and every six
//! hours, downloads the file for how it was installed, checks the SHA-256,
//! unpacks it beside the data folder, and puts it in place when the app
//! quits, or at once with *Restart to update*. Nothing is sent anywhere.
//!
//! | Installed as | Update |
//! |---|---|
//! | macOS `PilotHouse.app` | the release's app zip replaces the app |
//! | Windows installer | the release's installer runs silently |
//! | Windows portable folder | the release's zip is unpacked over it |
//! | Linux AppImage | the new AppImage replaces it |
//! | Linux tarball folder | the new binary replaces it |
//! | anything else (`.deb`, a build from source) | a link to the release |

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use gpui_kit::{App, AppContext as _, Global, SharedString, Task};
use serde::Deserialize;

const REPO: &str = "LibreGuild/PilotHouse";
const AUTO_CHECK: &str = "updates.auto_check";
const AUTO_INSTALL: &str = "updates.auto_install";
const EVERY: Duration = Duration::from_secs(6 * 60 * 60);
const FIRST_CHECK: Duration = Duration::from_secs(20);

/// What `latest.json` says.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Manifest {
    pub version: String,
    #[serde(default)]
    pub platforms: HashMap<String, Asset>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Asset {
    pub url: String,
    pub sha256: String,
    #[serde(default)]
    pub size: u64,
}

/// How this copy of PilotHouse was installed, so an update can replace it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Install {
    MacApp {
        bundle: PathBuf,
    },
    WindowsSetup {
        exe: PathBuf,
    },
    WindowsPortable {
        dir: PathBuf,
        exe: PathBuf,
    },
    LinuxAppImage {
        path: PathBuf,
    },
    LinuxTarball {
        dir: PathBuf,
    },
    /// Not one PilotHouse can update in place.
    Manual,
}

impl Install {
    /// This copy's install, from where its program is.
    pub(crate) fn detect() -> Self {
        // A test install, for trying updates from a debug build.
        #[cfg(debug_assertions)]
        if let Some(bundle) = std::env::var_os("PILOTHOUSE_UPDATE_TEST_BUNDLE") {
            return Install::MacApp {
                bundle: PathBuf::from(bundle),
            };
        }
        let Ok(exe) = std::env::current_exe() else {
            return Install::Manual;
        };
        Self::of(&exe, std::env::var_os("APPIMAGE").map(PathBuf::from))
    }

    fn of(exe: &Path, appimage: Option<PathBuf>) -> Self {
        let dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
        if cfg!(target_os = "macos") {
            // …/PilotHouse.app/Contents/MacOS/pilothouse
            let bundle = dir.parent().and_then(Path::parent);
            // Only an app it can move: its folder writable (not
            // /Applications for a standard user), and not run from the
            // read-only copy macOS makes of a quarantined download.
            return match bundle {
                Some(bundle)
                    if bundle.extension().is_some_and(|ext| ext == "app")
                        && !bundle.to_string_lossy().contains("/AppTranslocation/")
                        && bundle.parent().is_some_and(writable) =>
                {
                    Install::MacApp {
                        bundle: bundle.to_path_buf(),
                    }
                }
                _ => Install::Manual,
            };
        }
        if cfg!(windows) {
            if dir.join("Uninstall.exe").is_file() {
                return Install::WindowsSetup {
                    exe: exe.to_path_buf(),
                };
            }
            if dir.join("LICENSE").is_file() && writable(&dir) {
                return Install::WindowsPortable {
                    dir,
                    exe: exe.to_path_buf(),
                };
            }
            return Install::Manual;
        }
        if let Some(path) = appimage.filter(|path| path.is_file()) {
            return Install::LinuxAppImage { path };
        }
        // The tarball's folder: the binary with its desktop entry.
        if dir.join("pilothouse.desktop").is_file() && writable(&dir) {
            return Install::LinuxTarball { dir };
        }
        Install::Manual
    }

    /// Its file in `latest.json`.
    fn platform(&self) -> Option<&'static str> {
        match self {
            Install::MacApp { .. } => Some("macos-universal"),
            Install::WindowsSetup { .. } => Some("windows-x64-setup"),
            Install::WindowsPortable { .. } => Some("windows-x64-portable"),
            Install::LinuxAppImage { .. } => Some("linux-x86_64-appimage"),
            Install::LinuxTarball { .. } => Some("linux-x86_64-tarball"),
            Install::Manual => None,
        }
    }
}

fn writable(dir: &Path) -> bool {
    let probe = dir.join(".pilothouse-write-test");
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Whether version `candidate` is newer than `current` (`1.2.3`, with an
/// optional `-suffix` that sorts before the plain version; suffixes compare
/// part by part, numbers as numbers: `beta.10` is after `beta.9`).
pub(crate) fn is_newer(candidate: &str, current: &str) -> bool {
    fn suffix_order(new: &str, old: &str) -> std::cmp::Ordering {
        let mut new_parts = new.split('.');
        let mut old_parts = old.split('.');
        loop {
            match (new_parts.next(), old_parts.next()) {
                (None, None) => return std::cmp::Ordering::Equal,
                (None, Some(_)) => return std::cmp::Ordering::Less,
                (Some(_), None) => return std::cmp::Ordering::Greater,
                (Some(a), Some(b)) => {
                    let order = match (a.parse::<u64>(), b.parse::<u64>()) {
                        (Ok(a), Ok(b)) => a.cmp(&b),
                        (Ok(_), Err(_)) => std::cmp::Ordering::Less,
                        (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
                        (Err(_), Err(_)) => a.cmp(b),
                    };
                    if order != std::cmp::Ordering::Equal {
                        return order;
                    }
                }
            }
        }
    }
    fn parts(version: &str) -> (Vec<u64>, Option<String>) {
        let version = version.trim().trim_start_matches('v');
        let (numbers, suffix) = match version.split_once('-') {
            Some((numbers, suffix)) => (numbers, Some(suffix.to_string())),
            None => (version, None),
        };
        let numbers = numbers
            .split('.')
            .map(|part| part.parse().unwrap_or(0))
            .collect();
        (numbers, suffix)
    }
    let (new, new_suffix) = parts(candidate);
    let (old, old_suffix) = parts(current);
    let width = new.len().max(old.len());
    let pad = |mut numbers: Vec<u64>| {
        numbers.resize(width, 0);
        numbers
    };
    match pad(new).cmp(&pad(old)) {
        std::cmp::Ordering::Equal => match (new_suffix, old_suffix) {
            (None, Some(_)) => true,
            (Some(new), Some(old)) => suffix_order(&new, &old) == std::cmp::Ordering::Greater,
            _ => false,
        },
        order => order == std::cmp::Ordering::Greater,
    }
}

/// Where updates come from: this repository's releases (a debug build may
/// point elsewhere to be tried out).
fn manifest_url() -> String {
    #[cfg(debug_assertions)]
    if let Ok(url) = std::env::var("PILOTHOUSE_UPDATE_MANIFEST") {
        return url;
    }
    format!("https://github.com/{REPO}/releases/latest/download/latest.json")
}

/// Whether `url` is a file of this repository's releases (no `..` or
/// escapes that could lead out of them).
fn trusted(url: &str) -> bool {
    #[cfg(debug_assertions)]
    if std::env::var("PILOTHOUSE_UPDATE_MANIFEST").is_ok() {
        return true;
    }
    url.strip_prefix(&format!("https://github.com/{REPO}/releases/download/"))
        .is_some_and(|rest| {
            !rest
                .split('/')
                .any(|part| part.is_empty() || part.starts_with('.'))
                && rest
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+' | '/'))
        })
}

/// Whether `version` is a plain version (`1.2.3`, `1.2.3-beta.1`): it names
/// a folder, so nothing that could lead elsewhere.
fn plain_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= 32
        && !version.starts_with('.')
        && !version.contains("..")
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
}

fn agent() -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
    let tls = TlsConfig::builder()
        .provider(TlsProvider::NativeTls)
        .root_certs(RootCerts::PlatformVerifier)
        .build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .timeout_connect(Some(Duration::from_secs(20)))
        .user_agent(concat!("PilotHouse/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// Reads `latest.json`. Blocking.
fn fetch_manifest() -> Result<Manifest> {
    let mut response = agent()
        .get(&manifest_url())
        .call()
        .context("could not reach GitHub")?;
    let text = response
        .body_mut()
        .with_config()
        .limit(1 << 20)
        .read_to_string()?;
    let manifest: Manifest =
        serde_json::from_str(&text).context("the release's latest.json could not be read")?;
    if !plain_version(&manifest.version) {
        bail!("the release's latest.json names no plain version");
    }
    Ok(manifest)
}

/// Where downloaded updates are kept until they are installed.
fn updates_dir() -> PathBuf {
    ph_store::paths::data_dir().join("updates")
}

/// Downloads `asset` into `folder`, counting bytes in `done`, and checks
/// its SHA-256. Blocking.
fn download(asset: &Asset, folder: &Path, done: &AtomicU64) -> Result<PathBuf> {
    use sha2::Digest as _;
    use std::io::{Read as _, Write as _};
    if !trusted(&asset.url) {
        bail!("the update is not from {REPO}'s releases");
    }
    let name = asset
        .url
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty() && !name.contains(['\\', '/']) && !name.starts_with('.'))
        .context("the update has no file name")?;
    std::fs::create_dir_all(folder)?;
    let path = folder.join(name);
    let partial = folder.join(format!("{name}.part"));
    // A download that breaks off leaves nothing behind.
    let fetched = (|| -> Result<sha2::Sha256> {
        let mut response = agent().get(&asset.url).call()?;
        let mut reader = response.body_mut().with_config().limit(2 << 30).reader();
        let mut file = std::fs::File::create(&partial)?;
        let mut hasher = sha2::Sha256::new();
        let mut buffer = vec![0; 256 * 1024];
        let mut total = 0u64;
        loop {
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            total += read as u64;
            if asset.size > 0 && total > asset.size {
                bail!("the download is larger than the release says");
            }
            hasher.update(&buffer[..read]);
            file.write_all(&buffer[..read])?;
            done.fetch_add(read as u64, Ordering::Relaxed);
        }
        file.flush()?;
        Ok(hasher)
    })();
    let hasher = match fetched {
        Ok(hasher) => hasher,
        Err(err) => {
            let _ = std::fs::remove_file(&partial);
            return Err(err);
        }
    };
    let digest = hasher.finalize();
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    if !hex.eq_ignore_ascii_case(asset.sha256.trim()) {
        let _ = std::fs::remove_file(&partial);
        bail!("the download does not match its checksum");
    }
    std::fs::rename(&partial, &path)?;
    Ok(path)
}

/// Unpacks a downloaded update into `staging`, ready to put in place;
/// returns what [`apply`] installs. Blocking.
fn stage(install: &Install, file: &Path, staging: &Path) -> Result<PathBuf> {
    let _ = std::fs::remove_dir_all(staging);
    std::fs::create_dir_all(staging)?;
    let run = |program: &str, args: &[&std::ffi::OsStr]| -> Result<()> {
        let status = std::process::Command::new(program).args(args).status()?;
        if !status.success() {
            bail!("{program} could not unpack the update");
        }
        Ok(())
    };
    match install {
        Install::MacApp { .. } => {
            run(
                "ditto",
                &[
                    "-x".as_ref(),
                    "-k".as_ref(),
                    file.as_os_str(),
                    staging.as_os_str(),
                ],
            )?;
            let app = staging.join("PilotHouse.app");
            if !app.join("Contents/MacOS/pilothouse").is_file() {
                bail!("the update holds no PilotHouse.app");
            }
            Ok(app)
        }
        Install::WindowsSetup { .. } => Ok(file.to_path_buf()),
        Install::WindowsPortable { .. } => {
            run(
                "tar",
                &[
                    "-xf".as_ref(),
                    file.as_os_str(),
                    "-C".as_ref(),
                    staging.as_os_str(),
                ],
            )?;
            if !staging.join("pilothouse.exe").is_file() {
                bail!("the update holds no pilothouse.exe");
            }
            Ok(staging.to_path_buf())
        }
        Install::LinuxAppImage { .. } => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755))?;
            }
            Ok(file.to_path_buf())
        }
        Install::LinuxTarball { .. } => {
            run(
                "tar",
                &[
                    "-xzf".as_ref(),
                    file.as_os_str(),
                    "-C".as_ref(),
                    staging.as_os_str(),
                ],
            )?;
            let binary = staging.join("pilothouse").join("pilothouse");
            if !binary.is_file() {
                bail!("the update holds no pilothouse binary");
            }
            Ok(binary)
        }
        Install::Manual => bail!("this copy of PilotHouse cannot update itself"),
    }
}

/// The script that puts `staged` in place once PilotHouse (process `pid`)
/// has quit, and starts it again when `relaunch`.
pub(crate) fn apply_script(
    install: &Install,
    staged: &Path,
    pid: u32,
    relaunch: bool,
) -> Option<String> {
    let quote = |path: &Path| format!("'{}'", path.display().to_string().replace('\'', "'\\''"));
    let wait = format!("while kill -0 {pid} 2>/dev/null; do sleep 0.2; done");
    match install {
        Install::MacApp { bundle } => {
            let (old, new) = (bundle.with_extension("app.old"), quote(staged));
            // The old app steps aside; put back if the new one cannot move in.
            Some(format!(
                "{wait}\nrm -rf {old}\nif mv {app} {old}; then if mv {new} {app}; then rm -rf {old}; else mv {old} {app}; fi; fi\n{launch}\n",
                old = quote(&old),
                app = quote(bundle),
                launch = if relaunch {
                    format!("open {}", quote(bundle))
                } else {
                    String::new()
                },
            ))
        }
        Install::LinuxAppImage { path } => {
            let next = path.with_extension("AppImage.new");
            Some(format!(
                "{wait}\ncp {new} {next} && chmod +x {next} && mv {next} {path}\n{launch}\n",
                new = quote(staged),
                next = quote(&next),
                path = quote(path),
                launch = if relaunch {
                    format!("{} >/dev/null 2>&1 &", quote(path))
                } else {
                    String::new()
                },
            ))
        }
        Install::LinuxTarball { dir } => {
            let binary = dir.join("pilothouse");
            let next = dir.join("pilothouse.new");
            Some(format!(
                "{wait}\ncp {new} {next} && chmod +x {next} && mv {next} {binary}\n{launch}\n",
                new = quote(staged),
                next = quote(&next),
                binary = quote(&binary),
                launch = if relaunch {
                    format!("{} >/dev/null 2>&1 &", quote(&binary))
                } else {
                    String::new()
                },
            ))
        }
        Install::WindowsSetup { exe } => Some(format!(
            "Wait-Process -Id {pid} -ErrorAction SilentlyContinue; Start-Process -FilePath {setup} -ArgumentList '/S' -Wait; {launch}",
            setup = ps_quote(staged),
            launch = if relaunch {
                format!("Start-Process -FilePath {}", ps_quote(exe))
            } else {
                String::new()
            },
        )),
        Install::WindowsPortable { dir, exe } => Some(format!(
            "Wait-Process -Id {pid} -ErrorAction SilentlyContinue; Copy-Item -Path (Join-Path {staged} '*') -Destination {dir} -Recurse -Force; {launch}",
            staged = ps_quote(staged),
            dir = ps_quote(dir),
            launch = if relaunch {
                format!("Start-Process -FilePath {}", ps_quote(exe))
            } else {
                String::new()
            },
        )),
        Install::Manual => None,
    }
}

fn ps_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "''"))
}

/// Starts the script that installs `staged` after PilotHouse quits.
fn apply(install: &Install, staged: &Path, relaunch: bool) -> Result<()> {
    let script = apply_script(install, staged, std::process::id(), relaunch)
        .context("this copy of PilotHouse cannot update itself")?;
    let mut command = if cfg!(windows) {
        let mut command = std::process::Command::new("powershell");
        command.args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &script]);
        command
    } else {
        let mut command = std::process::Command::new("/bin/sh");
        command.args(["-c", &script]);
        command
    };
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    command.spawn().context("could not start the update")?;
    Ok(())
}

/// Where an update stands.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Status {
    Idle,
    Checking,
    UpToDate,
    /// Newer, waiting for the user to download it.
    Available {
        version: String,
    },
    Downloading {
        version: String,
    },
    /// Downloaded and unpacked: installs on quit or *Restart to update*.
    Ready {
        version: String,
        staged: PathBuf,
    },
    /// Newer, but this copy is not one PilotHouse updates itself.
    Manual {
        version: String,
    },
    Failed(SharedString),
}

pub(crate) struct Updater {
    pub status: Status,
    install: Install,
    manifest: Option<Manifest>,
    /// Bytes downloaded so far, and the total.
    pub progress: (Arc<AtomicU64>, u64),
    _task: Option<Task<()>>,
    _schedule: Option<Task<()>>,
}

impl Global for Updater {}

/// The page of the newest release.
pub(crate) const RELEASES_URL: &str = "https://github.com/LibreGuild/PilotHouse/releases/latest";

pub(crate) fn auto_check(cx: &App) -> bool {
    crate::AppState::store(cx)
        .setting::<Option<bool>>(AUTO_CHECK)
        .unwrap_or(true)
}

pub(crate) fn auto_install(cx: &App) -> bool {
    crate::AppState::store(cx)
        .setting::<Option<bool>>(AUTO_INSTALL)
        .unwrap_or(true)
}

pub(crate) fn set_auto_check(value: bool, cx: &mut App) {
    let _ = crate::AppState::store(cx).set_setting(AUTO_CHECK, &Some(value));
}

pub(crate) fn set_auto_install(value: bool, cx: &mut App) {
    let _ = crate::AppState::store(cx).set_setting(AUTO_INSTALL, &Some(value));
}

impl Updater {
    pub(crate) fn status(cx: &App) -> Status {
        cx.try_global::<Self>()
            .map(|updater| updater.status.clone())
            .unwrap_or(Status::Idle)
    }

    /// Bytes downloaded and the total, while downloading.
    pub(crate) fn progress(cx: &App) -> Option<(u64, u64)> {
        let updater = cx.try_global::<Self>()?;
        let (done, total) = &updater.progress;
        Some((done.load(Ordering::Relaxed), *total))
    }

    fn set(status: Status, cx: &mut App) {
        cx.global_mut::<Self>().status = status;
        cx.refresh_windows();
    }
}

/// Starts checking, at start and every six hours while allowed; an update
/// downloaded installs when the app quits.
pub(crate) fn install(cx: &mut App) {
    if cx.has_global::<Updater>() {
        return;
    }
    let schedule = cx.spawn(async move |cx| {
        cx.background_executor().timer(FIRST_CHECK).await;
        // Downloads of earlier runs were installed, or are out of date (by
        // now an install script of the last run is long done with them).
        cx.background_spawn(async {
            let _ = std::fs::remove_dir_all(updates_dir());
        })
        .await;
        loop {
            cx.update(|cx| {
                if auto_check(cx) {
                    check(false, cx);
                }
            });
            cx.background_executor().timer(EVERY).await;
        }
    });
    cx.set_global(Updater {
        status: Status::Idle,
        install: Install::detect(),
        manifest: None,
        progress: (Arc::new(AtomicU64::new(0)), 0),
        _task: None,
        _schedule: Some(schedule),
    });
    cx.on_app_quit(|cx| {
        if let Some(updater) = cx.try_global::<Updater>()
            && let Status::Ready { staged, .. } = &updater.status
            && let Err(err) = apply(&updater.install, staged, false)
        {
            tracing::warn!("installing the update failed: {err:#}");
        }
        async {}
    })
    .detach();
}

/// Looks for an update now (`manual`: the user asked, so even when checks
/// are off, and the answer shows).
pub(crate) fn check(manual: bool, cx: &mut App) {
    let Some(updater) = cx.try_global::<Updater>() else {
        return;
    };
    if matches!(
        updater.status,
        Status::Checking | Status::Downloading { .. } | Status::Ready { .. }
    ) {
        return;
    }
    Updater::set(Status::Checking, cx);
    let fetch = cx.background_spawn(async move { fetch_manifest() });
    let task = cx.spawn(async move |cx| {
        let result = fetch.await;
        cx.update(|cx| match result {
            Ok(manifest) if is_newer(&manifest.version, env!("CARGO_PKG_VERSION")) => {
                let version = manifest.version.clone();
                let install = cx.global::<Updater>().install.clone();
                let has_file = install
                    .platform()
                    .is_some_and(|platform| manifest.platforms.contains_key(platform));
                cx.global_mut::<Updater>().manifest = Some(manifest);
                if !has_file {
                    Updater::set(Status::Manual { version }, cx);
                } else if auto_install(cx) || manual {
                    download_update(cx);
                } else {
                    Updater::set(Status::Available { version }, cx);
                }
            }
            Ok(_) => Updater::set(Status::UpToDate, cx),
            Err(err) => {
                tracing::info!("update check failed: {err:#}");
                Updater::set(
                    if manual {
                        Status::Failed(format!("{err:#}").into())
                    } else {
                        Status::Idle
                    },
                    cx,
                )
            }
        });
    });
    cx.global_mut::<Updater>()._task = Some(task);
}

/// Downloads, checks and unpacks the update `latest.json` named.
pub(crate) fn download_update(cx: &mut App) {
    let updater = cx.global::<Updater>();
    let (Some(manifest), Some(platform)) = (updater.manifest.clone(), updater.install.platform())
    else {
        return;
    };
    let Some(asset) = manifest.platforms.get(platform).cloned() else {
        return;
    };
    let install = updater.install.clone();
    let version = manifest.version.clone();
    let size = asset.size;
    let done = Arc::new(AtomicU64::new(0));
    let folder = updates_dir().join(&version);
    let work = {
        let done = done.clone();
        cx.background_spawn(async move {
            let file = download(&asset, &folder, &done)?;
            stage(&install, &file, &folder.join("staged"))
        })
    };
    {
        let updater = cx.global_mut::<Updater>();
        updater.progress = (done, size);
    }
    Updater::set(
        Status::Downloading {
            version: version.clone(),
        },
        cx,
    );
    // Redraws the progress while it downloads.
    let ticker = cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            let downloading = cx.update(|cx| {
                cx.refresh_windows();
                matches!(Updater::status(cx), Status::Downloading { .. })
            });
            if !downloading {
                return;
            }
        }
    });
    let task = cx.spawn(async move |cx| {
        let result = work.await;
        drop(ticker);
        cx.update(|cx| match result {
            Ok(staged) => Updater::set(Status::Ready { version, staged }, cx),
            Err(err) => Updater::set(Status::Failed(format!("{err:#}").into()), cx),
        });
    });
    cx.global_mut::<Updater>()._task = Some(task);
}

/// Installs the update now: PilotHouse quits, is replaced, and starts again.
pub(crate) fn restart_to_update(cx: &mut App) {
    let Some(updater) = cx.try_global::<Updater>() else {
        return;
    };
    let Status::Ready { staged, .. } = updater.status.clone() else {
        return;
    };
    match apply(&updater.install, &staged, true) {
        Ok(()) => {
            // Installed by the script now, not again on quit.
            Updater::set(Status::Idle, cx);
            cx.quit();
        }
        Err(err) => Updater::set(Status::Failed(format!("{err:#}").into()), cx),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.3.0", "0.2.0"));
        assert!(is_newer("v0.2.1", "0.2.0"));
        assert!(is_newer("1.0.0", "1.0.0-beta"));
        assert!(!is_newer("1.0.0-beta", "1.0.0"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(is_newer("1.0.0-beta.10", "1.0.0-beta.9"));
        assert!(!is_newer("1.0.0-beta.9", "1.0.0-beta.10"));
        assert!(is_newer("1.0.0-rc.1", "1.0.0-beta.2"));
    }

    #[test]
    fn reads_the_manifest() {
        let manifest: Manifest = serde_json::from_str(
            r#"{"version":"0.3.0","notes":"n","platforms":{"macos-universal":{"url":"https://github.com/LibreGuild/PilotHouse/releases/download/v0.3.0/PilotHouse-macOS-universal.zip","sha256":"ab","size":3}}}"#,
        )
        .unwrap();
        assert_eq!(manifest.platforms["macos-universal"].size, 3);
        assert!(trusted(&manifest.platforms["macos-universal"].url));
        assert!(!trusted("https://example.com/PilotHouse.zip"));
        assert!(!trusted(
            "https://github.com/LibreGuild/PilotHouse/releases/download/../../../evil/x/releases/download/v1/a.zip"
        ));
        assert!(!trusted(
            "https://github.com/LibreGuild/PilotHouse/releases/download/v1/%2e%2e/a.zip"
        ));
        assert!(plain_version("0.3.0-beta.1"));
        assert!(!plain_version("../../x"));
        assert!(!plain_version("0.3/0"));
    }

    #[test]
    fn knows_how_it_was_installed() {
        if cfg!(target_os = "macos") {
            // An app in a folder it can write to.
            let folder = std::env::temp_dir();
            let bundle = folder.join("PilotHouse.app");
            assert_eq!(
                Install::of(&bundle.join("Contents/MacOS/pilothouse"), None),
                Install::MacApp { bundle }
            );
            assert_eq!(
                Install::of(
                    Path::new(
                        "/private/var/folders/x/AppTranslocation/1/d/PilotHouse.app/Contents/MacOS/pilothouse"
                    ),
                    None
                ),
                Install::Manual,
                "macOS's read-only copy"
            );
            assert_eq!(
                Install::of(Path::new("/repo/target/debug/pilothouse"), None),
                Install::Manual
            );
        }
    }

    #[test]
    fn scripts_wait_then_replace() {
        let script = apply_script(
            &Install::MacApp {
                bundle: PathBuf::from("/Applications/PilotHouse.app"),
            },
            Path::new("/data/updates/0.3.0/staged/PilotHouse.app"),
            42,
            true,
        )
        .unwrap();
        assert!(script.starts_with("while kill -0 42"));
        assert!(
            script.contains("mv '/Applications/PilotHouse.app' '/Applications/PilotHouse.app.old'")
        );
        assert!(script.contains("open '/Applications/PilotHouse.app'"));
        assert!(apply_script(&Install::Manual, Path::new("/x"), 1, false).is_none());
    }
}

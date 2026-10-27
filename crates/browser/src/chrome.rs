//! Finding and starting Chrome.
//!
//! Each project gets its own profile folder in the data directory, so logins
//! and cookies persist per project and never mix with the user's own Chrome.
//! Chrome runs headless; PilotHouse draws its frames in the pane.
//!
//! PilotHouse talks to Chrome over a private pipe (`--remote-debugging-pipe`),
//! not a TCP port: a DevTools port on localhost could be used by any process
//! of any user on the machine to drive the logged-in browser and read its
//! cookies. The pipe also ends Chrome when PilotHouse exits, even on a crash.
//! A port is opened only when the user asks for one (for another DevTools
//! client), see [`Chrome::launch`].
//!
//! Any Chromium-family browser works. With none installed, the pane asks
//! the user to choose one ([`resolve_choice`]) or links to Chrome's download
//! page; PilotHouse never downloads a browser itself.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};

/// macOS app bundles, checked in order.
const MAC_APPS: &[&str] = &[
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
];
/// Windows installs, relative to `%ProgramFiles%`, `%ProgramFiles(x86)%` and
/// `%LOCALAPPDATA%` (per-user installs). Edge ships with every Windows.
const WINDOWS_APPS: &[&str] = &[
    r"Google\Chrome\Application\chrome.exe",
    r"Chromium\Application\chrome.exe",
    r"BraveSoftware\Brave-Browser\Application\brave.exe",
    r"Microsoft\Edge\Application\msedge.exe",
];
/// Linux (and anything else): programs on `PATH`.
const PROGRAMS: &[&str] = &[
    "google-chrome",
    "google-chrome-stable",
    "chromium",
    "chromium-browser",
    "microsoft-edge",
    "brave-browser",
];

/// The setting holding the browser the user chose (a path).
pub const CHOSEN_SETTING: &str = "browser.chrome_path";

/// Where Chrome can be downloaded, for the pane's link.
pub const DOWNLOAD_PAGE: &str = "https://www.google.com/chrome/";

/// The browser the user chose (`chosen`, if it is still there), else
/// `$PILOTHOUSE_CHROME`, else the first Chromium-family browser installed.
pub fn find_chrome(chosen: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = chosen.filter(|path| path.is_file()) {
        return Some(path.to_path_buf());
    }
    if let Some(path) = std::env::var_os("PILOTHOUSE_CHROME").map(PathBuf::from) {
        return path.exists().then_some(path);
    }
    if cfg!(target_os = "macos")
        && let Some(app) = MAC_APPS
            .iter()
            .map(PathBuf::from)
            .find(|path| path.exists())
    {
        return Some(app);
    }
    if cfg!(windows) {
        let roots: Vec<PathBuf> = ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"]
            .into_iter()
            .filter_map(|var| ph_env::var(var).map(PathBuf::from))
            .collect();
        // Chrome first wherever it is installed, then the others.
        for app in WINDOWS_APPS {
            if let Some(path) = roots
                .iter()
                .map(|root| root.join(app))
                .find(|path| path.exists())
            {
                return Some(path);
            }
        }
    }
    PROGRAMS.iter().find_map(|program| ph_env::which(program))
}

/// The program to run for a browser the user picked: the file itself, or the
/// executable inside a macOS `.app` bundle. `None` if it is neither.
pub fn resolve_choice(picked: &Path) -> Option<PathBuf> {
    if picked.is_file() {
        return Some(picked.to_path_buf());
    }
    if picked.extension().is_some_and(|ext| ext == "app") {
        let inside = picked.join("Contents/MacOS");
        // The executable is named after the app; fall back to the only file.
        let named = picked
            .file_stem()
            .map(|stem| inside.join(stem))
            .filter(|path| path.is_file());
        return named.or_else(|| {
            let files: Vec<PathBuf> = std::fs::read_dir(&inside)
                .ok()?
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.is_file())
                .collect();
            (files.len() == 1).then(|| files[0].clone())
        });
    }
    None
}

pub fn profile_dir(project_id: i64) -> PathBuf {
    ph_store::paths::data_dir()
        .join("browser")
        .join(project_id.to_string())
}

/// A running Chrome and the two ends of its DevTools pipe.
pub struct Chrome {
    child: Child,
    /// Chrome's messages to us.
    pub reader: Option<std::io::PipeReader>,
    /// Our messages to Chrome.
    pub writer: Option<std::io::PipeWriter>,
    /// `http://127.0.0.1:<port>` when a port was asked for.
    pub devtools: Option<String>,
}

impl Drop for Chrome {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Chrome {
    /// Starts Chrome on `profile`. With `open_port`, Chrome also listens on a
    /// random localhost port so other DevTools clients can attach.
    pub fn launch(binary: &Path, profile: &Path, open_port: bool) -> Result<Self> {
        ph_store::paths::ensure_private_dir(profile)?;
        stop_leftover(profile);
        let port_file = profile.join("DevToolsActivePort");
        let _ = std::fs::remove_file(&port_file);

        let (chrome_reads, writer) = std::io::pipe()?;
        let (reader, chrome_writes) = std::io::pipe()?;
        let mut command = Command::new(binary);
        command
            .arg("--headless=new")
            .arg("--remote-debugging-pipe")
            .arg(format!("--user-data-dir={}", profile.display()))
            .args([
                "--no-first-run",
                "--no-default-browser-check",
                // Keep it on-device: no sync, updates, metrics or Google services.
                "--disable-background-networking",
                "--disable-component-update",
                "--disable-sync",
                "--disable-domain-reliability",
                "--disable-features=Translate,OptimizationHints,MediaRouter",
                "--no-pings",
                // No macOS keychain prompts from a headless browser. The
                // profile folder is private to the user instead.
                "--use-mock-keychain",
                "--password-store=basic",
                "--window-size=1280,800",
                // Pages would otherwise see `navigator.webdriver` (Chrome
                // sets it for any browser driven over DevTools), which bot
                // checks look at first.
                "--disable-blink-features=AutomationControlled",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if open_port {
            command.arg("--remote-debugging-port=0");
        }
        command.arg("about:blank");
        give_pipe(&mut command, &chrome_reads, &chrome_writes)?;
        let child = command
            .spawn()
            .with_context(|| format!("starting {}", binary.display()))?;
        // Chrome holds its ends now; ours must close so EOF reaches the reader.
        drop((chrome_reads, chrome_writes));

        let mut chrome = Self {
            child,
            reader: Some(reader),
            writer: Some(writer),
            devtools: None,
        };
        if open_port {
            chrome.devtools = Some(wait_for_port(&port_file)?);
        }
        Ok(chrome)
    }
}

/// Makes Chrome's pipe ends its file descriptors 3 (it reads) and 4 (it
/// writes), as `--remote-debugging-pipe` expects.
#[cfg(unix)]
fn give_pipe(
    command: &mut Command,
    reads: &std::io::PipeReader,
    writes: &std::io::PipeWriter,
) -> Result<()> {
    use std::os::fd::AsRawFd as _;
    use std::os::unix::process::CommandExt as _;
    let (reads, writes) = (reads.as_raw_fd(), writes.as_raw_fd());
    // SAFETY: runs in the child between fork and exec, and only calls the
    // async-signal-safe `fcntl` and `dup2`.
    unsafe {
        command.pre_exec(move || {
            // Copy both above 4 first, so neither is overwritten by the
            // other's move (pipes can already sit on 3 or 4). `dup2` also
            // clears close-on-exec on the copies Chrome keeps.
            let high_reads = libc::fcntl(reads, libc::F_DUPFD, 10);
            let high_writes = libc::fcntl(writes, libc::F_DUPFD, 10);
            if high_reads < 0
                || high_writes < 0
                || libc::dup2(high_reads, 3) < 0
                || libc::dup2(high_writes, 4) < 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(())
}

/// On Windows, Chrome takes the pipe as two inheritable handles named on
/// its command line. They are marked inheritable only for this launch, and
/// ours are closed right after it, so EOF still reaches Chrome when
/// PilotHouse exits. (A child spawned by another thread in that instant could
/// inherit them too; it would only delay that EOF until it exits.)
#[cfg(windows)]
fn give_pipe(
    command: &mut Command,
    reads: &std::io::PipeReader,
    writes: &std::io::PipeWriter,
) -> Result<()> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
    for handle in [reads.as_raw_handle(), writes.as_raw_handle()] {
        // SAFETY: both handles are valid pipe ends owned by the caller.
        if unsafe { SetHandleInformation(handle as _, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) }
            == 0
        {
            return Err(std::io::Error::last_os_error()).context("preparing Chrome's pipe");
        }
    }
    command.arg(format!(
        "--remote-debugging-io-pipes={},{}",
        reads.as_raw_handle() as usize,
        writes.as_raw_handle() as usize
    ));
    Ok(())
}

/// Chrome writes its debugging port on the first line of this file.
fn wait_for_port(port_file: &Path) -> Result<String> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(text) = std::fs::read_to_string(port_file)
            && let Ok(port) = text
                .lines()
                .next()
                .unwrap_or_default()
                .trim()
                .parse::<u16>()
        {
            return Ok(format!("http://127.0.0.1:{port}"));
        }
        if Instant::now() > deadline {
            bail!("Chrome did not open its DevTools port within 15 seconds");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Stops a Chrome still holding this profile (one from an older PilotHouse
/// that ran with a port and outlived it). Otherwise the new Chrome would hand
/// over to the old one and exit at once. Windows keeps no `SingletonLock`
/// symlink, and Chrome there exits with its pipe, so nothing is needed.
#[cfg(not(unix))]
fn stop_leftover(_: &Path) {}

/// `command` (a process's command line) runs on `profile` exactly.
#[cfg(unix)]
fn on_profile(command: &str, profile: &Path) -> bool {
    let option = format!("--user-data-dir={}", profile.display());
    command.match_indices(&option).any(|(at, _)| {
        command[at + option.len()..]
            .chars()
            .next()
            .is_none_or(char::is_whitespace)
    })
}

#[cfg(unix)]
fn stop_leftover(profile: &Path) {
    // `SingletonLock` is a symlink to `<host>-<pid>`.
    let Ok(target) = std::fs::read_link(profile.join("SingletonLock")) else {
        return;
    };
    let Some(pid) = target
        .to_string_lossy()
        .rsplit('-')
        .next()
        .and_then(|pid| pid.parse::<u32>().ok())
    else {
        return;
    };
    let Ok(output) = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
    else {
        return;
    };
    // Only ever a Chrome on this very profile (not `…/browser/12` for
    // `…/browser/1`).
    let command = String::from_utf8_lossy(&output.stdout);
    if !on_profile(&command, profile) {
        return;
    }
    tracing::info!(
        "stopping a leftover Chrome (pid {pid}) on {}",
        profile.display()
    );
    let _ = Command::new("kill").arg(pid.to_string()).status();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && profile.join("SingletonLock").symlink_metadata().is_ok() {
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {

    #[cfg(unix)]
    #[test]
    fn a_leftover_chrome_is_matched_by_its_whole_profile() {
        let profile = Path::new("/data/browser/1");
        assert!(on_profile(
            "chrome --user-data-dir=/data/browser/1 --headless",
            profile
        ));
        assert!(on_profile(
            "chrome --user-data-dir=/data/browser/1",
            profile
        ));
        assert!(!on_profile(
            "chrome --user-data-dir=/data/browser/12 --x",
            profile
        ));
    }
    use super::*;

    #[test]
    fn a_picked_app_bundle_runs_its_executable() {
        let root = std::env::temp_dir().join(format!("ph-chrome-{}", std::process::id()));
        let app = root.join("Fancy Browser.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::write(app.join("Contents/MacOS/Fancy Browser"), "").unwrap();
        assert_eq!(
            resolve_choice(&app),
            Some(app.join("Contents/MacOS/Fancy Browser"))
        );
        let binary = app.join("Contents/MacOS/Fancy Browser");
        assert_eq!(resolve_choice(&binary), Some(binary.clone()));
        assert_eq!(
            resolve_choice(&root),
            None,
            "a plain folder is not a browser"
        );
        assert_eq!(
            find_chrome(Some(&binary)),
            Some(binary),
            "the choice comes first"
        );
    }
}

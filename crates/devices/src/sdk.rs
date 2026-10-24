//! Finding the user's Android SDK tools. PilotHouse never installs them.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct Sdk {
    pub root: Option<PathBuf>,
    pub adb: Option<PathBuf>,
    pub emulator: Option<PathBuf>,
}

impl Sdk {
    /// Looks in `$ANDROID_HOME`, `$ANDROID_SDK_ROOT`, the default install
    /// folders, then the login shell's `PATH`.
    pub fn locate() -> Self {
        let home = dirs_home();
        let candidates = [
            ph_env::var("ANDROID_HOME").map(PathBuf::from),
            ph_env::var("ANDROID_SDK_ROOT").map(PathBuf::from),
            std::env::var_os("ANDROID_HOME").map(PathBuf::from),
            home.as_ref().map(|home| home.join("Library/Android/sdk")),
            home.as_ref().map(|home| home.join("Android/Sdk")),
            std::env::var_os("LOCALAPPDATA").map(|dir| PathBuf::from(dir).join("Android/Sdk")),
        ];
        let root = candidates
            .into_iter()
            .flatten()
            .find(|dir| dir.join("platform-tools").is_dir());
        let tool = |sub: &str, name: &str| {
            root.as_ref()
                .map(|root| root.join(sub).join(exe(name)))
                .filter(|path| path.exists())
                .or_else(|| ph_env::which(name))
        };
        Self {
            adb: tool("platform-tools", "adb"),
            emulator: tool("emulator", "emulator"),
            root,
        }
    }

    /// What to tell the user when adb is missing.
    pub fn missing_message() -> &'static str {
        "Android platform tools were not found. Install Android Studio (or the SDK command-line tools), \
         or set ANDROID_HOME to your SDK folder, then reopen this pane."
    }
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// True if `path` looks like an installed APK.
pub fn is_apk(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("apk"))
        && path.is_file()
}

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

/// The JDK bundled with Android Studio, which Studio builds with by default.
/// The JDK on `PATH` may be too new for the project's Gradle or Kotlin
/// (Gradle fails with just "25" on Java 25), so builds use this one too.
pub fn studio_jdk() -> Option<PathBuf> {
    let home = dirs_home();
    let program_files = std::env::var_os("ProgramFiles").map(PathBuf::from);
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let candidates = [
        Some(PathBuf::from(
            "/Applications/Android Studio.app/Contents/jbr/Contents/Home",
        )),
        home.as_ref()
            .map(|home| home.join("Applications/Android Studio.app/Contents/jbr/Contents/Home")),
        program_files.map(|dir| dir.join("Android/Android Studio/jbr")),
        local.map(|dir| dir.join("Programs/Android Studio/jbr")),
        Some(PathBuf::from("/opt/android-studio/jbr")),
        home.as_ref().map(|home| home.join("android-studio/jbr")),
        Some(PathBuf::from("/snap/android-studio/current/jbr")),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|jdk| jdk.join("bin").join(exe("java")).is_file())
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

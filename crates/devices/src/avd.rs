//! Making, wiping and deleting emulators (AVDs) without the SDK's
//! command-line tools, which many installs lack.
//!
//! An AVD is two things in the AVD folder (`~/.android/avd`): a `<name>.ini`
//! that points at a `<name>.avd` folder holding `config.ini` (the hardware
//! and the system image) and, once it has run, the emulator's disks. That is
//! what `avdmanager` writes too, so the emulator and Android Studio treat
//! these AVDs like their own.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

/// An installed system image an emulator can run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemImage {
    pub api: u32,
    /// `google_apis_playstore`, `google_apis`, `default`…
    pub tag: String,
    /// `Google Play`, `Google APIs`…
    pub tag_display: String,
    /// `arm64-v8a`, `x86_64`…
    pub abi: String,
    /// The image folder, relative to the SDK, ending in `/`
    /// (`system-images/android-35/google_apis/arm64-v8a/`).
    pub sysdir: String,
}

impl SystemImage {
    pub fn label(&self) -> String {
        format!("API {} · {} · {}", self.api, self.tag_display, self.abi)
    }
}

/// The screen and memory of a new emulator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Phone,
    Tablet,
}

impl Profile {
    pub const ALL: [Profile; 2] = [Profile::Phone, Profile::Tablet];

    pub fn label(self) -> &'static str {
        match self {
            Profile::Phone => "Phone (1080 × 2400)",
            Profile::Tablet => "Tablet (2560 × 1600)",
        }
    }

    /// Width, height, density and orientation.
    fn screen(self) -> (u32, u32, u32, &'static str) {
        match self {
            Profile::Phone => (1080, 2400, 420, "portrait"),
            Profile::Tablet => (2560, 1600, 320, "landscape"),
        }
    }
}

/// The AVD folder: `$ANDROID_AVD_HOME`, else `$ANDROID_USER_HOME/avd`, else
/// `~/.android/avd`, the order the emulator looks in.
pub fn avd_home() -> Option<PathBuf> {
    if let Some(dir) = ph_env::var("ANDROID_AVD_HOME") {
        return Some(PathBuf::from(dir));
    }
    if let Some(dir) = ph_env::var("ANDROID_USER_HOME") {
        return Some(PathBuf::from(dir).join("avd"));
    }
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|home| PathBuf::from(home).join(".android").join("avd"))
}

/// System images installed in the SDK at `sdk_root`, newest first.
pub fn system_images(sdk_root: &Path) -> Vec<SystemImage> {
    let mut images = Vec::new();
    let dirs = |dir: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect()
    };
    for platform in dirs(&sdk_root.join("system-images")) {
        for tag in dirs(&platform) {
            for abi in dirs(&tag) {
                if let Some(image) = read_image(sdk_root, &abi) {
                    images.push(image);
                }
            }
        }
    }
    images.sort_by(|a, b| b.api.cmp(&a.api).then_with(|| a.sysdir.cmp(&b.sysdir)));
    images
}

/// The image in `dir`, from its `source.properties`; `None` if it is not one
/// (or is only partly downloaded: no `system.img`).
fn read_image(sdk_root: &Path, dir: &Path) -> Option<SystemImage> {
    if !dir.join("system.img").is_file() {
        return None;
    }
    let properties = std::fs::read_to_string(dir.join("source.properties")).ok()?;
    let value = |key: &str| {
        properties
            .lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
            .map(|value| value.trim().to_string())
    };
    let relative = dir.strip_prefix(sdk_root).ok()?;
    let sysdir = relative.to_string_lossy().replace('\\', "/") + "/";
    let tag = value("SystemImage.TagId").unwrap_or_else(|| "default".into());
    Some(SystemImage {
        api: value("AndroidVersion.ApiLevel")?.parse().ok()?,
        tag_display: value("SystemImage.TagDisplay").unwrap_or_else(|| tag.clone()),
        tag,
        abi: value("SystemImage.Abi")?,
        sysdir,
    })
}

/// `My Phone 2` → `My_Phone_2`: AVD names are letters, digits, `.`, `_`, `-`.
pub fn avd_name(display: &str) -> String {
    display
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Writes a new AVD named after `display` into `home`. Returns its name.
pub fn create(home: &Path, display: &str, image: &SystemImage, profile: Profile) -> Result<String> {
    let name = avd_name(display);
    if name.is_empty() || name.chars().all(|c| c == '_') {
        bail!("Give the emulator a name.");
    }
    let (ini, folder) = (
        home.join(format!("{name}.ini")),
        home.join(format!("{name}.avd")),
    );
    if ini.exists() || folder.exists() {
        bail!("There is already an emulator named “{name}”.");
    }
    std::fs::create_dir_all(&folder).with_context(|| format!("creating {}", folder.display()))?;
    let (width, height, density, orientation) = profile.screen();
    let cpu = match image.abi.as_str() {
        "arm64-v8a" => "arm64",
        "armeabi-v7a" => "arm",
        other => other,
    };
    let config = [
        format!("AvdId={name}"),
        format!("PlayStore.enabled={}", image.tag.contains("playstore")),
        format!("abi.type={}", image.abi),
        format!("avd.ini.displayname={}", display.trim()),
        "avd.ini.encoding=UTF-8".into(),
        // Grown on demand: the disk file starts small.
        "disk.dataPartition.size=6G".into(),
        "fastboot.forceColdBoot=no".into(),
        "hw.accelerometer=yes".into(),
        "hw.audioInput=yes".into(),
        "hw.battery=yes".into(),
        format!("hw.cpu.arch={cpu}"),
        "hw.cpu.ncore=4".into(),
        "hw.dPad=no".into(),
        "hw.gps=yes".into(),
        "hw.gpu.enabled=yes".into(),
        "hw.gpu.mode=auto".into(),
        format!("hw.initialOrientation={orientation}"),
        "hw.keyboard=yes".into(),
        format!("hw.lcd.density={density}"),
        format!("hw.lcd.height={height}"),
        format!("hw.lcd.width={width}"),
        "hw.mainKeys=no".into(),
        "hw.ramSize=2048".into(),
        // No SD card file is made, so none is configured.
        "hw.sdCard=no".into(),
        format!("image.sysdir.1={}", image.sysdir),
        "showDeviceFrame=no".into(),
        format!("tag.display={}", image.tag_display),
        format!("tag.id={}", image.tag),
    ];
    std::fs::write(folder.join("config.ini"), config.join("\n") + "\n")?;
    let pointer = [
        "avd.ini.encoding=UTF-8".to_string(),
        format!("path={}", folder.display()),
        format!("target=android-{}", image.api),
    ];
    std::fs::write(&ini, pointer.join("\n") + "\n")?;
    Ok(name)
}

/// The `.avd` folder of `name`, from its `.ini` (the folder may live
/// elsewhere), else beside it.
fn folder_of(home: &Path, name: &str) -> PathBuf {
    std::fs::read_to_string(home.join(format!("{name}.ini")))
        .ok()
        .and_then(|ini| {
            ini.lines()
                .find_map(|line| line.strip_prefix("path="))
                .map(|path| PathBuf::from(path.trim()))
        })
        .filter(|path| path.is_dir())
        .unwrap_or_else(|| home.join(format!("{name}.avd")))
}

/// What a fresh start throws away: the user data, cache and snapshots. The
/// configuration (and an SD card) stay.
const DATA_FILES: &[&str] = &[
    "userdata-qemu.img",
    "userdata-qemu.img.qcow2",
    "cache.img",
    "cache.img.qcow2",
    "encryptionkey.img",
    "encryptionkey.img.qcow2",
    "bootcompleted.ini",
];

/// Resets `name` to a factory state, like Android Studio's *Wipe Data*. The
/// emulator must not be running. Returns the bytes freed.
pub fn wipe(home: &Path, name: &str) -> Result<u64> {
    let folder = folder_of(home, name);
    if !folder.join("config.ini").is_file() {
        bail!("There is no emulator named “{name}”.");
    }
    let mut freed = 0;
    for file in DATA_FILES {
        let path = folder.join(file);
        if let Ok(meta) = std::fs::metadata(&path) {
            freed += meta.len();
            std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        }
    }
    let snapshots = folder.join("snapshots");
    if snapshots.is_dir() {
        freed += dir_size(&snapshots);
        std::fs::remove_dir_all(&snapshots)
            .with_context(|| format!("removing {}", snapshots.display()))?;
    }
    Ok(freed)
}

/// Removes `name` and its disks. The emulator must not be running.
pub fn delete(home: &Path, name: &str) -> Result<()> {
    let folder = folder_of(home, name);
    // Only ever an AVD's own folder.
    if !folder.join("config.ini").is_file() || folder.extension().is_none_or(|ext| ext != "avd") {
        bail!("There is no emulator named “{name}”.");
    }
    std::fs::remove_dir_all(&folder).with_context(|| format!("removing {}", folder.display()))?;
    let ini = home.join(format!("{name}.ini"));
    if ini.exists() {
        std::fs::remove_file(&ini)?;
    }
    Ok(())
}

fn dir_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| match entry.metadata() {
            Ok(meta) if meta.is_dir() => dir_size(&entry.path()),
            Ok(meta) => meta.len(),
            Err(_) => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ph-avd-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// An SDK with one image; `name` keeps tests that run at once apart.
    fn fake_sdk(name: &str) -> PathBuf {
        let sdk = scratch(name);
        let image = sdk.join("system-images/android-35/google_apis_playstore/arm64-v8a");
        std::fs::create_dir_all(&image).unwrap();
        std::fs::write(image.join("system.img"), "").unwrap();
        std::fs::write(
            image.join("source.properties"),
            "AndroidVersion.ApiLevel=35\nSystemImage.Abi=arm64-v8a\nSystemImage.TagId=google_apis_playstore\nSystemImage.TagDisplay=Google Play\n",
        )
        .unwrap();
        // Partly downloaded: no system.img, so not offered.
        std::fs::create_dir_all(sdk.join("system-images/android-36/google_apis/x86_64")).unwrap();
        sdk
    }

    #[test]
    fn finds_installed_images() {
        let images = system_images(&fake_sdk("sdk-images"));
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].label(), "API 35 · Google Play · arm64-v8a");
        assert_eq!(
            images[0].sysdir,
            "system-images/android-35/google_apis_playstore/arm64-v8a/"
        );
    }

    #[test]
    fn creates_wipes_and_deletes() {
        let image = system_images(&fake_sdk("sdk-create")).remove(0);
        let home = scratch("home");
        let name = create(&home, "Test Phone", &image, Profile::Phone).unwrap();
        assert_eq!(name, "Test_Phone");
        let config = std::fs::read_to_string(home.join("Test_Phone.avd/config.ini")).unwrap();
        assert!(config.contains("image.sysdir.1=system-images/android-35/"));
        assert!(config.contains("hw.cpu.arch=arm64") && config.contains("PlayStore.enabled=true"));
        assert!(
            create(&home, "Test Phone", &image, Profile::Tablet).is_err(),
            "names are unique"
        );
        assert!(create(&home, "  ", &image, Profile::Tablet).is_err());

        let folder = home.join("Test_Phone.avd");
        std::fs::write(folder.join("userdata-qemu.img"), [0u8; 10]).unwrap();
        std::fs::write(folder.join("sdcard.img"), [0u8; 4]).unwrap();
        std::fs::create_dir_all(folder.join("snapshots/default_boot")).unwrap();
        std::fs::write(folder.join("snapshots/default_boot/ram.bin"), [0u8; 5]).unwrap();
        assert_eq!(wipe(&home, "Test_Phone").unwrap(), 15);
        assert!(!folder.join("userdata-qemu.img").exists() && !folder.join("snapshots").exists());
        assert!(folder.join("sdcard.img").exists() && folder.join("config.ini").exists());

        delete(&home, "Test_Phone").unwrap();
        assert!(!folder.exists() && !home.join("Test_Phone.ini").exists());
        assert!(delete(&home, "Test_Phone").is_err());
    }
}

//! What this build is called, who makes it, its icons and where it looks for
//! updates, from `brand/brand.toml` and `brand/icons/` at the repository
//! root. This crate is the one place the rest of the app reads them from, so
//! an organisation's build changes that folder and nothing else (see
//! docs/customizing.md).
//!
//! The file is read at build time (`build.rs`); a mistake in it stops the
//! build with a message rather than shipping a broken app.

/// One place updates come from (see `[[updates.sources]]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdateSource {
    /// Said in messages ("couldn't reach the mirror").
    pub name: &'static str,
    /// The signed `latest.json`.
    pub manifest: &'static str,
    /// The folder downloads are under (`<files><tag>/<name>`).
    pub files: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/brand.rs"));

/// A feature `[features]` can switch off (see `brand/brand.toml`). One that
/// is off is not in the app at all: not in the tools, menus, the command
/// palette, Settings or the shortcuts, and agents do not get its tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Feature {
    // The tools beside the chat.
    Files,
    Changes,
    Pulls,
    Board,
    Browser,
    Devices,
    Runs,
    Tests,
    Terminal,
    Logcat,
    // Elsewhere.
    GitHub,
    Schedules,
    Utilities,
    Usage,
    NetworkDrives,
    ParallelRuns,
    Presentations,
    ChromeExtension,
    QuickChat,
}

impl Feature {
    pub const ALL: [Feature; 19] = [
        Feature::Files,
        Feature::Changes,
        Feature::Pulls,
        Feature::Board,
        Feature::Browser,
        Feature::Devices,
        Feature::Runs,
        Feature::Tests,
        Feature::Terminal,
        Feature::Logcat,
        Feature::GitHub,
        Feature::Schedules,
        Feature::Utilities,
        Feature::Usage,
        Feature::NetworkDrives,
        Feature::ParallelRuns,
        Feature::Presentations,
        Feature::ChromeExtension,
        Feature::QuickChat,
    ];

    /// Its key in `[features]`.
    pub fn key(self) -> &'static str {
        match self {
            Feature::Files => "files",
            Feature::Changes => "changes",
            Feature::Pulls => "pulls",
            Feature::Board => "board",
            Feature::Browser => "browser",
            Feature::Devices => "devices",
            Feature::Runs => "runs",
            Feature::Tests => "tests",
            Feature::Terminal => "terminal",
            Feature::Logcat => "logcat",
            Feature::GitHub => "github",
            Feature::Schedules => "schedules",
            Feature::Utilities => "utilities",
            Feature::Usage => "usage",
            Feature::NetworkDrives => "network_drives",
            Feature::ParallelRuns => "parallel_runs",
            Feature::Presentations => "presentations",
            Feature::ChromeExtension => "chrome_extension",
            Feature::QuickChat => "quick_chat",
        }
    }
}

/// Whether this build has `feature` (on unless `[features]` switches it off).
pub fn enabled(feature: Feature) -> bool {
    !SWITCHED_OFF.contains(&feature.key())
}

/// The icons, in `brand/icons/`.
pub mod icons {
    /// The app icon, 256 px square: welcome screen, chat, About.
    pub const APP_256_PNG: &[u8] = include_bytes!("../../../brand/icons/app-256.png");
    /// The title bar's mark, 80 px square (drawn at 20 px).
    pub const TITLE_BAR_PNG: &[u8] = include_bytes!("../../../brand/icons/title-bar.png");
    /// An organisation's own icons, from `brand/icons/custom/<name>.svg`:
    /// name and SVG. `ph_ui::custom_icon(name)` draws one.
    pub const CUSTOM: &[(&str, &[u8])] = super::CUSTOM_ICONS;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_brand_is_complete() {
        assert!(!NAME.is_empty());
        assert_eq!(name!(), NAME);
        assert_eq!(stem!(), STEM);
        assert_eq!(STEM, NAME.replace(' ', ""));
        assert!(!DATA_FOLDER.is_empty() && !DATA_FOLDER.contains(['/', '\\']));
        assert!(!KEYCHAIN.is_empty());
        assert!(!UPDATES_ENABLED || !UPDATE_SOURCES.is_empty());
        assert!(!UPDATE_KEY.trim().is_empty());
        // PNG files, as the app draws them.
        for icon in [icons::APP_256_PNG, icons::TITLE_BAR_PNG] {
            assert_eq!(&icon[..8], b"\x89PNG\r\n\x1a\n");
        }
    }

    #[test]
    fn the_feature_keys_match_what_the_build_checks() {
        let keys: Vec<&str> = Feature::ALL.iter().map(|feature| feature.key()).collect();
        assert_eq!(keys, FEATURE_KEYS, "build.rs FEATURES and Feature::ALL");
        // What is off is a known feature.
        assert!(SWITCHED_OFF.iter().all(|key| FEATURE_KEYS.contains(key)));
    }
}

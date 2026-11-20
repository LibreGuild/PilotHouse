//! The tools beside the chat, picked from the icon bar on the right. Files
//! is one of them: its tree shows here, and the files it opens beside the
//! chat. (The sidebar lists workspaces and their chats.)

use gpui_kit::Action;
use ph_ui::Icons;

use crate::actions::{
    ShowBrowser, ShowChanges, ShowDevices, ShowFiles, ShowLogcat, ShowPulls, ShowRuns,
    ShowTerminal, ShowTests,
};

/// Which tool the context panel shows. Persisted per project by [`Self::key`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ContextTab {
    #[default]
    Files,
    Changes,
    Pulls,
    Terminal,
    Browser,
    Devices,
    Runs,
    Tests,
    /// A device's log, a tool of its own (the Devices pane is its screen).
    Logcat,
}

impl ContextTab {
    /// In the icon bar's order: the column's tools, then the bottom
    /// panel's.
    pub const ALL: [ContextTab; 9] = [
        ContextTab::Files,
        ContextTab::Changes,
        ContextTab::Pulls,
        ContextTab::Browser,
        ContextTab::Devices,
        ContextTab::Runs,
        ContextTab::Tests,
        ContextTab::Terminal,
        ContextTab::Logcat,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ContextTab::Files => "Files",
            ContextTab::Changes => "Git",
            ContextTab::Pulls => "PRs",
            ContextTab::Terminal => "Terminal",
            ContextTab::Browser => "Browser",
            ContextTab::Devices => "Devices",
            ContextTab::Runs => "Run",
            ContextTab::Tests => "Tests",
            ContextTab::Logcat => "Logcat",
        }
    }

    /// What the tool is, in a line (Settings → Features).
    pub fn about(self) -> &'static str {
        match self {
            ContextTab::Files => "The folder tree and the editor.",
            ContextTab::Changes => "Changes to review, branches, history and stashes.",
            ContextTab::Pulls => "The repository's pull requests.",
            ContextTab::Terminal => "Terminals in the workspace's folder.",
            ContextTab::Browser => "A browser agents can drive, in Preview.",
            ContextTab::Devices => "Android emulators and phones, in Preview.",
            ContextTab::Runs => "Run configurations and the ports they open.",
            ContextTab::Tests => "UI tests in plain words, run in the browser or on a device.",
            ContextTab::Logcat => "The log of the Android device in use.",
        }
    }

    pub fn icon(self) -> Icons {
        match self {
            ContextTab::Files => Icons::Files,
            ContextTab::Changes => Icons::GitBranch,
            ContextTab::Pulls => Icons::GitPullRequest,
            ContextTab::Terminal => Icons::Terminal,
            ContextTab::Browser => Icons::Globe,
            ContextTab::Devices => Icons::TabletSmartphone,
            ContextTab::Runs => Icons::Play,
            ContextTab::Tests => Icons::FlaskConical,
            ContextTab::Logcat => Icons::ScrollText,
        }
    }

    /// Stable identifier used in saved layouts and element ids.
    pub fn key(self) -> &'static str {
        match self {
            ContextTab::Files => "files",
            ContextTab::Changes => "changes",
            ContextTab::Pulls => "pulls",
            ContextTab::Terminal => "terminal",
            ContextTab::Browser => "browser",
            ContextTab::Devices => "devices",
            ContextTab::Runs => "runs",
            ContextTab::Tests => "tests",
            ContextTab::Logcat => "logcat",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|tab| tab.key() == key)
    }

    /// The action that switches to this tab, so buttons and shortcuts agree.
    pub fn action(self) -> Box<dyn Action> {
        match self {
            ContextTab::Files => Box::new(ShowFiles),
            ContextTab::Changes => Box::new(ShowChanges),
            ContextTab::Pulls => Box::new(ShowPulls),
            ContextTab::Terminal => Box::new(ShowTerminal),
            ContextTab::Browser => Box::new(ShowBrowser),
            ContextTab::Devices => Box::new(ShowDevices),
            ContextTab::Runs => Box::new(ShowRuns),
            ContextTab::Tests => Box::new(ShowTests),
            ContextTab::Logcat => Box::new(ShowLogcat),
        }
    }

    /// The brand's feature that is this tool (`[features]` in
    /// `brand/brand.toml`).
    pub fn feature(self) -> ph_brand::Feature {
        use ph_brand::Feature;
        match self {
            ContextTab::Files => Feature::Files,
            ContextTab::Changes => Feature::Changes,
            ContextTab::Pulls => Feature::Pulls,
            ContextTab::Terminal => Feature::Terminal,
            ContextTab::Browser => Feature::Browser,
            ContextTab::Devices => Feature::Devices,
            ContextTab::Runs => Feature::Runs,
            ContextTab::Tests => Feature::Tests,
            ContextTab::Logcat => Feature::Logcat,
        }
    }

    /// Whether this build has the tool at all (the brand can leave one out;
    /// the user's own switches are in Settings → Features).
    pub fn in_build(self) -> bool {
        ph_brand::enabled(self.feature())
    }

    /// The tools this build has, in [`Self::ALL`]'s order.
    pub fn available() -> impl Iterator<Item = ContextTab> {
        Self::ALL.into_iter().filter(|tab| tab.in_build())
    }
}

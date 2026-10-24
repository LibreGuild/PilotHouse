//! The tools beside the chat, picked from the icon bar on the right. (Files
//! are not a tool: the tree is in the sidebar, open files beside the chat.)

use gpui_kit::Action;
use ph_ui::Icons;

use crate::actions::{
    ShowBoard, ShowBrowser, ShowChanges, ShowDevices, ShowPulls, ShowRuns, ShowTerminal, ShowTests,
};

/// Which tool the context panel shows. Persisted per project by [`Self::key`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContextTab {
    #[default]
    Changes,
    Pulls,
    Board,
    Terminal,
    Browser,
    Devices,
    Runs,
    Tests,
}

impl ContextTab {
    pub const ALL: [ContextTab; 8] = [
        ContextTab::Changes,
        ContextTab::Pulls,
        ContextTab::Board,
        ContextTab::Terminal,
        ContextTab::Browser,
        ContextTab::Devices,
        ContextTab::Runs,
        ContextTab::Tests,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ContextTab::Changes => "Git",
            ContextTab::Pulls => "PRs",
            ContextTab::Board => "Board",
            ContextTab::Terminal => "Terminal",
            ContextTab::Browser => "Browser",
            ContextTab::Devices => "Devices",
            ContextTab::Runs => "Run",
            ContextTab::Tests => "Tests",
        }
    }

    pub fn icon(self) -> Icons {
        match self {
            ContextTab::Changes => Icons::GitBranch,
            ContextTab::Pulls => Icons::GitPullRequest,
            ContextTab::Board => Icons::SquareKanban,
            ContextTab::Terminal => Icons::Terminal,
            ContextTab::Browser => Icons::Globe,
            ContextTab::Devices => Icons::TabletSmartphone,
            ContextTab::Runs => Icons::Play,
            ContextTab::Tests => Icons::FlaskConical,
        }
    }

    /// Stable identifier used in saved layouts and element ids.
    pub fn key(self) -> &'static str {
        match self {
            ContextTab::Changes => "changes",
            ContextTab::Pulls => "pulls",
            ContextTab::Board => "board",
            ContextTab::Terminal => "terminal",
            ContextTab::Browser => "browser",
            ContextTab::Devices => "devices",
            ContextTab::Runs => "runs",
            ContextTab::Tests => "tests",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|tab| tab.key() == key)
    }

    /// The action that switches to this tab, so buttons and shortcuts agree.
    pub fn action(self) -> Box<dyn Action> {
        match self {
            ContextTab::Changes => Box::new(ShowChanges),
            ContextTab::Pulls => Box::new(ShowPulls),
            ContextTab::Board => Box::new(ShowBoard),
            ContextTab::Terminal => Box::new(ShowTerminal),
            ContextTab::Browser => Box::new(ShowBrowser),
            ContextTab::Devices => Box::new(ShowDevices),
            ContextTab::Runs => Box::new(ShowRuns),
            ContextTab::Tests => Box::new(ShowTests),
        }
    }

    pub fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|tab| *tab == self)
            .unwrap_or_default()
    }
}

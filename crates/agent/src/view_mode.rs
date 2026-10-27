//! How much of an agent's work the transcript shows, for every chat:
//! each step (*Detailed*), runs of steps folded into one line (*Normal*),
//! or only what it said and changed (*Summary*).

use gpui_kit::{App, Global};
use ph_store::Store;
use serde::{Deserialize, Serialize};

const KEY: &str = "chat.view_mode";

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewMode {
    Detailed,
    #[default]
    Normal,
    Summary,
}

impl ViewMode {
    pub const ALL: [ViewMode; 3] = [ViewMode::Detailed, ViewMode::Normal, ViewMode::Summary];

    pub fn label(self) -> &'static str {
        match self {
            ViewMode::Detailed => "Detailed: every step",
            ViewMode::Normal => "Normal: steps folded into runs",
            ViewMode::Summary => "Summary: replies and changes only",
        }
    }

    /// The mode chats are shown in.
    pub fn current(cx: &App) -> Self {
        cx.try_global::<Current>()
            .map(|current| current.0)
            .unwrap_or_default()
    }

    /// Reads the saved mode, once.
    pub(crate) fn load(store: &Store, cx: &mut App) {
        if !cx.has_global::<Current>() {
            cx.set_global(Current(store.setting(KEY)));
        }
    }

    /// Makes `self` the mode for every chat, and keeps it.
    pub(crate) fn choose(self, store: &Store, cx: &mut App) {
        cx.set_global(Current(self));
        if let Err(err) = store.set_setting(KEY, &self) {
            tracing::warn!("saving the chat view mode failed: {err:#}");
        }
    }
}

struct Current(ViewMode);

impl Global for Current {}

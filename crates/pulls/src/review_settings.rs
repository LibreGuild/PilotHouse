//! What happens when a pull request asks for the user's review, chosen
//! under Settings → GitHub: whether it is announced, and whether an agent
//! reviews it by itself.

use ph_store::Store;
use serde::{Deserialize, Serialize};

const KEY: &str = "github.reviews";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewSettings {
    /// A system notification for each new request.
    pub announce: bool,
    /// An agent starts on each new request (reading only; nothing is sent
    /// to GitHub). Off unless asked for: nothing starts by itself.
    pub auto_review: bool,
}

impl Default for ReviewSettings {
    fn default() -> Self {
        Self {
            announce: true,
            auto_review: false,
        }
    }
}

impl ReviewSettings {
    pub fn read(store: &Store) -> Self {
        store.setting::<Option<Self>>(KEY).unwrap_or_default()
    }

    pub fn save(&self, store: &Store) {
        if let Err(err) = store.set_setting(KEY, &Some(*self)) {
            tracing::warn!("saving the review settings failed: {err:#}");
        }
    }
}

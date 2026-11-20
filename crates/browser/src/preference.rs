//! Which browser agents open web pages in first: the user's own Chrome
//! (through PilotHouse's extension) or the built-in browser. A setting
//! (Settings → Agents), Chrome unless chosen otherwise.
//!
//! It is a preference and not a wall. Agents have both families of tools
//! (`chrome_*`, `browser_*`): the server's instructions say which to reach
//! for first ([`preference_note`] answers the bridge when a chat starts),
//! and a page asked for in Chrome while Chrome is not connected opens in
//! the built-in browser by itself, with a word to the agent to go on there
//! (see `chrome.open` in `agent_control.rs`). So a chat works on a computer
//! where the extension was never set up, without the agent first failing.

use ph_store::Store;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{AgentChrome, AgentChromeStatus};

const KEY: &str = "browser.preferred";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Preferred {
    /// The user's own Chrome, with their sign-ins.
    #[default]
    Chrome,
    /// PilotHouse's own browser, in its pane.
    BuiltIn,
}

impl Preferred {
    pub const ALL: [Preferred; 2] = [Preferred::Chrome, Preferred::BuiltIn];

    /// As Settings names it. "PilotHouse Chrome" is the extension's Chrome:
    /// the extension keeps that name in a build of another name.
    pub fn label(self) -> &'static str {
        match self {
            Preferred::Chrome => "Chrome (PilotHouse Chrome)",
            Preferred::BuiltIn => "Built-in browser",
        }
    }

    /// As the bridge is told.
    fn key(self) -> &'static str {
        match self {
            Preferred::Chrome => "chrome",
            Preferred::BuiltIn => "built_in",
        }
    }
}

/// The browser agents use first: the user's choice, among the browsers this
/// build has.
pub fn preferred(store: &Store) -> Preferred {
    use ph_brand::{Feature, enabled};
    if !enabled(Feature::ChromeExtension) {
        return Preferred::BuiltIn;
    }
    if !enabled(Feature::Browser) {
        return Preferred::Chrome;
    }
    store.setting(KEY)
}

pub fn set_preferred(store: &Store, preferred: Preferred) -> anyhow::Result<()> {
    store.set_setting(KEY, &preferred)
}

/// Whether the extension is connected: whether Chrome can be used now.
pub fn chrome_connected(cx: &gpui_kit::App) -> bool {
    AgentChrome::global(cx).is_some_and(|chrome| {
        matches!(
            chrome.read(cx).status(),
            AgentChromeStatus::Connected { .. }
        )
    })
}

/// What the bridge asks when a chat starts (`browser.preference`), for the
/// instructions it gives the agent.
pub(crate) fn preference_note(store: &Store, cx: &gpui_kit::App) -> Value {
    json!({
        "preferred": preferred(store).key(),
        "chrome_connected": chrome_connected(cx),
    })
}

/// What an agent is told when a page it asked for in Chrome opened in the
/// built-in browser.
pub(crate) const FELL_BACK: &str = "Chrome is not connected (the PilotHouse extension is not set up, or Chrome is closed), so this page was opened in the built-in browser instead. Go on with the browser_ tools (browser_snapshot, browser_click, browser_type, browser_screenshot…): the chrome_ tools do not reach this page. The user's sign-ins are not here; if the page needs them, ask the user to set the extension up in Settings → Chrome extension.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrome_is_preferred_until_chosen_otherwise() {
        let store = Store::open_in_memory().unwrap();
        // A build with both browsers (PilotHouse's own).
        if ph_brand::enabled(ph_brand::Feature::ChromeExtension)
            && ph_brand::enabled(ph_brand::Feature::Browser)
        {
            assert_eq!(preferred(&store), Preferred::Chrome);
            set_preferred(&store, Preferred::BuiltIn).unwrap();
            assert_eq!(preferred(&store), Preferred::BuiltIn);
        }
        assert_eq!(Preferred::Chrome.key(), "chrome");
        assert_eq!(Preferred::BuiltIn.key(), "built_in");
    }
}

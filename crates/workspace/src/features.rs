//! The brand's features (`[features]` in `brand/brand.toml`, read by
//! `ph_brand`) where the window shell meets them: which action opens which
//! feature, so every list of actions (the command palette, the title bar's
//! menu, the shortcuts sheet) leaves out the ones this build does not have,
//! and their handlers do nothing.
//!
//! The tools beside the chat are [`ContextTab`](crate::ContextTab)s, with
//! [`ContextTab::in_build`](crate::ContextTab::in_build); their actions are
//! here too.

use gpui_kit::Action;
use ph_brand::{Feature, enabled};

use crate::actions::*;

/// The feature `action` opens, for an action that opens one.
pub(crate) fn of_action(action: &dyn Action) -> Option<Feature> {
    let any = action.as_any();
    let feature = if any.is::<ShowFiles>() {
        Feature::Files
    } else if any.is::<ShowChanges>() {
        Feature::Changes
    } else if any.is::<ShowPulls>() {
        Feature::Pulls
    } else if any.is::<ShowBoard>() {
        Feature::Board
    } else if any.is::<ShowBrowser>() {
        Feature::Browser
    } else if any.is::<ShowDevices>() {
        Feature::Devices
    } else if any.is::<ShowRuns>() {
        Feature::Runs
    } else if any.is::<ShowTests>() {
        Feature::Tests
    } else if any.is::<ShowTerminal>() || any.is::<ToggleTerminal>() {
        Feature::Terminal
    } else if any.is::<ShowLogcat>() {
        Feature::Logcat
    } else if any.is::<ShowGitHubView>() {
        Feature::GitHub
    } else if any.is::<ShowSchedules>() {
        Feature::Schedules
    } else if any.is::<ShowUtilities>() {
        Feature::Utilities
    } else if any.is::<ShowUsage>() {
        Feature::Usage
    } else if any.is::<QuickChat>() {
        Feature::QuickChat
    } else if any.is::<ShowNetworkDrives>() {
        Feature::NetworkDrives
    } else if any.is::<ShowParallelRuns>() {
        Feature::ParallelRuns
    } else if any.is::<NewPresentation>() || any.is::<OpenPresentation>() {
        Feature::Presentations
    } else if any.is::<ph_browser::OpenChromeExtensionSettings>() {
        Feature::ChromeExtension
    } else {
        return None;
    };
    Some(feature)
}

/// Whether this build has what `action` opens (an action that opens no
/// feature always is).
pub(crate) fn allows(action: &dyn Action) -> bool {
    of_action(action).is_none_or(enabled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ContextTab;

    #[test]
    fn each_tool_action_is_its_tools_feature() {
        for tab in ContextTab::ALL {
            assert_eq!(of_action(&*tab.action()), Some(tab.feature()), "{tab:?}");
        }
        assert_eq!(of_action(&ShowGitHubView), Some(Feature::GitHub));
        assert_eq!(of_action(&NewPresentation), Some(Feature::Presentations));
        assert_eq!(of_action(&NewSession), None);
        assert!(allows(&NewSession));
    }
}

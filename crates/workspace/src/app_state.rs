use gpui_kit::{App, Global, Window};
use ph_store::Store;
use ph_theme::{Appearance, Preferences};

use crate::context_panel::ContextTab;

/// Settings keys owned by the shell.
pub(crate) mod keys {
    pub const APPEARANCE: &str = "appearance";
    pub const LAST_PROJECT: &str = "workspace.last_project";
    pub const SIDEBAR_COLLAPSED: &str = "workspace.sidebar_collapsed";
    /// The workspaces unfolded in the sidebar (their chats listed).
    pub const SIDEBAR_EXPANDED: &str = "workspace.sidebar_expanded";
    pub const DISABLED_FEATURES: &str = "features.disabled";
    pub const PREFERENCES: &str = "appearance.preferences";
}

/// Process-wide services shared by every view.
///
/// Installed once at startup as a GPUI global; views read it through
/// [`AppState::global`]. Holds handles only, never per-view state.
pub struct AppState {
    store: Store,
}

impl Global for AppState {}

impl AppState {
    /// Installs the app state and the saved look-and-feel preferences.
    pub fn install(store: Store, cx: &mut App) {
        let preferences: Preferences = store.setting(keys::PREFERENCES);
        cx.set_global(Self { store });
        cx.set_global(preferences);
    }

    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    pub fn store(cx: &App) -> &Store {
        &Self::global(cx).store
    }

    pub fn appearance(cx: &App) -> Appearance {
        Self::store(cx).setting(keys::APPEARANCE)
    }

    /// Context-panel tools the user has switched on (Settings → Features).
    pub fn enabled_tabs(cx: &App) -> Vec<ContextTab> {
        let disabled: Vec<String> = Self::store(cx).setting(keys::DISABLED_FEATURES);
        ContextTab::ALL
            .into_iter()
            .filter(|tab| !disabled.iter().any(|key| key == tab.key()))
            .collect()
    }

    pub fn set_tab_enabled(tab: ContextTab, enabled: bool, cx: &mut App) {
        let store = Self::store(cx);
        let mut disabled: Vec<String> = store.setting(keys::DISABLED_FEATURES);
        disabled.retain(|key| key != tab.key());
        if !enabled {
            disabled.push(tab.key().to_string());
        }
        if let Err(err) = store.set_setting(keys::DISABLED_FEATURES, &disabled) {
            tracing::warn!("saving features failed: {err:#}");
        }
        cx.refresh_windows();
    }

    /// Changes the look-and-feel preferences, saves them and applies them to
    /// every window (fonts and colors refresh at once; editors follow).
    pub fn update_preferences(cx: &mut App, change: impl FnOnce(&mut Preferences)) {
        let mut preferences = ph_theme::preferences::preferences(cx);
        change(&mut preferences);
        if let Err(err) = Self::store(cx).set_setting(keys::PREFERENCES, &preferences) {
            tracing::warn!("saving preferences failed: {err:#}");
        }
        cx.set_global(preferences);
        ph_theme::preferences::apply(None, cx);
        for window in cx.windows() {
            let _ = window.update(cx, |_, window, cx| {
                ph_theme::preferences::apply(Some(window), cx)
            });
        }
        cx.refresh_windows();
    }

    /// Saves the appearance preference and applies it immediately.
    pub fn set_appearance(appearance: Appearance, window: &mut Window, cx: &mut App) {
        if let Err(err) = Self::store(cx).set_setting(keys::APPEARANCE, &appearance) {
            tracing::warn!("saving appearance failed: {err:#}");
        }
        ph_theme::apply_appearance(appearance, Some(window), cx);
    }
}

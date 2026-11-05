use gpui_kit::{App, Global};
use ph_store::Store;
use ph_theme::{Appearance, Preferences};

use crate::context_panel::ContextTab;

/// Settings keys owned by the shell.
pub(crate) mod keys {
    pub const APPEARANCE: &str = "appearance";
    pub const LAST_PROJECT: &str = "workspace.last_project";
    pub const SIDEBAR_COLLAPSED: &str = "workspace.sidebar_collapsed";
    /// The sidebar's width as dragged, in pixels (0 for the default).
    pub const SIDEBAR_WIDTH: &str = "workspace.sidebar_width";
    /// The workspaces unfolded in the sidebar (their chats listed).
    pub const SIDEBAR_EXPANDED: &str = "workspace.sidebar_expanded";
    /// Chats pinned to the top of the sidebar, in pin order.
    pub const SIDEBAR_PINNED: &str = "workspace.sidebar_pinned";
    /// Chats marked done: listed apart, under *Done*.
    pub const SIDEBAR_SETTLED: &str = "workspace.sidebar_settled";
    /// Sidebar panes collapsed and sections folded away (`chats`,
    /// `workspaces`, `pinned`).
    pub const SIDEBAR_FOLDED: &str = "workspace.sidebar_folded";
    /// The Chats pane's part of the sidebar's height while the Workspaces
    /// pane is open too, 0 to 1 (0 for an even split).
    pub const SIDEBAR_SPLIT: &str = "workspace.sidebar_split";
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
        cx.set_global(DisabledTabs(store.setting(keys::DISABLED_FEATURES)));
        let mut preferences: Preferences = store.setting(keys::PREFERENCES);
        // Sizes saved with older defaults move to the new ones, once.
        if preferences.migrate_sizes()
            && let Err(err) = store.set_setting(keys::PREFERENCES, &preferences)
        {
            tracing::warn!("saving the preferences failed: {err:#}");
        }
        // A light or dark choice from before themes had names becomes the
        // theme it showed, once.
        let appearance: Appearance = store.setting(keys::APPEARANCE);
        let migrated = appearance.migrated(&preferences);
        if migrated != appearance
            && let Err(err) = store.set_setting(keys::APPEARANCE, &migrated)
        {
            tracing::warn!("saving the theme choice failed: {err:#}");
        }
        cx.set_global(Self { store });
        cx.set_global(preferences);
        ph_theme::themes::set_user_dir(ph_store::paths::data_dir().join("themes"), cx);
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
        // Kept in memory: asked on every frame of every workspace.
        let disabled: Vec<String> = match cx.try_global::<DisabledTabs>() {
            Some(known) => known.0.clone(),
            None => Self::store(cx).setting(keys::DISABLED_FEATURES),
        };
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
        cx.set_global(DisabledTabs(disabled));
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

    /// Saves the theme choice and shows it in every window (a preview
    /// being designed is dropped).
    pub fn set_appearance(appearance: Appearance, cx: &mut App) {
        if let Err(err) = Self::store(cx).set_setting(keys::APPEARANCE, &appearance) {
            tracing::warn!("saving the theme choice failed: {err:#}");
        }
        ph_theme::themes::set_preview(None, cx);
        Self::refresh_theme(cx);
    }

    /// Shows the chosen theme (or the preview of one being designed) in
    /// every window.
    pub fn refresh_theme(cx: &mut App) {
        let appearance = Self::appearance(cx);
        let mut shown = false;
        for window in cx.windows() {
            shown |= window
                .update(cx, |_, window, cx| {
                    ph_theme::apply_appearance(&appearance, Some(window), cx)
                })
                .is_ok();
        }
        if !shown {
            ph_theme::apply_appearance(&appearance, None, cx);
        }
        cx.refresh_windows();
    }
}

/// The tools turned off in Settings, as saved (read once, then kept).
struct DisabledTabs(Vec<String>);

impl gpui_kit::Global for DisabledTabs {}

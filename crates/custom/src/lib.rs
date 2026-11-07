//! An organisation's own additions to PilotHouse, in one place:
//!
//! - **views** of its own beside Workspaces and GitHub (the switch at the top
//!   left of the window, the command palette's *Show the … view*);
//! - **commands** in the command palette;
//! - **startup** work ([`init`]): its own migrations, globals, key bindings.
//!
//! PilotHouse ships [`views`] and [`commands`] empty and [`init`] doing
//! nothing, and never fills them, so a build that does merges new PilotHouse releases without a
//! conflict here. Put the code of each addition in a crate of its own
//! (`crates/acme-reports`), add it to this crate's `Cargo.toml`, and list it
//! below. docs/customizing.md walks through it; the brand (name, icons,
//! features on or off) is in `brand/`.

use gpui_kit::component::Icon;
use gpui_kit::{Action, AnyView, App, Window};
use serde::Deserialize;

/// What an addition gets from the app.
pub struct CustomContext {
    /// The app's database (settings, workspaces, chats). Keep your own
    /// settings under keys starting with `x.<your name>.`, and your own
    /// tables on a migration track of your own (`Store::apply_track`).
    pub store: ph_store::Store,
}

/// A view of the organisation's own, beside Workspaces and GitHub.
pub struct CustomView {
    /// A short, stable id: `reports`. Lowercase letters, digits and `-`.
    pub id: &'static str,
    /// Its name in the switch and the command palette: `Reports`.
    pub label: &'static str,
    /// Its icon: a Lucide icon (`ph_ui::Icons::ChartPie.into()`) or one of
    /// the organisation's own (`ph_ui::custom_icon("reports")`, from
    /// `brand/icons/custom/reports.svg`).
    pub icon: fn() -> Icon,
    /// Makes the view, the first time it is shown; the window keeps it
    /// while it is open.
    pub build: fn(&CustomContext, &mut Window, &mut App) -> AnyView,
}

/// A command of the organisation's own, in the command palette.
pub struct CustomCommand {
    /// What the palette shows: `Open the team wiki`.
    pub label: &'static str,
    pub icon: fn() -> Icon,
    /// Words that also find it: `&["wiki", "docs"]`.
    pub keywords: &'static [&'static str],
    pub run: fn(&CustomContext, &mut Window, &mut App),
}

/// Shows the view `id` of [`views`] (dispatch it from your own code, or
/// bind a key to it).
#[derive(Action, Clone, PartialEq, Deserialize)]
#[action(namespace = custom, no_json)]
pub struct ShowView {
    pub id: String,
}

/// Runs once when the app starts, before any window opens: apply your own
/// migrations here (`context.store.apply_track(...)`), set up globals, bind
/// keys.
///
/// ```ignore
/// pub fn init(context: &CustomContext, cx: &mut App) {
///     if let Err(err) = context.store.apply_track("acme", acme_reports::MIGRATIONS) {
///         eprintln!("Acme's tables could not be updated: {err:#}");
///     }
///     acme_reports::init(cx);
/// }
/// ```
pub fn init(_context: &CustomContext, _cx: &mut App) {}

/// The organisation's views, in the order the switch shows them.
///
/// ```ignore
/// pub fn views() -> Vec<CustomView> {
///     vec![CustomView {
///         id: "reports",
///         label: "Reports",
///         icon: || ph_ui::custom_icon("reports"),
///         build: |context, window, cx| acme_reports::view(context.store.clone(), window, cx),
///     }]
/// }
/// ```
pub fn views() -> Vec<CustomView> {
    Vec::new()
}

/// The organisation's commands, in the command palette's *Custom* group.
///
/// ```ignore
/// pub fn commands() -> Vec<CustomCommand> {
///     vec![CustomCommand {
///         label: "Open the team wiki",
///         icon: || ph_ui::Icons::BookOpen.into(),
///         keywords: &["wiki", "docs"],
///         run: |_, _, cx| cx.open_url("https://wiki.acme.example"),
///     }]
/// }
/// ```
pub fn commands() -> Vec<CustomCommand> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_short_and_unique() {
        let views = views();
        let mut ids: Vec<&str> = views.iter().map(|view| view.id).collect();
        for id in &ids {
            assert!(
                !id.is_empty()
                    && id
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "view id {id:?}: lowercase letters, digits and -"
            );
            assert!(
                *id != "workspaces" && *id != "github",
                "{id} is PilotHouse's own view"
            );
        }
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), views.len(), "two views have one id");
    }
}

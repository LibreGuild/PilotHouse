//! Workspace windows. The main window holds most workspaces; one can move
//! to a window of its own (two monitors, or two side by side). A workspace
//! is shown by one window at a time: picking it in another window brings
//! the one that shows it forward. Its chats, shells and test runs belong to
//! the workspace, not the window, so they carry on through the move.

use std::collections::HashMap;

use anyhow::Result;
use gpui_kit::component::TitleBar;
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Entity, Global, Size, WeakEntity, WindowBounds,
    WindowOptions,
};
use ph_store::projects::ProjectId;
use ph_theme::layout;

use crate::AppState;
use crate::workspace::Workspace;

/// Every open workspace window, and which shows each workspace.
#[derive(Default)]
struct Windows {
    open: Vec<(AnyWindowHandle, WeakEntity<Workspace>)>,
    shown_in: HashMap<ProjectId, AnyWindowHandle>,
}

impl Global for Windows {}

/// Notes a window's root workspace.
pub(crate) fn register(handle: AnyWindowHandle, workspace: &Entity<Workspace>, cx: &mut App) {
    let windows = cx.default_global::<Windows>();
    windows.open.push((handle, workspace.downgrade()));
}

/// Workspace `id` is shown in window `handle` now.
pub(crate) fn set_shown_in(id: ProjectId, handle: AnyWindowHandle, cx: &mut App) {
    cx.default_global::<Windows>().shown_in.insert(id, handle);
}

/// The window that shows workspace `id`, if it is open.
pub(crate) fn window_showing(id: ProjectId, cx: &mut App) -> Option<AnyWindowHandle> {
    prune(cx);
    cx.try_global::<Windows>()?.shown_in.get(&id).copied()
}

/// The other window that shows workspace `id`, if it is still open.
pub(crate) fn shown_elsewhere(
    id: ProjectId,
    here: AnyWindowHandle,
    cx: &mut App,
) -> Option<(AnyWindowHandle, Entity<Workspace>)> {
    prune(cx);
    let windows = cx.try_global::<Windows>()?;
    let handle = *windows.shown_in.get(&id)?;
    if handle == here {
        return None;
    }
    let workspace = windows
        .open
        .iter()
        .find(|(open, _)| *open == handle)
        .and_then(|(_, workspace)| workspace.upgrade())?;
    // It may have moved on since (a window closes its spaces when closed).
    workspace
        .read(cx)
        .has_space(id)
        .then_some((handle, workspace))
}

/// Every open window's workspace, to tell them all of a change (a
/// workspace added, renamed or removed).
pub(crate) fn all(cx: &mut App) -> Vec<Entity<Workspace>> {
    prune(cx);
    cx.try_global::<Windows>()
        .map(|windows| {
            windows
                .open
                .iter()
                .filter_map(|(_, workspace)| workspace.upgrade())
                .collect()
        })
        .unwrap_or_default()
}

/// Some open workspace window, for when the main one has closed.
pub(crate) fn first_open(cx: &App) -> Option<(AnyWindowHandle, Entity<Workspace>)> {
    cx.try_global::<Windows>()?
        .open
        .iter()
        .find_map(|(handle, workspace)| Some((*handle, workspace.upgrade()?)))
}

/// Forgets windows that closed.
fn prune(cx: &mut App) {
    if !cx.has_global::<Windows>() {
        return;
    }
    let windows = cx.global_mut::<Windows>();
    windows
        .open
        .retain(|(_, workspace)| workspace.upgrade().is_some());
    let open: Vec<AnyWindowHandle> = windows.open.iter().map(|(handle, _)| *handle).collect();
    windows.shown_in.retain(|_, handle| open.contains(handle));
}

/// How a workspace window opens: our title bar, the saved default size.
pub fn window_options(cx: &App) -> WindowOptions {
    let mut options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            Size::new(layout::WINDOW_DEFAULT_WIDTH, layout::WINDOW_DEFAULT_HEIGHT),
            cx,
        ))),
        window_min_size: Some(Size::new(
            layout::WINDOW_MIN_WIDTH,
            layout::WINDOW_MIN_HEIGHT,
        )),
        ..TitleBar::window_options()
    };
    // Centre the macOS window buttons in our taller title bar.
    if let Some(titlebar) = options.titlebar.as_mut() {
        let (x, y) = layout::TRAFFIC_LIGHTS;
        titlebar.traffic_light_position = Some(gpui_kit::point(x, y));
    }
    options
}

/// Opens a workspace window showing `project` (the last one shown when
/// `None`).
pub fn open_window(
    project: Option<ProjectId>,
    cx: &mut App,
) -> Result<(AnyWindowHandle, Entity<Workspace>)> {
    let options = window_options(cx);
    let (handle, workspace) = gpui_kit::open_window(options, cx, move |window, cx| {
        // Apply the saved appearance before the first frame so there is no flash.
        ph_theme::apply_appearance(AppState::appearance(cx), Some(window), cx);
        cx.new(|cx| Workspace::new_showing(project, window, cx))
    })?;
    register(handle, &workspace, cx);
    Ok((handle, workspace))
}

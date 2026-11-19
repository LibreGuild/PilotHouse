//! The width of the list beside Reviews (its inbox and repositories), which
//! the user drags, and how long ago something happened, in a word.
//!
//! This was the sidebar: the list of workspaces and their chats beside the
//! screen in front. The Bridge lists every task now, and the column of icons
//! holds the workspaces; what is left here is what other views still share.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{
    App, AppContext as _, Context, InteractiveElement as _, IntoElement, Pixels, Render,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use ph_theme::layout;

/// The sidebar's width, as the user last dragged it (the workspace's and
/// the GitHub view's sidebars share it).
#[derive(Clone, Copy)]
struct SidebarWidth {
    width: Pixels,
    /// Bumped by each drag step: only the last one is saved.
    edit: u64,
}

impl gpui_kit::Global for SidebarWidth {}

/// Reads the saved width (at startup).
pub(crate) fn load_sizes(cx: &mut App) {
    let store = crate::AppState::store(cx);
    let saved: f32 = store.setting(crate::app_state::keys::SIDEBAR_WIDTH);
    let width = if saved > 0. {
        px(saved).clamp(layout::SIDEBAR_MIN, layout::SIDEBAR_MAX)
    } else {
        layout::SIDEBAR_WIDTH
    };
    cx.set_global(SidebarWidth { width, edit: 0 });
}

pub(crate) fn width(cx: &App) -> Pixels {
    cx.try_global::<SidebarWidth>()
        .map_or(layout::SIDEBAR_WIDTH, |sidebar| sidebar.width)
}

/// What is dragged when the sidebar's edge is.
pub(crate) struct SidebarEdge;

impl Render for SidebarEdge {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui_kit::Empty
    }
}

/// The grip that resizes the list: the gap to the right of its island
/// (`frame::list_island`); the window passes the drag to [`drag_width`].
pub(crate) fn resize_edge(cx: &App) -> impl IntoElement {
    let hover = cx.theme().border;
    let gap = crate::frame::gap();
    div()
        .id("sidebar-resize")
        .absolute()
        .top_0()
        .right_0()
        .bottom(gap)
        .w(gap)
        .rounded_full()
        .cursor_col_resize()
        .hover(move |style| style.bg(hover))
        .on_drag(SidebarEdge, |_, _, _, cx| cx.new(|_| SidebarEdge))
}

/// The edge was dragged to `width`: the sidebars follow, and the width is
/// saved once the drag rests.
pub(crate) fn drag_width(width: Pixels, cx: &mut App) {
    let width = width.clamp(layout::SIDEBAR_MIN, layout::SIDEBAR_MAX);
    let edit = cx
        .try_global::<SidebarWidth>()
        .map_or(0, |sidebar| sidebar.edit)
        + 1;
    cx.set_global(SidebarWidth { width, edit });
    cx.refresh_windows();
    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(std::time::Duration::from_millis(400))
            .await;
        cx.update(|cx| {
            if cx
                .try_global::<SidebarWidth>()
                .is_some_and(|sidebar| sidebar.edit == edit)
            {
                _ = crate::AppState::store(cx)
                    .set_setting(crate::app_state::keys::SIDEBAR_WIDTH, &f32::from(width));
            }
        });
    })
    .detach();
}

/// How long ago `millis` was, as lists say it: *now*, *5m*, *3h*, *Tue*,
/// *Mar 4*.
pub(crate) fn age(millis: i64) -> String {
    let Ok(at) = jiff::Timestamp::from_millisecond(millis) else {
        return String::new();
    };
    let now = jiff::Timestamp::now();
    let minutes = (now.as_second() - at.as_second()) / 60;
    match minutes {
        ..1 => "now".into(),
        1..60 => format!("{minutes}m"),
        60..1440 => format!("{}h", minutes / 60),
        1440..10080 => at
            .to_zoned(jiff::tz::TimeZone::system())
            .strftime("%a")
            .to_string(),
        _ => at
            .to_zoned(jiff::tz::TimeZone::system())
            .strftime("%b %-d")
            .to_string(),
    }
}

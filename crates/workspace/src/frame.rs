//! The window's frame: the title bar, the two columns of icons and the
//! status bar are one piece, with no line between them, around the islands
//! in the middle (the list, the panes), which are what changes with what is
//! shown. This module paints the frame and sets the islands in it.
//!
//! The frame is lit from above: a little lighter at the top than at the
//! bottom, in the theme's own colour. With a workspace in front the light
//! has that workspace's colour, strongest in the title bar and fading down
//! the two columns, so a glance at the window's edge says where a new chat
//! would go. The Chats list and GitHub belong to no workspace and stay
//! plain.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{
    AnyElement, App, Global, Hsla, IntoElement, ParentElement as _, Rems, Rgba, Styled as _,
    Window, div, linear_color_stop, linear_gradient, relative,
};

use crate::sidebar;
use crate::workspace::{Section, View, Workspace};

/// The room between two islands, and under them.
pub(crate) fn gap() -> Rems {
    ph_ui::rems_from_px(6.)
}

/// How strong the workspace's colour is at the top of the frame. Low: it
/// says which workspace, and is not there to be looked at.
const TINT: (f32, f32) = (0.17, 0.15);
/// How far down the window the colour reaches.
const TINT_REACH: f32 = 0.62;
/// How much lighter the top of the frame is, and how much darker its
/// bottom, than the theme's frame colour.
const LIGHT: (f32, f32) = (0.028, 0.018);

/// The frame's paint, as the window being drawn laid it: what an island
/// needs to know of what is behind its corners. A window's root sets it as
/// it renders, before the islands under it do (one global serves every
/// window, each in its turn). None for a frame that shows the desktop or a
/// picture (Settings → Appearance): what is behind is not a colour then.
#[derive(Clone, Copy, Default)]
struct Light(Option<Paint>);

impl Global for Light {}

#[derive(Clone, Copy)]
struct Paint {
    top: Hsla,
    bottom: Hsla,
    /// The workspace's colour and its strength at the very top.
    tint: Option<(Hsla, f32)>,
}

impl Paint {
    /// The frame's colour at `height` of the window (0 its top, 1 its
    /// bottom): what [`layers`] paints there, worked out the way the
    /// gradients blend (plain sRGB).
    fn at(&self, height: f32) -> Hsla {
        let mix = |from: Hsla, to: Hsla, by: f32| {
            let (from, to) = (Rgba::from(from), Rgba::from(to));
            let channel = |from: f32, to: f32| from + (to - from) * by;
            Hsla::from(Rgba {
                r: channel(from.r, to.r),
                g: channel(from.g, to.g),
                b: channel(from.b, to.b),
                a: 1.,
            })
        };
        let base = mix(self.top, self.bottom, height.clamp(0., 1.));
        match self.tint {
            Some((color, strength)) if height < TINT_REACH => {
                mix(base, color, strength * (1. - height / TINT_REACH))
            }
            _ => base,
        }
    }
}

/// What is painted behind the frame, under everything else in the window.
pub(crate) fn layers(workspace: &Workspace, cx: &mut App) -> Vec<AnyElement> {
    let theme = cx.theme();
    let dark = theme.is_dark();
    let layer = || div().absolute().top_0().left_0().w_full();
    let tint = tint(workspace, cx).map(|color| (color, if dark { TINT.0 } else { TINT.1 }));
    let mut layers = Vec::new();
    // Only a frame of the theme's own colour is lit: one that shows the
    // desktop or a picture has that behind it instead.
    let frame = theme.sidebar;
    let paint = (frame.a >= 1.).then(|| {
        let shade = |by: f32| Hsla {
            l: (frame.l + by).clamp(0., 1.),
            ..frame
        };
        Paint {
            top: shade(LIGHT.0),
            bottom: shade(-LIGHT.1),
            tint,
        }
    });
    if let Some(paint) = paint {
        layers.push(
            layer()
                .h_full()
                .bg(linear_gradient(
                    180.,
                    linear_color_stop(paint.top, 0.),
                    linear_color_stop(paint.bottom, 1.),
                ))
                .into_any_element(),
        );
    }
    if let Some((color, strength)) = tint {
        layers.push(
            layer()
                .h(relative(TINT_REACH))
                .bg(linear_gradient(
                    180.,
                    linear_color_stop(color.opacity(strength), 0.),
                    // The same colour, clear: fading to black would grey it.
                    linear_color_stop(color.opacity(0.), 1.),
                ))
                .into_any_element(),
        );
    }
    cx.set_global(Light(paint));
    layers
}

/// The colour of the workspace in front, when one is.
pub(crate) fn tint(workspace: &Workspace, cx: &App) -> Option<Hsla> {
    if workspace.view() != View::Workspaces || workspace.section() != Section::Workspaces {
        return None;
    }
    let id = workspace.active_project_id()?;
    let project = workspace
        .projects()
        .iter()
        .find(|project| project.id == id)?;
    Some(ph_ui::workspace_color(
        project.shade(ph_ui::WORKSPACE_COLORS.len()),
        cx.theme().is_dark(),
    ))
}

/// An island of the frame: a rounded panel between the title bar and the
/// status bar, its corners squared with the frame behind them.
pub(crate) fn island(window: &Window, cx: &App) -> ph_ui::Surface {
    let surface = ph_ui::Surface::new();
    let Some(paint) = cx.try_global::<Light>().and_then(|light| light.0) else {
        return surface;
    };
    // Every island starts under the title bar and ends over the status bar
    // (about: the frame hardly changes over a few pixels down there).
    let height = f32::from(window.viewport_size().height).max(1.);
    let top = f32::from(ph_theme::layout::TITLE_BAR_HEIGHT) / height;
    let bottom = 1. - f32::from(window.rem_size()) * 2. / height;
    surface.behind(paint.at(top), paint.at(bottom))
}

/// The list beside what is in front (the workspaces, the chats, GitHub's
/// repositories) as an island, of the width the lists share, with the grip
/// that resizes it in the gap to its right.
pub(crate) fn list_island(list: impl IntoElement, window: &Window, cx: &App) -> impl IntoElement {
    div()
        .relative()
        .flex_none()
        .h_full()
        .pr(gap())
        .pb(gap())
        .child(
            div()
                .w(sidebar::width(cx))
                .h_full()
                .child(island(window, cx).child(list)),
        )
        .child(sidebar::resize_edge(cx))
}

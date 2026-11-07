//! A drawn diagram (a Mermaid one, in a reply or a document) fitted to its
//! column. On hover, buttons at its top right zoom it out and in where it
//! is, fit it again, open it large in a dialog and copy its source; a pinch
//! on the trackpad, or ⌘/Ctrl and the wheel, zoom it too.

use std::sync::Arc;

use gpui_kit::component::{
    ActiveTheme as _, Icon, Selectable as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, ElementId, Entity, Image, InteractiveElement as _, IntoElement, MouseButton, ObjectFit,
    ParentElement as _, Pixels, Point, RenderOnce, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, StyledImage as _, Window, div, img, point, px,
};

use crate::{CopyButton, Icons};

/// Zoomed where it is, a diagram grows to at most this tall, then scrolls
/// (unless it was taller already when fitted).
const ZOOMED_HEIGHT: f32 = 480.;
const MIN_SCALE: f32 = 0.1;
const MAX_SCALE: f32 = 8.;
/// One press of a zoom button.
const STEP: f32 = 1.25;

/// A diagram's picture with its zoom buttons.
#[derive(Clone, IntoElement)]
pub struct DiagramCard {
    id: ElementId,
    image: Arc<Image>,
    shape: Shape,
    source: SharedString,
}

/// What zooming needs to know of a diagram and where it is shown.
#[derive(Clone, Copy)]
struct Shape {
    /// The picture's own size, in pixels.
    width: f32,
    height: f32,
    /// The widest it is drawn when fitted in a column.
    max_width: f32,
    /// Shown in the dialog: it fills it, and fits both ways.
    expanded: bool,
}

/// How a diagram is zoomed, kept by its id from frame to frame.
struct Zoom {
    /// Against the picture's own size; `None` fits it.
    scale: Option<f32>,
    /// The view the picture scrolls in once it is larger.
    scroll: ScrollHandle,
}

impl DiagramCard {
    /// `image` is `width` × `height` pixels, drawn from `source`; fitted,
    /// it is at most `max_width` wide (and never larger than its own size).
    /// `id` keeps its zoom while it is shown, so it comes from what the
    /// diagram belongs to, never its place in a list.
    pub fn new(
        id: impl Into<ElementId>,
        image: Arc<Image>,
        (width, height): (f32, f32),
        source: impl Into<SharedString>,
        max_width: f32,
    ) -> Self {
        Self {
            id: id.into(),
            image,
            shape: Shape {
                width: width.max(1.),
                height: height.max(1.),
                max_width,
                expanded: false,
            },
            source: source.into(),
        }
    }

    /// The same diagram for the dialog: zoomed on its own, with ids of its own.
    fn expanded(mut self) -> Self {
        self.id = (self.id, "large").into();
        self.shape.expanded = true;
        self
    }

    /// The diagram large, in a dialog most of the window's size.
    fn open(self, window: &mut Window, cx: &mut App) {
        window.open_dialog(cx, move |dialog, window, _| {
            let view = window.viewport_size();
            dialog.title("Diagram").w(view.width * 0.9).child(
                div()
                    .w_full()
                    .h(view.height * 0.72)
                    .child(self.clone().expanded()),
            )
        });
    }
}

impl Shape {
    /// The scale that fits it in a view `view` wide (and, in the dialog,
    /// `view_height` tall).
    fn fit(&self, view: f32, view_height: f32) -> f32 {
        if self.expanded {
            (view / self.width).min(view_height / self.height)
        } else {
            (view.min(self.max_width) / self.width).min(1.)
        }
    }

    /// How tall its view is at `scale` in a column `view` wide (the dialog
    /// keeps its own height).
    fn view_height(&self, scale: f32, view: f32) -> f32 {
        let fitted = self.height * self.fit(view, 0.);
        (self.height * scale).min(ZOOMED_HEIGHT.max(fitted))
    }
}

impl Zoom {
    /// Zooms by `factor` around `at` (a point in the window; the middle of
    /// the view without one): the part of the picture there stays there.
    fn zoom_by(&mut self, shape: Shape, factor: f32, at: Option<Point<Pixels>>) {
        let bounds = self.scroll.bounds();
        let (view_w, view_h) = (
            f32::from(bounds.size.width).max(1.),
            f32::from(bounds.size.height).max(1.),
        );
        let before = self.scale.unwrap_or_else(|| shape.fit(view_w, view_h));
        let after = (before * factor).clamp(MIN_SCALE, MAX_SCALE);
        let at = at.map_or((view_w / 2., view_h / 2.), |at| {
            (
                f32::from(at.x - bounds.origin.x),
                f32::from(at.y - bounds.origin.y),
            )
        });
        let new_view_h = if shape.expanded {
            view_h
        } else {
            shape.view_height(after, view_w)
        };
        let offset = self.scroll.offset();
        let x = keep_under(
            at.0,
            f32::from(offset.x),
            view_w,
            shape.width * before,
            shape.width * after,
        );
        let y = keep_under(
            at.1.min(new_view_h),
            f32::from(offset.y),
            new_view_h,
            shape.height * before,
            shape.height * after,
        );
        self.scroll.set_offset(point(px(x), px(y)));
        self.scale = Some(after);
    }

    fn fit(&mut self) {
        self.scale = None;
        self.scroll.set_offset(point(px(0.), px(0.)));
    }
}

/// The scroll offset (zero or less) along one side that keeps the point of
/// a picture under `at` (from the view's edge) in place while its drawn
/// length goes from `old` to `new`, in a view `view` long that centers it
/// while it is shorter.
fn keep_under(at: f32, offset: f32, view: f32, old: f32, new: f32) -> f32 {
    let lead = |length: f32| ((view - length) / 2.).max(0.);
    let fraction = ((at - offset - lead(old)) / old.max(1.)).clamp(0., 1.);
    (at - lead(new) - fraction * new).clamp((view - new).min(0.), 0.)
}

impl RenderOnce for DiagramCard {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let zoom = window.use_keyed_state(self.id.clone(), cx, |_, _| Zoom {
            scale: None,
            scroll: ScrollHandle::new(),
        });
        let (scale, scroll) = {
            let zoom = zoom.read(cx);
            (zoom.scale, zoom.scroll.clone())
        };
        let shape = self.shape;
        let theme = cx.theme();
        let view_w = f32::from(scroll.bounds().size.width);

        let view = div()
            .id("diagram-view")
            .w_full()
            .track_scroll(&scroll)
            .on_pinch({
                let zoom = zoom.clone();
                move |event, _, cx| {
                    if event.delta != 0. {
                        update(&zoom, cx, |zoom| {
                            zoom.zoom_by(shape, 1. + event.delta, Some(event.position))
                        });
                    }
                    cx.stop_propagation();
                }
            })
            .on_scroll({
                let zoom = zoom.clone();
                move |event, window, cx| {
                    if !event.modifiers.secondary(cx) {
                        return;
                    }
                    let dy = f32::from(event.delta.pixel_delta(window.line_height()).y);
                    if dy != 0. {
                        let factor = (dy * 0.004).clamp(-0.4, 0.4).exp();
                        update(&zoom, cx, |zoom| {
                            zoom.zoom_by(shape, factor, Some(event.position))
                        });
                    }
                    cx.stop_propagation();
                }
            });
        let view = match scale {
            // As large as fits, keeping its shape.
            None if shape.expanded => view.h_full().child(
                img(self.image.clone())
                    .size_full()
                    .object_fit(ObjectFit::Contain),
            ),
            // As wide as the column allows (at most its own size), and as
            // tall as that width keeps its shape.
            None => {
                let mut fitted = div().w_full().max_w(px(shape.width.min(shape.max_width)));
                fitted.style().aspect_ratio = Some(shape.width / shape.height);
                view.flex().justify_center().child(
                    fitted.child(
                        img(self.image.clone())
                            .size_full()
                            .object_fit(ObjectFit::Contain),
                    ),
                )
            }
            // Its size at this scale, centered while smaller than the view
            // and scrollable once larger.
            Some(scale) => {
                let (width, height) = (shape.width * scale, shape.height * scale);
                let view = if shape.expanded {
                    view.h_full()
                } else {
                    view.h(px(shape.view_height(scale, view_w)))
                };
                view.overflow_scroll().child(
                    div()
                        .w(px(width))
                        .h(px(height))
                        .min_w_full()
                        .min_h_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            img(self.image.clone())
                                .flex_none()
                                .w(px(width))
                                .h(px(height))
                                .object_fit(ObjectFit::Contain),
                        ),
                )
            }
        };

        // Each card's buttons have ids of their own (one diagram can show in
        // a chat and in a pull request at once).
        let child = |name: &'static str| ElementId::from((self.id.clone(), name));
        let button = |id: &'static str, icon: Icon, label: &'static str| {
            Button::new(child(id))
                .ghost()
                .xsmall()
                .icon(icon)
                .accessibility_label(label)
                .tooltip(label)
        };
        let zoom_button = |id, icon: Icons, label, factor: f32| {
            let zoom = zoom.clone();
            button(id, Icon::new(icon), label).on_click(move |_, _, cx| {
                update(&zoom, cx, |zoom| zoom.zoom_by(shape, factor, None))
            })
        };
        let expand = (!shape.expanded).then(|| {
            let card = self.clone();
            button(
                "expand",
                Icon::new(Icons::Maximize2),
                "Open the diagram large",
            )
            .on_click(move |_, window, cx| card.clone().open(window, cx))
        });
        let tools = h_flex()
            .absolute()
            .top_1()
            .right_1()
            .gap_0p5()
            .p_0p5()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .shadow_sm()
            .when(!shape.expanded, |this| {
                this.invisible()
                    .group_hover("diagram-card", |style| style.visible())
            })
            // A click here is for the buttons only (a document's block
            // under it would start editing on a double click).
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(zoom_button(
                "zoom-out",
                Icons::ZoomOut,
                "Zoom out (pinch, or ⌘ and scroll)",
                1. / STEP,
            ))
            .child(zoom_button(
                "zoom-in",
                Icons::ZoomIn,
                "Zoom in (pinch, or ⌘ and scroll)",
                STEP,
            ))
            .child({
                let zoom = zoom.clone();
                button("fit", Icon::new(Icons::Scan), "Fit")
                    .selected(scale.is_none())
                    .on_click(move |_, _, cx| update(&zoom, cx, Zoom::fit))
            })
            .children(expand)
            .child(
                CopyButton::new(child("copy"), self.source.to_string())
                    .tooltip("Copy the diagram's source"),
            );

        div()
            .id(self.id)
            .group("diagram-card")
            .relative()
            .w_full()
            .when(shape.expanded, |this| this.h_full())
            .child(view)
            .child(tools)
    }
}

fn update(zoom: &Entity<Zoom>, cx: &mut App, change: impl FnOnce(&mut Zoom)) {
    zoom.update(cx, |zoom, cx| {
        change(zoom);
        cx.notify();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zooming_keeps_the_point_under_the_pointer() {
        // A 400 px picture centered in a 1000 px view (300 px each side),
        // zoomed to 2000 px around its middle: its middle stays.
        let offset = keep_under(500., 0., 1000., 400., 2000.);
        assert!((offset + 1000. - 500.).abs() < 0.01, "{offset}");
        // Around a quarter of the way in: that point stays.
        let offset = keep_under(400., 0., 1000., 400., 2000.);
        assert!((offset + 500. - 400.).abs() < 0.01, "{offset}");
        // Never scrolled past either end.
        assert_eq!(keep_under(500., -900., 1000., 2000., 400.), 0.);
        assert!(keep_under(999., 0., 1000., 400., 100_000.) >= 1000. - 100_000.);
    }

    #[test]
    fn fitted_and_zoomed_sizes() {
        let shape = Shape {
            width: 1200.,
            height: 300.,
            max_width: 680.,
            expanded: false,
        };
        // In a 500 px column it fits the column; in a wide one, its maximum.
        assert_eq!(shape.fit(500., 0.), 500. / 1200.);
        assert_eq!(shape.fit(2000., 0.), 680. / 1200.);
        // Zoomed in, its view grows to a cap, then scrolls.
        assert_eq!(shape.view_height(1., 500.), 300.);
        assert_eq!(shape.view_height(4., 500.), ZOOMED_HEIGHT);
        // The dialog fits it both ways.
        let large = Shape {
            expanded: true,
            ..shape
        };
        assert_eq!(large.fit(2400., 300.), 1.);
        assert_eq!(large.fit(600., 900.), 0.5);
    }
}

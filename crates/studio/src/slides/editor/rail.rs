//! The slides down the left: a small picture of each (made only for those
//! in view), dragged to reorder, right-clicked for what can be done to a
//! slide, with the deck's sections marked where they start.

use std::ops::Range;

use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    menu::{ContextMenuExt as _, PopupMenuItem},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, Pixels, Point, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, StyledImage as _, Window, div, img, px, uniform_list,
};
use ph_ui::{Icons, MenuButton as _};
use serde_json::json;

use super::SlidesEditor;
use crate::slides::document::Purpose;
use crate::slides::ops::Op;

/// The rail's width, and its pictures' widest and tallest.
const WIDTH: f32 = 196.;
const THUMB_WIDTH: f32 = 148.;
const THUMB_HEIGHT: f32 = 150.;
/// Pictures kept this many slides either side of those in view.
const KEEP: usize = 12;

/// A slide being dragged to another place.
#[derive(Clone)]
struct DraggedSlide {
    index: usize,
    label: SharedString,
}

struct DragPreview {
    label: SharedString,
}

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded(cx.theme().radius)
            .bg(cx.theme().popover)
            .border_1()
            .border_color(cx.theme().border)
            .shadow_md()
            .text_sm()
            .child(self.label.clone())
    }
}

impl SlidesEditor {
    /// The size of the rail's pictures (points of screen).
    fn thumb_size(&self, cx: &App) -> (f32, f32) {
        let size = &self.deck(cx).size;
        let aspect = size.width / size.height.max(1.);
        let w = THUMB_WIDTH.min(THUMB_HEIGHT * aspect);
        (w, w / aspect)
    }

    pub(crate) fn render_rail(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // What was in view last frame decides which pictures stay.
        if let Some(view) = self.rail_view.take() {
            self.keep_rail(view, cx);
        }
        let theme = cx.theme().clone();
        let count = self.deck(cx).slides.len();
        let list = uniform_list(
            "slides-rail",
            count,
            cx.processor(|this, range: Range<usize>, window, cx| {
                // The list also asks for its first row alone, to measure
                // it: the longest range asked for is the one in view.
                if this
                    .rail_view
                    .as_ref()
                    .is_none_or(|seen| range.len() > seen.len())
                {
                    this.rail_view = Some(range.clone());
                }
                range
                    .map(|index| this.render_thumb(index, window, cx))
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.rail)
        .flex_1()
        .py_2();
        let layouts: Vec<(String, String)> = self
            .deck(cx)
            .theme
            .layouts
            .iter()
            .map(|layout| (layout.id.clone(), layout.name.clone()))
            .collect();
        let editor = cx.weak_entity();
        v_flex()
            .flex_none()
            .w(px(WIDTH))
            .h_full()
            .border_r_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .child(list)
            .child(
                div().p_2().border_t_1().border_color(theme.border).child(
                    Button::new("slides-rail-new")
                        .ghost()
                        .small()
                        .icon(Icons::Plus)
                        .label("New slide")
                        .dropdown_caret(true)
                        .menu(gpui_kit::Anchor::BottomLeft, move |menu, _, _| {
                            layouts.iter().fold(menu, |menu, (id, name)| {
                                let (editor, id) = (editor.clone(), id.clone());
                                menu.item(PopupMenuItem::new(name.clone()).on_click(
                                    move |_, window, cx| {
                                        _ = editor.update(cx, |this, cx| {
                                            this.new_slide(Some(id.clone()), window, cx)
                                        });
                                    },
                                ))
                            })
                        }),
                ),
            )
            .into_any_element()
    }

    /// Lets go of the pictures of slides far from view.
    fn keep_rail(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        let deck = self.deck(cx);
        let from = range.start.saturating_sub(KEEP);
        let to = (range.end + KEEP).min(deck.slides.len());
        let keep: std::collections::HashSet<String> = deck.slides[from..to]
            .iter()
            .map(|slide| slide.id.clone())
            .collect();
        self.doc
            .update(cx, |doc, _| doc.forget_rail(&|id| keep.contains(id)));
    }

    fn render_thumb(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let (w, h) = self.thumb_size(cx);
        let pixels = (w * window.scale_factor()).round() as u32;
        let image = self.doc.update(cx, |doc, cx| {
            doc.picture(index, Purpose::Rail, pixels, None, cx)
        });
        let deck = self.deck(cx);
        let slide = deck.slides[index].clone();
        let section = deck
            .sections
            .iter()
            .find(|section| section.slide == slide.id)
            .map(|section| section.name.clone());
        let current = index == self.current;
        let title = slide
            .elements
            .iter()
            .find(|element| element.placeholder.is_some_and(|kind| kind.is_title()))
            .and_then(|element| element.kind.text())
            .map(|text| text.plain())
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| format!("Slide {}", index + 1));
        let dragged = DraggedSlide {
            index,
            label: title.clone().into(),
        };
        let editor = cx.weak_entity();
        let count = deck.slides.len();
        let hidden = slide.hidden;
        div()
            .id(("slide-thumb", index))
            .w_full()
            .px_2()
            .py(px(5.))
            .child(
                gpui_kit::component::h_flex()
                    .items_start()
                    .gap_1()
                    .child(
                        v_flex()
                            .w(px(22.))
                            .items_end()
                            .pt_0p5()
                            .gap_1()
                            .text_xs()
                            .text_color(if current {
                                theme.foreground
                            } else {
                                theme.muted_foreground
                            })
                            .child((index + 1).to_string())
                            .when(hidden, |this| {
                                this.child(
                                    gpui_kit::component::Icon::new(Icons::EyeOff)
                                        .xsmall()
                                        .text_color(theme.muted_foreground),
                                )
                            }),
                    )
                    .child(
                        div()
                            .relative()
                            .w(px(w))
                            .h(px(h))
                            .rounded(px(3.))
                            .overflow_hidden()
                            .border_2()
                            .border_color(if current {
                                theme.ring
                            } else {
                                theme.border.opacity(0.6)
                            })
                            .bg(theme.muted)
                            .when(hidden, |this| this.opacity(0.5))
                            .children(
                                image.map(|image| {
                                    img(image).size_full().object_fit(ObjectFit::Fill)
                                }),
                            )
                            .when_some(section, |this, name| {
                                this.child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .left_0()
                                        .max_w_full()
                                        .px_1()
                                        .rounded_br(px(3.))
                                        .bg(theme.ring)
                                        .text_color(gpui_kit::white())
                                        .text_xs()
                                        .truncate()
                                        .child(name),
                                )
                            }),
                    ),
            )
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| this.go_to(index, window, cx)))
            .on_drag(
                dragged,
                |dragged: &DraggedSlide, _: Point<Pixels>, _, cx: &mut App| {
                    let label = dragged.label.clone();
                    cx.new(|_| DragPreview { label })
                },
            )
            .drag_over::<DraggedSlide>(|style, _, _, cx| {
                style.border_t_2().border_color(cx.theme().ring)
            })
            .on_drop(
                cx.listener(move |this, dragged: &DraggedSlide, window, cx| {
                    this.move_slide(dragged.index, index, window, cx);
                }),
            )
            .context_menu(move |menu, _, _| {
                let action = |label: &'static str,
                              run: fn(
                    &mut SlidesEditor,
                    usize,
                    &mut Window,
                    &mut Context<SlidesEditor>,
                )| {
                    let editor = editor.clone();
                    PopupMenuItem::new(label).on_click(move |_, window, cx| {
                        _ = editor.update(cx, |this, cx| run(this, index, window, cx));
                    })
                };
                menu.item(action("New slide after", |this, index, window, cx| {
                    this.go_to(index, window, cx);
                    this.new_slide(None, window, cx);
                }))
                .item(action("Duplicate", |this, index, window, cx| {
                    let id = this.deck(cx).slides[index].id.clone();
                    if this
                        .apply(
                            "duplicate",
                            vec![Op::DuplicateSlide { slide: json!(id) }],
                            window,
                            cx,
                        )
                        .is_some()
                    {
                        this.go_to(index + 1, window, cx);
                    }
                }))
                .item(action(
                    if hidden {
                        "Show in presenting"
                    } else {
                        "Hide in presenting"
                    },
                    |this, index, window, cx| {
                        let slide = &this.deck(cx).slides[index];
                        let (id, hidden) = (slide.id.clone(), !slide.hidden);
                        this.apply(
                            "hide",
                            vec![Op::UpdateSlide {
                                slide: json!(id),
                                layout: None,
                                background: None,
                                notes: None,
                                hidden: Some(hidden),
                                transition: None,
                                transition_from: None,
                                transition_seconds: None,
                            }],
                            window,
                            cx,
                        );
                    },
                ))
                .item(action(
                    "Start a section here…",
                    |this, index, window, cx| {
                        this.open_section(index, window, cx);
                    },
                ))
                .item(action("Present from here", |this, index, window, cx| {
                    this.present_from(index, window, cx)
                }))
                .separator()
                .item(
                    action("Move up", |this, index, window, cx| {
                        if index > 0 {
                            this.move_slide(index, index - 1, window, cx);
                        }
                    })
                    .disabled(index == 0),
                )
                .item(
                    action("Move down", |this, index, window, cx| {
                        this.move_slide(index, index + 1, window, cx);
                    })
                    .disabled(index + 1 >= count),
                )
                .separator()
                .item(action("Delete slide", |this, index, window, cx| {
                    this.delete_slide(index, window, cx)
                }))
            })
            .into_any_element()
    }

    /// Moves slide `from` to where `to` is.
    pub(crate) fn move_slide(
        &mut self,
        from: usize,
        to: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if from == to {
            return;
        }
        let Some(id) = self.deck(cx).slides.get(from).map(|slide| slide.id.clone()) else {
            return;
        };
        self.apply(
            "move slide",
            vec![Op::MoveSlide {
                slide: json!(id),
                to: to + 1,
            }],
            window,
            cx,
        );
        self.go_to(to, window, cx);
    }

    pub(crate) fn delete_slide(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self
            .deck(cx)
            .slides
            .get(index)
            .map(|slide| slide.id.clone())
        else {
            return;
        };
        self.finish_editing(window, cx);
        self.apply(
            "delete slide",
            vec![Op::RemoveSlide { slide: json!(id) }],
            window,
            cx,
        );
        let count = self.deck(cx).slides.len();
        self.go_to(index.min(count.saturating_sub(1)), window, cx);
    }
}

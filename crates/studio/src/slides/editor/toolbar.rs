//! The bar across the top: what can be added to the slide, and the
//! deck's layout, theme and size; the zoom and the notes on the right.

use gpui_kit::component::{
    ActiveTheme as _, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::PopupMenuItem,
};
use gpui_kit::{
    AnyElement, Context, IntoElement, ParentElement as _, Styled as _, Window, div, px,
};
use ph_ui::{Icons, MenuButton as _};
use serde_json::json;

use super::SlidesEditor;
use crate::slides::model::SIZES;
use crate::slides::ops::Op;

/// The shapes offered: their name in the model and to the user.
pub(crate) const SHAPES: &[(&str, &str)] = &[
    ("rect", "Rectangle"),
    ("round_rect", "Rounded rectangle"),
    ("ellipse", "Oval"),
    ("triangle", "Triangle"),
    ("right_triangle", "Right triangle"),
    ("diamond", "Diamond"),
    ("parallelogram", "Parallelogram"),
    ("trapezoid", "Trapezoid"),
    ("pentagon", "Pentagon"),
    ("hexagon", "Hexagon"),
    ("octagon", "Octagon"),
    ("chevron", "Chevron"),
    ("home_plate", "Arrow box"),
    ("right_arrow", "Right arrow"),
    ("left_arrow", "Left arrow"),
    ("up_arrow", "Up arrow"),
    ("down_arrow", "Down arrow"),
    ("left_right_arrow", "Left-right arrow"),
    ("star5", "Star"),
    ("star6", "Six-point star"),
    ("plus", "Plus"),
    ("heart", "Heart"),
    ("callout", "Speech bubble"),
];

/// The charts offered.
pub(crate) const CHARTS: &[(&str, &str)] = &[
    ("column", "Column"),
    ("stacked_column", "Stacked column"),
    ("bar", "Bar"),
    ("line", "Line"),
    ("area", "Area"),
    ("pie", "Pie"),
    ("doughnut", "Doughnut"),
    ("scatter", "Scatter"),
];

impl SlidesEditor {
    pub(crate) fn insert_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let width = self.deck(cx).size.width * 0.5;
        let frame = self.centered(width, 64., cx);
        if let Some(id) = self.insert(
            json!({ "type": "text", "text": "", "frame": frame }),
            window,
            cx,
        ) {
            self.start_editing(&id, window, cx);
        }
    }

    pub(crate) fn insert_shape(
        &mut self,
        geometry: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let element = match geometry {
            "line" | "arrow" => {
                let mut frame = self.centered(260., 0., cx);
                frame["h"] = json!(0.);
                json!({
                    "type": "shape", "geometry": "line", "frame": frame,
                    "line": { "width": 3, "color": "accent1", "tail": geometry == "arrow" },
                })
            }
            _ => json!({
                "type": "shape", "geometry": geometry, "fill": "accent1",
                "frame": self.centered(200., 140., cx),
            }),
        };
        self.insert(element, window, cx);
    }

    pub(crate) fn insert_table(
        &mut self,
        columns: usize,
        rows: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cells: Vec<Vec<serde_json::Value>> = (0..rows)
            .map(|row| {
                (0..columns)
                    .map(|column| {
                        let text = if row == 0 {
                            format!("Heading {}", column + 1)
                        } else {
                            String::new()
                        };
                        json!({ "text": text })
                    })
                    .collect()
            })
            .collect();
        let width = self.deck(cx).size.width * 0.75;
        let frame = self.centered(width, rows as f32 * 38., cx);
        self.insert(
            json!({ "type": "table", "columns": vec![1; columns], "rows": cells, "header": true, "banded": true, "frame": frame }),
            window,
            cx,
        );
    }

    pub(crate) fn insert_chart(
        &mut self,
        chart: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (categories, series) = match chart {
            "pie" | "doughnut" => (
                json!(["First", "Second", "Third"]),
                json!([{ "name": "Share", "values": [50, 30, 20] }]),
            ),
            "scatter" => (
                json!([]),
                json!([{ "name": "Series 1", "x": [1, 2, 3, 4, 5], "values": [2, 4, 3, 6, 7] }]),
            ),
            _ => (
                json!(["Q1", "Q2", "Q3", "Q4"]),
                json!([
                    { "name": "Series 1", "values": [4, 6, 5, 8] },
                    { "name": "Series 2", "values": [3, 4, 6, 7] }
                ]),
            ),
        };
        let size = &self.deck(cx).size;
        let frame = self.centered(size.width * 0.6, size.height * 0.6, cx);
        let made = self.insert(
            json!({ "type": "chart", "chart": chart, "categories": categories, "series": series, "labels": false, "frame": frame }),
            window,
            cx,
        );
        if let Some(id) = made {
            self.open_chart(id, window, cx);
        }
    }

    pub(crate) fn insert_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let frame = self.centered(460., 180., cx);
        let made = self.insert(
            json!({ "type": "code", "language": "rust", "code": "fn main() {\n    println!(\"Hello\");\n}", "frame": frame }),
            window,
            cx,
        );
        if let Some(id) = made {
            self.start_editing(&id, window, cx);
        }
    }

    pub(crate) fn set_layout(
        &mut self,
        layout: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(slide) = self
            .deck(cx)
            .slides
            .get(self.current)
            .map(|slide| slide.id.clone())
        else {
            return;
        };
        self.apply(
            "layout",
            vec![Op::UpdateSlide {
                slide: json!(slide),
                layout: Some(layout),
                background: None,
                notes: None,
                hidden: None,
                transition: None,
                transition_from: None,
                transition_seconds: None,
            }],
            window,
            cx,
        );
    }

    pub(crate) fn set_theme(
        &mut self,
        theme: Option<String>,
        saved: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.finish_editing(window, cx);
        self.apply(
            "theme",
            vec![Op::SetTheme {
                theme,
                custom: None,
                saved,
                colors: serde_json::Value::Null,
                fonts: None,
                background: None,
            }],
            window,
            cx,
        );
    }

    pub(crate) fn set_size(
        &mut self,
        preset: Option<String>,
        width: Option<f32>,
        height: Option<f32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.finish_editing(window, cx);
        self.apply(
            "size",
            vec![Op::SetSize {
                preset,
                width,
                height,
                scale: true,
            }],
            window,
            cx,
        );
        self.fit(cx);
    }

    pub(crate) fn render_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let editor = cx.weak_entity();
        let deck = self.deck(cx);
        let layouts: Vec<(String, String)> = deck
            .theme
            .layouts
            .iter()
            .map(|layout| (layout.id.clone(), layout.name.clone()))
            .collect();
        let current_layout = deck
            .slides
            .get(self.current)
            .map(|slide| slide.layout.clone());
        let theme_name = deck.theme.name.clone();
        let preset = deck.size.preset.clone();
        // Narrow (the chat open, a small window): icons only, their names
        // in the tooltips.
        let width = f32::from(self.bounds.get().size.width) + 196.;
        let compact = width < 1200.;
        let tight = width < 840.;
        let named = |button: Button, label: &'static str, shown: bool| {
            if shown {
                button.label(label)
            } else {
                button.tooltip(label)
            }
        };
        let tool = |id: &'static str, icon: Icons, label: &'static str, tooltip: &'static str| {
            let button = Button::new(id).ghost().small().icon(icon);
            if compact {
                button.tooltip(format!("{label}: {tooltip}"))
            } else {
                button.label(label).tooltip(tooltip)
            }
        };
        let new_slide = {
            let editor = editor.clone();
            let layouts = layouts.clone();
            Button::new("slides-new")
                .ghost()
                .small()
                .icon(Icons::Plus)
                .label("Slide")
                .dropdown_caret(true)
                .tooltip(if cfg!(target_os = "macos") {
                    "A new slide (⌘M)"
                } else {
                    "A new slide (Ctrl+M)"
                })
                .menu(gpui_kit::Anchor::TopLeft, move |menu, _, _| {
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
                })
        };
        let shapes = {
            let editor = editor.clone();
            named(
                Button::new("slides-shapes").ghost().small(),
                "Shape",
                !compact,
            )
            .icon(Icons::Shapes)
            .dropdown_caret(true)
            .menu(gpui_kit::Anchor::TopLeft, move |menu, _, _| {
                let item = |label: &'static str, geometry: &'static str| {
                    let editor = editor.clone();
                    PopupMenuItem::new(label).on_click(move |_, window, cx| {
                        _ = editor.update(cx, |this, cx| this.insert_shape(geometry, window, cx));
                    })
                };
                let menu = menu
                    .item(item("Line", "line"))
                    .item(item("Arrow", "arrow"))
                    .separator();
                SHAPES
                    .iter()
                    .fold(menu, |menu, (geometry, label)| {
                        menu.item(item(label, geometry))
                    })
                    .max_h(px(420.))
                    .scrollable(true)
            })
        };
        let table = {
            let editor = editor.clone();
            named(
                Button::new("slides-table").ghost().small(),
                "Table",
                !compact,
            )
            .icon(Icons::Table)
            .dropdown_caret(true)
            .menu(gpui_kit::Anchor::TopLeft, move |menu, _, _| {
                [(2, 2), (3, 3), (4, 3), (3, 5), (4, 5), (5, 6)]
                    .into_iter()
                    .fold(menu, |menu, (columns, rows)| {
                        let editor = editor.clone();
                        menu.item(
                            PopupMenuItem::new(format!("{columns} columns × {rows} rows"))
                                .on_click(move |_, window, cx| {
                                    _ = editor.update(cx, |this, cx| {
                                        this.insert_table(columns, rows, window, cx)
                                    });
                                }),
                        )
                    })
            })
        };
        let chart = {
            let editor = editor.clone();
            named(
                Button::new("slides-chart").ghost().small(),
                "Chart",
                !compact,
            )
            .icon(Icons::ChartColumn)
            .dropdown_caret(true)
            .menu(gpui_kit::Anchor::TopLeft, move |menu, _, _| {
                CHARTS.iter().fold(menu, |menu, (chart, label)| {
                    let editor = editor.clone();
                    menu.item(PopupMenuItem::new(*label).on_click(move |_, window, cx| {
                        _ = editor.update(cx, |this, cx| this.insert_chart(chart, window, cx));
                    }))
                })
            })
        };
        let layout = {
            let editor = editor.clone();
            named(
                Button::new("slides-layout").ghost().small(),
                "Layout",
                !tight,
            )
            .icon(Icons::LayoutTemplate)
            .dropdown_caret(true)
            .menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                layouts.iter().fold(menu, |menu, (id, name)| {
                    let (editor, id) = (editor.clone(), id.clone());
                    menu.item(
                        PopupMenuItem::new(name.clone())
                            .checked(current_layout.as_ref() == Some(&id))
                            .on_click(move |_, window, cx| {
                                _ = editor
                                    .update(cx, |this, cx| this.set_layout(id.clone(), window, cx));
                            }),
                    )
                })
            })
        };
        let theme_menu = {
            let editor = editor.clone();
            named(Button::new("slides-theme").ghost().small(), "Theme", !tight)
                .icon(Icons::Palette)
                .dropdown_caret(true)
                .menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                    let mut menu = menu.label("Built in");
                    for (id, name) in crate::slides::theme::builtins() {
                        let editor = editor.clone();
                        menu = menu.item(
                            PopupMenuItem::new(name)
                                .checked(theme_name == name)
                                .on_click(move |_, window, cx| {
                                    _ = editor.update(cx, |this, cx| {
                                        this.set_theme(Some(id.to_string()), None, window, cx)
                                    });
                                }),
                        );
                    }
                    let saved =
                        crate::slides::space::saved_themes(&crate::slides::space::themes_dir());
                    if !saved.is_empty() {
                        menu = menu.separator().label("Yours");
                        for (name, _) in saved {
                            let editor = editor.clone();
                            menu = menu.item(
                                PopupMenuItem::new(name.clone())
                                    .checked(theme_name == name)
                                    .on_click(move |_, window, cx| {
                                        _ = editor.update(cx, |this, cx| {
                                            this.set_theme(None, Some(name.clone()), window, cx)
                                        });
                                    }),
                            );
                        }
                    }
                    let (colors, save) = (editor.clone(), editor.clone());
                    menu.separator()
                        .item(PopupMenuItem::new("Colours and fonts…").on_click(
                            move |_, window, cx| {
                                _ = colors
                                    .update(cx, |this, cx| this.open_theme_editor(window, cx));
                            },
                        ))
                        .item(PopupMenuItem::new("Save this theme…").on_click(
                            move |_, window, cx| {
                                _ = save.update(cx, |this, cx| this.open_save_theme(window, cx));
                            },
                        ))
                        .max_h(px(480.))
                        .scrollable(true)
                })
        };
        let size = {
            let editor = editor.clone();
            named(Button::new("slides-size").ghost().small(), "Size", !tight)
                .icon(Icons::Ratio)
                .dropdown_caret(true)
                .menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                    let custom = editor.clone();
                    SIZES
                        .iter()
                        .fold(menu, |menu, (id, name, _, _)| {
                            let editor = editor.clone();
                            menu.item(
                                PopupMenuItem::new(*name)
                                    .checked(preset.as_deref() == Some(*id))
                                    .on_click(move |_, window, cx| {
                                        _ = editor.update(cx, |this, cx| {
                                            this.set_size(
                                                Some(id.to_string()),
                                                None,
                                                None,
                                                window,
                                                cx,
                                            )
                                        });
                                    }),
                            )
                        })
                        .separator()
                        .item(
                            PopupMenuItem::new("Custom size…").on_click(move |_, window, cx| {
                                _ = custom.update(cx, |this, cx| this.open_custom_size(window, cx));
                            }),
                        )
                })
        };
        let percent = self
            .placement
            .map(|placement| (placement.scale * 100.).round() as u32)
            .unwrap_or(100);
        h_flex()
            .flex_none()
            .h(px(40.))
            .px_2()
            .gap_0p5()
            .border_b_1()
            .border_color(theme.border)
            .child(new_slide)
            .child(div().w(px(1.)).h(px(18.)).mx_1().bg(theme.border))
            .child(
                tool("slides-text", Icons::Type, "Text", "A text box")
                    .on_click(cx.listener(|this, _, window, cx| this.insert_text(window, cx))),
            )
            .child(shapes)
            .child(
                tool(
                    "slides-picture",
                    Icons::Image,
                    "Picture",
                    "Pictures from files (or paste one)",
                )
                .on_click(cx.listener(|this, _, window, cx| this.pick_pictures(window, cx))),
            )
            .child(table)
            .child(chart)
            .child(
                tool(
                    "slides-icon",
                    Icons::Sticker,
                    "Icon",
                    "An icon from the library",
                )
                .on_click(cx.listener(|this, _, window, cx| this.open_icons(None, window, cx))),
            )
            .child(
                tool(
                    "slides-code",
                    Icons::Braces,
                    "Code",
                    "Code, coloured as in an editor",
                )
                .on_click(cx.listener(|this, _, window, cx| this.insert_code(window, cx))),
            )
            .child(
                tool(
                    "slides-svg",
                    Icons::FileImage,
                    "SVG",
                    "A drawing from SVG (a file or pasted)",
                )
                .on_click(cx.listener(|this, _, window, cx| this.open_svg(None, window, cx))),
            )
            .child(div().flex_1().min_w_2())
            .child(layout)
            .child(theme_menu)
            .child(size)
            .child(div().w(px(1.)).h(px(18.)).mx_1().bg(theme.border))
            .child(
                Button::new("slides-zoom-out")
                    .ghost()
                    .xsmall()
                    .icon(Icons::Minus)
                    .tooltip("Zoom out")
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_at(1. / 1.25, None, cx))),
            )
            .child(
                Button::new("slides-zoom-fit")
                    .ghost()
                    .xsmall()
                    .label(format!("{percent}%"))
                    .tooltip("Fit the slide")
                    .on_click(cx.listener(|this, _, _, cx| this.fit(cx))),
            )
            .child(
                Button::new("slides-zoom-in")
                    .ghost()
                    .xsmall()
                    .icon(Icons::Plus)
                    .tooltip("Zoom in")
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_at(1.25, None, cx))),
            )
            .child({
                let comments = self
                    .slide(cx)
                    .map(|slide| {
                        slide
                            .comments
                            .iter()
                            .filter(|comment| !comment.resolved)
                            .count()
                    })
                    .unwrap_or(0);
                let button = Button::new("slides-comments")
                    .ghost()
                    .small()
                    .icon(Icons::MessagesSquare)
                    .selected(self.side == Some(super::side::Side::Comments))
                    .tooltip("Comments on this slide")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_side(super::side::Side::Comments, window, cx)
                    }));
                if comments > 0 {
                    button.label(comments.to_string())
                } else {
                    button
                }
            })
            .child(
                Button::new("slides-references")
                    .ghost()
                    .small()
                    .icon(Icons::FolderOpen)
                    .selected(self.side == Some(super::side::Side::References))
                    .tooltip("References: files to work from")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_side(super::side::Side::References, window, cx)
                    })),
            )
            .child(
                Button::new("slides-notes")
                    .ghost()
                    .small()
                    .icon(Icons::StickyNote)
                    .selected(self.notes_shown)
                    .tooltip("Speaker notes")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.notes_shown = !this.notes_shown;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
}

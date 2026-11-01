//! The bar under the slide: only what the selection takes. Text: font,
//! size, bold and the rest, colour, alignment, lists, spacing. Shapes:
//! fill, outline, shadow. Pictures, tables, charts, code and icons their
//! own; several elements: aligning and grouping; nothing: the slide's
//! background and transition. It reads the deck each time it is drawn, so
//! an agent's change (or undo) shows in it at once.

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::PopupMenuItem,
    popover::Popover,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use ph_ui::{Icons, MenuButton as _};
use serde_json::{Value, json};

use super::SlidesEditor;
use super::canvas::to_hsla;
use crate::slides::model::*;
use crate::slides::ops::Op;
use crate::slides::text;

/// The standard colours under the theme's.
const STANDARD: [&str; 10] = [
    "#C00000", "#FF0000", "#FFC000", "#FFFF00", "#92D050", "#00B050", "#00B0F0", "#0070C0",
    "#002060", "#7030A0",
];
/// The theme's slots across, and how each is lightened or darkened down.
const COLUMNS: [&str; 10] = [
    "light1", "dark1", "light2", "dark2", "accent1", "accent2", "accent3", "accent4", "accent5",
    "accent6",
];
const SHADES: [&str; 5] = ["+80", "+60", "+40", "-25", "-50"];
const SIZES: [f32; 16] = [
    10., 12., 14., 16., 18., 20., 24., 28., 32., 36., 40., 48., 54., 60., 72., 96.,
];
/// Families offered besides the theme's, when the system has them.
const FAMILIES: [&str; 16] = [
    "Arial",
    "Helvetica",
    "Helvetica Neue",
    "Inter",
    "Roboto",
    "Segoe UI",
    "Calibri",
    "Aptos",
    "Verdana",
    "Trebuchet MS",
    "Georgia",
    "Times New Roman",
    "Garamond",
    "Palatino",
    "Courier New",
    "Menlo",
];
const LANGUAGES: [&str; 16] = [
    "rust",
    "python",
    "javascript",
    "typescript",
    "go",
    "java",
    "kotlin",
    "swift",
    "c",
    "cpp",
    "csharp",
    "sql",
    "bash",
    "json",
    "yaml",
    "html",
];

/// What a colour pick does.
type Pick = Rc<dyn Fn(Option<String>, &mut Window, &mut App)>;

impl SlidesEditor {
    /// Changes the selected elements with `set` (merged into each).
    pub(crate) fn set_selected(
        &mut self,
        what: &'static str,
        set: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.finish_editing(window, cx);
        let ops = self
            .selected
            .iter()
            .map(|id| Op::UpdateElement {
                id: id.clone(),
                set: set.clone(),
            })
            .collect();
        self.apply(what, ops, window, cx);
    }

    /// Styles all the text of the selected elements.
    pub(crate) fn style_selected(
        &mut self,
        what: &'static str,
        run: Value,
        paragraph: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.finish_editing(window, cx);
        let ops: Vec<Op> = self
            .selected_elements(cx)
            .into_iter()
            .filter(|element| element.kind.text().is_some())
            .map(|element| Op::StyleText {
                id: element.id.clone(),
                paragraphs: None,
                find: None,
                run: run.clone(),
                paragraph: paragraph.clone(),
            })
            .collect();
        if !ops.is_empty() {
            self.apply(what, ops, window, cx);
        }
    }

    /// The text style of the first selected element with text, as drawn.
    fn text_style(&self, cx: &App) -> Option<(text::Style, text::Para, Anchor, Autofit)> {
        let deck = self.deck(cx);
        let slide = self.slide(cx)?;
        let element = self
            .selected_elements(cx)
            .into_iter()
            .find(|element| element.kind.text().is_some())?;
        let body = element.kind.text()?;
        let place = element
            .placeholder
            .and_then(|kind| deck.theme.layout(&slide.layout)?.placeholder(kind));
        let paras = text::paragraphs(
            body,
            &text::Context {
                theme: &deck.theme,
                place,
                color: None,
                size: None,
                bold: None,
            },
        );
        let para = paras.first()?.clone();
        let style = para
            .runs
            .first()
            .map(|(_, style)| style.clone())
            .unwrap_or_else(|| para.style.clone());
        Some((style, para, body.anchor, body.autofit))
    }

    pub(crate) fn render_bar(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let selected: Vec<Element> = self.selected_elements(cx).into_iter().cloned().collect();
        let mut items: Vec<AnyElement> = Vec::new();
        match selected.as_slice() {
            [] => self.slide_items(&mut items, cx),
            [element] => {
                match &element.kind {
                    Kind::Text(text_box) => {
                        self.text_items(&mut items, cx);
                        items.push(divider(cx));
                        items.push(self.swatch(
                            "bar-box-fill",
                            "Box fill",
                            fill_color(&text_box.fill),
                            true,
                            Rc::new(fill_pick(cx)),
                            cx,
                        ));
                    }
                    Kind::Shape(shape) => {
                        self.shape_items(&mut items, shape, cx);
                        if shape.geometry != Geometry::Line {
                            items.push(divider(cx));
                            self.text_items(&mut items, cx);
                        }
                    }
                    Kind::Picture(picture) => self.picture_items(&mut items, element, picture, cx),
                    Kind::Table(table) => self.table_items(&mut items, element, table, cx),
                    Kind::Chart(chart) => self.chart_items(&mut items, element, chart, cx),
                    Kind::Code(code) => self.code_items(&mut items, code, cx),
                    Kind::Icon(icon) => self.icon_items(&mut items, element, icon, cx),
                    Kind::Svg(_) => {
                        let id = element.id.clone();
                        items.push(
                            Button::new("bar-svg-edit")
                                .ghost()
                                .small()
                                .label("Edit SVG…")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_svg(Some(id.clone()), window, cx)
                                }))
                                .into_any_element(),
                        );
                    }
                    Kind::Raw(raw) => {
                        items.push(label(
                            &format!(
                                "{}, kept as it was in the PowerPoint file",
                                if raw.what.is_empty() {
                                    "A PowerPoint object"
                                } else {
                                    &raw.what
                                }
                            ),
                            cx,
                        ));
                    }
                    Kind::Group(_) => {
                        items.push(
                            Button::new("bar-ungroup")
                                .ghost()
                                .small()
                                .icon(Icons::Ungroup)
                                .label("Ungroup")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.ungroup(&super::Ungroup, window, cx)
                                }))
                                .into_any_element(),
                        );
                    }
                }
                items.push(divider(cx));
                self.common_items(&mut items, element, cx);
            }
            many => {
                self.many_items(&mut items, many.len(), cx);
                if many.iter().any(|element| element.kind.text().is_some()) {
                    items.push(divider(cx));
                    self.text_items(&mut items, cx);
                }
            }
        }
        h_flex()
            .id("slides-bar")
            .flex_none()
            .h(px(42.))
            .px_2()
            .gap_0p5()
            .border_t_1()
            .border_color(theme.border)
            .overflow_x_scroll()
            .children(items)
            .into_any_element()
    }

    /// Nothing selected: the slide's own settings.
    fn slide_items(&self, items: &mut Vec<AnyElement>, cx: &mut Context<Self>) {
        let Some(slide) = self.slide(cx) else {
            return;
        };
        let (id, hidden, transition) = (slide.id.clone(), slide.hidden, slide.transition);
        let background = slide.background.as_ref().and_then(fill_color);
        items.push(label("Slide", cx));
        let editor = cx.weak_entity();
        let slide_id = id.clone();
        let pick: Pick = Rc::new(move |color, window, cx| {
            let background = match color {
                Some(color) => json!(color),
                None => Value::Null,
            };
            _ = editor.update(cx, |this, cx| {
                this.apply(
                    "background",
                    vec![Op::UpdateSlide {
                        slide: json!(slide_id),
                        layout: None,
                        background: Some(background.clone()),
                        notes: None,
                        hidden: None,
                        transition: None,
                        transition_from: None,
                        transition_seconds: None,
                    }],
                    window,
                    cx,
                );
            });
        });
        items.push(self.swatch_with(
            "bar-background",
            "Background",
            background,
            Some("Theme background"),
            pick,
            cx,
        ));
        let (from, seconds) = self
            .slide(cx)
            .map(|slide| (slide.transition_from, slide.transition_seconds))
            .unwrap_or((Direction::Right, 0.5));
        let editor = cx.weak_entity();
        let slide_id = id.clone();
        items.push(
            Button::new("bar-transition")
                .ghost()
                .small()
                .label(if transition == Transition::None {
                    "No transition".to_string()
                } else {
                    format!("{} · {seconds:.2} s", transition.label())
                })
                .dropdown_caret(true)
                .tooltip("How the slide comes in when presenting")
                .menu(gpui_kit::Anchor::BottomLeft, move |menu, _, _| {
                    let set = |label: String,
                               checked: bool,
                               kind: Option<Transition>,
                               side: Option<Direction>,
                               time: Option<f32>| {
                        let (editor, slide_id) = (editor.clone(), slide_id.clone());
                        PopupMenuItem::new(label)
                            .checked(checked)
                            .on_click(move |_, window, cx| {
                                _ = editor.update(cx, |this, cx| {
                                    this.apply(
                                        "transition",
                                        vec![Op::UpdateSlide {
                                            slide: json!(slide_id),
                                            layout: None,
                                            background: None,
                                            notes: None,
                                            hidden: None,
                                            transition: kind,
                                            transition_from: side,
                                            transition_seconds: time,
                                        }],
                                        window,
                                        cx,
                                    );
                                });
                            })
                    };
                    let mut menu = menu;
                    for (kind, name) in Transition::ALL {
                        menu = menu.item(set(
                            name.to_string(),
                            kind == transition,
                            Some(kind),
                            None,
                            None,
                        ));
                    }
                    if transition.has_direction() {
                        menu = menu.separator();
                        for (side, name) in Direction::ALL {
                            menu = menu.item(set(
                                name.to_string(),
                                side == from,
                                None,
                                Some(side),
                                None,
                            ));
                        }
                    }
                    if transition != Transition::None {
                        menu = menu.separator();
                        for time in [0.3, 0.5, 0.8, 1.2, 2.] {
                            menu = menu.item(set(
                                format!("{time} seconds"),
                                (time - seconds).abs() < 0.01,
                                None,
                                None,
                                Some(time),
                            ));
                        }
                    }
                    menu
                })
                .into_any_element(),
        );
        let slide_id = id;
        items.push(
            Button::new("bar-hidden")
                .ghost()
                .small()
                .icon(Icons::EyeOff)
                .label("Hide in presenting")
                .selected(hidden)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.apply(
                        "hide",
                        vec![Op::UpdateSlide {
                            slide: json!(slide_id),
                            layout: None,
                            background: None,
                            notes: None,
                            hidden: Some(!hidden),
                            transition: None,
                            transition_from: None,
                            transition_seconds: None,
                        }],
                        window,
                        cx,
                    );
                }))
                .into_any_element(),
        );
        items.push(div().flex_1().into_any_element());
        items.push(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("Click a shape or text to change it; double-click to type.")
                .into_any_element(),
        );
    }

    fn text_items(&self, items: &mut Vec<AnyElement>, cx: &mut Context<Self>) {
        let Some((style, para, anchor, autofit)) = self.text_style(cx) else {
            return;
        };
        let deck = self.deck(cx);
        let fonts = deck.theme.fonts.clone();
        let shown = deck.theme.font(&style.font).to_string();
        let editor = cx.weak_entity();
        let font_editor = editor.clone();
        items.push(
            Button::new("bar-font")
                .ghost()
                .small()
                .label(shorten(&shown, 18))
                .dropdown_caret(true)
                .tooltip("Font")
                .menu(gpui_kit::Anchor::BottomLeft, move |menu, _, _| {
                    let item = |label: String, font: &'static str, value: String| {
                        let editor = font_editor.clone();
                        let _ = font;
                        PopupMenuItem::new(label).on_click(move |_, window, cx| {
                            _ = editor.update(cx, |this, cx| {
                                this.style_selected(
                                    "font",
                                    json!({ "font": value }),
                                    Value::Null,
                                    window,
                                    cx,
                                )
                            });
                        })
                    };
                    let mut menu = menu
                        .item(item(
                            format!("Heading font ({})", fonts.heading),
                            "",
                            "heading".into(),
                        ))
                        .item(item(
                            format!("Body font ({})", fonts.body),
                            "",
                            "body".into(),
                        ))
                        .item(item(
                            format!("Code font ({})", fonts.code),
                            "",
                            "code".into(),
                        ))
                        .separator();
                    for family in FAMILIES {
                        if crate::slides::fonts::fonts().has(family) {
                            menu = menu.item(item(family.to_string(), "", family.to_string()));
                        }
                    }
                    menu.max_h(px(420.)).scrollable(true)
                })
                .into_any_element(),
        );
        let size = style.size;
        let size_editor = editor.clone();
        items.push(
            Button::new("bar-size-down")
                .ghost()
                .xsmall()
                .icon(Icons::Minus)
                .tooltip("Smaller")
                .on_click(cx.listener(move |this, _, window, cx| {
                    let next = SIZES
                        .iter()
                        .rev()
                        .find(|s| **s < size - 0.5)
                        .copied()
                        .unwrap_or((size - 2.).max(6.));
                    this.style_selected("size", json!({ "size": next }), Value::Null, window, cx);
                }))
                .into_any_element(),
        );
        items.push(
            Button::new("bar-size")
                .ghost()
                .xsmall()
                .label(format!("{}", size.round()))
                .tooltip("Size in points")
                .menu(gpui_kit::Anchor::BottomLeft, move |menu, _, _| {
                    SIZES.iter().fold(menu, |menu, s| {
                        let (editor, s) = (size_editor.clone(), *s);
                        menu.item(
                            PopupMenuItem::new(format!("{s}"))
                                .checked((s - size).abs() < 0.5)
                                .on_click(move |_, window, cx| {
                                    _ = editor.update(cx, |this, cx| {
                                        this.style_selected(
                                            "size",
                                            json!({ "size": s }),
                                            Value::Null,
                                            window,
                                            cx,
                                        )
                                    });
                                }),
                        )
                    })
                })
                .into_any_element(),
        );
        items.push(
            Button::new("bar-size-up")
                .ghost()
                .xsmall()
                .icon(Icons::Plus)
                .tooltip("Larger")
                .on_click(cx.listener(move |this, _, window, cx| {
                    let next = SIZES
                        .iter()
                        .find(|s| **s > size + 0.5)
                        .copied()
                        .unwrap_or(size + 8.);
                    this.style_selected("size", json!({ "size": next }), Value::Null, window, cx);
                }))
                .into_any_element(),
        );
        for (id, icon, tip, key, on) in [
            ("bar-bold", Icons::Bold, "Bold", "bold", style.bold),
            (
                "bar-italic",
                Icons::Italic,
                "Italic",
                "italic",
                style.italic,
            ),
            (
                "bar-underline",
                Icons::Underline,
                "Underline",
                "underline",
                style.underline,
            ),
            (
                "bar-strike",
                Icons::Strikethrough,
                "Strike through",
                "strike",
                style.strike,
            ),
        ] {
            items.push(
                Button::new(id)
                    .ghost()
                    .small()
                    .icon(icon)
                    .selected(on)
                    .tooltip(tip)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.style_selected("style", json!({ key: !on }), Value::Null, window, cx)
                    }))
                    .into_any_element(),
            );
        }
        let pick_editor = editor.clone();
        items.push(self.swatch_with(
            "bar-text-color",
            "Text colour",
            Some(format!("#{}", style.color.hex())),
            Some("Automatic"),
            Rc::new(move |color, window, cx| {
                _ = pick_editor.update(cx, |this, cx| {
                    let color = color.map(Value::String).unwrap_or(Value::Null);
                    this.style_selected(
                        "colour",
                        json!({ "color": color }),
                        Value::Null,
                        window,
                        cx,
                    )
                });
            }),
            cx,
        ));
        items.push(divider(cx));
        for (id, icon, tip, align) in [
            ("bar-left", Icons::TextAlignStart, "Align left", Align::Left),
            (
                "bar-center",
                Icons::TextAlignCenter,
                "Centre",
                Align::Center,
            ),
            (
                "bar-right",
                Icons::TextAlignEnd,
                "Align right",
                Align::Right,
            ),
            (
                "bar-justify",
                Icons::TextAlignJustify,
                "Justify",
                Align::Justify,
            ),
        ] {
            items.push(
                Button::new(id)
                    .ghost()
                    .small()
                    .icon(icon)
                    .selected(para.align == align)
                    .tooltip(tip)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.style_selected(
                            "align",
                            Value::Null,
                            json!({ "align": align }),
                            window,
                            cx,
                        )
                    }))
                    .into_any_element(),
            );
        }
        let bulleted = matches!(&para.bullet, Some((text, _)) if !text.chars().next().is_some_and(|c| c.is_ascii_digit()));
        let numbered = matches!(&para.bullet, Some((text, _)) if text.chars().next().is_some_and(|c| c.is_ascii_digit()));
        items.push(
            Button::new("bar-bullets")
                .ghost()
                .small()
                .icon(Icons::List)
                .selected(bulleted)
                .tooltip("Bullets")
                .on_click(cx.listener(move |this, _, window, cx| {
                    let bullet = if bulleted {
                        json!("none")
                    } else {
                        json!({ "char": "•" })
                    };
                    this.style_selected(
                        "bullets",
                        Value::Null,
                        json!({ "bullet": bullet }),
                        window,
                        cx,
                    )
                }))
                .into_any_element(),
        );
        items.push(
            Button::new("bar-numbers")
                .ghost()
                .small()
                .icon(Icons::ListOrdered)
                .selected(numbered)
                .tooltip("Numbered")
                .on_click(cx.listener(move |this, _, window, cx| {
                    let bullet = if numbered {
                        json!("none")
                    } else {
                        json!("number")
                    };
                    this.style_selected(
                        "bullets",
                        Value::Null,
                        json!({ "bullet": bullet }),
                        window,
                        cx,
                    )
                }))
                .into_any_element(),
        );
        let level = para.level;
        items.push(
            Button::new("bar-outdent")
                .ghost()
                .small()
                .icon(Icons::ListIndentDecrease)
                .disabled(level == 0)
                .tooltip("Less indent")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.style_selected(
                        "indent",
                        Value::Null,
                        json!({ "level": level.saturating_sub(1) }),
                        window,
                        cx,
                    )
                }))
                .into_any_element(),
        );
        items.push(
            Button::new("bar-indent")
                .ghost()
                .small()
                .icon(Icons::ListIndentIncrease)
                .tooltip("More indent")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.style_selected(
                        "indent",
                        Value::Null,
                        json!({ "level": (level + 1).min(4) }),
                        window,
                        cx,
                    )
                }))
                .into_any_element(),
        );
        let spacing = para.line_spacing;
        let spacing_editor = editor.clone();
        let anchor_editor = editor.clone();
        let fit_editor = editor;
        items.push(
            Button::new("bar-spacing")
                .ghost()
                .small()
                .icon(Icons::Baseline)
                .dropdown_caret(true)
                .tooltip("Spacing, placement and fitting")
                .menu(gpui_kit::Anchor::BottomLeft, move |menu, _, _| {
                    let mut menu = menu.label("Line spacing");
                    for (value, name) in [
                        (1.0, "Single"),
                        (1.15, "1.15"),
                        (1.5, "1.5"),
                        (2.0, "Double"),
                    ] {
                        let editor = spacing_editor.clone();
                        menu = menu.item(
                            PopupMenuItem::new(name)
                                .checked((spacing - value).abs() < 0.01)
                                .on_click(move |_, window, cx| {
                                    _ = editor.update(cx, |this, cx| {
                                        this.style_selected(
                                            "spacing",
                                            Value::Null,
                                            json!({ "line_spacing": value }),
                                            window,
                                            cx,
                                        )
                                    });
                                }),
                        );
                    }
                    menu = menu.separator().label("Text in the box");
                    for (value, name) in [
                        (Anchor::Top, "At the top"),
                        (Anchor::Middle, "In the middle"),
                        (Anchor::Bottom, "At the bottom"),
                    ] {
                        let editor = anchor_editor.clone();
                        menu =
                            menu.item(PopupMenuItem::new(name).checked(anchor == value).on_click(
                                move |_, window, cx| {
                                    _ = editor.update(cx, |this, cx| {
                                        this.set_selected(
                                            "anchor",
                                            json!({ "text": { "anchor": value } }),
                                            window,
                                            cx,
                                        )
                                    });
                                },
                            ));
                    }
                    menu = menu.separator().label("Too much text");
                    for (value, name) in [
                        (Autofit::Shrink, "Shrinks to fit"),
                        (Autofit::Grow, "The box grows"),
                        (Autofit::None, "Runs over"),
                    ] {
                        let editor = fit_editor.clone();
                        menu =
                            menu.item(PopupMenuItem::new(name).checked(autofit == value).on_click(
                                move |_, window, cx| {
                                    _ = editor.update(cx, |this, cx| {
                                        this.set_selected(
                                            "fit",
                                            json!({ "text": { "autofit": value } }),
                                            window,
                                            cx,
                                        )
                                    });
                                },
                            ));
                    }
                    menu
                })
                .into_any_element(),
        );
    }

    fn shape_items(&self, items: &mut Vec<AnyElement>, shape: &Shape, cx: &mut Context<Self>) {
        let line_only = shape.geometry == Geometry::Line;
        if !line_only {
            items.push(self.swatch(
                "bar-fill",
                "Fill",
                fill_color(&shape.fill),
                true,
                Rc::new(fill_pick(cx)),
                cx,
            ));
        }
        let editor = cx.weak_entity();
        let line_color = shape.line.color.as_ref().map(|color| color.0.clone());
        let line_editor = editor.clone();
        items.push(self.swatch(
            "bar-line",
            if line_only { "Line colour" } else { "Outline" },
            if shape.line.width > 0. || line_only { line_color.or(Some("dark1".into())) } else { None },
            !line_only,
            Rc::new(move |color, window, cx| {
                _ = line_editor.update(cx, |this, cx| {
                    let set = match color {
                        Some(color) => json!({ "line": { "color": color, "width": if line_only { Value::Null } else { json!(2) } } }),
                        None => json!({ "line": { "width": 0 } }),
                    };
                    let set = strip_nulls(set);
                    this.set_selected("outline", set, window, cx)
                });
            }),
            cx,
        ));
        let (width, dash, head, tail) = (
            shape.line.width,
            shape.line.dash,
            shape.line.head,
            shape.line.tail,
        );
        let width_editor = editor.clone();
        items.push(
            Button::new("bar-line-width")
                .ghost()
                .small()
                .label(if width > 0. {
                    format!("{} pt", width)
                } else {
                    "No line".into()
                })
                .dropdown_caret(true)
                .tooltip("Line width and dashes")
                .menu(gpui_kit::Anchor::BottomLeft, move |menu, _, _| {
                    let mut menu = menu.label("Width");
                    for value in [0.5, 1., 1.5, 2., 3., 4., 6., 8.] {
                        let editor = width_editor.clone();
                        menu = menu.item(
                            PopupMenuItem::new(format!("{value} pt"))
                                .checked((width - value).abs() < 0.01)
                                .on_click(move |_, window, cx| {
                                    _ = editor.update(cx, |this, cx| {
                                        this.set_selected(
                                            "line width",
                                            json!({ "line": { "width": value } }),
                                            window,
                                            cx,
                                        )
                                    });
                                }),
                        );
                    }
                    menu = menu.separator().label("Dashes");
                    for (value, name) in [
                        (Dash::Solid, "Solid"),
                        (Dash::Dash, "Dashed"),
                        (Dash::Dot, "Dotted"),
                        (Dash::DashDot, "Dash dot"),
                    ] {
                        let editor = width_editor.clone();
                        menu = menu.item(PopupMenuItem::new(name).checked(dash == value).on_click(
                            move |_, window, cx| {
                                _ = editor.update(cx, |this, cx| {
                                    this.set_selected(
                                        "dash",
                                        json!({ "line": { "dash": value } }),
                                        window,
                                        cx,
                                    )
                                });
                            },
                        ));
                    }
                    menu
                })
                .into_any_element(),
        );
        if line_only {
            for (id, label, key, on) in [
                ("bar-head", "Start arrow", "head", head),
                ("bar-tail", "End arrow", "tail", tail),
            ] {
                items.push(
                    Button::new(id)
                        .ghost()
                        .small()
                        .label(label)
                        .selected(on)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.set_selected("arrow", json!({ "line": { key: !on } }), window, cx)
                        }))
                        .into_any_element(),
                );
            }
        } else {
            let shadow = shape.shadow.is_some();
            items.push(
                Button::new("bar-shadow")
                    .ghost()
                    .small()
                    .label("Shadow")
                    .selected(shadow)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let set = if shadow {
                            json!({ "shadow": null })
                        } else {
                            json!({ "shadow": {} })
                        };
                        this.set_selected("shadow", set, window, cx)
                    }))
                    .into_any_element(),
            );
        }
    }

    fn picture_items(
        &self,
        items: &mut Vec<AnyElement>,
        element: &Element,
        picture: &Picture,
        cx: &mut Context<Self>,
    ) {
        let cropping = self.cropping.as_ref() == Some(&element.id);
        let turned = element.frame.rotation.rem_euclid(360.).abs() > 0.01;
        let crop_id = element.id.clone();
        items.push(
            Button::new("bar-crop")
                .ghost()
                .small()
                .icon(Icons::Crop)
                .label(if cropping { "Done cropping" } else { "Crop" })
                .selected(cropping)
                .disabled(turned)
                .tooltip(if turned {
                    "Crop a picture that is not turned"
                } else {
                    "Drag the picture's sides to cut it (Esc when done)"
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this.cropping.take().is_none() {
                        this.start_crop(&crop_id, window, cx);
                    }
                    cx.notify();
                }))
                .into_any_element(),
        );
        let editor = cx.weak_entity();
        let fit = picture.fit;
        items.push(
            Button::new("bar-fit")
                .ghost()
                .small()
                .label(match fit {
                    Fit::Cover => "Fill the frame",
                    Fit::Contain => "Fit inside",
                    Fit::Stretch => "Stretch",
                })
                .dropdown_caret(true)
                .menu(gpui_kit::Anchor::BottomLeft, move |menu, _, _| {
                    [
                        (Fit::Cover, "Fill the frame (cut to fit)"),
                        (Fit::Contain, "Fit inside"),
                        (Fit::Stretch, "Stretch"),
                    ]
                    .into_iter()
                    .fold(menu, |menu, (value, name)| {
                        let editor = editor.clone();
                        menu.item(PopupMenuItem::new(name).checked(fit == value).on_click(
                            move |_, window, cx| {
                                _ = editor.update(cx, |this, cx| {
                                    this.set_selected("fit", json!({ "fit": value }), window, cx)
                                });
                            },
                        ))
                    })
                })
                .into_any_element(),
        );
        let radius = picture.radius;
        let editor = cx.weak_entity();
        items.push(
            Button::new("bar-radius")
                .ghost()
                .small()
                .label(if radius > 0. {
                    "Rounded"
                } else {
                    "Square corners"
                })
                .dropdown_caret(true)
                .menu(gpui_kit::Anchor::BottomLeft, move |menu, _, _| {
                    [
                        (0., "Square"),
                        (8., "A little rounded"),
                        (16., "Rounded"),
                        (32., "Very rounded"),
                        (10_000., "Round"),
                    ]
                    .into_iter()
                    .fold(menu, |menu, (value, name)| {
                        let editor = editor.clone();
                        menu.item(
                            PopupMenuItem::new(name)
                                .checked((radius - value).abs() < 0.5)
                                .on_click(move |_, window, cx| {
                                    _ = editor.update(cx, |this, cx| {
                                        this.set_selected(
                                            "corners",
                                            json!({ "radius": value }),
                                            window,
                                            cx,
                                        )
                                    });
                                }),
                        )
                    })
                })
                .into_any_element(),
        );
        let shadow = picture.shadow.is_some();
        items.push(
            Button::new("bar-picture-shadow")
                .ghost()
                .small()
                .label("Shadow")
                .selected(shadow)
                .on_click(cx.listener(move |this, _, window, cx| {
                    let set = if shadow {
                        json!({ "shadow": null })
                    } else {
                        json!({ "shadow": {} })
                    };
                    this.set_selected("shadow", set, window, cx)
                }))
                .into_any_element(),
        );
        let id = element.id.clone();
        items.push(
            Button::new("bar-replace")
                .ghost()
                .small()
                .label("Replace…")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.replace_picture(id.clone(), window, cx)
                }))
                .into_any_element(),
        );
    }

    fn table_items(
        &self,
        items: &mut Vec<AnyElement>,
        element: &Element,
        table: &Table,
        cx: &mut Context<Self>,
    ) {
        let id = element.id.clone();
        items.push(
            Button::new("bar-table-edit")
                .ghost()
                .small()
                .icon(Icons::Pencil)
                .label("Edit cells…")
                .on_click(
                    cx.listener(move |this, _, window, cx| this.open_table(id.clone(), window, cx)),
                )
                .into_any_element(),
        );
        let (header, banded) = (table.header, table.banded);
        items.push(
            Button::new("bar-table-header")
                .ghost()
                .small()
                .label("Header row")
                .selected(header)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_selected("header", json!({ "header": !header }), window, cx)
                }))
                .into_any_element(),
        );
        items.push(
            Button::new("bar-table-banded")
                .ghost()
                .small()
                .label("Banded rows")
                .selected(banded)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_selected("banded", json!({ "banded": !banded }), window, cx)
                }))
                .into_any_element(),
        );
        let editor = cx.weak_entity();
        items.push(
            self.swatch(
                "bar-table-accent",
                "Header colour",
                Some(
                    table
                        .accent
                        .as_ref()
                        .map_or("accent1".into(), |color| color.0.clone()),
                ),
                false,
                Rc::new(move |color, window, cx| {
                    _ = editor.update(cx, |this, cx| {
                        this.set_selected("accent", json!({ "accent": color }), window, cx)
                    });
                }),
                cx,
            ),
        );
        let size = table.font_size;
        for (id, icon, delta) in [
            ("bar-table-smaller", Icons::Minus, -2.),
            ("bar-table-larger", Icons::Plus, 2.),
        ] {
            items.push(
                Button::new(id)
                    .ghost()
                    .xsmall()
                    .icon(icon)
                    .tooltip(if delta < 0. {
                        "Smaller text"
                    } else {
                        "Larger text"
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_selected(
                            "table size",
                            json!({ "font_size": (size + delta).clamp(6., 60.) }),
                            window,
                            cx,
                        )
                    }))
                    .into_any_element(),
            );
        }
    }

    fn chart_items(
        &self,
        items: &mut Vec<AnyElement>,
        element: &Element,
        chart: &Chart,
        cx: &mut Context<Self>,
    ) {
        let id = element.id.clone();
        items.push(
            Button::new("bar-chart-data")
                .ghost()
                .small()
                .icon(Icons::Pencil)
                .label("Edit data…")
                .on_click(
                    cx.listener(move |this, _, window, cx| this.open_chart(id.clone(), window, cx)),
                )
                .into_any_element(),
        );
        let editor = cx.weak_entity();
        let current = serde_json::to_value(chart.chart)
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_default();
        let label = super::toolbar::CHARTS
            .iter()
            .find(|(id, _)| *id == current)
            .map_or("Chart", |(_, name)| name);
        items.push(
            Button::new("bar-chart-kind")
                .ghost()
                .small()
                .label(label)
                .dropdown_caret(true)
                .menu(gpui_kit::Anchor::BottomLeft, move |menu, _, _| {
                    super::toolbar::CHARTS
                        .iter()
                        .fold(menu, |menu, (id, name)| {
                            let editor = editor.clone();
                            menu.item(PopupMenuItem::new(*name).checked(current == *id).on_click(
                                move |_, window, cx| {
                                    _ = editor.update(cx, |this, cx| {
                                        this.set_selected(
                                            "chart",
                                            json!({ "chart": id }),
                                            window,
                                            cx,
                                        )
                                    });
                                },
                            ))
                        })
                })
                .into_any_element(),
        );
        let (legend, labels) = (chart.legend, chart.labels);
        items.push(
            Button::new("bar-chart-legend")
                .ghost()
                .small()
                .label("Legend")
                .selected(legend)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_selected("legend", json!({ "legend": !legend }), window, cx)
                }))
                .into_any_element(),
        );
        items.push(
            Button::new("bar-chart-labels")
                .ghost()
                .small()
                .label("Values")
                .selected(labels)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_selected("labels", json!({ "labels": !labels }), window, cx)
                }))
                .into_any_element(),
        );
    }

    fn code_items(&self, items: &mut Vec<AnyElement>, code: &Code, cx: &mut Context<Self>) {
        let editor = cx.weak_entity();
        let language = code.language.clone();
        items.push(
            Button::new("bar-code-language")
                .ghost()
                .small()
                .label(if language.is_empty() {
                    "Plain".to_string()
                } else {
                    language.clone()
                })
                .dropdown_caret(true)
                .menu(gpui_kit::Anchor::BottomLeft, move |menu, _, _| {
                    LANGUAGES
                        .iter()
                        .fold(menu, |menu, name| {
                            let editor = editor.clone();
                            menu.item(
                                PopupMenuItem::new(*name)
                                    .checked(language == *name)
                                    .on_click(move |_, window, cx| {
                                        _ = editor.update(cx, |this, cx| {
                                            this.set_selected(
                                                "language",
                                                json!({ "language": name }),
                                                window,
                                                cx,
                                            )
                                        });
                                    }),
                            )
                        })
                        .max_h(px(360.))
                        .scrollable(true)
                })
                .into_any_element(),
        );
        let (dark, numbers, size) = (code.dark, code.line_numbers, code.font_size);
        items.push(
            Button::new("bar-code-dark")
                .ghost()
                .small()
                .label("Dark")
                .selected(dark)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_selected("dark", json!({ "dark": !dark }), window, cx)
                }))
                .into_any_element(),
        );
        items.push(
            Button::new("bar-code-numbers")
                .ghost()
                .small()
                .label("Line numbers")
                .selected(numbers)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_selected("numbers", json!({ "line_numbers": !numbers }), window, cx)
                }))
                .into_any_element(),
        );
        for (id, icon, delta) in [
            ("bar-code-smaller", Icons::Minus, -1.),
            ("bar-code-larger", Icons::Plus, 1.),
        ] {
            items.push(
                Button::new(id)
                    .ghost()
                    .xsmall()
                    .icon(icon)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_selected(
                            "code size",
                            json!({ "font_size": (size + delta).clamp(6., 48.) }),
                            window,
                            cx,
                        )
                    }))
                    .into_any_element(),
            );
        }
    }

    fn icon_items(
        &self,
        items: &mut Vec<AnyElement>,
        element: &Element,
        icon: &Icon,
        cx: &mut Context<Self>,
    ) {
        let editor = cx.weak_entity();
        items.push(self.swatch(
            "bar-icon-color",
            "Icon colour",
            Some(icon.color.0.clone()),
            false,
            Rc::new(move |color, window, cx| {
                _ = editor.update(cx, |this, cx| {
                    this.set_selected("icon colour", json!({ "color": color }), window, cx)
                });
            }),
            cx,
        ));
        let stroke = icon.stroke;
        let editor = cx.weak_entity();
        items.push(
            Button::new("bar-icon-stroke")
                .ghost()
                .small()
                .label("Weight")
                .dropdown_caret(true)
                .menu(gpui_kit::Anchor::BottomLeft, move |menu, _, _| {
                    [
                        (1., "Thin"),
                        (1.5, "Light"),
                        (2., "Regular"),
                        (2.5, "Bold"),
                        (3., "Heavy"),
                    ]
                    .into_iter()
                    .fold(menu, |menu, (value, name)| {
                        let editor = editor.clone();
                        menu.item(
                            PopupMenuItem::new(name)
                                .checked((stroke - value).abs() < 0.01)
                                .on_click(move |_, window, cx| {
                                    _ = editor.update(cx, |this, cx| {
                                        this.set_selected(
                                            "stroke",
                                            json!({ "stroke": value }),
                                            window,
                                            cx,
                                        )
                                    });
                                }),
                        )
                    })
                })
                .into_any_element(),
        );
        let id = element.id.clone();
        items.push(
            Button::new("bar-icon-change")
                .ghost()
                .small()
                .label("Change…")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_icons(Some(id.clone()), window, cx)
                }))
                .into_any_element(),
        );
    }

    /// Several selected: lining up, spacing, grouping.
    fn many_items(&self, items: &mut Vec<AnyElement>, count: usize, cx: &mut Context<Self>) {
        items.push(label(&format!("{count} selected"), cx));
        for (id, icon, tip, to) in [
            (
                "bar-align-left",
                Icons::AlignStartVertical,
                "Line up left edges",
                "left",
            ),
            (
                "bar-align-center",
                Icons::AlignCenterVertical,
                "Line up centres",
                "center",
            ),
            (
                "bar-align-right",
                Icons::AlignEndVertical,
                "Line up right edges",
                "right",
            ),
            (
                "bar-align-top",
                Icons::AlignStartHorizontal,
                "Line up tops",
                "top",
            ),
            (
                "bar-align-middle",
                Icons::AlignCenterHorizontal,
                "Line up middles",
                "middle",
            ),
            (
                "bar-align-bottom",
                Icons::AlignEndHorizontal,
                "Line up bottoms",
                "bottom",
            ),
        ] {
            items.push(
                Button::new(id)
                    .ghost()
                    .small()
                    .icon(icon)
                    .tooltip(tip)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let ids = this.selected.clone();
                        this.apply(
                            "align",
                            vec![Op::Align {
                                ids,
                                to: to.into(),
                                slide: false,
                            }],
                            window,
                            cx,
                        );
                    }))
                    .into_any_element(),
            );
        }
        if count >= 3 {
            for (id, label, axis) in [
                ("bar-distribute-h", "Space across", "horizontal"),
                ("bar-distribute-v", "Space down", "vertical"),
            ] {
                items.push(
                    Button::new(id)
                        .ghost()
                        .small()
                        .label(label)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let ids = this.selected.clone();
                            this.apply(
                                "distribute",
                                vec![Op::Distribute {
                                    ids,
                                    axis: axis.into(),
                                }],
                                window,
                                cx,
                            );
                        }))
                        .into_any_element(),
                );
            }
        }
        items.push(
            Button::new("bar-group")
                .ghost()
                .small()
                .icon(Icons::Group)
                .label("Group")
                .on_click(cx.listener(|this, _, window, cx| this.group(&super::Group, window, cx)))
                .into_any_element(),
        );
        items.push(
            Button::new("bar-many-delete")
                .ghost()
                .small()
                .icon(Icons::Trash)
                .tooltip("Delete")
                .on_click(
                    cx.listener(|this, _, window, cx| this.delete(&super::Delete, window, cx)),
                )
                .into_any_element(),
        );
    }

    /// For any one element: its order, opacity, copy and delete.
    fn common_items(&self, items: &mut Vec<AnyElement>, element: &Element, cx: &mut Context<Self>) {
        items.push(self.animation_button(element, cx));
        let editor = cx.weak_entity();
        items.push(
            Button::new("bar-arrange")
                .ghost()
                .small()
                .icon(Icons::Layers)
                .dropdown_caret(true)
                .tooltip("Order and alignment")
                .menu(gpui_kit::Anchor::BottomRight, move |menu, _, _| {
                    let order = |label: &'static str, to: &'static str| {
                        let editor = editor.clone();
                        PopupMenuItem::new(label).on_click(move |_, window, cx| {
                            _ = editor.update(cx, |this, cx| this.order(to, window, cx));
                        })
                    };
                    let align = |label: &'static str, to: &'static str| {
                        let editor = editor.clone();
                        PopupMenuItem::new(label).on_click(move |_, window, cx| {
                            _ = editor.update(cx, |this, cx| {
                                let ids = this.selected.clone();
                                this.apply(
                                    "align",
                                    vec![Op::Align {
                                        ids,
                                        to: to.into(),
                                        slide: true,
                                    }],
                                    window,
                                    cx,
                                );
                            });
                        })
                    };
                    menu.item(order("Bring to front", "front"))
                        .item(order("Bring forward", "forward"))
                        .item(order("Send backward", "backward"))
                        .item(order("Send to back", "back"))
                        .separator()
                        .label("On the slide")
                        .item(align("Centre across", "center"))
                        .item(align("Centre up and down", "middle"))
                        .item(align("Left", "left"))
                        .item(align("Right", "right"))
                        .item(align("Top", "top"))
                        .item(align("Bottom", "bottom"))
                })
                .into_any_element(),
        );
        let opacity = element.opacity;
        let editor = cx.weak_entity();
        items.push(
            Button::new("bar-opacity")
                .ghost()
                .small()
                .label(format!("{}%", (opacity * 100.).round()))
                .tooltip("Opacity")
                .menu(gpui_kit::Anchor::BottomRight, move |menu, _, _| {
                    [1., 0.85, 0.7, 0.5, 0.3, 0.15]
                        .into_iter()
                        .fold(menu, |menu, value| {
                            let editor = editor.clone();
                            menu.item(
                                PopupMenuItem::new(format!("{}%", (value * 100.) as u32))
                                    .checked((opacity - value).abs() < 0.01)
                                    .on_click(move |_, window, cx| {
                                        _ = editor.update(cx, |this, cx| {
                                            this.set_selected(
                                                "opacity",
                                                json!({ "opacity": value }),
                                                window,
                                                cx,
                                            )
                                        });
                                    }),
                            )
                        })
                })
                .into_any_element(),
        );
        items.push(div().flex_1().into_any_element());
        items.push(
            Button::new("bar-duplicate")
                .ghost()
                .small()
                .icon(Icons::CopyPlus)
                .tooltip("Duplicate")
                .on_click(
                    cx.listener(|this, _, window, cx| {
                        this.duplicate(&super::Duplicate, window, cx)
                    }),
                )
                .into_any_element(),
        );
        items.push(
            Button::new("bar-delete")
                .ghost()
                .small()
                .icon(Icons::Trash)
                .tooltip("Delete")
                .on_click(
                    cx.listener(|this, _, window, cx| this.delete(&super::Delete, window, cx)),
                )
                .into_any_element(),
        );
    }

    /// How the element comes in when presenting: effect, when, from where,
    /// how long, and its place in the slide's order.
    fn animation_button(&self, element: &Element, cx: &mut Context<Self>) -> AnyElement {
        let animations = self
            .slide(cx)
            .map(|slide| slide.animations.clone())
            .unwrap_or_default();
        let place = animations
            .iter()
            .position(|animation| animation.id == element.id);
        let current = place.map(|ix| animations[ix].clone());
        let count = animations.len();
        let id = element.id.clone();
        let editor = cx.weak_entity();
        Button::new("bar-animation")
            .ghost()
            .small()
            .icon(Icons::Sparkles)
            .label(match (&current, place) {
                (Some(animation), Some(ix)) => format!("{}. {}", ix + 1, animation.effect.label()),
                _ => "Animate".into(),
            })
            .dropdown_caret(true)
            .tooltip("How it comes in when presenting")
            .menu(gpui_kit::Anchor::BottomRight, move |menu, _, _| {
                let animate = |label: String, checked: bool, op: Op| {
                    let editor = editor.clone();
                    PopupMenuItem::new(label)
                        .checked(checked)
                        .on_click(move |_, window, cx| {
                            let op = op.clone();
                            _ = editor.update(cx, |this, cx| {
                                this.apply("animation", vec![op], window, cx);
                            });
                        })
                };
                let change = |effect: Option<Effect>,
                              from: Option<Direction>,
                              start: Option<Start>,
                              seconds: Option<f32>,
                              at: Option<usize>| Op::Animate {
                    id: id.clone(),
                    effect,
                    from,
                    start,
                    seconds,
                    delay: None,
                    at,
                };
                let mut menu = menu.label("Comes in");
                for (effect, name) in Effect::ALL {
                    let checked = current
                        .as_ref()
                        .is_some_and(|animation| animation.effect == effect);
                    menu = menu.item(animate(
                        name.to_string(),
                        checked,
                        change(Some(effect), None, None, None, None),
                    ));
                }
                if let Some(animation) = &current {
                    menu = menu.separator().label("Starts");
                    for (start, name) in [
                        (Start::OnClick, "On click"),
                        (Start::WithPrevious, "With the one before"),
                        (Start::AfterPrevious, "After the one before"),
                    ] {
                        menu = menu.item(animate(
                            name.into(),
                            animation.start == start,
                            change(None, None, Some(start), None, None),
                        ));
                    }
                    if matches!(animation.effect, Effect::FlyIn | Effect::Wipe) {
                        menu = menu.separator();
                        for (side, name) in Direction::ALL {
                            menu = menu.item(animate(
                                name.into(),
                                animation.from == side,
                                change(None, Some(side), None, None, None),
                            ));
                        }
                    }
                    menu = menu.separator().label("Takes");
                    for seconds in [0.3, 0.5, 1., 2.] {
                        menu = menu.item(animate(
                            format!("{seconds} seconds"),
                            (animation.seconds - seconds).abs() < 0.01,
                            change(None, None, None, Some(seconds), None),
                        ));
                    }
                    let ix = place.unwrap_or(0);
                    menu = menu.separator();
                    if ix > 0 {
                        menu = menu.item(animate(
                            "Play earlier".into(),
                            false,
                            change(None, None, None, None, Some(ix)),
                        ));
                    }
                    if ix + 1 < count {
                        menu = menu.item(animate(
                            "Play later".into(),
                            false,
                            change(None, None, None, None, Some(ix + 2)),
                        ));
                    }
                    menu = menu.item(animate(
                        "No animation".into(),
                        false,
                        Op::Unanimate { id: id.clone() },
                    ));
                }
                menu
            })
            .into_any_element()
    }

    /// A colour button whose popover offers the theme's colours, lighter
    /// and darker, the standard ones, and none when `none` is allowed.
    fn swatch(
        &self,
        id: &'static str,
        tooltip: &'static str,
        current: Option<String>,
        none: bool,
        pick: Pick,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.swatch_with(id, tooltip, current, none.then_some("No colour"), pick, cx)
    }

    fn swatch_with(
        &self,
        id: &'static str,
        tooltip: &'static str,
        current: Option<String>,
        none: Option<&'static str>,
        pick: Pick,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.deck(cx).theme.clone();
        let shown = current
            .as_ref()
            .map(|color| to_hsla(theme.resolve(&Color(color.clone()))));
        let border = cx.theme().border;
        let trigger = Button::new(id).ghost().small().tooltip(tooltip).child(
            div()
                .size(px(16.))
                .rounded(px(3.))
                .border_1()
                .border_color(border)
                .when_some(shown, |this, color| this.bg(color))
                .when(shown.is_none(), |this| {
                    this.child(gpui_kit::component::Icon::new(Icons::Slash).xsmall())
                }),
        );
        let current = current.unwrap_or_default();
        Popover::new(id)
            .anchor(gpui_kit::Anchor::BottomLeft)
            .offset(ph_ui::POPUP_GAP)
            .trigger(trigger)
            .content(move |_, _, cx| {
                let popover = cx.entity().downgrade();
                let ring = cx.theme().ring;
                let border = cx.theme().border;
                let cell = |color: String, title: String| {
                    let (pick, popover) = (pick.clone(), popover.clone());
                    let selected = color.eq_ignore_ascii_case(&current);
                    div()
                        .id(gpui_kit::SharedString::from(format!("swatch-{color}")))
                        .size(px(18.))
                        .rounded(px(3.))
                        .border_1()
                        .border_color(if selected { ring } else { border })
                        .when(selected, |this| this.border_2())
                        .bg(to_hsla(theme.resolve(&Color(color.clone()))))
                        .cursor_pointer()
                        .tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(title.clone())
                                .build(window, cx)
                        })
                        .on_click(move |_, window, cx| {
                            pick(Some(color.clone()), window, cx);
                            _ = popover.update(cx, |popover, cx| popover.dismiss(window, cx));
                        })
                };
                let mut rows = v_flex().gap_1().p_1();
                rows = rows.child(
                    h_flex().gap_1().children(
                        COLUMNS
                            .iter()
                            .map(|slot| cell(slot.to_string(), slot_name(slot))),
                    ),
                );
                for shade in SHADES {
                    rows = rows.child(h_flex().gap_1().children(COLUMNS.iter().map(|slot| {
                        let color = format!("{slot}{shade}");
                        let title = format!(
                            "{}, {}% {}",
                            slot_name(slot),
                            shade.trim_start_matches(['+', '-']),
                            if shade.starts_with('+') {
                                "lighter"
                            } else {
                                "darker"
                            }
                        );
                        cell(color, title)
                    })));
                }
                rows = rows.child(div().h(px(4.))).child(
                    h_flex().gap_1().children(
                        STANDARD
                            .iter()
                            .map(|hex| cell(hex.to_string(), hex.to_string())),
                    ),
                );
                if let Some(none) = none {
                    let (pick, popover) = (pick.clone(), popover.clone());
                    rows = rows.child(
                        Button::new("swatch-none")
                            .ghost()
                            .xsmall()
                            .label(none)
                            .on_click(move |_, window, cx| {
                                pick(None, window, cx);
                                _ = popover.update(cx, |popover, cx| popover.dismiss(window, cx));
                            }),
                    );
                }
                rows
            })
            .into_any_element()
    }
}

/// What picking a fill does to the selection.
fn fill_pick(
    cx: &mut Context<SlidesEditor>,
) -> impl Fn(Option<String>, &mut Window, &mut App) + 'static {
    let editor = cx.weak_entity();
    move |color, window, cx| {
        _ = editor.update(cx, |this, cx| {
            let fill = color.unwrap_or_else(|| "none".into());
            this.set_selected("fill", json!({ "fill": fill }), window, cx)
        });
    }
}

/// A solid fill's colour.
fn fill_color(fill: &Fill) -> Option<String> {
    match fill {
        Fill::Solid { color, .. } => Some(color.0.clone()),
        Fill::Gradient { stops, .. } => stops.first().map(|stop| stop.color.0.clone()),
        _ => None,
    }
}

fn slot_name(slot: &str) -> String {
    match slot {
        "light1" => "Background".into(),
        "dark1" => "Text".into(),
        "light2" => "Background 2".into(),
        "dark2" => "Text 2".into(),
        other => other.replace("accent", "Accent "),
    }
}

/// `set` without its `null`s (which would clear fields).
fn strip_nulls(mut value: Value) -> Value {
    if let Value::Object(map) = &mut value {
        map.retain(|_, value| !value.is_null());
        for value in map.values_mut() {
            *value = strip_nulls(value.take());
        }
    }
    value
}

fn shorten(text: &str, most: usize) -> String {
    if text.chars().count() > most {
        format!("{}…", text.chars().take(most - 1).collect::<String>())
    } else {
        text.to_string()
    }
}

fn label(text: &str, cx: &App) -> AnyElement {
    div()
        .px_1()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.to_string())
        .into_any_element()
}

fn divider(cx: &App) -> AnyElement {
    div()
        .flex_none()
        .w(px(1.))
        .h(px(18.))
        .mx_1()
        .bg(cx.theme().border)
        .into_any_element()
}

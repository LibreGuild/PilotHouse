//! JSON, YAML, TOML and XML: the text in a coloured editor, and beside it
//! (or under it, in a narrow dialog) the result in another, coloured as
//! what it became. A text that cannot be read says where, with its line.
//!
//! Also the plain view, input over result, for a utility without a view of
//! its own.

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _,
    checkbox::Checkbox,
    h_flex,
    input::{Editor, EditorState, InputEvent, Textarea, TextareaState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, Subscription, Task,
    Window, div,
};
use ph_ui::Icons;

use super::parts::{self, Tone};
use super::{Ctx, ToolView};
use crate::data::{Converted, Format, Indent, Layout, Problem, Target};

/// A moment after the last key, so a big text is not read at each one.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(120);

pub(crate) struct Formats {
    ctx: Ctx,
    format: Format,
    mode: usize,
    layout: Layout,
    input: Entity<EditorState>,
    output: Entity<EditorState>,
    result: Option<Result<Converted, Problem>>,
    _input: Subscription,
    _run: Option<Task<()>>,
}

impl Formats {
    fn modes(&self) -> &'static [&'static str] {
        match self.format {
            Format::Json => &["Format", "Minify", "To YAML", "To TOML", "To CSV"],
            Format::Yaml => &["Format", "To JSON", "To TOML"],
            Format::Toml => &["Format", "To JSON", "To YAML"],
            Format::Xml => &["Format", "Minify"],
        }
    }

    fn target(&self) -> Target {
        let label = self.modes()[self.mode.min(self.modes().len() - 1)];
        match label {
            "Minify" => Target::Minified,
            "To JSON" => Target::To(Format::Json),
            "To YAML" => Target::To(Format::Yaml),
            "To TOML" => Target::To(Format::Toml),
            "To CSV" => Target::Csv,
            _ => Target::Pretty,
        }
    }

    /// The language the result is coloured as.
    fn output_language(&self) -> &'static str {
        match self.target() {
            Target::Pretty | Target::Minified => self.format.language(),
            Target::To(format) => format.language(),
            Target::Csv => "csv",
        }
    }

    /// Whether the result is JSON, which the indent applies to.
    fn makes_json(&self) -> bool {
        matches!(
            (self.target(), self.format),
            (Target::To(Format::Json), _) | (Target::Pretty, Format::Json)
        )
    }

    fn run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().to_string();
        if text.trim().is_empty() {
            self._run = None;
            self.show(None, window, cx);
            return;
        }
        let (format, target, layout) = (self.format, self.target(), self.layout);
        self._run = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(SETTLE).await;
            let result = cx
                .background_spawn(
                    async move { crate::data::convert(format, target, &text, layout) },
                )
                .await;
            _ = this.update_in(cx, |this, window, cx| this.show(Some(result), window, cx));
        }));
    }

    fn show(
        &mut self,
        result: Option<Result<Converted, Problem>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = match &result {
            Some(Ok(converted)) => converted.text.clone(),
            _ => String::new(),
        };
        let language = self.output_language();
        self.output.update(cx, |output, cx| {
            output.set_highlighter(language, cx);
            output.set_value(text, window, cx);
        });
        self.result = result;
        cx.notify();
    }

    fn paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = parts::clipboard_text(cx) {
            self.take(text, window, cx);
        }
    }

    fn render_status(&self, cx: &App) -> impl IntoElement {
        let format = self.format.name();
        match &self.result {
            None => parts::label("Result", cx).into_any_element(),
            Some(Err(_)) => {
                parts::badge(format!("Not valid {format}"), Tone::Bad, cx).into_any_element()
            }
            Some(Ok(converted)) => {
                let mut said = vec![format!("Valid {format}")];
                if let Some(values) = converted.values {
                    said.push(format!("{values} values"));
                }
                if let Some(depth) = converted.depth.filter(|depth| *depth > 0) {
                    said.push(format!(
                        "{depth} level{} deep",
                        if depth == 1 { "" } else { "s" }
                    ));
                }
                said.push(size(converted.text.len()));
                h_flex()
                    .gap_2()
                    .min_w_0()
                    .child(parts::badge(said.remove(0), Tone::Good, cx))
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(said.join(" · ")),
                    )
                    .into_any_element()
            }
        }
    }

    fn render_options(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let indent = self.layout.indent;
        let json = self.makes_json();
        h_flex()
            .flex_none()
            .gap_3()
            .when(json, |this| {
                this.child(parts::modes(
                    "utility-indent",
                    &["2 spaces", "4 spaces", "Tab"],
                    match indent {
                        Indent::Two => 0,
                        Indent::Four => 1,
                        Indent::Tab => 2,
                    },
                    cx.listener(|this, ix: &usize, window, cx| {
                        this.layout.indent = match ix {
                            0 => Indent::Two,
                            1 => Indent::Four,
                            _ => Indent::Tab,
                        };
                        this.run(window, cx);
                    }),
                ))
            })
            .child(
                Checkbox::new("utility-sort-keys")
                    .small()
                    .label("Sort keys")
                    .checked(self.layout.sort_keys)
                    .on_click(cx.listener(|this, checked: &bool, window, cx| {
                        this.layout.sort_keys = *checked;
                        this.run(window, cx);
                    })),
            )
    }
}

/// `1.2 KB`, `830 bytes`.
pub(crate) fn size(bytes: usize) -> String {
    match bytes {
        0..1_000 => format!("{bytes} bytes"),
        1_000..1_000_000 => format!("{:.1} KB", bytes as f64 / 1e3),
        _ => format!("{:.1} MB", bytes as f64 / 1e6),
    }
}

impl ToolView for Formats {
    fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let format = match ctx.id {
            "yaml" => Format::Yaml,
            "toml" => Format::Toml,
            "xml" => Format::Xml,
            _ => Format::Json,
        };
        let placeholder = match format {
            Format::Json => {
                "Paste JSON: from a log, an API, a file. JSON kept in a string is read too."
            }
            Format::Yaml => "Paste YAML: a compose file, a workflow, a Kubernetes manifest",
            Format::Toml => "Paste TOML: Cargo.toml, pyproject.toml, a config",
            Format::Xml => "Paste XML: a layout, a manifest, an SVG",
        };
        let input = parts::editor(format.language(), placeholder, window, cx);
        let output = parts::editor(format.language(), "", window, cx);
        let subscription =
            cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.run(window, cx);
                }
            });
        Self {
            ctx,
            format,
            mode: 0,
            layout: Layout::default(),
            input,
            output,
            result: None,
            _input: subscription,
            _run: None,
        }
    }

    fn take(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, cx| input.set_value(text, window, cx));
        self.run(window, cx);
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }
}

impl Render for Formats {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let wide = super::wide(window);
        let input_text = self.input.read(cx).value().to_string();
        let output_text = match &self.result {
            Some(Ok(converted)) => converted.text.clone(),
            _ => String::new(),
        };
        let note = match &self.result {
            Some(Ok(converted)) => converted.note,
            _ => None,
        };
        let input = v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .gap_1()
            .child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .child(parts::label("Input", cx).flex_1())
                    .child(
                        parts::action("utility-paste", Icons::ClipboardPaste, "Paste")
                            .on_click(cx.listener(|this, _, window, cx| this.paste(window, cx))),
                    )
                    .child(
                        parts::action("utility-clear", Icons::Eraser, "Clear")
                            .disabled(input_text.is_empty())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.take(String::new(), window, cx);
                                this.focus(window, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(Editor::new(&self.input).h_full().small()),
            );
        let result_body = match &self.result {
            Some(Err(problem)) => v_flex()
                .gap_2()
                .child(parts::problem(problem, &input_text, cx))
                .children(parts::suggestion(&input_text, &self.ctx, cx))
                .into_any_element(),
            None => parts::surface(cx)
                .size_full()
                .child(parts::hint(
                    "The result shows here as you type or paste.",
                    cx,
                ))
                .into_any_element(),
            Some(Ok(_)) => Editor::new(&self.output)
                .h_full()
                .small()
                .readonly(true)
                .into_any_element(),
        };
        let output = v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .gap_1()
            .child(
                h_flex()
                    .flex_none()
                    .h(ph_ui::rems_from_px(24.))
                    .gap_1()
                    .child(div().flex_1().min_w_0().child(self.render_status(cx)))
                    .child(
                        ph_ui::CopyButton::new("utility-copy", output_text.clone())
                            .label("Copy")
                            .disabled(output_text.is_empty()),
                    ),
            )
            .when_some(note, |this, note| {
                this.child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(cx.theme().info)
                        .child(note),
                )
            })
            .child(div().flex_1().min_h_0().child(result_body));
        let mode = self.mode;
        let modes = self.modes();
        v_flex()
            .size_full()
            .gap_2()
            .child(parts::header(
                &self.ctx,
                parts::modes(
                    "utility-modes",
                    modes,
                    mode,
                    cx.listener(|this, ix: &usize, window, cx| {
                        this.mode = *ix;
                        this.run(window, cx);
                    }),
                ),
            ))
            .when(self.format != Format::Xml, |this| {
                this.child(self.render_options(cx))
            })
            .child(div().flex_1().min_h_0().map(|this| {
                if wide {
                    this.child(
                        h_flex()
                            .size_full()
                            .items_start()
                            .gap_3()
                            .child(input.h_full())
                            .child(output.h_full()),
                    )
                } else {
                    this.child(v_flex().size_full().gap_3().child(input).child(output))
                }
            }))
    }
}

/// Input over result, as text: for a utility without a view of its own.
pub(crate) struct Plain {
    ctx: Ctx,
    mode: usize,
    text: Entity<TextareaState>,
    second: Entity<TextareaState>,
    output: Result<String, String>,
    _inputs: Vec<Subscription>,
    _run: Option<Task<()>>,
}

impl Plain {
    fn run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = crate::Input {
            text: self.text.read(cx).value().to_string(),
            second: self.second.read(cx).value().to_string(),
            mode: self.mode,
        };
        let id = self.ctx.id;
        self._run = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(SETTLE).await;
            let output = cx
                .background_spawn(async move {
                    crate::run(id, &input).map_err(|err| format!("{err:#}"))
                })
                .await;
            _ = this.update(cx, |this, cx| {
                this.output = output;
                cx.notify();
            });
        }));
    }
}

impl ToolView for Plain {
    fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let utility = ctx.utility();
        let text = parts::area(utility.input, window, cx);
        let second = parts::area(utility.second.unwrap_or(""), window, cx);
        let inputs = [&text, &second]
            .into_iter()
            .map(|input| {
                cx.subscribe_in(input, window, |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.run(window, cx);
                    }
                })
            })
            .collect();
        Self {
            ctx,
            mode: 0,
            text,
            second,
            output: Ok(String::new()),
            _inputs: inputs,
            _run: None,
        }
    }

    fn take(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.text
            .update(cx, |input, cx| input.set_value(text, window, cx));
        self.run(window, cx);
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        self.text.update(cx, |input, cx| input.focus(window, cx));
    }
}

impl Render for Plain {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let utility = self.ctx.utility();
        let theme = cx.theme();
        let copy = self.output.clone().unwrap_or_default();
        v_flex()
            .size_full()
            .gap_2()
            .child(parts::header(
                &self.ctx,
                div().when(utility.modes.len() > 1, |this| {
                    this.child(parts::modes(
                        "utility-modes",
                        utility.modes,
                        self.mode,
                        cx.listener(|this, ix: &usize, window, cx| {
                            this.mode = *ix;
                            this.run(window, cx);
                        }),
                    ))
                }),
            ))
            .child(Textarea::new(&self.text).h(ph_ui::rems_from_px(110.)))
            .when(utility.second.is_some(), |this| {
                this.child(Textarea::new(&self.second).h(ph_ui::rems_from_px(70.)))
            })
            .child(
                h_flex()
                    .gap_1()
                    .child(parts::label("Result", cx).flex_1())
                    .child(
                        ph_ui::CopyButton::new("utility-copy", copy.clone())
                            .label("Copy")
                            .disabled(copy.is_empty()),
                    ),
            )
            .child(
                parts::surface(cx)
                    .id("utility-output")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_2()
                    .font_family(theme.mono_font_family.clone())
                    .text_xs()
                    .map(|this| match &self.output {
                        Ok(text) => this.child(text.clone()),
                        Err(err) => this.text_color(theme.danger).child(err.clone()),
                    }),
            )
    }
}

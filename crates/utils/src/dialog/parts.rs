//! What the utilities' views are made of: the heading with its switch, a
//! value that is copied by a click, a badge, a problem shown at its line,
//! the offer to open a text in the utility that reads it, and editors.

use std::time::Duration;

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{EditorState, InputState, TextareaState},
    tab::{Tab, TabBar},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, ClipboardItem, Div, ElementId, Entity, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div,
};

use super::Ctx;

/// The utility's name, and its switch (or other controls) at the right.
pub(crate) fn header(ctx: &Ctx, controls: impl IntoElement) -> Div {
    h_flex()
        .flex_none()
        .h_7()
        .gap_3()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_base()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .child(ctx.utility().name),
        )
        .child(controls)
}

/// The ways to run a utility, as the one switch other panes have for two
/// views of one thing.
pub(crate) fn modes(
    id: &'static str,
    labels: &[&'static str],
    selected: usize,
    on_click: impl Fn(&usize, &mut Window, &mut App) + 'static,
) -> TabBar {
    TabBar::new(id)
        .segmented()
        .small()
        .selected_index(selected)
        .on_click(on_click)
        .children(labels.iter().map(|label| Tab::new().label(*label)))
}

/// A small label over a field or a section.
pub(crate) fn label(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

/// A ghost button with an icon, for a view's actions.
pub(crate) fn action(
    id: impl Into<ElementId>,
    icon: impl Into<Icon>,
    text: &'static str,
) -> Button {
    Button::new(id).ghost().xsmall().icon(icon).label(text)
}

/// A muted line in place of a result not there yet.
pub(crate) fn hint(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .p_2()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

/// An error, in red.
pub(crate) fn failure(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .p_2()
        .rounded(cx.theme().radius)
        .bg(cx.theme().danger.opacity(0.08))
        .text_sm()
        .text_color(cx.theme().danger)
        .child(text.into())
}

/// What could not be read, with the line it went wrong at and a mark under
/// the place, so it is found without counting lines.
pub(crate) fn problem(problem: &crate::data::Problem, input: &str, cx: &App) -> Div {
    let theme = cx.theme();
    let excerpt = problem.line.and_then(|line| {
        let text = input.lines().nth(line.checked_sub(1)?)?;
        let column = problem.column.unwrap_or(1).saturating_sub(1);
        // Long lines (minified JSON) are cut to the part around the place.
        let chars: Vec<char> = text.chars().collect();
        let start = column.saturating_sub(24);
        let end = (column + 24).min(chars.len());
        let shown: String = chars[start.min(chars.len())..end].iter().collect();
        let lead = if start > 0 { "…" } else { "" };
        let caret = column - start + lead.chars().count();
        Some((format!("{lead}{shown}"), caret, line))
    });
    let where_ = match (problem.line, problem.column) {
        (Some(line), Some(column)) => format!("Line {line}, column {column}"),
        (Some(line), None) => format!("Line {line}"),
        _ => String::new(),
    };
    v_flex()
        .gap_1()
        .p_2()
        .rounded(theme.radius)
        .bg(theme.danger.opacity(0.08))
        .child(
            div()
                .text_sm()
                .text_color(theme.danger)
                .child(problem.message.clone()),
        )
        .when(!where_.is_empty(), |this| {
            this.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(where_),
            )
        })
        .when_some(excerpt, |this, (line, caret, _)| {
            this.child(
                // On one line each, or the mark would not stand under its
                // place.
                v_flex()
                    .overflow_x_hidden()
                    .font_family(theme.mono_font_family.clone())
                    .text_xs()
                    .child(div().whitespace_nowrap().child(line))
                    .child(
                        div()
                            .whitespace_nowrap()
                            .text_color(theme.danger)
                            .child(format!("{}^", " ".repeat(caret))),
                    ),
            )
        })
}

/// When another utility reads `text` (a JWT pasted into JSON): says what
/// it looks like, with a button that opens it there.
pub(crate) fn suggestion(text: &str, ctx: &Ctx, cx: &App) -> Option<AnyElement> {
    let id = crate::detect::guess(text)
        .into_iter()
        .find(|id| *id != ctx.id)?;
    let utility = crate::find(id)?;
    let (ctx, text) = (ctx.clone(), text.to_string());
    Some(
        h_flex()
            .gap_2()
            .px_2()
            .py_1()
            .rounded(cx.theme().radius)
            .bg(cx.theme().info.opacity(0.08))
            .text_sm()
            .child(
                Icon::new(IconName::Info)
                    .xsmall()
                    .text_color(cx.theme().info),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(format!("This looks like {}.", what(id))),
            )
            .child(
                Button::new("utility-open-suggested")
                    .xsmall()
                    .primary()
                    .label(format!("Open in {}", utility.name))
                    .on_click(move |_, window, cx| ctx.open_in(id, text.clone(), window, cx)),
            )
            .into_any_element(),
    )
}

/// What a text that utility `id` reads is called: "a JWT", "JSON".
fn what(id: &str) -> &'static str {
    match id {
        "json" => "JSON",
        "yaml" => "YAML",
        "toml" => "TOML",
        "xml" => "XML",
        "csv" => "CSV",
        "base64" => "Base64",
        "url" => "a URL, or percent-encoded text",
        "hex" => "hex",
        "html" => "HTML with entities",
        "jwt" => "a JSON Web Token",
        "x509" => "a certificate",
        "ids" => "a UUID or ULID",
        "time" => "a time",
        "cron" => "a cron line",
        "color" => "a color",
        "ip" => "an IP address or network",
        "numbers" => "a number in another base",
        _ => "something another utility reads",
    }
}

/// How a badge reads.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    Good,
    Warn,
    Bad,
    Plain,
}

/// A short state in a pill: *Valid*, *Expires in 3 hours*, *AA*.
pub(crate) fn badge(text: impl Into<SharedString>, tone: Tone, cx: &App) -> Div {
    let theme = cx.theme();
    let color = match tone {
        Tone::Good => theme.success,
        Tone::Warn => theme.warning,
        Tone::Bad => theme.danger,
        Tone::Plain => theme.muted_foreground,
    };
    // In a row of its own, so that it is as wide as its words wherever it
    // is put (a block would stretch it across).
    h_flex().flex_none().child(
        div()
            .px_1p5()
            .py_0p5()
            .rounded_full()
            .bg(color.opacity(0.14))
            .text_color(color)
            .text_xs()
            .font_weight(gpui_kit::FontWeight::MEDIUM)
            .child(text.into()),
    )
}

/// A panel that holds a result, set apart from the inputs.
pub(crate) fn surface(cx: &App) -> Div {
    div()
        .rounded(cx.theme().radius)
        .bg(cx.theme().muted.opacity(0.5))
}

/// How long a copied value shows its tick.
const SHOWN: Duration = Duration::from_millis(1400);

/// A named value, copied by a click anywhere on it: a hash, a date, a
/// color's form. Its copy icon shows on hover and turns into a tick.
#[derive(IntoElement)]
pub(crate) struct Value {
    id: ElementId,
    label: SharedString,
    value: SharedString,
    note: Option<SharedString>,
    color: Option<Hsla>,
    mono: bool,
    label_width: f32,
}

impl Value {
    pub fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        value: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            value: value.into(),
            note: None,
            color: None,
            mono: true,
            label_width: 112.,
        }
    }

    /// Words after the value, not copied with it ("3 days ago").
    pub fn note(mut self, note: impl Into<SharedString>) -> Self {
        self.note = Some(note.into());
        self
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    /// In the interface's font rather than the code font.
    pub fn plain(mut self) -> Self {
        self.mono = false;
        self
    }

    pub fn label_width(mut self, width: f32) -> Self {
        self.label_width = width;
        self
    }
}

impl RenderOnce for Value {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let copied = window.use_keyed_state(self.id.clone(), cx, |_, _| false);
        let done = *copied.read(cx);
        let theme = cx.theme();
        let group = SharedString::from(format!("value-{:?}", self.id));
        let value = self.value.clone();
        h_flex()
            .id(self.id.clone())
            .group(group.clone())
            .w_full()
            .items_start()
            .gap_3()
            .px_2()
            .py_1()
            .rounded(theme.radius)
            .cursor_pointer()
            .hover(|style| style.bg(theme.list_hover))
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new("Click to copy").build(window, cx)
            })
            .on_click(move |_, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(value.to_string()));
                copied.update(cx, |copied, cx| {
                    *copied = true;
                    cx.notify();
                });
                let copied = copied.clone();
                cx.spawn(async move |cx| {
                    cx.background_executor().timer(SHOWN).await;
                    copied.update(cx, |copied, cx| {
                        *copied = false;
                        cx.notify();
                    });
                })
                .detach();
            })
            .child(
                div()
                    .w(ph_ui::rems_from_px(self.label_width))
                    .flex_none()
                    .pt_0p5()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(self.label),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .when(self.mono, |this| {
                        this.font_family(theme.mono_font_family.clone())
                    })
                    .when_some(self.color, |this, color| this.text_color(color))
                    .child(self.value),
            )
            .when_some(self.note, |this, note| {
                this.child(
                    div()
                        .flex_none()
                        .pt_0p5()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(note),
                )
            })
            .child(
                div()
                    .flex_none()
                    .mt_0p5()
                    .when(!done, |this| {
                        this.invisible().group_hover(group, |style| style.visible())
                    })
                    .child(
                        Icon::new(if done {
                            IconName::Check
                        } else {
                            IconName::Copy
                        })
                        .xsmall()
                        .text_color(if done {
                            theme.success
                        } else {
                            theme.muted_foreground
                        }),
                    ),
            )
    }
}

/// A code editor coloured as `language`, like the file editor.
pub(crate) fn editor(
    language: &str,
    placeholder: &str,
    window: &mut Window,
    cx: &mut App,
) -> Entity<EditorState> {
    let (language, placeholder) = (language.to_string(), placeholder.to_string());
    cx.new(|cx| {
        let mut editor = EditorState::new(window, cx);
        editor.set_highlighter_factory(ph_files::highlighter(), cx);
        editor
            .searchable(true)
            .language(language)
            .line_number(true)
            .soft_wrap(true)
            .folding(true)
            .placeholder(placeholder)
    })
}

/// A box of plain text over several lines.
pub(crate) fn area(placeholder: &str, window: &mut Window, cx: &mut App) -> Entity<TextareaState> {
    let placeholder = placeholder.to_string();
    cx.new(|cx| TextareaState::new(window, cx).placeholder(placeholder))
}

/// A one-line field.
pub(crate) fn line(placeholder: &str, window: &mut Window, cx: &mut App) -> Entity<InputState> {
    let placeholder = placeholder.to_string();
    cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
}

/// The clipboard's text, if it holds any.
pub(crate) fn clipboard_text(cx: &App) -> Option<String> {
    cx.read_from_clipboard().and_then(|item| item.text())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_detected_kind_has_words() {
        for text in [
            "{}",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.c2ln",
            "1700000000",
            "#fff",
            "10.0.0.0/8",
        ] {
            for id in crate::detect::guess(text) {
                assert_ne!(what(id), "something another utility reads", "{id}");
            }
        }
    }
}

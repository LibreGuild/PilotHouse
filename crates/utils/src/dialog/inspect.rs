//! Text tools, number bases and IP networks: one input, and what it is in
//! every form, each copied by a click.
//!
//! Text tools also find what cannot be seen: a zero-width space in a key
//! that will not match, a typographic quote pasted into code, a Cyrillic
//! letter in a name that looks Latin. They are listed by line, and the text
//! can be copied without them.

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _,
    checkbox::Checkbox,
    h_flex,
    input::{Input as Field, InputEvent, InputState, Textarea, TextareaState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
};
use ph_ui::Icons;

use super::parts::{self, Tone, Value};
use super::{Ctx, ToolView};
use crate::words::{LineWork, Sort};

pub(crate) struct TextTools {
    ctx: Ctx,
    mode: usize,
    text: Entity<TextareaState>,
    work: LineWork,
    _input: Subscription,
}

impl ToolView for TextTools {
    fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let text = parts::area(
            "Text: a name to recase, a paragraph to count, lines to sort",
            window,
            cx,
        );
        let subscription = cx.subscribe(&text, |_, _, _: &InputEvent, cx| cx.notify());
        Self {
            ctx,
            mode: 0,
            text,
            work: LineWork {
                sort: Some(Sort::Natural),
                unique: false,
                trim: false,
                drop_blank: false,
                reverse: false,
            },
            _input: subscription,
        }
    }

    fn take(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.text
            .update(cx, |input, cx| input.set_value(text, window, cx));
        cx.notify();
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        self.text.update(cx, |text, cx| text.focus(window, cx));
    }
}

/// The text without what cannot be seen, and with straight quotes, plain
/// spaces and hyphens: what code and keys expect.
fn cleaned(text: &str) -> String {
    text.chars()
        .filter_map(|c| match c {
            '\u{200B}'
            | '\u{200C}'
            | '\u{200D}'
            | '\u{2060}'
            | '\u{FEFF}'
            | '\u{00AD}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}' => None,
            '\u{00A0}' | '\u{202F}' | '\u{2007}' | '\u{2009}' | '\u{200A}' | '\u{2002}'
            | '\u{2003}' => Some(' '),
            '\u{2018}' | '\u{2019}' => Some('\''),
            '\u{201C}' | '\u{201D}' => Some('"'),
            '\u{2013}' | '\u{2014}' | '\u{2212}' => Some('-'),
            c => Some(c),
        })
        .collect()
}

impl TextTools {
    fn render_count(&self, text: &str, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let counts = crate::words::counts(text);
        let tile = |value: usize, name: &'static str| {
            v_flex()
                .flex_1()
                .min_w(ph_ui::rems_from_px(84.))
                .p_2()
                .rounded(theme.radius)
                .bg(theme.muted.opacity(0.5))
                .child(
                    div()
                        .text_lg()
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .child(value.to_string()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(name),
                )
        };
        let odd = crate::words::odd_characters(text);
        let clean = cleaned(text);
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(tile(counts.characters, "characters"))
                    .child(tile(counts.words, "words"))
                    .child(tile(counts.lines, "lines"))
                    .child(tile(counts.bytes, "bytes (UTF-8)"))
                    .child(tile(counts.utf16, "length in JavaScript")),
            )
            .child(if odd.is_empty() {
                h_flex()
                    .px_2()
                    .child(parts::badge(
                        "Nothing hidden: no invisible or look-alike characters",
                        Tone::Good,
                        cx,
                    ))
                    .into_any_element()
            } else {
                v_flex()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .px_2()
                            .gap_2()
                            .child(parts::badge(
                                format!(
                                    "{} character{} you may not see",
                                    odd.len(),
                                    if odd.len() == 1 { "" } else { "s" }
                                ),
                                Tone::Warn,
                                cx,
                            ))
                            .child(div().flex_1())
                            .child(
                                ph_ui::CopyButton::new("text-copy-clean", clean)
                                    .label("Copy without them"),
                            ),
                    )
                    .children(odd.iter().take(200).enumerate().map(|(ix, odd)| {
                        Value::new(
                            SharedString::from(format!("text-odd-{ix}")),
                            format!("Line {}, char {}", odd.line, odd.at + 1),
                            odd.name,
                        )
                        .plain()
                        .note(odd.code.clone())
                    }))
                    .into_any_element()
            })
            .into_any_element()
    }

    fn render_lines(&self, text: &str, cx: &mut Context<Self>) -> AnyElement {
        let work = self.work;
        let result = crate::words::lines(text, work);
        let before = text.lines().count();
        let after = if result.is_empty() {
            0
        } else {
            result.lines().count()
        };
        let check =
            |id: &'static str, label: &'static str, on: bool, set: fn(&mut LineWork, bool)| {
                Checkbox::new(id)
                    .small()
                    .label(label)
                    .checked(on)
                    .on_click(cx.listener(move |this, value: &bool, _, cx| {
                        set(&mut this.work, *value);
                        cx.notify();
                    }))
            };
        v_flex()
            .size_full()
            .gap_2()
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .child(parts::modes(
                        "text-sort",
                        &["Keep order", "A to Z", "By length"],
                        match work.sort {
                            None => 0,
                            Some(Sort::Natural) => 1,
                            Some(Sort::Length) => 2,
                        },
                        cx.listener(|this, ix: &usize, _, cx| {
                            this.work.sort = match ix {
                                0 => None,
                                1 => Some(Sort::Natural),
                                _ => Some(Sort::Length),
                            };
                            cx.notify();
                        }),
                    ))
                    .child(check(
                        "text-unique",
                        "Remove repeats",
                        work.unique,
                        |w, on| w.unique = on,
                    ))
                    .child(check("text-trim", "Trim", work.trim, |w, on| w.trim = on))
                    .child(check(
                        "text-blank",
                        "Drop blank lines",
                        work.drop_blank,
                        |w, on| w.drop_blank = on,
                    ))
                    .child(check("text-reverse", "Reverse", work.reverse, |w, on| {
                        w.reverse = on
                    })),
            )
            .child(
                h_flex()
                    .child(parts::label(format!("{before} lines → {after}"), cx).flex_1())
                    .child(
                        ph_ui::CopyButton::new("text-copy-lines", result.clone())
                            .label("Copy")
                            .disabled(result.is_empty()),
                    ),
            )
            .child(
                parts::surface(cx)
                    .id("text-lines")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_2()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_xs()
                    .child(result),
            )
            .into_any_element()
    }
}

impl Render for TextTools {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let text = self.text.read(cx).value().to_string();
        let content: AnyElement = if text.is_empty() {
            parts::hint("Type or paste some text.", cx).into_any_element()
        } else {
            match self.mode {
                0 => v_flex()
                    .gap_0p5()
                    .children(crate::words::cases(&text).into_iter().map(|(name, value)| {
                        Value::new(SharedString::from(format!("case-{name}")), name, value)
                            .label_width(136.)
                    }))
                    .into_any_element(),
                1 => self.render_count(&text, cx),
                _ => self.render_lines(&text, cx),
            }
        };
        v_flex()
            .size_full()
            .gap_2()
            .child(parts::header(
                &self.ctx,
                parts::modes(
                    "utility-modes",
                    &["Cases", "Count and find", "Lines"],
                    self.mode,
                    cx.listener(|this, ix: &usize, _, cx| {
                        this.mode = *ix;
                        cx.notify();
                    }),
                ),
            ))
            .child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .child(parts::label("Text", cx).flex_1())
                    .child(
                        parts::action("utility-paste", Icons::ClipboardPaste, "Paste").on_click(
                            cx.listener(|this, _, window, cx| {
                                if let Some(text) = parts::clipboard_text(cx) {
                                    this.take(text, window, cx);
                                }
                            }),
                        ),
                    )
                    .child(
                        parts::action("utility-clear", Icons::Eraser, "Clear")
                            .disabled(text.is_empty())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.take(String::new(), window, cx);
                                this.focus(window, cx);
                            })),
                    ),
            )
            .child(
                Textarea::new(&self.text)
                    .h(ph_ui::rems_from_px(110.))
                    .flex_none(),
            )
            .child(
                v_flex()
                    .id("text-body")
                    .flex_1()
                    .min_h_0()
                    .when(self.mode != 2, |this| this.overflow_y_scroll())
                    .child(content),
            )
    }
}

pub(crate) struct Numbers {
    ctx: Ctx,
    input: Entity<InputState>,
    _input: Subscription,
}

impl ToolView for Numbers {
    fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = parts::line("A number: 255, 0xff, 0o377, 0b1111_1111, -12", window, cx);
        let subscription = cx.subscribe(&input, |_, _, _: &InputEvent, cx| cx.notify());
        Self {
            ctx,
            input,
            _input: subscription,
        }
    }

    fn take(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| {
            input.set_value(text.trim().to_string(), window, cx)
        });
        cx.notify();
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }
}

impl Render for Numbers {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let text = self.input.read(cx).value().to_string();
        let content: AnyElement = if text.trim().is_empty() {
            parts::hint(
                "Decimal, hex (0x or with a–f), octal (0o) or binary (0b).",
                cx,
            )
            .into_any_element()
        } else {
            match crate::numbers::read(&text) {
                Ok(value) => v_flex()
                    .gap_0p5()
                    .children(
                        crate::numbers::forms(value)
                            .into_iter()
                            .map(|(name, value)| {
                                Value::new(
                                    SharedString::from(format!("number-{name}")),
                                    name,
                                    value,
                                )
                            }),
                    )
                    .into_any_element(),
                Err(err) => v_flex()
                    .gap_2()
                    .child(parts::failure(format!("{err:#}"), cx))
                    .children(parts::suggestion(&text, &self.ctx, cx))
                    .into_any_element(),
            }
        };
        v_flex()
            .size_full()
            .gap_2()
            .child(parts::header(&self.ctx, div()))
            .child(Field::new(&self.input).small().flex_none())
            .child(
                v_flex()
                    .id("numbers-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(content),
            )
    }
}

pub(crate) struct Ip {
    ctx: Ctx,
    network: Entity<InputState>,
    address: Entity<InputState>,
    _inputs: Vec<Subscription>,
}

impl ToolView for Ip {
    fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let network = parts::line(
            "A network: 10.0.0.0/24, 192.168.1.0 255.255.255.0, fd00::/8, or one address",
            window,
            cx,
        );
        let address = parts::line("Is this address in it? (optional)", window, cx);
        let inputs = vec![
            cx.subscribe(&network, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe(&address, |_, _, _: &InputEvent, cx| cx.notify()),
        ];
        Self {
            ctx,
            network,
            address,
            _inputs: inputs,
        }
    }

    fn take(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.network.update(cx, |input, cx| {
            input.set_value(text.trim().to_string(), window, cx)
        });
        cx.notify();
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        self.network.update(cx, |input, cx| input.focus(window, cx));
    }
}

impl Render for Ip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let text = self.network.read(cx).value().to_string();
        let address = self.address.read(cx).value().to_string();
        let content: AnyElement = if text.trim().is_empty() {
            parts::hint("Type a network to see its range, mask and size.", cx).into_any_element()
        } else {
            match crate::net::read(&text) {
                Ok(network) => {
                    let inside = (!address.trim().is_empty()).then(|| {
                        address
                            .trim()
                            .parse::<std::net::IpAddr>()
                            .map(|address| network.contains(address))
                    });
                    v_flex()
                        .gap_2()
                        .child(v_flex().gap_0p5().children(
                            crate::net::facts(network).into_iter().map(|(name, value)| {
                                let row = Value::new(
                                    SharedString::from(format!("ip-{name}")),
                                    name,
                                    value,
                                );
                                if name == "Kind" { row.plain() } else { row }
                            }),
                        ))
                        .child(div().px_2().child(Field::new(&self.address).small()))
                        .children(inside.map(|inside| {
                            div().px_2().child(match inside {
                                Ok(true) => parts::badge("It is in this network", Tone::Good, cx),
                                Ok(false) => {
                                    parts::badge("It is not in this network", Tone::Bad, cx)
                                }
                                Err(_) => parts::badge("That is not an IP address", Tone::Warn, cx),
                            })
                        }))
                        .into_any_element()
                }
                Err(err) => v_flex()
                    .gap_2()
                    .child(parts::failure(format!("{err:#}"), cx))
                    .children(parts::suggestion(&text, &self.ctx, cx))
                    .into_any_element(),
            }
        };
        v_flex()
            .size_full()
            .gap_2()
            .child(parts::header(&self.ctx, div()))
            .child(Field::new(&self.network).small().flex_none())
            .child(
                v_flex()
                    .id("ip-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(content),
            )
    }
}

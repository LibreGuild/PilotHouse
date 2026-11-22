//! New ids and secrets, made as soon as the view opens (nothing to type),
//! each copied by a click, and made again with one button. An id pasted
//! in says what kind it is and when it was made.

use gpui_kit::component::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::{Input as Field, InputEvent, InputState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
};

use super::parts::{self, Tone, Value};
use super::{Ctx, ToolView};
use crate::hash::{Characters, IdKind};

const COUNTS: [usize; 4] = [1, 5, 10, 50];

pub(crate) struct Ids {
    ctx: Ctx,
    kind: usize,
    count: usize,
    upper: bool,
    no_dashes: bool,
    made: Vec<String>,
    read: Entity<InputState>,
    _read: Subscription,
}

impl Ids {
    fn kind(&self) -> IdKind {
        [IdKind::UuidV4, IdKind::UuidV7, IdKind::Ulid][self.kind]
    }

    fn make(&mut self, cx: &mut Context<Self>) {
        let kind = self.kind();
        self.made = (0..COUNTS[self.count])
            .filter_map(|_| crate::hash::new_id(kind).ok())
            .collect();
        cx.notify();
    }

    /// An id as the options ask: upper case, without dashes.
    fn shown(&self, id: &str) -> String {
        let uuid = self.kind() != IdKind::Ulid;
        let mut id = id.to_string();
        if uuid && self.no_dashes {
            id = id.replace('-', "");
        }
        if uuid && self.upper {
            id = id.to_uppercase();
        }
        id
    }
}

impl ToolView for Ids {
    fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let read = parts::line(
            "Paste a UUID or ULID to see its kind and when it was made",
            window,
            cx,
        );
        let subscription = cx.subscribe(&read, |_, _, _: &InputEvent, cx| cx.notify());
        let mut view = Self {
            ctx,
            kind: 0,
            count: 1,
            upper: false,
            no_dashes: false,
            made: Vec::new(),
            read,
            _read: subscription,
        };
        view.make(cx);
        view
    }

    fn take(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.read
            .update(cx, |read, cx| read.set_value(text, window, cx));
        cx.notify();
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        self.read.update(cx, |read, cx| read.focus(window, cx));
    }
}

impl Render for Ids {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shown: Vec<String> = self.made.iter().map(|id| self.shown(id)).collect();
        let all = shown.join("\n");
        let uuid = self.kind() != IdKind::Ulid;
        let about = match self.kind() {
            IdKind::UuidV4 => "Random. The usual choice when order does not matter.",
            IdKind::UuidV7 => {
                "Starts with the time: sorts by when it was made, which keeps database indexes compact."
            }
            IdKind::Ulid => "Like UUID v7, written in 26 characters that sort as text.",
        };
        let text = self.read.read(cx).value().to_string();
        let facts = (!text.trim().is_empty()).then(|| crate::hash::read_id(&text));
        v_flex()
            .size_full()
            .gap_2()
            .child(parts::header(
                &self.ctx,
                parts::modes(
                    "utility-modes",
                    &["UUID v4", "UUID v7", "ULID"],
                    self.kind,
                    cx.listener(|this, ix: &usize, _, cx| {
                        this.kind = *ix;
                        this.make(cx);
                    }),
                ),
            ))
            .child(parts::label(about, cx))
            .child(
                h_flex()
                    .flex_none()
                    .gap_3()
                    .child(parts::modes(
                        "ids-count",
                        &["1", "5", "10", "50"],
                        self.count,
                        cx.listener(|this, ix: &usize, _, cx| {
                            this.count = *ix;
                            this.make(cx);
                        }),
                    ))
                    .when(uuid, |this| {
                        this.child(
                            Checkbox::new("ids-upper")
                                .small()
                                .label("Upper case")
                                .checked(self.upper)
                                .on_click(cx.listener(|this, on: &bool, _, cx| {
                                    this.upper = *on;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Checkbox::new("ids-no-dashes")
                                .small()
                                .label("No dashes")
                                .checked(self.no_dashes)
                                .on_click(cx.listener(|this, on: &bool, _, cx| {
                                    this.no_dashes = *on;
                                    cx.notify();
                                })),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        Button::new("ids-again")
                            .ghost()
                            .xsmall()
                            .icon(IconName::RefreshCw)
                            .label("New ones")
                            .on_click(cx.listener(|this, _, _, cx| this.make(cx))),
                    )
                    .when(shown.len() > 1, |this| {
                        this.child(ph_ui::CopyButton::new("ids-copy-all", all).label("Copy all"))
                    }),
            )
            .child(
                v_flex()
                    .id("ids-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .gap_0p5()
                    .children(shown.into_iter().enumerate().map(|(ix, id)| {
                        Value::new(
                            SharedString::from(format!("id-{ix}")),
                            format!("{}", ix + 1),
                            id,
                        )
                        .label_width(28.)
                    })),
            )
            .child(
                v_flex()
                    .flex_none()
                    .gap_1()
                    .pt_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(parts::label("Read an id", cx))
                    .child(Field::new(&self.read).small())
                    .map(|this| match facts {
                        None => this,
                        Some(Err(err)) => this.child(parts::failure(format!("{err:#}"), cx)),
                        Some(Ok(facts)) => this.child(
                            v_flex()
                                .gap_0p5()
                                .child(Value::new("id-kind", "Kind", facts.kind.clone()).plain())
                                .when_some(facts.made, |this, made| {
                                    this.child(
                                        Value::new(
                                            "id-made",
                                            "Made",
                                            made.to_zoned(jiff::tz::TimeZone::system())
                                                .strftime("%a %Y-%m-%d %H:%M:%S %Z")
                                                .to_string(),
                                        )
                                        .note(crate::time::from_now(made)),
                                    )
                                })
                                .when_some(facts.note.clone(), |this, note| {
                                    this.child(Value::new("id-note", "About", note).plain())
                                }),
                        ),
                    }),
            )
    }
}

const LENGTHS: [usize; 6] = [12, 16, 24, 32, 48, 64];
const KEY_BYTES: [usize; 3] = [16, 32, 64];

pub(crate) struct Secrets {
    ctx: Ctx,
    /// 0 a password, 1 a key in hex, 2 a key in Base64.
    kind: usize,
    length: usize,
    bytes: usize,
    characters: Characters,
    made: Vec<String>,
    error: Option<String>,
}

impl Secrets {
    fn make(&mut self, cx: &mut Context<Self>) {
        let made: anyhow::Result<Vec<String>> = (0..5)
            .map(|_| match self.kind {
                0 => crate::hash::password(LENGTHS[self.length], self.characters),
                kind => crate::hash::random_key(KEY_BYTES[self.bytes], kind == 2),
            })
            .collect();
        match made {
            Ok(made) => {
                self.made = made;
                self.error = None;
            }
            Err(err) => {
                self.made.clear();
                self.error = Some(format!("{err:#}"));
            }
        }
        cx.notify();
    }

    fn render_characters(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let characters = self.characters;
        let check =
            |id: &'static str, label: &'static str, on: bool, set: fn(&mut Characters, bool)| {
                Checkbox::new(id)
                    .small()
                    .label(label)
                    .checked(on)
                    .on_click(cx.listener(move |this, value: &bool, _, cx| {
                        set(&mut this.characters, *value);
                        this.make(cx);
                    }))
            };
        h_flex()
            .flex_wrap()
            .gap_3()
            .child(check("secret-lower", "a–z", characters.lower, |c, on| {
                c.lower = on
            }))
            .child(check("secret-upper", "A–Z", characters.upper, |c, on| {
                c.upper = on
            }))
            .child(check(
                "secret-digits",
                "0–9",
                characters.digits,
                |c, on| c.digits = on,
            ))
            .child(check(
                "secret-symbols",
                "!#$%",
                characters.symbols,
                |c, on| c.symbols = on,
            ))
            .child(
                check(
                    "secret-unambiguous",
                    "No look-alikes",
                    characters.unambiguous,
                    |c, on| c.unambiguous = on,
                )
                .tooltip(
                    "Leaves out l, I, 1, O, 0 and o, for a password read out or typed from paper",
                ),
            )
    }
}

impl ToolView for Secrets {
    fn new(ctx: Ctx, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            ctx,
            kind: 0,
            length: 2,
            bytes: 1,
            characters: Characters::default(),
            made: Vec::new(),
            error: None,
        };
        view.make(cx);
        view
    }

    fn take(&mut self, text: String, _: &mut Window, cx: &mut Context<Self>) {
        if let Ok(length) = text.trim().parse::<usize>()
            && let Some(ix) = LENGTHS.iter().position(|value| *value >= length)
        {
            self.length = ix;
            self.make(cx);
        }
    }

    fn focus(&self, _: &mut Window, _: &mut App) {}
}

impl Render for Secrets {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bits = if self.kind == 0 {
            self.characters.bits(LENGTHS[self.length])
        } else {
            KEY_BYTES[self.bytes] as f64 * 8.0
        };
        let (strength, tone) = match bits {
            bits if bits >= 128.0 => ("Very strong", Tone::Good),
            bits if bits >= 80.0 => ("Strong", Tone::Good),
            bits if bits >= 60.0 => ("Fair", Tone::Warn),
            _ => ("Weak", Tone::Bad),
        };
        v_flex()
            .size_full()
            .gap_2()
            .child(parts::header(
                &self.ctx,
                parts::modes(
                    "utility-modes",
                    &["Password", "Hex key", "Base64 key"],
                    self.kind,
                    cx.listener(|this, ix: &usize, _, cx| {
                        this.kind = *ix;
                        this.make(cx);
                    }),
                ),
            ))
            .child(parts::label(
                "Made on this computer with the system's secure random source; nothing is kept.",
                cx,
            ))
            .map(|this| {
                if self.kind == 0 {
                    this.child(
                        h_flex()
                            .flex_none()
                            .gap_2()
                            .child(parts::label("Length", cx))
                            .child(parts::modes(
                                "secret-length",
                                &["12", "16", "24", "32", "48", "64"],
                                self.length,
                                cx.listener(|this, ix: &usize, _, cx| {
                                    this.length = *ix;
                                    this.make(cx);
                                }),
                            )),
                    )
                    .child(self.render_characters(cx))
                } else {
                    this.child(
                        h_flex()
                            .flex_none()
                            .gap_2()
                            .child(parts::label("Bytes", cx))
                            .child(parts::modes(
                                "secret-bytes",
                                &["16 (128 bits)", "32 (256 bits)", "64 (512 bits)"],
                                self.bytes,
                                cx.listener(|this, ix: &usize, _, cx| {
                                    this.bytes = *ix;
                                    this.make(cx);
                                }),
                            )),
                    )
                }
            })
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .child(parts::badge(
                        format!("{strength} · {} bits", bits.round()),
                        tone,
                        cx,
                    ))
                    .child(div().flex_1())
                    .child(
                        Button::new("secrets-again")
                            .ghost()
                            .xsmall()
                            .icon(IconName::RefreshCw)
                            .label("New ones")
                            .on_click(cx.listener(|this, _, _, cx| this.make(cx))),
                    ),
            )
            .when_some(self.error.clone(), |this, error| {
                this.child(parts::failure(error, cx))
            })
            .child(
                v_flex()
                    .id("secrets-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .gap_0p5()
                    .children(self.made.iter().enumerate().map(|(ix, secret)| {
                        Value::new(
                            SharedString::from(format!("secret-{ix}")),
                            format!("{}", ix + 1),
                            secret.clone(),
                        )
                        .label_width(28.)
                    })),
            )
    }
}

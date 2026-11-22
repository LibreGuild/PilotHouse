//! Tokens, certificates and hashes, shown as what they say rather than as
//! text: a JWT's claims with their dates and whether it has expired, a
//! certificate's days left and chain, every hash of a text or a file with
//! the one that matches a checksum marked, an HMAC checked as a webhook
//! sends it.

use std::path::PathBuf;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::{
        Editor, EditorState, Input as Field, InputEvent, InputState, TextDecoration,
        TextDecorationCollection, Textarea, TextareaState,
    },
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, HighlightStyle, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Task, Window, div,
};
use ph_ui::Icons;
use serde_json::Value as Json;

use super::parts::{self, Tone, Value};
use super::{Ctx, ToolView};
use crate::encode::{self, Signature};

/// A view's scrolling body under its heading.
fn body(id: &'static str) -> gpui_kit::Stateful<gpui_kit::Div> {
    v_flex()
        .id(id)
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .gap_3()
}

/// A section's title over its rows.
fn section(title: impl Into<SharedString>, cx: &App) -> gpui_kit::Div {
    div()
        .px_2()
        .pt_1()
        .text_xs()
        .font_weight(gpui_kit::FontWeight::MEDIUM)
        .text_color(cx.theme().muted_foreground)
        .child(title.into())
}

/// The label row over an input: its name and its Paste and Clear.
fn input_bar(
    label: &'static str,
    paste: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static,
    clear: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static,
    empty: bool,
    cx: &App,
) -> gpui_kit::Div {
    h_flex()
        .flex_none()
        .gap_1()
        .child(parts::label(label, cx).flex_1())
        .child(parts::action("utility-paste", Icons::ClipboardPaste, "Paste").on_click(paste))
        .child(
            parts::action("utility-clear", Icons::Eraser, "Clear")
                .disabled(empty)
                .on_click(clear),
        )
}

/// "in 3 hours", "2 days ago".
fn when(seconds: i64) -> String {
    jiff::Timestamp::from_second(seconds)
        .map(crate::time::from_now)
        .unwrap_or_default()
}

/// A time in this computer's zone.
fn local(seconds: i64) -> String {
    jiff::Timestamp::from_second(seconds)
        .map(|at| {
            at.to_zoned(jiff::tz::TimeZone::system())
                .strftime("%a %Y-%m-%d %H:%M:%S %Z")
                .to_string()
        })
        .unwrap_or_else(|_| seconds.to_string())
}

pub(crate) struct Jwt {
    ctx: Ctx,
    /// The token, its three parts coloured in place.
    token: Entity<EditorState>,
    colors: Option<TextDecorationCollection>,
    secret: Entity<InputState>,
    base64_secret: bool,
    _inputs: Vec<Subscription>,
}

impl ToolView for Jwt {
    fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let token = cx.new(|cx| {
            EditorState::new(window, cx)
                .line_number(false)
                .soft_wrap(true)
                .placeholder("Paste a JSON Web Token (a Bearer in front is fine)")
        });
        let secret = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("The secret, to check an HS256/384/512 signature")
                .masked(true)
        });
        let inputs = vec![
            cx.subscribe(&token, |this, _, _: &InputEvent, cx| this.color(cx)),
            cx.subscribe(&secret, |_, _, _: &InputEvent, cx| cx.notify()),
        ];
        Self {
            ctx,
            token,
            colors: None,
            secret,
            base64_secret: false,
            _inputs: inputs,
        }
    }

    fn take(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.token
            .update(cx, |token, cx| token.set_value(text, window, cx));
        self.color(cx);
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        self.token.update(cx, |token, cx| token.focus(window, cx));
    }
}

impl Jwt {
    /// Colours the token's three parts where it was pasted, as jwt.io
    /// does: header, claims, signature.
    fn color(&mut self, cx: &mut Context<Self>) {
        let text = self.token.read(cx).value().to_string();
        let theme = cx.theme();
        let colors = [theme.danger, theme.info, theme.success];
        let mut marks = Vec::new();
        if encode::jwt_read(&text).is_ok() {
            let mut at = 0;
            for (ix, part) in text.split('.').enumerate() {
                let end = at + part.len();
                // A Bearer in front stays uncoloured.
                let start = if ix == 0 {
                    at + part.rfind(' ').map_or(0, |space| space + 1)
                } else {
                    at
                };
                marks.push(TextDecoration::new(
                    start..end,
                    HighlightStyle {
                        color: Some(colors[ix.min(2)]),
                        ..Default::default()
                    },
                ));
                at = end + 1;
            }
        }
        match &self.colors {
            Some(collection) => collection.set(marks, cx),
            None => {
                self.colors = Some(self.token.update(cx, |token, cx| {
                    token.create_decorations_collection(marks, cx)
                }))
            }
        }
        cx.notify();
    }

    fn render_token(&self, token: &encode::Jwt, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let now = jiff::Timestamp::now().as_second();
        let claims = token.claims.as_object().cloned().unwrap_or_default();
        let seconds = |key: &str| claims.get(key).and_then(Json::as_i64);
        let mut badges = vec![parts::badge(token.algorithm().to_string(), Tone::Plain, cx)];
        match (seconds("nbf"), seconds("exp")) {
            (Some(from), _) if from > now => badges.push(parts::badge(
                format!("Not valid until {}", when(from)),
                Tone::Warn,
                cx,
            )),
            (_, Some(until)) if until <= now => badges.push(parts::badge(
                format!("Expired {}", when(until)),
                Tone::Bad,
                cx,
            )),
            (_, Some(until)) => badges.push(parts::badge(
                format!("Valid, expires {}", when(until)),
                Tone::Good,
                cx,
            )),
            _ => badges.push(parts::badge("Never expires", Tone::Warn, cx)),
        }
        if let Some(issued) = seconds("iat") {
            badges.push(parts::badge(
                format!("Issued {}", when(issued)),
                Tone::Plain,
                cx,
            ));
        }
        let shown = |value: &Json| match value {
            Json::String(text) => text.clone(),
            Json::Array(items) if items.iter().all(Json::is_string) => items
                .iter()
                .filter_map(Json::as_str)
                .collect::<Vec<_>>()
                .join(", "),
            other => other.to_string(),
        };
        let claim_rows: Vec<AnyElement> = claims
            .iter()
            .map(|(key, value)| {
                let label = match encode::claim_meaning(key) {
                    Some(meaning) => format!("{meaning} ({key})"),
                    None => key.clone(),
                };
                let row = Value::new(
                    SharedString::from(format!("jwt-claim-{key}")),
                    label,
                    shown(value),
                );
                match (key.as_str(), value.as_i64()) {
                    ("exp" | "iat" | "nbf" | "auth_time", Some(at)) => Value::new(
                        SharedString::from(format!("jwt-claim-{key}")),
                        format!("{} ({key})", encode::claim_meaning(key).unwrap_or(key)),
                        local(at),
                    )
                    .note(format!("{at} · {}", when(at))),
                    _ => row,
                }
                .into_any_element()
            })
            .collect();
        let header_rows: Vec<AnyElement> = token
            .header
            .as_object()
            .into_iter()
            .flatten()
            .map(|(key, value)| {
                Value::new(
                    SharedString::from(format!("jwt-header-{key}")),
                    key.clone(),
                    shown(value),
                )
                .into_any_element()
            })
            .collect();
        let secret = self.secret.read(cx).value().to_string();
        let verdict =
            (!secret.is_empty()).then(|| encode::jwt_verify(token, &secret, self.base64_secret));
        let claims_json = serde_json::to_string_pretty(&token.claims).unwrap_or_default();
        let header_json = serde_json::to_string_pretty(&token.header).unwrap_or_default();
        v_flex()
            .gap_3()
            .child(h_flex().flex_wrap().gap_1p5().children(badges))
            .child(
                v_flex()
                    .gap_0p5()
                    .child(
                        h_flex().child(section("Claims", cx).flex_1()).child(
                            ph_ui::CopyButton::new("jwt-copy-claims", claims_json)
                                .label("Copy as JSON"),
                        ),
                    )
                    .children(claim_rows),
            )
            .child(
                v_flex()
                    .gap_0p5()
                    .child(
                        h_flex().child(section("Header", cx).flex_1()).child(
                            ph_ui::CopyButton::new("jwt-copy-header", header_json)
                                .label("Copy as JSON"),
                        ),
                    )
                    .children(header_rows),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(section("Signature", cx))
                    .child(
                        h_flex()
                            .gap_2()
                            .px_2()
                            .child(
                                div()
                                    .flex_1()
                                    .child(Field::new(&self.secret).small().mask_toggle()),
                            )
                            .child(
                                Checkbox::new("jwt-secret-base64")
                                    .small()
                                    .label("Secret is Base64")
                                    .checked(self.base64_secret)
                                    .on_click(cx.listener(|this, on: &bool, _, cx| {
                                        this.base64_secret = *on;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(div().px_2().map(|this| {
                        match verdict {
                            None => this.child(parts::label(
                                "Nothing is sent anywhere: the signature is checked here.",
                                cx,
                            )),
                            Some(Signature::Valid) => this.child(parts::badge(
                                "The signature is valid for this secret",
                                Tone::Good,
                                cx,
                            )),
                            Some(Signature::Invalid) => this.child(parts::badge(
                                "The signature does not match this secret",
                                Tone::Bad,
                                cx,
                            )),
                            Some(Signature::Unchecked(why)) => this.child(
                                div()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child(why),
                            ),
                        }
                    })),
            )
            .into_any_element()
    }
}

impl Render for Jwt {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let text = self.token.read(cx).value().to_string();
        let read = (!text.trim().is_empty()).then(|| encode::jwt_read(&text));
        let content = match &read {
            None => parts::hint(
                "Paste a token to see its claims, when it expires, and whether its signature holds.",
                cx,
            )
            .into_any_element(),
            Some(Err(err)) => v_flex()
                .gap_2()
                .child(parts::failure(format!("{err:#}"), cx))
                .children(parts::suggestion(&text, &self.ctx, cx))
                .into_any_element(),
            Some(Ok(token)) => self.render_token(token, cx),
        };
        v_flex()
            .size_full()
            .gap_2()
            .child(parts::header(&self.ctx, div()))
            .child(input_bar(
                "Token",
                cx.listener(|this, _, window, cx| {
                    if let Some(text) = parts::clipboard_text(cx) {
                        this.take(text, window, cx);
                    }
                }),
                cx.listener(|this, _, window, cx| {
                    this.take(String::new(), window, cx);
                    this.focus(window, cx);
                }),
                text.is_empty(),
                cx,
            ))
            .child(
                Editor::new(&self.token)
                    .small()
                    .h(ph_ui::rems_from_px(84.))
                    .flex_none(),
            )
            .child(body("jwt-body").child(content))
    }
}

pub(crate) struct Certificates {
    ctx: Ctx,
    pem: Entity<TextareaState>,
    /// A DER file opened, which is not text.
    file: Option<(String, Vec<u8>)>,
    error: Option<String>,
    _input: Subscription,
}

impl ToolView for Certificates {
    fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let pem = parts::area(
            "Paste a certificate or a chain (-----BEGIN CERTIFICATE-----), or open a .pem, .crt or .cer file",
            window,
            cx,
        );
        let input = cx.subscribe(&pem, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.file = None;
                this.error = None;
                cx.notify();
            }
        });
        Self {
            ctx,
            pem,
            file: None,
            error: None,
            _input: input,
        }
    }

    fn take(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.pem
            .update(cx, |pem, cx| pem.set_value(text, window, cx));
        self.file = None;
        cx.notify();
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        self.pem.update(cx, |pem, cx| pem.focus(window, cx));
    }
}

impl Certificates {
    fn open_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Read".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let read = cx.background_spawn(async move { read_small(&path) }).await;
            _ = this.update_in(cx, |this, window, cx| match read {
                Ok((name, bytes)) => match String::from_utf8(bytes.clone()) {
                    Ok(text) if text.contains("-----BEGIN") => this.take(text, window, cx),
                    _ => {
                        this.take(String::new(), window, cx);
                        this.file = Some((name, bytes));
                        cx.notify();
                    }
                },
                Err(err) => {
                    this.error = Some(err);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn render_card(
        &self,
        ix: usize,
        cert: &crate::cert::Certificate,
        count: usize,
        cx: &App,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let now = jiff::Timestamp::now().as_second();
        let days = cert.days_left(now);
        let (state, tone) = if now < cert.not_before {
            (
                format!("Not valid until {}", when(cert.not_before)),
                Tone::Warn,
            )
        } else if now > cert.not_after {
            (format!("Expired {}", when(cert.not_after)), Tone::Bad)
        } else if days < 30 {
            (format!("Expires in {days} days"), Tone::Warn)
        } else {
            (format!("Valid · {days} days left"), Tone::Good)
        };
        let role = if cert.self_signed && cert.ca {
            "Root"
        } else if cert.self_signed {
            "Self-signed"
        } else if ix > 0 || cert.ca {
            "Intermediate"
        } else if count > 1 {
            "Leaf"
        } else {
            "Certificate"
        };
        // How much of its life has passed.
        let lived = ((now - cert.not_before) as f32
            / (cert.not_after - cert.not_before).max(1) as f32)
            .clamp(0., 1.);
        let id = |name: &str| SharedString::from(format!("cert-{ix}-{name}"));
        let mut rows = vec![
            Value::new(id("subject"), "Subject", cert.subject.clone()).into_any_element(),
            Value::new(id("issuer"), "Issued by", cert.issuer.clone()).into_any_element(),
            Value::new(id("from"), "Valid from", crate::cert::day(cert.not_before))
                .note(when(cert.not_before))
                .into_any_element(),
            Value::new(id("to"), "Valid until", crate::cert::day(cert.not_after))
                .note(when(cert.not_after))
                .into_any_element(),
        ];
        if !cert.names.is_empty() {
            rows.push(
                Value::new(id("names"), "Names", cert.names.join(", "))
                    .note(format!("{}", cert.names.len()))
                    .into_any_element(),
            );
        }
        rows.extend([
            Value::new(id("key"), "Key", cert.key.clone()).into_any_element(),
            Value::new(id("signature"), "Signature", cert.signature.clone()).into_any_element(),
            Value::new(
                id("ca"),
                "Can sign others",
                if cert.ca { "Yes (a CA)" } else { "No" },
            )
            .plain()
            .into_any_element(),
            Value::new(id("serial"), "Serial", cert.serial.clone()).into_any_element(),
            Value::new(id("sha256"), "SHA-256", cert.sha256.clone()).into_any_element(),
            Value::new(id("sha1"), "SHA-1", cert.sha1.clone()).into_any_element(),
        ]);
        v_flex()
            .gap_1()
            .p_2()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_2()
                    .px_2()
                    .child(parts::badge(role, Tone::Plain, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(
                                cert.common_name
                                    .clone()
                                    .unwrap_or_else(|| cert.subject.clone()),
                            ),
                    )
                    .child(parts::badge(state, tone, cx)),
            )
            .child(
                div().px_2().child(
                    div()
                        .h(ph_ui::rems_from_px(3.))
                        .w_full()
                        .rounded_full()
                        .bg(theme.muted)
                        .child(
                            div()
                                .h_full()
                                .rounded_full()
                                .w(gpui_kit::relative(lived))
                                .bg(match tone {
                                    Tone::Good => theme.success,
                                    Tone::Warn => theme.warning,
                                    _ => theme.danger,
                                }),
                        ),
                ),
            )
            .children(rows)
    }
}

/// A small file's name and bytes (certificates, keys).
fn read_small(path: &std::path::Path) -> Result<(String, Vec<u8>), String> {
    let size = std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    if size > 4 << 20 {
        return Err("That file is too large to be a certificate.".into());
    }
    let bytes = std::fs::read(path).map_err(|err| err.to_string())?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok((name, bytes))
}

impl Render for Certificates {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let text = self.pem.read(cx).value().to_string();
        let bytes: Option<Vec<u8>> = match &self.file {
            Some((_, bytes)) => Some(bytes.clone()),
            None => (!text.trim().is_empty()).then(|| text.clone().into_bytes()),
        };
        let content = match (&self.error, bytes.map(|bytes| crate::cert::certificates(&bytes))) {
            (Some(err), _) => parts::failure(err.clone(), cx).into_any_element(),
            (None, None) => parts::hint(
                "Paste a certificate to see who it is for, who signed it, and how long it has left.",
                cx,
            )
            .into_any_element(),
            (None, Some(Err(err))) => v_flex()
                .gap_2()
                .child(parts::failure(format!("{err:#}"), cx))
                .children(parts::suggestion(&text, &self.ctx, cx))
                .into_any_element(),
            (None, Some(Ok(certs))) => {
                // Each one signed by the next, as a server sends its chain.
                let broken = certs
                    .windows(2)
                    .position(|pair| pair[0].issuer != pair[1].subject);
                v_flex()
                    .gap_2()
                    .when(certs.len() > 1, |this| {
                        this.child(match broken {
                            None => parts::badge(
                                format!("A chain of {}: each signed by the next", certs.len()),
                                Tone::Good,
                                cx,
                            ),
                            Some(at) => parts::badge(
                                format!(
                                    "Certificate {} was not signed by certificate {}: out of order, or one is missing",
                                    at + 1,
                                    at + 2
                                ),
                                Tone::Warn,
                                cx,
                            ),
                        })
                    })
                    .children(
                        certs
                            .iter()
                            .enumerate()
                            .map(|(ix, cert)| self.render_card(ix, cert, certs.len(), cx)),
                    )
                    .into_any_element()
            }
        };
        let file = self.file.as_ref().map(|(name, _)| name.clone());
        v_flex()
            .size_full()
            .gap_2()
            .child(parts::header(&self.ctx, div()))
            .child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .child(parts::label("Certificate", cx).flex_1())
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
                        parts::action("utility-open-file", Icons::FolderOpen, "Open file…")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_file(window, cx)),
                            ),
                    )
                    .child(
                        parts::action("utility-clear", Icons::Eraser, "Clear")
                            .disabled(text.is_empty() && self.file.is_none())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.take(String::new(), window, cx);
                                this.error = None;
                                this.focus(window, cx);
                            })),
                    ),
            )
            .map(|this| match file {
                Some(name) => this.child(
                    h_flex()
                        .flex_none()
                        .gap_2()
                        .child(parts::badge(name, Tone::Plain, cx))
                        .child(parts::label("read as DER", cx)),
                ),
                None => this.child(
                    Textarea::new(&self.pem)
                        .h(ph_ui::rems_from_px(90.))
                        .flex_none(),
                ),
            })
            .child(body("cert-body").child(content))
    }
}

/// What is hashed: the text typed, or a file.
enum Source {
    Text,
    File {
        name: String,
        size: u64,
        hashes: Option<Result<Vec<crate::hash::Hash>, String>>,
    },
}

pub(crate) struct Hashes {
    ctx: Ctx,
    text: Entity<TextareaState>,
    expected: Entity<InputState>,
    source: Source,
    _inputs: Vec<Subscription>,
    _hashing: Option<Task<()>>,
}

impl ToolView for Hashes {
    fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let text = parts::area("Text to hash, or open a file", window, cx);
        let expected = parts::line(
            "A checksum to compare: hex, Base64, sha256:…, or an SRI hash (sha384-…)",
            window,
            cx,
        );
        let inputs = vec![
            cx.subscribe(&text, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.source = Source::Text;
                    cx.notify();
                }
            }),
            cx.subscribe(&expected, |_, _, _: &InputEvent, cx| cx.notify()),
        ];
        Self {
            ctx,
            text,
            expected,
            source: Source::Text,
            _inputs: inputs,
            _hashing: None,
        }
    }

    fn take(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.text
            .update(cx, |input, cx| input.set_value(text, window, cx));
        self.source = Source::Text;
        cx.notify();
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        self.text.update(cx, |text, cx| text.focus(window, cx));
    }
}

impl Hashes {
    fn open_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Hash".into()),
        });
        self._hashing = Some(cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            let Some(path): Option<PathBuf> = paths.into_iter().next() else {
                return;
            };
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
            _ = this.update(cx, |this, cx| {
                this.source = Source::File {
                    name: name.clone(),
                    size,
                    hashes: None,
                };
                cx.notify();
            });
            let hashed = cx
                .background_spawn(async move {
                    crate::hash::file_hashes(&path)
                        .map(|(_, hashes)| hashes)
                        .map_err(|err| format!("{err:#}"))
                })
                .await;
            _ = this.update(cx, |this, cx| {
                this.source = Source::File {
                    name,
                    size,
                    hashes: Some(hashed),
                };
                cx.notify();
            });
        }));
    }
}

impl Render for Hashes {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let text = self.text.read(cx).value().to_string();
        let expected = self.expected.read(cx).value().to_string();
        let hashes: Result<Vec<crate::hash::Hash>, String> = match &self.source {
            Source::Text => Ok(crate::hash::hashes(text.as_bytes())),
            Source::File { hashes: None, .. } => Ok(Vec::new()),
            Source::File {
                hashes: Some(hashes),
                ..
            } => hashes.clone(),
        };
        let hashing = matches!(self.source, Source::File { hashes: None, .. });
        let matched = hashes
            .as_ref()
            .ok()
            .filter(|_| !expected.trim().is_empty())
            .map(|hashes| crate::hash::matching(&expected, hashes).map(|hash| hash.name));
        let rows: Vec<AnyElement> = match &hashes {
            Err(err) => vec![parts::failure(err.clone(), cx).into_any_element()],
            Ok(hashes) => hashes
                .iter()
                .map(|hash| {
                    let weak = matches!(hash.name, "MD5" | "SHA-1");
                    let row = Value::new(
                        SharedString::from(format!("hash-{}", hash.name)),
                        hash.name,
                        hash.hex.clone(),
                    );
                    let row = if matched == Some(Some(hash.name)) {
                        row.color(theme.success).note("matches")
                    } else if weak {
                        row.note("not for security")
                    } else {
                        row
                    };
                    row.into_any_element()
                })
                .chain(
                    hashes
                        .iter()
                        .find(|hash| hash.name == "SHA-384")
                        .map(|hash| {
                            Value::new("hash-sri", "SRI", format!("sha384-{}", hash.base64))
                                .note("integrity=")
                                .into_any_element()
                        }),
                )
                .collect(),
        };
        let file = match &self.source {
            Source::File { name, size, .. } => {
                Some(format!("{name} · {}", super::data::size(*size as usize)))
            }
            Source::Text => None,
        };
        v_flex()
            .size_full()
            .gap_2()
            .child(parts::header(&self.ctx, div()))
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
                        parts::action("utility-open-file", Icons::FolderOpen, "Hash a file…")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_file(window, cx)),
                            ),
                    )
                    .child(
                        parts::action("utility-clear", Icons::Eraser, "Clear")
                            .disabled(text.is_empty() && file.is_none())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.take(String::new(), window, cx);
                                this.focus(window, cx);
                            })),
                    ),
            )
            .map(|this| match file {
                Some(file) => this.child(
                    h_flex()
                        .flex_none()
                        .gap_2()
                        .child(parts::badge(file, Tone::Plain, cx))
                        .when(hashing, |this| this.child(parts::label("Hashing…", cx)))
                        .child(
                            Button::new("hash-back-to-text")
                                .ghost()
                                .xsmall()
                                .label("Hash text instead")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.source = Source::Text;
                                    this._hashing = None;
                                    this.focus(window, cx);
                                    cx.notify();
                                })),
                        ),
                ),
                None => this.child(
                    Textarea::new(&self.text)
                        .h(ph_ui::rems_from_px(96.))
                        .flex_none(),
                ),
            })
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .child(div().flex_1().child(Field::new(&self.expected).small()))
                    .children(matched.map(|matched| match matched {
                        Some(name) => parts::badge(format!("Matches {name}"), Tone::Good, cx),
                        None => parts::badge("No hash matches", Tone::Bad, cx),
                    })),
            )
            .child(body("hash-body").gap_0p5().children(rows))
    }
}

pub(crate) struct Hmac {
    ctx: Ctx,
    algorithm: usize,
    message: Entity<TextareaState>,
    key: Entity<InputState>,
    expected: Entity<InputState>,
    _inputs: Vec<Subscription>,
}

const MACS: [crate::hash::MacAlgorithm; 3] = [
    crate::hash::MacAlgorithm::Sha256,
    crate::hash::MacAlgorithm::Sha1,
    crate::hash::MacAlgorithm::Sha512,
];

impl ToolView for Hmac {
    fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let message = parts::area(
            "The message: a webhook's body exactly as received",
            window,
            cx,
        );
        let key = parts::line("The key (the webhook's signing secret)", window, cx);
        let expected = parts::line(
            "A signature to check: sha256=…, hex or Base64 (optional)",
            window,
            cx,
        );
        let inputs = vec![
            cx.subscribe(&key, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe(&expected, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe(&message, |_, _, _: &InputEvent, cx| cx.notify()),
        ];
        Self {
            ctx,
            algorithm: 0,
            message,
            key,
            expected,
            _inputs: inputs,
        }
    }

    fn take(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.message
            .update(cx, |message, cx| message.set_value(text, window, cx));
        cx.notify();
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        self.message
            .update(cx, |message, cx| message.focus(window, cx));
    }
}

impl Render for Hmac {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let message = self.message.read(cx).value().to_string();
        let key = self.key.read(cx).value().to_string();
        let expected = self.expected.read(cx).value().to_string();
        let mac = crate::hash::hmac_of(MACS[self.algorithm], key.as_bytes(), message.as_bytes());
        let hex = ::hex::encode(&mac);
        let base64 = encode::base64_encode(&mac, false);
        let prefix = ["sha256", "sha1", "sha512"][self.algorithm];
        let checked =
            (!expected.trim().is_empty()).then(|| crate::hash::signature_matches(&expected, &mac));
        let ready = !key.is_empty();
        v_flex()
            .size_full()
            .gap_2()
            .child(parts::header(
                &self.ctx,
                parts::modes(
                    "utility-modes",
                    &["SHA-256", "SHA-1", "SHA-512"],
                    self.algorithm,
                    cx.listener(|this, ix: &usize, _, cx| {
                        this.algorithm = *ix;
                        cx.notify();
                    }),
                ),
            ))
            .child(parts::label("Message", cx))
            .child(Textarea::new(&self.message).h(ph_ui::rems_from_px(96.)).flex_none())
            .child(parts::label("Key", cx))
            .child(Field::new(&self.key).small().mask_toggle())
            .child(
                body("hmac-body")
                    .gap_0p5()
                    .map(|this| {
                        if !ready {
                            return this.child(parts::hint("Type the key to sign the message.", cx));
                        }
                        this.child(Value::new("hmac-hex", "Hex", hex.clone()))
                            .child(Value::new("hmac-base64", "Base64", base64))
                            .child(
                                Value::new("hmac-header", "As a header", format!("{prefix}={hex}"))
                                    .note("GitHub's X-Hub-Signature-256"),
                            )
                    })
                    .child(div().h_2())
                    .child(div().px_2().child(Field::new(&self.expected).small()))
                    .children(checked.filter(|_| ready).map(|matches| {
                        div().px_2().pt_1().child(if matches {
                            parts::badge("The signature matches", Tone::Good, cx)
                        } else {
                            parts::badge(
                                "The signature does not match: check the key, and that the body is byte for byte what was sent",
                                Tone::Bad,
                                cx,
                            )
                        })
                    })),
            )
    }
}

//! Base64, URL encoding, hex and HTML entities: the text and its encoded
//! form side by side, both editable. Typing on either side writes the
//! other, so there is no Encode or Decode to choose first.
//!
//! What Base64 or hex holds is not always text: then it is shown as bytes
//! (a hex dump), as a picture when it is one, and can be saved as a file.
//! A file can be encoded too (an icon as a data URI). A URL on the text
//! side is also taken apart, its query decoded.

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::{InputEvent, Textarea, TextareaState},
    notification::Notification,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, Image, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, img,
};
use ph_ui::Icons;

use super::parts::{self, Tone, Value};
use super::{Ctx, ToolView};
use crate::encode;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Base64,
    Url,
    Hex,
    Html,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Text,
    Encoded,
}

/// Bytes that are not text: decoded, or read from a file to encode.
struct Bytes {
    bytes: Arc<Vec<u8>>,
    mime: Option<String>,
    /// The file they came from, when they were read from one.
    file: Option<String>,
    picture: Option<Arc<Image>>,
    size: Option<(u32, u32)>,
}

impl Bytes {
    fn new(bytes: Vec<u8>, mime: Option<String>, file: Option<String>) -> Self {
        let mime = encode::sniff(&bytes).map(str::to_string).or(mime);
        let size = encode::picture_size(&bytes);
        // Only raster kinds within a sane size are drawn: GPUI would read
        // other bytes as SVG (which can name files), and a picture of a
        // hundred thousand pixels a side would take gigabytes.
        let format = ph_ui::images_safe::raster_format(&bytes)
            .filter(|format| !matches!(format, gpui_kit::ImageFormat::Gif));
        let picture = match (format, size) {
            (Some(format), Some((width, height))) if width <= 8192 && height <= 8192 => {
                Some(Arc::new(Image::from_bytes(format, bytes.clone())))
            }
            _ => None,
        };
        Self {
            bytes: Arc::new(bytes),
            mime,
            file,
            picture,
            size,
        }
    }

    fn describe(&self) -> String {
        let mut said = vec![super::data::size(self.bytes.len())];
        if let Some(mime) = &self.mime {
            said.push(mime.clone());
        }
        if let Some((width, height)) = self.size {
            said.push(format!("{width} × {height}"));
        }
        said.join(" · ")
    }
}

pub(crate) struct Codec {
    ctx: Ctx,
    kind: Kind,
    text: Entity<TextareaState>,
    encoded: Entity<TextareaState>,
    /// The side typed in last, written again when an option changes.
    from: Side,
    /// What was last written into each side from the other: its change
    /// event (which may come after the write) is not typing.
    written: [Option<String>; 2],
    error: Option<String>,
    /// What the encoded side holds, when it is not text; or the file being
    /// encoded.
    bytes: Option<Bytes>,
    url_safe: bool,
    /// Hex: 0 none, 1 space, 2 colon between bytes.
    separator: usize,
    upper: bool,
    non_ascii: bool,
    _inputs: Vec<Subscription>,
}

impl Codec {
    fn names(&self) -> (&'static str, &'static str) {
        match self.kind {
            Kind::Base64 => ("Text", "Base64"),
            Kind::Url => ("Text", "Encoded"),
            Kind::Hex => ("Text", "Hex"),
            Kind::Html => ("Text", "With entities"),
        }
    }

    fn encode_bytes(&self, bytes: &[u8]) -> String {
        match self.kind {
            Kind::Base64 => encode::base64_encode(bytes, self.url_safe),
            Kind::Hex => encode::hex_encode(bytes, ["", " ", ":"][self.separator], self.upper),
            Kind::Url => encode::url_encode(&String::from_utf8_lossy(bytes)),
            Kind::Html => encode::html_escape(&String::from_utf8_lossy(bytes), self.non_ascii),
        }
    }

    /// Writes the other side from the one typed in.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.error = None;
        match self.from {
            Side::Text => {
                if let Some(file) = self.bytes.as_ref().filter(|bytes| bytes.file.is_some()) {
                    let encoded = self.encode_bytes(&file.bytes.clone());
                    self.write(Side::Encoded, encoded, window, cx);
                    cx.notify();
                    return;
                }
                self.bytes = None;
                let text = self.text.read(cx).value().to_string();
                let encoded = self.encode_bytes(text.as_bytes());
                self.write(Side::Encoded, encoded, window, cx);
            }
            Side::Encoded => {
                self.bytes = None;
                let encoded = self.encoded.read(cx).value().to_string();
                if encoded.trim().is_empty() {
                    self.write(Side::Text, String::new(), window, cx);
                    cx.notify();
                    return;
                }
                let decoded: Result<(Vec<u8>, Option<String>), String> = match self.kind {
                    Kind::Base64 => encode::base64_decode(&encoded)
                        .map(|decoded| (decoded.bytes, decoded.mime))
                        .map_err(|err| format!("{err:#}")),
                    Kind::Hex => encode::hex_decode(&encoded)
                        .map(|bytes| (bytes, None))
                        .map_err(|err| format!("{err:#}")),
                    Kind::Url => encode::url_decode(&encoded)
                        .map(|text| (text.into_bytes(), None))
                        .map_err(|err| format!("{err:#}")),
                    Kind::Html => Ok((encode::html_unescape(&encoded).into_bytes(), None)),
                };
                match decoded {
                    Err(err) => self.error = Some(err),
                    Ok((bytes, mime)) => match encode::as_text(&bytes) {
                        Some(text) => {
                            let text = text.to_string();
                            self.write(Side::Text, text, window, cx);
                        }
                        None => {
                            self.write(Side::Text, String::new(), window, cx);
                            self.bytes = Some(Bytes::new(bytes, mime, None));
                        }
                    },
                }
            }
        }
        cx.notify();
    }

    fn write(&mut self, side: Side, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let field = match side {
            Side::Text => self.text.clone(),
            Side::Encoded => self.encoded.clone(),
        };
        if field.read(cx).value() == text {
            return;
        }
        self.written[side as usize] = Some(text.clone());
        field.update(cx, |field, cx| field.set_value(text, window, cx));
    }

    fn typed(&mut self, side: Side, window: &mut Window, cx: &mut Context<Self>) {
        let value = match side {
            Side::Text => self.text.read(cx).value().to_string(),
            Side::Encoded => self.encoded.read(cx).value().to_string(),
        };
        if self.written[side as usize]
            .take()
            .is_some_and(|written| written == value)
        {
            return;
        }
        // Typing on the text side leaves a file that was being encoded.
        if side == Side::Text
            && self
                .bytes
                .as_ref()
                .is_some_and(|bytes| bytes.file.is_some())
        {
            self.bytes = None;
        }
        self.from = side;
        self.sync(window, cx);
    }

    fn paste(&mut self, side: Side, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = parts::clipboard_text(cx) else {
            return;
        };
        self.from = side;
        self.bytes = None;
        self.write(side, text, window, cx);
        self.sync(window, cx);
    }

    fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.bytes = None;
        self.write(Side::Text, String::new(), window, cx);
        self.write(Side::Encoded, String::new(), window, cx);
        self.error = None;
        self.from = Side::Text;
        self.text.update(cx, |text, cx| text.focus(window, cx));
        cx.notify();
    }

    /// Asks for a file and encodes it.
    fn open_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Encode".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let read = cx
                .background_spawn(async move {
                    let size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
                    if size > 20 << 20 {
                        return Err("Files up to 20 MB are encoded here.".to_string());
                    }
                    let bytes = std::fs::read(&path).map_err(|err| err.to_string())?;
                    let name = path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    Ok((name, bytes))
                })
                .await;
            _ = this.update_in(cx, |this, window, cx| match read {
                Ok((name, bytes)) => {
                    this.bytes = Some(Bytes::new(bytes, None, Some(name)));
                    this.write(Side::Text, String::new(), window, cx);
                    this.from = Side::Text;
                    this.sync(window, cx);
                }
                Err(err) => {
                    this.error = Some(err);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Asks where, and saves the decoded bytes there.
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bytes) = &self.bytes else {
            return;
        };
        let extension = match bytes.mime.as_deref() {
            Some("image/png") => "png",
            Some("image/jpeg") => "jpg",
            Some("image/gif") => "gif",
            Some("image/webp") => "webp",
            Some("image/bmp") => "bmp",
            Some("application/pdf") => "pdf",
            Some("application/zip") => "zip",
            Some("application/gzip") => "gz",
            _ => "bin",
        };
        let folder = dirs::download_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        let chosen = cx.prompt_for_new_path(&folder, Some(&format!("decoded.{extension}")));
        let data = bytes.bytes.clone();
        cx.spawn_in(window, async move |_, cx| {
            let Ok(Ok(Some(path))) = chosen.await else {
                return;
            };
            let written = cx
                .background_spawn({
                    let path = path.clone();
                    async move { std::fs::write(&path, data.as_slice()) }
                })
                .await;
            _ = cx.update(|window, cx| {
                let notification = match written {
                    Ok(()) => Notification::success(format!(
                        "Saved {}.",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    )),
                    Err(err) => Notification::error(format!("Could not save it: {err}")),
                };
                window.push_notification(notification, cx);
            });
        })
        .detach();
    }

    fn render_options(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex().gap_3().map(|this| match self.kind {
            Kind::Base64 => this.child(
                Checkbox::new("utility-url-safe")
                    .small()
                    .label("URL-safe")
                    .checked(self.url_safe)
                    .tooltip("- and _ in place of + and /, for URLs and file names")
                    .on_click(cx.listener(|this, on: &bool, window, cx| {
                        this.url_safe = *on;
                        this.from = Side::Text;
                        this.sync(window, cx);
                    })),
            ),
            Kind::Hex => this
                .child(parts::modes(
                    "utility-hex-separator",
                    &["ab12", "ab 12", "ab:12"],
                    self.separator,
                    cx.listener(|this, ix: &usize, window, cx| {
                        this.separator = *ix;
                        this.from = Side::Text;
                        this.sync(window, cx);
                    }),
                ))
                .child(
                    Checkbox::new("utility-hex-upper")
                        .small()
                        .label("Upper case")
                        .checked(self.upper)
                        .on_click(cx.listener(|this, on: &bool, window, cx| {
                            this.upper = *on;
                            this.from = Side::Text;
                            this.sync(window, cx);
                        })),
                ),
            Kind::Html => this.child(
                Checkbox::new("utility-html-non-ascii")
                    .small()
                    .label("Also é, ü, emoji…")
                    .checked(self.non_ascii)
                    .tooltip("Every character beyond ASCII as an entity (&#xE9;)")
                    .on_click(cx.listener(|this, on: &bool, window, cx| {
                        this.non_ascii = *on;
                        this.from = Side::Text;
                        this.sync(window, cx);
                    })),
            ),
            Kind::Url => this,
        })
    }

    /// The bytes that are not text: what they are, a picture or a dump.
    fn render_bytes(&self, bytes: &Bytes, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let from_file = bytes.file.clone();
        let picture = bytes
            .picture
            .clone()
            .zip(bytes.size)
            .map(|(picture, (width, height))| {
                // As large as fits in a box of 220 by 160, never larger than it is.
                let scale = (220. / width as f32).min(160. / height as f32).min(1.);
                img(picture)
                    .w(ph_ui::rems_from_px(width as f32 * scale))
                    .h(ph_ui::rems_from_px(height as f32 * scale))
                    .rounded(theme.radius)
            });
        let dump = encode::hex_dump(&bytes.bytes, 256);
        let uri_mime = bytes
            .mime
            .clone()
            .unwrap_or_else(|| "application/octet-stream".into());
        let data = bytes.bytes.clone();
        v_flex()
            .size_full()
            .gap_2()
            .p_2()
            .child(
                // Wraps: beside the other side, there is room for half of it.
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(parts::badge(
                        match &from_file {
                            Some(name) => name.clone(),
                            None => "Not text".to_string(),
                        },
                        Tone::Plain,
                        cx,
                    ))
                    .child(parts::label(bytes.describe(), cx).flex_1().min_w_0())
                    .when(self.kind == Kind::Base64, |this| {
                        this.child(
                            ph_ui::CopyButton::with("utility-copy-data-uri", move |_, _| {
                                encode::data_uri(&data, &uri_mime)
                            })
                            .label("Copy as data URI"),
                        )
                    })
                    .when(from_file.is_none(), |this| {
                        this.child(
                            parts::action(
                                "utility-save-bytes",
                                IconName::ArrowDown,
                                "Save as file…",
                            )
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                        )
                    }),
            )
            .children(picture)
            .child(
                // Lines of a dump stay whole (offset, bytes, characters line
                // up only so): a narrow column scrolls sideways.
                div()
                    .id("utility-hex-dump")
                    .flex_1()
                    .min_h_0()
                    .overflow_scrollbar()
                    .whitespace_nowrap()
                    .font_family(theme.mono_font_family.clone())
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(dump),
            )
    }

    fn render_url(&self, cx: &App) -> Option<gpui_kit::AnyElement> {
        if self.kind != Kind::Url {
            return None;
        }
        let parts = encode::url_parts(&self.text.read(cx).value()).ok()?;
        let mut rows = vec![Value::new("url-scheme", "Scheme", parts.scheme.clone())];
        if let Some(host) = &parts.host {
            rows.push(Value::new("url-host", "Host", host.clone()));
        }
        if let Some(port) = parts.port {
            rows.push(Value::new("url-port", "Port", port.to_string()));
        }
        if let Some(user) = &parts.user {
            rows.push(Value::new("url-user", "User", user.clone()));
        }
        rows.push(Value::new("url-path", "Path", parts.path.clone()));
        for (ix, (key, value)) in parts.query.iter().enumerate() {
            rows.push(Value::new(
                SharedString::from(format!("url-query-{ix}")),
                format!("?{key}"),
                value.clone(),
            ));
        }
        if let Some(fragment) = &parts.fragment {
            rows.push(Value::new("url-fragment", "#", fragment.clone()));
        }
        Some(
            v_flex()
                .flex_none()
                .gap_0p5()
                .child(parts::label("Parts (the query decoded)", cx).px_2())
                .children(rows)
                .into_any_element(),
        )
    }
}

impl ToolView for Codec {
    fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let kind = match ctx.id {
            "url" => Kind::Url,
            "hex" => Kind::Hex,
            "html" => Kind::Html,
            _ => Kind::Base64,
        };
        let (text_hint, encoded_hint) = match kind {
            Kind::Base64 => ("Text to encode", "Base64 or a data: URI to decode"),
            Kind::Url => ("Text or a URL", "Percent-encoded text to decode (%20…)"),
            Kind::Hex => ("Text to encode", "Hex to decode (68 69, 0x6869, 68:69)"),
            Kind::Html => (
                "Text to escape",
                "HTML with entities to read (&lt; &#65; &copy;)",
            ),
        };
        let text = parts::area(text_hint, window, cx);
        let encoded = parts::area(encoded_hint, window, cx);
        let inputs = vec![
            cx.subscribe_in(&text, window, |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.typed(Side::Text, window, cx);
                }
            }),
            cx.subscribe_in(
                &encoded,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.typed(Side::Encoded, window, cx);
                    }
                },
            ),
        ];
        Self {
            ctx,
            kind,
            text,
            encoded,
            from: Side::Text,
            written: [None, None],
            error: None,
            bytes: None,
            url_safe: false,
            separator: 0,
            upper: false,
            non_ascii: false,
            _inputs: inputs,
        }
    }

    /// Text handed over goes where it fits: an encoded form to be decoded
    /// goes on the encoded side.
    fn take(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let encoded = match self.kind {
            Kind::Base64 => encode::base64_decode(&text).is_ok() && !text.contains(' '),
            Kind::Hex => encode::hex_decode(&text).is_ok(),
            Kind::Url => text.contains('%') && !text.contains("://"),
            Kind::Html => text.contains('&') && text.contains(';'),
        };
        let side = if encoded { Side::Encoded } else { Side::Text };
        self.bytes = None;
        self.from = side;
        self.write(side, text, window, cx);
        self.sync(window, cx);
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        self.text.update(cx, |text, cx| text.focus(window, cx));
    }
}

impl Render for Codec {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let wide = super::wide(window);
        let (text_name, encoded_name) = self.names();
        let text_value = self.text.read(cx).value().to_string();
        let encoded_value = self.encoded.read(cx).value().to_string();
        let file = matches!(self.kind, Kind::Base64 | Kind::Hex);
        // The decoded text, offered to the utility that reads it.
        let offer = (self.from == Side::Encoded && self.error.is_none())
            .then(|| parts::suggestion(&text_value, &self.ctx, cx))
            .flatten();
        let refused = self
            .error
            .as_ref()
            .and_then(|_| parts::suggestion(&encoded_value, &self.ctx, cx));
        let text_side = v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .gap_1()
            .child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .child(parts::label(text_name, cx).flex_1())
                    .child(
                        parts::action("utility-paste-text", Icons::ClipboardPaste, "Paste")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.paste(Side::Text, window, cx)
                            })),
                    )
                    .when(file, |this| {
                        this.child(
                            parts::action("utility-open-file", Icons::FolderOpen, "Open file…")
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.open_file(window, cx)),
                                ),
                        )
                    })
                    .child(
                        ph_ui::CopyButton::new("utility-copy-text", text_value.clone())
                            .label("Copy")
                            .disabled(text_value.is_empty()),
                    ),
            )
            .child(div().flex_1().min_h_0().map(|this| {
                match &self.bytes {
                    Some(bytes) => this.child(
                        parts::surface(cx)
                            .size_full()
                            .child(self.render_bytes(bytes, cx)),
                    ),
                    None => this.child(Textarea::new(&self.text).h_full()),
                }
            }))
            .children(offer);
        let encoded_side = v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .gap_1()
            .child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .child(parts::label(encoded_name, cx).flex_1())
                    .child(
                        parts::action("utility-paste-encoded", Icons::ClipboardPaste, "Paste")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.paste(Side::Encoded, window, cx)
                            })),
                    )
                    .child(
                        ph_ui::CopyButton::new("utility-copy-encoded", encoded_value.clone())
                            .label("Copy")
                            .disabled(encoded_value.is_empty()),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(Textarea::new(&self.encoded).h_full()),
            )
            .when_some(self.error.clone(), |this, error| {
                this.child(parts::failure(error, cx).flex_none())
            })
            .children(refused);
        let url = self.render_url(cx);
        let empty = text_value.is_empty() && encoded_value.is_empty() && self.bytes.is_none();
        v_flex()
            .size_full()
            .gap_2()
            .child(parts::header(
                &self.ctx,
                h_flex().gap_3().child(self.render_options(cx)).child(
                    Button::new("utility-clear")
                        .ghost()
                        .xsmall()
                        .icon(Icons::Eraser)
                        .label("Clear")
                        .disabled(empty)
                        .on_click(cx.listener(|this, _, window, cx| this.clear(window, cx))),
                ),
            ))
            .child(parts::label("Type or paste on either side: the other follows.", cx).flex_none())
            .child(div().flex_1().min_h_0().map(|this| {
                if wide {
                    this.child(
                        h_flex()
                            .size_full()
                            .items_start()
                            .gap_3()
                            .child(text_side.h_full())
                            .child(encoded_side.h_full()),
                    )
                } else {
                    this.child(
                        v_flex()
                            .size_full()
                            .gap_3()
                            .child(text_side)
                            .child(encoded_side),
                    )
                }
            }))
            .when_some(url, |this, url| {
                this.child(
                    div()
                        .id("utility-url-parts")
                        .flex_none()
                        .max_h(ph_ui::rems_from_px(200.))
                        .overflow_y_scroll()
                        .child(url),
                )
            })
    }
}

//! Images (and SVG drawings and diagrams): fitted to the pane, zoomable
//! with the buttons or ⌘/Ctrl and the wheel, with their size and format.

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::component::{
    ActiveTheme as _, IconName, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Image, ImageFormat, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, Render, ScrollHandle, ScrollWheelEvent, SharedString,
    StatefulInteractiveElement as _, Styled as _, StyledImage as _, Task, Window, div, img, px,
};

use crate::byte_size;
use crate::mermaid::{Diagram, svg_size};

#[derive(Debug, Clone, Copy, PartialEq)]
enum Zoom {
    /// As large as fits, never past its own size.
    Fit,
    Scale(f32),
}

const MIN_SCALE: f32 = 0.05;
const MAX_SCALE: f32 = 16.;
/// The largest image file read.
const MAX_FILE: u64 = 256 * 1024 * 1024;
/// The largest picture drawn: decoded, it takes four bytes a pixel, and a
/// GPU texture has at most 16,384 pixels a side.
const MAX_SIDE: u32 = 16_384;
const MAX_PIXELS: u64 = 100_000_000;

/// Whether a `width` × `height` picture can be drawn.
pub(crate) fn fits(width: u32, height: u32) -> bool {
    width <= MAX_SIDE && height <= MAX_SIDE && width as u64 * height as u64 <= MAX_PIXELS
}

struct Loaded {
    image: Arc<Image>,
    /// Natural size in pixels.
    size: (f32, f32),
    /// "PNG · 245 KB".
    info: String,
}

pub struct ImagePreview {
    path: Option<PathBuf>,
    loaded: Option<Loaded>,
    error: Option<SharedString>,
    zoom: Zoom,
    scroll: ScrollHandle,
    _load: Option<Task<()>>,
}

impl ImagePreview {
    /// Shows the image file at `path`, or nothing until given an SVG or a
    /// diagram.
    pub fn new(path: Option<PathBuf>, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            path,
            loaded: None,
            error: None,
            zoom: Zoom::Fit,
            scroll: ScrollHandle::new(),
            _load: None,
        };
        this.reload(cx);
        this
    }

    /// Reads the file again (it changed on disk).
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let read = cx.background_spawn(async move { read_image(&path) });
        self._load = Some(cx.spawn(async move |this, cx| {
            let result = read.await;
            _ = this.update(cx, |this, cx| {
                match result {
                    Ok(loaded) => {
                        this.loaded = Some(loaded);
                        this.error = None;
                    }
                    Err(err) => this.error = Some(err.into()),
                }
                cx.notify();
            });
        }));
    }

    /// Shows an SVG document (the editor's text); a broken one keeps the
    /// last good drawing, with the reason.
    pub fn set_svg(&mut self, text: Arc<str>, cx: &mut Context<Self>) {
        match svg_size(&text) {
            _ if links_other_computers(&text) => {
                self.loaded = None;
                self.error = Some(
                    "This drawing links to files on another computer, so it is not drawn.".into(),
                );
            }
            Some((width, height)) if !fits(width as u32, height as u32) => {
                self.loaded = None;
                self.error = Some("This drawing is too large to show here.".into());
            }
            Some(size) => {
                self.loaded = Some(Loaded {
                    image: Arc::new(Image::from_bytes(
                        ImageFormat::Svg,
                        text.as_bytes().to_vec(),
                    )),
                    size,
                    info: format!("SVG · {}", byte_size(text.len() as u64)),
                });
                self.error = None;
            }
            None => self.error = Some("Not an SVG drawing yet.".into()),
        }
        cx.notify();
    }

    pub fn show_diagram(&mut self, diagram: Diagram, cx: &mut Context<Self>) {
        self.loaded = Some(Loaded {
            image: diagram.image,
            size: (diagram.width, diagram.height),
            info: "Mermaid diagram".into(),
        });
        self.error = None;
        cx.notify();
    }

    fn scale(&self, view: (f32, f32)) -> f32 {
        match (self.zoom, &self.loaded) {
            (Zoom::Scale(scale), _) => scale,
            (Zoom::Fit, Some(loaded)) => {
                let (width, height) = loaded.size;
                (view.0 / width).min(view.1 / height).min(1.)
            }
            (Zoom::Fit, None) => 1.,
        }
    }

    fn zoom_by(&mut self, factor: f32, cx: &mut Context<Self>) {
        let view = self.view_size();
        let scale = (self.scale(view) * factor).clamp(MIN_SCALE, MAX_SCALE);
        self.zoom = Zoom::Scale(scale);
        cx.notify();
    }

    /// The scroll area's size, less its padding.
    fn view_size(&self) -> (f32, f32) {
        let bounds = self.scroll.bounds();
        let padding = 2. * PADDING;
        (
            (f32::from(bounds.size.width) - padding).max(1.),
            (f32::from(bounds.size.height) - padding).max(1.),
        )
    }

    fn on_wheel(&mut self, event: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !event.modifiers.secondary() {
            return;
        }
        let delta = f32::from(event.delta.pixel_delta(window.line_height()).y);
        if delta != 0. {
            self.zoom_by(if delta > 0. { 1.1 } else { 1. / 1.1 }, cx);
            cx.stop_propagation();
        }
    }
}

const PADDING: f32 = 24.;

/// Whether an SVG document links a file on another computer (an `<image>`
/// or `<use>` pointing at `\\server\share`, or written with character
/// references): drawing it would read that file, and on Windows reading one
/// sends the user's sign-in there.
fn links_other_computers(svg: &str) -> bool {
    svg.match_indices("href").any(|(at, _)| {
        let rest = svg[at + 4..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            return false;
        };
        let rest = rest.trim_start();
        let value = match rest.chars().next() {
            Some(quote @ ('"' | '\'')) => rest[1..].split(quote).next().unwrap_or(""),
            _ => return false,
        };
        let value = value.trim();
        let value = value.strip_prefix("file:").unwrap_or(value);
        ph_ui::platform::is_network_path(value) || value.to_ascii_lowercase().starts_with("&#")
    })
}

fn read_image(path: &std::path::Path) -> Result<Loaded, String> {
    let too_large = "This image is too large to show here (try Open in Default App).";
    let length = std::fs::metadata(path)
        .map_err(|err| format!("Could not read the image: {err}"))?
        .len();
    if length > MAX_FILE {
        return Err(too_large.into());
    }
    let bytes = std::fs::read(path).map_err(|err| format!("Could not read the image: {err}"))?;
    let format = ::image::guess_format(&bytes)
        .map_err(|_| "This image format cannot be shown.".to_string())?;
    let (gpui_format, label) = match format {
        ::image::ImageFormat::Png => (ImageFormat::Png, "PNG"),
        ::image::ImageFormat::Jpeg => (ImageFormat::Jpeg, "JPEG"),
        ::image::ImageFormat::Gif => (ImageFormat::Gif, "GIF"),
        ::image::ImageFormat::WebP => (ImageFormat::Webp, "WebP"),
        ::image::ImageFormat::Bmp => (ImageFormat::Bmp, "BMP"),
        ::image::ImageFormat::Tiff => (ImageFormat::Tiff, "TIFF"),
        ::image::ImageFormat::Ico => (ImageFormat::Ico, "ICO"),
        ::image::ImageFormat::Pnm => (ImageFormat::Pnm, "PNM"),
        _ => return Err("This image format cannot be shown.".into()),
    };
    let (width, height) = ::image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .ok()
        .and_then(|reader| reader.into_dimensions().ok())
        .ok_or("The image could not be read.")?;
    if !fits(width, height) {
        return Err(format!("{too_large} It is {width} × {height} pixels."));
    }
    let size = (width as f32, height as f32);
    let info = format!("{label} · {}", byte_size(bytes.len() as u64));
    Ok(Loaded {
        image: Arc::new(Image::from_bytes(gpui_format, bytes)),
        size,
        info,
    })
}

impl Render for ImagePreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let Some(loaded) = &self.loaded else {
            let text = self.error.clone().unwrap_or_else(|| "Loading…".into());
            return crate::notice(text, cx);
        };
        // Fitting needs the pane's size, known once it has been laid out.
        if self.zoom == Zoom::Fit && self.scroll.bounds().size.width <= px(0.) {
            cx.on_next_frame(window, |_, _, cx| cx.notify());
        }
        let scale = self.scale(self.view_size());
        let (width, height) = (loaded.size.0 * scale, loaded.size.1 * scale);
        let percent = format!("{:.0}%", scale * 100.);
        let info = format!(
            "{} × {} · {}",
            loaded.size.0.round(),
            loaded.size.1.round(),
            loaded.info
        );
        let image = img(loaded.image.clone())
            .w(px(width))
            .h(px(height))
            .object_fit(ObjectFit::Contain);
        // A frame keeps transparent parts readable on either theme.
        let frame = div()
            .flex_none()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border.opacity(0.6))
            .bg(theme.secondary.opacity(0.5))
            .overflow_hidden()
            .child(image);
        div()
            .relative()
            .size_full()
            .bg(theme.background)
            .child(
                div()
                    .id("image-scroll")
                    .size_full()
                    .overflow_scroll()
                    .track_scroll(&self.scroll)
                    .on_scroll_wheel(cx.listener(Self::on_wheel))
                    .child(
                        // Centered while smaller than the pane, scrollable
                        // once larger.
                        div()
                            .min_w_full()
                            .min_h_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .p(px(PADDING))
                            .child(frame),
                    ),
            )
            .child(
                h_flex()
                    .absolute()
                    .bottom_2()
                    .left_2()
                    .right_2()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(info),
                    )
                    .when_some(self.error.clone(), |this, error| {
                        this.child(div().text_xs().text_color(theme.danger).child(error))
                    })
                    .child(
                        h_flex()
                            .flex_none()
                            .gap_0p5()
                            .p_0p5()
                            .rounded(theme.radius)
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.popover)
                            .shadow_sm()
                            .child(
                                Button::new("image-zoom-out")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Minus)
                                    .tooltip("Zoom out (⌘ and scroll)")
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.zoom_by(1. / 1.25, cx)),
                                    ),
                            )
                            .child(
                                Button::new("image-actual-size")
                                    .ghost()
                                    .xsmall()
                                    .label(percent)
                                    .tooltip("Actual size")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.zoom = Zoom::Scale(1.);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("image-zoom-in")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Plus)
                                    .tooltip("Zoom in (⌘ and scroll)")
                                    .on_click(cx.listener(|this, _, _, cx| this.zoom_by(1.25, cx))),
                            )
                            .child(
                                Button::new("image-fit")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Minimize)
                                    .tooltip("Fit to the pane")
                                    .selected(self.zoom == Zoom::Fit)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.zoom = Zoom::Fit;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_well() {
        assert_eq!(byte_size(900), "900 bytes");
        assert_eq!(byte_size(1536), "1.5 KB");
        assert_eq!(byte_size(250 * 1024), "250 KB");
        assert_eq!(byte_size(3 * 1024 * 1024), "3.0 MB");
    }
}

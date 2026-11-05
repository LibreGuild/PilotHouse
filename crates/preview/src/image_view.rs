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
    App, AppContext as _, Context, Image, ImageFormat, ImageSource, InteractiveElement as _,
    IntoElement, ObjectFit, ParentElement as _, Render, RenderImage, ScrollEvent, ScrollHandle,
    SharedString, StatefulInteractiveElement as _, Styled as _, StyledImage as _, Task, Window,
    div, img, px,
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

/// A picture as it is drawn.
#[derive(Clone)]
enum Picture {
    /// The file's bytes, decoded by GPUI.
    Encoded(Arc<Image>),
    /// Pixels decoded here, smaller than the file (a very large picture).
    Drawn(Arc<RenderImage>),
}

impl Picture {
    fn source(&self) -> ImageSource {
        match self {
            Picture::Encoded(image) => ImageSource::Image(image.clone()),
            Picture::Drawn(image) => ImageSource::Render(image.clone()),
        }
    }

    fn same(&self, other: &Picture) -> bool {
        match (self, other) {
            // The same bytes are the same picture to GPUI (one cache entry).
            (Picture::Encoded(a), Picture::Encoded(b)) => a.id() == b.id(),
            (Picture::Drawn(a), Picture::Drawn(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }

    /// Lets go of its pixels and textures.
    fn release(self, cx: &mut App) {
        match self {
            Picture::Encoded(image) => ph_ui::images::release_image(image, cx),
            Picture::Drawn(image) => ph_ui::images::release_render_image(image, cx),
        }
    }
}

/// Pictures larger than this many pixels are drawn at this size at most:
/// a 50-megapixel photo takes 200 MB decoded, and as much again on the GPU,
/// to be shown a pane wide.
const DRAWN_SIDE: u32 = 4096;

struct Loaded {
    image: Picture,
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
        // Closed: its picture leaves GPUI's cache too.
        cx.on_release(|this, cx| this.set_loaded_on_release(cx))
            .detach();
        this
    }

    fn set_loaded_on_release(&mut self, cx: &mut App) {
        if let Some(old) = self.loaded.take()
            && old.info != "Mermaid diagram"
        {
            old.image.release(cx);
        }
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
                        this.set_loaded(Some(loaded), cx);
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
            _ if ph_ui::images_safe::links_files(&text) => {
                self.set_loaded(None, cx);
                self.error =
                    Some("This drawing links to other files, so it is not drawn here.".into());
            }
            Some((width, height)) if !fits(width as u32, height as u32) => {
                self.set_loaded(None, cx);
                self.error = Some("This drawing is too large to show here.".into());
            }
            Some(size) => {
                self.set_loaded(
                    Some(Loaded {
                        image: Picture::Encoded(Arc::new(Image::from_bytes(
                            ImageFormat::Svg,
                            text.as_bytes().to_vec(),
                        ))),
                        size,
                        info: format!("SVG · {}", byte_size(text.len() as u64)),
                    }),
                    cx,
                );
                self.error = None;
            }
            None => self.error = Some("Not an SVG drawing yet.".into()),
        }
        cx.notify();
    }

    pub fn show_diagram(&mut self, diagram: Diagram, cx: &mut Context<Self>) {
        self.set_loaded(
            Some(Loaded {
                image: Picture::Encoded(diagram.image),
                size: (diagram.width, diagram.height),
                info: "Mermaid diagram".into(),
            }),
            cx,
        );
        self.error = None;
        cx.notify();
    }

    /// Shows `loaded`; the picture shown before leaves GPUI's cache of
    /// decoded images (it would stay there for good: an SVG edited for an
    /// hour, or a screenshot an agent keeps writing, would fill memory).
    /// A diagram stays: its drawings are cached and shared.
    fn set_loaded(&mut self, loaded: Option<Loaded>, cx: &mut Context<Self>) {
        if let Some(old) = std::mem::replace(&mut self.loaded, loaded)
            && self
                .loaded
                .as_ref()
                .is_none_or(|new| !new.image.same(&old.image))
            && old.info != "Mermaid diagram"
        {
            old.image.release(cx);
        }
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

    fn on_wheel(&mut self, event: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !event.modifiers.secondary(cx) {
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
    // A photo taken turned says so (EXIF); it is shown upright, so its
    // sides swap.
    let orientation = ::image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .ok()
        .and_then(|reader| reader.into_decoder().ok())
        .and_then(|mut decoder| {
            use ::image::ImageDecoder as _;
            decoder.orientation().ok()
        })
        .unwrap_or(::image::metadata::Orientation::NoTransforms);
    let turned = matches!(
        orientation,
        ::image::metadata::Orientation::Rotate90
            | ::image::metadata::Orientation::Rotate270
            | ::image::metadata::Orientation::Rotate90FlipH
            | ::image::metadata::Orientation::Rotate270FlipH
    );
    let size = if turned {
        (height as f32, width as f32)
    } else {
        (width as f32, height as f32)
    };
    let info = format!("{label} · {}", byte_size(bytes.len() as u64));
    // A very large still picture is drawn smaller (an animated GIF keeps
    // its frames as they are).
    if width.max(height) > DRAWN_SIDE
        && format != ::image::ImageFormat::Gif
        && let Some(drawn) = draw_smaller(&bytes, format, orientation)
    {
        return Ok(Loaded {
            image: Picture::Drawn(drawn),
            size,
            info,
        });
    }
    Ok(Loaded {
        image: Picture::Encoded(Arc::new(Image::from_bytes(gpui_format, bytes))),
        size,
        info,
    })
}

/// `bytes` decoded and scaled to fit [`DRAWN_SIDE`], as GPUI draws pixels
/// (BGRA).
fn draw_smaller(
    bytes: &[u8],
    format: ::image::ImageFormat,
    orientation: ::image::metadata::Orientation,
) -> Option<Arc<RenderImage>> {
    let mut decoded = ::image::load_from_memory_with_format(bytes, format).ok()?;
    // Upright, as GPUI shows the smaller ones.
    decoded.apply_orientation(orientation);
    let decoded = decoded.into_rgba8();
    let (width, height) = decoded.dimensions();
    let scale = DRAWN_SIDE as f32 / width.max(height) as f32;
    let (width, height) = (
        ((width as f32 * scale).round() as u32).max(1),
        ((height as f32 * scale).round() as u32).max(1),
    );
    // A box filter on whole pixels: no floating-point copy of the picture
    // (which would take four times its decoded size).
    let mut pixels = ::image::imageops::thumbnail(&decoded, width, height);
    drop(decoded);
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new(vec![::image::Frame::new(
        pixels,
    )])))
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
            // Weak: a closed tab is not kept until the next frame.
            let this = cx.weak_entity();
            window.on_next_frame(move |_, cx| {
                _ = this.update(cx, |_, cx| cx.notify());
            });
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
        let image = img(loaded.image.source())
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
                    .on_scroll(cx.listener(Self::on_wheel))
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

    #[test]
    fn drawings_that_link_files_are_not_drawn() {
        assert!(ph_ui::images_safe::links_files(
            r#"<svg><image href="/dev/zero"/></svg>"#
        ));
        assert!(ph_ui::images_safe::links_files(
            r#"<svg><image xlink:href='\\server\s\a.png'/></svg>"#
        ));
        assert!(ph_ui::images_safe::links_files(
            r#"<svg><image href="&#47;etc"/></svg>"#
        ));
        assert!(!ph_ui::images_safe::links_files(
            r##"<svg><use href="#dot"/><image href="data:image/png;base64,AA"/></svg>"##
        ));
        assert!(!ph_ui::images_safe::links_files(
            r#"<svg><a href="https://example.com">x</a></svg>"#
        ));
    }
}

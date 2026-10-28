//! PDF documents: every page in a column, drawn as it scrolls into view,
//! fitted to the pane's width or zoomed. Pages are drawn by a pure-Rust
//! renderer, so nothing outside the app is needed; text cannot be selected
//! (Open in Default App for that).

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::component::{
    ActiveTheme as _, IconName, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
};
use gpui_kit::{
    AppContext as _, Context, ImageSource, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, RenderImage, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, StyledImage as _, Task, Window, div, img, px,
};

use crate::byte_size;

/// Space between pages and around the column.
const GAP: f32 = 16.;
/// Pages drawn at once, and kept drawn.
const RENDERING: usize = 2;
const KEEP: usize = 12;
/// Drawn pages kept use at most this much memory (four bytes a pixel).
const KEEP_BYTES: u64 = 256 * 1024 * 1024;
/// A page is drawn at most this many pixels a side and in all (past it,
/// zooming in scales the drawing up): a whole page is drawn, so far in, its
/// pixels would be hundreds of megabytes for the part on screen.
const MAX_SIDE: f32 = 8192.;
const MAX_PIXELS: f32 = 16_000_000.;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Zoom {
    FitWidth,
    Scale(f32),
}

struct Document {
    /// Parsed once; pages are drawn from it on other threads.
    pdf: Arc<hayro::hayro_syntax::Pdf>,
    /// The file's size.
    length: usize,
    /// Each page's size in points.
    sizes: Vec<(f32, f32)>,
}

/// Pixels per point for drawing a `size` page at `pixels`, within
/// [`MAX_SIDE`] and [`MAX_PIXELS`].
fn page_scale(size: (f32, f32), pixels: f32) -> f32 {
    let (width, height) = (size.0.max(1.), size.1.max(1.));
    pixels
        .min(MAX_SIDE / width)
        .min(MAX_SIDE / height)
        .min((MAX_PIXELS / (width * height)).sqrt())
}

pub struct PdfView {
    path: PathBuf,
    document: Option<Result<Document, SharedString>>,
    zoom: Zoom,
    /// Drawn pages, by page and pixel width.
    drawn: HashMap<usize, (u32, Arc<RenderImage>)>,
    drawing: HashSet<usize>,
    scroll: ScrollHandle,
    _load: Option<Task<()>>,
    _draw: Vec<Task<()>>,
}

impl PdfView {
    pub fn new(path: PathBuf, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            path,
            document: None,
            zoom: Zoom::FitWidth,
            drawn: HashMap::new(),
            drawing: HashSet::new(),
            scroll: ScrollHandle::new(),
            _load: None,
            _draw: Vec::new(),
        };
        this.reload(cx);
        // Closing the tab lets the drawn pages' textures go.
        cx.on_release(|this, cx| {
            for (_, (_, image)) in this.drawn.drain() {
                cx.drop_image(image, None);
            }
        })
        .detach();
        this
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let path = self.path.clone();
        let read = cx.background_spawn(async move { open(&path) });
        self._load = Some(cx.spawn(async move |this, cx| {
            let result = read.await;
            _ = this.update(cx, |this, cx| {
                this.document = Some(result.map_err(Into::into));
                this.forget_drawn(cx);
                cx.notify();
            });
        }));
    }

    /// Lets go of every drawn page (and its GPU texture).
    fn forget_drawn(&mut self, cx: &mut Context<Self>) {
        for (_, (_, image)) in self.drawn.drain() {
            cx.drop_image(image, None);
        }
        self.drawing.clear();
        self._draw.clear();
    }

    /// Points to pixels on screen for the pane width `width`.
    fn scale(&self, width: f32, sizes: &[(f32, f32)]) -> f32 {
        match self.zoom {
            Zoom::Scale(scale) => scale,
            Zoom::FitWidth => {
                let widest = sizes.iter().map(|size| size.0).fold(1., f32::max);
                ((width - 2. * GAP) / widest).clamp(0.1, 8.)
            }
        }
    }

    fn zoom_by(&mut self, factor: f32, cx: &mut Context<Self>) {
        let Some(Ok(document)) = &self.document else {
            return;
        };
        let width = f32::from(self.scroll.bounds().size.width);
        let scale = (self.scale(width, &document.sizes) * factor).clamp(0.1, 8.);
        self.set_zoom(Zoom::Scale(scale), cx);
    }

    /// Zooms to `zoom`, keeping the same place of the document in view.
    fn set_zoom(&mut self, zoom: Zoom, cx: &mut Context<Self>) {
        let Some(Ok(document)) = &self.document else {
            return;
        };
        let width = f32::from(self.scroll.bounds().size.width).max(200.);
        let before = self.scale(width, &document.sizes);
        self.zoom = zoom;
        let ratio = self.scale(width, &document.sizes) / before;
        let offset = self.scroll.offset();
        self.scroll
            .set_offset(gpui_kit::point(offset.x * ratio, offset.y * ratio));
        cx.notify();
    }

    /// Starts drawing the pages in `pages` that are not drawn at `pixels`
    /// pixels per point yet.
    fn draw(&mut self, pages: Range<usize>, pixels: f32, cx: &mut Context<Self>) {
        let Some(Ok(document)) = &self.document else {
            return;
        };
        for page in pages {
            let pixels = page_scale(document.sizes[page], pixels);
            let width = (document.sizes[page].0 * pixels).round() as u32;
            let drawn_ok = self
                .drawn
                .get(&page)
                .is_some_and(|(drawn_width, _)| drawn_width.abs_diff(width) <= 2);
            if drawn_ok || self.drawing.contains(&page) || self.drawing.len() >= RENDERING {
                continue;
            }
            self.drawing.insert(page);
            let pdf = document.pdf.clone();
            let draw = cx.background_spawn(async move { draw_page(&pdf, page, pixels) });
            self._draw.push(cx.spawn(async move |this, cx| {
                let drawn = draw.await;
                _ = this.update(cx, |this, cx| {
                    this.drawing.remove(&page);
                    if let Some(image) = drawn {
                        if let Some((_, old)) = this.drawn.insert(page, (width, image)) {
                            cx.drop_image(old, None);
                        }
                        this.keep_near(page, cx);
                    }
                    cx.notify();
                });
            }));
        }
        self._draw.retain(|task| !task.is_ready());
    }

    /// Lets go of drawn pages far from `page`, keeping at most [`KEEP`] of
    /// them and [`KEEP_BYTES`] of pixels (zoomed in, fewer pages fit).
    fn keep_near(&mut self, page: usize, cx: &mut Context<Self>) {
        let bytes = |image: &RenderImage| {
            let size = image.size(0);
            size.width.0 as u64 * size.height.0 as u64 * 4
        };
        let mut total: u64 = self.drawn.values().map(|(_, image)| bytes(image)).sum();
        let mut far: Vec<usize> = self.drawn.keys().copied().collect();
        far.sort_by_key(|other| std::cmp::Reverse(other.abs_diff(page)));
        for other in far {
            if other == page || (self.drawn.len() <= KEEP && total <= KEEP_BYTES) {
                break;
            }
            if let Some((_, image)) = self.drawn.remove(&other) {
                total -= bytes(&image);
                cx.drop_image(image, None);
            }
        }
    }
}

fn open(path: &std::path::Path) -> Result<Document, String> {
    let bytes = std::fs::read(path).map_err(|err| err.to_string())?;
    let length = bytes.len();
    let pdf = hayro::hayro_syntax::Pdf::new(bytes)
        .map_err(|err| format!("Not a readable PDF ({err:?}); it may be encrypted."))?;
    let sizes: Vec<(f32, f32)> = pdf
        .pages()
        .iter()
        .map(|page| page.render_dimensions())
        .collect();
    if sizes.is_empty() {
        return Err("This PDF has no pages.".into());
    }
    Ok(Document {
        pdf: Arc::new(pdf),
        length,
        sizes,
    })
}

/// Page `page` drawn at `pixels` pixels per point, on white.
pub fn draw_page(
    pdf: &hayro::hayro_syntax::Pdf,
    page: usize,
    pixels: f32,
) -> Option<Arc<RenderImage>> {
    let pages = pdf.pages();
    let page = pages.get(page)?;
    let cache = hayro::RenderCache::new();
    let pixmap = hayro::render(
        page,
        &cache,
        &hayro::hayro_interpret::InterpreterSettings::default(),
        &hayro::RenderSettings::default(),
        &hayro::PixmapSettings {
            x_scale: pixels,
            y_scale: pixels,
            bg_color: hayro::vello_cpu::color::palette::css::WHITE,
        },
    );
    let (width, height) = (pixmap.width() as u32, pixmap.height() as u32);
    if width == 0 || height == 0 {
        return None;
    }
    // Opaque pixels (white behind), so swapping red and blue gives BGRA.
    // The renderer's own buffer, not a copy: a page can be 64 MB.
    let mut pixels =
        pixmap.take_rgba8(hayro::vello_cpu::peniko::ImageAlphaType::AlphaPremultiplied);
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    let buffer = ::image::RgbaImage::from_raw(width, height, pixels)?;
    Some(Arc::new(RenderImage::new(vec![::image::Frame::new(
        buffer,
    )])))
}

impl Render for PdfView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (sizes, length) = match &self.document {
            None => return crate::notice("Opening the PDF…", cx),
            Some(Err(err)) => return crate::notice(err.clone(), cx),
            Some(Ok(document)) => (document.sizes.clone(), document.length),
        };
        let pane = self.scroll.bounds();
        let scale = self.scale(f32::from(pane.size.width).max(200.), &sizes);
        // Which pages are in view (and one either side).
        let top = -f32::from(self.scroll.offset().y);
        let bottom = top + f32::from(pane.size.height);
        let mut y = GAP;
        let mut first = None;
        let mut last = 0;
        for (ix, (_, height)) in sizes.iter().enumerate() {
            let page_bottom = y + height * scale;
            if page_bottom >= top && y <= bottom {
                first.get_or_insert(ix);
                last = ix + 1;
            }
            y = page_bottom + GAP;
        }
        let visible = match first {
            Some(first) => first..last,
            None => 0..1.min(sizes.len()),
        };
        let wanted = visible.start.saturating_sub(1)..(visible.end + 1).min(sizes.len());
        self.draw(wanted, scale * window.scale_factor(), cx);
        let current = visible.start + 1;
        let theme = cx.theme();

        let pages = sizes.iter().enumerate().map(|(ix, (width, height))| {
            let (width, height) = (width * scale, height * scale);
            div()
                .flex_none()
                .w(px(width))
                .h(px(height))
                .bg(gpui_kit::white())
                .shadow_md()
                .rounded(px(2.))
                .overflow_hidden()
                .children(self.drawn.get(&ix).map(|(_, image)| {
                    img(ImageSource::Render(image.clone()))
                        .size_full()
                        .object_fit(gpui_kit::ObjectFit::Fill)
                }))
        });
        let label = match self.zoom {
            Zoom::FitWidth => "Fit width".to_string(),
            Zoom::Scale(scale) => format!("{:.0}%", scale * 100.),
        };
        div()
            .relative()
            .size_full()
            .bg(theme.secondary.opacity(0.6))
            .child(
                div()
                    .id("pdf-pages")
                    .size_full()
                    .overflow_scroll()
                    .track_scroll(&self.scroll)
                    .child(
                        div()
                            .min_w_full()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(GAP))
                            .p(px(GAP))
                            .children(pages),
                    ),
            )
            .child(
                h_flex()
                    .absolute()
                    .bottom_2()
                    .right_3()
                    .gap_0p5()
                    .p_0p5()
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.popover)
                    .shadow_sm()
                    .text_xs()
                    .child(
                        div()
                            .px_2()
                            .text_color(theme.muted_foreground)
                            .child(format!(
                                "Page {current} of {} · {}",
                                sizes.len(),
                                byte_size(length as u64)
                            )),
                    )
                    .child(
                        Button::new("pdf-zoom-out")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Minus)
                            .tooltip("Zoom out")
                            .on_click(cx.listener(|this, _, _, cx| this.zoom_by(1. / 1.2, cx))),
                    )
                    .child(
                        Button::new("pdf-fit")
                            .ghost()
                            .xsmall()
                            .label(label)
                            .selected(self.zoom == Zoom::FitWidth)
                            .tooltip("Fit the pages to the pane's width")
                            .on_click(
                                cx.listener(|this, _, _, cx| this.set_zoom(Zoom::FitWidth, cx)),
                            ),
                    )
                    .child(
                        Button::new("pdf-zoom-in")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Plus)
                            .tooltip("Zoom in")
                            .on_click(cx.listener(|this, _, _, cx| this.zoom_by(1.2, cx))),
                    ),
            )
            .into_any_element()
    }
}

impl Drop for PdfView {
    fn drop(&mut self) {
        self._draw.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-page PDF with a filled square, written by hand.
    fn tiny_pdf() -> Vec<u8> {
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R >>".to_string(),
            {
                let content = "1 0 0 rg 10 10 50 50 re f";
                format!(
                    "<< /Length {} >>\nstream\n{content}\nendstream",
                    content.len()
                )
            },
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (ix, object) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", ix + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
        );
        for offset in offsets {
            out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        out
    }

    #[test]
    fn opens_and_draws_pages() {
        let path = std::env::temp_dir().join(format!("ph-pdf-{}.pdf", std::process::id()));
        std::fs::write(&path, tiny_pdf()).unwrap();
        let document = open(&path).unwrap();
        assert_eq!(document.sizes, [(200., 100.)]);
        let image = draw_page(&document.pdf, 0, 2.).unwrap();
        let size = image.size(0);
        assert_eq!((size.width.0, size.height.0), (400, 200));
        assert!(draw_page(&document.pdf, 5, 1.).is_none());
        // Zoomed far in, a page is drawn no larger than MAX_SIDE a side.
        assert_eq!(page_scale((612., 792.), 2.), 2.);
        let scale = page_scale((612., 792.), 32.);
        assert!(612. * 792. * scale * scale <= MAX_PIXELS + 1.);
        assert!(792. * page_scale((100., 5000.), 32.) <= MAX_SIDE * 792. / 5000. + 0.01);
        std::fs::write(&path, b"not a pdf").unwrap();
        assert!(open(&path).is_err());
    }
}

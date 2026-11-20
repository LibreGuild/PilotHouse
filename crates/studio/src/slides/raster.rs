//! Slides as pixels: the SVG of a slide drawn by resvg, with the fonts
//! the text was laid out with.

use std::path::Path;

use anyhow::{Context as _, Result};
use image::RgbaImage;
use resvg::{tiny_skia, usvg};

use super::fonts::fonts;
use super::model::Deck;
use super::svg::{self, Options};

/// The pictures a slide's SVG may load: its own media files (named as
/// PilotHouse names them, in a `media` folder or its small copies), never
/// anything else an SVG names (a device such as /dev/zero would be read
/// forever, a network path would leak the user's credentials).
pub fn image_resolver() -> usvg::ImageHrefResolver<'static> {
    let load = usvg::ImageHrefResolver::default_string_resolver();
    usvg::ImageHrefResolver {
        resolve_data: usvg::ImageHrefResolver::default_data_resolver(),
        resolve_string: Box::new(move |href, options| {
            media_href(href).then(|| load(href, options)).flatten()
        }),
    }
}

/// Whether `href` names one of a deck's media files (see
/// [`image_resolver`]), a regular file of a sane size.
pub fn media_href(href: &str) -> bool {
    let path = std::path::Path::new(href);
    let named = || -> Option<bool> {
        let name = path.file_name()?.to_str()?;
        let folder = path.parent()?;
        let in_media = match folder.file_name()?.to_str()? {
            super::space::MEDIA => true,
            super::space::SMALL => folder.parent()?.file_name()? == super::space::MEDIA,
            _ => false,
        };
        Some(in_media && super::space::safe_media_name(name))
    };
    named() == Some(true)
        && std::fs::metadata(path).is_ok_and(|file| file.is_file() && file.len() <= 512 << 20)
        && decodable(path)
}

/// Whether the picture in `path` can be drawn safely. A bitmap is decoded
/// whole, at the size its header says, so a small file that claims to be
/// huge would ask for more memory than there is: at most
/// [`super::space::MOST_PIXELS`]. Any other file is taken for an SVG, which
/// must not nest too deeply (see [`too_deep`]).
fn decodable(path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    // By what the file holds, as the drawing tells a bitmap from an SVG.
    let Ok(reader) = image::ImageReader::new(std::io::BufReader::new(file)).with_guessed_format()
    else {
        return false;
    };
    if reader.format().is_none() {
        // An SVG, as text: packed with gzip its nesting cannot be counted.
        return std::fs::read(path)
            .is_ok_and(|svg| !svg.starts_with(&[0x1f, 0x8b]) && !too_deep(&svg));
    }
    reader
        .into_dimensions()
        .is_ok_and(|(width, height)| width as u64 * height as u64 <= super::space::MOST_PIXELS)
}

/// How deep an SVG graphic may nest to be drawn: more than drawing
/// programs write. Reading, working out and painting it each call
/// themselves once or more for each level, on a background thread's small
/// stack; using that up ends the app.
const SVG_MOST_DEPTH: usize = 64;

/// Whether SVG `source` nests too deeply to be drawn.
pub(crate) fn too_deep(source: &[u8]) -> bool {
    super::pptx::xml_depth(source) > SVG_MOST_DEPTH
}

/// What `work` gives, or `None` when it panicked (logged as `what`).
/// Reading a file from anywhere and drawing a slide run on background
/// tasks, where a panic ends the whole app on macOS, and both work on what
/// files and decks say; the causes known are checked for, this is for the
/// ones that are not.
pub(crate) fn caught<T>(what: &str, work: impl FnOnce() -> T) -> Option<T> {
    // Nothing such work leaves half done is used again: it reads the deck
    // and the file, and the fonts' lock is taken again whatever happened
    // (see `text::lay_out_at`).
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)) {
        Ok(done) => Some(done),
        Err(panic) => {
            let why = panic
                .downcast_ref::<&str>()
                .map(|why| why.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            tracing::error!("{what} panicked: {why}");
            None
        }
    }
}

/// Options for SVG that is only checked or measured: it loads nothing.
pub fn inert_options() -> usvg::Options<'static> {
    usvg::Options {
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..usvg::Options::default()
    }
}

/// `svg` drawn `width` pixels wide (its height in proportion).
pub fn rasterize(svg: &str, width: u32) -> Result<RgbaImage> {
    caught("drawing a slide", || draw(svg, width))
        .unwrap_or_else(|| Err(anyhow::anyhow!("the slide could not be drawn")))
}

fn draw(svg: &str, width: u32) -> Result<RgbaImage> {
    let options = usvg::Options {
        fontdb: fonts().db.clone(),
        image_href_resolver: image_resolver(),
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_str(svg, &options).context("the slide's drawing")?;
    let size = tree.size();
    let width = width.clamp(1, 8192);
    let scale = width as f32 / size.width().max(1.);
    let height = ((size.height() * scale).round() as u32).clamp(1, 8192);
    let mut pixmap = tiny_skia::Pixmap::new(width, height).context("memory for the slide")?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let mut pixels = pixmap.take();
    // tiny-skia keeps colours premultiplied by alpha.
    for pixel in pixels.as_chunks_mut::<4>().0 {
        let alpha = pixel[3] as u32;
        if alpha > 0 && alpha < 255 {
            for channel in &mut pixel[..3] {
                *channel = ((*channel as u32 * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
    RgbaImage::from_raw(width, height, pixels).context("the slide's pixels")
}

/// Slide `index` of `deck`, `width` pixels wide.
pub fn slide(
    deck: &Deck,
    index: usize,
    width: u32,
    media: Option<&Path>,
    editing: bool,
) -> Result<RgbaImage> {
    let svg = svg::slide(
        deck,
        index,
        &Options {
            media,
            editing,
            hide_text: None,
            small: false,
            hidden: &[],
        },
    );
    rasterize(&svg, width)
}

#[cfg(test)]
pub(crate) mod tests {
    use serde_json::json;

    use super::*;
    use crate::slides::model::SlideSize;
    use crate::slides::ops::{Op, apply, new_deck};

    /// A deck showing every kind of element.
    pub(crate) fn sample(theme: &str) -> Deck {
        let mut deck = new_deck("Quarterly review", theme, SlideSize::default());
        let ops: Vec<serde_json::Value> = vec![
            json!({"op": "add_slide", "title": "Where we are", "body": "Revenue up **18%** on last quarter\n- Two new regions\n  - Berlin and *Lisbon*\n- Churn down to 2.1%\n1. Hire\n1. Ship"}),
            json!({"op": "add_slide", "layout": "title_only", "title": "Growth by region", "elements": [
                {"type": "chart", "chart": "column", "categories": ["Q1", "Q2", "Q3", "Q4"],
                 "series": [{"name": "EMEA", "values": [12, 15, 19, 24]}, {"name": "APAC", "values": [8, 11, 10, 16]}],
                 "labels": true, "frame": {"x": 60, "y": 150, "w": 420, "h": 330}},
                {"type": "chart", "chart": "doughnut", "categories": ["Web", "Mobile", "API"], "series": [{"name": "Share", "values": [50, 30, 20]}],
                 "labels": true, "frame": {"x": 520, "y": 150, "w": 380, "h": 330}}
            ]}),
            json!({"op": "add_slide", "layout": "title_only", "title": "Plan and code", "elements": [
                {"type": "table", "columns": [2, 1, 1], "frame": {"x": 60, "y": 150, "w": 440, "h": 200},
                 "rows": [[{"text": "Task"}, {"text": "Owner"}, {"text": "Due"}], [{"text": "Launch"}, {"text": "Ana"}, {"text": "May"}], [{"text": "Docs"}, {"text": "Raj"}, {"text": "June"}]]},
                {"type": "code", "language": "rs", "code": "fn main() {\n    println!(\"hello\");\n}", "line_numbers": true,
                 "frame": {"x": 530, "y": 150, "w": 370, "h": 140}},
                {"type": "shape", "geometry": "right_arrow", "fill": "accent2", "text": "Next", "frame": {"x": 530, "y": 320, "w": 180, "h": 80},
                 "shadow": {}},
                {"type": "icon", "icon": "rocket", "color": "accent3", "frame": {"x": 740, "y": 320, "w": 80, "h": 80}},
                {"type": "shape", "geometry": "line", "line": {"width": 3, "color": "accent4", "tail": true}, "frame": {"x": 60, "y": 400, "w": 400, "h": 80}},
                {"type": "svg", "source": "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 10 10'><circle cx='5' cy='5' r='4' fill='orange'/></svg>",
                 "frame": {"x": 840, "y": 330, "w": 60, "h": 60}}
            ]}),
            json!({"op": "add_slide", "layout": "statement", "title": "3×", "subtitle": "faster builds"}),
            json!({"op": "add_slide", "layout": "section", "title": "Next steps"}),
        ];
        for op in ops {
            let op: Op = serde_json::from_value(op).unwrap();
            apply(&mut deck, op).unwrap();
        }
        deck
    }

    #[test]
    fn a_panic_while_drawing_is_an_error() {
        assert_eq!(caught("a test", || 2), Some(2));
        assert_eq!(
            caught::<u8>("a test (this panic is meant)", || panic!("meant")),
            None
        );
    }

    /// A PNG of `width` × `height` by its header (its pixels are those of
    /// a small one: nothing reads that far).
    pub(crate) fn png_claiming(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::RgbImage::new(2, 2)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        // The header chunk: its length and name (8 bytes after the 8 of the
        // signature), 13 bytes that start with the size, and their CRC.
        bytes[16..20].copy_from_slice(&width.to_be_bytes());
        bytes[20..24].copy_from_slice(&height.to_be_bytes());
        let mut crc = !0u32;
        for byte in &bytes[12..29] {
            crc ^= *byte as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        bytes[29..33].copy_from_slice(&(!crc).to_be_bytes());
        bytes
    }

    #[test]
    fn a_picture_too_large_to_decode_is_not_loaded() {
        let media = std::env::temp_dir()
            .join(format!("pilothouse-href-{}", std::process::id()))
            .join(crate::slides::space::MEDIA);
        std::fs::create_dir_all(&media).unwrap();
        let file = |name: &str, bytes: &[u8]| {
            let path = media.join(name);
            std::fs::write(&path, bytes).unwrap();
            path.to_string_lossy().into_owned()
        };
        assert!(media_href(&file("fine.png", &png_claiming(4000, 3000))));
        assert!(!media_href(&file(
            "huge.png",
            &png_claiming(50_000, 50_000)
        )));
        // An SVG by how deep it nests.
        let svg = |depth: usize| {
            format!(
                "<svg xmlns='http://www.w3.org/2000/svg'>{}{}</svg>",
                "<g>".repeat(depth),
                "</g>".repeat(depth)
            )
        };
        assert!(media_href(&file("flat.svg", svg(10).as_bytes())));
        assert!(!media_href(&file("deep.svg", svg(5000).as_bytes())));
        let _ = std::fs::remove_dir_all(media.parent().unwrap());
    }

    #[test]
    fn draws_every_kind() {
        let deck = sample("clean");
        for index in 0..deck.slides.len() {
            let image = slide(&deck, index, 480, None, true).unwrap();
            assert_eq!(image.dimensions(), (480, 270));
            // Not all one colour.
            let first = image.get_pixel(0, 0);
            assert!(image.pixels().any(|pixel| pixel != first), "slide {index}");
        }
    }

    /// Writes every theme's sample deck as PNGs for a look:
    /// `SLIDES_PNG=/some/dir cargo test -p ph-studio slides_png -- --ignored`.
    #[test]
    #[ignore]
    fn slides_png() {
        let Ok(dir) = std::env::var("SLIDES_PNG") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (theme, _) in crate::slides::theme::builtins() {
            let deck = sample(theme);
            for index in 0..deck.slides.len() {
                let started = std::time::Instant::now();
                let image = slide(&deck, index, 1920, None, false).unwrap();
                if theme == "clean" {
                    println!("{theme} {index}: {:?}", started.elapsed());
                }
                image
                    .save(dir.join(format!("{theme}-{index}.png")))
                    .unwrap();
            }
        }
    }
}

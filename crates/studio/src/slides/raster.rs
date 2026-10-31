//! Slides as pixels: the SVG of a slide drawn by resvg, with the fonts
//! the text was laid out with.

use std::path::Path;

use anyhow::{Context as _, Result};
use image::RgbaImage;
use resvg::{tiny_skia, usvg};

use super::fonts::fonts;
use super::model::Deck;
use super::svg::{self, Options};

/// `svg` drawn `width` pixels wide (its height in proportion).
pub fn rasterize(svg: &str, width: u32) -> Result<RgbaImage> {
    let options = usvg::Options {
        fontdb: fonts().db.clone(),
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

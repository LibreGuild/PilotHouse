//! Making the result: the original picture through an [`ImageEdit`].
//!
//! The same steps make the preview (from a smaller copy of the original)
//! and the saved file (from the original itself), so what is shown is what
//! is saved. In order: turn, crop, adjust colors, blur or pixelate areas,
//! resize, draw the marks, frame. Marks are drawn after resizing so lines
//! and text are sharp at the result's size; they are drawn as SVG by resvg,
//! the renderer GPUI already carries, so text uses the system's fonts.

use std::borrow::Cow;
use std::fmt::Write as _;
use std::sync::{Arc, OnceLock};

use image::{RgbaImage, imageops};
use resvg::tiny_skia::{self, FillRule, Paint as SkPaint, PathBuilder, Pixmap, Transform};
use resvg::usvg;

use crate::edit::{
    Adjust, Align, Color, Frame, ImageEdit, Mark, MarkKind, Orientation, Paint, Rect,
};

/// How carefully pixels are resampled: quickly for the preview, finely
/// for the saved file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    Preview,
    Full,
}

/// Fonts for text marks, searched once.
fn fonts() -> Arc<usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut fonts = usvg::fontdb::Database::new();
            fonts.load_system_fonts();
            Arc::new(fonts)
        })
        .clone()
}

/// Families tried for text, the first found used: the system's own UI
/// faces, then common ones.
const FONT_FAMILY: &str =
    "'SF Pro Text', 'Helvetica Neue', 'Segoe UI', Inter, Roboto, Arial, 'DejaVu Sans', sans-serif";

/// The result of `edit` on `source`, a copy of an `original`-sized picture
/// (smaller for a preview; the result is smaller by as much).
pub fn render(
    source: &RgbaImage,
    original: (u32, u32),
    edit: &ImageEdit,
    quality: Quality,
) -> RgbaImage {
    let scale = source.width() as f32 / original.0.max(1) as f32;
    let turned = orient(source, edit.orientation);
    let crop = edit.crop_rect(original);
    let (x, y) = (
        (crop.x * scale).round() as u32,
        (crop.y * scale).round() as u32,
    );
    let (x, y) = (
        x.min(turned.width().saturating_sub(1)),
        y.min(turned.height().saturating_sub(1)),
    );
    let width = ((crop.width * scale).round() as u32).clamp(1, turned.width() - x);
    let height = ((crop.height * scale).round() as u32).clamp(1, turned.height() - y);
    let mut image = if (x, y, width, height) == (0, 0, turned.width(), turned.height()) {
        turned.into_owned()
    } else {
        imageops::crop_imm(turned.as_ref(), x, y, width, height).to_image()
    };

    adjust(&mut image, &edit.adjust, scale);

    // Hiding comes before resizing, at the picture's own pixels.
    let at_crop = |area: Rect, by: f32| {
        Rect::new(
            (area.x - crop.x) * by,
            (area.y - crop.y) * by,
            area.width * by,
            area.height * by,
        )
    };
    for mark in &edit.marks {
        match mark.kind {
            MarkKind::Blur { strength, .. } => {
                let area = at_crop(mark.kind.area().expect("an area mark"), scale);
                blur_area(&mut image, area, strength * scale);
            }
            MarkKind::Pixelate { size, .. } => {
                let area = at_crop(mark.kind.area().expect("an area mark"), scale);
                pixelate(&mut image, area, (size * scale).max(2.));
            }
            _ => {}
        }
    }

    let (out_width, out_height) = edit.output_size(original);
    let target = (
        ((out_width as f32 * scale).round() as u32).max(1),
        ((out_height as f32 * scale).round() as u32).max(1),
    );
    if image.dimensions() != target {
        let filter = match quality {
            Quality::Preview => imageops::FilterType::Triangle,
            Quality::Full => imageops::FilterType::Lanczos3,
        };
        image = imageops::resize(&image, target.0, target.1, filter);
    }

    let (by_x, by_y) = (target.0 as f32 / crop.width, target.1 as f32 / crop.height);
    let drawn: Vec<&Mark> = edit
        .marks
        .iter()
        .filter(|mark| !matches!(mark.kind, MarkKind::Blur { .. } | MarkKind::Pixelate { .. }))
        .collect();
    if !drawn.is_empty() {
        let place = |p: [f32; 2]| [(p[0] - crop.x) * by_x, (p[1] - crop.y) * by_y];
        draw_marks(&mut image, &drawn, place, (by_x + by_y) / 2.);
    }

    match &edit.frame {
        Some(frame) => framed(&image, frame, scale),
        None => image,
    }
}

/// `source` turned as `orientation` says.
pub(crate) fn orient(source: &RgbaImage, orientation: Orientation) -> Cow<'_, RgbaImage> {
    let flipped = if orientation.flipped {
        Cow::Owned(imageops::flip_horizontal(source))
    } else {
        Cow::Borrowed(source)
    };
    match orientation.turns % 4 {
        1 => Cow::Owned(imageops::rotate90(flipped.as_ref())),
        2 => Cow::Owned(imageops::rotate180(flipped.as_ref())),
        3 => Cow::Owned(imageops::rotate270(flipped.as_ref())),
        _ => flipped,
    }
}

/// Color changes, in one pass over the pixels; blur and sharpen after.
fn adjust(image: &mut RgbaImage, adjust: &Adjust, scale: f32) {
    if adjust.is_identity() {
        return;
    }
    let brightness = adjust.brightness / 100. * 0.5;
    let contrast = 1. + adjust.contrast / 100.;
    let saturation = if adjust.grayscale {
        0.
    } else {
        1. + adjust.saturation / 100.
    };
    let lut: [f32; 256] = std::array::from_fn(|v| {
        ((v as f32 / 255. - 0.5) * contrast + 0.5 + brightness).clamp(0., 1.) * 255.
    });
    let tone = brightness != 0. || contrast != 1.;
    if tone || saturation != 1. || adjust.invert {
        for pixel in image.pixels_mut() {
            let [mut r, mut g, mut b, _] = pixel.0.map(f32::from);
            if tone {
                (r, g, b) = (lut[r as usize], lut[g as usize], lut[b as usize]);
            }
            if saturation != 1. {
                let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
                r = luma + (r - luma) * saturation;
                g = luma + (g - luma) * saturation;
                b = luma + (b - luma) * saturation;
            }
            if adjust.invert {
                (r, g, b) = (255. - r, 255. - g, 255. - b);
            }
            let channel = |value: f32| value.round().clamp(0., 255.) as u8;
            pixel.0 = [channel(r), channel(g), channel(b), pixel.0[3]];
        }
    }
    if adjust.blur > 0. {
        box_blur(image, adjust.blur * scale);
    }
    if adjust.sharpen > 0. {
        let mut soft = image.clone();
        box_blur(&mut soft, (1.5 * scale).max(1.));
        let amount = adjust.sharpen / 100. * 1.5;
        for (pixel, soft) in image.pixels_mut().zip(soft.pixels()) {
            for channel in 0..3 {
                let value = pixel.0[channel] as f32;
                let sharp = value + amount * (value - soft.0[channel] as f32);
                pixel.0[channel] = sharp.round().clamp(0., 255.) as u8;
            }
        }
    }
}

/// A blur close to a Gaussian of `radius`, in time independent of it:
/// three passes of a running-sum box blur each way.
pub(crate) fn box_blur(image: &mut RgbaImage, radius: f32) {
    let box_radius = (radius * 0.6).round() as usize;
    if box_radius == 0 {
        return;
    }
    let (width, height) = (image.width() as usize, image.height() as usize);
    let data: &mut [u8] = image.as_mut();
    let mut line = Vec::new();
    for _ in 0..3 {
        for row in 0..height {
            blur_line(data, row * width * 4, 4, width, box_radius, &mut line);
        }
        for column in 0..width {
            blur_line(data, column * 4, width * 4, height, box_radius, &mut line);
        }
    }
}

/// One box blur pass along a line of `count` pixels starting at byte
/// `start`, `step` bytes apart; edges repeat the end pixel.
fn blur_line(
    data: &mut [u8],
    start: usize,
    step: usize,
    count: usize,
    radius: usize,
    line: &mut Vec<[u8; 4]>,
) {
    line.clear();
    line.extend((0..count).map(|ix| {
        let at = start + ix * step;
        [data[at], data[at + 1], data[at + 2], data[at + 3]]
    }));
    let pixel = |ix: isize| line[ix.clamp(0, count as isize - 1) as usize];
    let window = (2 * radius + 1) as u32;
    let mut sum = [0u32; 4];
    for ix in -(radius as isize)..=radius as isize {
        let p = pixel(ix);
        for c in 0..4 {
            sum[c] += p[c] as u32;
        }
    }
    for ix in 0..count {
        let at = start + ix * step;
        for c in 0..4 {
            data[at + c] = ((sum[c] + window / 2) / window) as u8;
        }
        let (out, into) = (
            pixel(ix as isize - radius as isize),
            pixel(ix as isize + radius as isize + 1),
        );
        for c in 0..4 {
            sum[c] = sum[c] + into[c] as u32 - out[c] as u32;
        }
    }
}

/// The pixels of `area` (clamped to the picture), as a sub-rectangle.
fn pixel_area(image: &RgbaImage, area: Rect) -> Option<(u32, u32, u32, u32)> {
    let x0 = area.x.max(0.).floor() as u32;
    let y0 = area.y.max(0.).floor() as u32;
    let x1 = (area.right().ceil().max(0.) as u32).min(image.width());
    let y1 = (area.bottom().ceil().max(0.) as u32).min(image.height());
    (x1 > x0 && y1 > y0).then_some((x0, y0, x1 - x0, y1 - y0))
}

pub(crate) fn blur_area(image: &mut RgbaImage, area: Rect, strength: f32) {
    let Some((x, y, width, height)) = pixel_area(image, area) else {
        return;
    };
    let mut part = imageops::crop_imm(image, x, y, width, height).to_image();
    // Twice over, so no outline of the text underneath survives.
    box_blur(&mut part, strength.max(2.));
    box_blur(&mut part, strength.max(2.));
    imageops::replace(image, &part, x as i64, y as i64);
}

pub(crate) fn pixelate(image: &mut RgbaImage, area: Rect, block: f32) {
    let Some((x, y, width, height)) = pixel_area(image, area) else {
        return;
    };
    let block = block.round().max(2.) as u32;
    for by in (y..y + height).step_by(block as usize) {
        for bx in (x..x + width).step_by(block as usize) {
            let (bw, bh) = (block.min(x + width - bx), block.min(y + height - by));
            let mut sum = [0u64; 4];
            for py in by..by + bh {
                for px in bx..bx + bw {
                    let p = image.get_pixel(px, py).0;
                    for c in 0..4 {
                        sum[c] += p[c] as u64;
                    }
                }
            }
            let count = (bw * bh) as u64;
            let average = sum.map(|total| (total / count) as u8);
            for py in by..by + bh {
                for px in bx..bx + bw {
                    image.put_pixel(px, py, image::Rgba(average));
                }
            }
        }
    }
}

/// Text made safe to put inside SVG.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if c.is_control() && c != '\n' => {}
            c => out.push(c),
        }
    }
    out
}

/// `fill="…" fill-opacity="…"` (or stroke) for a color.
fn paint(attribute: &str, color: Color) -> String {
    let [r, g, b, a] = color.0;
    if a == 255 {
        format!(r##"{attribute}="#{r:02x}{g:02x}{b:02x}""##)
    } else {
        format!(
            r##"{attribute}="#{r:02x}{g:02x}{b:02x}" {attribute}-opacity="{:.3}""##,
            a as f32 / 255.
        )
    }
}

/// White on dark colors, black on light ones.
fn on(color: Color) -> &'static str {
    let [r, g, b, _] = color.0.map(|c| c as f32 / 255.);
    if 0.2126 * r + 0.7152 * g + 0.0722 * b > 0.6 {
        "#000000"
    } else {
        "#ffffff"
    }
}

fn svg_options() -> usvg::Options<'static> {
    usvg::Options {
        fontdb: fonts(),
        font_family: "sans-serif".into(),
        // The SVG is PilotHouse's own and links nothing; refuse anyway.
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..Default::default()
    }
}

/// How a text mark is laid out: its size, weight, and its lines lined up
/// as `align` says within a block `width` wide.
struct TextStyle<'a> {
    size: f32,
    bold: bool,
    fill: &'a str,
    align: Align,
    width: f32,
}

/// The SVG for one text mark's lines, its top left corner `at`.
fn text_svg(out: &mut String, id: &str, [x, y]: [f32; 2], style: &TextStyle, text: &str) {
    let TextStyle {
        size,
        bold,
        fill,
        align,
        width,
    } = *style;
    let weight = if bold { 700 } else { 400 };
    let (anchor, x) = match align {
        Align::Left => ("start", x),
        Align::Center => ("middle", x + width / 2.),
        Align::Right => ("end", x + width),
    };
    let _ = write!(
        out,
        r#"<text id="{id}" x="{x:.2}" y="{:.2}" text-anchor="{anchor}" font-family="{FONT_FAMILY}" font-size="{size:.2}" font-weight="{weight}" {fill} xml:space="preserve">"#,
        y + size * 0.88
    );
    for (ix, line) in text.lines().enumerate() {
        let dy = if ix == 0 { 0. } else { size * 1.25 };
        let _ = write!(
            out,
            r#"<tspan x="{x:.2}" dy="{dy:.2}">{}</tspan>"#,
            escape(line)
        );
    }
    out.push_str("</text>");
}

/// Where each text mark's letters are (by mark id), for the box behind
/// them or lining up its lines: laid out once, left aligned.
fn text_bounds(
    width: u32,
    height: u32,
    marks: &[&Mark],
    place: &dyn Fn([f32; 2]) -> [f32; 2],
    by: f32,
) -> Vec<(u32, Rect)> {
    let mut svg =
        format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}">"#);
    let mut wanted = Vec::new();
    for mark in marks {
        if let MarkKind::Text {
            x,
            y,
            text,
            size,
            bold,
            background,
            align,
            ..
        } = &mark.kind
            && (background.is_some() || *align != Align::Left)
        {
            let [x, y] = place([*x, *y]);
            let style = TextStyle {
                size: size * by,
                bold: *bold,
                fill: r##"fill="#000""##,
                align: Align::Left,
                width: 0.,
            };
            text_svg(&mut svg, &format!("t{}", mark.id), [x, y], &style, text);
            wanted.push(mark.id);
        }
    }
    if wanted.is_empty() {
        return Vec::new();
    }
    svg.push_str("</svg>");
    let Ok(tree) = usvg::Tree::from_str(&svg, &svg_options()) else {
        return Vec::new();
    };
    wanted
        .into_iter()
        .filter_map(|id| {
            let bounds = tree.node_by_id(&format!("t{id}"))?.abs_bounding_box();
            Some((
                id,
                Rect::new(bounds.x(), bounds.y(), bounds.width(), bounds.height()),
            ))
        })
        .collect()
}

/// Draws `marks` over `image`: `place` takes a point of the turned
/// original to the image, and `by` is how much larger sizes get.
fn draw_marks(
    image: &mut RgbaImage,
    marks: &[&Mark],
    place: impl Fn([f32; 2]) -> [f32; 2],
    by: f32,
) {
    let (width, height) = image.dimensions();
    if let Some(layer) = marks_layer(width, height, marks, place, by) {
        blend(image, &layer);
    }
}

/// `marks` (and their shadows) drawn on a see-through `width` × `height`
/// layer, to lay over a picture (or many frames of a video).
pub(crate) fn marks_layer(
    width: u32,
    height: u32,
    marks: &[&Mark],
    place: impl Fn([f32; 2]) -> [f32; 2],
    by: f32,
) -> Option<Pixmap> {
    let boxes = text_bounds(width, height, marks, &place, by);
    let head =
        format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}">"#);
    let mut all = head.clone();
    // The marks a shadow lifts off the picture, drawn again for it.
    let mut lifted = head;
    let mut svg = String::new();
    let area = |rect: Rect| {
        let a = place([rect.x, rect.y]);
        let b = place([rect.right(), rect.bottom()]);
        Rect::from_corners(a, b)
    };
    let mut any_lifted = false;
    for mark in marks {
        let lifts = match &mark.kind {
            MarkKind::Highlight { .. } | MarkKind::Spotlight { .. } => false,
            MarkKind::Text { background, .. } => background.is_none(),
            _ => true,
        };
        svg.clear();
        match &mark.kind {
            MarkKind::Rect {
                color,
                stroke,
                fill,
                radius,
                ..
            } => {
                let r = area(mark.kind.area().expect("an area"));
                let fill =
                    fill.map_or_else(|| r#"fill="none""#.to_string(), |fill| paint("fill", fill));
                let _ = write!(
                    svg,
                    r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" rx="{:.2}" {fill} {} stroke-width="{:.2}" stroke-linejoin="round"/>"#,
                    r.x,
                    r.y,
                    r.width,
                    r.height,
                    radius * by,
                    paint("stroke", *color),
                    stroke * by
                );
            }
            MarkKind::Ellipse {
                color,
                stroke,
                fill,
                ..
            } => {
                let r = area(mark.kind.area().expect("an area"));
                let fill =
                    fill.map_or_else(|| r#"fill="none""#.to_string(), |fill| paint("fill", fill));
                let _ = write!(
                    svg,
                    r#"<ellipse cx="{:.2}" cy="{:.2}" rx="{:.2}" ry="{:.2}" {fill} {} stroke-width="{:.2}"/>"#,
                    r.x + r.width / 2.,
                    r.y + r.height / 2.,
                    r.width / 2.,
                    r.height / 2.,
                    paint("stroke", *color),
                    stroke * by
                );
            }
            MarkKind::Line {
                from,
                to,
                color,
                stroke,
            } => {
                let (a, b) = (place(*from), place(*to));
                let _ = write!(
                    svg,
                    r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" {} stroke-width="{:.2}" stroke-linecap="round"/>"#,
                    a[0],
                    a[1],
                    b[0],
                    b[1],
                    paint("stroke", *color),
                    stroke * by
                );
            }
            MarkKind::Arrow {
                from,
                to,
                color,
                stroke,
            } => {
                let (a, b) = (place(*from), place(*to));
                let width = stroke * by;
                let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                let length = (dx * dx + dy * dy).sqrt().max(0.001);
                let (ux, uy) = (dx / length, dy / length);
                let head = (width * 4.5).max(8.).min(length * 0.6);
                let base = [b[0] - ux * head * 0.85, b[1] - uy * head * 0.85];
                let half = head * 0.5;
                let (left, right) = (
                    [b[0] - ux * head - uy * half, b[1] - uy * head + ux * half],
                    [b[0] - ux * head + uy * half, b[1] - uy * head - ux * half],
                );
                let color_stroke = paint("stroke", *color);
                let _ = write!(
                    svg,
                    r#"<g><line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" {color_stroke} stroke-width="{width:.2}" stroke-linecap="round"/><polygon points="{:.2},{:.2} {:.2},{:.2} {:.2},{:.2}" {} {color_stroke} stroke-width="{:.2}" stroke-linejoin="round"/></g>"#,
                    a[0],
                    a[1],
                    base[0],
                    base[1],
                    b[0],
                    b[1],
                    left[0],
                    left[1],
                    right[0],
                    right[1],
                    paint("fill", *color),
                    (width * 0.5).max(1.),
                );
            }
            MarkKind::Pen {
                points,
                color,
                stroke,
            } => {
                let points: String = points
                    .iter()
                    .map(|p| {
                        let p = place(*p);
                        format!("{:.2},{:.2} ", p[0], p[1])
                    })
                    .collect();
                let _ = write!(
                    svg,
                    r#"<polyline points="{points}" fill="none" {} stroke-width="{:.2}" stroke-linecap="round" stroke-linejoin="round"/>"#,
                    paint("stroke", *color),
                    stroke * by
                );
            }
            MarkKind::Text {
                x,
                y,
                text,
                size,
                color,
                background,
                bold,
                align,
            } => {
                let [x, y] = place([*x, *y]);
                let size = size * by;
                let measured = boxes
                    .iter()
                    .find(|(id, _)| *id == mark.id)
                    .map(|(_, bounds)| *bounds);
                let fill = paint("fill", *color);
                // The block's left edge as laid out, so lining up keeps it.
                let (left, width) = measured.map_or((x, 0.), |bounds| (bounds.x, bounds.width));
                let style = TextStyle {
                    size,
                    bold: *bold,
                    fill: &fill,
                    align: *align,
                    width,
                };
                let at = [if *align == Align::Left { x } else { left }, y];
                if let Some(background) = background
                    && let Some(bounds) = measured
                {
                    let r = bounds.grow(size * 0.3);
                    let _ = write!(
                        svg,
                        r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" rx="{:.2}" {}/>"#,
                        r.x,
                        r.y,
                        r.width,
                        r.height,
                        size * 0.25,
                        paint("fill", *background)
                    );
                    text_svg(&mut svg, "", at, &style, text);
                } else {
                    text_svg(&mut svg, "", at, &style, text);
                }
            }
            MarkKind::Step {
                x,
                y,
                number,
                color,
                size,
            } => {
                let [x, y] = place([*x, *y]);
                let size = size * by;
                let font = size * 0.55;
                let _ = write!(
                    svg,
                    r##"<g><circle cx="{x:.2}" cy="{y:.2}" r="{:.2}" {} stroke="#ffffff" stroke-width="{:.2}"/><text x="{x:.2}" y="{:.2}" text-anchor="middle" font-family="{FONT_FAMILY}" font-size="{font:.2}" font-weight="700" fill="{}">{number}</text></g>"##,
                    size / 2.,
                    paint("fill", *color),
                    (size * 0.07).max(1.),
                    y + font * 0.36,
                    on(*color),
                );
            }
            MarkKind::Highlight { color, .. } => {
                let r = area(mark.kind.area().expect("an area"));
                let _ = write!(
                    svg,
                    r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" rx="{:.2}" {}/>"#,
                    r.x,
                    r.y,
                    r.width,
                    r.height,
                    (2. * by).max(1.),
                    paint("fill", *color)
                );
            }
            MarkKind::Spotlight { dim, .. } => {
                let r = area(mark.kind.area().expect("an area"));
                let _ = write!(
                    svg,
                    r##"<path d="M0 0H{width}V{height}H0Z M{:.2} {:.2}H{:.2}V{:.2}H{:.2}Z" fill-rule="evenodd" fill="#000000" fill-opacity="{:.3}"/>"##,
                    r.x,
                    r.y,
                    r.right(),
                    r.bottom(),
                    r.x,
                    *dim as f32 / 100.
                );
            }
            MarkKind::Blur { .. } | MarkKind::Pixelate { .. } => {}
        }
        all.push_str(&svg);
        if lifts {
            lifted.push_str(&svg);
            any_lifted = true;
        }
    }
    all.push_str("</svg>");
    lifted.push_str("</svg>");
    let parse = |svg: &str| match usvg::Tree::from_str(svg, &svg_options()) {
        Ok(tree) => Some(tree),
        Err(err) => {
            tracing::warn!("drawing marks failed: {err}");
            None
        }
    };
    let tree = parse(&all)?;
    let mut pixmap = Pixmap::new(width, height)?;
    if any_lifted && let Some(lifted) = parse(&lifted) {
        draw_shadow(
            &mut pixmap,
            (1.5 * by).max(1.),
            (2. * by).max(1.),
            |small, scale| {
                resvg::render(
                    &lifted,
                    Transform::from_scale(scale, scale),
                    &mut small.as_mut(),
                );
            },
        );
    }
    resvg::render(&tree, Transform::identity(), &mut pixmap.as_mut());
    Some(pixmap)
}

/// Lays `layer` (premultiplied) over `image`.
pub(crate) fn blend(image: &mut RgbaImage, layer: &Pixmap) {
    if image.dimensions() != (layer.width(), layer.height()) {
        return;
    }
    for (pixel, over) in image
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(layer.data().as_chunks::<4>().0)
    {
        let alpha = over[3] as u32;
        if alpha == 0 {
            continue;
        }
        let under = pixel[3] as u32;
        let rest = 255 - alpha;
        // Premultiplied over, back to straight alpha.
        let out_alpha = alpha + (under * rest + 127) / 255;
        for c in 0..3 {
            let premultiplied = over[c] as u32 + (pixel[c] as u32 * under / 255 * rest + 127) / 255;
            pixel[c] = (premultiplied * 255 / out_alpha.max(1)).min(255) as u8;
        }
        pixel[3] = out_alpha.min(255) as u8;
    }
}

/// How much smaller shadows are made than what casts them: a shadow is
/// soft, so a quarter of the pixels look the same and cost a sixteenth.
const SHADOW_SCALE: f32 = 4.;

/// Draws onto `canvas` the soft shadow of what `cast` draws (into a
/// smaller pixmap, at the scale given), `drop` pixels lower, blurred by
/// `soften`, at `opacity`.
fn draw_shadow_with(
    canvas: &mut Pixmap,
    drop: f32,
    soften: f32,
    opacity: f32,
    cast: impl FnOnce(&mut Pixmap, f32),
) {
    let scale = 1. / SHADOW_SCALE;
    let (width, height) = (
        ((canvas.width() as f32 * scale).ceil() as u32).max(1),
        ((canvas.height() as f32 * scale).ceil() as u32).max(1),
    );
    let Some(mut small) = Pixmap::new(width, height) else {
        return;
    };
    cast(&mut small, scale);
    let mut layer = from_pixmap(small);
    for pixel in layer.pixels_mut() {
        pixel.0 = [0, 0, 0, (pixel.0[3] as f32 * opacity) as u8];
    }
    box_blur(&mut layer, (soften * scale).max(1.));
    let Some(shadow) = to_pixmap(&layer) else {
        return;
    };
    let paint = tiny_skia::PixmapPaint {
        quality: tiny_skia::FilterQuality::Bilinear,
        ..Default::default()
    };
    canvas.draw_pixmap(
        0,
        0,
        shadow.as_ref(),
        &paint,
        Transform::from_row(SHADOW_SCALE, 0., 0., SHADOW_SCALE, 0., drop),
        None,
    );
}

/// The light shadow under marks.
fn draw_shadow(canvas: &mut Pixmap, drop: f32, soften: f32, cast: impl FnOnce(&mut Pixmap, f32)) {
    draw_shadow_with(canvas, drop, soften, 0.35, cast);
}

/// The picture as a tiny-skia pixmap (premultiplied).
fn to_pixmap(image: &RgbaImage) -> Option<Pixmap> {
    let mut data = image.as_raw().clone();
    for pixel in data.as_chunks_mut::<4>().0 {
        let alpha = pixel[3] as u32;
        if alpha < 255 {
            for c in &mut pixel[..3] {
                *c = ((*c as u32 * alpha + 127) / 255) as u8;
            }
        }
    }
    Pixmap::from_vec(
        data,
        tiny_skia::IntSize::from_wh(image.width(), image.height())?,
    )
}

/// A pixmap back as a picture (straight alpha).
fn from_pixmap(pixmap: Pixmap) -> RgbaImage {
    let (width, height) = (pixmap.width(), pixmap.height());
    let mut data = pixmap.take();
    for pixel in data.as_chunks_mut::<4>().0 {
        let alpha = pixel[3] as u32;
        if alpha > 0 && alpha < 255 {
            for c in &mut pixel[..3] {
                *c = ((*c as u32 * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
    RgbaImage::from_raw(width, height, data).expect("a pixmap's size")
}

/// A rectangle with round corners of `radius`.
fn rounded_rect(x: f32, y: f32, width: f32, height: f32, radius: f32) -> Option<tiny_skia::Path> {
    let r = radius.min(width / 2.).min(height / 2.).max(0.);
    if r <= 0. {
        return tiny_skia::Rect::from_xywh(x, y, width, height).map(PathBuilder::from_rect);
    }
    // Quarter circles as cubic curves.
    let k = r * 0.552_284_8;
    let (right, bottom) = (x + width, y + height);
    let mut path = PathBuilder::new();
    path.move_to(x + r, y);
    path.line_to(right - r, y);
    path.cubic_to(right - r + k, y, right, y + r - k, right, y + r);
    path.line_to(right, bottom - r);
    path.cubic_to(
        right,
        bottom - r + k,
        right - r + k,
        bottom,
        right - r,
        bottom,
    );
    path.line_to(x + r, bottom);
    path.cubic_to(x + r - k, bottom, x, bottom - r + k, x, bottom - r);
    path.line_to(x, y + r);
    path.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    path.close();
    path.finish()
}

/// `image` on a background, with space around it, round corners and a
/// shadow; sizes in pixels of the result, times `scale`.
fn framed(image: &RgbaImage, frame: &Frame, scale: f32) -> RgbaImage {
    let padding = (frame.padding as f32 * scale).round() as u32;
    let (width, height) = image.dimensions();
    let (outer_w, outer_h) = (width + 2 * padding, height + 2 * padding);
    let Some(mut canvas) = Pixmap::new(outer_w, outer_h) else {
        return image.clone();
    };
    let full = tiny_skia::Rect::from_xywh(0., 0., outer_w as f32, outer_h as f32);
    let sk_color = |color: Color| {
        let [r, g, b, a] = color.0;
        tiny_skia::Color::from_rgba8(r, g, b, a)
    };
    if let (Some(background), Some(full)) = (frame.background, full) {
        let mut paint = SkPaint::default();
        match background {
            Paint::Solid(color) => paint.set_color(sk_color(color)),
            Paint::Gradient([from, to]) => {
                if let Some(shader) = tiny_skia::LinearGradient::new(
                    tiny_skia::Point::from_xy(0., 0.),
                    tiny_skia::Point::from_xy(outer_w as f32, outer_h as f32),
                    vec![
                        tiny_skia::GradientStop::new(0., sk_color(from)),
                        tiny_skia::GradientStop::new(1., sk_color(to)),
                    ],
                    tiny_skia::SpreadMode::Pad,
                    Transform::identity(),
                ) {
                    paint.shader = shader;
                }
            }
        }
        canvas.fill_rect(full, &paint, Transform::identity(), None);
    }
    let radius = frame.radius as f32 * scale;
    let (x, y) = (padding as f32, padding as f32);
    let Some(shape) = rounded_rect(x, y, width as f32, height as f32, radius) else {
        return image.clone();
    };
    if frame.shadow > 0 && padding > 0 {
        let strength = frame.shadow as f32 / 100.;
        let spread = padding as f32 * 0.45 * strength.sqrt();
        draw_shadow_with(
            &mut canvas,
            spread * 0.35,
            spread,
            0.55 * strength,
            |small, scale| {
                let mut paint = SkPaint::default();
                paint.set_color(tiny_skia::Color::BLACK);
                paint.anti_alias = true;
                small.fill_path(
                    &shape,
                    &paint,
                    FillRule::Winding,
                    Transform::from_scale(scale, scale),
                    None,
                );
            },
        );
    }
    let Some(picture) = to_pixmap(image) else {
        return image.clone();
    };
    // The picture fills its rounded shape (no mask the size of it all).
    let paint = SkPaint {
        anti_alias: true,
        shader: tiny_skia::Pattern::new(
            picture.as_ref(),
            tiny_skia::SpreadMode::Pad,
            tiny_skia::FilterQuality::Nearest,
            1.,
            Transform::from_translate(x, y),
        ),
        ..Default::default()
    };
    canvas.fill_path(
        &shape,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
    from_pixmap(canvas)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{MarkType, Size, Turn};

    fn picture() -> RgbaImage {
        RgbaImage::from_fn(40, 20, |x, y| {
            image::Rgba([(x * 6) as u8, (y * 12) as u8, 100, 255])
        })
    }

    #[test]
    fn nothing_changed_is_the_same_picture() {
        let source = picture();
        let result = render(&source, (40, 20), &ImageEdit::default(), Quality::Full);
        assert_eq!(result, source);
    }

    #[test]
    fn crop_turn_and_resize_make_the_expected_size() {
        let source = picture();
        let mut edit = ImageEdit {
            crop: Some(Rect::new(10., 0., 20., 10.)),
            ..Default::default()
        };
        edit.turn(Turn::RotateRight, (40, 20));
        let result = render(&source, (40, 20), &edit, Quality::Full);
        assert_eq!(result.dimensions(), (10, 20));
        edit.resize = Some(Size {
            width: 5,
            height: 10,
        });
        edit.frame = Some(Frame {
            padding: 3,
            ..Default::default()
        });
        let result = render(&source, (40, 20), &edit, Quality::Full);
        assert_eq!(result.dimensions(), (11, 16));
        assert_eq!(result.dimensions(), edit.result_size((40, 20)));
    }

    #[test]
    fn a_preview_is_the_result_made_smaller() {
        let source = RgbaImage::from_pixel(400, 200, image::Rgba([10, 20, 30, 255]));
        let small = imageops::thumbnail(&source, 100, 50);
        let edit = ImageEdit {
            crop: Some(Rect::new(100., 0., 200., 200.)),
            frame: Some(Frame::default()),
            ..Default::default()
        };
        let full = render(&source, (400, 200), &edit, Quality::Full);
        let preview = render(&small, (400, 200), &edit, Quality::Preview);
        assert_eq!(full.dimensions(), (200 + 128, 200 + 128));
        assert_eq!(preview.dimensions(), (50 + 32, 50 + 32));
    }

    #[test]
    fn grayscale_has_no_color_left() {
        let mut edit = ImageEdit::default();
        edit.adjust.grayscale = true;
        let result = render(&picture(), (40, 20), &edit, Quality::Full);
        assert!(
            result
                .pixels()
                .all(|p| p.0[0] == p.0[1] && p.0[1] == p.0[2])
        );
    }

    #[test]
    fn hidden_areas_lose_their_detail() {
        let source = RgbaImage::from_fn(60, 60, |x, _| {
            let v = if x % 2 == 0 { 0 } else { 255 };
            image::Rgba([v, v, v, 255])
        });
        let mut edit = ImageEdit::default();
        edit.add_mark(
            MarkType::Pixelate.mark([0., 0.], [30., 30.], Color::RED),
            (60, 60),
        )
        .unwrap();
        edit.add_mark(
            MarkType::Blur.mark([30., 30.], [60., 60.], Color::RED),
            (60, 60),
        )
        .unwrap();
        let result = render(&source, (60, 60), &edit, Quality::Full);
        let spread = |x0: u32, y0: u32| {
            let values: Vec<u8> = (x0..x0 + 20)
                .map(|x| result.get_pixel(x, y0 + 10).0[0])
                .collect();
            values.iter().max().unwrap() - values.iter().min().unwrap()
        };
        assert!(spread(2, 2) < 200, "pixelated");
        assert!(spread(36, 36) < 40, "blurred");
        assert!(spread(36, 2) > 200, "left alone");
    }

    #[test]
    fn marks_draw_over_the_picture() {
        let source = RgbaImage::from_pixel(200, 100, image::Rgba([255, 255, 255, 255]));
        let mut edit = ImageEdit::default();
        edit.add_mark(
            MarkType::Rect.mark([20., 20.], [180., 80.], Color::RED),
            (200, 100),
        )
        .unwrap();
        edit.add_mark(
            MarkType::Step.mark([100., 50.], [100., 50.], Color::RED),
            (200, 100),
        )
        .unwrap();
        let result = render(&source, (200, 100), &edit, Quality::Full);
        let red = |p: &image::Rgba<u8>| p.0[0] > 200 && p.0[1] < 120 && p.0[2] < 120;
        assert!(red(result.get_pixel(20, 50)), "the rectangle's edge");
        assert!(result.pixels().filter(|p| red(p)).count() > 300);
    }

    #[test]
    fn text_lines_line_up() {
        let source = RgbaImage::from_pixel(400, 120, image::Rgba([255, 255, 255, 255]));
        // Where the short second line's ink is, across.
        let middle_of_second_line = |align: Align| {
            let mut edit = ImageEdit::default();
            let text = MarkKind::Text {
                x: 10.,
                y: 10.,
                text: "A much longer first line\nI".into(),
                size: 30.,
                color: Color::BLACK,
                background: None,
                bold: true,
                align,
            };
            edit.add_mark(text, (400, 120)).unwrap();
            let image = render(&source, (400, 120), &edit, Quality::Full);
            let row = (10. + 30. * 1.25 + 15.) as u32;
            let ink: Vec<f32> = (0..400)
                .filter(|x| image.get_pixel(*x, row).0[0] < 100)
                .map(|x| x as f32)
                .collect();
            ink.iter().sum::<f32>() / ink.len().max(1) as f32
        };
        let left = middle_of_second_line(Align::Left);
        let center = middle_of_second_line(Align::Center);
        let right = middle_of_second_line(Align::Right);
        assert!(left < center && center < right, "{left} {center} {right}");
    }

    #[test]
    fn text_is_escaped() {
        assert_eq!(escape("<a href='x'>&"), "&lt;a href=&apos;x&apos;&gt;&amp;");
    }
}

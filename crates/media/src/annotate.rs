//! Annotations drawn over a frame, and burning them into a picture.
//!
//! Shapes are stored in frame pixels, so they stay put when the pane is
//! resized and can be rasterized at full resolution for the agent.

use image::{Rgba, RgbaImage};

use crate::frame::Frame;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Rect,
    Arrow,
    Pen,
    /// Numbered markers: "1", "2"… so the message can refer to them.
    Pin,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    Rect { from: (f32, f32), to: (f32, f32) },
    Arrow { from: (f32, f32), to: (f32, f32) },
    Pen { points: Vec<(f32, f32)> },
    Pin { at: (f32, f32), number: u32 },
}

/// The annotation color: a clear red that reads on light and dark screens.
pub const COLOR: [u8; 4] = [255, 59, 48, 255];

/// Stroke width relative to the frame, so it looks the same on any resolution.
pub fn stroke_width(frame: &Frame) -> f32 {
    (frame.width.max(frame.height) as f32 / 300.0).clamp(2.0, 8.0)
}

/// A copy of `frame` with `shapes` drawn on it.
pub fn render(frame: &Frame, shapes: &[Shape]) -> Frame {
    let mut image = frame.to_image();
    let width = stroke_width(frame);
    for shape in shapes {
        match shape {
            Shape::Rect { from, to } => {
                let (x0, y0, x1, y1) = (
                    from.0.min(to.0),
                    from.1.min(to.1),
                    from.0.max(to.0),
                    from.1.max(to.1),
                );
                line(&mut image, (x0, y0), (x1, y0), width);
                line(&mut image, (x1, y0), (x1, y1), width);
                line(&mut image, (x1, y1), (x0, y1), width);
                line(&mut image, (x0, y1), (x0, y0), width);
            }
            Shape::Arrow { from, to } => {
                line(&mut image, *from, *to, width);
                for head in arrow_head(*from, *to, width * 5.0) {
                    line(&mut image, *to, head, width);
                }
            }
            Shape::Pen { points } => {
                for pair in points.windows(2) {
                    line(&mut image, pair[0], pair[1], width);
                }
            }
            Shape::Pin { at, number } => pin(&mut image, *at, *number, width * 4.0),
        }
    }
    let (w, h) = image.dimensions();
    Frame::from_rgba(w, h, image.into_raw())
}

/// The two end points of an arrow head at `to`.
pub fn arrow_head(from: (f32, f32), to: (f32, f32), length: f32) -> [(f32, f32); 2] {
    let angle = (to.1 - from.1).atan2(to.0 - from.0);
    let spread = 0.5;
    [
        (
            to.0 - length * (angle - spread).cos(),
            to.1 - length * (angle - spread).sin(),
        ),
        (
            to.0 - length * (angle + spread).cos(),
            to.1 - length * (angle + spread).sin(),
        ),
    ]
}

/// A thick line, drawn as discs along its length.
fn line(image: &mut RgbaImage, from: (f32, f32), to: (f32, f32), width: f32) {
    let length = ((to.0 - from.0).powi(2) + (to.1 - from.1).powi(2)).sqrt();
    let steps = (length / (width / 3.0).max(0.5)).ceil().max(1.0) as usize;
    for step in 0..=steps {
        let t = step as f32 / steps as f32;
        disc(
            image,
            (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t),
            width / 2.0,
            COLOR,
        );
    }
}

fn disc(image: &mut RgbaImage, center: (f32, f32), radius: f32, color: [u8; 4]) {
    let (w, h) = image.dimensions();
    let r = radius.max(0.5);
    let (x0, x1) = (
        (center.0 - r).floor().max(0.0) as u32,
        (center.0 + r).ceil().min(w as f32 - 1.0).max(0.0) as u32,
    );
    let (y0, y1) = (
        (center.1 - r).floor().max(0.0) as u32,
        (center.1 + r).ceil().min(h as f32 - 1.0).max(0.0) as u32,
    );
    for y in y0..=y1 {
        for x in x0..=x1 {
            if (x as f32 - center.0).powi(2) + (y as f32 - center.1).powi(2) <= r * r {
                image.put_pixel(x, y, Rgba(color));
            }
        }
    }
}

/// A filled circle with a white number (a 3×5 pixel font, scaled).
fn pin(image: &mut RgbaImage, at: (f32, f32), number: u32, radius: f32) {
    disc(image, at, radius, COLOR);
    let digits = number.to_string();
    let scale = (radius / 4.0).max(1.0);
    let glyph_w = 4.0 * scale;
    let total = glyph_w * digits.len() as f32 - scale;
    let mut x = at.0 - total / 2.0;
    let y = at.1 - 2.5 * scale;
    for digit in digits.chars() {
        if let Some(bits) = digit.to_digit(10).map(|d| DIGITS[d as usize]) {
            for (row, pattern) in bits.iter().enumerate() {
                for col in 0..3 {
                    if pattern & (0b100 >> col) != 0 {
                        let (px, py) = (x + col as f32 * scale, y + row as f32 * scale);
                        for dy in 0..scale.ceil() as u32 {
                            for dx in 0..scale.ceil() as u32 {
                                let (fx, fy) = (px as u32 + dx, py as u32 + dy);
                                if fx < image.width() && fy < image.height() {
                                    image.put_pixel(fx, fy, Rgba([255, 255, 255, 255]));
                                }
                            }
                        }
                    }
                }
            }
        }
        x += glyph_w;
    }
}

/// 3×5 bitmaps for 0–9, one row per entry (bit 2 is the left column).
const DIGITS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b010, 0b010, 0b010],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
];

#[cfg(test)]
mod tests {
    use super::*;

    fn red_pixels(frame: &Frame) -> usize {
        // BGRA: red is the third byte.
        frame
            .bgra()
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[2] == 255 && p[1] == 59)
            .count()
    }

    #[test]
    fn shapes_are_burned_in() {
        let blank = Frame::from_rgba(200, 200, vec![0; 200 * 200 * 4]);
        assert_eq!(red_pixels(&blank), 0);
        let marked = render(
            &blank,
            &[
                Shape::Rect {
                    from: (20.0, 20.0),
                    to: (80.0, 60.0),
                },
                Shape::Arrow {
                    from: (100.0, 100.0),
                    to: (150.0, 150.0),
                },
                Shape::Pin {
                    at: (40.0, 150.0),
                    number: 7,
                },
            ],
        );
        assert!(red_pixels(&marked) > 300);
        // The original frame is left untouched.
        assert_eq!(red_pixels(&blank), 0);
    }
}

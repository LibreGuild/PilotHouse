//! What an image edit is: a description of the changes, never the pixels.
//!
//! The picture on disk is only read. An [`ImageEdit`] says how to turn it
//! into the result (turn, crop, adjust, mark, resize, frame), so every step
//! can be changed or undone at any time, by hand or by an agent, and the
//! result is made again from the original. Positions (the crop, the marks)
//! are pixels of the original picture as turned, so they stay on the same
//! things whatever the crop or size of the result.

use anyhow::{Result, bail};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

/// The largest side of a result, and its most pixels: decoded it takes four
/// bytes a pixel, and a GPU texture has at most 16,384 pixels a side.
pub const MAX_SIDE: u32 = 16_384;
pub const MAX_PIXELS: u64 = 100_000_000;
/// The most marks on one picture, and the longest text of one.
const MAX_MARKS: usize = 500;
const MAX_TEXT: usize = 500;
const MAX_POINTS: usize = 5_000;

/// A color, written `#rrggbb` or `#rrggbbaa`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color(pub [u8; 4]);

impl Color {
    pub const RED: Color = Color([255, 59, 48, 255]);
    pub const YELLOW: Color = Color([255, 214, 10, 255]);
    pub const WHITE: Color = Color([255, 255, 255, 255]);
    pub const BLACK: Color = Color([0, 0, 0, 255]);

    pub fn parse(text: &str) -> Option<Color> {
        let hex = text.trim().strip_prefix('#')?;
        let byte = |at: usize| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok();
        match hex.len() {
            6 => Some(Color([byte(0)?, byte(2)?, byte(4)?, 255])),
            8 => Some(Color([byte(0)?, byte(2)?, byte(4)?, byte(6)?])),
            3 => {
                let nibble = |at: usize| {
                    u8::from_str_radix(hex.get(at..at + 1)?, 16)
                        .ok()
                        .map(|n| n * 17)
                };
                Some(Color([nibble(0)?, nibble(1)?, nibble(2)?, 255]))
            }
            _ => None,
        }
    }

    /// `#rrggbb`, with `aa` when not opaque.
    pub fn hex(self) -> String {
        let [r, g, b, a] = self.0;
        if a == 255 {
            format!("#{r:02x}{g:02x}{b:02x}")
        } else {
            format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
        }
    }

    pub fn alpha(self) -> f32 {
        self.0[3] as f32 / 255.
    }

    pub fn with_alpha(self, alpha: u8) -> Color {
        let [r, g, b, _] = self.0;
        Color([r, g, b, alpha])
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.hex())
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Color::parse(&text).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "`{text}` is not a color: write #rrggbb or #rrggbbaa"
            ))
        })
    }
}

/// A rectangle in pixels of the turned original.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    /// The rectangle with corners `a` and `b`, either way round.
    pub fn from_corners(a: [f32; 2], b: [f32; 2]) -> Rect {
        Rect::new(
            a[0].min(b[0]),
            a[1].min(b[1]),
            (a[0] - b[0]).abs(),
            (a[1] - b[1]).abs(),
        )
    }

    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    pub fn contains(&self, point: [f32; 2]) -> bool {
        point[0] >= self.x
            && point[0] <= self.right()
            && point[1] >= self.y
            && point[1] <= self.bottom()
    }

    /// Grown by `by` on every side.
    pub fn grow(&self, by: f32) -> Rect {
        Rect::new(
            self.x - by,
            self.y - by,
            self.width + 2. * by,
            self.height + 2. * by,
        )
    }

    /// Clamped into a `width` × `height` picture, at least one pixel.
    pub fn clamped(&self, width: f32, height: f32) -> Rect {
        let x = self.x.clamp(0., (width - 1.).max(0.));
        let y = self.y.clamp(0., (height - 1.).max(0.));
        Rect::new(
            x,
            y,
            self.width.clamp(1., width - x),
            self.height.clamp(1., height - y),
        )
    }

    fn is_finite(&self) -> bool {
        [self.x, self.y, self.width, self.height]
            .iter()
            .all(|value| value.is_finite())
    }
}

/// How the original is turned: flipped left to right first (when
/// `flipped`), then turned a quarter clockwise `turns` times.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Orientation {
    pub turns: u8,
    pub flipped: bool,
}

/// One step of turning, as the toolbar offers them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Turn {
    RotateLeft,
    RotateRight,
    FlipHorizontal,
    FlipVertical,
}

impl Turn {
    /// Where point `p` of a `width` × `height` picture goes.
    fn map(self, p: [f32; 2], width: f32, height: f32) -> [f32; 2] {
        match self {
            Turn::RotateRight => [height - p[1], p[0]],
            Turn::RotateLeft => [p[1], width - p[0]],
            Turn::FlipHorizontal => [width - p[0], p[1]],
            Turn::FlipVertical => [p[0], height - p[1]],
        }
    }

    fn swaps_sides(self) -> bool {
        matches!(self, Turn::RotateLeft | Turn::RotateRight)
    }
}

impl Orientation {
    /// The orientation after `turn`, as seen on the turned picture.
    fn then(self, turn: Turn) -> Orientation {
        // Flipping after turning t quarters is flipping first and turning
        // the other way: flip ∘ rot(t) = rot(-t) ∘ flip.
        let (turns, flipped) = match turn {
            Turn::RotateRight => (self.turns + 1, self.flipped),
            Turn::RotateLeft => (self.turns + 3, self.flipped),
            Turn::FlipHorizontal => (4 - self.turns % 4, !self.flipped),
            Turn::FlipVertical => (6 - self.turns % 4, !self.flipped),
        };
        Orientation {
            turns: turns % 4,
            flipped,
        }
    }

    /// The size of a `width` × `height` original once turned.
    pub fn size(self, width: u32, height: u32) -> (u32, u32) {
        if self.turns % 2 == 1 {
            (height, width)
        } else {
            (width, height)
        }
    }
}

/// Changes to the whole picture's colors. Zero (or off) is unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Adjust {
    /// -100 to 100.
    pub brightness: f32,
    /// -100 to 100.
    pub contrast: f32,
    /// -100 (gray) to 100.
    pub saturation: f32,
    pub grayscale: bool,
    pub invert: bool,
    /// Blur radius in pixels of the original, 0 to 50.
    pub blur: f32,
    /// 0 to 100.
    pub sharpen: f32,
}

impl Adjust {
    pub fn is_identity(&self) -> bool {
        *self == Adjust::default()
    }

    fn sanitize(&mut self) {
        let clamp = |value: &mut f32, min: f32, max: f32| {
            *value = if value.is_finite() {
                value.clamp(min, max)
            } else {
                0.
            }
        };
        clamp(&mut self.brightness, -100., 100.);
        clamp(&mut self.contrast, -100., 100.);
        clamp(&mut self.saturation, -100., 100.);
        clamp(&mut self.blur, 0., 50.);
        clamp(&mut self.sharpen, 0., 100.);
    }
}

/// The size of the result (of the cropped picture, before any frame).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

/// What shows around a framed picture: one color, or a gradient from the
/// top left to the bottom right. `None` is transparent.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Paint {
    Gradient([Color; 2]),
    Solid(Color),
}

/// A frame for a screenshot: space around it, a background, round corners
/// and a shadow.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Frame {
    /// Space around the picture, in pixels of the result.
    pub padding: u32,
    pub background: Option<Paint>,
    /// Corner radius, in pixels of the result.
    pub radius: u32,
    /// 0 (none) to 100.
    pub shadow: u8,
}

impl Default for Frame {
    fn default() -> Self {
        Frame {
            padding: 64,
            background: Some(Paint::Gradient([
                Color([99, 102, 241, 255]),
                Color([168, 85, 247, 255]),
            ])),
            radius: 12,
            shadow: 40,
        }
    }
}

/// How the lines of a text line up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// A mark drawn on the picture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mark {
    #[serde(default)]
    pub id: u32,
    #[serde(flatten)]
    pub kind: MarkKind,
}

fn red() -> Color {
    Color::RED
}

fn highlight() -> Color {
    Color::YELLOW.with_alpha(110)
}

fn yes() -> bool {
    true
}

fn half() -> u8 {
    55
}

/// What a mark is. Sizes left at 0 are made to suit the picture when the
/// mark is added.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum MarkKind {
    Rect {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        #[serde(default = "red")]
        color: Color,
        /// Line width; 0 suits the picture.
        #[serde(default)]
        stroke: f32,
        #[serde(default)]
        fill: Option<Color>,
        #[serde(default)]
        radius: f32,
    },
    Ellipse {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        #[serde(default = "red")]
        color: Color,
        #[serde(default)]
        stroke: f32,
        #[serde(default)]
        fill: Option<Color>,
    },
    Arrow {
        from: [f32; 2],
        to: [f32; 2],
        #[serde(default = "red")]
        color: Color,
        #[serde(default)]
        stroke: f32,
    },
    Line {
        from: [f32; 2],
        to: [f32; 2],
        #[serde(default = "red")]
        color: Color,
        #[serde(default)]
        stroke: f32,
    },
    Pen {
        points: Vec<[f32; 2]>,
        #[serde(default = "red")]
        color: Color,
        #[serde(default)]
        stroke: f32,
    },
    /// Text whose top left corner is at `x`, `y`.
    Text {
        x: f32,
        y: f32,
        text: String,
        /// Font size; 0 suits the picture.
        #[serde(default)]
        size: f32,
        #[serde(default = "red")]
        color: Color,
        /// A box behind the text.
        #[serde(default)]
        background: Option<Color>,
        #[serde(default = "yes")]
        bold: bool,
        /// How its lines line up (its top left corner stays put).
        #[serde(default)]
        align: Align,
    },
    /// A numbered circle centered on `x`, `y`.
    Step {
        x: f32,
        y: f32,
        #[serde(default)]
        number: u32,
        #[serde(default = "red")]
        color: Color,
        /// Diameter; 0 suits the picture.
        #[serde(default)]
        size: f32,
    },
    /// A see-through marker over an area.
    Highlight {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        #[serde(default = "highlight")]
        color: Color,
    },
    /// An area blurred past reading (an email, a token).
    Blur {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        /// Blur radius; 0 suits the area.
        #[serde(default)]
        strength: f32,
    },
    /// An area made of large squares.
    Pixelate {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        /// Square size; 0 suits the area.
        #[serde(default)]
        size: f32,
    },
    /// Everything but an area dimmed.
    Spotlight {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        /// How dark the rest gets, 0 to 100.
        #[serde(default = "half")]
        dim: u8,
    },
}

/// The kinds of marks, for the toolbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MarkType {
    Rect,
    Ellipse,
    Arrow,
    Line,
    Pen,
    Text,
    Step,
    Highlight,
    Blur,
    Pixelate,
    Spotlight,
}

impl MarkType {
    /// Whether the mark is an area drawn by dragging its corners.
    pub fn is_area(self) -> bool {
        matches!(
            self,
            MarkType::Rect
                | MarkType::Ellipse
                | MarkType::Highlight
                | MarkType::Blur
                | MarkType::Pixelate
                | MarkType::Spotlight
        )
    }

    /// A new mark of this type over `area` (from `from` to `to` for lines),
    /// in `color` where it has one.
    pub fn mark(self, from: [f32; 2], to: [f32; 2], color: Color) -> MarkKind {
        let r = Rect::from_corners(from, to);
        let (x, y, width, height) = (r.x, r.y, r.width, r.height);
        match self {
            MarkType::Rect => MarkKind::Rect {
                x,
                y,
                width,
                height,
                color,
                stroke: 0.,
                fill: None,
                radius: 0.,
            },
            MarkType::Ellipse => MarkKind::Ellipse {
                x,
                y,
                width,
                height,
                color,
                stroke: 0.,
                fill: None,
            },
            MarkType::Arrow => MarkKind::Arrow {
                from,
                to,
                color,
                stroke: 0.,
            },
            MarkType::Line => MarkKind::Line {
                from,
                to,
                color,
                stroke: 0.,
            },
            MarkType::Pen => MarkKind::Pen {
                points: vec![from, to],
                color,
                stroke: 0.,
            },
            MarkType::Text => MarkKind::Text {
                x: from[0],
                y: from[1],
                text: "Text".into(),
                size: 0.,
                color,
                background: None,
                bold: true,
                align: Align::Left,
            },
            MarkType::Step => MarkKind::Step {
                x: from[0],
                y: from[1],
                number: 0,
                color,
                size: 0.,
            },
            MarkType::Highlight => MarkKind::Highlight {
                x,
                y,
                width,
                height,
                color: highlight(),
            },
            MarkType::Blur => MarkKind::Blur {
                x,
                y,
                width,
                height,
                strength: 0.,
            },
            MarkType::Pixelate => MarkKind::Pixelate {
                x,
                y,
                width,
                height,
                size: 0.,
            },
            MarkType::Spotlight => MarkKind::Spotlight {
                x,
                y,
                width,
                height,
                dim: half(),
            },
        }
    }
}

impl MarkKind {
    pub fn kind(&self) -> MarkType {
        match self {
            MarkKind::Rect { .. } => MarkType::Rect,
            MarkKind::Ellipse { .. } => MarkType::Ellipse,
            MarkKind::Arrow { .. } => MarkType::Arrow,
            MarkKind::Line { .. } => MarkType::Line,
            MarkKind::Pen { .. } => MarkType::Pen,
            MarkKind::Text { .. } => MarkType::Text,
            MarkKind::Step { .. } => MarkType::Step,
            MarkKind::Highlight { .. } => MarkType::Highlight,
            MarkKind::Blur { .. } => MarkType::Blur,
            MarkKind::Pixelate { .. } => MarkType::Pixelate,
            MarkKind::Spotlight { .. } => MarkType::Spotlight,
        }
    }

    /// The mark's area, for an area mark.
    pub fn area(&self) -> Option<Rect> {
        match *self {
            MarkKind::Rect {
                x,
                y,
                width,
                height,
                ..
            }
            | MarkKind::Ellipse {
                x,
                y,
                width,
                height,
                ..
            }
            | MarkKind::Highlight {
                x,
                y,
                width,
                height,
                ..
            }
            | MarkKind::Blur {
                x,
                y,
                width,
                height,
                ..
            }
            | MarkKind::Pixelate {
                x,
                y,
                width,
                height,
                ..
            }
            | MarkKind::Spotlight {
                x,
                y,
                width,
                height,
                ..
            } => Some(Rect::new(x, y, width, height)),
            _ => None,
        }
    }

    /// Sets an area mark's area.
    pub fn set_area(&mut self, area: Rect) {
        match self {
            MarkKind::Rect {
                x,
                y,
                width,
                height,
                ..
            }
            | MarkKind::Ellipse {
                x,
                y,
                width,
                height,
                ..
            }
            | MarkKind::Highlight {
                x,
                y,
                width,
                height,
                ..
            }
            | MarkKind::Blur {
                x,
                y,
                width,
                height,
                ..
            }
            | MarkKind::Pixelate {
                x,
                y,
                width,
                height,
                ..
            }
            | MarkKind::Spotlight {
                x,
                y,
                width,
                height,
                ..
            } => {
                (*x, *y, *width, *height) = (area.x, area.y, area.width, area.height);
            }
            _ => {}
        }
    }

    /// Every point that places the mark, to move or turn it.
    fn points_mut(&mut self) -> Vec<&mut [f32; 2]> {
        match self {
            MarkKind::Arrow { from, to, .. } | MarkKind::Line { from, to, .. } => vec![from, to],
            MarkKind::Pen { points, .. } => points.iter_mut().collect(),
            _ => Vec::new(),
        }
    }

    /// Moves the mark by `dx`, `dy`.
    pub fn translate(&mut self, dx: f32, dy: f32) {
        if let Some(area) = self.area() {
            self.set_area(Rect::new(area.x + dx, area.y + dy, area.width, area.height));
            return;
        }
        match self {
            MarkKind::Text { x, y, .. } | MarkKind::Step { x, y, .. } => {
                *x += dx;
                *y += dy;
            }
            _ => {
                for point in self.points_mut() {
                    point[0] += dx;
                    point[1] += dy;
                }
            }
        }
    }

    /// Follows the picture through `turn` (it is `width` × `height`
    /// before). Text stays upright, at its turned corner.
    fn turn(&mut self, turn: Turn, width: f32, height: f32) {
        if let Some(area) = self.area() {
            let a = turn.map([area.x, area.y], width, height);
            let b = turn.map([area.right(), area.bottom()], width, height);
            self.set_area(Rect::from_corners(a, b));
            return;
        }
        match self {
            MarkKind::Text { x, y, .. } | MarkKind::Step { x, y, .. } => {
                [*x, *y] = turn.map([*x, *y], width, height);
            }
            _ => {
                for point in self.points_mut() {
                    *point = turn.map(*point, width, height);
                }
            }
        }
    }

    /// The color a mark is drawn in, for marks that have one.
    pub fn color(&self) -> Option<Color> {
        match self {
            MarkKind::Rect { color, .. }
            | MarkKind::Ellipse { color, .. }
            | MarkKind::Arrow { color, .. }
            | MarkKind::Line { color, .. }
            | MarkKind::Pen { color, .. }
            | MarkKind::Text { color, .. }
            | MarkKind::Step { color, .. }
            | MarkKind::Highlight { color, .. } => Some(*color),
            _ => None,
        }
    }

    pub fn set_color(&mut self, new: Color) {
        match self {
            MarkKind::Highlight { color, .. } => *color = new.with_alpha(color.0[3]),
            MarkKind::Rect { color, .. }
            | MarkKind::Ellipse { color, .. }
            | MarkKind::Arrow { color, .. }
            | MarkKind::Line { color, .. }
            | MarkKind::Pen { color, .. }
            | MarkKind::Text { color, .. }
            | MarkKind::Step { color, .. } => *color = new,
            _ => {}
        }
    }

    /// The mark at `alpha` (0 to 1) of its own opacity, fading in or out.
    /// Areas that hide stay hidden.
    pub fn faded(&self, alpha: f32) -> MarkKind {
        let alpha = alpha.clamp(0., 1.);
        let fade = |color: &mut Color| color.0[3] = (color.0[3] as f32 * alpha).round() as u8;
        let mut kind = self.clone();
        match &mut kind {
            MarkKind::Rect { color, fill, .. } | MarkKind::Ellipse { color, fill, .. } => {
                fade(color);
                if let Some(fill) = fill {
                    fade(fill);
                }
            }
            MarkKind::Text {
                color, background, ..
            } => {
                fade(color);
                if let Some(background) = background {
                    fade(background);
                }
            }
            MarkKind::Arrow { color, .. }
            | MarkKind::Line { color, .. }
            | MarkKind::Pen { color, .. }
            | MarkKind::Step { color, .. }
            | MarkKind::Highlight { color, .. } => fade(color),
            MarkKind::Spotlight { dim, .. } => *dim = (*dim as f32 * alpha).round() as u8,
            MarkKind::Blur { .. } | MarkKind::Pixelate { .. } => {}
        }
        kind
    }

    /// The line width, for marks drawn with lines.
    pub fn stroke(&self) -> Option<f32> {
        match self {
            MarkKind::Rect { stroke, .. }
            | MarkKind::Ellipse { stroke, .. }
            | MarkKind::Arrow { stroke, .. }
            | MarkKind::Line { stroke, .. }
            | MarkKind::Pen { stroke, .. } => Some(*stroke),
            _ => None,
        }
    }

    pub fn set_stroke(&mut self, width: f32) {
        if let MarkKind::Rect { stroke, .. }
        | MarkKind::Ellipse { stroke, .. }
        | MarkKind::Arrow { stroke, .. }
        | MarkKind::Line { stroke, .. }
        | MarkKind::Pen { stroke, .. } = self
        {
            *stroke = width;
        }
    }

    /// Roughly where the mark is, for picking it with the pointer.
    pub fn bounds(&self) -> Rect {
        if let Some(area) = self.area() {
            return area;
        }
        match self {
            MarkKind::Text {
                x, y, text, size, ..
            } => {
                let lines = text.lines().count().max(1) as f32;
                let longest = text.lines().map(|l| l.chars().count()).max().unwrap_or(1) as f32;
                Rect::new(*x, *y, longest * size * 0.58, lines * size * 1.25)
            }
            MarkKind::Step { x, y, size, .. } => {
                Rect::new(x - size / 2., y - size / 2., *size, *size)
            }
            MarkKind::Arrow { from, to, .. } | MarkKind::Line { from, to, .. } => {
                Rect::from_corners(*from, *to)
            }
            MarkKind::Pen { points, .. } => {
                let (mut min, mut max) = ([f32::MAX; 2], [f32::MIN; 2]);
                for p in points {
                    min = [min[0].min(p[0]), min[1].min(p[1])];
                    max = [max[0].max(p[0]), max[1].max(p[1])];
                }
                Rect::from_corners(min, max)
            }
            _ => Rect::new(0., 0., 0., 0.),
        }
    }

    /// Sizes left at 0 made to suit a picture whose longer side is `side`.
    pub(crate) fn fill_sizes(&mut self, side: f32) {
        let line = (side / 280.).clamp(2., 48.);
        match self {
            MarkKind::Rect { stroke, .. }
            | MarkKind::Ellipse { stroke, .. }
            | MarkKind::Arrow { stroke, .. }
            | MarkKind::Line { stroke, .. }
            | MarkKind::Pen { stroke, .. }
                if *stroke <= 0. =>
            {
                *stroke = line
            }
            MarkKind::Text { size, .. } if *size <= 0. => *size = (side / 30.).clamp(12., 400.),
            MarkKind::Step { size, .. } if *size <= 0. => *size = (side / 22.).clamp(18., 400.),
            MarkKind::Blur {
                strength,
                width,
                height,
                ..
            } if *strength <= 0. => *strength = (width.min(*height) / 4.).clamp(6., 60.),
            MarkKind::Pixelate {
                size,
                width,
                height,
                ..
            } if *size <= 0. => *size = (width.min(*height) / 6.).clamp(6., 64.),
            _ => {}
        }
    }

    /// Clamps sizes and checks the mark can be drawn.
    pub(crate) fn check(&mut self) -> Result<()> {
        let finite = |values: &[f32]| values.iter().all(|value| value.is_finite());
        let ok = match self {
            MarkKind::Pen { points, .. } => {
                if points.len() > MAX_POINTS {
                    bail!("a pen mark has at most {MAX_POINTS} points");
                }
                points.iter().all(|p| finite(p))
            }
            MarkKind::Text {
                text, x, y, size, ..
            } => {
                if text.chars().count() > MAX_TEXT {
                    bail!("a text mark has at most {MAX_TEXT} characters");
                }
                finite(&[*x, *y, *size])
            }
            MarkKind::Step { x, y, size, .. } => finite(&[*x, *y, *size]),
            MarkKind::Arrow {
                from, to, stroke, ..
            }
            | MarkKind::Line {
                from, to, stroke, ..
            } => finite(&[from[0], from[1], to[0], to[1], *stroke]),
            _ => self.area().is_some_and(|area| area.is_finite()),
        };
        if !ok {
            bail!("a mark's position is not a number");
        }
        if let Some(area) = self.area()
            && (area.width < 0. || area.height < 0.)
        {
            self.set_area(Rect::from_corners(
                [area.x, area.y],
                [area.right(), area.bottom()],
            ));
        }
        match self {
            MarkKind::Rect { stroke, radius, .. } => {
                *stroke = stroke.clamp(0., 200.);
                *radius = radius.clamp(0., 2_000.);
            }
            MarkKind::Ellipse { stroke, .. }
            | MarkKind::Arrow { stroke, .. }
            | MarkKind::Line { stroke, .. }
            | MarkKind::Pen { stroke, .. } => *stroke = stroke.clamp(0., 200.),
            MarkKind::Text { size, .. } | MarkKind::Step { size, .. } => {
                *size = size.clamp(0., 2_000.)
            }
            MarkKind::Blur { strength, .. } => *strength = strength.clamp(0., 200.),
            MarkKind::Pixelate { size, .. } => *size = size.clamp(0., 500.),
            MarkKind::Spotlight { dim, .. } => *dim = (*dim).min(100),
            MarkKind::Highlight { .. } => {}
        }
        Ok(())
    }
}

/// Everything done to one picture. Its default changes nothing.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ImageEdit {
    pub orientation: Orientation,
    /// The part kept, in pixels of the turned original (all of it when
    /// `None`).
    pub crop: Option<Rect>,
    pub adjust: Adjust,
    pub marks: Vec<Mark>,
    /// The result's size (all of the crop at one to one when `None`).
    pub resize: Option<Size>,
    pub frame: Option<Frame>,
}

impl ImageEdit {
    pub fn is_unchanged(&self) -> bool {
        *self == ImageEdit::default()
    }

    /// The turned original's size, for an original of `size`.
    pub fn turned_size(&self, size: (u32, u32)) -> (u32, u32) {
        self.orientation.size(size.0, size.1)
    }

    /// The part kept, clamped into the turned original.
    pub fn crop_rect(&self, size: (u32, u32)) -> Rect {
        let (width, height) = self.turned_size(size);
        let all = Rect::new(0., 0., width as f32, height as f32);
        self.crop
            .map_or(all, |crop| crop.clamped(width as f32, height as f32))
    }

    /// The result's size before a frame: the resize, else the crop's.
    pub fn output_size(&self, size: (u32, u32)) -> (u32, u32) {
        match self.resize {
            Some(resize) => (resize.width, resize.height),
            None => {
                let crop = self.crop_rect(size);
                (
                    crop.width.round().max(1.) as u32,
                    crop.height.round().max(1.) as u32,
                )
            }
        }
    }

    /// The whole result's size, frame and all.
    pub fn result_size(&self, size: (u32, u32)) -> (u32, u32) {
        let (width, height) = self.output_size(size);
        let padding = self.frame.map_or(0, |frame| frame.padding);
        (width + 2 * padding, height + 2 * padding)
    }

    /// Where a point of the turned original lands in the result.
    pub fn to_result(&self, size: (u32, u32), point: [f32; 2]) -> [f32; 2] {
        let crop = self.crop_rect(size);
        let (scale_x, scale_y) = self.scale(size);
        let padding = self.frame.map_or(0., |frame| frame.padding as f32);
        [
            (point[0] - crop.x) * scale_x + padding,
            (point[1] - crop.y) * scale_y + padding,
        ]
    }

    /// Where a point of the result is on the turned original.
    pub fn from_result(&self, size: (u32, u32), point: [f32; 2]) -> [f32; 2] {
        let crop = self.crop_rect(size);
        let (scale_x, scale_y) = self.scale(size);
        let padding = self.frame.map_or(0., |frame| frame.padding as f32);
        [
            (point[0] - padding) / scale_x + crop.x,
            (point[1] - padding) / scale_y + crop.y,
        ]
    }

    /// Result pixels per pixel of the crop, across and down.
    pub fn scale(&self, size: (u32, u32)) -> (f32, f32) {
        let crop = self.crop_rect(size);
        let (width, height) = self.output_size(size);
        (width as f32 / crop.width, height as f32 / crop.height)
    }

    /// Turns the picture, keeping the crop and the marks on what they were
    /// on. `size` is the original's.
    pub fn turn(&mut self, turn: Turn, size: (u32, u32)) {
        let (width, height) = self.turned_size(size);
        let (width, height) = (width as f32, height as f32);
        if let Some(crop) = self.crop {
            let a = turn.map([crop.x, crop.y], width, height);
            let b = turn.map([crop.right(), crop.bottom()], width, height);
            self.crop = Some(Rect::from_corners(a, b));
        }
        for mark in &mut self.marks {
            mark.kind.turn(turn, width, height);
        }
        if turn.swaps_sides()
            && let Some(resize) = &mut self.resize
        {
            std::mem::swap(&mut resize.width, &mut resize.height);
        }
        self.orientation = self.orientation.then(turn);
    }

    /// Adds `kind` with its sizes made to suit a picture of `size`;
    /// returns its id.
    pub fn add_mark(&mut self, mut kind: MarkKind, size: (u32, u32)) -> Result<u32> {
        if self.marks.len() >= MAX_MARKS {
            bail!("a picture has at most {MAX_MARKS} marks");
        }
        kind.check()?;
        let (width, height) = self.turned_size(size);
        kind.fill_sizes(width.max(height) as f32);
        if let MarkKind::Step { number, .. } = &mut kind
            && *number == 0
        {
            *number = self.next_step();
        }
        let id = self.marks.iter().map(|mark| mark.id).max().unwrap_or(0) + 1;
        self.marks.push(Mark { id, kind });
        Ok(id)
    }

    /// The number the next step mark gets.
    pub fn next_step(&self) -> u32 {
        self.marks
            .iter()
            .filter_map(|mark| match mark.kind {
                MarkKind::Step { number, .. } => Some(number),
                _ => None,
            })
            .max()
            .unwrap_or(0)
            + 1
    }

    pub fn mark(&self, id: u32) -> Option<&Mark> {
        self.marks.iter().find(|mark| mark.id == id)
    }

    pub fn mark_mut(&mut self, id: u32) -> Option<&mut Mark> {
        self.marks.iter_mut().find(|mark| mark.id == id)
    }

    /// Applies an agent's changes: `changes` holds only what changes, in
    /// the shape this edit is written in (`crop`, `adjust`, `resize`,
    /// `frame`; `null` removes an optional part). Marks change with
    /// `add`, `update` (by id, only what changes) and `remove`. `size` is
    /// the original's.
    pub fn apply(&mut self, request: &Value, size: (u32, u32)) -> Result<Vec<String>> {
        let mut done = Vec::new();
        let mut next = self.clone();
        if let Some(turns) = request.get("transform") {
            let turns: Vec<Turn> = serde_json::from_value(turns.clone())
                .map_err(|err| anyhow::anyhow!("transform: {err}"))?;
            for turn in turns {
                next.turn(turn, size);
            }
            done.push("turned".into());
        }
        if let Some(changes) = request.get("set").filter(|changes| !changes.is_null()) {
            if !changes.is_object() {
                bail!("`set` is an object of the parts that change");
            }
            if changes.get("marks").is_some() || changes.get("orientation").is_some() {
                bail!("change marks with add, update and remove, and turn with transform");
            }
            // A resize may give one side: completed once the crop is known.
            let mut changes = changes.clone();
            let resize = changes
                .as_object_mut()
                .and_then(|changes| changes.remove("resize"));
            let mut value = serde_json::to_value(&next)?;
            merge(&mut value, &changes);
            next = serde_json::from_value(value).map_err(|err| anyhow::anyhow!("set: {err}"))?;
            match resize {
                Some(Value::Null) => next.resize = None,
                Some(resize) => next.resize = complete_resize(&resize, next.crop_rect(size))?,
                None => {}
            }
            done.push("changed".into());
        }
        if let Some(remove) = request.get("remove") {
            match remove {
                Value::String(all) if all == "all" => next.marks.clear(),
                _ => {
                    let ids: Vec<u32> = serde_json::from_value(remove.clone())
                        .map_err(|_| anyhow::anyhow!("remove: a list of mark ids, or \"all\""))?;
                    next.marks.retain(|mark| !ids.contains(&mark.id));
                }
            }
            done.push("removed marks".into());
        }
        if let Some(updates) = request.get("update").and_then(Value::as_array) {
            for update in updates {
                let id = update
                    .get("id")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| anyhow::anyhow!("update: each needs the mark's id"))?
                    as u32;
                let mark = next
                    .mark_mut(id)
                    .ok_or_else(|| anyhow::anyhow!("update: no mark {id}"))?;
                let mut value = serde_json::to_value(&mark.kind)?;
                merge(&mut value, update);
                if let Some(object) = value.as_object_mut() {
                    object.remove("id");
                }
                let mut kind: MarkKind = serde_json::from_value(value)
                    .map_err(|err| anyhow::anyhow!("update mark {id}: {err}"))?;
                kind.check()?;
                mark.kind = kind;
            }
            done.push("updated marks".into());
        }
        if let Some(adds) = request.get("add").and_then(Value::as_array) {
            let mut ids = Vec::new();
            for add in adds {
                let mut add = add.clone();
                if let Some(object) = add.as_object_mut() {
                    object.remove("id");
                }
                let kind: MarkKind =
                    serde_json::from_value(add).map_err(|err| anyhow::anyhow!("add: {err}"))?;
                ids.push(next.add_mark(kind, size)?.to_string());
            }
            done.push(format!("added marks {}", ids.join(", ")));
        }
        next.sanitize(size)?;
        *self = next;
        Ok(done)
    }

    /// Clamps every number to what can be drawn.
    pub fn sanitize(&mut self, size: (u32, u32)) -> Result<()> {
        self.orientation.turns %= 4;
        let (width, height) = self.turned_size(size);
        if let Some(crop) = self.crop {
            if !crop.is_finite() {
                bail!("the crop is not a number");
            }
            let crop = crop.clamped(width as f32, height as f32);
            self.crop = (crop != Rect::new(0., 0., width as f32, height as f32)).then_some(crop);
        }
        self.adjust.sanitize();
        for mark in &mut self.marks {
            mark.kind.check()?;
        }
        if let Some(resize) = &mut self.resize {
            resize.width = resize.width.clamp(1, MAX_SIDE);
            resize.height = resize.height.clamp(1, MAX_SIDE);
        }
        if let Some(frame) = &mut self.frame {
            frame.padding = frame.padding.min(2_000);
            frame.radius = frame.radius.min(2_000);
            frame.shadow = frame.shadow.min(100);
        }
        let (width, height) = self.result_size(size);
        if width > MAX_SIDE || height > MAX_SIDE || width as u64 * height as u64 > MAX_PIXELS {
            bail!(
                "the result would be {width} × {height} pixels; at most {MAX_SIDE} a side and {MAX_PIXELS} in all"
            );
        }
        Ok(())
    }
}

/// A resize given by width or height alone keeps the crop's proportions.
fn complete_resize(resize: &Value, crop: Rect) -> Result<Option<Size>> {
    let side = |key: &str| resize.get(key).and_then(Value::as_f64);
    let ratio = crop.width / crop.height;
    let (width, height) = match (side("width"), side("height")) {
        (Some(width), Some(height)) => (width, height),
        (Some(width), None) => (width, width / ratio as f64),
        (None, Some(height)) => (height * ratio as f64, height),
        (None, None) => bail!("resize: give width, height or both"),
    };
    if !(width.is_finite() && height.is_finite()) || width < 1. || height < 1. {
        bail!("resize: sizes are whole pixels, at least 1");
    }
    Ok(Some(Size {
        width: width.round() as u32,
        height: height.round() as u32,
    }))
}

/// JSON merge patch (RFC 7386): `patch`'s values replace `target`'s, objects
/// merge, `null` removes.
pub fn merge(target: &mut Value, patch: &Value) {
    let Value::Object(patch) = patch else {
        *target = patch.clone();
        return;
    };
    if !target.is_object() {
        *target = Value::Object(Default::default());
    }
    let target = target.as_object_mut().expect("made an object above");
    for (key, value) in patch {
        if value.is_null() {
            target.remove(key);
        } else {
            merge(target.entry(key.clone()).or_insert(Value::Null), value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SIZE: (u32, u32) = (400, 200);

    #[test]
    fn colors_read_and_write() {
        assert_eq!(Color::parse("#ff3b30"), Some(Color::RED));
        assert_eq!(Color::parse("#FFF"), Some(Color::WHITE));
        assert_eq!(Color::parse("#00000080"), Some(Color([0, 0, 0, 128])));
        assert_eq!(Color::parse("red"), None);
        assert_eq!(Color([0, 0, 0, 128]).hex(), "#00000080");
    }

    #[test]
    fn turning_four_times_is_where_it_started() {
        let mut edit = ImageEdit {
            crop: Some(Rect::new(10., 20., 100., 50.)),
            ..Default::default()
        };
        edit.add_mark(MarkType::Arrow.mark([5., 6.], [50., 60.], Color::RED), SIZE)
            .unwrap();
        let start = edit.clone();
        for _ in 0..4 {
            edit.turn(Turn::RotateRight, SIZE);
        }
        assert_eq!(edit, start);
        edit.turn(Turn::FlipHorizontal, SIZE);
        edit.turn(Turn::FlipHorizontal, SIZE);
        assert_eq!(edit, start);
    }

    #[test]
    fn turning_keeps_the_crop_on_the_same_part() {
        let mut edit = ImageEdit {
            crop: Some(Rect::new(0., 0., 100., 50.)),
            ..Default::default()
        };
        edit.turn(Turn::RotateRight, SIZE);
        // The top left corner is now at the top right of a 200 × 400 picture.
        assert_eq!(edit.crop, Some(Rect::new(150., 0., 50., 100.)));
        assert_eq!(edit.turned_size(SIZE), (200, 400));
        edit.turn(Turn::FlipVertical, SIZE);
        assert_eq!(
            edit.orientation,
            Orientation {
                turns: 1,
                flipped: true
            }
            .then(Turn::RotateRight)
            .then(Turn::RotateLeft)
        );
    }

    #[test]
    fn flipping_composes_as_seen() {
        // Flipping up and down after a quarter turn equals a quarter turn
        // the other way after a flip left to right.
        let o = Orientation::default()
            .then(Turn::RotateRight)
            .then(Turn::FlipVertical);
        assert_eq!(
            o,
            Orientation {
                turns: 1,
                flipped: true
            }
        );
    }

    #[test]
    fn an_agent_changes_only_what_it_names() {
        let mut edit = ImageEdit::default();
        edit.apply(
            &json!({ "set": { "adjust": { "brightness": 20, "grayscale": true }, "resize": { "width": 200 } } }),
            SIZE,
        )
        .unwrap();
        assert_eq!(edit.adjust.brightness, 20.);
        assert!(edit.adjust.grayscale);
        assert_eq!(
            edit.resize,
            Some(Size {
                width: 200,
                height: 100
            })
        );
        edit.apply(&json!({ "set": { "adjust": { "contrast": 500 } } }), SIZE)
            .unwrap();
        assert_eq!(edit.adjust.brightness, 20.);
        assert_eq!(edit.adjust.contrast, 100.);
        edit.apply(&json!({ "set": { "resize": null } }), SIZE)
            .unwrap();
        assert_eq!(edit.resize, None);
        assert!(
            edit.apply(&json!({ "set": { "adjust": { "brightnes": 1 } } }), SIZE)
                .is_err()
        );
    }

    #[test]
    fn marks_are_added_updated_and_removed() {
        let mut edit = ImageEdit::default();
        edit.apply(
            &json!({ "add": [
                { "type": "rect", "x": 10, "y": 10, "width": 50, "height": 20 },
                { "type": "step", "x": 100, "y": 100 },
                { "type": "text", "x": 5, "y": 5, "text": "Login", "color": "#ffffff" }
            ] }),
            SIZE,
        )
        .unwrap();
        assert_eq!(edit.marks.len(), 3);
        let MarkKind::Rect { stroke, .. } = edit.marks[0].kind else {
            panic!()
        };
        assert!(stroke >= 2., "a size made to suit the picture");
        let MarkKind::Step { number, .. } = edit.marks[1].kind else {
            panic!()
        };
        assert_eq!(number, 1);
        edit.apply(&json!({ "update": [{ "id": 3, "text": "Sign in" }] }), SIZE)
            .unwrap();
        let MarkKind::Text { text, color, .. } = &edit.marks[2].kind else {
            panic!()
        };
        assert_eq!((text.as_str(), *color), ("Sign in", Color::WHITE));
        edit.apply(&json!({ "remove": [1] }), SIZE).unwrap();
        assert_eq!(edit.marks.iter().map(|m| m.id).collect::<Vec<_>>(), [2, 3]);
        edit.apply(&json!({ "remove": "all" }), SIZE).unwrap();
        assert!(edit.marks.is_empty());
    }

    #[test]
    fn a_failed_change_changes_nothing() {
        let mut edit = ImageEdit::default();
        let before = edit.clone();
        let result = edit.apply(
            &json!({ "set": { "adjust": { "brightness": 10 } }, "add": [{ "type": "circle" }] }),
            SIZE,
        );
        assert!(result.is_err());
        assert_eq!(edit, before);
        assert!(
            edit.apply(
                &json!({ "set": { "resize": { "width": 99999, "height": 99999 } } }),
                SIZE
            )
            .is_err()
        );
    }

    #[test]
    fn result_positions_go_both_ways() {
        let edit = ImageEdit {
            crop: Some(Rect::new(100., 50., 200., 100.)),
            resize: Some(Size {
                width: 100,
                height: 50,
            }),
            frame: Some(Frame {
                padding: 10,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(edit.result_size(SIZE), (120, 70));
        let point = [150., 75.];
        let there = edit.to_result(SIZE, point);
        assert_eq!(there, [35., 22.5]);
        assert_eq!(edit.from_result(SIZE, there), point);
    }
}

//! A presentation as PilotHouse keeps it: slides of positioned elements
//! (shapes, text, pictures, tables, charts, code, icons, SVG, groups) in a
//! theme of colours, fonts and layouts. It maps onto PowerPoint's own model,
//! so it exports to editable PPTX, and it is what agents edit (as JSON).
//!
//! Lengths are points (1/72 inch): a 16:9 slide is 960 × 540. Colours are
//! theme slots (`accent1`, made lighter with `accent1+40` or darker with
//! `accent1-25`) or `#RRGGBB[AA]`, so a new theme recolours the deck.
//! Unset text properties come from the placeholder's style, then the
//! theme's.

use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// The deck format's version, for reading older files.
pub const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Deck {
    pub version: u32,
    pub title: String,
    pub size: SlideSize,
    pub theme: Theme,
    /// Shared, so a copy of the deck (undo steps, drawing off the UI
    /// thread) shares the slides it did not change.
    pub slides: Vec<Arc<Slide>>,
    /// Named groups of slides: each starts at its slide.
    pub sections: Vec<Section>,
    /// The next number for a slide or element id.
    pub next_id: u32,
    /// The PowerPoint file it was opened from, when it was.
    pub source: Option<Source>,
}

impl Default for Deck {
    fn default() -> Self {
        Deck {
            version: VERSION,
            title: String::new(),
            size: SlideSize::default(),
            theme: Theme::default(),
            slides: Vec::new(),
            sections: Vec::new(),
            next_id: 1,
            source: None,
        }
    }
}

/// A deck opened from a PowerPoint file: a copy of the file in the space
/// (its masters, layouts and theme are written back as they were while the
/// theme is the one read), and where it came from (to save back to).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Source {
    /// The copy's name in the space's folder.
    pub file: String,
    /// The file it was opened from.
    pub original: Option<std::path::PathBuf>,
    /// The theme as read, so a theme changed since is written anew.
    pub theme: Option<Box<Theme>>,
    /// When the original was last changed (seconds since 1970) as this
    /// deck knows it: opening it again reopens this deck while it is so.
    pub stamp: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Section {
    pub name: String,
    /// The id of the slide it starts at.
    pub slide: String,
}

/// The slides' size, points.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SlideSize {
    pub width: f32,
    pub height: f32,
    /// Which of [`SIZES`] it is, if one.
    pub preset: Option<String>,
}

impl Default for SlideSize {
    fn default() -> Self {
        SlideSize {
            width: 960.,
            height: 540.,
            preset: Some("16:9".into()),
        }
    }
}

/// The sizes offered: id, name, width and height in points.
pub const SIZES: &[(&str, &str, f32, f32)] = &[
    ("16:9", "Widescreen 16:9", 960., 540.),
    ("16:10", "16:10", 960., 600.),
    ("4:3", "Standard 4:3", 720., 540.),
    ("a4", "A4 landscape", 841.89, 595.28),
    ("a4-portrait", "A4 portrait", 595.28, 841.89),
    ("letter", "US Letter landscape", 792., 612.),
    ("letter-portrait", "US Letter portrait", 612., 792.),
    ("1:1", "Square 1:1", 720., 720.),
    ("4:5", "Portrait 4:5", 720., 900.),
    ("9:16", "Vertical 9:16", 540., 960.),
];

impl SlideSize {
    pub fn preset(id: &str) -> Option<SlideSize> {
        SIZES
            .iter()
            .find(|(preset, ..)| *preset == id)
            .map(|(preset, _, width, height)| SlideSize {
                width: *width,
                height: *height,
                preset: Some(preset.to_string()),
            })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Slide {
    pub id: String,
    /// The id of its layout in the theme.
    pub layout: String,
    /// Over the layout's and the theme's.
    pub background: Option<Fill>,
    pub elements: Vec<Element>,
    pub notes: String,
    pub hidden: bool,
    /// How the slide comes in when presenting.
    pub transition: Transition,
    /// Where it comes from (push, wipe, cover).
    pub transition_from: Direction,
    /// Seconds.
    pub transition_seconds: f32,
    /// Elements coming in one after another when presenting, in order.
    pub animations: Vec<Animation>,
    pub comments: Vec<Comment>,
}

impl Default for Slide {
    fn default() -> Self {
        Slide {
            id: String::new(),
            layout: "blank".into(),
            background: None,
            elements: Vec::new(),
            notes: String::new(),
            hidden: false,
            transition: Transition::None,
            transition_from: Direction::Right,
            transition_seconds: 0.5,
            animations: Vec::new(),
            comments: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transition {
    #[default]
    None,
    Fade,
    /// The new slide pushes the old out.
    Push,
    /// The new slide is uncovered across the old.
    Wipe,
    /// The new slide slides in over the old.
    Cover,
    /// The new slide grows in from the middle.
    Zoom,
    Dissolve,
}

impl Transition {
    pub const ALL: [(Transition, &'static str); 7] = [
        (Transition::None, "None"),
        (Transition::Fade, "Fade"),
        (Transition::Push, "Push"),
        (Transition::Wipe, "Wipe"),
        (Transition::Cover, "Cover"),
        (Transition::Zoom, "Zoom"),
        (Transition::Dissolve, "Dissolve"),
    ];

    pub fn label(self) -> &'static str {
        Transition::ALL
            .iter()
            .find(|(kind, _)| *kind == self)
            .map_or("None", |(_, name)| name)
    }

    /// Whether it comes from a side.
    pub fn has_direction(self) -> bool {
        matches!(
            self,
            Transition::Push | Transition::Wipe | Transition::Cover
        )
    }
}

/// The side something comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Left,
    #[default]
    Right,
    Top,
    Bottom,
}

impl Direction {
    pub const ALL: [(Direction, &'static str); 4] = [
        (Direction::Left, "From the left"),
        (Direction::Right, "From the right"),
        (Direction::Top, "From the top"),
        (Direction::Bottom, "From the bottom"),
    ];
}

/// An element's entrance when presenting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Animation {
    /// The element (top level on the slide).
    pub id: String,
    pub effect: Effect,
    /// Fly in and wipe: from where.
    pub from: Direction,
    pub start: Start,
    /// Seconds.
    pub seconds: f32,
    /// Seconds after it would start.
    pub delay: f32,
}

impl Default for Animation {
    fn default() -> Self {
        Animation {
            id: String::new(),
            effect: Effect::Fade,
            from: Direction::Bottom,
            start: Start::OnClick,
            seconds: 0.5,
            delay: 0.,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    /// At once.
    Appear,
    #[default]
    Fade,
    FlyIn,
    Zoom,
    Wipe,
}

impl Effect {
    pub const ALL: [(Effect, &'static str); 5] = [
        (Effect::Appear, "Appear"),
        (Effect::Fade, "Fade in"),
        (Effect::FlyIn, "Fly in"),
        (Effect::Zoom, "Zoom in"),
        (Effect::Wipe, "Wipe in"),
    ];

    pub fn label(self) -> &'static str {
        Effect::ALL
            .iter()
            .find(|(kind, _)| *kind == self)
            .map_or("Fade in", |(_, name)| name)
    }
}

/// When an animation starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Start {
    #[default]
    OnClick,
    WithPrevious,
    AfterPrevious,
}

/// A note left on a slide, at a place or on an element.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Comment {
    pub id: String,
    pub author: String,
    pub text: String,
    /// Where its marker is on the slide (points).
    pub x: f32,
    pub y: f32,
    /// The element it is about, if one.
    pub element: Option<String>,
    pub resolved: bool,
    /// Seconds since 1970.
    pub at: u64,
    pub replies: Vec<Reply>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Reply {
    pub author: String,
    pub text: String,
    pub at: u64,
}

/// Where an element is: its box, turned about its centre, then flipped.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Frame {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Degrees, clockwise.
    pub rotation: f32,
    pub flip_h: bool,
    pub flip_v: bool,
}

impl Frame {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Frame {
        Frame {
            x,
            y,
            w,
            h,
            ..Frame::default()
        }
    }

    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2., self.y + self.h / 2.)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Element {
    pub id: String,
    /// What it is called in lists (and PowerPoint's selection pane).
    pub name: String,
    pub frame: Frame,
    /// The layout's placeholder it fills, which gives it its place and
    /// text style.
    pub placeholder: Option<PlaceholderKind>,
    /// 0 (clear) to 1.
    pub opacity: f32,
    pub kind: Kind,
}

impl Default for Element {
    fn default() -> Self {
        Element {
            id: String::new(),
            name: String::new(),
            frame: Frame::default(),
            placeholder: None,
            opacity: 1.,
            kind: Kind::Text(TextBox::default()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Kind {
    Shape(Shape),
    Text(TextBox),
    Picture(Picture),
    Table(Table),
    Chart(Chart),
    Code(Code),
    Icon(Icon),
    Svg(SvgArt),
    Group(Group),
    /// Something from a PowerPoint file PilotHouse does not edit (a video,
    /// an embedded object, ink), kept as it was and written back.
    Raw(Raw),
}

impl Kind {
    pub fn label(&self) -> &'static str {
        match self {
            Kind::Shape(_) => "Shape",
            Kind::Text(_) => "Text",
            Kind::Picture(_) => "Picture",
            Kind::Table(_) => "Table",
            Kind::Chart(_) => "Chart",
            Kind::Code(_) => "Code",
            Kind::Icon(_) => "Icon",
            Kind::Svg(_) => "Graphic",
            Kind::Group(_) => "Group",
            Kind::Raw(_) => "PowerPoint object",
        }
    }

    /// The text it holds, when it holds text in one body.
    pub fn text(&self) -> Option<&TextBody> {
        match self {
            Kind::Shape(shape) => shape.text.as_ref(),
            Kind::Text(text) => Some(&text.text),
            _ => None,
        }
    }

    pub fn text_mut(&mut self) -> Option<&mut TextBody> {
        match self {
            Kind::Shape(shape) => Some(shape.text.get_or_insert_with(TextBody::default)),
            Kind::Text(text) => Some(&mut text.text),
            _ => None,
        }
    }
}

/// A shape: a preset outline (or a path of its own), filled and stroked,
/// with text inside if it has any.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shape {
    pub geometry: Geometry,
    /// `rect` corners rounded this much (points), `round_rect`'s default.
    pub radius: Option<f32>,
    /// For `path`: SVG path data in the frame's own box (0,0 to w,h).
    pub path: Option<String>,
    pub fill: Fill,
    pub line: Line,
    pub shadow: Option<Shadow>,
    pub text: Option<TextBody>,
    /// For lines: the elements their ends are fixed to, so they follow
    /// when those move.
    pub connect: Option<Connect>,
    /// From a PowerPoint file: its preset outline there and its
    /// adjustments, written back while `geometry` is still the one read
    /// (PilotHouse draws the nearest it has).
    pub preset: Option<(String, Vec<(String, String)>)>,
}

/// A line's ends fixed to elements: each to an element's side (0 top,
/// 1 right, 2 bottom, 3 left, at its middle).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Connect {
    pub start: Option<(String, u8)>,
    pub end: Option<(String, u8)>,
}

impl Default for Shape {
    fn default() -> Self {
        Shape {
            geometry: Geometry::Rect,
            radius: None,
            path: None,
            fill: Fill::Solid {
                color: Color::from("accent1"),
                opacity: 1.,
            },
            line: Line::default(),
            shadow: None,
            text: None,
            connect: None,
            preset: None,
        }
    }
}

/// The preset outlines: PowerPoint's names (`prstGeom`) in snake case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Geometry {
    #[default]
    Rect,
    RoundRect,
    Ellipse,
    Triangle,
    RightTriangle,
    Diamond,
    Parallelogram,
    Trapezoid,
    Pentagon,
    Hexagon,
    Octagon,
    Chevron,
    HomePlate,
    RightArrow,
    LeftArrow,
    UpArrow,
    DownArrow,
    LeftRightArrow,
    Star5,
    Star6,
    Plus,
    Heart,
    /// A speech bubble with its tail at the bottom left.
    Callout,
    /// A straight line from the frame's top left to its bottom right
    /// (flips choose other corners).
    Line,
    /// `Shape::path`.
    Path,
}

impl Geometry {
    pub const ALL: [Geometry; 24] = [
        Geometry::Rect,
        Geometry::RoundRect,
        Geometry::Ellipse,
        Geometry::Triangle,
        Geometry::RightTriangle,
        Geometry::Diamond,
        Geometry::Parallelogram,
        Geometry::Trapezoid,
        Geometry::Pentagon,
        Geometry::Hexagon,
        Geometry::Octagon,
        Geometry::Chevron,
        Geometry::HomePlate,
        Geometry::RightArrow,
        Geometry::LeftArrow,
        Geometry::UpArrow,
        Geometry::DownArrow,
        Geometry::LeftRightArrow,
        Geometry::Star5,
        Geometry::Star6,
        Geometry::Plus,
        Geometry::Heart,
        Geometry::Callout,
        Geometry::Line,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Geometry::Rect => "Rectangle",
            Geometry::RoundRect => "Rounded rectangle",
            Geometry::Ellipse => "Ellipse",
            Geometry::Triangle => "Triangle",
            Geometry::RightTriangle => "Right triangle",
            Geometry::Diamond => "Diamond",
            Geometry::Parallelogram => "Parallelogram",
            Geometry::Trapezoid => "Trapezoid",
            Geometry::Pentagon => "Pentagon",
            Geometry::Hexagon => "Hexagon",
            Geometry::Octagon => "Octagon",
            Geometry::Chevron => "Chevron",
            Geometry::HomePlate => "Arrow pentagon",
            Geometry::RightArrow => "Right arrow",
            Geometry::LeftArrow => "Left arrow",
            Geometry::UpArrow => "Up arrow",
            Geometry::DownArrow => "Down arrow",
            Geometry::LeftRightArrow => "Double arrow",
            Geometry::Star5 => "Star",
            Geometry::Star6 => "Six-point star",
            Geometry::Plus => "Plus",
            Geometry::Heart => "Heart",
            Geometry::Callout => "Callout",
            Geometry::Line => "Line",
            Geometry::Path => "Freeform",
        }
    }
}

/// A text box: text, with a fill and outline when it has them.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TextBox {
    pub text: TextBody,
    pub fill: Fill,
    pub line: Line,
}

/// Text as paragraphs; given as a string, it is read as light Markdown
/// (see `markdown`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, from = "TextForm")]
pub struct TextBody {
    pub paragraphs: Vec<Paragraph>,
    pub anchor: Anchor,
    /// Inner margins: left, top, right, bottom.
    pub insets: [f32; 4],
    pub autofit: Autofit,
    /// Lines wrap at the box's width (else they run on).
    pub wrap: bool,
}

impl Default for TextBody {
    fn default() -> Self {
        TextBody {
            paragraphs: Vec::new(),
            anchor: Anchor::Top,
            insets: [7.2, 3.6, 7.2, 3.6],
            autofit: Autofit::None,
            wrap: true,
        }
    }
}

/// How a text body may be written: Markdown, or in full.
#[derive(Deserialize)]
#[serde(untagged)]
enum TextForm {
    Markdown(String),
    Full(TextFields),
}

#[derive(Deserialize)]
#[serde(default)]
struct TextFields {
    paragraphs: Vec<Paragraph>,
    anchor: Anchor,
    insets: [f32; 4],
    autofit: Autofit,
    wrap: bool,
}

impl Default for TextFields {
    fn default() -> Self {
        let body = TextBody::default();
        TextFields {
            paragraphs: body.paragraphs,
            anchor: body.anchor,
            insets: body.insets,
            autofit: body.autofit,
            wrap: body.wrap,
        }
    }
}

impl From<TextForm> for TextBody {
    fn from(form: TextForm) -> Self {
        match form {
            TextForm::Markdown(text) => super::markdown::to_text(&text),
            TextForm::Full(fields) => TextBody {
                paragraphs: fields.paragraphs,
                anchor: fields.anchor,
                insets: fields.insets,
                autofit: fields.autofit,
                wrap: fields.wrap,
            },
        }
    }
}

impl TextBody {
    /// Its text, paragraphs on lines of their own.
    pub fn plain(&self) -> String {
        self.paragraphs
            .iter()
            .map(Paragraph::plain)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// A body of one plain paragraph per line of `text`.
    pub fn from_plain(text: &str) -> TextBody {
        TextBody {
            paragraphs: text.split('\n').map(Paragraph::plain_text).collect(),
            ..TextBody::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    #[default]
    Top,
    Middle,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Autofit {
    /// Text runs out of the box when there is too much.
    #[default]
    None,
    /// Text gets smaller to fit.
    Shrink,
    /// The box grows to fit.
    Grow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Paragraph {
    pub runs: Vec<Run>,
    pub align: Option<Align>,
    /// Outline level: 0 is the top, each one more indented.
    pub level: u8,
    pub bullet: Option<Bullet>,
    /// Points before and after the paragraph.
    pub space_before: Option<f32>,
    pub space_after: Option<f32>,
    /// Line height as a multiple of the font's (1 is single).
    pub line_spacing: Option<f32>,
}

impl Paragraph {
    pub fn plain(&self) -> String {
        self.runs.iter().map(|run| run.text.as_str()).collect()
    }

    pub fn plain_text(text: &str) -> Paragraph {
        Paragraph {
            runs: vec![Run {
                text: text.to_string(),
                ..Run::default()
            }],
            ..Paragraph::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Bullet {
    /// No bullet, even where the style has one.
    None,
    /// `•`, `–`, `→` …
    Char(String),
    /// 1. 2. 3.
    Number,
}

/// A stretch of text in one style; unset properties are inherited.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Run {
    pub text: String,
    /// A font family, or `heading`, `body` or `code` for the theme's.
    pub font: Option<String>,
    /// Points.
    pub size: Option<f32>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strike: Option<bool>,
    pub color: Option<Color>,
    pub highlight: Option<Color>,
    pub link: Option<String>,
    /// Superscript (above 0) or subscript (below), percent of the size.
    pub baseline: Option<i16>,
}

impl Run {
    /// Its style without its text, to compare runs by.
    pub fn same_style(&self, other: &Run) -> bool {
        Run {
            text: String::new(),
            ..self.clone()
        } == Run {
            text: String::new(),
            ..other.clone()
        }
    }
}

/// A colour: a theme slot (`accent1`, `dark1`, `light1` … with `+N` lighter
/// or `-N` darker by N percent) or `#RRGGBB` / `#RRGGBBAA`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Color(pub String);

impl From<&str> for Color {
    fn from(text: &str) -> Self {
        Color(text.to_string())
    }
}

/// A colour as its parts, resolved against a theme.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const BLACK: Rgba = Rgba::opaque(0, 0, 0);
    pub const WHITE: Rgba = Rgba::opaque(255, 255, 255);

    pub const fn opaque(r: u8, g: u8, b: u8) -> Rgba {
        Rgba { r, g, b, a: 255 }
    }

    pub fn hex(&self) -> String {
        format!("{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    pub fn parse_hex(text: &str) -> Option<Rgba> {
        let hex = text.trim().strip_prefix('#').unwrap_or(text.trim());
        let byte = |at: usize| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok();
        match hex.len() {
            6 => Some(Rgba::opaque(byte(0)?, byte(2)?, byte(4)?)),
            8 => Some(Rgba {
                r: byte(0)?,
                g: byte(2)?,
                b: byte(4)?,
                a: byte(6)?,
            }),
            3 => {
                let nibble = |at: usize| {
                    u8::from_str_radix(hex.get(at..at + 1)?, 16)
                        .ok()
                        .map(|value| value * 17)
                };
                Some(Rgba::opaque(nibble(0)?, nibble(1)?, nibble(2)?))
            }
            _ => None,
        }
    }

    /// Lighter (towards white, `amount` above 0) or darker (below 0), by
    /// that fraction.
    pub fn shade(self, amount: f32) -> Rgba {
        let mix = |channel: u8| {
            let channel = channel as f32;
            let value = if amount >= 0. {
                channel + (255. - channel) * amount.min(1.)
            } else {
                channel * (1. + amount.max(-1.))
            };
            value.round().clamp(0., 255.) as u8
        };
        Rgba {
            r: mix(self.r),
            g: mix(self.g),
            b: mix(self.b),
            a: self.a,
        }
    }

    /// Relative luminance, 0 (black) to 1 (white).
    pub fn luminance(&self) -> f32 {
        let linear = |channel: u8| {
            let value = channel as f32 / 255.;
            if value <= 0.03928 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(self.r) + 0.7152 * linear(self.g) + 0.0722 * linear(self.b)
    }
}

/// A theme slot and how much lighter or darker, from `accent1+40`.
pub fn parse_slot(text: &str) -> Option<(&str, f32)> {
    let text = text.trim();
    let split = text.find(['+', '-']).unwrap_or(text.len());
    let (slot, amount) = text.split_at(split);
    let amount = if amount.is_empty() {
        0.
    } else {
        amount.parse::<f32>().ok()? / 100.
    };
    SLOTS.contains(&slot).then_some((slot, amount))
}

/// The theme's colour slots.
pub const SLOTS: [&str; 12] = [
    "dark1",
    "light1",
    "dark2",
    "light2",
    "accent1",
    "accent2",
    "accent3",
    "accent4",
    "accent5",
    "accent6",
    "link",
    "followed_link",
];

/// A fill; given as a string, a solid colour.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", try_from = "FillForm")]
pub enum Fill {
    #[default]
    None,
    Solid {
        color: Color,
        #[serde(default = "one")]
        opacity: f32,
    },
    /// Stops from 0 to 1; linear at `angle` degrees (0: left to right,
    /// 90: top to bottom), or radial from the centre.
    Gradient {
        stops: Vec<Stop>,
        #[serde(default)]
        angle: f32,
        #[serde(default)]
        radial: bool,
    },
    Picture {
        media: String,
        #[serde(default)]
        fit: Fit,
    },
}

fn one() -> f32 {
    1.
}

#[derive(Deserialize)]
#[serde(untagged)]
enum FillForm {
    Color(String),
    Full(FillFull),
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum FillFull {
    None,
    Solid {
        color: Color,
        #[serde(default = "one")]
        opacity: f32,
    },
    Gradient {
        stops: Vec<Stop>,
        #[serde(default)]
        angle: f32,
        #[serde(default)]
        radial: bool,
    },
    Picture {
        media: String,
        #[serde(default)]
        fit: Fit,
    },
}

impl TryFrom<FillForm> for Fill {
    type Error = String;

    fn try_from(form: FillForm) -> Result<Self, String> {
        Ok(match form {
            FillForm::Color(color) if color == "none" => Fill::None,
            FillForm::Color(color) => Fill::Solid {
                color: Color(color),
                opacity: 1.,
            },
            FillForm::Full(FillFull::None) => Fill::None,
            FillForm::Full(FillFull::Solid { color, opacity }) => Fill::Solid { color, opacity },
            FillForm::Full(FillFull::Gradient {
                stops,
                angle,
                radial,
            }) => Fill::Gradient {
                stops,
                angle,
                radial,
            },
            FillForm::Full(FillFull::Picture { media, fit }) => Fill::Picture { media, fit },
        })
    }
}

impl Fill {
    pub fn solid(color: &str) -> Fill {
        Fill::Solid {
            color: Color::from(color),
            opacity: 1.,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stop {
    pub at: f32,
    pub color: Color,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Line {
    pub color: Option<Color>,
    /// Points; 0 is no line.
    pub width: f32,
    pub dash: Dash,
    /// Arrowheads at the start and end (lines).
    pub head: bool,
    pub tail: bool,
}

impl Default for Line {
    fn default() -> Self {
        Line {
            color: None,
            width: 0.,
            dash: Dash::Solid,
            head: false,
            tail: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dash {
    #[default]
    Solid,
    Dash,
    Dot,
    DashDot,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shadow {
    pub color: Color,
    /// Points.
    pub blur: f32,
    pub distance: f32,
    /// Degrees: the direction the shadow falls (90: straight down).
    pub angle: f32,
    pub opacity: f32,
}

impl Default for Shadow {
    fn default() -> Self {
        Shadow {
            color: Color::from("#000000"),
            blur: 12.,
            distance: 4.,
            angle: 90.,
            opacity: 0.25,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fit {
    /// Filling the frame, cut where it overflows.
    #[default]
    Cover,
    /// All of it inside the frame.
    Contain,
    /// Stretched to the frame.
    Stretch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Picture {
    /// A file in the space's `media` folder.
    pub media: String,
    pub fit: Fit,
    /// Fractions cut off each side: left, top, right, bottom.
    pub crop: [f32; 4],
    pub radius: f32,
    pub line: Line,
    pub shadow: Option<Shadow>,
    /// What it shows, for screen readers.
    pub alt: String,
}

impl Default for Picture {
    fn default() -> Self {
        Picture {
            media: String::new(),
            fit: Fit::Cover,
            crop: [0.; 4],
            radius: 0.,
            line: Line::default(),
            shadow: None,
            alt: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Table {
    /// Relative column widths.
    pub columns: Vec<f32>,
    pub rows: Vec<Vec<Cell>>,
    /// The first row styled as a header.
    pub header: bool,
    /// Every other row tinted.
    pub banded: bool,
    pub border: Color,
    /// The header's fill (the theme's first accent when unset).
    pub accent: Option<Color>,
    /// Points.
    pub font_size: f32,
}

impl Default for Table {
    fn default() -> Self {
        Table {
            columns: Vec::new(),
            rows: Vec::new(),
            header: true,
            banded: true,
            border: Color::from("dark1+80"),
            accent: None,
            font_size: 14.,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Cell {
    pub text: TextBody,
    pub fill: Option<Fill>,
    /// Columns and rows it spans (1 is itself); cells it covers are empty.
    pub span: Option<(u16, u16)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Chart {
    pub chart: ChartKind,
    pub title: String,
    pub categories: Vec<String>,
    pub series: Vec<Series>,
    pub legend: bool,
    /// Values written on the bars, points or slices.
    pub labels: bool,
    pub x_title: String,
    pub y_title: String,
    pub font_size: f32,
}

impl Default for Chart {
    fn default() -> Self {
        Chart {
            chart: ChartKind::Column,
            title: String::new(),
            categories: Vec::new(),
            series: Vec::new(),
            legend: true,
            labels: false,
            x_title: String::new(),
            y_title: String::new(),
            font_size: 12.,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChartKind {
    #[default]
    Column,
    Bar,
    StackedColumn,
    Line,
    Area,
    Pie,
    Doughnut,
    Scatter,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Series {
    pub name: String,
    pub values: Vec<f64>,
    /// For scatter charts: each point's x (else the category's index).
    pub x: Vec<f64>,
    pub color: Option<Color>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Code {
    pub language: String,
    pub code: String,
    pub font_size: f32,
    /// Light text on dark (else dark on light).
    pub dark: bool,
    pub line_numbers: bool,
}

impl Default for Code {
    fn default() -> Self {
        Code {
            language: String::new(),
            code: String::new(),
            font_size: 14.,
            dark: true,
            line_numbers: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Icon {
    /// A Lucide icon's name (`rocket`, `chart-line`, `shield-check` …).
    pub icon: String,
    pub color: Color,
    /// The stroke, in the icon's 24-unit grid.
    pub stroke: f32,
}

impl Default for Icon {
    fn default() -> Self {
        Icon {
            icon: String::new(),
            color: Color::from("accent1"),
            stroke: 2.,
        }
    }
}

/// A graphic drawn as SVG: anything shapes cannot say.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SvgArt {
    /// A whole SVG document; its viewBox is fitted to the frame.
    pub source: String,
}

/// A PowerPoint object kept as its XML, with the files it uses.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Raw {
    /// The element's XML as it was in the slide (`p:pic`, `p:graphicFrame`
    /// …), its namespaces declared on it.
    pub xml: String,
    /// The relationships it names: id in the XML, type, and the file
    /// (in the space's `media`) or the link it points to.
    pub rels: Vec<RawRel>,
    /// A picture of it (in `media`) to show, when the file had one.
    pub preview: Option<String>,
    /// What it is, in words (`Video`, `Embedded object` …).
    pub what: String,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RawRel {
    pub id: String,
    pub kind: String,
    /// A file in `media`, or (external) a link.
    pub target: String,
    pub external: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Group {
    /// Positioned on the slide like any element.
    pub children: Vec<Element>,
}

/// A deck's look: colours, fonts, the slide background and the layouts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Theme {
    pub name: String,
    /// `dark1`, `light1`, `dark2`, `light2`, `accent1`…`accent6`, `link`,
    /// `followed_link` as `#RRGGBB`.
    pub colors: Vec<(String, String)>,
    pub fonts: Fonts,
    pub background: Fill,
    /// Drawn on every slide under its own (a logo, a footer bar).
    pub decorations: Vec<Element>,
    pub title: TextStyle,
    pub body: TextStyle,
    pub layouts: Vec<Layout>,
}

impl Default for Theme {
    fn default() -> Self {
        super::theme::builtin("clean").expect("the clean theme")
    }
}

impl Theme {
    /// A slot's colour.
    pub fn slot(&self, slot: &str) -> Option<Rgba> {
        self.colors
            .iter()
            .find(|(name, _)| name == slot)
            .and_then(|(_, hex)| Rgba::parse_hex(hex))
    }

    /// `color` as RGBA (black when it cannot be read).
    pub fn resolve(&self, color: &Color) -> Rgba {
        let text = color.0.trim();
        if text.starts_with('#') {
            return Rgba::parse_hex(text).unwrap_or(Rgba::BLACK);
        }
        match parse_slot(text) {
            Some((slot, amount)) => self
                .slot(slot)
                .map(|rgba| rgba.shade(amount))
                .unwrap_or(Rgba::BLACK),
            None => Rgba::parse_hex(text).unwrap_or(Rgba::BLACK),
        }
    }

    pub fn layout(&self, id: &str) -> Option<&Layout> {
        self.layouts.iter().find(|layout| layout.id == id)
    }

    /// A font name with the theme's names (`heading`, `body`, `code`) put
    /// in.
    pub fn font<'a>(&'a self, name: &'a str) -> &'a str {
        match name {
            "heading" => &self.fonts.heading,
            "body" => &self.fonts.body,
            "code" => &self.fonts.code,
            other => other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Fonts {
    pub heading: String,
    pub body: String,
    pub code: String,
}

impl Default for Fonts {
    fn default() -> Self {
        Fonts {
            heading: "Arial".into(),
            body: "Arial".into(),
            code: "Courier New".into(),
        }
    }
}

/// A text style: what a run that sets nothing looks like.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextStyle {
    pub font: String,
    pub size: f32,
    pub color: Color,
    pub bold: bool,
    pub italic: bool,
    pub align: Align,
    pub line_spacing: f32,
    /// Points after each paragraph.
    pub space_after: f32,
    /// The bullet on each level (body text).
    pub bullets: Vec<Bullet>,
}

impl Default for TextStyle {
    fn default() -> Self {
        TextStyle {
            font: "body".into(),
            size: 18.,
            color: Color::from("dark1"),
            bold: false,
            italic: false,
            align: Align::Left,
            line_spacing: 1.1,
            space_after: 6.,
            bullets: Vec::new(),
        }
    }
}

/// A slide layout: where its placeholders go, how their text looks, and
/// what is drawn behind them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    pub id: String,
    pub name: String,
    pub background: Option<Fill>,
    pub decorations: Vec<Element>,
    pub placeholders: Vec<Placeholder>,
    /// The theme's decorations are drawn under its own.
    pub theme_decorations: bool,
    /// From a PowerPoint file: its part there (`ppt/slideLayouts/…`).
    pub part: Option<String>,
}

impl Default for Layout {
    fn default() -> Self {
        Layout {
            id: "blank".into(),
            name: "Blank".into(),
            background: None,
            decorations: Vec::new(),
            placeholders: Vec::new(),
            theme_decorations: true,
            part: None,
        }
    }
}

impl Layout {
    pub fn placeholder(&self, kind: PlaceholderKind) -> Option<&Placeholder> {
        self.placeholders.iter().find(|place| place.kind == kind)
    }
}

/// What a placeholder holds; slides' elements fill them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaceholderKind {
    Title,
    Subtitle,
    Body,
    /// The second body of a two-column layout.
    Body2,
    /// The headings over a comparison's bodies.
    Heading,
    Heading2,
    Picture,
    /// A big number or statement.
    Statement,
    Caption,
    Footer,
    SlideNumber,
}

impl PlaceholderKind {
    pub fn label(self) -> &'static str {
        match self {
            PlaceholderKind::Title => "Title",
            PlaceholderKind::Subtitle => "Subtitle",
            PlaceholderKind::Body | PlaceholderKind::Body2 => "Text",
            PlaceholderKind::Heading | PlaceholderKind::Heading2 => "Heading",
            PlaceholderKind::Picture => "Picture",
            PlaceholderKind::Statement => "Statement",
            PlaceholderKind::Caption => "Caption",
            PlaceholderKind::Footer => "Footer",
            PlaceholderKind::SlideNumber => "Slide number",
        }
    }

    /// Whether its text follows the theme's title style (else the body's).
    pub fn is_title(self) -> bool {
        matches!(
            self,
            PlaceholderKind::Title
                | PlaceholderKind::Statement
                | PlaceholderKind::Heading
                | PlaceholderKind::Heading2
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Placeholder {
    pub kind: PlaceholderKind,
    pub frame: Frame,
    /// What it says before anything is typed in.
    pub prompt: String,
    /// Over the theme's style for its kind.
    pub size: Option<f32>,
    pub color: Option<Color>,
    pub bold: Option<bool>,
    pub align: Option<Align>,
    pub anchor: Anchor,
    pub autofit: Autofit,
    /// Body text: bullets on (the theme's per level).
    pub bullets: bool,
    /// From a PowerPoint file: the placeholder's type and index there.
    pub ph: Option<(String, u32)>,
}

impl Default for Placeholder {
    fn default() -> Self {
        Placeholder {
            kind: PlaceholderKind::Body,
            frame: Frame::default(),
            prompt: String::new(),
            size: None,
            color: None,
            bold: None,
            align: None,
            anchor: Anchor::Top,
            autofit: Autofit::Shrink,
            bullets: false,
            ph: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_resolve_against_the_theme() {
        let theme = Theme::default();
        let accent = theme.slot("accent1").unwrap();
        assert_eq!(theme.resolve(&Color::from("accent1")), accent);
        let lighter = theme.resolve(&Color::from("accent1+50"));
        assert!(lighter.luminance() > accent.luminance());
        let darker = theme.resolve(&Color::from("accent1-50"));
        assert!(darker.luminance() < accent.luminance());
        assert_eq!(
            theme.resolve(&Color::from("#FF000080")),
            Rgba {
                r: 255,
                g: 0,
                b: 0,
                a: 128
            }
        );
        assert_eq!(theme.resolve(&Color::from("#0f0")), Rgba::opaque(0, 255, 0));
        assert_eq!(parse_slot("accent9"), None);
    }

    #[test]
    fn agents_json_reads() {
        let element: Element = serde_json::from_str(
            r##"{"id":"e3","frame":{"x":40,"y":40,"w":200,"h":80},
                "kind":{"type":"shape","geometry":"round_rect","fill":{"type":"solid","color":"accent2"},
                "text":{"paragraphs":[{"runs":[{"text":"Hi","bold":true}]}]}}}"##,
        )
        .unwrap();
        let Kind::Shape(shape) = &element.kind else {
            panic!("a shape");
        };
        assert_eq!(shape.geometry, Geometry::RoundRect);
        assert_eq!(element.opacity, 1.);
        assert_eq!(element.kind.text().unwrap().plain(), "Hi");
    }
}

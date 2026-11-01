//! Slide text: each run's style worked out (the run's own, then its
//! placeholder's, then the theme's), then laid out in its box with
//! cosmic-text: wrapped, aligned, bulleted and, for boxes that shrink to
//! fit, made smaller until it does (as PowerPoint's autofit does).

use super::fonts::fonts;
use super::model::*;

/// A run's style as drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct Style {
    /// The family as named in the deck.
    pub font: String,
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub color: Rgba,
    pub highlight: Option<Rgba>,
    pub link: Option<String>,
    pub baseline: i16,
}

/// A paragraph ready to lay out.
#[derive(Debug, Clone)]
pub struct Para {
    pub runs: Vec<(String, Style)>,
    pub align: Align,
    pub level: u8,
    /// Its bullet's text and style (a number already worked out).
    pub bullet: Option<(String, Style)>,
    pub space_before: f32,
    pub space_after: f32,
    pub line_spacing: f32,
    /// The style of an empty paragraph's line.
    pub style: Style,
}

/// Where a text body's style comes from.
pub struct Context<'a> {
    pub theme: &'a Theme,
    pub place: Option<&'a Placeholder>,
    /// The colour text takes when nothing sets one (inside a filled shape,
    /// one that reads on the fill).
    pub color: Option<Rgba>,
    /// The size and weight it takes when nothing sets them (a table's).
    pub size: Option<f32>,
    pub bold: Option<bool>,
}

impl Context<'_> {
    fn base(&self) -> TextStyle {
        let theme = self.theme;
        match self.place {
            Some(place) if place.kind.is_title() => theme.title.clone(),
            Some(_) => theme.body.clone(),
            // Free text: the body's look, a little smaller, no bullets.
            None => TextStyle {
                size: (theme.body.size * 0.82).round(),
                bullets: Vec::new(),
                ..theme.body.clone()
            },
        }
    }
}

/// The paragraphs of `body` with every style worked out.
pub fn paragraphs(body: &TextBody, cx: &Context) -> Vec<Para> {
    let theme = cx.theme;
    let base = cx.base();
    let place = cx.place;
    let bullets = place.is_some_and(|place| place.bullets);
    let mut numbers = [0u32; 8];
    let base_color = place
        .and_then(|place| place.color.as_ref())
        .map(|color| theme.resolve(color))
        .or(cx.color)
        .unwrap_or_else(|| theme.resolve(&base.color));
    let base_style = Style {
        font: theme.font(&base.font).to_string(),
        size: place
            .and_then(|place| place.size)
            .or(cx.size)
            .unwrap_or(base.size),
        bold: place
            .and_then(|place| place.bold)
            .or(cx.bold)
            .unwrap_or(base.bold),
        italic: base.italic,
        underline: false,
        strike: false,
        color: base_color,
        highlight: None,
        link: None,
        baseline: 0,
    };
    body.paragraphs
        .iter()
        .map(|paragraph| {
            let runs: Vec<(String, Style)> = paragraph
                .runs
                .iter()
                .map(|run| (run.text.clone(), style(run, &base_style, theme)))
                .collect();
            let first = runs
                .first()
                .map(|(_, style)| style.clone())
                .unwrap_or_else(|| base_style.clone());
            let level = paragraph.level.min(7);
            let bullet = match &paragraph.bullet {
                Some(Bullet::None) => None,
                Some(bullet) => Some(bullet.clone()),
                None if bullets => Some(
                    base.bullets
                        .get(level as usize)
                        .cloned()
                        .unwrap_or(Bullet::Char("•".into())),
                ),
                None => None,
            };
            // Numbers count on through a list, and start again after it.
            if !matches!(bullet, Some(Bullet::Number)) {
                numbers[level as usize..].iter_mut().for_each(|n| *n = 0);
            }
            let bullet = bullet.map(|bullet| {
                let text = match bullet {
                    Bullet::Number => {
                        numbers[level as usize] += 1;
                        numbers[level as usize + 1..]
                            .iter_mut()
                            .for_each(|n| *n = 0);
                        format!("{}.", numbers[level as usize])
                    }
                    Bullet::Char(text) => text,
                    Bullet::None => String::new(),
                };
                let style = Style {
                    bold: false,
                    italic: false,
                    underline: false,
                    strike: false,
                    link: None,
                    highlight: None,
                    baseline: 0,
                    ..first.clone()
                };
                (text, style)
            });
            Para {
                runs,
                align: paragraph
                    .align
                    .or(place.and_then(|place| place.align))
                    .unwrap_or(base.align),
                level,
                bullet,
                space_before: paragraph.space_before.unwrap_or(0.),
                space_after: paragraph.space_after.unwrap_or(base.space_after),
                line_spacing: paragraph
                    .line_spacing
                    .unwrap_or(base.line_spacing)
                    .clamp(0.5, 4.),
                style: first,
            }
        })
        .collect()
}

fn style(run: &Run, base: &Style, theme: &Theme) -> Style {
    Style {
        font: run
            .font
            .as_deref()
            .map(|font| theme.font(font).to_string())
            .unwrap_or_else(|| base.font.clone()),
        size: run.size.unwrap_or(base.size).clamp(1., 400.),
        bold: run.bold.unwrap_or(base.bold),
        italic: run.italic.unwrap_or(base.italic),
        underline: run.underline.unwrap_or(run.link.is_some()),
        strike: run.strike.unwrap_or(false),
        color: match (&run.color, &run.link) {
            (Some(color), _) => theme.resolve(color),
            (None, Some(_)) => theme.resolve(&Color::from("link")),
            (None, None) => base.color,
        },
        highlight: run.highlight.as_ref().map(|color| theme.resolve(color)),
        link: run.link.clone(),
        baseline: run.baseline.unwrap_or(0),
    }
}

/// Text laid out in its box.
#[derive(Debug, Clone, Default)]
pub struct Laid {
    pub lines: Vec<Line>,
    /// From the top of the first line to the bottom of the last.
    pub height: f32,
    /// How much the text was made smaller to fit (1: not at all).
    pub scale: f32,
}

#[derive(Debug, Clone)]
pub struct Line {
    /// The baseline, from the top of the text.
    pub baseline: f32,
    pub top: f32,
    pub height: f32,
    pub pieces: Vec<Piece>,
    pub bullet: Option<Piece>,
}

/// A stretch of one style on a line.
#[derive(Debug, Clone)]
pub struct Piece {
    pub x: f32,
    pub width: f32,
    pub text: String,
    pub style: Style,
}

/// One line's height at `size`, before line spacing (PowerPoint's single
/// spacing).
const LINE: f32 = 1.2;

/// Lays out `paras` in a box `width` wide; with `fit`, made smaller (to
/// 40%) until it is no taller than `fit`.
pub fn lay_out(paras: &[Para], width: f32, wrap: bool, fit: Option<f32>) -> Laid {
    let laid = lay_out_at(paras, width, wrap, 1.);
    let Some(room) = fit else {
        return laid;
    };
    if laid.height <= room + 0.5 {
        return laid;
    }
    let mut scale = 0.9;
    while scale > 0.4 {
        let smaller = lay_out_at(paras, width, wrap, scale);
        if smaller.height <= room + 0.5 {
            return smaller;
        }
        scale -= 0.05;
    }
    lay_out_at(paras, width, wrap, 0.4)
}

fn lay_out_at(paras: &[Para], width: f32, wrap: bool, scale: f32) -> Laid {
    use cosmic_text::{Align as TextAlign, Attrs, Buffer, Metrics, Shaping, Wrap};
    let fonts = fonts();
    let mut system = fonts.system.lock().unwrap_or_else(|err| err.into_inner());
    let mut lines = Vec::new();
    let mut y = 0f32;
    for (index, para) in paras.iter().enumerate() {
        let base = para.style.size * scale;
        let spacing = para.line_spacing;
        if index > 0 {
            y += para.space_before * scale;
        }
        let indent = para.level as f32 * base * 1.6;
        let hang = if para.bullet.is_some() {
            base * 1.1
        } else {
            0.
        };
        let left = indent + hang;
        let text_width = (width - left).max(base);
        let families: Vec<String> = para
            .runs
            .iter()
            .map(|(_, style)| fonts.family(&style.font))
            .collect();
        let empty = para.runs.iter().all(|(text, _)| text.is_empty());
        let empty_family = fonts.family(&para.style.font);
        let metrics = Metrics::new(base, base * LINE * spacing);
        let mut buffer = Buffer::new(&mut system, metrics);
        buffer.set_size(Some(text_width), None);
        buffer.set_wrap(if wrap { Wrap::WordOrGlyph } else { Wrap::None });
        let spans: Vec<(&str, Attrs)> = if empty {
            vec![(" ", attrs(&empty_family, 0, &para.style, scale, spacing))]
        } else {
            para.runs
                .iter()
                .enumerate()
                .filter(|(_, (text, _))| !text.is_empty())
                .map(|(ix, (text, style))| {
                    (
                        text.as_str(),
                        attrs(&families[ix], ix, style, scale, spacing),
                    )
                })
                .collect()
        };
        let default = Attrs::new().metrics(metrics);
        let align = match para.align {
            Align::Left => TextAlign::Left,
            Align::Center => TextAlign::Center,
            Align::Right => TextAlign::Right,
            Align::Justify => TextAlign::Justified,
        };
        buffer.set_rich_text(spans, &default, Shaping::Advanced, Some(align));
        buffer.shape_until_scroll(&mut system, false);
        let mut first = true;
        let mut bottom = y;
        for run in buffer.layout_runs() {
            let top = y + run.line_top;
            let mut pieces: Vec<Piece> = Vec::new();
            if !empty {
                let mut start = 0;
                while start < run.glyphs.len() {
                    let ix = run.glyphs[start].metadata;
                    let mut end = start;
                    while end + 1 < run.glyphs.len() && run.glyphs[end + 1].metadata == ix {
                        end += 1;
                    }
                    let glyphs = &run.glyphs[start..=end];
                    let from = glyphs.iter().map(|glyph| glyph.start).min().unwrap_or(0);
                    let to = glyphs.iter().map(|glyph| glyph.end).max().unwrap_or(0);
                    let x = glyphs.iter().map(|glyph| glyph.x).fold(f32::MAX, f32::min);
                    let right = glyphs
                        .iter()
                        .map(|glyph| glyph.x + glyph.w)
                        .fold(f32::MIN, f32::max);
                    let text = run.text.get(from..to).unwrap_or_default().to_string();
                    let style = para.runs[ix].1.clone();
                    pieces.push(Piece {
                        x: left + x,
                        width: right - x,
                        text,
                        style: Style {
                            size: style.size * scale,
                            ..style
                        },
                    });
                    start = end + 1;
                }
            }
            let bullet = (first && para.bullet.is_some()).then(|| {
                let (text, style) = para.bullet.clone().expect("checked above");
                Piece {
                    x: indent,
                    width: hang,
                    text,
                    style: Style {
                        size: style.size * scale,
                        ..style
                    },
                }
            });
            first = false;
            lines.push(Line {
                baseline: y + run.line_y,
                top,
                height: run.line_height,
                pieces,
                bullet,
            });
            bottom = top + run.line_height;
        }
        y = bottom + para.space_after * scale;
    }
    // The last paragraph's space after does not count.
    if let Some(last) = paras.last() {
        y -= last.space_after * scale;
    }
    Laid {
        lines,
        height: y.max(0.),
        scale,
    }
}

/// The attributes cosmic-text shapes a run with.
fn attrs<'a>(
    family: &'a str,
    ix: usize,
    style: &Style,
    scale: f32,
    spacing: f32,
) -> cosmic_text::Attrs<'a> {
    use cosmic_text::{Attrs, Family, Metrics, Style as Slant, Weight};
    let size = run_size(style) * scale;
    Attrs::new()
        .family(Family::Name(family))
        .weight(Weight(super::fonts::fonts().weight(family, style.bold)))
        .style(if style.italic {
            Slant::Italic
        } else {
            Slant::Normal
        })
        .metrics(Metrics::new(size, size * LINE * spacing))
        .metadata(ix)
}

/// A run's size, smaller for super- and subscript.
fn run_size(style: &Style) -> f32 {
    if style.baseline != 0 {
        style.size * 0.65
    } else {
        style.size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(text: &str) -> Vec<Para> {
        let theme = Theme::default();
        paragraphs(
            &super::super::markdown::to_text(text),
            &Context {
                theme: &theme,
                place: None,
                color: None,
                size: None,
                bold: None,
            },
        )
    }

    #[test]
    fn wraps_and_shrinks() {
        let paras = body("A sentence long enough that it has to wrap onto more than one line here");
        let wide = lay_out(&paras, 2000., true, None);
        let narrow = lay_out(&paras, 200., true, None);
        assert_eq!(wide.lines.len(), 1);
        assert!(narrow.lines.len() > 2, "{}", narrow.lines.len());
        assert!(narrow.height > wide.height);
        let fitted = lay_out(&paras, 200., true, Some(wide.height * 1.5));
        assert!(fitted.scale < 1.);
        assert!(fitted.height <= wide.height * 1.5 + 0.5);
        // The pieces hold the text in order.
        let text: Vec<String> = narrow
            .lines
            .iter()
            .map(|line| line.pieces.iter().map(|piece| piece.text.clone()).collect())
            .collect();
        let text = text.join(" ");
        assert_eq!(text.split_whitespace().count(), 15);
    }

    #[test]
    fn bullets_and_numbers() {
        let paras = body("1. one\n1. two\n- dot");
        assert_eq!(paras[0].bullet.as_ref().unwrap().0, "1.");
        assert_eq!(paras[1].bullet.as_ref().unwrap().0, "2.");
        assert_eq!(paras[2].bullet.as_ref().unwrap().0, "•");
        let laid = lay_out(&paras, 400., true, None);
        assert_eq!(laid.lines.len(), 3);
        let line = &laid.lines[0];
        assert!(line.bullet.is_some());
        assert!(line.pieces[0].x > line.bullet.as_ref().unwrap().x);
    }
}

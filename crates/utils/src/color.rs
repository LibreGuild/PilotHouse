//! Colors: read in any form CSS, Android or Flutter writes them, written
//! in each of those, how well text reads on them, and lighter and darker
//! steps.

use anyhow::{Context as _, Result, bail};

use crate::Input;

/// A color in sRGB, with its opacity (0 to 1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: f64,
}

impl Rgba {
    pub const WHITE: Rgba = Rgba::opaque(255, 255, 255);
    pub const BLACK: Rgba = Rgba::opaque(0, 0, 0);

    pub const fn opaque(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    /// `#rrggbb`, or `#rrggbbaa` when not opaque.
    pub fn hex(self) -> String {
        if self.a >= 1.0 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!(
                "#{:02x}{:02x}{:02x}{:02x}",
                self.r,
                self.g,
                self.b,
                (self.a * 255.0).round() as u8
            )
        }
    }

    /// Hue (degrees), saturation and lightness (0 to 1).
    pub fn hsl(self) -> (f64, f64, f64) {
        let (r, g, b) = (
            self.r as f64 / 255.0,
            self.g as f64 / 255.0,
            self.b as f64 / 255.0,
        );
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        let l = (max + min) / 2.0;
        if max == min {
            return (0.0, 0.0, l);
        }
        let d = max - min;
        let s = if l > 0.5 {
            d / (2.0 - max - min)
        } else {
            d / (max + min)
        };
        let h = if max == r {
            (g - b) / d + if g < b { 6.0 } else { 0.0 }
        } else if max == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        };
        (h * 60.0, s, l)
    }

    pub fn from_hsl(h: f64, s: f64, l: f64, a: f64) -> Self {
        let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
        let x = c * (1.0 - ((h.rem_euclid(360.0) / 60.0) % 2.0 - 1.0).abs());
        let m = l - c / 2.0;
        let (r, g, b) = match (h.rem_euclid(360.0) / 60.0) as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let byte = |value: f64| ((value + m) * 255.0).round().clamp(0.0, 255.0) as u8;
        Rgba {
            r: byte(r),
            g: byte(g),
            b: byte(b),
            a,
        }
    }

    /// Lightness, chroma and hue in OKLCH, the space CSS uses for colors
    /// that look evenly spaced.
    pub fn oklch(self) -> (f64, f64, f64) {
        let linear = |byte: u8| {
            let value = byte as f64 / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        let (r, g, b) = (linear(self.r), linear(self.g), linear(self.b));
        let l = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
        let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
        let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
        let lightness = 0.210_454_255_3 * l + 0.793_617_785 * m - 0.004_072_046_8 * s;
        let a = 1.977_998_495_1 * l - 2.428_592_205 * m + 0.450_593_709_9 * s;
        let b = 0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766 * s;
        let chroma = (a * a + b * b).sqrt();
        let hue = if chroma < 1e-4 {
            0.0
        } else {
            b.atan2(a).to_degrees().rem_euclid(360.0)
        };
        (lightness, chroma, hue)
    }

    /// Relative luminance, as WCAG works it out.
    pub fn luminance(self) -> f64 {
        let channel = |byte: u8| {
            let value = byte as f64 / 255.0;
            if value <= 0.03928 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(self.r) + 0.7152 * channel(self.g) + 0.0722 * channel(self.b)
    }

    /// The color laid over `under`, as it shows when not opaque.
    pub fn over(self, under: Rgba) -> Rgba {
        let mix = |top: u8, bottom: u8| {
            (top as f64 * self.a + bottom as f64 * (1.0 - self.a)).round() as u8
        };
        Rgba::opaque(
            mix(self.r, under.r),
            mix(self.g, under.g),
            mix(self.b, under.b),
        )
    }
}

/// How well text of one color reads on the other: 1 (not at all) to 21.
pub fn contrast(text: Rgba, background: Rgba) -> f64 {
    let (text, background) = (text.over(background), background.over(Rgba::WHITE));
    let (a, b) = (text.luminance(), background.luminance());
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// What WCAG says of a contrast: "AAA", "AA", "AA for large text" or
/// "fails".
pub fn grade(contrast: f64) -> &'static str {
    if contrast >= 7.0 {
        "AAA"
    } else if contrast >= 4.5 {
        "AA"
    } else if contrast >= 3.0 {
        "AA large"
    } else {
        "Fails"
    }
}

/// The color in each form it is written in, by name.
pub fn forms(color: Rgba) -> Vec<(&'static str, String)> {
    let (h, s, l) = color.hsl();
    let (ok_l, ok_c, ok_h) = color.oklch();
    let alpha = |separator: &str| {
        if color.a >= 1.0 {
            String::new()
        } else {
            format!("{separator}{}", trim_number(color.a))
        }
    };
    let argb = format!(
        "0x{:02X}{:02X}{:02X}{:02X}",
        (color.a * 255.0).round() as u8,
        color.r,
        color.g,
        color.b
    );
    let mut forms = vec![
        ("HEX", color.hex()),
        (
            "RGB",
            format!("rgb({} {} {}{})", color.r, color.g, color.b, alpha(" / ")),
        ),
        (
            "HSL",
            format!(
                "hsl({} {}% {}%{})",
                h.round(),
                (s * 100.0).round(),
                (l * 100.0).round(),
                alpha(" / ")
            ),
        ),
        (
            "OKLCH",
            format!(
                "oklch({}% {} {}{})",
                trim_number((ok_l * 1000.0).round() / 10.0),
                trim_number((ok_c * 1000.0).round() / 1000.0),
                trim_number((ok_h * 10.0).round() / 10.0),
                alpha(" / ")
            ),
        ),
        ("Android", argb.clone()),
        ("Flutter", format!("Color({argb})")),
        (
            "SwiftUI",
            format!(
                "Color(red: {}, green: {}, blue: {}{})",
                trim_number((color.r as f64 / 255.0 * 1000.0).round() / 1000.0),
                trim_number((color.g as f64 / 255.0 * 1000.0).round() / 1000.0),
                trim_number((color.b as f64 / 255.0 * 1000.0).round() / 1000.0),
                if color.a >= 1.0 {
                    String::new()
                } else {
                    format!(", opacity: {}", trim_number(color.a))
                }
            ),
        ),
    ];
    if let Some(name) = name_of(color) {
        forms.insert(1, ("Name", name.to_string()));
    }
    forms
}

/// `0.5` rather than `0.500`, `12` rather than `12.0`.
fn trim_number(value: f64) -> String {
    let text = format!("{value:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Steps from near white to near black through the color, at even
/// lightness, the color itself among them.
pub fn shades(color: Rgba) -> Vec<Rgba> {
    let (h, s, l) = color.hsl();
    let mut steps: Vec<Rgba> = [0.95, 0.85, 0.75, 0.65, 0.55, 0.45, 0.35, 0.25, 0.15]
        .iter()
        .map(|lightness| Rgba::from_hsl(h, s, *lightness, 1.0))
        .collect();
    // The step nearest to the color becomes the color.
    if let Some(nearest) = [0.95f64, 0.85, 0.75, 0.65, 0.55, 0.45, 0.35, 0.25, 0.15]
        .iter()
        .enumerate()
        .min_by(|a, b| (a.1 - l).abs().total_cmp(&(b.1 - l).abs()))
        .map(|(ix, _)| ix)
    {
        steps[nearest] = Rgba { a: 1.0, ..color };
    }
    steps
}

/// Reads a color: `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb(…)`,
/// `rgba(…)`, `hsl(…)`, a CSS name, or Android's and Flutter's
/// `0xAARRGGBB` (also inside `Color(…)`).
pub fn parse(text: &str) -> Result<Rgba> {
    let lower = text.trim().trim_end_matches(';').trim().to_lowercase();
    let lower = lower
        .strip_prefix("color(")
        .and_then(|inner| inner.strip_suffix(')'))
        .map_or(lower.as_str(), str::trim)
        .to_string();
    if let Some(hex) = lower.strip_prefix("0x") {
        let value = u32::from_str_radix(hex, 16).context("Not a hex color")?;
        let [a, r, g, b] = match hex.len() {
            8 => value.to_be_bytes(),
            6 => (value | 0xff00_0000).to_be_bytes(),
            _ => bail!("0x colors have 6 or 8 digits (AARRGGBB)."),
        };
        return Ok(Rgba {
            r,
            g,
            b,
            a: a as f64 / 255.0,
        });
    }
    if let Some(hex) = lower.strip_prefix('#').or_else(|| {
        (lower.len() == 6 && lower.chars().all(|c| c.is_ascii_hexdigit())).then_some(lower.as_str())
    }) {
        // Digits only (sliced by byte below: other characters would cut
        // one in two).
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            bail!("Not a hex color");
        }
        let hex: String = match hex.len() {
            3 | 4 => hex.chars().flat_map(|c| [c, c]).collect(),
            6 | 8 => hex.to_string(),
            _ => bail!("A hex color has 3, 4, 6 or 8 digits."),
        };
        let byte = |ix: usize| u8::from_str_radix(&hex[ix..ix + 2], 16).context("Not a hex color");
        let a = if hex.len() == 8 {
            byte(6)? as f64 / 255.0
        } else {
            1.0
        };
        return Ok(Rgba {
            r: byte(0)?,
            g: byte(2)?,
            b: byte(4)?,
            a,
        });
    }
    let numbers = |prefix: &str| -> Option<Vec<(f64, bool)>> {
        let inner = lower
            .strip_prefix(prefix)?
            .trim_start_matches('a')
            .trim()
            .strip_prefix('(')?
            .strip_suffix(')')?;
        inner
            .split([',', ' ', '/'])
            .filter(|part| !part.is_empty())
            .map(|part| {
                let percent = part.ends_with('%');
                part.trim_end_matches(['%', '°'])
                    .trim_end_matches("deg")
                    .parse()
                    .ok()
                    .map(|value| (value, percent))
            })
            .collect()
    };
    let alpha = |values: &[(f64, bool)]| match values.get(3) {
        Some((value, true)) => (value / 100.0).clamp(0.0, 1.0),
        Some((value, false)) => value.clamp(0.0, 1.0),
        None => 1.0,
    };
    if let Some(values) = numbers("rgb").filter(|values| values.len() >= 3) {
        let channel = |(value, percent): (f64, bool)| {
            let value = if percent { value * 2.55 } else { value };
            value.round().clamp(0.0, 255.0) as u8
        };
        return Ok(Rgba {
            r: channel(values[0]),
            g: channel(values[1]),
            b: channel(values[2]),
            a: alpha(&values),
        });
    }
    if let Some(values) = numbers("hsl").filter(|values| values.len() >= 3) {
        return Ok(Rgba::from_hsl(
            values[0].0,
            values[1].0 / 100.0,
            values[2].0 / 100.0,
            alpha(&values),
        ));
    }
    if let Some((_, value)) = NAMED.iter().find(|(name, _)| *name == lower) {
        let [_, r, g, b] = value.to_be_bytes();
        return Ok(Rgba::opaque(r, g, b));
    }
    if lower == "transparent" {
        return Ok(Rgba {
            a: 0.0,
            ..Rgba::BLACK
        });
    }
    bail!("Give a color as #rrggbb, rgb(…), hsl(…), a CSS name or 0xAARRGGBB.")
}

/// The CSS name of exactly this color, if it has one.
pub fn name_of(color: Rgba) -> Option<&'static str> {
    if color.a < 1.0 {
        return None;
    }
    let value = u32::from_be_bytes([0, color.r, color.g, color.b]);
    NAMED
        .iter()
        .find(|(_, named)| *named == value)
        .map(|(name, _)| *name)
}

/// A color in hex, RGB, HSL and the other forms, and how text reads on it.
pub fn color(input: &Input) -> Result<String> {
    let color = parse(&input.text)?;
    let mut lines: Vec<String> = forms(color)
        .into_iter()
        .map(|(name, value)| format!("{name}: {value}"))
        .collect();
    for (name, text) in [("White", Rgba::WHITE), ("Black", Rgba::BLACK)] {
        let ratio = contrast(text, color);
        lines.push(format!(
            "{name} text on it: {ratio:.2}:1 ({})",
            grade(ratio)
        ));
    }
    Ok(lines.join("\n"))
}

/// The CSS named colors.
const NAMED: &[(&str, u32)] = &[
    ("aliceblue", 0xf0f8ff),
    ("antiquewhite", 0xfaebd7),
    ("aqua", 0x00ffff),
    ("aquamarine", 0x7fffd4),
    ("azure", 0xf0ffff),
    ("beige", 0xf5f5dc),
    ("bisque", 0xffe4c4),
    ("black", 0x000000),
    ("blanchedalmond", 0xffebcd),
    ("blue", 0x0000ff),
    ("blueviolet", 0x8a2be2),
    ("brown", 0xa52a2a),
    ("burlywood", 0xdeb887),
    ("cadetblue", 0x5f9ea0),
    ("chartreuse", 0x7fff00),
    ("chocolate", 0xd2691e),
    ("coral", 0xff7f50),
    ("cornflowerblue", 0x6495ed),
    ("cornsilk", 0xfff8dc),
    ("crimson", 0xdc143c),
    ("cyan", 0x00ffff),
    ("darkblue", 0x00008b),
    ("darkcyan", 0x008b8b),
    ("darkgoldenrod", 0xb8860b),
    ("darkgray", 0xa9a9a9),
    ("darkgreen", 0x006400),
    ("darkgrey", 0xa9a9a9),
    ("darkkhaki", 0xbdb76b),
    ("darkmagenta", 0x8b008b),
    ("darkolivegreen", 0x556b2f),
    ("darkorange", 0xff8c00),
    ("darkorchid", 0x9932cc),
    ("darkred", 0x8b0000),
    ("darksalmon", 0xe9967a),
    ("darkseagreen", 0x8fbc8f),
    ("darkslateblue", 0x483d8b),
    ("darkslategray", 0x2f4f4f),
    ("darkslategrey", 0x2f4f4f),
    ("darkturquoise", 0x00ced1),
    ("darkviolet", 0x9400d3),
    ("deeppink", 0xff1493),
    ("deepskyblue", 0x00bfff),
    ("dimgray", 0x696969),
    ("dimgrey", 0x696969),
    ("dodgerblue", 0x1e90ff),
    ("firebrick", 0xb22222),
    ("floralwhite", 0xfffaf0),
    ("forestgreen", 0x228b22),
    ("fuchsia", 0xff00ff),
    ("gainsboro", 0xdcdcdc),
    ("ghostwhite", 0xf8f8ff),
    ("gold", 0xffd700),
    ("goldenrod", 0xdaa520),
    ("gray", 0x808080),
    ("green", 0x008000),
    ("greenyellow", 0xadff2f),
    ("grey", 0x808080),
    ("honeydew", 0xf0fff0),
    ("hotpink", 0xff69b4),
    ("indianred", 0xcd5c5c),
    ("indigo", 0x4b0082),
    ("ivory", 0xfffff0),
    ("khaki", 0xf0e68c),
    ("lavender", 0xe6e6fa),
    ("lavenderblush", 0xfff0f5),
    ("lawngreen", 0x7cfc00),
    ("lemonchiffon", 0xfffacd),
    ("lightblue", 0xadd8e6),
    ("lightcoral", 0xf08080),
    ("lightcyan", 0xe0ffff),
    ("lightgoldenrodyellow", 0xfafad2),
    ("lightgray", 0xd3d3d3),
    ("lightgreen", 0x90ee90),
    ("lightgrey", 0xd3d3d3),
    ("lightpink", 0xffb6c1),
    ("lightsalmon", 0xffa07a),
    ("lightseagreen", 0x20b2aa),
    ("lightskyblue", 0x87cefa),
    ("lightslategray", 0x778899),
    ("lightslategrey", 0x778899),
    ("lightsteelblue", 0xb0c4de),
    ("lightyellow", 0xffffe0),
    ("lime", 0x00ff00),
    ("limegreen", 0x32cd32),
    ("linen", 0xfaf0e6),
    ("magenta", 0xff00ff),
    ("maroon", 0x800000),
    ("mediumaquamarine", 0x66cdaa),
    ("mediumblue", 0x0000cd),
    ("mediumorchid", 0xba55d3),
    ("mediumpurple", 0x9370db),
    ("mediumseagreen", 0x3cb371),
    ("mediumslateblue", 0x7b68ee),
    ("mediumspringgreen", 0x00fa9a),
    ("mediumturquoise", 0x48d1cc),
    ("mediumvioletred", 0xc71585),
    ("midnightblue", 0x191970),
    ("mintcream", 0xf5fffa),
    ("mistyrose", 0xffe4e1),
    ("moccasin", 0xffe4b5),
    ("navajowhite", 0xffdead),
    ("navy", 0x000080),
    ("oldlace", 0xfdf5e6),
    ("olive", 0x808000),
    ("olivedrab", 0x6b8e23),
    ("orange", 0xffa500),
    ("orangered", 0xff4500),
    ("orchid", 0xda70d6),
    ("palegoldenrod", 0xeee8aa),
    ("palegreen", 0x98fb98),
    ("paleturquoise", 0xafeeee),
    ("palevioletred", 0xdb7093),
    ("papayawhip", 0xffefd5),
    ("peachpuff", 0xffdab9),
    ("peru", 0xcd853f),
    ("pink", 0xffc0cb),
    ("plum", 0xdda0dd),
    ("powderblue", 0xb0e0e6),
    ("purple", 0x800080),
    ("rebeccapurple", 0x663399),
    ("red", 0xff0000),
    ("rosybrown", 0xbc8f8f),
    ("royalblue", 0x4169e1),
    ("saddlebrown", 0x8b4513),
    ("salmon", 0xfa8072),
    ("sandybrown", 0xf4a460),
    ("seagreen", 0x2e8b57),
    ("seashell", 0xfff5ee),
    ("sienna", 0xa0522d),
    ("silver", 0xc0c0c0),
    ("skyblue", 0x87ceeb),
    ("slateblue", 0x6a5acd),
    ("slategray", 0x708090),
    ("slategrey", 0x708090),
    ("snow", 0xfffafa),
    ("springgreen", 0x00ff7f),
    ("steelblue", 0x4682b4),
    ("tan", 0xd2b48c),
    ("teal", 0x008080),
    ("thistle", 0xd8bfd8),
    ("tomato", 0xff6347),
    ("turquoise", 0x40e0d0),
    ("violet", 0xee82ee),
    ("wheat", 0xf5deb3),
    ("white", 0xffffff),
    ("whitesmoke", 0xf5f5f5),
    ("yellow", 0xffff00),
    ("yellowgreen", 0x9acd32),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_with_other_characters_are_refused() {
        // Sliced by byte, `é` would have been cut in two (a panic).
        assert!(parse("#aééa").is_err());
        assert!(parse("aééa").is_err());
        assert!(parse("#12g").is_err());
        assert!(parse("#fa0").is_ok());
    }

    #[test]
    fn reads_every_form() {
        let blue = Rgba::opaque(59, 130, 246);
        assert_eq!(parse("#3b82f6").unwrap(), blue);
        assert_eq!(parse("rgb(59, 130, 246)").unwrap(), blue);
        assert_eq!(parse("rgb(59 130 246 / 50%)").unwrap().a, 0.5);
        assert_eq!(parse("0xFF3B82F6").unwrap(), blue);
        assert_eq!(parse("Color(0xFF3B82F6)").unwrap(), blue);
        assert_eq!(parse("hsl(0, 100%, 50%)").unwrap().hex(), "#ff0000");
        assert_eq!(parse("RebeccaPurple").unwrap().hex(), "#663399");
        assert_eq!(parse("#fff").unwrap().hex(), "#ffffff");
        assert_eq!(parse("#3b82f680").unwrap().hex(), "#3b82f680");
        let said = color(&Input::new("#3b82f6")).unwrap();
        assert!(said.contains("RGB: rgb(59 130 246)"), "{said}");
        assert!(said.contains("Android: 0xFF3B82F6"), "{said}");
    }

    #[test]
    fn works_out_contrast_and_steps() {
        assert!((contrast(Rgba::BLACK, Rgba::WHITE) - 21.0).abs() < 0.01);
        assert_eq!(
            grade(contrast(Rgba::WHITE, parse("#767676").unwrap())),
            "AA"
        );
        let blue = Rgba::opaque(59, 130, 246);
        let steps = shades(blue);
        assert_eq!(steps.len(), 9);
        assert!(steps.contains(&blue));
        let (l, c, h) = parse("#ff0000").unwrap().oklch();
        assert!((l - 0.628).abs() < 0.01 && (c - 0.258).abs() < 0.01 && (h - 29.2).abs() < 0.5);
        assert_eq!(name_of(parse("#ff0000").unwrap()), Some("red"));
    }
}

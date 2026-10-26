//! Terminal colors.
//!
//! The 16 ANSI colors are a palette tuned for terminals, one for dark mode
//! (after One Dark) and one for light (after GitHub's): the theme's own
//! reds and greens are made for badges and read pale as text, and bright
//! colors must differ from normal ones. Background, foreground, cursor and
//! selection follow the theme. Indexed colors 16–255 follow the standard
//! xterm cube and grey ramp; programs may still override any of them (OSC 4).
//!
//! This is one of the few places raw color values are allowed: terminal
//! palettes and the xterm cube are part of the terminal's look and protocol.

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};
use gpui_kit::component::Theme;
use gpui_kit::{Hsla, Rgba, rgb};

pub struct Palette {
    ansi: [Hsla; 16],
    pub foreground: Hsla,
    pub background: Hsla,
    pub cursor: Hsla,
    /// Behind selected text.
    pub selection: Hsla,
}

/// Opacity applied to "dim" text (SGR 2).
const DIM: f32 = 0.66;

/// Black, red, green, yellow, blue, magenta, cyan, white; then the bright
/// eight.
const DARK: [u32; 16] = [
    0x3f4451, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xd7dae0, //
    0x6b7280, 0xff7b86, 0xb5e890, 0xffd68a, 0x82c4ff, 0xde9bf0, 0x7ad7e0, 0xffffff,
];
const LIGHT: [u32; 16] = [
    0x24292f, 0xcf222e, 0x116329, 0x7d4e00, 0x0969da, 0x8250df, 0x1b7c83, 0x6e7781, //
    0x57606a, 0xa40e26, 0x1a7f37, 0x9a6700, 0x218bff, 0xa475f9, 0x3192aa, 0x8c959f,
];

impl Palette {
    pub fn from_theme(theme: &Theme) -> Self {
        let ansi = if theme.is_dark() { DARK } else { LIGHT };
        Self {
            ansi: ansi.map(|hex| rgb(hex).into()),
            foreground: theme.foreground,
            background: theme.background,
            cursor: theme.caret,
            selection: theme.selection,
        }
    }

    /// The colour a program asks about: 0–255 the palette, 256 the text,
    /// 257 the background, 258 the cursor.
    pub fn query(&self, index: usize) -> alacritty_terminal::vte::ansi::Rgb {
        let color = match index {
            256 => self.foreground,
            257 => self.background,
            258 => self.cursor,
            index => self.indexed(index.min(255) as u8),
        };
        let rgba = Rgba::from(color);
        alacritty_terminal::vte::ansi::Rgb {
            r: (rgba.r * 255.0).round() as u8,
            g: (rgba.g * 255.0).round() as u8,
            b: (rgba.b * 255.0).round() as u8,
        }
    }

    /// Resolves a cell color. `overrides` are colors the program set itself.
    pub fn resolve(&self, color: Color, overrides: &Colors) -> Hsla {
        match color {
            Color::Spec(rgb) => to_hsla(rgb),
            Color::Indexed(index) => match overrides[index as usize] {
                Some(rgb) => to_hsla(rgb),
                None => self.indexed(index),
            },
            Color::Named(named) => match overrides[named] {
                Some(rgb) => to_hsla(rgb),
                None => self.named(named),
            },
        }
    }

    fn named(&self, named: NamedColor) -> Hsla {
        let index = named as usize;
        if index < 16 {
            return self.ansi[index];
        }
        match named {
            NamedColor::Foreground | NamedColor::BrightForeground => self.foreground,
            NamedColor::Background => self.background,
            NamedColor::Cursor => self.cursor,
            NamedColor::DimForeground => self.foreground.opacity(DIM),
            // The dim variants of the eight base colors follow DimBlack in order.
            other => {
                let base = (other as usize).saturating_sub(NamedColor::DimBlack as usize);
                self.ansi
                    .get(base)
                    .copied()
                    .unwrap_or(self.foreground)
                    .opacity(DIM)
            }
        }
    }

    fn indexed(&self, index: u8) -> Hsla {
        match index {
            0..=15 => self.ansi[index as usize],
            16..=231 => {
                let index = index - 16;
                let level = |value: u8| if value == 0 { 0 } else { value * 40 + 55 };
                to_hsla(Rgb {
                    r: level(index / 36),
                    g: level((index / 6) % 6),
                    b: level(index % 6),
                })
            }
            _ => {
                let grey = 8 + (index - 232) * 10;
                to_hsla(Rgb {
                    r: grey,
                    g: grey,
                    b: grey,
                })
            }
        }
    }
}

fn to_hsla(color: Rgb) -> Hsla {
    Rgba {
        r: f32::from(color.r) / 255.0,
        g: f32::from(color.g) / 255.0,
        b: f32::from(color.b) / 255.0,
        a: 1.0,
    }
    .into()
}

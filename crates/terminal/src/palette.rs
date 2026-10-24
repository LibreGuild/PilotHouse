//! Terminal colors from the app theme.
//!
//! The 16 ANSI colors come from the theme's base palette, so terminals match
//! light and dark mode. Indexed colors 16–255 follow the standard xterm cube
//! and grey ramp; programs may still override any of them (OSC 4).
//!
//! This is one of the few places raw color values are allowed: the xterm cube
//! is part of the terminal protocol, not a design choice.

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};
use gpui_kit::component::Theme;
use gpui_kit::{Hsla, Rgba};

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

impl Palette {
    pub fn from_theme(theme: &Theme) -> Self {
        let black = if theme.is_dark() {
            theme.muted
        } else {
            theme.foreground
        };
        let white = if theme.is_dark() {
            theme.foreground
        } else {
            theme.muted_foreground
        };
        Self {
            ansi: [
                black,
                theme.red,
                theme.green,
                theme.yellow,
                theme.blue,
                theme.magenta,
                theme.cyan,
                white,
                theme.muted_foreground,
                theme.red_light,
                theme.green_light,
                theme.yellow_light,
                theme.blue_light,
                theme.magenta_light,
                theme.cyan_light,
                theme.foreground,
            ],
            foreground: theme.foreground,
            background: theme.background,
            cursor: theme.caret,
            selection: theme.selection,
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

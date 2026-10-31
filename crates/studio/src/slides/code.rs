//! Code on slides, coloured by syntect (the highlighter the editor uses)
//! with a dark or a light theme. The same tokens are drawn and exported to
//! PPTX as coloured runs, so the code stays text there too.

use std::sync::OnceLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Theme, ThemeSet};
use syntect::parsing::SyntaxSet;

use super::model::Rgba;

/// A token: its text, colour, and whether bold or italic.
pub type Token = (String, Rgba, bool, bool);

fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn theme(dark: bool) -> &'static Theme {
    static THEMES: OnceLock<ThemeSet> = OnceLock::new();
    let set = THEMES.get_or_init(ThemeSet::load_defaults);
    let name = if dark {
        "base16-ocean.dark"
    } else {
        "InspiredGitHub"
    };
    set.themes
        .get(name)
        .or_else(|| set.themes.values().next())
        .expect("syntect has themes")
}

/// The background a code block has in each theme.
pub fn background(dark: bool) -> Rgba {
    theme(dark)
        .settings
        .background
        .map(|color| Rgba {
            r: color.r,
            g: color.g,
            b: color.b,
            a: 255,
        })
        .unwrap_or(if dark {
            Rgba::opaque(0x2B, 0x30, 0x3B)
        } else {
            Rgba::opaque(0xF6, 0xF8, 0xFA)
        })
}

/// `code` in `language` (an extension or a name: `rs`, `Rust`,
/// `python`…), as lines of tokens.
pub fn highlight(code: &str, language: &str, dark: bool) -> Vec<Vec<Token>> {
    let set = syntaxes();
    let syntax = set
        .find_syntax_by_token(language)
        .or_else(|| set.find_syntax_by_extension(language))
        .unwrap_or_else(|| set.find_syntax_plain_text());
    let theme = theme(dark);
    let plain = theme
        .settings
        .foreground
        .map(|color| Rgba::opaque(color.r, color.g, color.b))
        .unwrap_or(if dark { Rgba::WHITE } else { Rgba::BLACK });
    let mut highlighter = HighlightLines::new(syntax, theme);
    code.split('\n')
        .map(|line| {
            let with_newline = format!("{line}\n");
            match highlighter.highlight_line(&with_newline, set) {
                Ok(ranges) => ranges
                    .into_iter()
                    .filter_map(|(style, text)| {
                        let text = text.trim_end_matches('\n');
                        (!text.is_empty()).then(|| {
                            (
                                text.replace('\t', "    "),
                                Rgba::opaque(
                                    style.foreground.r,
                                    style.foreground.g,
                                    style.foreground.b,
                                ),
                                style.font_style.contains(FontStyle::BOLD),
                                style.font_style.contains(FontStyle::ITALIC),
                            )
                        })
                    })
                    .collect(),
                Err(_) => vec![(line.replace('\t', "    "), plain, false, false)],
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn colours_tokens() {
        let lines = super::highlight("fn main() {\n    let x = 1;\n}", "rs", true);
        assert_eq!(lines.len(), 3);
        let colours: std::collections::HashSet<String> = lines
            .iter()
            .flatten()
            .map(|(_, color, ..)| color.hex())
            .collect();
        assert!(colours.len() > 2, "{colours:?}");
        let text: String = lines[1].iter().map(|(text, ..)| text.as_str()).collect();
        assert_eq!(text, "    let x = 1;");
    }
}

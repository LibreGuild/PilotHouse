//! A deck on disk: the files in an artifact's `site/` folder that the
//! deck's page shows.
//!
//! ```text
//! site/
//!   deck.json          the slides in order, their notes, the deck's size
//!   deck.css           the deck's look: colours, fonts, how each kind of slide is laid out
//!   slides/cover.html  one file a slide: a `<section class="slide">` and what is on it
//!   media/             pictures and videos the slides show
//! ```
//!
//! Plain files, because an agent changes a deck with the tools it has for
//! any file, and because the user can read, copy and keep them. The page
//! never holds the truth: what it changes is written here at once.

use std::path::{Component, Path};

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// What a deck's artifact says it is (`artifact.json`).
pub const KIND: &str = "slides";
pub const DECK: &str = "deck.json";
pub const STYLE: &str = "deck.css";
pub const SLIDES: &str = "slides";
pub const MEDIA: &str = "media";

/// A slide's size when the deck says none: 16:9, in the page's pixels.
pub const SIZE: [u32; 2] = [1920, 1080];

/// `deck.json`. Fields a build does not know are kept.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Deck {
    pub title: String,
    /// A slide's width and height, in the page's pixels.
    pub size: [u32; 2],
    /// How one slide gives way to the next when presenting, unless a slide
    /// says otherwise: `none`, `fade`, `slide`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub transition: String,
    pub slides: Vec<SlideRef>,
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

impl Default for Deck {
    fn default() -> Self {
        Self {
            title: String::new(),
            size: SIZE,
            transition: String::new(),
            slides: Vec::new(),
            other: Map::new(),
        }
    }
}

/// One slide of a deck: its file, and what is said of it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SlideRef {
    /// Its file, relative to the deck (`slides/cover.html`).
    pub file: String,
    /// What the presenter says; never shown to the audience.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub notes: String,
    /// Left out when presenting and exporting.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub skip: bool,
    /// The part of the deck it starts (shown where all slides are laid out).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub section: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub transition: String,
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

impl Deck {
    pub fn parse(text: &str) -> Result<Self> {
        let mut deck: Deck = serde_json::from_str(text).context("deck.json is not readable")?;
        if deck.size[0] < 16 || deck.size[1] < 16 {
            deck.size = SIZE;
        }
        Ok(deck)
    }

    pub fn to_text(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into());
        text.push('\n');
        text
    }

    /// The slides shown when presenting and exporting.
    pub fn shown(&self) -> impl Iterator<Item = &SlideRef> {
        self.slides.iter().filter(|slide| !slide.skip)
    }
}

/// Whether `path` (relative to the deck, with `/`) is one of the deck's
/// text files, whose changes the page follows and can undo: `deck.json`,
/// `deck.css` and the slides.
pub fn tracked(path: &str) -> bool {
    if path == DECK || path == STYLE {
        return true;
    }
    path.strip_prefix("slides/").is_some_and(|name| {
        name.strip_suffix(".html").is_some_and(|stem| {
            !stem.is_empty()
                && stem.len() <= 80
                && stem
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
    })
}

/// `path` as a place inside the deck: no way up or out, no hidden files.
pub fn inside(path: &str) -> Result<&Path> {
    let asked = Path::new(path);
    let plain = asked.components().all(|part| match part {
        Component::Normal(name) => !name.to_string_lossy().starts_with('.'),
        _ => false,
    });
    if !plain || path.is_empty() || path.contains(['\\', ':', '\0']) {
        bail!("{path} is not a file of the deck");
    }
    Ok(asked)
}

/// A name for a slide's file that no slide has: `wanted` made plain, with a
/// number when it is taken.
pub fn free_slide_file(wanted: &str, taken: impl Fn(&str) -> bool) -> String {
    let mut stem: String = wanted
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    while stem.contains("--") {
        stem = stem.replace("--", "-");
    }
    let stem = stem.trim_matches('-');
    let stem: String = if stem.is_empty() { "slide" } else { stem }
        .chars()
        .take(40)
        .collect();
    let mut name = format!("{SLIDES}/{stem}.html");
    let mut n = 2;
    while taken(&name) {
        name = format!("{SLIDES}/{stem}-{n}.html");
        n += 1;
    }
    name
}

/// A short mark of a text, to tell whether it changed (FNV-1a, 64 bits).
/// Not for secrets: only this app's own pages compare them.
pub fn mark(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deck_reads_and_keeps_what_it_does_not_know() {
        let deck = Deck::parse(
            r#"{"title":"Plan","slides":[{"file":"slides/a.html","notes":"hello","mood":"calm"}],"brand":"acme"}"#,
        )
        .unwrap();
        assert_eq!(deck.size, SIZE);
        assert_eq!(deck.slides[0].notes, "hello");
        let text = deck.to_text();
        assert!(text.contains("\"brand\": \"acme\"") && text.contains("\"mood\": \"calm\""));
        // What says nothing is left out.
        assert!(!text.contains("skip") && !text.contains("section"));
        assert_eq!(Deck::parse(&text).unwrap(), deck);
        assert!(Deck::parse("{").is_err());
        // A size that is none falls back.
        assert_eq!(Deck::parse(r#"{"size":[0,0]}"#).unwrap().size, SIZE);
    }

    #[test]
    fn only_the_decks_own_text_files_are_tracked() {
        for path in [
            "deck.json",
            "deck.css",
            "slides/cover.html",
            "slides/a-b_2.html",
        ] {
            assert!(tracked(path), "{path}");
        }
        for path in [
            "slides/.hidden.html",
            "slides/a/b.html",
            "slides/a.htm",
            "media/a.png",
            "../deck.json",
            "slides/.html",
        ] {
            assert!(!tracked(path), "{path}");
        }
    }

    #[test]
    fn paths_stay_inside_the_deck() {
        assert!(inside("media/photo.png").is_ok());
        for path in [
            "",
            "../x",
            "/etc/passwd",
            "media/../../x",
            ".git/config",
            "a\\b",
            "c:x",
        ] {
            assert!(inside(path).is_err(), "{path}");
        }
    }

    #[test]
    fn slide_files_get_plain_free_names() {
        let taken = |name: &str| name == "slides/our-plan.html";
        assert_eq!(
            free_slide_file("Our plan!", taken),
            "slides/our-plan-2.html"
        );
        assert_eq!(free_slide_file("  ", |_| false), "slides/slide.html");
        assert_eq!(free_slide_file("Été 2026", |_| false), "slides/t-2026.html");
    }

    #[test]
    fn marks_tell_texts_apart() {
        assert_eq!(mark("a"), mark("a"));
        assert_ne!(mark("a"), mark("b"));
        assert_eq!(mark("").len(), 16);
    }
}

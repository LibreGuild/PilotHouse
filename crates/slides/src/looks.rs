//! The looks a new deck can start from, and the layouts *New slide* offers.
//!
//! A look is the deck's `deck.css`, whole: its colours and fonts as
//! variables on top, then how headings, lists, tables, cards and each kind
//! of slide are set. Written into the deck, it is the deck's own from then
//! on: the user or an agent changes any of it. Only fonts every computer
//! has, or that the app brings (Inter, JetBrains Mono), so a deck reads the
//! same on a colleague's machine.

use crate::deck::{DECK, Deck, STYLE};
use crate::live::Change;

/// A look: its name, what it is like, and what makes it differ.
pub struct Look {
    pub id: &'static str,
    pub name: &'static str,
    pub hint: &'static str,
    /// Its colours and fonts.
    variables: &'static str,
    /// What only this look has (a title slide's decoration).
    extra: &'static str,
}

const SERIF: &str = "\"Iowan Old Style\", \"Palatino Linotype\", Palatino, Georgia, serif";
const SANS: &str =
    "Inter, -apple-system, \"Segoe UI\", Roboto, \"Helvetica Neue\", Arial, sans-serif";
const MONO: &str = "\"JetBrains Mono\", ui-monospace, Menlo, Consolas, monospace";

pub const LOOKS: &[Look] = &[
    Look {
        id: "paper",
        name: "Paper",
        hint: "Warm, serif headings",
        variables: "--bg: #f6f1e7; --fg: #241d17; --muted: #6f6357; --accent: #a8431f; --accent-fg: #fff8f0; --surface: #fffaf2; --line: #dfd5c4; --font-head: SERIF; --font-body: SANS; --head-weight: 700;",
        extra: ".slide.title::before { content: \"\"; position: absolute; right: -140px; top: -200px; width: 620px; height: 620px; border-radius: 50%; background: var(--accent); }\n.slide.title::after { content: \"\"; position: absolute; right: 190px; bottom: -150px; width: 360px; height: 360px; border-radius: 50%; background: var(--line); }\n.slide.title > * { position: relative; z-index: 1; }",
    },
    Look {
        id: "midnight",
        name: "Midnight",
        hint: "Dark, calm, blue accent",
        variables: "--bg: #0f1420; --fg: #eef1f8; --muted: #9aa5bd; --accent: #6ea0ff; --accent-fg: #0b1020; --surface: #1a2133; --line: #2a3350; --font-head: SANS; --font-body: SANS; --head-weight: 700;",
        extra: ".slide.title { background: radial-gradient(1200px 700px at 85% -10%, #24345f 0%, transparent 60%), var(--bg); }\n.slide.title h1 { letter-spacing: -0.03em; }",
    },
    Look {
        id: "clean",
        name: "Clean",
        hint: "White, plain, for any subject",
        variables: "--bg: #ffffff; --fg: #15171c; --muted: #646b78; --accent: #2457e6; --accent-fg: #ffffff; --surface: #f3f5f9; --line: #dde1ea; --font-head: SANS; --font-body: SANS; --head-weight: 750;",
        extra: ".slide.title::before { content: \"\"; position: absolute; left: 112px; top: 96px; width: 96px; height: 10px; border-radius: 5px; background: var(--accent); }\n.slide.title h1 { letter-spacing: -0.03em; }",
    },
    Look {
        id: "forest",
        name: "Forest",
        hint: "Deep green and cream",
        variables: "--bg: #f3efe2; --fg: #16302a; --muted: #586b62; --accent: #1f6b52; --accent-fg: #f3efe2; --surface: #fbf8ee; --line: #d4d2bd; --font-head: SERIF; --font-body: SANS; --head-weight: 700;",
        extra: ".slide.title { background: var(--fg); color: var(--bg); }\n.slide.title .lead { color: inherit; opacity: 0.72; }\n.slide.title .eyebrow { color: #8fd3b4; }\n.slide.title::after { content: \"\"; position: absolute; left: 0; right: 0; bottom: 0; height: 26px; background: var(--accent); }",
    },
    Look {
        id: "bold",
        name: "Bold",
        hint: "Black and yellow, large type",
        variables: "--bg: #101010; --fg: #f7f7f2; --muted: #a8a89c; --accent: #ffd60a; --accent-fg: #101010; --surface: #1d1d1b; --line: #33332f; --font-head: SANS; --font-body: SANS; --head-weight: 850;",
        extra: ".slide h1, .slide h2 { text-transform: uppercase; letter-spacing: -0.02em; }\n.slide.title h1 { font-size: 132px; }\n.slide.title::before { content: \"\"; position: absolute; left: 0; top: 0; bottom: 0; width: 28px; background: var(--accent); }",
    },
    Look {
        id: "slate",
        name: "Slate",
        hint: "Grey-blue, quiet, for reports",
        variables: "--bg: #eef1f5; --fg: #1c2530; --muted: #5d6a7a; --accent: #c2410c; --accent-fg: #ffffff; --surface: #ffffff; --line: #cfd6e0; --font-head: SANS; --font-body: SANS; --head-weight: 700;",
        extra: ".slide { border-top: 14px solid var(--fg); }\n.slide.section { border-top-color: transparent; }",
    },
];

/// How everything on a slide is set, from the look's variables.
const RULES: &str = r#"
.slide {
  background: var(--bg);
  color: var(--fg);
  font: 400 34px/1.4 var(--font-body);
  padding: 96px 112px;
  display: flex;
  flex-direction: column;
}
.slide h1, .slide h2, .slide h3 { font-family: var(--font-head); margin: 0; text-wrap: balance; }
.slide h1 { font-size: 108px; line-height: 1.04; letter-spacing: -0.02em; font-weight: var(--head-weight); margin-bottom: 36px; }
.slide h2 { font-size: 64px; line-height: 1.1; letter-spacing: -0.015em; font-weight: var(--head-weight); margin-bottom: 44px; }
.slide h3 { font-size: 38px; line-height: 1.2; font-weight: 650; margin-bottom: 14px; }
.slide p { margin: 0 0 22px; }
.slide ul, .slide ol { margin: 0 0 22px; padding-left: 1.15em; }
.slide li { margin: 0 0 14px; padding-left: 0.2em; }
.slide li::marker { color: var(--accent); }
.slide a { color: var(--accent); }
.slide strong { font-weight: 700; }
.slide code { font-family: var(--font-code); font-size: 0.88em; background: var(--surface); padding: 0.1em 0.35em; border-radius: 8px; }
.slide pre { font: 26px/1.5 var(--font-code); background: var(--surface); padding: 32px 36px; border-radius: 20px; overflow: hidden; margin: 0 0 22px; }
.slide pre code { background: none; padding: 0; font-size: inherit; }

/* Small pieces. */
.eyebrow { font: 650 22px/1.2 var(--font-body); letter-spacing: 0.16em; text-transform: uppercase; color: var(--accent); margin: 0 0 22px; }
.lead { font-size: 44px; line-height: 1.3; color: var(--muted); max-width: 30em; }
.small, .caption, .slide figcaption { font-size: 24px; color: var(--muted); }
.muted { color: var(--muted); }
.stat { font: var(--head-weight) 120px/1 var(--font-head); color: var(--accent); letter-spacing: -0.02em; margin: 0 0 12px; }
.footer { margin-top: auto; padding-top: 28px; font-size: 22px; color: var(--muted); display: flex; justify-content: space-between; gap: 40px; }

/* Things side by side: columns share the width; a card is a column with a
   ground of its own. */
.cols { display: grid; grid-auto-flow: column; grid-auto-columns: minmax(0, 1fr); gap: 44px; }
.card { background: var(--surface); border-radius: 24px; padding: 40px 40px 36px; border-top: 6px solid var(--accent); }
.card > :last-child, .cols > * > :last-child, .slide > :last-child { margin-bottom: 0; }

.slide table { width: 100%; font-size: 28px; line-height: 1.3; }
.slide th { text-align: left; font-weight: 650; color: var(--muted); font-size: 22px; letter-spacing: 0.08em; text-transform: uppercase; padding: 0 20px 18px 0; border-bottom: 3px solid var(--fg); }
.slide td { padding: 20px 20px 20px 0; border-bottom: 1px solid var(--line); vertical-align: top; }
.slide blockquote { margin: 0; font: var(--head-weight) 72px/1.15 var(--font-head); letter-spacing: -0.015em; text-wrap: balance; max-width: 17em; }
.slide cite { display: block; font-style: normal; font-size: 30px; color: var(--muted); margin-top: 36px; }
.slide figure { margin: 0; display: flex; flex-direction: column; gap: 18px; min-height: 0; }
.slide img, .slide video { display: block; border-radius: 20px; object-fit: cover; }
.slide [data-chart] { flex: 1; min-height: 0; margin: 0; }

/* Kinds of slide: `<section class="slide title">` and so on. */
.slide.title, .slide.section, .slide.quote, .slide.closing { justify-content: center; }
.slide.title h1 { max-width: 13em; }
.slide.section { background: var(--accent); color: var(--accent-fg); }
.slide.section .eyebrow { color: inherit; opacity: 0.75; }
.slide.section h2 { font-size: 96px; max-width: 14em; margin-bottom: 20px; }
.slide.section .lead { color: inherit; opacity: 0.8; }
.slide.picture { flex-direction: row; gap: 72px; align-items: center; }
.slide.picture > * { flex: 1; min-width: 0; }
.slide.closing h2 { font-size: 104px; }
.slide.dark { background: var(--fg); color: var(--bg); }
.slide.dark .lead, .slide.dark .muted { color: inherit; opacity: 0.7; }
"#;

impl Look {
    /// The look's `deck.css`.
    pub fn css(&self) -> String {
        let variables = self
            .variables
            .replace("SERIF", SERIF)
            .replace("SANS", SANS)
            .split("; ")
            .map(|line| format!("  {};", line.trim_end_matches(';')))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "/* The deck's look ({name}). Colours and fonts first: change them here and every\n   slide follows. Sizes are in pixels of a slide. */\n.slide {{\n{variables}\n  --font-code: {MONO};\n}}\n{RULES}\n/* {name}'s own. */\n{extra}\n",
            name = self.name,
            extra = self.extra,
        )
    }
}

pub fn look(id: &str) -> &'static Look {
    LOOKS.iter().find(|look| look.id == id).unwrap_or(&LOOKS[0])
}

/// The files of a new deck: no slides yet, and `look`'s `deck.css`.
pub fn new_deck(title: &str, look_id: &str) -> Vec<Change> {
    let deck = Deck {
        title: title.to_string(),
        transition: "fade".into(),
        ..Deck::default()
    };
    vec![
        Change {
            path: DECK.into(),
            text: Some(deck.to_text()),
        },
        Change {
            path: STYLE.into(),
            text: Some(look(look_id).css()),
        },
    ]
}

/// A layout *New slide* offers: its name and what the slide starts as.
pub struct Layout {
    pub id: &'static str,
    pub name: &'static str,
    pub html: &'static str,
}

pub const LAYOUTS: &[Layout] = &[
    Layout {
        id: "blank",
        name: "Blank",
        html: "<section class=\"slide\">\n</section>\n",
    },
    Layout {
        id: "title",
        name: "Title",
        html: "<section class=\"slide title\">\n  <p class=\"eyebrow\">Subject</p>\n  <h1>Presentation title</h1>\n  <p class=\"lead\">A one-line subtitle for the presentation.</p>\n</section>\n",
    },
    Layout {
        id: "section",
        name: "Section",
        html: "<section class=\"slide section\">\n  <p class=\"eyebrow\">Section 01</p>\n  <h2>Section title</h2>\n</section>\n",
    },
    Layout {
        id: "body",
        name: "Title + body",
        html: "<section class=\"slide\">\n  <h2>Heading</h2>\n  <ul>\n    <li>The first point, in one line.</li>\n    <li>The second point.</li>\n    <li>The third point.</li>\n  </ul>\n</section>\n",
    },
    Layout {
        id: "columns",
        name: "Two columns",
        html: "<section class=\"slide\">\n  <h2>Heading</h2>\n  <div class=\"cols\">\n    <div class=\"card\">\n      <h3>First</h3>\n      <p>What is to be said about it.</p>\n    </div>\n    <div class=\"card\">\n      <h3>Second</h3>\n      <p>What is to be said about it.</p>\n    </div>\n  </div>\n</section>\n",
    },
    Layout {
        id: "table",
        name: "Table",
        html: "<section class=\"slide\">\n  <h2>Summary</h2>\n  <table>\n    <thead>\n      <tr><th>Item</th><th>Status</th><th>Owner</th></tr>\n    </thead>\n    <tbody>\n      <tr><td>First</td><td>Done</td><td>Name</td></tr>\n      <tr><td>Second</td><td>Under way</td><td>Name</td></tr>\n      <tr><td>Third</td><td>Next</td><td>Name</td></tr>\n    </tbody>\n  </table>\n</section>\n",
    },
    Layout {
        id: "quote",
        name: "Quote",
        html: "<section class=\"slide quote\">\n  <blockquote>\u{201c}A memorable line worth a whole slide.\u{201d}</blockquote>\n  <cite>Who said it</cite>\n</section>\n",
    },
    Layout {
        id: "numbers",
        name: "Numbers",
        html: "<section class=\"slide\">\n  <h2>In numbers</h2>\n  <div class=\"cols\">\n    <div>\n      <p class=\"stat\">42%</p>\n      <p>What the first number measures.</p>\n    </div>\n    <div>\n      <p class=\"stat\">3\u{d7}</p>\n      <p>What the second number measures.</p>\n    </div>\n    <div>\n      <p class=\"stat\">120</p>\n      <p>What the third number measures.</p>\n    </div>\n  </div>\n</section>\n",
    },
    Layout {
        id: "chart",
        name: "Chart",
        html: "<section class=\"slide\">\n  <h2>What the chart shows</h2>\n  <figure data-chart='{\"type\":\"bar\",\"labels\":[\"Q1\",\"Q2\",\"Q3\",\"Q4\"],\"series\":[{\"name\":\"Revenue\",\"values\":[12,18,15,24]}]}'></figure>\n</section>\n",
    },
    Layout {
        id: "closing",
        name: "Thank you",
        html: "<section class=\"slide closing\">\n  <h2>Thank you</h2>\n  <p class=\"lead\">Questions, and where to reach you.</p>\n</section>\n",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_look_is_a_whole_stylesheet_with_its_own_colours() {
        for look in LOOKS {
            let css = look.css();
            assert!(css.contains(&format!("({})", look.name)), "{}", look.id);
            for name in [
                "--bg:",
                "--fg:",
                "--accent:",
                "--font-head:",
                "--font-code:",
            ] {
                assert!(css.contains(name), "{} has {name}", look.id);
            }
            // The placeholders for the font stacks are written out.
            assert!(
                !css.contains("SERIF") && !css.contains("SANS;"),
                "{}",
                look.id
            );
            assert_eq!(
                css.matches('{').count(),
                css.matches('}').count(),
                "{}",
                look.id
            );
        }
        assert_eq!(look("no such look").id, LOOKS[0].id);
    }

    #[test]
    fn a_new_deck_has_its_look_and_no_slides() {
        let files = new_deck("Plan", "midnight");
        let deck = Deck::parse(files[0].text.as_deref().unwrap()).unwrap();
        assert_eq!((deck.title.as_str(), deck.slides.len()), ("Plan", 0));
        assert!(files[1].text.as_deref().unwrap().contains("(Midnight)"));
        assert!(files.iter().all(|file| crate::deck::tracked(&file.path)));
    }

    #[test]
    fn every_layout_is_one_slide() {
        for layout in LAYOUTS {
            assert!(
                layout.html.starts_with("<section class=\"slide"),
                "{}",
                layout.id
            );
            assert!(
                layout.html.trim_end().ends_with("</section>"),
                "{}",
                layout.id
            );
            assert_eq!(layout.html.matches("<section").count(), 1, "{}", layout.id);
        }
    }
}

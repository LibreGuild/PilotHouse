//! Text as words: the cases code names are written in, counts, characters
//! that cannot be seen or are not what they look like, and work on lines.

use anyhow::Result;

use crate::Input;

/// The words of a name however it is written: `getHTTPResponse`,
/// `get_http_response`, `Get HTTP response` all give get, http, response.
pub fn words_of(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let chars: Vec<char> = text.chars().collect();
    for (ix, c) in chars.iter().copied().enumerate() {
        if !c.is_alphanumeric() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            continue;
        }
        let previous = ix.checked_sub(1).map(|ix| chars[ix]);
        let next = chars.get(ix + 1).copied();
        let starts = match previous {
            Some(previous) if previous.is_alphanumeric() => {
                // fooBar, or the R of HTTPResponse.
                (c.is_uppercase() && previous.is_lowercase())
                    || (c.is_uppercase()
                        && previous.is_uppercase()
                        && next.is_some_and(char::is_lowercase))
                    || (c.is_alphabetic() && previous.is_ascii_digit() && word.len() > 1)
            }
            _ => false,
        };
        if starts && !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
        word.push(c);
    }
    if !word.is_empty() {
        words.push(word);
    }
    words.into_iter().map(|word| word.to_lowercase()).collect()
}

fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// The text in each case, by name.
pub fn cases(text: &str) -> Vec<(&'static str, String)> {
    let words = words_of(text);
    let joined = |separator: &str| words.join(separator);
    let camel: String = words
        .iter()
        .enumerate()
        .map(|(ix, word)| {
            if ix == 0 {
                word.clone()
            } else {
                capitalized(word)
            }
        })
        .collect();
    let pascal: String = words.iter().map(|word| capitalized(word)).collect();
    let sentence = {
        let all = joined(" ");
        capitalized(&all)
    };
    vec![
        ("camelCase", camel),
        ("PascalCase", pascal),
        ("snake_case", joined("_")),
        ("SCREAMING_SNAKE", joined("_").to_uppercase()),
        ("kebab-case", joined("-")),
        (
            "Title Case",
            words
                .iter()
                .map(|word| capitalized(word))
                .collect::<Vec<_>>()
                .join(" "),
        ),
        ("Sentence case", sentence),
        ("dot.case", joined(".")),
        ("lowercase", text.to_lowercase()),
        ("UPPERCASE", text.to_uppercase()),
    ]
}

/// How much text there is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    /// Characters as a person counts them (`é` is one, however written).
    pub characters: usize,
    pub words: usize,
    pub lines: usize,
    /// Bytes in UTF-8, what a length limit on a server usually counts.
    pub bytes: usize,
    /// UTF-16 units, what JavaScript's `length` counts.
    pub utf16: usize,
    pub blank_lines: usize,
}

pub fn counts(text: &str) -> Counts {
    Counts {
        characters: text.chars().filter(|c| !is_combining(*c)).count(),
        words: text.split_whitespace().count(),
        lines: if text.is_empty() {
            0
        } else {
            text.lines().count()
        },
        bytes: text.len(),
        utf16: text.encode_utf16().count(),
        blank_lines: text.lines().filter(|line| line.trim().is_empty()).count(),
    }
}

fn is_combining(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
}

/// A character that is easy to miss: where it is (in characters), what it
/// is, and how it is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Odd {
    pub at: usize,
    pub line: usize,
    pub name: &'static str,
    pub code: String,
}

/// Characters that cannot be seen, that look like others, or that mean
/// trouble in code: zero-width spaces, a no-break space, typographic
/// quotes, a byte order mark, direction marks, controls.
pub fn odd_characters(text: &str) -> Vec<Odd> {
    let mut line = 1;
    let mut found = Vec::new();
    for (at, c) in text.chars().enumerate() {
        if c == '\n' {
            line += 1;
            continue;
        }
        let name = match c {
            '\u{200B}' => "zero-width space",
            '\u{200C}' => "zero-width non-joiner",
            '\u{200D}' => "zero-width joiner",
            '\u{2060}' => "word joiner",
            '\u{FEFF}' => "byte order mark",
            '\u{00A0}' => "no-break space",
            '\u{202F}' => "narrow no-break space",
            '\u{2007}' | '\u{2009}' | '\u{200A}' | '\u{2002}' | '\u{2003}' => {
                "a wide or thin space"
            }
            '\u{00AD}' => "soft hyphen",
            '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' => {
                "text direction mark"
            }
            '\u{2018}' | '\u{2019}' => "typographic single quote",
            '\u{201C}' | '\u{201D}' => "typographic double quote",
            '\u{2013}' => "en dash",
            '\u{2014}' => "em dash",
            '\u{2212}' => "minus sign (not a hyphen)",
            '\u{2026}' => "ellipsis (one character)",
            '\r' => "carriage return",
            '\t' => continue,
            c if c.is_control() => "control character",
            // Letters from other scripts that look Latin: a Cyrillic а in
            // a name, a Greek ο in a domain.
            '\u{0430}' | '\u{0435}' | '\u{043E}' | '\u{0440}' | '\u{0441}' | '\u{0445}'
            | '\u{0443}' | '\u{0410}' | '\u{0415}' | '\u{041E}' | '\u{0420}' | '\u{0421}'
            | '\u{0425}' => "Cyrillic letter that looks Latin",
            '\u{03BF}' | '\u{039F}' | '\u{03B1}' | '\u{0391}' => "Greek letter that looks Latin",
            _ => continue,
        };
        found.push(Odd {
            at,
            line,
            name,
            code: format!("U+{:04X}", c as u32),
        });
    }
    found
}

/// What can be done to lines.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LineWork {
    pub sort: Option<Sort>,
    pub unique: bool,
    pub trim: bool,
    pub drop_blank: bool,
    pub reverse: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    /// A to Z, numbers by their value (`2` before `10`), any case.
    Natural,
    /// By length, shortest first.
    Length,
}

pub fn lines(text: &str, work: LineWork) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    if work.trim {
        lines = lines
            .into_iter()
            .map(|line| line.trim().to_string())
            .collect();
    }
    if work.drop_blank {
        lines.retain(|line| !line.trim().is_empty());
    }
    if work.unique {
        let mut seen = std::collections::HashSet::new();
        lines.retain(|line| seen.insert(line.clone()));
    }
    match work.sort {
        Some(Sort::Natural) => lines.sort_by(|a, b| natural(a, b)),
        Some(Sort::Length) => lines.sort_by_key(|line| line.chars().count()),
        None => {}
    }
    if work.reverse {
        lines.reverse();
    }
    lines.join("\n")
}

/// Compares as a person sorts: case aside, runs of digits by their value.
fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    let pieces = |text: &str| -> Vec<(bool, String)> {
        let mut out: Vec<(bool, String)> = Vec::new();
        for c in text.chars() {
            let digit = c.is_ascii_digit();
            match out.last_mut() {
                Some((was_digit, piece)) if *was_digit == digit => piece.push(c),
                _ => out.push((digit, c.to_string())),
            }
        }
        out
    };
    let (left, right) = (pieces(a), pieces(b));
    for ((left_digit, left), (right_digit, right)) in left.iter().zip(&right) {
        let order = if *left_digit && *right_digit {
            let (l, r) = (left.trim_start_matches('0'), right.trim_start_matches('0'));
            l.len().cmp(&r.len()).then_with(|| l.cmp(r))
        } else {
            left.to_lowercase().cmp(&right.to_lowercase())
        };
        if order.is_ne() {
            return order;
        }
    }
    left.len().cmp(&right.len()).then_with(|| a.cmp(b))
}

/// Cases, counts and odd characters, or the lines sorted.
pub fn text(input: &Input) -> Result<String> {
    Ok(match input.mode {
        0 => cases(&input.text)
            .into_iter()
            .map(|(name, text)| format!("{name}: {text}"))
            .collect::<Vec<_>>()
            .join("\n"),
        1 => {
            let counted = counts(&input.text);
            let mut out = vec![
                format!("Characters: {}", counted.characters),
                format!("Words: {}", counted.words),
                format!("Lines: {}", counted.lines),
                format!("Bytes (UTF-8): {}", counted.bytes),
                format!("UTF-16 units (JavaScript length): {}", counted.utf16),
            ];
            let odd = odd_characters(&input.text);
            if odd.is_empty() {
                out.push("No hidden or look-alike characters.".into());
            }
            for odd in odd {
                out.push(format!(
                    "Line {}, character {}: {} ({})",
                    odd.line,
                    odd.at + 1,
                    odd.name,
                    odd.code
                ));
            }
            out.join("\n")
        }
        _ => lines(
            &input.text,
            LineWork {
                sort: Some(Sort::Natural),
                unique: true,
                trim: true,
                drop_blank: true,
                reverse: false,
            },
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_words_in_any_case() {
        assert_eq!(words_of("getHTTPResponse"), ["get", "http", "response"]);
        assert_eq!(words_of("user_id-v2 Name"), ["user", "id", "v2", "name"]);
        let of = |name: &str| {
            cases("parse JSON body")
                .into_iter()
                .find(|(label, _)| *label == name)
                .unwrap()
                .1
        };
        assert_eq!(of("camelCase"), "parseJsonBody");
        assert_eq!(of("PascalCase"), "ParseJsonBody");
        assert_eq!(of("SCREAMING_SNAKE"), "PARSE_JSON_BODY");
        assert_eq!(of("kebab-case"), "parse-json-body");
        assert_eq!(of("Sentence case"), "Parse json body");
    }

    #[test]
    fn counts_and_finds_what_hides() {
        let counted = counts("héllo wörld\n\nx");
        assert_eq!(
            (counted.characters, counted.words, counted.lines),
            (14, 3, 3)
        );
        assert_eq!(counted.bytes, 16);
        assert_eq!(counts("😀").utf16, 2);
        let odd = odd_characters("a\u{200B}b\n“q” раypal");
        let names: Vec<&str> = odd.iter().map(|odd| odd.name).collect();
        assert_eq!(names[0], "zero-width space");
        assert!(names.contains(&"typographic double quote"));
        assert!(names.contains(&"Cyrillic letter that looks Latin"));
        assert_eq!(odd[1].line, 2);
    }

    #[test]
    fn works_on_lines() {
        let all = LineWork {
            sort: Some(Sort::Natural),
            unique: true,
            trim: true,
            drop_blank: true,
            reverse: false,
        };
        assert_eq!(
            lines(" b\nfile10\nfile2\n\nb\nA", all),
            "A\nb\nfile2\nfile10"
        );
        let reversed = LineWork {
            reverse: true,
            ..LineWork::default()
        };
        assert_eq!(lines("1\n2\n3", reversed), "3\n2\n1");
    }
}

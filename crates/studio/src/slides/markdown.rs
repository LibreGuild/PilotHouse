//! Light Markdown to slide text, the way agents (and people) write it:
//! a paragraph a line, `- ` and `1. ` lists (indent two spaces a level),
//! `**bold**`, `*italic*`, `~~struck~~`, `` `code` `` and `[links](url)`.
//! When some lines are list items, the others are not (so a body
//! placeholder's bullets stay off them); with none, the placeholder decides.

use super::model::{Bullet, Paragraph, Run, TextBody};

/// The paragraphs of `markdown`, a line each: a line ending in two spaces
/// goes on in the same paragraph (a line break within it, as Markdown has
/// it).
pub fn paragraph_lines(markdown: &str) -> Vec<String> {
    let mut joined: Vec<String> = Vec::new();
    let mut open = false;
    for line in markdown.trim_end_matches('\n').split('\n') {
        let line = line.trim_end_matches('\r');
        let continues = line.ends_with("  ") && !line.trim().is_empty();
        let text = if continues { line.trim_end() } else { line };
        match joined.last_mut() {
            Some(last) if open => {
                last.push('\n');
                last.push_str(text.trim_start());
            }
            _ => joined.push(text.to_string()),
        }
        open = continues;
    }
    joined
}

pub fn to_text(markdown: &str) -> TextBody {
    let joined = paragraph_lines(markdown);
    let lines: Vec<&str> = joined.iter().map(String::as_str).collect();
    let listed = lines.iter().any(|line| list_item(line).is_some());
    let paragraphs = lines
        .iter()
        .map(|line| {
            let line = line.trim_end_matches('\r');
            match list_item(line) {
                Some((level, bullet, rest)) => Paragraph {
                    runs: runs(rest),
                    level,
                    bullet: Some(bullet),
                    ..Paragraph::default()
                },
                None => {
                    let (heading, rest) = match line.trim_start().strip_prefix("# ") {
                        Some(rest) => (true, rest),
                        None => (false, line.trim()),
                    };
                    let mut runs = runs(rest);
                    if heading {
                        for run in &mut runs {
                            run.bold = Some(true);
                        }
                    }
                    Paragraph {
                        runs,
                        bullet: listed.then_some(Bullet::None),
                        ..Paragraph::default()
                    }
                }
            }
        })
        .collect();
    TextBody {
        paragraphs,
        ..TextBody::default()
    }
}

/// `body` as the Markdown [`to_text`] reads back: for typing in place.
pub fn from_text(body: &TextBody) -> String {
    body.paragraphs
        .iter()
        .map(|paragraph| {
            let marker = match &paragraph.bullet {
                Some(Bullet::Char(_)) => "- ",
                Some(Bullet::Number) => "1. ",
                _ => "",
            };
            let mut text: String = paragraph.runs.iter().map(run_markdown).collect();
            if marker.is_empty() {
                // What would read as a list or a heading is escaped.
                if text.starts_with("# ") || list_item(&text).is_some() {
                    text.insert(0, '\\');
                }
                text
            } else {
                format!("{}{marker}{text}", "  ".repeat(paragraph.level as usize))
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// One run as inline Markdown.
fn run_markdown(run: &Run) -> String {
    // A line break within the paragraph, as Markdown writes one.
    if run.text.contains('\n') {
        return run
            .text
            .split('\n')
            .map(|piece| {
                run_markdown(&Run {
                    text: piece.to_string(),
                    ..run.clone()
                })
            })
            .collect::<Vec<_>>()
            .join("  \n");
    }
    let chars: Vec<char> = run.text.chars().collect();
    let mut text = String::with_capacity(run.text.len());
    for (ix, c) in chars.iter().enumerate() {
        let special = match c {
            '\\' | '*' | '`' | '[' => true,
            '~' => chars.get(ix + 1) == Some(&'~') || (ix > 0 && chars[ix - 1] == '~'),
            '_' => boundary(&chars, ix),
            _ => false,
        };
        if special {
            text.push('\\');
        }
        text.push(*c);
    }
    if text.trim().is_empty() {
        return text;
    }
    if run.font.as_deref() == Some("code") && !run.text.contains('`') {
        text = format!("`{}`", run.text);
    }
    if let Some(link) = &run.link {
        text = format!("[{text}]({link})");
    }
    if run.strike == Some(true) {
        text = format!("~~{text}~~");
    }
    if run.italic == Some(true) {
        text = format!("*{text}*");
    }
    if run.bold == Some(true) {
        text = format!("**{text}**");
    }
    text
}

/// `line` as a list item: its level, bullet and text.
fn list_item(line: &str) -> Option<(u8, Bullet, &str)> {
    let indent = line.len() - line.trim_start().len();
    let level = (indent / 2).min(4) as u8;
    let rest = line.trim_start();
    for marker in ["- ", "* ", "• ", "+ "] {
        if let Some(text) = rest.strip_prefix(marker) {
            let bullet = if level == 0 { "•" } else { "–" };
            return Some((level, Bullet::Char(bullet.into()), text));
        }
    }
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    if digits > 0
        && let Some(text) = rest[digits..]
            .strip_prefix(". ")
            .or_else(|| rest[digits..].strip_prefix(") "))
    {
        return Some((level, Bullet::Number, text));
    }
    None
}

/// The runs of one line's inline Markdown.
fn runs(text: &str) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::new();
    let (mut bold, mut italic, mut strike) = (false, false, false);
    let mut current = String::new();
    let flush =
        |out: &mut Vec<Run>, current: &mut String, bold: bool, italic: bool, strike: bool| {
            if current.is_empty() {
                return;
            }
            out.push(Run {
                text: std::mem::take(current),
                bold: bold.then_some(true),
                italic: italic.then_some(true),
                strike: strike.then_some(true),
                ..Run::default()
            });
        };
    let chars: Vec<char> = text.chars().collect();
    let mut ix = 0;
    while ix < chars.len() {
        let rest: String = chars[ix..].iter().take(2).collect();
        if chars[ix] == '\\' && ix + 1 < chars.len() {
            current.push(chars[ix + 1]);
            ix += 2;
            continue;
        }
        if rest == "**" || rest == "__" {
            flush(&mut out, &mut current, bold, italic, strike);
            bold = !bold;
            ix += 2;
            continue;
        }
        if rest == "~~" {
            flush(&mut out, &mut current, bold, italic, strike);
            strike = !strike;
            ix += 2;
            continue;
        }
        if chars[ix] == '*' || (chars[ix] == '_' && boundary(&chars, ix)) {
            flush(&mut out, &mut current, bold, italic, strike);
            italic = !italic;
            ix += 1;
            continue;
        }
        if chars[ix] == '`'
            && let Some(end) = chars[ix + 1..].iter().position(|c| *c == '`')
        {
            flush(&mut out, &mut current, bold, italic, strike);
            out.push(Run {
                text: chars[ix + 1..ix + 1 + end].iter().collect(),
                font: Some("code".into()),
                bold: bold.then_some(true),
                italic: italic.then_some(true),
                ..Run::default()
            });
            ix += end + 2;
            continue;
        }
        if chars[ix] == '['
            && let Some(close) = chars[ix..].iter().position(|c| *c == ']')
            && chars.get(ix + close + 1) == Some(&'(')
            && let Some(end) = chars[ix + close + 2..].iter().position(|c| *c == ')')
        {
            flush(&mut out, &mut current, bold, italic, strike);
            let label: String = chars[ix + 1..ix + close].iter().collect();
            let url: String = chars[ix + close + 2..ix + close + 2 + end].iter().collect();
            out.push(Run {
                text: label,
                link: Some(url),
                bold: bold.then_some(true),
                italic: italic.then_some(true),
                ..Run::default()
            });
            ix += close + 3 + end;
            continue;
        }
        current.push(chars[ix]);
        ix += 1;
    }
    flush(&mut out, &mut current, bold, italic, strike);
    if out.is_empty() {
        out.push(Run::default());
    }
    out
}

/// Whether `_` at `ix` starts or ends emphasis (not inside a word, as in
/// `snake_case`).
fn boundary(chars: &[char], ix: usize) -> bool {
    let before = ix.checked_sub(1).and_then(|at| chars.get(at));
    let after = chars.get(ix + 1);
    !(before.is_some_and(|c| c.is_alphanumeric()) && after.is_some_and(|c| c.is_alphanumeric()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_lists_and_emphasis() {
        let text = to_text(
            "Intro with **bold** and *it*\n- one\n  - nested `x`\n1. first\nsnake_case [site](https://a.b)",
        );
        assert_eq!(text.paragraphs.len(), 5);
        let intro = &text.paragraphs[0];
        assert_eq!(
            intro.bullet,
            Some(Bullet::None),
            "lists elsewhere: no bullet"
        );
        assert_eq!(intro.runs[1].text, "bold");
        assert_eq!(intro.runs[1].bold, Some(true));
        assert_eq!(intro.runs[3].italic, Some(true));
        assert_eq!(text.paragraphs[1].bullet, Some(Bullet::Char("•".into())));
        assert_eq!(text.paragraphs[2].level, 1);
        assert_eq!(text.paragraphs[2].runs[1].font.as_deref(), Some("code"));
        assert_eq!(text.paragraphs[3].bullet, Some(Bullet::Number));
        let last = &text.paragraphs[4];
        assert_eq!(last.runs[0].text, "snake_case ");
        assert_eq!(last.runs[1].link.as_deref(), Some("https://a.b"));
        // Without lists, the placeholder decides.
        assert_eq!(to_text("plain").paragraphs[0].bullet, None);
    }

    #[test]
    fn back_to_markdown() {
        for markdown in [
            "Intro with **bold** and *it*\n- one\n  - nested `x`\n1. first",
            "snake_case [site](https://a.b) and \\*stars\\*",
            "- one line  \n  and its second\n- another",
            "plain\nlines",
        ] {
            let text = to_text(markdown);
            assert_eq!(to_text(&from_text(&text)), text, "{markdown}");
        }
    }
}

//! Text: regular expressions, diffs and Markdown.

use std::ops::Range;

use anyhow::Result;

use crate::Input;

/// How a pattern reads its text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Flags {
    /// `i`: upper and lower case alike.
    pub ignore_case: bool,
    /// `m`: `^` and `$` at each line.
    pub multi_line: bool,
    /// `s`: `.` also takes a line break.
    pub dot_all: bool,
    /// `x`: spaces and `#` comments in the pattern are ignored.
    pub verbose: bool,
}

/// One match: where it is in the text, and each group's place (none for a
/// group that took no part).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub range: Range<usize>,
    pub groups: Vec<(String, Option<Range<usize>>)>,
}

/// Matches shown at most; a pattern that matches everywhere (`.`) on a
/// big text would otherwise fill the screen and the memory.
pub const MAX_MATCHES: usize = 1000;

/// The pattern with its flags. It takes what JavaScript's and PCRE's take,
/// look-around and back-references too, and gives up rather than run for
/// ever on a pattern that backtracks without end.
pub fn pattern(text: &str, flags: Flags) -> Result<fancy_regex::Regex> {
    let mut inline = String::new();
    for (on, flag) in [
        (flags.ignore_case, 'i'),
        (flags.multi_line, 'm'),
        (flags.dot_all, 's'),
        (flags.verbose, 'x'),
    ] {
        if on {
            inline.push(flag);
        }
    }
    let full = if inline.is_empty() {
        text.to_string()
    } else {
        format!("(?{inline}){text}")
    };
    fancy_regex::RegexBuilder::new(&full)
        .backtrack_limit(1_000_000)
        .build()
        .map_err(|err| anyhow::anyhow!("Not a valid pattern: {err}"))
}

/// Every match of `pattern` in `text` (up to [`MAX_MATCHES`]), and whether
/// there were more.
pub fn find_all(pattern: &fancy_regex::Regex, text: &str) -> Result<(Vec<Found>, bool)> {
    let names: Vec<String> = pattern
        .capture_names()
        .enumerate()
        .map(|(ix, name)| name.map_or_else(|| ix.to_string(), str::to_string))
        .collect();
    let mut found = Vec::new();
    for caps in pattern.captures_iter(text) {
        let caps = caps.map_err(|err| anyhow::anyhow!("The pattern gave up: {err}"))?;
        if found.len() == MAX_MATCHES {
            return Ok((found, true));
        }
        let whole = caps.get(0).expect("group 0 is the match");
        found.push(Found {
            range: whole.range(),
            groups: names
                .iter()
                .enumerate()
                .skip(1)
                .map(|(ix, name)| (name.clone(), caps.get(ix).map(|group| group.range())))
                .collect(),
        });
    }
    Ok((found, false))
}

/// `text` with each match replaced: `$1`, `${name}` and `$0` stand for
/// what a group took, `$$` for a dollar.
pub fn replace_all(pattern: &fancy_regex::Regex, text: &str, with: &str) -> Result<String> {
    pattern
        .try_replacen(text, 0, with)
        .map(|replaced| replaced.into_owned())
        .map_err(|err| anyhow::anyhow!("The pattern gave up: {err}"))
}

/// Every match of the pattern in the text, with its groups.
pub fn regex(input: &Input) -> Result<String> {
    let pattern = pattern(input.text.trim(), Flags::default())?;
    let (matches, more) = find_all(&pattern, &input.second)?;
    let text = &input.second;
    if input.mode == 1 {
        let json: Vec<serde_json::Value> = matches
            .iter()
            .map(|found| {
                let groups: serde_json::Map<String, serde_json::Value> = found
                    .groups
                    .iter()
                    .filter_map(|(name, range)| {
                        Some((name.clone(), text[range.clone()?].to_string().into()))
                    })
                    .collect();
                serde_json::json!({ "match": &text[found.range.clone()], "start": found.range.start, "groups": groups })
            })
            .collect();
        return Ok(serde_json::to_string_pretty(&json)?);
    }
    if matches.is_empty() {
        return Ok("No matches.".into());
    }
    let mut out = vec![format!(
        "{}{} matches",
        matches.len(),
        if more { "+" } else { "" }
    )];
    for (n, found) in matches.iter().enumerate() {
        out.push(format!(
            "{}. “{}” at {}",
            n + 1,
            &text[found.range.clone()],
            found.range.start
        ));
        for (label, range) in &found.groups {
            if let Some(range) = range {
                out.push(format!("   {label}: “{}”", &text[range.clone()]));
            }
        }
    }
    Ok(out.join("\n"))
}

/// A unified diff from the first text to the second.
pub fn diff(input: &Input) -> Result<String> {
    let text = patch(&input.text, &input.second, 3, false);
    Ok(if text.is_empty() {
        "They are the same.".into()
    } else {
        text
    })
}

/// The unified diff of two texts with `context` lines around each change;
/// with `ignore_space`, lines that differ only in spaces count as the same
/// (and show as they are after).
pub fn patch(before: &str, after: &str, context: usize, ignore_space: bool) -> String {
    // A last line without a line break would be told apart from the same
    // line with one; a person comparing two pastes does not mean that.
    let ended = |text: &str| {
        if text.is_empty() || text.ends_with('\n') {
            text.to_string()
        } else {
            format!("{text}\n")
        }
    };
    let (before, after) = (ended(before), ended(after));
    if !ignore_space {
        return similar::TextDiff::from_lines(&before, &after)
            .unified_diff()
            .context_radius(context)
            .header("before", "after")
            .to_string();
    }
    let squeeze = |text: &str| -> String {
        text.lines()
            .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    };
    let (left, right) = (squeeze(&before), squeeze(&after));
    let diff = similar::TextDiff::from_lines(&left, &right);
    // The changes are worked out on the squeezed lines; the lines shown are
    // the ones as written.
    let before_lines: Vec<&str> = before.lines().collect();
    let after_lines: Vec<&str> = after.lines().collect();
    let mut out = String::new();
    for group in diff.grouped_ops(context) {
        let (first, last) = (&group[0], &group[group.len() - 1]);
        let old = first.old_range().start..last.old_range().end;
        let new = first.new_range().start..last.new_range().end;
        if out.is_empty() {
            out.push_str("--- before\n+++ after\n");
        }
        out.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            old.start + 1,
            old.len(),
            new.start + 1,
            new.len()
        ));
        for op in &group {
            for change in diff.iter_changes(op) {
                // A place past the end (an empty text has no lines) is an
                // empty line, not a crash.
                fn of<'a>(lines: &[&'a str], at: Option<usize>) -> &'a str {
                    at.and_then(|at| lines.get(at)).copied().unwrap_or_default()
                }
                let (sign, line) = match change.tag() {
                    similar::ChangeTag::Equal => (' ', of(&after_lines, change.new_index())),
                    similar::ChangeTag::Delete => ('-', of(&before_lines, change.old_index())),
                    similar::ChangeTag::Insert => ('+', of(&after_lines, change.new_index())),
                };
                out.push(sign);
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out
}

/// GitHub-flavored Markdown to HTML.
pub fn markdown(input: &Input) -> Result<String> {
    markdown::to_html_with_options(&input.text, &markdown::Options::gfm())
        .map_err(|err| anyhow::anyhow!("{err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regex_diff_markdown() {
        let found = regex(&Input {
            second: "id=12, id=7".into(),
            ..Input::new(r"id=(?P<n>\d+)")
        })
        .unwrap();
        assert!(found.starts_with("2 matches") && found.contains("n: “7”"));
        assert!(regex(&Input::new("(")).is_err());
        // Look-around and back-references, as in JavaScript.
        let price = pattern(r"(?<=\$)\d+", Flags::default()).unwrap();
        let (found, more) = find_all(&price, "$12 and 7 and $3").unwrap();
        assert_eq!((found.len(), more), (2, false));
        assert_eq!(found[0].range, 1..3);
        let twice = pattern(
            r"(\w)\1",
            Flags {
                ignore_case: true,
                ..Flags::default()
            },
        )
        .unwrap();
        assert_eq!(find_all(&twice, "bOok").unwrap().0.len(), 1);
        assert_eq!(
            replace_all(&price, "$12 and $3", "[$0]").unwrap(),
            "$[12] and $[3]"
        );
        let every = pattern(".", Flags::default()).unwrap();
        let (found, more) = find_all(&every, &"x".repeat(2000)).unwrap();
        assert_eq!((found.len(), more), (MAX_MATCHES, true));
        let changed = diff(&Input {
            second: "a\nc\n".into(),
            ..Input::new("a\nb\n")
        })
        .unwrap();
        assert!(changed.contains("-b") && changed.contains("+c"));
        // Spaces alone do not count when asked, and a missing last line
        // break never does.
        assert_eq!(patch("a  b\nc", "a b\nc\n", 3, true), "");
        assert_eq!(patch("x", "x\n", 3, false), "");
        let spaced = patch("a\nb  c\nd\n", "a\nb c\nD\n", 3, true);
        assert!(spaced.contains(" b c\n-d\n+D"), "{spaced}");
        assert!(
            markdown(&Input::new("# Hi\n\n- [x] done"))
                .unwrap()
                .contains("<h1>Hi</h1>")
        );
    }
}

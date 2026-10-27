//! Checking a finding against the code: the lines it quotes as evidence
//! must be in the file. A quote found moves the finding to the lines it was
//! copied from (agents miscount lines); one not found marks the finding as
//! unverified, the most common false positive being code that is not there.

/// Where `evidence` is in `text` (1-based first and last line), the place
/// nearest `near` when it is in several. Whitespace and the first and last
/// quoted lines' partial matches are forgiven; `None` when not found or
/// when the quote is too short to tell.
pub fn locate(text: &str, evidence: &str, near: Option<u64>) -> Option<(u64, u64)> {
    let quote: Vec<String> = evidence
        .lines()
        .map(squash)
        .filter(|line| !line.is_empty())
        // Diff markers the agent copied along.
        .map(|line| {
            line.strip_prefix(['+', '-'])
                .map(str::trim)
                .unwrap_or(&line)
                .to_string()
        })
        .filter(|line| !line.is_empty() && !line.starts_with("..."))
        .collect();
    let total: usize = quote.iter().map(String::len).sum();
    if quote.is_empty() || total < 6 {
        return None;
    }
    let lines: Vec<String> = text.lines().map(squash).collect();
    let mut found: Vec<(u64, u64)> = Vec::new();
    for start in 0..lines.len() {
        // Find each quoted line in order, skipping blank lines between.
        let mut at = start;
        let mut first = None;
        let mut matched = true;
        for (ix, wanted) in quote.iter().enumerate() {
            while at < lines.len() && lines[at].is_empty() {
                at += 1;
            }
            let Some(line) = lines.get(at) else {
                matched = false;
                break;
            };
            let edge = ix == 0 || ix + 1 == quote.len();
            let ok =
                line == wanted || (edge && wanted.len() >= 6 && line.contains(wanted.as_str()));
            if !ok {
                matched = false;
                break;
            }
            first.get_or_insert(at);
            at += 1;
        }
        if matched && let Some(first) = first {
            found.push((first as u64 + 1, at as u64));
        }
    }
    let near = near.unwrap_or(0);
    found
        .into_iter()
        .min_by_key(|(start, _)| start.abs_diff(near))
}

/// A line without its indentation and with runs of whitespace as one space.
fn squash(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "fn main() {\n    let retries = 3;\n\n    loop {\n        call();\n    }\n}\n\nfn other() {\n    loop {\n        call();\n    }\n}\n";

    #[test]
    fn finds_the_quoted_lines_nearest_the_reported_place() {
        assert_eq!(locate(FILE, "let retries = 3;", Some(9)), Some((2, 2)));
        // Indentation, blank lines and diff markers are forgiven.
        assert_eq!(
            locate(FILE, "+ loop {\n+     call();", Some(1)),
            Some((4, 5))
        );
        // The same code twice: the one nearest the reported line.
        assert_eq!(
            locate(FILE, "loop {\n  call();\n}", Some(11)),
            Some((10, 12))
        );
    }

    #[test]
    fn code_that_is_not_there_is_not_found() {
        assert_eq!(locate(FILE, "let retries = 5;", None), None);
        assert_eq!(locate(FILE, "}", None), None, "too short to tell");
        assert_eq!(locate(FILE, "", None), None);
    }
}

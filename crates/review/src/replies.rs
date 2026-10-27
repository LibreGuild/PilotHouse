//! Answers to findings on GitHub: a reply on a finding's line, or a
//! conversation comment naming it (`F3: …`), that starts with *Fixed*,
//! *Not a problem* (or *False positive*) or *Won't fix*, and why. Anything
//! else is conversation, shown with the finding but changing nothing.

/// What a reply says about a finding: the status it claims, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// `fixed`, `not_problem` or `wont_fix`.
    pub status: &'static str,
    pub reason: String,
}

/// The claim in `text`, if it makes one.
pub fn classify(text: &str) -> Option<Answer> {
    let text = without_label(text.trim());
    let lower = text.to_ascii_lowercase();
    const WORDS: [(&str, &[&str]); 3] = [
        ("fixed", &["fixed", "done", "resolved", "addressed"]),
        (
            "not_problem",
            &[
                "not a problem",
                "false positive",
                "not an issue",
                "not a bug",
                "not applicable",
            ],
        ),
        (
            "wont_fix",
            &["won't fix", "wont fix", "will not fix", "won’t fix"],
        ),
    ];
    for (status, words) in WORDS {
        for word in words {
            if lower.starts_with(word) {
                let rest = text[word.len()..]
                    .trim_start_matches(|c: char| {
                        c == ':'
                            || c == '-'
                            || c == '—'
                            || c == '.'
                            || c == ','
                            || c.is_whitespace()
                    })
                    .trim();
                // "fixed in abc123" keeps the commit as the reason.
                return Some(Answer {
                    status,
                    reason: rest.to_string(),
                });
            }
        }
    }
    None
}

/// The finding a conversation comment is about (`F3: …`, `**F3** …`,
/// `@F3 …`), and the rest of it.
pub fn label_of(text: &str) -> Option<(String, &str)> {
    let trimmed = text.trim().trim_start_matches(['*', '@', '#']);
    let digits: String = trimmed
        .strip_prefix(['F', 'f'])?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    if digits.is_empty() {
        return None;
    }
    let rest = trimmed[1 + digits.len()..].trim_start_matches(['*', ':', ' ', '-', '—']);
    Some((format!("F{digits}"), rest))
}

fn without_label(text: &str) -> &str {
    match label_of(text) {
        Some((_, rest)) => rest,
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_claims_and_their_reasons() {
        assert_eq!(
            classify("Not a problem: the gateway validates this."),
            Some(Answer {
                status: "not_problem",
                reason: "the gateway validates this.".into()
            })
        );
        assert_eq!(
            classify("False positive — see auth.rs").unwrap().status,
            "not_problem"
        );
        assert_eq!(classify("Fixed in abc123").unwrap().reason, "in abc123");
        assert_eq!(
            classify("won't fix, out of scope").unwrap().reason,
            "out of scope"
        );
        assert_eq!(classify("**F3**: fixed").unwrap().status, "fixed");
        assert_eq!(classify("Why is this a problem?"), None);
    }

    #[test]
    fn finds_the_label_a_comment_is_about() {
        assert_eq!(
            label_of("F3: not a problem"),
            Some(("F3".into(), "not a problem"))
        );
        assert_eq!(label_of("**F12** fixed"), Some(("F12".into(), "fixed")));
        assert_eq!(label_of("Fine by me"), None);
        // What the author's PilotHouse posts for a finding not on the code.
        let posted = "**F6**: Won't fix: The sleep is cheap here; leaving it.";
        assert_eq!(label_of(posted).unwrap().0, "F6");
        assert_eq!(
            classify(posted),
            Some(Answer {
                status: "wont_fix",
                reason: "The sleep is cheap here; leaving it.".into()
            })
        );
    }
}

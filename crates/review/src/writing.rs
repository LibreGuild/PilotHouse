//! How a finding reads: written for someone who has not read the code, in
//! three parts kept in one Markdown text (what happens, why it matters, an
//! example), and the rules an agent is given for writing them.
//!
//! The parts live in the finding's body under fixed labels, so the same
//! text reads well on GitHub and can be taken apart again for the card.

/// The parts of a finding's text, in the order shown.
pub const WHAT: &str = "What happens";
pub const WHY: &str = "Why it matters";
pub const EXAMPLE: &str = "Example";

const LABELS: [&str; 3] = [WHAT, WHY, EXAMPLE];

/// One part of a finding's text: its label, when it has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub label: Option<&'static str>,
    pub text: String,
}

/// The finding's text from its parts; a part left empty is left out.
pub fn compose(what: &str, why: &str, example: &str) -> String {
    [(WHAT, what), (WHY, why), (EXAMPLE, example)]
        .iter()
        .filter(|(_, text)| !text.trim().is_empty())
        .map(|(label, text)| format!("**{label}:** {}", text.trim()))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The label a paragraph starts with (`**What happens:** …`), and the rest.
fn labelled(paragraph: &str) -> Option<(&'static str, &str)> {
    let rest = paragraph.trim_start().strip_prefix("**")?;
    let (name, after) = rest.split_once("**")?;
    let name = name.trim().trim_end_matches([':', '.']).trim();
    let label = LABELS
        .into_iter()
        .find(|label| label.eq_ignore_ascii_case(name))?;
    Some((label, after.trim_start_matches([':', '.']).trim_start()))
}

/// `body` as its parts. Text written before the format (or by an agent that
/// did not keep to it) is one part without a label.
pub fn sections(body: &str) -> Vec<Section> {
    let mut sections: Vec<Section> = Vec::new();
    let mut fenced = false;
    for paragraph in body.trim().split("\n\n") {
        // A blank line inside a code fence does not end a part.
        let starts = !fenced;
        if paragraph.matches("```").count() % 2 == 1 {
            fenced = !fenced;
        }
        match labelled(paragraph).filter(|_| starts) {
            Some((label, text)) => sections.push(Section {
                label: Some(label),
                text: text.to_string(),
            }),
            None => match sections.last_mut() {
                Some(last) => {
                    last.text.push_str("\n\n");
                    last.text.push_str(paragraph);
                }
                None => sections.push(Section {
                    label: None,
                    text: paragraph.to_string(),
                }),
            },
        }
    }
    sections.retain(|section| !section.text.trim().is_empty());
    sections
}

/// What an agent is told about writing findings, whatever the template: the
/// reader is a person deciding in a minute whether this matters.
pub const RULES: &str = "\
Each finding is read by a teammate who has not read this code and decides in a minute whether it matters. Write for them:
- `title`: one plain sentence saying what goes wrong and for whom or when: the effect, not the code. Good: \"A shopper whose cart was saved before the last update cannot check out\". Bad: \"Unhandled error in restore()\", \"Possible issue with session handling\".
- `what`: what the code does now and what it should do instead, in one to three short sentences. Name functions and files in backticks, and say what they do.
- `why`: what it costs in the real world: who notices, and what they see, lose or wait for. If nobody would notice, say what it makes harder later.
- `example`: one concrete case that triggers it, with real values or steps, and what happens then (\"A cart saved on 3 March, opened after the session expired: `restore` fails and the order is refused\").
- `suggestion`: what to change, in a sentence or two; put code in `suggested_code`.
- Short sentences and everyday words. Explain a term a newcomer to this code would not know, in a few words. No \"could potentially\", \"consider\" or \"it seems\": say what happens and when. No praise, no repeating the title.

Before you answer, go through your findings once more and try to prove each one wrong: read the callers, the configuration and the tests again. Drop what does not hold. A finding that is wrong costs the reader more than a small one you leave out.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composes_and_takes_apart() {
        let body = compose(
            "`restore` fails on an old cart.",
            "The shopper cannot check out.",
            "",
        );
        assert_eq!(
            body,
            "**What happens:** `restore` fails on an old cart.\n\n**Why it matters:** The shopper cannot check out."
        );
        let parts = sections(&body);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].label, Some(WHAT));
        assert_eq!(parts[1].text, "The shopper cannot check out.");
    }

    #[test]
    fn keeps_text_without_labels_and_code_whole() {
        let parts = sections("Plain text.\n\nMore of it.");
        assert_eq!(
            parts,
            [Section {
                label: None,
                text: "Plain text.\n\nMore of it.".into()
            }]
        );
        // A label inside a code block is code; a later paragraph joins the
        // part before it.
        let parts = sections(
            "**what happens.** It breaks:\n\n```\na\n\n**Example:** not a label\n```\n\n**Example:** x = 0",
        );
        assert_eq!(parts.len(), 2);
        assert!(parts[0].text.contains("not a label"));
        assert_eq!(parts[1].label, Some(EXAMPLE));
        assert_eq!(parts[1].text, "x = 0");
    }
}

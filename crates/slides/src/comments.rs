//! Comments on a deck: what the user (or the agent answering) wrote about a
//! slide or one thing on it. Kept in `comments.json` beside the deck's
//! `site/` folder, so they stay with the deck and are never served to the
//! people it is shared with.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const FILE: &str = "comments.json";
/// A comment's text, at most. More is a document, not a comment.
const MAX_TEXT: usize = 4_000;
const MAX_COMMENTS: usize = 2_000;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Comment {
    /// Its own in this deck (`c7`).
    pub id: String,
    /// The slide it is on, by its file.
    pub file: String,
    /// The thing on the slide it is about (its `data-id`), or empty for the
    /// slide as a whole.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub element: String,
    /// What that thing was when it was written ("Heading \"Our plan\""), to
    /// read without the slide, and once the thing is gone.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub label: String,
    pub author: String,
    pub text: String,
    /// Seconds since 1970.
    pub at: u64,
    /// Sent to the agent when it was written.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub agent: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub resolved: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub replies: Vec<Reply>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Reply {
    pub author: String,
    pub text: String,
    pub at: u64,
}

/// The deck's comments (`folder` is its artifact's), oldest first.
pub fn read(folder: &Path) -> Vec<Comment> {
    std::fs::read(folder.join(FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write(folder: &Path, comments: &[Comment]) -> Result<()> {
    let json = serde_json::to_vec_pretty(comments)?;
    ph_artifacts::library::write_atomic(&folder.join(FILE), &json)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|now| now.as_secs())
        .unwrap_or_default()
}

fn text_of(op: &Value) -> Result<String> {
    let text = op["text"].as_str().unwrap_or_default().trim();
    if text.is_empty() {
        bail!("a comment says something");
    }
    Ok(text.chars().take(MAX_TEXT).collect())
}

/// What a change did: every comment as it is now, and the one it touched.
pub struct Changed {
    pub all: Vec<Comment>,
    pub touched: Option<Comment>,
}

/// Changes the deck's comments as `op` says, written by `author`:
/// `{op: "add", file, element, label, text, agent}`, `{op: "reply", id,
/// text}`, `{op: "resolve", id, resolved}`, `{op: "delete", id}`.
/// One caller at a time (the caller holds the deck's lock).
pub fn apply(folder: &Path, op: &Value, author: &str) -> Result<Changed> {
    let mut all = read(folder);
    let kind = op["op"].as_str().unwrap_or_default();
    let find = |all: &[Comment]| -> Result<usize> {
        let id = op["id"].as_str().unwrap_or_default();
        all.iter()
            .position(|comment| comment.id == id)
            .with_context(|| format!("there is no comment {id}"))
    };
    let touched = match kind {
        "add" => {
            if all.len() >= MAX_COMMENTS {
                bail!("this deck has too many comments; delete resolved ones first");
            }
            let text = text_of(op)?;
            let number = all
                .iter()
                .filter_map(|comment| comment.id.strip_prefix('c')?.parse::<u64>().ok())
                .max()
                .unwrap_or(0)
                + 1;
            let short = |key: &str, most: usize| -> String {
                op[key]
                    .as_str()
                    .unwrap_or_default()
                    .chars()
                    .take(most)
                    .collect()
            };
            let comment = Comment {
                id: format!("c{number}"),
                file: short("file", 200),
                element: short("element", 80),
                label: short("label", 200),
                author: author.to_string(),
                text,
                at: now(),
                agent: op["agent"].as_bool().unwrap_or(false),
                resolved: false,
                replies: Vec::new(),
            };
            if comment.file.is_empty() {
                bail!("a comment is on a slide");
            }
            all.push(comment.clone());
            Some(comment)
        }
        "reply" => {
            let text = text_of(op)?;
            let at = find(&all)?;
            all[at].replies.push(Reply {
                author: author.to_string(),
                text,
                at: now(),
            });
            if let Some(agent) = op["agent"].as_bool() {
                all[at].agent = agent;
            }
            Some(all[at].clone())
        }
        "resolve" => {
            let at = find(&all)?;
            all[at].resolved = op["resolved"].as_bool().unwrap_or(true);
            Some(all[at].clone())
        }
        "delete" => {
            let at = find(&all)?;
            all.remove(at);
            None
        }
        other => bail!("comments take add, reply, resolve and delete, not `{other}`"),
    };
    write(folder, &all)?;
    Ok(Changed { all, touched })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn comments_are_added_answered_resolved_and_deleted() {
        let folder =
            std::env::temp_dir().join(format!("ph-slides-comments-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        assert!(read(&folder).is_empty());

        let added = apply(
            &folder,
            &json!({"op": "add", "file": "slides/a.html", "element": "t1", "label": "Heading", "text": " Shorter ", "agent": true}),
            "You",
        )
        .unwrap();
        let first = added.touched.unwrap();
        assert_eq!(
            (first.id.as_str(), first.text.as_str(), first.agent),
            ("c1", "Shorter", true)
        );
        apply(
            &folder,
            &json!({"op": "add", "file": "slides/b.html", "text": "Second"}),
            "You",
        )
        .unwrap();

        let answered = apply(
            &folder,
            &json!({"op": "reply", "id": "c1", "text": "Done"}),
            "Agent",
        )
        .unwrap();
        assert_eq!(answered.touched.unwrap().replies[0].author, "Agent");
        apply(&folder, &json!({"op": "resolve", "id": "c1"}), "Agent").unwrap();
        let all = read(&folder);
        assert!(all[0].resolved && !all[1].resolved);
        // Deleting the first leaves the second its id; the next gets a new one.
        apply(&folder, &json!({"op": "delete", "id": "c1"}), "You").unwrap();
        let next = apply(
            &folder,
            &json!({"op": "add", "file": "slides/a.html", "text": "Third"}),
            "You",
        )
        .unwrap();
        assert_eq!(next.touched.unwrap().id, "c3");

        for bad in [
            json!({"op": "add", "file": "slides/a.html", "text": "  "}),
            json!({"op": "add", "text": "No slide"}),
            json!({"op": "reply", "id": "c9", "text": "x"}),
            json!({"op": "shout"}),
        ] {
            assert!(apply(&folder, &bad, "You").is_err(), "{bad}");
        }
        assert_eq!(read(&folder).len(), 2);
        let _ = std::fs::remove_dir_all(&folder);
    }
}

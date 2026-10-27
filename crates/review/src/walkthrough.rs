//! A pull request explained in three short parts, read in a minute:
//!
//! 1. **Gist**: what it does and why (saying where the why comes from: the
//!    description, the commits, or a guess), before and after.
//! 2. **At a glance**: the changed files grouped by purpose (core logic,
//!    API, tests, config…), each group a chip that filters the Files tab.
//! 3. **Tour**: three to six stops in reading order, each a few lines of
//!    the real code with one sentence; the tour runs inside the Files tab.
//!
//! Optionally one small flow (who calls what), which PilotHouse draws
//! itself from the agent's steps, so it is always valid.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Most tour stops kept.
pub const MAX_STOPS: usize = 6;
/// Most lines a stop points at.
const MAX_STOP_LINES: u64 = 40;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Walkthrough {
    pub gist: Gist,
    pub groups: Vec<Group>,
    pub tour: Vec<Stop>,
    #[serde(default)]
    pub flow: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Gist {
    pub what: String,
    pub why: String,
    /// `description`, `commits` or `guess`.
    pub why_from: String,
    pub before: String,
    pub after: String,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Group {
    pub name: String,
    /// `core`, `api`, `ui`, `data`, `tests`, `config`, `docs`, `generated`
    /// or `other`.
    pub kind: String,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Stop {
    pub title: String,
    pub path: String,
    pub start: u64,
    pub end: u64,
    pub note: String,
}

/// One arrow of the flow: `from` → `to`, labelled.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Step {
    pub from: String,
    pub to: String,
    pub label: String,
}

/// The walkthrough in `reply` (the last JSON block with a `gist` or
/// `tour`), read leniently.
pub fn parse(reply: &str) -> Option<Walkthrough> {
    let value = crate::parse::json_values(reply)
        .into_iter()
        .rev()
        .find(|value| value.get("gist").is_some() || value.get("tour").is_some())?;
    let text = |value: &Value, keys: &[&str]| -> String {
        keys.iter()
            .find_map(|key| value.get(*key).and_then(Value::as_str))
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    let gist = value.get("gist").cloned().unwrap_or(Value::Null);
    let why_from = text(&gist, &["why_from", "source"]).to_ascii_lowercase();
    let mut walkthrough = Walkthrough {
        gist: Gist {
            what: text(&gist, &["what", "summary", "tldr"]),
            why: text(&gist, &["why", "reason"]),
            why_from: if why_from.contains("desc") {
                "description"
            } else if why_from.contains("commit") {
                "commits"
            } else {
                "guess"
            }
            .into(),
            before: text(&gist, &["before"]),
            after: text(&gist, &["after"]),
        },
        groups: value
            .get("groups")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|group| {
                let files: Vec<String> = match group.get("files") {
                    Some(Value::Array(files)) => files
                        .iter()
                        .filter_map(Value::as_str)
                        .map(|path| path.trim().trim_start_matches("./").to_string())
                        .collect(),
                    Some(Value::String(one)) => vec![one.trim().to_string()],
                    _ => Vec::new(),
                };
                let name = text(group, &["name", "title"]);
                (!name.is_empty() && !files.is_empty()).then(|| Group {
                    kind: kind_of(&text(group, &["kind", "type"]), &name),
                    name,
                    files,
                })
            })
            .collect(),
        tour: value
            .get("tour")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|stop| {
                let path = text(stop, &["path", "file"])
                    .trim_start_matches("./")
                    .to_string();
                let number = |key: &str| match stop.get(key) {
                    Some(Value::Number(n)) => n.as_u64(),
                    Some(Value::String(s)) => s.trim().parse().ok(),
                    _ => None,
                };
                let start = number("start").or_else(|| number("line"))?;
                let end = number("end").unwrap_or(start);
                let (start, end) = (start.min(end).max(1), start.max(end).max(1));
                (!path.is_empty()).then(|| Stop {
                    title: text(stop, &["title", "name"]),
                    path,
                    start,
                    end: end.min(start + MAX_STOP_LINES),
                    note: text(stop, &["note", "why", "text"]),
                })
            })
            .take(MAX_STOPS)
            .collect(),
        flow: value
            .get("flow")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|step| {
                let (from, to) = (text(step, &["from"]), text(step, &["to"]));
                (!from.is_empty() && !to.is_empty()).then(|| Step {
                    from,
                    to,
                    label: text(step, &["label", "what"]),
                })
            })
            .take(8)
            .collect(),
    };
    if walkthrough.gist.what.is_empty() && walkthrough.tour.is_empty() {
        return None;
    }
    if walkthrough.gist.what.is_empty() {
        walkthrough.gist.what = walkthrough.tour[0].note.clone();
    }
    Some(walkthrough)
}

fn kind_of(kind: &str, name: &str) -> String {
    let both = format!("{kind} {name}").to_ascii_lowercase();
    for (word, kind) in [
        ("test", "tests"),
        ("generat", "generated"),
        ("doc", "docs"),
        ("config", "config"),
        ("build", "config"),
        ("api", "api"),
        ("ui", "ui"),
        ("view", "ui"),
        ("data", "data"),
        ("schema", "data"),
        ("core", "core"),
        ("logic", "core"),
    ] {
        if both.contains(word) {
            return kind.into();
        }
    }
    "other".into()
}

impl Walkthrough {
    /// Drops stops in files not in the change, and keeps lines inside the
    /// file (`lines` gives a file's line count at the reviewed commit).
    pub fn check(&mut self, changed: &[String], lines: &dyn Fn(&str) -> Option<u64>) {
        self.tour.retain(|stop| changed.contains(&stop.path));
        for stop in &mut self.tour {
            if let Some(count) = lines(&stop.path) {
                stop.end = stop.end.min(count.max(1));
                stop.start = stop.start.min(stop.end);
            }
        }
        // Files the groups do not name join "Other".
        let named: Vec<&String> = self.groups.iter().flat_map(|g| &g.files).collect();
        let rest: Vec<String> = changed
            .iter()
            .filter(|path| !named.contains(path))
            .cloned()
            .collect();
        for group in &mut self.groups {
            group.files.retain(|path| changed.contains(path));
        }
        self.groups.retain(|group| !group.files.is_empty());
        if !rest.is_empty() {
            self.groups.push(Group {
                name: "Other".into(),
                kind: "other".into(),
                files: rest,
            });
        }
    }

    /// The flow as a Mermaid diagram (labels made safe), when it has one.
    pub fn mermaid(&self) -> Option<String> {
        if self.flow.is_empty() {
            return None;
        }
        let mut names: Vec<String> = Vec::new();
        let id = |name: &str, names: &mut Vec<String>| -> String {
            let at = names
                .iter()
                .position(|known| known == name)
                .unwrap_or_else(|| {
                    names.push(name.to_string());
                    names.len() - 1
                });
            format!("n{at}")
        };
        let mut lines = vec!["flowchart LR".to_string()];
        for step in &self.flow {
            let (from, to) = (id(&step.from, &mut names), id(&step.to, &mut names));
            if step.label.is_empty() {
                lines.push(format!("  {from} --> {to}"));
            } else {
                lines.push(format!("  {from} -->|{}| {to}", safe(&step.label)));
            }
        }
        for (at, name) in names.iter().enumerate() {
            lines.push(format!("  n{at}[\"{}\"]", safe(name)));
        }
        Some(lines.join("\n"))
    }

    /// The walkthrough as Markdown, for a PR description or a message.
    pub fn markdown(&self) -> String {
        let mut md = format!("**What it does.** {}\n", self.gist.what);
        if !self.gist.why.is_empty() {
            let from = match self.gist.why_from.as_str() {
                "description" => "from the description",
                "commits" => "from the commits",
                _ => "a guess from the code",
            };
            md.push_str(&format!("\n**Why** ({from}). {}\n", self.gist.why));
        }
        if !self.gist.before.is_empty() || !self.gist.after.is_empty() {
            md.push_str(&format!(
                "\n**Before:** {}  \n**After:** {}\n",
                self.gist.before, self.gist.after
            ));
        }
        if !self.groups.is_empty() {
            md.push_str("\n**At a glance:** ");
            md.push_str(
                &self
                    .groups
                    .iter()
                    .map(|group| format!("{} ({})", group.name, group.files.len()))
                    .collect::<Vec<_>>()
                    .join(" · "),
            );
            md.push('\n');
        }
        if !self.tour.is_empty() {
            md.push_str("\n**Read in this order**\n\n");
            for (ix, stop) in self.tour.iter().enumerate() {
                md.push_str(&format!(
                    "{}. **{}**: `{}:{}-{}`. {}\n",
                    ix + 1,
                    stop.title,
                    stop.path,
                    stop.start,
                    stop.end,
                    stop.note
                ));
            }
        }
        if let Some(diagram) = self.mermaid() {
            md.push_str(&format!("\n```mermaid\n{diagram}\n```\n"));
        }
        md
    }
}

/// A label Mermaid takes as text: no quotes, brackets or pipes.
fn safe(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '"' | '`' => '\'',
            '[' | '{' | '<' => '(',
            ']' | '}' | '>' => ')',
            '|' => '/',
            c => c,
        })
        .take(60)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPLY: &str = r#"Here it is.
```json pilothouse-walkthrough
{"gist": {"what": "Caches users.", "why": "Logins were slow.", "why_from": "the PR description", "before": "Every request read the DB.", "after": "Reads hit a cache."},
 "groups": [{"name": "Core logic", "files": ["src/cache.rs", "./src/user.rs"]}, {"name": "Tests", "kind": "tests", "files": "tests/cache.rs"}],
 "tour": [{"title": "The cache", "path": "src/cache.rs", "start": "40", "end": 10, "note": "Cleared on write."},
          {"title": "Gone", "path": "nope.rs", "line": 1, "note": "x"}],
 "flow": [{"from": "Login \"page\"", "to": "Cache[x]", "label": "reads|"}]}
```"#;

    #[test]
    fn reads_and_checks_a_walkthrough() {
        let mut walkthrough = parse(REPLY).unwrap();
        assert_eq!(walkthrough.gist.why_from, "description");
        assert_eq!(walkthrough.groups[0].kind, "core");
        assert_eq!(walkthrough.groups[1].files, ["tests/cache.rs"]);
        assert_eq!(
            (walkthrough.tour[0].start, walkthrough.tour[0].end),
            (10, 40)
        );
        let changed = vec![
            "src/cache.rs".to_string(),
            "src/user.rs".to_string(),
            "tests/cache.rs".to_string(),
            "README.md".to_string(),
        ];
        walkthrough.check(&changed, &|_| Some(30));
        assert_eq!(walkthrough.tour.len(), 1, "a stop outside the change goes");
        assert_eq!(walkthrough.tour[0].end, 30, "kept inside the file");
        assert_eq!(walkthrough.groups.last().unwrap().files, ["README.md"]);
        let diagram = walkthrough.mermaid().unwrap();
        assert!(diagram.contains("n0 -->|reads/| n1"));
        assert!(diagram.contains("n1[\"Cache(x)\"]"));
        assert!(walkthrough.markdown().contains("1. **The cache**"));
        assert_eq!(parse("no walkthrough"), None);
    }
}

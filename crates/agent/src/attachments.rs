//! Things attached to a message: files, board cards, screenshots, log lines.
//!
//! The composer shows them as chips. On send, each becomes ACP content for
//! the agent and a small summary for the transcript.

use std::path::PathBuf;

use ph_files::roots::Roots;

use agent_client_protocol_schema::v1::{ContentBlock, ImageContent, ResourceLink, TextContent};
use ph_store::Store;
use ph_ui::Icons;

use crate::attachment_view::Preview;
use crate::transcript::AttachmentSummary;

/// Text kept with a sent message for its preview (the agent got all of it).
const SUMMARY_TEXT_LIMIT: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub enum Attachment {
    /// A project file (relative path), sent as a link the agent can read.
    File(String),
    /// A board card, sent as its title, description and acceptance criteria.
    Card(i64),
    /// A PNG on disk (a capture or annotation), sent as an image.
    Image { label: String, path: PathBuf },
    /// Plain text with a label, e.g. selected log lines or an element's details.
    Text { label: String, text: String },
}

impl Attachment {
    pub fn label(&self) -> String {
        match self {
            Attachment::File(path) => path.rsplit('/').next().unwrap_or(path).to_string(),
            Attachment::Card(id) => format!("Card #{id}"),
            Attachment::Image { label, .. } | Attachment::Text { label, .. } => label.clone(),
        }
    }

    pub fn icon(&self) -> Icons {
        match self {
            Attachment::File(path) => ph_ui::file_icon(path).0,
            Attachment::Card(_) => Icons::SquareKanban,
            Attachment::Image { .. } => Icons::Image,
            Attachment::Text { .. } => Icons::ScrollText,
        }
    }

    /// How the attachment is kept with the sent message.
    pub fn summary(&self, roots: &Roots, project_id: i64, store: &Store) -> AttachmentSummary {
        let mut summary = AttachmentSummary {
            label: self.label(),
            image: None,
            text: None,
            file: None,
        };
        match self.preview(roots, project_id, store) {
            Preview::Image(path) => summary.image = Some(path),
            Preview::Text(mut text) => {
                if text.len() > SUMMARY_TEXT_LIMIT {
                    let mut end = SUMMARY_TEXT_LIMIT;
                    while !text.is_char_boundary(end) {
                        end -= 1;
                    }
                    text.truncate(end);
                    text.push_str("\n…");
                }
                summary.text = Some(text);
            }
            Preview::File(path) => summary.file = Some(path),
            Preview::None => {}
        }
        summary
    }

    /// What looking inside the attachment shows.
    pub fn preview(&self, roots: &Roots, project_id: i64, store: &Store) -> Preview {
        match self {
            Attachment::File(path) => match roots.absolute(path) {
                Some(path) => Preview::File(path),
                None => Preview::None,
            },
            Attachment::Card(id) => card_text(store, project_id, *id)
                .map(Preview::Text)
                .unwrap_or(Preview::None),
            Attachment::Image { path, .. } => Preview::Image(path.clone()),
            Attachment::Text { text, .. } => Preview::Text(text.clone()),
        }
    }

    /// The ACP content blocks for this attachment.
    ///
    /// Images go as image content when the agent accepts them, otherwise as a
    /// link to the file so the agent can still open it.
    pub fn to_blocks(
        &self,
        roots: &Roots,
        project_id: i64,
        store: &Store,
        accepts_images: bool,
    ) -> Vec<ContentBlock> {
        match self {
            Attachment::File(path) => {
                let Some(absolute) = roots.absolute(path) else {
                    return Vec::new();
                };
                vec![ContentBlock::ResourceLink(ResourceLink::new(
                    path.clone(),
                    ph_ui::platform::file_url(&absolute),
                ))]
            }
            Attachment::Card(id) => card_text(store, project_id, *id)
                .map(|text| vec![ContentBlock::Text(TextContent::new(text))])
                .unwrap_or_default(),
            Attachment::Image { label, path } => match std::fs::read(path) {
                Ok(bytes) if accepts_images => {
                    vec![ContentBlock::Image(ImageContent::new(
                        base64(&bytes),
                        "image/png",
                    ))]
                }
                _ => vec![ContentBlock::ResourceLink(ResourceLink::new(
                    label.clone(),
                    ph_ui::platform::file_url(path),
                ))],
            },
            Attachment::Text { label, text } => {
                vec![ContentBlock::Text(TextContent::new(format!(
                    "{label}:\n{text}"
                )))]
            }
        }
    }
}

/// A card as text. Only this project's cards: `#12` must not read another
/// board.
fn card_text(store: &Store, project_id: i64, id: i64) -> Option<String> {
    match store.card(id) {
        Ok(Some(card)) if card.project_id == project_id => Some(format!(
            "Board card #{} [{}]: {}\n{}\nAcceptance criteria:\n{}",
            card.id,
            card.status.key(),
            card.title,
            card.description,
            card.acceptance
        )),
        _ => None,
    }
}

/// Live context the workspace provides on demand, for `@screenshot`,
/// `@log` and `@test-run` and the attach menu. Each returns nothing when
/// there is nothing to send (no browser or device is showing anything yet,
/// no test ran).
pub struct LiveSources {
    pub screenshot: Box<LiveSource>,
    pub logs: Box<LiveSource>,
    /// The newest UI test results, and the failing step's screen.
    pub tests: Box<ManyLiveSource>,
}

pub type LiveSource = dyn Fn(&mut gpui_kit::App) -> Option<Attachment>;
/// A source that may give several attachments.
pub type ManyLiveSource = dyn Fn(&mut gpui_kit::App) -> Vec<Attachment>;

/// The live mentions a message contains.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LiveMentions {
    pub screenshot: bool,
    pub logs: bool,
    pub tests: bool,
}

pub fn live_mentions(text: &str) -> LiveMentions {
    let mut found = LiveMentions::default();
    for token in text.split_whitespace() {
        match token.trim_end_matches([',', '.', ';', ':', ')', '?', '!']) {
            "@screenshot" => found.screenshot = true,
            "@log" | "@logs" => found.logs = true,
            "@test-run" | "@tests" => found.tests = true,
            _ => {}
        }
    }
    found
}

/// A pull request, as `@pr:12` attaches it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullBrief {
    pub number: u64,
    pub title: String,
    pub author: String,
    /// `feature → main`.
    pub branches: String,
    pub url: String,
    pub body: String,
}

/// `@pr:12` mentions of known pull requests, as text for the agent.
pub fn pull_mentions(text: &str, pulls: &[PullBrief]) -> Vec<Attachment> {
    let mut found: Vec<Attachment> = Vec::new();
    for token in text.split_whitespace() {
        let token = token.trim_end_matches([',', '.', ';', ':', ')', '?', '!']);
        let Some(number) = token
            .strip_prefix("@pr:")
            .or_else(|| token.strip_prefix("@pr#"))
            .and_then(|number| number.parse::<u64>().ok())
        else {
            continue;
        };
        if let Some(pull) = pulls.iter().find(|pull| pull.number == number) {
            let attachment = Attachment::Text {
                label: format!("PR #{number}"),
                text: format!(
                    "Pull request #{} “{}” by {} ({}), {}\n\n{}\n\nIts changes: `git fetch origin pull/{}/head` then `git diff {}...FETCH_HEAD` (or `gh pr diff {}`).",
                    pull.number,
                    pull.title,
                    pull.author,
                    pull.branches,
                    pull.url,
                    if pull.body.trim().is_empty() {
                        "(no description)"
                    } else {
                        pull.body.trim()
                    },
                    pull.number,
                    pull.branches.rsplit(" → ").next().unwrap_or("HEAD"),
                    pull.number,
                ),
            };
            if !found.contains(&attachment) {
                found.push(attachment);
            }
        }
    }
    found
}

/// Finds `@path` (files that exist) and `#12` (cards) mentions in a message.
pub fn mentions(text: &str, roots: &Roots) -> Vec<Attachment> {
    let mut found = Vec::new();
    for token in text.split_whitespace() {
        let token = token.trim_end_matches([',', '.', ';', ':', ')', '?', '!']);
        if let Some(path) = token.strip_prefix('@') {
            if !path.is_empty() && inside(roots, path) {
                found.push(Attachment::File(path.to_string()));
            } else if let Some(code) = code_at(roots, path) {
                found.push(code);
            }
        } else if let Some(id) = token.strip_prefix('#').and_then(|n| n.parse::<i64>().ok()) {
            found.push(Attachment::Card(id));
        }
    }
    found.dedup();
    found
}

/// `@path:line`: the code starting at that line (a definition, to its end
/// or 80 lines), as text the agent reads at once.
fn code_at(roots: &Roots, mention: &str) -> Option<Attachment> {
    let (path, line) = mention.rsplit_once(':')?;
    let line: usize = line.parse().ok().filter(|line| *line > 0)?;
    if !inside(roots, path) {
        return None;
    }
    let text = std::fs::read_to_string(roots.absolute(path)?).ok()?;
    let snippet = definition(&text, line)?;
    let name = path.rsplit('/').next().unwrap_or(path);
    Some(Attachment::Text {
        label: format!("{name}:{line}"),
        text: format!("{path}, from line {line}:\n```\n{snippet}\n```"),
    })
}

/// The lines from `line` (1-based) to the end of what starts there: until a
/// line indented no deeper than the first (a closing bracket on it is kept).
fn definition(text: &str, line: usize) -> Option<String> {
    const MAX: usize = 80;
    let lines: Vec<&str> = text.lines().skip(line - 1).collect();
    let first = lines.first()?;
    let indent = |line: &str| line.len() - line.trim_start().len();
    let base = indent(first);
    let mut taken = vec![*first];
    for line in lines.iter().skip(1).take(MAX - 1) {
        if line.trim().is_empty() {
            taken.push(line);
            continue;
        }
        if indent(line) <= base {
            if line.trim_start().starts_with(['}', ')', ']']) || line.trim() == "end" {
                taken.push(line);
            }
            break;
        }
        taken.push(line);
    }
    while taken.last().is_some_and(|line| line.trim().is_empty()) {
        taken.pop();
    }
    Some(taken.join("\n"))
}

/// Whether `path` names a file inside one of the workspace's folders (no
/// `..` or absolute escapes, symlinks resolved), so a mention never attaches
/// files elsewhere.
fn inside(roots: &Roots, path: &str) -> bool {
    let Some((root, _)) = roots.split(path) else {
        return false;
    };
    let (Ok(root), Some(Ok(file))) = (
        ph_store::paths::canonical(&root.path),
        roots
            .absolute(path)
            .map(|file| ph_store::paths::canonical(&file)),
    ) else {
        return false;
    };
    file.starts_with(&root) && file.is_file()
}

/// Standard base64 (RFC 4648) without a dependency for one encoder.
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_mention_takes_its_definition() {
        let text = "use x;\n\nfn add(a: i32) -> i32 {\n    a + 1\n}\n\nfn other() {}\n";
        assert_eq!(
            definition(text, 3).unwrap(),
            "fn add(a: i32) -> i32 {\n    a + 1\n}"
        );
        let python =
            "class A:\n    def f(self):\n        return 1\n\n    def g(self):\n        pass\n";
        assert_eq!(
            definition(python, 2).unwrap(),
            "    def f(self):\n        return 1"
        );
        assert!(definition(text, 99).is_none());
    }

    #[test]
    fn base64_matches_rfc_examples() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn finds_file_and_card_mentions() {
        let root = std::env::temp_dir().join(format!("ph-mentions-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "").unwrap();
        let found = mentions(
            "fix @src/main.rs and @nope.rs, see #12. @../outside @/etc/hosts",
            &Roots::single(root.clone()),
        );
        assert_eq!(
            found,
            vec![Attachment::File("src/main.rs".into()), Attachment::Card(12)]
        );
        let found = live_mentions("see @screenshot, and @logs. Why does @test-run fail?");
        assert!(found.screenshot && found.logs && found.tests);
        assert_eq!(live_mentions("email me@log.com"), LiveMentions::default());
    }
}

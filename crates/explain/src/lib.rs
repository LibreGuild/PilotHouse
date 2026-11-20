//! Explanations of changes, in plain words, where the code is read.
//!
//! Agents write the code and review it; what gets lost is the developer's
//! own picture of what happened. This crate shows an explanation an agent
//! wrote (`ph_review::explanation` is the model: the prompt, the shared
//! file, the lookup):
//!
//! - [`ExplanationView`]: beside the changes in the Git tab (the work not
//!   committed yet) and on a commit in History. From here it is asked for,
//!   written again, copied, and added to the next commit as a file for the
//!   team.
//! - [`WhyView`]: *Why is this here?* for a line in the editor: the
//!   explanation of the commit that wrote it, or for a line not committed
//!   yet, the request in a chat that wrote it.
//!
//! Nothing here starts an agent: the views ask ([`ExplanationEvent::Ask`]),
//! and whoever shows them starts the chat.

mod view;
mod why;

use std::path::Path;

pub use view::{ExplanationEvent, ExplanationView, Target};
pub use why::{Answer, Why, WhyEvent, WhyView, look_up};

use ph_review::explanation::find::Found;

/// Who wrote `found` and when, in a line: "Claude Code · 3 days ago", and
/// for a shared one whose author is known, "Jane with Claude Code · …".
fn byline(found: &Found) -> String {
    let who = match (found.author.trim(), found.agent.trim()) {
        ("", "") => String::new(),
        ("", agent) => agent.to_string(),
        (author, "") => author.to_string(),
        (author, agent) => format!("{author} with {agent}"),
    };
    let when = ph_ui::time::ago(found.written);
    if who.is_empty() {
        when
    } else {
        format!("{who} · {when}")
    }
}

/// `markdown` with its code references made links to the files they name in
/// `top` (the repository's folder), opened by whoever shows it. A reference
/// to a file that is not there stays code.
fn linked(top: &Path, markdown: &str) -> String {
    ph_review::walkthrough::link_references(markdown, |path, start, _| {
        top.join(path)
            .is_file()
            .then(|| ph_review::walkthrough::file_link(path, start))
    })
}

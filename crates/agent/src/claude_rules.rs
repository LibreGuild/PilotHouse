//! What Claude Code may do in a folder without asking: the rules an
//! "Always allow" answer leaves behind.
//!
//! An approval answered *Always* is kept by the agent, not by PilotHouse:
//! Claude Code writes a rule (`Bash(cargo test:*)`) to the folder's
//! `.claude/settings.local.json`, and never asks for that again. A rule
//! that cannot be seen cannot be taken back, so this reads them, and takes
//! one out of that file when asked. Rules in `.claude/settings.json` are
//! the project's (shared with the team, in its repository): shown, and
//! left to be changed there.
//!
//! The other agents keep such answers for the session only, or in files
//! of their own that this does not read.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Context, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div,
};
use serde_json::Value;

/// Where a rule is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kept {
    /// This computer's own settings for the folder: where an "Always
    /// allow" answer goes, and what can be taken back here.
    Here,
    /// The project's settings, shared through its repository.
    Project,
}

/// One thing Claude Code may do in a folder without asking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// As Claude Code writes it: `Bash(cargo test:*)`, `Edit`, `WebFetch(domain:docs.rs)`.
    pub text: String,
    pub kept: Kept,
}

fn file(root: &Path, kept: Kept) -> PathBuf {
    root.join(".claude").join(match kept {
        Kept::Here => "settings.local.json",
        Kept::Project => "settings.json",
    })
}

fn allow_list(settings: &Value) -> Vec<String> {
    settings["permissions"]["allow"]
        .as_array()
        .map(|rules| {
            rules
                .iter()
                .filter_map(|rule| rule.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The rules for folder `root`: this computer's first, then the project's.
/// A file that is not there, or not what Claude Code writes, has none.
pub fn rules(root: &Path) -> Vec<Rule> {
    [Kept::Here, Kept::Project]
        .into_iter()
        .flat_map(|kept| {
            let settings: Value = std::fs::read_to_string(file(root, kept))
                .ok()
                .and_then(|text| serde_json::from_str(&text).ok())
                .unwrap_or(Value::Null);
            allow_list(&settings)
                .into_iter()
                .map(move |text| Rule { text, kept })
        })
        .collect()
}

/// Takes rule `text` out of this computer's settings for `root`: Claude
/// Code asks for that action again. Everything else in the file stays as
/// it is.
pub fn take_back(root: &Path, text: &str) -> Result<()> {
    let path = file(root, Kept::Here);
    let read =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let mut settings: Value =
        serde_json::from_str(&read).with_context(|| format!("reading {}", path.display()))?;
    let Some(allow) = settings["permissions"]["allow"].as_array_mut() else {
        return Ok(());
    };
    allow.retain(|rule| rule.as_str() != Some(text));
    let mut written = serde_json::to_string_pretty(&settings)?;
    written.push('\n');
    std::fs::write(&path, written).with_context(|| format!("writing {}", path.display()))
}

/// The rules of one folder, to read and to take back.
pub struct RulesView {
    root: PathBuf,
    rules: Vec<Rule>,
    error: Option<SharedString>,
}

impl RulesView {
    pub fn new(root: PathBuf) -> Self {
        let rules = rules(&root);
        Self {
            root,
            rules,
            error: None,
        }
    }

    fn take_back(&mut self, text: &str, cx: &mut Context<Self>) {
        self.error = take_back(&self.root, text)
            .err()
            .map(|err| format!("Could not take it back: {err:#}").into());
        self.rules = rules(&self.root);
        cx.notify();
    }
}

impl Render for RulesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rules.iter().enumerate().map(|(ix, rule)| {
            let text = rule.text.clone();
            h_flex()
                .gap_2()
                .px_2()
                .h_8()
                .rounded(cx.theme().radius)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_xs()
                        .child(rule.text.clone()),
                )
                .map(|row| match rule.kept {
                    Kept::Here => row.child(
                        Button::new(("rule-take-back", ix))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .tooltip("Take it back: Claude Code asks again")
                            .accessibility_label(format!("Take back {}", rule.text))
                            .on_click(cx.listener(move |this, _, _, cx| this.take_back(&text, cx))),
                    ),
                    Kept::Project => row.child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("the project's"),
                    ),
                })
        });
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        "What Claude Code does in this folder without asking. An approval answered \"always\" adds a rule here (.claude/settings.local.json). The project's own rules are in .claude/settings.json.",
                    ),
            )
            .when(self.rules.is_empty(), |this| {
                this.child(
                    div()
                        .py_3()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Nothing is always allowed here: Claude Code asks, as its mode says."),
                )
            })
            .child(
                v_flex()
                    .id("claude-rules")
                    .max_h(ph_ui::rems_from_px(360.))
                    .overflow_y_scroll()
                    .children(rows),
            )
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(error),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_always_allowed_action_can_be_seen_and_taken_back() {
        let root = std::env::temp_dir().join(format!("ph-claude-rules-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // No settings at all: no rules, and nothing to take back.
        assert!(rules(&root).is_empty());
        assert!(take_back(&root, "Bash(ls:*)").is_err());

        std::fs::create_dir_all(root.join(".claude")).unwrap();
        std::fs::write(
            root.join(".claude/settings.local.json"),
            r#"{
  "permissions": {
    "allow": ["Bash(cargo test:*)", "Edit", "WebFetch(domain:docs.rs)"],
    "deny": ["Bash(rm -rf:*)"]
  },
  "enableAllProjectMcpServers": true
}"#,
        )
        .unwrap();
        std::fs::write(
            root.join(".claude/settings.json"),
            r#"{ "permissions": { "allow": ["Bash(npm run lint)"] } }"#,
        )
        .unwrap();
        let found = rules(&root);
        assert_eq!(
            found
                .iter()
                .map(|rule| (rule.text.as_str(), rule.kept))
                .collect::<Vec<_>>(),
            vec![
                ("Bash(cargo test:*)", Kept::Here),
                ("Edit", Kept::Here),
                ("WebFetch(domain:docs.rs)", Kept::Here),
                ("Bash(npm run lint)", Kept::Project),
            ]
        );

        // Taken back: that rule goes, and nothing else in the file.
        take_back(&root, "Edit").unwrap();
        let settings: Value = serde_json::from_str(
            &std::fs::read_to_string(root.join(".claude/settings.local.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            settings["permissions"]["allow"],
            serde_json::json!(["Bash(cargo test:*)", "WebFetch(domain:docs.rs)"])
        );
        assert_eq!(
            settings["permissions"]["deny"],
            serde_json::json!(["Bash(rm -rf:*)"])
        );
        assert_eq!(settings["enableAllProjectMcpServers"], true);
        // The project's rules are not this computer's to change.
        take_back(&root, "Bash(npm run lint)").unwrap();
        assert!(
            rules(&root)
                .iter()
                .any(|rule| rule.text == "Bash(npm run lint)" && rule.kept == Kept::Project)
        );
        // A file that is not settings is left alone.
        std::fs::write(root.join(".claude/settings.local.json"), "not json").unwrap();
        assert!(take_back(&root, "Edit").is_err());
        assert_eq!(
            std::fs::read_to_string(root.join(".claude/settings.local.json")).unwrap(),
            "not json"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}

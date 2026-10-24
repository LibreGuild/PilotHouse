//! What each workspace is doing, for the switcher and the title bar: read
//! from its chats and test runs, which live outside any window, so a
//! workspace that is not on screen still says it is working, needs an
//! answer, failed, or finished something not seen yet.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{App, Hsla};
use ph_store::projects::ProjectId;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorkspaceStatus {
    /// Chats working, and a test run going.
    pub working: usize,
    /// Agents waiting for an answer.
    pub waiting: usize,
    /// The newest test run failed.
    pub failed: bool,
    /// Chats and test runs that finished while the workspace was away.
    pub unseen: usize,
}

impl WorkspaceStatus {
    pub fn of(project_id: ProjectId, cx: &App) -> Self {
        let mut status = Self::default();
        if let Some(chats) = ph_agent::ChatHub::existing(project_id, cx) {
            let activity = chats.read(cx).activity(cx);
            status.working += activity.working;
            status.waiting += activity.waiting;
            status.unseen += activity.unseen;
        }
        if let Some(runs) = ph_uitest::runner::TestRunHub::existing(project_id, cx) {
            let runs = runs.read(cx);
            status.working += usize::from(runs.running().is_some());
            status.failed = runs.failed();
            status.unseen += usize::from(runs.unseen());
        }
        status
    }

    /// Something here wants the user: a question, or news not seen yet.
    pub fn wants_you(&self) -> bool {
        self.waiting > 0 || self.unseen > 0
    }

    /// The most urgent state, in a few words, and its colour.
    pub fn summary(&self, cx: &App) -> Option<(String, Hsla)> {
        let theme = cx.theme();
        if self.waiting > 0 {
            let text = match self.waiting {
                1 => "Needs your answer".to_string(),
                n => format!("{n} questions for you"),
            };
            return Some((text, theme.warning));
        }
        if self.failed {
            return Some(("Tests failing".into(), theme.danger));
        }
        if self.working > 0 {
            return Some(("Working…".into(), theme.info));
        }
        if self.unseen > 0 {
            return Some(("Finished".into(), theme.success));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn questions_and_news_want_the_user() {
        assert!(!WorkspaceStatus::default().wants_you());
        let working = WorkspaceStatus {
            working: 2,
            ..Default::default()
        };
        assert!(!working.wants_you(), "working alone needs nothing");
        assert!(
            WorkspaceStatus {
                waiting: 1,
                ..Default::default()
            }
            .wants_you()
        );
        assert!(
            WorkspaceStatus {
                unseen: 1,
                ..Default::default()
            }
            .wants_you()
        );
    }
}

//! How a workspace's screen is divided: the chat on one side and, beside
//! it, one result pane for what the task produced, under a row of names
//! (Changes, Preview, Terminal, Files, and the workspace's other tools).
//!
//! The screen was a dock before: every tool a tab that could be dragged
//! anywhere. Few arranged it, an arrangement saved could come back wrong,
//! each tool was named twice (its icon in a column and its tab), and the
//! tools that need room (the browser, a phone, the board) sat in a narrow
//! column. Two panes with fixed places do less and hold up, and either can
//! be folded away to give the other the window.

use std::collections::HashSet;

use ph_ui::Icons;

use crate::context_panel::ContextTab;

/// What a part of the screen shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Pane {
    Chat,
    /// The open files.
    Editor,
    Tool(ContextTab),
}

/// A name over the result pane. Four are always the same; a workspace's
/// other tools (the board, run configurations, tests, pull requests, the
/// phone's log) each stand for themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResultTab {
    /// The review of what changed (the Git tool).
    Changes,
    /// What the work looks like when it runs: the browser, or a phone.
    Preview,
    Terminal,
    /// The folders' tree and the files open from it.
    Files,
    Other(ContextTab),
}

impl ResultTab {
    /// The name `tool` shows under.
    pub(crate) fn of(tool: ContextTab) -> Self {
        match tool {
            ContextTab::Changes => ResultTab::Changes,
            ContextTab::Browser | ContextTab::Devices => ResultTab::Preview,
            ContextTab::Terminal => ResultTab::Terminal,
            ContextTab::Files => ResultTab::Files,
            other => ResultTab::Other(other),
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            ResultTab::Changes => "Changes",
            ResultTab::Preview => "Preview",
            ResultTab::Terminal => "Terminal",
            ResultTab::Files => "Files",
            ResultTab::Other(tool) => tool.label(),
        }
    }

    pub(crate) fn icon(self) -> Icons {
        match self {
            ResultTab::Changes => ContextTab::Changes.icon(),
            ResultTab::Preview => Icons::Eye,
            ResultTab::Terminal => ContextTab::Terminal.icon(),
            ResultTab::Files => ContextTab::Files.icon(),
            ResultTab::Other(tool) => tool.icon(),
        }
    }

    /// Its element id's last part (`context-tab-<key>`), which tests and
    /// the automation socket click.
    pub(crate) fn key(self) -> &'static str {
        match self {
            ResultTab::Changes => ContextTab::Changes.key(),
            ResultTab::Preview => "preview",
            ResultTab::Terminal => ContextTab::Terminal.key(),
            ResultTab::Files => ContextTab::Files.key(),
            ResultTab::Other(tool) => tool.key(),
        }
    }
}

/// Which panes a workspace's screen shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Layout {
    /// The tool the result pane shows ([`ContextTab::Files`] stands for
    /// the tree and the editor together).
    pub tool: ContextTab,
    /// What Preview showed last: the browser, or the devices.
    pub preview: ContextTab,
    /// The result pane is in view (else the chat has the screen).
    pub result_open: bool,
    /// The chat is folded to a strip (the result pane has the screen).
    pub chat_folded: bool,
    /// The folders' tree shows beside the editor.
    pub tree_open: bool,
    /// What the result pane showed before the terminal, for ⌘J to go back.
    pub before_terminal: Option<ContextTab>,
    /// The chat's share of the window's width (0 to 1) once the line
    /// between the panes was dragged; before that it starts at two fifths.
    pub chat_share: Option<f32>,
    /// A file's changes, picked in the Changes list, show beside the list
    /// (the editor has them in front).
    pub diff_beside: bool,
}

impl Layout {
    /// The chat and `tool` side by side.
    pub fn new(tool: ContextTab, result_open: bool) -> Self {
        Self {
            tool,
            preview: match tool {
                ContextTab::Devices => ContextTab::Devices,
                _ => ContextTab::Browser,
            },
            result_open,
            chat_folded: false,
            tree_open: true,
            before_terminal: None,
            chat_share: None,
            diff_beside: false,
        }
    }

    /// The name in front over the result pane.
    pub fn tab(&self) -> ResultTab {
        ResultTab::of(self.tool)
    }

    /// What is on screen.
    pub fn panes(&self) -> HashSet<Pane> {
        let mut panes = HashSet::new();
        if !self.chat_folded {
            panes.insert(Pane::Chat);
        }
        if self.result_open {
            match self.tool {
                ContextTab::Files => {
                    panes.insert(Pane::Editor);
                    if self.tree_open {
                        panes.insert(Pane::Tool(ContextTab::Files));
                    }
                }
                tool => {
                    panes.insert(Pane::Tool(tool));
                    if tool == ContextTab::Changes && self.diff_beside {
                        panes.insert(Pane::Editor);
                    }
                }
            }
        }
        panes
    }

    /// Shows `tool` in the result pane, opening the pane.
    pub fn show_tool(&mut self, tool: ContextTab) {
        if tool == ContextTab::Terminal && self.tool != ContextTab::Terminal {
            self.before_terminal = Some(self.tool);
        }
        if matches!(tool, ContextTab::Browser | ContextTab::Devices) {
            self.preview = tool;
        }
        self.tool = tool;
        self.result_open = true;
    }

    /// Shows what name `tab` stands for.
    pub fn show_tab(&mut self, tab: ResultTab) {
        self.show_tool(match tab {
            ResultTab::Changes => ContextTab::Changes,
            ResultTab::Preview => self.preview,
            ResultTab::Terminal => ContextTab::Terminal,
            ResultTab::Files => ContextTab::Files,
            ResultTab::Other(tool) => tool,
        });
    }

    /// Brings the chat back into view.
    pub fn show_chat(&mut self) {
        self.chat_folded = false;
    }

    /// Folds the chat away or brings it back. With nothing beside it the
    /// chat stays: something is always on screen.
    pub fn toggle_chat(&mut self) {
        self.chat_folded = !self.chat_folded && self.result_open;
    }

    /// Hides the result pane or brings it back. Hiding it brings a folded
    /// chat back: something is always on screen.
    pub fn toggle_result(&mut self) {
        self.result_open = !self.result_open;
        if !self.result_open {
            self.chat_folded = false;
        }
    }

    /// ⌘J and ⌃`: the terminal; with it in front already, what was there
    /// before it (the chat alone, when the result pane was shut).
    pub fn toggle_terminal(&mut self) {
        if self.result_open && self.tool == ContextTab::Terminal {
            match self.before_terminal.take() {
                Some(tool) => self.show_tool(tool),
                None => self.toggle_result(),
            }
        } else {
            let was_open = self.result_open;
            self.show_tool(ContextTab::Terminal);
            if !was_open {
                self.before_terminal = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn something_is_always_on_screen() {
        let mut layout = Layout::new(ContextTab::Changes, true);
        layout.toggle_chat();
        assert!(layout.chat_folded && layout.result_open);
        // Hiding the result pane too would leave nothing: the chat returns.
        layout.toggle_result();
        assert!(!layout.chat_folded && !layout.result_open);
        // Alone, the chat cannot be folded.
        layout.toggle_chat();
        assert!(!layout.chat_folded);
        assert_eq!(layout.panes(), HashSet::from([Pane::Chat]));
    }

    #[test]
    fn files_are_the_tree_and_the_editor() {
        let mut layout = Layout::new(ContextTab::Files, true);
        assert_eq!(
            layout.panes(),
            HashSet::from([Pane::Chat, Pane::Editor, Pane::Tool(ContextTab::Files)])
        );
        layout.tree_open = false;
        assert_eq!(layout.panes(), HashSet::from([Pane::Chat, Pane::Editor]));
    }

    #[test]
    fn changes_show_beside_their_list() {
        let mut layout = Layout::new(ContextTab::Changes, true);
        layout.diff_beside = true;
        assert_eq!(
            layout.panes(),
            HashSet::from([Pane::Chat, Pane::Tool(ContextTab::Changes), Pane::Editor])
        );
        // Only there: another tool has the pane to itself.
        layout.show_tool(ContextTab::Board);
        assert_eq!(
            layout.panes(),
            HashSet::from([Pane::Chat, Pane::Tool(ContextTab::Board)])
        );
    }

    #[test]
    fn preview_remembers_browser_or_phone() {
        let mut layout = Layout::new(ContextTab::Changes, true);
        layout.show_tab(ResultTab::Preview);
        assert_eq!(layout.tool, ContextTab::Browser);
        layout.show_tool(ContextTab::Devices);
        layout.show_tab(ResultTab::Changes);
        layout.show_tab(ResultTab::Preview);
        assert_eq!(layout.tool, ContextTab::Devices, "the phone was shown last");
        assert_eq!(layout.tab(), ResultTab::Preview);
    }

    #[test]
    fn the_terminal_key_goes_there_and_back() {
        let mut layout = Layout::new(ContextTab::Changes, true);
        layout.toggle_terminal();
        assert_eq!(layout.tool, ContextTab::Terminal);
        layout.toggle_terminal();
        assert_eq!(layout.tool, ContextTab::Changes);
        // From the chat alone, the key goes back to the chat alone.
        let mut layout = Layout::new(ContextTab::Changes, false);
        layout.toggle_terminal();
        assert!(layout.result_open && layout.tool == ContextTab::Terminal);
        layout.toggle_terminal();
        assert!(!layout.result_open);
        // Opened on the terminal: there is nothing to go back to but the
        // chat alone.
        let mut layout = Layout::new(ContextTab::Terminal, true);
        layout.toggle_terminal();
        assert!(!layout.result_open);
        layout.toggle_terminal();
        assert!(layout.result_open && layout.tool == ContextTab::Terminal);
    }
}

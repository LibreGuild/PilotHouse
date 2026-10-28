//! One open file: loads it off the UI thread, shows it in the code editor,
//! tracks unsaved edits and saves on ⌘S. A toolbar shows where the file is
//! and toggles blame; a footer shows the cursor position and who last
//! changed the cursor's line.
//!
//! Files with more to show than their text get a viewer from `ph-preview`:
//! a text kind (Markdown, JSON, CSV…) switches between its source, its
//! preview and both side by side; media, documents and binaries are shown
//! by their viewer alone.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, LazyLock, Mutex};

use gpui_kit::component::input::{
    Editor, EditorState, InputEvent, Position, RopeExt as _, TabSize, TextDecoration,
    TextDecorationCollection,
};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, Window, div,
};
use ph_ui::MenuButton as _;
use ph_ui::{EmptyState, Icons};

use crate::blame_view::{self, Blamed};
use crate::code_intel::{self, OpenLocation};
use crate::large::LargeFileView;
use crate::{
    AskAboutSelection, CopyAsMarkdown, CopyFilePath, CopyFileRelativePath, CopyLinkToLines,
    CopyReference, ExplainSelection, FindBugsInSelection, FindReferences, RenameSymbol, RevealFile,
    RevealInFiles, ShowOutline, WriteTestsForSelection,
};

/// What the user wants to know about selected code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskIntent {
    Explain,
    FindBugs,
    WriteTests,
    /// Attach it and let the user type the question.
    Custom,
}

impl AskIntent {
    pub const ALL: [AskIntent; 4] = [
        AskIntent::Explain,
        AskIntent::FindBugs,
        AskIntent::WriteTests,
        AskIntent::Custom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            AskIntent::Explain => "Explain",
            AskIntent::FindBugs => "Find bugs",
            AskIntent::WriteTests => "Write tests",
            AskIntent::Custom => "Ask…",
        }
    }
}

/// Selected code to ask the agent about.
#[derive(Debug, Clone, PartialEq)]
pub struct Ask {
    pub relative: String,
    /// 1-based, inclusive.
    pub lines: (u32, u32),
    pub code: String,
    pub language: String,
    pub intent: AskIntent,
}

impl Ask {
    /// `src/app.py:10-14`.
    pub fn label(&self) -> String {
        if self.lines.0 == self.lines.1 {
            format!("{}:{}", self.relative, self.lines.0)
        } else {
            format!("{}:{}-{}", self.relative, self.lines.0, self.lines.1)
        }
    }

    /// The code as it is attached, fenced with its language.
    pub fn attachment_text(&self) -> String {
        format!(
            "`{}`:\n```{}\n{}\n```",
            self.label(),
            self.language,
            self.code.trim_end()
        )
    }

    /// The chat's title: `Explain src/app.py:10-14`.
    pub fn title(&self) -> String {
        format!(
            "{} {}",
            self.intent.label().trim_end_matches('…'),
            self.label()
        )
    }

    /// The prompt for the intent; `None` for a custom question. Each asks
    /// the agent to look at the code's context in the codebase first, and
    /// to keep the answer short.
    pub fn prompt(&self) -> Option<String> {
        let place = self.label();
        let context = "Before answering, look up what it calls and what calls it in this codebase, \
                       so the answer reflects how it is really used.";
        Some(match self.intent {
            AskIntent::Explain => format!(
                "Explain the attached code from `{place}`: what it does, why it is there, and how it fits \
                 into the codebase. {context} Be brief: a short summary, then the key points."
            ),
            AskIntent::FindBugs => format!(
                "Look for bugs and edge cases in the attached code from `{place}`. {context} List real \
                 problems with the line they are on and a fix for each; say so if it looks right. Do not edit files."
            ),
            AskIntent::WriteTests => format!(
                "Write tests for the attached code from `{place}`, following the project's existing test \
                 conventions and framework. {context} Cover normal cases, edges and failures."
            ),
            AskIntent::Custom => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub enum FileViewEvent {
    Ask(Ask),
    /// Show this HTML file in the workspace's browser.
    OpenInBrowser(PathBuf),
    /// Select this file in the Files tree.
    RevealInTree,
}

/// The view last picked for each kind of file, so the next one opens the
/// same way (for this run of the app).
static LAST_VIEWS: LazyLock<Mutex<HashMap<Kind, View>>> = LazyLock::new(Mutex::default);

/// How long the preview waits after typing stops before it follows.
const PREVIEW_DELAY: std::time::Duration = std::time::Duration::from_millis(150);

impl gpui_kit::EventEmitter<FileViewEvent> for FileView {}
use ph_lsp::{Binding, LspEvent, LspManager};
use ph_preview::rich::RichEditor;
use ph_preview::{FilePreview, Kind, PreviewEvent, TextPreview, View};

use crate::SaveFile;

/// Files bigger than this open in the read-only large-file viewer instead of
/// the editor, so a stray log or dump cannot stall the UI.
const MAX_EDITABLE_BYTES: u64 = 4 * 1024 * 1024;
/// How much of a file is sniffed for NUL bytes to decide it is binary.
const BINARY_SNIFF_BYTES: usize = 8 * 1024;

pub const FILE_VIEW_CONTEXT: &str = "FileView";

#[derive(Debug, Clone, PartialEq)]
enum Content {
    Loading,
    Text,
    Binary,
    /// Over the editor's limit: shown by the large-file viewer.
    TooLarge(u64),
    Large(Entity<LargeFileView>),
    /// Shown by its viewer: media, documents, binaries.
    Media(Entity<FilePreview>),
    Failed(SharedString),
}

pub struct FileView {
    relative: SharedString,
    root: PathBuf,
    absolute: PathBuf,
    /// Blame for the saved file, read when first asked for.
    blame: Option<Rc<Blamed>>,
    show_blame: bool,
    blame_error: Option<SharedString>,
    _blame: Option<Task<()>>,
    /// The project's language servers, and how to open another file.
    lsp: Option<Entity<LspManager>>,
    open_location: Option<OpenLocation>,
    /// Errors and warnings from the language server.
    problems: (usize, usize),
    /// The diagnostics' underlines.
    squiggles: Option<TextDecorationCollection>,
    _sync: Option<Task<()>>,
    _intel: Option<Task<()>>,
    editor: Entity<EditorState>,
    content: Content,
    dirty: bool,
    /// The text came back from changes kept after a crash or quit
    /// ([`crate::unsaved`]), not from the file.
    restored: bool,
    /// The file is not UTF-8: it is shown read-only, as saving the text
    /// would replace every byte that is not.
    not_utf8: bool,
    /// Why the last save failed; the text stays, to save again.
    save_error: Option<SharedString>,
    /// Keeps the unsaved text a second after the last change.
    _keep: Option<Task<()>>,
    /// Line to reveal once loading finishes (0-based).
    pending_line: Option<u32>,
    /// How Files names the file (its folder's name first in a multi-folder
    /// workspace); defaults to `relative`.
    label: SharedString,
    /// Shown in a project: there is a chat to ask about code, Git for blame,
    /// and a path worth a breadcrumb. A lone file's window has none of them.
    asks: bool,
    /// What the file is, and how a text kind is shown now.
    kind: Kind,
    view: View,
    /// The rendered side of a text kind, made when first shown.
    preview: Option<Entity<TextPreview>>,
    _preview_sync: Option<Task<()>>,
    _preview_events: Option<Subscription>,
    /// Markdown edited as formatted text (the Rich view).
    rich: Option<Entity<RichEditor>>,
    _rich_events: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl FileView {
    pub fn new(
        root: &std::path::Path,
        relative: &str,
        lsp: Option<Entity<LspManager>>,
        open_location: Option<OpenLocation>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let language = language_for(relative);
        let kind = Kind::of(std::path::Path::new(relative));
        let view = LAST_VIEWS
            .lock()
            .ok()
            .and_then(|views| views.get(&kind).copied())
            .or_else(|| kind.views().first().copied())
            .unwrap_or(View::Source);
        let options = ph_theme::preferences::preferences(cx).editor;
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                // ⌘F find and replace in the file.
                .searchable(true)
                .language(language)
                .line_number(options.line_numbers)
                .soft_wrap(options.soft_wrap)
                .folding(options.folding)
                .indent_guides(options.indent_guides)
                .tab_size(TabSize {
                    tab_size: options.tab_size,
                    hard_tabs: false,
                })
        });
        // Follow changes made in Settings while the file is open.
        let preferences =
            cx.observe_global_in::<ph_theme::Preferences>(window, |this, window, cx| {
                let options = ph_theme::preferences::preferences(cx).editor;
                this.editor.update(cx, |editor, cx| {
                    editor.set_line_number(options.line_numbers, window, cx);
                    editor.set_soft_wrap(options.soft_wrap, window, cx);
                    editor.set_folding(options.folding, window, cx);
                    editor.set_indent_guides(options.indent_guides, window, cx);
                    editor.set_tab_size(
                        TabSize {
                            tab_size: options.tab_size,
                            hard_tabs: false,
                        },
                        cx,
                    );
                });
            });
        let subscription = cx.subscribe(&editor, |this: &mut Self, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                if !this.dirty {
                    this.dirty = true;
                    cx.notify();
                }
                this.schedule_sync(cx);
                this.schedule_keep(cx);
                this.schedule_preview(cx);
            }
        });
        // The footer follows the cursor.
        let cursor = cx.observe(&editor, |_, _, cx| cx.notify());
        let mut view = Self {
            relative: relative.to_string().into(),
            root: root.to_path_buf(),
            absolute: root.join(relative),
            blame: None,
            show_blame: false,
            blame_error: None,
            _blame: None,
            editor,
            content: Content::Loading,
            dirty: false,
            restored: false,
            not_utf8: false,
            save_error: None,
            _keep: None,
            pending_line: None,
            asks: true,
            kind,
            view,
            preview: None,
            _preview_sync: None,
            _preview_events: None,
            rich: None,
            _rich_events: None,
            label: SharedString::default(),
            _subscriptions: vec![subscription, preferences, cursor],
            lsp: None,
            open_location,
            problems: (0, 0),
            squiggles: None,
            _sync: None,
            _intel: None,
        };
        // Closing the file or the window keeps unsaved text right away (the
        // timer may not have fired yet).
        cx.on_release(|this, cx| {
            if this.dirty && this.content == Content::Text {
                let text = this.editor.read(cx).value().to_string();
                if let Err(err) = crate::unsaved::keep(&this.absolute, &text) {
                    tracing::warn!("keeping unsaved changes failed: {err}");
                }
            }
        })
        .detach();
        if let Some(lsp) = lsp {
            let diagnostics =
                cx.subscribe(
                    &lsp,
                    |this: &mut Self, _, event: &LspEvent, cx| match event {
                        LspEvent::Diagnostics(path) if *path == this.absolute => {
                            this.show_diagnostics(cx)
                        }
                        LspEvent::Status => cx.notify(),
                        _ => {}
                    },
                );
            view._subscriptions.push(diagnostics);
            view.lsp = Some(lsp);
        }
        view.load(window, cx);
        view
    }

    /// Opens the file on its language server and hooks up hover, go to
    /// definition and completion.
    fn attach_lsp(&mut self, text: &str, cx: &mut Context<Self>) {
        let Some(lsp) = self.lsp.clone() else { return };
        lsp.update(cx, |lsp, cx| lsp.open(&self.absolute, text, cx));
        if !lsp.read(cx).is_open(&self.absolute) {
            return;
        }
        let binding = Rc::new(Binding {
            manager: lsp.downgrade(),
            path: self.absolute.clone(),
        });
        let (here, open) = (self.absolute.clone(), self.open_location.clone());
        self.editor.update(cx, |editor, cx| {
            let lsp = editor.lsp_mut();
            lsp.hover_provider = Some(binding.clone());
            lsp.definition_provider = Some(binding.clone());
            lsp.completion_provider = Some(binding);
            // A definition in another file opens it there.
            lsp.show_document = Some(Rc::new(move |params, window, cx| {
                let Some(path) = ph_lsp::uri::to_path(&params.uri) else {
                    return false;
                };
                if path == here {
                    return false;
                }
                let line = params.selection.map(|range| range.start.line).unwrap_or(0);
                match &open {
                    Some(open) => {
                        open(path, line, window, cx);
                        true
                    }
                    None => false,
                }
            }));
            editor.refresh(cx);
        });
    }

    /// Tells the language server about edits, once typing pauses.
    fn schedule_sync(&mut self, cx: &mut Context<Self>) {
        if self.lsp.is_none() {
            return;
        }
        self._sync = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(250))
                .await;
            _ = this.update(cx, |this, cx| {
                let text = this.editor.read(cx).value().to_string();
                if let Some(lsp) = this.lsp.clone() {
                    lsp.update(cx, |lsp, cx| lsp.change(&this.absolute, &text, cx));
                }
            });
        }));
    }

    fn show_diagnostics(&mut self, cx: &mut Context<Self>) {
        let Some(lsp) = &self.lsp else { return };
        let diagnostics: Vec<lsp_types::Diagnostic> =
            lsp.read(cx).diagnostics(&self.absolute).to_vec();
        self.problems = (
            diagnostics
                .iter()
                .filter(|d| d.severity == Some(lsp_types::DiagnosticSeverity::ERROR))
                .count(),
            diagnostics
                .iter()
                .filter(|d| d.severity == Some(lsp_types::DiagnosticSeverity::WARNING))
                .count(),
        );
        // Wavy underlines, as text decorations the editor keeps in place
        // while the user types (until the server sends fresh ones).
        let (error, warning, info) = (cx.theme().danger, cx.theme().warning, cx.theme().info);
        self.editor.update(cx, |editor, cx| {
            let text = gpui_kit::component::input::Rope::from(editor.value().as_ref());
            let decorations: Vec<TextDecoration> = diagnostics
                .iter()
                .filter_map(|diagnostic| {
                    let range = ph_lsp::position::range_to_editor(&text, diagnostic.range);
                    let start = text.position_to_offset(&range.start);
                    let end = text
                        .position_to_offset(&range.end)
                        .max(start + 1)
                        .min(text.len());
                    let color = match diagnostic.severity {
                        Some(lsp_types::DiagnosticSeverity::ERROR) => error,
                        Some(lsp_types::DiagnosticSeverity::WARNING) => warning,
                        _ => info,
                    };
                    (start < end).then(|| {
                        TextDecoration::new(
                            start..end,
                            gpui_kit::HighlightStyle {
                                underline: Some(gpui_kit::UnderlineStyle {
                                    color: Some(color),
                                    thickness: gpui_kit::px(1.),
                                    wavy: true,
                                }),
                                ..Default::default()
                            },
                        )
                    })
                })
                .collect();
            if let Some(set) = editor.diagnostics_mut() {
                set.reset(&text);
                set.extend(diagnostics.clone());
            }
            match &self.squiggles {
                Some(collection) => collection.set(decorations, cx),
                None => {
                    self.squiggles = Some(editor.create_decorations_collection(decorations, cx))
                }
            }
            editor.refresh(cx);
        });
        cx.notify();
    }

    fn cursor_position(&self, cx: &gpui_kit::App) -> lsp_types::TextDocumentPositionParams {
        lsp_types::TextDocumentPositionParams {
            text_document: lsp_types::TextDocumentIdentifier {
                uri: ph_lsp::uri::from_path(&self.absolute)
                    .unwrap_or_else(|| "file:///".parse().expect("valid uri")),
            },
            position: {
                let editor = self.editor.read(cx);
                let text = gpui_kit::component::input::Rope::from(editor.value().as_ref());
                ph_lsp::position::to_server(&text, editor.cursor_position())
            },
        }
    }

    /// For a file shown without a project: no asking, no blame, and no
    /// toolbar (its window's title says what it is).
    pub fn standalone(&mut self) {
        self.asks = false;
    }

    fn ask_about_selection(
        &mut self,
        _: &AskAboutSelection,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ask(AskIntent::Custom, cx);
    }

    fn explain_selection(&mut self, _: &ExplainSelection, _: &mut Window, cx: &mut Context<Self>) {
        self.ask(AskIntent::Explain, cx);
    }

    fn find_bugs_in_selection(
        &mut self,
        _: &FindBugsInSelection,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ask(AskIntent::FindBugs, cx);
    }

    fn write_tests_for_selection(
        &mut self,
        _: &WriteTestsForSelection,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ask(AskIntent::WriteTests, cx);
    }

    /// Sends the selection (or the cursor's line) to the chat.
    pub fn ask(&mut self, intent: AskIntent, cx: &mut Context<Self>) {
        if !self.asks {
            return;
        }
        let Some((code, lines)) = self.selection(cx) else {
            return;
        };
        cx.emit(FileViewEvent::Ask(Ask {
            relative: self.relative_path().to_string(),
            lines,
            code,
            language: language_for(&self.relative),
            intent,
        }));
    }

    /// The selected text and its lines (1-based), or the cursor's line
    /// without a selection; `None` when that is blank.
    fn selection(&self, cx: &gpui_kit::App) -> Option<(String, (u32, u32))> {
        let editor = self.editor.read(cx);
        let text = editor.value();
        let mut range = editor.selected_range();
        let rope = gpui_kit::component::input::Rope::from(text.as_ref());
        if range.is_empty() {
            let line = rope.offset_to_position(range.start).line as usize;
            range = rope.line_start_offset(line)..rope.line_end_offset(line);
        }
        let range = range.start.min(text.len())..range.end.min(text.len());
        let code = text[range.clone()].to_string();
        if code.trim().is_empty() {
            return None;
        }
        let start = rope.offset_to_position(range.start).line + 1;
        let end = rope
            .offset_to_position(range.end.saturating_sub(1).max(range.start))
            .line
            + 1;
        Some((code, (start, end)))
    }

    /// `src/app.rs:10-14` for the selected lines.
    fn reference(&self, cx: &gpui_kit::App) -> Option<String> {
        let (_, (start, end)) = self.selection(cx)?;
        let path = self.relative_path();
        Some(if start == end {
            format!("{path}:{start}")
        } else {
            format!("{path}:{start}-{end}")
        })
    }

    fn copy_as_markdown(&mut self, _: &CopyAsMarkdown, _: &mut Window, cx: &mut Context<Self>) {
        let (Some((code, _)), Some(reference)) = (self.selection(cx), self.reference(cx)) else {
            return;
        };
        let text = format!(
            "`{reference}`\n```{}\n{}\n```\n",
            language_for(&self.relative),
            code.trim_end_matches('\n')
        );
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
    }

    fn copy_reference(&mut self, _: &CopyReference, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(reference) = self.reference(cx) {
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(reference));
        }
    }

    /// A link to the selected lines on GitHub (or GitLab), at the commit
    /// checked out.
    fn copy_link_to_lines(
        &mut self,
        _: &CopyLinkToLines,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui_kit::component::WindowExt as _;
        use gpui_kit::component::notification::Notification;
        let Some((_, (start, end))) = self.selection(cx) else {
            return;
        };
        let (root, relative) = (self.root.clone(), self.relative.to_string());
        let find = cx.background_spawn(async move {
            let repo = ph_git::remote::hosted_repo(&root)?;
            // The folder may sit below the repository's top.
            let prefix = ph_git::git(&root, &["rev-parse", "--show-prefix"]).ok()?;
            let commit = ph_git::git(&root, &["rev-parse", "HEAD"]).ok()?;
            let path = format!("{}{relative}", prefix.trim());
            Some(line_link(&repo, commit.trim(), &path, start, end))
        });
        cx.spawn_in(window, async move |_, cx| {
            let link = find.await;
            _ = cx.update(|window, cx| match link {
                Some(link) => cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(link)),
                None => window.push_notification(
                    Notification::info("This file is not in a repository on GitHub or GitLab."),
                    cx,
                ),
            });
        })
        .detach();
    }

    fn find_references(&mut self, _: &FindReferences, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(lsp), Some(open)) = (self.lsp.clone(), self.open_location.clone()) else {
            return;
        };
        let params = lsp_types::ReferenceParams {
            text_document_position: self.cursor_position(cx),
            context: lsp_types::ReferenceContext {
                include_declaration: true,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        let request = lsp.update(cx, |lsp, cx| {
            lsp.request::<lsp_types::request::References>(&self.absolute, params, cx)
        });
        let root = self.root.clone();
        self._intel = Some(cx.spawn_in(window, async move |_, cx| {
            let locations = request.await.ok().flatten().unwrap_or_default();
            let places = cx
                .background_spawn(async move { code_intel::places(&locations) })
                .await;
            _ = cx.update(|window, cx| {
                code_intel::open_places(
                    format!("References ({})", places.len()),
                    &root,
                    places,
                    open,
                    window,
                    cx,
                )
            });
        }));
    }

    fn show_outline(&mut self, _: &ShowOutline, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(lsp), Some(open)) = (self.lsp.clone(), self.open_location.clone()) else {
            return;
        };
        let params = lsp_types::DocumentSymbolParams {
            text_document: self.cursor_position(cx).text_document,
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        let request = lsp.update(cx, |lsp, cx| {
            lsp.request::<lsp_types::request::DocumentSymbolRequest>(&self.absolute, params, cx)
        });
        let path = self.absolute.clone();
        self._intel = Some(cx.spawn_in(window, async move |_, cx| {
            let Some(response) = request.await.ok().flatten() else {
                return;
            };
            _ = cx.update(|window, cx| {
                code_intel::open_outline(path, code_intel::outline(response), open, window, cx)
            });
        }));
    }

    fn rename_symbol(&mut self, _: &RenameSymbol, window: &mut Window, cx: &mut Context<Self>) {
        let Some(lsp) = self.lsp.clone() else { return };
        let position = self.cursor_position(cx);
        let editor = self.editor.read(cx);
        let current = word_at(&editor.value(), editor.cursor());
        let view = cx.weak_entity();
        let rename = Rc::new(
            move |name: String, window: &mut Window, cx: &mut gpui_kit::App| {
                let params = lsp_types::RenameParams {
                    text_document_position: position.clone(),
                    new_name: name,
                    work_done_progress_params: Default::default(),
                };
                _ = view.update(cx, |this, cx| {
                    let request = lsp.update(cx, |lsp, cx| {
                        lsp.request::<lsp_types::request::Rename>(&this.absolute, params, cx)
                    });
                    this._intel = Some(cx.spawn_in(window, async move |this, cx| {
                        let result = request.await;
                        _ = this.update_in(cx, |this, window, cx| {
                            this.apply_rename(result, window, cx)
                        });
                    }));
                });
            },
        );
        code_intel::ask_new_name(current, rename, window, cx);
    }

    fn apply_rename(
        &mut self,
        result: anyhow::Result<Option<lsp_types::WorkspaceEdit>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui_kit::component::WindowExt as _;
        use gpui_kit::component::notification::Notification;
        let edit = match result {
            Ok(Some(edit)) => edit,
            Ok(None) => {
                return window
                    .push_notification(Notification::warning("Nothing to rename here."), cx);
            }
            Err(err) => {
                return window.push_notification(Notification::error(format!("{err:#}")), cx);
            }
        };
        let text = gpui_kit::component::input::Rope::from(self.editor.read(cx).value().as_ref());
        match code_intel::apply_edit(edit, &self.absolute, &text) {
            Ok((own, files)) => {
                if let Some(own) = own {
                    self.editor
                        .update(cx, |editor, cx| editor.set_value(own, window, cx));
                    self.dirty = true;
                }
                window.push_notification(
                    Notification::success(format!(
                        "Renamed in {files} file{}",
                        if files == 1 { "" } else { "s" }
                    )),
                    cx,
                );
            }
            Err(err) => window.push_notification(Notification::error(format!("{err:#}")), cx),
        }
        cx.notify();
    }

    /// Names the file as Files does in a multi-folder workspace.
    pub fn set_label(&mut self, label: String) {
        self.label = label.into();
    }

    /// The file's path as Files names it.
    pub fn relative_path(&self) -> &SharedString {
        if !self.label.is_empty() {
            return &self.label;
        }
        &self.relative
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Re-reads the file from disk unless the user has unsaved edits, which
    /// always win over a change made underneath (e.g. by an agent).
    pub fn reload_if_clean(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.dirty {
            self.load(window, cx);
        }
    }

    /// Moves the cursor to `line` (0-based), now or once the file has loaded.
    pub fn go_to_line(&mut self, line: u32, window: &mut Window, cx: &mut Context<Self>) {
        if self.content == Content::Loading {
            self.pending_line = Some(line);
            return;
        }
        self.editor.update(cx, |editor, cx| {
            editor.set_cursor_position(Position { line, character: 0 }, window, cx);
        });
    }

    /// Shows or hides blame, reading it the first time.
    pub fn toggle_blame(&mut self, cx: &mut Context<Self>) {
        self.show_blame = !self.show_blame;
        if self.show_blame && self.blame.is_none() {
            self.load_blame(cx);
        }
        cx.notify();
    }

    fn load_blame(&mut self, cx: &mut Context<Self>) {
        let (root, relative, absolute) = (
            self.root.clone(),
            self.relative.to_string(),
            self.absolute.clone(),
        );
        let read = cx.background_spawn(async move {
            let blame = ph_git::blame::blame(&root, &relative)?;
            let text = std::fs::read_to_string(&absolute)?;
            let lines = text
                .lines()
                .map(|line| SharedString::from(line.to_string()))
                .collect();
            anyhow::Ok(Blamed { lines, blame })
        });
        self._blame = Some(cx.spawn(async move |this, cx| {
            let result = read.await;
            _ = this.update(cx, |this, cx| {
                match result {
                    Ok(blamed) => {
                        this.blame = Some(Rc::new(blamed));
                        this.blame_error = None;
                    }
                    Err(err) => {
                        this.blame_error = Some(format!("No blame: {err:#}").into());
                        this.show_blame = false;
                    }
                }
                cx.notify();
            });
        }));
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A large file reads on from where it was (a growing log).
        if let Content::Large(view) = &self.content {
            view.update(cx, |view, cx| view.refresh(cx));
            return;
        }
        // A viewer reads the file itself.
        if let Content::Media(view) = &self.content {
            view.update(cx, |view, cx| view.reload(window, cx));
            return;
        }
        if !self.kind.is_text() {
            self.show_media(self.kind, window, cx);
            return;
        }
        // The file changed on disk: blame is out of date.
        self.blame = None;
        if self.show_blame {
            self.load_blame(cx);
        }
        let path = self.absolute.clone();
        let read = cx.background_spawn(async move {
            let (text, not_utf8) = read_text(&path)?;
            // Changes kept from a crash or quit come back, still unsaved.
            let kept = crate::unsaved::kept(&path).filter(|kept| *kept != text);
            if kept.is_none() {
                crate::unsaved::forget(&path);
            }
            Ok((text, kept, not_utf8))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = read.await;
            _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok((text, kept, not_utf8)) => {
                        this.not_utf8 = not_utf8;
                        this.attach_lsp(&text, cx);
                        let restored = kept.is_some();
                        let shown = kept.unwrap_or(text);
                        this.editor
                            .update(cx, |editor, cx| editor.set_value(shown, window, cx));
                        this.content = Content::Text;
                        this.dirty = restored;
                        this.restored = restored;
                        if let Some(line) = this.pending_line.take() {
                            this.go_to_line(line, window, cx);
                        }
                        this.update_preview(window, cx);
                    }
                    // A file named like text that is not: its bytes.
                    Err(Content::Binary) => this.show_media(Kind::Text, window, cx),
                    Err(Content::TooLarge(_)) => {
                        let path = this.absolute.clone();
                        let view = cx.new(|cx| LargeFileView::new(path, window, cx));
                        this.content = Content::Large(view);
                    }
                    Err(content) => this.content = content,
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Shows the file with its viewer (`Kind::Text`: as bytes).
    fn show_media(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.absolute.clone();
        let view = cx.new(|cx| FilePreview::new(kind, path, window, cx));
        self.content = Content::Media(view);
        cx.notify();
    }

    /// How the file is shown: one of its kind's views.
    pub fn set_view(&mut self, view: View, window: &mut Window, cx: &mut Context<Self>) {
        if !self.kind.views().contains(&view) {
            return;
        }
        // Leaving the rich editor, its last keystrokes go to the text first.
        if self.view == View::Rich
            && view != View::Rich
            && let Some(rich) = self.rich.clone()
        {
            let text = rich.update(cx, |rich, cx| rich.text(cx));
            self.replace_text(text, window, cx);
        }
        self.view = view;
        if let Ok(mut views) = LAST_VIEWS.lock() {
            views.insert(self.kind, view);
        }
        self.update_preview(window, cx);
        cx.notify();
    }

    /// Whether the file's text is on screen to type into.
    pub fn takes_keys(&self) -> bool {
        self.content == Content::Text && self.shows_editor()
    }

    /// Whether the code editor is on screen.
    fn shows_editor(&self) -> bool {
        !self.shows_preview() || self.view == View::Split
    }

    /// Whether the rendered side is on screen.
    fn shows_preview(&self) -> bool {
        self.content == Content::Text && self.view != View::Source && !self.kind.views().is_empty()
    }

    /// Hands the preview the editor's text now (made when first needed).
    fn update_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.shows_preview() {
            return;
        }
        if self.view == View::Rich {
            let rich = match &self.rich {
                Some(rich) => rich.clone(),
                None => {
                    let path = self.absolute.clone();
                    let rich = cx.new(|cx| RichEditor::new(path, cx));
                    self._rich_events = Some(cx.subscribe_in(
                        &rich,
                        window,
                        |this, _, event: &PreviewEvent, window, cx| {
                            this.on_preview(event, window, cx)
                        },
                    ));
                    self.rich = Some(rich.clone());
                    rich
                }
            };
            let text = self.editor.read(cx).value().to_string();
            rich.update(cx, |rich, cx| rich.set_text(&text, cx));
            return;
        }
        let preview = match &self.preview {
            Some(preview) => preview.clone(),
            None => {
                let (kind, path) = (self.kind, self.absolute.clone());
                let preview = cx.new(|cx| TextPreview::new(kind, path, window, cx));
                self._preview_events = Some(cx.subscribe_in(
                    &preview,
                    window,
                    |this, _, event: &PreviewEvent, window, cx| this.on_preview(event, window, cx),
                ));
                self.preview = Some(preview.clone());
                preview
            }
        };
        let text: Arc<str> = self.editor.read(cx).value().as_ref().into();
        preview.update(cx, |preview, cx| preview.set_text(text, cx));
    }

    /// Follows typing, once it pauses.
    fn schedule_preview(&mut self, cx: &mut Context<Self>) {
        if !self.shows_preview() {
            return;
        }
        self._preview_sync = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PREVIEW_DELAY).await;
            _ = this.update(cx, |this, cx| {
                let (Some(preview), true) = (
                    this.preview.clone(),
                    this.shows_preview() && this.view != View::Rich,
                ) else {
                    return;
                };
                let text: Arc<str> = this.editor.read(cx).value().as_ref().into();
                preview.update(cx, |preview, cx| preview.set_text(text, cx));
            });
        }));
    }

    fn on_preview(&mut self, event: &PreviewEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            PreviewEvent::OpenPath(path, line) => {
                // After this update: opening reads the open files, this one
                // among them.
                if let Some(open) = self.open_location.clone() {
                    let (path, line) = (path.clone(), line.unwrap_or(0));
                    window.defer(cx, move |window, cx| open(path, line, window, cx));
                }
            }
            PreviewEvent::ReplaceText(text) => self.replace_text(text.clone(), window, cx),
        }
    }

    /// Replaces the whole text as an edit (unsaved until ⌘S), keeping the
    /// cursor's line.
    pub fn replace_text(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.content != Content::Text || self.not_utf8 {
            return;
        }
        if self.editor.read(cx).value().as_ref() == text.as_str() {
            return;
        }
        let line = self.editor.read(cx).cursor_position().line;
        // Placing the cursor also focuses the editor: only when it is on
        // screen, or the hidden editor would take the keys from the view
        // that made the change.
        let shown = self.shows_editor();
        self.editor.update(cx, |editor, cx| {
            editor.set_value(text, window, cx);
            if shown {
                editor.set_cursor_position(Position { line, character: 0 }, window, cx);
            }
        });
        self.dirty = true;
        self.schedule_sync(cx);
        self.schedule_keep(cx);
        self.update_preview(window, cx);
        cx.notify();
    }

    /// Formats (`indent`) or minifies (`None`) a JSON file's text.
    fn reformat_json(
        &mut self,
        indent: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui_kit::component::WindowExt as _;
        use gpui_kit::component::notification::Notification;
        let text = self.editor.read(cx).value().to_string();
        let result = match indent {
            Some(indent) => ph_preview::json::format(&text, indent),
            None => ph_preview::json::minify(&text),
        };
        match result {
            Ok(text) => self.replace_text(text, window, cx),
            Err(err) => window
                .push_notification(Notification::warning(format!("Not valid JSON: {err}")), cx),
        }
    }

    /// Stops a playing video or sound (its tab went to the back).
    pub fn pause_media(&mut self, cx: &mut Context<Self>) {
        if let Content::Media(view) = &self.content {
            view.update(cx, |view, cx| view.pause(cx));
        }
    }

    /// Keeps the unsaved text on disk a second after the last change, so a
    /// crash or quit loses at most that second.
    fn schedule_keep(&mut self, cx: &mut Context<Self>) {
        self._keep = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(1))
                .await;
            let Ok((path, text)) = this.update(cx, |this, cx| {
                (
                    this.absolute.clone(),
                    this.editor.read(cx).value().to_string(),
                )
            }) else {
                return;
            };
            let keep = cx
                .background_spawn(async move { crate::unsaved::keep(&path, &text) })
                .await;
            if let Err(err) = keep {
                tracing::warn!("keeping unsaved changes failed: {err}");
            }
        }));
    }

    /// Throws away changes restored after a crash: the file as it is on disk.
    fn discard_restored(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        crate::unsaved::forget(&self.absolute);
        self.restored = false;
        self._keep = None;
        self.load(window, cx);
    }

    fn save(&mut self, _: &SaveFile, window: &mut Window, cx: &mut Context<Self>) {
        if self.content != Content::Text || self.not_utf8 {
            return;
        }
        // The rich editor's last keystrokes, before its timer.
        if let (Some(rich), View::Rich) = (self.rich.clone(), self.view) {
            let text = rich.update(cx, |rich, cx| rich.text(cx));
            self.replace_text(text, window, cx);
        }
        let text = self.editor.read(cx).value().to_string();
        let path = self.absolute.clone();
        let write = cx.background_spawn(async move { std::fs::write(&path, text) });
        cx.spawn(async move |this, cx| {
            let result = write.await;
            _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.dirty = false;
                        this.restored = false;
                        this.save_error = None;
                        this._keep = None;
                        crate::unsaved::forget(&this.absolute);
                        if let Some(lsp) = this.lsp.clone() {
                            lsp.update(cx, |lsp, _| lsp.saved(&this.absolute));
                        }
                        this.blame = None;
                        if this.show_blame {
                            this.load_blame(cx);
                        }
                    }
                    Err(err) => this.save_error = Some(format!("Could not save: {err}").into()),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// Reads a file as text (and whether it was not UTF-8, so shown with
/// replacement characters), or explains why it cannot be shown as text.
fn read_text(path: &std::path::Path) -> Result<(String, bool), Content> {
    let size = std::fs::metadata(path)
        .map_err(|err| Content::Failed(err.to_string().into()))?
        .len();
    if size > MAX_EDITABLE_BYTES {
        // Only text goes to the large-file viewer.
        let mut head = vec![0u8; BINARY_SNIFF_BYTES];
        let file =
            std::fs::File::open(path).map_err(|err| Content::Failed(err.to_string().into()))?;
        let read = crate::large::index::read_at(&file, &mut head, 0)
            .map_err(|err| Content::Failed(err.to_string().into()))?;
        if head[..read].contains(&0) {
            return Err(Content::Binary);
        }
        return Err(Content::TooLarge(size));
    }
    let bytes = std::fs::read(path).map_err(|err| Content::Failed(err.to_string().into()))?;
    if bytes[..bytes.len().min(BINARY_SNIFF_BYTES)].contains(&0) {
        return Err(Content::Binary);
    }
    match String::from_utf8(bytes) {
        Ok(text) => Ok((text, false)),
        Err(err) => Ok((String::from_utf8_lossy(err.as_bytes()).into_owned(), true)),
    }
}

/// The highlighter accepts a file extension or a well-known file name.
fn language_for(relative: &str) -> String {
    let name = relative.rsplit('/').next().unwrap_or(relative);
    match name.rsplit_once('.') {
        Some((_, extension)) if !extension.is_empty() => extension.to_lowercase(),
        _ => name.to_string(),
    }
}

impl Focusable for FileView {
    fn focus_handle(&self, cx: &gpui_kit::App) -> FocusHandle {
        self.editor.read(cx).focus_handle(cx)
    }
}

/// The editor's right-click menu: asking the agent first (when there is a
/// chat), then code navigation (with a language server), then the usual
/// editing items.
/// A link to lines `start`–`end` of `path` at `commit` on the host.
fn line_link(
    repo: &ph_git::remote::HostedRepo,
    commit: &str,
    path: &str,
    start: u32,
    end: u32,
) -> String {
    let lines = if start == end {
        format!("L{start}")
    } else if repo.host.contains("gitlab") {
        format!("L{start}-{end}")
    } else {
        format!("L{start}-L{end}")
    };
    let blob = if repo.host.contains("gitlab") {
        "-/blob"
    } else {
        "blob"
    };
    format!(
        "https://{}/{}/{}/{blob}/{commit}/{}#{lines}",
        repo.host,
        repo.owner,
        repo.name,
        ph_ui::platform::url_path(path)
    )
}

fn editor_menu(
    mut menu: gpui_kit::component::native_menu::NativeMenu,
    asks: bool,
    navigates: bool,
) -> gpui_kit::component::native_menu::NativeMenu {
    use gpui_kit::component::input as edit;
    if asks {
        menu = menu
            .menu("Add to Chat", Box::new(AskAboutSelection))
            .menu("Explain", Box::new(ExplainSelection))
            .menu("Find Bugs", Box::new(FindBugsInSelection))
            .menu("Write Tests", Box::new(WriteTestsForSelection))
            .separator();
    }
    menu = menu
        .menu_with_disabled(
            "Go to Definition",
            !navigates,
            Box::new(edit::GoToDefinition),
        )
        .menu_with_disabled("Find References", !navigates, Box::new(FindReferences))
        .menu_with_disabled("Rename Symbol", !navigates, Box::new(RenameSymbol))
        .menu_with_disabled(
            "Code Actions",
            !navigates,
            Box::new(edit::ToggleCodeActions),
        )
        .separator()
        .menu("Undo", Box::new(edit::Undo))
        .menu("Redo", Box::new(edit::Redo))
        .separator()
        .menu("Cut", Box::new(edit::Cut))
        .menu("Copy", Box::new(edit::Copy))
        .menu("Paste", Box::new(edit::Paste))
        .menu("Copy as Markdown", Box::new(CopyAsMarkdown))
        .menu("Copy Reference (path:lines)", Box::new(CopyReference));
    if asks {
        menu = menu.menu("Copy Link to Lines", Box::new(CopyLinkToLines));
    }
    menu = menu
        .separator()
        .menu("Find…", Box::new(edit::Search))
        .menu("Replace…", Box::new(edit::Replace))
        .menu("Select All", Box::new(edit::SelectAll));
    if asks {
        menu = menu
            .separator()
            .menu("Copy Path", Box::new(CopyFilePath))
            .menu("Copy Relative Path", Box::new(CopyFileRelativePath))
            .menu(ph_ui::platform::reveal_label(), Box::new(RevealFile))
            .menu("Reveal in Files", Box::new(RevealInFiles));
    }
    menu
}

impl Render for FileView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match &self.content {
            Content::Text
                if self.view == View::Rich && self.shows_preview() && self.rich.is_some() =>
            {
                self.rich
                    .clone()
                    .map(|rich| rich.into_any_element())
                    .unwrap_or_else(|| div().into_any_element())
            }
            Content::Text if self.shows_preview() && self.preview.is_some() => {
                let preview = self.preview.clone().map(|p| p.into_any_element());
                let editor = self.render_editor(cx);
                match self.view {
                    View::Split => h_flex()
                        .size_full()
                        .child(div().flex_1().min_w_0().h_full().child(editor))
                        .child(div().w_px().h_full().bg(cx.theme().border))
                        .child(div().flex_1().min_w_0().h_full().children(preview))
                        .into_any_element(),
                    _ => div().size_full().children(preview).into_any_element(),
                }
            }
            Content::Text => self.render_editor(cx),
            Content::Media(view) => view.clone().into_any_element(),
            Content::Binary => EmptyState::new("Binary file")
                .icon(Icons::File)
                .description("This file is not text, so it is not shown here.")
                .into_any_element(),
            Content::Loading => div().size_full().into_any_element(),
            Content::Large(view) => view.clone().into_any_element(),
            // Briefly, until the viewer is made.
            Content::TooLarge(_) => div().size_full().into_any_element(),
            Content::Failed(reason) => EmptyState::new("Could not open file")
                .icon(Icons::CircleAlert)
                .description(reason.clone())
                .into_any_element(),
        };
        let body = match (&self.blame, self.show_blame, &self.content) {
            (Some(blamed), true, Content::Text) if !self.shows_preview() => {
                blame_view::render(blamed.clone(), cx)
            }
            _ => body,
        };
        // The cursor's line and blame belong to the editor.
        let is_text = self.content == Content::Text && self.shows_editor();
        v_flex()
            .key_context(FILE_VIEW_CONTEXT)
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::find_references))
            .on_action(cx.listener(Self::rename_symbol))
            .on_action(cx.listener(Self::show_outline))
            .on_action(cx.listener(Self::ask_about_selection))
            .on_action(cx.listener(Self::explain_selection))
            .on_action(cx.listener(Self::find_bugs_in_selection))
            .on_action(cx.listener(Self::write_tests_for_selection))
            .on_action(cx.listener(Self::copy_as_markdown))
            .on_action(cx.listener(Self::copy_reference))
            .on_action(cx.listener(Self::copy_link_to_lines))
            .on_action(cx.listener(|this, _: &CopyFilePath, _, cx| {
                let path = this.absolute.to_string_lossy().into_owned();
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(path));
            }))
            .on_action(cx.listener(|this, _: &CopyFileRelativePath, _, cx| {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                    this.relative.to_string(),
                ));
            }))
            .on_action(cx.listener(|this, _: &RevealFile, _, cx| cx.reveal_path(&this.absolute)))
            .on_action(
                cx.listener(|_, _: &RevealInFiles, _, cx| cx.emit(FileViewEvent::RevealInTree)),
            )
            .size_full()
            .when(self.asks, |this| this.child(self.render_toolbar(cx)))
            .child(div().flex_1().min_h_0().child(body))
            .when(is_text, |this| this.child(self.render_footer(cx)))
    }
}

impl FileView {
    /// The code editor.
    fn render_editor(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let markdown = self.kind == Kind::Markdown;
        match &self.content {
            Content::Text => div()
                .size_full()
                .when(markdown, |this| {
                    this.capture_key_down(cx.listener(Self::continue_markdown_list))
                })
                .child(
                    Editor::new(&self.editor)
                        .readonly(self.not_utf8)
                        .bordered(false)
                        .size_full()
                        .context_menu({
                            let asks = self.asks;
                            let navigates = self
                                .lsp
                                .as_ref()
                                .is_some_and(|lsp| lsp.read(cx).is_open(&self.absolute));
                            move |menu, _, _| editor_menu(menu, asks, navigates)
                        }),
                )
                .into_any_element(),
            _ => div().into_any_element(),
        }
    }

    /// Enter in a Markdown list item starts the next item; in an empty one,
    /// ends the list.
    fn continue_markdown_list(
        &mut self,
        event: &gpui_kit::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keystroke = &event.keystroke;
        if keystroke.key != "enter" || keystroke.modifiers.modified() || self.not_utf8 {
            return;
        }
        // Not the find bar's Enter, only the text's.
        if !self.editor.read(cx).focus_handle(cx).is_focused(window) {
            return;
        }
        let editor = self.editor.read(cx);
        let range = editor.selected_range();
        if !range.is_empty() {
            return;
        }
        let text = editor.value();
        let cursor = range.start.min(text.len());
        let line_start = text[..cursor].rfind('\n').map_or(0, |at| at + 1);
        // Only at the end of the line.
        if !text[cursor..].starts_with('\n') && cursor != text.len() {
            return;
        }
        let Some(next) = ph_preview::rich::blocks::continue_list(&text[line_start..cursor]) else {
            return;
        };
        self.editor.update(cx, |editor, cx| match next {
            ph_preview::rich::blocks::Continue::Item(prefix) => {
                editor.insert(format!("\n{prefix}"), window, cx);
            }
            ph_preview::rich::blocks::Continue::End => {
                editor.set_selected_range(line_start..cursor, cx);
                editor.insert("", window, cx);
            }
        });
        cx.stop_propagation();
    }
}

impl FileView {
    /// The path as breadcrumbs, and the blame toggle.
    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let parts: Vec<String> = self.relative.split('/').map(str::to_string).collect();
        let last = parts.len().saturating_sub(1);
        let (icon, tint) = ph_ui::file_icon(&self.relative);
        h_flex()
            .flex_none()
            .h_8()
            .px_3()
            .gap_1()
            .overflow_hidden()
            .border_b_1()
            .border_color(cx.theme().border)
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            // The path gives way first when the pane is narrow, so the
            // buttons stay reachable.
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .overflow_hidden()
                    .children(parts.into_iter().enumerate().map(|(ix, part)| {
                        h_flex()
                            .flex_none()
                            .gap_1()
                            .when(ix == last, |this| {
                                this.child(Icon::new(icon).xsmall().text_color(tint))
                                    .text_color(cx.theme().foreground)
                            })
                            .child(part)
                            .when(ix < last, |this| this.child(div().opacity(0.5).child("›")))
                    })),
            )
            .when_some(self.blame_error.clone(), |this, error| {
                this.child(div().min_w_0().truncate().child(error))
            })
            .children(self.render_kind_actions(cx))
            .children(self.render_view_switch(cx))
            .when(
                self.lsp
                    .as_ref()
                    .is_some_and(|lsp| lsp.read(cx).is_open(&self.absolute)),
                |this| {
                    this.child(
                        Button::new("show-outline")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Menu)
                            .accessibility_label("Outline")
                            .tooltip("Symbols in this file (⌘⇧O)")
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(ShowOutline), cx)
                            }),
                    )
                    .child(
                        Button::new("find-references")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Search)
                            .accessibility_label("References")
                            .tooltip("Uses of the symbol at the cursor (⇧F12); F2 renames it")
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(FindReferences), cx)
                            }),
                    )
                },
            )
            // Asking and blame work on the editor's text.
            .when(
                self.content == Content::Text && self.shows_editor(),
                |this| {
                    this.when(self.asks, |this| {
                        this.child(
                            Button::new("ask-about-selection")
                                .ghost()
                                .xsmall()
                                .icon(ph_ui::Icons::Sparkles)
                                .label("Ask")
                                .tooltip("Ask the agent about the selected code (⌘L)")
                                .menu(gpui_kit::Anchor::TopRight, {
                                    let view = cx.weak_entity();
                                    move |menu, _, _| {
                                        AskIntent::ALL.iter().fold(menu, |menu, intent| {
                                            let (view, intent) = (view.clone(), *intent);
                                            menu.item(
                                                gpui_kit::component::menu::PopupMenuItem::new(
                                                    intent.label(),
                                                )
                                                .on_click(move |_, _, cx| {
                                                    _ = view.update(cx, |view, cx| {
                                                        view.ask(intent, cx)
                                                    });
                                                }),
                                            )
                                        })
                                    }
                                }),
                        )
                    })
                    .child(
                        Button::new("toggle-blame")
                            .ghost()
                            .xsmall()
                            .icon(Icons::GitBranch)
                            .label(if self.show_blame {
                                "Hide blame"
                            } else {
                                "Blame"
                            })
                            .selected(self.show_blame)
                            .tooltip(if self.show_blame {
                                "Hide blame"
                            } else {
                                "Blame: who last changed each line"
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_blame(cx))),
                    )
                },
            )
    }

    /// Source, preview, both (or rich, for Markdown): a segmented switch.
    fn render_view_switch(&self, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        let views = self.kind.views();
        if self.content != Content::Text || views.len() < 2 {
            return None;
        }
        let theme = cx.theme();
        Some(
            h_flex()
                .flex_none()
                .p_0p5()
                .gap_0p5()
                .rounded(theme.radius)
                .bg(theme.secondary)
                .children(views.iter().map(|view| {
                    let (view, current) = (*view, *view == self.view);
                    Button::new(SharedString::from(format!("view-{}", view.key())))
                        .ghost()
                        .xsmall()
                        .label(view.label())
                        .selected(current)
                        .tooltip(match view {
                            View::Source => "The file's text",
                            View::Preview => "The file rendered",
                            View::Split => "The text and the rendering side by side",
                            View::Rich => "Edit the document as formatted text",
                        })
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.set_view(view, window, cx)),
                        )
                }))
                .into_any_element(),
        )
    }

    /// What the kind of file offers: formatting JSON, opening HTML in the
    /// browser, a media file in its own app.
    fn render_kind_actions(&self, cx: &mut Context<Self>) -> Vec<gpui_kit::AnyElement> {
        let mut actions = Vec::new();
        let text = self.content == Content::Text;
        if text && self.kind == Kind::Json && self.view != View::Preview && !self.not_utf8 {
            actions.push(
                Button::new("json-format")
                    .ghost()
                    .xsmall()
                    .icon(Icons::Braces)
                    .label("Format")
                    .tooltip("Indent the JSON two spaces a level")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.reformat_json(Some(2), window, cx)),
                    )
                    .into_any_element(),
            );
            actions.push(
                Button::new("json-minify")
                    .ghost()
                    .xsmall()
                    .label("Minify")
                    .tooltip("Put the JSON on one line")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.reformat_json(None, window, cx)),
                    )
                    .into_any_element(),
            );
        }
        if text && self.kind == Kind::Html {
            let path = self.absolute.clone();
            actions.push(
                Button::new("html-browser")
                    .ghost()
                    .xsmall()
                    .icon(Icons::Globe)
                    .label("Open in Browser")
                    .tooltip("Show the page in the workspace's browser")
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(FileViewEvent::OpenInBrowser(path.clone()))
                    }))
                    .into_any_element(),
            );
        }
        if matches!(self.content, Content::Media(_))
            && ph_ui::platform::opens_safely(&self.absolute)
        {
            let path = self.absolute.clone();
            actions.push(
                Button::new("open-default-app")
                    .ghost()
                    .xsmall()
                    .icon(IconName::ExternalLink)
                    .label("Open in Default App")
                    .on_click(move |_, _, cx| cx.open_with_system(&path))
                    .into_any_element(),
            );
        }
        actions
    }

    /// Cursor position, and the cursor line's last change when known.
    fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let position = self.editor.read(cx).cursor_position();
        let line = position.line as usize;
        let blame = self
            .blame
            .as_ref()
            .and_then(|blamed| blamed.blame.get(line))
            .filter(|_| !self.dirty)
            .map(|blame| {
                if blame.uncommitted() {
                    "Not committed yet".to_string()
                } else {
                    format!(
                        "{}, {}: {}",
                        blame.author,
                        blame_view::date(blame.time),
                        blame.summary
                    )
                }
            });
        h_flex()
            .flex_none()
            .h_6()
            .px_3()
            .gap_3()
            .overflow_hidden()
            .whitespace_nowrap()
            .border_t_1()
            .border_color(cx.theme().border)
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(div().flex_none().child(format!(
                "Ln {}, Col {}",
                line + 1,
                position.character + 1
            )))
            // Blame gives way to the counts on the right.
            .child(div().flex_1().min_w_0().truncate().children(blame))
            .when(self.problems.0 > 0, |this| {
                this.child(div().text_color(cx.theme().danger).child(format!(
                    "{} error{}",
                    self.problems.0,
                    if self.problems.0 == 1 { "" } else { "s" }
                )))
            })
            .when(self.problems.1 > 0, |this| {
                this.child(div().text_color(cx.theme().warning).child(format!(
                    "{} warning{}",
                    self.problems.1,
                    if self.problems.1 == 1 { "" } else { "s" }
                )))
            })
            .children(self.server_label(cx))
            .when(self.not_utf8, |this| {
                this.child(
                    div()
                        .text_color(cx.theme().warning)
                        .child("Not UTF-8: read-only"),
                )
            })
            .children(
                self.save_error
                    .clone()
                    .map(|error| div().text_color(cx.theme().danger).child(error)),
            )
            .when(self.restored, |this| {
                this.child(
                    div()
                        .text_color(cx.theme().warning)
                        .child("Restored unsaved changes"),
                )
                .child(
                    Button::new("discard-restored")
                        .ghost()
                        .xsmall()
                        .label("Discard")
                        .tooltip("Back to the file as saved")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.discard_restored(window, cx)),
                        ),
                )
            })
            .when(self.dirty && !self.restored, |this| this.child("Unsaved"))
    }

    /// The language server for this file and how it is doing.
    fn server_label(&self, cx: &gpui_kit::App) -> Option<String> {
        let (spec, _) = ph_lsp::servers::find(&self.absolute)?;
        let status = self
            .lsp
            .as_ref()?
            .read(cx)
            .status()
            .into_iter()
            .find(|(name, _)| *name == spec.name)?
            .1;
        Some(match status {
            ph_lsp::ServerStatus::Starting => format!("{} starting…", spec.name),
            ph_lsp::ServerStatus::Ready => spec.name.to_string(),
            ph_lsp::ServerStatus::Failed(_) => format!("{} failed", spec.name),
        })
    }

    /// Closes the file on its language server (the tab is going away).
    pub fn detach_lsp(&mut self, cx: &mut Context<Self>) {
        if let Some(lsp) = self.lsp.take() {
            lsp.update(cx, |lsp, _| lsp.close(&self.absolute));
        }
    }
}

/// The identifier around `offset`, to start a rename with.
fn word_at(text: &str, offset: usize) -> String {
    let offset = offset.min(text.len());
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let start = text[..offset]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map(|(i, _)| i)
        .unwrap_or(offset);
    let end = text[offset..]
        .char_indices()
        .find(|(_, c)| !is_word(*c))
        .map(|(i, _)| offset + i)
        .unwrap_or(text.len());
    text[start..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_to_lines_on_the_host() {
        let repo = |host: &str| ph_git::remote::HostedRepo {
            remote: "origin".into(),
            host: host.into(),
            owner: "acme".into(),
            name: "app".into(),
        };
        assert_eq!(
            line_link(&repo("github.com"), "abc", "docs/My #1 Notes.md", 3, 5),
            "https://github.com/acme/app/blob/abc/docs/My%20%231%20Notes.md#L3-L5"
        );
        assert_eq!(
            line_link(&repo("gitlab.com"), "abc", "a.rs", 7, 7),
            "https://gitlab.com/acme/app/-/blob/abc/a.rs#L7"
        );
    }

    #[test]
    fn finds_the_word_at_the_cursor() {
        assert_eq!(word_at("let total_count = 1;", 6), "total_count");
        assert_eq!(word_at("a.b", 0), "a");
        assert_eq!(word_at("  ", 1), "");
    }

    #[test]
    fn asks_name_the_code_and_its_lines() {
        let mut ask = Ask {
            relative: "src/app.py".into(),
            lines: (10, 14),
            code: "def total(values):\n    return sum(values)\n".into(),
            language: "py".into(),
            intent: AskIntent::Explain,
        };
        assert_eq!(ask.label(), "src/app.py:10-14");
        assert_eq!(ask.title(), "Explain src/app.py:10-14");
        assert_eq!(
            ask.attachment_text(),
            "`src/app.py:10-14`:\n```py\ndef total(values):\n    return sum(values)\n```"
        );
        let prompt = ask.prompt().unwrap();
        assert!(prompt.contains("`src/app.py:10-14`") && prompt.contains("this codebase"));

        ask.lines = (3, 3);
        ask.intent = AskIntent::Custom;
        assert_eq!(ask.label(), "src/app.py:3");
        assert_eq!(ask.title(), "Ask src/app.py:3");
        assert_eq!(
            ask.prompt(),
            None,
            "a custom question is the user's to type"
        );
        ask.intent = AskIntent::FindBugs;
        assert!(ask.prompt().unwrap().contains("Do not edit files"));
    }

    #[test]
    fn language_comes_from_extension_or_name() {
        assert_eq!(language_for("src/main.rs"), "rs");
        assert_eq!(language_for("Makefile"), "Makefile");
        assert_eq!(language_for("web/App.TSX"), "tsx");
    }

    #[test]
    fn binary_and_large_files_are_refused() {
        let dir = std::env::temp_dir().join(format!("ph-fileview-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let binary = dir.join("blob.bin");
        std::fs::write(&binary, [1u8, 0, 2]).unwrap();
        assert_eq!(read_text(&binary), Err(Content::Binary));
        let text = dir.join("note.txt");
        std::fs::write(&text, "hello").unwrap();
        assert_eq!(read_text(&text).unwrap(), ("hello".to_string(), false));
        let latin1 = dir.join("latin1.txt");
        std::fs::write(&latin1, b"caf\xe9").unwrap();
        assert_eq!(
            read_text(&latin1).unwrap(),
            ("caf\u{fffd}".to_string(), true)
        );
    }
}

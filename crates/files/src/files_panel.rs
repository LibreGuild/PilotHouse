//! A workspace's files: the folder tree (or search results), drawn in the
//! sidebar by [`FileTree`], and the open files, drawn by the panel itself in
//! the editor column beside the chat. One model serves both, so picking a
//! file in the tree opens it in the editor.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    list::ListItem,
    menu::ContextMenuExt as _,
    tab::{Tab, TabBar},
    tree::{TreeEvent, TreeState, tree},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Task, WeakEntity, Window, div, px,
};
use ph_ui::{EmptyState, Icons};

use crate::dir_tree::DirTree;
use crate::file_index::FileList;
use crate::file_view::FileView;
use crate::roots::Roots;
use crate::search::SearchResults;
use crate::watcher::{FilesChanged, ProjectWatcher};
use crate::{CloseFile, quick_open};

pub const FILES_PANEL_CONTEXT: &str = "FilesPanel";

/// A tree row's height, and how far each level is indented.
const TREE_ROW_HEIGHT: gpui_kit::Pixels = px(24.);
const TREE_INDENT: gpui_kit::Pixels = px(14.);

/// Changed paths, and the folders holding them.
#[derive(Default)]
pub(crate) struct TreeChanges {
    pub(crate) files: HashMap<String, ph_git::Change>,
    dirs: HashSet<String>,
}

/// The colour a change is marked in.
fn change_color(change: ph_git::Change, cx: &gpui_kit::App) -> gpui_kit::Hsla {
    let theme = cx.theme();
    match change {
        ph_git::Change::Modified | ph_git::Change::Renamed => theme.warning,
        ph_git::Change::Added | ph_git::Change::Untracked => theme.success,
        ph_git::Change::Deleted | ph_git::Change::Conflicted => theme.danger,
    }
}

#[derive(Debug, Clone)]
pub enum FilesPanelEvent {
    /// The set of open files or the active one changed (for layout restore).
    OpenFilesChanged,
    /// A file was opened or its tab chosen: the editor should be in view.
    Opened,
    /// Ask the agent about selected code.
    Ask(crate::Ask),
    /// Make this folder the workspace's primary one.
    MakePrimary(PathBuf),
    /// Take this folder out of the workspace (it stays on disk).
    RemoveFolder(PathBuf),
    /// Open a terminal in this folder.
    OpenInTerminal(PathBuf),
    /// Open this file in a window of its own.
    OpenInWindow(PathBuf),
    /// Attach this file (a tree path) to the chat's next message.
    AddToChat(String),
    /// Attach this file and start a question about it in the chat.
    AskAboutFile(String),
    /// Start a chat whose agent works in this folder.
    NewChatIn(PathBuf),
    /// Show a file's changes since the last commit: the repository folder
    /// and the path within it.
    CompareWithHead(PathBuf, String),
    /// Show the differences between two files.
    CompareFiles(PathBuf, PathBuf),
    /// Show the commits that changed a file: the repository folder and the
    /// path within it.
    ShowHistory(PathBuf, String),
    /// Show this HTML file in the workspace's browser.
    OpenInBrowser(PathBuf),
    /// A file was selected in the tree from its tab: show the Files tool.
    RevealInTree,
    /// Add a folder to the workspace (the folder picker).
    AddFolder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Browse {
    Tree,
    Search,
}

#[derive(Default)]
struct Search {
    query: String,
    /// The folder searched, when not the whole workspace.
    scope: Option<String>,
    /// Files whose name matches, above the text matches.
    files: Vec<std::sync::Arc<str>>,
    results: SearchResults,
    running: bool,
    error: Option<SharedString>,
    /// Set to stop the search under way (a newer one replaces it).
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    _task: Option<Task<()>>,
    /// Searches a moment after typing (or a file change) stops.
    _debounce: Option<Task<()>>,
}

/// Places Back keeps.
const MAX_HISTORY: usize = 100;

/// How long typing pauses before the search runs.
const SEARCH_DELAY: std::time::Duration = std::time::Duration::from_millis(250);

pub struct FilesPanel {
    pub(crate) weak: WeakEntity<Self>,
    /// The workspace's folders (see [`Roots`]).
    pub(crate) roots: Roots,
    /// Per folder: whether it is in a Git repository.
    pub(crate) repos: Vec<bool>,
    /// Each folder's language servers (started when a file needs one).
    lsps: Vec<Entity<ph_lsp::LspManager>>,
    pub(crate) tree: DirTree,
    pub(crate) tree_state: Entity<TreeState>,
    /// Selected once the tree lists it (a file just made or renamed).
    pub(crate) pending_select: Option<String>,
    browse: Browse,
    search_input: Entity<InputState>,
    search: Search,
    /// Where Find in Folder searches next (`None`: the whole workspace).
    pub(crate) search_scope: Option<String>,
    pub(crate) open_files: Vec<Entity<FileView>>,
    pub(crate) active: Option<usize>,
    /// Open files, the one in front last: closing it brings back the one
    /// used before, as VS Code does.
    recent: Vec<gpui_kit::EntityId>,
    /// Where Back (and Forward) go: a file and a 0-based line, newest last.
    back: Vec<(String, u32)>,
    forward: Vec<(String, u32)>,
    /// Back or Forward is moving: what it does is not history itself.
    navigating: bool,
    /// Each open file's subscriptions, dropped with its tab.
    file_subscriptions: HashMap<gpui_kit::EntityId, [Subscription; 2]>,
    /// The Changes tab (after the files' tabs), and whether it is in front.
    diff: Option<Entity<crate::diff_tab::DiffTab>>,
    pub(crate) diff_active: bool,
    /// The tab strip's scroll, to bring the tab in front into view.
    tabs_scroll: gpui_kit::ScrollHandle,
    /// What Copy or Cut picked, for Paste.
    pub(crate) clipboard: Option<crate::tree_menu::Clipboard>,
    /// The file picked with Select for Compare.
    pub(crate) compare: Option<String>,
    /// File list for the finder; rebuilt lazily after file-system changes.
    index: Option<FileList>,
    index_stale: bool,
    /// Git's changes, marked in the tree.
    pub(crate) changes: Rc<TreeChanges>,
    /// Tree paths with staged changes.
    pub(crate) staged: Rc<HashSet<String>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<FilesPanelEvent> for FilesPanel {}

impl FilesPanel {
    /// Shows `roots`; `watchers` (one per folder) report file-system
    /// changes, and the panel refreshes what they touch.
    pub fn new(
        roots: Roots,
        watchers: &[Entity<ProjectWatcher>],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let tree_state = cx.new(|cx| TreeState::new(cx));
        let search_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search in workspace"));

        let mut subscriptions = vec![
            cx.subscribe(&tree_state, |this, _, event: &TreeEvent, cx| match event {
                TreeEvent::Expanded(id) => this.expand_dir(id.to_string(), cx),
                TreeEvent::Collapsed(id) => {
                    this.tree.set_expanded(id, false);
                    this.rebuild_tree(cx);
                }
            }),
            cx.subscribe_in(
                &search_input,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => this.run_search(window, cx),
                    // Results follow the query as it is typed.
                    InputEvent::Change => this.search_soon(window, cx),
                    _ => {}
                },
            ),
        ];

        for watcher in watchers {
            subscriptions.push(cx.subscribe_in(
                watcher,
                window,
                |this, _, event: &FilesChanged, window, cx| {
                    this.on_files_changed(event.paths.clone(), window, cx);
                },
            ));
        }
        let lsps = roots
            .list()
            .iter()
            .map(|root| {
                let path = root.path.clone();
                cx.new(|_| ph_lsp::LspManager::new(path))
            })
            .collect();
        // Which folders are repositories, for the menu's Git items.
        let folders: Vec<PathBuf> = roots.list().iter().map(|root| root.path.clone()).collect();
        let check = cx.background_spawn(async move {
            folders
                .iter()
                .map(|folder| ph_git::is_repository(folder))
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let repos = check.await;
            _ = this.update(cx, |this, _| this.repos = repos);
        })
        .detach();
        let mut panel = Self {
            weak: cx.weak_entity(),
            lsps,
            repos: Vec::new(),
            roots,
            tree: DirTree::default(),
            tree_state,
            pending_select: None,
            browse: Browse::Tree,
            search_input,
            search: Search::default(),
            search_scope: None,
            open_files: Vec::new(),
            diff: None,
            diff_active: false,
            tabs_scroll: gpui_kit::ScrollHandle::new(),
            active: None,
            recent: Vec::new(),
            back: Vec::new(),
            forward: Vec::new(),
            navigating: false,
            file_subscriptions: HashMap::new(),
            clipboard: None,
            compare: None,
            staged: Rc::default(),
            index: None,
            index_stale: true,
            changes: Rc::default(),
            _subscriptions: subscriptions,
        };
        panel.expand_dir(String::new(), cx);
        panel
    }

    /// Project-relative paths of the open files, in tab order.
    pub fn open_file_paths(&self, cx: &gpui_kit::App) -> Vec<String> {
        self.open_files
            .iter()
            .map(|file| file.read(cx).relative_path().to_string())
            .collect()
    }

    pub fn active_file_path(&self, cx: &gpui_kit::App) -> Option<String> {
        self.active
            .and_then(|ix| self.open_files.get(ix))
            .map(|file| file.read(cx).relative_path().to_string())
    }

    /// Reopens files from a saved layout. Missing files are skipped.
    pub fn restore(
        &mut self,
        files: &[String],
        active: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let existing: Vec<&String> = files
            .iter()
            .filter(|path| self.roots.absolute(path).is_some_and(|path| path.is_file()))
            .collect();
        for relative in existing {
            self.open_file(relative, None, window, cx);
        }
        if let Some(ix) = active.and_then(|active| self.index_of(active, cx)) {
            self.activate(ix, cx);
        }
        cx.notify();
    }

    /// Opens `relative` (or focuses its tab), optionally at a 0-based line.
    pub fn open_file(
        &mut self,
        relative: &str,
        line: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ix = match self.index_of(relative, cx) {
            Some(ix) => ix,
            None => {
                let Some((root, within)) = self.roots.split(relative) else {
                    return;
                };
                let root_ix = self
                    .roots
                    .list()
                    .iter()
                    .position(|candidate| candidate == root)
                    .unwrap_or_default();
                let (root, within) = (root.path.clone(), within.to_string());
                let label = relative.to_string();
                let lsp = self.lsps.get(root_ix).cloned();
                let panel = cx.weak_entity();
                let open: crate::code_intel::OpenLocation =
                    Rc::new(move |path, line, window, cx| {
                        _ = panel
                            .update(cx, |panel, cx| panel.open_location(&path, line, window, cx));
                    });
                let view = cx.new(|cx| {
                    let mut view = FileView::new(&root, &within, lsp, Some(open), window, cx);
                    view.set_label(label);
                    view
                });
                // A tab shows its file's state (unsaved, …) even while
                // another is in front.
                let observe = cx.observe(&view, |_, _, cx| cx.notify());
                let events = cx.subscribe(
                    &view,
                    |this, view, event: &crate::file_view::FileViewEvent, cx| match event {
                        crate::file_view::FileViewEvent::Ask(ask) => {
                            cx.emit(FilesPanelEvent::Ask(ask.clone()))
                        }
                        crate::file_view::FileViewEvent::OpenInBrowser(path) => {
                            cx.emit(FilesPanelEvent::OpenInBrowser(path.clone()))
                        }
                        crate::file_view::FileViewEvent::RevealInTree => {
                            let path = view.read(cx).relative_path().to_string();
                            this.reveal_in_tree(path, cx);
                        }
                        crate::file_view::FileViewEvent::Jumped(line) => {
                            let path = view.read(cx).relative_path().to_string();
                            this.remember(path, *line);
                        }
                    },
                );
                self.file_subscriptions
                    .insert(view.entity_id(), [observe, events]);
                self.open_files.push(view);
                self.open_files.len() - 1
            }
        };
        // Leaving the file in front: Back returns to where it was.
        if let Some(active) = self.active.filter(|active| *active != ix)
            && let Some(file) = self.open_files.get(active)
        {
            let (path, at) = {
                let file = file.read(cx);
                (file.relative_path().to_string(), file.cursor_line(cx))
            };
            self.remember(path, at);
        }
        if let Some(line) = line {
            self.open_files[ix].update(cx, |file, cx| file.go_to_line(line, window, cx));
        }
        self.activate(ix, cx);
        cx.emit(FilesPanelEvent::OpenFilesChanged);
        cx.emit(FilesPanelEvent::Opened);
        cx.notify();
    }

    /// Keeps a place for Back; a new place forgets what Forward had.
    fn remember(&mut self, path: String, line: u32) {
        if self.navigating {
            return;
        }
        if self.back.last() != Some(&(path.clone(), line)) {
            self.back.push((path, line));
            if self.back.len() > MAX_HISTORY {
                self.back.remove(0);
            }
        }
        self.forward.clear();
    }

    /// Back (`true`) or Forward: to the place before, skipping files that
    /// are gone.
    pub fn navigate(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        let here = self
            .active
            .and_then(|ix| self.open_files.get(ix))
            .map(|file| {
                let file = file.read(cx);
                (file.relative_path().to_string(), file.cursor_line(cx))
            });
        loop {
            let next = if back {
                self.back.pop()
            } else {
                self.forward.pop()
            };
            let Some((path, line)) = next else {
                return;
            };
            if self
                .roots
                .absolute(&path)
                .is_none_or(|path| !path.is_file())
            {
                continue;
            }
            if let Some(here) = here.clone() {
                if back {
                    self.forward.push(here);
                } else {
                    self.back.push(here);
                }
            }
            self.navigating = true;
            self.open_file(&path, Some(line), window, cx);
            self.navigating = false;
            return;
        }
    }

    /// Closes every tab but `keep`.
    fn close_others(&mut self, keep: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.close_where(|ix, _| ix != keep, window, cx);
    }

    /// Closes the tabs after `ix`.
    fn close_right(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.close_where(|other, _| other > ix, window, cx);
    }

    /// Closes the tabs without unsaved changes.
    fn close_saved(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_where(|_, file| !file.is_dirty(), window, cx);
    }

    pub fn close_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_where(|_, _| true, window, cx);
    }

    /// Closes the tabs `close` picks (by index and file).
    fn close_where(
        &mut self,
        close: impl Fn(usize, &FileView) -> bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let picked = self
            .open_files
            .iter()
            .enumerate()
            .filter(|(ix, file)| close(*ix, file.read(cx)))
            .map(|(_, file)| file.clone())
            .collect();
        self.request_close(picked, window, cx);
    }

    /// ⌘N in the editor: a new file in the open file's folder (or the
    /// workspace's first), opened once made.
    pub fn new_file_beside_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dir = self
            .active
            .and_then(|ix| self.open_files.get(ix))
            .and_then(|file| {
                file.read(cx)
                    .relative_path()
                    .rsplit_once('/')
                    .map(|(dir, _)| dir.to_string())
            })
            .unwrap_or_else(|| {
                if self.roots.is_multi() {
                    self.roots.list()[0].name.clone()
                } else {
                    String::new()
                }
            });
        self.new_entry(dir, false, window, cx);
    }

    /// Closes tab `ix`, asking first about unsaved changes.
    pub fn close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(file) = self.open_files.get(ix).cloned() {
            self.request_close(vec![file], window, cx);
        }
    }

    /// Closes `files`. With unsaved changes among them it asks first, as VS
    /// Code does: save them, drop them, or keep the tabs open.
    fn request_close(
        &mut self,
        files: Vec<Entity<FileView>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui_kit::component::WindowExt as _;
        let unsaved: Vec<Entity<FileView>> = files
            .iter()
            .filter(|file| file.read(cx).is_dirty())
            .cloned()
            .collect();
        if unsaved.is_empty() {
            self.close_views(&files, cx);
            return;
        }
        let question = match unsaved.as_slice() {
            [file] => format!(
                "Save the changes to {}?",
                file_name(file.read(cx).relative_path())
            ),
            _ => format!("Save the changes to {} files?", unsaved.len()),
        };
        let panel = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let save = {
                let (panel, files, unsaved) = (panel.clone(), files.clone(), unsaved.clone());
                move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut gpui_kit::App| {
                    window.close_dialog(cx);
                    let saves: Vec<Task<bool>> = unsaved
                        .iter()
                        .map(|file| file.update(cx, |file, cx| file.save_file(window, cx)))
                        .collect();
                    let (panel, files, unsaved) = (panel.clone(), files.clone(), unsaved.clone());
                    window
                        .spawn(cx, async move |cx| {
                            let mut saved = Vec::with_capacity(saves.len());
                            for save in saves {
                                saved.push(save.await);
                            }
                            // A file that could not be saved stays open, its
                            // footer saying why.
                            let close: Vec<_> = files
                                .iter()
                                .filter(|file| {
                                    unsaved
                                        .iter()
                                        .position(|other| other == *file)
                                        .is_none_or(|ix| saved[ix])
                                })
                                .cloned()
                                .collect();
                            _ = panel.update(cx, |panel, cx| panel.close_views(&close, cx));
                        })
                        .detach();
                }
            };
            let discard = {
                let (panel, files, unsaved) = (panel.clone(), files.clone(), unsaved.clone());
                move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut gpui_kit::App| {
                    window.close_dialog(cx);
                    for file in &unsaved {
                        file.update(cx, |file, cx| file.discard_changes(cx));
                    }
                    _ = panel.update(cx, |panel, cx| panel.close_views(&files, cx));
                }
            };
            dialog
                .title(question.clone())
                .w(px(440.))
                .child(
                    div()
                        .text_sm()
                        .child("Your changes are lost if you don't save them."),
                )
                .footer(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .child(
                            Button::new("close-discard")
                                .ghost()
                                .label("Don't Save")
                                .on_click(discard),
                        )
                        .child(div().flex_1())
                        .child(
                            Button::new("close-cancel")
                                .ghost()
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("close-save")
                                .primary()
                                .label("Save")
                                .on_click(save),
                        ),
                )
        });
    }

    /// Closes the tabs of `files` that are still open.
    fn close_views(&mut self, files: &[Entity<FileView>], cx: &mut Context<Self>) {
        for file in files {
            if let Some(ix) = self.open_files.iter().position(|open| open == file) {
                self.close_file(ix, cx);
            }
        }
    }

    /// The tree path of tab `ix`.
    fn tab_path(&self, ix: usize, cx: &gpui_kit::App) -> Option<String> {
        self.open_files
            .get(ix)
            .map(|file| file.read(cx).relative_path().to_string())
    }

    /// Selects `path` in the tree (opening the folders above it).
    fn reveal_in_tree(&mut self, path: String, cx: &mut Context<Self>) {
        self.browse = Browse::Tree;
        self.show_new(&path, cx);
        cx.emit(FilesPanelEvent::RevealInTree);
    }

    /// Gives the keyboard to the open file in front when its text is on
    /// screen (not a preview or a viewer, whose hidden text must not take
    /// keys).
    pub fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(file) = self.active.and_then(|ix| self.open_files.get(ix)) else {
            return;
        };
        if file.read(cx).takes_keys() {
            let focus = gpui_kit::Focusable::focus_handle(file.read(cx), cx);
            focus.focus(window, cx);
        }
    }

    /// Brings tab `ix` forward; a video or sound in the tab going back stops.
    pub(crate) fn activate(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(file) = self.open_files.get(ix) else {
            return;
        };
        let id = file.entity_id();
        self.recent.retain(|recent| *recent != id);
        self.recent.push(id);
        if let Some(previous) = self.active.filter(|previous| *previous != ix)
            && let Some(file) = self.open_files.get(previous)
        {
            file.update(cx, |file, cx| file.pause_media(cx));
        }
        self.active = Some(ix);
        self.diff_active = false;
        // The Changes tab, when open, is the strip's first.
        self.tabs_scroll
            .scroll_to_item(ix + usize::from(self.diff.is_some()));
    }

    /// Opens an absolute path at a 0-based line, if it is in the project.
    pub fn open_location(
        &mut self,
        path: &std::path::Path,
        line: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(relative) = self.roots.path_of(path) else {
            return;
        };
        self.open_file(&relative, Some(line), window, cx);
    }

    /// Closes tab `ix` as it is (unsaved text is kept for next time; see
    /// [`Self::close_tab`] to ask first).
    pub fn close_file(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.open_files.len() {
            return;
        }
        let closed = self.open_files.remove(ix);
        let id = closed.entity_id();
        self.file_subscriptions.remove(&id);
        self.recent.retain(|recent| *recent != id);
        closed.update(cx, |file, cx| file.detach_lsp(cx));
        self.active = match self.active {
            _ if self.open_files.is_empty() => None,
            // The tab in front closed: the one used before it comes back.
            Some(active) if active == ix => self
                .recent
                .last()
                .and_then(|id| {
                    self.open_files
                        .iter()
                        .position(|file| file.entity_id() == *id)
                })
                .or(Some(ix.min(self.open_files.len() - 1))),
            Some(active) if active > ix => Some(active - 1),
            Some(active) => Some(active.min(self.open_files.len() - 1)),
            None => None,
        };
        if let Some(active) = self.active {
            self.activate(active, cx);
        }
        cx.emit(FilesPanelEvent::OpenFilesChanged);
        cx.notify();
    }

    /// Opens the ⌘P finder. Uses the cached file list right away and
    /// rebuilds it in the background when files changed since.
    pub fn quick_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let panel = cx.weak_entity();
        let on_open = move |path: &str, window: &mut Window, cx: &mut gpui_kit::App| {
            let path = path.to_string();
            _ = panel.update(cx, |panel, cx| panel.open_file(&path, None, window, cx));
        };
        match (&self.index, self.index_stale) {
            (Some(index), false) => quick_open::open(index.clone(), on_open, window, cx),
            _ => {
                let roots = self.roots.clone();
                let build = cx.background_spawn(async move { roots.index() });
                cx.spawn_in(window, async move |this, cx| {
                    let index = build.await;
                    _ = this.update_in(cx, |this, window, cx| {
                        this.index = Some(index.clone());
                        this.index_stale = false;
                        quick_open::open(index, on_open, window, cx);
                    });
                })
                .detach();
            }
        }
    }

    /// ⌘T: a function, class or type by name, in every folder.
    pub fn go_to_symbol(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let multi = self.roots.is_multi();
        let folders: Vec<(String, PathBuf)> = self
            .roots
            .list()
            .iter()
            .map(|root| (root.name.clone(), root.path.clone()))
            .collect();
        let indexes = folders
            .iter()
            .map(|(name, path)| (name.clone(), crate::code_index::CodeIndex::for_root(path)))
            .collect();
        let panel = cx.weak_entity();
        crate::symbol_search::open(
            indexes,
            move |folder, symbol, window, cx| {
                let Some((name, _)) = folders.get(folder) else {
                    return;
                };
                let path = if multi {
                    format!("{name}/{}", symbol.path)
                } else {
                    symbol.path.to_string()
                };
                let line = symbol.line.saturating_sub(1) as u32;
                _ = panel.update(cx, |panel, cx| {
                    panel.open_file(&path, Some(line), window, cx)
                });
            },
            window,
            cx,
        );
    }

    /// Switches the tree to workspace search and focuses the query.
    pub fn find_in_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.browse = Browse::Search;
        self.search_input
            .update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    /// Back from search to the folder tree.
    pub fn show_tree(&mut self, cx: &mut Context<Self>) {
        self.browse = Browse::Tree;
        self.search_scope = None;
        cx.notify();
    }

    /// The tree shows search results rather than folders.
    pub fn is_searching(&self) -> bool {
        self.browse == Browse::Search
    }

    /// How many open files have unsaved changes.
    pub fn unsaved_count(&self, cx: &gpui_kit::App) -> usize {
        self.open_files
            .iter()
            .filter(|file| file.read(cx).is_dirty())
            .count()
    }

    /// Some file, or the Changes tab, is open (the editor column shows
    /// only then).
    pub fn has_open_files(&self) -> bool {
        !self.open_files.is_empty() || self.diff.is_some()
    }

    /// Opens the file selected in the tree (folders fold with the arrows).
    pub fn open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self
            .tree_state
            .read(cx)
            .selected_entry()
            .filter(|entry| !entry.is_folder())
            .map(|entry| entry.item().id.to_string());
        if let Some(path) = selected {
            self.open_file(&path, None, window, cx);
        }
    }

    /// The tree or the search results, for [`FileTree`].
    fn render_browser(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.browse {
            Browse::Tree => v_flex()
                .size_full()
                .child(self.render_tree_toolbar(cx))
                .child(div().flex_1().min_h_0().child(self.render_tree(cx)))
                .into_any_element(),
            Browse::Search => self.render_search(cx),
        }
    }

    /// The tree's first row: a search field that opens workspace search,
    /// and buttons to make files and folders.
    fn render_tree_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let button = |id: &'static str, icon: Icons, tip: &'static str| {
            Button::new(id)
                .ghost()
                .xsmall()
                .icon(icon)
                .tooltip(tip)
                .accessibility_label(tip)
        };
        h_flex()
            .flex_none()
            .px_2()
            .pt_2()
            .pb_1()
            .gap_0p5()
            .child(
                h_flex()
                    .id("files-search")
                    .role(gpui_kit::accesskit::Role::Button)
                    .aria_label("Search in workspace")
                    .flex_1()
                    .min_w_0()
                    .h_6()
                    .px_2()
                    .gap_1p5()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(cx.theme().input)
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .cursor_text()
                    .hover(|style| style.border_color(cx.theme().ring))
                    .on_click(cx.listener(|this, _, window, cx| this.find_in_project(window, cx)))
                    .child(Icon::new(IconName::Search).xsmall())
                    .child(div().min_w_0().truncate().child("Search files")),
            )
            .child(
                button("files-new-file", Icons::FilePlus, "New file").on_click(
                    cx.listener(|this, _, window, cx| this.new_in_selection(false, window, cx)),
                ),
            )
            .child(
                button("files-new-folder", Icons::FolderPlus, "New folder").on_click(
                    cx.listener(|this, _, window, cx| this.new_in_selection(true, window, cx)),
                ),
            )
            .child(
                button(
                    "files-collapse",
                    Icons::ChevronsDownUp,
                    "Collapse all folders",
                )
                .on_click(cx.listener(|this, _, _, cx| this.collapse_all(cx))),
            )
            .child(
                button(
                    "files-add-folder",
                    Icons::FolderInput,
                    "Add a folder to this workspace",
                )
                .on_click(cx.listener(|_, _, _, cx| cx.emit(FilesPanelEvent::AddFolder))),
            )
            .into_any_element()
    }

    pub(crate) fn index_of(&self, relative: &str, cx: &gpui_kit::App) -> Option<usize> {
        self.open_files
            .iter()
            .position(|file| file.read(cx).relative_path().as_ref() == relative)
    }

    // ---- Tree --------------------------------------------------------------

    fn expand_dir(&mut self, dir: String, cx: &mut Context<Self>) {
        if DirTree::is_placeholder(&dir) {
            return;
        }
        self.tree.set_expanded(&dir, true);
        if self.tree.is_listed(&dir) {
            self.rebuild_tree(cx);
            return;
        }
        self.load_dirs(vec![dir], cx);
    }

    /// Lists `dirs` in the background, then rebuilds the tree once.
    pub(crate) fn load_dirs(&mut self, dirs: Vec<String>, cx: &mut Context<Self>) {
        let roots = self.roots.clone();
        let listings = cx.background_spawn(async move {
            dirs.into_iter()
                .map(|dir| {
                    let entries = roots.list_dir(&dir);
                    (dir, entries)
                })
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let listings = listings.await;
            _ = this.update(cx, |this, cx| {
                for (dir, entries) in listings {
                    this.tree.set_listing(&dir, entries);
                }
                this.rebuild_tree(cx);
            });
        })
        .detach();
    }

    pub(crate) fn rebuild_tree(&mut self, cx: &mut Context<Self>) {
        let items = self.tree.items();
        let pending = self.pending_select.clone();
        let found = self.tree_state.update(cx, |state, cx| {
            let selected = state.selected_item().map(|item| item.id.clone());
            state.set_items(items, cx);
            let pending = pending.and_then(|id| state.index_of(&id.into()));
            if let Some(ix) = pending.or_else(|| selected.and_then(|id| state.index_of(&id))) {
                state.set_selected_index(Some(ix), cx);
                state.scroll_to_item(ix, gpui_kit::ScrollStrategy::Center);
            }
            pending.is_some()
        });
        if found {
            self.pending_select = None;
        }
        cx.notify();
    }

    /// Refreshes whatever a batch of file-system changes touched.
    fn on_files_changed(
        &mut self,
        changed: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.index_stale = true;
        // Something was staged or committed: the changed-line marks
        // compare against Git's copy again.
        if changed.iter().any(|path| {
            path.file_name().is_some_and(|name| name == "index")
                && path
                    .parent()
                    .and_then(|dir| dir.file_name())
                    .is_some_and(|dir| dir == ".git")
        }) {
            for file in &self.open_files {
                file.update(cx, |file, cx| file.refresh_git_base(cx));
            }
        }
        // Declarations are read again where files changed.
        for root in self.roots.list() {
            if let Some(index) = crate::code_index::CodeIndex::existing(&root.path) {
                index.invalidate(&changed);
            }
        }
        let relative: Vec<String> = changed
            .iter()
            .filter_map(|path| self.roots.path_of(path))
            .filter(|path| !path.split('/').any(|part| part == ".git"))
            .collect();
        // Results follow the files (a line changed, a file went away).
        if !relative.is_empty()
            && self.browse == Browse::Search
            && !self.search.query.trim().is_empty()
        {
            self.search_soon(window, cx);
        }

        let mut dirs: Vec<String> = relative
            .iter()
            .map(|path| {
                path.rsplit_once('/')
                    .map(|(dir, _)| dir.to_string())
                    .unwrap_or_default()
            })
            .filter(|dir| self.tree.is_listed(dir))
            .collect();
        dirs.sort();
        dirs.dedup();
        if !dirs.is_empty() {
            self.load_dirs(dirs, cx);
        }

        for file in &self.open_files {
            let path = file.read(cx).relative_path().to_string();
            if relative.contains(&path) {
                file.update(cx, |file, cx| file.reload_if_clean(window, cx));
            }
        }
    }

    // ---- Search ------------------------------------------------------------

    /// Runs the typed query again (the folder searched changed).
    pub(crate) fn search_again(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.search_input.read(cx).value().trim().is_empty() {
            self.run_search(window, cx);
        }
    }

    /// Runs the search a moment from now, unless asked again before.
    fn search_soon(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search._debounce = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(SEARCH_DELAY).await;
            _ = this.update_in(cx, |this, window, cx| this.run_search(window, cx));
        }));
    }

    /// Searches file names, then file contents, for the typed query. A
    /// search still running is stopped first.
    fn run_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use std::sync::atomic::{AtomicBool, Ordering};
        self.search._debounce = None;
        if let Some(cancel) = self.search.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        let query = self.search_input.read(cx).value().to_string();
        let scope = self.search_scope.clone();
        self.search.query = query.clone();
        self.search.scope = scope.clone();
        self.search.error = None;
        if query.trim().is_empty() {
            self.search.files.clear();
            self.search.results = SearchResults::default();
            self.search.running = false;
            self.search._task = None;
            cx.notify();
            return;
        }
        self.search.running = true;
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        self.search.cancel = Some(cancel.clone());
        let roots = self.roots.clone();
        // The file list for names, rebuilt when files changed since.
        let index = self.index.clone().filter(|_| !self.index_stale);
        let names = {
            let (roots, query, scope) = (roots.clone(), query.clone(), scope.clone());
            cx.background_spawn(async move {
                let rebuilt = index.is_none();
                let index = index.unwrap_or_else(|| roots.index());
                let prefix = scope.map(|dir| format!("{}/", dir.trim_end_matches('/')));
                let found = crate::search::match_file_names(&index, &query, |path| {
                    prefix
                        .as_deref()
                        .is_none_or(|prefix| path.starts_with(prefix))
                });
                (found, rebuilt.then_some(index))
            })
        };
        self.search._task = Some(cx.spawn_in(window, async move |this, cx| {
            let (files, rebuilt) = names.await;
            let stop = cancel.clone();
            let Ok(text) = this.update(cx, |this, cx| {
                if let Some(index) = rebuilt {
                    this.index = Some(index);
                    this.index_stale = false;
                }
                this.search.files = files;
                cx.notify();
                cx.background_spawn(async move {
                    match &scope {
                        Some(dir) => roots.search_in(dir, &query, &stop),
                        None => roots.search(&query, &stop),
                    }
                })
            }) else {
                return;
            };
            let result = text.await;
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            _ = this.update(cx, |this, cx| {
                this.search.running = false;
                match result {
                    Ok(results) => this.search.results = results,
                    Err(err) => this.search.error = Some(format!("{err:#}").into()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    // ---- Rendering -----------------------------------------------------------

    /// The folder tree: chevrons and indent guides show the nesting, each
    /// file and folder its kind's icon, and changed files their Git mark.
    /// With several folders, each is a root with a menu.
    fn render_tree(&self, cx: &mut Context<Self>) -> AnyElement {
        let panel = cx.weak_entity();
        let menu_panel = cx.weak_entity();
        let changes = self.changes.clone();
        let ignored = self.tree.ignored();
        let multi = self.roots.list().len() > 1;
        let primary = self.roots.list().first().map(|root| root.name.clone());
        let compare = self.compare.clone();
        let cut: Rc<HashSet<String>> = Rc::new(
            self.clipboard
                .as_ref()
                .filter(|clip| clip.cut)
                .map(|clip| {
                    clip.paths
                        .iter()
                        .filter_map(|path| self.roots.path_of(path))
                        .collect()
                })
                .unwrap_or_default(),
        );
        tree(&self.tree_state, move |ix, entry, selected, _, cx| {
            let item = entry.item();
            let path = item.id.to_string();
            let is_file = !entry.is_folder();
            let depth = entry.depth();
            let theme = cx.theme();
            let guide = theme.border.opacity(0.7);
            let change = changes.files.get(&path).copied();
            let holds_changes = !is_file && changes.dirs.contains(&path);
            let tint = change.map(|change| change_color(change, cx));
            // Ignored files, and what Cut picked, are dimmed.
            let is_ignored = ignored.contains(&path) || cut.contains(&path);
            let is_compared = compare.as_deref() == Some(path.as_str());
            let icon = if is_file {
                ph_ui::file_icon_element(&item.label, px(16.))
            } else {
                ph_ui::folder_icon_element(&item.label, px(16.))
            };
            let chevron = if is_file {
                div().flex_none().w(px(12.)).into_any_element()
            } else {
                Icon::new(if entry.is_expanded() {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .with_size(px(12.))
                .text_color(theme.muted_foreground)
                .into_any_element()
            };
            let is_primary_root = multi && depth == 0 && primary.as_deref() == Some(&path);
            let panel = panel.clone();
            ListItem::new(ix)
                .selected(selected)
                .py_0()
                .pl(px(6.))
                .pr_2()
                .child(
                    h_flex()
                        .relative()
                        .h(TREE_ROW_HEIGHT)
                        .min_w_0()
                        .gap_1()
                        .pl(TREE_INDENT * depth)
                        .text_sm()
                        .when(is_ignored, |this| this.opacity(0.5))
                        // A line per level, through the rows, under each
                        // folder's chevron.
                        .children((0..depth).map(|level| {
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left(TREE_INDENT * level + px(5.5))
                                .w_px()
                                .bg(guide)
                        }))
                        .child(chevron)
                        .child(icon)
                        .child(
                            div()
                                .truncate()
                                .when_some(tint, |this, tint| this.text_color(tint))
                                .when(holds_changes, |this| {
                                    this.text_color(theme.warning.opacity(0.9))
                                })
                                .child(item.label.clone()),
                        )
                        .when(is_primary_root, |this| {
                            this.child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child("primary"),
                            )
                        })
                        .when(is_compared, |this| {
                            this.child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child("compare"),
                            )
                        }),
                )
                .when_some(change, |this, change| {
                    this.suffix(move |_, cx| {
                        div()
                            .text_xs()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .text_color(change_color(change, cx))
                            .child(change.letter().to_string())
                    })
                })
                .when(holds_changes, |this| {
                    this.suffix(|_, cx| {
                        div()
                            .size(px(6.))
                            .rounded_full()
                            .bg(cx.theme().warning.opacity(0.8))
                    })
                })
                .when(is_file, |this| {
                    this.on_click(move |_, window, cx| {
                        _ = panel.update(cx, |panel, cx| panel.open_file(&path, None, window, cx));
                    })
                })
        })
        // Every row has the file menu; with several folders, a folder's
        // own row also makes it primary or takes it out of the workspace.
        .context_menu(move |_, entry, menu, _, cx| {
            crate::tree_menu::build(&menu_panel, &entry.item().id, entry.is_folder(), menu, cx)
        })
        .size_full()
        .into_any_element()
    }

    /// Git's view of the workspace's folders: each changed path (as the
    /// tree names it) and what happened to it.
    pub fn set_changes(
        &mut self,
        files: HashMap<String, ph_git::Change>,
        staged: HashSet<String>,
        cx: &mut Context<Self>,
    ) {
        if staged != *self.staged {
            self.staged = Rc::new(staged);
        }
        if files == self.changes.files {
            return;
        }
        let mut dirs = HashSet::new();
        for path in files.keys() {
            let mut dir = path.as_str();
            while let Some((parent, _)) = dir.rsplit_once('/') {
                dirs.insert(parent.to_string());
                dir = parent;
            }
        }
        self.changes = Rc::new(TreeChanges { files, dirs });
        cx.notify();
    }

    fn render_search(&self, cx: &mut Context<Self>) -> AnyElement {
        let results = &self.search.results;
        let mut rows: Vec<AnyElement> = Vec::new();
        if !self.search.files.is_empty() {
            rows.push(section_label("Files", cx));
        }
        for (ix, path) in self.search.files.iter().enumerate() {
            let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
            let open = path.to_string();
            rows.push(
                h_flex()
                    .id(("search-file", ix))
                    .px_2()
                    .gap_1p5()
                    .rounded(cx.theme().radius)
                    .text_xs()
                    .cursor_pointer()
                    .hover(|style| style.bg(cx.theme().list_hover))
                    .child(ph_ui::file_icon_element(name, px(14.)))
                    .child(div().flex_none().child(name.to_string()))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(cx.theme().muted_foreground)
                            .child(dir.to_string()),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_file(&open, None, window, cx);
                    }))
                    .into_any_element(),
            );
        }
        if !results.matches.is_empty() && !self.search.files.is_empty() {
            rows.push(section_label("Text", cx));
        }
        let mut current_file: Option<&str> = None;
        for (ix, found) in results.matches.iter().enumerate() {
            if current_file != Some(found.path.as_str()) {
                current_file = Some(found.path.as_str());
                let (dir, name) = found.path.rsplit_once('/').unwrap_or(("", &found.path));
                let count = results.matches[ix..]
                    .iter()
                    .take_while(|other| other.path == found.path)
                    .count();
                let open = found.path.clone();
                rows.push(
                    h_flex()
                        .id(("search-file-hits", ix))
                        .w_full()
                        .mt_2()
                        .px_2()
                        .gap_1p5()
                        .rounded(cx.theme().radius)
                        .text_xs()
                        .cursor_pointer()
                        .hover(|style| style.bg(cx.theme().list_hover))
                        .child(ph_ui::file_icon_element(name, px(14.)))
                        .child(
                            div()
                                .flex_none()
                                .font_weight(gpui_kit::FontWeight::MEDIUM)
                                .child(name.to_string()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(cx.theme().muted_foreground)
                                .child(dir.to_string()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .px_1()
                                .rounded_full()
                                .bg(cx.theme().muted)
                                .text_color(cx.theme().muted_foreground)
                                .child(count.to_string()),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_file(&open, None, window, cx);
                        }))
                        .into_any_element(),
                );
            }
            let path = found.path.clone();
            let line = found.line.saturating_sub(1) as u32;
            rows.push(
                h_flex()
                    .id(("search-hit", ix))
                    .px_2()
                    .gap_2()
                    .rounded(cx.theme().radius)
                    .text_xs()
                    .cursor_pointer()
                    .hover(|style| style.bg(cx.theme().list_hover))
                    .child(
                        div()
                            .flex_none()
                            .w_8()
                            .text_color(cx.theme().muted_foreground)
                            .child(found.line.to_string()),
                    )
                    .child(div().truncate().child(found.text.clone()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_file(&path, Some(line), window, cx);
                    }))
                    .into_any_element(),
            );
        }

        let status: Option<SharedString> = if self.search.running {
            Some("Searching…".into())
        } else if let Some(error) = &self.search.error {
            Some(error.clone())
        } else if self.search.query.trim().is_empty() {
            Some("File names and text in files.".into())
        } else if results.matches.is_empty() && self.search.files.is_empty() {
            Some("No results.".into())
        } else if results.matches.is_empty() {
            None
        } else if results.truncated {
            Some(format!("First {} matches.", results.matches.len()).into())
        } else {
            Some(format!("{} matches.", results.matches.len()).into())
        };

        let scope = self.search_scope.clone().map(|dir| {
            h_flex()
                .mx_2()
                .mb_1()
                .gap_1()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(Icon::new(Icons::FolderOpen).xsmall())
                .child(div().flex_1().truncate().child(format!("In {dir}")))
                .child(
                    Button::new("search-everywhere")
                        .ghost()
                        .xsmall()
                        .icon(IconName::Close)
                        .tooltip("Search the whole workspace")
                        .accessibility_label("Search the whole workspace")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.search_scope = None;
                            this.search_again(window, cx);
                            cx.notify();
                        })),
                )
        });
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .p_2()
                    .gap_1()
                    .child(
                        Button::new("search-back")
                            .ghost()
                            .xsmall()
                            .icon(IconName::ArrowLeft)
                            .tooltip("Back to the files (Esc)")
                            .accessibility_label("Back to the files")
                            .on_click(cx.listener(|this, _, _, cx| this.show_tree(cx))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            // Escape in the field goes back to the files.
                            .on_key_down(cx.listener(
                                |this, event: &gpui_kit::KeyDownEvent, _, cx| {
                                    if event.keystroke.key == "escape" {
                                        this.show_tree(cx);
                                    }
                                },
                            ))
                            .child(Input::new(&self.search_input).small()),
                    ),
            )
            .children(scope)
            .when_some(status, |this, status| {
                this.child(
                    div()
                        .px_3()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(status),
                )
            })
            .child(
                v_flex()
                    .id("search-results")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_1()
                    .pb_2()
                    .children(rows),
            )
            .into_any_element()
    }

    /// Shows changes (a patch `load` reads) in the Changes tab, made the
    /// first time, and brings it to the front.
    pub fn show_diff(
        &mut self,
        title: impl Into<SharedString>,
        load: crate::diff_tab::LoadDiff,
        cx: &mut Context<Self>,
    ) {
        let title = title.into();
        match &self.diff {
            Some(tab) => tab.update(cx, |tab, cx| tab.show(title, load, cx)),
            None => self.diff = Some(cx.new(|cx| crate::diff_tab::DiffTab::new(title, load, cx))),
        }
        self.diff_active = true;
        self.tabs_scroll.scroll_to_item(0);
        cx.emit(FilesPanelEvent::Opened);
        cx.notify();
    }

    fn close_diff(&mut self, cx: &mut Context<Self>) {
        self.diff = None;
        self.diff_active = false;
        cx.emit(FilesPanelEvent::OpenFilesChanged);
        cx.notify();
    }

    fn render_viewer(&self, cx: &mut Context<Self>) -> AnyElement {
        let diff_tab = self.diff.as_ref().map(|diff| {
            let title = diff.read(cx).title();
            Tab::new()
                .prefix(div().pl_2().child(Icon::new(Icons::GitCompare).xsmall()))
                .child(
                    div()
                        .id("diff-tab-label")
                        .min_w_0()
                        .whitespace_nowrap()
                        .child(title),
                )
                .suffix(
                    Button::new("close-diff")
                        .ghost()
                        .xsmall()
                        .icon(IconName::Close)
                        .accessibility_label("Close the changes")
                        .on_click(cx.listener(|this, _, _, cx| this.close_diff(cx))),
                )
        });
        let showing_diff = self.diff_active && self.diff.is_some();
        let active = self.active.and_then(|ix| self.open_files.get(ix));
        if active.is_none() && self.diff.is_none() {
            return EmptyState::new("No file open")
                .icon(Icons::File)
                .description(format!(
                    "Pick a file in the tree, or press {} to find one.",
                    ph_ui::platform::shortcut("secondary-p")
                ))
                .into_any_element();
        }

        let tabs = self.open_files.iter().enumerate().map(|(ix, file)| {
            let file = file.read(cx);
            let name = file
                .relative_path()
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_string();
            let icon = ph_ui::file_icon_element(&name, px(14.));
            let dirty = file.is_dirty();
            let panel = cx.weak_entity();
            let group = SharedString::from(format!("open-file-{ix}"));
            // As in VS Code: unsaved changes show as a dot where the close
            // button is, which comes back while the pointer is on the tab.
            let close = Button::new(("close-file", ix))
                .ghost()
                .xsmall()
                .icon(IconName::Close)
                .accessibility_label("Close file")
                .on_click(cx.listener(move |this, _, window, cx| {
                    // Not also a click on the tab, which would bring
                    // forward the tab now at this place.
                    cx.stop_propagation();
                    this.close_tab(ix, window, cx);
                }));
            let suffix = div()
                .relative()
                .flex()
                .items_center()
                .justify_center()
                .when(dirty, |this| {
                    this.child(
                        div()
                            .absolute()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .group_hover(group.clone(), |style| style.invisible())
                            .child(
                                div()
                                    .size(px(7.))
                                    .rounded_full()
                                    .bg(cx.theme().foreground.opacity(0.75)),
                            ),
                    )
                })
                .child(
                    div()
                        .when(dirty, |this| {
                            this.invisible()
                                .group_hover(group.clone(), |style| style.visible())
                        })
                        .child(close),
                );
            Tab::new()
                .group(group)
                .prefix(div().pl_2().child(icon))
                .child(
                    div()
                        .id(("open-file-label", ix))
                        .min_w_0()
                        .whitespace_nowrap()
                        .child(name)
                        // A middle click closes the tab.
                        .on_mouse_up(
                            gpui_kit::MouseButton::Middle,
                            cx.listener(move |this, _, window, cx| this.close_tab(ix, window, cx)),
                        )
                        .context_menu(move |menu, _, _| tab_menu(menu, panel.clone(), ix)),
                )
                .suffix(suffix)
        });

        // The Changes tab comes first, where it is seen however many files
        // are open; the files' tabs follow it.
        let offset = usize::from(self.diff.is_some());
        let body = match (showing_diff, &self.diff, active) {
            (true, Some(diff), _) | (false, Some(diff), None) => diff.clone().into_any_element(),
            (_, _, Some(active)) => active.clone().into_any_element(),
            _ => div().into_any_element(),
        };
        v_flex()
            .size_full()
            .child(
                TabBar::new("open-files")
                    .track_scroll(&self.tabs_scroll)
                    .underline()
                    .small()
                    .selected_index(if showing_diff || active.is_none() {
                        0
                    } else {
                        self.active.unwrap_or_default() + offset
                    })
                    .on_click(cx.listener(move |this, ix: &usize, _, cx| {
                        if offset == 1 && *ix == 0 {
                            this.diff_active = true;
                        } else {
                            this.activate(*ix - offset, cx);
                        }
                        cx.emit(FilesPanelEvent::OpenFilesChanged);
                        cx.notify();
                    }))
                    .children(diff_tab)
                    .children(tabs),
            )
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }
}

impl Render for FilesPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The viewer takes only the width it gets; long lines scroll inside
        // the editor instead of widening the column.
        div()
            .key_context(FILES_PANEL_CONTEXT)
            .on_action(cx.listener(|this, _: &crate::NavigateBack, window, cx| {
                this.navigate(true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &crate::NavigateForward, window, cx| {
                this.navigate(false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &crate::NewFile, window, cx| {
                this.new_file_beside_active(window, cx)
            }))
            .on_action(cx.listener(|this, _: &CloseFile, window, cx| {
                if this.diff_active && this.diff.is_some() {
                    this.close_diff(cx);
                } else if let Some(ix) = this.active {
                    this.close_tab(ix, window, cx);
                }
            }))
            .size_full()
            .min_w_0()
            .overflow_hidden()
            .child(self.render_viewer(cx))
    }
}

/// The folder tree (or search results) of a [`FilesPanel`], for the sidebar.
pub struct FileTree {
    files: Entity<FilesPanel>,
    _observe: Subscription,
}

impl FileTree {
    pub fn new(files: Entity<FilesPanel>, cx: &mut Context<Self>) -> Self {
        Self {
            _observe: cx.observe(&files, |_, _, cx| cx.notify()),
            files,
        }
    }

    pub fn files(&self) -> &Entity<FilesPanel> {
        &self.files
    }
}

impl Render for FileTree {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = self.files.update(cx, |files, cx| files.render_browser(cx));
        // The tree's keys act on its selected row.
        fn key<A: gpui_kit::Action>(
            cx: &mut Context<FileTree>,
        ) -> impl Fn(&A, &mut Window, &mut gpui_kit::App) + 'static {
            cx.listener(|this, action: &A, window, cx| {
                this.files
                    .update(cx, |files, cx| files.on_tree_key(action, window, cx))
            })
        }
        div()
            .key_context(crate::FILE_TREE_CONTEXT)
            .size_full()
            .on_action(cx.listener(|this, _: &crate::OpenSelected, window, cx| {
                this.files
                    .update(cx, |files, cx| files.open_selected(window, cx))
            }))
            .on_action(key::<crate::NewFile>(cx))
            .on_action(key::<crate::NewFolder>(cx))
            .on_action(key::<crate::RenameEntry>(cx))
            .on_action(key::<crate::TrashEntry>(cx))
            .on_action(key::<crate::CopyEntry>(cx))
            .on_action(key::<crate::CutEntry>(cx))
            .on_action(key::<crate::PasteEntry>(cx))
            .on_action(key::<crate::DuplicateEntry>(cx))
            .on_action(key::<crate::RevealEntry>(cx))
            .on_action(key::<crate::CopyEntryPath>(cx))
            .on_action(key::<crate::CopyRelativePath>(cx))
            .child(body)
    }
}

/// A heading in the search results.
fn section_label(label: &'static str, cx: &gpui_kit::App) -> AnyElement {
    div()
        .pt_2()
        .px_2()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(label)
        .into_any_element()
}

/// The last part of a path.
fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The menu of open-file tab `ix`.
fn tab_menu(
    menu: gpui_kit::component::menu::PopupMenu,
    panel: gpui_kit::WeakEntity<FilesPanel>,
    ix: usize,
) -> gpui_kit::component::menu::PopupMenu {
    use gpui_kit::component::menu::PopupMenuItem;
    type Run = fn(&mut FilesPanel, usize, &mut Window, &mut Context<FilesPanel>);
    let item = |label: &'static str, run: Run| {
        let panel = panel.clone();
        PopupMenuItem::new(label).on_click(move |_, window, cx| {
            _ = panel.update(cx, |panel, cx| run(panel, ix, window, cx));
        })
    };
    menu.item(
        item("Close", |panel, ix, window, cx| {
            panel.close_tab(ix, window, cx)
        })
        .action(Box::new(CloseFile)),
    )
    .item(item("Close Others", |panel, ix, window, cx| {
        panel.close_others(ix, window, cx)
    }))
    .item(item("Close to the Right", |panel, ix, window, cx| {
        panel.close_right(ix, window, cx)
    }))
    .item(item("Close Saved", |panel, _, window, cx| {
        panel.close_saved(window, cx)
    }))
    .item(item("Close All", |panel, _, window, cx| {
        panel.close_all(window, cx)
    }))
    .separator()
    .item(item("Copy Path", |panel, ix, _, cx| {
        if let Some(path) = panel
            .tab_path(ix, cx)
            .and_then(|path| panel.roots.absolute(&path))
        {
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                path.to_string_lossy().into_owned(),
            ));
        }
    }))
    .item(item("Copy Relative Path", |panel, ix, _, cx| {
        if let Some(path) = panel.tab_path(ix, cx) {
            let within = panel
                .roots
                .split(&path)
                .map(|(_, within)| within.to_string())
                .unwrap_or(path);
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(within));
        }
    }))
    .separator()
    .item(item(ph_ui::platform::reveal_label(), |panel, ix, _, cx| {
        if let Some(path) = panel
            .tab_path(ix, cx)
            .and_then(|path| panel.roots.absolute(&path))
        {
            cx.reveal_path(&path);
        }
    }))
    .item(item("Reveal in Files", |panel, ix, _, cx| {
        if let Some(path) = panel.tab_path(ix, cx) {
            panel.reveal_in_tree(path, cx);
        }
    }))
}

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
    results: SearchResults,
    running: bool,
    error: Option<SharedString>,
    _task: Option<Task<()>>,
}

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
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.run_search(window, cx);
                    }
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
                self._subscriptions.push(cx.subscribe(
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
                    },
                ));
                self.open_files.push(view);
                self.open_files.len() - 1
            }
        };
        if let Some(line) = line {
            self.open_files[ix].update(cx, |file, cx| file.go_to_line(line, window, cx));
        }
        self.activate(ix, cx);
        cx.emit(FilesPanelEvent::OpenFilesChanged);
        cx.emit(FilesPanelEvent::Opened);
        cx.notify();
    }

    /// Closes every tab but `keep`.
    fn close_others(&mut self, keep: usize, cx: &mut Context<Self>) {
        self.close_where(|ix, _| ix != keep, cx);
    }

    /// Closes the tabs after `ix`.
    fn close_right(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.close_where(|other, _| other > ix, cx);
    }

    /// Closes the tabs without unsaved changes.
    fn close_saved(&mut self, cx: &mut Context<Self>) {
        self.close_where(|_, file| !file.is_dirty(), cx);
    }

    pub fn close_all(&mut self, cx: &mut Context<Self>) {
        self.close_where(|_, _| true, cx);
    }

    /// Closes the tabs `close` picks (by index and file), from the last so
    /// the indexes stay right. Unsaved text is kept, as for any closed tab.
    fn close_where(&mut self, close: impl Fn(usize, &FileView) -> bool, cx: &mut Context<Self>) {
        let picked: Vec<usize> = self
            .open_files
            .iter()
            .enumerate()
            .filter(|(ix, file)| close(*ix, file.read(cx)))
            .map(|(ix, _)| ix)
            .collect();
        for ix in picked.into_iter().rev() {
            self.close_file(ix, cx);
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

    pub fn close_file(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.open_files.len() {
            return;
        }
        let closed = self.open_files.remove(ix);
        closed.update(cx, |file, cx| file.detach_lsp(cx));
        self.active = match self.active {
            _ if self.open_files.is_empty() => None,
            Some(active) if active > ix => Some(active - 1),
            Some(active) => Some(active.min(self.open_files.len() - 1)),
            None => None,
        };
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
        let relative: Vec<String> = changed
            .iter()
            .filter_map(|path| self.roots.path_of(path))
            .filter(|path| !path.split('/').any(|part| part == ".git"))
            .collect();

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

    fn run_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.search_input.read(cx).value().to_string();
        let roots = self.roots.clone();
        let scope = self.search_scope.clone();
        self.search.query = query.clone();
        self.search.scope = scope.clone();
        self.search.running = true;
        self.search.error = None;
        let search = cx.background_spawn(async move {
            match &scope {
                Some(dir) => roots.search_in(dir, &query),
                None => roots.search(&query),
            }
        });
        // Replacing the task cancels a search that is still running.
        self.search._task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = search.await;
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
        let mut current_file: Option<&str> = None;
        for (ix, found) in results.matches.iter().enumerate() {
            if current_file != Some(found.path.as_str()) {
                current_file = Some(found.path.as_str());
                rows.push(
                    div()
                        .pt_2()
                        .px_2()
                        .text_xs()
                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                        .truncate()
                        .child(found.path.clone())
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
        } else if self.search.query.is_empty() {
            Some("Type and press Enter.".into())
        } else if results.matches.is_empty() {
            Some("No results.".into())
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
            let label = if file.is_dirty() {
                format!("● {name}")
            } else {
                name
            };
            let panel = cx.weak_entity();
            Tab::new()
                .prefix(div().pl_2().child(icon))
                .child(
                    div()
                        .id(("open-file-label", ix))
                        .min_w_0()
                        .whitespace_nowrap()
                        .child(label)
                        .context_menu(move |menu, _, _| tab_menu(menu, panel.clone(), ix)),
                )
                .suffix(
                    Button::new(("close-file", ix))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Close)
                        .accessibility_label("Close file")
                        .on_click(cx.listener(move |this, _, _, cx| this.close_file(ix, cx))),
                )
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
            .on_action(cx.listener(|this, _: &CloseFile, _, cx| {
                if this.diff_active && this.diff.is_some() {
                    this.close_diff(cx);
                } else if let Some(ix) = this.active {
                    this.close_file(ix, cx);
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
        item("Close", |panel, ix, _, cx| panel.close_file(ix, cx)).action(Box::new(CloseFile)),
    )
    .item(item("Close Others", |panel, ix, _, cx| {
        panel.close_others(ix, cx)
    }))
    .item(item("Close to the Right", |panel, ix, _, cx| {
        panel.close_right(ix, cx)
    }))
    .item(item("Close Saved", |panel, _, _, cx| panel.close_saved(cx)))
    .item(item("Close All", |panel, _, _, cx| panel.close_all(cx)))
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

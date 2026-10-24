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
    tab::{Tab, TabBar},
    tree::{TreeEvent, TreeState, tree},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Task, Window, div, px,
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
struct TreeChanges {
    files: HashMap<String, ph_git::Change>,
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
    /// Ask the agent about selected code.
    Ask(crate::Ask),
    /// Make this folder the workspace's primary one.
    MakePrimary(PathBuf),
    /// Take this folder out of the workspace (it stays on disk).
    RemoveFolder(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Browse {
    Tree,
    Search,
}

#[derive(Default)]
struct Search {
    query: String,
    results: SearchResults,
    running: bool,
    error: Option<SharedString>,
    _task: Option<Task<()>>,
}

pub struct FilesPanel {
    /// The workspace's folders (see [`Roots`]).
    roots: Roots,
    /// Each folder's language servers (started when a file needs one).
    lsps: Vec<Entity<ph_lsp::LspManager>>,
    tree: DirTree,
    tree_state: Entity<TreeState>,
    browse: Browse,
    search_input: Entity<InputState>,
    search: Search,
    open_files: Vec<Entity<FileView>>,
    active: Option<usize>,
    /// File list for the finder; rebuilt lazily after file-system changes.
    index: Option<FileList>,
    index_stale: bool,
    /// Git's changes, marked in the tree.
    changes: Rc<TreeChanges>,
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
        let mut panel = Self {
            lsps,
            roots,
            tree: DirTree::default(),
            tree_state,
            browse: Browse::Tree,
            search_input,
            search: Search::default(),
            open_files: Vec::new(),
            active: None,
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
            self.active = Some(ix);
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
                    |_, _, event: &crate::file_view::FileViewEvent, cx| match event {
                        crate::file_view::FileViewEvent::Ask(ask) => {
                            cx.emit(FilesPanelEvent::Ask(ask.clone()))
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
        self.active = Some(ix);
        cx.emit(FilesPanelEvent::OpenFilesChanged);
        cx.notify();
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

    /// Some file is open (the editor column shows only then).
    pub fn has_open_files(&self) -> bool {
        !self.open_files.is_empty()
    }

    /// The tree or the search results, for [`FileTree`].
    fn render_browser(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.browse {
            Browse::Tree => self.render_tree(cx),
            Browse::Search => self.render_search(cx),
        }
    }

    fn index_of(&self, relative: &str, cx: &gpui_kit::App) -> Option<usize> {
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
    fn load_dirs(&mut self, dirs: Vec<String>, cx: &mut Context<Self>) {
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

    fn rebuild_tree(&mut self, cx: &mut Context<Self>) {
        let items = self.tree.items();
        self.tree_state.update(cx, |state, cx| {
            let selected = state.selected_item().map(|item| item.id.clone());
            state.set_items(items, cx);
            if let Some(ix) = selected.and_then(|id| state.index_of(&id)) {
                state.set_selected_index(Some(ix), cx);
            }
        });
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

    fn run_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.search_input.read(cx).value().to_string();
        let roots = self.roots.clone();
        self.search.query = query.clone();
        self.search.running = true;
        self.search.error = None;
        let search = cx.background_spawn(async move { roots.search(&query) });
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
        let multi = self.roots.list().len() > 1;
        let primary = self.roots.list().first().map(|root| root.name.clone());
        let roots: Vec<(String, PathBuf)> = self
            .roots
            .list()
            .iter()
            .map(|root| (root.name.clone(), root.path.clone()))
            .collect();
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
        // With several folders, a folder's menu makes it primary or takes
        // it out of the workspace.
        .context_menu(move |_, entry, menu, _, _| {
            if !multi || entry.depth() != 0 {
                return menu;
            }
            let name = entry.item().id.to_string();
            let Some((_, root)) = roots.iter().find(|(root, _)| *root == name).cloned() else {
                return menu;
            };
            let (primary_panel, remove_panel, remove_root) =
                (menu_panel.clone(), menu_panel.clone(), root.clone());
            menu.item(
                gpui_kit::component::menu::PopupMenuItem::new("Make primary (agents start here)")
                    .on_click(move |_, _, cx| {
                        _ = primary_panel.update(cx, |_, cx| {
                            cx.emit(FilesPanelEvent::MakePrimary(root.clone()))
                        });
                    }),
            )
            .item(
                gpui_kit::component::menu::PopupMenuItem::new("Remove from workspace").on_click(
                    move |_, _, cx| {
                        _ = remove_panel.update(cx, |_, cx| {
                            cx.emit(FilesPanelEvent::RemoveFolder(remove_root.clone()))
                        });
                    },
                ),
            )
        })
        .size_full()
        .into_any_element()
    }

    /// Git's view of the workspace's folders: each changed path (as the
    /// tree names it) and what happened to it.
    pub fn set_changes(&mut self, files: HashMap<String, ph_git::Change>, cx: &mut Context<Self>) {
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

        v_flex()
            .size_full()
            .child(div().p_2().child(Input::new(&self.search_input).small()))
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

    fn render_viewer(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(active) = self.active.and_then(|ix| self.open_files.get(ix)) else {
            return EmptyState::new("No file open")
                .icon(Icons::File)
                .description(format!(
                    "Pick a file in the tree, or press {} to find one.",
                    ph_ui::platform::shortcut("secondary-p")
                ))
                .into_any_element();
        };

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
            Tab::new()
                .prefix(div().pl_2().child(icon))
                .label(label)
                .suffix(
                    Button::new(("close-file", ix))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Close)
                        .accessibility_label("Close file")
                        .on_click(cx.listener(move |this, _, _, cx| this.close_file(ix, cx))),
                )
        });

        v_flex()
            .size_full()
            .child(
                TabBar::new("open-files")
                    .underline()
                    .small()
                    .selected_index(self.active.unwrap_or_default())
                    .on_click(cx.listener(|this, ix: &usize, _, cx| {
                        this.active = Some(*ix);
                        cx.emit(FilesPanelEvent::OpenFilesChanged);
                        cx.notify();
                    }))
                    .children(tabs),
            )
            .child(div().flex_1().min_h_0().child(active.clone()))
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
                if let Some(ix) = this.active {
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
        div().size_full().child(body)
    }
}

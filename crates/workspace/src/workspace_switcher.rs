//! The workspace switcher: the list that opens from the workspace's name
//! in the title bar, as a project switcher does in an editor.
//!
//! The workspaces used to stand in the column of icons, a badge each. With
//! many of them the column was full of badges, the button to add one was
//! past its end, and every place added to the column pushed them further
//! down. Here they are one list, however many there are: the one used last
//! on top, each with its colour, its folder and a dot for what goes on in
//! it, found by typing a part of its name or folder, and opened with a
//! click or with the arrow keys and Enter. Opening a folder and making a
//! workspace without one are at its foot, always in view.
//!
//! What can be done to a workspace (a new chat or task in it, a folder
//! added, renamed, moved to a window of its own, another colour, removed)
//! opens under its row, from the button at its end or a right click: the
//! switcher is a popover, and a menu over it would close it.

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Action, AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _,
    IntoElement, KeyDownEvent, MouseButton, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, WeakEntity, Window, div,
};
use ph_store::projects::{Project, ProjectId};
use ph_ui::Icons;

use crate::actions::*;
use crate::status::WorkspaceStatus;
use crate::workspace::{View, Workspace};

pub(crate) struct WorkspaceSwitcher {
    workspace: WeakEntity<Workspace>,
    search: Entity<InputState>,
    /// The row the arrow keys are on, which Enter opens.
    cursor: usize,
    /// The workspace whose actions show under its row.
    open: Option<ProjectId>,
    /// Its colours show there, to pick one.
    colours: bool,
    _subscriptions: Vec<Subscription>,
}

impl WorkspaceSwitcher {
    pub(crate) fn new(
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Find a workspace…"));
        let subscriptions = vec![cx.subscribe_in(
            &search,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    // What is typed starts at the best match.
                    this.cursor = 0;
                    this.open = None;
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.enter(window, cx),
                _ => {}
            },
        )];
        Self {
            workspace,
            search,
            cursor: 0,
            open: None,
            colours: false,
            _subscriptions: subscriptions,
        }
    }

    /// The switcher opens: an empty search with the keyboard, and the
    /// arrow keys on the workspace used before the one in front, so that
    /// Enter goes back to it.
    pub(crate) fn opened(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = None;
        self.colours = false;
        self.cursor = 1;
        let search = self.search.clone();
        // Once it is drawn: there is nothing to focus before.
        window.defer(cx, move |window, cx| {
            search.update(cx, |search, cx| {
                search.set_value("", window, cx);
                search.focus(window, cx);
            })
        });
        // Emptying the search put the keys on the first row again.
        cx.defer_in(window, |this, _, cx| {
            this.cursor = 1;
            cx.notify();
        });
        cx.notify();
    }

    /// The user's workspaces that have what is typed in their name or in
    /// one of their folders, the one used last first.
    fn found(&self, workspace: &Workspace, cx: &App) -> Vec<Project> {
        let typed = self.search.read(cx).value().trim().to_lowercase();
        workspace
            .recent()
            .into_iter()
            .filter(|project| matches(project, &typed))
            .cloned()
            .collect()
    }

    fn close(&self, window: &mut Window, cx: &mut App) {
        _ = self.workspace.update(cx, |workspace, cx| {
            workspace.set_switcher_open(false, window, cx)
        });
    }

    /// Shows workspace `id`.
    fn switch_to(&mut self, id: ProjectId, window: &mut Window, cx: &mut Context<Self>) {
        self.close(window, cx);
        let workspace = self.workspace.clone();
        // Once the popover has gone: the workspace takes the keyboard.
        window.defer(cx, move |window, cx| {
            _ = workspace.update(cx, |workspace, cx| {
                workspace.open_project(id, window, cx);
                workspace.focus_front(window, cx);
            });
        });
    }

    /// Enter: the row the arrow keys are on.
    fn enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let found = self.found(workspace.read(cx), cx);
        if let Some(project) = found.get(self.cursor.min(found.len().saturating_sub(1))) {
            self.switch_to(project.id, window, cx);
        }
    }

    /// The arrow keys move through the rows (the search keeps the others).
    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let rows = self
            .workspace
            .upgrade()
            .map_or(0, |workspace| self.found(workspace.read(cx), cx).len());
        if rows == 0 {
            return;
        }
        let cursor = self.cursor.min(rows - 1);
        self.cursor = match event.keystroke.key.as_str() {
            "down" => (cursor + 1) % rows,
            "up" => (cursor + rows - 1) % rows,
            _ => return,
        };
        cx.stop_propagation();
        cx.notify();
    }

    /// Shows workspace `id` and runs `action` there, as its menu does:
    /// the actions are the ones the workspace in front takes.
    fn act(
        &mut self,
        id: ProjectId,
        action: Box<dyn Action>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close(window, cx);
        let workspace = self.workspace.clone();
        window.defer(cx, move |window, cx| {
            let here = workspace
                .update(cx, |workspace, cx| {
                    workspace.open_project(id, window, cx);
                    workspace.view() == View::Workspaces
                        && workspace.active_project_id() == Some(id)
                })
                .unwrap_or(false);
            // Shown in another window: that window came forward, and the
            // action is not run here, on whatever this one shows.
            if here {
                window.dispatch_action(action, cx);
            }
        });
    }

    /// The actions of the workspace whose row is open, under it.
    fn render_actions(&self, project: &Project, cx: &mut Context<Self>) -> AnyElement {
        let id = project.id;
        let theme = cx.theme();
        if self.colours {
            let current = project.shade(ph_ui::WORKSPACE_COLORS.len());
            return h_flex()
                .pl(ph_ui::rems_from_px(42.))
                .pr_2()
                .pb_1p5()
                .gap_1p5()
                .flex_wrap()
                .children(
                    ph_ui::WORKSPACE_COLORS
                        .iter()
                        .enumerate()
                        .map(|(ix, (name, _))| {
                            let color = ph_ui::workspace_color(ix, theme.is_dark());
                            div()
                                .id(("workspace-colour", ix))
                                .flex_none()
                                .size(ph_ui::rems_from_px(18.))
                                .rounded_full()
                                .bg(color)
                                .border_2()
                                .border_color(if ix == current {
                                    theme.foreground
                                } else {
                                    gpui_kit::transparent_black()
                                })
                                .cursor_pointer()
                                .tooltip({
                                    let name: SharedString = (*name).into();
                                    move |window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(name.clone())
                                            .build(window, cx)
                                    }
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    _ = this.workspace.update(cx, |workspace, cx| {
                                        workspace.set_color(id, Some(ix as u8), cx)
                                    });
                                    this.colours = false;
                                    cx.notify();
                                }))
                        }),
                )
                .into_any_element();
        }
        let action = |key: &'static str,
                      label: &'static str,
                      icon: Icons,
                      make: fn() -> Box<dyn Action>,
                      cx: &mut Context<Self>| {
            Button::new(SharedString::from(format!("workspace-{key}-{id}")))
                .ghost()
                .xsmall()
                .icon(icon)
                .label(label)
                .on_click(cx.listener(move |this, _, window, cx| this.act(id, make(), window, cx)))
        };
        let task = crate::tasks::of(crate::AppState::store(cx), id).is_some();
        h_flex()
            .pl(ph_ui::rems_from_px(38.))
            .pr_2()
            .pb_1p5()
            .gap_0p5()
            .flex_wrap()
            .child(
                Button::new(SharedString::from(format!("workspace-chat-{id}")))
                    .ghost()
                    .xsmall()
                    .icon(Icons::SquarePen)
                    .label("New chat")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.close(window, cx);
                        let workspace = this.workspace.clone();
                        window.defer(cx, move |window, cx| {
                            _ = workspace
                                .update(cx, |workspace, cx| workspace.new_chat_in(id, window, cx));
                        });
                    })),
            )
            .child(action(
                "task",
                "New task",
                Icons::GitBranch,
                || Box::new(NewTask),
                cx,
            ))
            .when(task, |this| {
                this.child(action(
                    "finish",
                    "Finish task",
                    Icons::Check,
                    || Box::new(FinishTask),
                    cx,
                ))
            })
            .child(action(
                "folder",
                "Add a folder",
                Icons::FolderPlus,
                || Box::new(AddFolderToWorkspace),
                cx,
            ))
            .child(action(
                "rename",
                "Rename",
                Icons::Pencil,
                || Box::new(RenameWorkspace),
                cx,
            ))
            .child(action(
                "window",
                "New window",
                Icons::ExternalLink,
                || Box::new(OpenInNewWindow),
                cx,
            ))
            .child(
                Button::new(SharedString::from(format!("workspace-colour-{id}")))
                    .ghost()
                    .xsmall()
                    .icon(Icons::Palette)
                    .label("Colour")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.colours = true;
                        cx.notify();
                    })),
            )
            .child(
                action(
                    "remove",
                    "Remove",
                    Icons::Trash,
                    || Box::new(RemoveWorkspace),
                    cx,
                )
                .tooltip(concat!(
                    "Remove from ",
                    ph_brand::name!(),
                    " (its folders stay)"
                )),
            )
            .into_any_element()
    }

    /// One workspace: its colour and letter, its name over its folder,
    /// what goes on in it, and the button for what can be done. The one
    /// to go `back` to says the key that does so.
    #[allow(clippy::too_many_arguments)]
    fn render_row(
        &self,
        project: &Project,
        row: usize,
        in_front: bool,
        status: WorkspaceStatus,
        back: bool,
        on_cursor: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let id = project.id;
        let color = ph_ui::workspace_color(
            project.shade(ph_ui::WORKSPACE_COLORS.len()),
            theme.is_dark(),
        );
        let folders = project.folders();
        let place: SharedString = match folders.as_slice() {
            [] => "No folders".into(),
            [only] => place_of(only).into(),
            [first, rest @ ..] => format!("{} and {} more", place_of(first), rest.len()).into(),
        };
        // Every folder in full, for the tooltip of a line that leaves
        // some of it out (a long path cut short, or folders not named).
        let whole: Vec<SharedString> = folders
            .iter()
            .map(|folder| ph_ui::platform::display_path(folder).into())
            .collect();
        let cut = match whole.as_slice() {
            [] => false,
            [only] => *only != place,
            _ => true,
        };
        let open = self.open == Some(id);
        let dot = status_color(&status, cx);
        let keys = back.then_some(if cfg!(target_os = "macos") {
            "⌃⇥"
        } else {
            "Ctrl+Tab"
        });
        let group: SharedString = format!("workspace-row-{id}").into();
        v_flex()
            .flex_none()
            .rounded(theme.radius)
            .when(on_cursor || open, |this| this.bg(theme.list_hover))
            .child(
                h_flex()
                    .id(("switch-workspace", id as u64))
                    .group(group.clone())
                    .role(gpui_kit::accesskit::Role::MenuItem)
                    .aria_label(SharedString::from(project.name.clone()))
                    .h(ph_ui::rems_from_px(44.))
                    .px_2()
                    .gap_2p5()
                    .rounded(theme.radius)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.list_hover))
                    // Findable in UI tests and by the automation socket.
                    .test_support()
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.switch_to(id, window, cx)),
                    )
                    // A right click is the button at the row's end.
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, _, _, cx| {
                            this.open = (this.open != Some(id)).then_some(id);
                            this.colours = false;
                            this.cursor = row;
                            cx.notify();
                        }),
                    )
                    .child(badge(&project.name, color, in_front, 24., cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .gap_1p5()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_sm()
                                            .when(in_front, |this| {
                                                this.font_weight(gpui_kit::FontWeight::MEDIUM)
                                            })
                                            .child(project.name.clone()),
                                    )
                                    .children(dot.map(|color| {
                                        div()
                                            .flex_none()
                                            .size(ph_ui::rems_from_px(7.))
                                            .rounded_full()
                                            .bg(color)
                                    })),
                            )
                            .child(
                                div()
                                    .id(("workspace-place", id as u64))
                                    .truncate()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(place)
                                    .when(cut, |this| {
                                        this.tooltip(move |window, cx| {
                                            let whole = whole.clone();
                                            gpui_kit::component::tooltip::Tooltip::element(
                                                move |_, _| v_flex().children(whole.clone()),
                                            )
                                            .build(window, cx)
                                        })
                                    }),
                            ),
                    )
                    .children(keys.map(|keys| {
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(keys)
                    }))
                    .when(in_front, |this| {
                        this.child(
                            Icon::new(IconName::Check)
                                .small()
                                .text_color(theme.muted_foreground),
                        )
                    })
                    .child(
                        div()
                            // There while the row is pointed at, or open.
                            .when(!open, |this| {
                                this.invisible().group_hover(group, |style| style.visible())
                            })
                            .child(
                                Button::new(SharedString::from(format!("workspace-more-{id}")))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Ellipsis)
                                    .tooltip("What can be done with this workspace")
                                    .accessibility_label("Workspace actions")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.open = (this.open != Some(id)).then_some(id);
                                        this.colours = false;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .when(open, |this| this.child(self.render_actions(project, cx)))
            .into_any_element()
    }
}

/// A workspace's mark: the first letter of its name on its colour, the same
/// in a row of the list and beside the name in the title bar. `strong` for
/// the workspace in front.
pub(crate) fn badge(
    name: &str,
    color: gpui_kit::Hsla,
    strong: bool,
    size: f32,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let letter: SharedString = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect::<String>())
        .unwrap_or_else(|| "·".into())
        .into();
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(ph_ui::rems_from_px(size))
        .rounded(theme.radius)
        .bg(color.opacity(if strong { 0.36 } else { 0.2 }))
        .text_color(if theme.is_dark() {
            theme.foreground
        } else {
            color
        })
        .text_size(ph_ui::rems_from_px(size / 2.))
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .child(letter)
        .into_any_element()
}

/// A folder as a row says it: from the home folder (`~`) when it is short
/// enough to read whole, else its last folders, which tell it apart (the
/// start of a long path is the same for all of them).
fn place_of(folder: &std::path::Path) -> String {
    const WHOLE: usize = 44;
    let whole = ph_ui::platform::display_path(folder);
    if whole.chars().count() <= WHOLE {
        return whole;
    }
    let parts: Vec<String> = folder
        .components()
        .filter_map(|part| match part {
            std::path::Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let tail = &parts[parts.len().saturating_sub(2)..];
    format!(
        "…{}{}",
        std::path::MAIN_SEPARATOR,
        tail.join(std::path::MAIN_SEPARATOR_STR)
    )
}

/// Whether `project` has `typed` (lowercase) in its name or a folder.
fn matches(project: &Project, typed: &str) -> bool {
    typed.is_empty()
        || project.name.to_lowercase().contains(typed)
        || project
            .folders()
            .iter()
            .any(|folder| folder.to_string_lossy().to_lowercase().contains(typed))
}

/// The colour of the dot for what goes on in a workspace: amber when an
/// agent waits for an answer, red for failing tests, green for news not
/// seen, blue while one works.
pub(crate) fn status_color(status: &WorkspaceStatus, cx: &App) -> Option<gpui_kit::Hsla> {
    let theme = cx.theme();
    Some(if status.waiting > 0 {
        theme.warning
    } else if status.failed {
        theme.danger
    } else if status.unseen > 0 {
        theme.success
    } else if status.working > 0 {
        theme.info
    } else {
        return None;
    })
}

impl Render for WorkspaceSwitcher {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(workspace) = self.workspace.upgrade() else {
            return div().into_any_element();
        };
        let (found, in_front, statuses, before, any) = {
            let workspace = workspace.read(cx);
            let found = self.found(workspace, cx);
            let in_front = (workspace.view() == View::Workspaces)
                .then(|| workspace.active_project_id())
                .flatten();
            let statuses: Vec<WorkspaceStatus> = found
                .iter()
                .map(|project| workspace.status(project.id))
                .collect();
            (
                found,
                in_front,
                statuses,
                workspace.before(),
                workspace.listed().next().is_some(),
            )
        };
        let cursor = self.cursor.min(found.len().saturating_sub(1));
        let searching = !self.search.read(cx).value().trim().is_empty();
        let muted = cx.theme().muted_foreground;
        let rows: Vec<AnyElement> = found
            .iter()
            .zip(statuses)
            .enumerate()
            .map(|(row, (project, status))| {
                self.render_row(
                    project,
                    row,
                    in_front == Some(project.id),
                    status,
                    before == Some(project.id),
                    row == cursor,
                    cx,
                )
            })
            .collect();
        let none = rows.is_empty();
        let foot = |id: &'static str,
                    label: &'static str,
                    icon: Icons,
                    make: fn() -> Box<dyn Action>,
                    cx: &mut Context<Self>| {
            h_flex()
                .id(id)
                .h_8()
                .px_2()
                .gap_2p5()
                .rounded(cx.theme().radius)
                .cursor_pointer()
                .text_sm()
                .hover(|style| style.bg(cx.theme().list_hover))
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .justify_center()
                        .w(ph_ui::rems_from_px(24.))
                        .child(Icon::new(icon).small().text_color(muted)),
                )
                .child(label)
                .test_support()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.close(window, cx);
                    window.dispatch_action(make(), cx);
                }))
        };
        v_flex()
            .id("workspace-switcher-list")
            .w(ph_ui::rems_from_px(380.))
            .gap_1()
            .capture_key_down(cx.listener(|this, event, _, cx| this.key_down(event, cx)))
            .child(
                Input::new(&self.search)
                    .small()
                    .prefix(Icon::new(IconName::Search).xsmall().text_color(muted)),
            )
            .child(
                v_flex()
                    .id("workspace-switcher-rows")
                    .max_h(ph_ui::rems_from_px(420.))
                    .overflow_y_scroll()
                    .gap_0p5()
                    .children(rows)
                    .when(none, |this| {
                        this.child(div().px_2().py_2().text_sm().text_color(muted).child(
                            if searching {
                                "No workspace of that name or folder."
                            } else if any {
                                "No other workspaces."
                            } else {
                                "No workspaces yet. Open a folder to start."
                            },
                        ))
                    }),
            )
            .child(div().flex_none().h_px().bg(cx.theme().border))
            .child(foot(
                "switcher-open-folder",
                "Open a folder…",
                Icons::FolderOpen,
                || Box::new(AddProject),
                cx,
            ))
            .child(foot(
                "switcher-new-workspace",
                "New workspace without folders",
                Icons::SquarePen,
                || Box::new(NewWorkspace),
                cx,
            ))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ph_store::projects::ProjectKind;

    fn project(name: &str, path: &str) -> Project {
        Project {
            id: 1,
            name: name.into(),
            path: path.into(),
            last_opened_at: 0,
            extra_folders: vec!["/work/shared/design".into()],
            empty: false,
            kind: ProjectKind::Folder,
            color: None,
        }
    }

    #[test]
    fn a_long_folder_is_said_by_its_end() {
        let long =
            std::path::Path::new("/Volumes/work/clients/acme/2026/mobile/android-client-rewrite");
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(
            place_of(long),
            format!("…{sep}mobile{sep}android-client-rewrite")
        );
        assert_eq!(place_of(std::path::Path::new("/srv/app")), "/srv/app");
    }

    #[test]
    fn a_workspace_is_found_by_its_name_or_a_folder() {
        let app = project("Mobile App", "/Users/sam/code/android-client");
        assert!(matches(&app, ""));
        assert!(matches(&app, "mobile"));
        assert!(matches(&app, "android-cl"));
        assert!(matches(&app, "design"), "a folder added to it");
        assert!(!matches(&app, "backend"));
    }
}

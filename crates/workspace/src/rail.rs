//! The column of icons at the window's left edge: where the window can go.
//! First the places that are always there (the Bridge, Reviews, *Chats* for
//! the chats that need no folder, an organisation's own views), then, under
//! a line, the user's workspaces, each a badge in its colour, then a way to
//! add one, and Settings at the foot. An entry shows its name under it, or
//! in a tooltip when the names are switched off (Settings → Appearance).
//!
//! *Chats* is a workspace like the others underneath, but it is not one of
//! the user's projects: it cannot be removed and has no folder. So it does
//! not sit among them as a letter on a colour, moving down as they are
//! added; it has a fixed place and an icon, like the Bridge.

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    menu::{ContextMenuExt as _, PopupMenuItem},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div,
};
use ph_store::projects::{Project, ProjectId};
use ph_ui::{Icons, MenuButton as _};

use crate::actions::{AddProject, NewWorkspace, OpenSettings};
use crate::workspace::{View, Workspace};

/// A column's width in pixels at the default interface size: icons alone,
/// or wider with their names.
pub(crate) fn width_px(labels: bool) -> f32 {
    if labels { 64. } else { 42. }
}

/// Whether names show under the icons (they do unless switched off).
pub(crate) fn labels(cx: &App) -> bool {
    cx.try_global::<ph_theme::Preferences>()
        .is_none_or(|preferences| preferences.rail_names)
}

/// One entry of a column.
pub(crate) struct Item {
    pub id: SharedString,
    pub label: SharedString,
    pub icon: Icon,
    /// Its shortcut, said in the tooltip.
    pub hint: Option<String>,
    /// What the tooltip says of it, where its name is not enough.
    pub about: Option<&'static str>,
    pub selected: bool,
    /// A count or a dot at its top right corner.
    pub badge: Option<AnyElement>,
}

impl Item {
    /// The entry; `on_click` runs when it is pressed.
    pub(crate) fn render(
        self,
        labels: bool,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
        cx: &App,
    ) -> AnyElement {
        self.element(labels, on_click, cx)
            // Findable by label in UI tests and automation.
            .test_support()
            .into_any_element()
    }

    /// The entry, for one that takes more (a menu on a right click).
    pub(crate) fn element(
        self,
        labels: bool,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
        cx: &App,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let theme = cx.theme();
        let said = self.about.map_or(self.label.to_string(), str::to_string);
        let tip = match &self.hint {
            Some(hint) => format!("{said} ({hint})"),
            None => said,
        };
        v_flex()
            .id(self.id)
            .role(gpui_kit::accesskit::Role::Tab)
            .aria_label(self.label.clone())
            .relative()
            .flex_none()
            .w(ph_ui::rems_from_px(width_px(labels) - 6.))
            .when(labels, |this| this.py_1p5().gap_1())
            .when(!labels, |this| this.h(ph_ui::rems_from_px(34.)))
            .items_center()
            .justify_center()
            .rounded(theme.radius)
            .cursor_pointer()
            .text_color(if self.selected {
                theme.foreground
            } else {
                theme.muted_foreground
            })
            .when(self.selected, |this| this.bg(theme.secondary_hover))
            .when(!self.selected, |this| {
                this.hover(|style| style.bg(theme.secondary_hover).text_color(theme.foreground))
            })
            // With names shown the tooltip would only repeat them.
            .when(
                !labels || self.hint.is_some() || self.about.is_some(),
                |this| {
                    this.tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                    })
                },
            )
            .on_click(move |_, window, cx| on_click(window, cx))
            .child(self.icon.size(ph_ui::rems_from_px(18.)))
            .when(labels, |this| {
                this.child(
                    div()
                        .max_w_full()
                        .truncate()
                        .text_size(ph_ui::rems_from_px(10.))
                        .when(self.selected, |this| {
                            this.font_weight(gpui_kit::FontWeight::MEDIUM)
                        })
                        .child(self.label),
                )
            })
            .children(self.badge)
    }
}

/// A count at an entry's corner (99+ beyond two digits).
pub(crate) fn count_badge(count: usize, color: Option<Hsla>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .absolute()
        .top(ph_ui::rems_from_px(3.))
        .right(ph_ui::rems_from_px(3.))
        .min_w(ph_ui::rems_from_px(14.))
        .h(ph_ui::rems_from_px(14.))
        .px(ph_ui::rems_from_px(3.))
        .rounded_full()
        .map(|this| match color {
            Some(color) => this.bg(color.opacity(0.22)).text_color(color),
            None => this.bg(theme.secondary_active).text_color(theme.foreground),
        })
        .text_size(ph_ui::rems_from_px(9.))
        .flex()
        .items_center()
        .justify_center()
        .child(if count > 99 {
            "99+".to_string()
        } else {
            count.to_string()
        })
        .into_any_element()
}

/// A dot at an entry's corner: something there wants a look.
pub(crate) fn dot_badge(color: Hsla) -> AnyElement {
    div()
        .absolute()
        .top(ph_ui::rems_from_px(5.))
        .right(ph_ui::rems_from_px(7.))
        .size(ph_ui::rems_from_px(7.))
        .rounded_full()
        .bg(color)
        .into_any_element()
}

/// A column of entries.
pub(crate) fn column(
    id: &'static str,
    labels: bool,
    items: impl IntoIterator<Item = AnyElement>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    v_flex()
        .id(id)
        .role(gpui_kit::accesskit::Role::TabList)
        .flex_none()
        .w(ph_ui::rems_from_px(width_px(labels)))
        .h_full()
        .py_1()
        .gap_0p5()
        .items_center()
        .children(items)
}

/// The left column: where the window can go.
pub(crate) fn render_places(
    workspace: &Workspace,
    cx: &mut gpui_kit::Context<Workspace>,
) -> impl IntoElement {
    let labels = labels(cx);
    let this = cx.entity().downgrade();
    let view = workspace.view();
    let mac = cfg!(target_os = "macos");
    let hint = |mac_keys: &str, other: &str| Some(if mac { mac_keys } else { other }.to_string());
    let show = |target: View| {
        let this = this.clone();
        move |window: &mut Window, cx: &mut App| {
            _ = this.update(cx, |workspace, cx| workspace.show(target, window, cx));
        }
    };

    let (needs_you, _) = crate::bridge::counts(workspace, cx);
    let waiting = workspace
        .github_view()
        .map_or(0, |github| github.read(cx).waiting_for_review());

    let mut places = vec![
        Item {
            id: "view-bridge".into(),
            label: "Bridge".into(),
            icon: Icon::new(Icons::ShipWheel),
            hint: hint("⌘⇧E", "Ctrl+Shift+E"),
            about: None,
            selected: view == View::Bridge,
            badge: (needs_you > 0 && view != View::Bridge)
                .then(|| count_badge(needs_you, Some(cx.theme().warning), cx)),
        }
        .render(labels, show(View::Bridge), cx),
    ];
    if ph_brand::enabled(ph_brand::Feature::GitHub) {
        places.push(
            Item {
                id: "view-github".into(),
                label: "Reviews".into(),
                icon: Icon::new(Icons::GitPullRequest),
                hint: hint("⌘⇧G", "Ctrl+Shift+G"),
                about: None,
                selected: view == View::GitHub,
                badge: (waiting > 0).then(|| count_badge(waiting, Some(cx.theme().warning), cx)),
            }
            .render(labels, show(View::GitHub), cx),
        );
    }
    // The workspace in front, when a workspace's screen is.
    let in_front = (view == View::Workspaces)
        .then(|| workspace.active_project_id())
        .flatten();
    // Chats: the chats that need no folder. Always here, in this place.
    if let Some(chats) = workspace.chats_place() {
        let id = chats.id;
        let (open, menu_owner) = (this.clone(), this.clone());
        places.push(
            Item {
                id: "view-chats".into(),
                // Its name is the user's to change, like a workspace's.
                label: chats.name.clone().into(),
                icon: Icon::new(Icons::MessagesSquare),
                hint: hint("⌘⇧D", "Ctrl+Shift+D"),
                about: Some("Chats that need no folder"),
                selected: in_front == Some(id),
                badge: status_dot(&workspace.status(id), cx),
            }
            .element(
                labels,
                move |window, cx| {
                    _ = open.update(cx, |workspace, cx| workspace.open_project(id, window, cx));
                },
                cx,
            )
            .test_support()
            .context_menu(move |menu, window, cx| {
                crate::bridge::workspace_menu_items(menu, menu_owner.clone(), id, window, cx)
            })
            .into_any_element(),
        );
    }
    for custom in ph_custom::views() {
        places.push(
            Item {
                id: custom.id.into(),
                label: custom.label.into(),
                icon: (custom.icon)(),
                hint: None,
                about: None,
                selected: view == View::Custom(custom.id),
                badge: None,
            }
            .render(labels, show(View::Custom(custom.id)), cx),
        );
    }

    // The user's workspaces, each a badge, numbered as their keys are.
    let listed: Vec<Project> = workspace.listed().cloned().collect();
    let badges: Vec<AnyElement> = listed
        .iter()
        .enumerate()
        .map(|(ix, project)| workspace_badge(workspace, project, ix, in_front, labels, cx))
        .collect();

    column("view-rail", labels, places)
        .child(
            div()
                .flex_none()
                .w(ph_ui::rems_from_px(20.))
                .h_px()
                .my_1()
                .bg(cx.theme().border),
        )
        .child(
            v_flex()
                .id("rail-workspaces")
                .flex_1()
                .min_h_0()
                .w_full()
                .items_center()
                .gap_1()
                .overflow_y_scroll()
                .children(badges)
                .child(
                    Button::new("rail-add-workspace")
                        .ghost()
                        .small()
                        .icon(IconName::Plus)
                        .tooltip("Open a folder as a workspace, or make one without folders")
                        .accessibility_label("Add a workspace")
                        .menu(gpui_kit::Anchor::TopLeft, |menu, _, _| {
                            menu.item(
                                PopupMenuItem::new("Open a folder as a workspace")
                                    .icon(Icon::new(Icons::FolderOpen))
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(Box::new(AddProject), cx)
                                    }),
                            )
                            .item(
                                PopupMenuItem::new("New workspace without folders")
                                    .icon(Icon::new(Icons::SquarePen))
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(Box::new(NewWorkspace), cx)
                                    }),
                            )
                        }),
                ),
        )
        .child(
            Item {
                id: "open-settings".into(),
                label: "Settings".into(),
                icon: Icon::new(IconName::Settings),
                hint: hint("⌘,", "Ctrl+,"),
                about: None,
                selected: false,
                badge: None,
            }
            .render(
                labels,
                |window, cx| window.dispatch_action(Box::new(OpenSettings), cx),
                cx,
            ),
        )
}

/// A workspace in the column: its first letter on its colour, a dot for
/// what it is doing, a ring while it is in front. A right click has what
/// can be done to it.
fn workspace_badge(
    workspace: &Workspace,
    project: &Project,
    ix: usize,
    in_front: Option<ProjectId>,
    labels: bool,
    cx: &mut gpui_kit::Context<Workspace>,
) -> AnyElement {
    let theme = cx.theme();
    let id = project.id;
    let selected = in_front == Some(id);
    let color = ph_ui::workspace_color(
        project.shade(ph_ui::WORKSPACE_COLORS.len()),
        theme.is_dark(),
    );
    let status = workspace.status(id);
    let summary = status.summary(cx).map(|(text, _)| text);
    let keys = (ix < 9).then(|| {
        if cfg!(target_os = "macos") {
            format!("⌃{}", ix + 1)
        } else {
            format!("Alt+{}", ix + 1)
        }
    });
    let tip = match (summary, keys) {
        (Some(text), Some(keys)) => format!("{} · {text} ({keys})", project.name),
        (Some(text), None) => format!("{} · {text}", project.name),
        (None, Some(keys)) => format!("{} ({keys})", project.name),
        (None, None) => project.name.clone(),
    };
    let letter: SharedString = project
        .name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect::<String>())
        .unwrap_or_else(|| "·".into())
        .into();
    let entity = cx.entity().downgrade();
    let (open, menu_owner) = (entity.clone(), entity);
    let name: SharedString = project.name.clone().into();
    v_flex()
        .id(("rail-workspace", id as u64))
        .role(gpui_kit::accesskit::Role::Tab)
        .aria_label(name.clone())
        .relative()
        .flex_none()
        .w(ph_ui::rems_from_px(width_px(labels) - 6.))
        .when(labels, |this| this.py_1().gap_0p5())
        .items_center()
        .cursor_pointer()
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
        })
        .on_click(move |_, window, cx| {
            _ = open.update(cx, |workspace, cx| workspace.open_project(id, window, cx));
        })
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .size(ph_ui::rems_from_px(30.))
                .rounded(theme.radius)
                .bg(color.opacity(if selected { 0.36 } else { 0.2 }))
                .text_color(if theme.is_dark() {
                    theme.foreground
                } else {
                    color
                })
                .text_sm()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                // The one in front wears its colour as a ring.
                .border_2()
                .border_color(if selected {
                    color
                } else {
                    gpui_kit::transparent_black()
                })
                .hover(|style| style.bg(color.opacity(0.36)))
                .child(letter),
        )
        .when(labels, |this| {
            this.child(
                div()
                    .max_w_full()
                    .truncate()
                    .text_size(ph_ui::rems_from_px(10.))
                    .text_color(if selected {
                        theme.foreground
                    } else {
                        theme.muted_foreground
                    })
                    .child(name),
            )
        })
        .children(status_dot(&status, cx))
        // Findable by name in UI tests and automation.
        .test_support()
        .context_menu(move |menu, window, cx| {
            crate::bridge::workspace_menu_items(menu, menu_owner.clone(), id, window, cx)
        })
        .into_any_element()
}

/// The dot at a place's corner for what happens there: amber when an agent
/// waits for an answer, red for failing tests, green for news not seen.
fn status_dot(status: &crate::status::WorkspaceStatus, cx: &App) -> Option<AnyElement> {
    let theme = cx.theme();
    let color = if status.waiting > 0 {
        theme.warning
    } else if status.failed {
        theme.danger
    } else if status.unseen > 0 {
        theme.success
    } else if status.working > 0 {
        theme.info
    } else {
        return None;
    };
    Some(dot_badge(color))
}

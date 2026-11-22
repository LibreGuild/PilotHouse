//! The column of icons at the window's left edge: where the window can go.
//! The places that are always there, in the order a day goes: *Workspaces*
//! and *Chats* (for the chats that need no folder), where the work is done;
//! the Bridge, what goes on across all of it; Reviews and *Artifacts*, what
//! is around the work; then an organisation's own views. At the foot,
//! *Utilities* and Settings. An entry shows its name under
//! it, or in a tooltip when the names are switched off (Settings →
//! Appearance).
//!
//! *Utilities* is not a place: it opens its dialog over whatever is in
//! front, for a quick decode or format in the middle of other work, and
//! closing it is back where one was. It is in the column because inside
//! the title bar's menu few found it.
//!
//! The entry in front is marked on its icon: a rounded box behind the icon
//! alone, and its name in full strength under it. The box used to hold the
//! name too, and a long one (*Workspaces*) touched its edges.
//!
//! The user's workspaces are one entry, however many there are: they used
//! to be a badge each, which filled the column, hid the button to add one
//! past its end, and would have been pushed down by every place added
//! here. *Workspaces* brings the one used last in front, and its name in
//! the title bar opens the list of them (`workspace_switcher`).
//!
//! *Chats* is a workspace like the others underneath, but it is not one of
//! the user's projects: it cannot be removed and has no folder. So it is
//! not in their list; it has a place and an icon of its own, like the
//! Bridge.

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, menu::ContextMenuExt as _, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div,
};
use ph_ui::Icons;

use crate::actions::{OpenSettings, ShowUtilities};
use crate::workspace::{View, Workspace};

/// A column's width in pixels at the default interface size: icons alone,
/// or wider with their names.
pub(crate) fn width_px(labels: bool) -> f32 {
    // With names: as wide as the longest of them, "Workspaces".
    if labels { 68. } else { 42. }
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
        let group = SharedString::from(format!("rail-{}", self.id));
        let selected = self.selected;
        // The box behind the icon: wide and low over a name, a square of
        // the column's width without one.
        let (box_width, box_height) = if labels { (40., 28.) } else { (36., 32.) };
        let icon = div()
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w(ph_ui::rems_from_px(box_width))
            .h(ph_ui::rems_from_px(box_height))
            .rounded(theme.radius)
            .when(selected, |this| this.bg(theme.secondary_hover))
            .when(!selected, |this| {
                this.group_hover(group.clone(), |style| style.bg(theme.secondary_hover))
            })
            .child(self.icon.size(ph_ui::rems_from_px(18.)))
            // At the icon's corner, wherever the name ends.
            .children(self.badge);
        v_flex()
            .id(self.id)
            .group(group.clone())
            .role(gpui_kit::accesskit::Role::Tab)
            .aria_label(self.label.clone())
            .flex_none()
            .w(ph_ui::rems_from_px(width_px(labels) - 4.))
            .when(labels, |this| this.py_1().gap_0p5())
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_color(if selected {
                theme.foreground
            } else {
                theme.muted_foreground
            })
            .when(!selected, |this| {
                this.hover(|style| style.text_color(theme.foreground))
            })
            // With names shown the tooltip would only repeat them.
            .when(
                !labels || self.hint.is_some() || self.about.is_some(),
                |this| {
                    this.tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(ph_ui::tip(tip.clone()))
                            .build(window, cx)
                    })
                },
            )
            .on_click(move |_, window, cx| on_click(window, cx))
            .child(icon)
            .when(labels, |this| {
                this.child(
                    div()
                        .max_w_full()
                        .truncate()
                        .text_size(ph_ui::rems_from_px(10.))
                        .when(selected, |this| {
                            this.font_weight(gpui_kit::FontWeight::MEDIUM)
                        })
                        .child(self.label),
                )
            })
    }
}

/// A count at the corner of an entry's icon (99+ beyond two digits).
pub(crate) fn count_badge(count: usize, color: Option<Hsla>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .absolute()
        .top_0()
        .right_0()
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

/// A dot at the corner of an entry's icon: something there wants a look.
pub(crate) fn dot_badge(color: Hsla) -> AnyElement {
    div()
        .absolute()
        .top(ph_ui::rems_from_px(3.))
        .right(ph_ui::rems_from_px(6.))
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

    // The places, in the order a day goes: where the work is done (a
    // workspace, or a chat that needs no folder), then what goes on across
    // all of it, then what is around the work (reviews of other people's
    // changes, what agents made). The workspaces lead: they are what the
    // app is for, and where someone new begins.
    let mut places = Vec::new();
    // The workspace in front, when a workspace's screen is.
    let in_front = (view == View::Workspaces)
        .then(|| workspace.active_project_id())
        .flatten();
    // Workspaces: the user's own, as one place. What goes on in those not
    // in front shows as its dot, since they have no badge to wear it.
    let chats = workspace.chats_place().map(|chats| chats.id);
    let workspace_in_front = in_front.filter(|id| Some(*id) != chats);
    let elsewhere = workspace
        .listed()
        .filter(|project| Some(project.id) != workspace_in_front)
        .fold(
            crate::status::WorkspaceStatus::default(),
            |mut all, project| {
                let status = workspace.status(project.id);
                all.working += status.working;
                all.waiting += status.waiting;
                all.unseen += status.unseen;
                all.failed |= status.failed;
                all
            },
        );
    let any = workspace.listed().next().is_some();
    {
        let (open, menu_owner) = (this.clone(), this.clone());
        let entry = Item {
            id: "view-workspaces".into(),
            label: "Workspaces".into(),
            icon: Icon::new(Icons::Folders),
            hint: None,
            about: Some(if workspace_in_front.is_some() {
                "Switch to another workspace"
            } else if any {
                "The workspace used last"
            } else {
                "What a workspace is, and how to make one"
            }),
            selected: workspace_in_front.is_some() || view == View::NoWorkspaces,
            badge: status_dot(&elsewhere, cx),
        }
        .element(
            labels,
            move |window, cx| {
                _ = open.update(cx, |workspace, cx| workspace.show_workspaces(window, cx));
            },
            cx,
        )
        .test_support();
        // The menu of the one in front, as on its name in the title bar.
        places.push(match workspace_in_front {
            Some(id) => entry
                .context_menu(move |menu, window, cx| {
                    crate::bridge::workspace_menu_items(menu, menu_owner.clone(), id, window, cx)
                })
                .into_any_element(),
            None => entry.into_any_element(),
        });
    }
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
    // The Bridge: what goes on in all of them, and what waits for the user.
    places.push(
        Item {
            id: "view-bridge".into(),
            label: "Bridge".into(),
            icon: Icon::new(Icons::ShipWheel),
            hint: hint("⌘⇧E", "Ctrl+Shift+E"),
            about: Some("Every task in every workspace, by what it needs from you"),
            selected: view == View::Bridge,
            badge: (needs_you > 0 && view != View::Bridge)
                .then(|| count_badge(needs_you, Some(cx.theme().warning), cx)),
        }
        .render(labels, show(View::Bridge), cx),
    );
    if ph_brand::enabled(ph_brand::Feature::GitHub) {
        places.push(
            Item {
                id: "view-github".into(),
                label: "Reviews".into(),
                icon: Icon::new(Icons::GitPullRequest),
                hint: hint("⌘⇧G", "Ctrl+Shift+G"),
                about: Some("Pull requests that wait for your review, and agents' reviews of them"),
                selected: view == View::GitHub,
                badge: (waiting > 0).then(|| count_badge(waiting, Some(cx.theme().warning), cx)),
            }
            .render(labels, show(View::GitHub), cx),
        );
    }
    // Artifacts: what agents made for the user, whichever chat it was in.
    if ph_brand::enabled(ph_brand::Feature::Artifacts) {
        places.push(
            Item {
                id: "view-artifacts".into(),
                label: "Artifacts".into(),
                icon: Icon::new(Icons::Shapes),
                hint: None,
                about: Some("Pages agents made for you, kept on this computer"),
                selected: view == View::Artifacts,
                // One of them is open to the network.
                badge: ph_artifacts::Artifacts::global(cx)
                    .is_some_and(|artifacts| artifacts.read(cx).any_shared())
                    .then(|| dot_badge(cx.theme().success)),
            }
            .render(labels, show(View::Artifacts), cx),
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

    column("view-rail", labels, places)
        .child(div().flex_1())
        .when(ph_brand::enabled(ph_brand::Feature::Utilities), |this| {
            this.child(
                Item {
                    id: "open-utilities".into(),
                    label: "Utilities".into(),
                    icon: Icon::new(Icons::Wrench),
                    hint: None,
                    about: Some("JSON, JWT, Base64, times, regex and more"),
                    selected: false,
                    badge: None,
                }
                .render(
                    labels,
                    |window, cx| window.dispatch_action(Box::new(ShowUtilities), cx),
                    cx,
                ),
            )
        })
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

/// The dot at a place's corner for what happens there: amber when an agent
/// waits for an answer, red for failing tests, green for news not seen.
fn status_dot(status: &crate::status::WorkspaceStatus, cx: &App) -> Option<AnyElement> {
    crate::workspace_switcher::status_color(status, cx).map(dot_badge)
}

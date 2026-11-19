//! The icon columns at the window's two edges: on the left what the window
//! shows (workspaces, chats, GitHub, an organisation's own views), on the
//! right the tools of the workspace in front. Both are drawn here, so they
//! look and behave the same: an icon with its name in a tooltip, or under
//! it when the user asked for names (Settings → Appearance).

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{ActiveTheme as _, Icon, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div,
};

use crate::workspace::{Section, View, Workspace};

/// A column's width in pixels at the default interface size: icons alone,
/// or wider with their names.
pub(crate) fn width_px(labels: bool) -> f32 {
    if labels { 64. } else { 42. }
}

/// Whether the user wants names under the icons.
pub(crate) fn labels(cx: &App) -> bool {
    cx.try_global::<ph_theme::Preferences>()
        .is_some_and(|preferences| preferences.tool_labels)
}

/// One entry of a column.
pub(crate) struct Item {
    pub id: SharedString,
    pub label: SharedString,
    pub icon: Icon,
    /// Its shortcut, said in the tooltip.
    pub hint: Option<String>,
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
        let theme = cx.theme();
        let tip = match &self.hint {
            Some(hint) => format!("{} ({hint})", self.label),
            None => self.label.to_string(),
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
            .when(!labels || self.hint.is_some(), |this| {
                this.tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                })
            })
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
            // Findable by label in UI tests and automation.
            .test_support()
            .into_any_element()
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

/// The left column: what the window shows. Pressing the entry already in
/// front hides or shows its list, as in VS Code.
pub(crate) fn render_views(
    workspace: &Workspace,
    cx: &mut gpui_kit::Context<Workspace>,
) -> impl IntoElement {
    let labels = labels(cx);
    let this = cx.entity().downgrade();
    let (view, section) = (workspace.view(), workspace.section());
    let mac = cfg!(target_os = "macos");
    let hint = |mac_keys: &str, other: &str| Some(if mac { mac_keys } else { other }.to_string());

    // What waits in the lists that are not in front: amber when an agent
    // waits for an answer, green when there is only news.
    let news = |ids: Vec<ph_store::projects::ProjectId>, cx: &App| {
        let statuses: Vec<_> = ids.into_iter().map(|id| workspace.status(id)).collect();
        if statuses.iter().any(|status| status.waiting > 0) {
            Some(dot_badge(cx.theme().warning))
        } else if statuses.iter().any(|status| status.unseen > 0) {
            Some(dot_badge(cx.theme().success))
        } else {
            None
        }
    };
    let in_front = (view == View::Workspaces)
        .then(|| workspace.active_project_id())
        .flatten();
    let workspace_ids = workspace
        .projects()
        .iter()
        .map(|project| project.id)
        .filter(|id| Some(*id) != in_front)
        .collect();
    let chat_ids = workspace
        .chats_space()
        .map(|chats| chats.id)
        .filter(|id| Some(*id) != in_front)
        .into_iter()
        .collect();
    let waiting = workspace
        .github_view()
        .map_or(0, |github| github.read(cx).waiting_for_review());

    let mut items = Vec::new();
    let show = |target: Target| {
        let this = this.clone();
        move |window: &mut Window, cx: &mut App| {
            _ = this.update(cx, |workspace, cx| workspace.show(target, window, cx));
        }
    };
    items.push(
        Item {
            id: "view-workspaces".into(),
            label: "Workspaces".into(),
            icon: Icon::new(ph_ui::Icons::LayoutGrid),
            hint: hint("⌘⇧E", "Ctrl+Shift+E"),
            selected: view == View::Workspaces && section == Section::Workspaces,
            badge: news(workspace_ids, cx),
        }
        .render(labels, show(Target::Section(Section::Workspaces)), cx),
    );
    if workspace.chats_space().is_some() {
        items.push(
            Item {
                id: "view-chats".into(),
                label: "Chats".into(),
                icon: Icon::new(ph_ui::Icons::MessagesSquare),
                hint: hint("⌘⇧D", "Ctrl+Shift+D"),
                selected: view == View::Workspaces && section == Section::Chats,
                badge: news(chat_ids, cx),
            }
            .render(labels, show(Target::Section(Section::Chats)), cx),
        );
    }
    if ph_brand::enabled(ph_brand::Feature::GitHub) {
        items.push(
            Item {
                id: "view-github".into(),
                label: "GitHub".into(),
                icon: Icon::new(ph_ui::Icons::Github),
                hint: hint("⌘⇧G", "Ctrl+Shift+G"),
                selected: view == View::GitHub,
                badge: (waiting > 0).then(|| count_badge(waiting, Some(cx.theme().warning), cx)),
            }
            .render(labels, show(Target::View(View::GitHub)), cx),
        );
    }
    for custom in ph_custom::views() {
        items.push(
            Item {
                id: custom.id.into(),
                label: custom.label.into(),
                icon: (custom.icon)(),
                hint: None,
                selected: view == View::Custom(custom.id),
                badge: None,
            }
            .render(labels, show(Target::View(View::Custom(custom.id))), cx),
        );
    }
    column("view-rail", labels, items)
}

/// What an entry of the left column shows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Target {
    /// Workspaces or Chats: a list beside the screen in front.
    Section(Section),
    /// GitHub, or one of the organisation's own.
    View(View),
}

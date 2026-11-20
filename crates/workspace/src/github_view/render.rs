//! How the GitHub view is drawn: its sidebar (inbox lists, repositories),
//! the page in the middle and the repository's chat. An inbox list's page
//! is the queue's (`queue`).

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::Input,
    resizable::{h_resizable, resizable_panel},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use ph_github::search::Inbox;
use ph_theme::layout;
use ph_ui::{EmptyState, Icons, MenuButton as _};

use super::{GithubView, Page};

impl GithubView {
    fn render_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let inbox = Inbox::ALL.into_iter().map(|kind| {
            let count = self.inbox.get(&kind).map(|list| list.items.len());
            let selected = self.page == Page::Inbox(kind);
            row(
                SharedString::from(format!("github-inbox-{}", inbox_key(kind))),
                inbox_icon(kind),
                kind.label().to_string(),
                count
                    .filter(|count| *count > 0)
                    .map(|count| count.to_string()),
                selected,
                cx,
            )
            .on_click(cx.listener(move |this, _, _, cx| this.show_inbox(kind, cx)))
            .test_support()
            .into_any_element()
        });
        let repos = self.repositories();
        let repo_rows = repos.into_iter().map(|repo| {
            let selected = self.page == Page::Repo(repo.id.clone());
            let id = repo.id.clone();
            let tooltip = if repo.workspaces.is_empty() {
                format!("{} on {}", repo.id.full_name(), repo.id.host)
            } else {
                format!(
                    "{} on {}, in {}",
                    repo.id.full_name(),
                    repo.id.host,
                    repo.workspaces.join(", ")
                )
            };
            let removable = repo.added.then(|| {
                let (view, id) = (cx.entity().downgrade(), repo.id.clone());
                Button::new(SharedString::from(format!(
                    "github-repo-menu-{}",
                    repo.id.key()
                )))
                .ghost()
                .xsmall()
                .icon(IconName::Ellipsis)
                .accessibility_label("Repository menu")
                .menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                    let (view, id) = (view.clone(), id.clone());
                    menu.item(
                        gpui_kit::component::menu::PopupMenuItem::new("Remove from the list")
                            .on_click(move |_, _, cx| {
                                _ = view.update(cx, |view, cx| view.remove_repository(&id, cx));
                            }),
                    )
                })
            });
            // The name, its owner muted after it (a long one is cut first).
            row(
                SharedString::from(format!("github-repo-{}", repo.id.key())),
                Icons::GitPullRequest,
                repo.id.name.clone(),
                Some(repo.id.owner.clone()),
                selected,
                cx,
            )
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(ph_ui::tip(tooltip.clone()))
                    .build(window, cx)
            })
            .on_click(
                cx.listener(move |this, _, window, cx| this.show_repo(id.clone(), window, cx)),
            )
            .children(removable)
            .test_support()
            .into_any_element()
        });
        let adding = self.adding.as_ref().map(|(input, _)| {
            div().px_1().py_1().child(
                Input::new(input)
                    .small()
                    .prefix(Icon::new(IconName::Search).xsmall()),
            )
        });
        v_flex()
            .id("github-sidebar")
            .size_full()
            .px_1()
            .pb_2()
            .overflow_y_scroll()
            .child(
                section("Inbox", cx).child(
                    Button::new("github-refresh")
                        .ghost()
                        .xsmall()
                        .icon(IconName::RefreshCw)
                        // The page's header shows that it is being read.
                        .disabled(self.loading)
                        .tooltip("Read again")
                        .accessibility_label("Refresh")
                        .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                ),
            )
            .children(inbox)
            .child(
                section("Repositories", cx).child(
                    Button::new("github-add-repo")
                        .ghost()
                        .xsmall()
                        .icon(IconName::Plus)
                        .tooltip("Add a repository")
                        .accessibility_label("Add a repository")
                        .on_click(cx.listener(|this, _, window, cx| this.start_adding(window, cx))),
                ),
            )
            .children(adding)
            .children(repo_rows)
            .when(
                self.repositories().is_empty() && self.adding.is_none(),
                |this| {
                    this.child(
                        div()
                            .px_2()
                            .py_1()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("Your workspaces' repositories show here; add others with +."),
                    )
                },
            )
            // What the clones take on disk, and where to clean them up.
            .when_some(self.clones_size.filter(|size| *size > 0), |this, size| {
                this.child(div().flex_1().min_h_4()).child(
                    h_flex()
                        .mt_2()
                        .px_2()
                        .gap_1p5()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(Icon::new(Icons::HardDrive).xsmall())
                        .child(
                            div().flex_1().child(format!(
                                "Clones use {}",
                                ph_files::large::format_size(size)
                            )),
                        )
                        .child(
                            Button::new("github-storage")
                                .ghost()
                                .xsmall()
                                .label("Manage")
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(
                                        Box::new(crate::actions::OpenStorageSettings),
                                        cx,
                                    )
                                }),
                        ),
                )
            })
            .test_support()
            .into_any_element()
    }

    fn render_page(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.accounts.is_empty() {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(
                    EmptyState::new("Connect GitHub")
                        .icon(Icons::GitPullRequest)
                        .description(
                            "Add a token for github.com or your GitHub Enterprise server to see the pull requests waiting for you and review them with an agent.",
                        )
                        .action(
                            Button::new("github-open-settings")
                                .primary()
                                .label("Open Settings")
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(
                                        Box::new(crate::actions::OpenGitHubSettings),
                                        cx,
                                    )
                                }),
                        ),
                )
                .into_any_element();
        }
        match &self.page {
            Page::Inbox(kind) => self.render_inbox(*kind, cx),
            Page::Repo(id) => match self.panel_of(id) {
                Some(panel) => panel.clone().into_any_element(),
                None => div().into_any_element(),
            },
        }
    }
}

impl Render for GithubView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sidebar = if self.sidebar_shown {
            crate::sidebar::width(cx)
        } else {
            px(0.)
        };
        // What the page and the chat share: the window less the column of
        // icons, the list and the gaps.
        let rail = ph_ui::scaled_px(crate::rail::width_px(crate::rail::labels(cx)), window);
        let available = (window.viewport_size().width - rail - sidebar - px(16.)).max(px(0.));
        let chat_width = (available * 0.45).max(layout::CHAT_MIN);
        let page = crate::frame::island(window, cx).child(self.render_page(cx));
        let gap = crate::frame::gap();
        // The chat is the pull request's: it shows while one is open.
        let body = match self.current_chat().cloned() {
            Some(chat) => h_resizable("github-split")
                .child(
                    resizable_panel()
                        .size_range(layout::CONTEXT_PANEL_MIN..gpui_kit::Pixels::MAX)
                        .child(div().size_full().pr(gap / 2.).child(page)),
                )
                .child(
                    resizable_panel()
                        .size(chat_width)
                        .size_range(layout::CHAT_MIN..gpui_kit::Pixels::MAX)
                        .child(
                            div()
                                .size_full()
                                .pl(gap / 2.)
                                .child(crate::frame::island(window, cx).child(chat)),
                        ),
                )
                .into_any_element(),
            None => page.into_any_element(),
        };
        h_flex()
            .id("github-view")
            .size_full()
            .items_stretch()
            // The workspace window resizes it as it does its own sidebar.
            .when(self.sidebar_shown, |this| {
                let list = self.render_sidebar(cx);
                this.child(crate::frame::list_island(list, window, cx))
            })
            .child(div().flex_1().min_w_0().h_full().pr_2().pb(gap).child(body))
    }
}

/// An inbox pull request's triage, as small chips.
pub(super) fn triage_chips(triage: &ph_review::triage::Triage, cx: &App) -> Vec<AnyElement> {
    use ph_review::triage::Ci;
    let chip = |text: String, color: gpui_kit::Hsla| {
        div()
            .px_1p5()
            .rounded_full()
            .bg(color.opacity(0.14))
            .text_color(color)
            .child(text)
            .into_any_element()
    };
    let mut chips = Vec::new();
    if triage.behind {
        chips.push(chip("pushed since your review".into(), cx.theme().info));
    } else if triage.reviewed {
        chips.push(chip("reviewed".into(), cx.theme().muted_foreground));
    }
    chips.push(chip(
        format!("{} · {} lines", triage.size.label(), triage.lines),
        cx.theme().muted_foreground,
    ));
    match triage.ci {
        Ci::Failing => chips.push(chip("CI failing".into(), cx.theme().danger)),
        Ci::Running => chips.push(chip("CI running".into(), cx.theme().warning)),
        Ci::Passing => chips.push(chip("CI green".into(), cx.theme().success)),
        Ci::None => {}
    }
    for risk in &triage.risks {
        chips.push(chip((*risk).to_string(), cx.theme().warning));
    }
    chips
}

/// A sidebar section's title, with `actions` added by the caller.
fn section(title: &'static str, cx: &App) -> gpui_kit::Div {
    h_flex()
        .flex_none()
        .h_8()
        .mt_1()
        .px_1()
        .gap_0p5()
        .text_xs()
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
        .child(div().flex_1().child(title.to_uppercase()))
}

/// A row of the sidebar: icon, name, and muted after it a count or an
/// owner.
fn row(
    id: SharedString,
    icon: Icons,
    label: String,
    count: Option<String>,
    selected: bool,
    cx: &App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    h_flex()
        .id(id)
        .h_7()
        .px_2()
        .gap_2()
        .rounded(cx.theme().radius)
        .cursor_pointer()
        .text_sm()
        .when(selected, |this| {
            this.bg(cx.theme().sidebar_accent)
                .text_color(cx.theme().sidebar_accent_foreground)
        })
        .when(!selected, |this| {
            this.hover(|style| style.bg(cx.theme().sidebar_accent.opacity(0.6)))
        })
        .child(
            // As large as the row's text, like the rows of the other lists.
            Icon::new(icon)
                .small()
                .text_color(cx.theme().muted_foreground),
        )
        .child(div().flex_1().min_w_0().truncate().child(label))
        .when_some(count, |this, count| {
            this.child(
                div()
                    .flex_shrink_0()
                    .max_w(ph_ui::rems_from_px(96.))
                    .truncate()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(count),
            )
        })
}

pub(super) fn inbox_icon(kind: Inbox) -> Icons {
    match kind {
        Inbox::ReviewRequested => Icons::Eye,
        Inbox::Mine => Icons::GitPullRequest,
        Inbox::Assigned => Icons::User,
    }
}

fn inbox_key(kind: Inbox) -> &'static str {
    match kind {
        Inbox::ReviewRequested => "review",
        Inbox::Mine => "mine",
        Inbox::Assigned => "assigned",
    }
}

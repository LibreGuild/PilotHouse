//! How the GitHub view is drawn: its sidebar (inbox lists, repositories),
//! the page in the middle and the repository's chat.

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::Input,
    resizable::{h_resizable, resizable_panel},
    spinner::Spinner,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use ph_github::search::Inbox;
use ph_pulls::clones::RepoId;
use ph_theme::layout;
use ph_ui::{EmptyState, Icons, MenuButton as _, Surface};

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
            row(
                SharedString::from(format!("github-repo-{}", repo.id.key())),
                Icons::GitPullRequest,
                repo.id.full_name(),
                None,
                selected,
                cx,
            )
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
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
            .flex_none()
            .w(layout::SIDEBAR_WIDTH)
            .h_full()
            .pl_2()
            .pb_2()
            .pr_1()
            .overflow_y_scroll()
            .child(
                section("Inbox", cx).child(
                    Button::new("github-refresh")
                        .ghost()
                        .xsmall()
                        .icon(IconName::RefreshCw)
                        .loading(self.loading)
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
            .test_support()
            .into_any_element()
    }

    /// One inbox list: each pull request with its repository.
    fn render_inbox(&self, kind: Inbox, cx: &mut Context<Self>) -> AnyElement {
        let list = self.inbox.get(&kind);
        let items = list.map(|list| list.items.clone()).unwrap_or_default();
        let error = list.and_then(|list| list.error.clone());
        let rows =
            items.into_iter().enumerate().map(|(ix, (host, pull))| {
                let repo = pull
                    .repo()
                    .and_then(|(owner, name)| RepoId::parse(&format!("{host}/{owner}/{name}")));
                let number = pull.number;
                let place = repo
                    .as_ref()
                    .map(|repo| repo.full_name())
                    .unwrap_or_default();
                v_flex()
                    .id(("github-found", ix))
                    .flex_none()
                    .gap_0p5()
                    .px_2p5()
                    .py_2()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(cx.theme().border)
                    .cursor_pointer()
                    .hover(|style| style.bg(cx.theme().list_hover))
                    .when_some(repo, |this, repo| {
                        this.on_click(cx.listener(move |this, _, window, cx| {
                            this.show_pull(repo.clone(), number, window, cx)
                        }))
                    })
                    .child(
                        h_flex()
                            .gap_2()
                            .child(Icon::new(Icons::GitPullRequest).small().text_color(
                                if pull.draft {
                                    cx.theme().muted_foreground
                                } else {
                                    cx.theme().success
                                },
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_sm()
                                    .truncate()
                                    .child(pull.title.clone()),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "{place} #{number} · {} · {}",
                                pull.user.login,
                                ph_pulls::short_date(&pull.updated_at)
                            )),
                    )
                    .into_any_element()
            });
        let empty = list.is_none_or(|list| list.items.is_empty()) && !self.loading;
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .h_10()
                    .px_3()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        Icon::new(inbox_icon(kind))
                            .small()
                            .text_color(cx.theme().info),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(kind.label()),
                    )
                    .child(div().flex_1())
                    .when(self.loading, |this| this.child(Spinner::new().xsmall())),
            )
            .when_some(error, |this, error| {
                this.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(error),
                )
            })
            .child(
                v_flex()
                    .id("github-inbox-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_2()
                    .gap_1()
                    .when(empty, |this| {
                        this.child(
                            div()
                                .p_3()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(match kind {
                                    Inbox::ReviewRequested => "Nothing waits for your review.",
                                    Inbox::Mine => "You have no open pull requests.",
                                    Inbox::Assigned => "No open pull requests are assigned to you.",
                                }),
                        )
                    })
                    .children(rows),
            )
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
                                        Box::new(crate::actions::OpenSettings),
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

    /// The repository's chat, or what will be there.
    fn render_chat(&self, _: &mut Context<Self>) -> AnyElement {
        if let Some(chat) = self.current_chat() {
            return chat.clone().into_any_element();
        }
        let (title, text) = match &self.page {
            Page::Repo(id) => (
                format!("Chats about {}", id.full_name()),
                "Open a pull request and pick Review with agent: PilotHouse clones the repository for the view, checks the PR out beside it, and the review chat runs here. Ask the agent about the change afterwards in the same chat.",
            ),
            Page::Inbox(_) => (
                "Pick a pull request".to_string(),
                "The chat of its repository shows here: agent reviews, and your questions about the change.",
            ),
        };
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .p_6()
            .child(
                EmptyState::new(title)
                    .icon(Icons::Sparkles)
                    .description(text),
            )
            .into_any_element()
    }
}

impl Render for GithubView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sidebar = if self.sidebar_shown {
            layout::SIDEBAR_WIDTH
        } else {
            px(0.)
        };
        let available = window.viewport_size().width - sidebar - px(16.);
        let chat_width = (available * 0.4).max(layout::CHAT_MIN);
        let page = Surface::new().child(self.render_page(cx));
        let chat = Surface::new().child(self.render_chat(cx));
        h_flex()
            .id("github-view")
            .size_full()
            .items_stretch()
            .when(self.sidebar_shown, |this| {
                this.child(self.render_sidebar(cx))
            })
            .child(
                div().flex_1().min_w_0().h_full().pr_2().pb_2().child(
                    h_resizable("github-split")
                        .child(
                            resizable_panel()
                                .size_range(layout::CONTEXT_PANEL_MIN..gpui_kit::Pixels::MAX)
                                .child(div().size_full().pr_1().child(page)),
                        )
                        .child(
                            resizable_panel()
                                .size(chat_width)
                                .size_range(layout::CHAT_MIN..gpui_kit::Pixels::MAX)
                                .child(div().size_full().pl_1().child(chat)),
                        ),
                ),
            )
    }
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

/// A row of the sidebar: icon, name, an optional count.
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
            Icon::new(icon)
                .xsmall()
                .text_color(cx.theme().muted_foreground),
        )
        .child(div().flex_1().min_w_0().truncate().child(label))
        .when_some(count, |this, count| {
            this.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(count),
            )
        })
}

fn inbox_icon(kind: Inbox) -> Icons {
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

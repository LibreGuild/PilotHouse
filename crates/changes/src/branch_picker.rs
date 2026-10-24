//! The branch picker, opened from the branch in the status bar: switch,
//! create and delete branches, and fetch, pull or push.

use std::time::{Duration, Instant};

use gpui_kit::Focusable as _;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    spinner::Spinner,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, Subscription, Task,
    Window, div, px,
};
use ph_git::branches::{self, Branch};
use ph_ui::Icons;

use crate::repo::Repo;

/// Branches older than this are re-read when the picker is shown again.
const STALE: Duration = Duration::from_secs(10);

pub struct BranchPicker {
    repo: Entity<Repo>,
    search: Entity<InputState>,
    branches: Vec<Branch>,
    loaded_at: Option<Instant>,
    _load: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl BranchPicker {
    pub fn new(repo: Entity<Repo>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Find or create a branch…"));
        let subscriptions = vec![
            cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
            // A switch or fetch changes the list.
            cx.observe(&repo, |this, _, cx| {
                this.loaded_at = None;
                cx.notify();
            }),
        ];
        Self {
            repo,
            search,
            branches: Vec::new(),
            loaded_at: None,
            _load: None,
            _subscriptions: subscriptions,
        }
    }

    /// Called as the popover renders: on opening (search not focused yet),
    /// starts with an empty, focused search and a fresh branch list.
    pub fn focus_when_opened(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.read(cx).focus_handle(cx).is_focused(window) {
            return;
        }
        let search = self.search.clone();
        window.defer(cx, move |window, cx| {
            search.update(cx, |search, cx| {
                search.set_value("", window, cx);
                search.focus(window, cx);
            })
        });
        self.loaded_at = None;
    }

    fn load_if_stale(&mut self, cx: &mut Context<Self>) {
        if self._load.is_some() || self.loaded_at.is_some_and(|at| at.elapsed() < STALE) {
            return;
        }
        let root = self.repo.read(cx).root().to_path_buf();
        let read = cx.background_spawn(async move { branches::branches(&root) });
        self._load = Some(cx.spawn(async move |this, cx| {
            let list = read.await;
            _ = this.update(cx, |this, cx| {
                this._load = None;
                this.loaded_at = Some(Instant::now());
                this.branches = list.unwrap_or_default();
                cx.notify();
            });
        }));
    }

    fn switch(&mut self, branch: &Branch, cx: &mut Context<Self>) {
        let (name, remote) = (branch.name.clone(), branch.remote);
        self.repo.update(cx, |repo, cx| {
            repo.run(
                format!("Switching to {name}"),
                move |root| {
                    branches::switch(root, &name, remote)?;
                    Ok(format!("Switched to {name}"))
                },
                cx,
            )
        });
    }

    fn create(&mut self, name: String, cx: &mut Context<Self>) {
        self.repo.update(cx, |repo, cx| {
            repo.run(
                format!("Creating {name}"),
                move |root| {
                    branches::create(root, &name, None)?;
                    Ok(format!("Created and switched to {name}"))
                },
                cx,
            )
        });
    }

    fn delete(&mut self, name: String, cx: &mut Context<Self>) {
        self.repo.update(cx, |repo, cx| {
            repo.run(
                format!("Deleting {name}"),
                move |root| {
                    branches::delete(root, &name, false)?;
                    Ok(format!("Deleted {name}"))
                },
                cx,
            )
        });
    }

    fn sync(&mut self, action: Sync, cx: &mut Context<Self>) {
        let has_upstream = self
            .repo
            .read(cx)
            .status()
            .is_some_and(|status| status.upstream.is_some());
        self.repo.update(cx, |repo, cx| match action {
            Sync::Fetch => repo.run(
                "Fetching",
                |root| branches::fetch(root).map(|_| "Fetched from all remotes".into()),
                cx,
            ),
            Sync::Pull => repo.run(
                "Pulling",
                |root| branches::pull(root).map(|_| "Up to date with the remote".into()),
                cx,
            ),
            Sync::Push => repo.run(
                "Pushing",
                move |root| branches::push(root, has_upstream).map(|_| "Pushed".into()),
                cx,
            ),
        });
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let repo = self.repo.read(cx);
        let status = repo.status().cloned().unwrap_or_default();
        let busy = repo.busy().cloned();
        let branch = status
            .branch
            .clone()
            .unwrap_or_else(|| format!("Detached at {}", status.head.clone().unwrap_or_default()));
        let tracking = match (&status.upstream, status.ahead, status.behind) {
            (None, _, _) => "No upstream yet: Push publishes it".to_string(),
            (Some(up), 0, 0) => format!("Up to date with {up}"),
            (Some(up), ahead, behind) => format!("{ahead} ahead, {behind} behind {up}"),
        };
        let button = |id: &'static str,
                      icon: IconName,
                      label: &'static str,
                      action: Sync,
                      cx: &mut Context<Self>| {
            Button::new(id)
                .small()
                .outline()
                .icon(icon)
                .label(label)
                .disabled(busy.is_some())
                .on_click(cx.listener(move |this, _, _, cx| this.sync(action, cx)))
        };
        v_flex()
            .gap_2()
            .p_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Icon::new(Icons::GitBranch)
                            .small()
                            .text_color(cx.theme().info),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(branch),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(tracking),
            )
            .child(
                h_flex()
                    .gap_1p5()
                    .child(button(
                        "git-fetch",
                        IconName::RefreshCw,
                        "Fetch",
                        Sync::Fetch,
                        cx,
                    ))
                    .child(
                        button("git-pull", IconName::ArrowDown, "Pull", Sync::Pull, cx)
                            .disabled(busy.is_some() || status.upstream.is_none()),
                    )
                    .child(button(
                        "git-push",
                        IconName::ArrowUp,
                        "Push",
                        Sync::Push,
                        cx,
                    ))
                    .when_some(busy, |this, busy| {
                        this.child(
                            h_flex()
                                .gap_1()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(Spinner::new().xsmall())
                                .child(busy),
                        )
                    }),
            )
    }

    fn render_row(&self, ix: usize, branch: &Branch, cx: &mut Context<Self>) -> AnyElement {
        let row_branch = branch.clone();
        let name = branch.name.clone();
        let deletable = !branch.current && !branch.remote;
        h_flex()
            .id(("branch", ix))
            .group("branch-row")
            .gap_2()
            .px_2()
            .py_1()
            .rounded(cx.theme().radius)
            .cursor_pointer()
            .hover(|style| style.bg(cx.theme().list_hover))
            .when(branch.current, |this| this.bg(cx.theme().list_active))
            .on_click(cx.listener(move |this, _, _, cx| {
                if !row_branch.current {
                    this.switch(&row_branch, cx);
                }
            }))
            .child(
                Icon::new(if branch.remote {
                    Icons::Globe
                } else {
                    Icons::GitBranch
                })
                .xsmall()
                .text_color(cx.theme().muted_foreground),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_sm().truncate().child(branch.name.clone()))
                    .child(
                        div()
                            .text_xs()
                            .truncate()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{} · {}", ago(branch.updated), branch.subject)),
                    ),
            )
            .when(branch.current, |this| {
                this.child(
                    Icon::new(IconName::Check)
                        .small()
                        .text_color(cx.theme().info),
                )
            })
            .when(deletable, |this| {
                this.child(
                    div()
                        .invisible()
                        .group_hover("branch-row", |style| style.visible())
                        .child(
                            Button::new(("delete-branch", ix))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Delete)
                                .tooltip("Delete branch (only if merged)")
                                .accessibility_label("Delete branch")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.delete(name.clone(), cx)
                                })),
                        ),
                )
            })
            .into_any_element()
    }
}

#[derive(Clone, Copy)]
enum Sync {
    Fetch,
    Pull,
    Push,
}

impl Render for BranchPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.load_if_stale(cx);
        let query = self.search.read(cx).value().trim().to_string();
        let lower = query.to_lowercase();
        let matching: Vec<(usize, Branch)> = self
            .branches
            .iter()
            .filter(|branch| lower.is_empty() || branch.name.to_lowercase().contains(&lower))
            .cloned()
            .enumerate()
            .collect();
        let exact = self.branches.iter().any(|branch| branch.name == query);
        let can_create = !query.is_empty() && !exact && self.repo.read(cx).busy().is_none();
        let local: Vec<_> = matching.iter().filter(|(_, b)| !b.remote).collect();
        let remote: Vec<_> = matching.iter().filter(|(_, b)| b.remote).collect();
        let section = |title: &'static str, cx: &Context<Self>| {
            div()
                .px_2()
                .pt_2()
                .pb_1()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(title)
        };
        v_flex()
            .w(px(400.))
            .max_h(px(520.))
            .child(self.render_header(cx))
            .child(
                div().p_2().child(
                    Input::new(&self.search)
                        .small()
                        .prefix(Icon::new(IconName::Search).xsmall()),
                ),
            )
            .when(can_create, |this| {
                let name = query.clone();
                this.child(
                    h_flex()
                        .id("create-branch")
                        .mx_2()
                        .gap_2()
                        .px_2()
                        .py_1p5()
                        .rounded(cx.theme().radius)
                        .cursor_pointer()
                        .hover(|style| style.bg(cx.theme().list_hover))
                        .text_sm()
                        .on_click(cx.listener(move |this, _, _, cx| this.create(name.clone(), cx)))
                        .child(
                            Icon::new(IconName::Plus)
                                .small()
                                .text_color(cx.theme().info),
                        )
                        .child(format!("Create branch “{query}” from here")),
                )
            })
            .child(
                v_flex()
                    .id("branch-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_1()
                    .pb_2()
                    .when(!local.is_empty(), |this| this.child(section("Local", cx)))
                    .children(
                        local
                            .into_iter()
                            .map(|(ix, branch)| self.render_row(*ix, branch, cx)),
                    )
                    .when(!remote.is_empty(), |this| this.child(section("Remote", cx)))
                    .children(
                        remote
                            .into_iter()
                            .map(|(ix, branch)| self.render_row(*ix, branch, cx)),
                    )
                    .when(matching.is_empty() && self.loaded_at.is_some(), |this| {
                        this.child(
                            div()
                                .p_3()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("No branches match."),
                        )
                    }),
            )
    }
}

pub use ph_ui::time::ago;

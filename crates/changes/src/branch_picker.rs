//! The branch picker, opened from the branch in the status bar: switch,
//! create and delete branches, and fetch, pull or push.

use std::time::{Duration, Instant};

use gpui_kit::Focusable as _;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, Subscription, Task,
    Window, div,
};
use ph_git::branches::{self, Branch};
use ph_ui::Icons;

use crate::repo::Repo;

/// Branches older than this are re-read when the picker is shown again.
const STALE: Duration = Duration::from_secs(10);

pub struct BranchPicker {
    repo: Entity<Repo>,
    search: Entity<InputState>,
    /// The remote branches are listed, not the local ones.
    show_remote: bool,
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
            cx.subscribe_in(
                &search,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => this.enter(window, cx),
                    _ => {}
                },
            ),
            // A switch or fetch changes the list.
            cx.observe(&repo, |this, _, cx| {
                this.loaded_at = None;
                cx.notify();
            }),
        ];
        Self {
            repo,
            search,
            show_remote: false,
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

    /// Enter in the search: makes the branch it names, or switches to the
    /// only one it finds.
    fn enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.search.read(cx).value().trim().to_string();
        if self.repo.read(cx).busy().is_some() {
            return;
        }
        let lower = query.to_lowercase();
        let found: Vec<&Branch> = self
            .branches
            .iter()
            .filter(|branch| {
                branch.remote == self.show_remote && branch.name.to_lowercase().contains(&lower)
            })
            .collect();
        // The branch typed, also as it would be made ("fix login" is
        // `fix-login`): one there already is switched to, not made again.
        let made = new_branch_name(&query);
        let exact = self.branches.iter().find(|branch| {
            branch.name == query
                || matches!(&made, Some(Ok(name)) if !branch.remote && branch.name == *name)
        });
        match (exact, made, &found[..]) {
            (Some(branch), _, _) => self.switch(&branch.clone(), cx),
            (None, Some(Ok(name)), _) => self.create(name, window, cx),
            (None, _, [only]) => self.switch(&(*only).clone(), cx),
            _ => {}
        }
    }

    fn create(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        // The list shows every branch again, the new one among them.
        self.search
            .update(cx, |search, cx| search.set_value("", window, cx));
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
                                .child(ph_ui::Loader::new().xsmall())
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
        let busy = self.repo.read(cx).busy().is_some();
        let current = self
            .repo
            .read(cx)
            .status()
            .and_then(|status| status.branch.clone());
        // What the search would make: a branch that is not there yet.
        let new_branch = new_branch_name(&query).filter(|name| match name {
            Ok(name) => !self.branches.iter().any(|branch| &branch.name == name),
            Err(_) => true,
        });
        let local_count = matching.iter().filter(|(_, b)| !b.remote).count();
        let remote_count = matching.len() - local_count;
        // Long lists of either kind: one at a time, chosen by the tabs.
        let shown: Vec<_> = matching
            .iter()
            .filter(|(_, branch)| branch.remote == self.show_remote)
            .collect();
        let tab = |id: &'static str, label: String, remote: bool, cx: &mut Context<Self>| {
            let selected = self.show_remote == remote;
            h_flex()
                .id(id)
                .flex_1()
                .justify_center()
                .h_7()
                .rounded(cx.theme().radius)
                .text_sm()
                .cursor_pointer()
                .text_color(if selected {
                    cx.theme().foreground
                } else {
                    cx.theme().muted_foreground
                })
                .when(selected, |this| {
                    this.bg(cx.theme().background)
                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.show_remote = remote;
                    cx.notify();
                }))
                .child(label)
        };
        let tabs = h_flex()
            .mx_2()
            .p_0p5()
            .gap_0p5()
            .rounded(cx.theme().radius)
            .bg(cx.theme().muted)
            .child(tab(
                "branches-local",
                format!("Local {local_count}"),
                false,
                cx,
            ))
            .child(tab(
                "branches-remote",
                format!("Remote {remote_count}"),
                true,
                cx,
            ));
        v_flex()
            .w(ph_ui::rems_from_px(400.))
            .max_h(ph_ui::rems_from_px(520.))
            .child(self.render_header(cx))
            .child(
                div().p_2().child(
                    Input::new(&self.search)
                        .small()
                        .prefix(Icon::new(IconName::Search).xsmall()),
                ),
            )
            .child(tabs)
            // Making a branch is always on offer here: what to do with no
            // name yet, the branch a name would make, or why a name will
            // not do.
            .child(match new_branch {
                None if !query.is_empty() => div().into_any_element(),
                None => {
                    let search = self.search.clone();
                    create_row(cx)
                        .text_color(cx.theme().muted_foreground)
                        .cursor_pointer()
                        .hover(|style| style.bg(cx.theme().list_hover))
                        .on_click(move |_, window, cx| {
                            search.update(cx, |search, cx| search.focus(window, cx))
                        })
                        .child(Icon::new(IconName::Plus).small())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child("New branch: type its name above, then Enter"),
                        )
                        .into_any_element()
                }
                Some(Ok(name)) => {
                    let from = current.clone().unwrap_or_else(|| "here".into());
                    let label = format!("Create branch “{name}” from {from}");
                    create_row(cx)
                        .when(!busy, |this| {
                            this.cursor_pointer()
                                .hover(|style| style.bg(cx.theme().list_hover))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.create(name.clone(), window, cx)
                                }))
                        })
                        .when(busy, |this| this.opacity(0.5))
                        .child(
                            Icon::new(IconName::Plus)
                                .small()
                                .text_color(cx.theme().info),
                        )
                        .child(div().flex_1().min_w_0().child(label))
                        .child(
                            div()
                                .flex_none()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("Enter"),
                        )
                        .into_any_element()
                }
                Some(Err(why)) => create_row(cx)
                    .text_color(cx.theme().warning)
                    .child(Icon::new(IconName::TriangleAlert).small())
                    .child(div().flex_1().min_w_0().child(why))
                    .into_any_element(),
            })
            .child(
                v_flex()
                    .id("branch-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_1()
                    .pb_2()
                    .pt_1()
                    .children(
                        shown
                            .iter()
                            .map(|(ix, branch)| self.render_row(*ix, branch, cx)),
                    )
                    .when(shown.is_empty() && self.loaded_at.is_some(), |this| {
                        this.child(
                            div()
                                .p_3()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(match (self.show_remote, local_count, remote_count) {
                                    (false, 0, other) | (true, other, 0) if other > 0 => {
                                        format!("None here; {other} in the other tab.")
                                    }
                                    _ => "No branches match.".to_string(),
                                }),
                        )
                    }),
            )
    }
}

pub use ph_ui::time::ago;

/// The row under the search that makes a branch.
fn create_row(cx: &gpui_kit::App) -> gpui_kit::Stateful<gpui_kit::Div> {
    h_flex()
        .id("create-branch")
        .flex_none()
        .mx_2()
        .mt_1()
        .gap_2()
        .px_2()
        .py_1p5()
        .rounded(cx.theme().radius)
        .text_sm()
}

/// The branch `query` would make: spaces become dashes (as GitHub and VS
/// Code do), and a name Git refuses comes back with why. `None` for no
/// name yet.
fn new_branch_name(query: &str) -> Option<Result<String, &'static str>> {
    let name = query.split_whitespace().collect::<Vec<_>>().join("-");
    if name.is_empty() {
        return None;
    }
    // git check-ref-format's rules, said in words before Git is asked.
    let refused = if name
        .chars()
        .any(|c| "~^:?*[\\".contains(c) || c.is_control())
    {
        Some("A branch name cannot contain ~ ^ : ? * [ or \\")
    } else if name.contains("..") || name.contains("@{") || name.contains("//") {
        Some("A branch name cannot contain “..”, “@{” or “//”")
    } else if name.starts_with(['-', '/', '.']) || name.contains("/.") {
        Some("A branch name, or a part of it after a /, cannot start with - / or .")
    } else if name.ends_with(['/', '.']) || name.ends_with(".lock") || name.contains(".lock/") {
        Some("A branch name, or a part of it before a /, cannot end with / . or .lock")
    } else if name == "@" || name == "HEAD" {
        Some("That name is taken by Git itself")
    } else {
        None
    };
    Some(match refused {
        Some(why) => Err(why),
        None => Ok(name),
    })
}

#[cfg(test)]
mod tests {
    use super::new_branch_name;

    #[test]
    fn a_typed_name_becomes_a_branch_name_or_says_why_not() {
        assert_eq!(new_branch_name("   "), None);
        assert_eq!(
            new_branch_name("fix login  page"),
            Some(Ok("fix-login-page".into()))
        );
        assert_eq!(
            new_branch_name("feature/auto-save"),
            Some(Ok("feature/auto-save".into()))
        );
        for refused in [
            "a..b", "-x", "x.lock", "x/", "a~1", "what?", "HEAD", "a:b", "a/.b", "a.lock/b",
        ] {
            assert!(
                matches!(new_branch_name(refused), Some(Err(_))),
                "{refused}"
            );
        }
    }
}

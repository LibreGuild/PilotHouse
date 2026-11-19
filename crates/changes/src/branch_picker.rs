//! The branch picker, opened from the branch in the status bar.
//!
//! Top to bottom: the branch checked out, with fetch, pull and push (each
//! saying how many commits it would move); a search, with **New branch**
//! beside it; and one list of the other branches, the local ones first, then
//! those that are on a remote only. A click on a branch switches to it, and
//! the button at its end opens what else can be done with it: a new branch
//! from it, rename, merge into the branch checked out, delete.
//!
//! Everything asked before it is done (a name, a yes to losing commits) is
//! asked in place, in the list: the picker is a popover, and a menu or a
//! dialog over it would close it.

use std::collections::HashSet;
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
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Task, Window, div,
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
    /// The form that makes a branch, when it is open.
    creating: Option<Creating>,
    /// The branch whose actions show under its row.
    open: Option<String>,
    /// What is asked about the open branch before it is done.
    asking: Option<Asking>,
    /// The branches on a remote only are listed (they are while searching
    /// too, and when there is no other branch here).
    remote_open: bool,
    _load: Option<Task<()>>,
    _check: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

/// The new-branch form.
struct Creating {
    name: Entity<InputState>,
    /// The branch it starts from; `None` for where the repository is now.
    from: Option<String>,
    _enter: Subscription,
}

enum Asking {
    /// A new name for the branch.
    Rename {
        input: Entity<InputState>,
        _enter: Subscription,
    },
    /// It has commits no other branch here has: delete it all the same?
    Delete,
}

impl BranchPicker {
    pub fn new(repo: Entity<Repo>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Find a branch…"));
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
            branches: Vec::new(),
            loaded_at: None,
            creating: None,
            open: None,
            asking: None,
            remote_open: false,
            _load: None,
            _check: None,
            _subscriptions: subscriptions,
        }
    }

    /// Called as the popover renders: on opening (search not focused yet),
    /// starts with an empty, focused search and a fresh branch list.
    pub fn focus_when_opened(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A form of the picker's has the keyboard: it is in use, not opening.
        if self.creating.is_some() || matches!(self.asking, Some(Asking::Rename { .. })) {
            return;
        }
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

    /// The branch checked out, when the repository is on one.
    fn current(&self, cx: &gpui_kit::App) -> Option<String> {
        self.repo
            .read(cx)
            .status()
            .and_then(|status| status.branch.clone())
    }

    fn listed(&self, query: &str) -> (Vec<Branch>, Vec<Branch>) {
        listed(&self.branches, query)
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

    /// Enter in the search: switches to the branch it names or the only one
    /// it finds, or makes the branch it names.
    fn enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.search.read(cx).value().trim().to_string();
        if query.is_empty() || self.repo.read(cx).busy().is_some() {
            return;
        }
        let (here, away) = self.listed(&query);
        // The branch typed, also as it would be made ("fix login" is
        // `fix-login`): one there already is switched to, not made again.
        let made = new_branch_name(&query);
        let exact = self.branches.iter().find(|branch| {
            branch.name == query
                || matches!(&made, Some(Ok(name)) if !branch.remote && branch.name == *name)
        });
        let found: Vec<&Branch> = here.iter().chain(&away).collect();
        let switch_to = match (exact, made, &found[..]) {
            (Some(branch), _, _) if branch.current => return,
            (Some(branch), _, _) => branch.clone(),
            (None, _, [only]) => (*only).clone(),
            (None, Some(Ok(name)), _) => return self.create(name, None, window, cx),
            _ => return,
        };
        // The search found what it was for: the list is whole again.
        self.search
            .update(cx, |search, cx| search.set_value("", window, cx));
        self.switch(&switch_to, cx);
    }

    /// Opens the new-branch form, starting from `from` (where the
    /// repository is now, without one), with `name` typed already.
    fn start_creating(
        &mut self,
        from: Option<String>,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| {
            let mut input =
                InputState::new(window, cx).placeholder("Its name, e.g. fix/login-redirect");
            input.set_value(name, window, cx);
            input
        });
        let enter =
            cx.subscribe_in(
                &input,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => this.submit_creating(window, cx),
                    _ => {}
                },
            );
        input.update(cx, |input, cx| input.focus(window, cx));
        self.creating = Some(Creating {
            name: input,
            from,
            _enter: enter,
        });
        self.asking = None;
        cx.notify();
    }

    fn submit_creating(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(creating) = &self.creating else {
            return;
        };
        let typed = creating.name.read(cx).value().to_string();
        let from = creating.from.clone();
        if let Some(Ok(name)) = new_branch_name(&typed)
            && !self.taken(&name)
            && self.repo.read(cx).busy().is_none()
        {
            self.create(name, from, window, cx);
        }
    }

    /// Whether a local branch has this name already.
    fn taken(&self, name: &str) -> bool {
        self.branches
            .iter()
            .any(|branch| !branch.remote && branch.name == name)
    }

    fn create(
        &mut self,
        name: String,
        from: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The list shows every branch again, the new one checked out.
        self.creating = None;
        self.open = None;
        let search = self.search.clone();
        search.update(cx, |search, cx| {
            search.set_value("", window, cx);
            search.focus(window, cx);
        });
        self.repo.update(cx, |repo, cx| {
            repo.run(
                format!("Creating {name}"),
                move |root| {
                    branches::create(root, &name, from.as_deref())?;
                    Ok(format!("Created and switched to {name}"))
                },
                cx,
            )
        });
    }

    fn start_rename(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("Its new name");
            input.set_value(name.to_string(), window, cx);
            input
        });
        let enter = cx.subscribe_in(
            &input,
            window,
            |this, _, event: &InputEvent, _, cx| match event {
                InputEvent::Change => cx.notify(),
                InputEvent::PressEnter { .. } => this.submit_rename(cx),
                _ => {}
            },
        );
        input.update(cx, |input, cx| input.focus(window, cx));
        self.open = Some(name.to_string());
        self.asking = Some(Asking::Rename {
            input,
            _enter: enter,
        });
        cx.notify();
    }

    fn submit_rename(&mut self, cx: &mut Context<Self>) {
        let (Some(from), Some(Asking::Rename { input, .. })) = (self.open.clone(), &self.asking)
        else {
            return;
        };
        let typed = input.read(cx).value().to_string();
        let Some(Ok(to)) = new_branch_name(&typed) else {
            return;
        };
        if to == from || self.taken(&to) || self.repo.read(cx).busy().is_some() {
            return;
        }
        self.asking = None;
        self.open = None;
        self.repo.update(cx, |repo, cx| {
            repo.run(
                format!("Renaming {from}"),
                move |root| {
                    branches::rename(root, &from, &to)?;
                    Ok(format!("Renamed {from} to {to}"))
                },
                cx,
            )
        });
    }

    fn merge(&mut self, name: String, cx: &mut Context<Self>) {
        self.open = None;
        let into = self.current(cx).unwrap_or_else(|| "this commit".into());
        self.repo.update(cx, |repo, cx| {
            repo.run(
                format!("Merging {name}"),
                move |root| {
                    branches::merge(root, &name)?;
                    Ok(format!("Merged {name} into {into}"))
                },
                cx,
            )
        });
    }

    /// Deletes `name` when nothing is lost with it; asks first when it has
    /// commits that are on no other branch here.
    fn ask_delete(&mut self, name: String, cx: &mut Context<Self>) {
        let root = self.repo.read(cx).root().to_path_buf();
        let checked = name.clone();
        let merged = cx.background_spawn(async move { branches::is_merged(&root, &checked) });
        self._check = Some(cx.spawn(async move |this, cx| {
            let merged = merged.await;
            _ = this.update(cx, |this, cx| {
                if merged {
                    this.delete(name, false, cx);
                } else {
                    this.open = Some(name);
                    this.asking = Some(Asking::Delete);
                    cx.notify();
                }
            });
        }));
    }

    fn delete(&mut self, name: String, force: bool, cx: &mut Context<Self>) {
        self.open = None;
        self.asking = None;
        self.repo.update(cx, |repo, cx| {
            repo.run(
                format!("Deleting {name}"),
                move |root| {
                    branches::delete(root, &name, force)?;
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

    /// The branch checked out, where it stands with its remote, and the
    /// three that move commits between the two.
    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let repo = self.repo.read(cx);
        let status = repo.status().cloned().unwrap_or_default();
        let busy = repo.busy().cloned();
        let idle = busy.is_none();
        let branch = status
            .branch
            .clone()
            .unwrap_or_else(|| format!("Detached at {}", status.head.clone().unwrap_or_default()));
        let commits = |count: u32| if count == 1 { "commit" } else { "commits" };
        let tracking = match (&status.upstream, status.ahead, status.behind) {
            (None, _, _) => "Only on this computer".to_string(),
            (Some(up), 0, 0) => format!("Up to date with {up}"),
            (Some(up), ahead, 0) => format!("{ahead} {} to push to {up}", commits(ahead)),
            (Some(up), 0, behind) => format!("{behind} {} to pull from {up}", commits(behind)),
            (Some(up), ahead, behind) => {
                format!("{ahead} to push, {behind} to pull · {up}")
            }
        };
        // Icons, each saying in its tooltip what it would do, with the
        // number of commits it would move beside it: names on all three
        // left the branch's own name alone at the other end of the row.
        let sync = |id: &'static str, icon: Icon, name: &'static str, count: u32| {
            Button::new(id)
                .small()
                .ghost()
                .icon(icon)
                .when(count > 0, |this| this.label(count.to_string()))
                .accessibility_label(name)
                .disabled(!idle)
        };
        let up = status.upstream.clone().unwrap_or_default();
        let (pull_tip, push_tip) = (
            match status.behind {
                0 => format!("Pull from {up}"),
                behind => format!("Pull {behind} {} from {up}", commits(behind)),
            },
            match (status.upstream.is_some(), status.ahead) {
                (false, _) => "Publish: put this branch on the remote".to_string(),
                (true, 0) => format!("Push to {up}"),
                (true, ahead) => format!("Push {ahead} {} to {up}", commits(ahead)),
            },
        );
        let published = status.upstream.is_some();
        h_flex()
            .gap_2()
            .p_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                Icon::new(Icons::GitBranch)
                    .small()
                    .text_color(cx.theme().link),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(branch),
                    )
                    .child(match busy {
                        Some(busy) => h_flex()
                            .gap_1p5()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(ph_ui::Loader::new().xsmall())
                            .child(busy)
                            .into_any_element(),
                        None => div()
                            .truncate()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(tracking)
                            .into_any_element(),
                    }),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap_0p5()
                    .child(
                        sync("git-fetch", Icon::new(IconName::RefreshCw), "Fetch", 0)
                            .tooltip("Fetch: see what is new on the remotes")
                            .on_click(cx.listener(|this, _, _, cx| this.sync(Sync::Fetch, cx))),
                    )
                    // Nothing to pull from before it is published.
                    .when(published, |this| {
                        this.child(
                            sync(
                                "git-pull",
                                Icon::new(IconName::ArrowDown),
                                "Pull",
                                status.behind,
                            )
                            .tooltip(pull_tip)
                            .on_click(cx.listener(|this, _, _, cx| this.sync(Sync::Pull, cx))),
                        )
                    })
                    .child(
                        sync(
                            "git-push",
                            if published {
                                Icon::new(IconName::ArrowUp)
                            } else {
                                Icon::new(Icons::CloudUpload)
                            },
                            if published { "Push" } else { "Publish" },
                            if published { status.ahead } else { 0 },
                        )
                        .tooltip(push_tip)
                        .on_click(cx.listener(|this, _, _, cx| this.sync(Sync::Push, cx))),
                    ),
            )
    }

    /// The form that makes a branch: its name, where it starts, and why a
    /// name will not do.
    fn render_creating(&self, creating: &Creating, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let typed = creating.name.read(cx).value().to_string();
        let from = creating
            .from
            .clone()
            .or_else(|| self.current(cx))
            .unwrap_or_else(|| "this commit".into());
        let busy = self.repo.read(cx).busy().is_some();
        let (ready, note, warn) = match new_branch_name(&typed) {
            None => (false, None, false),
            Some(Err(why)) => (false, Some(why.to_string()), true),
            Some(Ok(name)) if self.taken(&name) => (
                false,
                Some(format!("There is a branch “{name}” already.")),
                true,
            ),
            // Said when it is not what was typed (spaces become dashes).
            Some(Ok(name)) if name != typed.trim() => {
                (true, Some(format!("It will be called {name}.")), false)
            }
            Some(Ok(_)) => (true, None, false),
        };
        v_flex()
            .id("new-branch-form")
            .mx_2()
            .mt_2()
            .p_2()
            .gap_2()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .bg(theme.secondary)
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(format!("New branch from {from}")),
            )
            .child(Input::new(&creating.name).small())
            .when_some(note, |this, note| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(if warn {
                            theme.warning
                        } else {
                            theme.muted_foreground
                        })
                        .child(note),
                )
            })
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("new-branch-cancel")
                            .small()
                            .ghost()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.creating = None;
                                let search = this.search.clone();
                                search.update(cx, |search, cx| search.focus(window, cx));
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("new-branch-create")
                            .small()
                            .primary()
                            .label("Create branch")
                            .disabled(!ready || busy)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.submit_creating(window, cx)),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_row(&self, branch: &Branch, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (row_branch, toggled) = (branch.clone(), branch.name.clone());
        let open = self.open.as_deref() == Some(branch.name.as_str());
        // Where it stands with its remote, when the two differ.
        let apart = match (branch.ahead, branch.behind) {
            (0, 0) => None,
            (ahead, 0) => Some(format!("↑{ahead}")),
            (0, behind) => Some(format!("↓{behind}")),
            (ahead, behind) => Some(format!("↑{ahead} ↓{behind}")),
        };
        let row = h_flex()
            .id(SharedString::from(format!("branch-{}", branch.name)))
            .group("branch-row")
            .gap_2()
            .px_2()
            .py_1()
            .rounded(theme.radius)
            .cursor_pointer()
            .hover(|style| style.bg(theme.list_hover))
            .when(open, |this| this.bg(theme.list_hover))
            .on_click(cx.listener(move |this, _, _, cx| this.switch(&row_branch, cx)))
            .child(
                Icon::new(if branch.remote {
                    Icons::Globe
                } else {
                    Icons::GitBranch
                })
                .xsmall()
                .text_color(theme.muted_foreground),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .min_w_0()
                                    .text_sm()
                                    .truncate()
                                    .child(branch.name.clone()),
                            )
                            .when_some(apart, |this, apart| {
                                this.child(
                                    div()
                                        .flex_none()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(apart),
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .truncate()
                            .text_color(theme.muted_foreground)
                            .child(format!("{} · {}", ago(branch.updated), branch.subject)),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .when(!open, |this| {
                        this.invisible()
                            .group_hover("branch-row", |style| style.visible())
                    })
                    .child(
                        Button::new(SharedString::from(format!("branch-more-{}", branch.name)))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Ellipsis)
                            .tooltip("What else to do with it")
                            .accessibility_label("Branch actions")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.asking = None;
                                this.open = (!open).then(|| toggled.clone());
                                cx.notify();
                            })),
                    ),
            );
        if !open {
            return row.into_any_element();
        }
        v_flex()
            .child(row)
            .child(self.render_actions(branch, cx))
            .into_any_element()
    }

    /// What can be done with a branch besides switching to it, under its
    /// row; or what is asked before one of those is done.
    fn render_actions(&self, branch: &Branch, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let name = branch.name.clone();
        let busy = self.repo.read(cx).busy().is_some();
        let strip = || h_flex().flex_wrap().gap_1().pl_7().pr_2().pb_1p5();
        match &self.asking {
            Some(Asking::Rename { input, .. }) => {
                let typed = input.read(cx).value().to_string();
                let ready = matches!(new_branch_name(&typed), Some(Ok(to)) if to != name && !self.taken(&to));
                strip()
                    .child(div().flex_1().min_w_0().child(Input::new(input).small()))
                    .child(
                        Button::new("branch-rename-cancel")
                            .small()
                            .ghost()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.asking = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("branch-rename-save")
                            .small()
                            .primary()
                            .label("Rename")
                            .disabled(!ready || busy)
                            .on_click(cx.listener(|this, _, _, cx| this.submit_rename(cx))),
                    )
                    .into_any_element()
            }
            Some(Asking::Delete) => {
                let into = self
                    .current(cx)
                    .unwrap_or_else(|| "the commit checked out".into());
                v_flex()
                    .gap_1p5()
                    .pl_7()
                    .pr_2()
                    .pb_1p5()
                    .child(div().text_xs().text_color(theme.warning).child(format!(
                        "It has commits that are not in {into}. Deleting it loses them."
                    )))
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new("branch-delete-anyway")
                                    .small()
                                    .danger()
                                    .label("Delete anyway")
                                    .disabled(busy)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.delete(name.clone(), true, cx)
                                    })),
                            )
                            .child(
                                Button::new("branch-delete-keep")
                                    .small()
                                    .ghost()
                                    .label("Keep it")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.asking = None;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .into_any_element()
            }
            None => {
                let action = |id: &'static str, label: String| {
                    Button::new(id)
                        .xsmall()
                        .outline()
                        .label(label)
                        .disabled(busy)
                };
                let (from, renamed, merged, deleted) =
                    (name.clone(), name.clone(), name.clone(), name.clone());
                strip()
                    .child(
                        action("branch-new-from", "New branch from here".into()).on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.start_creating(Some(from.clone()), String::new(), window, cx)
                            }),
                        ),
                    )
                    // A branch on a remote is the remote's to rename, merge
                    // from after a checkout, or delete.
                    .when(!branch.remote, |this| {
                        this.child(
                            action("branch-rename", "Rename".into()).on_click(cx.listener(
                                move |this, _, window, cx| this.start_rename(&renamed, window, cx),
                            )),
                        )
                        .when_some(self.current(cx), |this, current| {
                            this.child(
                                action("branch-merge", format!("Merge into {current}")).on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.merge(merged.clone(), cx)
                                    }),
                                ),
                            )
                        })
                        .child(
                            action("branch-delete", "Delete".into())
                                .icon(Icons::Trash)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.ask_delete(deleted.clone(), cx)
                                })),
                        )
                    })
                    .into_any_element()
            }
        }
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
        let (here, away) = self.listed(&query);
        let busy = self.repo.read(cx).busy().is_some();
        let current = self.current(cx);
        // What the search would make: a branch that is not there yet.
        let new_branch = new_branch_name(&query).filter(|name| match name {
            Ok(name) => !self.branches.iter().any(|branch| &branch.name == name),
            Err(_) => true,
        });
        // The branches on a remote only are many in a shared repository:
        // folded until asked for, searched for, or all there is.
        let remote_shown = self.remote_open || !query.is_empty() || here.is_empty();
        let muted = cx.theme().muted_foreground;
        let heading = move |text: String| {
            div()
                .px_2()
                .pt_2()
                .pb_1()
                .text_xs()
                .font_weight(gpui_kit::FontWeight::MEDIUM)
                .text_color(muted)
                .child(text)
        };
        let nothing = here.is_empty() && away.is_empty();
        let loading = self.loaded_at.is_none();

        let top = match &self.creating {
            Some(creating) => self.render_creating(creating, cx),
            None => h_flex()
                .gap_2()
                .px_2()
                .pt_2()
                .child(
                    div().flex_1().min_w_0().child(
                        Input::new(&self.search)
                            .small()
                            .prefix(Icon::new(IconName::Search).xsmall()),
                    ),
                )
                .child(
                    Button::new("new-branch")
                        .small()
                        .outline()
                        .icon(IconName::Plus)
                        .tooltip("New branch")
                        .accessibility_label("New branch")
                        .on_click(cx.listener(|this, _, window, cx| {
                            // What was typed to find a branch is likely
                            // the name of the one wanted.
                            let typed = this.search.read(cx).value().trim().to_string();
                            this.start_creating(None, typed, window, cx)
                        })),
                )
                .into_any_element(),
        };

        // Searching for a name no branch has offers to make it, at once.
        let quick = match (&self.creating, new_branch) {
            (None, Some(Ok(name))) => {
                let from = current.clone().unwrap_or_else(|| "here".into());
                let label = format!("Create branch “{name}” from {from}");
                Some(
                    create_row(cx)
                        .when(!busy, |this| {
                            this.cursor_pointer()
                                .hover(|style| style.bg(cx.theme().list_hover))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.create(name.clone(), None, window, cx)
                                }))
                        })
                        .when(busy, |this| this.opacity(0.5))
                        .child(
                            Icon::new(IconName::Plus)
                                .small()
                                .text_color(cx.theme().link),
                        )
                        .child(div().flex_1().min_w_0().truncate().child(label))
                        .into_any_element(),
                )
            }
            (None, Some(Err(why))) if nothing => Some(
                create_row(cx)
                    .text_color(cx.theme().warning)
                    .child(Icon::new(IconName::TriangleAlert).small())
                    .child(div().flex_1().min_w_0().child(why))
                    .into_any_element(),
            ),
            _ => None,
        };

        // A search that finds nothing and offers nothing says so (the branch
        // checked out is over the list, not in it).
        let no_match = (!loading && nothing && !query.is_empty() && quick.is_none()).then(|| {
            if current.as_deref() == Some(query.as_str()) {
                format!("You are on {query}.")
            } else {
                "No branch matches.".to_string()
            }
        });
        let remote_count = away.len();
        v_flex()
            .w(ph_ui::rems_from_px(420.))
            .max_h(ph_ui::rems_from_px(560.))
            .child(self.render_header(cx))
            .child(top)
            .children(quick)
            .child(
                v_flex()
                    .id("branch-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_1()
                    .pb_2()
                    .when(!here.is_empty(), |this| {
                        this.child(heading("Local".into()))
                            .children(here.iter().map(|branch| self.render_row(branch, cx)))
                    })
                    .when(remote_count > 0, |this| {
                        this.child(
                            h_flex()
                                .id("branches-remote")
                                .gap_1()
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.remote_open = !this.remote_open;
                                    cx.notify();
                                }))
                                .child(heading(format!("On a remote only · {remote_count}")))
                                // Nothing to fold while it is all there is.
                                .when(query.is_empty() && !here.is_empty(), |this| {
                                    this.child(
                                        div().pt_1().child(
                                            Icon::new(if remote_shown {
                                                IconName::ChevronDown
                                            } else {
                                                IconName::ChevronRight
                                            })
                                            .xsmall()
                                            .text_color(cx.theme().muted_foreground),
                                        ),
                                    )
                                }),
                        )
                        .when(remote_shown, |this| {
                            this.children(away.iter().map(|branch| self.render_row(branch, cx)))
                        })
                    })
                    .when(loading && nothing, |this| {
                        this.child(
                            h_flex()
                                .gap_2()
                                .p_3()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(ph_ui::Loader::new().xsmall())
                                .child("Reading the branches…"),
                        )
                    })
                    .when_some(no_match, |this, text| {
                        this.child(
                            div()
                                .p_3()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(text),
                        )
                    })
                    .when(!loading && nothing && query.is_empty(), |this| {
                        this.child(
                            div()
                                .p_3()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("No other branches yet."),
                        )
                    }),
            )
    }
}

pub use ph_ui::time::ago;

/// The other local branches, and the branches on a remote only, that
/// `query` finds. The branch checked out is not among them (it is over the
/// list), nor is a remote branch that a local one tracks or shares its name
/// with: that is the local branch, and listing both made every branch of a
/// shared repository show twice.
fn listed(all: &[Branch], query: &str) -> (Vec<Branch>, Vec<Branch>) {
    let lower = query.to_lowercase();
    let matches =
        |branch: &&Branch| lower.is_empty() || branch.name.to_lowercase().contains(&lower);
    let local: HashSet<&str> = all
        .iter()
        .filter(|branch| !branch.remote)
        .flat_map(|branch| std::iter::once(branch.name.as_str()).chain(branch.upstream.as_deref()))
        .collect();
    let here = all
        .iter()
        .filter(|branch| !branch.remote && !branch.current)
        .filter(matches)
        .cloned()
        .collect();
    let away = all
        .iter()
        .filter(|branch| {
            branch.remote
                && !local.contains(branch.name.as_str())
                && !local.contains(short_name(&branch.name))
        })
        .filter(matches)
        .cloned()
        .collect();
    (here, away)
}

/// The row under the search that makes the branch searched for.
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

/// `feature/x` of `origin/feature/x`: a remote branch's name without its
/// remote.
fn short_name(remote_branch: &str) -> &str {
    remote_branch
        .split_once('/')
        .map_or(remote_branch, |(_, rest)| rest)
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
    use super::{Branch, listed, new_branch_name};

    fn branch(name: &str, remote: bool, current: bool, upstream: Option<&str>) -> Branch {
        Branch {
            name: name.into(),
            remote,
            current,
            upstream: upstream.map(str::to_string),
            ahead: 0,
            behind: 0,
            updated: 0,
            subject: String::new(),
        }
    }

    /// One list, each branch once: the branch checked out is over it, and a
    /// remote branch shows only when no local branch stands for it.
    #[test]
    fn the_list_has_each_branch_once() {
        let all = [
            branch("main", false, true, Some("origin/main")),
            branch("feature/chart", false, false, Some("origin/feature/chart")),
            branch("notes", false, false, None),
            branch("origin/main", true, false, None),
            branch("origin/feature/chart", true, false, None),
            // Not tracked, but a local branch of its name is it all the same.
            branch("origin/notes", true, false, None),
            branch("origin/dependabot/serde", true, false, None),
        ];
        let names = |list: Vec<Branch>| list.into_iter().map(|b| b.name).collect::<Vec<_>>();
        let (here, away) = listed(&all, "");
        assert_eq!(names(here), ["feature/chart", "notes"]);
        assert_eq!(names(away), ["origin/dependabot/serde"]);
        // A search looks through both, whatever its case.
        let (here, away) = listed(&all, "SERDE");
        assert!(here.is_empty());
        assert_eq!(names(away), ["origin/dependabot/serde"]);
        let (here, away) = listed(&all, "main");
        assert!(
            here.is_empty() && away.is_empty(),
            "the one checked out is over the list"
        );
    }

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

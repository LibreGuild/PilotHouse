//! Parallel runs: one prompt, several agents, each in its own worktree from
//! the same snapshot of the project (uncommitted work included), so their
//! results can be compared and one kept.
//!
//! Worktrees live in PilotHouse's data folder and hold a full copy of the
//! project's files (Git's history is shared), so the start dialog says how
//! much space each copy takes, and discarding a lane removes its copy.

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    notification::Notification,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Task,
    WeakEntity, Window, div, px,
};
use ph_agent::agents::{AGENTS, AgentSpec, Availability};
use ph_git::parallel::{Applied, DiffStat};
use ph_store::parallel::{Lane, ParallelRun};
use ph_store::projects::ProjectId;

use crate::AppState;
use crate::project_space::ProjectSpace;

/// `<data dir>/worktrees/<project>-run<run>-<agent>`.
fn lane_path(project: ProjectId, run: i64, agent: &str) -> PathBuf {
    ph_store::paths::data_dir()
        .join("worktrees")
        .join(format!("{project}-run{run}-{agent}"))
}

/// Takes the snapshot and makes a worktree per agent (blocking). Returns the
/// run's id and each lane's id and worktree.
fn prepare(
    store: &ph_store::Store,
    project: ProjectId,
    root: &std::path::Path,
    prompt: &str,
    agents: &[&'static AgentSpec],
) -> Result<(i64, Vec<(i64, PathBuf)>)> {
    let base = ph_git::checkpoint::create(root, &format!("Parallel run: {prompt}"))?
        .context("parallel runs need a Git repository")?;
    let run = store.create_parallel_run(project, prompt, &base)?;
    let mut lanes = Vec::new();
    for agent in agents {
        let path = lane_path(project, run, agent.id);
        if let Some(parent) = path.parent() {
            ph_store::paths::ensure_private_dir(parent)?;
        }
        ph_git::worktree::add_detached(root, &path, &base)
            .with_context(|| format!("making a worktree for {}", agent.name))?;
        let lane = store.add_parallel_lane(run, agent.id, &path.to_string_lossy())?;
        lanes.push((lane, path));
    }
    Ok((run, lanes))
}

/// The dialog that picks agents for `prompt` and starts the run.
pub fn open_start(
    space: WeakEntity<ProjectSpace>,
    root: PathBuf,
    prompt: String,
    window: &mut Window,
    cx: &mut App,
) {
    let view = cx.new(|cx| StartView::new(space, root, prompt, window, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title("Run with several agents")
            .w(px(560.))
            .child(view.clone())
    });
}

struct StartView {
    space: WeakEntity<ProjectSpace>,
    root: PathBuf,
    prompt: Entity<TextareaState>,
    /// Installed agents and whether each is picked.
    agents: Vec<(&'static AgentSpec, bool)>,
    /// How much a copy of the project's files takes.
    size: Option<u64>,
    starting: bool,
    error: Option<SharedString>,
    _size: Task<()>,
}

impl StartView {
    fn new(
        space: WeakEntity<ProjectSpace>,
        root: PathBuf,
        prompt: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx)
                .placeholder("What every agent should do")
                .rows(4);
            state.set_value(prompt, window, cx);
            state
        });
        let agents: Vec<_> = AGENTS
            .iter()
            .filter(|agent| matches!(agent.availability(), Availability::Ready { .. }))
            .enumerate()
            .map(|(ix, agent)| (agent, ix < 2))
            .collect();
        let measure = root.clone();
        let size = cx.spawn(async move |this, cx| {
            let size = cx
                .background_spawn(async move { ph_git::worktree::checkout_size(&measure) })
                .await
                .ok();
            _ = this.update(cx, |this, cx| {
                this.size = size;
                cx.notify();
            });
        });
        Self {
            space,
            root,
            prompt: editor,
            agents,
            size: None,
            starting: false,
            error: None,
            _size: size,
        }
    }

    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = self.prompt.read(cx).value().trim().to_string();
        let agents: Vec<&'static AgentSpec> = self
            .agents
            .iter()
            .filter(|(_, picked)| *picked)
            .map(|(agent, _)| *agent)
            .collect();
        if prompt.is_empty() || agents.len() < 2 {
            self.error = Some("Write what to do and pick at least two agents.".into());
            cx.notify();
            return;
        }
        let Some(space) = self.space.upgrade() else {
            return;
        };
        let project = space.read(cx).project().id;
        let root = self.root.clone();
        self.starting = true;
        self.error = None;
        cx.notify();
        let picked = agents.clone();
        let task_prompt = prompt.clone();
        let store = AppState::store(cx).clone();
        let work =
            cx.background_spawn(
                async move { prepare(&store, project, &root, &task_prompt, &picked) },
            );
        cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            _ = this.update_in(cx, |this, window, cx| match result {
                Ok((_, lanes)) => {
                    window.close_dialog(cx);
                    space.update(cx, |space, cx| {
                        space.run_parallel(&prompt, agents, lanes, window, cx)
                    });
                }
                Err(err) => {
                    this.starting = false;
                    this.error = Some(format!("{err:#}").into());
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

/// `12.3 MB`.
fn size_text(bytes: u64) -> String {
    let kb = bytes as f64 / 1024.0;
    let mb = kb / 1024.0;
    if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else if mb >= 1.0 {
        format!("{mb:.1} MB")
    } else {
        format!("{} KB", kb.ceil() as u64)
    }
}

impl Render for StartView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let picked = self.agents.iter().filter(|(_, picked)| *picked).count();
        let space_note = match self.size {
            Some(size) => format!(
                "Each agent works on its own copy of the project ({} each, {} in all), made from its current state, uncommitted changes included. Copies live in PilotHouse's data folder; discard them from Parallel runs when done.",
                size_text(size),
                size_text(size * picked as u64)
            ),
            None => "Each agent works on its own copy of the project, made from its current state."
                .to_string(),
        };
        v_flex()
            .gap_3()
            .child(Textarea::new(&self.prompt))
            .child(
                h_flex()
                    .gap_3()
                    .flex_wrap()
                    .children(self.agents.iter().enumerate().map(|(ix, (agent, picked))| {
                        Checkbox::new(("parallel-agent", ix))
                            .label(agent.name)
                            .checked(*picked)
                            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                if let Some(entry) = this.agents.get_mut(ix) {
                                    entry.1 = *checked;
                                }
                                cx.notify();
                            }))
                    })),
            )
            .when(self.agents.len() < 2, |this| {
                this.child(div().text_sm().text_color(cx.theme().warning).child(
                    "Install at least two agents (Settings → Agents) to run them side by side.",
                ))
            })
            .child(div().text_xs().text_color(muted).child(space_note))
            .when_some(self.error.clone(), |this, error| {
                this.child(div().text_xs().text_color(cx.theme().danger).child(error))
            })
            .child(
                h_flex().justify_end().child(
                    Button::new("parallel-start")
                        .small()
                        .primary()
                        .label(if self.starting {
                            "Preparing…"
                        } else {
                            "Start"
                        })
                        .disabled(self.starting || picked < 2)
                        .on_click(cx.listener(|this, _, window, cx| this.start(window, cx))),
                ),
            )
    }
}

/// The dialog comparing the project's runs, to keep one lane's work.
pub fn open_runs(
    space: WeakEntity<ProjectSpace>,
    project: ProjectId,
    root: PathBuf,
    window: &mut Window,
    cx: &mut App,
) {
    let view = cx.new(|cx| RunsView::new(space, project, root, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title("Parallel runs")
            .w(px(640.))
            .child(view.clone())
    });
}

struct RunsView {
    space: WeakEntity<ProjectSpace>,
    project: ProjectId,
    root: PathBuf,
    runs: Vec<ParallelRun>,
    /// Each open lane's changes, once measured.
    stats: std::collections::HashMap<i64, Result<DiffStat, SharedString>>,
    busy: Option<i64>,
    _measure: Option<Task<()>>,
}

impl RunsView {
    fn new(
        space: WeakEntity<ProjectSpace>,
        project: ProjectId,
        root: PathBuf,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self {
            space,
            project,
            root,
            runs: Vec::new(),
            stats: Default::default(),
            busy: None,
            _measure: None,
        };
        view.reload(cx);
        view
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.runs = AppState::store(cx)
            .parallel_runs(self.project)
            .unwrap_or_default()
            .into_iter()
            .take(10)
            .collect();
        let open: Vec<(i64, PathBuf, String)> = self
            .runs
            .iter()
            .flat_map(|run| {
                run.lanes
                    .iter()
                    .filter(|lane| lane.state == "open")
                    .map(|lane| (lane.id, PathBuf::from(&lane.worktree), run.base.clone()))
            })
            .collect();
        self._measure = Some(cx.spawn(async move |this, cx| {
            for (lane, worktree, base) in open {
                let stat = cx
                    .background_spawn(
                        async move { ph_git::parallel::changes_since(&worktree, &base) },
                    )
                    .await
                    .map_err(|err| SharedString::from(format!("{err:#}")));
                if this
                    .update(cx, |this, cx| {
                        this.stats.insert(lane, stat);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
        cx.notify();
    }

    /// Brings a lane's changes into the project (after a checkpoint of it).
    fn apply(&mut self, lane: Lane, base: String, window: &mut Window, cx: &mut Context<Self>) {
        self.busy = Some(lane.id);
        cx.notify();
        let root = self.root.clone();
        let work = cx.background_spawn(async move {
            ph_git::checkpoint::create(&root, "Before keeping a parallel run's work")?;
            let patch = ph_git::parallel::patch_since(std::path::Path::new(&lane.worktree), &base)?;
            ph_git::parallel::apply_patch(&root, &patch)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            _ = this.update_in(cx, |this, window, cx| {
                this.busy = None;
                let note = match result {
                    Ok(applied) => {
                        let _ = AppState::store(cx).set_lane_state(lane.id, "applied");
                        match applied {
                            Applied::Clean => Notification::success(
                                "Applied that agent's changes to the project.",
                            ),
                            Applied::WithConflicts => Notification::warning(
                                "Applied with conflicts: resolve them in the Git tab.",
                            ),
                        }
                    }
                    Err(err) => {
                        Notification::error(format!("Could not apply the changes: {err:#}"))
                    }
                };
                window.push_notification(note, cx);
                this.reload(cx);
            });
        })
        .detach();
    }

    /// Removes a kept lane's worktree; it stays kept.
    fn remove_copy(&mut self, lane: Lane, cx: &mut Context<Self>) {
        if let Err(err) =
            ph_git::worktree::remove(&self.root, std::path::Path::new(&lane.worktree), true)
        {
            tracing::warn!("removing {} failed: {err:#}", lane.worktree);
        }
        self.reload(cx);
    }

    /// Removes a lane's worktree.
    fn discard(&mut self, lane: Lane, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let result = ph_git::worktree::remove(&root, std::path::Path::new(&lane.worktree), true);
        if result.is_ok() || !std::path::Path::new(&lane.worktree).exists() {
            let _ = AppState::store(cx).set_lane_state(lane.id, "discarded");
        }
        self.reload(cx);
    }
}

impl Render for RunsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let rows = self.runs.iter().map(|run| {
            let lanes = run.lanes.iter().map(|lane| {
                let name = ph_agent::agents::find(&lane.agent_id)
                    .map(|agent| agent.name)
                    .unwrap_or("Agent");
                let stat = match (lane.state.as_str(), self.stats.get(&lane.id)) {
                    ("applied", _) => "Kept: its changes are in the project".to_string(),
                    ("discarded", _) => "Discarded".to_string(),
                    (_, Some(Ok(stat))) if stat.files == 0 => "No changes yet".to_string(),
                    (_, Some(Ok(stat))) => format!(
                        "{} file{} · +{} −{}",
                        stat.files,
                        if stat.files == 1 { "" } else { "s" },
                        stat.insertions,
                        stat.deletions
                    ),
                    (_, Some(Err(err))) => err.to_string(),
                    (_, None) => "Measuring…".to_string(),
                };
                let open = lane.state == "open";
                // A kept lane's copy stays for its chat until removed.
                let copy_left = lane.state == "applied" && std::path::Path::new(&lane.worktree).exists();
                let remove_lane = lane.clone();
                let busy = self.busy == Some(lane.id);
                let (apply_lane, discard_lane, base) = (lane.clone(), lane.clone(), run.base.clone());
                let session = lane.session_id;
                let space = self.space.clone();
                h_flex()
                    .gap_2()
                    .px_3()
                    .py_1p5()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().secondary)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_sm().child(name))
                            .child(div().text_xs().text_color(muted).child(stat)),
                    )
                    .when_some(session, |this, session| {
                        this.child(
                            Button::new(SharedString::from(format!("lane-chat-{}", lane.id)))
                                .xsmall()
                                .ghost()
                                .label("Chat")
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    _ = space.update(cx, |space, cx| space.select_session(session, cx));
                                }),
                        )
                    })
                    .when(copy_left, |this| {
                        this.child(
                            Button::new(SharedString::from(format!("lane-remove-{}", lane.id)))
                                .xsmall()
                                .ghost()
                                .label("Remove copy")
                                .tooltip("Its changes are in the project; free the space its copy takes")
                                .on_click(cx.listener(move |this, _, _, cx| this.remove_copy(remove_lane.clone(), cx))),
                        )
                    })
                    .when(open, |this| {
                        this.child(
                            Button::new(SharedString::from(format!("lane-apply-{}", lane.id)))
                                .xsmall()
                                .primary()
                                .label(if busy { "Applying…" } else { "Keep" })
                                .disabled(busy)
                                .tooltip("Apply this agent's changes to the project (a checkpoint is taken first)")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.apply(apply_lane.clone(), base.clone(), window, cx)
                                })),
                        )
                        .child(
                            Button::new(SharedString::from(format!("lane-discard-{}", lane.id)))
                                .xsmall()
                                .ghost()
                                .label("Discard")
                                .tooltip("Remove this agent's copy of the project")
                                .on_click(cx.listener(move |this, _, _, cx| this.discard(discard_lane.clone(), cx))),
                        )
                    })
            });
            v_flex()
                .gap_1()
                .child(
                    h_flex()
                        .gap_2()
                        .child(div().flex_1().min_w_0().truncate().text_sm().child(run.prompt.clone()))
                        .child(div().text_xs().text_color(muted).child(ph_ui::time::ago(run.created_at / 1000))),
                )
                .children(lanes)
        });
        v_flex()
            .id("parallel-runs")
            .gap_3()
            .max_h(px(520.))
            .overflow_y_scroll()
            .children(rows)
            .when(self.runs.is_empty(), |this| {
                this.child(div().py_6().text_center().text_sm().text_color(muted).child(
                    "No parallel runs yet. Start one from the composer's + menu: Run with several agents.",
                ))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_plainly() {
        assert_eq!(size_text(5 << 20), "5.0 MB");
        assert_eq!(size_text(3 << 30), "3.0 GB");
        assert_eq!(size_text(1_500), "2 KB");
    }
}

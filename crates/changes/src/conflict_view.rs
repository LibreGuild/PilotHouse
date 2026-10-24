//! Resolving one file's merge conflicts block by block: keep ours, theirs or
//! both, or ask the agent for a combined version, then mark it resolved.

use std::path::PathBuf;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    notification::Notification,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Task,
    Window, div,
};
use ph_git::conflicts::{self, Resolution, Segment};

use crate::commit_box::{Writer, cut};
use crate::repo::Repo;

#[derive(Debug, Clone)]
pub enum ConflictEvent {
    /// The file is resolved (or the user went back).
    Close,
}

pub struct ConflictView {
    repo: Entity<Repo>,
    path: String,
    segments: Vec<Segment>,
    resolutions: Vec<Option<Resolution>>,
    writer: Option<Writer>,
    /// The conflict an agent is merging now.
    asking: Option<usize>,
    error: Option<SharedString>,
    _task: Option<Task<()>>,
}

impl EventEmitter<ConflictEvent> for ConflictView {}

impl ConflictView {
    pub fn new(
        repo: Entity<Repo>,
        path: String,
        writer: Option<Writer>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self {
            repo,
            path,
            segments: Vec::new(),
            resolutions: Vec::new(),
            writer,
            asking: None,
            error: None,
            _task: None,
        };
        view.load(cx);
        view
    }

    fn file(&self, cx: &Context<Self>) -> PathBuf {
        self.repo.read(cx).root().join(&self.path)
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        match std::fs::read_to_string(self.file(cx)) {
            Ok(text) => {
                self.segments = conflicts::parse(&text);
                self.resolutions = vec![None; conflicts::conflicts(&self.segments).len()];
            }
            Err(err) => self.error = Some(format!("Cannot read {}: {err}", self.path).into()),
        }
        cx.notify();
    }

    fn resolve(&mut self, ix: usize, resolution: Resolution, cx: &mut Context<Self>) {
        if let Some(slot) = self.resolutions.get_mut(ix) {
            *slot = Some(resolution);
            cx.notify();
        }
    }

    /// Asks the agent to merge both sides of conflict `ix`.
    fn ask_agent(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(writer), Some(conflict)) = (
            self.writer.clone(),
            conflicts::conflicts(&self.segments)
                .get(ix)
                .cloned()
                .cloned(),
        ) else {
            return;
        };
        let prompt = format!(
            "Resolve this git merge conflict in `{}`. Combine both sides so the \
             intent of each is kept. Reply with only the merged text, no \
             markers, no code fences, no commentary, and do not use tools.\n\n\
             Ours ({}):\n```\n{}```\n\nTheirs ({}):\n```\n{}```",
            self.path,
            conflict.ours_label,
            cut(&conflict.ours, 6_000),
            conflict.theirs_label,
            cut(&conflict.theirs, 6_000)
        );
        let root = self.repo.read(cx).root().to_path_buf();
        self.asking = Some(ix);
        cx.notify();
        let task = writer(prompt, root, cx);
        self._task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            _ = this.update_in(cx, |this, window, cx| {
                this.asking = None;
                match result {
                    Ok(text) => {
                        let mut text = crate::commit_box::clean_message(&text);
                        if !text.ends_with('\n') {
                            text.push('\n');
                        }
                        this.resolve(ix, Resolution::Custom(text), cx);
                    }
                    Err(err) => {
                        window.push_notification(Notification::error(format!("{err:#}")), cx)
                    }
                }
                cx.notify();
            });
        }));
    }

    /// Writes the file with the chosen resolutions and stages it.
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = conflicts::render(&self.segments, &self.resolutions);
        if let Err(err) = std::fs::write(self.file(cx), text) {
            window.push_notification(Notification::error(format!("Cannot save: {err}")), cx);
            return;
        }
        let path = self.path.clone();
        self.repo.update(cx, |repo, cx| {
            repo.run(
                "Marking resolved",
                move |root| {
                    conflicts::mark_resolved(root, &path).map(|_| format!("Resolved {path}"))
                },
                cx,
            )
        });
        cx.emit(ConflictEvent::Close);
    }

    fn render_conflict(
        &self,
        ix: usize,
        conflict: &conflicts::Conflict,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let chosen = self.resolutions.get(ix).cloned().flatten();
        let side = |title: String, text: &str, picked: bool, cx: &Context<Self>| {
            v_flex()
                .flex_1()
                .min_w_0()
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(if picked {
                    cx.theme().info
                } else {
                    cx.theme().border
                })
                .child(
                    div()
                        .px_2()
                        .py_0p5()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .bg(cx.theme().secondary)
                        .child(title),
                )
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_xs()
                        .child(text.trim_end().to_string()),
                )
        };
        let picked = |which: &Resolution| chosen.as_ref() == Some(which);
        let button = |id: &'static str,
                      label: &'static str,
                      resolution: Resolution,
                      cx: &mut Context<Self>| {
            let selected = chosen.as_ref() == Some(&resolution);
            Button::new((id, ix))
                .xsmall()
                .when(selected, |this| this.primary())
                .when(!selected, |this| this.outline())
                .label(label)
                .on_click(
                    cx.listener(move |this, _, _, cx| this.resolve(ix, resolution.clone(), cx)),
                )
        };
        v_flex()
            .flex_none()
            .gap_2()
            .p_2()
            .rounded(cx.theme().radius_lg)
            .border_1()
            .border_color(if chosen.is_some() {
                cx.theme().success.opacity(0.6)
            } else {
                cx.theme().warning.opacity(0.6)
            })
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .child(format!("Conflict {}", ix + 1)),
                    )
                    .child(button("use-ours", "Ours", Resolution::Ours, cx))
                    .child(button("use-theirs", "Theirs", Resolution::Theirs, cx))
                    .child(button("use-both", "Both", Resolution::Both, cx))
                    .when(self.writer.is_some(), |this| {
                        this.child(
                            Button::new(("ask-agent-merge", ix))
                                .xsmall()
                                .ghost()
                                .icon(ph_ui::Icons::Sparkles)
                                .label(if self.asking == Some(ix) {
                                    "Merging…"
                                } else {
                                    "Ask agent"
                                })
                                .disabled(self.asking.is_some())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.ask_agent(ix, window, cx)
                                })),
                        )
                    }),
            )
            .child(
                h_flex()
                    .items_start()
                    .gap_2()
                    .child(side(
                        format!("Ours · {}", conflict.ours_label),
                        &conflict.ours,
                        picked(&Resolution::Ours),
                        cx,
                    ))
                    .child(side(
                        format!("Theirs · {}", conflict.theirs_label),
                        &conflict.theirs,
                        picked(&Resolution::Theirs),
                        cx,
                    )),
            )
            .when_some(
                match &chosen {
                    Some(Resolution::Custom(text)) => Some(text.clone()),
                    _ => None,
                },
                |this, text| this.child(side("Agent's merge".into(), &text, true, cx)),
            )
            .into_any_element()
    }
}

impl Render for ConflictView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let found: Vec<conflicts::Conflict> = conflicts::conflicts(&self.segments)
            .into_iter()
            .cloned()
            .collect();
        let done = self.resolutions.iter().filter(|r| r.is_some()).count();
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .px_3()
                    .py_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        Button::new("conflict-back")
                            .ghost()
                            .xsmall()
                            .icon(IconName::ArrowLeft)
                            .accessibility_label("Back")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(ConflictEvent::Close))),
                    )
                    .child(div().flex_1().truncate().text_sm().child(self.path.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{done} of {} resolved", found.len())),
                    )
                    .child(
                        Button::new("conflict-save")
                            .small()
                            .primary()
                            .label(if found.is_empty() {
                                "Mark resolved"
                            } else {
                                "Save and mark resolved"
                            })
                            .disabled(done < found.len())
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ),
            )
            .child(
                v_flex()
                    .id("conflicts")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_3()
                    .gap_3()
                    .when_some(self.error.clone(), |this, error| {
                        this.child(div().text_sm().text_color(cx.theme().danger).child(error))
                    })
                    .when(found.is_empty() && self.error.is_none(), |this| {
                        this.child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("No conflict markers left in this file."),
                        )
                    })
                    .children(
                        found
                            .iter()
                            .enumerate()
                            .map(|(ix, conflict)| self.render_conflict(ix, conflict, cx)),
                    ),
            )
    }
}

//! A pull request's Walkthrough tab: the gist (what, why and where the why
//! comes from, before and after), the changed files grouped at a glance
//! (each group filters the Files tab), a short tour that runs inside the
//! Files tab, and, when calls cross files, a small flow PilotHouse draws
//! itself. See [`ph_review::walkthrough`].

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    notification::Notification,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, ClipboardItem, Context, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    div,
};
use ph_github::models::PullRequest;
use ph_review::walkthrough::Walkthrough;
use ph_store::Store;
use ph_store::reviews::PullRef;
use ph_ui::Icons;

use crate::AgentRequest;
use crate::hosting::Hosting;

#[derive(Debug, Clone)]
pub enum WalkthroughEvent {
    Agent(AgentRequest),
    ShowLine {
        path: String,
        line: Option<u64>,
    },
    /// Show only these files in the Files tab.
    Filter(Option<Vec<String>>),
}

pub struct WalkthroughView {
    store: Store,
    hosting: Hosting,
    number: u64,
    pull: Option<PullRequest>,
    /// The newest walkthrough, checked against the changed files, and the
    /// commit it read.
    walkthrough: Option<(Walkthrough, String)>,
    /// A walkthrough was asked for and is being written.
    waiting: bool,
}

impl EventEmitter<WalkthroughEvent> for WalkthroughView {}

impl WalkthroughView {
    pub fn new(store: Store, hosting: Hosting, number: u64, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            store,
            hosting,
            number,
            pull: None,
            walkthrough: None,
            waiting: false,
        };
        view.reload(&[], cx);
        view
    }

    /// The pull request as last loaded, and its changed files (stops and
    /// groups outside them go).
    pub fn set_pull(&mut self, pull: PullRequest, changed: &[String], cx: &mut Context<Self>) {
        self.pull = Some(pull);
        self.reload(changed, cx);
    }

    pub fn reload(&mut self, changed: &[String], cx: &mut Context<Self>) {
        let pull = PullRef {
            host: self.hosting.account.host.clone(),
            owner: self.hosting.repo.owner.clone(),
            repo: self.hosting.repo.name.clone(),
            number: self.number,
        };
        let run = self
            .store
            .latest_pr_run(&pull, "walkthrough")
            .ok()
            .flatten();
        // A turn that failed leaves its run unfinished: after a while it is
        // no longer waited for.
        self.waiting = run.as_ref().is_some_and(|run| {
            !run.finished && ph_store::now_millis() - run.created_at < 20 * 60 * 1000
        });
        self.walkthrough = run
            .filter(|run| run.finished)
            .and_then(|run| {
                let walkthrough: Walkthrough = serde_json::from_str(&run.data).ok()?;
                Some((walkthrough, run.head_sha))
            })
            .map(|(mut walkthrough, sha)| {
                if !changed.is_empty() {
                    walkthrough.check(changed, &|_| None);
                }
                (walkthrough, sha)
            });
        cx.notify();
    }

    /// The tour's stops, for the Files tab.
    pub fn tour(&self) -> Vec<ph_review::walkthrough::Stop> {
        self.walkthrough
            .as_ref()
            .map(|(walkthrough, _)| walkthrough.tour.clone())
            .unwrap_or_default()
    }

    fn ask(&mut self, cx: &mut Context<Self>) {
        if let Some(pull) = &self.pull {
            self.waiting = true;
            cx.emit(WalkthroughEvent::Agent(AgentRequest::Walkthrough {
                pull: Box::new(pull.clone()),
            }));
            cx.notify();
        }
    }

    fn render_gist(&self, walkthrough: &Walkthrough, cx: &mut Context<Self>) -> AnyElement {
        let gist = &walkthrough.gist;
        let from = match gist.why_from.as_str() {
            "description" => "from the description",
            "commits" => "from the commits",
            _ => "a guess from the code",
        };
        let label = |text: &'static str| {
            div()
                .flex_none()
                .w_16()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(text)
        };
        v_flex()
            .gap_1p5()
            .child(div().text_base().child(gist.what.clone()))
            .when(!gist.why.is_empty(), |this| {
                this.child(
                    h_flex().items_start().gap_2().child(label("Why")).child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .child(gist.why.clone())
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(if gist.why_from == "guess" {
                                        cx.theme().warning
                                    } else {
                                        cx.theme().muted_foreground
                                    })
                                    .child(from),
                            ),
                    ),
                )
            })
            .when(!gist.before.is_empty(), |this| {
                this.child(
                    h_flex().items_start().gap_2().child(label("Before")).child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .child(gist.before.clone()),
                    ),
                )
            })
            .when(!gist.after.is_empty(), |this| {
                this.child(
                    h_flex()
                        .items_start()
                        .gap_2()
                        .child(label("After"))
                        .child(div().flex_1().min_w_0().text_sm().child(gist.after.clone())),
                )
            })
            .into_any_element()
    }

    fn render_groups(&self, walkthrough: &Walkthrough, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .gap_1()
            .child(heading("At a glance", cx))
            .child(h_flex().gap_1().flex_wrap().children(
                walkthrough.groups.iter().enumerate().map(|(ix, group)| {
                    let files = group.files.clone();
                    let color = match group.kind.as_str() {
                        "core" | "api" => cx.theme().info,
                        "tests" => cx.theme().success,
                        "ui" => cx.theme().warning,
                        _ => cx.theme().muted_foreground,
                    };
                    div()
                        .id(("walk-group", ix))
                        .role(gpui_kit::accesskit::Role::Button)
                        .aria_label(SharedString::from(format!("Show {}", group.name)))
                        .px_2()
                        .py_0p5()
                        .rounded_full()
                        .text_xs()
                        .cursor_pointer()
                        .bg(color.opacity(0.14))
                        .text_color(color)
                        .hover(|style| style.bg(color.opacity(0.24)))
                        .tooltip({
                            let tip: SharedString = files.join("\n").into();
                            move |window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(tip.clone())
                                    .build(window, cx)
                            }
                        })
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(WalkthroughEvent::Filter(Some(files.clone())))
                        }))
                        .child(format!("{} · {}", group.name, group.files.len()))
                }),
            ))
            .into_any_element()
    }

    fn render_tour(&self, walkthrough: &Walkthrough, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .gap_1()
            .child(heading("Read in this order", cx))
            .children(walkthrough.tour.iter().enumerate().map(|(ix, stop)| {
                let (path, line) = (stop.path.clone(), Some(stop.start));
                h_flex()
                    .id(("walk-stop", ix))
                    .role(gpui_kit::accesskit::Role::Button)
                    .aria_label(SharedString::from(format!(
                        "Stop {}: {}",
                        ix + 1,
                        stop.title
                    )))
                    .items_start()
                    .gap_2()
                    .p_1p5()
                    .rounded(cx.theme().radius)
                    .cursor_pointer()
                    .hover(|style| style.bg(cx.theme().list_hover))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(WalkthroughEvent::ShowLine {
                            path: path.clone(),
                            line,
                        })
                    }))
                    .child(
                        div()
                            .flex_none()
                            .size_5()
                            .rounded_full()
                            .bg(cx.theme().primary.opacity(0.16))
                            .text_color(cx.theme().primary)
                            .text_xs()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child((ix + 1).to_string()),
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
                                            .text_sm()
                                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                                            .child(stop.title.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().link)
                                            .truncate()
                                            .child(format!(
                                                "{}:{}-{}",
                                                stop.path, stop.start, stop.end
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(stop.note.clone()),
                            ),
                    )
            }))
            .into_any_element()
    }

    /// The flow, drawn as boxes and arrows in rows (from → to).
    fn render_flow(&self, walkthrough: &Walkthrough, cx: &mut Context<Self>) -> AnyElement {
        let node = |text: String| {
            div()
                .flex_none()
                .max_w_48()
                .truncate()
                .px_2()
                .py_1()
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().secondary)
                .text_xs()
                .child(text)
        };
        v_flex()
            .gap_1()
            .child(heading("Flow", cx))
            .children(walkthrough.flow.iter().map(|step| {
                h_flex()
                    .gap_2()
                    .child(node(step.from.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(if step.label.is_empty() {
                                "→".to_string()
                            } else {
                                format!("— {} →", step.label)
                            }),
                    )
                    .child(node(step.to.clone()))
            }))
            .into_any_element()
    }
}

impl Render for WalkthroughView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some((walkthrough, sha)) = self.walkthrough.clone() else {
            return v_flex()
                .p_3()
                .gap_2()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(if self.waiting {
                    "The agent is writing the walkthrough (in the chat beside)…"
                } else {
                    "A short explanation of the change: what and why, the files at a glance, and a tour in reading order."
                })
                .when(!self.waiting, |this| {
                    this.child(
                        h_flex().child(
                            Button::new("walk-ask")
                                .small()
                                .primary()
                                .icon(Icons::Sparkles)
                                .label("Walk me through it")
                                .disabled(self.pull.is_none())
                                .on_click(cx.listener(|this, _, _, cx| this.ask(cx))),
                        ),
                    )
                })
                .into_any_element();
        };
        let head = self.pull.as_ref().map(|pull| pull.head.sha.clone());
        let stale = head.is_some_and(|head| head != sha);
        v_flex()
            .p_3()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("At {}", &sha[..sha.len().min(8)]))
                    .when(stale, |this| {
                        this.child(
                            div()
                                .text_color(cx.theme().warning)
                                .child("new commits since"),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        Button::new("walk-copy")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Copy)
                            .label("Copy")
                            .tooltip("Copy as Markdown (for the description or a message)")
                            .on_click(cx.listener({
                                let markdown = walkthrough.markdown();
                                move |_, _, window, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        markdown.clone(),
                                    ));
                                    window.push_notification(
                                        Notification::success("Copied the walkthrough"),
                                        cx,
                                    );
                                }
                            })),
                    )
                    .child(
                        Button::new("walk-again")
                            .xsmall()
                            .ghost()
                            .icon(IconName::RefreshCw)
                            .label("Again")
                            .disabled(self.waiting)
                            .on_click(cx.listener(|this, _, _, cx| this.ask(cx))),
                    ),
            )
            .child(self.render_gist(&walkthrough, cx))
            .when(!walkthrough.groups.is_empty(), |this| {
                this.child(self.render_groups(&walkthrough, cx))
            })
            .when(!walkthrough.tour.is_empty(), |this| {
                this.child(self.render_tour(&walkthrough, cx))
            })
            .when(!walkthrough.flow.is_empty(), |this| {
                this.child(self.render_flow(&walkthrough, cx))
            })
            .into_any_element()
    }
}

fn heading(text: &'static str, cx: &Context<WalkthroughView>) -> impl IntoElement {
    div()
        .text_xs()
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
        .child(SharedString::from(text))
}

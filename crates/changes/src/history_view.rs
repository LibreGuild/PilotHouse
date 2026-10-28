//! History: commits with a branch graph, and one commit's changes.

use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, Context, Entity, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, PathBuilder, Pixels, Render, StatefulInteractiveElement as _,
    Styled as _, Task, Window, canvas, div, point, px,
};
use ph_git::FileChange;
use ph_git::log::{self, Commit, GraphRow};

use crate::branch_picker::ago;
use crate::commit_box::{Writer, clean_message, cut};
use crate::repo::Repo;
use ph_preview::diff as diff_view;

/// Commits read per page.
const PAGE: usize = 200;
const ROW_HEIGHT: f32 = 44.;
const LANE_WIDTH: f32 = 14.;

pub struct HistoryView {
    repo: Entity<Repo>,
    commits: Vec<Commit>,
    rows: Vec<GraphRow>,
    /// Include every branch and tag, not only HEAD's history.
    all: bool,
    more: bool,
    selected: Option<usize>,
    detail: Option<(String, Vec<FileChange>)>,
    loaded: bool,
    writer: Option<Writer>,
    writing: bool,
    _load: Option<Task<()>>,
    _write: Option<Task<()>>,
    _detail: Option<Task<()>>,
}

impl HistoryView {
    pub fn new(repo: Entity<Repo>, cx: &mut Context<Self>) -> Self {
        // New commits, switches and fetches change the history.
        cx.observe(&repo, |this, _, cx| {
            if this.loaded {
                this.reload(cx)
            }
        })
        .detach();
        let mut view = Self {
            repo,
            commits: Vec::new(),
            rows: Vec::new(),
            all: true,
            more: false,
            selected: None,
            detail: None,
            loaded: false,
            writer: None,
            writing: false,
            _load: None,
            _write: None,
            _detail: None,
        };
        view.reload(cx);
        view
    }

    pub fn set_writer(&mut self, writer: Option<Writer>) {
        self.writer = writer;
    }

    /// Release notes for the commits since the last tag, written by an agent.
    fn write_changelog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(writer) = self.writer.clone() else {
            return;
        };
        let root = self.repo.read(cx).root().to_path_buf();
        self.writing = true;
        cx.notify();
        let collect = cx.background_spawn({
            let root = root.clone();
            async move {
                let tag = log::last_tag(&root);
                let range = match &tag {
                    Some(tag) => format!("{tag}..HEAD"),
                    None => "HEAD".to_string(),
                };
                let commits = log::summaries(&root, &range)?;
                anyhow::Ok((tag, commits))
            }
        });
        self._write = Some(cx.spawn_in(window, async move |this, cx| {
            let result = match collect.await {
                Ok((tag, commits)) if !commits.trim().is_empty() => {
                    let since = tag.map(|tag| format!(" since {tag}")).unwrap_or_default();
                    let prompt = format!(
                        "Write release notes (Markdown) for the changes{since} from the \
                         commit list below. Group them under Features, Fixes and Other \
                         as fits, one short user-facing line each, merging duplicates. \
                         Reply with the notes only and do not use tools.\n\n{}",
                        cut(&commits, 16_000)
                    );
                    match this.update(cx, |_, cx| writer(prompt, root, cx)) {
                        Ok(task) => task.await,
                        Err(_) => return,
                    }
                }
                Ok(_) => Err(anyhow::anyhow!(
                    "There are no new commits since the last tag."
                )),
                Err(err) => Err(err),
            };
            _ = this.update_in(cx, |this, window, cx| {
                this.writing = false;
                match result {
                    Ok(notes) => {
                        crate::text_dialog::open("Changelog", clean_message(&notes), window, cx)
                    }
                    Err(err) => window.push_notification(
                        gpui_kit::component::notification::Notification::error(format!("{err:#}")),
                        cx,
                    ),
                }
                cx.notify();
            });
        }));
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.load(0, cx);
    }

    fn load(&mut self, skip: usize, cx: &mut Context<Self>) {
        let root = self.repo.read(cx).root().to_path_buf();
        let all = self.all;
        let read = cx.background_spawn(async move { log::log(&root, PAGE, skip, all) });
        self._load = Some(cx.spawn(async move |this, cx| {
            let page = read.await.unwrap_or_default();
            _ = this.update(cx, |this, cx| {
                this.loaded = true;
                this.more = page.len() == PAGE;
                if skip == 0 {
                    let unchanged = this.commits.first().map(|c| &c.id)
                        == page.first().map(|c| &c.id)
                        && this.commits.len() <= page.len();
                    if unchanged && !this.commits.is_empty() {
                        return;
                    }
                    this.commits = page;
                    this.selected = None;
                    this.detail = None;
                } else {
                    this.commits.extend(page);
                }
                this.rows = log::graph(&this.commits);
                cx.notify();
            });
        }));
    }

    fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(commit) = self.commits.get(ix).cloned() else {
            return;
        };
        self.selected = Some(ix);
        self.detail = None;
        let root = self.repo.read(cx).root().to_path_buf();
        let read = cx.background_spawn(async move { log::changes(&root, &commit) });
        self._detail = Some(cx.spawn(async move |this, cx| {
            let files = read.await.unwrap_or_default();
            _ = this.update(cx, |this, cx| {
                if this.selected == Some(ix) {
                    let id = this
                        .commits
                        .get(ix)
                        .map(|c| c.id.clone())
                        .unwrap_or_default();
                    this.detail = Some((id, files));
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    fn render_row(&self, ix: usize, lanes: usize, cx: &mut Context<Self>) -> AnyElement {
        let commit = &self.commits[ix];
        let row = self.rows.get(ix).cloned().unwrap_or_default();
        // Every row is as wide as the widest, so subjects line up.
        let width = px(LANE_WIDTH * lanes.max(1) as f32 + 6.);
        let selected = self.selected == Some(ix);
        h_flex()
            .id(("commit", ix))
            .h(px(ROW_HEIGHT))
            .pr_2()
            .gap_1()
            .cursor_pointer()
            .when(selected, |this| this.bg(cx.theme().list_active))
            .when(!selected, |this| {
                this.hover(|style| style.bg(cx.theme().list_hover))
            })
            .on_click(cx.listener(move |this, _, _, cx| this.select(ix, cx)))
            .child(graph_cell(row, width, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        h_flex()
                            .gap_1()
                            .min_w_0()
                            .children(commit.refs.iter().map(|name| ref_badge(name, cx)))
                            .child(div().text_sm().truncate().child(commit.subject.clone())),
                    )
                    .child(
                        div()
                            .text_xs()
                            .truncate()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "{} · {} · {}",
                                commit.short(),
                                commit.author,
                                ago(commit.time)
                            )),
                    ),
            )
            .into_any_element()
    }

    fn render_detail(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let commit = self.commits.get(self.selected?)?;
        let files = self
            .detail
            .as_ref()
            .filter(|(id, _)| *id == commit.id)
            .map(|(_, files)| files.clone());
        Some(
            v_flex()
                .id("commit-detail")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .p_3()
                .gap_2()
                .border_t_1()
                .border_color(cx.theme().border)
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .text_sm()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .child(commit.subject.clone()),
                        )
                        .child(
                            Button::new("close-commit")
                                .ghost()
                                .xsmall()
                                .icon(IconName::Close)
                                .accessibility_label("Close commit")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.selected = None;
                                    cx.notify();
                                })),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!(
                            "{} · {} <{}> · {}",
                            commit.id,
                            commit.author,
                            commit.email,
                            ago(commit.time)
                        )),
                )
                .map(|this| match files {
                    None => this.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Loading changes…"),
                    ),
                    Some(files) if files.is_empty() => {
                        this.child(div().text_xs().child("No file changes."))
                    }
                    Some(files) => {
                        this.children(files.iter().map(|file| diff_view::file_diff(file, cx)))
                    }
                })
                .into_any_element(),
        )
    }
}

impl Render for HistoryView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let detail = self.render_detail(cx);
        let list = v_flex()
            .id("history-list")
            .when(detail.is_some(), |this| this.h(px(ROW_HEIGHT * 7.)))
            .when(detail.is_none(), |this| this.flex_1())
            .min_h_0()
            .overflow_y_scroll()
            .children(
                {
                    let lanes = self
                        .rows
                        .iter()
                        .map(|row| row.width)
                        .max()
                        .unwrap_or(1)
                        .min(12);
                    (0..self.commits.len())
                        .map(move |ix| (ix, lanes))
                        .collect::<Vec<_>>()
                }
                .into_iter()
                .map(|(ix, lanes)| self.render_row(ix, lanes, cx)),
            )
            .when(self.more, |this| {
                this.child(
                    div().p_2().child(
                        Button::new("history-more")
                            .small()
                            .ghost()
                            .label("Load older commits")
                            .on_click(cx.listener(|this, _, _, cx| {
                                let skip = this.commits.len();
                                this.load(skip, cx)
                            })),
                    ),
                )
            })
            .when(self.loaded && self.commits.is_empty(), |this| {
                this.child(
                    div()
                        .p_3()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("No commits yet."),
                )
            });
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .px_3()
                    .py_1()
                    .gap_2()
                    .child(
                        Checkbox::new("history-all")
                            .label("All branches")
                            .checked(self.all)
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.all = *checked;
                                this.commits.clear();
                                this.reload(cx);
                            })),
                    )
                    .child(div().flex_1())
                    .when(self.writer.is_some(), |this| {
                        this.child(
                            Button::new("write-changelog")
                                .xsmall()
                                .ghost()
                                .icon(ph_ui::Icons::Sparkles)
                                .label(if self.writing { "Writing…" } else { "Write changelog" })
                                .tooltip("An agent writes release notes from the commits since the last tag")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    if !this.writing {
                                        this.write_changelog(window, cx)
                                    }
                                })),
                        )
                    }),
            )
            .child(list)
            .children(detail)
    }
}

/// `HEAD -> main`, `origin/main`, `tag: v1` as small badges.
fn ref_badge(name: &str, cx: &App) -> AnyElement {
    let (text, color) = if let Some(branch) = name.strip_prefix("HEAD -> ") {
        (branch.to_string(), cx.theme().info)
    } else if let Some(tag) = name.strip_prefix("tag: ") {
        (tag.to_string(), cx.theme().warning)
    } else {
        (name.to_string(), cx.theme().muted_foreground)
    };
    div()
        .flex_none()
        .px_1p5()
        .rounded(cx.theme().radius)
        .border_1()
        .border_color(color.opacity(0.5))
        .text_color(color)
        .text_xs()
        .child(text)
        .into_any_element()
}

/// Lane colors, cycled: fixed hues that stay apart on light and dark.
fn lane_color(lane: usize) -> Hsla {
    const COLORS: [u32; 6] = [0x5b9cf0, 0xe0956f, 0x8fc27a, 0xb48ee0, 0xe5c15c, 0x5cc4c4];
    gpui_kit::rgb(COLORS[lane % COLORS.len()]).into()
}

/// One row of the graph: lines for passing and joining lanes, the dot.
fn graph_cell(row: GraphRow, width: Pixels, cx: &App) -> AnyElement {
    let colors: Vec<Hsla> = (0..row.width.max(1) + 1).map(lane_color).collect();
    let background = cx.theme().background;
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, _| {
            let x =
                |lane: usize| bounds.origin.x + px(LANE_WIDTH * lane as f32 + LANE_WIDTH / 2. + 3.);
            let (top, bottom) = (bounds.origin.y, bounds.origin.y + bounds.size.height);
            let middle = bounds.origin.y + bounds.size.height / 2.;
            let line = |from: gpui_kit::Point<Pixels>,
                        to: gpui_kit::Point<Pixels>,
                        color: Hsla,
                        window: &mut Window| {
                let mut path = PathBuilder::stroke(px(1.6));
                path.move_to(from);
                path.line_to(to);
                if let Ok(path) = path.build() {
                    window.paint_path(path, color);
                }
            };
            for &lane in &row.passing {
                line(
                    point(x(lane), top),
                    point(x(lane), bottom),
                    colors[lane % colors.len()],
                    window,
                );
            }
            if row.from_above {
                line(
                    point(x(row.lane), top),
                    point(x(row.lane), middle),
                    colors[row.lane % colors.len()],
                    window,
                );
            }
            for &lane in &row.merging_in {
                line(
                    point(x(lane), top),
                    point(x(row.lane), middle),
                    colors[lane % colors.len()],
                    window,
                );
            }
            for &lane in &row.to_parents {
                line(
                    point(x(row.lane), middle),
                    point(x(lane), bottom),
                    colors[lane % colors.len()],
                    window,
                );
            }
            // The dot, ringed in the background color so lines stop short of it.
            let center = point(x(row.lane), middle);
            let dot = |radius: f32, color: Hsla, window: &mut Window| {
                let r = px(radius);
                window.paint_quad(
                    gpui_kit::fill(
                        Bounds::new(
                            point(center.x - r, center.y - r),
                            gpui_kit::size(r * 2., r * 2.),
                        ),
                        color,
                    )
                    .corner_radii(r),
                );
            };
            dot(5.5, background, window);
            dot(4., colors[row.lane % colors.len()], window);
        },
    )
    .w(width)
    .h_full()
    .flex_none()
    .into_any_element()
}

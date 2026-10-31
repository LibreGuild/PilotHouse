//! The editor's side panel: the slide's comments (with their markers on
//! the slide), and the presentation's references, files to work from that
//! the agent reads and whose pictures, pages, text and tables go into the
//! slides.

use std::path::{Path, PathBuf};

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::PopupMenuItem,
    notification::Notification,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Bounds, Context, Entity, InteractiveElement as _, IntoElement,
    MouseButton, ParentElement as _, Pixels, StatefulInteractiveElement as _, Styled as _, Window,
    div, px,
};
use ph_ui::{Icons, MenuButton as _};
use serde_json::json;

use super::canvas::Placement;
use super::{SlidesEditor, SlidesEditorEvent};
use crate::slides::model::Comment;
use crate::slides::ops::Op;
use crate::slides::space::{REFERENCES, Space};

/// What the side panel shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Comments,
    References,
}

/// A file in the space's `references/` folder.
#[derive(Debug, Clone)]
pub(crate) struct Reference {
    pub path: PathBuf,
    pub name: String,
    pub kind: RefKind,
    /// Pages (PDF) or slides (PowerPoint), when known.
    pub pages: Option<usize>,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefKind {
    Picture,
    Pdf,
    Slides,
    Markdown,
    Table,
    Other,
}

impl RefKind {
    fn of(path: &Path) -> RefKind {
        let extension = path
            .extension()
            .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        match extension.as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" => RefKind::Picture,
            "pdf" => RefKind::Pdf,
            "pptx" | "potx" => RefKind::Slides,
            "md" | "markdown" | "txt" => RefKind::Markdown,
            "csv" | "tsv" => RefKind::Table,
            _ => RefKind::Other,
        }
    }

    fn icon(self) -> Icons {
        match self {
            RefKind::Picture => Icons::Image,
            RefKind::Pdf => Icons::FileText,
            RefKind::Slides => Icons::Presentation,
            RefKind::Markdown => Icons::ScrollText,
            RefKind::Table => Icons::FileSpreadsheet,
            RefKind::Other => Icons::Files,
        }
    }
}

/// The files in `folder`, with their pages counted (off the UI thread).
fn list_references(folder: &Path) -> Vec<Reference> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut found: Vec<Reference> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .map(|entry| {
            let path = entry.path();
            let kind = RefKind::of(&path);
            let pages = match kind {
                RefKind::Pdf => ph_preview::pdf_page_count(&path).ok(),
                RefKind::Slides => slide_count(&path),
                _ => None,
            };
            Reference {
                name: entry.file_name().to_string_lossy().into_owned(),
                bytes: entry.metadata().map(|meta| meta.len()).unwrap_or(0),
                path,
                kind,
                pages,
            }
        })
        .collect();
    found.sort_by_key(|reference| reference.name.to_lowercase());
    found
}

/// How many slides a PowerPoint file has (from its parts' names).
fn slide_count(path: &Path) -> Option<usize> {
    let file = std::fs::File::open(path).ok()?;
    let archive = zip::ZipArchive::new(std::io::BufReader::new(file)).ok()?;
    let count = archive
        .file_names()
        .filter(|name| {
            name.as_ref()
                .ok()
                .and_then(|name| name.strip_prefix("ppt/slides/slide"))
                .and_then(|rest| rest.strip_suffix(".xml"))
                .is_some_and(|number| number.parse::<u32>().is_ok())
        })
        .count();
    Some(count)
}

/// Copies `source` into `folder`, under a name not taken yet.
fn copy_in(source: &Path, folder: &Path) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(folder)?;
    let name = source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem.to_string(), format!(".{extension}")),
        _ => (name.clone(), String::new()),
    };
    let mut target = folder.join(&name);
    let mut n = 2;
    while target.exists() {
        target = folder.join(format!("{stem} {n}{extension}"));
        n += 1;
    }
    std::fs::copy(source, &target)?;
    Ok(target)
}

/// A CSV (or TSV) file's rows, at most `rows` by `columns`.
pub(crate) fn read_csv(text: &str, tabs: bool, rows: usize, columns: usize) -> Vec<Vec<String>> {
    let separator = if tabs { '\t' } else { ',' };
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cell.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            c if c == separator && !quoted => row.push(std::mem::take(&mut cell)),
            '\r' if !quoted => {}
            '\n' if !quoted => {
                row.push(std::mem::take(&mut cell));
                if row.iter().any(|cell| !cell.trim().is_empty()) {
                    out.push(std::mem::take(&mut row));
                } else {
                    row.clear();
                }
                if out.len() >= rows {
                    break;
                }
            }
            c => cell.push(c),
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        if out.len() < rows && row.iter().any(|cell| !cell.trim().is_empty()) {
            out.push(row);
        }
    }
    for row in &mut out {
        row.truncate(columns);
        for cell in row.iter_mut() {
            *cell = cell.trim().to_string();
        }
    }
    out
}

/// "just now", "5 minutes ago", "3 hours ago", else the day.
fn ago(at: u64) -> String {
    let now = crate::slides::ops::now();
    let seconds = now.saturating_sub(at);
    match seconds {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", seconds / 60),
        3600..86_400 => format!("{} h ago", seconds / 3600),
        _ => format!("{} days ago", seconds / 86_400),
    }
}

fn byte_size(bytes: u64) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{:.0} KB", bytes as f64 / 1024.),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.),
    }
}

impl SlidesEditor {
    /// The field comments and replies are typed in (Enter sends).
    pub(crate) fn comment_field(window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Add a comment"));
        cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.send_comment(window, cx);
            }
        })
        .detach();
        input
    }

    pub(crate) fn toggle_side(&mut self, side: Side, window: &mut Window, cx: &mut Context<Self>) {
        self.side = if self.side == Some(side) {
            None
        } else {
            Some(side)
        };
        match self.side {
            Some(Side::References) => self.load_references(cx),
            Some(Side::Comments) => {
                self.comment_input
                    .update(cx, |input, cx| input.focus(window, cx));
            }
            None => {}
        }
        cx.notify();
    }

    /// Lists the references again (off the UI thread).
    pub(crate) fn load_references(&mut self, cx: &mut Context<Self>) {
        let folder = self.doc.read(cx).folder().join(REFERENCES);
        let listed = cx.background_spawn(async move { list_references(&folder) });
        cx.spawn(async move |this, cx| {
            let listed = listed.await;
            _ = this.update(cx, |this, cx| {
                this.references = listed;
                cx.notify();
            });
        })
        .detach();
    }

    /// Copies files into `references/`.
    pub(crate) fn add_references(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let folder = self.doc.read(cx).folder().join(REFERENCES);
        let copied = cx.background_spawn(async move {
            paths
                .iter()
                .filter(|path| path.is_file())
                .map(|path| copy_in(path, &folder))
                .collect::<Vec<_>>()
        });
        cx.spawn_in(window, async move |this, cx| {
            let copied = copied.await;
            _ = this.update_in(cx, |this, window, cx| {
                for failed in copied.iter().filter_map(|copied| copied.as_ref().err()) {
                    window.push_notification(
                        Notification::error(format!("Not added: {failed:#}")),
                        cx,
                    );
                }
                this.side = Some(Side::References);
                this.load_references(cx);
            });
        })
        .detach();
    }

    fn pick_references(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Add".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            _ = this.update_in(cx, |this, window, cx| {
                this.add_references(paths, window, cx)
            });
        })
        .detach();
    }

    /// Puts what `reference` holds on the slide: a picture, a page or
    /// slide as a picture (`page`, from 0), text or a table.
    fn use_reference(
        &mut self,
        reference: Reference,
        page: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let space = self.doc.read(cx).space().clone();
        match reference.kind {
            RefKind::Picture => {
                let path = reference.path.clone();
                let added = cx.background_spawn(async move { space.add_media(&path) });
                cx.spawn_in(window, async move |this, cx| {
                    let added = added.await;
                    _ = this.update_in(cx, |this, window, cx| match added {
                        Ok(name) => this.insert_picture(name, window, cx),
                        Err(err) => {
                            window.push_notification(Notification::error(format!("{err:#}")), cx)
                        }
                    });
                })
                .detach();
            }
            RefKind::Pdf | RefKind::Slides => {
                let path = reference.path.clone();
                let kind = reference.kind;
                let made = cx.background_spawn(async move {
                    let png = match kind {
                        RefKind::Pdf => ph_preview::pdf_page_png(&path, page, 1920)
                            .map_err(anyhow::Error::msg)?,
                        _ => slide_png(&path, page)?,
                    };
                    space.add_media_bytes(&png, "png")
                });
                cx.spawn_in(window, async move |this, cx| {
                    let made = made.await;
                    _ = this.update_in(cx, |this, window, cx| match made {
                        Ok(name) => this.insert_picture(name, window, cx),
                        Err(err) => window.push_notification(
                            Notification::error(format!("Not inserted: {err:#}")),
                            cx,
                        ),
                    });
                })
                .detach();
            }
            RefKind::Markdown => {
                let text = std::fs::read_to_string(&reference.path).unwrap_or_default();
                let text: String = text.chars().take(4000).collect();
                if text.trim().is_empty() {
                    return;
                }
                let size = &self.deck(cx).size;
                let frame = self.centered(size.width * 0.8, size.height * 0.7, cx);
                self.insert(
                    json!({ "type": "text", "text": text.trim(), "frame": frame }),
                    window,
                    cx,
                );
            }
            RefKind::Table => {
                let text = std::fs::read_to_string(&reference.path).unwrap_or_default();
                let tabs = reference.name.to_ascii_lowercase().ends_with(".tsv");
                let rows = read_csv(&text, tabs, 30, 10);
                let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
                if columns == 0 {
                    window.push_notification(Notification::error("That file has no rows."), cx);
                    return;
                }
                let cells: Vec<Vec<serde_json::Value>> = rows
                    .iter()
                    .map(|row| {
                        (0..columns)
                            .map(|column| json!({ "text": row.get(column).cloned().unwrap_or_default() }))
                            .collect()
                    })
                    .collect();
                let width = self.deck(cx).size.width * 0.85;
                let frame = self.centered(width, rows.len() as f32 * 30., cx);
                self.insert(
                    json!({ "type": "table", "columns": vec![1; columns], "rows": cells, "header": true, "banded": true, "frame": frame }),
                    window,
                    cx,
                );
            }
            RefKind::Other if ph_ui::platform::opens_safely(&reference.path) => {
                cx.open_with_system(&reference.path)
            }
            RefKind::Other => cx.reveal_path(&reference.path),
        }
    }

    /// A CSV's numbers as a column chart: the first column names, each
    /// other column a series.
    fn chart_reference(
        &mut self,
        reference: &Reference,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = std::fs::read_to_string(&reference.path).unwrap_or_default();
        let tabs = reference.name.to_ascii_lowercase().ends_with(".tsv");
        let rows = read_csv(&text, tabs, 40, 8);
        let Some((header, body)) = rows.split_first() else {
            return;
        };
        let number = |cell: &str| {
            cell.replace([',', '%', '$', '€', '£'], "")
                .trim()
                .parse::<f64>()
                .ok()
        };
        let categories: Vec<String> = body
            .iter()
            .map(|row| row.first().cloned().unwrap_or_default())
            .collect();
        let series: Vec<serde_json::Value> = (1..header.len())
            .filter(|column| body.iter().any(|row| row.get(*column).and_then(|cell| number(cell)).is_some()))
            .map(|column| {
                json!({
                    "name": header[column],
                    "values": body.iter().map(|row| row.get(column).and_then(|cell| number(cell)).unwrap_or(0.)).collect::<Vec<_>>(),
                })
            })
            .collect();
        if series.is_empty() {
            window.push_notification(Notification::error("No columns of numbers to chart."), cx);
            return;
        }
        let size = &self.deck(cx).size;
        let frame = self.centered(size.width * 0.7, size.height * 0.65, cx);
        self.insert(
            json!({ "type": "chart", "chart": "column", "categories": categories, "series": series, "legend": true, "frame": frame }),
            window,
            cx,
        );
    }

    fn send_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.comment_input.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        let op = match self.replying.take() {
            Some(comment) => Op::ReplyComment {
                comment,
                text,
                author: None,
            },
            None => {
                let Some(slide) = self.slide(cx).map(|slide| slide.id.clone()) else {
                    return;
                };
                Op::AddComment {
                    slide: json!(slide),
                    text,
                    id: (self.selected.len() == 1).then(|| self.selected[0].clone()),
                    x: None,
                    y: None,
                    author: None,
                }
            }
        };
        if self.apply("comment", vec![op], window, cx).is_some() {
            self.comment_input.update(cx, |input, cx| {
                input.set_value("", window, cx);
                input.set_placeholder("Add a comment", window, cx);
            });
        }
    }

    pub(crate) fn render_side(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let side = self.side?;
        let theme = cx.theme().clone();
        let body = match side {
            Side::Comments => self.render_comments(window, cx),
            Side::References => self.render_references(cx),
        };
        let title = match side {
            Side::Comments => "Comments",
            Side::References => "References",
        };
        Some(
            v_flex()
                .id("slides-side")
                .flex_none()
                .w(px(300.))
                .h_full()
                .border_l_1()
                .border_color(theme.border)
                .bg(theme.background)
                .child(
                    h_flex()
                        .flex_none()
                        .h(px(36.))
                        .px_3()
                        .gap_2()
                        .border_b_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .flex_1()
                                .text_sm()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .child(title),
                        )
                        .child(
                            Button::new("side-close")
                                .ghost()
                                .xsmall()
                                .icon(Icons::Close)
                                .tooltip("Close")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.side = None;
                                    cx.notify();
                                })),
                        ),
                )
                .child(body)
                .into_any_element(),
        )
    }

    fn render_comments(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let comments: Vec<Comment> = self
            .slide(cx)
            .map(|slide| slide.comments.clone())
            .unwrap_or_default();
        let elsewhere: Vec<usize> = self
            .deck(cx)
            .slides
            .iter()
            .enumerate()
            .filter(|(index, slide)| {
                *index != self.current && slide.comments.iter().any(|comment| !comment.resolved)
            })
            .map(|(index, _)| index)
            .collect();
        let replying_to = self
            .replying
            .as_ref()
            .and_then(|id| comments.iter().find(|comment| &comment.id == id))
            .map(|comment| comment.author.clone());
        let card = |number: usize, comment: &Comment, cx: &mut Context<Self>| {
            let (reply_id, resolve_id, remove_id) =
                (comment.id.clone(), comment.id.clone(), comment.id.clone());
            let resolved = comment.resolved;
            let replying = self.replying.as_ref() == Some(&comment.id);
            v_flex()
                .id(gpui_kit::ElementId::Name(
                    format!("comment-{}", comment.id).into(),
                ))
                .gap_1()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(if replying { theme.ring } else { theme.border })
                .when(resolved, |this| this.opacity(0.6))
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            div()
                                .flex_none()
                                .size(px(18.))
                                .rounded_full()
                                .bg(theme.primary)
                                .text_color(theme.primary_foreground)
                                .text_xs()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(number.to_string()),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .truncate()
                                .child(comment.author.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(ago(comment.at)),
                        ),
                )
                .child(div().text_sm().child(comment.text.clone()))
                .children(comment.replies.iter().map(|reply| {
                    v_flex()
                        .ml_3()
                        .pl_2()
                        .border_l_2()
                        .border_color(theme.border)
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                        .child(reply.author.clone()),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(ago(reply.at)),
                                ),
                        )
                        .child(div().text_sm().child(reply.text.clone()))
                }))
                .child(
                    h_flex()
                        .gap_1()
                        .child(
                            Button::new(gpui_kit::ElementId::Name(
                                format!("reply-{}", comment.id).into(),
                            ))
                            .ghost()
                            .xsmall()
                            .label("Reply")
                            .selected(replying)
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.replying = if this.replying.as_ref() == Some(&reply_id) {
                                        None
                                    } else {
                                        Some(reply_id.clone())
                                    };
                                    let placeholder = if this.replying.is_some() {
                                        "Reply"
                                    } else {
                                        "Add a comment"
                                    };
                                    this.comment_input.update(cx, |input, cx| {
                                        input.set_placeholder(placeholder, window, cx);
                                        input.focus(window, cx);
                                    });
                                    cx.notify();
                                },
                            )),
                        )
                        .child(
                            Button::new(gpui_kit::ElementId::Name(
                                format!("resolve-{}", comment.id).into(),
                            ))
                            .ghost()
                            .xsmall()
                            .label(if resolved { "Reopen" } else { "Resolve" })
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.apply(
                                        "resolve comment",
                                        vec![Op::ResolveComment {
                                            comment: resolve_id.clone(),
                                            resolved: !resolved,
                                        }],
                                        window,
                                        cx,
                                    );
                                },
                            )),
                        )
                        .child(div().flex_1())
                        .child(
                            Button::new(gpui_kit::ElementId::Name(
                                format!("remove-{}", comment.id).into(),
                            ))
                            .ghost()
                            .xsmall()
                            .icon(Icons::Trash)
                            .tooltip("Delete the comment and its replies")
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    if this.replying.as_ref() == Some(&remove_id) {
                                        this.replying = None;
                                    }
                                    this.apply(
                                        "delete comment",
                                        vec![Op::RemoveComment {
                                            comment: remove_id.clone(),
                                        }],
                                        window,
                                        cx,
                                    );
                                },
                            )),
                        ),
                )
        };
        let open: Vec<AnyElement> = comments
            .iter()
            .enumerate()
            .filter(|(_, comment)| !comment.resolved)
            .map(|(ix, comment)| card(ix + 1, comment, cx).into_any_element())
            .collect();
        let done: Vec<AnyElement> = comments
            .iter()
            .enumerate()
            .filter(|(_, comment)| comment.resolved)
            .map(|(ix, comment)| card(ix + 1, comment, cx).into_any_element())
            .collect();
        let next_elsewhere = elsewhere
            .iter()
            .find(|index| **index > self.current)
            .or(elsewhere.first())
            .copied();
        let about = if self.selected.len() == 1 {
            "About the selected element"
        } else {
            "About this slide"
        };
        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                v_flex()
                    .id("comments-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_3()
                    .gap_2()
                    .when(comments.is_empty(), |this| {
                        this.child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child("No comments on this slide. Select something and write one below to say what it is about."),
                        )
                    })
                    .children(open)
                    .when(!done.is_empty(), |this| {
                        this.child(div().pt_2().text_xs().text_color(theme.muted_foreground).child("Resolved")).children(done)
                    })
                    .when_some(next_elsewhere, |this, index| {
                        this.child(
                            Button::new("comments-elsewhere")
                                .ghost()
                                .small()
                                .label(format!(
                                    "{} other slide{} with comments: go to slide {}",
                                    elsewhere.len(),
                                    if elsewhere.len() == 1 { "" } else { "s" },
                                    index + 1
                                ))
                                .on_click(cx.listener(move |this, _, window, cx| this.go_to(index, window, cx))),
                        )
                    }),
            )
            .child(
                v_flex()
                    .flex_none()
                    .p_3()
                    .gap_2()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        div().text_xs().text_color(theme.muted_foreground).child(match &replying_to {
                            Some(author) => format!("Replying to {author}"),
                            None => about.to_string(),
                        }),
                    )
                    .child(Input::new(&self.comment_input).small())
                    .child(
                        h_flex()
                            .gap_1()
                            .child(div().flex_1())
                            .when(self.replying.is_some(), |this| {
                                this.child(Button::new("comment-cancel").ghost().small().label("Cancel").on_click(cx.listener(
                                    |this, _, window, cx| {
                                        this.replying = None;
                                        this.comment_input.update(cx, |input, cx| input.set_placeholder("Add a comment", window, cx));
                                        cx.notify();
                                    },
                                )))
                            })
                            .child(
                                Button::new("comment-send")
                                    .primary()
                                    .small()
                                    .label(if self.replying.is_some() { "Reply" } else { "Comment" })
                                    .disabled(self.slide(cx).is_none())
                                    .on_click(cx.listener(|this, _, window, cx| this.send_comment(window, cx))),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_references(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let editor = cx.weak_entity();
        let rows: Vec<AnyElement> = self
            .references
            .iter()
            .enumerate()
            .map(|(ix, reference)| {
                let menu_reference = reference.clone();
                let editor = editor.clone();
                let detail = match (reference.kind, reference.pages) {
                    (RefKind::Pdf, Some(pages)) => format!(
                        "{pages} page{} · {}",
                        if pages == 1 { "" } else { "s" },
                        byte_size(reference.bytes)
                    ),
                    (RefKind::Slides, Some(slides)) => format!(
                        "{slides} slide{} · {}",
                        if slides == 1 { "" } else { "s" },
                        byte_size(reference.bytes)
                    ),
                    _ => byte_size(reference.bytes),
                };
                h_flex()
                    .id(gpui_kit::ElementId::Name(format!("reference-{ix}").into()))
                    .gap_2()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .hover(|this| this.bg(theme.muted))
                    .child(
                        gpui_kit::component::Icon::new(reference.kind.icon())
                            .small()
                            .flex_none()
                            .text_color(theme.muted_foreground),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_sm().truncate().child(reference.name.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(detail),
                            ),
                    )
                    .child(
                        Button::new(gpui_kit::ElementId::Name(
                            format!("reference-use-{ix}").into(),
                        ))
                        .ghost()
                        .xsmall()
                        .icon(Icons::Ellipsis)
                        .tooltip("Use it")
                        .menu(
                            gpui_kit::Anchor::TopRight,
                            move |menu, _, _| {
                                let reference = menu_reference.clone();
                                let item = |label: String, page: usize| {
                                    let (editor, reference) = (editor.clone(), reference.clone());
                                    PopupMenuItem::new(label).on_click(move |_, window, cx| {
                                        _ = editor.update(cx, |this, cx| {
                                            this.use_reference(reference.clone(), page, window, cx)
                                        });
                                    })
                                };
                                let mut menu = match reference.kind {
                                    RefKind::Picture => {
                                        menu.item(item("Insert on this slide".into(), 0))
                                    }
                                    RefKind::Pdf | RefKind::Slides => {
                                        let what = if reference.kind == RefKind::Pdf {
                                            "page"
                                        } else {
                                            "slide"
                                        };
                                        let count = reference.pages.unwrap_or(1).clamp(1, 40);
                                        (0..count).fold(
                                            menu.label(format!("Insert a {what} as a picture")),
                                            |menu, page| {
                                                menu.item(item(
                                                    format!(
                                                        "{} {}",
                                                        if what == "page" {
                                                            "Page"
                                                        } else {
                                                            "Slide"
                                                        },
                                                        page + 1
                                                    ),
                                                    page,
                                                ))
                                            },
                                        )
                                    }
                                    RefKind::Markdown => {
                                        menu.item(item("Insert its text".into(), 0))
                                    }
                                    RefKind::Table => {
                                        let (chart_editor, chart_reference) =
                                            (editor.clone(), reference.clone());
                                        menu.item(item("Insert as a table".into(), 0)).item(
                                            PopupMenuItem::new("Insert as a chart").on_click(
                                                move |_, window, cx| {
                                                    _ = chart_editor.update(cx, |this, cx| {
                                                        this.chart_reference(
                                                            &chart_reference,
                                                            window,
                                                            cx,
                                                        )
                                                    });
                                                },
                                            ),
                                        )
                                    }
                                    RefKind::Other => menu,
                                };
                                let (ask, open, name, path) = (
                                    editor.clone(),
                                    reference.path.clone(),
                                    reference.name.clone(),
                                    reference.path.clone(),
                                );
                                menu = menu
                                    .separator()
                                    .item(PopupMenuItem::new("Ask the agent to use it").on_click(
                                        move |_, _, cx| {
                                            _ = ask.update(cx, |_, cx| {
                                                cx.emit(SlidesEditorEvent::Ask(format!(
                                                    "Using references/{name}, "
                                                )));
                                            });
                                        },
                                    ))
                                    // Opened only when it is a document, never a
                                    // program (references come from anywhere).
                                    .when(ph_ui::platform::opens_safely(&open), |menu| {
                                        menu.item(
                                            PopupMenuItem::new("Open").on_click(move |_, _, cx| {
                                                cx.open_with_system(&open)
                                            }),
                                        )
                                    })
                                    .item(
                                        PopupMenuItem::new(super::reveal_label())
                                            .on_click(move |_, _, cx| cx.reveal_path(&path)),
                                    );
                                menu
                            },
                        ),
                    )
                    .into_any_element()
            })
            .collect();
        v_flex()
            .id("references")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_2()
            .gap_1()
            .on_drop(cx.listener(|this, paths: &gpui_kit::ExternalPaths, window, cx| {
                this.add_references(paths.paths().to_vec(), window, cx);
            }))
            .child(
                div()
                    .px_1()
                    .pb_2()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("Files to work from: the agent reads them, and their pictures, pages, text and tables go on slides. Drop files here."),
            )
            .children(rows)
            .child(
                h_flex().pt_2().child(
                    Button::new("references-add")
                        .outline()
                        .small()
                        .icon(Icons::Plus)
                        .label("Add files…")
                        .on_click(cx.listener(|this, _, window, cx| this.pick_references(window, cx))),
                ),
            )
            .into_any_element()
    }

    /// The markers of the slide's open comments, numbered as in the panel.
    pub(crate) fn render_pins(
        &self,
        placement: &Placement,
        bounds: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        let Some(slide) = self.slide(cx) else {
            return Vec::new();
        };
        slide
            .comments
            .iter()
            .enumerate()
            .filter(|(_, comment)| !comment.resolved)
            .map(|(ix, comment)| {
                // On the element it is about, where that is now.
                let spot = comment
                    .element
                    .as_ref()
                    .and_then(|id| slide.elements.iter().find(|element| &element.id == id))
                    .map(|element| {
                        let [_, top, right, _] =
                            crate::slides::editor::geometry::bounds(&element.frame);
                        [right, top]
                    })
                    .unwrap_or([comment.x, comment.y]);
                let at = placement.to_screen(spot) - bounds.origin;
                let id = comment.id.clone();
                div()
                    .id(gpui_kit::ElementId::Name(
                        format!("pin-{}", comment.id).into(),
                    ))
                    .absolute()
                    .left(at.x - px(10.))
                    .top(at.y - px(10.))
                    .size(px(20.))
                    .rounded_full()
                    .border_2()
                    .border_color(theme.background)
                    .bg(theme.primary)
                    .text_color(theme.primary_foreground)
                    .text_xs()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .shadow_sm()
                    .child((ix + 1).to_string())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.side = Some(Side::Comments);
                            this.replying = Some(id.clone());
                            this.comment_input.update(cx, |input, cx| {
                                input.set_placeholder("Reply", window, cx);
                                input.focus(window, cx);
                            });
                            cx.notify();
                        }),
                    )
                    .into_any_element()
            })
            .collect()
    }
}

/// Slide `index` of the PowerPoint file at `path`, as a PNG 1920 wide.
fn slide_png(path: &Path, index: usize) -> anyhow::Result<Vec<u8>> {
    // A folder of its own per call: two inserts at once never share one.
    static CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let call = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let scratch = std::env::temp_dir().join(format!(
        "pilothouse-reference-{}-{call}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch)?;
    let drawn = (|| {
        let (space, _) = crate::slides::space::import_pptx(&scratch, path)?;
        let (_, deck) = Space::open(&space.path)?;
        anyhow::ensure!(index < deck.slides.len(), "there is no slide {}", index + 1);
        let image = crate::slides::raster::slide(&deck, index, 1920, Some(&space.media()), false)?;
        let mut png = Vec::new();
        image.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
        Ok(png)
    })();
    let _ = std::fs::remove_dir_all(&scratch);
    drawn
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_cells_with_quotes() {
        let rows = read_csv(
            "name,\"a, b\",c\n\"say \"\"hi\"\"\",2,3\n\n4,5",
            false,
            10,
            10,
        );
        assert_eq!(
            rows,
            vec![
                vec!["name", "a, b", "c"],
                vec!["say \"hi\"", "2", "3"],
                vec!["4", "5"]
            ]
        );
    }
}

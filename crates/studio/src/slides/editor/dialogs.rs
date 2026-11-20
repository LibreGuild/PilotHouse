//! The editor's dialogs: a table's cells and a chart's data typed as rows,
//! the icon library, SVG, export options, the theme's colours and fonts,
//! saving a theme, a custom size, sections, renaming and earlier versions.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::DialogFooter,
    h_flex,
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    notification::Notification,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, Image, ImageFormat, InteractiveElement as _,
    IntoElement, ObjectFit, ParentElement as _, SharedString, StatefulInteractiveElement as _,
    Styled as _, StyledImage as _, Window, div, img, px,
};
use ph_ui::Icons;
use serde_json::{Value, json};

use super::SlidesEditor;
use crate::slides::document::ExportFormat;
use crate::slides::model::*;
use crate::slides::ops::Op;

/// A footer with Cancel and one primary button.
fn footer(
    id: &'static str,
    label: &'static str,
    run: impl Fn(&mut Window, &mut App) + 'static,
) -> DialogFooter {
    DialogFooter::new()
        .w_full()
        .child(div().flex_1())
        .child(
            Button::new(SharedString::from(format!("{id}-cancel")))
                .ghost()
                .label("Cancel")
                .on_click(|_, window, cx| window.close_dialog(cx)),
        )
        .child(
            Button::new(id)
                .primary()
                .label(label)
                .on_click(move |_, window, cx| {
                    window.close_dialog(cx);
                    run(window, cx);
                }),
        )
}

fn hint(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
        .into_any_element()
}

/// Rows of cells as typed: a line a row, cells apart by `|` or tabs.
fn rows_of(text: &str) -> Vec<Vec<String>> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            // A tab is a cell's end also where the line starts: an empty
            // first cell pasted from a spreadsheet stays a cell.
            if line.contains('\t') {
                let cells = line.trim_end().split('\t');
                return cells.map(|cell| cell.trim().to_string()).collect();
            }
            let line = line.trim();
            // A Markdown row (`| a | b |`) loses one bar at each end, and
            // only when it has both: ` | a | b` starts with an empty cell.
            let inner = line
                .strip_prefix('|')
                .and_then(|rest| rest.strip_suffix('|'))
                .filter(|rest| !rest.ends_with('\\'));
            let cells = split_bars(inner.unwrap_or(line));
            if inner.is_none() && cells.len() == 1 {
                return line
                    .split(',')
                    .map(|cell| cell.trim().to_string())
                    .collect();
            }
            cells
        })
        .collect()
}

/// The cells of a row written with bars; `\|` is a bar in a cell.
fn split_bars(line: &str) -> Vec<String> {
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                chars.next();
                cell.push('|');
            }
            '|' => cells.push(std::mem::take(&mut cell).trim().to_string()),
            c => cell.push(c),
        }
    }
    cells.push(cell.trim().to_string());
    cells
}

/// A cell's text as a row shows it: on one line.
fn cell_shown(text: &str) -> String {
    text.replace(['\n', '\t'], " ").trim().to_string()
}

/// A row as [`rows_of`] reads it back: cells apart by ` | `, a bar in a
/// cell as `\|`. It is wrapped in bars where it would otherwise read as
/// something else (one cell alone, or an empty cell at both ends).
fn row_text(cells: &[String]) -> String {
    let row = cells
        .iter()
        .map(|cell| cell_shown(cell).replace('|', "\\|"))
        .collect::<Vec<_>>()
        .join(" | ");
    let ends_empty = cells.first().is_none_or(|cell| cell.trim().is_empty())
        && cells.last().is_none_or(|cell| cell.trim().is_empty());
    if cells.len() < 2 || ends_empty {
        format!("| {row} |")
    } else {
        row
    }
}

/// A cell's text typed anew, as plain text (`# of seats` is what it says)
/// in the look its text began with.
fn retyped(old: &TextBody, typed: &str) -> TextBody {
    let was = old.paragraphs.first().cloned().unwrap_or_default();
    let run = Run {
        text: typed.to_string(),
        link: None,
        ..was.runs.first().cloned().unwrap_or_default()
    };
    TextBody {
        paragraphs: vec![Paragraph {
            runs: vec![run],
            ..was
        }],
        ..old.clone()
    }
}

impl SlidesEditor {
    fn textarea(
        &self,
        text: String,
        rows: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextareaState> {
        cx.new(|cx| {
            TextareaState::new(window, cx)
                .default_value(text)
                .auto_grow(rows, rows + 10)
        })
    }

    fn input(
        &self,
        text: String,
        placeholder: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .default_value(text)
        })
    }

    /// A table's cells as rows of text.
    pub(crate) fn open_table(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some((
            _,
            Element {
                kind: Kind::Table(table),
                ..
            },
        )) = self.deck(cx).element(&id).map(|(i, e)| (i, e.clone()))
        else {
            return;
        };
        let text = table
            .rows
            .iter()
            .map(|row| row_text(&row.iter().map(|cell| cell.text.plain()).collect::<Vec<_>>()))
            .collect::<Vec<_>>()
            .join("\n");
        let input = self.textarea(text, 6, window, cx);
        let editor = cx.entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let (input, editor, table, id) = (input.clone(), editor.clone(), table.clone(), id.clone());
            dialog
                .title("Table")
                .w(ph_ui::scaled_px(560., window))
                .child(
                    v_flex()
                        .gap_2()
                        .child(hint("A line a row, cells apart by | (or tabs, as pasted from a spreadsheet). The first row is the header.", cx))
                        .child(Textarea::new(&input).font_family(cx.theme().mono_font_family.clone())),
                )
                .footer(footer("table-save", "Save", move |window, cx| {
                    let rows = rows_of(input.read(cx).value().as_ref());
                    let columns = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
                    let cells: Vec<Vec<Value>> = rows
                        .iter()
                        .enumerate()
                        .map(|(r, row)| {
                            (0..columns)
                                .map(|c| {
                                    // Cells kept where they were keep their
                                    // fill; one whose text was not touched
                                    // keeps all of it (styles, lines, links).
                                    let old = table.rows.get(r).and_then(|row| row.get(c));
                                    let typed = row.get(c).map(String::as_str).unwrap_or_default();
                                    let mut cell = old
                                        .map(|cell| serde_json::to_value(cell).unwrap_or_default())
                                        .unwrap_or_else(|| json!({}));
                                    let was = old.map(|cell| &cell.text);
                                    if was.is_none_or(|text| cell_shown(&text.plain()) != typed) {
                                        let text = retyped(was.unwrap_or(&TextBody::default()), typed);
                                        cell["text"] = serde_json::to_value(text).unwrap_or_default();
                                    }
                                    cell
                                })
                                .collect()
                        })
                        .collect();
                    let widths: Vec<f32> = if table.columns.len() == columns {
                        table.columns.clone()
                    } else {
                        vec![1.; columns]
                    };
                    editor.update(cx, |this, cx| {
                        this.apply("table", vec![Op::UpdateElement {
                            id: id.clone(),
                            set: json!({ "rows": cells, "columns": widths }),
                        }], window, cx);
                    });
                }))
        });
    }

    /// A chart's title and data as rows: the series across the top, a
    /// category a line.
    pub(crate) fn open_chart(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Element {
            kind: Kind::Chart(chart),
            ..
        }) = self.deck(cx).element(&id).map(|(_, e)| e.clone())
        else {
            return;
        };
        let scatter = chart.chart == ChartKind::Scatter;
        let count = chart.categories.len().max(
            chart
                .series
                .iter()
                .map(|series| series.values.len())
                .max()
                .unwrap_or(0),
        );
        let corner = if scatter { "x" } else { "" }.to_string();
        let names = chart.series.iter().map(|series| series.name.clone());
        let mut lines = vec![row_text(
            &std::iter::once(corner).chain(names).collect::<Vec<_>>(),
        )];
        for ix in 0..count {
            let first = if scatter {
                chart
                    .series
                    .first()
                    .and_then(|series| series.x.get(ix))
                    .map(|x| x.to_string())
                    .unwrap_or_default()
            } else {
                chart.categories.get(ix).cloned().unwrap_or_default()
            };
            let values = chart.series.iter().map(|series| {
                series
                    .values
                    .get(ix)
                    .map(|v| v.to_string())
                    .unwrap_or_default()
            });
            lines.push(row_text(
                &std::iter::once(first).chain(values).collect::<Vec<_>>(),
            ));
        }
        let data = self.textarea(lines.join("\n"), 6, window, cx);
        let title = self.input(chart.title.clone(), "No title", window, cx);
        let editor = cx.entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let (data, title, editor, chart, id) = (data.clone(), title.clone(), editor.clone(), chart.clone(), id.clone());
            dialog
                .title("Chart data")
                .w(ph_ui::scaled_px(560., window))
                .child(
                    v_flex()
                        .gap_2()
                        .child(h_flex().gap_2().child(div().w(ph_ui::rems_from_px(60.)).text_sm().child("Title")).child(div().flex_1().child(Input::new(&title))))
                        .child(hint(
                            if scatter {
                                "The first row names the series; then a line a point: its x, then each series' y."
                            } else {
                                "The first row names the series; then a line a category: its name, then each series' value. Cells apart by | or tabs (paste from a spreadsheet)."
                            },
                            cx,
                        ))
                        .child(Textarea::new(&data).font_family(cx.theme().mono_font_family.clone())),
                )
                .footer(footer("chart-save", "Save", move |window, cx| {
                    let rows = rows_of(data.read(cx).value().as_ref());
                    let Some((head, body)) = rows.split_first() else {
                        return;
                    };
                    let names: Vec<String> = head.iter().skip(1).cloned().collect();
                    // `NaN` and `inf` parse as numbers but cannot be kept: they
                    // read as any other text does.
                    let number = |text: &str| text.replace([',', '%', '$', '€', '£'], "").trim().parse::<f64>().ok().filter(|value| value.is_finite());
                    let categories: Vec<String> = body.iter().map(|row| row.first().cloned().unwrap_or_default()).collect();
                    let series: Vec<Value> = names
                        .iter()
                        .enumerate()
                        .map(|(ix, name)| {
                            let values: Vec<f64> = body
                                .iter()
                                .map(|row| row.get(ix + 1).and_then(|cell| number(cell)).unwrap_or(0.))
                                .collect();
                            let mut series = json!({ "name": name, "values": values });
                            if let Some(color) = chart.series.get(ix).and_then(|series| series.color.clone()) {
                                series["color"] = json!(color);
                            }
                            if scatter {
                                series["x"] = json!(categories.iter().map(|x| number(x).unwrap_or(0.)).collect::<Vec<_>>());
                            }
                            series
                        })
                        .collect();
                    let title = title.read(cx).value().to_string();
                    editor.update(cx, |this, cx| {
                        this.apply("chart", vec![Op::UpdateElement {
                            id: id.clone(),
                            set: json!({
                                "title": title,
                                "categories": if scatter { json!([]) } else { json!(categories) },
                                "series": series,
                            }),
                        }], window, cx);
                    });
                }))
        });
    }

    /// The icon library: search, then a click adds the icon (or changes
    /// the one being replaced).
    pub(crate) fn open_icons(
        &mut self,
        target: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let search = self.input(String::new(), "Search icons", window, cx);
        search.update(cx, |input, cx| input.focus(window, cx));
        let names: Rc<Vec<String>> = Rc::new(crate::slides::icons::names());
        // The icons as pictures, made once for this dialog.
        let pictures: Rc<RefCell<std::collections::HashMap<String, Arc<Image>>>> = Rc::default();
        let editor = cx.entity();
        let watch = cx.subscribe_in(&search, window, |_, _, event: &InputEvent, window, _| {
            if matches!(event, InputEvent::Change) {
                window.refresh();
            }
        });
        self.dialog_watch = Some(watch);
        window.open_dialog(cx, move |dialog, window, cx| {
            let query = search.read(cx).value().trim().to_lowercase();
            let shown: Vec<String> = names
                .iter()
                .filter(|name| {
                    query.is_empty() || query.split_whitespace().all(|word| name.contains(word))
                })
                .take(240)
                .cloned()
                .collect();
            let border = cx.theme().border;
            let hover = cx.theme().accent;
            let grid = div()
                .flex()
                .flex_wrap()
                .gap_1()
                .children(shown.into_iter().map(|name| {
                    let picture = pictures
                        .borrow_mut()
                        .entry(name.clone())
                        .or_insert_with(|| {
                            let svg = crate::slides::icons::svg(&name)
                                .map(|bytes| {
                                    String::from_utf8_lossy(&bytes)
                                        .replace("currentColor", "#444444")
                                })
                                .unwrap_or_default();
                            Arc::new(Image::from_bytes(ImageFormat::Svg, svg.into_bytes()))
                        })
                        .clone();
                    let (editor, target, title) = (editor.clone(), target.clone(), name.clone());
                    div()
                        .id(SharedString::from(format!("icon-{name}")))
                        .size(ph_ui::rems_from_px(44.))
                        .p(ph_ui::rems_from_px(9.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(border)
                        .bg(gpui_kit::white())
                        .hover(move |style| style.bg(hover))
                        .cursor_pointer()
                        .tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(ph_ui::tip(title.clone()))
                                .build(window, cx)
                        })
                        .child(img(picture).size_full().object_fit(ObjectFit::Contain))
                        .on_click(move |_, window, cx| {
                            window.close_dialog(cx);
                            editor.update(cx, |this, cx| {
                                this.place_icon(&name, target.clone(), window, cx)
                            });
                        })
                }));
            dialog
                .title("Icons")
                .w(ph_ui::scaled_px(620., window))
                .child(
                    v_flex().gap_2().child(Input::new(&search)).child(
                        div()
                            .id("icon-grid")
                            .max_h(ph_ui::rems_from_px(420.))
                            .overflow_y_scroll()
                            .child(grid),
                    ),
                )
        });
    }

    fn place_icon(
        &mut self,
        name: &str,
        target: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match target {
            Some(id) => {
                self.apply(
                    "icon",
                    vec![Op::UpdateElement {
                        id,
                        set: json!({ "icon": name }),
                    }],
                    window,
                    cx,
                );
            }
            None => {
                let frame = self.centered(96., 96., cx);
                self.insert(
                    json!({ "type": "icon", "icon": name, "color": "accent1", "frame": frame }),
                    window,
                    cx,
                );
            }
        }
    }

    /// Another picture in the same frame.
    pub(crate) fn replace_picture(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let chosen = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Replace".into()),
        });
        let space = self.doc.read(cx).space().clone();
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let added = cx
                .background_spawn(async move { space.add_media(&path) })
                .await;
            _ = this.update_in(cx, |this, window, cx| match added {
                Ok(name) => {
                    this.apply(
                        "picture",
                        vec![Op::UpdateElement {
                            id,
                            set: json!({ "media": name }),
                        }],
                        window,
                        cx,
                    );
                }
                Err(err) => window.push_notification(Notification::error(format!("{err:#}")), cx),
            });
        })
        .detach();
    }

    /// SVG typed or pasted (or read from a file), as a new drawing or over
    /// one's.
    pub(crate) fn open_svg(
        &mut self,
        target: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let source = target
            .as_ref()
            .and_then(|id| self.deck(cx).element(id))
            .and_then(|(_, element)| match &element.kind {
                Kind::Svg(art) => Some(art.source.clone()),
                _ => None,
            })
            .unwrap_or_default();
        let input = self.textarea(source, 8, window, cx);
        let editor = cx.entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let (input, editor, target) = (input.clone(), editor.clone(), target.clone());
            let file_input = input.clone();
            dialog
                .title("SVG drawing")
                .w(ph_ui::scaled_px(620., window))
                .child(
                    v_flex()
                        .gap_2()
                        .child(hint("Paste SVG markup, or read it from a file. Ask the agent beside you for diagrams and illustrations as SVG.", cx))
                        .child(div().id("svg-source").max_h(ph_ui::rems_from_px(380.)).overflow_y_scroll().child(Textarea::new(&input).font_family(cx.theme().mono_font_family.clone())))
                        .child(
                            Button::new("svg-file")
                                .ghost()
                                .small()
                                .icon(Icons::FolderOpen)
                                .label("From a file")
                                .on_click(move |_, window, cx| {
                                    let chosen = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
                                        files: true,
                                        directories: false,
                                        multiple: false,
                                        prompt: Some("Open".into()),
                                    });
                                    let input = file_input.clone();
                                    window
                                        .spawn(cx, async move |cx| {
                                            let Ok(Ok(Some(paths))) = chosen.await else {
                                                return;
                                            };
                                            // Read off the UI thread: the file can be of any size, on any disk.
                                            let read = cx.background_spawn(async move {
                                                paths.first().and_then(|path| std::fs::read_to_string(path).ok())
                                            });
                                            let Some(text) = read.await else {
                                                return;
                                            };
                                            _ = cx.update(|window, cx| {
                                                input.update(cx, |input, cx| input.set_value(text, window, cx));
                                            });
                                        })
                                        .detach();
                                }),
                        ),
                )
                .footer(footer("svg-save", if target.is_some() { "Save" } else { "Add" }, move |window, cx| {
                    let source = input.read(cx).value().to_string();
                    editor.update(cx, |this, cx| this.place_svg(source, target.clone(), window, cx));
                }))
        });
    }

    fn place_svg(
        &mut self,
        source: String,
        target: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if source.trim().is_empty() {
            return;
        }
        match target {
            Some(id) => {
                self.apply(
                    "svg",
                    vec![Op::UpdateElement {
                        id,
                        set: json!({ "source": source }),
                    }],
                    window,
                    cx,
                );
            }
            None => {
                // As wide as tall as the drawing says, which takes reading
                // all of it: off the UI thread.
                let parsed = source.clone();
                let aspect = cx.background_spawn(async move {
                    // One that nests too deeply is not read (it would use
                    // up this thread's stack); adding it is refused with
                    // the reason a moment later.
                    if crate::slides::raster::too_deep(parsed.as_bytes()) {
                        return 1.;
                    }
                    resvg::usvg::Tree::from_str(&parsed, &crate::slides::raster::inert_options())
                        .map(|tree| tree.size().width() / tree.size().height().max(1.))
                        .unwrap_or(1.)
                });
                cx.spawn_in(window, async move |this, cx| {
                    let aspect = aspect.await;
                    _ = this.update_in(cx, |this, window, cx| {
                        let size = &this.deck(cx).size;
                        let fit = (size.width * 0.5 / aspect).min(size.height * 0.5);
                        let frame = this.centered(fit * aspect, fit, cx);
                        this.insert(
                            json!({ "type": "svg", "source": source, "frame": frame }),
                            window,
                            cx,
                        );
                    });
                })
                .detach();
            }
        }
    }

    /// Export with every option: what, which slides, notes, picture size.
    pub(crate) fn open_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.deck(cx).slides.len();
        let format = Rc::new(Cell::new(ExportFormat::Pptx));
        let scope = Rc::new(Cell::new(0u8));
        let notes = Rc::new(Cell::new(false));
        let width = Rc::new(Cell::new(1920u32));
        let range = self.input(format!("1-{count}"), "1-3, 5", window, cx);
        let editor = cx.entity();
        let current = self.current;
        window.open_dialog(cx, move |dialog, window, cx| {
            let (format, scope, notes, width, range, editor) =
                (format.clone(), scope.clone(), notes.clone(), width.clone(), range.clone(), editor.clone());
            let chip = |id: &'static str, label: &'static str, on: bool, set: Rc<dyn Fn()>| {
                Button::new(id).xsmall().outline().label(label).selected(on).on_click(move |_, window, _| {
                    set();
                    window.refresh();
                })
            };
            let row = |label: &'static str, children: Vec<AnyElement>| {
                h_flex().gap_2().child(div().w(ph_ui::rems_from_px(80.)).text_sm().child(label)).child(h_flex().gap_1().flex_wrap().children(children))
            };
            let chosen = format.get();
            let formats: Vec<AnyElement> = [
                ("export-pptx", "PowerPoint", ExportFormat::Pptx),
                ("export-pdf", "PDF", ExportFormat::Pdf),
                ("export-png", "Pictures (PNG)", ExportFormat::Png),
            ]
            .into_iter()
            .map(|(id, label, value)| {
                let format = format.clone();
                chip(id, label, chosen == value, Rc::new(move || format.set(value))).into_any_element()
            })
            .collect();
            let scopes: Vec<AnyElement> = [("export-all", "All", 0u8), ("export-this", "This slide", 1), ("export-some", "These:", 2)]
                .into_iter()
                .map(|(id, label, value)| {
                    let scope = scope.clone();
                    chip(id, label, scope.get() == value, Rc::new(move || scope.set(value))).into_any_element()
                })
                .chain(std::iter::once(div().w(ph_ui::rems_from_px(110.)).child(Input::new(&range).small()).into_any_element()))
                .collect();
            let go = {
                let (format, scope, notes, width, range) = (format.clone(), scope.clone(), notes.clone(), width.clone(), range.clone());
                move |window: &mut Window, cx: &mut App| {
                    let slides = match scope.get() {
                        0 => Vec::new(),
                        1 => vec![current],
                        _ => {
                            let slides = parse_range(range.read(cx).value().as_ref(), count);
                            // An empty list is every slide to the export:
                            // a range that names none is refused instead.
                            if slides.is_empty() {
                                window.push_notification(
                                    Notification::error(format!(
                                        "Not exported: no slide is in that range. Give slides from 1 to {count}, as in 1-3, 5."
                                    )),
                                    cx,
                                );
                                return;
                            }
                            slides
                        }
                    };
                    let (format, notes, width) = (format.get(), notes.get(), width.get());
                    editor.update(cx, |this, cx| this.export_with(format, slides, notes, width, window, cx));
                }
            };
            dialog
                .title("Export")
                .w(ph_ui::scaled_px(500., window))
                .child(
                    v_flex()
                        .gap_3()
                        .child(row("As", formats))
                        .child(row("Slides", scopes))
                        .when(chosen == ExportFormat::Pdf, |this| {
                            let notes_cell = notes.clone();
                            this.child(row("Notes", vec![chip("export-notes", "A notes page after each slide", notes.get(), Rc::new(move || notes_cell.set(!notes_cell.get()))).into_any_element()]))
                        })
                        .when(chosen == ExportFormat::Png, |this| {
                            let sizes: Vec<AnyElement> = [("export-720", "1280 px", 1280u32), ("export-1080", "1920 px", 1920), ("export-4k", "3840 px", 3840)]
                                .into_iter()
                                .map(|(id, label, value)| {
                                    let width = width.clone();
                                    chip(id, label, width.get() == value, Rc::new(move || width.set(value))).into_any_element()
                                })
                                .collect();
                            this.child(row("Width", sizes))
                        })
                        .child(hint("Files go to the presentation's exports folder, which opens when they are written.", cx)),
                )
                .footer(footer("export-go", "Export", go))
        });
    }

    fn export_with(
        &mut self,
        format: ExportFormat,
        slides: Vec<usize>,
        notes: bool,
        width: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if format == ExportFormat::Png && width != 1920 {
            // A size of its own: the folder (or file) says it.
            let doc = self.doc.read(cx);
            let mut path = doc.export_path("png");
            let stem = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            path.set_file_name(format!("{stem} {width}.png"));
            let writing = self.doc.update(cx, |doc, cx| {
                doc.export(format, path, slides, notes, width, cx)
            });
            cx.spawn_in(window, async move |_, cx| {
                let result = writing.await;
                _ = cx.update(|window, cx| match result {
                    Ok(written) => ph_ui::platform::reveal(&written, cx),
                    Err(err) => window.push_notification(
                        Notification::error(format!("Not exported: {err:#}")),
                        cx,
                    ),
                });
            })
            .detach();
            return;
        }
        self.export(format, slides, notes, None, window, cx);
    }

    pub(crate) fn open_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let title = self.deck(cx).title.clone();
        let input = self.input(title, "Title", window, cx);
        input.update(cx, |input, cx| input.focus(window, cx));
        let doc = self.doc.clone();
        window.open_dialog(cx, move |dialog, window, cx| {
            let (input, doc) = (input.clone(), doc.clone());
            dialog
                .title("Rename")
                .w(ph_ui::scaled_px(420., window))
                .child(v_flex().gap_2().child(Input::new(&input)).child(hint(
                    "The title shows in the list of presentations and names exports.",
                    cx,
                )))
                .footer(footer("rename-save", "Rename", move |_, cx| {
                    let title = input.read(cx).value().to_string();
                    doc.update(cx, |doc, cx| doc.rename(&title, cx));
                }))
        });
    }

    pub(crate) fn open_save_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let deck = self.deck(cx);
        let name = if crate::slides::theme::builtins().any(|(_, name)| name == deck.theme.name) {
            format!("{} theme", deck.title)
        } else {
            deck.theme.name.clone()
        };
        let input = self.input(name, "Name", window, cx);
        input.update(cx, |input, cx| input.focus(window, cx));
        let editor = cx.entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let (input, editor) = (input.clone(), editor.clone());
            dialog
                .title("Save theme")
                .w(ph_ui::scaled_px(440., window))
                .child(v_flex().gap_2().child(Input::new(&input)).child(hint("Its colours, fonts, background, logo and layouts, for any presentation (Theme menu, under Yours).", cx)))
                .footer(footer("theme-save", "Save", move |window, cx| {
                    let name = input.read(cx).value().trim().to_string();
                    if name.is_empty() {
                        return;
                    }
                    let deck = editor.read(cx).doc.read(cx).deck();
                    // Written off the UI thread.
                    let (theme, size, saved) = (deck.theme.clone(), deck.size.clone(), name.clone());
                    let written = cx.background_spawn(async move {
                        crate::slides::space::save_theme(&crate::slides::space::themes_dir(), &saved, &theme, &size).map(|_| ())
                    });
                    let said = name.clone();
                    window
                        .spawn(cx, async move |cx| {
                            let result = written.await;
                            _ = cx.update(|window, cx| {
                                let notification = match result {
                                    Ok(()) => Notification::success(format!("Saved the theme {said}.")),
                                    Err(err) => Notification::error(format!("Not saved: {err:#}")),
                                };
                                window.push_notification(notification, cx);
                            });
                        })
                        .detach();
                    // The deck now uses it by that name.
                    editor.update(cx, |this, cx| this.change("theme name", |deck| deck.theme.name = name.clone(), cx));
                }))
        });
    }

    /// The theme's twelve colours and three fonts.
    pub(crate) fn open_theme_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let theme = self.deck(cx).theme.clone();
        let colors: Vec<(String, Entity<InputState>)> = SLOTS
            .iter()
            .map(|slot| {
                let hex = theme
                    .slot(slot)
                    .map(|rgba| format!("#{}", rgba.hex()))
                    .unwrap_or_default();
                (slot.to_string(), self.input(hex, "#RRGGBB", window, cx))
            })
            .collect();
        let fonts: Vec<(&'static str, Entity<InputState>)> = [
            ("heading", theme.fonts.heading.clone()),
            ("body", theme.fonts.body.clone()),
            ("code", theme.fonts.code.clone()),
        ]
        .into_iter()
        .map(|(key, value)| (key, self.input(value, "Family", window, cx)))
        .collect();
        for (_, input) in &colors {
            let watch = cx.subscribe_in(input, window, |_, _, event: &InputEvent, window, _| {
                if matches!(event, InputEvent::Change) {
                    window.refresh();
                }
            });
            self.dialog_subscriptions.push(watch);
        }
        let editor = cx.entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let (colors_in, fonts_in, editor, closing) = (colors.clone(), fonts.clone(), editor.clone(), editor.clone());
            let swatches = colors.iter().map(|(slot, input)| {
                let value = input.read(cx).value().to_string();
                let color = Rgba::parse_hex(value.trim()).map(super::canvas::to_hsla);
                h_flex()
                    .gap_2()
                    .w(ph_ui::rems_from_px(250.))
                    .child(div().w(ph_ui::rems_from_px(84.)).text_sm().child(slot_label(slot)))
                    .child(
                        div()
                            .size(ph_ui::rems_from_px(18.))
                            .rounded(px(3.))
                            .border_1()
                            .border_color(cx.theme().border)
                            .when_some(color, |this, color| this.bg(color)),
                    )
                    .child(div().w(ph_ui::rems_from_px(110.)).child(Input::new(input).small()))
            });
            let font_rows = fonts.iter().map(|(key, input)| {
                h_flex()
                    .gap_2()
                    .child(div().w(ph_ui::rems_from_px(84.)).text_sm().child(match *key {
                        "heading" => "Headings",
                        "body" => "Body text",
                        _ => "Code",
                    }))
                    .child(div().w(ph_ui::rems_from_px(220.)).child(Input::new(input).small()))
            });
            dialog
                .title("Colours and fonts")
                .w(ph_ui::scaled_px(560., window))
                .child(
                    v_flex()
                        .gap_3()
                        .child(div().flex().flex_wrap().gap_2().children(swatches))
                        .child(v_flex().gap_2().children(font_rows))
                        .child(hint("Every slide follows: text, shapes and charts in theme colours change with it. Fonts missing on a computer show in a look-alike.", cx)),
                )
                .footer(footer("theme-colors-save", "Apply", move |window, cx| {
                    let mut colors = serde_json::Map::new();
                    for (slot, input) in &colors_in {
                        let value = input.read(cx).value().trim().to_string();
                        let value = if value.starts_with('#') { value } else { format!("#{value}") };
                        if Rgba::parse_hex(&value).is_some() {
                            colors.insert(slot.clone(), json!(value.to_uppercase()));
                        }
                    }
                    let mut fonts = serde_json::Map::new();
                    for (key, input) in &fonts_in {
                        let value = input.read(cx).value().trim().to_string();
                        if !value.is_empty() {
                            fonts.insert(key.to_string(), json!(value));
                        }
                    }
                    editor.update(cx, |this, cx| {
                        this.apply("theme", vec![Op::SetTheme {
                            theme: None,
                            custom: None,
                            saved: None,
                            colors: Value::Object(colors),
                            fonts: Some(Value::Object(fonts)),
                            background: None,
                        }], window, cx);
                        this.dialog_subscriptions.clear();
                    });
                }))
                .on_close(move |_, _, cx| closing.update(cx, |this, _| this.dialog_subscriptions.clear()))
        });
    }

    pub(crate) fn open_custom_size(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let size = self.deck(cx).size.clone();
        let inches = |points: f32| format!("{:.2}", points / 72.);
        let width = self.input(inches(size.width), "Width", window, cx);
        let height = self.input(inches(size.height), "Height", window, cx);
        let unit = Rc::new(Cell::new(0u8));
        let editor = cx.entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let (width, height, unit, editor) =
                (width.clone(), height.clone(), unit.clone(), editor.clone());
            let units: Vec<AnyElement> = [
                ("size-in", "inches", 0u8),
                ("size-cm", "cm", 1),
                ("size-px", "pixels", 2),
                ("size-pt", "points", 3),
            ]
            .into_iter()
            .map(|(id, label, value)| {
                let unit = unit.clone();
                Button::new(id)
                    .xsmall()
                    .outline()
                    .label(label)
                    .selected(unit.get() == value)
                    .on_click(move |_, window, _| {
                        unit.set(value);
                        window.refresh();
                    })
                    .into_any_element()
            })
            .collect();
            dialog
                .title("Custom size")
                .w(ph_ui::scaled_px(440., window))
                .child(
                    v_flex()
                        .gap_3()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(div().w(ph_ui::rems_from_px(110.)).child(Input::new(&width)))
                                .child("×")
                                .child(
                                    div()
                                        .w(ph_ui::rems_from_px(110.))
                                        .child(Input::new(&height)),
                                ),
                        )
                        .child(h_flex().gap_1().children(units))
                        .child(hint("What is on the slides is scaled to the new size.", cx)),
                )
                .footer(footer("size-save", "Apply", move |window, cx| {
                    let per = match unit.get() {
                        0 => 72.,
                        1 => 72. / 2.54,
                        2 => 0.75,
                        _ => 1.,
                    };
                    let read = |input: &Entity<InputState>| {
                        input
                            .read(cx)
                            .value()
                            .trim()
                            .replace(',', ".")
                            .parse::<f32>()
                            .ok()
                    };
                    let (Some(w), Some(h)) = (read(&width), read(&height)) else {
                        window.push_notification(
                            Notification::error("Give the width and height as numbers."),
                            cx,
                        );
                        return;
                    };
                    let (w, h) = ((w * per).clamp(72., 4000.), (h * per).clamp(72., 4000.));
                    editor.update(cx, |this, cx| {
                        this.set_size(None, Some(w), Some(h), window, cx)
                    });
                }))
        });
    }

    /// Starts (or renames, or removes) a section at slide `index`.
    pub(crate) fn open_section(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let deck = self.deck(cx);
        let Some(slide) = deck.slides.get(index).map(|slide| slide.id.clone()) else {
            return;
        };
        let existing = deck
            .sections
            .iter()
            .find(|section| section.slide == slide)
            .map(|section| section.name.clone());
        let input = self.input(
            existing.clone().unwrap_or_default(),
            "Section name",
            window,
            cx,
        );
        input.update(cx, |input, cx| input.focus(window, cx));
        let editor = cx.entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let (input, editor, slide, existing) = (
                input.clone(),
                editor.clone(),
                slide.clone(),
                existing.clone(),
            );
            let remove = existing.clone().map(|name| {
                let editor = editor.clone();
                Button::new("section-remove")
                    .ghost()
                    .small()
                    .label("Remove the section")
                    .on_click(move |_, window, cx| {
                        window.close_dialog(cx);
                        editor.update(cx, |this, cx| {
                            this.apply(
                                "section",
                                vec![Op::RemoveSection { name: name.clone() }],
                                window,
                                cx,
                            );
                        });
                    })
            });
            dialog
                .title("Section")
                .w(ph_ui::scaled_px(420., window))
                .child(
                    v_flex()
                        .gap_2()
                        .child(Input::new(&input))
                        .child(hint(
                            "Sections group slides; they show in the rail and in PowerPoint.",
                            cx,
                        ))
                        .children(remove),
                )
                .footer(footer("section-save", "Save", move |window, cx| {
                    let name = input.read(cx).value().trim().to_string();
                    if name.is_empty() {
                        return;
                    }
                    let mut ops = Vec::new();
                    if let Some(old) = &existing {
                        ops.push(Op::RemoveSection { name: old.clone() });
                    }
                    ops.push(Op::AddSection {
                        name,
                        slide: json!(slide),
                    });
                    editor.update(cx, |this, cx| {
                        this.apply("section", ops, window, cx);
                    });
                }))
        });
    }

    /// The versions kept as the deck was saved, to go back to one.
    pub(crate) fn open_versions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The folder is listed off the UI thread; then the dialog opens.
        let space = self.doc.read(cx).space().clone();
        let listed = cx.background_spawn(async move { space.versions() });
        cx.spawn_in(window, async move |this, cx| {
            let versions = listed.await;
            _ = this.update_in(cx, |this, window, cx| {
                this.show_versions(versions, window, cx)
            });
        })
        .detach();
    }

    fn show_versions(
        &mut self,
        versions: Vec<(PathBuf, std::time::SystemTime)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editor = cx.entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let rows: Vec<AnyElement> = if versions.is_empty() {
                vec![hint(
                    "None yet: a version is kept every few minutes while you work.",
                    cx,
                )]
            } else {
                versions
                    .iter()
                    .enumerate()
                    .map(|(ix, (path, at))| {
                        let ago = at
                            .elapsed()
                            .map(|elapsed| ago(elapsed.as_secs()))
                            .unwrap_or_default();
                        let (editor, path) = (editor.clone(), path.clone());
                        h_flex()
                            .gap_2()
                            .py_1()
                            .child(div().flex_1().text_sm().child(ago))
                            .child(
                                Button::new(("version-restore", ix))
                                    .ghost()
                                    .xsmall()
                                    .label("Go back to this")
                                    .on_click(move |_, window, cx| {
                                        window.close_dialog(cx);
                                        // Read off the UI thread: a deck
                                        // can be several megabytes.
                                        let (editor, path) = (editor.clone(), path.clone());
                                        let read = cx.background_spawn(async move {
                                            let bytes = std::fs::read(&path)?;
                                            anyhow::Ok(serde_json::from_slice::<Deck>(&bytes)?)
                                        });
                                        window
                                            .spawn(cx, async move |cx| {
                                                let read = read.await;
                                                _ = cx.update(|window, cx| match read {
                                                    Ok(deck) => editor.update(cx, |this, cx| {
                                                        this.doc.update(cx, |doc, cx| {
                                                            doc.settle();
                                                            doc.commit("restore", deck, cx);
                                                            doc.settle();
                                                        });
                                                    }),
                                                    Err(err) => window.push_notification(
                                                        Notification::error(format!(
                                                            "Not read: {err:#}"
                                                        )),
                                                        cx,
                                                    ),
                                                });
                                            })
                                            .detach();
                                    }),
                            )
                            .into_any_element()
                    })
                    .collect()
            };
            dialog
                .title("Earlier versions")
                .w(ph_ui::scaled_px(420., window))
                .child(
                    v_flex()
                        .children(rows)
                        .child(hint("Going back can be undone.", cx)),
                )
        });
    }
}

impl SlidesEditor {
    /// Find (and replace) text in every slide.
    pub(crate) fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_editing(window, cx);
        let find = self.input(String::new(), "Find", window, cx);
        let replace = self.input(String::new(), "Replace with", window, cx);
        // Once the dialog is up (opened from a key, the key's own focus
        // handling comes after).
        let focus = find.clone();
        window.defer(cx, move |window, cx| {
            focus.update(cx, |input, cx| input.focus(window, cx))
        });
        let watch = cx.subscribe_in(&find, window, |_, _, event: &InputEvent, window, _| {
            if matches!(event, InputEvent::Change) {
                window.refresh();
            }
        });
        self.dialog_watch = Some(watch);
        let editor = cx.entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let wanted = find.read(cx).value().to_string();
            // Where it is: slide, element.
            let found: Vec<(usize, String)> = if wanted.is_empty() {
                Vec::new()
            } else {
                let deck = editor.read(cx).deck(cx);
                let mut found = Vec::new();
                for (index, slide) in deck.slides.iter().enumerate() {
                    for element in &slide.elements {
                        let mut texts = Vec::new();
                        collect_text(element, &mut texts);
                        let count = texts
                            .iter()
                            .map(|text| text.matches(wanted.as_str()).count())
                            .sum::<usize>();
                        for _ in 0..count {
                            found.push((index, element.id.clone()));
                        }
                    }
                }
                found
            };
            let total = found.len();
            let (next_editor, all_editor) = (editor.clone(), editor.clone());
            let (find_all, replace_all) = (find.clone(), replace.clone());
            let next = Button::new("find-next")
                .ghost()
                .label("Find next")
                .disabled(total == 0)
                .on_click(move |_, window, cx| {
                    let found = found.clone();
                    next_editor.update(cx, |this, cx| {
                        // The next after the selection, slide by slide.
                        let here = (this.current, this.selected.first().cloned());
                        if let Some((slide, id)) = next_found(&found, &here) {
                            this.go_to(slide, window, cx);
                            this.select(vec![id], cx);
                        }
                    });
                });
            let all = Button::new("find-replace")
                .primary()
                .label("Replace all")
                .disabled(total == 0)
                .on_click(move |_, window, cx| {
                    let find = find_all.read(cx).value().to_string();
                    let replace = replace_all.read(cx).value().to_string();
                    if find.is_empty() {
                        return;
                    }
                    all_editor.update(cx, |this, cx| {
                        this.apply(
                            "replace",
                            vec![Op::Replace {
                                find,
                                replace,
                                id: None,
                            }],
                            window,
                            cx,
                        );
                    });
                    window.refresh();
                });
            dialog
                .title("Find and replace")
                .w(ph_ui::scaled_px(460., window))
                .child(
                    v_flex()
                        .gap_2()
                        .child(Input::new(&find))
                        .child(Input::new(&replace))
                        .child(hint(
                            if wanted.is_empty() {
                                "Text in every slide's boxes, shapes and tables.".to_string()
                            } else {
                                format!("Found {total} time{}.", if total == 1 { "" } else { "s" })
                            },
                            cx,
                        )),
                )
                .footer(
                    DialogFooter::new()
                        .w_full()
                        .child(div().flex_1())
                        .child(next)
                        .child(all),
                )
        });
    }
}

/// The place after `here` (a slide and its selected element) among `found`,
/// which has an entry for each time the text occurs: the next other element,
/// round to the first again; `here` itself when it is the only one.
fn next_found(
    found: &[(usize, String)],
    here: &(usize, Option<String>),
) -> Option<(usize, String)> {
    let is_here = |(slide, id): &&(usize, String)| *slide == here.0 && Some(id) == here.1.as_ref();
    let start = found
        .iter()
        .position(|entry| is_here(&entry))
        .map_or(0, |ix| ix + 1);
    found
        .iter()
        .cycle()
        .skip(start)
        // Once round at most: with every entry in the one element (the
        // text twice in it) there is no other to come to.
        .take(found.len())
        .find(|entry| !is_here(entry))
        .or(found.first())
        .cloned()
}

/// Every text an element holds (its tables' cells, its group's too).
fn collect_text(element: &Element, out: &mut Vec<String>) {
    match &element.kind {
        Kind::Table(table) => {
            for row in &table.rows {
                for cell in row {
                    out.push(cell.text.plain());
                }
            }
        }
        Kind::Group(group) => {
            for child in &group.children {
                collect_text(child, out);
            }
        }
        kind => {
            if let Some(text) = kind.text() {
                out.push(text.plain());
            }
        }
    }
}

/// `1-3, 5` as slide indexes (from 0), within `count`.
fn parse_range(text: &str, count: usize) -> Vec<usize> {
    let mut out = Vec::new();
    for part in text.split([',', ' ']).filter(|part| !part.is_empty()) {
        // Also the dashes a keyboard puts in place of a hyphen.
        let (from, to) = match part.split_once(['-', '–', '—']) {
            Some((from, to)) => (
                from.trim().parse::<usize>().ok(),
                to.trim().parse::<usize>().ok(),
            ),
            None => {
                let n = part.trim().parse::<usize>().ok();
                (n, n)
            }
        };
        if let (Some(from), Some(to)) = (from, to) {
            for n in from.max(1)..=to.min(count) {
                if !out.contains(&(n - 1)) {
                    out.push(n - 1);
                }
            }
        }
    }
    out
}

fn slot_label(slot: &str) -> String {
    match slot {
        "dark1" => "Text".into(),
        "light1" => "Background".into(),
        "dark2" => "Text 2".into(),
        "light2" => "Background 2".into(),
        "link" => "Link".into(),
        "followed_link" => "Visited link".into(),
        other => other.replace("accent", "Accent "),
    }
}

fn ago(seconds: u64) -> String {
    match seconds {
        0..=59 => "Just now".into(),
        60..=3599 => format!("{} minutes ago", seconds / 60),
        3600..=86_399 => format!("{} hours ago", seconds / 3600),
        _ => format!("{} days ago", seconds / 86_400),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn rows_and_ranges() {
        assert_eq!(
            super::rows_of("a | b\n1\t2\n"),
            vec![vec!["a", "b"], vec!["1", "2"]]
        );
        assert_eq!(super::parse_range("1-3, 5, 9", 6), vec![0, 1, 2, 4]);
        assert_eq!(super::parse_range("2–4", 6), vec![1, 2, 3]);
        assert!(super::parse_range("9", 6).is_empty());
    }

    #[test]
    fn an_empty_first_cell_stays_a_cell() {
        use super::{row_text, rows_of};
        // The chart's header: an empty corner, then the series.
        assert_eq!(
            rows_of(" | Sales | Costs"),
            vec![vec!["", "Sales", "Costs"]]
        );
        assert_eq!(rows_of("\tSales\tCosts"), vec![vec!["", "Sales", "Costs"]]);
        // A Markdown row has a bar at both ends, which are not cells.
        assert_eq!(rows_of("| a | b |"), vec![vec!["a", "b"]]);
        // Every row comes back as it was written.
        for row in [
            vec!["", "Sales", "Costs"],
            vec!["Q1", "1", ""],
            vec!["", "b", ""],
            vec!["", "", ""],
            vec![""],
            vec!["one, with a comma"],
            vec!["a|b", "c\\", "|"],
            vec!["", "x|"],
        ] {
            let cells: Vec<String> = row.iter().map(|cell| cell.to_string()).collect();
            assert_eq!(rows_of(&row_text(&cells)), vec![cells], "{row:?}");
        }
    }

    #[test]
    fn a_cell_typed_anew_is_plain_and_keeps_its_look() {
        use crate::slides::model::{Align, Color, TextBody};
        let mut old: TextBody = crate::slides::markdown::to_text("**Region**\nEMEA");
        old.paragraphs[0].align = Some(Align::Right);
        old.paragraphs[0].runs[0].color = Some(Color::from("accent2"));
        // Shown on one line, so a cell not touched compares equal.
        assert_eq!(super::cell_shown(&old.plain()), "Region EMEA");
        let typed = super::retyped(&old, "# of seats");
        assert_eq!(typed.plain(), "# of seats");
        let run = &typed.paragraphs[0].runs[0];
        assert_eq!(
            (run.bold, run.color.clone()),
            (Some(true), Some(Color::from("accent2")))
        );
        assert_eq!(typed.paragraphs[0].align, Some(Align::Right));
    }

    #[test]
    fn find_next_ends_when_the_text_is_twice_in_one_element() {
        let found = |ids: &[&str]| -> Vec<(usize, String)> {
            ids.iter().map(|id| (0, id.to_string())).collect()
        };
        let at = |id: &str| (0, Some(id.to_string()));
        let next =
            |ids: &[&str], here: &str| super::next_found(&found(ids), &at(here)).map(|(_, id)| id);
        // Nothing else to go to: it stays (this once never returned).
        assert_eq!(next(&["a", "a"], "a").as_deref(), Some("a"));
        assert_eq!(next(&["a", "a", "b"], "a").as_deref(), Some("b"));
        assert_eq!(next(&["a", "a", "b"], "b").as_deref(), Some("a"));
        assert_eq!(next(&["a", "b"], "elsewhere").as_deref(), Some("a"));
        assert_eq!(next(&[], "a"), None);
    }
}

//! Jupyter notebooks: Markdown cells rendered, code cells highlighted, and
//! their saved outputs (text, images, errors) under them.

use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine as _;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, EventEmitter, Image, ImageFormat,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Task, Window, div, img, px,
};
use serde_json::Value;

use crate::PreviewEvent;
use crate::markdown::{READING_WIDTH, markdown_view};

#[derive(Debug, Clone)]
enum Cell {
    Markdown(SharedString),
    Code {
        source: SharedString,
        count: Option<u64>,
        outputs: Vec<Output>,
    },
    Raw(SharedString),
}

#[derive(Debug, Clone)]
enum Output {
    Text(SharedString),
    Image {
        image: Arc<Image>,
        width: f32,
        height: f32,
    },
    Error(SharedString),
}

struct Notebook {
    language: String,
    cells: Vec<Cell>,
}

pub struct NotebookPreview {
    path: PathBuf,
    notebook: Option<Result<Notebook, SharedString>>,
    scroll: ScrollHandle,
    _parse: Option<Task<()>>,
}

impl EventEmitter<PreviewEvent> for NotebookPreview {}

impl NotebookPreview {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            notebook: None,
            scroll: ScrollHandle::new(),
            _parse: None,
        }
    }

    pub fn set_text(&mut self, text: Arc<str>, cx: &mut Context<Self>) {
        let parse = cx.background_spawn(async move { parse(&text) });
        self._parse = Some(cx.spawn(async move |this, cx| {
            let parsed = parse.await;
            _ = this.update(cx, |this, cx| {
                match parsed {
                    Ok(notebook) => this.notebook = Some(Ok(notebook)),
                    // While typing, the last good notebook stays.
                    Err(err) if !matches!(this.notebook, Some(Ok(_))) => {
                        this.notebook = Some(Err(err.into()))
                    }
                    Err(_) => {}
                }
                cx.notify();
            });
        }));
    }
}

/// A notebook's text: `source` is a string or a list of lines.
fn text_of(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(lines) => lines.iter().filter_map(Value::as_str).collect(),
        _ => String::new(),
    }
}

fn parse(text: &str) -> Result<Notebook, String> {
    let root: Value = serde_json::from_str(text).map_err(|err| err.to_string())?;
    let language = root
        .pointer("/metadata/language_info/name")
        .or_else(|| root.pointer("/metadata/kernelspec/language"))
        .and_then(Value::as_str)
        .unwrap_or("python")
        .to_string();
    let cells = root
        .get("cells")
        .and_then(Value::as_array)
        .ok_or("This notebook has no cells.")?
        .iter()
        .map(|cell| {
            let source: SharedString = text_of(&cell["source"]).into();
            match cell["cell_type"].as_str() {
                Some("markdown") => Cell::Markdown(source),
                Some("code") => Cell::Code {
                    source,
                    count: cell["execution_count"].as_u64(),
                    outputs: cell["outputs"]
                        .as_array()
                        .map(|outputs| outputs.iter().filter_map(output).collect())
                        .unwrap_or_default(),
                },
                _ => Cell::Raw(source),
            }
        })
        .collect();
    Ok(Notebook { language, cells })
}

fn output(value: &Value) -> Option<Output> {
    match value["output_type"].as_str()? {
        "stream" => Some(Output::Text(text_of(&value["text"]).into())),
        "error" => {
            let traceback: Vec<String> = value["traceback"]
                .as_array()
                .map(|lines| {
                    lines
                        .iter()
                        .filter_map(Value::as_str)
                        .map(|line| {
                            crate::ansi::parse(line)
                                .into_iter()
                                .map(|line| line.text)
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                        .collect()
                })
                .unwrap_or_default();
            let text = if traceback.is_empty() {
                format!(
                    "{}: {}",
                    value["ename"].as_str().unwrap_or("Error"),
                    value["evalue"].as_str().unwrap_or("")
                )
            } else {
                traceback.join("\n")
            };
            Some(Output::Error(text.into()))
        }
        "execute_result" | "display_data" => {
            let data = &value["data"];
            for (mime, format) in [
                ("image/png", ImageFormat::Png),
                ("image/jpeg", ImageFormat::Jpeg),
            ] {
                let encoded: String = text_of(&data[mime]).split_whitespace().collect();
                if encoded.is_empty() {
                    continue;
                }
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .ok()?;
                let (width, height) = ::image::ImageReader::new(std::io::Cursor::new(&bytes))
                    .with_guessed_format()
                    .ok()?
                    .into_dimensions()
                    .ok()?;
                if !crate::image_view::fits(width, height) {
                    return Some(Output::Text("(an image too large to show)".into()));
                }
                return Some(Output::Image {
                    image: Arc::new(Image::from_bytes(format, bytes)),
                    width: width as f32,
                    height: height as f32,
                });
            }
            let text = text_of(&data["text/plain"]);
            (!text.is_empty()).then(|| Output::Text(text.into()))
        }
        _ => None,
    }
}

/// `source` as a Markdown code block, its fence longer than any run of
/// backticks inside, so the code cannot end it early.
fn fenced(language: &str, source: &str) -> String {
    let longest = source.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    format!("{fence}{language}\n{source}\n{fence}")
}

impl Render for NotebookPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let notebook = match &self.notebook {
            None => return crate::notice("Reading the notebook…", cx),
            Some(Err(err)) => return crate::notice(format!("Not a notebook: {err}"), cx),
            Some(Ok(notebook)) => notebook,
        };
        let dark = cx.theme().is_dark();
        let path = self.path.clone();
        let language = notebook.language.clone();
        let cells: Vec<AnyElement> = notebook
            .cells
            .iter()
            .enumerate()
            .map(|(ix, cell)| render_cell(ix, cell, &language, &path, dark, cx))
            .collect();
        div()
            .id("notebook-scroll")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .bg(cx.theme().background)
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(READING_WIDTH + 80.))
                    .mx_auto()
                    .px_6()
                    .py_5()
                    .gap_4()
                    .children(cells),
            )
            .into_any_element()
    }
}

fn render_cell(
    ix: usize,
    cell: &Cell,
    language: &str,
    path: &std::path::Path,
    dark: bool,
    cx: &mut Context<NotebookPreview>,
) -> AnyElement {
    let theme = cx.theme();
    let gutter = |label: Option<String>| {
        div()
            .w(px(56.))
            .flex_none()
            .pt_2()
            .text_xs()
            .text_color(theme.muted_foreground)
            .font_family(theme.mono_font_family.clone())
            .children(label)
    };
    match cell {
        Cell::Markdown(text) => h_flex()
            .items_start()
            .child(gutter(None))
            .child(div().flex_1().min_w_0().child(markdown_view(
                ("notebook-md", ix),
                text.clone(),
                path,
                dark,
                crate::markdown::opener(cx.weak_entity()),
            )))
            .into_any_element(),
        Cell::Raw(text) => h_flex()
            .items_start()
            .child(gutter(None))
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .font_family(theme.mono_font_family.clone())
                    .child(text.clone()),
            )
            .into_any_element(),
        Cell::Code {
            source,
            count,
            outputs,
        } => {
            let code = fenced(language, source);
            let label = Some(match count {
                Some(count) => format!("[{count}]"),
                None => "[ ]".to_string(),
            });
            v_flex()
                .gap_2()
                .child(
                    h_flex().items_start().child(gutter(label)).child(
                        div().flex_1().min_w_0().child(
                            gpui_kit::base::text::TextView::markdown(("notebook-code", ix), code)
                                .selectable(true),
                        ),
                    ),
                )
                .when(!outputs.is_empty(), |this| {
                    this.child(
                        h_flex().items_start().child(gutter(None)).child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_1()
                                .children(outputs.iter().map(|output| render_output(output, cx))),
                        ),
                    )
                })
                .into_any_element()
        }
    }
}

fn render_output(output: &Output, cx: &App) -> AnyElement {
    let theme = cx.theme();
    match output {
        Output::Text(text) => div()
            .text_sm()
            .font_family(theme.mono_font_family.clone())
            .text_color(theme.foreground.opacity(0.85))
            .child(text.clone())
            .into_any_element(),
        Output::Error(text) => div()
            .px_3()
            .py_2()
            .rounded(theme.radius)
            .bg(theme.danger.opacity(0.08))
            .text_sm()
            .font_family(theme.mono_font_family.clone())
            .text_color(theme.danger)
            .child(text.clone())
            .into_any_element(),
        Output::Image {
            image,
            width,
            height,
        } => {
            let scale = (READING_WIDTH / width).min(1.);
            img(image.clone())
                .w(px(width * scale))
                .h(px(height * scale))
                .into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_cannot_close_its_fence() {
        assert_eq!(fenced("python", "x = 1"), "```python\nx = 1\n```");
        assert_eq!(
            fenced("md", "````\nhi\n```"),
            "`````md\n````\nhi\n```\n`````"
        );
    }

    #[test]
    fn reads_cells_and_outputs() {
        let text = r##"{
          "metadata": {"language_info": {"name": "python"}},
          "cells": [
            {"cell_type": "markdown", "source": ["# Title\n", "text"]},
            {"cell_type": "code", "execution_count": 3, "source": "print(1)",
             "outputs": [
               {"output_type": "stream", "name": "stdout", "text": ["1\n"]},
               {"output_type": "error", "ename": "ValueError", "evalue": "bad", "traceback": ["\u001b[31mValueError\u001b[0m: bad"]}
             ]}
          ]
        }"##;
        let notebook = parse(text).unwrap();
        assert_eq!(notebook.language, "python");
        assert!(
            matches!(&notebook.cells[0], Cell::Markdown(text) if text.as_ref() == "# Title\ntext")
        );
        let Cell::Code { count, outputs, .. } = &notebook.cells[1] else {
            panic!("a code cell")
        };
        assert_eq!(*count, Some(3));
        assert!(matches!(&outputs[0], Output::Text(text) if text.as_ref() == "1\n"));
        assert!(matches!(&outputs[1], Output::Error(text) if text.as_ref() == "ValueError: bad"));
        assert!(parse("{}").is_err());
    }
}

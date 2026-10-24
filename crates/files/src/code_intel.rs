//! Language-server features beyond what the editor does itself: find
//! references, the file outline, and renaming a symbol across the project.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context as _, Result, bail};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState, Rope, RopeExt as _},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use lsp_types::{DocumentSymbolResponse, Location, SymbolKind, TextEdit, WorkspaceEdit};

/// Opens a file at a 0-based line (from a reference, a symbol…).
pub type OpenLocation = Rc<dyn Fn(PathBuf, u32, &mut Window, &mut App)>;

/// One place in the code, ready to show.
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub path: PathBuf,
    pub line: u32,
    /// The line's text, trimmed.
    pub preview: String,
}

/// Places for `locations`, with each line's text read from disk.
pub fn places(locations: &[Location]) -> Vec<Place> {
    let mut files: HashMap<PathBuf, Vec<String>> = HashMap::new();
    locations
        .iter()
        .filter_map(|location| {
            let path = ph_lsp::uri::to_path(&location.uri)?;
            let lines = files.entry(path.clone()).or_insert_with(|| {
                std::fs::read_to_string(&path)
                    .map(|text| text.lines().map(str::to_string).collect())
                    .unwrap_or_default()
            });
            let line = location.range.start.line;
            Some(Place {
                preview: lines
                    .get(line as usize)
                    .map(|text| text.trim().to_string())
                    .unwrap_or_default(),
                path,
                line,
            })
        })
        .collect()
}

/// A dialog listing places to jump to (references, search hits).
pub fn open_places(
    title: String,
    root: &Path,
    places: Vec<Place>,
    open: OpenLocation,
    window: &mut Window,
    cx: &mut App,
) {
    let root = root.to_path_buf();
    window.open_dialog(cx, move |dialog, _, cx| {
        let rows: Vec<AnyElement> = places
            .iter()
            .enumerate()
            .map(|(ix, place)| {
                let (path, line, open) = (place.path.clone(), place.line, open.clone());
                let shown = path
                    .strip_prefix(&root)
                    .unwrap_or(&path)
                    .display()
                    .to_string();
                let (icon, tint) = ph_ui::file_icon(&shown);
                v_flex()
                    .id(("place", ix))
                    .px_2()
                    .py_1()
                    .rounded(cx.theme().radius)
                    .cursor_pointer()
                    .hover(|style| style.bg(cx.theme().list_hover))
                    .on_click(move |_, window, cx| {
                        window.close_dialog(cx);
                        open(path.clone(), line, window, cx)
                    })
                    .child(
                        h_flex()
                            .gap_1p5()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(Icon::new(icon).xsmall().text_color(tint))
                            .child(format!("{shown}:{}", line + 1)),
                    )
                    .child(
                        div()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_sm()
                            .truncate()
                            .child(place.preview.clone()),
                    )
                    .into_any_element()
            })
            .collect();
        let empty = rows.is_empty();
        dialog.title(title.clone()).w(px(720.)).child(
            v_flex()
                .id("places")
                .max_h(px(520.))
                .overflow_y_scroll()
                .gap_0p5()
                .when(empty, |this| {
                    this.child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("Nothing found."),
                    )
                })
                .children(rows),
        )
    });
}

/// A symbol of the outline, flattened with its depth.
#[derive(Debug, Clone, PartialEq)]
pub struct Symbol {
    pub name: String,
    pub detail: String,
    pub kind: SymbolKind,
    pub line: u32,
    pub depth: usize,
}

/// The outline as a flat list (nested symbols indented).
pub fn outline(response: DocumentSymbolResponse) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    match response {
        DocumentSymbolResponse::Flat(list) => {
            for symbol in list {
                symbols.push(Symbol {
                    name: symbol.name,
                    detail: symbol.container_name.unwrap_or_default(),
                    kind: symbol.kind,
                    line: symbol.location.range.start.line,
                    depth: 0,
                });
            }
        }
        DocumentSymbolResponse::Nested(list) => {
            fn walk(list: Vec<lsp_types::DocumentSymbol>, depth: usize, out: &mut Vec<Symbol>) {
                for symbol in list {
                    out.push(Symbol {
                        name: symbol.name,
                        detail: symbol.detail.unwrap_or_default(),
                        kind: symbol.kind,
                        line: symbol.selection_range.start.line,
                        depth,
                    });
                    walk(symbol.children.unwrap_or_default(), depth + 1, out);
                }
            }
            walk(list, 0, &mut symbols);
        }
    }
    symbols
}

fn kind_label(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::FUNCTION | SymbolKind::METHOD | SymbolKind::CONSTRUCTOR => "fn",
        SymbolKind::STRUCT | SymbolKind::CLASS | SymbolKind::OBJECT => "type",
        SymbolKind::ENUM | SymbolKind::ENUM_MEMBER => "enum",
        SymbolKind::INTERFACE => "trait",
        SymbolKind::CONSTANT | SymbolKind::VARIABLE | SymbolKind::FIELD | SymbolKind::PROPERTY => {
            "var"
        }
        SymbolKind::MODULE | SymbolKind::NAMESPACE | SymbolKind::PACKAGE => "mod",
        _ => "·",
    }
}

/// The outline in a dialog; picking a symbol jumps to it.
pub fn open_outline(
    path: PathBuf,
    symbols: Vec<Symbol>,
    open: OpenLocation,
    window: &mut Window,
    cx: &mut App,
) {
    window.open_dialog(cx, move |dialog, _, cx| {
        let rows: Vec<AnyElement> = symbols
            .iter()
            .enumerate()
            .map(|(ix, symbol)| {
                let (path, line, open) = (path.clone(), symbol.line, open.clone());
                h_flex()
                    .id(("symbol", ix))
                    .gap_2()
                    .pl(px(8. + 14. * symbol.depth as f32))
                    .pr_2()
                    .py_0p5()
                    .rounded(cx.theme().radius)
                    .cursor_pointer()
                    .hover(|style| style.bg(cx.theme().list_hover))
                    .on_click(move |_, window, cx| {
                        window.close_dialog(cx);
                        open(path.clone(), line, window, cx)
                    })
                    .child(
                        div()
                            .w(px(36.))
                            .flex_none()
                            .text_xs()
                            .text_color(cx.theme().info)
                            .font_family(cx.theme().mono_font_family.clone())
                            .child(kind_label(symbol.kind)),
                    )
                    .child(div().text_sm().child(symbol.name.clone()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(symbol.detail.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{}", symbol.line + 1)),
                    )
                    .into_any_element()
            })
            .collect();
        dialog.title("Outline").w(px(600.)).child(
            v_flex()
                .id("outline")
                .max_h(px(520.))
                .overflow_y_scroll()
                .children(rows),
        )
    });
}

/// What [`ask_new_name`] calls with the chosen name.
pub type OnRename = Rc<dyn Fn(String, &mut Window, &mut App)>;

/// Asks for the new name, then calls `rename` with it.
pub fn ask_new_name(current: String, rename: OnRename, window: &mut Window, cx: &mut App) {
    let input = cx.new(|cx| {
        let mut input = InputState::new(window, cx);
        input.set_value(current.clone(), window, cx);
        input
    });
    let focus = input.clone();
    window.defer(cx, move |window, cx| {
        focus.update(cx, |input, cx| input.focus(window, cx))
    });
    window.open_dialog(cx, move |dialog, _, _| {
        let (input, rename) = (input.clone(), rename.clone());
        dialog.title("Rename symbol").w(px(420.)).child(
            v_flex().gap_2().child(Input::new(&input)).child(
                h_flex().justify_end().child(
                    Button::new("rename-confirm")
                        .small()
                        .primary()
                        .icon(IconName::Check)
                        .label("Rename")
                        .on_click(move |_, window, cx| {
                            let name = input.read(cx).value().trim().to_string();
                            window.close_dialog(cx);
                            if !name.is_empty() {
                                rename(name, window, cx);
                            }
                        }),
                ),
            ),
        )
    });
}

/// Applies a rename's edits: the open file's text in memory (returned),
/// other files on disk. Returns the new text of `open_path` if it changed,
/// and how many files were edited.
pub fn apply_edit(
    edit: WorkspaceEdit,
    open_path: &Path,
    open_text: &Rope,
) -> Result<(Option<String>, usize)> {
    let mut by_file: HashMap<PathBuf, Vec<TextEdit>> = HashMap::new();
    for (uri, edits) in edit.changes.unwrap_or_default() {
        if let Some(path) = ph_lsp::uri::to_path(&uri) {
            by_file.entry(path).or_default().extend(edits);
        }
    }
    if let Some(lsp_types::DocumentChanges::Edits(documents)) = edit.document_changes {
        for document in documents {
            if let Some(path) = ph_lsp::uri::to_path(&document.text_document.uri) {
                by_file
                    .entry(path)
                    .or_default()
                    .extend(document.edits.into_iter().map(|edit| match edit {
                        lsp_types::OneOf::Left(edit) => edit,
                        lsp_types::OneOf::Right(annotated) => annotated.text_edit,
                    }));
            }
        }
    }
    // A file with unsaved changes in another tab would overwrite the
    // rename when saved: none is changed until those are saved.
    let unsaved: Vec<String> = by_file
        .keys()
        .filter(|path| !same_file(path, open_path) && crate::unsaved::kept(path).is_some())
        .map(|path| {
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    if !unsaved.is_empty() {
        bail!(
            "save {} first: the rename changes it too",
            unsaved.join(", ")
        );
    }
    let mut open_result = None;
    let files = by_file.len();
    for (path, edits) in by_file {
        if same_file(&path, open_path) {
            open_result = Some(apply_text_edits(open_text.clone(), edits));
        } else {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let updated = apply_text_edits(Rope::from(text.as_str()), edits);
            std::fs::write(&path, updated)
                .with_context(|| format!("writing {}", path.display()))?;
        }
    }
    Ok((open_result, files))
}

/// The same file, however the server spelled the path (case only where the
/// file system ignores it: the real paths are compared).
fn same_file(a: &Path, b: &Path) -> bool {
    a == b
        || std::fs::canonicalize(a)
            .ok()
            .zip(std::fs::canonicalize(b).ok())
            .is_some_and(|(a, b)| a == b)
}

/// Applies a server's edits to `text`, from last to first, so earlier
/// positions stay valid.
pub fn apply_text_edits(mut text: Rope, mut edits: Vec<TextEdit>) -> String {
    for edit in &mut edits {
        edit.range = ph_lsp::position::range_to_editor(&text, edit.range);
    }
    edits
        .sort_by_key(|edit| std::cmp::Reverse((edit.range.start.line, edit.range.start.character)));
    for edit in edits {
        let start = text.position_to_offset(&edit.range.start);
        let end = text.position_to_offset(&edit.range.end);
        text.replace(start..end.max(start), &edit.new_text);
    }
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{Position, Range};

    fn edit(line: u32, from: u32, to: u32, text: &str) -> TextEdit {
        TextEdit {
            range: Range {
                start: Position {
                    line,
                    character: from,
                },
                end: Position {
                    line,
                    character: to,
                },
            },
            new_text: text.into(),
        }
    }

    #[test]
    fn applies_edits_back_to_front() {
        let text = Rope::from("let count = 1;\nprint(count, count)\n");
        let edits = vec![
            edit(0, 4, 9, "total"),
            edit(1, 6, 11, "total"),
            edit(1, 13, 18, "total"),
        ];
        assert_eq!(
            apply_text_edits(text, edits),
            "let total = 1;\nprint(total, total)\n"
        );
    }

    #[test]
    fn flattens_nested_outlines() {
        #[allow(deprecated)]
        let child = lsp_types::DocumentSymbol {
            name: "run".into(),
            detail: Some("fn()".into()),
            kind: SymbolKind::METHOD,
            tags: None,
            deprecated: None,
            range: Range::default(),
            selection_range: Range {
                start: Position {
                    line: 4,
                    character: 0,
                },
                end: Position {
                    line: 4,
                    character: 3,
                },
            },
            children: None,
        };
        #[allow(deprecated)]
        let parent = lsp_types::DocumentSymbol {
            name: "App".into(),
            detail: None,
            kind: SymbolKind::STRUCT,
            tags: None,
            deprecated: None,
            range: Range::default(),
            selection_range: Range::default(),
            children: Some(vec![child]),
        };
        let flat = outline(DocumentSymbolResponse::Nested(vec![parent]));
        assert_eq!(
            flat.iter()
                .map(|s| (s.name.as_str(), s.depth, s.line))
                .collect::<Vec<_>>(),
            [("App", 0, 0), ("run", 1, 4)]
        );
    }
}

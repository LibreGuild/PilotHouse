//! JSON, JSON Lines, YAML and TOML as a tree: fold and unfold, pick a value
//! to see its path, copy the path or the value.

use std::collections::HashSet;
use std::ops::Range;
use std::sync::Arc;

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, ClipboardItem, Context, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Task,
    UniformListScrollHandle, Window, div, px, uniform_list,
};

use crate::Kind;
use crate::json::{self, Node};

/// Levels open when a file is first shown.
const OPEN_LEVELS: usize = 2;
const ROW_HEIGHT: f32 = 22.;
const INDENT: f32 = 16.;

/// One visible row.
#[derive(Debug, Clone)]
struct Row {
    depth: usize,
    key: Option<String>,
    /// JSONPath of the value (`$.a[0]`).
    path: String,
    kind: ValueKind,
    /// The value, or `{3}` / `[5]` for a folder.
    text: String,
    expandable: bool,
    expanded: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ValueKind {
    Null,
    Bool,
    Number,
    String,
    Array,
    Object,
}

pub struct TreePreview {
    kind: Kind,
    root: Option<Result<Arc<Node>, SharedString>>,
    /// Paths unfolded; `None` until the first parse sets the opening levels.
    expanded: Option<HashSet<String>>,
    rows: Vec<Row>,
    selected: Option<String>,
    scroll: UniformListScrollHandle,
    _parse: Option<Task<()>>,
}

impl TreePreview {
    pub fn new(kind: Kind, _: &mut Context<Self>) -> Self {
        Self {
            kind,
            root: None,
            expanded: None,
            rows: Vec::new(),
            selected: None,
            scroll: UniformListScrollHandle::new(),
            _parse: None,
        }
    }

    pub fn set_text(&mut self, text: Arc<str>, cx: &mut Context<Self>) {
        let kind = self.kind;
        let parse = cx.background_spawn(async move {
            match kind {
                Kind::JsonLines => Ok(json::parse_json_lines(&text)),
                Kind::Yaml => json::parse_yaml(&text),
                Kind::Toml => json::parse_toml(&text),
                _ => json::parse_json(&text),
            }
        });
        self._parse = Some(cx.spawn(async move |this, cx| {
            let parsed = parse.await;
            _ = this.update(cx, |this, cx| {
                match parsed {
                    Ok(node) => {
                        let node = Arc::new(node);
                        if this.expanded.is_none() {
                            let mut open = HashSet::new();
                            open_levels(&node, "$", 0, &mut open);
                            this.expanded = Some(open);
                        }
                        this.root = Some(Ok(node));
                    }
                    // A file being typed is often invalid for a moment:
                    // the last good tree stays, with the reason.
                    Err(err) => match &this.root {
                        Some(Ok(_)) => {}
                        _ => this.root = Some(Err(err.into())),
                    },
                }
                this.rebuild();
                cx.notify();
            });
        }));
    }

    fn rebuild(&mut self) {
        self.rows.clear();
        let (Some(Ok(root)), Some(expanded)) = (&self.root, &self.expanded) else {
            return;
        };
        let root = root.clone();
        flatten(&root, None, "$".into(), 0, expanded, &mut self.rows);
    }

    fn toggle(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(ix) else {
            return;
        };
        self.selected = Some(row.path.clone());
        if row.expandable
            && let Some(expanded) = &mut self.expanded
            && !expanded.remove(&row.path)
        {
            expanded.insert(row.path.clone());
        }
        self.rebuild();
        cx.notify();
    }

    fn set_all(&mut self, open: bool, cx: &mut Context<Self>) {
        let mut expanded = HashSet::new();
        if open && let Some(Ok(root)) = &self.root {
            open_all(root, "$", &mut expanded);
        } else {
            expanded.insert("$".to_string());
        }
        self.expanded = Some(expanded);
        self.rebuild();
        cx.notify();
    }

    fn selected_value(&self) -> Option<String> {
        let (Some(Ok(root)), Some(path)) = (&self.root, &self.selected) else {
            return None;
        };
        find(root, "$", path).map(|node| match node {
            Node::String(text) => text.clone(),
            other => to_json(other),
        })
    }
}

/// Unfolds the first [`OPEN_LEVELS`] levels below `depth`.
fn open_levels(node: &Node, path: &str, depth: usize, open: &mut HashSet<String>) {
    if depth >= OPEN_LEVELS {
        return;
    }
    open.insert(path.to_string());
    for (child_path, child) in children(node, path) {
        open_levels(child, &child_path, depth + 1, open);
    }
}

fn open_all(node: &Node, path: &str, open: &mut HashSet<String>) {
    open.insert(path.to_string());
    for (child_path, child) in children(node, path) {
        open_all(child, &child_path, open);
    }
}

fn children<'a>(node: &'a Node, path: &str) -> Vec<(String, &'a Node)> {
    match node {
        Node::Array(items) => items
            .iter()
            .enumerate()
            .map(|(ix, item)| (format!("{path}[{ix}]"), item))
            .collect(),
        Node::Object(entries) => entries
            .iter()
            .map(|(key, value)| (child_path(path, key), value))
            .collect(),
        _ => Vec::new(),
    }
}

/// `$.name`, or `$["odd key"]` for keys that are not identifiers.
fn child_path(path: &str, key: &str) -> String {
    let plain = !key.is_empty()
        && !key.starts_with(|c: char| c.is_ascii_digit())
        && key
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '$');
    if plain {
        format!("{path}.{key}")
    } else {
        format!("{path}[{}]", serde_json::Value::String(key.to_string()))
    }
}

fn flatten(
    node: &Node,
    key: Option<String>,
    path: String,
    depth: usize,
    expanded: &HashSet<String>,
    rows: &mut Vec<Row>,
) {
    let (kind, text, count) = match node {
        Node::Null => (ValueKind::Null, "null".to_string(), None),
        Node::Bool(value) => (ValueKind::Bool, value.to_string(), None),
        Node::Number(text) => (ValueKind::Number, text.clone(), None),
        Node::String(text) => (ValueKind::String, format!("\"{}\"", shorten(text)), None),
        Node::Array(items) => (
            ValueKind::Array,
            format!("[{}]", items.len()),
            Some(items.len()),
        ),
        Node::Object(entries) => (
            ValueKind::Object,
            format!("{{{}}}", entries.len()),
            Some(entries.len()),
        ),
    };
    let expandable = count.is_some_and(|count| count > 0);
    let open = expandable && expanded.contains(&path);
    rows.push(Row {
        depth,
        key,
        path: path.clone(),
        kind,
        text,
        expandable,
        expanded: open,
    });
    if !open {
        return;
    }
    match node {
        Node::Array(items) => {
            for (ix, item) in items.iter().enumerate() {
                let child = format!("{path}[{ix}]");
                flatten(item, Some(ix.to_string()), child, depth + 1, expanded, rows);
            }
        }
        Node::Object(entries) => {
            for (name, value) in entries {
                let child = child_path(&path, name);
                flatten(value, Some(name.clone()), child, depth + 1, expanded, rows);
            }
        }
        _ => {}
    }
}

fn shorten(text: &str) -> String {
    const MAX: usize = 200;
    let one_line = text.replace('\n', "⏎");
    match one_line.char_indices().nth(MAX) {
        Some((at, _)) => format!("{}…", &one_line[..at]),
        None => one_line,
    }
}

fn find<'a>(node: &'a Node, path: &str, wanted: &str) -> Option<&'a Node> {
    if path == wanted {
        return Some(node);
    }
    children(node, path)
        .into_iter()
        .filter(|(child, _)| wanted.starts_with(child.as_str()))
        .find_map(|(child, value)| find(value, &child, wanted))
}

fn to_json(node: &Node) -> String {
    match node {
        Node::Null => "null".into(),
        Node::Bool(value) => value.to_string(),
        Node::Number(text) => text.clone(),
        Node::String(text) => serde_json::Value::String(text.clone()).to_string(),
        Node::Array(items) => format!(
            "[{}]",
            items.iter().map(to_json).collect::<Vec<_>>().join(",")
        ),
        Node::Object(entries) => format!(
            "{{{}}}",
            entries
                .iter()
                .map(|(key, value)| format!(
                    "{}:{}",
                    serde_json::Value::String(key.clone()),
                    to_json(value)
                ))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

fn value_color(kind: ValueKind, cx: &App) -> Hsla {
    let theme = cx.theme();
    match kind {
        ValueKind::String => theme.success,
        ValueKind::Number => theme.info,
        ValueKind::Bool | ValueKind::Null => theme.warning,
        ValueKind::Array | ValueKind::Object => theme.muted_foreground,
    }
}

impl Render for TreePreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let error = match &self.root {
            None => return crate::notice("Reading…", cx),
            Some(Err(err)) => return crate::notice(format!("Not valid: {err}"), cx),
            Some(Ok(_)) => None::<SharedString>,
        };
        let _ = error;
        let theme = cx.theme();
        let selected = self.selected.clone();
        let count = self.rows.len();
        let list = uniform_list(
            "data-tree",
            count,
            cx.processor(move |this, range: Range<usize>, _, cx| {
                range
                    .filter_map(|ix| {
                        let row = this.rows.get(ix)?.clone();
                        Some(render_row(ix, row, this.selected.as_deref(), cx))
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.scroll)
        .size_full()
        .py_1();
        let value = self.selected_value();
        v_flex()
            .size_full()
            .bg(theme.background)
            .child(
                h_flex()
                    .flex_none()
                    .h_8()
                    .px_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(theme.border)
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(div().flex_1().child(format!("{count} rows shown")))
                    .child(
                        Button::new("tree-expand-all")
                            .ghost()
                            .xsmall()
                            .label("Expand all")
                            .on_click(cx.listener(|this, _, _, cx| this.set_all(true, cx))),
                    )
                    .child(
                        Button::new("tree-collapse-all")
                            .ghost()
                            .xsmall()
                            .label("Collapse all")
                            .on_click(cx.listener(|this, _, _, cx| this.set_all(false, cx))),
                    ),
            )
            .child(div().flex_1().min_h_0().child(list))
            .when_some(selected, |this, path| {
                let copy_path = path.clone();
                this.child(
                    h_flex()
                        .flex_none()
                        .h_8()
                        .px_2()
                        .gap_1()
                        .border_t_1()
                        .border_color(theme.border)
                        .text_xs()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_family(theme.mono_font_family.clone())
                                .child(path),
                        )
                        .child(
                            Button::new("tree-copy-path")
                                .ghost()
                                .xsmall()
                                .icon(IconName::Copy)
                                .label("Path")
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        copy_path.clone(),
                                    ))
                                }),
                        )
                        .when_some(value, |this, value| {
                            this.child(
                                Button::new("tree-copy-value")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Copy)
                                    .label("Value")
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            value.clone(),
                                        ))
                                    }),
                            )
                        }),
                )
            })
            .into_any_element()
    }
}

fn render_row(
    ix: usize,
    row: Row,
    selected: Option<&str>,
    cx: &mut Context<TreePreview>,
) -> gpui_kit::AnyElement {
    let theme = cx.theme();
    let is_selected = selected == Some(row.path.as_str());
    let chevron = if row.expandable {
        Icon::new(if row.expanded {
            IconName::ChevronDown
        } else {
            IconName::ChevronRight
        })
        .xsmall()
        .text_color(theme.muted_foreground)
        .into_any_element()
    } else {
        div().w(px(12.)).into_any_element()
    };
    h_flex()
        .id(("tree-row", ix))
        .h(px(ROW_HEIGHT))
        .pl(px(8. + INDENT * row.depth as f32))
        .pr_2()
        .gap_1()
        .text_sm()
        .font_family(theme.mono_font_family.clone())
        .cursor_pointer()
        .when(is_selected, |this| this.bg(theme.list_active))
        .when(!is_selected, |this| {
            this.hover(|this| this.bg(theme.list_hover))
        })
        .child(chevron)
        .when_some(row.key.clone(), |this, key| {
            this.child(div().flex_none().text_color(theme.foreground).child(key))
                .child(
                    div()
                        .flex_none()
                        .text_color(theme.muted_foreground)
                        .child(":"),
                )
        })
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_color(value_color(row.kind, cx))
                .child(row.text.clone()),
        )
        .on_click(cx.listener(move |this, _, _, cx| this.toggle(ix, cx)))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_and_rows() {
        let node = json::parse_json(r#"{"a":{"b":[1,2]},"odd key":true}"#).unwrap();
        let mut open = HashSet::new();
        open_levels(&node, "$", 0, &mut open);
        let mut rows = Vec::new();
        flatten(&node, None, "$".into(), 0, &open, &mut rows);
        let paths: Vec<&str> = rows.iter().map(|row| row.path.as_str()).collect();
        assert_eq!(paths, ["$", "$.a", "$.a.b", "$[\"odd key\"]"]);
        assert_eq!(rows[2].text, "[2]");
        assert!(matches!(find(&node, "$", "$.a.b[1]"), Some(Node::Number(n)) if n == "2"));
        assert_eq!(to_json(&node), r#"{"a":{"b":[1,2]},"odd key":true}"#);
    }
}

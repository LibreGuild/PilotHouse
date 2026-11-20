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
    App, AppContext as _, Context, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Task,
    UniformListScrollHandle, Window, div, uniform_list,
};

use crate::Kind;
use crate::json::{self, Node};

/// Levels open when a file is first shown.
const OPEN_LEVELS: usize = 2;
const ROW_HEIGHT: f32 = 22.;
const INDENT: f32 = 16.;
/// "Expand all" unfolds values while the rows stay under this many: a
/// large file has millions of values, and a row each is hundreds of MB.
const UNFOLD_ROWS: usize = 100_000;

/// One visible row.
#[derive(Debug, Clone, PartialEq)]
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
    /// Shared with the task that lays out the rows of a text just read.
    expanded: Option<Arc<HashSet<String>>>,
    rows: Vec<Row>,
    selected: Option<String>,
    /// "Expand all" left values folded: there were too many rows.
    limited: bool,
    scroll: UniformListScrollHandle,
    _parse: Option<Task<()>>,
    _unfold: Option<Task<()>>,
}

impl TreePreview {
    pub fn new(kind: Kind, _: &mut Context<Self>) -> Self {
        Self {
            kind,
            root: None,
            expanded: None,
            rows: Vec::new(),
            selected: None,
            limited: false,
            scroll: UniformListScrollHandle::new(),
            _parse: None,
            _unfold: None,
        }
    }

    pub fn set_text(&mut self, text: Arc<str>, cx: &mut Context<Self>) {
        let kind = self.kind;
        let unfolded = self.expanded.clone();
        // Read and laid out as rows off the UI thread: a large file has
        // hundreds of thousands of rows.
        let parse = cx.background_spawn(async move {
            let node = match kind {
                Kind::JsonLines => Ok(json::parse_json_lines(&text)),
                Kind::Yaml => json::parse_yaml(&text),
                Kind::Toml => json::parse_toml(&text),
                _ => json::parse_json(&text),
            }?;
            let unfolded = unfolded.unwrap_or_else(|| {
                let mut open = HashSet::new();
                open_levels(&node, "$", 0, &mut open);
                Arc::new(open)
            });
            let rows = all_rows(&node, &unfolded);
            Ok::<_, String>((Arc::new(node), unfolded, rows))
        });
        self._parse = Some(cx.spawn(async move |this, cx| {
            let parsed = parse.await;
            _ = this.update(cx, |this, cx| {
                match parsed {
                    Ok((node, unfolded, rows)) => {
                        let now = this.expanded.get_or_insert_with(|| unfolded.clone());
                        let same = Arc::ptr_eq(now, &unfolded);
                        // What was picked may be gone from the new text.
                        if let Some(path) = &this.selected
                            && find(&node, "$", path).is_none()
                        {
                            this.selected = None;
                        }
                        this.root = Some(Ok(node));
                        // Folded or unfolded while it was read: the rows
                        // are laid out again, as things are now.
                        if same {
                            this.rows = rows;
                        } else {
                            this.rebuild();
                        }
                    }
                    // A file being typed is often invalid for a moment:
                    // the last good tree stays, with the reason.
                    Err(err) => match &this.root {
                        Some(Ok(_)) => {}
                        _ => this.root = Some(Err(err.into())),
                    },
                }
                cx.notify();
            });
        }));
    }

    fn rebuild(&mut self) {
        self.rows = match (&self.root, &self.expanded) {
            (Some(Ok(root)), Some(expanded)) => all_rows(root, expanded),
            _ => Vec::new(),
        };
    }

    fn toggle(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(ix) else {
            return;
        };
        self.selected = Some(row.path.clone());
        if row.expandable
            && let (Some(Ok(root)), Some(expanded)) = (&self.root, &mut self.expanded)
        {
            toggle_rows(root, &mut self.rows, Arc::make_mut(expanded), ix);
            self.limited = false;
        }
        cx.notify();
    }

    fn set_all(&mut self, open: bool, cx: &mut Context<Self>) {
        let Some(Ok(root)) = &self.root else {
            return;
        };
        let root = root.clone();
        // Off the UI thread: every row is laid out again.
        let work = cx.background_spawn({
            let root = root.clone();
            async move {
                let (expanded, limited) = if open {
                    open_all(&root, UNFOLD_ROWS)
                } else {
                    (HashSet::from(["$".to_string()]), false)
                };
                let rows = all_rows(&root, &expanded);
                (expanded, limited, rows)
            }
        });
        self._unfold = Some(cx.spawn(async move |this, cx| {
            let (expanded, limited, rows) = work.await;
            _ = this.update(cx, |this, cx| {
                // Only for the tree it was worked out for.
                if matches!(&this.root, Some(Ok(now)) if Arc::ptr_eq(now, &root)) {
                    this.expanded = Some(Arc::new(expanded));
                    this.rows = rows;
                    this.limited = limited;
                    cx.notify();
                }
            });
        }));
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

/// How many values `node` holds (none, unless it is a list or an object).
fn count(node: &Node) -> usize {
    match node {
        Node::Array(items) => items.len(),
        Node::Object(entries) => entries.len(),
        _ => 0,
    }
}

/// Every value unfolded, in the file's order, while the rows stay under
/// `rows` (the top level always is unfolded); and whether some values had
/// to stay folded.
fn open_all(root: &Node, rows: usize) -> (HashSet<String>, bool) {
    let mut open = HashSet::new();
    let mut room = rows.saturating_sub(1);
    let mut limited = false;
    open_below(root, "$", true, &mut room, &mut limited, &mut open);
    (open, limited)
}

fn open_below(
    node: &Node,
    path: &str,
    always: bool,
    room: &mut usize,
    limited: &mut bool,
    open: &mut HashSet<String>,
) {
    // Unfolded, it shows a row more for each value it holds.
    let inside = count(node);
    if inside == 0 {
        return;
    }
    if inside > *room && !always {
        *limited = true;
        return;
    }
    *room = room.saturating_sub(inside);
    open.insert(path.to_string());
    // A path is only made for what can be unfolded.
    match node {
        Node::Array(items) => {
            for (ix, item) in items.iter().enumerate() {
                if count(item) > 0 {
                    open_below(item, &format!("{path}[{ix}]"), false, room, limited, open);
                }
            }
        }
        Node::Object(entries) => {
            for (key, value) in entries {
                if count(value) > 0 {
                    open_below(value, &child_path(path, key), false, room, limited, open);
                }
            }
        }
        _ => {}
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

/// All the rows of `root`, with `expanded` unfolded.
fn all_rows(root: &Node, expanded: &HashSet<String>) -> Vec<Row> {
    let mut rows = Vec::new();
    flatten(root, None, "$".into(), 0, expanded, &mut rows);
    rows
}

/// The value row `ix` shows, found by its place under each row above it
/// (not by its path: an object can hold a key twice).
fn node_at<'a>(root: &'a Node, rows: &[Row], ix: usize) -> Option<&'a Node> {
    // Its place among its parent's values, then the parent's, up to the top.
    let mut places = Vec::new();
    let (mut at, mut depth) = (ix, rows.get(ix)?.depth);
    while depth > 0 {
        let mut place = 0;
        loop {
            at = at.checked_sub(1)?;
            match rows[at].depth {
                above if above < depth => break,
                same if same == depth => place += 1,
                _ => {}
            }
        }
        places.push(place);
        depth -= 1;
    }
    let mut node = root;
    for place in places.into_iter().rev() {
        node = match node {
            Node::Array(items) => items.get(place)?,
            Node::Object(entries) => &entries.get(place)?.1,
            _ => return None,
        };
    }
    Some(node)
}

/// Folds or unfolds row `ix`: only its own rows are made or dropped. (All
/// the rows were once laid out again at every click, hundreds of thousands
/// in a large file.)
fn toggle_rows(root: &Node, rows: &mut Vec<Row>, expanded: &mut HashSet<String>, ix: usize) {
    let (depth, path) = (rows[ix].depth, rows[ix].path.clone());
    let open = !rows[ix].expanded;
    let mut inside = Vec::new();
    if open {
        expanded.insert(path.clone());
        if let Some(node) = node_at(root, rows, ix) {
            flatten_children(node, &path, depth, expanded, &mut inside);
        }
    } else {
        expanded.remove(&path);
    }
    rows[ix].expanded = open;
    // Its rows are those after it that are deeper.
    let end = rows[ix + 1..]
        .iter()
        .position(|row| row.depth <= depth)
        .map_or(rows.len(), |after| ix + 1 + after);
    rows.splice(ix + 1..end, inside);
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
    if open {
        flatten_children(node, &path, depth, expanded, rows);
    }
}

/// The rows of the values `node` (at `path` and `depth`) holds.
fn flatten_children(
    node: &Node,
    path: &str,
    depth: usize,
    expanded: &HashSet<String>,
    rows: &mut Vec<Row>,
) {
    match node {
        Node::Array(items) => {
            for (ix, item) in items.iter().enumerate() {
                let child = format!("{path}[{ix}]");
                flatten(item, Some(ix.to_string()), child, depth + 1, expanded, rows);
            }
        }
        Node::Object(entries) => {
            for (name, value) in entries {
                let child = child_path(path, name);
                flatten(value, Some(name.clone()), child, depth + 1, expanded, rows);
            }
        }
        _ => {}
    }
}

fn shorten(text: &str) -> String {
    const MAX: usize = 200;
    // Cut first: a long text is not copied whole to show its start.
    let (start, more) = match text.char_indices().nth(MAX) {
        Some((at, _)) => (&text[..at], true),
        None => (text, false),
    };
    let mut one_line = start.replace('\n', "⏎");
    if more {
        one_line.push('…');
    }
    one_line
}

fn find<'a>(node: &'a Node, path: &str, wanted: &str) -> Option<&'a Node> {
    if path == wanted {
        return Some(node);
    }
    // A list's item is found by its number, not by making every item's
    // path to compare (a list can have hundreds of thousands).
    if let Node::Array(items) = node {
        let (index, _) = wanted
            .strip_prefix(path)?
            .strip_prefix('[')?
            .split_once(']')?;
        let index: usize = index.parse().ok()?;
        return find(items.get(index)?, &format!("{path}[{index}]"), wanted);
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
        let rows_shown = if self.limited {
            format!("{count} rows shown · too many to unfold them all")
        } else {
            format!("{count} rows shown")
        };
        let view = cx.weak_entity();
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
                    .child(div().flex_1().child(rows_shown))
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
                            ph_ui::CopyButton::new("tree-copy-path", copy_path.clone())
                                .xsmall()
                                .label("Path"),
                        )
                        // The value's text is made at the click: a large
                        // value's is megabytes, and this draws every frame.
                        .child(
                            ph_ui::CopyButton::with("tree-copy-value", move |_, cx| {
                                view.read_with(cx, |this, _| this.selected_value())
                                    .ok()
                                    .flatten()
                                    .unwrap_or_default()
                            })
                            .xsmall()
                            .label("Value"),
                        ),
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
        div().w(ph_ui::rems_from_px(12.)).into_any_element()
    };
    h_flex()
        .id(("tree-row", ix))
        .h(ph_ui::rems_from_px(ROW_HEIGHT))
        .pl(ph_ui::rems_from_px(8. + INDENT * row.depth as f32))
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

    #[test]
    fn a_click_changes_only_its_rows() {
        // A key twice, lists in lists, and something empty.
        let node = json::parse_json(
            r#"{"a":{"b":[1,[2,3],{"c":null}]},"a":{"d":[4]},"e":[],"f":[[5],[6,7]]}"#,
        )
        .unwrap();
        let mut expanded = HashSet::new();
        open_levels(&node, "$", 0, &mut expanded);
        let mut rows = all_rows(&node, &expanded);
        // The second `a` is found by its place, not its path.
        let second = rows.iter().rposition(|row| row.path == "$.a").unwrap();
        assert!(matches!(
            node_at(&node, &rows, second),
            Some(Node::Object(entries)) if entries[0].0 == "d"
        ));
        // A list's items are found by their numbers.
        assert!(matches!(find(&node, "$", "$.f[1][0]"), Some(Node::Number(n)) if n == "6"));
        assert!(find(&node, "$", "$.f[2]").is_none() && find(&node, "$", "$.f[01]").is_none());
        // Every row that folds, clicked until it has been both ways and is
        // unfolded (so that what it holds is clicked too): the rows are
        // what laying them all out gives. (Not the twice-used key: both of
        // its rows follow the one path.)
        let mut ix = 0;
        let mut clicks = 0;
        while ix < rows.len() {
            if rows[ix].expandable && rows[ix].path != "$.a" {
                for _ in 0..if rows[ix].expanded { 2 } else { 3 } {
                    toggle_rows(&node, &mut rows, &mut expanded, ix);
                    assert_eq!(rows, all_rows(&node, &expanded), "row {ix}");
                    clicks += 1;
                }
            }
            ix += 1;
        }
        assert!(clicks >= 15, "{clicks}");
    }

    #[test]
    fn expand_all_stops_at_too_many_rows() {
        let node = json::parse_json(r#"{"a":[1,2,3],"b":{"c":[4,5,6,7]},"d":[8]}"#).unwrap();
        let (open, limited) = open_all(&node, 1_000);
        assert!(!limited);
        assert_eq!(all_rows(&node, &open).len(), 13);
        // Room for the top level, `a`'s values and `b`'s one: `c` stays folded.
        let (open, limited) = open_all(&node, 9);
        assert!(limited);
        let mut paths: Vec<&str> = open.iter().map(String::as_str).collect();
        paths.sort();
        assert_eq!(paths, ["$", "$.a", "$.b", "$.d"]);
        // The top level is unfolded whatever it holds.
        let (open, limited) = open_all(&node, 1);
        assert!(limited && open.contains("$") && open.len() == 1);
    }

    #[test]
    fn long_text_is_cut_to_a_line() {
        assert_eq!(shorten("a\nb"), "a⏎b");
        let long = "é\n".repeat(150);
        let short = shorten(&long);
        assert_eq!(short.chars().count(), 201);
        assert!(short.starts_with("é⏎é⏎") && short.ends_with("⏎…"));
    }
}

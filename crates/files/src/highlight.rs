//! Syntax highlighting that keeps up with typing on a slow computer.
//!
//! The library's highlighter colours a file with syntect, which cannot
//! resume part way, so every keystroke reparsed the whole file (up to
//! 64 KB) before the character showed: a few milliseconds on a fast Mac,
//! tens on a slow PC, where typing lagged behind in bursts.
//!
//! This one works like VS Code's tokenizer. Each line keeps the parser's
//! state at its start and its own coloured spans. An edit reparses from its
//! first line and stops as soon as a following line starts in the state it
//! started in before, which for ordinary typing is the next line: one line
//! of work per keystroke, whatever the file's size. An edit that changes
//! everything after it (opening a block comment) colours what is near it at
//! once and the rest off the UI thread, on a snapshot; an edit made while
//! that runs makes it start over. Lines longer than [`LONG_LINE`] (minified
//! code) are not coloured, as in VS Code. Folds come from braces, worked out
//! off the UI thread a moment after typing stops.
//!
//! Scope names and style runs follow gpui-component's syntect backend
//! (Apache-2.0), so the colours are the same as before.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use gpui_kit::component::input::{
    EditorState, FoldRange, HighlightStyleResolver, InputEdit, InputHighlighter,
    InputHighlighterFactory, Rope, RopeExt as _,
};
use gpui_kit::{Context, HighlightStyle, SharedString, Task, Window};
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};

/// The syntaxes, read once (tens of milliseconds) and shared.
static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(two_face::syntax::extra_newlines);

/// Lines longer than this are left uncoloured: a regex parser can spend
/// seconds on one minified line.
const LONG_LINE: usize = 20_000;
/// At most this much reparsing happens on the UI thread for one edit; the
/// rest continues in the background.
const SYNC_BUDGET: Duration = Duration::from_millis(4);
/// Lines coloured at once when a file opens, before the rest in the
/// background (a screenful, with room to scroll).
const FIRST_LINES: usize = 400;
/// Distinct line-start states a parser keeps at hand to share.
const RECENT_STATES: usize = 8;
/// Folds are worked out this long after typing stops.
const FOLD_DELAY: Duration = Duration::from_millis(300);

/// The highlight names the theme knows; a span stores its index.
const NAMES: &[&str] = &[
    "comment.doc",
    "comment",
    "string.escape",
    "string.regex",
    "string",
    "number",
    "boolean",
    "constant",
    "operator",
    "preproc",
    "keyword",
    "type",
    "function",
    "tag",
    "attribute",
    "label",
    "title",
    "variable.special",
    "property",
    "variable",
    "emphasis.strong",
    "emphasis",
    "link_uri",
    "text.literal",
    "punctuation.list_marker",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation",
];

/// The parser's state at the start of a line. Shared between lines that
/// start alike, which is most of them, so a long file keeps few copies.
type State = Arc<(ParseState, ScopeStack)>;

/// A coloured span of a line: bytes from the line's start, and its name.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Span {
    start: u32,
    end: u32,
    name: u8,
}

#[derive(Clone)]
struct Line {
    start: State,
    spans: Vec<Span>,
}

/// The highlighter installed in every code editor of the Files tool.
pub fn factory() -> InputHighlighterFactory {
    Rc::new(|language| {
        find_syntax(language).map(|syntax| Box::new(Highlighter::new(language, syntax)) as _)
    })
}

/// The syntax for an editor language (an extension or a file name).
fn find_syntax(language: &str) -> Option<&'static SyntaxReference> {
    let syntaxes = &*SYNTAXES;
    let lower = language.to_ascii_lowercase();
    let alias = match lower.as_str() {
        "text" | "txt" => return None,
        "csharp" => "cs",
        "make" | "makefile" => "Makefile",
        "kts" => "kt",
        "mjs" | "cjs" | "jsx" => "js",
        "mts" | "cts" => "ts",
        "h" | "hpp" | "hh" | "hxx" => "cpp",
        "dockerfile" => "Dockerfile",
        other => other,
    };
    [language, lower.as_str(), alias]
        .into_iter()
        .find_map(|token| syntaxes.find_syntax_by_token(token))
        .filter(|syntax| syntax.name != "Plain Text")
}

struct Highlighter {
    language: SharedString,
    /// Shared with the background work that finishes parsing and folding.
    inner: Rc<RefCell<Inner>>,
    _parse: Option<Task<()>>,
    _folds: Option<Task<()>>,
}

struct Inner {
    syntax: &'static SyntaxReference,
    /// The text as last seen, to find lines and their offsets.
    text: Rope,
    lines: Vec<Line>,
    /// Lines still to parse: from the first, and through the second at
    /// least. Past that, parsing stops at the first line that starts as it
    /// did before the edit (the lines up to it start out as placeholders).
    dirty: Option<(usize, usize)>,
    /// Bumped by every edit and shared with background work, which stops
    /// when it no longer matches.
    version: Arc<AtomicU64>,
    folds: Vec<FoldRange>,
    names: HashMap<Scope, Option<u8>>,
}

impl Highlighter {
    fn new(language: &str, syntax: &'static SyntaxReference) -> Self {
        Self {
            language: language.to_owned().into(),
            inner: Rc::new(RefCell::new(Inner::new(syntax))),
            _parse: None,
            _folds: None,
        }
    }
}

impl Inner {
    fn new(syntax: &'static SyntaxReference) -> Self {
        Self {
            syntax,
            text: Rope::new(),
            lines: Vec::new(),
            dirty: None,
            version: Arc::new(AtomicU64::new(0)),
            folds: Vec::new(),
            names: HashMap::new(),
        }
    }

    fn version(&self) -> u64 {
        self.version.load(Ordering::Relaxed)
    }

    /// Takes in new text: `edit` says what changed, or `None` when all of
    /// it is new (opened, replaced, or reloaded).
    fn set_text(&mut self, edit: Option<InputEdit>, text: &Rope) {
        self.version.fetch_add(1, Ordering::Relaxed);
        self.text = text.clone();
        match edit {
            Some(edit) if !self.lines.is_empty() => self.splice(&edit),
            _ => self.reset(),
        }
        // The lines must match the text, or a lost edit would misplace every
        // colour after it: start over rather than draw wrong.
        if self.lines.len() != self.text.lines_len().max(1) {
            self.reset();
        }
    }

    fn reset(&mut self) {
        let count = self.text.lines_len().max(1);
        let state = Arc::new((ParseState::new(self.syntax), ScopeStack::new()));
        self.lines = vec![
            Line {
                start: state,
                spans: Vec::new(),
            };
            count
        ];
        self.dirty = Some((0, count - 1));
        self.folds.clear();
    }

    /// Keeps the lines in step with an edit: the edited lines are replaced
    /// by as many lines as the edit leaves, all to be parsed.
    fn splice(&mut self, edit: &InputEdit) {
        let first = edit.start_position.row;
        if first >= self.lines.len() {
            self.reset();
            return;
        }
        let old_last = edit.old_end_position.row.clamp(first, self.lines.len() - 1);
        let new_last = edit.new_end_position.row.max(first);
        let fresh = Line {
            start: self.lines[first].start.clone(),
            spans: Vec::new(),
        };
        self.lines.splice(
            first..=old_last,
            std::iter::repeat_n(fresh, new_last - first + 1),
        );
        let delta = new_last as isize - old_last as isize;
        let moved = |line: usize| {
            if line > old_last {
                line.saturating_add_signed(delta)
            } else {
                line.min(new_last)
            }
        };
        // Folds below the edit move with their lines.
        if delta != 0 {
            for fold in &mut self.folds {
                fold.start_line = moved(fold.start_line);
                fold.end_line = moved(fold.end_line).max(fold.start_line);
            }
        }
        self.dirty = Some(match self.dirty {
            Some((from, through)) => (from.min(first), moved(through).max(new_last)),
            None => (first, new_last),
        });
    }

    /// Parses dirty lines on this thread for up to `budget` and `max_lines`,
    /// and returns whether every line is coloured.
    fn parse_here(&mut self, budget: Option<Duration>, max_lines: Option<usize>) -> bool {
        let Some((from, through)) = self.dirty else {
            return true;
        };
        let started = Instant::now();
        let mut parser = LineParser::new(self.lines[from].start.clone());
        let mut index = from;
        while index < self.lines.len() {
            let spans = parser.parse(&self.text, index, &mut self.names);
            self.lines[index].spans = spans;
            index += 1;
            if index == self.lines.len()
                || (index > through && *self.lines[index].start == *parser.state)
            {
                self.dirty = None;
                return true;
            }
            self.lines[index].start = parser.state.clone();
            let over_budget = budget.is_some_and(|budget| started.elapsed() > budget);
            let over_lines = max_lines.is_some_and(|max| index - from >= max);
            if over_budget || over_lines {
                self.dirty = Some((index, through.max(index)));
                return false;
            }
        }
        self.dirty = None;
        true
    }

    /// Lines parsed in the background for `version`, from line `from`.
    fn take_parsed(&mut self, version: u64, from: usize, lines: Vec<Line>) {
        if version != self.version() || from + lines.len() > self.lines.len() {
            return;
        }
        for (offset, line) in lines.into_iter().enumerate() {
            self.lines[from + offset] = line;
        }
        self.dirty = None;
    }
}

/// Parses consecutive lines, sharing the state of lines that start alike.
struct LineParser {
    state: State,
    /// Recently seen states: lines take turns between a few (in a block, in
    /// a string, at the top level), so most lines share one of them.
    recent: Vec<State>,
    buffer: String,
}

impl LineParser {
    fn new(state: State) -> Self {
        Self {
            recent: vec![state.clone()],
            state,
            buffer: String::new(),
        }
    }

    /// Parses line `index` from the current state, which moves on to the
    /// next line's.
    fn parse(
        &mut self,
        text: &Rope,
        index: usize,
        names: &mut HashMap<Scope, Option<u8>>,
    ) -> Vec<Span> {
        read_line(text, index, &mut self.buffer);
        let (spans, next) = parse_line(&self.buffer, &self.state, &SYNTAXES, names);
        if next != *self.state {
            self.state = match self.recent.iter().position(|state| **state == next) {
                Some(found) => self.recent[found].clone(),
                None => {
                    let state = Arc::new(next);
                    if self.recent.len() == RECENT_STATES {
                        self.recent.remove(0);
                    }
                    self.recent.push(state.clone());
                    state
                }
            };
        }
        spans
    }
}

impl Highlighter {
    /// Colours the dirty lines off the UI thread, on a snapshot of the text,
    /// and takes the result if no edit came in meanwhile.
    fn parse_later(&mut self, window: &mut Window, cx: &mut Context<EditorState>) {
        let inner = self.inner.borrow();
        let Some((from, through)) = inner.dirty else {
            drop(inner);
            self._parse = None;
            return;
        };
        let text = inner.text.clone();
        let start = inner.lines[from].start.clone();
        // The starts of the lines after the ones that must be parsed, to
        // stop where the text starts as it did.
        let old: Vec<State> = inner.lines[through + 1..]
            .iter()
            .map(|line| line.start.clone())
            .collect();
        let count = inner.lines.len();
        let version = inner.version();
        let latest = inner.version.clone();
        drop(inner);
        let shared = self.inner.clone();
        let task = cx.background_executor().spawn(async move {
            let mut names = HashMap::new();
            let mut parser = LineParser::new(start);
            let mut lines = Vec::new();
            for index in from..count {
                // A newer edit makes this work useless.
                if index % 256 == 0 && latest.load(Ordering::Relaxed) != version {
                    return None;
                }
                if index > through && *old[index - through - 1] == *parser.state {
                    break;
                }
                let start = parser.state.clone();
                let spans = parser.parse(&text, index, &mut names);
                lines.push(Line { start, spans });
            }
            Some(lines)
        });
        self._parse = Some(cx.spawn_in(window, async move |editor, cx| {
            let Some(lines) = task.await else { return };
            shared.borrow_mut().take_parsed(version, from, lines);
            _ = editor.update(cx, |_, cx| cx.notify());
        }));
    }

    /// Works out the folds a moment after typing stops, off the UI thread.
    fn fold_later(&mut self, window: &mut Window, cx: &mut Context<EditorState>) {
        let (text, version, latest) = {
            let inner = self.inner.borrow();
            (inner.text.clone(), inner.version(), inner.version.clone())
        };
        let shared = self.inner.clone();
        self._folds = Some(cx.spawn_in(window, async move |editor, cx| {
            cx.background_executor().timer(FOLD_DELAY).await;
            let folds = cx
                .background_executor()
                .spawn(async move {
                    brace_fold_ranges(&text, &|| latest.load(Ordering::Relaxed) != version)
                })
                .await;
            let Some(folds) = folds else { return };
            {
                let mut inner = shared.borrow_mut();
                if inner.version() != version {
                    return;
                }
                inner.folds = folds.clone();
            }
            _ = editor.update(cx, |editor, cx| {
                editor.apply_highlighter_fold_candidates(folds, cx)
            });
        }));
    }
}

impl InputHighlighter for Highlighter {
    fn language(&self) -> SharedString {
        self.language.clone()
    }

    fn update(
        &mut self,
        edit: Option<InputEdit>,
        text: &Rope,
        folding: bool,
        window: &mut Window,
        cx: &mut Context<EditorState>,
    ) {
        let done = {
            let mut inner = self.inner.borrow_mut();
            inner.set_text(edit, text);
            if !folding {
                inner.folds.clear();
            }
            match edit {
                Some(_) => inner.parse_here(Some(SYNC_BUDGET), None),
                None => inner.parse_here(None, Some(FIRST_LINES)),
            }
        };
        if done {
            self._parse = None;
        } else {
            self.parse_later(window, cx);
        }
        if folding {
            self.fold_later(window, cx);
        } else {
            self._folds = None;
        }
    }

    fn styles(
        &self,
        range: &Range<usize>,
        resolver: &dyn HighlightStyleResolver,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let inner = self.inner.borrow();
        let mut runs = Vec::new();
        let mut cursor = range.start;
        let text_len = inner.text.len();
        if range.start >= text_len || inner.lines.is_empty() {
            if range.start < range.end {
                runs.push((range.clone(), HighlightStyle::default()));
            }
            return runs;
        }
        let first = inner.text.offset_to_point(range.start).row;
        let last = inner
            .text
            .offset_to_point(range.end.min(text_len).saturating_sub(1).max(range.start))
            .row;
        let mut styles: [Option<HighlightStyle>; NAMES.len()] = [None; NAMES.len()];
        for index in first..=last.min(inner.lines.len() - 1) {
            let line_start = inner.text.line_start_offset(index);
            for span in &inner.lines[index].spans {
                let start = (line_start + span.start as usize).max(range.start);
                let end = (line_start + span.end as usize).min(range.end);
                if start >= end || end <= cursor {
                    continue;
                }
                if cursor < start {
                    runs.push((cursor..start, HighlightStyle::default()));
                }
                let style = *styles[span.name as usize].get_or_insert_with(|| {
                    resolver
                        .style(NAMES[span.name as usize])
                        .unwrap_or_default()
                });
                runs.push((start.max(cursor)..end, style));
                cursor = end;
            }
        }
        if cursor < range.end {
            runs.push((cursor..range.end, HighlightStyle::default()));
        }
        runs
    }

    fn fold_ranges(&self, _: &Rope) -> Vec<FoldRange> {
        self.inner.borrow().folds.clone()
    }

    /// The editor asks for the folds of the edited lines on every keystroke;
    /// the whole set follows once typing stops.
    fn fold_ranges_for_edit(&self, range: Range<usize>, text: &Rope) -> Vec<FoldRange> {
        let first = text.offset_to_point(range.start).row;
        let last = text.offset_to_point(range.end.min(text.len())).row;
        self.inner
            .borrow()
            .folds
            .iter()
            .filter(|fold| (first..=last).contains(&fold.start_line))
            .copied()
            .collect()
    }
}

/// Line `index` of `text` into `buffer`, ending with a newline (syntect's
/// newline syntaxes expect one on every line).
fn read_line(text: &Rope, index: usize, buffer: &mut String) {
    buffer.clear();
    if index >= text.lines_len() {
        return;
    }
    for chunk in text.slice_line(index).chunks() {
        buffer.push_str(chunk);
    }
    buffer.push('\n');
}

/// One line's spans, and the state the next line starts in.
fn parse_line(
    line: &str,
    start: &State,
    syntaxes: &SyntaxSet,
    names: &mut HashMap<Scope, Option<u8>>,
) -> (Vec<Span>, (ParseState, ScopeStack)) {
    let (mut parser, mut scopes) = (start.0.clone(), start.1.clone());
    let mut spans: Vec<Span> = Vec::new();
    if line.len() > LONG_LINE {
        return (spans, (parser, scopes));
    }
    let Ok(operations) = parser.parse_line(line, syntaxes) else {
        // A line syntect cannot parse keeps the state it started with.
        return (spans, (start.0.clone(), start.1.clone()));
    };
    // The visible text, without the line ending.
    let visible = line.trim_end_matches(['\n', '\r']).len();
    let mut push = |range: Range<usize>, scopes: &ScopeStack| {
        let range = range.start.min(visible)..range.end.min(visible);
        if range.is_empty() {
            return;
        }
        // The innermost scope that names something wins, so a string's
        // quotes colour as the string.
        let name = scopes
            .scopes
            .iter()
            .rev()
            .find_map(|scope| *names.entry(*scope).or_insert_with(|| semantic_name(*scope)));
        if let Some(name) = name {
            match spans.last_mut() {
                Some(last) if last.end as usize == range.start && last.name == name => {
                    last.end = range.end as u32;
                }
                _ => spans.push(Span {
                    start: range.start as u32,
                    end: range.end as u32,
                    name,
                }),
            }
        }
    };
    let mut cursor = 0;
    for (index, operation) in operations {
        push(cursor..index, &scopes);
        let _ = scopes.apply(&operation);
        cursor = index;
    }
    push(cursor..line.len(), &scopes);
    (spans, (parser, scopes))
}

/// The theme's highlight name (as an index into [`NAMES`]) for a TextMate
/// scope, or `None` to let an enclosing scope decide.
fn semantic_name(scope: Scope) -> Option<u8> {
    let scope = scope.build_string();
    let is = |prefix: &str| {
        scope == prefix
            || scope
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('.'))
    };
    let name = if is("punctuation.definition.string")
        || is("punctuation.definition.comment")
        || is("invalid")
        || is("meta")
        || is("source")
        || is("text")
    {
        // Delimiters belong to what they delimit; the rest are containers.
        return None;
    } else if is("comment.block.documentation") || is("comment.line.documentation") {
        "comment.doc"
    } else if is("comment") {
        "comment"
    } else if is("constant.character.escape") {
        "string.escape"
    } else if is("string.regexp") {
        "string.regex"
    } else if is("string") {
        "string"
    } else if is("constant.numeric") {
        "number"
    } else if is("constant.language.boolean") {
        "boolean"
    } else if is("constant") || is("support.constant") || is("entity.name.constant") {
        "constant"
    } else if is("keyword.operator") {
        "operator"
    } else if is("keyword.control.import") || is("meta.preprocessor") {
        "preproc"
    } else if is("keyword") || is("storage.modifier") {
        "keyword"
    } else if is("storage.type") {
        // `fn`, `class` and `let` are declarations; `int` and `bool` types.
        if [
            "class",
            "enum",
            "function",
            "impl",
            "interface",
            "module",
            "namespace",
            "struct",
            "trait",
            "type",
            "let",
            "var",
            "const",
        ]
        .iter()
        .any(|word| scope.contains(word))
        {
            "keyword"
        } else {
            "type"
        }
    } else if is("entity.name.function")
        || is("support.function")
        || is("variable.function")
        || is("meta.function-call")
    {
        "function"
    } else if is("entity.name.type")
        || is("entity.name.class")
        || is("entity.name.struct")
        || is("entity.name.enum")
        || is("entity.name.trait")
        || is("entity.name.interface")
        || is("entity.other.inherited-class")
        || is("support.type")
        || is("support.class")
    {
        "type"
    } else if is("entity.name.tag") {
        "tag"
    } else if is("entity.other.attribute-name") {
        "attribute"
    } else if is("entity.name.label") {
        "label"
    } else if is("entity.name.section") || is("markup.heading") {
        "title"
    } else if is("variable.language") {
        "variable.special"
    } else if is("variable.other.member")
        || is("variable.other.property")
        || is("support.variable.property")
        || is("meta.mapping.key")
    {
        "property"
    } else if is("variable") {
        "variable"
    } else if is("markup.bold") {
        "emphasis.strong"
    } else if is("markup.italic") {
        "emphasis"
    } else if is("markup.underline.link") {
        "link_uri"
    } else if is("markup.raw") {
        "text.literal"
    } else if is("markup.list") {
        "punctuation.list_marker"
    } else if is("punctuation.section") || is("punctuation.definition.block") {
        "punctuation.bracket"
    } else if is("punctuation.separator") || is("punctuation.terminator") {
        "punctuation.delimiter"
    } else if is("punctuation") {
        "punctuation"
    } else {
        return None;
    };
    NAMES
        .iter()
        .position(|known| *known == name)
        .map(|index| index as u8)
}

/// Fold candidates from matching braces outside strings and line comments
/// (the structure most languages share; indentation-only languages get
/// none).
/// `None` when `cancelled` says the text changed meanwhile.
fn brace_fold_ranges(text: &Rope, cancelled: &dyn Fn() -> bool) -> Option<Vec<FoldRange>> {
    let mut starts = Vec::new();
    let mut ranges = Vec::new();
    let mut line = String::new();
    for line_number in 0..text.lines_len() {
        if line_number % 1024 == 0 && cancelled() {
            return None;
        }
        read_line(text, line_number, &mut line);
        if line.len() > LONG_LINE {
            continue;
        }
        let mut chars = line.chars().peekable();
        let mut quote = None;
        let mut escaped = false;
        while let Some(character) = chars.next() {
            match quote {
                Some(open) => {
                    if escaped {
                        escaped = false;
                    } else if character == '\\' {
                        escaped = true;
                    } else if character == open {
                        quote = None;
                    }
                }
                None => match character {
                    '/' if chars.peek() == Some(&'/') => break,
                    '#' => break,
                    '"' | '\'' | '`' => quote = Some(character),
                    '{' => starts.push(line_number),
                    '}' => {
                        if let Some(start) = starts.pop()
                            && line_number.saturating_sub(start) >= 2
                        {
                            ranges.push(FoldRange::new(start, line_number));
                        }
                    }
                    _ => {}
                },
            }
        }
    }
    ranges.sort_by_key(|range| range.start_line);
    ranges.dedup_by_key(|range| range.start_line);
    Some(ranges)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::component::input::Point;

    /// Replaces `range` of `text` with `new`, and describes the edit as the
    /// editor does.
    fn edit(text: &mut Rope, range: Range<usize>, new: &str) -> InputEdit {
        let start_position = text.offset_to_point(range.start);
        let old_end_position = text.offset_to_point(range.end);
        text.replace(range.clone(), new);
        let new_end = range.start + new.len();
        InputEdit {
            start_byte: range.start,
            old_end_byte: range.end,
            new_end_byte: new_end,
            start_position,
            old_end_position,
            new_end_position: text.offset_to_point(new_end),
        }
    }

    fn inner(language: &str) -> Inner {
        Inner::new(find_syntax(language).expect("a syntax"))
    }

    /// Every line's spans, parsing the whole text from scratch.
    fn from_scratch(language: &str, text: &Rope) -> Vec<Vec<Span>> {
        let mut fresh = inner(language);
        fresh.set_text(None, text);
        assert!(fresh.parse_here(None, None));
        fresh.lines.into_iter().map(|line| line.spans).collect()
    }

    fn spans(inner: &Inner) -> Vec<Vec<Span>> {
        inner.lines.iter().map(|line| line.spans.clone()).collect()
    }

    fn names_on(inner: &Inner, line: usize) -> Vec<&'static str> {
        inner.lines[line]
            .spans
            .iter()
            .map(|span| NAMES[span.name as usize])
            .collect()
    }

    const RUST: &str = "fn main() {\n    let x = 1;\n    // note\n    println!(\"{x}\");\n}\n";

    #[test]
    fn edits_colour_the_same_as_parsing_from_scratch() {
        let mut text = Rope::from(RUST);
        let mut state = inner("rs");
        state.set_text(None, &text);
        assert!(state.parse_here(None, None));
        fn at(text: &Rope, needle: &str) -> usize {
            text.to_string().find(needle).unwrap()
        }
        type MakeEdit = Box<dyn Fn(&Rope) -> (Range<usize>, String)>;
        let edits: Vec<MakeEdit> = vec![
            // Typing a character.
            Box::new(|t| (at(t, "1;")..at(t, "1;"), "2".into())),
            // A new line.
            Box::new(|t| {
                (
                    at(t, "    // note")..at(t, "    // note"),
                    "    let y = \"s\";\n".into(),
                )
            }),
            // Opening a block comment changes every line after it.
            Box::new(|t| (at(t, "    let x")..at(t, "    let x"), "/*".into())),
            // Closing it again.
            Box::new(|t| (at(t, "    println")..at(t, "    println"), "*/".into())),
            // Pasting several lines.
            Box::new(|t| {
                (
                    at(t, "}\n")..at(t, "}\n"),
                    "    if x {\n        return;\n    }\n".into(),
                )
            }),
            // Deleting across lines.
            Box::new(|t| (at(t, "let y")..at(t, "println"), String::new())),
        ];
        for make in edits {
            let (range, new) = make(&text);
            let change = edit(&mut text, range, &new);
            state.set_text(Some(change), &text);
            assert!(state.parse_here(None, None));
            assert_eq!(spans(&state), from_scratch("rs", &text), "after {new:?}");
        }
    }

    #[test]
    fn a_pasted_block_is_coloured_on_every_line() {
        let mut text = Rope::from("fn a() {}\n");
        let mut state = inner("rs");
        state.set_text(None, &text);
        state.parse_here(None, None);
        let change = edit(&mut text, 10..10, "fn b() {}\nfn c() {}\nfn d() {}\n");
        state.set_text(Some(change), &text);
        assert!(state.parse_here(None, None));
        for line in 1..=3 {
            assert!(names_on(&state, line).contains(&"keyword"), "line {line}");
        }
    }

    #[test]
    fn a_block_comment_reaches_the_lines_after_it() {
        let mut text = Rope::from("int a;\nint b;\nint c;\n");
        let mut state = inner("c");
        state.set_text(None, &text);
        state.parse_here(None, None);
        let change = edit(&mut text, 0..0, "/* ");
        state.set_text(Some(change), &text);
        state.parse_here(None, None);
        assert_eq!(names_on(&state, 2), vec!["comment"]);
    }

    #[test]
    fn work_stops_at_the_budget_and_resumes() {
        let source = "let a = 1;\n".repeat(2_000);
        let text = Rope::from(source.as_str());
        let mut state = inner("js");
        state.set_text(None, &text);
        assert!(!state.parse_here(None, Some(100)));
        assert_eq!(state.dirty, Some((100, 2_000)));
        assert!(state.parse_here(None, None));
        assert_eq!(spans(&state), from_scratch("js", &text));
    }

    #[test]
    fn background_results_for_old_text_are_dropped() {
        let mut text = Rope::from(RUST);
        let mut state = inner("rs");
        state.set_text(None, &text);
        let stale = state.version();
        let change = edit(&mut text, 0..0, "\n");
        state.set_text(Some(change), &text);
        state.take_parsed(stale, 0, Vec::new());
        assert!(state.dirty.is_some());
    }

    #[test]
    fn long_lines_are_left_plain() {
        let source = format!("let a = \"{}\";\n", "x".repeat(LONG_LINE));
        let text = Rope::from(source.as_str());
        let mut state = inner("js");
        state.set_text(None, &text);
        state.parse_here(None, None);
        assert!(state.lines[0].spans.is_empty());
    }

    #[test]
    fn styles_cover_the_range_without_gaps() {
        struct Plain;
        impl HighlightStyleResolver for Plain {
            fn style(&self, _: &str) -> Option<HighlightStyle> {
                Some(HighlightStyle::default())
            }
        }
        let text = Rope::from(RUST);
        let highlighter = Highlighter::new("rs", find_syntax("rs").unwrap());
        {
            let mut inner = highlighter.inner.borrow_mut();
            inner.set_text(None, &text);
            inner.parse_here(None, None);
        }
        for range in [0..RUST.len(), 5..40, 12..13, RUST.len() - 2..RUST.len() + 5] {
            let runs = highlighter.styles(&range, &Plain);
            assert_eq!(runs.first().unwrap().0.start, range.start);
            assert_eq!(runs.last().unwrap().0.end, range.end);
            for pair in runs.windows(2) {
                assert_eq!(pair[0].0.end, pair[1].0.start, "{range:?}");
            }
        }
    }

    #[test]
    fn every_supported_language_has_a_syntax() {
        let mut missing = Vec::new();
        for language in [
            "c", "h", "cpp", "hpp", "cc", "cs", "py", "kt", "kts", "java", "js", "jsx", "mjs",
            "ts", "tsx", "html", "css", "rs", "go", "json", "md", "toml", "yaml", "sh",
        ] {
            missing.extend(find_syntax(language).is_none().then_some(language));
        }
        assert!(missing.is_empty(), "{missing:?}");
        assert!(find_syntax("txt").is_none());
    }

    #[test]
    fn folds_follow_braces_and_skip_strings() {
        let text = Rope::from("fn a() {\n    let s = \"{\";\n    b();\n}\n");
        let folds = brace_fold_ranges(&text, &|| false).unwrap();
        assert_eq!(folds, vec![FoldRange::new(0, 3)]);
        assert!(brace_fold_ranges(&text, &|| true).is_none());
        let _ = Point::new(0, 0);
    }
}

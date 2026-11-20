//! Whether Markdown nests too deeply to be parsed. The parser makes one
//! node inside another for every `>`, list marker and emphasis mark, takes
//! time that grows with the square of that depth, and the tree is then read
//! by functions that call themselves: a few kilobytes of `>` in a file, a
//! reply or a pull request froze the app and then overflowed its stack.
//! Such text is found here first, in one pass over its lines, and shown as
//! plain text ([`guarded`]).
//!
//! The pass never thinks a text shallower than the parser does. Where it
//! cannot know, it counts: so the rare text that only looks deep is shown
//! plain too.

use std::borrow::Cow;

/// The deepest quotes, lists and emphasis may nest. Documents written by
/// people stay below it (of four thousand read while this was written,
/// none passed it). The text view reads the tree with functions that call
/// themselves, at about 4 KB of stack for a level of lists and 5 KB for
/// one of emphasis: both this deep together take some 300 KB, of the
/// 512 KB a background thread has on macOS.
pub const DEEPEST: usize = 32;
/// The most `[` left open in a paragraph. Brackets do not nest in the tree,
/// they only cost the parser time, and plain text has many that never
/// close (terminal colour codes), so this limit is a wide one.
const OPEN_BRACKETS: usize = 1024;
/// How much of a text that is too deep is shown.
const SHOWN: usize = 32 * 1024;

/// What is said over a text shown plain.
const NOTE: &str = "*This text nests too deeply to be shown formatted. Here it is as plain text.*";
/// The same for a text the parser does not get through ([`parses`]).
const UNREAD: &str = "*This text could not be read as Markdown. Here it is as plain text.*";

/// `text` as it is when it can be parsed; otherwise a note and its start
/// as a code block, where nothing is read as Markdown.
pub fn guarded(text: &str) -> Cow<'_, str> {
    if shown_plain(text) {
        return Cow::Borrowed(text);
    }
    let note = if nests_too_deep(text) {
        NOTE
    } else if !parses(text) {
        UNREAD
    } else {
        return Cow::Borrowed(text);
    };
    let mut end = text.len().min(SHOWN);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let shown = &text[..end];
    // A fence longer than any run of backticks inside: the text cannot
    // end it.
    let fence = "`".repeat(longest_run(shown).max(2) + 1);
    let more = if end < text.len() { "\n…" } else { "" };
    Cow::Owned(format!("{note}\n\n{fence}\n{shown}{more}\n{fence}\n"))
}

thread_local! {
    /// This thread is inside [`parses`]' trial.
    static ON_TRIAL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether this thread is trying a parse that may panic and is caught. The
/// app's crash handler asks, to write no crash report for it.
pub fn on_trial() -> bool {
    ON_TRIAL.with(std::cell::Cell::get)
}

/// The most answers of [`parses`] kept.
const TRIALS_KEPT: usize = 2048;

/// Whether the parser gets through `text`. `markdown` 1.0.0 (the newest)
/// panics on a few small texts, such as a link written apart with a rule
/// right under it:
///
/// ```text
/// [r]: u
/// ---
/// Title
/// ---
/// ```
///
/// The text view parses on the UI thread, so one such README, reply or pull
/// request body ended the app. Each text is tried here once, the panic
/// caught, and the answer kept: the views ask again on every draw.
pub fn parses(text: &str) -> bool {
    use std::collections::HashMap;
    use std::hash::BuildHasher as _;
    use std::sync::{Mutex, OnceLock};

    // Keyed by a hash salted for this run, so no text can be made to match
    // another's answer.
    type Tried = (std::hash::RandomState, HashMap<u64, bool>);
    static TRIED: OnceLock<Mutex<Tried>> = OnceLock::new();
    let tried = TRIED.get_or_init(Mutex::default);
    let key = {
        let tried = tried.lock().unwrap_or_else(|err| err.into_inner());
        let key = tried.0.hash_one(text);
        if let Some(known) = tried.1.get(&key) {
            return *known;
        }
        key
    };
    // As the text view parses (GitHub's Markdown and formulas), and with a
    // header block where the text could start one.
    let trial = |header: bool| {
        let mut options = markdown::ParseOptions::gfm();
        options.constructs.math_text = true;
        options.constructs.math_flow = true;
        options.constructs.frontmatter = header;
        ON_TRIAL.with(|on| on.set(true));
        let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // An error is the parser's own answer: the view shows it.
            let _ = markdown::to_mdast(text, &options);
        }));
        ON_TRIAL.with(|on| on.set(false));
        done.is_ok()
    };
    let parses = trial(false) && (!text.starts_with("---") || trial(true));
    let mut tried = tried.lock().unwrap_or_else(|err| err.into_inner());
    if tried.1.len() >= TRIALS_KEPT {
        tried.1.clear();
    }
    tried.1.insert(key, parses);
    parses
}

/// The longest run of backticks in `text`.
fn longest_run(text: &str) -> usize {
    text.split(|c| c != '`').map(str::len).max().unwrap_or(0)
}

/// Whether `text` is what [`guarded`] makes: the note and one code block,
/// which nothing inside can end. So a text guarded twice (a document cut
/// into pieces, then each piece drawn) is not wrapped twice; and a text
/// only made to look like it is still nothing but a code block.
fn shown_plain(text: &str) -> bool {
    let Some(rest) = text
        .strip_prefix(NOTE)
        .or_else(|| text.strip_prefix(UNREAD))
    else {
        return false;
    };
    // (Lines kept apart for the reader end in two blanks.)
    let Some(rest) = rest.trim_start_matches(' ').strip_prefix("\n\n") else {
        return false;
    };
    let Some((fence, body)) = rest.split_once('\n') else {
        return false;
    };
    fence.len() >= 3
        && fence.bytes().all(|byte| byte == b'`')
        && body
            .strip_suffix('\n')
            .and_then(|body| body.strip_suffix(fence))
            .is_some_and(|body| body.ends_with('\n') && longest_run(body) < fence.len())
}

/// Whether `added`, put at the end of a `text` that was not too deep,
/// makes it so. Only the paragraph being added to is read again: a reply
/// that arrives in thousands of pieces is not read whole for each.
pub fn deepens(text: &str, added: &str) -> bool {
    // After the last blank line, where nothing is open (looked for from
    // the end, and no further back than it must be).
    let from = text.rfind("\n\n").map_or(0, |at| at + 2);
    let from = text[from..]
        .rfind("\n\r\n")
        .map_or(from, |at| from + at + 3);
    let mut tail = String::with_capacity(text.len() - from + added.len());
    tail.push_str(&text[from..]);
    tail.push_str(added);
    // (Or stops the parser: see `parses`.)
    nests_too_deep(&tail) || !parses(&tail)
}

/// Whether quotes, lists, emphasis or brackets in `text` may nest deeper
/// than the limits above.
pub fn nests_too_deep(text: &str) -> bool {
    deeper_than(text, DEEPEST)
}

/// [`nests_too_deep`], for any limit (a test holds it against the parser
/// with a low one, which short texts reach).
fn deeper_than(text: &str, limit: usize) -> bool {
    // The parser skips a byte order mark; the first line's marks follow it.
    let mut rest = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut inline = Inline::new(limit);
    while !rest.is_empty() {
        // Lines end as the parser has them: `\n`, `\r\n` or `\r` alone.
        let end = rest.find(['\n', '\r']).unwrap_or(rest.len());
        let (line, after) = rest.split_at(end);
        rest = after
            .strip_prefix("\r\n")
            .or_else(|| after.strip_prefix(['\n', '\r']))
            .unwrap_or(after);
        if containers(line, limit) > limit {
            return true;
        }
        // A paragraph ends at a blank line: no emphasis or bracket stays
        // open past it. Only spaces and tabs make a line blank to the
        // parser.
        if line.bytes().all(|byte| matches!(byte, b' ' | b'\t')) {
            inline = Inline::new(limit);
        } else if inline.read(line) {
            return true;
        }
    }
    false
}

/// How many quotes, list items and footnotes `line` may be inside, from
/// the marks it starts with: each `>`, list marker and `[^note]:`, and one
/// more for every two columns of indentation before a mark (an item's text
/// starts at least two columns after its marker, so a line indented that
/// far can be inside an item of a line above). Counted up to just past
/// `limit`.
fn containers(line: &str, limit: usize) -> usize {
    let bytes = line.as_bytes();
    let space = |at: usize| matches!(bytes.get(at), None | Some(b' ' | b'\t'));
    let (mut ix, mut depth) = (0, 0);
    // The columns of blanks since the last mark: at most (a tab reaches
    // the next multiple of four), and at least.
    let (mut indent, mut least) = (0, 0);
    // The last mark was a list marker.
    let mut item = false;
    while depth <= limit {
        let marker = match bytes.get(ix) {
            Some(b' ') => {
                (indent, least, ix) = (indent + 1, least + 1, ix + 1);
                continue;
            }
            Some(b'\t') => {
                (indent, least, ix) = (indent + 4, least + 1, ix + 1);
                continue;
            }
            // Five columns after a list marker, the rest of the line is
            // code: nothing in it nests (a box drawn with `*`).
            Some(_) if item && least > 4 => break,
            Some(b'>') => 1,
            Some(b'-' | b'*' | b'+') if space(ix + 1) => 1,
            Some(b'0'..=b'9') => {
                let digits = bytes[ix..]
                    .iter()
                    .take_while(|byte| byte.is_ascii_digit())
                    .count();
                match bytes.get(ix + digits) {
                    Some(b'.' | b')') if digits <= 9 && space(ix + digits + 1) => digits + 1,
                    _ => break,
                }
            }
            Some(b'[') if bytes.get(ix + 1) == Some(&b'^') => match line[ix..].find("]:") {
                Some(end) => end + 2,
                None => break,
            },
            _ => break,
        };
        item = !matches!(bytes[ix], b'>' | b'[');
        depth += 1 + indent / 2;
        (indent, least) = (0, 0);
        ix += marker;
    }
    depth
}

/// Emphasis and brackets that may still be open in the paragraph being
/// read.
struct Inline {
    /// The deepest emphasis may nest.
    limit: usize,
    /// `[` not closed yet.
    brackets: usize,
    /// Emphasis marks that may still be waiting for their closing ones:
    /// never fewer than the parser has.
    open: usize,
    /// `open` has passed the limit in this paragraph.
    over: bool,
    /// Marks that may close emphasis, seen since then. Emphasis is only
    /// that deep once as many marks close it.
    closing: usize,
}

impl Inline {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            brackets: 0,
            open: 0,
            over: false,
            closing: 0,
        }
    }

    /// Reads one line of a paragraph; whether it is too deep now.
    fn read(&mut self, line: &str) -> bool {
        let bytes = line.as_bytes();
        let run = |from: usize| {
            bytes[from..]
                .iter()
                .take_while(|byte| **byte == bytes[from])
                .count()
        };
        // Marks on this line that surely open emphasis and nothing else,
        // innermost last: the mark, how many, and the brackets open then.
        // `**word**` is taken off the count again when its closing marks
        // come, so that a long list of bold words is not counted as deep.
        // That is only right when the parser pairs the same marks: so only
        // marks that can do nothing but close are paired (were the opening
        // ones not the parser's, they do nothing at all), and whatever may
        // hide closing marks from the parser empties this list, or makes
        // them `literal` below. The marks left in it stay counted.
        let mut pending: Vec<(u8, usize, usize)> = Vec::new();
        // Up to here the line is code, if anything is. Marks in code or in
        // an address are counted, never paired.
        let mut code_until = 0;
        // A run of backticks had no like on this line. The line is not
        // searched again for each further run (a line of runs of every
        // length would be read once for each): they all empty the list.
        let mut code_open = false;
        let addresses = addresses(bytes);
        let mut address = 0;
        let mut ix = 0;
        while ix < bytes.len() {
            match bytes[ix] {
                b'[' => {
                    self.brackets += 1;
                    if self.brackets > OPEN_BRACKETS {
                        return true;
                    }
                    ix += 1;
                }
                b']' => {
                    self.brackets = self.brackets.saturating_sub(1);
                    // Marks opened inside the brackets close inside them.
                    while pending.last().is_some_and(|open| open.2 > self.brackets) {
                        pending.pop();
                    }
                    ix += 1;
                    // A link's address with a title, or cut off: where it
                    // ends is not known here.
                    while addresses.get(address).is_some_and(|part| part.end <= ix) {
                        address += 1;
                    }
                    let known = addresses
                        .get(address)
                        .is_some_and(|part| part.start == ix + 1);
                    if bytes.get(ix) == Some(&b'(') && !known {
                        pending.clear();
                    }
                }
                // An escaped mark is no mark, and `<…>` may be a tag or an
                // address.
                b'\\' | b'<' => {
                    pending.clear();
                    ix += 1;
                }
                b'`' => {
                    let length = run(ix);
                    if ix >= code_until {
                        // (A backtick in an address written out is part
                        // of the address, and which run then opens code
                        // is not known.)
                        while addresses.get(address).is_some_and(|part| part.end <= ix) {
                            address += 1;
                        }
                        let end = if code_open
                            || addresses.get(address).is_some_and(|part| part.start <= ix)
                        {
                            None
                        } else {
                            code_end(bytes, ix + length, length)
                        };
                        match end {
                            Some(end) => code_until = end,
                            // Code that ends on a later line, if at all.
                            None => {
                                code_open = true;
                                pending.clear();
                            }
                        }
                    }
                    ix += length;
                }
                mark @ (b'*' | b'_' | b'~') => {
                    let length = run(ix);
                    // After a backslash (not one that is escaped itself)
                    // the first mark is no mark: written `\_`, an
                    // underscore is only that.
                    let escaped = bytes[..ix]
                        .iter()
                        .rev()
                        .take_while(|byte| **byte == b'\\')
                        .count()
                        % 2
                        == 1;
                    while addresses.get(address).is_some_and(|part| part.end <= ix) {
                        address += 1;
                    }
                    let literal = ix < code_until
                        || addresses.get(address).is_some_and(|part| part.start <= ix);
                    // (Not in an address written out: the backslash may be
                    // the address's last character, and the mark a mark.)
                    if escaped && length == 1 && !literal {
                        ix += 1;
                        continue;
                    }
                    let around = (
                        line[..ix].chars().next_back(),
                        line[ix + length..].chars().next(),
                    );
                    if self.mark((mark, length, !escaped), around, literal, &mut pending) {
                        return true;
                    }
                    ix += length;
                }
                _ => ix += 1,
            }
        }
        false
    }

    /// Counts a run of emphasis marks (the mark, how many, and whether it
    /// is what it looks like) between the characters `around` it; whether
    /// emphasis is too deep now. The parser lets a run open emphasis when a
    /// word or (after a space or punctuation) punctuation follows, close it
    /// when one comes before, and both inside a word (not `_`) or between
    /// marks.
    fn mark(
        &mut self,
        (mark, length, plain): (u8, usize, bool),
        (before, after): (Option<char>, Option<char>),
        literal: bool,
        pending: &mut Vec<(u8, usize, usize)>,
    ) -> bool {
        let space = |c: Option<char>| c.is_none_or(char::is_whitespace);
        // Only ASCII is sorted into words and punctuation here: the parser
        // has a table of its own for the rest. A mark beside another mark
        // opens and closes by rules of its own.
        let word = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric());
        let point = |c: Option<char>| {
            c.is_some_and(|c| c.is_ascii_punctuation() && !matches!(c, '*' | '_' | '~'))
        };
        let (may_open, may_close) = (!space(after), !space(before));
        if !(may_open || may_close) || (mark == b'_' && word(before) && word(after)) {
            return false;
        }
        // Surely the one and not the other. Of a run that is not `plain`
        // (its first mark is escaped: the parser sees another run), nothing
        // is sure.
        let opens = plain && may_open && (!may_close || (point(before) && word(after)));
        let closes = plain && may_close && (!may_open || (point(after) && word(before)));
        // The closing marks of the innermost ones waiting: as many as
        // those, they use each other up whole (strikethrough only as one
        // or two marks).
        let paired = closes
            && !literal
            && (mark != b'~' || length <= 2)
            && pending.last() == Some(&(mark, length, self.brackets));
        if paired {
            pending.pop();
            self.open -= length;
        } else if may_open && !closes {
            self.open += length;
            self.over |= self.open > self.limit;
            match (literal, opens) {
                // Counted, and no more.
                (true, _) => {}
                (false, true) => pending.push((mark, length, self.brackets)),
                // It may close some of what waits, in part.
                (false, false) => pending.clear(),
            }
        }
        if may_close && !opens && self.over {
            self.closing += length;
        }
        self.closing > self.limit
    }
}

/// The parts of a line that may be an address, in order: a link's
/// (`](address)`), and whole words that hold one written out (`www.…`,
/// `https://…`, a mail address), which the parser makes a link of up to
/// the word's end. It reads no marks in an address.
fn addresses(bytes: &[u8]) -> Vec<std::ops::Range<usize>> {
    // (A mail address has a name before its `@`: `**@someone**` is none.)
    let name = |byte: &u8| byte.is_ascii_alphanumeric() || b"._+-".contains(byte);
    let written_out = |at: usize, end: usize| {
        (bytes[at] == b'@' && at > 0 && name(&bytes[at - 1]))
            || bytes[at..end].starts_with(b"://")
            || bytes[at..end]
                .get(..4)
                .is_some_and(|start| start.eq_ignore_ascii_case(b"www."))
    };
    let mut found: Vec<std::ops::Range<usize>> = Vec::new();
    // How far addresses are still searched for their ends: a few times the
    // line, however many `](` it has.
    let mut room = 4 * bytes.len() + 1024;
    let mut start = 0;
    while start < bytes.len() {
        let end = bytes[start..]
            .iter()
            .position(u8::is_ascii_whitespace)
            .map_or(bytes.len(), |at| start + at);
        let mut ix = start;
        while ix < end {
            if bytes[ix] == b']'
                && bytes.get(ix + 1) == Some(&b'(')
                && let Some(close) = address_end(bytes, ix + 2, &mut room)
            {
                found.push(ix + 2..close);
                // Were this no link after all, an address written out in
                // it would go on past the bracket, unless all that follows
                // is what such an address never ends with.
                let goes_on = (ix + 2..close).any(|at| written_out(at, close))
                    && bytes[close..end]
                        .iter()
                        .any(|byte| !b"?!.,:*_~)".contains(byte));
                if goes_on {
                    found.push(close..end);
                    break;
                }
                ix = close;
            } else if written_out(ix, end) {
                // The whole word, in place of what was found in it.
                while found.last().is_some_and(|part| part.start >= start) {
                    found.pop();
                }
                found.push(start..end);
                break;
            } else {
                ix += 1;
            }
        }
        start = end + 1;
    }
    found
}

/// Where the code span opened by a run of `length` backticks ends on this
/// line: after the next run of exactly as many, from `from` on.
fn code_end(bytes: &[u8], from: usize, length: usize) -> Option<usize> {
    let mut ix = from;
    while ix < bytes.len() {
        if bytes[ix] != b'`' {
            ix += 1;
            continue;
        }
        let run = bytes[ix..].iter().take_while(|byte| **byte == b'`').count();
        ix += run;
        if run == length {
            return Some(ix);
        }
    }
    None
}

/// Where a link's address `(…)` that starts at `from` ends, when it is an
/// address alone. With a space in it a title may follow, and where that
/// ends is not known here; nor is it once `room` bytes have been read.
fn address_end(bytes: &[u8], from: usize, room: &mut usize) -> Option<usize> {
    let mut depth = 0usize;
    for (offset, byte) in bytes[from..].iter().enumerate() {
        *room = room.checked_sub(1)?;
        match byte {
            b'(' => depth += 1,
            b')' if depth == 0 => return Some(from + offset + 1),
            b')' => depth -= 1,
            b'<' => return None,
            byte if byte.is_ascii_whitespace() => return None,
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_text_that_stops_the_parser_is_shown_plain() {
        // The smallest ones found; each ended the app in the text view.
        for stops in ["[r]: u\n---\nTitle\n---\n", "[r]:u\n-\na\n-\n"] {
            assert!(!parses(stops), "{stops:?}");
            // Asked again, the answer is the one kept.
            assert!(!parses(stops));
            let shown = guarded(stops);
            assert!(shown.starts_with(UNREAD), "{shown}");
            assert!(shown.contains(stops.trim_end()));
            // What is shown instead is read without trouble, and is not
            // wrapped a second time.
            assert!(parses(&shown));
            assert_eq!(guarded(&shown), shown);
        }
        // A reply that grows into such a text is caught as it does.
        assert!(deepens("Before.\n\n[r]: u\n---\nTitle\n", "---\n"));
        // Ordinary text is left as it is.
        let plain = "# Title\n\n[r]: u\n\n---\n\nText with a [link][r].\n";
        assert!(parses(plain));
        assert!(matches!(guarded(plain), Cow::Borrowed(_)));
        assert!(!deepens("Before.\n\n", "More *text*.\n"));
        assert!(!on_trial());
    }

    #[test]
    fn deep_quotes_and_lists_are_found() {
        assert!(nests_too_deep(&format!("{}x", ">".repeat(65))));
        assert!(nests_too_deep(&format!("{}x", "> ".repeat(65))));
        assert!(nests_too_deep(&format!("{}x", "- ".repeat(65))));
        assert!(nests_too_deep(&format!("{}x", "1. ".repeat(65))));
        assert!(nests_too_deep(&format!("\u{feff}{}x", ">".repeat(65))));
        assert!(nests_too_deep(&format!("[^a]: {}x", ">".repeat(65))));
        // A list nested by indentation, a line a level, with any line end.
        for end in ["\n", "\r\n", "\r"] {
            let nested: String = (0..70)
                .map(|level| format!("{}- a{end}", "  ".repeat(level)))
                .collect();
            assert!(nests_too_deep(&nested));
        }
        let tabbed: String = (0..40)
            .map(|level| format!("{}- a\n", "\t".repeat(level)))
            .collect();
        assert!(nests_too_deep(&tabbed));
        assert!(!nests_too_deep(&format!("{}x", ">".repeat(DEEPEST))));
        assert!(nests_too_deep(&format!("{}x", ">".repeat(DEEPEST + 1))));
        assert!(!nests_too_deep("- a\n  - b\n    - c\n      1. d\n"));
    }

    #[test]
    fn deep_emphasis_is_found() {
        assert!(nests_too_deep(&format!(
            "{}a{}",
            "*".repeat(200),
            "*".repeat(200)
        )));
        assert!(nests_too_deep(&format!(
            "{}x{}",
            "*a ".repeat(70),
            " a*".repeat(70)
        )));
        assert!(nests_too_deep(&format!(
            "{}x{}",
            "_a\n".repeat(70),
            "\na_".repeat(70)
        )));
        assert!(nests_too_deep(&format!(
            "{}x{}",
            "~~a ".repeat(70),
            " a~~".repeat(70)
        )));
        // Closing marks hidden in a link's address or in code leave the
        // opening ones waiting.
        assert!(nests_too_deep(&format!(
            "{}x{}",
            "**a [x](u**) ".repeat(70),
            " a**".repeat(70)
        )));
        assert!(nests_too_deep(&format!(
            "{}x{}",
            "**a `b**` ".repeat(70),
            " a**".repeat(70)
        )));
    }

    #[test]
    fn ordinary_documents_pass() {
        // Long lists of bold words, links and code, with no blank line.
        let list: String = (0..500)
            .map(|ix| {
                format!("- **[Project {ix}](https://example.com/a_b_{ix})** - *fast*, `x`, __init__.py, (**it**) and **`--flag`** ~~old~~ by @someone_{ix}\n")
            })
            .collect();
        assert!(!nests_too_deep(&list));
        // Pointers, globs and rules: marks that open nothing.
        let code: String = (0..500)
            .map(|ix| {
                format!(
                    "int f{ix}(struct a *b, char **c);\n*.o\n* * *\n{}\n",
                    "*".repeat(80)
                )
            })
            .collect();
        assert!(!nests_too_deep(&code));
        // A box drawn with stars: after a list marker and five blanks,
        // the rest of a line is code.
        let boxed = format!("*{}*\n", " ".repeat(200));
        assert!(!nests_too_deep(&boxed.repeat(20)));
        // Bold mentions, as release notes have them.
        assert!(!nests_too_deep(
            &"- Fix a thing (#1) **@someone**\n".repeat(500)
        ));
        // Terminal colours: brackets that never close.
        assert!(!nests_too_deep(&"\u{1b}[31mred\u{1b}[0m\n".repeat(200)));
        assert!(nests_too_deep(&"[".repeat(2000)));
        // Closing marks inside an address written out are not read.
        assert!(nests_too_deep(&format!(
            "{}x{}",
            "**a http://x.y/b**)c ".repeat(70),
            " a**".repeat(70)
        )));
        assert!(nests_too_deep(&format!(
            "{}x{}",
            "_a b_.c@d.e ".repeat(70),
            " a_".repeat(70)
        )));
        // A backtick in an address written out opens no code: the next
        // one does, and hides the closing marks after it.
        assert!(nests_too_deep(&format!(
            "{}x{}",
            "**a www.` x` b** `\n".repeat(70),
            " a**".repeat(70)
        )));
        // A backslash that ends an address written out escapes nothing.
        assert!(nests_too_deep(&format!(
            "{}x{}",
            "*a ".repeat(70),
            " www.a\\*".repeat(70)
        )));
        // Escaped marks are no marks, however many.
        assert!(!nests_too_deep(
            &"opj\\_dwt\\_decode\\_1 and \\*x\\* ".repeat(500)
        ));
        // An escaped mark before a run changes what the run is.
        assert!(nests_too_deep(&format!(
            "{}x{}",
            "*a ".repeat(70),
            " a\\**".repeat(70)
        )));
        // A blank line ends the paragraph: nothing adds up across it.
        assert!(!nests_too_deep(&format!(
            "{}\n\n{}",
            "*a ".repeat(70),
            " a*".repeat(70)
        )));
    }

    #[test]
    fn text_that_grows_is_read_from_its_last_paragraph() {
        let before = format!("{}\n\nA reply, so far:\n", "- item\n".repeat(1000));
        assert!(!deepens(&before, "more of it"));
        assert!(deepens(&before, &">".repeat(65)));
        // What makes it deep may come in pieces, each harmless alone.
        let mut text = before;
        let mut deep = false;
        for _ in 0..7 {
            deep |= deepens(&text, ">>>>>>>>>>");
            text.push_str(">>>>>>>>>>");
        }
        assert!(deep);
        assert!(deepens("a\r\n\r\n", &"> ".repeat(65)));
        assert!(!deepens(&format!("{}\n\n", ">".repeat(65)), "after it"));
    }

    /// (Containers, emphasis) at their deepest in the parser's tree of
    /// `text`; `None` when it has none to give.
    fn parsed_depths(text: &str) -> Option<(usize, usize)> {
        use ::markdown::mdast::Node;
        // (The parser panics on a few texts.)
        let root = std::panic::catch_unwind(|| {
            ::markdown::to_mdast(text, &::markdown::ParseOptions::gfm())
        })
        .ok()?
        .ok()?;
        let mut deepest = (0, 0);
        let mut left = vec![(&root, 0, 0)];
        while let Some((node, containers, emphasis)) = left.pop() {
            let (containers, emphasis) = match node {
                Node::Blockquote(_) | Node::ListItem(_) | Node::FootnoteDefinition(_) => {
                    (containers + 1, emphasis)
                }
                Node::Emphasis(_) | Node::Strong(_) | Node::Delete(_) => (containers, emphasis + 1),
                _ => (containers, emphasis),
            };
            deepest = (deepest.0.max(containers), deepest.1.max(emphasis));
            for child in node.children().into_iter().flatten() {
                left.push((child, containers, emphasis));
            }
        }
        Some(deepest)
    }

    /// The promise the guard rests on, held against the parser itself:
    /// whatever it lets through, the parser nests no deeper than the limit.
    /// Texts are made of the pieces that nest and of those that hide marks
    /// from the parser, the same on every run; the limit is a low one, so
    /// that texts this short reach it. (When the parser is replaced by a
    /// newer one, this is what says whether the guard still holds.)
    #[test]
    fn what_passes_the_parser_does_not_nest_deeper() {
        const PIECES: &[&str] = &[
            "*",
            "*",
            "**",
            "***",
            "_",
            "__",
            "~",
            "~~",
            "a",
            "b",
            " ",
            " ",
            "\n",
            "\n\n",
            "[",
            "]",
            "(",
            ")",
            "`",
            "``",
            "\\",
            "<",
            ">",
            "> ",
            "- ",
            "1. ",
            ".",
            ",",
            "\"",
            "!",
            "|",
            "é",
            "\t",
            "    ",
            "[^a]: ",
            "\r",
            "x*a ",
            "b** ",
            "(*a ",
            "[x](u**) ",
            "[x](u \"t** \") ",
            "`c*` ",
            "a* ",
            " *a",
            "a_ ",
            " _a",
            "www.x.com/",
            "https://x.y/",
            "![",
            "](u)",
            "][r]",
            "\n[r]: u\n",
            "~~a ",
            " a~~",
            "**a ",
            " a**",
            "\u{feff}",
            "```\n",
            "a*b",
            "\\_",
            "http://x.y/b**)c ",
            "b_.c@d.e ",
            "](http://y)z*a ",
            "www.` ",
            "www.a\\",
            "x` b** `",
            "-     ",
            "*      ",
            "- \t",
            "\u{a0}",
            "&ast;",
            "<b>",
            "\n  - a",
            "\n    - a",
            "\n      - a",
            "\n\t- a",
            "\n   1. a",
            "\n> - a",
            "\n>   - a",
            "\n-   - a",
            "\n-    - a",
            "a\\** ",
            "\\**a ",
            "*a ",
            " a*",
        ];
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = |below: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % below as u64) as usize
        };
        let mut failed = Vec::new();
        let mut passed = 0;
        for round in 0..20_000 {
            let limit = 1 + round % 3;
            let mut text = String::new();
            for _ in 0..3 + next(30) {
                let piece = PIECES[next(PIECES.len())];
                for _ in 0..1 + next(6) {
                    text.push_str(piece);
                }
            }
            if deeper_than(&text, limit) {
                continue;
            }
            let Some((containers, emphasis)) = parsed_depths(&text) else {
                continue;
            };
            passed += 1;
            if containers > limit || emphasis > limit {
                failed.push((limit, containers, emphasis, text));
            }
        }
        assert!(failed.is_empty(), "{failed:?}");
        // (Enough of them pass for the check to mean something.)
        assert!(passed > 2000, "{passed}");
    }

    #[test]
    fn deep_text_is_shown_plain() {
        assert!(matches!(guarded("# Title\n\n- a\n"), Cow::Borrowed(_)));
        let deep = format!("{}```\nx", ">".repeat(100));
        let shown = guarded(&deep);
        assert!(shown.starts_with("*This text nests too deeply"));
        assert!(shown.contains(&format!("\n````\n{deep}\n````\n")));
        // Guarded again, it is the same: one note, one block.
        assert!(matches!(guarded(&shown), Cow::Borrowed(_)));
        let with_breaks = shown.replacen(".*\n", ".*  \n", 1);
        assert!(matches!(guarded(&with_breaks), Cow::Borrowed(_)));
        // A text that only starts like one, and then nests, is not let by.
        let forged = format!("{NOTE}\n\n```\nx\n```\n{}x", ">".repeat(100));
        assert!(matches!(guarded(&forged), Cow::Owned(_)));
        let forged = format!("{NOTE}\n\n```\n```\n{}x\n```\n", ">".repeat(100));
        assert!(matches!(guarded(&forged), Cow::Owned(_)));
        // Only its start, cut between characters.
        let long = "é".repeat(SHOWN);
        let deep = format!("{}\n{long}", ">".repeat(100));
        let shown = guarded(&deep);
        assert!(shown.len() < SHOWN + 200 && shown.contains("\n…\n"));
    }
}

//! Reading files too big for the editor: gigabytes of logs, dumps and data.
//!
//! The file is read with positioned reads, never memory-mapped, so a file
//! that is truncated while it is open cannot crash the app (a mapping
//! faults). A background scan records where every [`STRIDE`]th line starts,
//! which is all that stays in memory (a 1 GB file of 100-byte lines needs
//! about 80 KB); any line is then found by scanning forward from the
//! checkpoint before it.

use std::fs::File;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

use memchr::memmem;

/// Lines between checkpoints.
pub const STRIDE: u64 = 1024;
/// Bytes read at a time.
pub const CHUNK: usize = 1 << 20;
/// Bytes of a line that are shown; the rest is cut.
pub const MAX_LINE_BYTES: usize = 4 * 1024;
/// Most bytes one call to [`read_lines`] scans, so a file of a few enormous
/// lines cannot stall a frame.
const SCAN_BUDGET: u64 = 64 << 20;

/// Fills `buf` from `offset`, returning fewer bytes only at the end.
pub fn read_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        let at = offset + filled as u64;
        #[cfg(unix)]
        let read = std::os::unix::fs::FileExt::read_at(file, &mut buf[filled..], at);
        #[cfg(windows)]
        let read = std::os::windows::fs::FileExt::seek_read(file, &mut buf[filled..], at);
        match read {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
    Ok(filled)
}

/// Where lines start, every [`STRIDE`] lines, for the part of the file
/// scanned so far. Scanning can resume when the file grows (a log).
#[derive(Debug, Clone)]
pub struct LineIndex {
    /// `checkpoints[k]` is where line `k * STRIDE` starts.
    checkpoints: Vec<u64>,
    /// Newlines seen.
    newlines: u64,
    /// Where the line after the last newline starts.
    tail: u64,
    /// Bytes scanned.
    scanned: u64,
}

impl Default for LineIndex {
    fn default() -> Self {
        Self {
            checkpoints: vec![0],
            newlines: 0,
            tail: 0,
            scanned: 0,
        }
    }
}

impl LineIndex {
    /// Indexes the next bytes of the file (`chunk` starts at `scanned()`).
    pub fn feed(&mut self, chunk: &[u8]) {
        let base = self.scanned;
        for newline in memchr::memchr_iter(b'\n', chunk) {
            let next = base + newline as u64 + 1;
            self.newlines += 1;
            self.tail = next;
            if self.newlines.is_multiple_of(STRIDE) {
                self.checkpoints.push(next);
            }
        }
        self.scanned += chunk.len() as u64;
    }

    pub fn scanned(&self) -> u64 {
        self.scanned
    }

    /// Lines in the scanned part; a last line without a newline counts.
    pub fn lines(&self) -> u64 {
        self.newlines + (self.scanned > self.tail) as u64
    }

    /// The checkpoint at or before `line`: `(its line, its offset)`.
    pub fn anchor(&self, line: u64) -> (u64, u64) {
        let k = ((line / STRIDE) as usize).min(self.checkpoints.len() - 1);
        (k as u64 * STRIDE, self.checkpoints[k])
    }

    /// The checkpoint at or before byte `offset`.
    pub fn anchor_of_offset(&self, offset: u64) -> (u64, u64) {
        let k = self
            .checkpoints
            .partition_point(|&start| start <= offset)
            .max(1)
            - 1;
        (k as u64 * STRIDE, self.checkpoints[k])
    }
}

/// Scans `file` from where `index` stopped up to `len`, calling `progress`
/// after each chunk. Stops early (returning `Ok(false)`) when `cancel` is set.
pub fn scan(
    file: &File,
    len: u64,
    index: &std::sync::Mutex<LineIndex>,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64),
) -> io::Result<bool> {
    let mut buf = vec![0u8; CHUNK];
    let mut offset = index.lock().unwrap_or_else(|e| e.into_inner()).scanned();
    while offset < len {
        if cancel.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let want = CHUNK.min((len - offset) as usize);
        let read = read_at(file, &mut buf[..want], offset)?;
        if read == 0 {
            break;
        }
        index
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .feed(&buf[..read]);
        offset += read as u64;
        progress(offset);
    }
    Ok(true)
}

/// One line as shown.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub number: u64,
    /// Where it starts, and where its text ends (before the newline).
    pub start: u64,
    pub end: u64,
    /// At most [`MAX_LINE_BYTES`] of it, made printable.
    pub text: String,
}

impl Line {
    /// Longer than what is shown.
    pub fn is_cut(&self) -> bool {
        self.end - self.start > MAX_LINE_BYTES as u64
    }
}

/// Lines `first..first + count` (fewer at the end), scanning from `anchor`
/// (`(line, offset)` at or before `first`: a checkpoint, or a line read
/// earlier).
pub fn read_lines(
    file: &File,
    len: u64,
    anchor: (u64, u64),
    first: u64,
    count: usize,
) -> io::Result<Vec<Line>> {
    let (mut line, mut offset) = anchor;
    let mut out = Vec::with_capacity(count);
    let mut buf = vec![0u8; CHUNK];
    let mut start = offset;
    let mut shown: Vec<u8> = Vec::new();
    let budget_end = offset.saturating_add(SCAN_BUDGET);
    while out.len() < count && offset < len && offset < budget_end {
        let read = read_at(file, &mut buf[..CHUNK.min((len - offset) as usize)], offset)?;
        if read == 0 {
            break;
        }
        let chunk = &buf[..read];
        let mut from = 0;
        for newline in memchr::memchr_iter(b'\n', chunk) {
            if line >= first {
                keep(&mut shown, &chunk[from..newline]);
                out.push(finish(line, start, offset + newline as u64, &mut shown));
                if out.len() == count {
                    return Ok(out);
                }
            }
            line += 1;
            from = newline + 1;
            start = offset + from as u64;
        }
        if line >= first {
            keep(&mut shown, &chunk[from..]);
        }
        offset += read as u64;
    }
    // The last line, without a newline (or cut by the scan budget).
    if out.len() < count && line >= first && start < offset {
        out.push(finish(line, start, offset, &mut shown));
    }
    Ok(out)
}

/// Adds bytes of the current line, up to the shown maximum.
fn keep(shown: &mut Vec<u8>, bytes: &[u8]) {
    let room = MAX_LINE_BYTES.saturating_sub(shown.len());
    shown.extend_from_slice(&bytes[..bytes.len().min(room)]);
}

fn finish(number: u64, start: u64, end: u64, shown: &mut Vec<u8>) -> Line {
    let text = printable(shown);
    shown.clear();
    Line {
        number,
        start,
        end,
        text,
    }
}

/// Lossy UTF-8 without the line's `\r`, with tabs expanded and other
/// control characters shown as `·`.
fn printable(bytes: &[u8]) -> String {
    let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
    let mut text = String::with_capacity(bytes.len());
    for c in String::from_utf8_lossy(bytes).chars() {
        match c {
            '\t' => text.push_str("    "),
            c if c.is_control() => text.push('·'),
            c => text.push(c),
        }
    }
    text
}

/// The line `offset` is on.
pub fn line_of(file: &File, index: &LineIndex, offset: u64) -> io::Result<u64> {
    let (mut line, mut at) = index.anchor_of_offset(offset);
    let mut buf = vec![0u8; CHUNK];
    while at < offset {
        let want = CHUNK.min((offset - at) as usize);
        let read = read_at(file, &mut buf[..want], at)?;
        if read == 0 {
            break;
        }
        line += memchr::memchr_iter(b'\n', &buf[..read]).count() as u64;
        at += read as u64;
    }
    Ok(line)
}

/// Raw bytes `start..end`, at most `cap` of them (for copying lines).
pub fn read_range(file: &File, start: u64, end: u64, cap: usize) -> io::Result<Vec<u8>> {
    let mut buf = vec![0u8; (end.saturating_sub(start) as usize).min(cap)];
    let read = read_at(file, &mut buf, start)?;
    buf.truncate(read);
    Ok(buf)
}

/// What to look for: smart case, so lowercase text matches any case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    needle: Vec<u8>,
    ignore_case: bool,
}

impl Query {
    pub fn new(text: &str) -> Option<Self> {
        if text.is_empty() {
            return None;
        }
        let ignore_case = !text.chars().any(char::is_uppercase);
        let needle = if ignore_case {
            text.to_ascii_lowercase()
        } else {
            text.to_string()
        };
        Some(Self {
            needle: needle.into_bytes(),
            ignore_case,
        })
    }

    pub fn len(&self) -> usize {
        self.needle.len()
    }

    pub fn is_empty(&self) -> bool {
        self.needle.is_empty()
    }
}

/// The first match starting at or after `from`, or `None`. Checks `cancel`
/// between chunks.
pub fn find_next(
    file: &File,
    len: u64,
    query: &Query,
    from: u64,
    cancel: &AtomicBool,
) -> io::Result<Option<u64>> {
    let finder = memmem::Finder::new(&query.needle);
    let overlap = query.needle.len().saturating_sub(1) as u64;
    let mut buf = vec![0u8; CHUNK + query.needle.len()];
    let mut at = from;
    while at < len {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let want = buf.len().min((len - at) as usize);
        let read = read_at(file, &mut buf[..want], at)?;
        if read == 0 {
            break;
        }
        let hay = &mut buf[..read];
        if query.ignore_case {
            hay.make_ascii_lowercase();
        }
        if let Some(found) = finder.find(hay) {
            return Ok(Some(at + found as u64));
        }
        if (read as u64) <= overlap {
            break;
        }
        at += read as u64 - overlap;
    }
    Ok(None)
}

/// The last match starting before `before`, or `None`.
pub fn find_prev(
    file: &File,
    query: &Query,
    before: u64,
    cancel: &AtomicBool,
) -> io::Result<Option<u64>> {
    let finder = memmem::FinderRev::new(&query.needle);
    let overlap = query.needle.len().saturating_sub(1) as u64;
    let mut buf = vec![0u8; CHUNK + query.needle.len()];
    // Matches must start before `before`, so they may end up to
    // `needle - 1` bytes after it.
    let mut end = before.saturating_add(overlap);
    while end > 0 {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let start = end.saturating_sub(buf.len() as u64);
        let read = read_at(file, &mut buf[..(end - start) as usize], start)?;
        let hay = &mut buf[..read];
        if query.ignore_case {
            hay.make_ascii_lowercase();
        }
        let mut search = &hay[..];
        while let Some(found) = finder.rfind(search) {
            let offset = start + found as u64;
            if offset < before {
                return Ok(Some(offset));
            }
            search = &search[..found + query.needle.len() - 1];
        }
        if start == 0 {
            break;
        }
        end = start + overlap;
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::sync::Mutex;

    fn file_with(name: &str, bytes: &[u8]) -> (std::path::PathBuf, File) {
        let dir = std::env::temp_dir().join(format!("ph-large-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::File::create(&path)
            .unwrap()
            .write_all(bytes)
            .unwrap();
        let file = File::open(&path).unwrap();
        (path, file)
    }

    fn indexed(file: &File, len: u64) -> LineIndex {
        let index = Mutex::new(LineIndex::default());
        assert!(scan(file, len, &index, &AtomicBool::new(false), |_| {}).unwrap());
        index.into_inner().unwrap()
    }

    #[test]
    fn counts_lines_with_and_without_a_final_newline() {
        let mut index = LineIndex::default();
        assert_eq!(index.lines(), 0);
        index.feed(b"a\nb");
        assert_eq!(index.lines(), 2);
        index.feed(b"\n");
        assert_eq!(
            index.lines(),
            2,
            "a final newline ends the line, it adds none"
        );
        index.feed(b"\n");
        assert_eq!(index.lines(), 3, "an empty line");
    }

    #[test]
    fn finds_any_line_through_checkpoints() {
        let text: String = (0..5000).map(|n| format!("line {n}\n")).collect();
        let (_, file) = file_with("many.txt", text.as_bytes());
        let len = text.len() as u64;
        let index = indexed(&file, len);
        assert_eq!(index.lines(), 5000);
        assert_eq!(
            index.checkpoints.len(),
            5,
            "one per 1024 lines, plus line 0"
        );

        for first in [0, 1023, 1024, 3000, 4998] {
            let lines = read_lines(&file, len, index.anchor(first), first, 3).unwrap();
            let numbers: Vec<_> = lines.iter().map(|line| line.number).collect();
            let expected: Vec<_> = (first..(first + 3).min(5000)).collect();
            assert_eq!(numbers, expected);
            assert_eq!(lines[0].text, format!("line {first}"));
            assert_eq!(line_of(&file, &index, lines[0].start).unwrap(), first);
        }
        // Reading on from a line read earlier gives the same lines.
        let earlier = &read_lines(&file, len, index.anchor(3000), 3000, 1).unwrap()[0];
        assert_eq!(
            read_lines(&file, len, (3000, earlier.start), 3002, 1).unwrap()[0].text,
            "line 3002"
        );
    }

    #[test]
    fn long_lines_are_cut_and_text_made_printable() {
        let mut bytes = vec![b'x'; MAX_LINE_BYTES + 100];
        bytes.extend_from_slice(b"\n\tTab\x01\r\nlast");
        let (_, file) = file_with("long.txt", &bytes);
        let len = bytes.len() as u64;
        let lines = read_lines(&file, len, (0, 0), 0, 10).unwrap();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].is_cut());
        assert_eq!(lines[0].text.len(), MAX_LINE_BYTES);
        assert_eq!(lines[1].text, "    Tab·");
        assert_eq!((lines[2].number, lines[2].text.as_str()), (2, "last"));
    }

    #[test]
    fn scanning_resumes_when_a_log_grows() {
        let (path, file) = file_with("grow.log", b"one\ntwo\n");
        let index = Mutex::new(LineIndex::default());
        let cancel = AtomicBool::new(false);
        scan(&file, 8, &index, &cancel, |_| {}).unwrap();
        assert_eq!(index.lock().unwrap().lines(), 2);
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"three\nfou")
            .unwrap();
        let len = std::fs::metadata(&path).unwrap().len();
        scan(&file, len, &index, &cancel, |_| {}).unwrap();
        assert_eq!(index.lock().unwrap().lines(), 4);
        cancel.store(true, Ordering::Relaxed);
        let fresh = Mutex::new(LineIndex::default());
        assert!(!scan(&file, len, &fresh, &cancel, |_| {}).unwrap());
    }

    #[test]
    fn searches_both_ways_with_smart_case() {
        let mut text = "x".repeat(CHUNK - 3);
        text.push_str("Needle at the seam; needle again\nNEEDLE");
        let (_, file) = file_with("search.txt", text.as_bytes());
        let len = text.len() as u64;
        let cancel = AtomicBool::new(false);
        let seam = (CHUNK - 3) as u64;

        let any = Query::new("needle").unwrap();
        assert_eq!(find_next(&file, len, &any, 0, &cancel).unwrap(), Some(seam));
        let second = find_next(&file, len, &any, seam + 1, &cancel)
            .unwrap()
            .unwrap();
        assert_eq!(&text[second as usize..second as usize + 6], "needle");
        let third = find_next(&file, len, &any, second + 1, &cancel)
            .unwrap()
            .unwrap();
        assert_eq!(&text[third as usize..], "NEEDLE");
        assert_eq!(
            find_next(&file, len, &any, third + 1, &cancel).unwrap(),
            None
        );

        assert_eq!(
            find_prev(&file, &any, third, &cancel).unwrap(),
            Some(second)
        );
        assert_eq!(find_prev(&file, &any, second, &cancel).unwrap(), Some(seam));
        assert_eq!(find_prev(&file, &any, seam, &cancel).unwrap(), None);

        let exact = Query::new("NEEDLE").unwrap();
        assert_eq!(
            find_next(&file, len, &exact, 0, &cancel).unwrap(),
            Some(third)
        );
        assert_eq!(Query::new(""), None);
    }
}

//! Spreadsheets (Excel and OpenDocument), read-only: a tab per sheet, the
//! cells in a table with the sheet's column letters.

use std::io::{BufRead, Read, Seek};
use std::path::{Path, PathBuf};

use calamine::{Data, Dimensions, ExcelDateTime, Reader as _, Sheets, open_workbook_auto};
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    tab::{Tab, TabBar},
    v_flex,
};
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Task, Window, div,
};

use crate::byte_size;
use crate::table::{TableData, TableGrid};

/// Rows read from a sheet at most.
const MAX_ROWS: usize = 20_000;
/// A workbook file larger than this is not opened.
const MAX_FILE: u64 = 64 << 20;
/// What a zipped workbook (.xlsx, .xlsb, .ods) may unpack to, all its parts
/// together: its texts are read whole into memory.
const MAX_UNPACKED: u64 = 256 << 20;
/// The parts a zipped workbook may have.
const MAX_PARTS: usize = 10_000;
/// The bytes one shared text takes in its part at the least (`<si/>`).
const SHARED_TEXT_BYTES: u64 = 5;
/// A count of shared texts up to this is let through whatever the part's
/// size: room for as many is small, and some programs write the number of
/// cells there.
const HARMLESS_TEXTS: u64 = 1 << 22;

pub struct SpreadsheetView {
    path: PathBuf,
    sheets: Option<Result<Vec<String>, SharedString>>,
    selected: usize,
    grid: Entity<TableGrid>,
    error: Option<SharedString>,
    _load: Option<Task<()>>,
}

impl SpreadsheetView {
    pub fn new(path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            path,
            sheets: None,
            selected: 0,
            grid: cx.new(|cx| TableGrid::new(window, cx)),
            error: None,
            _load: None,
        };
        this.reload(cx);
        this
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.show(self.selected, cx);
    }

    fn show(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.selected = ix;
        let path = self.path.clone();
        let read = cx.background_spawn(async move { read_sheet(&path, ix) });
        self._load = Some(cx.spawn(async move |this, cx| {
            let result = read.await;
            _ = this.update(cx, |this, cx| {
                match result {
                    Ok((sheets, data, note)) => {
                        this.sheets = Some(Ok(sheets));
                        this.error = None;
                        this.grid
                            .update(cx, |grid, cx| grid.set_data(data, note.map(Into::into), cx));
                    }
                    Err(err) if this.sheets.is_none() => {
                        this.sheets = Some(Err(format!("{err:#}").into()))
                    }
                    Err(err) => this.error = Some(format!("{err:#}").into()),
                }
                cx.notify();
            });
        }));
    }
}

/// The sheet names, and sheet `ix`'s cells.
pub fn read_sheet(
    path: &Path,
    ix: usize,
) -> anyhow::Result<(Vec<String>, TableData, Option<String>)> {
    check_workbook(path)?;
    let mut workbook = open_workbook_auto(path)?;
    let sheets = workbook.sheet_names();
    let name = sheets
        .get(ix)
        .or(sheets.first())
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("The workbook has no sheets."))?;
    // Excel files are read cell by cell, up to the rows shown: a sheet's
    // grid can be far bigger than its cells (one cell at the last row and
    // column), so it is never laid out whole.
    let mut grid = Grid::default();
    match &mut workbook {
        Sheets::Xlsx(book) => {
            let mut cells = book.worksheet_cells_reader(&name)?;
            grid.start(cells.dimensions());
            while let Some(cell) = cells.next_cell()? {
                if !grid.add(cell.get_position(), Data::from(cell.get_value().clone())) {
                    break;
                }
            }
        }
        Sheets::Xlsb(book) => {
            let mut cells = book.worksheet_cells_reader(&name)?;
            grid.start(cells.dimensions());
            while let Some(cell) = cells.next_cell()? {
                if !grid.add(cell.get_position(), Data::from(cell.get_value().clone())) {
                    break;
                }
            }
        }
        _ => {
            let range = workbook.worksheet_range(&name)?;
            if let (Some(start), Some(end)) = (range.start(), range.end()) {
                grid.start(Dimensions { start, end });
            }
            for (row, column, value) in range.used_cells() {
                let at = (grid.first.0 + row as u32, grid.first.1 + column as u32);
                if !grid.add(at, value.clone()) {
                    break;
                }
            }
        }
    }
    let (data, note) = grid.finish();
    Ok((sheets, data, note))
}

/// Refuses a workbook that would take too much memory to read, as far as
/// can be told before reading it. The library believes what a file says
/// about itself (how many texts it holds), makes room for it at once, and
/// a program that runs out of memory ends: the whole app, here.
fn check_workbook(path: &Path) -> anyhow::Result<()> {
    let file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    anyhow::ensure!(
        size <= MAX_FILE,
        "The file is {}: workbooks up to {} are shown here.",
        byte_size(size),
        byte_size(MAX_FILE)
    );
    // The zipped kinds say what they unpack to; of an .xls no more is known.
    match zip::ZipArchive::new(std::io::BufReader::new(file)) {
        Ok(mut archive) => check_archive(&mut archive, MAX_UNPACKED),
        Err(_) => Ok(()),
    }
}

/// A zipped workbook's parts: what they unpack to (reading stops at the
/// size a part declares, so these hold), and the texts it says it shares.
fn check_archive<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    most: u64,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        archive.len() <= MAX_PARTS,
        "The file holds {} parts, more than a workbook has.",
        archive.len()
    );
    let mut unpacked = 0u64;
    let mut shared = Vec::new();
    for ix in 0..archive.len() {
        let part = archive.by_index_raw(ix)?;
        unpacked = unpacked.saturating_add(part.size());
        // Found whatever folder the workbook's parts are in, and whatever
        // the case, as the library finds it.
        let name = part.name()?.replace('\\', "/").to_ascii_lowercase();
        if name == "sharedstrings.xml" || name.ends_with("/sharedstrings.xml") {
            shared.push(ix);
        }
    }
    anyhow::ensure!(
        unpacked <= most,
        "The file unpacks to {}, more than the {} read here.",
        byte_size(unpacked),
        byte_size(most)
    );
    for ix in shared {
        // A part that cannot be read is the library's to report.
        let Ok(part) = archive.by_index(ix) else {
            continue;
        };
        let room = (part.size() / SHARED_TEXT_BYTES).max(HARMLESS_TEXTS);
        let claimed = claimed_texts(std::io::BufReader::new(part));
        anyhow::ensure!(
            claimed <= room,
            "The file says it holds {claimed} texts, more than it has room for."
        );
    }
    Ok(())
}

/// The largest `uniqueCount="…"` in a shared-texts part, wherever it is
/// written: the library makes room for the one in the first `sst` tag it
/// meets, and telling which that is would be reading XML just as it does.
fn claimed_texts(mut part: impl BufRead) -> u64 {
    const KEY: &[u8] = b"uniqueCount";
    // How much of the key still matches when its next byte does not.
    const BACK: [usize; 11] = [0, 0, 0, 0, 1, 0, 0, 0, 1, 2, 0];
    #[derive(Clone, Copy)]
    enum At {
        Key(usize),
        Equals,
        Quote,
        Digits(u64),
    }
    let key = |mut matched: usize, byte: u8| {
        while matched > 0 && byte != KEY[matched] {
            matched = BACK[matched - 1];
        }
        if byte == KEY[matched] {
            matched += 1;
        }
        if matched == KEY.len() {
            At::Equals
        } else {
            At::Key(matched)
        }
    };
    let mut at = At::Key(0);
    let mut most = 0u64;
    // A part cut short is read as far as it goes, as the library reads it.
    while let Ok(bytes) = part.fill_buf() {
        if bytes.is_empty() {
            break;
        }
        for &byte in bytes {
            at = match at {
                At::Key(matched) => key(matched, byte),
                At::Equals | At::Quote if byte.is_ascii_whitespace() => at,
                At::Equals if byte == b'=' => At::Quote,
                At::Quote if byte == b'"' || byte == b'\'' => At::Digits(0),
                At::Digits(count) if byte.is_ascii_digit() => At::Digits(
                    count
                        .saturating_mul(10)
                        .saturating_add((byte - b'0') as u64),
                ),
                At::Digits(count) => {
                    most = most.max(count);
                    key(0, byte)
                }
                At::Equals | At::Quote => key(0, byte),
            };
        }
        let read = bytes.len();
        part.consume(read);
    }
    if let At::Digits(count) = at {
        most = most.max(count);
    }
    most
}

/// Columns read from a sheet at most.
const MAX_COLUMNS: usize = 1_000;

/// A sheet's cells, gathered as they are read.
#[derive(Default)]
struct Grid {
    /// The sheet's first row and column.
    first: (u32, u32),
    /// Rows the sheet says it has.
    height: usize,
    rows: Vec<Vec<SharedString>>,
    width: usize,
}

impl Grid {
    fn start(&mut self, dimensions: Dimensions) {
        self.first = dimensions.start;
        self.height = (dimensions.end.0.saturating_sub(dimensions.start.0) as usize) + 1;
    }

    /// Puts a cell in; false once past the rows shown.
    fn add(&mut self, (row, column): (u32, u32), value: Data) -> bool {
        let row = row.saturating_sub(self.first.0) as usize;
        let column = column.saturating_sub(self.first.1) as usize;
        if row >= MAX_ROWS {
            return false;
        }
        if column >= MAX_COLUMNS || matches!(value, Data::Empty) {
            return true;
        }
        if self.rows.len() <= row {
            self.rows.resize_with(row + 1, Vec::new);
        }
        let cells = &mut self.rows[row];
        if cells.len() <= column {
            cells.resize(column + 1, SharedString::default());
        }
        cells[column] = cell_text(&value).into();
        self.width = self.width.max(column + 1);
        true
    }

    fn finish(self) -> (TableData, Option<String>) {
        let columns = (0..self.width)
            .map(|ix| column_name(self.first.1 as usize + ix).into())
            .collect();
        let height = self.height.max(self.rows.len());
        let note = (height > MAX_ROWS).then(|| format!("first {MAX_ROWS} of {height} rows"));
        let data = TableData {
            columns,
            rows: self.rows,
        };
        (data, note)
    }
}

/// A, B, … Z, AA, AB…
pub fn column_name(mut ix: usize) -> String {
    let mut name = String::new();
    loop {
        name.insert(0, (b'A' + (ix % 26) as u8) as char);
        if ix < 26 {
            break;
        }
        ix = ix / 26 - 1;
    }
    name
}

fn cell_text(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::Float(value) if value.fract() == 0. && value.abs() < 1e15 => {
            format!("{}", *value as i64)
        }
        Data::DateTime(when) => date_text(when),
        other => other.to_string(),
    }
}

/// A date or time cell as the sheet means it, not the count of days it is
/// stored as: `2023-07-16`, with the time when it has one; a time of day
/// alone, or a length of time, as `H:MM:SS`.
fn date_text(when: &ExcelDateTime) -> String {
    let days = when.as_f64();
    if !days.is_finite() {
        return days.to_string();
    }
    // A length of time, or a time of day (less than a day, so no date).
    if when.is_duration() || (0. ..1.).contains(&days) {
        let seconds = (days.abs() * 86_400.).round() as u64;
        let sign = if days < 0. { "-" } else { "" };
        return format!(
            "{sign}{}:{:02}:{:02}",
            seconds / 3_600,
            seconds / 60 % 60,
            seconds % 60
        );
    }
    // No date is before the first day.
    if days < 0. {
        return days.to_string();
    }
    let (year, month, day, hour, minute, second, _) = when.to_ymd_hms_milli();
    let date = format!("{year:04}-{month:02}-{day:02}");
    if (hour, minute, second) == (0, 0, 0) {
        date
    } else {
        format!("{date} {hour:02}:{minute:02}:{second:02}")
    }
}

impl Render for SpreadsheetView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sheets = match &self.sheets {
            None => return crate::notice("Reading the workbook…", cx),
            Some(Err(err)) => return crate::notice(format!("Not a readable workbook: {err}"), cx),
            Some(Ok(sheets)) => sheets.clone(),
        };
        let body = match &self.error {
            Some(error) => crate::notice(error.clone(), cx),
            None => self.grid.clone().into_any_element(),
        };
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(div().flex_1().min_h_0().child(body))
            .child(
                TabBar::new("sheets")
                    .small()
                    .selected_index(self.selected)
                    .on_click(cx.listener(|this, ix: &usize, _, cx| this.show(*ix, cx)))
                    .children(sheets.into_iter().map(|name| Tab::new().label(name))),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_letters() {
        assert_eq!(column_name(0), "A");
        assert_eq!(column_name(25), "Z");
        assert_eq!(column_name(26), "AA");
        assert_eq!(column_name(27), "AB");
        assert_eq!(column_name(701), "ZZ");
        assert_eq!(column_name(702), "AAA");
        assert_eq!(cell_text(&Data::Float(3.0)), "3");
        assert_eq!(cell_text(&Data::Float(2.5)), "2.5");
    }

    #[test]
    fn dates_show_as_dates() {
        use calamine::ExcelDateTimeType::{DateTime, TimeDelta};
        let cell = |days, kind| cell_text(&Data::DateTime(ExcelDateTime::new(days, kind, false)));
        assert_eq!(cell(45_123., DateTime), "2023-07-16");
        assert_eq!(cell(45_123.5, DateTime), "2023-07-16 12:00:00");
        // A time of day, and lengths of time (which pass a day).
        assert_eq!(cell(0.25, DateTime), "6:00:00");
        assert_eq!(cell(1.5, TimeDelta), "36:00:00");
        assert_eq!(cell(-0.5, TimeDelta), "-12:00:00");
        assert_eq!(cell(f64::NAN, DateTime), "NaN");
    }

    #[test]
    fn the_texts_a_part_claims() {
        let claimed = |xml: &str| claimed_texts(xml.as_bytes());
        assert_eq!(claimed(r#"<sst count="9" uniqueCount="3"><si/></sst>"#), 3);
        assert_eq!(claimed("<x:sst uniqueCount = '0042'>"), 42);
        assert_eq!(claimed("<sst>"), 0);
        // Not the first one only: the first may be in a comment.
        assert_eq!(
            claimed(r#"<!-- uniqueCount="1" --><sst uniqueCount="999999999999">"#),
            999_999_999_999
        );
        assert_eq!(
            claimed(r#"<sst uniquniqueCount="7" uniqueCouniqueCount="8">"#),
            8
        );
        assert_eq!(
            claimed(r#"<sst uniqueCount="99999999999999999999999999">"#),
            u64::MAX
        );
        // However the bytes arrive.
        let slow = std::io::BufReader::with_capacity(3, &b"<sst uniqueCount=\"1234\">"[..]);
        assert_eq!(claimed_texts(slow), 1_234);
    }

    /// A zipped workbook with one part.
    fn zipped(name: &str, text: &str) -> zip::ZipArchive<std::io::Cursor<Vec<u8>>> {
        use std::io::Write as _;
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(text.as_bytes()).unwrap();
        zip::ZipArchive::new(writer.finish().unwrap()).unwrap()
    }

    #[test]
    fn a_workbook_that_claims_too_much_is_refused() {
        let honest = r#"<sst count="2" uniqueCount="2"><si><t>a</t></si><si><t>b</t></si></sst>"#;
        assert!(check_archive(&mut zipped("xl/sharedStrings.xml", honest), MAX_UNPACKED).is_ok());
        // More than the part holds, but too few to matter.
        let loose = r#"<sst uniqueCount="50000"><si><t>a</t></si></sst>"#;
        assert!(check_archive(&mut zipped("xl/sharedStrings.xml", loose), MAX_UNPACKED).is_ok());
        let hostile = r#"<sst uniqueCount="999999999999"></sst>"#;
        for name in [
            "xl/sharedStrings.xml",
            "XL\\SHAREDSTRINGS.XML",
            "sharedStrings.xml",
        ] {
            let err = check_archive(&mut zipped(name, hostile), MAX_UNPACKED).unwrap_err();
            assert!(err.to_string().contains("999999999999 texts"), "{err}");
        }
        // Another part may say what it likes.
        assert!(check_archive(&mut zipped("xl/workbook.xml", hostile), MAX_UNPACKED).is_ok());
        // What it unpacks to is counted before anything is unpacked.
        let err = check_archive(&mut zipped("content.xml", &"a".repeat(2_000)), 1_000).unwrap_err();
        assert!(err.to_string().contains("unpacks to"), "{err}");
    }

    #[test]
    fn a_far_cell_lays_out_nothing_big() {
        let mut grid = Grid::default();
        grid.start(Dimensions {
            start: (2, 1),
            end: (1_048_575, 16_383),
        });
        assert!(grid.add((2, 1), Data::String("name".into())));
        assert!(grid.add((3, 2), Data::Float(4.)));
        assert!(grid.add((4, 16_383), Data::Float(9.)));
        assert!(!grid.add((1_048_575, 16_383), Data::Float(1.)));
        let (data, note) = grid.finish();
        assert_eq!(data.columns, ["B", "C"]);
        assert_eq!(data.rows, [vec!["name"], vec!["", "4"]]);
        assert_eq!(note.as_deref(), Some("first 20000 of 1048574 rows"));
    }
}

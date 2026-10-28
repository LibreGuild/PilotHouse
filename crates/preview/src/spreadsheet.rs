//! Spreadsheets (Excel and OpenDocument), read-only: a tab per sheet, the
//! cells in a table with the sheet's column letters.

use std::path::{Path, PathBuf};

use calamine::{Data, Dimensions, Reader as _, Sheets, open_workbook_auto};
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    tab::{Tab, TabBar},
    v_flex,
};
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Task, Window, div,
};

use crate::table::{TableData, TableGrid};

/// Rows read from a sheet at most.
const MAX_ROWS: usize = 20_000;

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
        other => other.to_string(),
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

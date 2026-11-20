//! Tables: a grid with a sticky header, sorting by a column and a filter,
//! for CSV and TSV files, database tables and spreadsheets.

use std::ops::Range;
use std::sync::Arc;

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    Task, UniformListScrollHandle, Window, div, uniform_list,
};

/// Rows of text under column names.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TableData {
    pub columns: Vec<SharedString>,
    pub rows: Vec<Vec<SharedString>>,
}

const ROW_HEIGHT: f32 = 26.;

/// A cell on its row's one line: line breaks inside it (a spreadsheet's
/// wrapped text) show as `⏎` rather than spilling over the rows below.
fn one_line(cell: &SharedString) -> SharedString {
    if cell.contains(['\n', '\r']) {
        cell.replace("\r\n", " ⏎ ")
            .replace(['\n', '\r'], " ⏎ ")
            .into()
    } else {
        cell.clone()
    }
}
const MIN_WIDTH: f32 = 64.;
const MAX_WIDTH: f32 = 360.;
/// The row-number column.
const NUMBER_WIDTH: f32 = 56.;
/// Columns are built this far past each side of what is in view.
const OVERSCAN: f32 = 400.;

/// A table, sortable and filtered as typed.
pub struct TableGrid {
    data: Arc<TableData>,
    widths: Vec<f32>,
    /// Each column's left edge and, last, the right edge of the last one
    /// (from the first column's left edge): which columns are in view.
    starts: Vec<f32>,
    /// Rows shown, in order (after the filter and the sort).
    order: Arc<Vec<usize>>,
    sort: Option<(usize, bool)>,
    filter: Entity<InputState>,
    /// Shown above the grid ("first 1,000 rows").
    note: Option<SharedString>,
    rows_scroll: UniformListScrollHandle,
    across: ScrollHandle,
    _filter: Subscription,
    _reorder: Option<Task<()>>,
}

/// Tables up to this many rows are filtered and sorted at once; bigger ones
/// in the background, so typing in the filter stays quick.
const ORDER_AT_ONCE: usize = 5_000;

impl TableGrid {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter rows"));
        let subscription = cx.subscribe(&filter, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.reorder(cx);
            }
        });
        Self {
            data: Arc::default(),
            widths: Vec::new(),
            starts: vec![0.],
            order: Arc::default(),
            sort: None,
            filter,
            note: None,
            rows_scroll: UniformListScrollHandle::new(),
            across: ScrollHandle::new(),
            _filter: subscription,
            _reorder: None,
        }
    }

    pub fn set_data(
        &mut self,
        data: TableData,
        note: Option<SharedString>,
        cx: &mut Context<Self>,
    ) {
        // A new table (another one picked) loses the old sort.
        if data.columns != self.data.columns {
            self.sort = None;
        }
        self.widths = widths(&data);
        self.starts = starts(&self.widths);
        // Rows in their own order until filtered and sorted.
        self.order = Arc::new((0..data.rows.len()).collect());
        self.data = Arc::new(data);
        self.note = note;
        self.reorder(cx);
    }

    fn reorder(&mut self, cx: &mut Context<Self>) {
        let query = self.filter.read(cx).value().trim().to_lowercase();
        let (data, sort) = (self.data.clone(), self.sort);
        if data.rows.len() <= ORDER_AT_ONCE {
            self._reorder = None;
            self.order = Arc::new(order_rows(&data, &query, sort));
            cx.notify();
            return;
        }
        let work = cx.background_spawn({
            let data = data.clone();
            async move { order_rows(&data, &query, sort) }
        });
        self._reorder = Some(cx.spawn(async move |this, cx| {
            let order = work.await;
            _ = this.update(cx, |this, cx| {
                // Only for the rows it was worked out for.
                if Arc::ptr_eq(&this.data, &data) {
                    this.order = Arc::new(order);
                    cx.notify();
                }
            });
        }));
    }

    fn sort_by(&mut self, column: usize, cx: &mut Context<Self>) {
        self.sort = match self.sort {
            Some((current, true)) if current == column => Some((column, false)),
            Some((current, false)) if current == column => None,
            _ => Some((column, true)),
        };
        self.reorder(cx);
    }
}

/// The rows matching `query` (lowercase), in `sort`'s order: by a column,
/// ascending or not, numbers as numbers when the whole column is numbers.
pub(crate) fn order_rows(data: &TableData, query: &str, sort: Option<(usize, bool)>) -> Vec<usize> {
    let mut order: Vec<usize> = (0..data.rows.len())
        .filter(|&ix| {
            query.is_empty()
                || data.rows[ix]
                    .iter()
                    .any(|cell| cell.to_lowercase().contains(query))
        })
        .collect();
    let Some((column, ascending)) = sort else {
        return order;
    };
    let cell = |ix: usize| data.rows[ix].get(column).map_or("", |cell| cell.as_ref());
    let numbers: Option<Vec<Option<f64>>> = (0..data.rows.len())
        .map(|ix| {
            let text = cell(ix).trim();
            if text.is_empty() {
                Some(None)
            } else {
                number(text).map(Some)
            }
        })
        .collect();
    // Each cell's key is worked out once, not at every comparison.
    match numbers {
        // `NaN` reads as a number too, and has to have its place (after
        // the others): sorting needs every pair to compare.
        Some(numbers) => order.sort_by(|a, b| match (numbers[*a], numbers[*b]) {
            (Some(a), Some(b)) => a.total_cmp(&b),
            (a, b) => a.is_some().cmp(&b.is_some()),
        }),
        None => {
            let texts: Vec<String> = (0..data.rows.len())
                .map(|ix| cell(ix).to_lowercase())
                .collect();
            order.sort_by(|a, b| texts[*a].cmp(&texts[*b]));
        }
    }
    if !ascending {
        order.reverse();
    }
    order
}

fn number(text: &str) -> Option<f64> {
    text.trim().replace(',', "").parse::<f64>().ok()
}

/// Each column's left edge, and the right edge of the last.
fn starts(widths: &[f32]) -> Vec<f32> {
    let mut edge = 0.;
    let mut starts = Vec::with_capacity(widths.len() + 1);
    starts.push(edge);
    for width in widths {
        edge += width;
        starts.push(edge);
    }
    starts
}

/// The columns with any part between `from` and `to` (`starts` as
/// [`starts`] gives them).
fn columns_between(starts: &[f32], from: f32, to: f32) -> Range<usize> {
    let count = starts.len().saturating_sub(1);
    let first = starts[1..].partition_point(|right| *right <= from);
    let end = starts[..count].partition_point(|left| *left < to);
    first..end.max(first)
}

impl TableGrid {
    /// The columns to build: those in view where the table is scrolled to,
    /// and some either side. A table can have thousands of columns, and
    /// building a cell for each, in every row, every frame, hangs the app.
    fn columns_in_view(&self, window: &Window) -> Range<usize> {
        // Widths are pixels at the usual interface size.
        let scale = f32::from(ph_ui::rems_from_px(1.).to_pixels(window.rem_size()));
        if scale <= 0. {
            return 0..self.widths.len();
        }
        let content = NUMBER_WIDTH + self.starts.last().copied().unwrap_or(0.);
        // Where the scroll will be once laid out: never past the end (a
        // table swapped for a narrower one, a hard flick).
        let window_width = f32::from(window.viewport_size().width) / scale;
        let pane = match f32::from(self.across.bounds().size.width) / scale {
            width if width > 0. => width,
            _ => window_width,
        };
        let left = (-f32::from(self.across.offset().x) / scale).clamp(0., (content - pane).max(0.))
            - NUMBER_WIDTH;
        columns_between(
            &self.starts,
            left - OVERSCAN,
            left + window_width + OVERSCAN,
        )
    }
}

/// Column widths from their names and first rows.
fn widths(data: &TableData) -> Vec<f32> {
    (0..data.columns.len())
        .map(|column| {
            let longest = std::iter::once(&data.columns[column])
                .chain(data.rows.iter().take(200).filter_map(|row| row.get(column)))
                .map(|text| text.chars().count().min(60))
                .max()
                .unwrap_or(0);
            (longest as f32 * 7.4 + 28.).clamp(MIN_WIDTH, MAX_WIDTH)
        })
        .collect()
}

impl Render for TableGrid {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let total: f32 = NUMBER_WIDTH + self.starts.last().copied().unwrap_or(0.);
        // Only the columns in view are built; the ones before them are a
        // space of their width.
        let in_view = self.columns_in_view(window);
        let before = self.starts[in_view.start];
        let shown = self.order.len();
        let all = self.data.rows.len();
        let count = if shown == all {
            format!("{all} rows")
        } else {
            format!("{shown} of {all} rows")
        };
        let header = h_flex()
            .h(ph_ui::rems_from_px(ROW_HEIGHT + 4.))
            .w(ph_ui::rems_from_px(total))
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.secondary)
            .text_xs()
            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
            .child(div().w(ph_ui::rems_from_px(NUMBER_WIDTH)).flex_none())
            .child(div().w(ph_ui::rems_from_px(before)).flex_none())
            .children(in_view.clone().map(|ix| {
                let name = &self.data.columns[ix];
                let arrow = match self.sort {
                    Some((column, true)) if column == ix => Some(IconName::ChevronUp),
                    Some((column, false)) if column == ix => Some(IconName::ChevronDown),
                    _ => None,
                };
                h_flex()
                    .id(("table-column", ix))
                    .flex_none()
                    .w(ph_ui::rems_from_px(self.widths[ix]))
                    .h_full()
                    .px_2()
                    .gap_1()
                    .border_l_1()
                    .border_color(theme.border.opacity(0.6))
                    .cursor_pointer()
                    .hover(|this| this.bg(theme.list_hover))
                    .child(div().min_w_0().truncate().child(name.clone()))
                    .children(arrow.map(|arrow| Icon::new(arrow).xsmall()))
                    .on_click(cx.listener(move |this, _, _, cx| this.sort_by(ix, cx)))
            }));
        let data = self.data.clone();
        let order = self.order.clone();
        let widths: Vec<(usize, f32)> = in_view.map(|ix| (ix, self.widths[ix])).collect();
        let rows = uniform_list("table-rows", shown, move |range: Range<usize>, _, cx| {
            let theme = cx.theme();
            range
                .map(|position| {
                    let ix = order[position];
                    let row = &data.rows[ix];
                    h_flex()
                        .h(ph_ui::rems_from_px(ROW_HEIGHT))
                        .w(ph_ui::rems_from_px(total))
                        .text_sm()
                        .border_b_1()
                        .border_color(theme.border.opacity(0.35))
                        .when(position % 2 == 1, |this| {
                            this.bg(theme.secondary.opacity(0.35))
                        })
                        .child(
                            div()
                                .w(ph_ui::rems_from_px(NUMBER_WIDTH))
                                .flex_none()
                                .px_2()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child((ix + 1).to_string()),
                        )
                        .child(div().w(ph_ui::rems_from_px(before)).flex_none())
                        .children(widths.iter().map(|(column, width)| {
                            div()
                                .flex_none()
                                .w(ph_ui::rems_from_px(*width))
                                .px_2()
                                .truncate()
                                .border_l_1()
                                .border_color(theme.border.opacity(0.35))
                                .children(row.get(*column).map(one_line))
                        }))
                        .into_any_element()
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(&self.rows_scroll)
        .w(ph_ui::rems_from_px(total))
        .h_full();
        v_flex()
            .size_full()
            .bg(theme.background)
            .child(
                h_flex()
                    .flex_none()
                    .px_2()
                    .py_1p5()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .w(ph_ui::rems_from_px(240.))
                            .child(Input::new(&self.filter).small()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(count),
                    )
                    .when_some(self.note.clone(), |this, note| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(note),
                        )
                    }),
            )
            .child(
                div()
                    .id("table-across")
                    .flex_1()
                    .min_h_0()
                    .overflow_x_scroll()
                    .track_scroll(&self.across)
                    .child(
                        v_flex()
                            .h_full()
                            .w(ph_ui::rems_from_px(total))
                            .child(header)
                            .child(div().flex_1().min_h_0().child(rows)),
                    ),
            )
    }
}

/// A CSV or TSV file as a table, its first row the column names.
pub struct TablePreview {
    tabs: bool,
    grid: Entity<TableGrid>,
    error: Option<SharedString>,
    _parse: Option<Task<()>>,
}

impl TablePreview {
    pub fn new(tabs: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            tabs,
            grid: cx.new(|cx| TableGrid::new(window, cx)),
            error: None,
            _parse: None,
        }
    }

    pub fn set_text(&mut self, text: Arc<str>, cx: &mut Context<Self>) {
        let tabs = self.tabs;
        let parse = cx.background_spawn(async move { parse_delimited(&text, tabs) });
        self._parse = Some(cx.spawn(async move |this, cx| {
            let parsed = parse.await;
            _ = this.update(cx, |this, cx| {
                match parsed {
                    Ok(data) => {
                        this.error = None;
                        this.grid
                            .update(cx, |grid, cx| grid.set_data(data, None, cx));
                    }
                    Err(err) => this.error = Some(err.into()),
                }
                cx.notify();
            });
        }));
    }
}

/// Reads CSV (or TSV); rows may have fewer or more cells than the header.
pub fn parse_delimited(text: &str, tabs: bool) -> Result<TableData, String> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(if tabs { b'\t' } else { b',' })
        .from_reader(text.as_bytes());
    let mut records = reader.records();
    let columns: Vec<SharedString> = match records.next() {
        Some(header) => header
            .map_err(|err| err.to_string())?
            .iter()
            .map(|cell| SharedString::from(cell.to_string()))
            .collect(),
        None => return Ok(TableData::default()),
    };
    let mut rows = Vec::new();
    let mut widest = columns.len();
    for record in records {
        let record = record.map_err(|err| err.to_string())?;
        widest = widest.max(record.len());
        rows.push(
            record
                .iter()
                .map(|cell| SharedString::from(cell.to_string()))
                .collect(),
        );
    }
    let mut columns = columns;
    // Rows wider than the header get numbered columns.
    while columns.len() < widest {
        columns.push(format!("Column {}", columns.len() + 1).into());
    }
    Ok(TableData { columns, rows })
}

impl Render for TablePreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match &self.error {
            Some(error) => crate::notice(format!("Not a valid table: {error}"), cx),
            None => self.grid.clone().into_any_element(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_csv_and_tsv() {
        let data = parse_delimited("name,age\n\"Doe, J\",42\nAl,7,extra\n", false).unwrap();
        assert_eq!(data.columns, ["name", "age", "Column 3"]);
        assert_eq!(data.rows[0], ["Doe, J", "42"]);
        assert_eq!(data.rows.len(), 2);
        let tsv = parse_delimited("a\tb\n1\t2\n", true).unwrap();
        assert_eq!(tsv.rows[0], ["1", "2"]);
        assert_eq!(parse_delimited("", false).unwrap(), TableData::default());
    }

    #[test]
    fn numbers_sort_as_numbers() {
        assert_eq!(number("1,200.5"), Some(1200.5));
        assert_eq!(number("x"), None);
        let data = parse_delimited("n,name\n10,b\n9,A\n,c\n100,a\n", false).unwrap();
        assert_eq!(order_rows(&data, "", Some((0, true))), [2, 1, 0, 3]);
        assert_eq!(order_rows(&data, "", Some((0, false))), [3, 0, 1, 2]);
        assert_eq!(order_rows(&data, "", Some((1, true))), [1, 3, 0, 2]);
        assert_eq!(order_rows(&data, "a", None), [1, 3]);
    }

    #[test]
    fn only_the_columns_in_view() {
        let starts = starts(&[100., 50., 200., 100.]);
        assert_eq!(starts, [0., 100., 150., 350., 450.]);
        assert_eq!(columns_between(&starts, 0., 120.), 0..2);
        // A column's edge at the edge of the view is not in it.
        assert_eq!(columns_between(&starts, 100., 150.), 1..2);
        assert_eq!(columns_between(&starts, 149., 151.), 1..3);
        assert_eq!(columns_between(&starts, -500., 5_000.), 0..4);
        // Out of the table, and a table with no columns: none, in range.
        assert_eq!(columns_between(&starts, 900., 1_000.), 4..4);
        assert_eq!(columns_between(&starts, -90., -10.), 0..0);
        assert_eq!(columns_between(&[0.], -400., 2_000.), 0..0);
    }

    #[test]
    fn a_column_with_nan_sorts() {
        // Empty first, then the numbers, then `NaN`, whatever the rows' order.
        let mut text = String::from("loss\n");
        for ix in 0..60 {
            match ix % 3 {
                0 => text.push_str("nan\n"),
                1 => text.push_str("\"\"\n"),
                _ => text.push_str(&format!("{}\n", 60 - ix)),
            }
        }
        let data = parse_delimited(&text, false).unwrap();
        let order = order_rows(&data, "", Some((0, true)));
        let cells: Vec<&str> = order.iter().map(|ix| data.rows[*ix][0].as_ref()).collect();
        assert!(cells[..20].iter().all(|cell| cell.is_empty()), "{cells:?}");
        let numbers: Vec<f64> = cells[20..40].iter().map(|c| c.parse().unwrap()).collect();
        assert!(numbers.is_sorted(), "{numbers:?}");
        assert!(cells[40..].iter().all(|cell| *cell == "nan"), "{cells:?}");
    }
}

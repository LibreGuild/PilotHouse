//! Rows as a table: the grid CSV and SQLite results are shown in (columns
//! sized to what they hold, resizable, sorted by a click on a header,
//! only the rows in view drawn), and the CSV view.

use gpui_kit::component::table::{Column, ColumnSort, DataTable, TableDelegate, TableState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _,
    checkbox::Checkbox,
    h_flex,
    input::{Editor, EditorState, InputEvent},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Task, Window, div, px,
};
use ph_ui::Icons;

use super::parts::{self, Tone};
use super::{Ctx, ToolView};

/// A cell: its text, and whether it stands for no value (SQL's NULL).
#[derive(Clone)]
pub(crate) struct Cell {
    pub text: SharedString,
    pub null: bool,
}

/// What a grid shows.
#[derive(Default)]
pub(crate) struct Grid {
    /// Each column's name, and its type when it has one.
    pub columns: Vec<(SharedString, Option<SharedString>)>,
    pub rows: Vec<Vec<Cell>>,
    /// The order rows are shown in (sorted by a header, or as read).
    order: Vec<usize>,
}

impl Grid {
    pub fn new(columns: Vec<(SharedString, Option<SharedString>)>, rows: Vec<Vec<Cell>>) -> Self {
        let order = (0..rows.len()).collect();
        Self {
            columns,
            rows,
            order,
        }
    }

    /// A column as wide as its longest text (within reason).
    fn width(&self, column: usize) -> f32 {
        let longest = self
            .rows
            .iter()
            .take(500)
            .filter_map(|row| row.get(column))
            .map(|cell| cell.text.chars().count())
            .chain(self.columns.get(column).map(|(name, kind)| {
                // The header holds the name, the type and the sort mark.
                name.chars().count() + kind.as_ref().map_or(0, |kind| kind.chars().count() + 1) + 4
            }))
            .max()
            .unwrap_or(4);
        (longest as f32 * 7.5 + 24.).clamp(64., 360.)
    }
}

/// Text and numbers compared as a person expects: numbers by value.
fn compare(a: &str, b: &str) -> std::cmp::Ordering {
    match (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
        (Ok(a), Ok(b)) => a.total_cmp(&b),
        _ => a.to_lowercase().cmp(&b.to_lowercase()),
    }
}

impl TableDelegate for Grid {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        let (name, _) = &self.columns[col_ix];
        Column::new(SharedString::from(format!("c{col_ix}")), name.clone())
            .width(px(self.width(col_ix)))
            .min_width(px(48.))
            .max_width(px(900.))
            .resizable(true)
            .sortable()
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
        let rows = &self.rows;
        let text = |row: usize| {
            rows[row]
                .get(col_ix)
                .map(|cell| cell.text.as_ref())
                .unwrap_or("")
        };
        let mut order: Vec<usize> = (0..rows.len()).collect();
        match sort {
            ColumnSort::Ascending => order.sort_by(|a, b| compare(text(*a), text(*b))),
            ColumnSort::Descending => order.sort_by(|a, b| compare(text(*b), text(*a))),
            ColumnSort::Default => {}
        }
        self.order = order;
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let (name, kind) = self.columns[col_ix].clone();
        // The name before the type: when the column is narrow, the type
        // gives way.
        h_flex()
            .size_full()
            .gap_1()
            .child(div().flex_none().max_w_full().truncate().child(name))
            .when_some(kind, |this, kind| {
                this.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(kind),
                )
            })
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let row = self.order.get(row_ix).copied().unwrap_or(row_ix);
        let cell = self.rows.get(row).and_then(|row| row.get(col_ix)).cloned();
        let theme = cx.theme();
        div().truncate().map(|this| match cell {
            Some(cell) if cell.null => this
                .text_color(theme.muted_foreground)
                .italic()
                .child(cell.text),
            Some(cell) => this.child(cell.text),
            None => this,
        })
    }

    fn cell_label(&self, row_ix: usize, col_ix: usize, _: &App) -> Option<SharedString> {
        let row = self.order.get(row_ix).copied().unwrap_or(row_ix);
        self.rows
            .get(row)
            .and_then(|row| row.get(col_ix))
            .map(|cell| cell.text.clone())
    }
}

/// A grid's state, made with nothing in it.
pub(crate) fn grid(window: &mut Window, cx: &mut App) -> Entity<TableState<Grid>> {
    cx.new(|cx| {
        TableState::new(Grid::default(), window, cx)
            .col_movable(false)
            .row_selectable(false)
    })
}

/// Shows `grid` in `state`.
pub(crate) fn show(state: &Entity<TableState<Grid>>, grid: Grid, cx: &mut App) {
    state.update(cx, |state, cx| {
        *state.delegate_mut() = grid;
        state.refresh(cx);
        cx.notify();
    });
}

/// The grid as drawn.
pub(crate) fn render(state: &Entity<TableState<Grid>>) -> impl IntoElement {
    DataTable::new(state).stripe(true).bordered(true).small()
}

pub(crate) struct Csv {
    ctx: Ctx,
    as_json: bool,
    header: bool,
    input: Entity<EditorState>,
    json: Entity<EditorState>,
    grid: Entity<TableState<Grid>>,
    read: Option<Result<(usize, usize, char), String>>,
    _input: Subscription,
    _run: Option<Task<()>>,
}

impl Csv {
    fn run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().to_string();
        if text.trim().is_empty() {
            self.read = None;
            show(&self.grid, Grid::default(), cx);
            cx.notify();
            return;
        }
        let header = self.header;
        self._run = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(120))
                .await;
            let read = cx
                .background_spawn(async move {
                    let table =
                        crate::data::table(&text, header).map_err(|err| format!("{err:#}"))?;
                    let json = serde_json::to_string_pretty(&crate::data::objects(
                        &table.header,
                        &table.rows,
                    ))
                    .unwrap_or_default();
                    Ok::<_, String>((table, json))
                })
                .await;
            _ = this.update_in(cx, |this, window, cx| match read {
                Ok((table, json)) => {
                    this.read = Some(Ok((table.rows.len(), table.columns(), table.separator)));
                    let columns = (0..table.columns())
                        .map(|ix| {
                            let name = table
                                .header
                                .get(ix)
                                .cloned()
                                .unwrap_or_else(|| format!("Column {}", ix + 1));
                            (SharedString::from(name), None)
                        })
                        .collect();
                    let rows = table
                        .rows
                        .into_iter()
                        .map(|row| {
                            row.into_iter()
                                .map(|text| Cell {
                                    text: text.into(),
                                    null: false,
                                })
                                .collect()
                        })
                        .collect();
                    show(&this.grid, Grid::new(columns, rows), cx);
                    this.json
                        .update(cx, |output, cx| output.set_value(json, window, cx));
                    cx.notify();
                }
                Err(err) => {
                    this.read = Some(Err(err));
                    cx.notify();
                }
            });
        }));
    }
}

impl ToolView for Csv {
    fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = parts::editor(
            "csv",
            "Paste CSV or TSV (a spreadsheet's cells paste as TSV)",
            window,
            cx,
        );
        let json = parts::editor("json", "", window, cx);
        let subscription =
            cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.run(window, cx);
                }
            });
        let grid = grid(window, cx);
        Self {
            ctx,
            as_json: false,
            header: true,
            input,
            json,
            grid,
            read: None,
            _input: subscription,
            _run: None,
        }
    }

    fn take(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, cx| input.set_value(text, window, cx));
        self.run(window, cx);
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }
}

impl Render for Csv {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let text = self.input.read(cx).value().to_string();
        let json = self.json.read(cx).value().to_string();
        let status: AnyElement = match &self.read {
            None => parts::label("Result", cx).into_any_element(),
            Some(Err(_)) => parts::badge("Not read", Tone::Bad, cx).into_any_element(),
            Some(Ok((rows, columns, separator))) => h_flex()
                .gap_2()
                .child(parts::badge(
                    format!("{rows} rows · {columns} columns"),
                    Tone::Good,
                    cx,
                ))
                .child(parts::label(
                    format!(
                        "separated by {}",
                        match separator {
                            '\t' => "tabs".to_string(),
                            ',' => "commas".to_string(),
                            ';' => "semicolons".to_string(),
                            other => format!("“{other}”"),
                        }
                    ),
                    cx,
                ))
                .into_any_element(),
        };
        let result: AnyElement = match (&self.read, self.as_json) {
            (None, _) => parts::surface(cx)
                .size_full()
                .child(parts::hint("The rows show here as a table.", cx))
                .into_any_element(),
            (Some(Err(err)), _) => v_flex()
                .gap_2()
                .child(parts::failure(err.clone(), cx))
                .children(parts::suggestion(&text, &self.ctx, cx))
                .into_any_element(),
            (Some(Ok(_)), false) => render(&self.grid).into_any_element(),
            (Some(Ok(_)), true) => Editor::new(&self.json)
                .h_full()
                .small()
                .readonly(true)
                .into_any_element(),
        };
        v_flex()
            .size_full()
            .gap_2()
            .child(parts::header(
                &self.ctx,
                h_flex()
                    .gap_3()
                    .child(
                        Checkbox::new("csv-header")
                            .small()
                            .label("First row names the columns")
                            .checked(self.header)
                            .on_click(cx.listener(|this, on: &bool, window, cx| {
                                this.header = *on;
                                this.run(window, cx);
                            })),
                    )
                    .child(parts::modes(
                        "utility-modes",
                        &["Table", "To JSON"],
                        usize::from(self.as_json),
                        cx.listener(|this, ix: &usize, _, cx| {
                            this.as_json = *ix == 1;
                            cx.notify();
                        }),
                    )),
            ))
            .child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .child(parts::label("CSV", cx).flex_1())
                    .child(
                        parts::action("utility-paste", Icons::ClipboardPaste, "Paste").on_click(
                            cx.listener(|this, _, window, cx| {
                                if let Some(text) = parts::clipboard_text(cx) {
                                    this.take(text, window, cx);
                                }
                            }),
                        ),
                    )
                    .child(
                        parts::action("utility-clear", Icons::Eraser, "Clear")
                            .disabled(text.is_empty())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.take(String::new(), window, cx);
                                this.focus(window, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .h(gpui_kit::relative(0.3))
                    .child(Editor::new(&self.input).h_full().small()),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .child(div().flex_1().child(status))
                    .when(self.as_json, |this| {
                        this.child(
                            ph_ui::CopyButton::new("csv-copy-json", json.clone())
                                .label("Copy")
                                .disabled(json.is_empty()),
                        )
                    }),
            )
            .child(div().flex_1().min_h_0().child(result))
    }
}

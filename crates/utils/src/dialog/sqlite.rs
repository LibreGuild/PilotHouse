//! A SQLite file: its tables down the side with their row counts, a query
//! (a click on a table writes one), and the rows in a grid with each
//! column's type. The file is opened read-only, so nothing in it changes.
//! A file is chosen, typed, or dropped on the view.

use std::path::PathBuf;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Editor, EditorState, Input as Field, InputEvent, InputState},
    table::TableState,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, ExternalPaths, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Task, Window, div,
};
use ph_ui::Icons;

use super::parts::{self, Tone};
use super::table::{self, Cell, Grid};
use super::{Ctx, ToolView};
use crate::sqlite::{TableInfo, file_of, rows_at, tables_at};

pub(crate) struct Sqlite {
    ctx: Ctx,
    path: Entity<InputState>,
    query: Entity<EditorState>,
    grid: Entity<TableState<Grid>>,
    file: Option<PathBuf>,
    tables: Result<Vec<TableInfo>, String>,
    picked: Option<String>,
    /// How the last query went: rows shown and whether there were more.
    ran: Option<Result<(usize, bool), String>>,
    running: bool,
    _inputs: Vec<Subscription>,
    _load: Option<Task<()>>,
    _run: Option<Task<()>>,
}

impl Sqlite {
    /// Opens the file the path names and lists its tables.
    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.path.read(cx).value().to_string();
        if text.trim().is_empty() {
            self.file = None;
            self.tables = Ok(Vec::new());
            cx.notify();
            return;
        }
        self._load = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(200))
                .await;
            let loaded = cx
                .background_spawn(async move {
                    let file = file_of(&text).map_err(|err| format!("{err:#}"))?;
                    let tables = tables_at(&file).map_err(|err| format!("{err:#}"))?;
                    Ok::<_, String>((file, tables))
                })
                .await;
            _ = this.update_in(cx, |this, window, cx| {
                match loaded {
                    Ok((file, tables)) => {
                        let first = tables
                            .iter()
                            .find(|table| !table.view)
                            .map(|table| table.name.clone());
                        this.file = Some(file);
                        this.tables = Ok(tables);
                        // A file just opened shows its first table.
                        if let Some(first) = first.filter(|_| this.picked.is_none()) {
                            this.pick(first, window, cx);
                        }
                    }
                    Err(err) => {
                        this.file = None;
                        this.tables = Err(err);
                    }
                }
                cx.notify();
            });
        }));
    }

    fn pick(&mut self, table: String, window: &mut Window, cx: &mut Context<Self>) {
        let sql = format!("SELECT * FROM \"{}\" LIMIT 200", table.replace('"', "\"\""));
        self.picked = Some(table);
        self.query
            .update(cx, |query, cx| query.set_value(sql, window, cx));
        self.run(window, cx);
    }

    fn run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(file) = self.file.clone() else {
            return;
        };
        let sql = self.query.read(cx).value().to_string();
        if sql.trim().is_empty() {
            return;
        }
        self.running = true;
        cx.notify();
        self._run = Some(cx.spawn_in(window, async move |this, cx| {
            let rows = cx
                .background_spawn(
                    async move { rows_at(&file, &sql).map_err(|err| format!("{err:#}")) },
                )
                .await;
            _ = this.update(cx, |this, cx| {
                this.running = false;
                match rows {
                    Ok(rows) => {
                        this.ran = Some(Ok((rows.rows.len(), rows.more)));
                        let columns = rows
                            .columns
                            .into_iter()
                            .map(|(name, kind)| {
                                (SharedString::from(name), kind.map(SharedString::from))
                            })
                            .collect();
                        let cells = rows
                            .rows
                            .into_iter()
                            .map(|row| {
                                row.into_iter()
                                    .map(|cell| Cell {
                                        null: matches!(cell, crate::sqlite::Cell::Null),
                                        text: cell.to_string().into(),
                                    })
                                    .collect()
                            })
                            .collect();
                        table::show(&this.grid, Grid::new(columns, cells), cx);
                    }
                    Err(err) => this.ran = Some(Err(err)),
                }
                cx.notify();
            });
        }));
    }

    fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            if let Some(path) = paths.into_iter().next() {
                _ = this.update_in(cx, |this, window, cx| {
                    this.open(path.display().to_string(), window, cx)
                });
            }
        })
        .detach();
    }

    fn open(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.picked = None;
        self.ran = None;
        table::show(&self.grid, Grid::default(), cx);
        self.path
            .update(cx, |input, cx| input.set_value(path, window, cx));
        self.load(window, cx);
    }

    fn render_tables(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let tables = match &self.tables {
            Ok(tables) => tables,
            Err(_) => return div().into_any_element(),
        };
        let mut items: Vec<AnyElement> = Vec::new();
        for (views, title) in [(false, "Tables"), (true, "Views")] {
            let listed: Vec<&TableInfo> =
                tables.iter().filter(|table| table.view == views).collect();
            if listed.is_empty() {
                continue;
            }
            items.push(parts::label(title, cx).px_2().pt_1().into_any_element());
            for table in listed {
                let name = table.name.clone();
                let picked = self.picked.as_deref() == Some(table.name.as_str());
                let columns = table
                    .columns
                    .iter()
                    .map(|(name, kind)| format!("{name} {kind}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                items.push(
                    h_flex()
                        .id(SharedString::from(format!("sqlite-table-{}", table.name)))
                        .gap_2()
                        .px_2()
                        .py_1()
                        .rounded(theme.radius)
                        .text_sm()
                        .cursor_pointer()
                        .when(picked, |this| this.bg(theme.list_active))
                        .when(!picked, |this| {
                            this.hover(|style| style.bg(theme.list_hover))
                        })
                        .tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(columns.clone())
                                .build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.pick(name.clone(), window, cx)
                        }))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(table.name.clone()),
                        )
                        // Findable by the automation socket and in UI tests.
                        .test_support()
                        .children(table.rows.map(|rows| {
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(rows.to_string())
                        }))
                        .into_any_element(),
                );
            }
        }
        if items.is_empty() {
            items.push(parts::hint("No tables.", cx).into_any_element());
        }
        v_flex()
            .id("sqlite-tables")
            .w(ph_ui::rems_from_px(176.))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .pr_2()
            .border_r_1()
            .border_color(theme.border)
            .children(items)
            .into_any_element()
    }
}

impl ToolView for Sqlite {
    fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let path = parts::line(
            "The path of a .db, .sqlite or .sqlite3 file, or drop one here",
            window,
            cx,
        );
        let query = parts::editor("sql", "SELECT … (read only)", window, cx);
        let inputs = vec![
            cx.subscribe_in(&path, window, |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.picked = None;
                    this.load(window, cx);
                }
            }),
            cx.subscribe_in(&query, window, |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter {
                    secondary: true, ..
                } = event
                {
                    this.run(window, cx);
                }
            }),
        ];
        let grid = table::grid(window, cx);
        Self {
            ctx,
            path,
            query,
            grid,
            file: None,
            tables: Ok(Vec::new()),
            picked: None,
            ran: None,
            running: false,
            _inputs: inputs,
            _load: None,
            _run: None,
        }
    }

    fn take(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.open(text.trim().to_string(), window, cx);
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        self.path.update(cx, |path, cx| path.focus(window, cx));
    }
}

impl Render for Sqlite {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let opened = self.file.is_some();
        let status: AnyElement = match (&self.ran, self.running) {
            (_, true) => parts::label("Running…", cx).into_any_element(),
            (None, _) => parts::label("Rows", cx).into_any_element(),
            (Some(Ok((rows, more))), _) => parts::badge(
                if *more {
                    format!("The first {rows} rows")
                } else {
                    format!("{rows} rows")
                },
                Tone::Good,
                cx,
            )
            .into_any_element(),
            (Some(Err(_)), _) => {
                parts::badge("The query did not run", Tone::Bad, cx).into_any_element()
            }
        };
        let result: AnyElement = match &self.ran {
            Some(Err(err)) => parts::failure(err.clone(), cx).into_any_element(),
            Some(Ok(_)) => table::render(&self.grid).into_any_element(),
            None => parts::surface(cx)
                .size_full()
                .child(parts::hint(
                    "Pick a table, or write a query and press Run (⌘↵).",
                    cx,
                ))
                .into_any_element(),
        };
        let body: AnyElement = match (&self.tables, opened) {
            (Err(err), _) => parts::failure(err.clone(), cx).into_any_element(),
            (Ok(_), false) => parts::surface(cx)
                .size_full()
                .child(parts::hint(
                    "Choose a SQLite file, or drop one here. It is opened read-only: nothing in it can change.",
                    cx,
                ))
                .into_any_element(),
            (Ok(_), true) => h_flex()
                .size_full()
                .items_start()
                .gap_3()
                .child(self.render_tables(cx))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .gap_2()
                        .child(
                            div()
                                .flex_none()
                                .h(ph_ui::rems_from_px(88.))
                                .child(Editor::new(&self.query).h_full().small()),
                        )
                        .child(
                            h_flex()
                                .flex_none()
                                .gap_2()
                                .child(div().flex_1().child(status))
                                .child(
                                    Button::new("sqlite-run")
                                        .primary()
                                        .xsmall()
                                        .icon(gpui_kit::component::IconName::Play)
                                        .label("Run")
                                        .on_click(cx.listener(|this, _, window, cx| this.run(window, cx))),
                                ),
                        )
                        .child(div().flex_1().min_h_0().child(result)),
                )
                .into_any_element(),
        };
        v_flex()
            .id("sqlite")
            .size_full()
            .gap_2()
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                if let Some(path) = paths.paths().first() {
                    this.open(path.display().to_string(), window, cx);
                }
            }))
            .child(parts::header(&self.ctx, div()))
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .child(div().flex_1().child(Field::new(&self.path).small()))
                    .child(
                        parts::action("sqlite-choose", Icons::FolderOpen, "Choose…")
                            .on_click(cx.listener(|this, _, window, cx| this.choose(window, cx))),
                    ),
            )
            .child(div().flex_1().min_h_0().child(body))
    }
}

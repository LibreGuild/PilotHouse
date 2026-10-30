//! ⌘T: Go to Symbol in the workspace. Functions, classes and types by name,
//! from the folders' code indexes ([`CodeIndex`]), searched off the UI
//! thread as the name is typed.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::{
    ActiveTheme as _, IndexPath, WindowExt as _, h_flex,
    list::{List, ListDelegate, ListItem, ListState},
};
use gpui_kit::{App, AppContext as _, ParentElement as _, Styled as _, Task, Window, div};

use crate::code_index::CodeIndex;
use crate::symbols::Symbol;

/// Results shown at most.
const MAX_RESULTS: usize = 200;

/// Opens a symbol: its folder's index, the symbol (path within that folder).
type OnOpen = Rc<dyn Fn(usize, &Symbol, &mut Window, &mut App)>;

pub struct SymbolSearch {
    /// Each folder's name (shown when there are several) and index.
    indexes: Vec<(String, Arc<CodeIndex>)>,
    results: Vec<(usize, Symbol)>,
    selected: Option<IndexPath>,
    on_open: OnOpen,
}

impl ListDelegate for SymbolSearch {
    type Item = ListItem;

    fn perform_search(
        &mut self,
        query: &str,
        window: &mut Window,
        cx: &mut gpui_kit::Context<ListState<Self>>,
    ) -> Task<()> {
        let query = query.trim().to_string();
        let indexes: Vec<Arc<CodeIndex>> = self
            .indexes
            .iter()
            .map(|(_, index)| index.clone())
            .collect();
        // The first search may build an index: never on the UI thread.
        let found = cx.background_spawn(async move {
            if query.is_empty() {
                return Vec::new();
            }
            let mut found: Vec<(usize, Symbol)> = indexes
                .iter()
                .enumerate()
                .flat_map(|(ix, index)| {
                    index
                        .search(&query, MAX_RESULTS)
                        .into_iter()
                        .map(move |symbol| (ix, symbol))
                })
                .collect();
            let lower = query.to_lowercase();
            found.sort_by_key(|(_, symbol)| {
                let name = symbol.name.to_lowercase();
                (!name.starts_with(&lower), name.len())
            });
            found.truncate(MAX_RESULTS);
            found
        });
        cx.spawn_in(window, async move |list, cx| {
            let results = found.await;
            _ = list.update_in(cx, |list, window, cx| {
                let first = (!results.is_empty()).then(|| IndexPath::new(0));
                list.delegate_mut().results = results;
                // Enter opens the best match right away.
                list.set_selected_index(first, window, cx);
                cx.notify();
            });
        })
    }

    fn items_count(&self, _: usize, _: &App) -> usize {
        self.results.len()
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        _: &mut Window,
        cx: &mut gpui_kit::Context<ListState<Self>>,
    ) -> Option<ListItem> {
        let (folder, symbol) = self.results.get(ix.row)?;
        let place = if self.indexes.len() > 1 {
            format!(
                "{}/{}:{}",
                self.indexes[*folder].0, symbol.path, symbol.line
            )
        } else {
            format!("{}:{}", symbol.path, symbol.line)
        };
        Some(
            ListItem::new(ix).selected(self.selected == Some(ix)).child(
                h_flex()
                    .gap_2()
                    .min_w_0()
                    .child(
                        div()
                            .flex_none()
                            .w_12()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(symbol.kind),
                    )
                    .child(div().flex_none().child(symbol.name.clone()))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(place),
                    ),
            ),
        )
    }

    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        _: &mut Window,
        cx: &mut gpui_kit::Context<ListState<Self>>,
    ) {
        self.selected = ix;
        cx.notify();
    }

    fn confirm(
        &mut self,
        _: bool,
        window: &mut Window,
        cx: &mut gpui_kit::Context<ListState<Self>>,
    ) {
        let Some((folder, symbol)) = self
            .selected
            .and_then(|ix| self.results.get(ix.row))
            .cloned()
        else {
            return;
        };
        window.close_dialog(cx);
        (self.on_open)(folder, &symbol, window, cx);
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut gpui_kit::Context<ListState<Self>>) {
        window.close_dialog(cx);
    }
}

/// Opens the picker over `indexes` (each folder's name and index).
pub fn open(
    indexes: Vec<(String, Arc<CodeIndex>)>,
    on_open: impl Fn(usize, &Symbol, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    // Read ahead while the name is typed.
    for (_, index) in &indexes {
        let index = index.clone();
        cx.background_spawn(async move { index.warm() }).detach();
    }
    let delegate = SymbolSearch {
        indexes,
        results: Vec::new(),
        selected: None,
        on_open: Rc::new(on_open),
    };
    let state = cx.new(|cx| ListState::new(delegate, window, cx).searchable(true));
    let focus = state.clone();
    window.defer(cx, move |window, cx| {
        focus.update(cx, |state, cx| state.focus(window, cx))
    });
    window.open_dialog(cx, move |dialog, _, _| {
        dialog.close_button(false).p_0().child(
            List::new(&state)
                .search_placeholder("Go to symbol (function, class, type)…")
                .h_96(),
        )
    });
}

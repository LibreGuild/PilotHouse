//! ⌘P: fuzzy-find a file in the project and open it.

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, IndexPath, WindowExt as _, h_flex,
    list::{List, ListDelegate, ListItem, ListState},
};
use gpui_kit::{App, AppContext as _, ParentElement as _, Styled as _, Task, Window, div};

use crate::file_index::FileList;
use crate::fuzzy::fuzzy_match;

/// Results beyond this are dropped; nobody scrolls past the first screens.
const MAX_RESULTS: usize = 200;

type OnOpen = Rc<dyn Fn(&str, &mut Window, &mut App)>;

pub struct QuickOpenDelegate {
    files: FileList,
    /// Indices into `files`, best match first.
    results: Vec<usize>,
    selected: Option<IndexPath>,
    on_open: OnOpen,
}

impl QuickOpenDelegate {
    fn new(files: FileList, on_open: OnOpen) -> Self {
        let results = (0..files.len().min(MAX_RESULTS)).collect();
        Self {
            files,
            results,
            selected: None,
            on_open,
        }
    }
}

/// Ranks every file against `query`, best first.
fn rank(files: &FileList, query: &str) -> Vec<usize> {
    if query.is_empty() {
        return (0..files.len().min(MAX_RESULTS)).collect();
    }
    let mut scored: Vec<(i32, usize)> = files
        .iter()
        .enumerate()
        .filter_map(|(ix, path)| fuzzy_match(query, path).map(|found| (found.score, ix)))
        .collect();
    scored.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.truncate(MAX_RESULTS);
    scored.into_iter().map(|(_, ix)| ix).collect()
}

impl ListDelegate for QuickOpenDelegate {
    type Item = ListItem;

    fn perform_search(
        &mut self,
        query: &str,
        window: &mut Window,
        cx: &mut gpui_kit::Context<ListState<Self>>,
    ) -> Task<()> {
        let files = self.files.clone();
        let query = query.to_string();
        // Scoring 100k paths takes a few milliseconds: keep it off the UI thread.
        let ranked = cx.background_spawn(async move { rank(&files, &query) });
        cx.spawn_in(window, async move |list, cx| {
            let results = ranked.await;
            _ = list.update(cx, |list, cx| {
                list.delegate_mut().results = results;
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
        let path = self.files.get(*self.results.get(ix.row)?)?;
        let (dir, name) = match path.rsplit_once('/') {
            Some((dir, name)) => (dir, name),
            None => ("", path.as_ref()),
        };
        Some(
            ListItem::new(ix).selected(self.selected == Some(ix)).child(
                h_flex()
                    .gap_2()
                    .min_w_0()
                    .child(ph_ui::file_icon_element(path.as_ref(), gpui_kit::px(16.)))
                    .child(div().flex_none().child(name.to_string()))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(dir.to_string()),
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
        let Some(path) = self
            .selected
            .and_then(|ix| self.results.get(ix.row))
            .and_then(|ix| self.files.get(*ix))
            .cloned()
        else {
            return;
        };
        window.close_dialog(cx);
        (self.on_open)(&path, window, cx);
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut gpui_kit::Context<ListState<Self>>) {
        window.close_dialog(cx);
    }
}

/// Opens the finder over `files`; `on_open` receives the chosen relative path.
pub fn open(
    files: FileList,
    on_open: impl Fn(&str, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let on_open: OnOpen = Rc::new(on_open);
    let state = cx.new(|cx| {
        ListState::new(QuickOpenDelegate::new(files, on_open), window, cx).searchable(true)
    });
    let focus = state.clone();
    window.defer(cx, move |window, cx| {
        focus.update(cx, |state, cx| state.focus(window, cx))
    });
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .close_button(false)
            .p_0()
            .child(List::new(&state).search_placeholder("Go to file…").h_96())
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn ranks_best_match_first_and_caps_results() {
        let files: FileList = Arc::new(
            ["src/lib.rs", "src/files_panel.rs", "README.md"]
                .into_iter()
                .map(Arc::from)
                .collect(),
        );
        let ranked = rank(&files, "panel");
        assert_eq!(files[ranked[0]].as_ref(), "src/files_panel.rs");
        assert_eq!(ranked.len(), 1);
        assert_eq!(rank(&files, "").len(), 3);
    }
}

//! The editor's LSP hooks, answered by the project's [`LspManager`].

use std::path::PathBuf;

use anyhow::Result;
use gpui_kit::component::input::{
    CompletionProvider, DefinitionProvider, HoverProvider, Rope, RopeExt as _,
};
use gpui_kit::{App, Task, WeakEntity, Window};
use lsp_types::request::{Completion, GotoDefinition, HoverRequest};
use lsp_types::{
    CompletionContext, CompletionParams, CompletionResponse, GotoDefinitionParams,
    GotoDefinitionResponse, Hover, HoverParams, LocationLink, TextDocumentIdentifier,
    TextDocumentPositionParams,
};

use crate::manager::LspManager;

/// One open file's link to its language server.
#[derive(Clone)]
pub struct Binding {
    pub manager: WeakEntity<LspManager>,
    pub path: PathBuf,
}

impl Binding {
    fn position(&self, text: &Rope, offset: usize) -> Option<TextDocumentPositionParams> {
        Some(TextDocumentPositionParams {
            text_document: TextDocumentIdentifier {
                uri: crate::uri::from_path(&self.path)?,
            },
            position: crate::position::to_server(text, text.offset_to_position(offset)),
        })
    }
}

impl HoverProvider for Binding {
    fn hover(
        &self,
        text: &Rope,
        offset: usize,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Option<Hover>>> {
        let (Some(manager), Some(position)) = (self.manager.upgrade(), self.position(text, offset))
        else {
            return Task::ready(Ok(None));
        };
        let params = HoverParams {
            text_document_position_params: position,
            work_done_progress_params: Default::default(),
        };
        let request = manager.update(cx, |manager, cx| {
            manager.request::<HoverRequest>(&self.path, params, cx)
        });
        let text = text.clone();
        cx.background_spawn(async move {
            Ok(request.await?.map(|mut hover| {
                hover.range = hover
                    .range
                    .map(|range| crate::position::range_to_editor(&text, range));
                hover
            }))
        })
    }
}

impl DefinitionProvider for Binding {
    fn definitions(
        &self,
        text: &Rope,
        offset: usize,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<LocationLink>>> {
        let (Some(manager), Some(position)) = (self.manager.upgrade(), self.position(text, offset))
        else {
            return Task::ready(Ok(Vec::new()));
        };
        let params = GotoDefinitionParams {
            text_document_position_params: position,
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        let request = manager.update(cx, |manager, cx| {
            manager.request::<GotoDefinition>(&self.path, params, cx)
        });
        let (text, path) = (text.clone(), self.path.clone());
        cx.background_spawn(async move {
            let links = match request.await? {
                None => Vec::new(),
                Some(GotoDefinitionResponse::Link(links)) => links,
                Some(GotoDefinitionResponse::Scalar(location)) => vec![link(location)],
                Some(GotoDefinitionResponse::Array(locations)) => {
                    locations.into_iter().map(link).collect()
                }
            };
            Ok(links
                .into_iter()
                .map(|link| to_editor_link(link, &path, &text))
                .collect())
        })
    }
}

/// `link`'s ranges as the editor counts: in this file's `text`, or in the
/// target file as saved.
fn to_editor_link(mut link: LocationLink, path: &std::path::Path, text: &Rope) -> LocationLink {
    use crate::position::range_to_editor;
    link.origin_selection_range = link
        .origin_selection_range
        .map(|range| range_to_editor(text, range));
    let target = crate::uri::to_path(&link.target_uri);
    let saved;
    let target_text = match target {
        Some(target) if target != path => {
            let Ok(read) = std::fs::read_to_string(&target) else {
                return link;
            };
            saved = Rope::from(read.as_str());
            &saved
        }
        _ => text,
    };
    link.target_range = range_to_editor(target_text, link.target_range);
    link.target_selection_range = range_to_editor(target_text, link.target_selection_range);
    link
}

fn link(location: lsp_types::Location) -> LocationLink {
    LocationLink {
        origin_selection_range: None,
        target_uri: location.uri,
        target_range: location.range,
        target_selection_range: location.range,
    }
}

impl CompletionProvider for Binding {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        trigger: CompletionContext,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<CompletionResponse>> {
        let (Some(manager), Some(position)) = (self.manager.upgrade(), self.position(text, offset))
        else {
            return Task::ready(Ok(CompletionResponse::Array(Vec::new())));
        };
        let params = CompletionParams {
            text_document_position: position,
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: Some(trigger),
        };
        // The server must see the text being completed.
        let current = text.to_string();
        let request = manager.update(cx, |manager, cx| {
            manager.change(&self.path, &current, cx);
            manager.request::<Completion>(&self.path, params, cx)
        });
        let text = text.clone();
        cx.background_spawn(async move {
            let mut response = request
                .await?
                .unwrap_or(CompletionResponse::Array(Vec::new()));
            let items = match &mut response {
                CompletionResponse::Array(items) => items,
                CompletionResponse::List(list) => &mut list.items,
            };
            for item in items {
                completion_to_editor(item, &text);
            }
            Ok(response)
        })
    }

    fn is_completion_trigger(&self, _offset: usize, new_text: &str, _: &mut App) -> bool {
        new_text.chars().last().is_some_and(|c| {
            c.is_alphanumeric() || matches!(c, '_' | '.' | ':' | '>' | '/' | '@' | '<' | '"' | '\'')
        })
    }
}

use gpui_kit::AppContext as _;

/// A completion's edits as the editor counts columns.
fn completion_to_editor(item: &mut lsp_types::CompletionItem, text: &Rope) {
    use crate::position::range_to_editor;
    match &mut item.text_edit {
        Some(lsp_types::CompletionTextEdit::Edit(edit)) => {
            edit.range = range_to_editor(text, edit.range);
        }
        Some(lsp_types::CompletionTextEdit::InsertAndReplace(edit)) => {
            edit.insert = range_to_editor(text, edit.insert);
            edit.replace = range_to_editor(text, edit.replace);
        }
        None => {}
    }
    for edit in item.additional_text_edits.iter_mut().flatten() {
        edit.range = range_to_editor(text, edit.range);
    }
}

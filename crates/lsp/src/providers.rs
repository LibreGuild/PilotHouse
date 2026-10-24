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
            position: text.offset_to_position(offset),
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
        manager.update(cx, |manager, cx| {
            manager.request::<HoverRequest>(&self.path, params, cx)
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
        cx.background_spawn(async move {
            Ok(match request.await? {
                None => Vec::new(),
                Some(GotoDefinitionResponse::Link(links)) => links,
                Some(GotoDefinitionResponse::Scalar(location)) => vec![link(location)],
                Some(GotoDefinitionResponse::Array(locations)) => {
                    locations.into_iter().map(link).collect()
                }
            })
        })
    }
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
        cx.background_spawn(async move {
            Ok(request
                .await?
                .unwrap_or(CompletionResponse::Array(Vec::new())))
        })
    }

    fn is_completion_trigger(&self, _offset: usize, new_text: &str, _: &mut App) -> bool {
        new_text.chars().last().is_some_and(|c| {
            c.is_alphanumeric() || matches!(c, '_' | '.' | ':' | '>' | '/' | '@' | '<' | '"' | '\'')
        })
    }
}

use gpui_kit::AppContext as _;

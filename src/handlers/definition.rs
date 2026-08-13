//! Go-to-definition for symbols inside SpeedScript sections.

use tower_lsp::lsp_types::{GotoDefinitionParams, GotoDefinitionResponse, Location, Url};

use crate::analysis::SymbolKind;
use crate::backend::Backend;

pub async fn goto_definition(
    backend: &Backend,
    params: GotoDefinitionParams,
) -> Option<GotoDefinitionResponse> {
    let text_document_position = &params.text_document_position_params;
    let uri: &Url = &text_document_position.text_document.uri;
    let doc = backend.state.documents.get(uri)?;
    let position = text_document_position.position;
    let (idx, _point) = doc.resolve_position(position)?;
    let (name, _range) = crate::handlers::hover::word_at(&doc, idx, position)?;
    let analysis = crate::analysis::analyze(&doc, idx)?;
    let symbol = analysis.find(&name)?;
    if symbol.kind == SymbolKind::Function && symbol.range == (0, 0) {
        return None;
    }
    let sec = &doc.sections()[idx];
    let start = doc.point_to_position(sec.content_start + symbol.range.0);
    let end = doc.point_to_position(sec.content_start + symbol.range.1);
    Some(GotoDefinitionResponse::Scalar(Location {
        uri: uri.clone(),
        range: tower_lsp::lsp_types::Range { start, end },
    }))
}

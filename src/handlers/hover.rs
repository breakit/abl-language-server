//! Hover for symbols inside SpeedScript sections.

use tower_lsp::lsp_types::{Hover, HoverContents, HoverParams, MarkupContent, MarkupKind};

use crate::analysis::SymbolKind;
use crate::backend::Backend;
use crate::document::WebSpeedDocument;

/// The identifier word spanning an LSP position within a code section.
pub fn word_at(
    doc: &WebSpeedDocument,
    idx: usize,
    position: tower_lsp::lsp_types::Position,
) -> Option<(String, tower_lsp::lsp_types::Range)> {
    let sec = &doc.sections()[idx];
    let byte = doc.position_to_byte(position)?;
    let content_byte = byte.saturating_sub(sec.content_start).min(sec.content_end - sec.content_start);
    let content = sec.content(&doc.text);
    let b = content.as_bytes();
    if content_byte < content.len() && !b[content_byte].is_ascii_alphanumeric() {
        return None;
    }
    let mut start = content_byte;
    while start > 0 && b[start - 1].is_ascii_alphanumeric() {
        start -= 1;
    }
    let mut end = content_byte;
    while end < content.len() && b[end].is_ascii_alphanumeric() {
        end += 1;
    }
    let text = content[start..end].to_string();
    let range = tower_lsp::lsp_types::Range {
        start: doc.point_to_position(sec.content_start + start),
        end: doc.point_to_position(sec.content_start + end),
    };
    Some((text, range))
}

pub async fn hover(backend: &Backend, params: HoverParams) -> Option<Hover> {
    let text_document_position = &params.text_document_position_params;
    let uri = &text_document_position.text_document.uri;
    let doc = backend.state.documents.get(uri)?;
    let position = text_document_position.position;
    let (idx, _point) = doc.resolve_position(position)?;
    let (name, range) = word_at(&doc, idx, position)?;
    let analysis = crate::analysis::analyze(&doc, idx)?;
    let symbol = analysis.find(&name)?;
    let kind_name = match symbol.kind {
        SymbolKind::Variable => "variable",
        SymbolKind::Parameter => "parameter",
        SymbolKind::Buffer => "buffer",
        SymbolKind::Property => "property",
        SymbolKind::Function => "function",
        SymbolKind::Table => "table",
        SymbolKind::Preprocessor => "preprocessor define",
    };
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format!("**{}** — {} definition", symbol.name, kind_name),
        }),
        range: Some(range),
    })
}
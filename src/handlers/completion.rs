//! Completion inside SpeedScript sections.

use tower_lsp::lsp_types::*;

use crate::document::WebSpeedDocument;

pub fn complete(
    doc: &WebSpeedDocument,
    position: Position,
    include_keywords: bool,
) -> Option<Vec<CompletionItem>> {
    let (idx, point) = doc.resolve_position(position)?;
    let sec = &doc.sections()[idx];
    if sec.kind != crate::scanner::SectionKind::SpeedScript {
        return None;
    }
    let analysis = crate::analysis::analyze(doc, idx)?;
    let content = sec.content(&doc.text);
    let prefix = prefix_at(content, point.column);
    let lower = prefix.to_ascii_lowercase();

    let mut items: Vec<CompletionItem> = Vec::new();
    for sym in analysis.symbols() {
        if !sym.name.to_ascii_lowercase().starts_with(&lower) {
            continue;
        }
        let kind = match sym.kind {
            crate::analysis::SymbolKind::Variable => Some(CompletionItemKind::VARIABLE),
            crate::analysis::SymbolKind::Parameter => Some(CompletionItemKind::VARIABLE),
            crate::analysis::SymbolKind::Buffer => Some(CompletionItemKind::CLASS),
            crate::analysis::SymbolKind::Property => Some(CompletionItemKind::PROPERTY),
            crate::analysis::SymbolKind::Function => Some(CompletionItemKind::FUNCTION),
            crate::analysis::SymbolKind::Table => Some(CompletionItemKind::STRUCT),
            crate::analysis::SymbolKind::Preprocessor => Some(CompletionItemKind::CONSTANT),
        };
        items.push(CompletionItem {
            label: sym.name.clone(),
            kind,
            ..Default::default()
        });
    }
    if include_keywords {
        for kw in crate::keywords::ABL_KEYWORDS {
            if kw.to_ascii_lowercase().starts_with(&lower) {
                items.push(CompletionItem {
                    label: kw.to_string(),
                    kind: Some(CompletionItemKind::KEYWORD),
                    sort_text: Some("1".to_string()),
                    ..Default::default()
                });
            }
        }
    }
    Some(items)
}

/// Identifier prefix ending at `column` (byte column within the section).
fn prefix_at(content: &str, column: usize) -> &str {
    let byte = column.min(content.len());
    let head = &content[..byte];
    let start = head
        .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'))
        .map(|i| i + 1)
        .unwrap_or(0);
    &head[start..]
}
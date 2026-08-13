//! Semantic tokens for SpeedScript sections.
//!
//! The legend maps tree-sitter-abl node kinds onto standard semantic token
//! types; the encoder walks each SpeedScript tree and emits relative
//! delta-encoded tokens.

use tower_lsp::lsp_types::{
    SemanticTokenType, SemanticTokens, SemanticTokensLegend, SemanticTokensResult,
};

pub const TYPES: &[&str] = &[
    "variable",
    "parameter",
    "type",
    "function",
    "property",
    "keyword",
    "string",
    "number",
    "comment",
    "namespace",
];

pub fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: TYPES.iter().map(|s| SemanticTokenType::new(s)).collect(),
        token_modifiers: vec![],
    }
}

/// Returns semantic tokens for all SpeedScript sections.
pub fn tokens_for(doc: &crate::document::WebSpeedDocument) -> Option<SemanticTokensResult> {
    let mut candidates: Vec<crate::semantic::Candidate> = Vec::new();
    for (idx, sec) in doc.sections().iter().enumerate() {
        if sec.kind != crate::scanner::SectionKind::SpeedScript {
            continue;
        }
        let tree = doc.tree(idx)?;
        crate::semantic::collect(doc, idx, tree.root_node(), &mut candidates);
    }
    let data = crate::semantic::delta_encode(&mut candidates);
    Some(SemanticTokensResult::Tokens(SemanticTokens {
        result_id: None,
        data,
    }))
}

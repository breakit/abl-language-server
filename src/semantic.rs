//! Tree-walk semantic classification for SpeedScript sections.
//!
//! Maps tree-sitter-abl node kinds onto the semantic token legend in
//! `handlers::semantic_tokens`, then delta-encodes tokens in document order.

use tower_lsp::lsp_types::{Position, SemanticToken};
use tree_sitter::Node;

use crate::document::WebSpeedDocument;

/// Token type indices into `TYPES`.
const VARIABLE: usize = 0;
const PARAMETER: usize = 1;
const TYPE: usize = 2;
const FUNCTION: usize = 3;
const PROPERTY: usize = 4;
const KEYWORD: usize = 5;
const STRING: usize = 6;
const NUMBER: usize = 7;
const COMMENT: usize = 8;
const NAMESPACE: usize = 9;

/// A classified token before delta encoding.
#[derive(Clone, Copy)]
pub struct Candidate {
    pub position: Position,
    pub length: u32,
    pub token_type: usize,
}

/// Classifies a node; `None` means "no token".
fn classify(node: Node) -> Option<usize> {
    match node.kind() {
        "comment" | "line_comment" | "block_comment" => Some(COMMENT),
        "string_literal" => Some(STRING),
        "number_literal" | "date_literal" | "boolean_literal" | "null_literal" => Some(NUMBER),
        "function_definition" | "function_forward_definition" | "function_call" => Some(FUNCTION),
        "variable_definition" => Some(VARIABLE),
        "parameter_definition" => Some(PARAMETER),
        "property_definition" => Some(PROPERTY),
        "nested_type_name" | "generic_type" => Some(TYPE),
        "namespace_prefix" => Some(NAMESPACE),
        kind if is_anonymous_keyword(kind) => Some(KEYWORD),
        kind if is_preprocessor(kind) => Some(KEYWORD),
        _ => None,
    }
}

/// Anonymous tokens whose kind is uppercase ABL keyword text (e.g. `MESSAGE`,
/// `DEFINE`, `IF`); excludes punctuation and operators.
fn is_anonymous_keyword(kind: &str) -> bool {
    let mut chars = kind.chars();
    match chars.next() {
        Some(c) if c.is_ascii_uppercase() => {}
        _ => return false,
    }
    let rest = chars.as_str();
    !rest.is_empty()
        && rest
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-')
}

fn is_preprocessor(kind: &str) -> bool {
    kind.starts_with("global_define_preprocessor_directive")
        || kind.starts_with("scoped_define_preprocessor_directive")
        || kind.starts_with("if_preprocessor_directive")
        || kind.starts_with("elseif_branch")
        || kind.starts_with("else_branch")
        || kind.starts_with("endif_branch")
        || kind.starts_with("undefine_preprocessor_directive")
        || kind.starts_with("message_preprocessor_directive")
        || kind.starts_with("include_file_reference")
        || kind == "preprocessor_name"
}

/// Collects classification candidates in tree order (not yet encoded).
pub fn collect(doc: &WebSpeedDocument, idx: usize, node: Node, out: &mut Vec<Candidate>) {
    if let Some(token_type) = classify(node) {
        let range = doc.node_range(idx, node);
        let length = range.end.character.saturating_sub(range.start.character) as u32;
        if length > 0 {
            out.push(Candidate { position: range.start, length, token_type });
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect(doc, idx, child, out);
    }
}

/// Sorts candidates into document order and delta-encodes them.
pub fn delta_encode(candidates: &mut Vec<Candidate>) -> Vec<SemanticToken> {
    candidates.sort_by_key(|c| (c.position.line, c.position.character));
    let mut out = Vec::with_capacity(candidates.len());
    let mut prev_line = 0u32;
    let mut prev_char = 0u32;
    for c in candidates {
        let delta_line = c.position.line - prev_line;
        let delta_start = if delta_line == 0 {
            c.position.character - prev_char
        } else {
            c.position.character
        };
        out.push(SemanticToken {
            delta_line,
            delta_start,
            length: c.length,
            token_type: c.token_type as u32,
            token_modifiers_bitset: 0,
        });
        prev_line = c.position.line;
        prev_char = c.position.character;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::WebSpeedDocument;

    #[test]
    fn encodes_speedscript_section() {
        let mut doc = WebSpeedDocument::new();
        doc.update("<% /* note */ message \"hi\" + 1. %>".to_string());
        let idx = doc
            .sections()
            .iter()
            .position(|s| s.kind == crate::scanner::SectionKind::SpeedScript)
            .unwrap();
        let mut cands = Vec::new();
        collect(&doc, idx, doc.tree(idx).unwrap().root_node(), &mut cands);
        let tokens = delta_encode(&mut cands);
        assert!(!tokens.is_empty());
        let types: Vec<u32> = tokens.iter().map(|t| t.token_type).collect();
        assert!(types.contains(&(COMMENT as u32)), "{types:?}");
        assert!(types.contains(&(STRING as u32)), "{types:?}");
        assert!(types.contains(&(NUMBER as u32)), "{types:?}");
        assert!(types.contains(&(KEYWORD as u32)), "{types:?}");
    }

    #[test]
    fn delta_starts_at_zero() {
        let mut doc = WebSpeedDocument::new();
        doc.update("<% message \"hi\". %>".to_string());
        let idx = doc
            .sections()
            .iter()
            .position(|s| s.kind == crate::scanner::SectionKind::SpeedScript)
            .unwrap();
        let mut cands = Vec::new();
        collect(&doc, idx, doc.tree(idx).unwrap().root_node(), &mut cands);
        let tokens = delta_encode(&mut cands);
        assert!(tokens.iter().all(|t| t.delta_line == 0));
    }
}
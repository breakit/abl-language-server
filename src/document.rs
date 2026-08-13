//! In-memory model of an open WebSpeed document.
//!
//! Owns the raw text, the section layout (from `scanner::scan`), a per-section
//! parse tree for code sections, and the line indexes used to convert between
//! tree-sitter-local coordinates and LSP document coordinates.

use std::sync::Mutex;

use tree_sitter::{Node, Parser, Point, Tree};

use crate::positions::LineIndex;
use crate::scanner::{scan, Section, SectionKind};

/// Builds and reuses the JavaScript parser (tree-sitter parsers are stateful).
struct JsParser(Mutex<Parser>);

impl JsParser {
    fn new() -> Self {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_javascript::LANGUAGE.into())
            .expect("tree-sitter-javascript language failed to load");
        JsParser(Mutex::new(parser))
    }

    fn parse(&self, source: &str) -> Option<Tree> {
        self.0.lock().expect("js parser lock").parse(source, None)
    }
}

/// A WebSpeed document as seen by the server.
#[derive(Default)]
pub struct WebSpeedDocument {
    pub text: String,
    line_index: LineIndex,
    sections: Vec<Section>,
    /// Line index of each section's content, aligned with `sections`.
    content_indexes: Vec<Option<LineIndex>>,
    /// Parse tree of each section, aligned with `sections` (None for Html).
    trees: Vec<Option<Tree>>,
    js: JsParser,
}

impl Default for JsParser {
    fn default() -> Self {
        Self::new()
    }
}

impl WebSpeedDocument {
    pub fn new() -> Self {
        WebSpeedDocument::default()
    }

    /// Replaces the document content (FULL text sync) and reparses.
    pub fn update(&mut self, text: String) {
        self.text = text;
        self.line_index = LineIndex::new(&self.text);
        self.sections = scan(&self.text);
        self.content_indexes = self
            .sections
            .iter()
            .map(|s| match s.kind {
                SectionKind::Html => None,
                _ => Some(LineIndex::new(s.content(&self.text))),
            })
            .collect();
        self.trees = self
            .sections
            .iter()
            .map(|s| match s.kind {
                SectionKind::SpeedScript => crate::parse_abl(s.content(&self.text)),
                SectionKind::Javascript => self.js.parse(s.content(&self.text)),
                SectionKind::Html => None,
            })
            .collect();
    }

    pub fn sections(&self) -> &[Section] {
        &self.sections
    }

    /// Global byte offset -> section index, or `None` outside any code block.
    pub fn section_index_at_byte(&self, byte: usize) -> Option<usize> {
        let idx = self.sections.partition_point(|s| s.start <= byte);
        let sec = self.sections.get(idx.wrapping_sub(1))?;
        if byte < sec.end && sec.kind != SectionKind::Html {
            Some(idx - 1)
        } else {
            None
        }
    }

    /// LSP position -> (section index, point within that section's content).
    pub fn resolve_position(&self, position: tower_lsp::lsp_types::Position) -> Option<(usize, Point)> {
        let byte = self.line_index.byte_of(&self.text, position)?;
        let idx = self.section_index_at_byte(byte)?;
        let sec = &self.sections[idx];
        let content_index = self.content_indexes[idx].as_ref()?;
        let local = byte
            .min(sec.content_end)
            .saturating_sub(sec.content_start)
            .min(sec.content_end - sec.content_start);
        Some((idx, content_index.point_of(sec.content(&self.text), local)))
    }

    /// Tree-sitter node range (within a code section) -> LSP range in the
    /// document. The section's content start byte is `sections[idx].content_start`.
    pub fn node_range(&self, idx: usize, node: Node) -> tower_lsp::lsp_types::Range {
        let sec = &self.sections[idx];
        let content_index = self.content_indexes[idx].as_ref().expect("code section index");
        let start_byte = sec.content_start + self.point_byte(content_index, node.start_position());
        let end_byte = sec.content_start + self.point_byte(content_index, node.end_position());
        tower_lsp::lsp_types::Range {
            start: self.line_index.position_of(&self.text, start_byte),
            end: self.line_index.position_of(&self.text, end_byte),
        }
    }

    /// Parse tree of a code section.
    pub fn tree(&self, idx: usize) -> Option<&Tree> {
        self.trees[idx].as_ref()
    }

    /// Section whose tree contains `node`.
    pub fn section_of_node(&self, idx: usize, _node: Node) -> &Section {
        &self.sections[idx]
    }

    fn point_byte(&self, index: &LineIndex, point: Point) -> usize {
        index.starts()[point.row] + point.column
    }
}

/// Walks a tree and collects the topmost ERROR/MISSING nodes.
pub fn collect_error_nodes<'t>(node: Node<'t>, out: &mut Vec<Node<'t>>) {
    if node.is_error() || node.is_missing() {
        out.push(node);
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_error_nodes(child, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::positions::utf16_len;

    #[test]
    fn parses_all_sections() {
        let mut doc = WebSpeedDocument::new();
        doc.update("<script>let a = 1;</script><% message \"hi\". %>".to_string());
        let tree_count = doc.trees.iter().flatten().count();
        assert_eq!(tree_count, 2);
        let idx = doc.sections.iter().position(|s| s.kind == SectionKind::SpeedScript).unwrap();
        let tree = doc.tree(idx).unwrap();
        assert!(!tree.root_node().has_error(), "{}", tree.root_node().to_sexp());
        let content = doc.sections[idx].content(&doc.text);
        assert!(content.contains("message"));
    }

    #[test]
    fn ably_syntax_errors_are_collected() {
        let mut doc = WebSpeedDocument::new();
        doc.update("before<% message \"hi\" %><script>let a = ;</script>".to_string());
        let sp = doc
            .sections
            .iter()
            .position(|s| s.kind == SectionKind::SpeedScript)
            .unwrap();
        let js = doc
            .sections
            .iter()
            .position(|s| s.kind == SectionKind::Javascript)
            .unwrap();
        let mut errors = Vec::new();
        collect_error_nodes(doc.tree(sp).unwrap().root_node(), &mut errors);
        assert!(!errors.is_empty(), "missing terminator must error");
        let mut js_errors = Vec::new();
        collect_error_nodes(doc.tree(js).unwrap().root_node(), &mut js_errors);
        assert!(!js_errors.is_empty());
    }

    #[test]
    fn node_ranges_map_to_document_coordinates() {
        let mut doc = WebSpeedDocument::new();
        doc.update("<html>\n<% message \"hi\". %>\n</html>".to_string());
        let sp = doc
            .sections
            .iter()
            .position(|s| s.kind == SectionKind::SpeedScript)
            .unwrap();
        let tree = doc.tree(sp).unwrap();
        let root = tree.root_node();
        let mut cursor = root.walk();
        let msg = root
            .children(&mut cursor)
            .find(|n| n.kind() == "message_statement")
            .expect("message statement");
        let range = doc.node_range(sp, msg);
        assert_eq!(range.start.line, 1);
        assert_eq!(range.start.character, 3);
        assert_eq!(range.end.line, 1);
        assert_eq!(range.end.character, 3 + utf16_len("message \"hi\"."));
    }

    #[test]
    fn resolve_position_locates_sections() {
        let mut doc = WebSpeedDocument::new();
        doc.update("<script>ab()</script><% message \"hi\". %>".to_string());
        // Cursor inside the <script> open tag clamps to JS content start.
        let (idx, point) = doc
            .resolve_position(tower_lsp::lsp_types::Position { line: 0, character: 2 })
            .unwrap();
        assert_eq!(doc.sections[idx].kind, SectionKind::Javascript);
        assert_eq!((point.row, point.column), (0, 0));

        // Cursor inside the SpeedScript block resolves to its content.
        let (idx, point) = doc
            .resolve_position(tower_lsp::lsp_types::Position { line: 0, character: 24 })
            .unwrap();
        assert_eq!(doc.sections[idx].kind, SectionKind::SpeedScript);
        assert_eq!((point.row, point.column), (0, 1));
    }

    #[test]
    fn html_positions_resolve_to_none() {
        let mut doc = WebSpeedDocument::new();
        doc.update("<p>text</p><% a. %>".to_string());
        let pos = tower_lsp::lsp_types::Position { line: 0, character: 3 };
        assert!(doc.resolve_position(pos).is_none());
    }
}
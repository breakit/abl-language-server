//! WebSpeed Language Server library.
//!
//! Serves OpenEdge WebSpeed `.htm`/`.html` files: composite documents with
//! embedded SpeedScript (`<% ... %>`) and JavaScript (`<script>`) sections.

pub mod document;
pub mod positions;
pub mod scanner;

pub use tree_sitter::{self, Node, Point, Tree};

/// Parses ABL/SpeedScript source with the shared ecosystem grammar.
pub fn parse_abl(source: &str) -> Option<Tree> {
    abl_parser::parse(source)
}
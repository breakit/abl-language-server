//! WebSpeed Language Server library.
//!
//! Serves OpenEdge WebSpeed `.htm`/`.html` files: composite documents with
//! embedded SpeedScript (`<% ... %>`) and JavaScript (`<script>`) sections.

pub mod analysis;
pub mod backend;
pub mod config;
pub mod document;
pub mod handlers;
pub mod keywords;
pub mod positions;
pub mod scanner;
pub mod semantic;

pub use tree_sitter::{self, Node, Point, Tree};

/// Parses ABL/SpeedScript source with the shared ecosystem grammar.
pub fn parse_abl(source: &str) -> Option<Tree> {
    abl_parser::parse(source)
}

/// Runs the language server over stdio until the client exits.
pub async fn run() {
    use tower_lsp::{LspService, Server};

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(backend::Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
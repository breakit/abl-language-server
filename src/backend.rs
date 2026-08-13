use tower_lsp::Client;

/// LSP request handler state.
pub struct Backend {
    #[allow(dead_code)]
    client: Client,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Backend { client }
    }
}
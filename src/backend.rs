use std::path::Path;
use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::Mutex;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{async_trait, Client, LanguageServer};

use crate::config::{self, ServerConfig};
use crate::document::WebSpeedDocument;

/// Shared server state.
pub struct BackendState {
    pub documents: DashMap<Url, WebSpeedDocument>,
    pub config: Mutex<ServerConfig>,
}

/// LSP request handler.
pub struct Backend {
    pub client: Client,
    pub state: Arc<BackendState>,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Backend {
            client,
            state: Arc::new(BackendState {
                documents: DashMap::new(),
                config: Mutex::new(ServerConfig::default()),
            }),
        }
    }

    fn is_webspeed(uri: &Url) -> bool {
        let ext = Path::new(uri.path())
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default();
        matches!(ext.to_ascii_lowercase().as_str(), "htm" | "html")
    }

    fn is_config(uri: &Url) -> bool {
        uri.path().ends_with("abl.toml")
    }

    /// Reloads configuration from the workspace root.
    async fn reload_config(&self, root: &Path) {
        let cfg = config::load(root);
        log::info!(
            "config: {} diagnostics={} formatting={}",
            cfg.source.as_deref().map(|p| p.display().to_string()).unwrap_or_else(|| "<defaults>".to_string()),
            cfg.lsp.diagnostics.enabled,
            cfg.lsp.formatting.enabled
        );
        *self.state.config.lock().await = cfg;
    }

    async fn publish_diagnostics_for(&self, uri: &Url) {
        let (diags, version) = {
            let guard = self.state.documents.get(uri);
            let Some(doc) = guard else { return };
            (crate::handlers::diagnostics::collect(doc.value()), doc.version())
        };
        let _ = self
            .client
            .publish_diagnostics(uri.clone(), diags, version)
            .await;
    }
}

#[async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let root = params
            .root_uri
            .clone()
            .or_else(|| {
                params
                    .workspace_folders
                    .as_ref()
                    .and_then(|fs| fs.first())
                    .map(|f| f.uri.clone())
            })
            .and_then(|u| u.to_file_path().ok())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

        self.reload_config(&root).await;

        let capabilities = ServerCapabilities {
            text_document_sync: Some(TextDocumentSyncCapability::Kind(
                TextDocumentSyncKind::FULL,
            )),
            completion_provider: Some(CompletionOptions::default()),
            hover_provider: Some(HoverProviderCapability::Simple(true)),
            definition_provider: Some(OneOf::Left(true)),
            document_formatting_provider: Some(OneOf::Left(true)),
            semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
                SemanticTokensOptions {
                    legend: crate::handlers::semantic_tokens::legend(),
                    full: Some(SemanticTokensFullOptions::Bool(true)),
                    range: None,
                    work_done_progress_options: Default::default(),
                },
            )),
            ..Default::default()
        };

        Ok(InitializeResult {
            capabilities,
            ..Default::default()
        })
    }

    async fn initialized(&self, _params: InitializedParams) {}

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        if !Self::is_webspeed(&uri) {
            return;
        }
        let mut doc = WebSpeedDocument::new();
        doc.update(params.text_document.text);
        doc.set_version(Some(params.text_document.version));
        self.state.documents.insert(uri.clone(), doc);
        self.publish_diagnostics_for(&uri).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        if !Self::is_webspeed(&uri) {
            if Self::is_config(&uri) {
                let root = std::env::current_dir().unwrap_or_default();
                self.reload_config(&root).await;
            }
            return;
        }
        let Some(mut guard) = self.state.documents.get_mut(&uri) else {
            return;
        };
        if let Some(change) = params.content_changes.last() {
            guard.update(change.text.clone());
        }
        guard.set_version(Some(params.text_document.version));
        drop(guard);
        self.publish_diagnostics_for(&uri).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri;
        if Self::is_config(&uri) {
            let root = std::env::current_dir().unwrap_or_default();
            self.reload_config(&root).await;
            return;
        }
        if !Self::is_webspeed(&uri) {
            return;
        }
        if let Some(doc) = self.state.documents.get(&uri) {
            crate::handlers::diagnostics::run_lint(&self.client, &uri, doc.value(), &self.state).await;
        }
        self.publish_diagnostics_for(&uri).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.state.documents.remove(&uri);
        let _ = self.client.publish_diagnostics(uri, vec![], None).await;
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let Some(doc) = self.state.documents.get(&uri) else {
            return Ok(None);
        };
        let include_keywords = self.state.config.lock().await.lsp.completion.keywords;
        let items = crate::handlers::completion::complete(&doc, position, include_keywords);
        Ok(items.map(CompletionResponse::Array))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        Ok(crate::handlers::hover::hover(self, params).await)
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        Ok(crate::handlers::definition::goto_definition(self, params).await)
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let uri = params.text_document.uri;
        let Some(doc) = self.state.documents.get(&uri) else {
            return Ok(None);
        };
        let cfg = self.state.config.lock().await.clone();
        if !cfg.lsp.formatting.enabled {
            return Ok(None);
        }
        Ok(crate::handlers::formatting::format_document(
            &doc,
            cfg.lsp.formatting.idempotence,
        ))
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let uri = params.text_document.uri;
        let Some(doc) = self.state.documents.get(&uri) else {
            return Ok(None);
        };
        let cfg = self.state.config.lock().await.clone();
        if !cfg.lsp.semantic_tokens.enabled {
            return Ok(None);
        }
        Ok(crate::handlers::semantic_tokens::tokens_for(&doc))
    }
}
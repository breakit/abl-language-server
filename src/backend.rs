use dashmap::mapref::entry::Entry;
use dashmap::{DashMap, DashSet};
use log::{debug, warn};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use tokio::sync::Mutex as AsyncMutex;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};
use tree_sitter::{Language, Parser, Tree};

use crate::config::{AblConfig, find_workspace_root, load_from_workspace_root};
use crate::utils::paths::{
    expand_dumpfile_pattern, resolve_config_path, resolve_glob_pattern, resolve_include_path,
};

#[derive(Clone)]
pub struct DbFieldInfo {
    pub name: String,
    pub field_type: Option<String>,
    pub format: Option<String>,
    pub label: Option<String>,
    pub description: Option<String>,
}

#[derive(Clone)]
pub struct CachedCompletionSymbol {
    pub label: String,
    pub kind: CompletionItemKind,
    pub detail: String,
}

pub struct IncludeCompletionCacheEntry {
    pub symbols: Vec<CachedCompletionSymbol>,
}

pub struct IncludeParseCacheEntry {
    pub text: Arc<String>,
    pub tree: Tree,
}

pub struct DocumentState {
    pub text: String,
    pub version: i32,
    pub tree_version: i32,
    pub tree: Option<Tree>,
    pub parser: StdMutex<Parser>,
    pub diag_task: Option<tokio::task::JoinHandle<()>>,
}

pub struct BackendState {
    pub abl_language: Language,
    pub df_parser: AsyncMutex<Parser>,
    pub documents: DashMap<Url, DocumentState>,
    pub workspace_root: AsyncMutex<Option<std::path::PathBuf>>,
    pub config: AsyncMutex<AblConfig>,
    pub db_tables: DashSet<String>,
    pub db_sequences: DashSet<String>,
    pub db_table_labels: DashMap<String, String>,
    pub db_table_definitions: DashMap<String, Vec<Location>>,
    pub db_sequence_definitions: DashMap<String, Vec<Location>>,
    pub db_field_definitions: DashMap<String, Vec<Location>>,
    pub db_index_definitions: DashMap<String, Vec<Location>>,
    pub db_indexes_by_table: DashMap<String, Vec<String>>,
    pub db_index_fields_by_table_index: DashMap<String, Vec<String>>,
    pub db_fields_by_table: DashMap<String, Vec<DbFieldInfo>>,
    pub include_completion_cache: DashMap<PathBuf, IncludeCompletionCacheEntry>,
    pub include_parse_cache: DashMap<PathBuf, IncludeParseCacheEntry>,
    pub embedded_ambient_paths: AsyncMutex<Option<Vec<PathBuf>>>,
}

#[derive(Clone)]
pub struct Backend {
    pub client: Client,
    pub state: Arc<BackendState>,
}

impl Deref for Backend {
    type Target = BackendState;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let root = find_workspace_root(&params);
        {
            let mut workspace_root = self.workspace_root.lock().await;
            *workspace_root = root;
        }
        self.reload_workspace_config().await;
        let semantic_tokens_enabled = self.config.lock().await.semantic_tokens.enabled;

        Ok(InitializeResult {
            server_info: None,
            offset_encoding: Some("utf-8".to_string()),

            capabilities: ServerCapabilities {
                document_formatting_provider: Some(OneOf::Left(true)),
                inlay_hint_provider: None,
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::FULL),
                        save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                            include_text: Some(true),
                        })),
                        ..Default::default()
                    },
                )),
                completion_provider: Some(CompletionOptions {
                    resolve_provider: Some(false),
                    trigger_characters: Some(vec![".".to_string(), " ".to_string()]),
                    work_done_progress_options: Default::default(),
                    all_commit_characters: None,
                    completion_item: None,
                }),
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
                    retrigger_characters: Some(vec![",".to_string()]),
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                }),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                execute_command_provider: None,
                workspace: None,
                semantic_tokens_provider: if semantic_tokens_enabled {
                    Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
                        SemanticTokensOptions {
                            legend: SemanticTokensLegend {
                                token_types: vec![SemanticTokenType::TYPE],
                                token_modifiers: vec![],
                            },
                            range: Some(true),
                            full: Some(SemanticTokensFullOptions::Bool(true)),
                            work_done_progress_options: WorkDoneProgressOptions::default(),
                        },
                    ))
                } else {
                    None
                },
                definition_provider: Some(OneOf::Left(true)),
                references_provider: Some(OneOf::Left(true)),
                rename_provider: None,
                ..ServerCapabilities::default()
            },
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        debug!("initialized!");
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        self.handle_did_open(params).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        self.handle_did_change(params).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        self.handle_did_save(params).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        self.handle_did_close(params).await;
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        self.handle_goto_definition(params).await
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        self.handle_references(params).await
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        self.handle_hover(params).await
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        self.handle_semantic_tokens_full(params).await
    }

    async fn semantic_tokens_range(
        &self,
        params: SemanticTokensRangeParams,
    ) -> Result<Option<SemanticTokensRangeResult>> {
        self.handle_semantic_tokens_range(params).await
    }

    async fn inlay_hint(
        &self,
        _params: tower_lsp::lsp_types::InlayHintParams,
    ) -> Result<Option<Vec<InlayHint>>> {
        Ok(None)
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        self.handle_completion(params).await
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        self.handle_signature_help(params).await
    }

    async fn rename(&self, _params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        Ok(None)
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        self.handle_formatting(params).await
    }

    async fn did_change_configuration(&self, _: DidChangeConfigurationParams) {
        self.reload_workspace_config().await;
        debug!("configuration changed!");
    }

    async fn did_change_workspace_folders(&self, params: DidChangeWorkspaceFoldersParams) {
        if let Some(folder) = params.event.added.first() {
            if let Ok(path) = folder.uri.to_file_path() {
                let mut workspace_root = self.workspace_root.lock().await;
                *workspace_root = Some(path);
            }
        } else {
            let mut workspace_root = self.workspace_root.lock().await;
            *workspace_root = None;
        }
        self.reload_workspace_config().await;
        debug!("workspace folders changed!");
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        for change in params.changes {
            if is_abl_toml_uri(&change.uri) {
                self.reload_workspace_config().await;
                break;
            } else if self.is_configured_dumpfile_uri(&change.uri).await {
                self.reload_db_tables_from_current_config().await;
                break;
            }
        }
        debug!("watched files have changed!");
    }

    async fn execute_command(&self, _: ExecuteCommandParams) -> Result<Option<Value>> {
        debug!("command executed!");

        Ok(None)
    }
}

impl Backend {
    fn new_document_state(&self, text: String, version: i32) -> DocumentState {
        DocumentState {
            text,
            version,
            tree_version: -1,
            tree: None,
            parser: StdMutex::new(self.new_abl_parser()),
            diag_task: None,
        }
    }

    pub fn new_abl_parser(&self) -> Parser {
        let mut parser = Parser::new();
        parser
            .set_language(&self.abl_language)
            .expect("Error loading abl parser");
        parser
    }

    pub fn get_document_text(&self, uri: &Url) -> Option<String> {
        self.documents.get(uri).map(|d| d.text.clone())
    }

    pub fn get_document_version(&self, uri: &Url) -> Option<i32> {
        self.documents.get(uri).map(|d| d.version)
    }

    pub fn set_document_text_version(
        &self,
        uri: &Url,
        version: i32,
        text: String,
        clear_tree: bool,
    ) {
        match self.documents.entry(uri.clone()) {
            Entry::Occupied(mut entry) => {
                let doc = entry.get_mut();
                doc.version = version;
                doc.text = text;
                if clear_tree {
                    doc.tree = None;
                    doc.tree_version = -1;
                }
            }
            Entry::Vacant(entry) => {
                let mut doc = self.new_document_state(text, version);
                if clear_tree {
                    doc.tree = None;
                    doc.tree_version = -1;
                }
                entry.insert(doc);
            }
        }
    }

    pub fn get_document_tree_or_parse(&self, uri: &Url) -> Option<Tree> {
        let mut doc = self.documents.get_mut(uri)?;
        if doc.tree_version == doc.version
            && let Some(tree) = &doc.tree
        {
            return Some(tree.clone());
        }
        let text = doc.text.clone();
        let parsed = {
            let mut parser = doc.parser.lock().expect("ABL parser mutex poisoned");
            parser.parse(text.as_str(), None)?
        };
        doc.tree = Some(parsed.clone());
        doc.tree_version = doc.version;
        Some(parsed)
    }

    pub fn set_document_tree_if_version(&self, uri: &Url, version: i32, tree: Tree) {
        if let Some(mut doc) = self.documents.get_mut(uri)
            && doc.version == version
        {
            doc.tree = Some(tree);
            doc.tree_version = version;
        }
    }

    pub fn take_document_diag_task(&self, uri: &Url) -> Option<tokio::task::JoinHandle<()>> {
        let mut doc = self.documents.get_mut(uri)?;
        doc.diag_task.take()
    }

    pub fn replace_document_diag_task(&self, uri: &Url, handle: tokio::task::JoinHandle<()>) {
        match self.documents.entry(uri.clone()) {
            Entry::Occupied(mut entry) => {
                let doc = entry.get_mut();
                if let Some(prev) = doc.diag_task.take() {
                    prev.abort();
                }
                doc.diag_task = Some(handle);
            }
            Entry::Vacant(entry) => {
                let mut doc = self.new_document_state(String::new(), -1);
                doc.diag_task = Some(handle);
                entry.insert(doc);
            }
        }
    }

    pub async fn reload_workspace_config(&self) {
        let workspace_root = self.workspace_root.lock().await.clone();
        let loaded = load_from_workspace_root(workspace_root.as_deref()).await;

        let dumpfiles = loaded.config.dumpfile.clone();
        let mut config = self.config.lock().await;
        *config = loaded.config;
        drop(config);

        self.reload_db_tables(workspace_root.as_deref(), &dumpfiles)
            .await;

        if let Some(path) = loaded.path {
            if Path::new(&path).exists() {
                debug!("loaded workspace config from {}", path.display());
            } else {
                debug!(
                    "workspace config not found, using defaults (expected path: {})",
                    path.display()
                );
            }
        } else {
            warn!("workspace root is unknown; using default config");
        }
    }

    pub async fn maybe_reload_config_for_uri(&self, uri: &Url) {
        if is_abl_toml_uri(uri) {
            self.reload_workspace_config().await;
        }
    }

    pub async fn maybe_reload_db_tables_for_uri(&self, uri: &Url) {
        if self.is_configured_dumpfile_uri(uri).await {
            self.reload_db_tables_from_current_config().await;
        }
    }

    pub async fn resolve_include_path_for(
        &self,
        current_file: &Path,
        include: &str,
    ) -> Option<std::path::PathBuf> {
        let workspace_root = self.workspace_root.lock().await.clone();
        let propath = self.config.lock().await.propath.clone();
        resolve_include_path(workspace_root.as_deref(), &propath, current_file, include)
    }

    pub async fn get_cached_include_parse(
        &self,
        include_path: &Path,
    ) -> Option<(Arc<String>, Tree)> {
        if let Some(entry) = self.include_parse_cache.get(include_path) {
            return Some((entry.text.clone(), entry.tree.clone()));
        }

        let include_text = tokio::fs::read_to_string(include_path).await.ok()?;
        let mut parser = self.new_abl_parser();
        let include_tree = parser.parse(include_text.as_str(), None)?;
        let text = Arc::new(include_text);
        self.include_parse_cache.insert(
            include_path.to_path_buf(),
            IncludeParseCacheEntry {
                text: text.clone(),
                tree: include_tree.clone(),
            },
        );
        Some((text, include_tree))
    }

    pub async fn ambient_paths(&self) -> Vec<PathBuf> {
        let workspace_root = self.workspace_root.lock().await.clone();
        let ambient = self.config.lock().await.ambient.clone();
        let mut out = collect_configured_ambient_paths(workspace_root.as_deref(), &ambient.paths);
        if ambient.builtin {
            out.extend(self.ensure_embedded_ambient_paths().await);
        }
        dedup_paths_preserve_order(out)
    }

    async fn ensure_embedded_ambient_paths(&self) -> Vec<PathBuf> {
        let mut guard = self.embedded_ambient_paths.lock().await;
        if let Some(paths) = guard.as_ref() {
            return paths.clone();
        }

        let base_dir = std::env::temp_dir()
            .join("abl-language-server")
            .join("embedded-ambient");
        let mut paths = Vec::new();

        for file in crate::embedded_ambient::EMBEDDED_AMBIENT_FILES {
            let relative = file
                .relative_path
                .strip_prefix("ambient/")
                .unwrap_or(file.relative_path);
            let path = base_dir.join(relative);
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&path, file.contents);

            let mut parser = self.new_abl_parser();
            let Some(tree) = parser.parse(file.contents, None) else {
                continue;
            };
            self.include_parse_cache.insert(
                path.clone(),
                IncludeParseCacheEntry {
                    text: Arc::new(file.contents.to_string()),
                    tree,
                },
            );
            paths.push(path);
        }

        paths.sort();
        paths.dedup();
        *guard = Some(paths.clone());
        paths
    }

    pub fn invalidate_include_caches_for_uri(&self, uri: &Url) {
        let Ok(path) = uri.to_file_path() else {
            return;
        };
        self.include_completion_cache.remove(&path);
        self.include_parse_cache.remove(&path);
    }

    async fn reload_db_tables(&self, workspace_root: Option<&Path>, dumpfiles: &[String]) {
        let mut tables = HashSet::<String>::new();
        let mut sequences = HashSet::<String>::new();
        let mut table_labels = HashMap::<String, String>::new();
        let mut definitions = HashMap::<String, Vec<Location>>::new();
        let mut sequence_definitions = HashMap::<String, Vec<Location>>::new();
        let mut field_definitions = HashMap::<String, Vec<Location>>::new();
        let mut index_definitions = HashMap::<String, Vec<Location>>::new();
        let mut indexes_by_table = HashMap::<String, Vec<String>>::new();
        let mut index_fields_by_table_index = HashMap::<String, Vec<String>>::new();
        let mut fields_by_table = HashMap::<String, Vec<DbFieldInfo>>::new();
        let mut resolved_dumpfiles = Vec::<PathBuf>::new();
        for dumpfile in dumpfiles {
            resolved_dumpfiles.extend(expand_dumpfile_pattern(workspace_root, dumpfile));
        }
        resolved_dumpfiles.sort();
        resolved_dumpfiles.dedup();
        for path in resolved_dumpfiles {
            let Ok(contents) = tokio::fs::read_to_string(&path).await else {
                continue;
            };

            let tree = {
                let mut parser = self.df_parser.lock().await;
                parser.parse(&contents, None)
            };
            let Some(tree) = tree else {
                continue;
            };

            crate::analysis::df::collect_df_table_names(
                tree.root_node(),
                contents.as_bytes(),
                &mut tables,
            );
            let Some(uri) = Url::from_file_path(&path).ok() else {
                continue;
            };
            let mut sites = Vec::new();
            crate::analysis::df::collect_df_table_sites(
                tree.root_node(),
                contents.as_bytes(),
                &mut sites,
            );
            for site in sites {
                let key = site.name.to_ascii_uppercase();
                tables.insert(key.clone());
                table_labels.entry(key.clone()).or_insert(site.name);
                definitions.entry(key).or_default().push(Location {
                    uri: uri.clone(),
                    range: site.range,
                });
            }

            let mut sequence_sites = Vec::new();
            crate::analysis::df::collect_df_sequence_sites(
                tree.root_node(),
                contents.as_bytes(),
                &mut sequence_sites,
            );
            for site in sequence_sites {
                let key = site.name.to_ascii_uppercase();
                sequences.insert(key.clone());
                sequence_definitions.entry(key).or_default().push(Location {
                    uri: uri.clone(),
                    range: site.range,
                });
            }

            let mut field_sites = Vec::new();
            crate::analysis::df::collect_df_field_sites(
                tree.root_node(),
                contents.as_bytes(),
                &mut field_sites,
            );
            for site in field_sites {
                field_definitions
                    .entry(site.name.to_ascii_uppercase())
                    .or_default()
                    .push(Location {
                        uri: uri.clone(),
                        range: site.range,
                    });
            }

            let mut table_fields = Vec::new();
            crate::analysis::df::collect_df_table_fields(
                tree.root_node(),
                contents.as_bytes(),
                &mut table_fields,
            );
            for pair in table_fields {
                fields_by_table
                    .entry(pair.table.to_ascii_uppercase())
                    .or_default()
                    .push(DbFieldInfo {
                        name: pair.field,
                        field_type: pair.field_type,
                        format: pair.format,
                        label: pair.label,
                        description: pair.description,
                    });
            }

            let mut index_sites = Vec::new();
            crate::analysis::df::collect_df_index_sites(
                tree.root_node(),
                contents.as_bytes(),
                &mut index_sites,
            );
            for site in index_sites {
                index_definitions
                    .entry(site.name.to_ascii_uppercase())
                    .or_default()
                    .push(Location {
                        uri: uri.clone(),
                        range: site.range,
                    });
            }

            let mut table_indexes = Vec::new();
            crate::analysis::df::collect_df_table_indexes(
                tree.root_node(),
                contents.as_bytes(),
                &mut table_indexes,
            );
            for pair in table_indexes {
                let table_upper = pair.table.to_ascii_uppercase();
                let index_upper = pair.index.to_ascii_uppercase();
                indexes_by_table
                    .entry(table_upper.clone())
                    .or_default()
                    .push(pair.index.clone());
                index_fields_by_table_index
                    .insert(format!("{table_upper}\u{1f}{index_upper}"), pair.fields);
            }
        }

        self.db_tables.clear();
        for table in tables {
            self.db_tables.insert(table);
        }
        self.db_sequences.clear();
        for sequence in sequences {
            self.db_sequences.insert(sequence);
        }
        self.db_table_definitions.clear();
        for (k, v) in definitions {
            self.db_table_definitions.insert(k, v);
        }
        self.db_sequence_definitions.clear();
        for (k, v) in sequence_definitions {
            self.db_sequence_definitions.insert(k, v);
        }
        self.db_table_labels.clear();
        for (k, v) in table_labels {
            self.db_table_labels.insert(k, v);
        }
        self.db_field_definitions.clear();
        for (k, v) in field_definitions {
            self.db_field_definitions.insert(k, v);
        }
        self.db_index_definitions.clear();
        for (k, v) in index_definitions {
            self.db_index_definitions.insert(k, v);
        }
        for indexes in indexes_by_table.values_mut() {
            indexes.sort_by(|a, b| {
                a.to_ascii_uppercase()
                    .cmp(&b.to_ascii_uppercase())
                    .then(a.cmp(b))
            });
            indexes.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        }
        self.db_indexes_by_table.clear();
        for (k, v) in indexes_by_table {
            self.db_indexes_by_table.insert(k, v);
        }
        self.db_index_fields_by_table_index.clear();
        for (k, v) in index_fields_by_table_index {
            self.db_index_fields_by_table_index.insert(k, v);
        }
        for fields in fields_by_table.values_mut() {
            fields.sort_by(|a, b| {
                a.name
                    .to_ascii_uppercase()
                    .cmp(&b.name.to_ascii_uppercase())
                    .then(a.name.cmp(&b.name))
            });
            fields.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
        }
        self.db_fields_by_table.clear();
        for (k, v) in fields_by_table {
            self.db_fields_by_table.insert(k, v);
        }
        debug!(
            "loaded schema from dumpfile(s): tables={}, sequences={}, fields={}, indexes={}, table_field_sets={}",
            self.db_tables.len(),
            self.db_sequences.len(),
            self.db_field_definitions.len(),
            self.db_index_definitions.len(),
            self.db_fields_by_table.len()
        );
    }

    async fn reload_db_tables_from_current_config(&self) {
        let workspace_root = self.workspace_root.lock().await.clone();
        let dumpfiles = self.config.lock().await.dumpfile.clone();
        self.reload_db_tables(workspace_root.as_deref(), &dumpfiles)
            .await;
    }

    async fn is_configured_dumpfile_uri(&self, uri: &Url) -> bool {
        let Ok(uri_path) = uri.to_file_path() else {
            return false;
        };

        let workspace_root = self.workspace_root.lock().await.clone();
        let dumpfiles = self.config.lock().await.dumpfile.clone();
        dumpfiles
            .iter()
            .flat_map(|dumpfile| expand_dumpfile_pattern(workspace_root.as_deref(), dumpfile))
            .any(|path| path == uri_path)
    }
}

fn is_abl_toml_uri(uri: &Url) -> bool {
    uri.to_file_path()
        .ok()
        .and_then(|path| path.file_name().map(|name| name == "abl.toml"))
        .unwrap_or(false)
}

fn collect_configured_ambient_paths(
    workspace_root: Option<&Path>,
    ambient: &[String],
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in ambient {
        if entry.contains('*') {
            let Some((parent, file_name_pattern)) = resolve_glob_pattern(workspace_root, entry)
            else {
                continue;
            };
            let Ok(read_dir) = std::fs::read_dir(&parent) else {
                continue;
            };
            let mut matches = read_dir
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .map(|name| crate::utils::paths::wildcard_match(&file_name_pattern, name))
                        .unwrap_or(false)
                })
                .filter(|path| path.is_file())
                .collect::<Vec<_>>();
            matches.sort();
            out.extend(matches);
        } else if let Some(path) = resolve_config_path(workspace_root, entry) {
            out.push(path);
        }
    }
    out
}

fn dedup_paths_preserve_order(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for path in paths {
        if seen.insert(path.clone()) {
            out.push(path);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Backend, BackendState};
    use crate::config::AblConfig;
    use dashmap::{DashMap, DashSet};
    use std::sync::Arc;
    use tokio::sync::Mutex as AsyncMutex;
    use tower_lsp::{Client, LspService};

    fn test_backend() -> Backend {
        let (service, _socket) = LspService::build(|client: Client| Backend {
            client,
            state: Arc::new(BackendState {
                abl_language: tree_sitter_abl::LANGUAGE.into(),
                df_parser: AsyncMutex::new({
                    let mut p = tree_sitter::Parser::new();
                    p.set_language(&tree_sitter_df::LANGUAGE.into())
                        .expect("set df language");
                    p
                }),
                documents: DashMap::new(),
                workspace_root: AsyncMutex::new(None),
                config: AsyncMutex::new(AblConfig::default()),
                db_tables: DashSet::new(),
                db_sequences: DashSet::new(),
                db_table_labels: DashMap::new(),
                db_table_definitions: DashMap::new(),
                db_sequence_definitions: DashMap::new(),
                db_field_definitions: DashMap::new(),
                db_index_definitions: DashMap::new(),
                db_indexes_by_table: DashMap::new(),
                db_index_fields_by_table_index: DashMap::new(),
                db_fields_by_table: DashMap::new(),
                include_completion_cache: DashMap::new(),
                include_parse_cache: DashMap::new(),
                embedded_ambient_paths: AsyncMutex::new(None),
            }),
        })
        .finish();
        let backend = service.inner().clone();
        drop(service);
        backend
    }

    #[tokio::test]
    async fn ambient_paths_include_configured_globs_and_embedded_defaults() {
        let base = std::env::temp_dir().join(format!(
            "abl_ls_ambient_glob_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("epoch")
                .as_nanos()
        ));
        let ambient_dir = base.join("ambient");
        std::fs::create_dir_all(&ambient_dir).expect("create ambient dir");
        std::fs::write(ambient_dir.join("a.i"), "/* a */").expect("write a");
        std::fs::write(ambient_dir.join("b.i"), "/* b */").expect("write b");
        std::fs::write(ambient_dir.join("c.p"), "/* c */").expect("write c");

        let backend = test_backend();
        {
            let mut workspace_root = backend.workspace_root.lock().await;
            *workspace_root = Some(base.clone());
        }
        {
            let mut config = backend.config.lock().await;
            config.ambient.paths = vec!["ambient/*.i".to_string()];
        }

        let paths = backend.ambient_paths().await;
        assert!(paths.contains(&ambient_dir.join("a.i")));
        assert!(paths.contains(&ambient_dir.join("b.i")));
        assert!(
            paths
                .iter()
                .any(|path| path.file_name().and_then(|name| name.to_str()) == Some("index.i"))
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn ambient_paths_can_disable_embedded_defaults() {
        let backend = test_backend();
        {
            let mut config = backend.config.lock().await;
            config.ambient.builtin = false;
        }

        let paths = backend.ambient_paths().await;
        assert!(paths.is_empty());
    }
}

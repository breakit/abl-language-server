//! Server configuration from `abl.toml`.
//!
//! The ecosystem's single config schema is `.abl.toml`; this server reuses the
//! `[fmt]` section from `abl-config` (for formatting) and adds WebSpeed/LSP
//! specific top-level sections that mirror the reference server's layout:
//! `[diagnostics]`, `[completion]`, `[formatting]`, `[semantic_tokens]`.
//! A missing file or missing keys fall back to defaults.

use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct DiagnosticsConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Report unknown variable references against the document symbol index.
    #[serde(default = "default_true")]
    pub unknown_variables: bool,
    /// Report unknown function calls against the document symbol index.
    #[serde(default = "default_true")]
    pub unknown_functions: bool,
    /// Shell out to the `abl-lint` binary on save and publish its findings.
    #[serde(default = "default_true")]
    pub lint: bool,
    /// `abl-lint` binary path; defaults to `abl-lint` on PATH.
    #[serde(default)]
    pub lint_binary: Option<String>,
}

impl Default for DiagnosticsConfig {
    fn default() -> Self {
        DiagnosticsConfig {
            enabled: true,
            unknown_variables: true,
            unknown_functions: true,
            lint: true,
            lint_binary: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CompletionConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Offer ABL keywords in SpeedScript sections.
    #[serde(default = "default_true")]
    pub keywords: bool,
}

impl Default for CompletionConfig {
    fn default() -> Self {
        CompletionConfig {
            enabled: true,
            keywords: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct FormattingConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Run the formatter twice and only apply the result if both passes agree.
    #[serde(default = "default_true")]
    pub idempotence: bool,
}

impl Default for FormattingConfig {
    fn default() -> Self {
        FormattingConfig {
            enabled: true,
            idempotence: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SemanticTokensConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for SemanticTokensConfig {
    fn default() -> Self {
        SemanticTokensConfig { enabled: true }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct LspConfig {
    #[serde(default)]
    pub diagnostics: DiagnosticsConfig,
    #[serde(default)]
    pub completion: CompletionConfig,
    #[serde(default)]
    pub formatting: FormattingConfig,
    #[serde(default)]
    pub semantic_tokens: SemanticTokensConfig,
    /// Include search roots for `{...}` references (future).
    #[serde(default)]
    pub propath: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub lsp: LspConfig,
    pub fmt: abl_config::FmtOpts,
    /// Directory of the config file, used to resolve relative paths.
    pub base_dir: PathBuf,
    /// Path of the loaded `abl.toml`, if any.
    pub source: Option<PathBuf>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            lsp: LspConfig::default(),
            fmt: abl_config::FmtOpts::default(),
            base_dir: PathBuf::from("."),
            source: None,
        }
    }
}

fn default_true() -> bool {
    true
}

fn find_config(root: &Path) -> Option<PathBuf> {
    for dir in [root.to_path_buf(), root.join(".config")] {
        let candidate = dir.join("abl.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Loads server config from `<root>/abl.toml` (or `.config/abl.toml`),
/// falling back to defaults when absent or unreadable.
pub fn load(root: &Path) -> ServerConfig {
    let mut cfg = ServerConfig {
        base_dir: root.to_path_buf(),
        ..ServerConfig::default()
    };
    let Some(path) = find_config(root) else {
        return cfg;
    };
    let contents = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("abl.toml unreadable at {}: {e}", path.display());
            return cfg;
        }
    };
    let shared = abl_config::load(&path);
    match toml::from_str::<LspConfig>(&contents) {
        Ok(lsp) => {
            cfg.lsp = lsp;
            cfg.fmt = shared.fmt.clone().validate().unwrap_or_else(|e| {
                log::warn!("invalid [fmt] in {}: {e}", path.display());
                shared.fmt
            });
            cfg.source = Some(path);
        }
        Err(e) => log::warn!("invalid abl.toml at {}: {e}", path.display()),
    }
    cfg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_webspeed_sections_and_reuses_fmt() {
        let dir = std::env::temp_dir().join("wls-config-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("abl.toml");
        std::fs::write(
            &path,
            r#"
propath = ["includes"]

[fmt]
print_width = 100

[diagnostics]
lint_binary = "/opt/abl/bin/abl-lint"
unknown_variables = false

[formatting]
enabled = false
"#,
        )
        .unwrap();
        let cfg = load(&dir);
        assert_eq!(
            cfg.lsp.diagnostics.lint_binary.as_deref(),
            Some("/opt/abl/bin/abl-lint")
        );
        assert!(!cfg.lsp.diagnostics.unknown_variables);
        assert!(cfg.lsp.diagnostics.unknown_functions);
        assert!(!cfg.lsp.formatting.enabled);
        assert_eq!(cfg.fmt.print_width, 100);
        assert_eq!(cfg.lsp.propath, vec!["includes"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn defaults_on_missing_config() {
        let dir = std::env::temp_dir().join("wls-config-missing");
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = load(&dir);
        assert!(cfg.lsp.diagnostics.enabled);
        assert!(cfg.lsp.formatting.enabled);
        assert!(cfg.source.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

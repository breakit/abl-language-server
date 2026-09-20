use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::path::Component;
use std::path::{Path, PathBuf};
use tower_lsp::lsp_types::InitializeParams;

use crate::analysis::formatting::{FormatterOptions, LineEnding};

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct AblConfig {
    pub completion: CompletionConfig,
    pub diagnostics: DiagnosticsConfig,
    pub formatting: FormattingConfig,
    pub semantic_tokens: SemanticTokensConfig,
    #[serde(default, deserialize_with = "deserialize_dumpfile")]
    pub dumpfile: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_ambient_config")]
    pub ambient: AmbientConfig,
    #[serde(default, deserialize_with = "deserialize_propath")]
    pub propath: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AmbientConfig {
    pub builtin: bool,
    #[serde(default, deserialize_with = "deserialize_string_or_vec")]
    pub paths: Vec<String>,
}

impl Default for AmbientConfig {
    fn default() -> Self {
        Self {
            builtin: true,
            paths: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CompletionConfig {
    pub enabled: bool,
}

impl Default for CompletionConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DiagnosticsConfig {
    pub enabled: bool,
    pub unknown_variables: DiagnosticFeatureConfig,
    pub unknown_functions: DiagnosticFeatureConfig,
}

impl Default for DiagnosticsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            unknown_variables: DiagnosticFeatureConfig::default(),
            unknown_functions: DiagnosticFeatureConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DiagnosticFeatureConfig {
    pub enabled: bool,
    #[serde(default, deserialize_with = "deserialize_string_or_vec")]
    pub exclude: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_string_or_vec")]
    pub ignore: Vec<String>,
}

impl Default for DiagnosticFeatureConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            exclude: Vec::new(),
            ignore: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct FormattingConfig {
    pub enabled: bool,
    pub indent_size: usize,
    pub use_tabs: bool,
    pub trim_trailing_whitespace: bool,
    pub insert_final_newline: bool,
    pub trim_final_newlines: bool,
    pub max_consecutive_blank_lines: usize,
    pub line_ending: LineEnding,
    pub line_width: usize,
    pub idempotence: bool,
}

impl Default for FormattingConfig {
    fn default() -> Self {
        let options = FormatterOptions::default();
        Self {
            enabled: false,
            indent_size: options.indent_size,
            use_tabs: options.use_tabs,
            trim_trailing_whitespace: options.trim_trailing_whitespace,
            insert_final_newline: options.insert_final_newline,
            trim_final_newlines: options.trim_final_newlines,
            max_consecutive_blank_lines: options.max_consecutive_blank_lines,
            line_ending: options.line_ending,
            line_width: options.line_width,
            idempotence: true,
        }
    }
}

impl FormattingConfig {
    pub fn formatter_options(&self) -> FormatterOptions {
        FormatterOptions {
            indent_size: self.indent_size,
            use_tabs: self.use_tabs,
            trim_trailing_whitespace: self.trim_trailing_whitespace,
            insert_final_newline: self.insert_final_newline,
            trim_final_newlines: self.trim_final_newlines,
            max_consecutive_blank_lines: self.max_consecutive_blank_lines,
            line_ending: self.line_ending,
            line_width: self.line_width,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SemanticTokensConfig {
    pub enabled: bool,
}

impl Default for SemanticTokensConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone)]
pub struct LoadedAblConfig {
    pub config: AblConfig,
    pub path: Option<PathBuf>,
}

pub fn find_workspace_root(params: &InitializeParams) -> Option<PathBuf> {
    if let Some(folders) = &params.workspace_folders {
        for folder in folders {
            if let Ok(path) = folder.uri.to_file_path() {
                return Some(path);
            }
        }
    }

    if let Some(root_uri) = &params.root_uri
        && let Ok(path) = root_uri.to_file_path()
    {
        return Some(path);
    }

    None
}

pub async fn load_from_workspace_root(root: Option<&Path>) -> LoadedAblConfig {
    let Some(root) = root else {
        return LoadedAblConfig {
            config: AblConfig::default(),
            path: None,
        };
    };

    let path = root.join("abl.toml");
    match tokio::fs::read_to_string(&path).await {
        Ok(contents) => match toml::from_str::<PartialAblConfig>(&contents) {
            Ok(root_partial) => LoadedAblConfig {
                config: load_with_inheritance(&path, root_partial).await,
                path: Some(path),
            },
            Err(_) => LoadedAblConfig {
                config: AblConfig::default(),
                path: Some(path),
            },
        },
        Err(err) if err.kind() == ErrorKind::NotFound => LoadedAblConfig {
            config: AblConfig::default(),
            path: Some(path),
        },
        Err(_) => LoadedAblConfig {
            config: AblConfig::default(),
            path: Some(path),
        },
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct PartialAblConfig {
    #[serde(default, deserialize_with = "deserialize_optional_string_or_vec")]
    inherits: Option<Vec<String>>,
    completion: Option<PartialCompletionConfig>,
    diagnostics: Option<PartialDiagnosticsConfig>,
    formatting: Option<PartialFormattingConfig>,
    semantic_tokens: Option<PartialSemanticTokensConfig>,
    #[serde(default, deserialize_with = "deserialize_optional_string_or_vec")]
    dumpfile: Option<Vec<String>>,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_partial_ambient_config"
    )]
    ambient: Option<PartialAmbientConfig>,
    #[serde(default, deserialize_with = "deserialize_optional_string_or_vec")]
    propath: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct PartialCompletionConfig {
    enabled: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct PartialDiagnosticsConfig {
    enabled: Option<bool>,
    unknown_variables: Option<PartialDiagnosticFeatureConfig>,
    unknown_functions: Option<PartialDiagnosticFeatureConfig>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct PartialDiagnosticFeatureConfig {
    enabled: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_optional_string_or_vec")]
    exclude: Option<Vec<String>>,
    #[serde(default, deserialize_with = "deserialize_optional_string_or_vec")]
    ignore: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct PartialFormattingConfig {
    enabled: Option<bool>,
    indent_size: Option<usize>,
    use_tabs: Option<bool>,
    trim_trailing_whitespace: Option<bool>,
    insert_final_newline: Option<bool>,
    trim_final_newlines: Option<bool>,
    max_consecutive_blank_lines: Option<usize>,
    line_ending: Option<LineEnding>,
    line_width: Option<usize>,
    idempotence: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct PartialSemanticTokensConfig {
    enabled: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct PartialAmbientConfig {
    builtin: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_optional_string_or_vec")]
    paths: Option<Vec<String>>,
}

async fn load_with_inheritance(path: &Path, root_partial: PartialAblConfig) -> AblConfig {
    let root_identity = path_identity(path);
    let mut partials = HashMap::<PathBuf, PartialAblConfig>::new();
    partials.insert(root_identity.clone(), root_partial);

    let mut visited = HashSet::<PathBuf>::new();
    let mut visiting = HashSet::<PathBuf>::new();
    let mut order = Vec::<PathBuf>::new();
    let mut stack = vec![(root_identity, false)];

    while let Some((current, exit)) = stack.pop() {
        if exit {
            visiting.remove(&current);
            visited.insert(current.clone());
            order.push(current);
            continue;
        }

        if visited.contains(&current) || visiting.contains(&current) {
            continue;
        }
        visiting.insert(current.clone());

        let current_partial = if let Some(cfg) = partials.get(&current).cloned() {
            cfg
        } else {
            match read_partial_config(&current).await {
                Some(cfg) => {
                    partials.insert(current.clone(), cfg.clone());
                    cfg
                }
                None => {
                    visiting.remove(&current);
                    visited.insert(current);
                    continue;
                }
            }
        };

        stack.push((current.clone(), true));

        if let Some(inherits) = current_partial.inherits {
            for inherited in inherits.iter().rev() {
                let inherited_path = resolve_inherited_path(&current, inherited);
                let inherited_identity = path_identity(&inherited_path);
                if visited.contains(&inherited_identity) || visiting.contains(&inherited_identity) {
                    continue;
                }

                if let std::collections::hash_map::Entry::Vacant(entry) =
                    partials.entry(inherited_identity.clone())
                    && let Some(cfg) = read_partial_config(&inherited_identity).await
                {
                    entry.insert(cfg);
                    stack.push((inherited_identity, false));
                }
            }
        }
    }

    let mut merged = AblConfig::default();
    for config_path in order {
        if let Some(partial) = partials.get(&config_path) {
            merge_partial_into(&mut merged, partial, &config_path);
        }
    }
    merged
}

async fn read_partial_config(path: &Path) -> Option<PartialAblConfig> {
    let contents = tokio::fs::read_to_string(path).await.ok()?;
    toml::from_str::<PartialAblConfig>(&contents).ok()
}

fn resolve_inherited_path(current_config_path: &Path, inherited: &str) -> PathBuf {
    let inherited_path = PathBuf::from(inherited);
    if inherited_path.is_absolute() {
        inherited_path
    } else {
        current_config_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(inherited_path)
    }
}

fn path_identity(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn merge_partial_into(base: &mut AblConfig, partial: &PartialAblConfig, config_path: &Path) {
    if let Some(completion) = &partial.completion
        && let Some(enabled) = completion.enabled
    {
        base.completion.enabled = enabled;
    }

    if let Some(diagnostics) = &partial.diagnostics {
        if let Some(enabled) = diagnostics.enabled {
            base.diagnostics.enabled = enabled;
        }
        if let Some(unknown_variables) = &diagnostics.unknown_variables {
            if let Some(enabled) = unknown_variables.enabled {
                base.diagnostics.unknown_variables.enabled = enabled;
            }
            if let Some(exclude) = &unknown_variables.exclude {
                base.diagnostics.unknown_variables.exclude =
                    resolve_path_list_relative_to_config(config_path, exclude);
            }
            if let Some(ignore) = &unknown_variables.ignore {
                base.diagnostics.unknown_variables.ignore = ignore.clone();
            }
        }
        if let Some(unknown_functions) = &diagnostics.unknown_functions {
            if let Some(enabled) = unknown_functions.enabled {
                base.diagnostics.unknown_functions.enabled = enabled;
            }
            if let Some(exclude) = &unknown_functions.exclude {
                base.diagnostics.unknown_functions.exclude =
                    resolve_path_list_relative_to_config(config_path, exclude);
            }
            if let Some(ignore) = &unknown_functions.ignore {
                base.diagnostics.unknown_functions.ignore = ignore.clone();
            }
        }
    }

    if let Some(formatting) = &partial.formatting {
        if let Some(enabled) = formatting.enabled {
            base.formatting.enabled = enabled;
        }
        if let Some(indent_size) = formatting.indent_size {
            base.formatting.indent_size = indent_size;
        }
        if let Some(use_tabs) = formatting.use_tabs {
            base.formatting.use_tabs = use_tabs;
        }
        if let Some(trim_trailing_whitespace) = formatting.trim_trailing_whitespace {
            base.formatting.trim_trailing_whitespace = trim_trailing_whitespace;
        }
        if let Some(insert_final_newline) = formatting.insert_final_newline {
            base.formatting.insert_final_newline = insert_final_newline;
        }
        if let Some(trim_final_newlines) = formatting.trim_final_newlines {
            base.formatting.trim_final_newlines = trim_final_newlines;
        }
        if let Some(max_consecutive_blank_lines) = formatting.max_consecutive_blank_lines {
            base.formatting.max_consecutive_blank_lines = max_consecutive_blank_lines;
        }
        if let Some(line_ending) = formatting.line_ending {
            base.formatting.line_ending = line_ending;
        }
        if let Some(line_width) = formatting.line_width {
            base.formatting.line_width = line_width;
        }
        if let Some(idempotence) = formatting.idempotence {
            base.formatting.idempotence = idempotence;
        }
    }

    if let Some(semantic_tokens) = &partial.semantic_tokens
        && let Some(enabled) = semantic_tokens.enabled
    {
        base.semantic_tokens.enabled = enabled;
    }

    if let Some(dumpfile) = &partial.dumpfile {
        base.dumpfile
            .extend(resolve_path_list_relative_to_config(config_path, dumpfile));
    }
    if let Some(ambient) = &partial.ambient {
        if let Some(builtin) = ambient.builtin {
            base.ambient.builtin = builtin;
        }
        if let Some(paths) = &ambient.paths {
            base.ambient
                .paths
                .extend(resolve_path_list_relative_to_config(config_path, paths));
        }
    }
    if let Some(propath) = &partial.propath {
        for resolved in resolve_path_list_relative_to_config(config_path, propath) {
            push_unique_string_value(&mut base.propath, resolved);
        }
    }

    let config_dir = config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    push_unique_string_path(&mut base.propath, &config_dir);
}

fn resolve_path_list_relative_to_config(config_path: &Path, values: &[String]) -> Vec<String> {
    let base_dir = config_path.parent().unwrap_or_else(|| Path::new("."));
    values
        .iter()
        .map(|value| {
            let candidate = PathBuf::from(value);
            let resolved = if candidate.is_absolute() {
                candidate
            } else {
                base_dir.join(candidate)
            };
            normalize_path_lexical(resolved)
                .to_string_lossy()
                .to_string()
        })
        .collect()
}

fn normalize_path_lexical(path: PathBuf) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        if component == Component::CurDir {
            continue;
        }
        out.push(component.as_os_str());
    }
    out
}

fn push_unique_string_path(out: &mut Vec<String>, path: &Path) {
    push_unique_string_value(out, path.to_string_lossy().to_string());
}

fn push_unique_string_value(out: &mut Vec<String>, value: String) {
    if out.iter().any(|existing| existing == &value) {
        return;
    }
    out.push(value);
}

fn deserialize_dumpfile<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_optional_string_or_vec(deserializer).map(|v| v.unwrap_or_default())
}

fn deserialize_propath<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_optional_string_or_vec(deserializer).map(|v| v.unwrap_or_default())
}

fn deserialize_string_or_vec<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_optional_string_or_vec(deserializer).map(|v| v.unwrap_or_default())
}

fn deserialize_optional_string_or_vec<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrVec {
        Single(String),
        Multiple(Vec<String>),
    }

    let parsed = Option::<StringOrVec>::deserialize(deserializer)?;
    Ok(parsed.map(|v| match v {
        StringOrVec::Single(path) => vec![path],
        StringOrVec::Multiple(paths) => paths,
    }))
}

fn deserialize_ambient_config<'de, D>(deserializer: D) -> Result<AmbientConfig, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_optional_partial_ambient_config(deserializer)
        .map(|partial| partial.map(AmbientConfig::from).unwrap_or_default())
}

fn deserialize_optional_partial_ambient_config<'de, D>(
    deserializer: D,
) -> Result<Option<PartialAmbientConfig>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<PartialAmbientConfig>::deserialize(deserializer)
}

impl From<PartialAmbientConfig> for AmbientConfig {
    fn from(value: PartialAmbientConfig) -> Self {
        Self {
            builtin: value.builtin.unwrap_or(true),
            paths: value.paths.unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AblConfig, load_from_workspace_root};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn parses_dumpfile_and_propath_as_single_string() {
        let cfg: AblConfig = toml::from_str(
            r#"
dumpfile = "database.df"
propath = "src/includes"

[ambient]
paths = "ambient/docs.i"
"#,
        )
        .expect("parse config");

        assert_eq!(cfg.dumpfile, vec!["database.df"]);
        assert!(cfg.ambient.builtin);
        assert_eq!(cfg.ambient.paths, vec!["ambient/docs.i"]);
        assert_eq!(cfg.propath, vec!["src/includes"]);
    }

    #[test]
    fn parses_dumpfile_and_propath_as_arrays() {
        let cfg: AblConfig = toml::from_str(
            r#"
dumpfile = ["a.df", "b.df"]
propath = ["/global/a", "relative/includes"]

[ambient]
builtin = false
paths = ["/global/docs.i", "relative/ambient.i"]
"#,
        )
        .expect("parse config");

        assert_eq!(cfg.dumpfile, vec!["a.df", "b.df"]);
        assert!(!cfg.ambient.builtin);
        assert_eq!(
            cfg.ambient.paths,
            vec!["/global/docs.i", "relative/ambient.i"]
        );
        assert_eq!(cfg.propath, vec!["/global/a", "relative/includes"]);
    }

    #[test]
    fn parses_formatting_settings() {
        let cfg: AblConfig = toml::from_str(
            r#"
[formatting]
enabled = true
indent_size = 4
use_tabs = false
trim_trailing_whitespace = false
insert_final_newline = false
trim_final_newlines = false
max_consecutive_blank_lines = 2
line_ending = "crlf"
line_width = 100
idempotence = false
"#,
        )
        .expect("parse config");

        assert!(cfg.formatting.enabled);
        assert_eq!(cfg.formatting.indent_size, 4);
        assert!(!cfg.formatting.use_tabs);
        assert!(!cfg.formatting.trim_trailing_whitespace);
        assert!(!cfg.formatting.insert_final_newline);
        assert!(!cfg.formatting.trim_final_newlines);
        assert_eq!(cfg.formatting.max_consecutive_blank_lines, 2);
        assert_eq!(cfg.formatting.line_ending, super::LineEnding::Crlf);
        assert_eq!(cfg.formatting.line_width, 100);
        assert!(!cfg.formatting.idempotence);
    }

    #[test]
    fn uses_standard_formatter_line_width_by_default() {
        assert_eq!(AblConfig::default().formatting.line_width, 100);
    }

    #[test]
    fn parses_diagnostic_feature_excludes() {
        let cfg: AblConfig = toml::from_str(
            r#"
[diagnostics.unknown_variables]
exclude = ["legacy/*.p", "tmp/**/*.p"]
ignore = ["BatchRun", "Today"]

[diagnostics.unknown_functions]
enabled = false
exclude = "special.p"
ignore = "custom_func"
"#,
        )
        .expect("parse config");

        assert_eq!(
            cfg.diagnostics.unknown_variables.exclude,
            vec!["legacy/*.p", "tmp/**/*.p"]
        );
        assert_eq!(
            cfg.diagnostics.unknown_variables.ignore,
            vec!["BatchRun", "Today"]
        );
        assert!(!cfg.diagnostics.unknown_functions.enabled);
        assert_eq!(cfg.diagnostics.unknown_functions.exclude, vec!["special.p"]);
        assert_eq!(
            cfg.diagnostics.unknown_functions.ignore,
            vec!["custom_func"]
        );
    }

    #[tokio::test]
    async fn loads_inherited_config_and_applies_child_overrides() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let base_dir = std::env::temp_dir().join(format!("abl-ls-config-{ts}"));
        std::fs::create_dir_all(&base_dir).expect("create temp dir");

        let parent = base_dir.join("base.toml");
        let child = base_dir.join("abl.toml");

        std::fs::write(
            &parent,
            r#"
dumpfile = "parent.df"
propath = ["parent/includes"]

[ambient]
builtin = false
paths = ["parent/ambient.i"]

[completion]
enabled = false

[diagnostics]
enabled = false

[diagnostics.unknown_variables]
exclude = "./parent-excluded.p"
ignore = ["PARENT-GLOBAL"]
"#,
        )
        .expect("write parent config");

        std::fs::write(
            &child,
            r#"
inherits = "base.toml"
dumpfile = ["child.df"]
propath = ["child/includes"]

[ambient]
paths = ["child/ambient.i"]

[diagnostics.unknown_variables]
exclude = "./child-excluded.p"
ignore = ["CHILD-GLOBAL"]
"#,
        )
        .expect("write child config");

        let loaded = load_from_workspace_root(Some(&base_dir)).await;
        assert!(!loaded.config.completion.enabled);
        assert!(!loaded.config.diagnostics.enabled);
        assert_eq!(
            loaded.config.diagnostics.unknown_variables.ignore,
            vec!["CHILD-GLOBAL"]
        );
        assert_eq!(
            loaded.config.diagnostics.unknown_variables.exclude,
            vec![
                child
                    .parent()
                    .expect("child dir")
                    .join("child-excluded.p")
                    .to_string_lossy()
                    .to_string()
            ]
        );
        assert_eq!(
            loaded.config.dumpfile,
            vec![
                parent
                    .parent()
                    .expect("parent dir")
                    .join("parent.df")
                    .to_string_lossy()
                    .to_string(),
                child
                    .parent()
                    .expect("child dir")
                    .join("child.df")
                    .to_string_lossy()
                    .to_string(),
            ]
        );
        assert_eq!(
            loaded.config.ambient.paths,
            vec![
                parent
                    .parent()
                    .expect("parent dir")
                    .join("parent/ambient.i")
                    .to_string_lossy()
                    .to_string(),
                child
                    .parent()
                    .expect("child dir")
                    .join("child/ambient.i")
                    .to_string_lossy()
                    .to_string(),
            ]
        );
        assert!(!loaded.config.ambient.builtin);
        assert_eq!(
            loaded.config.propath,
            vec![
                parent
                    .parent()
                    .expect("parent dir")
                    .join("parent/includes")
                    .to_string_lossy()
                    .to_string(),
                parent
                    .parent()
                    .expect("parent dir")
                    .to_string_lossy()
                    .to_string(),
                child
                    .parent()
                    .expect("child dir")
                    .join("child/includes")
                    .to_string_lossy()
                    .to_string(),
            ]
        );

        let _ = std::fs::remove_dir_all(&base_dir);
    }
}

//! Diagnostic collection and publishing.
//!
//! Diagnostics come from three sources, all mapped into the original document:
//!   * syntax errors in SpeedScript sections (tree-sitter-abl ERROR/MISSING)
//!   * syntax errors in JavaScript sections (tree-sitter-javascript)
//!   * `abl-lint` binary findings (shelled out on save)
//!
//! Unknown-variable / unknown-function diagnostics live in `analysis`.

use std::process::Stdio;
use std::sync::Arc;

use tower_lsp::Client;
use tower_lsp::lsp_types::*;

use crate::analysis;
use crate::backend::BackendState;
use crate::document::{WebSpeedDocument, collect_error_nodes};
use crate::scanner::SectionKind;

pub fn source() -> &'static str {
    "webspeed"
}

/// Collects syntax + symbol diagnostics for a document (no lint shell-out).
pub fn collect(doc: &WebSpeedDocument) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for (idx, sec) in doc.sections().iter().enumerate() {
        if sec.kind == SectionKind::Html {
            continue;
        }
        let Some(tree) = doc.tree(idx) else { continue };
        let mut errors = Vec::new();
        collect_error_nodes(tree.root_node(), &mut errors);
        if errors.is_empty() {
            // Whole-section check; error collection above covers it.
        }
        for node in errors {
            let range = doc.node_range(idx, node);
            out.push(Diagnostic {
                range,
                severity: Some(DiagnosticSeverity::ERROR),
                source: Some(source().to_string()),
                message: if node.is_missing() {
                    "Missing token (syntax error)".to_string()
                } else {
                    "Syntax error".to_string()
                },
                ..Default::default()
            });
        }

        if sec.kind == SectionKind::SpeedScript {
            let analysis = analysis::analyze(doc, idx);
            if let Some(analysis) = analysis {
                for v in analysis.unknown_variables {
                    out.push(v.diagnostic(doc, idx));
                }
                for f in analysis.unknown_functions {
                    out.push(f.diagnostic(doc, idx));
                }
            }
        }
    }
    out
}

/// Runs the `abl-lint` binary on the document's file and publishes its
/// findings as diagnostics. Called on save.
pub async fn run_lint(
    client: &Client,
    uri: &Url,
    _doc: &WebSpeedDocument,
    state: &Arc<BackendState>,
) {
    let cfg = state.config.lock().await.clone();
    if !cfg.lsp.diagnostics.enabled || !cfg.lsp.diagnostics.lint {
        return;
    }
    let Some(path) = uri.to_file_path().ok() else {
        return;
    };
    let binary = cfg
        .lsp
        .diagnostics
        .lint_binary
        .as_deref()
        .unwrap_or("abl-lint");
    let output = tokio::process::Command::new(binary)
        .arg("check")
        .arg(&path)
        .current_dir(&cfg.base_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await;
    let Ok(output) = output else {
        log::debug!("abl-lint not runnable: {binary}");
        return;
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let diags = parse_lint_output(&stdout);
    if !diags.is_empty() {
        let _ = client.publish_diagnostics(uri.clone(), diags, None).await;
    }
}

/// Parses `path:line:col: rule [severity]: message` lines from `abl-lint`.
pub fn parse_lint_output(stdout: &str) -> Vec<Diagnostic> {
    stdout
        .lines()
        .filter_map(|line| {
            // "path:12:5: block_structure [error]: message here"
            let (head, message) = line.rsplit_once(": ")?;
            let (head, severity) = head.rsplit_once(' ')?;
            let severity = severity.trim_matches(|c| c == '[' || c == ']');
            let (loc, rule) = head.rsplit_once(": ")?;
            // loc is `path:LINE:COL`; split from the right gives col last.
            let (path_line, col) = loc.rsplit_once(':')?;
            let (_path, line) = path_line.rsplit_once(':')?;
            let line: u32 = line.trim().parse().ok()?;
            let col: u32 = col.trim().parse().ok()?;
            let severity = match severity {
                "error" => DiagnosticSeverity::ERROR,
                "warning" => DiagnosticSeverity::WARNING,
                _ => DiagnosticSeverity::INFORMATION,
            };
            Some(Diagnostic {
                range: Range {
                    start: Position {
                        line: line.saturating_sub(1),
                        character: col.saturating_sub(1),
                    },
                    end: Position {
                        line: line.saturating_sub(1),
                        character: col,
                    },
                },
                severity: Some(severity),
                source: Some("abl-lint".to_string()),
                code: Some(NumberOrString::String(rule.to_string())),
                message: message.to_string(),
                ..Default::default()
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lint_output_lines() {
        let out = "path.p:12:5: block_structure [error]: message here\n\
                   path.p:2:1: public_variable [warning]: other\n";
        let diags = parse_lint_output(out);
        assert_eq!(diags.len(), 2);
        assert_eq!(
            diags[0].range.start,
            Position {
                line: 11,
                character: 4
            }
        );
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(
            diags[0].code,
            Some(NumberOrString::String("block_structure".into()))
        );
        assert_eq!(diags[1].severity, Some(DiagnosticSeverity::WARNING));
    }

    #[test]
    fn ignores_non_matching_lines() {
        assert!(parse_lint_output("some random output\n").is_empty());
    }
}

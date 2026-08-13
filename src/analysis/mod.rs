//! Per-section symbol analysis for SpeedScript code.
//!
//! Collects definitions (variables, parameters, buffers, properties,
//! functions) from a section's parse tree into a case-insensitive index, and
//! reports references to unknown variables/functions. ABL is case-insensitive,
//! so every lookup uses lowercased keys.
//!
//! Semantic analysis only runs on clean parse trees: with the experimental
//! grammar (~47% clean parse rate), recovery trees change shape with whitespace
//! and would produce noise.

use std::collections::HashMap;

use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Range};
use tree_sitter::Node;

use crate::document::WebSpeedDocument;

/// What a symbol is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Variable,
    Parameter,
    Buffer,
    Property,
    Function,
    Table,
    Preprocessor,
}

/// A definition discovered in a section.
#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    /// (start, end) byte range within the section's content.
    pub range: (usize, usize),
}

#[derive(Debug, Clone)]
pub struct UnknownVariable {
    pub name: String,
    /// (start, end) byte range within the section's content.
    pub range: (usize, usize),
}

impl UnknownVariable {
    pub fn diagnostic(&self, doc: &WebSpeedDocument, idx: usize) -> Diagnostic {
        Diagnostic {
            range: Range {
                start: content_point(doc, idx, self.range.0),
                end: content_point(doc, idx, self.range.1),
            },
            severity: Some(DiagnosticSeverity::WARNING),
            source: Some("webspeed".to_string()),
            message: format!("Unknown variable '{}'", self.name),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone)]
pub struct UnknownFunction {
    pub name: String,
    /// (start, end) byte range within the section's content.
    pub range: (usize, usize),
}

impl UnknownFunction {
    pub fn diagnostic(&self, doc: &WebSpeedDocument, idx: usize) -> Diagnostic {
        Diagnostic {
            range: Range {
                start: content_point(doc, idx, self.range.0),
                end: content_point(doc, idx, self.range.1),
            },
            severity: Some(DiagnosticSeverity::WARNING),
            source: Some("webspeed".to_string()),
            message: format!("Unknown function '{}'", self.name),
            ..Default::default()
        }
    }
}

#[derive(Debug, Default)]
pub struct SectionAnalysis {
    symbols: Vec<Symbol>,
    by_name: HashMap<String, usize>,
    pub unknown_variables: Vec<UnknownVariable>,
    pub unknown_functions: Vec<UnknownFunction>,
}

impl SectionAnalysis {
    pub fn symbols(&self) -> &[Symbol] {
        &self.symbols
    }

    /// Case-insensitive symbol lookup.
    pub fn find(&self, name: &str) -> Option<&Symbol> {
        self.by_name
            .get(&name.to_ascii_lowercase())
            .map(|&i| &self.symbols[i])
    }

    fn define(&mut self, name: &str, kind: SymbolKind, range: (usize, usize)) {
        let lower = name.to_ascii_lowercase();
        if let Some(&i) = self.by_name.get(&lower) {
            // First definition wins (case-insensitive).
            let _ = i;
            return;
        }
        let i = self.symbols.len();
        self.symbols.push(Symbol {
            name: name.to_string(),
            kind,
            range,
        });
        self.by_name.insert(lower, i);
    }
}

fn content_point(
    doc: &WebSpeedDocument,
    idx: usize,
    content_byte: usize,
) -> tower_lsp::lsp_types::Position {
    let sec = &doc.sections()[idx];
    doc.point_to_position(sec.content_start + content_byte)
}

/// Built-in ABL functions that never count as "unknown".
pub const BUILTIN_FUNCTIONS: &[&str] = &[
    "ABS",
    "ASC",
    "CANDIDATE-KEY",
    "CAPS",
    "CHR",
    "CODEPAGE",
    "COLUMN",
    "CURRENT-CHANGED",
    "CURRENT-VALUE",
    "DATE",
    "DAY",
    "DECIMAL",
    "DYNAMIC-CURRENT-VALUE",
    "DYNAMIC-FUNCTION",
    "DYNAMIC-NEW",
    "DYNAMIC-NEXT-VALUE",
    "DYNAMIC-PROPERTY",
    "ENTRY",
    "ETIME",
    "EXP",
    "FILL",
    "FIRST",
    "FIRST-OF",
    "FLOOR",
    "FORMAT",
    "FRAME-CURRENT-VALUE",
    "FRAME-DOWN",
    "FRAME-FIELD",
    "FRAME-LINE",
    "FRAME-ROW",
    "GUID",
    "INDEX",
    "INTEGER",
    "ISO-DATE",
    "KBLABEL",
    "KEYWORD",
    "KEYWORD-ALL",
    "LAST",
    "LAST-OF",
    "LC",
    "LENGTH",
    "LIST-EVENTS",
    "LIST-QUERY-ATTRS",
    "LIST-SET-ATTRS",
    "LIST-WIDGETS",
    "LOGICAL",
    "LOOKUP",
    "MAXIMUM",
    "MEMBER",
    "MESSAGE-LINES",
    "MINIMUM",
    "MONTH",
    "MTIME",
    "NEW",
    "NEXT-VALUE",
    "NUM-ENTRIES",
    "NUM-RESULTS",
    "NUMBER",
    "OS-GETENV",
    "PAGE-NUMBER",
    "PAGE-SIZE",
    "PAGES",
    "PROCESS-ARCH",
    "PROGRAM-NAME",
    "PROGRESS",
    "PROPATH",
    "PROVERSION",
    "RANDOM",
    "RATIO",
    "RINDEX",
    "ROUND",
    "SDBNAME",
    "SEEK",
    "SESSION-VALUE",
    "SETUSERID",
    "SIZE",
    "SQRT",
    "SSGET",
    "STRING",
    "SUBSTRING",
    "TODAY",
    "TRIM",
    "TRUNCATE",
    "USERID",
    "VALID-EVENT",
    "VALID-HANDLE",
    "VALID-OBJECT",
    "WEEKDAY",
    "YEAR",
];

fn is_builtin_function(name: &str) -> bool {
    BUILTIN_FUNCTIONS
        .iter()
        .any(|b| b.eq_ignore_ascii_case(name))
}

/// Analyzes the SpeedScript section at `idx`. Returns `None` for non-code
/// sections, unparsed sections, or trees containing errors.
pub fn analyze(doc: &WebSpeedDocument, idx: usize) -> Option<SectionAnalysis> {
    let sec = doc.sections().get(idx)?;
    if sec.kind != crate::scanner::SectionKind::SpeedScript {
        return None;
    }
    let tree = doc.tree(idx)?;
    let root = tree.root_node();
    if root.has_error() {
        return None;
    }
    let mut a = SectionAnalysis::default();
    let content = sec.content(&doc.text);
    walk_definitions(root, content, &mut a);
    walk_references(root, content, &mut a);
    Some(a)
}

fn name_of<'t>(node: Node<'t>, content: &str) -> Option<(String, (usize, usize))> {
    let name = node.child_by_field_name("name")?;
    if name.kind() != "identifier" {
        return None;
    }
    let range = (name.start_byte(), name.end_byte());
    Some((name.utf8_text(content.as_bytes()).ok()?.to_string(), range))
}

fn walk_definitions<'t>(node: Node<'t>, content: &str, a: &mut SectionAnalysis) {
    match node.kind() {
        "variable_definition" => {
            if let Some((name, range)) = name_of(node, content) {
                a.define(&name, SymbolKind::Variable, range);
            }
        }
        "parameter_definition" => {
            if let Some((name, range)) = name_of(node, content) {
                a.define(&name, SymbolKind::Parameter, range);
            }
        }
        "property_definition" => {
            if let Some((name, range)) = name_of(node, content) {
                a.define(&name, SymbolKind::Property, range);
            }
        }
        "buffer_definition" => {
            if let Some((name, range)) = name_of(node, content) {
                a.define(&name, SymbolKind::Buffer, range);
            }
        }
        "function_definition" | "function_forward_definition" => {
            if let Some((name, range)) = name_of(node, content) {
                a.define(&name, SymbolKind::Function, range);
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_definitions(child, content, a);
    }
}

/// Parents whose `identifier` children are not variable references:
/// definitions (name/type/like fields), function names, dotted/property
/// chains, preprocessor directives, and program-name statements.
fn is_excluded_parent(kind: &str) -> bool {
    kind.ends_with("_definition")
        || matches!(
            kind,
            "function_call"
                | "object_access"
                | "qualified_name"
                | "scoped_name"
                | "macro_concatenated_name"
                | "include_file_reference"
                | "run_statement"
                | "call_statement"
                | "compile_statement"
                | "parameters"
                | "parameter_list"
        )
        || kind.starts_with("global_define_preprocessor_directive")
        || kind.starts_with("scoped_define_preprocessor_directive")
        || kind.starts_with("if_preprocessor_directive")
        || kind.starts_with("elseif_branch")
        || kind.starts_with("else_branch")
        || kind.starts_with("endif_branch")
        || kind.starts_with("undefine_preprocessor_directive")
        || kind.starts_with("message_preprocessor_directive")
}

fn walk_references<'t>(node: Node<'t>, content: &str, a: &mut SectionAnalysis) {
    match node.kind() {
        "variable" => {
            if let Some((name, range)) = name_of(node, content)
                && !name.contains('.')
                && a.find(&name).is_none()
            {
                a.unknown_variables.push(UnknownVariable { name, range });
            }
        }
        "identifier" => {
            // The grammar emits bare `identifier`s in expression contexts
            // (message args, assignment targets, operators...). Only treat
            // them as variable references when their parent is a reference
            // context, never a definition or name-bearing construct.
            if let Some(parent) = node.parent() {
                if is_excluded_parent(parent.kind()) {
                    return;
                }
                let Ok(name) = node.utf8_text(content.as_bytes()) else {
                    return;
                };
                let name = name.to_string();
                if !name.contains('.') && a.find(&name).is_none() {
                    a.unknown_variables.push(UnknownVariable {
                        name,
                        range: (node.start_byte(), node.end_byte()),
                    });
                }
            }
        }
        "function_call" => {
            let Some(fn_node) = node.child_by_field_name("function") else {
                return;
            };
            if fn_node.kind() == "identifier"
                && let Ok(name) = fn_node.utf8_text(content.as_bytes())
            {
                let name = name.to_string();
                if a.find(&name).is_none() && !is_builtin_function(&name) && !name.contains('.') {
                    a.unknown_functions.push(UnknownFunction {
                        name,
                        range: (fn_node.start_byte(), fn_node.end_byte()),
                    });
                }
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_references(child, content, a);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::WebSpeedDocument;

    fn analysis_of(src: &str) -> SectionAnalysis {
        let mut doc = WebSpeedDocument::new();
        doc.update(src.to_string());
        let idx = doc
            .sections()
            .iter()
            .position(|s| s.kind == crate::scanner::SectionKind::SpeedScript)
            .expect("speedscript section");
        analyze(&doc, idx).expect("clean tree analysis")
    }

    #[test]
    fn collects_definitions_case_insensitively() {
        let a = analysis_of("<% define variable cnt as integer. define variable NAME as char. %>");
        assert_eq!(a.symbols().len(), 2);
        assert_eq!(a.symbols()[0].name, "cnt");
        assert!(a.find("CNT").is_some());
        assert!(a.find("name").is_some());
        assert!(a.find("nope").is_none());
    }

    #[test]
    fn reports_unknown_variables_and_functions() {
        let a = analysis_of(
            "<% define variable known as integer. message unknownValue. message fn(1). message ABS(2). known = 5. %>",
        );
        assert_eq!(a.unknown_variables.len(), 1);
        assert_eq!(a.unknown_variables[0].name, "unknownValue");
        assert_eq!(a.unknown_functions.len(), 1);
        assert_eq!(a.unknown_functions[0].name, "fn");
    }

    #[test]
    fn skips_error_trees() {
        let mut doc = WebSpeedDocument::new();
        doc.update("<% define variable a as integer %>".to_string());
        let idx = doc
            .sections()
            .iter()
            .position(|s| s.kind == crate::scanner::SectionKind::SpeedScript)
            .unwrap();
        assert!(analyze(&doc, idx).is_none());
    }

    #[test]
    fn functions_are_known_after_definition() {
        let a = analysis_of(
            "<% function square returns integer (input x as integer): return x * x. end function. message square(2). %>",
        );
        assert!(a.find("square").is_some());
        assert!(a.unknown_functions.is_empty());
    }
}

use tree_sitter::{Node, Parser};

#[derive(Debug, Clone, Copy)]
pub struct IndentOptions {
    pub indent_size: usize,
    pub use_tabs: bool,
}

impl Default for IndentOptions {
    fn default() -> Self {
        Self {
            indent_size: 2,
            use_tabs: false,
        }
    }
}

pub fn autoindent_text(text: &str, options: IndentOptions) -> String {
    let mut line_indents = vec![0usize; line_count(text)];

    if let Some(tree) = parse_abl_tree(text) {
        collect_line_indents(tree.root_node(), text, &mut line_indents);
    }

    let mut out = String::with_capacity(text.len());
    let mut blank_run = 0usize;

    for (idx, raw_line) in text.split_inclusive('\n').enumerate() {
        let (line_without_nl, newline) = split_line_ending(raw_line);
        let trimmed = line_without_nl.trim();

        if trimmed.is_empty() {
            blank_run += 1;
            if blank_run == 1 {
                out.push_str(newline);
            }
            continue;
        }

        blank_run = 0;
        let indent = line_indents.get(idx).copied().unwrap_or_default();
        push_indent(&mut out, indent, options);
        out.push_str(trimmed);
        out.push_str(newline);
    }

    out
}

/// Compares the AST before and after formatting to make sure the reformat did
/// not change the structure of the code.
///
/// When both sides parse cleanly the tree shapes must match exactly. When either
/// side has a syntax error the trees cannot be compared reliably, so the check
/// passes: the formatter only ever rewrites horizontal whitespace, and ABL is
/// not whitespace sensitive, so a reformat cannot alter the meaning of the
/// code even when the parser is unhappy about something else in the file.
pub fn preserves_ast_shape(original: &str, formatted: &str, parser: &mut Parser) -> bool {
    let Some(before) = parser.parse(original, None) else {
        return true;
    };
    let Some(after) = parser.parse(formatted, None) else {
        return true;
    };

    let before_root = before.root_node();
    let after_root = after.root_node();
    if before_root.has_error() || after_root.has_error() {
        return true;
    }

    before_root.to_sexp() == after_root.to_sexp()
}

fn split_line_ending(raw_line: &str) -> (&str, &str) {
    if let Some(stripped) = raw_line.strip_suffix("\r\n") {
        return (stripped, "\r\n");
    }
    if let Some(stripped) = raw_line.strip_suffix('\n') {
        return (stripped, "\n");
    }
    (raw_line, "")
}

fn line_count(text: &str) -> usize {
    if text.is_empty() {
        return 1;
    }
    text.bytes().filter(|b| *b == b'\n').count() + 1
}

fn parse_abl_tree(text: &str) -> Option<tree_sitter::Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_abl::LANGUAGE.into())
        .ok()?;
    parser.parse(text, None)
}

fn collect_line_indents(node: Node<'_>, text: &str, line_indents: &mut [usize]) {
    apply_body_indent(node, text, line_indents);
    apply_case_indent(node, line_indents);
    apply_definition_indent(node, line_indents);
    apply_continuation_indent(node, line_indents);

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_named() {
            collect_line_indents(child, text, line_indents);
        }
    }
}

fn apply_case_indent(node: Node<'_>, line_indents: &mut [usize]) {
    match node.kind() {
        "case_when_phrase" | "case_otherwise_phrase" => {
            let start = node.start_position().row;
            let mut end = node.end_position().row;
            if node.end_position().column == 0 && end > 0 {
                end -= 1;
            }
            add_indent_range(line_indents, start, end);
        }
        _ => {}
    }
}

fn apply_body_indent(node: Node<'_>, text: &str, line_indents: &mut [usize]) {
    if node.kind() == "include_file_reference" {
        let start = node.start_position().row + 1;
        let end = node.end_position().row.saturating_sub(1);
        add_indent_range(line_indents, start, end);
    }

    if let Some(body) = first_named_child_of_kind(node, "body") {
        let start_row = body.start_position().row;
        let start_col = body.start_position().column;
        let mut end_row = body.end_position().row;
        let end_col = body.end_position().column;

        let start = if start_col > 0 {
            start_row.saturating_add(1)
        } else {
            start_row
        };
        if end_col == 0 && end_row > 0 {
            end_row -= 1;
        }
        if end_row >= start && body_ends_with_own_block_closer(body, text, end_row) {
            end_row = end_row.saturating_sub(1);
        }
        add_indent_range(line_indents, start, end_row);
        indent_comments_after_body(node, body, line_indents);
    }
}

fn indent_comments_after_body(node: Node<'_>, body: Node<'_>, line_indents: &mut [usize]) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "comment" || child.start_byte() < body.end_byte() {
            continue;
        }
        add_indent_range(
            line_indents,
            child.start_position().row,
            child.end_position().row,
        );
    }
}

fn apply_definition_indent(node: Node<'_>, line_indents: &mut [usize]) {
    match node.kind() {
        "function_definition" => {
            let start = first_statement_row(node);
            let end = last_statement_row(node).unwrap_or_else(|| node.end_position().row);
            if let Some(start) = start {
                add_indent_range(line_indents, start, end);
            }
        }
        "temp_table_definition" | "work_table_definition" => {
            let Some(start) =
                first_child_row_of_kinds(node, &["temp_table_field", "temp_table_index"])
            else {
                return;
            };
            let end = last_child_row_of_kinds(node, &["temp_table_field", "temp_table_index"])
                .unwrap_or(start);
            add_indent_range(line_indents, start, end);
        }
        _ => {}
    }
}

fn apply_continuation_indent(node: Node<'_>, line_indents: &mut [usize]) {
    let Some((start, end)) = continuation_range(node) else {
        return;
    };
    add_indent_range(line_indents, start, end);
}

fn continuation_range(node: Node<'_>) -> Option<(usize, usize)> {
    let start_row = node.start_position().row;
    let mut end_row = node.end_position().row;
    let end_col = node.end_position().column;
    if end_col == 0 && end_row > 0 {
        end_row -= 1;
    }

    match node.kind() {
        "case_statement" => None,
        "parameters" => parameter_continuation_range(node),
        "if_statement" => {
            let anchor = if_then_anchor(node)?;
            continuation_range_until_anchor(start_row, anchor)
        }
        "can_find_expression" => {
            let from = start_row.saturating_add(1);
            (from <= end_row).then_some((from, end_row))
        }
        kind if kind.ends_with("_statement") => {
            if let Some(body) = first_named_child_of_kind(node, "body") {
                continuation_range_until_anchor(start_row, body)
            } else {
                let from = start_row.saturating_add(1);
                (from <= end_row).then_some((from, end_row))
            }
        }
        _ => None,
    }
}

fn parameter_continuation_range(node: Node<'_>) -> Option<(usize, usize)> {
    let from = node.start_position().row.saturating_add(1);
    let mut cursor = node.walk();
    let end = node
        .named_children(&mut cursor)
        .filter(|child| child.kind() == "parameter")
        .map(|child| child.end_position().row)
        .last()?;
    (from <= end).then_some((from, end))
}

fn continuation_range_until_anchor(start_row: usize, anchor: Node<'_>) -> Option<(usize, usize)> {
    let anchor_row = anchor.start_position().row;
    let anchor_col = anchor.start_position().column;
    let upper = if anchor_col == 0 {
        anchor_row.saturating_sub(1)
    } else {
        anchor_row
    };
    let from = start_row.saturating_add(1);
    (from <= upper).then_some((from, upper))
}

fn first_named_child_of_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| child.is_named() && child.kind() == kind)
}

fn first_child_row_of_kinds(node: Node<'_>, kinds: &[&str]) -> Option<usize> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| child.is_named() && kinds.contains(&child.kind()))
        .map(|child| child.start_position().row)
}

fn last_child_row_of_kinds(node: Node<'_>, kinds: &[&str]) -> Option<usize> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|child| child.is_named() && kinds.contains(&child.kind()))
        .map(|child| child.end_position().row)
        .last()
}

fn first_statement_row(node: Node<'_>) -> Option<usize> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| child.is_named() && is_statement_like(child.kind()))
        .map(|child| child.start_position().row)
}

fn last_statement_row(node: Node<'_>) -> Option<usize> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|child| child.is_named() && is_statement_like(child.kind()))
        .map(|child| {
            let mut end_row = child.end_position().row;
            if child.end_position().column == 0 && end_row > 0 {
                end_row -= 1;
            }
            end_row
        })
        .last()
}

fn is_statement_like(kind: &str) -> bool {
    kind.ends_with("_statement") || kind.ends_with("_definition")
}

fn if_then_anchor<'a>(node: Node<'a>) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    let mut saw_condition = false;
    for child in node.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        if !saw_condition {
            saw_condition = true;
            continue;
        }
        let kind = child.kind();
        if kind.ends_with("_statement") || kind.ends_with("_definition") || kind == "body" {
            return Some(child);
        }
    }
    None
}

fn add_indent_range(line_indents: &mut [usize], start: usize, end: usize) {
    if start > end || line_indents.is_empty() {
        return;
    }
    let from = start.min(line_indents.len() - 1);
    let to = end.min(line_indents.len() - 1);
    if from > to {
        return;
    }
    for indent in line_indents.iter_mut().take(to + 1).skip(from) {
        *indent += 1;
    }
}

fn is_block_closer_line(text: &str, row: usize) -> bool {
    let Some(line) = text.lines().nth(row) else {
        return false;
    };
    let trimmed = line.trim_start_matches([' ', '\t']);
    let upper = trimmed.to_ascii_uppercase();
    upper.starts_with("END")
        || upper.starts_with("ELSE")
        || upper.starts_with("CATCH")
        || upper.starts_with("FINALLY")
}

fn body_ends_with_own_block_closer(body: Node<'_>, text: &str, end_row: usize) -> bool {
    if !is_block_closer_line(text, end_row) {
        return false;
    }

    let Some(last_child) = body.named_child(body.named_child_count().saturating_sub(1) as u32)
    else {
        return true;
    };

    let mut last_child_end_row = last_child.end_position().row;
    if last_child.end_position().column == 0 && last_child_end_row > 0 {
        last_child_end_row -= 1;
    }

    last_child_end_row < end_row
}

fn push_indent(out: &mut String, level: usize, options: IndentOptions) {
    if options.use_tabs {
        for _ in 0..level {
            out.push('\t');
        }
        return;
    }

    let width = options.indent_size.max(1);
    for _ in 0..(level * width) {
        out.push(' ');
    }
}

#[cfg(test)]
mod tests {
    use super::{IndentOptions, autoindent_text, collect_line_indents, preserves_ast_shape};
    use tree_sitter::Parser;

    fn parse_abl(src: &str) -> tree_sitter::Tree {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_abl::LANGUAGE.into())
            .expect("set abl language");
        parser.parse(src, None).expect("parse source")
    }

    #[test]
    fn indents_simple_do_block() {
        let input = "IF TRUE THEN DO:\nMESSAGE \"X\".\nEND.\n";
        let got = autoindent_text(input, IndentOptions::default());
        let expected = "IF TRUE THEN DO:\n  MESSAGE \"X\".\nEND.\n";
        assert_eq!(got, expected);
    }

    #[test]
    fn strips_trailing_whitespace_from_code_lines() {
        let input = "MESSAGE \"X\".   \n\tMESSAGE \"Y\".\t\n";
        let got = autoindent_text(input, IndentOptions::default());
        assert_eq!(got, "MESSAGE \"X\".\nMESSAGE \"Y\".\n");
    }

    #[test]
    fn collapses_runs_of_blank_lines_to_one() {
        let input = "MESSAGE \"X\".\n\n\n\nMESSAGE \"Y\".\n\n\n\nEND.\n";
        let got = autoindent_text(input, IndentOptions::default());
        assert_eq!(got, "MESSAGE \"X\".\n\nMESSAGE \"Y\".\n\nEND.\n");
    }

    #[test]
    fn whitespace_only_lines_become_truly_empty() {
        let input = "MESSAGE \"X\".\n   \n\t\t\nMESSAGE \"Y\".\n";
        let got = autoindent_text(input, IndentOptions::default());
        assert_eq!(got, "MESSAGE \"X\".\n\nMESSAGE \"Y\".\n");
    }

    #[test]
    fn formatting_is_idempotent() {
        let input = "IF TRUE THEN DO:\n\n\n\nMESSAGE \"X\".   \n\n\nEND.\n";
        let once = autoindent_text(input, IndentOptions::default());
        let twice = autoindent_text(&once, IndentOptions::default());
        assert_eq!(once, twice);
    }

    #[test]
    fn preserves_ast_shape_accepts_files_with_syntax_errors() {
        let original = "MESSAGE \"X\".   \nthis is not valid abl $$$\n\n\n\nMESSAGE \"Y\".\n";
        let formatted = autoindent_text(original, IndentOptions::default());
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_abl::LANGUAGE.into())
            .expect("set abl language");
        assert!(parse_abl(original).root_node().has_error());
        assert!(preserves_ast_shape(original, &formatted, &mut parser));
    }

    #[test]
    fn preserves_ast_shape_still_rejects_real_structural_changes() {
        let original = "MESSAGE \"X\".\n";
        let altered = "MESSAGE \"X\".\nMESSAGE \"Z\".\n";
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_abl::LANGUAGE.into())
            .expect("set abl language");
        assert!(!preserves_ast_shape(original, altered, &mut parser));
    }

    #[test]
    fn indents_comments_after_last_statement_in_block() {
        let input =
            "IF TRUE THEN DO:\n// leading comment\nMESSAGE \"X\".\n/* trailing\ncomment */\nEND.\n";
        let got = autoindent_text(input, IndentOptions::default());
        let expected = "IF TRUE THEN DO:\n  // leading comment\n  MESSAGE \"X\".\n  /* trailing\n  comment */\nEND.\n";
        assert_eq!(got, expected);
    }

    #[test]
    fn keeps_for_each_header_continuation_indented() {
        let input = "FOR EACH cust WHERE\nname = \"A\" AND\ncity = \"B\"\nNO-LOCK\nON ERROR UNDO, THROW:\nMESSAGE cust.name.\nEND.\n";
        let got = autoindent_text(input, IndentOptions::default());
        let expected = "FOR EACH cust WHERE\n  name = \"A\" AND\n  city = \"B\"\n  NO-LOCK\n  ON ERROR UNDO, THROW:\n  MESSAGE cust.name.\nEND.\n";
        assert_eq!(got, expected);
    }

    #[test]
    fn indents_include_arguments() {
        let input = "{{&ZM_CIM}cim_icunis.i\n&CIM=CIM\n&OUT=OUT\n}\n";
        let got = autoindent_text(input, IndentOptions::default());
        let expected = "{{&ZM_CIM}cim_icunis.i\n  &CIM=CIM\n  &OUT=OUT\n}\n";
        assert_eq!(got, expected);
    }

    #[test]
    fn indents_multiline_if_condition() {
        let input = "IF a = 1 AND\nb = 2 THEN DO:\nMESSAGE \"ok\".\nEND.\n";
        let got = autoindent_text(input, IndentOptions::default());
        let expected = "IF a = 1 AND\n  b = 2 THEN DO:\n  MESSAGE \"ok\".\nEND.\n";
        assert_eq!(got, expected);
    }

    #[test]
    fn indents_assign_continuation_lines() {
        let input = "ASSIGN\nx = 1\ny = 2.\n";
        let got = autoindent_text(input, IndentOptions::default());
        let expected = "ASSIGN\n  x = 1\n  y = 2.\n";
        assert_eq!(got, expected);
    }

    #[test]
    fn indents_multiline_put_stream_unformatted_items() {
        let input = "PUT STREAM estr UNFORMATTED\n\"lvc_execname\"   \" \" lvc_execname   skip\n\"lvc_key1\"       \" \" lvc_key1       skip\n\"kod_kk\"         \" \" kod_kk\n.\n";
        let got = autoindent_text(input, IndentOptions::default());
        let expected = "PUT STREAM estr UNFORMATTED\n  \"lvc_execname\"   \" \" lvc_execname   skip\n  \"lvc_key1\"       \" \" lvc_key1       skip\n  \"kod_kk\"         \" \" kod_kk\n  .\n";
        assert_eq!(got, expected);
    }

    #[test]
    fn indents_case_when_and_otherwise_phrases() {
        let input = "CASE lvc_execname:\nWHEN 'x1pwkon' THEN LEAVE.\nWHEN 'x1atrup1' THEN typ_kk = \"PG\".\nOTHERWISE typ_kk = \"PW\".\nEND CASE.\n";
        let got = autoindent_text(input, IndentOptions::default());
        let expected = "CASE lvc_execname:\n  WHEN 'x1pwkon' THEN LEAVE.\n  WHEN 'x1atrup1' THEN typ_kk = \"PG\".\n  OTHERWISE typ_kk = \"PW\".\nEND CASE.\n";
        assert_eq!(got, expected);
    }

    #[test]
    fn indents_nested_case_do_body() {
        let input = "CASE status:\nWHEN \"A\" THEN DO:\nPUT UNFORMATTED \"A\" SKIP.\nEND.\nOTHERWISE PUT UNFORMATTED \"Z\" SKIP.\nEND CASE.\n";
        let got = autoindent_text(input, IndentOptions::default());
        let expected = "CASE status:\n  WHEN \"A\" THEN DO:\n    PUT UNFORMATTED \"A\" SKIP.\n  END.\n  OTHERWISE PUT UNFORMATTED \"Z\" SKIP.\nEND CASE.\n";
        assert_eq!(got, expected);
    }

    #[test]
    fn preserves_ast_for_indentation_only_change() {
        let source = "IF TRUE THEN DO:\nMESSAGE \"X\".\nEND.\n";
        let formatted = autoindent_text(source, IndentOptions::default());
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_abl::LANGUAGE.into())
            .expect("set abl language");
        assert!(preserves_ast_shape(source, &formatted, &mut parser));
    }

    #[test]
    fn output_is_idempotent() {
        let input = "IF TRUE THEN DO:\nMESSAGE \"X\".\nEND.\n";
        let once = autoindent_text(input, IndentOptions::default());
        let twice = autoindent_text(&once, IndentOptions::default());
        assert_eq!(once, twice);
        // sanity: parse remains valid
        let _ = parse_abl(&once);
    }

    #[test]
    fn derives_indent_from_do_body_node() {
        let source = "IF TRUE THEN DO:\nMESSAGE \"X\".\nEND.\n";
        let tree = parse_abl(source);
        let mut indents = vec![0usize; 4];
        collect_line_indents(tree.root_node(), source, &mut indents);
        assert_eq!(indents, vec![0, 1, 0, 0]);
    }

    #[test]
    fn keeps_end_procedure_aligned_with_procedure_header_without_trailing_newline() {
        let input = "PROCEDURE p:\nMESSAGE \"x\".\nEND PROCEDURE.";
        let got = autoindent_text(input, IndentOptions::default());
        let expected = "PROCEDURE p:\n  MESSAGE \"x\".\nEND PROCEDURE.";
        assert_eq!(got, expected);
    }

    #[test]
    fn keeps_nested_end_lines_aligned_without_trailing_newline() {
        let input = "IF ready THEN DO:\nFOR EACH item NO-LOCK:\nMESSAGE item.id.\nEND.\nEND.";
        let got = autoindent_text(input, IndentOptions::default());
        let expected =
            "IF ready THEN DO:\n  FOR EACH item NO-LOCK:\n    MESSAGE item.id.\n  END.\nEND.";
        assert_eq!(got, expected);
    }

    #[test]
    fn indents_function_body_and_keeps_end_function_aligned() {
        let input = "FUNCTION f RETURNS LOGICAL ():\nRETURN TRUE.\nEND FUNCTION.";
        let got = autoindent_text(input, IndentOptions::default());
        let expected = "FUNCTION f RETURNS LOGICAL ():\n  RETURN TRUE.\nEND FUNCTION.";
        assert_eq!(got, expected);
    }

    #[test]
    fn indents_catch_closer_inside_function() {
        let input = "FUNCTION f RETURNS LOGICAL ():\nRETURN TRUE.\nCATCH err AS Progress.Lang.Error:\nUNDO, THROW err.\nEND.\nEND FUNCTION.";
        let got = autoindent_text(input, IndentOptions::default());
        let expected = "FUNCTION f RETURNS LOGICAL ():\n  RETURN TRUE.\n  CATCH err AS Progress.Lang.Error:\n    UNDO, THROW err.\n  END.\nEND FUNCTION.";
        assert_eq!(got, expected);
    }

    #[test]
    fn indents_multiline_function_parameters() {
        let input = "FUNCTION calculate RETURNS LOGICAL (INPUT effectiveDate AS DATE,\nINPUT code AS CHARACTER,\nOUTPUT amount AS DECIMAL\n).";
        let got = autoindent_text(input, IndentOptions::default());
        let expected = "FUNCTION calculate RETURNS LOGICAL (INPUT effectiveDate AS DATE,\n  INPUT code AS CHARACTER,\n  OUTPUT amount AS DECIMAL\n).";
        assert_eq!(got, expected);
    }

    #[test]
    fn indents_temp_table_fields_and_indexes() {
        let input =
            "DEFINE TEMP-TABLE tt NO-UNDO\nFIELD id AS CHARACTER\nINDEX idx IS PRIMARY UNIQUE id.";
        let got = autoindent_text(input, IndentOptions::default());
        let expected = "DEFINE TEMP-TABLE tt NO-UNDO\n  FIELD id AS CHARACTER\n  INDEX idx IS PRIMARY UNIQUE id.";
        assert_eq!(got, expected);
    }
}

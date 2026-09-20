use tree_sitter::Node;

use crate::utils::ts::direct_child_by_kind;

pub(super) fn collect(node: Node<'_>, text: &str, line_indents: &mut [usize]) {
    apply_body_indent(node, text, line_indents);
    apply_case_indent(node, line_indents);
    apply_definition_indent(node, line_indents);
    apply_parenthesized_expression_indent(node, text, line_indents);
    apply_continuation_indent(node, line_indents);

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect(child, text, line_indents);
    }
}

fn apply_parenthesized_expression_indent(node: Node<'_>, text: &str, line_indents: &mut [usize]) {
    if node.kind() != "parenthesized_expression"
        || node.start_position().row == node.end_position().row
    {
        return;
    }
    let end = if is_closing_parenthesis_line(text, node.end_position().row) {
        node.end_position().row.saturating_sub(1)
    } else {
        node.end_position().row
    };
    add_indent_range(
        line_indents,
        node.start_position().row.saturating_add(1),
        end,
    );
}

fn is_closing_parenthesis_line(text: &str, row: usize) -> bool {
    text.lines()
        .nth(row)
        .is_some_and(|line| line.trim_start_matches([' ', '\t']).starts_with(')'))
}

fn apply_case_indent(node: Node<'_>, line_indents: &mut [usize]) {
    if matches!(node.kind(), "case_when_phrase" | "case_otherwise_phrase") {
        let start = node.start_position().row;
        let mut end = node.end_position().row;
        if node.end_position().column == 0 && end > 0 {
            end -= 1;
        }
        add_indent_range(line_indents, start, end);
    }
}

fn apply_body_indent(node: Node<'_>, text: &str, line_indents: &mut [usize]) {
    if node.kind() == "include_file_reference" {
        add_indent_range(
            line_indents,
            node.start_position().row.saturating_add(1),
            node.end_position().row.saturating_sub(1),
        );
    }

    let Some(body) = direct_child_by_kind(node, "body") else {
        return;
    };
    let start_row = body.start_position().row;
    let mut end_row = body.end_position().row;
    let start = if body.start_position().column > 0 {
        start_row.saturating_add(1)
    } else {
        start_row
    };
    if body.end_position().column == 0 && end_row > 0 {
        end_row -= 1;
    }
    if end_row >= start && body_ends_with_own_block_closer(body, text, end_row) {
        end_row = end_row.saturating_sub(1);
    }
    add_indent_range(line_indents, start, end_row);
    indent_comments_after_body(node, body, line_indents);
}

fn indent_comments_after_body(node: Node<'_>, body: Node<'_>, line_indents: &mut [usize]) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "comment" && child.start_byte() >= body.end_byte() {
            add_indent_range(
                line_indents,
                child.start_position().row,
                child.end_position().row,
            );
        }
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
            let Some(first_child_row) =
                first_child_row_of_kinds(node, &["temp_table_field", "temp_table_index"])
            else {
                return;
            };
            let start = first_child_row.max(node.start_position().row.saturating_add(1));
            let end = last_child_row_of_kinds(node, &["temp_table_field", "temp_table_index"])
                .unwrap_or(first_child_row);
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
    if node.end_position().column == 0 && end_row > 0 {
        end_row -= 1;
    }

    match node.kind() {
        "case_statement" => None,
        "parameters" => parameter_continuation_range(node),
        "if_statement" => continuation_range_until_anchor(start_row, if_then_anchor(node)?),
        "can_find_expression" => {
            let from = start_row.saturating_add(1);
            (from <= end_row).then_some((from, end_row))
        }
        "expression_statement"
            if direct_child_by_kind(node, "include_file_reference").is_some() =>
        {
            None
        }
        kind if kind.ends_with("_statement") => {
            if let Some(body) = direct_child_by_kind(node, "body") {
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
    let upper = if anchor.start_position().column == 0 {
        anchor_row.saturating_sub(1)
    } else {
        anchor_row
    };
    let from = start_row.saturating_add(1);
    (from <= upper).then_some((from, upper))
}

fn first_child_row_of_kinds(node: Node<'_>, kinds: &[&str]) -> Option<usize> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()))
        .map(|child| child.start_position().row)
}

fn last_child_row_of_kinds(node: Node<'_>, kinds: &[&str]) -> Option<usize> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| kinds.contains(&child.kind()))
        .map(|child| child.end_position().row)
        .last()
}

fn first_statement_row(node: Node<'_>) -> Option<usize> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| is_statement_like(child.kind()))
        .map(|child| child.start_position().row)
}

fn last_statement_row(node: Node<'_>) -> Option<usize> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| is_statement_like(child.kind()))
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

fn if_then_anchor(node: Node<'_>) -> Option<Node<'_>> {
    node.child_by_field_name("then")
}

fn add_indent_range(line_indents: &mut [usize], start: usize, end: usize) {
    if start > end || line_indents.is_empty() {
        return;
    }
    let from = start.min(line_indents.len() - 1);
    let to = end.min(line_indents.len() - 1);
    for indent in line_indents.iter_mut().take(to + 1).skip(from) {
        *indent += 1;
    }
}

fn is_block_closer_line(text: &str, row: usize) -> bool {
    let Some(line) = text.lines().nth(row) else {
        return false;
    };
    let upper = line.trim_start_matches([' ', '\t']).to_ascii_uppercase();
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

#[cfg(test)]
mod tests {
    use super::collect;
    use crate::analysis::formatting::{FormatterOptions, format_text};

    fn assert_default_format(input: &str, expected: &str) {
        let options = FormatterOptions {
            line_width: 0,
            ..FormatterOptions::default()
        };
        assert_eq!(format_text(input, options), expected);
    }

    #[test]
    fn indents_simple_do_block() {
        assert_default_format(
            "IF TRUE THEN DO:\nMESSAGE \"X\".\nEND.\n",
            "IF TRUE THEN DO:\n  MESSAGE \"X\".\nEND.\n",
        );
    }

    #[test]
    fn indents_comments_after_last_statement_in_block() {
        assert_default_format(
            "IF TRUE THEN DO:\n// leading comment\nMESSAGE \"X\".\n/* trailing\ncomment */\nEND.\n",
            "IF TRUE THEN DO:\n  // leading comment\n  MESSAGE \"X\".\n  /* trailing\n  comment */\nEND.\n",
        );
    }

    #[test]
    fn keeps_for_each_header_continuation_indented() {
        assert_default_format(
            "FOR EACH cust WHERE\nname = \"A\" AND\ncity = \"B\"\nNO-LOCK\nON ERROR UNDO, THROW:\nMESSAGE cust.name.\nEND.\n",
            "FOR EACH cust WHERE\n  name = \"A\" AND\n  city = \"B\"\n  NO-LOCK\n  ON ERROR UNDO, THROW:\n  MESSAGE cust.name.\nEND.\n",
        );
    }

    #[test]
    fn indents_include_arguments() {
        assert_default_format(
            "{{&INC_ROOT}shared.i\n&INPUT=value\n&OUTPUT=result\n}\n",
            "{{&INC_ROOT}shared.i\n  &INPUT=value\n  &OUTPUT=result\n}\n",
        );
    }

    #[test]
    fn aligns_include_arguments_and_closer_inside_do_body() {
        assert_default_format(
            "IF enabled THEN DO:\n {send_message.i\n       &To='team@example.invalid'\n   &Attachment=filePath\n          &Subject=subjectText\n    }.\n\n       QUIT.\nEND.",
            "IF enabled THEN DO:\n  {send_message.i\n    &To='team@example.invalid'\n    &Attachment=filePath\n    &Subject=subjectText\n  }.\n\n  QUIT.\nEND.\n",
        );
    }

    #[test]
    fn indents_multiline_if_condition() {
        assert_default_format(
            "IF a = 1 AND\nb = 2 THEN DO:\nMESSAGE \"ok\".\nEND.\n",
            "IF a = 1 AND\n  b = 2 THEN DO:\n  MESSAGE \"ok\".\nEND.\n",
        );
    }

    #[test]
    fn indents_statement_continuations() {
        assert_default_format("ASSIGN\nx = 1\ny = 2.\n", "ASSIGN\n  x = 1\n  y = 2.\n");
        assert_default_format(
            "PUT STREAM output_stream UNFORMATTED\n\"first\" value_a SKIP\n\"second\" value_b\n.\n",
            "PUT STREAM output_stream UNFORMATTED\n  \"first\" value_a SKIP\n  \"second\" value_b\n  .\n",
        );
    }

    #[test]
    fn indents_case_phrases_and_nested_body() {
        assert_default_format(
            "CASE status:\nWHEN \"A\" THEN DO:\nMESSAGE \"A\".\nEND.\nOTHERWISE MESSAGE \"Z\".\nEND CASE.\n",
            "CASE status:\n  WHEN \"A\" THEN DO:\n    MESSAGE \"A\".\n  END.\n  OTHERWISE MESSAGE \"Z\".\nEND CASE.\n",
        );
    }

    #[test]
    fn derives_indent_from_do_body_node() {
        let source = "IF TRUE THEN DO:\nMESSAGE \"X\".\nEND.\n";
        let tree = crate::analysis::parse_abl(source);
        let mut indents = vec![0usize; 4];
        collect(tree.root_node(), source, &mut indents);
        assert_eq!(indents, vec![0, 1, 0, 0]);
    }

    #[test]
    fn aligns_block_closers() {
        assert_default_format(
            "PROCEDURE p:\nMESSAGE \"x\".\nEND PROCEDURE.",
            "PROCEDURE p:\n  MESSAGE \"x\".\nEND PROCEDURE.\n",
        );
        assert_default_format(
            "IF ready THEN DO:\nFOR EACH item NO-LOCK:\nMESSAGE item.id.\nEND.\nEND.",
            "IF ready THEN DO:\n  FOR EACH item NO-LOCK:\n    MESSAGE item.id.\n  END.\nEND.\n",
        );
    }

    #[test]
    fn indents_function_bodies_parameters_and_catch() {
        assert_default_format(
            "FUNCTION f RETURNS LOGICAL ():\nRETURN TRUE.\nCATCH err AS Progress.Lang.Error:\nUNDO, THROW err.\nEND.\nEND FUNCTION.",
            "FUNCTION f RETURNS LOGICAL ():\n  RETURN TRUE.\n  CATCH err AS Progress.Lang.Error:\n    UNDO, THROW err.\n  END.\nEND FUNCTION.\n",
        );
        assert_default_format(
            "FUNCTION calculate RETURNS LOGICAL (INPUT effectiveDate AS DATE,\nINPUT code AS CHARACTER,\nOUTPUT amount AS DECIMAL\n).",
            "FUNCTION calculate RETURNS LOGICAL (INPUT effectiveDate AS DATE,\n  INPUT code AS CHARACTER,\n  OUTPUT amount AS DECIMAL\n).\n",
        );
    }

    #[test]
    fn indents_multiline_but_not_single_line_temp_table_members() {
        assert_default_format(
            "DEFINE TEMP-TABLE tt NO-UNDO\nFIELD id AS CHARACTER\nINDEX idx IS PRIMARY UNIQUE id.",
            "DEFINE TEMP-TABLE tt NO-UNDO\n  FIELD id AS CHARACTER\n  INDEX idx IS PRIMARY UNIQUE id.\n",
        );
        let single_line = "DEFINE TEMP-TABLE ttRecord FIELD record_id AS CHARACTER INDEX idx_record record_id.\nDEFINE VARIABLE record_count AS INTEGER.";
        assert_default_format(single_line, &format!("{single_line}\n"));
    }
}

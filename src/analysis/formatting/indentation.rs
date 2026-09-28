use tree_sitter::Node;

use crate::utils::ts::direct_child_by_kind;

pub(super) fn collect(node: Node<'_>, text: &str, line_indents: &mut [usize]) {
    apply_body_indent(node, text, line_indents);
    apply_member_indent(node, line_indents);
    apply_property_accessor_indent(node, line_indents);
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
    // Skip the body's first line only when that line still carries trailing
    // header text, such as the `ON ERROR UNDO, THROW:` closing a FOR EACH or
    // the `):` closing a constructor signature. Such a line holds the header,
    // so it keeps the node's own indentation.
    //
    // Testing the column alone gets this wrong whenever the header spans
    // several lines, as with a constructor whose `input` parameters wrap. There
    // the body starts at a non-zero column but on a line of its own that holds
    // no header text, and skipping it made the indent unstable between passes,
    // so the idempotence check rejected the whole document. A non-zero start
    // column only means "header text on this line" when the preceding sibling
    // of the body ends on that same row.
    let start = if body.start_position().column > 0 && previous_sibling_ends_on_row(node, body) {
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

/// Indents the members of a class, interface, or data source by one level.
///
/// These constructs have no `body` child: their members are direct named
/// children of the definition node, so `apply_body_indent` never sees them and
/// they collapse to the left margin. The header tokens (the type name and
/// anything sharing its line) are excluded so only real members move in, and
/// the closing `end class.` is excluded because it is not a named child.
///
/// `enum_statement` is deliberately absent: it ends in `_statement` and is
/// already covered by `continuation_range`, so a second rule would indent its
/// members twice.
fn apply_member_indent(node: Node<'_>, line_indents: &mut [usize]) {
    if !matches!(
        node.kind(),
        "class_definition" | "interface_definition" | "data_source_class_definition"
    ) {
        return;
    }

    let header_row = node.start_position().row;
    let mut cursor = node.walk();
    let mut start: Option<usize> = None;
    let mut end: Option<usize> = None;

    for child in node.named_children(&mut cursor) {
        // Skip the name in the `class Foo:` header, and any token sharing the
        // header line, so the header keeps the node's own indentation.
        if child.kind() == "identifier" || child.start_position().row == header_row {
            continue;
        }
        let from = child.start_position().row;
        let to = child.end_position().row;
        start = Some(start.map_or(from, |current: usize| current.min(from)));
        end = Some(end.map_or(to, |current: usize| current.max(to)));
    }

    if let (Some(from), Some(to)) = (start, end) {
        add_indent_range(line_indents, from, to);
    }
}

/// Indents the statements inside a property's `get:` and `set:` accessors.
///
/// `property_definition` has no `body` child either. Its accessor statements
/// are direct named children, bracketed by anonymous `GET`/`SET` header tokens
/// and an `END GET.` / `END SET.` closer, so no other rule indents them and
/// they stay flush with the property declaration.
///
/// The grammar stores the headers and closers as unnamed tokens, so the range
/// is found by walking the child list and pairing each accessor keyword with
/// the matching `END`. A one-line accessor such as `GET.` has no statements
/// between the two and is skipped.
fn apply_property_accessor_indent(node: Node<'_>, line_indents: &mut [usize]) {
    if node.kind() != "property_definition" {
        return;
    }

    let mut cursor = node.walk();
    let mut start: Option<usize> = None;
    for child in node.children(&mut cursor) {
        match child.kind() {
            // `GET:` and `SET (params):` open an accessor; the body starts on
            // the following line unless the header itself wraps.
            "GET" | "SET" => {
                start = Some(child.start_position().row.saturating_add(1));
            }
            // `END GET.` / `END SET.` closes it, so the row above is the last
            // body line.
            "END" => {
                if let Some(from) = start.take() {
                    let to = child.start_position().row.saturating_sub(1);
                    if to >= from {
                        add_indent_range(line_indents, from, to);
                    }
                }
            }
            _ => {}
        }
    }
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

/// True when the child immediately preceding `body` ends on the body's first row.
///
/// Used to tell whether a body that starts mid-line shares that line with the
/// header text in front of it. Returns true when there is no preceding child,
/// since a body that is the node's first child can only share the header line
/// when the header itself began on that row.
fn previous_sibling_ends_on_row(node: Node<'_>, body: Node<'_>) -> bool {
    let row = body.start_position().row;
    let mut cursor = node.walk();
    let mut previous: Option<Node<'_>> = None;
    for child in node.named_children(&mut cursor) {
        if child.id() == body.id() {
            return match previous {
                None => node.start_position().row == row,
                Some(prev) => prev.end_position().row == row,
            };
        }
        previous = Some(child);
    }
    false
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
    fn indents_members_of_class_interface_and_data_source() {
        assert_default_format(
            "class Foo:\ndefine variable a as integer no-undo.\nconstructor public Foo():\nx = 1.\nend constructor.\nmethod public void bar():\ny = 2.\nend method.\nend class.",
            "class Foo:\n  define variable a as integer no-undo.\n  constructor public Foo():\n    x = 1.\n  end constructor.\n  method public void bar():\n    y = 2.\n  end method.\nend class.\n",
        );
        // Members sitting flush at the left margin are the case this fixes.
        assert_default_format(
            "class Foo:\ndefine variable a as integer no-undo.\ndefine variable b as character no-undo.\nend class.",
            "class Foo:\n  define variable a as integer no-undo.\n  define variable b as character no-undo.\nend class.\n",
        );
        assert_default_format(
            "interface Foo:\nmethod public void bar().\nend interface.",
            "interface Foo:\n  method public void bar().\nend interface.\n",
        );
        assert_default_format(
            "CLASS Foo INHERITS Bar:\nMETHOD PUBLIC VOID baz():\nRETURN.\nEND METHOD.\nEND CLASS.",
            "CLASS Foo INHERITS Bar:\n  METHOD PUBLIC VOID baz():\n    RETURN.\n  END METHOD.\nEND CLASS.\n",
        );
    }

    #[test]
    fn leaves_enum_members_at_the_indent_continuation_range_already_gives() {
        // enum_statement ends in `_statement` and is handled by
        // continuation_range; the member rule must not indent it a second time.
        assert_default_format(
            "ENUM Foo:\ndefine ENUM no\nyes\nmaybe.\nend enum.",
            "ENUM Foo:\n  define ENUM no\n  yes\n  maybe.\n  end enum.\n",
        );
    }

    #[test]
    fn indents_property_getter_and_setter_bodies() {
        assert_default_format(
            "class Foo:\ndefine variable b as integer no-undo.\nDEFINE PRIVATE PROPERTY a AS CHAR NO-UNDO\nGET:\nRETURN 1.\nEND GET.\nSET (v AS CHAR):\na = v.\nEND SET.\nmethod public void m():\ny = 2.\nend method.\nend class.",
            "class Foo:\n  define variable b as integer no-undo.\n  DEFINE PRIVATE PROPERTY a AS CHAR NO-UNDO\n  GET:\n    RETURN 1.\n  END GET.\n  SET (v AS CHAR):\n    a = v.\n  END SET.\n  method public void m():\n    y = 2.\n  end method.\nend class.\n",
        );
    }

    #[test]
    fn indents_nested_statements_inside_property_accessors() {
        assert_default_format(
            "class Foo:\nDEFINE PRIVATE PROPERTY a AS CHAR NO-UNDO\nGET:\nIF TRUE THEN DO:\nRETURN 1.\nEND.\nEND GET.\nmethod public void m():\ny = 2.\nend method.\nend class.",
            "class Foo:\n  DEFINE PRIVATE PROPERTY a AS CHAR NO-UNDO\n  GET:\n    IF TRUE THEN DO:\n      RETURN 1.\n    END.\n  END GET.\n  method public void m():\n    y = 2.\n  end method.\nend class.\n",
        );
    }

    #[test]
    fn handles_property_accessors_without_bodies() {
        // `GET.` / `SET.` with no statements must not shift the following lines.
        assert_default_format(
            "class Foo:\nDEFINE PUBLIC PROPERTY m_Total AS INTEGER NO-UNDO\nGET.\nSET.\nmethod public void m():\ny = 2.\nend method.\nEND CLASS.",
            "class Foo:\n  DEFINE PUBLIC PROPERTY m_Total AS INTEGER NO-UNDO\n  GET.\n  SET.\n  method public void m():\n    y = 2.\n  end method.\nEND CLASS.\n",
        );
    }

    #[test]
    fn indents_constructor_body_when_parameters_wrap_over_lines() {
        // Regression: the body starts at a non-zero column but on a line of its
        // own, so a plain column test treated it as header text. The two
        // formatting passes then disagreed and the idempotence check rejected
        // the whole document.
        let options = FormatterOptions {
            line_width: 0,
            ..FormatterOptions::default()
        };
        let input = "class Foo:\ndefine variable a as integer no-undo.\nconstructor public Foo(\ninput s1 as character,\ninput s2 as character\n):\nx = 1.\ny = 2.\nend constructor.\nmethod public void m():\nz = 3.\nend method.\nend class.\n";
        let once = format_text(input, options);
        assert_eq!(
            once,
            "class Foo:\n  define variable a as integer no-undo.\n  constructor public Foo(\n    input s1 as character,\n    input s2 as character\n  ):\n    x = 1.\n    y = 2.\n  end constructor.\n  method public void m():\n    z = 3.\n  end method.\nend class.\n"
        );
        assert_eq!(format_text(&once, options), once);
    }
}

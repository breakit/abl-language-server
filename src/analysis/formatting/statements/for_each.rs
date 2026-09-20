use tree_sitter::Node;

use super::super::FormatterOptions;
use super::LayoutEdit;
use super::condition;
use super::support::{
    collapse_range, collapse_whitespace, contains_unsafe_multiline_content, continuation_column,
    fits, is_multiline, make_edit, statement_exceeds_width,
};
use crate::utils::ts::direct_child_by_kind;

pub(super) fn layout(node: Node<'_>, text: &str, options: FormatterOptions) -> Option<LayoutEdit> {
    let body = direct_child_by_kind(node, "body")?;
    if direct_named_children_before(node, body.start_byte())
        .into_iter()
        .any(contains_unsafe_multiline_content)
    {
        return None;
    }
    let header_end = body.start_byte().checked_add(1)?;
    if text.as_bytes().get(body.start_byte()) != Some(&b':') {
        return None;
    }
    let original = text.get(node.start_byte()..header_end)?;
    let collapsed = collapse_whitespace(original);
    if is_multiline(original) && fits(node.start_position().column, &collapsed, options.line_width)
    {
        return make_edit(node.start_byte(), header_end, original, collapsed);
    }
    if !is_multiline(original) && !statement_exceeds_width(node, original, options.line_width) {
        return None;
    }

    let replacement = wrap_header(node, body, text, options)?;
    make_edit(node.start_byte(), header_end, original, replacement)
}

fn wrap_header(
    node: Node<'_>,
    body: Node<'_>,
    text: &str,
    options: FormatterOptions,
) -> Option<String> {
    let record_phrase = direct_child_by_kind(node, "record_phrase")?;
    let condition = record_phrase.child_by_field_name("where")?;
    let mut lines = vec![collapse_range(
        text,
        node.start_byte(),
        condition.start_byte(),
    )?];
    lines.extend(condition::layout_lines(
        condition,
        text,
        continuation_column(node, options),
        options,
    )?);

    let mut trailing_parts = Vec::new();
    let record_tail = collapse_range(text, condition.end_byte(), record_phrase.end_byte())?;
    if !record_tail.is_empty() {
        trailing_parts.push(record_tail);
    }
    let mut cursor = node.walk();
    for clause in node.named_children(&mut cursor) {
        if clause.start_byte() < record_phrase.end_byte() || clause.kind() == "body" {
            continue;
        }
        let mut parts = if clause.kind() == "by_phrase" {
            split_by_phrases(clause, text)?
        } else {
            vec![collapse_range(
                text,
                clause.start_byte(),
                clause.end_byte(),
            )?]
        };
        trailing_parts.append(&mut parts);
    }
    pack_parts(
        &mut lines,
        trailing_parts,
        continuation_column(node, options),
        options.line_width,
    );
    lines
        .last_mut()?
        .push_str(text.get(body.start_byte()..body.start_byte() + 1)?);
    Some(lines.join("\n"))
}

fn split_by_phrases(node: Node<'_>, text: &str) -> Option<Vec<String>> {
    let mut starts = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named()
            && child
                .utf8_text(text.as_bytes())
                .is_ok_and(|token| token.eq_ignore_ascii_case("BY"))
        {
            starts.push(child.start_byte());
        }
    }
    if starts.is_empty() {
        return Some(vec![collapse_range(
            text,
            node.start_byte(),
            node.end_byte(),
        )?]);
    }
    starts
        .iter()
        .enumerate()
        .map(|(index, start)| {
            let end = starts.get(index + 1).copied().unwrap_or(node.end_byte());
            collapse_range(text, *start, end)
        })
        .collect()
}

fn pack_parts(lines: &mut Vec<String>, parts: Vec<String>, column: usize, line_width: usize) {
    let mut current = String::new();
    for part in parts {
        let candidate_len = if current.is_empty() {
            part.chars().count()
        } else {
            current.chars().count() + 1 + part.chars().count()
        };
        if !current.is_empty() && column + candidate_len > line_width {
            lines.push(current);
            current = part;
        } else {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(&part);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
}

fn direct_named_children_before(node: Node<'_>, end: usize) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| child.start_byte() < end)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::super::FormatterOptions;
    use crate::analysis::formatting::test_support::assert_safe_format;

    #[test]
    fn wraps_conditions_and_greedily_groups_trailing_clauses() {
        let input = "FOR EACH detail_record WHERE detail_record.parent_id = parent_record.id AND detail_record.owner = ownerId AND (parent_record.repeatable = FALSE OR DATE(detail_record.created_at) = TODAY) NO-LOCK BY detail_record.delivery BY detail_record.created_at ON ERROR UNDO, THROW:\nMESSAGE \"x\".\nEND.";
        let options = FormatterOptions {
            line_width: 90,
            ..FormatterOptions::default()
        };
        let expected = "FOR EACH detail_record WHERE\n  detail_record.parent_id = parent_record.id AND\n  detail_record.owner = ownerId AND\n  (parent_record.repeatable = FALSE OR DATE(detail_record.created_at) = TODAY)\n  NO-LOCK BY detail_record.delivery BY detail_record.created_at ON ERROR UNDO, THROW:\n  MESSAGE \"x\".\nEND.\n";
        assert_safe_format(input, expected, options);
    }

    #[test]
    fn collapses_separate_trailing_clauses_when_they_fit() {
        let input = "FOR EACH detail WHERE\n  detail.parent_id = an_extremely_long_parent_identifier_that_keeps_the_header_from_collapsing\n  NO-LOCK\n\n  BY detail.delivery\n  BY detail.date\n  BY detail.position:\nMESSAGE \"x\".\n\nMESSAGE \"y\".\nEND.";
        let options = FormatterOptions {
            line_width: 120,
            ..FormatterOptions::default()
        };
        let expected = "FOR EACH detail WHERE\n  detail.parent_id = an_extremely_long_parent_identifier_that_keeps_the_header_from_collapsing\n  NO-LOCK BY detail.delivery BY detail.date BY detail.position:\n  MESSAGE \"x\".\n\n  MESSAGE \"y\".\nEND.\n";
        assert_safe_format(input, expected, options);
    }

    #[test]
    fn recursively_wraps_parenthesized_where_expressions() {
        let input = "FOR EACH record WHERE record.key = keyValue AND (record.kind = \"A\" OR record.kind = \"B\") AND (record.site = siteValue OR record.owner = ownerValue OR (IF groupValue <> \"\" THEN record.group = groupValue ELSE FALSE) OR record.region = regionValue) NO-LOCK:\nMESSAGE \"x\".\nEND.";
        let options = FormatterOptions {
            line_width: 72,
            ..FormatterOptions::default()
        };
        let expected = "FOR EACH record WHERE\n  record.key = keyValue AND\n  (record.kind = \"A\" OR record.kind = \"B\") AND\n  (\n    record.site = siteValue OR\n    record.owner = ownerValue OR\n    (IF groupValue <> \"\" THEN record.group = groupValue ELSE FALSE) OR\n    record.region = regionValue\n  )\n  NO-LOCK:\n  MESSAGE \"x\".\nEND.\n";
        assert_safe_format(input, expected, options);
    }
}

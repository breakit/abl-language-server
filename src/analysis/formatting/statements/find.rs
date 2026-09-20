use tree_sitter::Node;

use super::super::FormatterOptions;
use super::LayoutEdit;
use super::condition;
use super::support::{
    collapse_range, collapse_whitespace, continuation_column, fits, is_multiline, make_edit,
    statement_exceeds_width,
};
use crate::utils::ts::direct_child_by_kind;

pub(super) fn layout(node: Node<'_>, text: &str, options: FormatterOptions) -> Option<LayoutEdit> {
    let original = node.utf8_text(text.as_bytes()).ok()?;
    let collapsed = collapse_whitespace(original);
    if is_multiline(original) && fits(node.start_position().column, &collapsed, options.line_width)
    {
        return make_edit(node.start_byte(), node.end_byte(), original, collapsed);
    }
    if !is_multiline(original) && !statement_exceeds_width(node, original, options.line_width) {
        return None;
    }

    let replacement = wrap(node, text, options)?;
    make_edit(node.start_byte(), node.end_byte(), original, replacement)
}

fn wrap(node: Node<'_>, text: &str, options: FormatterOptions) -> Option<String> {
    let where_phrase = direct_child_by_kind(node, "where_phrase");
    if let Some(condition) = where_phrase.and_then(|phrase| phrase.child_by_field_name("where")) {
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
        append_tail(&mut lines, text, condition.end_byte(), node.end_byte())?;
        return Some(lines.join("\n"));
    }

    let lock_phrase = first_lock_phrase(node);
    let first_break = where_phrase.or(lock_phrase)?;
    let mut lines = vec![collapse_range(
        text,
        node.start_byte(),
        first_break.start_byte(),
    )?];
    if let Some(where_phrase) = where_phrase {
        lines.push(collapse_range(
            text,
            where_phrase.start_byte(),
            where_phrase.end_byte(),
        )?);
    }
    let tail_start = lock_phrase
        .map(|lock| lock.start_byte())
        .unwrap_or_else(|| where_phrase.expect("where phrase exists").end_byte());
    append_tail(&mut lines, text, tail_start, node.end_byte())?;
    Some(lines.join("\n"))
}

fn first_lock_phrase(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| matches!(child.kind(), "exclusive_lock" | "share_lock" | "no_lock"))
}

fn append_tail(lines: &mut Vec<String>, text: &str, start: usize, end: usize) -> Option<()> {
    if start >= end {
        return Some(());
    }
    let tail = collapse_range(text, start, end)?;
    if tail == "." {
        lines.last_mut()?.push('.');
    } else if !tail.is_empty() {
        lines.push(tail);
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::super::super::FormatterOptions;
    use crate::analysis::formatting::test_support::{assert_format, assert_safe_format};

    #[test]
    fn wraps_clauses_at_line_width() {
        let input = "FIND FIRST audit_record WHERE audit_record.tenant = tenantId AND audit_record.category = categoryValue AND audit_record.sequence = sequenceValue EXCLUSIVE-LOCK NO-WAIT NO-ERROR.";
        let options = FormatterOptions {
            line_width: 80,
            ..FormatterOptions::default()
        };
        let expected = "FIND FIRST audit_record WHERE\n  audit_record.tenant = tenantId AND\n  audit_record.category = categoryValue AND\n  audit_record.sequence = sequenceValue\n  EXCLUSIVE-LOCK NO-WAIT NO-ERROR.\n";
        assert_safe_format(input, expected, options);
    }

    #[test]
    fn collapses_when_it_fits_line_width() {
        let input = "FIND FIRST customer\n  WHERE customer.id = customerId\n  NO-LOCK.";
        let options = FormatterOptions {
            line_width: 100,
            ..FormatterOptions::default()
        };
        assert_format(
            input,
            "FIND FIRST customer WHERE customer.id = customerId NO-LOCK.\n",
            options,
        );
    }
}

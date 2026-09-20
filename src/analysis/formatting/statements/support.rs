use tree_sitter::Node;

use super::super::FormatterOptions;
use super::LayoutEdit;

pub(super) fn make_edit(
    start: usize,
    end: usize,
    original: &str,
    replacement: String,
) -> Option<LayoutEdit> {
    (replacement != original).then_some(LayoutEdit {
        start,
        end,
        replacement,
    })
}

pub(super) fn collapse_range(text: &str, start: usize, end: usize) -> Option<String> {
    Some(collapse_whitespace(text.get(start..end)?))
}

pub(super) fn collapse_whitespace(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn fits(column: usize, text: &str, line_width: usize) -> bool {
    column + text.chars().count() <= line_width
}

pub(super) fn is_multiline(text: &str) -> bool {
    text.contains('\n') || text.contains('\r')
}

pub(super) fn statement_exceeds_width(node: Node<'_>, original: &str, line_width: usize) -> bool {
    original.lines().any(|line| {
        node.start_position().column + line.trim_start_matches([' ', '\t']).chars().count()
            > line_width
    })
}

pub(super) fn continuation_column(node: Node<'_>, options: FormatterOptions) -> usize {
    node.start_position().column + options.indent_size.max(1)
}

pub(super) fn contains_unsafe_multiline_content(node: Node<'_>) -> bool {
    if node.kind() == "comment"
        || (node.kind() == "string_literal" && node.start_position().row != node.end_position().row)
    {
        return true;
    }
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .any(contains_unsafe_multiline_content)
}

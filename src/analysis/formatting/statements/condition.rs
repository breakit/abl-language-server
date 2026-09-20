use tree_sitter::Node;

use super::super::FormatterOptions;
use super::support::{collapse_range, fits};

pub(super) fn layout_lines(
    condition: Node<'_>,
    text: &str,
    column: usize,
    options: FormatterOptions,
) -> Option<Vec<String>> {
    let collapsed = collapse_range(text, condition.start_byte(), condition.end_byte())?;
    if fits(column, &collapsed, options.line_width) {
        return Some(vec![collapsed]);
    }

    let operator = direct_boolean_operator(condition, text)?;
    let terms = flatten_boolean_expression(condition, text, operator)?;
    let term_count = terms.len();
    let mut lines = Vec::with_capacity(term_count);
    for (index, term) in terms.into_iter().enumerate() {
        let mut term_lines = layout_boolean_term(term, text, column, options)?;
        if index + 1 < term_count {
            term_lines.last_mut()?.push(' ');
            term_lines.last_mut()?.push_str(operator);
        }
        lines.extend(term_lines);
    }
    Some(lines)
}

fn direct_boolean_operator(node: Node<'_>, text: &str) -> Option<&'static str> {
    ["OR", "AND"]
        .into_iter()
        .find(|keyword| has_direct_keyword(node, text, keyword))
}

fn layout_boolean_term(
    node: Node<'_>,
    text: &str,
    column: usize,
    options: FormatterOptions,
) -> Option<Vec<String>> {
    let collapsed = collapse_range(text, node.start_byte(), node.end_byte())?;
    if fits(column, &collapsed, options.line_width) || node.kind() != "parenthesized_expression" {
        return Some(vec![collapsed]);
    }

    let inner = node.named_child(0)?;
    let operator = if has_direct_keyword(inner, text, "OR") {
        "OR"
    } else if has_direct_keyword(inner, text, "AND") {
        "AND"
    } else {
        return Some(vec![collapsed]);
    };
    let terms = flatten_boolean_expression(inner, text, operator)?;
    if terms.len() < 2 {
        return Some(vec![collapsed]);
    }

    let nested_column = column + options.indent_size.max(1);
    let term_count = terms.len();
    let mut lines = Vec::new();
    for (index, term) in terms.into_iter().enumerate() {
        let mut term_lines = layout_boolean_term(term, text, nested_column, options)?;
        if index + 1 < term_count {
            term_lines.last_mut()?.push(' ');
            term_lines.last_mut()?.push_str(operator);
        }
        lines.extend(term_lines);
    }
    lines.insert(0, "(".to_string());
    lines.push(")".to_string());
    Some(lines)
}

fn flatten_boolean_expression<'a>(
    node: Node<'a>,
    text: &str,
    keyword: &str,
) -> Option<Vec<Node<'a>>> {
    if node.kind() == "binary_expression" && has_direct_keyword(node, text, keyword) {
        let left = node.named_child(0)?;
        let right = node.named_child(1)?;
        let mut parts = flatten_boolean_expression(left, text, keyword)?;
        parts.extend(flatten_boolean_expression(right, text, keyword)?);
        return Some(parts);
    }
    Some(vec![node])
}

fn has_direct_keyword(node: Node<'_>, text: &str, keyword: &str) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor).any(|child| {
        !child.is_named()
            && child
                .utf8_text(text.as_bytes())
                .is_ok_and(|token| token.eq_ignore_ascii_case(keyword))
    })
}

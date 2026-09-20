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
    if let Some(simplified) = simplify_single_statement_do(node, text)
        && fits(
            node.start_position().column,
            &simplified,
            options.line_width,
        )
    {
        return make_edit(node.start_byte(), node.end_byte(), original, simplified);
    }

    let collapsed = collapse_whitespace(original);
    if is_multiline(original)
        && can_collapse(node)
        && fits(node.start_position().column, &collapsed, options.line_width)
    {
        return make_edit(node.start_byte(), node.end_byte(), original, collapsed);
    }
    if !is_multiline(original) && !statement_exceeds_width(node, original, options.line_width) {
        return None;
    }

    let replacement = wrap(node, text, options)?;
    make_edit(node.start_byte(), node.end_byte(), original, replacement)
}

fn simplify_single_statement_do(node: Node<'_>, text: &str) -> Option<String> {
    if node.child_by_field_name("else").is_some() {
        return None;
    }
    let do_statement = node.child_by_field_name("then")?;
    let statement = plain_do_body_statement(do_statement, text)?;
    if statement.start_position().row != statement.end_position().row {
        return None;
    }
    let header = collapse_range(text, node.start_byte(), do_statement.start_byte())?;
    let action = collapse_range(text, statement.start_byte(), statement.end_byte())?;
    Some(format!("{header} {action}"))
}

pub(super) fn plain_do_body_statement<'a>(do_statement: Node<'a>, text: &str) -> Option<Node<'a>> {
    if do_statement.kind() != "do_statement" {
        return None;
    }
    let body = direct_child_by_kind(do_statement, "body")?;
    let mut do_cursor = do_statement.walk();
    if do_statement.named_children(&mut do_cursor).count() != 1
        || !collapse_range(text, do_statement.start_byte(), body.start_byte())?
            .eq_ignore_ascii_case("DO")
        || !collapse_range(text, body.end_byte(), do_statement.end_byte())?
            .eq_ignore_ascii_case("END.")
    {
        return None;
    }
    let mut body_cursor = body.walk();
    let mut children = body.named_children(&mut body_cursor);
    let statement = children.next()?;
    children.next().is_none().then_some(statement)
}

fn can_collapse(node: Node<'_>) -> bool {
    let Some(then_statement) = node.child_by_field_name("then") else {
        return false;
    };
    then_statement.start_position().row == then_statement.end_position().row
        && node.child_by_field_name("else").is_none()
}

fn wrap(node: Node<'_>, text: &str, options: FormatterOptions) -> Option<String> {
    let then_statement = node.child_by_field_name("then")?;
    if then_statement.start_position().row != then_statement.end_position().row {
        return None;
    }

    let header = collapse_range(text, node.start_byte(), then_statement.start_byte())?;
    let action = collapse_range(text, then_statement.start_byte(), node.end_byte())?;
    if fits(node.start_position().column, &header, options.line_width) {
        return Some(format!("{header}\n {action}"));
    }

    let condition_node = node.named_child(0)?;
    if condition_node.end_byte() > then_statement.start_byte() {
        return None;
    }
    let prefix = collapse_range(text, node.start_byte(), condition_node.start_byte())?;
    let mut lines = condition::layout_lines(
        condition_node,
        text,
        continuation_column(node, options) + " THEN".len(),
        options,
    )?;
    lines.first_mut()?.insert_str(0, &format!("{prefix} "));
    lines.last_mut()?.push_str(" THEN");
    lines.push(format!(" {action}"));
    Some(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::super::super::FormatterOptions;
    use crate::analysis::formatting::test_support::{assert_format, assert_safe_format};

    #[test]
    fn wraps_and_collapses_single_action_statements() {
        let long = "IF TRIM(request_record.display_name) = \"\" THEN UNDO, THROW NEW Progress.Lang.AppError(\"Display name is required\", 400).";
        let options = FormatterOptions {
            line_width: 100,
            ..FormatterOptions::default()
        };
        let expected = "IF TRIM(request_record.display_name) = \"\" THEN\n  UNDO, THROW NEW Progress.Lang.AppError(\"Display name is required\", 400).\n";
        assert_format(long, expected, options);
        assert_format(
            "IF ready THEN\n  RETURN TRUE.",
            "IF ready THEN RETURN TRUE.\n",
            options,
        );
    }

    #[test]
    fn removes_plain_single_statement_do_wrapper_when_result_fits() {
        let input = "IF request_failed THEN DO:\n  UNDO, THROW NEW Progress.Lang.AppError(\"Request rejected\", 422).\nEND.";
        let options = FormatterOptions {
            line_width: 140,
            ..FormatterOptions::default()
        };
        let expected = "IF request_failed THEN UNDO, THROW NEW Progress.Lang.AppError(\"Request rejected\", 422).\n";
        assert_safe_format(input, expected, options);
    }

    #[test]
    fn wraps_each_boolean_condition_when_the_if_header_exceeds_line_width() {
        let input = "IF NOT CSV-CONTAINS(Survey.Customers, customerCode) OR NOT CSV-CONTAINS(Survey.Deliveries, visit.Delivery) OR NOT CSV-CONTAINS(Survey.Agents, visit.Agent) OR NOT CSV-CONTAINS(Survey.Regions, regionCode) THEN {raise_error.i \"Survey is not assigned to this visit\"}";
        let options = FormatterOptions {
            line_width: 90,
            ..FormatterOptions::default()
        };
        let expected = "IF NOT CSV-CONTAINS(Survey.Customers, customerCode) OR\n  NOT CSV-CONTAINS(Survey.Deliveries, visit.Delivery) OR\n  NOT CSV-CONTAINS(Survey.Agents, visit.Agent) OR\n  NOT CSV-CONTAINS(Survey.Regions, regionCode) THEN\n  {raise_error.i \"Survey is not assigned to this visit\"}\n";
        assert_safe_format(input, expected, options);
    }
}

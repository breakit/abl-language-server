use tree_sitter::{Node, Parser};

mod indentation;
mod statements;
mod whitespace;

#[cfg(test)]
mod test_support;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineEnding {
    Preserve,
    Lf,
    Crlf,
}

#[derive(Debug, Clone, Copy)]
pub struct FormatterOptions {
    pub indent_size: usize,
    pub use_tabs: bool,
    pub trim_trailing_whitespace: bool,
    pub insert_final_newline: bool,
    pub trim_final_newlines: bool,
    pub max_consecutive_blank_lines: usize,
    pub line_ending: LineEnding,
    pub line_width: usize,
}

impl Default for FormatterOptions {
    fn default() -> Self {
        Self {
            indent_size: 2,
            use_tabs: false,
            trim_trailing_whitespace: true,
            insert_final_newline: true,
            trim_final_newlines: true,
            max_consecutive_blank_lines: 1,
            line_ending: LineEnding::Preserve,
            line_width: 100,
        }
    }
}

pub fn format_text(text: &str, options: FormatterOptions) -> String {
    let preserved_line_ending = whitespace::first_line_ending(text).unwrap_or("\n");
    let laid_out = layout_statements(text, options);
    let mut line_indents = vec![0usize; whitespace::line_count(&laid_out)];
    if let Some(tree) = parse_abl_tree(&laid_out) {
        indentation::collect(tree.root_node(), &laid_out, &mut line_indents);
    }
    whitespace::render(&laid_out, &line_indents, preserved_line_ending, options)
}

fn layout_statements(text: &str, options: FormatterOptions) -> String {
    if options.line_width == 0 {
        return text.to_string();
    }
    let Some(tree) = parse_abl_tree(text) else {
        return text.to_string();
    };

    let edits = statements::layout_edits(tree.root_node(), text, options);
    if edits.is_empty() {
        return text.to_string();
    }

    let mut result = text.to_string();
    for edit in edits.into_iter().rev() {
        result.replace_range(edit.start..edit.end, &edit.replacement);
    }
    result
}

pub fn is_safe_format(original: &str, formatted: &str, parser: &mut Parser) -> bool {
    let Some(before) = parser.parse(original, None) else {
        return false;
    };
    let Some(after) = parser.parse(formatted, None) else {
        return false;
    };

    let before_root = before.root_node();
    let after_root = after.root_node();
    if before_root.has_error() || after_root.has_error() {
        return false;
    }
    if before_root.to_sexp() == after_root.to_sexp() {
        return true;
    }
    normalized_ast_shape(before_root, original) == normalized_ast_shape(after_root, formatted)
}

fn normalized_ast_shape(node: Node<'_>, text: &str) -> String {
    let mut shape = String::new();
    push_normalized_ast_shape(node, text, &mut shape);
    shape
}

fn push_normalized_ast_shape(node: Node<'_>, text: &str, shape: &mut String) {
    if is_if_then_do_wrapper(node)
        && let Some(statement) = statements::plain_do_body_statement(node, text)
    {
        push_normalized_ast_shape(statement, text, shape);
        return;
    }

    shape.push('(');
    shape.push_str(node.kind());
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        push_normalized_ast_shape(child, text, shape);
    }
    shape.push(')');
}

fn is_if_then_do_wrapper(node: Node<'_>) -> bool {
    if node.kind() != "do_statement" {
        return false;
    }
    let Some(parent) = node.parent() else {
        return false;
    };
    if parent.kind() != "if_statement" {
        return false;
    }
    let Some(then_statement) = parent.child_by_field_name("then") else {
        return false;
    };
    then_statement.start_byte() == node.start_byte() && then_statement.end_byte() == node.end_byte()
}

fn parse_abl_tree(text: &str) -> Option<tree_sitter::Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_abl::LANGUAGE.into())
        .ok()?;
    parser.parse(text, None)
}

#[cfg(test)]
mod tests {
    use super::{FormatterOptions, format_text, is_safe_format};

    #[test]
    fn formatting_is_idempotent_and_safe_for_indentation_changes() {
        let input = "IF TRUE THEN DO:\nMESSAGE \"X\".\nEND.\n";
        let formatted = format_text(input, FormatterOptions::default());
        assert_eq!(
            formatted,
            format_text(&formatted, FormatterOptions::default())
        );

        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_abl::LANGUAGE.into())
            .expect("set ABL language");
        assert!(is_safe_format(input, &formatted, &mut parser));
    }
}

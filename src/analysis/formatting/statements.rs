mod condition;
mod find;
mod for_each;
mod if_statement;
mod support;

use tree_sitter::Node;

use super::FormatterOptions;
use support::contains_unsafe_multiline_content;

#[derive(Debug)]
pub(super) struct LayoutEdit {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) replacement: String,
}

pub(super) fn plain_do_body_statement<'a>(node: Node<'a>, text: &str) -> Option<Node<'a>> {
    if_statement::plain_do_body_statement(node, text)
}

pub(super) fn layout_edits(
    root: Node<'_>,
    text: &str,
    options: FormatterOptions,
) -> Vec<LayoutEdit> {
    let mut edits = Vec::new();
    collect_layout_edits(root, text, options, &mut edits);
    edits
}

fn collect_layout_edits(
    node: Node<'_>,
    text: &str,
    options: FormatterOptions,
    edits: &mut Vec<LayoutEdit>,
) {
    let edit = layout_statement(node, text, options);
    let edit_end = edit.as_ref().map(|edit| edit.end);
    if let Some(edit) = edit {
        edits.push(edit);
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if edit_end.is_none_or(|end| child.end_byte() > end) {
            collect_layout_edits(child, text, options, edits);
        }
    }
}

fn layout_statement(node: Node<'_>, text: &str, options: FormatterOptions) -> Option<LayoutEdit> {
    match node.kind() {
        "find_statement" if !contains_unsafe_multiline_content(node) => {
            find::layout(node, text, options)
        }
        "if_statement" if !contains_unsafe_multiline_content(node) => {
            if_statement::layout(node, text, options)
        }
        "for_statement" => for_each::layout(node, text, options),
        _ => None,
    }
}

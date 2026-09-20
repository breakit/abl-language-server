use super::{FormatterOptions, format_text, is_safe_format};

pub(super) fn assert_format(input: &str, expected: &str, options: FormatterOptions) {
    assert_eq!(format_text(input, options), expected);
}

pub(super) fn assert_safe_format(input: &str, expected: &str, options: FormatterOptions) {
    let formatted = format_text(input, options);
    assert_eq!(formatted, expected);

    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_abl::LANGUAGE.into())
        .expect("set ABL language");
    assert!(is_safe_format(input, &formatted, &mut parser));
}

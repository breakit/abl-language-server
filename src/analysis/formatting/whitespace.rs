use super::{FormatterOptions, LineEnding};

pub(super) fn line_count(text: &str) -> usize {
    if text.is_empty() {
        return 1;
    }
    text.bytes().filter(|byte| *byte == b'\n').count() + 1
}

pub(super) fn render(
    text: &str,
    line_indents: &[usize],
    preserved_line_ending: &str,
    options: FormatterOptions,
) -> String {
    let mut output = String::with_capacity(text.len());
    let mut consecutive_blank_lines = 0usize;

    for (index, raw_line) in text.split_inclusive('\n').enumerate() {
        let (line_without_ending, line_ending) = split_line_ending(raw_line);
        let trimmed = line_without_ending.trim_start_matches([' ', '\t']);
        if trimmed.is_empty() {
            if consecutive_blank_lines < options.max_consecutive_blank_lines {
                output.push_str(select_line_ending(
                    line_ending,
                    preserved_line_ending,
                    options,
                ));
            }
            consecutive_blank_lines += 1;
            continue;
        }
        consecutive_blank_lines = 0;

        push_indent(
            &mut output,
            line_indents.get(index).copied().unwrap_or_default(),
            options,
        );
        if options.trim_trailing_whitespace {
            output.push_str(trimmed.trim_end_matches([' ', '\t']));
        } else {
            output.push_str(trimmed);
        }
        output.push_str(select_line_ending(
            line_ending,
            preserved_line_ending,
            options,
        ));
    }

    if options.trim_final_newlines {
        while output.ends_with('\n') {
            output.pop();
            if output.ends_with('\r') {
                output.pop();
            }
        }
    }
    if options.insert_final_newline && !output.is_empty() && !output.ends_with('\n') {
        output.push_str(select_line_ending(
            preserved_line_ending,
            preserved_line_ending,
            options,
        ));
    }
    output
}

pub(super) fn first_line_ending(text: &str) -> Option<&'static str> {
    let newline = text.find('\n')?;
    if newline > 0 && text.as_bytes()[newline - 1] == b'\r' {
        Some("\r\n")
    } else {
        Some("\n")
    }
}

fn split_line_ending(raw_line: &str) -> (&str, &str) {
    if let Some(stripped) = raw_line.strip_suffix("\r\n") {
        return (stripped, "\r\n");
    }
    if let Some(stripped) = raw_line.strip_suffix('\n') {
        return (stripped, "\n");
    }
    (raw_line, "")
}

fn select_line_ending<'a>(
    original: &'a str,
    preserved: &'a str,
    options: FormatterOptions,
) -> &'a str {
    if original.is_empty() {
        return original;
    }
    match options.line_ending {
        LineEnding::Preserve => preserved,
        LineEnding::Lf => "\n",
        LineEnding::Crlf => "\r\n",
    }
}

fn push_indent(output: &mut String, level: usize, options: FormatterOptions) {
    if options.use_tabs {
        output.extend(std::iter::repeat_n('\t', level));
        return;
    }
    output.extend(std::iter::repeat_n(' ', level * options.indent_size.max(1)));
}

#[cfg(test)]
mod tests {
    use super::super::{FormatterOptions, LineEnding, format_text};

    #[test]
    fn applies_conventional_defaults() {
        let input = "MESSAGE \"A\".   \n\n\nMESSAGE \"B\".\n\n";
        assert_eq!(
            format_text(input, FormatterOptions::default()),
            "MESSAGE \"A\".\n\nMESSAGE \"B\".\n"
        );
    }

    #[test]
    fn cleanup_can_be_disabled() {
        let input = "MESSAGE \"A\".   \n\n\nMESSAGE \"B\".";
        let options = FormatterOptions {
            trim_trailing_whitespace: false,
            insert_final_newline: false,
            trim_final_newlines: false,
            max_consecutive_blank_lines: usize::MAX,
            ..FormatterOptions::default()
        };
        assert_eq!(format_text(input, options), input);
    }

    #[test]
    fn normalizes_line_endings() {
        let options = FormatterOptions {
            line_ending: LineEnding::Crlf,
            ..FormatterOptions::default()
        };
        assert_eq!(
            format_text("MESSAGE \"A\".\nMESSAGE \"B\".", options),
            "MESSAGE \"A\".\r\nMESSAGE \"B\".\r\n"
        );
    }

    #[test]
    fn wrapping_preserves_crlf_document_style() {
        let input = "FIND FIRST audit_record WHERE audit_record.tenant = tenantId AND audit_record.category = categoryValue NO-LOCK.\r\n";
        let options = FormatterOptions {
            line_width: 60,
            ..FormatterOptions::default()
        };
        let expected = "FIND FIRST audit_record WHERE\r\n  audit_record.tenant = tenantId AND\r\n  audit_record.category = categoryValue\r\n  NO-LOCK.\r\n";
        assert_eq!(format_text(input, options), expected);
    }

    #[test]
    fn can_remove_the_final_newline() {
        let options = FormatterOptions {
            insert_final_newline: false,
            ..FormatterOptions::default()
        };
        assert_eq!(format_text("MESSAGE \"A\".\n\n", options), "MESSAGE \"A\".");
    }
}

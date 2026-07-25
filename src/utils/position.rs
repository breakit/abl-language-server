use tower_lsp::lsp_types::Position;

/// Converts an LSP Position to a UTF-8 byte offset in the text.
/// Assumes Position.character is a UTF-8 byte column within that line.
pub fn lsp_pos_to_utf8_byte_offset(text: &str, pos: Position) -> Option<usize> {
    let line = pos.line as usize;
    let col = pos.character as usize;

    let mut cur_line = 0usize;
    let mut line_start = 0usize;

    if line != 0 {
        let mut found = false;
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                cur_line += 1;
                if cur_line == line {
                    line_start = i + 1;
                    found = true;
                    break;
                }
            }
        }

        if !found {
            if cur_line == line {
                // Cursor can legally point at the final empty line after a trailing newline.
                line_start = text.len();
            } else {
                return None;
            }
        }
    }

    // find line end
    let line_end = text[line_start..]
        .find('\n')
        .map(|d| line_start + d)
        .unwrap_or(text.len());

    let target = line_start.saturating_add(col);
    if target > line_end {
        Some(line_end)
    } else if !text.is_char_boundary(target) {
        Some(target.saturating_sub(1))
    } else {
        Some(target)
    }
}

/// Walks backward from offset and captures [A-Za-z0-9_]* as prefix.
pub fn ascii_ident_prefix(text: &str, mut offset: usize) -> String {
    let bytes = text.as_bytes();
    if offset > bytes.len() {
        offset = bytes.len();
    }
    let mut start = offset;
    while start > 0 {
        let c = bytes[start - 1];
        let is_ident = c.is_ascii_alphanumeric() || c == b'_';
        if !is_ident {
            break;
        }
        start -= 1;
    }
    text[start..offset].to_string()
}

/// Returns the full ASCII identifier at the given offset or immediately before it.
pub fn ascii_ident_at_or_before(text: &str, mut offset: usize) -> Option<String> {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return None;
    }

    if offset > bytes.len() {
        offset = bytes.len();
    }

    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';

    let cursor = if offset < bytes.len() && is_ident(bytes[offset]) {
        offset
    } else if offset > 0 && is_ident(bytes[offset - 1]) {
        offset - 1
    } else {
        return None;
    };

    let mut start = cursor;
    while start > 0 && is_ident(bytes[start - 1]) {
        start -= 1;
    }

    let mut end = cursor + 1;
    while end < bytes.len() && is_ident(bytes[end]) {
        end += 1;
    }

    Some(text[start..end].to_string())
}

/// Returns a symbol at offset allowing dashes inside the token.
pub fn ascii_ident_or_dash_at_or_before(text: &str, mut offset: usize) -> Option<String> {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return None;
    }

    if offset > bytes.len() {
        offset = bytes.len();
    }

    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-';

    let cursor = if offset < bytes.len() && is_ident(bytes[offset]) {
        offset
    } else if offset > 0 && is_ident(bytes[offset - 1]) {
        offset - 1
    } else {
        return None;
    };

    let mut start = cursor;
    while start > 0 && is_ident(bytes[start - 1]) {
        start -= 1;
    }

    let mut end = cursor + 1;
    while end < bytes.len() && is_ident(bytes[end]) {
        end += 1;
    }

    Some(text[start..end].to_string())
}

/// Returns preprocessor name when cursor is on `{&NAME}`.
pub fn preprocessor_name_at_or_before(text: &str, mut offset: usize) -> Option<String> {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    if offset > bytes.len() {
        offset = bytes.len();
    }

    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-';
    let cursor = if offset < bytes.len() && is_ident(bytes[offset]) {
        offset
    } else if offset > 0 && is_ident(bytes[offset - 1]) {
        offset - 1
    } else {
        return None;
    };

    let mut start = cursor;
    while start > 0 && is_ident(bytes[start - 1]) {
        start -= 1;
    }
    if start < 2 || bytes[start - 2] != b'{' || bytes[start - 1] != b'&' {
        return None;
    }

    let mut end = cursor + 1;
    while end < bytes.len() && is_ident(bytes[end]) {
        end += 1;
    }

    Some(text[start..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        ascii_ident_at_or_before, ascii_ident_or_dash_at_or_before, ascii_ident_prefix,
        lsp_pos_to_utf8_byte_offset, preprocessor_name_at_or_before,
    };
    use tower_lsp::lsp_types::Position;

    #[test]
    fn extracts_prefix_and_identifiers() {
        let text = "abc f-lpd_det.xyz";
        assert_eq!(ascii_ident_prefix(text, 3), "abc");
        assert_eq!(ascii_ident_at_or_before(text, 3).as_deref(), Some("abc"));
        assert_eq!(
            ascii_ident_or_dash_at_or_before(text, 13).as_deref(),
            Some("f-lpd_det")
        );
    }

    #[test]
    fn position_to_offset_clamps_to_line_end() {
        let text = "abc\nxy";
        let off = lsp_pos_to_utf8_byte_offset(text, Position::new(1, 10)).expect("offset");
        assert_eq!(off, text.len());
    }

    #[test]
    fn position_to_offset_handles_final_empty_line() {
        let text = "abc\n";
        let off = lsp_pos_to_utf8_byte_offset(text, Position::new(1, 0)).expect("offset");
        assert_eq!(off, text.len());
    }

    #[test]
    fn position_to_offset_handles_multiple_trailing_empty_lines() {
        let text = "abc\n\n";
        let off = lsp_pos_to_utf8_byte_offset(text, Position::new(2, 0)).expect("offset");
        assert_eq!(off, text.len());
    }

    #[test]
    fn extracts_preprocessor_name_from_reference() {
        let text = r#"MESSAGE {&Test-Mode}."#;
        let off = text.find("Mode").expect("mode offset");
        assert_eq!(
            preprocessor_name_at_or_before(text, off).as_deref(),
            Some("Test-Mode")
        );
    }
}

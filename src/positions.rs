//! Document position mapping.
//!
//! LSP `Position.character` is counted in UTF-16 code units, while tree-sitter
//! works in bytes. `LineIndex` bridges both: it maps byte offsets in a text to
//! (line, character) pairs and back, handling multi-byte UTF-8 sequences.

use tower_lsp::lsp_types::Position;

/// Byte offsets of every line start in a text.
#[derive(Debug, Clone, Default)]
pub struct LineIndex {
    starts: Vec<usize>,
}

/// Number of UTF-16 code units in `s` (matches the LSP character metric).
pub fn utf16_len(s: &str) -> u32 {
    s.chars().map(|c| c.len_utf16() as u32).sum()
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let mut starts = vec![0usize];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                starts.push(i + 1);
            }
        }
        LineIndex { starts }
    }

    pub fn line_count(&self) -> u32 {
        self.starts.len() as u32
    }

    /// Line start byte offsets.
    pub fn starts(&self) -> &[usize] {
        &self.starts
    }

    /// Line index (0-based) containing `byte`.
    fn line_of(&self, byte: usize) -> usize {
        self.starts.partition_point(|&s| s <= byte) - 1
    }

    /// Converts a clamped byte offset into an LSP position.
    pub fn position_of(&self, text: &str, byte: usize) -> Position {
        let mut byte = byte.min(text.len());
        while !text.is_char_boundary(byte) {
            byte -= 1;
        }
        let line = self.line_of(byte);
        let line_start = self.starts[line];
        Position {
            line: line as u32,
            character: utf16_len(&text[line_start..byte]),
        }
    }

    /// Converts an LSP position back into a byte offset, if `line` exists.
    /// `character` counts UTF-16 code units; the result is always a char
    /// boundary. Out-of-range columns clamp to the end of the line.
    pub fn byte_of(&self, text: &str, position: Position) -> Option<usize> {
        let line = position.line as usize;
        let start = *self.starts.get(line)?;
        let end = self
            .starts
            .get(line + 1)
            .copied()
            .map(|s| s.saturating_sub(1).min(text.len()))
            .unwrap_or(text.len());
        let line_text = &text[start..end];

        let mut units = 0u32;
        for (i, ch) in line_text.char_indices() {
            if units >= position.character {
                return Some(start + i);
            }
            units += ch.len_utf16() as u32;
        }
        Some(start + line_text.len())
    }

    /// Converts a byte offset into a tree-sitter `Point` (row, byte column).
    /// The byte is clamped into `[0, text.len()]`.
    pub fn point_of(&self, text: &str, byte: usize) -> tree_sitter::Point {
        let byte = byte.min(text.len());
        let line = self.line_of(byte);
        tree_sitter::Point {
            row: line,
            column: byte - self.starts[line],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text() -> &'static str {
        "abc\ndef\n\nça va\n"
    }

    #[test]
    fn positions_round_trip() {
        let t = text();
        let idx = LineIndex::new(t);
        for (i, _) in t.char_indices().chain([(t.len(), ' ')]) {
            let pos = idx.position_of(t, i);
            let byte = idx.byte_of(t, pos).expect("byte_of must resolve");
            assert_eq!(byte, i, "round trip at byte {i}");
        }
        // Mid-char bytes floor to the preceding char boundary.
        let pos = idx.position_of(t, 10);
        assert_eq!(idx.byte_of(t, pos), Some(9));
    }

    #[test]
    fn utf16_character_counting() {
        let t = "é”; x";
        let idx = LineIndex::new(t);
        // é = 2 bytes, 1 utf16 unit; ” = 3 bytes, 1 utf16 unit.
        let pos = idx.position_of(t, 6);
        assert_eq!(pos.character, 3);
        assert_eq!(idx.byte_of(t, pos), Some(6));
    }

    #[test]
    fn column_beyond_line_end_clamps() {
        let t = "ab\ncd";
        let idx = LineIndex::new(t);
        assert_eq!(
            idx.byte_of(
                t,
                Position {
                    line: 0,
                    character: 50
                }
            ),
            Some(2)
        );
        assert_eq!(
            idx.byte_of(
                t,
                Position {
                    line: 9,
                    character: 0
                }
            ),
            None
        );
        assert_eq!(
            idx.position_of(t, t.len()),
            Position {
                line: 1,
                character: 2
            }
        );
    }
}

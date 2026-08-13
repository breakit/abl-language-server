//! WebSpeed section scanner.
//!
//! A WebSpeed `.htm`/`.html` file is a composite document: an HTML shell with
//! embedded SpeedScript blocks (`<% ... %>`, ABL procedures executed at render
//! time) and JavaScript blocks (`<script> ... </script>`, client-side).
//!
//! The scanner is a hand-written state machine over the raw bytes. It is
//! intentionally permissive: HTML between code blocks is opaque, and `%>` is
//! only honored outside SpeedScript `"..."` strings (with `~` escapes) and
//! `/* */` / `//` comments. An unterminated code block extends to EOF.

/// A section kind inside a WebSpeed document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionKind {
    /// Plain HTML text, opaque to this server.
    Html,
    /// SpeedScript block between `<%` and `%>`.
    SpeedScript,
    /// JavaScript block inside `<script>` / `</script>`.
    Javascript,
}

/// A contiguous region of a WebSpeed document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub kind: SectionKind,
    /// Byte range of the whole section, delimiters included.
    pub start: usize,
    pub end: usize,
    /// Byte range of the inner code content, delimiters excluded
    /// (Html sections span `start..end`).
    pub content_start: usize,
    pub content_end: usize,
}

impl Section {
    pub fn content<'a>(&self, doc: &'a str) -> &'a str {
        &doc[self.content_start..self.content_end]
    }
}

const OPEN: &[u8] = b"<%";
const CLOSE: &[u8] = b"%>";
const SCRIPT_OPEN: &[u8] = b"<script";
const SCRIPT_CLOSE: &[u8] = b"</script";

/// Case-insensitive substring search.
fn find_ci(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    debug_assert!(!needle.is_empty());
    if haystack.len() < from + needle.len() {
        return None;
    }
    (from..=haystack.len() - needle.len()).find(|&i| {
        haystack[i..i + needle.len()]
            .iter()
            .zip(needle)
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
    })
}

/// Byte-exact substring search.
fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    debug_assert!(!needle.is_empty());
    haystack
        .get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| i + from)
}

/// Whether the byte following an `<script` match makes it a tag start
/// (whitespace, `>`, `/`, or EOF) rather than e.g. `<scripting>`.
fn is_tag_boundary(bytes: &[u8], i: usize) -> bool {
    matches!(
        bytes.get(i),
        None | Some(b' ' | b'\t' | b'\r' | b'\n' | b'>' | b'/')
    )
}

/// Byte index just past the `>` that terminates the `<script ...>` open tag.
fn script_open_end(bytes: &[u8], open_at: usize) -> usize {
    match find(bytes, b">", open_at + SCRIPT_OPEN.len()) {
        Some(i) => i + 1,
        None => bytes.len(),
    }
}

/// Byte index of the `>` that terminates a matched `</script` close tag
/// (whitespace permitted between the name and `>`).
fn script_close_end(bytes: &[u8], close_at: usize) -> usize {
    let mut i = close_at + SCRIPT_CLOSE.len();
    while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\r' | b'\n') {
        i += 1;
    }
    if i < bytes.len() && bytes[i] == b'>' {
        i + 1
    } else {
        bytes.len()
    }
}

/// Finds the `%>` closing delimiter in SpeedScript content, ignoring
/// occurrences inside `"..."` strings (with `~` escapes), `/* */` block
/// comments and `//` line comments.
fn speedscript_close(bytes: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    let mut in_string = false;
    let mut in_block = false;
    let mut in_line = false;
    while i < bytes.len() {
        let b = bytes[i];
        if in_block {
            if b == b'*' && bytes.get(i + 1) == Some(&b'/') {
                in_block = false;
                i += 1;
            }
        } else if in_line {
            if b == b'\n' {
                in_line = false;
            }
        } else if in_string {
            if b == b'~' {
                i += 1;
            } else if b == b'"' {
                in_string = false;
            }
        } else {
            match b {
                b'"' => in_string = true,
                b'/' if bytes.get(i + 1) == Some(&b'*') => {
                    in_block = true;
                    i += 1;
                }
                b'/' if bytes.get(i + 1) == Some(&b'/') => {
                    in_line = true;
                    i += 1;
                }
                _ if bytes[i..].starts_with(CLOSE) => return Some(i),
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// Splits a WebSpeed document into sections.
///
/// Produces a complete, gap-free coverage of `source`: Html sections fill the
/// gaps between code blocks (empty gaps are omitted). Sections are sorted by
/// `start`.
pub fn scan(source: &str) -> Vec<Section> {
    let bytes = source.as_bytes();
    let n = bytes.len();
    let mut sections: Vec<Section> = Vec::new();
    let mut pos = 0usize;

    while pos < n {
        let open = find(bytes, OPEN, pos);
        let script_open = find_ci(bytes, SCRIPT_OPEN, pos);

        let (kind, start): (SectionKind, usize) = match (open, script_open) {
            (Some(o), Some(s)) => {
                if is_tag_boundary(bytes, s + SCRIPT_OPEN.len()) && s < o {
                    (SectionKind::Javascript, s)
                } else {
                    (SectionKind::SpeedScript, o)
                }
            }
            (Some(o), None) => (SectionKind::SpeedScript, o),
            (None, Some(s)) if is_tag_boundary(bytes, s + SCRIPT_OPEN.len()) => {
                (SectionKind::Javascript, s)
            }
            _ => break,
        };

        if start > pos {
            sections.push(Section {
                kind: SectionKind::Html,
                start: pos,
                end: start,
                content_start: pos,
                content_end: start,
            });
        }

        let section = match kind {
            SectionKind::SpeedScript => {
                let open_end = start + OPEN.len();
                match speedscript_close(bytes, open_end) {
                    Some(c) => Section {
                        kind,
                        start,
                        end: c + CLOSE.len(),
                        content_start: open_end,
                        content_end: c,
                    },
                    None => Section {
                        kind,
                        start,
                        end: n,
                        content_start: open_end,
                        content_end: n,
                    },
                }
            }
            SectionKind::Javascript => {
                let content_start = script_open_end(bytes, start);
                if content_start >= n {
                    Section {
                        kind,
                        start,
                        end: n,
                        content_start,
                        content_end: n,
                    }
                } else {
                    match find_ci(bytes, SCRIPT_CLOSE, content_start) {
                        Some(c) => Section {
                            kind,
                            start,
                            end: script_close_end(bytes, c),
                            content_start,
                            content_end: c,
                        },
                        None => Section {
                            kind,
                            start,
                            end: n,
                            content_start,
                            content_end: n,
                        },
                    }
                }
            }
            SectionKind::Html => unreachable!(),
        };
        pos = section.end;
        sections.push(section);
    }

    if pos < n {
        sections.push(Section {
            kind: SectionKind::Html,
            start: pos,
            end: n,
            content_start: pos,
            content_end: n,
        });
    }

    sections
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(sections: &[Section]) -> Vec<SectionKind> {
        sections.iter().map(|s| s.kind).collect()
    }

    #[test]
    fn single_speedscript_block() {
        let src = "<html><body><% message \"hi\". %>world</body></html>";
        let s = scan(src);
        assert_eq!(
            kinds(&s),
            vec![
                SectionKind::Html,
                SectionKind::SpeedScript,
                SectionKind::Html
            ]
        );
        let sp = &s[1];
        assert_eq!(&src[sp.content_start..sp.content_end], " message \"hi\". ");
        assert_eq!(&src[sp.start..sp.end], "<% message \"hi\". %>");
        assert_eq!(sp.start, 12);
        assert_eq!(sp.end, 31);
    }

    #[test]
    fn speedscript_close_inside_string_and_comments_is_skipped() {
        let src = "<% message \"%> not a close\". /* %> also not */ // %> nor this\n%>";
        let s = scan(src);
        assert_eq!(kinds(&s), vec![SectionKind::SpeedScript]);
        assert_eq!(s[0].end, src.len());
        assert!(src[..s[0].content_end].ends_with("\n"));
    }

    #[test]
    fn close_inside_escaped_string_is_skipped() {
        let src = "x<% message \"a %> b\". %>y<% message \"c\". %>z";
        let s = scan(src);
        assert_eq!(
            kinds(&s),
            vec![
                SectionKind::Html,
                SectionKind::SpeedScript,
                SectionKind::Html,
                SectionKind::SpeedScript,
                SectionKind::Html
            ]
        );
        assert_eq!(
            &src[s[1].content_start..s[1].content_end],
            " message \"a %> b\". "
        );
        assert_eq!(s[1].end, s[2].start);
        assert_eq!(&src[s[2].content_start..s[2].content_end], "y");
        assert_eq!(
            &src[s[3].content_start..s[3].content_end],
            " message \"c\". "
        );
    }

    #[test]
    fn script_and_speedscript_interleaved() {
        let src = "<script>var a = 1; // <% not a block\nfoo();</script><% run x.p. %><script>bar()</script>";
        let s = scan(src);
        assert_eq!(
            kinds(&s),
            vec![
                SectionKind::Javascript,
                SectionKind::SpeedScript,
                SectionKind::Javascript
            ]
        );
        assert_eq!(
            &src[s[0].content_start..s[0].content_end],
            "var a = 1; // <% not a block\nfoo();"
        );
        assert_eq!(&src[s[1].content_start..s[1].content_end], " run x.p. ");
        assert_eq!(&src[s[2].content_start..s[2].content_end], "bar()");
    }

    #[test]
    fn script_with_attributes() {
        let src = "<!--x--><script type=\"text/javascript\" src=\"a.js\"></script><p>t</p>";
        let s = scan(src);
        assert_eq!(
            kinds(&s),
            vec![
                SectionKind::Html,
                SectionKind::Javascript,
                SectionKind::Html
            ]
        );
        let js = &s[1];
        assert_eq!(
            &src[js.start..js.end],
            "<script type=\"text/javascript\" src=\"a.js\"></script>"
        );
        assert_eq!(js.content_start, js.content_end);
    }

    #[test]
    fn uppercase_script_tags() {
        let src = "<SCRIPT>let x = 1;</SCRIPT><% message \"a\". %>";
        let s = scan(src);
        assert_eq!(
            kinds(&s),
            vec![SectionKind::Javascript, SectionKind::SpeedScript]
        );
        assert_eq!(&src[s[0].content_start..s[0].content_end], "let x = 1;");
    }

    #[test]
    fn comment_wrapped_speedscript() {
        let src = "<!--<% message \"hi\". %>-->";
        let s = scan(src);
        assert_eq!(
            kinds(&s),
            vec![
                SectionKind::Html,
                SectionKind::SpeedScript,
                SectionKind::Html
            ]
        );
        assert_eq!(s[0].end, 4);
        assert_eq!(&src[s[1].start..s[1].end], "<% message \"hi\". %>");
        assert_eq!(s[2].start, s[1].end);
    }

    #[test]
    fn unterminated_blocks_extend_to_eof() {
        assert_eq!(scan("a<% let").len(), 2);
        assert_eq!(scan("<script>let x").len(), 1);
        let src = "<% message \"hi\". ";
        let s = scan(src);
        assert_eq!(s[0].end, src.len());
        assert_eq!(s[0].content_end, src.len());
    }

    #[test]
    fn no_code_blocks() {
        assert_eq!(
            kinds(&scan("<html><body>plain</body></html>")),
            vec![SectionKind::Html]
        );
    }

    #[test]
    fn coverage_is_complete() {
        let srcs = [
            "<html><body><% a. %><script>s()</script></body></html>",
            "a",
            "",
            "<%x%>",
            "<script></script>",
            "<!--<%a%>--><script>/*%>*/</script><%b%>",
        ];
        for src in srcs {
            let s = scan(src);
            let mut prev_end = 0;
            for sec in &s {
                assert_eq!(sec.start, prev_end, "no gap before {sec:?} in {src:?}");
                if sec.kind == SectionKind::Html {
                    assert_eq!(sec.content_start, sec.start);
                    assert_eq!(sec.content_end, sec.end);
                } else {
                    assert!(sec.content_start >= sec.start);
                    assert!(sec.content_end <= sec.end);
                }
                prev_end = sec.end;
            }
            assert_eq!(prev_end, src.len(), "coverage end mismatch for {src:?}");
        }
    }
}

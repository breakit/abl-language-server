//! Whole-document formatting.
//!
//! Applies `abl_printer` to each SpeedScript section only; HTML and JavaScript
//! pass through untouched. The optional idempotence check formats twice and
//! only applies the edit if both passes agree (matching the ecosystem's
//! corpus-proven safety stance).

use tower_lsp::lsp_types::*;

use crate::document::WebSpeedDocument;
use crate::scanner::SectionKind;

/// Formats the document; returns a single full-document edit when any
/// SpeedScript section changed.
pub fn format_document(doc: &WebSpeedDocument, idempotence: bool) -> Option<Vec<TextEdit>> {
    let options = abl_printer::Options::default();
    let mut edits: Vec<TextEdit> = Vec::new();
    for (idx, sec) in doc.sections().iter().enumerate() {
        if sec.kind != SectionKind::SpeedScript {
            continue;
        }
        let content = sec.content(&doc.text);
        let formatted = abl_printer::format_source(content, &options);
        if formatted == content {
            continue;
        }
        if idempotence {
            let again = abl_printer::format_source(&formatted, &options);
            if again != formatted {
                log::debug!("idempotence check failed; skipping section {idx}");
                continue;
            }
        }
        edits.push(TextEdit {
            range: Range {
                start: doc.point_to_position(sec.content_start),
                end: doc.point_to_position(sec.content_end),
            },
            new_text: formatted,
        });
    }
    if edits.is_empty() { None } else { Some(edits) }
}

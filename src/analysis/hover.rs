use dashmap::DashMap;
use tower_lsp::lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind};
use tree_sitter::Node;

use crate::analysis::buffers::collect_buffer_mappings;
use crate::analysis::functions::FunctionSignature;
use crate::analysis::local_tables::collect_local_table_definitions;
use crate::backend::DbFieldInfo;
use crate::utils::ts::node_trimmed_text;

#[derive(Clone)]
pub struct DbFieldMatch {
    pub table: String,
    pub field: DbFieldInfo,
}

pub fn symbol_at_offset(root: Node<'_>, text: &str, offset: usize) -> Option<String> {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return None;
    }

    let mut probes = Vec::with_capacity(2);
    let clamped = offset.min(bytes.len().saturating_sub(1));
    probes.push(clamped);
    if clamped > 0 {
        probes.push(clamped - 1);
    }

    for probe in probes {
        let Some(node) = root.named_descendant_for_byte_range(probe, probe) else {
            continue;
        };
        if node.kind() == "identifier" {
            return node_trimmed_text(node, bytes);
        }
    }

    None
}

pub fn is_comment_offset(root: Node<'_>, offset: usize) -> bool {
    let Some(mut node) = root.descendant_for_byte_range(offset, offset) else {
        return false;
    };

    loop {
        if matches!(node.kind(), "comment" | "block_comment" | "line_comment") {
            return true;
        }

        let Some(parent) = node.parent() else {
            return false;
        };
        node = parent;
    }
}

pub fn markdown_hover(markdown: String) -> Hover {
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: markdown,
        }),
        range: None,
    }
}

pub fn function_signature_hover(sig: &FunctionSignature) -> Hover {
    let header = match sig.return_type {
        Some(ref ret) => format!(
            "`FUNCTION {}({}) RETURNS {}`",
            sig.name,
            sig.parameters
                .iter()
                .map(|param| param.label.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            ret
        ),
        None => format!(
            "`FUNCTION {}({})`",
            sig.name,
            sig.parameters
                .iter()
                .map(|param| param.label.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };

    let mut sections = vec![header];
    if let Some(description) = &sig.documentation.description {
        sections.push(description.clone());
    }
    if sig
        .parameters
        .iter()
        .any(|param| param.documentation.is_some())
    {
        let params = sig
            .parameters
            .iter()
            .filter_map(|param| {
                param
                    .documentation
                    .as_ref()
                    .map(|doc| format!("- `{}`: {}", param.name, doc))
            })
            .collect::<Vec<_>>()
            .join("\n");
        if !params.is_empty() {
            sections.push(format!("**Parameters**\n{params}"));
        }
    }
    if let Some(returns) = &sig.documentation.returns {
        sections.push(format!("**Returns**\n{returns}"));
    }

    markdown_hover(sections.join("\n\n"))
}

pub fn find_db_field_matches(
    db_fields_by_table: &DashMap<String, Vec<DbFieldInfo>>,
    field_upper: &str,
) -> Vec<DbFieldMatch> {
    let mut out = Vec::new();
    for entry in db_fields_by_table.iter() {
        let table = entry.key();
        let fields = entry.value();
        for field in fields {
            if field.name.eq_ignore_ascii_case(field_upper) {
                out.push(DbFieldMatch {
                    table: table.clone(),
                    field: field.clone(),
                });
            }
        }
    }
    out.sort_by(|a, b| a.table.cmp(&b.table));
    out
}

pub fn find_db_table_field_hover(
    root: Node<'_>,
    text: &str,
    offset: usize,
    db_fields_by_table: &DashMap<String, Vec<DbFieldInfo>>,
) -> Option<Hover> {
    let (qualifier_upper, field_upper, field_display) =
        extract_qualified_field_at_offset(text, offset)?;

    let mut mappings = Vec::new();
    collect_buffer_mappings(root, text.as_bytes(), &mut mappings);
    let table_symbol = mappings
        .into_iter()
        .find(|mapping| mapping.alias.eq_ignore_ascii_case(&qualifier_upper))
        .map(|mapping| mapping.table.trim().to_string())
        .unwrap_or(qualifier_upper);

    let mut matches =
        find_db_field_matches_for_table(db_fields_by_table, &table_symbol, &field_upper, true);
    if matches.is_empty() {
        matches =
            find_db_field_matches_for_table(db_fields_by_table, &table_symbol, &field_upper, false);
    }

    if matches.len() == 1 {
        return Some(db_field_hover(&matches[0]));
    }
    if matches.is_empty() {
        return None;
    }

    let preview = matches
        .iter()
        .take(8)
        .map(|matched| format!("- `{}`", matched.table))
        .collect::<Vec<_>>()
        .join("\n");
    let suffix = if matches.len() > 8 { "\n- ..." } else { "" };
    Some(markdown_hover(format!(
        "**DB Field** `{}`\n\nFound in matching tables:\n{}{}",
        field_display, preview, suffix
    )))
}

fn find_db_field_matches_for_table(
    db_fields_by_table: &DashMap<String, Vec<DbFieldInfo>>,
    table_symbol: &str,
    field_symbol: &str,
    require_exact_table: bool,
) -> Vec<DbFieldMatch> {
    let table_tail = unqualified_table_name(table_symbol);
    let mut matches = db_fields_by_table
        .iter()
        .filter(|entry| {
            if require_exact_table {
                entry.key().eq_ignore_ascii_case(table_symbol)
            } else {
                unqualified_table_name(entry.key()).eq_ignore_ascii_case(table_tail)
            }
        })
        .flat_map(|entry| {
            entry
                .value()
                .iter()
                .filter(|field| field.name.eq_ignore_ascii_case(field_symbol))
                .cloned()
                .map(|field| DbFieldMatch {
                    table: entry.key().clone(),
                    field,
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    matches.sort_by(|a, b| a.table.cmp(&b.table));
    matches
}

fn unqualified_table_name(name: &str) -> &str {
    name.trim().rsplit('.').next().unwrap_or_default().trim()
}

pub fn db_field_hover(matched: &DbFieldMatch) -> Hover {
    let mut lines = vec![format!("**DB Field** `{}`", matched.field.name)];
    lines.push(format!("Table: `{}`", matched.table));
    if let Some(ty) = &matched.field.field_type {
        lines.push(format!("Type: `{}`", ty));
    }
    if let Some(label) = &matched.field.label {
        lines.push(format!("Label: {}", label));
    }
    if let Some(format) = &matched.field.format {
        lines.push(format!("Format: {}", format));
    }
    if let Some(desc) = &matched.field.description {
        lines.push(format!("Description: {}", desc));
    }
    markdown_hover(lines.join("\n\n"))
}

pub fn find_local_table_field_hover(root: Node<'_>, text: &str, offset: usize) -> Option<Hover> {
    let (qualifier_upper, field_upper, field_display) =
        extract_qualified_field_at_offset(text, offset)?;
    let src = text.as_bytes();

    let mut local_defs = Vec::new();
    collect_local_table_definitions(root, src, &mut local_defs);

    let mut table_upper = Some(qualifier_upper.clone());
    if !local_defs.iter().any(|d| d.name_upper == qualifier_upper) {
        let mut mappings = Vec::new();
        collect_buffer_mappings(root, src, &mut mappings);
        table_upper = mappings
            .into_iter()
            .find(|m| m.alias.eq_ignore_ascii_case(&qualifier_upper))
            .map(|m| m.table.trim().to_ascii_uppercase());
    }

    let table_upper = table_upper?;
    let def = local_defs
        .into_iter()
        .find(|d| d.name_upper == table_upper)?;
    let field = def
        .fields
        .into_iter()
        .find(|f| f.name.eq_ignore_ascii_case(&field_upper))?;

    let mut lines = vec![format!("**Local Field** `{}`", field_display)];
    lines.push(format!("Table: `{}`", table_upper));
    if let Some(ty) = &field.field_type {
        lines.push(format!("Type: `{}`", ty));
    }

    Some(markdown_hover(lines.join("\n\n")))
}

pub fn find_local_table_field_hover_by_symbol(
    root: Node<'_>,
    text: &str,
    symbol: &str,
) -> Option<Hover> {
    let mut local_defs = Vec::new();
    collect_local_table_definitions(root, text.as_bytes(), &mut local_defs);
    let symbol_upper = symbol.to_ascii_uppercase();

    let mut matches = Vec::new();
    for def in local_defs {
        for field in def.fields {
            if field.name.eq_ignore_ascii_case(&symbol_upper) {
                matches.push((def.name_upper.clone(), field));
            }
        }
    }

    if matches.is_empty() {
        return None;
    }

    if matches.len() == 1 {
        let (table, field) = &matches[0];
        let mut lines = vec![format!("**Local Field** `{}`", field.name)];
        lines.push(format!("Table: `{}`", table));
        if let Some(ty) = &field.field_type {
            lines.push(format!("Type: `{}`", ty));
        }
        return Some(markdown_hover(lines.join("\n\n")));
    }

    let preview = matches
        .iter()
        .take(8)
        .map(|(table, _)| format!("- `{}`", table))
        .collect::<Vec<_>>()
        .join("\n");
    let suffix = if matches.len() > 8 { "\n- ..." } else { "" };
    Some(markdown_hover(format!(
        "**Local Field** `{}`\n\nFound in tables:\n{}{}",
        symbol, preview, suffix
    )))
}

fn extract_qualified_field_at_offset(
    text: &str,
    offset: usize,
) -> Option<(String, String, String)> {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return None;
    }

    let mut i = offset.min(bytes.len().saturating_sub(1));
    if !is_ident_char(bytes[i]) {
        if i == 0 || !is_ident_char(bytes[i - 1]) {
            return None;
        }
        i -= 1;
    }

    let mut field_start = i;
    while field_start > 0 && is_ident_char(bytes[field_start - 1]) {
        field_start -= 1;
    }
    let mut field_end = i + 1;
    while field_end < bytes.len() && is_ident_char(bytes[field_end]) {
        field_end += 1;
    }
    if field_start == field_end {
        return None;
    }
    if field_start == 0 || bytes[field_start - 1] != b'.' {
        return None;
    }

    let field_display = text[field_start..field_end].to_string();
    let field_upper = field_display.to_ascii_uppercase();

    let q_end = field_start - 1;
    if q_end == 0 {
        return None;
    }
    let mut q_start = q_end;
    while q_start > 0 && is_ident_char(bytes[q_start - 1]) {
        q_start -= 1;
    }
    if q_start == q_end {
        return None;
    }
    let qualifier_upper = text[q_start..q_end].to_ascii_uppercase();
    if qualifier_upper.is_empty() {
        return None;
    }

    Some((qualifier_upper, field_upper, field_display))
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')
}

#[cfg(test)]
mod tests {
    use super::{
        extract_qualified_field_at_offset, find_db_field_matches, find_db_table_field_hover,
        find_local_table_field_hover_by_symbol, symbol_at_offset,
    };
    use crate::analysis::parse_abl;
    use crate::backend::DbFieldInfo;
    use dashmap::DashMap;
    use tower_lsp::lsp_types::HoverContents;

    #[test]
    fn finds_local_table_field_hover_by_symbol() {
        let src = r#"
DEFINE TEMP-TABLE ZM_CENY NO-UNDO
  FIELD ZM_CENY_KOD AS CHARACTER
  FIELD ZM_CENY_START AS DATE.
"#;
        let tree = parse_abl(src);

        let hover = find_local_table_field_hover_by_symbol(tree.root_node(), src, "ZM_CENY_KOD")
            .expect("field hover");

        let HoverContents::Markup(markup) = hover.contents else {
            panic!("expected markdown hover");
        };
        assert!(markup.value.contains("ZM_CENY_KOD"));
        assert!(markup.value.contains("ZM_CENY"));
    }

    #[test]
    fn symbol_at_offset_prefers_actual_index_identifier() {
        let src = "FOR EACH ZM_PRICES NO-LOCK USE-INDEX PRICE_IDX:\nEND.\n";
        let tree = parse_abl(src);
        let offset = src.find("PRICE_IDX").expect("index offset") + 2;

        assert_eq!(
            symbol_at_offset(tree.root_node(), src, offset).as_deref(),
            Some("PRICE_IDX")
        );
    }

    #[test]
    fn extracts_symbol_and_qualified_field_at_offset() {
        let src = "DISPLAY ttCustomer.name.";
        let tree = parse_abl(src);
        let symbol = symbol_at_offset(
            tree.root_node(),
            src,
            src.find("name").expect("field offset") + 1,
        )
        .expect("symbol");
        assert_eq!(symbol, "name");

        let qualified = extract_qualified_field_at_offset(src, src.find("name").expect("offset"))
            .expect("qualified");
        assert_eq!(qualified.0, "TTCUSTOMER");
        assert_eq!(qualified.1, "NAME");
        assert_eq!(qualified.2, "name");
    }

    #[test]
    fn detects_offsets_inside_comments() {
        let src = "/* comment with source-string */\nFUNCTION foo RETURNS INTEGER () FORWARD.\n";
        let tree = parse_abl(src);
        let offset = src.find("source-string").expect("comment text") + 2;

        assert!(super::is_comment_offset(tree.root_node(), offset));
    }

    #[test]
    fn finds_db_field_matches_across_tables() {
        let map = DashMap::<String, Vec<DbFieldInfo>>::new();
        map.insert(
            "Customer".to_string(),
            vec![DbFieldInfo {
                name: "Name".to_string(),
                field_type: Some("CHARACTER".to_string()),
                format: None,
                label: None,
                description: None,
            }],
        );
        map.insert(
            "Order".to_string(),
            vec![DbFieldInfo {
                name: "name".to_string(),
                field_type: Some("CHARACTER".to_string()),
                format: None,
                label: None,
                description: None,
            }],
        );

        let matches = find_db_field_matches(&map, "NAME");
        assert_eq!(matches.len(), 2);
        assert!(matches.iter().any(|m| m.table == "Customer"));
        assert!(matches.iter().any(|m| m.table == "Order"));
    }

    #[test]
    fn qualified_db_field_hover_uses_its_table() {
        let src = "DISPLAY Debtor.LastModifiedDate.";
        let tree = parse_abl(src);
        let map = DashMap::<String, Vec<DbFieldInfo>>::new();
        for table in ["Debtor", "Creditor"] {
            map.insert(
                table.to_string(),
                vec![DbFieldInfo {
                    name: "LastModifiedDate".to_string(),
                    field_type: Some("DATETIME-TZ".to_string()),
                    format: None,
                    label: None,
                    description: None,
                }],
            );
        }

        let offset = src.find("LastModifiedDate").expect("field offset") + 2;
        let hover =
            find_db_table_field_hover(tree.root_node(), src, offset, &map).expect("field hover");
        let HoverContents::Markup(markup) = hover.contents else {
            panic!("expected markdown hover");
        };

        assert!(markup.value.contains("Table: `Debtor`"));
        assert!(markup.value.contains("Type: `DATETIME-TZ`"));
        assert!(!markup.value.contains("Found in tables"));
        assert!(!markup.value.contains("Creditor"));
    }

    #[test]
    fn qualified_db_field_hover_resolves_buffer_alias() {
        let src = r#"
DEFINE BUFFER bDebtor FOR Debtor.
DISPLAY bDebtor.LastModifiedDate.
"#;
        let tree = parse_abl(src);
        let map = DashMap::<String, Vec<DbFieldInfo>>::new();
        map.insert(
            "Debtor".to_string(),
            vec![DbFieldInfo {
                name: "LastModifiedDate".to_string(),
                field_type: Some("DATETIME-TZ".to_string()),
                format: None,
                label: None,
                description: None,
            }],
        );

        let offset = src.rfind("LastModifiedDate").expect("field offset") + 2;
        let hover =
            find_db_table_field_hover(tree.root_node(), src, offset, &map).expect("field hover");
        let HoverContents::Markup(markup) = hover.contents else {
            panic!("expected markdown hover");
        };

        assert!(markup.value.contains("Table: `Debtor`"));
        assert!(markup.value.contains("Type: `DATETIME-TZ`"));
    }
}

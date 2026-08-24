use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;

use crate::analysis::buffers::collect_buffer_mappings;
use crate::analysis::completion::lookup_case_insensitive_indexes_by_table;
use crate::analysis::definition::{
    resolve_include_definition_location, resolve_include_directive_location,
    resolve_preprocessor_define_match,
};
use crate::analysis::definitions::collect_definition_symbols;
use crate::analysis::functions::{
    find_function_signature, find_function_signature_from_ambient,
    find_function_signature_from_includes,
};
use crate::analysis::hover::{
    db_field_hover, find_db_field_matches, find_db_table_field_hover, find_local_table_field_hover,
    find_local_table_field_hover_by_symbol, function_signature_hover, is_comment_offset,
    markdown_hover, symbol_at_offset,
};
use crate::analysis::includes::{
    collect_include_sites_from_tree, include_site_matches_file_offset,
};
use crate::analysis::schema::normalize_lookup_key;
use crate::analysis::schema_lookup::has_schema_key;
use crate::backend::Backend;
use crate::utils::position::{
    ascii_ident_at_or_before, ascii_ident_or_dash_at_or_before, lsp_pos_to_utf8_byte_offset,
    preprocessor_name_at_or_before,
};

impl Backend {
    pub async fn handle_hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;

        let text = match self.get_document_text(&uri) {
            Some(t) => t,
            None => return Ok(None),
        };
        let tree = match self.get_document_tree_or_parse(&uri) {
            Some(t) => t,
            None => return Ok(None),
        };

        let offset = match lsp_pos_to_utf8_byte_offset(&text, pos) {
            Some(o) => o,
            None => return Ok(None),
        };

        if is_comment_offset(tree.root_node(), offset) {
            return Ok(None);
        }

        if let Some(location) =
            resolve_include_directive_location(self, &uri, &text, tree.root_node(), offset).await
        {
            let path_display = location
                .uri
                .to_file_path()
                .ok()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| location.uri.to_string());
            let include_site = collect_include_sites_from_tree(tree.root_node(), text.as_bytes())
                .into_iter()
                .find(|site| include_site_matches_file_offset(site, offset));

            let mut lines = vec![
                "**Include File**".to_string(),
                format!("Path: `{}`", path_display),
            ];

            if let Some(site) = include_site
                && let Some(include_text) = text.get(site.start_offset..site.end_offset)
            {
                let preprocessor_names = extract_preprocessor_names_from_include_text(include_text);
                if !preprocessor_names.is_empty() {
                    lines.push("Preprocessors:".to_string());
                    for name in preprocessor_names {
                        if let Some(matched) = resolve_preprocessor_define_match(
                            self,
                            &uri,
                            &text,
                            tree.root_node(),
                            &name,
                            site.start_offset,
                        )
                        .await
                        {
                            let value = matched.value.unwrap_or_default();
                            lines.push(format!("- `{}` = `{}`", matched.name, value));
                        } else {
                            lines.push(format!("- `{}` = `<unresolved>`", name));
                        }
                    }
                }
            }

            return Ok(Some(markdown_hover(lines.join("\n\n"))));
        }

        let symbol = match symbol_at_offset(tree.root_node(), &text, offset).or_else(|| {
            ascii_ident_or_dash_at_or_before(&text, offset)
                .or_else(|| ascii_ident_at_or_before(&text, offset))
        }) {
            Some(s) => s,
            None => return Ok(None),
        };

        if let Some(macro_name) = preprocessor_name_at_or_before(&text, offset)
            && let Some(matched) = resolve_preprocessor_define_match(
                self,
                &uri,
                &text,
                tree.root_node(),
                &macro_name,
                offset,
            )
            .await
        {
            let kind = if matched.is_global {
                "&GLOBAL-DEFINE"
            } else {
                "&SCOPED-DEFINE"
            };
            let mut markdown = format!("**{}** `{}`", kind, matched.name);
            if let Some(value) = matched.value {
                markdown.push_str(&format!("\n\nValue: `{}`", value));
            }
            return Ok(Some(markdown_hover(markdown)));
        }
        let symbol_upper = normalize_lookup_key(&symbol, true);
        let use_index_table_key = self
            .resolve_use_index_table_key(&uri, &text, tree.root_node(), offset)
            .await;

        if let Some(table_key) = &use_index_table_key
            && let Some(indexes) =
                lookup_case_insensitive_indexes_by_table(&self.db_indexes_by_table, table_key)
            && let Some(index_name) = indexes
                .iter()
                .find(|idx| idx.eq_ignore_ascii_case(&symbol))
                .cloned()
        {
            let key = format!(
                "{}\u{1f}{}",
                table_key.to_ascii_uppercase(),
                index_name.to_ascii_uppercase()
            );
            let mut lines = vec![format!("**DB Index** `{}`", index_name)];
            lines.push(format!("Table: `{}`", table_key));
            if let Some(fields) = self.db_index_fields_by_table_index.get(&key)
                && !fields.is_empty()
            {
                lines.push("Fields (order):".to_string());
                for field in fields.iter() {
                    lines.push(format!("- `{field}`"));
                }
            }
            return Ok(Some(markdown_hover(lines.join("\n\n"))));
        }

        if let Some(sig) = find_function_signature(tree.root_node(), text.as_bytes(), &symbol) {
            return Ok(Some(function_signature_hover(&sig)));
        }
        if let Some(sig) = find_function_signature_from_includes(
            self,
            &uri,
            &text,
            tree.root_node(),
            offset,
            &symbol,
        )
        .await
        {
            return Ok(Some(function_signature_hover(&sig)));
        }
        if let Some(sig) = find_function_signature_from_ambient(self, &symbol).await {
            return Ok(Some(function_signature_hover(&sig)));
        }

        let mut defs = Vec::new();
        collect_definition_symbols(tree.root_node(), text.as_bytes(), &mut defs);
        if let Some(def) = defs
            .into_iter()
            .find(|d| d.label.eq_ignore_ascii_case(&symbol))
        {
            return Ok(Some(markdown_hover(format!(
                "**{}**\n\nType: `{}`",
                def.label, def.detail
            ))));
        }

        if let Some(location) = resolve_include_definition_location(
            self,
            &uri,
            &text,
            tree.root_node(),
            &symbol,
            offset,
        )
        .await
            && let Ok(path) = location.uri.to_file_path()
            && let Some((include_text, include_tree)) = self.get_cached_include_parse(&path).await
        {
            let mut include_defs = Vec::new();
            collect_definition_symbols(
                include_tree.root_node(),
                include_text.as_bytes(),
                &mut include_defs,
            );
            if let Some(def) = include_defs
                .into_iter()
                .find(|d| d.label.eq_ignore_ascii_case(&symbol))
            {
                return Ok(Some(markdown_hover(format!(
                    "**{}**\n\nType: `{}`\n\nDefined in include: `{}`",
                    def.label,
                    def.detail,
                    path.display()
                ))));
            }

            if let Some(field_hover) = find_local_table_field_hover_by_symbol(
                include_tree.root_node(),
                &include_text,
                &symbol,
            ) {
                return Ok(Some(field_hover));
            }
        }

        let mut buffers = Vec::new();
        collect_buffer_mappings(tree.root_node(), text.as_bytes(), &mut buffers);
        if let Some(buf) = buffers
            .into_iter()
            .find(|b| b.alias.eq_ignore_ascii_case(&symbol))
        {
            return Ok(Some(markdown_hover(format!(
                "**Buffer** `{}`\n\nFor table: `{}`",
                buf.alias, buf.table
            ))));
        }

        if has_schema_key(&self.db_table_definitions, &symbol_upper) {
            return Ok(Some(markdown_hover(format!("**DB Table** `{}`", symbol))));
        }

        if has_schema_key(&self.db_sequence_definitions, &symbol_upper) {
            return Ok(Some(markdown_hover(format!(
                "**DB Sequence** `{}`",
                symbol
            ))));
        }

        if let Some(local_field_hover) =
            find_local_table_field_hover(tree.root_node(), &text, offset)
        {
            return Ok(Some(local_field_hover));
        }
        if let Some(db_field_hover) =
            find_db_table_field_hover(tree.root_node(), &text, offset, &self.db_fields_by_table)
        {
            return Ok(Some(db_field_hover));
        }
        if let Some(local_field_hover) =
            find_local_table_field_hover_by_symbol(tree.root_node(), &text, &symbol)
        {
            return Ok(Some(local_field_hover));
        }

        let field_matches = find_db_field_matches(&self.db_fields_by_table, &symbol_upper);
        if !field_matches.is_empty() {
            if field_matches.len() == 1 {
                return Ok(Some(db_field_hover(&field_matches[0])));
            }

            let preview = field_matches
                .iter()
                .take(8)
                .map(|m| format!("- `{}`", m.table))
                .collect::<Vec<_>>()
                .join("\n");
            let suffix = if field_matches.len() > 8 {
                "\n- ..."
            } else {
                ""
            };
            return Ok(Some(markdown_hover(format!(
                "**DB Field** `{}`\n\nFound in tables:\n{}{}",
                symbol, preview, suffix
            ))));
        }

        if has_schema_key(&self.db_index_definitions, &symbol_upper) {
            return Ok(Some(markdown_hover(format!("**DB Index** `{}`", symbol))));
        }

        Ok(None)
    }
}

fn extract_preprocessor_names_from_include_text(include_text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0usize;
    let bytes = include_text.as_bytes();

    while i + 2 <= bytes.len() {
        if i + 2 < bytes.len() && bytes[i] == b'{' && bytes[i + 1] == b'&' {
            let mut j = i + 2;
            while j < bytes.len()
                && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_' || bytes[j] == b'-')
            {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'}' && j > i + 2 {
                let name = include_text[i + 2..j].to_string();
                if !out.iter().any(|n: &String| n.eq_ignore_ascii_case(&name)) {
                    out.push(name);
                }
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }

    out
}

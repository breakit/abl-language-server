# webspeed-language-server

Language Server Protocol implementation for OpenEdge WebSpeed `.htm`/`.html`
files, written in Rust. Part of the ABL tooling ecosystem (linter, formatter,
MCP server, and this language server).

A WebSpeed document is a **composite file** with three kinds of content:

1. **HTML** — the page shell (passes through untouched)
2. **SpeedScript** — ABL procedures between `<%` and `%>`, executed at render
   time; parsed with the shared `tree-sitter-abl` grammar
3. **JavaScript** — client-side code between `<script>` / `</script>`

The server splits the file into sections, parses SpeedScript with
[tree-sitter-abl](https://github.com/usagi-coffee/tree-sitter-abl) (pinned
`v0.0.52`, as in the reference
[abl-language-server](https://github.com/usagi-coffee/abl-language-server))
and JavaScript with `tree-sitter-javascript`, and maps every result back into
the original document coordinates.

## Features

| Feature | Notes |
|---|---|
| Text sync | `TextDocumentSyncKind::FULL` |
| Diagnostics | SpeedScript + JavaScript syntax errors, unknown-variable / unknown-function warnings (clean sections only), `abl-lint` findings on save |
| Completion | Local symbols (variables, parameters, buffers, properties, functions — case-insensitive) + ABL keywords, inside SpeedScript sections |
| Hover | Symbol definition lookup (type/detail) |
| Go to definition | Symbol definitions within SpeedScript sections |
| Semantic tokens | Keywords, strings, numbers, comments, definitions — delta-encoded |
| Formatting | `abl-printer` applied to SpeedScript sections only, guarded by an optional idempotence check |

## Ecosystem integration

The server consumes the ecosystem crates from the sibling
`abl-fmt`/`abl-lint` repos via path dependencies:

- `abl-parser` — shared ABL parsing (`abl_parser::parse`)
- `abl-printer` — formatting (`format_source`)
- `abl-config` — `.abl.toml` `[fmt]` options
- `abl-lint` — invoked as a binary (its rule engine has no library target
  yet); findings parsed from `path:line:col: rule [severity]: message` lines

## Configuration

The server looks for `<workspace-root>/abl.toml` (or `.config/abl.toml`).
All keys are optional:

```toml
# Include search roots (future use).
propath = ["includes"]

[fmt]                      # shared formatter options (abl-config)
print_width = 80
tab_width = 2
use_tabs = false
end_of_line = "lf"

[diagnostics]
enabled = true
unknown_variables = true
unknown_functions = true
lint = true                # run abl-lint on save
lint_binary = "abl-lint"   # path or PATH lookup

[completion]
enabled = true
keywords = true

[formatting]
enabled = true
idempotence = true         # format twice, apply only if both passes agree

[semantic_tokens]
enabled = true
```

## Editor setup

Point your LSP client at the binary (e.g. `target/release/webspeed-language-server`):

- **VS Code / Zed**: registers as `webspeed_language_server` over stdio;
  associate it with `htm`/`html` files (see the reference server's
  `vscode-openedge-abl` / `zed-openedge-abl` extensions for the pattern).

## Development

```sh
cargo build --release
cargo test                # unit tests + stdio smoke tests
cargo clippy --all-targets
```

The smoke tests in `tests/smoke.rs` spawn the real binary and drive it with
raw JSON-RPC frames (initialize → didOpen → publishDiagnostics → shutdown),
so no client is needed to verify end-to-end behavior.

## Known limitations

- Semantic analysis (unknown variables/functions) only runs on parse-clean
  sections; the pinned grammar is experimental (~47% clean parse rate), so
  recovery-tree noise is deliberately suppressed.
- HTML is opaque: no HTML structure diagnostics, no HTML-aware completion.
- Directory-level symbols / cross-file goto-definition, `.df` schema
  integration, and `{include.i}` resolution are future work (mirrored from
  the reference server's roadmap).

## License

MIT
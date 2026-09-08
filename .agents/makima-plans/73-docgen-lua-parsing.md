## Goal

Resolve #73 by replacing docgen's string-based bundled Lua command scanner with syntax-tree extraction. Static declarations must produce correct metadata, while unsupported declarations and parse failures must stop generation with actionable diagnostics instead of silently omitting commands.

## Implementation Summary

Use the existing workspace `tree-sitter` 0.26 and `tree-sitter-lua` 0.5 dependencies directly in `maki-docgen`. Replace the helpers in `maki-docgen/src/lua_util.rs`, propagate errors through `gen_commands.rs` and the page collection boundary in `main.rs`, and preserve the current generated command inventory.

This is static documentation extraction, not runtime registration or general Lua evaluation. Support Lua syntax accepted by the existing grammar; explicitly reject unsupported syntax, including Luau-only constructs, rather than extracting from a recovered/partial tree. The current 27 bundled init files were inspected and contain no observed Luau-only syntax. Runtime remains Luau. Alias resolution, variable propagation, computed metadata evaluation, and changes to other docgen scanners are out of scope.

## Implementation Plan

### 1. Replace string scanning with structural extraction

- Add `tree-sitter`, `tree-sitter-lua`, and `color-eyre` workspace dependencies to `maki-docgen/Cargo.toml`; update lockfile wiring as necessary. Use `color_eyre::Result` for this internal generator utility and contextual errors, not panics.
- Delete `find_matching_brace`, `extract_lua_field`, `extract_lua_bool`, and the existing simplistic unescaper after replacing their sole caller (`lua_util.rs:56`).
- Make `parse_lua_commands` fallible. Parse the entire source and reject ERROR/missing nodes before extracting commands. Report a one-based line/column and reason; the loader adds the source path.
- Walk actual `function_call` nodes, matching the canonical `maki.api.register_command` callee structurally. Support equivalent literal bracket member access as well as dotted access, with transparent parentheses where applicable. Ignore unrelated receivers, similarly named functions, comments, and strings. Aliases and arbitrary computed callees are not resolved and are outside the recognized-call contract.
- Accept exactly one inline table argument, either parenthesized or Lua's table-call shorthand. A recognized call with variable/function/computed arguments is an explicit unsupported-declaration error.
- Extract only immediate table fields: required literal strings `name` and `description`, required literal boolean `tui_only`, and optional literal string `argument_hint` (absent or literal nil means None). Accept identifier keys and bracketed string-literal keys. Ignore statically named unrelated fields such as handlers/completions without examining their values for metadata.
- Reject missing required fields, duplicate documented fields, wrong types, computed documented values, and computed table keys that could overwrite metadata. Do not evaluate expressions, even constant concatenation. Reject unkeyed entries in the registration table as unsupported instead of guessing their meaning. Nested tables/functions cannot supply missing outer metadata. Continue walking the syntax tree for independently nested registration calls.
- Implement a small literal decoder driven by string nodes, not source-wide searching. The grammar exposes `start`, optional `content`, and `end`; it does not decode values. Handle single/double quotes, empty strings, escaped quotes/backslashes, standard control escapes, decimal/hex byte escapes, Unicode escapes, escaped physical newlines, `\z` whitespace skipping, long-bracket delimiter levels, initial long-string newline removal, and Lua newline normalization. Accumulate bytes where needed and explicitly reject invalid escapes/ranges or non-UTF-8 metadata. Never execute plugin source or an expression to decode it.
- Useful cached grammar references: `tree-sitter-lua-0.5.0/grammar.js:410–457` (strings), `:499–548` (calls, arguments, table fields). These are available under the Cargo registry source directory.

### 2. Propagate failures without partial output

- Make `load_builtin_plugin_commands` return Result. Factor directory loading behind a path-taking internal helper for testing. Preserve immediate plugin-directory discovery, opt-in plugins, and name sorting. Missing `init.lua` remains a deliberate skip (`plugins/lib` has none); propagate other reads, invalid UTF-8, and enumeration failures with paths. Ignore non-directory entries.
- Change `gen_commands::generate` to Result<String>; use `?` at the loader boundary. Update its tests accordingly.
- Adapt `main.rs`'s `Page` callback to Result<String>, wrapping the six unchanged infallible generators with noncapturing `Ok(...)` closures. Collect all worker results before check/write operations, converting join failures into contextual errors rather than unwrap panics. On a generation error print the error chain and return failure for both normal and `--check` modes. Keep other generator implementations unchanged.
- Factor a mode-aware runner used directly by `main`, accepting page callbacks and output/diagnostic sinks for tests and returning the process status. Ensure any page error prevents all check/write callbacks. Exercise both normal and check modes in `generation_failure_prevents_output`, asserting failure status, path-bearing diagnostics, and zero output callbacks. Add `worker_panic_reports_generation_failure` to verify join failures produce contextual diagnostics and failure without output. This is a small test seam, not a new CLI option or generic generation framework.

### 3. Regression coverage and contributor guidance

- Add same-file, snake_case tests using `#[test_case]` for parser/decoder matrices and shared error-message constants for assertions. Cover positive declarations and all unsupported cases above, checking positions as well as error categories.
- Add directory-loader tests with isolated temporary fixtures using an existing workspace test utility if available, otherwise a small std-only unique temporary-directory fixture with cleanup; no sleeps or shared paths. Cover missing init files, bad source, invalid UTF-8, missing root, opt-in discovery, and sorting.
- Extend command-generation tests to assert every bundled command's rendered metadata, not only `/thinking`. Current sorted inventory: automode, build, memory, plan, rename, sessions, splash, splash-fps, thinking, usage. Pin expected literal metadata from the current sources so tests detect extraction regressions; do not generate expected values with the parser under test.
- Add a short contributor-facing note to the existing root `AGENTS.md` Docs section describing the static inline-table contract, grammar limitation, unsupported-expression errors, and `just gen-docs-check`. Do not add a new documentation file or change generated user docs merely to document internals.

## Acceptance Criteria

- **AC.1:** Equivalent supported Lua formatting, comments, table-call forms, literal field keys, nested handlers, and literal strings produce identical correct metadata. Verified by `parses_static_command_forms` and `decodes_lua_string_literals`.
- **AC.2:** Non-registration text/calls do not yield commands, and nested fields cannot masquerade as outer metadata. Verified by `ignores_non_registration_syntax` and `rejects_missing_outer_metadata`.
- **AC.3:** Recognized dynamic/invalid declarations and malformed or unsupported syntax fail explicitly with source position, and loader errors include the file path. Verified by `rejects_unsupported_command_metadata`, `rejects_invalid_source`, and `loader_reports_source_path`.
- **AC.4:** Discovery preserves all bundled/opt-in commands in sorted order, skips directories without init.lua, and reports other filesystem/data failures. Verified by `loads_plugin_directories`, `loader_rejects_invalid_utf8`, `loader_rejects_missing_root`, and `bundled_command_metadata_matches_expected`.
- **AC.5:** Generation errors reach the top-level failure path and prevent any documentation check/write callbacks. Verified by `generation_failure_prevents_output` and `generation_success_emits_all_pages` with injected callbacks/output sink.
- **AC.6:** All bundled sources parse, rendered bundled metadata remains correct, and checked-in generated docs stay current. Verified by `bundled_command_metadata_matches_expected`, `generated_plugin_rows_match_expected`, and `just gen-docs-check`.

## Test Strategy

Mappings are explicit in Acceptance Criteria. Parser and decoding tests are pure units; directory loading and the bundled-source inventory exercise the real filesystem/parser boundary; rendered-row tests exercise extraction through command-page generation. Add the callback/output test seam in phase 2 to prove no partial output, rather than relying on inspection of control flow.

Positive matrices include whitespace between callee/arguments, comments around fields, braces and fake registrations in strings/comments, quoted and long strings, CRLF normalization, escaped delimiters, false booleans, optional nil, and multiple/nested calls. Negative matrices include variable arguments, computed strings/booleans/keys, duplicate/missing fields, wrong types, invalid UTF-8 decoding, syntax errors, and representative Luau type annotations/compound assignments (explicit error, never partial success).

After implementation run cheapest first: `cargo check -p maki-docgen --tests`, `cargo clippy -p maki-docgen --tests -- -D warnings`, `cargo nextest run -p maki-docgen`, `cargo fmt --all -- --check`, then `just gen-docs-check`. Run repository-wide lint/tests if feasible after scoped checks. No builds/tests were run during read-only planning. No missing test infrastructure remains: required fixture/seam work is included above.

## Review Strategy

Before handoff, ask a plan_reviewer to review this artifact against #73 and fix or rebut findings; repeat if high/critical findings remain. After implementation and automatable tests, dispatch a general review subagent focused on AST matching, literal correctness, error propagation, unsupported-form behavior, and unnecessary scope growth. Fix or explicitly rebut all findings and repeat review for critical findings until resolved or operator-blocked.

## Documentation Strategy

Update only the existing contributor Docs guidance in `AGENTS.md` to make static extraction's supported contract and Lua-versus-Luau limitation discoverable. Generated `site/docs/content/commands/_index.md` should remain unchanged; verify through `just gen-docs-check`. No runtime API or end-user workflow changes are intended.

## Risks, Blockers, and Required Decisions

- Existing Lua grammar does not accept all runtime Luau syntax. Chosen policy is explicit whole-file failure with location, never best-effort recovery. Future full Luau support is a separate parser change; current corpus compatibility is enforced by tests.
- Canonical direct calls are recognized; aliases, shadowing, post-registration mutations, and control-flow reachability are not analyzed. This extracts declarations, not a predicted runtime registry. Document that boundary rather than adding an evaluator.
- Lua strings are byte sequences but documentation requires UTF-8. Decoder tests and explicit rejection prevent silently corrupted output.
- Stricter errors intentionally turn previous silent omissions into failed doc generation. All current bundled metadata is literal and explicitly includes tui_only, so no source migration is expected.
- No operator-only decisions or known blockers remain. Main-thread error propagation is slightly broader than the scanner replacement but needed to avoid introducing new panic-based error handling.

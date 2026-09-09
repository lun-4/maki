# Change default theme to `makima` (rename `lunared`, fix first-run default)

Closes https://github.com/lun-4/makima/issues/82 (+ follow-up comment: rename to `makima`).

## Goal

The bundled `lunared` theme becomes `makima` and is the real default theme: a fresh install (no persisted theme, no `ui.theme` config) boots with the makima palette, the theme picker highlights it, and no code keeps assuming the previous default (`dracula`).

## Implementation Summary

Three parts, all verified by investigation:

1. **Rename `lunared` → `makima`.** The theme is a bundled TOML named by file stem (`maki-ui/build.rs` reads `src/themes/*.toml`, `maki-ui/src/theme.rs:131-132` includes the generated catalog). Rename `maki-ui/src/themes/lunared.toml`, update the two tests that load it by name (`maki-ui/src/components/file_completion.rs:1814,2003`), and rename the docs-site theme of the same name (`site/docs/extra/lunared.json` + `site/docs/config.toml:9-10`, referenced from `site/docs/DESIGN.md:27`).

2. **Make it the default, and fix the latent first-run bug.** `maki-ui/src/theme.rs:14` has `const DEFAULT_THEME: &str = "dracula";`, but `Theme::load_or_bundled()` (`maki-ui/src/theme.rs:976-987`) — which initializes the global `THEME` palette — falls back to `BUNDLED_THEMES[0]` on a fresh run. `BUNDLED_THEMES` is alphabetically sorted (`maki-ui/build.rs:15`), so `BUNDLED_THEMES[0]` is `ayu_dark`. As of today, a fresh install actually renders `ayu_dark` while `current_theme_name()` (used by the picker, `maki-ui/src/theme.rs:162-166`) reports `dracula` — the constant and the real palette have been out of sync. This is almost certainly why the issue says "I keep thinking we've done this and we haven't". Fix: flip `DEFAULT_THEME` to `"makima"` **and** make the `load_or_bundled` fallback load `DEFAULT_THEME` (via the existing `load_bundled`, `maki-ui/src/theme.rs:177-183`), in both test and production builds.

3. **Remove hardcoded palette fallbacks; degrade gracefully instead (operator decision, supersedes the earlier re-point plan).** Every bundled splash and the agent-splash skill template hardcode the old default's palette as pre-seed fallbacks in `theme_or(name, fallback)` calls (dracula bg `#282a36`, fg `#f8f8f2`, accent `#ffb86c`, in-progress `#f1fa8c`); `plugins/lib/maki/color.lua` and `matrix.lua` carry `#000000` fallbacks. No hardcoded defaults: the host theme is the single color source. The splash frame protocol drops the vestigial `bg` key (the blit already ignores the plugin's bg and paints the host's `theme.background` for every explicit-style row, `maki-ui/src/splash.rs:200-214`), a style table may omit `fg` (the host paints its theme foreground), and every splash resolves semantic names per frame via `maki.ui.theme_color`, degrading structurally when a name is nil. That nil path is unreachable in the TUI (`EventLoop::new` seeds the palette synchronously before the first frame, `maki-ui/src/event_loop.rs:586-592`, and every `install` re-seeds, `maki-ui/src/theme.rs:169-175`) but is live in the maki-lua test/bench host, which never links maki-ui.

Non-goals: no migration for users who persisted `lunared` in their state dir or set `ui.theme = "lunared"` in config (graceful existing fallback: unknown name warns/ignores, `maki-ui/src/event_loop.rs:578-584`); `site/demo.cast` is a pre-`lunared`-era recording (it contains no `lunared` string; its recorded picker frame shows the old theme list with the previous default's colors) and stays as-is; the unrelated stale "26 themes" count on the homepage (`site/index.html:960`, catalog actually has 34) is out of scope. Verified: no tracked file contains `lunared` other than the two being renamed.

## Implementation Plan

### Phase 1 — Rename `lunared` → `makima`

1. `git mv maki-ui/src/themes/lunared.toml maki-ui/src/themes/makima.toml`.
   - Update the header comment line 1 (`# Lunared theme for Makima`) to describe it as the default Makima theme.
2. `maki-ui/src/components/file_completion.rs:1814` and `:2003` — `.load("lunared")` → `.load("makima")`.
3. Docs site (the docs syntax theme is a port of the same palette, named after it):
   - `git mv site/docs/extra/lunared.json site/docs/extra/makima.json`; set `"name": "makima"` inside.
   - `site/docs/config.toml:9-10`: `theme = "makima"`, `extra_themes = ["extra/makima.json"]`.
   - `site/docs/DESIGN.md:27`: `lunared \`#120c0c\`` → `makima \`#120c0c\``. Also fix the stale path on line 37 (`extra/maki.json`, which does not exist; the real file is the one being renamed) → `extra/makima.json`.

### Phase 2 — Default theme + first-run palette fix (`maki-ui/src/theme.rs`)

1. Line 14: `const DEFAULT_THEME: &str = "makima";`
2. `load_or_bundled()` (lines 976-987): replace the final fallback `Self::from_toml(BUNDLED_THEMES[0].toml)` with `load_bundled(DEFAULT_THEME)` and handle the `Result` (it cannot fail once the constant is a bundled name; `expect`/panic with the error message is acceptable here since it is a startup invariant). Update the stale doc comment — the test build no longer pins "the first bundled theme", it pins the default theme, still deterministic and zero I/O (`load_bundled` reads the in-memory catalog, no disk).
   - Rationale for changing the test-build pin too: it keeps test behavior representative of a production first run, and all theme-sensitive tests already install an explicit theme under `theme_test_guard()` — verified that every concrete-color assertion in `maki-ui` either parses explicit inline tomls or runs under the guard, so no test depends on the old `ayu_dark` pin.
3. New tests in the `theme.rs` `#[cfg(test)]` module (snake_case, constants for magic values per AGENTS.md, shared error/status constants; plain `#[test]` — none of the three is parameterizable, and the module's existing style is plain `#[test]`):
   - `default_theme_is_bundled`: `load_bundled(DEFAULT_THEME)` parses; its `background` equals a `MAKIMA_BG` const `Color::Rgb(0x12, 0x0c, 0x0c)` (pinned from `makima.toml` palette). This fails if the default points at a missing or wrong theme.
   - `initial_theme_falls_back_to_default`: in the test build `Theme::load_or_bundled()` returns the default-theme palette (`background == MAKIMA_BG`) — pins the new fallback branch.
   - `current_theme_name_falls_back_to_default`: a fresh `InMemoryThemesProvider::bundled()` with no `select`/`persist` returns `current_theme_name() == DEFAULT_THEME` — pins the documented "current → persisted → DEFAULT_THEME" chain (currently untested).

### Phase 3 — Remove hardcoded palette fallbacks; graceful degradation

Design: the host theme is the only color source. `maki.ui.theme_color(name)` stays the query — it returns `#rrggbb` or nil (binding: `maki-lua/src/api/ui/mod.rs:173-191`; resolution: `maki-highlight/src/lib.rs:55-72` — the seeded fast map is the only live source: the syntect-settings fallback branch at :63-71 is dead code, it serializes a settings array and `as_object()?` always fails, so no name resolves there). When a name is nil the frame degrades instead of painting baked-in colors:

- a style table may omit `fg` → the host blits its theme foreground;
- the `bg` key is dropped from the protocol: the blit already ignores the plugin's bg and paints the host's `theme.background` (`maki-ui/src/splash.rs:200-214`), so the key is dead state;
- blended/dimmed variants degrade to their unblended form; an element with no resolvable color still renders in the host foreground.

In the TUI the nil path is unreachable: `EventLoop::new` calls `refresh_syntax_theme()` synchronously before the loop starts (`maki-ui/src/event_loop.rs:586-592`) and every `install` re-seeds via `store_and_bump` (`maki-ui/src/theme.rs:169-175`). It is live in the maki-lua test/bench host, which never links maki-ui: `maki_highlight::set_ui_colors` is only called from `maki-ui/src/highlight.rs:31` (verified), so `theme_color` is nil there.

1. Protocol — `maki-lua/src/splash.rs`:
   - `SplashStyle::Rgba { fg: Option<(u8, u8, u8)>, bold: bool }` — drop `bg` (enum at :17-26).
   - `parse_style` table branch (:118-131): `fg` optional — absent → `None`, present → must parse as hex (keep the `bad splash fg color '{s}'` error); `bg` is no longer read, so a pre-change plugin that still sends `bg` keeps working. Generic error message becomes `"splash segment style must be a string or a {fg,bold} table"`.
   - New `#[cfg(test)]` module in the same file (plain `#[test]`, matching the module's style; none is parameterizable):
     - `parse_style_empty_table_degrades_to_host_fg` — `{}` → `Rgba { fg: None, bold: false }`.
     - `parse_style_fg_hex_and_bold` — `{ fg = "#112233", bold = true }` → `Rgba { fg: Some((0x11, 0x22, 0x33)), bold: true }`.
     - `parse_style_ignores_bg_key` — `{ fg = "#112233", bg = "#ff0000" }` parses.
     - `parse_style_rejects_bad_fg` — `{ fg = "red" }` errors with the shared `bad splash fg color` message const.
     - `parse_segment_missing_style_errors` — `{ glyphs = "x" }` errors (unchanged behavior, now pinned).
2. Host blit — `maki-ui/src/splash.rs`:
   - `Rgba` branch (:206-214): `fg` is `None` → `theme.foreground` (from the existing `let theme = theme::current();` at :167); bg stays the host's `theme.background`.
   - Update the two stale comments about the plugin supplying its own bg (`maki-ui/src/splash.rs:200-204` prose "even when the plugin's own bg guess is stale", and the `:367-371` test doc that names a "Dracula fallback"): both become stale once plugins stop sending `bg` and the host owns the background.
   - Tests: drop the now-absent `bg` field from the literal at :357-361; replace `blit_text_bg_uses_theme_background_not_plugin_bg` (:372-404) with two palette-agnostic tests (both read `theme::current()`, no color constants):
     - `blit_rgba_missing_fg_uses_theme_foreground` — `Rgba { fg: None, bold: false }` row: cell fg == theme foreground, cell bg == theme background.
     - `blit_rgba_keeps_explicit_fg_on_theme_bg` — `Rgba { fg: Some((10, 20, 30)), bold: false }` row: cell fg == `Color::Rgb(10, 20, 30)`, cell bg == theme background (keeps the host-owns-bg invariant the old test covered).
   - `test_splash_lua_matches_rust_golden` (:446-493) compares glyphs and positions only, no colors — unchanged, no re-record.
3. Bundled splashes — `plugins/splashes_default/splash/` — delete the `theme_or` helper and every dracula tuple from all six files; each keeps a small local decode of `theme_color` to `{r,g,b}` or nil (the files are deliberately self-contained single modules — existing pattern):
   - `default.lua` — `refresh_colors` (:56-62) stays per-frame (tracks runtime theme switches): `BG` = `theme_color("background")` decoded or nil; `FG`/`ACCENT`/`TIP` = `foreground`/`accent`/`todo_in_progress` decoded or nil; delete `BG_HEX` and the stale :51-54 comment. `text_color` (:103-109):
     - target nil → `{ bold = bold }` (host fg);
     - target present, `BG` nil → `{ fg = hex(target...), bold = bold }` (unblended — the only degradation a missing background allows);
     - otherwise → the current bg-blend, unchanged.
     Tip spacer (:283) → `{ bold = false }` (a host-fg space erases the field, same as before).
   - `voronoi.lua`, `aurora.lua`, `caustics.lua`, `kaleidoscope.lua`, `metaballs.lua` — keep only `FG` (decoded tuple or nil) for the version string; delete `BG`, `BG_HEX`, and `ACCENT` (dead: the `bg` key is host-ignored and `ACCENT` is assigned but never read — verified by grep). `color(hex)`/`cell_style` → `{ fg = hex, bold = false }` (no `bg` key); `version_style` → `FG and color(rgb_to_hex(FG, 0.4 * f)) or { bold = false }` (voronoi :173, aurora :176, caustics :164, kaleidoscope :188, metaballs :177); the tiny-screen guard `flat_rows(w, h, color(BG_HEX))` → `flat_rows(w, h, { bold = false })` in **all five files** (voronoi :169, aurora :172, caustics :160, kaleidoscope :184, metaballs :173 — missing one leaves a `color(nil)` crash that no CI/test path reaches, since nothing pulls a sub-8x6 frame).
   - `matrix.lua` — delete the `#000000` empty-cell fg and the `bg` key (:110, :116): `next_fg = cell and cell.fg` (nil runs coalesce as-is) and `flush` emits `{ fg = fg, bold = false }`.
4. Color helper — `plugins/lib/maki/color.lua`:
   - `M.dim` (:17-20): drop `or "#000000"`; when `theme_color("background")` is nil, return the color unchanged (undimmed). Rename the local `bg` to `background` (keeps the Phase 5 `bg` sweep clean). Sole caller `plugins/grep/init.lua:74` is unaffected in the TUI (seeded); headless degrades instead of dimming toward black.
5. Skill template + convention docs:
   - `.agents/skills/aesthetic-splash-screens/references/template.lua` — same treatment: no `theme_or`/tuples; `BG_HEX` = `theme_color("background")` or nil (stays a hex string, `color(hex)` keys on it); `FG` = `theme_color("foreground")` decoded to a tuple or nil (used by `rgb_to_hex`); drop the unused `ACCENT` (:30); `color(hex)` → `{ fg = hex, bold = false }`; `new_grid` bg style (:59) → `BG_HEX and color(BG_HEX) or { bold = false }` (rename the local `bg` to `cell_style` to keep the Phase 5 `bg` sweep clean); `flat_rows` guard (:192) and LABEL/version lines (:206-207) → `FG and color(rgb_to_hex(FG, k * f)) or { bold = false }`.
   - `.agents/skills/aesthetic-splash-screens/SKILL.md` — protocol line (:56-59) becomes `{ fg = "#rrggbb", bold = bool }` (fg optional: omitted → host theme foreground; the cell background is always the host theme background); add one line to the theme-aware bullet (:30-36): a name `theme_color` cannot resolve degrades to the host foreground — never bake a fallback palette; the two `seg.style.bg` debug references (`SKILL.md:133`, `references/shader-porting.md:78`) become `seg.style.fg` — the style no longer carries a `bg`; also reword the prose at `references/shader-porting.md:76` ("bg-colored spaces" → "theme-background-colored spaces") so the Phase 5 `bg` sweep stays clean.
6. User docs — `site/docs/content/splash/_index.md`:
   - Style list (:34-38): the table style is `{ fg = "#rrggbb", bold = false }`; `fg` optional (host paints the theme foreground when omitted); the cell background is always the host's theme background — rewrite the stale "opaque background keeps the starfield" rationale.
   - Matrix example (:158-234): drop `local BG = "#000000"` (:160) and every `bg = BG` key; empty runs use a style without `fg`.
   - `site/docs/content/lua-api/_index.md` is generated: `just gen-docs` refreshes the embedded `maki.color` source (the `or "#000000"` at :6117 disappears with the file change).

Do not touch: `site/asciinema-player.css` (third-party player's own "dracula" theme), `maki-ui/src/themes/dracula.toml` and all tests that use dracula explicitly (`theme.rs` tests, `markdown.rs`, `tool_display.rs`, `segment.rs`, `arg_completion.rs` `BASE_THEME`, `maki-match` fixtures) — dracula remains a bundled theme and those are explicit choices, not default assumptions. `color_compat.rs:243` is a standalone color test, not theme-dependent.

### Phase 4 — Generated docs

- `just gen-docs` to regenerate `site/docs/content/configuration/_index.md` — the `Available themes:` list (line 89) is generated from `maki_ui::BUNDLED_THEMES` by `maki-docgen/src/gen_config.rs:116-121`, so it picks up `makima` and drops `lunared` automatically. The `ui.theme` prose ("the built-in default on first run", line 87) names no theme, so no prose change.
- `just gen-docs-check` must pass afterwards (it is in `just ci`).

### Phase 5 — Residue sweep

- Case-insensitive `git grep lunared` (tracked files only): expected result is zero matches. A plain `grep -r` also hits stale `target/debug/deps/*.d` artifacts referencing `lunared.toml` until the next rebuild, so scope to tracked files.
- Fallback residue (expected zero; scoped so the theme-agnostic `maki-ui` test fixtures do not false-positive): `git grep -nE 'theme_or|40, 42, 54|248, 248, 242|255, 184, 108|241, 250, 140|#000000' -- plugins .agents site/docs`.
- Protocol residue (expected zero — no splash Lua may reference the dropped `bg` style key): `git grep -n 'bg' -- plugins/splashes_default/ plugins/lib/maki/color.lua .agents/skills/aesthetic-splash-screens/references/`.
- Positive check: `maki.ui.theme_color` still appears in the six theme-resolving splash files (`default.lua` + the five shader splashes) and `template.lua` (per-frame resolution is the new contract); `matrix.lua` is excluded by design — its green palette is the effect itself, per the skill's "effects that ARE their palette" convention. The generated lua-api docs no longer contain `or "#000000"`.

## Acceptance Criteria

- **AC.1** — `makima` is a bundled theme and `lunared` no longer exists: `InMemoryThemesProvider::bundled().load("makima")` succeeds, `.load("lunared")` errors with `unknown theme: lunared`; `all_bundled_themes_parse` passes with `makima` in the catalog.
- **AC.2** — `makima` is the default: `DEFAULT_THEME == "makima"`, a fresh provider with no selection reports it via `current_theme_name()`, and its palette is the makima one (bg `#120c0c`).
- **AC.3** — First-run palette matches the default: with no persisted theme, the initial `THEME` palette (the `load_or_bundled` fallback) is the makima palette, not `BUNDLED_THEMES[0]`.
- **AC.4** — Graceful degradation: in the unseeded maki-lua harness (where `theme_color` is nil for every name), the default splash still renders a complete frame and every `Rgba` row degrades to `fg: None`; the maki-ui blit paints an `fg: None` row with the theme foreground on the theme background; `maki.color.dim(color, factor)` returns the color unchanged (identity) instead of crashing or dimming toward black.
- **AC.5** — No hardcoded palette fallbacks remain: `git grep -nE 'theme_or|40, 42, 54|248, 248, 242|255, 184, 108|241, 250, 140|#000000' -- plugins .agents site/docs` returns zero; the splash style protocol no longer carries a `bg` key (`SplashStyle::Rgba { fg: Option<_>, bold }`); a pre-change style table that still sends `bg` still parses (the key is ignored).
- **AC.6** — Docs are consistent: `just gen-docs-check` passes; the generated `ui.theme` list contains `makima` but not `lunared`; the generated lua-api `maki.color` source no longer contains `or "#000000"`; the splash guide's protocol section and matrix example match the new frame format; `site/docs/config.toml` + `extra/makima.json` build the docs site with the renamed theme.
- **AC.7** — No stale default assumptions: `git grep -i lunared` over tracked files returns zero matches; dracula remains a bundled theme and its explicit test usages are untouched.

## Test Strategy

All checks run via the existing harnesses (`just check`, `just lint`, `just test` = nextest, `just gen-docs-check`); no new test infrastructure needed.

| AC | Test | Notes |
|---|---|---|
| AC.1 | existing `all_bundled_themes_parse` (theme.rs:1275) + existing `completion_kind_highlights_do_not_collapse_into_item_colour` / `selected_row_fuzzy_match_spans_keep_selection_background` (file_completion.rs:1812,1992) renamed to load `"makima"` | the completion tests fail if `makima` is not a bundled name; a new assertion `load("lunared")` errors goes in `load_unknown_theme_errors` style or as a second case |
| AC.2 | new `default_theme_is_bundled` + new `current_theme_name_falls_back_to_default` (theme.rs test module) | the fallback-chain test is new coverage of previously untested code |
| AC.3 | new `initial_theme_falls_back_to_default` (theme.rs test module) | exercises the exact `load_or_bundled` fallback in the test build. The production persisted-name branch is pre-existing and covered by `disk_provider_persist_round_trip` (theme.rs:1660); the end-to-end "fresh terminal boots makima" behavior is additionally spot-checked manually by running `cargo run` without state |
| AC.4 | new `test_splash_default_degrades_without_theme_colors` (`maki-lua/tests/plugin_host.rs`, alongside `test_splash_slot_default` :4869) + new `test_color_dim_is_identity_without_theme_colors` (same file, `test_version_api` pattern: `PluginHost::new` + a `load_source` probe tool that returns `require("maki.color").dim("#8899aa", 0.5)`, asserted `== "#8899aa"`) + new `blit_rgba_missing_fg_uses_theme_foreground` and `blit_rgba_keeps_explicit_fg_on_theme_bg` (`maki-ui/src/splash.rs` test module) | the plugin_host test boots `splashes_default` and pulls a frame; in the maki-lua test binary nothing calls `maki_highlight::set_ui_colors` (verified: only `maki-ui/src/highlight.rs:31` does), so `theme_color` is nil for every name and it asserts the logo row is `Rgba { fg: None, .. }` and no `Rgba` row carries `fg: Some(_)`. The dim test fails on a wrong nil guard (e.g. `nil:match` crash) or a resurrected `#000000` — `require("maki.color")` resolves from the bundled `plugins/lib` dir, which is always on the require path (`maki-lua/src/loader.rs:40-168`). The existing `test_splash_slot_default`, `test_splash_host_boots_and_serves_frames`, and `test_splash_lua_matches_rust_golden` (glyph-only — no color re-record) run the same unseeded path and keep passing. The blit tests read `theme::current()`, so they hold for any initial palette |
| AC.5 | new maki-lua parse unit tests in `maki-lua/src/splash.rs` (`parse_style_empty_table_degrades_to_host_fg`, `parse_style_fg_hex_and_bold`, `parse_style_ignores_bg_key`, `parse_style_rejects_bad_fg`, `parse_segment_missing_style_errors`) + Phase 5 residue sweeps as explicit steps | the parse tests fail if the protocol regains a required `fg` or stops tolerating stale `bg` keys; the sweeps fail if any fallback tuple/hex or `theme_or` reappears under `plugins`/`.agents`/`site/docs` |
| AC.6 | `just gen-docs-check` (run after `just gen-docs`); optional visual check of the renamed docs theme via `sh site/build.sh` (zola build of `site/docs`, the site's real build entrypoint) | the rename + protocol doc edits are covered by the file moves/edits regardless; the build is a spot check that `extra/makima.json` resolves. The splash-guide and SKILL.md edits are hand-written (not generated) — covered by the AC.5/AC.6 sweeps and review |
| AC.7 | Phase 5 sweeps as explicit steps (`git grep -i lunared` zero; fallback + `bg` residue sweeps); `just test` full suite guards against any missed fallback in rendering | the suite itself is the regression net: any test that secretly depended on the old initial palette or the old style shape fails loudly |

Manual validation (limitations noted): launch the TUI in a clean environment (`MAKIMA_*` state dir pointed at an empty temp dir) and confirm the splash + UI render with the makima palette (warm near-black `#120c0c` background, red accent) and that the theme picker highlights `makima`. This is the only way to see the production first-run path (test builds skip the persisted-name branch); automated coverage for it is limited to the `load_or_bundled` fallback test.

## Review Strategy

- Plan mode: dispatch `plan_reviewer` on this file before `plan_submit`; fix or rebut all critical/high findings and re-run until clean.
- Implementation: after changes and passing `just ci`-equivalent checks, dispatch a `general` review subagent over the diff; fix or rebut all findings, re-review until no critical findings.

## Documentation Strategy

- User docs:
  - Generated: `just gen-docs` refreshes the `ui.theme` reference list and the embedded `maki.color` source (the `or "#000000"` fallback disappears with the file change); `just gen-docs-check` guards it (AC.6).
  - Hand-written `site/docs/content/splash/_index.md`: the style list (:34-38) and the matrix override example (:158-234) are updated to the new frame format (no `bg` key, optional `fg`, host-owned cell background).
  - `site/docs/DESIGN.md`: name/path fixes only (Phase 1.3).
- Agent-facing: `.agents/skills/aesthetic-splash-screens/` — `references/template.lua` rewritten to the no-fallback convention and `SKILL.md` protocol/theme-aware sections updated (Phase 3.5).
- `AGENTS.md`: no change — it mentions neither themes nor the splash protocol.

## Risks, Blockers, and Required Decisions

- **Splash protocol change is user-visible for third-party splashes**: a style table's `bg` key stops having any effect (it was never painted — the host always used its own `theme.background`, `maki-ui/src/splash.rs:200-214`), and omitting `fg` is now meaningful (host theme foreground). Parsing stays lenient — unknown keys are ignored, so pre-change plugins keep running — but the docs are the contract and the repo is pre-1.0. `SplashStyle` is a `pub` enum in maki-lua; the in-repo consumers are exactly the two `splash.rs` files plus the `plugin_host.rs` pattern matches (verified by `grep SplashStyle::`), so no other Rust call sites break.
- **Bench behavior**: `maki-lua/benches/splash_perf.rs` already runs the pre-seed path (`raw_render` stubs `theme_color` to nil at :43-50; `pull_roundtrip` boots an unseeded host at :221-256), so after the change it measures the degradation path — fg-less styles must parse cleanly through `frame_from_lua`. Verified the bench has no golden-hash assertion, so nothing to re-record.
- **Test-build palette shift**: the `load_or_bundled` fallback changes the test-build initial palette from `ayu_dark` to `makima`. Investigation found no test asserting the old palette concretely (all theme-sensitive tests install explicitly under `theme_test_guard()`), but if `just test` surfaces one, fix by making the test explicit (install under the guard), not by reverting the fix.
- **Persisted/`ui.theme = "lunared"` users**: after upgrade the name is unknown; existing graceful fallback applies (config warns at `event_loop.rs:582`, persisted name falls through to default). No migration — deliberate, the repo is pre-1.0 and the fallback is clean.
- **`site/demo.cast`** is a pre-`lunared`-era recording (no `lunared` string; its recorded picker frame shows the old theme list with the previous default's colors). It is a historical recording; re-recording the demo is out of scope. Flag to the operator if a fresh demo is desired.
- **Docs-site visual check**: `sh site/build.sh` builds the docs site (zola, per `DESIGN.md`); the renamed `extra/makima.json` + config edit should build cleanly, but this is a spot check, not part of `just ci`.
- No required operator decisions remain: the one open choice from the issue (`makima` vs `makimared`) was answered by the operator in this request (`makima`), and the fallback design (no hardcoded defaults, graceful degradation) was set by the operator after the plan was drafted.
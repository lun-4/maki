# Fix wheel scroll snapping back to the cursor in Lua float windows (/sessions)

## Goal

Wheel scrolling inside a Lua float window (e.g. the `/sessions` picker) must survive the window's periodic re-renders: the viewport stays where the user scrolled it, and only moves when the cursor row actually changes.

## Implementation Summary

Single, contained fix in `maki-ui/src/components/lua_float.rs`. `FloatWindow::bring_cursor_into_view` (currently `lua_float.rs:118-129`) is what snaps the scroll offset back toward the cursor; it runs from two entry points on every picker re-render:

- `FloatManager::tick` dirty-buffer path (`lua_float.rs:230-232`): the sessions plugin re-sets the buffer on every render, so `read_if_dirty()` is always `Some`.
- `set_cursor` (`lua_float.rs:97-99`), reached via the `SetCursor` command (`lua_float.rs:241-242`): the plugin re-asserts `cursor_line` on every render.

The plugin's periodic re-renders make both paths fire about once a second (`TICK_MS=100` × `AGE_TICKS=10` in `plugins/sessions/init.lua:20-21,487-488`) and on any `SessionStatusChanged` (`init.lua:514-530`), yanking `scroll_offset` back to the top cursor row via `adjust_scroll` (`lua_float.rs:617-635`, `if cursor < offset { offset = cursor }`).

Fix: make the offset follow the cursor only when the cursor row actually changed. `set_cursor` computes `cursor_moved = row != self.cursor` *before* overwriting the field (a naive comparison inside `bring_cursor_into_view` cannot work: `set_cursor` has already replaced `self.cursor` by the time it runs, so the old row is gone). `bring_cursor_into_view` takes a `cursor_moved: bool`: it adjusts the offset when the cursor row moved or was clamped by content shrink, and otherwise only clamps the offset to the max (never dragging it toward the cursor). The cursor itself is always clamped into bounds, so promises 1-2 of the struct doc stay intact. No Lua/plugin changes needed; the fix covers the picker pattern used by `/sessions` and the other Lua float lists.

Example: the `/sessions` window opens with `reserved_top = 2`; the cursor starts at row 2 and the user wheel-scrolls down (`scroll_offset` grows, cursor still row 2). Every subsequent re-render re-asserts row 2, which is the same effective row, so the offset is left alone.

## Implementation Plan

### Phase 1: change `set_cursor` and `bring_cursor_into_view` (`maki-ui/src/components/lua_float.rs:97-129`)

There are exactly three call sites for `bring_cursor_into_view`: `set_cursor` (`:99`), the `tick` dirty-buffer path (`:232`), and one direct call in the existing test `bring_cursor_into_view_after_content_grows` (`:1799`, updated for the new signature).

```rust
fn set_cursor(&mut self, row: usize) {
    let cursor_moved = row != self.cursor;
    self.cursor = row;
    self.bring_cursor_into_view(cursor_moved);
}

fn bring_cursor_into_view(&mut self, cursor_moved: bool) {
    let layout = self.layout();
    let effective = self.cursor.saturating_sub(layout.reserved_top);
    let clamped = effective.min(layout.scrollable.saturating_sub(1));
    self.cursor = clamped + layout.reserved_top;
    if cursor_moved || clamped != effective {
        self.scroll_offset = adjust_scroll(
            clamped,
            self.scroll_offset,
            layout.scrollable,
            self.viewport_h,
        );
    } else {
        self.scroll_offset = self.scroll_offset.min(layout.max_offset(self.viewport_h));
    }
}
```

The dirty-buf path in `tick` calls `win.bring_cursor_into_view(false)`: content changed but the cursor row did not, so the offset is preserved; if content shrank and the cursor had to be clamped, `clamped != effective` fires and the offset follows, which keeps bounds maintenance on that path.

Behavior matrix (worked out against the real code):

- Re-asserting the same row after a wheel scroll: `cursor_moved = false`, `clamped == effective`, offset only min-clamped, so it stays put. This is the fix.
- `SetCursor` to a new in-bounds row: `moved = true`, offset follows the cursor. `set_cursor_brings_cursor_into_view` (`:1784`) passes: `set_cursor(19)` on a 20-line/5-row viewport sets offset 15, cursor visible in `[15, 20)`.
- Content shrinks so the cursor row is past the end: `clamped != effective`, offset follows (existing `reserved_bottom_clamp_on_shrink` `:1354`, `cursor_does_not_enter_reserved_bottom` `:1337`, `set_cursor_on_empty_buf` `:1307` still pass; note `set_cursor_on_empty_buf` moves 0→5 so `moved` is true).
- `reserved_top_clamps_cursor_down` (`:1488`): `SetCursor(0)` with cursor already 0 gives `moved = false`, but the cursor clamp still runs (`self.cursor = 0 + 2 = 2`), the else-branch keeps offset 0; asserts cursor == 2, passes.
- `tick_reads_dirty_buf_before_processing_set_cursor` (`:1424`): dirty path is a no-op (`moved = false`, cursor in bounds), then `SetCursor(5)` is `moved = true` and snaps; asserts cursor == 5, passes.
- `bring_cursor_into_view_after_content_grows` (`:1792`): update the direct call at `:1799` to `win.bring_cursor_into_view(false)`; cursor row unchanged (2), offset min-clamped to 0, cursor still visible in `[0, 3)`; `assert_cursor_visible` + `assert_invariants` pass.
- `invariants_hold_across_action_sequence` (`:1831`) and `scroll_by_persists_across_refresh_layout`/`refresh_layout_clamps_when_viewport_grows` (`:1771`,`:1805`) are unaffected (`refresh_layout` untouched).
- `adjust_scroll` (`:617`) is unchanged.

### Phase 2: update the design comment (`lua_float.rs:48-58`)

Promise 3 currently reads "`set_cursor` and `bring_cursor_into_view` place the cursor inside the visible band whenever there is anything to scroll." Restate it: the offset follows the cursor only when the cursor row changes or is clamped by shrink; re-asserting an unchanged cursor never moves the offset. The offset stays at or below `max_offset` at all times (the new else-branch min-clamp enforces it inside `bring_cursor_into_view`, and `refresh_layout` keeps the existing render-path restraint from promise 4). This makes the documented contract match the actual wheel-scroll behavior promises 3-4 now deliver together.

### Phase 3: regression tests

Add to the `#[cfg(test)]` module in `lua_float.rs`, next to `scroll_by_persists_across_refresh_layout` (`:1771`). Reuse test helpers `make_window_n`, `make_config`, `refresh_layout`, `scroll_by`, `set_cursor`, `assert_cursor_visible`, `assert_invariants`; define a new const (AGENTS.md: test messages are shared constants), e.g.

```rust
const CURSOR_REASSERT_PRESERVED: &str = "re-asserting an unchanged cursor must not pull the offset toward it";
```

1. `set_cursor_same_row_preserves_wheel_scroll` — the exact user scenario at window level, with `reserved_top = 2` to mirror `/sessions`:
   - `make_window_n(20)`, `config.reserved_top = 2`, `refresh_layout(5)`.
   - `set_cursor(2)` (first selection below the pinned filter/header rows).
   - `scroll_by(-8)` (wheel down; offset grows to 8, cursor row 2 is now above the viewport).
   - `set_cursor(2)` again (the periodic picker re-assertion).
   - Assert `scroll_offset` unchanged (`{CURSOR_REASSERT_PRESERVED}`) and `assert_invariants`.

2. `set_cursor_new_row_after_wheel_scroll_brings_it_into_view` — the other half of the contract: after the same scroll-away, `set_cursor(5)` (selection moved) must pull the cursor into the visible band. Reuse `assert_cursor_visible`. (Selection keys in the picker route through `set_sel` → `render` → `set_cursor`, so this pins that arrow keys still follow the selection.)

3. `tick_preserves_wheel_scroll_across_periodic_rereder` — manager-tick level, driving the real command path: open a float with a 20-line buffer, `refresh_layout(5)` + `scroll_by(-8)`, then in one tick send `SetCursor` with the current row and re-`set_lines` the buffer (as `render()` does), and assert the offset is preserved. Use a real `SharedBuf` via `make_buf`.

Existing tests needing no behavioral change: `scroll_by_persists_across_refresh_layout`, `set_cursor_brings_cursor_into_view`, `invariants_hold_across_action_sequence`, `window_command_owes_exactly_one_frame`, `multiple_commands_in_single_tick`, `adjust_scroll_cases`. `bring_cursor_into_view_after_content_grows` only needs its direct call at `:1799` updated to the new `bring_cursor_into_view(false)` signature.

### Phase 4: verification

- `cargo nextest run -p maki-ui` (full crate, or filter `lua_float`).
- `cargo clippy --all --tests -- -D warnings` (or `-p maki-ui`).
- Manual smoke test: open `/sessions`, wheel-scroll down, confirm the viewport stays put past the 1s age tick and across a session-status change (e.g. a background agent flipping state); arrow keys still move the selection and follow it.

## Acceptance Criteria

- AC.1: Wheel-scrolling the `/sessions` picker keeps the scrolled viewport position for as long as the picker is open, including across the 1s age-tick re-render and any `SessionStatusChanged` refresh. (Manual TUI check; mechanism pinned by unit tests below.)
- AC.2: Moving the selection (arrow keys, page up/down, filter edits) still brings the cursor into the visible band when it is off-screen. (Manual check + `set_cursor_new_row_after_wheel_scroll_brings_it_into_view`.)
- AC.3: `cargo nextest run -p maki-ui` passes, including the pre-existing float tests that pin cursor clamping and scroll behavior (`scroll_by_persists_across_refresh_layout`, `set_cursor_brings_cursor_into_view`, `bring_cursor_into_view_after_content_grows`, `invariants_hold_across_action_sequence`, `reserved_bottom_clamp_on_shrink`, `cursor_does_not_enter_reserved_bottom`, `set_cursor_on_empty_buf`, `reserved_top_clamps_cursor_down`, `adjust_scroll_cases`, `tick_reads_dirty_buf_before_processing_set_cursor`).
- AC.4: The new regression tests exist and pass; `set_cursor_same_row_preserves_wheel_scroll` and `tick_preserves_wheel_scroll_across_periodic_rereder` fail against the pre-fix code (revert the two-function change, `set_cursor` + `bring_cursor_into_view`, to confirm). `set_cursor_new_row_after_wheel_scroll_brings_it_into_view` is a contract pin: it passes before and after, guarding the "selection movement still follows the cursor after a scroll-away" behavior.

## Test Strategy

| Criterion | Test(s) |
|---|---|
| AC.1 | `set_cursor_same_row_preserves_wheel_scroll`, `tick_preserves_wheel_scroll_across_periodic_rereder` |
| AC.2 | `set_cursor_new_row_after_wheel_scroll_brings_it_into_view` |
| AC.3 | Existing suite listed in AC.3 via `cargo nextest run -p maki-ui` |
| AC.4 | `set_cursor_same_row_preserves_wheel_scroll`, `tick_preserves_wheel_scroll_across_periodic_rereder` (revert-sensitive); `set_cursor_new_row_after_wheel_scroll_brings_it_into_view` as contract pin |

Layering: pure state-machine logic (scroll math on `FloatWindow`), so unit tests in the same file are the right layer, matching the existing `lua_float.rs` test module style. There is no harness that drives a wheel scroll into an open Lua float end-to-end (app tests inject `Msg::Mouse` for clicks and drags, but floats open through the Lua host and the float wheel path is not driven there); the manual check in AC.1 covers the end-to-end behavior and the unit tests pin the exact mechanism at both entry points (dirty-buf path and `SetCursor` command), which is where any regression would reappear. The maki-lua plugin integration suite (`maki-lua/tests/plugin_host.rs`) does not exercise mouse wheel input into floats, so it is not extended.

## Review Strategy

Plan-mode review: run `plan_reviewer` over this file before `plan_submit`, fix or rebut findings.

Implementation review: after the change and after all automatable tests pass, the executing session dispatches a `general` subagent to review the diff for correctness, doc-consistency, and adherence to the AGENTS.md guidelines (the repo has no separate review guidance). Fix or rebut all findings; rerun review if any are critical.

## Documentation Strategy

Only the design comment in `lua_float.rs` (`:48-58`) needs updating, to state the new cursor/scroll contract (Phase 2). No user-facing doc changes: the site docs describe `mouse_scroll_lines` configuration (`site/docs/content/configuration/_index.md:79`) but nothing about float scroll persistence; `plugins/sessions/init.lua` is untouched. Update `AGENTS.md` only if this reveals a convention worth recording (unlikely).

## Risks, Blockers, and Required Decisions

- Behavior change is intentional and narrow: re-asserting an unchanged cursor row no longer scrolls. The only scenario that loses behavior is a Lua window that re-sets its buffer or cursor without the cursor row changing and relies on the snap to keep the selection visible after *other* content changes above it; the sessions plugin recomputes and re-asserts the correct row on every render, and known float pickers route all selection movement through `set_cursor`, so none regress.
- `bring_cursor_into_view` gains a `cursor_moved: bool` parameter: exactly three call sites exist (`set_cursor` `:99`, `tick` `:232`, one direct test call `:1799`), and all three are updated in Phase 1, so the signature change cannot drift.
- Caveat on the mechanism's edge: `cursor_moved = row != self.cursor` compares the raw asserted row against the stored clamped cursor, so a window that repeatedly asserts a row strictly below `reserved_top` would still re-snap (each assert reads as a move). This does not apply to `/sessions` (`init.lua:142-183` asserts `cursor_line >= board.reserved`, matching the stored value) or any other in-tree float, and is not a regression (today snaps unconditionally), but the fix is scoped to the picker pattern rather than universal.
- `viewport_h` is 1 until the first real render; `bring_cursor_into_view` can compute an offset on stale viewport height exactly as before the change (no new risk, and windows render on the same frame they open).
- Manual verification of the real mouse interaction is not automatable in this repo today (no TUI/mouse test harness); the gap is covered by the mechanism-level unit tests and is surfaced here rather than hidden.
- No blockers or operator decisions needed.
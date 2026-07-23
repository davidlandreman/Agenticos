# Plan: SVG icons in the ring-3 desktop + delete the kernel `guishell`

**Date:** 2026-07-22
**Status:** Implemented
**Depends on:** `2026-07-21-001-feat-ring3-desktop-shell-plan.md` (the ring-3
`DESKTOP.ELF` shell, now the default).

## Implementation notes (2026-07-22)

Both parts landed. Chosen approach for Part 1 was **Option A**: a userland SVG
rasterizer module (`userland/libs/gui/src/svg.rs`, a userland-adapted copy of
the kernel rasterizer producing an ARGB buffer) plus `Canvas::blit_argb`;
`userland/apps/desktop/src/icons.rs` now `include_bytes!`es the shared
`assets/icons/start/*.svg` and blits the rasterized icons (fixed-color, matching
the kernel — no theme tinting).

Part 2 removed `src/commands/guishell/` (replaced by the slim
`src/commands/desktop.rs` holding `init_desktop_root_only` +
`spawn_ring3_desktop_shell`), the kernel `windows/start_menu.rs`,
`windows/taskbar.rs`, and `dialogs/run.rs`, the `AGENTICOS_SHELL`/
`opt/agenticos/shell` boot selection (boot is now unconditional ring-3), and the
`start_menu_tests`/`taskbar_tests` modules. Removing the kernel Start menu
orphaned the **kernel** SVG rasterizer (`src/graphics/images/svg.rs`) and its
`svg_image` test — both deleted, since the userland twin now carries the only
SVG capability. Assorted chrome-only theme helpers
(`draw_taskbar_surface`/`draw_tray_well`/`taskbar_text`/`chrome_effect` +
`ThemeSpec.chrome_effect`, `draw_recessed_panel`, `draw_menu_separator`,
`separator_highlight`) were removed as dead code. `cargo check` is clean in both
`--features test` and default builds; the targeted boot suite
(`window_theme gui_userland time`) passes.

## Goal

Two coupled changes:

1. **Ring-3 icon parity.** Make `DESKTOP.ELF` render the same SVG Start-menu
   icons the kernel `guishell` uses (`assets/icons/start/*.svg`), replacing the
   crude procedural `Canvas` icons it draws today.
2. **Retire the legacy shell.** Delete the in-kernel ring-0 `guishell` so the
   ring-3 desktop is the only shell, keeping only the small kernel helpers the
   ring-3 path still needs.

Do Part 1 first and verify the desktop visually, then Part 2. They are
independent, and landing Part 1 first de-risks the removal by proving the
ring-3 shell is fully self-sufficient.

## Background — what exists today

- **Kernel guishell icons** are 13 SVG files in `assets/icons/start/*.svg`,
  `include_bytes!`-embedded in `src/window/windows/start_menu.rs:31-58` and
  rasterized by the kernel's self-contained SVG rasterizer
  `src/graphics/images/svg.rs` (no_std + `alloc` + `libm`, ~630 lines;
  `from_bytes` → parsed shapes, `sample_at_size(x, y, w, h) -> Option<Color>`,
  `None` = transparent). The kernel blits at fixed full color and does **not**
  tint for disabled/highlighted rows (`start_menu.rs:454-461`).
- **Ring-3 desktop icons** are hand-drawn `Canvas` line/rect primitives in
  `userland/apps/desktop/src/icons.rs` (181 lines, `enum Icon` with 13
  variants; `draw(canvas, icon, x, y, s, fg, accent)` used at
  `userland/apps/desktop/src/main.rs:417`). They currently track the theme via
  `fg`/`accent`. The CLAUDE.md notes these are procedural because "ring-3 has
  no SVG rasterizer."
- **Ring-3 has no image-blit path**, but `Canvas`
  (`userland/libs/gui/src/lib.rs`) already has a private `blend_pixel`
  (`:171`) and `pixels_mut()` (`:129`), and userland crates already depend on
  `libm` (`userland/libs/gui/Cargo.toml:16`) and `alloc`.
- **`guishell`** (`src/commands/guishell/mod.rs`, 896 lines) is now only the
  `ring0` legacy fallback (default is ring-3). It also holds the small helpers
  the ring-3 path still calls at boot.

## Part 1 — SVG icons in the ring-3 desktop

### Design decision: how to rasterize SVG in ring-3

- **(A, recommended) Port the rasterizer into a userland lib.** Add a small
  `userland/libs/svg` crate (or a module inside `gui`) — a userland-adapted
  copy of `src/graphics/images/svg.rs` that swaps the kernel's `Image`/`Color`
  for a plain ARGB output (`Vec<u32>` + width/height, `None` → transparent).
  `libm` and `alloc` are already available in userland. **Tradeoff:** the
  ~630-line rasterizer is duplicated (kernel + userland); the SVG *assets*
  remain single-source.
- **(A′) Extract a truly shared no_std crate** both the kernel and userland
  depend on, making the kernel's `SvgImage` a thin adapter over it. No code
  duplication, but touches `src/graphics/` and is more invasive.
- **(B) Build-time rasterization** into embedded RGBA arrays via `build.rs`.
  Rejected — adds host-side raster tooling for little gain.

**Chosen: Option A.** (Revisit A′ later if a second userland consumer of SVG
appears.)

### Steps

1. Create the userland SVG raster crate/module. Port parsing +
   `sample_at_size`; emit an ARGB `Vec<u32>` (`None` sample → transparent /
   alpha 0) plus width/height.
2. Add a public `Canvas::blit_argb(x, y, w, h, &[u32])` (or `blit_icon`) in
   `userland/libs/gui/src/lib.rs`, reusing the existing alpha-blend logic and
   skipping fully-transparent pixels.
3. In `userland/apps/desktop`, `include_bytes!` the same 13
   `assets/icons/start/*.svg` (path `../../../../assets/icons/start/…` from
   `userland/apps/desktop/src/`). Rasterize once at startup at the sizes used
   (root row + program row) and cache the buffers.
4. Rewrite `userland/apps/desktop/src/icons.rs`: keep the `Icon` enum, map each
   variant → its SVG bytes, and blit the cached raster. Keep the `draw()` call
   site at `main.rs:417` working (drop or ignore the `fg`/`accent` params).
5. Optionally align ring-3 icon sizes with the kernel's `ROOT_ICON_SIZE = 24` /
   `PROGRAM_ICON_SIZE = 18` (`start_menu.rs:27-28`).

### Behavior note (confirm)

Adopting the SVGs means the ring-3 icons become **fixed full color** and stop
tracking the theme `fg`/`accent` — exactly matching the kernel, which blits
SVGs untinted regardless of disabled/highlighted state. Assumed intended.

## Part 2 — Delete the kernel `guishell`

### Keep (relocate)

Move to a small kernel module (e.g. `src/window/desktop_root.rs`), since both
are still called at boot for the ring-3 shell:

- `guishell::init_desktop_root_only()` (`mod.rs:248-291`) — screen +
  desktop-root wallpaper, no chrome.
- `guishell::spawn_ring3_desktop_shell()` (`mod.rs:509-511`).

### Delete

- The ring-0 body of `src/commands/guishell/mod.rs`: `init_guishell`,
  `spawn_guishell_process`, `guishell_process_main`, `GUIShellState`,
  `queue_action`/`PendingAction`, taskbar policy (`sync_taskbar_buttons` etc.),
  `run_command_argv`/`web_browser_argv`, `signal_guishell`. Then remove the now
  ring-0-only module `src/commands/guishell/` (and the `pub mod guishell;` in
  `src/commands/mod.rs:2`).
- Kernel window pieces used only by the ring-0 shell — verify each first:
  `src/window/windows/start_menu.rs` (its SVG-icon logic now lives in ring-3),
  `src/window/windows/taskbar.rs`, `src/window/dialogs/run.rs`. Remove their
  `mod` declarations in `src/window/windows/mod.rs` / `src/window/dialogs/`.
- `src/window/manager.rs:692` `guishell::signal_guishell()` call (a no-op in
  ring-3 anyway — `process_id` is `None`).
- Boot wiring in `src/kernel.rs`: drop `ring3_desktop_shell_requested()`
  (`:608-612`) and the ring-0 branch (`init_guishell_desktop` `:211-212`,
  `spawn_guishell_process` `:833`); boot always does root-only + spawn ring-3
  shell. Remove the `AGENTICOS_SHELL` / `opt/agenticos/shell` fw_cfg selection
  and any launch-script defaults for it (`scripts/qemu-compositor.sh`,
  `.conductor/run.sh`, `build.sh` if present).
- Tests: `src/tests/start_menu_tests.rs` (entire module — it drives the kernel
  Start menu) and its registration in `src/tests/mod.rs:89,208`. Fix comments
  in `src/tests/gui_userland.rs` that reference guishell.

### Leave alone

- `src/commands/gui_launch_table.rs` — independent empty skeleton, out of scope.

### Docs

Update `src/commands/CLAUDE.md`, root `CLAUDE.md`, and
`docs/window_system_design.md` to drop all `ring0`/guishell-fallback language,
and mark `2026-07-21-001-feat-ring3-desktop-shell-plan.md`'s "remaining:
deleting guishell" item done.

## Risks / open decisions

- **No ring-0 fallback after removal.** If `DESKTOP.ELF` ever fails to load
  there is no in-kernel shell. Acceptable per the migration endgame, but worth
  an explicit sign-off.
- **Icons become theme-independent** (fixed color), matching the kernel. If
  theme-tinted icons are still wanted, that would require tinting the SVG
  raster or keeping some procedural icons.
- **Rasterizer duplication** (Option A). Accepted for now; A′ (shared crate) is
  the cleanup path if a second consumer appears.

## Validation

- Boot the default (ring-3) desktop; confirm the Start menu and Programs
  fly-out render the SVG icons at the correct sizes with correct transparency,
  across Classic / Aero / Futurism themes.
- `./test.sh` green after removing `start_menu_tests` and the guishell code
  (no dangling references).
- Grep for `guishell`, `GUIShell`, `start_menu`, `signal_guishell`,
  `AGENTICOS_SHELL` to confirm no stale references remain.

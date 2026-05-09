# `src/window/` — Window System

Hierarchical GUI window management with parent-child coordinate transformations, event routing through the window tree, configurable double buffering, and hardware-cursor mouse support. The kernel boots into a GUI desktop (blue background) with a windowed terminal.

## Key files

- `mod.rs` — window-system init and global functions (`render_frame`, etc.).
- `types.rs` — core types: `WindowId`, `ScreenId`, `Rect`, `Point`, `ColorDepth`.
- `event.rs` — keyboard, mouse, and window events.
- `graphics.rs` — `GraphicsDevice` trait that abstracts rendering targets.
- `manager.rs` — `WindowManager`. Coordinates windows and screens. Owns `render_window_tree_with_offset`, which performs parent-child coordinate transformation.
- `screen.rs` — virtual screen abstraction (today there is one physical display).
- `console.rs` — kernel `print!` macro output buffer.
- `cursor.rs` — `CursorRenderer`. Background save/restore for clean cursor movement.
- `keyboard.rs` — PS/2 scancode-set-2 → `KeyCode` conversion *for window events* (distinct from the lower-level driver in `src/input/`).
- `terminal.rs`, `terminal_factory.rs` — terminal-window support; the factory wires terminal windows up to the shell.
- `windows/` — concrete window implementations: `base.rs` (parent-child tracking), `container.rs`, `text.rs` (grid-based text), `terminal.rs` (interactive), `frame.rs` (title bar + borders), `desktop.rs` (background).
- `adapters/` — `GraphicsDevice` implementations: `direct_framebuffer.rs` (fast, used for cursor) and `double_buffered.rs` (smooth).
- `dialogs/` — dialog-window scaffolding.

## Window types

| Type | Purpose | Notes |
|---|---|---|
| `DesktopWindow` | Full-screen background | Optionally owns BMP wallpaper bytes (loaded via `window::load_default_wallpaper`) and blits them through `GraphicsDevice::draw_image_scaled`. Falls back to solid blue (RGB `0, 50, 100`) when no wallpaper is provided or parsing fails — boot must succeed in either branch. |
| `FrameWindow` | Title bar + borders | Active = blue chrome; inactive = grey. Title bar 24 px, border 2 px. Uses `WindowBase`. |
| `TextWindow` | Grid-based text rendering | Cell size derived from the system TTF (`get_default_font().cell_width()` × `line_height()`). Tracks dirty cells for incremental updates. Dark grey background (RGB `32, 32, 32`). |
| `TerminalWindow` | Interactive terminal | Wraps `TextWindow`, adds input handling, command history, cursor. |
| `ContainerWindow` | Generic parent | For grouping children. |

All windows derive from `WindowBase` for consistent parent-child tracking.

## Default desktop layout

Boot lands in GUI mode:

- `DesktopWindow` (full-screen). Reads `/WALLPAPR.BMP` from the FAT root via `window::load_default_wallpaper` during `init_guishell`; on success, the BMP is stretched to the full screen via `GraphicsDevice::draw_image_scaled`. Missing or malformed wallpaper degrades to the legacy solid-blue fill — never panics.
- `FrameWindow` titled "AgenticOS Terminal" at `(100, 50)`, 800×600 (or smaller if the screen is smaller).
- `TerminalWindow` inside the frame.

## Coordinate transformation

Child windows are positioned relative to their parent's coordinate system. `render_window_tree_with_offset` (in `manager.rs`) walks the tree, accumulating the offset into each child's bounds during render. Child bounds are temporarily adjusted during rendering — read from `WindowBase` after the render call to get the original.

## Z-order

There is a single source of truth for z-order: each parent's `children` Vec. `children[0]` is the bottom-most sibling, `children[len-1]` is the top. Both rendering (`render_window_tree`) and hit-testing (`topmost_at`, `start_drag_if_on_title_bar`) read from this same ordering, so they cannot drift. `bring_to_front(id)` moves the window to the end of its parent's children, then walks up the ancestor chain doing the same — so focusing a deep child also surfaces the enclosing frame above its siblings. `focus_window` calls `bring_to_front` automatically; callers should not need to call both.

## Cursor rendering

Owned by this folder, NOT `src/drivers/`. `CursorRenderer` (in `cursor.rs`) saves the framebuffer region under the cursor before drawing, restores it before the next move. The 12×12 arrow sprite lives in `src/graphics/mouse_cursor.rs`. Cursor uses the direct-framebuffer adapter (the double-buffered path is too slow for cursor latency).

## Implementation status

- **Phases 1-3** (core + basic windows + graphics integration): complete.
- **Phase 4** (UI controls): partial — `FrameWindow`, focus management, mouse interaction done.
- **Phase 5** (drag/drop, resize, menus): future.

## Cross-references

- Drawing primitives (lines, fonts, compositor) live in `src/graphics/` — see `src/graphics/CLAUDE.md`.
- Typed input events come from `src/input/` — see `src/input/CLAUDE.md`.
- Mouse hardware (PS/2, VirtIO tablet) lives in `src/drivers/` — see `src/drivers/CLAUDE.md`.
- Detailed architecture: `docs/window_system_design.md`.
- Shell ↔ terminal-window integration: `docs/shell_window_integration.md`.

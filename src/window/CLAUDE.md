# `src/window/` — Window System

Hierarchical GUI window management with parent-child coordinate transformations, event routing through the window tree, configurable double buffering, and hardware-cursor mouse support. The kernel boots into a GUI desktop (blue background) with a windowed terminal.

## Key files

- `mod.rs` — window-system init and global functions (`render_frame`, `process_terminal_output`, `process_event`).
- `compositor.rs` — U10 compositor kernel thread. Spawned at boot from `src/kernel.rs`; its loop polls input + processes terminal output + calls `render_frame` then `yield_current`. Storage uses interrupt-driven VirtIO DMA, so input and rendering continue during binary loads.
- `types.rs` — core types: `WindowId`, `ScreenId`, `Rect`, `Point`, `ColorDepth`.
- `event.rs` — keyboard, mouse, and window events.
- `graphics.rs` — `GraphicsDevice` trait that abstracts rendering targets.
- `manager.rs` — `WindowManager`. Coordinates windows and screens. Owns `render_window_tree_with_offset`, which performs parent-child coordinate transformation.
  Left-button routing captures the pressed window through motion and release,
  so draggable controls continue receiving events outside their hit bounds.
- `renderer/` — boot policy and two real renderer siblings. `legacy` preserves
  the dirty framebuffer/cursor path; `retained` rasterizes the desktop and each
  visible top-level subtree into separate premultiplied surfaces, builds a flat
  scene, and uses either the CPU reference engine or the qualified VirGL engine.
  CPU output presents through the boot framebuffer or VirtIO-GPU 2D; VirGL
  presents its host texture directly and uses the VirtIO hardware cursor.
- `theme/` — the theme system. Each theme is a `ThemeSpec` (token, frame
  metrics, frame/chrome compositor effects, painter fn) resolved through
  `spec_for(kind)`; adding a theme means adding a spec + palette/style and a
  painter only if it introduces a new finish. Three built-ins: `classic`
  renders Windows 98 "Windows Standard" chrome — raised 3D bevel border,
  horizontal caption gradient, raised ButtonFace close button; `aero`
  supplies translucent rounded glass, shadows, and radius-6 backdrop blur;
  `futurism` (the Auto default on retained CPU/VirGL) draws a frosted dark
  translucent title bar over radius-6 backdrop blur (the qualified VirGL
  pipeline's maximum) meeting the content well directly, content flush to
  the window edge inside a 1px dark hairline
  rim (no light borders), 12px-rounded top corners, a soft 22px drop shadow,
  and a rounded soft-red close button. Its rounded *bottom* corners are
  carved by `Window::paint_overlay` — a post-children pass the manager runs
  so the frame can replace the client's corner pixels with the shadowed arc
  (surface ARGB writes are exact replacement); `ThemeSpec.draw_frame_overlay`
  opts a theme in. `frame_util.rs` holds the shared shadow/corner geometry
  both translucent painters use. Caption-button geometry is data-driven via
  `FrameMetrics.button_*`, shared by painting and `manager.rs` hit-testing
  through `theme::close_button_rect`. `controls.rs` is the single source of
  truth for *control* surfaces: a theme-dispatched `ControlPalette`
  (`controls::palette()`) plus a `ControlStyle` whose `ControlFinish`
  (`Bevel98` / `GlassKd4` / `SoftRounded`) drives the drawing helpers
  (`draw_button`, `draw_field`, `draw_raised_panel`, `draw_selection`,
  `draw_menu_surface`, and the chrome helpers `draw_taskbar_surface` /
  `draw_tray_well` / `draw_task_button` / `taskbar_text`). Every widget in
  `windows/` delegates its surface rendering there — Classic gets Win98
  bevels and navy selection, Aero rounded gradient buttons and `#CBE8F6`
  selection, Futurism flat white rounded buttons, `#3C8CF0` accent, rounded
  `#DCE9FC` selection pills, and frosted translucent taskbar/start-menu
  surfaces (`theme::chrome_effect()`). The resolved theme is published to
  ring-3 as `/etc/theme` (see `src/userland/etc.rs::publish_theme`), which
  `userland/libs/gui`'s `theme` module mirrors (unknown tokens degrade to
  Classic). Runtime changes from `CONTROL.ELF` retheme every frame, preserve
  client sizes, update compositor effects, repaint kernel controls,
  republish `/etc/theme`, and broadcast a coalesced process-global GUI event
  (payload codes 1=Classic, 2=Aero, 3=Futurism). Normative Classic/Aero
  color tables live in
  `docs/plans/2026-07-18-003-feat-theme-aware-controls-plan.md`; Futurism's
  in `docs/plans/2026-07-18-007-feat-futurism-theme-plan.md`.
- `screen.rs` — virtual screen abstraction (today there is one physical display).
- `console.rs` — kernel `print!` macro output buffer.
- `cursor.rs` — `CursorRenderer`. Background save/restore and the 12×12 arrow sprite.
- `keyboard.rs` — PS/2 scancode-set-2 → `KeyCode` conversion *for window events* (distinct from the lower-level driver in `src/input/`).
- `terminal.rs`, `terminal_factory.rs` — terminal-window support; the factory wires terminal windows up to the shell.
- `windows/` — concrete window implementations: `base.rs` (parent-child tracking), `container.rs`, `text.rs` (grid-based text), `terminal.rs` (interactive), `frame.rs` (title bar + borders), `desktop.rs` (background), `start_menu.rs` (classic root menu plus Programs fly-out in one active popup, with ordinary SVG assets under `assets/icons/start/`), and `taskbar.rs` (task-button geometry plus the minute-updated UTC tray).
- `windows/remote_surface.rs` — server-decorated client surface for ring-3
  apps. It owns the copied XRGB8888 buffer and forwards input/resize/close/focus
  events to the owning PID's GUI queue. Its enclosing frame title can be
  updated through the ownership-checked ring-3 GUI ABI. Under strict VirGL it
  may instead own a logical GL client ID whose front texture is inserted as an
  external retained layer clipped to the content well.
- `adapters/` — `GraphicsDevice` implementations: `direct_framebuffer.rs` (fast, used for cursor) and `double_buffered.rs` (smooth).
- `dialogs/` — kernel dialog-window scaffolding, including the non-blocking Run dialog. Run keeps input state outside the manager registry and launches submitted text through zsh `-c`.

## Window types

| Type | Purpose | Notes |
|---|---|---|
| `DesktopWindow` | Full-screen background | Owns optional live-replaceable BMP wallpaper bytes and blits them through `GraphicsDevice::draw_image_scaled`. Falls back to solid blue (RGB `0, 50, 100`) when no wallpaper is provided or parsing fails — boot must succeed in either branch. |
| `FrameWindow` | Title bar + borders | Metrics and painting come from the active Classic/Aero theme. Aero requires the retained renderer. Uses `WindowBase`. |
| `TextWindow` | Grid-based text rendering | Cell size derived from the system TTF (`get_default_font().cell_width()` × `line_height()`). Tracks dirty cells for incremental updates. Dark grey background (RGB `32, 32, 32`). |
| `TerminalWindow` | Interactive terminal | Wraps `TextWindow`, adds input handling, command history, cursor. |
| `ContainerWindow` | Generic parent | For grouping children. |
| `StartMenuWindow` | GUIShell Start popup | Theme-aware popup panels, selection-colored rotated `AgenticOS` banner, typed disabled/separator/action rows, SVG-backed root/program icons, and an in-window Programs fly-out so outside-click dismissal still tracks one popup. |
| `TaskbarTrayWindow` | Right-side notification tray | Theme-aware tray well (recessed bevel Classic, flat border Aero, translucent rounded well Futurism) with `HH:MM UTC` and `YYYY-MM-DD`; compares the RTC-backed epoch minute in `prepare_for_render` and invalidates only at minute boundaries. |
| `RemoteSurface` | Ring-3 client pixels | Kernel-owned copy-blit buffer or one attached VirGL client texture; close requests are delivered to the client. |

All windows derive from `WindowBase` for consistent parent-child tracking.

## Default desktop layout

Boot lands in GUI mode:

- `DesktopWindow` (full-screen). Reads `/WALLPAPR.BMP` from the FAT root via `window::load_default_wallpaper` during `init_guishell`; on success, the BMP is stretched to the full screen via `GraphicsDevice::draw_image_scaled`. Missing or malformed wallpaper degrades to the legacy solid-blue fill — never panics.
- `FrameWindow` titled "Terminal" at `(100, 50)`, 800×600 (or smaller if the screen is smaller). After command submission, its title shows `Terminal - ` followed by the first 40 characters of the command.
- `TerminalWindow` inside the frame.
- Bottom taskbar with a Start button, dynamically-sized frame buttons, and a
  recessed right-side UTC date/time tray. Start opens the classic menu;
  Programs launches the six pinned apps (including GL Arena), Run opens a modal command field, and
  Shut Down is an explicit safe placeholder until a clean power-off path
  exists. Task-button layout reserves the tray span and never overlaps it.

## TerminalWindow ↔ terminal subsystem

Since the ANSI/VT overhaul (docs/plans/2026-05-24-001-...), `TerminalWindow` owns a `terminal::vte::Vte` parser + `terminal::screen::Screen` and the `TextWindow` is just a renderer. On every `prepare_for_render`, TerminalWindow drains the pty master's output queue, feeds bytes through `Vte → Screen`, pushes DSR replies back into the slave's input, and copies the Screen's visible viewport down to TextWindow via `set_cell`. Local echoes (canonical-mode typing) go through the parser too — single source of truth.

PTY lookup goes through `terminal::pty::master_for_terminal(WindowId)` / `slave_for_terminal(WindowId)`. `userland::stdin` and `userland::tty` are now shims over `terminal::pty`.

## Coordinate transformation

Child windows are positioned relative to their parent's coordinate system. `render_window_tree_with_offset` (in `manager.rs`) walks the tree, accumulating the offset into each child's bounds during render. Child bounds are temporarily adjusted during rendering — read from `WindowBase` after the render call to get the original.

## Z-order

There is a single source of truth for z-order: each parent's `children` Vec. `children[0]` is the bottom-most sibling, `children[len-1]` is the top. Both rendering (`render_window_tree`) and hit-testing (`topmost_at`, `start_drag_if_on_title_bar`) read from this same ordering, so they cannot drift. `bring_to_front(id)` moves the window to the end of its parent's children, then walks up the ancestor chain doing the same — so focusing a deep child also surfaces the enclosing frame above its siblings. `focus_window` calls `bring_to_front` automatically; callers should not need to call both.

## Cursor rendering

Owned by this folder, NOT `src/drivers/`. `CursorRenderer` (in `cursor.rs`) owns the 12×12 arrow sprite, saves the framebuffer region under the cursor before drawing, and restores it before the next move. Cursor uses the direct-framebuffer adapter (the double-buffered path is too slow for cursor latency).

That save/restore behavior applies only to `legacy`. In `retained`, the cursor
is drawn as the final canonical output overlay after damaged regions have been
recomposed; it never restores framebuffer background.

## Window-manager synchronization

`WINDOW_MANAGER` uses `PreemptionMutex`, not `InterruptMutex`: a render may be
long, so the PIT and device IRQs must remain enabled while its lock is held.
The timer continues advancing time, but takes only its minimal tick/EOI path;
scheduler housekeeping and kernel-thread context switches resume after the
critical section ends. Interrupt handlers must never access the manager
directly; they enqueue input or work for the compositor thread to consume. This
invariant prevents same-CPU spin-lock deadlocks, interrupt-context allocator
activity during rendering, and drag-time clock starvation.

## Renderer boot policy

`build.sh` passes `opt/agenticos/compositor` (`legacy`, `retained`, `gpu`, or
`auto`) and `opt/agenticos/gpu_strict` through QEMU `fw_cfg`. Missing policy
defaults to `legacy`. `gpu` and `auto` select VirGL after capset, clear,
alpha/readback, and lifecycle qualification; non-strict requests fall back to
retained CPU if qualification or runtime composition fails. Strict GPU mode
fails initialization or panics on a runtime GPU failure instead.
On macOS, an explicit host-side `gpu` request must first pass the pinned custom
QEMU verifier; see `docs/macos-virgl-qualification.md`.
`AGENTICOS_THEME=classic|aero|futurism|auto` is passed as
`opt/agenticos/theme`. Explicit Aero or Futurism is available on retained CPU
and qualified VirGL; VirGL performs the radius-6 frame/chrome backdrop blur
on the host GPU (larger radii are rejected by the qualified blur pipeline —
see `gpu_backdrop_radius_supported`). `auto` selects Futurism for
retained CPU and qualified VirGL. Legacy selects Classic, and a non-strict
runtime fallback to legacy activates Classic before repainting.

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

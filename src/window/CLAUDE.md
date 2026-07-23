# `src/window/` — Window System

Hierarchical GUI window management with parent-child coordinate
transformations, event routing through the window tree, configurable double
buffering, hardware-cursor mouse support, and copy-blit surfaces owned by
ring-3 applications.

## Key files

- `mod.rs` — window-system init and global functions (`render_frame`,
  `process_event`).
- `compositor.rs` — compositor kernel thread. Spawned at boot from
  `src/kernel.rs`; its loop polls input and calls `render_frame`. Storage uses
  interrupt-driven VirtIO DMA, so input and rendering continue during binary
  loads.
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
  horizontal caption gradient, raised ButtonFace caption buttons; `aero`
  supplies translucent rounded glass, shadows, and radius-6 backdrop blur;
  `futurism` (the Auto default on retained CPU/VirGL) draws a frosted dark
  translucent title bar over radius-6 backdrop blur (the qualified VirGL
  pipeline's maximum) meeting the content well directly, content flush to
  the window edge inside a 1px dark hairline
  rim (no light borders), 12px-rounded top corners, a soft 22px drop shadow,
  and rounded caption controls with a soft-red close button. Backdrop strength follows the frame's
  effective alpha, so the blurred shadow gutter fades smoothly to the sharp
  desktop. Its rounded *bottom* corners are
  carved by `Window::paint_overlay` — a post-children pass the manager runs
  so the frame can replace the client's corner pixels with the shadowed arc
  (surface ARGB writes are exact replacement); `ThemeSpec.draw_frame_overlay`
  opts a theme in. `frame_util.rs` holds the shared shadow/corner geometry
  both translucent painters use. Caption-button geometry is data-driven via
  `FrameMetrics.button_*` and `theme::caption_button_layout`, shared by all
  three painters and `manager.rs` hit-testing. Resizable frames expose real
  minimize, maximize/restore, and close controls; fixed frames are close-only.
  `controls.rs` is the single source of
  truth for *control* surfaces: a theme-dispatched `ControlPalette`
  (`controls::palette()`) plus a `ControlStyle` whose `ControlFinish`
  (`Bevel98` / `GlassKd4` / `SoftRounded`) drives the drawing helpers
  (`draw_button`, `draw_field`, `draw_raised_panel`, `draw_selection`,
  `draw_menu_surface`, and `draw_task_button`, still used by the `Button`
  widget's taskbar style). The kernel taskbar/tray/Start-menu chrome helpers
  (`draw_taskbar_surface` / `draw_tray_well` / `taskbar_text` / the
  `theme::chrome_effect()` accessor) were removed with the kernel `guishell`;
  the ring-3 shell carries its own copies in `userland/libs/gui::theme`. Every
  widget in `windows/` delegates its surface rendering here — Classic gets
  Win98 bevels and navy selection, Aero rounded gradient buttons and `#CBE8F6`
  selection, Futurism flat white rounded buttons, `#3C8CF0` accent, and rounded
  `#DCE9FC` selection pills. The resolved theme is published to
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
- `windows/` — concrete window implementations: `base.rs` (parent-child
  tracking), `container.rs`, `frame.rs` (title bar + borders), `desktop.rs`
  (background), ordinary widgets, and `remote_surface.rs` for ring-3 clients.
  The interactive terminal, taskbar, and Start menu are ring-3 applications.
- `windows/remote_surface.rs` — server-decorated client surface for ring-3
  apps. It owns the copied XRGB8888 buffer and forwards input/resize/close/focus
  events to the owning PID's GUI queue. Its enclosing frame title can be
  updated through the ownership-checked ring-3 GUI ABI. Under strict VirGL it
  may instead own a logical GL client ID whose front texture is inserted as an
  external retained layer clipped to the content well.
- `adapters/` — `GraphicsDevice` implementations: `direct_framebuffer.rs` (fast, used for cursor) and `double_buffered.rs` (smooth).
- `dialogs/` — kernel dialog-window scaffolding: `message_box` (info/warning/error). The kernel Run dialog was removed with `guishell`; the ring-3 shell owns its own Run window.

## Window types

| Type | Purpose | Notes |
|---|---|---|
| `DesktopWindow` | Full-screen background | Owns optional live-replaceable BMP wallpaper bytes and blits them through `GraphicsDevice::draw_image_scaled`. Falls back to solid blue (RGB `0, 50, 100`) when no wallpaper is provided or parsing fails — boot must succeed in either branch. |
| `FrameWindow` | Title bar + borders | Theme-painted caption controls; resizable frames minimize to the taskbar and maximize/restore within the desktop work area, while fixed frames remain close-only. Uses `WindowBase`. |
| `ContainerWindow` | Generic parent | For grouping children. |
| `RemoteSurface` | Ring-3 client pixels | Kernel-owned copy-blit buffer or one attached VirGL client texture; close requests are delivered to the client. |

The Start menu and taskbar/tray are no longer kernel windows — the ring-3 shell
(`userland/apps/desktop/`) draws them as undecorated/panel `RemoteSurface`s.

All windows derive from `WindowBase` for consistent parent-child tracking.

## Default desktop layout

Boot lands in GUI mode. The kernel stands up only the desktop root
(`commands::desktop::init_desktop_root_only`); the taskbar, Start menu, tray,
and any initial windows are owned by the ring-3 shell (`DESKTOP.ELF`).

- `DesktopWindow` (full-screen). Loads the configured wallpaper BMP via
  `system_control::load_configured_wallpaper` when the desktop root is created;
  on success, the BMP is stretched to the full screen via
  `GraphicsDevice::draw_image_scaled`. Missing or malformed wallpaper degrades
  to the solid-blue fill — never panics.
- The ring-3 shell docks a bottom taskbar panel (Start button, frame buttons,
  UTC date/time tray) and opens frames on demand. Minimized frames keep their
  task button; activating it (via `gui_shell_window_action` →
  `WindowManager::activate_frame`) restores the frame in its retained
  normal/maximized placement and returns focus to its content. See
  `src/commands/CLAUDE.md`.

## Ring-3 terminal boundary

The window system sees the terminal only as a `RemoteSurface` owned by
`TERMINAL.ELF`. Keystrokes and close/focus events use the ordinary GUI event
queue; the app presents its rendered XRGB8888 cell surface through
`gui_win_present`. VT parsing, scrollback, caret state, key encoding, and font
rasterization are entirely outside the kernel window tree.

The kernel PTY service remains under `src/terminal/pty.rs`. `pty_open` keys a
PTY to the terminal app's owned content-surface `WindowId`, and GUI cleanup
releases that PTY when the surface owner exits.

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
- **Phase 4** (UI controls): partial — decorated `FrameWindow`, functional
  caption controls, focus management, and mouse interaction done.
- **Phase 5** (advanced interaction): moving, edge resize, minimize/maximize,
  menus, and dialogs done; drag/drop and snapping remain future work.

## Cross-references

- Drawing primitives (lines, fonts, compositor) live in `src/graphics/` — see `src/graphics/CLAUDE.md`.
- Typed input events come from `src/input/` — see `src/input/CLAUDE.md`.
- Mouse hardware (PS/2, VirtIO tablet) lives in `src/drivers/` — see `src/drivers/CLAUDE.md`.
- Detailed architecture: `docs/window_system_design.md`.
- Ring-3 terminal migration:
  `docs/plans/2026-07-21-001-feat-terminal-emulator-userland-plan.md`.

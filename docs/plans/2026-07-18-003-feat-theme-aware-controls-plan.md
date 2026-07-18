---
title: "feat: theme-aware window controls (Aero / Classic) across kernel and ring-3 widgets"
type: feat
status: complete
date: 2026-07-18
---

# feat: theme-aware window controls (Aero / Classic) across kernel and ring-3 widgets

## Summary

The boot-selected theme (`src/window/theme/`, `AGENTICOS_THEME=classic|aero|auto`) currently styles only the **window frame** (title bar, borders, close button). Everything inside a window — every button, text field, list, menu, scrollbar, taskbar — draws one hardcoded style regardless of theme. This plan makes all controls follow the active theme:

- **Classic** — Windows 98 "Windows Standard": raised/sunken 3D bevels on ButtonFace `#C0C0C0`, navy selection, consistent with the existing classic frame in `src/window/theme/classic.rs`.
- **Aero** — the Windows 7 look from the reference screenshot: rounded-corner buttons with a subtle 4-stop vertical gradient (white → light grey), 1px grey border, inner white highlight, and a blue-glow border for the default/hot state.

Two widget worlds must both convert:

1. **Kernel-side widgets** (`src/window/windows/`: `Button`, `TextInput`, `List`, `MenuBar`, `Taskbar`, `Toolbar`, `ProgressBar`, `ScrollView`, `StatusBar`, `TreeView`, …) used by guishell, explorer, tasks, and the kernel dialogs in `src/window/dialogs/`.
2. **Ring-3 toolkit widgets** (`userland/libs/gui`: `Button`, `TextField`, `ListView`, `MenuBar`) and the common-dialogs library (`userland/libs/dialogs`), used by notepad, guidemo, and future ports.

Ring-3 apps can't see kernel state, so the kernel publishes the selected theme as a new kernel-managed `/etc/theme` file (same pattern as DHCP's `/etc/resolv.conf`) and the toolkit reads it once at startup. **No new syscalls, no GUI ABI change.**

## Problem Frame

- `theme::active()` is decided once at boot (`theme::init_boot_policy`, called from `WindowManager` init at `src/window/manager.rs:106`) and is honored only by `FrameWindow` chrome via `theme::draw_frame`.
- Kernel widgets draw a fixed Win9x-ish style from the shared `PALETTE_*` constants in `src/window/mod.rs` plus per-widget hardcodes (e.g. `windows/button.rs` paints its own white/dark-grey bevel). Under Aero, glassy frames wrap Win95-looking interiors.
- The ring-3 toolkit is flatter still: `gui::Button::draw` is a flat `COLOR_PANEL` fill + 1px `COLOR_BORDER` rect, with "hot" rendered as a solid blue fill. The dialogs library inherits this. Nothing in ring 3 even knows which theme is active — there is no channel for it.
- The reference screenshot (`Nouvelle` button) defines the Aero button target: rounded corners (~3px radius), thin blue outer border with a light-blue glow, 1px inner white highlight, vertical gradient from near-white to light grey, black centered label.

## Key design decisions

**KD1 — One kernel source of truth: `src/window/theme/controls.rs`.**
A `ControlPalette` (colors) + state-aware drawing helpers, dispatched on `theme::active()`:

```rust
pub enum ControlState { Normal, Hot, Pressed, Disabled }
pub fn palette() -> &'static ControlPalette;            // per active theme
pub fn draw_button(device, rect, state);                 // face + edges, no label
pub fn draw_sunken_field(device, rect);                  // TextInput/List/scroll wells
pub fn draw_raised_panel(device, rect);                  // taskbar, toolbars, status bar
pub fn draw_selection(device, rect) / selection_text();  // list/menu highlight
```

Widgets keep owning layout, labels, and hit-testing; they delegate *surface rendering* to these helpers. Theme is boot-static, so reading `theme::active()` at paint time is safe and no invalidation machinery is needed.

**KD2 — Ring-3 discovery via kernel-managed `/etc/theme`.**
Boot ordering supports it: `etc::init()` runs at `kernel.rs:93`, the window manager (and theme selection) initializes at `init_display` a few lines later, so a `etc::publish_theme()` call placed in `kernel.rs` right after `init_display` writes `aero\n` or `classic\n` with the filesystem already up and the final (fallback-resolved) theme known. This mirrors `net::resolver_config::publish` — including "write outside any window-manager lock". Userland reads it once at startup; missing/garbled file defaults to Classic. No ABI version bump; `gui_win_create`'s `flags` stays `0`.

**KD3 — Userland mirrors the palette; it does not share code with the kernel.**
The kernel draws through `GraphicsDevice` (with alpha, TTF text); the toolkit draws into an opaque XRGB `Canvas` with an 8×8 font. Sharing a crate would couple `no_std` kernel graphics to the Linux-ABI userland build for ~30 color constants. Instead `userland/libs/gui` gets its own `theme` module with the same named colors, kept in sync by the spec table below (single place both sides cite).

**KD4 — Aero control spec (from the screenshot + Win7 values).**

| Element | Normal | Hot / Default | Pressed | Disabled |
|---|---|---|---|---|
| Border (1px, radius 3) | `#707070` | `#3C7FB1` + 1px outer glow `#A9D4F0` | `#2C628B` | `#ADB2B5` |
| Inner highlight (1px) | `#FCFCFC` | `#FCFCFC` | none (inner shadow `#9DB6C8`) | none |
| Fill gradient (4 stops, top→bottom) | `#F2F2F2 #EBEBEB #DDDDDD #CFCFCF` | `#EAF6FD #D9F0FC #BEE6FD #A7D9F5` | `#E5F4FC #C4E5F6 #98D1EF #68B3DB` | flat `#F4F4F4` |
| Label | black | black | black | `#838383` |

Fields/lists: white interior, 1px `#ABABAB` border (focus: `#3C7FB1`). Selection: `#CBE8F6` fill + `#26A0DA`-ish border, black text (Aero keeps dark text on light selection). Panels (taskbar/toolbar/status): vertical gradient `#F0F5FA → #CFD9E4` with a 1px `#B6BCC6` edge.

**KD5 — Classic control spec.**
Reuse the constants already in `theme/classic.rs` (export them): FACE `#C0C0C0`, BEVEL_HIGHLIGHT white, BEVEL_LIGHT `#DFDFDF`, BEVEL_SHADOW `#808080`, BEVEL_DARK black.

- Raised (button/panel): outer top-left white, outer bottom-right black; inner top-left `#DFDFDF`, inner bottom-right `#808080`. Pressed: sunken (inverted) + label shifted (+1,+1). Default button: extra 1px black rim outside the bevel.
- Sunken (field/list well): outer top-left `#808080`, inner top-left black, outer bottom-right white, inner bottom-right `#DFDFDF`; white interior.
- Selection & menu highlight: navy `#000080` + white text.

**KD6 — Button state model stays small.**
Kernel `Button` already tracks `pressed` + `enabled`; map those onto `ControlState`. The ring-3 `Button::draw(canvas, hot)` boolean keeps its signature (`hot` = default/accent button, used today by the dialogs) and maps to `Hot` vs `Normal`; a `draw_state(canvas, state)` superset is added for apps that track pressed state. True mouse-hover tracking (Move-event driven) is **out of scope** for both worlds — nothing tracks hover today and it's orthogonal plumbing.

**KD7 — Calc keeps its bespoke dark style.**
`CALC.ELF`'s dark key grid is deliberate app styling (Win10-calculator-like), not stock controls; it doesn't use `gui::Button`. Leave it; note an optional follow-up to offer a themed variant.

## Phases

### Phase 1 — Kernel control-theme foundation

- Add `src/window/theme/controls.rs` per KD1/KD4/KD5; export the classic bevel constants from `classic.rs` for reuse.
- Gradient/rounded helpers: per-row `fill_rect` for gradients (cheap); corner clipping reuses the integer-circle approach already in `aero.rs::outside_rounded_rect`. On opaque targets corners are filled with a caller-supplied background color (no alpha assumption), so the same helper works for the legacy renderer.
- Convert the four core widgets: `windows/button.rs` (delete its inline bevel; themed default while keeping `set_bg_color`/`set_text_color` as explicit overrides — audit call sites, e.g. taskbar), `windows/text_input.rs`, `windows/list.rs`, `windows/menu.rs` + `menu_bar.rs` + `menu_bar_popup.rs`.
- In-kernel tests (new `theme_controls` test module): palette dispatch per `ThemeKind`, button edge pixels for classic raised/sunken, aero gradient stop rows — rendered into the existing test `GraphicsDevice` capture used by window tests.

### Phase 2 — Kernel widget sweep

- Remaining widgets: `taskbar.rs` (Start button + window buttons), `toolbar.rs`, `progress_bar.rs`, `scroll_view.rs` (scrollbar track/thumb), `status_bar.rs`, `splitter.rs`, `path_bar.rs`, `multi_column_list.rs`, `tree_view.rs`, `icon_view.rs`, `dialog.rs`, `label.rs` (text color only).
- Kernel dialogs `src/window/dialogs/{message_box,file_open,file_save}.rs` and app-level color hardcodes in `src/commands/{guishell,explorer,tasks}` route through `controls::palette()`.
- Audit pass: `grep -rn 'Color::new\|PALETTE_' src/window/windows src/window/dialogs src/commands` — every hit either moves to the palette or gets a comment stating why it is theme-invariant (e.g. calc-like intentional styling, wallpaper fallback blue).

### Phase 3 — Publish `/etc/theme`

- `src/userland/etc.rs`: add `THEME_PATH = "/etc/theme"` + `publish_theme(kind: ThemeKind)` (create + write, resolver-style temp/rename not needed — written once before any ring-3 process exists).
- Call from `kernel.rs` immediately after `init_display` (theme now final, `/etc` already populated, no WM lock held). `/etc` is already kernel-owned, so userland writes are rejected for free.
- Test: after boot-policy init in the test harness, `publish_theme` + read-back asserts contents match `theme::active().as_str()`.

### Phase 4 — Ring-3 toolkit theming (`userland/libs/gui`)

- New `theme` module: `enum Theme { Classic, Aero }`, palette tables per KD4/KD5, `Theme::load()` reading `/etc/theme` via `runtime::openat`/`read` (default Classic), and a process-global `theme::current()` set lazily or by an explicit `gui::init()`.
- `Canvas` primitives: `fill_vgradient(x, y, w, h, stops: &[u32])` and `rounded_rect(x, y, w, h, radius, border, bg_outside)` (+ filled variant). Opaque canvas ⇒ corner pixels are painted with `bg_outside`.
- Widgets:
  - `Button` — themed per KD4/KD5 (classic: bevel + pressed label shift; aero: screenshot look; `hot` = blue-border default state). Add `draw_state` per KD6.
  - `TextField` — sunken bevel (classic) / `#ABABAB` border with `#3C7FB1` focus ring (aero); caret unchanged.
  - `ListView` — themed border, selection colors (`navy+white` vs `#CBE8F6`+black), themed scrollbar thumb.
  - `MenuBar` — classic: flat `#C0C0C0` + bevel bottom edge, navy item highlight; aero: panel gradient + `#CBE8F6` highlight.
- Replace the `COLOR_PANEL`/`COLOR_BORDER`/`COLOR_HIGHLIGHT` constants' *uses* inside widgets with palette lookups; keep the constants exported (apps use them for their own chrome) but document them as theme-agnostic legacy values.

### Phase 5 — Dialogs library + app adoption

- `userland/libs/dialogs`: background clears, divider lines, and swatch borders in `file_dialog.rs`, `message_box.rs`, `color_picker.rs` go through `gui::theme` (window background: `#F0F0F0` aero / `#C0C0C0` classic). Buttons/list/field theming arrives free via Phase 4.
- `notepad`: menu bar, status strip, and selection colors via the theme palette.
- `guidemo`: extend to render every widget in every state — the visual reference client for both themes.
- `painting`, `calc`: no stock-control usage to convert (KD7); painting's dialogs pick the theme up via the library.
- Rebuild prebuilt-managed apps only if any prebuilt app links the toolkit (today none do — zsh/BusyBox are unaffected; no `refresh-prebuilt.sh` run expected).

### Phase 6 — Validation and docs

- `cargo fmt` / `cargo clippy` / `cargo check`; `./test.sh theme_controls etc window` (plus full `./test.sh`).
- Manual matrix: `AGENTICOS_THEME=classic ./build.sh` and `AGENTICOS_THEME=aero ./build.sh` (retained compositor), exercising: guishell Start menu + taskbar, explorer, tasks, a kernel dialog, notepad (menus + file dialog + message box), guidemo, `cat /etc/theme` from zsh. Verify legacy-renderer fallback still boots all-Classic.
- Docs: update `src/window/CLAUDE.md` (theme section now covers controls), `userland` docs for `/etc/theme` + toolkit theming, root `CLAUDE.md` current-state line.

## Risks / open questions

- **Kernel `Button` color overrides** — `set_bg_color`/`set_text_color` call sites (taskbar, possibly guishell) may rely on custom colors; the audit in Phase 1 decides per-site: drop the override (adopt theme) or keep it as an intentional exception.
- **Legacy renderer + Classic-only fallback** — controls read `theme::active()`, which is already forced to Classic on legacy, so no extra branching; but verify no widget assumes alpha support (all Phase 1 helpers must draw opaque on classic paths).
- **Palette drift between kernel and userland** — mitigated by the KD4/KD5 tables in this doc being the single normative spec; a follow-up could generate both from one table if it ever churns.
- **`/etc/theme` read failures in ring 3** — silent default to Classic; apps never fail to start because of theming.
- **Repaint cost of gradients** — per-row `fill_rect`, ~30 rows per button; negligible next to existing full-surface repaints (known issue #5 in root CLAUDE.md is unchanged).

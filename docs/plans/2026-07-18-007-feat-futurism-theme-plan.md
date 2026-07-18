---
title: "feat: Futurism theme as the new default, plus a data-driven theme registry"
type: feat
status: completed
date: 2026-07-18
---

# feat: Futurism theme as the new default, plus a data-driven theme registry

## Implementation outcome

Completed on 2026-07-18. All six units landed:

- **U1** — `ThemeKind::from_u8`, a `ThemeSpec` registry (`spec_for` /
  `active_spec`, token, renderer requirement, metrics, frame/chrome effects,
  painter fn) in `src/window/theme/mod.rs`, and a `ControlStyle` with a
  `ControlFinish` (`Bevel98` / `GlassKd4` / `SoftRounded`) in `controls.rs`.
  Every binary theme branch (kernel and ring-3) now reads style data. The
  Aero shadow/corner geometry moved verbatim into `theme/frame_util.rs`,
  parameterized, and the existing Classic/Aero pixel-regression tests pass
  unchanged (the refactor gate).
- **U2** — `theme/futurism.rs` paints the frosted dark title bar (ARGB
  gradient over a radius-6 backdrop blur — initially 10, capped post-landing
  at the qualified VirGL blur pipeline's maximum after a strict-GPU boot
  panicked with `UnsupportedEffect`; a new test pins every theme's effect
  radii to `gpu_backdrop_radius_supported`), hairline rims, 12/8px rounded
  corners, 22px soft shadow, and the rounded soft-red close button.
  `Auto` resolves to Futurism on retained CPU/VirGL. Preference code 3,
  availability bit `1 << 2`, `settings.conf` + `/etc/theme` + fw_cfg token
  `futurism`, broadcast payload code 3.
- **U3** — Futurism taskbar (translucent `#1A2440` tint over backdrop blur),
  translucent rounded task pills with white labels and an accent Start pill
  (`Button::set_taskbar_style`), translucent tray well with white clock
  text, and a frosted start-menu surface. Chrome windows adopt
  `theme::chrome_effect()` (taskbar via `prepare_for_render`, start menu at
  construction). Taskbar height stays 32 (scope cut: a per-theme height
  would ripple through boot layout and runtime relayout for little visual
  gain).
- **U4** — Ring-3 mirror in `userland/libs/gui/src/theme.rs`
  (`Theme::Futurism`, `Finish`, `FUTURISM_PALETTE`, soft-rounded painters,
  token-based `/etc/theme` decode with unknown-token→Classic fallback),
  `THEME_FUTURISM` / `THEME_AVAILABLE_FUTURISM` runtime constants, and
  `Theme::is_modern()` adopted by the common file dialog.
- **U5** — Control Center's Appearance page is table-driven
  (`THEME_TILES`: Automatic, Futurism, Aero Glass, Classic), with a
  Futurism-styled preview used for both the Futurism and Automatic tiles
  and availability derived from the snapshot mask.
- **U6** — Selection-matrix, code/token round-trip, Futurism metrics,
  translucent-frame paint, broadcast-code, and preference-persistence
  tests. Full suite: **871/871 tests pass**. Verified end-to-end in QEMU
  (retained compositor, QMP screendumps): boot lands on Futurism, the Start
  menu/fly-out, terminal frame, and taskbar pills match the reference mock,
  and Control Center switches Futurism ↔ Classic live with persistence
  ("Saved to /data").

Deferred as planned: minimize/maximize caption buttons (window-management
feature; hit-test variants reserved), title-bar app icons, a dedicated UI
sans font, per-theme accent parameterization, and a `theme` CLI. One known
pre-existing quirk observed during verification: during a live theme switch
the switching app's own client content can lag one presentation behind the
chrome until its next event — an artifact of the existing retheme/present
ordering, unchanged by this work.

## Summary

Add a third desktop theme, **Futurism**, modeled on the reference mock at
`.context/attachments/q7SQaJ/image.png`: a modern frosted-glass look with
large rounded corners, soft drop shadows, translucent dark title bars over
backdrop blur, flat rounded controls with hairline borders, a frosted
translucent taskbar with pill-shaped task buttons, and a light near-white
content palette with a `#3C8CF0` blue accent.

Futurism becomes the **default** theme: `ThemeRequest::Auto` resolves to
Futurism on the retained-CPU and VirGL renderers (Classic remains the Legacy
fallback). Classic and Aero stay fully selectable and visually unchanged.

Because more themes are expected after this one, the plan front-loads a
**flexibility refactor**: the current two-theme assumptions (binary
`if active() == Aero` branches, hardcoded `1 => Aero, _ => Classic` decodes,
the Control Center's inline three-tile loop) are replaced with a data-driven
`ThemeSpec` registry on the kernel side, a mirrored style-driven toolkit on
the ring-3 side, and a table-driven Appearance page. After this lands, a
fourth theme should be addable by writing one spec + palette (and optionally
a painter), with no scattered match-arm hunting.

---

## Design language extracted from the mock

All values are logical pixels (the mock is 2x). Exact constants get tuned
against screenshots during implementation; these are the starting points.

### Window chrome

| Property | Value |
|---|---|
| Corner radius | 12 on **all four** corners (Aero is 11 top / 7 bottom) |
| Border | 1px hairline: outer dark `rgba(20,30,60,0.25)`, inner highlight `rgba(255,255,255,0.35)` — no bevel |
| Title bar height | 34 |
| Title bar fill | Translucent dark indigo vertical gradient, ~`#1E2A52` α≈0.88 → `#2A3862` α≈0.80, composited over backdrop blur |
| Backdrop effect | `LayerEffect::BackdropSample { radius: 10 }` |
| Title text | White, default font, left-aligned after a 12px inset; inactive: white at ~65% |
| Shadow | Soft drop shadow, `shadow_margin` 22, peak alpha ~80 active / ~44 inactive, corner-aware falloff (larger and softer than Aero's 16/96) |
| Close button | Rounded-rect ~30×22, radius 7, soft red `#E8564A` fill, white × glyph; hover brightens, pressed darkens |
| Min/max buttons | Present in the mock but **deferred** — see Out of scope |

The mock shows an app icon badge in the title bar and three caption buttons.
Icon badges and minimize/maximize are window-manager features, not theme
properties, and are split out as follow-ups (the `HitTestRegion::MinimizeButton`
/ `MaximizeButton` variants in `src/window/types.rs:393-398` are already
reserved and unused).

### Control palette (kernel `ControlPalette` + ring-3 `Palette`)

| Role | Value |
|---|---|
| content_bg | `#F7F9FC` |
| text | `#1F2937` |
| disabled_text | `#94A3B8` |
| border | `#D3DBE8` |
| field_bg | `#FFFFFF` |
| field_text | `#1F2937` |
| selection_bg | `#DCE9FC` |
| selection_text | `#1D4ED8` |
| progress_fill | `#3C8CF0` (the mock's stated accent) |
| scrollbar_thumb | `#C3CEDF` |
| scrollbar_track | `#EEF2F8` |

### Control finish

- **Buttons**: white fill, radius 8, 1px `border` outline; hot `#F3F7FE` with
  accent-tinted border; pressed `#E4ECF8`; **no pressed label shift** (flat
  modern behavior, unlike Classic).
- **Fields**: white, radius 8, 1px `border`; focused ring is a 2px accent
  border (`#3C8CF0`), not the Classic recessed bevel.
- **Selection**: rounded (radius 6) `selection_bg` fill with 1px `#8FB7F2`
  border — matches the sidebar "Home" pill in the mock.
- **Panels**: flat fill with hairline borders; no raised/recessed bevels.
- **Menu surface** (start menu, dropdowns, combo popups): frosted white —
  ARGB white at ~90% over `BackdropSample { radius: 10 }`, radius 12,
  hairline border, rounded accent-tinted selection rows.
- **Progress**: rounded track + rounded accent fill (mock's "Sync-backed" card).

### Taskbar and tray

- Frosted translucent bar: ARGB dark-blue tint (~`#1A2440` α≈0.35) over
  `BackdropSample { radius: 10 }`; hairline top border; height 48
  (Classic/Aero keep their current height).
- Task buttons: pill/rounded-rect radius 10, translucent white fill
  (~α 0.14, active window α 0.28), 1px `rgba(255,255,255,0.28)` border,
  white labels, small accent "running" dot at the right (mock shows this).
- Start button: accent-tinted rounded rect with white label.
- Tray: no recessed bevel — a rounded translucent well with white text.

### Typography

White title/taskbar text on dark glass, dark slate text on light content.
Reuse the existing bundled fonts (`get_default_font`, JetBrains Mono in the
terminal). A dedicated modern UI sans (Inter-like) is an optional follow-up,
not part of this plan.

---

## Problem frame: why a refactor comes first

The Explore pass found the two-theme assumption baked in at ~15 sites.
Adding Futurism by pattern-matching Aero would triple the branch count and
make theme #4 worse. Concrete hazards:

- `src/window/theme/mod.rs:183-186` — `active()` decodes `1 => Aero,
  _ => Classic`; a third stored value silently becomes Classic.
- `src/window/theme/controls.rs:153,450,465` — binary
  `if active() == Classic` / `== Aero` style branches.
- `userland/libs/gui/src/theme.rs` — binary decode of `/etc/theme`
  (`starts_with(b"aero")`), of the theme-changed payload (`payload[0]==2`),
  and binary widget-paint branches.
- `src/system_control.rs:290-295` — `theme_available_mask` hardcodes two bits.
- `userland/apps/control/src/main.rs:305-337,697-709,817` — the Appearance
  page hardcodes `0..3` tiles, `/ 3` width math, and `kind == 2 || kind == 0`
  preview styling.
- `src/userland/gui.rs:155-163` — broadcast payload codes 1/2 only.

The fix is to make "which theme" a lookup into data, and make painters branch
on **style properties** rather than theme identity.

---

## High-level technical design

### 1. Kernel `ThemeSpec` registry (`src/window/theme/mod.rs`)

```rust
#[repr(u8)]
pub enum ThemeKind { Classic = 0, Aero = 1, Futurism = 2 }

impl ThemeKind {
    pub fn from_u8(v: u8) -> Option<ThemeKind>; // exhaustive, no `_ => Classic`
}

pub struct ThemeSpec {
    pub kind: ThemeKind,
    pub token: &'static str,          // "futurism" — /etc/theme + settings.conf + fw_cfg
    pub display_name: &'static str,   // "Futurism"
    pub requires_modern_renderer: bool, // Aero: true, Futurism: true, Classic: false
    pub frame_metrics: FrameMetrics,
    pub frame_effect: LayerEffect,    // Futurism: BackdropSample { radius: 10 }
    pub palette: ControlPalette,
    pub style: ControlStyle,          // see below
    pub draw_frame: fn(&mut dyn Canvas, &FrameChrome, &FrameMetrics),
}

pub static THEMES: [&ThemeSpec; 3] = [&CLASSIC_SPEC, &AERO_SPEC, &FUTURISM_SPEC];
pub fn spec_for(kind: ThemeKind) -> &'static ThemeSpec;
pub fn active_spec() -> &'static ThemeSpec;
```

`metrics_for`, `frame_effect_for`, `draw_frame_for`, and `ThemeKind::as_str`
become thin reads of the spec. `active()`/`activate()` keep the `AtomicU8`
but decode via `ThemeKind::from_u8` with an explicit Classic fallback only
for a corrupt value.

`select_theme` matrix:

| Request \ Renderer | Legacy | RetainedCpu / Virgl |
|---|---|---|
| Auto | Classic (reason: legacy renderer) | **Futurism** |
| Futurism | Classic fallback + reason | Futurism |
| Aero | Classic fallback + reason | Aero |
| Classic | Classic | Classic |

The runtime renderer-fallback path (`manager.rs:983-1000`) already forces
Classic and republishes — unchanged, it now also covers Futurism.

### 2. `ControlStyle` + finish-driven painters (`src/window/theme/controls.rs`)

Replace theme-identity branches with style data:

```rust
pub enum ControlFinish { Bevel98, GlassKd4, SoftRounded }

pub struct ControlStyle {
    pub finish: ControlFinish,
    pub corner_radius: u32,          // buttons/fields; 0 for Classic
    pub pressed_label_shift: bool,   // Classic: true, others: false
    pub selection_border: bool,      // Aero/Futurism: true
    pub selection_radius: u32,       // Futurism: 6
    pub beveled_panels: bool,        // Classic/Aero raised/recessed vs flat hairline
    pub focus_ring: FocusRing,       // ClassicRecess | AccentBorder
}
```

`draw_button`, `draw_field`, `draw_raised_panel`, `draw_recessed_panel`,
`draw_menu_surface`, `draw_menu_separator`, `draw_selection` dispatch on
`active_spec().style.finish` (a `match` over `ControlFinish`, which is
exhaustive by construction). Classic and Aero rendering must be
**pixel-identical** after this refactor — that is the refactor's acceptance
gate. New `SoftRounded` painters implement the Futurism finish (flat rounded
rects, hairline borders, accent focus ring, rounded selection).

New chrome helpers for the taskbar family:

- `draw_taskbar_surface` / `draw_tray_well` / `draw_task_button` — Classic and
  Aero arms delegate to the existing raised/recessed panel + button paths
  (pixel-identical); the Futurism arm paints the translucent pill styling.
- Per-theme `ChromeMetrics { taskbar_height, task_button_radius, .. }` on the
  spec so the taskbar can grow to 48 under Futurism only.

### 3. Futurism frame painter (`src/window/theme/futurism.rs`)

New painter following `aero.rs`'s structure: shadow → translucent title-bar
gradient (ARGB over the backdrop blur) → hairline border → anti-aliased
rounded corners on all four corners (reusing/generalizing
`finish_rounded_corners`, which already handles top/bottom radii — lift the
corner clipping into a shared helper both painters call) → rounded red close
button → white title text.

`FUTURISM_METRICS`: title_bar 34, border 1, radius 12/12, shadow_margin 22,
button 30×22, right margin 10.

### 4. Chrome windows (`src/window/windows/taskbar.rs`, `start_menu.rs`)

- Taskbar and start menu set their `WindowAttributes.effect` from the active
  spec (`LayerEffect::BackdropSample` under Futurism, `None` otherwise) and
  repaint through the new `draw_taskbar_surface` / existing
  `draw_menu_surface` helpers. `WindowAttributes` already carries `effect`
  (`src/window/types.rs:292`), so no compositor changes are needed.
- On theme change, `apply_theme_request` additionally updates
  taskbar/start-menu effects and re-lays-out the taskbar if
  `taskbar_height` differs (same invalidate-everything path it uses today).
- Tray text color and start-menu selection already flow through
  `controls::palette()` — no per-site changes.

### 5. Persistence, syscall, and event plumbing

- `src/system_control.rs`: `ThemePreference::Futurism = 3` (`parse`/`as_str`
  token `"futurism"`); availability bit `THEME_AVAILABLE_FUTURISM = 1 << 2`,
  set in `snapshot()` whenever the renderer is non-Legacy (same gate as
  Aero); snapshot `active_theme` / publication mapping via `ThemeKind`.
- `src/userland/gui.rs::broadcast_theme_changed`: payload code **3 =
  Futurism** (0 reserved, 1 Classic, 2 Aero — existing codes unchanged).
- `src/userland/etc.rs::publish_theme`: writes the spec token, so
  `/etc/theme` becomes `futurism\n` (token, newline — same shape as today).
- fw_cfg boot policy: `AGENTICOS_THEME=futurism` accepted by
  `ThemeRequest::parse`; `auto` (and unset) now lands on Futurism on modern
  renderers.
- Settings migration: users with persisted `theme=auto` (or no settings file)
  get Futurism automatically on next boot; persisted `theme=classic` /
  `theme=aero` are untouched.

### 6. Ring-3 mirror (`userland/runtime`, `userland/libs/gui`, `userland/libs/dialogs`)

- `userland/runtime/src/lib.rs`: `THEME_FUTURISM = 3`,
  `THEME_AVAILABLE_FUTURISM = 1 << 2`.
- `userland/libs/gui/src/theme.rs`: mirror the kernel design —
  `enum Theme { Classic, Aero, Futurism }`, `FUTURISM_PALETTE`, and a ring-3
  `ControlStyle` table so widget painting branches on finish, not identity.
  `/etc/theme` decode becomes token equality (`b"futurism"`, `b"aero"`,
  `b"classic"`), with **Classic as the unknown-token fallback** (an old app
  binary meeting a future theme degrades safely). Theme-changed payload
  decode gains code 3 with the same fallback.
- Widgets (`Button`, `TextField`, `ListView`, `MenuBar`, `TabBar`,
  `ColumnListView`, `TimeSeriesGraph`, scrollbars) pick up the rounded flat
  finish through the shared draw helpers; spot-fix any residual binary
  branches (`theme.rs:189,400`, `file_dialog.rs:312,697`).
- Client content stays **opaque** (ring-3 surfaces are XRGB copy-blit); the
  mock agrees — only chrome and taskbar are translucent. No surface-format
  work needed.

### 7. Control Center Appearance page (`userland/apps/control/src/main.rs`)

Make the tile list table-driven:

```rust
struct ThemeTile { pref: u64, label: &'static str, availability_bit: u64 /* 0 = always */ }
const THEME_TILES: &[ThemeTile] = &[
    ThemeTile { pref: THEME_AUTO,     label: "Automatic",  availability_bit: 0 },
    ThemeTile { pref: THEME_FUTURISM, label: "Futurism",   availability_bit: THEME_AVAILABLE_FUTURISM },
    ThemeTile { pref: THEME_AERO,     label: "Aero Glass", availability_bit: THEME_AVAILABLE_AERO },
    ThemeTile { pref: THEME_CLASSIC,  label: "Classic",    availability_bit: THEME_AVAILABLE_CLASSIC },
];
```

Layout math, hit-testing, and disabling derive from `THEME_TILES.len()` and
the snapshot's `theme_available_mask`. `draw_theme_preview` becomes a match
over a per-tile preview style (Futurism preview: dark rounded title bar,
light body, accent pill). The Automatic tile's caption notes the resolved
default ("Automatic — Futurism" when the mask allows it). `apply_theme`,
`active_theme_name`, `requested_theme_name` extend to code 3.

---

## Implementation units

Ordered; each unit leaves the tree green (`cargo check`, `./test.sh`).

1. **U1 — Registry refactor, no visual change.** `ThemeKind::from_u8`,
   `ThemeSpec` + `THEMES` registry, `ControlStyle`/`ControlFinish`, port
   Classic + Aero specs and all `controls.rs`/`mod.rs` dispatch onto them;
   convert every binary theme `if` (kernel side) to style-data reads.
   Acceptance: existing theme tests pass; Classic and Aero screenshots are
   pixel-identical before/after.
2. **U2 — Futurism kernel theme.** `futurism.rs` painter, shared rounded-
   corner helper, `FUTURISM_METRICS`/palette/style/spec, `select_theme`
   default flip, system_control preference + availability bit + tokens,
   `/etc/theme` + broadcast code 3, fw_cfg token.
3. **U3 — Chrome styling.** Taskbar/tray/start-menu Futurism surfaces,
   per-theme `ChromeMetrics`, backdrop effects on chrome windows, theme-change
   relayout of the taskbar.
4. **U4 — Ring-3 toolkit.** Runtime constants, `libs/gui` theme mirror +
   `SoftRounded` widget painting, dialogs cleanup. Rebuild staged apps and
   visually verify FILEMAN/NOTEPAD/CALC/TASKMGR/CONTROL under all three
   themes.
5. **U5 — Control Center.** Table-driven Appearance tiles, Futurism preview,
   four-tile layout, "Automatic — Futurism" caption.
6. **U6 — Tests + docs.** Test additions below; update `CLAUDE.md` (project
   overview paragraph), `src/window/CLAUDE.md`, and the theme sections of
   `docs/window_system_design.md`.

## Expected file map

| File | Change |
|---|---|
| `src/window/theme/mod.rs` | `ThemeKind::Futurism`, `from_u8`, `ThemeSpec` + registry, default flip |
| `src/window/theme/futurism.rs` | **new** frame painter |
| `src/window/theme/aero.rs` | extract shared rounded-corner/shadow helpers |
| `src/window/theme/controls.rs` | `ControlStyle`/`ControlFinish`, `SoftRounded` painters, taskbar surface helpers |
| `src/window/windows/frame.rs` | spec-driven effect (mechanical) |
| `src/window/windows/taskbar.rs`, `start_menu.rs` | chrome surfaces, effects, per-theme height |
| `src/window/manager.rs` | chrome effect/height update on theme change |
| `src/system_control.rs` | `ThemePreference::Futurism`, availability bit, mappings |
| `src/userland/gui.rs` | broadcast code 3 |
| `src/userland/etc.rs` | token-driven publish (via spec) |
| `userland/runtime/src/lib.rs` | `THEME_FUTURISM`, availability bit |
| `userland/libs/gui/src/theme.rs` | `Theme::Futurism`, palette, style table, token decode |
| `userland/libs/gui/src/*.rs` widgets | finish-driven paint touch-ups |
| `userland/libs/dialogs/src/file_dialog.rs` | drop binary Aero branches |
| `userland/apps/control/src/main.rs` | table-driven Appearance page + preview |
| `src/tests/…` | theme selection/persistence/mask/publish tests |
| `CLAUDE.md`, `src/window/CLAUDE.md`, `docs/window_system_design.md` | docs |

## Verification

- **In-kernel tests** (`./test.sh`): selection matrix (Auto→Futurism on
  retained/VirGL, Auto→Classic on Legacy, Futurism-on-Legacy fallback with
  reason), `ThemeKind::from_u8` round-trips, spec lookup consistency
  (token/metrics/palette per kind), `ThemePreference` parse/serialize
  round-trip incl. `futurism`, `theme_available_mask` per renderer,
  `/etc/theme` token after switching, broadcast payload code 3, close-button
  rect within Futurism metrics.
- **Refactor gate (U1)**: before/after screenshots of Classic and Aero
  (desktop, start menu, an app window, file dialog) must be identical.
- **Manual/visual** (`./build.sh`): boot with no args → Futurism desktop;
  compare side-by-side with the mock (title bar, caption button, toolbar
  field, sidebar selection pill, taskbar pills, start menu frosting); switch
  Automatic→Classic→Aero→Futurism live from Control Center and confirm open
  apps + `/etc/theme` + `cat /etc/theme` in the terminal converge; reboot and
  confirm persistence; `AGENTICOS_NETWORK=off AGENTICOS_THEME=classic` boot
  honors the override; run once with `AGENTICOS_RENDER_STATS=1` to compare
  blur cost vs Aero.

## Risks and mitigations

- **CPU-compositor blur cost.** Futurism adds backdrop regions (taskbar +
  start menu + every title bar at radius 10 vs Aero's 6). Mitigate: measure
  with `AGENTICOS_RENDER_STATS=1`; the radius and the taskbar tint alpha are
  single spec constants and can be tuned down; blur already three-pass box
  (`scene.rs::backdrop_box_radii`) and VirGL offloads it to the host GPU.
- **Refactor regressions in Classic/Aero.** Mitigated by the U1
  pixel-identical screenshot gate before any Futurism code lands.
- **Old ring-3 binaries vs new token.** Prebuilt ELFs (`ZSH.ELF`, `BB.ELF`,
  `TCC.ELF`) don't read the GUI theme; all toolkit apps rebuild from source
  each run. The unknown-token→Classic fallback in `libs/gui` protects any
  stale binary regardless.
- **Default-flip surprise.** Users who never chose a theme silently move from
  Aero to Futurism. Intended per the request ("default going forward");
  Classic/Aero remain one Control Center click away and persisted explicit
  choices are honored.
- **Renderer fallback.** The existing runtime VirGL/retained failure path
  forces Classic and republishes; Futurism inherits it unchanged. Covered by
  a selection-matrix test.

## Out of scope and follow-ups

- **Minimize/maximize caption buttons** (shown in the mock). Requires real
  window-management semantics (minimize-to-taskbar state, restore, maximize
  layout), not theming. The `HitTestRegion::MinimizeButton`/`MaximizeButton`
  variants are already reserved; propose as its own plan, after which the
  Futurism painter grows the two frosted buttons from the mock.
- **Title-bar app icon badges** (rounded-square icon next to the title in the
  mock) — needs a per-window icon attribute end-to-end; follow-up.
- **A dedicated modern UI sans font** (the mock's Inter-like face) and a
  Futurism default gradient wallpaper — optional polish follow-ups.
- **Per-theme accent color / light-dark variants** (the mock's
  `accent=#3c8cf0 · glass=frosted` readout hints at parameterized themes).
  The `ThemeSpec` registry is the natural home for a future `accent` field;
  not in this release.
- **A `theme` CLI** (`theme --current` in the mock) — trivial once the
  snapshot syscall is stable; follow-up.

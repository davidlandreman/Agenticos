---
title: "feat: Windows-7-style Aero glass window frames with a theme system"
status: planned
created: 2026-07-17
plan_type: feat
depth: deep
related_docs:
  - src/window/CLAUDE.md
  - src/graphics/CLAUDE.md
  - docs/window_system_design.md
  - docs/plans/2026-07-17-002-feat-optional-retained-gpu-compositor-plan.md
---

# feat: Windows-7-style Aero glass window frames with a theme system

## Summary

Restyle `FrameWindow` chrome to a Windows 7 Aero look — translucent "glass"
title bar and borders, rounded corners, and a soft drop shadow — behind a new
**theme system**. The current rendering (blue active / grey inactive title
bar, square 2 px borders, no shadow) becomes the **Classic** theme and remains
the fallback: it is what you get when the theme is set to `classic`, when the
Aero theme cannot run (legacy renderer), or when anything in Aero
initialization fails.

The retained compositor (plan 2026-07-17-002) already provides every
compositing primitive Aero needs:

- Per-top-level-root **premultiplied ARGB surfaces** (`src/graphics/surface.rs`)
  — rounded corners are just transparent corner pixels; translucent glass is
  just alpha < 255 chrome pixels.
- **`CpuCompositionEngine`** does source-over blending with per-layer opacity
  (`src/graphics/composition/cpu.rs`).
- **`LayerEffect::BackdropSample { radius }`** is already reserved in
  `src/graphics/scene.rs` as "contract for a later glass pass; not currently
  rendered" — this plan implements it.
- Per-window **`CompositorProperties`** (opacity/transform/effect) are already
  copied from each root window into its scene layer in
  `WindowManager::render_retained` (`src/window/manager.rs:1056-1063`).

The legacy renderer has no alpha channel and no scene, so **Aero requires the
retained renderer**. That is the fallback rule in one sentence: *Aero + legacy
renderer → Classic, with a `debug_warn` naming the reason.*

## Goals

1. A theme abstraction that owns frame **metrics** (title-bar height, border
   width, corner radii, shadow margin) and frame **painting**, selected at
   boot.
2. An **Aero** theme: glass gradient chrome, 1 px dark outer + 1 px bright
   inner edge, rounded top corners, Win7-style red close button, title text
   with a dark halo for readability on glass, and a soft drop shadow.
3. A **Classic** theme that is pixel-for-pixel today's rendering.
4. Backdrop **blur** under the glass (real Aero, phase 2) via
   `LayerEffect::BackdropSample`.
5. Boot-time selection via fw_cfg (`opt/agenticos/theme`), same pattern as the
   compositor policy, plumbed through `build.sh` / env var.

## Non-goals

- Runtime theme switching UI (the plumbing makes it cheap later — invalidate
  all frames — but no settings surface is added now).
- Theming anything beyond frame chrome (taskbar/start menu in `guishell`,
  widget palette). The `PALETTE_*` constants in `src/window/mod.rs` stay as
  they are; a later pass can move them behind the theme.
- Hover/pressed states for the close button (no mouse-hover tracking in
  frames today).
- Rounded-corner hit testing. Hit tests stay rectangular; the few transparent
  corner pixels still hit the frame. Win7 behaved the same way.
- VirGL/GPU-accelerated blur. The CPU engine gets the reference
  implementation; the scene contract stays backend-neutral.

## Current state (what the plan builds on)

- `src/window/windows/frame.rs` — `FrameWindow` hardcodes
  `title_bar_height: 24`, `border_width: 2` and paints via three private
  methods (`draw_title_bar`, `draw_close_button`, `draw_borders`) against the
  `GraphicsDevice` trait. `content_area()` and `hit_test()` derive from the
  same fields. Active/inactive chrome uses `PALETTE_CHROME_ACTIVE` /
  `PALETTE_CHROME_INACTIVE`.
- `src/window/renderer/surface_canvas.rs` — `SurfaceCanvas` implements
  `GraphicsDevice` over a `Surface`, but writes **alpha = 255 always**
  (`PremulArgb::from_rgba(r, g, b, u8::MAX)`). No way for a widget to emit
  translucent or transparent pixels today.
- `src/window/manager.rs::render_retained` — rasterizes each top-level root
  into a surface sized exactly to its `bounds`; shadows have nowhere to live.
  Move-only damage uses `previous_bounds`.
- `src/graphics/composition/cpu.rs` — rejects
  `LayerEffect::BackdropSample` with `CompositionError::UnsupportedEffect`.
- `Layer::output_bounds()` in `scene.rs` inflates by the backdrop radius —
  wrong for a backdrop effect (blur *samples* beyond the layer, it does not
  *emit* beyond it); fixed in phase 2.
- Renderer boot default is `legacy` (`opt/agenticos/compositor` via fw_cfg).

## Design

### 1. Theme module — `src/window/theme/`

```
src/window/theme/
  mod.rs      // ThemeKind, boot selection + fallback, active-theme global
  classic.rs  // today's frame painting, moved verbatim
  aero.rs     // Win7 glass painting
```

- `ThemeKind { Classic, Aero }` stored in an `AtomicU8` (same pattern as
  `renderer::REQUEST`). `theme::active()` reads it; `theme::init_boot_policy()`
  reads fw_cfg **after** the renderer is selected and applies the fallback
  rule:

  ```
  requested = fw_cfg "opt/agenticos/theme"   // classic | aero | auto; missing → auto
  if requested == aero  && renderer == legacy  → Classic + debug_warn
  if requested == auto                          → Aero if renderer is retained, else Classic
  ```

- `FrameMetrics { title_bar_height, border_width, corner_radius_top,
  corner_radius_bottom, shadow_margin }` — a plain `Copy` struct returned by
  `theme::metrics()`:

  | | Classic | Aero |
  |---|---|---|
  | title bar | 24 | 28 |
  | border | 2 | 5 |
  | corner radius top / bottom | 0 / 0 | 8 / 4 |
  | shadow margin | 0 | 16 |

- Painting entry point: `theme::draw_frame(chrome: &FrameChrome, device: &mut
  dyn GraphicsDevice)` where `FrameChrome { bounds, title, active,
  close_button_rect }` carries everything the painters need. Enum dispatch
  (match on `ThemeKind`), no trait objects — two variants, kernel code,
  keep it boring.

`FrameWindow` changes:

- Drop the hardcoded `title_bar_height` / `border_width` fields; read
  `theme::metrics()` in `content_area()`, `hit_test()`, and `paint()`. The
  close-button geometry helper moves next to the metrics so `hit_test` and
  the painters agree on one rect.
- `paint()` becomes: build `FrameChrome`, call `theme::draw_frame`.
- `classic.rs` receives the bodies of `draw_title_bar` / `draw_close_button`
  / `draw_borders` **unchanged** — same colors, same 24/2 metrics, same
  inactive-border grey `Color::new(150, 150, 150)`. This is the fallback
  guarantee.

Audit task: grep for hardcoded `24` / `title_bar_height` / frame-size
assumptions outside `frame.rs` (e.g. `terminal_factory.rs`, `guishell`
placement code, dialogs) and route them through `theme::metrics()`.

### 2. Alpha-capable painting — `GraphicsDevice` extension

Aero needs to *write* translucency and transparency into the surface, not
blend it. Add two defaulted methods to `GraphicsDevice`
(`src/window/graphics.rs`):

```rust
/// Write an exact ARGB value (replace semantics, not source-over).
/// alpha = 0 punches a fully transparent hole (used by rounded corners).
fn fill_rect_argb(&mut self, x: i32, y: i32, w: u32, h: u32,
                  color: Color, alpha: u8) { /* default: see below */ }
fn draw_pixel_argb(&mut self, x: i32, y: i32, color: Color, alpha: u8) { … }
```

- `SurfaceCanvas` overrides both: `PremulArgb::from_rgba(r, g, b, alpha)`
  written straight into the surface. This is the only path Aero actually
  uses (the theme fallback rule guarantees Aero never paints on a legacy
  adapter).
- Default impl for legacy adapters: blend `color` against `read_pixel` at
  `alpha` (visual approximation, ignores alpha = 0 punch-out). Exists only so
  the trait stays object-safe and total; documented as approximation.

### 3. Decoration insets — room for the shadow

Shadows and (conceptually) the anti-aliased corner fringe live **outside**
`bounds`. Rather than a separate shadow layer per window (more scene churn,
more damage bookkeeping), each root's surface is inflated:

- New defaulted `Window` method: `fn decoration_insets(&self) -> Insets`
  (zero default). `FrameWindow` returns
  `Insets::uniform(theme::metrics().shadow_margin)` — zero under Classic, so
  the retained pipeline is bit-identical for Classic.
- `render_retained` computes `decorated_bounds = bounds + insets` per root
  and uses it for `ensure_surface`, the layer `destination_rect`, the
  `SurfaceCanvas` origin, repaint-region clipping, and move-only damage
  (`previous_bounds` stores decorated bounds). Because `SurfaceCanvas` maps
  absolute screen coordinates, the Aero painter simply draws the shadow at
  `bounds.x - margin …` and it lands in the margin naturally.
- Hit testing, focus, drag, and z-order all keep using undecorated `bounds` —
  the shadow is click-through by construction.
- Surface budget impact: the default 800×600 frame grows to 832×632 ≈ +3.5%
  bytes. Negligible against the 48 MiB budget.

### 4. Aero painting (`aero.rs`) — phase 1, no blur yet

Draw order into the (inflated) surface:

1. **Shadow** — precomputed 1D falloff ramp (quadratic ease-out over
   `shadow_margin` px, peak alpha ≈ 96 when active / 56 when inactive),
   black. Edges use the ramp; corners use `ramp[dx] * ramp[dy] / 255`
   (radial product). Pure `fill`/`draw_pixel_argb` writes, ~small const
   table, no allocation.
2. **Glass chrome** — title bar + borders as one region:
   - Vertical gradient, Win7-ish: active = cool blue-white, top rows
     lighter (`argb(α≈180, 220, 235, 250)` → `argb(α≈150, 160, 190, 220)`);
     inactive = desaturated grey (`argb(α≈190, 200, 205, 210)` →
     `argb(α≈170, 170, 175, 180)`). Exact stops tuned on screenshots.
   - 1 px outer edge `argb(120, 0, 0, 0)`, 1 px inner edge
     `argb(90, 255, 255, 255)` — the classic glass rim.
3. **Rounded corners** — punch-out pass over the four corner squares:
   distance from the corner circle center decides `outside → alpha 0`,
   `within 1.5 px of the arc → scale existing pixel's alpha by coverage`
   (cheap AA via distance, no supersampling), `inside → untouched`. Runs
   after chrome so it clips both shadow-adjacent rim and gradient.
4. **Close button** — rounded-rect red pill (gradient
   `(232, 17, 35)` → `(140, 10, 20)`, radius 3, 1 px darker border), white ×
   from the existing two-diagonals code. Geometry from the shared
   close-button helper (also used by `hit_test`).
5. **Title text** — existing `draw_text`, white, with a 1 px offset dark
   shadow pass (`argb(140, 0, 0, 0)` at +1,+1) drawn first so text stays
   readable over bright backdrops — the budget version of Win7's text glow.

The content window paints over the client area exactly as today (children
are rasterized into the same root surface by `render_layer_tree_in_region`),
so glass only ever shows where chrome is.

### 5. Backdrop blur — phase 2, real glass

Implement `LayerEffect::BackdropSample { radius }` in
`CpuCompositionEngine::compose`:

1. When the z-ordered walk reaches a layer with `BackdropSample`, the output
   already holds everything beneath it. Copy the backdrop under
   `draw ∪ (chrome region ± radius)` into a scratch buffer, run a
   **3-pass sliding-window box blur** (O(n) per pass, integer, ≈ Gaussian),
   write it back, then source-over the layer as usual. Translucent chrome
   pixels now blend against blurred backdrop; the opaque client area fully
   covers its (wastefully) blurred pixels — acceptable at these sizes
   (≈ 40–60k chrome pixels for the default frame; three passes of a
   sliding-window blur is a few hundred µs of CPU, and only on damaged
   frames).
2. **Damage correctness**: a glass layer turns backdrop changes into visual
   changes up to `radius` px away. In `render_retained`, expand each damage
   rect by `radius` where it intersects a glass layer before composing, so
   blur inputs inside the pass are current. Fix
   `Layer::output_bounds()` to stop inflating by the radius (backdrop
   sampling reads beyond the layer; it does not emit beyond it) — the reserved
   semantics were never exercised, so this is safe to correct.
3. Wire-up: under Aero, `FrameWindow` sets
   `CompositorProperties { effect: BackdropSample { radius: 4 }, .. }` on its
   base; `render_retained` already copies it onto the scene layer.
4. Keep the engine's `UnsupportedEffect` error for any future engine that
   genuinely cannot do it; the CPU reference now supports it.

Phase 2 is separable: phase 1 ships crisp-translucent glass (Win7 "Aero
basic-plus" look); phase 2 upgrades it to blurred glass without touching the
theme code — only the engine and one `CompositorProperties` line.

### 6. Boot plumbing

- `build.sh` / `test.sh`: `AGENTICOS_THEME` env var (default empty → `auto`)
  → QEMU `-fw_cfg name=opt/agenticos/theme,string=…`, alongside the existing
  compositor policy.
- `src/window/theme/mod.rs::init_boot_policy()` called from window-system
  init after `select_renderer` resolves, since the fallback rule needs the
  selected renderer kind.
- Renderer default stays `legacy` for now; seeing Aero requires
  `AGENTICOS_COMPOSITOR=retained` (or whatever the existing build.sh knob
  is). Flipping the default renderer is a separate decision once retained
  has more soak time — noted as follow-up, not part of this plan.

## Implementation phases

**Phase A — theme skeleton + Classic extraction (no visual change)**
1. `src/window/theme/{mod,classic}.rs`; `FrameMetrics`; boot policy +
   fallback rule; `build.sh`/`test.sh` plumbing.
2. `FrameWindow` reads metrics from theme, paints via `theme::draw_frame`;
   Classic paints exactly today's chrome. Audit hardcoded metric users.
3. Tests: selection/fallback matrix; Classic key-pixel regression (titlebar
   color at active/inactive, border color, close-button rect) against a
   rasterized `Surface`.

**Phase B — alpha writes + insets (still no visual change under Classic)**
4. `fill_rect_argb`/`draw_pixel_argb` + `SurfaceCanvas` overrides.
5. `Insets`, `Window::decoration_insets`, `render_retained` decorated-bounds
   switch (surface size, destination rect, canvas origin, move damage).
6. Tests: surface dimensions = bounds + insets; move-only damage covers old
   and new decorated bounds; zero-inset path byte-identical.

**Phase C — Aero phase 1 (translucent glass, corners, shadow)**
7. `aero.rs` painter (shadow ramp, gradient chrome, rim, corner punch-out,
   close button, title shadow).
8. Tests on a rasterized frame surface: corner pixel alpha = 0 outside the
   arc; chrome pixel alpha within expected translucent band; shadow-margin
   alpha monotonically decreasing outward; client-area pixels untouched by
   chrome. Manual: boot retained + aero, screenshot via the agenticos MCP
   tools, tune gradient stops.

**Phase D — Aero phase 2 (backdrop blur)**
9. Box blur in `CpuCompositionEngine`; damage expansion; `output_bounds`
   fix; `FrameWindow` sets `BackdropSample { radius: 4 }` under Aero.
10. Tests: uniform backdrop → blur is identity; single-impulse backdrop →
    energy spreads within radius and sums preserved; damage rect adjacent to
    glass layer produces correct pixels (the stale-neighborhood case);
    `UnsupportedEffect` no longer returned by the CPU engine.

## Risks / open questions

- **Aero is invisible on default boots** while `legacy` remains the default
  renderer. Mitigation: `auto` theme means the moment the retained renderer
  becomes default, Aero lights up with no further change. Consider making
  `retained` the default in a follow-up.
- **Focus-change repaint cost**: active↔inactive already invalidates the
  frame; Aero adds shadow-margin pixels and (phase 2) a blur pass to that
  repaint. Bounded by chrome area, not window area — acceptable, but the
  render-stats line (`render_stats renderer=retained …`) should be checked
  before/after on the default desktop.
- **Blur vs. damage subtleties** (phase 2, item 2) are the riskiest
  correctness spot; the impulse/adjacency tests exist precisely for it.
- **Win7 look is gradient-tuning-heavy**: expect a screenshot-iterate loop on
  the exact stops; the plan fixes structure, not final constants.

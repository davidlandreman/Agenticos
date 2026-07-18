---
title: "feat: dark translucent terminal content well on modern renderers"
type: feat
status: completed
date: 2026-07-18
---

# Dark translucent terminal content well on modern renderers

## Implementation outcome

Completed on 2026-07-18. `ThemeSpec` now owns a terminal-well material:
Classic uses opaque `#202020`, while Aero and Futurism use the same tint at
alpha 232 over their existing radius-6 frame backdrop effect. `TextWindow`
uses one material fill helper for full and incremental paints, retaining the
legacy renderer's opaque bulk-fill fast path.

The Screen-to-TextWindow sync now carries whether a background came from
`ColorSpec::Default`, so only the default well is translucent. Explicit ANSI
black/indexed/RGB backgrounds remain opaque, as do glyphs and the caret.
Theme changes and compositor fallback reuse the existing full invalidation and
Classic activation paths.

Regression coverage pins the per-theme material/effect contract, full and
incremental alpha replacement, explicit-background opacity, crisp glyphs, and
CPU backdrop sampling. `cargo check`, the 26-test focused theme/composition
suite, a retained-CPU/Futurism boot with all 12 theme tests, and the complete
910-test kernel suite pass. The dedicated hardware VirGL runner was not run in
this workspace because no qualified `AGENTICOS_QEMU_BIN` was configured; the
declared radius remains covered by the existing GPU-support invariant.

## Summary

Make the terminal's default content well a dark, slightly translucent frosted
surface when AgenticOS is using a modern theme on a renderer with backdrop
composition (retained CPU or qualified VirGL). The default starting material
is the existing terminal grey, `#202020`, at alpha `232` (about 91% opaque),
over the theme's existing radius-6 backdrop blur.

Classic and the Legacy renderer remain fully opaque. Explicit ANSI background
colors also remain opaque, so TUIs and commands that deliberately paint a cell
background preserve their intended colors. Text and the caret remain opaque
for contrast.

This feature does not add another compositor layer or an independent blur
effect. A top-level frame and its content already rasterize into one retained
surface, and Aero/Futurism frame layers already carry
`BackdropSample { radius: 6 }`. Painting the terminal's default well with
fractional alpha lets that layer's existing coverage scan and blur pipeline
include the content area alongside the title bar. The covered area and blur
work grow when the well is damaged, which the validation matrix measures.

## Current state

- `TextWindow::paint` fills the whole terminal and every incremental dirty-cell
  range with opaque `Color::new(32, 32, 32)`.
- A `FrameWindow` and all of its children rasterize into one canonical retained
  surface. Only the frame root contributes the scene layer's compositor
  properties.
- Aero and Futurism frame roots already request radius-6 backdrop sampling.
  Classic requests no effect, and Legacy selects Classic as its fallback.
- `SurfaceCanvas::fill_rect_argb` uses exact ARGB replacement, while legacy
  graphics devices safely approximate alpha by blending into their RGB target.
- Backdrop coverage is derived from fractional-alpha pixels. Once the terminal
  well is fractional, no new region plumbing is needed.
- `terminal::screen::Cell` retains `ColorSpec::Default` versus explicit ANSI
  colors, but `TerminalWindow::sync_text_window_from_screen` currently resolves
  both to RGB before passing them to `TextWindow`. `TextWindow` consequently
  treats any RGB black background as though it were the default background.

## Product decisions

### PD1 — translucency follows the resolved theme/renderer policy

Add a terminal-well material to `ThemeSpec`:

```rust
pub struct TerminalWellMaterial {
    pub tint: Color,
    pub alpha: u8,
}
```

The initial table is:

| Theme | Tint | Alpha | Backdrop |
|---|---:|---:|---|
| Classic | `#202020` | 255 | none |
| Aero | `#202020` | 232 | existing radius 6 |
| Futurism | `#202020` | 232 | existing radius 6 |

This uses the same capability gate already enforced by theme selection: Aero
and Futurism cannot remain active on Legacy, and a runtime compositor fallback
activates Classic before repainting. It also keeps explicit Classic visually
faithful even when Classic is selected on VirGL.

Do not query the global window manager from `TextWindow::paint`; painting runs
inside the manager's render transaction. The active, fallback-resolved theme is
the safe local source of truth.

### PD2 — reuse the frame layer's backdrop effect

Do not split terminal content into a child scene layer and do not introduce a
terminal-specific effect radius. The existing top-level frame layer already
contains both the title bar and content well. Its fractional-alpha coverage
metadata naturally restricts backdrop work to translucent pixels.

Keep the terminal radius at the theme's existing value rather than declaring a
second constant. This preserves the qualified VirGL limit and avoids stacked
blur halos or a second full-screen scratch sequence.

### PD3 — only the default terminal background is glass

Preserve the distinction between `ColorSpec::Default` and an explicit
background through the Screen-to-TextWindow seam:

- default background: draw the theme's terminal-well ARGB material;
- `Indexed(0)`, any other indexed color, and `Rgb(...)`: draw an opaque cell;
- glyphs and the caret: continue drawing opaque;
- padding and unused pixels: use the same default well material.

This avoids making `\x1b[40m`, full-screen TUIs, or color test output
unexpectedly transparent. Add a `background_is_default` bit to `CharCell` (or
an equivalent small representation) instead of inferring semantics from RGB.

### PD4 — full and incremental paint paths must be identical

Centralize default-well painting in one helper used by:

- the initial/full terminal bounds fill;
- incremental dirty-cell bounding fills;
- default-background cells if a caller needs to repaint them explicitly.

The helper uses `fill_rect_argb` with the active material. Explicit cell
backgrounds continue to use opaque `fill_rect`. This prevents typing from
leaving opaque rectangles in an otherwise translucent terminal.

### PD5 — keep readability and performance measurable

Start at alpha 232 rather than a more transparent glass value. It keeps the
terminal dark over bright wallpapers while making wallpaper shape and color
visible after blur. Tune only from side-by-side screenshots on a high-contrast
wallpaper.

Use existing `AGENTICOS_RENDER_STATS=1` telemetry to compare idle, typing,
scrolling, and dragging. Idle must remain damage-free; typing should blur only
the published dirty range plus its radius halo; a terminal scroll may repaint
and blur the full content well, which is expected but must remain interactive.

## Implementation units

### U1 — add the theme material contract

- Add `TerminalWellMaterial` and a `terminal_well` field to `ThemeSpec` in
  `src/window/theme/mod.rs`.
- Define Classic as opaque and Aero/Futurism as `#202020` at alpha 232.
- Add `terminal_well_for(ThemeKind)` and `terminal_well()` accessors.
- Extend the theme invariant test: every translucent terminal material must
  belong to a theme whose frame effect is `BackdropSample`, and that radius
  must pass `gpu_backdrop_radius_supported`.

Acceptance: the material table is data-driven, Classic is opaque, and no
translucent theme can silently omit or exceed the qualified blur effect.

### U2 — preserve default-background semantics

- Extend `CharCell` in `src/window/windows/text.rs` with whether its background
  is terminal-default.
- Update the Screen sync seam in `src/window/windows/terminal.rs` to pass
  `matches!(cell.bg, ColorSpec::Default)` alongside the resolved color.
- Keep newly allocated/cleared cells default-backed. Preserve explicit ANSI
  black as explicit and opaque.
- Update the `src/terminal/CLAUDE.md` data-flow description for the enriched
  `set_cell` contract.

Acceptance: default and explicit black can resolve to the same RGB while still
painting with different alpha.

### U3 — paint the glass well correctly

- Add a small `TextWindow` helper that fills a rectangle with the active
  terminal material using `fill_rect_argb`.
- Replace both hardcoded opaque `#202020` default fills (full and incremental)
  with the helper.
- Paint only non-default cell backgrounds as opaque cell rectangles.
- Leave glyph and caret drawing unchanged.
- Confirm runtime theme changes require no cached material: the current
  `apply_theme_request` invalidates every window and forces a full repaint, so
  the helper reads the newly active material on that repaint.

Acceptance: no opaque dirty-cell patches appear after typing, erasing,
scrolling, resizing, moving another window over the terminal, or switching
themes.

### U4 — regression coverage and documentation

- Add surface-raster tests in `src/tests/window_theme.rs` (or a focused terminal
  paint test module) covering:
  - Classic full-well alpha 255;
  - Futurism full-well and padding alpha 232;
  - an incremental blank/default-cell repaint staying alpha 232;
  - explicit indexed black and RGB backgrounds staying alpha 255;
  - glyph/caret pixels remaining opaque.
- Add an integration assertion that the translucent material plus the existing
  frame effect samples a changing backdrop; rely on the existing CPU/VirGL
  blur suites for the blur algorithm itself.
- Update `src/window/CLAUDE.md`, `src/terminal/CLAUDE.md`, and the root current
  state to describe the terminal well and its opaque fallback.

Acceptance: tests cover the semantic seam and both paint paths without
duplicating compositor algorithm tests.

## Validation

Automated:

```sh
cargo fmt --check
cargo check
./test.sh --skip-userland window_theme composition_cpu retained_scene
AGENTICOS_QEMU_BIN="$QEMU_VIRGL_PREFIX/bin/qemu-system-x86_64" \
  AGENTICOS_QEMU_GL=es ./scripts/test-virgl-integration.sh
```

Then run the full suite with `./test.sh`.

Manual matrix:

1. Qualified VirGL + Futurism with a high-contrast wallpaper: verify a dark,
   subtly blurred well, crisp text, and no sharp wallpaper detail through it.
2. Retained CPU + Futurism: verify pixel-equivalent material behavior and
   acceptable typing/scrolling responsiveness.
3. Legacy/Classic: verify the terminal remains opaque `#202020` with no visual
   regression.
4. Live-switch Futurism -> Classic -> Aero: verify the open terminal changes
   material immediately without stale blocks or a one-frame opaque well.
5. Run ANSI background samples, including explicit black, indexed colors, and
   24-bit RGB; verify those cell rectangles stay opaque.
6. With `AGENTICOS_RENDER_STATS=1`, compare idle, typing, scrolling, resizing,
   and dragging. Confirm no unsupported-effect error or renderer fallback in a
   strict VirGL boot.

## Out of scope

- User-configurable terminal opacity, tint, or blur radius.
- Per-window opacity controls or a terminal settings UI.
- Making arbitrary ring-3 GUI content wells translucent.
- A second scene layer for child windows or multiple effects per layer.
- Changing the terminal palette, font, padding, glyph attributes, or caret
  style.

## Risks and mitigations

- **Full-well blur during scroll is expensive on retained CPU.** Measure with
  existing telemetry; keep the radius at 6 and alpha high. If measured
  interaction is unacceptable, restrict the translucent material to the
  accelerated renderer in a follow-up rather than weakening compositor
  correctness.
- **Incremental paints can accidentally overwrite alpha.** One shared material
  fill helper and a dedicated incremental-alpha regression test close this
  path.
- **Explicit ANSI black can be mistaken for default black.** Carry the
  `ColorSpec::Default` distinction explicitly instead of comparing RGB.
- **Runtime fallback can leave a glass surface on Legacy.** The existing
  fallback activates Classic and forces repaint; cover the opaque Classic
  material and live theme switch in tests/manual validation.
- **Large fractional coverage increases coverage work.** The terminal shares
  the already-effectful frame surface, so no additional surface or scan is
  introduced; telemetry will reveal whether the larger covered region needs a
  later static-coverage optimization.

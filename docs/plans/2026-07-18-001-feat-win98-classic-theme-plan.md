# Windows 98 Classic Theme

Update the `classic` window-frame theme to be a faithful reproduction of Windows 98
window chrome: the horizontal gradient title bar, the raised 3D beveled border that
gives the window its depth shadow, and Win98-accurate dimensions, colors, and caption
font size.

## Background / current state

- `src/window/theme/classic.rs` paints a flat 2px solid border (blue when active,
  grey when inactive), a 24px flat blue title bar, white 14px title text, and a
  solid red 16×16 close box.
- `CLASSIC_METRICS` in `src/window/theme/mod.rs`: `title_bar_height: 24`,
  `border_width: 2`, `shadow_margin: 0`.
- `theme::close_button_rect(bounds, metrics)` is shared by painting
  (`FrameWindow::paint`) and hit-testing (`manager.rs:1776`), so button geometry
  changes propagate to both automatically.
- `FrameWindow::content_area` and the resize-border hit test both derive from
  `FrameMetrics`, so metric changes flow through with no per-call-site edits.
- The system font is a monospaced TTF parsed once at 14px
  (`src/graphics/fonts/core_font.rs`, `SYSTEM_FONT_PX = 14`). `TtfFont::from_data`
  accepts an arbitrary pixel size, so a second, smaller instantiation is cheap.
- Key-pixel regression tests live in `src/tests/window_theme.rs`
  (`./test.sh window_theme`).

## Windows 98 reference spec ("Windows Standard" scheme)

A note on the "depth shadow": Win98 windows have no soft drop shadow. The depth
effect comes from the two-pixel **raised bevel** around the window — light edges on
the top/left, dark shadow edges on the bottom/right (black outermost, grey inner).
That bevel is what we reproduce exactly. (A soft blurred shadow like Aero's would
be an anachronism; see Open questions.)

### Colors

| Role | Win98 default | RGB |
|---|---|---|
| Active caption gradient, left | Navy | `0, 0, 128` (#000080) |
| Active caption gradient, right | Medium blue | `16, 132, 208` (#1084D0) |
| Inactive caption gradient, left | Grey | `128, 128, 128` (#808080) |
| Inactive caption gradient, right | Light grey | `181, 181, 181` (#B5B5B5) |
| Active caption text | White | `255, 255, 255` |
| Inactive caption text | Silver | `192, 192, 192` (#C0C0C0) |
| 3D face (border fill, buttons) | ButtonFace | `192, 192, 192` (#C0C0C0) |
| 3D light (outer top/left bevel) | ButtonLight | `223, 223, 223` (#DFDFDF) |
| 3D highlight (inner top/left bevel) | White | `255, 255, 255` |
| 3D shadow (inner bottom/right bevel) | ButtonShadow | `128, 128, 128` (#808080) |
| 3D dark shadow (outer bottom/right bevel) | Black | `0, 0, 0` |
| Button glyph (the ✕) | Black | `0, 0, 0` |

The border bevel does NOT change with focus in Win98 — only the caption gradient
and caption text do. This is a deliberate behavior change from today's classic
theme (which recolors the whole border on focus).

### Geometry

| Metric | Win98 value |
|---|---|
| Sizing border (frame) width | 4 px (2 px bevel + 2 px ButtonFace fill) |
| Title bar (caption) height | 18 px (SM_CYCAPTION 19 = 18 + 1 separator) |
| Caption button size | 16 × 14 px |
| Button margins inside caption | 2 px from caption top, 2 px from caption right edge |
| Caption text inset | 2 px from caption left edge, vertically centered, **bold** |
| Caption font | 8 pt MS Sans Serif bold ≈ 11 px |
| Corner radius / drop shadow | none (`shadow_margin` stays 0) |

Border cross-section, outermost ring inward:

1. top/left `#DFDFDF`, bottom/right `#000000`
2. top/left `#FFFFFF`, bottom/right `#808080`
3. + 4. `#C0C0C0` fill on all four sides

Caption occupies `(x+4, y+4)` to `(right-4, y+4+18)`; client content starts at
`y + 22` (unchanged formula in `FrameWindow::content_area`, new numbers via
metrics).

### Close button

Raised ButtonFace push button: fill `#C0C0C0`; bevel 1px `#FFFFFF` top/left,
1px `#000000` bottom/right, then 1px `#DFDFDF` top/left, 1px `#808080`
bottom/right (standard button edge — note the button bevel order differs from the
window edge: highlight outermost, light inner). Glyph: black ✕ rendered as a small
hardcoded bitmap (Marlett-style, ~7×7, 2px-thick diagonal strokes) centered in the
button — not `draw_line` diagonals, so it is pixel-stable.

## Implementation

### 1. `src/window/theme/mod.rs` — metrics

- `CLASSIC_METRICS` → `title_bar_height: 18`, `border_width: 4`,
  `shadow_margin: 0` (unchanged), radii 0 (unchanged).
- Extend `FrameMetrics` with `button_width`, `button_height`,
  `button_right_margin` so `close_button_rect` stays data-driven for both themes:
  - Classic: `16 / 14 / 2` → rect at
    `x = right − border − 2 − 16`, `y = y + border + (18 − 14)/2` (= border + 2).
  - Aero: `16 / 16 / 4` — chosen so the computed rect is bit-identical to today's
    output (`x = right − border − 4 − 16`, `y = y + border + (28 − 16)/2`); the
    Aero tests must keep passing untouched.
- Rewrite `close_button_rect` to use the new fields (drop the local `SIZE` /
  `PADDING` consts). `manager.rs` hit-testing needs no change.

### 2. `src/graphics/fonts/core_font.rs` — caption font

- Add `CAPTION_FONT_PX: u16 = 11` and a second `Once<FontRef>` slot,
  `CAPTION_FONT`, parsed from the same `SYSTEM_TTF_DATA` inside `init_fonts`
  (same `Box::leak` pattern, same fall-through to the 8×8 embedded font before
  init / on parse failure).
- Add `get_caption_font() -> FontRef`.
- Bold is synthesized in the theme by double-striking (draw at `x` and `x+1`),
  the same trick classic GDI used for synthetic bold. Our TTF is monospaced
  rather than proportional MS Sans Serif — that is the one place we knowingly
  approximate; see Open questions for the exact-font option.

### 3. `src/window/theme/classic.rs` — full repaint rewrite

Define a private `mod colors` with named consts from the table above (stop using
`PALETTE_CHROME_ACTIVE` / `PALETTE_CHROME_INACTIVE`; those remain for widgets).
Then paint, in order:

1. **Bevel frame** — four `draw_line`/`fill_rect` rings per the cross-section
   above. Draw top/left edges first, then bottom/right edges over the full side
   length so the dark edge owns the top-right and bottom-left corner pixels
   (matching GDI `DrawEdge`). Fill remaining 2px ring with ButtonFace. Identical
   for active and inactive.
2. **Caption gradient** — for each column `i` in `0..caption_width`, lerp
   left→right color and `fill_rect(x+4+i, y+4, 1, 18)`. Reuse the
   `lerp_u8`/`lerp_color` helpers currently private to `aero.rs` by hoisting them
   into `theme/mod.rs` (pub(super)).
3. **Close button** — raised bevel + black ✕ bitmap per the spec above, drawn at
   `chrome.close_button_rect`.
4. **Caption text** — `get_caption_font()`, white (active) / silver (inactive),
   double-struck for bold, baseline centered:
   `text_y = y + border + (18 − line_height)/2`, `text_x = x + border + 2`.
   Clip: skip drawing into the button area — elide the text region at
   `close_button_rect.x − 2` via `set_clip_rect` around the text draw (restore to
   `None`/previous after).

Delete nothing else; `draw` keeps its signature so `theme::draw_frame_for`
dispatch is unchanged.

### 4. Tests — `src/tests/window_theme.rs`

- `test_metrics_and_decoration_geometry`: title 24 → 18, border 2 → 4; add
  asserts for the new button-metric fields (classic 16×14/2, aero 16×16/4) and
  that classic `close_button_rect` for an 80×50 window is exactly
  `Rect::new(80−4−2−16, 4+2, 16, 14)`.
- `test_classic_key_pixels_regression`: rewrite the expected pixels —
  - `(0,0)` = `#DFDFDF`, `(1,1)` = `#FFFFFF`, `(2,2)` = `#C0C0C0`,
    `(W−1,H−1)` = `#000000`, `(W−2,H−2)` = `#808080` — same values for active
    AND inactive (borders no longer follow focus);
  - caption left column `(4,10)` ≈ `#000080` and right end of the gradient near
    `#1084D0` (assert channel ranges, e.g. red==16±2, to avoid over-fitting the
    lerp rounding); inactive: `#808080` → `#B5B5B5`;
  - a button-face pixel `#C0C0C0` and one ✕ glyph pixel `#000000`
    (replaces the red-button assert).
- Aero tests must pass unmodified — they are the guard that the
  `FrameMetrics`/`close_button_rect` refactor didn't move Aero geometry.

### 5. Docs

- Update `src/window/CLAUDE.md` theme bullet ("classic preserves the historical
  chrome" → describes the Win98 chrome) and the classic.rs doc comment (it claims
  "kept pixel-for-pixel for the fallback path" — no longer true).

## Verification

1. `cargo check`, `cargo clippy`, `cargo fmt`.
2. `./test.sh window_theme` — exit 33.
3. `./test.sh` full run — the theme is painted in every GUI boot, so unrelated
   window/manager tests double as smoke coverage.
4. Visual check in QEMU, both renderer paths (classic must render identically on
   both, since it's the legacy-renderer fallback):
   - `AGENTICOS_THEME=classic AGENTICOS_COMPOSITOR=retained ./build.sh`
   - `AGENTICOS_THEME=classic AGENTICOS_COMPOSITOR=legacy ./build.sh` (also the
     default when `AGENTICOS_COMPOSITOR` is unset)
   Check: gradient direction, bevel corner pixels, focus change (click desktop →
   caption goes grey gradient, border unchanged), close button look and click
   target, dragging by title bar, resizing on the now-4px border, terminal
   content area alignment (no gap/overlap at the new `y+22` content top).

## Open questions / follow-ups (not blocking)

1. **Min/max buttons + system icon.** A close-only caption is itself authentic
   Win98 (it is exactly the dialog-window chrome), so this plan ships close-only.
   Rendering inert minimize/maximize buttons would look more like an app window
   but adds fake affordances the WM can't honor yet; recommend deferring until
   minimize/maximize exist.
2. **Exact caption font.** For true pixel parity with 8pt MS Sans Serif bold we'd
   need to embed a free proportional bitmap font (MS Sans Serif itself is not
   redistributable). Follow-up if the 11px monospace + synthetic bold isn't
   close enough on screen.
3. **Optional soft drop shadow.** If a modern-style blurred shadow is wanted
   around classic windows despite being non-authentic, it's a small follow-up:
   set `shadow_margin` on `CLASSIC_METRICS` and port Aero's `draw_shadow` — but
   only for the retained renderer (legacy has no alpha compositing), which would
   break "classic renders identically on both paths." Recommend not doing it.
4. **Rest of the desktop.** Widgets (`PALETTE_*` in `window/mod.rs`), the
   guishell taskbar/Start menu, and the desktop color (`#008080` teal) are out of
   scope here; a follow-up "Win98 desktop" pass could retheme those with the same
   color table.

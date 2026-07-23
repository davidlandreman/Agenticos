---
title: "feat: Task Manager theme parity and reusable tab/chart surfaces"
type: feat
status: implemented
date: 2026-07-23
depth: medium
related_docs:
  - docs/plans/2026-07-18-003-feat-theme-aware-controls-plan.md
  - docs/plans/2026-07-18-003-feat-ring3-task-manager-and-procfs-plan.md
  - docs/plans/2026-07-18-007-feat-userland-gui-control-maturity-and-scrollable-text-area-plan.md
  - docs/plans/2026-07-18-007-feat-futurism-theme-plan.md
  - userland/README.md
---

# feat: Task Manager theme parity and reusable tab/chart surfaces

## Summary

Bring `TASKMGR.ELF` into full live parity with Classic, Aero, and Futurism by
finishing the theme support of the shared controls it already consumes.

This is not a private Task Manager tab implementation. `TabBar` already lives
in `userland/libs/gui`, is re-exported from the toolkit root, and is available
to every ring-3 application. The defect is that its painter is a single
hard-coded flat construction: the selected tab always uses
`Palette::field_bg`, which is white in all three themes. The same audit found
similar incomplete migrations in `TimeSeriesGraph`, `ColumnListView`, and
Task Manager's hand-painted client/status surfaces.

The change therefore has two layers:

1. complete the reusable toolkit components and theme painters;
2. simplify Task Manager so it composes those controls and uses theme tokens
   instead of legacy fixed colors.

No kernel GUI component, GUI syscall, `/proc` format, sampler behavior, or
window-frame work is required.

## Current state and findings

### Tabs are already shared, but their finish is not themed

`userland/libs/gui/src/lib.rs::TabBar` owns labels, active index, hit testing,
typed pointer input, and Left/Right/Home/End navigation. Task Manager is its
only production consumer today, but the type is public and documented in
`userland/README.md`.

`TabBar::draw` reads `theme::palette()`, then constructs every theme the same
way:

- the strip uses `content_bg`;
- the selected tab uses `field_bg`;
- a two-pixel `selection_bg` rule is drawn across its top;
- inactive labels use `disabled_text`.

Every theme deliberately has a white `field_bg`, because fields and list
wells are white. That token is wrong for a selected tab in Classic and
explains the reported always-white result. Inactive tabs are also enabled
navigation choices, not disabled controls, so grey disabled text gives the
wrong state cue.

The theme layer already has the correct architectural precedent:
`draw_button`, `draw_field`, `draw_selection`, and `draw_menu_surface`
dispatch on `Finish::{Bevel98, GlassKd4, SoftRounded}`. Tabs need the same
surface-owner split: `TabBar` keeps geometry/text/input; `theme` paints the
strip and individual tab faces.

### Task Manager still paints an Aero-like fixed shell

`TaskMgr::render` imports the toolkit's legacy `COLOR_*` constants and:

- clears the entire client surface to `COLOR_WHITE`;
- draws secondary text with fixed `COLOR_TEXT_DIM`;
- draws performance/stat text with fixed `COLOR_TEXT`;
- draws the status strip with fixed `COLOR_PANEL`, `COLOR_BORDER`, and
  `COLOR_TEXT`;
- duplicates graph legend colors with fixed `COLOR_HIGHLIGHT` and
  `COLOR_ACCENT2`.

This makes the page behind the shared controls white in Classic instead of
ButtonFace `#C0C0C0`, and prevents Futurism's content colors from reaching the
footer and labels. Live theme notification is already correct:
`TaskMgr::route` applies the process-global theme event, refreshes an open
modal, marks the main window dirty, and repaints without restart.

### A shared status bar exists but Task Manager bypasses it

`userland/libs/gui::StatusBar` already owns themed background, divider, text,
and vertical alignment. Task Manager hand-computes the same 22-pixel strip.
Adopting the shared control removes one app-local theme surface and gives
future status-bar painter improvements to Task Manager automatically.

### The monitoring widgets are only partially theme-aware

`TimeSeriesGraph` is a shared toolkit component, but all of its visual tokens
are fixed: white surface, `#EAEAEA` grid, legacy border/text, blue line/fill,
and green secondary line. The Futurism plan named it as a theme-aware widget,
but the implementation never completed that conversion.

`ColumnListView` uses the active palette for its field, header, dividers,
text, and selection colors. It still fills selected rows directly instead of
calling `theme::draw_selection`, so Aero's selection outline and Futurism's
rounded selection finish are lost. Its header is also one flat
`content_bg` band in every theme.

`Button` and the interactive `Scrollbar` already use finish-dispatched theme
painters and need no Task Manager-specific work.

### The reference gallery does not exercise these controls

`GUIDEMO.ELF` was intended to show `TabBar` and the monitoring widgets, but
currently renders neither `TabBar`, `ColumnListView`, `TimeSeriesGraph`, nor
`StatusBar`. That allowed the incomplete Classic/Futurism paths to remain
unnoticed.

## Product and architecture decisions

### D1 — `TabBar` remains a shared ring-3 GUI toolkit component

Keep the visual widget in `userland/libs/gui`; optionally move its
implementation from the monolithic `lib.rs` to `tab_bar.rs`, with the same
root re-export. Do not add:

- a Task Manager-local tab renderer;
- a second kernel `src/window/windows/TabBar`;
- a GUI ABI for tabs;
- pixel painting to `gui-core`.

`gui-core` is intentionally the runtime-free home for geometry, input, scroll,
focus, and text-edit models. Pixel controls belong in `gui`. A separate pure
tab-state model is unnecessary for the current active-index behavior.

### D2 — Theme helpers own tab surface construction

Add theme-level painters along these lines:

```rust
pub fn draw_tab_strip(canvas: &mut Canvas, bounds: Rect);
pub fn draw_tab(canvas: &mut Canvas, bounds: Rect, selected: bool);
pub fn tab_text(selected: bool) -> u32;
```

Names may follow the existing theme module's conventions, but the ownership
boundary is fixed: the widget computes tab widths and label positions; the
theme determines faces, borders, corner treatment, selected-page merge, and
text color.

Target constructions:

| Finish | Strip and inactive tabs | Selected tab |
|---|---|---|
| Classic / `Bevel98` | ButtonFace strip with a dark baseline; normal black text, not disabled text | ButtonFace raised tab with Classic highlight/light top-left and shadow/dark right edge; omit the bottom edge so it joins the page |
| Aero / `GlassKd4` | `content_bg` strip, subtle baseline, normal text | light rounded-top face with Aero border/highlight and a restrained blue selected cue |
| Futurism / `SoftRounded` | `content_bg` strip with border-colored divider, normal text | selection-tinted rounded pill/segment with accent border/text; no forced white field face |

The selected state must remain legible without relying only on color. Classic
uses elevation, Aero uses face/border treatment, and Futurism uses shape plus
accent.

### D3 — Shared visualization tokens are theme data, not app constants

Add a small visualization palette to `userland/libs/gui::theme` (either named
fields on `Palette` or a separate `DataVizPalette`) covering:

- chart surface;
- grid;
- primary line and fill;
- secondary line;
- chart border and text, if they do not simply delegate to field tokens.

`TimeSeriesGraph::draw` uses the active visualization palette and
`draw_field_border` for its well. Default intent:

- Classic: white sunken data well, dark navy primary line, subdued light fill;
- Aero: white well, Aero blue primary line/fill;
- Futurism: white/light well, `#3C8CF0` primary line and soft blue fill;
- secondary RX/TX green remains semantically stable but must meet contrast
  against every chart surface.

Expose the effective primary/secondary colors through the theme palette so a
consumer legend and the graph cannot drift.

### D4 — Finish the shared list/header presentation where Task Manager exposes it

Have `ColumnListView` call `theme::draw_selection` for selected rows, clipped
to the body and excluding the scrollbar gutter. Add a theme-owned header cell
or header-band painter:

- Classic headers read as raised clickable controls;
- Aero headers use the light panel/border construction;
- Futurism headers stay flat with subtle dividers and an accent sort cue.

Keep sorting, numeric comparison, column geometry, selection keys, and
scrolling unchanged. Sort direction must remain visible in all themes.

### D5 — Task Manager composes shared controls and palette tokens

Change `TaskMgr` to:

- clear the page with `theme::palette().content_bg`;
- use `palette.text` and `palette.disabled_text` for page copy;
- own and lay out a `StatusBar` instead of painting the footer by hand;
- source the network legend from the same visualization palette as
  `TimeSeriesGraph`;
- keep green as the semantic secondary series, not as generic application
  chrome;
- retain the current `Button`, `ColumnListView`, graph, and dialog types.

Application-owned data and graph labels remain in Task Manager. Generic
surface styling belongs in the toolkit.

### D6 — Route tabs through their typed control contract

Task Manager currently bypasses `TabBar::handle_input` for pointer clicks and
cycles tabs on every plain Tab key even though the documented convention is
Ctrl+Tab. Converge on the shared response contract:

- pointer input goes through `TabBar::handle_input`;
- Ctrl+Tab advances and Ctrl+Shift+Tab reverses, wrapping;
- Left/Right/Home/End continue to work when routed to the tab strip;
- plain Tab is reserved for future focus traversal and no longer changes
  pages;
- a `Changed(index)` action marks the app dirty and updates no data-plane
  state.

If full focus traversal is not introduced in this change, the arrow-key
route may remain an explicit Task Manager shortcut. Do not create a partial
application-wide focus manager solely for tabs.

### D7 — The visual gallery is the regression surface

Extend `GUIDEMO.ELF` with a compact `TabBar`, `ColumnListView`,
`TimeSeriesGraph`, and `StatusBar`. It should demonstrate:

- selected and inactive tabs;
- a sorted column and selected row;
- one- and two-series charts;
- live theme change without reopening the app.

This closes the acceptance gap left by the earlier control-maturity and
Futurism plans.

## Implementation units

### U1 — Shared tab painter and module boundary

- Add the finish-dispatched tab strip/tab/text helpers in
  `userland/libs/gui/src/theme.rs`.
- Move `TabBar` to `userland/libs/gui/src/tab_bar.rs` if doing so keeps the
  change easier to review; preserve `pub use tab_bar::{TabBar, TabBarAction}`
  and construction behavior.
- Replace the selected `field_bg` and inactive `disabled_text` choices with
  the new theme contract.
- Add reverse cycling and modifier-correct Ctrl+Tab handling to the shared
  control.

Gate: a small canvas renders visibly different Classic, Aero, and Futurism
selected tabs; selected faces never use an accidental universal-white token.

### U2 — Shared monitoring and list surfaces

- Add visualization palette data and convert `TimeSeriesGraph`.
- Convert `ColumnListView` selection to `draw_selection`.
- Add and adopt the shared list-header painter without changing layout or
  sort behavior.
- Improve `StatusBar` through a theme helper only if Classic's panel edge
  cannot be expressed by its existing palette-based implementation.

Gate: these components contain no use of the legacy `COLOR_*` constants
unless a remaining constant is documented as intentionally semantic and
theme-invariant.

### U3 — Task Manager adoption

- Replace fixed page/text colors with `gui::theme` values.
- Add a `StatusBar` field, update its bounds in `layout`, and set its text from
  the existing status snapshot in `render`.
- Use the shared visualization colors for RX/TX legend swatches.
- Route tab input through the control response.
- Preserve the 1 Hz sampler, 100 ms sleep loop, selected PID, End Task
  escalation, resize layout, and modal routing.

Gate: `userland/apps/taskmgr/src/main.rs` no longer imports
`COLOR_WHITE`, `COLOR_PANEL`, `COLOR_BORDER`, `COLOR_TEXT`,
`COLOR_TEXT_DIM`, `COLOR_HIGHLIGHT`, or `COLOR_ACCENT2`.

### U4 — Gallery, documentation, and validation

- Add the four missing controls to `GUIDEMO.ELF`.
- Update `userland/README.md` only where behavior changed: tabs are
  finish-themed, graphs use visualization tokens, and Ctrl+Tab semantics are
  explicit.
- Add focused behavior tests for tab selection/wrapping/modifiers at the
  lowest practical layer. Keep color/pixel checks in the `gui` crate or a
  small render harness; do not put palette data in `gui-core` just to make a
  test compile.
- Build the toolkit, GUIDEMO, and Task Manager, then run the relevant existing
  GUI checks.

## Validation matrix

### Automated

- Formatting and compile checks for `gui`, `guidemo`, and `taskmgr`.
- Existing `./userland/test-gui-core.sh`.
- Tab behavior tests:
  - click selects the correct variable-width tab;
  - click outside the strip is ignored;
  - Ctrl+Tab wraps first → last → first as appropriate;
  - Ctrl+Shift+Tab wraps in reverse;
  - plain Tab does not change the active page;
  - Left/Right/Home/End clamp or select as documented.
- Painter checks or render-harness assertions:
  - Classic selected tab includes Classic bevel colors and is not white;
  - Futurism selected tab includes the accent/selection treatment;
  - inactive enabled labels do not use `disabled_text`;
  - graph border/surface/primary line dispatch by theme;
  - `ColumnListView` selected rows traverse the theme selection painter.

### Manual QEMU

Run Task Manager and GUIDEMO under all three themes and switch themes live
from Control Center while both remain open.

Verify:

- **Classic:** client/page/footer are ButtonFace grey; selected tab is a
  raised Classic tab joined to the page; list headers are raised; charts are
  white sunken wells rather than unframed white cards.
- **Aero:** selected tab, list headers, status bar, charts, buttons, and
  scrollbars read as one light control family.
- **Futurism:** no legacy grey/black borders or universal-white tab face
  remain; selection pills, accent cues, chart colors, and muted text are
  consistent.
- Active, inactive, and disabled states are visually distinct.
- Processes, Performance, and Network retain their existing data and layout
  across resize.
- Mouse tab selection, Ctrl+Tab, Ctrl+Shift+Tab, list sorting/scrolling, End
  Task confirmation, and live modal retheming still work.

## Scope boundaries

In scope:

- shared ring-3 tab, graph, list-header/selection, and status surfaces used by
  Task Manager;
- Task Manager removal of fixed theme-sensitive colors;
- the GUIDEMO regression surface.

Out of scope:

- Task Manager sampler or `/proc` changes;
- new metrics, charts, pages, icons, or process actions;
- redesigning Control Center's custom sidebar navigation;
- kernel-side tabs or a new GUI ABI;
- a general layout engine or application-wide focus manager;
- converting intentionally bespoke applications such as Calc to stock
  controls;
- making opaque ring-3 client surfaces translucent.

## Risks

- **Classic tab geometry:** a selected tab must overwrite the strip baseline
  beneath itself but not leave a one-pixel seam into the page. Keep all
  painting clipped to the tab strip and test the boundary row.
- **Rounded selection clipping:** Futurism row selection must not paint into
  the scrollbar gutter, header, or neighboring rows.
- **Tiny graph bounds:** `TimeSeriesGraph` currently performs plot-width
  arithmetic that assumes usable dimensions. Theme borders must preserve its
  current small-size guards or add explicit safe early rendering.
- **Public API churn:** moving `TabBar` to a module must preserve its root
  re-export. Prefer accessors for new behavior, but avoid unrelated breaking
  changes.
- **Theme-event duplication:** Task Manager uses raw nonblocking runtime
  events and explicitly calls `apply_system_event`; keep that path. Do not
  also apply the same event through a second wrapper in its loop.

## Definition of done

Task Manager contains no fixed colors for theme-sensitive chrome, all of its
stock surfaces come from shared GUI controls/theme painters, and switching
among Classic, Aero, and Futurism updates the open window immediately. The
Classic selected tab is a native-looking raised grey tab rather than a white
field, and `TabBar` remains the one reusable ring-3 tab component for future
applications.

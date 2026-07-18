---
title: "feat: replace every Links2 widget with native AgenticOS UX"
type: feat
status: implemented
date: 2026-07-18
depth: large
related_docs:
  - docs/plans/2026-07-18-008-feat-links2-rust-gui-driver-plan.md
  - docs/plans/2026-07-18-007-feat-userland-gui-control-maturity-and-scrollable-text-area-plan.md
  - docs/plans/2026-07-18-006-feat-modern-common-file-dialog-plan.md
  - docs/plans/2026-07-18-003-feat-theme-aware-controls-plan.md
  - src/userland/CLAUDE.md
  - src/window/CLAUDE.md
  - userland/apps/links2/README.md
  - userland/libs/gui/src/lib.rs
  - userland/libs/dialogs/src/lib.rs
  - https://links.twibright.com/user_en.html
---

# feat: replace every Links2 widget with native AgenticOS UX

## Summary

Replace the entire Links-owned graphical widget layer with AgenticOS's shared
ring-3 GUI language while keeping Links 2.30 as the browser engine. In
AgenticOS graphics mode, Links continues to own networking, cache/history,
HTML parsing and layout, page painting, form data, bookmarks, downloads,
configuration, and command callbacks. A Rust native-widget host owns all
application chrome and all interactive control presentation and behavior:

- persistent application menus, submenus, context menus, and select popups;
- navigation toolbar, location field, status surface, and document scrollbars;
- message, input, settings, information, and download dialogs;
- buttons, labels, text/password fields, check boxes, radio groups, combo
  boxes, progress bars, lists, trees, and dialog button rows;
- bookmark, association, extension, block-list, history, and download
  management surfaces;
- embedded HTML text/password/file fields, text areas, check boxes, radio
  buttons, selects, submit/reset/button controls, and their focus treatment.

This is a semantic replacement, not a skin. The AgenticOS host measures,
lays out, paints, focuses, hit-tests, edits, scrolls, and activates controls.
The Links C side remains the authoritative browser model and invokes the same
existing callbacks after receiving stable native action IDs.

The completion bar is intentionally strict: when `LINKS.ELF` runs with the
`agenticos` graphics driver, no Links BFU graphical menu, dialog, field,
button, check-box, progress-meter, list, form-control, title-row, status-row,
or scrollbar renderer may paint widget pixels. Text-mode Links retains the
existing BFU implementation and behavior.

## What “every widget” means

The following are widgets and are in scope:

1. **Browser chrome** — menu bar, navigation buttons, location entry, status,
   page scrollbars, focus indicators, busy/progress indication.
2. **Transient UI** — menus, nested menus, context menus, combo/select popups,
   tooltips where Links exposes them, message boxes, prompts, and modal
   dialogs.
3. **Management UI** — bookmarks and all other `listedit.c` list/tree
   managers, download progress, configuration editors, and history choices.
4. **Embedded HTML controls** — every visible `FC_*` form control, including
   editing and pointer/keyboard behavior rather than only its border colors.

The following are browser content, not widgets, and remain Links-rendered:

- page text, headings, links, tables, frames, backgrounds, and ordinary
  images;
- author-supplied image-submit pixels (`FC_IMAGE`). The image remains page
  content, but focus, pressed state, activation, and keyboard behavior use the
  native control path;
- hidden form inputs (`FC_HIDDEN`), because they have no visible or
  interactive surface;
- HTML layout, submission encoding, navigation policy, cache, downloads, and
  protocol behavior.

## Current-state findings

The pinned Links 2.30 source has three UI layers that must all be replaced.

### Links BFU menus and dialogs

`bfu.c` centralizes nearly all application UI:

- `do_mainmenu`, `do_menu`, and `do_menu_selected` own the main menu, dynamic
  menus, nested menus, HTML select popups, and context menus;
- `do_dialog` and `dialog_func` own dialog lifecycle, focus, keyboard routing,
  mouse routing, validation, and redraw;
- `struct dialog_item` exposes only `D_CHECKBOX`, `D_FIELD`, `D_FIELD_PASS`,
  and `D_BUTTON`; labels and groups are emitted through `dlg_format_*`
  helpers by each dialog's layout function;
- `msg_box` and `input_field` are constructors over the same generic dialog
  machinery.

This centralization makes a complete replacement possible without rewriting
every settings dialog by hand, provided the integration intercepts semantic
constructors and layout helpers rather than graphics primitives.

### Specialized Links windows

Two important surfaces bypass portions of the generic BFU renderer:

- `listedit.c` custom-paints and custom-routes bookmark trees and the
  association, extension, and block-list managers;
- `session.c::download_window_function` custom-paints transfer text and a
  progress meter inside a generic dialog.

These need explicit native adapters after the generic dialog bridge lands.

### Embedded document controls

The graphical page renderer draws controls inside `view_gr.c::g_text_draw`.
It currently renders check boxes, radio buttons, selects, fields, password
fields, file fields, and text areas as styled text and underscores. Links
owns editing in `view.c::field_op`, activation in `view.c::enter`, select
menus through `do_select_submenu`, and document scrollbars in `view_gr.c`.

A complete replacement therefore needs both a native measurement hook during
HTML layout and a native control registry during document drawing. A paint-
only swap would leave Links's text-oriented focus and editing behavior in
place and does not satisfy this plan.

### AgenticOS GUI platform

The current Links driver exposes only raster and device callbacks. It has no
menu/dialog/widget semantics. The shared ring-3 toolkit already provides
theme-aware buttons, text fields, text areas, sliders, scrollbars, lists,
column lists, tabs, and common dialogs, but it deliberately lacks:

- a widget tree, focus manager, automatic layout, or command registry;
- multi-menu bars, nested popup menus, combo boxes, check boxes, radio
  buttons, tree views, progress bars, and reusable modal layout;
- an embedded-musl link mode. `runtime` currently owns standalone startup and
  a `brk` allocator, while Links already owns startup and musl's allocator;
- general UTF-8 glyph measurement. The current canvas cache and layout assume
  the bundled monospaced system face's ASCII cell width.

Those are platform prerequisites, not Links-specific drawing code.

## Goals

1. Make Links graphics mode look and behave like an AgenticOS application in
   both Classic and Aero themes.
2. Replace every visible and interactive Links widget, including HTML form
   controls, without replacing the browser engine or page renderer.
3. Use the same GUI control implementation as other ring-3 applications, not
   a Links-only imitation of it.
4. Preserve Links callback, validation, history, configuration, form,
   navigation, and download semantics behind opaque command IDs.
5. Give all replacement controls coherent focus traversal, keyboard
   accelerators, pointer capture, dismissal, editing, selection, and resize
   behavior.
6. Preserve text-mode `links`, `links -dump`, and non-AgenticOS graphics
   source paths.
7. Keep the C fork reviewable through narrow hooks and versioned C/Rust data
   contracts.
8. Prove by instrumentation that the legacy graphical widget renderers are
   unreachable under the AgenticOS native path.

## Non-goals

- Replacing Links's HTML parser, layout engine, page text/image renderer,
  networking, history, cache, bookmark storage, form submission, or download
  engine.
- Adding JavaScript, TLS, IPv6, tabs, multiprocess site isolation, media, or
  modern CSS features.
- Adding a kernel syscall for each control. AgenticOS ring-3 applications own
  their control trees and paint client surfaces; this feature follows that
  architecture.
- Making Links's BFU generic across all upstream platforms. Text mode keeps
  BFU, and only the `agenticos` graphical backend selects the native host.
- Shipping an accessibility/screen-reader service before AgenticOS has a
  system accessibility ABI. The native tree still records roles, names,
  values, enabled state, and focus order so it can be published later.
- An OS-wide clipboard service. Native editing provides selection and an
  application-local clipboard and wires Links's clipboard callbacks; a shared
  desktop clipboard can replace the storage later without changing widgets.

## Requirements

### R1 — complete graphical widget replacement

- **R1.1.** `agenticos` graphics mode never calls a legacy graphical widget
  painter for menus, dialogs, lists, controls, chrome, status, progress, or
  scrollbars.
- **R1.2.** A test-only strict mode records any attempted legacy widget paint
  and fails the QEMU fixture with the widget category and source hook.
- **R1.3.** Text mode continues to call the existing BFU paths unchanged.
- **R1.4.** Page content remains visible while native transient UI is open,
  but cannot receive input through a modal or popup capture layer.

### R2 — one native widget implementation

- **R2.1.** Links consumes shared `gui-core` state and shared `gui` rendering;
  it does not fork button/menu/text-edit implementations into the driver.
- **R2.2.** The toolkit gains a borrowed render target so it can draw directly
  into Links's existing XRGB8888 surface without a second full-size canvas.
- **R2.3.** Standalone Rust apps and the embedded Links host produce identical
  control geometry and pixels for the same theme, bounds, state, and text.
- **R2.4.** The embedded host uses musl `malloc`/`realloc`/`free` as Rust's
  sole allocator. It never links runtime startup or the runtime `brk` heap.

### R3 — semantic C/Rust boundary

- **R3.1.** The C side publishes versioned, bounded descriptors containing
  copied UTF-8 strings, stable IDs, roles, flags, values, and bounds.
- **R3.2.** Rust never retains a raw Links callback or transient Links pointer.
  C retains the ID-to-callback/data mapping and owns all callback lifetimes.
- **R3.3.** Rust returns actions into a bounded queue. C dispatches them from a
  Links bottom half after the Rust call returns, preventing reentrant menu or
  dialog destruction.
- **R3.4.** Malformed kinds, lengths, IDs, UTF-8, nesting, and counts fail
  closed. One bad surface closes that native UI transaction, not the browser.

### R4 — native focus and input behavior

- **R4.1.** One focus manager owns Tab/Shift-Tab order, default/cancel buttons,
  mnemonic activation, arrow navigation, Enter/Space activation, and focus
  restoration after transient UI closes.
- **R4.2.** Pointer press captures through release; activation occurs only on
  a valid release; cancel/close clears capture and pressed state.
- **R4.3.** Menus support keyboard traversal, type mnemonics, nested submenus,
  disabled/check/radio states, shortcut labels, hover switching, outside-click
  dismissal, Escape unwinding, and screen-edge clamping.
- **R4.4.** Text controls support selection, caret placement, drag selection,
  Home/End/arrows, Shift extension, Ctrl-A/C/X/V, password masking, read-only,
  maximum length, multiline navigation, and scroll-to-caret.
- **R4.5.** Theme, focus, resize, close, and cancellation events leave no stuck
  modal, pressed, hover, capture, or caret state.

### R5 — native browser chrome

- **R5.1.** A persistent multi-menu bar exposes the existing File, View,
  Setup, and Help command families and their dynamic submenus.
- **R5.2.** A navigation toolbar provides Back, Forward, Reload/Stop, location
  entry, and Go using existing Links commands and history.
- **R5.3.** The page title remains in the server-rendered frame title. The old
  in-content back-arrow/title row is removed.
- **R5.4.** A native status surface shows hovered-link/status/loading text and
  does not consume document height when configured hidden.
- **R5.5.** The document viewport is explicitly inset by chrome metrics;
  Links page rendering never draw-overlaps the menu, toolbar, or status.

### R6 — native menus and dialogs

- **R6.1.** Every `do_menu`, `do_menu_selected`, and `do_mainmenu` invocation
  becomes a native menu transaction under the AgenticOS driver.
- **R6.2.** Dynamic history, download, language, charset, font, window, image
  map, HTML select, and link-context menus retain ordering and callbacks.
- **R6.3.** Every generic `struct dialog` item maps to a native control, and
  every `dlg_format_*` label/group maps to native layout content.
- **R6.4.** Links validation functions remain authoritative. Failed OK
  validation focuses the offending native control and presents the translated
  native error without closing the dialog.
- **R6.5.** `msg_box`, `input_field`, file selection, overwrite confirmation,
  color selection, and download progress use shared native dialog surfaces.
- **R6.6.** Dialog updates are incremental: a transfer progress refresh does
  not rebuild unrelated controls or reset focus.

### R7 — native management surfaces

- **R7.1.** `listedit.c` data is exposed as a native flat list or tree model,
  preserving folder open state, selection marks, current item, scrolling,
  add/edit/delete/move/search actions, and custom action buttons.
- **R7.2.** Bookmarks use a tree view; associations, extensions, and the block
  list use the appropriate list/column-list presentation.
- **R7.3.** Native list actions call existing C operations. Rust never mutates
  Links list nodes directly.
- **R7.4.** Large lists virtualize visible rows and bound copied strings.

### R8 — native HTML controls

- **R8.1.** HTML layout asks the native host for control minimum/preferred
  metrics; rendered bounds and hit bounds derive from the same result.
- **R8.2.** Visible control identity is stable across redraw and scrolling via
  session/frame/form/control IDs, never a raw address alone.
- **R8.3.** The native host draws and routes all visible `FC_TEXT`,
  `FC_PASSWORD`, `FC_FILE_UPLOAD`, `FC_TEXTAREA`, `FC_CHECKBOX`, `FC_RADIO`,
  `FC_SELECT`, `FC_SUBMIT`, `FC_RESET`, and `FC_BUTTON` controls.
- **R8.4.** Rust editing state synchronizes atomically to Links `form_state`
  before validation, submit, reset, navigation, reload, or form destruction.
- **R8.5.** Radio exclusion, select indices, default/reset values, read-only,
  disabled state, maximum length, rows/columns, and GET/POST submission remain
  Links-authoritative.
- **R8.6.** File upload invokes the native common file dialog and writes the
  chosen guest path through the existing Links form state.
- **R8.7.** Controls clip and scroll with their document frame; off-screen
  controls neither draw nor accept pointer input.
- **R8.8.** Form focus continues to participate in Links link traversal and
  frame navigation while using native focus visuals and editing behavior.

### R9 — theme, font, and localization

- **R9.1.** All controls use the active AgenticOS control palette and metrics;
  theme changes repaint open menus, dialogs, management windows, and HTML
  controls without closing them or losing state.
- **R9.2.** The shared font layer gains measured UTF-8 glyph advances and a
  bounded glyph cache. Layout never assumes one byte or one ASCII cell per
  character.
- **R9.3.** Links remains the translation owner. Translated UTF-8 labels,
  mnemonics, shortcuts, values, and messages cross the bridge as copied text.
- **R9.4.** Small windows reflow or scroll dialogs rather than truncating
  buttons or placing controls outside the client area.

### R10 — lifecycle, bounds, and compatibility

- **R10.1.** Closing the browser destroys every native modal/popup and frees
  every copied descriptor, control state, bitmap, and callback registry entry.
- **R10.2.** Menu/dialog node counts, nesting, text bytes, list rows, form
  controls, and action queues have explicit caps and checked arithmetic.
- **R10.3.** Forked DNS helpers inherit no live GUI host state or descriptors.
- **R10.4.** `links`, `links2`, `links -dump`, network fixtures, config files,
  bookmarks, history, downloads, and command shortcuts retain behavior.
- **R10.5.** The stripped prebuilt remains below the user-binary input cap and
  links no dynamic interpreter or host library.

## Architecture decisions

### AD1 — replace semantics before pixels

Do not detect menu rectangles or text in raster callbacks. Add an
AgenticOS-native UI hook table to the pinned Links source and invoke it from
BFU constructors, BFU layout helpers, specialized managers, and HTML control
layout/draw/event sites. Raster callbacks remain page-painting primitives.

The high-level flow is:

```text
Links model / callbacks (C)
        |
        | versioned semantic descriptors + stable IDs
        v
AgenticOS native-widget adapter (thin C registry)
        |
        v
Rust NativeUiHost ── gui-core state/layout/focus
        |            gui theme/font/control rendering
        v
existing Links XRGB8888 surface + existing GUI window/event fd
        |
        | bounded AgUiAction queue
        v
Links bottom half -> original callback / validation / form mutation
```

This preserves Links's single-threaded selector and callback order.

### AD2 — keep Links as the model owner

Native controls do not become a second browser model. Links continues to own:

- menu callback/data pairs and dynamic menu generation;
- dialog item storage, checks, OK/cancel callbacks, refresh, and abort;
- bookmark/list nodes and edit operations;
- download status and cancellation;
- HTML `form_control` and `form_state`, reset, radio groups, select values,
  validation, submission encoding, and navigation.

Rust owns presentation state that Links does not model well: hover, pressed,
pointer capture, focus ring, selection anchor, popup placement, caret blink,
layout cache, and per-control scroll geometry. Every action synchronizes its
value to C before invoking the existing Links callback.

### AD3 — make the GUI stack embeddable in a musl process

Add explicit runtime modes:

- `standalone` (default): current `_start`, panic, and `brk` allocator for
  native Rust ELFs;
- `embedded`: syscall/types only, with no startup symbol, panic handler, or
  global allocator.

The Links Rust static library selects `embedded`, defines one global allocator
backed by musl `malloc`/`realloc`/`free`, and supplies its existing fatal panic
path. `gui` and `dialogs` forward the runtime feature and must not assume the
standalone heap. This keeps one allocator owner while allowing shared `Vec`,
`String`, font, widget, and dialog code.

### AD4 — add a borrowed canvas, not another framebuffer

Factor drawing so `Canvas` operations target either owned pixels or a checked
borrowed XRGB8888 slice. The Links host wraps its current surface with a
`BorrowedCanvas` during a render call. It never allocates or copies a second
1024x700 buffer merely to use native controls.

Clips remain nested and restored through guards. A widget can draw only inside
the supplied surface length, stride, damage region, and clip.

### AD5 — one input router and one modal stack

The driver drains the existing selectable GUI event descriptor and routes in
this order:

1. an owned common-dialog window matching `event.window`;
2. the top native modal or popup capture layer;
3. persistent browser chrome;
4. the visible embedded HTML control registry;
5. ordinary Links page keyboard/mouse handling.

Consumed events never also reach Links. Unconsumed page events retain the
existing mapping. When a native action needs C work, it is queued and handled
after event draining returns to Links.

### AD6 — native dialogs are declarative views over BFU storage

Avoid rewriting roughly every menu/settings callsite. In the AgenticOS path,
the second/rendering pass through `dlg_format_text`,
`dlg_format_text_and_field`, `dlg_format_buttons`,
`dlg_format_checkbox(es)`, `dlg_format_field`, and `dlg_format_group` emits a
declarative dialog tree. The host's form layout measures and places it.

Legacy BFU coordinates are ignored for native layout but remain populated so
text mode and callback assumptions survive. `dialog_item_data` continues to
hold C values. Specialized download/list surfaces provide dedicated model
adapters because they draw outside these helpers.

### AD7 — persistent chrome uses explicit document insets

The graphics device continues to describe the whole client surface. A native
chrome layout returns a document viewport:

```text
+----------------------------------------------------------+
| File  View  Setup  Help                                  |
+----------------------------------------------------------+
| <-  ->  Reload  [ location........................ ] Go   |
+----------------------------------------------------------+
|                                                          |
|                 Links-rendered document                  |
|            with native embedded form controls            |
|                                               [scroll]   |
+----------------------------------------------------------+
| status / hovered URL / loading progress                  |
+----------------------------------------------------------+
```

`session.c::set_doc_view` consumes that rectangle. The old graphical title
row, bottom status painter, and scrollbars are bypassed. Resize computes
chrome first, then updates the Links viewport and HTML layout once.

### AD8 — HTML controls use stable ephemeral registration

During document layout, C asks Rust for native metrics. During each visible
draw, C registers controls with stable IDs and physical clipped bounds, then
Rust paints them. The registry is generation-based: controls not observed in
the current frame are removed after the draw. A control ID includes logical
session, frame, form, and control ordinals so address reuse cannot target stale
form data.

Control events return value/edit actions. C resolves the ID against the
current document generation, updates `form_state`, and requests Links redraw
or activation. Navigation/reload invalidates the entire generation before
freeing forms.

### AD9 — no silent fallback after migration

A temporary development flag may switch individual categories while their
milestone is in progress. Each milestone removes that category's fallback.
The final `agenticos` build has no runtime option that silently returns to the
Links graphical widget painter. Initialization fails with a bounded native
error if the host cannot start. Text mode remains the supported fallback.

## Native control matrix

| Links surface | Semantic hook | Native replacement | Authoritative state |
|---|---|---|---|
| Main menu | `do_mainmenu` / `activate_bfu_technology` | `MenuBar` + command model | Links callbacks |
| Dropdown/nested menu | `do_menu*`, `in_m` | `MenuPopup` hierarchy | C menu registry |
| Link context menu | `link_menu` -> `do_menu` | Context `MenuPopup` | Links callbacks |
| HTML select | `do_select_submenu` | `ComboBox` popup | `form_state.state` |
| Message box | `msg_box` | shared `MessageBoxView` | Links button callbacks |
| Prompt | `input_field` | form dialog + `TextField`/history combo | dialog `cdata` |
| Generic settings dialog | `do_dialog`, `dlg_format_*` | native modal form layout | dialog items/checks |
| Password entry | `D_FIELD_PASS` | `PasswordField` | dialog `cdata` |
| Check/radio groups | `D_CHECKBOX`, `gid/gnum` | `CheckBox`/`RadioButton` | dialog checked state |
| Dialog buttons | `D_BUTTON`, `B_ENTER/B_ESC` | default/cancel `Button` | item callback |
| Bookmark manager | `listedit.c`, tree mode | virtualized `TreeView` | Links list nodes |
| Other list managers | `listedit.c` | `ListView`/`ColumnListView` | Links list nodes |
| Download window | `download_window_function` | labels + `ProgressBar` + buttons | download/status structs |
| Color selection | color callbacks/dialog | shared `ColorPicker` view | Links color option |
| Save/download path | `query_file` | shared `FileDialog` view | Links download callback |
| Overwrite question | `does_file_exist` | native confirmation | Links continuation |
| Browser title row | `draw_title` | frame title + toolbar | session/location |
| Browser status row | `print_screen_status` | `StatusBar` | `ses->st` |
| Page scrollbars | `draw_*scroll_bar` | native `Scrollbar` | view positions |
| HTML text/password/file | `g_text_draw`, `field_op` | native field controls | `form_state.string` |
| HTML textarea | same | native `TextArea` | string/caret/view positions |
| HTML checkbox/radio | same + `enter` | native check/radio | `form_state.state` |
| HTML submit/reset/button | `g_text_draw`, `enter` | native `Button` | Links activation |
| HTML image submit | image object + activation | author image + native focus/press | Links activation |

## C/Rust interface

Add `userland/apps/links2/driver/agenticos_ui.h` as the sole shared contract.
All structs start with `{ version, byte_len }`; all enums have fixed-width
integer representation. Representative contracts:

```c
struct ag_ui_text {
    const unsigned char *ptr;
    uint32_t len;
};

struct ag_ui_node {
    uint32_t version, byte_len;
    uint64_t id;
    uint32_t kind, flags, role, group;
    int32_t x, y;
    uint32_t width, height;
    int64_t value, value_min, value_max;
    struct ag_ui_text label, secondary, value_text;
};

struct ag_ui_action {
    uint32_t version, byte_len;
    uint64_t target;
    uint32_t kind, flags;
    int64_t value;
    const unsigned char *text;
    uint32_t text_len;
};
```

The production contract uses explicit begin/add/finalize calls or a bounded
packed snapshot; it does not expose Rust layout or `Vec` layouts to C.

Initial limits:

- 64 open transient surfaces per browser process;
- 8 nested popups;
- 256 nodes per ordinary dialog/menu and 1,024 visible HTML controls;
- 4,096 virtualized list rows per snapshot, with only visible row text copied;
- 1 MiB aggregate copied UI text and 256 KiB per action text;
- 256 queued actions, coalescing value/hover/repaint updates where safe.

Exceeding a limit yields a translated native error and closes the affected
surface. It never truncates a callback mapping into the wrong item.

## Implementation milestones

### M0 — lock the legacy behavior and widget inventory

**Files:**

- `docs/plans/2026-07-18-009-feat-links2-native-widget-host-plan.md`
- `userland/apps/links2/patches/` inventory notes/tests
- `tools/net-test-http.py` and GUI fixture assets
- `src/tests/gui_userland.rs`

**Work:**

- add one deterministic fixture containing every visible HTML control type,
  long content requiring both page scrollbars, nested frames, disabled and
  read-only controls, Unicode labels, and GET/POST verification;
- script the current File/View/Setup/Help, nested/dynamic/context/select,
  message/prompt/settings, list/tree, and download surfaces;
- record callback order, form submission bytes, config mutations, bookmark
  mutations, focus transitions, and close/cancel behavior;
- enumerate every `do_dialog`, `msg_box`, `input_field`, `do_menu`, custom
  `listedit`, download, `g_text_draw` form, and scrollbar callsite in the
  pinned source; classify any callsite not covered by the matrix above;
- add test-only counters for legacy graphical widget paint categories before
  replacing them.

**Exit bar:** every graphical widget is assigned a native replacement and a
behavior fixture; the text/GUI baseline passes before platform refactoring.

### M1 — make the shared GUI toolkit embeddable and complete its controls

**Files:**

- `userland/runtime/Cargo.toml`
- `userland/runtime/src/lib.rs`
- `userland/libs/gui-core/src/{lib,input,text_edit,focus,command,layout}.rs`
- `userland/libs/gui/src/{lib,font,theme,canvas,menu,checkbox,radio,combo,tree,progress,toolbar,status,modal}.rs`
- `userland/libs/gui/Cargo.toml`
- `userland/libs/dialogs/Cargo.toml`
- `userland/test-gui-core.sh`

**Work:**

- split `runtime` into default standalone and allocator/startup-free embedded
  features without changing default app builds;
- add owned and borrowed canvas backends with identical checked drawing;
- add measured UTF-8 system-font text, clipping, ellipsis, alignment, mnemonic
  ranges, and a bounded glyph cache;
- implement `WidgetId`, command/action models, focus traversal, pointer
  capture, cancellation, automatic row/column/form layout, and scrollable
  modal content;
- mature `MenuBar` into multiple menus and add nested `MenuPopup`;
- add `CheckBox`, `RadioButton`, `ComboBox`, `PasswordField`, `TreeView`,
  `ProgressBar`, `Toolbar`, `StatusBar`, and `ModalHost`;
- factor common dialog presentation from window ownership so MessageBox,
  FileDialog, and ColorPicker views can run through either a normal Window or
  the Links modal host;
- migrate at least GUIDEMO to exercise the new controls and retain existing
  API compatibility where practical.

**Tests:**

- geometry/pixel equality between owned and borrowed canvas;
- focus, Tab/Shift-Tab, default/cancel, mnemonic, capture/cancel, menu nesting,
  outside dismissal, disabled/check/radio, combo, tree, and modal tests;
- UTF-8 measure/draw/cache bounds and malformed-string rejection;
- standalone builds retain the current allocator/startup; embedded link proof
  has no `_start`, `brk` allocator, or duplicate panic symbol.

**Exit bar:** a small static-musl C process links the embedded GUI stack with a
musl-backed Rust allocator and displays the native control catalog on a
borrowed surface.

### M2 — add the native host, semantic ABI, and event arbitration

**Files:**

- `userland/apps/links2/driver/agenticos_ui.h`
- `userland/apps/links2/driver/agenticos-ui.c`
- `userland/apps/links2/driver/agenticos.c`
- `userland/apps/links2/driver-rs/src/{lib,allocator,host,abi,event,render}.rs`
- `userland/apps/links2/driver-rs/Cargo.toml`
- `userland/apps/links2/patches/0003-add-agenticos-native-ui-hooks.patch`
- `userland/apps/links2/Makefile`

**Work:**

- define versioned descriptors, copied-text rules, caps, stable IDs, callback
  registries, action queues, and teardown order;
- add a generic native UI hook table to Links, selected only when the active
  driver advertises the AgenticOS native host;
- route GUI events through common-dialog windows, modal/popup, chrome, HTML
  controls, then Links page handling;
- defer C callback execution to bottom halves and validate that a target still
  belongs to the current UI generation;
- integrate dirty union so widget state changes schedule one coalesced present;
- make resize, theme, close, after-fork, and partial-init teardown idempotent;
- implement strict legacy-paint instrumentation for tests.

**Exit bar:** a synthetic C menu/dialog/control tree round-trips through Rust,
handles injected input, dispatches original C callbacks in order, repaints,
and leaks no callback registry, action, fd, surface, or allocation.

### M3 — replace browser chrome and all menu surfaces

**Files:**

- Links patch touching `menu.c`, `bfu.c`, `session.c`, `view.c`, and `links.h`
- `driver-rs/src/{chrome,menu}.rs`
- shared GUI menu/toolbar/status modules

**Work:**

- create persistent File/View/Setup/Help menus from existing translated menu
  definitions and dynamic callback builders;
- replace `do_mainmenu`, `do_menu`, `do_menu_selected`, link context menus,
  image-map menus, HTML select menus, history/download/window/language/font/
  charset menus, separators, shortcuts, and nested `in_m` menus;
- map native menu actions to the original `func(term, data, context)` pair;
- build native Back, Forward, Reload/Stop, location/history combo, Go, and
  loading state over existing Links operations;
- replace graphical title and status rows, compute an explicit document
  viewport, and update page layout on chrome visibility or resize;
- preserve all keyboard shortcuts even when the menu bar is not focused.

**Tests:**

- every static and dynamic menu opens by mouse and keyboard, nested menus
  switch and dismiss correctly, and the original callback fires exactly once;
- address typing/history/navigation and Back/Forward/Reload match baseline;
- small/large resize, Classic/Aero live switch, focus loss, and close while a
  menu is open leave valid content geometry;
- strict counters report zero title/status/menu legacy paint calls.

**Exit bar:** the main browser window has no Links-drawn chrome or menu pixels.

### M4 — replace generic BFU dialogs and common dialog flows

**Files:**

- Links patch touching `bfu.c`, `menu.c`, `default.c`, `view.c`, and dialog
  callsites requiring semantic metadata
- `driver-rs/src/{dialog,dialog_builder}.rs`
- `userland/libs/dialogs/src/*`

**Work:**

- intercept `do_dialog` lifecycle and emit labels, fields, passwords,
  checkboxes/radios, groups, buttons, default/cancel roles, and history;
- make `dlg_format_*` functions produce native declarative layout during the
  native rendering pass while preserving text behavior;
- synchronize edit/check values to `dialog_item_data` before original checks
  and button callbacks;
- retain refresh/abort and focus the item rejected by `check_dialog`;
- convert `msg_box` and `input_field` to shared native views;
- replace `query_file` and overwrite/rename prompts with the common native
  file/message dialog views while preserving Links continuation flags;
- map Links color selection to the shared native color picker;
- ensure every settings/info/about/keys/search/goto/save/proxy/network/cache/
  HTML/cookie/font/resize dialog is covered through the generic bridge.

**Tests:**

- one fixture per D_* type plus mixed groups, long labels, Unicode, history,
  validation failure, default Enter, cancel Escape, refresh, and abort;
- file open/save path, filter, overwrite/rename/cancel, and read-only mount
  behavior matches common dialog expectations and Links callbacks;
- enumerate all pinned-source dialog constructors and assert native creation;
- strict counters report zero generic BFU dialog-item/frame paint calls.

**Exit bar:** every ordinary Links prompt and settings dialog is native and
the old graphical dialog path is unreachable under `agenticos`.

### M5 — replace list managers and download progress

**Files:**

- Links patch touching `listedit.c`, `bookmark.c`, `types.c`, `block.c`, and
  `session.c`
- `driver-rs/src/{list_bridge,download}.rs`
- shared GUI tree/list/progress modules

**Work:**

- expose visible rows, depth, folder/open/marked/current state, and stable row
  IDs from `list_description` without exposing mutable list pointers to Rust;
- render bookmarks as a native tree and other managers as list/column-list
  views with native scrollbars, search, action buttons, double-click, and
  keyboard behavior;
- translate row/action events back to existing listedit operations;
- expose download URL, file, state, received/total, speed, elapsed, ETA, and
  cancel/background/delete commands to a native progress dialog;
- update progress values in place and coalesce refreshes.

**Tests:**

- nested bookmark folder open/close, mark, add/edit/delete/move/search and
  persistence;
- association, extension, and block-list CRUD and cancel/save behavior;
- large-list virtualization and Unicode row text;
- known/unknown-size download, completion, error, background, abort, and
  abort-delete transitions;
- strict counters report zero listedit/download legacy widget paint calls.

**Exit bar:** all custom management and progress surfaces are native.

### M6 — replace document scrollbars and every HTML form control

**Files:**

- Links patch touching `html.c`, `html_r.c`, `html_gr.c`, `view.c`,
  `view_gr.c`, and `links.h`
- `driver-rs/src/{document_controls,form_bridge}.rs`
- shared GUI field/text-area/check/radio/combo/button/scrollbar modules

**Work:**

- add native control measurement during graphical HTML layout;
- assign stable session/frame/form/control IDs and invalidate generations on
  reformat, reset, navigation, reload, and destruction;
- replace document scrollbar draw/hit/drag/wheel behavior while keeping Links
  view positions authoritative;
- register and draw native fields, password fields, file fields, text areas,
  checkboxes, radio buttons, selects, submit/reset/button controls, and image
  input focus/press overlays in physical clipped bounds;
- adapt native `TextEdit`/`TextArea` selection and caret state to Links
  `form_state`, including codepage conversion, maximum lengths, line wrapping,
  viewport positions, read-only state, reset, and submission synchronization;
- route select popups through native combo menus and file upload through the
  common file dialog;
- mirror native focus to Links `current_link`/`locked_link` so Tab, frame
  navigation, link menus, submission, and status text retain semantics;
- prevent offscreen/covered controls from drawing or hit-testing.

**Tests:**

- deterministic form fixture covers every visible FC_* type, tab order,
  mouse focus, selection/editing, Unicode, password masking, max length,
  readonly/disabled, text-area scrolling, radio exclusion, select, reset,
  file choice, and submit;
- server verifies exact GET/POST and file-field data after native edits;
- nested frames, page scroll, resize/reflow, control removal, navigation while
  focused, back/forward restoration, and theme change preserve valid state;
- controls partially clipped at every edge never draw or hit outside content;
- strict counters report zero Links form-control and scrollbar paint calls.

**Exit bar:** no visible Links widget remains inside the document viewport.

### M7 — remove migration paths, refresh the prebuilt, and document ownership

**Files:**

- all Links native UI patches and driver sources
- `userland/apps/links2/Makefile`
- `userland/apps/links2/README.md`
- `userland/prebuilt/LINKS.ELF`
- `userland/prebuilt/README.md`
- `CLAUDE.md`
- `src/userland/CLAUDE.md`
- GUI/dialog subsystem documentation

**Work:**

- remove category flags and native-to-legacy graphical fallbacks;
- keep text BFU compiled and prove its selection is mode-based, not a runtime
  failure fallback from native graphics;
- assert build inputs include all GUI/runtime/dialog sources so stale stamps
  cannot retain an older widget host;
- refresh and validate the committed prebuilt through the standard workflow;
- record final ELF size/symbols and fail the build above the loader cap;
- update architecture docs to state that Links owns browser data/page layout
  while the shared GUI host owns every graphical widget.

**Exit bar:** clean clones boot the native-widget browser from the committed
ELF, text mode remains unchanged, and source/docs describe one ownership model.

### M8 — complete bounded QEMU and manual acceptance

**Automated QEMU sequence:**

1. Start the browser from the Start menu and verify persistent native chrome.
2. Exercise every top-level, nested, dynamic, context, and select menu by key
   and pointer; verify callback counts and dismissal.
3. Exercise the generic dialog catalog, validation failures, settings writes,
   file dialog, color picker, and message choices.
4. Exercise bookmarks and each list manager with CRUD, move, search, and save.
5. Start slow known-size and unknown-size downloads; verify progress and every
   cancellation/background path while another app remains responsive.
6. Exercise the all-controls form and verify exact server submissions.
7. Scroll and resize across every edge case; switch Classic/Aero with menus,
   dialogs, text selection, and downloads open.
8. Close during each transient state and compare PID GUI records, fds, heap,
   callbacks, and surfaces to baseline.
9. Run text-mode HTTP/DNS/dump fixtures and native app GUI regression tests.
10. Assert every legacy graphical widget counter is zero for the entire run.

**Manual matrix:**

- Classic and Aero;
- mouse-only, keyboard-only, and mixed interaction;
- minimum supported window, default 1024x700, maximized display;
- short/long/Unicode translations and values;
- no network, slow network, error response, redirect, and active download;
- focus switching among browser, modal/common dialog, Terminal, and Task
  Manager;
- restart plus overlay `sync` for settings, bookmarks, and history.

**Exit bar:** the full matrix passes with no Links-rendered widget pixels,
crashes, spins, leaks, duplicate actions, stale form updates, or text-mode
regressions.

## Verification commands

Run proportionally after each milestone and the complete set before refresh:

```sh
cargo fmt --all --manifest-path userland/Cargo.toml -- --check
./userland/test-gui-core.sh
cargo check --release --manifest-path userland/Cargo.toml
cargo check --manifest-path userland/apps/links2/driver-rs/Cargo.toml \
  --target x86_64-unknown-linux-musl -Z build-std=core,alloc,compiler_builtins
make -C userland/apps/links2
REBUILD_LINKS2=1 ./build.sh -n
cargo check
cargo check --features test
./test.sh gui_userland userland start_menu window_theme
./test.sh
```

The source rebuild must additionally verify:

- static x86-64 non-PIE `ET_EXEC`, no interpreter, no dynamic dependencies;
- one allocator path backed by musl and no runtime `_start`/`brk` heap;
- all native UI patch assertions match the pinned Links 2.30 sources;
- `LINKS.ELF` remains below the kernel binary-read cap;
- two clean rebuilds are byte-identical or any nondeterminism is explained and
  removed before committing the prebuilt.

## Dependency and delivery order

```text
M0 inventory/baseline
       |
       v
M1 embeddable GUI + missing controls
       |
       v
M2 semantic host/event arbitration
       |
       +----------+----------+
       v          v          v
M3 chrome/menu  M4 dialogs  M6 HTML-control foundations
                  |
                  v
              M5 lists/download
       \          |          /
        +---------+---------+
                  v
          M6 complete HTML controls
                  |
                  v
          M7 removal/prebuilt/docs
                  |
                  v
             M8 full acceptance
```

Land M1 as a reusable GUI-platform change before the Links patches. Land the
Links migration in category-complete commits: chrome/menus, generic dialogs,
specialized surfaces, then embedded controls. Do not land a category with two
competing focus or event models enabled in production.

## Risks and mitigations

| Risk | Consequence | Mitigation |
|---|---|---|
| Rust allocator conflicts with musl | heap corruption | embedded runtime mode; one musl-backed global allocator; symbol/link tests |
| Callback fires while Rust/C UI is being destroyed | UAF/reentrancy | opaque IDs; bounded action queue; bottom-half dispatch after Rust returns |
| Dynamic submenu callback opens another menu | lost nesting or double popup | C transaction stack intercepts nested `do_menu` and attaches it to the requesting native popup |
| Generic dialog custom layout hides semantics | missing labels/controls | capture every `dlg_format_*` helper; dedicated adapters for only the two custom surfaces |
| Links and Rust disagree on form text/caret | corrupt submit or stale edit | atomic sync before callbacks/navigation; stable generations; exact POST fixtures |
| Native form metrics destabilize page layout | overlap or reflow loop | one measurement API used by layout/draw/hit-test; cache by theme/font/control descriptor |
| Control registry retains old document pointers | stale action mutates new page | logical IDs + generation validation; invalidate before freeing formatted data |
| Popup/modal and page both receive an event | duplicate navigation/action | single ordered router; consumed flag; callback-count tests |
| UTF-8 labels exceed ASCII font path | blank/mismeasured UI | measured Unicode glyph cache with fallback; multilingual fixtures |
| Large dialogs/lists exceed memory or window | OOM/unusable UI | caps, virtualization, scrollable modal content, checked allocation |
| Shared toolkit refactor regresses native apps | broad GUI regressions | default APIs remain; GUIDEMO catalog and all app checks before Links integration |
| Native UI substantially grows prebuilt | loader rejection | size gate each milestone; reuse shared code/LTO; keep page font/image stack unchanged |
| Theme changes rebuild state | lost edit/menu/modal | state separate from render cache; theme invalidates metrics/pixels only |
| Text-mode source accidentally selects native hooks | terminal regression | driver capability gate plus text fixtures at every milestone |

## Final acceptance criteria

The feature is complete only when all statements are true:

1. `links2 -g -driver agenticos` contains no Links-drawn widget pixels; strict
   legacy paint counters remain zero across the full UI test sequence.
2. Menus, toolbar, status, scrollbars, every dialog/control type, all list
   managers, download progress, and every visible HTML form control use shared
   AgenticOS GUI implementations.
3. Native focus, capture, keyboard navigation, editing, selection, modal
   dismissal, validation, resize, and live theme behavior are coherent.
4. Original Links callbacks, config/bookmark/history/download mutations, form
   reset/submission bytes, and navigation behavior match the baseline.
5. Page content and HTML layout remain Links-owned; no browser-engine behavior
   has moved into Rust.
6. Text-mode `links`, `links2`, and `-dump` retain existing behavior.
7. Browser close, process exit, error, and fork paths leak no windows, fds,
   callback entries, form registrations, allocations, or popup/modal state.
8. The committed `LINKS.ELF` is reproducible, static, within the loader cap,
   staged by normal builds, and launches from the Start menu on a clean clone.

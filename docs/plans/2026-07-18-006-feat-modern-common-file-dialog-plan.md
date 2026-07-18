---
title: "feat: modern common file dialog"
type: feat
status: completed
date: 2026-07-18
---

# feat: modern common file dialog

## Implementation outcome

Completed on 2026-07-18. The active ring-3 common chooser is now a responsive
760 x 500 Finder/Explorer-style dialog with shared code-drawn browser chrome,
Places, bounded history, breadcrumbs/Ctrl-L location entry, current-folder and
file-type filters, sortable metadata details, icon grid view, path-keyed single
selection, real timestamp-based double-click, explicit focus/Tab routing,
mode-aware final-target validation, default extensions, in-window overwrite
confirmation, and capability-aware New Folder with overlay sync reporting.

`FileDialog::open` and `save` remain source-compatible. Rich callers use owned
`FileDialogOptions`, `FileFilter`, and `FilePlace` configuration. Notepad now
supplies text filters, a `.txt` default, and current/last-directory behavior;
GUIDEMO exposes a direct Open smoke path. File Manager consumes the promoted
`gui::file_ui` icons, navigation buttons, breadcrumbs, Places, clipping,
date/size formatting, and mount capability model while retaining its own
multi-selection and operation policy. The capability model was also brought in
line with the current ext2 `/data` mount, enabling normal directory operations
there. The unused kernel open/save chooser modules were removed.

Verification completed with userland and kernel formatting, the full release
userland workspace check, regular and test-feature kernel checks, the complete
two-pass `./build.sh -n`, and the QEMU suite (`847` tests passed). Strict
`-D warnings` Clippy remains blocked by five pre-existing missing `# Safety`
sections in `userland/runtime`; ordinary Clippy completed and identified no new
build-blocking issue. The interactive Aero/Classic visual matrix remains the
recommended human acceptance pass.

## Summary

Replace the active ring-3 common `FileDialog`'s 560 x 380 directory label,
`[DIR]` text rows, synthetic `..`, and filename box with a modern local-file
picker that is immediately familiar to users of Finder and Windows Explorer.

The new chooser is a focused selection surface, not a miniature file manager:
it gains navigation history, Places, breadcrumbs/location entry, filtering,
file-type choices, details and grid views, real double-click, metadata, proper
focus/keyboard behavior, and mode-aware validation. It does not gain file
copy/delete/rename, recursive search, cloud providers, thumbnails, or preview.

Open and Save continue to share one implementation and the existing retained,
nonblocking modal contract. Existing callers keep compiling through
`FileDialog::open(start_dir)` and `FileDialog::save(suggested_path)`; richer
callers can provide options. Notepad adopts a text-file filter and sensible
last-directory behavior.

The old kernel-side `src/window/dialogs/file_open.rs` and `file_save.rs` have no
callers and are not the common dialog used by native apps. Remove those dead
exports as part of the migration so the repository has one canonical file
chooser rather than a second, even older implementation waiting to be revived.

## Problem frame

The active implementation is
`userland/libs/dialogs/src/file_dialog.rs`. It currently:

- opens a fixed 560 x 380 top-level window;
- draws `Directory: /path` as plain text and navigates upward through a
  synthetic `..` row;
- flattens every item into one 16 px `ListView` row, with folders represented as
  the literal string `[DIR] name` and no size, type, or modified information;
- treats a second click on the selected row as activation even though the GUI
  event ABI now provides real mouse-button timestamps;
- has no Back, Forward, Up, Places, breadcrumbs, editable location, refresh,
  sort, view choice, or current-folder filter;
- routes nearly every non-arrow key into the filename field because it has no
  explicit focus model or Tab order;
- stores no `FileMode` after construction, so Open and Save differ only by title
  and button label;
- lets Enter or the confirm button return an empty, missing, or directory path
  without validation; and
- immediately returns a file on activation, leaving no room for mode-specific
  selection behavior or Save overwrite confirmation.

The newer standalone File Manager already proves most of the desired visual
and interaction vocabulary: code-drawn navigation/place/file icons, Places,
breadcrumbs and Ctrl-L, current-folder filtering, details/grid views, metadata,
sorting, path-keyed selection, and true timestamp-based double-click. Those
pieces remain largely app-local in `userland/apps/fileman/src/main.rs` because
the File Manager was their only consumer. The common dialog is now the concrete
second consumer that justifies promoting the stable presentation primitives.

The shared filesystem API is also ready: `gui::DirEntry` already carries name,
directory status, size, modified time, and mode from `getdents64` plus
`newfstatat`. This feature requires no new filesystem syscall and no GUI ABI
change.

## Product experience

### Primary layout

Initial client size is 760 x 500, resizable down to 620 x 390. This fits the
default 1280 x 720 guest while giving filenames and metadata useful space.

```text
+ Open File -------------------------------------------------------------+
| [<] [>] [Up] [Refresh]   Root > host        [Filter this folder____]   |
|------------------------------------------------------------------------|
| PLACES       | Name                  Size       Type        Modified    |
| Start        |---------------------------------------------------------|
| Root         | [folder] docs         --         Folder      --          |
| Data         | [text]   README.md     7 KB       Text file   18 Jul ... |
| Host         | [app]    NOTEPAD.ELF  34 KB      Application 18 Jul ... |
|              |                                                         |
|              |                                                         |
|              |---------------------------------------------------------|
|              | 12 items | README.md | 7 KB                    [List][Grid]
| File name: [README.md_________________________]  [Text files_______v]   |
|                                                  [ Open ] [ Cancel ]   |
+------------------------------------------------------------------------+
```

The diagram describes code-drawn controls, not Unicode glyph dependencies.
The server-owned frame remains boot-theme aware. The browser surface uses the
same restrained hierarchy as File Manager under Aero; Classic keeps its stock
control rendering while receiving the same modern layout and interaction
model. Modernity here comes primarily from orientation, spacing, icons,
metadata, and predictable behavior—not from forcing a global desktop retheme.

### Interaction contract

- Single click selects and updates the file-name field/status. It never closes
  the dialog.
- Double-click or Enter activates. A directory navigates; in Open mode a valid
  file confirms. Double-click is same-item, within 50 ticks and 4 px, using
  `GuiEvent.payload[4..=5]`.
- Back/Forward preserve a bounded 64-location history. Up disables at `/`.
  Alt-Left/Alt-Right traverse history; Backspace goes up; F5 refreshes.
- Breadcrumb segments navigate. Ctrl-L switches to full location entry; Enter
  navigates and Escape returns to breadcrumbs.
- Places contains the requested start location (when distinct), Root, Data,
  and Host. Missing/unreadable standard places are omitted. The options API can
  add caller-owned places later without changing dialog internals.
- Ctrl-F focuses a substring filter for the current folder. It is deliberately
  labeled “Filter this folder”; it is not recursive search or indexing.
- Details view sorts folder-first by Name, Size, Type, or Modified. Grid view
  shows responsive icon tiles. One path-keyed selection model backs both.
- File-type choices filter files but never hide directories. “All files” is
  available unless the caller explicitly disables it. Changing the type choice
  reconciles selection and the enabled state of the confirm button.
- Tab/Shift-Tab cycle visible controls in a stable order. Arrow/Home/End/Page
  keys operate on the focused list/grid. Enter is focus-sensitive. Escape exits
  location/filter/name submodes first and otherwise cancels the dialog.
- Loading or navigation failures preserve the last successfully displayed
  folder and show an inline error/status message. The dialog does not disappear
  on recoverable input mistakes.

### Mode behavior

| Action | Open | Save |
|---|---|---|
| Select file | Fill name, enable Open if allowed by active filter | Fill name, enable Save |
| Activate file | Return its absolute path | Attempt Save; existing file enters overwrite confirmation |
| Activate directory | Navigate | Navigate |
| Type directory path + Enter | Navigate | Navigate |
| Empty/missing target | Keep open with explanation | Empty invalid; a new valid filename is allowed |
| Confirm existing directory | Navigate, never return it | Reject as a filename target |
| Existing target | Return only an existing regular file | Confirm overwrite in the same dialog window |
| Read-only directory | Browsing works | Save disabled with `Read-only` status |

Overwrite confirmation is an in-window confirmation panel/state, not a nested
`MessageBox`. A process has one GUI event queue and `Modal` routes by top-level
window handle; keeping the confirmation in the existing picker window avoids a
second level of modal routing in every host app.

## Requirements

### R1 — Compatible, configurable API

- **R1.1.** Preserve:
  `FileDialog::open(start_dir) -> Result<FileDialog, i64>` and
  `FileDialog::save(suggested_path) -> Result<FileDialog, i64>`.
- **R1.2.** Add owned, `no_std`-friendly configuration types such as
  `FileDialogOptions`, `FileFilter`, and `FilePlace`, plus an
  `open_with(options)` / `save_with(options)` constructor. Options cover title,
  initial path, commit label, filters, default filter, optional default
  extension, additional places, and initial view.
- **R1.3.** Keep the result `DialogStatus<String>` and `ModalOutcome::Path`.
  This unit remains a single-file picker; do not break all hosts for speculative
  multi-selection or folder-selection results.
- **R1.4.** Store `FileMode` in `FileDialog` and derive title, labels,
  validation, activation, writable-state, and overwrite behavior from it.
- **R1.5.** Options own their strings/vectors. Do not introduce `'static`
  requirements that make app-computed paths or translated labels impossible.

### R2 — Promote only the stable shared browser primitives

- **R2.1.** Move the code-drawn navigation, place, folder/document/application
  icons, clipped/ellipsized text, and byte-size formatting out of File Manager
  into a focused `gui::file_ui` (or equivalently narrow) module.
- **R2.2.** Promote `IconButton` and `BreadcrumbBar` now that both File Manager
  and FileDialog need the same drawing, hit geometry, enabled state, path
  mapping, and left-overflow behavior.
- **R2.3.** Add a lightweight Places sidebar component driven by caller-owned
  `{label, path, icon}` rows. It must not hardcode mount policy inside a generic
  widget.
- **R2.4.** Keep directory navigation, filters, mode validation, overwrite
  state, and single-selection policy in `dialogs`; they are chooser policy, not
  generic GUI behavior.
- **R2.5.** Do not force File Manager onto a single-selection common-dialog
  table/grid abstraction. File Manager needs multi-selection and operations;
  share visual primitives and breadcrumbs/Places while each surface retains
  its honest interaction model.
- **R2.6.** Convert File Manager to the promoted primitives in the same unit.
  Its behavior and palette must remain unchanged. This makes the extraction
  executable proof rather than leaving a second copy behind.

### R3 — Browser model and responsive layout

- **R3.1.** `FileDialog` state includes current directory, back/forward stacks,
  enriched entries, visible indices, selected absolute path, focused control,
  sort key/direction, view mode, scroll position, text filter, active file-type
  choice, Places, last click, status/error, and optional overwrite/new-folder
  substate.
- **R3.2.** Sort folders first with a stable ASCII-case-insensitive name
  comparison and original-name tie-breaker. Size/Type/Modified sorts retain the
  folder-first partition.
- **R3.3.** Selection identity is the absolute path, never a displayed row
  index. Sort, filter, refresh, and view changes cannot redirect Open/Save to a
  different file.
- **R3.4.** Details view shows Name, Size, Type, and Modified when width permits;
  columns collapse in that order at the minimum size. Grid columns respond to
  available width. Both views clamp scrolling after resize/filter/refresh.
- **R3.5.** Breadcrumbs elide from the left when deep paths exceed available
  width while preserving Root and the current folder. Ctrl-L always exposes the
  full normalized path.
- **R3.6.** Resize recomputes all bounds and preserves navigation, selection,
  focus, view, and scroll. Below the supported minimum, layout clamps safely
  rather than underflowing unsigned dimensions.
- **R3.7.** Render only when state changes. Mouse moves without hover/drag state
  do not cause a full-surface present.

### R4 — Correct selection, focus, and validation

- **R4.1.** Replace `ListView`'s second-click activation with dialog-owned real
  double-click tracking. A slow second click simply remains selected.
- **R4.2.** Add explicit focus routing and Tab order for toolbar, location,
  filter, Places, content, filename, file type, commit, and Cancel. Draw a
  visible focus treatment for keyboard users.
- **R4.3.** Open confirms only an existing non-directory target that remains
  permitted by the selected filter. An absolute typed path is supported; a
  relative typed path resolves against the current directory.
- **R4.4.** Save accepts a valid non-empty leaf name or absolute path. Reject
  `.`, `..`, NUL, a trailing `/`, and a directory target. Apply the configured
  default extension only when the user supplied no extension and the active
  filter has one unambiguous default.
- **R4.5.** Stat the final target at confirmation time. Directory contents and
  cached metadata are advisory; the final result must not be based solely on a
  stale row.
- **R4.6.** Confirm is visibly disabled when the current state cannot succeed.
  Clicking a disabled action does nothing; pressing Enter produces at most one
  concise inline explanation.
- **R4.7.** The file-type dropdown is dialog-local until a second non-dialog
  consumer needs a general ComboBox. It supports click, arrows, Home/End,
  Enter, Escape, outside-click dismissal, and edge clamping.

### R5 — Save parity and bounded folder creation

- **R5.1.** Existing-file Save transitions to an in-window
  `ConfirmOverwrite { path }` state with Replace and Cancel choices. Replace
  returns the path; it does not write the file itself.
- **R5.2.** Extract the current mount capability classification from File
  Manager into a small shared filesystem-policy helper. The chooser uses it to
  label `/host` and `/bin` read-only, `/data` persistent with normal ext2
  directory operations, and overlay paths sync-backed. Syscall errors remain
  authoritative.
- **R5.3.** Offer New Folder from the toolbar and Ctrl-Shift-N only where the
  capability model allows directory creation. Use an inline name editor with
  the same leaf-name validation as File Manager.
- **R5.4.** New Folder calls `mkdir`, refreshes and selects/navigates to the new
  folder, and calls `sync` for overlay-backed paths. A sync failure explains
  that the visible change may not survive reboot.
- **R5.5.** Do not add rename, delete, copy/paste, context menus, or recursive
  operations to the picker. Those belong in File Manager.

### R6 — Notepad and reference-client adoption

- **R6.1.** Notepad Open uses a “Text documents” filter for its recognized text
  extensions plus “All files”. The filter limits discovery, not I/O truth;
  selecting an arbitrary UTF-8 file through All files remains supported.
- **R6.2.** Notepad Save supplies a text-file choice and `.txt` default
  extension. Existing document names remain prefilled exactly.
- **R6.3.** Notepad opens the picker at the current document's parent when one
  exists; otherwise it uses its last successful chooser directory, then
  `/host` as fallback. This state stays app-local—no unsafe process-global
  “last folder” cache in the library.
- **R6.4.** Add an Open-dialog launch to `GUIDEMO.ELF` so every common dialog
  has a direct manual smoke path without going through Notepad I/O behavior.
- **R6.5.** Host modality remains unchanged: main windows service
  Resize/Close/Focus but ignore key/mouse while a modal is active; all events
  continue to flow through the host's one outer event loop.

### R7 — Remove the unused kernel chooser and document the boundary

- **R7.1.** Delete `src/window/dialogs/file_open.rs` and `file_save.rs`, their
  `#[allow(dead_code)]` module declarations, and their unused re-exports after a
  final call-site audit.
- **R7.2.** Keep kernel `MessageBox` and Run dialog code; they have live kernel
  callers and are outside this migration.
- **R7.3.** Update `userland/README.md`, root `CLAUDE.md`, and the File Manager
  README/shared-primitive notes. Clearly identify `userland/libs/dialogs` as
  the native-app common-dialog implementation.

## Scope boundaries

### Included

- A polished single-file Open experience and matching Save chrome.
- Local filesystem Places, history, breadcrumbs/location, type and substring
  filtering, sorting, metadata, list/grid views, and refresh.
- Real double-click, complete keyboard routing, responsive resizing, and
  validation that keeps errors inside the dialog.
- Safe overwrite confirmation and capability-aware New Folder.
- Narrow sharing with File Manager where a second consumer now exists.
- Retirement of the unused kernel open/save chooser.

### Deferred

- Multiple selection and folder-picker outcomes.
- Recents/MRU, favorites persistence, tags, saved searches, indexing, and
  recursive search.
- Cloud providers, network locations, removable-device discovery, and a Shell
  namespace beyond real AgenticOS paths.
- Thumbnails, Quick Look/preview panes, image decoding, MIME sniffing, and file
  contents in the picker.
- Drag-and-drop, rename, delete, copy/move, context menus, and creating regular
  files from inside the picker.
- Remembering size/view/folder across processes or reboots.
- A blocking `open_file()` API, kernel-enforced modality, or GUI ABI changes.
- A global desktop/control-theme redesign.

## High-level technical design

### Dependency shape

```text
runtime
  GUI events/timestamps, stat/getdents, mkdir/sync
    |
    +-- gui
    |    existing Window/Canvas/widgets/list_dir
    |    + file_ui: icons, IconButton, BreadcrumbBar, PlacesSidebar,
    |               ellipsis, size formatting, mount capability labels
    |
    +-- dialogs
         FileDialog options + browser/selection/mode state
           |
           +-- notepad / guidemo

fileman -----------------------> gui::file_ui
  keeps its app-specific multi-selection, operations, and browser rendering
```

No `dialogs -> fileman` dependency and no kernel GUI call are introduced.

### Dialog state transitions

```text
Browsing
  + navigation command -> list target -> NavigationSuccess | InlineError
  + select file        -> Selected(path)
  + filter/sort/view   -> rebuild visible projection, reconcile selection
  + Open confirm       -> stat -> Done(path) | InlineError
  + Save confirm       -> stat
       + missing       -> Done(path)
       + regular file  -> ConfirmOverwrite(path)
       + directory     -> navigate/reject according to initiating action
  + New Folder         -> EditingFolderName -> mkdir -> refresh/navigate
  + Escape/Close       -> Done(None)

ConfirmOverwrite(path)
  + Replace            -> Done(path)
  + Cancel/Escape      -> Browsing with prior selection intact
```

The visible rows/tiles are a projection over canonical entries. The selected
absolute path is re-resolved after every projection change and statted again at
commit.

### External design anchors

The options surface deliberately follows current platform concepts without
copying unsupported platform features. Windows exposes suggested folders,
commit labels, file-type choices/filters, persisted picker identity, and view
mode on its current
[FileOpenPicker](https://learn.microsoft.com/en-us/windows/windows-app-sdk/api/winrt/microsoft.windows.storage.pickers.fileopenpicker)
API. Apple's
[NSOpenPanel](https://developer.apple.com/documentation/appkit/nsopenpanel/canchoosedirectories)
separates file, directory, and multiple-selection policy. AgenticOS adopts the
configuration boundary now, but keeps this implementation single-file-only
until a real caller needs a wider result type.

## Implementation units

### U1 — Shared file-browser presentation primitives

Promote icons, ellipsis, size formatting, `IconButton`, `BreadcrumbBar`,
Places, and mount-capability presentation into `gui::file_ui`. Switch File
Manager to those primitives without changing behavior.

Verification:

- Release-check `gui` and File Manager.
- Manually compare File Manager before/after in Details and Grid views at wide
  and minimum sizes; navigation hits, breadcrumbs, icons, and capability status
  remain identical.

### U2 — Configurable FileDialog shell and browser model

Add options/filters/places, retain `FileMode`, replace the fixed layout, load
metadata, and implement responsive toolbar/sidebar/breadcrumb/details/grid/
status/footer rendering. Preserve legacy constructors.

Verification:

- Existing Notepad compiles before its U5 adoption changes.
- Open `/`, `/host`, `/data`, and a directory longer than one page; resize from
  760 x 500 down to 620 x 390 in both views.

### U3 — Navigation, focus, filtering, and Open correctness

Add history, Places, Ctrl-L, Ctrl-F, sorting, type filtering, explicit focus,
Tab order, keyboard selection, path-keyed selection, true double-click, refresh,
and final-target stat validation.

Verification:

- Exercise every shortcut and focus transition using keyboard only.
- Confirm single click never closes; a slow second click never activates; true
  double-click and Enter do.
- Try empty, relative, absolute, missing, directory, filtered-out, and stale
  paths. No invalid Open result escapes.

### U4 — Save parity, overwrite, and New Folder

Implement default extension, writable/capability state, inline overwrite
confirmation, New Folder, refresh, and overlay sync reporting.

Verification:

- Save a new `.txt`, cancel and accept overwrite, attempt a directory target,
  and attempt Save under `/host` and `/bin`.
- Create folders on the overlay and `/data`, reject invalid/colliding names,
  verify the disabled action on read-only mounts, and verify an overlay sync
  error is described honestly.

### U5 — Notepad, GUIDEMO, and dead-kernel-dialog migration

Configure Notepad's filters/start location, add the GUIDEMO Open smoke, remove
the unused kernel file open/save modules, and update documentation.

Verification:

- Notepad Open and Save As return the expected path; cancellation leaves editor
  state untouched; non-UTF-8 and I/O errors still use MessageBox.
- GUIDEMO can launch and dismiss FileDialog repeatedly without leaking windows.
- `rg` confirms no kernel file-open/save symbol or stale documentation remains.

### U6 — Build and manual acceptance matrix

Run:

```sh
cargo fmt --manifest-path userland/Cargo.toml --all
cargo check --release --manifest-path userland/Cargo.toml -p gui -p dialogs -p fileman -p notepad -p guidemo
./build.sh -n
./test.sh --skip-userland
```

Then boot both:

```sh
AGENTICOS_THEME=aero ./build.sh
AGENTICOS_THEME=classic ./build.sh
```

Manual matrix: mouse and keyboard-only; Details and Grid; `/`, `/root`,
`/data`, `/host`, `/bin`; empty and long directories; long names/deep paths;
filter changes; back/forward branches; resize; invalid navigation; Open; new
Save; overwrite accept/cancel; read-only Save; New Folder; window Close and
Escape. Because the repository has no automated ring-3 widget test harness,
do not claim pixel/interaction unit coverage that does not exist; the release
build, kernel regression suite, GUIDEMO reference path, and explicit QEMU matrix
are the acceptance evidence for this unit.

Sequencing is strict: U1 -> U2 -> U3 -> U4 -> U5 -> U6. Each unit must leave
the userland workspace buildable; the compatibility constructors keep Notepad
working while the new dialog is assembled behind them.

## Key technical decisions

### KTD1 — Modernize the native common dialog, remove the unused kernel picker

Native apps already use `userland/libs/dialogs`; the kernel file open/save
modules have zero callers and weaker behavior. Maintaining both would guarantee
visual and semantic drift. Kernel MessageBox/Run remain because they have live
kernel responsibilities.

### KTD2 — Preserve the retained modal API

One process has one GUI event queue. A nested blocking loop would consume or
drop host-window events. `handle_event -> DialogStatus` remains the correct ABI
shape, and overwrite stays within the same picker window.

### KTD3 — Share presentation primitives, not a compromised browser widget

The common dialog needs single selection and strict commit validation; File
Manager needs multi-selection, mutation, context actions, and child launching.
Icons, breadcrumbs, Places, ellipsis, and capability labels are stable shared
concepts. A monolithic shared browser would either leak File Manager policy into
dialogs or weaken File Manager.

### KTD4 — Configuration now, broader result types later

File filters, places, titles, initial view, and default extensions are useful to
current callers and fit owned options. Multiple-file and folder results would
break `DialogStatus<String>`/`ModalOutcome::Path` and have no current consumer,
so they are deliberately deferred.

### KTD5 — No fake search, Recents, or preview

AgenticOS has no index, MRU service, thumbnails, content decoders, or provider
namespace. The UI says “Filter this folder” and shows metadata the filesystem
actually provides. A smaller honest picker is more modern than controls that
promise unavailable behavior.

## Risks and mitigations

### RK-1 — Promotion regresses File Manager

The existing code is app-local and tightly coupled to layout constants.
Mitigation: U1 moves only primitives with two consumers, switches File Manager
first, and has a before/after manual parity gate before FileDialog uses them.

### RK-2 — Full-surface presents become expensive at 760 x 500

Ring-3 windows present whole XRGB surfaces. Mitigation: the picker is
event-driven, presents only on visual state changes, ignores irrelevant mouse
moves, and does no animation. The surface is still smaller than File Manager's
already-qualified 920 x 580 client.

### RK-3 — Long directories pause listing

`gui::list_dir` stats each entry synchronously. Typical boot/host directories
are small, but a very large directory can stall. Mitigation: record the issue
and test a realistically long folder. Incremental directory scanning belongs in
`gui::list_dir`/a filesystem job API and is a follow-up unless the acceptance
test shows close/resize latency is materially poor; do not build a dialog-only
half-async scanner.

### RK-4 — Save capability labels can become stale as filesystems improve

The topology model reflects today's documented mounts, while syscalls remain
authoritative. Keep capability classification in one shared helper used by File
Manager and FileDialog, and update that helper when FAT directory mutation or
new mounts land.

### RK-5 — Classic theme still looks intentionally classic

The feature modernizes information architecture in both themes but does not
override the user's boot-selected control style. Acceptance must judge behavior
and hierarchy separately from Classic bevel styling; Aero is the visual target
for the modern-light reference.

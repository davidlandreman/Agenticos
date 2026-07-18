---
title: "feat: modern standalone ring-3 file manager"
type: feat
status: completed
date: 2026-07-18
---

# feat: modern standalone ring-3 file manager

## Summary

Replace the remaining kernel-side `explorer` with a standalone native
`FILEMAN.ELF`: a modern, graphical file manager whose information architecture
is immediately familiar to users of Finder and Windows Explorer. It should be
pleasant for everyday browsing, not merely a directory-list demo.

## Implementation Outcome

Completed on 2026-07-18. The delivered `FILEMAN.ELF` is a 92 KB standalone
ring-3 application with history, Places, breadcrumbs/address entry, details and
grid views, sorting, filtering, path-keyed multi-selection, context actions,
real double-click timing, filesystem-aware capabilities, file operations,
Notepad/ELF launching, child reaping, and dynamic titles.

The reusable boundary was kept intentionally narrow during implementation:
filesystem metadata, process/filesystem wrappers, event metadata, and window
title support live in the shared runtime/toolkit, while controls used only by
File Manager remain app-local until a second consumer establishes their stable
API. The legacy kernel Explorer was removed and both Start → Programs → File
Manager and `/bin/explorer [path]` now launch the standalone ELF.

Verification completed with `cargo check`, `cargo check --features test`, the
release userland build, `./build.sh -n`, targeted GUI/bin/start-menu tests, and
the full QEMU suite (770 tests).

The app keeps the existing `explorer` command for compatibility, gains a pinned
Start -> Programs -> File Manager entry, and opens as an ordinary ring-3 GUI
process. Its first complete release includes:

- Back, Forward, Up, Home, breadcrumb navigation, and an editable location bar.
- A Places sidebar for Home, Root, Data, and Host.
- Details and icon-grid views with file-type icons, sorting, filtering,
  scrolling, keyboard navigation, multi-selection, and real double-click.
- Familiar operations: New Folder, Rename, Delete, Cut, Copy, Paste, Refresh,
  and properties/status information, constrained honestly by each mount's
  capabilities.
- Opening text-like files in `NOTEPAD.ELF` and executing `.ELF` files without
  replacing the file-manager process.
- Clear read-only and persistence behavior: `/host` and `/bin` are read-only,
  `/data` supports persistent file writes but not directory mutation, and root
  overlay mutations are synced automatically with visible failure reporting.

This is a ring-3 application migration, not a new kernel-owned widget tree. The
only kernel GUI additions are two small platform affordances needed for a
familiar application: changing a live window title and timestamping mouse-button
events so double-click is real rather than "click the selected row again."

---

## Problem Frame

AgenticOS already has a visually substantial kernel-side explorer under
`src/commands/explorer/`: toolbar, path bar, directory tree, multi-column list,
status bar, folder-first sorting, and extension dispatch. It is nevertheless the
wrong long-term application shape:

1. It runs in ring 0, stores per-instance state in a global map, and spin/yield
   polls for callbacks. A file browser should not have kernel privilege or a
   bespoke callback registry.
2. It uses kernel widgets unavailable to native applications. Keeping it would
   preserve two GUI stacks and make future file-manager features kernel work.
3. Its interaction model is a proof of concept: no history, breadcrumbs,
   filtering, multi-selection, icon view, context actions, copy/move, rename,
   delete, or folder creation.
4. "Double-click" is currently a second click on an already-selected row because
   `GuiEvent` carries no button-event timestamp.
5. Opening a file is kernel policy. Text dispatch directly calls the kernel's
   user-app launcher and `.ELF` dispatch launches from a kernel thread. A native
   file manager instead needs a small, reusable userland `fork` + `execve` helper.

The platform now has the prerequisites the earlier explorer did not: four
ring-3 GUI syscalls, a retained-mode userland toolkit, shared userland dialogs,
multi-process scheduling, `fork`/`execve`/`wait4`, writable overlay root,
persistent `/data`, and long-name-aware directory enumeration.

### Filesystem reality the UI must expose

The file manager must not imply capabilities that the backing filesystem does
not have:

| Namespace | Read | Create/write file | Unlink file | mkdir/rmdir/rename | Persistence |
|---|:---:|:---:|:---:|:---:|---|
| `/` overlay | Yes | Yes | Yes | Yes | After `sync(2)` |
| `/data` FAT32 | Yes | Yes | Yes | No (deferred FAT work) | Immediate |
| `/host` vvfat | Yes | No | No | No | Host-owned/read-only |
| `/bin` synthetic | Yes | No | No | No | Kernel-owned/read-only |

Actions are enabled from an explicit capability model and still handle syscall
errors as authoritative. Unsupported actions never silently disappear: their
disabled state or resulting message explains why.

---

## Product Experience

### Primary layout

Initial client size is approximately 920 x 600, resizable down to 680 x 420.
The system-owned frame remains themed by AgenticOS; the client surface uses a
modern light application theme with generous spacing, thin separators, one blue
accent, and code-drawn icons.

```text
+ File Manager - /host -----------------------------------------------+
| [Back] [Forward] [Up] [Home]   Root > host     [Filter this folder] |
|---------------------------------------------------------------------|
| PLACES       | Name                  Size       Type       Modified  |
| Home         |------------------------------------------------------|
| Root         | [folder] docs         --         Folder     --        |
| Data         | [text]   README.md     7.1 KB     MD file    --        |
| Host         | [app]    NOTEPAD.ELF  31 KB      Application --       |
|              |                                                      |
|              |                                                      |
|              |                                                      |
|              |------------------------------------------------------|
|              | 3 items | 1 selected | /host is read-only   [List][Grid]
+---------------------------------------------------------------------+
```

The symbols above describe controls, not text glyphs. The app draws crisp
navigation, folder, document, executable, storage, lock, list, and grid icons
with `Canvas` primitives so the visual does not depend on unsupported Unicode
font glyphs or external bitmap assets.

### Interaction contract

- Single click selects. Ctrl-click toggles selection. Shift-click selects a
  range. Clicking empty space clears selection.
- Double-click or Enter activates. A folder navigates; a recognized file opens.
- Backspace navigates up; Alt-Left/Alt-Right traverse history; Ctrl-L edits the
  full location; Ctrl-F focuses the folder filter; F5 refreshes.
- F2 starts inline rename. Delete opens a permanent-delete confirmation. There
  is no fake Trash or Recycle Bin.
- Ctrl-Shift-N creates a folder through an inline name field. Ctrl-C/X/V use an
  app-local file-operation clipboard. Cross-application clipboard integration is
  explicitly deferred.
- Right-click selects the target if necessary and opens an in-canvas context
  menu with Open, Cut, Copy, Rename, Delete, and Properties. Disabled actions
  remain visible with muted labels.
- Details view has sortable Name, Size, Type, and Modified columns. Icon view
  presents responsive tiles. Both share one selection model and scroll offset.
- The filter is deliberately current-folder substring filtering, not a recursive
  search falsely presented as a complete search service.
- Loading, copying, and syncing show progress in the status area. Close remains
  responsive while an operation is in progress and asks before abandoning an
  incomplete copy.

### Visual tokens

Keep the palette app-local for this unit; do not retheme the desktop:

| Role | Value |
|---|---|
| App background | `#F6F8FB` |
| Surface / content | `#FFFFFF` |
| Primary text | `#20242C` |
| Secondary text | `#687386` |
| Divider / border | `#D9E0EA` |
| Accent | `#2F73DA` |
| Selection | `#DCEBFF` with accent outline |
| Folder | `#E9B949` |
| Destructive | `#C83A3A` |

The existing 8 x 8 bitmap font stays the text source. Modernity comes from
hierarchy, alignment, spacing, restrained color, icons, and interaction quality;
this plan does not introduce an unrelated font-rendering project.

---

## Requirements

### R1 - Standalone application and state model

- **R1.1.** Add `userland/apps/fileman/` as a no_std Rust workspace package
  depending on `runtime`, `gui`, and `dialogs`. It is built every run and staged
  as uppercase-8.3-safe `FILEMAN.ELF` through one manifest row.
- **R1.2.** The app owns ordinary per-process state: current directory,
  back/forward stacks, entries, selection, focus target, sort, view mode,
  filter, modal/context-menu state, clipboard, and optional background job.
  There are no globals and no polling kernel process.
- **R1.3.** Startup accepts an optional path in `argv[1]`. With no argument it
  tries `$HOME`, then falls back to `/` if Home is missing or unreadable.
- **R1.4.** One event loop routes main-window and dialog events by window handle,
  follows the existing `dialogs::Modal` pattern, and presents only when visual
  state changed. Mouse moves alone do not trigger full-surface copies.
- **R1.5.** Pure model logic lives behind a library target so navigation-history,
  sorting, filtering, selection, path joining, name validation, size formatting,
  and conflict naming can be host-unit-tested without booting QEMU.

### R2 - Userland GUI toolkit growth

Grow `userland/libs/gui` only with pieces directly consumed by the app:

- **R2.1 `Theme` and geometry helpers.** App-selectable colors plus text
  measurement/ellipsis, clipping helpers, rounded rectangles, dividers, and the
  small code-drawn icon set. Existing apps keep their current defaults.
- **R2.2 `IconButton`.** Icon, bounds, enabled/pressed state, tooltip text,
  draw, and hit test. The file manager owns tooltip timing; no kernel tooltip.
- **R2.3 `BreadcrumbBar`.** A sequence of path components with individually
  clickable segments, overflow elision from the left, and a Ctrl-L editable
  location-field mode using `TextField`.
- **R2.4 `Sidebar`.** Grouped place rows with icon, label, target path,
  selected state, wheel scrolling, and keyboard movement.
- **R2.5 `TableView`.** Multiple columns, header hit testing, resize-safe column
  widths, vertical scrolling, shared multi-selection, focused row, and row
  activation. Header click returns the requested sort key/direction.
- **R2.6 `IconGrid`.** Responsive tile layout over the same item model,
  icon/label painting, scrolling, arrow-key spatial navigation, selection, and
  activation.
- **R2.7 `PopupMenu`.** In-canvas context menu with enabled/disabled entries,
  separators, accelerators, outside-click dismissal, and edge clamping.
- **R2.8.** Generalize `TextField` with placeholder text, focus outline, and a
  caller-supplied validator hook/result. Inline rename and New Folder reuse it.

`TableView` and `IconGrid` must not own file-manager-specific entries. They
consume lightweight row/tile display data and return indices/actions, preserving
the toolkit/application layering established by `ListView` and dialogs.

### R3 - Directory and metadata model

- **R3.1.** Extend `gui::list_dir` (or move the richer API into a focused
  `gui::fs` module) to preserve inode/type from `getdents64` and obtain size,
  mode, and modified time with a new `runtime::newfstatat` wrapper. Avoid the
  current open-every-entry fallback when the directory type is known.
- **R3.2.** App `FileEntry` fields: name, absolute path, kind (folder,
  executable, text, image, archive, generic), size, modified timestamp, and
  capability flags. Type derives from directory mode first, then case-insensitive
  extension; it is presentation/dispatch metadata, not a MIME claim.
- **R3.3.** Folder-first stable sorting with Name, Size, Type, and Modified keys;
  ascending/descending toggle; ASCII-case-insensitive comparison with original
  name as deterministic tie-breaker.
- **R3.4.** Current-folder filtering is case-insensitive and updates both views
  without rereading the directory. Selection remains keyed by path, so sorting
  or filtering does not accidentally apply an operation to a different row.
- **R3.5.** Directory reads and per-entry stat enrichment use a small incremental
  job pump. Names may appear first and metadata fill in afterward; directories
  with many entries must not freeze close/navigation for seconds.
- **R3.6.** Refresh keeps selection for paths still present, drops vanished
  paths, and reports read failures through `MessageBox::error` while retaining
  the last successful view.

### R4 - Navigation and familiar controls

- **R4.1.** Back/Forward use bounded stacks (64 entries), avoid duplicate
  adjacent history entries, and clear Forward after a new branch. Up derives a
  normalized parent and disables at `/`.
- **R4.2.** Breadcrumb segments navigate without corrupting history. Ctrl-L
  shows the normalized absolute path; Enter navigates, Escape restores crumbs.
  Relative location input resolves against the current directory.
- **R4.3.** Places are Home (only when valid), Root, Data (only when mounted),
  and Host (only when mounted). Unavailable places remain omitted rather than
  navigating to guaranteed errors.
- **R4.4.** Details/icon choice persists for the process lifetime and can be
  changed through toolbar buttons and View-menu/context actions.
- **R4.5.** Status shows item count, selected count/bytes, current path
  capability (`Read-only`, `Persistent`, or `Sync-backed`), and active job
  progress/error.
- **R4.6.** Window resize recomputes layout, preserves scroll/selection, and
  collapses nonessential status text before reducing the main viewport.
- **R4.7.** Properties opens a read-only `MessageBox`-style panel for one
  selected item showing name, full path, kind, size, modified time, mode, and
  current mount capability. Multiple selection shows aggregate count/size.

### R5 - Real double-click and dynamic title

- **R5.1.** Use unused payload slots on mouse button events compatibly:
  `payload[4..=5]` carry the current 64-bit 100 Hz monotonic tick for
  `GUI_MOUSE_DOWN`/`GUI_MOUSE_UP`. Scroll events retain their existing delta
  fields. The 32-byte `GuiEvent` layout and `GUI_ABI_VERSION = 1` remain intact.
- **R5.2.** Add GUI syscall 5005,
  `gui_win_set_title(handle, title_ptr, title_len)`, enforcing calling-PID
  ownership and the same bounded UTF-8 validation used at create. Add
  `FrameWindow::set_title`, runtime wrapper, and `gui::Window::set_title`.
- **R5.3.** The app recognizes a double-click only when two primary-button downs
  hit the same item within 50 ticks and within a small movement threshold.
  Selection is still immediate on the first click.
- **R5.4.** Successful navigation sets the frame title to
  `<folder name> - File Manager` (root uses `/ - File Manager`) and keeps the
  breadcrumb as the full-path authority.

### R6 - File operations

- **R6.1 Runtime wrappers.** Add safe no_std wrappers/constants for `rmdir`,
  `newfstatat`, `access`, `fork`, `execve`, `wait4`, `sync`, and the new title
  syscall. Existing kernel handlers are reused; no new filesystem syscall ABI
  is needed.
- **R6.2 Capability model.** Classify the current mount using the documented
  topology, disable impossible actions, and translate negative errno values into
  specific messages. Syscall results remain authoritative if topology changes.
- **R6.3 New Folder.** Ctrl-Shift-N starts inline naming. Validate non-empty,
  not `.`/`..`, no `/` or NUL, and no collision before `mkdir`. This is enabled
  on overlay root and disabled on `/data`, `/host`, and `/bin` until FAT
  directory mutation lands.
- **R6.4 Rename.** F2 edits exactly one selected item in place, preserving an
  extension-aware selection range where practical. Commit calls `rename`; Esc
  cancels. Disabled for read-only and `/data` paths under current capabilities.
- **R6.5 Delete.** A `MessageBox` lists the item/count and says deletion is
  permanent. Files use `unlink`; empty directories use `rmdir`; mixed
  multi-selection processes the selected entries one by one only after explicit
  confirmation. No recursive folder deletion in v1.
- **R6.6 Internal Cut/Copy/Paste.** Clipboard entries are absolute source paths
  plus Cut/Copy intent. Paste copies regular files with a bounded 32 KiB buffer.
  Move first tries `rename`; on `-EXDEV` for a regular file it copies, closes,
  verifies final size, then unlinks the source. Directories are not recursively
  copied/moved in v1 and receive an explicit explanation.
- **R6.7 Conflicts and failure atomicity.** Never overwrite silently. Write a
  destination to a sibling temporary name, close it, then rename into place only
  where rename is supported. On `/data`, where rename is unavailable, require a
  non-existing final target and remove a partial target on failure. A conflict
  stops that item and offers a safe generated `"name copy.ext"` destination on
  the overlay or an 8.3-safe suffixed name on `/data`.
- **R6.8 Responsive jobs.** Copy and sync run as incremental app state machines:
  drain pending GUI events nonblocking, process one bounded chunk, update status
  at a throttled cadence, and honor cancel/close. Do not fork a worker that would
  need to share mutable GUI state.
- **R6.9 Persistence.** After a successful mutating batch under overlay `/`, call
  `sync(2)` automatically. If sync fails, report that the visible in-memory
  change succeeded but may not survive reboot. `/data` file mutations are
  already immediate and do not need overlay sync.

### R7 - Opening files and child lifecycle

- **R7.1.** Add a runtime helper that builds NUL-terminated argv/envp arrays,
  calls `fork`, `execve`s immediately in the child, exits 126 on exec failure,
  and returns the child PID to the parent. It must not allocate or hold an
  allocator lock between child-side fork return and `execve`.
- **R7.2.** The file manager retains its startup environment and reaps children
  with `wait4(..., WNOHANG, ...)`. `gui_next_event` interrupted by `SIGCHLD`
  triggers a reap-and-retry path, so child exits do not accumulate zombies.
- **R7.3.** Text-like extensions (`txt`, `md`, `rs`, `toml`, `json`, `log`,
  `conf`, `sh`, `c`, `h`, `cpp`) spawn `/bin/notepad <path>`.
- **R7.4.** `.ELF` activation execs the selected absolute path as `argv[0]` after
  an executable-mode/access check. Launch failure is reported in the parent;
  because exec failure happens in the child, use a small close-on-exec status
  pipe if the existing pipe/fcntl surface supports it, otherwise report the
  child's 126 status on reap with the attempted path retained in the child map.
- **R7.5.** Unknown file types open a concise "No application is registered"
  message and still expose Properties. Do not guess that arbitrary files are
  text.

### R8 - Migration and desktop integration

- **R8.1.** Add `FILEMAN_HOST_PATH` and move `explorer` from `GUI_APPLETS` to
  sorted `DIRECT_APPLETS`. `/bin/explorer`, `stat`, `access`, and `/bin`
  enumeration keep working but rewrite directly to `/host/FILEMAN.ELF`.
- **R8.2.** Add Start -> Programs -> File Manager using
  `spawn_gui_user_app("/host/FILEMAN.ELF", ["explorer"])`.
- **R8.3.** Remove the `explorer` arm from `gui_launch_table`, update its
  synchronization tests, delete `src/commands/explorer/`, and remove the module
  registration. `tasks` remains the last kernel GUI app behind `GLAUNCH.ELF`.
- **R8.4.** Multiple launches create independent processes/windows. Closing or
  crashing one instance destroys only its PID-owned windows and leaves child
  applications and other manager instances intact.
- **R8.5.** Update root, commands, userland, and window documentation plus the
  userland app manifest/layout. Mark this plan completed only after the manual
  acceptance pass.

---

## Scope Boundaries

### Included now

- High-quality local browsing in two view modes.
- Navigation history, places, breadcrumbs/location entry, sorting, and
  current-folder filtering.
- Multi-selection and familiar keyboard/mouse interactions.
- Safe, capability-aware file/folder mutation available on today's mounts.
- App-local cut/copy/paste for regular files.
- Text-file and ELF launching through normal userland process primitives.
- Full replacement of the kernel-side explorer.

### Explicitly deferred

- Trash/Recycle Bin, undo history, and recoverable delete.
- Recursive search, indexing, saved searches, tags, favorites persistence, and
  filesystem change notifications.
- Thumbnails, previews, image decoding, media metadata, and MIME detection.
- Drag-and-drop within or between windows; cross-application clipboard.
- Recursive directory copy/move/delete. Add it after cancellation, conflict,
  rollback, and `/data` directory mutations have a trustworthy contract.
- Editing permissions/ownership, symlink creation, archive browsing, network
  mounts, and tabs.
- New FAT directory mutation. The app will automatically enable more actions on
  `/data` once that filesystem work lands, but this plan does not smuggle it in.
- A global modern desktop theme. Only the app client is styled here.

---

## High-Level Technical Design

### Crate and module shape

```text
runtime
  process wrappers: fork / execve / wait4
  fs wrappers: newfstatat / rmdir / sync
  GUI 5005 wrapper + mouse timestamp constants
    |
    +-- gui
    |    Theme, icons, IconButton, BreadcrumbBar, Sidebar,
    |    TableView, IconGrid, PopupMenu, richer TextField
    |
    +-- dialogs
         existing FileDialog / MessageBox / ColorPicker
              |
              +-- fileman (FILEMAN.ELF)
                   main.rs        startup + event/job pump
                   app.rs         state machine and commands
                   model.rs       entries, history, selection, sort/filter
                   filesystem.rs  listing, metadata, capabilities
                   operations.rs  copy/move/delete/sync jobs
                   launch.rs      child spawn/reap and extension dispatch
                   render.rs      responsive layout and client theme
```

The package should expose its pure `model` pieces through `src/lib.rs` for host
tests while `src/main.rs` remains the no_std/no_main ELF entry point.

### State transitions

```text
GUI event
  -> route by window handle
  -> translate hit/key into AppCommand
  -> mutate AppState or start Job
  -> if navigation: start incremental DirectoryScan
  -> if operation: advance one bounded Job step
  -> if state is visually dirty: render once + full-surface present
  -> block in gui_next_event when idle

Active Job
  -> try_next_event (close/cancel/resize remain responsive)
  -> process <= 32 KiB or one metadata entry
  -> present progress only when percentage/text changes
  -> nanosleep briefly when neither input nor useful work is ready
```

### Selection identity

Never store an operation target as a displayed row number. The canonical
selection is an ordered set of absolute paths plus one focused/anchor path. A
view builds transient path-to-index mappings after sort/filter. This prevents a
refresh or late metadata sort from redirecting Delete/Rename to the wrong file.

### Child launch flow

```text
activate README.md
  -> spawn_exec("/bin/notepad", ["notepad", "/host/README.md"], envp)
       -> fork()
          parent: records pid -> attempted launch; keeps GUI loop running
          child:  execve immediately; on failure exit(126)
  -> SIGCHLD interrupts/reawakens parent
  -> wait4(WNOHANG) reaps and reports abnormal/126 exit when appropriate
```

No custom "open file" kernel syscall is introduced. The app exercises the same
process model available to zsh and future native applications.

---

## Implementation Units

### U1. Platform affordances and runtime wrappers

Add mouse-button timestamps, GUI syscall 5005, `FrameWindow::set_title`, kernel
tests, runtime syscall wrappers/constants (including F2/F5), safe C-string/argv
builders, and `gui::Window::set_title`.

Verification:

- GUI event encoding test pins timestamp placement without changing struct size.
- Title syscall tests cover success, bad pointer, bad handle, and wrong PID.
- `cargo build --release --manifest-path userland/Cargo.toml` remains green.

### U2. Demand-driven toolkit controls

Implement theme/icon primitives, `IconButton`, `BreadcrumbBar`, `Sidebar`,
`TableView`, `IconGrid`, `PopupMenu`, and `TextField` improvements. Keep existing
widget rendering unchanged unless a caller opts into the new theme.

Verification:

- Host tests cover widget hit geometry, table header/row mapping, grid spatial
  navigation, breadcrumb overflow, popup edge clamping, and selection events.
- Add a temporary or permanent `GUIDEMO.ELF` showcase screen for manual resize,
  scroll, keyboard, and right-click smoke; remove temporary hooks before U7 if
  they make the minimal demo confusing.

### U3. File-manager shell and read-only browsing

Create the app/package/manifest row and implement responsive layout, Places,
directory scan, metadata model, history, breadcrumbs/location entry, details
view, icon view, sorting, filtering, status, selection, activation of folders,
resize, close, and dynamic title. No mutation yet.

Verification:

- Launch `/host/FILEMAN.ELF`, browse `/`, `/host`, `/data`, and a long directory.
- Back/Forward branch behavior, Ctrl-L, Ctrl-F, F5, double-click, Enter,
  multi-selection, both views, sort directions, and resize all work.
- Invalid initial paths and unreadable navigation preserve the last good view and
  show a userland message box.

### U4. Child spawning and file opening

Add fork/exec/wait helpers, child bookkeeping/reaping, text extension dispatch,
ELF execution, and unknown-type errors. The file-manager window must remain live
while Notepad or another ELF runs.

Verification:

- Open two text files into separate Notepad processes.
- Launch a valid ELF, a malformed/non-executable file renamed `.ELF`, and a
  missing path; failures do not kill or hang the manager.
- Close children in different orders and confirm no zombie records remain.

### U5. Namespace mutations

Implement capability-aware New Folder, inline Rename, permanent Delete, error
mapping, selection refresh, and root-overlay auto-sync. Keep unsupported `/data`
directory actions and all `/host` mutations visibly disabled.

Verification:

- Under `/`: create folder, rename it, create/select/delete a file, sync, reboot,
  and confirm the result persisted.
- Under `/data`: file delete succeeds; New Folder and Rename explain the current
  FAT limitation.
- Under `/host` and `/bin`: all mutation actions are disabled and direct syscall
  attempts still surface read-only/permission errors.

### U6. Copy/cut/paste job engine

Implement app-local clipboard, regular-file copy, safe move fallback, bounded
incremental I/O, conflict naming, cancellation, progress, partial-output cleanup,
and post-operation refresh/sync.

Verification:

- Copy small and multi-MiB files from `/host` to `/`, `/host` to `/data`, and
  within `/`; byte counts and content match.
- Cut a regular file within overlay (rename path) and across mounts
  (copy-verify-unlink path).
- Cancel mid-copy, fill a destination to `ENOSPC`, close mid-job, and collide
  with an existing name; no silent overwrite or orphan temp file remains.

### U7. Replace kernel explorer and finish integration

Wire `/bin/explorer` directly to `FILEMAN.ELF`, add the Start-menu item, delete
the kernel explorer, update launch-table/bin-namespace tests, docs, and plan
status.

Verification:

- Start -> Programs -> File Manager and `explorer /host` in zsh both launch the
  standalone app with the expected starting directory.
- Two manager instances coexist; closing one does not affect the other.
- `./build.sh -n`, targeted GUI/filesystem/bin-namespace tests, and full
  `./test.sh` pass.

Sequencing: U1 -> U2 -> U3 -> U4 -> U5 -> U6 -> U7. U3 is the first user-visible
milestone; U5 is the first mutation-capable release. Each unit should remain
buildable and reviewable without leaving `/bin/explorer` broken.

---

## Key Technical Decisions

### KTD1. `FILEMAN.ELF` replaces explorer; it does not coexist indefinitely

The ring-3 GUI platform exists to remove application policy from the kernel.
Keeping both implementations would immediately create behavior and bug-fix
drift. The direct `/bin/explorer` rewrite preserves the user-facing contract.

### KTD2. Finder/Explorer familiarity is interaction architecture, not imitation

The app borrows the durable shared grammar (Places, navigation history,
breadcrumbs, details/grid views, inline rename, context actions) without copying
either product's branding or fighting the OS-owned frame theme.

### KTD3. One selection model serves both views

Details and grid views are projections of the same path-keyed state. Switching
views never loses or retargets selection and file operations stay independent of
row layout.

### KTD4. Mouse timestamps extend unused payload, not the ABI struct

Real double-click materially improves familiarity. Reusing button-event payload
slots is backward compatible, keeps `GuiEvent` at 32 bytes, and avoids a new
event ABI version solely for one app.

### KTD5. Opening apps uses fork/exec, not a file-manager kernel syscall

The process platform already supports the Linux primitives. A reusable runtime
helper benefits every future native launcher and keeps extension policy in the
app. Immediate child-side exec also minimizes post-fork allocator risk.

### KTD6. Operations reflect filesystem capability instead of simulating parity

Read-only host and incomplete FAT directory mutation are system truths. Disabled
actions and precise errors are better than controls that fail unpredictably or
an app-private virtual filesystem illusion.

### KTD7. No recursive destructive operations in v1

Recursive copy/delete requires a much larger failure and rollback contract,
especially across the volatile overlay and partially writable FAT mount. Files
and empty directories cover the safe useful subset; recursion follows after the
job engine has proven cancellation and conflict handling.

### KTD8. Background work is an incremental state machine, not a shared-memory worker

The app has one GUI event queue and no thread runtime. A bounded stepper keeps
the single-owner state model, remains responsive, and avoids synchronizing GUI
state across forked address spaces.

---

## Risks and Mitigations

### RK-1. Full-surface presents become expensive at file-manager size

A 920 x 600 XRGB surface is about 2.1 MiB per present. Mitigation: render only
after visual state changes, avoid hover repaint on every mouse move, coalesce job
progress, and keep large directory metadata incremental. Damage rectangles or
shared surfaces remain platform follow-ups if profiling shows a real bottleneck.

### RK-2. Per-entry stat is slow on FAT/PIO

Names render immediately; metadata enriches incrementally and close/navigation
can cancel the scan. Known directory types from `getdents64` avoid unnecessary
open/stat probes. The UI shows a loading state instead of appearing hung.

### RK-3. Fork from a Rust allocator-backed app is fragile

The child calls `execve` immediately using buffers fully prepared before fork,
does not allocate, does not present GUI, and exits directly on failure. Runtime
documentation makes this helper the only supported native-app spawn path.

### RK-4. Copy cancellation leaves partial files

Overlay destinations use a sibling temp and rename-on-complete. `/data` lacks
rename, so a new final target is used only after collision checks and is unlinked
on failure/cancel. Errors report cleanup failure separately if it occurs.

### RK-5. Automatic root sync can be slow or fail

Sync runs through the responsive job state, status distinguishes "changed in
memory" from "saved," and failure never claims persistence. Direct `/data`
writes skip the overlay sync path.

### RK-6. Toolkit scope expands into a second desktop framework

Every new control in R2 has an immediate file-manager consumer and a narrow
event/result API. Layout policy, file icons, operations, and application commands
remain in `fileman`; the generic toolkit does not learn about paths or files.

---

## Acceptance Criteria

- `FILEMAN.ELF` is a standalone ET_EXEC x86-64 binary staged by the manifest.
- Start -> Programs -> File Manager and `/bin/explorer [path]` launch it.
- The kernel-side explorer module and dispatch arm are gone.
- Browsing `/`, `/host`, and `/data` supports responsive navigation, history,
  breadcrumbs/location entry, Places, details/grid views, sort, filter,
  multi-selection, keyboard controls, right-click, and true double-click.
- Text files open in Notepad; valid ELFs launch concurrently; failed launches do
  not terminate the manager; children are reaped.
- Supported New Folder/Rename/Delete/Cut/Copy/Paste operations work and
  unsupported mount actions are visibly disabled with accurate explanations.
- Overlay changes are synced and survive a reboot; sync failure is not hidden.
- Multiple instances and child apps coexist without leaked PID-owned windows.
- Host model/widget tests, kernel GUI/bin-namespace tests, `./build.sh -n`, and
  full `./test.sh` pass.

---

## System-Wide Impact

- **Userland:** one new built-every-run app and reusable runtime process/fs
  wrappers; GUI toolkit gains the first serious navigation/data-view controls.
- **Kernel GUI ABI:** one ownership-checked title syscall plus compatible mouse
  timestamp metadata; no new file operation ABI.
- **Kernel commands:** explorer removed; only Tasks remains behind the legacy
  GUI launcher.
- **Desktop:** one pinned File Manager program entry; no global theme change.
- **Filesystem:** no implementation changes, but current mount capabilities become
  explicit product behavior and receive an end-to-end graphical client.
- **Documentation/testing:** userland app guide, subsystem guides, bin namespace
  invariants, GUI ABI docs, and plan status updated together.

## Origin

Requested 2026-07-18: "Plan to build a ELF file manager style app. Make it
modern, graphical, and useful and feel familiar to modern finder / windows
explorer." Scoped against the existing kernel explorer, completed ring-3 GUI
platform, common dialogs library, current filesystem mount topology, and
multi-process userland support.

---
title: "feat: modern CONTROL.ELF system settings and live personalization"
type: feat
status: completed
date: 2026-07-18
---

# feat: modern CONTROL.ELF system settings and live personalization

## Implementation outcome

Completed on 2026-07-18. The Start-menu Settings row and the `control` and
`settings` command aliases now launch the standalone ring-3 `CONTROL.ELF`.
Its responsive Control Center UI provides searchable Home, Appearance,
Desktop, System, Network, and About pages backed by live system state.

The versioned system-control syscall owns persistent theme and wallpaper
preferences in `/data/agenticos/settings.conf`. Theme changes update existing
kernel frames and themed ring-3 applications without changing client size;
wallpaper changes validate BMP input before replacing the live desktop.
Process-global, coalesced theme/settings events keep multiple Settings windows
and open applications synchronized, while renderer fallback republishes a
consistent Classic state.

Verification completed with clean kernel and userland `cargo check` runs, a
successful release `./build.sh -n` that staged `CONTROL.ELF`, and the complete
QEMU `./test.sh` suite: all 855 tests passed. Strict project-wide Clippy remains
blocked by pre-existing legacy lints and no-std test-target errors outside this
feature's changes.

## Summary

Add a standalone ring-3 `CONTROL.ELF` application titled **Settings** and make
the existing root Start-menu Settings row launch it. The application uses a
modern Control Center layout inspired by Windows 11 Settings and macOS System
Settings: searchable navigation on the left, a spacious page header, rounded
cards, concise secondary text, code-drawn icons, and immediate controls with
clear current-state feedback.

The first release must be useful rather than a collection of placeholders:

- **Appearance** switches between Automatic, Classic, and Aero at runtime and
  persists the preference. Kernel chrome, kernel controls, open ring-3 apps,
  and `/etc/theme` converge on the new effective theme without restarting.
- **Desktop** chooses a BMP wallpaper or restores the bundled default, applies
  it live, and persists the selected path.
- **Home, System, Network, and About** summarize real state already exposed by
  the kernel: active renderer, display size, uptime, memory, interface traffic,
  resolver configuration, architecture, and personalization choices.

There are no inert toggles. Settings that AgenticOS cannot change yet are not
shown as controls. The app's page model and the versioned system-control ABI
leave room for later display, clock, networking, accessibility, and agent
settings without redesigning the first release.

---

## Problem frame

The Start menu already contains a `Settings` row, but it is deliberately typed
as a disabled placeholder. The system also already has two complete themes,
Classic and Aero, but selection is boot-only:

1. QEMU passes `opt/agenticos/theme` from `AGENTICOS_THEME`.
2. `WindowManager::new` resolves it against the selected renderer.
3. `window::theme::ACTIVE` stores the effective theme.
4. The kernel writes that value once to `/etc/theme`.
5. Each ring-3 process reads `/etc/theme` once and caches its own palette.

That shape is insufficient for a settings application. Calling the existing
crate-private `theme::activate` would change only future paint dispatch. It
would not update frame compositor effects, relayout frame content for the new
metrics, repaint all windows, refresh `/etc/theme`, notify open clients, or
survive a reboot. Runtime renderer fallback has the same partial-update problem
today: it activates Classic internally but does not republish the userland
state.

Wallpaper has a similar one-shot path. GUIShell reads `/WALLPAPR.BMP` before it
creates `DesktopWindow`; the desktop owns the bytes and has no live setter.

The control panel therefore needs one coherent settings owner, not direct
access to scattered globals.

---

## Product experience

### Window and layout

`CONTROL.ELF` opens at approximately 900 x 620 client pixels, with a supported
minimum of 720 x 480. It is an ordinary resizable ring-3 window under the
system-owned Classic/Aero frame.

```text
+ Settings ---------------------------------------------------------------+
| [ Search settings... ]  |  Appearance                                   |
|                         |  Choose how AgenticOS looks                    |
| Home                    |                                                |
| Appearance              |  + Theme ----------------------------------+  |
| Desktop                 |  | [Automatic] [Classic] [Aero Glass]      |  |
|                         |  |  preview       preview      preview      |  |
| System                  |  +------------------------------------------+  |
| Network                 |                                                |
| About                   |  Changes apply immediately                     |
|                         |                                                |
+-------------------------------------------------------------------------+
```

At wide sizes, the 220-pixel sidebar remains visible. Below 800 pixels, it
collapses to a narrow icon rail with the page title retained in the content
header. The content pane scrolls vertically if a page does not fit. There is no
horizontal scrolling.

### Visual language

The Control Center client uses a stable app-local modern palette even when the
system is switched to Classic. This preserves the requested modern identity
while the surrounding frame and the preview cards honestly show the selected
system theme.

| Role | Color |
|---|---|
| App background | `#F5F6F8` |
| Sidebar | `#EEF1F5` |
| Card surface | `#FFFFFF` |
| Primary text | `#1F2329` |
| Secondary text | `#667085` |
| Divider | `#D9DEE7` |
| Accent | `#3478E5` |
| Accent soft | `#E6F0FF` |
| Success | `#218739` |
| Warning | `#A76500` |
| Destructive | `#C73535` |

Rounded cards, selection pills, toggles, radio tiles, and navigation/status
icons are drawn with `Canvas` primitives. Do not depend on unsupported Unicode
glyphs or add bitmap icon assets. Continue using the bundled system TTF already
used by `libs/gui`.

### Pages

#### Home

- Greeting/title and a compact device summary: AgenticOS, x86_64, display
  dimensions, and effective renderer.
- Two actionable cards linking to Appearance and Desktop, showing the current
  effective theme and current wallpaper source.
- Small status cards for uptime, memory use, and network byte totals.

#### Appearance

- Three large choice tiles: **Automatic**, **Classic**, and **Aero Glass**.
- Each tile includes a miniature frame/control preview rendered explicitly for
  that theme. Automatic shows the theme it currently resolves to.
- Selection applies immediately. There is no Apply button for a reversible
  setting.
- Aero is disabled under the legacy renderer with the explanation “Aero
  requires the retained compositor.” It is never accepted and silently
  downgraded.
- A status line distinguishes “Saved” from “Applied for this session; settings
  storage is unavailable.” If a boot-time explicit theme override is active,
  state that it may be reapplied at the next launch.

#### Desktop

- A wallpaper card with a thumbnail-like label, the normalized source path,
  **Choose image...**, and **Restore default**.
- The picker uses `dialogs::FileDialog`, filters/validates `.bmp`, and starts at
  the current wallpaper directory when possible.
- A chosen image is parsed and applied before it is persisted. Bad, empty, or
  oversized files leave the current desktop unchanged and produce a specific
  message.
- “Restore default” reloads `/WALLPAPR.BMP`; if the bundled file is missing or
  invalid, the existing solid AgenticOS blue remains the safe fallback.
- Stretch-to-fill remains the only wallpaper mode in this unit. Do not expose a
  fit-mode control until the desktop implements multiple modes.

#### System

- Read-only rows for display dimensions, renderer (`legacy`, `retained`, or
  `gpu`), uptime, total/free memory, and kernel heap use.
- Values come from the system-control snapshot and `/proc/{uptime,meminfo}`.
  Refresh on page entry and every five seconds while visible.

#### Network

- Read-only interface traffic from `/proc/net/dev` and DNS servers/search
  information from `/etc/resolv.conf`.
- Do not label the system “connected” solely because byte counters or a resolver
  file exist. DHCP state and interface control are outside this unit.

#### About

- AgenticOS name, development-build label, x86_64 architecture, and a short
  description of the project.
- Links are rendered as selectable/copyable-looking text only if a URL-opening
  service exists; otherwise use ordinary text and do not fake activation.

### Search and keyboard behavior

- The search field filters page names and indexed setting-row names. Results
  are a short list of navigable destinations, not a full-text filesystem
  search.
- `Ctrl+F` focuses search, `Escape` clears search or dismisses a modal, arrow
  keys move through navigation/results, `Enter` activates, and `Ctrl+Tab`
  advances pages.
- A mouse wheel scrolls the content page under the pointer. Resize preserves
  the current page, selection, modal state, and valid scroll position.

---

## Requirements

### R1 - Standalone application and launch integration

- **R1.1.** Add `userland/apps/control/` as a `no_std` Rust package depending
  on `runtime`, `gui`, and `dialogs`, with the standard
  `userland_build_support::configure()` build script.
- **R1.2.** Add it to the userland Cargo workspace and
  `userland/apps.manifest.sh` as a built-every-run app staged under the
  FAT-safe name `CONTROL.ELF`.
- **R1.3.** Add `/bin/control` and `/bin/settings` as sorted direct-app aliases
  for `/host/CONTROL.ELF`. Keep `control` as `argv[0]` for the Start launch;
  accept either alias from zsh.
- **R1.4.** Replace the disabled root Start row with
  `StartMenuAction::Settings`. GUIShell defers it through a new
  `PendingAction::SpawnControl` and launches
  `spawn_gui_user_app("/host/CONTROL.ELF", "control")` after closing Start.
- **R1.5.** Multiple launches create independent app processes/windows. Global
  settings changes remain coherent through the shared kernel owner and theme
  broadcasts.

### R2 - Versioned system-control ABI

- **R2.1.** Reserve private syscall **5010** as
  `system_control(command, value, data_ptr, data_len, flags)`. Add the number
  to kernel `abi::nr`, the dispatcher, `userland/runtime`, and ABI tests.
- **R2.2.** Version 1 commands are:
  - `GET_SNAPSHOT` - write `SystemControlSnapshotV1` to the caller.
  - `GET_WALLPAPER_PATH` - copy the normalized configured path into a bounded
    caller buffer.
  - `SET_THEME` - `value` is Automatic, Classic, or Aero.
  - `SET_WALLPAPER_PATH` - `data_ptr/data_len` is a bounded UTF-8 path.
  - `RESET_WALLPAPER` - select the bundled default.
- **R2.3.** `SystemControlSnapshotV1` begins with `version` and `byte_len`, then
  contains theme preference, effective theme, theme availability mask,
  renderer kind, boot-override flags, wallpaper state, persistence flags, and
  display width/height. Reserved zero fields permit compatible growth. Pin its
  exact `repr(C)` layout with kernel/runtime size and offset assertions.
- **R2.4.** Getter commands return bytes written; setter commands return `0`
  when live application and persistence both succeed, `1` when the change is
  live but session-only, and negative errno when no change was applied. Runtime
  wraps this as `ApplyResult::{Persisted, SessionOnly}` so apps do not interpret
  the positive status ad hoc.
- **R2.5.** Unknown command, enum, flag, version, nonzero reserved input, short
  output buffer, invalid user pointer, relative/control-character path, or path
  longer than 1024 bytes returns an appropriate negative errno without changing
  state.
- **R2.6.** AgenticOS is currently a single-user root system: any live ring-3
  process may call 5010, matching existing `kill(2)` authority. Document this
  explicitly so a future user/permission model can put authorization at this
  one boundary.

### R3 - Settings ownership and persistence

- **R3.1.** Add a focused kernel module (recommended:
  `src/system_control.rs`) that owns requested settings, persistence, validation,
  and orchestration. `window::theme` continues to own effective drawing state;
  `DesktopWindow` continues to own wallpaper bytes. Do not introduce duplicate
  effective-theme globals.
- **R3.2.** Persist a forward-compatible, bounded line file at
  `/data/agenticos/settings.conf` with keys `theme=auto|classic|aero` and
  `wallpaper=default|<absolute-path>`. Reject newline/control characters in
  paths, ignore unknown keys, and default individual malformed keys rather than
  discarding all settings.
- **R3.3.** Create `/data/agenticos` when writable. Save via a sibling temporary
  file, close/sync it, rename atomically, then sync the backing filesystem. A
  missing/read-only `/data` makes changes session-only; it never blocks GUI boot.
- **R3.4.** Load the file after `/data` mount and overlay restoration but before
  display/window-manager initialization. Cap the file at 4 KiB and emit concise
  warnings for malformed or unavailable state.
- **R3.5.** Theme boot precedence is:
  1. explicit fw_cfg `classic`/`aero` for the current launch;
  2. persisted `theme` when fw_cfg is missing or `auto`;
  3. existing renderer-derived Automatic behavior.
  An unavailable Aero request resolves to Classic but remains the stored
  preference so it becomes usable on a later retained boot.
- **R3.6.** A live user choice updates the in-memory preference even if saving
  fails. The snapshot reports both preference and effective value, preventing
  “Automatic (Aero)” or “Aero requested, Classic active” from being ambiguous.

### R4 - Coherent live theme transitions

- **R4.1.** Add a `WindowManager` theme-transition entry point that accepts a
  `ThemeRequest`, resolves it against the *current* renderer, and either returns
  a `ThemeSelection` or rejects unsupported explicit Aero with `-ENOTSUP`.
- **R4.2.** A successful effective-theme change is one window-manager
  transaction:
  1. capture old/new frame metrics;
  2. activate the new effective theme;
  3. update every `FrameWindow` compositor effect (`None` versus Aero backdrop
     sample) and invalidate its old/new decorated extents;
  4. preserve each frame's client width/height while adjusting outer frame
     bounds, clamp top-level frames to the visible desktop, and relayout child
     content wells;
  5. invalidate every visible window because kernel controls read the global
     palette at paint time;
  6. request a full repaint so legacy and retained output cannot retain old
     pixels or cached effects.
- **R4.3.** Add the minimum typed frame/desktop accessors needed for this work;
  do not downcast arbitrary `dyn Window` or expose the registry to userland.
- **R4.4.** After releasing the window-manager lock, rewrite `/etc/theme` and
  broadcast the transition. Filesystem I/O and GUI/process wakeups must never
  occur under the window-manager lock.
- **R4.5.** Extend fixed-size `GuiEvent` compatibly with
  `GUI_EVENT_THEME_CHANGED = 6`, `window = 0`, `payload[0] = effective theme`,
  and `payload[1] = requested preference`. Keep `GUI_ABI_VERSION = 1` and the
  32-byte layout because no field layout changes.
- **R4.6.** Broadcast one process-global theme event per GUI-owning PID and
  coalesce an older pending theme event to the newest value. Preserve the
  bounded-queue and wake-on-event behavior.
- **R4.7.** `gui::next_event` and `gui::try_next_event` update the process-local
  `gui::theme` cache before returning a theme event. Audit every shipped GUI
  event loop (`notepad`, `guidemo`, `taskmgr`, `fileman`, `calc`, `painting`,
  and `glgame`) so it redraws theme-sensitive content or explicitly consumes
  the event when its client pixels are intentionally theme-invariant. Active
  dialogs redraw too.
- **R4.8.** Route non-strict retained-to-legacy runtime fallback through the
  same internal theme-application mechanics. Queue `/etc/theme` publication
  and the GUI broadcast for a post-render, outside-WM-lock drain so fallback
  can no longer leave kernel and userland palettes divergent.
- **R4.9.** Repeatedly selecting the existing request/effective theme is
  idempotent: preference persistence may be refreshed, but there is no needless
  full repaint or duplicate broadcast.

### R5 - Live wallpaper control

- **R5.1.** Add `DesktopWindow::set_wallpaper(Option<Vec<u8>>)` and the narrow
  typed `Window` accessor needed by `WindowManager`. Replacing bytes clears or
  invalidates the cached backing store and dirties the full desktop.
- **R5.2.** `SET_WALLPAPER_PATH` accepts only a normalized absolute path. Read
  the file outside the window-manager lock, cap it at 16 MiB, and validate it
  with `BmpImage::from_bytes` before touching the active desktop.
- **R5.3.** Only after validation succeeds, move the owned bytes into the
  desktop under one short WM transaction, invalidate it, and request a full
  repaint. The old wallpaper remains intact on read, size, parse, allocation,
  or desktop-lookup failure.
- **R5.4.** Persist the normalized path only after live application succeeds.
  Reset uses `default` in the settings file and reloads `/WALLPAPR.BMP`; a
  missing/bad bundled asset selects the existing solid-blue fallback without a
  panic.
- **R5.5.** GUIShell boot reads the resolved wallpaper preference rather than
  calling a fixed-path loader. A missing saved custom image falls back to the
  bundled default, logs once, and reports the fallback state in the control
  snapshot so Settings can explain it.
- **R5.6.** Do not pass raw wallpaper bytes through syscall 5010. Kernel-side
  bounded file loading avoids a second large user-copy allocation and makes
  persisted boot behavior use the exact same validation path as live apply.

### R6 - Control Center application model

- **R6.1.** Keep app state per process: active page, sidebar/search focus,
  search text/results, scroll offset per page, latest snapshot/status samples,
  optional modal, hover/pressed target, and transient success/error banner.
  No kernel callbacks or app-global mutable state.
- **R6.2.** Separate pure page/search/layout/settings-response logic from the
  `_start` and rendering loop (`model.rs`, `layout.rs`, `status.rs` or an
  equivalent small split). Rendering consumes immutable view data; hit testing
  derives from the same computed rectangles.
- **R6.3.** Build Control Center-specific `Sidebar`, `SettingsCard`,
  `ChoiceTile`, `StatusRow`, and `Toast` locally. Promote only generic helpers
  needed by a second consumer into `libs/gui`.
- **R6.4.** Extend `gui::theme` with explicit-theme preview helpers such as
  `palette_for(Theme)` and `draw_button_for(Theme, ...)`; never change the
  process-global active theme temporarily to draw previews.
- **R6.5.** Use `dialogs::FileDialog` and `MessageBox` for wallpaper selection
  and errors. Route events by window handle using the existing `Modal` pattern,
  including theme-change redraw while a modal is open.
- **R6.6.** Present only when visual state changes. Home/System/Network timed
  refresh uses nonblocking event drain plus `nanosleep`; other pages may block
  on `gui::next_event` when no banner timeout or refresh deadline is active.
- **R6.7.** Parse `/proc` and resolver text defensively with bounded reads.
  Missing individual sources show “Unavailable” in that row and do not stop
  Settings from opening or personalization from working.

### R7 - Tests, documentation, and observability

- **R7.1.** Add focused kernel tests for config parse/serialize/defaults,
  boot precedence, missing `/data`, atomic-save failure behavior, syscall input
  validation, snapshot ABI layout, and session-only return status.
- **R7.2.** Add window/theme tests for Classic-to-Aero-to-Classic transitions:
  frame effect, preserved client size, relaid child bounds, screen clamping,
  full repaint, palette dispatch, and idempotent re-selection.
- **R7.3.** Add GUI queue tests for one event per PID, payload encoding,
  replacement/coalescing, queue bounds, and wake behavior.
- **R7.4.** Add wallpaper tests for valid live replacement, cached backing-store
  invalidation, malformed/empty/oversized rejection without state loss, default
  reset, and boot fallback when a persisted path is unavailable.
- **R7.5.** Update Start-menu tests for enabled Settings dispatch without
  changing root order/height. Update `/bin` tests for both aliases and manifest
  staging checks for `CONTROL.ELF`.
- **R7.6.** Add concise serial records for settings load/save and live apply,
  including request/effective theme, renderer, persistence outcome, and
  wallpaper path/fallback reason. Never log file contents.
- **R7.7.** Update root, window, userland, commands, and userland-build docs to
  describe the live settings owner, syscall 5010, theme event, wallpaper
  persistence, executable/aliases, and Start integration. Flip this plan to
  `completed` only after the acceptance matrix passes.

---

## High-level technical design

### Ownership and change flow

```text
CONTROL.ELF
  -> syscall 5010
     -> system_control validates requested preference/path
        -> load/parse file if needed (no WM lock)
        -> WindowManager applies theme or desktop bytes (short WM lock)
        -> persist /data/agenticos/settings.conf (no WM lock)
        -> publish /etc/theme when relevant
        -> broadcast one GUI_EVENT_THEME_CHANGED per GUI PID
           -> gui toolkit updates its cached palette
           -> each open app redraws its own client surface
```

The syscall is an orchestration boundary, not a drawing API. Kernel theme code
remains the rendering authority; ring-3 receives a small typed control surface
and ordinary events.

### Why a syscall instead of writable `/etc/theme`

Making `/etc/theme` writable would provide no safe hook to relayout frame
metrics, change compositor effects, validate renderer support, repaint the
desktop, or notify processes. A virtual writable file with side effects would
also violate the current rule that `/etc` is kernel-managed and turn `write(2)`
into a hidden GUI control plane.

Syscall 5010 makes validation, application, persistence, and notification one
explicit transaction. `/etc/theme` remains a readable compatibility/publication
file for apps and shell inspection.

### Why one versioned control syscall

Theme-specific and wallpaper-specific syscall numbers would work now but force
the private ABI to grow for every settings page. A command plus versioned
snapshot provides one authorization and validation boundary while keeping
commands strongly typed. It must not become a generic arbitrary key/value bag:
each command gets an enum, bounds, semantics, tests, and an explicit runtime
wrapper.

### Theme geometry policy

Classic and Aero use different border/title/shadow metrics. A runtime change
must not unexpectedly resize every application's client canvas. For each frame,
derive the current content width/height from the old metrics, rebuild the outer
frame size with the new metrics, then relayout children. Keep the outer top-left
when possible and clamp the result to the display. This preserves client buffer
dimensions, avoiding a storm of synthetic resize reallocations while still
making hit testing and painting use the new metrics.

### Persistence semantics

`/data` is the direct persistent ext2 namespace and is available before display
initialization, making it a better settings authority than the recreated
managed `/etc` or the sync-backed overlay. Failure to mount/write it is a
degraded but supported boot mode. Live changes still work and are clearly
reported as session-only.

---

## Implementation units

### U1 - Settings state, persistence, and read-only snapshot

1. Add `src/system_control.rs` with enums, snapshot, bounded config parser,
   load/save helpers, persistence status, and QEMU-test hooks.
2. Initialize it in `kernel.rs` after storage/overlay setup and before display.
3. Add syscall 5010 to kernel ABI and runtime with `GET_SNAPSHOT` and
   `GET_WALLPAPER_PATH` first.
4. Add config and ABI tests before any mutating command.

### U2 - Live theme application and propagation

1. Refactor theme activation into `WindowManager::apply_theme_request` plus a
   frame retheme helper that preserves client size and updates effects.
2. Implement `SET_THEME`, `/etc/theme` republish, persistence, and explicit
   unsupported-Aero errors.
3. Add the theme GUI event, coalesced broadcast, toolkit cache update, and app
   event-loop audit.
4. Route renderer runtime fallback through the same internal apply path and a
   deferred outside-lock publication drain.
5. Land transition/queue tests and run the Classic/Aero renderer matrix.

### U3 - Live wallpaper application

1. Add desktop typed access/setter and resolved wallpaper loading.
2. Implement bounded validation, `SET_WALLPAPER_PATH`, and
   `RESET_WALLPAPER` without holding the WM lock during I/O.
3. Switch GUIShell boot wallpaper loading to the stored preference/fallback
   model.
4. Land wallpaper and persistence tests.

### U4 - CONTROL.ELF shell and real pages

1. Scaffold the app, manifest/workspace membership, event loop, responsive
   sidebar/content layout, modern tokens, navigation, and search.
2. Implement Home and Appearance, including explicit Classic/Aero preview
   rendering and live apply result banners.
3. Implement Desktop through common dialogs.
4. Implement bounded System/Network samplers and About.
5. Verify resize, keyboard, search, modals, session-only state, and multiple
   instances.

### U5 - Desktop and shell integration

1. Stage `CONTROL.ELF`; add `CONTROL_HOST_PATH` and direct aliases.
2. Enable the existing Start Settings row and wire deferred launch.
3. Update start/bin/manifest coverage and all subsystem documentation.
4. Run the complete automated and manual acceptance suites, then record the
   implementation outcome in this plan.

---

## Expected file map

### New

- `src/system_control.rs` - settings state, persistence, syscall handler, and
  post-WM publication orchestration.
- `userland/apps/control/Cargo.toml`
- `userland/apps/control/build.rs`
- `userland/apps/control/src/main.rs`
- `userland/apps/control/src/model.rs`
- `userland/apps/control/src/layout.rs`
- `userland/apps/control/src/status.rs`

### Modified

- `src/kernel.rs` and the crate module registry
- `src/userland/abi.rs`, `src/userland/gui.rs`, `src/userland/etc.rs`, and
  `src/userland/bin_namespace.rs`
- `src/window/theme/mod.rs`, `src/window/manager.rs`, `src/window/mod.rs`,
  `src/window/windows/frame.rs`, `src/window/windows/desktop.rs`, and any
  narrow trait accessors in the window interface
- `src/window/windows/start_menu.rs` and `src/commands/guishell/mod.rs`
- `userland/runtime/src/lib.rs`, `userland/libs/gui/src/{lib,theme}.rs`, and
  dialog theme-event handling
- Shipped GUI app event loops that consume the new global event
- `userland/Cargo.toml` and `userland/apps.manifest.sh`
- Focused `src/tests/` topics/registries for system control, theme, GUI, desktop,
  Start, userland ABI, and `/bin`
- `CLAUDE.md`, `src/window/CLAUDE.md`, `src/userland/CLAUDE.md`,
  `src/commands/CLAUDE.md`, and `userland/README.md`

---

## Verification

### Automated

```sh
cargo fmt --check
cargo check
cargo check --features test
cargo clippy
cargo build --manifest-path userland/Cargo.toml --release
./build.sh -n
./test.sh start_menu window_theme gui_userland desktop_window userland
./test.sh system_control
./test.sh
```

Add the exact new test topic name to `./test.sh -l`; `system_control` above is
the recommended stable filter.

### Manual QEMU acceptance

Run at least:

```sh
AGENTICOS_COMPOSITOR=legacy AGENTICOS_THEME=auto ./build.sh
AGENTICOS_COMPOSITOR=retained AGENTICOS_THEME=auto ./build.sh
AGENTICOS_COMPOSITOR=retained AGENTICOS_THEME=classic ./build.sh
```

Verify:

- Start -> Settings is enabled, closes Start, and launches exactly one
  `CONTROL.ELF` window per click. `/bin/control` and `/bin/settings` work.
- The Control Center remains usable at default/minimum sizes and every page,
  search result, keyboard path, scroll path, and modal is reachable.
- On retained rendering, Classic <-> Aero changes frame chrome, controls,
  shadows/blur, taskbar/Start, Settings previews, Notepad, GUIDemo, Task Manager,
  and any open common dialog without restarting or changing client size.
- On legacy rendering, Aero is visibly disabled and a direct syscall attempt
  returns `-ENOTSUP`; Classic and Automatic remain safe.
- `cat /etc/theme` matches the effective theme after every transition.
- Closing/reopening an app after a switch uses the same theme as already-open
  apps.
- Restarting with fw_cfg `auto` restores the saved theme. An explicit launch
  override is identified in Settings and wins for that boot.
- Choosing a valid BMP applies it live, survives reboot, and does not freeze
  window dragging/input while the file is read. Restore Default returns to the
  bundled image.
- Missing, malformed, empty, and oversized BMPs leave the prior wallpaper
  visible and show a useful error. A saved custom file removed before reboot
  falls back cleanly.
- With `/data` unavailable/read-only, theme and wallpaper still apply for the
  session and the UI says they were not saved.
- Two Settings windows remain synchronized via snapshots/events rather than
  maintaining conflicting local truth.
- A forced non-strict retained renderer failure switches the whole system to
  Classic, republishes `/etc/theme`, and redraws open themed apps.

---

## Risks and mitigations

- **Theme metrics can desynchronize frame paint, hit testing, and client
  bounds.** Keep one manager transaction driven by `metrics_for(old/new)`, use
  the same helpers as frame paint/hit testing, and pin client-size preservation
  in tests.
- **A theme event can be lost behind a busy mouse queue.** Coalesce to the
  newest value and keep it as an ordinary bounded event; `/etc/theme` plus the
  snapshot remain authoritative recovery sources on the next redraw/query.
- **Lock inversion can deadlock the single CPU.** File reads/writes and GUI
  wakeups happen outside the WM lock. The WM transaction receives owned bytes
  and returns plain state; it never calls VFS, process, or GUI queue code.
- **Wallpaper replacement can create a large transient allocation.** Enforce a
  16 MiB source cap, validate once, move the single owned buffer into the
  desktop, and do not accept raw syscall pixel buffers.
- **Persist succeeded but live apply failed, or vice versa.** Apply first and
  persist second. Negative errno means unchanged; positive session-only status
  means the user sees the live change and an honest persistence warning.
- **Boot override versus user preference can look like failed persistence.**
  Return both fields and an override flag in the snapshot and explain the
  precedence in Appearance.
- **Modern controls duplicate generic toolkit ideas.** Keep first-consumer
  pieces app-local; only explicit-theme preview primitives belong in shared
  theme code now.
- **Read-only status pages can imply unsupported management.** Use labels such
  as “System information” and “Network activity”; do not render switches or
  buttons for unavailable operations.

---

## Out of scope and follow-ups

- Changing compositor/renderer mode at runtime; renderer initialization and
  VirGL qualification remain boot policy.
- UI scaling, font selection, accent-color customization, high contrast, dark
  mode, animation policy, or accessibility services.
- Wallpaper fit/center/tile modes, JPEG/PNG decoding, thumbnails, slideshows,
  or per-screen wallpaper.
- Display resolution switching, multi-monitor arrangement, refresh-rate or GPU
  configuration.
- Network enable/disable, DHCP/static addressing, DNS editing, Wi-Fi, proxy,
  firewall, or TLS settings.
- Time zone/locale/12-hour clock controls; the taskbar remains UTC.
- User accounts, per-user settings, privileges, policy locking, and settings
  roaming.
- Automatic registration/discovery of third-party settings panes. Add pages to
  the explicit model until there is a package/permission system to trust.
- Launching external URLs or adding inert “Check for updates” affordances.

Likely next pages, once their subsystems exist, are Display, Date & Time,
Accessibility, Network configuration, and an Agentic runtime page. Each should
arrive with a real kernel/userland control surface and tests, not as a disabled
mock control.

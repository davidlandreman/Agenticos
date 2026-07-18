---
title: "feat: ring-3 GUI app platform, userland build unification, and notepad extraction"
type: feat
status: completed
date: 2026-07-18
---

# feat: ring-3 GUI app platform, userland build unification, and notepad extraction

## Summary

Extract `notepad` from the kernel into a real ring-3 ELF (`NOTEPAD.ELF`) launched by the Start menu and by typing `notepad` in zsh — and build the platform that makes the second, third, and tenth such app cheap. This is an umbrella plan with four phases, each independently landable:

- **Phase 0 — userland build unification.** Replace today's three divergent shipping mechanisms with one declarative app manifest consumed by a shared staging library used by both `build.sh` and `test.sh`, and one uniform prebuilt-refresh path. No kernel changes.
- **Phase 1 — kernel GUI surface for ring 3.** Four new AgenticOS syscalls (`gui_win_create`, `gui_win_present`, `gui_next_event`, `gui_win_destroy`) backed by a new `RemoteSurface` window type, a per-process GUI event queue with a blocking wait, and window teardown on process death. Proven by a throwaway `GUIDEMO.ELF`.
- **Phase 2 — userland runtime + GUI toolkit.** Grow `userland/runtime` into a usable libc-lite (allocator, full syscall stubs, argv/env), and add a `gui` toolkit crate (event loop, software canvas, text rendering sharing the kernel's font bytes).
- **Phase 3 — the notepad port.** `userland/apps/notepad` (no_std Rust workspace member, built every run, staged as `NOTEPAD.ELF`), Start-menu and `/bin/notepad` rewiring, working Save via the real filesystem write syscalls, deletion of `src/commands/notepad/`.

The other four kernel GUI apps (`calc`, `painting`, `tasks`, `explorer`) stay kernel-side; migrating them is follow-up work that reuses everything this plan builds.

---

## Problem Frame

`notepad` is not a userland app that happens to be linked into the kernel — it is ring-0 kernel code (`src/commands/notepad/mod.rs`, ~550 lines) implementing the kernel's `RunnableProcess` trait and calling `with_window_manager(...)` directly to compose kernel-owned widgets (`FrameWindow`, `MenuBar`, `ScrollView`, `TextEditor`, kernel dialogs). The "launch from zsh" path is an illusion of userland: `execve("/bin/notepad")` is rewritten to `GLAUNCH.ELF`, which calls `sys_gui_launch(5000)` — which spawns the *kernel-side* app and exits. The Start menu doesn't even round-trip through ring 3; `guishell::spawn_notepad()` calls `gui_launch_table::spawn_by_name("notepad")` in-kernel.

Making notepad a separate ELF is therefore blocked on a missing platform, not on packaging:

1. **Ring 3 has no GUI surface.** The syscall table is a solid Linux ABI plus exactly one custom syscall (`GUI_LAUNCH = 5000`). There is no window-create syscall, no path for ring-3 pixels to reach the screen (`mmap` rejects `MAP_SHARED` with `-ENOSYS`, and `MemoryMapper::map_user_region` can only allocate fresh zeroed frames — it cannot share a kernel buffer with a user address space), and no input event channel (keyboard reaches ring 3 only as pty bytes; mouse events never reach ring 3 at all).
2. **The Rust userland runtime is four syscall stubs.** `userland/runtime` exposes `print`, `exit`, `gui_launch`, and `argv0_from_stack`. No allocator, no file I/O, no toolkit. Any real app would hand-roll `asm!`.
3. **The userland build system has three divergent shipping patterns.** `ZSH`/`BB` use `stage_*` functions in `prebuilt-lib.sh` refreshed by `refresh-prebuilt.sh`; the `compiler-compat`/`network-test` fixtures use per-Makefile `make refresh` targets and inline copy loops that exist only in `test.sh`; `HELLO`/`GLAUNCH`/`HELLOCPP` use hand-written staging blocks duplicated across `build.sh` and `test.sh`. The dir→8.3-name mapping (`guilaunch`→`GLAUNCH`, `busybox`→`BB`, …) lives as scattered string literals. `test.sh` stages `BB.ELF` twice. READMEs have drifted (the layout tree omits `busybox/` and `network-test/`). A new app author has to guess which of the three patterns to follow.

Two strong foundations already exist and shape the design:

- **The blocking scheduler substrate.** `block_current_ring3_and_yield` + `Ring3BlockReason` + wake helpers (`src/userland/switch.rs`) is exactly what a blocking "get next GUI event" syscall needs — a new block reason and a wake call, no new scheduler machinery.
- **The terminal/PTY precedent.** The windowed terminal already proves the shape "kernel-owned window ↔ per-process queues ↔ blocking ring-3 syscalls ↔ compositor render loop ↔ focus-based input routing" (`src/window/terminal_factory.rs`, `src/terminal/pty.rs`). The GUI surface follows the identical shape with a richer channel than raw bytes.

A pleasant side effect: kernel notepad's Save is stubbed ("filesystem is currently read-only" — it predates the Phase-B write syscalls). The ring-3 notepad saves through real `openat`/`write` to the writable overlay/tmpfs namespaces, so extraction fixes Save rather than porting a stub. The port also eliminates the kernel version's three ugliest warts, all forced by in-kernel callback constraints: the global `NOTEPAD_STATES: Mutex<BTreeMap<usize, NotepadState>>` (widget callbacks can't capture `&mut self`), the busy-poll run loop, and the `TODO` downcasts where it can't get its `TextEditor` back out of the window registry.

---

## Requirements

### Phase 0 — userland build unification

- **R0.1.** A declarative app manifest at `userland/apps.manifest.sh` (shell-sourceable table): one row per app declaring `name`, `source dir`, `build-kind ∈ {cargo, make}`, `staged 8.3 name`, `ship-kind ∈ {built-every-run, prebuilt-managed, test-fixture}`, and toolchain requirement (`rust-nightly` | `musl-cc` | `musl-cxx`). All current apps (`hello`, `guilaunch`, `hello-cpp`, `zsh`, `busybox`, `compiler-compat` fixtures, `network-test`) get rows.
- **R0.2.** A shared staging library (`userland/stage-lib.sh`, generalizing `prebuilt-lib.sh`) that drives all staging from the manifest. Both `build.sh` and `test.sh` call it instead of their current inline blocks. Semantics preserved: `--skip-userland` (test.sh) skips built-every-run apps but always stages prebuilts and test fixtures; `--rebuild-userland` / `REBUILD_USERLAND=1` / per-app `REBUILD_<NAME>=1` force rebuilds of prebuilt-managed apps; prebuilt-managed staging soft-fails in build/test; `readelf -h` Type==EXEC validation happens in exactly one shared helper.
- **R0.3.** `refresh-prebuilt.sh` iterates every manifest row with ship-kind `prebuilt-managed` or `test-fixture` (hard-fail on error, print `git status --short userland/prebuilt/`, no auto-commit). The per-Makefile `make refresh` targets either delegate to it or are removed.
- **R0.4.** Fix the known staging duplication: `BB.ELF` staged twice in `test.sh`; `build.sh` vs `test.sh` divergence on fixtures becomes an explicit manifest property (fixtures staged by test.sh only) rather than accidental.
- **R0.5.** Rust per-app `build.rs` deduplication: a shared build-support helper (small path crate or shared include) so a new workspace app doesn't copy-paste linker-arg emission.
- **R0.6.** Documentation de-drift: `userland/README.md` layout tree lists all apps; `userland/prebuilt/README.md` table covers all prebuilt/fixture artifacts; both describe the manifest as the single "add a new app" entry point.

### Phase 1 — kernel GUI surface

- **R1.1.** Four new syscalls in the AgenticOS range, dispatched in `src/userland/abi.rs`, handlers in `src/userland/syscalls.rs` (or a new `gui_syscalls.rs`):
  - `GUI_WIN_CREATE = 5001`: `(width, height, title_ptr, title_len, flags) -> handle | -errno`. Creates a `FrameWindow` (kernel decorations: title bar, borders, close button, dragging, focus) wrapping a new `RemoteSurface` content window; registers ownership under the calling PID; returns a small per-process integer handle (≥1).
  - `GUI_WIN_PRESENT = 5002`: `(handle, pixels_ptr, width, height, stride_bytes) -> 0 | -errno`. Usercopies the pixel buffer into the kernel-owned surface and invalidates the window. Full-surface presents only in v1 (no damage rects). Pixel format: the compositor's native 32-bit format, documented as a constant in the ABI.
  - `GUI_NEXT_EVENT = 5003`: `(event_buf_ptr, buf_len, flags) -> 0 | -errno`. Dequeues one event from the calling process's GUI event queue into a fixed-size `#[repr(C)]` struct. Empty queue + default flags → block via new `Ring3BlockReason::WaitingForGuiEvent`; `GUI_NONBLOCK` flag → `-EAGAIN`.
  - `GUI_WIN_DESTROY = 5004`: `(handle) -> 0 | -errno`. Destroys the frame + surface windows and frees the handle.
- **R1.2.** `RemoteSurface` window type (`src/window/windows/remote_surface.rs`): owns the kernel-side pixel buffer; `paint()` blits it via `GraphicsDevice::blit_buffer`; `handle_event()` encodes keyboard/mouse/resize/close/focus events into the owning process's queue and wakes it. Close is delivered as an event, not enforced — the app decides when to destroy.
- **R1.3.** Per-process GUI event queue: bounded (128 events), each event tagged with its window handle (multi-window per process works — dialogs). Overflow policy: coalesce consecutive mouse-move events for the same window; otherwise drop-oldest with a debug-log line. Wake path mirrors `wake_ring3_blocked_on_input`.
- **R1.4.** The event struct is a versioned fixed-size POD (`#[repr(C)]`, 32 bytes): `{ kind: u32, window: u32, payload: [u32; 6] }` with kinds Key (keycode, char, modifiers, pressed), Mouse (x, y, buttons, kind: move/down/up/scroll), Resize (w, h), Close, FocusChange. Layout defined once in the kernel ABI and mirrored in the userland runtime; a compile-time size assertion on both sides.
- **R1.5.** Process-death cleanup: `cleanup_user_process` destroys all windows owned by the dying PID (frame + surface) and frees the event queue, respecting `with_window_manager` lock ordering. A crashed app cannot leak windows.
- **R1.6.** Resize semantics v1: on user resize of the frame, the kernel resizes the content area, delivers a Resize event, and keeps blitting the last-presented buffer clipped/anchored top-left until the app presents at the new size.
- **R1.7.** A throwaway validation app `userland/apps/guidemo/` (staged `GUIDEMO.ELF`, built-every-run, one manifest row): fills a background color, draws a rectangle that follows mouse position, changes color on keypress, exits cleanly on Close. Serves as the manual smoke for Phase 1 and stays in-tree as the minimal reference client.
- **R1.8.** In-kernel tests: syscall handler tests (create/destroy lifecycles, bad handle, bad pointers, event struct encoding, queue overflow/coalescing, nonblocking `-EAGAIN`) and a PID-cleanup test.

### Phase 2 — userland runtime + GUI toolkit

- **R2.1.** `userland/runtime` grows into a libc-lite for no_std Rust apps: syscall stubs for at least `read`, `write`, `openat`, `close`, `lseek`, `fstat`, `getdents64`, `mkdir`, `unlink`, `rename`, `ftruncate`, `nanosleep`, `brk`, `exit_group`, plus the four `gui_*` calls and the shared event struct; a `#[global_allocator]` (simple free-list over `brk`-grown memory) so `alloc::{Vec, String, Box}` work; `_start` glue exposing argv/env; the panic handler.
- **R2.2.** New toolkit crate `userland/libs/gui` (workspace member, no_std + alloc): `Window` wrapper over the handles, an event-loop helper, and a software `Canvas` over a `Vec<u32>` (fill_rect, horizontal/vertical lines, `draw_text`).
- **R2.3.** Font sharing: the kernel's bitmap font bytes move to a dependency-free path crate (e.g. `shared/fontdata/`) consumed by both the kernel (path dependency) and the toolkit — one font source, no drift. Kernel rendering behavior unchanged.
- **R2.4.** Toolkit widgets are demand-driven: only what notepad needs (menu bar with dropdowns, scrollable text view, message/file dialogs — see R3.4). No speculative widget gallery.

### Phase 3 — the notepad port

- **R3.1.** `userland/apps/notepad/`: no_std Rust workspace member depending on `runtime` + `gui`; built every run; staged as `NOTEPAD.ELF`; one manifest row. App owns its state in normal structs with a normal event loop (`loop { next_event; update; present }`) — no globals, no polling.
- **R3.2.** Feature parity with kernel notepad: menu bar (File: New/Open/Save/Save As/Exit), editable text area with cursor/selection/scrolling, open-file flow, error/info dialogs — plus **working Save/Save As** targeting the writable namespaces (overlay `/`, `/data`) via real write syscalls. `/host` stays read-only and Save there surfaces the `-EROFS` as a dialog.
- **R3.3.** Launch rewiring:
  - `guishell::spawn_notepad()` stops calling `spawn_by_name("notepad")` and instead spawns `/host/NOTEPAD.ELF` the way the terminal spawns zsh: a kernel wrapper thread invoking `launcher::launch_user_binary` and blocking on `WaitingForRing3Exit` (extract a shared helper from `terminal_factory` rather than duplicating it).
  - `bin_namespace.rs`: `notepad` moves out of `GUI_APPLETS` into a direct execve rewrite to `/host/NOTEPAD.ELF` (argv[0] preserved); `stat`/`access`/`getdents64` on `/bin` keep listing it.
  - `gui_launch_table.rs` drops the `notepad` arm; the `test_every_gui_applet_dispatches` sync test updates accordingly.
- **R3.4.** File Open/Save dialogs are implemented in the toolkit (or notepad itself) as ring-3 windows using `getdents64`/`stat` — no new kernel dialog syscalls.
- **R3.5.** Delete `src/commands/notepad/` and its `mod.rs` registration. While touching `guishell`'s Start menu, fix the latent sizing bug (`menu_items` hardcoded to 3 with 4 items added).
- **R3.6.** End-to-end smoke (manual): boot → Start → Notepad opens the ring-3 app; typing works; File→Open loads a file from `/host`; File→Save writes a file under `/` (survives `sync` + reboot via overlay persistence); close button delivers Close and the window tears down; `notepad` typed in zsh launches it; killing the process (window close during edit) leaks no windows.
- **R3.7.** Documentation: `CLAUDE.md` current-state + subsystem index; `src/commands/CLAUDE.md` (notepad removed, migration pattern noted); `src/userland/CLAUDE.md` (GUI syscalls); `src/window/CLAUDE.md` (`RemoteSurface`); `userland/README.md` ("adding a GUI app" section referencing the manifest and toolkit).

---

## Scope Boundaries

### Outside this plan's scope

- **Migrating `calc`, `painting`, `tasks`, `explorer`.** Each is a follow-up plan reusing this platform; each migration grows the toolkit with the widgets it needs. `guishell` (desktop/taskbar) stays kernel-side indefinitely — it *is* the window-management policy layer.
- **Shared-memory surfaces.** v1 is copy-blit. The handle-based ABI deliberately hides the transport so a later `MAP_SHARED`-style upgrade (new frame-sharing machinery in `src/mm/`) changes no app code. Not started here.
- **Damage rects / partial presents.** Full-surface presents only; the syscall signature leaves room (flags) for later damage support.
- **Clipboard, cursor-shape control, window icons, IME.** None exist kernel-side either.
- **A C-ABI header for the GUI syscalls.** musl/C apps could call them, but the supported first-class pattern for OS-native GUI apps is no_std Rust; a C header is follow-up if wanted.
- **Retiring `GLAUNCH.ELF` / `sys_gui_launch`.** They still serve the four remaining kernel-side apps. They retire when the last app migrates.

### Deferred to Follow-Up Work

- **Shared-memory surface upgrade** if blit copying ever shows in profiles (painting is the likely trigger).
- **`gui_win_set_title` and other window-property syscalls** — notepad wants "filename — Notepad" titles; v1 can encode the title at create time and recreate on rename, or just keep a static title; a set-title syscall is a trivial 5005 later.
- **Event-queue readiness integration with `poll`** (a GUI event fd) — needed only when an app wants to multiplex sockets + GUI events.
- **Migrating the remaining four GUI apps**, then deleting `gui_launch_table`, `GUI_APPLETS`, `GLAUNCH.ELF`, and syscall 5000.

---

## High-Level Technical Design

Directional guidance for review, not implementation specification.

### Launch flow (Start → Notepad, after Phase 3)

```
guishell start menu click "Notepad"
 └─ queue_action(SpawnNotepad) → guishell process wakes
     └─ spawn_gui_user_app("/host/NOTEPAD.ELF")        [shared helper, extracted
         └─ kernel wrapper thread:                       from terminal_factory]
              launch_user_binary("/host/NOTEPAD.ELF", ["notepad"], env)
              block WaitingForRing3Exit(pid)

NOTEPAD.ELF (ring 3):
  gui_win_create(640, 400, "Untitled — Notepad") → handle 1
  render into Vec<u32>; gui_win_present(1, buf, 640, h, stride)
  loop {
    gui_next_event(&mut ev)          ← blocks (WaitingForGuiEvent)
    match ev.kind { Key → edit buffer, Mouse → menus/caret,
                    Resize → realloc + re-present, Close → save-check → break }
    gui_win_present(...)             ← only after state changes
  }
  gui_win_destroy(1); exit(0)
```

Typing `notepad` in zsh takes the same ring-3 path via the execve rewrite — no GLAUNCH shim, no syscall 5000.

### Event flow (kernel side)

```
compositor input routing (focused window)
 └─ RemoteSurface::handle_event(Event)
     ├─ encode → GuiEvent { kind, window: handle, payload }
     ├─ push to per-process queue (bounded 128, coalesce mouse-moves)
     └─ wake_ring3_blocked_on_gui_event(pid)
          └─ blocked gui_next_event re-fires (restartable-syscall rewind,
             same mechanism as read-on-pty), dequeues, usercopies out
```

The queue lives keyed by PID alongside the process table; `RemoteSurface` holds `(owner_pid, handle)`. `cleanup_user_process` walks the PID's handle table and destroys windows inside one `with_window_manager` block.

### Present path

`gui_win_present` validates the handle and dimensions, usercopies `stride × height` bytes into the `RemoteSurface`'s kernel buffer (allocated at current content size), and invalidates the window so the next compositor frame repaints. A notepad-sized window (~640×400×4 ≈ 1 MiB) at typing rates is a few MiB/s of copying — irrelevant. No mm changes anywhere in this plan.

### The manifest (Phase 0)

```sh
# userland/apps.manifest.sh — single source of truth for userland apps
#          name        dir              build  staged        ship             toolchain
app_row    hello       apps/hello       cargo  HELLO.ELF     built-every-run  rust-nightly
app_row    guilaunch   apps/guilaunch   cargo  GLAUNCH.ELF   built-every-run  rust-nightly
app_row    hello-cpp   apps/hello-cpp   make   HELLOCPP.ELF  built-every-run  musl-cxx
app_row    zsh         apps/zsh         make   ZSH.ELF       prebuilt-managed musl-cc
app_row    busybox     apps/busybox     make   BB.ELF        prebuilt-managed musl-cc
app_row    guidemo     apps/guidemo     cargo  GUIDEMO.ELF   built-every-run  rust-nightly   # Phase 1
app_row    notepad     apps/notepad     cargo  NOTEPAD.ELF   built-every-run  rust-nightly   # Phase 3
...fixture rows for compiler-compat / network-test (ship: test-fixture)
```

`stage-lib.sh` interprets rows generically: cargo rows build via the workspace (one `cargo build --release` covers all of them), make rows via `make -C`, prebuilt rows follow the existing rebuild-or-copy decision tree, fixture rows are staged only by `test.sh`.

### Sequencing dependency notes

Phase 0 has no dependency on the rest and de-risks every later "add an app" step. Phase 1 needs nothing from Phase 0 besides a manifest row for GUIDEMO (trivial either way). Phase 2 depends on Phase 1's ABI being final enough to mirror the event struct. Phase 3 depends on 1 + 2. Phases 0 and 1 can proceed in parallel in separate workspaces.

---

## Implementation Units

### U0. Manifest + shared staging library + refresh unification (Phase 0)
`userland/apps.manifest.sh`, `userland/stage-lib.sh` (subsuming `prebuilt-lib.sh`), rewire `build.sh`/`test.sh` staging blocks, generalize `refresh-prebuilt.sh`, shared readelf-EXEC helper, Rust build.rs dedup, README de-drift (R0.1–R0.6). Verify: clean-clone `./build.sh -n` and `./test.sh --skip-userland` stage identical artifact sets to today (byte-compare `host_share/`); `--rebuild-userland` still round-trips ZSH/BB.

### U1. GUI syscall ABI + RemoteSurface + event queue (Phase 1)
Syscall numbers/dispatch (`abi.rs`), handlers, `RemoteSurface`, per-process queue + `WaitingForGuiEvent` block/wake, PID cleanup, event struct + assertions (R1.1–R1.6, R1.8).

### U2. GUIDEMO validation app (Phase 1)
`userland/apps/guidemo/` + manifest row + manual smoke (R1.7). Gate for declaring the ABI usable.

### U3. Runtime expansion (Phase 2)
Syscall stubs, global allocator, `_start` argv/env, gui wrappers + mirrored event struct (R2.1). GUIDEMO rebased onto it as the consumer test.

### U4. Toolkit + shared font (Phase 2)
`userland/libs/gui` canvas/event-loop, `shared/fontdata` crate consumed by kernel + toolkit (R2.2–R2.4). Kernel side is a pure refactor — no rendering change.

### U5. Notepad app (Phase 3)
`userland/apps/notepad/` with editor model, menus, dialogs, working Save (R3.1, R3.2, R3.4).

### U6. Launch rewiring + kernel notepad deletion + docs (Phase 3)
guishell spawn helper, bin_namespace rewrite, gui_launch_table removal, delete `src/commands/notepad/`, guishell menu-count fix, smoke + docs (R3.3, R3.5–R3.7).

---

## Key Technical Decisions

### KTD1. Copy-blit first; shared-memory surface as a later upgrade
User-confirmed. Copy-blit needs zero mm changes and rides existing usercopy; the handle ABI hides the transport so the upgrade is non-breaking. Notepad-scale blit bandwidth is negligible.

### KTD2. Server-side decorations
User-confirmed. The kernel's `FrameWindow` keeps owning chrome, dragging, z-order, and focus; apps draw only their content area. Keeps window-management policy in one place and makes clients trivial.

### KTD3. OS-native GUI apps are no_std Rust on an expanded `runtime`
User-confirmed. Uses the nightly toolchain the kernel already requires — no musl cross-toolchain dependency for OS-native apps, and they join the existing Cargo workspace. musl/C remains the pattern for upstream ports (zsh, BusyBox).

### KTD4. Prebuilt only when slow or toolchain-gated (existing rule stands)
User-confirmed. Workspace Rust apps build in seconds with a universally-present toolchain; committing their binaries adds churn for nothing. Ship-kind is a manifest flag, so flipping any app later is a one-line change.

### KTD5. Per-process GUI event queue
User-confirmed. One blocking `gui_next_event` per process with the window handle inside each event handles multi-window apps (dialogs) naturally and avoids a wait-any primitive.

### KTD6. Font bytes shared via a common crate, not duplicated
User-confirmed. One `shared/fontdata` path crate consumed by kernel and toolkit; no drift between kernel and userland text rendering.

### KTD7. One umbrella plan; notepad only in the first migration
User-confirmed. Four phases in this document, each landable alone; `calc`/`painting`/`tasks`/`explorer` are follow-up plans.

### KTD8. Dialogs are userland, not kernel syscalls
Ring 3 already has `getdents64`/`stat`/`openat`; a file dialog is just another window. Keeps the kernel ABI at four calls.

### KTD9. Syscall numbers 5001–5004, continuing the 5000+ AgenticOS range
Follows the precedent set by `GUI_LAUNCH = 5000` (`abi.rs` reserves ≥5000 explicitly).

---

## Risks and Mitigations

### RK-1. Event encoding loses information the app needs
The compositor's `Event::Keyboard`/`Event::Mouse` must flatten into the POD struct (keycode + char + modifiers). If the kernel event types carry less than apps need (e.g. key-release, scroll deltas), extend the kernel input path first. Mitigation: GUIDEMO exists precisely to shake this out before notepad; the struct is versioned.

### RK-2. Callback/lock re-entrancy when pushing events
`RemoteSurface::handle_event` runs during window-manager event routing (inside the WM lock). Pushing to the PID queue and waking must not re-enter `with_window_manager`. Mitigation: the queue is process-table-side, not WM-side; wake helpers already run lock-free in the pty path — copy that discipline.

### RK-3. Process death vs. WM lock ordering
`cleanup_user_process` destroying windows could race compositor painting of the same surface. Mitigation: destruction goes through the same `with_window_manager` path any kernel app uses; the surface buffer is owned by the window object, so registry removal is sufficient.

### RK-4. no_std allocator bugs surface as heisenbugs in apps
Mitigation: keep the allocator dead simple (free-list over brk), add allocator unit tests runnable as a userland fixture, and lean on GUIDEMO → notepad as staged consumers.

### RK-5. Blocked-on-GUI process never wakes (lost wakeup)
Same hazard class the pty path already handles. Mitigation: reuse the restartable-syscall rewind pattern (re-check queue on re-dispatch), and make the wake unconditional on every enqueue.

### RK-6. Phase 0 silently changes staging behavior
Mitigation: U0's verification is a byte-compare of `host_share/` before/after on both `build.sh` and `test.sh` paths, plus a `--rebuild-userland` round-trip.

### RK-7. Kernel font refactor breaks kernel text rendering
Mitigation: `shared/fontdata` is a data-only move; kernel rendering code changes only its `use` path. Existing graphics tests + boot smoke cover it.

---

## System-Wide Impact

- **New public ABI surface** (syscalls 5001–5004 + event struct) that future apps depend on — versioned struct and documented pixel format from day one.
- **Process table** gains a GUI event queue + window-handle table per process; `cleanup_user_process` gains a teardown step.
- **`src/window/`** gains `RemoteSurface`; no changes to existing widgets or compositor scheduling.
- **`src/commands/`** shrinks by one app now, four later; `gui_launch_table` sync test churn.
- **Userland workspace** gains `libs/gui`, `apps/guidemo`, `apps/notepad`; `runtime` becomes a real dependency surface with its own compatibility expectations.
- **Build scripts** lose ~150 lines of duplicated staging logic in favor of the manifest + stage-lib.
- **Docs**: four CLAUDE.md files, two READMEs, and this plan's status updates.

## Open Questions

- Exact keycode/modifier encoding in the event struct (mirror the kernel's internal key event fields vs. a defined-in-ABI mapping) — settle during U1 with GUIDEMO as the consumer.
- Whether `gui_win_create` takes an initial background color (avoids a flash of garbage before the first present) — cheap to add, decide in U1.
- Allocator choice detail (hand-rolled free-list vs. vendored `linked_list_allocator`) — decide in U3; either satisfies R2.1.
- Window title updates ("filename — Notepad"): live with static title in v1, or pull `gui_win_set_title` (5005) forward into Phase 1 if it turns out trivial.

## Origin

Brainstormed 2026-07-17/18 from the request to extract `notepad` into a separately-compiled ELF run by the Start menu, and to establish maintainable patterns for the many OS-shipped ELF executables to come. Design decisions (KTD1–KTD7) confirmed by the user via Q&A. Exploration established that all five GUI apps are ring-0 kernel code behind `sys_gui_launch`, that no ring-3 windowing surface exists, and that the userland build system has three divergent shipping patterns needing unification.

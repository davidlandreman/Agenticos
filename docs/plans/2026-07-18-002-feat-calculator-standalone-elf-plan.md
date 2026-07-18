---
title: "feat: extract calculator into a standalone ring-3 ELF"
type: feat
status: planned
date: 2026-07-18
depth: medium
related_docs:
  - docs/plans/2026-07-18-001-feat-ring3-gui-platform-notepad-and-userland-unification-plan.md
  - userland/README.md
  - src/commands/CLAUDE.md
  - src/userland/CLAUDE.md
---

# feat: extract calculator into a standalone ring-3 ELF

## Summary

Move the calculator from `src/commands/calc/` into a no_std Rust application at
`userland/apps/calc/`, build it on every normal userland build, and stage it as
`/host/CALC.ELF`. Start → Calc and the synthetic `/bin/calc` entry will both
launch that ELF directly, following the path established by `NOTEPAD.ELF`.

The port preserves the existing four-operation calculator and 4×4 button grid,
but replaces its kernel widget tree, global `CALC_STATES` map, callbacks, and
busy-poll loop with an application-owned state machine and a blocking ring-3 GUI
event loop. Once the direct launch paths are in place, delete the kernel
`RunnableProcess` implementation and remove `calc` from `GLAUNCH.ELF` dispatch.

This is one focused migration. The ring-3 GUI ABI, runtime allocator, software
canvas, event queue, direct-app namespace, build manifest, and launcher helper
already exist because of the notepad extraction.

---

## Current state and motivation

`src/commands/calc/mod.rs` is roughly 450 lines of ring-0 application code. It:

- implements `RunnableProcess` and constructs kernel `FrameWindow`,
  `ContainerWindow`, `Label`, and `Button` objects directly;
- stores per-instance state in a global `Mutex<BTreeMap<usize, CalcState>>`
  because kernel button callbacks cannot borrow the owning application;
- replaces the display label in the window registry whenever its text changes;
- spins, calls `yield_if_needed`, and polls for window destruction instead of
  blocking on input;
- is launched from Start through `gui_launch_table::spawn_by_name("calc")`;
- is launched from zsh through `/bin/calc` → `/host/GLAUNCH.ELF` → syscall
  5000 → the same kernel launch table.

The notepad work made all of those constraints unnecessary. A native GUI ELF
can own an ordinary `CalcState`, draw into `gui::Canvas`, block in
`gui::next_event()`, and rely on per-process GUI cleanup when it exits. Moving
calculator now both shrinks the kernel and validates that the ring-3 platform is
a repeatable migration path rather than a notepad-specific one-off.

## Goals

1. Produce a standalone `CALC.ELF` from in-tree Rust source on every build.
2. Preserve the calculator's visible layout and calculation semantics:
   - four rows: `7 8 9 /`, `4 5 6 *`, `1 2 3 -`, `C 0 = +`;
   - addition, subtraction, multiplication, and division;
   - left-to-right pending-operation evaluation;
   - 12-character input cap, leading-zero replacement, and one decimal point;
   - integer formatting when practical, otherwise at most six fractional
     digits with trailing zeroes removed;
   - division by zero displays `Error`, and the next numeric input recovers.
3. Support mouse operation and useful keyboard equivalents without adding a
   new ABI: digits and operators use their character events, `Enter` performs
   equals, `C`/`c` or `Escape` clears, and `.` exposes the model's existing
   decimal-input support.
4. Keep every calculator instance independent with no global mutable app
   state.
5. Make both Start → Calc and zsh `calc` execute the direct ELF, not
   `GLAUNCH.ELF` or a kernel calculator process.
6. Remove the obsolete kernel calculator module and update live documentation
   and routing tests.

## Non-goals

- Scientific, programmer, history, memory-register, sign-toggle, percentage,
  localization, or arbitrary-precision modes.
- Changing the GUI syscall ABI or sharing user pages with the compositor.
- Building a general retained widget hierarchy in `userland/libs/gui`.
- Migrating `painting`, `tasks`, or `explorer`; they remain behind
  `GLAUNCH.ELF` and syscall 5000.
- Removing `GLAUNCH.ELF` or syscall 5000 while those apps still need them.
- Rewriting historical plans that accurately describe the system at the time
  they were written.

---

## Design

### Launch paths after the migration

```text
Start → Calc
  └─ guishell::spawn_calc
       └─ spawn_gui_user_app("/host/CALC.ELF", ["calc"])
            └─ launcher::launch_user_binary(...)

zsh: calc
  └─ execve("/bin/calc", ["calc"], envp)
       └─ bin_namespace direct rewrite → "/host/CALC.ELF"

CALC.ELF (ring 3)
  └─ gui_win_create → render/present → blocking gui_next_event loop
       └─ close → gui_win_destroy/exit → kernel per-PID GUI cleanup
```

The Start-menu path retains the small kernel wrapper thread required by
`launch_user_binary`'s blocking lifetime. The calculator itself is a ring-3
process and owns no kernel `RunnableProcess` implementation. A zsh launch uses
the normal fork/exec/wait path and does not create the legacy launcher shim.

### Application structure

Add `userland/apps/calc/` with the same package shape as `guidemo` and
`notepad`:

- `Cargo.toml`: binary package depending on `runtime` and `gui`, plus the
  `userland-build-support` build dependency;
- `build.rs`: `userland_build_support::configure("calc")`;
- `src/main.rs`: no_std/no_main entry point, model, rendering, hit-testing,
  blocking event loop, panic-to-exit handler.

The application should use three small concepts rather than reproduce the
kernel widget tree:

- `CalcState`: display string, accumulator, pending operation, and
  `clear_on_next`;
- a static button descriptor table containing label, row/column, action, and
  colors;
- `Calculator`: the `gui::Window`, `CalcState`, focus/pressed state if used,
  and event/render methods.

Button geometry and drawing stay app-local for this migration. The current
userland toolkit deliberately exposes a canvas rather than a widget tree, and
one fixed calculator grid is not enough evidence for a stable general-purpose
button API. If a later migration needs the same control behavior, it can lift
the proven descriptor/draw/hit-test shape into `libs/gui` then.

### State machine

Port the existing behavior into methods on the local state:

- `input_digit(character)` clears after an operator/result, replaces a leading
  zero, rejects a second decimal point, and enforces the display cap;
- `set_operator(op)` first evaluates an unconsumed pending operation, stores
  the current display as the accumulator, records the new operation, and marks
  the next numeric input to clear;
- `equals()` evaluates once and clears `pending_op`;
- `clear()` restores `0`, accumulator `0`, no pending operation, and no
  deferred clear;
- `evaluate_pending()` performs checked user-visible formatting, mapping
  non-finite results to `Error` and retaining the existing six-place trimming
  behavior.

Keep the model free of window handles and callbacks. Multiple app processes
then receive independent state naturally, and closing one cannot affect
another.

### Rendering and input

Create one server-decorated `gui::Window` titled `Calculator`. Render the
client surface from scratch only when state, focus, pointer state, or size
changes:

- dark panel background and display well;
- right-aligned display text, clamped so long/error text never underflows its
  x coordinate;
- the existing orange operators, green equals, red clear, and dark digit
  buttons;
- simple light/dark borders so button boundaries remain visible under both
  desktop themes.

Mouse-down inside a button activates its action. Key events act only on key
press, using the character and keycode fields already delivered by GUI ABI v1.
Close exits the loop. Resize resizes the canvas, recomputes or centers the
fixed grid with saturating dimensions, and presents again. Focus-change may
adjust the focused visual state but must not affect calculation state.

There is no idle polling and no `nanosleep`: `gui::next_event()` blocks the
ring-3 process until the kernel queues input, resize, focus, or close.

### Packaging and namespace

- Add `apps/calc` to `userland/Cargo.toml`.
- Add one built-every-run Cargo row to `userland/apps.manifest.sh`, staging
  `target/x86_64-unknown-none/release/calc` as `CALC.ELF`.
- Do not edit `build.sh` or `test.sh`; the manifest-driven staging library is
  the source of truth.
- Add `CALC_HOST_PATH = "/host/CALC.ELF"` to
  `src/userland/bin_namespace.rs`.
- Move `calc` from sorted `GUI_APPLETS` to sorted `DIRECT_APPLETS` and map it
  in `lookup_direct`. `/bin` listing, `stat`, `access`, and exec rewrite then
  continue to work through the existing direct-app machinery.

`DIRECT_APPLETS` becomes `&["calc", "notepad"]`; `GUI_APPLETS` becomes
`&["explorer", "painting", "tasks"]`. The lists remain sorted and disjoint
from BusyBox.

### Kernel cleanup

- Change `guishell::spawn_calc()` to call `spawn_gui_user_app` with
  `/host/CALC.ELF` and `argv[0] = "calc"`, matching `spawn_notepad()`.
- Remove the `calc` factory arm and test mirror entry from
  `src/commands/gui_launch_table.rs`.
- Remove `pub mod calc` from `src/commands/mod.rs`.
- Delete `src/commands/calc/mod.rs`.

The Start menu label/order/count do not change. The launch-table table and its
sync test continue to cover the three remaining kernel GUI applets.

---

## Implementation units

### U1. Standalone calculator app and build row

Create the no_std `apps/calc` package, implement the local calculation state,
canvas renderer, mouse hit-testing, keyboard mappings, resize handling, close
handling, and blocking event loop. Register the package in the userland Cargo
workspace and the app manifest as `CALC.ELF`.

Verification:

- `cargo build --manifest-path userland/Cargo.toml --release -p calc`
- confirm the staged/built artifact is x86-64 `ET_EXEC`, static, non-PIE, and
  has no `PT_INTERP` through the existing staging validation;
- `./build.sh -n` stages `host_share/CALC.ELF` without hand-written build-script
  changes.

### U2. Direct launch routing and kernel calculator removal

Add the direct namespace mapping, rewire Start → Calc, remove calc from the
legacy launch table, and delete the kernel module. Update namespace and launch
table tests in the same unit so no intermediate commit advertises a `/bin/calc`
path that cannot launch.

Verification:

- `/bin/calc` rewrites to `/host/CALC.ELF` and preserves `argv[0] = "calc"`;
- `/bin/notepad` still rewrites to `NOTEPAD.ELF`;
- `/bin/painting` still rewrites to `GLAUNCH.ELF`;
- merged `/bin` entries remain sorted, complete, and collision-free;
- the GUI launch table covers exactly `explorer`, `painting`, and `tasks`.

### U3. Tests, smoke coverage, and documentation refresh

Update live descriptions of the app inventory and launch paths in:

- `CLAUDE.md`;
- `src/commands/CLAUDE.md`;
- `src/process/CLAUDE.md` where calc is used as a kernel-process example;
- `src/userland/CLAUDE.md`;
- `userland/README.md`;
- `userland/apps/guilaunch/README.md`;
- module-level comments in `bin_namespace.rs` and `gui_launch_table.rs`.

Run `rg` for live `src/commands/calc`, kernel-side calc, and stale
`painting/calc/tasks/explorer` lists. Historical plans remain unchanged.

Automated verification:

```sh
cargo fmt --check
cargo fmt --manifest-path userland/Cargo.toml --check
cargo check
./test.sh bin_namespace gui_launch_table gui_userland
./build.sh -n
```

Manual QEMU smoke:

1. Start → Calc opens a window and leaves the desktop/compositor responsive.
2. Mouse sequence `1`, `2`, `+`, `7`, `=` displays `19`.
3. Keyboard sequence `9`, `/`, `0`, `Enter` displays `Error`; typing a digit
   recovers, and `Escape` clears to `0`.
4. Chained `2 + 3 * 4 =` displays `20`, preserving left-to-right semantics.
5. A decimal keyboard sequence such as `1 . 5 + 2 =` displays `3.5`.
6. Launch two calculators and verify their displays are independent.
7. Close one calculator; its window disappears, the other remains usable, and
   no orphan remote surfaces remain after process cleanup.
8. In zsh, `calc` opens the same ELF, the shell waits normally for its child,
   and the prompt returns after the calculator closes.
9. Start → Painting and zsh `painting` still exercise the legacy GLAUNCH
   path successfully.

---

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| Floating-point parsing/formatting increases the tiny ELF or exposes missing compiler intrinsics | Build the app in U1 before routing changes; preserve the simple existing f64 operations and release size profile. If an intrinsic is unresolved, replace only that operation with a small local helper rather than adding kernel support. |
| Small resize dimensions cause unsigned geometry underflow | Use `saturating_sub`, clamp text origins, and skip button hit regions that do not fit. |
| Namespace list drift sends `calc` through both direct and legacy paths | Move the name between lists and remove the launch-table arm in one unit; retain sorted/disjoint and dispatch tests. |
| Direct zsh launch changes lifetime from short-lived GLAUNCH shim to the real app child | This is intentional and matches notepad: zsh waits for the GUI process and resumes when the window closes. Verify both Start and shell paths manually. |
| Closing during input leaks a remote surface | Use `gui::Window` RAII plus existing process-death GUI cleanup; cover close and two-instance behavior in the smoke test. |

## Expected file changes

Add:

- `userland/apps/calc/Cargo.toml`
- `userland/apps/calc/build.rs`
- `userland/apps/calc/src/main.rs`

Modify:

- `userland/Cargo.toml`
- `userland/apps.manifest.sh`
- `src/userland/bin_namespace.rs`
- `src/commands/guishell/mod.rs`
- `src/commands/gui_launch_table.rs`
- `src/commands/mod.rs`
- the live documentation files listed in U3

Delete:

- `src/commands/calc/mod.rs`

No changes are expected in `src/userland/gui_syscalls.rs`,
`src/userland/gui.rs`, `src/window/windows/remote_surface.rs`, `build.sh`, or
`test.sh`.

## Done criteria

- `CALC.ELF` is built and staged by the manifest-driven userland pipeline.
- Start and `/bin/calc` both launch the standalone ring-3 application.
- Calculator mouse, keyboard, chaining, formatting, error recovery, resize,
  multi-instance, and close behavior pass the smoke matrix.
- `calc` is absent from `GUI_APPLETS`, `gui_launch_table`,
  `src/commands/mod.rs`, and the kernel source tree.
- The focused QEMU tests and build-only boot image complete successfully.
- Live documentation names three remaining kernel GUI apps and two direct GUI
  ELFs (`CALC.ELF`, `NOTEPAD.ELF`).

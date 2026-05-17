---
title: "feat: zsh as default terminal + userland launchers for GUI apps"
type: feat
status: active
date: 2026-05-16
---

# feat: zsh as default terminal + userland launchers for GUI apps

## Summary

Make the GUI terminal window spawn ring-3 `zsh` directly instead of the kernel-space cooperative shell loop, and move the kernel-side GUI app commands (`painting`, `calc`, `notepad`, `tasks`, `explorer`) behind a new userland launcher binary so zsh's PATH lookup launches them transparently via the existing `/bin/<applet>` namespace. Add a new syscall `sys_gui_launch(name)` that the kernel-side GUI apps stay behind. Delete the in-kernel command interpreter (`src/commands/shell/`), the 12 file-utility commands fully covered by BusyBox (`cat`, `head`, `tail`, `grep`, `wc`, `hexdump`, `echo`, `pwd`, `ls`, `dir`, `touch`, `time`), the `run` ELF launcher command (obsoleted by zsh's `execve`), and the command registry plumbing in `src/process/manager.rs`. Keep `guishell` (the desktop/taskbar manager, called from boot) untouched.

---

## Problem Frame

The kernel currently runs two parallel shell systems:

1. A **kernel-space cooperative shell** (`src/commands/shell/shell_process.rs`) that polls for terminal input, hand-parses commands, and dispatches them via a `BTreeMap<String, CommandFactory>` registry in `src/process/manager.rs`. The shell hardcodes `help`, `clear`, `cmd`, and `exit`; all other commands are kernel-space `RunnableProcess` implementations registered at boot in `src/kernel.rs::init`.
2. A **ring-3 Linux userland** with `ZSH.ELF` + `BB.ELF`. zsh has been the goal interactive shell since the userland bring-up. BusyBox already covers every file-utility command the kernel shell implements (`cat`, `ls`, `grep`, `head`, etc.) and resolves automatically via the kernel-side `/bin/<applet>` namespace (`src/userland/bin_namespace.rs`).

The kernel shell is the bootstrap fallback that hasn't been retired. Today a user must type `run /host/ZSH.ELF` to drop into zsh; the terminal window otherwise spins the cooperative poll loop forever. This plan finishes the migration: zsh becomes the default, and the kernel shell + 12 redundant file-utility commands disappear.

The remaining commands are not file utilities — they're GUI app launchers (`painting`, `calc`, `notepad`, `tasks`, `explorer`) whose `RunnableProcess::run` opens a new top-level window and runs an in-kernel event loop. They have no userland equivalent, and BusyBox doesn't ship anything resembling them. Three packaging options were considered:

1. **Taskbar-only** — drop typed entry points; users launch from the desktop taskbar that `guishell` already manages.
2. **Tiny command registry** — keep the registry only for these five names.
3. **Per-app launcher binaries** — small ring-3 ELFs that issue a syscall to spawn the existing kernel-side GUI app.

The user picked option 3 (see Key Technical Decisions). The launcher mirrors the BusyBox multicall trick: one shared `GLAUNCH.ELF` whose argv[0] selects which kernel-side app to spawn. Wiring it through `src/userland/bin_namespace.rs` means zsh's PATH lookup finds `painting` as `/bin/painting` without zsh-side configuration.

This plan keeps the GUI apps themselves kernel-space (no porting of windowing code to userland). Only the *launch surface* moves. That keeps the scope manageable while giving us a real userland on/off ramp for any future migration of the GUI apps to ring 3.

---

## Requirements

- **R1.** New syscall `sys_gui_launch(name_ptr: *const u8, name_len: usize) -> i32` in `src/userland/syscalls.rs`. Looks up `name` in a small kernel-side table mapping strings to `fn() -> Box<dyn RunnableProcess>`. On match, spawns the GUI app via the existing process spawn path and returns 0. On unknown name, returns `-ENOENT`. On spawn failure, returns `-ENOMEM` or `-EAGAIN` as appropriate. Syscall number chosen from the OS-internal range used by other AgenticOS-specific calls (NOT a Linux syscall number — Linux uses this slot for something else; pick from a range we own).
- **R2.** New userland app `userland/apps/guilaunch/` (Rust, `#![no_std]` + custom panic handler, static-musl-target like the kernel itself uses for ELF testing). The binary reads `argv[0]`, issues `sys_gui_launch(argv[0])`, exits with status 0 on success or non-zero on error. Size budget: ≤32 KiB stripped.
- **R3.** Build-on-every-run for `GLAUNCH.ELF` — mirrors `HELLO.ELF` / `HELLOCPP.ELF` (NOT prebuilt-managed like ZSH/BB). `build.sh` and `test.sh` build it fresh each invocation; no entry in `userland/prebuilt/`. Failure to build fails the overall build (it's small and fast enough that a soft-fail isn't needed).
- **R4.** Extend `src/userland/bin_namespace.rs`:
  - Add a second applet list `GUI_APPLETS: &[&str] = &["painting", "calc", "notepad", "tasks", "explorer"]`.
  - `apply_bin_rewrite("/bin/<gui_applet>")` returns `("/host/GLAUNCH.ELF", "<gui_applet>")` (parallel to the existing BusyBox path).
  - `getdents64` on `/bin` enumerates BOTH the BusyBox applets AND the GUI applets in sorted order.
  - `stat`/`access` on `/bin/<gui_applet>` succeeds with mode `0755`.
- **R5.** New TerminalWindow ↔ zsh integration in `src/window/terminal_factory.rs::spawn_terminal_with_shell`:
  - Replaces the current `register_terminal` + `shell_process::register_shell` calls.
  - Spawns `/host/ZSH.ELF` as a ring-3 process bound to the TerminalWindow's input/output (stdin reads from window key events, stdout writes to window text grid).
  - The exact stdio plumbing follows whatever path the current `run /host/ZSH.ELF` command uses today — this plan does NOT redesign the userland stdio surface, only generalizes its terminal wiring.
- **R6.** `src/kernel.rs::init_guishell_desktop` continues to call `init_guishell()` (the desktop/taskbar manager) but the default terminal it spawns runs zsh via R5, not the kernel shell.
- **R7.** Delete the following from `src/commands/`: `cat/`, `dir/`, `echo/`, `grep/`, `head/`, `hexdump/`, `ls/`, `pwd/`, `tail/`, `time/`, `touch/`, `wc/`, `shell/`, `run/`. That's 14 directories total. Update `src/commands/mod.rs` to remove `pub mod` lines.
- **R8.** Delete `src/process/manager.rs::register_command`, `execute_command`, `list_commands`, and the `CommandFactory` registry `BTreeMap`. Delete the corresponding `register_command(...)` block in `src/kernel.rs::init` for the 14 deleted commands. The five GUI commands (`painting`, `calc`, `notepad`, `tasks`, `explorer`) and `guishell` are not registered through this path after the cleanup — they're invoked only via `sys_gui_launch` (GUI apps) or directly from boot (`guishell`).
- **R9.** Keep the five GUI app `RunnableProcess` implementations (`src/commands/painting/`, `calc/`, `notepad/`, `tasks/`, `explorer/`) and `src/commands/guishell/` in place. They're now invoked only via `sys_gui_launch` (or boot, for guishell) rather than via the command registry. Their internal APIs don't change.
- **R10.** Add `src/userland/bin_namespace.rs` GUI rewrite tests + a syscall handler test for `sys_gui_launch` (success, unknown name, spawn failure). Existing BusyBox-applet tests must still pass.
- **R11.** End-to-end smoke (manual): boot, terminal opens directly into a zsh prompt (no "AgenticOS>" prompt visible), `ls /host` works, `which painting` prints `/bin/painting`, typing `painting` opens the painting window, typing `tasks` opens the task manager, `echo hello` works, `nonexistentcommand` produces zsh's standard "command not found" error.
- **R12.** Documentation updates:
  - `CLAUDE.md` — "Current State" reflects zsh as the default terminal shell; "Internal Commands" / project structure references to the 14 deleted dirs removed; `src/commands/` subsystem entry updated.
  - `src/commands/CLAUDE.md` — rewrite to reflect the GUI-app-only role.
  - `src/userland/CLAUDE.md` (if exists; create stub if not) — document `sys_gui_launch` and the GUILAUNCH multicall pattern.
  - `userland/README.md` — new "GUILAUNCH" subsection.
  - `docs/shell_window_integration.md` — update or note as historical (the integration model has changed).

---

## Scope Boundaries

### Outside this plan's scope

- **Porting GUI apps to userland.** `painting`, `calc`, `notepad`, `tasks`, `explorer` remain kernel-space. Moving them to ring 3 requires designing a windowing syscall API + a userland windowing SDK — a separate multi-PR effort.
- **Job control in zsh.** `bg`, `fg`, `jobs` already documented as `-ENOSYS` in the BusyBox plan; nothing changes here.
- **Multi-terminal stdio isolation.** If `spawn_terminal_with_shell` is invoked twice (e.g., by a future taskbar "new terminal" action), each must get its own zsh + isolated stdio. The current command-registry-spawned `cmd` is being deleted; if the taskbar relies on it for "new terminal," route it directly through `spawn_terminal_with_shell` instead. No new mechanism in this plan.
- **Filesystem writes.** Still read-only. zsh's `>` redirection still fails with EROFS. Not blocking — zsh tolerates it.
- **Replacing `guishell`.** The desktop/taskbar manager stays kernel-space and stays invoked from boot directly, not via the command registry. Migrating it to userland is a much larger plan.
- **A general `posix_spawn`-style launcher syscall.** `sys_gui_launch` is narrow on purpose — a string-keyed table of known apps, not an arbitrary kernel-process spawner.

### Deferred to Follow-Up Work

- **Returning PID from `sys_gui_launch`.** User picked 0/-errno for now. Adding PID later is forward-compatible (success path could return a non-negative PID instead of 0; callers that ignore the value still work).
- **Per-app launcher binaries instead of multicall.** If GUILAUNCH grows or the apps diverge in launch-time concerns (env vars, working dir, argv parsing), split into one ELF per app. At 5 apps with identical launch semantics, multicall is the right call.
- **Auto-derivation of the GUI applet list.** Right now `bin_namespace.rs` hardcodes the list. If/when GUI apps get added or removed frequently, drive the list from a single source-of-truth.
- **Removing the kernel-side GUI apps in favor of true userland windowing.** Tracked as a future cross-cutting plan.

---

## High-Level Technical Design

This illustrates the intended approach and is directional guidance for review, not implementation specification. The implementing agent should treat it as context, not code to reproduce.

### Launch flow (typing `painting` in zsh)

```
zsh: painting
 ├─ zsh PATH lookup:
 │    access("/bin/painting", X_OK)
 │     └─ kernel: bin_namespace::is_gui_applet("painting") → true → return 0
 └─ execve("/bin/painting", ["painting"], envp)
      └─ kernel execve_handler:
           1. normalize_path           → "/bin/painting"
           2. apply_bin_rewrite        → Some(("/host/GLAUNCH.ELF", "painting"))
           3. argv[0] := "painting"
           4. load_elf("/host/GLAUNCH.ELF") + enter_user_mode
      guilaunch _start (ring 3):
           argv[0] = "painting"
           sys_gui_launch("painting", 8)
            └─ kernel:
                 lookup "painting" in GUI_LAUNCH_TABLE → create_painting_process
                 spawn via the existing kernel-process path
                 return 0
           exit(0)
```

zsh's PATH lookup is unchanged from the BusyBox path. The new code path is the second arm of `apply_bin_rewrite` (which now distinguishes GUI applets from BusyBox applets) and the new `sys_gui_launch` syscall.

### Terminal-window ↔ zsh integration

```
init_guishell_desktop()
 └─ spawn_terminal_with_shell()
     ├─ spawn_terminal()  (creates TerminalWindow as today)
     └─ NEW: spawn_zsh_for_terminal(terminal_id)
         ├─ allocate a ring-3 process
         ├─ bind process stdin to TerminalWindow's input event channel
         ├─ bind process stdout/stderr to TerminalWindow's text grid
         ├─ load /host/ZSH.ELF + envp(PATH=/bin:/host, HOME=/, TERM=linux)
         └─ enter_user_mode
```

The stdio binding is the riskiest unknown. Today `run /host/ZSH.ELF` runs zsh successfully in a TerminalWindow, so the wiring exists — this plan generalizes it into a reusable seam. Implementation should locate the `run` command's stdio setup and extract it.

### Component sketch

```
userland/
└── apps/
    └── guilaunch/              # NEW
        ├── Cargo.toml          # crate-type bin, no_std, custom target
        ├── src/main.rs         # ~30 lines: read argv[0], issue syscall, exit
        ├── Makefile            # builds via cargo + strips
        └── README.md

src/userland/
├── bin_namespace.rs            # extended: GUI_APPLETS list + GUI arm in apply_bin_rewrite + getdents merging
├── syscalls.rs                 # NEW: sys_gui_launch handler + dispatch case
└── (loader.rs, lifecycle.rs unchanged for this plan)

src/window/
├── terminal_factory.rs         # extended: spawn_zsh_for_terminal, replaces register_shell call
└── (terminal.rs, terminal.rs.input wiring updated to reflect new stdio target)

src/commands/
├── cat/, dir/, echo/, grep/, head/, hexdump/, ls/, pwd/, tail/, time/, touch/, wc/   # DELETED
├── shell/                                                                            # DELETED
├── run/                                                                              # DELETED
├── painting/, calc/, notepad/, tasks/, explorer/, guishell/   # KEPT (invoked via sys_gui_launch / boot)
└── mod.rs                      # pub mod lines for deleted commands removed

src/process/manager.rs          # CommandFactory registry deleted; execute_command/list_commands/register_command deleted

src/kernel.rs::init             # 14 register_command() calls deleted; sys_gui_launch dispatch case added
```

### Sequencing dependency notes

R4 (bin_namespace GUI arm) and R1 (sys_gui_launch syscall) can be developed in parallel and independently tested. R5 (TerminalWindow → zsh) is independent of both — it could be done first to validate stdio plumbing before touching syscalls or bin_namespace. R7/R8 (deletions) come last, after the new path is verified end-to-end.

---

## Implementation Units

### U1. `sys_gui_launch` syscall + dispatch table

**Goal:** A new kernel syscall accepts an applet name and spawns the matching kernel-side GUI app process.

**Requirements:** R1, R9

**Dependencies:** None

**Files:**
- `src/userland/syscalls.rs` — add `sys_gui_launch_handler(name_ptr, name_len) -> i64`. Use the existing `copy_user_cstr` helper (or equivalent for sized buffers — check what `execve_handler` uses for argv strings).
- `src/userland/abi.rs` (or wherever the syscall number→handler dispatch lives) — add the new dispatch case. Pick a syscall number from the AgenticOS-internal range; document it inline.
- `src/commands/gui_launch_table.rs` — NEW small module that exposes `pub fn spawn_by_name(name: &str) -> Result<(), i32>`. Internal `&[(&str, fn() -> Box<dyn RunnableProcess>)]` table, sorted. Returns `-ENOENT` (38 or our chosen errno) on miss, propagates spawn errors.
- `src/commands/mod.rs` — `pub mod gui_launch_table;`
- `src/tests/userland/gui_launch.rs` — NEW test module.

**Approach:**
- The table lives in `src/commands/` (not `src/userland/`) because it references the kernel-side `RunnableProcess` factory functions for the GUI apps. `src/userland/bin_namespace.rs` only cares about names, not factories.
- Spawn path: reuse whatever the current `register_command` + `execute_command` path does internally for `RunnableProcess` (i.e., wrap in a kernel process, hand to the scheduler). The mechanism stays; only the dispatch entry point changes.
- The syscall handler does input validation (name length cap, valid UTF-8 if relevant, no null bytes) before calling `spawn_by_name`. Treat `name_len` over a small bound (e.g., 32) as `-EINVAL`.

**Patterns to follow:**
- Other recently-added kernel-internal syscalls (search `src/userland/syscalls.rs` for any AgenticOS-specific syscall — if none exists, this is the first; document carefully).
- `register_command` call sites in `src/kernel.rs::init` for what each GUI app's factory function is named (e.g., `create_painting_process`).

**Test scenarios:**
- `spawn_by_name("painting")` returns `Ok(())` and a `painting` process appears in the scheduler.
- `spawn_by_name("nonexistent")` returns `Err(-ENOENT)`.
- `sys_gui_launch_handler` with a null name_ptr returns `-EFAULT`.
- `sys_gui_launch_handler` with name_len > 32 returns `-EINVAL`.
- `sys_gui_launch_handler` with a name pointing to non-UTF8 bytes returns `-EINVAL`.

**Verification:** `./test.sh gui_launch` passes. The full suite (`./test.sh`) is still green.

---

### U2. `GLAUNCH.ELF` userland binary

**Goal:** A tiny static-musl ring-3 ELF that reads `argv[0]` and issues `sys_gui_launch`.

**Requirements:** R2, R3

**Dependencies:** U1 (the syscall must exist for the binary to be testable end-to-end, but the binary builds without it)

**Files:**
- `userland/apps/guilaunch/Cargo.toml` — crate-type `bin`, `[profile.release] opt-level = "z"`, panic = abort.
- `userland/apps/guilaunch/src/main.rs` — `#![no_std]`, `#![no_main]`, custom `_start` that reads argv[0] from the standard initial-stack layout, issues the syscall via inline asm (`int 0x80` or `syscall` instruction depending on what the kernel's ring-3 ABI uses), exits via the exit syscall.
- `userland/apps/guilaunch/Makefile` — `cargo build --release --target x86_64-unknown-linux-musl`, strip, copy to `userland/apps/guilaunch/build/guilaunch`.
- `userland/apps/guilaunch/README.md` — short note explaining the multicall pattern and why we don't use the C runtime.
- `userland/apps/guilaunch/.gitignore` — `build/`, `target/`.
- `build.sh` — add a `build_guilaunch` step alongside the HELLO/HELLOCPP builds, and stage to `host_share/GLAUNCH.ELF`.
- `test.sh` — same.

**Approach:**
- Look at how `HELLO.ELF` builds today (Rust, no_std, custom panic, custom `_start`). GUILAUNCH is the same pattern minus the print; reuse the entry-point and syscall-stub code wholesale.
- The syscall stub for `sys_gui_launch` is two `mov` + `syscall` + return — write inline asm rather than building a userland abstraction.
- Argv[0] parsing: at process start, RSP points at argc, then argv[0..argc] as pointers, then NULL, then envp, then NULL, then auxv. Read `*(rsp + 8)` for argv[0] pointer, then strlen.
- Use `core::str::from_utf8` to validate the name before passing it — or just pass the bytes through and let the kernel validate. Cheaper to let the kernel validate.

**Patterns to follow:** `userland/apps/hello/` (Rust HELLO.ELF).

**Test scenarios:**
- Build smoke: `make -C userland/apps/guilaunch` produces `build/guilaunch`, ET_EXEC, stripped, ≤32 KiB.
- Manual smoke (post-U3+U4): `/host/GLAUNCH.ELF painting` (run from zsh) opens the painting window.

**Verification:** `file build/guilaunch` reports `ELF 64-bit LSB executable, x86-64, statically linked, stripped`.

---

### U3. Extend `bin_namespace.rs` with GUI applets

**Goal:** `/bin/painting`, `/bin/calc`, etc. resolve to `/host/GLAUNCH.ELF` with `argv[0]` set to the applet name. `getdents64` on `/bin` lists both BusyBox and GUI applets in one sorted batch.

**Requirements:** R4, R10

**Dependencies:** None (independent of U1/U2; the rewrite is testable with mock load_elf)

**Files:**
- `src/userland/bin_namespace.rs` — extend:
  - Add `pub const GUI_APPLETS: &[&str] = &["calc", "explorer", "notepad", "painting", "tasks"];` (sorted).
  - Refactor `apply_bin_rewrite(normalized: &str) -> Option<(&'static str, &str)>` to return either `("/host/BB.ELF", name)` or `("/host/GLAUNCH.ELF", name)`.
  - Update `getdents64` synthesizer to merge both lists in sorted order (use `core::iter::merge` or manual two-pointer merge — at ~155 entries either is fine).
  - Update `is_applet` / `applet_count` helpers similarly.
- `src/tests/userland/bin_namespace.rs` — extend with GUI-applet cases.

**Approach:**
- The applet → binary mapping is now keyed on which list the name belongs to. Two `binary_search` calls (BusyBox first, then GUI) is simplest. Cost: O(log n + log m) ≈ 8 + 3 = 11 comparisons worst case. No need to merge into a single sorted list since GUI applet names don't collide with BusyBox names — but ADD a debug-only assertion at startup that the two lists are disjoint, to catch future drift.
- `getdents64` must emit entries in sorted order across both lists, not list-by-list. A merge step suffices; cache the merged sorted iteration or just merge on each call (called rarely; not a hot path).

**Patterns to follow:** The existing BusyBox arm of `apply_bin_rewrite`.

**Test scenarios:**
- `apply_bin_rewrite("/bin/painting")` returns `Some(("/host/GLAUNCH.ELF", "painting"))`.
- `apply_bin_rewrite("/bin/ls")` still returns `Some(("/host/BB.ELF", "ls"))` (no regression).
- `apply_bin_rewrite("/bin/nonexistent")` returns `None`.
- `getdents64` on `/bin` includes both `ls` (BusyBox) and `painting` (GUI) in sorted order.
- `access("/bin/painting", X_OK)` returns 0; `stat("/bin/painting")` returns mode `0755`.
- Disjoint-lists assertion (debug build): does not panic with the current lists.

**Verification:** `./test.sh bin_namespace` passes including new cases. Full suite green.

---

### U4. TerminalWindow ↔ zsh integration

**Goal:** `spawn_terminal_with_shell` spawns ring-3 zsh bound to the new TerminalWindow's stdio instead of registering a kernel-shell poll loop.

**Requirements:** R5, R6

**Dependencies:** None (independent of U1–U3, but smoke testing requires them)

**Files:**
- `src/window/terminal_factory.rs::spawn_terminal_with_shell` — replace the `register_terminal` + `shell_process::register_shell` calls with `spawn_zsh_for_terminal(terminal_id)`.
- `src/window/terminal_factory.rs` — new helper `spawn_zsh_for_terminal(terminal_id) -> Result<ProcessId, &'static str>`. Loads `/host/ZSH.ELF` via the existing ELF loader, allocates a ring-3 process with stdio bound to the window.
- `src/userland/stdin.rs` and `src/userland/tty.rs` — likely need a way to bind a process's stdin/stdout/stderr to a specific WindowId. If `src/commands/run/mod.rs` already does this for the existing `run /host/ZSH.ELF` flow, extract the binding into a reusable function.
- `src/kernel.rs::init_guishell_desktop` — no change required if `spawn_terminal_with_shell` continues to be called.

**Approach:**
- **First step is investigation, not coding.** Read `src/commands/run/mod.rs` to see how stdio is bound today when the user types `run /host/ZSH.ELF`. The output text appears in the TerminalWindow, and keystrokes reach zsh — so the binding exists. Find it. Extract it into a function that can be called from `terminal_factory.rs` directly.
- The TerminalWindow needs to know that its stdio is now backed by a userland process, not a polled kernel shell. This may affect how keystrokes are routed (today they go through `shell_process::poll`).
- Process lifetime: when zsh exits (user types `exit`), the TerminalWindow should close. Hook this into the existing `close_terminal` path — when the bound userland process exits, the terminal factory receives a notification and tears down the window. If no exit-notify mechanism exists today, this is a real new piece of work; flag it.
- Initial envp: `PATH=/bin:/host`, `HOME=/`, `TERM=linux` (matches the BusyBox plan's R8).
- If `/host/ZSH.ELF` is missing, surface a clear "zsh not found" message in the TerminalWindow rather than panicking. This is the only fallback path that remains after the kernel shell is deleted.

**Patterns to follow:** `src/commands/run/mod.rs` for the existing stdio binding. The existing `run` command's `enter_user_mode` call shows the canonical launch sequence.

**Test scenarios:**
- Manual: boot, terminal window opens, zsh prompt visible (`$ ` or whatever zsh's default prompt is), keystrokes echo, `ls /host` produces output.
- Manual: type `exit` in zsh; window closes cleanly without leaving a dangling process or window.
- Manual: simulate missing ZSH.ELF (remove from host_share, boot); window shows error message, no panic.
- Kernel test: spawn_zsh_for_terminal returns a valid ProcessId when called with a real terminal_id and ZSH.ELF present.

**Verification:** Boot interactively; smoke-test the manual scenarios.

---

### U5. Delete the kernel shell, file-utility commands, `run`, and the command registry

**Goal:** All code paths for the old in-kernel command interpreter are removed. The kernel boots, zsh runs, GUI apps launch via U1–U4, no dead code remains.

**Requirements:** R7, R8

**Dependencies:** U1, U2, U3, U4 (deletions are the last step)

**Files:**
- `src/commands/` — delete subdirectories: `cat/`, `dir/`, `echo/`, `grep/`, `head/`, `hexdump/`, `ls/`, `pwd/`, `tail/`, `time/`, `touch/`, `wc/`, `shell/`, `run/`.
- `src/commands/mod.rs` — remove `pub mod` lines for deleted commands.
- `src/process/manager.rs` — delete `register_command`, `execute_command`, `list_commands`, the `CommandFactory` type, the registry `BTreeMap`. Keep `get_process_list`, `allocate_pid`, and anything else the GUI apps + scheduler still need (read carefully — `tasks` uses `get_process_list`).
- `src/kernel.rs::init` — delete the `register_command(...)` block for the 14 commands.
- `src/window/terminal.rs::register_terminal` — keep if still used by U4's zsh stdio binding; delete if not.
- `src/commands/shell/shell_process.rs` (deleted with `shell/`) — `register_shell`, `poll`, `process_command` all go.

**Approach:**
- Delete in a single commit so the kernel compiles cleanly at HEAD; otherwise reviewers see broken intermediate states. Use `cargo check` between each `pub mod` removal to catch leftover refs.
- Watch for transitive uses: `src/commands/<name>/` modules may be referenced from `src/commands/mod.rs`, kernel init, tests in `src/tests/`, or doc comments. `grep -rn "commands::shell\|commands::cat\|commands::run" src/` catches most.
- `src/commands/tasks/` uses `crate::process::get_process_list()` — this is a `ProcessManager` method, not part of the command registry, so it survives.
- `src/window/CLAUDE.md` mentions terminal-factory wiring to "the shell" — update once U4 lands.

**Patterns to follow:** Any prior PR that removed a kernel subsystem cleanly — search git log for `removal` or `cleanup` to find precedent for the deletion style.

**Test scenarios:**
- `cargo check` passes after every individual `pub mod` removal.
- `cargo build --release` passes at the end of the unit.
- `./test.sh` passes (the test suite must not have referenced the deleted commands).
- Manual: boot still works, terminal still shows zsh, GUI apps still launch.

**Verification:** Final `cargo build --release` + `./test.sh` + interactive boot smoke.

---

### U6. End-to-end smoke + docs

**Goal:** New default workflow is verified interactively and documented.

**Requirements:** R11, R12

**Dependencies:** U1–U5

**Files:**
- `CLAUDE.md` — Current State paragraph updated (zsh is the default terminal; the in-kernel command interpreter is removed); Project Structure index updated (no more `src/commands/shell/`, etc.); Known Issues updated if anything from the deleted-command list was previously called out.
- `src/commands/CLAUDE.md` — rewrite. Now: "GUI app launchers (kernel-side `RunnableProcess` impls), invoked from userland via `sys_gui_launch` or from boot for `guishell`. NOT a shell command system anymore."
- `src/userland/CLAUDE.md` — create if missing; document `sys_gui_launch`, the GUILAUNCH multicall pattern, and the `/bin/<gui_applet>` entries alongside the existing BusyBox entries.
- `userland/README.md` — new "GUI app launchers" subsection.
- `docs/shell_window_integration.md` — either update to describe the zsh integration, or add a header noting the document is historical (was about the deleted kernel shell) and pointing readers to the new design.

**Approach:**
- Walk through R11's smoke list interactively.
- Don't expand the docs beyond what's necessary — the BusyBox plan's pattern is to keep CLAUDE.md short and push detail into the relevant subsystem docs.

**Test scenarios (manual, R11):**
- Boot. Terminal window opens directly into zsh prompt (no "AgenticOS>" prompt). PASS if zsh's `$ ` prompt is visible.
- `ls /host` produces the staged file list. PASS if `ZSH.ELF`, `BB.ELF`, `GLAUNCH.ELF`, `HELLO.ELF` all appear.
- `which painting` prints `/bin/painting`.
- `painting` (typed in zsh) opens the painting window. The zsh process continues running afterward — the launcher exits, painting runs concurrently.
- `tasks`, `calc`, `notepad`, `explorer` likewise.
- `echo hello | wc -c` prints `6` (or matches BusyBox `wc -c` semantics).
- `nonexistentcommand` produces zsh's standard "command not found" error.
- Type `exit` in zsh; terminal window closes cleanly.

**Verification:** All manual smoke items pass. `./test.sh` is still green.

---

## Key Technical Decisions

### KTD1. Single GUILAUNCH multicall binary, not five per-app binaries

User picked option 3 (per-app launchers) from the discussion; the multicall variant is the natural implementation. Reasons:
- All five apps have identical launch semantics — load a kernel-side `RunnableProcess`, no app-specific argv parsing, no app-specific environment.
- BusyBox's multicall pattern is already deeply familiar to the codebase (kernel-side argv[0] rewriting in `bin_namespace.rs`); reusing it costs nothing.
- One binary to build, ship, stage, and update if the syscall ABI changes.
- If the apps diverge later (one needs a working dir, one needs a different env), split then. YAGNI now.

Trade-off accepted: same argv[0]-deviation-from-Linux quirk as BusyBox (caller's argv[0] is overwritten by the kernel rewrite). Documented at the rewrite site.

### KTD2. Rust for the launcher binary

User picked Rust over C. Justification:
- Consistency with `HELLO.ELF` (the existing Rust userland reference).
- Same toolchain (cargo + the kernel's nightly Rust) — no additional build dependency.
- The binary is ~30 LOC; the language choice is mostly aesthetic at this size, but Rust gets us safer pointer arithmetic for argv parsing.

### KTD3. Build-on-every-run, not prebuilt-managed

User picked build-every-run. Reasons:
- Binary is tiny (~10 KiB expected) — build time is negligible.
- Avoids the `refresh-prebuilt.sh` dance when the syscall ABI or applet list changes.
- Fresh clones don't get a faster boot from prebuilding GUILAUNCH because the kernel itself takes orders of magnitude longer to compile anyway.

### KTD4. Syscall returns `0/-errno`, not PID

User picked `0/-errno` over returning the spawned process's PID. Reasons:
- Launcher only needs to know success/failure; it exits immediately after.
- No current consumer for the PID (no shell `wait` for kernel processes, no `fg`/`bg`).
- Forward-compatible: a future revision can return a non-negative PID on success without breaking callers that just check `>= 0`.

### KTD5. One combined PR, not two sequential

User picked combined. Reasons:
- The deletions in U5 are the riskiest part, and they're hard to reason about without seeing the full replacement path landed. A reviewer wants to see "the kernel shell goes away AND here's what replaces it" in one diff.
- The two halves are coupled in the failure modes (if zsh fails to launch, the kernel shell isn't there as a fallback) — splitting the PR means a window of time where the system is degraded relative to either before-state or after-state.
- A reviewer asked to approve only U1–U4 without U5 would reasonably ask "why land this if the old code is still there?" — the value is in the cleanup.

Trade-off accepted: bigger diff. Mitigated by clear unit-level commits within the PR (one commit per U-unit).

### KTD6. Keep GUI apps kernel-side; move only the launch surface

The "right" long-term answer is to port `painting`, `calc`, etc. to ring-3 userland and design a windowing syscall API. That's a major effort (probably multi-PR). This plan does the cheap, high-value step: get zsh in front of the user, get the kernel shell out, give the GUI apps a userland launch path that doesn't depend on the old command registry. The GUI apps themselves stay kernel-space until a separate plan picks up the porting work.

---

## Risks and Mitigations

### R-1. TerminalWindow ↔ zsh stdio binding is non-trivial to generalize

**Likelihood:** Medium-high — this is the riskiest part of the plan. The current `run` command works, but the stdio binding may be entangled with the run command's particulars.

**Impact:** High — if zsh's stdout doesn't reach the TerminalWindow or stdin doesn't reach zsh, the terminal is dead.

**Mitigation:** Do U4 first (in any order, but BEFORE U5's deletions). Get zsh working via `spawn_terminal_with_shell` while the kernel shell is still available as a side-by-side comparison. Only delete the kernel shell once U4 is verified.

### R-2. The `cmd` command (deleted) was the only way to open a second terminal

**Likelihood:** Certain.

**Impact:** Low if no current workflow depends on multiple terminals; medium if the taskbar exposes a "new terminal" button that called `cmd`.

**Mitigation:** Audit the taskbar (in `src/commands/guishell/` or wherever the taskbar UI lives) for any callers of `execute_command("cmd")`. If found, route directly through `spawn_terminal_with_shell` instead.

### R-3. Zsh process exit doesn't tear down the TerminalWindow

**Likelihood:** Medium — the existing `run /host/ZSH.ELF` flow may not propagate exit cleanly because the window outlives the run command anyway.

**Impact:** Medium — orphan windows accumulate; user can't close terminal via `exit` (would have to use the window close button).

**Mitigation:** During U4, explicitly wire the process-exit path to `close_terminal(terminal_id)`. If no such hook exists, this is a real new feature; flag it and decide whether to do it in this PR or defer.

### R-4. Tests reference the deleted commands

**Likelihood:** Low — kernel tests primarily target subsystems, not shell commands.

**Impact:** Low — `cargo build --features test` fails loudly; easy to fix.

**Mitigation:** Run `./test.sh` after U5; fix any references. If a test was using a deleted command as a stand-in for "spawn a kernel process," rewrite it to use the GUI app spawn path or a dedicated test process.

### R-5. zsh fails to launch (e.g., loader regression) and there's no fallback

**Likelihood:** Low at steady state; medium during U4 development.

**Impact:** Catastrophic during dev (no usable terminal); medium in production (boot reaches GUI but terminal is unusable).

**Mitigation:** Keep the kernel shell available during U4 development (i.e., do U4 with the shell still present, validate, THEN delete). Post-U5, ensure the TerminalWindow shows a clear error message ("zsh failed to launch: <reason>") if ZSH.ELF is missing or load fails — degrade visibly, don't panic.

### R-6. `apply_bin_rewrite` regression: BusyBox applets stop working

**Likelihood:** Low — the refactor in U3 is mechanical.

**Impact:** High — `ls`, `cat`, etc. would all break.

**Mitigation:** Keep all existing BusyBox bin_namespace tests; add the GUI tests as parallel cases. Run the full BusyBox smoke from `2026-05-16-002` plan's R10 after U3.

---

## System-Wide Impact

- **`src/userland/`** gains one syscall and one applet-list extension. The new syscall is opt-in (other code paths unchanged).
- **`src/commands/`** shrinks significantly: 14 directories deleted, the registry plumbing removed. The remaining six directories (`painting`, `calc`, `notepad`, `tasks`, `explorer`, `guishell`) keep their internal APIs untouched, but their invocation surface moves from "shell command name" to "syscall name" (or boot, for guishell).
- **`src/process/manager.rs`** loses the command registry. `ProcessManager` keeps its scheduler / PID-allocation responsibilities.
- **`src/window/terminal_factory.rs`** swaps the kernel-shell registration for a zsh process spawn. `src/window/terminal.rs` may need adjustments for stdio routing (TBD during U4).
- **`src/kernel.rs::init`** loses ~14 lines of `register_command` calls.
- **Documentation** updates in 4–5 places; pattern matches prior userland-plan deletions.
- **No changes to** `src/fs/`, `src/drivers/`, `src/mm/`, `src/graphics/`, `src/window/manager.rs`, `src/arch/`, `src/input/`.

Net code reduction is significant: 14 command dirs + the registry + the kernel shell adds up to roughly 1500–2500 LOC removed (estimate based on the count of files). New code is small: one syscall handler, one GUILAUNCH binary, ~50 lines of `bin_namespace.rs` extension, ~30 lines of terminal_factory changes.

---

## Open Questions

- **Q1.** Does any current code call `execute_command(...)` programmatically (not via user-typed shell input)? `grep -rn "execute_command" src/` during U5. Likely callers: the `cmd` command (being deleted), possibly a test, possibly the taskbar. Each call site needs a per-case decision. **Resolution: investigate at start of U5; expected to be small.**
- **Q2.** Does the TerminalWindow's input event handling currently call into `shell_process::poll`? If so, U4 must rewire it to deliver keystrokes to the bound userland process's stdin instead. **Resolution: investigate as part of U4; this is the core of the U4 work.**
- **Q3.** What syscall number to assign `sys_gui_launch`? Linux syscall numbers are taken; the kernel needs an OS-internal range. Check `src/userland/abi.rs` / `syscalls.rs` for prior art. **Resolution: pick during U1 implementation; document inline.**
- **Q4.** Should `sys_gui_launch` be allowed to spawn `guishell`? Currently `guishell` is the desktop manager, called from boot. If a future workflow needs to restart it, the table could include it — but spawning the desktop manager from userland feels like a footgun. **Resolution: exclude `guishell` from the GUI_LAUNCH_TABLE for now. Document.**
- **Q5.** Should there be a `which`-style fallback for unknown applets? Today if the user types `painting` and the GUILAUNCH ELF is missing, zsh would report a misleading error. **Resolution: rely on the same staging-failure path as BB.ELF — if GLAUNCH.ELF is missing, the kernel log warns at boot and `painting` fails with a clear error from execve. No special-case handling.**

---

## Origin

This plan was generated from a chat exchange between the user and the AI. The kernel-side prerequisites — zsh (`2026-05-09-003`), BusyBox + `/bin/<applet>` namespace (`2026-05-16-002`), the demand-grown user stack (`2026-05-16-003`), and the signal-handling work bundled with that — are all in `main` as of 2026-05-16.

# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## AI context layout

This project splits AI-agent context into three layers, loaded with different timing:

- **`.claude/rules/*.md`** — project-wide rules. Loaded eagerly at session start. Apply regardless of which folder is being touched (`no_std`, panic handler, testing flow).
- **`CLAUDE.md`** at the repo root — this file. Orientation, build commands, and the directory index.
- **`src/<subsystem>/CLAUDE.md`** — subsystem context. Loaded on demand when Claude reads a file in that directory.

See `docs/ai-context-conventions.md` for the convention in detail (when to add a new folder file, what shape they follow, why no frontmatter on rules, etc.).

## Project Overview

AgenticOS is a Rust-based operating system targeting Intel x86-64 architecture. This project implements a bare-metal OS from scratch with the eventual goal of supporting agent-based computing capabilities.

**Current State**: The OS has a solid foundation with memory management, filesystem support, display/graphics, and basic process management. A window system provides hierarchical window management, event routing, and mouse support. The OS boots into a GUI desktop with a blue background; clicking Start → Terminal opens a windowed terminal that launches ring-3 `zsh` (`/host/ZSH.ELF`) directly as its shell. A real ring-3 userland runs Linux static-musl binaries: `zsh` is the interactive shell, a static BusyBox (`BB.ELF`) provides ~240 coreutils applets via a kernel-side virtual `/bin/<applet>` namespace (`src/userland/bin_namespace.rs`), and a small `GLAUNCH.ELF` multicall binary surfaces the kernel-side GUI apps (`painting`, `calc`, `notepad`, `tasks`, `explorer`) at `/bin/<name>` so zsh's PATH lookup spawns them via the AgenticOS-internal `sys_gui_launch` syscall. Write-side BusyBox applets (`cp`, `mv`, `rm`, …) surface `EROFS` at runtime because the FS is read-only. The "Agentic" aspects (agent runtime, advanced process management) are not yet implemented.

The legacy kernel-side command interpreter (the `shell/` process that hand-parsed commands) and its hardcoded utilities (`cat`, `ls`, `grep`, `pwd`, `wc`, `hexdump`, `echo`, `dir`, `head`, `tail`, `time`, `touch`, `wc`, `run`) were removed when zsh became the default — see `docs/plans/2026-05-16-004-feat-zsh-default-terminal-and-gui-launchers-plan.md`. Type those names in zsh and BusyBox handles them.

## Common Commands

### Build and Run
- `./build.sh` — Build kernel in release mode, create disk images, and run in QEMU (recommended)
- `./build.sh -c` — Clean build (removes all artifacts first)
- `./build.sh -d` — Build in debug mode (larger kernel, slower boot, more symbols)
- `./build.sh -n` — Build only, don't run QEMU
- `./build.sh --rebuild-userland` — Recompile prebuilt-managed userland apps (zsh, future Linux ports) from source instead of copying the committed `userland/prebuilt/<NAME>.ELF`. Same flag works on `test.sh`. Env equivalents: `REBUILD_USERLAND=1`, or per-app `REBUILD_ZSH=1`. See `userland/prebuilt/README.md`.
- `./build.sh -h` — Show help and usage
- `cargo build` — Build the kernel only (won't create disk images)
- `cargo build --release` — Build optimized release version

**Prebuilt userland ELFs**: `ZSH.ELF` (and any future Linux ports that fetch upstream tarballs) ship as committed binaries under `userland/prebuilt/`. Fresh clones boot a working zsh without the `x86_64-linux-musl-cross` toolchain installed. `HELLO.ELF` (Rust) and `HELLOCPP.ELF` (small C++ wrapper) are NOT prebuilt — they build from source on every run. After changing the upstream source / Makefile / patches of a prebuilt-managed app, run `./userland/refresh-prebuilt.sh` and commit the updated binary alongside the source change.

**QEMU Configuration**: 128 MiB RAM, serial output, VirtIO tablet for seamless mouse, `isa-debug-exit` for test integration.

### Testing
- `./test.sh` — Run all kernel tests in QEMU with automatic exit
- `./test.sh arc heap` — Run only the listed test modules
- `./test.sh 'arc::test_weak*'` — Glob within a module
- `./test.sh -l` — List available modules and exit
- `./test.sh --skip-userland` — Skip the userland prebuild (faster iteration)
- `./test.sh --rebuild-userland` — Force-recompile prebuilt-managed userland apps (see Build and Run)
- `cargo build --features test` — Build kernel with test features enabled

Tests run automatically on kernel boot when built with the test feature. QEMU exits with success/failure codes via `isa-debug-exit`. The filter is delivered at runtime via QEMU `fw_cfg`, so changing it does not trigger a kernel rebuild. See `.claude/rules/testing-flow.md` for exit-code semantics and filter syntax, and `src/tests/CLAUDE.md` for how to add a new test or topic module.

### Code Quality
- `cargo fmt` — Format code
- `cargo clippy` — Lint
- `cargo check` — Quick compilation check (preferred for validating code changes — avoids producing binaries)

### Parallel development with Conductor
This repo is configured for [conductor.build](https://www.conductor.build) — see `docs/conductor-workflow.md` for the full reference. Lifecycle is declared in `conductor.json`; `.conductor/setup.sh` bootstraps a workspace, `.conductor/run.sh` invokes `./build.sh`, `.conductor/archive.sh` cleans up QEMU on teardown. Each Conductor workspace is a git worktree with its own `target/` and QEMU process; the compound-engineering plugin is enabled via the committed `.claude/settings.json`. When proposing or evaluating cross-cutting changes, point the user at `docs/conductor-workflow.md` rather than re-deriving the workflow.

## Project Structure

The project follows a modular monolithic kernel design with clear separation of concerns. All code runs in kernel space (ring 0) with no user/kernel boundary yet.

### Top-level core files
- `src/main.rs` — Minimal kernel entry point (< 25 lines)
- `src/kernel.rs` — Kernel initialization and boot sequence
- `src/panic.rs` — Custom panic handler
- `src/bootloader_config.rs` — Bootloader configuration

### Subsystem index
Each entry below points to the folder's own `CLAUDE.md`, which carries the detailed context for that subsystem. Folder files load on demand when Claude reads files in that directory.

- `src/arch/` — Architecture-specific code (x86_64 IDT, interrupts). No folder file yet — currently thin.
- `src/commands/` — Kernel-side GUI app launchers (`painting`, `calc`, `notepad`, `tasks`, `explorer`) + `guishell` (desktop/taskbar manager). Invoked via `sys_gui_launch` from ring-3 (`GLAUNCH.ELF`) or directly from boot in the case of `guishell`. See [`src/commands/CLAUDE.md`](src/commands/CLAUDE.md).
- `src/drivers/` — Hardware drivers (PCI, IDE, PS/2, VirtIO, framebuffer display). See [`src/drivers/CLAUDE.md`](src/drivers/CLAUDE.md).
- `src/fs/` — Read-only FAT12/16/32 filesystem with `Arc`-based handles. See [`src/fs/CLAUDE.md`](src/fs/CLAUDE.md).
- `src/graphics/` — Drawing primitives, text rendering, image loading, compositor. See [`src/graphics/CLAUDE.md`](src/graphics/CLAUDE.md).
- `src/input/` — Lock-free input pipeline (SPSC queue, scancode state machines). See [`src/input/CLAUDE.md`](src/input/CLAUDE.md).
- `src/lib/` — Custom `Arc`, debug logging, `Testable` trait. See [`src/lib/CLAUDE.md`](src/lib/CLAUDE.md).
- `src/mm/` — Frame allocator, heap allocator, paging, page-fault demand mapping. See [`src/mm/CLAUDE.md`](src/mm/CLAUDE.md).
- `src/process/` — Process traits and the live preemptive scheduler. (The shell-command registry that used to live here was removed when zsh became the default terminal.) See [`src/process/CLAUDE.md`](src/process/CLAUDE.md).
- `src/stdlib/` — `Read`/`Write` traits, async waker. No folder file yet — currently thin.
- `src/tests/` — In-kernel test modules. See [`src/tests/CLAUDE.md`](src/tests/CLAUDE.md).
- `src/userland/` — Ring-3 ELF loader, Linux x86-64 ABI, lifecycle (`enter_user_mode`, `cleanup_user_process`, `BinaryLoadGuard`). No folder file yet; design lives in `docs/plans/2026-05-08-004-feat-userland-app-platform-plan.md` and `docs/plans/2026-05-09-001-feat-userland-linux-abi-cpp-hello-plan.md`.
- `src/window/` — Window system (hierarchy, types, default desktop, cursor rendering). See [`src/window/CLAUDE.md`](src/window/CLAUDE.md).

### Configuration files
- `Cargo.toml` — Project manifest
- `rust-toolchain.toml` — Nightly Rust with required components
- `.cargo/config.toml` — Build configuration and target settings
- `x86_64-agenticos.json` — Custom target specification

### Documentation
- `docs/ARCHITECTURE.md` — Detailed architecture documentation
- `docs/IMPLEMENTATION_PLAN.md` — Phased development roadmap
- `docs/ai-context-conventions.md` — How AI-agent context files are organized in this repo
- `docs/conductor-workflow.md` — Conductor workspace lifecycle reference
- `docs/window_system_design.md` — Window system architecture and implementation status
- `docs/shell_window_integration.md` — Shell/terminal window integration design
- `docs/solutions/learnings/` — Post-mortems and patterns from prior debugging journeys. Read the relevant one before touching adjacent code. The `2026-05-09-multi-mib-user-binary-load.md` learning covers the seven-issue chain that made multi-MiB user binaries appear to hang under interactive boot (frame allocator, hot-path logging, `read_to_vec` zero-fill, FAT temp buffer, SSE enable, GUI/render contention, IDE PIO atomicity).
- `README.md` — Project README

## Known Issues and Technical Debt

These are cross-cutting (not subsystem-local). Subsystem-specific known issues live in the relevant folder file (e.g., the graphics refactor list lives in `src/graphics/CLAUDE.md`).

### Current Limitations
1. **No Multitasking** — Everything runs synchronously in kernel space.
2. **Read-Only Filesystem** — No write support yet (in-progress per `docs/plans/2026-05-16-005-feat-filesystem-write-and-long-names-plan.md`). LFN read + mixed-case lookup shipped 2026-05-16.
3. **Limited Test Coverage** — Many subsystems lack comprehensive tests.
4. **Global State** — Heavy use of `static mut` and `lazy_static`.
5. **No User Space** — Everything runs in ring 0 (kernel mode).
6. **Constant Window Repainting** — `TextWindow` repaints unnecessarily in some paths.

### Areas Needing Refactoring
1. **Graphics Subsystem** — Complex relationships between display modules. (Detail in `src/graphics/CLAUDE.md`.)
2. **Error Handling** — Inconsistent use of `panic!` vs `Result`.
3. **Command System** — Could benefit from better parsing/validation.
4. **Mouse Integration** — Cursor rendering tightly coupled to display.

### Deferred from the zsh-interactive bring-up
Bundled with `nosuchcommand` / `ls`-from-zsh fixes. Each is non-blocking for basic interactive zsh, but the next workload that exercises them will hit the gap.

1. ~~**Demand-grown user stack**~~ — **resolved** by `docs/plans/2026-05-16-003-feat-userland-demand-grown-stack-plan.md`. The ring-3 page-fault handler now grows the stack on demand (`src/userland/lifecycle.rs::try_grow_user_stack`), capped per-process by `USER_STACK_MAX_GROWTH_PAGES` and per-binary by `highest_pt_load_end + USER_STACK_GUARD_PAGES * 0x1000`. Initial commit is `USER_STACK_INITIAL_PAGES = 8` pages (down from the 64-page eager mapping).
2. ~~**Signal mask not restored on `rt_sigreturn`** (sa_mask path)~~ — **resolved** for the regular delivery path. `deliver_signal` (`src/userland/syscalls.rs`) now writes the pre-delivery `signal_state.blocked` into the signal frame between the saved `UserState` and the signum word, and `rt_sigreturn_handler` restores it. `maybe_deliver_signal` installs the POSIX handler mask (`old | sa_mask | bit(signum)`, stripping SIGKILL/SIGSTOP) atomically with the consume, via the new `handler_blocked_mask` helper. The `rt_sigsuspend` "restore pre-suspend mask" gap is **still open** — sigsuspend saves the mask it installs, not the mask that was active before the call, so a handler delivered during sigsuspend sees the new mask restored on rt_sigreturn instead of the pre-suspend one. zsh re-asserts via `rt_sigprocmask` in its `waitjobs` loop, so this still doesn't bite in practice; full sigsuspend POSIX-correctness needs a per-process "pending sigsuspend restore" slot, which is left for follow-up.
3. **POSIX `WIFSIGNALED` encoding in `wait4`** — `src/userland/syscalls.rs::wait4_handler` only knows the cooperative-exit status encoding (`((code & 0xFF) << 8)`). For child crashes we record `exit_code = 128 + signum` (shell convention), so the parent's `wait4` writes a status whose low 7 bits are 0 — `WIFEXITED` returns true and `WEXITSTATUS` is `128+signum`. zsh therefore reports `nosuchcommand` returning `139` instead of printing "Segmentation fault". Proper fix: extend `ZombieRecord` to carry whether the child died via signal, and have `wait4_handler` emit either `(code & 0xff) << 8` (exited) or `signum & 0x7f` (signaled).

## Important Resources

- Implementation plan: `docs/IMPLEMENTATION_PLAN.md`
- Architecture documentation: `docs/ARCHITECTURE.md`
- AI-context conventions: `docs/ai-context-conventions.md`
- Tutorial reference: <https://os.phil-opp.com/>

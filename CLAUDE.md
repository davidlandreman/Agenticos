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

**Current State**: The OS has memory management, writable overlay/data filesystems, display/graphics, preemptive kernel and ring-3 scheduling, and a Linux static-musl process platform. A window system provides hierarchical window management, event routing, mouse support, copy-blit ring-3 client surfaces, and qualified VirGL client textures composited into ordinary content wells. The OS boots into a GUI desktop with a Windows 95/98-style Start menu and a recessed right-side taskbar tray showing RTC-backed UTC date/time: Programs launches Terminal, File Manager, Notepad, Calc, Painting, GL Arena, or Task Manager; Settings launches the modern ring-3 `CONTROL.ELF`; Run executes a submitted command through zsh; Documents remains a reserved placeholder. Control Center has Home/Appearance/Desktop/System/Network/About pages, live persistent Automatic/Classic/Aero switching, and live persistent BMP wallpaper selection through versioned system-control syscall 5010. `GLGAME.ELF` is a real-time colored-geometry 3D game using the bounded `userland/libs/gl` OpenGL-style frontend and syscalls 5006-5009; it requires strict VirGL for playable mode. Terminal launches ring-3 zsh with shipped `/etc/zshrc` defaults, a pruned upstream function library, and an agnoster prompt rendered with the bundled Powerline-capable JetBrains Mono subset. File Manager is the standalone ring-3 `FILEMAN.ELF` (compat command `explorer`) with Finder/Explorer-style navigation and filesystem operations; Notepad is `NOTEPAD.ELF` with real filesystem-backed Open and Save, Calc is `CALC.ELF`, and Painting is the `PAINTING.ELF` bouncing-shapes demo. Task Manager is the standalone ring-3 `TASKMGR.ELF` (also `tasks`/`taskmgr` in zsh) — a tabbed monitor (Processes / Performance / Network) with sortable process lists, CPU/memory/network history graphs, and End Task backed by real `kill(2)` SIGTERM→SIGKILL escalation. Its data plane is a minimal synthetic read-only `/proc` (`uptime`, `meminfo`, `stat`, `loadavg`, `net/dev`, `/proc/<pid>/{stat,status,cmdline,statm}`, plus AgenticOS extension tables under `/proc/agenticos/{kthreads,gui,sockets}`) generated snapshot-at-open, alongside a real `sysinfo(2)` — which also makes BusyBox `ps`, `free`, and `uptime` work. `nanosleep` really blocks (PIT-tick granularity, ISR-driven wake), unhandled fatal signals take their default action (terminate) at the dispatcher tail, and `kill(2)` can target any live ring-3 PID; kernel threads are view-only in the task manager. Ring-3 GUI apps share a userland toolkit: `userland/libs/gui` retained-mode widgets (`Button`, `TextField`, `ListView`, `MenuBar`, `TabBar`, `ColumnListView`, `TimeSeriesGraph`) plus `userland/libs/dialogs`, a common-dialogs library (`FileDialog`, `MessageBox`, `ColorPicker`) built entirely from the GUI syscalls — no kernel dialog ABI. All controls — kernel widgets and the ring-3 toolkit alike — follow the active Classic/Aero theme (`src/window/theme/controls.rs`); the kernel republishes `/etc/theme` and sends a coalesced theme-change GUI event so open apps update immediately. A static BusyBox (`BB.ELF`) provides core utilities plus `ping`, `nc`, `nslookup`, and HTTP-only `wget` through the virtual `/bin/<applet>` namespace. A static TinyCC (`TCC.ELF`, reachable as `/bin/tcc` with a `/bin/cc` alias) compiles and links C programs on-target against the musl sysroot staged at `/host/sysroot`, writing output to the boot-provisioned writable `/work` scratch directory (`cd /work && tcc -o hello /host/sysroot/examples/hello.c && ./hello`); `tcc -run` is unsupported (W^X). TinyCC is the deliberate stepping stone toward GCC — see `docs/plans/2026-07-18-003-feat-tinycc-port-plan.md` and `userland/apps/tcc/README.md`. A single-interface, polling-driven IPv4 stack uses modern VirtIO-net + smoltcp for DHCPv4, ICMP, UDP, TCP, and DHCP-backed musl name resolution through a kernel-managed `/etc`. A fail-closed cryptographic random broker selects host-backed modern VirtIO RNG in QEMU or x86-64 RDRAND on physical hardware and feeds `AT_RANDOM`, `getrandom(2)`, `/dev/urandom`, and network seeds. IPv6, TLS/HTTPS, interrupt-driven NIC I/O, and the "Agentic" runtime remain deferred.

Kernel-requested programs use one persistent `process-service`: Start, Run, and Terminal enqueue requests and return immediately, and the service later reaps detached exits from its own stack.

The legacy kernel-side command interpreter (the `shell/` process that hand-parsed commands) and its hardcoded utilities (`cat`, `ls`, `grep`, `pwd`, `wc`, `hexdump`, `echo`, `dir`, `head`, `tail`, `time`, `touch`, `wc`, `run`) were removed when zsh became the default — see `docs/plans/2026-05-16-004-feat-zsh-default-terminal-and-gui-launchers-plan.md`. Type those names in zsh and BusyBox handles them.

The native common `FileDialog` is a modern Finder/Explorer-style chooser with
Places, navigation history, breadcrumbs, file/type filtering, details and grid
views, real double-click, and validated Open/Save behavior. Shared browser
presentation primitives live in `userland/libs/gui::file_ui`; the obsolete,
unused kernel-side file open/save dialogs have been removed.

## Common Commands

### Build and Run
- `./build.sh` — Build kernel in release mode, create disk images, and run in QEMU (recommended)
- `./build.sh -c` — Clean build (removes all artifacts first)
- `./build.sh -d` — Build in debug mode (larger kernel, slower boot, more symbols)
- `./build.sh -n` — Build only, don't run QEMU
- `./build.sh --rebuild-userland` — Recompile prebuilt-managed userland apps (zsh, BusyBox, TinyCC, future Linux ports) from source instead of copying the committed `userland/prebuilt/<NAME>.ELF`. Same flag works on `test.sh`. Env equivalents: `REBUILD_USERLAND=1`, or per-app `REBUILD_ZSH=1` / `REBUILD_TCC=1`. See `userland/prebuilt/README.md`.
- `./build.sh -h` — Show help and usage
- `cargo build` — Build the kernel only (won't create disk images)
- `cargo build --release` — Build optimized release version

**Prebuilt userland ELFs**: `ZSH.ELF` (and any future Linux ports that fetch upstream tarballs) ship as committed binaries under `userland/prebuilt/`. Fresh clones boot a working zsh without the `x86_64-linux-musl-cross` toolchain installed. `HELLO.ELF` (Rust) and `HELLOCPP.ELF` (small C++ wrapper) are NOT prebuilt — they build from source on every run. After changing the upstream source / Makefile / patches of a prebuilt-managed app, run `./userland/refresh-prebuilt.sh` and commit the updated binary alongside the source change.

**QEMU Configuration**: 2 GiB RAM by default (override with `AGENTICOS_QEMU_MEMORY`), serial output, a UTC CMOS RTC, host-`/dev/urandom`-backed modern VirtIO RNG, VirtIO tablet, explicit modern VirtIO-net with QEMU user-mode NAT, and `isa-debug-exit` for test integration. Set `AGENTICOS_NETWORK=off` for a no-NIC interactive boot; tests use restricted networking plus repository-owned guest-forwarded services. QEMU builds without the slirp `user` backend (the pinned macOS VirGL bottle) automatically get networking through a stock-QEMU slirp bridge (`scripts/qemu-slirp-bridge.sh`), so VirGL GPU launches are networked too.

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

Set `AGENTICOS_RENDER_STATS=1` with a retained compositor launch to emit
per-frame raster/upload/composition/blur/fence/presentation counters. The
optional pinned macOS VirGL host verifier and its side-by-side QEMU rules are
documented in `docs/macos-virgl-qualification.md`.

### Parallel development with Conductor
This repo is configured for [conductor.build](https://www.conductor.build) — see `docs/conductor-workflow.md` for the full reference. Lifecycle is declared in `conductor.json`; `.conductor/setup.sh` bootstraps a workspace, `.conductor/run.sh` invokes `./build.sh`, `.conductor/archive.sh` cleans up QEMU on teardown. Each Conductor workspace is a git worktree with its own `target/` and QEMU process; the compound-engineering plugin is enabled via the committed `.claude/settings.json`. When proposing or evaluating cross-cutting changes, point the user at `docs/conductor-workflow.md` rather than re-deriving the workflow.

## Project Structure

The project follows a modular monolithic kernel design with clear separation of concerns. All code runs in kernel space (ring 0) with no user/kernel boundary yet.

### Top-level core files
- `src/main.rs` — Minimal kernel entry point (< 25 lines)
- `src/kernel.rs` — Kernel initialization and boot sequence
- `src/time.rs` — PIT monotonic clock plus the boot RTC-anchored UTC wall clock
- `src/panic.rs` — Custom panic handler
- `src/bootloader_config.rs` — Bootloader configuration

### Subsystem index
Each entry below points to the folder's own `CLAUDE.md`, which carries the detailed context for that subsystem. Folder files load on demand when Claude reads files in that directory.

- `src/arch/` — Architecture-specific code (x86_64 IDT, interrupts). No folder file yet — currently thin.
- `src/commands/` — `guishell` plus the (empty today) GUI launch table. File Manager, Calc, Notepad, Painting, GL Arena, and the Task Manager are ring-3 ELFs under `userland/apps/`. See [`src/commands/CLAUDE.md`](src/commands/CLAUDE.md).
- `src/drivers/` — Hardware drivers (PCI, PS/2, VirtIO including block storage, framebuffer display). See [`src/drivers/CLAUDE.md`](src/drivers/CLAUDE.md).
- `src/fs/` — VFS with ext2, FAT12/16/32, tmpfs, overlay, and `Arc`-based handles. See [`src/fs/CLAUDE.md`](src/fs/CLAUDE.md).
- `src/graphics/` — Drawing primitives, text rendering, image loading, compositor. See [`src/graphics/CLAUDE.md`](src/graphics/CLAUDE.md).
- `src/input/` — Lock-free input pipeline (SPSC queue, scancode state machines). See [`src/input/CLAUDE.md`](src/input/CLAUDE.md).
- `src/lib/` — Custom `Arc`, debug logging, `Testable` trait. See [`src/lib/CLAUDE.md`](src/lib/CLAUDE.md).
- `src/mm/` — Frame allocator, heap allocator, paging, page-fault demand mapping. See [`src/mm/CLAUDE.md`](src/mm/CLAUDE.md).
- `src/net/` — Single-interface IPv4/DHCP/ICMP/UDP/TCP stack and bounded socket registry. See [`src/net/CLAUDE.md`](src/net/CLAUDE.md).
- `src/process/` — Process traits and the live preemptive scheduler. (The shell-command registry that used to live here was removed when zsh became the default terminal.) See [`src/process/CLAUDE.md`](src/process/CLAUDE.md).
- `src/system_control.rs` — Persistent system preference owner and private syscall 5010 (theme/wallpaper query and mutation).
- `src/stdlib/` — `Read`/`Write` traits, async waker. No folder file yet — currently thin.
- `src/terminal/` — VT100/xterm terminal emulation: PTY pair, ANSI/VT parser, character grid + scrollback + alt-screen, caret, per-pty termios/winsize, key encoding. See [`src/terminal/CLAUDE.md`](src/terminal/CLAUDE.md).
- `src/tests/` — In-kernel test modules. See [`src/tests/CLAUDE.md`](src/tests/CLAUDE.md).
- `src/userland/` — Ring-3 ELF loader, Linux x86-64 ABI, lifecycle, and GUI syscalls/event ownership. See [`src/userland/CLAUDE.md`](src/userland/CLAUDE.md).
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
- `docs/solutions/learnings/` — Post-mortems and patterns from prior debugging journeys. Read the relevant one before touching adjacent code. The `2026-05-09-multi-mib-user-binary-load.md` learning covers the seven-issue chain that made multi-MiB user binaries appear to hang under interactive boot. The `2026-05-24-syscall-stub-callee-saved-leak.md` learning covers the SYSCALL stub bug that segfaulted zsh on the first interactive `ls` — kernel scratch leaked into user `rbx` across blocking syscalls because the stub didn't push user callee-saved registers and the Rust-side capture helper read clobbered live registers.
- `README.md` — Project README

## Known Issues and Technical Debt

These are cross-cutting (not subsystem-local). Subsystem-specific known issues live in the relevant folder file (e.g., the graphics refactor list lives in `src/graphics/CLAUDE.md`).

### Current Limitations
1. **No SMP** — Single CPU. The scheduler is preemptive (PIT @ 100 Hz) and multitasks kernel threads + ring-3 processes (U5-U8 in `docs/plans/2026-05-16-005-feat-multi-ring3-process-scheduling-plan.md`), but doesn't exploit multiple cores.
2. **Three namespaces with different persistence semantics.** `/` is `overlay(tmpfs, boot-FAT)` — RAM upper, FAT lower. `/data` is a persistent ext2 VirtIO block disk supporting normal Unix directory/link metadata. `/host` is vvfat (read-only). Overlay writes to `/` survive reboot via the BusyBox `sync` applet (calls `sync(2)` → overlay-state.{0,1} on `/data`). `/work` is provisioned on the overlay at every boot as the conventional scratch/compiler-output directory (ring-3 processes start with cwd `/host`, which is read-only). An explicitly supplied old FAT image can be mounted read-only at `/legacy-data` for migration.
3. **Limited Test Coverage** — Many subsystems lack comprehensive tests.
4. **Global State** — Heavy use of `static mut` and `lazy_static`.
5. **Constant Window Repainting** — `TextWindow` repaints unnecessarily in some paths.
6. **Network scope is deliberately small** — One polling modern VirtIO NIC with IPv4 and DHCP-backed DNS; IPv6, TLS, and NIC interrupts are follow-ups.

### Areas Needing Refactoring
1. **Graphics Subsystem** — Complex relationships between display modules. (Detail in `src/graphics/CLAUDE.md`.)
2. **Error Handling** — Inconsistent use of `panic!` vs `Result`.
3. **Command System** — Could benefit from better parsing/validation.
4. **Mouse Integration** — Cursor rendering tightly coupled to display.

### Deferred from the zsh-interactive bring-up
Bundled with `nosuchcommand` / `ls`-from-zsh fixes. Each is non-blocking for basic interactive zsh, but the next workload that exercises them will hit the gap.

1. ~~**Demand-grown user stack**~~ — **resolved** by `docs/plans/2026-05-16-003-feat-userland-demand-grown-stack-plan.md`. The ring-3 page-fault handler now grows the stack on demand (`src/userland/lifecycle.rs::try_grow_user_stack`), capped per-process by `USER_STACK_MAX_GROWTH_PAGES` and per-binary by `highest_pt_load_end + USER_STACK_GUARD_PAGES * 0x1000`. Initial commit is `USER_STACK_INITIAL_PAGES = 8` pages (down from the 64-page eager mapping).
2. ~~**Signal mask not restored on `rt_sigreturn` / `rt_sigsuspend`**~~ — **resolved**. `deliver_signal` (`src/userland/syscalls.rs`) writes the mask to restore into the signal frame, and `rt_sigreturn_handler` restores it. Regular delivery saves the pre-delivery mask; `rt_sigsuspend` saves the pre-suspend mask in `SignalState::suspend_restore_mask` while its temporary mask is active. `maybe_deliver_signal` installs the POSIX handler mask (`delivery_mask | sa_mask | bit(signum)`, stripping SIGKILL/SIGSTOP) atomically with signal consumption and transfers the correct restore mask into the frame.
3. **POSIX `WIFSIGNALED` encoding in `wait4`** — `src/userland/syscalls.rs::wait4_handler` only knows the cooperative-exit status encoding (`((code & 0xFF) << 8)`). For child crashes we record `exit_code = 128 + signum` (shell convention), so the parent's `wait4` writes a status whose low 7 bits are 0 — `WIFEXITED` returns true and `WEXITSTATUS` is `128+signum`. zsh therefore reports `nosuchcommand` returning `139` instead of printing "Segmentation fault". Proper fix: extend `ZombieRecord` to carry whether the child died via signal, and have `wait4_handler` emit either `(code & 0xff) << 8` (exited) or `signum & 0x7f` (signaled).

## Important Resources

- Implementation plan: `docs/IMPLEMENTATION_PLAN.md`
- Architecture documentation: `docs/ARCHITECTURE.md`
- AI-context conventions: `docs/ai-context-conventions.md`
- Tutorial reference: <https://os.phil-opp.com/>

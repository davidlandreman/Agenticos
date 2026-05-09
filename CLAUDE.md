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

**Current State**: The OS has a solid foundation with memory management, filesystem support, display/graphics, and basic process management. A window system provides hierarchical window management, event routing, and mouse support. The OS boots into a GUI desktop with a blue background and a windowed terminal application. The "Agentic" aspects (agent runtime, advanced process management) are not yet implemented.

## Common Commands

### Build and Run
- `./build.sh` — Build kernel in release mode, create disk images, and run in QEMU (recommended)
- `./build.sh -c` — Clean build (removes all artifacts first)
- `./build.sh -d` — Build in debug mode (larger kernel, slower boot, more symbols)
- `./build.sh -n` — Build only, don't run QEMU
- `./build.sh -h` — Show help and usage
- `cargo build` — Build the kernel only (won't create disk images)
- `cargo build --release` — Build optimized release version

**QEMU Configuration**: 128 MiB RAM, serial output, VirtIO tablet for seamless mouse, `isa-debug-exit` for test integration.

### Testing
- `./test.sh` — Run all kernel tests in QEMU with automatic exit
- `./test.sh arc heap` — Run only the listed test modules
- `./test.sh 'arc::test_weak*'` — Glob within a module
- `./test.sh -l` — List available modules and exit
- `./test.sh --skip-userland` — Skip the userland prebuild (faster iteration)
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
- `src/commands/` — Shell commands (18 implemented). See [`src/commands/CLAUDE.md`](src/commands/CLAUDE.md).
- `src/drivers/` — Hardware drivers (PCI, IDE, PS/2, VirtIO, framebuffer display). See [`src/drivers/CLAUDE.md`](src/drivers/CLAUDE.md).
- `src/fs/` — Read-only FAT12/16/32 filesystem with `Arc`-based handles. See [`src/fs/CLAUDE.md`](src/fs/CLAUDE.md).
- `src/graphics/` — Drawing primitives, text rendering, image loading, compositor. See [`src/graphics/CLAUDE.md`](src/graphics/CLAUDE.md).
- `src/input/` — Lock-free input pipeline (SPSC queue, scancode state machines). See [`src/input/CLAUDE.md`](src/input/CLAUDE.md).
- `src/lib/` — Custom `Arc`, debug logging, `Testable` trait. See [`src/lib/CLAUDE.md`](src/lib/CLAUDE.md).
- `src/mm/` — Frame allocator, heap allocator, paging, page-fault demand mapping. See [`src/mm/CLAUDE.md`](src/mm/CLAUDE.md).
- `src/process/` — Process traits and command dispatcher (scheduler scaffolding present, not active). See [`src/process/CLAUDE.md`](src/process/CLAUDE.md).
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
2. **Read-Only Filesystem** — No write support implemented.
3. **8.3 Filenames Only** — No long filename support.
4. **Limited Test Coverage** — Many subsystems lack comprehensive tests.
5. **Global State** — Heavy use of `static mut` and `lazy_static`.
6. **No User Space** — Everything runs in ring 0 (kernel mode).
7. **Constant Window Repainting** — `TextWindow` repaints unnecessarily in some paths.

### Areas Needing Refactoring
1. **Graphics Subsystem** — Complex relationships between display modules. (Detail in `src/graphics/CLAUDE.md`.)
2. **Error Handling** — Inconsistent use of `panic!` vs `Result`.
3. **Command System** — Could benefit from better parsing/validation.
4. **Mouse Integration** — Cursor rendering tightly coupled to display.

## Important Resources

- Implementation plan: `docs/IMPLEMENTATION_PLAN.md`
- Architecture documentation: `docs/ARCHITECTURE.md`
- AI-context conventions: `docs/ai-context-conventions.md`
- Tutorial reference: <https://os.phil-opp.com/>

---
title: Split CLAUDE.md into folder-scoped AI context files
type: refactor
status: completed
date: 2026-05-08
---

# Split CLAUDE.md into folder-scoped AI context files

## Summary

Replace the monolithic ~709-line root `CLAUDE.md` with a thin orientation file at the root plus per-subsystem `CLAUDE.md` files in each major `src/` folder, and move project-wide rules (no_std, panic handler, testing flow) into `.claude/rules/`. Folder files load lazily when Claude reads files in that directory; the root file shrinks to build commands, project overview, and a directory index.

---

## Problem Frame

The root `CLAUDE.md` has grown to ~709 lines and ~30 KB. Every Claude Code session loads it in full at launch, which inflates the context window and (per Anthropic's published guidance) reduces adherence beyond ~200 lines. The file mixes orthogonal concerns — build commands, no_std restrictions, memory management internals, FAT filesystem quirks, window-system architecture, mouse driver details, and shell command implementation — so an agent working on `src/fs/` carries window-system trivia in context, and vice versa. Sections also drift out of sync with code because there is no locality between the documentation and the files it describes.

Anthropic and the broader agent-tool ecosystem now treat nested `CLAUDE.md` (and the cross-vendor `AGENTS.md`) as a first-class pattern: subdirectory files load on demand only when the agent reads files in that directory. This refactor adopts that pattern for AgenticOS.

---

## Requirements

- R1. Root `CLAUDE.md` shrinks to ≤ ~200 lines and contains: project overview, common commands (build / test / code quality / conductor), top-level core files (`main.rs`, `kernel.rs`, `panic.rs`), configuration files list, top-level documentation list, known issues / technical debt (cross-cutting only), important resources, and a directory index pointing to per-folder files.
- R2. Each major `src/` subsystem with non-trivial domain context has its own `CLAUDE.md` describing that subsystem's purpose, key files, rules, and gotchas.
- R3. Project-wide rules that apply regardless of which folder is being touched (no_std discipline, panic handler rules, testing flow) live in `.claude/rules/` and load eagerly at session start.
- R4. No piece of guidance is duplicated between root, folder files, and `.claude/rules/` — each topic has exactly one authoritative home. (One-line invariant reminders that prevent likely mistakes — e.g., "Arc here is the kernel's custom impl, not `alloc::sync::Arc`" — may appear in more than one place; the rule applies to prose explanations, not safety reminders.)
- R5. Existing top-level `docs/` files (`ARCHITECTURE.md`, `IMPLEMENTATION_PLAN.md`, `docs/window_system_design.md`, `docs/shell_window_integration.md`, `docs/conductor-workflow.md`) remain in place and are referenced from the most relevant folder file rather than being moved or duplicated.
- R6. The split follows the Claude Code documented convention as of 2026-05-08 (filename `CLAUDE.md`, no `paths:`/`globs:` frontmatter due to known loading bugs, lazy-load on subdirectory access).

---

## Scope Boundaries

- Not adopting `AGENTS.md` — AgenticOS uses Claude Code exclusively. If multi-vendor support becomes a need later, root can become a thin wrapper that does `@AGENTS.md`.
- Not rewriting `ARCHITECTURE.md`, `IMPLEMENTATION_PLAN.md`, or any `docs/*.md` design documents.
- Not adding new product or feature documentation — only redistributing what already exists in the root file (with light reorganization).
- Not using `paths:` / `globs:` frontmatter in `.claude/rules/` — current Claude Code releases have open bugs (issues #16299, #16853, #21858, #23478) that make path-scoped lazy loading unreliable. Rules in this plan load unconditionally.
- Not introducing `@path` imports in the root file as a splitting mechanism — `@` imports load eagerly, so they organize source text without reducing tokens. Folder files are the actual splitting tool.
- Not creating folder files for thin subdirectories where one or two files would carry no useful conventions (`src/stdlib/`, `src/arch/x86_64/`'s leaf files); root index covers them with a one-liner.

### Deferred to Follow-Up Work

- Re-introducing path-scoped rule frontmatter once the open Claude Code bugs are resolved — track upstream issues and migrate eagerly-loaded rules to `paths:`/`globs:` then.
- Adding folder files to subsystems that are currently thin but may grow (`src/arch/`, `src/stdlib/`) — leave a stub or skip until the subsystem accumulates enough conventions to justify a file.

---

## Context & Research

### Relevant Code and Patterns

- Current monolithic file: `CLAUDE.md` (709 lines)
- Existing top-level docs that folder files should reference, not duplicate:
  - `docs/ARCHITECTURE.md`
  - `docs/IMPLEMENTATION_PLAN.md`
  - `docs/window_system_design.md`
  - `docs/shell_window_integration.md`
  - `docs/conductor-workflow.md`
- Existing `.claude/settings.json` ships with the repo (compound-engineering plugin enabled per workspace) — `.claude/rules/` will sit alongside it.
- Subsystem layout in `src/`: `arch`, `commands`, `drivers`, `fs`, `graphics`, `input`, `lib`, `mm`, `process`, `stdlib`, `tests`, `window`.

### External References

- [Claude Code memory documentation](https://code.claude.com/docs/en/memory) — official spec for `CLAUDE.md` loading semantics (root + ancestors load eagerly; subdirectory files load on file access; not re-injected after `/compact`).
- [AGENTS.md specification](https://agents.md/) — cross-vendor convention; not adopted here, see Scope Boundaries.
- [Monorepo CLAUDE.md split case study](https://dev.to/anvodev/how-i-organized-my-claudemd-in-a-monorepo-with-too-many-contexts-37k7) — practitioner reduced root from 47k → 9k words by distributing context into folder files.
- Open Claude Code bugs informing the no-frontmatter decision: [#16299](https://github.com/anthropics/claude-code/issues/16299), [#16853](https://github.com/anthropics/claude-code/issues/16853), [#21858](https://github.com/anthropics/claude-code/issues/21858), [#23478](https://github.com/anthropics/claude-code/issues/23478).

---

## Key Technical Decisions

- **Filename: `CLAUDE.md`, not `AGENTS.md`.** Single-vendor project; `CLAUDE.md` is what Claude Code reads natively. Switching is a one-file change later if needed.
- **No frontmatter on `.claude/rules/*.md` files.** Path-scoped frontmatter has open loading bugs; rules listed here are short enough that eager loading every session is acceptable.
- **Folder files use a consistent shape, not a strict template.** Each carries: a one-line purpose, a key-files list with one-line descriptions, conventions / gotchas, and references to deeper docs in `docs/`. No frontmatter, no required sections.
- **Root `CLAUDE.md` keeps the directory index.** Each entry points to the folder file (e.g., `src/fs/CLAUDE.md`) rather than describing the subsystem inline. This is what makes the split discoverable.
- **No `@path` imports.** They load eagerly (research finding) and would defeat the purpose. Folder files are the splitting mechanism; the root file is plain Markdown with relative-path references.
- **Top-level `docs/*.md` files are referenced, not moved.** They are stable design docs, not subsystem-local context. Folder files link to them where relevant (e.g., `src/window/CLAUDE.md` references `docs/window_system_design.md`).

---

## Open Questions

### Resolved During Planning

- Should we introduce `AGENTS.md`? — No. Single-vendor project; revisit if multi-vendor tooling is adopted.
- Should `.claude/rules/` use path-scoped frontmatter? — No. Open bugs make it unreliable; eager-load short rule files instead.
- Should we split `src/arch/` and `src/stdlib/`? — No folder file for now; both are thin enough that the root index entry suffices. Add later when content justifies it.
- Where do mouse-related notes live (driver-level vs. window cursor rendering)? — PS/2 and VirtIO mouse hardware notes go in `src/drivers/CLAUDE.md`; cursor rendering goes in `src/window/CLAUDE.md`. Each side links to the other in one line.

### Deferred to Implementation

- Exact wording / structure of each folder file's "gotchas" section — left to the implementer's judgment as content is moved out of root.
- Whether `src/graphics/fonts/` and `src/graphics/images/` warrant nested folder files — defer until `src/graphics/CLAUDE.md` is written; if its file index reads cleanly, no nested files are needed.

---

## Output Structure

```
CLAUDE.md                          # slimmed root (~150-200 lines)
.claude/
  rules/
    no-std.md                      # no_std discipline; alloc allowed post-init
    panic-and-attributes.md        # #![no_std] / #![no_main] / #[panic_handler] rules
    testing-flow.md                # ./test.sh, QEMU exit codes, no_std test framework
src/
  arch/                            # (no folder file -- thin; root index covers it)
  commands/CLAUDE.md               # shell commands list + "adding a new command" recipe
  drivers/CLAUDE.md                # PCI, IDE, PS/2, VirtIO; mouse hardware paths
  fs/CLAUDE.md                     # block / VFS / FAT layering; Arc-based file API; limitations
  graphics/CLAUDE.md               # framebuffer architecture, double buffering, refactor notes
  input/CLAUDE.md                  # lock-free SPSC queue, scancode state machines, integration
  lib/CLAUDE.md                    # custom Arc/Weak, debug logging, test_utils
  mm/CLAUDE.md                     # frame allocator, heap, paging, page-fault flow
  process/CLAUDE.md                # process traits, command registry, what's NOT implemented
  stdlib/                          # (no folder file -- thin)
  tests/CLAUDE.md                  # writing tests, test runner, exit codes
  window/CLAUDE.md                 # window hierarchy, types, default desktop, cursor rendering
```

---

## Implementation Units

### U1. Define the convention and write the meta-doc

**Goal:** Capture the split convention so future agents and contributors know what goes where, and so the convention survives turnover.

**Requirements:** R1, R3, R6

**Dependencies:** None

**Files:**
- Create: `docs/ai-context-conventions.md` (short doc — what `CLAUDE.md` files contain at root vs. folder level, what `.claude/rules/` is for, why no frontmatter, how to add a new folder file)

**Approach:**
- Document the layered model: `.claude/rules/` (always loaded, project-wide rules) → root `CLAUDE.md` (always loaded, orientation + index) → `src/<subsystem>/CLAUDE.md` (lazy, loads when agent reads files in that folder).
- Document the standard shape of a folder file: purpose line, key files, conventions / gotchas, references.
- Note the no-frontmatter decision and link to the open Claude Code bugs so a future reader knows when to revisit.
- Cross-link from root `CLAUDE.md` (added in U13).

**Test scenarios:**
- Happy path: a contributor opening `docs/ai-context-conventions.md` learns what file to create when adding a new subsystem, and where to put a project-wide rule, without reading any other doc.
- Edge case: doc explicitly addresses the "should I use `AGENTS.md`?" question and the "should I use `paths:` frontmatter?" question, since both are likely to come up.

**Verification:**
- A reviewer who has never seen this plan can read `docs/ai-context-conventions.md` and correctly predict where to place new content.

---

### U2. Establish `.claude/rules/` directory with project-wide rules

**Goal:** Move always-applicable, cross-cutting rules out of the root `CLAUDE.md` and into eagerly-loaded rule files, so they cannot be missed during sessions that don't touch the root.

**Requirements:** R3, R4, R6

**Dependencies:** U1 (convention defined)

**Files:**
- Create: `.claude/rules/no-std.md` — covers: no `std::*` imports; `core::*` and `alloc::*` only; `Vec<T>` / `String` available *after* heap initialization; no `HashMap` from std (use `BTreeMap`); no file I/O / threads / network.
- Create: `.claude/rules/panic-and-attributes.md` — covers: `#![no_std]`, `#![no_main]`, `#[no_mangle]`, `#[panic_handler]` requirements; what custom panic handler does in normal vs. test mode.
- Create: `.claude/rules/testing-flow.md` — covers: `./test.sh` invocation, custom test framework (no_std environment, QEMU integration, serial-port debug output — folds in the "Testing Approach" bullet at root lines ~273-276), QEMU exit codes (33 = success, 35 = failure), how the panic handler differs in test mode.

**Approach:**
- Each rule file is short (under ~50 lines). No frontmatter. Plain Markdown.
- Rule files focus on *what to do / not do*, not architecture. Architecture for memory and tests still lives in `src/mm/CLAUDE.md` and `src/tests/CLAUDE.md` respectively.
- Mention briefly in each file where the deeper context lives ("see `src/mm/CLAUDE.md` for heap internals").

**Test scenarios:**
- Happy path: opening any rule file in isolation gives a complete picture of the constraint without requiring the reader to chase references.
- Edge case: the no_std rule file makes clear that `alloc` types are allowed (post-init) — historically a confusing point given the file's emphasis on "no std".
- Edge case: when a contributor adds a new file under `src/`, the no_std rule is in context regardless of which folder.

**Verification:**
- A new Claude Code session that does not read the root file still has no_std discipline, panic-handler rules, and the testing flow available because `.claude/rules/` is eagerly loaded.

---

### U3. Write `src/mm/CLAUDE.md`

**Goal:** Move the Memory Management section out of root and co-locate it with `src/mm/` files.

**Requirements:** R2, R4

**Dependencies:** U1

**Files:**
- Create: `src/mm/CLAUDE.md`
- Source content: lines ~283-340 of root `CLAUDE.md` ("Memory Management" through "Debugging" subsection)

**Approach:**
- Carry forward: heap location (`0x_4444_4444_0000`), 100 MiB size, `linked_list_allocator` v0.10 backend, frame allocator behavior (skip frame 0), `OffsetPageTable` usage, page-fault demand-paging flow.
- Drop the duplicated `Vec`/`String` usage example — that's covered by `.claude/rules/no-std.md`.
- Reference `.claude/rules/no-std.md` in one line for the alloc rules rather than repeating them.
- Key files index: `frame_allocator.rs`, `heap.rs`, `paging.rs`, `memory.rs`.

**Test scenarios:**
- Happy path: an agent editing `src/mm/heap.rs` has the heap base address, allocator backend, and OOM behavior in context without needing to read the root file.
- Edge case: the page-fault flow (handler allocates frame, maps page, returns) is documented because that behavior is non-obvious from the code alone.

**Verification:**
- The Memory Management section in root is removed in U13. After that removal, all heap / paging context still discoverable from `src/mm/CLAUDE.md`.

---

### U4. Write `src/fs/CLAUDE.md`

**Goal:** Co-locate filesystem subsystem context with `src/fs/` files.

**Requirements:** R2, R4

**Dependencies:** U1

**Files:**
- Create: `src/fs/CLAUDE.md`
- Source content: lines ~663-708 of root ("Filesystem Support" section)

**Approach:**
- Carry forward: layered architecture (BlockDevice → MBR partition → VFS → FAT), Arc-based file handle pattern (`File::open_read`, `read_to_string`, clone semantics), the four current limitations (read-only, 8.3 only, FAT only, no subdirs).
- Key files index: `filesystem.rs`, `partition.rs`, `vfs.rs`, `file_handle.rs`, `fs_manager.rs`, `fat/` subtree.
- Reference `src/lib/CLAUDE.md` for the underlying Arc implementation rather than re-explaining it.
- Reference `src/drivers/CLAUDE.md` for `block.rs` / `ide.rs` (the storage substrate sits in drivers, not fs).

**Test scenarios:**
- Happy path: an agent editing `src/fs/fat/filesystem.rs` knows about the 8.3-filename and read-only constraints in context.
- Edge case: the cross-reference to `src/drivers/` for block storage is explicit, so an agent making a write-support change does not miss the IDE driver layer.

**Verification:**
- Filesystem section removed from root in U13; all FAT / VFS / handle context discoverable from `src/fs/CLAUDE.md`.

---

### U5. Write `src/drivers/CLAUDE.md`

**Goal:** Co-locate hardware-driver context (PCI, IDE, PS/2, VirtIO, mouse hardware) with `src/drivers/`.

**Requirements:** R2, R4

**Dependencies:** U1

**Files:**
- Create: `src/drivers/CLAUDE.md`
- Source content: lines ~572-607 of "Mouse Support" (hardware drivers only) — VirtIO tablet detection via PCI, PS/2 controller setup, PS/2 mouse packet validation, IRQ12; plus block device / IDE notes from the existing module organization list. The cursor-rendering subsection at lines ~608-612 is owned by U7 — do not extract it here.

**Approach:**
- Document the input-method selection sequence (PCI scan → VirtIO tablet if present → PS/2 fallback).
- Document `-device virtio-tablet-pci` QEMU flag dependency.
- Document the 3-byte PS/2 packet format and the bit-3 validation requirement.
- Cross-reference: `src/input/CLAUDE.md` for the lock-free queue that consumes raw events; `src/window/CLAUDE.md` for cursor rendering (NOT here).
- Key files index: `pci.rs`, `keyboard.rs`, `mouse.rs`, `ps2_controller.rs`, `block.rs`, `ide.rs`, `virtio/`, `display/`. Triage `mouse_old.rs` (currently sits alongside `mouse.rs`) — if dead, note for removal in a separate PR; if legacy-but-kept, document why.

**Test scenarios:**
- Happy path: an agent editing `src/drivers/mouse.rs` understands the PS/2 packet format and screen-clamping behavior in context.
- Edge case: the document makes clear that `mouse_old.rs` exists alongside `mouse.rs` (the dir listing showed both) — flag whether this is dead code or intentional.

**Verification:**
- Mouse / PCI / IDE driver context discoverable from `src/drivers/CLAUDE.md` after root section is removed in U13.

---

### U6. Write `src/input/CLAUDE.md`

**Goal:** Co-locate the input processing pipeline (lock-free queue, scancode state machines) with `src/input/`.

**Requirements:** R2, R4

**Dependencies:** U1

**Files:**
- Create: `src/input/CLAUDE.md`
- Source content: lines ~617-661 of root ("Input Processing Pipeline" section)

**Approach:**
- Carry forward: three-layer architecture (hardware → processing → event), the SPSC ring buffer (256 entries, power of 2 for modulo, atomic Release/Acquire), the rationale for lock-free (the historical try_lock issue in interrupt context), the scancode-set-2 state machine, the integration call from kernel idle loop.
- Drop the embedded code snippets (`push` / `pop` signatures) — those are visible in `src/input/queue.rs` itself; the doc just needs to communicate the design intent.
- Key files index: `queue.rs`, `keyboard_driver.rs`, `mouse_driver.rs`, `mod.rs`.
- Cross-reference: `src/drivers/CLAUDE.md` for raw hardware events; `src/window/CLAUDE.md` for typed event delivery.

**Test scenarios:**
- Happy path: an agent editing `src/input/queue.rs` understands the SPSC invariant and the why-lock-free rationale in context.
- Edge case: an agent considering "let me just add a Mutex here" sees the historical context that explains why that's wrong.

**Verification:**
- Input pipeline context discoverable from `src/input/CLAUDE.md` after root section is removed in U13.

---

### U7. Write `src/window/CLAUDE.md`

**Goal:** Co-locate window system context (hierarchy, window types, cursor rendering, default desktop layout) with `src/window/`.

**Requirements:** R2, R4, R5

**Dependencies:** U1

**Files:**
- Create: `src/window/CLAUDE.md`
- Source content: lines ~522-570 ("Window System") plus the cursor rendering subsection from "Mouse Support" (lines ~608-612)

**Approach:**
- Carry forward: window hierarchy and parent-child coordinate transformation, the five window types (Desktop, Frame, Text, Terminal, Container) with their distinguishing details, default desktop layout (blue background, terminal at 100,50 sized 800x600), implementation status by phase.
- Reference `docs/window_system_design.md` for the deeper architecture doc rather than restating.
- Reference `docs/shell_window_integration.md` for terminal-window / shell integration.
- Cross-reference: `src/graphics/CLAUDE.md` for the underlying drawing primitives; `src/input/CLAUDE.md` for the event source.
- Key files index: `mod.rs`, `types.rs`, `event.rs`, `graphics.rs`, `manager.rs`, `screen.rs`, `console.rs`, `cursor.rs`, `keyboard.rs`, `terminal.rs`, `terminal_factory.rs`, `windows/` subtree, `adapters/` subtree, `dialogs/` subtree.

**Test scenarios:**
- Happy path: an agent editing `src/window/manager.rs` has the parent-child coordinate transform invariant in context.
- Edge case: cursor rendering location (here, not in `src/drivers/`) is explicit so an agent doesn't accidentally split rendering across both folders.

**Verification:**
- Window system context discoverable from `src/window/CLAUDE.md` after root section is removed in U13.

---

### U8. Write `src/graphics/CLAUDE.md`

**Goal:** Co-locate graphics / display subsystem context (framebuffer, double buffering, primitives) with `src/graphics/`.

**Requirements:** R2, R4

**Dependencies:** U1

**Files:**
- Create: `src/graphics/CLAUDE.md`
- Source content: lines ~395-431 ("Graphics and Display Subsystem") plus the performance-considerations bullets relevant to graphics from "Performance Considerations" (lines ~193-202)

**Approach:**
- Carry forward: framebuffer-not-VGA fact, `USE_DOUBLE_BUFFER` flag in `src/drivers/display/display.rs`, performance insights (framebuffer slow, bulk copy fast, scrolling via memmove, static allocation), graphics capabilities summary, current architecture issues, recommended layered refactor.
- Note the cross-folder coupling: the double-buffer flag lives in `src/drivers/display/display.rs` even though graphics primitives live in `src/graphics/`. Cross-reference `src/drivers/CLAUDE.md`.
- Key files index: `core_gfx.rs`, `core_text.rs`, `compositor.rs`, `framebuffer.rs`, `render.rs`, `mouse_cursor.rs`, `fonts/`, `images/`.
- Skip nested folder files for `fonts/` and `images/` initially — the file index covers them.

**Test scenarios:**
- Happy path: an agent editing `src/graphics/compositor.rs` understands dirty-rect tracking and the cursor-overlay model.
- Edge case: the "current architecture issues" subsection is preserved verbatim — it's a known-pain marker that future refactor work needs.
- Edge case: the cross-folder coupling note for `USE_DOUBLE_BUFFER` is clear so an agent flipping the flag knows to look in `src/drivers/display/`.

**Verification:**
- Graphics context discoverable from `src/graphics/CLAUDE.md` after root section is removed in U13.

---

### U9. Write `src/process/CLAUDE.md`

**Goal:** Co-locate the process / command-dispatcher subsystem context with `src/process/`.

**Requirements:** R2, R4

**Dependencies:** U1

**Files:**
- Create: `src/process/CLAUDE.md`
- Source content: lines ~432-461 ("Process Management" overview)

**Approach:**
- Carry forward: the "process management is really a command dispatcher" framing, what's implemented (Process / BaseProcess traits, sequential PIDs, command registry, shell integration), what's NOT implemented (no scheduling / context switch / isolation / concurrency / IPC).
- Move the "Adding New Commands" recipe to `src/commands/CLAUDE.md` (U10) where it semantically belongs.
- Note the presence of `context.rs`, `pcb.rs`, `scheduler.rs`, `stack.rs` in the directory listing — these exist but are not yet wired up. Document this so a contributor knows there is partial scheduler scaffolding present.
- Key files index: `process.rs`, `manager.rs`, `pcb.rs`, `context.rs`, `scheduler.rs`, `stack.rs`.

**Test scenarios:**
- Happy path: an agent reading `src/process/manager.rs` understands that this is a command dispatcher today, with scheduling scaffolding present but not active.
- Edge case: the disconnect between root CLAUDE.md's "no scheduling" claim and the existence of `scheduler.rs` / `context.rs` / `pcb.rs` / `stack.rs` in the codebase is explicit, so future work knows whether to extend the scaffolding or replace it.

**Verification:**
- Process context discoverable from `src/process/CLAUDE.md` after root section is removed in U13.

---

### U10. Write `src/commands/CLAUDE.md`

**Goal:** Co-locate shell-command implementation guidance and the "how to add a new command" recipe with `src/commands/`.

**Requirements:** R2, R4

**Dependencies:** U1, U9

**Files:**
- Create: `src/commands/CLAUDE.md`
- Source content: lines ~462-520 ("How Commands Work" + "Adding New Commands"), plus updated current-command list reflecting actual `src/commands/` directory contents (which now includes `calc`, `guishell`, `notepad`, `painting`, `tasks` in addition to the original 13 listed in root).

**Approach:**
- Carry forward: registration / execution / `RunnableProcess` trait pattern; the four-step "add a new command" recipe (create file, implement traits, register in `kernel.rs`, export from `mod.rs`).
- Update the command list — root CLAUDE.md says "13 implemented", but the current `src/commands/` directory contains 18 command directories (verified by listing `src/commands/*/`): `calc`, `cat`, `dir`, `echo`, `grep`, `guishell`, `head`, `hexdump`, `ls`, `notepad`, `painting`, `pwd`, `shell`, `tail`, `tasks`, `time`, `touch`, `wc`. Reflect this exact list in the folder file.
- Cross-reference `src/process/CLAUDE.md` for the underlying process abstraction.

**Test scenarios:**
- Happy path: an agent asked to add a new shell command can follow the four-step recipe end-to-end without reading any other file.
- Edge case: the recipe's "register in kernel.rs" step is preserved with the actual function name (`register_command`), so the agent doesn't have to grep for it.
- Edge case: the command list is current — drifted entries from the original root file are corrected.

**Verification:**
- Adding-a-command recipe is in `src/commands/CLAUDE.md` after root section is removed in U13. Following the recipe still produces a working command (no recipe steps were lost in the move).

---

### U11. Write `src/lib/CLAUDE.md`

**Goal:** Co-locate the custom Arc / Weak / debug / test-utils library context with `src/lib/`.

**Requirements:** R2, R4

**Dependencies:** U1

**Files:**
- Create: `src/lib/CLAUDE.md`
- Source content: lines ~245-271 ("Arc (Atomic Reference Counting)") plus the brief notes about `debug.rs` (5-level logging) and `test_utils.rs` (Testable trait, test runner) from the module organization list.

**Approach:**
- Carry forward: the Arc usage example, the four key features (atomic refcounting, weak refs, `!Sized` support, heap-integrated), notes on `debug.rs` and `test_utils.rs`.
- Reference `src/tests/CLAUDE.md` (U12) for how `test_utils` is consumed.
- Note `debug_breakpoint.rs` in the dir listing.
- Key files index: `arc.rs`, `debug.rs`, `debug_breakpoint.rs`, `test_utils.rs`.

**Test scenarios:**
- Happy path: an agent editing `src/lib/arc.rs` has the Arc/Weak design intent in context.
- Edge case: the doc makes clear that this is a *custom* Arc, not `alloc::sync::Arc` — important because the alloc version exists but the project chose not to use it.

**Verification:**
- Arc context discoverable from `src/lib/CLAUDE.md` after root section is removed in U13.

---

### U12. Write `src/tests/CLAUDE.md`

**Goal:** Co-locate the kernel-level testing-framework details (writing / running tests, test runner, exit codes) with `src/tests/`.

**Requirements:** R2, R4

**Dependencies:** U1, U2 (the `.claude/rules/testing-flow.md` rule must already exist so this file can reference rather than duplicate)

**Files:**
- Create: `src/tests/CLAUDE.md`
- Source content: lines ~342-393 ("Testing Framework" — architecture, writing tests, running tests, output format)

**Approach:**
- Carry forward: how to add a test (function + register in `get_tests()` returning `&'static [&'static dyn Testable]`), the example test pattern, output format ("name and [ok]" pattern, serial output).
- Reference `.claude/rules/testing-flow.md` for the high-level invocation (`./test.sh`) and exit codes — do not duplicate.
- Reference `src/lib/CLAUDE.md` for the `Testable` trait and `test_runner()` definition.
- Key files index: `basic.rs`, `memory.rs`, `heap.rs`, `arc.rs`, `display.rs`, `interrupts.rs`, `filesystem.rs`.

**Test scenarios:**
- Happy path: an agent adding a new test follows the example pattern and registers it correctly without reading the root file.
- Edge case: the contract that `get_tests()` returns a `&'static` slice (not `Vec` — historical no_std workaround) is preserved.
- Edge case: no duplication of `./test.sh` invocation or QEMU exit codes between this file and `.claude/rules/testing-flow.md` — the rule file owns invocation, this file owns authoring.

**Verification:**
- Testing-framework context discoverable from `src/tests/CLAUDE.md` after root section is removed in U13.

---

### U13. Slim the root `CLAUDE.md` and add the directory index

**Goal:** Replace the existing 709-line root file with a focused orientation file (≤ ~200 lines) that covers project overview, build / test commands, conductor integration, cross-cutting issues, and a directory index pointing to every folder file.

**Requirements:** R1, R4, R5

**Dependencies:** U1, U2, U3, U4, U5, U6, U7, U8, U9, U10, U11, U12 (all destinations must exist before content is removed from root)

**Files:**
- Modify: `CLAUDE.md`

**Approach:**
- Keep at root: Project Overview (and its "Current State" paragraph), Common Commands (build / test / code quality / conductor), Project Structure → Core Files (top-level `main.rs` / `kernel.rs` / `panic.rs`), Configuration Files list, Documentation list (existing top-level docs references — `ARCHITECTURE.md`, `IMPLEMENTATION_PLAN.md`, `docs/window_system_design.md`, `docs/shell_window_integration.md`, `docs/conductor-workflow.md`), Known Issues / Technical Debt (cross-cutting, not subsystem-specific), Important Resources (tutorial reference, OS Development Specifics → "Important Resources" subsection from current root lines ~278-281).
- Replace the long Module Organization subsection with a one-line-per-folder index pointing to each `src/<subsystem>/CLAUDE.md` (e.g., `- src/fs/ — filesystem layer (FAT12/16/32, read-only). See src/fs/CLAUDE.md`).
- Remove these now-redundant sections (they live elsewhere): no_std restrictions, heap allocation example, Arc, OS Development Specifics → Key Attributes, Memory Management, Testing Framework, Graphics and Display Subsystem, Process Management, Window System, Mouse Support, Input Processing Pipeline, Filesystem Support.
- Add a brief "AI context layout" paragraph near the top: "Project-wide rules live in `.claude/rules/`. Subsystem-specific context lives in `src/<subsystem>/CLAUDE.md` and loads on demand. See `docs/ai-context-conventions.md` for the convention."
- Verify final line count is ≤ ~200.

**Execution note:** Do this unit *after* every destination file exists so no content is destroyed without a home. The atomic operation: for each section being removed, confirm the destination file contains the equivalent content, then delete from root.

**Test scenarios:**
- Happy path: a fresh Claude Code session starting from root has build commands, project overview, and the folder index in context — under ~200 lines.
- Happy path: every removed section's content can be located by following the directory index.
- Edge case: cross-cutting items (e.g., the "no_std" rule, panic handler attributes) are not removed entirely — they moved to `.claude/rules/`, which is also eagerly loaded, so the constraint is still in every session's context.
- Edge case: the directory index entry exists for every `src/` subdirectory, including thin ones (`stdlib/`, `arch/`) that don't have their own folder file — those entries say "no folder file; see directory contents" so the absence is intentional and visible.
- Error path: confirm no section was deleted whose content is not present in either a folder file or `.claude/rules/`. Diff the original 709-line root against the union of the new root + all folder files + all rule files; everything substantive should appear in the union.

**Verification:**
- `wc -l CLAUDE.md` returns ≤ ~200.
- A `grep` for any major topic (e.g., "heap", "FAT", "VirtIO", "scancode", "no_std") still finds it in the union of (root + folder files + rules).
- Section-mapping table: produce a checklist mapping each H2/H3 heading from the original 709-line root file to its destination file (root / folder / rules / dropped-as-redundant). Every original heading must appear in the table; reviewer signs off on the table before deletion lands. This is the dedup audit for R4 — review the destination cells column-by-column to confirm no heading lands in two places.
- Resolve the bare-name vs. `docs/`-prefixed reference question: root currently cites `IMPLEMENTATION_PLAN.md` and `architecture.md` (bare), while these files exist both at the repo root and under `docs/`. Pick one canonical location and update root references; if duplicates are stale, flag for removal in a follow-up PR.

---

## System-Wide Impact

- **Interaction graph:** Other docs that link into root `CLAUDE.md` continue to work — the file is not deleted, just slimmed. Any external links (Conductor docs, `docs/conductor-workflow.md`, etc.) that reference specific sections of the root file may need updating if those sections moved; spot-check during U13.
- **State lifecycle risks:** None — this is a documentation refactor; no code changes.
- **API surface parity:** None — no API changes.
- **Integration coverage:** Manual review is the only verification mode (documentation has no test suite). The cross-file diff in U13's verification step is the safety net against accidental content loss.
- **Unchanged invariants:** All existing source code, tests, build scripts, and `.claude/settings.json` are unchanged. `docs/` directory contents are unchanged. The Conductor workflow continues to work — `.conductor/setup.sh` etc. don't read `CLAUDE.md`.

---

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| Content lost during the move (some section in root has no destination) | U13's verification step diffs the union of new files against the original root; flag any orphan content before deletion |
| Folder file drift — subsystem changes but its `CLAUDE.md` is not updated | Co-location makes this less likely than a monolithic file (the doc is right next to the code), but no automated guard exists. Accept this risk |
| Path-scoped frontmatter bugs are fixed and we're carrying eager-loaded rules unnecessarily | Tracked in Deferred to Follow-Up Work; revisit when upstream issues close |
| Some folder files are too thin to justify (e.g., `src/process/`) and add maintenance burden without value | If a draft folder file ends up under ~30 lines and the content is generic, fold it back into root and skip; document the decision in `docs/ai-context-conventions.md` |
| Content drifts between `.claude/rules/` and folder files (e.g., no_std mentioned in both) | Each rule file explicitly says where the deeper context lives; folder files reference rather than restate. Reviewed in U13 |
| Default Claude Code behavior changes (e.g., subdirectory files become eagerly loaded) | Convention doc (`docs/ai-context-conventions.md`) cites the specific behavior so a future reader knows what assumption was made; revisit if Anthropic ships a behavior change |

---

## Documentation / Operational Notes

- This refactor is purely documentation; there is no rollout, monitoring, or feature flag.
- After landing, mention the new layout briefly in the next PR description so contributors know to add subsystem-specific context to the right folder file going forward.
- `docs/ai-context-conventions.md` is the durable reference; future agents that join the project will read it via the root `CLAUDE.md` index.

---

## Sources & References

- Origin: direct user request — split monolithic `CLAUDE.md` into folder-scoped files following 2026 best practice.
- Related code: `CLAUDE.md` (root, 709 lines, ~30 KB); `.claude/settings.json` (existing).
- External docs:
  - [Claude Code memory documentation](https://code.claude.com/docs/en/memory)
  - [AGENTS.md spec](https://agents.md/)
  - [Monorepo CLAUDE.md split case study](https://dev.to/anvodev/how-i-organized-my-claudemd-in-a-monorepo-with-too-many-contexts-37k7)
  - Open Claude Code path-frontmatter bugs: [#16299](https://github.com/anthropics/claude-code/issues/16299), [#16853](https://github.com/anthropics/claude-code/issues/16853), [#21858](https://github.com/anthropics/claude-code/issues/21858), [#23478](https://github.com/anthropics/claude-code/issues/23478)

---
title: "feat: Userland app platform (ring-3 ELF apps loaded from /host)"
type: feat
status: active
created: 2026-05-08
plan_id: 2026-05-08-004
depth: deep
---

# feat: Userland app platform (ring-3 ELF apps loaded from /host)

## Summary

Stand up a userland application platform: build sub-apps in a sibling cargo project, stage their ELF artifacts into `host_share/` (visible inside the OS at `/host`), and run them from the shell with a new `run` verb. Apps execute in ring 3 with paging-enforced isolation and reach kernel functionality through a name-keyed syscall ABI (kernel-exposed symbol table; loader patches GOT slots to a kernel-supplied user-trampoline page). First shipped app prints "hello" and exits; the same path supports any future app.

---

## Problem Frame

The kernel today runs everything in ring 0. The 18 shell commands are Rust modules compiled into the kernel binary; adding a new "program" requires touching kernel source and rebuilding the OS. There is no isolation between commands and no way to ship a binary independent of the kernel.

The host folder mount landed in commit 681ef89 (`/host` → `host_share/` via QEMU vvfat) and gives us a transport for getting host-built artifacts into the guest. The natural next step is to use that transport for *executable* artifacts, not just data files — turning the kernel into a real application platform.

User intent (from Phase 0 dialogue): full ring-3 isolation with paging + syscalls (the most ambitious option offered), and a name-keyed ABI so the kernel API can grow without renumbering syscall vectors. The first app is intentionally trivial — proving the pipeline end-to-end is the real deliverable.

---

## Requirements

- **R1.** Sub-apps are built from a separate cargo target in this repo (not compiled into the kernel binary).
- **R2.** The build pipeline produces an executable file (ELF) and stages it into `host_share/` under an uppercase 8.3 name visible at `/host/` inside the guest.
- **R3.** The shell launches an app via a new `run /HOST/<NAME>.ELF` verb.
- **R4.** Apps execute in ring 3 with paging-enforced isolation from the kernel (USER bit, no kernel access).
- **R5.** Apps reach kernel functionality by **name** — the kernel exposes a symbol table; the loader resolves named imports to syscall stubs at load time.
- **R6.** The first app prints text and exits cleanly; the shell prompt returns afterward.
- **R7.** A user-mode fault (page fault, GP, UD, divide-error, etc.) terminates the app cleanly without taking the kernel down.
- **R8.** Loader and runtime are robust against malformed ELF input — typed errors, no kernel panics.
- **R9.** Repeated runs do not leak frames, page mappings, PCBs, or terminal state.
- **R10.** Existing in-kernel commands and the existing preemptive scheduler continue to work unchanged.

---

## Output Structure

The plan creates a new sibling cargo project at the repo root. Tree showing the expected layout — implementer may adjust if a better arrangement emerges:

```
mogadishu/
├── userland/                    # NEW — sibling cargo project (not a workspace member)
│   ├── Cargo.toml
│   ├── .cargo/
│   │   └── config.toml          # build-std for the userland target
│   ├── x86_64-userland.json     # custom target (or reuse x86_64-unknown-none)
│   ├── linker.ld                # base address 0x40_0000, _start entry, no PT_INTERP
│   ├── runtime/                 # tiny "userlib": panic_handler, mem* shims, syscall stubs (placeholder — kernel rewrites GOT to trampoline)
│   │   ├── Cargo.toml
│   │   └── src/lib.rs
│   └── apps/
│       └── hello/
│           ├── Cargo.toml
│           └── src/main.rs      # #![no_std] #![no_main] _start: print("hello\n"); exit(0)
├── src/
│   ├── arch/x86_64/
│   │   ├── gdt.rs               # NEW
│   │   ├── syscall.rs           # NEW (int 0x80 dispatcher)
│   │   ├── interrupts.rs        # MOD (ring-3 fault routing, 0x80 vector, IST for #DF)
│   │   ├── preemption.rs        # MOD (cs/ss from PCB, ring-3 awareness)
│   │   └── context_switch.rs    # MOD (cs/ss from PCB)
│   ├── userland/                # NEW kernel-side userland subsystem
│   │   ├── mod.rs
│   │   ├── CLAUDE.md
│   │   ├── error.rs             # LoaderError
│   │   ├── loader.rs            # ELF parse + map + relocate (transactional)
│   │   ├── image.rs             # UserImage handle (Drop unmaps + frees)
│   │   ├── abi.rs               # SYSCALL_TABLE registry, register_syscall(name, handler)
│   │   ├── trampoline.rs        # builds + maps the user-trampoline page
│   │   ├── syscalls.rs          # kernel-side handlers for print, exit
│   │   └── lifecycle.rs         # cleanup_user_process(), continuation/long-jump
│   ├── commands/run/            # NEW shell verb
│   │   └── mod.rs
│   ├── mm/paging.rs             # MOD (map_user_region using map_to_with_table_flags)
│   ├── process/
│   │   ├── pcb.rs               # MOD (user-process fields)
│   │   └── context.rs           # MOD (cs/ss fields)
│   ├── tests/userland.rs        # NEW
│   └── kernel.rs                # MOD (init order, register_command("run", …))
├── host_share/
│   └── HELLO.ELF                # produced by build.sh (uppercase 8.3)
└── build.sh                     # MOD (build userland → stage → build kernel)
```

---

## Key Technical Decisions

| ID | Decision | Rationale |
|---|---|---|
| **D1** | **Syscall transport: `int 0x80` (DPL=3) for the first cut.** Defer `syscall`/`sysret` MSR setup to a later track. | Reuses the existing IDT machinery the kernel already trusts. Isolates "did I enter ring 3 correctly?" from "did I configure SCE/STAR/LSTAR/SFMASK correctly?" Eliminates a triple-fault debugging surface. The print-only first app is not call-rate-bound. |
| **D2** | **GDT layout (selector → DPL → kind):** `0x00` null, `0x08` kernel code (DPL 0), `0x10` kernel data (DPL 0), `0x18` user data (DPL 3), `0x20` user code (DPL 3), `0x28` TSS (16 bytes, system). | Preserves the literal `0x08`/`0x10` selectors hard-coded in the existing naked asm in `src/arch/x86_64/context_switch.rs` and `src/arch/x86_64/preemption.rs`. The user-data-before-user-code order keeps the door open for `syscall`/`sysret` later (which computes user CS = STAR[63:48] + 16 and user SS = +8 by formula). |
| **D3** | **ELF format: static, non-PIE, fixed link base `0x0040_0000`.** | Eliminates `R_X86_64_RELATIVE` handling. Loader still walks `R_X86_64_GLOB_DAT` / `R_X86_64_JUMP_SLOT` for kernel-imported symbols (the whole point of name-keyed ABI). One user app at a time means VA collisions are not a concern. |
| **D4** | **Symbol-keyed ABI via a kernel-supplied user-trampoline page.** Kernel maps a small USER+R+X page at a fixed user VA containing one stub per kernel-exported symbol (`mov rax, <id>; int 0x80; ret`). Kernel symbol table maps names → addresses inside that page. Loader patches each user GOT slot to point into the trampoline page. | Avoids the trap of patching GOT slots to ring-0 addresses (which would `#GP` on first call). This is the vDSO pattern, narrowed to one mechanism. The kernel API surface is just "register a (name, handler) pair" — adding new syscalls later does not require renumbering or re-linking existing apps that don't use them. |
| **D5** | **Single shared CR3, fixed user VA range, full unmap on exit. At most one user process at a time.** Assert this invariant in `run`. | Avoids per-process page-table machinery (Cr3 swap on context switch, propagating the mapper through interrupt handlers). Matches the user-confirmed "single-app, synchronous to completion" scope. Per-process address spaces are explicitly deferred. |
| **D6** | **Single static TSS. `rsp0` set once per `run`. Add IST[0] with a 4 KiB stack wired to `#DF`.** | RSP0 is the only TSS field that materially matters in long mode. IST for `#DF` is cheap insurance against silent triple-faults from kernel-stack overflow during user-mode work — Phil-Opp's canonical recommendation. |
| **D7** | **Return-from-user via a saved kernel continuation.** Before `iretq`-ing to ring 3, the kernel-side `run` saves a "return-to-shell" continuation (kernel RSP + label). Both the `exit` syscall and ring-3 fault paths long-jump to it. The timer-interrupt handler refuses to deschedule when `cs.RPL == 3` — it just `iretq`s back to user. | Simplest correct model for single-app-synchronous. The user app behaves as a giant CPL=3 "syscall" from the shell's perspective. No CR3 swap needed; no full ring-3 register save in the timer ISR. |
| **D8** | **Loader is a transaction (`Result<UserImage, LoaderError>`). `UserImage` owns the frame list + mapping range + user-stack frames; `Drop` unmaps and frees.** Failure mid-load returns `Err` and commits nothing. | Defends R8 and R9 — a partially-mapped, partially-resolved binary cannot leak into kernel state. Mirrors the static-slot/handle pattern already used in `src/process/stack.rs` and the FAT mount wrappers. |
| **D9** | **Userland is a sibling cargo project at `userland/`, not a workspace member.** Built with its own target dir before the kernel; `build.sh` copies the artifact into `host_share/HELLO.ELF` (uppercase 8.3). | Cargo has no stable per-member target override; mixing kernel and userland targets in one workspace fights the toolchain. Sibling project = independent `target/`, independent `build-std`, no `--target` gymnastics. Matches the EuraliOS / MaestrOS pattern. |
| **D10** | **Watchdog policy for ring-3 processes:** in the timer handler, when `cs.RPL == 3` is detected, update `last_activity_tick` to current. | A CPU-bound but otherwise healthy user app should not get reaped by the existing 1000-tick (~10 s) watchdog merely for not making syscalls. A wedged user app — one that has fallen off the scheduler — still trips the watchdog because the timer never sees CPL=3 for it. |
| **D11** | **NX/WX hygiene applied now even though `EFER.NXE` is not enabled today.** `.text` mapped R+X (no W); `.rodata` mapped R (no W, NX); `.data`/`.bss`/stack/GOT mapped R+W+NX; user-trampoline page R+X (no W). USER_ACCESSIBLE on every parent PT entry on the user path. | Setting bits correctly today means a future `EFER.NXE = 1` flip is a one-line change. The parent-entry USER bit is the #1 OSDev landmine — `Mapper::map_to` does not propagate USER on existing parents; we must use `map_to_with_table_flags` and explicitly upgrade pre-existing parents. |

---

## High-Level Technical Design

The end-to-end run flow, from shell verb to exit, framed for design review. **This is directional guidance, not implementation specification.** The implementer should treat it as context.

```mermaid
sequenceDiagram
    participant User as User
    participant Shell as Shell (ring 0)
    participant Run as run command
    participant Loader as ELF loader
    participant MM as paging / frames
    participant CPU as CPU
    participant App as Hello app (ring 3)
    participant Sys as int 0x80 handler

    User->>Shell: run /HOST/HELLO.ELF
    Shell->>Run: dispatch via command registry
    Run->>Loader: bytes = File::open_read("/HOST/HELLO.ELF")
    Loader->>Loader: parse ELF64 header + program headers
    Loader->>MM: alloc frames; map_user_region (USER+R+X / R+W+NX)
    Loader->>MM: map user stack + zero .bss
    Loader->>Loader: walk RELA; resolve names via SYSCALL_TABLE
    Loader->>Loader: patch GOT slots → user-trampoline-page addresses
    Loader-->>Run: Ok(UserImage)
    Run->>Run: save kernel continuation; set TSS.rsp0
    Run->>CPU: iretq frame { user_ss=0x1B, user_rsp, rflags=0x202, user_cs=0x23, user_rip=entry }
    CPU->>App: enter ring 3 at _start
    App->>App: stub: mov rax, PRINT_ID; int 0x80
    CPU->>Sys: vector 0x80 (DPL=3 entry; CPU loads rsp0)
    Sys->>Sys: validate (ptr, len) inside user mappings
    Sys->>Sys: call kernel println! (current_output_terminal still set)
    Sys-->>App: iretq back to ring 3
    App->>App: stub: mov rax, EXIT_ID; int 0x80
    CPU->>Sys: vector 0x80
    Sys->>Run: long-jump to saved continuation (NOT iretq)
    Run->>Loader: drop UserImage → unmap + free frames
    Run->>Shell: clear_current_output_terminal; notify_command_finished
    Shell->>User: prompt
```

Fault path (alternative ending): on any ring-3 #PF / #GP / #UD / #DE / #AC / #SS, the exception handler detects `cs.RPL == 3`, calls `cleanup_user_process(pid, AbnormalExit { vector, error_code })`, and long-jumps to the same continuation as `exit`. The shell sees a non-zero exit and prints a diagnostic.

---

## Implementation Units

### U1. GDT, TSS, ring-3 entry primitive

- **Goal.** Build a minimum GDT (D2) and a single static TSS (D6); expose a kernel-only primitive that constructs an `iretq` frame for ring 3 and transfers control. Verifiable without any ELF parsing — drives a few hand-rolled bytes in ring 3 that issue `int 0x80` and `hlt`.
- **Requirements.** R4, R10.
- **Dependencies.** None.
- **Files.**
  - `src/arch/x86_64/gdt.rs` (new) — `GlobalDescriptorTable` with the D2 layout; `lazy_static!` static GDT + static TSS; `init()` that loads GDT, reloads CS via far return, loads SS/DS/ES, `ltr`s the TSS; an IST[0] stack for `#DF`.
  - `src/arch/x86_64/mod.rs` — re-export `gdt`.
  - `src/arch/x86_64/CLAUDE.md` (new or extend; `src/arch/` has no folder file today) — document the selector layout as a hard interface that `context_switch.rs` and `preemption.rs` depend on.
  - `src/kernel.rs` — call `gdt::init()` early in boot, before any later subsystem that might trigger interrupts using the new TSS.
  - `src/tests/userland.rs` (new, scaffolded here) — first test only: `gdt_loads_and_tr_set`.
- **Approach.** Use `x86_64::structures::gdt::GlobalDescriptorTable`, `Descriptor::user_code_segment()`, `Descriptor::user_data_segment()`, `Descriptor::tss_segment(&'static TaskStateSegment)`. Build the TSS with a 16 KiB kernel `rsp0` stack (statically allocated, page-aligned) and a 4 KiB IST[0] stack. Wire `#DF` to `IST[0]` via `EntryOptions::set_stack_index(0)`. The selector returned by `add_entry` already has the correct RPL.
- **Patterns to follow.** `lazy_static!` static-init pattern used elsewhere in the kernel (`STATIC_MAPPER`, `MOUNTED_FAT_WRAPPERS`). No `Box::leak`.
- **Test scenarios.**
  - GDT loaded; `tr` (task register) is non-zero after `init()`.
  - Reading the kernel CS selector returns `0x08`; reading kernel SS returns `0x10`. The hard-coded asm constants still work.
  - Smoke: a unit test triggers a `#DF` via deliberate stack overflow (or a synthetic invocation) and observes the IST[0] stack was used (e.g., the handler sets a sentinel and returns). This validates that an IST entry actually saves us; if scoping pressure is high, fold this into U2 instead.
- **Verification.** Kernel boots; existing tests still pass; `gdt_loads_and_tr_set` passes; the existing scheduler & preemption keep working (no regression from the new GDT).

### U2. Exception-handler refactor for ring-3 faults

- **Goal.** Refactor every exception handler in `src/arch/x86_64/interrupts.rs` to detect `(frame.code_segment & 3) == 3` and route to `cleanup_user_process(reason: AbnormalExit)` instead of `panic!()`. The function name is canonical from here through U7 — do not introduce a separate `kill_current_user_process` placeholder. In U2 it logs and halts; U7 promotes it to the real teardown.
- **Requirements.** R7, R8.
- **Dependencies.** U1 (TSS must exist before the handler can rely on `rsp0`).
- **Files.**
  - `src/arch/x86_64/interrupts.rs` — `page_fault`, `general_protection_fault`, `invalid_opcode`, `divide_error`, `overflow`, `alignment_check`, `stack_segment_fault`, `bound_range_exceeded` handlers.
  - `src/mm/paging.rs::handle_page_fault` — currently auto-maps anything in heap/stack range. Add a guard: if `(frame.code_segment & 3) == 3` and the address is outside the user VA range, do NOT call `handle_page_fault`; route to user-fault path instead. Inside the user VA range, still do not auto-map — user faults are lifecycle events, not lazy mappings (per `src/mm/CLAUDE.md` philosophy).
  - `src/userland/lifecycle.rs` (new) — `cleanup_user_process(reason: AbnormalExit)` placeholder (logs + halts in U2; U7 promotes to real teardown). Define `AbnormalExit { vector: u8, error_code: Option<u64>, fault_addr: Option<VirtAddr> }`.
- **Approach.** A small inline helper `cs_is_user(frame) -> bool` reused across all handlers. Kernel-side faults retain their current `panic!` behavior (still bugs we want loud); only the ring-3 case branches to the new path. `panic!` inside an interrupt handler remains forbidden per `.claude/rules/panic-and-attributes.md` — the user-fault path must not panic; it logs + cleanly returns or halts the placeholder.
- **Patterns to follow.** Existing handler structure in `src/arch/x86_64/interrupts.rs`. Uniform diagnostic logging via `crate::lib::debug`.
- **Execution note.** Refactor handlers BEFORE enabling user-mode execution end-to-end. Otherwise, U6/U7 test failures kernel-panic the harness (per the flow analysis sequencing concern).
- **Test scenarios.**
  - Synthetic: invoke each handler from a kernel-mode context (e.g., divide-by-zero in a test) and confirm the `cs.RPL == 0` branch still panics the kernel as today (no regression).
  - Synthetic: construct a fake `InterruptStackFrame` with `code_segment | 3` set in a unit test and verify the user-routing branch is taken.
  - The page-fault handler does NOT auto-map a fault from CPL=3 even if the address falls inside the kernel heap range.
- **Verification.** All existing exception tests pass. New unit tests for the user-routing branch pass.

### U3. PCB + context-switch ring-3 awareness

- **Goal.** Extend `CpuContext` and `PCB` to carry user-mode bookkeeping (saved CS, SS; user-process flag; user VA bounds; kernel `rsp0` value). Replace literal `0x08` / `0x10` pushes in `preemption.rs` and `context_switch.rs` with values from the saved context. Timer handler (D7): when preempting from `cs.RPL == 3`, update `last_activity_tick` (D10) and `iretq` immediately back to user (no scheduler entry — single-app-synchronous). **Critical sequencing:** the `cs.RPL == 3` check must execute at the very top of `timer_handler_inner`, reading `frame.code_segment` from the raw `InterruptStackFrame` *before* any save-to-PCB logic runs. The existing handler unconditionally writes `frame.rsp`/`frame.rip`/`frame.rflags` into the active `CpuContext`; if the new check runs after that, it has already corrupted the kernel-side `run` PCB with user-mode register values, and the next `switch_to_full_context_iretq` will push kernel CS=0x08 with a user RIP and double-fault. Append the new fields to the *end* of `CpuContext` so all existing naked-asm offsets remain stable.
- **Requirements.** R10, R7.
- **Dependencies.** U1.
- **Files.**
  - `src/process/context.rs` — add `cs: u16`, `ss: u16` fields to `CpuContext`. Default to `0x08`/`0x10` for kernel processes (preserves existing behavior).
  - `src/process/pcb.rs` — add `is_user_process: bool`, optional `user_image_id: Option<UserImageId>` (handle into U6's UserImage registry).
  - `src/arch/x86_64/preemption.rs` — replace literal `0x08`/`0x10` pushes with values from the saved CpuContext. Insert the `(frame.code_segment & 3) == 3` early-return at the top of `timer_handler_inner` (before any save/yield logic): refresh `last_activity_tick` on the active user PCB and bail. Detect CPL exclusively from the raw interrupt frame, never from a PCB field (a stale PCB would silently mis-route).
  - `src/arch/x86_64/context_switch.rs` — same.
  - `src/process/CLAUDE.md` — refresh the stale "scaffolding not wired" note (the scheduler is active and load-bearing today; an AI agent reading this file during U3 implementation must not be misled).
- **Approach.** The literal pushes today happen in naked asm; refactor to load CS/SS from a known offset in the per-process state struct (a small per-CPU "current context" pointer). For kernel processes nothing observable changes.
- **Patterns to follow.** Existing scheduler register save/restore in `preemption.rs:54+`. Keep the same struct layout discipline; document the offsets if asm hard-codes them.
- **Test scenarios.**
  - Existing kernel-process scheduling tests pass (no regression — kernel CS/SS still 0x08/0x10).
  - Synthetic: a "fake user process" CpuContext with `cs=0x23, ss=0x1B` is preempted; the resume path's iretq frame contains those exact selectors, not the kernel ones. Verifiable in a unit test by inspecting the constructed frame in memory.
  - Watchdog test: a fake CPL=3 preemption frame triggers the `last_activity_tick` refresh in the timer ISR.
- **Verification.** Tests pass; existing 18 commands still spawn and run unchanged; preemption load test (busy kernel work) does not regress.

### U4. User VA range + USER_ACCESSIBLE mapping API

- **Goal.** Define the user VA range. Add `MemoryMapper::map_user_region(virt_range, perms)` and `unmap_user_region(virt_range)` using `map_to_with_table_flags` so USER_ACCESSIBLE is propagated to parent table entries (D11). User-page faults are NOT auto-handled.
- **Requirements.** R4, R9.
- **Dependencies.** U2 (page-fault handler must not auto-map user-range faults).
- **Files.**
  - `src/mm/paging.rs` — new `map_user_region`, `unmap_user_region`, `upgrade_parent_user_bits` helpers.
  - `src/mm/CLAUDE.md` — document the new VA partition.
  - `src/mm/memory.rs` — frame-tracking helper for the loader's transactional teardown (return a list of frames the unmap freed so callers can verify in tests).
- **Approach.**
  - User binary load base: `0x0000_0000_0040_0000` (matches D3; far below kernel heap at `0x_4444_4444_0000` and process stacks at `0x_5555_0000_0000`).
  - User stack: `[0x0000_0000_0080_0000 - stack_size, 0x0000_0000_0080_0000)` initially; one guard page below the bottom mapped not-present.
  - User trampoline page: `0x0000_0000_0090_0000` (single 4 KiB page, R+X, USER, NX off).
  - Use `Mapper::map_to_with_table_flags(page, frame, leaf_flags, parent_flags = PRESENT | WRITABLE | USER_ACCESSIBLE, allocator)` and treat `MapToError::PageAlreadyMapped` as a hard error — the user range must be empty when load begins.
- **Patterns to follow.** Existing `MemoryMapper` API in `src/mm/paging.rs`. Static-slot pattern for any per-process tracking data.
- **Test scenarios.**
  - `map_user_region` followed by a kernel-mode read of the page: succeeds (kernel can read user memory; the inverse is what's blocked).
  - After `map_user_region`, every parent PT entry on the path (PML4 → PDPT → PD → PT) has the USER bit set. Verify by walking the page tables in a test.
  - `unmap_user_region` returns the freed frames; reading the page after unmap faults.
  - Mapping a region that overlaps the kernel heap range returns `Err`, does not silently overwrite. (Defensive — should never happen given D3, but covers the validation path.)
  - Fault at a user-range address from CPL=3 does NOT auto-map (per U2), confirming the API and the fault handler are disjoint.
- **Verification.** Tests pass; `src/mm/CLAUDE.md` updated.

### U5. int 0x80 syscall transport, symbol table, user-trampoline page

- **Goal.** Wire the syscall transport. IDT vector `0x80` with DPL=3 (D1). Build a `SYSCALL_TABLE` registry mapping `name → (id, kernel_handler)`. Construct the user-trampoline page (D4) at `0x0090_0000` containing one stub per registered syscall. Register `print(ptr, len)` and `exit(code)` as the first two syscalls.
- **Requirements.** R5, R6.
- **Dependencies.** U1, U4.
- **Files.**
  - `src/arch/x86_64/syscall.rs` (new) — **two-piece handler.** `extern "x86-interrupt"` does NOT expose GP regs, so the IDT vector points at a small `#[naked]` stub that pushes RAX/RDI/RSI/RDX/R10/R8/R9 onto the stack into a `SyscallArgs { rax, rdi, rsi, rdx, r10, r8, r9 }` struct, calls a regular Rust dispatcher `syscall_dispatch(&mut SyscallArgs) -> i64`, writes the return value back into the saved RAX slot, restores regs, and `iretq`s. Mirrors the pattern in `src/arch/x86_64/preemption.rs::timer_interrupt_handler_preemptive` (naked outer wrapper + Rust inner handler).
  - `src/arch/x86_64/interrupts.rs` — register the naked stub at vector `0x80` with `EntryOptions::set_privilege_level(PrivilegeLevel::Ring3)`. Use a trap gate (preserves IF) or interrupt gate (clears IF on entry) — interrupt gate is the safer default; document the choice.
  - `src/userland/abi.rs` (new) — `register_syscall(name: &'static str, handler: fn(&mut SyscallArgs) -> i64)`. Static-slot table (`[Option<SyscallEntry>; MAX_SYSCALLS]`, MAX = 64 to start).
  - `src/userland/syscalls.rs` (new) — `print_handler(ptr, len)` validates the user pointer range against the current PCB's user VA bounds, then writes via the kernel's existing `crate::print!` (the shell already set `current_output_terminal` before spawning the `run` command). `exit_handler(code)` records the code on the PCB and triggers the long-jump to the saved continuation (D7).
  - `src/userland/trampoline.rs` (new) — emits a 4 KiB page where each stub is `mov rax, <id>; int 0x80; ret` (~9 bytes); maps it into the user address space. Records `(name → trampoline VA + offset)` so the loader (U6) can patch GOT slots.
- **Approach.** The print syscall validates `(ptr, ptr+len)` lies within mapped user pages via the PCB's recorded user-VA bounds; rejects with an `EFAULT`-style error otherwise. UTF-8 is trusted (truncate on invalid). `print` adds no newline; callers include it explicitly.
- **Patterns to follow.** IDT entry registration in existing `src/arch/x86_64/interrupts.rs`. Static-slot table from `src/process/stack.rs`. Error propagation from `src/fs/file_handle.rs`.
- **Test scenarios.**
  - Synthetic: register a "nop" syscall; invoke `int 0x80` with its ID from a kernel-mode test driver; verify the handler ran.
  - Trampoline page byte layout: `mov rax, imm32; int 0x80; ret` for ID=0 and ID=1 are correctly assembled (compare bytes).
  - `print(valid_ptr, len)` from a fake-user context calls `crate::print!` exactly once with the right slice.
  - `print(0xffff_8000_0000_0000, 5)` (kernel address) is rejected without dereferencing the pointer.
  - `print(end_of_page - 5, 100)` spanning into an unmapped user page is rejected.
  - `print` with `len = 0` succeeds and prints nothing.
  - `exit(42)` triggers the long-jump and records `42` as the exit code on the PCB.
- **Verification.** All synthetic tests pass; trampoline page is mapped USER+R+X; IDT entry has DPL=3.

### U6. ELF loader + relocations

- **Goal.** Parse a static non-PIE ELF64 (D3); validate; map PT_LOAD segments via `map_user_region` with correct WX/U flags (D11); zero `.bss`; allocate user stack with a guard page; walk `R_X86_64_GLOB_DAT` / `R_X86_64_JUMP_SLOT` relocations and patch GOT slots to point into the user-trampoline page (D4) using addresses from `SYSCALL_TABLE`; produce a `UserImage` handle (D8) whose `Drop` unmaps and frees on either success commit-into-PCB or failure rollback.
- **Requirements.** R2, R5, R8, R9.
- **Dependencies.** U4, U5.
- **Files.**
  - `src/userland/loader.rs` (new) — entry point `load_elf(bytes: &[u8]) -> Result<UserImage, LoaderError>`.
  - `src/userland/image.rs` (new) — `UserImage { entry: VirtAddr, frames: Vec<PhysFrame>, mappings: Vec<PageRange>, stack_top: VirtAddr, user_va_bounds: VirtRange }`. `impl Drop` unmaps and frees.
  - `src/userland/error.rs` (new) — `LoaderError { BadMagic, WrongArch, WrongType, Truncated, OverlappingPtLoad, VaOutOfRange, EntryNotMapped, UnsupportedReloc(u32), UnresolvedImport(&'static str), TlsUnsupported, AlignmentBad, OutOfFrames }`.
  - `src/userland/mod.rs` — module wiring.
  - `Cargo.toml` — add `xmas-elf` (no_std, zero alloc) OR commit to a hand-rolled minimal parser. Recommend hand-rolled (~150 lines) given the static non-PIE constraint and `no_std` discipline; reassess at U8 if the parse code grows.
- **Approach.**
  - **Validation phase first, allocation second.** Parse all program headers, validate magic / class=ELFCLASS64 / data=ELFDATA2LSB / machine=EM_X86_64 / type=ET_EXEC / e_phnum reasonable; check every PT_LOAD has `p_align == 0x1000` and that `p_offset % 0x1000 == p_vaddr % 0x1000`; check no PT_LOAD overlaps another or any reserved kernel VA; check `e_entry` lies inside some PT_LOAD; reject PT_TLS and PT_INTERP.
  - **Allocation phase.** Only after full validation: alloc frames, map regions with leaf flags from `p_flags` (PF_X→no-NX; !PF_W→no-WRITABLE; always USER + present), copy `p_filesz` bytes, zero `[p_filesz, p_memsz)`.
  - **Relocation phase.** Walk `.rela.dyn` and `.rela.plt`. For each entry: resolve symbol name in `SYSCALL_TABLE`. If found, write `trampoline_va + offset` into `*(load_base + r_offset)` for `R_X86_64_GLOB_DAT` and `R_X86_64_JUMP_SLOT`. Reject any other relocation type with `UnsupportedReloc`.
  - **Stack phase.** Map user stack at the fixed range (D4 / U4). Place a guard page below.
  - On any error, drop the partial `UserImage` — its `Drop` unmaps and frees what was committed.
- **Patterns to follow.** Transactional handle pattern with `Drop` cleanup (analogous to `src/fs/file_handle.rs`'s `Arc<File>` lifetime). Typed error from `src/fs/filesystem.rs`. Use the kernel's custom `Arc` if any sharing is needed.
- **Execution note.** Implement validation paths and their negative tests test-first — they're easy to assert against and the most common failure surface for malformed input.
- **Test scenarios.**
  - **Happy path:** load a fixture ELF (a 1 KiB embedded blob in the test binary representing a static-non-PIE ELF that calls `print` once and `exit(0)`); `UserImage` returned with correct `entry`; trampoline-resolved GOT slots point into the trampoline page.
  - **Bad magic:** 4-byte file `"XXXX"` → `LoaderError::BadMagic`.
  - **Wrong arch:** ELF with `e_machine == EM_AARCH64` → `LoaderError::WrongArch`.
  - **Wrong type:** ELF with `e_type == ET_REL` → `LoaderError::WrongType`.
  - **Truncated:** header claims 4 program headers, file ends after 2 → `LoaderError::Truncated`.
  - **VA overlaps kernel:** crafted PT_LOAD at `0x_4444_4444_0000` → `LoaderError::VaOutOfRange`.
  - **Overlapping PT_LOAD:** two segments with overlapping VA ranges → `LoaderError::OverlappingPtLoad`.
  - **Entry outside any PT_LOAD:** crafted `e_entry` past all loaded segments → `LoaderError::EntryNotMapped`.
  - **Unsupported reloc:** ELF with `R_X86_64_TPOFF64` → `LoaderError::UnsupportedReloc(R_X86_64_TPOFF64)`.
  - **Unresolved import:** ELF imports `nonexistent_kernel_symbol` → `LoaderError::UnresolvedImport("nonexistent_kernel_symbol")`.
  - **PT_TLS rejected explicitly:** ELF with `PT_TLS` → `LoaderError::TlsUnsupported` (not silently ignored).
  - **`.bss` zero-fill:** ELF where `_start` reads a `.bss` global initialized to zero — happy-path execution test asserts the read returned zero (deferred to U7's E2E test, but loader-side: verify `[p_filesz, p_memsz)` is zeroed in the mapped page after load).
  - **Frame leak on rollback:** capture `frame_allocator.free_count()`; load an ELF that fails at the relocation phase (e.g., unresolved import); after the error returns, free count is unchanged from before the attempt.
- **Verification.** All scenarios above pass. The fixture ELF can be built once via the userland project (U8) and embedded in the test binary as `include_bytes!`.

### U7. `run` shell verb + lifecycle integration

- **Goal.** Register `run` as a shell command. Read `/HOST/<NAME>.ELF`, call the loader, set up the iretq frame, save the kernel continuation (D7), `iretq` to ring 3, and on return (whether via `exit` syscall or fault) tear everything down. Wire `cleanup_user_process` (the U2 placeholder) to the same teardown. Assert the single-user-app invariant (D5).
- **Requirements.** R3, R6, R7, R9, R10.
- **Dependencies.** U2, U3, U5, U6.
- **Files.**
  - `src/commands/run/mod.rs` (new) — mirror the structure of `src/commands/cat/mod.rs`. `RunProcess` impl of `RunnableProcess`. Factory `create_run_process`.
  - `src/userland/lifecycle.rs` — promote U2's placeholder to the real `cleanup_user_process(pid, exit_reason)`. Drops the `UserImage` (which unmaps + frees), clears the user-process slot, calls `clear_current_output_terminal`, and `notify_command_finished(tid)`.
  - `src/userland/mod.rs` — public `enter_user_mode(image: UserImage, pcb_id: usize)` that builds the iretq frame and uses naked asm to push selectors / RSP / RFLAGS / CS / RIP and execute `iretq`. Saves the kernel continuation (RSP + label) into a per-CPU "current user return point" before iretq.
  - `src/process/pcb.rs` — track the active `UserImage` handle (transferred from loader on commit).
  - `src/kernel.rs` — `register_command("run", create_run_process)` next to existing entries.
  - `src/commands/mod.rs` — `pub mod run;`.
  - `src/process/CLAUDE.md` — refresh to remove the now-stale "scaffolding not wired" note (the recent learnings research flagged this).
- **Approach.**
  - The `run` command parses `args[0]` as the path; rejects no-arg invocations.
  - Reads the file via `crate::fs::File::open_read(path)?.read_to_vec()?`. Errors propagate as `Result<(), String>` matching the existing process error style.
  - Calls `loader::load_elf(&bytes)`; on `Ok(image)`, transfers ownership into a per-CPU active-user slot.
  - Asserts no other user app is currently active (panic in debug, return error in release — matches the "internal assumption" boundary in CLAUDE.md guidance).
  - Sets `TSS.privilege_stack_table[0]` to the kernel rsp0 stack top.
  - Saves the kernel continuation, then enters user mode.
  - On return (long-jump from `exit` syscall, or from a fault handler via `cleanup_user_process`): consumes the active-user slot, drops the `UserImage`, clears the active-user slot, returns from the closure passed to `spawn_process`. The shell's existing `notify_command_finished` flow (in `src/process/manager.rs`) takes over.
- **Patterns to follow.** Existing command structure in `src/commands/cat/mod.rs`. Long-jump / continuation pattern is new; prefer a small, well-commented `unsafe` helper in `src/userland/lifecycle.rs` rather than spreading naked asm across files.
- **Test scenarios** (these are the end-to-end / integration tests):
  - **T-E2E-happy:** load the embedded fixture ELF, run, assert the kernel-side serial output captured "hello\n" and the recorded exit code is 0.
  - **T-E2E-shell:** boot the OS with `host_share/HELLO.ELF` present, type `run /HOST/HELLO.ELF` programmatically (via the shell command path), assert the same observable outcome and that the prompt returns.
  - **T-E2E-fault-UD:** fixture ELF whose first instruction is `0F 0B` (UD2): `run` returns to shell, a diagnostic is logged, no kernel panic, frame allocator returns to baseline.
  - **T-E2E-fault-PF:** fixture ELF that derefs `0xdead_beef_0000` from ring 3: same outcome.
  - **T-E2E-fault-GP:** fixture ELF that executes `cli`: same outcome.
  - **T-E2E-bad-pointer-syscall:** fixture ELF that calls `print` with a kernel-range pointer: app receives a non-zero return from the syscall, exits cleanly with that code; no kernel-memory disclosure.
  - **T-E2E-watchdog:** fixture ELF whose `_start` is `jmp _start` — busy spin without syscalls. After ~10 s the watchdog kills the app; teardown runs; prompt returns. (Confirms D10 behavior — preempted ring-3 work counts as activity, but the kill path still works for processes that have fallen off the scheduler. NB: a tight `jmp $` will be repeatedly preempted, so D10 keeps it alive forever — see the Risk Analysis row.)
  - **T-E2E-leak-loop:** load + exit the happy-path fixture 100 times in a row; frame allocator free count returns to baseline after each run.
  - **T-E2E-fault-leak-loop:** same, but with the UD2 fault fixture; frame count returns to baseline.
  - **T-E2E-no-arg:** `run` with no path argument returns an error to the shell, no panic.
  - **T-E2E-missing-file:** `run /HOST/MISSING.ELF` returns the FS-layer error, no panic.
  - **T-E2E-second-app-rejected:** while a long-running fixture is in user mode, somehow re-entering `run` (e.g., from a kernel-test driver simulating a second invocation) returns the single-user invariant error, not a state-corrupting double-load.
- **Verification.** All E2E tests pass on `./test.sh`. Boot with `./build.sh`, type `run /HOST/HELLO.ELF` interactively, observe "hello" in the terminal, prompt returns.

### U8. Userland sibling cargo project + first hello app + build orchestration

- **Goal.** Stand up `userland/` as a sibling cargo project (D9). Provide a minimal user-side runtime (`#[panic_handler]`, `_start`, `mem*` shims). Produce `userland/apps/hello/` whose `_start` calls `print("hello\n"); exit(0)`. Extend `build.sh` to build userland first, copy the artifact to `host_share/HELLO.ELF` (uppercase 8.3), then build the kernel. Update `test.sh` symmetrically so the same artifact is present in test boots.
- **Requirements.** R1, R2, R6.
- **Dependencies.** U6 (loader must accept the format we produce — design and test in lockstep). The U6 fixture ELF can be the early build of this app, embedded via `include_bytes!` for unit tests.
- **Files.**
  - `userland/Cargo.toml` — sibling project; `[[bin]]` per app; `panic = "abort"` profiles; `lto = true`, `strip = true` in release.
  - `userland/.cargo/config.toml` — `target = "x86_64-unknown-none"` (start here; create a custom target spec only if needed); `[unstable] build-std = ["core", "compiler_builtins"]`, `build-std-features = ["compiler-builtins-mem"]`.
  - `userland/linker.ld` — `ENTRY(_start)`, `. = 0x40_0000`, sections `.text .rodata .data .bss`. No `PT_INTERP`. `-z noexecstack -z now`.
  - `userland/runtime/Cargo.toml`, `userland/runtime/src/lib.rs` — exposes `pub extern "C" fn print(ptr, len)`, `pub extern "C" fn exit(code)` as **unresolved external symbols** (declared, not defined). The linker emits relocations for them; the kernel loader patches the GOT to the trampoline page. Provides `#[panic_handler]` that calls the unresolved `exit` (so a panic in user code is a clean exit, not a UD2).
  - `userland/apps/hello/Cargo.toml`, `userland/apps/hello/src/main.rs` — `#![no_std] #![no_main]`; `#[no_mangle] pub extern "C" fn _start() -> !`; calls `runtime::print(MSG.as_ptr(), MSG.len()); runtime::exit(0)`.
  - `build.sh` — new step: `cargo build --release --manifest-path userland/Cargo.toml`; `cp userland/target/x86_64-unknown-none/release/hello host_share/HELLO.ELF`. Stage uppercase 8.3. Run before the kernel build.
  - `test.sh` — same staging step; ensures the test boot has `HELLO.ELF` available.
  - `.gitignore` — `userland/target/`.
- **Approach.**
  - Userland is **declarative-only** about `print`/`exit`: the runtime crate marks them `extern "C"` with no body. The actual implementation is the trampoline page the kernel maps in. This is the load-bearing trick that makes the symbol-keyed ABI work without a linker plugin: the user binary just has unresolved relocations, and the kernel loader resolves them.
  - Linker invocation must NOT mark these symbols as needing a dynamic library at runtime — `--unresolved-symbols=ignore-in-object-files` is too broad; the right knob is to emit them as undefined weak / undefined dynamic symbols and let the kernel loader resolve. Concrete linker incantation is an implementation detail; the goal is "produce GLOB_DAT/JUMP_SLOT relocations referring to these names."
  - If linker tooling proves stubborn, fallback option: provide kernel-resolved syscall stubs in the runtime crate as `extern "C" fn print(ptr, len) { unsafe { asm!("mov rax, 0; int 0x80", ...) } }` — direct numeric syscall — and skip the symbol-keyed mechanism for the runtime crate's two functions. This degrades to the standard libc model for those two; future symbols added without rebuilding the runtime would still need the symbol-keyed path. Capture this fallback in deferred work.
  - Build-pipeline race: stage to `host_share/.HELLO.ELF.tmp` then atomic rename to `host_share/HELLO.ELF` so a parallel QEMU snapshot does not catch a half-written file (per the flow analysis).
- **Patterns to follow.** Conductor-onboarding plan (`docs/plans/2026-05-08-001-feat-conductor-build-onboarding-plan.md`) for build-script artifact discipline (manifest-relative paths, env-var overrides). vvfat plan (`docs/plans/2026-05-08-003-feat-host-folder-mount-vvfat-plan.md`) for `host_share/` semantics (read-only, snapshot-at-boot, 8.3 uppercase).
- **Test scenarios.**
  - **T-userbuild-1:** `cargo build --release --manifest-path userland/Cargo.toml` produces an ELF.
  - **T-userbuild-2:** the produced ELF passes the U6 loader's validation (run U6 happy-path tests against this artifact).
  - **T-userbuild-3:** `build.sh` stages the artifact at `host_share/HELLO.ELF` (uppercase 8.3 visible).
  - **T-userbuild-4:** `test.sh` likewise stages it; a kernel test that reads `/HOST/HELLO.ELF` succeeds.
  - **T-userbuild-5:** running `build.sh` twice in succession does not result in a stale or corrupt staged file.
  - **T-userbuild-6:** running `build.sh` while another `build.sh` is mid-stage does not interleave; one finishes with a complete file.
- **Verification.** Boot with `./build.sh`; observe `HELLO.ELF` present in `/host` from inside the OS (`ls /HOST`); type `run /HOST/HELLO.ELF`; see "hello" in the terminal; prompt returns. The full E2E loop matches the Mermaid diagram.

---

## Scope Boundaries

**In scope:**
- Single user app at a time, synchronous to completion.
- Static non-PIE ELF format, fixed user load base.
- `int 0x80` syscall transport with name-keyed ABI via user trampoline page.
- `print`, `exit` syscalls only.
- One first app: `HELLO.ELF`.
- Cleanup on cooperative exit, fault, and watchdog kill.
- GDT, TSS, IST[0] for `#DF`.

### Deferred for later

- **Multitasking of user processes.** Per-process address spaces (Cr3 swap), TSS.rsp0 multiplexing across user processes, scheduler-managed coexisting user PCBs.
- **`syscall`/`sysret` transport.** MSR setup (EFER.SCE, STAR, LSTAR, SFMASK), naked-asm entry stub with `swapgs` discipline, RCX-canonical-address mitigation. The chosen GDT layout (D2) keeps this door open.
- **Filesystem write support / longer filenames.** `host_share/` remains read-only and 8.3 uppercase; rebuild + reboot to update.
- **A libc port / POSIX compatibility.** Userland links only against the small kernel-provided ABI.
- **Dynamic loading of user-supplied shared libraries.** Only kernel-exported symbols resolve.
- **TLS in user binaries** — `PT_TLS` is rejected explicitly by the loader; revisit when an app actually needs it.
- **Static-PIE / ASLR for user binaries.** Non-PIE for now; revisit if multiple user apps coexist at runtime.
- **`EFER.NXE = 1`.** NX bits are set in page-table entries today (D11) but not enforced. Flipping the bit is a follow-up unit.
- **A second / third app.** The platform supports any further app; this plan ships only HELLO.

### Deferred to Follow-Up Work

- **Linker-tooling fallback.** If the symbol-keyed ABI's GLOB_DAT/JUMP_SLOT generation proves fragile in U8, the temporary fallback (numeric `int 0x80` stubs in the runtime crate) ships and a follow-up unit hardens the linker invocation.
- **Frame-leak harness as a permanent test fixture.** Once U6/U7 land, generalize the per-test free-count snapshots into a shared utility under `src/tests/`.
- **Stale-binary fingerprint logging.** Log the loaded ELF's size and an XOR-fold hash on `run` so a stale boot is observable in serial output.

### Outside this plan's identity

- This plan is not the path to a Linux-compatible OS. POSIX, ELF interp / dynamic linker, signals, process groups, fork+exec are all explicit non-goals.
- This plan does not replace the in-kernel command registry. Existing 18 commands continue to ship in-kernel.

---

## Risk Analysis & Mitigation

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| First `iretq` to ring 3 triple-faults due to GDT/TSS misconfiguration. | High (this is OSDev's classic landmine) | Hard reboot per failure; long debug cycle. | U1 lands first with a *non-ELF* ring-3 smoke test (hand-rolled bytes). Decouples "did I configure ring 3?" from "did I parse the ELF correctly?" Boot QEMU with `-d int,cpu_reset -no-reboot -no-shutdown` while debugging — the int log shows the exact fault sequence. |
| Existing naked-asm in `preemption.rs` / `context_switch.rs` resumes a preempted user process with kernel CS/SS. | High if not addressed; **silent** until preemption coincides with user code, then arbitrary ring-0 execution. | Triple fault or arbitrary code at ring 0. | U3 explicitly replaces the literal `0x08`/`0x10` pushes with PCB-driven values. Test T-E2E-watchdog and a "long-running user compute" regression test exercise the preempt-during-user-code path. |
| Forgetting USER_ACCESSIBLE on a parent PT entry — user code page-faults on first instruction, fault handler is broken because user-fault refactor is incomplete. | Medium | Triple fault; cryptic. | U2 lands before U6/U7 (sequencing constraint, called out in execution notes). U4 uses `map_to_with_table_flags` that explicitly propagates U bit to parents. A unit test walks parent PT entries to confirm the U bit. |
| Page-fault handler's existing "swallow PageAlreadyMapped" behavior masks a user/kernel mapping clash. | Medium | Stale mapping silently used; later memory corruption. | The user-mapping API (`map_user_region`) does NOT go through `handle_page_fault`; it calls `mapper.map_to_with_table_flags` directly and treats `PageAlreadyMapped` as a hard error. |
| Loader allocates frames, then fails at relocation phase, leaks frames. | Medium | Slow leak across runs. | `UserImage` Drop pattern + transactional load (validate first, allocate second). Test T-E2E-leak-loop and T-E2E-fault-leak-loop assert frame allocator returns to baseline. |
| Watchdog policy interacts oddly with user apps: a tight `jmp $` is preempted on every tick, D10 refreshes `last_activity_tick`, app never gets reaped. | Low (not a correctness bug, just a UX surprise) | A wedged-but-spinning user app must be killed via reboot. | Document explicitly. The contract is "ring-3 work that is being scheduled counts as alive." Add a follow-up unit to introduce a separate "ring-3 wall-clock budget" if this becomes a real problem. |
| Two parallel Conductor workspaces race on `host_share/HELLO.ELF` staging. | Low (workspaces do isolate `target/`) | One QEMU sees a half-written ELF and the loader returns Truncated. | U8 stages via tempfile + atomic rename. Even if the rename loses the race, the loader's typed error path keeps the kernel up and the user just retries. |
| `bootloader_api` `Mappings::physical_memory` not opted in → `BootInfo::physical_memory_offset` is `None`, kernel's existing `OffsetPageTable` construction silently breaks. | Already fine today (kernel works) | If it ever changes, a confusing hard-to-localize regression. | Document in `src/bootloader_config.rs` as a hard interface that user-mapping code depends on. Add a startup assertion that `physical_memory_offset` is `Some(_)`. |
| Linker-tooling: emitting GLOB_DAT/JUMP_SLOT relocations against undefined symbols in a static binary is non-default for `lld`. | Medium (this is the most probable U8 surprise) | Symbol-keyed ABI broken in practice; would force fallback. | U8 documents the fallback (numeric stubs in runtime crate) up front. The fallback ships a working print app; the symbol-keyed ABI hardens in a follow-up. |
| `vvfat` is read-only and snapshots at QEMU launch — every userland edit-test cycle requires reboot. | Certain (it's the design) | Slower iteration than developers expect. | Document in the `run` command's help text and in `userland/README.md`. virtio-9p is the natural follow-on if this becomes painful (per vvfat plan 003 deferred work). |

---

## Phased Delivery

The implementation units have a strict dependency order. Phasing the rollout into three logical milestones keeps the repo bootable and tests green at every step:

- **Phase A — Privilege machinery (U1, U2, U3).** Lands the ring-3 prerequisites (GDT, TSS, IST), refactors exception handlers for `cs.RPL == 3`, and teaches the scheduler to preserve user CS/SS. **Observable deliverable:** the kernel boots and runs all existing tests; a unit test in U1 successfully drives a few hand-rolled bytes in ring 3 that issue `int 0x80` and return.
- **Phase B — Mapping + transport + loader (U4, U5, U6).** Adds the user VA range and mapping API, the `int 0x80` dispatcher + symbol table + trampoline page, and the transactional ELF loader. **Observable deliverable:** the kernel can load an embedded fixture ELF and observe its first syscall; loader negative tests all pass.
- **Phase C — End-to-end (U7, U8).** Wires the `run` shell verb, the lifecycle/cleanup function, and the userland sibling project. **Observable deliverable:** boot with `./build.sh`, type `run /HOST/HELLO.ELF`, see "hello", prompt returns.

Each phase is mergeable independently; intermediate phases leave no dead code (every subsystem added is exercised by at least one test).

---

## Operational / Rollout Notes

- **Iteration cycle.** Editing a user app requires: rebuild userland → re-stage to `host_share/` → reboot QEMU. The vvfat snapshot-at-boot semantics are unchanged from plan 003. Document this in `userland/README.md` and in `run`'s help output.
- **Failure visibility.** All loader and lifecycle errors print to serial (and to the active terminal when the shell context is alive). Boot QEMU with `-d int,cpu_reset` while debugging the first `iretq` — Phase A development should keep this flag on by default in `test.sh` until U1 stabilizes.
- **Test-suite cost.** The E2E tests in U7 add boot time. Estimate ~1 s of additional test runtime; tolerable. If it grows, tag the most expensive tests as `#[cfg(feature = "slow_tests")]`.
- **Doc updates** (in scope, in the relevant unit):
  - `src/mm/CLAUDE.md` — user VA partition (U4).
  - `src/arch/x86_64/CLAUDE.md` (new or fold into existing) — GDT layout (U1).
  - `src/process/CLAUDE.md` — refresh the stale "scaffolding not wired" note (in U3 scope).
  - `docs/ARCHITECTURE.md` — add the userland subsystem (U7).
  - `docs/IMPLEMENTATION_PLAN.md` — reflect the new "userland" track (U7).
  - `README.md` — short "running a user app" section (U8).
- **No Conductor / runner changes needed.** `conductor.json`, `.conductor/setup.sh`, `.conductor/run.sh`, `.conductor/archive.sh` are unaffected. Each Conductor workspace gets its own `userland/target/` for free.

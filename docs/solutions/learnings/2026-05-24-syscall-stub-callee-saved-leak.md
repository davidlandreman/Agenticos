---
date: 2026-05-24
topic: kernel arch + userland scheduler
applies_to:
  - src/arch/x86_64/syscall.rs
  - src/userland/user_state.rs
  - src/userland/switch.rs
  - src/userland/syscalls.rs
  - src/userland/abi.rs
keywords:
  - SYSCALL
  - callee-saved
  - rbx
  - rbp
  - r13
  - r14
  - r15
  - capture_callee_saved
  - block_current_ring3_and_yield
  - read_user_callee_saved
  - musl errno
  - TCB
  - PROTECTION_VIOLATION
---

# Kernel-scratch values leaked into user rbx across blocking syscalls

## Symptom

After multi-ring-3 scheduling (U5–U10) landed, typing `ls` at the zsh
prompt segfaulted zsh 100 % of the time. The fault was always:

```
PAGE FAULT @ 0x4444448F1C{00,30,40}    (kernel-heap range)
RIP = 0x43FC50                          (musl: mov DWORD [rbx], 0)
error_code = PROTECTION_VIOLATION | CAUSED_BY_WRITE | USER_MODE
```

The fault address drifted by tens of bytes per boot but always sat
inside the kernel heap (`0x4444_4444_0000 + ~5 MiB`). Process state at
fault time looked clean — fs_base correct, current_user_pid correct,
CR3 correct.

## False trails (≈ 1.5 hours)

Spent a long time chasing TCB-corruption hypotheses:

- Added a TCB[0] integrity check at every syscall entry/exit, every
  timer ISR tick at CPL=3, and right before each `resume_ring3`'s iretq.
- All four checkpoints reported TCB[0] = `fs_base` = clean self-pointer
  in every observation, including the one immediately before the fault.
- Added a syscall-return-value scanner to catch any handler returning a
  kernel-heap pointer disguised as a user value. Never fired.
- Logged `read_handler`'s user buffer address (clean `0x3016100`, far
  from TCB) and confirmed only one byte (`'l'`) was copied before the
  fault.

The disassembly of the faulting RIP gave the breakthrough:

```
43fc38: call 0x512eab            ; __errno_location (mov rax, fs:0 ; add rax, 0x34)
43fc44: mov  rbx, rax            ; cache errno-ptr in rbx
43fc50: mov  DWORD PTR [rbx], 0  ; ← FAULTS
```

`__errno_location` reads `fs:0` correctly (TCB clean → returns a USER
address `0x58a42c`), but by the next iteration of the loop, `rbx`
contains a kernel-heap pointer. **Something between `mov rbx, rax`
and the next dereference was clobbering rbx with a kernel value.**

## Root cause

The SYSCALL stub in `src/arch/x86_64/syscall.rs` only pushed `r12`,
`r11`, `rcx`, and the seven `SyscallArgs` registers onto the kernel
stack. **It did not push user's `rbx`, `rbp`, `r13`, `r14`, `r15`.**
The plan was to lean on Rust's SysV calling convention to preserve
them across the dispatcher.

That works for the simple call/return path. The trap is the helper we
used to *capture* user callee-saved registers for fork / wait / signal
snapshots:

```rust
let mut callee = CalleeSavedSnapshot::default();
unsafe { capture_callee_saved(&mut callee as *mut _); }
```

`capture_callee_saved` was a naked-asm helper that read the **live**
`rbx/rbp/r12-r15`. But by the time it ran inside any syscall handler,
Rust's prologue for `syscall_dispatch_entry` had already executed:

```asm
0x126b10: push %r15
0x126b12: push %r14
0x126b14: push %r12
0x126b16: push %rbx          ; user's rbx saved to the kernel stack
0x126b17: sub  $0xE8, %rsp
0x126b1e: mov  %rdi, %rbx    ; ★ rbx is now the &SyscallArgs pointer
                              ;   (a kernel-stack address ~ 0x_5555_…)
0x126b5c: call capture_callee_saved
```

So `callee.rbx` recorded the **args pointer** (later: any kernel
scratch value the dispatcher happened to hold in rbx), not user's rbx.

The bug stayed dormant during simple syscalls because the SYSCALL
stub's iretq epilogue didn't write anywhere from the snapshot — Rust's
own push/pop preserved the user value on the kernel stack and
restored it.

It detonated only when a syscall **blocked**. The block path in
`block_current_ring3_and_yield` did:

```rust
let snapshot = UserState {
    …
    rbx: callee.rbx,   // ← garbage
    rbp: callee.rbp,
    …
};
process.saved_user_state = snapshot;
mark_ring3_blocked(me, reason);
resume_ring3(next_pid);
```

On wake, `resume_ring3`'s naked-asm faithfully restored the garbage rbx
to user mode via iretq. zsh resumed with `rbx = some_kernel_heap_ptr`,
re-executed SYSCALL (a no-op for rbx), got back to the loop body, and
the very next `mov [rbx], 0` faulted on the kernel address.

`ls` was the smallest interaction that exercised the bug because the
prompt-read in zsh is exactly "block on stdin → wake on Enter → loop
calls `__errno_location`."

## Fix

Two-part: capture user callee-saved registers **before any Rust runs**,
and read them from those explicit slots rather than live registers.

1. `src/arch/x86_64/syscall.rs` — SYSCALL stub now pushes user
   `r15/r14/r13/rbp/rbx` between the existing `push r12` (which stashes
   user r12, then loads user RSP into the r12 register) and the
   `push r11`/`push rcx` for RFLAGS/RIP. New stack layout, offsets
   measured from the `&SyscallArgs` pointer the stub passes to Rust:

   ```text
   [args +  56] rcx  (user RIP)
   [args +  64] r11  (user RFLAGS)
   [args +  72] rbx  (user)
   [args +  80] rbp  (user)
   [args +  88] r13  (user)
   [args +  96] r14  (user)
   [args + 104] r15  (user)
   [args + 112] r12  (original user R12)
   gs:[8]       rsp  (user RSP — written by the stub before stack switch,
                      stable for the whole syscall because FMASK masks IF
                      and no nested SYSCALL can overwrite it)
   ```

   The iretq epilogue gains five matching pops (`rbx`, `rbp`, `r13`,
   `r14`, `r15`) so user values are restored on the normal return path.

2. `src/userland/user_state.rs` — Added `read_user_callee_saved(args)`
   that reads from the new slots, plus `read_user_r12(args)` and
   `read_user_rsp()` (inline asm `mov gs:[8]`). Deleted
   `capture_callee_saved`. `CalleeSavedSnapshot` survives as the
   return type; the `r12_register` field still aliases user RSP for
   backward source compat.

3. Updated consumers — `block_current_ring3_and_yield`, `fork_handler`,
   `deliver_signal`, `maybe_deliver_signal`, `read_handler`,
   `wait4_handler`, `syscall_dispatch` — to drop the manual capture
   step. The `&CalleeSavedSnapshot` parameters on these functions are
   gone; the args pointer alone is enough now.

## How to avoid recurrence

- **Capture user GPRs only inside naked-asm at the actual ring-3 → 0
  transition.** Once a single byte of Rust has run, callee-saved
  registers may hold the dispatcher's locals. Reading them post-hoc
  yields lies that look like valid pointers and silently survive
  through save/restore.
- **If a register must round-trip through a save/restore path, the
  save site must be earlier than any compiler-controlled prologue.**
  Pushing in the naked stub is the only reliable spot in this kernel.
- **Test the blocking syscall path explicitly.** A correctness bug in
  user-state capture is invisible when syscalls complete synchronously
  (Rust's own push/pop unwinds correctly). It only surfaces when the
  snapshot is consumed by a resume — i.e., a blocking syscall that
  goes through `block_current_ring3_and_yield` → `resume_ring3`. zsh's
  interactive prompt was the first realistic workload that hit this
  path.

## Diagnostic gotcha

`__errno_location` returns `*(fs:0) + 0x34`. Whenever a ring-3 fault
address lands `0x34` above an unmapped-to-user page, the obvious
hypothesis is "TCB[0] is corrupt." That is *one* explanation; the
other — equally consistent with the symptom — is "rbx held the result
of a stale `__errno_location` call from before something clobbered the
register." Always check what loaded the source register, not just the
TLS area the symbol *would have* read.

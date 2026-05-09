---
date: 2026-05-09
topic: feat-userland-cpp-hello-world
---

# Static C++ Hello World on AgenticOS via Linux ABI

## Summary

Stand up enough Linux x86-64 ABI compatibility — `syscall` instruction, Linux syscall numbers, Linux-shaped initial stack with auxv, PT_TLS in the ELF loader, and a fault-driven minimum syscall surface — that a `g++ -static -no-pie` C++ "hello world" linked against musl + libstdc++ runs end-to-end on AgenticOS. The C++ source lives in the monorepo alongside the existing rust userland sample, and `build.sh` builds it and stages the ELF into `host_share/` for `/host`-based loading.

---

## Problem Frame

Today AgenticOS has a usable ring-3 platform (PR #12): an ELF64 static loader, a name-keyed `int 0x80` syscall table with two entries (`print`, `exit`), and a single rust userland app at `userland/apps/hello/` that the build pipeline stages into `host_share/HELLO.ELF`. The platform works, but it speaks a private ABI: any binary that wants to run has to link against `userland/runtime/`, target the custom `linker.ld`, and call only `print` and `exit`. There is no path from a stock host C/C++ toolchain to a binary that loads on the OS, because nothing on the kernel side speaks an ABI those toolchains know how to emit for.

The cost shape of staying private is obvious as the OS grows: every userland program is a port. A C app needs a libc port, a C++ app needs a libstdc++ port, a developer who wants to run something off-the-shelf cannot. The strategic horizon — eventually running `rustc` on the OS — is unreachable through this trajectory without writing a custom Rust target backend, libc, and toolchain. The cheapest move that opens that horizon is to commit, now, to Linux x86-64 ABI compatibility: implement the `syscall` instruction with Linux syscall numbers and the Linux initial-stack contract, and let stock musl-based static binaries run as-is. The first concrete milestone — and the artifact that proves the ABI choice was correct — is a `g++` C++ iostream "hello world" running unchanged.

---

## Actors

- A1. **Developer (human)** — writes and edits the C++ source under `userland/apps/`, runs `./build.sh` / `./test.sh`, types `run /HOST/HELLOCPP.ELF` in the guest shell, and reads its output.
- A2. **Host C++ toolchain** — a musl-based static cross-compiler (e.g., `x86_64-linux-musl-g++`) installed on the developer's machine; produces the ELF.
- A3. **Build system** — `build.sh` and `test.sh`, which orchestrate kernel and userland builds and stage the resulting ELF into `host_share/`.
- A4. **AgenticOS kernel** — loads the ELF from `/host`, services the syscalls the C++ binary issues, returns control to the shell on exit or fatal trap.

---

## Key Flows

- F1. **Developer runs a C++ app on the OS**
  - **Trigger:** Developer wants to see a C++ change run on AgenticOS.
  - **Actors:** A1, A2, A3, A4.
  - **Steps:** Developer edits `userland/apps/hello-cpp/...`, runs `./build.sh`. Build system invokes the host C++ toolchain to produce a static ELF, copies it to `host_share/HELLOCPP.ELF`, then builds the kernel and launches QEMU. Developer types `run /HOST/HELLOCPP.ELF`. Kernel loads the ELF, sets up the Linux-shaped initial stack with auxv, allocates the TLS block, jumps to ring 3 via the kernel-continuation path. Binary's iostream output appears on the serial/text path. Binary calls `exit_group(0)`; control returns to the shell.
  - **Outcome:** Observable C++ output and a clean exit, with no manual ELF construction.
  - **Covered by:** R1, R2, R3, R4, R5, R6, R7, R8, R9, R12.

- F2. **Failure: host toolchain missing**
  - **Trigger:** Developer runs `./build.sh` on a machine without a musl C++ cross-compiler installed.
  - **Actors:** A1, A3.
  - **Steps:** Build system probes for the toolchain. The probe fails. Build script emits a clear error naming the missing binary and a one-line install hint, and exits non-zero before kernel build proceeds.
  - **Outcome:** Failure is loud, fast, and actionable; no half-built artifacts.
  - **Covered by:** R1.

- F3. **Failure: binary issues an unimplemented syscall mid-run**
  - **Trigger:** A C++ binary built against a slightly newer musl emits a syscall the kernel does not implement or stub.
  - **Actors:** A4.
  - **Steps:** The dispatcher hits an unregistered syscall number. The kernel logs the number and a short description, terminates the user process via the existing fault-cleanup path, and returns control to the shell.
  - **Outcome:** Missing-syscall failures are diagnostic, not fatal to the kernel.
  - **Covered by:** R14.

---

## Requirements

**Toolchain and monorepo layout**
- R1. The C++ build step probes the host for a musl-based static C++ cross-compiler (e.g., `x86_64-linux-musl-g++`). When present, it builds the C++ app. When absent, `build.sh` and `test.sh` fail with a clear actionable message naming the missing tool and a one-line install pointer; no half-built artifacts.
- R2. C++ app source lives in the monorepo as a sibling to the existing rust userland sample, under `userland/apps/<name-cpp>/`. The `userland/` Cargo workspace is not modified to host C++ — the C++ app uses its own non-Cargo build (Makefile or small shell script) invoked by the top-level build orchestrator.
- R3. `build.sh` and `test.sh` build the C++ app alongside the rust userland app and stage the resulting ELF into `host_share/` under an uppercase 8.3 filename (e.g., `HELLOCPP.ELF`). The existing `HELLO.ELF` continues to be staged.
- R4. The C++ app is statically linked against musl + libstdc++ with `-static -no-pie`. No dynamic linker, no shared libraries.

**Userland ABI — Linux x86-64 compatibility**
- R5. Userland enters the kernel via the x86-64 `syscall` instruction. The kernel programs `MSR_LSTAR`, `MSR_STAR`, `MSR_SFMASK`, and uses `swapgs` to reach the per-CPU kernel stack. The existing `int 0x80` syscall path may be removed or aliased; backward compatibility for the old numbering is not required.
- R6. Syscall numbers and argument-register convention follow the Linux x86-64 ABI: RAX = syscall number on entry and return value on exit; arguments in RDI, RSI, RDX, R10, R8, R9; errors returned as `-errno` in RAX.
- R7. The kernel constructs a Linux-shaped initial user stack on process start: argc, argv (with at least argv[0]), envp (may be empty), auxv with at minimum `AT_RANDOM`, `AT_PAGESZ`, `AT_PHDR`, `AT_PHENT`, `AT_PHNUM`, `AT_NULL`. `AT_RANDOM` points to 16 bytes of random or pseudo-random data the kernel provides; quality is not a requirement for this milestone.

**ELF loader extensions**
- R8. The loader supports `PT_TLS` segments: copies tdata, reserves tbss, allocates a per-process TLS block in the user address space, and arranges for it to be installed via the `arch_prctl(ARCH_SET_FS, ...)` syscall.
- R9. The loader maps multiple `PT_LOAD` segments with their respective leaf permissions (R, RX, RW). Sections needed by libstdc++'s exception unwinder (`.eh_frame`, `.eh_frame_hdr`) are present in the loaded image and discoverable at the addresses recorded in the program headers.
- R10. The loader's per-binary file-size cap is raised to accommodate a static C++ iostream binary (expected order: hundreds of KiB to a few MiB). The FAT/vvfat mount continues to host the file.
- R11. Additional relocation types required by libstdc++ static binaries are supported. Unsupported relocation types continue to fail with a clear, named error rather than corrupting the image.

**Syscall surface**
- R12. The kernel implements the syscalls actually invoked by a static C++ iostream "hello world" through completion. The expected target set, to be refined fault-driven during implementation, includes: `write`, `writev`, `read`, `open`/`openat`, `close`, `fstat`/`newfstatat`, `lseek`, `mmap` (anonymous + file-backed read), `munmap`, `mprotect`, `brk`, `arch_prctl`, `set_tid_address`, `exit_group`, `ioctl`, `readlink`/`readlinkat`, `getrandom`, `rt_sigaction`, `rt_sigprocmask`, `futex`, `getpid`, `getuid`, `getgid`, `prlimit64`. The actual minimum list is whatever this binary observably needs.
- R13. Any syscall in R12 may be implemented as a stub-but-correct return — e.g., `ioctl(TCGETS)` returns `-ENOTTY`; `rt_sigaction` records nothing and returns 0; `futex(..., FUTEX_WAIT, ...)` returns `-EAGAIN`; `getuid`/`getgid` return 0 — provided the binary completes successfully and the stub is documented at the call site. Real implementations defer to later milestones.
- R14. Any syscall the binary invokes that is neither implemented nor stubbed traps cleanly: the kernel logs the syscall number with a short description, terminates the user process via the existing fault-cleanup path, and returns control to the shell. The kernel does not panic, hang, or silently mis-route.

**Existing rust userland**
- R15. The existing rust userland sample (`userland/apps/hello`) continues to load and run after the ABI switch. `userland/runtime/` is updated to issue calls via the `syscall` instruction with Linux numbers (`write` / `exit_group`).

**Developer iteration**
- R16. `userland/README.md` is updated to document the C++ app: the host toolchain prerequisite, how to add a new C++ app, the output filename convention, and the existing read-only `/host` snapshot caveat.
- R17. Freestanding test fixtures (extending `src/tests/userland_fixtures.rs`) exercise the SYSCALL transition and the Linux initial-stack contract independently of the host C++ toolchain. These run under `./test.sh` on any developer machine and gate against regressions even when no developer rebuilds the C++ app locally.

---

## Acceptance Examples

- AE1. **Covers R1, R3.** Given a developer machine with `x86_64-linux-musl-g++` on `PATH`, when `./build.sh` runs, the C++ app builds and `host_share/HELLOCPP.ELF` is present, alongside the existing `host_share/HELLO.ELF`.
- AE2. **Covers R1.** Given a developer machine without a musl C++ cross-compiler, when `./build.sh` runs, the script exits non-zero before the kernel build with a message naming the missing binary and a one-line install hint; no `host_share/HELLOCPP.ELF` is created or left stale.
- AE3. **Covers R5, R6, R7, R8, R9, R12.** Given `host_share/HELLOCPP.ELF` is a static `g++ -static -no-pie` C++ iostream hello-world built per R4, when the user types `run /HOST/HELLOCPP.ELF` in the guest shell, the program's hello-world output appears on the serial/text path, the program exits with status 0 via `exit_group`, and the shell prompt returns.
- AE4. **Covers R14.** Given a binary issues a syscall that has neither an implementation nor a stub, when the kernel dispatches that syscall, the kernel logs the syscall number and terminates the binary via the existing fault-cleanup path. The shell prompt returns; QEMU does not exit; no panic is printed.
- AE5. **Covers R15.** Given the existing rust `HELLO.ELF` is loaded after the ABI switch lands, when the user types `run /HOST/HELLO.ELF`, it prints `hello\n` and exits 0 as before.
- AE6. **Covers R17.** Given `./test.sh` runs on a developer machine without the musl C++ toolchain installed, the SYSCALL-transition and Linux-initial-stack regression fixtures execute and pass.

---

## Success Criteria

- A developer can write a small C++ program under `userland/apps/`, run `./build.sh`, type one shell command in the guest, and see their program's iostream output run on AgenticOS — without writing any custom syscall stubs or linker scripts inside the C++ project.
- Adding a previously-unimplemented Linux syscall to satisfy a new binary is one or two well-bounded code edits in the dispatcher and one new entry in the syscall table — repeatable from PR to PR with no scaffolding rework.
- A regression in the SYSCALL transition or the Linux initial-stack contract surfaces in `./test.sh` on any developer machine, even one without the host C++ toolchain installed.
- The Linux ABI commitment is observably correct: the same `HELLOCPP.ELF` binary, copied off the developer's machine and run under a stock Linux x86_64 host, produces identical output.

---

## Scope Boundaries

- Writable filesystem support (vvfat write-through, FAT write driver, alternative writable disk).
- Multitasking, multiple concurrent user processes, fork/exec, threads, real signal delivery.
- Dynamic linking, shared libraries, `ld-linux` interpreter support.
- `rustc`, cargo, on-OS compilation, large-memory workloads.
- Network syscalls; terminal/tty subsystem beyond what iostream's buffering choice requires.
- Backwards compatibility for the existing `int 0x80` syscall numbering — the transition is one-way.
- A custom GCC target spec; non-musl libc; vendoring the musl/libstdc++ toolchain into the repo.
- Userland heap or memory primitives beyond what musl needs to reach `main` and complete `exit_group`.
- A complete or even broadly representative POSIX/Linux syscall implementation. Only the syscalls this milestone's binary observably needs are in scope.
- Quality `AT_RANDOM` entropy. A predictable byte source is acceptable for this milestone.

---

## Key Decisions

- **Linux x86-64 ABI compat over a custom OS ABI.** Keeps the path open to running unmodified static Linux binaries — including a future `rustc` — without a custom toolchain port. The cost is a larger eventual syscall surface; the milestone scope contains that cost by only implementing what this binary needs.
- **musl + libstdc++ over glibc + libstdc++.** musl is designed to be statically linked; glibc-static binaries are NSS-, locale-, and `dlopen`-fragile and would force broader kernel surface for the same hello-world result.
- **Incremental fault-driven sequencing (Approach B from brainstorm).** SYSCALL switch + Linux initial stack land first, then a real binary drives each missing-syscall PR. Every change produces an observable behavior delta. Avoids the "build a giant runtime in the dark" failure mode.
- **C++ source as a non-Cargo sibling under `userland/apps/`.** Keeps the "userland app" mental model uniform with the existing rust sample without forcing a Cargo workspace to host a C++ project. The build orchestration is `build.sh`'s job, not Cargo's.
- **Stub-but-correct syscalls accepted.** Real `rt_sigaction`, `futex`, `getrandom` quality, etc. are wasted effort for this milestone. Stubs become the seam where later milestones land real behavior.
- **One-way ABI transition.** Keeping both `int 0x80` and `syscall` paths alive doubles the surface for no value — the existing rust app is the only client and gets ported as part of this work (R15).

---

## Dependencies / Assumptions

- The existing FAT driver and vvfat mount can host a binary in the hundreds-of-KiB-to-low-MB range once the loader's file-size cap is raised. Verification is a planning-time check of the loader read path and FAT cluster handling.
- A musl-cross-make-style `x86_64-linux-musl` g++ on a recent toolchain produces an ELF compatible with the loader's existing ELF64 / x86-64 / ET_EXEC constraints once R8, R9, R10, R11 land.
- Current QEMU RAM (128 MiB) is sufficient for a libstdc++ binary's static image and its musl heap. No expansion expected for hello world.
- Stub responses for `rt_sigaction`, `futex`, `getrandom`, `prlimit64` etc. do not cause musl startup or libstdc++ static initializers to abort. If they do, the affected requirement shifts under R12 from stub to real implementation.
- The kernel's existing user-mode entry/exit machinery (`enter_user_mode_asm`, kernel-continuation long-jump, fault-cleanup path) is reusable for the new SYSCALL flow with adjustments to the entry stub; no rewrite of the lifecycle subsystem.

---

## Outstanding Questions

### Resolve Before Planning

- *(none — milestone is well-scoped after dialogue.)*

### Deferred to Planning

- [Affects R10][Technical] What is the loader's effective file-size cap today, and what is required to raise it (buffer sizing, heap pressure, FAT read path)?
- [Affects R12][Needs research] What is the actual minimum syscall set for the chosen musl + libstdc++ hello-world, measured via `strace` of the host-built binary? Use the result to prune the R12 list before implementation, and again to confirm coverage at the end.
- [Affects R8][Technical] Where does the per-process TLS block sit in the user address space, and how is its lifetime tied to the active-user record (`ActiveUser`)?
- [Affects R5][Technical] Does the existing IDT/interrupt entry remain correct once `MSR_LSTAR` etc. are programmed, or does the SYSCALL fast path need additional GDT entries (CS/SS swap, kernel/user descriptor pairs)?
- [Affects R7][Technical] What value source supplies `AT_RANDOM` — kernel RNG, fixed bytes, hashed boot counter? Any choice is acceptable; the decision is implementation-time.
- [Affects R3][Technical] If the developer toolchain is missing, do we additionally support an opt-in flag (`SKIP_CPP=1`) to skip the C++ build rather than fail? Default per R1 is fail; an escape hatch may help CI on machines that genuinely cannot install the toolchain.
- [Affects R15][Technical] Does the rust userland's `runtime` crate switch to inline-asm `syscall` stubs, or to small Rust wrappers around a single `extern "C"` `syscall` shim? Either works; choice driven by what minimizes drift from the rust hello sample.

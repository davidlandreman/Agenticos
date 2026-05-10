---
date: 2026-05-09
topic: kernel mm + fs + arch
applies_to:
  - src/mm/frame_allocator.rs
  - src/mm/paging.rs
  - src/arch/x86_64/interrupts.rs
  - src/arch/x86_64/fpu.rs
  - src/drivers/ide.rs
  - src/fs/fat/fat_filesystem.rs
  - src/fs/file_handle.rs
  - src/userland/lifecycle.rs
  - src/commands/run/mod.rs
  - src/kernel.rs
keywords:
  - frame allocator
  - page fault
  - demand paging
  - heap
  - SSE
  - CR0
  - CR4
  - PIO
  - IDE
  - FAT
  - read_to_vec
  - MaybeUninit
  - set_len
  - GUIShell
  - render_frame
---

# Loading a 5.79 MiB C++ binary in interactive mode

The userland platform's `run /host/HELLOCPP.ELF` started life as "appears to hang for 30+ minutes." It now completes in under a second. The fix was not one bug — it was a chain of seven independent issues that each turned a slow path into a slower one. None of them are obvious in isolation; together they explain why the symptom was indistinguishable from a deadlock.

This document is the post-mortem so the next time someone wants to load a multi-MiB ELF (or anything else that does multi-MiB heap demand-paging + multi-thousand-cluster FAT reads + ring-3 transitions), the trail is here.

## Symptom

`run /host/HELLOCPP.ELF` (5.79 MiB, static `g++ -no-pie` against musl + libstdc++):

- Terminal accepted the command, page-fault log line streamed quickly for ~25 entries, then went silent for many minutes.
- `tasks` (in another terminal) showed `run` still alive.
- No fault, no panic, no watchdog kill, no error message.
- The same code path completed in 640 ms in test mode.

## Root causes (in dependency order)

1. **Frame allocator was O(n²).** `BootInfoFrameAllocator::allocate_frame` rebuilt `usable_frames().nth(self.next)` on every call. After ~1000 allocations, each new alloc walked 1000 entries. Page faults during the read demand-mapped ~1414 heap pages, costing ~1.1 M iterator steps total.
2. **Per-fault logging burned UART vmexits.** The page-fault path emitted ~6 info/debug lines per fault (`>>> PAGE FAULT`, `Page fault in heap region`, `Handling page fault`, two `Usable region`, `Successfully mapped page`). At ~280 bytes/fault through `qemu_print` → `uart_16550`, each byte was a vmexit, dominating wall-clock time.
3. **`File::read_to_vec` zero-filled before reading.** `Vec::with_capacity(N) + Vec::resize(N, 0) + read(...)` touched every backing page TWICE — once to write zeros, once to write the actual bytes. For 5.79 MiB that's ~2828 page faults instead of ~1414.
4. **FAT layer over-allocated per `read()` call.** `<Fat as Filesystem>::read` allocated AND zero-filled a temp buffer the size of the entire file (rounded to cluster), then memcpy'd it into the caller's buffer. For a full-file read, that's another 5.79 MiB allocation + 1414 page faults of zero-fill, plus the actual file read time.
5. **CR0/CR4 SSE bits weren't enabled for ring 3.** The kernel target spec uses `+soft-float` so the kernel never needed SSE itself. `CR0.EM` was 1 and `CR4.OSFXSR` was 0. The first SSE2 instruction in user mode (`movq xmm0, rbx` inside musl's `__init_tls`) trapped as `#UD` (vector 6) before reaching `main`. The Rust hello binary doesn't use SSE so it succeeded; the C++ binary failed silently (the abnormal-exit path was slow enough under interactive load to look like a hang).
6. **GUIShell + compositor competed with the run process for CPU.** With the QEMU window open, every `render_frame()` from the kernel main loop wrote to the framebuffer (~3.7 MiB at 1280×720×32), and host display work added vmexit overhead per pixel. While the kernel main loop's housekeeping ran, the run process was preempted out, slowing the load to crawl. Test mode skips GUIShell entirely (`#[cfg(not(feature = "test"))]`), which is why the same code path completed in 640 ms there.
7. **IDE PIO is not preemption-safe.** `wait_drq` polls the IDE status port for the data-ready bit. When the timer ISR preempts the run process mid-poll and schedules GUIShell, the DRQ window slips and `wait_drq` times out 1000 iterations later — by which time the IDE has DRQ set again, so the final status read after the timeout shows `0x58` (DRDY|DSC|DRQ) and the read fails as `Filesystem error: I/O error`. Test mode has no GUIShell competing, so the timer-tick preemption never lands inside the IDE polling loop, and PIO completes uninterrupted.

## Fixes

| # | File | Fix |
|---|---|---|
| 1 | `src/mm/frame_allocator.rs` | Replaced iterator-rebuild with bump cursor `(region_idx, next_addr, frames_issued)`. O(1) per call. Periodic info-level summary every 256 frames. Pure cursor exposed via `test_support` for unit tests. |
| 2 | `src/mm/paging.rs`, `src/arch/x86_64/interrupts.rs`, `src/mm/frame_allocator.rs` | Demoted per-fault and per-allocation logs from info/debug to trace. Kept `>>> PAGE FAULT at …` at info as the minimum signal a debugger needs. |
| 3 | `src/fs/file_handle.rs::read_to_vec` | Reads directly into uninitialized `Vec` capacity via `Vec::with_capacity` + `core::slice::from_raw_parts_mut` + `set_len`. Special-cases `size == 0` for the dangling-pointer guard. SAFETY comment cites the contract. |
| 4 | `src/fs/fat/fat_filesystem.rs::<Fat as Filesystem>::read` | Hot-path branch: `position == 0 && buffer.len() >= file.size` passes the caller's buffer straight to `read_file`, no intermediate allocation. Fallback path retained for partial reads. |
| 5 | `src/arch/x86_64/fpu.rs` (new) | `enable_sse()` clears `CR0.EM`, sets `CR0.MP` + `CR4.OSFXSR` + `CR4.OSXMMEXCPT_ENABLE`. Called once at the top of `kernel::init()` before any ring-3 transition path is reachable. |
| 6 | `src/userland/lifecycle.rs`, `src/commands/run/mod.rs`, `src/kernel.rs` | Added `BinaryLoadGuard` (RAII) and `binary_load_in_progress()`. `RunProcess::run` constructs the guard for the entire `read → load → exec → exit` window. Kernel main loop reads the flag and pauses GUI work (mouse polling, input event routing, shell polling, `render_frame`) while it's set. Terminal-output buffering still runs so write-syscall bytes accumulate. |
| 7 | `src/drivers/ide.rs::read_sectors`, `write_sectors` | Wrap each PIO transaction in `InterruptGuard::disable()`. Per-call window is bounded (≤ 64 KiB) so the IRQ-disabled section is small. RAII guard ensures restoration on `?`-propagated errors. |

## What does NOT work, and why

- **"Just wait longer"** — not a fix for a deadlock-like symptom and not a fix for IDE PIO state-machine wedges either.
- **`-display none` headless** — bypasses #6 but not #7; also you can't drive the interactive shell from headless.
- **Bumping `wait_drq`'s timeout to 100k iterations** — doesn't address the root cause (preemption); under load you'd just wait longer for the same wedge.
- **Removing `BinaryLoadGuard` and just relying on the IDE IRQ-disable** — the PIO atomicity fix is necessary, but without the guard the run process still loses ~80 % of its CPU to GUIShell + render_frame, stretching the load past the user's patience threshold.
- **Adding the test-mode end-to-end test (`test_run_hellocpp_end_to_end`) without fixing the SS-restore latent bug** — the test triggers a fault path that long-jumps back without restoring the kernel SS selector, which then breaks the `test_gdt_kernel_selectors` sibling test downstream. Cooperative-exit doesn't trip this because the SYSCALL path keeps SS set. The latent bug is in `src/userland/lifecycle.rs::restore_continuation` — out of scope for this PR but worth tracking.

## Diagnostic tests added

- `src/tests/heap.rs::test_heap_burst_throughput` — allocates 6 MiB, touches every page once, reports per-page fault cost. Catches regression in the page-fault hot path.
- `src/tests/memory.rs::test_live_frame_allocator_throughput` — drives the live `BootInfoFrameAllocator` 256 times, asserts O(1).
- `src/tests/memory.rs::test_frame_cursor_*` — synthetic-region tests for the cursor: null-frame skip, region-boundary crossing, non-Usable region skip, exhaustion, monotonic 4096-call ordering.
- `src/tests/filesystem.rs::test_fat_read_throughput_system_ttf` and `test_fat_read_throughput_host_hellocpp` — `[perf]` lines with bytes/sec for FAT throughput.
- `src/tests/filesystem.rs::test_read_to_vec_vs_pre_zero_baseline` — quantifies the savings from #3.
- `src/tests/filesystem.rs::test_run_hellocpp_end_to_end` — full run path under test mode (read → load_elf → ring-3 → exit_group). Asserts cooperative exit with code 0 in under 60 s.

## Open follow-ups

- ~~**SS-restore in `restore_continuation`**~~ — **Resolved 2026-05-09 in U6** (commit `3204502`). Fix inserts `mov ax, 0x10; mov ss, ax` in `restore_continuation`'s naked asm; new regression test `test_kernel_ss_after_user_fault` triggers a #UD then asserts SS == 0x10. Original symptom: the abnormal-exit long-jump path didn't restore the kernel SS selector after a ring-3 fault; SS stayed NULL, breaking any subsequent code that read SS. Cooperative SYSCALL exits preserved SS via STAR, so this only bit the fault path.
- **FAT cluster-walk caching** — `fat_table.rs` re-reads the FAT entry from disk for every cluster in the chain. Acceptable for current sizes; revisit if multi-MiB binaries become common. Already noted in `src/fs/CLAUDE.md`.
- **Switching IDE off PIO** — DMA or virtio-blk would remove both the `wait_drq` polling and the IRQ-disabled atomicity requirement.

## Code patterns to reuse

- **Bump-cursor allocator with periodic summary** — `BootInfoFrameAllocator` in `src/mm/frame_allocator.rs` is a small, testable model for "I have ascending state and a hot path; demote per-call logs to trace and emit a periodic info summary every N calls."
- **`BinaryLoadGuard`** — RAII counter pattern for "while-in-this-region pause GUI." If we ever add other long-running synchronous workloads (e.g., a save-state operation), use the same primitive rather than special-casing.
- **`InterruptGuard::disable()` around hardware PIO** — any hardware that uses register-window protocols (IDE PIO, possibly future PS/2 sequencing if added) needs this. The guard is RAII so `?`-propagated errors don't leak the IRQ-disabled state.
- **Read-into-uninit `Vec`** — `read_to_vec` in `src/fs/file_handle.rs` is the reference impl. Pattern documented in `src/mm/CLAUDE.md`.

## Why the symptom looked like a deadlock

The "fast then stops" pattern is what happens when a kernel-mode loop runs for ~25 page faults' worth (FAT-internal zero-fill + initial cluster reads succeed) and then the IDE state machine wedges (`wait_drq` times out, returns `Err`, the error propagates up). With `BinaryLoadGuard` added, the kernel main loop pauses rendering while the load runs, so the error message stays buffered until the run process exits — making the failure look silent rather than producing the red error text. Once `run` returns and the guard drops, the next render flushes the terminal and the user sees `Filesystem error: I/O error`. Adding `[ide] wait_drq timeout, channel=Primary, status=0x58` (status `0x58` = DRDY|DSC|**DRQ** — the DRQ bit is set the moment after we gave up spinning) was the diagnostic that turned this into a tractable bug.

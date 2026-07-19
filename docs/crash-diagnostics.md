# Crash diagnostics

AgenticOS always compiles a minimal, allocation-free crash capsule core and a
128-record per-CPU flight recorder. Rich modes are selected at launch:

```sh
AGENTICOS_DIAGNOSTICS=record ./build.sh
AGENTICOS_DIAGNOSTICS=strict ./test.sh diagnostics
```

`record` expands each CPU ring to 1,024 records and latches the first shadow
violation without stopping the guest. `strict` escalates that same first
violation into an invariant crash capsule. Ordinary launches remain `minimal`
and do not attach a host debugcon file. The selected and configured modes are
embedded independently in the capsule; a mismatch makes the manifest
untrusted.

Rich launches write `.context/crashes/<run-id>/manifest.json`, `capsule.bin`,
and `kernel.elf.ref`. Decode a completed stream with:

```sh
python3 tools/crash_decode.py .context/crashes/<run-id>/capsule.bin \
  --manifest .context/crashes/<run-id>/manifest.json \
  --elf target/x86_64-unknown-none/release/agenticos
```

The decoder validates header, payload, and per-section CRCs before producing
`report.json` and `report.md`. It treats unknown sections as forward-compatible,
labels duplicate or missing evidence explicitly, and will not trust symbols
unless the build ID and ELF hash match the manifest.
Expected-fatal harness runs also copy the matching ELF into the run directory
as `kernel.elf`, so later builds or workspace cleanup cannot silently change
the symbols used for that artifact. When frames are available, the decoder
uses `llvm-addr2line` or `addr2line` only after the identity checks pass.

## Evidence interpretation

`report.json` separates three categories:

- Capsule facts are under `trigger`, `cpus`, `trace`, `shadow`, `violation`,
  `backtrace`, and `footer`. They passed the capsule and section CRC checks.
- Decoder inferences are only under `inferences`; they are never silently
  promoted into shadow or CPU facts.
- `missing` names required sections absent from this complete capsule. An
  absent section is unavailable evidence, not evidence that its subsystem was
  healthy. Section `stable: false` likewise means a transition was observed,
  not that either the old or new state can be assumed.

`run.manifest_trusted` requires matching run ID, build ID, and diagnostic
personality. `run.symbols_trusted` additionally requires the supplied ELF's
SHA-256 to match the manifest. Never symbolize against a convenient current
build after either flag becomes false. Backtrace reason `5` explicitly means
stack bounds were unavailable to the crash walker in schema v1; the absence
of frames is recorded rather than inferred.

Run expected-fatal smoke cases with:

```sh
scripts/test-crash-diagnostics.sh panic
scripts/test-crash-diagnostics.sh fatal-page-fault
scripts/test-crash-diagnostics.sh missing-cpu
scripts/test-crash-diagnostics.sh sched-duplicate
scripts/test-crash-diagnostics.sh cont-signal-wake
scripts/test-crash-diagnostics.sh cont-invalid-stack
scripts/test-crash-diagnostics.sh pager-short-read
scripts/test-crash-diagnostics.sh io-wrong-wake
scripts/test-crash-diagnostics.sh io-lost-wake
scripts/test-crash-diagnostics.sh io-double-complete
scripts/test-crash-diagnostics.sh io-early-consume
scripts/test-crash-diagnostics.sh as-destroy-active
scripts/test-crash-diagnostics.sh stack-retire-active
scripts/test-crash-diagnostics.sh mm-double-release
scripts/test-crash-diagnostics.sh mm-wrong-unmap
scripts/test-crash-diagnostics.sh mm-wx
scripts/test-crash-diagnostics.sh lock-recursion
scripts/test-crash-diagnostics.sh lock-wrong-owner
scripts/test-crash-diagnostics.sh lock-wrong-context
scripts/test-crash-diagnostics.sh lock-cycle
```

Each case has a 180-second process-group timeout covering build and QEMU; set
`AGENTICOS_CRASH_TIMEOUT_SECONDS` to change the bound. A timeout kills the
entire spawned group and fails without attempting to decode a stale or empty
capsule.

The harness requires a non-success QEMU exit, a complete decodable capsule,
matching run/build identity, and no missing required section. Normal SMP=4
crashes must capture all four CPUs. `missing-cpu` deliberately withholds one
NMI acknowledgement and requires a bounded, valid partial capsule instead of
a hang. `sched-duplicate` requires strict mode to report `SCHED-001` as the
first invariant.
`fatal-page-fault` performs one inline-assembly read from a known unmapped
canonical kernel address and requires vector 14 plus the exact CR2 address;
this qualifies the logger-free fatal exception route itself.
`cont-signal-wake` proves a generic wake cannot make a block-I/O continuation
runnable and requires `CONT-004` as the first invariant.
`cont-invalid-stack` attempts to dispatch a saved continuation outside its
declared live kernel stack and requires `CONT-002` as the first invariant.
`pager-short-read` preserves requested and actual byte counts and requires
`PAGER-004`. The four `io-*` cases distinguish a wrong token (`IO-003`), lost
wake publication (`IO-004`), duplicate completion (`IO-001`), and result
consumption before completion (`IO-002`).
`as-destroy-active` requires `AS-003` before an active user L4 can be freed;
`stack-retire-active` requires `STACK-001` before an active rsp0 stack can be
retired or reused. The three `mm-*` cases require exact `MM-002`, `MM-001`,
and `MM-004` signatures for typed-reference underflow, a mismatched mapping
key, and an executable writable user leaf.
The four `lock-*` cases require exact `LOCK-002`, `LOCK-001`, `LOCK-003`, and
`LOCK-004` signatures for recursive acquisition, owner mismatch, invalid
IF/preemption context, and a dependency cycle.

## Crash-path rules

- Never allocate, format text, touch the filesystem/display, or acquire a
  production lock after crash ownership is elected.
- The first owner writes the capsule; nested entrants only increment the
  secondary marker and halt/exit.
- Fatal exception handlers elect the capsule owner before any contended debug
  logging. Their unmatched boundary entry and trigger section preserve the
  vector, error code, fault address, and instruction pointer.
- The owner captures itself, broadcasts a panic NMI, and waits only for a
  bounded TSC/spin budget. Remote CPUs snapshot on their private panic IST and
  halt without taking production locks.
- Recorder hooks accept integers only. Place them beside the production commit
  they describe, not before it.
- A CPU trace slot is readable only when its release-published sequence remains
  stable across the copy.
- Decoder inferences remain separate from capsule facts. Missing evidence is
  not evidence that a subsystem was healthy.

Scheduler trace entities use bit 63 to distinguish user processes from
kernel threads. `scheduler_dispatch` records the receiving CPU, selection
source (`fair_queue`, `user_queue`, `force_running`, or `resume_same_cpu`),
and whether a latency deadline was missed. `context_publish` records the
resulting production run state plus whether the entity existed and was newly
enqueued. Both carry the committed scheduler-shadow epoch, which is the
causal ordering key across CPUs; TSC values alone are not used to infer that
ordering.

`interrupt_entry` and `interrupt_exit` identify the x86 vector and interrupted
CPL. Exit records additionally say whether EOI was sent and whether the frame
returned, switched to a user or kernel entity, terminated, or recovered a
page fault through COW, page-in, stack growth, or kernel demand mapping. A
fatal path intentionally has no successful exit record.
SYSCALL uses a synthetic boundary ID rather than pretending to be an IDT
vector: entry records carry the Linux syscall number and current PID; paired
exits carry the same number and signed return value. A blocking handoff may
legitimately have no paired exit on that CPU.

`io_token` follows a pager-associated monotonic VirtIO block token through
submit, complete, wake queue, wake acceptance, and consumption. Its causal
epoch is the nonzero page-in generation. Ordinary filesystem requests remain
available in the bounded I/O shadow but do not consume recorder bandwidth;
rejected, lost, and wrong-token pager wakes are explicit phases rather than
inferred from a missing success record.

Lazy file page-in now follows a private-frame commit protocol: allocate and
zero privately, perform an exact-length read, revalidate the L4/VMA, and then
install the present leaf. Signals stay pending while a kernel block-I/O
continuation is suspended; only its exact completed token may wake it.

## Scheduler shadow

The production scheduler enables one fixed-capacity, crash-readable shadow
namespace. Isolated `Scheduler` values used in unit tests do not join that
namespace. Each mutation publishes an odd transition epoch with a pending
operation/subject, then commits an even epoch. The capsule therefore shows
whether a crash interrupted a transition as well as the last committed state
of every observed entity.

Current invariant ownership is `0x01xx_xxxx` (`SCHED-*`). In record mode the
first violation remains latched for later inspection; in strict mode the same
first violation is the crash signature. Never clear or overwrite the latch to
make a later symptom appear first.

Lazy page-in and ring-3 VirtIO are represented as separate correlated
transactions. Pager records include L4/VMA generation, page, private frame,
requested/actual bytes, checksum, and typed terminal reason. I/O records keep
device/queue head, exact token, waiter PID, completion status/length, wake
publication/acceptance, and consumption as distinct facts. The active pager
generation links a block request submitted during population back to its page
transaction; zero means the request was not part of a page-in.

Saved ring-3 kernel continuations have their own generation and retain the
exact request token, owner PID, saved RIP/RSP/RFLAGS, and declared stack
bounds. The shadow distinguishes save publication, exact wake acceptance,
dispatch, and one-time consumption. A completion that races ahead of context
publication is retained as an explicit pending-wake fact; the scheduler and
continuation become runnable only after the save is published.

Each user L4 and per-task kernel stack also has an independent lifetime
generation. Address-space records retain the owning TGID, shared-task count,
VMA generation, active CPU mask, and build/live/destroy state. Stack records
retain the owner PID, exact bounds, active CPU, last installed RSP, and
allocation/live/retirement state. Exit handoffs publish stack inactivity only
after assembly has moved to a different stack.

Rich modes also reserve a full physical-frame ownership ledger and a bounded
user-leaf mapping table beside the allocator metadata before heap startup.
Every usable frame receives a 24-byte generation/state/reference record and,
through 2 GiB, the mapping table receives one 40-byte slot per usable frame:
64 bytes per frame, or 1.5625% of managed RAM. Above that point total shadow
storage stays capped at 32 MiB and the remaining budget determines mapping
capacity; strict mode refuses boot if the complete frame ledger itself cannot
fit. Root, page-table, leaf,
COW, heap, and transient ownership updates are typed at their production
commit points. Address-space publication and destruction independently walk
the raw page tables to reconcile USER ancestry, W^X, frame types, allocator
counts, and exact mapping generations.

The `shadow_memory` capsule section reports provisioned capacity, live mapping
load, maximum probe distance, rejected insertions, transition stability, and
the 256 most recently mutated frame and mapping records. A full mapping table
never evicts live facts; it latches `DIAG-CAPACITY-MAPPING` and preserves
existing entries.

The scheduler, process table, memory mapper, stack allocator, heap allocator,
and serial logger mutexes carry static lock classes in rich modes. Their
wrappers publish owner CPU/entity, acquisition TSC, waiter and failed-try
counts, and observed dependency edges without acquiring a diagnostic lock.
The wrappers verify owner-matched release, reject recursion, check that IF or
preemption is disabled for the mutex kind, and reject an edge that would make
the dependency graph cyclic. Shadow release is published while production
exclusion still holds, followed by production unlock and then restoration of
IF or preemption.
Routine heap and serial acquisitions update the shadow counters but are not
copied into the flight recorder, preventing allocator and logging traffic from
displacing the paging and handoff events being diagnosed.

The `shadow_locks` capsule section always includes one fixed-size record for
each critical class. An owner CPU of `255` means unowned; `order_edges` is a
bitset indexed by static lock class. Counts and edges are historical evidence,
while owner, entity, TSC, and waiters describe the crash-time observation.

The reviewed outer-to-inner partial order is scheduler → process table →
stack/heap → memory mapper → serial logger, with stack and heap intentionally
unordered peers. Scheduler may also enter stack, heap, mapper, or serial
directly; process table may enter stack, heap, mapper, or serial directly;
stack may enter heap, mapper, or serial. Any edge outside this declared DAG,
or any observed path that closes a cycle, is `LOCK-004`. In particular,
mapper → heap is forbidden because heap demand paging already requires
heap → mapper. User mapping buffers are allocated before `MAPPER` is taken.

The `shadow_cpu` section records one fixed 96-byte composite per initialized
CPU. Stable user checkpoints correlate the scheduler's running entity,
`current_user_pid`, live CR3, TSS `rsp0`, the GS SYSCALL stack top, address-
space generation, and kernel-stack generation. Transient `loading_user` and
`loading_kernel` phases retain a completed-step mask and pending outgoing
context entity, so a crash in the middle of a handoff is evidence of where the
handoff stopped rather than an automatic mismatch. An odd epoch or section
flag means the crash interrupted a shadow update and the record is unstable.
ELF preparation uses a separate `address_space_setup` phase while the loader
temporarily installs a not-yet-runnable L4; its generation and restore-to-
kernel-CR3 boundary remain visible if paging fails during image construction.

Release and development kernels retain frame pointers. Fatal serialization
walks at most 32 frames and dereferences only inside either the active user
task's generated kernel-stack bounds or the fixed kernel-thread stack arena.
Backtrace reason 5 means no crash-readable bounds were available, 6 means the
initial frame pointer was outside those bounds, and 7 means it led outside
kernel text. Partial or capped walks keep their frames but set the section's
incomplete flag. The boot/main and panic-IST stack layouts remain explicitly
unavailable until they have equally strict bounds.

CPU handoff invariant IDs are stable artifact signatures:

| ID | Meaning |
|---|---|
| `CPU-001` (`0x02000001`) | stable scheduler entity and current user PID disagree |
| `CPU-002` (`0x02000002`) | stable or declared target CR3 disagrees with its live L4 generation |
| `CPU-003` (`0x02000003`) | kernel-stable state lacks the permanent kernel CR3 or retains a user PID |
| `CPU-004` (`0x02000004`) | pending context publication is missing, duplicated, or names the wrong outgoing entity |
| `CPU-005` (`0x02000005`) | CR3, rsp0, GS stack, extended-state, and PID operations violate declared order |

## Invariant namespace

The high byte owns the diagnostic domain; IDs are stable artifact signatures,
not source line numbers.

| Range | Owner | Current meaning |
|---|---|---|
| `0x01xx_xxxx` | scheduler | duplicate identity/CPU, publication, affinity, and lifecycle transitions |
| `0x02xx_xxxx` | CPU handoff | stable entity/PID, CR3, kernel phase, context publication, and operation order |
| `0x03xx_xxxx` | pager | transaction order, identity, exact population length |
| `0x04xx_xxxx` | block I/O | token/request state, owner, completion and wake causality |
| `0x05xx_xxxx` | continuation | publication, stack/RIP validity, consume, generic-wake rejection |
| `0x06xx_xxxx` | address space | generated L4 identity, activation, ownership, destruction |
| `0x07xx_xxxx` | kernel stack | generated ownership, active bounds, retirement/reuse |
| `0x08xx_xxxx` | memory | mapping identity, typed refs, topology/W^X, frame type, mapper recursion |
| `0x09xx_xxxx` | locks | owner/release, recursion, context, declared dependency DAG |
| `0x0fxx_xxxx` | diagnostics | bounded shadow capacity exhaustion |

The first violation latch is immutable. Do not clear it or renumber an ID to
make a later symptom appear causal. A new invariant gets a deliberate-negative
test that asserts the exact ID and a clean strict workload that reaches the
same subsystem without latching it.

## Resource and perturbation budget

| Component | Minimal | Record/strict |
|---|---:|---:|
| flight recorder at 8 CPUs | 64 KiB (128 × 64-byte slots/CPU) | 512 KiB (1,024 × 64-byte slots/CPU) |
| crash arena | 256 KiB static | 256 KiB static |
| memory ledger | disabled | 24 bytes/usable frame plus 40-byte mapping slots |
| rich memory cap | disabled | min(2% of managed RAM, 32 MiB) |

At the standard 256 MiB test topology, a representative strict boot reports
about 60.6k usable frames and reserves about 3.70 MiB (3.88 MB) for the full
frame ledger plus one mapping slot per frame (roughly 1.56% of managed RAM). Other bounded
shadow tables are static. Routine heap/serial lock traffic is excluded from
the recorder; successful paging hooks perform integer/atomic updates only.

## Adding or changing a shadow domain

1. Assign the domain's stable ID range and fixed-capacity crash-readable
   representation. Shadow state must not drive or repair production behavior.
2. Put integer-only hooks beside production commit points. Do not allocate,
   format, perform port I/O, or acquire a diagnostic/production lock from a
   recorder hook.
3. Serialize a versioned section using bounded reads. Extend the hostile-length
   Python decoder tests and mark the section required only for personalities
   that always emit it.
4. Add legal transition tests, exact-ID negative injections, a clean strict
   workload, and an artifact assertion. Preserve the immutable first latch.
5. Document what is a capsule fact, what the decoder infers, and which missing
   or unstable states remain unavailable evidence.

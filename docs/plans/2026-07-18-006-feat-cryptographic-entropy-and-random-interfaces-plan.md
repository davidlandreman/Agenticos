---
title: "feat: cryptographic entropy and Linux random interfaces"
status: implemented
created: 2026-07-18
plan_type: feat
depth: deep
related_docs:
  - CLAUDE.md
  - src/drivers/CLAUDE.md
  - src/net/CLAUDE.md
  - src/userland/CLAUDE.md
  - src/tests/CLAUDE.md
  - docs/ARCHITECTURE.md
  - docs/IMPLEMENTATION_PLAN.md
  - https://docs.oasis-open.org/virtio/virtio/v1.4/cs01/virtio-v1.4-cs01.html
  - https://www.qemu.org/docs/master/system/qemu-manpage.html
  - https://man7.org/linux/man-pages/man2/getrandom.2.html
  - https://www.intel.com/content/www/us/en/developer/articles/guide/intel-digital-random-number-generator-drng-software-implementation-guide.html
---

# feat: cryptographic entropy and Linux random interfaces

## Summary

Replace every security-relevant deterministic seed with bytes from a trusted
platform random source and expose one kernel-owned random service to all
consumers:

```text
QEMU host /dev/urandom                    physical x86-64 CPU
          |                                      |
    virtio-rng-pci                         CPUID + RDRAND
          |                                      |
          +------------ source selection --------+
                               |
                    kernel random broker
                               |
             +-----------------+--------------------+
             |                 |                    |
         AT_RANDOM          getrandom(2)       /dev/urandom
             |                 |                    |
         musl canary      libc / TLS callers   pathname callers
                               |
                    smoltcp seed + ephemeral-port start
```

Do not build a home-grown entropy pool or another deterministic PRNG in this
slice. The VirtIO entropy device is specified to supply high-quality random
bytes directly, and Intel documents `RDRAND` as the conditioned hardware DRBG
output intended for cryptographic random values. Reading those sources on
demand is simpler to audit than seeding a new software generator, avoids a new
cryptographic dependency, and removes the current timer-seeded xorshift state
entirely.

This is a prerequisite for TLS, not a TLS delivery. The first browser remains
HTTP-only until this plan is implemented and a separate TLS milestone supplies
a reviewed protocol library, certificate roots, hostname verification, and a
trusted wall-clock policy.

## Current-state evidence

- `src/userland/syscalls.rs::getrandom_handler` seeds xorshift64 from
  `time::monotonic_ns()`, caps each call at 4 KiB, ignores all flags, and calls
  the result sufficient for libc despite explicitly saying it is not
  cryptographically secure.
- `src/userland/mod.rs::AT_RANDOM_BYTES` is sixteen fixed `0x42` bytes copied
  into every process's initial stack. Every musl process therefore begins with
  the same stack-canary seed.
- `src/net/stack.rs` derives smoltcp's `Config::random_seed` from the fixed
  QEMU MAC address. TCP sequence generation is consequently reproducible
  across boots.
- `src/net/socket.rs::SocketRegistry` starts ephemeral allocation at port
  49152 every boot and then increments linearly.
- There is no random driver under `src/drivers/virtio/`, no VirtIO entropy PCI
  ID in `src/drivers/pci.rs`, and neither `build.sh` nor `test.sh` attaches a
  random device.
- The syscall layer synthesizes `/bin` and `/proc`, but opening
  `/dev/urandom` falls through to the mounted filesystems and returns
  `ENOENT`. `/dev/tty` exists only as a synthetic `readlink` result.
- `src/tests/userland.rs::test_dispatch_getrandom_fills_buffer` checks only
  that the deterministic output is not all zero. The initial-stack fixture
  checks that `AT_RANDOM` has a non-null pointer, not that its payload changes.
- The committed compiler-compat libc fixture calls `getrandom(32)` but does
  not cover flags, failure, `/dev/urandom`, or distinct output.

## Goals

1. Supply cryptographically strong random bytes in the default QEMU launch
   through a modern VirtIO entropy device backed explicitly by the host's
   `/dev/urandom`.
2. Supply a bounded x86-64 hardware fallback through CPUID-gated `RDRAND` for
   physical machines or VMs without VirtIO RNG.
3. Fail closed when neither trusted source is usable. Never fall back to the
   PIT, RTC, MAC address, keyboard timing, fixed constants, xorshift, or other
   uncredited state.
4. Route process `AT_RANDOM`, Linux `getrandom(2)`, a real readable
   `/dev/urandom` character-device node, smoltcp's seed, and the initial
   ephemeral-port offset through the same service.
5. Preserve bounded allocation, user-copy validation, single-CPU lock safety,
   exact VirtIO completion validation, and finite device timeouts.
6. Prove the plumbing deterministically where possible and use live
   nondeterminism checks only as smoke tests, never as claims of statistical
   or cryptographic certification.
7. Keep all HTTP behavior working and keep documentation honest that entropy
   alone does not enable HTTPS.

## Non-goals

- TLS, HTTPS, BearSSL integration, cipher suites, certificate parsing, a root
  CA bundle, revocation, SNI, hostname verification, or TLS session storage.
- NTP, authenticated time synchronization, or a claim that the boot CMOS RTC
  is sufficient certificate-validation policy.
- A Linux-style environmental-noise pool, entropy accounting, input-event
  crediting, `/proc/sys/kernel/random`, `getentropy(3)` in libc, or a kernel
  CSPRNG/DRBG.
- `/dev/random`, `hwrng`, writable random devices, entropy injection from
  userland, or blocking hot-plug of a source after boot.
- FIPS validation, SP 800-90 certification, statistical randomness suites, or
  a claim that the guest can defend against a malicious hypervisor. A VM
  necessarily trusts the hypervisor that supplies its device and virtual CPU.
- Reseeding a software PRNG because this plan intentionally does not add one.
- ASLR, randomized mmap placement, randomized kernel addresses, UUID APIs, or
  secret persistence across reboot.
- Randomizing socket IDs or other counters whose only purpose is uniqueness.

## Security and compatibility contract

### Source trust

The service reports `Ready` only after one of these source contracts succeeds:

| Priority | Source | Readiness contract | Runtime failure |
|---:|---|---|---|
| 1 | Modern VirtIO entropy device, PCI ID `0x1044` | Version 1 negotiated, queue 0 configured, exact-length boot probe completes | Retain all DMA memory, quarantine the queue, and switch to a probed CPU source if present |
| 2 | x86-64 `RDRAND` | CPUID leaf 1 ECX bit 30 set and bounded carry-flag-checked probe succeeds | Retry each word at most 10 times, then disable the source and fail |
| - | No trusted source | `Unavailable` | Never synthesize bytes |

`RDSEED` is not needed in this design: it exists to seed a software PRNG, and
there is no software PRNG in scope. `RDRAND` is the CPU's conditioned random
output. If a later milestone adds a kernel DRBG, that milestone should revisit
`RDSEED`, source mixing, reseed intervals, backtracking resistance, and a
reviewed no-std cryptographic crate as one coherent design.

### Failure behavior

- The kernel may finish booting without entropy so diagnostics remain
  available, but it must log one clear `entropy unavailable` warning without
  logging any random bytes.
- A fresh process launch cannot construct a fixed or weak `AT_RANDOM`; it
  returns `EnterError::EntropyUnavailable` before the process becomes runnable.
- `execve` obtains its new 16-byte seed before detaching the old image. Failure
  returns `-EIO` and leaves the calling process intact.
- Network initialization requires random interface and port seeds. Failure
  leaves the network stack down rather than restoring MAC-derived TCP state.
- `getrandom(..., GRND_NONBLOCK)` returns `-EAGAIN` when the service is not
  ready. Because AgenticOS has no entropy hot-plug worker in this slice, a
  nominally blocking request with a permanently absent or failed source
  returns `-EIO` rather than parking forever.
- `/dev/urandom` remains a visible character node, but a read returns `-EIO`
  when the source is unavailable.
- A failed or short device request never copies partial/stale bytes to a user
  buffer. Kernel staging is zeroed before DMA and cleared before release on an
  error path.

### Linux-facing behavior

`getrandom` accepts `GRND_NONBLOCK` and `GRND_RANDOM` and rejects every unknown
flag with `-EINVAL`. Both accepted modes use the same nondepleting strong
source; `GRND_RANDOM` is capped at 512 bytes per call, while ordinary reads
retain the kernel's 4 KiB staging bound. A ready request of at most 256 bytes
returns the full requested count, matching the important libc/getentropy call
shape. Larger calls may return a positive short count and callers must loop.

`/dev/urandom` supports `open`/`openat` read-only, `read`, `fstat`, `stat`,
`access`, `dup`/fork inheritance, `FD_CLOEXEC`, directory enumeration under
`/dev`, and `poll` readability. It reports `S_IFCHR | 0444`, uses the Linux
random-device identity (major 1, minor 9) when `st_rdev` can represent it, and
returns `ESPIPE` from `lseek`. Writable opens and mutation attempts are
rejected. `/dev/random` remains absent rather than being a misleading alias.

## Key decisions

### Read trusted bytes directly; do not replace xorshift with a fancier PRNG

A ChaCha-based kernel DRBG could reduce device traffic, but it would introduce
key-state lifetime, reseeding, fork/snapshot behavior, output limits,
backtracking resistance, and dependency-review questions. Current consumers
request tiny amounts of random data, and QEMU's VirtIO RNG default rate is
effectively unbounded for this workload. Direct reads have the smallest trusted
computing base:

```text
source says N bytes completed exactly -> broker returns N bytes
anything else                         -> broker returns an error
```

Do not hash predictable timer/MAC/input values into the output and count that
as entropy. If future diversity is desired, add it only through a separately
reviewed extract-and-expand design.

### Prefer VirtIO RNG in QEMU and retain RDRAND as a platform fallback

The VirtIO specification defines entropy device ID 4, request queue 0, no
device-specific feature bits or configuration, and device-writable request
buffers. The modern PCI ID is `0x1040 + 4 = 0x1044`, matching the repository's
existing modern-device discovery convention.

`build.sh` and `test.sh` attach:

```text
-object rng-random,id=agenticos-rng,filename=/dev/urandom
-device virtio-rng-pci,disable-legacy=on,rng=agenticos-rng
```

The kernel should prefer that explicit device so default tests prove the new
driver rather than silently passing through virtual `RDRAND`. CPU fallback is
still initialized and retained so a later VirtIO failure can fail over without
inventing bytes. A live test asserts that the default test topology selected
`VirtioRng`, making a missing QEMU argument or discovery regression visible.

### Keep random source state behind an interrupt-safe broker

Add `src/random.rs` as the only consumer-facing API:

```rust
pub enum RandomError { Unavailable, Timeout, DeviceFailure, CpuFailure }
pub enum SourceKind { VirtioRng, Rdrand }

pub fn init();
pub fn source_kind() -> Option<SourceKind>;
pub fn fill_bytes(out: &mut [u8]) -> Result<(), RandomError>;
pub fn random_u64() -> Result<u64, RandomError>;
```

The global service uses `InterruptMutex`, not a plain `spin::Mutex`: random
reads occur from preemptible kernel launch/network threads and IF-cleared
syscall paths on one CPU. Never hold the process table, network lock, FD-table
closure, memory-mapper lock, or binary-setup lock while waiting on an entropy
device. Stage bytes first, release the broker, then enter those subsystems.
If a source fails after filling an earlier internal chunk, zero the complete
caller slice and restart the complete request on the retained fallback; if the
fallback also fails, return an error with the caller slice zeroed.

Source-specific code stays below the broker:

- `src/drivers/virtio/rng.rs` owns VirtIO registers, queue 0, and one pinned
  zeroed DMA page.
- `src/arch/x86_64/random.rs` owns CPUID feature decoding, unsafe instruction
  invocation, carry checking, retry bounds, and tail-byte copying.
- Consumers never call either implementation directly.

### Make VirtIO failure DMA-safe

For each request, the driver:

1. clamps the chunk to the pinned DMA page;
2. zeroes the entire submitted range;
3. submits one device-writable descriptor with an owned token;
4. publishes it with the existing release fence and notifies queue 0;
5. polls with a finite spin budget;
6. accepts only the expected descriptor/token and exactly the requested used
   length;
7. copies into the broker's kernel buffer only after full validation.

On timeout, the device may still own the descriptor. Do not drop the queue or
DMA page, reset and reuse the buffer speculatively, or let an allocator reclaim
it. Mark that driver quarantined and retain it for the rest of the boot. The
broker may switch to an already-probed CPU source; otherwise it returns an
error. Malformed completions follow the same quarantine path and never panic.

### Generate `AT_RANDOM` outside the stack-layout primitive

Change `build_initial_stack` to accept `&[u8; 16]` rather than reading a global
constant or hiding a fallible device request inside raw user-stack writes. The
launcher generates all sixteen bytes before calling the layout primitive and
can therefore propagate failure cleanly. In the production launcher, obtain
the bytes before acquiring `BINARY_SETUP_MUTEX`, then thread the explicit
value through `setup_user_process*`; do not poll the entropy device inside the
CR3-sensitive transaction. Tests can pass a known fixture value to test frame
layout without making deterministic test injection part of the production
broker.

Fresh launch and successful `execve` each get a new value. `fork` naturally
inherits the parent's existing user memory and stack canary, matching ordinary
process semantics.

### Add a minimal synthetic devfs at the syscall boundary

Follow the existing virtual `/bin` and `/proc` pattern rather than teaching
FAT/tmpfs/ext2 about character devices in this milestone. Add
`src/userland/devfs.rs` to classify only `/dev` and `/dev/urandom`, plus two
explicit FD variants:

```text
VirtualDevDir { cursor, cloexec }
Urandom { cloexec }
```

Dedicated variants keep dynamic random reads distinct from snapshot-backed
`VirtualFile`. `getdents64_virtual_dev` emits `urandom` as `DT_CHR`, and path
stat/access plus FD stat share one node definition. Normalize before
classification and reject every namespace mutation so `..` aliases cannot
bypass policy.

### Seed all network randomness before taking the network lock

After discovering a NIC but before constructing `NetworkStack`, request one
small kernel buffer and split it into:

- a full random `u64` for `smoltcp::iface::Config::random_seed`;
- a 14-bit offset into the IANA dynamic/private range for
  `SocketRegistry::next_ephemeral` (`49152 + (seed & 0x3fff)`).

Pass the initial port into `SocketRegistry::new`; allocation can remain a
bounded linear scan from that unpredictable start. Do not request random bytes
while holding `NETWORK`, and do not fall back to the NIC MAC.

### Keep the browser HTTP-only after this lands

Cryptographic randomness fixes only one TLS prerequisite. Documentation may
say that AgenticOS now has cryptographic random interfaces suitable for a
future TLS library, but it must continue to describe BusyBox `wget` and the
first browser as HTTP-only. HTTPS remains deferred until a separate plan
defines at least:

- the reviewed TLS implementation and supported protocol/cipher profile;
- system trust-store format, provenance, update mechanism, and storage;
- DNS hostname/SNI and certificate-name validation;
- certificate validity against an explicit trusted-time policy;
- failure UI and a rule that certificate errors never downgrade to HTTP.

## Expected file map

```text
src/
  main.rs                           export the kernel random module
  random.rs                         broker, source selection, failover, errors
  kernel.rs                         initialize entropy before network/userland
  arch/x86_64/
    mod.rs                          export CPU random module
    random.rs                       CPUID-gated RDRAND fallback
    CLAUDE.md                       CPU source/retry safety contract
  drivers/
    pci.rs                          modern entropy PCI ID + finder
    virtio/
      mod.rs                        export rng driver
      rng.rs                        polling VirtIO entropy device
    CLAUDE.md                       queue/DMA/quarantine invariants
  net/
    stack.rs                        strong smoltcp seed
    socket.rs                       randomized ephemeral start
    CLAUDE.md                       entropy-before-NETWORK rule
  userland/
    mod.rs                          explicit AT_RANDOM input + entry error
    syscalls.rs                     getrandom and devfs FD operations
    fdtable.rs                      Urandom + VirtualDevDir variants
    devfs.rs                        /dev namespace classification/stat/listing
    launcher.rs                     propagate launch entropy failure
    CLAUDE.md                       Linux random ABI and fail-closed behavior
  tests/
    mod.rs                          entropy module registration
    entropy.rs                      broker, CPU decoder, VirtIO live tests
    userland.rs                     syscall/devfs tests
    userland_fixtures.rs            inspect AT_RANDOM payload

userland/
  apps/compiler-compat/src/libc.c   getrandom + /dev/urandom ABI smoke
  apps/compiler-compat/README.md    document random coverage
  prebuilt/compiler-compat/CCLIBC.ELF

build.sh, test.sh                   attach explicit host-backed virtio-rng
CLAUDE.md, README.md,
userland/README.md,
docs/ARCHITECTURE.md,
docs/IMPLEMENTATION_PLAN.md         delivered entropy, TLS still deferred
```

No new Cargo dependency is expected. If implementation discovers that direct
source reads cannot meet the required behavior and proposes a software DRBG,
stop and revise this plan rather than slipping a cryptographic primitive into
an implementation unit.

## Implementation units

### U1 — Pin the broker and Linux ABI contracts with focused tests

**Goal:** Establish failure, flag, source, and consumer behavior before
removing deterministic output.

**Files:** new `src/tests/entropy.rs`, `src/tests/mod.rs`,
`src/tests/userland.rs`, `src/tests/userland_fixtures.rs`.

1. Define tests around a local/test source interface for exact fill, zero
   length, source absence, short/failing source, and no partial-output commit.
   Do not install a deterministic provider into the production global.
2. Expand syscall tests for flags 0, `GRND_NONBLOCK`, `GRND_RANDOM`, unknown
   bits, zero length, invalid user ranges, the 256-byte full-return contract,
   and the 4 KiB/512-byte bounds.
3. Extend the initial-stack fixture to dereference and inspect all 16 bytes,
   and add a two-launch test showing successful exec/launches do not reuse the
   previous fixed value.
4. Add devfs contract tests for normalized path classification, read-only
   open, char-device stat, `ESPIPE`, directory `DT_CHR`, dup/cloexec, and
   rejected mutation.
5. Name live output-distinctness checks as smoke tests. A pair of unequal
   128- or 256-bit values catches wiring regressions with negligible false
   failure probability but is not an entropy-quality proof.

**Exit:** tests describe the intended secure failure behavior and fail against
the fixed/xorshift implementation.

### U2 — Implement trusted platform sources and the kernel broker

**Goal:** Produce strong bytes without userland or network dependencies.

**Files:** `src/main.rs`, new `src/random.rs`, new
`src/drivers/virtio/rng.rs`, new `src/arch/x86_64/random.rs`, `src/drivers/virtio/mod.rs`,
`src/arch/x86_64/mod.rs`, `src/drivers/pci.rs`, subsystem documentation.

1. Add modern entropy PCI ID `0x1044` and cached finder alongside block/net.
2. Initialize a VirtIO 1.x entropy device with exactly queue 0, no
   device-specific feature bits, one owned DMA page, DRIVER_OK, and INTx
   disabled because this first driver polls.
3. Implement exact-length, finite, DMA-safe requests and permanent quarantine
   on timeout/malformed completion.
4. Implement CPUID decoding plus `RDRAND` word/tail fill with carry checking
   and ten retries per word. Invoke the intrinsic only after the feature bit is
   known present.
5. Add the interrupt-safe global broker, probe VirtIO first, retain a probed
   CPU fallback, switch once on runtime device failure, and expose only the
   small safe API above.
6. Ensure empty output succeeds without touching hardware, source errors never
   mutate caller-visible output, and logs expose source kind/error only.

**Exit:** a booted kernel can fill repeated buffers through either source;
missing/failing sources yield typed errors and no synthesized output.

### U3 — Attach VirtIO RNG in every QEMU topology and initialize it early

**Goal:** Make strong entropy mandatory and observable in the supported
development/test environment.

**Files:** `build.sh`, `test.sh`, `src/kernel.rs`, new entropy tests.

1. Add the explicit `rng-random` object and modern `virtio-rng-pci` device to
   interactive and test QEMU arguments independently of NIC enablement,
   display/VirGL selection, legacy data disks, and slirp bridging.
2. Validate the host entropy path before launching QEMU and emit an actionable
   error rather than silently omitting the device.
3. Call `random::init()` after heap/memory/scheduler readiness and before
   network initialization or any ring-3 process can be prepared.
4. Log one boot line with `virtio-rng`, `rdrand`, or `unavailable`; never log
   probes or returned bytes.
5. Add a required live test that the default `test.sh` topology selects
   `VirtioRng`, completes exact-size requests, and produces distinct smoke-test
   buffers.

**Exit:** normal and filtered QEMU tests cannot accidentally exercise the CPU
fallback in place of the intended VirtIO driver.

### U4 — Replace fixed process and network seeds

**Goal:** Remove every known security-relevant deterministic consumer.

**Files:** `src/userland/mod.rs`, `src/userland/launcher.rs`,
`src/userland/syscalls.rs` (`execve` transaction), `src/net/stack.rs`,
`src/net/socket.rs`, affected tests.

1. Delete `AT_RANDOM_BYTES`; make `build_initial_stack` take an explicit
   `[u8; 16]` reference and update pure layout tests with fixture bytes.
2. Generate fresh bytes before the launcher takes `BINARY_SETUP_MUTEX`, thread
   them explicitly through `setup_user_process*`, add
   `EnterError::EntropyUnavailable`, and unwind the not-yet-runnable image and
   address space normally on failure. Synchronous test/compatibility entry
   wrappers obtain their seed before entering the same setup path.
3. In `execve`, obtain entropy before detaching the old process transaction;
   map failure to `EIO` with no change to the current image, VMAs, signal
   state, or FD table.
4. Request network seed material before taking `NETWORK`, pass it into
   `NetworkStack` and `SocketRegistry`, and delete MAC/fixed-port derivations.
5. Audit the tree again for fixed/time-derived values used as secrets,
   nonces, ISNs, randomized allocators, or canaries. Do not alter stable IDs or
   test hashes that are intentionally deterministic.

**Exit:** no `0x42` AT_RANDOM constant, timer-seeded xorshift, MAC-derived
smoltcp seed, or fixed ephemeral starting point remains.

### U5 — Deliver `getrandom(2)` and `/dev/urandom`

**Goal:** Give unmodified libc and TLS libraries coherent Linux-shaped random
interfaces backed by the broker.

**Files:** new `src/userland/devfs.rs`, `src/userland/mod.rs`,
`src/userland/fdtable.rs`, `src/userland/syscalls.rs`, userland tests.

1. Rewrite `getrandom_handler` around `random::fill_bytes`: validate flags and
   user range before generation, stage at most the mode's bound, fill fully,
   then copy once to user memory.
2. Map unavailable/transient/permanent errors exactly as defined in the
   failure contract and retain zero-length behavior.
3. Implement the minimal normalized `/dev` namespace and dedicated directory/
   random FD slots.
4. Route `read`, `fstat`, path stat, access, getdents64, lseek, poll,
   dup/fork, close, and fcntl/cloexec through those variants without holding an
   FD/process lock during a hardware request.
5. Reject write opens, writes, truncate, unlink, rename, and directory
   mutations involving the synthetic namespace.
6. Keep `READ_MAX_LEN` and usercopy discipline; a large `/dev/urandom` read may
   short-read at 4 KiB and libc may loop.

**Exit:** static-musl code can use either interface and receives the same
strong source semantics; error paths expose no partial bytes.

### U6 — Extend committed userland compatibility coverage

**Goal:** Prove a real static-musl binary consumes both interfaces.

**Files:** `userland/apps/compiler-compat/src/libc.c`,
`userland/apps/compiler-compat/README.md`, refreshed
`userland/prebuilt/compiler-compat/CCLIBC.ELF`,
`src/tests/compiler_compat.rs` if reporting needs expansion.

1. Retain the existing exact 32-byte `getrandom` call and add checks for a
   second distinct result and invalid flags.
2. Open `/dev/urandom` read-only, verify char-device `fstat`, read at least 32
   bytes, verify `lseek` fails with `ESPIPE`, and close it.
3. Compare the two interfaces only for successful non-fixed output; do not
   require them to return identical streams or run statistical tests.
4. Rebuild through the fixture's documented Makefile/refresh flow and commit
   source plus ELF together so `--skip-userland` remains meaningful.

**Exit:** the booted compiler-compat ladder fails if either random ABI
regresses or the committed fixture is stale/missing.

### U7 — Documentation, security audit, and TLS boundary

**Goal:** Advertise exactly what strong entropy enables and what remains
unsafe/unimplemented.

**Files:** `CLAUDE.md`, subsystem CLAUDE files, `README.md`,
`userland/README.md`, `docs/ARCHITECTURE.md`,
`docs/IMPLEMENTATION_PLAN.md`, BusyBox/browser plan or README when present.

1. Replace statements that `AT_RANDOM`/`getrandom` are fixed or deterministic
   with the source/failure contract.
2. Document `/dev/urandom`, QEMU's host-backed device, RDRAND fallback,
   unavailable behavior, and the no-pool/no-DRBG decision.
3. State the VM trust boundary explicitly: a hostile QEMU controls both the
   entropy device and virtual CPU and is outside the guest threat model.
4. Keep BusyBox `wget` and the first browser labeled HTTP-only. List entropy,
   TLS library, roots, hostname verification, and trusted time as separate
   readiness gates.
5. Record the final audit search and test results. If the implementation
   reveals a durable VirtIO timeout/DMA lesson not already in subsystem docs,
   add it under `docs/solutions/`.

**Exit:** documentation never equates a functioning RNG with complete or
verified HTTPS support.

## Validation matrix

| Layer | Happy path | Failure/edge path |
|---|---|---|
| CPU source | CPUID present, carry succeeds, word + tail fills | feature absent, ten consecutive failures, zero length |
| VirtIO protocol | modern ID, queue 0, exact completion | missing feature, short/oversize/wrong token, timeout quarantine |
| Broker | VirtIO selected, CPU retained, full fill | VirtIO fails over once, both absent, no partial commit |
| Boot/QEMU | host `/dev/urandom` -> virtio-rng -> ready log | host path missing, device omitted, source unavailable warning |
| Initial stack | fresh 16-byte payload per launch/exec | no source aborts launch; exec keeps old transaction |
| `getrandom` | flags 0/NONBLOCK/RANDOM, <=256 full return | unknown flags, bad pointer, unavailable, large short read |
| devfs | open/read/stat/list/poll/dup/close | write/mutation denied, lseek ESPIPE, unavailable read EIO |
| Network | strong smoltcp seed, randomized dynamic-port start | no source leaves network down; no MAC/fixed fallback |
| Static musl | compiler fixture uses syscall and device | stale/missing ELF fails, no unknown syscall fallback |
| Regression | zsh, compiler, DNS, HTTP wget still work | no NIC remains a valid boot; HTTP is not relabeled HTTPS |

Required commands:

```sh
cargo fmt --check
cargo check
./test.sh --skip-userland entropy userland compiler_compat
./test.sh --skip-userland network network_userland
AGENTICOS_TEST_NETWORK=off ./test.sh --skip-userland entropy userland
./test.sh --skip-userland
```

Before completion, refresh the changed compiler-compat ELF through its
documented build path and run an interactive smoke:

```sh
ls -l /dev/urandom
dd if=/dev/urandom bs=32 count=1 | hexdump -C
wget -q -O - http://example.com/
```

The smoke confirms interfaces and HTTP regression only. Do not add a public
HTTPS command to acceptance for this plan.

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| Calling `RDRAND` on an unsupported CPU causes `#UD` | Gate the intrinsic behind CPUID in one architecture module |
| CPU source temporarily has no word | Check carry and retry ten times; then fail closed |
| VirtIO completion is short or malicious | Require expected token and exact length before copying |
| Timed-out DMA is freed while device may still write | Quarantine and retain queue/page for the boot lifetime |
| Random lock deadlocks on a preempted single CPU | Use `InterruptMutex`; never wait while holding process/network/FD locks |
| Device polling stalls the desktop | Bound each request, keep consumer chunks <=4 KiB, and fail over/fail closed |
| Bad user pointer consumes data or sees partial output | Validate first; stage fully; copy once after success |
| Process launch silently restores fixed canary | Make entropy an explicit fallible setup input; no weak fallback |
| `execve` failure destroys the caller | Obtain seed before detaching the old transaction |
| Missing QEMU device is hidden by virtual RDRAND | Required test asserts default source kind is VirtIO RNG |
| `/dev/urandom` is mistaken for a normal file | Dedicated dynamic FD variant and `S_IFCHR`/`DT_CHR` reporting |
| Output comparison is mistaken for crypto proof | Label it smoke only; security derives from documented source contracts |
| Entropy work causes premature HTTPS claims | Keep TLS/library/roots/hostname/time gates explicit and separate |

## Completion criteria

- Default `build.sh` and `test.sh` attach a modern VirtIO entropy device backed
  by the host's `/dev/urandom`, and the guest reports it as the selected source.
- A physical/VM boot without that device can use CPUID-gated, carry-checked
  `RDRAND`; a boot with neither source never emits substitute random bytes.
- `AT_RANDOM` is fresh per launch/exec and no fixed payload remains.
- `getrandom` validates flags and pointers, honors the small-request contract,
  and never runs xorshift or another deterministic fallback.
- `/dev/urandom` is a coherent read-only character device under a listable
  synthetic `/dev` namespace.
- smoltcp's seed and the ephemeral-port starting point come from the broker,
  not the MAC, timer, or a fixed constant.
- VirtIO timeout/malformed-completion paths retain DMA ownership, return an
  error, and never panic or copy partial data.
- The committed static-musl compatibility fixture exercises both public random
  interfaces, and focused plus full QEMU suites pass.
- Documentation says cryptographic randomness is delivered but TLS/HTTPS is
  still not; the first browser remains HTTP-only.

## Implementation verification

Completed on 2026-07-18. The final source audit found no xorshift,
`AT_RANDOM_BYTES`, MAC-derived smoltcp seed, or fixed ephemeral-port initializer
remaining under `src/`. `cargo fmt --check`, normal/test-feature `cargo check`,
shell syntax checks, and `git diff --check` pass. Focused QEMU runs passed for
the entropy module, `/dev/urandom` syscall behavior, the no-NIC boot, and the
refreshed static-musl compiler-compat fixture. The complete hermetic QEMU suite
passed all 852 tests, including DNS and HTTP-only BusyBox/zsh `wget` coverage.

## Follow-ups

1. Plan and integrate a reviewed no-std/userland TLS stack with a narrow TLS
   1.2/1.3 profile and explicit entropy callback.
2. Define CA-bundle provenance, updates, persistence, hostname/SNI validation,
   and a trusted-time policy before enabling HTTPS.
3. Add `/dev/random` only if an application requires it and the kernel gains a
   meaningful readiness/entropy-accounting model.
4. Add a reviewed kernel DRBG only if device latency or future high-volume
   consumers justify the extra cryptographic state; include RDSEED/reseeding
   and VM snapshot/rollback behavior in that design.
5. Add non-x86 entropy backends when AgenticOS gains another architecture.
6. Consider ASLR and randomized mmap/stack placement as a separate exploit-
   mitigation milestone after random bytes are dependable.

## References

- OASIS VirtIO 1.4, entropy device and modern PCI discovery:
  https://docs.oasis-open.org/virtio/virtio/v1.4/cs01/virtio-v1.4-cs01.html
- QEMU random backends and `rng-random` `/dev/urandom` default:
  https://www.qemu.org/docs/master/system/qemu-manpage.html
- Linux `getrandom(2)` flags, small-read, error, and size behavior:
  https://man7.org/linux/man-pages/man2/getrandom.2.html
- Intel DRNG guide, CPUID bits, carry checking, cryptographic use, and the
  ten-retry `RDRAND` recommendation:
  https://www.intel.com/content/www/us/en/developer/articles/guide/intel-digital-random-number-generator-drng-software-implementation-guide.html

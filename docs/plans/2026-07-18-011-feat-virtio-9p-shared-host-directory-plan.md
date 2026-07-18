---
title: "feat: mount a shared host directory at /shared via virtio-9p and a 9P2000.L client"
type: feat
status: completed
date: 2026-07-18
depth: large
related_docs:
  - src/drivers/CLAUDE.md
  - src/fs/CLAUDE.md
  - src/userland/CLAUDE.md
  - docs/ai-context-conventions.md
  - docs/macos-virgl-qualification.md
---

# feat: mount a shared host directory at /shared via virtio-9p and a 9P2000.L client

## Summary

Give every AgenticOS instance — regardless of which Conductor worktree launched
it — a read-write `/shared` mount backed by one fixed host directory
(`~/.agenticos/shared` by default), using QEMU's `virtio-9p-pci` device and a
new in-kernel 9P2000.L client. Multiple simultaneously running instances can
read and write `/shared` concurrently because the macOS kernel owns the real
filesystem; the guests are RPC clients. Force-killing QEMU can never leave the
share dirty or read-only: APFS is journaled and there is no guest-side image to
fsck. This supersedes any plan to share a second ext2 block image between
instances, which is unfixably corruption-prone with more than one writer.

Both QEMU binaries in use ship the device: stock Homebrew QEMU 11.0.1 and the
pinned VirGL bottle (`/opt/homebrew/Cellar/qemu/1.0.27/bin/qemu-system-x86_64`,
QEMU 10.2.50) both list `virtio-9p-pci` in `-device help`. No launch
configuration is excluded.

---

## Current state and motivation

Research findings this plan is built on (verified against the tree at head):

**VirtIO infrastructure is ready to reuse verbatim.**
- PCI scan and cache: `enumerate_devices_cached()` at `src/drivers/pci.rs:262`;
  virtio device IDs at `src/drivers/pci.rs:274-282` (modern IDs only, no legacy
  path). virtio-9p is modern ID `0x1049` (0x1040 + type 9), not yet present.
- `VirtioDevice` (`src/drivers/virtio/common.rs:578`) parses the four modern
  PCI capabilities; `begin_init` (`common.rs:876`) / `finish_init`
  (`common.rs:941`) drive feature negotiation; `setup_queue`
  (`common.rs:948`) is one-call virtqueue bring-up.
- `Virtqueue::add_chain` (`common.rs:369`) builds mixed device-readable /
  device-writable descriptor chains — exactly the T-message/R-message pair a 9P
  RPC needs. `wait_used` (`common.rs:549`) is the bounded polling completion
  primitive; `VirtqBuffer::try_from_slice_segments` (`common.rs:96`) page-splits
  heap buffers into physical segments.
- `src/drivers/virtio/rng.rs` (95 lines) is the polling-driver lifecycle
  template, including `disable_intx()` after init and quarantine-on-timeout.
  `read_mac_stable` at `src/drivers/virtio/net.rs:123` is the config-generation
  stable-read loop to copy for the 9P `mount_tag`.
- Boot bring-up region: `src/kernel.rs:92-99` calls
  `drivers::virtio::block::init()`, `init_filesystems()`,
  `try_mount_host_disk()`, `try_mount_data_disk()`; a new 9p init and
  `try_mount_shared()` slot in here.

**The VFS accepts a new backend at `/shared` with zero overlay changes.**
- One trait: `Filesystem` at `src/fs/filesystem.rs:154` (`&'static dyn` usage
  throughout). Mount resolution is pure longest-prefix
  (`find_filesystem`, `src/fs/vfs.rs:112`), so `/shared` wins over the `/`
  overlay automatically, exactly like `/data` and `/host`.
- Registration pattern: static slot array + `get_vfs().mount(path, fs,
  &NULL_BLOCK_DEVICE)` (`src/fs/vfs.rs:55`, slots at `vfs.rs:12-27`).
- FAT (`src/fs/fat/fat_filesystem.rs:81`) is the precedent for a partial
  backend (`ReadOnly` vs `UnsupportedOperation` split); ext2
  (`src/fs/ext2/filesystem.rs:1482`) is the full-RW reference including
  `unix_metadata` (`:1529`), `set_times` (`:1693`), `rename` (`:1800`),
  symlink/link/read_link (`:1895`, `:1869`, `:1924`).
- There is **no page/dentry/inode cache** between syscalls and backends — every
  `read`/`write` re-issues the backend op (`src/userland/syscalls.rs:662`,
  `src/fs/file_handle.rs:125`). A cacheless 9P client therefore gets
  cross-instance coherence for free; nothing stale to invalidate.
- Backends may block: the descriptor lock is dropped before backend calls
  (`src/fs/file_handle.rs:143-147`), and virtio-blk already parks ring-3
  callers on I/O (`src/drivers/virtio/block.rs:300-390`,
  `src/userland/switch.rs:345`). A polling first cut needs none of that but the
  door is open for an interrupt-driven follow-up.
- Syscall surface that must be served: open/openat (AT_FDCWD only,
  `syscalls.rs:2636`), read/readv, write/writev (staged in ≤4 KiB chunks),
  lseek, getdents64 (→ `enumerate_dir`), stat family (→ `unix_metadata` /
  `symlink_metadata` / `handle_metadata` → `fill_unix_stat`,
  `syscalls.rs:3658`), unlinkat, renameat (same-mount only; cross-mount is
  EXDEV by design, `vfs.rs:531`), mkdirat, ftruncate, fsync/fdatasync,
  utimensat, readlinkat, symlinkat, linkat. `statx`/`statfs`/`chown` are not
  dispatched at all; chmod/fchmod are validated no-ops (`syscalls.rs:3999`).

**Launch wiring has established patterns for everything needed.**
- Device-support probe precedent: `"$QEMU_BIN" -device help | grep -q ...`
  (`scripts/qemu-compositor.sh:44-59`, `build.sh:212`).
- Env-gated optional device precedent: the `AGENTICOS_LEGACY_DATA_IMAGE` block
  (`build.sh:202-208`, `test.sh:178-184`).
- QEMU arg assembly: `build.sh:239-260`; test.sh device region
  `test.sh:158-233`.
- Verified on this machine: stock QEMU 11.0.1 and the pinned VirGL bottle
  (10.2.50) both report `virtio-9p-pci`, so the probe is a safety net, not a
  fork in behavior.

**Motivation.** Conductor runs many worktrees, each with its own `/data` image
under `target/`. There is no way today for two instances to exchange files, and
no worktree-independent persistent location. Sharing an ext2 image via a second
virtio-blk device cannot support concurrent writers (independent guest block
caches, no cluster coherence) and inherits ext2's no-journal fragility on
force-stop. A 9P share dissolves both problems structurally.

## Goals

1. `/shared` mounted read-write in every instance, backed by one fixed host
   directory chosen independently of the worktree (`AGENTICOS_SHARED_DIR`,
   default `${HOME}/.agenticos/shared`).
2. Safe concurrent use by multiple simultaneously running instances
   (host-kernel coherence; guest does no data caching).
3. Force-stop safety: killing QEMU at any moment loses at most in-flight
   requests; the share is never dirty, never demoted to read-only, never needs
   fsck.
4. Full everyday POSIX surface through zsh/BusyBox/TinyCC on `/shared`:
   create, read, write, append, truncate, mkdir, rm/rmdir, mv (within the
   mount), ls -l with real sizes/times, cat of host-created files, symlinks,
   fsync, utimensat.
5. Works under every launch configuration, including VirGL GPU boots and
   `./test.sh`, with a graceful skip (boot continues, no `/shared`) when the
   device is absent.
6. In-kernel test module (`./test.sh p9`) covering the client end-to-end
   against a per-run temp host directory.

## Non-goals

- **No guest-side caching.** Every op is an RPC. Latency tuning (dentry/attr
  caches, larger syscall staging buffers) is deliberately excluded to keep
  cross-instance coherence trivial; revisit only with measurements.
- **No byte-range locking** (`Tlock`/`Tgetlock`): concurrent writers to the
  same file are last-write-wins, as on any NFS-ish scratch share.
- **No interrupt-driven completion** in the first cut — polling like rng/net;
  the virtio-blk INTx pattern is the documented follow-up if spin time shows up
  in real use.
- **No xattrs, no chown/chmod semantics** beyond the kernel's existing
  validated no-ops.
- **No mount-tag-driven dynamic mounts** — one device, one tag
  (`agenticos-shared`), one fixed mount point `/shared`.
- **No changes to `/data`, the overlay, or persistence sync** — `/shared` sits
  beside them.

---

## Design

### Topology and host-side wiring (U5)

```text
macOS host                              guest kernel
~/.agenticos/shared  ◄── APFS ops ──  QEMU fsdev "local"
        ▲                                   │ virtio-9p-pci (0x1049)
        │                                   ▼ mount_tag = "agenticos-shared"
  other instances                    src/drivers/virtio/p9.rs   (transport)
  (same fsdev path,                  src/fs/p9/                 (9P2000.L client)
   own QEMU processes)               VFS mount at /shared
```

build.sh gains, next to the existing network/legacy-data blocks:

```text
AGENTICOS_SHARED_DIR   host directory (default ${HOME}/.agenticos/shared, mkdir -p)
AGENTICOS_SHARED=off   disable the device entirely
```

When enabled and `"$QEMU_BIN" -device help | grep -q virtio-9p-pci` succeeds:

```text
-fsdev local,id=agenticos-shared-fsdev,path=$AGENTICOS_SHARED_DIR,security_model=none
-device virtio-9p-pci,disable-legacy=on,fsdev=agenticos-shared-fsdev,mount_tag=agenticos-shared
```

`security_model=none` passes host permissions through and silently ignores
ownership changes the QEMU process cannot perform. That matches the guest,
whose chmod/fchmod are already validated no-ops, and keeps host-side files
completely ordinary (no mapped xattrs). Multiple QEMU processes pointing at the
same `path` are safe: each fsdev is a plain userspace client of APFS.

test.sh mirrors the block with `AGENTICOS_TEST_SHARED_DIR` defaulting to a
`mktemp -d` per-run directory (removed on exit via the existing trap pattern),
pre-seeded with a fixture file so the in-kernel tests can verify visibility of
host-created content.

### Transport: virtio-9p-pci driver (U1)

New `src/drivers/virtio/p9.rs`, structured on `rng.rs`:

- `src/drivers/pci.rs`: add `VIRTIO_DEVICE_9P: u16 = 0x1049` and
  `find_virtio_9p_devices()` modeled on `find_virtio_block_devices()`
  (`pci.rs:313`).
- Features: `begin_init(VIRTIO_F_VERSION_1 | VIRTIO_9P_F_MOUNT_TAG, same)` —
  the tag is required, it is the device's identity the way `agenticos-data` is
  virtio-blk's serial.
- Config space: `{ u16 tag_len; u8 tag[] }`, read with the
  config-generation-stable loop copied from `read_mac_stable`
  (`net.rs:123`). Only a device whose tag is `agenticos-shared` is adopted.
- One virtqueue (queue 0) via `setup_queue`, then `finish_init()` and
  `pci.disable_intx()`.
- RPC primitive: `P9Transport::rpc(request: &[u8], response: &mut [u8]) ->
  Result<usize, P9TransportError>` — one `add_chain` with the request pages
  device-readable and the response pages device-writable, `notify()`, then
  `wait_used(head, budget)` with a `core::hint::spin_loop` body. Timeout or a
  malformed completion quarantines the channel (poisoned flag), mirroring
  rng's failure discipline; subsequent VFS ops return `IoError` rather than
  wedging the kernel.
- Buffers: two heap `Vec<u8>` of `MSIZE` bytes (see below) translated per-call
  with `try_from_slice_segments` / `try_from_mut_slice_segments`; no persistent
  DMA pages beyond the rings.
- Locking: the transport (buffers + virtqueue + fid table above it) lives
  behind one plain `Mutex` — requests serialize. VFS callers never hold other
  spin locks across backend calls (`file_handle.rs:143-147`), and the driver is
  polling with INTx disabled, so the virtio-blk `InterruptMutex` discipline is
  not needed.
- Boot: `drivers::virtio::p9::init()` called from `kernel_main` right after
  `block::init()` (`kernel.rs:92-99`). Absent device → `init` logs and returns;
  nothing else changes.

### Protocol: 9P2000.L client (U2)

New `src/fs/p9/` module (`protocol.rs` codec + `client.rs` fid/ops), kernel
`no_std` + `alloc` only:

- Version handshake at mount time: `Tversion(msize = MSIZE, "9P2000.L")`,
  honor the (possibly lower) `msize` in `Rversion`; then
  `Tattach(fid = 0, afid = NOFID, uname = "root", aname = "", n_uname = 0)`.
  `MSIZE = 64 KiB` — large enough that `Treaddir` pages rarely, small enough
  that a request+response chain stays ≤ 32 descriptors, comfortably inside the
  device's queue depth. Syscall-layer reads/writes are staged in ≤ 4 KiB chunks
  anyway (`write_handler`, `syscalls.rs:198`), so `msize` mostly matters for
  readdir and future batching.
- Message set (T/R pairs): `version`, `attach`, `walk` (≤ 16 names per hop,
  chunked), `clunk`, `lopen`, `lcreate`, `read`, `write`, `readdir`,
  `getattr`, `setattr` (size for truncate, atime/mtime for utimensat),
  `mkdir`, `unlinkat` (with `AT_REMOVEDIR` for rmdir), `renameat`, `symlink`,
  `readlink`, `link`, `fsync`, `statfs`, and `Rlerror` decoding.
- `Rlerror` carries a Linux errno; map ENOENT→`NotFound`, EEXIST→
  `AlreadyExists`, ENOTDIR/EISDIR/ENOTEMPTY→their variants, EROFS→`ReadOnly`,
  everything else→`IoError` (`FilesystemError`, `src/fs/filesystem.rs:6`).
- Fid management: fid 0 is the attach root, held forever; other fids come from
  a free-list allocator inside the client mutex. Every path op is
  walk-from-root → use → clunk. `File::open` keeps its fid for the handle's
  lifetime (stored in `FileHandle::inode`, released in `close`); `stat`,
  `enumerate_dir`, and directory mutations use transient fids. Tag space: a
  single fixed tag (requests serialize under the client mutex); `NOTAG` for
  version.

### VFS backend: read surface first (U3)

`P9Filesystem` in `src/fs/p9/filesystem.rs` implementing `Filesystem`
(`src/fs/filesystem.rs:154`), registered like ext2's static slots: a
`MOUNTED_P9: [Option<P9Filesystem>; 1]`-style slot in `src/fs/vfs.rs` plus a
`mount_p9(transport, "/shared")` helper using `NULL_BLOCK_DEVICE`
(`vfs.rs:55`). New `try_mount_shared()` in `src/kernel.rs` after
`try_mount_data_disk()` (`kernel.rs:97-99`): adopt the transport if U1 found
the tagged device, run version/attach, mount, log one line either way.

Read surface in this unit:

- `stat` / `unix_metadata` / `symlink_metadata` / `handle_metadata` from
  `Tgetattr` — real mode/uid/gid/size/nlink/times flow into `fill_unix_stat`
  (`syscalls.rs:3658`), so `ls -l` on `/shared` shows host truth (unlike FAT's
  synthesized defaults).
- `enumerate_dir` from `Treaddir` looping until EOF (transient fid, opened
  `O_RDONLY | O_DIRECTORY`), converting `dirent`s to `DirectoryEntry`
  (`filesystem.rs:72`); `read_dir` returns `UnsupportedOperation` exactly as
  ext2 does (`ext2/filesystem.rs:1502`).
- `open` (read modes) / `read` / `seek` / `close`; `seek` permits positions
  past EOF (host handles sparse semantics). `read` issues `Tread` at
  `handle.position`.
- `stats()` from `Tstatfs`; `read_link` from `Treadlink`.
- `is_read_only()` returns `false` from the start; write ops return
  `UnsupportedOperation` until U4 lands (the FAT partial-backend pattern,
  `fat_filesystem.rs:439-454`).

### VFS backend: write surface (U4)

- `open` with `create`/`truncate`/`append`: walk to parent, `Tlcreate` when the
  leaf is absent, `Tsetattr(size=0)` for truncate-on-open, position = size for
  append (size from `Tgetattr` at open).
- `write` → `Twrite` at `handle.position`, updating handle size/position;
  `truncate` → `Tsetattr(size)`; `set_times` → `Tsetattr(atime/mtime)`
  (serving `utimensat`, `syscalls.rs:4197`).
- `mkdir` → `Tmkdir`; `unlink` → `Tunlinkat(0)`; `rmdir` →
  `Tunlinkat(AT_REMOVEDIR)`; `rename` → `Trenameat` (same-mount only — the VFS
  already rejects cross-mount as EXDEV, `vfs.rs:531`, and BusyBox `mv` falls
  back to copy+delete).
- `symlink` → `Tsymlink`; `link` → `Tlink`.
- `sync_handle` → `Tfsync` on the open fid; `sync()` is a no-op success (every
  write already reached the host kernel; there is no guest dirty state).

### Failure and force-stop semantics (U1/U3)

- Guest killed mid-write: the host either processed the `Twrite` or it never
  arrived — APFS metadata is journaled either way. Nothing to repair, no
  read-only demotion. This is the property that motivated Option B.
- QEMU without the device / probe failure / `AGENTICOS_SHARED=off`: `p9::init`
  finds nothing, `try_mount_shared` logs `shared: no virtio-9p device;
  /shared not mounted`, boot proceeds identically to today.
- Transport timeout/desync: channel quarantines; all subsequent `/shared` ops
  fail with `IoError` (EIO in userland). No retry loops in interrupt-sensitive
  paths, no panic (`.claude/rules/no-std.md`).

### Tests (U6)

New `src/tests/p9.rs` module (registered per `src/tests/CLAUDE.md`), running
against test.sh's per-run temp share:

- fixture visibility: read and verify the host-pre-seeded file's exact bytes;
- create → write → fsync → read-back → `unix_metadata` size/mtime sanity;
- multi-chunk file (≥ 256 KiB, exercising many Tread/Twrite RPCs and
  seek-past-4KiB positions);
- `enumerate_dir` sees created entries; mkdir/rmdir; unlink; rename within a
  directory and across directories in the mount;
- symlink + read_link round trip;
- error paths: `NotFound` on missing path, `AlreadyExists` on duplicate
  create-exclusive, `NotEmpty` on rmdir of a populated directory.

Cross-instance coherence is a documented manual smoke (two `./build.sh`
instances from different worktrees against the default share; `touch` in one,
`ls` + `cat` in the other) — automating dual-QEMU orchestration is not worth
the harness complexity today.

---

## Implementation units

### U1. Transport: PCI ID, device bring-up, polled RPC channel

`src/drivers/pci.rs` (`VIRTIO_DEVICE_9P`, `find_virtio_9p_devices`),
`src/drivers/virtio/p9.rs` (init on the rng lifecycle template, mount-tag
config read on the `read_mac_stable` pattern, `P9Transport::rpc` via
`add_chain`/`notify`/`wait_used`, quarantine-on-timeout), `pub mod p9;` in
`src/drivers/virtio/mod.rs`, init call in `src/kernel.rs`.

Verification:
- `cargo check`, `cargo fmt --check`, `cargo clippy`.
- Manual QEMU: boot with the device attached (hand-added `-fsdev`/`-device`
  args until U5); serial log shows the tag `agenticos-shared` discovered and
  the queue up. Boot with `AGENTICOS_SHARED=off`-equivalent (no device) shows
  the graceful-skip line.

### U2. 9P2000.L codec and client ops

`src/fs/p9/protocol.rs` (message ser/de, dirent and attr wire structs,
`Rlerror`→`FilesystemError` mapping), `src/fs/p9/client.rs` (version/attach,
fid free list, walk chunking at 16 names, the full op set from the Design
list), unit-style tests where codec round-trips can run in the kernel test
harness.

Verification:
- `cargo check`; codec round-trip cases included in the U6 test module skeleton
  (encode → decode equality, truncated-buffer rejection).
- Manual QEMU: temporary debug hook performs version/attach/getattr(".") at
  boot and logs the Rgetattr fields for the share root.

### U3. Read-only `/shared`: backend + mount

`src/fs/p9/filesystem.rs` (read surface per Design), `src/fs/p9/mod.rs`,
static slot + `mount_p9` in `src/fs/vfs.rs`, `try_mount_shared()` in
`src/kernel.rs`.

Verification:
- `cargo check`; `./test.sh p9` (read-side subset against the pre-seeded
  fixture).
- Manual QEMU (zsh): `ls -l /shared`, `cat /shared/<host-created file>`,
  `stat` output shows real host size/mtime; `df`-style `stats()` returns
  without error.

### U4. Write surface

Complete `P9Filesystem`: lcreate/write/truncate/append, mkdir/unlink/rmdir,
renameat, symlink/link, set_times, fsync.

Verification:
- `./test.sh p9` full module green.
- Manual QEMU (zsh): `echo hi > /shared/x && cat /shared/x`; `mkdir -p
  /shared/a/b`; `mv /shared/x /shared/a/`; `rm -r /shared/a`; append with
  `>>`; `cd /work && tcc -o /shared/hello /host/sysroot/examples/hello.c &&
  /shared/hello` (cross-mount compile output exercises create+write+execve
  read-back).
- Host side: the files appear in `~/.agenticos/shared` with ordinary
  ownership; editing one on the host is immediately visible to a subsequent
  guest `cat` (no guest cache).

### U5. Launch wiring: build.sh, test.sh, probe

`AGENTICOS_SHARED_DIR` / `AGENTICOS_SHARED=off` block in `build.sh`
(mkdir -p, `-device help` probe with a skip warning, `-fsdev`+`-device` args
next to the legacy-data block); `AGENTICOS_TEST_SHARED_DIR` (default
`mktemp -d`, trap-cleaned, fixture pre-seed) in `test.sh`.

Verification:
- `./build.sh -n` assembles args correctly (echoed command line);
  `./build.sh` with stock QEMU and with the VirGL bottle
  (`AGENTICOS_QEMU_BIN=/opt/homebrew/Cellar/qemu/1.0.27/bin/qemu-system-x86_64`
  + compositor path) both boot with `/shared` mounted.
- `AGENTICOS_SHARED=off` and a probe-failure simulation
  (`AGENTICOS_*_OVERRIDE`-style escape hatch or a PATH shim) both boot without
  the device and without `/shared`.
- Manual two-instance smoke: two worktrees, one default share; `touch` in one
  instance is visible to `ls` in the other within one command.

### U6. Test module and docs

`src/tests/p9.rs` (cases per Design; registered in the test index),
documentation refresh: root `CLAUDE.md` (Current State paragraph + namespace
list gains `/shared`), `src/drivers/CLAUDE.md` (new driver entry),
`src/fs/CLAUDE.md` (new backend + mount), this plan's implementation notes.

Verification:
- `./test.sh p9` green in a clean clone (prebuilt userland path, no musl
  toolchain).
- `./test.sh` full suite unaffected when the share device is present and when
  `AGENTICOS_TEST_SHARED_DIR` handling is exercised.
- Docs mention the manual two-instance coherence smoke and the
  `security_model=none` rationale.

---

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| Polled `wait_used` spins too long on slow host ops (huge dirs, cold disk), stalling one CPU | Generous spin budget with `spin_loop` hint (rng precedent); ops are host-syscall-fast in the `local` fsdev; documented follow-up is the virtio-blk INTx + `Waiter` blocking pattern (`block.rs:300-390`) if real workloads show spin time |
| `Rversion` negotiates msize below 64 KiB or queue depth is smaller than expected | Honor the server's msize; cap descriptor usage at `min(msize pages, queue_size/2)`; readdir already loops, reads/writes already chunk |
| Transport desync (short reply, tag mismatch, timeout) corrupts client state | Single-tag serialized RPCs make desync detectable; quarantine the channel on first violation and fail `/shared` ops with `IoError` — never retry from a partially parsed buffer |
| `security_model=none` cannot represent guest chown/chmod | Guest chmod/fchmod are already validated no-ops (`syscalls.rs:3999`); document that `/shared` ownership is the host user's, which is the desired behavior for a personal share |
| Two instances write the same file concurrently | Host-kernel coherence prevents filesystem damage; content is last-write-wins as documented (locking is a non-goal; `Tlock` is the extension point) |
| Symlinks inside the share point outside it (host escape) | Accepted for a personal dev share; QEMU resolves server-side with the QEMU process's own privileges — same exposure as the existing vvfat `/host` share |
| A future QEMU build lacks `virtio-9p-pci` | `-device help` probe with warning + clean skip; kernel already boots identically without the device |
| Fid leaks under error paths wedge the server's fid table | Client-side RAII-ish fid guard (clunk on drop path in every op); QEMU's fid table is per-connection and resets on guest restart anyway |

## Expected file changes

Add:
- `src/drivers/virtio/p9.rs`
- `src/fs/p9/mod.rs`, `src/fs/p9/protocol.rs`, `src/fs/p9/client.rs`,
  `src/fs/p9/filesystem.rs`
- `src/tests/p9.rs`
- `docs/plans/2026-07-18-011-feat-virtio-9p-shared-host-directory-plan.md`
  (this file)

Modify:
- `src/drivers/pci.rs` (device ID + finder)
- `src/drivers/virtio/mod.rs` (module registration)
- `src/fs/vfs.rs` (static slot + `mount_p9`)
- `src/fs/mod.rs` (module registration)
- `src/kernel.rs` (`p9::init()` + `try_mount_shared()`)
- `src/tests/mod.rs` / test index (register `p9`)
- `build.sh`, `test.sh` (share device blocks)
- `CLAUDE.md`, `src/drivers/CLAUDE.md`, `src/fs/CLAUDE.md`

No changes expected in:
- `src/fs/overlay/`, `src/fs/ext2/`, `src/fs/fat/`, persistence sync,
  `src/userland/syscalls.rs` (the existing dispatch already routes every
  needed syscall through the `Filesystem` trait), scheduler/interrupt code.

## Done criteria

- Booting via `./build.sh` from any worktree mounts the same
  `~/.agenticos/shared` at `/shared`, read-write, on both stock QEMU and the
  VirGL bottle.
- Two concurrently running instances see each other's `/shared` writes with
  plain `ls`/`cat`, and `kill -9` of either QEMU leaves the share fully
  usable with no recovery step.
- The U4 zsh command matrix (redirect, append, mkdir -p, mv, rm -r, tcc
  compile to `/shared` and execute) passes; `ls -l` shows real host
  sizes/mtimes.
- `./test.sh p9` passes in a clean clone; the full `./test.sh` suite stays
  green.
- With the device absent or `AGENTICOS_SHARED=off`, boot output and behavior
  are unchanged except one skip log line.

---

## Implementation notes (2026-07-18, all units landed)

What actually happened, for the follow-up to inherit:

- **The plan's shape held.** U1–U6 landed as specified: `src/drivers/virtio/p9.rs`
  (transport), `src/fs/p9/{protocol,client,filesystem}.rs` (codec, client,
  backend), `mount_p9` + `MOUNTED_P9` slot in `src/fs/vfs.rs`,
  `try_mount_shared()` in `src/kernel.rs`, share blocks in `build.sh`/`test.sh`,
  and `src/tests/p9.rs` (11 tests). First full QEMU run of `./test.sh p9`
  passed all 11 tests with zero protocol-level debugging — the codec unit
  tests and the strict envelope validation in `P9Client::rpc` did their job.
- **U3 and U4 merged.** Splitting the read and write surfaces into separate
  landings bought nothing once the client existed; the backend shipped whole.
- **Heap-slice DMA was already precedented** — the GPU control queue uses
  `try_from_slice_segments`/`try_from_mut_slice_segments` on heap buffers, so
  the transport needed no owned `DmaPage` pool beyond the virtqueue rings.
- **An opened fid cannot be walked** (9P rule). `enumerate_dir` therefore
  clones the directory fid: the clone is opened for `Treaddir`, the original
  stays walkable for the per-entry `Tgetattr` pass that fills real sizes and
  times (~3 RPCs per entry; fine at `local`-fsdev latency).
- **Symlink resolution is client-side** (bounded depth 8, absolute targets
  reinterpreted against the share root) because Twalk hands back the symlink
  node itself. `symlink_metadata` stays lstat-shaped; `open`/`stat`/
  `unix_metadata` resolve.
- **Deviation from plan:** none of substance. The `test.sh` skip path became
  a kernel-side self-skip (`find_virtio_9p_devices().is_empty()` → skip,
  matching the `net::is_available()` convention) instead of a shell-side
  filter, so a present device with a broken mount still fails hard.
- **Watch list:** (1) polled `wait_used` under the client spin lock — if a
  slow host op ever stalls a CPU visibly, move to the virtio-blk INTx +
  `Waiter` pattern; (2) per-entry getattr in `enumerate_dir` on huge
  directories; (3) `test.sh` runs QEMU under `set -eu`, so the script exits
  with QEMU's raw code (33 = pass) — the trailing "Tests passed!" mapping is
  dead code that predates this plan.

---
title: "feat: Linux-compatible ext2 filesystem"
type: feat
status: implemented
date: 2026-07-18
depth: deep
---

# feat: Linux-compatible ext2 filesystem

## Implementation Status

Implemented on 2026-07-18. The default `/data` image is ext2; the kernel
supports the bounded feature profile described below, full direct/indirect
block mapping, namespace and link mutations, sparse files, Unix metadata,
dirty-volume gating, optional `/legacy-data`, and host-side fsck/interoperability
scripts. The kernel filesystem suite passes against both the default 4 KiB /
256-byte-inode image and a 1 KiB / 128-byte-inode eight-group image; e2fsprogs
1.47.0 reports both guest-mutated images clean under `e2fsck -fn`.

## Summary

Add a real, writable, Linux-compatible ext2 implementation to AgenticOS and
make it the default backing filesystem for `/data`. The volume must be created
by `mke2fs`, mountable and repairable by Linux/e2fsprogs, and remain clean under
`e2fsck -fn` after AgenticOS creates, rewrites, truncates, links, renames, and
deletes files and directories.

This is deliberately an **ext2** delivery, not a partial driver marketed as
ext4. AgenticOS will classify ext2/ext3/ext4 from superblock feature bits and
reject unsupported layouts explicitly. ext3 journal replay and ext4 extents,
64-bit group descriptors, metadata checksums, inline data, and bigalloc are
separate follow-ups.

The end-state mount topology is:

| Mount | Backing store | Mutability | Persistence |
|---|---|---:|---|
| `/` | `overlay(tmpfs, boot FAT)` | Read/write | Snapshot blob on `/data` after `sync(2)` |
| `/data` | Linux-compatible ext2 image | Read/write | Immediate metadata/data writes; clean checkpoint on `sync(2)` |
| `/host` | QEMU vvfat | Read-only | Host-owned |
| `/legacy-data` | Optional old FAT `data.img` | Read-only | Migration-only |

The BIOS boot disk remains FAT because the current bootloader image builder
owns that contract. Moving `/` itself to ext2, adding JBD2, or replacing the
bootloader is not required to deliver a real Linux filesystem.

## Problem Frame

AgenticOS has most of the surrounding plumbing, but no ext filesystem today:

- `detect_filesystem` checks only the ext magic and always returns `Ext2`; it
  does not inspect revision or feature flags.
- `vfs::auto_mount` recognizes `Ext2 | Ext3 | Ext4` only to return
  `UnsupportedOperation`.
- `/data` is a whole-disk FAT32 image. It supports regular-file create/write/
  unlink, but FAT mkdir, rmdir, and rename remain deferred.
- The generic `Filesystem` contract loses Unix metadata and lacks truncate,
  links, symlink, per-handle stat, and per-filesystem sync operations.
- `File::seek` refuses offsets beyond EOF, so sparse-file semantics are
  impossible even if a backing filesystem supports holes.
- `fill_stat` fabricates mode, ownership, link count, inode number, and times
  instead of reporting filesystem metadata.
- `find_filesystem` uses raw `starts_with`; a mount at `/data` can incorrectly
  match `/database`.
- The current build script creates `data.img` with the Rust `fatfs` crate, and
  the test runner never validates guest writes with a second implementation.

The feature is complete only when the on-disk structures and the user-visible
semantics agree. A directory-list-only parser is useful bring-up work, but is
not the product outcome.

## Goals

1. Read and write ext2 revision 1 volumes with 1 KiB, 2 KiB, or 4 KiB blocks,
   128- to 256-byte inodes, multiple block groups, and 255-byte names.
2. Support regular files, directories, symbolic links, hard links, sparse
   holes, direct/singly/doubly/triply-indirect block maps, and case-sensitive
   path lookup.
3. Support create, read, write, truncate, mkdir, rmdir, unlink, rename, link,
   symlink, readlink, stat, and sync with Linux-compatible errno behavior.
4. Preserve inode identity and Unix metadata through the VFS and Linux stat
   ABI. All current processes remain uid/gid 0; permission enforcement is not
   introduced by this feature.
5. Make `/data` ext2 by default without overwriting an existing FAT image.
6. Prove two-way interoperability with `mke2fs`, `debugfs`, and `e2fsck`.
7. Fail closed on incompatible feature flags or an unclean volume; never
   silently reinterpret an ext3/ext4 volume as ext2.

## Non-Goals

- ext3/JBD2 journal replay or journaled writes.
- ext4 extents, 64-bit block numbers/descriptors, flex block groups,
  uninitialized groups, metadata checksums, bigalloc, inline data, encryption,
  verity, casefold, orphan files, or fast commits.
- POSIX ACLs, extended attributes, quotas, project IDs, immutable/append-only
  policy enforcement, device nodes, FIFOs, or sockets stored on disk.
- Online resize, defragmentation, discard/TRIM, or a kernel `mkfs` utility.
- GPT parsing. Whole-disk ext2 and MBR type `0x83` partitions are sufficient.
- Replacing the boot FAT, `/host` vvfat, tmpfs, overlay, or synthetic `/bin`.
- Full multi-user authorization. Metadata is stored and reported, but the
  current all-root process model remains.

## Compatibility Contract

### Supported baseline

The default image is created with a deliberately narrow profile:

```text
revision       dynamic/revision 1
block size     4096 bytes (driver also tests 1024 and 2048)
inode size     256 bytes (driver accepts 128..256, aligned and bounded)
incompat       FILETYPE
ro_compat      SPARSE_SUPER | LARGE_FILE
compat         none
journal        none
```

The image command should be equivalent to:

```sh
mke2fs -q -t ext2 -F -b 4096 -I 256 \
  -L AGENTIC-DATA -O none,filetype,sparse_super,large_file \
  -E lazy_itable_init=0 target/bootloader/data-ext2.img
```

The implementation must validate the emitted feature mask rather than assume
the local e2fsprogs defaults stayed constant.

### Mount policy

| Volume shape | Read-only mount | Read/write mount |
|---|---:|---:|
| Supported ext2 profile, clean | Yes | Yes |
| Unknown incompat bit | No | No |
| Unknown ro-compat bit | Yes only after structural validation | No |
| `HAS_JOURNAL` without recovery needed | Yes, no journal replay | No |
| `RECOVER`/journal replay required | No | No |
| extents, 64-bit, metadata checksum, bigalloc, inline data | No | No |
| Dirty/error ext2 state | Yes | No unless explicit developer override |

`detect_filesystem` classifies the family for diagnostics; the ext2 mount
validator decides whether the exact layout is usable. ext3/ext4 detection is
therefore honest even before their implementations exist.

### Explicit limits

- Files use the ext2 `i_block[15]` tree and the regular-file high-size field.
  Arithmetic is `u64` and checked before conversion to device LBAs.
- The driver accepts only filesystem blocks that are powers of two from
  1024 through 4096 and exact multiples of the underlying 512-byte sector.
- Names are arbitrary non-NUL bytes except `/`, at most 255 bytes. The current
  VFS exposes UTF-8 strings, so invalid UTF-8 directory names are visible to
  low-level ext2 tests but return a defined `InvalidPath` at the string API.
- On-disk special files are reported as `Other`; opening them returns
  `UnsupportedOperation` until device-node support exists.

## Key Technical Decisions

### D1. ext2 is the first honest compatibility target

ext2 supplies the Linux filesystem model AgenticOS is missing—stable inodes,
case-sensitive names, links, sparse files, permissions, and full directory
mutation—without pretending to implement the substantially larger ext4/JBD2
contract. ext4's journal protects metadata transactions, while ext4 extents,
checksums, and 64-bit descriptors change core on-disk interpretation. Those
features must be implemented before an ext4 claim is made.

### D2. `/data` migrates first; the boot filesystem does not

The ext2 driver is generic and mountable through the VFS, but the first
production consumer is `/data`. This isolates writer bugs from the boot disk,
preserves `/host`, and avoids coupling filesystem work to the bootloader. The
existing overlay snapshot files continue to live on `/data`; their format does
not need to change.

### D3. Use e2fsprogs for formatting and verification

Do not write a second, host-only formatter and then test the kernel against its
own assumptions. `mke2fs` creates the default and golden images, `debugfs`
inspects data written by AgenticOS, and `e2fsck -fn` validates allocation,
directory, inode, and link-count consistency. On macOS these commands come from
Homebrew `e2fsprogs` (keg-only on typical installations), so tooling searches
both `PATH` and the Homebrew opt prefix and emits an actionable failure.

### D4. Add a filesystem-block adapter; do not leak 512-byte sector math

`BlockDevice` addresses hardware sectors. ext2 addresses filesystem blocks.
A checked adapter performs byte-range and 1/2/4 KiB filesystem-block I/O,
including the superblock at byte 1024. All ext2 modules consume that adapter,
which centralizes overflow, alignment, bounds, short-buffer, and read-only
checks. `PartitionBlockDevice` must forward `is_read_only` and `flush`.

### D5. Pin the filesystem in open handles

`File` currently re-resolves its stored path on every operation. The revised
open-file description stores the owning `&'static dyn Filesystem` and an opaque
filesystem handle. Reads, writes, fstat, truncate, sync, and close use the
pinned owner even after rename or unlink. This is required for Unix
unlink-while-open semantics and removes path lookup from sparse ELF page-ins.

The ext2 open-handle table maps opaque IDs to `{ inode, access_flags }` and
keeps a per-inode open count. An inode whose link count reaches zero is
reclaimed only after its last open description closes.

### D6. Serialize metadata mutation on the single CPU

Each ext2 mount owns parsed immutable geometry plus an interrupt-safe mutable
state lock. Allocation bitmaps, group/superblock counters, inode changes, and
directory edits occur under one defined lock order. Do not hold process-table,
fd-table, user-pointer, or overlay locks while acquiring it. Reads may use
bounded metadata caches, but no cache entry can outlive its owning mount.

### D7. ext2 write ordering plus a clean-checkpoint gate

There is no journal. The driver therefore uses a conservative dirty-volume
contract:

1. A clean mount starts in `CleanCheckpoint` state.
2. Before the first metadata-changing operation after a checkpoint, clear
   `EXT2_VALID_FS` in the primary superblock and flush it.
3. Apply operation-specific ordering so an interruption tends toward a leak,
   not a live directory entry pointing at reallocated data.
4. `sync` flushes file data, indirect blocks, inodes, bitmaps, group
   descriptors, and the primary/backup superblocks, then sets
   `EXT2_VALID_FS` and flushes again.
5. The next mutation clears the bit before touching metadata.

On boot, an unclean volume mounts read-only with a loud diagnostic. A developer
may opt into a forced writable mount through fw_cfg, but normal launch and tests
must not force it silently. `scripts/fsck-data.sh` provides the supported repair
path while QEMU is stopped.

Operation ordering:

| Operation | Required order before final counter/checkpoint updates |
|---|---|
| Grow/write | Allocate+zero data/indirect blocks, write data, then publish pointers and size in inode |
| Create | Allocate/initialize inode, then publish directory entry |
| Unlink | Remove directory entry/decrement links, then free inode blocks when links and opens reach zero |
| Truncate shrink | Reduce inode size/pointers first, then free now-unreachable blocks |
| `mkdir` | Allocate inode+block, write `.`/`..`, update link counts, then publish parent entry |
| `rmdir` | Verify only `.`/`..`, remove parent entry/update links, then reclaim child |
| Rename | Publish/replace destination, update `..` when moving a directory, then remove source |

Rename still has a duplicate-name window without a journal. A crash leaves the
volume dirty, and `e2fsck` is the recovery authority. The implementation must
not claim atomic crash recovery.

### D8. Store real metadata but keep root authorization

`DirectoryEntry` (or a replacement `Metadata` type) carries inode, mode, uid,
gid, link count, size, allocated 512-byte blocks, and atime/mtime/ctime. FAT,
tmpfs, overlay, `/bin`, pipes, and sockets synthesize appropriate values.
`fill_stat` becomes a mechanical ABI conversion rather than a policy layer.

New ext2 objects default to uid/gid 0 and modes derived from the syscall mode
after umask (start with process umask `022`). The current kernel does not deny
access from mode bits because every process reports uid 0. chmod/chown can be a
follow-up; stored metadata is nevertheless preserved and reported correctly.

## Detailed Requirements

### R1 - Block and VFS foundations

- Add checked `read_bytes`, `write_bytes`, `read_fs_block`, and
  `write_fs_block` helpers over `BlockDevice`.
- Reject zero-length, overflowed, misaligned, out-of-capacity, and partial
  requests consistently; never panic on a malformed image.
- Forward partition `flush`, `is_read_only`, and capacity correctly.
- Fix mount matching so `/data` matches `/data` and `/data/...`, not
  `/database`; normalize the relative root path to one convention.
- Replace filesystem-family-specific static arrays in `vfs.rs` with bounded
  mount-owned slots that can hold FAT or ext2 without unsafe slot-index
  assumptions. A smaller first patch may add `MOUNTED_EXT2`, but the public
  mount path must not branch by filesystem after construction.
- Pin the owning filesystem in `File` and `Directory` descriptions.
- Extend the trait with per-handle `metadata`, `truncate`, `sync_handle`,
  `read_link`, `link`, and `symlink` operations. Unsupported filesystems return
  explicit errors.
- Make `vfs_sync_all` dependency-aware: flush overlay/synthetic producers before
  the writable filesystem that stores their output, then take the ext2 clean
  checkpoint last. Do not rely on incidental mount insertion order.
- Permit seek beyond EOF for regular writable files. Reads from holes return
  zeroes; a write allocates only touched blocks.

### R2 - On-disk parsing and validation

- Parse little-endian fields explicitly; do not dereference packed structs.
- Validate magic, revision, state, error policy, block/inode counts, block
  size, blocks/inodes per group, first data block, inode size, descriptor
  placement, and all multiplication/addition before I/O.
- Compute block-group count from both block and inode geometry and require the
  results to agree within the final partial group.
- Parse primary superblock and descriptor table, with backup-superblock
  locations derived from sparse-super rules.
- Classify ext2/ext3/ext4 from compat/incompat/ro-compat bits.
- Validate every bitmap, inode-table, indirect, and data block number belongs
  to the volume and is not reserved metadata before following it.
- Surface `Corrupted` for structural violations, `InvalidFilesystem` for a bad
  mount target, and `UnsupportedFeature` (new error) for a valid but unsupported
  layout. Map these to `EIO` and `EOPNOTSUPP`/`ENOTSUP`, respectively.

### R3 - Inodes and block maps

- Locate inode N via its group descriptor and inode table; inode 2 is root.
- Decode/encode mode, uid/gid, timestamps, link count, `i_blocks`, flags,
  generation, size low/high, deletion time, and `i_block[15]` while preserving
  unknown tail bytes in larger inodes.
- Implement logical-to-physical lookup for 12 direct pointers plus single,
  double, and triple indirection with checked fan-out math.
- Treat zero pointers as sparse holes on read.
- Allocate and zero indirect blocks before publishing parent pointers.
- Track `i_blocks` in 512-byte sectors, including indirect metadata blocks.
- Truncate growth changes only size; truncate shrink detaches pointers before
  returning blocks to bitmaps and recursively frees empty indirect blocks.
- Cap an operation's temporary memory independently of file size. Reading a
  4 KiB ELF page must not allocate the whole file.

### R4 - Directory and path semantics

- Parse both legacy `ext2_dir_entry` and FILETYPE `ext2_dir_entry_2` records.
- Validate `rec_len` alignment/bounds, name length, inode range, and complete
  consumption of each directory block.
- Lookups are byte-exact and case-sensitive. Readdir filters unused records but
  reports `.` and `..` according to Linux behavior; higher UI layers may hide
  them.
- Insert by splitting an existing record's slack; append a new directory block
  when no record fits.
- Delete by merging the removed record into the previous record when possible,
  or zeroing its inode when it begins a block.
- `mkdir` creates correct `.` and `..`, increments the parent's link count,
  and initializes directory size/block accounting.
- `rmdir` rejects non-empty directories. Root and active mount points return
  `EBUSY`.
- Rename supports same- and cross-directory moves within one mount, destination
  replacement, file/directory type rules, parent link-count changes, and
  descendant-cycle prevention. Cross-mount remains `EXDEV`.

### R5 - Allocation and accounting

- Select groups using free-count hints, then scan block/inode bitmaps with a
  bounded next-fit cursor. Never allocate reserved inodes or metadata blocks.
- Update the bitmap, group descriptor count, and superblock count as one
  serialized mutation with defined write ordering and rollback before
  publication when an I/O error occurs.
- Honor the final partial block group and reserved-block count.
- Initialize every newly allocated data or metadata block to zero before it is
  reachable, preventing stale-data exposure.
- Recalculate `FilesystemStats` from superblock counters and expose actual
  block size/inode totals.
- Unit-test double allocation, double free, exhausted groups, partial final
  groups, counter underflow/overflow, and injected write failures.

### R6 - Files and open-handle lifetime

- Open regular files by inode, not by a path-derived pseudo-inode.
- Preserve shared file position across dup/fork through the existing `Arc<File>`
  open description.
- Enforce read/write/open flags and append at the filesystem operation boundary.
- `O_CREAT|O_EXCL`, `O_TRUNC`, append, read/write, and sparse write-past-EOF
  match Linux behavior. Add `O_EXCL` handling to `open_common`.
- An unlinked file remains readable/writable/stat-able through open fds. Its
  blocks and inode are reclaimed after both link count and open count reach
  zero.
- `fstat`, `ftruncate`, `fsync`, and `fdatasync` operate on the pinned handle's
  filesystem, not all mounts or the old pathname.
- Short writes are allowed only after some bytes were committed; otherwise
  return the underlying error. Update the live handle size only after the inode
  write succeeds.

### R7 - Links and Unix metadata

- Hard links increment/decrement inode link count and reject directories.
- Fast symlinks store targets up to the inline `i_block` capacity; longer
  symlinks use ordinary data blocks.
- Path resolution follows symlinks component-by-component with a fixed limit
  of 40 traversals; `lstat`/`AT_SYMLINK_NOFOLLOW` inspect the link itself.
- Wire Linux x86-64 `link`, `linkat`, `symlink`, and `symlinkat` syscall numbers.
  Existing procfs `readlink` special cases remain first; filesystem symlinks are
  the fallback.
- Populate stat inode, mode, uid/gid, nlink, size, blocks, block size, and
  timestamps from ext2. FAT/tmpfs/overlay keep coherent synthetic metadata.
- Use the PIT-backed realtime approximation for mutations until RTC support
  lands. Preserve timestamps created by Linux tools even though boot-relative
  AgenticOS time may be earlier.

### R8 - Image lifecycle and migration

- Rename the new default image to `target/bootloader/data-ext2.img`; never
  overwrite or reformat an existing image whose magic/features do not match.
- `build.rs` creates the file only when absent, invokes the discovered
  `mke2fs`, then reopens and validates the exact supported feature profile.
- `build.sh` and `test.sh` attach `data-ext2.img` at IDE index 2. Tests use a
  fresh or copied image with QEMU snapshot isolation as appropriate.
- Retain `AGENTICOS_DATA_IMAGE` as an exact image override. Detection, not the
  filename, determines FAT versus ext2.
- Add optional `AGENTICOS_LEGACY_DATA_IMAGE`; when set, attach it at IDE index
  3 and mount its FAT filesystem read-only at `/legacy-data`. This supports an
  in-guest `cp -a /legacy-data/. /data/` migration without risking the source.
- `.conductor/setup.sh` verifies `mke2fs`, `e2fsck`, and `debugfs` and prints
  the Homebrew `brew install e2fsprogs` remedy. It should not silently install
  packages.
- Add `scripts/fsck-data.sh` that refuses to run while the image is attached to
  a live workspace QEMU, defaults to read-only `e2fsck -fn`, and requires an
  explicit repair flag for mutations.

### R9 - Boot and mount behavior

- `kernel::try_mount_data_disk` accepts the supported ext2 profile and mounts
  it writable at `/data`; legacy FAT remains supported when explicitly used.
- An invalid or unsupported ext volume logs the feature names and raw masks,
  then leaves `/data` absent. An unclean supported volume mounts read-only.
- Plumb `AGENTICOS_FORCE_DIRTY_MOUNT=1` through QEMU fw_cfg to the kernel. The
  default is false in both interactive and test launches.
- Add optional Secondary Slave probing for `/legacy-data`; its absence is
  silent and it is never considered an overlay persistence target.
- Restore the root overlay only after writable `/data` is mounted, as today.
- Update File Manager's capability model: ext2 `/data` supports new folder,
  rename, move, and delete rather than the current FAT limitation.

## Implementation Units

### U1 - Checked filesystem-block I/O

**Files:**

- Create `src/fs/block_io.rs`.
- Modify `src/drivers/block.rs`, `src/fs/partition.rs`, `src/fs/mod.rs`.
- Test in new `src/tests/fs_block_io.rs` or ext2 host-unit module.

**Work:** Implement checked sector/byte/fs-block translation and a bounded
in-memory block device for tests. Forward partition capabilities and flush.
Pin behavior for unaligned superblock access, 1/2/4 KiB blocks, last-block
bounds, overflow, read-only writes, and injected I/O failure.

**Gate:** `cargo check --features test`; focused block-I/O tests pass.

### U2 - VFS and metadata contract cleanup

**Files:**

- Modify `src/fs/filesystem.rs`, `src/fs/file_handle.rs`, `src/fs/fs_manager.rs`,
  `src/fs/vfs.rs`.
- Adapt `src/fs/fat/fat_filesystem.rs`, `src/fs/tmpfs/filesystem.rs`,
  `src/fs/overlay/filesystem.rs`.
- Modify `src/userland/fdtable.rs`, `src/userland/syscalls.rs`, and focused tests.

**Work:** Pin filesystem owners in handles; fix mount-component matching; add
real metadata, truncate, handle-stat/sync, and link operations with defaults;
permit sparse seeks; make fstat/fsync use the handle. Keep current FAT/tmpfs/
overlay behavior green before adding ext2.

**Gate:** All existing filesystem and userland tests pass unchanged except
assertions intentionally upgraded for metadata/seek behavior.

### U3 - Superblock, feature, and group parser

**Files:**

- Create `src/fs/ext2/{mod.rs,ondisk.rs,superblock.rs,group.rs,error.rs}`.
- Modify `src/fs/filesystem.rs` detection.
- Add e2fsprogs-created fixtures under `tests/fixtures/ext2/` or generate them
  deterministically in a test-preparation script.

**Work:** Parse/validate ext geometry and classify ext2/ext3/ext4 feature sets.
Cover 1/2/4 KiB, multiple groups, partial final group, sparse-super backups,
bad magic, inconsistent counts, unsupported bits, dirty state, and truncated
devices.

**Gate:** No malformed fixture panics or issues out-of-range I/O; classification
and mount-policy tests pass.

### U4 - Read-only inodes, block maps, directories, and files

**Files:**

- Create `src/fs/ext2/{inode.rs,block_map.rs,directory.rs,filesystem.rs}`.
- Modify `src/fs/mod.rs`, `src/fs/vfs.rs` for a read-only ext2 mount slot.
- Extend `src/tests/filesystem.rs` or add `src/tests/ext2.rs`.

**Work:** Load inodes, map all indirection levels, read holes as zero, walk
case-sensitive directories, stat, enumerate, open, partial-read, and fast
symlink data. Keep reads page-sized and allocation-bounded.

**Gate:** AgenticOS reads an e2fsprogs golden tree containing nested dirs,
mixed-case distinct names, a sparse file, fast/slow symlinks, hard links,
single/double-indirect files, and preserved metadata. Host-calculated hashes
match guest reads.

### U5 - Allocation and writable regular files

**Files:**

- Create `src/fs/ext2/{allocator.rs,writeback.rs}`.
- Extend `inode.rs`, `block_map.rs`, `filesystem.rs`.
- Add failure-injection tests.

**Work:** Implement inode/block allocation, zero-before-publish, indirect-tree
growth, create, overwrite, append, sparse write, truncate grow/shrink, inode
accounting, and open-unlinked lifetime. Mark dirty before mutation and implement
the clean sync checkpoint.

**Gate:** Boundary tests cross direct→single and single→double mappings;
bitmap/counter tests stay consistent after injected failures; an AgenticOS-
written image passes `e2fsck -fn`.

### U6 - Writable directories and rename

**Files:** Extend `src/fs/ext2/directory.rs`, `allocator.rs`, `filesystem.rs` and
filesystem/userland tests.

**Work:** Implement record split/merge, directory growth, mkdir/rmdir, unlink,
same/cross-directory rename, destination replacement, `..` maintenance,
link-count updates, cycle prevention, and error mapping.

**Gate:** A matrix of file/dir rename cases matches Linux errno expectations;
each resulting image passes `e2fsck -fn`; crash-point simulations always leave
the volume marked dirty.

### U7 - Symlinks, hard links, and real stat data

**Files:**

- Extend filesystem/VFS APIs and ext2 implementation.
- Modify `src/userland/abi.rs`, `src/userland/syscalls.rs`.
- Modify `userland/libs/runtime` syscall wrappers as required.

**Work:** Wire link/symlink syscalls, component-wise symlink resolution with a
40-hop limit, lstat/no-follow behavior, fast and block-backed symlinks, inode
identity, mode/owner/link/timestamp stat fields, and root-umask defaults.

**Gate:** BusyBox `ln`, `ln -s`, `readlink`, `stat`, `cp -a`, and directory
operations work on `/data`; link counts and symlink targets agree in `debugfs`.

### U8 - Mount slots, image tooling, and default `/data`

**Files:**

- Modify `build.rs`, `Cargo.toml`, `build.sh`, `test.sh`.
- Modify `src/kernel.rs`, `src/fs/vfs.rs`.
- Modify `.conductor/setup.sh`.
- Create `scripts/fsck-data.sh` and `scripts/test-ext2-interop.sh`.

**Work:** Create/validate `data-ext2.img`, attach and mount it, plumb the dirty
override, retain explicit FAT override, and add optional `/legacy-data`. Do not
delete `data.img` or auto-copy user data.

**Gate:** Fresh clone setup creates ext2 once and reuses it across builds;
`./build.sh -n`, focused QEMU tests, and the host interop script pass on macOS
and Linux environments with e2fsprogs.

### U9 - Product integration, documentation, and full qualification

**Files:**

- Update `src/fs/CLAUDE.md`, root `CLAUDE.md`, `README.md`,
  `docs/ARCHITECTURE.md`, and `docs/IMPLEMENTATION_PLAN.md`.
- Update `userland/apps/fileman/src/main.rs` and its capability tests.
- Add `docs/solutions/learnings/YYYY-MM-DD-ext2-write-bringup.md` after the
  implementation, capturing actual failures and invariants.

**Work:** Document the mount topology, compatibility mask, repair/migration
workflow, crash limitations, and ext3/ext4 non-support. Update UI messaging and
remove the statement that `/data` cannot mutate directories.

**Gate:** `cargo fmt --check`, `cargo check`, `cargo check --features test`,
targeted ext2/filesystem/userland/file-manager tests, `./build.sh -n`, full
`./test.sh`, and destructive-copy host interop all pass.

## Test Strategy

### Pure/in-kernel structural tests

- Superblock offsets, endian decoding, feature masks, group-count math.
- Block/inode bitmap scans and counters across group boundaries.
- Inode-table offsets for 128/256-byte inodes.
- Direct/single/double/triple logical block index calculation.
- Sparse reads and zero-before-publish.
- Directory record validation, split, merge, and block growth.
- Fast/slow symlink storage and traversal-loop detection.
- Open-unlinked lifetime and delayed reclamation.
- I/O failure at every metadata publication boundary.
- Mount prefix boundary regression (`/data` versus `/database`).

### Golden-image QEMU tests

Generate fixtures with e2fsprogs, not with AgenticOS:

- 1 KiB and 4 KiB blocks.
- One and multiple block groups.
- 128- and 256-byte inodes.
- Nested directories and names at 1, 255, and invalid 256 bytes.
- `case` and `CASE` as distinct entries.
- Empty, inline-sized, direct, single-indirect, double-indirect, and sparse
  files with host-recorded hashes.
- Hard-link pairs and short/long symlinks.
- Clean, dirty, unsupported-feature, corrupt-rec_len, invalid-pointer, and
  inconsistent-counter images.

### Two-way interoperability test

`scripts/test-ext2-interop.sh` operates on a temporary explicit path:

1. Create and populate ext2 with `mke2fs`/`debugfs`.
2. Boot QEMU without `snapshot=on` for that temporary image only.
3. In the guest, mutate a deterministic tree and call `sync`.
4. Exit through `isa-debug-exit`.
5. Run `e2fsck -fn`; require a zero/clean result.
6. Use `debugfs` to dump files, inspect inodes/link counts/modes, and hash data.
7. Modify the tree with `debugfs`, reboot AgenticOS read-only, and verify the
   guest observes the host changes.

The script must never point at the developer's persistent default image unless
they pass it explicitly.

### Performance/regression checks

- 4 KiB `read_at` on a multi-MiB ext2 ELF allocates no file-sized temporary.
- Sequential read/write throughput is logged separately for FAT and ext2.
- Directory creation of 1,000 files is not quadratic in inode or group scans.
- PIO requests remain at most 128 sectors and preserve the IDE interrupt-guard
  invariant.
- Existing boot-FAT, `/host`, tmpfs, overlay, zsh, BusyBox, GUI application,
  and sparse ELF-loader tests remain green.

## Rollout and Migration

1. Land U1-U4 with ext2 read-only and no default mount change.
2. Land U5-U7 behind an `AGENTICOS_DATA_IMAGE` opt-in. Run destructive tests
   only on generated/copy images.
3. Land U8 to create `data-ext2.img` and make it the fresh-clone default.
4. Preserve old `target/bootloader/data.img`; do not rename, delete, or format
   it automatically.
5. Users with old FAT data can set `AGENTICOS_LEGACY_DATA_IMAGE` and copy from
   `/legacy-data` to `/data`, call `sync`, inspect with `e2fsck -fn`, then remove
   the legacy override.
6. Keep explicit FAT `/data` mounting working for one release cycle. Mark it
   legacy in logs/docs, then decide separately whether FAT writes remain a
   supported data-volume option.

Rollback is configuration-only: point `AGENTICOS_DATA_IMAGE` at the preserved
FAT image. No ext2 rollout step mutates that source.

## Risks and Mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Malformed metadata causes out-of-bounds disk/heap access | Kernel corruption | Explicit endian parsing, checked math, volume bounds on every pointer, corrupt-image fixtures |
| No journal means interrupted rename/allocation can be inconsistent | Data loss | Dirty-before-mutation, clean sync checkpoint, read-only fallback, e2fsck repair workflow, honest ext2 scope |
| Static handle/mount storage repeats current unsafe lifetime patterns | Unsound references | Pin owners, bounded mount-owned storage, no slot reuse while mounted; refactor before ext2 default |
| Single-CPU spinlock is held across preemption | Deadlock | Interrupt-safe filesystem mutation lock and documented lock order |
| Metadata writes become very slow over IDE PIO | Poor UX | Bounded metadata cache, group hints/next-fit cursors, batched whole-fs-block I/O, performance gates |
| e2fsprogs is keg-only/missing on macOS | Setup failure | Conductor preflight, PATH+Homebrew opt discovery, exact install message |
| Existing FAT data is overwritten | Irrecoverable user loss | New filename, magic validation, no auto-format/rename, optional read-only legacy mount |
| Partial ext4 support is mistaken for compatibility | Corruption of foreign volumes | Feature-mask gate and diagnostics; ext3/ext4 always read-only/rejected until their own plans land |
| Overlay sync marks ext2 clean while another mutation races | False clean state | Serialize sync and mutations under one filesystem transaction state lock |
| Symlink traversal escapes mount assumptions or loops | Wrong target/infinite loop | Component-wise normalization, mount re-resolution after expansion, 40-hop bound, no raw concatenation |

## Acceptance Criteria

- A default `data-ext2.img` reports as ext2 to `file(1)`/`blkid` and mounts on
  Linux without conversion.
- AgenticOS mounts it at `/data` and supports nested create/write/read/truncate/
  rename/unlink/rmdir plus hard and symbolic links.
- `/data/case` and `/data/CASE` coexist and retain distinct inode identities.
- Sparse writes do not allocate blocks for holes, and reads return zeroes.
- `stat` reports real ext2 inode, mode, uid/gid, link count, size, block count,
  and timestamps.
- Open files survive rename and unlink; final close reclaims an unlinked inode.
- A clean `sync` makes `e2fsck -fn` report no errors after the complete guest
  mutation matrix.
- Dirty or unsupported volumes never mount writable by default and provide an
  actionable serial diagnostic.
- Existing FAT boot/root reads, `/host`, tmpfs/overlay persistence, and all
  userland applications continue to work.
- Existing FAT `data.img` files are never overwritten during build or upgrade.

## Deferred Follow-Ups

1. **JBD2 + ext3:** journal discovery, checksum/version validation, replay,
   transaction credits, revoke records, checkpointing, and ordered-data mode.
2. **ext4 read support:** extents, 64-bit descriptors, flex/uninitialized
   groups, directory indexes, and metadata checksums.
3. **ext4 write support:** delayed/multiblock allocation, extent-tree edits,
   orphan handling, journal integration, and crash qualification.
4. **Permissions/ownership:** per-process credentials, umask syscall/state,
   chmod/chown, access checks, sticky/setgid directory semantics, and ACLs.
5. **Extended attributes:** inode/body xattrs and Linux xattr syscalls.
6. **Native ext root:** a separately built system image mounted at `/`, with
   boot FAT exposed at `/boot` and a migration away from the tmpfs overlay.
7. **Storage transport:** VirtIO-blk and a real buffer/page cache to replace
   synchronous IDE PIO as the primary filesystem transport.

## References

- [Linux kernel ext4 on-disk superblock documentation](https://www.kernel.org/doc/html/latest/filesystems/ext4/super.html)
- [Linux kernel ext4 inode documentation](https://www.kernel.org/doc/html/latest/filesystems/ext4/inodes.html)
- [Linux kernel ext4 directory-entry documentation](https://www.kernel.org/doc/html/latest/filesystems/ext4/directory.html)
- [Linux kernel ext4 block-map documentation](https://www.kernel.org/doc/html/latest/filesystems/ext4/blockmap.html)
- [Linux kernel JBD2 journal documentation](https://www.kernel.org/doc/html/latest/filesystems/ext4/journal.html)
- [e2fsprogs source repository](https://github.com/tytso/e2fsprogs)

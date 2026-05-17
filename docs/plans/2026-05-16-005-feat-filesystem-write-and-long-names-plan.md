---
title: "feat: Filesystem write support and long/mixed-case filenames"
type: feat
status: active
created: 2026-05-16
rebased_against_main: 2026-05-16 (after PR #28)
depth: deep
---

# feat: Filesystem write support and long/mixed-case filenames

## Summary

Take the AgenticOS filesystem layer from "read-only FAT, 8.3 uppercase" to "read+write FAT with long mixed-case names, plus a RAM overlay that makes the root writable from day one." Delivered in four sequenced phases so the high-risk on-disk-write code lands only after every prerequisite (mixed-case round-trip, overlay-backed writable namespace, complete syscall surface) is already tested and shipped.

Today three concepts sit awkwardly together:

1. **Boot FAT root (`/`)** — built by the `bootloader` crate, mounted from Primary Master partition 1, read-only, 8.3 uppercase only.
2. **vvfat `/host` mount** — QEMU synthesizes a FAT16 image from `host_share/` on the Mac, read-only by design (`fat:rw:` in vvfat is known-corrupting), 8.3 uppercase as currently parsed.
3. **Synthesized `/bin` namespace** — kernel intercepts `/bin/<applet>` at the syscall layer. After PR #28 it has **two arms**: BusyBox applets rewrite to `/host/BB.ELF` (read paths still resolve to `BB.ELF`); GUI applets (`painting`, `calc`, `notepad`, `tasks`, `explorer`) rewrite to `/host/GLAUNCH.ELF` which then invokes `sys_gui_launch` (nr 5000) to spawn the kernel-side `RunnableProcess`. The merged applet list is exposed via `bin_namespace::merged_bin_entries()`.

The plan keeps all three, but unifies them around one upgraded FAT reader that understands VFAT LFN entries, and adds an overlay/tmpfs layer plus real FAT writes so userland can actually create, modify, and delete files.

The `/bin` namespace (both arms) is explicitly **out of scope** — it intercepts at the syscall layer before the FS is consulted and is orthogonal to read/write/LFN concerns.

---

## Problem Frame

### Three concrete pain points users hit today

1. **Writes are impossible.** `open(O_WRONLY)`, `mkdir`, `unlink`, `rename` all return `EROFS` at the syscall layer (`src/userland/syscalls.rs:1833`, `src/userland/syscalls.rs:98`). zsh can't keep history, scripts can't redirect to files, agents have no scratch space. The FS trait declares `write`/`mkdir`/`unlink`/`rmdir`/`rename`/`sync` (`src/fs/filesystem.rs:177-195`) but the FAT impl hard-returns `ReadOnly` for every one (`src/fs/fat/fat_filesystem.rs:152,237,250,254,258,262`).

2. **8.3 uppercase only.** The on-disk format is fine — every FAT-formatted disk in 2026 uses VFAT LFN entries. The kernel's directory parser explicitly *filters out* LFN entries (`src/fs/fat/directory.rs:187`). The `LongFileNameEntry` struct exists with a `chars()` method but is never decoded into anything usable. End result: `notes.markdown` on the Mac side becomes invisible inside `/host`, and the boot image must hand-curate filenames like `HELLO.TXT` to `BB.ELF`.

3. **Build-side image generator drops case.** The `bootloader` crate's FAT writer uppercases everything. Even if the parser learned LFN tomorrow, the boot image would still ship 8.3 uppercase names because no LFN entries are emitted. Fixing this requires either replacing the image-builder or layering a post-process step.

### Why fixing these is non-trivial

The block layer is **already write-capable** (`IdeController::write_sectors` at `src/drivers/ide.rs:592-686`, `IdeBlockDevice::write_blocks` at `src/drivers/ide.rs:729`, `PartitionBlockDevice::write_blocks` pass-through in `src/fs/partition.rs`). The hard part is the FAT layer:

- FAT table writeback (FAT12 packed-byte, FAT16, FAT32; both copies in mirroring mode; FSINFO maintenance on FAT32).
- Cluster allocation (next-fit with FSINFO hint, fallback to linear scan).
- Directory entry mutation (slot reuse via `0xE5` tombstone, terminator `0x00` discipline, multi-slot LFN runs).
- LFN write (8.3 alias generation with `~N` collision suffix, UCS-2 encoding, reverse-order slot layout, checksum byte).
- Dirty bit and (eventual) chkdsk-equivalent recovery.
- Crash safety with no journal — the honest position is "we may corrupt on power-loss, document it, run a sweeper on next mount".

### Why a phased delivery

Big-bang FAT writes against the boot disk is the recipe for losing the kernel between reboots. Instead:

- Phase A ships **read-side correctness** (LFN, mixed case) without touching any write code. Low risk, immediately user-visible.
- Phase B ships **writes that physically cannot corrupt disk** (tmpfs + overlay). Lets us define and exercise the full write syscall surface against an in-RAM target.
- Phase C ships **on-disk writes** to a new third disk (`/data`), separate from `/` so a write bug can't brick the boot FS. The proven syscall surface from Phase B is now backed by FAT writes.
- Phase D ships **persistence** by flushing the overlay upper back to disk on `sync`/shutdown. Reboots survive.

---

## Scope Boundaries

### In scope
- VFAT LFN read parsing (UCS-2 → UTF-8, checksum validation, reverse-order slot collection, orphan-run tolerance, 0x05↔0xE5 translation).
- Lowercase 8.3 via the lowercase-attribute bits (`0x08`/`0x10` in the reserved byte) so `readme.txt` round-trips without an LFN slot.
- Replace the `bootloader`-crate-internal FAT image generator with a build-side step using the `fatfs` Rust crate so the boot image carries real lowercase + LFN entries. (For the bootable boot sector + kernel binary the bootloader crate stays; only the *data files* land via a post-process step.)
- An in-RAM `tmpfs` filesystem (new `src/fs/tmpfs/`) implementing the full `Filesystem` trait.
- A copy-up `overlay` filesystem (new `src/fs/overlay/`) that merges an upper writable FS over a lower read-only FS, with whiteouts and opaque-directory markers.
- Mount `/` as `overlay(upper=tmpfs, lower=boot-FAT-ro)` so all of userland sees a writable root.
- New userland syscalls: `mkdir`, `mkdirat`, `unlink`, `unlinkat`, `rmdir`, `rename`, `renameat`, `creat`, `ftruncate`, `truncate`, `fsync`, `fdatasync`, `pread64`, `pwrite64`. Open the existing `EROFS` gate at `src/userland/syscalls.rs:1833` for write modes.
- FAT write support: `write`, `mkdir`, `unlink`, `rmdir`, `rename`, plus internal pieces (`fat_table::write_entry`, `find_free_cluster`, `extend_chain`, `free_chain`, FSINFO maintenance, dirty bit, LFN write with short-name alias generator).
- A new third IDE drive (Primary Master partition is `/`, Primary Slave is `/host`, Secondary Master is the new writable FAT image mounted at `/data`).
- Persistence: on `sync` or clean shutdown, flush overlay upper layer back to the writable FAT mount (so things written under `/data/...` persist trivially, and a documented "snapshot to /data/persist.tar" path exists for `/` overlay state).
- Tests: LFN parse against golden image, write round-trip, mkdir/unlink/rename in tmpfs, FAT free-cluster allocator unit tests, overlay copy-up/whiteout tests, regression for existing read-side tests under the new overlay-mounted root.
- Documentation: `src/fs/CLAUDE.md` rewrite to reflect the new architecture; updated "Known Issues and Technical Debt" in root `CLAUDE.md`; a `docs/solutions/learnings/2026-MM-DD-fat-write-bringup.md` post-mortem after Phase C.

### Deferred to Follow-Up Work
- **Crash-safety / chkdsk-equivalent on mount.** Phase D ships best-effort flush ordering and the dirty bit; a real `fsck` pass (cluster leak detection, cross-link repair) is a follow-up. Phase D notes the failure mode explicitly.
- **Long-filename support on the `bootloader` crate's UEFI image variant.** The plan ships the BIOS image path; UEFI image uses the same data files but the bootloader crate routes them differently. Catching UEFI up is a separate, smaller plan.
- **Subdirectory enumeration in `enumerate_dir`.** `DirectoryIterator` is largely stubbed (`src/fs/filesystem.rs:231`). Phase A teaches the FAT directory walker about LFN; turning `DirectoryIterator` into a real streaming iterator (rather than the current snapshot-`Vec` model) is a separate cleanup.
- **FAT cluster-walk caching.** Documented as open in `docs/solutions/learnings/2026-05-09-multi-mib-user-binary-load.md` — every chain walk re-reads FAT entries from disk. Phase C may amplify this; if it bites, address separately.
- **Replacing IDE PIO with virtio-blk or DMA.** Same learning identifies this as the proper long-term IRQ-jitter fix. Out of scope here; mitigated by the existing `InterruptGuard` discipline.
- **Replacing vvfat with virtio-9p.** Live mac↔guest edits remain a future track (captured in the original `/host` plan).
- **A second filesystem implementation (ext2, custom AgFS).** Considered (see Alternatives), rejected for now.
- **`/bin` namespace migration to real FS entries.** Orthogonal; stays as kernel synthesis.
- **`utimensat` and POSIX timestamps.** Without an RTC driver, we set all create/modify timestamps to build time. Adding an RTC + real timestamps is a separate plan.

### Out of scope (not part of this product direction)
- Mounting non-FAT host filesystems.
- A host-side daemon, write-back sync agent, or vvfat read-write mode.
- Multi-user permissions beyond the existing `FileAttributes` bits.

---

## Key Technical Decisions

**D1. Extend FAT in place; do not add ext2 (yet).**
The block layer already does writes. `PartitionBlockDevice::write_blocks` is pass-through. The only layer rejecting writes is `src/fs/fat/fat_filesystem.rs`. Adding ext2 would duplicate ~2–4k LOC for marginal gain — and `/host` (vvfat) would still need the FAT writer to be the source-of-truth reader anyway. Revisit ext2 if FAT's structural limits (8.3 root dir size on FAT12/16, no symlinks, no extended attributes) start to bite.

**D2. tmpfs + overlay before on-disk writes.**
The cheapest path to unblocking userland write syscalls is a RAM-only FS with a copy-up overlay over the existing read-only FAT root. This:
- Lets us define and ship the complete syscall surface (mkdir/unlink/rename/ftruncate/pread/pwrite/fsync) once, against tmpfs.
- Decouples "userland thinks it has writes" from "we trust our FAT writer enough to point it at the boot disk."
- Survives every reboot regression cleanly — RAM is wiped, lower FS is untouched.
- Gives agents scratch space immediately (Phase B end state), independent of Phase C/D landing.

**D3. Writable on-disk FAT lives on a new third disk, not `/`.**
Phase C lands a third IDE drive (Secondary Master) carrying a freshly-`mkfs.fat`'d FAT32 image, mounted at `/data`. Reasons:
- A FAT writer bug at `/` would corrupt the boot image and brick the next reboot. At `/data` it loses only test data.
- The boot image is regenerated from `assets/` on every `cargo build` by `build.rs`, so writes-to-`/` wouldn't persist anyway — they'd be discarded at the next compile.
- A clean separation between immutable boot artifacts (root) and mutable user state (`/data`) is good systems hygiene independent of the bring-up risk.

**D4. Persistence story = overlay flush, not in-place writes to `/`.**
Phase D adds `sync_overlay_to_disk()` that snapshots the overlay upper-layer (tmpfs contents + whiteout list) and writes it to `/data/overlay-state/` as a structured dump that the boot path reads back on next mount. Trade: simpler than write-through, no risk of partial in-place mutations corrupting the boot FAT. Cost: a "save your work before shutdown" step (initially manual via `sync` shell command; eventually wired into the panic handler and orderly shutdown).

**D5. Use the `fatfs` Rust crate for the build-side image post-process.**
The bootloader crate's FAT writer can't be configured for LFN/case preservation. Rather than fork it, run a small `cargo xtask` (or shell step in `build.sh`) that opens the bootloader-produced `bios.img`, walks its FAT root, and re-writes every entry through `fatfs`'s LFN-aware path. Tooling lives in `xtask/` (new). Works identically on macOS and Linux CI — no `mtools` install required.

**D6. LFN read in Phase A; LFN write deferred inside Phase C.**
LFN reading is ~300 LOC with low risk: collect contiguous `0x0F`-attr slots, validate sequence + checksum, decode UCS-2. LFN *writing* is materially harder (short-name alias collision scan, slot allocation, atomicity vs the trailing 8.3 stub) and is properly part of the FAT writer in Phase C. Until then, the writable mount only at `/data` accepts 8.3-fitting names; userland sees lowercase + long names everywhere they're physically present on disk (boot image, `/host`) but new files created on `/data` initially get short names. Phase C closes the gap.

**D7. FAT writer: next-fit allocation, both FATs mirrored, FSINFO honored as hint, dirty bit set on mount-for-write.**
Standard discipline lifted from the Microsoft FAT spec and `rafalh/rust-fatfs`. Next-fit with `FSI_Nxt_Free` hint matches Windows' behavior and avoids the O(n) FAT scan on every allocate. Mirroring both FAT copies matches what every other FAT writer does and prevents `chkdsk` from yelling on a host. Dirty bit (FAT16 bit 15 of FAT[1], FAT32 bit 27) lets a future fsck pass detect unclean shutdown.

**D8. Open the syscall write gate exactly once, in Phase B, behind a `vfs_is_writable_at(path)` check.**
The `EROFS` short-circuit at `src/userland/syscalls.rs:1833` becomes a VFS query. A path is writable iff its resolving mount supports writes. Phase B makes `/` writable via the overlay; Phase C makes `/data` writable via FAT; `/host` and `/bin` (synthesized) stay non-writable forever. Single decision point, no per-syscall sprinkling.

**D9. Reject path-rewrite expansion for the writable mount.**
`apply_fs_rewrite` (`src/userland/path.rs:98`) currently hard-codes `/etc/...` → `/host/etc/...`. Resist the temptation to add `/home/...` → `/data/home/...`. New writable areas live behind real VFS mounts, not rewrite rules. Future agents reading paths see what the kernel sees.

**D10. The `/bin` namespace stays as syscall-layer interception.**
Despite the file system becoming writable, the `/bin/<applet>` rewrite (`src/userland/bin_namespace.rs`) remains a syscall-handler-level mechanism. It is intentionally invisible to the FS layer. Migrating it to "real" FS entries would mean either symlinking the ~240 merged applets (BusyBox + GUI launchers; no symlink support today) or duplicating `BB.ELF`/`GLAUNCH.ELF` per applet. Both are worse than the current synthesis. New write-side syscalls (Phase B U5) must explicitly reject mutations under `/bin` with `EPERM` — the namespace is read-only by construction.

**D11. Documented crash position: "no journal, sweeper on next mount, best-effort flush ordering."**
Phase C orders FAT-table writes before directory-entry writes (so a crash leaks a cluster rather than cross-linking one). Phase D documents the gap. Real `fsck` is deferred. Acceptable for a hobby kernel; users are warned.

---

## High-Level Technical Design

This sketch illustrates the intended approach and is directional guidance for review, not implementation specification. The implementing agent should treat it as context, not code to reproduce.

### End-state mount topology (after Phase D)

```
                 ┌──────────────────────────────────────────────┐
   Userland  ──▶ │  syscalls.rs (open/mkdir/unlink/read/write)  │
                 └────────────────────┬─────────────────────────┘
                                      │ resolve_user_path + apply_bin_rewrite
                                      ▼
                 ┌──────────────────────────────────────────────┐
                 │  VFS (vfs.rs)  — longest-prefix mount lookup │
                 │                                              │
                 │   /        → overlay(upper=tmpfs, lower=fat) │
                 │   /host    → fat (read-only, vvfat-backed)   │
                 │   /data    → fat (read-write, 3rd disk)      │
                 └────────┬─────────────────────┬──────┬────────┘
                          │                     │      │
                          ▼                     ▼      ▼
                 ┌──────────────┐      ┌──────────────────────┐
                 │  overlay/    │      │  fat/  (now r/w)     │
                 │  ├ tmpfs     │      │  ├ LFN read + write  │
                 │  └ fat (ro)  │      │  ├ FAT12/16/32 write │
                 └──────────────┘      │  ├ FSINFO + dirty    │
                                       │  └ cluster alloc     │
                                       └──────────┬───────────┘
                                                  ▼
                                       ┌──────────────────────┐
                                       │ Partition + IDE PIO  │
                                       │ (writes already work)│
                                       └──────────────────────┘
```

### Overlay copy-up flow

```
write("/etc/foo")
    │
    ▼
overlay.open(path, O_WRONLY)
    │
    ├── upper.exists(path)?  ──yes──▶ return upper handle
    │
    ├── upper.is_whiteout(path)? ──yes──▶ ENOENT
    │
    └── lower.exists(path)?
            │
            ├── yes  ──▶  COPY-UP: read full file from lower,
            │              upper.create(path), upper.write(full bytes),
            │              return upper handle
            │
            └── no   ──▶  upper.create(path), return upper handle
```

### LFN read state machine

```
walk_directory():
  lfn_run = []
  for each 32-byte entry in directory cluster chain:
    if entry.first_byte == 0x00: break (end of directory)
    if entry.first_byte == 0xE5: lfn_run.clear(); continue (deleted)
    if entry.attr == 0x0F:
        lfn_run.push(entry)               # accumulate reversed
        continue
    # entry is an 8.3 stub
    if lfn_run not empty:
        if not validate_sequence(lfn_run): drop run; use 8.3 only
        if checksum(entry.short_name) != lfn_run[0].checksum: drop run
        long_name = decode_ucs2_reverse(lfn_run)
    else:
        long_name = decode_8_3_with_case_bits(entry)
    emit(DirectoryEntry { name: long_name, attrs: entry.attrs, ... })
    lfn_run.clear()
```

### Per-phase deliverable shape

| Phase | New code lives in | Touches | User-visible delta |
|---|---|---|---|
| A | `src/fs/fat/directory.rs`, `xtask/` (new) | `build.rs`, `build.sh`, `src/fs/CLAUDE.md` | `ls /host` and `ls /` show lowercase + long names; existing binaries still load |
| B | `src/fs/tmpfs/` (new), `src/fs/overlay/` (new), `src/userland/syscalls.rs` | `src/fs/vfs.rs`, `src/kernel.rs::init_filesystems`, `src/fs/file_handle.rs` | `touch /tmp/foo`, `mkdir /var/log`, `zsh` history works; lost on reboot |
| C | `src/fs/fat/fat_table.rs`, `src/fs/fat/filesystem.rs`, `src/fs/fat/fat_filesystem.rs`, `src/fs/fat/directory.rs` (LFN write) | `src/kernel.rs` (third disk mount), `build.sh` (mkfs new disk image) | `touch /data/foo` survives reboot; mkdir/unlink/rename on `/data` |
| D | `src/fs/overlay/sync.rs` (new), `src/userland/syscalls.rs` (add `sync`/`syncfs` syscalls) | `src/panic.rs` (orderly-shutdown hook eventually), `src/kernel.rs` (boot-time restore) | `/bin/sync` (BusyBox applet) persists `/etc`/`/home` overlay deltas to `/data` via the new POSIX `sync` syscall |

---

## Output Structure

New directories and files this plan introduces. Existing files modified in place are listed per implementation unit.

```
src/fs/
├── tmpfs/                        (new, Phase B)
│   ├── mod.rs
│   ├── filesystem.rs              # Filesystem trait impl
│   ├── node.rs                    # in-memory file/dir nodes
│   └── handle.rs                  # tmpfs file/dir handles
├── overlay/                       (new, Phase B; sync added Phase D)
│   ├── mod.rs
│   ├── filesystem.rs              # Filesystem trait impl, copy-up
│   ├── whiteout.rs                # whiteout + opaque dir markers
│   └── sync.rs                    # Phase D: flush upper to disk
└── fat/                           (modified)
    ├── lfn.rs                     (new Phase A — LFN read; Phase C — LFN write)
    ├── alloc.rs                   (new Phase C — cluster allocator + FSINFO)
    └── ... existing files

xtask/                             (new, Phase A)
├── Cargo.toml
└── src/
    └── main.rs                    # post-process bios.img through fatfs

src/tests/
├── tmpfs.rs                       (new, Phase B)
├── overlay.rs                     (new, Phase B)
├── fat_lfn.rs                     (new, Phase A)
└── fat_write.rs                   (new, Phase C)
```

This is a scope declaration. Per-unit `Files:` sections remain authoritative.

---

## Implementation Units

### Phase A — Read-side correctness (mixed case + long names)

#### U1. Build-side: post-process boot image with `fatfs` crate

**Goal:** The bundled BIOS image carries real lowercase + LFN filenames so future readers can validate against ground truth.

**Requirements:** Mixed-case round-trip; foundation for U2/U3 testing.

**Dependencies:** none.

**Files:**
- `xtask/Cargo.toml` (new)
- `xtask/src/main.rs` (new — opens `target/bootloader/bios.img`, walks FAT root, rewrites entries via `fatfs` crate preserving source-file case from `assets/`)
- `build.sh` (modify — invoke `cargo run -p xtask -- repack <img>` after the bootloader build at line 187)
- `build.rs` (review — no changes expected; xtask runs after `cargo build` finishes)
- `Cargo.toml` (modify — add `xtask` to workspace `members`)
- `src/tests/fat_lfn.rs` (new — placeholder asserting `bios.img` contains LFN entries; full parsing in U2)

**Approach:**
- xtask reads the source `assets/` directory listing, opens the just-built `bios.img` via `fatfs::FileSystem::new`, deletes each existing entry, re-writes with the source-side filename (preserving case). The `fatfs` crate handles LFN slot generation, short-name alias generation, and lowercase-attribute-bit emission for pure-lowercase 8.3 names.
- Idempotent: re-running on an already-repacked image is a no-op (file contents unchanged).
- Fail loudly if `fatfs` version doesn't support LFN write (lock version in `xtask/Cargo.toml`).

**Patterns to follow:**
- `build.sh` already has a multi-stage build (line 187 invokes cargo build then assembles disk image). Add xtask call as a new stage with clear logging.
- Lock crate versions explicitly in `xtask/Cargo.toml` (project convention from `src/Cargo.toml` style).

**Test scenarios:**
- Happy path: After `./build.sh`, `mdir -i target/bootloader/bios.img ::` (or `fatfs`-based inspection in a test) shows mixed-case names matching `assets/` filenames byte-for-byte.
- Edge: file named `readme.txt` (pure lowercase 8.3) ends up with lowercase-attribute-bit set, no LFN slot consumed.
- Edge: file named `Hello World.markdown` produces one LFN run + 8.3 stub `HELLOW~1.MAR`.
- Edge: re-running xtask on already-repacked image leaves it byte-identical (snapshot test).
- Failure: source file with character invalid in FAT (`*`, `?`, `<`, `>`) fails xtask with a clear error rather than silently truncating.

**Verification:** `./build.sh` succeeds; `xxd target/bootloader/bios.img | grep -i '0f 00'` shows LFN slot signatures; existing kernel boot still works (LFN slots are skipped by today's parser).

---

#### U2. VFAT LFN read parser

**Goal:** Directory enumeration returns long mixed-case filenames; existing 8.3 lookups still work.

**Requirements:** Mixed-case round-trip end-to-end; foundation for every later phase.

**Dependencies:** U1 (need an image with LFN entries to test against).

**Files:**
- `src/fs/fat/lfn.rs` (new — LFN slot collection, checksum validation, UCS-2 → UTF-8 decode, lowercase-attribute decode)
- `src/fs/fat/directory.rs` (modify — replace the `!is_lfn()` filter at line 187 with a stateful walker that accumulates LFN runs and pairs them with the trailing 8.3 stub)
- `src/fs/fat/types.rs` (modify — extend `LongFileNameEntry` with real decode method)
- `src/fs/fat/filesystem.rs` (modify — `find_file` at line 176 now compares against decoded long names; keep `eq_ignore_ascii_case` for ASCII-only paths)
- `src/fs/filesystem.rs` (review — `DirectoryEntry.name` is already 256-byte fixed; no change)
- `src/tests/fat_lfn.rs` (modify — real LFN parse tests)
- `src/fs/CLAUDE.md` (modify — remove "8.3 only" limitation, document the new behavior and remaining caveats)

**Approach:**
- New `LfnWalker` struct holds the in-progress reversed-run buffer. Feed it 32-byte entries; it returns `Some(DirectoryEntry)` when a non-LFN, non-deleted entry closes a run.
- Validate every slot in a run: sequence number descending (with `0x40` set on the first), checksum of every slot equals checksum-of-trailing-8.3-name. Mismatch → drop the LFN run, fall back to the 8.3 name (do not panic).
- UCS-2 decode handles surrogate pairs leniently (accept like Linux does). Replace `0x05` first byte with `0xE5` per the spec.
- Lowercase-attribute bits (`0x08` / `0x10` in offset 12 of the 8.3 entry) toggle case for the basename / extension respectively, even when no LFN run is present.
- Path lookup (`find_file`) now compares case-insensitively against the *decoded* name. UTF-8 case folding via `core::str::eq_ignore_ascii_case` for the ASCII-only fast path; for non-ASCII, byte-exact match (full Unicode folding is out of scope).

**Patterns to follow:**
- The existing `read_directory_array` / `list_directory` pattern in `src/fs/fat/filesystem.rs:76` — keep the same array-out model for now; streaming iteration is a separate cleanup.
- `read_to_vec` uninit-capacity pattern from `src/fs/file_handle.rs` if any LFN decode buffer is multi-MiB (it won't be — names are bounded).

**Test scenarios:**
- Happy: golden image contains `Hello World.markdown`; `stat("/Hello World.markdown")` returns success with `name == "Hello World.markdown"`.
- Happy: pure-lowercase 8.3 (`readme.txt`) is returned lowercase even though on-disk 11-byte name is `README  TXT` + lowercase-attr.
- Edge: orphan LFN run (LFN slots followed by deleted entry `0xE5`) is silently skipped.
- Edge: checksum mismatch on slot 2 of 3 — entire LFN run discarded, 8.3 name returned instead.
- Edge: deleted entry first byte `0x05` (spec-encoded `0xE5`) translated back correctly.
- Edge: empty directory (first entry `0x00`) returns no entries.
- Edge: directory entry exactly at cluster boundary (LFN run spans two clusters).
- Failure: malformed sequence numbers (`3, 1, 2` instead of `3, 2, 1`) discards the run, falls back to 8.3.
- Integration: `File::open_read("/host/notes.markdown")` succeeds against a vvfat-served host file with that exact lowercase mixed-case name.
- Regression: every existing test in `src/tests/filesystem.rs` still passes (loading `/system.ttf`, `/host/HELLO.TXT`, etc.).

**Verification:** `./test.sh fat_lfn filesystem` passes; manual `ls /host` (once `ls` is wired post-Phase-B, or via a test fixture) shows mixed case.

---

### Phase B — Writable namespace via tmpfs + overlay

#### U3. tmpfs filesystem implementation

**Goal:** An in-RAM filesystem implementing the full `Filesystem` trait, mountable at any path.

**Requirements:** Foundation for U4 (overlay) and U5 (syscall write surface).

**Dependencies:** U2.

**Files:**
- `src/fs/tmpfs/mod.rs` (new)
- `src/fs/tmpfs/node.rs` (new — `enum TmpfsNode { File(Vec<u8>), Dir(BTreeMap<String, Arc<Mutex<TmpfsNode>>>) }`)
- `src/fs/tmpfs/filesystem.rs` (new — `impl Filesystem for Tmpfs`)
- `src/fs/tmpfs/handle.rs` (new — read/write/seek on file nodes, snapshot iter on dirs)
- `src/fs/mod.rs` (modify — add `pub mod tmpfs;`)
- `src/tests/tmpfs.rs` (new)
- `src/tests/mod.rs` (modify — register `("tmpfs", tmpfs::get_tests)`)

**Approach:**
- One root `Arc<Mutex<TmpfsNode::Dir>>` per Tmpfs instance.
- Path resolution walks `/` segments via the BTreeMap.
- File handles carry an `Arc<Mutex<TmpfsNode::File>>` + position. Read/write operate on the inner `Vec<u8>` (extending it on write past end).
- `mkdir`/`unlink`/`rmdir`/`rename` mutate the parent dir's BTreeMap atomically (held mutex).
- No file size limit beyond available heap. Document the 100 MiB kernel heap ceiling in `src/fs/CLAUDE.md`.
- `Filesystem::sync` is a no-op (RAM only).

**Patterns to follow:**
- Use `crate::lib::arc::Arc` everywhere — never `alloc::sync::Arc` (no-std rule per `.claude/rules/no-std.md`).
- Use `alloc::collections::BTreeMap` — never `HashMap` (no-std rule).
- Mirror the `FileHandle` POD shape from `src/fs/filesystem.rs:43` so the high-level `File` wrapper in `src/fs/file_handle.rs` doesn't need a type-specific branch.

**Test scenarios:**
- Happy: `create("/tmp/foo")`, `write("hello")`, `seek(0)`, `read` returns `"hello"`.
- Happy: `mkdir("/var/log")`, `create("/var/log/syslog")`, `read_dir("/var/log")` returns `["syslog"]`.
- Happy: `unlink("/tmp/foo")` followed by `stat("/tmp/foo")` returns `NotFound`.
- Happy: `rename("/a", "/b")` works within and across directories.
- Edge: write past current size extends the file with the new bytes (no zero-fill of any gap; matching POSIX).
- Edge: `mkdir` on an existing path returns `AlreadyExists`.
- Edge: `rmdir` on non-empty directory returns `NotEmpty`.
- Edge: filename with embedded `/` or null byte returns `InvalidPath`.
- Edge: zero-length file (`create` then `close` without writing) is readable, returns 0 bytes, has size 0.
- Failure: `read` on a directory handle returns `IsADirectory`.
- Failure: heap exhaustion during write returns an `IoError` (don't panic).
- Concurrency (single-threaded but: ) two handles to the same file see consistent writes (Mutex-held).

**Verification:** `./test.sh tmpfs` passes; tmpfs mounts at `/scratch` in a manual boot via a temporary test-only `init_filesystems` patch.

---

#### U4. Overlay filesystem (copy-up + whiteouts)

**Goal:** Mount an upper writable FS over a lower read-only FS, with full POSIX-style read/write/delete semantics.

**Requirements:** Foundation for mounting `/` as writable in U6.

**Dependencies:** U3.

**Files:**
- `src/fs/overlay/mod.rs` (new)
- `src/fs/overlay/filesystem.rs` (new — `impl Filesystem for Overlay { upper: Arc<dyn Filesystem>, lower: Arc<dyn Filesystem> }`)
- `src/fs/overlay/whiteout.rs` (new — whiteout markers + opaque-dir markers)
- `src/fs/mod.rs` (modify — add `pub mod overlay;`)
- `src/tests/overlay.rs` (new)
- `src/tests/mod.rs` (modify — register)

**Approach:**
- Reads: try upper first; if `NotFound`, try lower; if upper has a whiteout marker for the path, return `NotFound` even if lower has it.
- Writes / creates: ensure parent dir exists in upper (recursive mkdir copy-up of ancestors), then perform write on upper.
- Open for write on a file that exists only in lower: copy-up entire file from lower into upper, then operate on upper.
- Unlink of a lower-only file: create whiteout marker in upper (no delete on lower — read-only).
- Unlink of a both-layers file: delete from upper, create whiteout (lower would otherwise re-surface).
- rmdir: only allowed on dirs with no merged entries (count children of merged view).
- Whiteout marker = zero-byte file named `.wh.<name>` in upper (sentinel convention; portable across upper FS types). Document the sentinel.
- Opaque dir marker = file named `.wh..wh..opq` in upper directory (skip lower contents when listing).
- readdir: merge upper + lower entries, upper shadows lower by name, skip whiteouts, skip the sentinel names themselves.

**Patterns to follow:**
- The `Arc<dyn Filesystem>` pattern requires `Filesystem` to be object-safe — review `src/fs/filesystem.rs:137` (it already is, since `DirectoryIterator` borrows `&dyn Filesystem`).
- The longest-prefix mount lookup in `src/fs/vfs.rs:82` already returns `&dyn Filesystem`, so the overlay slots into the existing mount machinery without changes.

**Test scenarios:**
- Happy: lower has `/etc/passwd`, overlay reads it transparently.
- Happy: write to `/etc/passwd` triggers copy-up; subsequent read returns new content; lower file untouched (verified by reading the lower FS directly via test helper).
- Happy: `mkdir("/var/log")` (didn't exist on either) succeeds; only upper holds it.
- Happy: `unlink` on lower-only file creates whiteout; subsequent `stat` returns `NotFound`; `readdir` of parent omits it.
- Happy: `unlink` then re-`create` of same name removes whiteout and creates fresh upper file.
- Edge: rmdir of a dir that exists only in lower creates an opaque marker + whiteout (so it disappears from listings); next create of same name fresh-starts.
- Edge: copy-up of a 5 MiB file goes through the existing `read_to_vec` fast path (don't regress page-fault behavior).
- Edge: rename across copy-up boundary (`/etc/foo` → `/var/foo`): copy-up of foo into upper, then mv within upper, then whiteout `/etc/foo`.
- Edge: readdir of `/` merges upper and lower, dedups by name (upper wins), skips `.wh.*` sentinels.
- Failure: copy-up when upper is full (heap exhausted) returns `DiskFull`, no partial state in upper.
- Failure: write to a path whose lower equivalent is a directory and upper doesn't shadow it returns `IsADirectory`.

**Verification:** `./test.sh overlay` passes.

---

#### U5. Userland syscall write surface

**Goal:** All the syscalls userland needs to actually use a writable FS, reached via the existing `resolve_user_path` pipeline so `/bin` and `/etc` rewrites continue to apply.

**Requirements:** End-to-end userland writes once U6 mounts `/` as overlay.

**Dependencies:** U3, U4.

**Files:**
- `src/userland/syscalls.rs` (modify — open the `EROFS` gate at line 1833, add new handler functions, register in the syscall dispatcher)
- `src/fs/filesystem.rs` (modify — add `truncate(&self, handle: &mut FileHandle, size: u64) -> Result<(), FilesystemError>` to the `Filesystem` trait; default impl returns `UnsupportedOperation` so FAT-read-only doesn't need to stub it explicitly)
- `src/fs/file_handle.rs` (review — `File::open_write` / `File::create` at lines 100/117 already exist and call through; verify they correctly propagate trait errors; add `File::truncate(u64)`)
- `src/fs/fs_manager.rs` (modify — add free functions for `mkdir`, `unlink`, `rmdir`, `rename`, `truncate`, `sync` mirroring existing `create_file`/`exists` style)
- `src/tests/filesystem.rs` (modify — add userland-level write round-trip tests routed through the syscall dispatch path)

**Approach:**
- Each new handler follows the pattern of `open_common` at `src/userland/syscalls.rs:1825`: `resolve_user_path` → optional `apply_bin_rewrite` (for syscalls where it applies — `unlink("/bin/ls")` must fail with `EPERM`, not delete the host BusyBox) → call into `crate::fs::` free function → translate `FilesystemError` → POSIX errno.
- Open gate: replace the literal `EROFS` short-circuit at line 1833 with a call to a new `vfs::is_writable(path)` helper that consults the resolving mount's `Filesystem::is_read_only()`. `/host` and `/bin` rewrites stay `EROFS`; `/` (overlay) and `/data` (Phase C) succeed.
- Add `pread64`/`pwrite64` (numbers 17/18 on x86_64 Linux) — use the existing `File::seek` + `File::read`/`write` under a temporary handle position (don't disturb the handle's current position; semantics match POSIX).
- Add `ftruncate`/`truncate` calling a new `Filesystem::truncate(handle, size)` trait method (tmpfs implements; FAT lands in Phase C; overlay copies up then truncates upper).
- Add `fsync`/`fdatasync` calling `Filesystem::sync()` — tmpfs no-op, FAT (Phase C) flushes pending FAT writes.
- Errno mapping: `AlreadyExists` → `EEXIST`, `NotEmpty` → `ENOTEMPTY`, `IsADirectory` → `EISDIR`, `NotADirectory` → `ENOTDIR`, `InvalidPath` → `EINVAL`, `DiskFull` → `ENOSPC`, `ReadOnly` → `EROFS`, `NotFound` → `ENOENT`, `PermissionDenied` → `EACCES`, `IoError` → `EIO`.

**Patterns to follow:**
- `open_common` at `src/userland/syscalls.rs:1825` for the full path-resolution → bin-rewrite → FS call chain.
- `stat_handler` at line 2072 for the bin-virtual short-circuit pattern.
- `READ_MAX_LEN = 4096` cap pattern (line 199) for `write`'s staging-buffer cap.
- The error→errno translation pattern already used by `open_handler`.

**Execution note:** Test-first for the syscall handlers — write the integration test against tmpfs-mounted-at-`/scratch` before implementing each handler, since the wiring (bin rewrite, errno translation, path resolution) is the easy thing to get subtly wrong.

**Test scenarios:**
- Happy: `mkdir("/scratch/foo")` followed by `stat("/scratch/foo")` reports a directory.
- Happy: `creat("/scratch/bar", 0o644)` returns fd, `write(fd, "hi")` returns 2, `close`, re-open for read, `read` returns `"hi"`.
- Happy: `unlink("/scratch/bar")`; subsequent `open` returns `ENOENT`.
- Happy: `rename("/scratch/a", "/scratch/b")` moves the file.
- Happy: `rename("/scratch/a", "/tmp/b")` works across directories within the same mount.
- Happy: `ftruncate(fd, 100)` extends a 10-byte file; subsequent read returns 100 bytes (10 original + 90 zero-padded, matching POSIX).
- Happy: `pwrite(fd, "X", 1, 50)` writes 1 byte at offset 50; original position unaffected.
- Edge: `unlink("/bin/ls")` returns `EPERM` (bin namespace forbids deletes); `/bin/ls` still resolves.
- Edge: `mkdir("/host/foo")` returns `EROFS`; `/host` still readable.
- Edge: `write(fd, ...)` where fd was opened `O_RDONLY` returns `EBADF`.
- Edge: `rename` across mounts (`/scratch/a` → `/data/b` once Phase C lands) returns `EXDEV` (POSIX semantics, not allowed without a copy fallback).
- Edge: `rmdir("/")` returns `EBUSY` or `EPERM`.
- Edge: relative paths via `mkdirat(AT_FDCWD, "foo", 0o755)` resolve against cwd.
- Failure: invalid fd to `ftruncate` returns `EBADF`.
- Integration: `zsh` can write `.zsh_history` (driven by setting `HISTFILE` to a path under tmpfs).
- Regression: existing `open_handler` read-only tests still pass.

**Verification:** `./test.sh filesystem tmpfs overlay` passes; interactive zsh boot can `touch /tmp/foo` and `cat /tmp/foo` shows expected behavior.

---

#### U6. Mount `/` as overlay(upper=tmpfs, lower=boot-FAT) at boot

**Goal:** Userland sees a writable root without changing the on-disk boot image.

**Requirements:** Phase B end-state — full write semantics RAM-backed.

**Dependencies:** U3, U4, U5.

**Files:**
- `src/kernel.rs` (modify — `init_filesystems` around line 184–429: mount FAT-readonly at an internal sentinel path like `__boot`, mount fresh Tmpfs at `__tmpfs`, register Overlay at `/` with `upper=tmpfs` and `lower=boot-FAT`. Or: mount the overlay directly and keep the lower invisible from the mount table.)
- `src/fs/vfs.rs` (modify — `auto_mount` at line 122: extend the FAT-only branch at line 127 with an `Overlay` arm. Bump `MAX_FAT_MOUNTS` if needed, or add a separate slot array for non-FAT mount instances.)
- `src/fs/CLAUDE.md` (modify — document the new mount topology and rationale)
- `src/tests/filesystem.rs` (modify — add a regression test asserting that pre-existing files (`/system.ttf`, `/HELLO.ELF`) still load after the overlay mount, and that a fresh file created in `/` is readable but not present in the lower FAT (test-helper accesses lower directly))

**Approach:**
- VFS gains the ability to hold non-FAT mounts. Cleanest: introduce an `enum MountedFs { Fat(FatFilesystemWrapper<'static>), Tmpfs(Tmpfs), Overlay(Overlay) }` and a uniform slot array.
- `init_filesystems` after FAT detection: instead of registering FAT at `/`, wrap it in an Overlay with a fresh Tmpfs upper, register the Overlay at `/`.
- The vvfat `/host` mount stays untouched — still a direct FAT mount.
- Boot order: detect drives → mount boot-FAT into lower-only slot → create Tmpfs → create Overlay → register at `/`. If any step fails, fall back to the old direct FAT-at-`/` mount with a warning log (graceful degradation).

**Patterns to follow:**
- Static slot arrays per the project's `PARTITION_DEVICES: [Option<...>; 4]` convention (`src/kernel.rs:115`) — keep dynamic allocation out of the mount table.
- Graceful-absence pattern from the original host-mount plan (`docs/plans/2026-05-08-003-feat-host-folder-mount-vvfat-plan.md` D7).

**Test scenarios:**
- Happy: After boot, `File::open_read("/system.ttf")` succeeds (lower passthrough).
- Happy: After boot, `File::create("/test.txt").write(b"hi")`, then `File::open_read("/test.txt").read_to_string()` returns `"hi"`.
- Happy: `File::open_read("/test.txt")` after reboot returns `NotFound` (overlay is RAM-only in Phase B; persistence lands in Phase D).
- Edge: `unlink("/system.ttf")` creates a whiteout in tmpfs; `stat` returns `NotFound`; lower image on disk untouched (verified via lower-FS test helper).
- Edge: re-create `/system.ttf` after whiteout works (whiteout cleared, fresh upper file).
- Edge: `/host` mount still works unchanged (longest-prefix routing still correct).
- Edge: `/bin/<applet>` still resolves (orthogonal to FS layer, but sanity check).
- Regression: every existing filesystem test in `src/tests/filesystem.rs` passes against the overlay-mounted root.

**Verification:** Full `./test.sh` passes; interactive boot: `touch /helloworld && cat /helloworld` round-trips.

---

### Phase C — Real on-disk FAT writes

#### U7. Third disk: writable FAT image at `/data`

**Goal:** A separate physical disk with a freshly-`mkfs`'d FAT32 image, mounted at `/data`. No write code yet on this unit — just plumbing.

**Requirements:** Isolate write bring-up risk from boot disk.

**Dependencies:** U6 (cleanest to add after the overlay mount table refactor).

**Files:**
- `build.sh` (modify — generate `data.img` via `fatfs`-based xtask or `mkfs.fat` if available; add QEMU `-drive file=...,if=ide,index=2` for Secondary Master)
- `xtask/src/main.rs` (modify — add `mkdata` subcommand that creates a 64 MiB FAT32 image with a single partition; reuse `fatfs` crate)
- `src/kernel.rs` (modify — `init_filesystems` probes Secondary Master via the existing IDE pattern, mounts at `/data`, still read-only at this unit)
- `src/drivers/ide.rs` (review — Secondary channel handling already supported per driver structure; verify probe extends to channel 1)
- `src/tests/filesystem.rs` (modify — add presence test for `/data` mount)
- `src/fs/CLAUDE.md` (modify — document the third mount)

**Approach:**
- Mirror the host-mount plan's pattern (D3 from `docs/plans/2026-05-08-003-feat-host-folder-mount-vvfat-plan.md`): graceful absence, log + continue if the third drive isn't present.
- Mount as read-only for this unit. U8 flips to writable once the FAT writer lands.

**Test scenarios:**
- Happy: `/data` appears in the mount table after boot.
- Happy: `File::open_read("/data/seed.txt")` succeeds if `xtask mkdata` seeded a sample file.
- Edge: kernel boots cleanly without Secondary Master drive (graceful absence).

**Verification:** `./build.sh` succeeds, boot logs show `/data` mount, `./test.sh` passes.

---

#### U8. FAT table writes + cluster allocator

**Goal:** Mutate the FAT in place — write entries, allocate free clusters, free chains, maintain FSINFO and the dirty bit.

**Requirements:** Foundation for U9 (directory writes) and U10 (file content writes).

**Dependencies:** U7.

**Files:**
- `src/fs/fat/fat_table.rs` (modify — add `write_entry` for FAT12/16/32, mirror across `num_fats` copies)
- `src/fs/fat/alloc.rs` (new — `ClusterAllocator` with next-fit + FSINFO hint; `find_free_cluster`, `extend_chain`, `free_chain`)
- `src/fs/fat/boot_sector.rs` (modify — extend BPB/EBPB parsing for the dirty-bit byte at offset 0x041 / 0x025, FSINFO sector location)
- `src/fs/fat/fat_filesystem.rs` (modify — set dirty bit on mount-for-write, clear on `sync`)
- `src/fs/fat/filesystem.rs` (modify — internal helpers consume the new allocator)
- `src/tests/fat_write.rs` (new — unit tests for allocator and FAT entry writes)
- `src/tests/mod.rs` (modify — register `("fat_write", fat_write::get_tests)`)

**Approach:**
- `write_entry`: read-modify-write for FAT12 (3-byte pair holds 2 entries), aligned write for FAT16/32. Mirror to every FAT copy (`num_fats`, typically 2). Touch the disk's IDE write path via `PartitionBlockDevice::write_blocks` (already pass-through).
- Cluster allocator state lives in `Arc<Mutex<...>>`: last-allocated cluster hint (seeded from FSINFO on mount-for-write), free-count cache (recomputed on first writer mount, decremented on alloc, incremented on free).
- `find_free_cluster`: start from hint, scan forward, wrap to cluster 2, fail with `DiskFull` if full scan finds none.
- `extend_chain(start, n)`: allocate n free clusters, link them to the chain via `write_entry`, return new tail.
- `free_chain(start)`: walk chain, mark each entry `0x00000000` (free).
- FSINFO update on `sync`: write back `FSI_Free_Count` and `FSI_Nxt_Free`. Honor as hint, never as truth — recompute free count from FAT on mount.
- Dirty bit: clear bit 15 (FAT16) / bit 27 (FAT32) of FAT[1] on first write; restore on `sync`.

**Patterns to follow:**
- IRQ-disabled-window discipline from `docs/solutions/learnings/2026-05-09-multi-mib-user-binary-load.md`: every IDE write call already wraps `InterruptGuard::disable()` per `src/drivers/ide.rs`. New callers chain through `PartitionBlockDevice::write_blocks` which delegates — no extra guarding needed at this layer.
- `read_to_vec` uninit-capacity pattern if any allocator temp buffer is large (FSINFO sector is one block — no concern).
- Don't bump `wait_drq` timeouts as a workaround for any IRQ jitter — the learning explicitly rejects this.

**Test scenarios:**
- Happy: allocate 1 cluster, write to FAT, read back via existing `read_entry` returns the new value.
- Happy: `extend_chain(start, 5)` produces a chain of length 5+previous; cluster IDs returned are all marked allocated; chain is linked correctly (walk via `follow_chain`).
- Happy: `free_chain(start)` after extend leaves all clusters marked free.
- Happy: dirty bit is clear after `mount_for_write`; set again after `sync`.
- Edge: FAT12 entry at odd cluster (high nibble in first byte, low byte in second): read-modify-write preserves the neighbor entry.
- Edge: mirroring writes both FAT copies (verify by reading the second FAT directly via partition layer).
- Edge: full disk — `find_free_cluster` returns `DiskFull` after a full scan.
- Edge: `find_free_cluster` wraps from end to cluster 2 when hint is near end of FAT.
- Edge: FSINFO `FSI_Free_Count` is `0xFFFFFFFF` (uninitialized) on mount — fall back to full scan.
- Failure: write to a cluster beyond disk end returns `IoError`, no partial state.
- Failure: cluster allocator under concurrent allocation (single-threaded kernel, but mutex contention test): two `find_free_cluster` calls return distinct clusters.

**Verification:** `./test.sh fat_write` passes; manual: mount `/data` writable, allocate a cluster via a kernel test command, dismount, mount on host with `fsck.fat` — clean.

---

#### U9. FAT directory entry writes + LFN write

**Goal:** Mutate FAT directory clusters — create, delete, rename, mkdir, with LFN slot generation and short-name alias collision handling.

**Requirements:** Userland `creat`/`mkdir`/`unlink`/`rename` against `/data`.

**Dependencies:** U8.

**Files:**
- `src/fs/fat/directory.rs` (modify — entry mutators: tombstone via `0xE5`, append-or-fill-slot allocation, LFN run writer)
- `src/fs/fat/lfn.rs` (modify — short-name alias generator with `~N` collision suffix; UCS-2 encoder; checksum compute; reverse-order slot emission)
- `src/fs/fat/filesystem.rs` (modify — `create_directory_entry`, `remove_directory_entry`, `rename_entry`, `make_directory`)
- `src/tests/fat_write.rs` (modify)
- `src/fs/CLAUDE.md` (modify — document remaining gaps: root-dir size limit on FAT12/16, no `utimensat`)

**Approach:**
- Short-name alias generation: strip illegal chars (`*?<>|"/\\:`), uppercase, basename → 6 chars + `~1`, ext → 3 chars; scan target directory for collision and bump `~N`; past `~9`, truncate basename further and use `~10`/`~100`. Match the documented Microsoft algorithm.
- LFN slot allocation: count needed slots (`ceil(name_len_in_utf16 / 13)` + 1 for the 8.3 stub), find that many consecutive free entries (tombstone or end-marker), write the run + stub.
- Directory growth: if no consecutive free slots exist, extend the directory chain via `extend_chain` (U8). Root directory on FAT12/16 is fixed-size — fail with `ENOSPC` rather than extend.
- Delete: walk back from the 8.3 stub through preceding LFN slots, tombstone each (`0xE5`).
- mkdir: allocate a free cluster (U8), write `.` and `.` entries into it, then create the directory entry in the parent.
- Rename: equivalent to create + delete, ordered carefully (create in destination first; only delete source if create succeeded; if dest and source are same dir and lengths fit in one slot, in-place is faster but optional).
- Flush ordering for crash safety: FAT writes before directory entry writes (so a crash leaks a cluster, not a cross-link).

**Patterns to follow:**
- Match the slot-discipline of existing read path in `directory.rs` — preserve the `0x00` end marker semantics (never overwrite without clearing the next entry to `0x00`).
- Use existing `IDX 0x05 ↔ 0xE5` translation logic from U2.

**Test scenarios:**
- Happy: create `/data/notes.markdown` (long name, 14 UTF-16 units) → directory contains an LFN run of 2 slots (each slot holds 13 UTF-16 units; 14 units need 2 slots) + 8.3 stub `NOTES~1.MAR`.
- Happy: create `/data/notes.md` (short, pure lowercase) → no LFN slot, lowercase-attr bits set on the 8.3 entry.
- Happy: create `/data/aaaa.txt`, `/data/aaab.txt` — both fit as 8.3, no LFN.
- Happy: create `/data/Long Filename.md` then `/data/Long Filename Two.md` — second one gets `~2` suffix on short-name alias.
- Happy: `mkdir("/data/subdir")`, `create("/data/subdir/x")`, `unlink("/data/subdir/x")`, `rmdir("/data/subdir")` all succeed.
- Happy: `rename("/data/a", "/data/b")` within same dir.
- Edge: unlink of LFN-named file tombstones all LFN slots and the 8.3 stub (verify by reading raw directory bytes).
- Edge: directory entry slot reuse — after unlink, next create fills the tombstoned slots before extending.
- Edge: root dir full on FAT12/16 — create returns `ENOSPC`.
- Edge: name with surrogate pair (emoji) — encoded as two UTF-16 units, decoded back correctly.
- Edge: short-name collision past `~9` (`Documents Backup`, 10 collisions) generates `DOCU~10.` short name.
- Edge: rename across directories within `/data` works.
- Edge: rename across mount points (`/scratch/a` → `/data/a`) returns `EXDEV` (per U5 semantics).
- Failure: invalid filename (embedded `/`) returns `EINVAL`.
- Failure: crash simulation (kernel panic mid-write) — on next mount, dirty bit is set, FAT scan reveals leaked cluster but no cross-link (document the recovery gap; full fsck deferred).
- Integration: full round-trip — userland `touch /data/test.txt`, reboot, `cat /data/test.txt` finds the file.

**Verification:** `./test.sh fat_write` passes; manual: dismount `/data`, mount on host with `fsck.fat -n` — clean.

---

#### U10. Wire FAT writes into the writable mount

**Goal:** Flip `/data` mount from read-only to read-write; userland write syscalls land on disk.

**Requirements:** End-to-end persistent writes on `/data`.

**Dependencies:** U8, U9.

**Files:**
- `src/fs/fat/fat_filesystem.rs` (modify — replace `ReadOnly` short-circuits at lines 152, 237, 250, 254, 258, 262 with real calls into the now-implemented inner methods)
- `src/fs/fat/fat_filesystem.rs` (modify — `is_read_only()` at line 30 returns false for the `/data` mount, true for `/` (lower) and `/host`)
- `src/fs/vfs.rs` (modify — `auto_mount` for the third disk passes a `writable: true` flag when constructing the wrapper)
- `src/kernel.rs` (modify — `/data` mount uses the writable wrapper)
- `src/tests/fat_write.rs` (modify — end-to-end test via the userland-equivalent file_handle API)

**Approach:**
- Construction-time choice: writable vs read-only is a flag on `FatFilesystemWrapper` set at mount time, not per-call.
- `is_read_only()` reflects the flag.
- All trait methods that currently return `ReadOnly` route to the new inner impl when the flag is set, return `ReadOnly` otherwise.

**Test scenarios:**
- Happy: `File::create("/data/foo.txt").write(b"hi")`, dismount, remount, read returns `"hi"`.
- Happy: `File::create("/").write(...)` still goes through the overlay (not direct to lower FAT — verify by reading lower directly).
- Happy: `File::create("/host/foo")` still returns `EROFS`.
- Edge: writing 5 MiB file to `/data` (forces multi-cluster allocation) round-trips correctly.
- Edge: filling `/data` to capacity returns `ENOSPC` cleanly, no partial corruption.
- Edge: power-loss-simulator (panic mid-write) — restart, `/data` mounts dirty, files written before crash are readable, file mid-write may be truncated or absent (document outcomes).
- Integration: zsh redirecting stdout to `/data/log.txt` works; subsequent `cat /data/log.txt` shows the content.

**Verification:** `./test.sh` full pass; manual round-trip across a reboot.

---

### Phase D — Persistence (overlay flush)

#### U11. Overlay sync to disk

**Goal:** A `sync` operation that snapshots the overlay's upper layer to a structured dump on `/data`, restored on next mount.

**Requirements:** Reboot-survivable writes to `/` (currently RAM-only).

**Dependencies:** U10.

**Files:**
- `src/fs/overlay/sync.rs` (new — `flush_upper_to_disk(overlay, target_fs, target_path)`; `restore_upper_from_disk(overlay, source_fs, source_path)`)
- `src/fs/overlay/filesystem.rs` (modify — `Filesystem::sync` invokes flush)
- `src/userland/syscalls.rs` (modify — implement the POSIX `sync` syscall (nr 162) and `syncfs` (nr 306) calling `vfs::sync_all()` / per-mount sync. BusyBox already ships a `sync` applet that issues this syscall, so `/bin/sync` becomes the user-facing entry point with zero kernel-side shell code. Note: the old kernel-side shell was removed by PR #28; the kernel has no command interpreter to register a `sync` command in.)
- `src/kernel.rs` (modify — after `/data` is mounted writable, call `restore_upper_from_disk` for the `/` overlay)
- `src/panic.rs` (modify — optional best-effort sync in panic path, behind a feature flag; the test-build panic handler bypasses it)
- `src/tests/overlay.rs` (modify — flush + restore round-trip test)
- `src/fs/CLAUDE.md` (modify — document the persistence model and its limits)

**Approach:**
- Dump format: a directory tree on `/data` mirroring the upper-layer namespace. Whiteouts represented as `.wh.<name>` files (same sentinel as in-memory). Opaque markers as `.wh..wh..opq`. Plain files copy 1:1.
- `sync`: walk upper tmpfs depth-first, mkdir + create-and-write at the mirror location on `/data`.
- On boot, if `/data/overlay-state/` exists, walk it back into a fresh tmpfs before mounting the overlay.
- This is simple and survivable; cost is a full write-out per sync rather than incremental. Optimize later if it bites.
- Sync is invoked: from userland via `/bin/sync` (BusyBox applet → `sync(2)` → kernel `sync_handler` → `vfs::sync_all()`); on graceful shutdown (future, once orderly shutdown exists); optionally in the panic handler (feature-gated).

**Test scenarios:**
- Happy: write `/etc/foo`, run sync, verify `/data/overlay-state/etc/foo` exists with same content.
- Happy: write `/etc/foo`, run sync, reboot, `/etc/foo` is still readable.
- Happy: unlink lower file `/system.ttf`, sync, reboot — `/system.ttf` still gone (whiteout restored).
- Happy: re-create whiteout-shadowed file, sync, reboot — file persists.
- Edge: sync with empty upper layer creates `/data/overlay-state/` (empty) or no-ops cleanly.
- Edge: sync when `/data` is full returns `ENOSPC`; in-RAM state untouched; next sync after freeing space succeeds.
- Edge: restore from corrupt `/data/overlay-state/` (a file appears where a dir should be) logs and continues with partial restore — does not refuse to boot.
- Failure: sync called when `/data` is unmounted returns error; in-RAM state untouched.
- Integration: full lifecycle — boot, modify root, sync, reboot, verify modifications persisted, modify again, sync, reboot.

**Verification:** `./test.sh overlay` passes; manual reboot-cycle test.

---

## System-Wide Impact

This plan touches:

- **Filesystem layer** — invasive: LFN parser, new tmpfs, new overlay, FAT writes, new third mount.
- **Userland syscall layer** — moderate: ~10 new handlers, one gate flipped. The handlers integrate with the existing `apply_bin_rewrite` two-arm namespace (BusyBox + GLAUNCH) — mutations under `/bin` always return `EPERM`.
- **Block / IDE drivers** — minimal: writes already supported; only verify Secondary Master probe.
- **Boot path (`src/kernel.rs`)** — moderate: new third disk, overlay construction, persistence restore.
- **Build system** — new xtask, modified `build.sh` for image post-process and third disk creation.
- **Documentation** — `src/fs/CLAUDE.md` substantial rewrite; root `CLAUDE.md` "Known Issues" update; new learning post-mortem after Phase C.
- **Testing** — four new test modules (`fat_lfn`, `tmpfs`, `overlay`, `fat_write`).
- **No impact** on graphics, window system, input, mm, process subsystems. `/bin` namespace explicitly untouched.

Affected parties:

- **Userland binaries** — gain `mkdir`/`unlink`/`rmdir`/`rename`/`ftruncate`/`fsync`/`pread`/`pwrite` syscalls and writable `/`. Existing read paths unchanged.
- **Developers using `/host`** — gain mixed-case file visibility; mount stays read-only.
- **Developers building from fresh clone** — pick up the xtask in the cargo workspace; `./build.sh` Just Works as before with one extra step.

---

## Alternative Approaches Considered

**A1. ext2 read+write instead of extending FAT.**
Spec is cleaner, native long lowercase names, no LFN goo, easier writer (variable-length directory records, direct/indirect block pointers — well-trodden in hobby OSes like szhou42/osdev, levex/osdev). Rejected because: block layer + partition layer + VFS already speak FAT fluently, the read fast path (`docs/solutions/learnings/2026-05-09-multi-mib-user-binary-load.md`) is tuned for FAT specifically, and the `/host` vvfat mount inherently requires a FAT reader regardless. Adding ext2 doubles the FS surface for no gain on `/host`. If FAT's structural limits start to bite (no symlinks, no xattrs, no inodes-as-identity), revisit.

**A2. Custom log-structured AgFS.**
Educational, trivial crash recovery via log replay, append-only writes match flash-friendly storage. Rejected as first persistent writer: no host tooling (would need to write our own `mkfs`), format churn during iteration, no existing fsck. Worth doing as v2 once the platform's workload is well-understood.

**A3. Big-bang FAT writes at `/` (no overlay, no third disk).**
Simpler architecturally — one writable mount, one FS implementation in play. Rejected because: a FAT writer bug at `/` bricks the boot image; the boot image is regenerated from `assets/` on every `cargo build` so writes wouldn't persist anyway; the overlay approach lets us ship userland writes (Phase B) months before trusting on-disk writes (Phase C).

**A4. tmpfs-only, no on-disk writes ever.**
Cheapest end state. Rejected because the user explicitly asked for read+write on the filesystems — and "writes that vanish on reboot" doesn't satisfy that. tmpfs+overlay alone is half the story; on-disk writes (Phase C) make it real.

**A5. Replace vvfat with virtio-9p as part of this plan.**
Would solve live mac↔guest editing and obviate the snapshot-at-boot limitation. Rejected as scope: virtio-9p is a substantial new driver (~500–800 LOC kernel-side, plus a new `Filesystem` impl). The user confirmed `/host` stays as-is. Captured in the original host-mount plan's Deferred section.

**A6. Migrate `/bin` to real FS entries.**
Would unify the FS story by making `/bin/<applet>` first-class on-disk entries. Rejected because: no symlink support in FAT (would need 226 copies of BB.ELF), no symlink support in the kernel either, and the current syscall-layer synthesis works perfectly. The user confirmed `/bin` stays orthogonal.

---

## Success Metrics

- **Mixed case end-to-end:** `notes.markdown` on the Mac side appears as `notes.markdown` (not `NOTES~1.MAR`) inside the guest at `/host/notes.markdown`.
- **Userland writes work in RAM (Phase B):** `touch /tmp/foo && cat /tmp/foo` round-trips inside zsh; `.zsh_history` accumulates across the session.
- **Persistent writes (Phase C+D):** A file written to `/data/test.txt` is present after `./build.sh` reboots the VM.
- **No regressions:** Full `./test.sh` passes at every phase boundary. Existing fixture loads (`/system.ttf`, `/HELLO.ELF`, `/host/HELLO.TXT`, `/host/HELLOCPP.ELF`) still work after Phase A's parser change.
- **Image build perf:** xtask repack adds <2s to a full `./build.sh` cycle.
- **No new IRQ-jitter regressions:** Mouse remains smooth during large `/data` writes (per the discipline in `docs/solutions/learnings/2026-05-09-multi-mib-user-binary-load.md`).

---

## Risk Analysis & Mitigation

**R1. FAT write bug corrupts disk image.** Mitigation: writes land at `/data` (third disk), not `/`; `/` writes go through tmpfs+overlay so the boot image stays immutable. Worst case during bring-up: `/data/data.img` needs to be regenerated, no boot impact.

**R2. LFN parser bug breaks existing reads.** Mitigation: Phase A is read-only; the parser falls back to 8.3 names on any validation failure (orphan run, checksum mismatch, malformed sequence). Existing tests provide regression coverage. The `bios.img` for the kernel's own boot files is the same image the parser walks — a regression here breaks boot, which is loud.

**R3. Overlay copy-up of large files (5 MiB ELFs) regresses page-fault behavior.** Mitigation: copy-up uses the existing `read_to_vec` uninit-capacity pattern (per the learning's invariant). Test with `/HELLOCPP.ELF` (5.79 MiB) under the overlay-mounted root.

**R4. IRQ-disabled-window during FAT writes causes mouse jitter or kernel-stack overflows.** Mitigation: writes already use the `InterruptGuard` discipline (per the learning). Chunk multi-cluster operations.

**R5. No crash safety means corruption on power-loss.** Mitigation: dirty bit on mount-for-write so future fsck can detect; flush ordering (FAT before directory) so a crash leaks a cluster rather than cross-linking; documented in `src/fs/CLAUDE.md` and root `CLAUDE.md` "Known Issues".

**R6. Scope creep from "while we're here" cleanups (FAT cluster cache, streaming `DirectoryIterator`, RTC for timestamps).** Mitigation: all listed under Deferred. Resist incorporating into active units.

**R7. `fatfs` crate has a `no_std` compatibility issue or version drift.** Mitigation: pin version in `xtask/Cargo.toml`; xtask runs in std on the host, not in the kernel — so kernel-side has no fatfs dependency. If the kernel-side write path ever wants to share code, that's a separate decision.

**R8. Long-name lookup on the boot path (`init_filesystems` reading `/HELLO.ELF`) slows boot.** Mitigation: LFN parsing is per-directory-walk; root directory entry count is small. Measure boot time before/after Phase A; expect <50ms regression.

**R9. `xtask` adds workspace complexity.** Mitigation: keep xtask single-purpose (image post-process + `mkdata`); document in root README; pattern is widely used in Rust projects.

---

## Phased Delivery

Each phase is independently mergeable, shippable, and reversible.

| Phase | Units | Net delta | Cost (rough) |
|---|---|---|---|
| **A — Read correctness** | U1, U2 | Mixed case + long names visible everywhere | 1–2 days |
| **B — RAM writes** | U3, U4, U5, U6 | Full userland write semantics, RAM-backed | 3–5 days |
| **C — Disk writes** | U7, U8, U9, U10 | `/data` persistent across reboots | 5–10 days |
| **D — Persistence of `/`** | U11 | `/` overlay survives reboots via flush-on-sync | 2–3 days |

Phase boundaries are deliberate sync points — each one ends with a green `./test.sh`, an updated `src/fs/CLAUDE.md`, and a working interactive boot. Phases A and B together (mixed case + RAM writes) are the highest-ROI subset and can ship before deciding on C/D.

---

## Dependencies / Prerequisites

- **`fatfs` Rust crate** — used host-side in xtask. Pin to a known-good version. No kernel-side dependency.
- **`InterruptGuard` discipline** — already enforced in `src/drivers/ide.rs`; new code inherits via `PartitionBlockDevice::write_blocks`.
- **`bin_namespace::apply_bin_rewrite`** — must continue to fire before any new write-side handler in `src/userland/syscalls.rs` so `/bin` paths can't be mutated.
- **`READ_MAX_LEN` and `WRITE_MAX_LEN` (both currently 4096 in `src/userland/syscalls.rs`)** — already enforced for the existing pipe/stdout write path. Phase B's new file-write handlers reuse `WRITE_MAX_LEN` and loop at the handler level for larger writes.

---

## Documentation Plan

- Phase A: `src/fs/CLAUDE.md` — remove "8.3 only" and "uppercase" caveats; document the LFN parser; note the lowercase-attr-bit shortcut.
- Phase B: `src/fs/CLAUDE.md` — add overlay/tmpfs section with the mount topology diagram; document the whiteout sentinel; note the volatility.
- Phase B: `src/userland/syscalls.rs` — keep handler comments minimal; cross-reference into `src/fs/CLAUDE.md` for the FS-layer behavior.
- Phase C: `docs/solutions/learnings/2026-MM-DD-fat-write-bringup.md` — post-mortem covering bring-up surprises (FAT12 packed-byte gotchas, FSINFO drift, dirty-bit handling, any IRQ-jitter findings).
- Phase D: `src/fs/CLAUDE.md` — document the persistence model, the sync command, and the crash-recovery gap.
- Root `CLAUDE.md` — update "Known Issues and Technical Debt": remove "Read-Only Filesystem", remove "8.3 Filenames Only"; add the chkdsk-equivalent and RTC-timestamps deferred items.

---

## Operational / Rollout Notes

- **Per-phase merge cadence:** each phase is one PR (or a small stack within a phase). A green CI + a manual `./build.sh` interactive boot is the merge gate.
- **No feature flags.** The architecture is the rollout — Phase B can't break Phase A because Phase A doesn't depend on it.
- **Backward compat for boot images:** Phase A's xtask post-process is idempotent; older `bios.img` files (without LFN) still work because the parser falls back to 8.3. Forward-compat the other direction (older kernel reading a Phase-A-built image) — older kernel ignores LFN entries (existing `is_lfn` filter), reads only 8.3 stubs, which still exist alongside LFN entries.
- **Disk image regeneration:** `/data` image is generated by `xtask mkdata` at `./build.sh` time. To start fresh, `rm target/data.img && ./build.sh`. Document in README.

---

## Future Considerations

- Move `/bin` namespace synthesis into the FS layer (would require symlink support — itself a worthwhile track).
- Real `fsck` on dirty mount (detect leaked clusters, cross-links; rebuild FAT#1 from FAT#0).
- RTC driver + real POSIX timestamps via `utimensat`.
- virtio-9p as a `/host` replacement for live mac↔guest editing.
- Switch IDE PIO to virtio-blk to obviate `InterruptGuard` windows entirely (per the multi-MiB-load learning).
- Move overlay flush from "snapshot on sync" to a write-through journal so persistence is automatic.

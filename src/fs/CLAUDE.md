# `src/fs/` — Filesystem Layer

Filesystem stack: checked block I/O → VFS → ext2/FAT/tmpfs/overlay with mount-pinned `Arc` file handles.

## Key files

- `filesystem.rs` — generic `Filesystem` trait that concrete filesystems implement.
- `partition.rs` — MBR partition table parsing.
- `vfs.rs` — virtual filesystem layer with mount management and filesystem detection.
- `file_handle.rs` — `Arc`-based `File` and directory handle API.
- `fs_manager.rs` — high-level filesystem operations that the rest of the kernel calls.
- `block_io.rs` — checked byte and filesystem-block I/O over sector devices.
- `p9/` — 9P2000.L client backend for the virtio-9p host share at `/shared` (`protocol.rs` wire codec, `client.rs` serialized fid/ops client, `filesystem.rs` trait impl). No guest-side caching by design: every op is a fresh RPC, which is what makes concurrent multi-instance use of one host directory coherent.
- `ext2/` — writable ext2 parser and allocator, including indirect blocks, directories, hard links, symlinks, sparse files, and Unix metadata.
- `fat/` — FAT12/16/32 implementation. `filesystem.rs` (FAT operations + long-name-aware `walk_directory`), `boot_sector.rs` (BPB parsing), `fat_table.rs` (cluster chain following), `directory.rs` (directory entry parsing — `DirectoryIterator` is the SFN-only low-level primitive), `lfn.rs` (VFAT LFN decoding + lowercase-attr-bit short-name formatting), `types.rs`.

## Architecture (bottom up)

```
Block device (src/drivers/block.rs, ide.rs)
  → MBR partition table (partition.rs)
    → VFS / mount manager (vfs.rs)
      → Concrete filesystem (ext2/, fat/, tmpfs/, overlay/)
        → File handles (file_handle.rs, fs_manager.rs)
```

Block devices and the VirtIO-blk driver live in `src/drivers/` — see `src/drivers/CLAUDE.md`. The `Arc` used in handles is the kernel's custom impl in `src/lib/arc.rs` (NOT `alloc::sync::Arc`) — see `src/lib/CLAUDE.md`.

The global mount table and backing filesystem slots are protected by one
coarse `PreemptionMutex`. Do not hold its guard while calling a filesystem
operation that can re-enter VFS; copy out the selected filesystem handle or
mount metadata first, then drop the guard.

## File handle API

```rust
use crate::fs::File;
use crate::lib::arc::Arc;

let file: Arc<File> = File::open_read("/TEST.TXT")?;
let content = file.read_to_string()?;

let file2 = file.clone();  // Arc-shared, both refer to the same handle
```

Cleanup is automatic when the last `Arc` reference drops.

## Current limitations

- **No subdirectory traversal yet** in some higher-level APIs (FAT subdir reads work via `walk_directory`; some legacy paths still assume single-level).
- **No in-kernel fsck.** Dirty ext2 volumes mount read-only unless the explicit developer override is set. Use `scripts/fsck-data.sh`; repairs are host-side and opt-in.
- **Supported ext profile is deliberately narrow.** ext3 journals and ext4-only features are rejected. The generated image uses `filetype`, `sparse_super`, and `large_file` only.

See `docs/plans/2026-05-16-005-feat-filesystem-write-and-long-names-plan.md` for the full plan.

## Mount topology

```
  /          → overlay(upper = Tmpfs, lower = boot FAT partition)
  /work      → directory on the overlay, provisioned at boot (scratch/compiler output)
  /host      → FAT (vvfat-backed, read-only; /host/sysroot is the TCC musl sysroot)
  /data      → ext2 (writable, `agenticos-data` VirtIO disk, persistent)
  /legacy-data → optional old FAT image (read-only, `agenticos-legacy` VirtIO disk)
  /shared    → 9p host directory (virtio-9p tag `agenticos-shared`, writable,
               worktree-independent, safe for concurrent instances; host default
               ~/.agenticos/shared, AGENTICOS_SHARED_DIR / AGENTICOS_SHARED=off)
  /bin/<applet> → synthesized at syscall layer (src/userland/bin_namespace.rs)
```

The overlay's upper is a fresh `Tmpfs` constructed at boot in `vfs::mount_overlay_root`; the lower is the bootloader-built FAT image. `/bin` is invisible to the FS layer — the syscall dispatcher intercepts opens/access/stat before reaching the VFS. `/work` is not a mount: `src/kernel.rs` mkdirs it idempotently on the overlay after overlay-state hydration.

**Seek past EOF** is allowed on writable filesystems, with the gap zero-filling on the next write; policy lives per-filesystem (`File::seek` just delegates). tmpfs gets it for free (`resize`), ext2 writes sparse gaps natively, and the FAT write path appends explicit zeros from EOF to the write position first, because `write_file_at` links gap clusters while walking to an offset but never writes their data. Read-only FAT mounts (`/host`, overlay-lower handles, `/legacy-data`) keep the `position > size` rejection at the FAT-level seek.

Writes under `/` go through copy-up into tmpfs; persistence happens via the `sync(2)` syscall (BusyBox `sync` applet) which flushes the upper to `/data/overlay-state.{0,1}` double-buffered blob with a 1-byte pointer commit. On boot, `restore_overlay_upper_from_data` validates the active blob's CRC32 and hydrates the upper tmpfs from it.

Copy-up is size-capped at 64 KiB (`overlay::filesystem::MAX_COPY_UP_BYTES`) to bound the heap-burst risk — bigger files surface as `EFBIG`.

Writes directly to `/data/<file>` skip the overlay and go straight to ext2. `fsync`, `fdatasync`, or `sync` checkpoints the filesystem clean bit and flushes the device.

`Filesystem::set_times` is the path-based timestamp mutation hook behind
`utimensat(2)`. `None` preserves a field. Tmpfs retains atime/mtime seconds in
a side map, overlay copies lower nodes up before a legal mutation, and ext2
updates inode atime/mtime plus ctime. Read-only FAT/vvfat mounts are rejected
by VFS policy before reaching the filesystem implementation.

## tmpfs and overlay

- `tmpfs/` — `BTreeMap<String, TmpNode>` directories with `Arc<Mutex<Vec<u8>>>` file bodies. Open handles are anchored in a per-FS side table so `unlink` doesn't drop data out from under live readers (POSIX unlink-while-open).
- `overlay/` — merge upper writable FS over lower read-only FS. Whiteouts are `.wh.<name>` sentinel files; opaque dir markers are `.wh..wh..opq`. Reads check upper first, fall through to lower unless whiteouted. Writes copy-up if needed, then mutate upper.

## Long filenames (VFAT LFN)

As of 2026-05-16 the driver decodes VFAT LFN runs and surfaces full mixed-case names everywhere `enumerate_dir` / `stat` / path lookup runs. `system.ttf` shows up as `system.ttf` (not `SYSTEM.TTF`), `agentic-banner.bmp` is reachable by that name (not `AGENTI~1.BMP`), and `/host/notes.markdown` works when the Mac side has such a file. Short-name fallback uses the lowercase-attr bits (offset 12, 0x08/0x10) so `readme.txt`-style 8.3 names that fit also display lowercase.

The decoder is in `fat/lfn.rs`. Validation is strict (sequence break / checksum mismatch / orphan slot → drop the run, fall back to SFN; matches Linux `fs/fat/dir.c` behavior). LFN _writing_ is not yet implemented — new files (when writes ship) will get short names until the Phase C LFN-write path lands.

Path lookup uses `eq_ignore_ascii_case` against the decoded name, so `/AGENTIC-BANNER.BMP` and `/agentic-banner.bmp` both resolve. Full Unicode case folding is out of scope; non-ASCII characters compare byte-exact.

## Multiple FAT mounts

`auto_mount` (in `vfs.rs`) supports up to `MAX_FAT_MOUNTS` (4) simultaneous FAT filesystems via a static array of wrappers. The root filesystem takes the first slot at boot; the host-folder mount at `/host` (when present) takes the second. Bumping the limit is a one-line change to `MAX_FAT_MOUNTS` plus extending the `[None; 4]` initializer.

## `Filesystem::read` fast path

`<Fat as Filesystem>::read` (in `fat/fat_filesystem.rs`) has a hot-path branch for the common case `position == 0 && buffer.len() >= file.size`: it passes the caller's buffer directly to `FileSystem::read_file`, which walks clusters and copies straight in. No intermediate file-sized allocation, no zero-fill.

Partial reads and short caller buffers use `FileSystem::read_file_at`, which
walks to the cluster containing the requested offset and reads only the
intersecting sectors. This is load-bearing for sparse executable VMAs: a 4 KiB
ELF page fault must not allocate and reconstruct the entire executable.

Why it matters: before the fast path, every `File::read` call on a multi-MiB file allocated AND zero-filled a temp the size of the file (~1414 page faults for a 5.79 MiB binary), then memcpy'd the whole thing into the caller — doubling page-fault and heap pressure. Loading a C++ ELF used to look like a hang for that reason.

## Gotchas

- **Mount point case is byte-exact.** `vfs.rs::find_filesystem` uses `path.starts_with(mount.path)` so `/host/FOO.TXT` works and `/HOST/FOO.TXT` returns NotFound. The mount POINT is byte-exact lowercase; files inside the mount are matched case-insensitively against their decoded long names.
- File names within mounts are case-insensitive (ASCII fold). `/system.ttf` and `/SYSTEM.TTF` both resolve to the same file.
- The FAT cluster-chain follower in `fat_table.rs` does not currently cache; reading a large file walks the FAT each cluster. Acceptable for current sizes; revisit if performance bites.
- VirtIO-blk reads and writes use owned DMA pages and sleep their caller until PCI completion; filesystem code must not retain spin locks across block calls.
- Whole-file FAT reads cache the active FAT sector while walking a cluster chain, then coalesce consecutive clusters into large `read_blocks` calls. This is load-bearing for executable startup: do not regress to one asynchronous request per cluster/FAT entry.
- The internal FAT `FileHandle.name` field is `[u8; 13]` — a legacy fixed slot. Long names are truncated when stored there, but `stat` and `enumerate_dir` use the full long-name path; the field is only authoritative for SFN-bounded callers.

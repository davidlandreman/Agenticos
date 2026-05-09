# `src/fs/` — Filesystem Layer

Read-only filesystem stack: block devices → MBR partition table → VFS → FAT12/16/32 with `Arc`-based file handles.

## Key files

- `filesystem.rs` — generic `Filesystem` trait that concrete filesystems implement.
- `partition.rs` — MBR partition table parsing.
- `vfs.rs` — virtual filesystem layer with mount management and filesystem detection.
- `file_handle.rs` — `Arc`-based `File` and directory handle API.
- `fs_manager.rs` — high-level filesystem operations that the rest of the kernel calls.
- `fat/` — FAT12/16/32 implementation. `filesystem.rs` (FAT operations, 8.3 filenames only), `boot_sector.rs` (BPB parsing), `fat_table.rs` (cluster chain following), `directory.rs` (directory entry parsing), `types.rs`.

## Architecture (bottom up)

```
Block device (src/drivers/block.rs, ide.rs)
  → MBR partition table (partition.rs)
    → VFS / mount manager (vfs.rs)
      → Concrete filesystem (fat/)
        → File handles (file_handle.rs, fs_manager.rs)
```

Block devices and the IDE driver live in `src/drivers/` — see `src/drivers/CLAUDE.md`. The `Arc` used in handles is the kernel's custom impl in `src/lib/arc.rs` (NOT `alloc::sync::Arc`) — see `src/lib/CLAUDE.md`.

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

- **Read-only.** No write support is implemented anywhere in the stack.
- **8.3 filenames only.** No long filename support. This applies to both the bundled BIOS image *and* any host folder mounted via the `/host` development mount — files staged from the Mac side must be uppercase 8.3 (e.g. `HELLO.TXT`, not `hello.txt` or `notes.markdown`) to be visible.
- **FAT only.** No other filesystem implementation.
- **No subdirectory traversal yet** in the higher-level API.

These are scope decisions, not bugs — write support is a future track.

## Multiple FAT mounts

`auto_mount` (in `vfs.rs`) supports up to `MAX_FAT_MOUNTS` (4) simultaneous FAT filesystems via a static array of wrappers. The root filesystem takes the first slot at boot; the host-folder mount at `/host` (when present) takes the second. Bumping the limit is a one-line change to `MAX_FAT_MOUNTS` plus extending the `[None; 4]` initializer.

## `Filesystem::read` fast path

`<Fat as Filesystem>::read` (in `fat/fat_filesystem.rs`) has a hot-path branch for the common case `position == 0 && buffer.len() >= file.size`: it passes the caller's buffer directly to `FileSystem::read_file`, which walks clusters and copies straight in. No intermediate file-sized allocation, no zero-fill.

The fallback path (partial reads or short caller buffers) still allocates a temp buffer the size of the file, reads into it, then memcpy's the requested slice — that's intrinsic to `read_file`'s contract (`buffer.len() >= file.size`). The fallback is fine for small files; it bites on multi-MiB reads, which is what the fast path eliminates.

Why it matters: before the fast path, every `File::read` call on a multi-MiB file allocated AND zero-filled a temp the size of the file (~1414 page faults for a 5.79 MiB binary), then memcpy'd the whole thing into the caller — doubling page-fault and heap pressure. Loading a C++ ELF used to look like a hang for that reason.

## Gotchas

- **Mount point case is byte-exact.** `vfs.rs::find_filesystem` uses `path.starts_with(mount.path)` so `/host/FOO.TXT` works and `/HOST/FOO.TXT` returns NotFound. Files inside the mount are uppercase 8.3 (FAT 8.3 limitation), but the mount POINT is lowercase — keep them straight.
- File paths are uppercase 8.3 (e.g., `/TEST.TXT`, not `/test.txt`).
- The FAT cluster-chain follower in `fat_table.rs` does not currently cache; reading a large file walks the FAT each cluster. Acceptable for current sizes; revisit if performance bites.
- IDE reads are PIO with interrupts disabled per `read_sectors` call — see `src/drivers/CLAUDE.md`. A cluster-walk that issues many `read_sectors` calls is therefore a sequence of small IRQ-disabled windows (one per cluster), not one big window.

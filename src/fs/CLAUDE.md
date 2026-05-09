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

## Gotchas

- File paths are uppercase 8.3 (e.g., `/TEST.TXT`, not `/test.txt`).
- The FAT cluster-chain follower in `fat_table.rs` does not currently cache; reading a large file walks the FAT each cluster. Acceptable for current sizes; revisit if performance bites.

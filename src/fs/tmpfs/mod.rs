//! In-RAM `tmpfs` filesystem.
//!
//! Backed entirely by the kernel heap. Files are `Vec<u8>` wrapped in
//! `Arc<Mutex<…>>`; directories are `BTreeMap<String, TmpNode>` of the
//! same.
//!
//! Open handles are anchored in a per-FS side table keyed by a unique
//! handle id. The `FileHandle.inode` field carries that id, so the
//! POD `Filesystem::FileHandle` shape stays unchanged while still
//! supporting POSIX unlink-while-open semantics: `unlink` drops the
//! directory-tree reference but the open-handle table keeps the
//! `Arc<Vec<u8>>` alive until the last `close` runs.
//!
//! No persistence — every reboot starts with an empty tmpfs.
//! Allocations are bounded only by the 100 MiB kernel heap; a per-FS
//! size cap can be added later if RAM pressure becomes an issue.

pub mod filesystem;

pub use filesystem::Tmpfs;

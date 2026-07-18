pub mod block_io;
pub mod ext2;
pub mod fat;
pub mod file_handle;
pub mod filesystem;
pub mod fs_manager;
pub mod overlay;
pub mod p9;
pub mod partition;
pub mod tmpfs;
pub mod vfs;

#[allow(unused_imports)]
pub use file_handle::{Directory, File};
pub use filesystem::{detect_filesystem, FilesystemType};
pub use partition::{read_partitions, PartitionBlockDevice};

// Convenience functions
pub use fs_manager::exists;
pub use fs_manager::metadata;

pub mod filesystem;
pub mod partition;
pub mod vfs;
pub mod fat;
pub mod fs_manager;
pub mod file_handle;

pub use filesystem::{FilesystemType, detect_filesystem};
pub use partition::{PartitionBlockDevice, read_partitions};
pub use file_handle::{File, Directory};

// Convenience functions
pub use fs_manager::exists;
pub use fs_manager::create_file;
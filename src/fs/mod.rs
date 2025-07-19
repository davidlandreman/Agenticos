pub mod filesystem;
pub mod partition;
pub mod vfs;
pub mod fat;
pub mod fs_manager;
pub mod file_handle;

pub use filesystem::{Filesystem, FilesystemType, FilesystemError, detect_filesystem};
pub use partition::{Partition, PartitionType, PartitionBlockDevice, read_partitions};
pub use vfs::{VirtualFilesystem, get_vfs, auto_mount};
pub use fs_manager::{FileSystemManager, Path, FsError, FsResult};
pub use file_handle::{File, Directory, FileError, FileResult};

// Convenience functions
pub use fs_manager::{read, read_to_string, write, exists, metadata, read_dir};
pub use fs_manager::{with_file, read_with, write_with, read_entire_file, for_each_line};
pub use fs_manager::{create_file, open_file_read, open_file_write, write_string, read_file_to_string};
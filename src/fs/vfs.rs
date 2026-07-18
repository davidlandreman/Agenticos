use crate::arch::x86_64::preemption_guard::{PreemptionMutex, PreemptionMutexGuard};
use crate::drivers::block::BlockDevice;
use crate::fs::fat::FatFilesystem;
use crate::fs::filesystem::{detect_filesystem, Filesystem, FilesystemError, FilesystemType};
use crate::{debug_error, debug_info};

// Static storage for mounted FAT filesystem wrappers. The wrapper owns the
// inner FatFilesystem, so we only need one array. Slot count matches the
// kernel's other static-slot conventions (cf. PARTITION_DEVICES). Bumping
// this is cheap if more than two simultaneous FAT mounts ever land.
const MAX_FAT_MOUNTS: usize = 4;
static mut MOUNTED_FAT_WRAPPERS: [Option<
    crate::fs::fat::fat_filesystem::FatFilesystemWrapper<'static>,
>; MAX_FAT_MOUNTS] = [None, None, None, None];

const MAX_EXT2_MOUNTS: usize = 2;
static mut MOUNTED_EXT2: [Option<crate::fs::ext2::Ext2Filesystem<'static>>; MAX_EXT2_MOUNTS] =
    [None, None];

/// Static slot for the boot-root tmpfs upper layer. There's only one
/// overlay at `/` for now; a second slot is cheap if other writable
/// mounts (e.g. `/tmp` as a separate tmpfs) ever land.
static mut MOUNTED_TMPFS: [Option<crate::fs::tmpfs::Tmpfs>; 2] = [None, None];

/// Static slot for the boot-root overlay. Holds the merged
/// (tmpfs over FAT) view that gets mounted at `/`.
static mut MOUNTED_OVERLAY: [Option<crate::fs::overlay::Overlay>; 2] = [None, None];

/// Static slot for the 9p-backed host share mounted at `/shared`. One
/// device/tag today; widen alongside a second `-fsdev` if one ever lands.
static mut MOUNTED_P9: [Option<crate::fs::p9::P9Filesystem>; 1] = [None];

/// A sentinel block device for filesystems with no underlying disk
/// (tmpfs, overlay). The VFS mount table requires a `&'static dyn
/// BlockDevice`; for non-disk-backed FSes we point at this no-op
/// implementation. Reads/writes return errors so any kernel code that
/// accidentally calls through it surfaces a loud failure rather than
/// silent zero-fill.
pub struct NullBlockDevice;

impl crate::drivers::block::BlockDevice for NullBlockDevice {
    fn read_blocks(&self, _start: u64, _count: u32, _buf: &mut [u8]) -> Result<(), &'static str> {
        Err("NullBlockDevice: no underlying disk")
    }
    fn write_blocks(&self, _start: u64, _count: u32, _buf: &[u8]) -> Result<(), &'static str> {
        Err("NullBlockDevice: no underlying disk")
    }
    fn block_size(&self) -> u32 {
        1
    }
    fn total_blocks(&self) -> u64 {
        0
    }
    fn name(&self) -> &str {
        "null"
    }
}

pub static NULL_BLOCK_DEVICE: NullBlockDevice = NullBlockDevice;

/// Mount point information
#[derive(Clone, Copy)]
pub struct MountPoint {
    pub path: &'static str,
    pub filesystem: &'static dyn Filesystem,
    #[expect(dead_code, reason = "intentional kernel API surface")]
    pub device: &'static dyn BlockDevice,
}

/// Virtual Filesystem Manager
pub struct VirtualFilesystem {
    mounts: [Option<MountPoint>; 16], // Support up to 16 mount points
    mount_count: usize,
}

impl VirtualFilesystem {
    pub const fn new() -> Self {
        Self {
            mounts: [None; 16],
            mount_count: 0,
        }
    }

    /// Mount a filesystem at the given path
    pub fn mount(
        &mut self,
        path: &'static str,
        filesystem: &'static dyn Filesystem,
        device: &'static dyn BlockDevice,
    ) -> Result<(), FilesystemError> {
        if self.mount_count >= self.mounts.len() {
            return Err(FilesystemError::DiskFull); // No more mount points available
        }

        // Check if path is already mounted
        for mount in self.mounts.iter().flatten() {
            if mount.path == path {
                return Err(FilesystemError::AlreadyExists);
            }
        }

        self.mounts[self.mount_count] = Some(MountPoint {
            path,
            filesystem,
            device,
        });
        self.mount_count += 1;

        debug_info!("Mounted {} filesystem at {}", filesystem.name(), path);
        Ok(())
    }

    /// Unmount a filesystem

    /// Find the filesystem for a given path
    pub fn find_filesystem<'a>(&self, path: &'a str) -> Option<(&'static dyn Filesystem, &'a str)> {
        let mut best_match: Option<(&MountPoint, usize)> = None;

        // Find the longest matching mount point
        for mount in self.mounts.iter().flatten() {
            let matches = if mount.path == "/" {
                path.starts_with('/')
            } else {
                path == mount.path
                    || path
                        .strip_prefix(mount.path)
                        .is_some_and(|rest| rest.starts_with('/'))
            };
            if matches {
                let mount_len = mount.path.len();
                if best_match.is_none() || mount_len > best_match.unwrap().1 {
                    best_match = Some((mount, mount_len));
                }
            }
        }

        best_match.map(|(mount, len)| {
            let relative_path = if mount.path == "/" {
                path
            } else if path.len() > len && path.as_bytes()[len] == b'/' {
                &path[len + 1..]
            } else if path.len() == len {
                "/"
            } else {
                &path[len..]
            };
            (mount.filesystem, relative_path)
        })
    }

    /// List all mount points
    pub fn list_mounts(&self) -> impl Iterator<Item = &MountPoint> {
        self.mounts.iter().flatten()
    }
}

/// Global mount table. The backing mount-slot arrays above are populated only
/// during BSP boot while APs remain parked, then become read-only; runtime
/// mount lookup/mutation is serialized here.
static VFS: PreemptionMutex<VirtualFilesystem> = PreemptionMutex::new(VirtualFilesystem::new());

/// Get the global VFS instance
pub fn get_vfs() -> PreemptionMutexGuard<'static, VirtualFilesystem> {
    VFS.lock()
}

/// Auto-mount a block device by detecting its filesystem type
pub fn auto_mount(
    device: &'static dyn BlockDevice,
    mount_path: &'static str,
) -> Result<FilesystemType, FilesystemError> {
    let fs_type = detect_filesystem(device)?;

    debug_info!("Detected filesystem type: {:?}", fs_type);

    match fs_type {
        FilesystemType::Fat12 | FilesystemType::Fat16 | FilesystemType::Fat32 => {
            unsafe {
                // Find the first free slot in the static wrapper array.
                let wrappers_ptr = &raw mut MOUNTED_FAT_WRAPPERS;
                let slot = (0..MAX_FAT_MOUNTS).find(|&i| (*wrappers_ptr)[i].is_none());
                let slot = match slot {
                    Some(i) => i,
                    None => {
                        debug_error!("No free FAT mount slots (max {})", MAX_FAT_MOUNTS);
                        return Err(FilesystemError::DiskFull);
                    }
                };

                // Try to create FAT filesystem with 'static lifetime
                match FatFilesystem::new(device) {
                    Ok(fat_fs) => {
                        // Transmute to 'static lifetime - safe because the device is 'static
                        let fat_fs_static: FatFilesystem<'static> = core::mem::transmute(fat_fs);
                        let wrapper = crate::fs::fat::fat_filesystem::FatFilesystemWrapper::new(
                            fat_fs_static,
                        );
                        (*wrappers_ptr)[slot] = Some(wrapper);

                        // Take a 'static reference to the wrapper now living in the slot.
                        if let Some(wrapper_ref) = (*&raw const MOUNTED_FAT_WRAPPERS)[slot].as_ref()
                        {
                            let mut vfs = get_vfs();
                            match vfs.mount(mount_path, wrapper_ref as &dyn Filesystem, device) {
                                Ok(_) => {
                                    debug_info!(
                                        "Successfully mounted FAT filesystem at {} (slot {})",
                                        mount_path,
                                        slot
                                    );
                                    Ok(fs_type)
                                }
                                Err(e) => {
                                    debug_error!(
                                        "Failed to mount FAT filesystem at {}: {:?}",
                                        mount_path,
                                        e
                                    );
                                    // Free the slot so a later attempt can reuse it.
                                    (*wrappers_ptr)[slot] = None;
                                    Err(e)
                                }
                            }
                        } else {
                            debug_error!("Failed to create wrapper reference");
                            (*wrappers_ptr)[slot] = None;
                            Err(FilesystemError::InvalidFilesystem)
                        }
                    }
                    Err(_) => {
                        debug_error!("Failed to initialize FAT filesystem");
                        Err(FilesystemError::InvalidFilesystem)
                    }
                }
            }
        }
        FilesystemType::Ext2 => mount_ext2(device, mount_path, false, false).map(|_| fs_type),
        FilesystemType::Ext3 | FilesystemType::Ext4 => {
            debug_info!("Ext3/ext4 features are not supported by the ext2 driver");
            Err(FilesystemError::UnsupportedFeature)
        }
        FilesystemType::Ntfs => {
            debug_info!("NTFS filesystem support not yet implemented");
            Err(FilesystemError::UnsupportedOperation)
        }
        FilesystemType::Unknown => {
            debug_error!("Unknown filesystem type");
            Err(FilesystemError::InvalidFilesystem)
        }
    }
}

/// Auto-mount but gate the resulting FAT as writable. Performs the
/// C-2 dirty-bit read-before-set check on the underlying disk; on a
/// dirty-bit failure refuses to mount writable (caller can fall back
/// to a read-only mount).
pub fn auto_mount_writable(
    device: &'static dyn BlockDevice,
    mount_path: &'static str,
    force_dirty_mount: bool,
) -> Result<FilesystemType, FilesystemError> {
    let fs_type = detect_filesystem(device)?;
    if fs_type == FilesystemType::Ext2 {
        mount_ext2(device, mount_path, true, force_dirty_mount)?;
        return Ok(fs_type);
    }
    if !matches!(
        fs_type,
        FilesystemType::Fat12 | FilesystemType::Fat16 | FilesystemType::Fat32
    ) {
        return Err(FilesystemError::UnsupportedFeature);
    }
    unsafe {
        let wrappers_ptr = &raw mut MOUNTED_FAT_WRAPPERS;
        let slot = (0..MAX_FAT_MOUNTS)
            .find(|&i| (*wrappers_ptr)[i].is_none())
            .ok_or(FilesystemError::DiskFull)?;
        let fat_fs = FatFilesystem::new(device).map_err(|_| FilesystemError::InvalidFilesystem)?;
        // C-2 dirty-bit gate. Errors here propagate as ReadOnly so
        // the caller can choose to retry as a normal (read-only)
        // mount with `auto_mount(...)`.
        fat_fs
            .enable_writes(force_dirty_mount)
            .map_err(|_| FilesystemError::ReadOnly)?;
        let fat_fs_static: FatFilesystem<'static> = core::mem::transmute(fat_fs);
        let wrapper =
            crate::fs::fat::fat_filesystem::FatFilesystemWrapper::new_writable(fat_fs_static);
        (*wrappers_ptr)[slot] = Some(wrapper);
        if let Some(wrapper_ref) = (*&raw const MOUNTED_FAT_WRAPPERS)[slot].as_ref() {
            let mut vfs = get_vfs();
            vfs.mount(mount_path, wrapper_ref as &dyn Filesystem, device)?;
            debug_info!(
                "Mounted {} as WRITABLE at {} (slot {})",
                wrapper_ref.name(),
                mount_path,
                slot
            );
            Ok(fs_type)
        } else {
            (*wrappers_ptr)[slot] = None;
            Err(FilesystemError::InvalidFilesystem)
        }
    }
}

fn mount_ext2(
    device: &'static dyn BlockDevice,
    mount_path: &'static str,
    writable: bool,
    force_dirty: bool,
) -> Result<(), FilesystemError> {
    unsafe {
        let slots = &raw mut MOUNTED_EXT2;
        let slot = (0..MAX_EXT2_MOUNTS)
            .find(|&index| (*slots)[index].is_none())
            .ok_or(FilesystemError::DiskFull)?;
        let filesystem = crate::fs::ext2::Ext2Filesystem::new(device, writable, force_dirty)?;
        (*slots)[slot] = Some(filesystem);
        let filesystem_ref = (*&raw const MOUNTED_EXT2)[slot]
            .as_ref()
            .ok_or(FilesystemError::InvalidFilesystem)?;
        if let Err(error) = get_vfs().mount(mount_path, filesystem_ref, device) {
            (*slots)[slot] = None;
            return Err(error);
        }
        debug_info!(
            "Mounted ext2 at {} (slot {}, writable={})",
            mount_path,
            slot,
            writable
        );
        Ok(())
    }
}

/// Mount the boot-root FAT as the LOWER layer of an overlay, with a
/// fresh tmpfs as the UPPER, and register the overlay at `/`. The FAT
/// itself is never publicly mounted — userland sees only the merged
/// view.
///
/// Panics on construction failure rather than silently degrading to a
/// read-only FAT mount (per the per-doc-review #A-5 decision: silent
/// degradation breaks downstream userland that assumes `/` is
/// writable).
pub fn mount_overlay_root(device: &'static dyn BlockDevice) -> Result<(), FilesystemError> {
    use crate::fs::overlay::Overlay;
    use crate::fs::tmpfs::Tmpfs;

    // 1. Construct the FAT lower layer in slot 0.
    let fat_fs = FatFilesystem::new(device).map_err(|_| FilesystemError::InvalidFilesystem)?;
    let fat_fs_static: FatFilesystem<'static> = unsafe { core::mem::transmute(fat_fs) };
    let wrapper = crate::fs::fat::fat_filesystem::FatFilesystemWrapper::new(fat_fs_static);
    unsafe {
        let slot = (0..MAX_FAT_MOUNTS)
            .find(|&i| MOUNTED_FAT_WRAPPERS[i].is_none())
            .ok_or(FilesystemError::DiskFull)?;
        MOUNTED_FAT_WRAPPERS[slot] = Some(wrapper);
    }
    let lower_ref: &'static dyn Filesystem = unsafe {
        MOUNTED_FAT_WRAPPERS[0]
            .as_ref()
            .ok_or(FilesystemError::InvalidFilesystem)? as &dyn Filesystem
    };

    // 2. Construct the tmpfs upper in slot 0.
    unsafe {
        MOUNTED_TMPFS[0] = Some(Tmpfs::new());
    }
    let upper_ref: &'static dyn Filesystem = unsafe {
        MOUNTED_TMPFS[0]
            .as_ref()
            .ok_or(FilesystemError::InvalidFilesystem)? as &dyn Filesystem
    };

    // 3. Construct the overlay in slot 0 and mount at `/`.
    unsafe {
        MOUNTED_OVERLAY[0] = Some(Overlay::new(upper_ref, lower_ref));
    }
    let overlay_ref: &'static dyn Filesystem = unsafe {
        MOUNTED_OVERLAY[0]
            .as_ref()
            .ok_or(FilesystemError::InvalidFilesystem)? as &dyn Filesystem
    };

    let mut vfs = get_vfs();
    vfs.mount("/", overlay_ref, &NULL_BLOCK_DEVICE)?;
    debug_info!("Mounted overlay(tmpfs over FAT) at /");
    Ok(())
}

/// Mount a 9p share (no backing block device) at `mount_path`. The
/// filesystem must arrive with its version/attach handshake already done.
pub fn mount_p9(
    filesystem: crate::fs::p9::P9Filesystem,
    mount_path: &'static str,
) -> Result<(), FilesystemError> {
    unsafe {
        let slot_ptr = &raw mut MOUNTED_P9;
        if (*slot_ptr)[0].is_some() {
            return Err(FilesystemError::AlreadyExists);
        }
        (*slot_ptr)[0] = Some(filesystem);
        let fs_ref = (*&raw const MOUNTED_P9)[0]
            .as_ref()
            .ok_or(FilesystemError::InvalidFilesystem)?;
        let mut vfs = get_vfs();
        vfs.mount(mount_path, fs_ref as &dyn Filesystem, &NULL_BLOCK_DEVICE)
    }
}

/// Convenience functions that operate on the global VFS

#[expect(
    dead_code,
    reason = "legacy convenience API; File pins its mount directly"
)]
pub fn vfs_open(
    path: &str,
    mode: crate::fs::filesystem::FileMode,
) -> Result<crate::fs::filesystem::FileHandle, FilesystemError> {
    let vfs = get_vfs();
    if let Some((fs, rel_path)) = vfs.find_filesystem(path) {
        fs.open(rel_path, mode)
    } else {
        Err(FilesystemError::NotFound)
    }
}

pub fn vfs_read_dir(
    path: &str,
) -> Result<crate::fs::filesystem::DirectoryIterator<'_>, FilesystemError> {
    let vfs = get_vfs();
    if let Some((fs, rel_path)) = vfs.find_filesystem(path) {
        fs.read_dir(rel_path)
    } else {
        Err(FilesystemError::NotFound)
    }
}

pub fn vfs_stat(path: &str) -> Result<crate::fs::filesystem::DirectoryEntry, FilesystemError> {
    let vfs = get_vfs();
    if let Some((fs, rel_path)) = vfs.find_filesystem(path) {
        fs.stat(rel_path)
    } else {
        Err(FilesystemError::NotFound)
    }
}

pub fn vfs_unix_metadata(
    path: &str,
) -> Result<crate::fs::filesystem::UnixMetadata, FilesystemError> {
    let (filesystem, relative) = get_vfs()
        .find_filesystem(path)
        .ok_or(FilesystemError::NotFound)?;
    filesystem.unix_metadata(relative)
}

pub fn vfs_symlink_metadata(
    path: &str,
) -> Result<crate::fs::filesystem::UnixMetadata, FilesystemError> {
    let (filesystem, relative) = get_vfs()
        .find_filesystem(path)
        .ok_or(FilesystemError::NotFound)?;
    filesystem.symlink_metadata(relative)
}

pub fn vfs_set_times(
    path: &str,
    accessed: Option<u64>,
    modified: Option<u64>,
) -> Result<(), FilesystemError> {
    let (filesystem, relative) = get_vfs()
        .find_filesystem(path)
        .ok_or(FilesystemError::NotFound)?;
    if filesystem.is_read_only() {
        return Err(FilesystemError::ReadOnly);
    }
    filesystem.set_times(relative, accessed, modified)
}

pub fn vfs_read_link(path: &str) -> Result<alloc::vec::Vec<u8>, FilesystemError> {
    let (filesystem, relative) = get_vfs()
        .find_filesystem(path)
        .ok_or(FilesystemError::NotFound)?;
    filesystem.read_link(relative)
}

pub fn vfs_symlink(target: &str, link_path: &str) -> Result<(), FilesystemError> {
    let (filesystem, relative) = get_vfs()
        .find_filesystem(link_path)
        .ok_or(FilesystemError::NotFound)?;
    if filesystem.is_read_only() {
        return Err(FilesystemError::ReadOnly);
    }
    filesystem.symlink(target, relative)
}

pub fn vfs_link(old_path: &str, new_path: &str) -> Result<(), FilesystemError> {
    let (old_fs, old_relative) = get_vfs()
        .find_filesystem(old_path)
        .ok_or(FilesystemError::NotFound)?;
    let (new_fs, new_relative) = get_vfs()
        .find_filesystem(new_path)
        .ok_or(FilesystemError::NotFound)?;
    if (old_fs as *const dyn Filesystem as *const ())
        != (new_fs as *const dyn Filesystem as *const ())
    {
        return Err(FilesystemError::UnsupportedOperation);
    }
    if old_fs.is_read_only() {
        return Err(FilesystemError::ReadOnly);
    }
    old_fs.link(old_relative, new_relative)
}

pub fn vfs_mkdir(path: &str) -> Result<(), FilesystemError> {
    let vfs = get_vfs();
    let (fs, rel) = vfs.find_filesystem(path).ok_or(FilesystemError::NotFound)?;
    if fs.is_read_only() {
        return Err(FilesystemError::ReadOnly);
    }
    fs.mkdir(rel)
}

pub fn vfs_unlink(path: &str) -> Result<(), FilesystemError> {
    let vfs = get_vfs();
    let (fs, rel) = vfs.find_filesystem(path).ok_or(FilesystemError::NotFound)?;
    if fs.is_read_only() {
        return Err(FilesystemError::ReadOnly);
    }
    fs.unlink(rel)
}

pub fn vfs_rmdir(path: &str) -> Result<(), FilesystemError> {
    let vfs = get_vfs();
    let (fs, rel) = vfs.find_filesystem(path).ok_or(FilesystemError::NotFound)?;
    if fs.is_read_only() {
        return Err(FilesystemError::ReadOnly);
    }
    fs.rmdir(rel)
}

/// Rename `old_path` to `new_path`. Both paths must resolve to the
/// same mount; cross-mount renames return `ReadOnly` (mapped to EXDEV
/// at the syscall boundary).
pub fn vfs_rename(old_path: &str, new_path: &str) -> Result<(), FilesystemError> {
    let vfs = get_vfs();
    let (fs_old, rel_old) = vfs
        .find_filesystem(old_path)
        .ok_or(FilesystemError::NotFound)?;
    let (fs_new, rel_new) = vfs
        .find_filesystem(new_path)
        .ok_or(FilesystemError::NotFound)?;
    // Use trait-object pointer identity to enforce same-mount.
    if (fs_old as *const dyn Filesystem as *const ())
        != (fs_new as *const dyn Filesystem as *const ())
    {
        return Err(FilesystemError::UnsupportedOperation);
    }
    if fs_old.is_read_only() {
        return Err(FilesystemError::ReadOnly);
    }
    fs_old.rename(rel_old, rel_new)
}

pub fn vfs_sync_all() -> Result<(), FilesystemError> {
    let mut filesystems: [Option<&'static dyn Filesystem>; 16] = [None; 16];
    let count = {
        let vfs = get_vfs();
        let mut count = 0usize;
        for mount in vfs.list_mounts() {
            filesystems[count] = Some(mount.filesystem);
            count += 1;
        }
        count
    };
    let mut last = Ok(());
    for filesystem in filesystems[..count].iter().flatten() {
        if let Err(e) = filesystem.sync() {
            last = Err(e);
        }
    }
    last
}

/// True iff `path` resolves to a writable mount. Caller still has to
/// handle the case where the mount itself rejects a specific
/// operation (e.g., `/bin` namespace shielding).
pub fn vfs_is_writable(path: &str) -> bool {
    let vfs = get_vfs();
    vfs.find_filesystem(path)
        .map(|(fs, _)| !fs.is_read_only())
        .unwrap_or(false)
}

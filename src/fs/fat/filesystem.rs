use crate::drivers::block::BlockDevice;
use crate::fs::fat::boot_sector::BootSector;
use crate::fs::fat::fat_table::FatTable;
use crate::fs::fat::directory::{DirectoryEntry as RawDirEntry, DirectoryIterator, LongFileNameEntry};
use crate::fs::fat::lfn::{
    format_short_name_with_case, LfnAccumulator, MAX_LFN_UTF8,
};
use crate::fs::fat::types::{FatType, FatError, ClusterId};
use crate::debug_info;
use alloc;

pub struct FatFilesystem<'a> {
    device: &'a dyn BlockDevice,
    fat_type: FatType,
    bytes_per_sector: u16,
    sectors_per_cluster: u8,
    first_data_sector: u32,
    root_dir_sectors: u32,
    root_cluster: ClusterId,
}

#[derive(Clone, Copy)]
pub struct FileHandle {
    pub name: [u8; 13],
    pub size: u32,
    pub first_cluster: ClusterId,
    pub is_directory: bool,
}

impl<'a> FatFilesystem<'a> {
    pub fn new(device: &'a dyn BlockDevice) -> Result<Self, FatError> {
        // Read boot sector
        let mut boot_sector_data = [0u8; 512];
        device.read_blocks(0, 1, &mut boot_sector_data)
            .map_err(|_| FatError::BlockDeviceError)?;
            
        let boot_sector = BootSector::from_bytes(&boot_sector_data)?;
        let fat_type = boot_sector.fat_type()?;
        
        let bytes_per_sector = boot_sector.bpb.bytes_per_sector;
        let sectors_per_cluster = boot_sector.bpb.sectors_per_cluster;
        
        debug_info!("FAT filesystem detected: {:?}", fat_type);
        debug_info!("Bytes per sector: {}", bytes_per_sector);
        debug_info!("Sectors per cluster: {}", sectors_per_cluster);
        
        let root_cluster = match fat_type {
            FatType::Fat32 => ClusterId(boot_sector.fat32_ext().root_cluster),
            _ => ClusterId::ROOT_FAT16,
        };
        
        Ok(Self {
            device,
            fat_type,
            bytes_per_sector,
            sectors_per_cluster,
            first_data_sector: boot_sector.first_data_sector(),
            root_dir_sectors: boot_sector.root_dir_sectors(),
            root_cluster,
        })
    }
    
    pub fn fat_type(&self) -> FatType {
        self.fat_type
    }
    
    fn cluster_to_sector(&self, cluster: ClusterId) -> u32 {
        ((cluster.0 - 2) * self.sectors_per_cluster as u32) + self.first_data_sector
    }
    
    fn read_cluster(&self, cluster: ClusterId, buffer: &mut [u8]) -> Result<(), FatError> {
        let sector = self.cluster_to_sector(cluster);
        self.device.read_blocks(sector as u64, self.sectors_per_cluster as u32, buffer)
            .map_err(|_| FatError::BlockDeviceError)?;
            
        Ok(())
    }
    
    pub fn read_file(&self, file: &FileHandle, buffer: &mut [u8]) -> Result<usize, FatError> {
        if file.is_directory {
            return Err(FatError::InvalidPath);
        }
        
        if buffer.len() < file.size as usize {
            return Err(FatError::BufferTooSmall);
        }
        
        // Handle empty files
        if file.size == 0 || file.first_cluster.0 == 0 {
            return Ok(0);
        }
        
        let cluster_size = self.sectors_per_cluster as usize * self.bytes_per_sector as usize;
        let mut bytes_read = 0;
        
        // Read boot sector to create FAT table
        let mut boot_sector_data = [0u8; 512];
        self.device.read_blocks(0, 1, &mut boot_sector_data)
            .map_err(|_| FatError::BlockDeviceError)?;
        let boot_sector = BootSector::from_bytes(&boot_sector_data)?;
        
        let fat_table = FatTable::new(self.device, boot_sector, self.fat_type);
        
        // Allocate a temporary buffer for reading full clusters
        let mut cluster_buffer = alloc::vec![0u8; cluster_size];
        
        fat_table.follow_chain(file.first_cluster, |cluster| {
            let bytes_to_read = core::cmp::min(cluster_size, file.size as usize - bytes_read);
            
            if bytes_to_read > 0 {
                // Read the full cluster into our temporary buffer
                self.read_cluster(cluster, &mut cluster_buffer)?;
                
                // Copy only the bytes we need into the output buffer
                buffer[bytes_read..bytes_read + bytes_to_read]
                    .copy_from_slice(&cluster_buffer[..bytes_to_read]);
                    
                bytes_read += bytes_to_read;
            }
            
            Ok(())
        })?;
        
        Ok(file.size as usize)
    }
    
    /// Walk an absolute path, descending into subdirectories component by
    /// component. Each intermediate component must resolve to a directory;
    /// the final component may be a file or a directory.
    ///
    /// `find_file("/")` returns a synthetic directory `FileHandle` for the
    /// root — callers that need its entries should call `list_directory`
    /// with `None` as the cluster.
    ///
    /// Uses the long-name-aware directory walker, so paths like
    /// `/system.ttf` or `/notes.markdown` resolve correctly regardless of
    /// whether the on-disk entry is 8.3 with case bits or has a full
    /// VFAT LFN run.
    pub fn find_file(&self, path: &str) -> Result<FileHandle, FatError> {
        let trimmed = path.trim_start_matches('/');
        if trimmed.is_empty() {
            return Ok(FileHandle {
                name: *b"/\0\0\0\0\0\0\0\0\0\0\0\0",
                size: 0,
                first_cluster: ClusterId(0),
                is_directory: true,
            });
        }

        let components: alloc::vec::Vec<&str> = trimmed
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();
        if components.is_empty() {
            return Err(FatError::NotFound);
        }

        let last = components.len() - 1;
        let mut current_cluster: Option<ClusterId> = None; // None = root
        for (idx, component) in components.iter().enumerate() {
            let mut found: Option<(ClusterId, u32, bool, [u8; 13])> = None;
            self.walk_directory(current_cluster, |name, raw, first_cluster, is_dir| {
                if name.eq_ignore_ascii_case(component) {
                    // Capture a short-form name for the FileHandle name
                    // field (legacy callers use it; long names just get
                    // truncated to 12 + null). The actual long name is
                    // surfaced through other paths (enumerate/stat).
                    let name_bytes = name.as_bytes();
                    let copy_len = name_bytes.len().min(12);
                    let mut name_arr = [0u8; 13];
                    name_arr[..copy_len].copy_from_slice(&name_bytes[..copy_len]);
                    found = Some((first_cluster, raw.file_size, is_dir, name_arr));
                    true // stop iteration
                } else {
                    false
                }
            })?;

            let (first_cluster, size, is_dir, name_arr) = found.ok_or(FatError::NotFound)?;
            if idx == last {
                return Ok(FileHandle {
                    name: name_arr,
                    size,
                    first_cluster,
                    is_directory: is_dir,
                });
            }
            if !is_dir {
                return Err(FatError::NotFound);
            }
            current_cluster = Some(first_cluster);
        }

        Err(FatError::NotFound)
    }

    /// Like `find_file` but also returns the decoded long name. Used
    /// by `stat` so the public `DirectoryEntry.name` carries the full
    /// VFAT name rather than a 12-byte truncation.
    ///
    /// Returns `(FileHandle, name_buf, name_len)` where the name bytes
    /// are the decoded UTF-8 (up to `MAX_LFN_UTF8`). For root, returns
    /// a synthetic handle with empty name.
    pub fn find_file_with_long_name(
        &self,
        path: &str,
    ) -> Result<(FileHandle, [u8; MAX_LFN_UTF8], usize), FatError> {
        let trimmed = path.trim_start_matches('/');
        if trimmed.is_empty() {
            let mut name = [0u8; MAX_LFN_UTF8];
            name[0] = b'/';
            return Ok((
                FileHandle {
                    name: *b"/\0\0\0\0\0\0\0\0\0\0\0\0",
                    size: 0,
                    first_cluster: ClusterId(0),
                    is_directory: true,
                },
                name,
                1,
            ));
        }

        let components: alloc::vec::Vec<&str> = trimmed
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();
        if components.is_empty() {
            return Err(FatError::NotFound);
        }

        let last = components.len() - 1;
        let mut current_cluster: Option<ClusterId> = None;
        let mut long_name = [0u8; MAX_LFN_UTF8];
        let mut long_len: usize = 0;

        for (idx, component) in components.iter().enumerate() {
            let mut found: Option<(ClusterId, u32, bool)> = None;
            let mut tmp_name = [0u8; MAX_LFN_UTF8];
            let mut tmp_len: usize = 0;
            self.walk_directory(current_cluster, |name, raw, first_cluster, is_dir| {
                if name.eq_ignore_ascii_case(component) {
                    let nb = name.as_bytes();
                    let copy = nb.len().min(MAX_LFN_UTF8);
                    tmp_name[..copy].copy_from_slice(&nb[..copy]);
                    tmp_len = copy;
                    found = Some((first_cluster, raw.file_size, is_dir));
                    true
                } else {
                    false
                }
            })?;
            let (first_cluster, size, is_dir) = found.ok_or(FatError::NotFound)?;

            if idx == last {
                long_name = tmp_name;
                long_len = tmp_len;
                let mut name_arr = [0u8; 13];
                let copy = tmp_len.min(12);
                name_arr[..copy].copy_from_slice(&tmp_name[..copy]);
                return Ok((
                    FileHandle {
                        name: name_arr,
                        size,
                        first_cluster,
                        is_directory: is_dir,
                    },
                    long_name,
                    long_len,
                ));
            }
            if !is_dir {
                return Err(FatError::NotFound);
            }
            current_cluster = Some(first_cluster);
        }

        Err(FatError::NotFound)
    }

    /// List the entries of a directory identified by either the root
    /// (`None`) or a starting cluster. Used by `enumerate_path` and the
    /// userland `getdents64` syscall.
    pub fn list_directory(
        &self,
        cluster: Option<ClusterId>,
        entries: &mut [FileHandle],
        max: usize,
    ) -> Result<usize, FatError> {
        match cluster {
            None => self.list_root_array(entries, max),
            Some(c) => self.read_directory_array(c, entries, max),
        }
    }

    /// Walk every valid (non-deleted, non-volume-label) entry in the
    /// directory at `cluster` (`None` = root for FAT12/16). Each
    /// callback invocation receives the decoded name (VFAT LFN run
    /// when present and valid; otherwise the 8.3 short name with
    /// lowercase-attr bits honored), the raw 32-byte entry, and the
    /// first cluster of the entry's data.
    ///
    /// `cb` may return `true` to stop iteration early (e.g., once a
    /// lookup has found its target).
    ///
    /// LFN runs that fail validation (orphan slots, checksum mismatch,
    /// sequence break) are silently discarded and the trailing 8.3
    /// stub is surfaced via its short-name fallback — matching how
    /// Linux's `fs/fat/dir.c` handles corruption.
    pub fn walk_directory(
        &self,
        cluster: Option<ClusterId>,
        mut cb: impl FnMut(&str, &RawDirEntry, ClusterId, bool) -> bool,
    ) -> Result<(), FatError> {
        let mut acc = LfnAccumulator::new();
        let mut name_buf = [0u8; MAX_LFN_UTF8];
        let mut short_buf = [0u8; 13];
        let mut stop = false;

        let mut process_block = |acc: &mut LfnAccumulator,
                                 cb: &mut dyn FnMut(&str, &RawDirEntry, ClusterId, bool) -> bool,
                                 stop: &mut bool,
                                 block: &[u8]|
         -> Result<bool, FatError> {
            // Walk 32-byte entries in this block, maintaining LFN state
            // across them. Returns Ok(true) on hitting the end-of-dir
            // marker.
            let mut off = 0;
            while off + RawDirEntry::SIZE <= block.len() {
                let entry_bytes = &block[off..off + RawDirEntry::SIZE];
                off += RawDirEntry::SIZE;

                let entry = RawDirEntry::from_bytes(entry_bytes)?;

                if entry.is_end() {
                    return Ok(true);
                }
                if entry.is_free() {
                    acc.reset();
                    continue;
                }

                let attrs = entry.attributes();
                if attrs.is_lfn() {
                    if let Ok(lfn) = LongFileNameEntry::from_bytes(entry_bytes) {
                        acc.push_slot(lfn);
                    }
                    continue;
                }

                // Volume-label entries are skipped (they carry no name
                // we want to surface and are not files).
                if attrs.is_volume_id() {
                    acc.reset();
                    continue;
                }

                // SFN stub — try the LFN decode first; on any failure
                // fall back to the SFN with case-bit awareness.
                let (name_bytes_used, name_len) = if !acc.is_empty() {
                    match acc.decode(entry, &mut name_buf) {
                        Some(n) => (true, n),
                        None => {
                            let sn = format_short_name_with_case(entry, &mut short_buf);
                            (false, sn)
                        }
                    }
                } else {
                    let sn = format_short_name_with_case(entry, &mut short_buf);
                    (false, sn)
                };
                acc.reset();

                let name_slice = if name_bytes_used {
                    &name_buf[..name_len]
                } else {
                    &short_buf[..name_len]
                };
                let name_str = match core::str::from_utf8(name_slice) {
                    Ok(s) => s,
                    Err(_) => continue, // skip un-decodable
                };

                let is_dir = attrs.is_directory();
                if cb(name_str, entry, entry.first_cluster(), is_dir) {
                    *stop = true;
                    return Ok(false);
                }
            }
            Ok(false)
        };

        match (self.fat_type, cluster) {
            (FatType::Fat12, None) | (FatType::Fat16, None) => {
                // Root directory is a fixed run of sectors before the
                // data area.
                let root_start_sector = self.first_data_sector - self.root_dir_sectors;
                let mut block = [0u8; 512];
                for i in 0..self.root_dir_sectors {
                    self.device
                        .read_blocks((root_start_sector + i) as u64, 1, &mut block)
                        .map_err(|_| FatError::BlockDeviceError)?;
                    let end = process_block(&mut acc, &mut cb, &mut stop, &block)?;
                    if stop || end {
                        return Ok(());
                    }
                }
            }
            (_, Some(start)) => {
                // Cluster chain — works for FAT12/16 subdirectories AND
                // FAT32 root (where root_cluster lives in the data area).
                let cluster_size =
                    self.sectors_per_cluster as usize * self.bytes_per_sector as usize;
                let mut buf = alloc::vec![0u8; cluster_size];

                let mut boot_sector_data = [0u8; 512];
                self.device
                    .read_blocks(0, 1, &mut boot_sector_data)
                    .map_err(|_| FatError::BlockDeviceError)?;
                let boot_sector = BootSector::from_bytes(&boot_sector_data)?;
                let fat_table = FatTable::new(self.device, boot_sector, self.fat_type);

                let mut end_hit = false;
                fat_table.follow_chain(start, |c| {
                    if stop || end_hit {
                        return Ok(());
                    }
                    self.read_cluster(c, &mut buf)?;
                    let end = process_block(&mut acc, &mut cb, &mut stop, &buf)?;
                    if end {
                        end_hit = true;
                    }
                    Ok(())
                })?;
            }
            (FatType::Fat32, None) => {
                // FAT32 root: walk from the recorded root_cluster.
                let root = self.root_cluster;
                return self.walk_directory(Some(root), cb);
            }
        }
        Ok(())
    }

    /// Variant of `find_file` that walks all path components except the
    /// last and returns the cluster where the final component would
    /// live, plus the final-component filename. Useful when the caller
    /// wants to list a directory by its path (`enumerate_path`) without
    /// a separate find→list sequence.
    pub fn resolve_directory(&self, path: &str) -> Result<Option<ClusterId>, FatError> {
        let fh = self.find_file(path)?;
        if !fh.is_directory {
            return Err(FatError::NotFound);
        }
        // Root is a sentinel: `/`-handle has cluster 0 but means root.
        let trimmed = path.trim_start_matches('/');
        if trimmed.is_empty() {
            Ok(None)
        } else {
            Ok(Some(fh.first_cluster))
        }
    }
}

// For now, we use a simpler approach with static arrays
impl FatFilesystem<'_> {
    pub fn list_root_array(&self, entries: &mut [FileHandle], max_entries: usize) -> Result<usize, FatError> {
        let mut count = 0;
        
        match self.fat_type {
            FatType::Fat12 | FatType::Fat16 => {
                let root_start_sector = self.first_data_sector - self.root_dir_sectors;
                let mut buffer = [0u8; 512];
                
                'outer: for i in 0..self.root_dir_sectors {
                    self.device.read_blocks((root_start_sector + i) as u64, 1, &mut buffer)
                        .map_err(|_| FatError::BlockDeviceError)?;
                        
                    for entry in DirectoryIterator::new(&buffer) {
                        if let Ok(dir_entry) = entry {
                            if count >= max_entries {
                                break 'outer;
                            }
                            
                            entries[count] = FileHandle {
                                name: dir_entry.format_name(),
                                size: dir_entry.file_size,
                                first_cluster: dir_entry.first_cluster(),
                                is_directory: dir_entry.attributes().is_directory(),
                            };
                            count += 1;
                        }
                    }
                }
            }
            FatType::Fat32 => {
                count = self.read_directory_array(self.root_cluster, entries, max_entries)?;
            }
        }
        
        Ok(count)
    }
    
    fn read_directory_array(&self, start_cluster: ClusterId, entries: &mut [FileHandle], max_entries: usize) -> Result<usize, FatError> {
        let cluster_size = self.sectors_per_cluster as usize * self.bytes_per_sector as usize;
        let mut buffer = [0u8; 8192]; // Fixed size buffer for cluster data
        let mut count = 0;
        
        // Read boot sector to create FAT table
        let mut boot_sector_data = [0u8; 512];
        self.device.read_blocks(0, 1, &mut boot_sector_data)
            .map_err(|_| FatError::BlockDeviceError)?;
        let boot_sector = BootSector::from_bytes(&boot_sector_data)?;
        
        let fat_table = FatTable::new(self.device, boot_sector, self.fat_type);
        
        fat_table.follow_chain(start_cluster, |cluster| {
            if count >= max_entries {
                return Ok(());
            }
            
            self.read_cluster(cluster, &mut buffer[..cluster_size])?;
            
            for entry in DirectoryIterator::new(&buffer[..cluster_size]) {
                if let Ok(dir_entry) = entry {
                    if count >= max_entries {
                        break;
                    }
                    
                    entries[count] = FileHandle {
                        name: dir_entry.format_name(),
                        size: dir_entry.file_size,
                        first_cluster: dir_entry.first_cluster(),
                        is_directory: dir_entry.attributes().is_directory(),
                    };
                    count += 1;
                }
            }
            
            Ok(())
        })?;
        
        Ok(count)
    }
}
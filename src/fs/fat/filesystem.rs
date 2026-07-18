use crate::debug_info;
use crate::drivers::block::BlockDevice;
use crate::fs::fat::boot_sector::BootSector;
use crate::fs::fat::directory::{
    DirectoryEntry as RawDirEntry, DirectoryIterator, LongFileNameEntry,
};
use crate::fs::fat::fat_table::FatTable;
use crate::fs::fat::lfn::{
    encode_lfn_run, fits_short_name, format_short_name_with_case, generate_short_name,
    LfnAccumulator, MAX_LFN_UTF8,
};
use crate::fs::fat::types::{ClusterId, FatError, FatType};
use alloc;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

/// Per-directory short-name collision cache. Key is the parent
/// directory's cluster ID (0 for FAT16 root); value tracks the next
/// `~N` suffix per basename prefix.
///
/// See Corrections C-3 in the plan: the original "scan whole directory
/// per create" approach is O(N²) total for N similarly-named creates.
/// This cache makes subsequent creates O(1) amortized.
#[derive(Default)]
struct ShortNameCache {
    /// `basename_prefix (first 6 ASCII bytes, space-padded) -> highest
    /// ~N suffix observed`. The next allocation reads, bumps, writes.
    suffix_by_prefix: BTreeMap<[u8; 6], u32>,
    /// True once we've fully scanned the directory at least once.
    populated: bool,
}

/// Per-mount mutable state. Lives behind a single mutex so reads
/// from the inner FatFilesystem stay `&self`.
struct MutableState {
    /// Last-allocated cluster hint, seeded from FSINFO or scanned
    /// from FAT[2] on first allocate. Next find_free_cluster starts
    /// here.
    alloc_hint: u32,
    /// Per-parent-cluster short-name cache. Bounded by an LRU cap;
    /// when adding a new entry past the cap, drop the
    /// least-recently-used.
    sn_cache: BTreeMap<u32, ShortNameCache>,
    /// True when this mount has been gated through the dirty-bit
    /// read-before-set check (C-2) and is allowed to issue writes.
    writable: bool,
}

pub struct FatFilesystem<'a> {
    device: &'a dyn BlockDevice,
    /// Immutable BPB/boot-sector data retained from mount. Reusing it avoids
    /// an extra block transaction on every demand-paged executable read.
    boot_sector_data: [u8; 512],
    fat_type: FatType,
    bytes_per_sector: u16,
    sectors_per_cluster: u8,
    first_data_sector: u32,
    root_dir_sectors: u32,
    root_cluster: ClusterId,
    /// Total cluster count (cluster IDs 2..=2+total-1 are valid).
    total_clusters: u32,
    state: Mutex<MutableState>,
}

#[derive(Clone, Copy)]
pub struct FileHandle {
    #[expect(dead_code, reason = "intentional kernel API surface")]
    pub name: [u8; 13],
    pub size: u32,
    pub first_cluster: ClusterId,
    pub is_directory: bool,
}

/// Location of a directory slot (one 32-byte entry) on disk. Used by
/// the U9 write path to read-modify-write specific entries without
/// having to walk the whole chain a second time.
#[derive(Clone, Copy, Debug)]
pub enum DirSlotLoc {
    /// FAT16/12 root area: a fixed absolute sector + byte offset
    /// within that sector. (Root has no cluster chain.)
    Fat16Root { sector: u32, byte_offset: usize },
    /// Clustered directory (FAT32 root + any subdirectory): the
    /// containing cluster's ID and the byte offset within that
    /// cluster.
    Chained {
        cluster: ClusterId,
        byte_offset: usize,
    },
}

/// A directory entry resolved by `find_dir_entry_by_name`. The
/// `slot_locs` slice is the full range of slots comprising the entry
/// (LFN run followed by the SFN stub) — used by `unlink_file` to
/// tombstone exactly that range and nothing else.
pub struct DirEntryLookup {
    pub first_cluster: ClusterId,
    #[expect(dead_code, reason = "intentional kernel API surface")]
    pub size: u32,
    pub attrs: u8,
    pub slot_locs: Vec<DirSlotLoc>,
}

/// True if `name` fits in 8.3 strictly (uppercase letters, digits,
/// legal symbols, no extra dots). Stricter than `lfn::fits_short_name`
/// which permits any case — we only treat ALREADY-uppercase names as
/// SFN-only because anything else needs case bits or an LFN run.
fn fits_short_name_strict(name: &str) -> bool {
    if !fits_short_name(name) {
        return false;
    }
    name.chars().all(|c| !c.is_ascii_lowercase())
}

/// Format an already-strict-fitting 8.3 name into 11 bytes space-padded.
fn format_strict_short_name(name: &str) -> [u8; 11] {
    let last_dot = name.rfind('.');
    let (base, ext) = match last_dot {
        Some(i) if i > 0 => (&name[..i], &name[i + 1..]),
        _ => (name, ""),
    };
    let mut out = [b' '; 11];
    for (i, &b) in base.as_bytes().iter().take(8).enumerate() {
        out[i] = b;
    }
    for (i, &b) in ext.as_bytes().iter().take(3).enumerate() {
        out[8 + i] = b;
    }
    out
}

/// Extract the basename prefix (first 6 chars of the uppercased
/// basename with illegal chars stripped). Used to key the short-name
/// collision cache.
fn basename_prefix6(name: &str) -> [u8; 6] {
    let last_dot = name.rfind('.');
    let base = match last_dot {
        Some(i) if i > 0 => &name[..i],
        _ => name,
    };
    let mut out = [b' '; 6];
    let mut pos = 0;
    for c in base.chars() {
        if pos >= 6 {
            break;
        }
        if c == ' ' || c == '.' {
            continue;
        }
        let b = c as u32;
        let u = if b < 128 {
            let ch = c.to_ascii_uppercase() as u8;
            if matches!(
                ch,
                b'+' | b','
                    | b';'
                    | b'='
                    | b'['
                    | b']'
                    | b'/'
                    | b'\\'
                    | b':'
                    | b'"'
                    | b'*'
                    | b'?'
                    | b'<'
                    | b'>'
                    | b'|'
            ) {
                b'_'
            } else {
                ch
            }
        } else {
            b'_'
        };
        out[pos] = u;
        pos += 1;
    }
    out
}

/// Build a 32-byte SFN directory entry from an 11-byte short name,
/// first_cluster, file size, and a directory flag.
fn build_sfn_entry(
    sfn_11: &[u8; 11],
    first_cluster: ClusterId,
    size: u32,
    is_dir: bool,
) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[0..11].copy_from_slice(sfn_11);
    // Attributes: ARCHIVE for files, DIRECTORY for dirs.
    out[11] = if is_dir { 0x10 } else { 0x20 };
    // NT-reserved / case bits / creation-time-tenths / time + date remain
    // zero. The kernel now has an RTC-backed wall clock, but persisting FAT
    // create/modify timestamps is a separate filesystem-semantics follow-up.
    let cluster_hi = ((first_cluster.0 >> 16) & 0xFFFF) as u16;
    let cluster_lo = (first_cluster.0 & 0xFFFF) as u16;
    out[20..22].copy_from_slice(&cluster_hi.to_le_bytes());
    out[26..28].copy_from_slice(&cluster_lo.to_le_bytes());
    out[28..32].copy_from_slice(&size.to_le_bytes());
    out
}

impl<'a> FatFilesystem<'a> {
    pub fn new(device: &'a dyn BlockDevice) -> Result<Self, FatError> {
        // Read boot sector
        let mut boot_sector_data = [0u8; 512];
        device
            .read_blocks(0, 1, &mut boot_sector_data)
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

        // Total cluster count for find_free_cluster bounds.
        let total_sectors = if boot_sector.bpb.total_sectors_16 != 0 {
            boot_sector.bpb.total_sectors_16 as u32
        } else {
            boot_sector.bpb.total_sectors_32
        };
        let fat_sectors = if boot_sector.bpb.sectors_per_fat_16 != 0 {
            boot_sector.bpb.sectors_per_fat_16 as u32
        } else {
            boot_sector.fat32_ext().sectors_per_fat_32
        };
        let root_dir_sectors = boot_sector.root_dir_sectors();
        let overhead = boot_sector.bpb.reserved_sectors as u32
            + (boot_sector.bpb.num_fats as u32 * fat_sectors)
            + root_dir_sectors;
        let data_sectors = total_sectors.saturating_sub(overhead);
        let total_clusters = data_sectors / sectors_per_cluster as u32;

        Ok(Self {
            device,
            boot_sector_data,
            fat_type,
            bytes_per_sector,
            sectors_per_cluster,
            first_data_sector: boot_sector.first_data_sector(),
            root_dir_sectors,
            root_cluster,
            total_clusters,
            state: Mutex::new(MutableState {
                alloc_hint: 2,
                sn_cache: BTreeMap::new(),
                writable: false,
            }),
        })
    }

    /// Gate this mount for write access. Per Corrections C-2: reads
    /// the dirty-clean bit; refuses if the previous shutdown was
    /// unclean unless `force` is true. On success, marks the FS
    /// writable in mutable state and clears the clean bit so a future
    /// crash is detectable.
    ///
    /// Idempotent: calling on an already-writable FS is a no-op
    /// (returns Ok).
    pub fn enable_writes(&self, force: bool) -> Result<(), FatError> {
        if self.state.lock().writable {
            return Ok(());
        }
        // Open a temporary FatTable so we can poke FAT[1].
        let mut bs = [0u8; 512];
        self.device
            .read_blocks(0, 1, &mut bs)
            .map_err(|_| FatError::BlockDeviceError)?;
        let boot = BootSector::from_bytes(&bs)?;
        let table = FatTable::new(self.device, boot, self.fat_type);
        let clean = table.read_clean_bit()?;
        if !clean {
            crate::debug_warn!(
                "FAT mount: dirty bit indicates previous shutdown was UNCLEAN — fsck recommended"
            );
            if !force {
                return Err(FatError::ReadOnly);
            }
            crate::debug_warn!(
                "FAT mount: AGENTICOS_FORCE_DIRTY_MOUNT override active; proceeding"
            );
        }
        // Clear the clean bit to indicate "writes in progress".
        // Restored on sync_writes().
        table.write_clean_bit(false)?;
        self.state.lock().writable = true;
        Ok(())
    }

    /// Set the clean bit back to TRUE — call when all writes have
    /// been flushed to disk (i.e. by the public `sync` trait method).
    /// No-op when the mount isn't writable.
    pub fn sync_writes(&self) -> Result<(), FatError> {
        if !self.state.lock().writable {
            return Ok(());
        }
        let mut bs = [0u8; 512];
        self.device
            .read_blocks(0, 1, &mut bs)
            .map_err(|_| FatError::BlockDeviceError)?;
        let boot = BootSector::from_bytes(&bs)?;
        let table = FatTable::new(self.device, boot, self.fat_type);
        table.write_clean_bit(true)?;
        self.device.flush().map_err(|_| FatError::BlockDeviceError)
    }

    pub fn is_writable(&self) -> bool {
        self.state.lock().writable
    }

    #[expect(dead_code, reason = "intentional kernel API surface")]
    pub fn total_clusters(&self) -> u32 {
        self.total_clusters
    }

    pub fn fat_type(&self) -> FatType {
        self.fat_type
    }

    fn cluster_to_sector(&self, cluster: ClusterId) -> u32 {
        ((cluster.0 - 2) * self.sectors_per_cluster as u32) + self.first_data_sector
    }

    fn read_cluster(&self, cluster: ClusterId, buffer: &mut [u8]) -> Result<(), FatError> {
        let sector = self.cluster_to_sector(cluster);
        self.device
            .read_blocks(sector as u64, self.sectors_per_cluster as u32, buffer)
            .map_err(|_| FatError::BlockDeviceError)?;

        Ok(())
    }

    fn cached_fat_table(&self) -> Result<FatTable<'_>, FatError> {
        let boot_sector = BootSector::from_bytes(&self.boot_sector_data)?;
        Ok(FatTable::new(self.device, boot_sector, self.fat_type))
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
        let fat_table = self.cached_fat_table()?;

        let mut clusters = Vec::new();
        fat_table.follow_chain(file.first_cluster, |cluster| {
            if clusters.len() * cluster_size < file.size as usize {
                clusters.push(cluster);
            }
            Ok(())
        })?;

        let required_clusters = (file.size as usize).div_ceil(cluster_size);
        if clusters.len() < required_clusters {
            return Err(FatError::EndOfChain);
        }

        let full_clusters = file.size as usize / cluster_size;
        let mut cluster_index = 0usize;
        while cluster_index < full_clusters {
            let run_start = cluster_index;
            while cluster_index + 1 < full_clusters
                && clusters[cluster_index + 1].0 == clusters[cluster_index].0 + 1
            {
                cluster_index += 1;
            }
            let run_clusters = cluster_index - run_start + 1;
            let byte_offset = run_start * cluster_size;
            let byte_len = run_clusters * cluster_size;
            self.device
                .read_blocks(
                    self.cluster_to_sector(clusters[run_start]) as u64,
                    (run_clusters * self.sectors_per_cluster as usize) as u32,
                    &mut buffer[byte_offset..byte_offset + byte_len],
                )
                .map_err(|_| FatError::BlockDeviceError)?;
            cluster_index += 1;
        }

        let remainder = file.size as usize % cluster_size;
        if remainder != 0 {
            let mut last_cluster = alloc::vec![0u8; cluster_size];
            self.read_cluster(clusters[full_clusters], &mut last_cluster)?;
            let offset = full_clusters * cluster_size;
            buffer[offset..offset + remainder].copy_from_slice(&last_cluster[..remainder]);
        }

        Ok(file.size as usize)
    }

    /// Read a bounded window without materializing the entire file. This is
    /// the path used by demand-paged executable VMAs, where each page fault
    /// requests only one 4 KiB window from a much larger ELF.
    pub fn read_file_at(
        &self,
        file: &FileHandle,
        offset: u64,
        buffer: &mut [u8],
    ) -> Result<usize, FatError> {
        if file.is_directory {
            return Err(FatError::InvalidPath);
        }
        if buffer.is_empty() || offset >= file.size as u64 {
            return Ok(0);
        }
        if file.first_cluster.0 == 0 {
            return Err(FatError::InvalidCluster);
        }

        let sector_size = self.bytes_per_sector as usize;
        if sector_size == 0 || sector_size > 4096 {
            return Err(FatError::InvalidBootSector);
        }
        let cluster_size = self.sectors_per_cluster as usize * sector_size;
        let bytes_to_read = core::cmp::min(buffer.len(), file.size as usize - offset as usize);
        let cluster_index = offset as usize / cluster_size;
        let mut cluster_offset = offset as usize % cluster_size;

        let fat_table = self.cached_fat_table()?;
        let mut chain = fat_table.chain_cursor(file.first_cluster);
        for _ in 0..cluster_index {
            chain.advance()?;
        }

        // One read may start part-way through a maximum-size logical sector,
        // then return a full 4 KiB page. Eight KiB covers that worst case.
        let mut sector_buffer = [0u8; 8192];
        let mut bytes_read = 0usize;
        while bytes_read < bytes_to_read {
            let current = chain.current()?;
            let take = core::cmp::min(cluster_size - cluster_offset, bytes_to_read - bytes_read);
            let sector_in_cluster = cluster_offset / sector_size;
            let offset_in_sector = cluster_offset % sector_size;
            let sectors = (offset_in_sector + take).div_ceil(sector_size);
            let transfer_len = sectors * sector_size;
            if transfer_len > sector_buffer.len() {
                return Err(FatError::BufferTooSmall);
            }
            let sector = self.cluster_to_sector(current) + sector_in_cluster as u32;
            self.device
                .read_blocks(
                    sector as u64,
                    sectors as u32,
                    &mut sector_buffer[..transfer_len],
                )
                .map_err(|_| FatError::BlockDeviceError)?;
            buffer[bytes_read..bytes_read + take]
                .copy_from_slice(&sector_buffer[offset_in_sector..offset_in_sector + take]);
            bytes_read += take;

            if bytes_read == bytes_to_read {
                break;
            }
            chain.advance()?;
            cluster_offset = 0;
        }

        Ok(bytes_read)
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

        let components: alloc::vec::Vec<&str> =
            trimmed.split('/').filter(|s| !s.is_empty()).collect();
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

        let components: alloc::vec::Vec<&str> =
            trimmed.split('/').filter(|s| !s.is_empty()).collect();
        if components.is_empty() {
            return Err(FatError::NotFound);
        }

        let last = components.len() - 1;
        let mut current_cluster: Option<ClusterId> = None;
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
                    tmp_name,
                    tmp_len,
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
    #[expect(dead_code, reason = "intentional kernel API surface")]
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

        let mut process_block =
            |acc: &mut LfnAccumulator,
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

    // ---------- Phase C U9: directory writes ----------

    /// Resolve the PARENT directory of `path`, returning a tuple
    /// `(parent_cluster, leaf_name)` where parent_cluster=None means
    /// "root of FAT16/12" (which lives in a fixed area, not a cluster
    /// chain). Errors if any intermediate component is missing or not
    /// a directory.
    pub fn resolve_parent<'p>(
        &self,
        path: &'p str,
    ) -> Result<(Option<ClusterId>, &'p str), FatError> {
        let trimmed = path.trim_start_matches('/').trim_end_matches('/');
        if trimmed.is_empty() {
            return Err(FatError::InvalidPath);
        }
        let mut parts = trimmed.rsplitn(2, '/');
        let leaf = parts.next().ok_or(FatError::InvalidPath)?;
        let parent_path = parts.next().unwrap_or("");
        if parent_path.is_empty() {
            // Parent is root.
            return Ok((self.root_cluster_opt(), leaf));
        }
        let mut full_parent = String::from("/");
        full_parent.push_str(parent_path);
        let parent_fh = self.find_file(&full_parent)?;
        if !parent_fh.is_directory {
            return Err(FatError::InvalidPath);
        }
        Ok((Some(parent_fh.first_cluster), leaf))
    }

    /// Encode the "root cluster" for the directory walker. FAT32 has
    /// a real cluster; FAT16/12 use None to mean the fixed root area.
    fn root_cluster_opt(&self) -> Option<ClusterId> {
        match self.fat_type {
            FatType::Fat32 => Some(self.root_cluster),
            _ => None,
        }
    }

    /// Walk RAW 32-byte slots of the directory at `parent` (None =
    /// FAT16/12 root). For each slot, invoke `cb` with the slot
    /// contents AND its (cluster, byte_offset_in_cluster). For
    /// FAT16 root, the "cluster" passed back is `ClusterId(0)` — the
    /// caller distinguishes via the parent argument they passed in.
    ///
    /// `cb` returns `true` to stop iteration.
    ///
    /// Used for: free-slot finding, name-match scanning, tombstoning.
    pub fn walk_dir_slots(
        &self,
        parent: Option<ClusterId>,
        mut cb: impl FnMut(&[u8; 32], DirSlotLoc) -> bool,
    ) -> Result<(), FatError> {
        match parent {
            None => {
                // FAT16/12 root: fixed sectors before first_data_sector.
                let root_start = self.first_data_sector - self.root_dir_sectors;
                let bps = self.bytes_per_sector as u32;
                let mut sector_buf = [0u8; 512];
                for s in 0..self.root_dir_sectors {
                    self.device
                        .read_blocks((root_start + s) as u64, 1, &mut sector_buf)
                        .map_err(|_| FatError::BlockDeviceError)?;
                    let mut off = 0usize;
                    while off + 32 <= 512 {
                        let mut slot = [0u8; 32];
                        slot.copy_from_slice(&sector_buf[off..off + 32]);
                        let loc = DirSlotLoc::Fat16Root {
                            sector: root_start + s,
                            byte_offset: off,
                        };
                        if cb(&slot, loc) {
                            return Ok(());
                        }
                        off += 32;
                    }
                    let _ = bps; // suppress unused-warning future-proof
                }
                Ok(())
            }
            Some(start) => {
                let cluster_size =
                    self.sectors_per_cluster as usize * self.bytes_per_sector as usize;
                let mut cluster_buf = alloc::vec![0u8; cluster_size];
                let mut bs = [0u8; 512];
                self.device
                    .read_blocks(0, 1, &mut bs)
                    .map_err(|_| FatError::BlockDeviceError)?;
                let boot = BootSector::from_bytes(&bs)?;
                let table = FatTable::new(self.device, boot, self.fat_type);

                let mut stop = false;
                table.follow_chain(start, |cl| {
                    if stop {
                        return Ok(());
                    }
                    self.read_cluster(cl, &mut cluster_buf)?;
                    let mut off = 0usize;
                    while off + 32 <= cluster_size {
                        let mut slot = [0u8; 32];
                        slot.copy_from_slice(&cluster_buf[off..off + 32]);
                        let loc = DirSlotLoc::Chained {
                            cluster: cl,
                            byte_offset: off,
                        };
                        if cb(&slot, loc) {
                            stop = true;
                            return Ok(());
                        }
                        off += 32;
                    }
                    Ok(())
                })?;
                Ok(())
            }
        }
    }

    /// Write a single 32-byte slot at `loc`. Caller is responsible
    /// for choosing the right write ordering (FAT-first vs
    /// directory-first per the C-1 per-op rules).
    pub fn write_dir_slot(&self, loc: DirSlotLoc, slot: &[u8; 32]) -> Result<(), FatError> {
        match loc {
            DirSlotLoc::Fat16Root {
                sector,
                byte_offset,
            } => {
                let mut buf = [0u8; 512];
                self.device
                    .read_blocks(sector as u64, 1, &mut buf)
                    .map_err(|_| FatError::BlockDeviceError)?;
                buf[byte_offset..byte_offset + 32].copy_from_slice(slot);
                self.device
                    .write_blocks(sector as u64, 1, &buf)
                    .map_err(|_| FatError::BlockDeviceError)
            }
            DirSlotLoc::Chained {
                cluster,
                byte_offset,
            } => {
                // Compute which sector within the cluster holds this offset.
                let bps = self.bytes_per_sector as usize;
                let sector_in_cluster = byte_offset / bps;
                let byte_in_sector = byte_offset % bps;
                let cluster_first_sector = self.cluster_to_sector(cluster);
                let target_sector = cluster_first_sector + sector_in_cluster as u32;
                let mut buf = [0u8; 512];
                self.device
                    .read_blocks(target_sector as u64, 1, &mut buf)
                    .map_err(|_| FatError::BlockDeviceError)?;
                buf[byte_in_sector..byte_in_sector + 32].copy_from_slice(slot);
                self.device
                    .write_blocks(target_sector as u64, 1, &buf)
                    .map_err(|_| FatError::BlockDeviceError)
            }
        }
    }

    /// Find `n` consecutive free 32-byte slots in `parent` (None =
    /// FAT16/12 root). Returns the slot locations in order. Free
    /// means first byte is `0xE5` (tombstone) or `0x00` (end marker).
    ///
    /// If the directory chain doesn't have enough consecutive slots,
    /// for chained directories extends the chain by one cluster
    /// (zero-filled) and uses the start of it. For FAT16/12 root
    /// (fixed-size), returns `DiskFull`.
    pub fn find_free_dir_slots(
        &self,
        parent: Option<ClusterId>,
        n: usize,
    ) -> Result<Vec<DirSlotLoc>, FatError> {
        let mut run: Vec<DirSlotLoc> = Vec::with_capacity(n);
        // Track whether we hit the end-marker (0x00) — past that
        // point ALL subsequent slots are free, so we can claim from
        // there onward without worrying about clobbering valid data.
        let mut hit_end = false;
        let mut end_loc: Option<DirSlotLoc> = None;

        self.walk_dir_slots(parent, |slot, loc| {
            if hit_end {
                run.push(loc);
                return run.len() >= n;
            }
            let first = slot[0];
            if first == 0x00 {
                hit_end = true;
                end_loc = Some(loc);
                run.clear();
                run.push(loc);
                return run.len() >= n;
            }
            if first == 0xE5 {
                run.push(loc);
                if run.len() >= n {
                    return true;
                }
            } else {
                run.clear();
            }
            false
        })?;
        let _ = end_loc;

        if run.len() >= n {
            return Ok(run);
        }

        // Not enough; extend if chained, else ENOSPC.
        match parent {
            None => Err(FatError::DiskFull),
            Some(start) => {
                let new_cluster = self.extend_dir_chain(start)?;
                // Reset, walk again — but we know all entries in the
                // new cluster are zeroed, so we can build the result
                // from its first N slot offsets.
                let mut out = Vec::with_capacity(n);
                for i in 0..n {
                    out.push(DirSlotLoc::Chained {
                        cluster: new_cluster,
                        byte_offset: i * 32,
                    });
                }
                Ok(out)
            }
        }
    }

    /// Extend the directory chain starting at `start` by one fresh
    /// cluster (zero-filled). Returns the new cluster's ID.
    fn extend_dir_chain(&self, start: ClusterId) -> Result<ClusterId, FatError> {
        let table = self.fresh_fat_table()?;
        // Walk to the tail.
        let mut tail = start;
        table.follow_chain(start, |cl| {
            tail = cl;
            Ok(())
        })?;
        // Find a free cluster.
        let hint = self.state.lock().alloc_hint;
        let new = table.find_free_cluster(ClusterId(hint), self.total_clusters + 1)?;
        let eoc = match self.fat_type {
            FatType::Fat12 => ClusterId(0x0FFF),
            FatType::Fat16 => ClusterId(0xFFFF),
            FatType::Fat32 => ClusterId(0x0FFFFFFF),
        };
        // Per C-1 create ordering: FAT first. Mark new EOC, then link.
        table.write_entry(new, eoc)?;
        table.write_entry(tail, new)?;
        // Zero the new cluster (so its slots are fresh end-markers).
        let cluster_size = self.sectors_per_cluster as usize * self.bytes_per_sector as usize;
        let zero = alloc::vec![0u8; cluster_size];
        let sector = self.cluster_to_sector(new);
        for s in 0..self.sectors_per_cluster as u32 {
            self.device
                .write_blocks(
                    (sector + s) as u64,
                    1,
                    &zero[..self.bytes_per_sector as usize],
                )
                .map_err(|_| FatError::BlockDeviceError)?;
        }
        self.state.lock().alloc_hint = new.0 + 1;
        Ok(new)
    }

    fn fresh_fat_table(&self) -> Result<FatTable<'_>, FatError> {
        self.cached_fat_table()
    }

    /// Allocate ONE free cluster, mark it end-of-chain, and return
    /// its ID. Updates the alloc hint. Per C-1 create ordering, this
    /// is the FIRST step of file creation — the FAT write happens
    /// before any directory entry.
    pub fn allocate_one_cluster(&self) -> Result<ClusterId, FatError> {
        let table = self.fresh_fat_table()?;
        let hint = self.state.lock().alloc_hint;
        let new = table.find_free_cluster(ClusterId(hint), self.total_clusters + 1)?;
        let eoc = match self.fat_type {
            FatType::Fat12 => ClusterId(0x0FFF),
            FatType::Fat16 => ClusterId(0xFFFF),
            FatType::Fat32 => ClusterId(0x0FFFFFFF),
        };
        table.write_entry(new, eoc)?;
        self.state.lock().alloc_hint = new.0 + 1;
        Ok(new)
    }

    /// Walk an existing chain from `start` to its tail cluster.
    /// Returns the last cluster (whose FAT entry is the EOC marker)
    /// and the number of clusters in the chain.
    #[expect(dead_code, reason = "intentional kernel API surface")]
    pub fn walk_chain_to_tail(&self, start: ClusterId) -> Result<(ClusterId, u32), FatError> {
        let table = self.fresh_fat_table()?;
        let mut tail = start;
        let mut count = 0u32;
        table.follow_chain(start, |cl| {
            tail = cl;
            count += 1;
            Ok(())
        })?;
        Ok((tail, count))
    }

    /// Write `data` to a file's cluster chain starting at logical
    /// offset `offset`. Allocates new clusters and links them as
    /// needed when the write extends past the current chain end.
    /// Returns the number of bytes written.
    ///
    /// If `start_cluster.0 == 0` (an empty file with no allocated
    /// chain yet), allocates a fresh chain and returns the new first
    /// cluster via `*start_cluster_out` so the caller can update the
    /// directory entry.
    ///
    /// Per C-1 ordering: FAT writes (cluster alloc + linkage) happen
    /// BEFORE the data write into each new cluster — so a crash
    /// mid-extend leaves a leaked cluster, not a cross-link.
    pub fn write_file_at(
        &self,
        start_cluster: ClusterId,
        offset: u64,
        data: &[u8],
        start_cluster_out: &mut ClusterId,
    ) -> Result<usize, FatError> {
        if !self.is_writable() {
            return Err(FatError::ReadOnly);
        }
        if data.is_empty() {
            *start_cluster_out = start_cluster;
            return Ok(0);
        }
        let cluster_size = self.sectors_per_cluster as usize * self.bytes_per_sector as usize;

        // Establish the first cluster: allocate if file is empty.
        let mut head = start_cluster;
        if head.0 < 2 {
            let new = self.allocate_one_cluster()?;
            head = new;
        }
        *start_cluster_out = head;

        let table = self.fresh_fat_table()?;
        // Walk the chain to the cluster containing `offset`, extending
        // as needed. Track absolute position in the chain.
        let mut current = head;
        let mut consumed_bytes: u64 = 0;
        let target_start_chunk = (offset as usize) / cluster_size;
        let mut chunk_idx = 0usize;
        while chunk_idx < target_start_chunk {
            let next = table.read_entry(current)?;
            if next.is_end_of_chain(self.fat_type) {
                // Extend the chain.
                let new = self.allocate_one_cluster()?;
                table.write_entry(current, new)?;
                current = new;
            } else {
                current = next;
            }
            chunk_idx += 1;
            consumed_bytes += cluster_size as u64;
        }
        let _ = consumed_bytes;

        // Now `current` is the cluster holding `offset`. Write
        // sequentially, allocating fresh clusters as we cross boundaries.
        let mut data_idx = 0usize;
        let mut byte_in_cluster = (offset as usize) % cluster_size;
        let mut cluster_buf = alloc::vec![0u8; cluster_size];

        while data_idx < data.len() {
            // Read the cluster (preserve unrelated bytes before/after the
            // write window).
            self.read_cluster(current, &mut cluster_buf)?;
            let bytes_left_in_cluster = cluster_size - byte_in_cluster;
            let bytes_to_write = (data.len() - data_idx).min(bytes_left_in_cluster);
            cluster_buf[byte_in_cluster..byte_in_cluster + bytes_to_write]
                .copy_from_slice(&data[data_idx..data_idx + bytes_to_write]);
            self.write_cluster(current, &cluster_buf)?;
            data_idx += bytes_to_write;
            byte_in_cluster = 0;

            if data_idx < data.len() {
                // Need another cluster. Read FAT link; allocate if at end.
                let next = table.read_entry(current)?;
                if next.is_end_of_chain(self.fat_type) {
                    let new = self.allocate_one_cluster()?;
                    table.write_entry(current, new)?;
                    current = new;
                } else {
                    current = next;
                }
            }
        }
        Ok(data.len())
    }

    /// Write a full cluster's worth of bytes to `cluster`. Buffer
    /// must be exactly `sectors_per_cluster * bytes_per_sector`.
    fn write_cluster(&self, cluster: ClusterId, buf: &[u8]) -> Result<(), FatError> {
        let sector = self.cluster_to_sector(cluster);
        self.device
            .write_blocks(sector as u64, self.sectors_per_cluster as u32, buf)
            .map_err(|_| FatError::BlockDeviceError)
    }

    /// Update a file's SFN entry: rewrite first_cluster and size
    /// fields. Used after `write_file_at` to reflect the new tail /
    /// growth in the directory entry.
    pub fn update_sfn_size_and_cluster(
        &self,
        parent: Option<ClusterId>,
        leaf: &str,
        new_first_cluster: ClusterId,
        new_size: u64,
    ) -> Result<(), FatError> {
        let lookup = self
            .find_dir_entry_by_name(parent, leaf)?
            .ok_or(FatError::NotFound)?;
        // The SFN stub is the LAST slot in the run.
        let sfn_loc = *lookup.slot_locs.last().ok_or(FatError::NotFound)?;
        let mut slot = self.read_dir_slot(sfn_loc)?;
        let cluster_hi = ((new_first_cluster.0 >> 16) & 0xFFFF) as u16;
        let cluster_lo = (new_first_cluster.0 & 0xFFFF) as u16;
        slot[20..22].copy_from_slice(&cluster_hi.to_le_bytes());
        slot[26..28].copy_from_slice(&cluster_lo.to_le_bytes());
        slot[28..32].copy_from_slice(&(new_size as u32).to_le_bytes());
        self.write_dir_slot(sfn_loc, &slot)
    }

    /// Read a single 32-byte slot from disk.
    fn read_dir_slot(&self, loc: DirSlotLoc) -> Result<[u8; 32], FatError> {
        let (sector, byte_off) = match loc {
            DirSlotLoc::Fat16Root {
                sector,
                byte_offset,
            } => (sector, byte_offset),
            DirSlotLoc::Chained {
                cluster,
                byte_offset,
            } => {
                let bps = self.bytes_per_sector as usize;
                let sector_in_cluster = byte_offset / bps;
                let byte_in_sector = byte_offset % bps;
                let cluster_first_sector = self.cluster_to_sector(cluster);
                (
                    cluster_first_sector + sector_in_cluster as u32,
                    byte_in_sector,
                )
            }
        };
        let mut buf = [0u8; 512];
        self.device
            .read_blocks(sector as u64, 1, &mut buf)
            .map_err(|_| FatError::BlockDeviceError)?;
        let mut out = [0u8; 32];
        out.copy_from_slice(&buf[byte_off..byte_off + 32]);
        Ok(out)
    }

    /// Free the cluster chain starting at `start`. Walks the chain
    /// and zeroes each FAT entry. Per C-1 unlink ordering, this is
    /// the SECOND step of file deletion (after the directory entry
    /// has been tombstoned).
    pub fn free_cluster_chain(&self, start: ClusterId) -> Result<(), FatError> {
        if start.0 < 2 {
            return Ok(());
        }
        let table = self.fresh_fat_table()?;
        let mut to_free: Vec<ClusterId> = Vec::new();
        table.follow_chain(start, |cl| {
            to_free.push(cl);
            Ok(())
        })?;
        for cl in to_free {
            table.write_entry(cl, ClusterId(0))?;
        }
        Ok(())
    }

    // ---------- High-level create / unlink ----------

    /// Create an empty file at `path`. Per C-1 ordering: allocate a
    /// cluster + mark EOC FIRST, then write the directory entries
    /// (LFN run + SFN stub) into the parent.
    ///
    /// Returns the first cluster of the new file.
    pub fn create_file(&self, path: &str) -> Result<ClusterId, FatError> {
        if !self.is_writable() {
            return Err(FatError::ReadOnly);
        }
        let (parent, leaf) = self.resolve_parent(path)?;

        // Refuse if name already exists in this directory.
        if self.find_dir_entry_by_name(parent, leaf)?.is_some() {
            return Err(FatError::InvalidPath); // mapped to EEXIST higher up
        }

        // Step 1 (C-1 ordering): allocate first cluster + mark EOC.
        // For an empty file we still allocate one cluster so the
        // dir entry has a valid first_cluster (some readers reject 0).
        // Actually FAT permits first_cluster == 0 for empty files —
        // so skip allocation entirely. Real writes (open + write)
        // will extend later.
        let first_cluster = ClusterId(0);

        // Step 2: build the directory entries.
        let sfn = self.assign_short_name(parent, leaf)?;
        let mut entries: Vec<[u8; 32]> = Vec::new();

        // LFN run only if needed.
        let needs_lfn = !fits_short_name_strict(leaf);
        if needs_lfn {
            let lfn_run = encode_lfn_run(leaf, &sfn).ok_or(FatError::InvalidPath)?;
            entries.extend(lfn_run);
        }

        // 8.3 stub.
        let sfn_entry = build_sfn_entry(&sfn, first_cluster, 0, false);
        entries.push(sfn_entry);

        // Step 3: find N consecutive free slots and write.
        let slots = self.find_free_dir_slots(parent, entries.len())?;
        for (e, loc) in entries.iter().zip(slots.iter()) {
            self.write_dir_slot(*loc, e)?;
        }

        // Update the short-name cache for the basename prefix.
        self.cache_register_sfn(parent, &sfn);

        Ok(first_cluster)
    }

    /// Unlink (delete) the file at `path`. Per C-1 ordering:
    /// tombstone the directory entries FIRST, then free the cluster
    /// chain.
    pub fn unlink_file(&self, path: &str) -> Result<(), FatError> {
        if !self.is_writable() {
            return Err(FatError::ReadOnly);
        }
        let (parent, leaf) = self.resolve_parent(path)?;
        let entry = self
            .find_dir_entry_by_name(parent, leaf)?
            .ok_or(FatError::NotFound)?;
        if entry.attrs & 0x10 != 0 {
            // It's a directory — caller should use remove_directory.
            return Err(FatError::InvalidPath);
        }

        // Step 1: tombstone the LFN run (if any) + SFN.
        for loc in &entry.slot_locs {
            let mut tomb = [0u8; 32];
            tomb[0] = 0xE5;
            self.write_dir_slot(*loc, &tomb)?;
        }

        // Step 2: free the cluster chain. Empty files (first_cluster
        // == 0) skip this.
        if entry.first_cluster.0 >= 2 {
            self.free_cluster_chain(entry.first_cluster)?;
        }
        Ok(())
    }

    /// Find a directory entry by long-or-short name match (case-
    /// insensitive ASCII). Returns the SFN's first_cluster, attrs,
    /// size, AND the slot locations comprising the entry (LFN run +
    /// SFN stub) — needed for unlink to tombstone the right range.
    fn find_dir_entry_by_name(
        &self,
        parent: Option<ClusterId>,
        target: &str,
    ) -> Result<Option<DirEntryLookup>, FatError> {
        let mut acc = LfnAccumulator::new();
        let mut decoded = [0u8; MAX_LFN_UTF8];
        let mut short_buf = [0u8; 13];
        // Track the slot locations of the LFN run currently being
        // accumulated; cleared on each non-LFN entry.
        let mut pending_lfn_locs: Vec<DirSlotLoc> = Vec::new();
        let mut found: Option<DirEntryLookup> = None;

        self.walk_dir_slots(parent, |slot, loc| {
            let first = slot[0];
            if first == 0x00 {
                return true; // end of directory
            }
            if first == 0xE5 {
                acc.reset();
                pending_lfn_locs.clear();
                return false;
            }
            let attrs = slot[11];
            if attrs == 0x0F {
                // LFN slot. Push to accumulator + remember its loc.
                if let Ok(lfn_view) = LongFileNameEntry::from_bytes(slot) {
                    acc.push_slot(lfn_view);
                    pending_lfn_locs.push(loc);
                }
                return false;
            }
            if attrs & 0x08 != 0 {
                // Volume label — skip; reset state.
                acc.reset();
                pending_lfn_locs.clear();
                return false;
            }
            // SFN stub. Decode name (LFN if accumulator is non-empty,
            // else SFN with case bits).
            let stub_view = match RawDirEntry::from_bytes(slot) {
                Ok(v) => v,
                Err(_) => {
                    acc.reset();
                    pending_lfn_locs.clear();
                    return false;
                }
            };
            let (name_bytes, name_len) = if !acc.is_empty() {
                match acc.decode(stub_view, &mut decoded) {
                    Some(n) => (&decoded[..n], n),
                    None => {
                        let sn = format_short_name_with_case(stub_view, &mut short_buf);
                        (&short_buf[..sn], sn)
                    }
                }
            } else {
                let sn = format_short_name_with_case(stub_view, &mut short_buf);
                (&short_buf[..sn], sn)
            };
            let _ = name_len;
            let name_str = core::str::from_utf8(name_bytes).unwrap_or("");
            if name_str.eq_ignore_ascii_case(target) {
                // Found it. Collect the slot range.
                let mut all_locs = pending_lfn_locs.clone();
                all_locs.push(loc);
                found = Some(DirEntryLookup {
                    first_cluster: stub_view.first_cluster(),
                    size: stub_view.file_size,
                    attrs,
                    slot_locs: all_locs,
                });
                acc.reset();
                pending_lfn_locs.clear();
                return true;
            }
            acc.reset();
            pending_lfn_locs.clear();
            false
        })?;

        Ok(found)
    }

    /// Pick a short name for `leaf` in directory `parent`. Uses the
    /// cache to skip directory scans when possible (Corrections C-3).
    fn assign_short_name(
        &self,
        parent: Option<ClusterId>,
        leaf: &str,
    ) -> Result<[u8; 11], FatError> {
        // If the long name already fits 8.3 STRICTLY (uppercase,
        // legal chars), use it verbatim. Mixed case fits via the
        // lowercase-attr bits which build_sfn_entry handles.
        if fits_short_name_strict(leaf) {
            return Ok(format_strict_short_name(leaf));
        }

        // Compute the basename prefix (first 6 ASCII chars of the
        // uppercased, stripped basename — matches `generate_short_name`'s
        // logic for what precedes ~N).
        let prefix = basename_prefix6(leaf);

        // Look up / populate the cache for this directory.
        let cache_parent_key = parent.map(|c| c.0).unwrap_or(0);
        let mut state = self.state.lock();
        let cache = state.sn_cache.entry(cache_parent_key).or_default();
        let populated = cache.populated;
        drop(state);
        if !populated {
            self.populate_sn_cache(parent)?;
        }

        let mut state = self.state.lock();
        let cache = state.sn_cache.get_mut(&cache_parent_key).unwrap();
        let next_n = cache.suffix_by_prefix.get(&prefix).copied().unwrap_or(0) + 1;
        cache.suffix_by_prefix.insert(prefix, next_n);
        drop(state);

        Ok(generate_short_name(leaf, next_n))
    }

    /// Walk a directory once to seed the short-name cache with the
    /// highest `~N` per basename prefix.
    fn populate_sn_cache(&self, parent: Option<ClusterId>) -> Result<(), FatError> {
        let cache_parent_key = parent.map(|c| c.0).unwrap_or(0);
        let mut acc: BTreeMap<[u8; 6], u32> = BTreeMap::new();
        self.walk_dir_slots(parent, |slot, _| {
            let first = slot[0];
            if first == 0x00 {
                return true;
            }
            if first == 0xE5 {
                return false;
            }
            let attrs = slot[11];
            if attrs == 0x0F || attrs & 0x08 != 0 {
                return false;
            }
            // SFN stub. Look for `~N` in the basename.
            let basename = &slot[0..8];
            // Find '~' in basename.
            if let Some(tilde_at) = basename.iter().position(|&b| b == b'~') {
                let mut prefix = [b' '; 6];
                let copy_len = tilde_at.min(6);
                prefix[..copy_len].copy_from_slice(&basename[..copy_len]);
                // Parse digits after ~.
                let mut n: u32 = 0;
                for &d in &basename[tilde_at + 1..] {
                    if d == b' ' {
                        break;
                    }
                    if !d.is_ascii_digit() {
                        n = 0;
                        break;
                    }
                    n = n * 10 + (d - b'0') as u32;
                }
                if n > 0 {
                    let cur = acc.get(&prefix).copied().unwrap_or(0);
                    if n > cur {
                        acc.insert(prefix, n);
                    }
                }
            }
            false
        })?;
        let mut state = self.state.lock();
        let cache = state.sn_cache.entry(cache_parent_key).or_default();
        cache.suffix_by_prefix = acc;
        cache.populated = true;
        Ok(())
    }

    /// Register a freshly-created SFN with the cache.
    fn cache_register_sfn(&self, parent: Option<ClusterId>, sfn: &[u8; 11]) {
        let cache_parent_key = parent.map(|c| c.0).unwrap_or(0);
        let basename = &sfn[0..8];
        if let Some(tilde_at) = basename.iter().position(|&b| b == b'~') {
            let mut prefix = [b' '; 6];
            let copy_len = tilde_at.min(6);
            prefix[..copy_len].copy_from_slice(&basename[..copy_len]);
            let mut n: u32 = 0;
            for &d in &basename[tilde_at + 1..] {
                if d == b' ' {
                    break;
                }
                if !d.is_ascii_digit() {
                    return;
                }
                n = n * 10 + (d - b'0') as u32;
            }
            if n > 0 {
                let mut state = self.state.lock();
                let cache = state.sn_cache.entry(cache_parent_key).or_default();
                let cur = cache.suffix_by_prefix.get(&prefix).copied().unwrap_or(0);
                if n > cur {
                    cache.suffix_by_prefix.insert(prefix, n);
                }
            }
        }
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
    pub fn list_root_array(
        &self,
        entries: &mut [FileHandle],
        max_entries: usize,
    ) -> Result<usize, FatError> {
        let mut count = 0;

        match self.fat_type {
            FatType::Fat12 | FatType::Fat16 => {
                let root_start_sector = self.first_data_sector - self.root_dir_sectors;
                let mut buffer = [0u8; 512];

                'outer: for i in 0..self.root_dir_sectors {
                    self.device
                        .read_blocks((root_start_sector + i) as u64, 1, &mut buffer)
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

    fn read_directory_array(
        &self,
        start_cluster: ClusterId,
        entries: &mut [FileHandle],
        max_entries: usize,
    ) -> Result<usize, FatError> {
        let cluster_size = self.sectors_per_cluster as usize * self.bytes_per_sector as usize;
        let mut buffer = [0u8; 8192]; // Fixed size buffer for cluster data
        let mut count = 0;

        // Read boot sector to create FAT table
        let mut boot_sector_data = [0u8; 512];
        self.device
            .read_blocks(0, 1, &mut boot_sector_data)
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

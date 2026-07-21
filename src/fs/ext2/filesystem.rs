use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::arch::x86_64::interrupt_guard::InterruptMutex;
use crate::drivers::block::BlockDevice;
use crate::fs::block_io::BlockIo;
use crate::fs::filesystem::{
    DirectoryEntry, DirectoryIterator, FileAttributes, FileHandle, FileMode, FileType, Filesystem,
    FilesystemError, FilesystemStats,
};

use super::ondisk::{le16, le32, put16, put32, read_groups, ExtGeometry, GroupDesc, EXT2_VALID_FS};

const ROOT_INODE: u32 = 2;
const HANDLE_BASE: u64 = 1u64 << 52;
const MODE_TYPE_MASK: u16 = 0xf000;
const MODE_REG: u16 = 0x8000;
const MODE_DIR: u16 = 0x4000;
const MODE_SYMLINK: u16 = 0xa000;
const DIR_FT_REG: u8 = 1;
const DIR_FT_DIR: u8 = 2;
const DIR_FT_SYMLINK: u8 = 7;

#[derive(Clone)]
struct Inode {
    number: u32,
    raw: Vec<u8>,
}

impl Inode {
    fn blank(number: u32, size: usize) -> Self {
        Self {
            number,
            raw: vec![0u8; size],
        }
    }

    fn mode(&self) -> u16 {
        le16(&self.raw, 0)
    }
    fn set_mode(&mut self, mode: u16) {
        put16(&mut self.raw, 0, mode);
    }
    fn is_dir(&self) -> bool {
        self.mode() & MODE_TYPE_MASK == MODE_DIR
    }
    fn is_file(&self) -> bool {
        self.mode() & MODE_TYPE_MASK == MODE_REG
    }
    fn is_symlink(&self) -> bool {
        self.mode() & MODE_TYPE_MASK == MODE_SYMLINK
    }
    fn size(&self) -> u64 {
        let low = le32(&self.raw, 4) as u64;
        if self.is_file() {
            low | ((le32(&self.raw, 108) as u64) << 32)
        } else {
            low
        }
    }
    fn set_size(&mut self, size: u64) {
        put32(&mut self.raw, 4, size as u32);
        if self.is_file() {
            put32(&mut self.raw, 108, (size >> 32) as u32);
        }
    }
    fn links(&self) -> u16 {
        le16(&self.raw, 26)
    }
    fn set_links(&mut self, links: u16) {
        put16(&mut self.raw, 26, links);
    }
    fn sectors(&self) -> u32 {
        le32(&self.raw, 28)
    }
    fn set_sectors(&mut self, sectors: u32) {
        put32(&mut self.raw, 28, sectors);
    }
    fn block(&self, index: usize) -> u32 {
        le32(&self.raw, 40 + index * 4)
    }
    fn set_block(&mut self, index: usize, block: u32) {
        put32(&mut self.raw, 40 + index * 4, block);
    }
    fn set_times(&mut self, now: u32) {
        put32(&mut self.raw, 8, now);
        put32(&mut self.raw, 12, now);
        put32(&mut self.raw, 16, now);
    }
    fn set_mtime_ctime(&mut self, now: u32) {
        put32(&mut self.raw, 12, now);
        put32(&mut self.raw, 16, now);
    }
    fn set_accessed(&mut self, value: u32) {
        put32(&mut self.raw, 8, value);
    }
    fn set_modified(&mut self, value: u32) {
        put32(&mut self.raw, 16, value);
    }
    fn set_changed(&mut self, value: u32) {
        put32(&mut self.raw, 12, value);
    }
    fn set_dtime(&mut self, now: u32) {
        put32(&mut self.raw, 20, now);
    }
}

#[derive(Clone, Copy)]
struct OpenFile {
    inode: u32,
    mode: FileMode,
}

struct MutableState {
    super_raw: [u8; 1024],
    groups: Vec<GroupDesc>,
    open: BTreeMap<u64, OpenFile>,
    next_handle: u64,
    dirty: bool,
}

#[derive(Clone)]
struct Dirent {
    inode: u32,
    name: Vec<u8>,
}

pub struct Ext2Filesystem<'a> {
    io: BlockIo<'a>,
    geometry: ExtGeometry,
    writable: bool,
    state: InterruptMutex<MutableState>,
}

impl<'a> Ext2Filesystem<'a> {
    pub fn new(
        device: &'a dyn BlockDevice,
        request_writable: bool,
        force_dirty: bool,
    ) -> Result<Self, FilesystemError> {
        let (geometry, super_raw) = ExtGeometry::parse(device)?;
        geometry.validate_features(request_writable)?;
        let clean = le16(&super_raw, 58) & EXT2_VALID_FS != 0;
        let writable = request_writable && !device.is_read_only() && (clean || force_dirty);
        if request_writable && !writable {
            return Err(FilesystemError::ReadOnly);
        }
        let io = BlockIo::new(device, geometry.block_size)?;
        let groups = read_groups(&io, &geometry)?;
        let root = Self {
            io,
            geometry,
            writable,
            state: InterruptMutex::new(MutableState {
                super_raw,
                groups,
                open: BTreeMap::new(),
                next_handle: HANDLE_BASE,
                dirty: !clean,
            }),
        };
        let inode = root.read_inode(ROOT_INODE)?;
        if !inode.is_dir() {
            return Err(FilesystemError::Corrupted);
        }
        Ok(root)
    }

    fn now() -> u32 {
        (crate::arch::x86_64::interrupts::get_timer_ticks() / 100).min(u32::MAX as u64) as u32
    }

    fn group_desc_offset(&self, group: u32) -> u64 {
        self.geometry.desc_table_block as u64 * self.geometry.block_size as u64 + group as u64 * 32
    }

    fn write_group(&self, group: u32, desc: &GroupDesc) -> Result<(), FilesystemError> {
        self.io
            .write_bytes(self.group_desc_offset(group), &desc.raw)
    }

    fn inode_offset(&self, inode: u32, groups: &[GroupDesc]) -> Result<u64, FilesystemError> {
        if inode == 0 || inode > self.geometry.inodes_count {
            return Err(FilesystemError::Corrupted);
        }
        let zero = inode - 1;
        let group = zero / self.geometry.inodes_per_group;
        let index = zero % self.geometry.inodes_per_group;
        let table = groups
            .get(group as usize)
            .ok_or(FilesystemError::Corrupted)?
            .inode_table();
        Ok(table as u64 * self.geometry.block_size as u64
            + index as u64 * self.geometry.inode_size as u64)
    }

    fn read_inode_with_groups(
        &self,
        inode: u32,
        groups: &[GroupDesc],
    ) -> Result<Inode, FilesystemError> {
        let mut raw = vec![0u8; self.geometry.inode_size as usize];
        self.io
            .read_bytes(self.inode_offset(inode, groups)?, &mut raw)?;
        Ok(Inode { number: inode, raw })
    }

    fn read_inode(&self, inode: u32) -> Result<Inode, FilesystemError> {
        let state = self.state.lock();
        self.read_inode_with_groups(inode, &state.groups)
    }

    fn write_inode(&self, inode: &Inode, groups: &[GroupDesc]) -> Result<(), FilesystemError> {
        self.io
            .write_bytes(self.inode_offset(inode.number, groups)?, &inode.raw)
    }

    fn write_super(&self, state: &MutableState) -> Result<(), FilesystemError> {
        self.io.write_bytes(1024, &state.super_raw)
    }

    fn mark_dirty(&self, state: &mut MutableState) -> Result<(), FilesystemError> {
        if !self.writable {
            return Err(FilesystemError::ReadOnly);
        }
        if !state.dirty {
            let value = le16(&state.super_raw, 58) & !EXT2_VALID_FS;
            put16(&mut state.super_raw, 58, value);
            put32(&mut state.super_raw, 48, Self::now());
            self.write_super(state)?;
            self.io.flush()?;
            state.dirty = true;
        }
        Ok(())
    }

    fn read_pointer_block(&self, block: u32) -> Result<Vec<u8>, FilesystemError> {
        if !self.geometry.valid_block(block) {
            return Err(FilesystemError::Corrupted);
        }
        let mut data = vec![0u8; self.geometry.block_size as usize];
        self.io.read_block(block as u64, &mut data)?;
        Ok(data)
    }

    fn block_at(&self, inode: &Inode, logical: u64) -> Result<u32, FilesystemError> {
        let fanout = (self.geometry.block_size / 4) as u64;
        if logical < 12 {
            return Ok(inode.block(logical as usize));
        }
        let mut n = logical - 12;
        if n < fanout {
            let root = inode.block(12);
            if root == 0 {
                return Ok(0);
            }
            let block = self.read_pointer_block(root)?;
            return Ok(le32(&block, n as usize * 4));
        }
        n -= fanout;
        let double_cap = fanout
            .checked_mul(fanout)
            .ok_or(FilesystemError::Corrupted)?;
        if n < double_cap {
            let root = inode.block(13);
            if root == 0 {
                return Ok(0);
            }
            let first = self.read_pointer_block(root)?;
            let child = le32(&first, (n / fanout) as usize * 4);
            if child == 0 {
                return Ok(0);
            }
            let second = self.read_pointer_block(child)?;
            return Ok(le32(&second, (n % fanout) as usize * 4));
        }
        n -= double_cap;
        let triple_cap = double_cap
            .checked_mul(fanout)
            .ok_or(FilesystemError::Corrupted)?;
        if n >= triple_cap {
            return Err(FilesystemError::BufferTooSmall);
        }
        let root = inode.block(14);
        if root == 0 {
            return Ok(0);
        }
        let first = self.read_pointer_block(root)?;
        let child1 = le32(&first, (n / double_cap) as usize * 4);
        if child1 == 0 {
            return Ok(0);
        }
        let second = self.read_pointer_block(child1)?;
        let rem = n % double_cap;
        let child2 = le32(&second, (rem / fanout) as usize * 4);
        if child2 == 0 {
            return Ok(0);
        }
        let third = self.read_pointer_block(child2)?;
        Ok(le32(&third, (rem % fanout) as usize * 4))
    }

    fn update_super_counts(
        &self,
        state: &mut MutableState,
        free_blocks_delta: i64,
        free_inodes_delta: i64,
    ) -> Result<(), FilesystemError> {
        let blocks = (le32(&state.super_raw, 12) as i64)
            .checked_add(free_blocks_delta)
            .ok_or(FilesystemError::Corrupted)?;
        let inodes = (le32(&state.super_raw, 16) as i64)
            .checked_add(free_inodes_delta)
            .ok_or(FilesystemError::Corrupted)?;
        if blocks < 0
            || blocks > self.geometry.blocks_count as i64
            || inodes < 0
            || inodes > self.geometry.inodes_count as i64
        {
            return Err(FilesystemError::Corrupted);
        }
        put32(&mut state.super_raw, 12, blocks as u32);
        put32(&mut state.super_raw, 16, inodes as u32);
        self.write_super(state)
    }

    fn allocate_block(&self, state: &mut MutableState) -> Result<u32, FilesystemError> {
        if le32(&state.super_raw, 12) <= self.geometry.reserved_blocks {
            return Err(FilesystemError::DiskFull);
        }
        for group in 0..self.geometry.group_count {
            if state.groups[group as usize].free_blocks() == 0 {
                continue;
            }
            let bitmap_block = state.groups[group as usize].block_bitmap();
            let mut bitmap = vec![0u8; self.geometry.block_size as usize];
            self.io.read_block(bitmap_block as u64, &mut bitmap)?;
            let start = self.geometry.group_start(group)?;
            let valid = core::cmp::min(
                self.geometry.blocks_per_group,
                self.geometry.blocks_count.saturating_sub(start),
            );
            for bit in 0..valid {
                let byte = (bit / 8) as usize;
                let mask = 1u8 << (bit % 8);
                if bitmap[byte] & mask != 0 {
                    continue;
                }
                bitmap[byte] |= mask;
                self.io.write_block(bitmap_block as u64, &bitmap)?;
                let desc = &mut state.groups[group as usize];
                desc.set_free_blocks(
                    desc.free_blocks()
                        .checked_sub(1)
                        .ok_or(FilesystemError::Corrupted)?,
                );
                let desc_copy = desc.clone();
                self.write_group(group, &desc_copy)?;
                self.update_super_counts(state, -1, 0)?;
                let block = start + bit;
                let zero = vec![0u8; self.geometry.block_size as usize];
                self.io.write_block(block as u64, &zero)?;
                return Ok(block);
            }
        }
        Err(FilesystemError::DiskFull)
    }

    fn free_block(&self, state: &mut MutableState, block: u32) -> Result<(), FilesystemError> {
        if !self.geometry.valid_block(block) {
            return Err(FilesystemError::Corrupted);
        }
        let rel = block - self.geometry.first_data_block;
        let group = rel / self.geometry.blocks_per_group;
        let bit = rel % self.geometry.blocks_per_group;
        let bitmap_block = state.groups[group as usize].block_bitmap();
        let mut bitmap = vec![0u8; self.geometry.block_size as usize];
        self.io.read_block(bitmap_block as u64, &mut bitmap)?;
        let byte = (bit / 8) as usize;
        let mask = 1u8 << (bit % 8);
        if bitmap[byte] & mask == 0 {
            return Err(FilesystemError::Corrupted);
        }
        bitmap[byte] &= !mask;
        self.io.write_block(bitmap_block as u64, &bitmap)?;
        let desc = &mut state.groups[group as usize];
        desc.set_free_blocks(
            desc.free_blocks()
                .checked_add(1)
                .ok_or(FilesystemError::Corrupted)?,
        );
        let desc_copy = desc.clone();
        self.write_group(group, &desc_copy)?;
        self.update_super_counts(state, 1, 0)
    }

    fn allocate_inode(
        &self,
        state: &mut MutableState,
        directory: bool,
    ) -> Result<u32, FilesystemError> {
        for group in 0..self.geometry.group_count {
            if state.groups[group as usize].free_inodes() == 0 {
                continue;
            }
            let bitmap_block = state.groups[group as usize].inode_bitmap();
            let mut bitmap = vec![0u8; self.geometry.block_size as usize];
            self.io.read_block(bitmap_block as u64, &mut bitmap)?;
            let group_first = group * self.geometry.inodes_per_group + 1;
            let valid = core::cmp::min(
                self.geometry.inodes_per_group,
                self.geometry.inodes_count.saturating_sub(group_first - 1),
            );
            for bit in 0..valid {
                let inode = group_first + bit;
                if inode < self.geometry.first_inode && inode != ROOT_INODE {
                    continue;
                }
                let byte = (bit / 8) as usize;
                let mask = 1u8 << (bit % 8);
                if bitmap[byte] & mask != 0 {
                    continue;
                }
                bitmap[byte] |= mask;
                self.io.write_block(bitmap_block as u64, &bitmap)?;
                let desc = &mut state.groups[group as usize];
                desc.set_free_inodes(
                    desc.free_inodes()
                        .checked_sub(1)
                        .ok_or(FilesystemError::Corrupted)?,
                );
                if directory {
                    desc.set_used_dirs(
                        desc.used_dirs()
                            .checked_add(1)
                            .ok_or(FilesystemError::Corrupted)?,
                    );
                }
                let desc_copy = desc.clone();
                self.write_group(group, &desc_copy)?;
                self.update_super_counts(state, 0, -1)?;
                return Ok(inode);
            }
        }
        Err(FilesystemError::DiskFull)
    }

    fn free_inode_bitmap(
        &self,
        state: &mut MutableState,
        inode: u32,
        directory: bool,
    ) -> Result<(), FilesystemError> {
        if inode < self.geometry.first_inode || inode > self.geometry.inodes_count {
            return Err(FilesystemError::Corrupted);
        }
        let zero = inode - 1;
        let group = zero / self.geometry.inodes_per_group;
        let bit = zero % self.geometry.inodes_per_group;
        let bitmap_block = state.groups[group as usize].inode_bitmap();
        let mut bitmap = vec![0u8; self.geometry.block_size as usize];
        self.io.read_block(bitmap_block as u64, &mut bitmap)?;
        let byte = (bit / 8) as usize;
        let mask = 1u8 << (bit % 8);
        if bitmap[byte] & mask == 0 {
            return Err(FilesystemError::Corrupted);
        }
        bitmap[byte] &= !mask;
        self.io.write_block(bitmap_block as u64, &bitmap)?;
        let desc = &mut state.groups[group as usize];
        desc.set_free_inodes(
            desc.free_inodes()
                .checked_add(1)
                .ok_or(FilesystemError::Corrupted)?,
        );
        if directory {
            desc.set_used_dirs(
                desc.used_dirs()
                    .checked_sub(1)
                    .ok_or(FilesystemError::Corrupted)?,
            );
        }
        let desc_copy = desc.clone();
        self.write_group(group, &desc_copy)?;
        self.update_super_counts(state, 0, 1)
    }

    fn add_inode_sectors(&self, inode: &mut Inode, blocks: u32) -> Result<(), FilesystemError> {
        let sectors = blocks
            .checked_mul(self.geometry.block_size / 512)
            .ok_or(FilesystemError::BufferTooSmall)?;
        inode.set_sectors(
            inode
                .sectors()
                .checked_add(sectors)
                .ok_or(FilesystemError::BufferTooSmall)?,
        );
        Ok(())
    }

    fn ensure_block(
        &self,
        state: &mut MutableState,
        inode: &mut Inode,
        logical: u64,
    ) -> Result<u32, FilesystemError> {
        let existing = self.block_at(inode, logical)?;
        if existing != 0 {
            return Ok(existing);
        }
        let fanout = (self.geometry.block_size / 4) as u64;
        let data = self.allocate_block(state)?;
        self.add_inode_sectors(inode, 1)?;
        if logical < 12 {
            inode.set_block(logical as usize, data);
            return Ok(data);
        }
        let mut n = logical - 12;
        if n < fanout {
            let root = if inode.block(12) == 0 {
                let b = self.allocate_block(state)?;
                self.add_inode_sectors(inode, 1)?;
                inode.set_block(12, b);
                b
            } else {
                inode.block(12)
            };
            let mut pointers = self.read_pointer_block(root)?;
            put32(&mut pointers, n as usize * 4, data);
            self.io.write_block(root as u64, &pointers)?;
            return Ok(data);
        }
        n -= fanout;
        let double_cap = fanout * fanout;
        if n < double_cap {
            let root = if inode.block(13) == 0 {
                let b = self.allocate_block(state)?;
                self.add_inode_sectors(inode, 1)?;
                inode.set_block(13, b);
                b
            } else {
                inode.block(13)
            };
            let mut first = self.read_pointer_block(root)?;
            let first_index = (n / fanout) as usize;
            let child = if le32(&first, first_index * 4) == 0 {
                let b = self.allocate_block(state)?;
                self.add_inode_sectors(inode, 1)?;
                put32(&mut first, first_index * 4, b);
                self.io.write_block(root as u64, &first)?;
                b
            } else {
                le32(&first, first_index * 4)
            };
            let mut second = self.read_pointer_block(child)?;
            put32(&mut second, (n % fanout) as usize * 4, data);
            self.io.write_block(child as u64, &second)?;
            return Ok(data);
        }
        n -= double_cap;
        let triple_cap = double_cap
            .checked_mul(fanout)
            .ok_or(FilesystemError::BufferTooSmall)?;
        if n >= triple_cap {
            self.free_block(state, data)?;
            return Err(FilesystemError::BufferTooSmall);
        }
        let root = if inode.block(14) == 0 {
            let b = self.allocate_block(state)?;
            self.add_inode_sectors(inode, 1)?;
            inode.set_block(14, b);
            b
        } else {
            inode.block(14)
        };
        let mut first = self.read_pointer_block(root)?;
        let i1 = (n / double_cap) as usize;
        let child1 = if le32(&first, i1 * 4) == 0 {
            let b = self.allocate_block(state)?;
            self.add_inode_sectors(inode, 1)?;
            put32(&mut first, i1 * 4, b);
            self.io.write_block(root as u64, &first)?;
            b
        } else {
            le32(&first, i1 * 4)
        };
        let mut second = self.read_pointer_block(child1)?;
        let rem = n % double_cap;
        let i2 = (rem / fanout) as usize;
        let child2 = if le32(&second, i2 * 4) == 0 {
            let b = self.allocate_block(state)?;
            self.add_inode_sectors(inode, 1)?;
            put32(&mut second, i2 * 4, b);
            self.io.write_block(child1 as u64, &second)?;
            b
        } else {
            le32(&second, i2 * 4)
        };
        let mut third = self.read_pointer_block(child2)?;
        put32(&mut third, (rem % fanout) as usize * 4, data);
        self.io.write_block(child2 as u64, &third)?;
        Ok(data)
    }

    fn pointers_empty(block: &[u8]) -> bool {
        block.chunks_exact(4).all(|p| p == [0, 0, 0, 0])
    }

    fn free_indirect_tree(
        &self,
        state: &mut MutableState,
        block: u32,
        depth: u8,
    ) -> Result<u32, FilesystemError> {
        let pointers = self.read_pointer_block(block)?;
        let mut freed = 1u32;
        for offset in (0..pointers.len()).step_by(4) {
            let child = le32(&pointers, offset);
            if child == 0 {
                continue;
            }
            if depth == 1 {
                self.free_block(state, child)?;
                freed = freed.checked_add(1).ok_or(FilesystemError::Corrupted)?;
            } else {
                freed = freed
                    .checked_add(self.free_indirect_tree(state, child, depth - 1)?)
                    .ok_or(FilesystemError::Corrupted)?;
            }
        }
        self.free_block(state, block)?;
        Ok(freed)
    }

    /// Remove allocated leaves at or beyond `keep_blocks` without iterating
    /// across holes. The returned count excludes `block` itself; `true` tells
    /// the caller that the now-empty pointer block can also be freed.
    fn prune_indirect_tree(
        &self,
        state: &mut MutableState,
        block: u32,
        depth: u8,
        base: u64,
        keep_blocks: u64,
    ) -> Result<(u32, bool), FilesystemError> {
        let fanout = (self.geometry.block_size / 4) as u64;
        let child_capacity = fanout.pow((depth - 1) as u32);
        let mut pointers = self.read_pointer_block(block)?;
        let mut freed = 0u32;
        for index in 0..fanout {
            let offset = index as usize * 4;
            let child = le32(&pointers, offset);
            if child == 0 {
                continue;
            }
            let child_base = base
                .checked_add(
                    index
                        .checked_mul(child_capacity)
                        .ok_or(FilesystemError::Corrupted)?,
                )
                .ok_or(FilesystemError::Corrupted)?;
            if child_base >= keep_blocks {
                let count = if depth == 1 {
                    self.free_block(state, child)?;
                    1
                } else {
                    self.free_indirect_tree(state, child, depth - 1)?
                };
                freed = freed.checked_add(count).ok_or(FilesystemError::Corrupted)?;
                put32(&mut pointers, offset, 0);
            } else if depth > 1
                && child_base
                    .checked_add(child_capacity)
                    .ok_or(FilesystemError::Corrupted)?
                    > keep_blocks
            {
                let (child_freed, empty) =
                    self.prune_indirect_tree(state, child, depth - 1, child_base, keep_blocks)?;
                freed = freed
                    .checked_add(child_freed)
                    .ok_or(FilesystemError::Corrupted)?;
                if empty {
                    self.free_block(state, child)?;
                    freed = freed.checked_add(1).ok_or(FilesystemError::Corrupted)?;
                    put32(&mut pointers, offset, 0);
                }
            }
        }
        let empty = Self::pointers_empty(&pointers);
        if !empty {
            self.io.write_block(block as u64, &pointers)?;
        }
        Ok((freed, empty))
    }

    #[expect(dead_code, reason = "retained single-block removal primitive")]
    fn remove_block(
        &self,
        state: &mut MutableState,
        inode: &mut Inode,
        logical: u64,
    ) -> Result<u32, FilesystemError> {
        let fanout = (self.geometry.block_size / 4) as u64;
        let mut freed = Vec::new();
        if logical < 12 {
            let data = inode.block(logical as usize);
            if data == 0 {
                return Ok(0);
            }
            inode.set_block(logical as usize, 0);
            freed.push(data);
        } else {
            let mut n = logical - 12;
            if n < fanout {
                let root = inode.block(12);
                if root == 0 {
                    return Ok(0);
                }
                let mut pointers = self.read_pointer_block(root)?;
                let data = le32(&pointers, n as usize * 4);
                if data == 0 {
                    return Ok(0);
                }
                put32(&mut pointers, n as usize * 4, 0);
                freed.push(data);
                if Self::pointers_empty(&pointers) {
                    inode.set_block(12, 0);
                    freed.push(root);
                } else {
                    self.io.write_block(root as u64, &pointers)?;
                }
            } else {
                n -= fanout;
                let double_cap = fanout * fanout;
                if n < double_cap {
                    let root = inode.block(13);
                    if root == 0 {
                        return Ok(0);
                    }
                    let mut first = self.read_pointer_block(root)?;
                    let i1 = (n / fanout) as usize;
                    let child = le32(&first, i1 * 4);
                    if child == 0 {
                        return Ok(0);
                    }
                    let mut second = self.read_pointer_block(child)?;
                    let i2 = (n % fanout) as usize;
                    let data = le32(&second, i2 * 4);
                    if data == 0 {
                        return Ok(0);
                    }
                    put32(&mut second, i2 * 4, 0);
                    freed.push(data);
                    if Self::pointers_empty(&second) {
                        put32(&mut first, i1 * 4, 0);
                        freed.push(child);
                    } else {
                        self.io.write_block(child as u64, &second)?;
                    }
                    if Self::pointers_empty(&first) {
                        inode.set_block(13, 0);
                        freed.push(root);
                    } else {
                        self.io.write_block(root as u64, &first)?;
                    }
                } else {
                    n -= double_cap;
                    let triple_cap = double_cap
                        .checked_mul(fanout)
                        .ok_or(FilesystemError::Corrupted)?;
                    if n >= triple_cap {
                        return Err(FilesystemError::BufferTooSmall);
                    }
                    let root = inode.block(14);
                    if root == 0 {
                        return Ok(0);
                    }
                    let mut first = self.read_pointer_block(root)?;
                    let i1 = (n / double_cap) as usize;
                    let child1 = le32(&first, i1 * 4);
                    if child1 == 0 {
                        return Ok(0);
                    }
                    let mut second = self.read_pointer_block(child1)?;
                    let rem = n % double_cap;
                    let i2 = (rem / fanout) as usize;
                    let child2 = le32(&second, i2 * 4);
                    if child2 == 0 {
                        return Ok(0);
                    }
                    let mut third = self.read_pointer_block(child2)?;
                    let i3 = (rem % fanout) as usize;
                    let data = le32(&third, i3 * 4);
                    if data == 0 {
                        return Ok(0);
                    }
                    put32(&mut third, i3 * 4, 0);
                    freed.push(data);
                    if Self::pointers_empty(&third) {
                        put32(&mut second, i2 * 4, 0);
                        freed.push(child2);
                    } else {
                        self.io.write_block(child2 as u64, &third)?;
                    }
                    if Self::pointers_empty(&second) {
                        put32(&mut first, i1 * 4, 0);
                        freed.push(child1);
                    } else {
                        self.io.write_block(child1 as u64, &second)?;
                    }
                    if Self::pointers_empty(&first) {
                        inode.set_block(14, 0);
                        freed.push(root);
                    } else {
                        self.io.write_block(root as u64, &first)?;
                    }
                }
            }
        }
        let count = freed.len() as u32;
        let sectors = count
            .checked_mul(self.geometry.block_size / 512)
            .ok_or(FilesystemError::Corrupted)?;
        inode.set_sectors(
            inode
                .sectors()
                .checked_sub(sectors)
                .ok_or(FilesystemError::Corrupted)?,
        );
        for block in freed {
            self.free_block(state, block)?;
        }
        Ok(count)
    }

    fn truncate_inode(
        &self,
        state: &mut MutableState,
        inode: &mut Inode,
        new_size: u64,
    ) -> Result<(), FilesystemError> {
        let old_size = inode.size();
        if new_size >= old_size {
            inode.set_size(new_size);
            inode.set_mtime_ctime(Self::now());
            return self.write_inode(inode, &state.groups);
        }
        // ext2 stores short symlink targets directly in i_block. They do not
        // own the block numbers those bytes happen to resemble, so reclaiming
        // one must clear the inline payload instead of walking block pointers.
        if inode.is_symlink() && inode.sectors() == 0 {
            inode.raw[40..100].fill(0);
            inode.set_size(new_size);
            inode.set_mtime_ctime(Self::now());
            return self.write_inode(inode, &state.groups);
        }
        let bs = self.geometry.block_size as u64;
        if new_size % bs != 0 {
            let logical = new_size / bs;
            let block = self.block_at(inode, logical)?;
            if block != 0 {
                let mut data = vec![0u8; bs as usize];
                self.io.read_block(block as u64, &mut data)?;
                data[new_size as usize % bs as usize..].fill(0);
                self.io.write_block(block as u64, &data)?;
            }
        }
        let keep_blocks = new_size.div_ceil(bs);
        let mut freed = 0u32;
        for logical in keep_blocks.min(12)..12 {
            let block = inode.block(logical as usize);
            if block != 0 {
                inode.set_block(logical as usize, 0);
                self.free_block(state, block)?;
                freed = freed.checked_add(1).ok_or(FilesystemError::Corrupted)?;
            }
        }
        let fanout = (self.geometry.block_size / 4) as u64;
        let mut base = 12u64;
        for (slot, depth) in [(12usize, 1u8), (13, 2), (14, 3)] {
            let root = inode.block(slot);
            if root != 0 {
                let (tree_freed, empty) =
                    self.prune_indirect_tree(state, root, depth, base, keep_blocks)?;
                freed = freed
                    .checked_add(tree_freed)
                    .ok_or(FilesystemError::Corrupted)?;
                if empty {
                    self.free_block(state, root)?;
                    freed = freed.checked_add(1).ok_or(FilesystemError::Corrupted)?;
                    inode.set_block(slot, 0);
                }
            }
            base = base
                .checked_add(fanout.pow(depth as u32))
                .ok_or(FilesystemError::Corrupted)?;
        }
        let sectors = freed
            .checked_mul(self.geometry.block_size / 512)
            .ok_or(FilesystemError::Corrupted)?;
        inode.set_sectors(
            inode
                .sectors()
                .checked_sub(sectors)
                .ok_or(FilesystemError::Corrupted)?,
        );
        inode.set_size(new_size);
        inode.set_mtime_ctime(Self::now());
        self.write_inode(inode, &state.groups)
    }

    fn read_inode_data(
        &self,
        inode: &Inode,
        position: u64,
        out: &mut [u8],
    ) -> Result<usize, FilesystemError> {
        if position >= inode.size() || out.is_empty() {
            return Ok(0);
        }
        let available = (inode.size() - position).min(out.len() as u64) as usize;
        let bs = self.geometry.block_size as usize;
        let mut done = 0usize;
        let mut block_buf = vec![0u8; bs];
        while done < available {
            let absolute = position + done as u64;
            let logical = absolute / bs as u64;
            let within = absolute as usize % bs;
            let count = core::cmp::min(bs - within, available - done);
            let physical = self.block_at(inode, logical)?;
            if physical == 0 {
                out[done..done + count].fill(0);
            } else {
                self.io.read_block(physical as u64, &mut block_buf)?;
                out[done..done + count].copy_from_slice(&block_buf[within..within + count]);
            }
            done += count;
        }
        Ok(done)
    }

    fn write_inode_data(
        &self,
        state: &mut MutableState,
        inode: &mut Inode,
        position: u64,
        data: &[u8],
    ) -> Result<usize, FilesystemError> {
        let end = position
            .checked_add(data.len() as u64)
            .ok_or(FilesystemError::BufferTooSmall)?;
        let bs = self.geometry.block_size as usize;
        let mut done = 0usize;
        while done < data.len() {
            let absolute = position + done as u64;
            let logical = absolute / bs as u64;
            let within = absolute as usize % bs;
            let count = core::cmp::min(bs - within, data.len() - done);
            let physical = self.ensure_block(state, inode, logical)?;
            if within == 0 && count == bs {
                self.io
                    .write_block(physical as u64, &data[done..done + count])?;
            } else {
                let mut block = vec![0u8; bs];
                self.io.read_block(physical as u64, &mut block)?;
                block[within..within + count].copy_from_slice(&data[done..done + count]);
                self.io.write_block(physical as u64, &block)?;
            }
            done += count;
        }
        if end > inode.size() {
            inode.set_size(end);
        }
        inode.set_mtime_ctime(Self::now());
        self.write_inode(inode, &state.groups)?;
        Ok(done)
    }

    fn parse_directory(&self, inode: &Inode) -> Result<Vec<Dirent>, FilesystemError> {
        if !inode.is_dir() {
            return Err(FilesystemError::NotADirectory);
        }
        let bs = self.geometry.block_size as usize;
        let blocks = inode.size().div_ceil(bs as u64);
        let mut entries = Vec::new();
        for logical in 0..blocks {
            let physical = self.block_at(inode, logical)?;
            if physical == 0 {
                return Err(FilesystemError::Corrupted);
            }
            let mut data = vec![0u8; bs];
            self.io.read_block(physical as u64, &mut data)?;
            let mut offset = 0usize;
            while offset < bs {
                if offset + 8 > bs {
                    return Err(FilesystemError::Corrupted);
                }
                let child = le32(&data, offset);
                let rec_len = le16(&data, offset + 4) as usize;
                let name_len = data[offset + 6] as usize;
                if rec_len < 8
                    || rec_len % 4 != 0
                    || offset + rec_len > bs
                    || name_len > rec_len - 8
                {
                    return Err(FilesystemError::Corrupted);
                }
                if child != 0 {
                    if child > self.geometry.inodes_count {
                        return Err(FilesystemError::Corrupted);
                    }
                    entries.push(Dirent {
                        inode: child,
                        name: data[offset + 8..offset + 8 + name_len].to_vec(),
                    });
                }
                offset += rec_len;
            }
        }
        Ok(entries)
    }

    fn lookup_child_with_groups(
        &self,
        directory: u32,
        name: &[u8],
        groups: &[GroupDesc],
    ) -> Result<Dirent, FilesystemError> {
        let inode = self.read_inode_with_groups(directory, groups)?;
        self.parse_directory(&inode)?
            .into_iter()
            .find(|entry| entry.name == name)
            .ok_or(FilesystemError::NotFound)
    }

    fn split_path<'p>(path: &'p str) -> impl Iterator<Item = &'p str> {
        path.split('/')
            .filter(|part| !part.is_empty() && *part != ".")
    }

    fn resolve_with_groups(
        &self,
        path: &str,
        groups: &[GroupDesc],
    ) -> Result<u32, FilesystemError> {
        let components = Self::split_path(path)
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        self.resolve_components(ROOT_INODE, components, groups, 0)
    }

    fn resolve_components(
        &self,
        mut current: u32,
        components: Vec<String>,
        groups: &[GroupDesc],
        symlink_depth: u8,
    ) -> Result<u32, FilesystemError> {
        if symlink_depth > 40 {
            return Err(FilesystemError::InvalidPath);
        }
        let mut index = 0usize;
        while index < components.len() {
            let part = components[index].as_str();
            if part == "." || part.is_empty() {
                index += 1;
                continue;
            }
            let parent = current;
            current = if part == ".." {
                self.lookup_child_with_groups(current, b"..", groups)?.inode
            } else {
                self.lookup_child_with_groups(current, part.as_bytes(), groups)?
                    .inode
            };
            let inode = self.read_inode_with_groups(current, groups)?;
            if inode.is_symlink() {
                let target = self.read_symlink_inode(&inode)?;
                let target =
                    core::str::from_utf8(&target).map_err(|_| FilesystemError::InvalidPath)?;
                let mut expanded = Self::split_path(target)
                    .map(ToString::to_string)
                    .collect::<Vec<_>>();
                expanded.extend(components[index + 1..].iter().cloned());
                let start = if target.starts_with('/') {
                    ROOT_INODE
                } else {
                    parent
                };
                return self.resolve_components(start, expanded, groups, symlink_depth + 1);
            }
            index += 1;
        }
        Ok(current)
    }

    fn resolve_no_follow_final(
        &self,
        path: &str,
        groups: &[GroupDesc],
    ) -> Result<u32, FilesystemError> {
        if path.trim_matches('/').is_empty() {
            return Ok(ROOT_INODE);
        }
        let (parent, leaf) = self.resolve_parent_with_groups(path, groups)?;
        Ok(self
            .lookup_child_with_groups(parent, leaf.as_bytes(), groups)?
            .inode)
    }

    fn read_symlink_inode(&self, inode: &Inode) -> Result<Vec<u8>, FilesystemError> {
        if !inode.is_symlink() {
            return Err(FilesystemError::InvalidPath);
        }
        let size = usize::try_from(inode.size()).map_err(|_| FilesystemError::BufferTooSmall)?;
        if size <= 60 && inode.sectors() == 0 {
            return Ok(inode.raw[40..40 + size].to_vec());
        }
        let mut target = vec![0u8; size];
        let read = self.read_inode_data(inode, 0, &mut target)?;
        target.truncate(read);
        Ok(target)
    }

    fn inode_metadata(&self, inode: &Inode) -> crate::fs::filesystem::UnixMetadata {
        crate::fs::filesystem::UnixMetadata {
            inode: inode.number as u64,
            mode: inode.mode() as u32,
            uid: le16(&inode.raw, 2) as u32,
            gid: le16(&inode.raw, 24) as u32,
            links: inode.links() as u64,
            size: inode.size(),
            blocks_512: inode.sectors() as u64,
            block_size: self.geometry.block_size,
            accessed: crate::fs::filesystem::UnixTimestamp::from_seconds(le32(&inode.raw, 8) as u64),
            changed: crate::fs::filesystem::UnixTimestamp::from_seconds(le32(&inode.raw, 12) as u64),
            modified: crate::fs::filesystem::UnixTimestamp::from_seconds(
                le32(&inode.raw, 16) as u64
            ),
        }
    }

    fn resolve_parent_with_groups<'p>(
        &self,
        path: &'p str,
        groups: &[GroupDesc],
    ) -> Result<(u32, &'p str), FilesystemError> {
        let trimmed = path.trim_end_matches('/');
        let index = trimmed.rfind('/');
        let (parent, leaf) = match index {
            Some(0) => ("/", &trimmed[1..]),
            Some(i) => (&trimmed[..i], &trimmed[i + 1..]),
            None => ("/", trimmed),
        };
        if leaf.is_empty() || leaf.len() > 255 || leaf.contains('\0') {
            return Err(FilesystemError::InvalidPath);
        }
        Ok((self.resolve_with_groups(parent, groups)?, leaf))
    }

    fn dir_file_type(inode: &Inode) -> u8 {
        if inode.is_dir() {
            DIR_FT_DIR
        } else if inode.is_symlink() {
            DIR_FT_SYMLINK
        } else {
            DIR_FT_REG
        }
    }

    fn insert_dirent(
        &self,
        state: &mut MutableState,
        parent_number: u32,
        child: u32,
        file_type: u8,
        name: &[u8],
    ) -> Result<(), FilesystemError> {
        if name.is_empty() || name.len() > 255 || name.contains(&0) || name.contains(&b'/') {
            return Err(FilesystemError::InvalidPath);
        }
        let mut parent = self.read_inode_with_groups(parent_number, &state.groups)?;
        if !parent.is_dir() {
            return Err(FilesystemError::NotADirectory);
        }
        if self
            .parse_directory(&parent)?
            .iter()
            .any(|entry| entry.name == name)
        {
            return Err(FilesystemError::AlreadyExists);
        }
        let needed = (8 + name.len()).next_multiple_of(4);
        let bs = self.geometry.block_size as usize;
        let blocks = parent.size().div_ceil(bs as u64);
        for logical in 0..blocks {
            let physical = self.block_at(&parent, logical)?;
            let mut data = vec![0u8; bs];
            self.io.read_block(physical as u64, &mut data)?;
            let mut offset = 0usize;
            while offset < bs {
                let existing_inode = le32(&data, offset);
                let rec_len = le16(&data, offset + 4) as usize;
                let name_len = data[offset + 6] as usize;
                if rec_len < 8 || rec_len % 4 != 0 || offset + rec_len > bs {
                    return Err(FilesystemError::Corrupted);
                }
                if existing_inode == 0 && rec_len >= needed {
                    Self::write_dirent_record(&mut data, offset, child, rec_len, file_type, name);
                    self.io.write_block(physical as u64, &data)?;
                    parent.set_mtime_ctime(Self::now());
                    return self.write_inode(&parent, &state.groups);
                }
                let actual = (8 + name_len).next_multiple_of(4);
                if existing_inode != 0 && rec_len >= actual + needed {
                    put16(&mut data, offset + 4, actual as u16);
                    Self::write_dirent_record(
                        &mut data,
                        offset + actual,
                        child,
                        rec_len - actual,
                        file_type,
                        name,
                    );
                    self.io.write_block(physical as u64, &data)?;
                    parent.set_mtime_ctime(Self::now());
                    return self.write_inode(&parent, &state.groups);
                }
                offset += rec_len;
            }
        }
        let logical = blocks;
        let physical = self.ensure_block(state, &mut parent, logical)?;
        let mut data = vec![0u8; bs];
        Self::write_dirent_record(&mut data, 0, child, bs, file_type, name);
        self.io.write_block(physical as u64, &data)?;
        parent.set_size((logical + 1) * bs as u64);
        parent.set_mtime_ctime(Self::now());
        self.write_inode(&parent, &state.groups)
    }

    fn write_dirent_record(
        data: &mut [u8],
        offset: usize,
        inode: u32,
        rec_len: usize,
        file_type: u8,
        name: &[u8],
    ) {
        put32(data, offset, inode);
        put16(data, offset + 4, rec_len as u16);
        data[offset + 6] = name.len() as u8;
        data[offset + 7] = file_type;
        data[offset + 8..offset + 8 + name.len()].copy_from_slice(name);
        data[offset + 8 + name.len()..offset + rec_len].fill(0);
    }

    fn remove_dirent(
        &self,
        state: &mut MutableState,
        parent_number: u32,
        name: &[u8],
    ) -> Result<Dirent, FilesystemError> {
        let mut parent = self.read_inode_with_groups(parent_number, &state.groups)?;
        if !parent.is_dir() {
            return Err(FilesystemError::NotADirectory);
        }
        let bs = self.geometry.block_size as usize;
        let blocks = parent.size().div_ceil(bs as u64);
        for logical in 0..blocks {
            let physical = self.block_at(&parent, logical)?;
            let mut data = vec![0u8; bs];
            self.io.read_block(physical as u64, &mut data)?;
            let mut offset = 0usize;
            let mut previous: Option<usize> = None;
            while offset < bs {
                let child = le32(&data, offset);
                let rec_len = le16(&data, offset + 4) as usize;
                let name_len = data[offset + 6] as usize;
                if rec_len < 8
                    || rec_len % 4 != 0
                    || offset + rec_len > bs
                    || name_len > rec_len - 8
                {
                    return Err(FilesystemError::Corrupted);
                }
                if child != 0 && &data[offset + 8..offset + 8 + name_len] == name {
                    let found = Dirent {
                        inode: child,
                        name: name.to_vec(),
                    };
                    if let Some(prev) = previous {
                        let combined = le16(&data, prev + 4) as usize + rec_len;
                        put16(&mut data, prev + 4, combined as u16);
                    } else {
                        put32(&mut data, offset, 0);
                    }
                    self.io.write_block(physical as u64, &data)?;
                    parent.set_mtime_ctime(Self::now());
                    self.write_inode(&parent, &state.groups)?;
                    return Ok(found);
                }
                if child != 0 {
                    previous = Some(offset);
                }
                offset += rec_len;
            }
        }
        Err(FilesystemError::NotFound)
    }

    fn set_dirent_inode(
        &self,
        state: &MutableState,
        directory: u32,
        name: &[u8],
        new_inode: u32,
    ) -> Result<(), FilesystemError> {
        let dir = self.read_inode_with_groups(directory, &state.groups)?;
        let bs = self.geometry.block_size as usize;
        for logical in 0..dir.size().div_ceil(bs as u64) {
            let physical = self.block_at(&dir, logical)?;
            let mut data = vec![0u8; bs];
            self.io.read_block(physical as u64, &mut data)?;
            let mut offset = 0usize;
            while offset < bs {
                let rec_len = le16(&data, offset + 4) as usize;
                let name_len = data[offset + 6] as usize;
                if rec_len < 8
                    || rec_len % 4 != 0
                    || offset + rec_len > bs
                    || name_len > rec_len - 8
                {
                    return Err(FilesystemError::Corrupted);
                }
                if le32(&data, offset) != 0 && &data[offset + 8..offset + 8 + name_len] == name {
                    put32(&mut data, offset, new_inode);
                    self.io.write_block(physical as u64, &data)?;
                    return Ok(());
                }
                offset += rec_len;
            }
        }
        Err(FilesystemError::Corrupted)
    }

    fn directory_empty(&self, inode: &Inode) -> Result<bool, FilesystemError> {
        Ok(self
            .parse_directory(inode)?
            .iter()
            .all(|entry| entry.name == b"." || entry.name == b".."))
    }

    fn initialize_directory(
        &self,
        state: &mut MutableState,
        inode: &mut Inode,
        parent: u32,
    ) -> Result<(), FilesystemError> {
        let block = self.ensure_block(state, inode, 0)?;
        let bs = self.geometry.block_size as usize;
        let dot_len = 12usize;
        let mut data = vec![0u8; bs];
        Self::write_dirent_record(&mut data, 0, inode.number, dot_len, DIR_FT_DIR, b".");
        Self::write_dirent_record(&mut data, dot_len, parent, bs - dot_len, DIR_FT_DIR, b"..");
        self.io.write_block(block as u64, &data)?;
        inode.set_size(bs as u64);
        self.write_inode(inode, &state.groups)
    }

    fn inode_is_open(state: &MutableState, inode: u32) -> bool {
        state.open.values().any(|open| open.inode == inode)
    }

    fn reclaim_inode(
        &self,
        state: &mut MutableState,
        inode_number: u32,
    ) -> Result<(), FilesystemError> {
        let mut inode = self.read_inode_with_groups(inode_number, &state.groups)?;
        let directory = inode.is_dir();
        self.truncate_inode(state, &mut inode, 0)?;
        inode.set_dtime(Self::now());
        inode.set_mode(0);
        inode.set_links(0);
        self.write_inode(&inode, &state.groups)?;
        self.free_inode_bitmap(state, inode_number, directory)
    }

    fn unlink_inode_after_dirent(
        &self,
        state: &mut MutableState,
        inode_number: u32,
    ) -> Result<(), FilesystemError> {
        let mut inode = self.read_inode_with_groups(inode_number, &state.groups)?;
        if inode.links() == 0 {
            return Err(FilesystemError::Corrupted);
        }
        inode.set_links(inode.links() - 1);
        inode.set_mtime_ctime(Self::now());
        if inode.links() == 0 {
            inode.set_dtime(Self::now());
        }
        self.write_inode(&inode, &state.groups)?;
        if inode.links() == 0 && !Self::inode_is_open(state, inode_number) {
            self.reclaim_inode(state, inode_number)?;
        }
        Ok(())
    }

    fn make_entry(&self, name: &[u8], inode: &Inode) -> DirectoryEntry {
        let copy = core::cmp::min(name.len(), 255);
        let mut entry = DirectoryEntry {
            name: [0u8; 256],
            name_len: copy,
            file_type: if inode.is_dir() {
                FileType::Directory
            } else if inode.is_symlink() {
                FileType::Symlink
            } else if inode.is_file() {
                FileType::File
            } else {
                FileType::Other
            },
            size: inode.size(),
            attributes: FileAttributes {
                read_only: inode.mode() & 0o222 == 0,
                hidden: false,
                system: false,
                archive: false,
            },
            created: le32(&inode.raw, 12) as u64,
            modified: le32(&inode.raw, 16) as u64,
            accessed: le32(&inode.raw, 8) as u64,
        };
        entry.name[..copy].copy_from_slice(&name[..copy]);
        entry
    }

    fn create_regular(
        &self,
        state: &mut MutableState,
        parent: u32,
        name: &[u8],
    ) -> Result<u32, FilesystemError> {
        let number = self.allocate_inode(state, false)?;
        let mut inode = Inode::blank(number, self.geometry.inode_size as usize);
        inode.set_mode(MODE_REG | 0o644);
        inode.set_links(1);
        inode.set_times(Self::now());
        self.write_inode(&inode, &state.groups)?;
        if let Err(error) = self.insert_dirent(state, parent, number, DIR_FT_REG, name) {
            let _ = self.reclaim_inode(state, number);
            return Err(error);
        }
        Ok(number)
    }

    fn validate_not_descendant(
        &self,
        state: &MutableState,
        moved: u32,
        destination_parent: u32,
    ) -> Result<(), FilesystemError> {
        let mut current = destination_parent;
        for _ in 0..=self.geometry.inodes_count {
            if current == moved {
                return Err(FilesystemError::InvalidPath);
            }
            if current == ROOT_INODE {
                return Ok(());
            }
            current = self
                .lookup_child_with_groups(current, b"..", &state.groups)?
                .inode;
        }
        Err(FilesystemError::Corrupted)
    }
}

impl Filesystem for Ext2Filesystem<'_> {
    fn name(&self) -> &str {
        "ext2"
    }

    fn is_read_only(&self) -> bool {
        !self.writable
    }

    fn stats(&self) -> Result<FilesystemStats, FilesystemError> {
        let state = self.state.lock();
        Ok(FilesystemStats {
            total_blocks: self.geometry.blocks_count as u64,
            free_blocks: le32(&state.super_raw, 12) as u64,
            block_size: self.geometry.block_size,
            total_inodes: self.geometry.inodes_count as u64,
            free_inodes: le32(&state.super_raw, 16) as u64,
        })
    }

    fn read_dir(&self, _path: &str) -> Result<DirectoryIterator<'_>, FilesystemError> {
        Err(FilesystemError::UnsupportedOperation)
    }

    fn enumerate_dir(&self, path: &str) -> Result<Vec<DirectoryEntry>, FilesystemError> {
        let state = self.state.lock();
        let number = self.resolve_with_groups(path, &state.groups)?;
        let inode = self.read_inode_with_groups(number, &state.groups)?;
        let mut result = Vec::new();
        for dirent in self.parse_directory(&inode)? {
            if dirent.name == b"." || dirent.name == b".." {
                continue;
            }
            let child = self.read_inode_with_groups(dirent.inode, &state.groups)?;
            result.push(self.make_entry(&dirent.name, &child));
        }
        Ok(result)
    }

    fn stat(&self, path: &str) -> Result<DirectoryEntry, FilesystemError> {
        let state = self.state.lock();
        let number = self.resolve_with_groups(path, &state.groups)?;
        let inode = self.read_inode_with_groups(number, &state.groups)?;
        let name = Self::split_path(path).last().unwrap_or("/").as_bytes();
        Ok(self.make_entry(name, &inode))
    }

    fn unix_metadata(
        &self,
        path: &str,
    ) -> Result<crate::fs::filesystem::UnixMetadata, FilesystemError> {
        let state = self.state.lock();
        let number = self.resolve_with_groups(path, &state.groups)?;
        let inode = self.read_inode_with_groups(number, &state.groups)?;
        Ok(self.inode_metadata(&inode))
    }

    fn symlink_metadata(
        &self,
        path: &str,
    ) -> Result<crate::fs::filesystem::UnixMetadata, FilesystemError> {
        let state = self.state.lock();
        let number = self.resolve_no_follow_final(path, &state.groups)?;
        let inode = self.read_inode_with_groups(number, &state.groups)?;
        Ok(self.inode_metadata(&inode))
    }

    fn handle_metadata(
        &self,
        handle: &FileHandle,
    ) -> Result<crate::fs::filesystem::UnixMetadata, FilesystemError> {
        let state = self.state.lock();
        let open = state
            .open
            .get(&handle.inode)
            .ok_or(FilesystemError::IoError)?;
        let inode = self.read_inode_with_groups(open.inode, &state.groups)?;
        Ok(self.inode_metadata(&inode))
    }

    fn open(&self, path: &str, mode: FileMode) -> Result<FileHandle, FilesystemError> {
        let mut state = self.state.lock();
        let mut number = match self.resolve_with_groups(path, &state.groups) {
            Ok(number) => number,
            Err(FilesystemError::NotFound) if mode.create => {
                self.mark_dirty(&mut state)?;
                let (parent, leaf) = self.resolve_parent_with_groups(path, &state.groups)?;
                self.create_regular(&mut state, parent, leaf.as_bytes())?
            }
            Err(error) => return Err(error),
        };
        let mut inode = self.read_inode_with_groups(number, &state.groups)?;
        if inode.is_dir() {
            return Err(FilesystemError::IsADirectory);
        }
        if !inode.is_file() && !inode.is_symlink() {
            return Err(FilesystemError::UnsupportedOperation);
        }
        if mode.write && !self.writable {
            return Err(FilesystemError::ReadOnly);
        }
        if inode.is_symlink() {
            number = inode.number;
        }
        if mode.truncate && mode.write && inode.size() != 0 {
            self.mark_dirty(&mut state)?;
            self.truncate_inode(&mut state, &mut inode, 0)?;
        }
        let id = state.next_handle;
        state.next_handle = state.next_handle.wrapping_add(1).max(HANDLE_BASE);
        state.open.insert(
            id,
            OpenFile {
                inode: number,
                mode,
            },
        );
        let position = if mode.append { inode.size() } else { 0 };
        Ok(FileHandle {
            inode: id,
            position,
            size: inode.size(),
            mode,
        })
    }

    fn close(&self, handle: &mut FileHandle) -> Result<(), FilesystemError> {
        let mut state = self.state.lock();
        let open = state
            .open
            .remove(&handle.inode)
            .ok_or(FilesystemError::IoError)?;
        if self.writable {
            let inode = self.read_inode_with_groups(open.inode, &state.groups)?;
            if inode.links() == 0 && !Self::inode_is_open(&state, open.inode) {
                self.mark_dirty(&mut state)?;
                self.reclaim_inode(&mut state, open.inode)?;
            }
        }
        Ok(())
    }

    fn read(&self, handle: &mut FileHandle, buffer: &mut [u8]) -> Result<usize, FilesystemError> {
        let mut state = self.state.lock();
        let open = *state
            .open
            .get(&handle.inode)
            .ok_or(FilesystemError::IoError)?;
        if !open.mode.read {
            return Err(FilesystemError::PermissionDenied);
        }
        let inode = self.read_inode_with_groups(open.inode, &state.groups)?;
        let n = self.read_inode_data(&inode, handle.position, buffer)?;
        handle.position += n as u64;
        handle.size = inode.size();
        if let Some(slot) = state.open.get_mut(&handle.inode) {
            slot.inode = open.inode;
        }
        Ok(n)
    }

    fn write(&self, handle: &mut FileHandle, buffer: &[u8]) -> Result<usize, FilesystemError> {
        let mut state = self.state.lock();
        let open = *state
            .open
            .get(&handle.inode)
            .ok_or(FilesystemError::IoError)?;
        if !open.mode.write {
            return Err(FilesystemError::PermissionDenied);
        }
        self.mark_dirty(&mut state)?;
        let mut inode = self.read_inode_with_groups(open.inode, &state.groups)?;
        let position = if open.mode.append {
            inode.size()
        } else {
            handle.position
        };
        let n = self.write_inode_data(&mut state, &mut inode, position, buffer)?;
        handle.position = position + n as u64;
        handle.size = inode.size();
        Ok(n)
    }

    fn seek(&self, handle: &mut FileHandle, position: u64) -> Result<u64, FilesystemError> {
        let state = self.state.lock();
        if !state.open.contains_key(&handle.inode) {
            return Err(FilesystemError::IoError);
        }
        handle.position = position;
        Ok(position)
    }

    fn truncate(&self, handle: &mut FileHandle, size: u64) -> Result<(), FilesystemError> {
        let mut state = self.state.lock();
        let open = *state
            .open
            .get(&handle.inode)
            .ok_or(FilesystemError::IoError)?;
        if !open.mode.write {
            return Err(FilesystemError::PermissionDenied);
        }
        self.mark_dirty(&mut state)?;
        let mut inode = self.read_inode_with_groups(open.inode, &state.groups)?;
        self.truncate_inode(&mut state, &mut inode, size)?;
        handle.size = size;
        if handle.position > size {
            handle.position = size;
        }
        Ok(())
    }

    fn set_times(
        &self,
        path: &str,
        accessed: Option<crate::fs::filesystem::UnixTimestamp>,
        modified: Option<crate::fs::filesystem::UnixTimestamp>,
    ) -> Result<(), FilesystemError> {
        let mut state = self.state.lock();
        self.mark_dirty(&mut state)?;
        let number = self.resolve_with_groups(path, &state.groups)?;
        let mut inode = self.read_inode_with_groups(number, &state.groups)?;
        if let Some(value) = accessed {
            inode.set_accessed(value.seconds.min(u32::MAX as u64) as u32);
        }
        if let Some(value) = modified {
            inode.set_modified(value.seconds.min(u32::MAX as u64) as u32);
        }
        inode.set_changed(Self::now());
        self.write_inode(&inode, &state.groups)
    }

    fn sync_handle(&self, _handle: &FileHandle, _data_only: bool) -> Result<(), FilesystemError> {
        self.sync()
    }

    fn mkdir(&self, path: &str) -> Result<(), FilesystemError> {
        let mut state = self.state.lock();
        self.mark_dirty(&mut state)?;
        let (parent_number, leaf) = self.resolve_parent_with_groups(path, &state.groups)?;
        if self
            .lookup_child_with_groups(parent_number, leaf.as_bytes(), &state.groups)
            .is_ok()
        {
            return Err(FilesystemError::AlreadyExists);
        }
        let number = self.allocate_inode(&mut state, true)?;
        let mut inode = Inode::blank(number, self.geometry.inode_size as usize);
        inode.set_mode(MODE_DIR | 0o755);
        inode.set_links(2);
        inode.set_times(Self::now());
        self.write_inode(&inode, &state.groups)?;
        if let Err(error) = self.initialize_directory(&mut state, &mut inode, parent_number) {
            let _ = self.reclaim_inode(&mut state, number);
            return Err(error);
        }
        self.insert_dirent(
            &mut state,
            parent_number,
            number,
            DIR_FT_DIR,
            leaf.as_bytes(),
        )?;
        let mut parent = self.read_inode_with_groups(parent_number, &state.groups)?;
        parent.set_links(
            parent
                .links()
                .checked_add(1)
                .ok_or(FilesystemError::Corrupted)?,
        );
        parent.set_mtime_ctime(Self::now());
        self.write_inode(&parent, &state.groups)
    }

    fn unlink(&self, path: &str) -> Result<(), FilesystemError> {
        let mut state = self.state.lock();
        self.mark_dirty(&mut state)?;
        let (parent, leaf) = self.resolve_parent_with_groups(path, &state.groups)?;
        let entry = self.lookup_child_with_groups(parent, leaf.as_bytes(), &state.groups)?;
        let inode = self.read_inode_with_groups(entry.inode, &state.groups)?;
        if inode.is_dir() {
            return Err(FilesystemError::IsADirectory);
        }
        self.remove_dirent(&mut state, parent, leaf.as_bytes())?;
        self.unlink_inode_after_dirent(&mut state, entry.inode)
    }

    fn rmdir(&self, path: &str) -> Result<(), FilesystemError> {
        if path.trim_matches('/').is_empty() {
            return Err(FilesystemError::PermissionDenied);
        }
        let mut state = self.state.lock();
        self.mark_dirty(&mut state)?;
        let (parent_number, leaf) = self.resolve_parent_with_groups(path, &state.groups)?;
        let entry = self.lookup_child_with_groups(parent_number, leaf.as_bytes(), &state.groups)?;
        let inode = self.read_inode_with_groups(entry.inode, &state.groups)?;
        if !inode.is_dir() {
            return Err(FilesystemError::NotADirectory);
        }
        if !self.directory_empty(&inode)? {
            return Err(FilesystemError::NotEmpty);
        }
        self.remove_dirent(&mut state, parent_number, leaf.as_bytes())?;
        let mut parent = self.read_inode_with_groups(parent_number, &state.groups)?;
        parent.set_links(
            parent
                .links()
                .checked_sub(1)
                .ok_or(FilesystemError::Corrupted)?,
        );
        parent.set_mtime_ctime(Self::now());
        self.write_inode(&parent, &state.groups)?;
        let mut child = inode;
        child.set_links(0);
        child.set_dtime(Self::now());
        self.write_inode(&child, &state.groups)?;
        self.reclaim_inode(&mut state, entry.inode)
    }

    fn rename(&self, old_path: &str, new_path: &str) -> Result<(), FilesystemError> {
        if old_path == new_path {
            return Ok(());
        }
        let mut state = self.state.lock();
        self.mark_dirty(&mut state)?;
        let (old_parent, old_leaf) = self.resolve_parent_with_groups(old_path, &state.groups)?;
        let (new_parent, new_leaf) = self.resolve_parent_with_groups(new_path, &state.groups)?;
        let source =
            self.lookup_child_with_groups(old_parent, old_leaf.as_bytes(), &state.groups)?;
        let source_inode = self.read_inode_with_groups(source.inode, &state.groups)?;
        if source_inode.is_dir() {
            self.validate_not_descendant(&state, source.inode, new_parent)?;
        }
        if let Ok(destination) =
            self.lookup_child_with_groups(new_parent, new_leaf.as_bytes(), &state.groups)
        {
            if destination.inode == source.inode {
                return Ok(());
            }
            let dest_inode = self.read_inode_with_groups(destination.inode, &state.groups)?;
            if source_inode.is_dir() != dest_inode.is_dir() {
                return Err(if source_inode.is_dir() {
                    FilesystemError::NotADirectory
                } else {
                    FilesystemError::IsADirectory
                });
            }
            if dest_inode.is_dir() && !self.directory_empty(&dest_inode)? {
                return Err(FilesystemError::NotEmpty);
            }
            self.remove_dirent(&mut state, new_parent, new_leaf.as_bytes())?;
            if dest_inode.is_dir() {
                let mut parent = self.read_inode_with_groups(new_parent, &state.groups)?;
                parent.set_links(parent.links().saturating_sub(1));
                self.write_inode(&parent, &state.groups)?;
                let mut removed = dest_inode;
                removed.set_links(0);
                self.write_inode(&removed, &state.groups)?;
                self.reclaim_inode(&mut state, destination.inode)?;
            } else {
                self.unlink_inode_after_dirent(&mut state, destination.inode)?;
            }
        }
        self.insert_dirent(
            &mut state,
            new_parent,
            source.inode,
            Self::dir_file_type(&source_inode),
            new_leaf.as_bytes(),
        )?;
        self.remove_dirent(&mut state, old_parent, old_leaf.as_bytes())?;
        if source_inode.is_dir() && old_parent != new_parent {
            self.set_dirent_inode(&state, source.inode, b"..", new_parent)?;
            let mut old_parent_inode = self.read_inode_with_groups(old_parent, &state.groups)?;
            old_parent_inode.set_links(old_parent_inode.links().saturating_sub(1));
            self.write_inode(&old_parent_inode, &state.groups)?;
            let mut new_parent_inode = self.read_inode_with_groups(new_parent, &state.groups)?;
            new_parent_inode.set_links(
                new_parent_inode
                    .links()
                    .checked_add(1)
                    .ok_or(FilesystemError::Corrupted)?,
            );
            self.write_inode(&new_parent_inode, &state.groups)?;
        }
        Ok(())
    }

    fn link(&self, old_path: &str, new_path: &str) -> Result<(), FilesystemError> {
        let mut state = self.state.lock();
        self.mark_dirty(&mut state)?;
        let source = self.resolve_no_follow_final(old_path, &state.groups)?;
        let mut inode = self.read_inode_with_groups(source, &state.groups)?;
        if inode.is_dir() {
            return Err(FilesystemError::PermissionDenied);
        }
        let (parent, leaf) = self.resolve_parent_with_groups(new_path, &state.groups)?;
        self.insert_dirent(
            &mut state,
            parent,
            source,
            Self::dir_file_type(&inode),
            leaf.as_bytes(),
        )?;
        inode.set_links(
            inode
                .links()
                .checked_add(1)
                .ok_or(FilesystemError::Corrupted)?,
        );
        inode.set_mtime_ctime(Self::now());
        self.write_inode(&inode, &state.groups)
    }

    fn symlink(&self, target: &str, link_path: &str) -> Result<(), FilesystemError> {
        if target.is_empty() || target.as_bytes().contains(&0) {
            return Err(FilesystemError::InvalidPath);
        }
        let mut state = self.state.lock();
        self.mark_dirty(&mut state)?;
        let (parent, leaf) = self.resolve_parent_with_groups(link_path, &state.groups)?;
        let number = self.allocate_inode(&mut state, false)?;
        let mut inode = Inode::blank(number, self.geometry.inode_size as usize);
        inode.set_mode(MODE_SYMLINK | 0o777);
        inode.set_links(1);
        inode.set_times(Self::now());
        if target.len() <= 60 {
            inode.raw[40..40 + target.len()].copy_from_slice(target.as_bytes());
            inode.set_size(target.len() as u64);
            self.write_inode(&inode, &state.groups)?;
        } else {
            self.write_inode(&inode, &state.groups)?;
            self.write_inode_data(&mut state, &mut inode, 0, target.as_bytes())?;
        }
        if let Err(error) =
            self.insert_dirent(&mut state, parent, number, DIR_FT_SYMLINK, leaf.as_bytes())
        {
            let _ = self.reclaim_inode(&mut state, number);
            return Err(error);
        }
        Ok(())
    }

    fn read_link(&self, path: &str) -> Result<Vec<u8>, FilesystemError> {
        let state = self.state.lock();
        let number = self.resolve_no_follow_final(path, &state.groups)?;
        let inode = self.read_inode_with_groups(number, &state.groups)?;
        self.read_symlink_inode(&inode)
    }

    fn sync(&self) -> Result<(), FilesystemError> {
        let mut state = self.state.lock();
        if self.writable && state.dirty {
            self.io.flush()?;
            let value = le16(&state.super_raw, 58) | EXT2_VALID_FS;
            put16(&mut state.super_raw, 58, value);
            put32(&mut state.super_raw, 48, Self::now());
            self.write_super(&state)?;
            self.io.flush()?;
            state.dirty = false;
        }
        Ok(())
    }
}

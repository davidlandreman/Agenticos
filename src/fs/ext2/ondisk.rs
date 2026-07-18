use alloc::vec::Vec;

use crate::drivers::block::BlockDevice;
use crate::fs::block_io::BlockIo;
use crate::fs::filesystem::{FilesystemError, FilesystemType};

pub const EXT2_MAGIC: u16 = 0xef53;
pub const EXT2_VALID_FS: u16 = 0x0001;
pub const FEATURE_COMPAT_HAS_JOURNAL: u32 = 0x0004;
pub const FEATURE_COMPAT_DIR_INDEX: u32 = 0x0020;
pub const FEATURE_INCOMPAT_FILETYPE: u32 = 0x0002;
pub const FEATURE_INCOMPAT_RECOVER: u32 = 0x0004;
pub const FEATURE_INCOMPAT_EXTENTS: u32 = 0x0040;
pub const FEATURE_INCOMPAT_64BIT: u32 = 0x0080;
pub const FEATURE_INCOMPAT_INLINE_DATA: u32 = 0x8000;
pub const FEATURE_RO_COMPAT_SPARSE_SUPER: u32 = 0x0001;
pub const FEATURE_RO_COMPAT_LARGE_FILE: u32 = 0x0002;
pub const FEATURE_RO_COMPAT_BIGALLOC: u32 = 0x0200;
pub const FEATURE_RO_COMPAT_METADATA_CSUM: u32 = 0x0400;

const SUPPORTED_COMPAT_RW: u32 = 0;
const SUPPORTED_INCOMPAT: u32 = FEATURE_INCOMPAT_FILETYPE;
const SUPPORTED_RO_COMPAT_RW: u32 = FEATURE_RO_COMPAT_SPARSE_SUPER | FEATURE_RO_COMPAT_LARGE_FILE;

#[inline]
pub fn le16(bytes: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([bytes[off], bytes[off + 1]])
}

#[inline]
pub fn le32(bytes: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
}

#[inline]
pub fn put16(bytes: &mut [u8], off: usize, value: u16) {
    bytes[off..off + 2].copy_from_slice(&value.to_le_bytes());
}

#[inline]
pub fn put32(bytes: &mut [u8], off: usize, value: u32) {
    bytes[off..off + 4].copy_from_slice(&value.to_le_bytes());
}

#[derive(Clone)]
pub struct ExtGeometry {
    pub blocks_count: u32,
    pub inodes_count: u32,
    pub reserved_blocks: u32,
    pub first_data_block: u32,
    pub block_size: u32,
    pub blocks_per_group: u32,
    pub inodes_per_group: u32,
    pub inode_size: u16,
    pub first_inode: u32,
    pub group_count: u32,
    pub desc_table_block: u32,
    pub compat: u32,
    pub incompat: u32,
    pub ro_compat: u32,
}

impl ExtGeometry {
    pub fn parse(device: &dyn BlockDevice) -> Result<(Self, [u8; 1024]), FilesystemError> {
        let probe = BlockIo::new(device, device.block_size())?;
        let mut raw = [0u8; 1024];
        probe.read_bytes(1024, &mut raw)?;
        if le16(&raw, 56) != EXT2_MAGIC {
            return Err(FilesystemError::InvalidFilesystem);
        }
        let log_block_size = le32(&raw, 24);
        if log_block_size > 2 {
            return Err(FilesystemError::UnsupportedFeature);
        }
        let block_size = 1024u32
            .checked_shl(log_block_size)
            .ok_or(FilesystemError::InvalidFilesystem)?;
        if block_size < 1024 || block_size > 4096 || block_size % device.block_size() != 0 {
            return Err(FilesystemError::UnsupportedFeature);
        }
        let blocks_count = le32(&raw, 4);
        let inodes_count = le32(&raw, 0);
        let first_data_block = le32(&raw, 20);
        let blocks_per_group = le32(&raw, 32);
        let inodes_per_group = le32(&raw, 40);
        if blocks_count == 0
            || inodes_count == 0
            || blocks_per_group == 0
            || inodes_per_group == 0
            || first_data_block >= blocks_count
        {
            return Err(FilesystemError::InvalidFilesystem);
        }
        let rev = le32(&raw, 76);
        let inode_size = if rev == 0 { 128 } else { le16(&raw, 88) };
        let first_inode = if rev == 0 { 11 } else { le32(&raw, 84) };
        if !(128..=256).contains(&inode_size) || inode_size % 4 != 0 {
            return Err(FilesystemError::UnsupportedFeature);
        }
        let block_groups = (blocks_count - first_data_block).div_ceil(blocks_per_group);
        let inode_groups = inodes_count.div_ceil(inodes_per_group);
        if block_groups == 0 || block_groups != inode_groups {
            return Err(FilesystemError::Corrupted);
        }
        let capacity_blocks = device.capacity() / block_size as u64;
        if blocks_count as u64 > capacity_blocks {
            return Err(FilesystemError::Corrupted);
        }
        let compat = le32(&raw, 92);
        let incompat = le32(&raw, 96);
        let ro_compat = le32(&raw, 100);
        Ok((
            Self {
                blocks_count,
                inodes_count,
                reserved_blocks: le32(&raw, 8),
                first_data_block,
                block_size,
                blocks_per_group,
                inodes_per_group,
                inode_size,
                first_inode,
                group_count: block_groups,
                desc_table_block: first_data_block + 1,
                compat,
                incompat,
                ro_compat,
            },
            raw,
        ))
    }

    pub fn validate_features(&self, writable: bool) -> Result<(), FilesystemError> {
        if self.incompat & !SUPPORTED_INCOMPAT != 0
            || self.incompat
                & (FEATURE_INCOMPAT_RECOVER
                    | FEATURE_INCOMPAT_EXTENTS
                    | FEATURE_INCOMPAT_64BIT
                    | FEATURE_INCOMPAT_INLINE_DATA)
                != 0
        {
            return Err(FilesystemError::UnsupportedFeature);
        }
        if writable
            && (self.compat & !SUPPORTED_COMPAT_RW != 0
                || self.ro_compat & !SUPPORTED_RO_COMPAT_RW != 0
                || self.compat & (FEATURE_COMPAT_HAS_JOURNAL | FEATURE_COMPAT_DIR_INDEX) != 0
                || self.ro_compat & (FEATURE_RO_COMPAT_BIGALLOC | FEATURE_RO_COMPAT_METADATA_CSUM)
                    != 0)
        {
            return Err(FilesystemError::UnsupportedFeature);
        }
        Ok(())
    }

    pub fn group_start(&self, group: u32) -> Result<u32, FilesystemError> {
        if group >= self.group_count {
            return Err(FilesystemError::Corrupted);
        }
        self.first_data_block
            .checked_add(
                group
                    .checked_mul(self.blocks_per_group)
                    .ok_or(FilesystemError::Corrupted)?,
            )
            .ok_or(FilesystemError::Corrupted)
    }

    pub fn valid_block(&self, block: u32) -> bool {
        block >= self.first_data_block && block < self.blocks_count
    }
}

#[derive(Clone)]
pub struct GroupDesc {
    pub raw: [u8; 32],
}

impl GroupDesc {
    pub fn block_bitmap(&self) -> u32 {
        le32(&self.raw, 0)
    }
    pub fn inode_bitmap(&self) -> u32 {
        le32(&self.raw, 4)
    }
    pub fn inode_table(&self) -> u32 {
        le32(&self.raw, 8)
    }
    pub fn free_blocks(&self) -> u16 {
        le16(&self.raw, 12)
    }
    pub fn set_free_blocks(&mut self, n: u16) {
        put16(&mut self.raw, 12, n);
    }
    pub fn free_inodes(&self) -> u16 {
        le16(&self.raw, 14)
    }
    pub fn set_free_inodes(&mut self, n: u16) {
        put16(&mut self.raw, 14, n);
    }
    pub fn used_dirs(&self) -> u16 {
        le16(&self.raw, 16)
    }
    pub fn set_used_dirs(&mut self, n: u16) {
        put16(&mut self.raw, 16, n);
    }
}

pub fn read_groups(
    io: &BlockIo<'_>,
    geom: &ExtGeometry,
) -> Result<Vec<GroupDesc>, FilesystemError> {
    let mut groups = Vec::with_capacity(geom.group_count as usize);
    let base = geom.desc_table_block as u64 * geom.block_size as u64;
    for i in 0..geom.group_count {
        let mut raw = [0u8; 32];
        io.read_bytes(base + i as u64 * 32, &mut raw)?;
        let desc = GroupDesc { raw };
        for block in [desc.block_bitmap(), desc.inode_bitmap(), desc.inode_table()] {
            if !geom.valid_block(block) {
                return Err(FilesystemError::Corrupted);
            }
        }
        groups.push(desc);
    }
    Ok(groups)
}

pub fn classify_ext(device: &dyn BlockDevice) -> Result<FilesystemType, FilesystemError> {
    let (geom, _) = ExtGeometry::parse(device)?;
    if geom.incompat
        & (FEATURE_INCOMPAT_EXTENTS | FEATURE_INCOMPAT_64BIT | FEATURE_INCOMPAT_INLINE_DATA)
        != 0
        || geom.ro_compat & (FEATURE_RO_COMPAT_BIGALLOC | FEATURE_RO_COMPAT_METADATA_CSUM) != 0
    {
        Ok(FilesystemType::Ext4)
    } else if geom.compat & FEATURE_COMPAT_HAS_JOURNAL != 0 {
        Ok(FilesystemType::Ext3)
    } else {
        Ok(FilesystemType::Ext2)
    }
}

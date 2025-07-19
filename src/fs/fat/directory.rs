use crate::fs::fat::types::{FileAttributes, ClusterId, FatError};
use core::mem::size_of;

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct DirectoryEntry {
    pub name: [u8; 8],
    pub extension: [u8; 3],
    pub attributes: u8,
    pub reserved: u8,
    pub creation_time_tenth: u8,
    pub creation_time: u16,
    pub creation_date: u16,
    pub last_access_date: u16,
    pub first_cluster_high: u16,
    pub write_time: u16,
    pub write_date: u16,
    pub first_cluster_low: u16,
    pub file_size: u32,
}

impl DirectoryEntry {
    pub const SIZE: usize = 32;
    pub const ENTRY_FREE: u8 = 0xE5;
    pub const ENTRY_END: u8 = 0x00;
    
    pub fn from_bytes(data: &[u8]) -> Result<&Self, FatError> {
        if data.len() < Self::SIZE {
            return Err(FatError::InvalidDirectoryEntry);
        }
        
        let entry = unsafe { &*(data.as_ptr() as *const Self) };
        Ok(entry)
    }
    
    pub fn is_free(&self) -> bool {
        self.name[0] == Self::ENTRY_FREE
    }
    
    pub fn is_end(&self) -> bool {
        self.name[0] == Self::ENTRY_END
    }
    
    pub fn is_valid(&self) -> bool {
        !self.is_free() && !self.is_end() && self.name[0] != 0
    }
    
    pub fn attributes(&self) -> FileAttributes {
        FileAttributes::new(self.attributes)
    }
    
    pub fn first_cluster(&self) -> ClusterId {
        let cluster = (self.first_cluster_high as u32) << 16 | self.first_cluster_low as u32;
        ClusterId(cluster)
    }
    
    pub fn short_name(&self) -> [u8; 11] {
        let mut result = [0u8; 11];
        result[..8].copy_from_slice(&self.name);
        result[8..].copy_from_slice(&self.extension);
        result
    }
    
    pub fn format_name(&self) -> [u8; 13] {
        let mut result = [b' '; 13];
        let mut pos = 0;
        
        // Copy base name, removing trailing spaces
        for &byte in &self.name {
            if byte != b' ' {
                result[pos] = byte;
                pos += 1;
            } else {
                break;
            }
        }
        
        // Add extension if present
        if self.extension[0] != b' ' {
            result[pos] = b'.';
            pos += 1;
            
            for &byte in &self.extension {
                if byte != b' ' {
                    result[pos] = byte;
                    pos += 1;
                } else {
                    break;
                }
            }
        }
        
        // Null terminate
        if pos < 13 {
            result[pos] = 0;
        }
        
        result
    }
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct LongFileNameEntry {
    pub order: u8,
    pub name1: [u16; 5],
    pub attributes: u8,
    pub entry_type: u8,
    pub checksum: u8,
    pub name2: [u16; 6],
    pub first_cluster: u16,
    pub name3: [u16; 2],
}

impl LongFileNameEntry {
    pub const LAST_ENTRY_MASK: u8 = 0x40;
    
    pub fn from_bytes(data: &[u8]) -> Result<&Self, FatError> {
        if data.len() < size_of::<Self>() {
            return Err(FatError::InvalidDirectoryEntry);
        }
        
        let entry = unsafe { &*(data.as_ptr() as *const Self) };
        Ok(entry)
    }
    
    pub fn is_last(&self) -> bool {
        self.order & Self::LAST_ENTRY_MASK != 0
    }
    
    pub fn sequence_number(&self) -> u8 {
        self.order & !Self::LAST_ENTRY_MASK
    }
    
    pub fn chars(&self) -> [u16; 13] {
        let mut chars = [0u16; 13];
        // Copy name1
        for i in 0..5 {
            chars[i] = unsafe { 
                let ptr = (self as *const Self as *const u8).add(1 + i * 2) as *const u16;
                core::ptr::read_unaligned(ptr)
            };
        }
        // Copy name2
        for i in 0..6 {
            chars[5 + i] = unsafe {
                let ptr = (self as *const Self as *const u8).add(14 + i * 2) as *const u16;
                core::ptr::read_unaligned(ptr)
            };
        }
        // Copy name3
        for i in 0..2 {
            chars[11 + i] = unsafe {
                let ptr = (self as *const Self as *const u8).add(28 + i * 2) as *const u16;
                core::ptr::read_unaligned(ptr)
            };
        }
        chars
    }
}

pub struct DirectoryIterator<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> DirectoryIterator<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }
}

impl<'a> Iterator for DirectoryIterator<'a> {
    type Item = Result<&'a DirectoryEntry, FatError>;
    
    fn next(&mut self) -> Option<Self::Item> {
        while self.offset + DirectoryEntry::SIZE <= self.data.len() {
            let entry_data = &self.data[self.offset..];
            self.offset += DirectoryEntry::SIZE;
            
            match DirectoryEntry::from_bytes(entry_data) {
                Ok(entry) => {
                    if entry.is_end() {
                        return None;
                    }
                    
                    if entry.is_valid() && !entry.attributes().is_lfn() {
                        return Some(Ok(entry));
                    }
                }
                Err(e) => return Some(Err(e)),
            }
        }
        
        None
    }
}
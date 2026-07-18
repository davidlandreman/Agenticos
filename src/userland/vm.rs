//! Per-address-space virtual-memory areas.

use alloc::vec::Vec;
use core::fmt;

use crate::fs::File;
use crate::lib::arc::Arc;
use crate::mm::paging::{is_kernel_reserved_slot, USER_CANONICAL_END, USER_LOAD_BASE};

pub const PAGE_SIZE: u64 = 0x1000;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct VmProt(u8);

impl VmProt {
    pub const NONE: Self = Self(0);
    pub const READ: Self = Self(1);
    pub const WRITE: Self = Self(2);
    pub const EXEC: Self = Self(4);

    pub const fn from_bits(bits: u8) -> Option<Self> {
        if bits & !7 == 0 {
            Some(Self(bits))
        } else {
            None
        }
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl fmt::Debug for VmProt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}{}{}",
            if self.contains(Self::READ) { 'r' } else { '-' },
            if self.contains(Self::WRITE) { 'w' } else { '-' },
            if self.contains(Self::EXEC) { 'x' } else { '-' }
        )
    }
}

#[derive(Clone)]
pub enum VmaBacking {
    ElfResident,
    Elf {
        file: Arc<File>,
        file_offset: u64,
        file_len: u64,
        zero_tail: u64,
    },
    Tls,
    Stack {
        floor: u64,
        guard_bytes: u64,
    },
    Heap,
    Anonymous,
    FilePrivate {
        file: Arc<File>,
        file_offset: u64,
        file_size: u64,
    },
}

impl fmt::Debug for VmaBacking {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ElfResident => f.write_str("ElfResident"),
            Self::Elf {
                file_offset,
                file_len,
                zero_tail,
                ..
            } => f
                .debug_struct("Elf")
                .field("file_offset", file_offset)
                .field("file_len", file_len)
                .field("zero_tail", zero_tail)
                .finish(),
            Self::Tls => f.write_str("Tls"),
            Self::Stack { floor, guard_bytes } => f
                .debug_struct("Stack")
                .field("floor", floor)
                .field("guard_bytes", guard_bytes)
                .finish(),
            Self::Heap => f.write_str("Heap"),
            Self::Anonymous => f.write_str("Anonymous"),
            Self::FilePrivate {
                file_offset,
                file_size,
                ..
            } => f
                .debug_struct("FilePrivate")
                .field("file_offset", file_offset)
                .field("file_size", file_size)
                .finish(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Vma {
    pub start: u64,
    pub end: u64,
    pub prot: VmProt,
    pub private: bool,
    pub grow_down: bool,
    pub backing: VmaBacking,
}

impl Vma {
    pub fn new(start: u64, end: u64, prot: VmProt, backing: VmaBacking) -> Result<Self, VmError> {
        validate_range(start, end)?;
        Ok(Self {
            start,
            end,
            prot,
            private: true,
            grow_down: matches!(backing, VmaBacking::Stack { .. }),
            backing,
        })
    }

    fn split_right(&self, start: u64) -> Self {
        let mut right = self.clone();
        if let VmaBacking::FilePrivate { file_offset, .. } | VmaBacking::Elf { file_offset, .. } =
            &mut right.backing
        {
            *file_offset += start - self.start;
        }
        right.start = start;
        right
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmError {
    InvalidRange,
    ReservedRange,
    Overlap,
    NotCovered,
    NoGap,
}

#[derive(Debug, Clone, Default)]
pub struct VmaSet {
    entries: Vec<Vma>,
}

impl VmaSet {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn as_slice(&self) -> &[Vma] {
        &self.entries
    }

    pub fn find(&self, address: u64) -> Option<&Vma> {
        let index = self.entries.partition_point(|vma| vma.end <= address);
        self.entries
            .get(index)
            .filter(|vma| vma.start <= address && address < vma.end)
    }

    pub fn is_free(&self, start: u64, end: u64) -> bool {
        if validate_range(start, end).is_err() {
            return false;
        }
        let index = self.entries.partition_point(|vma| vma.end <= start);
        self.entries.get(index).is_none_or(|vma| vma.start >= end)
    }

    pub fn insert(&mut self, vma: Vma) -> Result<(), VmError> {
        validate_range(vma.start, vma.end)?;
        let index = self.entries.partition_point(|old| old.start < vma.start);
        if index > 0 && self.entries[index - 1].end > vma.start {
            return Err(VmError::Overlap);
        }
        if self
            .entries
            .get(index)
            .is_some_and(|next| vma.end > next.start)
        {
            return Err(VmError::Overlap);
        }
        self.entries.insert(index, vma);
        self.merge_around(index);
        Ok(())
    }

    pub fn covers(&self, start: u64, len: u64, access: VmProt) -> bool {
        if len == 0 {
            return true;
        }
        let Some(end) = start.checked_add(len) else {
            return false;
        };
        if validate_range(start & !(PAGE_SIZE - 1), align_up(end)).is_err() {
            return false;
        }
        let mut cursor = start;
        let mut index = self.entries.partition_point(|vma| vma.end <= start);
        while cursor < end {
            let Some(vma) = self.entries.get(index) else {
                return false;
            };
            if vma.start > cursor || !vma.prot.contains(access) {
                return false;
            }
            cursor = vma.end.min(end);
            index += 1;
        }
        true
    }

    /// Remove every intersection with `[start, end)`, splitting retained
    /// prefix/suffix pieces. Holes are intentionally tolerated.
    pub fn remove(&mut self, start: u64, end: u64) -> Result<(), VmError> {
        validate_range(start, end)?;
        let mut rebuilt = Vec::with_capacity(self.entries.len() + 1);
        for vma in self.entries.drain(..) {
            if vma.end <= start || vma.start >= end {
                rebuilt.push(vma);
                continue;
            }
            if vma.start < start {
                let mut left = vma.clone();
                left.end = start;
                rebuilt.push(left);
            }
            if vma.end > end {
                rebuilt.push(vma.split_right(end));
            }
        }
        self.entries = rebuilt;
        Ok(())
    }

    pub fn protect(&mut self, start: u64, end: u64, prot: VmProt) -> Result<(), VmError> {
        validate_range(start, end)?;
        if !self.covers(start, end - start, VmProt::NONE) {
            return Err(VmError::NotCovered);
        }
        let mut rebuilt = Vec::with_capacity(self.entries.len() + 2);
        for vma in self.entries.drain(..) {
            if vma.end <= start || vma.start >= end {
                rebuilt.push(vma);
                continue;
            }
            if vma.start < start {
                let mut left = vma.clone();
                left.end = start;
                rebuilt.push(left);
            }
            let middle_start = vma.start.max(start);
            let middle_end = vma.end.min(end);
            let mut middle = vma.split_right(middle_start);
            middle.end = middle_end;
            middle.prot = prot;
            rebuilt.push(middle);
            if vma.end > end {
                rebuilt.push(vma.split_right(end));
            }
        }
        self.entries = rebuilt;
        self.merge_all();
        Ok(())
    }

    pub fn find_gap_top_down(&self, len: u64, ceiling: u64) -> Result<u64, VmError> {
        let len = align_up(len);
        if len == 0 || ceiling > USER_CANONICAL_END {
            return Err(VmError::InvalidRange);
        }
        let mut cursor = ceiling & !(PAGE_SIZE - 1);
        for vma in self.entries.iter().rev().filter(|vma| vma.start < ceiling) {
            let gap_floor = vma.end.min(cursor);
            if cursor.saturating_sub(gap_floor) >= len {
                let candidate = cursor - len;
                if validate_range(candidate, cursor).is_ok() {
                    return Ok(candidate);
                }
            }
            cursor = cursor.min(vma.start);
        }
        if cursor >= USER_LOAD_BASE + len {
            let candidate = cursor - len;
            if validate_range(candidate, cursor).is_ok() {
                return Ok(candidate);
            }
        }
        Err(VmError::NoGap)
    }

    fn merge_all(&mut self) {
        let mut index = 0;
        while index + 1 < self.entries.len() {
            if mergeable(&self.entries[index], &self.entries[index + 1]) {
                let end = self.entries[index + 1].end;
                self.entries[index].end = end;
                self.entries.remove(index + 1);
            } else {
                index += 1;
            }
        }
    }

    fn merge_around(&mut self, _index: usize) {
        self.merge_all();
    }
}

fn mergeable(left: &Vma, right: &Vma) -> bool {
    if left.end != right.start
        || left.prot != right.prot
        || left.private != right.private
        || left.grow_down != right.grow_down
    {
        return false;
    }
    match (&left.backing, &right.backing) {
        (VmaBacking::Tls, VmaBacking::Tls)
        | (VmaBacking::ElfResident, VmaBacking::ElfResident)
        | (VmaBacking::Heap, VmaBacking::Heap)
        | (VmaBacking::Anonymous, VmaBacking::Anonymous) => true,
        (
            VmaBacking::FilePrivate {
                file: left_file,
                file_offset: left_offset,
                ..
            },
            VmaBacking::FilePrivate {
                file: right_file,
                file_offset: right_offset,
                ..
            },
        ) => {
            Arc::ptr_eq(left_file, right_file)
                && *right_offset == *left_offset + (left.end - left.start)
        }
        _ => false,
    }
}

pub fn validate_range(start: u64, end: u64) -> Result<(), VmError> {
    if start < USER_LOAD_BASE
        || start >= end
        || start & (PAGE_SIZE - 1) != 0
        || end & (PAGE_SIZE - 1) != 0
        || end > USER_CANONICAL_END
    {
        return Err(VmError::InvalidRange);
    }
    let first_slot = (start >> 39) as usize;
    let last_slot = ((end - 1) >> 39) as usize;
    if (first_slot..=last_slot).any(is_kernel_reserved_slot) {
        return Err(VmError::ReservedRange);
    }
    Ok(())
}

pub const fn align_up(value: u64) -> u64 {
    value.saturating_add(PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

//! Reusable physical-frame allocator.
//!
//! The bootloader memory map is the source of truth for managed frames.  A
//! compact bitmap and refcount array are indexed by a frame's ordinal within
//! usable regions, so sparse high physical regions do not create metadata
//! holes.  The metadata itself is bootstrapped from one contiguous usable
//! extent before the Rust heap exists and is permanently pinned.

use bootloader_api::info::{MemoryRegion, MemoryRegionKind, MemoryRegions};
use core::sync::atomic::{AtomicIsize, Ordering};
use x86_64::structures::paging::{FrameAllocator, FrameDeallocator, PhysFrame, Size4KiB};
use x86_64::{PhysAddr, VirtAddr};

use crate::{debug_error, debug_info, debug_trace};

const PAGE_SIZE: u64 = 0x1000;
const PINNED_REFCOUNT: u32 = u32::MAX;
const SUMMARY_INTERVAL: u64 = 256;

/// Stable allocator counters used by diagnostics and leak tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameStats {
    pub total_usable: u64,
    pub pinned: u64,
    /// Managed, non-pinned frames with a positive reference count.
    pub allocated: u64,
    /// Allocated frames whose reference count is greater than one.
    pub shared: u64,
    pub free: u64,
}

impl FrameStats {
    pub const fn exclusive(self) -> u64 {
        self.allocated - self.shared
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameReleaseError {
    Unmanaged,
    AlreadyFree,
    Pinned,
    RefcountOverflow,
}

/// Test-only allocation failure boundary. `-1` means disabled; `0` fails the
/// next allocation; positive values count successful allocations remaining.
static FAIL_AFTER: AtomicIsize = AtomicIsize::new(-1);

#[cfg(feature = "test")]
pub struct AllocationFailureGuard;

#[cfg(feature = "test")]
impl Drop for AllocationFailureGuard {
    fn drop(&mut self) {
        FAIL_AFTER.store(-1, Ordering::SeqCst);
    }
}

#[cfg(feature = "test")]
pub fn fail_allocations_after(successes: usize) -> AllocationFailureGuard {
    FAIL_AFTER.store(successes as isize, Ordering::SeqCst);
    AllocationFailureGuard
}

fn injected_failure() -> bool {
    let mut remaining = FAIL_AFTER.load(Ordering::Relaxed);
    loop {
        if remaining < 0 {
            return false;
        }
        if remaining == 0 {
            return true;
        }
        match FAIL_AFTER.compare_exchange_weak(
            remaining,
            remaining - 1,
            Ordering::SeqCst,
            Ordering::Relaxed,
        ) {
            Ok(_) => return false,
            Err(actual) => remaining = actual,
        }
    }
}

/// Frame allocator backed by pre-heap bitmap/refcount metadata.
pub struct BootInfoFrameAllocator {
    memory_map: &'static MemoryRegions,
    bitmap: &'static mut [u64],
    refcounts: &'static mut [u32],
    next_word: usize,
    stats: FrameStats,
    allocations: u64,
}

impl BootInfoFrameAllocator {
    /// Initialize allocator metadata through the bootloader physical-memory
    /// alias. The selected metadata extent is removed from normal allocation.
    ///
    /// # Safety
    /// `memory_map` and the physical-memory alias must remain valid for the
    /// kernel lifetime, and this must be the sole allocator initialized over
    /// these usable regions.
    pub unsafe fn init(memory_map: &'static MemoryRegions, phys_offset: VirtAddr) -> Self {
        let total_frames = usable_frame_count(memory_map);
        assert!(
            total_frames > 0,
            "physical frame allocator has no usable frames"
        );

        let bitmap_words = total_frames.div_ceil(64) as usize;
        let bitmap_bytes = bitmap_words * core::mem::size_of::<u64>();
        let refcount_offset = align_up(bitmap_bytes as u64, 8) as usize;
        let metadata_bytes = refcount_offset
            .checked_add(total_frames as usize * core::mem::size_of::<u32>())
            .expect("frame allocator metadata size overflow");
        let metadata_pages = (metadata_bytes as u64).div_ceil(PAGE_SIZE);
        let metadata_start = find_metadata_extent(memory_map, metadata_pages).unwrap_or_else(|| {
            let largest = largest_usable_extent(memory_map);
            panic!(
                "frame allocator needs {} metadata bytes ({} pages), largest usable extent is {} bytes",
                metadata_bytes,
                metadata_pages,
                largest
            )
        });

        let metadata_va = phys_offset.as_u64() + metadata_start;
        core::ptr::write_bytes(
            metadata_va as *mut u8,
            0,
            (metadata_pages * PAGE_SIZE) as usize,
        );
        let bitmap = core::slice::from_raw_parts_mut(metadata_va as *mut u64, bitmap_words);
        let refcounts = core::slice::from_raw_parts_mut(
            (metadata_va as usize + refcount_offset) as *mut u32,
            total_frames as usize,
        );

        let mut allocator = Self {
            memory_map,
            bitmap,
            refcounts,
            next_word: 0,
            stats: FrameStats {
                total_usable: total_frames,
                pinned: 0,
                allocated: 0,
                shared: 0,
                free: total_frames,
            },
            allocations: 0,
        };

        // Null must never be returned even if firmware labels it usable.
        if let Some(index) = allocator.frame_index(PhysFrame::containing_address(PhysAddr::zero()))
        {
            allocator.pin_index(index);
        }
        for page in 0..metadata_pages {
            let frame =
                PhysFrame::containing_address(PhysAddr::new(metadata_start + page * PAGE_SIZE));
            allocator
                .pin_frame(frame)
                .expect("metadata extent must be usable");
        }

        debug_info!(
            "frame allocator: {} usable, {} metadata pages pinned, {} free",
            allocator.stats.total_usable,
            metadata_pages,
            allocator.stats.free
        );
        allocator
    }

    pub fn stats(&self) -> FrameStats {
        self.stats
    }

    #[cfg(feature = "test")]
    pub fn frames_issued(&self) -> u64 {
        self.allocations
    }

    pub fn refcount(&self, frame: PhysFrame<Size4KiB>) -> Option<u32> {
        let count = *self.refcounts.get(self.frame_index(frame)?)?;
        (count != PINNED_REFCOUNT).then_some(count)
    }

    pub fn retain_frame(&mut self, frame: PhysFrame<Size4KiB>) -> Result<u32, FrameReleaseError> {
        let index = self
            .frame_index(frame)
            .ok_or(FrameReleaseError::Unmanaged)?;
        let count = self.refcounts[index];
        match count {
            0 => Err(FrameReleaseError::AlreadyFree),
            PINNED_REFCOUNT => Err(FrameReleaseError::Pinned),
            count if count == u32::MAX - 1 => Err(FrameReleaseError::RefcountOverflow),
            _ => {
                let new_count = count + 1;
                self.refcounts[index] = new_count;
                if count == 1 {
                    self.stats.shared += 1;
                }
                Ok(new_count)
            }
        }
    }

    pub fn release_frame(&mut self, frame: PhysFrame<Size4KiB>) -> Result<u32, FrameReleaseError> {
        let index = self
            .frame_index(frame)
            .ok_or(FrameReleaseError::Unmanaged)?;
        let count = self.refcounts[index];
        match count {
            0 => Err(FrameReleaseError::AlreadyFree),
            PINNED_REFCOUNT => Err(FrameReleaseError::Pinned),
            _ => {
                let new_count = count - 1;
                self.refcounts[index] = new_count;
                if count == 2 {
                    self.stats.shared -= 1;
                }
                if new_count == 0 {
                    self.set_allocated(index, false);
                    self.stats.allocated -= 1;
                    self.stats.free += 1;
                    self.next_word = self.next_word.min(index / 64);
                }
                Ok(new_count)
            }
        }
    }

    pub fn pin_frame(&mut self, frame: PhysFrame<Size4KiB>) -> Result<(), FrameReleaseError> {
        let index = self
            .frame_index(frame)
            .ok_or(FrameReleaseError::Unmanaged)?;
        if self.refcounts[index] == PINNED_REFCOUNT {
            return Ok(());
        }
        if self.refcounts[index] != 0 {
            return Err(FrameReleaseError::RefcountOverflow);
        }
        self.pin_index(index);
        Ok(())
    }

    fn pin_index(&mut self, index: usize) {
        self.set_allocated(index, true);
        self.refcounts[index] = PINNED_REFCOUNT;
        self.stats.pinned += 1;
        self.stats.free -= 1;
    }

    fn set_allocated(&mut self, index: usize, allocated: bool) {
        let mask = 1u64 << (index % 64);
        if allocated {
            self.bitmap[index / 64] |= mask;
        } else {
            self.bitmap[index / 64] &= !mask;
        }
    }

    fn frame_index(&self, frame: PhysFrame<Size4KiB>) -> Option<usize> {
        compact_frame_index(self.memory_map, frame.start_address().as_u64())
    }

    fn frame_for_index(&self, index: usize) -> Option<PhysFrame<Size4KiB>> {
        compact_index_frame(self.memory_map, index)
    }
}

unsafe impl FrameAllocator<Size4KiB> for BootInfoFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        if injected_failure() || self.stats.free == 0 {
            return None;
        }
        let words = self.bitmap.len();
        for scanned in 0..words {
            let word_index = (self.next_word + scanned) % words;
            let free_bits = !self.bitmap[word_index];
            if free_bits == 0 {
                continue;
            }
            let bit = free_bits.trailing_zeros() as usize;
            let index = word_index * 64 + bit;
            if index >= self.refcounts.len() {
                continue;
            }
            self.set_allocated(index, true);
            self.refcounts[index] = 1;
            self.stats.allocated += 1;
            self.stats.free -= 1;
            self.next_word = word_index;
            self.allocations += 1;
            let frame = self
                .frame_for_index(index)
                .expect("bitmap index must map to a frame");
            debug_trace!("allocated frame {:?}", frame.start_address());
            if self.allocations.is_multiple_of(SUMMARY_INTERVAL) {
                debug_info!(
                    "frame allocator: {} allocations, {} free, {} shared",
                    self.allocations,
                    self.stats.free,
                    self.stats.shared
                );
            }
            return Some(frame);
        }
        None
    }
}

impl FrameDeallocator<Size4KiB> for BootInfoFrameAllocator {
    unsafe fn deallocate_frame(&mut self, frame: PhysFrame<Size4KiB>) {
        if let Err(error) = self.release_frame(frame) {
            debug_error!(
                "refused invalid frame deallocation {:?}: {:?}",
                frame,
                error
            );
            debug_assert!(false, "invalid frame deallocation: {:?}", error);
        }
    }
}

fn usable_bounds(region: &MemoryRegion) -> Option<(u64, u64)> {
    if region.kind != MemoryRegionKind::Usable {
        return None;
    }
    let start = align_up(region.start.max(PAGE_SIZE), PAGE_SIZE);
    let end = region.end & !(PAGE_SIZE - 1);
    (start < end).then_some((start, end))
}

fn usable_frame_count(regions: &[MemoryRegion]) -> u64 {
    regions
        .iter()
        .filter_map(usable_bounds)
        .map(|(start, end)| (end - start) / PAGE_SIZE)
        .sum()
}

fn compact_frame_index(regions: &[MemoryRegion], address: u64) -> Option<usize> {
    let mut base = 0usize;
    for region in regions {
        let Some((start, end)) = usable_bounds(region) else {
            continue;
        };
        if address >= start && address < end && address & (PAGE_SIZE - 1) == 0 {
            return Some(base + ((address - start) / PAGE_SIZE) as usize);
        }
        base += ((end - start) / PAGE_SIZE) as usize;
    }
    None
}

fn compact_index_frame(regions: &[MemoryRegion], mut index: usize) -> Option<PhysFrame<Size4KiB>> {
    for region in regions {
        let Some((start, end)) = usable_bounds(region) else {
            continue;
        };
        let count = ((end - start) / PAGE_SIZE) as usize;
        if index < count {
            return Some(PhysFrame::containing_address(PhysAddr::new(
                start + index as u64 * PAGE_SIZE,
            )));
        }
        index -= count;
    }
    None
}

fn find_metadata_extent(regions: &[MemoryRegion], pages: u64) -> Option<u64> {
    let bytes = pages.checked_mul(PAGE_SIZE)?;
    regions
        .iter()
        .filter_map(usable_bounds)
        .find_map(|(start, end)| (end - start >= bytes).then_some(start))
}

fn largest_usable_extent(regions: &[MemoryRegion]) -> u64 {
    regions
        .iter()
        .filter_map(usable_bounds)
        .map(|(start, end)| end - start)
        .max()
        .unwrap_or(0)
}

const fn align_up(value: u64, align: u64) -> u64 {
    value.saturating_add(align - 1) & !(align - 1)
}

#[cfg(feature = "test")]
pub mod test_support {
    use super::{compact_frame_index, compact_index_frame, usable_frame_count};
    use bootloader_api::info::{MemoryRegion, MemoryRegionKind};
    use x86_64::structures::paging::{PhysFrame, Size4KiB};
    use x86_64::PhysAddr;

    pub struct TestCursor {
        region: usize,
        next: u64,
    }

    impl TestCursor {
        pub const fn new() -> Self {
            Self {
                region: 0,
                next: u64::MAX,
            }
        }

        pub fn next(&mut self, regions: &[MemoryRegion]) -> Option<PhysFrame<Size4KiB>> {
            loop {
                let region = regions.get(self.region)?;
                if region.kind != MemoryRegionKind::Usable {
                    self.region += 1;
                    self.next = u64::MAX;
                    continue;
                }
                if self.next == u64::MAX {
                    self.next = (region.start.max(0x1000) + 0xfff) & !0xfff;
                }
                if self.next + 0x1000 > region.end {
                    self.region += 1;
                    self.next = u64::MAX;
                    continue;
                }
                let frame = PhysFrame::containing_address(PhysAddr::new(self.next));
                self.next += 0x1000;
                return Some(frame);
            }
        }
    }

    #[cfg_attr(feature = "test", expect(dead_code, reason = "production-only API"))]
    pub fn usable_frames(regions: &[MemoryRegion]) -> u64 {
        usable_frame_count(regions)
    }

    #[cfg_attr(feature = "test", expect(dead_code, reason = "production-only API"))]
    pub fn index_of(regions: &[MemoryRegion], frame: PhysFrame<Size4KiB>) -> Option<usize> {
        compact_frame_index(regions, frame.start_address().as_u64())
    }

    #[cfg_attr(feature = "test", expect(dead_code, reason = "production-only API"))]
    pub fn frame_at(regions: &[MemoryRegion], index: usize) -> Option<PhysFrame<Size4KiB>> {
        compact_index_frame(regions, index)
    }
}

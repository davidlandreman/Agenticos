//! Bump-cursor frame allocator over the bootloader's `MemoryRegions`.
//!
//! Hands out 4 KiB physical frames in ascending order from the lowest usable
//! region forward, skipping the null frame at physical address 0. Per-call
//! cost is amortized O(1): the cursor remembers which region it is in and
//! the next physical address inside that region, so each call advances by
//! one frame instead of rebuilding an iterator and walking it from scratch.
//!
//! See `docs/plans/2026-05-09-002-perf-frame-allocator-and-page-fault-hot-path-plan.md` U1
//! for the design rationale.

use bootloader_api::info::{MemoryRegion, MemoryRegionKind, MemoryRegions};
use x86_64::structures::paging::{FrameAllocator, PhysFrame, Size4KiB};
use x86_64::PhysAddr;

use crate::{debug_info, debug_trace};

/// Emit a periodic info-level summary every N allocations so a stuck system
/// is still observable even after U2 demotes the per-call log.
const SUMMARY_INTERVAL: u64 = 256;

/// Cursor state shared by `BootInfoFrameAllocator` and the pure helper used
/// in unit tests.
#[derive(Clone, Copy)]
struct Cursor {
    /// Index into the regions slice of the region the cursor is currently
    /// inside. May point past the last region once the allocator is
    /// exhausted; the next-frame logic returns `None` in that case.
    region_idx: usize,
    /// Next 4 KiB-aligned physical address to hand out. `u64::MAX` is the
    /// sentinel for "uninitialized — seek to the first Usable region on
    /// the next call." Real addresses cannot reach `u64::MAX` because
    /// memory regions stay below 2^52 in practice.
    next_addr: u64,
}

impl Cursor {
    const fn new() -> Self {
        Self {
            region_idx: 0,
            next_addr: u64::MAX,
        }
    }
}

/// Frame allocator that yields one physical frame per call from the
/// bootloader-provided memory map.
pub struct BootInfoFrameAllocator {
    memory_map: &'static MemoryRegions,
    cursor: Cursor,
    /// Total frames ever returned (excluding `None` outcomes). Drives the
    /// periodic summary log and gives `kernel_state`-style introspection a
    /// hook later.
    frames_issued: u64,
}

impl BootInfoFrameAllocator {
    /// SAFETY: `memory_map` must remain valid for the static lifetime; the
    /// caller (`MemoryMapper::new`) holds it via `STATIC_MEMORY_REGIONS` in
    /// `src/mm/memory.rs`.
    pub unsafe fn init(memory_map: &'static MemoryRegions) -> Self {
        debug_info!("Initializing frame allocator");
        BootInfoFrameAllocator {
            memory_map,
            cursor: Cursor::new(),
            frames_issued: 0,
        }
    }

    /// Total frames issued since boot. Useful for diagnostics.
    #[allow(dead_code)]
    pub fn frames_issued(&self) -> u64 {
        self.frames_issued
    }
}

unsafe impl FrameAllocator<Size4KiB> for BootInfoFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        let frame = next_frame(self.memory_map, &mut self.cursor)?;
        self.frames_issued += 1;
        debug_trace!("Allocated frame at {:?}", frame.start_address());
        if self.frames_issued.is_multiple_of(SUMMARY_INTERVAL) {
            debug_info!(
                "frame allocator: {} frames issued, region {}, next {:#x}",
                self.frames_issued,
                self.cursor.region_idx,
                self.cursor.next_addr
            );
        }
        Some(frame)
    }
}

/// Pure cursor advance step. Returns the next 4 KiB frame from `regions`
/// starting at `cursor` state, skipping non-Usable regions and the null
/// frame at physical address 0. Mutates `cursor` in place so the next call
/// resumes where this one left off.
///
/// Invariants honored:
/// - Frames returned are 4 KiB aligned and lie strictly inside a Usable
///   region.
/// - The null frame (`PhysAddr(0)`) is never returned.
/// - Within a single Usable region, returned frames are strictly
///   monotonically increasing by 0x1000.
///
/// Cross-region ordering follows the input slice's order, which matches
/// `MemoryRegions::iter()` for the live allocator.
fn next_frame(regions: &[MemoryRegion], cursor: &mut Cursor) -> Option<PhysFrame> {
    loop {
        if cursor.region_idx >= regions.len() {
            return None;
        }
        let region = &regions[cursor.region_idx];
        if region.kind != MemoryRegionKind::Usable {
            cursor.region_idx += 1;
            cursor.next_addr = u64::MAX;
            continue;
        }

        // Lazy-init `next_addr` to the start of this region (clamped past
        // the null frame). Re-set whenever we advance into a new region.
        if cursor.next_addr == u64::MAX || cursor.next_addr < region.start {
            cursor.next_addr = align_up_4k(region.start.max(0x1000));
        }

        if cursor.next_addr + 0x1000 > region.end {
            // Region exhausted. Move to the next one.
            cursor.region_idx += 1;
            cursor.next_addr = u64::MAX;
            continue;
        }

        let frame = PhysFrame::containing_address(PhysAddr::new(cursor.next_addr));
        cursor.next_addr += 0x1000;
        return Some(frame);
    }
}

/// Round `addr` up to the next 4 KiB boundary.
#[inline]
fn align_up_4k(addr: u64) -> u64 {
    (addr + 0xFFF) & !0xFFF
}

#[cfg(feature = "test")]
pub mod test_support {
    //! Test-only handles to the cursor primitives so unit tests in
    //! `src/tests/memory.rs` can drive the allocator over synthetic memory
    //! maps without consuming live physical frames.

    use super::{next_frame, Cursor};
    use bootloader_api::info::MemoryRegion;
    use x86_64::structures::paging::PhysFrame;

    /// Wrapper around the internal cursor for test fixtures.
    pub struct TestCursor(Cursor);

    impl TestCursor {
        pub const fn new() -> Self {
            Self(Cursor::new())
        }

        pub fn next(&mut self, regions: &[MemoryRegion]) -> Option<PhysFrame> {
            next_frame(regions, &mut self.0)
        }

        pub fn region_idx(&self) -> usize {
            self.0.region_idx
        }

        pub fn next_addr(&self) -> u64 {
            self.0.next_addr
        }
    }
}

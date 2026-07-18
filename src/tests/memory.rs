use crate::debug_info;
use crate::lib::test_utils::Testable;
use crate::mm::memory;

fn test_memory_stats() {
    let stats = memory::get_memory_stats();
    assert!(
        stats.total_memory > 0,
        "Total memory should be greater than 0"
    );
    assert!(
        stats.usable_memory > 0,
        "Usable memory should be greater than 0"
    );
    assert!(
        stats.usable_memory <= stats.total_memory,
        "Usable memory should not exceed total memory"
    );
    debug_info!(
        "Memory - Total: {} MB, Usable: {} MB",
        stats.total_memory / (1024 * 1024),
        stats.usable_memory / (1024 * 1024)
    );
}

fn test_memory_alignment() {
    // Test that memory addresses are properly aligned
    let addr1 = 0x1000;
    let addr2 = 0x2000;
    assert_eq!(addr1 % 4096, 0, "Address should be page aligned");
    assert_eq!(addr2 % 4096, 0, "Address should be page aligned");
}

fn test_memory_ranges() {
    let stats = memory::get_memory_stats();
    // Ensure we have reasonable memory amounts (at least 1MB usable)
    assert!(
        stats.usable_memory >= 1024 * 1024,
        "Should have at least 1MB of usable memory"
    );
}

// ---------- Frame allocator cursor (U1) ----------
//
// The cursor logic in `BootInfoFrameAllocator` is exercised here via the
// pure `next_frame` helper exposed under `test_support`. Driving the cursor
// over synthetic memory maps avoids consuming live physical frames during
// tests and lets us cover edge cases (region 0 starting at addr 0, exhaustion,
// non-Usable region skipping) deterministically.

use crate::mm::frame_allocator::test_support::TestCursor;
use bootloader_api::info::{MemoryRegion, MemoryRegionKind};

fn region(start: u64, end: u64, kind: MemoryRegionKind) -> MemoryRegion {
    MemoryRegion { start, end, kind }
}

fn test_frame_cursor_skips_null_frame() {
    // Region 0 starts at physical 0; the cursor must skip frame 0 and yield
    // the frame at 0x1000 first.
    let regions = [region(0, 0x10_000, MemoryRegionKind::Usable)];
    let mut cursor = TestCursor::new();

    let first = cursor.next(&regions).expect("first frame");
    assert_eq!(
        first.start_address().as_u64(),
        0x1000,
        "first frame must skip the null frame at 0x0"
    );
    let second = cursor.next(&regions).expect("second frame");
    assert_eq!(
        second.start_address().as_u64(),
        0x2000,
        "second frame must advance by 0x1000"
    );
}

fn test_frame_cursor_crosses_region_boundary() {
    // Two Usable regions with a gap: cursor must exhaust the first then
    // resume at the start of the second, never yielding addresses in the gap.
    let regions = [
        region(0x10_000, 0x12_000, MemoryRegionKind::Usable), // 2 frames
        region(0x50_000, 0x52_000, MemoryRegionKind::Usable), // 2 frames
    ];
    let mut cursor = TestCursor::new();

    let frames: alloc::vec::Vec<u64> = (0..4)
        .map(|_| cursor.next(&regions).unwrap().start_address().as_u64())
        .collect();
    assert_eq!(frames, [0x10_000, 0x11_000, 0x50_000, 0x51_000]);
    assert!(cursor.next(&regions).is_none(), "fifth call exhausts");
}

fn test_frame_cursor_skips_non_usable_regions() {
    // Bootloader and UnknownBios regions in the middle must be passed over.
    let regions = [
        region(0x10_000, 0x11_000, MemoryRegionKind::Usable),
        region(0x11_000, 0x20_000, MemoryRegionKind::Bootloader),
        region(0x20_000, 0x21_000, MemoryRegionKind::UnknownBios(0)),
        region(0x30_000, 0x31_000, MemoryRegionKind::Usable),
    ];
    let mut cursor = TestCursor::new();

    let first = cursor.next(&regions).unwrap().start_address().as_u64();
    let second = cursor.next(&regions).unwrap().start_address().as_u64();
    assert_eq!(first, 0x10_000);
    assert_eq!(
        second, 0x30_000,
        "must skip Bootloader and UnknownBios regions"
    );
    assert!(cursor.next(&regions).is_none());
}

fn test_frame_cursor_returns_none_when_exhausted() {
    // Once exhausted, the cursor must continue to return None on every call
    // (no underflow, no wrap-around, no panic).
    let regions = [region(0x10_000, 0x11_000, MemoryRegionKind::Usable)];
    let mut cursor = TestCursor::new();

    assert!(cursor.next(&regions).is_some());
    for _ in 0..10 {
        assert!(cursor.next(&regions).is_none(), "stays exhausted");
    }
}

fn test_frame_cursor_handles_empty_regions_slice() {
    let mut cursor = TestCursor::new();
    assert!(cursor.next(&[]).is_none());
}

fn test_frame_cursor_handles_all_non_usable_regions() {
    let regions = [
        region(0x10_000, 0x20_000, MemoryRegionKind::Bootloader),
        region(0x30_000, 0x40_000, MemoryRegionKind::UnknownBios(0)),
    ];
    let mut cursor = TestCursor::new();
    assert!(cursor.next(&regions).is_none());
}

fn test_frame_cursor_monotonic_over_4096_calls() {
    // Drive the cursor through enough frames to exercise both region
    // crossings and the periodic-summary boundary (256 in the live
    // allocator, but the cursor itself doesn't log so any count works
    // here). The invariant we lock in: every returned address is 4 KiB
    // aligned, strictly greater than the previous, never zero, and inside
    // some Usable region.
    //
    // Allocator-specific: this test asserts the bump-cursor's natural
    // ordering. A future swap to a free-list/bitmap allocator that
    // reorders frames would fail this test deliberately — that's the
    // signal to revisit U1's invariants when changing the allocator.
    let regions = [
        region(0, 0x9fc00, MemoryRegionKind::Usable),
        region(0x100_000, 0x100_000 + 0x1_000_000, MemoryRegionKind::Usable),
    ];
    let mut cursor = TestCursor::new();

    let mut prev: u64 = 0;
    let mut count = 0u64;
    while let Some(frame) = cursor.next(&regions) {
        let addr = frame.start_address().as_u64();
        assert_eq!(
            addr & 0xFFF,
            0,
            "frame {} not 4 KiB aligned: {:#x}",
            count,
            addr
        );
        assert!(
            addr > prev,
            "frame {} not strictly monotonic: {:#x} <= {:#x}",
            count,
            addr,
            prev
        );
        assert_ne!(addr, 0, "null frame returned at iteration {}", count);
        prev = addr;
        count += 1;
        if count >= 4096 {
            break;
        }
    }
    assert!(
        count >= 4096,
        "did not produce at least 4096 frames; got {}",
        count
    );
}

// ---------- Live frame allocator throughput (diagnostic) ----------
//
// Confirms allocation throughput and, critically, returns every frame so the
// test itself cannot hide reclamation regressions.

fn test_live_frame_allocator_throughput() {
    use crate::arch::x86_64::interrupts::get_timer_ticks;

    const N: u64 = 256;

    let before = crate::mm::memory::with_memory_mapper(|m| m.frame_stats()).unwrap();

    let t0 = get_timer_ticks();
    let (frames, allocated) = crate::mm::memory::with_memory_mapper(|m| {
        let mut frames = [None; N as usize];
        let mut allocated = 0usize;
        for slot in frames.iter_mut() {
            if let Some(frame) = m.allocate_test_frame() {
                *slot = Some(frame);
                allocated += 1;
            }
        }
        (frames, allocated)
    })
    .unwrap();
    let t1 = get_timer_ticks();

    let allocated_stats = crate::mm::memory::with_memory_mapper(|m| m.frame_stats()).unwrap();
    let elapsed = t1.saturating_sub(t0);

    debug_info!(
        "[perf] live frame alloc: {} frames in {} ticks ({} ms); free {} -> {}",
        allocated,
        elapsed,
        elapsed.saturating_mul(10),
        before.free,
        allocated_stats.free,
    );

    assert_eq!(allocated as u64, N, "expected to allocate all {} frames", N);
    assert_eq!(
        before.free - allocated_stats.free,
        N,
        "free count must fall by N while frames are owned"
    );
    assert!(
        elapsed < 100,
        "live frame allocator took {} ticks for {} frames — O(1) regression",
        elapsed,
        N
    );

    crate::mm::memory::with_memory_mapper(|m| {
        for frame in frames.iter().flatten().copied() {
            assert!(m.release_test_frame(frame), "test frame release failed");
        }
        let reused = m.allocate_test_frame().expect("released capacity reusable");
        assert!(
            frames.contains(&Some(reused)),
            "allocator did not reuse a released frame"
        );
        assert!(m.release_test_frame(reused));
    })
    .expect("memory mapper");
    let after = crate::mm::memory::with_memory_mapper(|m| m.frame_stats()).unwrap();
    assert_eq!(
        after, before,
        "live allocate/release must have zero frame delta"
    );
}

fn test_live_frame_refcounts_and_failure_injection() {
    let before = crate::mm::memory::with_memory_mapper(|m| m.frame_stats()).unwrap();
    let frame = crate::mm::memory::with_memory_mapper(|m| m.allocate_test_frame())
        .flatten()
        .expect("test frame");
    assert_eq!(
        crate::mm::memory::with_memory_mapper(|m| m.frame_refcount(frame)).flatten(),
        Some(1)
    );
    assert!(crate::mm::memory::with_memory_mapper(|m| m.retain_frame(frame)).unwrap());
    assert_eq!(
        crate::mm::memory::with_memory_mapper(|m| m.frame_refcount(frame)).flatten(),
        Some(2)
    );
    assert!(crate::mm::memory::with_memory_mapper(|m| m.release_test_frame(frame)).unwrap());
    assert_eq!(
        crate::mm::memory::with_memory_mapper(|m| m.frame_refcount(frame)).flatten(),
        Some(1)
    );
    assert!(crate::mm::memory::with_memory_mapper(|m| m.release_test_frame(frame)).unwrap());
    assert_eq!(
        crate::mm::memory::with_memory_mapper(|m| m.frame_refcount(frame)).flatten(),
        Some(0)
    );

    {
        let _failure = crate::mm::frame_allocator::fail_allocations_after(0);
        assert!(
            crate::mm::memory::with_memory_mapper(|m| m.allocate_test_frame())
                .flatten()
                .is_none()
        );
    }
    let after = crate::mm::memory::with_memory_mapper(|m| m.frame_stats()).unwrap();
    assert_eq!(
        after, before,
        "refcount and failed allocation must unwind exactly"
    );
}

// ---------- Page-fault demotion baseline (U4) ----------
//
// Documentary regression guard: allocates ~1 MiB of heap (256 demand-paged
// pages) and asserts the runtime debug level is `Debug`, the level the
// kernel boots with at `src/kernel.rs:14`. The actual U2 expectation —
// silence from per-fault `[INFO]`/`[DEBUG]` chatter at the default level —
// is observed by reading the test's serial output: a developer who
// regresses the U2 demotions will see those lines reappear during this
// test's heap touches. The assertion below pins the level invariant; the
// allocation forces the page-fault path to run inside the test
// environment.
//
// See plan U4 in
// docs/plans/2026-05-09-002-perf-frame-allocator-and-page-fault-hot-path-plan.md.
fn test_heap_demand_paging_at_default_log_level() {
    use crate::lib::debug::{get_debug_level, DebugLevel};

    const SIZE: usize = 1024 * 1024;
    let mut buf: alloc::vec::Vec<u8> = alloc::vec::Vec::with_capacity(SIZE);
    // Touch the front and back pages so demand-paging is exercised
    // without forcing a full memset (the goal is just to take a few
    // page faults, not stress the heap allocator).
    unsafe {
        buf.set_len(SIZE);
    }
    buf[0] = 0xAA;
    buf[SIZE - 1] = 0xBB;
    assert_eq!(buf[0], 0xAA);
    assert_eq!(buf[SIZE - 1], 0xBB);

    let level = get_debug_level();
    assert_eq!(
        level,
        DebugLevel::Debug,
        "test environment must match the boot default log level (Debug). \
         U2's per-fault log demotions to trace are tuned for this level — \
         changing the default level will silently invalidate the demotion \
         expectation."
    );
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_memory_stats,
        &test_memory_alignment,
        &test_memory_ranges,
        &test_frame_cursor_skips_null_frame,
        &test_frame_cursor_crosses_region_boundary,
        &test_frame_cursor_skips_non_usable_regions,
        &test_frame_cursor_returns_none_when_exhausted,
        &test_frame_cursor_handles_empty_regions_slice,
        &test_frame_cursor_handles_all_non_usable_regions,
        &test_frame_cursor_monotonic_over_4096_calls,
        &test_live_frame_allocator_throughput,
        &test_live_frame_refcounts_and_failure_injection,
        &test_heap_demand_paging_at_default_log_level,
    ]
}

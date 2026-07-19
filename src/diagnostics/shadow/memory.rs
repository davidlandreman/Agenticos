//! Compact physical-frame ownership and user-leaf mapping shadow.
//!
//! Storage is carved out beside the production allocator metadata before the
//! heap exists.  Mutations run under the production mapper lock, while the
//! sequence counters make a crash-time lock-free snapshot explicitly mark an
//! interrupted transition.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use super::{latch, ViolationRecord};

pub const MM_001: u32 = 0x0800_0001;
pub const MM_002: u32 = 0x0800_0002;
pub const MM_003: u32 = 0x0800_0003;
pub const MM_004: u32 = 0x0800_0004;
pub const MM_005: u32 = 0x0800_0005;
pub const MM_006: u32 = 0x0800_0006;
pub const MM_007: u32 = 0x0800_0007;
pub const DIAG_CAPACITY_MAPPING: u32 = 0x0f00_0007;

const RECENT_LEN: usize = 256;
const EMPTY: u8 = 0;
const OCCUPIED: u8 = 1;
const TOMBSTONE: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameState {
    Free = 0,
    Pinned = 1,
    Live = 2,
    Quarantined = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameKind {
    Unknown = 0,
    RootL4 = 1,
    PageTable = 2,
    UserLeaf = 3,
    KernelHeap = 4,
    KernelStack = 5,
    Other = 6,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameRefReason {
    RootL4 = 1,
    PageTable = 2,
    LeafMapping = 3,
    CowShare = 4,
    KernelHeap = 5,
    #[allow(dead_code, reason = "reserved for physical stack backing migration")]
    KernelStack = 6,
    Transient = 7,
    Other = 8,
}

impl FrameRefReason {
    pub const fn kind(self) -> FrameKind {
        match self {
            Self::RootL4 => FrameKind::RootL4,
            Self::PageTable => FrameKind::PageTable,
            Self::LeafMapping | Self::CowShare => FrameKind::UserLeaf,
            Self::KernelHeap => FrameKind::KernelHeap,
            Self::KernelStack => FrameKind::KernelStack,
            Self::Transient | Self::Other => FrameKind::Other,
        }
    }

    const fn bucket(self) -> Bucket {
        match self {
            Self::RootL4 | Self::PageTable => Bucket::Table,
            Self::LeafMapping | Self::CowShare => Bucket::Leaf,
            _ => Bucket::Transient,
        }
    }
}

#[derive(Clone, Copy)]
enum Bucket {
    Leaf,
    Table,
    Transient,
}

/// Exactly 24 bytes so a full frame ledger plus one mapping slot per usable
/// frame consumes 64 bytes/frame (1.5625% of managed RAM).
#[derive(Clone, Copy)]
#[repr(C)]
struct FrameRecord {
    generation: u32,
    expected_refs: u32,
    leaf_refs: u16,
    table_refs: u16,
    transient_refs: u16,
    state: u8,
    kind: u8,
    last_alloc_site: u16,
    last_release_site: u16,
    last_epoch: u32,
}

const _: () = assert!(core::mem::size_of::<FrameRecord>() == 24);

impl FrameRecord {
    const fn empty() -> Self {
        Self {
            generation: 0,
            expected_refs: 0,
            leaf_refs: 0,
            table_refs: 0,
            transient_refs: 0,
            state: FrameState::Free as u8,
            kind: FrameKind::Unknown as u8,
            last_alloc_site: 0,
            last_release_site: 0,
            last_epoch: 0,
        }
    }
}

/// Open-addressed mapping record. One slot is budgeted per usable frame.
#[derive(Clone, Copy)]
#[repr(C)]
struct MappingRecord {
    as_generation: u64,
    virtual_page: u64,
    frame_address: u64,
    frame_generation: u32,
    flags: u32,
    mapping_generation: u32,
    state: u8,
    probe_distance: u8,
    _reserved: u16,
}

const _: () = assert!(core::mem::size_of::<MappingRecord>() == 40);

impl MappingRecord {
    #[allow(
        dead_code,
        reason = "zeroed storage uses this layout value in unit tests"
    )]
    const fn empty() -> Self {
        Self {
            as_generation: 0,
            virtual_page: 0,
            frame_address: 0,
            frame_generation: 0,
            flags: 0,
            mapping_generation: 0,
            state: EMPTY,
            probe_distance: 0,
            _reserved: 0,
        }
    }
}

#[derive(Clone, Copy)]
struct Storage {
    frames: *mut FrameRecord,
    frame_count: usize,
    mappings: *mut MappingRecord,
    mapping_capacity: usize,
}

unsafe impl Send for Storage {}

struct StorageCell(UnsafeCell<Storage>);
unsafe impl Sync for StorageCell {}

static STORAGE: StorageCell = StorageCell(UnsafeCell::new(Storage {
    frames: core::ptr::null_mut(),
    frame_count: 0,
    mappings: core::ptr::null_mut(),
    mapping_capacity: 0,
}));
static SEQUENCE: AtomicU64 = AtomicU64::new(0);
static EPOCH: AtomicU32 = AtomicU32::new(1);
static MAPPING_GENERATION: AtomicU32 = AtomicU32::new(1);
static MAPPING_COUNT: AtomicU32 = AtomicU32::new(0);
static MAX_PROBE: AtomicU32 = AtomicU32::new(0);
static REJECTED_INSERTS: AtomicU32 = AtomicU32::new(0);
static RECENT_CURSOR: AtomicU32 = AtomicU32::new(0);
static RECENT_FRAMES: [AtomicU32; RECENT_LEN] = [const { AtomicU32::new(u32::MAX) }; RECENT_LEN];
static RECENT_MAPPING_CURSOR: AtomicU32 = AtomicU32::new(0);
static RECENT_MAPPINGS: [AtomicU32; RECENT_LEN] = [const { AtomicU32::new(u32::MAX) }; RECENT_LEN];

const MAX_STORAGE_BYTES: usize = 32 * 1024 * 1024;

const fn storage_budget(frame_count: usize) -> usize {
    let two_percent = frame_count.saturating_mul(4096) / 50;
    if two_percent < MAX_STORAGE_BYTES {
        two_percent
    } else {
        MAX_STORAGE_BYTES
    }
}

pub const fn mapping_capacity(frame_count: usize) -> usize {
    if !cfg!(any(feature = "diagnostics", feature = "diagnostics-strict")) {
        return 0;
    }
    let frames_bytes = frame_count.saturating_mul(core::mem::size_of::<FrameRecord>());
    let budget = storage_budget(frame_count);
    if frames_bytes > budget {
        return 0;
    }
    let available = budget - frames_bytes;
    let capacity = available / core::mem::size_of::<MappingRecord>();
    if capacity < frame_count {
        capacity
    } else {
        frame_count
    }
}

pub const fn storage_bytes(frame_count: usize) -> usize {
    if !cfg!(any(feature = "diagnostics", feature = "diagnostics-strict")) {
        return 0;
    }
    let capacity = mapping_capacity(frame_count);
    if capacity == 0 {
        return 0;
    }
    frame_count * core::mem::size_of::<FrameRecord>()
        + capacity * core::mem::size_of::<MappingRecord>()
}

/// Initialize the prefaulted shadow ledger inside allocator metadata.
///
/// # Safety
/// `base..base + storage_bytes(frame_count)` must be uniquely owned, mapped,
/// aligned for both record types, and valid for the kernel lifetime.
pub unsafe fn init(base: *mut u8, frame_count: usize) {
    if storage_bytes(frame_count) == 0 {
        return;
    }
    let frame_bytes = frame_count * core::mem::size_of::<FrameRecord>();
    unsafe { core::ptr::write_bytes(base, 0, storage_bytes(frame_count)) };
    let storage = unsafe { &mut *STORAGE.0.get() };
    storage.frames = base.cast();
    storage.frame_count = frame_count;
    storage.mappings = unsafe { base.add(frame_bytes) }.cast();
    storage.mapping_capacity = mapping_capacity(frame_count);
}

fn enabled() -> bool {
    crate::diagnostics::personality() != crate::diagnostics::Personality::Minimal
        && unsafe { (*STORAGE.0.get()).frame_count != 0 }
}

fn storage() -> Storage {
    unsafe { *STORAGE.0.get() }
}

fn report(id: u32, subject: u64, epoch: u64, expected: u64, observed: u64) {
    let first = latch(ViolationRecord {
        invariant_id: id,
        severity: 2,
        cpu: 0,
        mode: 0,
        domain: 7,
        epoch,
        subject,
        expected0: expected,
        observed0: observed,
        expected1: MAPPING_COUNT.load(Ordering::Relaxed).into(),
        observed1: SEQUENCE.load(Ordering::Relaxed),
        trace_sequence: 0,
    });
    if first && crate::diagnostics::personality() == crate::diagnostics::Personality::Strict {
        crate::diagnostics::crash::begin_invariant(id);
    }
}

fn begin_mutation() {
    SEQUENCE.fetch_add(1, Ordering::AcqRel);
}

fn end_mutation() {
    SEQUENCE.fetch_add(1, Ordering::Release);
}

fn touch(index: usize) -> u32 {
    let epoch = EPOCH.fetch_add(1, Ordering::Relaxed);
    let cursor = RECENT_CURSOR.fetch_add(1, Ordering::Relaxed) as usize % RECENT_LEN;
    RECENT_FRAMES[cursor].store(index as u32, Ordering::Release);
    epoch
}

fn touch_mapping(index: usize) {
    let cursor = RECENT_MAPPING_CURSOR.fetch_add(1, Ordering::Relaxed) as usize % RECENT_LEN;
    RECENT_MAPPINGS[cursor].store(index as u32, Ordering::Release);
}

fn bucket_mut(record: &mut FrameRecord, bucket: Bucket) -> &mut u16 {
    match bucket {
        Bucket::Leaf => &mut record.leaf_refs,
        Bucket::Table => &mut record.table_refs,
        Bucket::Transient => &mut record.transient_refs,
    }
}

fn bucket_count(record: &FrameRecord, bucket: Bucket) -> u16 {
    match bucket {
        Bucket::Leaf => record.leaf_refs,
        Bucket::Table => record.table_refs,
        Bucket::Transient => record.transient_refs,
    }
}

fn compatible(old: FrameKind, new: FrameKind) -> bool {
    old == FrameKind::Unknown
        || old == new
        || old == FrameKind::Other
        || (old == FrameKind::UserLeaf && new == FrameKind::UserLeaf)
}

fn decode_kind(value: u8) -> FrameKind {
    match value {
        1 => FrameKind::RootL4,
        2 => FrameKind::PageTable,
        3 => FrameKind::UserLeaf,
        4 => FrameKind::KernelHeap,
        5 => FrameKind::KernelStack,
        6 => FrameKind::Other,
        _ => FrameKind::Unknown,
    }
}

pub fn allocated(index: usize, reason: FrameRefReason, site: u16) {
    if !enabled() {
        return;
    }
    let store = storage();
    if index >= store.frame_count {
        report(
            MM_006,
            index as u64,
            0,
            store.frame_count as u64,
            index as u64,
        );
        return;
    }
    begin_mutation();
    let record = unsafe { &mut *store.frames.add(index) };
    if record.state != FrameState::Free as u8 || record.expected_refs != 0 {
        report(
            MM_002,
            index as u64,
            record.generation.into(),
            0,
            record.expected_refs.into(),
        );
    } else {
        let generation = record.generation.wrapping_add(1).max(1);
        *record = FrameRecord {
            generation,
            expected_refs: 1,
            state: FrameState::Live as u8,
            kind: reason.kind() as u8,
            last_alloc_site: site,
            last_epoch: touch(index),
            ..FrameRecord::empty()
        };
        *bucket_mut(record, reason.bucket()) = 1;
    }
    end_mutation();
}

pub fn pinned(index: usize, site: u16) {
    if !enabled() {
        return;
    }
    let store = storage();
    if index >= store.frame_count {
        return;
    }
    begin_mutation();
    let record = unsafe { &mut *store.frames.add(index) };
    if record.state == FrameState::Free as u8 {
        record.generation = record.generation.wrapping_add(1).max(1);
        record.state = FrameState::Pinned as u8;
        record.kind = FrameKind::Other as u8;
        record.expected_refs = u32::MAX;
        record.last_alloc_site = site;
        record.last_epoch = touch(index);
    }
    end_mutation();
}

pub fn retain(index: usize, reason: FrameRefReason, site: u16) {
    if !enabled() {
        return;
    }
    let store = storage();
    if index >= store.frame_count {
        return;
    }
    begin_mutation();
    let record = unsafe { &mut *store.frames.add(index) };
    let old_kind = decode_kind(record.kind);
    let new_kind = reason.kind();
    let bucket_count = bucket_count(record, reason.bucket());
    if record.state != FrameState::Live as u8
        || !compatible(old_kind, new_kind)
        || record.expected_refs == u32::MAX
        || bucket_count == u16::MAX
    {
        report(
            MM_002,
            index as u64,
            record.generation.into(),
            record.kind.into(),
            new_kind as u64,
        );
    } else {
        record.expected_refs += 1;
        *bucket_mut(record, reason.bucket()) += 1;
        if old_kind == FrameKind::Other {
            record.kind = new_kind as u8;
        }
        record.last_alloc_site = site;
        record.last_epoch = touch(index);
    }
    end_mutation();
}

pub fn release(index: usize, reason: FrameRefReason, allocator_count: u32, site: u16) {
    if !enabled() {
        return;
    }
    let store = storage();
    if index >= store.frame_count {
        return;
    }
    begin_mutation();
    let record = unsafe { &mut *store.frames.add(index) };
    let bucket_count = bucket_count(record, reason.bucket());
    if record.state != FrameState::Live as u8 || record.expected_refs == 0 || bucket_count == 0 {
        report(MM_002, index as u64, record.generation.into(), 1, 0);
    } else {
        record.expected_refs -= 1;
        *bucket_mut(record, reason.bucket()) -= 1;
        record.last_release_site = site;
        record.last_epoch = touch(index);
        if record.expected_refs != allocator_count {
            report(
                MM_003,
                index as u64,
                record.generation.into(),
                record.expected_refs.into(),
                allocator_count.into(),
            );
        }
        if allocator_count == 0 {
            if record.leaf_refs != 0 || record.table_refs != 0 || record.transient_refs != 0 {
                report(
                    MM_002,
                    index as u64,
                    record.generation.into(),
                    0,
                    record.expected_refs.into(),
                );
                record.state = FrameState::Quarantined as u8;
            } else {
                record.state = FrameState::Free as u8;
                record.kind = FrameKind::Unknown as u8;
            }
        }
    }
    end_mutation();
}

pub fn transfer(index: usize, from: FrameRefReason, to: FrameRefReason, site: u16) {
    if !enabled() {
        return;
    }
    let store = storage();
    if index >= store.frame_count {
        return;
    }
    begin_mutation();
    let record = unsafe { &mut *store.frames.add(index) };
    let from_count = *bucket_mut(record, from.bucket());
    if record.state != FrameState::Live as u8 || from_count == 0 {
        report(
            MM_002,
            index as u64,
            record.generation.into(),
            1,
            from_count.into(),
        );
    } else if !compatible(decode_kind(record.kind), to.kind()) {
        report(
            MM_006,
            index as u64,
            record.generation.into(),
            record.kind.into(),
            to.kind() as u64,
        );
    } else {
        *bucket_mut(record, from.bucket()) -= 1;
        let target = bucket_mut(record, to.bucket());
        if *target == u16::MAX {
            report(
                MM_002,
                index as u64,
                record.generation.into(),
                u16::MAX.into(),
                u16::MAX.into(),
            );
        } else {
            *target += 1;
            record.kind = to.kind() as u8;
            record.last_alloc_site = site;
            record.last_epoch = touch(index);
        }
    }
    end_mutation();
}

pub fn frame_generation(index: usize) -> Option<u32> {
    if !enabled() {
        return None;
    }
    let store = storage();
    let record = unsafe { store.frames.add(index).as_ref()? };
    (record.state == FrameState::Live as u8).then_some(record.generation)
}

pub fn validate_frame(
    index: usize,
    expected_kind: FrameKind,
    allocator_count: Option<u32>,
    subject: u64,
) -> bool {
    if !enabled() {
        return true;
    }
    let store = storage();
    if index >= store.frame_count {
        report(MM_006, subject, 0, store.frame_count as u64, index as u64);
        return false;
    }
    let record = unsafe { &*store.frames.add(index) };
    let kind = decode_kind(record.kind);
    if record.state != FrameState::Live as u8 || kind != expected_kind {
        report(
            MM_006,
            subject,
            record.generation.into(),
            expected_kind as u64,
            record.kind.into(),
        );
        return false;
    }
    if allocator_count.is_some_and(|count| count != record.expected_refs) {
        report(
            MM_003,
            subject,
            record.generation.into(),
            record.expected_refs.into(),
            allocator_count.unwrap().into(),
        );
        return false;
    }
    true
}

pub fn validate_leaf(
    as_generation: u64,
    virtual_page: u64,
    frame_address: u64,
    frame_generation: u32,
    flags: u64,
) -> bool {
    if as_generation == 0 || !enabled() {
        return true;
    }
    let store = storage();
    let Some(index) = find_mapping(store, as_generation, virtual_page) else {
        report(MM_001, virtual_page, as_generation, frame_address, 0);
        return false;
    };
    let record = unsafe { &*store.mappings.add(index) };
    if record.frame_address != frame_address
        || record.frame_generation != frame_generation
        || record.flags != normalize_flags(flags)
    {
        report(
            MM_001,
            virtual_page,
            as_generation,
            record.frame_address,
            frame_address,
        );
        return false;
    }
    true
}

pub fn report_topology(id: u32, subject: u64, as_generation: u64, flags: u64) {
    debug_assert!(id == MM_004 || id == MM_005 || id == MM_006);
    report(id, subject, as_generation, 0, flags);
}

pub fn report_mapper_recursion(subject: u64) {
    if enabled() {
        report(MM_007, subject, 0, 0, 1);
    }
}

fn mapping_hash(as_generation: u64, virtual_page: u64) -> usize {
    let mixed = as_generation.wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ virtual_page.rotate_left(23)
        ^ virtual_page.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    mixed as usize
}

fn find_mapping(store: Storage, as_generation: u64, virtual_page: u64) -> Option<usize> {
    if store.mapping_capacity == 0 {
        return None;
    }
    let start = mapping_hash(as_generation, virtual_page) % store.mapping_capacity;
    for distance in 0..store.mapping_capacity {
        let index = (start + distance) % store.mapping_capacity;
        let record = unsafe { &*store.mappings.add(index) };
        if record.state == EMPTY {
            return None;
        }
        if record.state == OCCUPIED
            && record.as_generation == as_generation
            && record.virtual_page == virtual_page
        {
            return Some(index);
        }
    }
    None
}

pub fn map_leaf(
    as_generation: u64,
    virtual_page: u64,
    frame_address: u64,
    frame_index: usize,
    flags: u64,
) {
    if as_generation == 0 || !enabled() {
        return;
    }
    if flags & (1 << 1) != 0 && flags & (1 << 63) == 0 {
        report(MM_004, virtual_page, as_generation, 0, flags);
        return;
    }
    let Some(frame_generation) = frame_generation(frame_index) else {
        report(MM_006, frame_address, as_generation, 1, 0);
        return;
    };
    let store = storage();
    begin_mutation();
    if find_mapping(store, as_generation, virtual_page).is_some() {
        report(MM_001, virtual_page, as_generation, 0, frame_address);
        end_mutation();
        return;
    }
    let start = mapping_hash(as_generation, virtual_page) % store.mapping_capacity;
    let mut insertion = None;
    for distance in 0..store.mapping_capacity {
        let index = (start + distance) % store.mapping_capacity;
        let record = unsafe { &*store.mappings.add(index) };
        if record.state != OCCUPIED {
            insertion = Some((index, distance));
            break;
        }
    }
    let Some((index, distance)) = insertion else {
        REJECTED_INSERTS.fetch_add(1, Ordering::Relaxed);
        report(
            DIAG_CAPACITY_MAPPING,
            virtual_page,
            as_generation,
            store.mapping_capacity as u64,
            MAPPING_COUNT.load(Ordering::Relaxed).into(),
        );
        end_mutation();
        return;
    };
    let record = unsafe { &mut *store.mappings.add(index) };
    *record = MappingRecord {
        as_generation,
        virtual_page,
        frame_address,
        frame_generation,
        flags: normalize_flags(flags),
        mapping_generation: MAPPING_GENERATION.fetch_add(1, Ordering::Relaxed),
        state: OCCUPIED,
        probe_distance: distance.min(u8::MAX as usize) as u8,
        _reserved: 0,
    };
    touch_mapping(index);
    MAPPING_COUNT.fetch_add(1, Ordering::Relaxed);
    MAX_PROBE.fetch_max(distance as u32, Ordering::Relaxed);
    end_mutation();
}

pub fn unmap_leaf(
    as_generation: u64,
    virtual_page: u64,
    frame_address: u64,
    frame_generation: u32,
) {
    if as_generation == 0 || !enabled() {
        return;
    }
    let store = storage();
    begin_mutation();
    let Some(index) = find_mapping(store, as_generation, virtual_page) else {
        report(MM_001, virtual_page, as_generation, frame_address, 0);
        end_mutation();
        return;
    };
    let record = unsafe { &mut *store.mappings.add(index) };
    if record.frame_address != frame_address || record.frame_generation != frame_generation {
        report(
            MM_001,
            virtual_page,
            as_generation,
            record.frame_address,
            frame_address,
        );
    } else {
        touch_mapping(index);
        record.state = TOMBSTONE;
        MAPPING_COUNT.fetch_sub(1, Ordering::Relaxed);
    }
    end_mutation();
}

pub fn move_leaf(as_generation: u64, source: u64, destination: u64) {
    if as_generation == 0 || !enabled() || source == destination {
        return;
    }
    let store = storage();
    begin_mutation();
    let source_index = find_mapping(store, as_generation, source);
    if source_index.is_none() || find_mapping(store, as_generation, destination).is_some() {
        report(MM_001, source, as_generation, source, destination);
        end_mutation();
        return;
    }
    let old = unsafe { *store.mappings.add(source_index.unwrap()) };
    touch_mapping(source_index.unwrap());
    unsafe { (*store.mappings.add(source_index.unwrap())).state = TOMBSTONE };
    let start = mapping_hash(as_generation, destination) % store.mapping_capacity;
    for distance in 0..store.mapping_capacity {
        let index = (start + distance) % store.mapping_capacity;
        let record = unsafe { &mut *store.mappings.add(index) };
        if record.state != OCCUPIED {
            *record = MappingRecord {
                virtual_page: destination,
                mapping_generation: MAPPING_GENERATION.fetch_add(1, Ordering::Relaxed),
                probe_distance: distance.min(u8::MAX as usize) as u8,
                ..old
            };
            touch_mapping(index);
            MAX_PROBE.fetch_max(distance as u32, Ordering::Relaxed);
            end_mutation();
            return;
        }
    }
    REJECTED_INSERTS.fetch_add(1, Ordering::Relaxed);
    report(
        DIAG_CAPACITY_MAPPING,
        destination,
        as_generation,
        store.mapping_capacity as u64,
        MAPPING_COUNT.load(Ordering::Relaxed).into(),
    );
    end_mutation();
}

pub fn update_leaf_flags(as_generation: u64, virtual_page: u64, flags: u64) {
    if as_generation == 0 || !enabled() {
        return;
    }
    if flags & (1 << 1) != 0 && flags & (1 << 63) == 0 {
        report(MM_004, virtual_page, as_generation, 0, flags);
        return;
    }
    let store = storage();
    begin_mutation();
    let Some(index) = find_mapping(store, as_generation, virtual_page) else {
        report(MM_001, virtual_page, as_generation, 1, 0);
        end_mutation();
        return;
    };
    let record = unsafe { &mut *store.mappings.add(index) };
    record.flags = normalize_flags(flags);
    record.mapping_generation = MAPPING_GENERATION.fetch_add(1, Ordering::Relaxed);
    touch_mapping(index);
    end_mutation();
}

pub fn replace_leaf(
    as_generation: u64,
    virtual_page: u64,
    old_frame: u64,
    new_frame: u64,
    new_frame_index: usize,
    flags: u64,
) {
    if as_generation == 0 || !enabled() {
        return;
    }
    let Some(new_generation) = frame_generation(new_frame_index) else {
        report(MM_006, new_frame, as_generation, 1, 0);
        return;
    };
    let store = storage();
    begin_mutation();
    let Some(index) = find_mapping(store, as_generation, virtual_page) else {
        report(MM_001, virtual_page, as_generation, old_frame, 0);
        end_mutation();
        return;
    };
    let record = unsafe { &mut *store.mappings.add(index) };
    if record.frame_address != old_frame {
        report(
            MM_001,
            virtual_page,
            as_generation,
            record.frame_address,
            old_frame,
        );
    } else {
        record.frame_address = new_frame;
        record.frame_generation = new_generation;
        record.flags = normalize_flags(flags);
        record.mapping_generation = MAPPING_GENERATION.fetch_add(1, Ordering::Relaxed);
        touch_mapping(index);
    }
    end_mutation();
}

const fn normalize_flags(flags: u64) -> u32 {
    // ACCESSED and DIRTY are CPU-maintained observations, not mapper commit
    // semantics. They may change at any time while a leaf is live.
    ((flags as u32) & !((1 << 5) | (1 << 6))) | (((flags >> 63) as u32) << 31)
}

pub fn remove_address_space(as_generation: u64) {
    if as_generation == 0 || !enabled() {
        return;
    }
    let store = storage();
    begin_mutation();
    for index in 0..store.mapping_capacity {
        let record = unsafe { &mut *store.mappings.add(index) };
        if record.state == OCCUPIED && record.as_generation == as_generation {
            touch_mapping(index);
            record.state = TOMBSTONE;
            MAPPING_COUNT.fetch_sub(1, Ordering::Relaxed);
        }
    }
    end_mutation();
}

pub fn write_snapshot(writer: &mut crate::diagnostics::wire::Writer<'_>) -> u32 {
    let before = SEQUENCE.load(Ordering::Acquire);
    let store = storage();
    writer.u32(store.frame_count as u32);
    writer.u32(store.mapping_capacity as u32);
    writer.u32(MAPPING_COUNT.load(Ordering::Relaxed));
    writer.u32(MAX_PROBE.load(Ordering::Relaxed));
    writer.u32(REJECTED_INSERTS.load(Ordering::Relaxed));
    writer.u32(core::mem::size_of::<FrameRecord>() as u32);
    writer.u64(before);
    let count_at = writer.len();
    writer.u32(0);
    let mut count = 0u32;
    for recent in &RECENT_FRAMES {
        let index = recent.load(Ordering::Acquire);
        if index == u32::MAX || index as usize >= store.frame_count {
            continue;
        }
        let record = unsafe { *store.frames.add(index as usize) };
        writer.u32(index);
        writer.u32(record.generation);
        writer.u32(record.expected_refs);
        writer.u16(record.leaf_refs);
        writer.u16(record.table_refs);
        writer.u16(record.transient_refs);
        writer.u8(record.state);
        writer.u8(record.kind);
        writer.u16(record.last_alloc_site);
        writer.u16(record.last_release_site);
        writer.u32(record.last_epoch);
        count += 1;
    }
    writer.patch_u32(count_at, count);
    let mappings_at = writer.len();
    writer.u32(0);
    let mut mappings = 0u32;
    for recent in &RECENT_MAPPINGS {
        let index = recent.load(Ordering::Acquire);
        if index == u32::MAX || index as usize >= store.mapping_capacity {
            continue;
        }
        let record = unsafe { *store.mappings.add(index as usize) };
        writer.u64(record.as_generation);
        writer.u64(record.virtual_page);
        writer.u64(record.frame_address);
        writer.u32(record.frame_generation);
        writer.u32(record.flags);
        writer.u32(record.mapping_generation);
        writer.u8(record.state);
        writer.u8(record.probe_distance);
        writer.u16(0);
        mappings += 1;
    }
    writer.patch_u32(mappings_at, mappings);
    let after = SEQUENCE.load(Ordering::Acquire);
    u32::from(before != after || after & 1 != 0)
}

pub fn snapshot_flags() -> u32 {
    u32::from(SEQUENCE.load(Ordering::Acquire) & 1 != 0)
}

#[cfg(feature = "test")]
pub fn inject_double_release(index: usize) {
    release(index, FrameRefReason::Other, 0, 0xffff);
    release(index, FrameRefReason::Other, 0, 0xffff);
}

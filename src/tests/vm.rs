use crate::lib::test_utils::Testable;
use crate::userland::vm::{VmError, VmProt, Vma, VmaBacking, VmaSet};

fn anon(start: u64, end: u64, prot: VmProt) -> Vma {
    Vma::new(start, end, prot, VmaBacking::Anonymous).expect("valid anonymous VMA")
}

fn test_adjacent_compatible_vmas_merge() {
    let rw = VmProt::READ.union(VmProt::WRITE);
    let mut set = VmaSet::new();
    set.insert(anon(0x400000, 0x402000, rw)).unwrap();
    set.insert(anon(0x402000, 0x404000, rw)).unwrap();
    assert_eq!(set.as_slice().len(), 1);
    assert_eq!(set.as_slice()[0].start, 0x400000);
    assert_eq!(set.as_slice()[0].end, 0x404000);
}

fn test_remove_middle_splits_and_reuses_gap() {
    let rw = VmProt::READ.union(VmProt::WRITE);
    let mut set = VmaSet::new();
    set.insert(anon(0x400000, 0x408000, rw)).unwrap();
    set.remove(0x402000, 0x406000).unwrap();
    assert_eq!(set.as_slice().len(), 2);
    assert!(set.find(0x401000).is_some());
    assert!(set.find(0x403000).is_none());
    assert!(set.find(0x407000).is_some());
    assert!(set.is_free(0x402000, 0x406000));
}

fn test_protect_splits_and_requires_full_coverage() {
    let rw = VmProt::READ.union(VmProt::WRITE);
    let mut set = VmaSet::new();
    set.insert(anon(0x400000, 0x406000, rw)).unwrap();
    set.protect(0x402000, 0x404000, VmProt::READ).unwrap();
    assert_eq!(set.as_slice().len(), 3);
    assert_eq!(set.find(0x403000).unwrap().prot, VmProt::READ);
    assert_eq!(
        set.protect(0x405000, 0x407000, VmProt::NONE),
        Err(VmError::NotCovered)
    );
}

fn test_top_down_gap_search_and_overlap_rejection() {
    let mut set = VmaSet::new();
    set.insert(anon(0x800000, 0x900000, VmProt::READ)).unwrap();
    assert_eq!(set.find_gap_top_down(0x3000, 0x1000000), Ok(0xffd000));
    assert_eq!(
        set.insert(anon(0x8ff000, 0x901000, VmProt::READ)),
        Err(VmError::Overlap)
    );
}

fn test_reserved_kernel_slots_and_wrap_are_rejected() {
    let heap_slot_start = 136u64 << 39;
    assert_eq!(
        Vma::new(
            heap_slot_start,
            heap_slot_start + 0x1000,
            VmProt::READ,
            VmaBacking::Anonymous,
        )
        .unwrap_err(),
        VmError::ReservedRange
    );
    assert!(Vma::new(u64::MAX & !0xfff, 0, VmProt::READ, VmaBacking::Anonymous,).is_err());
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_adjacent_compatible_vmas_merge,
        &test_remove_middle_splits_and_reuses_gap,
        &test_protect_splits_and_requires_full_coverage,
        &test_top_down_gap_search_and_overlap_rejection,
        &test_reserved_kernel_slots_and_wrap_are_rejected,
    ]
}

//! Checked user-memory copying with explicit lazy-page and COW resolution.

use core::mem::MaybeUninit;
use x86_64::structures::paging::PageTableFlags;
use x86_64::VirtAddr;

use crate::userland::abi::EFAULT;
use crate::userland::vm::{VmProt, Vma, VmaBacking};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code, reason = "stable diagnostic terminal-reason schema")]
#[repr(u16)]
pub enum PageInTerminalReason {
    PresentCommitted = 0,
    NoAddressSpace = 1,
    NoVma = 2,
    PermissionDenied = 3,
    MapperUnavailable = 4,
    FrameAllocationFailed = 5,
    PhysicalAliasFailed = 6,
    FileExtentOverflow = 7,
    FileOffsetInvalid = 8,
    IoCompletionError = 9,
    ShortRead = 10,
    VmaChangedDuringIo = 11,
    LeafCollision = 12,
    MapFailed = 13,
    RollbackReleaseFailed = 14,
}

#[derive(Debug, Clone, Copy)]
struct PageInFailure {
    reason: PageInTerminalReason,
    requested: usize,
    actual: usize,
}

pub fn ensure_user_page(address: u64, write: bool) -> Result<(), i64> {
    let page = address & !0xfff;
    let context = crate::userland::lifecycle::with_current_group(|process| {
        let space = process.address_space.as_ref()?;
        Some((
            space.l4_frame(),
            space.vma_generation(),
            space.vmas().find(address)?.clone(),
        ))
    });
    // Kernel-only syscall tests intentionally use host stack buffers plus the
    // legacy explicit bounds hook. Real processes always have an AddressSpace.
    let Some((l4, vma_generation, vma)) = context else {
        let bounds = crate::userland::abi::user_va_bounds().ok_or(EFAULT)?;
        return (bounds.start <= address && address < bounds.end)
            .then_some(())
            .ok_or(EFAULT);
    };
    let pager = (crate::diagnostics::personality() != crate::diagnostics::Personality::Minimal)
        .then(|| {
            crate::diagnostics::shadow::pager::begin(
                crate::userland::lifecycle::current_user_pid().unwrap_or(0),
                l4.start_address().as_u64(),
                vma_generation,
                page,
            )
        })
        .flatten();
    let outcome = ensure_user_page_inner(page, write, l4, vma_generation, &vma, pager);
    let (reason, requested, actual) = match outcome {
        Ok((requested, actual)) => (PageInTerminalReason::PresentCommitted, requested, actual),
        Err(failure) => (failure.reason, failure.requested, failure.actual),
    };
    crate::diagnostics::trace::record(
        crate::diagnostics::trace::EventKind::PageInTerminal,
        page,
        u64::from(reason as u16) | ((requested as u64) << 16),
        actual as u64,
        pager.map_or(0, crate::diagnostics::shadow::pager::Handle::generation),
    );
    if let (Some(handle), Err(failure)) = (pager, outcome) {
        crate::diagnostics::shadow::pager::abort(
            handle,
            failure.reason as u16,
            failure.requested,
            failure.actual,
        );
    }
    outcome.map(|_| ()).map_err(|_| EFAULT)
}

fn ensure_user_page_inner(
    page: u64,
    write: bool,
    l4: x86_64::structures::paging::PhysFrame,
    vma_generation: u64,
    vma: &Vma,
    pager: Option<crate::diagnostics::shadow::pager::Handle>,
) -> Result<(usize, usize), PageInFailure> {
    let fail = |reason| PageInFailure {
        reason,
        requested: 0,
        actual: 0,
    };
    let required = if write { VmProt::WRITE } else { VmProt::READ };
    if !vma.prot.contains(required) {
        return Err(fail(PageInTerminalReason::PermissionDenied));
    }

    let private_frame = crate::mm::memory::with_memory_mapper(|mapper| {
        if let Some((_frame, flags)) = mapper.leaf_info(l4, VirtAddr::new(page)) {
            if write && !flags.contains(PageTableFlags::WRITABLE) {
                return match mapper.resolve_cow(l4, VirtAddr::new(page)) {
                    crate::mm::paging::CowOutcome::Copied
                    | crate::mm::paging::CowOutcome::Upgraded => Ok(None),
                    _ => Err(PageInTerminalReason::PermissionDenied),
                };
            }
            return Ok(None);
        }
        let frame = mapper
            .allocate_private_zeroed_frame()
            .ok_or(PageInTerminalReason::FrameAllocationFailed)?;
        Ok(Some(frame))
    })
    .ok_or_else(|| fail(PageInTerminalReason::MapperUnavailable))?
    .map_err(fail)?;

    let Some(frame) = private_frame else {
        if let Some(handle) = pager {
            crate::diagnostics::shadow::pager::observe_present(handle);
        }
        return Ok((0, 0));
    };
    if let Some(handle) = pager {
        crate::diagnostics::shadow::pager::reserve_frame(handle, frame.start_address().as_u64());
    }
    let physical = frame.start_address().as_u64();
    let Some(virtual_address) = crate::mm::memory::phys_to_virt(physical) else {
        release_private(frame);
        return Err(fail(PageInTerminalReason::PhysicalAliasFailed));
    };
    let destination =
        unsafe { core::slice::from_raw_parts_mut(virtual_address as *mut u8, 0x1000) };
    let populate: Result<(usize, usize), PageInFailure> = (|| match &vma.backing {
        VmaBacking::FilePrivate {
            file,
            file_offset,
            file_size,
        } => {
            let offset = file_offset
                .checked_add(page - vma.start)
                .ok_or_else(|| fail(PageInTerminalReason::FileExtentOverflow))?;
            if offset >= *file_size {
                return Err(fail(PageInTerminalReason::FileOffsetInvalid));
            }
            let available = (*file_size - offset).min(0x1000) as usize;
            let actual = file
                .read_at(offset, &mut destination[..available])
                .map_err(|_| PageInFailure {
                    reason: PageInTerminalReason::IoCompletionError,
                    requested: available,
                    actual: 0,
                })?;
            if actual != available {
                return Err(PageInFailure {
                    reason: PageInTerminalReason::ShortRead,
                    requested: available,
                    actual,
                });
            }
            Ok((available, actual))
        }
        VmaBacking::Elf {
            file,
            file_offset,
            file_len,
            ..
        } => {
            let relative = page - vma.start;
            if relative < *file_len {
                let available = (*file_len - relative).min(0x1000) as usize;
                let offset = file_offset
                    .checked_add(relative)
                    .ok_or_else(|| fail(PageInTerminalReason::FileExtentOverflow))?;
                let actual = file
                    .read_at(offset, &mut destination[..available])
                    .map_err(|_| PageInFailure {
                        reason: PageInTerminalReason::IoCompletionError,
                        requested: available,
                        actual: 0,
                    })?;
                if actual != available {
                    return Err(PageInFailure {
                        reason: PageInTerminalReason::ShortRead,
                        requested: available,
                        actual,
                    });
                }
                return Ok((available, actual));
            }
            Ok((0, 0))
        }
        VmaBacking::ElfResident
        | VmaBacking::Tls
        | VmaBacking::Stack { .. }
        | VmaBacking::Heap
        | VmaBacking::Anonymous => Ok((0, 0)),
    })();
    let (requested, actual) = match populate {
        Ok(counts) => counts,
        Err(error) => {
            release_private(frame);
            return Err(error);
        }
    };
    if let Some(handle) = pager {
        crate::diagnostics::shadow::pager::populated(
            handle,
            requested,
            actual,
            crate::diagnostics::wire::fnv1a64(destination),
        );
    }

    // Blocking I/O dropped every VM lock. Confirm the process still owns the
    // same L4 and semantically identical VMA before publishing the leaf.
    let unchanged = crate::userland::lifecycle::with_current_group(|process| {
        let Some(space) = process.address_space.as_ref() else {
            return false;
        };
        space.l4_frame() == l4
            && space.vma_generation() == vma_generation
            && space
                .vmas()
                .find(page)
                .is_some_and(|current| same_vma(current, vma))
    });
    if !unchanged {
        release_private(frame);
        return Err(PageInFailure {
            reason: PageInTerminalReason::VmaChangedDuringIo,
            requested,
            actual,
        });
    }

    let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
    if vma.prot.contains(VmProt::WRITE) {
        flags.insert(PageTableFlags::WRITABLE);
    }
    if !vma.prot.contains(VmProt::EXEC) {
        flags.insert(PageTableFlags::NO_EXECUTE);
    }
    let committed = crate::mm::memory::with_memory_mapper(|mapper| {
        if mapper.leaf_info(l4, VirtAddr::new(page)).is_some() {
            return Err(PageInTerminalReason::LeafCollision);
        }
        mapper
            .map_private_frame_into(l4, VirtAddr::new(page), frame, flags)
            .map_err(|_| PageInTerminalReason::MapFailed)
    });
    match committed {
        Some(Ok(())) => {
            if let Some(handle) = pager {
                crate::diagnostics::shadow::pager::commit(handle);
            }
            Ok((requested, actual))
        }
        Some(Err(reason)) => {
            release_private(frame);
            Err(PageInFailure {
                reason,
                requested,
                actual,
            })
        }
        None => {
            release_private(frame);
            Err(PageInFailure {
                reason: PageInTerminalReason::MapperUnavailable,
                requested,
                actual,
            })
        }
    }
}

fn release_private(frame: x86_64::structures::paging::PhysFrame) {
    let _ = crate::mm::memory::with_memory_mapper(|mapper| mapper.release_private_frame(frame));
}

fn same_vma(current: &Vma, original: &Vma) -> bool {
    if current.start != original.start
        || current.end != original.end
        || current.prot != original.prot
        || current.private != original.private
        || current.grow_down != original.grow_down
        || current.mapping_id != original.mapping_id
    {
        return false;
    }
    match (&current.backing, &original.backing) {
        (VmaBacking::ElfResident, VmaBacking::ElfResident)
        | (VmaBacking::Tls, VmaBacking::Tls)
        | (VmaBacking::Heap, VmaBacking::Heap)
        | (VmaBacking::Anonymous, VmaBacking::Anonymous) => true,
        (
            VmaBacking::Stack {
                floor: left_floor,
                guard_bytes: left_guard,
            },
            VmaBacking::Stack {
                floor: right_floor,
                guard_bytes: right_guard,
            },
        ) => left_floor == right_floor && left_guard == right_guard,
        (
            VmaBacking::Elf {
                file: left_file,
                file_offset: left_offset,
                file_len: left_len,
                zero_tail: left_tail,
            },
            VmaBacking::Elf {
                file: right_file,
                file_offset: right_offset,
                file_len: right_len,
                zero_tail: right_tail,
            },
        ) => {
            crate::lib::arc::Arc::ptr_eq(left_file, right_file)
                && left_offset == right_offset
                && left_len == right_len
                && left_tail == right_tail
        }
        (
            VmaBacking::FilePrivate {
                file: left_file,
                file_offset: left_offset,
                file_size: left_size,
            },
            VmaBacking::FilePrivate {
                file: right_file,
                file_offset: right_offset,
                file_size: right_size,
            },
        ) => {
            crate::lib::arc::Arc::ptr_eq(left_file, right_file)
                && left_offset == right_offset
                && left_size == right_size
        }
        _ => false,
    }
}

pub fn ensure_user_range(ptr: u64, len: u64, write: bool) -> Result<(), i64> {
    if len == 0 {
        return Ok(());
    }
    let end = ptr.checked_add(len).ok_or(EFAULT)?;
    let has_address_space =
        crate::userland::lifecycle::with_current_group(|process| process.address_space.is_some());
    if !has_address_space {
        let bounds = crate::userland::abi::user_va_bounds().ok_or(EFAULT)?;
        return (ptr >= bounds.start && end <= bounds.end)
            .then_some(())
            .ok_or(EFAULT);
    }
    let mut page = ptr & !0xfff;
    while page < end {
        ensure_user_page(page.max(ptr), write)?;
        page = page.checked_add(0x1000).ok_or(EFAULT)?;
    }
    Ok(())
}

pub fn copy_from_user(destination: &mut [u8], source: u64) -> Result<(), i64> {
    ensure_user_range(source, destination.len() as u64, false)?;
    unsafe {
        core::ptr::copy_nonoverlapping(
            source as *const u8,
            destination.as_mut_ptr(),
            destination.len(),
        );
    }
    Ok(())
}

pub fn copy_to_user(destination: u64, source: &[u8]) -> Result<(), i64> {
    ensure_user_range(destination, source.len() as u64, true)?;
    unsafe {
        core::ptr::copy_nonoverlapping(source.as_ptr(), destination as *mut u8, source.len());
    }
    Ok(())
}

pub fn read_unaligned<T: Copy>(source: u64) -> Result<T, i64> {
    let mut value = MaybeUninit::<T>::uninit();
    let bytes = unsafe {
        core::slice::from_raw_parts_mut(value.as_mut_ptr() as *mut u8, core::mem::size_of::<T>())
    };
    copy_from_user(bytes, source)?;
    Ok(unsafe { value.assume_init() })
}

pub fn write_unaligned<T>(destination: u64, value: &T) -> Result<(), i64> {
    let bytes = unsafe {
        core::slice::from_raw_parts(value as *const T as *const u8, core::mem::size_of::<T>())
    };
    copy_to_user(destination, bytes)
}

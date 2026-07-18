//! Checked user-memory copying with explicit lazy-page and COW resolution.

use core::mem::MaybeUninit;
use x86_64::structures::paging::PageTableFlags;
use x86_64::VirtAddr;

use crate::userland::abi::EFAULT;
use crate::userland::vm::{VmProt, VmaBacking};

pub fn ensure_user_page(address: u64, write: bool) -> Result<(), i64> {
    let page = address & !0xfff;
    let context = crate::userland::lifecycle::with_current_group(|process| {
        let space = process.address_space.as_ref()?;
        Some((space.l4_frame(), space.vmas().find(address)?.clone()))
    });
    // Kernel-only syscall tests intentionally use host stack buffers plus the
    // legacy explicit bounds hook. Real processes always have an AddressSpace.
    let Some((l4, vma)) = context else {
        let bounds = crate::userland::abi::user_va_bounds().ok_or(EFAULT)?;
        return (bounds.start <= address && address < bounds.end)
            .then_some(())
            .ok_or(EFAULT);
    };
    let required = if write { VmProt::WRITE } else { VmProt::READ };
    if !vma.prot.contains(required) {
        return Err(EFAULT);
    }

    let mapped = crate::mm::memory::with_memory_mapper(|mapper| {
        if let Some((_frame, flags)) = mapper.leaf_info(l4, VirtAddr::new(page)) {
            if write && !flags.contains(PageTableFlags::WRITABLE) {
                return match mapper.resolve_cow(l4, VirtAddr::new(page)) {
                    crate::mm::paging::CowOutcome::Copied
                    | crate::mm::paging::CowOutcome::Upgraded => Ok(None),
                    _ => Err(EFAULT),
                };
            }
            return Ok(None);
        }

        let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
        if vma.prot.contains(VmProt::WRITE) {
            flags.insert(PageTableFlags::WRITABLE);
        }
        if !vma.prot.contains(VmProt::EXEC) {
            flags.insert(PageTableFlags::NO_EXECUTE);
        }
        let frame = mapper
            .map_zeroed_page_into(l4, VirtAddr::new(page), flags)
            .map_err(|_| EFAULT)?;
        Ok(Some(frame))
    })
    .unwrap_or(Err(EFAULT))?;

    // Never retain the global mapper lock while file-backed paging sleeps on
    // DMA. The frame remains owned by the address space mapping, and the
    // direct physical map gives us a stable destination while blocked.
    let Some(frame) = mapped else { return Ok(()) };
    let physical = frame.start_address().as_u64();
    let virtual_address = crate::mm::memory::phys_to_virt(physical).ok_or(EFAULT)?;
    let destination =
        unsafe { core::slice::from_raw_parts_mut(virtual_address as *mut u8, 0x1000) };
    let populate = (|| {
        match &vma.backing {
            VmaBacking::FilePrivate {
                file,
                file_offset,
                file_size,
            } => {
                let offset = file_offset + (page - vma.start);
                if offset >= *file_size {
                    return Err(EFAULT);
                }
                let available = (*file_size - offset).min(0x1000) as usize;
                if file.read_at(offset, &mut destination[..available]).is_err() {
                    return Err(EFAULT);
                }
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
                    if file
                        .read_at(file_offset + relative, &mut destination[..available])
                        .is_err()
                    {
                        return Err(EFAULT);
                    }
                }
            }
            VmaBacking::ElfResident
            | VmaBacking::Tls
            | VmaBacking::Stack { .. }
            | VmaBacking::Heap
            | VmaBacking::Anonymous => {}
        }
        Ok(())
    })();
    if populate.is_err() {
        let _ = crate::mm::memory::with_memory_mapper(|mapper| {
            mapper.unmap_page_from(l4, VirtAddr::new(page))
        });
    }
    populate
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

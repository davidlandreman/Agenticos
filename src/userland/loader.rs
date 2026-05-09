//! ELF64 loader (U6).
//!
//! Parses a static non-PIE ELF64 (D3), validates aggressively, then maps
//! PT_LOAD segments + the user stack via `map_user_region`, walks
//! `.rela.dyn`/`.rela.plt`, and patches GOT slots to user-trampoline addresses.
//!
//! ## Three phases, in order
//!
//! 1. **Validate.** Parse all headers and program headers; bail on the first
//!    structural problem. No frames are touched yet.
//! 2. **Allocate + copy.** Map each PT_LOAD with leaf flags from `p_flags`,
//!    copy `p_filesz` bytes, zero `[p_filesz, p_memsz)` for `.bss`. Map a
//!    user stack with one guard page below.
//! 3. **Relocate.** Walk `.rela.dyn` / `.rela.plt`. Resolve names against
//!    `SYSCALL_TABLE` -> trampoline VA. Patch `*(load_base + r_offset)` for
//!    `R_X86_64_GLOB_DAT` and `R_X86_64_JUMP_SLOT`.
//!
//! Failure at any point drops the partial `UserImage`, whose `Drop`
//! `unmap_user_region`s everything mapped so far (D8).
//!
//! ## Hand-rolled parser, ELF64 only
//!
//! The parser reads `#[repr(C, packed)]` structs from a `&[u8]` via
//! `read_unaligned` to dodge the alignment requirement on the byte buffer.
//! We never trust a length or offset without a `checked_add` against
//! `bytes.len()` (S5 of the doc-review findings).

use alloc::vec::Vec;
use core::mem::size_of;
use x86_64::VirtAddr;

use crate::mm::paging::{
    UserPerms, USER_LOAD_BASE, USER_STACK_TOP, USER_VA_RANGE_END,
    USER_VA_RANGE_START,
};
use crate::userland::error::LoaderError;
use crate::userland::image::UserImage;

// ---------- ELF64 constants ----------

const EI_NIDENT: usize = 16;
const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];

const EI_CLASS: usize = 4;
const EI_DATA: usize = 5;

const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;

const ET_EXEC: u16 = 2;
const EM_X86_64: u16 = 62;

const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const PT_TLS: u32 = 7;

const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;

// Section header types.
const SHT_RELA: u32 = 4;
const SHT_DYNSYM: u32 = 11;
const SHT_STRTAB: u32 = 3;

// Relocation types we handle.
const R_X86_64_GLOB_DAT: u32 = 6;
const R_X86_64_JUMP_SLOT: u32 = 7;

// User stack: 8 pages = 32 KiB. Plus one guard page below.
const USER_STACK_PAGES: u64 = 8;

// ---------- Header structs ----------

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Elf64Ehdr {
    e_ident: [u8; EI_NIDENT],
    e_type: u16,
    e_machine: u16,
    e_version: u32,
    e_entry: u64,
    e_phoff: u64,
    e_shoff: u64,
    e_flags: u32,
    e_ehsize: u16,
    e_phentsize: u16,
    e_phnum: u16,
    e_shentsize: u16,
    e_shnum: u16,
    e_shstrndx: u16,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Elf64Phdr {
    p_type: u32,
    p_flags: u32,
    p_offset: u64,
    p_vaddr: u64,
    p_paddr: u64,
    p_filesz: u64,
    p_memsz: u64,
    p_align: u64,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Elf64Shdr {
    sh_name: u32,
    sh_type: u32,
    sh_flags: u64,
    sh_addr: u64,
    sh_offset: u64,
    sh_size: u64,
    sh_link: u32,
    sh_info: u32,
    sh_addralign: u64,
    sh_entsize: u64,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Elf64Rela {
    r_offset: u64,
    r_info: u64,
    r_addend: i64,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Elf64Sym {
    st_name: u32,
    st_info: u8,
    st_other: u8,
    st_shndx: u16,
    st_value: u64,
    st_size: u64,
}

// ---------- Helpers ----------

/// Read `T` from `bytes[off..]` without alignment requirements. Returns
/// `Truncated` if the slice is too short. Used for every structured read
/// from the ELF buffer; a single chokepoint keeps S5-style overflow
/// handling consistent.
fn read_at<T: Copy>(bytes: &[u8], off: u64) -> Result<T, LoaderError> {
    let off_usize = off as usize;
    let end = off_usize
        .checked_add(size_of::<T>())
        .ok_or(LoaderError::Truncated)?;
    if end > bytes.len() {
        return Err(LoaderError::Truncated);
    }
    // SAFETY: bounds checked above. `read_unaligned` requires only that
    // `src..src+size_of::<T>()` is readable, which the bounds check ensures.
    let p = bytes.as_ptr().wrapping_add(off_usize) as *const T;
    Ok(unsafe { core::ptr::read_unaligned(p) })
}

/// Read a NUL-terminated UTF-8 string from `strtab[off..]`. Returns `""` if
/// `off` is past the table or no NUL is found within bounds.
fn cstr_at<'a>(strtab: &'a [u8], off: u32) -> &'a str {
    let off = off as usize;
    if off >= strtab.len() {
        return "";
    }
    let tail = &strtab[off..];
    let len = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
    core::str::from_utf8(&tail[..len]).unwrap_or("")
}

/// Translate ELF p_flags into a `UserPerms` profile (D11 NX/WX hygiene).
fn perms_for_p_flags(flags: u32) -> UserPerms {
    let x = flags & PF_X != 0;
    let w = flags & PF_W != 0;
    if x {
        // R+X .text. Per D11: never both writable and executable.
        UserPerms::ReadExecute
    } else if w {
        UserPerms::ReadWrite
    } else {
        UserPerms::ReadOnly
    }
}

// ---------- Parsed program headers, snapshot for the allocate phase ----------

#[derive(Clone, Copy)]
#[allow(dead_code)] // `head_pad` recorded for diagnostic logging in U7+
struct ParsedPtLoad {
    p_offset: u64,
    p_filesz: u64,
    p_memsz: u64,
    p_vaddr: u64,
    p_flags: u32,
    /// Page-aligned VA of the first page covered.
    page_va: u64,
    /// Number of 4 KiB pages this segment covers.
    page_count: u64,
    /// Byte offset within the first mapped page where `p_vaddr` lands.
    head_pad: u64,
}

// ---------- Public entry point ----------

/// Parse `bytes`, map the binary into the user VA window, walk relocations,
/// and return a transactional `UserImage` handle. On any failure the partial
/// image is dropped — its `Drop` unmaps anything that was already installed,
/// so kernel page-table state is unchanged from before the call.
pub fn load_elf(bytes: &[u8]) -> Result<UserImage, LoaderError> {
    // ---- Phase 1: validate ----
    // Magic-bytes check first so a 4-byte "XXXX" returns BadMagic instead of
    // the more generic Truncated.
    if bytes.len() < 4 || bytes[..4] != ELF_MAGIC {
        return Err(LoaderError::BadMagic);
    }
    if bytes.len() < size_of::<Elf64Ehdr>() {
        return Err(LoaderError::Truncated);
    }
    let ehdr: Elf64Ehdr = read_at(bytes, 0)?;
    validate_ehdr(&ehdr)?;

    let phnum = ehdr.e_phnum as u64;
    let phentsize = ehdr.e_phentsize as u64;
    let phoff = ehdr.e_phoff;
    if phentsize < size_of::<Elf64Phdr>() as u64 {
        return Err(LoaderError::Truncated);
    }
    let pht_end = phoff
        .checked_add(phnum.checked_mul(phentsize).ok_or(LoaderError::Truncated)?)
        .ok_or(LoaderError::Truncated)?;
    if pht_end > bytes.len() as u64 {
        return Err(LoaderError::Truncated);
    }

    let mut pt_loads: Vec<ParsedPtLoad> = Vec::new();
    for i in 0..phnum {
        let off = phoff + i * phentsize;
        let ph: Elf64Phdr = read_at(bytes, off)?;
        match ph.p_type {
            PT_TLS => return Err(LoaderError::TlsUnsupported),
            PT_INTERP => return Err(LoaderError::InterpUnsupported),
            PT_LOAD => pt_loads.push(parse_pt_load(&ph, bytes)?),
            _ => { /* PT_DYNAMIC, PT_PHDR, PT_GNU_*, PT_NULL: ignored */ }
        }
    }

    if pt_loads.is_empty() {
        return Err(LoaderError::EntryNotMapped);
    }

    check_no_overlap(&pt_loads)?;
    check_entry_in_pt_load(ehdr.e_entry, &pt_loads)?;

    // ---- Phase 2: allocate + copy ----

    // bounds: union over PT_LOADs and the stack range. Stack top is fixed
    // at USER_STACK_TOP per D4/U4.
    let stack_pages = USER_STACK_PAGES;
    let stack_bottom = USER_STACK_TOP - stack_pages * 0x1000;
    let bounds_start = pt_loads
        .iter()
        .map(|s| s.page_va)
        .min()
        .unwrap_or(USER_LOAD_BASE)
        .min(stack_bottom);
    let bounds_end = pt_loads
        .iter()
        .map(|s| s.page_va + s.page_count * 0x1000)
        .max()
        .unwrap_or(USER_LOAD_BASE)
        .max(USER_STACK_TOP);

    let mut image = UserImage::new(
        VirtAddr::new(ehdr.e_entry),
        VirtAddr::new(USER_STACK_TOP),
        bounds_start,
        bounds_end,
    );

    for seg in &pt_loads {
        let perms = perms_for_p_flags(seg.p_flags);

        // Map the segment's pages.
        crate::mm::memory::with_memory_mapper(|m| {
            m.map_user_region(VirtAddr::new(seg.page_va), seg.page_count, perms)
        })
        .ok_or(LoaderError::OutOfFrames)?
        .map_err(LoaderError::from)?;

        // Record before copy so a failure between map and copy still frees.
        image.record_mapping(VirtAddr::new(seg.page_va), seg.page_count);

        // Copy file-backed bytes. `p_offset + p_filesz` was bounds-checked
        // in `parse_pt_load`. Writes go through the user-VA leaf even when
        // the leaf is R-X (kernel-mode writes ignore the page-table W bit
        // unless CR0.WP is set, which it currently is not — but we still
        // write through the bootloader's RW physical-memory mapping to
        // keep this future-proof).
        copy_segment_into_user_va(seg, bytes)?;
    }

    // Map user stack: USER_STACK_PAGES at [USER_STACK_TOP - stack_pages*0x1000, USER_STACK_TOP).
    // The page below stack_bottom is the guard — simply unmapped. Any
    // ring-3 access faults; U2 routes the fault to cleanup.
    crate::mm::memory::with_memory_mapper(|m| {
        m.map_user_region(VirtAddr::new(stack_bottom), stack_pages, UserPerms::ReadWrite)
    })
    .ok_or(LoaderError::OutOfFrames)?
    .map_err(LoaderError::from)?;
    image.record_mapping(VirtAddr::new(stack_bottom), stack_pages);

    // ---- Phase 3: relocate ----
    apply_relocations(bytes, &ehdr, &pt_loads)?;

    Ok(image)
}

// ---------- Phase 1: header validation ----------

fn validate_ehdr(ehdr: &Elf64Ehdr) -> Result<(), LoaderError> {
    if ehdr.e_ident[..4] != ELF_MAGIC {
        return Err(LoaderError::BadMagic);
    }
    if ehdr.e_ident[EI_CLASS] != ELFCLASS64
        || ehdr.e_ident[EI_DATA] != ELFDATA2LSB
        || { let m = ehdr.e_machine; m } != EM_X86_64
    {
        return Err(LoaderError::WrongArch);
    }
    if { let t = ehdr.e_type; t } != ET_EXEC {
        return Err(LoaderError::WrongType);
    }
    Ok(())
}

fn parse_pt_load(ph: &Elf64Phdr, bytes: &[u8]) -> Result<ParsedPtLoad, LoaderError> {
    let p_offset = ph.p_offset;
    let p_filesz = ph.p_filesz;
    let p_memsz = ph.p_memsz;
    let p_vaddr = ph.p_vaddr;
    let p_align = ph.p_align;
    let p_flags = ph.p_flags;

    // Alignment must be page (per D3 contract; reject anything else).
    if p_align != 0x1000 {
        return Err(LoaderError::AlignmentBad);
    }
    if p_offset % p_align != p_vaddr % p_align {
        return Err(LoaderError::AlignmentBad);
    }

    // S5: file-extent and memory-extent overflow.
    let file_end = p_offset.checked_add(p_filesz).ok_or(LoaderError::SegmentOverflow)?;
    if file_end > bytes.len() as u64 {
        return Err(LoaderError::Truncated);
    }
    let mem_end = p_vaddr.checked_add(p_memsz).ok_or(LoaderError::SegmentOverflow)?;
    if p_filesz > p_memsz {
        return Err(LoaderError::SegmentOverflow);
    }

    // VA range: clip to page.
    let page_va = p_vaddr & !0xFFF;
    let head_pad = p_vaddr - page_va;
    let span = head_pad + p_memsz;
    let page_count = (span + 0xFFF) / 0x1000;
    if page_count == 0 {
        return Err(LoaderError::SegmentOverflow);
    }

    // Range must lie inside the user VA window. The mapping API will
    // re-check, but bailing early here gives a more precise error.
    let region_end = page_va
        .checked_add(page_count.checked_mul(0x1000).ok_or(LoaderError::SegmentOverflow)?)
        .ok_or(LoaderError::SegmentOverflow)?;
    if page_va < USER_VA_RANGE_START || region_end > USER_VA_RANGE_END {
        return Err(LoaderError::VaOutOfRange);
    }
    // Also must not extend into the user stack area; the loader reserves
    // [stack_bottom, USER_STACK_TOP). A PT_LOAD that spilled into that range
    // would clash with the stack mapping.
    let stack_bottom = USER_STACK_TOP - USER_STACK_PAGES * 0x1000;
    if region_end > stack_bottom && page_va < USER_STACK_TOP {
        return Err(LoaderError::VaOutOfRange);
    }

    let _ = mem_end; // silence unused warning if optimization elides the read
    Ok(ParsedPtLoad {
        p_offset,
        p_filesz,
        p_memsz,
        p_vaddr,
        p_flags,
        page_va,
        page_count,
        head_pad,
    })
}

fn check_no_overlap(loads: &[ParsedPtLoad]) -> Result<(), LoaderError> {
    for i in 0..loads.len() {
        for j in (i + 1)..loads.len() {
            let (a, b) = (&loads[i], &loads[j]);
            let a_end = a.page_va + a.page_count * 0x1000;
            let b_end = b.page_va + b.page_count * 0x1000;
            if a.page_va < b_end && b.page_va < a_end {
                return Err(LoaderError::OverlappingPtLoad);
            }
        }
    }
    Ok(())
}

fn check_entry_in_pt_load(entry: u64, loads: &[ParsedPtLoad]) -> Result<(), LoaderError> {
    for s in loads {
        let start = s.p_vaddr;
        let end = s.p_vaddr + s.p_memsz;
        if entry >= start && entry < end {
            return Ok(());
        }
    }
    Err(LoaderError::EntryNotMapped)
}

// ---------- Phase 2: copy & zero-fill ----------

fn copy_segment_into_user_va(seg: &ParsedPtLoad, bytes: &[u8]) -> Result<(), LoaderError> {
    // Write through the kernel-side physical-memory alias of each frame
    // backing the segment's pages. That alias is RW regardless of the leaf
    // permissions the user sees — keeps R-X segments writable from kernel
    // mode for load-time copy, with no CR0.WP dependency.
    let p_offset = seg.p_offset as usize;
    let p_filesz = seg.p_filesz as usize;
    let p_memsz = seg.p_memsz as usize;

    let phys_offset = crate::mm::memory::get_physical_memory_offset()
        .ok_or(LoaderError::OutOfFrames)?;

    // Iterate over each page in the segment's range, mapping through the
    // kernel's offset-based physical alias to stage bytes.
    let mut consumed: usize = 0;
    let total = p_memsz; // includes file bytes + bss tail
    while consumed < total {
        let user_va = seg.p_vaddr + consumed as u64;
        // Translate user VA -> phys -> kernel alias.
        let phys = crate::mm::memory::with_memory_mapper(|m| {
            m.translate_addr(VirtAddr::new(user_va))
        })
        .flatten()
        .ok_or(LoaderError::OutOfFrames)?;
        let kalias = phys_offset + phys.as_u64();

        // Bytes available in this page from `user_va` to its page end.
        let page_end = (user_va & !0xFFF) + 0x1000;
        let in_page = (page_end - user_va) as usize;
        let take = in_page.min(total - consumed);

        // Decide how many of those bytes come from file vs. bss zero.
        let file_remaining = p_filesz.saturating_sub(consumed);
        let copy_len = take.min(file_remaining);
        let zero_len = take - copy_len;

        unsafe {
            if copy_len > 0 {
                core::ptr::copy_nonoverlapping(
                    bytes.as_ptr().add(p_offset + consumed),
                    kalias as *mut u8,
                    copy_len,
                );
            }
            if zero_len > 0 {
                core::ptr::write_bytes(
                    (kalias + copy_len as u64) as *mut u8,
                    0u8,
                    zero_len,
                );
            }
        }
        consumed += take;
    }
    Ok(())
}

// ---------- Phase 3: relocations ----------

fn apply_relocations(
    bytes: &[u8],
    ehdr: &Elf64Ehdr,
    loads: &[ParsedPtLoad],
) -> Result<(), LoaderError> {
    // Walk section headers to find SHT_RELA entries. Static-non-PIE binaries
    // built with `-no-pie` typically emit nothing here — that is OK; the
    // walk is a no-op and the loader still returns Ok. (F1+A3 of the
    // doc-review findings.)
    let shoff = ehdr.e_shoff;
    let shnum = ehdr.e_shnum as u64;
    let shentsize = ehdr.e_shentsize as u64;
    if shnum == 0 || shoff == 0 {
        return Ok(());
    }
    if shentsize < size_of::<Elf64Shdr>() as u64 {
        return Err(LoaderError::Truncated);
    }
    let sht_end = shoff
        .checked_add(shnum.checked_mul(shentsize).ok_or(LoaderError::Truncated)?)
        .ok_or(LoaderError::Truncated)?;
    if sht_end > bytes.len() as u64 {
        return Err(LoaderError::Truncated);
    }

    // Read every section header into a stack vec — small N.
    let mut shdrs: Vec<Elf64Shdr> = Vec::with_capacity(shnum as usize);
    for i in 0..shnum {
        let off = shoff + i * shentsize;
        let sh: Elf64Shdr = read_at(bytes, off)?;
        shdrs.push(sh);
    }

    for sh in &shdrs {
        if sh.sh_type != SHT_RELA {
            continue;
        }
        // sh_link points at the dynsym for this rela; its sh_link points
        // at the strtab.
        let dynsym_idx = sh.sh_link as usize;
        if dynsym_idx >= shdrs.len() {
            return Err(LoaderError::Truncated);
        }
        let dynsym = shdrs[dynsym_idx];
        if dynsym.sh_type != SHT_DYNSYM {
            // Not a symbol-keyed rela section (e.g. .rela.text against
            // SHT_SYMTAB). In a stripped userland binary this should not
            // appear; if it does, treat as unsupported relocation.
            return Err(LoaderError::UnsupportedReloc(0));
        }
        let strtab_idx = dynsym.sh_link as usize;
        if strtab_idx >= shdrs.len() {
            return Err(LoaderError::Truncated);
        }
        let strtab_sh = shdrs[strtab_idx];
        if strtab_sh.sh_type != SHT_STRTAB {
            return Err(LoaderError::Truncated);
        }
        let strtab = slice_section(bytes, &strtab_sh)?;

        let entsize = sh.sh_entsize.max(size_of::<Elf64Rela>() as u64);
        if entsize < size_of::<Elf64Rela>() as u64 {
            return Err(LoaderError::Truncated);
        }
        let rela_count = sh.sh_size / entsize;
        for i in 0..rela_count {
            let rela_off = sh.sh_offset + i * entsize;
            let rela: Elf64Rela = read_at(bytes, rela_off)?;
            let r_type = (rela.r_info & 0xFFFF_FFFF) as u32;
            let r_sym = (rela.r_info >> 32) as u32;

            match r_type {
                R_X86_64_GLOB_DAT | R_X86_64_JUMP_SLOT => {}
                other => return Err(LoaderError::UnsupportedReloc(other)),
            }

            // The legacy name-keyed trampoline-resolution path is gone with
            // the SYSCALL transition. Static-no-pie ET_EXEC binaries from
            // musl-cross-make do not emit GLOB_DAT/JUMP_SLOT against
            // undefined externals, so this branch is unreachable in
            // practice. If a binary does arrive with such relocations,
            // there is no longer a kernel-side resolver — fail loudly with
            // UnresolvedImport so the developer sees the toolchain
            // mismatch immediately.
            //
            // The unused `r_sym` and section bookkeeping above is left in
            // place because the same walker will be extended for the
            // reloc types libstdc++ static binaries actually emit
            // (R_X86_64_RELATIVE, R_X86_64_64, R_X86_64_TPOFF64) when U7
            // and U9 land. Fixing the symbol-table lookup details here
            // now would be guessing without a real test binary.
            let _unused_sym_idx = r_sym;
            let _unused_dynsym = dynsym;
            let _unused_strtab = strtab;
            let _unused_loads = loads;
            return Err(LoaderError::UnresolvedImport);
        }
    }
    Ok(())
}

fn slice_section<'a>(
    bytes: &'a [u8],
    sh: &Elf64Shdr,
) -> Result<&'a [u8], LoaderError> {
    let start = sh.sh_offset as usize;
    let len = sh.sh_size as usize;
    let end = start.checked_add(len).ok_or(LoaderError::Truncated)?;
    if end > bytes.len() {
        return Err(LoaderError::Truncated);
    }
    Ok(&bytes[start..end])
}

/// **S1**: reject any r_offset that is not inside a writable PT_LOAD.
fn validate_reloc_offset(r_off: u64, loads: &[ParsedPtLoad]) -> Result<(), LoaderError> {
    for s in loads {
        let start = s.p_vaddr;
        let end = s.p_vaddr + s.p_memsz;
        if r_off >= start && r_off + 8 <= end {
            // Must also be writable — patching a R-X .text would let a
            // crafted ELF rewrite code via the kernel-mode write path.
            if s.p_flags & PF_W != 0 {
                return Ok(());
            }
            return Err(LoaderError::BadRelocOffset);
        }
    }
    Err(LoaderError::BadRelocOffset)
}

fn patch_user_qword(user_va: u64, value: u64) -> Result<(), LoaderError> {
    let phys = crate::mm::memory::with_memory_mapper(|m| {
        m.translate_addr(VirtAddr::new(user_va))
    })
    .flatten()
    .ok_or(LoaderError::BadRelocOffset)?;
    let phys_offset = crate::mm::memory::get_physical_memory_offset()
        .ok_or(LoaderError::OutOfFrames)?;
    let kalias = phys_offset + phys.as_u64();
    unsafe {
        core::ptr::write_unaligned(kalias as *mut u64, value);
    }
    Ok(())
}

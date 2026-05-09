//! Loader error type (U6).
//!
//! Every failure mode the ELF loader (`crate::userland::loader`) can return.
//! Distinct variants for each validation step let tests assert on the exact
//! reason a malformed input was rejected. `Debug` derive is sufficient — the
//! plan does not require `Display`, and these errors propagate to the shell
//! verb (U7) as opaque values that get logged, not formatted at the user.

use crate::mm::paging::UserMapError;

/// All ways the loader can refuse a binary or fail mid-load. Mid-load failures
/// trigger the transactional `Drop` rollback inside `UserImage`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoaderError {
    /// First four bytes are not `\x7FELF`.
    BadMagic,
    /// `e_ident[EI_CLASS] != ELFCLASS64`, `e_ident[EI_DATA] != ELFDATA2LSB`,
    /// or `e_machine != EM_X86_64`.
    WrongArch,
    /// `e_type != ET_EXEC` (we accept only static non-PIE executables, D3).
    WrongType,
    /// File ended before a header field, segment payload, or section the
    /// loader needed could be read. Also covers `e_phoff + phnum * phentsize`
    /// running past `bytes.len()`.
    Truncated,
    /// Two PT_LOAD segments cover overlapping virtual address ranges.
    OverlappingPtLoad,
    /// A PT_LOAD segment falls outside `USER_VA_RANGE_START..USER_VA_RANGE_END`.
    VaOutOfRange,
    /// `e_entry` does not lie inside any PT_LOAD segment.
    EntryNotMapped,
    /// Encountered a relocation type the loader does not implement. Carries
    /// the numeric type so failures are diagnosable without hex-decoding.
    UnsupportedReloc(u32),
    /// A relocation referenced a symbol whose name is not present in the
    /// `SYSCALL_TABLE` registry. The string is a `&'static` slice into a
    /// loader-internal buffer and is only valid for the lifetime of the
    /// loader call, but for the failure path that is sufficient — the
    /// caller logs and discards.
    UnresolvedImport,
    /// PT_TLS encountered. Rejected explicitly rather than silently ignored.
    TlsUnsupported,
    /// PT_INTERP encountered (we do not run a dynamic linker).
    InterpUnsupported,
    /// `p_align != 0x1000` or `p_offset % align != p_vaddr % align`.
    AlignmentBad,
    /// Frame allocator exhausted while mapping a PT_LOAD or stack page.
    OutOfFrames,
    /// `r_offset` for a relocation does not lie inside a writable PT_LOAD
    /// segment. **Defends S1 of the doc-review findings**: a crafted ELF
    /// with `r_offset = 0xFFFF_8000_0000_0000` would otherwise let a kernel-
    /// mode write corrupt arbitrary kernel memory during relocation.
    BadRelocOffset,
    /// A PT_LOAD's `p_offset + p_filesz` overflowed `u64` or ran past
    /// `bytes.len()`, or `p_vaddr + p_memsz` overflowed. **Defends S5**.
    SegmentOverflow,
    /// Internal: a mapping API call failed with a `UserMapError` that is not
    /// already covered by a more specific variant above.
    MappingFailed(UserMapError),
}

impl From<UserMapError> for LoaderError {
    fn from(e: UserMapError) -> Self {
        match e {
            UserMapError::OutOfFrames => LoaderError::OutOfFrames,
            UserMapError::VaOutOfRange => LoaderError::VaOutOfRange,
            other => LoaderError::MappingFailed(other),
        }
    }
}

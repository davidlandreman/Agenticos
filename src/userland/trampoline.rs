//! User-trampoline page (D4).
//!
//! A 4 KiB page mapped into user space at `USER_TRAMPOLINE_VA`, R+X+USER+NX-off.
//! Contains one syscall stub per registered syscall:
//!
//! ```asm
//! mov rax, <id>     ; B8 <id32>     -- 5 bytes
//! int 0x80          ; CD 80         -- 2 bytes
//! ret               ; C3            -- 1 byte
//! ```
//!
//! Total per-stub: 8 bytes. With 64-syscall ceiling that is 512 bytes — well
//! under the 4 KiB page budget. The U6 ELF loader resolves a user-imported
//! name to `(USER_TRAMPOLINE_VA + 8 * id)` and patches GOT slots accordingly.
//!
//! The page is built lazily on first user-app load (U7) rather than at boot,
//! because building it requires the heap allocator and frame allocator to be
//! up, and because new syscalls registered between boot and first user-app
//! load should still appear. The U5 deliverable here is the builder + tests;
//! mapping into the live user address space is exercised in U7.

use alloc::vec::Vec;
use spin::Mutex;
use x86_64::VirtAddr;

use crate::mm::paging::{UserMapError, UserPerms, USER_TRAMPOLINE_VA};
use crate::userland::abi::{snapshot_registry, MAX_SYSCALLS};

/// Bytes per stub: 5 (mov rax, imm32) + 2 (int 0x80) + 1 (ret).
pub const STUB_SIZE: usize = 8;

/// Resolved trampoline-VA entry for a registered syscall. The U6 loader uses
/// this to fill GOT slots: `R_X86_64_GLOB_DAT` / `R_X86_64_JUMP_SLOT` for
/// name `print` resolves to the address recorded here.
#[derive(Debug, Clone, Copy)]
pub struct TrampolineSymbol {
    pub name: &'static str,
    pub va: u64,
}

/// In-memory record of where each syscall stub lives in the trampoline page,
/// once the page has been emitted. Empty until `build_and_map_trampoline_page`
/// has run.
static TRAMPOLINE_SYMBOLS: Mutex<Vec<TrampolineSymbol>> = Mutex::new(Vec::new());

/// True once the trampoline page is mapped into the user address space.
static TRAMPOLINE_MAPPED: Mutex<bool> = Mutex::new(false);

/// Emit a single stub `mov rax, <id>; int 0x80; ret` into `dst[0..STUB_SIZE]`.
/// Pure function; tested directly in unit tests.
pub fn emit_stub(id: u32, dst: &mut [u8]) {
    assert!(dst.len() >= STUB_SIZE, "stub buffer too small");
    // mov rax, imm32 -- using the 5-byte form `B8 imm32` which zero-extends
    // to RAX in 64-bit mode. (The 64-bit immediate form `48 B8 imm64` is
    // 10 bytes; we only ever need IDs < 2^32, so the short form is enough.)
    //
    // Note: the bytes literally encode `mov eax, imm32`, but in 64-bit mode
    // writing to a 32-bit dest zero-extends into the full 64-bit register,
    // so the observable effect on RAX is identical for IDs that fit in u32.
    dst[0] = 0xB8;
    let id_le = id.to_le_bytes();
    dst[1] = id_le[0];
    dst[2] = id_le[1];
    dst[3] = id_le[2];
    dst[4] = id_le[3];
    // int 0x80
    dst[5] = 0xCD;
    dst[6] = 0x80;
    // ret
    dst[7] = 0xC3;
}

/// Build the trampoline page contents and return the 4 KiB byte buffer plus
/// the per-symbol VA records. Pure function — does not touch page tables;
/// tested in isolation. The returned byte buffer is what would be copied
/// into the freshly mapped trampoline page.
pub fn build_trampoline_bytes() -> (Vec<u8>, Vec<TrampolineSymbol>) {
    let mut bytes = alloc::vec![0u8; 0x1000];
    let mut symbols: Vec<TrampolineSymbol> = Vec::new();
    let registry = snapshot_registry();
    for id in 0..MAX_SYSCALLS {
        if let (Some(name), id_recorded) = registry[id] {
            debug_assert_eq!(id_recorded, id);
            let off = id * STUB_SIZE;
            emit_stub(id as u32, &mut bytes[off..off + STUB_SIZE]);
            symbols.push(TrampolineSymbol {
                name,
                va: USER_TRAMPOLINE_VA + off as u64,
            });
        }
    }
    (bytes, symbols)
}

/// Map the trampoline page into user VA and copy the synthesized stub bytes
/// into it. Idempotent: subsequent calls return `Ok(())` without remapping.
///
/// The page is mapped R+X+USER (NX off, per `UserPerms::ReadExecute`); writes
/// from kernel mode go through the bootloader's physical-memory offset
/// mapping (which is RW even though the user-VA leaf is R+X), so we can fill
/// the page contents *after* mapping.
pub fn build_and_map_trampoline_page() -> Result<(), UserMapError> {
    {
        if *TRAMPOLINE_MAPPED.lock() {
            return Ok(());
        }
    }

    let (bytes, symbols) = build_trampoline_bytes();

    let frames = crate::mm::memory::with_memory_mapper(|m| {
        m.map_user_region(
            VirtAddr::new(USER_TRAMPOLINE_VA),
            1,
            UserPerms::ReadExecute,
        )
    })
    .ok_or(UserMapError::OutOfFrames)??;

    // Copy stub bytes into the freshly mapped page via the kernel-visible VA
    // (the user VA is R+X — writing through it would page-fault). The
    // physical frame's kernel alias lives at `phys_offset + frame_pa`.
    let frame = frames[0];
    let phys_offset = crate::mm::memory::get_physical_memory_offset()
        .ok_or(UserMapError::OutOfFrames)?;
    let dst_va = phys_offset + frame.start_address().as_u64();
    unsafe {
        core::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            dst_va as *mut u8,
            0x1000,
        );
    }

    *TRAMPOLINE_SYMBOLS.lock() = symbols;
    *TRAMPOLINE_MAPPED.lock() = true;
    Ok(())
}

/// Resolve a symbol name to its trampoline VA. Returns `None` if the
/// trampoline page has not been built yet or if the name is unregistered.
/// U6 will call this from the relocation-walk to fill GOT slots.
pub fn resolve(name: &str) -> Option<u64> {
    TRAMPOLINE_SYMBOLS
        .lock()
        .iter()
        .find(|s| s.name == name)
        .map(|s| s.va)
}

/// Test-only access to the recorded symbol set.
#[cfg(feature = "test")]
pub fn snapshot_symbols() -> Vec<TrampolineSymbol> {
    TRAMPOLINE_SYMBOLS.lock().clone()
}

/// Test-only: tear down the trampoline-page state so a follow-up test can
/// re-map it. Used only by the U5 test suite.
#[cfg(feature = "test")]
pub fn reset_for_test() {
    if *TRAMPOLINE_MAPPED.lock() {
        let _ = crate::mm::memory::with_memory_mapper(|m| {
            m.unmap_user_region(VirtAddr::new(USER_TRAMPOLINE_VA), 1)
        });
        *TRAMPOLINE_MAPPED.lock() = false;
        TRAMPOLINE_SYMBOLS.lock().clear();
    }
}

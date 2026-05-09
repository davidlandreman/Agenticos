// Userland subsystem: ring-3 ELF apps loaded from /host.
//
// See `docs/plans/2026-05-08-004-feat-userland-app-platform-plan.md` for the
// design. This subsystem is built up across implementation units U1..U8.

pub mod abi;
pub mod error;
pub mod image;
pub mod lifecycle;
pub mod loader;
pub mod syscalls;

use core::arch::naked_asm;

use x86_64::VirtAddr;

use crate::userland::image::UserImage;
use crate::userland::lifecycle::{
    install_continuation, with_active_user, ExitKind, KernelContinuation,
};

// ---------- Linux x86-64 auxv constants ----------

const AT_NULL: u64 = 0;
const AT_PHDR: u64 = 3;
const AT_PHENT: u64 = 4;
const AT_PHNUM: u64 = 5;
const AT_PAGESZ: u64 = 6;
const AT_RANDOM: u64 = 25;

const ELF64_PHDR_SIZE: u64 = 56;

/// 16 fixed bytes the initial-stack builder copies onto the user stack
/// for `AT_RANDOM`. musl reads bytes 8..16 as the stack-canary seed and
/// hashes the full 16 for other internal uses. A fixed value is acceptable
/// per the milestone scope ("quality AT_RANDOM entropy is out of scope");
/// real entropy is a follow-up swap.
const AT_RANDOM_BYTES: [u8; 16] = [
    0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42,
    0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42,
];

/// argv[0] for ring-3 binaries. musl uses this only as a fallback for
/// `program_invocation_name`; the actual path is irrelevant to the
/// milestone surface (`write` / `exit_group`). A fixed string keeps the
/// kernel free of plumbing for the per-binary path.
const ARGV0: &[u8] = b"agenticos-app\0";

/// Errors that can happen at the lifecycle/entry layer (after the loader has
/// already produced a `UserImage`).
#[derive(Debug, Clone, Copy)]
pub enum EnterError {
    /// A user app is already active. Single-app-synchronous (D5).
    AlreadyActive,
}

/// Enter ring 3 with `image` as the live user binary.
///
/// **Diverges through the long-jump.** Returns to the caller only after the
/// user app exits (cooperatively via the `exit` syscall, or abnormally via a
/// fault routed through `cleanup_user_process`). On return, the active-user
/// slot has been populated with the exit kind/code; the run command reads
/// those, drops the `UserImage`, and reports back to the shell.
///
/// Steps:
/// 1. Reject if another user app is currently active (D5).
/// 2. Lazy-map the trampoline page (no-op after the first call).
/// 3. Install the active-user slot: take ownership of `image`, populate the
///    syscall pointer-validation bounds, clear any prior exit info.
/// 4. Stamp `TSS.privilege_stack_table[0]` with the kernel rsp0 stack top so
///    the CPU has somewhere to switch to on the next ring 3 → ring 0 trap.
/// 5. Build the iretq frame (user_ss=0x1B, user_rsp, rflags=0x202,
///    user_cs=0x23, user_rip=image.entry) and execute it from the
///    naked-asm setjmp prologue. The prologue saves callee-saved regs +
///    RSP + a resume label as the kernel continuation before iretq-ing.
///
/// On long-jump back, control resumes at the resume label inside
/// `enter_user_mode_asm`, which `ret`s to this function. We then read the
/// active-user slot to extract the exit kind/code and return them to the
/// caller.
pub fn enter_user_mode(image: UserImage) -> Result<(ExitKind, i64), EnterError> {
    // D5: only one user app at a time.
    with_active_user(|au| {
        if au.image.is_some() {
            return Err(EnterError::AlreadyActive);
        }
        Ok(())
    })?;

    // Capture the bits the initial-stack builder needs before the image
    // moves into the active-user slot.
    let entry = image.entry.as_u64();
    let stack_top = image.stack_top.as_u64();
    let bounds = crate::userland::abi::UserVaBounds {
        start: image.bounds_start,
        end: image.bounds_end,
    };
    let phdr_bytes = image.phdr_bytes.clone();
    let e_phnum = image.e_phnum;

    // The initial-stack builder writes through the kernel-visible alias
    // of the user stack pages — those are mapped R+W by the loader.
    let user_rsp = build_initial_stack(stack_top, &phdr_bytes, e_phnum);

    with_active_user(|au| {
        au.image = Some(image);
        au.exit_kind = ExitKind::None;
        au.exit_code = 0;
        au.brk_current = crate::mm::paging::USER_BRK_BASE;
        au.mmap_next = crate::mm::paging::USER_MMAP_BASE;
    });
    crate::userland::abi::set_user_va_bounds(bounds);

    // D6: TSS rsp0 = kernel rsp0 stack top.
    let rsp0 = crate::arch::x86_64::gdt::kernel_rsp0_top();
    unsafe {
        crate::arch::x86_64::gdt::set_kernel_rsp0(rsp0);
    }

    // Selectors. RPL=3 baked into the lower bits.
    let sel = crate::arch::x86_64::gdt::selectors();
    let user_cs = sel.user_code.0 as u64;
    let user_ss = sel.user_data.0 as u64;

    // S4: sanitize RFLAGS for ring-3 entry. Reserved bit 1 set, IF set,
    // IOPL=0, TF/NT/RF clear. 0x202 captures exactly that.
    let user_rflags: u64 = 0x202;

    // SAFETY: callee-saved regs + RSP are saved into the active-user slot's
    // continuation by the asm prologue; `iretq` then transitions to ring 3.
    // On exit/fault, `restore_continuation` jumps back to the resume label
    // and the function continues normally.
    unsafe {
        enter_user_mode_asm(entry, user_rsp, user_rflags, user_cs, user_ss);
    }

    // Long-jumped back. Read the recorded exit reason.
    let (kind, code) = with_active_user(|au| (au.exit_kind, au.exit_code));

    // Clear the syscall pointer-validation bounds — no user pointers are
    // valid until the next `run`.
    crate::userland::abi::clear_user_va_bounds();

    Ok((kind, code))
}

/// Build the Linux x86-64 initial stack frame.
///
/// Layout at `_start` (low addr to high), per the System V AMD64 psABI:
///
/// ```text
///   RSP -> argc                                       (1 qword)
///          argv[0] = ptr to argv0 string              (1 qword)
///          argv[1] = NULL                             (1 qword)
///          envp[0] = NULL                             (1 qword)
///          auxv pairs: AT_PHDR / AT_PHENT / AT_PHNUM /
///                       AT_PAGESZ / AT_RANDOM / AT_NULL  (6 × 16 bytes)
///          phdr_table (e_phnum × 56 bytes, padded to 16 align)
///          AT_RANDOM payload (16 bytes)
///          argv0 string ("agenticos-app\0", padded to 16 align)
/// ```
///
/// `RSP` is 16-aligned at `_start` per the ABI. The frame is built in
/// kernel mode by writing through the kernel-visible alias of the user
/// stack pages (mapped R+W by the loader). `_start` reads through ring-3
/// loads, which see the same bytes.
///
/// The phdrs are placed on the stack rather than at `USER_LOAD_BASE +
/// e_phoff` because the kernel's hand-rolled fixtures don't include the
/// program headers inside any PT_LOAD; copying them onto the stack lets
/// the same code path serve fixtures and real musl-cross-make binaries.
fn build_initial_stack(stack_top: u64, phdr_bytes: &[u8], e_phnum: u16) -> u64 {
    // Sizes (all 16-aligned where practical to keep argc 16-aligned).
    let argv0_size: u64 = align_up_16(ARGV0.len() as u64);
    let random_size: u64 = 16;
    let phdr_size: u64 = align_up_16((e_phnum as u64) * ELF64_PHDR_SIZE);
    // 6 auxv pairs × 16 bytes each.
    let auxv_size: u64 = 6 * 16;
    let envp_size: u64 = 8;          // single NULL terminator
    let argv_size: u64 = 8 + 8;      // argv[0] + NULL
    let argc_size: u64 = 8;
    let frame_size: u64 = argv0_size
        + random_size
        + phdr_size
        + auxv_size
        + envp_size
        + argv_size
        + argc_size;
    debug_assert!(frame_size % 16 == 0, "frame_size must be 16-aligned");

    let frame_base: u64 = stack_top - frame_size;

    // Compute addresses (low to high).
    let mut p = frame_base;
    let argc_at = p; p += argc_size;
    let argv0_ptr_at = p; p += 8;
    let argv1_at = p; p += 8;
    let envp0_at = p; p += 8;
    let auxv_at = p; p += auxv_size;
    let phdr_at = p; p += phdr_size;
    let random_at = p; p += random_size;
    let argv0_str_at = p; p += argv0_size;
    debug_assert_eq!(p, stack_top, "stack frame layout drift");

    // SAFETY: the user stack pages [stack_top - 8*0x1000, stack_top) are
    // mapped R+W by the loader, and we are CPL=0 — kernel writes ignore
    // the USER bit and the leaf flags. The frame fits inside the topmost
    // page (frame_size ≤ ~512 bytes for a typical phdr count).
    unsafe {
        // argc = 1
        write_u64_at(argc_at, 1);

        // argv[0] = argv0_str_at, argv[1] = NULL
        write_u64_at(argv0_ptr_at, argv0_str_at);
        write_u64_at(argv1_at, 0);

        // envp[0] = NULL
        write_u64_at(envp0_at, 0);

        // auxv pairs (low-to-high in declared order — order doesn't
        // matter to musl as long as AT_NULL terminates the list).
        let mut a = auxv_at;
        write_u64_at(a, AT_PHDR);    a += 8;
        write_u64_at(a, phdr_at);    a += 8;
        write_u64_at(a, AT_PHENT);   a += 8;
        write_u64_at(a, ELF64_PHDR_SIZE); a += 8;
        write_u64_at(a, AT_PHNUM);   a += 8;
        write_u64_at(a, e_phnum as u64); a += 8;
        write_u64_at(a, AT_PAGESZ);  a += 8;
        write_u64_at(a, 0x1000);     a += 8;
        write_u64_at(a, AT_RANDOM);  a += 8;
        write_u64_at(a, random_at);  a += 8;
        write_u64_at(a, AT_NULL);    a += 8;
        write_u64_at(a, 0);          let _ = a;

        // Copy phdr bytes onto the stack.
        let dst = phdr_at as *mut u8;
        core::ptr::copy_nonoverlapping(phdr_bytes.as_ptr(), dst, phdr_bytes.len());

        // Copy AT_RANDOM payload.
        let dst = random_at as *mut u8;
        core::ptr::copy_nonoverlapping(AT_RANDOM_BYTES.as_ptr(), dst, AT_RANDOM_BYTES.len());

        // Copy argv0 string.
        let dst = argv0_str_at as *mut u8;
        core::ptr::copy_nonoverlapping(ARGV0.as_ptr(), dst, ARGV0.len());
    }

    argc_at
}

#[inline]
fn align_up_16(n: u64) -> u64 {
    (n + 15) & !15
}

/// SAFETY: `addr` must point at a writable user-VA page mapped by the
/// loader; the kernel must be in CPL=0.
#[inline]
unsafe fn write_u64_at(addr: u64, value: u64) {
    core::ptr::write_unaligned(addr as *mut u64, value);
}

/// Setjmp prologue + ring-3 transition.
///
/// Inputs (System V ABI):
/// - `RDI` = user RIP (entry)
/// - `RSI` = user RSP
/// - `RDX` = user RFLAGS
/// - `RCX` = user CS
/// - `R8`  = user SS
///
/// Behavior:
/// 1. Save callee-saved regs (RBX, RBP, R12-R15), RSP, and the address of
///    the resume label into a `KernelContinuation` on the local stack.
/// 2. Call `lifecycle::install_continuation` with that struct.
/// 3. Build the iretq frame and `iretq` to ring 3.
/// 4. Resume label: when `restore_continuation` jumps here, the saved RSP
///    has already been restored, so `ret` returns from this function.
///
/// SAFETY:
/// - Must be called from CPL=0 with interrupts enabled (the iretq sets
///   IF=1 on the way in via RFLAGS=0x202; we explicitly do not need a
///   `cli`/`sti` dance because the TSS is already loaded with the same
///   kernel rsp0 stack we are running on, and the syscall path will not
///   land us elsewhere).
/// - The user CS/SS values must have RPL=3.
/// - `entry` must be inside a USER-mapped, executable page.
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn enter_user_mode_asm(
    _entry: u64,    // RDI
    _user_rsp: u64, // RSI
    _rflags: u64,   // RDX
    _user_cs: u64,  // RCX
    _user_ss: u64,  // R8
) {
    naked_asm!(
        // ----- Phase 1: build KernelContinuation on the local stack -----
        //
        // Allocate 64 bytes (8 qwords): rbx, rbp, r12, r13, r14, r15, rsp, rip.
        // We push them in struct order so layout matches `KernelContinuation`.
        //
        // After the user app exits, `restore_continuation` will load RSP
        // from offset +48 of the saved struct; at that point the saved
        // struct is *also* on the same stack we're about to leave, but the
        // reload makes that irrelevant — RSP becomes whatever we record in
        // the +48 slot. We record the value of RSP *as it should be on
        // resume*: just past the saved struct, so the matching `ret` at
        // the resume label has a clean stack to unwind to.
        //
        // Layout (low addr first):
        //  [rsp +  0] rbx
        //  [rsp +  8] rbp
        //  [rsp + 16] r12
        //  [rsp + 24] r13
        //  [rsp + 32] r14
        //  [rsp + 40] r15
        //  [rsp + 48] rsp_on_resume  (= rsp before this allocation)
        //  [rsp + 56] rip            (= 1f)
        //
        // We compute rsp_on_resume = rsp_now + 64 (the 8 qwords we just
        // allocated below).
        "sub rsp, 64",
        "mov [rsp + 0], rbx",
        "mov [rsp + 8], rbp",
        "mov [rsp + 16], r12",
        "mov [rsp + 24], r13",
        "mov [rsp + 32], r14",
        "mov [rsp + 40], r15",
        "lea rax, [rsp + 64]",          // rsp value to restore on resume
        "mov [rsp + 48], rax",
        "lea rax, [rip + 2f]",          // resume RIP
        "mov [rsp + 56], rax",

        // ----- Phase 2: install_continuation(&saved) -----
        //
        // System V: 1st arg in RDI. We stash the user-mode arg regs across
        // the call by saving them on the stack first, since `install_continuation`
        // is a regular Rust function and may clobber any caller-saved reg.
        //
        // Save: RDI (entry), RSI (user_rsp), RDX (rflags), RCX (user_cs), R8 (user_ss).
        "push rdi",
        "push rsi",
        "push rdx",
        "push rcx",
        "push r8",
        // The continuation lives at the original [rsp + 64] (we pushed 5 more
        // qwords -> +40 above the original), but `install_continuation` only
        // needs to read the contents — it copies into the global slot. Pass
        // a pointer to it via RDI.
        "lea rdi, [rsp + 40]",
        "call {install_continuation}",
        // Restore the user-mode arg regs.
        "pop r8",
        "pop rcx",
        "pop rdx",
        "pop rsi",
        "pop rdi",

        // ----- Phase 3: build iretq frame and transfer to ring 3 -----
        //
        // The CPU expects (from low to high addr on the kernel stack):
        //   RIP, CS, RFLAGS, RSP, SS
        // i.e. push in reverse order: SS, RSP, RFLAGS, CS, RIP.
        //
        // Inputs are still: RDI=entry, RSI=user_rsp, RDX=rflags, RCX=user_cs, R8=user_ss.
        "push r8",      // SS
        "push rsi",     // RSP
        "push rdx",     // RFLAGS
        "push rcx",     // CS
        "push rdi",     // RIP

        // Wipe GP regs we don't want leaking into ring 3. The user app's
        // `_start` is `extern "C"` but receives no arguments by convention,
        // so zeroing is fine. RAX/RCX/RDX may leak (we just used them); we
        // explicitly zero them here. R10/R11 are scratch in the SysV ABI.
        "xor rax, rax",
        "xor rbx, rbx",
        "xor rcx, rcx",
        "xor rdx, rdx",
        "xor rsi, rsi",
        "xor rdi, rdi",
        "xor rbp, rbp",
        "xor r8, r8",
        "xor r9, r9",
        "xor r10, r10",
        "xor r11, r11",
        "xor r12, r12",
        "xor r13, r13",
        "xor r14, r14",
        "xor r15, r15",

        "iretq",

        // ----- Resume label -----
        //
        // `restore_continuation` lands here with RSP = saved rsp_on_resume
        // (which is the original RSP at function entry, prior to our 64-byte
        // allocation). All callee-saved regs have already been restored.
        // A plain `ret` returns to the caller.
        "2:",
        "ret",

        install_continuation = sym install_continuation_thunk,
    );
}

/// Thin C-callable shim around `lifecycle::install_continuation`. The naked
/// stub references this by `sym` to dodge cross-crate-name-mangling concerns.
#[no_mangle]
extern "C" fn install_continuation_thunk(c: *const KernelContinuation) {
    // SAFETY: the asm prologue passes a pointer to a stack-local struct that
    // lives until the function returns; we copy out by-value into the global.
    let cont = unsafe { *c };
    install_continuation(cont);
}

/// Drop the active `UserImage`, if any. Called by the run command after
/// `enter_user_mode` returns. Separated out so tests can inspect the
/// active-user state before the image is released.
pub fn release_active_image() -> Option<UserImage> {
    with_active_user(|au| au.image.take())
}

/// Force-clear continuation/exit state without touching the image. Test-only
/// — used by the U7 test driver to recover after a synthetic `enter_user_mode`.
#[cfg(feature = "test")]
pub fn force_clear_active_for_test() {
    with_active_user(|au| {
        au.continuation = None;
        au.image = None;
        au.exit_kind = ExitKind::None;
        au.exit_code = 0;
        au.brk_current = 0;
        au.mmap_next = 0;
    });
    crate::userland::abi::clear_user_va_bounds();
}

// Suppress unused-import warning when only some entries are needed.
#[allow(dead_code)]
fn _va_addr_silencer(_v: VirtAddr) {}

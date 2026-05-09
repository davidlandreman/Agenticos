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
pub mod trampoline;

use core::arch::naked_asm;

use x86_64::VirtAddr;

use crate::userland::image::UserImage;
use crate::userland::lifecycle::{
    install_continuation, with_active_user, ExitKind, KernelContinuation,
};

/// Errors that can happen at the lifecycle/entry layer (after the loader has
/// already produced a `UserImage`).
#[derive(Debug, Clone, Copy)]
pub enum EnterError {
    /// A user app is already active. Single-app-synchronous (D5).
    AlreadyActive,
    /// The trampoline page failed to map.
    TrampolineMapFailed,
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

    // Lazy-map the trampoline (idempotent after the first call). The loader
    // already calls this, but `enter_user_mode` may be invoked directly from
    // tests, so guard here too.
    crate::userland::trampoline::build_and_map_trampoline_page()
        .map_err(|_| EnterError::TrampolineMapFailed)?;

    // Stamp the syscall pointer-validation bounds, install the image, clear
    // any prior exit. Single critical section.
    let entry = image.entry.as_u64();
    let stack_top = image.stack_top.as_u64();
    let bounds = crate::userland::abi::UserVaBounds {
        start: image.bounds_start,
        end: image.bounds_end,
    };
    with_active_user(|au| {
        au.image = Some(image);
        au.exit_kind = ExitKind::None;
        au.exit_code = 0;
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

    // The user RSP is one qword below stack_top so that `_start`'s
    // first-instruction stack alignment matches the System V "post-call"
    // 16-byte invariant. This matches the loader's record (`stack_top`) and
    // the kernel-process trampoline pattern in `CpuContext::init_for_new_process`.
    let user_rsp = stack_top - 8;

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
    });
    crate::userland::abi::clear_user_va_bounds();
}

// Suppress unused-import warning when only some entries are needed.
#[allow(dead_code)]
fn _va_addr_silencer(_v: VirtAddr) {}

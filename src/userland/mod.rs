// Userland subsystem: ring-3 ELF apps loaded from /host.
//
// See `docs/plans/2026-05-08-004-feat-userland-app-platform-plan.md` for the
// design. This subsystem is built up across implementation units U1..U8.

pub mod abi;
pub mod address_space;
pub mod bin_namespace;
pub mod error;
pub mod fdtable;
pub mod image;
pub mod kernel_stack;
pub mod launcher;
pub mod lifecycle;
pub mod loader;
pub mod path;
pub mod pipe;
pub mod signal;
pub mod stdin;
pub mod syscalls;
pub mod tty;
pub mod user_state;

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

/// Default argv[0] when the caller doesn't supply one. Tests and the
/// zero-arg `enter_user_mode(image)` wrapper use this. Real launches via
/// the `run` shell command pass the file path as argv[0].
const DEFAULT_ARGV0: &str = "agenticos-app";

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
    enter_user_mode_with(image, &[DEFAULT_ARGV0], &[])
}

/// Like `enter_user_mode`, but with caller-provided argv and envp.
///
/// `argv[0]` becomes `program_invocation_name` for the user program; later
/// entries are addressable via `argv` in `int main(int, char**, char**)`.
/// `envp` is presented as a null-terminated array of `KEY=VALUE` strings
/// preceding the auxv. Empty `argv` is filled with `DEFAULT_ARGV0`.
pub fn enter_user_mode_with(
    image: UserImage,
    argv: &[&str],
    envp: &[&str],
) -> Result<(ExitKind, i64), EnterError> {
    enter_user_mode_with_aspace(image, argv, envp, None)
}

/// Like `enter_user_mode_with`, but takes an already-built
/// `AddressSpace` to install on the new process. The caller (typically
/// `RunProcess::run_path`) constructed the L4, activated it, and then
/// loaded the ELF into it; we just need to record ownership on the
/// process so cleanup at exit drops the L4 frame too. Tests skip the
/// address space (passing `None`) and run on the kernel L4 directly.
pub fn enter_user_mode_with_aspace(
    image: UserImage,
    argv: &[&str],
    envp: &[&str],
    address_space: Option<crate::userland::address_space::AddressSpace>,
) -> Result<(ExitKind, i64), EnterError> {
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

    // Default argv[0] when the caller didn't supply anything — keeps musl
    // happy without forcing every test path to thread a path string.
    let default_argv = [DEFAULT_ARGV0];
    let argv_slice: &[&str] = if argv.is_empty() { &default_argv } else { argv };

    // The initial-stack builder writes through the kernel-visible alias
    // of the user stack pages — those are mapped R+W by the loader.
    let user_rsp = build_initial_stack(stack_top, &phdr_bytes, e_phnum, argv_slice, envp);

    // Phase 4 PR-A/B: install a fresh Process slot with a real PID
    // and (when one was provided) the L4 page table the loader mapped
    // into. Tests that bypass the run command pass `None` and stay on
    // the kernel L4.
    let _new_pid = crate::userland::lifecycle::install_new_process_opt(
        image,
        crate::mm::paging::USER_BRK_BASE,
        crate::mm::paging::USER_MMAP_BASE,
        address_space,
    );
    // U3: stash the launch path so `readlink("/proc/self/exe")` and
    // tools like zsh's `$ZSH_ARGZERO` resolution can recover it. We use
    // argv[0] as the canonical exe path — the run command and execve
    // both pass the FAT path there. Synthetic test launches that pass an
    // empty argv get the placeholder DEFAULT_ARGV0; that's fine because
    // those paths don't exercise readlink anyway.
    let exe_path = alloc::string::String::from(argv_slice[0]);
    crate::userland::lifecycle::with_active_user(|p| {
        p.exe_path = Some(exe_path);
    });
    // Phase 5 PR-B2 test hook: lets tests pre-install a signal action
    // before the user process starts running. Production launches
    // never set this; release builds compile it out.
    #[cfg(feature = "test")]
    {
        if let Some((sig, action)) = test_hooks::take_pre_iretq_signal_action() {
            with_active_user(|p| {
                p.signal_state.set_action(sig, action);
            });
        }
    }
    crate::userland::abi::set_user_va_bounds(bounds);
    // Phase 1 stdin: install an empty queue for `read(0, …)` to consume.
    // Cleared by `release_active_image` after the long-jump returns.
    crate::userland::stdin::install();
    // Phase 3 tty: reset termios to the default canonical/echo profile
    // so each user binary starts in a known-good state.
    crate::userland::tty::install_default();

    // Phase 5 PR-C1: point both TSS.rsp0 (used by interrupt gates)
    // and the SYSCALL stub's `gs:[0]` slot at this process's own
    // kernel stack — we just allocated it inside install_new_process.
    let rsp0 = crate::userland::lifecycle::with_current_process(|p| {
        p.kernel_stack.as_ref().expect("kernel_stack installed").top()
    });
    unsafe {
        crate::arch::x86_64::gdt::set_kernel_rsp0(rsp0);
        crate::arch::x86_64::syscall::set_percpu_kernel_rsp_top(rsp0.as_u64());
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
///          argv[0..argc]                              (argc × 8 bytes)
///          argv[argc] = NULL                          (1 qword)
///          envp[0..envc]                              (envc × 8 bytes)
///          envp[envc] = NULL                          (1 qword)
///          auxv pairs: AT_PHDR / AT_PHENT / AT_PHNUM /
///                       AT_PAGESZ / AT_RANDOM / AT_NULL  (6 × 16 bytes)
///          phdr_table (e_phnum × 56 bytes, padded to 16 align)
///          AT_RANDOM payload (16 bytes)
///          string pool: argv strings then envp strings, each NUL-terminated
///                       (padded to 16 align for the topmost frame edge)
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
pub(crate) fn build_initial_stack(
    stack_top: u64,
    phdr_bytes: &[u8],
    e_phnum: u16,
    argv: &[&str],
    envp: &[&str],
) -> u64 {
    let argc = argv.len() as u64;
    let envc = envp.len() as u64;

    // String-pool size: every argv/envp string contributes len + 1 (NUL).
    // Pad the whole pool up to 16 so the highest frame address sits on a
    // 16-byte boundary, which keeps argc 16-aligned.
    let strings_raw: u64 = argv.iter().map(|s| s.len() as u64 + 1).sum::<u64>()
        + envp.iter().map(|s| s.len() as u64 + 1).sum::<u64>();
    let strings_size: u64 = align_up_16(strings_raw);

    let random_size: u64 = 16;
    let phdr_size: u64 = align_up_16((e_phnum as u64) * ELF64_PHDR_SIZE);
    // 6 auxv pairs × 16 bytes each.
    let auxv_size: u64 = 6 * 16;
    let envp_array_size: u64 = (envc + 1) * 8;
    let argv_array_size: u64 = (argc + 1) * 8;
    let argc_size: u64 = 8;

    // The argv/envp arrays may have an odd number of pointers, which
    // breaks 16-alignment of argc. Insert a one-qword pad after auxv
    // when needed so RSP at argc is 16-aligned.
    let unaligned_top: u64 =
        argc_size + argv_array_size + envp_array_size + auxv_size + phdr_size + random_size + strings_size;
    let head_pad: u64 = if unaligned_top % 16 == 0 { 0 } else { 8 };
    let frame_size: u64 = unaligned_top + head_pad;
    debug_assert!(frame_size % 16 == 0, "frame_size must be 16-aligned");

    let frame_base: u64 = stack_top - frame_size;

    // Compute addresses (low to high).
    let mut p = frame_base;
    let argc_at = p; p += argc_size;
    let argv_at = p; p += argv_array_size;
    let envp_at = p; p += envp_array_size;
    let _pad_at = p; p += head_pad;
    let auxv_at = p; p += auxv_size;
    let phdr_at = p; p += phdr_size;
    let random_at = p; p += random_size;
    let strings_at = p; p += strings_size;
    debug_assert_eq!(p, stack_top, "stack frame layout drift");

    // SAFETY: the user stack pages [stack_top - 8*0x1000, stack_top) are
    // mapped R+W by the loader, and we are CPL=0 — kernel writes ignore
    // the USER bit and the leaf flags. The frame fits inside the topmost
    // page (frame_size ≤ ~512 bytes for a typical phdr count).
    unsafe {
        // argc
        write_u64_at(argc_at, argc);

        // argv pointers + NULL terminator. Strings are emitted into the
        // pool below; we record each string's address as we write it.
        let mut str_cursor = strings_at;
        for (i, s) in argv.iter().enumerate() {
            let str_addr = str_cursor;
            let bytes = s.as_bytes();
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), str_addr as *mut u8, bytes.len());
            *((str_addr + bytes.len() as u64) as *mut u8) = 0;
            str_cursor += bytes.len() as u64 + 1;
            write_u64_at(argv_at + (i as u64) * 8, str_addr);
        }
        write_u64_at(argv_at + argc * 8, 0);

        // envp pointers + NULL terminator.
        for (i, s) in envp.iter().enumerate() {
            let str_addr = str_cursor;
            let bytes = s.as_bytes();
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), str_addr as *mut u8, bytes.len());
            *((str_addr + bytes.len() as u64) as *mut u8) = 0;
            str_cursor += bytes.len() as u64 + 1;
            write_u64_at(envp_at + (i as u64) * 8, str_addr);
        }
        write_u64_at(envp_at + envc * 8, 0);

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

/// Phase 4 PR-D: iretq into ring 3 with caller-supplied registers,
/// **without** setjmp.
///
/// Used by `execve()` after the new image is loaded — the existing
/// kernel continuation (set by either `enter_user_mode_with` for
/// kernel-launched binaries, or `enter_user_mode_with_regs_asm` inside
/// `fork()`) stays in place. When the new program eventually exits
/// via `cooperative_exit`, it long-jumps to that pre-existing
/// continuation, which is what we want — exec'd children flow back to
/// `fork()`'s caller, exec'd top-level binaries flow back to the
/// `run` command.
///
/// Diverges. Caller's stack frame is abandoned.
///
/// SAFETY: `state` must describe a valid ring-3 entry point (RIP in a
/// USER-mapped executable page in the active L4, RSP in a USER-mapped
/// writable page in the same L4). `user_cs` / `user_ss` must carry
/// RPL=3 and refer to GDT entries with DPL=3.
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn iretq_to_user_with_regs(
    _state: *const crate::userland::user_state::UserState, // RDI
    _user_cs: u64,                                          // RSI
    _user_ss: u64,                                          // RDX
) -> ! {
    naked_asm!(
        // Build IRETQ frame (low → high after pushes: RIP, CS, RFLAGS, RSP, SS).
        "push rdx",                  // SS
        "mov rax, [rdi + 72]",
        "push rax",                  // user RSP
        "mov rax, [rdi + 120]",
        "push rax",                  // RFLAGS
        "push rsi",                  // CS
        "mov rax, [rdi + 112]",
        "push rax",                  // RIP

        // Load user GP regs from UserState.
        "mov r11, rdi",              // r11 = state ptr
        "mov rax, [r11 + 0]",
        "mov rbx, [r11 + 56]",
        "mov rbp, [r11 + 64]",
        "mov r12, [r11 + 80]",
        "mov r13, [r11 + 88]",
        "mov r14, [r11 + 96]",
        "mov r15, [r11 + 104]",
        "mov r10, [r11 + 32]",
        "mov r8,  [r11 + 40]",
        "mov r9,  [r11 + 48]",
        "mov rdx, [r11 + 24]",
        "mov rsi, [r11 + 16]",
        "mov rdi, [r11 + 8]",        // load rdi LAST (we used it as state ptr)
        // SYSCALL ABI clobbers rcx/r11; zero them for cleanliness so a
        // ring-3 program that incorrectly assumes them preserved gets
        // a deterministic value rather than kernel data.
        "xor r11, r11",
        "xor rcx, rcx",

        "iretq",
    );
}

/// Phase 4 PR-C2: setjmp + iretq with caller-supplied user registers.
///
/// Variant of `enter_user_mode_asm` for `fork()`'s child dispatch. Saves
/// kernel state into a `KernelContinuation` (so when the child
/// `_exit`s the kernel can long-jump back here), then loads the child's
/// full GP-register snapshot from `state` and `iretq`s into ring 3.
///
/// On long-jump return (after the child exits), control resumes at the
/// `2:` label and `ret`s back to the caller — same setjmp/longjmp
/// shape as `enter_user_mode_asm`.
///
/// `user_cs` is `0x23` (GDT slot 4 | RPL=3) and `user_ss` is `0x1B`
/// (GDT slot 3 | RPL=3). The caller passes them rather than us
/// hard-coding so the same helper could be reused if selectors ever
/// shift.
///
/// SAFETY: `state` must be a valid `UserState` describing a complete
/// ring-3 register frame. `state.rsp` must lie inside a USER-mapped
/// writable page in the currently active L4 (caller is responsible
/// for the CR3 switch before invoking this), and `state.rip` must be
/// inside a USER-mapped executable page in the same L4. Failures are
/// not recoverable — they fault inside ring 3 and route through the
/// existing fault-cleanup path.
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn enter_user_mode_with_regs_asm(
    _state: *const crate::userland::user_state::UserState, // RDI
    _user_cs: u64,                                          // RSI
    _user_ss: u64,                                          // RDX
) {
    naked_asm!(
        // ----- Phase 1: build KernelContinuation on the local stack -----
        // 64 bytes, same layout as `enter_user_mode_asm`:
        //  [rsp +  0..40] callee-saved (rbx, rbp, r12, r13, r14, r15)
        //  [rsp + 48]     rsp_on_resume (rsp_now + 64)
        //  [rsp + 56]     resume rip   (label 2 below)
        "sub rsp, 64",
        "mov [rsp + 0], rbx",
        "mov [rsp + 8], rbp",
        "mov [rsp + 16], r12",
        "mov [rsp + 24], r13",
        "mov [rsp + 32], r14",
        "mov [rsp + 40], r15",
        "lea rax, [rsp + 64]",
        "mov [rsp + 48], rax",
        "lea rax, [rip + 2f]",
        "mov [rsp + 56], rax",

        // ----- Phase 2: install_continuation(&saved) -----
        // Save user-state args across the call (RDI=state, RSI=cs, RDX=ss).
        "push rdi",
        "push rsi",
        "push rdx",
        "lea rdi, [rsp + 24]",      // ptr to saved continuation (above 3 pushes)
        "call {install_continuation}",
        "pop rdx",
        "pop rsi",
        "pop rdi",

        // ----- Phase 3: build IRETQ frame from UserState -----
        // CPU expects (low → high after pushes): RIP, CS, RFLAGS, RSP, SS.
        // Push reverse: SS, RSP, RFLAGS, CS, RIP.
        // UserState offsets:
        //   72: rsp, 112: rip, 120: rflags
        "push rdx",                  // SS
        "mov rax, [rdi + 72]",
        "push rax",                  // user RSP
        "mov rax, [rdi + 120]",
        "push rax",                  // RFLAGS
        "push rsi",                  // CS
        "mov rax, [rdi + 112]",
        "push rax",                  // RIP

        // ----- Phase 4: load user GP regs from UserState -----
        // Use r11 as the live state pointer through the loads (r11
        // is caller-saved in System V; the iretq clobbers nothing
        // except CS/SS/RIP/RSP/RFLAGS via the pushed frame, so r11
        // here doesn't conflict).
        // UserState offsets:
        //   0: rax, 8: rdi, 16: rsi, 24: rdx, 32: r10, 40: r8, 48: r9,
        //  56: rbx, 64: rbp, 80: r12, 88: r13, 96: r14, 104: r15.
        "mov r11, rdi",              // r11 = state ptr
        "mov rax, [r11 + 0]",
        "mov rbx, [r11 + 56]",
        "mov rbp, [r11 + 64]",
        "mov r12, [r11 + 80]",
        "mov r13, [r11 + 88]",
        "mov r14, [r11 + 96]",
        "mov r15, [r11 + 104]",
        "mov r10, [r11 + 32]",
        "mov r8,  [r11 + 40]",
        "mov r9,  [r11 + 48]",
        "mov rdx, [r11 + 24]",
        "mov rsi, [r11 + 16]",
        "mov rdi, [r11 + 8]",         // load rdi LAST — we used it as the state ptr
        // r11 itself is now stale (it held state ptr); user code
        // doesn't expect any specific value in r11 across SYSCALL,
        // and IRETQ doesn't touch it. Zero it for cleanliness.
        "xor r11, r11",
        // rcx similarly — clobbered by SYSCALL convention, zero it.
        "xor rcx, rcx",

        "iretq",

        // ----- Resume label -----
        // Restore-continuation lands here after the child exits.
        // RSP has already been restored to rsp_on_resume by the asm
        // jump in `restore_continuation`; just `ret` back to the caller.
        "2:",
        "ret",

        install_continuation = sym install_continuation_thunk,
    );
}

/// Drop the active `UserImage`, if any. Called by the run command after
/// `enter_user_mode` returns. Separated out so tests can inspect the
/// active-user state before the image is released.
///
/// Under the U1 process-table refactor, this removes the current entry
/// from `PROCESS_TABLE.by_pid` entirely rather than zeroing it in place
/// — the next `with_current_process` from a kernel-only path then sees
/// the sentinel naturally because `current_user_pid` is `None`.
pub fn release_active_image() -> (Option<UserImage>, Option<crate::userland::address_space::AddressSpace>) {
    let pair = match crate::userland::lifecycle::current_user_pid() {
        Some(pid) if pid != crate::userland::lifecycle::KERNEL_PID => {
            let mut process = crate::userland::lifecycle::remove_process(pid)
                .expect("current_user_pid set but entry missing from process table");
            let img = process.image.take();
            let aspace = process.address_space.take();
            // Heap-backed fields (fd_table, cwd, kernel_stack, signal_state)
            // are released when `process` drops at the end of this block.
            (img, aspace)
        }
        Some(_) => {
            // current_user_pid points at the sentinel (PID 0) — this
            // happens when test helpers re-install the sentinel via
            // `swap_current_process` after a synthetic fork dance. Just
            // clear current_user_pid; the sentinel itself is reset below.
            crate::userland::lifecycle::set_current_user_pid(None);
            (None, None)
        }
        None => (None, None),
    };
    // Reset the sentinel's fields. Some test helpers write to the
    // sentinel (via `with_active_user`) while no real process is
    // loaded, then expect a subsequent `reset_active_user` to clean it
    // up — the pre-PR-C singleton design did this implicitly by
    // resetting fields in place; the table design needs the explicit
    // reset since the sentinel persists across releases.
    crate::userland::lifecycle::reset_sentinel();
    crate::userland::stdin::clear();
    // Phase 5 PR-C1: revert the per-CPU rsp0 pointer to the global
    // boot stack so any kernel-side activity until the next user
    // launch lands on a known-valid buffer.
    let global_top = crate::arch::x86_64::gdt::kernel_rsp0_top();
    unsafe {
        crate::arch::x86_64::gdt::set_kernel_rsp0(global_top);
        crate::arch::x86_64::syscall::set_percpu_kernel_rsp_top(global_top.as_u64());
    }
    pair
}

/// Force-clear continuation/exit state without touching the image. Test-only
/// — used by the U7 test driver to recover after a synthetic `enter_user_mode`.
///
/// Under the U1 process-table refactor: removes the current entry from
/// the table (if any) so the next `with_current_process` observes the
/// sentinel. Equivalent to dropping `release_active_image`'s outputs
/// and clearing pointer-validation bounds + stdin.
#[cfg(feature = "test")]
pub fn force_clear_active_for_test() {
    match crate::userland::lifecycle::current_user_pid() {
        Some(pid) if pid != crate::userland::lifecycle::KERNEL_PID => {
            drop(crate::userland::lifecycle::remove_process(pid));
        }
        Some(_) => {
            crate::userland::lifecycle::set_current_user_pid(None);
        }
        None => {}
    }
    crate::userland::lifecycle::reset_sentinel();
    crate::userland::abi::clear_user_va_bounds();
    crate::userland::stdin::clear();
}

// Suppress unused-import warning when only some entries are needed.
#[allow(dead_code)]
fn _va_addr_silencer(_v: VirtAddr) {}

/// Test-only hooks that bridge the kernel-driven `enter_user_mode_with`
/// path with state tests want to inject just before iretq.
#[cfg(feature = "test")]
pub mod test_hooks {
    use crate::userland::signal::SigAction;
    use spin::Mutex;

    static PRE_IRETQ_SIGNAL_ACTION: Mutex<Option<(i32, SigAction)>> = Mutex::new(None);

    /// Tests call this before `enter_user_mode_with`/`enter_user_mode_with_aspace`;
    /// the action is installed on the new Process slot's signal table
    /// just before the iretq into ring 3.
    pub fn set_pre_iretq_signal_action(sig: i32, action: SigAction) {
        *PRE_IRETQ_SIGNAL_ACTION.lock() = Some((sig, action));
    }

    /// Consumed by `enter_user_mode_with` after it installs the
    /// process slot. One-shot — the slot is cleared after a take.
    pub fn take_pre_iretq_signal_action() -> Option<(i32, SigAction)> {
        PRE_IRETQ_SIGNAL_ACTION.lock().take()
    }
}

// Userland subsystem: ring-3 ELF apps loaded from /host.
//
// See `docs/plans/2026-05-08-004-feat-userland-app-platform-plan.md` for the
// design. This subsystem is built up across implementation units U1..U8.

pub mod abi;
pub mod address_space;
pub mod bin_namespace;
pub mod error;
pub mod etc;
pub mod fdtable;
pub mod image;
pub mod kernel_stack;
pub mod launcher;
pub mod lifecycle;
pub mod loader;
pub mod network_syscalls;
pub mod path;
pub mod pipe;
pub mod signal;
pub mod stdin;
pub mod switch;
pub mod syscalls;
pub mod tty;
pub mod user_state;
pub mod usercopy;
pub mod vm;

use core::arch::naked_asm;

use crate::userland::image::UserImage;
use crate::userland::lifecycle::ExitKind;

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
    0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42,
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
    #[expect(dead_code, reason = "intentional kernel API surface")]
    AlreadyActive,
    /// The loader-produced mappings could not be represented as VMAs.
    InvalidVmLayout,
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
#[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
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
    // Combined entry point used by tests + the synchronous-launch path
    // (`launch_user_binary`). Production launchers can call
    // `setup_user_process` + `wait_for_ring3_exit` directly to release
    // the binary-setup mutex before blocking.
    let new_pid = setup_user_process(image, argv, envp, address_space)?;
    Ok(wait_for_ring3_exit(new_pid))
}

/// U8 setup phase. Installs the Process, populates its
/// `saved_user_state` with the binary's entry frame, marks it
/// `ring3_ready`, and returns the new PID. Does NOT block.
///
/// **Caller responsibility (concurrency):** `address_space` must
/// currently be active on this CPU (CR3 = its L4). The caller must
/// guarantee that no other thread has activated a different L4
/// between its `aspace.activate()` and this call, because
/// `build_initial_stack` writes to user pages of `address_space`
/// through CR3. `launch_user_binary` enforces this via
/// `BINARY_SETUP_MUTEX`.
pub fn setup_user_process(
    mut image: UserImage,
    argv: &[&str],
    envp: &[&str],
    mut address_space: Option<crate::userland::address_space::AddressSpace>,
) -> Result<u32, EnterError> {
    // This is the single authoritative VMA initialization point for every
    // entry path. Building it exactly once avoids cloning and then dropping
    // every ELF-backed Arc during the CR3-sensitive setup transaction.
    if let Some(space) = address_space.as_mut() {
        space
            .initialize_vmas_from_image(&image)
            .map_err(|_| EnterError::InvalidVmLayout)?;
        image.transfer_mapping_ownership();
    }
    // Capture the bits the initial-stack builder needs before the image
    // moves into the new Process slot.
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
    // of the user stack pages. CR3 is already pointing at the new
    // process's L4 (the caller activated it before calling us), so
    // these writes land in the right address space.
    let user_rsp = build_initial_stack(stack_top, &phdr_bytes, e_phnum, argv_slice, envp);

    // Install the new Process. U8: don't make it current — just insert
    // and mark ready. The scheduler picks it up.
    let brk_base = image.brk_base;
    let new_pid = crate::userland::lifecycle::install_new_process_opt(
        image,
        brk_base,
        crate::mm::paging::USER_MMAP_BASE,
        address_space,
    );

    // U8: populate `saved_user_state` with the entry frame so the
    // first `resume_ring3` lands at the binary's entry point with a
    // clean register file (all GPRs zero, user RSP pointing at the
    // freshly-built argc/argv/envp/auxv frame).
    //
    // Also: inherit the launching kernel thread's `terminal_id` so
    // this ring-3 process's stdout/stderr route to the right terminal
    // window (the bugfix to the multi-terminal write-routing race).
    // Read from SCHEDULER.current()'s PCB.
    let launcher_terminal_id = {
        let sched = crate::process::scheduler::SCHEDULER.lock();
        sched
            .current()
            .and_then(|pid| sched.get_process(pid))
            .and_then(|pcb| pcb.terminal_id)
    };
    let exe_path = alloc::string::String::from(argv_slice[0]);
    crate::userland::lifecycle::with_process(new_pid, |p| {
        p.exe_path = Some(exe_path);
        p.terminal_id = launcher_terminal_id;
        p.saved_user_state = crate::userland::user_state::UserState {
            rip: entry,
            rsp: user_rsp,
            // Reserved bit 1 set, IF set, IOPL=0, TF/NT/RF clear.
            rflags: 0x202,
            ..Default::default()
        };
    });

    // U8: `install_new_process_opt` still sets `current_user_pid =
    // new_pid` (single-app-era invariant). Clear it so the first
    // `resume_ring3` from the kernel main loop atomically sets CR3 +
    // current_user_pid for us.
    if crate::userland::lifecycle::current_user_pid() == Some(new_pid) {
        crate::userland::lifecycle::set_current_user_pid(None);
    }

    // Phase 5 PR-B2 test hook: lets tests pre-install a signal action
    // before the user process starts running. Production launches
    // never set this; release builds compile it out.
    #[cfg(feature = "test")]
    {
        if let Some((sig, action)) = test_hooks::take_pre_iretq_signal_action() {
            crate::userland::lifecycle::with_process(new_pid, |p| {
                p.signal_state.set_action(sig, action);
            });
        }
    }
    crate::userland::abi::set_user_va_bounds(bounds);
    crate::userland::stdin::install();
    crate::userland::tty::install_default();

    crate::userland::lifecycle::mark_ring3_ready(new_pid);

    Ok(new_pid)
}

/// U8 wait phase. Blocks the calling kernel thread until ring-3
/// process `pid` exits, then reads its exit info and returns. Does
/// NOT remove the Process from PROCESS_TABLE — the caller's cleanup
/// (`release_active_image`) is the canonical drop site, and tests
/// rely on inspecting post-exit state via `with_current_process`.
///
/// Safe to call without any mutex — the kernel-thread block path
/// touches no user-VA state.
pub fn wait_for_ring3_exit(pid: u32) -> (ExitKind, i64) {
    crate::process::block_kernel_thread_for_ring3_exit(pid);

    // Restore `current_user_pid` to point at the just-exited process so
    // `release_active_image` (looks up by `current_user_pid`) finds
    // and removes it.
    crate::userland::lifecycle::set_current_user_pid(Some(pid));
    crate::userland::lifecycle::with_process(pid, |p| (p.exit_kind, p.exit_code))
        .unwrap_or((ExitKind::None, 0))
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
    let unaligned_top: u64 = argc_size
        + argv_array_size
        + envp_array_size
        + auxv_size
        + phdr_size
        + random_size
        + strings_size;
    let head_pad: u64 = if unaligned_top % 16 == 0 { 0 } else { 8 };
    let frame_size: u64 = unaligned_top + head_pad;
    debug_assert!(frame_size % 16 == 0, "frame_size must be 16-aligned");

    let frame_base: u64 = stack_top - frame_size;

    // Compute addresses (low to high).
    let mut p = frame_base;
    let argc_at = p;
    p += argc_size;
    let argv_at = p;
    p += argv_array_size;
    let envp_at = p;
    p += envp_array_size;
    let _pad_at = p;
    p += head_pad;
    let auxv_at = p;
    p += auxv_size;
    let phdr_at = p;
    p += phdr_size;
    let random_at = p;
    p += random_size;
    let strings_at = p;
    p += strings_size;
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
        write_u64_at(a, AT_PHDR);
        a += 8;
        write_u64_at(a, phdr_at);
        a += 8;
        write_u64_at(a, AT_PHENT);
        a += 8;
        write_u64_at(a, ELF64_PHDR_SIZE);
        a += 8;
        write_u64_at(a, AT_PHNUM);
        a += 8;
        write_u64_at(a, e_phnum as u64);
        a += 8;
        write_u64_at(a, AT_PAGESZ);
        a += 8;
        write_u64_at(a, 0x1000);
        a += 8;
        write_u64_at(a, AT_RANDOM);
        a += 8;
        write_u64_at(a, random_at);
        a += 8;
        write_u64_at(a, AT_NULL);
        a += 8;
        write_u64_at(a, 0);
        let _ = a;

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
    _user_cs: u64,                                         // RSI
    _user_ss: u64,                                         // RDX
) -> ! {
    naked_asm!(
        // Build IRETQ frame (low → high after pushes: RIP, CS, RFLAGS, RSP, SS).
        "push rdx", // SS
        "mov rax, [rdi + 72]",
        "push rax", // user RSP
        "mov rax, [rdi + 120]",
        "push rax", // RFLAGS
        "push rsi", // CS
        "mov rax, [rdi + 112]",
        "push rax", // RIP
        // Load user GP regs from UserState.
        "mov r11, rdi", // r11 = state ptr
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
        "mov rdi, [r11 + 8]", // load rdi LAST (we used it as state ptr)
        // SYSCALL ABI clobbers rcx/r11; zero them for cleanliness so a
        // ring-3 program that incorrectly assumes them preserved gets
        // a deterministic value rather than kernel data.
        "xor r11, r11",
        "xor rcx, rcx",
        "iretq",
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
pub fn release_active_image() -> (
    Option<UserImage>,
    Option<crate::userland::address_space::AddressSpace>,
) {
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

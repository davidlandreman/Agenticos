// MSR helpers for the SYSCALL/SYSRET fast-path setup and `arch_prctl`.
//
// One place for "what does this kernel write to which MSR for the SYSCALL
// transition" — boot-time programming (EFER.SCE, STAR, LSTAR, FMASK,
// IA32_GS_BASE, IA32_KERNEL_GS_BASE) and the runtime
// `arch_prctl(ARCH_SET_FS)` syscall handler.
//
// The bulk of the work is `x86_64::registers::model_specific`'s typed
// wrappers; this module exposes the small set of operations the SYSCALL
// path actually performs so callers don't repeat the same `unsafe` boilerplate.
//
// Single-CPU only. SMP support would re-run `program_syscall_msrs` and
// `init_gs_base` per AP at bringup; the per-CPU struct address would also
// have to come from a per-AP allocation rather than a single `static mut`.

use x86_64::VirtAddr;
use x86_64::registers::model_specific::{
    Efer, EferFlags, FsBase, GsBase, KernelGsBase, LStar, SFMask, Star,
};
use x86_64::registers::rflags::RFlags;
use x86_64::structures::gdt::SegmentSelector;

/// Enable `EFER.SCE` so the `syscall` instruction does not raise `#UD`.
///
/// Must run on the executing CPU before any user code can issue `syscall`.
pub fn enable_syscall_extensions() {
    unsafe {
        Efer::update(|flags| flags.insert(EferFlags::SYSTEM_CALL_EXTENSIONS));
    }
}

/// Program `IA32_STAR`, `IA32_LSTAR`, and `IA32_FMASK`.
///
/// `lstar` is the kernel-side entry point — the naked-asm SYSCALL stub. The
/// selectors come from `gdt::selectors()` and must satisfy the SYSRET
/// invariant baked into the GDT layout: user data immediately precedes user
/// code (slots 3 and 4, selectors 0x18/0x20) so that
/// `user_cs = STAR[63:48] + 16` and `user_ss = STAR[63:48] + 8` resolve
/// correctly. The wrapper validates this and returns `Err` if violated; we
/// surface that as a panic because it's a setup-time invariant, not a
/// runtime condition.
///
/// `IA32_FMASK` clears `IF` (interrupts must stay off until the stub
/// finishes the stack switch), `DF` (System V requires cleared DF on syscall
/// entry), and `AC` (alignment check would fault on the kernel stack
/// otherwise). Linux additionally clears `TF`, `IOPL`, `NT`; those are
/// safe to add later if needed but not load-bearing for this milestone.
pub fn program_syscall_msrs(
    cs_kernel: SegmentSelector,
    ss_kernel: SegmentSelector,
    cs_user: SegmentSelector,
    ss_user: SegmentSelector,
    lstar: VirtAddr,
) {
    Star::write(cs_user, ss_user, cs_kernel, ss_kernel)
        .expect("STAR selectors violate SYSRET +8/+16 invariant");
    LStar::write(lstar);
    SFMask::write(RFlags::INTERRUPT_FLAG | RFlags::DIRECTION_FLAG | RFlags::ALIGNMENT_CHECK);
}

/// Set `IA32_GS_BASE` and `IA32_KERNEL_GS_BASE` to the same per-CPU pointer.
///
/// At boot we point both at the kernel's PerCpu struct so the first
/// `swapgs` on SYSCALL entry leaves us in a known state regardless of which
/// MSR currently mirrors which value. Userland code that issues `wrgsbase`
/// (or, more realistically, the kernel's own `arch_prctl` for FS_BASE)
/// changes the user-visible GS at runtime; the kernel-side base persists in
/// `IA32_KERNEL_GS_BASE` and is restored by `swapgs` on the next entry.
pub fn init_gs_base(percpu_addr: u64) {
    let addr = VirtAddr::new(percpu_addr);
    GsBase::write(addr);
    KernelGsBase::write(addr);
}

/// Set `IA32_FS_BASE` — the implementation of `arch_prctl(ARCH_SET_FS, addr)`.
///
/// Writing FS_BASE via `wrmsr` is the fast path; the alternative (loading a
/// GDT descriptor) would require allocating a per-process descriptor and is
/// unnecessary on x86-64 where the FS_BASE MSR exists explicitly for this
/// pattern. musl's `__init_tls` issues this syscall before any TLS-using
/// code runs.
pub fn set_fs_base(addr: u64) {
    FsBase::write(VirtAddr::new(addr));
}

/// Read `IA32_FS_BASE` — used by `fork_handler` to snapshot the
/// parent's FS_BASE before the child runs, since musl in the child
/// (especially after execve) reinstalls its own FS_BASE pointing at
/// the child's musl-allocated TCB. Without a save/restore around the
/// child's run, the parent resumes with FS_BASE still aimed at an
/// address that exists in parent's L4 but contains unrelated data —
/// `%fs:0` returns garbage and the next `__errno_location`-style
/// access faults.
pub fn read_fs_base() -> u64 {
    FsBase::read().as_u64()
}

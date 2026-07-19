//! Fixed architectural snapshot used by the crash owner.

#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct RegisterSnapshot {
    pub rip: u64,
    pub rsp: u64,
    pub rbp: u64,
    pub rflags: u64,
    pub cr0: u64,
    pub cr2: u64,
    pub cr3: u64,
    pub cr4: u64,
    pub fs_base: u64,
    pub gs_base: u64,
    pub current_pid: u64,
    pub fidelity: u8,
    pub _reserved: [u8; 7],
}

pub const FIDELITY_HANDLER_LIVE: u8 = 3;
pub const FIDELITY_CPU_PUSHED: u8 = 2;

pub fn capture_live(rip: u64, fidelity: u8) -> RegisterSnapshot {
    let rsp: u64;
    let rbp: u64;
    let rflags: u64;
    let cr0: u64;
    let cr2: u64;
    let cr3: u64;
    let cr4: u64;
    unsafe {
        core::arch::asm!(
            "mov {rsp}, rsp",
            "mov {rbp}, rbp",
            "pushfq",
            "pop {rflags}",
            "mov {cr0}, cr0",
            "mov {cr2}, cr2",
            "mov {cr3}, cr3",
            "mov {cr4}, cr4",
            rsp = out(reg) rsp,
            rbp = out(reg) rbp,
            rflags = out(reg) rflags,
            cr0 = out(reg) cr0,
            cr2 = out(reg) cr2,
            cr3 = out(reg) cr3,
            cr4 = out(reg) cr4,
            options(nomem)
        );
    }
    RegisterSnapshot {
        rip,
        rsp,
        rbp,
        rflags,
        cr0,
        cr2,
        cr3,
        cr4,
        fs_base: crate::arch::x86_64::msr::read_fs_base(),
        gs_base: 0,
        current_pid: crate::arch::x86_64::percpu::current_user_pid()
            .map(u64::from)
            .unwrap_or(0),
        fidelity,
        _reserved: [0; 7],
    }
}

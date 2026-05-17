//! GLAUNCH.ELF — kernel-side GUI app launcher (multicall).
//!
//! One static ring-3 ELF that reads `argv[0]`, issues the AgenticOS
//! `gui_launch(name)` syscall, and exits. The kernel-side
//! `/bin/<gui_applet>` rewrite (`src/userland/bin_namespace.rs`) sends
//! `execve("/bin/painting", ["painting"], envp)` here, so the user
//! types `painting` in zsh and the kernel-side `PaintingProcess` gets
//! spawned via the syscall. See
//! `docs/plans/2026-05-16-004-feat-zsh-default-terminal-and-gui-launchers-plan.md`.

#![no_std]
#![no_main]

use runtime::{argv0_from_stack, exit, gui_launch};

#[no_mangle]
pub unsafe extern "C" fn _start() -> ! {
    // Pull the initial RSP — argc + argv layout lives at RSP. The
    // linker script sets ENTRY(_start) and the kernel ELF loader doesn't
    // touch the stack between iretq and our first instruction.
    let stack_top: *const u64;
    core::arch::asm!(
        "mov {0}, rsp",
        out(reg) stack_top,
        options(nomem, nostack, preserves_flags),
    );

    let (name_ptr, name_len) = argv0_from_stack(stack_top);
    if name_ptr.is_null() || name_len == 0 {
        // No argv[0] — the kernel always provides one for /bin/<applet>
        // rewrites, so this only happens if GUILAUNCH is launched
        // directly with a malformed argv. Exit non-zero so the failure
        // is visible.
        exit(2);
    }

    let rc = gui_launch(name_ptr, name_len);
    if rc == 0 {
        exit(0);
    }
    // rc is a negative errno; squash to a small positive exit code so
    // the shell sees a sensible value.
    exit(1);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe { exit(1) }
}

//! Assembly-level context switching for x86-64
//!
//! This module provides the low-level context switch function that saves
//! the current process's CPU state and restores another process's state.

use core::arch::naked_asm;
use crate::process::context::CpuContext;

/// Perform a context switch from one process to another.
///
/// This function saves the current CPU context to `old_ctx` and loads
/// the context from `new_ctx`. When this function "returns", it will
/// be returning in the context of the new process.
///
/// # Safety
///
/// This function is unsafe because:
/// - It modifies CPU registers directly
/// - The caller must ensure both contexts are valid
/// - Interrupts should be disabled during the switch
///
/// # Arguments
/// * `old_ctx` - Pointer to save the current context
/// * `new_ctx` - Pointer to the context to switch to
#[unsafe(naked)]
pub unsafe extern "C" fn switch_context(
    old_ctx: *mut CpuContext,
    new_ctx: *const CpuContext,
) {
    // The CpuContext layout (offsets in bytes):
    // 0:  rbx
    // 8:  rbp
    // 16: r12
    // 24: r13
    // 32: r14
    // 40: r15
    // 48: rsp
    // 56: rip
    // 64: rflags

    naked_asm!(
        // Save callee-saved registers to old context (rdi = old_ctx)
        "mov [rdi + 0], rbx",
        "mov [rdi + 8], rbp",
        "mov [rdi + 16], r12",
        "mov [rdi + 24], r13",
        "mov [rdi + 32], r14",
        "mov [rdi + 40], r15",

        // Save stack pointer (adjust for the return address on stack)
        // We save RSP+8 because when we restore and JMP (not RET),
        // we want RSP to be where it was before the CALL to switch_context
        "lea rax, [rsp + 8]",
        "mov [rdi + 48], rax",

        // Save return address as RIP (address after this function returns)
        // The return address is at [rsp] since we were just called
        "mov rax, [rsp]",
        "mov [rdi + 56], rax",

        // Save flags
        "pushfq",
        "pop rax",
        "mov [rdi + 64], rax",

        // Load callee-saved registers from new context (rsi = new_ctx)
        "mov rbx, [rsi + 0]",
        "mov rbp, [rsi + 8]",
        "mov r12, [rsi + 16]",
        "mov r13, [rsi + 24]",
        "mov r14, [rsi + 32]",
        "mov r15, [rsi + 40]",

        // Load flags
        "mov rax, [rsi + 64]",
        "push rax",
        "popfq",

        // Load stack pointer
        "mov rsp, [rsi + 48]",

        // Jump to new RIP
        "jmp [rsi + 56]",
    );
}

/// Switch to a new process context without saving the old one.
///
/// Used when starting the first process or when the current process
/// has terminated and there's nothing to save.
///
/// # Safety
///
/// Same safety requirements as `switch_context`.
#[unsafe(naked)]
pub unsafe extern "C" fn switch_to_context(new_ctx: *const CpuContext) {
    naked_asm!(
        // Load callee-saved registers from new context (rdi = new_ctx)
        "mov rbx, [rdi + 0]",
        "mov rbp, [rdi + 8]",
        "mov r12, [rdi + 16]",
        "mov r13, [rdi + 24]",
        "mov r14, [rdi + 32]",
        "mov r15, [rdi + 40]",

        // Load flags
        "mov rax, [rdi + 64]",
        "push rax",
        "popfq",

        // Load stack pointer
        "mov rsp, [rdi + 48]",

        // Jump to RIP
        "jmp [rdi + 56]",
    );
}

/// Context switch that saves callee-saved regs but restores ALL regs.
///
/// Used when switching from kernel to a preempted process:
/// - Kernel only needs callee-saved registers saved (it's a normal function call)
/// - Process needs ALL registers restored (it was interrupted mid-execution)
///
/// # Safety
///
/// Same safety requirements as `switch_context`.
#[unsafe(naked)]
pub unsafe extern "C" fn switch_context_full_restore(
    old_ctx: *mut CpuContext,
    new_ctx: *const CpuContext,
) {
    // The CpuContext layout (offsets in bytes):
    // 0:  rbx, 8: rbp, 16: r12, 24: r13, 32: r14, 40: r15
    // 48: rsp, 56: rip, 64: rflags
    // 72: rax, 80: rcx, 88: rdx, 96: rsi, 104: rdi
    // 112: r8, 120: r9, 128: r10, 136: r11

    naked_asm!(
        // ===== SAVE callee-saved registers to old context (rdi = old_ctx) =====
        "mov [rdi + 0], rbx",
        "mov [rdi + 8], rbp",
        "mov [rdi + 16], r12",
        "mov [rdi + 24], r13",
        "mov [rdi + 32], r14",
        "mov [rdi + 40], r15",

        // Save stack pointer (adjust for the return address on stack)
        "lea rax, [rsp + 8]",
        "mov [rdi + 48], rax",

        // Save return address as RIP
        "mov rax, [rsp]",
        "mov [rdi + 56], rax",

        // Save flags
        "pushfq",
        "pop rax",
        "mov [rdi + 64], rax",

        // ===== RESTORE ALL registers from new context (rsi = new_ctx) =====
        // Set up iretq frame on CURRENT stack
        "push 0x10",             // SS (kernel data segment)
        "push qword ptr [rsi + 48]",  // RSP
        "push qword ptr [rsi + 64]",  // RFLAGS
        "push 0x08",             // CS (kernel code segment)
        "push qword ptr [rsi + 56]",  // RIP

        // Restore all registers from context
        "mov rbx, [rsi + 0]",
        "mov rbp, [rsi + 8]",
        "mov r12, [rsi + 16]",
        "mov r13, [rsi + 24]",
        "mov r14, [rsi + 32]",
        "mov r15, [rsi + 40]",

        "mov rax, [rsi + 72]",
        "mov rcx, [rsi + 80]",
        "mov rdx, [rsi + 88]",
        "mov rdi, [rsi + 104]",  // Restore rdi before rsi
        "mov r8, [rsi + 112]",
        "mov r9, [rsi + 120]",
        "mov r10, [rsi + 128]",
        "mov r11, [rsi + 136]",
        "mov rsi, [rsi + 96]",   // Restore rsi last

        // Return via iretq
        "iretq",
    );
}

/// Switch to a process context restoring ALL registers (via iretq).
///
/// Used when resuming a process that was preempted by a timer interrupt.
/// Sets up an iretq frame on the CURRENT stack, restores registers, then
/// uses iretq to atomically load RIP, RSP, and RFLAGS.
///
/// # Safety
///
/// Same safety requirements as `switch_context`.
#[unsafe(naked)]
pub unsafe extern "C" fn switch_to_full_context_iretq(new_ctx: *const CpuContext) {
    naked_asm!(
        // rdi = new_ctx pointer
        // CpuContext layout:
        // 0:  rbx, 8: rbp, 16: r12, 24: r13, 32: r14, 40: r15
        // 48: rsp, 56: rip, 64: rflags
        // 72: rax, 80: rcx, 88: rdx, 96: rsi, 104: rdi
        // 112: r8, 120: r9, 128: r10, 136: r11

        // Set up iretq frame on CURRENT stack (don't corrupt target stack)
        // iretq frame: RIP, CS, RFLAGS, RSP, SS (from low to high address)
        "push 0x10",             // SS (kernel data segment)
        "push qword ptr [rdi + 48]",  // RSP
        "push qword ptr [rdi + 64]",  // RFLAGS
        "push 0x08",             // CS (kernel code segment)
        "push qword ptr [rdi + 56]",  // RIP

        // Now restore all registers from context
        // (rdi still points to context)
        "mov rbx, [rdi + 0]",
        "mov rbp, [rdi + 8]",
        "mov r12, [rdi + 16]",
        "mov r13, [rdi + 24]",
        "mov r14, [rdi + 32]",
        "mov r15, [rdi + 40]",

        "mov rax, [rdi + 72]",
        "mov rcx, [rdi + 80]",
        "mov rdx, [rdi + 88]",
        "mov rsi, [rdi + 96]",
        // rdi loaded last
        "mov r8, [rdi + 112]",
        "mov r9, [rdi + 120]",
        "mov r10, [rdi + 128]",
        "mov r11, [rdi + 136]",
        "mov rdi, [rdi + 104]",  // Load rdi last

        // Return via iretq - atomically sets RIP, CS, RFLAGS, RSP, SS
        "iretq",
    );
}

/// Wrapper function for entry point of new processes
///
/// This function is called when a new process starts. It retrieves
/// the actual entry function from the PCB and calls it.
#[no_mangle]
pub extern "C" fn process_entry_trampoline() {
    // Re-enable interrupts now that we're safely on the new stack
    unsafe { core::arch::asm!("sti"); }

    // Debug: We reached the trampoline
    crate::debug_info!(">>> process_entry_trampoline: ENTERED");

    // Get the current process and call its entry function
    crate::debug_info!(">>> process_entry_trampoline: about to lock scheduler");
    let mut scheduler = crate::process::scheduler::SCHEDULER.lock();
    crate::debug_info!(">>> process_entry_trampoline: scheduler locked");
    if let Some(pid) = scheduler.current() {
        if let Some(pcb) = scheduler.get_process_mut(pid) {
            if let Some(entry_fn) = pcb.entry_fn.take() {
                // Drop the scheduler lock before calling the entry function
                drop(scheduler);

                // Call the process's entry function
                entry_fn();
            }
        }
    }

    // If we get here, the process has finished - terminate it
    crate::process::terminate_current();
}

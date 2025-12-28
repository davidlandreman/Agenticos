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

        // Save stack pointer
        "mov [rdi + 48], rsp",

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

/// Wrapper function for entry point of new processes
///
/// This function is called when a new process starts. It retrieves
/// the actual entry function from the PCB and calls it.
#[no_mangle]
pub extern "C" fn process_entry_trampoline() {
    // Get the current process and call its entry function
    let mut scheduler = crate::process::scheduler::SCHEDULER.lock();
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

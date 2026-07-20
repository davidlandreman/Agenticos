//! Assembly-level context switching for x86-64
//!
//! This module provides the low-level context switch function that saves
//! the current process's CPU state and restores another process's state.

use crate::process::context::CpuContext;
use core::arch::naked_asm;

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
pub unsafe extern "C" fn switch_context(old_ctx: *mut CpuContext, new_ctx: *const CpuContext) {
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
        "cli",
        // Stage the target image in per-CPU storage while the source stack is
        // still private. `new_ctx` is commonly a Rust local on that stack.
        "mov r13, rdi",
        "mov rdi, rsi",
        "sub rsp, 8",
        "call {stage_handoff}",
        "add rsp, 8",
        "mov r12, rax",

        // Abandon the entity stack before publishing its completed context.
        // The per-CPU idle/main stack is inactive during entity execution.
        "mov rsp, gs:[{kernel_context_rsp_offset}]",
        "and rsp, -16",
        "mov rdi, r13",
        "call {publish_saved_context}",

        // Restore from per-CPU staging and use a synthetic return address for
        // the CPL0 handoff.
        "mov r11, r12",
        "cli",
        "mov rsp, [r11 + 48]",
        "push qword ptr [r11 + 56]",
        "mov rax, [r11 + 64]",
        "and rax, -513",
        "push rax",
        "popfq",
        "mov rbx, [r11 + 0]",
        "mov rbp, [r11 + 8]",
        "mov r12, [r11 + 16]",
        "mov r13, [r11 + 24]",
        "mov r14, [r11 + 32]",
        "mov r15, [r11 + 40]",
        "mov rax, [r11 + 72]",
        "mov rcx, [r11 + 80]",
        "mov rdx, [r11 + 88]",
        "mov rsi, [r11 + 96]",
        "mov rdi, [r11 + 104]",
        "mov r8, [r11 + 112]",
        "mov r9, [r11 + 120]",
        "mov r10, [r11 + 128]",
        "mov r11, [r11 + 136]",
        "sti",
        "ret",
        stage_handoff = sym stage_handoff_context,
        publish_saved_context = sym publish_saved_context,
        kernel_context_rsp_offset = const crate::arch::x86_64::percpu::KERNEL_CONTEXT_RSP_OFFSET,
    );
}

extern "C" fn stage_handoff_context(context: *const CpuContext) -> *const CpuContext {
    unsafe { crate::arch::x86_64::percpu::stage_handoff_context(context) }
}

extern "C" fn publish_saved_context(old_context: *mut CpuContext) {
    let published = publish_handoff_context(old_context);
    debug_assert!(published, "context switch saved an unregistered context");
}

/// Publish a completed kernel-side save when a handoff may originate either
/// from a registered kernel entity or from the per-CPU idle/main context.
/// Returns whether any scheduler entity was identified.
pub(crate) extern "C" fn publish_handoff_context(old_context: *mut CpuContext) -> bool {
    let mut scheduler = crate::process::scheduler::SCHEDULER.lock();
    let pending = crate::arch::x86_64::percpu::take_pending_context_publish();
    if let Some(entity) = pending {
        scheduler.publish_context(entity);
    }
    // A stale or concurrently superseded side-band user publication must not
    // strand a kernel thread whose save completed in the same handoff. The
    // pointer lookup is a no-op for user continuations, so publish both forms
    // when applicable and require at least one to identify the saved context.
    let kernel_published = scheduler.publish_kernel_context_ptr(old_context);
    drop(scheduler);
    if let Some(crate::process::entity::EntityId::UserProcess(pid)) = pending {
        crate::diagnostics::shadow::continuation::published(pid, unsafe { &*old_context });
        crate::diagnostics::shadow::stack::deactivate_owner(pid);
    }
    crate::diagnostics::shadow::cpu::commit_kernel();
    pending.is_some() || kernel_published
}

/// Publish the entity whose interrupt-driven save is complete. The assembly
/// caller has already installed a different stack before entering here.
pub(crate) extern "C" fn publish_pending_context() {
    if let Some(entity) = crate::arch::x86_64::percpu::take_pending_context_publish() {
        crate::process::scheduler::SCHEDULER
            .lock()
            .publish_context(entity);
        if let crate::process::entity::EntityId::UserProcess(pid) = entity {
            crate::diagnostics::shadow::stack::deactivate_owner(pid);
        }
    }
    crate::diagnostics::shadow::stack::complete_abandon();
    crate::diagnostics::shadow::cpu::commit_kernel();
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
        "mov r12, rdi",
        "mov rsp, [r12 + 48]",
        "and rsp, -16",
        "call {complete_stack_abandon}",
        "mov r11, r12",
        "mov rsp, [r11 + 48]",
        "push qword ptr [r11 + 56]",
        // Load flags with IF masked until the register image is complete.
        "mov rax, [r11 + 64]",
        "and rax, -513",
        "push rax",
        "popfq",
        "mov rbx, [r11 + 0]",
        "mov rbp, [r11 + 8]",
        "mov r12, [r11 + 16]",
        "mov r13, [r11 + 24]",
        "mov r14, [r11 + 32]",
        "mov r15, [r11 + 40]",
        "mov rax, [r11 + 72]",
        "mov rcx, [r11 + 80]",
        "mov rdx, [r11 + 88]",
        "mov rsi, [r11 + 96]",
        "mov rdi, [r11 + 104]",
        "mov r8, [r11 + 112]",
        "mov r9, [r11 + 120]",
        "mov r10, [r11 + 128]",
        "mov r11, [r11 + 136]",
        "sti",
        "ret",
        complete_stack_abandon = sym complete_stack_abandon,
    );
}

extern "C" fn complete_stack_abandon() {
    // `switch_to_context` is also the empty-run-queue path for a ring-3
    // syscall that just blocked. Its continuation cannot be published until
    // RSP has moved off the process's live kernel stack, which is true here.
    // Without this publication, a later pipe/readiness wake can mark the
    // entity Ready but cannot enqueue its still-unpublished context.
    publish_pending_context();
}

/// Abandon a terminated kernel thread, retire its stack from a different
/// stack, and restore the per-CPU kernel/main-loop context.
///
/// The stack allocator is shared across CPUs. Publishing the old stack before
/// changing RSP would let a concurrent spawn reuse memory that still contains
/// this CPU's live termination frames.
#[unsafe(naked)]
pub unsafe extern "C" fn switch_to_context_and_retire_stack(
    new_ctx: *const CpuContext,
    stack_base: u64,
) -> ! {
    naked_asm!(
        // Preserve both arguments across the Rust helper. The target register
        // image is restored below, so borrowing r12/r13 here is harmless.
        "mov r12, rdi",
        "mov r13, rsi",
        // Run the allocator on the target stack. Align down for the SysV call
        // and reload the exact saved RSP afterward.
        "mov rsp, [r12 + 48]",
        "and rsp, -16",
        "mov rdi, r13",
        "call {retire_stack}",
        "mov r11, r12",
        "mov rsp, [r11 + 48]",
        "push qword ptr [r11 + 56]",
        "mov rax, [r11 + 64]",
        "and rax, -513",
        "push rax",
        "popfq",
        "mov rbx, [r11 + 0]",
        "mov rbp, [r11 + 8]",
        "mov r12, [r11 + 16]",
        "mov r13, [r11 + 24]",
        "mov r14, [r11 + 32]",
        "mov r15, [r11 + 40]",
        "mov rax, [r11 + 72]",
        "mov rcx, [r11 + 80]",
        "mov rdx, [r11 + 88]",
        "mov rsi, [r11 + 96]",
        "mov rdi, [r11 + 104]",
        "mov r8, [r11 + 112]",
        "mov r9, [r11 + 120]",
        "mov r10, [r11 + 128]",
        "mov r11, [r11 + 136]",
        "sti",
        "ret",
        retire_stack = sym retire_kernel_stack,
    );
}

extern "C" fn retire_kernel_stack(stack_base: u64) {
    crate::process::stack::free_stack(stack_base);
}

/// Restore a kernel entity from a complete saved context and diverge.
///
/// Unlike `iretq`, this explicitly installs the target kernel stack before
/// transferring between two CPL0 contexts. The target RIP is pushed as a
/// synthetic return address so every GPR, including the scratch register used
/// as the context pointer, can be restored before `ret`.
#[unsafe(naked)]
pub unsafe extern "C" fn restore_kernel_context(new_ctx: *const CpuContext) -> ! {
    naked_asm!(
        // Stage the target image before abandoning the interrupt/source stack.
        "cli",
        "sub rsp, 8",
        "call {stage_handoff}",
        "add rsp, 8",
        "mov r12, rax",

        // Publish only after moving to the per-CPU idle/main stack.
        "mov rsp, gs:[{kernel_context_rsp_offset}]",
        "and rsp, -16",
        "call {publish_pending}",
        "mov r11, r12",
        "mov rsp, [r11 + 48]",
        "mov rax, [r11 + 56]",
        "push rax",
        // Restore all flags except IF while the register image is partial.
        // `sti; ret` below uses the architectural interrupt shadow to make
        // the final transfer atomic with respect to maskable interrupts.
        "mov rax, [r11 + 64]",
        "and rax, -513",
        "push rax",
        "popfq",
        "mov rbx, [r11 + 0]",
        "mov rbp, [r11 + 8]",
        "mov r12, [r11 + 16]",
        "mov r13, [r11 + 24]",
        "mov r14, [r11 + 32]",
        "mov r15, [r11 + 40]",
        "mov rax, [r11 + 72]",
        "mov rcx, [r11 + 80]",
        "mov rdx, [r11 + 88]",
        "mov rsi, [r11 + 96]",
        "mov rdi, [r11 + 104]",
        "mov r8, [r11 + 112]",
        "mov r9, [r11 + 120]",
        "mov r10, [r11 + 128]",
        "mov r11, [r11 + 136]",
        "sti",
        "ret",
        stage_handoff = sym stage_handoff_context,
        publish_pending = sym publish_pending_context,
        kernel_context_rsp_offset = const crate::arch::x86_64::percpu::KERNEL_CONTEXT_RSP_OFFSET,
    );
}

pub unsafe fn resume_kernel_thread(pid: crate::process::ProcessId) -> ! {
    let context = crate::process::scheduler::SCHEDULER
        .lock()
        .get_context(pid)
        .copied()
        .expect("resume_kernel_thread: unknown pid");
    validate_kernel_context(pid, &context);
    crate::diagnostics::shadow::cpu::begin_kernel(Some(
        crate::process::entity::EntityId::KernelThread(pid),
    ));
    crate::userland::lifecycle::set_current_user_pid(None);
    crate::diagnostics::shadow::cpu::clear_current_pid();
    crate::process::set_in_spawned_process(true);
    // Kernel entities never own a user address space. A ring-3 timer/exit may
    // select this thread directly, so restore the permanent kernel CR3 before
    // the old process can be reaped on another CPU.
    crate::mm::paging::activate_kernel_l4();
    let kernel_l4 = crate::mm::paging::kernel_l4_frame()
        .expect("kernel L4 vanished during kernel-thread handoff")
        .start_address()
        .as_u64();
    crate::diagnostics::shadow::cpu::install_kernel_cr3(kernel_l4);
    restore_kernel_context(&context)
}

pub fn validate_kernel_context(pid: crate::process::ProcessId, context: &CpuContext) {
    let stack_start = crate::process::stack::STACK_REGION_START;
    let stack_end = stack_start
        + (crate::process::stack::MAX_PROCESSES * (crate::process::stack::STACK_SIZE + 4096))
            as u64;
    assert!(
        context.rip >= 0xffff_8000_0000_0000
            && context.rsp >= stack_start
            && context.rsp <= stack_end
            && context.cs == 0x08
            && context.ss == 0x10,
        "invalid kernel context for PID {pid}: RIP={:#x} RSP={:#x} CS={:#x} SS={:#x}",
        context.rip,
        context.rsp,
        context.cs,
        context.ss,
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
        // Switch to the target stack before transferring. A same-privilege
        // IRETQ does not install RSP and makes the segment frame needlessly
        // fragile, so use a synthetic return address just like
        // restore_kernel_context.
        "cli",
        "mov r11, rsi",
        "mov rsp, [r11 + 48]",
        "push qword ptr [r11 + 56]", // RIP
        // Keep interrupts masked until the register image is complete. The
        // STI interrupt shadow extends through the final RET.
        "mov rax, [r11 + 64]",
        "and rax, -513",
        "push rax",
        "popfq",
        // Restore all registers from context
        "mov rbx, [r11 + 0]",
        "mov rbp, [r11 + 8]",
        "mov r12, [r11 + 16]",
        "mov r13, [r11 + 24]",
        "mov r14, [r11 + 32]",
        "mov r15, [r11 + 40]",
        "mov rax, [r11 + 72]",
        "mov rcx, [r11 + 80]",
        "mov rdx, [r11 + 88]",
        "mov rsi, [r11 + 96]",
        "mov rdi, [r11 + 104]",
        "mov r8, [r11 + 112]",
        "mov r9, [r11 + 120]",
        "mov r10, [r11 + 128]",
        "mov r11, [r11 + 136]",
        "sti",
        "ret",
    );
}

/// Wrapper function for entry point of new processes
///
/// This function is called when a new process starts. It retrieves
/// the actual entry function from the PCB and calls it.
#[no_mangle]
pub extern "C" fn process_entry_trampoline() {
    // Re-enable interrupts now that we're safely on the new stack
    unsafe {
        core::arch::asm!("sti");
    }

    crate::debug_trace!("process_entry_trampoline: entered");

    // Get the current process and call its entry function
    crate::debug_trace!("process_entry_trampoline: locking scheduler");
    let mut scheduler = crate::process::scheduler::SCHEDULER.lock();
    crate::debug_trace!("process_entry_trampoline: scheduler locked");
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

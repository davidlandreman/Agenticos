//! Timer-based preemption support
//!
//! This module implements the low-level timer interrupt handler that supports
//! true preemptive multitasking. When a timer interrupt fires during process
//! execution, this handler saves the full CPU state and can switch to another
//! process without any cooperation from the running process.

use core::arch::naked_asm;
use crate::process::context::CpuContext;

/// Saved context pointer for the current process during interrupt handling
/// This is set by the timer interrupt handler before calling the scheduler
#[no_mangle]
pub static mut CURRENT_CONTEXT_PTR: *mut CpuContext = core::ptr::null_mut();

/// Context to switch to (set by scheduler when preemption is needed)
#[no_mangle]
pub static mut NEXT_CONTEXT_PTR: *const CpuContext = core::ptr::null();

/// Flag indicating a context switch should occur on interrupt return
#[no_mangle]
pub static mut DO_CONTEXT_SWITCH: bool = false;

/// Flag indicating we should switch back to kernel (use jump, not iretq)
#[no_mangle]
pub static mut SWITCH_TO_KERNEL: bool = false;

/// Kernel context to return to (set by try_run_scheduled_processes before switching to a process)
#[no_mangle]
pub static mut KERNEL_CONTEXT: CpuContext = CpuContext::new();

/// The actual timer interrupt handler with preemption support.
///
/// This is a naked function that:
/// 1. Saves all registers to the stack
/// 2. Calls the Rust handler to check for preemption
/// 3. Either restores registers and returns normally, OR
/// 4. Switches to a different process context
///
/// The interrupt frame pushed by CPU is: SS, RSP, RFLAGS, CS, RIP (from high to low addr)
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn timer_interrupt_handler_preemptive() {
    naked_asm!(
        // Save all general purpose registers
        // The CPU already pushed SS, RSP, RFLAGS, CS, RIP
        "push rax",
        "push rbx",
        "push rcx",
        "push rdx",
        "push rsi",
        "push rdi",
        "push rbp",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        "push r12",
        "push r13",
        "push r14",
        "push r15",

        // Pass RSP as argument to the Rust handler (points to saved registers)
        "mov rdi, rsp",

        // Call the Rust handler
        "call {timer_handler_inner}",

        // Check if we need to switch context (use RIP-relative addressing)
        "lea rax, [rip + {do_switch}]",
        "mov al, [rax]",
        "test al, al",
        "jnz 3f",  // Jump to context switch code

        // Normal return - restore all registers
        "2:",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop r11",
        "pop r10",
        "pop r9",
        "pop r8",
        "pop rbp",
        "pop rdi",
        "pop rsi",
        "pop rdx",
        "pop rcx",
        "pop rbx",
        "pop rax",
        "iretq",

        // Context switch path
        "3:",
        // Clear the switch flag (use RIP-relative)
        "lea rax, [rip + {do_switch}]",
        "mov byte ptr [rax], 0",

        // Check if switching to kernel (simple jump) or process (iretq)
        "lea rax, [rip + {to_kernel}]",
        "mov al, [rax]",
        "test al, al",
        "jnz 4f",  // Jump to kernel return path

        // ========== Switch to PROCESS via iretq ==========
        // Load the new context pointer (use RIP-relative)
        "lea rax, [rip + {next_ctx}]",
        "mov rsi, [rax]",

        // Restore all registers from new context
        // CpuContext layout: rbx(0), rbp(8), r12(16), r13(24), r14(32), r15(40),
        //                    rsp(48), rip(56), rflags(64), rax(72), rcx(80), rdx(88),
        //                    rsi(96), rdi(104), r8(112), r9(120), r10(128), r11(136)

        "mov rbx, [rsi + 0]",
        "mov rbp, [rsi + 8]",
        "mov r12, [rsi + 16]",
        "mov r13, [rsi + 24]",
        "mov r14, [rsi + 32]",
        "mov r15, [rsi + 40]",

        // We need to set up an interrupt return frame on the new stack
        // Get new RSP and RIP
        "mov rax, [rsi + 48]",   // new RSP
        "mov rcx, [rsi + 56]",   // new RIP
        "mov rdx, [rsi + 64]",   // new RFLAGS

        // Load caller-saved registers (except rax, rcx, rdx which we're using)
        "mov r8, [rsi + 112]",
        "mov r9, [rsi + 120]",
        "mov r10, [rsi + 128]",
        "mov r11, [rsi + 136]",

        // Switch to new stack
        "mov rsp, rax",

        // Push interrupt return frame: SS, RSP, RFLAGS, CS, RIP
        "push 0x10",        // SS (kernel data segment)
        "push rax",         // RSP (same as what we just loaded)
        "push rdx",         // RFLAGS
        "push 0x08",        // CS (kernel code segment)
        "push rcx",         // RIP

        // Now load remaining registers from context
        // We need to reload rax, rcx, rdx, rsi, rdi from context
        // but rsi still points to context, so load others first
        "mov rax, [rsi + 72]",
        "mov rcx, [rsi + 80]",
        "mov rdx, [rsi + 88]",
        "mov rdi, [rsi + 104]",
        "mov rsi, [rsi + 96]",   // Load rsi last since we were using it

        "iretq",

        // ========== Switch to KERNEL via iretq ==========
        // Use iretq to atomically switch to kernel context
        // This is safe because iretq atomically sets RSP and enables interrupts
        "4:",
        // Clear the kernel flag
        "lea rax, [rip + {to_kernel}]",
        "mov byte ptr [rax], 0",

        // Load the kernel context pointer
        "lea rax, [rip + {next_ctx}]",
        "mov rdi, [rax]",

        // Set up iretq frame on CURRENT stack (safe, we're on interrupt stack)
        "push 0x10",                      // SS
        "push qword ptr [rdi + 48]",      // RSP
        // Load flags and ensure IF is set
        "mov rax, [rdi + 64]",
        "or rax, 0x200",                  // Set IF
        "push rax",                       // RFLAGS
        "push 0x08",                      // CS
        "push qword ptr [rdi + 56]",      // RIP

        // Restore callee-saved registers from context
        "mov rbx, [rdi + 0]",
        "mov rbp, [rdi + 8]",
        "mov r12, [rdi + 16]",
        "mov r13, [rdi + 24]",
        "mov r14, [rdi + 32]",
        "mov r15, [rdi + 40]",

        // iretq atomically restores RIP, CS, RFLAGS, RSP, SS
        "iretq",

        timer_handler_inner = sym timer_handler_inner,
        do_switch = sym DO_CONTEXT_SWITCH,
        to_kernel = sym SWITCH_TO_KERNEL,
        next_ctx = sym NEXT_CONTEXT_PTR,
    );
}

/// Stack frame layout after pushing all registers in the interrupt handler.
/// This matches the order we push registers in the assembly above.
#[repr(C)]
struct InterruptStackFrame {
    // Registers we pushed (in reverse order, so first pushed = highest address)
    r15: u64,
    r14: u64,
    r13: u64,
    r12: u64,
    r11: u64,
    r10: u64,
    r9: u64,
    r8: u64,
    rbp: u64,
    rdi: u64,
    rsi: u64,
    rdx: u64,
    rcx: u64,
    rbx: u64,
    rax: u64,
    // CPU-pushed interrupt frame
    rip: u64,
    cs: u64,
    rflags: u64,
    rsp: u64,
    ss: u64,
}

/// Inner handler called from the naked interrupt handler.
/// Checks if preemption is needed and sets up context switch if so.
#[no_mangle]
extern "C" fn timer_handler_inner(stack_frame: *mut InterruptStackFrame) {
    use core::sync::atomic::Ordering;
    use crate::arch::x86_64::interrupts::{TIMER_TICKS, PICS, InterruptIndex};

    // Increment tick counter
    let ticks = TIMER_TICKS.fetch_add(1, Ordering::Relaxed) + 1;

    // Check if we're running a spawned process
    let in_process = crate::process::is_in_spawned_process();

    // Try to acquire scheduler lock for sleep queue check and preemption
    let should_preempt = if let Some(mut sched) = crate::process::scheduler::SCHEDULER.try_lock() {
        if sched.is_initialized() {
            // Check sleep queue and wake any processes whose time has come
            sched.check_sleep_queue(ticks);

            // Only check for preemption if we're in a spawned process
            if in_process {
                sched.timer_tick()
            } else {
                false
            }
        } else {
            false
        }
    } else {
        false
    };

    if should_preempt && in_process {
        // Save current process context from the interrupt stack frame
        if let Some(mut sched) = crate::process::scheduler::SCHEDULER.try_lock() {
            if let Some(current_pid) = sched.current() {
                if let Some(ctx) = sched.get_context_mut(current_pid) {
                    // Copy register state from interrupt frame to context
                    let frame = unsafe { &*stack_frame };
                    ctx.rax = frame.rax;
                    ctx.rbx = frame.rbx;
                    ctx.rcx = frame.rcx;
                    ctx.rdx = frame.rdx;
                    ctx.rsi = frame.rsi;
                    ctx.rdi = frame.rdi;
                    ctx.rbp = frame.rbp;
                    ctx.r8 = frame.r8;
                    ctx.r9 = frame.r9;
                    ctx.r10 = frame.r10;
                    ctx.r11 = frame.r11;
                    ctx.r12 = frame.r12;
                    ctx.r13 = frame.r13;
                    ctx.r14 = frame.r14;
                    ctx.r15 = frame.r15;
                    ctx.rsp = frame.rsp;
                    ctx.rip = frame.rip;
                    ctx.rflags = frame.rflags;

                    crate::debug_trace!("Saved context for PID {:?}, RIP={:#x}", current_pid, ctx.rip);
                }

                // Move current to ready queue
                sched.yield_current();

                // Return to kernel context
                unsafe {
                    NEXT_CONTEXT_PTR = &KERNEL_CONTEXT as *const CpuContext;
                    SWITCH_TO_KERNEL = true;
                    DO_CONTEXT_SWITCH = true;
                }
                crate::debug_trace!("Preempting back to kernel, RIP={:#x}", unsafe { KERNEL_CONTEXT.rip });
            }
        }
    }

    // Send EOI
    unsafe {
        PICS.lock().notify_end_of_interrupt(InterruptIndex::Timer.as_u8());
    }
}

//! CPU context for process context switching
//!
//! This module defines the CPU register state that must be saved and restored
//! during context switches between processes.
//!
//! For voluntary switches (yield), only callee-saved registers need preservation.
//! For interrupt-based preemption, ALL registers must be saved since an interrupt
//! can occur at any point during execution.

/// CPU context representing the saved state of a process.
///
/// This structure stores ALL general-purpose registers to support both:
/// - Voluntary context switches (only callee-saved registers matter)
/// - Interrupt-based preemption (all registers must be preserved)
///
/// The layout is designed to match the order we push/pop in assembly.
/// The `#[repr(C)]` attribute ensures predictable memory layout.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CpuContext {
    // Callee-saved registers (used by voluntary switch_context)
    /// RBX - callee-saved general purpose register
    pub rbx: u64,
    /// RBP - base pointer (callee-saved)
    pub rbp: u64,
    /// R12 - callee-saved general purpose register
    pub r12: u64,
    /// R13 - callee-saved general purpose register
    pub r13: u64,
    /// R14 - callee-saved general purpose register
    pub r14: u64,
    /// R15 - callee-saved general purpose register
    pub r15: u64,
    /// RSP - stack pointer
    pub rsp: u64,
    /// RIP - instruction pointer (return address)
    pub rip: u64,
    /// RFLAGS - processor flags
    pub rflags: u64,

    // Caller-saved registers (needed for interrupt-based preemption)
    /// RAX - accumulator register
    pub rax: u64,
    /// RCX - counter register
    pub rcx: u64,
    /// RDX - data register
    pub rdx: u64,
    /// RSI - source index
    pub rsi: u64,
    /// RDI - destination index
    pub rdi: u64,
    /// R8 - general purpose register
    pub r8: u64,
    /// R9 - general purpose register
    pub r9: u64,
    /// R10 - general purpose register
    pub r10: u64,
    /// R11 - general purpose register
    pub r11: u64,
}

impl CpuContext {
    /// Create a new CPU context initialized to zero.
    pub const fn new() -> Self {
        CpuContext {
            rbx: 0,
            rbp: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
            rsp: 0,
            rip: 0,
            rflags: 0,
            rax: 0,
            rcx: 0,
            rdx: 0,
            rsi: 0,
            rdi: 0,
            r8: 0,
            r9: 0,
            r10: 0,
            r11: 0,
        }
    }

    /// Initialize a context for a new process.
    ///
    /// Sets up the context so that when switched to, execution will begin
    /// at `entry_point` with the stack pointer set to `stack_top`.
    ///
    /// # Arguments
    /// * `stack_top` - The top of the process's stack (highest address)
    /// * `entry_point` - The function address where execution should begin
    ///
    /// # Returns
    /// A new CpuContext ready for context switching
    pub fn init_for_new_process(stack_top: u64, entry_point: u64) -> Self {
        // Initial flags: interrupts DISABLED initially, reserved bit 1 always set
        // The process will enable interrupts once it's safely running
        const INITIAL_RFLAGS: u64 = 0x002; // Only reserved bit 1, no IF

        CpuContext {
            rbx: 0,
            rbp: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
            // Stack must be 16-byte aligned before call instruction
            // Subtract 8 for the "return address" slot that call would push
            rsp: stack_top - 8,
            rip: entry_point,
            rflags: INITIAL_RFLAGS,
            rax: 0,
            rcx: 0,
            rdx: 0,
            rsi: 0,
            rdi: 0,
            r8: 0,
            r9: 0,
            r10: 0,
            r11: 0,
        }
    }
}

impl Default for CpuContext {
    fn default() -> Self {
        Self::new()
    }
}

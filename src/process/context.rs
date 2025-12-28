//! CPU context for process context switching
//!
//! This module defines the CPU register state that must be saved and restored
//! during context switches between processes. Only callee-saved registers need
//! to be preserved, as the caller-saved registers are handled by the compiler.

/// CPU context representing the saved state of a process.
///
/// This structure stores the callee-saved registers according to the System V AMD64 ABI:
/// - RBX, RBP, R12-R15 are callee-saved (must be preserved across function calls)
/// - RSP is the stack pointer
/// - RIP is stored as the return address for context switch
/// - RFLAGS stores the processor flags
///
/// The `#[repr(C)]` attribute ensures the struct has a predictable memory layout
/// for the assembly context switch code.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CpuContext {
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
        // Initial flags: interrupts enabled (bit 9), reserved bit 1 always set
        const INITIAL_RFLAGS: u64 = 0x202;

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
        }
    }
}

impl Default for CpuContext {
    fn default() -> Self {
        Self::new()
    }
}

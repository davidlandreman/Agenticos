//! Process stack allocator
//!
//! Allocates separate stacks for each process from a dedicated memory region.
//! Each process gets a fixed-size stack to avoid stack overflow into other
//! process memory.

use alloc::vec::Vec;
use spin::Mutex;

/// Size of each process stack (64 KB)
pub const STACK_SIZE: usize = 64 * 1024;

/// Start of the stack allocation region (above heap at 0x4444_4444_0000)
pub const STACK_REGION_START: u64 = 0x_5555_0000_0000;

/// Maximum number of concurrent processes
pub const MAX_PROCESSES: usize = 64;

/// Guard page size between stacks to catch overflow
const GUARD_PAGE_SIZE: usize = 4096;

/// Total size for each stack slot (stack + guard page)
const STACK_SLOT_SIZE: usize = STACK_SIZE + GUARD_PAGE_SIZE;

/// Global stack allocator instance
pub static STACK_ALLOCATOR: Mutex<StackAllocator> = Mutex::new(StackAllocator::new());

/// Allocator for process stacks
///
/// Manages a region of virtual memory dedicated to process stacks.
/// Each stack slot includes a guard page at the bottom to detect overflow.
pub struct StackAllocator {
    /// Next stack index to allocate (if free_list is empty)
    next_index: usize,
    /// List of freed stack indices available for reuse
    free_list: Vec<usize>,
    /// Track which indices are currently allocated
    allocated: [bool; MAX_PROCESSES],
}

impl StackAllocator {
    /// Create a new stack allocator
    pub const fn new() -> Self {
        StackAllocator {
            next_index: 0,
            free_list: Vec::new(),
            allocated: [false; MAX_PROCESSES],
        }
    }

    /// Allocate a new process stack
    ///
    /// # Returns
    /// * `Ok((base, top))` - The base address and top (highest) address of the stack
    /// * `Err` - If no more stacks are available
    ///
    /// The stack grows downward, so `top` is where RSP should initially point,
    /// and `base` is the lowest valid stack address (above the guard page).
    pub fn allocate(&mut self) -> Result<(u64, u64), &'static str> {
        // Try to reuse a freed stack first
        let index = if let Some(idx) = self.free_list.pop() {
            idx
        } else {
            // Allocate a new slot
            if self.next_index >= MAX_PROCESSES {
                return Err("Maximum process limit reached");
            }
            let idx = self.next_index;
            self.next_index += 1;
            idx
        };

        self.allocated[index] = true;

        // Calculate addresses for this stack slot
        // Layout: [guard page][stack space]
        //         ^base      ^            ^top
        let slot_start = STACK_REGION_START + (index as u64 * STACK_SLOT_SIZE as u64);
        let stack_base = slot_start + GUARD_PAGE_SIZE as u64; // Above guard page
        let stack_top = slot_start + STACK_SLOT_SIZE as u64;  // Top of stack

        crate::debug_info!(
            "StackAllocator: Allocated stack {} at base={:#x} top={:#x}",
            index, stack_base, stack_top
        );

        Ok((stack_base, stack_top))
    }

    /// Free a previously allocated stack
    ///
    /// # Arguments
    /// * `stack_base` - The base address returned from `allocate()`
    pub fn free(&mut self, stack_base: u64) {
        // Calculate which index this stack belongs to
        let offset = stack_base - STACK_REGION_START - GUARD_PAGE_SIZE as u64;
        let index = (offset / STACK_SLOT_SIZE as u64) as usize;

        if index < MAX_PROCESSES && self.allocated[index] {
            self.allocated[index] = false;
            self.free_list.push(index);
            crate::debug_info!("StackAllocator: Freed stack {}", index);
        } else {
            crate::debug_warn!(
                "StackAllocator: Attempted to free invalid stack at {:#x}",
                stack_base
            );
        }
    }

    /// Get the number of currently allocated stacks
    pub fn allocated_count(&self) -> usize {
        self.allocated.iter().filter(|&&x| x).count()
    }

    /// Check if we can allocate more stacks
    pub fn can_allocate(&self) -> bool {
        !self.free_list.is_empty() || self.next_index < MAX_PROCESSES
    }
}

/// Allocate a stack using the global allocator
pub fn allocate_stack() -> Result<(u64, u64), &'static str> {
    STACK_ALLOCATOR.lock().allocate()
}

/// Free a stack using the global allocator
pub fn free_stack(stack_base: u64) {
    STACK_ALLOCATOR.lock().free(stack_base);
}

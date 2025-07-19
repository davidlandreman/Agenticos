use crate::{debug_info, debug_error};

/// Debug breakpoint handler that dumps current execution state
pub fn debug_breakpoint() {
    debug_info!("=== DEBUG BREAKPOINT TRIGGERED ===");
    
    // Get current stack pointer and frame pointer
    let rsp: u64;
    let rbp: u64;
    let rip: u64;
    
    unsafe {
        core::arch::asm!("mov {}, rsp", out(reg) rsp);
        core::arch::asm!("mov {}, rbp", out(reg) rbp);
        
        // Get return address from stack
        rip = *(rsp as *const u64);
    }
    
    debug_info!("Current execution state:");
    debug_info!("  RSP (Stack Pointer): 0x{:016x}", rsp);
    debug_info!("  RBP (Base Pointer):  0x{:016x}", rbp);
    debug_info!("  RIP (Return Address): 0x{:016x}", rip);
    
    // Dump stack trace
    dump_stack_trace(rbp);
    
    // Dump recent stack contents
    dump_stack_contents(rsp);
    
    debug_info!("=== END DEBUG BREAKPOINT ===");
}

/// Walk the stack frame chain to show call trace
fn dump_stack_trace(mut rbp: u64) {
    debug_info!("Stack trace:");
    
    let mut frame_count = 0;
    const MAX_FRAMES: usize = 10;
    
    while frame_count < MAX_FRAMES && rbp != 0 {
        // Stack frame layout: [old_rbp][return_address]
        // RBP points to old_rbp
        
        if rbp < 0x1000 || rbp > 0x7fffffffffff {
            debug_error!("  Invalid frame pointer: 0x{:016x}", rbp);
            break;
        }
        
        unsafe {
            // Get return address (rbp + 8)
            let return_addr_ptr = (rbp + 8) as *const u64;
            if return_addr_ptr as u64 > 0x7fffffffffff {
                debug_error!("  Invalid return address pointer: 0x{:016x}", return_addr_ptr as u64);
                break;
            }
            
            let return_addr = *return_addr_ptr;
            debug_info!("  Frame {}: 0x{:016x}", frame_count, return_addr);
            
            // Get previous frame pointer
            let old_rbp_ptr = rbp as *const u64;
            let old_rbp = *old_rbp_ptr;
            
            // Prevent infinite loops
            if old_rbp == rbp || old_rbp < rbp {
                break;
            }
            
            rbp = old_rbp;
        }
        
        frame_count += 1;
    }
    
    if frame_count == MAX_FRAMES {
        debug_info!("  ... (truncated at {} frames)", MAX_FRAMES);
    }
}

/// Dump the top of the stack for debugging
fn dump_stack_contents(rsp: u64) {
    debug_info!("Stack contents (top 16 entries):");
    
    for i in 0..16 {
        let addr = rsp + (i * 8);
        
        if addr > 0x7fffffffffff {
            break;
        }
        
        unsafe {
            let value = *(addr as *const u64);
            debug_info!("  [0x{:016x}] = 0x{:016x}", addr, value);
        }
    }
}

/// Trigger a software breakpoint interrupt (INT3)
pub fn software_breakpoint() {
    debug_info!("Triggering software breakpoint...");
    unsafe {
        core::arch::asm!("int3");
    }
}
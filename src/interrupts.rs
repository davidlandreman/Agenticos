use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};
use lazy_static::lazy_static;
use crate::{debug_error, debug_info, println};

lazy_static! {
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();
        
        // Set up exception handlers
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        idt.page_fault.set_handler_fn(page_fault_handler);
        idt.general_protection_fault.set_handler_fn(general_protection_fault_handler);
        idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
        idt.divide_error.set_handler_fn(divide_error_handler);
        idt.overflow.set_handler_fn(overflow_handler);
        idt.bound_range_exceeded.set_handler_fn(bound_range_exceeded_handler);
        idt.invalid_tss.set_handler_fn(invalid_tss_handler);
        idt.segment_not_present.set_handler_fn(segment_not_present_handler);
        idt.stack_segment_fault.set_handler_fn(stack_segment_fault_handler);
        idt.alignment_check.set_handler_fn(alignment_check_handler);
        
        idt
    };
}

pub fn init_idt() {
    debug_info!("Initializing Interrupt Descriptor Table...");
    IDT.load();
    debug_info!("IDT loaded successfully");
}

// Exception Handlers

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    debug_error!("EXCEPTION: BREAKPOINT");
    debug_error!("{:#?}", stack_frame);
    
    println!();
    println!("EXCEPTION: BREAKPOINT");
    println!("{:#?}", stack_frame);
}

extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: x86_64::structures::idt::PageFaultErrorCode,
) {
    use x86_64::registers::control::Cr2;
    
    debug_error!("EXCEPTION: PAGE FAULT");
    debug_error!("Accessed Address: {:?}", Cr2::read());
    debug_error!("Error Code: {:?}", error_code);
    debug_error!("{:#?}", stack_frame);
    
    println!();
    println!("EXCEPTION: PAGE FAULT");
    println!("Accessed Address: {:?}", Cr2::read());
    println!("Error Code: {:?}", error_code);
    println!("{:#?}", stack_frame);
    
    panic!("Page fault");
}

extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    _error_code: u64,
) {
    debug_error!("EXCEPTION: DOUBLE FAULT");
    debug_error!("{:#?}", stack_frame);
    
    loop {}
}

extern "x86-interrupt" fn general_protection_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    debug_error!("EXCEPTION: GENERAL PROTECTION FAULT");
    debug_error!("Error Code: {}", error_code);
    debug_error!("{:#?}", stack_frame);
    
    println!();
    println!("EXCEPTION: GENERAL PROTECTION FAULT");
    println!("Error Code: {}", error_code);
    println!("{:#?}", stack_frame);
    
    panic!("General protection fault");
}

extern "x86-interrupt" fn invalid_opcode_handler(stack_frame: InterruptStackFrame) {
    debug_error!("EXCEPTION: INVALID OPCODE");
    debug_error!("{:#?}", stack_frame);
    
    println!();
    println!("EXCEPTION: INVALID OPCODE");
    println!("{:#?}", stack_frame);
    
    panic!("Invalid opcode");
}

extern "x86-interrupt" fn divide_error_handler(stack_frame: InterruptStackFrame) {
    debug_error!("EXCEPTION: DIVIDE ERROR");
    debug_error!("{:#?}", stack_frame);
    
    println!();
    println!("EXCEPTION: DIVIDE ERROR");
    println!("{:#?}", stack_frame);
    
    panic!("Divide error");
}

extern "x86-interrupt" fn overflow_handler(stack_frame: InterruptStackFrame) {
    debug_error!("EXCEPTION: OVERFLOW");
    debug_error!("{:#?}", stack_frame);
    
    println!();
    println!("EXCEPTION: OVERFLOW");
    println!("{:#?}", stack_frame);
    
    panic!("Overflow");
}

extern "x86-interrupt" fn bound_range_exceeded_handler(stack_frame: InterruptStackFrame) {
    debug_error!("EXCEPTION: BOUND RANGE EXCEEDED");
    debug_error!("{:#?}", stack_frame);
    
    println!();
    println!("EXCEPTION: BOUND RANGE EXCEEDED");
    println!("{:#?}", stack_frame);
    
    panic!("Bound range exceeded");
}

extern "x86-interrupt" fn invalid_tss_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    debug_error!("EXCEPTION: INVALID TSS");
    debug_error!("Error Code: {}", error_code);
    debug_error!("{:#?}", stack_frame);
    
    println!();
    println!("EXCEPTION: INVALID TSS");
    println!("Error Code: {}", error_code);
    println!("{:#?}", stack_frame);
    
    panic!("Invalid TSS");
}

extern "x86-interrupt" fn segment_not_present_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    debug_error!("EXCEPTION: SEGMENT NOT PRESENT");
    debug_error!("Error Code: {}", error_code);
    debug_error!("{:#?}", stack_frame);
    
    println!();
    println!("EXCEPTION: SEGMENT NOT PRESENT");
    println!("Error Code: {}", error_code);
    println!("{:#?}", stack_frame);
    
    panic!("Segment not present");
}

extern "x86-interrupt" fn stack_segment_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    debug_error!("EXCEPTION: STACK SEGMENT FAULT");
    debug_error!("Error Code: {}", error_code);
    debug_error!("{:#?}", stack_frame);
    
    println!();
    println!("EXCEPTION: STACK SEGMENT FAULT");
    println!("Error Code: {}", error_code);
    println!("{:#?}", stack_frame);
    
    panic!("Stack segment fault");
}

extern "x86-interrupt" fn alignment_check_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    debug_error!("EXCEPTION: ALIGNMENT CHECK");
    debug_error!("Error Code: {}", error_code);
    debug_error!("{:#?}", stack_frame);
    
    println!();
    println!("EXCEPTION: ALIGNMENT CHECK");
    println!("Error Code: {}", error_code);
    println!("{:#?}", stack_frame);
    
    panic!("Alignment check");
}

// Test functions
#[cfg(test)]
mod tests {
    #[test_case]
    fn test_breakpoint_exception() {
        // invoke a breakpoint exception
        x86_64::instructions::interrupts::int3();
    }
}
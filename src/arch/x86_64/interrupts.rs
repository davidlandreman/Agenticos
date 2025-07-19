use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};
use lazy_static::lazy_static;
use pic8259::ChainedPics;
use spin::Mutex;
use crate::{debug_error, debug_info, println};

pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

pub static PICS: Mutex<ChainedPics> = Mutex::new(unsafe { 
    ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) 
});

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InterruptIndex {
    Timer = PIC_1_OFFSET,
    Keyboard,
    Mouse = PIC_2_OFFSET + 4,  // IRQ12
}

impl InterruptIndex {
    fn as_u8(self) -> u8 {
        self as u8
    }

    fn as_usize(self) -> usize {
        usize::from(self.as_u8())
    }
}

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
        
        // Set up hardware interrupt handlers
        idt[InterruptIndex::Timer.as_usize()].set_handler_fn(timer_interrupt_handler);
        idt[InterruptIndex::Keyboard.as_usize()].set_handler_fn(keyboard_interrupt_handler);
        idt[InterruptIndex::Mouse.as_usize()].set_handler_fn(mouse_interrupt_handler);
        
        idt
    };
}

pub fn init_idt() {
    debug_info!("Initializing Interrupt Descriptor Table...");
    
    // Verify IDT entries are set up
    debug_info!("IDT entries configured:");
    debug_info!("  Timer interrupt vector: {}", InterruptIndex::Timer.as_u8());
    debug_info!("  Keyboard interrupt vector: {}", InterruptIndex::Keyboard.as_u8());
    debug_info!("  Mouse interrupt vector: {}", InterruptIndex::Mouse.as_u8());
    
    IDT.load();
    debug_info!("IDT loaded successfully");
    
    debug_info!("Initializing PIC...");
    unsafe { 
        PICS.lock().initialize();
    }
    debug_info!("PIC initialized successfully");
    
    // Configure PIC masks
    debug_info!("Configuring PIC interrupt masks...");
    unsafe {
        use x86_64::instructions::port::Port;
        
        // Read current masks
        let mut pic1_mask_port = Port::<u8>::new(0x21);
        let mut pic2_mask_port = Port::<u8>::new(0xA1);
        
        let pic1_mask = pic1_mask_port.read();
        let pic2_mask = pic2_mask_port.read();
        debug_info!("Initial PIC masks: PIC1=0x{:02x}, PIC2=0x{:02x}", pic1_mask, pic2_mask);
        
        // Set specific mask values to enable our interrupts
        // PIC1: Enable timer (bit 0), keyboard (bit 1), cascade (bit 2)
        // All other interrupts masked (1)
        let new_pic1_mask: u8 = 0xF8;  // 11111000 - enables IRQ0, IRQ1, IRQ2
        
        // PIC2: Enable mouse (bit 4 = IRQ12)
        // All other interrupts masked (1)  
        let new_pic2_mask: u8 = 0xEF;  // 11101111 - enables IRQ12
        
        // Write new masks
        pic1_mask_port.write(new_pic1_mask);
        // Small delay
        for _ in 0..1000 {
            core::hint::spin_loop();
        }
        pic2_mask_port.write(new_pic2_mask);
        
        // Re-read to verify changes took effect
        let verify_pic1_mask = pic1_mask_port.read();
        let verify_pic2_mask = pic2_mask_port.read();
        debug_info!("Final PIC masks: PIC1=0x{:02x}, PIC2=0x{:02x}", verify_pic1_mask, verify_pic2_mask);
        
        // Check which interrupts are now enabled
        debug_info!("Enabled interrupts:");
        if verify_pic1_mask & 0x01 == 0 { debug_info!("  - Timer (IRQ0) enabled"); }
        if verify_pic1_mask & 0x02 == 0 { debug_info!("  - Keyboard (IRQ1) enabled"); }
        if verify_pic1_mask & 0x04 == 0 { debug_info!("  - Cascade (IRQ2) enabled"); }
        if verify_pic2_mask & 0x10 == 0 { debug_info!("  - Mouse (IRQ12) enabled"); }
    }
    
    debug_info!("Enabling interrupts...");
    x86_64::instructions::interrupts::enable();
    debug_info!("Interrupts enabled - system ready for hardware events");
}

// Exception Handlers

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    debug_error!("EXCEPTION: BREAKPOINT");
    debug_error!("{:#?}", stack_frame);
    
    println!();
    println!("EXCEPTION: BREAKPOINT");
    println!("{:#?}", stack_frame);
    
    // Call our enhanced debug breakpoint handler
    crate::lib::debug_breakpoint::debug_breakpoint();
}

extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: x86_64::structures::idt::PageFaultErrorCode,
) {
    use x86_64::registers::control::Cr2;
    use x86_64::VirtAddr;
    
    let accessed_addr = Cr2::read();
    
    // Don't check for physical memory offset access here - let the mapper handle it
    // The mapper knows the actual physical memory offset from the bootloader
    
    // Check if the fault is in our heap region
    const HEAP_START: u64 = 0x_4444_4444_0000;
    const HEAP_END: u64 = HEAP_START + (100 * 1024 * 1024); // 100 MiB
    
    if accessed_addr.as_u64() >= HEAP_START && accessed_addr.as_u64() < HEAP_END {
        // This is a heap access - we should allocate and map a page
        debug_info!("Page fault in heap region at {:?}", accessed_addr);
        
        // Try to handle the page fault
        if let Some(mapper) = unsafe { crate::mm::paging::get_mapper() } {
            if let Err(e) = mapper.handle_page_fault(accessed_addr) {
                debug_error!("Failed to handle page fault: {:?}", e);
                panic!("Failed to allocate memory for heap");
            }
            // Successfully mapped - return and retry the instruction
            return;
        }
    }
    
    // Not a heap fault or couldn't handle it - panic
    debug_error!("EXCEPTION: PAGE FAULT");
    debug_error!("Accessed Address: {:?}", accessed_addr);
    debug_error!("Error Code: {:?}", error_code);
    debug_error!("Instruction Pointer: {:?}", stack_frame.instruction_pointer);
    debug_error!("{:#?}", stack_frame);
    
    panic!("Unhandled page fault");
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

// Hardware Interrupt Handlers

static mut TIMER_TICKS: u64 = 0;

extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    unsafe {
        TIMER_TICKS += 1;
        // Only log every 100 ticks to avoid spam
        if TIMER_TICKS % 100 == 0 {
            crate::debug_debug!("Timer tick: {}", TIMER_TICKS);
        }
        PICS.lock().notify_end_of_interrupt(InterruptIndex::Timer.as_u8());
    }
}

extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;
    
    let mut port = Port::new(0x60);
    let scancode: u8 = unsafe { port.read() };
    
    // Debug: log keyboard interrupt
    crate::debug_trace!("Keyboard interrupt: scancode=0x{:02x}", scancode);
    
    crate::drivers::keyboard::add_scancode(scancode);
    
    // Wake up any processes waiting for stdin input
    crate::stdlib::waker::wake_stdin_waiters();
    
    unsafe {
        PICS.lock().notify_end_of_interrupt(InterruptIndex::Keyboard.as_u8());
    }
}

extern "x86-interrupt" fn mouse_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;
    
    let mut port = Port::new(0x60);
    let data: u8 = unsafe { port.read() };
    
    // Debug: log mouse interrupt
    crate::debug_trace!("Mouse interrupt: data=0x{:02x}", data);
    
    crate::drivers::mouse::handle_interrupt(data);
    
    unsafe {
        PICS.lock().notify_end_of_interrupt(InterruptIndex::Mouse.as_u8());
    }
}
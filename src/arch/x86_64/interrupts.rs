use crate::userland::lifecycle::{cleanup_user_process, frame_is_user, AbnormalExit};
use crate::{debug_error, debug_info, debug_trace, debug_warn, println};
use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use lazy_static::lazy_static;
use pic8259::ChainedPics;
use spin::Mutex;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};

/// If the saved CS in the interrupt frame has RPL=3, the fault occurred in
/// user mode — terminate the user process cleanly instead of panicking.
/// Vector numbers per Intel SDM Vol. 3A §6.15.
fn route_user_fault_or_panic(
    stack_frame: &InterruptStackFrame,
    vector: u8,
    error_code: Option<u64>,
    fault_addr: Option<x86_64::VirtAddr>,
    panic_msg: &'static str,
) -> ! {
    crate::diagnostics::trace::record_interrupt_boundary(
        crate::diagnostics::trace::EventKind::InterruptEntry,
        vector,
        (stack_frame.code_segment as u64 & 3) as u8,
        false,
        crate::diagnostics::trace::InterruptOutcome::Return,
    );
    if frame_is_user(stack_frame.code_segment as u64) {
        cleanup_user_process(AbnormalExit {
            vector,
            error_code,
            fault_addr,
            fault_rip: stack_frame.instruction_pointer,
        });
    }
    crate::diagnostics::crash::begin_trap(
        panic_msg,
        vector,
        error_code,
        fault_addr.map(|address| address.as_u64()),
        stack_frame.instruction_pointer.as_u64(),
    );
}

/// PIT (Programmable Interval Timer) base frequency in Hz
const PIT_BASE_FREQUENCY: u32 = 1_193_182;

/// Desired timer frequency in Hz (100 Hz = 10ms ticks)
/// This provides good balance between responsiveness and interrupt overhead
pub const TIMER_FREQUENCY_HZ: u32 = 100;

pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

pub static PICS: Mutex<ChainedPics> =
    Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

const CONTROLLER_PIC: u8 = 0;
const CONTROLLER_APIC: u8 = 1;
static INTERRUPT_CONTROLLER: AtomicU8 = AtomicU8::new(CONTROLLER_PIC);

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InterruptIndex {
    Timer = PIC_1_OFFSET,
    Keyboard,
    Mouse = PIC_2_OFFSET + 4, // IRQ12
}

impl InterruptIndex {
    pub fn as_u8(self) -> u8 {
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
        unsafe {
            idt.non_maskable_interrupt
                .set_handler_fn(crash_nmi_handler)
                .set_stack_index(crate::arch::x86_64::gdt::PANIC_NMI_IST_INDEX);
        }
        idt.page_fault.set_handler_fn(page_fault_handler);
        // Wire #DF to the IST stack so a kernel-stack overflow during user-mode
        // work cannot triple-fault the machine. Same `set_handler_addr` workaround
        // as the timer handler — the diverging-handler trait shape is not
        // expressible on this nightly.
        unsafe {
            let handler_addr = double_fault_handler as usize;
            idt.double_fault
                .set_handler_addr(x86_64::VirtAddr::new(handler_addr as u64))
                .set_stack_index(crate::arch::x86_64::gdt::DOUBLE_FAULT_IST_INDEX);
        }
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
        // Timer uses the preemptive handler for true multitasking
        unsafe {

            use crate::arch::x86_64::preemption::timer_interrupt_handler_preemptive;
            let handler_addr = timer_interrupt_handler_preemptive as usize;
            idt[InterruptIndex::Timer.as_usize()]
                .set_handler_addr(x86_64::VirtAddr::new(handler_addr as u64));
        }
        idt[InterruptIndex::Keyboard.as_usize()].set_handler_fn(keyboard_interrupt_handler);
        idt[InterruptIndex::Mouse.as_usize()].set_handler_fn(mouse_interrupt_handler);
        idt[PIC_1_OFFSET as usize + 3].set_handler_fn(pci_irq3_handler);
        idt[PIC_1_OFFSET as usize + 4].set_handler_fn(pci_irq4_handler);
        idt[PIC_1_OFFSET as usize + 5].set_handler_fn(pci_irq5_handler);
        idt[PIC_1_OFFSET as usize + 6].set_handler_fn(pci_irq6_handler);
        idt[PIC_1_OFFSET as usize + 7].set_handler_fn(pci_irq7_handler);
        idt[PIC_1_OFFSET as usize + 8].set_handler_fn(pci_irq8_handler);
        idt[PIC_1_OFFSET as usize + 9].set_handler_fn(pci_irq9_handler);
        idt[PIC_1_OFFSET as usize + 10].set_handler_fn(pci_irq10_handler);
        idt[PIC_1_OFFSET as usize + 11].set_handler_fn(pci_irq11_handler);
        idt[PIC_1_OFFSET as usize + 13].set_handler_fn(pci_irq13_handler);
        idt[PIC_1_OFFSET as usize + 14].set_handler_fn(pci_irq14_handler);
        idt[PIC_1_OFFSET as usize + 15].set_handler_fn(pci_irq15_handler);
        unsafe {
            use crate::arch::x86_64::preemption::timer_interrupt_handler_preemptive;
            idt[crate::arch::x86_64::lapic::LAPIC_TIMER_VECTOR as usize].set_handler_addr(
                x86_64::VirtAddr::new(timer_interrupt_handler_preemptive as usize as u64),
            );
        }
        idt[crate::arch::x86_64::lapic::RESCHEDULE_VECTOR as usize]
            .set_handler_fn(reschedule_interrupt_handler);
        idt[crate::arch::x86_64::lapic::HALT_VECTOR as usize]
            .set_handler_fn(halt_interrupt_handler);
        idt[crate::arch::x86_64::lapic::ERROR_VECTOR as usize]
            .set_handler_fn(lapic_error_interrupt_handler);
        idt[crate::arch::x86_64::lapic::SPURIOUS_VECTOR as usize]
            .set_handler_fn(lapic_spurious_interrupt_handler);

        // The legacy `int 0x80` IDT vector that PR #12 installed has been
        // removed: userland now enters the kernel via the `syscall` instruction
        // (programmed in `arch::x86_64::syscall::init_syscall_msrs`). A stray
        // `int 0x80` from a user binary now triggers `#GP`, which the
        // existing exception path routes to `cleanup_user_process`.
        idt
    };
}

/// Unmask a routed PCI INTx line after its driver is ready to acknowledge it.
pub fn enable_pci_irq(irq: u8) -> bool {
    if irq >= 16 || matches!(irq, 0 | 1 | 2 | 12) {
        return false;
    }
    if INTERRUPT_CONTROLLER.load(Ordering::Acquire) == CONTROLLER_APIC {
        return crate::arch::x86_64::ioapic::route_pci_irq(
            irq,
            crate::arch::x86_64::acpi::topology().bsp_lapic_id,
        );
    }
    x86_64::instructions::interrupts::without_interrupts(|| unsafe {
        use x86_64::instructions::port::Port;
        let (port, bit) = if irq < 8 {
            (0x21, irq)
        } else {
            (0xA1, irq - 8)
        };
        let mut mask = Port::<u8>::new(port);
        let current = mask.read();
        mask.write(current & !(1 << bit));
    });
    true
}

fn handle_pci_irq(irq: u8) {
    crate::drivers::virtio::block::handle_interrupt(irq);
    eoi(PIC_1_OFFSET + irq);
}

macro_rules! pci_irq_handlers {
    ($(($name:ident, $irq:literal)),+ $(,)?) => {$ (
        extern "x86-interrupt" fn $name(_frame: InterruptStackFrame) {
            handle_pci_irq($irq);
        }
    )+ };
}

pci_irq_handlers!(
    (pci_irq3_handler, 3),
    (pci_irq4_handler, 4),
    (pci_irq5_handler, 5),
    (pci_irq6_handler, 6),
    (pci_irq7_handler, 7),
    (pci_irq8_handler, 8),
    (pci_irq9_handler, 9),
    (pci_irq10_handler, 10),
    (pci_irq11_handler, 11),
    (pci_irq13_handler, 13),
    (pci_irq14_handler, 14),
    (pci_irq15_handler, 15),
);

/// Configure the PIT (Programmable Interval Timer) to fire at the specified frequency
///
/// The PIT has a base frequency of 1,193,182 Hz. We set a divisor to get our
/// desired frequency: divisor = base_freq / desired_freq
fn configure_pit() {
    use x86_64::instructions::port::Port;

    let divisor = (PIT_BASE_FREQUENCY / TIMER_FREQUENCY_HZ) as u16;
    debug_info!(
        "Configuring PIT: {} Hz (divisor {})",
        TIMER_FREQUENCY_HZ,
        divisor
    );

    unsafe {
        // PIT command port (0x43): Select channel 0, access mode lobyte/hibyte,
        // mode 3 (square wave generator), binary mode
        // Bits: 00 11 011 0 = 0x36
        let mut command_port: Port<u8> = Port::new(0x43);
        command_port.write(0x36);

        // Write divisor to channel 0 data port (0x40)
        // Low byte first, then high byte
        let mut data_port: Port<u8> = Port::new(0x40);
        data_port.write((divisor & 0xFF) as u8);
        data_port.write((divisor >> 8) as u8);
    }

    debug_info!(
        "PIT configured for {} Hz timer interrupts",
        TIMER_FREQUENCY_HZ
    );
}

pub fn init_idt() {
    debug_info!("Initializing Interrupt Descriptor Table...");

    // Verify IDT entries are set up
    debug_info!("IDT entries configured:");
    debug_info!(
        "  Timer interrupt vector: {}",
        InterruptIndex::Timer.as_u8()
    );
    debug_info!(
        "  Keyboard interrupt vector: {}",
        InterruptIndex::Keyboard.as_u8()
    );
    debug_info!(
        "  Mouse interrupt vector: {}",
        InterruptIndex::Mouse.as_u8()
    );

    IDT.load();
    debug_info!("IDT loaded successfully");
}

pub fn load_idt_on_ap() {
    IDT.load();
}

/// Configure the clock and external interrupt controller, then enable IRQs.
/// The IDT must already be loaded and the memory mapper/ACPI topology must be
/// initialized so the APIC MMIO pages can be installed.
pub fn init_interrupt_controllers() {
    configure_pit();

    let topology = crate::arch::x86_64::acpi::topology();
    let apic_ready = crate::arch::x86_64::lapic::init(topology.lapic_mmio_base)
        && crate::arch::x86_64::ioapic::init(topology);
    if apic_ready {
        mask_legacy_pic();
        INTERRUPT_CONTROLLER.store(CONTROLLER_APIC, Ordering::Release);
        debug_info!("interrupt delivery switched to LAPIC + IOAPIC");
    } else {
        init_legacy_pic();
        INTERRUPT_CONTROLLER.store(CONTROLLER_PIC, Ordering::Release);
        debug_warn!("APIC unavailable; retaining legacy PIC interrupt delivery");
    }

    debug_info!("Enabling interrupts...");
    x86_64::instructions::interrupts::enable();
    debug_info!("Interrupts enabled - system ready for hardware events");
}

fn init_legacy_pic() {
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
        debug_info!(
            "Initial PIC masks: PIC1=0x{:02x}, PIC2=0x{:02x}",
            pic1_mask,
            pic2_mask
        );

        // Set specific mask values to enable our interrupts
        // PIC1: Enable timer (bit 0), keyboard (bit 1), cascade (bit 2)
        // All other interrupts masked (1)
        let new_pic1_mask: u8 = 0xF8; // 11111000 - enables IRQ0, IRQ1, IRQ2

        // PIC2: Enable mouse (bit 4 = IRQ12)
        // All other interrupts masked (1)
        let new_pic2_mask: u8 = 0xEF; // 11101111 - enables IRQ12

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
        debug_info!(
            "Final PIC masks: PIC1=0x{:02x}, PIC2=0x{:02x}",
            verify_pic1_mask,
            verify_pic2_mask
        );

        // Check which interrupts are now enabled
        debug_info!("Enabled interrupts:");
        if verify_pic1_mask & 0x01 == 0 {
            debug_info!("  - Timer (IRQ0) enabled");
        }
        if verify_pic1_mask & 0x02 == 0 {
            debug_info!("  - Keyboard (IRQ1) enabled");
        }
        if verify_pic1_mask & 0x04 == 0 {
            debug_info!("  - Cascade (IRQ2) enabled");
        }
        if verify_pic2_mask & 0x10 == 0 {
            debug_info!("  - Mouse (IRQ12) enabled");
        }
    }
}

fn mask_legacy_pic() {
    unsafe {
        // Remap first so a late/in-service legacy edge cannot collide with an
        // exception vector, then mask every input permanently.
        PICS.lock().initialize();
        use x86_64::instructions::port::Port;
        Port::<u8>::new(0x21).write(0xff);
        Port::<u8>::new(0xa1).write(0xff);
    }
}

#[inline]
pub fn eoi(vector: u8) {
    if INTERRUPT_CONTROLLER.load(Ordering::Acquire) == CONTROLLER_APIC {
        crate::arch::x86_64::lapic::eoi();
    } else {
        unsafe { PICS.lock().notify_end_of_interrupt(vector) };
    }
}

// Exception Handlers

extern "x86-interrupt" fn crash_nmi_handler(stack_frame: InterruptStackFrame) {
    let previous_cpl = (stack_frame.code_segment as u64 & 3) as u8;
    crate::diagnostics::trace::record_interrupt_boundary(
        crate::diagnostics::trace::EventKind::InterruptEntry,
        2,
        previous_cpl,
        false,
        crate::diagnostics::trace::InterruptOutcome::Return,
    );
    crate::diagnostics::crash::handle_nmi(stack_frame.instruction_pointer.as_u64());
    crate::diagnostics::trace::record_interrupt_boundary(
        crate::diagnostics::trace::EventKind::InterruptExit,
        2,
        previous_cpl,
        false,
        crate::diagnostics::trace::InterruptOutcome::Return,
    );
}

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

    let accessed_addr = Cr2::read();
    let previous_cpl = (stack_frame.code_segment as u64 & 3) as u8;
    crate::diagnostics::trace::record_interrupt_boundary(
        crate::diagnostics::trace::EventKind::InterruptEntry,
        14,
        previous_cpl,
        false,
        crate::diagnostics::trace::InterruptOutcome::Return,
    );
    let trace_exit = |outcome| {
        crate::diagnostics::trace::record_interrupt_boundary(
            crate::diagnostics::trace::EventKind::InterruptExit,
            14,
            previous_cpl,
            false,
            outcome,
        );
    };
    crate::diagnostics::trace::record(
        crate::diagnostics::trace::EventKind::PageFault,
        accessed_addr.as_u64() & !0xfff,
        error_code.bits(),
        stack_frame.instruction_pointer.as_u64(),
        0,
    );

    // Routine demand faults are expected and can occur thousands of times
    // during process startup. Keep per-fault detail available for an
    // explicitly selected Trace session, but do no UART I/O at the default
    // level. Fatal paths below log the address, error code, RIP, and context.
    // UART output from this interrupt path is especially expensive under
    // QEMU because every byte can require a VM exit.
    debug_trace!("page fault at {:?}, error: {:?}", accessed_addr, error_code);

    // Don't check for physical memory offset access here - let the mapper handle it
    // The mapper knows the actual physical memory offset from the bootloader

    // Check if the fault is in our heap region
    const HEAP_START: u64 = crate::mm::heap::HEAP_START as u64;
    const HEAP_END: u64 = HEAP_START + crate::mm::heap::HEAP_SIZE as u64;

    // Check if the fault is in our process stack region
    const STACK_REGION_START: u64 = 0x_5555_0000_0000;
    const STACK_REGION_END: u64 = STACK_REGION_START + (64 * 68 * 1024); // 64 stacks * 68KB each

    let addr = accessed_addr.as_u64();

    // Ring-3 page faults: first try the demand-grown stack hook. If
    // the fault is inside the active process's stack-grow window and
    // the per-process budget allows, `try_grow_user_stack` maps a
    // fresh page and we return so the CPU retries the instruction.
    // Everything else (overflow, budget exhaustion, lock contention,
    // map failure, or a fault outside the grow window) routes to
    // `cleanup_user_process` with vector 14 / SIGSEGV — the single
    // termination path is covered by the fault-recovery and follow-up launch
    // regression in the userland suite.
    if frame_is_user(stack_frame.code_segment as u64) {
        if error_code.contains(
            x86_64::structures::idt::PageFaultErrorCode::PROTECTION_VIOLATION
                | x86_64::structures::idt::PageFaultErrorCode::CAUSED_BY_WRITE,
        ) {
            let cow_target = crate::userland::lifecycle::with_current_group(|process| {
                let space = process.address_space.as_ref()?;
                let writable = space
                    .vmas()
                    .find(accessed_addr.as_u64())
                    .is_some_and(|vma| vma.prot.contains(crate::userland::vm::VmProt::WRITE));
                writable.then_some(space.l4_frame())
            });
            if let Some(l4) = cow_target {
                if let Some(outcome) = crate::mm::memory::with_memory_mapper(|mapper| {
                    mapper.resolve_cow(l4, accessed_addr)
                }) {
                    match outcome {
                        crate::mm::paging::CowOutcome::Copied
                        | crate::mm::paging::CowOutcome::Upgraded => {
                            trace_exit(crate::diagnostics::trace::InterruptOutcome::RecoveredCow);
                            return;
                        }
                        crate::mm::paging::CowOutcome::NotCow
                        | crate::mm::paging::CowOutcome::OutOfFrames => {}
                    }
                }
            }
        }
        let has_address_space = crate::userland::lifecycle::with_current_group(|process| {
            process.address_space.is_some()
        });
        if has_address_space
            && !error_code
                .contains(x86_64::structures::idt::PageFaultErrorCode::PROTECTION_VIOLATION)
            && crate::userland::usercopy::ensure_user_page(
                accessed_addr.as_u64(),
                error_code.contains(x86_64::structures::idt::PageFaultErrorCode::CAUSED_BY_WRITE),
            )
            .is_ok()
        {
            trace_exit(crate::diagnostics::trace::InterruptOutcome::RecoveredPageIn);
            return;
        }
        use crate::userland::lifecycle::{try_grow_user_stack, GrowOutcome};
        match try_grow_user_stack(accessed_addr) {
            GrowOutcome::Grew => {
                trace_exit(crate::diagnostics::trace::InterruptOutcome::RecoveredStackGrowth);
                return;
            }
            GrowOutcome::NotStackGrow
            | GrowOutcome::Overflow
            | GrowOutcome::BudgetExhausted
            | GrowOutcome::LockContended
            | GrowOutcome::MapFailed => {}
        }
        debug_error!("EXCEPTION: PAGE FAULT (ring 3)");
        cleanup_user_process(AbnormalExit {
            vector: 14,
            error_code: Some(error_code.bits()),
            fault_addr: Some(accessed_addr),
            fault_rip: stack_frame.instruction_pointer,
        });
    }

    if (addr >= HEAP_START && addr < HEAP_END)
        || (addr >= STACK_REGION_START && addr < STACK_REGION_END)
    {
        // This is a heap or stack access - allocate and map a page.
        // Per-fault trace logging only; routine demand-paging at default
        // log level shouldn't burn UART vmexits. See plan U2
        // (docs/plans/2026-05-09-002-perf-frame-allocator-and-page-fault-hot-path-plan.md).
        // Unexpected faults remain visible through the error-level failure
        // paths below; successful demand faults stay silent by default.
        let region = if addr >= STACK_REGION_START {
            "stack"
        } else {
            "heap"
        };
        debug_trace!("Page fault in {} region at {:?}", region, accessed_addr);

        // Try to handle the page fault
        if let Some(result) =
            crate::mm::memory::with_memory_mapper(|mapper| mapper.handle_page_fault(accessed_addr))
        {
            if result.is_err() {
                crate::diagnostics::crash::begin_trap(
                    "kernel-demand-map-failed",
                    14,
                    Some(error_code.bits()),
                    Some(accessed_addr.as_u64()),
                    stack_frame.instruction_pointer.as_u64(),
                );
            }
            // Successfully mapped - return and retry the instruction
            trace_exit(crate::diagnostics::trace::InterruptOutcome::RecoveredKernelDemand);
            return;
        }
    }

    // Not a heap fault or couldn't handle it: elect and serialize the crash
    // owner before touching any contended logger state.
    crate::diagnostics::crash::begin_trap(
        "unhandled-page-fault",
        14,
        Some(error_code.bits()),
        Some(accessed_addr.as_u64()),
        stack_frame.instruction_pointer.as_u64(),
    );
}

extern "x86-interrupt" fn double_fault_handler(stack_frame: InterruptStackFrame, _error_code: u64) {
    crate::diagnostics::trace::record_interrupt_boundary(
        crate::diagnostics::trace::EventKind::InterruptEntry,
        8,
        (stack_frame.code_segment as u64 & 3) as u8,
        false,
        crate::diagnostics::trace::InterruptOutcome::Return,
    );
    crate::diagnostics::crash::begin_trap(
        "double-fault",
        8,
        Some(0),
        None,
        stack_frame.instruction_pointer.as_u64(),
    );
}

extern "x86-interrupt" fn general_protection_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    route_user_fault_or_panic(
        &stack_frame,
        13,
        Some(error_code),
        None,
        "General protection fault",
    );
}

extern "x86-interrupt" fn invalid_opcode_handler(stack_frame: InterruptStackFrame) {
    route_user_fault_or_panic(&stack_frame, 6, None, None, "Invalid opcode");
}

extern "x86-interrupt" fn divide_error_handler(stack_frame: InterruptStackFrame) {
    route_user_fault_or_panic(&stack_frame, 0, None, None, "Divide error");
}

extern "x86-interrupt" fn overflow_handler(stack_frame: InterruptStackFrame) {
    route_user_fault_or_panic(&stack_frame, 4, None, None, "Overflow");
}

extern "x86-interrupt" fn bound_range_exceeded_handler(stack_frame: InterruptStackFrame) {
    route_user_fault_or_panic(&stack_frame, 5, None, None, "Bound range exceeded");
}

extern "x86-interrupt" fn invalid_tss_handler(stack_frame: InterruptStackFrame, error_code: u64) {
    route_user_fault_or_panic(&stack_frame, 10, Some(error_code), None, "Invalid TSS");
}

extern "x86-interrupt" fn segment_not_present_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    route_user_fault_or_panic(
        &stack_frame,
        11,
        Some(error_code),
        None,
        "Segment not present",
    );
}

extern "x86-interrupt" fn stack_segment_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    route_user_fault_or_panic(
        &stack_frame,
        12,
        Some(error_code),
        None,
        "Stack segment fault",
    );
}

extern "x86-interrupt" fn alignment_check_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    route_user_fault_or_panic(&stack_frame, 17, Some(error_code), None, "Alignment check");
}

// Hardware Interrupt Handlers

/// Timer tick counter (atomic for safe access)
pub static TIMER_TICKS: AtomicU64 = AtomicU64::new(0);

/// Get the current timer tick count
pub fn get_timer_ticks() -> u64 {
    TIMER_TICKS.load(Ordering::Relaxed)
}

extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use crate::input::{RawInputEvent, INPUT_QUEUE};
    use x86_64::instructions::port::Port;

    let mut port = Port::new(0x60);
    let scancode: u8 = unsafe { port.read() };

    // Enqueue to lock-free queue - never blocks, never drops (unless queue full)
    if !INPUT_QUEUE.push(RawInputEvent::KeyboardScancode(scancode)) {
        // Queue full - this should be rare with 256 entry buffer
        crate::debug_warn!("Input queue full, dropping scancode 0x{:02x}", scancode);
    }

    eoi(InterruptIndex::Keyboard.as_u8());
}

extern "x86-interrupt" fn mouse_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use crate::input::{RawInputEvent, INPUT_QUEUE};
    use x86_64::instructions::port::Port;

    let mut port = Port::new(0x60);
    let data: u8 = unsafe { port.read() };

    // Update the mouse driver state so get_state() returns current position
    crate::drivers::mouse::handle_interrupt(data);

    // Enqueue to lock-free queue - never blocks
    if !INPUT_QUEUE.push(RawInputEvent::MousePacketByte(data)) {
        // Queue full - this should be rare with 256 entry buffer
        crate::debug_warn!("Input queue full, dropping mouse byte 0x{:02x}", data);
    }

    eoi(InterruptIndex::Mouse.as_u8());
}

static LAPIC_TIMER_TICKS: [AtomicU64; crate::arch::x86_64::acpi::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::arch::x86_64::acpi::MAX_CPUS];

extern "x86-interrupt" fn reschedule_interrupt_handler(stack_frame: InterruptStackFrame) {
    let previous_cpl = (stack_frame.code_segment as u64 & 3) as u8;
    crate::diagnostics::trace::record_interrupt_boundary(
        crate::diagnostics::trace::EventKind::InterruptEntry,
        crate::arch::x86_64::lapic::RESCHEDULE_VECTOR,
        previous_cpl,
        false,
        crate::diagnostics::trace::InterruptOutcome::Return,
    );
    crate::arch::x86_64::lapic::eoi();
    crate::userland::syscalls::maybe_terminate_pending_fatal_signal();
    crate::diagnostics::trace::record_interrupt_boundary(
        crate::diagnostics::trace::EventKind::InterruptExit,
        crate::arch::x86_64::lapic::RESCHEDULE_VECTOR,
        previous_cpl,
        true,
        crate::diagnostics::trace::InterruptOutcome::Return,
    );
}

extern "x86-interrupt" fn halt_interrupt_handler(_stack_frame: InterruptStackFrame) {
    crate::arch::x86_64::lapic::eoi();
    x86_64::instructions::interrupts::disable();
    loop {
        x86_64::instructions::hlt();
    }
}

extern "x86-interrupt" fn lapic_error_interrupt_handler(_stack_frame: InterruptStackFrame) {
    crate::debug_warn!(
        "local APIC error interrupt on CPU {}",
        crate::arch::x86_64::percpu::cpu_id()
    );
    crate::arch::x86_64::lapic::eoi();
}

extern "x86-interrupt" fn lapic_spurious_interrupt_handler(_stack_frame: InterruptStackFrame) {
    // Intel specifies that spurious interrupts do not receive an EOI.
}

#[cfg_attr(not(feature = "test"), expect(dead_code, reason = "SMP telemetry API"))]
pub fn lapic_timer_ticks(cpu: usize) -> u64 {
    LAPIC_TIMER_TICKS
        .get(cpu)
        .map(|ticks| ticks.load(Ordering::Relaxed))
        .unwrap_or(0)
}

pub fn record_lapic_timer_tick(cpu: usize) {
    if let Some(ticks) = LAPIC_TIMER_TICKS.get(cpu) {
        ticks.fetch_add(1, Ordering::Relaxed);
    }
}

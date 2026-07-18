use crate::debug_info;
#[cfg(feature = "test")]
use crate::lib::test_utils::Testable;

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_breakpoint_interrupt,
        &test_preemption_guard_preserves_interrupt_state,
        &test_preemption_guard_nests,
        &test_preemption_mutex_failed_try_lock_restores_depth,
        // Note: We can only safely test the breakpoint interrupt
        // Other interrupts that panic would terminate the test runner
    ]
}

fn test_breakpoint_interrupt() {
    debug_info!("Testing breakpoint interrupt...");

    // The breakpoint handler in interrupts.rs doesn't panic, so we can test it directly
    // Trigger breakpoint interrupt using int3 instruction
    unsafe {
        core::arch::asm!("int3");
    }

    // If we reach here without panicking, the test passes
    debug_info!("Breakpoint interrupt handled successfully");
}

fn test_preemption_guard_preserves_interrupt_state() {
    use crate::arch::x86_64::preemption_guard::{kernel_preemption_allowed, PreemptionGuard};

    let interrupts_were_enabled = x86_64::instructions::interrupts::are_enabled();
    assert!(kernel_preemption_allowed());
    {
        let _guard = PreemptionGuard::disable();
        assert!(!kernel_preemption_allowed());
        assert_eq!(
            x86_64::instructions::interrupts::are_enabled(),
            interrupts_were_enabled,
            "preemption guard must not change IF"
        );
    }
    assert!(kernel_preemption_allowed());
    assert_eq!(
        x86_64::instructions::interrupts::are_enabled(),
        interrupts_were_enabled
    );
}

fn test_preemption_guard_nests() {
    use crate::arch::x86_64::preemption_guard::{kernel_preemption_allowed, PreemptionGuard};

    assert!(kernel_preemption_allowed());
    let outer = PreemptionGuard::disable();
    assert!(!kernel_preemption_allowed());
    {
        let _inner = PreemptionGuard::disable();
        assert!(!kernel_preemption_allowed());
    }
    assert!(
        !kernel_preemption_allowed(),
        "dropping an inner guard must not enable preemption"
    );
    drop(outer);
    assert!(kernel_preemption_allowed());
}

fn test_preemption_mutex_failed_try_lock_restores_depth() {
    use crate::arch::x86_64::preemption_guard::{kernel_preemption_allowed, PreemptionMutex};

    let mutex = PreemptionMutex::new(7u8);
    assert!(kernel_preemption_allowed());
    let guard = mutex.lock();
    assert!(!kernel_preemption_allowed());
    assert!(mutex.try_lock().is_none());
    assert!(
        !kernel_preemption_allowed(),
        "failed try_lock must restore the outer guard's nesting depth"
    );
    assert_eq!(*guard, 7);
    drop(guard);
    assert!(kernel_preemption_allowed());

    let reacquired = mutex
        .try_lock()
        .expect("mutex must unlock before preemption becomes eligible");
    assert_eq!(*reacquired, 7);
    drop(reacquired);
    assert!(kernel_preemption_allowed());
}

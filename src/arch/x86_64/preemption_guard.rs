//! Nesting-safe preemption guards for long kernel critical sections.
//!
//! Unlike [`super::interrupt_guard::InterruptGuard`], these guards leave
//! hardware interrupts enabled. The PIT can therefore keep monotonic time and
//! device IRQs can continue to enqueue work; the timer handler defers scheduler
//! housekeeping and kernel-thread context switches until the critical section
//! ends.
//!
//! The depth is global because AgenticOS currently has one CPU. An SMP port
//! must move it into per-CPU state before using this primitive on more than one
//! processor.

use core::sync::atomic::{AtomicUsize, Ordering};

static PREEMPTION_DISABLE_DEPTH: AtomicUsize = AtomicUsize::new(0);

/// RAII guard that prevents timer-driven kernel scheduler work.
///
/// Hardware interrupts remain in their existing state. Guards may be nested;
/// preemption becomes eligible again only after the outermost guard drops.
pub struct PreemptionGuard {
    _private: (),
}

impl PreemptionGuard {
    #[inline]
    pub fn disable() -> Self {
        let previous = PREEMPTION_DISABLE_DEPTH.fetch_add(1, Ordering::AcqRel);
        debug_assert_ne!(previous, usize::MAX, "preemption guard depth overflow");
        Self { _private: () }
    }
}

impl Drop for PreemptionGuard {
    #[inline]
    fn drop(&mut self) {
        let previous = PREEMPTION_DISABLE_DEPTH.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "unbalanced preemption guard drop");
    }
}

/// Whether the current CPU is inside a preemption-disabled critical section.
#[inline]
pub fn preemption_disabled() -> bool {
    PREEMPTION_DISABLE_DEPTH.load(Ordering::Acquire) != 0
}

/// Whether the timer handler may run scheduler work for the current kernel thread.
#[inline]
pub fn kernel_preemption_allowed() -> bool {
    !preemption_disabled()
}

/// A spin mutex that prevents scheduler preemption without masking IRQs.
///
/// This is appropriate only for thread-context state that interrupt handlers
/// never acquire directly. On the single CPU, acquiring the preemption guard
/// before attempting the spin lock prevents another kernel thread from being
/// scheduled while the lock is held.
pub struct PreemptionMutex<T> {
    inner: spin::Mutex<T>,
}

/// Guard returned by [`PreemptionMutex`]. Field order is load-bearing: Rust
/// drops fields in declaration order, so the mutex unlocks before preemption
/// becomes eligible again.
pub struct PreemptionMutexGuard<'a, T> {
    inner: spin::MutexGuard<'a, T>,
    _preemption_guard: PreemptionGuard,
}

impl<T> PreemptionMutex<T> {
    pub const fn new(value: T) -> Self {
        Self {
            inner: spin::Mutex::new(value),
        }
    }

    pub fn lock(&self) -> PreemptionMutexGuard<'_, T> {
        let preemption_guard = PreemptionGuard::disable();
        let inner = self.inner.lock();
        PreemptionMutexGuard {
            inner,
            _preemption_guard: preemption_guard,
        }
    }

    #[cfg_attr(
        not(feature = "test"),
        expect(dead_code, reason = "test coverage and future non-blocking callers")
    )]
    pub fn try_lock(&self) -> Option<PreemptionMutexGuard<'_, T>> {
        let preemption_guard = PreemptionGuard::disable();
        let inner = self.inner.try_lock()?;
        Some(PreemptionMutexGuard {
            inner,
            _preemption_guard: preemption_guard,
        })
    }
}

impl<T> core::ops::Deref for PreemptionMutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> core::ops::DerefMut for PreemptionMutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

//! Nesting-safe preemption guards for long kernel critical sections.
//!
//! Unlike [`super::interrupt_guard::InterruptGuard`], these guards leave
//! hardware interrupts enabled. The PIT can therefore keep monotonic time and
//! device IRQs can continue to enqueue work; the timer handler defers scheduler
//! housekeeping and kernel-thread context switches until the critical section
//! ends.
//!
//! The nesting depth lives in per-CPU state so independent processors can
//! enter protected regions concurrently.

use core::sync::atomic::Ordering;

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
        let previous =
            crate::arch::x86_64::percpu::preemption_depth().fetch_add(1, Ordering::AcqRel);
        debug_assert_ne!(previous, usize::MAX, "preemption guard depth overflow");
        Self { _private: () }
    }
}

impl Drop for PreemptionGuard {
    #[inline]
    fn drop(&mut self) {
        let previous =
            crate::arch::x86_64::percpu::preemption_depth().fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "unbalanced preemption guard drop");
    }
}

/// Whether the current CPU is inside a preemption-disabled critical section.
#[inline]
pub fn preemption_disabled() -> bool {
    crate::arch::x86_64::percpu::preemption_depth().load(Ordering::Acquire) != 0
}

/// Whether the timer handler may run scheduler work for the current kernel thread.
#[inline]
pub fn kernel_preemption_allowed() -> bool {
    !preemption_disabled()
}

/// A spin mutex that prevents scheduler preemption without masking IRQs.
///
/// This is appropriate only for thread-context state that interrupt handlers
/// never acquire directly. Acquiring the per-CPU preemption guard before
/// attempting the spin lock prevents the owning kernel thread from being
/// switched out while the lock is held.
pub struct PreemptionMutex<T> {
    inner: spin::Mutex<T>,
    class: crate::diagnostics::shadow::locks::LockClassId,
}

/// Guard returned by [`PreemptionMutex`]. Its explicit drop order publishes
/// shadow release, unlocks the mutex, and only then lets preemption become
/// eligible again.
pub struct PreemptionMutexGuard<'a, T> {
    inner: core::mem::ManuallyDrop<spin::MutexGuard<'a, T>>,
    class: crate::diagnostics::shadow::locks::LockClassId,
    _preemption_guard: PreemptionGuard,
}

impl<T> PreemptionMutex<T> {
    pub const fn new(value: T) -> Self {
        Self {
            inner: spin::Mutex::new(value),
            class: crate::diagnostics::shadow::locks::LockClassId::Untracked,
        }
    }

    pub const fn new_tracked(
        value: T,
        class: crate::diagnostics::shadow::locks::LockClassId,
    ) -> Self {
        Self {
            inner: spin::Mutex::new(value),
            class,
        }
    }

    #[track_caller]
    pub fn lock(&self) -> PreemptionMutexGuard<'_, T> {
        let preemption_guard = PreemptionGuard::disable();
        let site = crate::diagnostics::shadow::locks::site_id(core::panic::Location::caller());
        crate::diagnostics::shadow::locks::before_acquire(
            self.class,
            crate::diagnostics::shadow::locks::LockKind::Preemption,
            site,
        );
        let inner = self.inner.lock();
        crate::diagnostics::shadow::locks::acquired(
            self.class,
            crate::diagnostics::shadow::locks::LockKind::Preemption,
            site,
        );
        PreemptionMutexGuard {
            inner: core::mem::ManuallyDrop::new(inner),
            class: self.class,
            _preemption_guard: preemption_guard,
        }
    }

    /// Whether the underlying spin mutex is currently held.
    ///
    /// This is intended for debug lock-order assertions only. The value can
    /// change immediately after it is observed, so callers must not use it to
    /// make synchronization decisions.
    #[cfg_attr(
        not(feature = "test"),
        expect(dead_code, reason = "debug lock-order assertion helper")
    )]
    pub fn is_locked(&self) -> bool {
        self.inner.is_locked()
    }

    #[cfg_attr(
        not(feature = "test"),
        expect(dead_code, reason = "test coverage and future non-blocking callers")
    )]
    #[track_caller]
    pub fn try_lock(&self) -> Option<PreemptionMutexGuard<'_, T>> {
        let preemption_guard = PreemptionGuard::disable();
        let site = crate::diagnostics::shadow::locks::site_id(core::panic::Location::caller());
        crate::diagnostics::shadow::locks::before_acquire(
            self.class,
            crate::diagnostics::shadow::locks::LockKind::Preemption,
            site,
        );
        let Some(inner) = self.inner.try_lock() else {
            crate::diagnostics::shadow::locks::failed_try(self.class);
            return None;
        };
        crate::diagnostics::shadow::locks::acquired(
            self.class,
            crate::diagnostics::shadow::locks::LockKind::Preemption,
            site,
        );
        Some(PreemptionMutexGuard {
            inner: core::mem::ManuallyDrop::new(inner),
            class: self.class,
            _preemption_guard: preemption_guard,
        })
    }
}

impl<T> Drop for PreemptionMutexGuard<'_, T> {
    fn drop(&mut self) {
        crate::diagnostics::shadow::locks::released(self.class);
        unsafe { core::mem::ManuallyDrop::drop(&mut self.inner) };
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

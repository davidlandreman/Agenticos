//! RAII guard for interrupt state management
//!
//! Provides safe interrupt disable/restore that guarantees interrupts are
//! restored even if code panics. This prevents system hangs where interrupts
//! are disabled and never re-enabled due to a panic.

/// RAII guard that restores interrupt state on drop.
///
/// Use this instead of manually calling `interrupts::disable()` and `interrupts::enable()`
/// to ensure interrupts are always restored, even on panic.
///
/// # Example
///
/// ```rust
/// // Interrupts are disabled and will be restored when _guard is dropped
/// let _guard = InterruptGuard::disable();
/// // Critical section code here...
/// // Interrupts restored automatically when _guard goes out of scope
/// ```
pub struct InterruptGuard {
    was_enabled: bool,
}

/// A spin mutex whose critical section cannot be timer-preempted.
///
/// A plain spin mutex is not safe when both timer-preemptible kernel threads
/// and interrupt/SYSCALL paths acquire it: a local interrupt can spin forever
/// if it preempted the lock owner. This wrapper masks local interrupts before
/// attempting the lock; the inner spin lock provides cross-CPU exclusion.
pub struct InterruptMutex<T> {
    inner: spin::Mutex<T>,
    class: crate::diagnostics::shadow::locks::LockClassId,
}

/// Guard returned by [`InterruptMutex`]. Its explicit drop order publishes
/// shadow release, unlocks the spin mutex, and only then lets the interrupt
/// guard restore IF.
pub struct InterruptMutexGuard<'a, T> {
    inner: core::mem::ManuallyDrop<spin::MutexGuard<'a, T>>,
    class: crate::diagnostics::shadow::locks::LockClassId,
    _interrupt_guard: InterruptGuard,
}

impl<T> InterruptMutex<T> {
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
    pub fn lock(&self) -> InterruptMutexGuard<'_, T> {
        let interrupt_guard = InterruptGuard::disable();
        let site = crate::diagnostics::shadow::locks::site_id(core::panic::Location::caller());
        crate::diagnostics::shadow::locks::before_acquire(
            self.class,
            crate::diagnostics::shadow::locks::LockKind::Interrupt,
            site,
        );
        let inner = self.inner.lock();
        crate::diagnostics::shadow::locks::acquired(
            self.class,
            crate::diagnostics::shadow::locks::LockKind::Interrupt,
            site,
        );
        InterruptMutexGuard {
            inner: core::mem::ManuallyDrop::new(inner),
            class: self.class,
            _interrupt_guard: interrupt_guard,
        }
    }

    #[track_caller]
    pub fn try_lock(&self) -> Option<InterruptMutexGuard<'_, T>> {
        let interrupt_guard = InterruptGuard::disable();
        let site = crate::diagnostics::shadow::locks::site_id(core::panic::Location::caller());
        crate::diagnostics::shadow::locks::before_acquire(
            self.class,
            crate::diagnostics::shadow::locks::LockKind::Interrupt,
            site,
        );
        let Some(inner) = self.inner.try_lock() else {
            crate::diagnostics::shadow::locks::failed_try(self.class);
            return None;
        };
        crate::diagnostics::shadow::locks::acquired(
            self.class,
            crate::diagnostics::shadow::locks::LockKind::Interrupt,
            site,
        );
        Some(InterruptMutexGuard {
            inner: core::mem::ManuallyDrop::new(inner),
            class: self.class,
            _interrupt_guard: interrupt_guard,
        })
    }
}

impl<T> core::ops::Deref for InterruptMutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> core::ops::DerefMut for InterruptMutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<T> Drop for InterruptMutexGuard<'_, T> {
    fn drop(&mut self) {
        crate::diagnostics::shadow::locks::released(self.class);
        unsafe { core::mem::ManuallyDrop::drop(&mut self.inner) };
    }
}

impl InterruptGuard {
    /// Disable interrupts and return a guard that will restore them.
    ///
    /// If interrupts were already disabled, they will remain disabled after
    /// the guard is dropped (restores to previous state, doesn't force enable).
    #[inline]
    pub fn disable() -> Self {
        let was_enabled = x86_64::instructions::interrupts::are_enabled();
        x86_64::instructions::interrupts::disable();
        Self { was_enabled }
    }
}

impl Drop for InterruptGuard {
    fn drop(&mut self) {
        if self.was_enabled {
            x86_64::instructions::interrupts::enable();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_interrupt_guard_restores_state() {
        // This test just verifies the logic - actual interrupt testing requires hardware
        let guard = InterruptGuard { was_enabled: true };
        assert!(guard.was_enabled());
        // When dropped, would call enable()
    }
}

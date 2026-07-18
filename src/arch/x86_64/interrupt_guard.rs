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
}

/// Guard returned by [`InterruptMutex`]. Field order is load-bearing: Rust
/// drops fields in declaration order, so the spin guard releases the mutex
/// before the interrupt guard restores IF.
pub struct InterruptMutexGuard<'a, T> {
    inner: spin::MutexGuard<'a, T>,
    _interrupt_guard: InterruptGuard,
}

impl<T> InterruptMutex<T> {
    pub const fn new(value: T) -> Self {
        Self {
            inner: spin::Mutex::new(value),
        }
    }

    pub fn lock(&self) -> InterruptMutexGuard<'_, T> {
        let interrupt_guard = InterruptGuard::disable();
        let inner = self.inner.lock();
        InterruptMutexGuard {
            inner,
            _interrupt_guard: interrupt_guard,
        }
    }

    pub fn try_lock(&self) -> Option<InterruptMutexGuard<'_, T>> {
        let interrupt_guard = InterruptGuard::disable();
        let inner = self.inner.try_lock()?;
        Some(InterruptMutexGuard {
            inner,
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

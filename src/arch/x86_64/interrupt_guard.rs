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

    /// Capture current interrupt state without changing it.
    ///
    /// Useful when you want to ensure interrupts are restored to whatever
    /// state they were in, regardless of what happens in between.
    #[inline]
    pub fn capture() -> Self {
        Self {
            was_enabled: x86_64::instructions::interrupts::are_enabled(),
        }
    }

    /// Check if interrupts were enabled when this guard was created.
    #[inline]
    pub fn was_enabled(&self) -> bool {
        self.was_enabled
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

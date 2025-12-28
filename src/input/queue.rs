//! Lock-free Single-Producer Single-Consumer (SPSC) ring buffer for input events.
//!
//! This queue is designed for interrupt-safe event handling:
//! - Producer: Interrupt handlers (keyboard IRQ1, mouse IRQ12)
//! - Consumer: Main kernel loop
//!
//! The lock-free design ensures interrupt handlers never block, eliminating
//! the dropped keyboard events that occurred with the previous try_lock() approach.

use core::sync::atomic::{AtomicUsize, Ordering};

/// Size of the event queue (must be power of 2 for efficient modulo)
pub const QUEUE_SIZE: usize = 256;

/// Raw input events from hardware before processing
#[derive(Clone, Copy, Debug)]
pub enum RawInputEvent {
    /// Raw keyboard scancode from PS/2 port
    KeyboardScancode(u8),
    /// Raw mouse packet byte from PS/2 port
    MousePacketByte(u8),
}

impl Default for RawInputEvent {
    fn default() -> Self {
        RawInputEvent::KeyboardScancode(0)
    }
}

/// Lock-free SPSC ring buffer for input events.
///
/// This is safe for single-producer (interrupt) / single-consumer (main loop)
/// use without any locking. The atomic operations ensure proper synchronization.
///
/// # Memory Ordering
/// - Producer uses Release ordering when updating head (makes writes visible)
/// - Consumer uses Acquire ordering when reading head (sees producer's writes)
/// - Consumer uses Release ordering when updating tail (makes reads complete)
/// - Producer uses Acquire ordering when reading tail (sees consumer's progress)
pub struct InputQueue {
    /// Ring buffer storage
    buffer: [RawInputEvent; QUEUE_SIZE],
    /// Write position (updated by producer/interrupt)
    head: AtomicUsize,
    /// Read position (updated by consumer/main loop)
    tail: AtomicUsize,
    /// Count of dropped events (for diagnostics)
    dropped: AtomicUsize,
}

impl InputQueue {
    /// Create a new empty input queue.
    ///
    /// This is a const fn so it can be used for static initialization.
    pub const fn new() -> Self {
        // Initialize buffer with default values
        // Note: We can't use array::from_fn in const context, so we use a workaround
        Self {
            buffer: [RawInputEvent::KeyboardScancode(0); QUEUE_SIZE],
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            dropped: AtomicUsize::new(0),
        }
    }

    /// Push an event from interrupt context.
    ///
    /// This method is designed to be called from interrupt handlers.
    /// It never blocks and is safe to call with interrupts disabled.
    ///
    /// Returns `true` if the event was queued, `false` if the queue was full.
    /// When the queue is full, the event is dropped and the dropped counter
    /// is incremented.
    ///
    /// # Safety
    /// This is safe because:
    /// - Only one producer (interrupts are disabled during ISR)
    /// - Atomic operations ensure visibility to consumer
    /// - We only write to slots that consumer has finished reading
    #[inline]
    pub fn push(&self, event: RawInputEvent) -> bool {
        let head = self.head.load(Ordering::Relaxed);
        let next_head = (head + 1) & (QUEUE_SIZE - 1); // Fast modulo for power of 2

        // Check if queue is full
        let tail = self.tail.load(Ordering::Acquire);
        if next_head == tail {
            // Queue full - increment dropped counter
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        }

        // Write the event
        // SAFETY: We are the only writer (interrupts disabled during ISR)
        // and we haven't updated head yet, so consumer won't read this slot.
        // We need to use a raw pointer because self is &self not &mut self.
        unsafe {
            let ptr = self.buffer.as_ptr() as *mut RawInputEvent;
            ptr.add(head).write(event);
        }

        // Make the write visible to consumer
        self.head.store(next_head, Ordering::Release);
        true
    }

    /// Pop an event from the main loop.
    ///
    /// Returns `Some(event)` if an event is available, `None` if the queue is empty.
    ///
    /// This method should only be called from the main kernel loop (single consumer).
    #[inline]
    pub fn pop(&self) -> Option<RawInputEvent> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);

        if tail == head {
            // Queue empty
            return None;
        }

        // Read the event
        // SAFETY: Producer has written to this slot and updated head
        let event = unsafe {
            let ptr = self.buffer.as_ptr();
            ptr.add(tail).read()
        };

        // Update tail to mark slot as available
        let next_tail = (tail + 1) & (QUEUE_SIZE - 1);
        self.tail.store(next_tail, Ordering::Release);

        Some(event)
    }

    /// Check if the queue is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.head.load(Ordering::Acquire) == self.tail.load(Ordering::Acquire)
    }

    /// Get the number of events currently in the queue.
    #[inline]
    pub fn len(&self) -> usize {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        (head.wrapping_sub(tail)) & (QUEUE_SIZE - 1)
    }

    /// Get the number of events that have been dropped due to queue overflow.
    #[inline]
    pub fn dropped_count(&self) -> usize {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Reset the dropped counter (useful after logging).
    #[inline]
    pub fn reset_dropped_count(&self) {
        self.dropped.store(0, Ordering::Relaxed);
    }
}

// SAFETY: InputQueue uses atomic operations for all shared state
unsafe impl Sync for InputQueue {}
unsafe impl Send for InputQueue {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_queue_empty() {
        let queue = InputQueue::new();
        assert!(queue.is_empty());
        assert_eq!(queue.len(), 0);
        assert!(queue.pop().is_none());
    }

    #[test_case]
    fn test_queue_push_pop() {
        let queue = InputQueue::new();

        assert!(queue.push(RawInputEvent::KeyboardScancode(0x1C)));
        assert!(!queue.is_empty());
        assert_eq!(queue.len(), 1);

        let event = queue.pop();
        assert!(matches!(event, Some(RawInputEvent::KeyboardScancode(0x1C))));
        assert!(queue.is_empty());
    }

    #[test_case]
    fn test_queue_multiple() {
        let queue = InputQueue::new();

        // Push multiple events
        for i in 0..10u8 {
            assert!(queue.push(RawInputEvent::KeyboardScancode(i)));
        }
        assert_eq!(queue.len(), 10);

        // Pop them in order (FIFO)
        for i in 0..10u8 {
            let event = queue.pop();
            assert!(matches!(event, Some(RawInputEvent::KeyboardScancode(x)) if x == i));
        }
        assert!(queue.is_empty());
    }

    #[test_case]
    fn test_queue_overflow() {
        let queue = InputQueue::new();

        // Fill the queue (capacity is QUEUE_SIZE - 1 due to ring buffer design)
        for i in 0..(QUEUE_SIZE - 1) {
            assert!(queue.push(RawInputEvent::KeyboardScancode(i as u8)));
        }

        // Next push should fail
        assert!(!queue.push(RawInputEvent::KeyboardScancode(0xFF)));
        assert_eq!(queue.dropped_count(), 1);

        // Pop one and try again
        queue.pop();
        assert!(queue.push(RawInputEvent::KeyboardScancode(0xFF)));
    }
}

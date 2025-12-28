//! Input handling subsystem for AgenticOS.
//!
//! This module provides a clean, interrupt-safe input handling architecture:
//!
//! 1. **Hardware Layer**: Interrupt handlers push raw events to a lock-free queue
//! 2. **Processing Layer**: `InputProcessor` converts raw events to typed events
//! 3. **Event Layer**: Typed events (KeyboardEvent, MouseEvent) for the window system
//!
//! # Architecture
//!
//! ```text
//! Hardware Interrupts (IRQ1, IRQ12)
//!          │
//!          ▼
//! ┌─────────────────┐
//! │   InputQueue    │  Lock-free SPSC ring buffer
//! │  (queue.rs)     │  - Never blocks in interrupt context
//! └────────┬────────┘
//!          │
//!          ▼
//! ┌─────────────────┐
//! │ InputProcessor  │  Converts raw → typed events
//! │  (this module)  │  - KeyboardDriver for scancodes
//! └────────┬────────┘  - MouseDriver for packets
//!          │
//!          ▼
//! ┌─────────────────┐
//! │  Window System  │  Routes events to windows
//! └─────────────────┘
//! ```
//!
//! # Usage
//!
//! ```ignore
//! // In kernel main loop:
//! let mut input_processor = InputProcessor::new(1280, 720);
//!
//! loop {
//!     // Process all pending input
//!     for event in input_processor.process_pending(&INPUT_QUEUE) {
//!         window_manager.handle_event(event);
//!     }
//!
//!     // ... rest of loop
//! }
//! ```

pub mod keyboard_driver;
pub mod mouse_driver;
pub mod queue;

pub use keyboard_driver::{keycode_to_char, KeyboardDriver};
pub use mouse_driver::MouseDriver;
pub use queue::{InputQueue, RawInputEvent, QUEUE_SIZE};

use crate::window::event::{Event, KeyModifiers, MouseButtons, MouseEvent, MouseEventType};
use crate::window::types::Point;

/// Global input event queue.
///
/// This queue is written to by interrupt handlers and read by the main loop.
/// It's safe to access from both contexts because it uses lock-free atomics.
///
/// Capacity: 256 events (~4 seconds of buffering at 60 events/sec)
pub static INPUT_QUEUE: InputQueue = InputQueue::new();

/// Central input processor that converts raw input events to typed events.
///
/// The `InputProcessor` maintains state machines for keyboard and mouse input,
/// handling multi-byte sequences and generating appropriate events.
///
/// Also supports VirtIO tablet for seamless mouse integration in QEMU.
pub struct InputProcessor {
    keyboard: KeyboardDriver,
    mouse: MouseDriver,
    /// Last known VirtIO tablet state (position, buttons)
    virtio_last_state: Option<(i32, i32, u8)>,
    /// Whether VirtIO tablet is active
    using_virtio: bool,
}

impl InputProcessor {
    /// Create a new input processor with the given screen dimensions.
    ///
    /// Screen dimensions are used for mouse position clamping.
    pub fn new(screen_width: i32, screen_height: i32) -> Self {
        let using_virtio = crate::drivers::mouse::is_virtio_tablet();
        Self {
            keyboard: KeyboardDriver::new(),
            mouse: MouseDriver::new(screen_width, screen_height),
            virtio_last_state: None,
            using_virtio,
        }
    }

    /// Create a new input processor with default 1280x720 screen.
    pub fn new_default() -> Self {
        Self::new(1280, 720)
    }

    /// Process all pending events from the input queue.
    ///
    /// Returns an iterator that yields typed events (KeyboardEvent, MouseEvent).
    /// This consumes events from the queue as they're processed.
    ///
    /// # Example
    /// ```ignore
    /// for event in input_processor.process_pending(&INPUT_QUEUE) {
    ///     match event {
    ///         Event::Keyboard(kb) => handle_keyboard(kb),
    ///         Event::Mouse(ms) => handle_mouse(ms),
    ///         _ => {}
    ///     }
    /// }
    /// ```
    pub fn process_pending<'a>(&'a mut self, queue: &'a InputQueue) -> ProcessIterator<'a> {
        ProcessIterator {
            processor: self,
            queue,
        }
    }

    /// Process a single raw input event and optionally generate a typed event.
    fn process_raw(&mut self, raw: RawInputEvent) -> Option<Event> {
        match raw {
            RawInputEvent::KeyboardScancode(scancode) => self
                .keyboard
                .process_scancode(scancode)
                .map(Event::Keyboard),
            RawInputEvent::MousePacketByte(byte) => {
                // Skip PS/2 mouse events if using VirtIO tablet
                if self.using_virtio {
                    return None;
                }
                self.mouse.process_byte(byte).map(Event::Mouse)
            }
        }
    }

    /// Check VirtIO tablet for state changes and generate mouse event if needed.
    ///
    /// This should be called from the main loop when using VirtIO tablet.
    /// Returns a MouseEvent if the tablet state changed.
    pub fn check_virtio_tablet(&mut self) -> Option<Event> {
        if !self.using_virtio {
            return None;
        }

        // Get current VirtIO tablet state
        let (x, y, buttons) = crate::drivers::mouse::get_state();

        // Check if state changed
        let changed = match self.virtio_last_state {
            Some((lx, ly, lb)) => x != lx || y != ly || buttons != lb,
            None => true, // First time - consider it changed
        };

        if !changed {
            return None;
        }

        // Determine event type
        let old_buttons = self.virtio_last_state.map(|(_, _, b)| b).unwrap_or(0);
        let old_pos = self.virtio_last_state.map(|(x, y, _)| (x, y));

        // Update last state
        self.virtio_last_state = Some((x, y, buttons));

        // Decode button states
        let mouse_buttons = MouseButtons {
            left: (buttons & 0x01) != 0,
            right: (buttons & 0x02) != 0,
            middle: (buttons & 0x04) != 0,
        };

        let old_mouse_buttons = MouseButtons {
            left: (old_buttons & 0x01) != 0,
            right: (old_buttons & 0x02) != 0,
            middle: (old_buttons & 0x04) != 0,
        };

        // Determine event type based on what changed
        let button_pressed = (mouse_buttons.left && !old_mouse_buttons.left)
            || (mouse_buttons.right && !old_mouse_buttons.right)
            || (mouse_buttons.middle && !old_mouse_buttons.middle);

        let button_released = (!mouse_buttons.left && old_mouse_buttons.left)
            || (!mouse_buttons.right && old_mouse_buttons.right)
            || (!mouse_buttons.middle && old_mouse_buttons.middle);

        let event_type = if button_pressed {
            MouseEventType::ButtonDown
        } else if button_released {
            MouseEventType::ButtonUp
        } else if old_pos.map(|(ox, oy)| x != ox || y != oy).unwrap_or(true) {
            MouseEventType::Move
        } else {
            return None;
        };

        let pos = Point::new(x, y);
        Some(Event::Mouse(MouseEvent {
            event_type,
            position: pos,
            global_position: pos,
            buttons: mouse_buttons,
        }))
    }

    /// Get current mouse position.
    #[inline]
    pub fn mouse_position(&self) -> (i32, i32) {
        self.mouse.position()
    }

    /// Get current keyboard modifier state.
    #[inline]
    pub fn current_modifiers(&self) -> KeyModifiers {
        self.keyboard.current_modifiers()
    }

    /// Update screen dimensions for mouse clamping.
    pub fn set_screen_bounds(&mut self, width: i32, height: i32) {
        self.mouse.set_screen_bounds(width, height);
    }
}

impl Default for InputProcessor {
    fn default() -> Self {
        Self::new_default()
    }
}

/// Iterator that processes raw events from the queue and yields typed events.
pub struct ProcessIterator<'a> {
    processor: &'a mut InputProcessor,
    queue: &'a InputQueue,
}

impl<'a> Iterator for ProcessIterator<'a> {
    type Item = Event;

    fn next(&mut self) -> Option<Event> {
        // Keep processing raw events until we get a typed event
        // (some scancodes like 0xF0 prefix don't produce events)
        loop {
            let raw = self.queue.pop()?;
            if let Some(event) = self.processor.process_raw(raw) {
                return Some(event);
            }
            // Continue to next raw event if this one didn't produce a typed event
        }
    }
}

/// Check if there are any pending input events.
#[inline]
pub fn has_pending_input() -> bool {
    !INPUT_QUEUE.is_empty()
}

/// Get the number of dropped events since last reset.
#[inline]
pub fn dropped_event_count() -> usize {
    INPUT_QUEUE.dropped_count()
}

/// Reset the dropped event counter.
#[inline]
pub fn reset_dropped_count() {
    INPUT_QUEUE.reset_dropped_count();
}

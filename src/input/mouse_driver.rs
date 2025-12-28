//! Mouse driver for PS/2 mouse packet processing.
//!
//! This module handles:
//! - Reassembling 3-byte PS/2 mouse packets
//! - Converting raw packets to MouseEvent types
//! - Tracking mouse position with screen bounds clamping
//! - Button state change detection

use crate::window::event::{MouseButtons, MouseEvent, MouseEventType};
use crate::window::types::Point;

/// Mouse driver state machine for processing PS/2 packets.
///
/// PS/2 mice send data in 3-byte packets:
/// - Byte 1: Buttons + overflow/sign bits
/// - Byte 2: X movement delta
/// - Byte 3: Y movement delta
#[derive(Debug)]
pub struct MouseDriver {
    /// Buffer for accumulating packet bytes
    packet_buffer: [u8; 3],
    /// Current position in packet (0-2)
    packet_index: usize,
    /// Current mouse position (clamped to screen)
    position: (i32, i32),
    /// Current button state
    buttons: MouseButtons,
    /// Screen bounds for clamping (width-1, height-1)
    screen_bounds: (i32, i32),
}

impl MouseDriver {
    /// Create a new mouse driver with given screen dimensions.
    ///
    /// Mouse position starts at the center of the screen.
    pub fn new(screen_width: i32, screen_height: i32) -> Self {
        Self {
            packet_buffer: [0; 3],
            packet_index: 0,
            position: (screen_width / 2, screen_height / 2),
            buttons: MouseButtons::default(),
            screen_bounds: (screen_width - 1, screen_height - 1),
        }
    }

    /// Create a new mouse driver with default 1280x720 screen dimensions.
    pub fn new_default() -> Self {
        Self::new(1280, 720)
    }

    /// Update screen dimensions (for when display mode changes).
    pub fn set_screen_bounds(&mut self, width: i32, height: i32) {
        self.screen_bounds = (width - 1, height - 1);
        // Clamp current position to new bounds
        self.position.0 = self.position.0.clamp(0, self.screen_bounds.0);
        self.position.1 = self.position.1.clamp(0, self.screen_bounds.1);
    }

    /// Process a raw mouse packet byte.
    ///
    /// Returns `Some(MouseEvent)` when a complete 3-byte packet produces an event,
    /// or `None` if more bytes are needed or no change occurred.
    pub fn process_byte(&mut self, byte: u8) -> Option<MouseEvent> {
        // Validate first byte of packet
        if self.packet_index == 0 {
            // In PS/2 mouse packets, bit 3 of the first byte is always set
            // This helps detect packet boundary misalignment
            if (byte & 0x08) == 0 {
                // Invalid first byte - skip and try to resync
                return None;
            }
        }

        // Store byte in packet buffer
        self.packet_buffer[self.packet_index] = byte;
        self.packet_index += 1;

        // Check if packet is complete
        if self.packet_index < 3 {
            return None;
        }

        // Reset for next packet
        self.packet_index = 0;

        // Process complete packet
        self.process_packet()
    }

    /// Process a complete 3-byte packet and generate event if needed.
    fn process_packet(&mut self) -> Option<MouseEvent> {
        let [flags, dx, dy] = self.packet_buffer;

        // Check for overflow flags - if set, ignore this packet
        if (flags & 0xC0) != 0 {
            // X or Y overflow - mouse moved too fast, skip
            return None;
        }

        // Extract X movement with sign extension
        let x_delta = if (flags & 0x10) != 0 {
            // Negative X (sign bit set)
            (dx as i16 | !0xFFi16) as i32
        } else {
            dx as i32
        };

        // Extract Y movement with sign extension
        let y_delta = if (flags & 0x20) != 0 {
            // Negative Y (sign bit set)
            (dy as i16 | !0xFFi16) as i32
        } else {
            dy as i32
        };

        // Save old state for comparison
        let old_pos = self.position;
        let old_buttons = self.buttons;

        // Update position (Y is inverted in PS/2 - up is positive, but screen Y increases down)
        self.position.0 = (self.position.0 + x_delta).clamp(0, self.screen_bounds.0);
        self.position.1 = (self.position.1 - y_delta).clamp(0, self.screen_bounds.1);

        // Extract button states
        self.buttons = MouseButtons {
            left: (flags & 0x01) != 0,
            right: (flags & 0x02) != 0,
            middle: (flags & 0x04) != 0,
        };

        // Determine event type based on what changed
        let event_type = self.determine_event_type(old_buttons, old_pos);

        // Only generate event if something actually changed
        event_type.map(|et| MouseEvent {
            event_type: et,
            position: Point::new(self.position.0, self.position.1),
            global_position: Point::new(self.position.0, self.position.1),
            buttons: self.buttons,
        })
    }

    /// Determine the event type based on what changed.
    fn determine_event_type(
        &self,
        old_buttons: MouseButtons,
        old_pos: (i32, i32),
    ) -> Option<MouseEventType> {
        // Check for button changes (takes priority over movement)
        let button_pressed = (self.buttons.left && !old_buttons.left)
            || (self.buttons.right && !old_buttons.right)
            || (self.buttons.middle && !old_buttons.middle);

        let button_released = (!self.buttons.left && old_buttons.left)
            || (!self.buttons.right && old_buttons.right)
            || (!self.buttons.middle && old_buttons.middle);

        if button_pressed {
            Some(MouseEventType::ButtonDown)
        } else if button_released {
            Some(MouseEventType::ButtonUp)
        } else if self.position != old_pos {
            Some(MouseEventType::Move)
        } else {
            // No change - don't generate event
            None
        }
    }

    /// Get current mouse position.
    #[inline]
    pub fn position(&self) -> (i32, i32) {
        self.position
    }

    /// Get current button state.
    #[inline]
    pub fn buttons(&self) -> MouseButtons {
        self.buttons
    }

    /// Check if left button is pressed.
    #[inline]
    pub fn is_left_pressed(&self) -> bool {
        self.buttons.left
    }

    /// Check if right button is pressed.
    #[inline]
    pub fn is_right_pressed(&self) -> bool {
        self.buttons.right
    }

    /// Check if middle button is pressed.
    #[inline]
    pub fn is_middle_pressed(&self) -> bool {
        self.buttons.middle
    }

    /// Reset packet accumulation state.
    ///
    /// Call this if packet stream gets out of sync.
    pub fn reset_packet_state(&mut self) {
        self.packet_index = 0;
    }

    /// Set mouse position directly (for testing or initialization).
    pub fn set_position(&mut self, x: i32, y: i32) {
        self.position.0 = x.clamp(0, self.screen_bounds.0);
        self.position.1 = y.clamp(0, self.screen_bounds.1);
    }
}

impl Default for MouseDriver {
    fn default() -> Self {
        Self::new_default()
    }
}

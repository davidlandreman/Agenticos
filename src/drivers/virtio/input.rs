//! VirtIO Input Device Driver
//!
//! Implements a driver for VirtIO input devices, specifically for tablet/mouse
//! with absolute positioning. This enables seamless mouse integration in QEMU.

use spin::Mutex;
use lazy_static::lazy_static;
use alloc::boxed::Box;
use crate::drivers::pci;
use crate::drivers::virtio::common::{VirtioDevice, Virtqueue};
use crate::debug_info;
use crate::debug_trace;

/// VirtIO input event types (from Linux input-event-codes.h)
pub mod event_types {
    pub const EV_SYN: u16 = 0x00;
    pub const EV_KEY: u16 = 0x01;
    pub const EV_REL: u16 = 0x02;
    pub const EV_ABS: u16 = 0x03;
}

/// VirtIO input absolute axis codes
pub mod abs_codes {
    pub const ABS_X: u16 = 0x00;
    pub const ABS_Y: u16 = 0x01;
}

/// VirtIO input button codes
pub mod btn_codes {
    pub const BTN_LEFT: u16 = 0x110;
    pub const BTN_RIGHT: u16 = 0x111;
    pub const BTN_MIDDLE: u16 = 0x112;
    pub const BTN_TOUCH: u16 = 0x14a;
}

/// VirtIO input event structure
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct VirtioInputEvent {
    pub event_type: u16,
    pub code: u16,
    pub value: u32,
}

/// Size of a VirtIO input event
const EVENT_SIZE: usize = core::mem::size_of::<VirtioInputEvent>();

/// Buffer for receiving events
#[repr(C, align(8))]
struct EventBuffer {
    events: [VirtioInputEvent; 64],
}

/// VirtIO tablet device state
pub struct VirtioTablet {
    device: VirtioDevice,
    eventq: Virtqueue,
    event_buffers: Box<EventBuffer>,
    /// Absolute X position (0-32767)
    abs_x: u32,
    /// Absolute Y position (0-32767)
    abs_y: u32,
    /// Button state (bit 0 = left, bit 1 = right, bit 2 = middle)
    buttons: u8,
    /// Screen width for coordinate scaling
    screen_width: u32,
    /// Screen height for coordinate scaling
    screen_height: u32,
    /// Whether position has been updated
    position_updated: bool,
}

lazy_static! {
    static ref TABLET: Mutex<Option<VirtioTablet>> = Mutex::new(None);
}

impl VirtioTablet {
    /// Initialize a VirtIO tablet from a PCI device
    pub fn new(pci_dev: pci::PciDevice, screen_width: u32, screen_height: u32) -> Option<Self> {
        let device = VirtioDevice::new(pci_dev)?;

        debug_info!("VirtIO input device found");

        // Initialize device
        if !device.init_simple() {
            debug_info!("Failed to initialize VirtIO input device");
            return None;
        }

        // Set up eventq (queue 0)
        let eventq = device.setup_queue(0)?;
        debug_info!("VirtIO input eventq size: {}", eventq.size);

        // Create event buffers
        let event_buffers = Box::new(EventBuffer {
            events: [VirtioInputEvent::default(); 64],
        });

        // Complete initialization
        device.finish_init();

        debug_info!("VirtIO tablet initialized (status: 0x{:02x})", device.read_status());

        let mut tablet = Self {
            device,
            eventq,
            event_buffers,
            abs_x: 0,
            abs_y: 0,
            buttons: 0,
            screen_width,
            screen_height,
            position_updated: false,
        };

        // Submit initial buffers for receiving events
        tablet.submit_buffers();

        Some(tablet)
    }

    /// Submit buffers to receive events
    fn submit_buffers(&mut self) {
        // Submit each event slot as a separate buffer
        for i in 0..self.event_buffers.events.len().min(self.eventq.size as usize) {
            let event_ptr = &self.event_buffers.events[i] as *const _ as *const u8;
            let buffer = unsafe { core::slice::from_raw_parts(event_ptr, EVENT_SIZE) };
            if self.eventq.add_buffer(buffer, true).is_some() {
                debug_trace!("Submitted event buffer {}", i);
            }
        }
        self.eventq.notify();
    }

    /// Process pending events
    pub fn poll(&mut self) -> bool {
        let mut had_events = false;

        // Check ISR to clear interrupt
        let isr = self.device.read_isr();
        if isr & 0x01 != 0 {
            debug_trace!("VirtIO input ISR: 0x{:02x}", isr);
        }

        // Process used buffers
        while let Some((desc_idx, _len)) = self.eventq.pop_used() {
            had_events = true;

            // Copy the event to avoid borrow issues
            let event = self.event_buffers.events[desc_idx as usize];
            self.process_event(&event);

            // Re-submit the buffer
            let event_ptr = &self.event_buffers.events[desc_idx as usize] as *const _ as *const u8;
            let buffer = unsafe { core::slice::from_raw_parts(event_ptr, EVENT_SIZE) };
            self.eventq.add_buffer(buffer, true);
        }

        if had_events {
            self.eventq.notify();
        }

        had_events
    }

    /// Process a single input event
    fn process_event(&mut self, event: &VirtioInputEvent) {
        match event.event_type {
            event_types::EV_ABS => {
                match event.code {
                    abs_codes::ABS_X => {
                        self.abs_x = event.value;
                        self.position_updated = true;
                        debug_trace!("Tablet ABS_X: {}", event.value);
                    }
                    abs_codes::ABS_Y => {
                        self.abs_y = event.value;
                        self.position_updated = true;
                        debug_trace!("Tablet ABS_Y: {}", event.value);
                    }
                    _ => {
                        debug_trace!("Tablet ABS unknown code {}: {}", event.code, event.value);
                    }
                }
            }
            event_types::EV_KEY => {
                let pressed = event.value != 0;
                match event.code {
                    btn_codes::BTN_LEFT | btn_codes::BTN_TOUCH => {
                        if pressed {
                            self.buttons |= 0x01;
                        } else {
                            self.buttons &= !0x01;
                        }
                        debug_info!("Tablet left button: {}", if pressed { "pressed" } else { "released" });
                    }
                    btn_codes::BTN_RIGHT => {
                        if pressed {
                            self.buttons |= 0x02;
                        } else {
                            self.buttons &= !0x02;
                        }
                        debug_info!("Tablet right button: {}", if pressed { "pressed" } else { "released" });
                    }
                    btn_codes::BTN_MIDDLE => {
                        if pressed {
                            self.buttons |= 0x04;
                        } else {
                            self.buttons &= !0x04;
                        }
                        debug_info!("Tablet middle button: {}", if pressed { "pressed" } else { "released" });
                    }
                    _ => {
                        debug_trace!("Tablet KEY unknown code {}: {}", event.code, event.value);
                    }
                }
            }
            event_types::EV_SYN => {
                // Sync event - marks end of a batch of events
                debug_trace!("Tablet SYN");
            }
            _ => {
                debug_trace!("Tablet event type {}: code {} value {}",
                    event.event_type, event.code, event.value);
            }
        }
    }

    /// Get scaled screen coordinates
    pub fn get_screen_position(&self) -> (i32, i32) {
        // VirtIO tablet uses 0-32767 range for absolute coordinates
        // Scale to screen dimensions
        let x = (self.abs_x as u64 * self.screen_width as u64 / 32768) as i32;
        let y = (self.abs_y as u64 * self.screen_height as u64 / 32768) as i32;
        (x, y)
    }

    /// Get raw absolute coordinates (0-32767)
    pub fn get_absolute_position(&self) -> (u32, u32) {
        (self.abs_x, self.abs_y)
    }

    /// Get button state
    pub fn get_buttons(&self) -> u8 {
        self.buttons
    }

    /// Check if position was updated since last check
    pub fn take_position_updated(&mut self) -> bool {
        let updated = self.position_updated;
        self.position_updated = false;
        updated
    }

    /// Update screen dimensions (for coordinate scaling)
    pub fn set_screen_size(&mut self, width: u32, height: u32) {
        self.screen_width = width;
        self.screen_height = height;
    }
}

/// Initialize the global VirtIO tablet if available
pub fn init(screen_width: u32, screen_height: u32) -> bool {
    debug_info!("Searching for VirtIO input devices...");

    let devices = pci::find_virtio_input_devices();
    if devices.is_empty() {
        debug_info!("No VirtIO input devices found");
        return false;
    }

    debug_info!("Found {} VirtIO input device(s)", devices.len());

    // Try to initialize the first one
    for dev in devices {
        debug_info!("Trying VirtIO input device {:04x}:{:04x} at {:02x}:{:02x}.{}",
            dev.vendor_id, dev.device_id, dev.bus, dev.device, dev.function);

        if let Some(tablet) = VirtioTablet::new(dev, screen_width, screen_height) {
            *TABLET.lock() = Some(tablet);
            debug_info!("VirtIO tablet initialized successfully");
            return true;
        }
    }

    debug_info!("Failed to initialize any VirtIO input device");
    false
}

/// Check if VirtIO tablet is available
pub fn is_available() -> bool {
    TABLET.lock().is_some()
}

/// Poll for events (call from main loop or interrupt handler)
pub fn poll() -> bool {
    if let Some(ref mut tablet) = *TABLET.lock() {
        tablet.poll()
    } else {
        false
    }
}

/// Get mouse state (x, y, buttons) - compatible with PS/2 mouse API
pub fn get_state() -> Option<(i32, i32, u8)> {
    let tablet = TABLET.lock();
    tablet.as_ref().map(|t| {
        let (x, y) = t.get_screen_position();
        (x, y, t.get_buttons())
    })
}

/// Handle VirtIO input interrupt
pub fn handle_interrupt() {
    poll();
}

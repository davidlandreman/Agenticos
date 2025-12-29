//! Window System for AgenticOS
//! 
//! This module provides a hierarchical window-based graphics system that supports
//! both GUI and text-based interfaces through a unified abstraction.

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use spin::Mutex;
use core::sync::atomic::{AtomicUsize, Ordering};

pub mod types;
pub mod event;
pub mod graphics;
pub mod manager;
pub mod screen;
pub mod adapters;
pub mod windows;
pub mod terminal;
pub mod console;
pub mod keyboard;
pub mod cursor;
pub mod terminal_factory;

pub use types::*;
pub use event::*;
pub use graphics::*;
pub use manager::*;
pub use screen::*;
pub use windows::*;

// Re-export commonly used types
pub use self::types::{WindowId, ScreenId, Rect, Point};
pub use self::event::{Event, EventResult};
pub use self::graphics::GraphicsDevice;

/// Core window trait that all visual elements implement
pub trait Window: Send {
    /// Get the unique identifier for this window
    fn id(&self) -> WindowId;
    
    /// Get the bounds of this window relative to its parent
    fn bounds(&self) -> Rect;
    
    /// Set the bounds of this window
    fn set_bounds(&mut self, bounds: Rect);

    /// Set bounds without triggering invalidation (for render-time transforms)
    fn set_bounds_no_invalidate(&mut self, bounds: Rect) {
        // Default implementation - subclasses should override
        self.set_bounds(bounds);
    }
    
    /// Check if this window is visible
    fn visible(&self) -> bool;
    
    /// Set the visibility of this window
    fn set_visible(&mut self, visible: bool);
    
    /// Get the parent window ID, if any
    fn parent(&self) -> Option<WindowId>;
    
    /// Get child window IDs
    fn children(&self) -> &[WindowId];
    
    /// Set the parent of this window
    fn set_parent(&mut self, parent: Option<WindowId>);
    
    /// Add a child window
    fn add_child(&mut self, child: WindowId);
    
    /// Remove a child window
    fn remove_child(&mut self, child: WindowId);
    
    /// Paint this window to the graphics device
    fn paint(&mut self, device: &mut dyn GraphicsDevice);
    
    /// Check if this window needs repainting
    fn needs_repaint(&self) -> bool;
    
    /// Mark this window as needing repaint
    fn invalidate(&mut self);
    
    /// Handle an event
    fn handle_event(&mut self, event: Event) -> EventResult;
    
    /// Check if this window can receive keyboard focus
    fn can_focus(&self) -> bool;
    
    /// Check if this window currently has focus
    fn has_focus(&self) -> bool;
    
    /// Set the focus state of this window
    fn set_focus(&mut self, focused: bool);
}

/// Global window manager instance
static WINDOW_MANAGER: Mutex<Option<WindowManager>> = Mutex::new(None);

/// Initialize the window manager with a graphics device
pub fn init_window_manager(device: Box<dyn GraphicsDevice>) {
    let mut wm_lock = WINDOW_MANAGER.lock();
    *wm_lock = Some(WindowManager::new(device));
}

/// Execute a function with the window manager
///
/// IMPORTANT: Disables interrupts while holding the lock to prevent
/// deadlocks with preemptive multitasking.
pub fn with_window_manager<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut WindowManager) -> R,
{
    // Disable interrupts to prevent preemption while holding the lock
    let was_enabled = x86_64::instructions::interrupts::are_enabled();
    x86_64::instructions::interrupts::disable();

    let result = {
        let mut wm_lock = WINDOW_MANAGER.lock();
        wm_lock.as_mut().map(f)
    };

    // Re-enable interrupts if they were enabled before
    if was_enabled {
        x86_64::instructions::interrupts::enable();
    }

    result
}

/// Try to execute a function with the window manager without blocking
pub fn try_with_window_manager<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut WindowManager) -> R,
{
    match WINDOW_MANAGER.try_lock() {
        Some(mut wm_lock) => wm_lock.as_mut().map(f),
        None => None,
    }
}

/// Create a new screen with the specified mode
pub fn create_screen(mode: ScreenMode) -> Option<ScreenId> {
    with_window_manager(|wm| wm.create_screen(mode))
}

/// Switch to a different screen
pub fn switch_screen(screen_id: ScreenId) {
    with_window_manager(|wm| wm.switch_screen(screen_id));
}

/// Create the default desktop environment
pub fn create_default_desktop() {
    with_window_manager(|wm| {
        // Create GUI screen
        let screen_id = wm.create_screen(ScreenMode::Gui);
        wm.switch_screen(screen_id);
        
        // Get actual screen dimensions from graphics device
        let width = wm.graphics_device.width() as u32;
        let height = wm.graphics_device.height() as u32;
        
        // Create desktop background window
        let desktop_id = wm.create_window(None);
        let desktop_window = Box::new(windows::DesktopWindow::new(desktop_id, Rect::new(0, 0, width, height)));
        wm.set_window_impl(desktop_id, desktop_window);
        
        // Set desktop as the root window for the screen
        if let Some(screen) = wm.get_active_screen_mut() {
            screen.set_root_window(desktop_id);
        }
        
        // Create frame window for terminal
        let frame_id = wm.create_window(Some(desktop_id));
        let mut frame_window = Box::new(windows::FrameWindow::new(frame_id, "AgenticOS Terminal"));
        
        // Set the parent of the frame window
        frame_window.set_parent(Some(desktop_id));
        
        // Position and size the frame window (not fullscreen)
        let frame_x = 100;
        let frame_y = 50;
        let frame_width = 800.min(width - 200);
        let frame_height = 600.min(height - 100);
        frame_window.set_bounds(Rect::new(frame_x as i32, frame_y as i32, frame_width, frame_height));
        
        // Create terminal window inside the frame
        let terminal_id = wm.create_window(Some(frame_id));
        let content_area = frame_window.content_area();
        // Terminal window is positioned at the content area offset within the frame
        let terminal_bounds = Rect::new(content_area.x, content_area.y, content_area.width, content_area.height);
        // Use new_with_id to ensure the terminal uses the ID from WindowManager
        let mut terminal_window = Box::new(windows::TerminalWindow::new_with_id(terminal_id, terminal_bounds));
        
        // Set the parent of the terminal window
        terminal_window.set_parent(Some(frame_id));
        
        // Set the terminal as the frame's content
        frame_window.set_content_window(terminal_id);
        
        // Add windows to registry - the frame window already has the terminal as a child
        wm.set_window_impl(frame_id, frame_window);
        wm.set_window_impl(terminal_id, terminal_window);
        
        // Add frame window to desktop's children
        if let Some(desktop) = wm.window_registry.get_mut(&desktop_id) {
            desktop.add_child(frame_id);
        }
        
        // Focus both the frame (for blue title bar) and terminal (for keyboard input)
        if let Some(frame) = wm.window_registry.get_mut(&frame_id) {
            frame.set_focus(true);
        }
        wm.focus_window(terminal_id);

        // Set this as the global terminal window
        terminal::set_terminal_window(terminal_id);
        
        // Force all windows to repaint
        if let Some(window) = wm.window_registry.get_mut(&desktop_id) {
            window.invalidate();
        }
        if let Some(window) = wm.window_registry.get_mut(&frame_id) {
            window.invalidate();
        }
        if let Some(window) = wm.window_registry.get_mut(&terminal_id) {
            window.invalidate();
        }
    });
}

/// Create a terminal window
pub fn create_terminal_window() -> WindowId {
    let window_id = with_window_manager(|wm| {
        // Get screen dimensions
        let width = wm.graphics_device.width() as u32;
        let height = wm.graphics_device.height() as u32;
        
        // Create window
        let window_id = wm.create_window(None);
        let terminal_window = Box::new(windows::TerminalWindow::new(Rect::new(0, 0, width, height)));
        
        wm.set_window_impl(window_id, terminal_window);
        
        // Set as root window if no root exists
        if let Some(screen) = wm.get_active_screen_mut() {
            if screen.root_window.is_none() {
                screen.set_root_window(window_id);
            }
        }
        
        // Focus the window
        wm.focus_window(window_id);
        
        window_id
    }).expect("Window manager not initialized");
    
    // Set as global terminal window
    terminal::set_terminal_window(window_id);
    
    window_id
}

/// Write text to a specific window (if it's a terminal window)
pub fn write_to_window(window_id: WindowId, text: &str) {
    with_window_manager(|wm| {
        // Try to get the window and write to it
        if let Some(window) = wm.window_registry.get_mut(&window_id) {
            // This is a bit hacky - we need to check if it's a terminal window
            // For now, just use the console buffer
            crate::print!("{}", text);
            // Mark window as needing repaint
            window.invalidate();
        }
    });
}

/// Process any pending terminal output.
///
/// This checks for console output and invalidates the terminal window if needed.
/// The actual text writing happens during paint, with suppress_invalidation set
/// to prevent re-invalidation loops.
pub fn process_terminal_output() {
    // Only process if there's actually output to process
    if !crate::window::console::has_output() {
        return;
    }

    // Get the global terminal window ID and invalidate it
    if let Some(terminal_id) = terminal::get_terminal_window() {
        with_window_manager(|wm| {
            if let Some(window) = wm.window_registry.get_mut(&terminal_id) {
                // Just invalidate - the terminal will process console output during paint
                // with suppress_invalidation set, preventing the re-invalidation loop
                window.invalidate();
            }
        });
    }
}

/// Render a single frame
pub fn render_frame() {
    with_window_manager(|wm| wm.render());
}

/// Process a typed event from the new input system.
///
/// This is the preferred way to handle input events - they have already
/// been processed by InputProcessor (scancode->KeyCode conversion,
/// modifier tracking, etc.) and are ready for routing to windows.
pub fn process_event(event: Event) {
    with_window_manager(|wm| wm.process_event(event));
}
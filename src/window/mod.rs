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
    
    /// Check if this window is visible
    fn visible(&self) -> bool;
    
    /// Get the parent window ID, if any
    fn parent(&self) -> Option<WindowId>;
    
    /// Get child window IDs
    fn children(&self) -> &[WindowId];
    
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
pub fn with_window_manager<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut WindowManager) -> R,
{
    let mut wm_lock = WINDOW_MANAGER.lock();
    wm_lock.as_mut().map(f)
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
        
        // Create a full-screen text window for now (acts as terminal)
        let window_id = wm.create_window(None);
        let mut text_window = Box::new(windows::TextWindow::new(Rect::new(0, 0, width, height)));
        
        // Add some initial text to verify it's working
        text_window.write_str("AgenticOS Window System Initialized\n");
        text_window.write_str("Terminal Ready.\n\n");
        
        wm.set_window_impl(window_id, text_window);
        
        // Set this as the root window for the screen
        if let Some(screen) = wm.get_active_screen_mut() {
            screen.set_root_window(window_id);
        }
        
        // Focus the window
        wm.focus_window(window_id);
        
        // Set this as the global terminal window
        terminal::set_terminal_window(window_id);
    });
}

/// Render a single frame
pub fn render_frame() {
    with_window_manager(|wm| wm.render());
}
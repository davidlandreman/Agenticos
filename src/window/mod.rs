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
        
        // Create a full-screen terminal window
        let window_id = wm.create_window(None);
        let terminal_window = Box::new(windows::TerminalWindow::new(Rect::new(0, 0, width, height)));
        
        wm.set_window_impl(window_id, terminal_window);
        
        // Set this as the root window for the screen
        if let Some(screen) = wm.get_active_screen_mut() {
            screen.set_root_window(window_id);
        }
        
        // Focus the window
        wm.focus_window(window_id);
        
        // Set this as the global terminal window
        terminal::set_terminal_window(window_id);
        
        // Force window invalidation to trigger repaint
        if let Some(window) = wm.window_registry.get_mut(&window_id) {
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

/// Render a single frame
pub fn render_frame() {
    with_window_manager(|wm| wm.render());
}
//! Window Manager - Central coordinator for all windows and screens

use alloc::boxed::Box;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use crate::drivers::mouse;
use super::{
    Window, WindowId, Screen, ScreenId, ScreenMode, GraphicsDevice,
    Event, EventResult, KeyboardEvent, MouseEvent, MouseEventType,
    Point, Rect, keyboard::{scancode_to_keycode, is_break_code, KeyboardState},
};

/// The Window Manager coordinates all windows across all screens
pub struct WindowManager {
    /// All screens in the system
    screens: Vec<Screen>,
    /// Currently active screen
    active_screen: ScreenId,
    /// Registry of all windows
    pub window_registry: BTreeMap<WindowId, Box<dyn Window>>,
    /// Focus stack - top element has focus
    focus_stack: Vec<WindowId>,
    /// Z-order of windows (back to front)
    z_order: Vec<WindowId>,
    /// Graphics device for rendering
    pub graphics_device: Box<dyn GraphicsDevice>,
    /// Last known mouse position
    last_mouse_pos: (usize, usize),
    /// Whether we need to redraw the screen
    needs_redraw: bool,
    /// Saved pixels under the mouse cursor
    cursor_saved_pixels: Vec<(usize, usize, crate::graphics::color::Color)>,
    /// Dirty region that needs updating
    dirty_region: Option<Rect>,
    /// Keyboard state tracker
    keyboard_state: KeyboardState,
    /// Whether we're expecting a break code after 0xF0
    expecting_break_code: bool,
}

impl WindowManager {
    /// Create a new window manager with the given graphics device
    pub fn new(graphics_device: Box<dyn GraphicsDevice>) -> Self {
        let mut wm = WindowManager {
            screens: Vec::new(),
            active_screen: ScreenId(0), // Will be set when first screen is created
            window_registry: BTreeMap::new(),
            focus_stack: Vec::new(),
            z_order: Vec::new(),
            graphics_device,
            last_mouse_pos: (0, 0),
            needs_redraw: true,
            cursor_saved_pixels: Vec::new(),
            dirty_region: None,
            keyboard_state: KeyboardState::default(),
            expecting_break_code: false,
        };
        
        // Create default text screen
        let default_screen = wm.create_screen(ScreenMode::Text);
        wm.active_screen = default_screen;
        
        wm
    }
    
    // Screen management
    
    /// Create a new screen with the specified mode
    pub fn create_screen(&mut self, mode: ScreenMode) -> ScreenId {
        let screen = Screen::new(mode);
        let screen_id = screen.id;
        self.screens.push(screen);
        screen_id
    }
    
    /// Switch to a different screen
    pub fn switch_screen(&mut self, screen_id: ScreenId) {
        // Verify screen exists
        if self.screens.iter().any(|s| s.id == screen_id) {
            self.active_screen = screen_id;
            // TODO: Handle focus changes when switching screens
        }
    }
    
    /// Get the active screen
    pub fn get_active_screen(&self) -> Option<&Screen> {
        self.screens.iter().find(|s| s.id == self.active_screen)
    }
    
    /// Get the active screen mutably
    pub fn get_active_screen_mut(&mut self) -> Option<&mut Screen> {
        self.screens.iter_mut().find(|s| s.id == self.active_screen)
    }
    
    // Window management
    
    /// Create a new window with optional parent
    pub fn create_window(&mut self, parent: Option<WindowId>) -> WindowId {
        // Generate a new window ID
        let window_id = WindowId::new();
        
        // Add to z-order (new windows go on top)
        self.z_order.push(window_id);
        
        // If there's a parent, we'll need to update parent-child relationships
        // when set_window_impl is called
        
        window_id
    }
    
    /// Set the implementation for a window
    pub fn set_window_impl(&mut self, id: WindowId, window: Box<dyn Window>) {
        self.window_registry.insert(id, window);
        self.z_order.push(id);
    }
    
    /// Destroy a window and all its children
    pub fn destroy_window(&mut self, id: WindowId) {
        // Remove from all tracking structures
        self.window_registry.remove(&id);
        self.focus_stack.retain(|&wid| wid != id);
        self.z_order.retain(|&wid| wid != id);
        
        // TODO: Recursively destroy children
    }
    
    /// Move a window to a new position
    pub fn move_window(&mut self, id: WindowId, x: i32, y: i32) {
        // TODO: Implement window movement
    }
    
    /// Resize a window
    pub fn resize_window(&mut self, id: WindowId, width: u32, height: u32) {
        // TODO: Implement window resizing
    }
    
    // Focus management
    
    /// Give focus to a specific window
    pub fn focus_window(&mut self, id: WindowId) {
        // Remove from focus stack if already present
        self.focus_stack.retain(|&wid| wid != id);
        
        // Add to top of focus stack
        if let Some(window) = self.window_registry.get(&id) {
            if window.can_focus() {
                // Remove focus from current window
                if let Some(&current_focus) = self.focus_stack.last() {
                    if let Some(current_window) = self.window_registry.get_mut(&current_focus) {
                        current_window.set_focus(false);
                    }
                }
                
                // Set focus on new window
                self.focus_stack.push(id);
                if let Some(window) = self.window_registry.get_mut(&id) {
                    window.set_focus(true);
                }
            }
        }
    }
    
    /// Get the currently focused window
    pub fn focused_window(&self) -> Option<WindowId> {
        self.focus_stack.last().copied()
    }
    
    /// Write text to a specific window (if it's a TextWindow)
    pub fn write_to_window(&mut self, window_id: WindowId, text: &str) {
        use crate::window::windows::TextWindow;
        
        // We need to temporarily remove the window to get mutable access
        if let Some(window) = self.window_registry.remove(&window_id) {
            // Try to downcast to TextWindow and write
            // This is a bit hacky but works for now
            // In the future we'd want a better interface
            
            // Put the window back
            self.window_registry.insert(window_id, window);
        }
    }
    
    // Event routing
    
    /// Process keyboard interrupt data
    pub fn handle_keyboard_scancode(&mut self, scancode: u8) {
        crate::debug_trace!("WindowManager::handle_keyboard_scancode: 0x{:02x}", scancode);
        
        // Special handling for 0xF0 prefix (scancode set 2 break code prefix)
        if scancode == 0xF0 {
            crate::debug_info!("Got 0xF0 prefix, next scancode will be a break code");
            self.expecting_break_code = true;
            return;
        }
        
        // Check if this is a break code (key release)
        let is_break = self.expecting_break_code;
        self.expecting_break_code = false;
        
        if is_break {
            crate::debug_info!("Processing break code (key release) for scancode 0x{:02x}", scancode);
            // Handle modifier key releases
            match scancode {
                0x12 | 0x59 => self.keyboard_state.modifiers.shift = false,  // Shift release
                0x14 => self.keyboard_state.modifiers.ctrl = false,          // Ctrl release
                0x11 => self.keyboard_state.modifiers.alt = false,           // Alt release
                _ => {}
            }
            // Don't process break codes further for now
            return;
        }
        
        // Update modifier state for make codes
        crate::debug_trace!("Updating keyboard modifiers...");
        self.keyboard_state.update_modifiers(scancode);
        crate::debug_trace!("Modifiers updated");
        
        // Convert scancode to KeyCode
        crate::debug_trace!("Converting scancode to keycode...");
        if let Some(key_code) = scancode_to_keycode(scancode) {
            crate::debug_trace!("Converted to KeyCode: {:?}", key_code);
            
            let event = KeyboardEvent {
                key_code,
                pressed: true,  // We're only handling make codes
                modifiers: self.keyboard_state.modifiers,
            };
            
            crate::debug_trace!("Routing keyboard event: pressed={}", event.pressed);
            self.route_keyboard_event(event);
            crate::debug_trace!("route_keyboard_event returned");
        } else {
            crate::debug_trace!("No KeyCode mapping for scancode 0x{:02x}", scancode);
        }
        crate::debug_trace!("handle_keyboard_scancode complete");
    }
    
    /// Route a keyboard event to the appropriate window
    pub fn route_keyboard_event(&mut self, event: KeyboardEvent) {
        crate::debug_trace!("route_keyboard_event called");
        
        // Send to focused window
        if let Some(focused) = self.focused_window() {
            crate::debug_trace!("Routing to focused window: {:?}", focused);
            self.route_event_to_window(focused, Event::Keyboard(event));
        } else {
            crate::debug_trace!("No focused window to route keyboard event to");
        }
    }
    
    /// Route a mouse event to the appropriate window
    pub fn route_mouse_event(&mut self, event: MouseEvent) {
        // Find window under cursor
        let global_pos = event.global_position;
        
        // Walk z-order from front to back
        for &window_id in self.z_order.iter().rev() {
            if let Some(window) = self.window_registry.get(&window_id) {
                let bounds = window.bounds();
                if bounds.contains_point(global_pos) {
                    // Create local event
                    let mut local_event = event;
                    local_event.position = Point::new(
                        global_pos.x - bounds.x,
                        global_pos.y - bounds.y,
                    );
                    
                    self.route_event_to_window(window_id, Event::Mouse(local_event));
                    break;
                }
            }
        }
    }
    
    /// Route an event to a specific window
    fn route_event_to_window(&mut self, window_id: WindowId, event: Event) {
        crate::debug_trace!("route_event_to_window: window={:?}, event={:?}", window_id, event);
        
        let result = if let Some(window) = self.window_registry.get_mut(&window_id) {
            crate::debug_trace!("Calling handle_event on window");
            let result = window.handle_event(event.clone());
            crate::debug_trace!("handle_event returned: {:?}", result);
            result
        } else {
            crate::debug_trace!("Window not found in registry!");
            EventResult::Ignored
        };
        
        // Handle propagation
        if result == EventResult::Propagate {
            if let Some(window) = self.window_registry.get(&window_id) {
                if let Some(parent_id) = window.parent() {
                    crate::debug_trace!("Propagating to parent: {:?}", parent_id);
                    self.route_event_to_window(parent_id, event);
                }
            }
        }
    }
    
    // Rendering
    
    /// Render the current state to the graphics device
    pub fn render(&mut self) {
        use crate::debug_trace;
        
        // Check if mouse moved
        let (mouse_x, mouse_y, _buttons) = mouse::get_state();
        let mouse_x = mouse_x.max(0) as usize;
        let mouse_y = mouse_y.max(0) as usize;
        let mouse_moved = (mouse_x, mouse_y) != self.last_mouse_pos;
        
        // Check if any windows need repaint
        let windows_need_repaint = self.window_registry.values().any(|w| w.needs_repaint());
        
        if windows_need_repaint {
            crate::debug_info!("Windows need repaint detected!");
        }
        
        // Only render if something changed
        if !self.needs_redraw && !mouse_moved && !windows_need_repaint {
            return; // Nothing to update
        }
        
        // If ONLY the mouse moved, do a fast cursor update
        if mouse_moved && !windows_need_repaint && !self.needs_redraw {
            debug_trace!("Fast mouse update");
            self.update_mouse_cursor_fast(mouse_x, mouse_y);
            self.last_mouse_pos = (mouse_x, mouse_y);
            self.graphics_device.flush();
            return;
        }
        
        crate::debug_trace!("Full frame render: windows_need_repaint={}, mouse_moved={}, needs_redraw={}", 
            windows_need_repaint, mouse_moved, self.needs_redraw);
        
        // Only clear the device if we need a full redraw
        // Individual windows will handle their own clearing/painting
        if self.needs_redraw {
            crate::debug_trace!("Clearing graphics device...");
            self.graphics_device.clear(crate::graphics::color::Color::BLACK);
            crate::debug_trace!("Graphics device cleared");
        }
        
        // Render the active screen's windows
        if let Some(screen) = self.get_active_screen() {
            if let Some(root_id) = screen.root_window {
                crate::debug_trace!("Rendering window tree starting from root: {:?}", root_id);
                self.render_window_tree(root_id);
            } else {
                crate::debug_warn!("No root window set for active screen!");
            }
        } else {
            crate::debug_warn!("No active screen!");
        }
        
        // Draw a simple mouse cursor (arrow shape)
        self.draw_mouse_cursor(mouse_x, mouse_y);
        
        self.last_mouse_pos = (mouse_x, mouse_y);
        self.needs_redraw = false;
        
        // Flush to physical framebuffer - this is the expensive operation!
        self.graphics_device.flush();
    }
    
    /// Fast update for mouse cursor - only redraw cursor area
    fn update_mouse_cursor_fast(&mut self, new_x: usize, new_y: usize) {
        use crate::graphics::color::Color;
        
        // Erase old cursor by redrawing that area
        let (old_x, old_y) = self.last_mouse_pos;
        
        // Calculate the bounding box of the old cursor (with some padding)
        let cursor_size = 12;
        let old_left = old_x.saturating_sub(1);
        let old_top = old_y.saturating_sub(1);
        let old_right = (old_x + cursor_size + 1).min(self.graphics_device.width());
        let old_bottom = (old_y + cursor_size + 1).min(self.graphics_device.height());
        
        // Redraw just the old cursor area by re-rendering windows in that region
        // For now, just fill with black (this is why mouse leaves trails)
        // TODO: Properly save/restore background
        for y in old_top..old_bottom {
            for x in old_left..old_right {
                self.graphics_device.draw_pixel(x, y, Color::BLACK);
            }
        }
        
        // Draw new cursor
        self.draw_mouse_cursor(new_x, new_y);
    }
    
    /// Draw the mouse cursor
    fn draw_mouse_cursor(&mut self, x: usize, y: usize) {
        use crate::graphics::color::Color;
        
        // Simple arrow cursor design
        let cursor_color = Color::WHITE;
        let outline_color = Color::BLACK;
        
        // Draw cursor with black outline for visibility
        let cursor_pixels = [
            (0, 0), (0, 1), (0, 2), (0, 3), (0, 4), (0, 5), (0, 6), (0, 7), (0, 8), (0, 9), (0, 10),
            (1, 0), (1, 1), (1, 2), (1, 3), (1, 4), (1, 5), (1, 6), (1, 7), (1, 8), (1, 9),
            (2, 2), (2, 3), (2, 4), (2, 5), (2, 6), (2, 7), (2, 8),
            (3, 3), (3, 4), (3, 5), (3, 6), (3, 7),
            (4, 4), (4, 5), (4, 6),
            (5, 5),
        ];
        
        // Draw black outline first
        for &(dx, dy) in &cursor_pixels {
            let px = x + dx;
            let py = y + dy;
            
            // Draw outline pixels
            if px > 0 {
                self.graphics_device.draw_pixel(px - 1, py, outline_color);
            }
            if px < self.graphics_device.width() - 1 {
                self.graphics_device.draw_pixel(px + 1, py, outline_color);
            }
            if py > 0 {
                self.graphics_device.draw_pixel(px, py - 1, outline_color);
            }
            if py < self.graphics_device.height() - 1 {
                self.graphics_device.draw_pixel(px, py + 1, outline_color);
            }
        }
        
        // Draw white cursor on top
        for &(dx, dy) in &cursor_pixels {
            let px = x + dx;
            let py = y + dy;
            
            if px < self.graphics_device.width() && py < self.graphics_device.height() {
                self.graphics_device.draw_pixel(px, py, cursor_color);
            }
        }
    }
    
    /// Recursively render a window and its children
    fn render_window_tree(&mut self, window_id: WindowId) {
        crate::debug_trace!("render_window_tree: {:?}", window_id);
        // We need to temporarily take the window out to avoid borrowing issues
        if let Some(mut window) = self.window_registry.remove(&window_id) {
            // Get window properties before painting
            let bounds = window.bounds();
            let visible = window.visible();
            let children = window.children().to_vec();
            
            crate::debug_trace!("Window {:?}: bounds={:?}, visible={}", window_id, bounds, visible);
            
            if visible {
                // Set clipping to window bounds
                self.graphics_device.set_clip_rect(Some(bounds));
                
                // Paint the window
                crate::debug_trace!("Calling paint on window {:?}", window_id);
                window.paint(&mut *self.graphics_device);
                
                // Put the window back
                self.window_registry.insert(window_id, window);
                
                // Recursively render children
                for child_id in children {
                    self.render_window_tree(child_id);
                }
                
                // Clear clipping
                self.graphics_device.set_clip_rect(None);
            } else {
                // Put the window back even if not visible
                self.window_registry.insert(window_id, window);
            }
        }
    }
}
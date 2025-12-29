//! Window Manager - Central coordinator for all windows and screens

use alloc::boxed::Box;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use crate::drivers::mouse;
use crate::graphics::compositor::Compositor;
use super::cursor::CursorRenderer;
use super::{
    Window, WindowId, Screen, ScreenId, ScreenMode, GraphicsDevice,
    Event, EventResult, KeyboardEvent, MouseEvent, MouseEventType,
    Point, Rect, keyboard::{scancode_to_keycode, is_break_code, KeyboardState},
};
use super::types::{InteractionState, HitTestResult, ResizeEdge, MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT};
use super::console::take_pending_invalidations;

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
    /// Compositor for dirty tracking and cursor overlay
    compositor: Compositor,
    /// Keyboard state tracker
    keyboard_state: KeyboardState,
    /// Whether we're expecting a break code after 0xF0
    expecting_break_code: bool,
    /// Current window interaction state (dragging, resizing)
    interaction_state: InteractionState,
    /// Last known mouse button state for detecting clicks
    last_mouse_buttons: u8,
    /// Cursor renderer for save/restore and drawing
    cursor: CursorRenderer,
}

impl WindowManager {
    /// Create a new window manager with the given graphics device
    pub fn new(graphics_device: Box<dyn GraphicsDevice>) -> Self {
        let width = graphics_device.width() as u32;
        let height = graphics_device.height() as u32;

        let mut wm = WindowManager {
            screens: Vec::new(),
            active_screen: ScreenId(0), // Will be set when first screen is created
            window_registry: BTreeMap::new(),
            focus_stack: Vec::new(),
            z_order: Vec::new(),
            graphics_device,
            compositor: Compositor::new(width, height),
            keyboard_state: KeyboardState::default(),
            expecting_break_code: false,
            interaction_state: InteractionState::Idle,
            last_mouse_buttons: 0,
            cursor: CursorRenderer::new(),
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
        
        // Store parent information for later use when set_window_impl is called
        // We'll establish the relationship in set_window_impl_with_parent
        
        window_id
    }
    
    /// Set the implementation for a window
    pub fn set_window_impl(&mut self, id: WindowId, mut window: Box<dyn Window>) {
        // Check if this window should have a parent based on how it was created
        // For now, we'll rely on the window having its parent set before calling this
        let parent_id = window.parent();
        
        // Add to registry
        self.window_registry.insert(id, window);
        
        // Only add to z-order if not already present
        if !self.z_order.contains(&id) {
            self.z_order.push(id);
        }
        
        // If the window has a parent, update the parent's children list
        if let Some(parent_id) = parent_id {
            // We need to add this window to its parent's children
            // However, the current Window trait doesn't have add_child method
            // This is a design limitation we need to work around
        }
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
                // Remove focus from current window AND its parent frame
                if let Some(&current_focus) = self.focus_stack.last() {
                    if let Some(current_window) = self.window_registry.get_mut(&current_focus) {
                        current_window.set_focus(false);
                    }
                    // Also unfocus the parent frame if it exists
                    if let Some(current_window) = self.window_registry.get(&current_focus) {
                        if let Some(parent_id) = current_window.parent() {
                            if let Some(parent) = self.window_registry.get_mut(&parent_id) {
                                parent.set_focus(false);
                            }
                        }
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

    /// Process typed events from the new input system.
    ///
    /// This method handles events that have already been processed by the
    /// InputProcessor (scancode->KeyCode conversion, modifier tracking, etc.)
    pub fn process_event(&mut self, event: Event) {
        match event {
            Event::Keyboard(kb_event) => {
                self.route_keyboard_event(kb_event);
            }
            Event::Mouse(mouse_event) => {
                self.route_mouse_event(mouse_event);
            }
            _ => {
                // Other events handled as before
            }
        }
    }
    
    /// Route a mouse event to the appropriate window
    pub fn route_mouse_event(&mut self, event: MouseEvent) {
        // Find window under cursor
        let global_pos = event.global_position;

        // Walk z-order from front to back (topmost windows first)
        for &window_id in self.z_order.iter().rev() {
            // Get global bounds for this window (accounts for parent hierarchy)
            if let Some(global_bounds) = self.get_global_bounds(window_id) {
                if global_bounds.contains_point(global_pos) {
                    // Create local event with position relative to this window's global position
                    let mut local_event = event;
                    local_event.position = Point::new(
                        global_pos.x - global_bounds.x,
                        global_pos.y - global_bounds.y,
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
        // Process any pending invalidations from deferred queue
        let pending = take_pending_invalidations();
        for window_id in pending {
            if let Some(window) = self.window_registry.get_mut(&window_id) {
                window.invalidate();
            }
        }

        // Check if mouse moved
        let (mouse_x, mouse_y, buttons) = mouse::get_state();
        let mouse_x = mouse_x.max(0) as usize;
        let mouse_y = mouse_y.max(0) as usize;

        // Handle window dragging
        self.handle_dragging(mouse_x as i32, mouse_y as i32, buttons);

        // Update cursor position in compositor (this marks dirty regions)
        let mouse_moved = self.compositor.update_cursor(mouse_x, mouse_y);

        // Check if any windows need repaint and mark their regions dirty
        for (window_id, window) in &self.window_registry {
            if window.needs_repaint() {
                let bounds = window.bounds();
                self.compositor.dirty.mark_dirty(bounds);
                crate::debug_trace!("Window {:?} needs repaint, marking dirty: {:?}", window_id, bounds);
            }
        }

        // Early exit if nothing needs rendering
        if !self.compositor.needs_render() && !mouse_moved {
            return; // Nothing to update - this is the key optimization!
        }

        // Begin frame
        self.compositor.begin_frame();

        // Restore old cursor background before any rendering
        // This erases the cursor from its old position
        self.cursor.restore_background(&mut *self.graphics_device);

        // Determine if we need full repaint or can do partial
        let full_repaint = self.compositor.dirty.needs_full_repaint();

        if full_repaint {
            crate::debug_trace!("Full frame render required");
            self.graphics_device.clear(crate::graphics::color::Color::BLACK);

            // When doing a full repaint, all windows must repaint
            // Otherwise windows that don't think they need repainting will skip
            // and leave holes where the screen was cleared
            for window in self.window_registry.values_mut() {
                window.invalidate();
            }
        }

        // Render the active screen's windows
        if let Some(screen) = self.get_active_screen() {
            if let Some(root_id) = screen.root_window {
                self.render_window_tree(root_id);
            }
        }

        // Save background at new cursor position, then draw cursor
        self.cursor.save_background(mouse_x, mouse_y, &*self.graphics_device);
        self.cursor.draw(mouse_x, mouse_y, &mut *self.graphics_device);

        // End frame and clear dirty tracking
        self.compositor.end_frame();

        // Flush to physical framebuffer
        self.graphics_device.flush();
    }

    /// Mark a window as needing repaint (for external callers).
    pub fn invalidate_window(&mut self, window_id: WindowId) {
        if let Some(window) = self.window_registry.get(&window_id) {
            let bounds = window.bounds();
            self.compositor.dirty.mark_dirty(bounds);
        }
    }

    /// Force a full repaint on the next frame.
    pub fn force_full_repaint(&mut self) {
        self.compositor.dirty.mark_full_repaint();
    }

    /// Recursively render a window and its children
    fn render_window_tree(&mut self, window_id: WindowId) {
        self.render_window_tree_with_offset(window_id, 0, 0);
    }
    
    /// Recursively render a window and its children with parent offset
    fn render_window_tree_with_offset(&mut self, window_id: WindowId, parent_x: i32, parent_y: i32) {
        self.render_window_tree_with_offset_propagate(window_id, parent_x, parent_y, false);
    }

    /// Internal helper for rendering with invalidation propagation
    fn render_window_tree_with_offset_propagate(
        &mut self,
        window_id: WindowId,
        parent_x: i32,
        parent_y: i32,
        parent_was_repainted: bool,
    ) {
        crate::debug_trace!("render_window_tree: {:?}, offset=({}, {})", window_id, parent_x, parent_y);
        // We need to temporarily take the window out to avoid borrowing issues
        if let Some(mut window) = self.window_registry.remove(&window_id) {
            // Get window properties before painting
            let mut bounds = window.bounds();
            let visible = window.visible();
            let children = window.children().to_vec();

            // If parent was repainted, this window must also repaint
            // (because the parent's background covered us)
            if parent_was_repainted && !window.needs_repaint() {
                window.invalidate();
            }

            // Check if this window will repaint (for propagating to children)
            let will_repaint = window.needs_repaint();

            // Adjust bounds by parent offset
            bounds.x += parent_x;
            bounds.y += parent_y;

            crate::debug_trace!("Window {:?}: absolute_bounds={:?}, visible={}", window_id, bounds, visible);

            if visible {
                // Save the original bounds
                let original_bounds = window.bounds();

                // Temporarily set absolute bounds for rendering (without invalidation!)
                window.set_bounds_no_invalidate(bounds);

                // Set clipping to window bounds
                self.graphics_device.set_clip_rect(Some(bounds));

                // Paint the window
                crate::debug_trace!("Calling paint on window {:?}", window_id);
                window.paint(&mut *self.graphics_device);

                // Restore original bounds (without invalidation!)
                window.set_bounds_no_invalidate(original_bounds);

                // Put the window back
                self.window_registry.insert(window_id, window);

                // Recursively render children with updated offset
                // Propagate repaint flag to children if this window was repainted
                for child_id in children {
                    self.render_window_tree_with_offset_propagate(child_id, bounds.x, bounds.y, will_repaint);
                }

                // Clear clipping
                self.graphics_device.set_clip_rect(None);
            } else {
                // Put the window back even if not visible
                self.window_registry.insert(window_id, window);
            }
        }
    }

    // Window Interaction (Dragging/Resizing)

    /// Handle window dragging based on current mouse state.
    fn handle_dragging(&mut self, mouse_x: i32, mouse_y: i32, buttons: u8) {
        let left_button_pressed = (buttons & 0x01) != 0;
        let left_button_was_pressed = (self.last_mouse_buttons & 0x01) != 0;
        self.last_mouse_buttons = buttons;

        match self.interaction_state {
            InteractionState::Idle => {
                // Check if we just pressed the left button
                if left_button_pressed && !left_button_was_pressed {
                    self.start_drag_if_on_title_bar(mouse_x, mouse_y);
                }
            }
            InteractionState::Dragging { window, start_mouse, start_window } => {
                if left_button_pressed {
                    // Continue dragging - move the window
                    let delta_x = mouse_x - start_mouse.x;
                    let delta_y = mouse_y - start_mouse.y;
                    let new_x = start_window.x + delta_x;
                    let new_y = start_window.y + delta_y;

                    // Move the window
                    if let Some(win) = self.window_registry.get_mut(&window) {
                        let old_bounds = win.bounds();

                        // Only update if position actually changed
                        if new_x != old_bounds.x || new_y != old_bounds.y {
                            // Update bounds
                            win.set_bounds(Rect::new(new_x, new_y, old_bounds.width, old_bounds.height));
                            win.invalidate();

                            // Force full repaint to properly redraw exposed areas
                            self.compositor.dirty.mark_full_repaint();

                            crate::debug_trace!("Dragging window {:?} to ({}, {})", window, new_x, new_y);
                        }
                    }
                } else {
                    // Button released - end drag
                    crate::debug_info!("Window drag ended");
                    self.interaction_state = InteractionState::Idle;
                }
            }
            InteractionState::Resizing { window, edge, start_mouse, start_bounds } => {
                if left_button_pressed {
                    // Calculate delta from start position
                    let delta_x = mouse_x - start_mouse.x;
                    let delta_y = mouse_y - start_mouse.y;

                    // Calculate new bounds using the helper method
                    let new_bounds = start_bounds.resize_edge(
                        edge,
                        delta_x,
                        delta_y,
                        MIN_WINDOW_WIDTH,
                        MIN_WINDOW_HEIGHT,
                    );

                    // Update the window bounds if changed
                    if let Some(win) = self.window_registry.get_mut(&window) {
                        let old_bounds = win.bounds();

                        // Only update if bounds actually changed
                        if old_bounds != new_bounds {
                            // Update window bounds
                            win.set_bounds(new_bounds);
                            win.invalidate();

                            // Force full repaint to properly redraw exposed areas
                            self.compositor.dirty.mark_full_repaint();

                            crate::debug_trace!("Resizing window {:?} to {:?}", window, new_bounds);
                        }
                    }

                    // Update child windows after resize
                    self.update_children_for_resized_window(window);
                } else {
                    // Button released - end resize
                    crate::debug_info!("Window resize ended");
                    self.interaction_state = InteractionState::Idle;
                }
            }
        }
    }

    /// Check if the mouse is on a title bar or border and start the appropriate interaction.
    fn start_drag_if_on_title_bar(&mut self, mouse_x: i32, mouse_y: i32) {
        // Find the topmost window under the mouse
        // Only check top-level windows (direct children of desktop, which have a parent)
        // Skip child windows like terminals inside frames - their bounds are parent-relative
        let mut target_window = None;
        let mut target_hit = HitTestResult::None;

        for &window_id in self.z_order.iter().rev() {
            if let Some(window) = self.window_registry.get(&window_id) {
                // Skip windows that are children of other windows (not top-level)
                // Top-level windows have a parent (the desktop), but their children
                // (like terminals inside frames) should be skipped
                if let Some(parent_id) = window.parent() {
                    // Check if parent is a frame (has its own parent = desktop)
                    if let Some(parent) = self.window_registry.get(&parent_id) {
                        if parent.parent().is_some() {
                            // This window's parent has a parent, so this is a grandchild
                            // (e.g., terminal inside frame inside desktop) - skip it
                            continue;
                        }
                    }
                }

                let bounds = window.bounds();
                let local_x = mouse_x - bounds.x;
                let local_y = mouse_y - bounds.y;

                if bounds.contains_point(Point::new(mouse_x, mouse_y)) {
                    // Hit test for frame-like windows
                    // Constants: 2px border, 24px title bar (so title bar area is y 2..26)
                    let border = 2;
                    let title_height = 24;

                    // Check border regions first (corners before edges)
                    let at_left = local_x < border;
                    let at_right = local_x >= bounds.width as i32 - border;
                    let at_top = local_y < border;
                    let at_bottom = local_y >= bounds.height as i32 - border;

                    if at_top && at_left {
                        target_window = Some(window_id);
                        target_hit = HitTestResult::Border(ResizeEdge::TopLeft);
                        break;
                    } else if at_top && at_right {
                        target_window = Some(window_id);
                        target_hit = HitTestResult::Border(ResizeEdge::TopRight);
                        break;
                    } else if at_bottom && at_left {
                        target_window = Some(window_id);
                        target_hit = HitTestResult::Border(ResizeEdge::BottomLeft);
                        break;
                    } else if at_bottom && at_right {
                        target_window = Some(window_id);
                        target_hit = HitTestResult::Border(ResizeEdge::BottomRight);
                        break;
                    } else if at_top {
                        target_window = Some(window_id);
                        target_hit = HitTestResult::Border(ResizeEdge::Top);
                        break;
                    } else if at_bottom {
                        target_window = Some(window_id);
                        target_hit = HitTestResult::Border(ResizeEdge::Bottom);
                        break;
                    } else if at_left {
                        target_window = Some(window_id);
                        target_hit = HitTestResult::Border(ResizeEdge::Left);
                        break;
                    } else if at_right {
                        target_window = Some(window_id);
                        target_hit = HitTestResult::Border(ResizeEdge::Right);
                        break;
                    } else if local_y >= border && local_y < border + title_height {
                        // Title bar (inside border, top 24 pixels)
                        // Check for close button first (16x16 button, 4px from right edge)
                        let close_btn_size = 16;
                        let close_btn_padding = 4;
                        let close_btn_x = bounds.width as i32 - border - close_btn_padding - close_btn_size;
                        let close_btn_y = border + (title_height - close_btn_size) / 2;

                        if local_x >= close_btn_x && local_x < close_btn_x + close_btn_size
                            && local_y >= close_btn_y && local_y < close_btn_y + close_btn_size
                        {
                            target_window = Some(window_id);
                            target_hit = HitTestResult::CloseButton;
                            break;
                        }

                        target_window = Some(window_id);
                        target_hit = HitTestResult::TitleBar;
                        break;
                    } else {
                        // Client area - focus the window but don't start drag/resize
                        target_window = Some(window_id);
                        target_hit = HitTestResult::Client;
                        break;
                    }
                }
            }
        }

        if let Some(window_id) = target_window {
            if let Some(window) = self.window_registry.get(&window_id) {
                let bounds = window.bounds();

                match target_hit {
                    HitTestResult::TitleBar => {
                        crate::debug_info!("Starting window drag for {:?} at ({}, {})", window_id, bounds.x, bounds.y);
                        self.interaction_state = InteractionState::Dragging {
                            window: window_id,
                            start_mouse: Point::new(mouse_x, mouse_y),
                            start_window: Point::new(bounds.x, bounds.y),
                        };
                        self.bring_to_front(window_id);
                        self.focus_frame_and_content(window_id);
                    }
                    HitTestResult::Border(edge) => {
                        crate::debug_info!("Starting window resize for {:?} edge {:?}", window_id, edge);
                        self.interaction_state = InteractionState::Resizing {
                            window: window_id,
                            edge,
                            start_mouse: Point::new(mouse_x, mouse_y),
                            start_bounds: bounds,
                        };
                        self.bring_to_front(window_id);
                        self.focus_frame_and_content(window_id);
                    }
                    HitTestResult::Client => {
                        // Clicked in client area - focus the window
                        crate::debug_info!("Clicked client area of {:?}", window_id);
                        self.bring_to_front(window_id);
                        self.focus_frame_and_content(window_id);
                    }
                    HitTestResult::CloseButton => {
                        // Close button clicked - destroy the window
                        crate::debug_info!("Close button clicked for {:?}", window_id);
                        self.destroy_window(window_id);
                        self.compositor.dirty.mark_full_repaint();
                    }
                    _ => {}
                }
            }
        }
    }

    /// Focus a frame window and its content (terminal) window
    fn focus_frame_and_content(&mut self, frame_id: WindowId) {
        // First, unfocus the currently focused window and its parent frame
        if let Some(&current_focus) = self.focus_stack.last() {
            // Unfocus the current window
            if let Some(window) = self.window_registry.get_mut(&current_focus) {
                window.set_focus(false);
            }
            // Also unfocus its parent frame if it has one
            if let Some(window) = self.window_registry.get(&current_focus) {
                if let Some(parent_id) = window.parent() {
                    if let Some(parent) = self.window_registry.get_mut(&parent_id) {
                        parent.set_focus(false);
                    }
                }
            }
        }

        // Focus the frame window (for blue title bar)
        if let Some(frame) = self.window_registry.get_mut(&frame_id) {
            frame.set_focus(true);
        }

        // Find and focus the content window (terminal) for keyboard input
        let content_id = {
            if let Some(frame) = self.window_registry.get(&frame_id) {
                // Get the first child which should be the terminal
                frame.children().first().copied()
            } else {
                None
            }
        };

        if let Some(content_id) = content_id {
            // Update focus stack
            self.focus_stack.retain(|&id| id != content_id);
            self.focus_stack.push(content_id);

            if let Some(content) = self.window_registry.get_mut(&content_id) {
                content.set_focus(true);
            }
            crate::debug_info!("Focused frame {:?} and content {:?}", frame_id, content_id);
        } else {
            // No content, just focus the frame itself
            self.focus_stack.retain(|&id| id != frame_id);
            self.focus_stack.push(frame_id);
            crate::debug_info!("Focused frame {:?} (no content)", frame_id);
        }
    }

    /// Bring a window to the front of the z-order.
    /// Also brings all children to front (after the parent).
    pub fn bring_to_front(&mut self, window_id: WindowId) {
        // First collect all children recursively
        let children = self.collect_children_recursive(window_id);

        // Remove from current position
        self.z_order.retain(|&id| id != window_id);
        // Add to front
        self.z_order.push(window_id);

        // Also move all children to front (after parent)
        for child_id in children {
            self.z_order.retain(|&id| id != child_id);
            self.z_order.push(child_id);
        }

        // Mark entire screen as needing repaint for proper layering
        self.compositor.dirty.mark_full_repaint();
    }

    /// Recursively collect all children of a window
    fn collect_children_recursive(&self, window_id: WindowId) -> Vec<WindowId> {
        let mut result = Vec::new();
        if let Some(window) = self.window_registry.get(&window_id) {
            for &child_id in window.children() {
                result.push(child_id);
                // Recursively get children of children
                result.extend(self.collect_children_recursive(child_id));
            }
        }
        result
    }

    /// Calculate the global bounds of a window by traversing the parent chain
    fn get_global_bounds(&self, window_id: WindowId) -> Option<Rect> {
        let window = self.window_registry.get(&window_id)?;
        let mut bounds = window.bounds();

        // Traverse parent chain and accumulate offsets
        let mut current_parent = window.parent();
        while let Some(parent_id) = current_parent {
            if let Some(parent_window) = self.window_registry.get(&parent_id) {
                let parent_bounds = parent_window.bounds();
                bounds.x += parent_bounds.x;
                bounds.y += parent_bounds.y;
                current_parent = parent_window.parent();
            } else {
                break;
            }
        }

        Some(bounds)
    }

    /// Update child windows after a parent has been resized.
    fn update_children_for_resized_window(&mut self, parent_id: WindowId) {
        // Get the parent's children and calculate new content area
        let (children, content_area) = {
            if let Some(window) = self.window_registry.get(&parent_id) {
                let children = window.children().to_vec();
                let bounds = window.bounds();

                // Calculate content area - for FrameWindow this excludes borders/title
                // Constants: 2px border on all sides, 24px title bar
                let border = 2u32;
                let title_height = 24u32;

                // Content area is relative to the parent window
                let content_area = Rect::new(
                    border as i32,
                    (border + title_height) as i32,
                    bounds.width.saturating_sub(border * 2),
                    bounds.height.saturating_sub(border * 2 + title_height),
                );
                (children, Some(content_area))
            } else {
                (Vec::new(), None)
            }
        };

        // Update each child's bounds to fill the content area
        if let Some(content_area) = content_area {
            for child_id in children {
                if let Some(child) = self.window_registry.get_mut(&child_id) {
                    let child_bounds = child.bounds();
                    // Only update if bounds differ
                    if child_bounds != content_area {
                        child.set_bounds(content_area);
                        child.invalidate();
                    }
                }
            }
        }
    }
}
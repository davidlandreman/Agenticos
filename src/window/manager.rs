//! Window Manager - Central coordinator for all windows and screens

use alloc::boxed::Box;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicPtr, Ordering};
use crate::drivers::mouse;
use crate::graphics::compositor::Compositor;
use super::cursor::CursorRenderer;
use super::{
    Window, WindowId, Screen, ScreenId, ScreenMode, GraphicsDevice,
    Event, EventResult, KeyboardEvent, MouseEvent, MouseEventType,
    Point, Rect, keyboard::{scancode_to_keycode, KeyboardState},
};
use super::types::{
    clamp_drag_x, clamp_drag_y,
    InteractionState, HitTestResult, ResizeEdge,
    MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT,
};
use super::console::take_pending_invalidations;
use super::windows::MenuBarPopup;

/// The Window Manager coordinates all windows across all screens
///
/// Z-order is encoded directly in each parent's `children` list:
/// `children[0]` is the bottom-most sibling, `children[len-1]` is the top.
/// Hit-testing and rendering both consult that single ordering — there is no
/// separate flat z-order list to drift out of sync.
pub struct WindowManager {
    /// All screens in the system
    screens: Vec<Screen>,
    /// Currently active screen
    active_screen: ScreenId,
    /// Registry of all windows
    pub window_registry: BTreeMap<WindowId, Box<dyn Window>>,
    /// Focus stack - top element has focus
    focus_stack: Vec<WindowId>,
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
    /// Currently active popup menu (if any)
    active_menu: Option<WindowId>,
    /// Taskbar window ID (if any)
    taskbar_id: Option<WindowId>,
    /// Currently active modal dialog (if any)
    modal_dialog: Option<WindowId>,
    /// Active menu bar popup: (menu_bar_id, popup_window_id)
    menu_bar_popup: Option<(WindowId, WindowId)>,
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
            graphics_device,
            compositor: Compositor::new(width, height),
            keyboard_state: KeyboardState::default(),
            expecting_break_code: false,
            interaction_state: InteractionState::Idle,
            last_mouse_buttons: 0,
            cursor: CursorRenderer::new(),
            active_menu: None,
            taskbar_id: None,
            modal_dialog: None,
            menu_bar_popup: None,
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
    pub fn create_window(&mut self, _parent: Option<WindowId>) -> WindowId {
        // Generate a new window ID
        let window_id = WindowId::new();
        
        // Store parent information for later use when set_window_impl is called
        // We'll establish the relationship in set_window_impl_with_parent
        
        window_id
    }
    
    /// Set the implementation for a window. If the window has a parent set,
    /// this also attaches it to the parent's children list (to the top of the
    /// sibling z-order). Callers no longer need a separate `parent.add_child`.
    pub fn set_window_impl(&mut self, id: WindowId, window: Box<dyn Window>) {
        let parent_id = window.parent();
        self.window_registry.insert(id, window);
        if let Some(parent_id) = parent_id {
            if let Some(parent) = self.window_registry.get_mut(&parent_id) {
                parent.add_child(id);
            }
        }
    }

    /// Destroy a window and all of its descendants.
    pub fn destroy_window(&mut self, id: WindowId) {
        // Snapshot children first (since registry is mutated below).
        let children: Vec<WindowId> = self.window_registry
            .get(&id)
            .map(|w| w.children().to_vec())
            .unwrap_or_default();
        for child_id in children {
            self.destroy_window(child_id);
        }

        // Detach from parent's children list.
        let parent_id = self.window_registry.get(&id).and_then(|w| w.parent());
        if let Some(parent_id) = parent_id {
            if let Some(parent) = self.window_registry.get_mut(&parent_id) {
                parent.remove_child(id);
            }
        }

        self.window_registry.remove(&id);
        self.focus_stack.retain(|&wid| wid != id);
    }

    // Focus management

    /// Give focus to a specific window.
    ///
    /// Also brings the window (and its ancestors) to the front of their parent's
    /// children list, and sets the visual focus state on a parent frame so the
    /// chrome displays as active. Most callers want this — focus implies the
    /// window should be visible and on top.
    pub fn focus_window(&mut self, id: WindowId) {
        let can_focus = self.window_registry.get(&id).map(|w| w.can_focus()).unwrap_or(false);
        if !can_focus {
            return;
        }

        // Unfocus the previously focused window and its parent frame (if any).
        if let Some(&current_focus) = self.focus_stack.last() {
            let current_parent = self.window_registry
                .get(&current_focus)
                .and_then(|w| w.parent());
            if let Some(w) = self.window_registry.get_mut(&current_focus) {
                w.set_focus(false);
            }
            if let Some(parent_id) = current_parent {
                if let Some(parent) = self.window_registry.get_mut(&parent_id) {
                    parent.set_focus(false);
                }
            }
        }

        // Move to top of focus stack.
        self.focus_stack.retain(|&wid| wid != id);
        self.focus_stack.push(id);

        // Set focus on the target.
        if let Some(w) = self.window_registry.get_mut(&id) {
            w.set_focus(true);
        }

        // Visually focus parent frame too (so the title bar turns blue when a
        // content window like a terminal is the keyboard target).
        let parent_id = self.window_registry.get(&id).and_then(|w| w.parent());
        if let Some(parent_id) = parent_id {
            if let Some(parent) = self.window_registry.get_mut(&parent_id) {
                parent.set_focus(true);
            }
        }

        // Bring this window (and its ancestor chain) to the front.
        self.bring_to_front(id);
    }

    /// Get the currently focused window
    pub fn focused_window(&self) -> Option<WindowId> {
        self.focus_stack.last().copied()
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

        // If modal dialog is active, only route to it or its children
        if let Some(modal_id) = self.modal_dialog {
            if let Some(focused) = self.focused_window() {
                if self.is_modal_window(focused) {
                    self.route_event_to_window(focused, Event::Keyboard(event));
                } else {
                    // Focus is on non-modal window, route to modal instead
                    self.route_event_to_window(modal_id, Event::Keyboard(event));
                }
            } else {
                self.route_event_to_window(modal_id, Event::Keyboard(event));
            }
            return;
        }

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
                // Signal any process waiting for input on the focused window
                if let Some(focused) = self.focused_window() {
                    if let Some(pid) = crate::process::get_process_for_terminal(focused) {
                        crate::process::signal_process(pid, crate::process::WakeEvents::INPUT);
                    }
                }
                self.route_keyboard_event(kb_event);
            }
            Event::Mouse(mouse_event) => {
                // Signal GUIShell for window events (clicks might need processing)
                if matches!(mouse_event.event_type, crate::window::event::MouseEventType::ButtonDown) {
                    crate::commands::guishell::signal_guishell();
                }
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

        // If there's an active menu, check if click is outside it
        if let Some(menu_id) = self.active_menu {
            if let Some(menu_bounds) = self.get_global_bounds(menu_id) {
                crate::debug_trace!("Active menu {:?} bounds: {:?}, mouse: {:?}", menu_id, menu_bounds, global_pos);
                // Check if this is a button down event
                if matches!(event.event_type, MouseEventType::ButtonDown) {
                    if !menu_bounds.contains_point(global_pos) {
                        // Click outside menu - close it and don't route the click
                        // (user's intent was to close the menu, not click what's underneath)
                        crate::debug_info!("Click outside menu - closing");
                        self.close_active_menu();
                        return;
                    } else {
                        crate::debug_info!("Click INSIDE menu {:?} at {:?}", menu_id, global_pos);
                    }
                }
            }
        }

        // If modal dialog is active, only route within the modal subtree.
        if let Some(modal_id) = self.modal_dialog {
            if let Some((hit_id, hit_bounds)) = self.topmost_at(modal_id, global_pos, 0, 0) {
                let mut local_event = event;
                local_event.position = Point::new(
                    global_pos.x - hit_bounds.x,
                    global_pos.y - hit_bounds.y,
                );
                self.route_event_to_window(hit_id, Event::Mouse(local_event));
            }
            // Click outside modal dialog - ignore
            return;
        }

        // Walk the active screen's window tree front-to-back, descending
        // into children last-first (so topmost siblings are tested first).
        let root_id = self.get_active_screen().and_then(|s| s.root_window);
        if let Some(root_id) = root_id {
            if let Some((hit_id, hit_bounds)) = self.topmost_at(root_id, global_pos, 0, 0) {
                // Scroll wheel events are routed to the nearest enclosing
                // ScrollView ancestor of the hit window (innermost match
                // wins). If no ScrollView is found in the chain, fall
                // through to standard delivery to the hit window.
                if matches!(event.event_type, MouseEventType::Scroll { .. }) {
                    if let Some(sv_id) = self.nearest_scroll_view_ancestor(hit_id) {
                        let sv_bounds = self
                            .get_global_bounds(sv_id)
                            .unwrap_or(hit_bounds);
                        let mut local_event = event;
                        local_event.position = Point::new(
                            global_pos.x - sv_bounds.x,
                            global_pos.y - sv_bounds.y,
                        );
                        self.route_event_to_window(sv_id, Event::Mouse(local_event));
                        return;
                    }
                }

                let mut local_event = event;
                local_event.position = Point::new(
                    global_pos.x - hit_bounds.x,
                    global_pos.y - hit_bounds.y,
                );
                self.route_event_to_window(hit_id, Event::Mouse(local_event));
            }
        }
    }

    /// Walk the parent chain starting at `id` (inclusive) and return the
    /// first window for which `is_scroll_view()` returns `true`. Returns
    /// `None` if no `ScrollView` ancestor exists.
    fn nearest_scroll_view_ancestor(&self, id: WindowId) -> Option<WindowId> {
        let mut current = Some(id);
        while let Some(cur_id) = current {
            let window = self.window_registry.get(&cur_id)?;
            if window.is_scroll_view() {
                return Some(cur_id);
            }
            current = window.parent();
        }
        None
    }

    /// Walk the subtree rooted at `id` and return the topmost visible window
    /// whose absolute bounds contain `point`, along with those absolute bounds.
    /// Children are tested last-to-first to honor sibling z-order.
    fn topmost_at(&self, id: WindowId, point: Point, parent_x: i32, parent_y: i32) -> Option<(WindowId, Rect)> {
        let window = self.window_registry.get(&id)?;
        if !window.visible() {
            return None;
        }
        let local = window.bounds();
        let abs = Rect::new(local.x + parent_x, local.y + parent_y, local.width, local.height);
        if !abs.contains_point(point) {
            return None;
        }
        for &child_id in window.children().iter().rev() {
            if let Some(hit) = self.topmost_at(child_id, point, abs.x, abs.y) {
                return Some(hit);
            }
        }
        Some((id, abs))
    }

    /// Walk the active screen's tree in render order (depth-first, children
    /// in declaration order — same order `render_window_tree` paints in)
    /// and return each window's id and absolute bounds.
    fn collect_render_order(&self) -> Vec<(WindowId, Rect)> {
        let mut out = Vec::new();
        if let Some(root_id) = self.get_active_screen().and_then(|s| s.root_window) {
            self.collect_render_order_recursive(root_id, 0, 0, &mut out);
        }
        out
    }

    fn collect_render_order_recursive(
        &self,
        id: WindowId,
        parent_x: i32,
        parent_y: i32,
        out: &mut Vec<(WindowId, Rect)>,
    ) {
        let window = match self.window_registry.get(&id) {
            Some(w) => w,
            None => return,
        };
        if !window.visible() {
            return;
        }
        let local = window.bounds();
        let abs = Rect::new(local.x + parent_x, local.y + parent_y, local.width, local.height);
        out.push((id, abs));
        for &child_id in window.children() {
            self.collect_render_order_recursive(child_id, abs.x, abs.y, out);
        }
    }

    /// For each dirty window, mark every later (above-in-z-order) window
    /// whose absolute bounds overlap as also dirty. This propagates in a
    /// single forward pass: by the time the loop reaches an index, any
    /// earlier overlapping dirty window has already invalidated it.
    fn cascade_invalidation(&mut self) {
        let order = self.collect_render_order();
        for i in 0..order.len() {
            let (id_i, bounds_i) = order[i];
            let dirty_i = self.window_registry
                .get(&id_i)
                .map(|w| w.needs_repaint())
                .unwrap_or(false);
            if !dirty_i {
                continue;
            }
            for j in (i + 1)..order.len() {
                let (id_j, bounds_j) = order[j];
                if bounds_i.intersects(&bounds_j) {
                    if let Some(w) = self.window_registry.get_mut(&id_j) {
                        if !w.needs_repaint() {
                            w.invalidate();
                        }
                    }
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

        // After dispatching, drain any staged `EnsureVisible` rect from
        // the target widget (e.g. `TextEditor` after a cursor move) and
        // forward it to the nearest enclosing `ScrollView` ancestor.
        // This keeps cursor-into-view automatic for any widget that
        // overrides `take_pending_ensure_visible` without requiring it
        // to hold a typed reference to its parent.
        let pending_rect = self
            .window_registry
            .get_mut(&window_id)
            .and_then(|w| w.take_pending_ensure_visible());
        if let Some(rect) = pending_rect {
            if let Some(sv_id) = self.nearest_scroll_view_ancestor(window_id) {
                if sv_id != window_id {
                    self.route_event_to_window(sv_id, Event::EnsureVisible(rect));
                }
            }
        }

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

    /// Process menu bar popup requests
    fn process_menu_bar_popups(&mut self) {
        // Collect window IDs first to avoid borrow issues
        let window_ids: Vec<WindowId> = self.window_registry.keys().cloned().collect();

        for window_id in window_ids {
            // Poll for pending popup using the trait method
            let pending_popup = {
                if let Some(window) = self.window_registry.get_mut(&window_id) {
                    window.poll_pending_popup()
                } else {
                    None
                }
            };

            if let Some(popup) = pending_popup {
                // Close any existing popup
                if let Some((old_menu_bar_id, old_popup_id)) = self.menu_bar_popup.take() {
                    // Notify the old menu bar that its popup is closing
                    if old_menu_bar_id != window_id {
                        if let Some(old_menu_bar) = self.window_registry.get_mut(&old_menu_bar_id) {
                            old_menu_bar.close_popup_menu();
                        }
                    }
                    self.destroy_window(old_popup_id);
                }

                // Get desktop ID for parenting
                let desktop_id = self.get_active_screen()
                    .and_then(|s| s.root_window)
                    .unwrap_or(WindowId::new());

                // Create the popup window
                let popup_id = self.create_window(Some(desktop_id));
                let popup_bounds = Rect::new(popup.x, popup.y, popup.width, popup.height);
                let mut popup_window = MenuBarPopup::new_with_id(
                    popup_id,
                    popup_bounds,
                    window_id,
                    popup.items,
                );
                // Set the parent so get_global_bounds works correctly
                popup_window.set_parent(Some(desktop_id));

                crate::debug_info!("Creating popup at bounds {:?}", popup_bounds);
                self.set_window_impl(popup_id, Box::new(popup_window));

                // Add popup to desktop's children
                if let Some(desktop) = self.window_registry.get_mut(&desktop_id) {
                    desktop.add_child(popup_id);
                }

                // Bring popup to front
                self.bring_to_front(popup_id);

                // Store the popup reference
                self.menu_bar_popup = Some((window_id, popup_id));

                // Set as active menu for click-outside detection
                self.active_menu = Some(popup_id);

                // Force full repaint so popup is drawn
                self.compositor.dirty.mark_full_repaint();

                crate::debug_info!("Created menu bar popup {:?} for menu bar {:?}", popup_id, window_id);
            }
        }

        // Process popup selections
        self.process_popup_selections();
    }

    /// Process pending popup selections
    fn process_popup_selections(&mut self) {
        // Collect window IDs first to avoid borrow issues
        let window_ids: Vec<WindowId> = self.window_registry.keys().cloned().collect();

        for window_id in window_ids {
            // Poll for pending selection using the trait method
            let selection = {
                if let Some(window) = self.window_registry.get_mut(&window_id) {
                    window.poll_pending_popup_selection()
                } else {
                    None
                }
            };

            if let Some((menu_bar_id, item_index)) = selection {
                crate::debug_info!("Processing popup selection: menu_bar={:?}, item={}", menu_bar_id, item_index);

                // Notify the menu bar of the selection
                if let Some(menu_bar) = self.window_registry.get_mut(&menu_bar_id) {
                    menu_bar.handle_popup_selection(item_index);
                }

                // Close the popup
                if let Some((_, popup_id)) = self.menu_bar_popup.take() {
                    self.active_menu = None;
                    self.destroy_window(popup_id);
                    self.compositor.dirty.mark_full_repaint();
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

        // Process menu bar popup requests
        self.process_menu_bar_popups();

        // Check if mouse moved
        let (mouse_x, mouse_y, buttons) = mouse::get_state();

        // Handle window dragging
        self.handle_dragging(mouse_x, mouse_y, buttons);

        // Update cursor position in compositor (this marks dirty regions).
        // The compositor still works in unsigned screen coordinates; clamp
        // the mouse position to the device for that path only.
        let mouse_moved = self
            .compositor
            .update_cursor(mouse_x.max(0) as usize, mouse_y.max(0) as usize);

        // Cascade invalidation across the z-order so that any window in
        // front of a dirty one (and overlapping it) repaints too. Without
        // this, a dirty inner widget (e.g. an editor in the inactive
        // notepad) paints over the chrome of a later sibling that thinks
        // it's clean, leaving the front frame's title bar overdrawn.
        self.cascade_invalidation();

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
                    let raw_x = start_window.x + delta_x;
                    let raw_y = start_window.y + delta_y;

                    let screen_w = self.graphics_device.width() as i32;
                    let screen_h = self.graphics_device.height() as i32;

                    // Move the window
                    if let Some(win) = self.window_registry.get_mut(&window) {
                        let old_bounds = win.bounds();

                        // Clamp so the title bar is always grabbable.
                        // Partial off-screen drag remains supported; the
                        // clamp only prevents the title bar from leaving
                        // the screen.
                        let new_x = clamp_drag_x(raw_x, old_bounds.width as i32, screen_w);
                        let new_y = clamp_drag_y(raw_y, screen_h);

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
        // If there's an active menu, don't process drag - let the click go to the menu
        if self.active_menu.is_some() {
            crate::debug_info!("start_drag_if_on_title_bar: active_menu present, skipping drag check");
            return;
        }

        // Find the topmost top-level window under the mouse. "Top-level" means
        // a direct child of the screen's root (i.e., a frame on the desktop).
        // Children of top-level windows (like terminals inside frames) are
        // intentionally skipped: drag/resize hit-testing operates on the frame.
        let mut target_window = None;
        let mut target_hit = HitTestResult::None;

        let root_id = self.get_active_screen().and_then(|s| s.root_window);
        let top_level: Vec<WindowId> = root_id
            .and_then(|rid| self.window_registry.get(&rid))
            .map(|root| root.children().to_vec())
            .unwrap_or_default();

        // Walk last-to-first so topmost siblings are tested first.
        for &window_id in top_level.iter().rev() {
            if let Some(window) = self.window_registry.get(&window_id) {
                if !window.visible() {
                    continue;
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
                        self.focus_frame_and_content(window_id);
                    }
                    HitTestResult::Client => {
                        // Clicked in client area - focus the window
                        crate::debug_info!("Clicked client area of {:?}", window_id);
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

    /// Click landed on a frame window — focus its content for keyboard
    /// input. We search depth-first for the first focusable descendant
    /// (e.g. the editor or terminal), since a frame's literal first child
    /// might be a menu bar or other non-focusable widget. Falls back to
    /// the frame itself, which is always focusable.
    fn focus_frame_and_content(&mut self, frame_id: WindowId) {
        let target = self.first_focusable_descendant(frame_id).unwrap_or(frame_id);
        self.focus_window(target);
    }

    /// Depth-first search for the first focusable window in `id`'s subtree
    /// (excluding `id` itself).
    fn first_focusable_descendant(&self, id: WindowId) -> Option<WindowId> {
        let window = self.window_registry.get(&id)?;
        for &child_id in window.children() {
            if let Some(child) = self.window_registry.get(&child_id) {
                if child.can_focus() {
                    return Some(child_id);
                }
                if let Some(found) = self.first_focusable_descendant(child_id) {
                    return Some(found);
                }
            }
        }
        None
    }

    /// Bring a window to the front of its parent's children list (i.e. the
    /// top of its sibling z-order), and recursively do the same for every
    /// ancestor up to the root. This is the single source of truth for
    /// z-order — rendering and hit-testing both read it from `children()`.
    pub fn bring_to_front(&mut self, window_id: WindowId) {
        let mut current = window_id;
        loop {
            let parent_id = match self.window_registry.get(&current).and_then(|w| w.parent()) {
                Some(p) => p,
                None => break, // reached the root
            };
            if let Some(parent) = self.window_registry.get_mut(&parent_id) {
                // remove_child + add_child moves the entry to the end of the
                // children Vec, which is the top of the local z-order.
                parent.remove_child(current);
                parent.add_child(current);
            }
            current = parent_id;
        }

        // Sibling reordering means areas previously occluded may now be
        // visible (and vice versa), so force a full repaint.
        self.compositor.dirty.mark_full_repaint();
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

    // Menu Management

    /// Set the active popup menu
    pub fn set_active_menu(&mut self, menu_id: Option<WindowId>) {
        self.active_menu = menu_id;
    }

    /// Get the active popup menu
    pub fn get_active_menu(&self) -> Option<WindowId> {
        self.active_menu
    }

    /// Close the active menu if one is open
    pub fn close_active_menu(&mut self) {
        if let Some(menu_id) = self.active_menu.take() {
            // If this is a menu bar popup, notify the menu bar
            if let Some((menu_bar_id, popup_id)) = self.menu_bar_popup.take() {
                if popup_id == menu_id {
                    if let Some(menu_bar) = self.window_registry.get_mut(&menu_bar_id) {
                        menu_bar.close_popup_menu();
                    }
                }
            }
            self.destroy_window(menu_id);
            self.compositor.dirty.mark_full_repaint();
        }
    }

    // Taskbar Management

    /// Set the taskbar window ID
    pub fn set_taskbar_id(&mut self, taskbar_id: Option<WindowId>) {
        self.taskbar_id = taskbar_id;
    }

    /// Get the taskbar window ID
    pub fn get_taskbar_id(&self) -> Option<WindowId> {
        self.taskbar_id
    }

    // Modal Dialog Management

    /// Set the modal dialog window ID
    /// When set, events will only be routed to the modal dialog and its children
    pub fn set_modal_dialog(&mut self, dialog_id: Option<WindowId>) {
        self.modal_dialog = dialog_id;
        if let Some(id) = dialog_id {
            // Focus the modal dialog
            self.focus_window(id);
        }
    }

    /// Get the current modal dialog window ID
    pub fn get_modal_dialog(&self) -> Option<WindowId> {
        self.modal_dialog
    }

    /// Check if a modal dialog is currently active
    pub fn has_modal_dialog(&self) -> bool {
        self.modal_dialog.is_some()
    }

    /// Check if a window is part of the modal dialog (is the dialog or a child of it)
    fn is_modal_window(&self, window_id: WindowId) -> bool {
        if let Some(modal_id) = self.modal_dialog {
            if window_id == modal_id {
                return true;
            }
            // Check if window is a descendant of the modal dialog
            let mut current = window_id;
            while let Some(window) = self.window_registry.get(&current) {
                if let Some(parent) = window.parent() {
                    if parent == modal_id {
                        return true;
                    }
                    current = parent;
                } else {
                    break;
                }
            }
        }
        false
    }

    /// Get all frame windows with their titles
    /// Returns a list of (frame_id, title) pairs
    /// Only returns windows that have a title (i.e., FrameWindows)
    pub fn get_frame_windows(&self) -> Vec<(WindowId, alloc::string::String)> {
        use alloc::string::String;

        let mut result = Vec::new();

        for (&window_id, window) in &self.window_registry {
            // Only include windows that have a title (FrameWindows)
            if let Some(title) = window.window_title() {
                // Skip the taskbar
                if Some(window_id) == self.taskbar_id {
                    continue;
                }
                result.push((window_id, String::from(title)));
            }
        }

        result
    }

    /// Get the screen dimensions
    pub fn screen_dimensions(&self) -> (u32, u32) {
        (
            self.graphics_device.width() as u32,
            self.graphics_device.height() as u32,
        )
    }

    /// Borrow a single window from the registry mutably as a `&mut dyn
    /// Window` and run `f` against it. Returns `true` if the window was
    /// found, `false` otherwise.
    ///
    /// While `f` runs, this manager is also exposed as the "active
    /// manager" via [`with_active_manager`]. That lets the closure (or
    /// methods called from inside it — notably layout containers'
    /// `set_bounds` overrides) reach back into the manager to mutate
    /// other windows. The window passed to `f` is detached from the
    /// registry for the duration of the call, so it cannot itself be
    /// re-entered through `with_active_manager` until `f` returns.
    pub fn with_window_mut<F>(&mut self, window_id: WindowId, f: F) -> bool
    where
        F: FnOnce(&mut dyn Window),
    {
        let mut window = match self.window_registry.remove(&window_id) {
            Some(w) => w,
            None => return false,
        };

        let self_ptr = self as *mut WindowManager;
        let prev = ACTIVE_MANAGER.swap(self_ptr, Ordering::SeqCst);

        f(&mut *window);

        ACTIVE_MANAGER.store(prev, Ordering::SeqCst);
        self.window_registry.insert(window_id, window);
        true
    }
}

/// Pointer to the manager whose `with_window_mut` is currently on the
/// stack, or null if no `with_window_mut` call is active. Used by
/// layout containers to write children's bounds back into the manager
/// from inside a `set_bounds` override.
///
/// SAFETY: only set/read inside `WindowManager::with_window_mut`. The
/// pointer is valid for the duration of that call because the closure
/// runs synchronously on the same thread; interrupts that touch the
/// manager are blocked by the surrounding `with_window_manager`'s
/// `InterruptGuard`.
static ACTIVE_MANAGER: AtomicPtr<WindowManager> = AtomicPtr::new(core::ptr::null_mut());

/// Run `f` against the currently-active `WindowManager`. Returns
/// `Some(f's result)` when called from inside a
/// `WindowManager::with_window_mut` callback, `None` otherwise.
///
/// Callers must not invoke `with_window_mut` on the same window id
/// that is currently being mutated — that window is removed from the
/// registry while its closure runs and would not be found.
pub fn with_active_manager<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut WindowManager) -> R,
{
    let ptr = ACTIVE_MANAGER.load(Ordering::SeqCst);
    if ptr.is_null() {
        return None;
    }
    // SAFETY: see ACTIVE_MANAGER doc comment. The pointer was set by
    // `with_window_mut` and we are still inside that call (or any
    // recursive `with_window_mut` it triggered).
    let manager = unsafe { &mut *ptr };
    Some(f(manager))
}
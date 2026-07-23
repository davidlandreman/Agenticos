//! Window Manager - Central coordinator for all windows and screens

use super::cursor::CursorRenderer;
use super::renderer::{
    boot_request, boot_strict, invalid_boot_request, select_renderer, RendererKind,
    RendererSelection, RendererState, RetainedRenderer, SurfaceCanvas,
};
use super::types::{
    clamp_drag_x, clamp_drag_y, HitTestResult, InteractionState, ResizeEdge, MIN_WINDOW_HEIGHT,
};
use super::windows::MenuBarPopup;
use super::{
    Event, EventResult, GraphicsDevice, KeyboardEvent, MouseEvent, MouseEventType, Point, Rect,
    Screen, ScreenId, ScreenMode, Window, WindowId,
};
use crate::drivers::mouse;
use crate::graphics::composition::{
    timestamp_cycles, ClientGlFrame, ClientGlId, ClientGlInfo, RenderStats,
};
use crate::graphics::compositor::Compositor;
use crate::graphics::present::{BootFramebufferPresenter, Presenter};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicPtr, Ordering};

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
    graphics_device: Box<dyn GraphicsDevice>,
    /// Compositor for dirty tracking and cursor overlay
    compositor: Compositor,
    /// Current window interaction state (dragging, resizing)
    interaction_state: InteractionState,
    /// Window that received the current left-button press. Motion and release
    /// stay routed here until the button is released, even outside its bounds.
    pointer_capture: Option<WindowId>,
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
    /// Selected rendering implementation. Legacy remains a real sibling and
    /// is installed atomically if retained initialization cannot complete.
    renderer: RendererState,
    renderer_selection: RendererSelection,
    last_render_stats: RenderStats,
}

impl WindowManager {
    /// Create a new window manager with the given graphics device
    pub fn new(graphics_device: Box<dyn GraphicsDevice>) -> Self {
        let width = graphics_device.width() as u32;
        let height = graphics_device.height() as u32;

        let request = boot_request();
        let strict = boot_strict();
        if invalid_boot_request() {
            crate::debug_warn!("compositor_request=invalid fallback=legacy");
        }

        let mut gpu_candidate = if matches!(
            request,
            super::renderer::CompositorRequest::Gpu | super::renderer::CompositorRequest::Auto
        ) {
            match RetainedRenderer::new_gpu(width, height) {
                Ok(renderer) => Some(renderer),
                Err(error) => {
                    crate::debug_warn!("VirGL compositor initialization failed: {:?}", error);
                    None
                }
            }
        } else {
            None
        };
        let mut retained_candidate =
            if request == super::renderer::CompositorRequest::Legacy || gpu_candidate.is_some() {
                None
            } else {
                RetainedRenderer::new(width, height).ok()
            };
        let selection = select_renderer(
            request,
            strict,
            retained_candidate.is_some(),
            gpu_candidate.is_some(),
        )
        .unwrap_or_else(|error| panic!("strict compositor initialization failed: {:?}", error));
        crate::window::theme::init_boot_policy(selection.selected);
        let renderer = match selection.selected {
            RendererKind::RetainedCpu => RendererState::Retained(
                retained_candidate
                    .take()
                    .expect("retained candidate selected without initialization"),
            ),
            RendererKind::Legacy => RendererState::Legacy,
            RendererKind::Virgl => RendererState::Retained(
                gpu_candidate
                    .take()
                    .expect("VirGL candidate selected without initialization"),
            ),
        };
        let presenter = match &renderer {
            RendererState::Retained(renderer) if renderer.uses_direct_scanout() => "virgl-direct",
            RendererState::Retained(renderer) if renderer.has_virtio_presenter() => "virtio-gpu-2d",
            _ => "boot-framebuffer",
        };
        crate::debug_info!(
            "compositor requested={} selected={} engine={} presenter={} strict={} fallback={}",
            selection.requested.as_str(),
            selection.selected.as_str(),
            match selection.selected {
                RendererKind::RetainedCpu => "cpu",
                RendererKind::Legacy => "legacy",
                RendererKind::Virgl => "virgl",
            },
            presenter,
            selection.strict,
            selection.fallback_reason.unwrap_or("none"),
        );

        let mut wm = WindowManager {
            screens: Vec::new(),
            active_screen: ScreenId(0), // Will be set when first screen is created
            window_registry: BTreeMap::new(),
            focus_stack: Vec::new(),
            graphics_device,
            compositor: Compositor::new(width, height),
            interaction_state: InteractionState::Idle,
            pointer_capture: None,
            last_mouse_buttons: 0,
            cursor: CursorRenderer::new(),
            active_menu: None,
            taskbar_id: None,
            modal_dialog: None,
            menu_bar_popup: None,
            renderer,
            renderer_selection: selection,
            last_render_stats: RenderStats::default(),
        };

        // Create default text screen
        let default_screen = wm.create_screen(ScreenMode::Text);
        wm.active_screen = default_screen;

        wm
    }

    pub const fn renderer_selection(&self) -> RendererSelection {
        self.renderer_selection
    }

    /// Apply a runtime theme request as one window-manager transaction.
    pub fn apply_theme_request(
        &mut self,
        requested: super::theme::ThemeRequest,
    ) -> Result<(super::theme::ThemeSelection, bool), i64> {
        if let Some(kind) = requested.explicit_kind() {
            if super::theme::spec_for(kind).requires_modern_renderer
                && self.renderer_selection.selected == RendererKind::Legacy
            {
                return Err(crate::userland::abi::ENOTSUP);
            }
        }
        let selection = super::theme::select_theme(requested, self.renderer_selection.selected);
        let old_kind = super::theme::active();
        if old_kind == selection.selected {
            return Ok((selection, false));
        }

        let old_metrics = super::theme::metrics_for(old_kind);
        let new_metrics = super::theme::metrics_for(selection.selected);
        super::theme::activate(selection.selected);
        let (screen_width, screen_height) = self.screen_dimensions();
        let frame_ids: Vec<WindowId> = self
            .window_registry
            .iter()
            .filter_map(|(&id, window)| window.window_title().map(|_| id))
            .collect();

        for frame_id in &frame_ids {
            if let Some(window) = self.window_registry.get_mut(frame_id) {
                if let Some(frame) = window.as_frame_window_mut() {
                    frame.apply_theme(old_metrics, new_metrics, selection.selected);
                    let mut bounds = frame.bounds();
                    bounds.x = bounds
                        .x
                        .clamp(0, screen_width.saturating_sub(bounds.width) as i32);
                    bounds.y = bounds
                        .y
                        .clamp(0, screen_height.saturating_sub(bounds.height) as i32);
                    frame.set_bounds(bounds);
                }
            }
        }
        for frame_id in frame_ids {
            self.update_children_for_resized_window(frame_id);
        }
        for window in self.window_registry.values_mut() {
            window.invalidate();
        }
        self.force_full_repaint();
        Ok((selection, true))
    }

    pub fn set_desktop_wallpaper(&mut self, wallpaper: Option<Vec<u8>>) -> bool {
        let root = self
            .get_active_screen()
            .and_then(|screen| screen.root_window);
        let Some(root) = root else {
            return false;
        };
        let changed = self
            .window_registry
            .get_mut(&root)
            .and_then(|window| window.as_desktop_window_mut())
            .map(|desktop| desktop.set_wallpaper(wallpaper))
            .is_some();
        if changed {
            self.force_full_repaint();
        }
        changed
    }
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub const fn render_stats(&self) -> RenderStats {
        self.last_render_stats
    }

    pub fn create_gl_client(&mut self, width: u32, height: u32) -> Result<ClientGlId, i64> {
        if self.renderer_selection.selected != RendererKind::Virgl
            || !self.renderer_selection.strict
        {
            return Err(crate::userland::abi::ENOTSUP);
        }
        let RendererState::Retained(renderer) = &mut self.renderer else {
            return Err(crate::userland::abi::ENOTSUP);
        };
        renderer
            .create_gl_client(width, height)
            .map_err(|_| crate::userland::abi::EIO)
    }

    pub fn submit_gl_client_frame(
        &mut self,
        surface_id: WindowId,
        id: ClientGlId,
        frame: ClientGlFrame,
    ) -> Result<(), i64> {
        let RendererState::Retained(renderer) = &mut self.renderer else {
            return Err(crate::userland::abi::ENOTSUP);
        };
        renderer
            .submit_gl_client_frame(id, frame)
            .map_err(|_| crate::userland::abi::EIO)?;
        if let Some((_, bounds)) = self
            .collect_render_order()
            .into_iter()
            .find(|(window_id, _)| *window_id == surface_id)
        {
            self.compositor.dirty.mark_dirty(bounds);
        } else {
            self.compositor.dirty.mark_full_repaint();
        }
        Ok(())
    }

    pub fn gl_client_info(&self, id: ClientGlId) -> Option<ClientGlInfo> {
        let RendererState::Retained(renderer) = &self.renderer else {
            return None;
        };
        renderer.gl_client_info(id)
    }

    pub fn destroy_gl_client(&mut self, id: ClientGlId) -> Result<(), i64> {
        let RendererState::Retained(renderer) = &mut self.renderer else {
            return Err(crate::userland::abi::ENOTSUP);
        };
        renderer
            .destroy_gl_client(id)
            .map_err(|_| crate::userland::abi::EIO)
    }

    #[expect(dead_code, reason = "intentional kernel API surface")]
    pub fn compositor_capabilities(&self) -> super::CompositorCapabilities {
        match self.renderer.kind() {
            RendererKind::Legacy => super::CompositorCapabilities {
                opacity: false,
                translation: false,
                backdrop_sample: false,
                accelerated: false,
            },
            RendererKind::RetainedCpu => super::CompositorCapabilities {
                opacity: true,
                translation: true,
                backdrop_sample: true,
                accelerated: false,
            },
            RendererKind::Virgl => super::CompositorCapabilities {
                opacity: true,
                translation: true,
                backdrop_sample: false,
                accelerated: true,
            },
        }
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
        let children: Vec<WindowId> = self
            .window_registry
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
        if self.pointer_capture == Some(id) {
            self.pointer_capture = None;
        }

        // Removing a window vacates its screen region; the compositor must
        // recompose so the desktop (or windows beneath it) shows through.
        // Without this, a window closed via a client-driven path
        // (`gui_win_destroy`, process-exit `cleanup_process`, or the ring-3
        // shell's `gui_shell_window_action` close) leaves stale pixels on
        // screen until an unrelated event forces a repaint. `mark_full_repaint`
        // is idempotent, so the recursion above coalesces to one repaint.
        self.compositor.dirty.mark_full_repaint();
    }

    // Focus management

    /// Give focus to a specific window.
    ///
    /// Also brings the window (and its ancestors) to the front of their parent's
    /// children list, and sets the visual focus state on a parent frame so the
    /// chrome displays as active. Most callers want this — focus implies the
    /// window should be visible and on top.
    pub fn focus_window(&mut self, id: WindowId) {
        let can_focus = self
            .window_registry
            .get(&id)
            .map(|w| w.can_focus() && self.window_effectively_visible(id))
            .unwrap_or(false);
        if !can_focus {
            return;
        }

        // Unfocus the previously focused window and its parent frame (if any).
        if let Some(&current_focus) = self.focus_stack.last() {
            let current_parent = self
                .window_registry
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

    fn window_effectively_visible(&self, id: WindowId) -> bool {
        let mut current = Some(id);
        while let Some(window_id) = current {
            let Some(window) = self.window_registry.get(&window_id) else {
                return false;
            };
            if !window.visible() {
                return false;
            }
            current = window.parent();
        }
        true
    }

    /// The desktop area available to ordinary frames. The current shell has
    /// one bottom-docked taskbar; if it is absent or hidden, use the screen.
    pub fn desktop_work_area(&self) -> Rect {
        let (width, height) = self.screen_dimensions();
        let mut work_area = Rect::new(0, 0, width, height);
        let Some(taskbar_id) = self.taskbar_id else {
            return work_area;
        };
        let Some(taskbar) = self.window_registry.get(&taskbar_id) else {
            return work_area;
        };
        if !taskbar.visible() {
            return work_area;
        }
        if let Some(bounds) = self.get_global_bounds(taskbar_id) {
            if bounds.y > 0 && bounds.y < height as i32 {
                work_area.height = bounds.y as u32;
            }
        }
        work_area
    }

    /// Hide a resizable frame while retaining its registry/taskbar identity.
    pub fn minimize_frame(&mut self, frame_id: WindowId) -> bool {
        let can_minimize = self
            .window_registry
            .get(&frame_id)
            .and_then(|window| window.as_frame_window())
            .is_some_and(|frame| frame.is_resizable() && !frame.is_minimized());
        if !can_minimize {
            return false;
        }

        let subtree = self.subtree_ids(frame_id);
        for id in &subtree {
            if let Some(window) = self.window_registry.get_mut(id) {
                window.set_focus(false);
            }
        }
        self.focus_stack.retain(|id| !subtree.contains(id));
        if self.pointer_capture.is_some_and(|id| subtree.contains(&id)) {
            self.pointer_capture = None;
        }
        if matches!(
            self.interaction_state,
            InteractionState::Dragging { window, .. }
                | InteractionState::Resizing { window, .. }
                if window == frame_id
        ) {
            self.interaction_state = InteractionState::Idle;
        }
        let changed = self
            .window_registry
            .get_mut(&frame_id)
            .and_then(|window| window.as_frame_window_mut())
            .is_some_and(|frame| frame.set_minimized(true));
        if !changed {
            return false;
        }
        self.compositor.dirty.mark_full_repaint();

        let next = self.next_visible_frame(frame_id);
        if let Some(next) = next {
            self.focus_frame_and_content(next);
        }
        true
    }

    /// Toggle a resizable frame between its normal bounds and the work area.
    pub fn toggle_maximize_frame(&mut self, frame_id: WindowId) -> bool {
        let work_area = self.desktop_work_area();
        let changed = self
            .window_registry
            .get_mut(&frame_id)
            .and_then(|window| window.as_frame_window_mut())
            .and_then(|frame| frame.toggle_maximized(work_area));
        let Some((old_bounds, new_bounds)) = changed else {
            return false;
        };
        self.update_children_for_resized_window(frame_id);
        self.compositor
            .dirty
            .mark_dirty(old_bounds.union(&new_bounds));
        self.focus_frame_and_content(frame_id);
        true
    }

    /// Restore a minimized frame if necessary, then raise and focus it.
    pub fn activate_frame(&mut self, frame_id: WindowId) -> bool {
        let is_frame = self
            .window_registry
            .get(&frame_id)
            .and_then(|window| window.as_frame_window())
            .is_some();
        if !is_frame {
            return false;
        }
        let restored = self
            .window_registry
            .get_mut(&frame_id)
            .and_then(|window| window.as_frame_window_mut())
            .is_some_and(|frame| frame.set_minimized(false));
        if restored {
            self.compositor.dirty.mark_full_repaint();
        }
        self.focus_frame_and_content(frame_id);
        true
    }

    fn subtree_ids(&self, root: WindowId) -> Vec<WindowId> {
        fn collect(manager: &WindowManager, id: WindowId, ids: &mut Vec<WindowId>) {
            ids.push(id);
            if let Some(window) = manager.window_registry.get(&id) {
                for &child in window.children() {
                    collect(manager, child, ids);
                }
            }
        }
        let mut ids = Vec::new();
        if self.window_registry.contains_key(&root) {
            collect(self, root, &mut ids);
        }
        ids
    }

    fn next_visible_frame(&self, excluded: WindowId) -> Option<WindowId> {
        let root_id = self.get_active_screen()?.root_window?;
        let root = self.window_registry.get(&root_id)?;
        root.children().iter().rev().copied().find(|&id| {
            id != excluded
                && Some(id) != self.taskbar_id
                && self
                    .window_registry
                    .get(&id)
                    .is_some_and(|window| window.visible() && window.as_frame_window().is_some())
        })
    }

    // Event routing

    /// Process keyboard interrupt data

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
                crate::debug_trace!(
                    "Active menu {:?} bounds: {:?}, mouse: {:?}",
                    menu_id,
                    menu_bounds,
                    global_pos
                );
                // Check if this is a button down event
                if matches!(event.event_type, MouseEventType::ButtonDown) {
                    if !menu_bounds.contains_point(global_pos) {
                        // Click outside menu - close it and don't route the click
                        // (user's intent was to close the menu, not click what's underneath)
                        crate::debug_trace!("Click outside menu - closing");
                        self.pointer_capture = None;
                        self.close_active_menu();
                        return;
                    } else {
                        crate::debug_trace!("Click inside menu {:?} at {:?}", menu_id, global_pos);
                    }
                }
            }
        }

        // A fresh press replaces any stale capture. Once a window receives
        // that press, keep routing motion and the matching release to it. This
        // is required for narrow draggable controls such as scrollbar thumbs:
        // ordinary per-event hit-testing would otherwise redirect motion as
        // soon as the pointer slipped outside the original target.
        if matches!(event.event_type, MouseEventType::ButtonDown) && event.buttons.left {
            self.pointer_capture = None;
        }
        if matches!(
            event.event_type,
            MouseEventType::Move | MouseEventType::ButtonUp
        ) {
            if let Some(captured) = self.pointer_capture {
                if matches!(event.event_type, MouseEventType::Move) && !event.buttons.left {
                    // Recover if a release was lost before it reached the
                    // queue; do not leave routing permanently captured.
                    self.pointer_capture = None;
                } else if let Some(bounds) = self.get_global_bounds(captured) {
                    let mut local_event = event;
                    local_event.position =
                        Point::new(global_pos.x - bounds.x, global_pos.y - bounds.y);
                    self.route_event_to_window(captured, Event::Mouse(local_event));
                    if matches!(event.event_type, MouseEventType::ButtonUp) && !event.buttons.left {
                        self.pointer_capture = None;
                    }
                    return;
                } else {
                    self.pointer_capture = None;
                }
            }
        }

        // If modal dialog is active, only route within the modal subtree.
        if let Some(modal_id) = self.modal_dialog {
            if let Some((hit_id, hit_bounds)) = self.topmost_at(modal_id, global_pos, 0, 0) {
                if matches!(event.event_type, MouseEventType::ButtonDown) && event.buttons.left {
                    self.pointer_capture = Some(hit_id);
                }
                let mut local_event = event;
                local_event.position =
                    Point::new(global_pos.x - hit_bounds.x, global_pos.y - hit_bounds.y);
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
                        let sv_bounds = self.get_global_bounds(sv_id).unwrap_or(hit_bounds);
                        let mut local_event = event;
                        local_event.position =
                            Point::new(global_pos.x - sv_bounds.x, global_pos.y - sv_bounds.y);
                        self.route_event_to_window(sv_id, Event::Mouse(local_event));
                        return;
                    }
                }

                if matches!(event.event_type, MouseEventType::ButtonDown) && event.buttons.left {
                    self.pointer_capture = Some(hit_id);
                }
                let mut local_event = event;
                local_event.position =
                    Point::new(global_pos.x - hit_bounds.x, global_pos.y - hit_bounds.y);
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
    fn topmost_at(
        &self,
        id: WindowId,
        point: Point,
        parent_x: i32,
        parent_y: i32,
    ) -> Option<(WindowId, Rect)> {
        let window = self.window_registry.get(&id)?;
        if !window.visible() {
            return None;
        }
        let local = window.bounds();
        let abs = Rect::new(
            local.x + parent_x,
            local.y + parent_y,
            local.width,
            local.height,
        );
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
        let abs = Rect::new(
            local.x + parent_x,
            local.y + parent_y,
            local.width,
            local.height,
        );
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
            let dirty_i = self
                .window_registry
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
        crate::debug_trace!(
            "route_event_to_window: window={:?}, event={:?}",
            window_id,
            event
        );

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
                let desktop_id = self
                    .get_active_screen()
                    .and_then(|s| s.root_window)
                    .unwrap_or(WindowId::new());

                // Create the popup window
                let popup_id = self.create_window(Some(desktop_id));
                let popup_bounds = Rect::new(popup.x, popup.y, popup.width, popup.height);
                let mut popup_window =
                    MenuBarPopup::new_with_id(popup_id, popup_bounds, window_id, popup.items);
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

                crate::debug_info!(
                    "Created menu bar popup {:?} for menu bar {:?}",
                    popup_id,
                    window_id
                );
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
                crate::debug_info!(
                    "Processing popup selection: menu_bar={:?}, item={}",
                    menu_bar_id,
                    item_index
                );

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
        // Process menu bar popup requests
        self.process_menu_bar_popups();

        // Check if mouse moved
        let (mouse_x, mouse_y, buttons) = mouse::get_state();

        // Handle window dragging
        self.handle_dragging(mouse_x, mouse_y, buttons);

        // Update the compositor cursor position. Legacy deliberately keeps
        // cursor footprints out of repaint damage; retained consumes the
        // previous position below as composition/presentation-only damage.
        // Clamp to unsigned coordinates for the shared cursor state.
        let previous_cursor_position = self.compositor.cursor_position();
        let mouse_moved = self
            .compositor
            .update_cursor(mouse_x.max(0) as usize, mouse_y.max(0) as usize);
        let previous_cursor_position = mouse_moved.then(|| {
            Point::new(
                previous_cursor_position.0.min(i32::MAX as usize) as i32,
                previous_cursor_position.1.min(i32::MAX as usize) as i32,
            )
        });

        // Cascade invalidation across the z-order so that any window in
        // front of a dirty one (and overlapping it) repaints too. Without
        // this, a dirty inner widget (e.g. an editor in the inactive
        // notepad) paints over the chrome of a later sibling that thinks
        // it's clean, leaving the front frame's title bar overdrawn.
        self.cascade_invalidation();

        // Mark every invalidated window's absolute bounds dirty (or its
        // narrowed `dirty_rect_hint` when one is supplied). See
        // `mark_dirty_for_invalidated_windows` for why this walks render
        // order rather than the flat registry.
        self.mark_dirty_for_invalidated_windows();

        // Early exit if nothing needs rendering
        if !self.compositor.needs_render() && !mouse_moved {
            // An idle sample is explicitly zero-work. Do not leave the prior
            // active frame's counters visible to diagnostics.
            self.last_render_stats = RenderStats::default();
            return; // Nothing to update - this is the key optimization!
        }

        if matches!(self.renderer, RendererState::Retained(_)) {
            let state = core::mem::replace(&mut self.renderer, RendererState::Legacy);
            let RendererState::Retained(mut retained) = state else {
                unreachable!()
            };
            match self.render_retained(&mut retained, mouse_x, mouse_y, previous_cursor_position) {
                Ok(()) => {
                    self.renderer = RendererState::Retained(retained);
                    self.compositor.end_frame();
                }
                Err(error) => {
                    if self.renderer_selection.selected == RendererKind::Virgl
                        && self.renderer_selection.strict
                    {
                        panic!("strict VirGL compositor failed at runtime: {:?}", error);
                    }
                    if self.renderer_selection.selected == RendererKind::Virgl {
                        // Release the sole VirtIO-GPU owner before the CPU
                        // renderer probes an optional 2D presenter.
                        drop(retained);
                        match RetainedRenderer::new(
                            self.graphics_device.width() as u32,
                            self.graphics_device.height() as u32,
                        ) {
                            Ok(cpu) => {
                                crate::debug_warn!(
                                    "VirGL compositor failed: {:?}; fallback=retained CPU",
                                    error
                                );
                                self.renderer = RendererState::Retained(cpu);
                                self.renderer_selection.selected = RendererKind::RetainedCpu;
                                self.renderer_selection.fallback_reason =
                                    Some("VirGL runtime failure");
                            }
                            Err(_) => {
                                crate::debug_warn!(
                                    "VirGL compositor and retained CPU fallback failed: {:?}; fallback=legacy",
                                    error
                                );
                                self.renderer_selection.selected = RendererKind::Legacy;
                                self.renderer_selection.fallback_reason =
                                    Some("VirGL and retained runtime failure");
                                let _ =
                                    self.apply_theme_request(super::theme::ThemeRequest::Classic);
                                crate::system_control::queue_theme_publication(
                                    super::theme::ThemeKind::Classic,
                                );
                            }
                        }
                    } else {
                        crate::debug_warn!(
                            "retained compositor failed: {:?}; fallback=legacy",
                            error
                        );
                        self.renderer_selection.selected = RendererKind::Legacy;
                        self.renderer_selection.fallback_reason = Some("retained runtime failure");
                        let _ = self.apply_theme_request(super::theme::ThemeRequest::Classic);
                        crate::system_control::queue_theme_publication(
                            super::theme::ThemeKind::Classic,
                        );
                    }
                    self.compositor.end_frame();
                    self.compositor.dirty.mark_full_repaint();
                    self.render();
                }
            }
            return;
        }

        let old_cursor_bounds = self.cursor.saved_bounds();

        // Restore old cursor background before any rendering
        // This erases the cursor from its old position
        self.cursor.restore_background(&mut *self.graphics_device);

        // Determine if we need full repaint or can do partial
        let full_repaint = self.compositor.dirty.needs_full_repaint();

        if full_repaint {
            crate::debug_trace!("Full frame render required");
            self.graphics_device
                .clear(crate::graphics::color::Color::BLACK);

            // When doing a full repaint, all windows must repaint
            // Otherwise windows that don't think they need repainting will skip
            // and leave holes where the screen was cleared
            for window in self.window_registry.values_mut() {
                window.invalidate();
            }
        }

        // Render the active screen's windows once per dirty region.
        //
        // Per-region rendering avoids the bounding-box clip leak: with two
        // disjoint dirty rects, the bounding box covers the corridor
        // between them, and any window whose bounds intersect the bbox
        // (notably the full-screen desktop) would blit across the gap,
        // overwriting valid pixels of unrelated windows. Iterating each
        // region with `clip = region ∩ bounds` keeps paint confined to
        // actually-dirty pixels.
        let regions: alloc::vec::Vec<Rect> = self.compositor.dirty.dirty_regions().collect();
        if let Some(screen) = self.get_active_screen() {
            if let Some(root_id) = screen.root_window {
                for region in &regions {
                    self.render_window_tree_in_region(root_id, *region, 0, 0);
                }
            }
        }

        // Save background at new cursor position, then draw cursor
        self.cursor
            .save_background(mouse_x, mouse_y, &*self.graphics_device);
        self.cursor
            .draw(mouse_x, mouse_y, &mut *self.graphics_device);

        // Presentation damage is broader than repaint damage: moving the
        // cursor restores its old background and draws at its new position
        // without asking the windows underneath to repaint.
        let screen_bounds = Rect::new(
            0,
            0,
            self.graphics_device.width() as u32,
            self.graphics_device.height() as u32,
        );
        let mut present_regions = Vec::new();
        for region in &regions {
            Self::add_present_region(&mut present_regions, *region, screen_bounds);
        }
        if let Some(old_cursor_bounds) = old_cursor_bounds {
            Self::add_present_region(&mut present_regions, old_cursor_bounds, screen_bounds);
        }
        Self::add_present_region(
            &mut present_regions,
            CursorRenderer::bounds_at(mouse_x, mouse_y),
            screen_bounds,
        );

        // End frame and clear dirty tracking
        self.compositor.end_frame();

        // Present only pixels touched by window repainting or cursor overlay.
        self.graphics_device.flush_regions(&present_regions);
        for window in self.window_registry.values_mut() {
            window.clear_composition_dirty();
        }
        self.last_render_stats = RenderStats {
            frames: 1,
            output_pixels_damaged: present_regions.iter().map(Rect::area).sum(),
            presents: 1,
            ..RenderStats::default()
        };
    }

    fn render_retained(
        &mut self,
        retained: &mut RetainedRenderer,
        mouse_x: i32,
        mouse_y: i32,
        previous_cursor_position: Option<Point>,
    ) -> Result<(), super::renderer::RetainedRendererError> {
        let full_repaint = self.compositor.dirty.needs_full_repaint();
        if full_repaint {
            for window in self.window_registry.values_mut() {
                window.invalidate();
            }
        }

        let regions: Vec<Rect> = self.compositor.dirty.dirty_regions().collect();
        let direct_scanout = retained.uses_direct_scanout();
        if direct_scanout
            && !full_repaint
            && regions.is_empty()
            && previous_cursor_position.is_some()
        {
            let presentation_started = timestamp_cycles();
            let cursor_pixels = retained
                .hardware_cursor_needs_image()
                .then(CursorRenderer::hardware_argb_64);
            retained.update_hardware_cursor(
                mouse_x.max(0) as u32,
                mouse_y.max(0) as u32,
                cursor_pixels.as_deref(),
            )?;
            self.last_render_stats = RenderStats {
                frames: 1,
                presents: 1,
                cursor_updates: 1,
                presentation_cycles: timestamp_cycles().saturating_sub(presentation_started),
                ..RenderStats::default()
            };
            for window in self.window_registry.values_mut() {
                window.clear_composition_dirty();
            }
            return Ok(());
        }
        let roots = self.collect_layer_roots();
        let decorated_roots: Vec<(WindowId, Rect)> = roots
            .iter()
            .map(|(id, bounds)| {
                let insets = self
                    .window_registry
                    .get(id)
                    .map(|window| window.decoration_insets())
                    .unwrap_or_default();
                (*id, insets.expand(*bounds))
            })
            .collect();
        let root_ids: Vec<WindowId> = roots.iter().map(|(id, _)| *id).collect();
        retained.retain_roots(&root_ids);
        let mut compose_regions = regions.clone();
        let screen_bounds = Rect::new(
            0,
            0,
            self.graphics_device.width() as u32,
            self.graphics_device.height() as u32,
        );

        // Cursor motion is composition/presentation damage, not surface
        // repaint damage. Recompose the old footprint to erase the previous
        // overlay, then present both it and the newly drawn footprint. Keep
        // these rectangles out of `regions` so cursor-only movement never
        // asks widgets to repaint or rerasterizes their retained surfaces.
        if !direct_scanout {
            if let Some(previous) = previous_cursor_position {
                Self::add_present_region(
                    &mut compose_regions,
                    CursorRenderer::bounds_at(previous.x, previous.y),
                    screen_bounds,
                );
                Self::add_present_region(
                    &mut compose_regions,
                    CursorRenderer::bounds_at(mouse_x, mouse_y),
                    screen_bounds,
                );
            }
        }

        let mut windows_rasterized = 0u64;
        let mut surface_pixels_updated = 0u64;
        let mut surface_raster_cycles = 0u64;
        for ((root, _), (_, bounds)) in roots.iter().zip(decorated_roots.iter()) {
            let previous = retained.previous_bounds(*root);
            let (surface_id, created) = retained.ensure_surface(*root, *bounds)?;
            if created {
                Self::add_present_region(&mut compose_regions, *bounds, screen_bounds);
            } else if let Some(previous) = previous {
                if previous != *bounds {
                    Self::add_present_region(
                        &mut compose_regions,
                        previous.union(bounds),
                        screen_bounds,
                    );
                }
            }
            let subtree_dirty = self.subtree_needs_repaint(*root, &root_ids);
            let moved_only = !created
                && previous
                    .map(|old| {
                        old.width == bounds.width
                            && old.height == bounds.height
                            && (old.x != bounds.x || old.y != bounds.y)
                    })
                    .unwrap_or(false)
                && !self.descendants_need_repaint(*root, &root_ids);

            if moved_only {
                if let Some(window) = self.window_registry.get_mut(root) {
                    window.clear_needs_repaint();
                }
                continue;
            }
            if !created && !full_repaint && !subtree_dirty {
                continue;
            }

            let repaint_regions: Vec<Rect> = if created || full_repaint {
                alloc::vec![*bounds]
            } else {
                regions
                    .iter()
                    .filter_map(|region| region.intersection(bounds))
                    .collect()
            };
            for repaint in repaint_regions {
                let raster_started = timestamp_cycles();
                let local = Rect::new(
                    repaint.x - bounds.x,
                    repaint.y - bounds.y,
                    repaint.width,
                    repaint.height,
                );
                let Some(surface) = retained.surface_mut(surface_id) else {
                    return Err(super::renderer::RetainedRendererError::Composition);
                };
                surface.clear(local, crate::graphics::surface::PremulArgb::TRANSPARENT);
                let mut canvas = SurfaceCanvas::new(
                    surface,
                    (bounds.x, bounds.y),
                    (self.graphics_device.width(), self.graphics_device.height()),
                );
                windows_rasterized =
                    windows_rasterized.saturating_add(self.render_layer_tree_in_region(
                        *root,
                        repaint,
                        0,
                        0,
                        &root_ids,
                        *root,
                        *bounds,
                        &mut canvas,
                    ) as u64);
                surface_pixels_updated = surface_pixels_updated.saturating_add(local.area());
                surface_raster_cycles = surface_raster_cycles
                    .saturating_add(timestamp_cycles().saturating_sub(raster_started));
            }
        }

        let canonical_scene = retained.build_scene(&decorated_roots);
        let mut scene = crate::graphics::scene::SceneFrame::new(
            canonical_scene.output_size.0,
            canonical_scene.output_size.1,
        );
        let mut next_z = 0i32;
        for ((root, absolute_bounds), mut layer) in
            roots.iter().zip(canonical_scene.layers.into_iter())
        {
            if let Some(window) = self.window_registry.get(root) {
                let properties = window.compositor_properties();
                layer.opacity = properties.opacity;
                layer.transform = properties.transform;
                layer.effect = properties.effect;
            }
            layer.z_index = next_z;
            next_z = next_z.saturating_add(1);
            scene.push(layer);

            let mut external = Vec::new();
            let local = self
                .window_registry
                .get(root)
                .map(|window| window.bounds())
                .unwrap_or(*absolute_bounds);
            self.collect_external_gl_layers_recursive(
                *root,
                absolute_bounds.x.saturating_sub(local.x),
                absolute_bounds.y.saturating_sub(local.y),
                &root_ids,
                &mut external,
            );
            for (client_id, destination) in external {
                let Some(info) = retained.gl_client_info(client_id) else {
                    continue;
                };
                let mut client_layer = crate::graphics::scene::Layer::virgl_client(
                    client_id,
                    info.width,
                    info.height,
                    destination,
                );
                client_layer.z_index = next_z;
                next_z = next_z.saturating_add(1);
                scene.push(client_layer);
            }
        }
        retained.prepare_backdrop_coverage(&mut scene);
        compose_regions = Self::expand_backdrop_damage(&scene, &compose_regions);
        let mut stats = retained.compose(&scene, &compose_regions)?;

        // CPU composition uses an output-surface overlay. Direct VirGL
        // scanout keeps the pointer on the dedicated hardware cursor plane.
        if !direct_scanout {
            let output = retained.output_mut();
            let mut canvas = SurfaceCanvas::new(
                output,
                (0, 0),
                (self.graphics_device.width(), self.graphics_device.height()),
            );
            self.cursor.draw(mouse_x, mouse_y, &mut canvas);
        }

        let presentation_started = timestamp_cycles();
        let presented_by_virtio = if direct_scanout {
            stats.scanout_flushes = retained.present_direct(&compose_regions)?;
            let cursor_pixels = retained
                .hardware_cursor_needs_image()
                .then(CursorRenderer::hardware_argb_64);
            if retained.update_hardware_cursor(
                mouse_x.max(0) as u32,
                mouse_y.max(0) as u32,
                cursor_pixels.as_deref(),
            )? {
                stats.cursor_updates = 1;
            }
            true
        } else {
            retained.present_virtio(&compose_regions)?
        };
        if !presented_by_virtio {
            let mut presenter = BootFramebufferPresenter::new(&mut *self.graphics_device);
            presenter
                .present(retained.output(), &compose_regions)
                .map_err(|_| super::renderer::RetainedRendererError::Composition)?;
        }
        let presentation_cycles = timestamp_cycles().saturating_sub(presentation_started);
        stats.frames = 1;
        stats.windows_rasterized = windows_rasterized;
        stats.surface_pixels_updated = surface_pixels_updated;
        stats.presents = 1;
        stats.surface_raster_cycles = surface_raster_cycles;
        stats.presentation_cycles = presentation_cycles;
        self.last_render_stats = stats;
        for window in self.window_registry.values_mut() {
            window.clear_composition_dirty();
        }
        if super::renderer::render_stats_enabled() {
            let engine = match retained.engine_kind() {
                crate::graphics::composition::CompositionEngineKind::Cpu => "cpu",
                crate::graphics::composition::CompositionEngineKind::Virgl => "virgl",
            };
            crate::debug_info!(
            "render_stats renderer={} engine={} presenter={} frames={} windows={} surface_pixels={} layers={} upload_bytes={} upload_regions={} texture_hits={} texture_misses={} texture_replacements={} texture_evictions={} texture_creates={} texture_destroys={} pipeline_creates={} sampler_view_creates={} sampler_view_destroys={} vertex_creates={} vertex_destroys={} vertex_capacity={} texture_cache_bytes={} texture_cache_peak={} command_dwords={} gpu_submissions={} readback_bytes={} output_regions={} output_pixels={} scanout_flushes={} backdrop_copies={} backdrop_copy_pixels={} blur_passes={} blur_pixels={} coverage_scans={} coverage_pixels={} coverage_regions={} blur_scratch_bytes={} cursor_updates={} presents={} cycles_raster={} cycles_upload={} cycles_compose={} cycles_blur={} cycles_fence={} cycles_readback={} cycles_present={} surface_bytes={} surface_peak={}",
            if engine == "virgl" { "gpu" } else { "retained" },
            engine,
            if direct_scanout { "virgl-direct" } else if presented_by_virtio { "virtio-gpu-2d" } else { "boot-framebuffer" },
            stats.frames,
            stats.windows_rasterized,
            stats.surface_pixels_updated,
            stats.layers_composed,
            stats.texture_bytes_uploaded,
            stats.texture_upload_regions,
            stats.texture_cache_hits,
            stats.texture_cache_misses,
            stats.texture_cache_replacements,
            stats.texture_cache_evictions,
            stats.texture_resources_created,
            stats.texture_resources_destroyed,
            stats.pipeline_objects_created,
            stats.sampler_views_created,
            stats.sampler_views_destroyed,
            stats.vertex_resources_created,
            stats.vertex_resources_destroyed,
            stats.vertex_buffer_capacity,
            stats.texture_cache_bytes,
            stats.texture_cache_peak_bytes,
            stats.command_stream_dwords,
            stats.gpu_submissions,
            stats.gpu_readback_bytes,
            stats.output_damage_regions,
            stats.output_pixels_damaged,
            stats.scanout_flushes,
            stats.backdrop_copies,
            stats.backdrop_copy_pixels,
            stats.backdrop_blur_passes,
            stats.backdrop_blur_pixels,
            stats.backdrop_coverage_scans,
            stats.backdrop_coverage_pixels_scanned,
            stats.backdrop_coverage_regions,
            stats.backdrop_scratch_bytes,
            stats.cursor_updates,
            stats.presents,
            stats.surface_raster_cycles,
            stats.texture_upload_cycles,
            stats.composition_cycles,
            stats.backdrop_blur_cycles,
            stats.fence_wait_cycles,
            stats.gpu_readback_cycles,
            stats.presentation_cycles,
            retained.budget().total(),
            retained.budget().peak_bytes(),
            );
        }
        Ok(())
    }

    /// Retained layer roots are the active screen root and each of its direct
    /// visible children. This groups a top-level frame with its widget subtree
    /// while keeping desktop, popups, taskbar, and sibling frames independent.
    fn collect_layer_roots(&self) -> Vec<(WindowId, Rect)> {
        let mut roots = Vec::new();
        let Some(root_id) = self
            .get_active_screen()
            .and_then(|screen| screen.root_window)
        else {
            return roots;
        };
        let Some(root) = self.window_registry.get(&root_id) else {
            return roots;
        };
        if !root.visible() {
            return roots;
        }
        let root_bounds = root.bounds();
        roots.push((root_id, root_bounds));
        for child_id in root.children() {
            let Some(child) = self.window_registry.get(child_id) else {
                continue;
            };
            if !child.visible() {
                continue;
            }
            let local = child.bounds();
            roots.push((
                *child_id,
                Rect::new(
                    root_bounds.x + local.x,
                    root_bounds.y + local.y,
                    local.width,
                    local.height,
                ),
            ));
        }
        roots
    }

    fn collect_external_gl_layers_recursive(
        &self,
        window_id: WindowId,
        parent_x: i32,
        parent_y: i32,
        layer_roots: &[WindowId],
        output: &mut Vec<(ClientGlId, Rect)>,
    ) {
        let Some(window) = self.window_registry.get(&window_id) else {
            return;
        };
        if !window.visible() {
            return;
        }
        let local = window.bounds();
        let absolute = Rect::new(
            parent_x.saturating_add(local.x),
            parent_y.saturating_add(local.y),
            local.width,
            local.height,
        );
        if let Some(client_id) = window.external_gl_client() {
            output.push((client_id, absolute));
        }
        for child in window.children() {
            if layer_roots.contains(child) {
                continue;
            }
            self.collect_external_gl_layers_recursive(
                *child,
                absolute.x,
                absolute.y,
                layer_roots,
                output,
            );
        }
    }

    fn subtree_needs_repaint(&self, id: WindowId, layer_roots: &[WindowId]) -> bool {
        let Some(window) = self.window_registry.get(&id) else {
            return false;
        };
        if window.needs_repaint() {
            return true;
        }
        window.children().iter().copied().any(|child| {
            !layer_roots.contains(&child) && self.subtree_needs_repaint(child, layer_roots)
        })
    }

    fn descendants_need_repaint(&self, id: WindowId, layer_roots: &[WindowId]) -> bool {
        let Some(window) = self.window_registry.get(&id) else {
            return false;
        };
        window.children().iter().copied().any(|child| {
            !layer_roots.contains(&child)
                && (self
                    .window_registry
                    .get(&child)
                    .map(|w| w.needs_repaint())
                    .unwrap_or(false)
                    || self.descendants_need_repaint(child, layer_roots))
        })
    }

    fn render_layer_tree_in_region(
        &mut self,
        window_id: WindowId,
        region: Rect,
        parent_x: i32,
        parent_y: i32,
        layer_roots: &[WindowId],
        layer_root: WindowId,
        root_paint_bounds: Rect,
        canvas: &mut SurfaceCanvas<'_>,
    ) -> usize {
        let Some(mut window) = self.window_registry.remove(&window_id) else {
            return 0;
        };
        let mut bounds = window.bounds();
        let visible = window.visible();
        let children = window.children().to_vec();
        let wants_overlay = window.wants_paint_overlay();
        bounds.x += parent_x;
        bounds.y += parent_y;
        if !visible {
            self.window_registry.insert(window_id, window);
            return 0;
        }

        let mut painted = 0;
        let paint_bounds = if window_id == layer_root {
            root_paint_bounds
        } else {
            bounds
        };
        let clip = paint_bounds.intersection(&region);
        if let Some(clip) = clip {
            let original = window.bounds();
            window.set_bounds_no_invalidate(bounds);
            canvas.set_clip_rect(Some(clip));
            // Canonical retained surfaces deliberately bypass framebuffer-
            // native WindowBuffer caches.
            window.paint(canvas);
            window.set_bounds_no_invalidate(original);
            canvas.set_clip_rect(None);
            painted = 1;
        }
        self.window_registry.insert(window_id, window);

        for child in children {
            if layer_roots.contains(&child) {
                continue;
            }
            painted += self.render_layer_tree_in_region(
                child,
                region,
                bounds.x,
                bounds.y,
                layer_roots,
                layer_root,
                root_paint_bounds,
                canvas,
            );
        }

        // Post-children pass: lets a themed frame re-carve translucent
        // geometry (rounded bottom corners) over content painted flush to
        // the window edge. Surface ARGB writes are exact replacement, so
        // the carve makes those pixels transparent again.
        if wants_overlay {
            if let Some(clip) = clip {
                if let Some(mut window) = self.window_registry.remove(&window_id) {
                    let original = window.bounds();
                    window.set_bounds_no_invalidate(bounds);
                    canvas.set_clip_rect(Some(clip));
                    window.paint_overlay(canvas);
                    window.set_bounds_no_invalidate(original);
                    canvas.set_clip_rect(None);
                    self.window_registry.insert(window_id, window);
                }
            }
        }
        painted
    }

    /// Clamp a presentation rectangle and merge it transitively with any
    /// overlapping rectangles already in the batch.
    fn add_present_region(regions: &mut Vec<Rect>, rect: Rect, screen: Rect) {
        let Some(mut merged) = rect.intersection(&screen) else {
            return;
        };

        let mut index = 0;
        while index < regions.len() {
            if regions[index].overlaps(&merged) {
                merged = merged.union(&regions.remove(index));
                index = 0;
            } else {
                index += 1;
            }
        }
        regions.push(merged);
    }

    fn expand_backdrop_damage(
        scene: &crate::graphics::scene::SceneFrame,
        damage: &[Rect],
    ) -> Vec<Rect> {
        let screen = Rect::new(0, 0, scene.output_size.0, scene.output_size.1);
        let mut expanded = Vec::new();
        for &rect in damage {
            Self::add_present_region(&mut expanded, rect, screen);
            for layer in &scene.layers {
                let crate::graphics::scene::LayerEffect::BackdropSample { radius } = layer.effect
                else {
                    continue;
                };
                let halo = crate::graphics::scene::inflate_rect(rect, radius as u32);
                for affected in crate::graphics::scene::backdrop_targets(scene, layer, halo, screen)
                {
                    Self::add_present_region(&mut expanded, affected, screen);
                }
            }
        }
        expanded
    }

    #[cfg(feature = "test")]
    pub fn test_expand_backdrop_damage(
        scene: &crate::graphics::scene::SceneFrame,
        damage: &[Rect],
    ) -> Vec<Rect> {
        Self::expand_backdrop_damage(scene, damage)
    }

    fn mark_dirty_for_invalidated_windows(&mut self) {
        let order = self.collect_render_order();
        for (window_id, abs_bounds) in &order {
            let (needs_repaint, composition_dirty, hint, insets) = self
                .window_registry
                .get(window_id)
                .map(|w| {
                    (
                        w.needs_repaint(),
                        w.composition_dirty(),
                        w.dirty_rect_hint(),
                        w.decoration_insets(),
                    )
                })
                .unwrap_or((false, false, None, super::Insets::ZERO));
            if !needs_repaint && !composition_dirty {
                continue;
            }
            // Translate the window-local hint to absolute coordinates
            // by adding the window's absolute origin. Falling back to
            // the full bounds is always correct (just less narrow).
            let dirty_rect = match hint.filter(|_| needs_repaint) {
                Some(local) => Rect::new(
                    abs_bounds.x + local.x,
                    abs_bounds.y + local.y,
                    local.width,
                    local.height,
                ),
                None => insets.expand(*abs_bounds),
            };
            self.compositor.dirty.mark_dirty(dirty_rect);
            crate::debug_trace!(
                "Window {:?} needs repaint, marking dirty: {:?}",
                window_id,
                dirty_rect
            );
        }
    }

    /// Test-only: drive just the dirty-marking pass without the rest of
    /// `render`. Used by `tests/window_manager_render.rs`.
    #[cfg(feature = "test")]
    pub fn test_mark_dirty_for_invalidated_windows(&mut self) {
        self.mark_dirty_for_invalidated_windows();
    }

    /// Test-only: snapshot of the compositor's currently-marked dirty rects.
    #[cfg(feature = "test")]
    pub fn test_dirty_regions(&self) -> alloc::vec::Vec<Rect> {
        self.compositor.dirty.dirty_regions().collect()
    }

    /// Test-only: clear the compositor's dirty state and full-repaint flag
    /// (the constructor sets full-repaint for the first frame, which a test
    /// that wants a quiet baseline needs to reset).
    #[cfg(feature = "test")]
    pub fn test_clear_dirty(&mut self) {
        self.compositor.dirty.clear();
    }

    /// Test-only: mark a single rect dirty.
    #[cfg(feature = "test")]
    pub fn test_mark_dirty(&mut self, rect: Rect) {
        self.compositor.dirty.mark_dirty(rect);
    }

    /// Test-only: mark a full repaint.
    #[cfg(feature = "test")]
    pub fn test_mark_full_repaint(&mut self) {
        self.compositor.dirty.mark_full_repaint();
    }

    /// Test-only: replace the selected renderer with retained CPU rendering.
    #[cfg(feature = "test")]
    pub fn test_force_retained_renderer(&mut self) {
        let width = self.graphics_device.width() as u32;
        let height = self.graphics_device.height() as u32;
        self.renderer = RendererState::Retained(
            RetainedRenderer::new(width, height).expect("test retained renderer should initialize"),
        );
        self.compositor.dirty.mark_full_repaint();
    }

    /// Test-only: return the compositor's current pointer position.
    #[cfg(feature = "test")]
    pub fn test_cursor_position(&self) -> Point {
        let (x, y) = self.compositor.cursor_position();
        Point::new(
            x.min(i32::MAX as usize) as i32,
            y.min(i32::MAX as usize) as i32,
        )
    }

    /// Test-only: read one pixel from the canonical retained output.
    #[cfg(feature = "test")]
    pub fn test_retained_output_pixel(&self, position: Point) -> Option<(u8, u8, u8, u8)> {
        if position.x < 0 || position.y < 0 {
            return None;
        }
        let RendererState::Retained(retained) = &self.renderer else {
            return None;
        };
        retained
            .output()
            .pixel(position.x as u32, position.y as u32)
            .map(crate::graphics::surface::PremulArgb::to_rgba)
    }

    /// Test-only: render a cursor-only retained frame at an injected position.
    #[cfg(feature = "test")]
    pub fn test_render_retained_cursor_at(&mut self, position: Point) {
        let previous = self.test_cursor_position();
        assert!(
            self.compositor
                .update_cursor(position.x.max(0) as usize, position.y.max(0) as usize),
            "test cursor position must change"
        );
        let state = core::mem::replace(&mut self.renderer, RendererState::Legacy);
        let RendererState::Retained(mut retained) = state else {
            panic!("test requires retained renderer")
        };
        self.render_retained(&mut retained, position.x, position.y, Some(previous))
            .expect("retained cursor-only frame should render");
        self.renderer = RendererState::Retained(retained);
        self.compositor.end_frame();
    }

    /// Test-only: drive just the active screen's render-tree walk. Skips the
    /// cursor and dirty-marking phases of `render`; tests set up dirty state
    /// and invalidations themselves.
    #[cfg(feature = "test")]
    pub fn test_render_active_screen(&mut self) {
        if let Some(screen) = self.get_active_screen() {
            if let Some(root_id) = screen.root_window {
                self.render_window_tree(root_id);
            }
        }
    }

    /// Test-only: force the manager into a Dragging interaction state and
    /// pretend the left mouse button is held. Lets a test drive the drag
    /// arm of `handle_dragging` without going through hit-testing on a
    /// title bar.
    #[cfg(feature = "test")]
    pub fn test_force_drag_state(
        &mut self,
        window_id: WindowId,
        start_mouse: Point,
        start_window: Point,
    ) {
        self.interaction_state = InteractionState::Dragging {
            window: window_id,
            start_mouse,
            start_window,
        };
        self.last_mouse_buttons = 0x01;
    }

    /// Test-only: run the production frame hit-test used on a fresh left
    /// button press.
    #[cfg(feature = "test")]
    pub fn test_start_drag_if_on_title_bar(&mut self, mouse_x: i32, mouse_y: i32) {
        self.start_drag_if_on_title_bar(mouse_x, mouse_y);
    }

    /// Test-only: invoke `handle_dragging` directly so a test can drive
    /// drag/resize ticks without simulating mouse input through the full
    /// interrupt pipeline.
    #[cfg(feature = "test")]
    pub fn test_handle_dragging(&mut self, mouse_x: i32, mouse_y: i32, buttons: u8) {
        self.handle_dragging(mouse_x, mouse_y, buttons);
    }

    /// Test-only: did the compositor's dirty manager flip to full-repaint?
    #[cfg(feature = "test")]
    pub fn test_needs_full_repaint(&self) -> bool {
        self.compositor.dirty.needs_full_repaint()
    }

    /// Mark a window as needing repaint (for external callers).

    /// Force a full repaint on the next frame.
    pub fn force_full_repaint(&mut self) {
        self.compositor.dirty.mark_full_repaint();
    }

    /// Render the entire window tree against a single clip region.
    ///
    /// Each window paints when its absolute bounds intersect `region`, with
    /// `clip = region ∩ bounds`. The compositor calls this once per dirty
    /// region (see `render`), so a window whose bounds intersect multiple
    /// disjoint regions paints once per region — each pass writes only the
    /// pixels actually dirtied in that region, never the corridor between.
    ///
    /// `paint()` impls must follow the contract on `Window::paint` and
    /// produce correct pixels for the clip on every call regardless of
    /// internal `needs_repaint` state. (See the trait doc; Phase A removed
    /// the early-return-on-`!needs_repaint` from every paint impl.)
    ///
    /// `wants_backing_store` windows skip per-region rasterization: the
    /// rasterization gate (`needs_repaint || backing_store.is_none()`)
    /// runs on the first region pass and the backing store is reused for
    /// subsequent passes — `paint_into_backing_store` clears
    /// `needs_repaint`, so the gate naturally short-circuits.
    fn render_window_tree_in_region(
        &mut self,
        window_id: WindowId,
        region: Rect,
        parent_x: i32,
        parent_y: i32,
    ) {
        crate::debug_trace!(
            "render_window_tree_in_region: {:?}, region={:?}, offset=({}, {})",
            window_id,
            region,
            parent_x,
            parent_y
        );

        let Some(mut window) = self.window_registry.remove(&window_id) else {
            return;
        };

        let mut bounds = window.bounds();
        let visible = window.visible();
        let children = window.children().to_vec();

        bounds.x += parent_x;
        bounds.y += parent_y;

        if !visible {
            self.window_registry.insert(window_id, window);
            return;
        }

        let clip = bounds.intersection(&region);

        if let Some(clip) = clip {
            let original_bounds = window.bounds();
            window.set_bounds_no_invalidate(bounds);

            self.graphics_device.set_clip_rect(Some(clip));

            if window.wants_backing_store() {
                if window.needs_repaint() || window.backing_store().is_none() {
                    crate::debug_trace!("Rasterizing backing store for {:?}", window_id);
                    window.paint_into_backing_store(&*self.graphics_device);
                }
                if let Some(buf) = window.backing_store() {
                    self.graphics_device.blit_buffer(bounds.x, bounds.y, buf);
                } else {
                    crate::debug_warn!(
                        "Window {:?} wants_backing_store but produced no buffer; \
                         falling back to direct paint",
                        window_id
                    );
                    window.paint(&mut *self.graphics_device);
                }
            } else {
                window.paint(&mut *self.graphics_device);
            }

            window.set_bounds_no_invalidate(original_bounds);
            self.graphics_device.set_clip_rect(None);
        }
        // If the window doesn't intersect `region` we still recurse so a
        // child whose bounds extend outside the parent's bounds (rare but
        // possible — e.g. tooltips) gets a chance to be visited. This
        // mirrors the prior tree walk's recursion-unconditional shape.

        self.window_registry.insert(window_id, window);

        for child_id in children {
            self.render_window_tree_in_region(child_id, region, bounds.x, bounds.y);
        }
    }

    /// Test-only: render the active screen's tree once per current dirty
    /// region — mirrors what `render()` does after the per-region rewrite,
    /// minus the cursor and dirty-marking phases. When no rects are dirty
    /// (e.g. the test set up a clean baseline and didn't mark anything),
    /// nothing paints — the same way a no-op frame skips rendering in
    /// production.
    #[cfg(feature = "test")]
    fn render_window_tree(&mut self, window_id: WindowId) {
        let regions: alloc::vec::Vec<Rect> = self.compositor.dirty.dirty_regions().collect();
        for region in &regions {
            self.render_window_tree_in_region(window_id, *region, 0, 0);
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
            InteractionState::Dragging {
                window,
                start_mouse,
                start_window,
            } => {
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
                            let new_bounds =
                                Rect::new(new_x, new_y, old_bounds.width, old_bounds.height);
                            win.set_bounds(new_bounds);
                            win.invalidate();

                            // Mark only the union of (old, new) bounds dirty
                            // — not the whole screen. Cascade invalidation
                            // (run later in render()) propagates this to
                            // overlapping siblings; the desktop / background
                            // beneath the now-exposed area is dirty-clipped
                            // by U4's intersection logic.
                            //
                            // Top-level windows (the only ones draggable per
                            // start_drag_if_on_title_bar) are children of the
                            // root at (0, 0), so local bounds equal absolute
                            // bounds — passing them straight to mark_dirty is
                            // correct.
                            self.compositor
                                .dirty
                                .mark_dirty(old_bounds.union(&new_bounds));

                            crate::debug_trace!(
                                "Dragging window {:?} to ({}, {})",
                                window,
                                new_x,
                                new_y
                            );
                        }
                    }
                } else {
                    // Button released - end drag
                    crate::debug_trace!("Window drag ended");
                    self.interaction_state = InteractionState::Idle;
                }
            }
            InteractionState::Resizing {
                window,
                edge,
                start_mouse,
                start_bounds,
            } => {
                if left_button_pressed {
                    // Calculate delta from start position
                    let delta_x = mouse_x - start_mouse.x;
                    let delta_y = mouse_y - start_mouse.y;

                    // Calculate new bounds using the helper method
                    let new_bounds = start_bounds.resize_edge(
                        edge,
                        delta_x,
                        delta_y,
                        crate::window::theme::minimum_resizable_frame_width(
                            crate::window::theme::metrics(),
                        ),
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

                            // Same union-mark pattern as the drag arm above.
                            // Resize is also restricted to top-level windows.
                            self.compositor
                                .dirty
                                .mark_dirty(old_bounds.union(&new_bounds));

                            crate::debug_trace!("Resizing window {:?} to {:?}", window, new_bounds);
                        }
                    }

                    // Update child windows after resize
                    self.update_children_for_resized_window(window);
                } else {
                    // Button released - end resize
                    crate::debug_trace!("Window resize ended");
                    self.interaction_state = InteractionState::Idle;
                }
            }
        }
    }

    /// Check if the mouse is on a title bar or border and start the appropriate interaction.
    fn start_drag_if_on_title_bar(&mut self, mouse_x: i32, mouse_y: i32) {
        // If there's an active menu, don't process drag - let the click go to the menu
        if self.active_menu.is_some() {
            crate::debug_trace!(
                "start_drag_if_on_title_bar: active_menu present, skipping drag check"
            );
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
                // Only decorated frame windows own the theme's title-bar and
                // border geometry. Other top-level desktop elements (notably
                // the taskbar) must still receive clicks, but are never
                // draggable or resizable.
                let Some(frame) = window.as_frame_window() else {
                    continue;
                };
                if !window.visible() {
                    continue;
                }
                let resizable = frame.is_resizable();
                let maximized = frame.is_maximized();
                let bounds = window.bounds();
                let local_x = mouse_x - bounds.x;
                let local_y = mouse_y - bounds.y;

                if bounds.contains_point(Point::new(mouse_x, mouse_y)) {
                    let metrics = crate::window::theme::metrics();
                    let border = metrics.border_width as i32;
                    let title_height = metrics.title_bar_height as i32;

                    // Check border regions first (corners before edges)
                    let at_left = local_x < border;
                    let at_right = local_x >= bounds.width as i32 - border;
                    let at_top = local_y < border;
                    let at_bottom = local_y >= bounds.height as i32 - border;

                    if resizable && !maximized && at_top && at_left {
                        target_window = Some(window_id);
                        target_hit = HitTestResult::Border(ResizeEdge::TopLeft);
                        break;
                    } else if resizable && !maximized && at_top && at_right {
                        target_window = Some(window_id);
                        target_hit = HitTestResult::Border(ResizeEdge::TopRight);
                        break;
                    } else if resizable && !maximized && at_bottom && at_left {
                        target_window = Some(window_id);
                        target_hit = HitTestResult::Border(ResizeEdge::BottomLeft);
                        break;
                    } else if resizable && !maximized && at_bottom && at_right {
                        target_window = Some(window_id);
                        target_hit = HitTestResult::Border(ResizeEdge::BottomRight);
                        break;
                    } else if resizable && !maximized && at_top {
                        target_window = Some(window_id);
                        target_hit = HitTestResult::Border(ResizeEdge::Top);
                        break;
                    } else if resizable && !maximized && at_bottom {
                        target_window = Some(window_id);
                        target_hit = HitTestResult::Border(ResizeEdge::Bottom);
                        break;
                    } else if resizable && !maximized && at_left {
                        target_window = Some(window_id);
                        target_hit = HitTestResult::Border(ResizeEdge::Left);
                        break;
                    } else if resizable && !maximized && at_right {
                        target_window = Some(window_id);
                        target_hit = HitTestResult::Border(ResizeEdge::Right);
                        break;
                    } else if local_y >= border && local_y < border + title_height {
                        let buttons = crate::window::theme::caption_button_layout(
                            Rect::new(0, 0, bounds.width, bounds.height),
                            metrics,
                            resizable,
                        );
                        let point = Point::new(local_x, local_y);
                        if buttons.close.contains_point(point) {
                            target_window = Some(window_id);
                            target_hit = HitTestResult::CloseButton;
                            break;
                        }
                        if buttons
                            .maximize
                            .is_some_and(|rect| rect.contains_point(point))
                        {
                            target_window = Some(window_id);
                            target_hit = HitTestResult::MaximizeButton;
                            break;
                        }
                        if buttons
                            .minimize
                            .is_some_and(|rect| rect.contains_point(point))
                        {
                            target_window = Some(window_id);
                            target_hit = HitTestResult::MinimizeButton;
                            break;
                        }

                        target_window = Some(window_id);
                        target_hit = if maximized {
                            HitTestResult::Client
                        } else {
                            HitTestResult::TitleBar
                        };
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
                        crate::debug_trace!(
                            "Starting window drag for {:?} at ({}, {})",
                            window_id,
                            bounds.x,
                            bounds.y
                        );
                        self.interaction_state = InteractionState::Dragging {
                            window: window_id,
                            start_mouse: Point::new(mouse_x, mouse_y),
                            start_window: Point::new(bounds.x, bounds.y),
                        };
                        self.focus_frame_and_content(window_id);
                    }
                    HitTestResult::Border(edge) => {
                        crate::debug_trace!(
                            "Starting window resize for {:?} edge {:?}",
                            window_id,
                            edge
                        );
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
                        crate::debug_info!("Close button clicked for {:?}", window_id);
                        if let Some(client_id) = self.first_close_request_descendant(window_id) {
                            self.route_event_to_window(
                                client_id,
                                Event::Close(crate::window::event::CloseEvent {
                                    window: window_id,
                                }),
                            );
                        } else {
                            // destroy_window marks the compositor dirty.
                            self.destroy_window(window_id);
                        }
                    }
                    HitTestResult::MinimizeButton => {
                        crate::debug_info!("Minimize button clicked for {:?}", window_id);
                        self.minimize_frame(window_id);
                    }
                    HitTestResult::MaximizeButton => {
                        crate::debug_info!("Maximize button clicked for {:?}", window_id);
                        self.toggle_maximize_frame(window_id);
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
        let target = self
            .first_focusable_descendant(frame_id)
            .unwrap_or(frame_id);
        self.focus_window(target);
    }

    /// Depth-first search for the first focusable window in `id`'s subtree
    /// (excluding `id` itself).
    fn first_focusable_descendant(&self, id: WindowId) -> Option<WindowId> {
        let window = self.window_registry.get(&id)?;
        for &child_id in window.children() {
            if let Some(child) = self.window_registry.get(&child_id) {
                if !child.visible() {
                    continue;
                }
                if child.can_focus() && self.window_effectively_visible(child_id) {
                    return Some(child_id);
                }
                if let Some(found) = self.first_focusable_descendant(child_id) {
                    return Some(found);
                }
            }
        }
        None
    }

    fn first_close_request_descendant(&self, id: WindowId) -> Option<WindowId> {
        let window = self.window_registry.get(&id)?;
        for &child_id in window.children() {
            if let Some(child) = self.window_registry.get(&child_id) {
                if child.accepts_close_request() {
                    return Some(child_id);
                }
                if let Some(found) = self.first_close_request_descendant(child_id) {
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

                let metrics = crate::window::theme::metrics();
                let border = metrics.border_width;
                let title_height = metrics.title_bar_height;

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

    /// Check if a modal dialog is currently active

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

    /// Frame list for the ring-3 desktop shell's taskbar: `(frame_id, title,
    /// state)` where state is `0` normal / `1` minimized / `2` maximized.
    /// Returns titled frames, carrying min/max state and excluding the shell's
    /// own panel (`taskbar_id`).
    pub fn shell_window_list(&self) -> Vec<(WindowId, alloc::string::String, u8)> {
        use alloc::string::String;

        let mut result = Vec::new();
        for (&window_id, window) in &self.window_registry {
            if Some(window_id) == self.taskbar_id {
                continue;
            }
            let Some(title) = window.window_title() else {
                continue;
            };
            let state = window
                .as_frame_window()
                .map(|frame| {
                    if frame.is_minimized() {
                        1
                    } else if frame.is_maximized() {
                        2
                    } else {
                        0
                    }
                })
                .unwrap_or(0);
            result.push((window_id, String::from(title), state));
        }
        result
    }

    /// Apply a taskbar action from the ring-3 desktop shell to a frame it does
    /// not own. `action`: `0` activate, `1` minimize, `2` maximize/restore
    /// toggle, `3` restore (activate), `4` close. Returns whether it applied.
    pub fn shell_window_action(&mut self, frame_id: WindowId, action: u32) -> bool {
        let is_frame = self
            .window_registry
            .get(&frame_id)
            .and_then(|window| window.as_frame_window())
            .is_some();
        if !is_frame {
            return false;
        }
        match action {
            0 | 3 => self.activate_frame(frame_id),
            1 => self.minimize_frame(frame_id),
            2 => self.toggle_maximize_frame(frame_id),
            4 => {
                self.destroy_window(frame_id);
                self.force_full_repaint();
                true
            }
            _ => false,
        }
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
/// runs synchronously on the same thread. The surrounding
/// `with_window_manager` prevents kernel-thread preemption, and interrupt
/// handlers never access the manager directly.
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

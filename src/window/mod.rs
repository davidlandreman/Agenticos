//! Window System for AgenticOS
//! 
//! This module provides a hierarchical window-based graphics system that supports
//! both GUI and text-based interfaces through a unified abstraction.

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

pub mod types;
pub mod event;
pub mod graphics;
pub mod manager;
pub mod screen;
pub mod adapters;
pub mod selection;
pub mod windows;
pub mod dialogs;
pub mod terminal;
pub mod console;
pub mod keyboard;
pub mod cursor;
pub mod compositor;
pub mod terminal_factory;

pub use types::*;
pub use event::*;
pub use manager::*;
pub use screen::*;

// Re-export commonly used types
pub use self::types::{WindowId, ScreenId, Rect, Point};
pub use self::event::{Event, EventResult};
pub use self::graphics::{GraphicsDevice, WindowBuffer};
pub use self::selection::{Selection, SelectionMode, ClickMods};

// =====================================================================
// Default widget palette
//
// Reconciles default color values across all widget constructors so
// default-styled widgets in the same window look visually consistent.
// Out of scope: a configurable theming system. Widgets that need a
// different look can still override their colors via setter methods.
//
// - PALETTE_CHROME_ACTIVE:   window-frame chrome when active (blue)
// - PALETTE_CHROME_INACTIVE: window-frame chrome when inactive (grey)
// - PALETTE_CONTENT_BG:      shared content background (used by
//                            Container, List, MultiColumnList,
//                            TreeView, IconView, ScrollView, Toolbar,
//                            StatusBar, PathBar, Menu, ProgressBar,
//                            TextInput, TextEditor, Button)
// - PALETTE_BORDER:          borders and dividers
// - PALETTE_HIGHLIGHT_BG:    selection / hover highlight background
// - PALETTE_HIGHLIGHT_TEXT:  text on highlight background
// - PALETTE_TEXT:            text on light backgrounds
// - PALETTE_PROGRESS_FILL:   filled portion of ProgressBar
//                            (matches highlight)
//
// Intentionally NOT covered by this palette (kept distinct on purpose):
// - DesktopWindow background (deep blue identity color).
// - TextWindow / TerminalWindow (dark grey background, light text).
// =====================================================================

/// Window-frame chrome color when the frame is active (focused).
pub const PALETTE_CHROME_ACTIVE: crate::graphics::color::Color =
    crate::graphics::color::Color::new(0, 100, 200);

/// Window-frame chrome color when the frame is inactive.
pub const PALETTE_CHROME_INACTIVE: crate::graphics::color::Color =
    crate::graphics::color::Color::new(100, 100, 100);

/// Shared light content background used by most widgets.
pub const PALETTE_CONTENT_BG: crate::graphics::color::Color =
    crate::graphics::color::Color::new(240, 240, 240);

/// Default border / divider color.
pub const PALETTE_BORDER: crate::graphics::color::Color =
    crate::graphics::color::Color::new(100, 100, 100);

/// Default selection / hover highlight background.
pub const PALETTE_HIGHLIGHT_BG: crate::graphics::color::Color =
    crate::graphics::color::Color::new(0, 120, 215);

/// Default text color drawn on `PALETTE_HIGHLIGHT_BG`.
pub const PALETTE_HIGHLIGHT_TEXT: crate::graphics::color::Color =
    crate::graphics::color::Color::WHITE;

/// Default text color on light backgrounds.
pub const PALETTE_TEXT: crate::graphics::color::Color =
    crate::graphics::color::Color::BLACK;

/// Filled portion of a `ProgressBar` — matches the highlight color.
pub const PALETTE_PROGRESS_FILL: crate::graphics::color::Color = PALETTE_HIGHLIGHT_BG;

/// Core window trait that all visual elements implement.
///
/// Most plumbing methods (id/bounds/visible/parent/children/needs_repaint/
/// invalidate/has_focus/set_focus/set_bounds/set_visible/...) default to a
/// one-line delegation through `base()` / `base_mut()`. Each widget only
/// supplies `base()`, `base_mut()`, `paint`, `handle_event`, and any
/// override (`can_focus`, `set_bounds` if it does extra work, etc.).
pub trait Window: Send {
    /// Borrow the widget's `WindowBase`. Used by the default implementations
    /// of the plumbing methods below (id/bounds/visible/...).
    fn base(&self) -> &windows::base::WindowBase;

    /// Mutably borrow the widget's `WindowBase`. Used by the default
    /// implementations of the plumbing methods below.
    fn base_mut(&mut self) -> &mut windows::base::WindowBase;

    /// Paint this window to the graphics device.
    ///
    /// **Contract**: when called, produce correct pixels for every pixel in
    /// `bounds() ∩ device.clip_rect()`. The compositor decides *whether* to
    /// call `paint()` (based on `needs_repaint || intersects_dirty`) and
    /// sets the device clip before the call. Implementations must NOT
    /// short-circuit on `!needs_repaint()` — that re-implements the
    /// compositor's decision and breaks the case where a static window
    /// shares pixels with a freshly-dirtied region (the desktop's
    /// backing-store blit overwrites those pixels and the window is
    /// expected to repaint over them).
    ///
    /// Internal dirty-tracking (e.g. `TextWindow::dirty_cells`) is allowed
    /// only as a *performance hint* to choose between full and incremental
    /// paint paths — never as an excuse to skip work the compositor has
    /// already decided is needed.
    fn paint(&mut self, device: &mut dyn GraphicsDevice);

    /// Handle an event
    fn handle_event(&mut self, event: Event) -> EventResult;

    /// Check if this window can receive keyboard focus
    fn can_focus(&self) -> bool {
        false
    }

    /// Get the unique identifier for this window
    fn id(&self) -> WindowId {
        self.base().id()
    }

    /// Get the bounds of this window relative to its parent
    fn bounds(&self) -> Rect {
        self.base().bounds()
    }

    /// Set the bounds of this window
    fn set_bounds(&mut self, bounds: Rect) {
        self.base_mut().set_bounds(bounds);
    }

    /// Set bounds without triggering invalidation (for render-time transforms)
    fn set_bounds_no_invalidate(&mut self, bounds: Rect) {
        self.base_mut().set_bounds_no_invalidate(bounds);
    }

    /// Check if this window is visible
    fn visible(&self) -> bool {
        self.base().visible()
    }

    /// Set the visibility of this window
    fn set_visible(&mut self, visible: bool) {
        self.base_mut().set_visible(visible);
    }

    /// Get the parent window ID, if any
    fn parent(&self) -> Option<WindowId> {
        self.base().parent()
    }

    /// Get child window IDs
    fn children(&self) -> &[WindowId] {
        self.base().children()
    }

    /// Set the parent of this window
    fn set_parent(&mut self, parent: Option<WindowId>) {
        self.base_mut().set_parent(parent);
    }

    /// Add a child window
    fn add_child(&mut self, child: WindowId) {
        self.base_mut().add_child(child);
    }

    /// Remove a child window
    fn remove_child(&mut self, child: WindowId) {
        self.base_mut().remove_child(child);
    }

    /// Check if this window needs repainting
    fn needs_repaint(&self) -> bool {
        self.base().needs_repaint()
    }

    /// Mark this window as needing repaint
    fn invalidate(&mut self) {
        self.base_mut().invalidate();
    }

    /// Check if this window currently has focus
    fn has_focus(&self) -> bool {
        self.base().has_focus()
    }

    /// Set the focus state of this window
    fn set_focus(&mut self, focused: bool) {
        self.base_mut().set_focus(focused);
    }

    /// Whether this window wants the compositor to blit it from a cached
    /// backing store rather than calling `paint()` per frame. Opting in
    /// means: the compositor calls `paint_into_backing_store` (only when
    /// `needs_repaint()` is true), then blits the result from
    /// `backing_store()` to the back buffer for the dirty intersection.
    /// Drag, resize, z-order, and cursor-only motion never re-rasterize.
    ///
    /// Default: opted out — the compositor uses the normal `paint()` path.
    /// Most window types should leave this default; opt in only when the
    /// per-frame rasterization cost is large enough to justify the memory
    /// (typically a wallpaper-like static surface).
    fn wants_backing_store(&self) -> bool {
        false
    }

    /// Rasterize the window's current content into its own backing buffer.
    /// Called by the compositor only when `wants_backing_store()` is true
    /// and `needs_repaint()` is true. Implementations are expected to
    /// allocate or resize the buffer to current bounds and write pixels
    /// in framebuffer-native format.
    ///
    /// `device` is provided so the implementation can query
    /// `pixel_format`, `bytes_per_pixel`, etc. at rasterization time —
    /// the buffer's byte layout has to match the framebuffer's so the
    /// compositor's blit stays a row `memcpy`.
    ///
    /// Default: no-op (matches `wants_backing_store == false`).
    fn paint_into_backing_store(&mut self, _device: &dyn GraphicsDevice) {}

    /// Borrow the window's cached backing store. Returns `None` until
    /// `paint_into_backing_store` has run at least once for the current
    /// content state, or for windows that don't opt in.
    fn backing_store(&self) -> Option<&WindowBuffer> {
        None
    }

    /// Per-frame preparation hook called once per window by the
    /// compositor *before* it consults dirty state for the frame.
    ///
    /// Use this to drain any external buffers whose contents must be
    /// reflected in the window's internal dirty tracking before the
    /// compositor decides what to paint. The canonical example is
    /// `TerminalWindow`, whose pending shell output lives in a
    /// per-terminal buffer until `process_terminal_output` writes it
    /// into the underlying `TextWindow`'s grid (which is what
    /// populates `dirty_cells`). If the drain runs only inside
    /// `paint()`, the compositor will already have computed
    /// `dirty_rect_hint()` against an empty `dirty_cells`, fall back
    /// to the full bounds, and blit the desktop's wallpaper across
    /// the whole terminal — leaving older lines as wallpaper after
    /// the incremental paint redraws only the freshly-typed cells.
    ///
    /// Default: no-op.
    fn prepare_for_render(&mut self) {}

    /// Optional narrower invalidation rect (in window-local coordinates,
    /// origin = window's top-left).
    ///
    /// When `Some`, the compositor uses this rect — translated to absolute
    /// screen coordinates by adding the window's absolute origin — instead
    /// of the window's full bounds when adding the window's dirty area to
    /// the dirty-rect tracker. This keeps the desktop's wallpaper blit
    /// (and any other lower-z window's repaint) confined to just the area
    /// that actually changed.
    ///
    /// Use this when the window has fine-grained internal dirty tracking
    /// (e.g. `TextWindow`'s per-cell `dirty_cells`) and the `paint()`
    /// implementation can repaint correctly within the narrowed clip.
    /// Returning `None` (the default) makes the compositor fall back to
    /// the full bounds — the safe choice for any window without
    /// sub-bounds dirty tracking.
    fn dirty_rect_hint(&self) -> Option<Rect> {
        None
    }

    /// Get the window title if this is a frame window
    /// Returns None for non-frame windows
    fn window_title(&self) -> Option<&str> {
        None
    }

    /// Grid dimensions if this window renders a text grid.
    /// Returns `Some((rows, cols))` for `TextWindow`/`TerminalWindow`,
    /// `None` otherwise. The terminal factory uses this to size the
    /// pty's `Winsize` from the actual on-screen grid.
    fn grid_size(&self) -> Option<(u16, u16)> {
        None
    }

    /// Poll for pending popup (used by MenuBar)
    /// Returns None for non-menu-bar windows
    fn poll_pending_popup(&mut self) -> Option<windows::PendingPopup> {
        None
    }

    /// Poll for pending popup selection (used by MenuBarPopup)
    /// Returns (menu_bar_id, item_index) if a selection was made
    fn poll_pending_popup_selection(&mut self) -> Option<(WindowId, usize)> {
        None
    }

    /// Handle popup selection (used by MenuBar)
    fn handle_popup_selection(&mut self, _item_index: usize) {}

    /// Close popup menu (used by MenuBar)
    fn close_popup_menu(&mut self) {}

    /// Discriminator used by the manager when routing `MouseEventType::Scroll`
    /// events. Returns `true` for `ScrollView`; default `false` for every
    /// other window type. The manager-side downcast performed when this
    /// returns `true` lets new scrollable wrappers be added without
    /// requiring every widget to opt into a typed accessor.
    fn is_scroll_view(&self) -> bool {
        false
    }

    /// Drain a pending `Event::EnsureVisible(rect)` payload, if any. The
    /// window manager calls this immediately after dispatching an event
    /// to a window; if the return value is `Some(rect)`, the manager
    /// forwards an `Event::EnsureVisible(rect)` to the nearest enclosing
    /// `ScrollView` ancestor so the rect is scrolled into view.
    ///
    /// The default returns `None`. Widgets like `TextEditor` (cursor
    /// move) override this to plumb cursor-into-view requests upward
    /// without needing a typed reference to their parent `ScrollView`.
    fn take_pending_ensure_visible(&mut self) -> Option<Rect> {
        None
    }

    /// Typed accessor used by `Toolbar` to toggle a button's enabled
    /// state through the manager. Returns `None` by default; `Button`
    /// overrides it to return `Some(self)`. Following the same opt-in
    /// pattern as `is_scroll_view`, this avoids a generic downcast
    /// machinery while keeping the trait small.
    fn as_button_mut(&mut self) -> Option<&mut windows::button::Button> {
        None
    }

    /// Typed accessor used by `StatusBar` to update a section's text
    /// through the manager. Returns `None` by default; `Label`
    /// overrides it to return `Some(self)`.
    fn as_label_mut(&mut self) -> Option<&mut windows::label::Label> {
        None
    }

    /// Typed accessor used by app code (e.g. File Explorer) to mutate
    /// a `MultiColumnList`'s rows through the manager.
    fn as_multi_column_list_mut(
        &mut self,
    ) -> Option<&mut windows::multi_column_list::MultiColumnList> {
        None
    }

    /// Typed accessor used by app code (e.g. File Explorer) to mutate
    /// a `TreeView` through the manager.
    fn as_tree_view_mut(&mut self) -> Option<&mut windows::tree_view::TreeView> {
        None
    }

    /// Typed accessor used by app code (e.g. File Explorer) to update
    /// a `PathBar`'s path through the manager.
    fn as_path_bar_mut(&mut self) -> Option<&mut windows::path_bar::PathBar> {
        None
    }
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
/// deadlocks with preemptive multitasking. Uses RAII guard to ensure
/// interrupts are restored even if the closure panics.
pub fn with_window_manager<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut WindowManager) -> R,
{
    use crate::arch::x86_64::interrupt_guard::InterruptGuard;

    // RAII guard ensures interrupts are restored even on panic
    let _guard = InterruptGuard::disable();

    let mut wm_lock = WINDOW_MANAGER.lock();
    wm_lock.as_mut().map(f)

    // _guard dropped here, restoring interrupt state
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

/// Path of the bundled default wallpaper on the FAT root. The basename is
/// 8.3-compliant — see `src/fs/CLAUDE.md` for the FAT layer's filename limit.
pub const DEFAULT_WALLPAPER_PATH: &str = "/WALLPAPR.BMP";

/// Read the default wallpaper file from the mounted filesystem and return its
/// raw bytes. Returns `None` on any failure — file missing, read error, or an
/// empty file. Callers fall back to a solid-color desktop when this returns
/// `None`, so a missing wallpaper never blocks boot.
pub fn load_default_wallpaper() -> Option<Vec<u8>> {
    let file = match crate::fs::File::open_read(DEFAULT_WALLPAPER_PATH) {
        Ok(f) => f,
        Err(_) => {
            crate::debug_info!(
                "Wallpaper {} not found on root filesystem; using solid background",
                DEFAULT_WALLPAPER_PATH
            );
            return None;
        }
    };

    let size = file.size() as usize;
    if size == 0 {
        return None;
    }

    let mut bytes = vec![0u8; size];
    match file.read(&mut bytes) {
        Ok(_) => Some(bytes),
        Err(_) => {
            crate::debug_info!("Failed to read wallpaper bytes from {}", DEFAULT_WALLPAPER_PATH);
            None
        }
    }
}

/// Create the default desktop environment
pub fn create_default_desktop() {
    let wallpaper = load_default_wallpaper();
    with_window_manager(|wm| {
        // Create GUI screen
        let screen_id = wm.create_screen(ScreenMode::Gui);
        wm.switch_screen(screen_id);

        // Get actual screen dimensions from graphics device
        let (width, height) = wm.screen_dimensions();

        // Create desktop background window
        let desktop_id = wm.create_window(None);
        let desktop_bounds = Rect::new(0, 0, width, height);
        let desktop_window: Box<dyn Window> = match wallpaper {
            Some(bytes) => Box::new(windows::DesktopWindow::new_with_wallpaper(desktop_id, desktop_bounds, bytes)),
            None => Box::new(windows::DesktopWindow::new(desktop_id, desktop_bounds)),
        };
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
        let (width, height) = wm.screen_dimensions();
        
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

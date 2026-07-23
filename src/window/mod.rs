//! Window System for AgenticOS
//!
//! This module provides a hierarchical window-based graphics system that supports
//! both GUI and text-based interfaces through a unified abstraction.

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::arch::x86_64::preemption_guard::PreemptionMutex;

pub mod adapters;
pub mod compositor;
pub mod console;
pub mod cursor;
pub mod dialogs;
pub mod event;
pub mod graphics;
pub mod keyboard;
pub mod manager;
pub mod renderer;
pub mod screen;
pub mod selection;
pub mod terminal;
pub mod terminal_factory;
pub mod theme;
pub mod types;
pub mod windows;

pub use event::*;
pub use manager::*;
pub use screen::*;
pub use types::*;

// Re-export commonly used types
pub use self::cursor::CursorIcon;
pub use self::event::{Event, EventResult};
pub use self::graphics::{GraphicsDevice, WindowBuffer};
#[allow(unused_imports)]
pub use self::selection::{ClickMods, Selection, SelectionMode};
pub use self::types::{Point, Rect, ScreenId, WindowId};

// =====================================================================
// Widget colors
//
// The former PALETTE_* constants were replaced by the theme-dispatched
// control palette in `theme::controls` (`controls::palette()`), which
// reconciles default widget colors per the boot-selected Classic/Aero
// theme. Intentionally NOT covered by that palette (kept distinct on
// purpose):
// - DesktopWindow background (deep blue identity color).
// - TextWindow / TerminalWindow (dark grey background, light text).
// =====================================================================

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

    /// Whether this window needs [`Window::paint_overlay`] after its
    /// children render. Checked per frame so runtime theme changes can turn
    /// the pass on or off.
    fn wants_paint_overlay(&self) -> bool {
        false
    }

    /// Post-children paint pass, run with the same coordinate and clip
    /// setup as `paint`. Themes use it to re-carve translucent geometry —
    /// e.g. Futurism's rounded bottom corners — over pixels the content
    /// child painted flush to the window edge.
    fn paint_overlay(&mut self, _device: &mut dyn GraphicsDevice) {}

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
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
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

    /// Clear content invalidation after a retained position-only update.
    fn clear_needs_repaint(&mut self) {
        self.base_mut().clear_needs_repaint();
    }

    fn compositor_properties(&self) -> CompositorProperties {
        self.base().compositor_properties()
    }

    #[expect(dead_code, reason = "intentional kernel API surface")]
    fn set_compositor_properties(&mut self, properties: CompositorProperties) {
        self.base_mut().set_compositor_properties(properties);
    }

    fn composition_dirty(&self) -> bool {
        self.base().composition_dirty()
    }

    fn clear_composition_dirty(&mut self) {
        self.base_mut().clear_composition_dirty();
    }

    /// Extra retained-surface space for non-interactive decorations.
    fn decoration_insets(&self) -> Insets {
        Insets::ZERO
    }

    /// Check if this window currently has focus
    fn has_focus(&self) -> bool {
        self.base().has_focus()
    }

    /// Set the focus state of this window
    fn set_focus(&mut self, focused: bool) {
        self.base_mut().set_focus(focused);
    }

    /// Mouse-pointer image preferred at a point in this window's local
    /// coordinates. The manager asks the deepest hovered window; ordinary
    /// widgets retain the desktop arrow.
    fn cursor_icon_at(&self, _point: Point) -> CursorIcon {
        CursorIcon::Arrow
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

    /// Typed immutable accessor for decorated-frame policy and state.
    fn as_frame_window(&self) -> Option<&windows::frame::FrameWindow> {
        None
    }

    /// Take a title update intended for this window's enclosing frame.
    fn take_pending_frame_title(&mut self) -> Option<alloc::string::String> {
        None
    }

    /// Typed accessor used by the ring-3 title syscall.
    fn as_frame_window_mut(&mut self) -> Option<&mut windows::frame::FrameWindow> {
        None
    }

    /// Typed accessor used by the system-control service for live wallpaper
    /// replacement.
    fn as_desktop_window_mut(&mut self) -> Option<&mut windows::desktop::DesktopWindow> {
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
    #[allow(dead_code)]
    fn as_button_mut(&mut self) -> Option<&mut windows::button::Button> {
        None
    }

    /// Typed accessor used by `StatusBar` to update a section's text
    /// through the manager. Returns `None` by default; `Label`
    /// overrides it to return `Some(self)`.
    fn as_label_mut(&mut self) -> Option<&mut windows::label::Label> {
        None
    }

    /// Whether a frame close-button click should be delivered to this client
    /// instead of immediately destroying the frame.
    fn accepts_close_request(&self) -> bool {
        false
    }

    /// Typed accessor used by the ring-3 present syscall.
    fn as_remote_surface_mut(&mut self) -> Option<&mut windows::remote_surface::RemoteSurface> {
        None
    }

    /// VirGL client texture attached to this ring-3 content well, if any.
    fn external_gl_client(&self) -> Option<crate::graphics::composition::ClientGlId> {
        None
    }

    /// Typed accessor used by app code (e.g. File Explorer) to mutate
    /// a `MultiColumnList`'s rows through the manager.
    #[allow(dead_code)]
    fn as_multi_column_list_mut(
        &mut self,
    ) -> Option<&mut windows::multi_column_list::MultiColumnList> {
        None
    }

    /// Typed accessor used by app code (e.g. File Explorer) to mutate
    /// a `TreeView` through the manager.
    #[allow(dead_code)]
    fn as_tree_view_mut(&mut self) -> Option<&mut windows::tree_view::TreeView> {
        None
    }

    /// Typed accessor used by app code (e.g. File Explorer) to update
    /// a `PathBar`'s path through the manager.
    #[allow(dead_code)]
    fn as_path_bar_mut(&mut self) -> Option<&mut windows::path_bar::PathBar> {
        None
    }
}

/// Global window manager instance
static WINDOW_MANAGER: PreemptionMutex<Option<WindowManager>> = PreemptionMutex::new(None);

/// Initialize the window manager with a graphics device
pub fn init_window_manager(device: Box<dyn GraphicsDevice>) {
    let mut wm_lock = WINDOW_MANAGER.lock();
    *wm_lock = Some(WindowManager::new(device));
}

/// Execute a function with the window manager
///
/// Prevents kernel-thread preemption while holding the lock, avoiding a
/// single-CPU spin-mutex deadlock without masking the PIT or device IRQs.
/// Interrupt handlers must never acquire the window manager directly; they
/// enqueue input/work for the compositor instead.
pub fn with_window_manager<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut WindowManager) -> R,
{
    let mut wm_lock = WINDOW_MANAGER.lock();
    wm_lock.as_mut().map(f)
}

/// Path of the bundled default wallpaper on the FAT root. The basename is
/// 8.3-compliant — see `src/fs/CLAUDE.md` for the FAT layer's filename limit.
pub const DEFAULT_WALLPAPER_PATH: &str = "/WALLPAPR.BMP";
const MAX_WALLPAPER_BYTES: usize = 16 * 1024 * 1024;

/// Read and validate a BMP wallpaper from an absolute VFS path.
pub fn load_wallpaper(path: &str) -> Option<Vec<u8>> {
    let file = crate::fs::File::open_read(path).ok()?;
    let size = file.size() as usize;
    if size == 0 || size > MAX_WALLPAPER_BYTES {
        crate::debug_warn!("wallpaper rejected path={} size={}", path, size);
        return None;
    }
    let bytes = file.read_to_vec().ok()?;
    if crate::graphics::images::BmpImage::from_bytes(&bytes).is_err() {
        crate::debug_warn!("wallpaper rejected invalid BMP: {}", path);
        return None;
    }
    Some(bytes)
}

/// Read the default wallpaper file from the mounted filesystem and return its
/// raw bytes. Returns `None` on any failure — file missing, read error, or an
/// empty file. Callers fall back to a solid-color desktop when this returns
/// `None`, so a missing wallpaper never blocks boot.
pub fn load_default_wallpaper() -> Option<Vec<u8>> {
    load_wallpaper(DEFAULT_WALLPAPER_PATH).or_else(|| {
        crate::debug_info!(
            "Wallpaper {} unavailable; using solid background",
            DEFAULT_WALLPAPER_PATH
        );
        None
    })
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

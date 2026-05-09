//! Tests for `WindowManager::render` — dirty marking, skip-paint, and
//! drag/resize bounds-union behavior.
//!
//! Uses an in-tree `RecordingDevice` (records clip-rect changes and draw
//! calls) and `TestWindow` (a minimal `Window` impl that paints a single
//! `fill_rect` keyed to its bounds so the device can attribute draw calls
//! back to the originating window). The fixture is intentionally small —
//! enough for U3/U4/U5 assertions, not a full GUI emulator.

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::graphics::color::Color;
use crate::lib::test_utils::Testable;
use crate::window::{
    ColorDepth, Event, EventResult, GraphicsDevice, Rect, Window, WindowId, WindowManager,
};

// ---- RecordingDevice ----------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RecordedFill {
    pub clip: Option<Rect>,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub color: Color,
}

pub(super) struct RecordingDevice {
    width: usize,
    height: usize,
    clip: Option<Rect>,
    pub fills: Vec<RecordedFill>,
    pub clip_changes: Vec<Option<Rect>>,
}

impl RecordingDevice {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            clip: None,
            fills: Vec::new(),
            clip_changes: Vec::new(),
        }
    }

    /// Reset recorded operations between phases of a multi-step test.
    pub fn reset(&mut self) {
        self.fills.clear();
        self.clip_changes.clear();
    }
}

impl GraphicsDevice for RecordingDevice {
    fn width(&self) -> usize { self.width }
    fn height(&self) -> usize { self.height }
    fn color_depth(&self) -> ColorDepth { ColorDepth::Bit32 }

    fn clear(&mut self, _color: Color) {}
    fn draw_pixel(&mut self, _x: i32, _y: i32, _color: Color) {}
    fn read_pixel(&self, _x: i32, _y: i32) -> Color { Color::BLACK }
    fn draw_line(&mut self, _x1: i32, _y1: i32, _x2: i32, _y2: i32, _color: Color) {}
    fn draw_rect(&mut self, _x: i32, _y: i32, _width: u32, _height: u32, _color: Color) {}

    fn fill_rect(&mut self, x: i32, y: i32, width: u32, height: u32, color: Color) {
        self.fills.push(RecordedFill { clip: self.clip, x, y, width, height, color });
    }

    fn set_clip_rect(&mut self, rect: Option<Rect>) {
        self.clip = rect;
        self.clip_changes.push(rect);
    }

    fn flush(&mut self) {}
}

// ---- TestWindow ---------------------------------------------------------

/// Minimal window for assertions. paint() emits one `fill_rect` keyed by
/// the window's `paint_color` so a recording device can attribute fills
/// back to the originating window.
pub(super) struct TestWindow {
    pub id: WindowId,
    pub bounds: Rect,
    pub parent: Option<WindowId>,
    pub children: Vec<WindowId>,
    pub visible: bool,
    pub needs_repaint: bool,
    pub focused: bool,
    pub focusable: bool,
    pub paint_color: Color,
    pub paint_count: u32,
}

impl TestWindow {
    pub fn new(id: WindowId, bounds: Rect, paint_color: Color) -> Self {
        Self {
            id, bounds,
            parent: None,
            children: Vec::new(),
            visible: true,
            needs_repaint: true,
            focused: false,
            focusable: true,
            paint_color,
            paint_count: 0,
        }
    }
}

impl Window for TestWindow {
    fn id(&self) -> WindowId { self.id }
    fn bounds(&self) -> Rect { self.bounds }
    fn set_bounds(&mut self, bounds: Rect) {
        self.bounds = bounds;
        self.needs_repaint = true;
    }
    fn set_bounds_no_invalidate(&mut self, bounds: Rect) { self.bounds = bounds; }
    fn visible(&self) -> bool { self.visible }
    fn set_visible(&mut self, v: bool) { self.visible = v; }
    fn parent(&self) -> Option<WindowId> { self.parent }
    fn children(&self) -> &[WindowId] { &self.children }
    fn set_parent(&mut self, p: Option<WindowId>) { self.parent = p; }
    fn add_child(&mut self, c: WindowId) {
        self.children.retain(|&x| x != c);
        self.children.push(c);
    }
    fn remove_child(&mut self, c: WindowId) { self.children.retain(|&x| x != c); }
    fn paint(&mut self, device: &mut dyn GraphicsDevice) {
        self.paint_count += 1;
        device.fill_rect(
            self.bounds.x,
            self.bounds.y,
            self.bounds.width,
            self.bounds.height,
            self.paint_color,
        );
        self.needs_repaint = false;
    }
    fn needs_repaint(&self) -> bool { self.needs_repaint }
    fn invalidate(&mut self) { self.needs_repaint = true; }
    fn handle_event(&mut self, _event: Event) -> EventResult { EventResult::Ignored }
    fn can_focus(&self) -> bool { self.focusable }
    fn has_focus(&self) -> bool { self.focused }
    fn set_focus(&mut self, f: bool) { self.focused = f; }
}

// ---- Helpers ------------------------------------------------------------

/// Build a `WindowManager` whose graphics device is a `RecordingDevice` of
/// the given dimensions. Returns the manager; the device is owned by the
/// manager and is not directly accessible. Tests that need to inspect the
/// device must drive their assertions through the manager's render path.
fn make_manager(width: u32, height: u32) -> WindowManager {
    let device = Box::new(RecordingDevice::new(width as usize, height as usize));
    WindowManager::new(device)
}

/// Construct a fresh test scene rooted at a screen with a top-level "desktop"
/// window plus the given children. Returns (root_id, child_ids).
fn build_simple_scene(
    wm: &mut WindowManager,
    root_bounds: Rect,
    children: &[Rect],
) -> (WindowId, Vec<WindowId>) {
    use crate::window::ScreenMode;
    let screen_id = wm.create_screen(ScreenMode::Gui);
    wm.switch_screen(screen_id);

    let root_id = wm.create_window(None);
    let mut root = Box::new(TestWindow::new(root_id, root_bounds, Color::new(10, 10, 10)));
    root.focusable = false;
    wm.set_window_impl(root_id, root);

    if let Some(screen) = wm.get_active_screen_mut() {
        screen.set_root_window(root_id);
    }

    let mut child_ids = Vec::new();
    for (i, &cb) in children.iter().enumerate() {
        let cid = wm.create_window(Some(root_id));
        let mut cw = Box::new(TestWindow::new(cid, cb, Color::new(100 + i as u8, 0, 0)));
        cw.set_parent(Some(root_id));
        wm.set_window_impl(cid, cw);
        child_ids.push(cid);
    }
    (root_id, child_ids)
}

/// Mark all windows in the manager as not needing repaint, so subsequent
/// dirty-marking assertions only see the windows the test explicitly
/// invalidates.
fn quiesce(wm: &mut WindowManager) {
    let ids: Vec<WindowId> = wm.window_registry.keys().cloned().collect();
    for id in ids {
        if let Some(w) = wm.window_registry.get_mut(&id) {
            // Clear via paint into a throwaway device.
            let mut throwaway = RecordingDevice::new(0, 0);
            w.paint(&mut throwaway);
        }
    }
    wm.test_clear_dirty();
}

// ---- U3 tests: absolute-bounds dirty marking ----------------------------

fn test_top_level_dirty_marks_at_absolute_bounds() {
    let mut wm = make_manager(800, 600);
    let (_root_id, _children) = build_simple_scene(
        &mut wm,
        Rect::new(0, 0, 800, 600),
        &[Rect::new(100, 50, 200, 150)],
    );
    quiesce(&mut wm);

    // Mark the top-level child for repaint. Its parent (root) is at (0, 0)
    // so absolute == local. Either way the dirty rect should land at
    // (100, 50, 200, 150).
    let child_id = *wm.window_registry.keys().nth(1).unwrap();
    if let Some(w) = wm.window_registry.get_mut(&child_id) { w.invalidate(); }

    wm.test_mark_dirty_for_invalidated_windows();
    let dirty: Vec<Rect> = wm.test_dirty_regions();
    assert!(dirty.iter().any(|r| *r == Rect::new(100, 50, 200, 150)),
        "expected dirty at (100, 50, 200, 150), got {:?}", dirty);
}

fn test_nested_child_dirty_marks_at_parent_offset() {
    let mut wm = make_manager(800, 600);
    use crate::window::ScreenMode;
    let screen_id = wm.create_screen(ScreenMode::Gui);
    wm.switch_screen(screen_id);

    // Root at (0, 0).
    let root_id = wm.create_window(None);
    let mut root = Box::new(TestWindow::new(root_id, Rect::new(0, 0, 800, 600), Color::BLACK));
    root.focusable = false;
    wm.set_window_impl(root_id, root);
    if let Some(screen) = wm.get_active_screen_mut() { screen.set_root_window(root_id); }

    // Parent at (100, 50) under root.
    let parent_id = wm.create_window(Some(root_id));
    let mut parent = Box::new(TestWindow::new(parent_id, Rect::new(100, 50, 400, 300), Color::WHITE));
    parent.set_parent(Some(root_id));
    wm.set_window_impl(parent_id, parent);

    // Grandchild at LOCAL (10, 20) under parent => absolute (110, 70).
    let child_id = wm.create_window(Some(parent_id));
    let mut child = Box::new(TestWindow::new(child_id, Rect::new(10, 20, 50, 40), Color::WHITE));
    child.set_parent(Some(parent_id));
    wm.set_window_impl(child_id, child);

    quiesce(&mut wm);

    if let Some(w) = wm.window_registry.get_mut(&child_id) { w.invalidate(); }

    wm.test_mark_dirty_for_invalidated_windows();
    let dirty = wm.test_dirty_regions();
    // The pre-fix bug would have marked (10, 20, 50, 40) (local). The fix
    // marks the absolute (110, 70, 50, 40).
    assert!(dirty.iter().any(|r| *r == Rect::new(110, 70, 50, 40)),
        "expected dirty at absolute (110, 70, 50, 40), got {:?}", dirty);
    assert!(!dirty.iter().any(|r| *r == Rect::new(10, 20, 50, 40)),
        "must not mark at local-only coords (10, 20, 50, 40); got {:?}", dirty);
}

fn test_two_children_at_distinct_absolute_positions() {
    let mut wm = make_manager(800, 600);
    let (_root_id, child_ids) = build_simple_scene(
        &mut wm,
        Rect::new(0, 0, 800, 600),
        &[Rect::new(50, 50, 100, 100), Rect::new(400, 300, 100, 100)],
    );
    quiesce(&mut wm);

    for cid in &child_ids {
        if let Some(w) = wm.window_registry.get_mut(cid) { w.invalidate(); }
    }

    wm.test_mark_dirty_for_invalidated_windows();
    let dirty = wm.test_dirty_regions();
    assert!(dirty.iter().any(|r| *r == Rect::new(50, 50, 100, 100)));
    assert!(dirty.iter().any(|r| *r == Rect::new(400, 300, 100, 100)));
}

fn test_clean_window_does_not_register_dirty() {
    let mut wm = make_manager(800, 600);
    let _ = build_simple_scene(
        &mut wm,
        Rect::new(0, 0, 800, 600),
        &[Rect::new(50, 50, 100, 100)],
    );
    quiesce(&mut wm);

    // No invalidate: child is clean.
    wm.test_mark_dirty_for_invalidated_windows();
    let dirty = wm.test_dirty_regions();
    assert!(dirty.is_empty(), "expected no dirty rects, got {:?}", dirty);
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_top_level_dirty_marks_at_absolute_bounds,
        &test_nested_child_dirty_marks_at_parent_offset,
        &test_two_children_at_distinct_absolute_positions,
        &test_clean_window_does_not_register_dirty,
    ]
}

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
use spin::Mutex;

use crate::graphics::color::Color;
use crate::lib::arc::Arc;
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

pub(super) struct RecordedState {
    pub clip: Option<Rect>,
    pub fills: Vec<RecordedFill>,
    pub clip_changes: Vec<Option<Rect>>,
}

impl RecordedState {
    fn new() -> Self {
        Self { clip: None, fills: Vec::new(), clip_changes: Vec::new() }
    }

    pub fn reset(&mut self) {
        self.fills.clear();
        self.clip_changes.clear();
    }

    /// Find every fill recorded with the given color (each TestWindow paints
    /// a single fill keyed on its `paint_color`, so this answers "which paints
    /// of window-X happened, and under what clip rect?").
    pub fn fills_with_color(&self, color: Color) -> Vec<RecordedFill> {
        self.fills.iter().copied().filter(|f| f.color == color).collect()
    }
}

pub(super) struct RecordingDevice {
    width: usize,
    height: usize,
    state: Arc<Mutex<RecordedState>>,
}

impl RecordingDevice {
    /// Construct a paired device + shared state handle. Tests give the device
    /// to the `WindowManager`; the state handle stays in the test for
    /// inspection.
    pub fn new_paired(width: usize, height: usize)
        -> (Self, Arc<Mutex<RecordedState>>)
    {
        let state = Arc::new(Mutex::new(RecordedState::new()));
        let dev = Self { width, height, state: state.clone() };
        (dev, state)
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
        let mut s = self.state.lock();
        let clip = s.clip;
        s.fills.push(RecordedFill { clip, x, y, width, height, color });
    }

    fn set_clip_rect(&mut self, rect: Option<Rect>) {
        let mut s = self.state.lock();
        s.clip = rect;
        s.clip_changes.push(rect);
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
/// the given dimensions. Returns the manager and a handle on the device's
/// shared state so the test can inspect what was drawn.
fn make_manager(width: u32, height: u32)
    -> (WindowManager, Arc<Mutex<RecordedState>>)
{
    let (device, state) = RecordingDevice::new_paired(width as usize, height as usize);
    (WindowManager::new(Box::new(device)), state)
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
            let (mut throwaway, _ignore) = RecordingDevice::new_paired(0, 0);
            w.paint(&mut throwaway);
        }
    }
    wm.test_clear_dirty();
}

// ---- U3 tests: absolute-bounds dirty marking ----------------------------

fn test_top_level_dirty_marks_at_absolute_bounds() {
    let (mut wm, _state) = make_manager(800, 600);
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
    let (mut wm, _state) = make_manager(800, 600);
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
    let (mut wm, _state) = make_manager(800, 600);
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
    let (mut wm, _state) = make_manager(800, 600);
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

// ---- U4 tests: skip-paint + dirty-clip ---------------------------------

const ROOT_COLOR: Color = Color { red: 10, green: 10, blue: 10 };
const CHILD0_COLOR: Color = Color { red: 100, green: 0, blue: 0 };

fn test_clean_distant_window_skipped() {
    let (mut wm, state) = make_manager(800, 600);
    let _ = build_simple_scene(
        &mut wm,
        Rect::new(0, 0, 800, 600),
        &[Rect::new(400, 400, 50, 50)], // far from cursor area
    );
    quiesce(&mut wm);
    state.lock().reset();

    // Cursor-sized dirty rect well away from the child window.
    wm.test_mark_dirty(Rect::new(10, 10, 22, 22));
    wm.test_render_active_screen();

    let s = state.lock();
    assert!(
        s.fills_with_color(CHILD0_COLOR).is_empty(),
        "clean window outside dirty union must be skipped; got fills {:?}",
        s.fills_with_color(CHILD0_COLOR)
    );
    // Root is full-screen so it always intersects any dirty rect — it should
    // still paint (this is the AE2-residual cost the desktop opt-in resolves
    // in U8/U9, not at this layer).
    assert!(
        !s.fills_with_color(ROOT_COLOR).is_empty(),
        "full-screen root should paint when dirty intersects"
    );
}

fn test_window_intersecting_dirty_paints_with_clipped_rect() {
    let (mut wm, state) = make_manager(800, 600);
    let _ = build_simple_scene(
        &mut wm,
        Rect::new(0, 0, 800, 600),
        &[Rect::new(50, 50, 100, 100)],
    );
    quiesce(&mut wm);
    state.lock().reset();

    // Dirty fully inside child bounds.
    wm.test_mark_dirty(Rect::new(75, 75, 50, 50));
    wm.test_render_active_screen();

    let s = state.lock();
    let fills = s.fills_with_color(CHILD0_COLOR);
    assert_eq!(fills.len(), 1, "child should paint exactly once");
    // Clip is bounds ∩ dirty_bbox = (50..150) ∩ (75..125) = (75, 75, 50, 50).
    assert_eq!(fills[0].clip, Some(Rect::new(75, 75, 50, 50)));
}

fn test_full_repaint_clips_to_window_bounds() {
    let (mut wm, state) = make_manager(800, 600);
    let _ = build_simple_scene(
        &mut wm,
        Rect::new(0, 0, 800, 600),
        &[Rect::new(50, 50, 100, 100)],
    );
    quiesce(&mut wm);
    state.lock().reset();

    wm.test_mark_full_repaint();
    // mark_full_repaint alone doesn't flag windows for repaint; render() does
    // that via its own loop. Mirror that here so the test mirrors render's
    // contract.
    let ids: Vec<WindowId> = wm.window_registry.keys().cloned().collect();
    for id in ids {
        if let Some(w) = wm.window_registry.get_mut(&id) { w.invalidate(); }
    }

    wm.test_render_active_screen();

    let s = state.lock();
    let fills = s.fills_with_color(CHILD0_COLOR);
    assert_eq!(fills.len(), 1);
    // Full-repaint dirty bbox is the whole screen → clip ∩ bounds = bounds.
    assert_eq!(fills[0].clip, Some(Rect::new(50, 50, 100, 100)));
}

fn test_parent_painted_propagates_to_clean_non_intersecting_child() {
    // The doc-review-issue regression guard for `will_repaint = should_paint`.
    // A parent that paints because its bounds intersected the dirty union
    // (not because of an explicit invalidate) must still mark its children
    // dirty so they paint over the parent's overdraw.
    let (mut wm, state) = make_manager(800, 600);
    let _ = build_simple_scene(
        &mut wm,
        Rect::new(0, 0, 800, 600),       // root: full screen
        &[Rect::new(50, 50, 80, 80)],    // child: (50, 50, 80, 80)
    );
    quiesce(&mut wm);
    state.lock().reset();

    // Dirty rect lands inside root but outside child:
    //   root  = (0..800, 0..600)
    //   child = (50..130, 50..130)
    //   dirty = (10..30, 10..30) — intersects root, not child.
    wm.test_mark_dirty(Rect::new(10, 10, 20, 20));
    wm.test_render_active_screen();

    let s = state.lock();
    assert!(!s.fills_with_color(ROOT_COLOR).is_empty(), "root should paint");
    assert!(
        !s.fills_with_color(CHILD0_COLOR).is_empty(),
        "child must paint after parent overdraw, even though child neither \
         had needs_repaint=true nor intersected the dirty rect — propagation \
         relies on will_repaint = should_paint"
    );
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        // U3 — absolute-bounds dirty marking
        &test_top_level_dirty_marks_at_absolute_bounds,
        &test_nested_child_dirty_marks_at_parent_offset,
        &test_two_children_at_distinct_absolute_positions,
        &test_clean_window_does_not_register_dirty,
        // U4 — skip-paint + dirty-clip
        &test_clean_distant_window_skipped,
        &test_window_intersecting_dirty_paints_with_clipped_rect,
        &test_full_repaint_clips_to_window_bounds,
        &test_parent_painted_propagates_to_clean_non_intersecting_child,
    ]
}

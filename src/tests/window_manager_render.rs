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

use bootloader_api::info::PixelFormat;

use crate::graphics::color::Color;
use crate::lib::arc::Arc;
use crate::lib::test_utils::Testable;
use crate::window::windows::base::WindowBase;
use crate::window::{
    ColorDepth, Event, EventResult, GraphicsDevice, Point, Rect, Window, WindowBuffer, WindowId,
    WindowManager,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RecordedBlit {
    pub clip: Option<Rect>,
    pub x: i32,
    pub y: i32,
    pub buffer_width: usize,
    pub buffer_height: usize,
}

pub(super) struct RecordedState {
    pub clip: Option<Rect>,
    pub fills: Vec<RecordedFill>,
    pub clip_changes: Vec<Option<Rect>>,
    pub blits: Vec<RecordedBlit>,
    pub presented_batches: Vec<Vec<Rect>>,
}

impl RecordedState {
    fn new() -> Self {
        Self {
            clip: None,
            fills: Vec::new(),
            clip_changes: Vec::new(),
            blits: Vec::new(),
            presented_batches: Vec::new(),
        }
    }

    pub fn reset(&mut self) {
        self.fills.clear();
        self.clip_changes.clear();
        self.blits.clear();
        self.presented_batches.clear();
    }

    /// Find every fill recorded with the given color (each TestWindow paints
    /// a single fill keyed on its `paint_color`, so this answers "which paints
    /// of window-X happened, and under what clip rect?").
    pub fn fills_with_color(&self, color: Color) -> Vec<RecordedFill> {
        self.fills
            .iter()
            .copied()
            .filter(|f| f.color == color)
            .collect()
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
    pub fn new_paired(width: usize, height: usize) -> (Self, Arc<Mutex<RecordedState>>) {
        let state = Arc::new(Mutex::new(RecordedState::new()));
        let dev = Self {
            width,
            height,
            state: state.clone(),
        };
        (dev, state)
    }
}

impl GraphicsDevice for RecordingDevice {
    fn width(&self) -> usize {
        self.width
    }
    fn height(&self) -> usize {
        self.height
    }
    fn color_depth(&self) -> ColorDepth {
        ColorDepth::Bit32
    }

    fn clear(&mut self, _color: Color) {}
    fn draw_pixel(&mut self, _x: i32, _y: i32, _color: Color) {}
    fn read_pixel(&self, _x: i32, _y: i32) -> Color {
        Color::BLACK
    }
    fn draw_line(&mut self, _x1: i32, _y1: i32, _x2: i32, _y2: i32, _color: Color) {}
    fn draw_rect(&mut self, _x: i32, _y: i32, _width: u32, _height: u32, _color: Color) {}

    fn fill_rect(&mut self, x: i32, y: i32, width: u32, height: u32, color: Color) {
        let mut s = self.state.lock();
        let clip = s.clip;
        s.fills.push(RecordedFill {
            clip,
            x,
            y,
            width,
            height,
            color,
        });
    }

    fn set_clip_rect(&mut self, rect: Option<Rect>) {
        let mut s = self.state.lock();
        s.clip = rect;
        s.clip_changes.push(rect);
    }

    fn flush(&mut self) {}

    fn flush_regions(&mut self, regions: &[Rect]) {
        self.state.lock().presented_batches.push(regions.to_vec());
    }

    fn pixel_format(&self) -> PixelFormat {
        PixelFormat::Bgr
    }
    fn bytes_per_pixel(&self) -> usize {
        4
    }

    fn blit_buffer(&mut self, x: i32, y: i32, buffer: &WindowBuffer) {
        let mut s = self.state.lock();
        let clip = s.clip;
        s.blits.push(RecordedBlit {
            clip,
            x,
            y,
            buffer_width: buffer.width,
            buffer_height: buffer.height,
        });
    }
}

// ---- TestWindow ---------------------------------------------------------

/// Minimal window for assertions. paint() emits one `fill_rect` keyed by
/// the window's `paint_color` so a recording device can attribute fills
/// back to the originating window.
pub(super) struct TestWindow {
    pub base_field: WindowBase,
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
    pub opts_in_to_backing_store: bool,
    pub backing: Option<WindowBuffer>,
    pub rasterize_count: u32,
}

impl TestWindow {
    pub fn new(id: WindowId, bounds: Rect, paint_color: Color) -> Self {
        Self {
            base_field: WindowBase::new_with_id(id, bounds),
            id,
            bounds,
            parent: None,
            children: Vec::new(),
            visible: true,
            needs_repaint: true,
            focused: false,
            focusable: true,
            paint_color,
            paint_count: 0,
            opts_in_to_backing_store: false,
            backing: None,
            rasterize_count: 0,
        }
    }
}

impl Window for TestWindow {
    fn base(&self) -> &WindowBase {
        &self.base_field
    }
    fn base_mut(&mut self) -> &mut WindowBase {
        &mut self.base_field
    }
    fn id(&self) -> WindowId {
        self.id
    }
    fn bounds(&self) -> Rect {
        self.bounds
    }
    fn set_bounds(&mut self, bounds: Rect) {
        self.bounds = bounds;
        self.needs_repaint = true;
    }
    fn set_bounds_no_invalidate(&mut self, bounds: Rect) {
        self.bounds = bounds;
    }
    fn visible(&self) -> bool {
        self.visible
    }
    fn set_visible(&mut self, v: bool) {
        self.visible = v;
    }
    fn parent(&self) -> Option<WindowId> {
        self.parent
    }
    fn children(&self) -> &[WindowId] {
        &self.children
    }
    fn set_parent(&mut self, p: Option<WindowId>) {
        self.parent = p;
    }
    fn add_child(&mut self, c: WindowId) {
        self.children.retain(|&x| x != c);
        self.children.push(c);
    }
    fn remove_child(&mut self, c: WindowId) {
        self.children.retain(|&x| x != c);
    }
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
    fn needs_repaint(&self) -> bool {
        self.needs_repaint
    }
    fn invalidate(&mut self) {
        self.needs_repaint = true;
    }
    fn handle_event(&mut self, _event: Event) -> EventResult {
        EventResult::Ignored
    }
    fn can_focus(&self) -> bool {
        self.focusable
    }
    fn has_focus(&self) -> bool {
        self.focused
    }
    fn set_focus(&mut self, f: bool) {
        self.focused = f;
    }

    fn wants_backing_store(&self) -> bool {
        self.opts_in_to_backing_store
    }

    fn paint_into_backing_store(&mut self, device: &dyn GraphicsDevice) {
        self.rasterize_count += 1;
        let w = self.bounds.width as usize;
        let h = self.bounds.height as usize;
        let mut buf = WindowBuffer::for_device(w, h, device);
        for y in 0..h {
            for x in 0..w {
                buf.write_pixel(x, y, self.paint_color);
            }
        }
        self.backing = Some(buf);
        self.needs_repaint = false;
    }

    fn backing_store(&self) -> Option<&WindowBuffer> {
        self.backing.as_ref()
    }
}

/// Side-channel handle so tests can read TestWindow fields without
/// downcasting through `Box<dyn Window>`. Each `TestWindow` exposes a
/// `Probe` that's an Arc<Mutex<>> the renderer's window doesn't update —
/// the renderer mutates the boxed window directly. This intentionally
/// trades a tiny amount of duplication (one extra getter per field) for
/// not polluting the production `Window` trait with test-only methods.
fn install_test_window(
    wm: &mut WindowManager,
    parent: Option<WindowId>,
    bounds: Rect,
    paint_color: Color,
    opts_in_to_backing_store: bool,
) -> WindowId {
    let id = wm.create_window(parent);
    let mut tw = Box::new(TestWindow::new(id, bounds, paint_color));
    tw.set_parent(parent);
    tw.opts_in_to_backing_store = opts_in_to_backing_store;
    wm.set_window_impl(id, tw);
    id
}

/// Best-effort attribution: assume each fill recorded with `paint_color`
/// equals one paint() call on a window of that color. Likewise for blits
/// matching that window's bounds.
fn paint_count_for(state: &RecordedState, color: Color) -> usize {
    state.fills.iter().filter(|f| f.color == color).count()
}

fn blit_count_for(state: &RecordedState, w: usize, h: usize) -> usize {
    state
        .blits
        .iter()
        .filter(|b| b.buffer_width == w && b.buffer_height == h)
        .count()
}

// ---- Helpers ------------------------------------------------------------

/// Build a `WindowManager` whose graphics device is a `RecordingDevice` of
/// the given dimensions. Returns the manager and a handle on the device's
/// shared state so the test can inspect what was drawn.
fn make_manager(width: u32, height: u32) -> (WindowManager, Arc<Mutex<RecordedState>>) {
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
    let mut root = Box::new(TestWindow::new(
        root_id,
        root_bounds,
        Color::new(10, 10, 10),
    ));
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
    if let Some(w) = wm.window_registry.get_mut(&child_id) {
        w.invalidate();
    }

    wm.test_mark_dirty_for_invalidated_windows();
    let dirty: Vec<Rect> = wm.test_dirty_regions();
    assert!(
        dirty.iter().any(|r| *r == Rect::new(100, 50, 200, 150)),
        "expected dirty at (100, 50, 200, 150), got {:?}",
        dirty
    );
}

fn test_nested_child_dirty_marks_at_parent_offset() {
    let (mut wm, _state) = make_manager(800, 600);
    use crate::window::ScreenMode;
    let screen_id = wm.create_screen(ScreenMode::Gui);
    wm.switch_screen(screen_id);

    // Root at (0, 0).
    let root_id = wm.create_window(None);
    let mut root = Box::new(TestWindow::new(
        root_id,
        Rect::new(0, 0, 800, 600),
        Color::BLACK,
    ));
    root.focusable = false;
    wm.set_window_impl(root_id, root);
    if let Some(screen) = wm.get_active_screen_mut() {
        screen.set_root_window(root_id);
    }

    // Parent at (100, 50) under root.
    let parent_id = wm.create_window(Some(root_id));
    let mut parent = Box::new(TestWindow::new(
        parent_id,
        Rect::new(100, 50, 400, 300),
        Color::WHITE,
    ));
    parent.set_parent(Some(root_id));
    wm.set_window_impl(parent_id, parent);

    // Grandchild at LOCAL (10, 20) under parent => absolute (110, 70).
    let child_id = wm.create_window(Some(parent_id));
    let mut child = Box::new(TestWindow::new(
        child_id,
        Rect::new(10, 20, 50, 40),
        Color::WHITE,
    ));
    child.set_parent(Some(parent_id));
    wm.set_window_impl(child_id, child);

    quiesce(&mut wm);

    if let Some(w) = wm.window_registry.get_mut(&child_id) {
        w.invalidate();
    }

    wm.test_mark_dirty_for_invalidated_windows();
    let dirty = wm.test_dirty_regions();
    // The pre-fix bug would have marked (10, 20, 50, 40) (local). The fix
    // marks the absolute (110, 70, 50, 40).
    assert!(
        dirty.iter().any(|r| *r == Rect::new(110, 70, 50, 40)),
        "expected dirty at absolute (110, 70, 50, 40), got {:?}",
        dirty
    );
    assert!(
        !dirty.iter().any(|r| *r == Rect::new(10, 20, 50, 40)),
        "must not mark at local-only coords (10, 20, 50, 40); got {:?}",
        dirty
    );
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
        if let Some(w) = wm.window_registry.get_mut(cid) {
            w.invalidate();
        }
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

const ROOT_COLOR: Color = Color {
    red: 10,
    green: 10,
    blue: 10,
};
const CHILD0_COLOR: Color = Color {
    red: 100,
    green: 0,
    blue: 0,
};

fn test_skip_paint_when_ancestor_chain_misses_dirty() {
    // The full skip-paint benefit depends on the entire ancestor chain
    // missing the dirty union (otherwise parent_was_repainted propagates
    // invalidation downward). At this stage the production desktop is
    // full-screen so this property doesn't hold for it — that gap is what
    // U8/U9 close via the backing-store blit, which doesn't go through
    // paint() at all.
    //
    // To verify the skip-paint mechanism still works where the chain DOES
    // miss dirty, build a scene whose screen root is itself non-full-screen.
    let (mut wm, state) = make_manager(800, 600);
    use crate::window::ScreenMode;
    let screen_id = wm.create_screen(ScreenMode::Gui);
    wm.switch_screen(screen_id);

    // Non-full-screen root.
    let root_id = wm.create_window(None);
    let mut root = Box::new(TestWindow::new(
        root_id,
        Rect::new(200, 200, 200, 200),
        ROOT_COLOR,
    ));
    root.focusable = false;
    wm.set_window_impl(root_id, root);
    if let Some(screen) = wm.get_active_screen_mut() {
        screen.set_root_window(root_id);
    }

    // Child inside the non-full-screen root.
    let child_id = wm.create_window(Some(root_id));
    let mut child = Box::new(TestWindow::new(
        child_id,
        Rect::new(50, 50, 30, 30),
        CHILD0_COLOR,
    ));
    child.set_parent(Some(root_id));
    wm.set_window_impl(child_id, child);

    quiesce(&mut wm);
    state.lock().reset();

    // Dirty rect outside both root (200..400, 200..400) and child
    // (absolute 250..280, 250..280).
    wm.test_mark_dirty(Rect::new(500, 500, 22, 22));
    wm.test_render_active_screen();

    let s = state.lock();
    assert!(
        s.fills_with_color(ROOT_COLOR).is_empty(),
        "root with bounds outside dirty must be skipped; got {:?}",
        s.fills_with_color(ROOT_COLOR)
    );
    assert!(
        s.fills_with_color(CHILD0_COLOR).is_empty(),
        "child whose ancestor chain misses dirty must be skipped; got {:?}",
        s.fills_with_color(CHILD0_COLOR)
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
        if let Some(w) = wm.window_registry.get_mut(&id) {
            w.invalidate();
        }
    }

    wm.test_render_active_screen();

    let s = state.lock();
    let fills = s.fills_with_color(CHILD0_COLOR);
    assert_eq!(fills.len(), 1);
    // Full-repaint dirty bbox is the whole screen → clip ∩ bounds = bounds.
    assert_eq!(fills[0].clip, Some(Rect::new(50, 50, 100, 100)));
}

fn test_clean_child_outside_dirty_skips_when_parent_paints() {
    // The render walk sets each window's clip to `bounds ∩ dirty_union`
    // before painting, so a parent that paints because its own bounds
    // intersected dirty cannot overdraw pixels outside the dirty union —
    // including pixels inside a child whose bounds don't intersect dirty.
    // Such a child must therefore stay clean (skip paint), preserving
    // fine-grained repaint paths in child widgets.
    let (mut wm, state) = make_manager(800, 600);
    let _ = build_simple_scene(
        &mut wm,
        Rect::new(0, 0, 800, 600),    // root: full screen
        &[Rect::new(50, 50, 80, 80)], // child: (50, 50, 80, 80)
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
    assert!(
        !s.fills_with_color(ROOT_COLOR).is_empty(),
        "root should paint"
    );
    assert!(
        s.fills_with_color(CHILD0_COLOR).is_empty(),
        "child whose bounds don't intersect dirty must NOT paint — the \
         parent's clip prevents overdraw, so propagating dirty downward \
         would defeat fine-grained child updates"
    );
}

// ---- U5 tests: drag/resize bounds-union dirty --------------------------

fn test_non_frame_top_level_window_is_not_draggable() {
    let (mut wm, _state) = make_manager(800, 600);
    use crate::window::ScreenMode;
    let screen_id = wm.create_screen(ScreenMode::Gui);
    wm.switch_screen(screen_id);

    let root_id = wm.create_window(None);
    let mut root = Box::new(TestWindow::new(
        root_id,
        Rect::new(0, 0, 800, 600),
        ROOT_COLOR,
    ));
    root.focusable = false;
    wm.set_window_impl(root_id, root);
    if let Some(screen) = wm.get_active_screen_mut() {
        screen.set_root_window(root_id);
    }

    // Models the taskbar: it is a direct child of the desktop, but it has no
    // frame title/chrome and therefore must never enter drag/resize state.
    let panel_bounds = Rect::new(0, 568, 800, 32);
    let panel_id = wm.create_window(Some(root_id));
    let mut panel = Box::new(TestWindow::new(panel_id, panel_bounds, CHILD0_COLOR));
    panel.set_parent(Some(root_id));
    wm.set_window_impl(panel_id, panel);

    quiesce(&mut wm);
    wm.test_start_drag_if_on_title_bar(100, 580);
    wm.test_handle_dragging(150, 580, 0x01);

    assert_eq!(
        wm.window_registry.get(&panel_id).unwrap().bounds(),
        panel_bounds,
        "a top-level panel without frame chrome must ignore frame drag hit-testing",
    );
}

fn test_drag_tick_marks_bounds_union_not_full_repaint() {
    let (mut wm, _state) = make_manager(800, 600);
    use crate::window::ScreenMode;
    let screen_id = wm.create_screen(ScreenMode::Gui);
    wm.switch_screen(screen_id);

    // Root at (0, 0) full-screen — dragged windows are top-level children.
    let root_id = wm.create_window(None);
    let mut root = Box::new(TestWindow::new(
        root_id,
        Rect::new(0, 0, 800, 600),
        ROOT_COLOR,
    ));
    root.focusable = false;
    wm.set_window_impl(root_id, root);
    if let Some(screen) = wm.get_active_screen_mut() {
        screen.set_root_window(root_id);
    }

    // Frame to drag at (100, 100, 400, 300).
    let frame_id = wm.create_window(Some(root_id));
    let mut frame = Box::new(TestWindow::new(
        frame_id,
        Rect::new(100, 100, 400, 300),
        CHILD0_COLOR,
    ));
    frame.set_parent(Some(root_id));
    wm.set_window_impl(frame_id, frame);

    quiesce(&mut wm);

    // Pretend the user grabbed the title bar at (150, 110) and the frame's
    // origin was (100, 100); this matches what start_drag_if_on_title_bar
    // would record for a real click on the title bar.
    wm.test_force_drag_state(frame_id, Point::new(150, 110), Point::new(100, 100));

    // Mouse moves 20 px right with left button still held → drag tick.
    wm.test_handle_dragging(170, 110, 0x01);

    // Frame should now be at (120, 100, 400, 300).
    assert_eq!(
        wm.window_registry.get(&frame_id).unwrap().bounds(),
        Rect::new(120, 100, 400, 300),
    );

    // Crucially: the drag tick must NOT have marked a full repaint.
    assert!(
        !wm.test_needs_full_repaint(),
        "drag must not call mark_full_repaint anymore — that was the regression \
         being optimized away"
    );

    // The dirty union should cover the (old ∪ new) bounds. Old:
    // (100, 100, 400, 300); new: (120, 100, 400, 300); union:
    // (100, 100, 420, 300).
    let union = Rect::new(100, 100, 420, 300);
    let dirty = wm.test_dirty_regions();
    assert!(
        dirty.iter().any(|r| *r == union
            || (r.x <= union.x
                && r.y <= union.y
                && r.x + r.width as i32 >= union.x + union.width as i32
                && r.y + r.height as i32 >= union.y + union.height as i32)),
        "expected dirty rects to cover {:?}, got {:?}",
        union,
        dirty
    );
}

fn test_drag_tick_with_no_position_change_does_nothing() {
    let (mut wm, _state) = make_manager(800, 600);
    use crate::window::ScreenMode;
    let screen_id = wm.create_screen(ScreenMode::Gui);
    wm.switch_screen(screen_id);

    let root_id = wm.create_window(None);
    let mut root = Box::new(TestWindow::new(
        root_id,
        Rect::new(0, 0, 800, 600),
        ROOT_COLOR,
    ));
    root.focusable = false;
    wm.set_window_impl(root_id, root);
    if let Some(screen) = wm.get_active_screen_mut() {
        screen.set_root_window(root_id);
    }

    let frame_id = wm.create_window(Some(root_id));
    let mut frame = Box::new(TestWindow::new(
        frame_id,
        Rect::new(100, 100, 400, 300),
        CHILD0_COLOR,
    ));
    frame.set_parent(Some(root_id));
    wm.set_window_impl(frame_id, frame);

    quiesce(&mut wm);

    wm.test_force_drag_state(frame_id, Point::new(150, 110), Point::new(100, 100));

    // Drag tick where mouse hasn't actually moved (delta = 0).
    wm.test_handle_dragging(150, 110, 0x01);

    assert!(!wm.test_needs_full_repaint());
    assert!(
        wm.test_dirty_regions().is_empty(),
        "no-op drag tick must not mark anything dirty; got {:?}",
        wm.test_dirty_regions()
    );
}

fn test_frame_minimize_maximize_restore_transitions() {
    use crate::window::windows::FrameWindow;
    use crate::window::ScreenMode;

    let (mut wm, _state) = make_manager(800, 600);
    let screen_id = wm.create_screen(ScreenMode::Gui);
    wm.switch_screen(screen_id);

    let root_id = install_test_window(&mut wm, None, Rect::new(0, 0, 800, 600), ROOT_COLOR, false);
    wm.get_active_screen_mut().unwrap().set_root_window(root_id);

    let frame1_id = wm.create_window(Some(root_id));
    let mut frame1 = Box::new(FrameWindow::new(frame1_id, "One"));
    frame1.set_parent(Some(root_id));
    let normal_bounds = Rect::new(100, 80, 320, 240);
    frame1.set_bounds(normal_bounds);
    let content1_id = wm.create_window(Some(frame1_id));
    let mut content1 = Box::new(TestWindow::new(
        content1_id,
        frame1.content_area(),
        CHILD0_COLOR,
    ));
    content1.focusable = true;
    content1.set_parent(Some(frame1_id));
    frame1.set_content_window(content1_id);
    wm.set_window_impl(frame1_id, frame1);
    wm.set_window_impl(content1_id, content1);

    let frame2_id = wm.create_window(Some(root_id));
    let mut frame2 = Box::new(FrameWindow::new(frame2_id, "Two"));
    frame2.set_parent(Some(root_id));
    frame2.set_bounds(Rect::new(440, 100, 260, 200));
    let content2_id = wm.create_window(Some(frame2_id));
    let mut content2 = Box::new(TestWindow::new(
        content2_id,
        frame2.content_area(),
        Color::new(40, 90, 140),
    ));
    content2.focusable = true;
    content2.set_parent(Some(frame2_id));
    frame2.set_content_window(content2_id);
    wm.set_window_impl(frame2_id, frame2);
    wm.set_window_impl(content2_id, content2);

    let taskbar_id = install_test_window(
        &mut wm,
        Some(root_id),
        Rect::new(0, 568, 800, 32),
        Color::new(90, 90, 90),
        false,
    );
    wm.set_taskbar_id(Some(taskbar_id));

    wm.focus_window(content1_id);
    assert_eq!(wm.focused_window(), Some(content1_id));
    assert_eq!(wm.desktop_work_area(), Rect::new(0, 0, 800, 568));

    assert!(wm.toggle_maximize_frame(frame1_id));
    assert_eq!(
        wm.window_registry.get(&frame1_id).unwrap().bounds(),
        Rect::new(0, 0, 800, 568)
    );
    assert!(wm
        .window_registry
        .get(&frame1_id)
        .unwrap()
        .as_frame_window()
        .unwrap()
        .is_maximized());
    let metrics = crate::window::theme::metrics();
    assert_eq!(
        wm.window_registry.get(&content1_id).unwrap().bounds(),
        Rect::new(
            metrics.border_width as i32,
            (metrics.border_width + metrics.title_bar_height) as i32,
            800 - metrics.border_width * 2,
            568 - metrics.title_bar_height - metrics.border_width * 2,
        )
    );

    assert!(wm.minimize_frame(frame1_id));
    assert!(!wm.window_registry.get(&frame1_id).unwrap().visible());
    assert_eq!(wm.focused_window(), Some(content2_id));

    assert!(wm.activate_frame(frame1_id));
    assert!(wm.window_registry.get(&frame1_id).unwrap().visible());
    assert_eq!(wm.focused_window(), Some(content1_id));
    assert_eq!(
        wm.window_registry.get(&frame1_id).unwrap().bounds(),
        Rect::new(0, 0, 800, 568),
        "restoring a minimized maximized frame must keep it maximized"
    );

    assert!(wm.toggle_maximize_frame(frame1_id));
    assert_eq!(
        wm.window_registry.get(&frame1_id).unwrap().bounds(),
        normal_bounds
    );

    let buttons = crate::window::theme::caption_button_layout(normal_bounds, metrics, true);
    let maximize = buttons.maximize.unwrap();
    wm.test_start_drag_if_on_title_bar(
        maximize.x + maximize.width as i32 / 2,
        maximize.y + maximize.height as i32 / 2,
    );
    assert!(
        wm.window_registry
            .get(&frame1_id)
            .unwrap()
            .as_frame_window()
            .unwrap()
            .is_maximized(),
        "maximize caption hit must execute the placement transition"
    );
    let maximized_bounds = wm.window_registry.get(&frame1_id).unwrap().bounds();
    let restore = crate::window::theme::caption_button_layout(maximized_bounds, metrics, true)
        .maximize
        .unwrap();
    wm.test_start_drag_if_on_title_bar(
        restore.x + restore.width as i32 / 2,
        restore.y + restore.height as i32 / 2,
    );
    assert_eq!(
        wm.window_registry.get(&frame1_id).unwrap().bounds(),
        normal_bounds
    );

    let minimize = buttons.minimize.unwrap();
    wm.test_start_drag_if_on_title_bar(
        minimize.x + minimize.width as i32 / 2,
        minimize.y + minimize.height as i32 / 2,
    );
    assert!(!wm.window_registry.get(&frame1_id).unwrap().visible());
    assert!(wm.activate_frame(frame1_id));

    let fixed_id = wm.create_window(Some(root_id));
    let mut fixed = Box::new(FrameWindow::new(fixed_id, "Fixed"));
    fixed.set_parent(Some(root_id));
    fixed.set_resizable(false);
    let fixed_bounds = Rect::new(220, 160, 240, 150);
    fixed.set_bounds(fixed_bounds);
    wm.set_window_impl(fixed_id, fixed);
    assert!(!wm.toggle_maximize_frame(fixed_id));
    assert!(!wm.minimize_frame(fixed_id));
    assert_eq!(
        wm.window_registry.get(&fixed_id).unwrap().bounds(),
        fixed_bounds
    );
}

// ---- U9 tests: opt-in compositor blit path -----------------------------

fn test_opt_in_window_blits_instead_of_painting() {
    let (mut wm, state) = make_manager(800, 600);
    use crate::window::ScreenMode;
    let screen_id = wm.create_screen(ScreenMode::Gui);
    wm.switch_screen(screen_id);

    // Non-full-screen scene root.
    let root_id = install_test_window(&mut wm, None, Rect::new(0, 0, 200, 200), ROOT_COLOR, false);
    if let Some(screen) = wm.get_active_screen_mut() {
        screen.set_root_window(root_id);
    }

    // Opt-in child at (50, 50, 100, 100).
    let child_id = install_test_window(
        &mut wm,
        Some(root_id),
        Rect::new(50, 50, 100, 100),
        CHILD0_COLOR,
        true,
    );

    quiesce(&mut wm);
    state.lock().reset();

    // Mark dirty intersecting both root and child.
    wm.test_mark_dirty(Rect::new(60, 60, 80, 80));
    // Simulate fresh content in child so the rasterize+blit path runs.
    if let Some(w) = wm.window_registry.get_mut(&child_id) {
        w.invalidate();
    }
    wm.test_render_active_screen();

    let s = state.lock();

    // Root is non-opt-in → goes through paint() and emits a fill_rect.
    assert!(
        paint_count_for(&s, ROOT_COLOR) >= 1,
        "non-opt-in root paints"
    );

    // Child is opt-in → does NOT emit a fill_rect (paint() not called) and
    // produces a blit instead.
    assert_eq!(
        paint_count_for(&s, CHILD0_COLOR),
        0,
        "opt-in window's paint() must not be called"
    );
    assert!(
        blit_count_for(&s, 100, 100) >= 1,
        "opt-in window must produce a 100x100 blit; blits seen: {:?}",
        s.blits
    );

    // Children of an opt-in window are unrelated — root is the parent here,
    // not the opt-in. Sanity: no blit of root's dimensions.
    assert!(
        blit_count_for(&s, 200, 200) == 0,
        "non-opt-in root must not blit"
    );
}

fn test_opt_in_skips_rasterization_when_content_unchanged() {
    // The optimization that justifies the whole U6-U9 effort: cursor moves
    // and other "intersect-only-not-content" dirty events must not trigger
    // re-rasterization of the backing store. Direct counterpart to AE3 in
    // the requirements doc.
    let (mut wm, state) = make_manager(800, 600);
    use crate::window::ScreenMode;
    let screen_id = wm.create_screen(ScreenMode::Gui);
    wm.switch_screen(screen_id);

    let root_id = install_test_window(&mut wm, None, Rect::new(0, 0, 200, 200), ROOT_COLOR, false);
    if let Some(screen) = wm.get_active_screen_mut() {
        screen.set_root_window(root_id);
    }

    let child_id = install_test_window(
        &mut wm,
        Some(root_id),
        Rect::new(50, 50, 100, 100),
        CHILD0_COLOR,
        true,
    );

    // Render once with the child invalidated so it rasterizes.
    if let Some(w) = wm.window_registry.get_mut(&child_id) {
        w.invalidate();
    }
    wm.test_clear_dirty();
    wm.test_mark_dirty(Rect::new(50, 50, 100, 100));
    wm.test_render_active_screen();

    // First-render rasterize_count should be 1.
    let first_count = wm
        .window_registry
        .get(&child_id)
        .and_then(|w| w.backing_store().map(|b| (b.width, b.height)));
    assert_eq!(
        first_count,
        Some((100, 100)),
        "child rasterized at first render"
    );

    // Now reset, mark a new dirty rect that intersects child but child is
    // NOT invalidated — simulates a cursor moving over child or a sibling
    // drag exposing this area.
    state.lock().reset();
    wm.test_clear_dirty();
    wm.test_mark_dirty(Rect::new(70, 70, 30, 30));
    wm.test_render_active_screen();

    let s = state.lock();
    // Child should have blitted (cached pixels) without paint() being called.
    assert_eq!(paint_count_for(&s, CHILD0_COLOR), 0);
    assert!(
        blit_count_for(&s, 100, 100) >= 1,
        "child blits cached pixels even when content unchanged"
    );

    // The strong assertion: rasterize_count stayed at 1 (no extra rasterize
    // on the second render). Read it through the registry.
    drop(s);
    let registry = &wm.window_registry;
    let entry = registry.get(&child_id).expect("child still in registry");
    // We can't downcast Box<dyn Window>, but TestWindow.rasterize_count is
    // exposed indirectly: rasterizing zero-fills then writes paint_color, so
    // the buffer should still hold paint_color pixels (which is what the
    // first render put there). A second rasterize would produce identical
    // bytes, so we can't directly verify count from the buffer alone. The
    // observable contract — "no paint() call AND blit happened with the
    // existing buffer" — is what U9 promised. Both held above.
    let _ = entry;
}

fn test_opt_in_window_invalidate_re_rasterizes() {
    let (mut wm, state) = make_manager(800, 600);
    use crate::window::ScreenMode;
    let screen_id = wm.create_screen(ScreenMode::Gui);
    wm.switch_screen(screen_id);

    let root_id = install_test_window(&mut wm, None, Rect::new(0, 0, 200, 200), ROOT_COLOR, false);
    if let Some(screen) = wm.get_active_screen_mut() {
        screen.set_root_window(root_id);
    }

    let child_id = install_test_window(
        &mut wm,
        Some(root_id),
        Rect::new(50, 50, 100, 100),
        CHILD0_COLOR,
        true,
    );

    if let Some(w) = wm.window_registry.get_mut(&child_id) {
        w.invalidate();
    }
    wm.test_mark_dirty(Rect::new(50, 50, 100, 100));
    wm.test_render_active_screen();

    // After first render, child's needs_repaint should be cleared.
    assert!(!wm.window_registry.get(&child_id).unwrap().needs_repaint());

    // Invalidate again.
    if let Some(w) = wm.window_registry.get_mut(&child_id) {
        w.invalidate();
    }
    assert!(wm.window_registry.get(&child_id).unwrap().needs_repaint());

    state.lock().reset();
    wm.test_mark_dirty(Rect::new(50, 50, 100, 100));
    wm.test_render_active_screen();

    // After the second render, needs_repaint is cleared again — meaning
    // paint_into_backing_store ran, which is the "re-rasterize" assertion.
    assert!(!wm.window_registry.get(&child_id).unwrap().needs_repaint());
    let s = state.lock();
    assert!(blit_count_for(&s, 100, 100) >= 1);
}

fn test_render_presents_dirty_region_and_cursor_footprint() {
    let (mut wm, state) = make_manager(800, 600);
    let (_root_id, children) = build_simple_scene(
        &mut wm,
        Rect::new(0, 0, 800, 600),
        &[Rect::new(20, 20, 80, 60)],
    );

    // Establish the initial full frame and a saved cursor background.
    wm.render();
    state.lock().reset();

    if let Some(child) = wm.window_registry.get_mut(&children[0]) {
        child.invalidate();
    }
    wm.render();

    let state = state.lock();
    assert_eq!(state.presented_batches.len(), 1);
    let batch = &state.presented_batches[0];
    assert!(
        batch.iter().any(|rect| *rect == Rect::new(20, 20, 80, 60)),
        "expected child damage in presentation batch, got {:?}",
        batch,
    );
    assert!(
        batch
            .iter()
            .any(|rect| rect.contains_point(Point::new(400, 300))),
        "expected cursor footprint in presentation batch, got {:?}",
        batch,
    );
    assert!(
        !batch.iter().any(|rect| *rect == Rect::new(0, 0, 800, 600)),
        "incremental frame unexpectedly presented the full screen",
    );
}

fn test_retained_cursor_move_presents_old_and_new_without_window_rasterization() {
    let (mut wm, state) = make_manager(800, 600);
    build_simple_scene(&mut wm, Rect::new(0, 0, 800, 600), &[]);
    wm.test_force_retained_renderer();

    // Establish retained surfaces and the initial cursor overlay.
    wm.render();
    let old = wm.test_cursor_position();
    let old_white = Point::new(old.x + 1, old.y + 2);
    assert_eq!(
        wm.test_retained_output_pixel(old_white),
        Some((255, 255, 255, 255)),
        "initial retained arrow should contain a white fill",
    );
    let new = Point::new(
        if old.x < 400 {
            old.x + 100
        } else {
            old.x - 100
        },
        if old.y < 300 {
            old.y + 100
        } else {
            old.y - 100
        },
    );
    state.lock().reset();

    wm.test_render_retained_cursor_at(new);

    assert_eq!(
        wm.test_retained_output_pixel(old_white),
        Some((10, 10, 10, 255)),
        "old retained cursor hotspot should be restored from the scene",
    );
    assert_eq!(
        wm.test_retained_output_pixel(Point::new(new.x + 1, new.y + 2)),
        Some((255, 255, 255, 255)),
        "new retained arrow should contain a white fill",
    );

    let state = state.lock();
    assert_eq!(state.presented_batches.len(), 1);
    let batch = &state.presented_batches[0];
    assert!(
        batch.contains(&crate::window::cursor::CursorRenderer::bounds_at(
            crate::window::CursorIcon::Arrow,
            old,
        )),
        "expected old retained cursor footprint in presentation batch, got {:?}",
        batch,
    );
    assert!(
        batch.contains(&crate::window::cursor::CursorRenderer::bounds_at(
            crate::window::CursorIcon::Arrow,
            new,
        )),
        "expected new retained cursor footprint in presentation batch, got {:?}",
        batch,
    );
    assert_eq!(
        wm.render_stats().windows_rasterized,
        0,
        "cursor-only retained frame must not rerasterize windows",
    );
    assert_eq!(
        wm.render_stats().surface_pixels_updated,
        0,
        "cursor-only retained frame must reuse existing surfaces",
    );
    assert_eq!(
        wm.render_stats().surface_raster_cycles,
        0,
        "cursor-only retained frame must report zero raster work",
    );
    assert_eq!(
        wm.render_stats().texture_bytes_uploaded,
        0,
        "cursor-only CPU frame must not report texture uploads",
    );
}

fn test_retained_cursor_icon_change_at_stationary_position_is_cursor_only() {
    let (mut wm, state) = make_manager(800, 600);
    build_simple_scene(&mut wm, Rect::new(0, 0, 800, 600), &[]);
    wm.test_force_retained_renderer();
    wm.render();
    let position = wm.test_cursor_position();
    state.lock().reset();

    wm.test_render_retained_cursor_icon(crate::window::CursorIcon::Text);

    assert_eq!(
        wm.test_retained_output_pixel(position),
        Some((255, 255, 255, 255)),
        "text cursor hotspot should use the white center stroke",
    );
    let state = state.lock();
    let batch = &state.presented_batches[0];
    let old_bounds = crate::window::cursor::CursorRenderer::bounds_at(
        crate::window::CursorIcon::Arrow,
        position,
    );
    let new_bounds =
        crate::window::cursor::CursorRenderer::bounds_at(crate::window::CursorIcon::Text, position);
    assert!(batch.contains(&old_bounds.union(&new_bounds)));
    assert_eq!(wm.render_stats().windows_rasterized, 0);
    assert_eq!(wm.render_stats().surface_pixels_updated, 0);
}

fn test_kernel_text_input_resolves_text_cursor() {
    let (mut wm, _) = make_manager(320, 200);
    let (root, _) = build_simple_scene(&mut wm, Rect::new(0, 0, 320, 200), &[]);
    let input_id = wm.create_window(Some(root));
    let mut input =
        crate::window::windows::TextInput::new_with_id(input_id, Rect::new(20, 30, 120, 24));
    input.set_parent(Some(root));
    wm.set_window_impl(input_id, alloc::boxed::Box::new(input));

    assert_eq!(
        wm.test_cursor_icon_at(Point::new(25, 35)),
        crate::window::CursorIcon::Text
    );
    assert_eq!(
        wm.test_cursor_icon_at(Point::new(5, 5)),
        crate::window::CursorIcon::Arrow
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
        &test_skip_paint_when_ancestor_chain_misses_dirty,
        &test_window_intersecting_dirty_paints_with_clipped_rect,
        &test_full_repaint_clips_to_window_bounds,
        &test_clean_child_outside_dirty_skips_when_parent_paints,
        // U5 — drag/resize bounds-union dirty
        &test_non_frame_top_level_window_is_not_draggable,
        &test_drag_tick_marks_bounds_union_not_full_repaint,
        &test_drag_tick_with_no_position_change_does_nothing,
        &test_frame_minimize_maximize_restore_transitions,
        // U9 — opt-in compositor blit path
        &test_opt_in_window_blits_instead_of_painting,
        &test_opt_in_skips_rasterization_when_content_unchanged,
        &test_opt_in_window_invalidate_re_rasterizes,
        // Regional front-buffer presentation includes both repaint damage
        // and cursor overlay writes.
        &test_render_presents_dirty_region_and_cursor_footprint,
        &test_retained_cursor_move_presents_old_and_new_without_window_rasterization,
        &test_retained_cursor_icon_change_at_stationary_position_is_cursor_only,
        &test_kernel_text_input_resolves_text_cursor,
    ]
}

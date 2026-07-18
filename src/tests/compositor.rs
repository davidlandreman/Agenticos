//! Tests for `DirtyRectManager`.

use crate::graphics::compositor::DirtyRectManager;
use crate::lib::test_utils::Testable;
use crate::window::types::Rect;

const W: u32 = 800;
const H: u32 = 600;

fn collect_dirty(mgr: &DirtyRectManager) -> alloc::vec::Vec<Rect> {
    mgr.dirty_regions().collect()
}

fn test_empty_manager_yields_nothing() {
    let mut mgr = DirtyRectManager::new(W, H);
    mgr.clear(); // first frame is forced full-repaint; clear it for this test
    let rects = collect_dirty(&mgr);
    assert_eq!(rects.len(), 0);
    assert!(!mgr.is_dirty());
    assert!(!mgr.needs_full_repaint());
}

fn test_single_mark_dirty_yields_clamped_rect() {
    let mut mgr = DirtyRectManager::new(W, H);
    mgr.clear();
    mgr.mark_dirty(Rect::new(10, 20, 30, 40));
    let rects = collect_dirty(&mgr);
    assert_eq!(rects.len(), 1);
    assert_eq!(rects[0], Rect::new(10, 20, 30, 40));
}

fn test_two_non_overlapping_marks_yield_both() {
    let mut mgr = DirtyRectManager::new(W, H);
    mgr.clear();
    mgr.mark_dirty(Rect::new(0, 0, 50, 50));
    mgr.mark_dirty(Rect::new(200, 200, 50, 50));
    let rects = collect_dirty(&mgr);
    assert_eq!(rects.len(), 2);
}

fn test_full_repaint_yields_single_screen_rect() {
    let mut mgr = DirtyRectManager::new(W, H);
    mgr.mark_full_repaint();
    let rects = collect_dirty(&mgr);
    assert_eq!(rects.len(), 1);
    assert_eq!(rects[0], Rect::new(0, 0, W, H));
    assert!(mgr.needs_full_repaint());
}

fn test_mark_dirty_then_full_repaint_collapses_to_screen() {
    let mut mgr = DirtyRectManager::new(W, H);
    mgr.clear();
    mgr.mark_dirty(Rect::new(10, 10, 20, 20));
    mgr.mark_full_repaint();
    let rects = collect_dirty(&mgr);
    assert_eq!(rects.len(), 1);
    assert_eq!(rects[0], Rect::new(0, 0, W, H));
}

fn test_clear_resets_state() {
    let mut mgr = DirtyRectManager::new(W, H);
    mgr.mark_full_repaint();
    mgr.clear();
    assert!(!mgr.needs_full_repaint());
    assert!(!mgr.is_dirty());
    let rects = collect_dirty(&mgr);
    assert_eq!(rects.len(), 0);
}

fn test_bounding_box_agrees_with_iterator_full_repaint() {
    let mut mgr = DirtyRectManager::new(W, H);
    mgr.mark_full_repaint();
    let bbox = mgr.bounding_box().expect("full-repaint should have a bbox");
    let rects = collect_dirty(&mgr);
    assert_eq!(rects.len(), 1);
    assert_eq!(rects[0], bbox);
}

// A single rect — even one covering most of the screen — must NOT trip the
// full-repaint area threshold. Window drags rely on this: the union of a
// window's old and new bounds easily exceeds 50% of a small framebuffer, but
// painting that single rect is still cheaper than a full-screen clear.
fn test_single_large_rect_does_not_trip_full_repaint() {
    let mut mgr = DirtyRectManager::new(W, H);
    mgr.clear();
    // ~60% of the screen as a single rect.
    let big = Rect::new(0, 0, W, (H as f32 * 0.6) as u32);
    mgr.mark_dirty(big);
    assert!(!mgr.needs_full_repaint());
    let rects = collect_dirty(&mgr);
    assert_eq!(rects.len(), 1);
    assert_eq!(rects[0], big);
}

// A single rect covering nearly the whole screen still avoids the trip,
// because the partial path is always at least as cheap as full-repaint when
// there's only one rect.
fn test_single_near_full_screen_rect_does_not_trip_full_repaint() {
    let mut mgr = DirtyRectManager::new(W, H);
    mgr.clear();
    let almost = Rect::new(0, 0, W, H - 1);
    mgr.mark_dirty(almost);
    assert!(!mgr.needs_full_repaint());
}

// Many separate rects whose total area exceeds the multi-rect threshold
// (0.85) must still trigger full-repaint — that's the case the threshold
// exists to catch.
fn test_many_rects_above_threshold_trip_full_repaint() {
    let mut mgr = DirtyRectManager::new(W, H);
    mgr.clear();
    // Stripe the screen with non-overlapping rects whose union > 85% area.
    // 12 horizontal stripes of W x 50 = W*600 = full screen.
    // Use 11 stripes (W*550 = ~91.7% of W*H = 480k/480k) and skip one row.
    // Actually 11 * 50 = 550 rows => 550/600 = 91.6% of height.
    let stripe_h = 50u32;
    for i in 0..11 {
        mgr.mark_dirty(Rect::new(0, (i * stripe_h) as i32, W, stripe_h));
    }
    assert!(
        mgr.needs_full_repaint(),
        "11 non-overlapping stripes (~91% area) should trip threshold"
    );
}

fn test_bounding_box_agrees_with_iterator_partial() {
    let mut mgr = DirtyRectManager::new(W, H);
    mgr.clear();
    mgr.mark_dirty(Rect::new(10, 20, 30, 40));
    mgr.mark_dirty(Rect::new(200, 100, 50, 50));
    let bbox = mgr
        .bounding_box()
        .expect("partial dirty should have a bbox");
    // bbox encloses both rects: x in [10, 250), y in [20, 150)
    assert_eq!(bbox, Rect::new(10, 20, 240, 130));
    let rects = collect_dirty(&mgr);
    assert!(!rects.is_empty());
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_empty_manager_yields_nothing,
        &test_single_mark_dirty_yields_clamped_rect,
        &test_two_non_overlapping_marks_yield_both,
        &test_full_repaint_yields_single_screen_rect,
        &test_mark_dirty_then_full_repaint_collapses_to_screen,
        &test_clear_resets_state,
        &test_bounding_box_agrees_with_iterator_full_repaint,
        &test_single_large_rect_does_not_trip_full_repaint,
        &test_single_near_full_screen_rect_does_not_trip_full_repaint,
        &test_many_rects_above_threshold_trip_full_repaint,
        &test_bounding_box_agrees_with_iterator_partial,
    ]
}

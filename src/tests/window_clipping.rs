//! Tests for signed-coordinate clipping in graphics adapters and the
//! drag-time title-bar visibility clamp.
//!
//! The clip helper and drag clamp are pure functions, so they can be
//! exercised here without constructing a real `DirectFrameBufferDevice`
//! against a bootloader framebuffer.

use crate::lib::test_utils::Testable;
use crate::window::Rect;
use crate::window::adapters::clip::{clip_line, clip_rect, pixel_visible};
use crate::window::types::{clamp_drag_x, clamp_drag_y, MIN_TITLEBAR_VISIBLE};

// -- clip_rect ------------------------------------------------------------

fn test_clip_rect_fully_inside() {
    let r = clip_rect(10, 20, 30, 40, 200, 200, None);
    assert_eq!(r, Some((10, 20, 30, 40)));
}

fn test_clip_rect_negative_origin_clips_top_left() {
    // (-50, -50) with 100x100 on a 200x200 device -> bottom-right 50x50.
    let r = clip_rect(-50, -50, 100, 100, 200, 200, None);
    assert_eq!(r, Some((0, 0, 50, 50)));
}

fn test_clip_rect_beyond_bottom_right_clips() {
    // (150, 150) with 100x100 on a 200x200 device -> top-left 50x50.
    let r = clip_rect(150, 150, 100, 100, 200, 200, None);
    assert_eq!(r, Some((150, 150, 50, 50)));
}

fn test_clip_rect_fully_off_screen_returns_none() {
    let r = clip_rect(-1000, -1000, 100, 100, 200, 200, None);
    assert!(r.is_none());

    let r = clip_rect(500, 500, 100, 100, 200, 200, None);
    assert!(r.is_none());
}

fn test_clip_rect_extreme_inputs_do_not_overflow() {
    // u32::MAX width is the canonical overflow probe; the helper widens
    // to i64 internally so this must clamp to the device, not panic.
    let r = clip_rect(0, 0, u32::MAX, u32::MAX, 200, 200, None);
    assert_eq!(r, Some((0, 0, 200, 200)));

    let r = clip_rect(i32::MIN, i32::MIN, 100, 100, 200, 200, None);
    assert!(r.is_none());

    let r = clip_rect(i32::MAX - 50, 0, 100, 100, 200, 200, None);
    assert!(r.is_none());
}

fn test_clip_rect_zero_size_returns_none() {
    assert!(clip_rect(10, 10, 0, 50, 200, 200, None).is_none());
    assert!(clip_rect(10, 10, 50, 0, 200, 200, None).is_none());
}

fn test_clip_rect_intersects_with_active_clip() {
    let clip = Rect::new(10, 10, 50, 50);
    // 200x200 fill_rect against a 50x50 clip -> 50x50 region.
    let r = clip_rect(0, 0, 200, 200, 200, 200, Some(&clip));
    assert_eq!(r, Some((10, 10, 50, 50)));
}

fn test_clip_rect_with_negative_origin_clip_excludes_negative_region() {
    let clip = Rect::new(-20, -20, 50, 50);
    // The clip region's visible portion is [0, 30) x [0, 30).
    let r = clip_rect(0, 0, 200, 200, 200, 200, Some(&clip));
    assert_eq!(r, Some((0, 0, 30, 30)));
}

// -- pixel_visible --------------------------------------------------------

fn test_pixel_visible_inside_returns_some() {
    assert_eq!(pixel_visible(0, 0, 200, 200, None), Some((0, 0)));
    assert_eq!(pixel_visible(199, 199, 200, 200, None), Some((199, 199)));
}

fn test_pixel_visible_negative_returns_none() {
    assert!(pixel_visible(-1, 0, 200, 200, None).is_none());
    assert!(pixel_visible(0, -1, 200, 200, None).is_none());
}

fn test_pixel_visible_beyond_device_returns_none() {
    assert!(pixel_visible(200, 0, 200, 200, None).is_none());
    assert!(pixel_visible(0, 200, 200, 200, None).is_none());
}

fn test_pixel_visible_clip_rect_excludes_outside() {
    let clip = Rect::new(10, 10, 50, 50);
    assert!(pixel_visible(5, 5, 200, 200, Some(&clip)).is_none());
    assert_eq!(pixel_visible(15, 15, 200, 200, Some(&clip)), Some((15, 15)));
    assert!(pixel_visible(60, 15, 200, 200, Some(&clip)).is_none());
}

// -- clip_line ------------------------------------------------------------

fn test_clip_line_fully_inside() {
    let r = clip_line(10, 10, 50, 50, 200, 200, None);
    assert_eq!(r, Some(((10, 10), (50, 50))));
}

fn test_clip_line_fully_outside_returns_none() {
    let r = clip_line(-100, 50, -50, 60, 200, 200, None);
    assert!(r.is_none());
}

fn test_clip_line_extreme_endpoints_clipped() {
    // A horizontal line spanning all of i32 should clip to the device,
    // not iterate billions of pixels later in Bresenham.
    let r = clip_line(i32::MIN, 50, i32::MAX, 50, 200, 200, None);
    assert!(r.is_some());
    let ((x1, _), (x2, _)) = r.unwrap();
    assert!(x1 >= 0 && x1 < 200);
    assert!(x2 >= 0 && x2 < 200);
}

fn test_clip_line_diagonal_clipped_to_device() {
    // A diagonal from (-50, -50) to (250, 250) on a 200x200 device — both
    // endpoints land somewhere inside the device after clipping.
    let r = clip_line(-50, -50, 250, 250, 200, 200, None);
    assert!(r.is_some());
    let ((x1, y1), (x2, y2)) = r.unwrap();
    assert!(x1 >= 0 && x1 < 200);
    assert!(y1 >= 0 && y1 < 200);
    assert!(x2 >= 0 && x2 < 200);
    assert!(y2 >= 0 && y2 < 200);
}

// -- drag clamp -----------------------------------------------------------

fn test_drag_clamp_x_within_lenient_zone_unchanged() {
    // A 400-wide window starting at x=100 dragged by -50 lands at 50;
    // the clamp's left bound is 80 - 400 = -320, so 50 is unchanged.
    assert_eq!(clamp_drag_x(50, 400, 800), 50);
}

fn test_drag_clamp_x_pushes_back_at_left_edge() {
    // Far-left drag clamps so MIN_TITLEBAR_VISIBLE pixels of title bar
    // remain at the left side of the screen.
    let result = clamp_drag_x(-1000, 400, 800);
    assert_eq!(result, MIN_TITLEBAR_VISIBLE - 400);
    // Title bar's right edge sits at MIN_TITLEBAR_VISIBLE.
    assert_eq!(result + 400, MIN_TITLEBAR_VISIBLE);
}

fn test_drag_clamp_x_pushes_back_at_right_edge() {
    // Far-right drag clamps to screen_width - MIN_TITLEBAR_VISIBLE.
    let result = clamp_drag_x(10_000, 400, 800);
    assert_eq!(result, 800 - MIN_TITLEBAR_VISIBLE);
}

fn test_drag_clamp_x_narrow_window() {
    // For a window narrower than MIN_TITLEBAR_VISIBLE, the clamp keeps
    // the entire window on screen rather than the constant strip.
    let narrow = 50;
    assert_eq!(clamp_drag_x(-1000, narrow, 800), 0);
    assert_eq!(clamp_drag_x(10_000, narrow, 800), 800 - narrow);
}

fn test_drag_clamp_y_at_top_clamps_to_zero() {
    assert_eq!(clamp_drag_y(-1000, 600), 0);
}

fn test_drag_clamp_y_at_bottom_keeps_titlebar_visible() {
    let result = clamp_drag_y(10_000, 600);
    assert_eq!(result, 600 - crate::window::theme::metrics().title_bar_height as i32);
}

fn test_drag_clamp_y_within_screen_unchanged() {
    assert_eq!(clamp_drag_y(50, 600), 50);
    assert_eq!(clamp_drag_y(500, 600), 500);
}

// -- registration ---------------------------------------------------------

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_clip_rect_fully_inside,
        &test_clip_rect_negative_origin_clips_top_left,
        &test_clip_rect_beyond_bottom_right_clips,
        &test_clip_rect_fully_off_screen_returns_none,
        &test_clip_rect_extreme_inputs_do_not_overflow,
        &test_clip_rect_zero_size_returns_none,
        &test_clip_rect_intersects_with_active_clip,
        &test_clip_rect_with_negative_origin_clip_excludes_negative_region,
        &test_pixel_visible_inside_returns_some,
        &test_pixel_visible_negative_returns_none,
        &test_pixel_visible_beyond_device_returns_none,
        &test_pixel_visible_clip_rect_excludes_outside,
        &test_clip_line_fully_inside,
        &test_clip_line_fully_outside_returns_none,
        &test_clip_line_extreme_endpoints_clipped,
        &test_clip_line_diagonal_clipped_to_device,
        &test_drag_clamp_x_within_lenient_zone_unchanged,
        &test_drag_clamp_x_pushes_back_at_left_edge,
        &test_drag_clamp_x_pushes_back_at_right_edge,
        &test_drag_clamp_x_narrow_window,
        &test_drag_clamp_y_at_top_clamps_to_zero,
        &test_drag_clamp_y_at_bottom_keeps_titlebar_visible,
        &test_drag_clamp_y_within_screen_unchanged,
    ]
}

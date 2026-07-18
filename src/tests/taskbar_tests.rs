use crate::graphics::surface::{Surface, SurfaceDesc};
use crate::lib::test_utils::Testable;
use crate::time::DateTime;
use crate::window::renderer::SurfaceCanvas;
use crate::window::theme::{self, ThemeKind};
use crate::window::windows::taskbar::{
    format_clock, tray_bounds, window_button_bounds, TaskbarTrayWindow, BUTTON_GAP, BUTTON_HEIGHT,
    BUTTON_Y_OFFSET, MAX_WINDOW_BUTTON_WIDTH, START_BUTTON_WIDTH, TRAY_WIDTH,
};
use crate::window::{Rect, Window, WindowId};

fn start_bounds() -> Rect {
    Rect::new(
        BUTTON_GAP as i32,
        BUTTON_Y_OFFSET as i32,
        START_BUTTON_WIDTH,
        BUTTON_HEIGHT,
    )
}

fn test_tray_is_right_anchored() {
    for width in [640, 1024, 1280] {
        let tray = tray_bounds(width);
        assert_eq!(tray.width, TRAY_WIDTH);
        assert_eq!(tray.right(), width as i32 - 2);
    }
}

fn test_start_buttons_and_tray_do_not_overlap() {
    let tray = tray_bounds(1280);
    let start = start_bounds();
    assert!(!start.intersects(&tray));
    for index in 0..4 {
        let button = window_button_bounds(1280, 4, index);
        assert!(!button.intersects(&start));
        assert!(!button.intersects(&tray));
        assert!(button.width <= MAX_WINDOW_BUTTON_WIDTH);
        if index > 0 {
            let previous = window_button_bounds(1280, 4, index - 1);
            assert!(previous.right() <= button.x);
        }
    }
}

fn test_task_buttons_share_only_middle_span() {
    let one = window_button_bounds(1280, 1, 0);
    assert_eq!(one.width, MAX_WINDOW_BUTTON_WIDTH);

    let tray = tray_bounds(640);
    for index in 0..12 {
        let button = window_button_bounds(640, 12, index);
        assert!(button.x >= start_bounds().right());
        assert!(button.right() <= tray.x - BUTTON_GAP as i32);
    }
}

fn test_narrow_geometry_saturates() {
    for width in [0, 32, 68, 80, 120] {
        let tray = tray_bounds(width);
        assert!(tray.x >= 0);
        assert!(tray.right() <= width as i32);
        for index in 0..32 {
            let button = window_button_bounds(width, 32, index);
            assert!(button.width == 0 || button.right() <= tray.x - BUTTON_GAP as i32);
        }
    }
}

fn test_clock_format_is_fixed_width() {
    let (time, date) = format_clock(DateTime {
        year: 2026,
        month: 7,
        day: 8,
        hour: 3,
        minute: 4,
        second: 59,
    });
    assert_eq!(time, "03:04 UTC");
    assert_eq!(date, "2026-07-08");
}

fn test_tray_invalidates_only_when_displayed_minute_changes() {
    let mut tray = TaskbarTrayWindow::new_with_id(WindowId(9001), Rect::new(0, 0, 96, 28));
    tray.clear_needs_repaint();
    let first = DateTime {
        year: 2026,
        month: 7,
        day: 18,
        hour: 23,
        minute: 59,
        second: 1,
    };
    tray.refresh_clock(Some(60_000_000_000), Some(first));
    assert!(tray.needs_repaint());
    assert_eq!(tray.clock_text(), ("23:59 UTC", "2026-07-18"));

    tray.clear_needs_repaint();
    tray.refresh_clock(Some(119_000_000_000), Some(first));
    assert!(!tray.needs_repaint());

    tray.refresh_clock(
        Some(120_000_000_000),
        Some(DateTime {
            year: 2026,
            month: 7,
            day: 19,
            hour: 0,
            minute: 0,
            second: 0,
        }),
    );
    assert!(tray.needs_repaint());
    assert_eq!(tray.clock_text(), ("00:00 UTC", "2026-07-19"));
}

fn test_unknown_clock_uses_placeholders() {
    let mut tray = TaskbarTrayWindow::new_with_id(WindowId(9002), Rect::new(0, 0, 96, 28));
    tray.refresh_clock(
        Some(60_000_000_000),
        Some(DateTime {
            year: 2026,
            month: 7,
            day: 18,
            hour: 12,
            minute: 0,
            second: 0,
        }),
    );
    tray.refresh_clock(None, None);
    assert_eq!(tray.clock_text(), ("--:-- UTC", "----------"));
}

fn test_tray_paints_recessed_bevel() {
    let previous_theme = theme::active();
    theme::activate(ThemeKind::Classic);
    let mut tray = TaskbarTrayWindow::new_with_id(WindowId(9003), Rect::new(0, 0, 96, 28));
    let mut surface = Surface::new(SurfaceDesc::new(96, 28)).unwrap();
    {
        let mut canvas = SurfaceCanvas::new(&mut surface, (0, 0), (96, 28));
        tray.paint(&mut canvas);
    }
    theme::activate(previous_theme);

    assert_eq!(surface.pixel(0, 0).unwrap().to_rgba(), (128, 128, 128, 255));
    assert_eq!(
        surface.pixel(95, 27).unwrap().to_rgba(),
        (255, 255, 255, 255)
    );
    assert_eq!(
        surface.pixel(2, 13).unwrap().to_rgba(),
        (192, 192, 192, 255)
    );
}

fn test_aero_tray_uses_control_palette() {
    let previous_theme = theme::active();
    theme::activate(ThemeKind::Aero);
    let mut tray = TaskbarTrayWindow::new_with_id(WindowId(9004), Rect::new(0, 0, 96, 28));
    let mut surface = Surface::new(SurfaceDesc::new(96, 28)).unwrap();
    {
        let mut canvas = SurfaceCanvas::new(&mut surface, (0, 0), (96, 28));
        tray.paint(&mut canvas);
    }
    theme::activate(previous_theme);

    assert_eq!(surface.pixel(0, 0).unwrap().to_rgba(), (112, 112, 112, 255));
    assert_eq!(
        surface.pixel(95, 27).unwrap().to_rgba(),
        (112, 112, 112, 255)
    );
    assert_eq!(
        surface.pixel(2, 13).unwrap().to_rgba(),
        (240, 240, 240, 255)
    );
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_tray_is_right_anchored,
        &test_start_buttons_and_tray_do_not_overlap,
        &test_task_buttons_share_only_middle_span,
        &test_narrow_geometry_saturates,
        &test_clock_format_is_fixed_width,
        &test_tray_invalidates_only_when_displayed_minute_changes,
        &test_unknown_clock_uses_placeholders,
        &test_tray_paints_recessed_bevel,
        &test_aero_tray_uses_control_palette,
    ]
}

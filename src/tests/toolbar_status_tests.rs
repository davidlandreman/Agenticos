//! Tests for U11: `Button::set_enabled`, `Toolbar`, and `StatusBar`.
//!
//! Most behavior here is exercised through the live `WindowManager`
//! (already initialized by kernel boot before the test runner fires)
//! because `Toolbar` and `StatusBar` register their child buttons /
//! labels into the manager so layout can write back through it.

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::lib::arc::Arc;

use crate::lib::test_utils::Testable;
use crate::window::event::{
    Event, KeyModifiers, MouseButtons, MouseEvent, MouseEventType,
};
use crate::window::types::Point;
use crate::window::windows::button::Button;
use crate::window::windows::layout::{SizeHint, Spacer, VBox};
use crate::window::windows::status_bar::StatusBar;
use crate::window::windows::toolbar::Toolbar;
use crate::window::{with_window_manager, Rect, Window, WindowId};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn click_at(local: Point) -> Event {
    Event::Mouse(MouseEvent {
        event_type: MouseEventType::ButtonDown,
        position: local,
        global_position: local,
        buttons: MouseButtons {
            left: true,
            ..MouseButtons::default()
        },
        modifiers: KeyModifiers::default(),
    })
}

fn release_at(local: Point) -> Event {
    Event::Mouse(MouseEvent {
        event_type: MouseEventType::ButtonUp,
        position: local,
        global_position: local,
        buttons: MouseButtons::default(),
        modifiers: KeyModifiers::default(),
    })
}

/// Run a closure that mutates a registered Button by id; afterwards the
/// button is put back in the registry so the rest of the test can
/// continue using it.
fn with_button<F, R>(id: WindowId, f: F) -> R
where
    F: FnOnce(&mut Button) -> R,
{
    let mut out: Option<R> = None;
    with_window_manager(|wm| {
        wm.with_window_mut(id, |w| {
            let btn = w.as_button_mut().expect("expected window to be a Button");
            out = Some(f(btn));
        });
    });
    out.expect("with_button: id not found in registry")
}

// ---------------------------------------------------------------------------
// Button::set_enabled
// ---------------------------------------------------------------------------

fn test_button_default_enabled_true() {
    let button = Button::new(Rect::new(0, 0, 60, 24), "OK");
    assert!(button.enabled());
}

fn test_button_set_enabled_false_then_true_round_trip() {
    let mut button = Button::new(Rect::new(0, 0, 60, 24), "OK");
    button.set_enabled(false);
    assert!(!button.enabled());
    button.set_enabled(true);
    assert!(button.enabled());
}

fn test_disabled_button_ignores_button_down_and_up() {
    let counter = Arc::new(AtomicUsize::new(0));
    let mut button = Button::new(Rect::new(0, 0, 60, 24), "OK");
    {
        let counter = Arc::clone(&counter);
        button.on_click(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
    }
    button.set_enabled(false);

    // Press + release entirely inside bounds — would normally fire the
    // callback exactly once on a normal button.
    let down = button.handle_event(click_at(Point::new(10, 10)));
    let up = button.handle_event(release_at(Point::new(10, 10)));
    assert_eq!(counter.load(Ordering::SeqCst), 0, "callback fired while disabled");
    // Both press and release report Ignored when disabled.
    assert!(matches!(
        down,
        crate::window::EventResult::Ignored
    ));
    assert!(matches!(up, crate::window::EventResult::Ignored));
}

fn test_re_enabling_button_restores_click_handling() {
    let counter = Arc::new(AtomicUsize::new(0));
    let mut button = Button::new(Rect::new(0, 0, 60, 24), "OK");
    {
        let counter = Arc::clone(&counter);
        button.on_click(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
    }
    button.set_enabled(false);
    // Disabled — should not fire.
    let _ = button.handle_event(click_at(Point::new(10, 10)));
    let _ = button.handle_event(release_at(Point::new(10, 10)));
    assert_eq!(counter.load(Ordering::SeqCst), 0);

    // Re-enable and click again.
    button.set_enabled(true);
    let _ = button.handle_event(click_at(Point::new(10, 10)));
    let _ = button.handle_event(release_at(Point::new(10, 10)));
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

// ---------------------------------------------------------------------------
// Toolbar
// ---------------------------------------------------------------------------

fn test_toolbar_three_buttons_left_aligned() {
    let toolbar_id = WindowId::new();
    let mut tb = Toolbar::new_with_id(toolbar_id, Rect::new(0, 0, 400, 32));
    let id_a = tb.add_button("Back", || {});
    let id_b = tb.add_button("Forward", || {});
    let id_c = tb.add_button("Up", || {});

    // Each button should land at the X immediately after the previous
    // one — verify left-to-right ordering with monotonically increasing
    // x positions and zero gap between them.
    let bounds_a = with_window_manager(|wm| wm.window_registry.get(&id_a).unwrap().bounds()).unwrap();
    let bounds_b = with_window_manager(|wm| wm.window_registry.get(&id_b).unwrap().bounds()).unwrap();
    let bounds_c = with_window_manager(|wm| wm.window_registry.get(&id_c).unwrap().bounds()).unwrap();

    assert_eq!(bounds_a.x, 0);
    assert_eq!(bounds_b.x, bounds_a.x + bounds_a.width as i32);
    assert_eq!(bounds_c.x, bounds_b.x + bounds_b.width as i32);
    // All three buttons are the same height (vertically centered inside
    // the 32-px strip).
    assert_eq!(bounds_a.height, bounds_b.height);
    assert_eq!(bounds_b.height, bounds_c.height);

    // Clean up.
    with_window_manager(|wm| {
        wm.set_window_impl(toolbar_id, Box::new(tb));
        wm.destroy_window(toolbar_id);
    });
}

fn test_toolbar_button_click_fires_callback() {
    let counter = Arc::new(AtomicUsize::new(0));
    let toolbar_id = WindowId::new();
    let mut tb = Toolbar::new_with_id(toolbar_id, Rect::new(0, 0, 400, 32));
    let btn_id = {
        let counter = Arc::clone(&counter);
        tb.add_button("Click", move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })
    };

    // Drive a press + release through the registered Button directly.
    with_button(btn_id, |b| {
        let bounds = b.bounds();
        let _ = b.handle_event(click_at(Point::new(
            (bounds.width as i32) / 2,
            (bounds.height as i32) / 2,
        )));
        let _ = b.handle_event(release_at(Point::new(
            (bounds.width as i32) / 2,
            (bounds.height as i32) / 2,
        )));
    });
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    with_window_manager(|wm| {
        wm.set_window_impl(toolbar_id, Box::new(tb));
        wm.destroy_window(toolbar_id);
    });
}

fn test_toolbar_set_enabled_disables_button() {
    let counter = Arc::new(AtomicUsize::new(0));
    let toolbar_id = WindowId::new();
    let mut tb = Toolbar::new_with_id(toolbar_id, Rect::new(0, 0, 400, 32));
    let btn_id = {
        let counter = Arc::clone(&counter);
        tb.add_button("Click", move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })
    };

    tb.set_enabled(btn_id, false);
    assert!(!with_button(btn_id, |b| b.enabled()));

    // Click should NOT fire the callback while disabled.
    with_button(btn_id, |b| {
        let _ = b.handle_event(click_at(Point::new(5, 5)));
        let _ = b.handle_event(release_at(Point::new(5, 5)));
    });
    assert_eq!(counter.load(Ordering::SeqCst), 0);

    // Re-enable and click — callback fires once.
    tb.set_enabled(btn_id, true);
    assert!(with_button(btn_id, |b| b.enabled()));
    with_button(btn_id, |b| {
        let _ = b.handle_event(click_at(Point::new(5, 5)));
        let _ = b.handle_event(release_at(Point::new(5, 5)));
    });
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    with_window_manager(|wm| {
        wm.set_window_impl(toolbar_id, Box::new(tb));
        wm.destroy_window(toolbar_id);
    });
}

fn test_empty_toolbar_does_not_panic() {
    // No buttons or separators — paint should be a background-only
    // operation. We can't easily exercise paint without a real graphics
    // device in tests, so we just confirm construction and bounds work.
    let tb = Toolbar::new(Rect::new(0, 0, 100, 32));
    assert_eq!(tb.bounds(), Rect::new(0, 0, 100, 32));
    assert!(tb.children().is_empty());
}

// ---------------------------------------------------------------------------
// StatusBar
// ---------------------------------------------------------------------------

fn test_status_bar_single_section_fills_full_width() {
    let sb_id = WindowId::new();
    let mut sb = StatusBar::new_with_id(sb_id, Rect::new(0, 0, 600, 20));
    let sec = sb.add_section("Ready", 1);

    let bounds = with_window_manager(|wm| wm.window_registry.get(&sec).unwrap().bounds()).unwrap();
    assert_eq!(bounds.x, 0);
    assert_eq!(bounds.width, 600);

    with_window_manager(|wm| {
        wm.set_window_impl(sb_id, Box::new(sb));
        wm.destroy_window(sb_id);
    });
}

fn test_status_bar_two_sections_equal_weight_split_50_50() {
    let sb_id = WindowId::new();
    let mut sb = StatusBar::new_with_id(sb_id, Rect::new(0, 0, 600, 20));
    let a = sb.add_section("A", 1);
    let b = sb.add_section("B", 1);

    let bounds_a = with_window_manager(|wm| wm.window_registry.get(&a).unwrap().bounds()).unwrap();
    let bounds_b = with_window_manager(|wm| wm.window_registry.get(&b).unwrap().bounds()).unwrap();
    assert_eq!(bounds_a.x, 0);
    assert_eq!(bounds_a.width, 300);
    assert_eq!(bounds_b.x, 300);
    assert_eq!(bounds_b.width, 300);

    with_window_manager(|wm| {
        wm.set_window_impl(sb_id, Box::new(sb));
        wm.destroy_window(sb_id);
    });
}

fn test_status_bar_set_section_text_updates_label() {
    let sb_id = WindowId::new();
    let mut sb = StatusBar::new_with_id(sb_id, Rect::new(0, 0, 600, 20));
    let sec = sb.add_section("Initial", 1);

    sb.set_section_text(sec, "Updated");

    // Verify the label's text was actually updated through the manager.
    let text: Option<String> = with_window_manager(|wm| {
        let mut out: Option<String> = None;
        wm.with_window_mut(sec, |w| {
            if let Some(label) = w.as_label_mut() {
                out = Some(String::from(label.text()));
            }
        });
        out
    })
    .flatten();
    assert_eq!(text.as_deref(), Some("Updated"));

    with_window_manager(|wm| {
        wm.set_window_impl(sb_id, Box::new(sb));
        wm.destroy_window(sb_id);
    });
}

fn test_status_bar_text_wider_than_section_does_not_overflow_into_neighbour() {
    // Two equal-weight sections in a 600-wide bar. The first section
    // gets a string longer than its 300-px width; we verify its bounds
    // remain pinned to its slot and the second section's slot is
    // unaffected. Label paints clip to its own bounds; this test
    // pins down the layout invariant rather than the paint clip.
    let sb_id = WindowId::new();
    let mut sb = StatusBar::new_with_id(sb_id, Rect::new(0, 0, 600, 20));
    let wide_text = "this is a very long status message that exceeds three hundred pixels";
    let a = sb.add_section(wide_text, 1);
    let b = sb.add_section("right", 1);

    let bounds_a = with_window_manager(|wm| wm.window_registry.get(&a).unwrap().bounds()).unwrap();
    let bounds_b = with_window_manager(|wm| wm.window_registry.get(&b).unwrap().bounds()).unwrap();
    assert_eq!(bounds_a.width, 300, "wide text should not stretch the slot");
    assert_eq!(bounds_b.x, 300, "neighbour slot should start exactly at 300");
    assert_eq!(bounds_b.width, 300);

    with_window_manager(|wm| {
        wm.set_window_impl(sb_id, Box::new(sb));
        wm.destroy_window(sb_id);
    });
}

// ---------------------------------------------------------------------------
// Integration: VBox containing Toolbar (fixed 32) + Spacer (fill 1) +
// StatusBar (fixed 20) lays out correctly in a 600-tall container.
// ---------------------------------------------------------------------------

fn test_vbox_toolbar_spacer_status_bar_layout() {
    // Construct the Toolbar and StatusBar OUTSIDE the window-manager
    // lock so their `add_button` / `add_section` calls (which acquire
    // the lock internally to register child windows) do not recurse on
    // the same lock.
    let toolbar_id = WindowId::new();
    let mut tb = Toolbar::new_with_id(toolbar_id, Rect::new(0, 0, 800, 32));
    let _btn = tb.add_button("Back", || {});

    let status_id = WindowId::new();
    let mut sb = StatusBar::new_with_id(status_id, Rect::new(0, 0, 800, 20));
    let _sec = sb.add_section("Ready", 1);

    with_window_manager(|wm| {
        wm.set_window_impl(toolbar_id, Box::new(tb));
        wm.set_window_impl(status_id, Box::new(sb));

        let spacer_id = wm.create_window(None);
        wm.set_window_impl(
            spacer_id,
            Box::new(Spacer::new_with_id(spacer_id, Rect::new(0, 0, 0, 0))),
        );

        // Build the outer VBox and parent the three widgets under it.
        let vbox_id = wm.create_window(None);
        let mut vbox = VBox::new_with_id(vbox_id, Rect::new(0, 0, 800, 600));
        vbox.add_child(toolbar_id, SizeHint::Fixed(32));
        vbox.add_child(spacer_id, SizeHint::Fill(1));
        vbox.add_child(status_id, SizeHint::Fixed(20));
        wm.set_window_impl(vbox_id, Box::new(vbox));

        // Trigger a real resize through the active-manager flow so the
        // VBox lays out the children for real.
        wm.with_window_mut(vbox_id, |w| {
            w.set_bounds(Rect::new(0, 0, 800, 600));
        });

        let toolbar_bounds = wm.window_registry.get(&toolbar_id).unwrap().bounds();
        let spacer_bounds = wm.window_registry.get(&spacer_id).unwrap().bounds();
        let status_bounds = wm.window_registry.get(&status_id).unwrap().bounds();

        // Toolbar pinned to top, 32 px tall.
        assert_eq!(toolbar_bounds, Rect::new(0, 0, 800, 32));
        // Spacer fills the middle: 600 - 32 - 20 = 548.
        assert_eq!(spacer_bounds, Rect::new(0, 32, 800, 548));
        // StatusBar pinned to bottom, 20 px tall.
        assert_eq!(status_bounds, Rect::new(0, 580, 800, 20));

        wm.destroy_window(vbox_id);
    });
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_button_default_enabled_true,
        &test_button_set_enabled_false_then_true_round_trip,
        &test_disabled_button_ignores_button_down_and_up,
        &test_re_enabling_button_restores_click_handling,
        &test_toolbar_three_buttons_left_aligned,
        &test_toolbar_button_click_fires_callback,
        &test_toolbar_set_enabled_disables_button,
        &test_empty_toolbar_does_not_panic,
        &test_status_bar_single_section_fills_full_width,
        &test_status_bar_two_sections_equal_weight_split_50_50,
        &test_status_bar_set_section_text_updates_label,
        &test_status_bar_text_wider_than_section_does_not_overflow_into_neighbour,
        &test_vbox_toolbar_spacer_status_bar_layout,
    ]
}

//! Message box dialogs for displaying information to the user

use alloc::boxed::Box;

use crate::graphics::color::Color;
use crate::window::windows::dialog::{
    clear_dialog_state, close_dialog_with_result, is_dialog_open, set_dialog_state, DialogResult,
};
use crate::window::windows::{
    Button, ContainerWindow, FrameWindow, HBox, Label, Padding, SizeHint, Spacer, VBox,
};
use crate::window::{with_window_manager, Rect, Window, WindowId};

/// Type of message box
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageBoxType {
    /// Informational message
    Info,
    /// Warning message
    Warning,
    /// Error message
    Error,
}

/// Show a message dialog and wait for user to dismiss it
pub fn show_message(title: &str, message: &str, msg_type: MessageBoxType) {
    // Calculate dialog size based on message
    let dialog_width = 350u32;
    let dialog_height = 150u32;

    // Get screen size and center dialog
    let (screen_width, screen_height) = with_window_manager(|wm| {
        (wm.graphics_device.width() as i32, wm.graphics_device.height() as i32)
    })
    .unwrap_or((800, 600));

    let dialog_x = (screen_width - dialog_width as i32) / 2;
    let dialog_y = (screen_height - dialog_height as i32) / 2;

    // Create dialog structure
    let (frame_id, _ok_button_id) = with_window_manager(|wm| {
        // Get desktop for parenting
        let desktop_id = wm
            .get_active_screen()
            .and_then(|s| s.root_window)
            .unwrap_or(WindowId::new());

        // Create frame window
        let frame_id = wm.create_window(Some(desktop_id));
        let mut frame = FrameWindow::new(frame_id, title);
        frame.set_bounds(Rect::new(dialog_x, dialog_y, dialog_width, dialog_height));
        frame.set_parent(Some(desktop_id));

        let content_area = frame.content_area();

        // Create container for content (sits inside the frame at content_area).
        let container_id = wm.create_window(Some(frame_id));
        let mut container = ContainerWindow::new_with_id(container_id, content_area);
        container.set_parent(Some(frame_id));

        // Choose icon color based on type
        let _icon_color = match msg_type {
            MessageBoxType::Info => Color::new(0, 120, 215),    // Blue
            MessageBoxType::Warning => Color::new(255, 165, 0), // Orange
            MessageBoxType::Error => Color::new(220, 20, 60),   // Crimson
        };

        // Padding wraps the root VBox; insets give breathing room.
        let padding_id = wm.create_window(Some(container_id));
        let mut padding = Padding::new_with_id(
            padding_id,
            Rect::new(0, 0, content_area.width, content_area.height),
            10,
            10,
            10,
            10,
        );
        padding.set_parent(Some(container_id));

        // Root VBox: message label on top, button row at bottom (centered).
        let vbox_id = wm.create_window(Some(padding_id));
        let mut vbox = VBox::new_with_id(vbox_id, Rect::new(0, 0, 0, 0));
        vbox.set_parent(Some(padding_id));

        // Message label fills remaining vertical space.
        let label_id = wm.create_window(Some(vbox_id));
        let mut label = Label::new_with_id(label_id, Rect::new(0, 0, 0, 0), message);
        label.set_parent(Some(vbox_id));

        // Button row HBox: [Spacer Fill(1)] [OK Fixed(80)] [Spacer Fill(1)]
        // — sandwich the button between two Fill(1) spacers to centre it.
        let button_row_id = wm.create_window(Some(vbox_id));
        let mut button_row = HBox::new_with_id(button_row_id, Rect::new(0, 0, 0, 0));
        button_row.set_parent(Some(vbox_id));

        let left_spacer_id = wm.create_window(Some(button_row_id));
        let mut left_spacer = Spacer::new_with_id(left_spacer_id, Rect::new(0, 0, 0, 0));
        left_spacer.set_parent(Some(button_row_id));

        let ok_button_id = wm.create_window(Some(button_row_id));
        let mut ok_button = Button::new_with_id(ok_button_id, Rect::new(0, 0, 0, 0), "OK");
        ok_button.set_parent(Some(button_row_id));
        ok_button.on_click(move || {
            close_dialog_with_result(DialogResult::Ok);
        });

        let right_spacer_id = wm.create_window(Some(button_row_id));
        let mut right_spacer = Spacer::new_with_id(right_spacer_id, Rect::new(0, 0, 0, 0));
        right_spacer.set_parent(Some(button_row_id));

        // Wire up layout-container children with sizing hints. Initial
        // relayouts during construction are no-ops (children not in the
        // registry yet); the cascade fires below via `with_window_mut`.
        button_row.add_child(left_spacer_id, SizeHint::Fill(1));
        button_row.add_child(ok_button_id, SizeHint::Fixed(80));
        button_row.add_child(right_spacer_id, SizeHint::Fill(1));

        vbox.add_child(label_id, SizeHint::Fill(1));
        vbox.add_child(button_row_id, SizeHint::Fixed(28));

        padding.set_child(vbox_id);

        // Register windows (set_window_impl automatically adds to z-order)
        wm.set_window_impl(frame_id, Box::new(frame));
        wm.set_window_impl(container_id, Box::new(container));
        wm.set_window_impl(label_id, Box::new(label));
        wm.set_window_impl(left_spacer_id, Box::new(left_spacer));
        wm.set_window_impl(ok_button_id, Box::new(ok_button));
        wm.set_window_impl(right_spacer_id, Box::new(right_spacer));
        wm.set_window_impl(button_row_id, Box::new(button_row));
        wm.set_window_impl(vbox_id, Box::new(vbox));
        wm.set_window_impl(padding_id, Box::new(padding));

        // Wire parent-child registry edges from desktop down to padding.
        // Layout containers maintain their own children via `WindowBase`.
        if let Some(desktop) = wm.window_registry.get_mut(&desktop_id) {
            desktop.add_child(frame_id);
        }
        if let Some(frame) = wm.window_registry.get_mut(&frame_id) {
            frame.add_child(container_id);
        }
        if let Some(container) = wm.window_registry.get_mut(&container_id) {
            container.add_child(padding_id);
        }

        // Trigger the layout cascade.
        wm.with_window_mut(padding_id, |w| {
            w.set_bounds(Rect::new(0, 0, content_area.width, content_area.height));
        });

        // Set as modal dialog
        wm.set_modal_dialog(Some(frame_id));

        (frame_id, ok_button_id)
    })
    .unwrap();

    // Set dialog state
    set_dialog_state(frame_id);

    // Wait for dialog to close (polling loop)
    while is_dialog_open() {
        // Allow other processes to run
        crate::process::yield_if_needed();

        // Check if window still exists
        let exists = with_window_manager(|wm| wm.window_registry.contains_key(&frame_id))
            .unwrap_or(false);

        if !exists {
            break;
        }

        // Small delay
        for _ in 0..10000 {
            core::hint::spin_loop();
        }
    }

    // Clean up
    with_window_manager(|wm| {
        wm.set_modal_dialog(None);
        wm.destroy_window(frame_id);
    });

    clear_dialog_state();
}

/// Show an info message
pub fn show_info(title: &str, message: &str) {
    show_message(title, message, MessageBoxType::Info);
}

/// Show a warning message
pub fn show_warning(title: &str, message: &str) {
    show_message(title, message, MessageBoxType::Warning);
}

/// Show an error message
pub fn show_error(title: &str, message: &str) {
    show_message(title, message, MessageBoxType::Error);
}

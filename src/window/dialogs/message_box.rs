//! Message box dialogs for displaying information to the user

use alloc::boxed::Box;
use alloc::string::String;

use crate::graphics::color::Color;
use crate::window::windows::dialog::{
    clear_dialog_state, close_dialog_with_result, is_dialog_open, set_dialog_state, DialogResult,
};
use crate::window::windows::{Button, ContainerWindow, FrameWindow, Label};
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
    let (frame_id, ok_button_id) = with_window_manager(|wm| {
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

        // Create container for content
        let container_id = wm.create_window(Some(frame_id));
        let mut container = ContainerWindow::new_with_id(
            container_id,
            Rect::new(
                content_area.x,
                content_area.y,
                content_area.width,
                content_area.height,
            ),
        );
        container.set_parent(Some(frame_id));

        // Choose icon color based on type
        let _icon_color = match msg_type {
            MessageBoxType::Info => Color::new(0, 120, 215),    // Blue
            MessageBoxType::Warning => Color::new(255, 165, 0), // Orange
            MessageBoxType::Error => Color::new(220, 20, 60),   // Crimson
        };

        // Create message label
        let label_id = wm.create_window(Some(container_id));
        let label_bounds = Rect::new(
            content_area.x + 20,
            content_area.y + 20,
            content_area.width - 40,
            60,
        );
        let mut label = Label::new_with_id(label_id, label_bounds, message);
        label.set_parent(Some(container_id));

        // Create OK button
        let ok_button_id = wm.create_window(Some(container_id));
        let button_width = 80;
        let button_height = 28;
        let button_x = content_area.x + (content_area.width as i32 - button_width) / 2;
        let button_y = content_area.y + content_area.height as i32 - button_height - 15;
        let button_bounds = Rect::new(button_x, button_y, button_width as u32, button_height as u32);
        let mut ok_button = Button::new_with_id(ok_button_id, button_bounds, "OK");
        ok_button.set_parent(Some(container_id));

        // Set up button callback
        ok_button.on_click(move || {
            close_dialog_with_result(DialogResult::Ok);
        });

        // Register windows (set_window_impl automatically adds to z-order)
        wm.set_window_impl(frame_id, Box::new(frame));
        wm.set_window_impl(container_id, Box::new(container));
        wm.set_window_impl(label_id, Box::new(label));
        wm.set_window_impl(ok_button_id, Box::new(ok_button));

        // Add children
        if let Some(desktop) = wm.window_registry.get_mut(&desktop_id) {
            desktop.add_child(frame_id);
        }
        if let Some(frame) = wm.window_registry.get_mut(&frame_id) {
            frame.add_child(container_id);
        }
        if let Some(container) = wm.window_registry.get_mut(&container_id) {
            container.add_child(label_id);
            container.add_child(ok_button_id);
        }

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

//! Save file dialog for saving files

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::fs::Directory;
use crate::fs::filesystem::FileType;
use crate::graphics::color::Color;
use crate::window::windows::dialog::{
    clear_dialog_state, close_dialog_with_result, get_dialog_result, is_dialog_open,
    set_dialog_state, DialogResult,
};
use crate::window::windows::{
    Button, Column, ContainerWindow, FrameWindow, HBox, Label, MultiColumnList, Padding, SizeHint,
    Spacer, TextInput, VBox,
};
use crate::window::{with_window_manager, Rect, Window, WindowId};

use super::message_box::show_error;

/// Show the save file dialog and return entered filename (or None if cancelled)
/// Note: Actual saving is not implemented - filesystem is read-only
pub fn show_save_dialog(default_name: &str) -> Option<String> {
    // Dialog dimensions
    let dialog_width = 500u32;
    let dialog_height = 450u32;

    // Get screen size and center dialog
    let (screen_width, screen_height) = with_window_manager(|wm| {
        (
            wm.graphics_device.width() as i32,
            wm.graphics_device.height() as i32,
        )
    })
    .unwrap_or((800, 600));

    let dialog_x = (screen_width - dialog_width as i32) / 2;
    let dialog_y = (screen_height - dialog_height as i32) / 2;

    // Get file list from filesystem
    let files = get_file_list("/");

    // Create dialog structure
    let (frame_id, _filename_input_id) = with_window_manager(|wm| {
        // Get desktop for parenting
        let desktop_id = wm
            .get_active_screen()
            .and_then(|s| s.root_window)
            .unwrap_or(WindowId::new());

        // Create frame window
        let frame_id = wm.create_window(Some(desktop_id));
        let mut frame = FrameWindow::new(frame_id, "Save As");
        frame.set_bounds(Rect::new(dialog_x, dialog_y, dialog_width, dialog_height));
        frame.set_parent(Some(desktop_id));

        let content_area = frame.content_area();

        // Create container for content (sits inside the frame at content_area).
        let container_id = wm.create_window(Some(frame_id));
        let mut container = ContainerWindow::new_with_id(container_id, content_area);
        container.set_parent(Some(frame_id));
        container.set_background_color(Color::new(240, 240, 240));

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

        // Root VBox stacks: path label, file list, filename row, button row.
        let vbox_id = wm.create_window(Some(padding_id));
        let mut vbox = VBox::new_with_id(vbox_id, Rect::new(0, 0, 0, 0));
        vbox.set_parent(Some(padding_id));

        // Path label
        let path_label_id = wm.create_window(Some(vbox_id));
        let mut path_label =
            Label::new_with_id(path_label_id, Rect::new(0, 0, 0, 0), "Save in: /");
        path_label.set_parent(Some(vbox_id));

        let spacer_after_path_id = wm.create_window(Some(vbox_id));
        let mut spacer_after_path =
            Spacer::new_with_id(spacer_after_path_id, Rect::new(0, 0, 0, 0));
        spacer_after_path.set_parent(Some(vbox_id));

        // File list (fills remaining vertical space).
        let list_id = wm.create_window(Some(vbox_id));
        let columns = alloc::vec![
            Column::new("Name", 250),
            Column::new("Size", 80),
            Column::new("Type", 100),
        ];
        let mut file_list = MultiColumnList::new_with_id(list_id, Rect::new(0, 0, 0, 0), columns);
        file_list.set_parent(Some(vbox_id));

        // Populate list with files
        for (name, size, file_type) in &files {
            file_list.add_row(alloc::vec![name.clone(), size.clone(), file_type.clone()]);
        }

        let spacer_after_list_id = wm.create_window(Some(vbox_id));
        let mut spacer_after_list =
            Spacer::new_with_id(spacer_after_list_id, Rect::new(0, 0, 0, 0));
        spacer_after_list.set_parent(Some(vbox_id));

        // Filename row HBox: [label "File name:" Fixed(80)] [input Fill(1)]
        let fn_row_id = wm.create_window(Some(vbox_id));
        let mut fn_row = HBox::new_with_id(fn_row_id, Rect::new(0, 0, 0, 0));
        fn_row.set_parent(Some(vbox_id));

        let fn_label_id = wm.create_window(Some(fn_row_id));
        let mut fn_label = Label::new_with_id(fn_label_id, Rect::new(0, 0, 0, 0), "File name:");
        fn_label.set_parent(Some(fn_row_id));

        let filename_input_id = wm.create_window(Some(fn_row_id));
        let mut filename_input =
            TextInput::new_with_id(filename_input_id, Rect::new(0, 0, 0, 0));
        filename_input.set_text(default_name);
        filename_input.set_parent(Some(fn_row_id));

        let spacer_after_fn_id = wm.create_window(Some(vbox_id));
        let mut spacer_after_fn = Spacer::new_with_id(spacer_after_fn_id, Rect::new(0, 0, 0, 0));
        spacer_after_fn.set_parent(Some(vbox_id));

        // Button row HBox: [Spacer Fill(1)] [Save Fixed(80)] [Spacer Fixed(8)] [Cancel Fixed(80)]
        let button_row_id = wm.create_window(Some(vbox_id));
        let mut button_row = HBox::new_with_id(button_row_id, Rect::new(0, 0, 0, 0));
        button_row.set_parent(Some(vbox_id));

        let left_spacer_id = wm.create_window(Some(button_row_id));
        let mut left_spacer = Spacer::new_with_id(left_spacer_id, Rect::new(0, 0, 0, 0));
        left_spacer.set_parent(Some(button_row_id));

        // Save button
        let save_button_id = wm.create_window(Some(button_row_id));
        let mut save_button = Button::new_with_id(save_button_id, Rect::new(0, 0, 0, 0), "Save");
        save_button.set_parent(Some(button_row_id));
        save_button.set_bg_color(Color::new(0, 120, 215));
        save_button.set_text_color(Color::WHITE);
        let _input_id_for_save = filename_input_id;
        save_button.on_click(move || {
            // For now, just close with cancel since save isn't implemented
            // The actual implementation would get the filename and try to save
            close_dialog_with_result(DialogResult::Custom(1)); // 1 = save attempted
        });

        let mid_spacer_id = wm.create_window(Some(button_row_id));
        let mut mid_spacer = Spacer::new_with_id(mid_spacer_id, Rect::new(0, 0, 0, 0));
        mid_spacer.set_parent(Some(button_row_id));

        // Cancel button
        let cancel_button_id = wm.create_window(Some(button_row_id));
        let mut cancel_button =
            Button::new_with_id(cancel_button_id, Rect::new(0, 0, 0, 0), "Cancel");
        cancel_button.set_parent(Some(button_row_id));
        cancel_button.on_click(|| {
            close_dialog_with_result(DialogResult::Cancel);
        });

        // Wire up layout-container children with sizing hints. The
        // initial relayouts are no-ops (children aren't in the registry
        // yet); the final cascade fires below via `with_window_mut`.
        button_row.add_child(left_spacer_id, SizeHint::Fill(1));
        button_row.add_child(save_button_id, SizeHint::Fixed(80));
        button_row.add_child(mid_spacer_id, SizeHint::Fixed(8));
        button_row.add_child(cancel_button_id, SizeHint::Fixed(80));

        fn_row.add_child(fn_label_id, SizeHint::Fixed(80));
        fn_row.add_child(filename_input_id, SizeHint::Fill(1));

        vbox.add_child(path_label_id, SizeHint::Fixed(20));
        vbox.add_child(spacer_after_path_id, SizeHint::Fixed(10));
        vbox.add_child(list_id, SizeHint::Fill(1));
        vbox.add_child(spacer_after_list_id, SizeHint::Fixed(10));
        vbox.add_child(fn_row_id, SizeHint::Fixed(24));
        vbox.add_child(spacer_after_fn_id, SizeHint::Fixed(10));
        vbox.add_child(button_row_id, SizeHint::Fixed(30));

        padding.set_child(vbox_id);

        // Register windows (set_window_impl automatically adds to z-order)
        wm.set_window_impl(frame_id, Box::new(frame));
        wm.set_window_impl(container_id, Box::new(container));
        wm.set_window_impl(path_label_id, Box::new(path_label));
        wm.set_window_impl(spacer_after_path_id, Box::new(spacer_after_path));
        wm.set_window_impl(list_id, Box::new(file_list));
        wm.set_window_impl(spacer_after_list_id, Box::new(spacer_after_list));
        wm.set_window_impl(fn_label_id, Box::new(fn_label));
        wm.set_window_impl(filename_input_id, Box::new(filename_input));
        wm.set_window_impl(fn_row_id, Box::new(fn_row));
        wm.set_window_impl(spacer_after_fn_id, Box::new(spacer_after_fn));
        wm.set_window_impl(left_spacer_id, Box::new(left_spacer));
        wm.set_window_impl(save_button_id, Box::new(save_button));
        wm.set_window_impl(mid_spacer_id, Box::new(mid_spacer));
        wm.set_window_impl(cancel_button_id, Box::new(cancel_button));
        wm.set_window_impl(button_row_id, Box::new(button_row));
        wm.set_window_impl(vbox_id, Box::new(vbox));
        wm.set_window_impl(padding_id, Box::new(padding));

        // Wire the parent-child registry edges from the desktop down to
        // the layout root. Layout containers track their own children
        // via `WindowBase`, so we only need the chain above the layout:
        //   desktop -> frame -> container -> padding (-> vbox -> ...)
        if let Some(desktop) = wm.window_registry.get_mut(&desktop_id) {
            desktop.add_child(frame_id);
        }
        if let Some(frame) = wm.window_registry.get_mut(&frame_id) {
            frame.add_child(container_id);
        }
        if let Some(container) = wm.window_registry.get_mut(&container_id) {
            container.add_child(padding_id);
        }

        // Trigger the layout cascade by setting the Padding's bounds
        // through the active manager.
        wm.with_window_mut(padding_id, |w| {
            w.set_bounds(Rect::new(0, 0, content_area.width, content_area.height));
        });

        // Set as modal dialog
        wm.set_modal_dialog(Some(frame_id));

        (frame_id, filename_input_id)
    })
    .unwrap();

    // Set dialog state
    set_dialog_state(frame_id);

    // Wait for dialog to close
    while is_dialog_open() {
        crate::process::yield_if_needed();

        let exists = with_window_manager(|wm| wm.window_registry.contains_key(&frame_id))
            .unwrap_or(false);

        if !exists {
            break;
        }

        for _ in 0..10000 {
            core::hint::spin_loop();
        }
    }

    // Get result before cleanup
    let result = get_dialog_result();

    // Clean up
    with_window_manager(|wm| {
        wm.set_modal_dialog(None);
        wm.destroy_window(frame_id);
    });

    clear_dialog_state();

    // Handle result
    match result {
        Some(DialogResult::Custom(1)) => {
            // Save was attempted - show not implemented message
            show_error(
                "Save Not Implemented",
                "The filesystem is currently read-only. Save functionality is not yet available.",
            );
            None
        }
        _ => None,
    }
}

/// Get list of files from a directory
fn get_file_list(path: &str) -> Vec<(String, String, String)> {
    let mut files = Vec::new();

    if let Ok(dir) = Directory::open(path) {
        for entry in dir.entries() {
            let name = String::from(entry.name_str());
            let is_dir = entry.file_type == FileType::Directory;
            let size = if is_dir {
                String::from("<DIR>")
            } else {
                format_size(entry.size as u64)
            };
            let file_type = if is_dir {
                String::from("Folder")
            } else {
                get_file_type(&name)
            };
            files.push((name, size, file_type));
        }
    }

    files
}

/// Format file size in human-readable form
fn format_size(size: u64) -> String {
    if size < 1024 {
        format!("{} B", size)
    } else if size < 1024 * 1024 {
        format!("{} KB", size / 1024)
    } else {
        format!("{} MB", size / (1024 * 1024))
    }
}

/// Get file type from extension
fn get_file_type(name: &str) -> String {
    if let Some(dot_pos) = name.rfind('.') {
        let ext = &name[dot_pos + 1..].to_uppercase();
        match ext.as_str() {
            "TXT" => String::from("Text File"),
            "MD" => String::from("Markdown"),
            "RS" => String::from("Rust Source"),
            "C" | "H" => String::from("C Source"),
            "BMP" => String::from("Bitmap Image"),
            "PNG" => String::from("PNG Image"),
            "FNT" => String::from("Font File"),
            "TTF" => String::from("TrueType Font"),
            _ => format!("{} File", ext),
        }
    } else {
        String::from("File")
    }
}

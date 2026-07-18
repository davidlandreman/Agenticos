//! Open file dialog for selecting files to open

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::fs::filesystem::FileType;
use crate::fs::Directory;
use crate::window::windows::dialog::{
    clear_dialog_state, close_dialog_with_result, get_dialog_id, get_dialog_result, is_dialog_open,
    set_dialog_state, DialogResult,
};
use crate::window::windows::{
    Button, Column, ContainerWindow, FrameWindow, HBox, Label, MultiColumnList, Padding, SizeHint,
    Spacer, VBox,
};
use crate::window::{with_window_manager, Rect, Window, WindowId};

/// Open the file dialog (non-blocking)
/// Returns the dialog's frame ID. Use `poll_file_dialog()` to check for results.
pub fn open_file_dialog() -> WindowId {
    // Dialog dimensions
    let dialog_width = 500u32;
    let dialog_height = 400u32;

    // Get screen size and center dialog
    let (screen_width, screen_height) = with_window_manager(|wm| {
        let (width, height) = wm.screen_dimensions();
        (width as i32, height as i32)
    })
    .unwrap_or((800, 600));

    let dialog_x = (screen_width - dialog_width as i32) / 2;
    let dialog_y = (screen_height - dialog_height as i32) / 2;

    // Get file list from filesystem
    let files = get_file_list("/");

    // Create dialog structure
    let frame_id = with_window_manager(|wm| {
        // Get desktop for parenting
        let desktop_id = wm
            .get_active_screen()
            .and_then(|s| s.root_window)
            .unwrap_or(WindowId::new());

        // Create frame window
        let frame_id = wm.create_window(Some(desktop_id));
        let mut frame = FrameWindow::new(frame_id, "Open File");
        frame.set_bounds(Rect::new(dialog_x, dialog_y, dialog_width, dialog_height));
        frame.set_parent(Some(desktop_id));

        let content_area = frame.content_area();

        // Create container for content (sits inside the frame at content_area).
        let container_id = wm.create_window(Some(frame_id));
        let mut container = ContainerWindow::new_with_id(container_id, content_area);
        container.set_parent(Some(frame_id));
        container.set_background_color(crate::window::theme::controls::palette().content_bg);

        // Padding wraps the VBox; insets give breathing room around all sides.
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

        // Root VBox: stacks the location label, file list, and button row.
        let vbox_id = wm.create_window(Some(padding_id));
        let mut vbox = VBox::new_with_id(vbox_id, Rect::new(0, 0, 0, 0));
        vbox.set_parent(Some(padding_id));

        // Path label (top of the VBox).
        let path_label_id = wm.create_window(Some(vbox_id));
        let mut path_label =
            Label::new_with_id(path_label_id, Rect::new(0, 0, 0, 0), "Location: /");
        path_label.set_parent(Some(vbox_id));

        // Spacer between label and list.
        let spacer_top_id = wm.create_window(Some(vbox_id));
        let mut spacer_top = Spacer::new_with_id(spacer_top_id, Rect::new(0, 0, 0, 0));
        spacer_top.set_parent(Some(vbox_id));

        // File list (fills the remaining vertical space).
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

        // Set up selection callback (double-click to open)
        let files_clone = files.clone();
        file_list.on_select(move |selection| {
            if let Some(row_idx) = selection.iter().next() {
                if let Some((name, _, _)) = files_clone.get(row_idx) {
                    close_dialog_with_result(DialogResult::FilePath(format!("/{}", name)));
                }
            }
        });

        // Spacer between list and button row.
        let spacer_bot_id = wm.create_window(Some(vbox_id));
        let mut spacer_bot = Spacer::new_with_id(spacer_bot_id, Rect::new(0, 0, 0, 0));
        spacer_bot.set_parent(Some(vbox_id));

        // Button row HBox: right-aligned via a Fill(1) Spacer at the left.
        let button_row_id = wm.create_window(Some(vbox_id));
        let mut button_row = HBox::new_with_id(button_row_id, Rect::new(0, 0, 0, 0));
        button_row.set_parent(Some(vbox_id));

        let left_spacer_id = wm.create_window(Some(button_row_id));
        let mut left_spacer = Spacer::new_with_id(left_spacer_id, Rect::new(0, 0, 0, 0));
        left_spacer.set_parent(Some(button_row_id));

        // Open button
        let open_button_id = wm.create_window(Some(button_row_id));
        let mut open_button = Button::new_with_id(open_button_id, Rect::new(0, 0, 0, 0), "Open");
        open_button.set_parent(Some(button_row_id));
        open_button.set_default(true);
        open_button.on_click(move || {
            // For now, just close - ideally we'd get the selected item
            close_dialog_with_result(DialogResult::Cancel);
        });

        // Spacer between Open and Cancel buttons.
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

        // Wire layout-container child lists with sizing hints. Each
        // `add_child` triggers a `relayout`, which is a no-op while the
        // active manager is unset (children are not yet in the registry).
        // The final cascade fires once the root layout is registered and
        // `set_bounds` is invoked through `with_window_mut`.
        button_row.add_child(left_spacer_id, SizeHint::Fill(1));
        button_row.add_child(open_button_id, SizeHint::Fixed(80));
        button_row.add_child(mid_spacer_id, SizeHint::Fixed(8));
        button_row.add_child(cancel_button_id, SizeHint::Fixed(80));

        vbox.add_child(path_label_id, SizeHint::Fixed(20));
        vbox.add_child(spacer_top_id, SizeHint::Fixed(10));
        vbox.add_child(list_id, SizeHint::Fill(1));
        vbox.add_child(spacer_bot_id, SizeHint::Fixed(10));
        vbox.add_child(button_row_id, SizeHint::Fixed(30));

        padding.set_child(vbox_id);

        // Register windows (set_window_impl automatically adds to z-order)
        wm.set_window_impl(frame_id, Box::new(frame));
        wm.set_window_impl(container_id, Box::new(container));
        wm.set_window_impl(path_label_id, Box::new(path_label));
        wm.set_window_impl(spacer_top_id, Box::new(spacer_top));
        wm.set_window_impl(list_id, Box::new(file_list));
        wm.set_window_impl(spacer_bot_id, Box::new(spacer_bot));
        wm.set_window_impl(left_spacer_id, Box::new(left_spacer));
        wm.set_window_impl(open_button_id, Box::new(open_button));
        wm.set_window_impl(mid_spacer_id, Box::new(mid_spacer));
        wm.set_window_impl(cancel_button_id, Box::new(cancel_button));
        wm.set_window_impl(button_row_id, Box::new(button_row));
        wm.set_window_impl(vbox_id, Box::new(vbox));
        wm.set_window_impl(padding_id, Box::new(padding));

        // Wire the parent-child registry edges. The layout containers
        // already track their own child lists via `WindowBase`, so we
        // only need to attach the chain rooted at the desktop:
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

        // Trigger the layout cascade by setting the Padding's bounds via
        // the active manager. This walks Padding -> VBox -> children and
        // writes each computed bounds back through `with_window_mut`.
        wm.with_window_mut(padding_id, |w| {
            w.set_bounds(Rect::new(0, 0, content_area.width, content_area.height));
        });

        // Set as modal dialog
        wm.set_modal_dialog(Some(frame_id));

        // Bring to front and focus
        wm.bring_to_front(frame_id);

        frame_id
    })
    .unwrap();

    // Set dialog state for tracking
    set_dialog_state(frame_id);

    frame_id
}

/// Poll for file dialog result (non-blocking)
/// Returns Some(path) if user selected a file, Some("") if cancelled, None if still open
pub fn poll_file_dialog() -> Option<Option<String>> {
    if is_dialog_open() {
        // Check if the dialog window still exists
        let dialog_id = get_dialog_id();
        if let Some(id) = dialog_id {
            let exists =
                with_window_manager(|wm| wm.window_registry.contains_key(&id)).unwrap_or(false);
            if !exists {
                // Window was closed (e.g., by close button)
                close_dialog_with_result(DialogResult::Cancel);
            } else {
                return None; // Still open
            }
        } else {
            return None;
        }
    }

    // Dialog closed - get result and clean up
    let result = get_dialog_result();

    // Clean up
    if let Some(id) = get_dialog_id() {
        with_window_manager(|wm| {
            wm.set_modal_dialog(None);
            wm.destroy_window(id);
        });
    }
    clear_dialog_state();

    // Return result
    match result {
        Some(DialogResult::FilePath(path)) => Some(Some(path)),
        Some(DialogResult::Cancel) | Some(DialogResult::Ok) => Some(None),
        _ => Some(None),
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

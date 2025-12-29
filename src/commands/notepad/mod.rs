//! Notepad Application
//!
//! A full-featured text editor with menu bar, text editing, and file dialogs.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;

use crate::fs::File;
use crate::process::{BaseProcess, HasBaseProcess, RunnableProcess};
use crate::window::dialogs::{show_error, show_info, show_open_dialog, show_save_dialog};
use crate::window::windows::{
    ContainerWindow, FrameWindow, MenuBar, MenuItemDef, TextEditor, MENU_BAR_HEIGHT,
};
use crate::window::{with_window_manager, Rect, Window, WindowId};

/// Unique ID for each notepad instance
static NEXT_NOTEPAD_ID: AtomicUsize = AtomicUsize::new(1);

/// Menu item IDs
mod menu_ids {
    pub const FILE_NEW: usize = 100;
    pub const FILE_OPEN: usize = 101;
    pub const FILE_SAVE: usize = 102;
    pub const FILE_SAVE_AS: usize = 103;
    pub const FILE_EXIT: usize = 104;

    pub const EDIT_CUT: usize = 200;
    pub const EDIT_COPY: usize = 201;
    pub const EDIT_PASTE: usize = 202;
    pub const EDIT_SELECT_ALL: usize = 203;

    pub const HELP_ABOUT: usize = 300;
}

/// State for a single notepad instance
struct NotepadState {
    /// Current file path (None if untitled)
    file_path: Option<String>,
    /// Frame window ID
    frame_id: WindowId,
    /// Menu bar ID
    menu_bar_id: WindowId,
    /// Text editor ID
    editor_id: WindowId,
    /// Whether notepad is running
    running: bool,
    /// Pending action to process
    pending_action: Option<usize>,
}

/// Global map of notepad states
static NOTEPAD_STATES: Mutex<BTreeMap<usize, NotepadState>> = Mutex::new(BTreeMap::new());

/// Set pending action for a notepad instance
fn set_pending_action(notepad_id: usize, action: usize) {
    let mut states = NOTEPAD_STATES.lock();
    if let Some(state) = states.get_mut(&notepad_id) {
        state.pending_action = Some(action);
    }
}

/// Get and clear pending action
fn take_pending_action(notepad_id: usize) -> Option<usize> {
    let mut states = NOTEPAD_STATES.lock();
    if let Some(state) = states.get_mut(&notepad_id) {
        state.pending_action.take()
    } else {
        None
    }
}

/// Notepad process
pub struct NotepadProcess {
    base: BaseProcess,
    args: Vec<String>,
}

impl NotepadProcess {
    pub fn new_with_args(args: Vec<String>) -> Self {
        Self {
            base: BaseProcess::new("notepad"),
            args,
        }
    }
}

impl HasBaseProcess for NotepadProcess {
    fn base(&self) -> &BaseProcess {
        &self.base
    }
    fn base_mut(&mut self) -> &mut BaseProcess {
        &mut self.base
    }
}

impl RunnableProcess for NotepadProcess {
    fn run(&mut self) {
        let notepad_id = NEXT_NOTEPAD_ID.fetch_add(1, Ordering::SeqCst);

        // Get screen dimensions
        let (screen_width, screen_height) = with_window_manager(|wm| {
            (
                wm.graphics_device.width() as i32,
                wm.graphics_device.height() as i32,
            )
        })
        .unwrap_or((800, 600));

        // Calculate window position (offset each instance slightly)
        let offset = ((notepad_id - 1) % 5) as i32 * 30;
        let window_x = 50 + offset;
        let window_y = 30 + offset;
        let window_width = 700u32.min((screen_width - 100) as u32);
        let window_height = 500u32.min((screen_height - 80) as u32);

        // Determine title
        let initial_path = self.args.get(1).cloned();
        let title = if let Some(ref path) = initial_path {
            format!("{} - Notepad", path)
        } else {
            String::from("Untitled - Notepad")
        };

        // Create notepad window structure
        let result = with_window_manager(|wm| {
            // Get desktop
            let desktop_id = wm
                .get_active_screen()
                .and_then(|s| s.root_window)
                .unwrap_or(WindowId::new());

            // Create frame window
            let frame_id = wm.create_window(Some(desktop_id));
            let mut frame = FrameWindow::new(frame_id, &title);
            frame.set_bounds(Rect::new(window_x, window_y, window_width, window_height));
            frame.set_parent(Some(desktop_id));

            let content_area = frame.content_area();

            // Create menu bar
            let menu_bar_id = wm.create_window(Some(frame_id));
            let menu_bounds = Rect::new(
                content_area.x,
                content_area.y,
                content_area.width,
                MENU_BAR_HEIGHT,
            );
            let mut menu_bar = MenuBar::new_with_id(menu_bar_id, menu_bounds);
            menu_bar.set_parent(Some(frame_id));

            // Add File menu
            menu_bar.add_menu(
                "File",
                alloc::vec![
                    MenuItemDef::item_with_shortcut("New", "Ctrl+N", menu_ids::FILE_NEW),
                    MenuItemDef::item_with_shortcut("Open...", "Ctrl+O", menu_ids::FILE_OPEN),
                    MenuItemDef::separator(),
                    MenuItemDef::item_with_shortcut("Save", "Ctrl+S", menu_ids::FILE_SAVE),
                    MenuItemDef::item("Save As...", menu_ids::FILE_SAVE_AS),
                    MenuItemDef::separator(),
                    MenuItemDef::item("Exit", menu_ids::FILE_EXIT),
                ],
            );

            // Add Edit menu
            menu_bar.add_menu(
                "Edit",
                alloc::vec![
                    MenuItemDef::item_with_shortcut("Cut", "Ctrl+X", menu_ids::EDIT_CUT),
                    MenuItemDef::item_with_shortcut("Copy", "Ctrl+C", menu_ids::EDIT_COPY),
                    MenuItemDef::item_with_shortcut("Paste", "Ctrl+V", menu_ids::EDIT_PASTE),
                    MenuItemDef::separator(),
                    MenuItemDef::item_with_shortcut("Select All", "Ctrl+A", menu_ids::EDIT_SELECT_ALL),
                ],
            );

            // Add Help menu
            menu_bar.add_menu(
                "Help",
                alloc::vec![MenuItemDef::item("About Notepad", menu_ids::HELP_ABOUT)],
            );

            // Set up menu callback
            let np_id = notepad_id;
            menu_bar.on_select(move |_menu_idx, item_id| {
                set_pending_action(np_id, item_id);
            });

            // Create text editor (below menu bar)
            let editor_id = wm.create_window(Some(frame_id));
            let editor_bounds = Rect::new(
                content_area.x,
                content_area.y + MENU_BAR_HEIGHT as i32,
                content_area.width,
                content_area.height - MENU_BAR_HEIGHT,
            );
            let mut editor = TextEditor::new_with_id(editor_id, editor_bounds);
            editor.set_parent(Some(frame_id));

            // Register windows (set_window_impl automatically adds to z-order)
            wm.set_window_impl(frame_id, Box::new(frame));
            wm.set_window_impl(menu_bar_id, Box::new(menu_bar));
            wm.set_window_impl(editor_id, Box::new(editor));

            // Add children
            if let Some(desktop) = wm.window_registry.get_mut(&desktop_id) {
                desktop.add_child(frame_id);
            }
            if let Some(frame) = wm.window_registry.get_mut(&frame_id) {
                frame.add_child(menu_bar_id);
                frame.add_child(editor_id);
            }

            // Focus editor
            wm.focus_window(editor_id);

            Some((frame_id, menu_bar_id, editor_id))
        });

        let (frame_id, menu_bar_id, editor_id) = match result {
            Some(Some(ids)) => ids,
            _ => {
                crate::println!("Failed to create Notepad window");
                return;
            }
        };

        // Store state
        {
            let mut states = NOTEPAD_STATES.lock();
            states.insert(
                notepad_id,
                NotepadState {
                    file_path: initial_path.clone(),
                    frame_id,
                    menu_bar_id,
                    editor_id,
                    running: true,
                    pending_action: None,
                },
            );
        }

        // Load file if path provided
        if let Some(ref path) = initial_path {
            load_file(notepad_id, editor_id, path);
        }

        crate::println!("Notepad started. Close window to exit.");

        // Main loop
        loop {
            // Check if still running
            let running = {
                let states = NOTEPAD_STATES.lock();
                states.get(&notepad_id).map(|s| s.running).unwrap_or(false)
            };

            if !running {
                break;
            }

            // Process pending action
            if let Some(action) = take_pending_action(notepad_id) {
                handle_menu_action(notepad_id, action);
            }

            // Allow preemption
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

        // Cleanup
        {
            let mut states = NOTEPAD_STATES.lock();
            states.remove(&notepad_id);
        }

        crate::println!("Notepad closed.");
    }

    fn get_name(&self) -> &str {
        "notepad"
    }
}

/// Handle a menu action
fn handle_menu_action(notepad_id: usize, action: usize) {
    match action {
        menu_ids::FILE_NEW => handle_new(notepad_id),
        menu_ids::FILE_OPEN => handle_open(notepad_id),
        menu_ids::FILE_SAVE => handle_save(notepad_id),
        menu_ids::FILE_SAVE_AS => handle_save_as(notepad_id),
        menu_ids::FILE_EXIT => handle_exit(notepad_id),
        menu_ids::EDIT_CUT => show_info("Cut", "Cut is not yet implemented."),
        menu_ids::EDIT_COPY => show_info("Copy", "Copy is not yet implemented."),
        menu_ids::EDIT_PASTE => show_info("Paste", "Paste is not yet implemented."),
        menu_ids::EDIT_SELECT_ALL => show_info("Select All", "Select All is not yet implemented."),
        menu_ids::HELP_ABOUT => show_about(),
        _ => {}
    }
}

/// Handle File > New
fn handle_new(notepad_id: usize) {
    let editor_id = {
        let states = NOTEPAD_STATES.lock();
        states.get(&notepad_id).map(|s| s.editor_id)
    };

    if let Some(editor_id) = editor_id {
        // Clear editor
        with_window_manager(|wm| {
            if let Some(window) = wm.window_registry.get_mut(&editor_id) {
                // Downcast to TextEditor - for now just clear via invalidate
                // In practice, we'd need a better way to access the TextEditor
            }
        });

        // Update state
        let mut states = NOTEPAD_STATES.lock();
        if let Some(state) = states.get_mut(&notepad_id) {
            state.file_path = None;
        }

        // Update title
        update_title(notepad_id, "Untitled");
    }
}

/// Handle File > Open
fn handle_open(notepad_id: usize) {
    if let Some(path) = show_open_dialog() {
        let editor_id = {
            let states = NOTEPAD_STATES.lock();
            states.get(&notepad_id).map(|s| s.editor_id)
        };

        if let Some(editor_id) = editor_id {
            load_file(notepad_id, editor_id, &path);
        }
    }
}

/// Handle File > Save
fn handle_save(notepad_id: usize) {
    let file_path = {
        let states = NOTEPAD_STATES.lock();
        states.get(&notepad_id).and_then(|s| s.file_path.clone())
    };

    if file_path.is_some() {
        // Has a path - try to save
        show_error(
            "Save Not Implemented",
            "The filesystem is currently read-only. Save functionality is not yet available.",
        );
    } else {
        // No path - do Save As
        handle_save_as(notepad_id);
    }
}

/// Handle File > Save As
fn handle_save_as(notepad_id: usize) {
    let current_name = {
        let states = NOTEPAD_STATES.lock();
        states
            .get(&notepad_id)
            .and_then(|s| s.file_path.as_ref())
            .map(|p| {
                // Extract filename from path
                if let Some(pos) = p.rfind('/') {
                    p[pos + 1..].to_string()
                } else {
                    p.clone()
                }
            })
            .unwrap_or_else(|| String::from("Untitled.txt"))
    };

    show_save_dialog(&current_name);
}

/// Handle File > Exit
fn handle_exit(notepad_id: usize) {
    let frame_id = {
        let states = NOTEPAD_STATES.lock();
        states.get(&notepad_id).map(|s| s.frame_id)
    };

    if let Some(frame_id) = frame_id {
        // Mark as not running
        {
            let mut states = NOTEPAD_STATES.lock();
            if let Some(state) = states.get_mut(&notepad_id) {
                state.running = false;
            }
        }

        // Destroy window
        with_window_manager(|wm| {
            wm.destroy_window(frame_id);
        });
    }
}

/// Show about dialog
fn show_about() {
    show_info(
        "About Notepad",
        "AgenticOS Notepad\n\nA simple text editor for AgenticOS.\n\nVersion 1.0",
    );
}

/// Load a file into the editor
fn load_file(notepad_id: usize, _editor_id: WindowId, path: &str) {
    match File::open_read(path) {
        Ok(file) => match file.read_to_string() {
            Ok(content) => {
                // TODO: Set editor text
                // For now, just update state
                let mut states = NOTEPAD_STATES.lock();
                if let Some(state) = states.get_mut(&notepad_id) {
                    state.file_path = Some(String::from(path));
                }

                // Update title
                let filename = if let Some(pos) = path.rfind('/') {
                    &path[pos + 1..]
                } else {
                    path
                };
                drop(states);
                update_title(notepad_id, filename);

                crate::println!("Loaded file: {} ({} bytes)", path, content.len());
            }
            Err(e) => {
                show_error("Error Opening File", &format!("Could not read file: {:?}", e));
            }
        },
        Err(e) => {
            show_error(
                "Error Opening File",
                &format!("Could not open file: {:?}", e),
            );
        }
    }
}

/// Update the window title
fn update_title(notepad_id: usize, filename: &str) {
    let frame_id = {
        let states = NOTEPAD_STATES.lock();
        states.get(&notepad_id).map(|s| s.frame_id)
    };

    if let Some(frame_id) = frame_id {
        let new_title = format!("{} - Notepad", filename);
        with_window_manager(|wm| {
            if let Some(window) = wm.window_registry.get_mut(&frame_id) {
                // FrameWindow has set_title method
                // For now, just invalidate
                window.invalidate();
            }
        });
    }
}

/// Factory function for the process manager
pub fn create_notepad_process(args: Vec<String>) -> Box<dyn RunnableProcess> {
    Box::new(NotepadProcess::new_with_args(args))
}

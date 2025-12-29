//! Dialog infrastructure for modal dialogs
//!
//! Provides helper types and functions for creating and managing modal dialogs.

use alloc::string::String;
use spin::Mutex;

use crate::window::WindowId;

/// Result from a dialog
#[derive(Debug, Clone)]
pub enum DialogResult {
    /// User confirmed (OK button)
    Ok,
    /// User cancelled (Cancel button or closed)
    Cancel,
    /// User selected a file path
    FilePath(String),
    /// Custom result with a numeric value
    Custom(usize),
}

/// State of a dialog being shown
pub struct DialogState {
    /// The dialog's window ID
    pub dialog_id: WindowId,
    /// The result when dialog closes
    pub result: Option<DialogResult>,
    /// Whether dialog is still open
    pub is_open: bool,
}

/// Global dialog state (only one modal dialog at a time)
static DIALOG_STATE: Mutex<Option<DialogState>> = Mutex::new(None);

/// Set a dialog as the current modal dialog
pub fn set_dialog_state(dialog_id: WindowId) {
    let mut state = DIALOG_STATE.lock();
    *state = Some(DialogState {
        dialog_id,
        result: None,
        is_open: true,
    });
}

/// Close the current dialog with a result
pub fn close_dialog_with_result(result: DialogResult) {
    let mut state = DIALOG_STATE.lock();
    if let Some(ref mut s) = *state {
        s.result = Some(result);
        s.is_open = false;
    }
}

/// Check if a dialog is currently open
pub fn is_dialog_open() -> bool {
    let state = DIALOG_STATE.lock();
    state.as_ref().map_or(false, |s| s.is_open)
}

/// Get the dialog result (if dialog has closed)
pub fn get_dialog_result() -> Option<DialogResult> {
    let state = DIALOG_STATE.lock();
    state.as_ref().and_then(|s| {
        if !s.is_open {
            s.result.clone()
        } else {
            None
        }
    })
}

/// Clear the dialog state
pub fn clear_dialog_state() {
    let mut state = DIALOG_STATE.lock();
    *state = None;
}

/// Get the current dialog window ID
pub fn get_dialog_id() -> Option<WindowId> {
    let state = DIALOG_STATE.lock();
    state.as_ref().map(|s| s.dialog_id)
}

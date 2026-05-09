//! OS Dialogs Library
//!
//! Provides reusable dialog windows for common operations:
//! - Message boxes (info, warning, error)
//! - Open file dialogs
//! - Save file dialogs

pub mod message_box;
pub mod file_open;
pub mod file_save;

pub use message_box::{show_info, show_error};
pub use file_open::{open_file_dialog, poll_file_dialog};
pub use file_save::show_save_dialog;

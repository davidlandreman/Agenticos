//! OS Dialogs Library
//!
//! Provides reusable dialog windows for common operations:
//! - Message boxes (info, warning, error)
//! - Open file dialogs
//! - Save file dialogs

#[allow(dead_code)]
pub mod file_open;
#[allow(dead_code)]
pub mod file_save;
#[allow(dead_code)]
pub mod message_box;
pub mod run;

#[allow(unused_imports)]
pub use file_open::{open_file_dialog, poll_file_dialog};
#[allow(unused_imports)]
pub use file_save::show_save_dialog;
#[allow(unused_imports)]
pub use message_box::{show_error, show_info};
pub use run::{open_run_dialog, poll_run_dialog};

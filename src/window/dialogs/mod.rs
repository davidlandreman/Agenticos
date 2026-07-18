//! OS Dialogs Library
//!
//! Provides reusable dialog windows for common operations:
//! - Message boxes (info, warning, error)
//! - Run command dialog

#[allow(dead_code)]
pub mod message_box;
pub mod run;

#[allow(unused_imports)]
pub use message_box::{show_error, show_info};
pub use run::{open_run_dialog, poll_run_dialog};

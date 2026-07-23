//! OS Dialogs Library
//!
//! Provides reusable dialog windows for common operations:
//! - Message boxes (info, warning, error)

#[allow(dead_code)]
pub mod message_box;

#[allow(unused_imports)]
pub use message_box::{show_error, show_info};

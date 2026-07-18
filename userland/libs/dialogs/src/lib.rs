//! Shared ring-3 common-dialog library: modal `FileDialog`, `MessageBox`, and
//! `ColorPicker` built from the ring-3 GUI syscalls plus `getdents64`/`fstat`.
//!
//! # Retained-mode API
//!
//! Each dialog owns its own [`gui::Window`] (created in the constructor,
//! destroyed on drop) and exposes:
//!
//! ```ignore
//! fn window_handle(&self) -> u32;
//! fn handle_event(&mut self, event: &runtime::GuiEvent) -> DialogStatus<T>;
//! ```
//!
//! The dialog renders itself internally (in its constructor and after each
//! handled event). The host app keeps running its own event loop and routes
//! events whose `event.window` matches the dialog to `handle_event`, dropping
//! the dialog when it returns [`DialogStatus::Done`].
//!
//! # Modality is app-side discipline
//!
//! The kernel does not block input to other windows. While a modal is open the
//! host **must ignore key/mouse events targeting its own main window** (it may
//! still service Resize/Close/Focus so the main window keeps repainting). A
//! single-modal host can hold one `Option<Modal>` field and dispatch via
//! [`Modal::window_handle`] / [`Modal::handle_event`].

#![no_std]

extern crate alloc;

use alloc::string::String;

mod color_picker;
mod file_dialog;
mod message_box;
pub mod path;

pub use color_picker::ColorPicker;
pub use file_dialog::{FileDialog, FileDialogOptions, FileFilter, FileMode, FileView};
pub use gui::file_ui::{FilePlace, PlaceIcon};
pub use message_box::{Buttons, MessageBox, MessageChoice};

/// The state of a modal dialog after handling an event.
pub enum DialogStatus<T> {
    /// The dialog is still open.
    Pending,
    /// The dialog closed. `None` means cancelled (Esc / Close / Cancel button).
    Done(Option<T>),
}

/// The outcome of the four-way [`Modal`] wrapper.
pub enum ModalOutcome {
    /// A path chosen from a [`FileDialog`].
    Path(String),
    /// A choice from a [`MessageBox`].
    Choice(MessageChoice),
    /// A color chosen from a [`ColorPicker`] (XRGB8888).
    Color(u32),
}

/// A convenience wrapper over the four dialog types so single-modal hosts hold
/// one `Option<Modal>` field and one dispatch arm.
///
/// Only one modal exists at a time, so keeping the variants inline avoids a
/// second allocation in these small native processes.
#[allow(clippy::large_enum_variant)]
pub enum Modal {
    File(FileDialog),
    Message(MessageBox),
    Color(ColorPicker),
}

impl Modal {
    pub fn window_handle(&self) -> u32 {
        match self {
            Modal::File(dialog) => dialog.window_handle(),
            Modal::Message(dialog) => dialog.window_handle(),
            Modal::Color(dialog) => dialog.window_handle(),
        }
    }

    pub fn handle_event(&mut self, event: &runtime::GuiEvent) -> DialogStatus<ModalOutcome> {
        match self {
            Modal::File(dialog) => map_status(dialog.handle_event(event), ModalOutcome::Path),
            Modal::Message(dialog) => map_status(dialog.handle_event(event), ModalOutcome::Choice),
            Modal::Color(dialog) => map_status(dialog.handle_event(event), ModalOutcome::Color),
        }
    }
}

fn map_status<T>(
    status: DialogStatus<T>,
    wrap: fn(T) -> ModalOutcome,
) -> DialogStatus<ModalOutcome> {
    match status {
        DialogStatus::Pending => DialogStatus::Pending,
        DialogStatus::Done(None) => DialogStatus::Done(None),
        DialogStatus::Done(Some(value)) => DialogStatus::Done(Some(wrap(value))),
    }
}

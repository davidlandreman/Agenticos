//! Extension-based file-open dispatch for the File Explorer.
//!
//! Pure logic — given a path, decide what to do. Keeps the action
//! decision separate from the actual `execute_command` / dialog
//! plumbing so it is trivially unit-testable.

extern crate alloc;

use alloc::string::String;

use super::dir_model::extract_extension;

/// What the Explorer should do when the user activates a file row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenAction {
    /// Spawn `notepad <path>` for editable text-like content.
    LaunchNotepad,
    /// Spawn `run <path>` for ELF userland binaries.
    LaunchRun,
    /// No registered handler for this extension. The String is the
    /// uppercase extension (with no leading dot, or empty for
    /// extensionless files) — used to compose the user-visible
    /// "No handler for type `.XYZ`" message.
    Unsupported(String),
}

/// Decide the open action for `path`. Pure function over the
/// uppercase extension; never inspects the filesystem.
pub fn dispatch_open(path: &str) -> OpenAction {
    let ext = extract_extension(path);
    match ext.as_str() {
        "TXT" | "MD" | "RS" => OpenAction::LaunchNotepad,
        "ELF" => OpenAction::LaunchRun,
        _ => OpenAction::Unsupported(ext),
    }
}

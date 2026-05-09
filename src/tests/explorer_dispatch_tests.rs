//! Tests for `src/commands/explorer/dispatch.rs`.
//!
//! Pure-logic coverage of extension -> `OpenAction` mapping. The
//! "another user app already running" guard is *not* exercised here
//! because it depends on live userland state; that path is covered
//! by the manual-smoke checklist in the plan.

extern crate alloc;

use alloc::string::String;

use crate::commands::explorer::dispatch::{dispatch_open, OpenAction};
use crate::lib::test_utils::Testable;

fn test_txt_launches_notepad() {
    assert_eq!(dispatch_open("/HELLO.TXT"), OpenAction::LaunchNotepad);
}

fn test_md_launches_notepad() {
    assert_eq!(dispatch_open("/README.MD"), OpenAction::LaunchNotepad);
}

fn test_rs_launches_notepad() {
    assert_eq!(dispatch_open("/MAIN.RS"), OpenAction::LaunchNotepad);
}

fn test_elf_launches_run() {
    assert_eq!(dispatch_open("/APP.ELF"), OpenAction::LaunchRun);
}

fn test_bmp_unsupported() {
    assert_eq!(
        dispatch_open("/IMAGE.BMP"),
        OpenAction::Unsupported(String::from("BMP"))
    );
}

fn test_extensionless_unsupported() {
    assert_eq!(
        dispatch_open("/MAKEFILE"),
        OpenAction::Unsupported(String::new())
    );
}

fn test_lowercase_normalizes_to_notepad() {
    // FAT exposes uppercase 8.3 names so lowercase paths shouldn't
    // appear from the live filesystem, but the dispatch is
    // case-insensitive on the extension to defend against future
    // mixed-case mounts.
    assert_eq!(dispatch_open("/lower.txt"), OpenAction::LaunchNotepad);
}

fn test_leading_dot_only_treated_as_extension() {
    // `.HIDDEN` has no other extension; the leading-dot text becomes
    // the extension. Acceptable for v1: there is no hidden-file
    // convention, and the user sees a clear MessageBox.
    assert_eq!(
        dispatch_open("/.HIDDEN"),
        OpenAction::Unsupported(String::from("HIDDEN"))
    );
}

fn test_empty_path_does_not_panic() {
    // Defensive: an empty path should map to extensionless ->
    // Unsupported(""), not panic.
    assert_eq!(
        dispatch_open(""),
        OpenAction::Unsupported(String::new())
    );
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_txt_launches_notepad,
        &test_md_launches_notepad,
        &test_rs_launches_notepad,
        &test_elf_launches_run,
        &test_bmp_unsupported,
        &test_extensionless_unsupported,
        &test_lowercase_normalizes_to_notepad,
        &test_leading_dot_only_treated_as_extension,
        &test_empty_path_does_not_panic,
    ]
}

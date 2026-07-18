//! Tests for `src/commands/explorer/dir_model.rs`.
//!
//! Covers extension parsing, display formatting, parent / child path
//! helpers, sort order, and a real-FS read of `/`.

extern crate alloc;

use alloc::string::String;

use crate::commands::explorer::dir_model::{
    child_path, extract_extension, format_size, format_type, parent_path, read_directory, DirEntry,
    EntryKind,
};
use crate::lib::test_utils::Testable;

fn folder(name: &str) -> DirEntry {
    DirEntry {
        name: String::from(name),
        size: 0,
        kind: EntryKind::Folder,
        full_path: child_path("/", name),
    }
}

fn file(name: &str, size: u64, ext: &str) -> DirEntry {
    DirEntry {
        name: String::from(name),
        size,
        kind: EntryKind::File {
            ext: String::from(ext),
        },
        full_path: child_path("/", name),
    }
}

fn test_extract_extension_text() {
    assert_eq!(extract_extension("HELLO.TXT"), "TXT");
}

fn test_extract_extension_empty_for_extensionless() {
    assert_eq!(extract_extension("MAKEFILE"), "");
}

fn test_extract_extension_uppercases() {
    assert_eq!(extract_extension("hello.txt"), "TXT");
}

fn test_extract_extension_leading_dot_only() {
    // A leading-dot-only name treats the rest as the extension. v1
    // has no hidden-file convention.
    assert_eq!(extract_extension(".HIDDEN"), "HIDDEN");
}

fn test_format_type_returns_dot_ext_for_files() {
    let f = file("HELLO.TXT", 10, "TXT");
    assert_eq!(format_type(&f), ".TXT");
}

fn test_format_type_returns_folder_for_folders() {
    assert_eq!(format_type(&folder("DOCS")), "Folder");
}

fn test_format_type_returns_file_for_extensionless() {
    let f = file("MAKEFILE", 0, "");
    assert_eq!(format_type(&f), "File");
}

fn test_format_size_dir() {
    assert_eq!(format_size(&folder("DOCS")), "<DIR>");
}

fn test_format_size_file_byte_count() {
    assert_eq!(format_size(&file("HELLO.TXT", 123, "TXT")), "123");
}

fn test_parent_path_root() {
    assert!(parent_path("/").is_none());
}

fn test_parent_path_root_child() {
    assert_eq!(parent_path("/HELLO.TXT").as_deref(), Some("/"));
}

fn test_parent_path_deep() {
    assert_eq!(parent_path("/A/B/C.TXT").as_deref(), Some("/A/B"));
}

fn test_parent_path_trailing_slash() {
    assert_eq!(parent_path("/A/B/").as_deref(), Some("/A"));
}

fn test_child_path_root_parent() {
    assert_eq!(child_path("/", "FOO.TXT"), "/FOO.TXT");
}

fn test_child_path_deep_parent() {
    assert_eq!(child_path("/A", "B"), "/A/B");
}

fn test_child_path_strips_trailing_slash() {
    assert_eq!(child_path("/A/", "B"), "/A/B");
}

fn test_read_directory_root_succeeds() {
    // Reading "/" against the live FAT mount should always succeed
    // and return a non-empty vector (the bundled BIOS image has at
    // least the wallpaper file).
    let entries = read_directory("/").expect("read_directory(\"/\") should succeed");
    assert!(
        !entries.is_empty(),
        "root directory should not be empty on the bundled image"
    );
}

fn test_read_directory_sort_folders_before_files() {
    // Synthesize the post-sort expectation against the live root
    // listing: every folder in the result must precede every file.
    let entries = read_directory("/").expect("read /");
    let mut seen_file = false;
    for e in &entries {
        match e.kind {
            EntryKind::Folder => {
                assert!(
                    !seen_file,
                    "folders must precede files in sort order, but folder `{}` came after a file",
                    e.name
                );
            }
            EntryKind::File { .. } => {
                seen_file = true;
            }
        }
    }
}

fn test_read_directory_nonexistent_returns_err() {
    let result = read_directory("/THIS/PATH/DOES/NOT/EXIST/XYZ");
    assert!(
        result.is_err(),
        "reading a nonexistent path should return an Err, got Ok"
    );
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_extract_extension_text,
        &test_extract_extension_empty_for_extensionless,
        &test_extract_extension_uppercases,
        &test_extract_extension_leading_dot_only,
        &test_format_type_returns_dot_ext_for_files,
        &test_format_type_returns_folder_for_folders,
        &test_format_type_returns_file_for_extensionless,
        &test_format_size_dir,
        &test_format_size_file_byte_count,
        &test_parent_path_root,
        &test_parent_path_root_child,
        &test_parent_path_deep,
        &test_parent_path_trailing_slash,
        &test_child_path_root_parent,
        &test_child_path_deep_parent,
        &test_child_path_strips_trailing_slash,
        &test_read_directory_root_succeeds,
        &test_read_directory_sort_folders_before_files,
        &test_read_directory_nonexistent_returns_err,
    ]
}

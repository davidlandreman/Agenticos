//! Directory traversal and entry formatting for the File Explorer.
//!
//! Wraps `crate::fs::Directory::open` with sorting (folders first,
//! alphabetical within each group) and pre-formatted display strings
//! (`Size` and `Type` columns) so the rest of the app sees a typed,
//! display-ready `Vec<DirEntry>`.

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::fs::file_handle::FileError;
use crate::fs::filesystem::FileType;
use crate::fs::Directory;

/// One row in the file list / one node in the tree.
#[derive(Debug, Clone)]
pub struct DirEntry {
    pub name: String,
    pub size: u64,
    pub kind: EntryKind,
    pub full_path: String,
}

/// Distinguishes folders from files; files carry their uppercase
/// extension (without the leading dot, e.g. `"TXT"`). The empty
/// string represents an extension-less file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryKind {
    Folder,
    File { ext: String },
}

/// Read a directory and return a sorted, display-ready vector of
/// entries. Sorting: folders first, then files; alphabetical within
/// each group (case-insensitive, but FAT names come back uppercase).
pub fn read_directory(path: &str) -> Result<Vec<DirEntry>, FileError> {
    let dir = Directory::open(path)?;
    let raw = dir.entries();

    let mut out: Vec<DirEntry> = Vec::with_capacity(raw.len());
    for entry in raw {
        let name = String::from(entry.name_str());
        // FAT exposes `.` and `..` pseudo-entries on subdirectories;
        // skip them — the Explorer's Up button is the supported way
        // to ascend, and showing `.` / `..` clutters the list.
        if name == "." || name == ".." {
            continue;
        }
        let is_folder = entry.file_type == FileType::Directory;
        let kind = if is_folder {
            EntryKind::Folder
        } else {
            EntryKind::File {
                ext: extract_extension(&name),
            }
        };
        let full_path = child_path(path, &name);
        let size = entry.size as u64;
        out.push(DirEntry {
            name,
            size,
            kind,
            full_path,
        });
    }

    out.sort_by(|a, b| {
        let a_folder = matches!(a.kind, EntryKind::Folder);
        let b_folder = matches!(b.kind, EntryKind::Folder);
        match (a_folder, b_folder) {
            (true, false) => core::cmp::Ordering::Less,
            (false, true) => core::cmp::Ordering::Greater,
            _ => a.name.cmp(&b.name),
        }
    });

    Ok(out)
}

/// Format the Size column for display. Folders render as `<DIR>`;
/// files render as a decimal byte count.
pub fn format_size(entry: &DirEntry) -> String {
    match entry.kind {
        EntryKind::Folder => String::from("<DIR>"),
        EntryKind::File { .. } => format!("{}", entry.size),
    }
}

/// Format the Type column for display. Folders render as `Folder`;
/// extensionless files render as `File`; files with an extension
/// render as `.EXT`.
pub fn format_type(entry: &DirEntry) -> String {
    match &entry.kind {
        EntryKind::Folder => String::from("Folder"),
        EntryKind::File { ext } if ext.is_empty() => String::from("File"),
        EntryKind::File { ext } => {
            let mut s = String::with_capacity(ext.len() + 1);
            s.push('.');
            s.push_str(ext);
            s
        }
    }
}

/// Compute the parent directory of a path. Root (`"/"`) has no parent.
pub fn parent_path(path: &str) -> Option<String> {
    if path == "/" {
        return None;
    }
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) => Some(String::from("/")),
        Some(idx) => Some(trimmed[..idx].to_string()),
        None => Some(String::from("/")),
    }
}

/// Join a parent path with a child name, collapsing duplicate slashes.
pub fn child_path(parent: &str, name: &str) -> String {
    if parent == "/" {
        let mut s = String::with_capacity(1 + name.len());
        s.push('/');
        s.push_str(name);
        s
    } else {
        let p = parent.trim_end_matches('/');
        let mut s = String::with_capacity(p.len() + 1 + name.len());
        s.push_str(p);
        s.push('/');
        s.push_str(name);
        s
    }
}

/// Return the uppercase extension of a filename (without the leading
/// dot). For `MAKEFILE` -> `""`. For `.HIDDEN` -> `"HIDDEN"` (a
/// leading-dot-only name is treated as the extension; v1 has no
/// hidden-file convention).
pub fn extract_extension(name: &str) -> String {
    match name.rfind('.') {
        None => String::new(),
        Some(idx) => {
            let ext = &name[idx + 1..];
            // Uppercase ASCII for case-insensitive matching against the
            // dispatch table. FAT 8.3 names are already uppercase; this
            // defends against future mixed-case mounts.
            let mut out = String::with_capacity(ext.len());
            for ch in ext.chars() {
                out.push(ch.to_ascii_uppercase());
            }
            out
        }
    }
}

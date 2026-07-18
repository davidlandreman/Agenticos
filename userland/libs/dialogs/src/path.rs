//! Path utilities shared by the dialogs (promoted from notepad's app-private
//! helpers). All operate on absolute POSIX-ish paths.

use alloc::format;
use alloc::string::String;

/// The parent of `path` (drops the last component). Root maps to itself.
pub fn parent_directory(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) | None => "/",
        Some(index) => &trimmed[..index],
    }
}

/// The directory a free-form path input refers to: the path itself when it
/// ends in `/`, otherwise its parent.
pub fn directory_for_input(path: &str) -> &str {
    if path.ends_with('/') {
        let directory = path.trim_end_matches('/');
        if directory.is_empty() {
            "/"
        } else {
            directory
        }
    } else {
        parent_directory(path)
    }
}

/// Join `name` onto `directory`, appending a trailing `/` for directories.
pub fn join_path(directory: &str, name: &str, is_dir: bool) -> String {
    let mut path = if directory == "/" {
        format!("/{name}")
    } else {
        format!("{directory}/{name}")
    };
    if is_dir {
        path.push('/');
    }
    path
}

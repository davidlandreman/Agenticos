//! Path utilities for ring-3 file syscalls.
//!
//! - `copy_user_cstr` — pull a NUL-terminated string out of user space,
//!   bounded by `PATH_MAX` and validated against the active user-VA
//!   window. Used by every path-taking syscall.
//! - `normalize_path` — resolve a user-supplied path against the
//!   process's current working directory. Handles `.` / `..` / repeated
//!   slashes; the result always begins with `/`.

use crate::userland::abi::{EFAULT, ENAMETOOLONG};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Linux convention. Paths longer than this are rejected with
/// `-ENAMETOOLONG` rather than truncated.
pub const PATH_MAX: usize = 4096;

/// Cap on argv/envp entry count. Linux's `_SC_ARG_MAX` is 32 KiB; we
/// pick a smaller bound that's still comfortable for shells.
pub const MAX_VECTOR_ENTRIES: usize = 256;

/// Copy a NULL-terminated user array of C-string pointers (e.g.
/// argv, envp) into a `Vec<String>`. Each pointer is read as a
/// 64-bit word from user memory; the array terminates at the first
/// NULL pointer. Returns `-EFAULT` for bad pointers, `-EINVAL` if
/// the array exceeds `MAX_VECTOR_ENTRIES`.
///
/// `arr_ptr == 0` is treated as an empty vector — same as Linux when
/// the syscall caller passes NULL.
pub fn copy_user_cstr_array(arr_ptr: u64) -> Result<alloc::vec::Vec<String>, i64> {
    use crate::userland::abi::{EFAULT, EINVAL};
    if arr_ptr == 0 {
        return Ok(alloc::vec::Vec::new());
    }
    let mut out: alloc::vec::Vec<String> = alloc::vec::Vec::new();
    for i in 0..MAX_VECTOR_ENTRIES {
        let entry_ptr = arr_ptr.checked_add((i * 8) as u64).ok_or(EFAULT)?;
        let str_ptr = crate::userland::usercopy::read_unaligned::<u64>(entry_ptr)?;
        if str_ptr == 0 {
            return Ok(out);
        }
        let s = copy_user_cstr(str_ptr)?;
        out.push(s);
    }
    Err(EINVAL)
}

/// Read a NUL-terminated user string starting at `ptr` into kernel
/// memory. Validates each byte against the active user-VA bounds; an
/// out-of-range byte returns `-EFAULT` before any read fault could
/// fire. Strings longer than `PATH_MAX` are rejected with
/// `-ENAMETOOLONG`. Non-UTF-8 bytes are rejected as `-EFAULT` (the
/// kernel's path resolver expects valid UTF-8).
pub fn copy_user_cstr(ptr: u64) -> Result<String, i64> {
    if ptr == 0 {
        return Err(EFAULT);
    }
    let mut bytes: Vec<u8> = Vec::new();
    for i in 0..PATH_MAX {
        let addr = ptr.checked_add(i as u64).ok_or(EFAULT)?;
        let b = crate::userland::usercopy::read_unaligned::<u8>(addr)?;
        if b == 0 {
            return String::from_utf8(bytes).map_err(|_| EFAULT);
        }
        bytes.push(b);
    }
    Err(ENAMETOOLONG)
}

/// Resolve `path` against `cwd`. Absolute paths ignore `cwd`; relative
/// paths are anchored at `cwd`. The result is normalized:
/// - leading/repeated `/` collapse,
/// - `.` segments are dropped,
/// - `..` segments pop the previous component (clamped at root),
/// - the result always starts with `/`.
pub fn normalize_path(cwd: &str, path: &str) -> String {
    let raw = if path.starts_with('/') {
        path.to_string()
    } else {
        let mut s = String::with_capacity(cwd.len() + 1 + path.len());
        s.push_str(cwd.trim_end_matches('/'));
        if s.is_empty() {
            s.push('/');
        }
        s.push('/');
        s.push_str(path);
        s
    };
    let mut stack: Vec<&str> = Vec::new();
    for part in raw.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            other => stack.push(other),
        }
    }
    if stack.is_empty() {
        String::from("/")
    } else {
        let mut out = String::new();
        for p in &stack {
            out.push('/');
            out.push_str(p);
        }
        out
    }
}

#[cfg(feature = "test")]
mod tests_internal {
    use super::*;

    pub fn test_normalize_absolute_keeps_path() {
        assert_eq!(normalize_path("/host", "/etc/passwd"), "/etc/passwd");
    }

    pub fn test_normalize_relative_anchors_at_cwd() {
        assert_eq!(normalize_path("/host", "foo.txt"), "/host/foo.txt");
        assert_eq!(normalize_path("/", "foo.txt"), "/foo.txt");
    }

    pub fn test_normalize_collapses_redundancy() {
        assert_eq!(normalize_path("/host", "./a/./b//c"), "/host/a/b/c");
        assert_eq!(normalize_path("/host", "a/../b"), "/host/b");
        assert_eq!(normalize_path("/host", "../.."), "/");
        assert_eq!(normalize_path("/", "../foo"), "/foo");
    }

    pub fn test_normalize_root_idempotent() {
        assert_eq!(normalize_path("/host", "/"), "/");
        assert_eq!(normalize_path("/host", "."), "/host");
    }

    pub fn test_etc_paths_normalize_into_runtime_namespace() {
        let normalized = normalize_path("/host", "/etc/../etc/shadow");
        assert_eq!(normalized, "/etc/shadow");
    }

    pub fn get_tests() -> &'static [&'static dyn crate::lib::test_utils::Testable] {
        &[
            &test_normalize_absolute_keeps_path,
            &test_normalize_relative_anchors_at_cwd,
            &test_normalize_collapses_redundancy,
            &test_normalize_root_idempotent,
            &test_etc_paths_normalize_into_runtime_namespace,
        ]
    }
}

#[cfg(feature = "test")]
pub use tests_internal::get_tests as path_tests;

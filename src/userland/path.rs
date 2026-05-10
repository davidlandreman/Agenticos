//! Path utilities for ring-3 file syscalls.
//!
//! - `copy_user_cstr` — pull a NUL-terminated string out of user space,
//!   bounded by `PATH_MAX` and validated against the active user-VA
//!   window. Used by every path-taking syscall.
//! - `normalize_path` — resolve a user-supplied path against the
//!   process's current working directory. Handles `.` / `..` / repeated
//!   slashes; the result always begins with `/`.

use crate::userland::abi::{user_va_bounds, EFAULT, ENAMETOOLONG};
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
    use crate::userland::abi::{user_va_bounds, EFAULT, EINVAL};
    if arr_ptr == 0 {
        return Ok(alloc::vec::Vec::new());
    }
    let bounds = user_va_bounds().ok_or(EFAULT)?;
    let mut out: alloc::vec::Vec<String> = alloc::vec::Vec::new();
    for i in 0..MAX_VECTOR_ENTRIES {
        let entry_ptr = arr_ptr.checked_add((i * 8) as u64).ok_or(EFAULT)?;
        let end = entry_ptr.checked_add(8).ok_or(EFAULT)?;
        if entry_ptr < bounds.start || end > bounds.end {
            return Err(EFAULT);
        }
        let str_ptr = unsafe { core::ptr::read_unaligned(entry_ptr as *const u64) };
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
    let bounds = user_va_bounds().ok_or(EFAULT)?;
    if ptr < bounds.start {
        return Err(EFAULT);
    }
    let mut bytes: Vec<u8> = Vec::new();
    for i in 0..PATH_MAX {
        let addr = ptr.checked_add(i as u64).ok_or(EFAULT)?;
        if addr >= bounds.end {
            return Err(EFAULT);
        }
        let b = unsafe { core::ptr::read_volatile(addr as *const u8) };
        if b == 0 {
            return String::from_utf8(bytes).map_err(|_| EFAULT);
        }
        bytes.push(b);
    }
    Err(ENAMETOOLONG)
}

/// Apply the U4 `/etc/...` allowlist rewrite to a *normalized* path.
/// Caller MUST have already run `normalize_path` so `..` segments are
/// collapsed — a path like `/etc/../etc/shadow` would otherwise match
/// the prefix here and rewrite surprisingly. The security finding from
/// the doc-review pass made this ordering a hard requirement: never
/// run the rewrite on the raw user string.
///
/// Today's allowlist is just `/etc/...` → `/host/etc/...`. This makes
/// musl's `getpwuid_r` find `/etc/passwd` (staged at
/// `host_share/ETC/PASSWD` by build.sh / test.sh). Other paths are
/// returned unchanged.
///
/// Known cosmetic limitation: `chdir("/etc")` rewrites to
/// `chdir("/host/etc")`, so the user-visible cwd becomes `/host/etc`.
/// Acceptable for Phase A-C; a real `/etc` VFS mount is the long-term
/// fix.
pub fn apply_fs_rewrite(normalized: &str) -> String {
    if normalized == "/etc" {
        return String::from("/host/etc");
    }
    if let Some(rest) = normalized.strip_prefix("/etc/") {
        let mut out = String::with_capacity("/host/etc/".len() + rest.len());
        out.push_str("/host/etc/");
        out.push_str(rest);
        return out;
    }
    String::from(normalized)
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

    // ---------- U4: /etc rewrite ----------

    pub fn test_etc_rewrite_passwd() {
        assert_eq!(apply_fs_rewrite("/etc/passwd"), "/host/etc/passwd");
        assert_eq!(apply_fs_rewrite("/etc/group"), "/host/etc/group");
    }

    pub fn test_etc_rewrite_bare_etc() {
        // /etc by itself rewrites to /host/etc — `cd /etc` then `pwd`
        // would land at /host/etc (acceptable cosmetic limitation).
        assert_eq!(apply_fs_rewrite("/etc"), "/host/etc");
    }

    pub fn test_etc_rewrite_pass_through() {
        // Non-/etc paths are unchanged.
        assert_eq!(apply_fs_rewrite("/host/zsh.elf"), "/host/zsh.elf");
        assert_eq!(apply_fs_rewrite("/"), "/");
        assert_eq!(apply_fs_rewrite("/etcetera/foo"), "/etcetera/foo"); // not /etc/
    }

    pub fn test_etc_rewrite_after_normalize_collapses_traversal() {
        // The security-critical ordering: normalize_path collapses ..
        // segments first, so /etc/../etc/shadow → /etc/shadow → rewrite
        // to /host/etc/shadow (which doesn't exist on the FAT mount,
        // returning ENOENT as expected). Without normalize_path first
        // the raw string would still match /etc/ and rewrite to
        // /host/etc/../etc/shadow.
        let normalized = normalize_path("/host", "/etc/../etc/shadow");
        assert_eq!(normalized, "/etc/shadow");
        assert_eq!(apply_fs_rewrite(&normalized), "/host/etc/shadow");
    }

    pub fn get_tests() -> &'static [&'static dyn crate::lib::test_utils::Testable] {
        &[
            &test_normalize_absolute_keeps_path,
            &test_normalize_relative_anchors_at_cwd,
            &test_normalize_collapses_redundancy,
            &test_normalize_root_idempotent,
            &test_etc_rewrite_passwd,
            &test_etc_rewrite_bare_etc,
            &test_etc_rewrite_pass_through,
            &test_etc_rewrite_after_normalize_collapses_traversal,
        ]
    }
}

#[cfg(feature = "test")]
pub use tests_internal::get_tests as path_tests;

//! Test filter — selects which kernel tests run under `./test.sh`.
//!
//! The filter string is read at boot via `fw_cfg` (file
//! `opt/agenticos/test_filter`), set by `test.sh` from positional CLI args.
//! Pre-heap safe: parsing operates on a fixed static buffer with `&str`
//! slicing only.
//!
//! Syntax: comma-separated patterns, matched against `<module>` or
//! `<module>::<fn>`. Each pattern supports `*` as a leading and/or trailing
//! wildcard:
//!   - `arc`              — exact module match
//!   - `arc::test_weak*`  — prefix glob within a module
//!   - `*scroll*`         — substring anywhere in `<module>::<fn>`
//! Empty / unset filter runs everything.

use crate::debug_info;
use crate::drivers::fw_cfg;

const FW_CFG_PATH: &str = "opt/agenticos/test_filter";
const BUF_LEN: usize = 256;

static mut FILTER_BUF: [u8; BUF_LEN] = [0; BUF_LEN];
static mut FILTER_LEN: usize = 0;

/// Read the filter string from fw_cfg into a static buffer.
///
/// Idempotent and silent when fw_cfg is absent or the file is missing
/// (in either case, no filter is set → all tests run).
pub fn init() {
    let len = unsafe {
        let buf = &mut *core::ptr::addr_of_mut!(FILTER_BUF);
        match fw_cfg::read_file(FW_CFG_PATH, buf) {
            Some(n) => n,
            None => 0,
        }
    };

    // Trim trailing NULs / whitespace so QEMU's NUL-terminated `string=` value
    // and any accidental trailing newline don't break matching.
    let trimmed = unsafe {
        let buf = &*core::ptr::addr_of!(FILTER_BUF);
        let slice = &buf[..len];
        let end = slice
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(slice.len());
        let mut e = end;
        while e > 0 && matches!(slice[e - 1], b' ' | b'\t' | b'\r' | b'\n') {
            e -= 1;
        }
        e
    };

    unsafe {
        FILTER_LEN = trimmed;
    }

    if trimmed > 0 {
        if let Some(s) = filter_str() {
            debug_info!("Test filter: {:?}", s);
        }
    }
}

/// Returns the active filter string, or `None` when no filter is set.
pub fn filter_str() -> Option<&'static str> {
    let len = unsafe { FILTER_LEN };
    if len == 0 {
        return None;
    }
    let buf = unsafe { &*core::ptr::addr_of!(FILTER_BUF) };
    core::str::from_utf8(&buf[..len]).ok()
}

/// True when no filter is active (i.e., run everything).
pub fn is_empty() -> bool {
    filter_str().is_none()
}

/// Match a single test against the filter.
///
/// `module` is the registry name (e.g. `"arc"`); `full_name` is
/// `"<module>::<fn>"` (e.g. `"arc::test_weak_basic"`). Returns true when no
/// filter is set.
pub fn matches(module: &str, full_name: &str) -> bool {
    let filter = match filter_str() {
        Some(s) => s,
        None => return true,
    };

    for pat in filter.split(',') {
        let pat = pat.trim();
        if pat.is_empty() {
            continue;
        }
        if matches_pattern(pat, module) || matches_pattern(pat, full_name) {
            return true;
        }
    }
    false
}

/// Glob-match a single pattern. Supports `*` as a leading/trailing wildcard.
/// Patterns without `*` must match exactly.
fn matches_pattern(pat: &str, target: &str) -> bool {
    let starts_wild = pat.starts_with('*');
    let ends_wild = pat.ends_with('*');
    let core = {
        let mut s = pat;
        if starts_wild {
            s = &s[1..];
        }
        if ends_wild && !s.is_empty() {
            s = &s[..s.len() - 1];
        }
        s
    };

    if core.is_empty() {
        // Pattern was "*" or "**" — matches anything non-empty.
        return !target.is_empty();
    }

    match (starts_wild, ends_wild) {
        (false, false) => target == core,
        (false, true) => target.starts_with(core),
        (true, false) => target.ends_with(core),
        (true, true) => target.contains(core),
    }
}

#[cfg(feature = "test")]
pub fn get_tests() -> &'static [&'static dyn crate::lib::test_utils::Testable] {
    &[
        &test_pattern_exact,
        &test_pattern_prefix,
        &test_pattern_suffix,
        &test_pattern_substring,
        &test_pattern_star_only,
    ]
}

#[cfg(feature = "test")]
fn test_pattern_exact() {
    assert!(matches_pattern("arc", "arc"));
    assert!(!matches_pattern("arc", "arcadia"));
}

#[cfg(feature = "test")]
fn test_pattern_prefix() {
    assert!(matches_pattern("arc::test_weak*", "arc::test_weak_basic"));
    assert!(!matches_pattern("arc::test_weak*", "arc::test_strong"));
}

#[cfg(feature = "test")]
fn test_pattern_suffix() {
    assert!(matches_pattern("*basic", "arc::test_weak_basic"));
    assert!(!matches_pattern("*basic", "arc::test_weak"));
}

#[cfg(feature = "test")]
fn test_pattern_substring() {
    assert!(matches_pattern("*weak*", "arc::test_weak_basic"));
    assert!(!matches_pattern("*weak*", "arc::test_strong"));
}

#[cfg(feature = "test")]
fn test_pattern_star_only() {
    assert!(matches_pattern("*", "anything"));
    assert!(!matches_pattern("*", ""));
}

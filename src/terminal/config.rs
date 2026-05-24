//! Compile-time configuration for the terminal subsystem.
//!
//! These knobs are intentionally `const` and not runtime-tunable. The plan
//! (`docs/plans/2026-05-24-001-feat-terminal-ansi-vt-pty-and-caret-plan.md`)
//! commits to compile-time configuration so the trade-offs are inspectable
//! during code review and visible in `git blame`.

/// Maximum number of scrolled-off lines retained per primary buffer.
/// 5000 × 200 cols × ~8 bytes/cell ≈ 8 MiB worst case per terminal — fine
/// on a 128 MiB system with one or two terminals open.
pub const SCROLLBACK_LINES: usize = 5000;

/// Half-period of the caret blink in milliseconds. The compositor flips the
/// blink phase once every BLINK_INTERVAL_MS; 500 ms gives the familiar
/// once-per-second on/off cycle.
pub const BLINK_INTERVAL_MS: u64 = 500;

/// Default grid dimensions if no window has attached yet. zsh / vi consult
/// `TIOCGWINSZ` to decide where to wrap; 80×24 is the universally-safe
/// default and matches the previous global `Winsize`.
pub const DEFAULT_COLS: u16 = 80;
pub const DEFAULT_ROWS: u16 = 24;

/// Horizontal-tab stop width. Hardcoded eight matches the VT100 default and
/// what every terminfo entry assumes.
pub const TAB_WIDTH: usize = 8;

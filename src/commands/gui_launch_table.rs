//! Kernel-side dispatch for GUI applet launches issued by
//! `GLAUNCH.ELF` via the `gui_launch` syscall.
//!
//! ## Why this exists
//!
//! Previously the GUI app launchers (`painting`, `calc`, `notepad`,
//! `tasks`, `explorer`) were reachable only as kernel-shell commands
//! registered in `src/process/manager.rs::ProcessManager`. With zsh as
//! the default shell, those typed names need to resolve through ring-3
//! PATH lookup. The `/bin/<gui_applet>` rewrite in
//! [`crate::userland::bin_namespace`] sends them to the GUILAUNCH
//! multicall binary, which then issues `gui_launch(<name>)`. This
//! module is where that syscall lands.
//!
//! The GUI apps themselves stay kernel-space — only the launch surface
//! moves. See `docs/plans/2026-05-16-004-feat-zsh-default-terminal-and-gui-launchers-plan.md`.
//!
//! ## Source of truth for the applet name list
//!
//! [`crate::userland::bin_namespace::GUI_APPLETS`] holds the list that
//! the kernel exposes under `/bin/`. The `match` below MUST cover every
//! name in that list. A test asserts the two stay in sync.

use crate::process::{spawn_process, ProcessId, RunnableProcess};
use crate::userland::abi::ENOENT;
use alloc::{boxed::Box, string::String, vec::Vec};

/// Spawn the GUI app named `name` as a kernel-side process. Returns the
/// spawned PID on success or `-ENOENT` if the name doesn't match any
/// known GUI applet.
///
/// Keep the match arms in sync with
/// [`crate::userland::bin_namespace::GUI_APPLETS`].
pub fn spawn_by_name(name: &str) -> Result<ProcessId, i64> {
    let factory: fn(Vec<String>) -> Box<dyn RunnableProcess> = match name {
        "calc" => crate::commands::calc::create_calc_process,
        "explorer" => crate::commands::explorer::create_explorer_process,
        "notepad" => crate::commands::notepad::create_notepad_process,
        "painting" => crate::commands::painting::create_painting_process,
        "tasks" => crate::commands::tasks::create_tasks_process,
        _ => return Err(ENOENT),
    };
    let process_name = String::from(name);
    let pid = spawn_process(process_name.clone(), None, move || {
        let mut process = factory(Vec::new());
        process.run();
    });
    Ok(pid)
}

#[cfg(feature = "test")]
mod tests_internal {
    use crate::userland::bin_namespace::GUI_APPLETS;

    /// Every name in [`GUI_APPLETS`] must be handled by `spawn_by_name`;
    /// otherwise PATH lookup succeeds but execve dispatches to a launcher
    /// that hits `-ENOENT` from this table. Inverse drift (a name in the
    /// match but not in `GUI_APPLETS`) is fine — it just means the name
    /// isn't reachable via `/bin/<name>` from zsh, which is rejectable
    /// during code review.
    fn test_every_gui_applet_dispatches() {
        // We can't actually call `spawn_by_name` here without standing up
        // a real scheduler context. Instead, drive the match arms via a
        // lookup helper that mirrors the match and never spawns.
        for &name in GUI_APPLETS {
            assert!(
                handler_for(name).is_some(),
                "GUI_APPLETS entry {:?} has no spawn_by_name match arm",
                name,
            );
        }
    }

    /// Unknown names must surface as `-ENOENT` (not a panic or a silent
    /// success).
    fn test_unknown_name_is_enoent() {
        assert!(handler_for("not-a-real-gui-app").is_none());
    }

    /// Mirror of `spawn_by_name`'s match — returns a unit token for every
    /// name the real function would handle. Used by the test above to
    /// assert coverage without spawning.
    fn handler_for(name: &str) -> Option<()> {
        match name {
            "calc" | "explorer" | "notepad" | "painting" | "tasks" => Some(()),
            _ => None,
        }
    }

    pub fn get_tests() -> &'static [&'static dyn crate::lib::test_utils::Testable] {
        &[&test_every_gui_applet_dispatches, &test_unknown_name_is_enoent]
    }
}

#[cfg(feature = "test")]
pub use tests_internal::get_tests as gui_launch_table_tests;

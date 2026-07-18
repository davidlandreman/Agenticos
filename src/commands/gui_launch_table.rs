//! Kernel-side dispatch for GUI applet launches issued by
//! `GLAUNCH.ELF` via the `gui_launch` syscall.
//!
//! ## Why this exists
//!
//! Kernel-side GUI apps were previously reachable only as kernel-shell
//! commands registered in `src/process/manager.rs::ProcessManager`.
//! With zsh as the default shell, those typed names resolve through
//! ring-3 PATH lookup: the `/bin/<gui_applet>` rewrite in
//! [`crate::userland::bin_namespace`] sends them to the GUILAUNCH
//! multicall binary, which then issues `gui_launch(<name>)`. This
//! module is where that syscall lands.
//!
//! **The applet table is empty today** — every GUI app (File Manager,
//! Notepad, Calc, Painting, GL Arena, Task Manager) has migrated to a
//! standalone ring-3 ELF with a direct `/bin` rewrite. The dispatch
//! skeleton stays so a future workload that genuinely requires ring-0
//! privileges can add an arm; keep any new arm in sync with
//! [`crate::userland::bin_namespace::GUI_APPLETS`] — a test asserts
//! the two stay in sync.

use crate::process::ProcessId;
use crate::userland::abi::ENOENT;

/// Spawn the GUI app named `name` as a kernel-side process. Returns the
/// spawned PID on success or `-ENOENT` if the name doesn't match any
/// known GUI applet — which today is every name, since the table is
/// empty. When re-adding an arm, restore the `spawn_process` +
/// factory-closure shape that the pre-migration table used (see git
/// history) and list the name in `GUI_APPLETS`.
pub fn spawn_by_name(name: &str) -> Result<ProcessId, i64> {
    let _ = name;
    Err(ENOENT)
}

#[cfg(feature = "test")]
mod tests_internal {
    use crate::userland::bin_namespace::GUI_APPLETS;

    /// Every name in [`GUI_APPLETS`] must be handled by `spawn_by_name`;
    /// otherwise PATH lookup succeeds but execve dispatches to a launcher
    /// that hits `-ENOENT` from this table. With the table empty, the
    /// invariant is that `GUI_APPLETS` is empty too.
    fn test_every_gui_applet_dispatches() {
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
        assert_eq!(
            crate::commands::gui_launch_table::spawn_by_name("not-a-real-gui-app"),
            Err(crate::userland::abi::ENOENT),
        );
    }

    /// Mirror of `spawn_by_name`'s match — returns a unit token for every
    /// name the real function would handle. Used by the test above to
    /// assert coverage without spawning.
    fn handler_for(_name: &str) -> Option<()> {
        None
    }

    pub fn get_tests() -> &'static [&'static dyn crate::lib::test_utils::Testable] {
        &[
            &test_every_gui_applet_dispatches,
            &test_unknown_name_is_enoent,
        ]
    }
}

#[cfg(feature = "test")]
pub use tests_internal::get_tests as gui_launch_table_tests;

//! Generic user-binary launcher — extracted from `src/commands/run/mod.rs`
//! so the same code path can launch zsh as the default terminal shell
//! (`src/window/terminal_factory.rs`) without depending on the
//! soon-to-be-deleted `run` shell command.
//!
//! See `docs/plans/2026-05-16-004-feat-zsh-default-terminal-and-gui-launchers-plan.md`.
//!
//! This is the synchronous launch path: callers block in
//! [`launch_user_binary`] until the user process exits (cooperative
//! `exit_group`, fault, or unimplemented syscall). It enforces D5
//! (single user app) the same way `run` did — a concurrent attempt
//! returns `Err(_)`.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::userland::lifecycle::ExitKind;

/// Largest user binary the loader will accept. Mirrors the cap in the
/// old `run` command (16 MiB) — enough headroom for a static
/// `libstdc++` C++ binary plus zsh, fails loudly before the heap
/// allocator panics on an oversized read.
const MAX_USER_BINARY_BYTES: u64 = 16 * 1024 * 1024;

/// Load `path` from the FAT mount and enter ring 3. `argv` becomes the
/// user process's `argv` (typically `[path]`); `envp` becomes its
/// `envp`.
///
/// Returns `Ok((kind, code))` on normal completion (which includes
/// faulting exits — they're observable, not propagated). Returns
/// `Err(msg)` if the launch itself fails (file missing, ELF malformed,
/// D5 violation, address-space allocation failure).
///
/// Blocks until the user process exits.
pub fn launch_user_binary(
    path: &str,
    argv: &[&str],
    envp: &[&str],
) -> Result<(ExitKind, i64), String> {
    if crate::userland::lifecycle::user_active() {
        return Err("another user app is already running".to_string());
    }

    // Each launch gets a fresh "seen syscalls" table so trace-mode
    // logging is meaningful per-binary rather than stale.
    crate::userland::abi::reset_unknown_syscall_trace();

    let aspace = crate::userland::address_space::AddressSpace::new()
        .map_err(|e| format!("AddressSpace::new: {:?}", e))?;
    // SAFETY: AddressSpace::new copies the kernel half from the kernel
    // L4, so kernel code after the CR3 write is still mapped.
    unsafe { aspace.activate(); }

    // Scope the load guard to file read + ELF parse + page mapping so
    // GUI / render housekeeping resumes as soon as ring-3 starts
    // executing. Dropping the guard before `enter_user_mode_with_aspace`
    // is deliberate (see
    // `docs/solutions/learnings/2026-05-09-multi-mib-user-binary-load.md`).
    let image = {
        let _load_guard = crate::userland::lifecycle::BinaryLoadGuard::enter();
        let bytes = read_file_bytes(path)?;
        crate::userland::loader::load_elf(&bytes)
            .map_err(|e| format!("loader error: {:?}", e))?
    };

    let result = crate::userland::enter_user_mode_with_aspace(image, argv, envp, Some(aspace))
        .map_err(|e| format!("enter_user_mode: {:?}", e))?;

    // Drop the active image (unmaps + frees user VA) and the address
    // space it ran on. `release_active_image` returns the AddressSpace
    // the process slot owned; its Drop impl restores CR3 to the kernel
    // L4 if it was still active.
    let (_img, _aspace) = crate::userland::release_active_image();

    Ok(result)
}

fn read_file_bytes(path: &str) -> Result<Vec<u8>, String> {
    use crate::fs::File;
    let file = File::open_read(path).map_err(|e| format!("open '{}': {}", path, e))?;
    let size = file.size();
    if size > MAX_USER_BINARY_BYTES {
        return Err(format!(
            "binary '{}' is {} bytes; max {} bytes ({} MiB)",
            path,
            size,
            MAX_USER_BINARY_BYTES,
            MAX_USER_BINARY_BYTES / (1024 * 1024)
        ));
    }
    file.read_to_vec()
        .map_err(|e| format!("read '{}': {}", path, e))
}

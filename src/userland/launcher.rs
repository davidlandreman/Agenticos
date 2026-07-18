//! Generic user-binary launcher — extracted from `src/commands/run/mod.rs`
//! so the same code path can launch zsh as the default terminal shell
//! (`src/window/terminal_factory.rs`) without depending on the
//! soon-to-be-deleted `run` shell command.
//!
//! See `docs/plans/2026-05-16-004-feat-zsh-default-terminal-and-gui-launchers-plan.md`.
//!
//! Production callers go through `process_service::submit`: one persistent
//! worker calls [`prepare_user_binary_unstarted`], publishes ownership, and
//! marks the new PID ready without waiting for its lifetime. The synchronous
//! [`launch_user_binary`] path remains only for QEMU fixtures that need to
//! drive a user binary inline and inspect its returned status.
//!
//! ## Concurrency invariant
//!
//! `aspace.activate() → load_elf → setup_user_process` MUST run
//! atomically wrt other launchers, because each step uses the active
//! CR3 implicitly (the loader's `map_user_region` writes into the
//! current L4; `build_initial_stack` writes user pages of the current
//! L4). [`BINARY_SETUP_MUTEX`] enforces this. The mutex is dropped
//! before [`wait_for_ring3_exit`](crate::userland::wait_for_ring3_exit)
//! so the actual ring-3 lifetime is NOT serialized — only the fast
//! setup phase is.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::userland::lifecycle::ExitKind;

/// U8 serialization: protects address-space setup and teardown from concurrent
/// launchers. Without it,
/// thread A's `aspace.activate()` makes CR3 point at A's L4; if
/// thread B preempts and activates B's L4, A's subsequent
/// `map_user_region` calls (inside `load_elf`) write mappings into
/// B's L4 — corrupting both processes.
///
/// The mutex is dropped **before** `wait_for_ring3_exit` (which
/// blocks on the kernel-thread scheduler) so concurrent launchers
/// serialize only their fast setup phase, not their entire ring-3
/// lifetime.
static BINARY_SETUP_MUTEX: Mutex<()> = Mutex::new(());

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
#[cfg_attr(
    not(feature = "test"),
    expect(dead_code, reason = "QEMU test compatibility API")
)]
pub fn launch_user_binary(
    path: &str,
    argv: &[&str],
    envp: &[&str],
) -> Result<(ExitKind, i64), String> {
    // U8: no more single-app guard. Multiple concurrent launches are
    // supported; each gets its own Process, address space, and
    // kernel-thread block until its child exits.

    let pid = prepare_user_binary_unstarted(path, argv, envp, None)?;
    crate::userland::lifecycle::mark_ring3_ready(pid);

    let result = crate::userland::wait_for_ring3_exit(pid);

    // Teardown also mutates the global mapper and may restore CR3. Serialize
    // it against another launch transaction, and ask the compositor to skip
    // allocation/render work while the ownership objects are dropped. Do not
    // suppress kernel-thread preemption here: destroying a sparse address
    // space can take long enough to make the desktop appear frozen.
    {
        let _setup_guard = BINARY_SETUP_MUTEX.lock();
        // `wait_for_ring3_exit` sets this too, but it is global scheduler
        // state and another ring-3 slice may have changed it before this
        // launcher reacquired the setup mutex. Reassert the exact child before
        // cleanup so it cannot remove another process.
        crate::userland::lifecycle::set_current_user_pid(Some(pid));
        let (image, address_space) = crate::userland::release_active_image();
        drop(image);
        drop(address_space);
    }

    Ok(result)
}

/// Load and fully install a user process without making it runnable.
///
/// The asynchronous process service uses this as its commit boundary: it can
/// publish detached ownership or honor cancellation before the first user
/// instruction executes, then either mark the PID ready or remove it.
pub(crate) fn prepare_user_binary_unstarted(
    path: &str,
    argv: &[&str],
    envp: &[&str],
    terminal_id: Option<crate::window::WindowId>,
) -> Result<u32, String> {
    crate::userland::abi::reset_unknown_syscall_trace();

    // VirtIO storage completion is asynchronous and independent of the
    // active CR3, so do the potentially long read before entering the
    // address-space setup transaction.
    let (file, bytes) = read_file_bytes(path)?;
    let mut at_random = [0u8; 16];
    crate::random::fill_bytes(&mut at_random)
        .map_err(|e| format!("entropy unavailable: {:?}", e))?;
    let _setup_guard = BINARY_SETUP_MUTEX.lock();

    let aspace = crate::userland::address_space::AddressSpace::new()
        .map_err(|e| format!("AddressSpace::new: {:?}", e))?;
    // SAFETY: AddressSpace::new copies the kernel half from the kernel L4,
    // and BINARY_SETUP_MUTEX excludes competing CR3-sensitive loaders.
    unsafe {
        aspace.activate();
    }

    let image = crate::userland::loader::load_elf_file(&bytes, file)
        .map_err(|e| format!("loader error: {:?}", e))?;

    crate::userland::setup_user_process_unstarted(
        image,
        argv,
        envp,
        Some(aspace),
        terminal_id,
        &at_random,
    )
    .map_err(|e| format!("setup_user_process: {:?}", e))
}

fn read_file_bytes(path: &str) -> Result<(crate::lib::arc::Arc<crate::fs::File>, Vec<u8>), String> {
    use crate::fs::File;
    let file = File::open_read(path).map_err(|e| format!("open '{}': {}", path, e))?;
    let bytes = file
        .read_to_vec()
        .map_err(|e| format!("read '{}': {}", path, e))?;
    Ok((file, bytes))
}

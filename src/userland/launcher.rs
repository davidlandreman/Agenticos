//! Generic user-binary launcher — extracted from `src/commands/run/mod.rs`
//! so the same code path can launch zsh as the default terminal shell
//! (`src/window/terminal_factory.rs`) without depending on the
//! soon-to-be-deleted `run` shell command.
//!
//! See `docs/plans/2026-05-16-004-feat-zsh-default-terminal-and-gui-launchers-plan.md`.
//!
//! This is the synchronous launch path: callers block in
//! [`launch_user_binary`] until the user process exits (cooperative
//! `exit_group` or fault). Unsupported syscalls return `-ENOSYS` to the
//! process so libc can choose a fallback. U8 removed the
//! single-user-app guard — multiple kernel threads can now call this
//! concurrently and each gets its own ring-3 process, scheduled
//! round-robin via the U5 timer ISR.
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
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use spin::Mutex;

use crate::userland::lifecycle::ExitKind;

/// U8 serialization: protects the `aspace.activate() → load_elf →
/// setup_user_process` window from concurrent launchers. Without it,
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
    // U8: no more single-app guard. Multiple concurrent launches are
    // supported; each gets its own Process, address space, and
    // kernel-thread block until its child exits.

    // Each launch gets a fresh "seen syscalls" table so trace-mode
    // logging is meaningful per-binary rather than stale.
    crate::userland::abi::reset_unknown_syscall_trace();

    // U8 concurrency fix: serialize the LOAD-AND-SETUP phase across
    // concurrent launchers. CR3 is a per-CPU resource — if two
    // launchers race their `aspace.activate()` calls, the loser's
    // subsequent `map_user_region` calls write into the winner's L4
    // (the loader uses the active CR3). The mutex is dropped before
    // `wait_for_ring3_exit` so two concurrent ring-3 processes still
    // run concurrently — only their setup phases serialize.
    let pid = {
        let _setup_guard = BINARY_SETUP_MUTEX.lock();

        let aspace = crate::userland::address_space::AddressSpace::new()
            .map_err(|e| format!("AddressSpace::new: {:?}", e))?;
        // SAFETY: AddressSpace::new copies the kernel half from the
        // kernel L4, so kernel code after the CR3 write is still
        // mapped. Holding `_setup_guard` guarantees no other launcher
        // will swap CR3 until we drop it.
        unsafe { aspace.activate(); }

        // The BinaryLoadGuard pauses the compositor's input + render
        // during the long PIO load (see
        // `docs/solutions/learnings/2026-05-09-multi-mib-user-binary-load.md`).
        let image = {
            let _load_guard = crate::userland::lifecycle::BinaryLoadGuard::enter();
            let bytes = read_file_bytes(path)?;
            crate::userland::loader::load_elf(&bytes)
                .map_err(|e| format!("loader error: {:?}", e))?
        };

        crate::userland::setup_user_process(image, argv, envp, Some(aspace))
            .map_err(|e| format!("setup_user_process: {:?}", e))?
        // _setup_guard drops here — other launchers can now run their
        // load + setup phases concurrently with our process running
        // in ring 3.
    };

    let result = crate::userland::wait_for_ring3_exit(pid);

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

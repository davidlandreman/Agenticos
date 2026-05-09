//! `run /host/<NAME>.ELF` — the userland-app launch verb (U7).
//!
//! Reads the file via the FAT FS layer, hands the bytes to the U6 ELF loader,
//! and on success enters ring 3 via `crate::userland::enter_user_mode`. On
//! return (cooperative `exit` syscall or fault routed through
//! `cleanup_user_process`), drops the `UserImage` (which unmaps + frees) and
//! reports the exit kind/code on the active terminal.
//!
//! The single-user-app invariant (D5) is enforced before the loader runs.
//!
//! Mirrors the structure of `src/commands/cat/mod.rs`.
//!
//! ## Path case sensitivity
//!
//! The host folder is mounted at `/host` (lowercase). The VFS path
//! resolver in `src/fs/vfs.rs` uses byte-exact `path.starts_with(mount.path)`
//! so `/HOST/HELLOCPP.ELF` returns "File or directory not found". Use
//! `/host/HELLOCPP.ELF`. Filenames inside the mount must be uppercase 8.3
//! per the FAT 8.3 limitation (see `src/fs/CLAUDE.md`), but the mount
//! point itself is lowercase.
//!
//! ## Performance hot path
//!
//! The run command grabs `crate::userland::lifecycle::BinaryLoadGuard` for
//! the duration of `RunProcess::run`. The kernel main loop reads
//! `binary_load_in_progress()` and pauses GUI/render housekeeping while
//! the guard is held — the run process gets the CPU end-to-end through
//! `read → load_elf → enter_user_mode → ring-3 → exit`. Without the gate,
//! `render_frame`'s framebuffer writes contend with PIO IDE reads and
//! stretch a sub-second load into many seconds (see
//! `docs/solutions/learnings/2026-05-09-multi-mib-user-binary-load.md`).

use crate::drivers::display::display;
use crate::graphics::color::Color;
use crate::process::{BaseProcess, HasBaseProcess, RunnableProcess};
use crate::userland::lifecycle::ExitKind;
use crate::{println};
use alloc::{boxed::Box, string::String, vec::Vec};

pub struct RunProcess {
    pub base: BaseProcess,
    args: Vec<String>,
}

impl RunProcess {
    pub fn new_with_args(args: Vec<String>) -> Self {
        Self {
            base: BaseProcess::new("run"),
            args,
        }
    }
}

impl HasBaseProcess for RunProcess {
    fn base(&self) -> &BaseProcess {
        &self.base
    }
    fn base_mut(&mut self) -> &mut BaseProcess {
        &mut self.base
    }
}

impl RunnableProcess for RunProcess {
    fn run(&mut self) {
        self.run();
    }
    fn get_name(&self) -> &str {
        self.base.get_name()
    }
}

impl RunProcess {
    pub fn run(&mut self) {
        if self.args.is_empty() {
            display::set_color(Color::RED);
            println!("run: missing path argument");
            println!("Usage: run <path>");
            display::set_color(Color::WHITE);
            return;
        }

        // Mark the kernel as actively loading/running a binary so the kernel
        // main loop pauses GUI/render housekeeping for the duration. This
        // covers the full read → load_elf → enter_user_mode → ring-3 →
        // exit window; without it, `render_frame`'s ~3.7 MiB-per-repaint
        // framebuffer writes contend with PIO IDE reads and heap demand-
        // paging, stretching the multi-MiB binary load well past what test
        // mode (no GUI) measures. RAII guard ensures the marker drops on
        // every exit path including early returns. (D5 single-user-app.)
        let _load_guard = crate::userland::lifecycle::BinaryLoadGuard::enter();

        let path = self.args[0].clone();
        match self.run_path(&path) {
            Ok(()) => {}
            Err(msg) => {
                display::set_color(Color::RED);
                println!("run: {}", msg);
                display::set_color(Color::WHITE);
            }
        }
    }

    fn run_path(&self, path: &str) -> Result<(), String> {
        // D5: refuse a second user app while one is already active.
        if crate::userland::lifecycle::user_active() {
            return Err(String::from("another user app is already running"));
        }

        // Read the ELF bytes through the FAT VFS.
        crate::debug_info!("[run] read_to_vec({}) starting", path);
        let bytes = read_file_bytes(path)?;
        crate::debug_info!("[run] read_to_vec returned {} bytes", bytes.len());

        // Parse + map + relocate. On error, the partial UserImage drops here
        // and the rollback unmaps any pages the loader had committed.
        let image = crate::userland::loader::load_elf(&bytes)
            .map_err(|e| alloc::format!("loader error: {:?}", e))?;

        // Enter ring 3. Returns when the user app exits (cooperative or
        // abnormal). The `image` was *moved* into the active-user slot by
        // `enter_user_mode` — we no longer own it.
        let result = crate::userland::enter_user_mode(image)
            .map_err(|e| alloc::format!("enter_user_mode: {:?}", e))?;

        // Drop the active image (unmaps + frees). We must do this BEFORE
        // returning so a follow-up `run` finds a clean user VA window.
        let _ = crate::userland::release_active_image();

        // Report the exit reason.
        match result {
            (ExitKind::Cooperative, code) => {
                crate::debug_info!("run: app exited normally with code {}", code);
                if code != 0 {
                    println!("[run] exit code {}", code);
                }
            }
            (ExitKind::Abnormal { vector, fault_rip }, _) => {
                display::set_color(Color::RED);
                println!(
                    "[run] app terminated by fault: vector={} rip={:#x}",
                    vector, fault_rip
                );
                display::set_color(Color::WHITE);
            }
            (ExitKind::UnimplementedSyscall { nr }, _) => {
                display::set_color(Color::RED);
                println!("[run] app issued unimplemented syscall nr={}", nr);
                display::set_color(Color::WHITE);
            }
            (ExitKind::None, _) => {
                // Should not happen — `enter_user_mode` only returns after
                // an exit was recorded. Log defensively.
                crate::debug_warn!("run: enter_user_mode returned with ExitKind::None");
            }
        }

        Ok(())
    }
}

/// Largest user binary the loader will accept, in bytes.
///
/// A static `g++ -static -no-pie` C++ iostream binary against musl + libstdc++
/// typically lands between 1 and 4 MiB depending on toolchain version. 16 MiB
/// gives ample headroom while keeping the failure mode visible: the kernel
/// heap is sized at 100 MiB, so a 16 MiB ELF plus the loader's working state
/// is comfortable; an outsized binary fails loud here rather than as a
/// confusing OOM panic deep inside `Vec::resize`.
const MAX_USER_BINARY_BYTES: u64 = 16 * 1024 * 1024;

fn read_file_bytes(path: &str) -> Result<Vec<u8>, String> {
    use crate::fs::File;

    let file = File::open_read(path).map_err(|e| alloc::format!("open '{}': {}", path, e))?;
    let size = file.size();
    if size > MAX_USER_BINARY_BYTES {
        return Err(alloc::format!(
            "binary '{}' is {} bytes; max {} bytes ({} MiB)",
            path,
            size,
            MAX_USER_BINARY_BYTES,
            MAX_USER_BINARY_BYTES / (1024 * 1024)
        ));
    }
    file.read_to_vec()
        .map_err(|e| alloc::format!("read '{}': {}", path, e))
}

pub fn create_run_process(args: Vec<String>) -> Box<dyn RunnableProcess> {
    Box::new(RunProcess::new_with_args(args))
}

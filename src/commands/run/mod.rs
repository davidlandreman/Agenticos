//! `run /HOST/<NAME>.ELF` — the userland-app launch verb (U7).
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
        let bytes = read_file_bytes(path)?;

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
            (ExitKind::None, _) => {
                // Should not happen — `enter_user_mode` only returns after
                // an exit was recorded. Log defensively.
                crate::debug_warn!("run: enter_user_mode returned with ExitKind::None");
            }
        }

        Ok(())
    }
}

fn read_file_bytes(path: &str) -> Result<Vec<u8>, String> {
    use crate::fs::File;

    let file = File::open_read(path).map_err(|e| alloc::format!("open '{}': {}", path, e))?;
    file.read_to_vec()
        .map_err(|e| alloc::format!("read '{}': {}", path, e))
}

pub fn create_run_process(args: Vec<String>) -> Box<dyn RunnableProcess> {
    Box::new(RunProcess::new_with_args(args))
}

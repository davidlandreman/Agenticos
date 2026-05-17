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
        // U8: strip leading flags before the path argument. Today we
        // recognize `--trace`, which flips the unknown-syscall trace
        // mode on for this launch (auto-restored after exit). Future
        // flags can join this loop without touching downstream code.
        let mut trace = false;
        let mut consumed = 0usize;
        for tok in self.args.iter() {
            match tok.as_str() {
                "--trace" => {
                    trace = true;
                    consumed += 1;
                }
                "--" => {
                    consumed += 1;
                    break;
                }
                s if s.starts_with("--") => {
                    display::set_color(Color::RED);
                    println!("run: unknown flag '{}'", s);
                    display::set_color(Color::WHITE);
                    return;
                }
                _ => break,
            }
        }
        let argv_after_flags: Vec<String> = self.args.iter().skip(consumed).cloned().collect();
        if argv_after_flags.is_empty() {
            display::set_color(Color::RED);
            println!("run: missing path argument");
            println!("Usage: run [--trace] <path> [args...]");
            display::set_color(Color::WHITE);
            return;
        }

        let path = argv_after_flags[0].clone();
        let prior_trace = crate::userland::abi::is_trace_mode();
        if trace {
            crate::userland::abi::set_trace_mode(true);
            println!("[run] unknown-syscall trace ON");
        }
        let result = self.run_path(&path, &argv_after_flags);
        if trace {
            // Always restore trace state — a launch shouldn't leak the
            // trace flag into the next `run` invocation.
            crate::userland::abi::set_trace_mode(prior_trace);
        }
        if let Err(msg) = result {
            display::set_color(Color::RED);
            println!("run: {}", msg);
            display::set_color(Color::WHITE);
        }
    }

    fn run_path(&self, path: &str, parsed_argv: &[String]) -> Result<(), String> {
        // D5: refuse a second user app while one is already active.
        if crate::userland::lifecycle::user_active() {
            return Err(String::from("another user app is already running"));
        }

        // Each launch gets a fresh "seen syscalls" table so trace mode's
        // first-occurrence logging is meaningful per-binary, not stale
        // from a previous run. The reset is cheap (512 atomic stores)
        // and runs unconditionally — if trace mode is off, nothing reads
        // SEEN_NRS anyway.
        crate::userland::abi::reset_unknown_syscall_trace();

        // Phase 4 PR-B: each user process runs on its own L4 page-
        // table root. Build it now (kernel-half entries are shared by
        // copying their PML4 entries; PML4[0] is empty), activate it,
        // and let the loader map user pages into PML4[0] of this fresh
        // L4.
        let aspace = crate::userland::address_space::AddressSpace::new()
            .map_err(|e| alloc::format!("AddressSpace::new: {:?}", e))?;
        // SAFETY: the kernel half (PML4 1..512) was just copied from
        // the kernel L4, so the very kernel code that runs after the
        // CR3 write is still reachable.
        unsafe { aspace.activate(); }

        // Scope the BinaryLoadGuard to the load phase (file read + ELF
        // parse + page mapping). It pauses GUI/render housekeeping so the
        // multi-MiB load runs uncontended (see
        // `docs/solutions/learnings/2026-05-09-multi-mib-user-binary-load.md`).
        // We deliberately drop the guard before `enter_user_mode` so that
        // during ring-3 execution the kernel main loop runs input routing
        // and `render_frame` — the user's typed input gets echoed and
        // their `write` syscalls become visible immediately.
        let image = {
            let _load_guard = crate::userland::lifecycle::BinaryLoadGuard::enter();
            crate::debug_info!("[run] read_to_vec({}) starting", path);
            let bytes = read_file_bytes(path)?;
            crate::debug_info!("[run] read_to_vec returned {} bytes", bytes.len());
            crate::userland::loader::load_elf(&bytes)
                .map_err(|e| alloc::format!("loader error: {:?}", e))?
        };

        // Build argv from the parsed tokens (path + remaining args, with
        // the leading flags U8 stripped already) and a small envp the
        // user app can rely on. Borrow as &str slices straight out of
        // the `Vec<String>` — the references stay valid until
        // `enter_user_mode_with` returns, which is the entire
        // user-process lifetime.
        let argv: Vec<&str> = parsed_argv.iter().map(|s| s.as_str()).collect();
        // U8: the envp is shaped for the staged /etc/passwd entry
        // (root:x:0:0::/root:/bin/zsh). HOME/USER/LOGNAME match what
        // musl's getpwuid_r would resolve so zsh doesn't have to do the
        // lookup at startup; SHELL=/bin/zsh keeps zsh's $SHELL accurate;
        // TERM=dumb dodges terminfo lookups (we don't ship a database
        // yet). PATH=/bin:/host so zsh's command lookup finds BusyBox
        // applets via the virtual /bin namespace
        // (src/userland/bin_namespace.rs) first, then falls back to
        // /host for other staged userland binaries.
        let envp: [&str; 7] = [
            "PATH=/bin:/host",
            "HOME=/root",
            "USER=root",
            "LOGNAME=root",
            "SHELL=/bin/zsh",
            "TERM=dumb",
            "LANG=C",
        ];

        // Enter ring 3. Returns when the user app exits (cooperative or
        // abnormal). The `image` and `aspace` were *moved* into the
        // active process slot — we no longer own either.
        let result = crate::userland::enter_user_mode_with_aspace(
            image, &argv, &envp, Some(aspace),
        )
        .map_err(|e| alloc::format!("enter_user_mode: {:?}", e))?;

        // Drop the active image (unmaps + frees) and the address space
        // it ran on. `release_active_image` returns the AddressSpace
        // it took out of the process slot (if any); its `Drop` impl
        // switches CR3 back to the kernel L4 if it was still active.
        let (_img, _aspace) = crate::userland::release_active_image();

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

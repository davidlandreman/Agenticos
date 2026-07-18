//! Terminal Factory for spawning new terminal windows
//!
//! This module provides functionality to create new terminal windows,
//! each with its own shell process. Used by the "cmd" command to spawn
//! additional terminals.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::window::{with_window_manager, WindowId, Rect, Window};
use crate::window::windows::{FrameWindow, TerminalWindow};
use crate::process::ProcessId;

/// Represents a terminal instance with its associated shell process
#[derive(Debug, Clone, Copy)]
pub struct TerminalInstance {
    /// The frame window containing the terminal
    pub frame_id: WindowId,
    /// The terminal window itself
    pub terminal_id: WindowId,
    /// The shell process associated with this terminal
    pub shell_pid: Option<ProcessId>,
    /// Terminal number for display purposes
    pub number: usize,
}

/// Counter for terminal numbering
static TERMINAL_COUNTER: Mutex<usize> = Mutex::new(1);

/// List of all terminal instances
static TERMINAL_INSTANCES: Mutex<Vec<TerminalInstance>> = Mutex::new(Vec::new());

/// Spawn a new terminal window with its own shell process
///
/// # Returns
/// * `Ok(TerminalInstance)` - The newly created terminal instance
/// * `Err(&'static str)` - Error message if creation failed
pub fn spawn_terminal() -> Result<TerminalInstance, &'static str> {
    let terminal_number = {
        let mut counter = TERMINAL_COUNTER.lock();
        let num = *counter;
        *counter += 1;
        num
    };

    let title = if terminal_number == 1 {
        String::from("AgenticOS Terminal")
    } else {
        alloc::format!("AgenticOS Terminal {}", terminal_number)
    };

    // Create the terminal window structure
    let instance = with_window_manager(|wm| {
        // Get screen dimensions
        let (screen_width, screen_height) = wm.screen_dimensions();

        // Find the desktop window (root of active screen)
        let desktop_id = wm.get_active_screen()
            .and_then(|s| s.root_window)
            .ok_or("No desktop window found")?;

        // Calculate position for new terminal (offset from previous ones)
        let existing_count = TERMINAL_INSTANCES.lock().len();
        let offset = (existing_count * 30) as i32;
        let frame_x = 100 + offset;
        let frame_y = 50 + offset;
        let frame_width = 800.min(screen_width - 200);
        let frame_height = 600.min(screen_height - 100);

        // Create frame window
        let frame_id = wm.create_window(Some(desktop_id));
        let mut frame_window = Box::new(FrameWindow::new(frame_id, &title));
        frame_window.set_parent(Some(desktop_id));
        frame_window.set_bounds(Rect::new(frame_x, frame_y, frame_width, frame_height));

        // Create terminal window inside the frame
        let terminal_id = wm.create_window(Some(frame_id));
        let content_area = frame_window.content_area();
        let terminal_bounds = Rect::new(
            content_area.x,
            content_area.y,
            content_area.width,
            content_area.height,
        );
        // Use new_with_id to ensure the terminal uses the ID from WindowManager
        let mut terminal_window = Box::new(TerminalWindow::new_with_id(terminal_id, terminal_bounds));
        terminal_window.set_parent(Some(frame_id));

        // Set the terminal as the frame's content
        frame_window.set_content_window(terminal_id);

        // Add windows to registry
        wm.set_window_impl(frame_id, frame_window);
        wm.set_window_impl(terminal_id, terminal_window);

        // Add frame window to desktop's children
        if let Some(desktop) = wm.window_registry.get_mut(&desktop_id) {
            desktop.add_child(frame_id);
        }

        // Bring to front and focus both frame and terminal
        wm.bring_to_front(frame_id);
        // Focus the frame (for blue title bar)
        if let Some(frame) = wm.window_registry.get_mut(&frame_id) {
            frame.set_focus(true);
        }
        // Focus the terminal (for keyboard input)
        wm.focus_window(terminal_id);

        // Invalidate windows to trigger repaint
        if let Some(window) = wm.window_registry.get_mut(&frame_id) {
            window.invalidate();
        }
        if let Some(window) = wm.window_registry.get_mut(&terminal_id) {
            window.invalidate();
        }

        Ok(TerminalInstance {
            frame_id,
            terminal_id,
            shell_pid: None,
            number: terminal_number,
        })
    }).ok_or("Window manager not initialized")??;

    // Store the instance
    TERMINAL_INSTANCES.lock().push(instance);

    crate::debug_info!(
        "Terminal factory: Created terminal {} (frame={:?}, terminal={:?})",
        terminal_number, instance.frame_id, instance.terminal_id
    );

    Ok(instance)
}

/// Spawn a terminal with `zsh` (`/host/ZSH.ELF`) running as its shell.
///
/// Replaces the prior cooperative-poll kernel shell. The terminal is
/// created first (so the window exists and can show an error if zsh
/// fails to launch), then a kernel-side process is spawned whose entry
/// function loads ZSH.ELF and `iretq`s into ring 3. zsh's stdio routes
/// through the existing path: `crate::print!` macro writes from
/// stdout/stderr land via `set_current_output_terminal`, and the
/// TerminalWindow pushes keystrokes into `crate::userland::stdin`
/// (raw or cooked depending on zsh's tty mode).
///
/// See `docs/plans/2026-05-16-004-feat-zsh-default-terminal-and-gui-launchers-plan.md`.
pub fn spawn_terminal_with_shell() -> Result<TerminalInstance, &'static str> {
    let mut instance = spawn_terminal()?;
    let terminal_id = instance.terminal_id;

    // Output routing — same registration the old kernel shell used so
    // text the loader prints during early launch (and any error
    // surfaced before zsh's first write) lands in this terminal.
    crate::window::terminal::register_terminal(terminal_id);

    // Sync the pty's winsize to the freshly-constructed terminal
    // window's grid. Without this, `TIOCGWINSZ` would report the
    // default 80×24 even for windows that render a different grid —
    // and vi / less would wrap to the wrong column.
    if let Some((rows, cols)) =
        crate::window::with_window_manager(|wm| wm.window_registry.get(&terminal_id).and_then(|w| w.grid_size()))
            .flatten()
    {
        crate::window::terminal::sync_terminal_winsize(terminal_id, rows, cols);
    }

    let pid = spawn_zsh_for_terminal(terminal_id);
    instance.shell_pid = Some(pid);

    {
        let mut instances = TERMINAL_INSTANCES.lock();
        if let Some(inst) = instances.iter_mut().find(|i| i.terminal_id == terminal_id) {
            inst.shell_pid = Some(pid);
        }
    }

    crate::debug_info!(
        "Terminal factory: zsh spawned for terminal {} with PID {:?}",
        instance.number, pid
    );

    Ok(instance)
}

/// FAT path the kernel loads when launching the default shell. Staged
/// from `userland/prebuilt/ZSH.ELF` by `build.sh` / `test.sh`.
pub(crate) const ZSH_HOST_PATH: &str = "/host/ZSH.ELF";

/// Environment used by every default interactive terminal and by the zsh
/// launch regression. Keeping one shared profile prevents the test path from
/// silently exercising a smaller initial stack than the desktop path.
pub(crate) const TERMINAL_SHELL_ENV: [&str; 8] = [
    "PATH=/bin:/host",
    "HOME=/root",
    "USER=root",
    "LOGNAME=root",
    "SHELL=/bin/zsh",
    "TERM=xterm-256color",
    "COLORTERM=truecolor",
    "LANG=C",
];

/// Spawn a kernel-side process whose entry function loads
/// `/host/ZSH.ELF` and enters ring 3 with stdio bound to `terminal_id`.
/// The kernel process blocks for the entire zsh session; on exit the terminal
/// remains open so its diagnostic banner stays visible.
fn spawn_zsh_for_terminal(terminal_id: WindowId) -> ProcessId {
    crate::process::spawn_process(
        alloc::string::String::from("zsh"),
        Some(terminal_id),
        move || {
            crate::window::terminal::set_current_output_terminal(terminal_id);

            // PATH hits the virtual /bin namespace first (BusyBox
            // applets + GUI app launchers), then /host for staged
            // binaries. TERM=xterm-256color advertises full ANSI / 256
            // color / cursor-positioning support so vi, less, and
            // agnoster pick the right terminfo behavior — the matching
            // parser support lives in `src/terminal/` (see the plan
            // `docs/plans/2026-05-24-001-feat-terminal-ansi-vt-pty-and-caret-plan.md`).
            // HOME/USER/LOGNAME/SHELL match the staged /etc/passwd
            // entry. COLORTERM=truecolor unlocks 24-bit-color code
            // paths in modern programs.
            let argv = [ZSH_HOST_PATH];
            match crate::userland::launcher::launch_user_binary(
                ZSH_HOST_PATH,
                &argv,
                &TERMINAL_SHELL_ENV,
            ) {
                Ok((kind, code)) => {
                    let msg = alloc::format!(
                        "[zsh exited: kind={:?}, code={}]",
                        kind,
                        code,
                    );
                    // Surface to BOTH serial (debug) and the terminal
                    // window so the user can see + capture diagnostics.
                    crate::debug_error!("Terminal factory: {}", msg);
                    let banner = alloc::format!("\n{}\n", msg);
                    crate::window::terminal::write_to_terminal_id(terminal_id, &banner);
                }
                Err(msg) => {
                    crate::println!("zsh failed to launch: {}", msg);
                    crate::debug_error!("Terminal factory: zsh launch failed: {}", msg);
                    let banner = alloc::format!(
                        "\n[zsh launch failed: {}]\n",
                        msg,
                    );
                    crate::window::terminal::write_to_terminal_id(terminal_id, &banner);
                }
            }

            crate::window::terminal::clear_current_output_terminal();
            // Leave the terminal window OPEN so the exit banner above
            // stays visible. The user can dismiss the window manually.
            // close_terminal(terminal_id);  // disabled for diagnostics
        },
    )
}

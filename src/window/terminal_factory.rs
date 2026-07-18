//! Terminal Factory for spawning new terminal windows
//!
//! This module provides functionality to create new terminal windows,
//! each with its own shell process. Used by the "cmd" command to spawn
//! additional terminals.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::userland::process_service::{
    LaunchId, LaunchOutcome, LaunchSpec, DEFAULT_USER_ENV, ZSH_HOST_PATH,
};
use crate::window::windows::{FrameWindow, TerminalWindow};
use crate::window::{with_window_manager, Rect, Window, WindowId};

/// Represents a terminal instance with its associated shell process
#[derive(Debug, Clone, Copy)]
pub struct TerminalInstance {
    /// The frame window containing the terminal
    pub frame_id: WindowId,
    /// The terminal window itself
    pub terminal_id: WindowId,
    /// Asynchronous launch associated with this terminal's shell.
    pub shell_launch: Option<LaunchId>,
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
        let desktop_id = wm
            .get_active_screen()
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
        let mut terminal_window =
            Box::new(TerminalWindow::new_with_id(terminal_id, terminal_bounds));
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
            shell_launch: None,
            number: terminal_number,
        })
    })
    .ok_or("Window manager not initialized")??;

    // Store the instance
    TERMINAL_INSTANCES.lock().push(instance);

    crate::debug_info!(
        "Terminal factory: Created terminal {} (frame={:?}, terminal={:?})",
        terminal_number,
        instance.frame_id,
        instance.terminal_id
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
    if let Some((rows, cols)) = crate::window::with_window_manager(|wm| {
        wm.window_registry
            .get(&terminal_id)
            .and_then(|w| w.grid_size())
    })
    .flatten()
    {
        crate::window::terminal::sync_terminal_winsize(terminal_id, rows, cols);
    }

    let launch = spawn_zsh_for_terminal(terminal_id);
    instance.shell_launch = launch;

    {
        let mut instances = TERMINAL_INSTANCES.lock();
        if let Some(inst) = instances.iter_mut().find(|i| i.terminal_id == terminal_id) {
            inst.shell_launch = launch;
        }
    }

    crate::debug_info!(
        "Terminal factory: zsh submitted for terminal {} as {:?}",
        instance.number,
        launch
    );

    Ok(instance)
}

/// Called by the window manager when a window is destroyed (e.g. the
/// user clicked a terminal frame's close button). If `window_id` is a
/// terminal frame or the terminal content window itself, tear down the
/// terminal's entire ring-3 process tree (`zsh` and everything it
/// forked — `ping`, `nc`, etc.) and drop the tracked `TerminalInstance`.
///
/// Without this, closing the window only removed the window from the
/// registry; the ring-3 zsh and its children kept running as orphans,
/// still time-sliced by the scheduler and still writing to a terminal
/// whose window no longer exists.
///
/// Safe (and cheap) to call for any window: non-terminal windows have no
/// matching `TerminalInstance`, so it is a no-op. Idempotent — the
/// instance is removed on the first matching call, so the recursive
/// `destroy_window` pass over the frame's children finds nothing to do.
pub fn on_window_destroyed(window_id: WindowId) {
    // Find and remove the matching instance (a terminal is destroyed via
    // its frame, but match the content window too for robustness).
    let instance = {
        let mut instances = TERMINAL_INSTANCES.lock();
        instances
            .iter()
            .position(|i| i.frame_id == window_id || i.terminal_id == window_id)
            .map(|pos| instances.remove(pos))
    };
    let Some(instance) = instance else {
        return;
    };

    crate::debug_info!(
        "Terminal factory: closing terminal {} (frame={:?}, terminal={:?}) — killing its ring-3 processes",
        instance.number,
        instance.frame_id,
        instance.terminal_id
    );

    // Cancel a not-yet-installed shell and kill zsh + every process it
    // forked if the launch has already committed. All descendants inherit the
    // same terminal_id.
    crate::userland::process_service::cancel_for_terminal(instance.terminal_id);
}

/// Queue `/host/ZSH.ELF` with stdio bound to `terminal_id`.
/// The shared process service owns setup and teardown; on exit the terminal
/// remains open so its diagnostic banner stays visible.
fn spawn_zsh_for_terminal(terminal_id: WindowId) -> Option<LaunchId> {
    let argv = [ZSH_HOST_PATH];
    let spec = LaunchSpec::new(ZSH_HOST_PATH, &argv, &DEFAULT_USER_ENV)
        .with_terminal(terminal_id)
        .on_complete(Box::new(move |outcome| {
            let msg = match outcome {
                LaunchOutcome::Exited { id, kind, code, .. } => {
                    alloc::format!("[zsh {id:?} exited: kind={kind:?}, code={code}]")
                }
                LaunchOutcome::Failed { id, error } => {
                    alloc::format!("[zsh {id:?} launch failed: {error}]")
                }
            };
            crate::debug_error!("Terminal factory: {}", msg);
            let banner = alloc::format!("\n{msg}\n");
            crate::window::terminal::write_to_terminal_id(terminal_id, &banner);
        }));
    match crate::userland::process_service::submit(spec) {
        Ok(id) => Some(id),
        Err(error) => {
            let banner = alloc::format!("\n[zsh launch submission failed: {error}]\n");
            crate::debug_error!("Terminal factory: {}", banner);
            crate::window::terminal::write_to_terminal_id(terminal_id, &banner);
            None
        }
    }
}

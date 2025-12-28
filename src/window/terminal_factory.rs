//! Terminal Factory for spawning new terminal windows
//!
//! This module provides functionality to create new terminal windows,
//! each with its own shell process. Used by the "cmd" command to spawn
//! additional terminals.

use alloc::boxed::Box;
use alloc::string::{String, ToString};
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
        let screen_width = wm.graphics_device.width() as u32;
        let screen_height = wm.graphics_device.height() as u32;

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

/// Spawn a terminal with an associated shell
pub fn spawn_terminal_with_shell() -> Result<TerminalInstance, &'static str> {
    let mut instance = spawn_terminal()?;

    // Register a shell for this terminal (cooperative/polled, not a real process)
    let terminal_id = instance.terminal_id;
    let pid = crate::process::allocate_pid();

    // Register the terminal for output routing
    crate::window::terminal::register_terminal(terminal_id);

    // Register the shell instance
    crate::commands::shell::shell_process::register_shell(terminal_id, pid);

    instance.shell_pid = Some(pid);

    // Update the stored instance
    {
        let mut instances = TERMINAL_INSTANCES.lock();
        if let Some(inst) = instances.iter_mut().find(|i| i.terminal_id == terminal_id) {
            inst.shell_pid = Some(pid);
        }
    }

    crate::debug_info!(
        "Terminal factory: Shell registered for terminal {} with PID {:?}",
        instance.number, pid
    );

    Ok(instance)
}

/// Get the shell process ID for a terminal
pub fn get_shell_for_terminal(terminal_id: WindowId) -> Option<ProcessId> {
    TERMINAL_INSTANCES.lock()
        .iter()
        .find(|i| i.terminal_id == terminal_id)
        .and_then(|i| i.shell_pid)
}

/// Get all terminal instances
pub fn get_all_terminals() -> Vec<TerminalInstance> {
    TERMINAL_INSTANCES.lock().clone()
}

/// Close a terminal instance
pub fn close_terminal(terminal_id: WindowId) {
    // Remove from instances list
    let instance = {
        let mut instances = TERMINAL_INSTANCES.lock();
        let pos = instances.iter().position(|i| i.terminal_id == terminal_id);
        pos.map(|p| instances.remove(p))
    };

    if let Some(inst) = instance {
        // Terminate the shell process
        if let Some(pid) = inst.shell_pid {
            crate::process::scheduler::SCHEDULER.lock().terminate(pid);
        }

        // Remove windows from manager
        with_window_manager(|wm| {
            // Get parent (desktop) before removing
            let parent_id = wm.window_registry.get(&inst.frame_id)
                .and_then(|w| w.parent());

            // Remove terminal window
            wm.window_registry.remove(&inst.terminal_id);

            // Remove frame window
            wm.window_registry.remove(&inst.frame_id);

            // Remove frame from desktop's children
            if let Some(parent_id) = parent_id {
                if let Some(desktop) = wm.window_registry.get_mut(&parent_id) {
                    desktop.remove_child(inst.frame_id);
                }
            }

            // Trigger full repaint
            wm.force_full_repaint();
        });

        crate::debug_info!("Terminal factory: Closed terminal {}", inst.number);
    }
}

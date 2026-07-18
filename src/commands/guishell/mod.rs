//! GUIShell - Graphical shell with taskbar and Start menu
//!
//! This module manages the desktop environment with a taskbar at the bottom
//! of the screen, a Start button that opens a menu, and buttons for open windows.
//!
//! The GUIShell runs as a background process that sleeps until events occur,
//! reducing CPU usage when the system is idle.

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::arch::x86_64::preemption_guard::PreemptionMutex;
use crate::process::{ProcessId, WakeEvents};
use crate::window::windows::taskbar::{
    tray_bounds, window_button_bounds, BUTTON_GAP, BUTTON_HEIGHT, BUTTON_Y_OFFSET,
    START_BUTTON_WIDTH,
};
use crate::window::windows::{
    Button, DesktopWindow, StartMenuAction, StartMenuWindow, TaskbarTrayWindow, TaskbarWindow,
};
use crate::window::{self, Point, Rect, Window, WindowId};

/// GUIShell state
pub struct GUIShellState {
    /// Desktop window ID
    pub desktop_id: Option<WindowId>,
    /// Taskbar window ID
    pub taskbar_id: Option<WindowId>,
    /// Start button ID
    pub start_button_id: Option<WindowId>,
    /// Right-side notification tray ID
    pub tray_id: Option<WindowId>,
    /// Current menu ID (if open)
    pub menu_id: Option<WindowId>,
    /// Tracked window buttons: (button_id, frame_id)
    pub window_buttons: Vec<(WindowId, WindowId)>,
    /// Whether the GUI shell is initialized
    pub initialized: bool,
    /// Deferred action to perform in next poll
    pub pending_action: Option<PendingAction>,
    /// Process ID of the GUIShell background process
    pub process_id: Option<ProcessId>,
}

/// Actions that need to be deferred to avoid deadlocks
#[derive(Clone)]
pub enum PendingAction {
    ToggleStartMenu,
    SpawnTerminal,
    SpawnPainting,
    SpawnCalc,
    SpawnGlGame,
    SpawnNotepad,
    SpawnTaskmgr,
    SpawnFileManager,
    OpenRunDialog,
    ShowShutdownNotice,
    FocusWindow(WindowId),
}

impl GUIShellState {
    pub const fn new() -> Self {
        GUIShellState {
            desktop_id: None,
            taskbar_id: None,
            start_button_id: None,
            tray_id: None,
            menu_id: None,
            window_buttons: Vec::new(),
            initialized: false,
            pending_action: None,
            process_id: None,
        }
    }
}

/// Queue a deferred action to be processed in the next poll
pub fn queue_action(action: PendingAction) {
    // Use lock() instead of try_lock() to ensure the action is always queued
    // try_lock() was silently dropping actions when the lock was briefly held
    let process_id = {
        let mut state = GUISHELL_STATE.lock();
        crate::debug_trace!(
            "GUIShell: Queuing action {:?}",
            core::mem::discriminant(&action)
        );
        state.pending_action = Some(action);
        state.process_id
    };

    // Signal the GUIShell process to wake up and handle the action
    if let Some(pid) = process_id {
        crate::process::signal_process(pid, WakeEvents::WINDOW_EVENT);
    }
}

/// Global GUIShell state
static GUISHELL_STATE: PreemptionMutex<GUIShellState> = PreemptionMutex::new(GUIShellState::new());

/// Enter the window manager from GUIShell code.
///
/// GUIShell state must always be released first. Keeping this check at the
/// boundary makes future state-to-window-manager lock regressions fail fast in
/// debug builds instead of wedging the compositor.
fn with_window_manager<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut window::WindowManager) -> R,
{
    debug_assert!(
        !GUISHELL_STATE.is_locked(),
        "GUIShell must release GUISHELL_STATE before acquiring the window manager"
    );
    window::with_window_manager(f)
}

/// Initialize the GUIShell desktop environment
#[cfg_attr(feature = "test", expect(dead_code, reason = "production-only API"))]
pub fn init_guishell() {
    {
        let state = GUISHELL_STATE.lock();
        if state.initialized {
            return;
        }
    }

    // Load the bundled wallpaper outside both GUI locks so the file read can't
    // block window or shell state. Falling back to None yields the legacy
    // solid-blue desktop.
    let wallpaper = window::load_default_wallpaper();

    let ids = with_window_manager(|wm| {
        // Get screen dimensions
        let (width, height) = wm.screen_dimensions();

        // Create GUI screen
        let screen_id = wm.create_screen(window::ScreenMode::Gui);
        wm.switch_screen(screen_id);

        // Create desktop background window — with wallpaper bytes when the
        // bundled BMP loaded successfully, otherwise the solid-color fallback.
        let desktop_id = wm.create_window(None);
        let desktop_bounds = Rect::new(0, 0, width, height);
        let desktop_window: Box<dyn window::Window> = match wallpaper {
            Some(bytes) => Box::new(DesktopWindow::new_with_wallpaper(
                desktop_id,
                desktop_bounds,
                bytes,
            )),
            None => Box::new(DesktopWindow::new(desktop_id, desktop_bounds)),
        };
        wm.set_window_impl(desktop_id, desktop_window);

        // Set desktop as the root window for the screen
        if let Some(screen) = wm.get_active_screen_mut() {
            screen.set_root_window(desktop_id);
        }

        // Create taskbar window at bottom of screen
        let taskbar_id = wm.create_window(Some(desktop_id));
        let mut taskbar = TaskbarWindow::new_with_id(taskbar_id, width, height);
        taskbar.set_parent(Some(desktop_id));
        wm.set_window_impl(taskbar_id, Box::new(taskbar));

        // Add taskbar to desktop's children
        if let Some(desktop) = wm.window_registry.get_mut(&desktop_id) {
            desktop.add_child(taskbar_id);
        }

        // Register taskbar with window manager
        wm.set_taskbar_id(Some(taskbar_id));

        // Create Start button as child of taskbar
        let start_button_id = wm.create_window(Some(taskbar_id));
        let start_bounds = Rect::new(
            BUTTON_GAP as i32,
            BUTTON_Y_OFFSET as i32,
            START_BUTTON_WIDTH,
            BUTTON_HEIGHT,
        );
        let mut start_button = Button::new_with_id(start_button_id, start_bounds, "Start");
        start_button.set_parent(Some(taskbar_id));

        // Set up click callback for Start button
        // Use deferred action to avoid deadlock (callback runs inside with_window_manager)
        start_button.on_click(|| {
            queue_action(PendingAction::ToggleStartMenu);
        });

        wm.set_window_impl(start_button_id, Box::new(start_button));

        // Add start button to taskbar's children
        if let Some(taskbar) = wm.window_registry.get_mut(&taskbar_id) {
            taskbar.add_child(start_button_id);
        }

        // Create the right-anchored notification tray as an independent child
        // so its minute updates invalidate only the tray region.
        let tray_id = wm.create_window(Some(taskbar_id));
        let mut tray = TaskbarTrayWindow::new_with_id(tray_id, tray_bounds(width));
        tray.set_parent(Some(taskbar_id));
        wm.set_window_impl(tray_id, Box::new(tray));

        // Force repaint of all windows
        if let Some(window) = wm.window_registry.get_mut(&desktop_id) {
            window.invalidate();
        }
        if let Some(window) = wm.window_registry.get_mut(&taskbar_id) {
            window.invalidate();
        }
        if let Some(window) = wm.window_registry.get_mut(&start_button_id) {
            window.invalidate();
        }
        if let Some(window) = wm.window_registry.get_mut(&tray_id) {
            window.invalidate();
        }

        // Force a full screen repaint
        wm.force_full_repaint();

        crate::debug_info!(
            "GUIShell: Desktop initialized (desktop={:?}, taskbar={:?}, start={:?}, tray={:?})",
            desktop_id,
            taskbar_id,
            start_button_id,
            tray_id
        );
        (desktop_id, taskbar_id, start_button_id, tray_id)
    });

    if let Some((desktop_id, taskbar_id, start_button_id, tray_id)) = ids {
        let mut state = GUISHELL_STATE.lock();
        state.desktop_id = Some(desktop_id);
        state.taskbar_id = Some(taskbar_id);
        state.start_button_id = Some(start_button_id);
        state.tray_id = Some(tray_id);
        state.initialized = true;
    }
}

/// Show the Start menu
fn show_start_menu() {
    let (taskbar_id, desktop_id) = {
        let state = GUISHELL_STATE.lock();

        // If menu already open, don't create another
        if state.menu_id.is_some() {
            return;
        }

        let taskbar_id = match state.taskbar_id {
            Some(id) => id,
            None => return,
        };

        let desktop_id = match state.desktop_id {
            Some(id) => id,
            None => return,
        };

        (taskbar_id, desktop_id)
    };

    let menu_id = with_window_manager(|wm| {
        // Get taskbar position
        let taskbar_bounds = wm
            .window_registry
            .get(&taskbar_id)
            .map(|w| w.bounds())
            .unwrap_or(Rect::new(0, 0, 0, 0));

        // Calculate menu dimensions from the typed Start-menu model.
        let menu_height = StartMenuWindow::root_height();
        let (screen_width, _) = wm.screen_dimensions();
        let menu_x =
            BUTTON_GAP.min(screen_width.saturating_sub(StartMenuWindow::maximum_width())) as i32;
        let menu_y = (taskbar_bounds.y - menu_height as i32).max(0);

        // Create menu window as child of desktop (so it's in the render hierarchy)
        let menu_id = wm.create_window(Some(desktop_id));
        let mut menu = StartMenuWindow::new_with_id(menu_id, Point::new(menu_x, menu_y));
        menu.set_parent(Some(desktop_id));

        // Use deferred actions because this callback runs under the window
        // manager lock. Disabled placeholders never emit an action.
        menu.on_select(|action| match action {
            StartMenuAction::FileManager => queue_action(PendingAction::SpawnFileManager),
            StartMenuAction::Terminal => queue_action(PendingAction::SpawnTerminal),
            StartMenuAction::Notepad => queue_action(PendingAction::SpawnNotepad),
            StartMenuAction::Painting => queue_action(PendingAction::SpawnPainting),
            StartMenuAction::Calc => queue_action(PendingAction::SpawnCalc),
            StartMenuAction::GlGame => queue_action(PendingAction::SpawnGlGame),
            StartMenuAction::TaskManager => queue_action(PendingAction::SpawnTaskmgr),
            StartMenuAction::Run => queue_action(PendingAction::OpenRunDialog),
            StartMenuAction::ShutDown => queue_action(PendingAction::ShowShutdownNotice),
        });

        wm.set_window_impl(menu_id, Box::new(menu));

        // Add menu to desktop's children
        if let Some(desktop) = wm.window_registry.get_mut(&desktop_id) {
            desktop.add_child(menu_id);
        }

        // Bring menu to front
        wm.bring_to_front(menu_id);

        // Set as active menu for click-outside handling
        wm.set_active_menu(Some(menu_id));

        // Force repaint
        if let Some(menu_win) = wm.window_registry.get_mut(&menu_id) {
            menu_win.invalidate();
        }
        wm.force_full_repaint();

        crate::debug_info!("GUIShell: Start menu opened (menu={:?})", menu_id);
        menu_id
    });

    if let Some(menu_id) = menu_id {
        GUISHELL_STATE.lock().menu_id = Some(menu_id);
    }
}

/// Close the Start menu
fn close_start_menu() {
    let menu = {
        let mut state = GUISHELL_STATE.lock();
        state
            .menu_id
            .take()
            .map(|menu_id| (menu_id, state.desktop_id))
    };

    if let Some((menu_id, desktop_id)) = menu {
        with_window_manager(|wm| {
            // Remove menu from desktop's children
            if let Some(desktop_id) = desktop_id {
                if let Some(desktop) = wm.window_registry.get_mut(&desktop_id) {
                    desktop.remove_child(menu_id);
                }
            }

            wm.destroy_window(menu_id);
            wm.set_active_menu(None);
            wm.force_full_repaint();
        });

        crate::debug_info!("GUIShell: Start menu closed");
    }
}

/// Spawn a new terminal.
///
/// U8 lifted the single-user-app restriction: each terminal gets its
/// own kernel-thread launcher + ring-3 zsh, and the scheduler
/// round-robins between them. Concurrent terminals are now expected.
fn spawn_terminal() {
    match crate::window::terminal_factory::spawn_terminal_with_shell() {
        Ok(instance) => {
            crate::debug_info!("GUIShell: Spawned terminal {:?}", instance.terminal_id);
        }
        Err(e) => {
            crate::debug_warn!("GUIShell: Failed to spawn terminal: {}", e);
        }
    }
}

/// Spawn the standalone ring-3 painting ELF on a blocking kernel wrapper
/// thread, the same path Start → Notepad uses.
fn spawn_painting() {
    crate::debug_info!("GUIShell: Spawning painting...");
    spawn_gui_user_app("/host/PAINTING.ELF", "painting");
}

fn spawn_calc() {
    crate::debug_info!("GUIShell: Spawning calc...");
    spawn_gui_user_app("/host/CALC.ELF", "calc");
}

fn spawn_glgame() {
    crate::debug_info!("GUIShell: Spawning GL Arena...");
    spawn_gui_user_app("/host/GLGAME.ELF", "glgame");
}

fn spawn_notepad() {
    crate::debug_info!("GUIShell: Spawning notepad...");
    spawn_gui_user_app("/host/NOTEPAD.ELF", "notepad");
}

fn spawn_taskmgr() {
    crate::debug_info!("GUIShell: Spawning task manager...");
    spawn_gui_user_app("/host/TASKMGR.ELF", "taskmgr");
}

fn spawn_file_manager() {
    crate::debug_info!("GUIShell: Spawning file manager...");
    spawn_gui_user_app("/host/FILEMAN.ELF", "explorer");
}

fn spawn_gui_user_app(path: &'static str, name: &'static str) {
    use crate::userland::process_service::{LaunchOutcome, LaunchSpec, DEFAULT_USER_ENV};

    let argv = [name];
    let spec =
        LaunchSpec::new(path, &argv, &DEFAULT_USER_ENV).on_complete(Box::new(move |outcome| {
            match outcome {
                LaunchOutcome::Exited {
                    pid, kind, code, ..
                } => crate::debug_info!(
                    "GUI user app {} pid={} exited: kind={:?}, code={}",
                    path,
                    pid,
                    kind,
                    code
                ),
                LaunchOutcome::Failed { error, .. } => {
                    crate::debug_error!("GUI user app {} failed: {}", path, error);
                    let message = alloc::format!("Could not start {}: {}", name, error);
                    crate::window::dialogs::show_error("Start", &message);
                }
            }
        }));
    if let Err(error) = crate::userland::process_service::submit(spec) {
        crate::debug_error!("Could not submit {}: {}", path, error);
        let message = alloc::format!("Could not start {}: {}", name, error);
        crate::window::dialogs::show_error("Start", &message);
    }
}

fn spawn_run_command(command: alloc::string::String) {
    use crate::userland::process_service::{
        LaunchOutcome, LaunchSpec, DEFAULT_USER_ENV, ZSH_HOST_PATH,
    };

    let argv = run_command_argv(command.as_str());
    let reported_command = command.clone();
    let spec = LaunchSpec::new(ZSH_HOST_PATH, &argv, &DEFAULT_USER_ENV)
        .with_cwd("/host")
        .on_complete(Box::new(move |outcome| match outcome {
            LaunchOutcome::Exited { kind, code: 0, .. } => {
                crate::debug_info!("Run command exited successfully: kind={:?}", kind);
            }
            LaunchOutcome::Exited { kind, code, .. } => {
                crate::debug_warn!(
                    "Run command failed: command={:?}, kind={:?}, code={}",
                    reported_command,
                    kind,
                    code
                );
                let message =
                    alloc::format!("Command exited with status {code}: {reported_command}");
                crate::window::dialogs::show_error("Run", &message);
            }
            LaunchOutcome::Failed { error, .. } => {
                crate::debug_error!(
                    "Run command {:?} failed to launch: {}",
                    reported_command,
                    error
                );
                let message = alloc::format!("Could not run {reported_command}: {error}");
                crate::window::dialogs::show_error("Run", &message);
            }
        }));
    if let Err(error) = crate::userland::process_service::submit(spec) {
        let message = alloc::format!("Could not run {command}: {error}");
        crate::window::dialogs::show_error("Run", &message);
    }
}

pub(crate) fn run_command_argv(command: &str) -> [&str; 3] {
    [
        crate::userland::process_service::ZSH_HOST_PATH,
        "-c",
        command,
    ]
}

/// Toggle the Start menu (show if hidden, hide if shown)
fn toggle_start_menu() {
    let menu_open = GUISHELL_STATE.lock().menu_id.is_some();
    crate::debug_trace!(
        "GUIShell: toggle_start_menu called, menu_open={}",
        menu_open
    );
    if menu_open {
        close_start_menu();
    } else {
        show_start_menu();
    }
}

/// Sync taskbar buttons with current frame windows
fn sync_taskbar_buttons() {
    let state = GUISHELL_STATE.lock();
    if state.taskbar_id.is_none() || state.desktop_id.is_none() {
        return;
    }
    let current_buttons: Vec<(WindowId, WindowId)> = state.window_buttons.clone();
    drop(state);

    // Get current frame windows
    let frame_windows = with_window_manager(|wm| wm.get_frame_windows()).unwrap_or_else(Vec::new);

    // Find frame windows that need buttons (with their titles)
    let mut frames_needing_buttons: Vec<(WindowId, alloc::string::String)> = Vec::new();
    let mut buttons_to_remove: Vec<WindowId> = Vec::new();

    // Check which frames need new buttons
    for (frame_id, title) in &frame_windows {
        let has_button = current_buttons.iter().any(|(_, fid)| fid == frame_id);
        if !has_button {
            frames_needing_buttons.push((*frame_id, title.clone()));
        }
    }

    // Check which buttons need to be removed (frame no longer exists)
    for (button_id, frame_id) in &current_buttons {
        let frame_exists = frame_windows.iter().any(|(fid, _)| fid == frame_id);
        if !frame_exists {
            buttons_to_remove.push(*button_id);
        }
    }

    // Add new buttons with their actual titles
    for (frame_id, title) in frames_needing_buttons {
        add_window_button(frame_id, &title);
    }

    // Remove old buttons
    for button_id in buttons_to_remove {
        remove_window_button(button_id);
    }

    // Update button layout
    update_button_layout();
}

/// Add a window button to the taskbar
fn add_window_button(frame_id: WindowId, title: &str) {
    let (taskbar_id, button_count) = {
        let state = GUISHELL_STATE.lock();
        let taskbar_id = match state.taskbar_id {
            Some(id) => id,
            None => return,
        };
        (taskbar_id, state.window_buttons.len() + 1)
    };

    let button_id = with_window_manager(|wm| {
        // Create a new button
        let button_id = wm.create_window(Some(taskbar_id));

        // Calculate initial position (will be updated by layout)
        let (screen_width, _) = wm.screen_dimensions();
        let bounds = window_button_bounds(screen_width, button_count, button_count - 1);

        // Truncate title if too long
        let display_title = if title.len() > 12 {
            &title[..12]
        } else {
            title
        };

        let mut button = Button::new_with_id(button_id, bounds, display_title);
        button.set_parent(Some(taskbar_id));

        // Set up click callback to focus the window
        // Use deferred action to avoid deadlock
        let focus_frame_id = frame_id;
        button.on_click(move || {
            queue_action(PendingAction::FocusWindow(focus_frame_id));
        });

        wm.set_window_impl(button_id, Box::new(button));

        // Add to taskbar's children
        if let Some(taskbar) = wm.window_registry.get_mut(&taskbar_id) {
            taskbar.add_child(button_id);
        }

        button_id
    });

    if let Some(button_id) = button_id {
        let mut state = GUISHELL_STATE.lock();
        state.window_buttons.push((button_id, frame_id));
        crate::debug_trace!(
            "GUIShell: Added window button {:?} for frame {:?}",
            button_id,
            frame_id
        );
    }
}

/// Remove a window button from the taskbar
fn remove_window_button(button_id: WindowId) {
    let taskbar_id = {
        let mut state = GUISHELL_STATE.lock();
        let taskbar_id = match state.taskbar_id {
            Some(id) => id,
            None => return,
        };

        // Remove from state before destroying the corresponding window.
        state.window_buttons.retain(|(bid, _)| *bid != button_id);
        taskbar_id
    };

    with_window_manager(|wm| {
        // Remove from taskbar's children
        if let Some(taskbar) = wm.window_registry.get_mut(&taskbar_id) {
            taskbar.remove_child(button_id);
        }

        // Destroy the button window
        wm.destroy_window(button_id);
        wm.force_full_repaint();
    });

    crate::debug_trace!("GUIShell: Removed window button {:?}", button_id);
}

/// Update the layout of window buttons on the taskbar
fn update_button_layout() {
    let state = GUISHELL_STATE.lock();
    let buttons: Vec<(WindowId, WindowId)> = state.window_buttons.clone();
    drop(state);

    if buttons.is_empty() {
        return;
    }

    with_window_manager(|wm| {
        let (screen_width, _) = wm.screen_dimensions();

        for (i, (button_id, _)) in buttons.iter().enumerate() {
            let bounds = window_button_bounds(screen_width, buttons.len(), i);

            if let Some(button) = wm.window_registry.get_mut(button_id) {
                button.set_bounds(bounds);
            }
        }
    });
}

/// Focus a window (called when taskbar button is clicked)
fn focus_window(frame_id: WindowId) {
    with_window_manager(|wm| {
        // Bring to front
        wm.bring_to_front(frame_id);

        // Focus the frame (for blue title bar)
        if let Some(frame) = wm.window_registry.get_mut(&frame_id) {
            frame.set_focus(true);
        }

        // Focus the content (terminal) for keyboard input
        if let Some(frame) = wm.window_registry.get(&frame_id) {
            if let Some(&content_id) = frame.children().first() {
                wm.focus_window(content_id);
            }
        }

        crate::debug_info!("GUIShell: Focused window {:?}", frame_id);
    });
}

// =============================================================================
// Process-Based GUIShell
// =============================================================================

/// Ticks between periodic taskbar syncs (10 = 100ms at 100Hz timer)
const TASKBAR_SYNC_INTERVAL: u64 = 10;

/// Spawn the GUIShell as a background process
///
/// This creates a process that sleeps until events occur, reducing CPU usage
/// when the system is idle. The process handles:
/// - Pending actions (menu clicks, spawning applications)
/// - Taskbar button synchronization (periodically)
pub fn spawn_guishell_process() {
    use alloc::string::String;

    let pid = crate::process::spawn_process(
        String::from("guishell"),
        None, // No terminal - GUIShell doesn't need one
        guishell_process_main,
    );

    // Store the process ID so we can signal it
    GUISHELL_STATE.lock().process_id = Some(pid);

    crate::debug_info!("GUIShell: Spawned background process with PID {:?}", pid);
}

/// Main entry point for the GUIShell process
fn guishell_process_main() {
    crate::debug_info!("GUIShell process: Starting main loop");

    loop {
        // Check if initialized - if not, wait
        if !GUISHELL_STATE.lock().initialized {
            // Wait a bit and check again
            crate::process::sleep_ms(100);
            continue;
        }

        // Process any pending actions
        process_pending_actions();

        // Sync taskbar buttons
        sync_taskbar_buttons();

        // Sleep until:
        // - A window event occurs (button click, etc.)
        // - Timer tick for periodic sync
        // We use sleep_ticks instead of sleep_until_event for periodic updates
        crate::process::sleep_ticks(TASKBAR_SYNC_INTERVAL);
    }
}

/// Process pending actions (called from the GUIShell process)
fn process_pending_actions() {
    // First, sync menu state with window manager
    let menu_id = GUISHELL_STATE.lock().menu_id;
    if let Some(menu_id) = menu_id {
        let menu_exists =
            with_window_manager(|wm| wm.window_registry.contains_key(&menu_id)).unwrap_or(false);

        if !menu_exists {
            let mut state = GUISHELL_STATE.lock();
            if state.menu_id == Some(menu_id) {
                crate::debug_info!(
                    "GUIShell: Menu {:?} was destroyed externally, clearing state",
                    menu_id
                );
                state.menu_id = None;
            }
        }
    }

    // Run dialogs are non-blocking so the GUIShell process can keep taskbar
    // buttons synchronized while the modal is open.
    if let Some(result) = crate::window::dialogs::poll_run_dialog() {
        if let Some(command) = result {
            spawn_run_command(command);
        }
    }

    // Take any pending action
    let pending_action = {
        let mut state = GUISHELL_STATE.lock();
        state.pending_action.take()
    };

    // Process pending action (outside the lock to avoid deadlocks)
    if let Some(action) = pending_action {
        match action {
            PendingAction::ToggleStartMenu => {
                toggle_start_menu();
            }
            PendingAction::SpawnTerminal => {
                close_start_menu();
                spawn_terminal();
            }
            PendingAction::SpawnPainting => {
                close_start_menu();
                spawn_painting();
            }
            PendingAction::SpawnCalc => {
                close_start_menu();
                spawn_calc();
            }
            PendingAction::SpawnGlGame => {
                close_start_menu();
                spawn_glgame();
            }
            PendingAction::SpawnNotepad => {
                close_start_menu();
                spawn_notepad();
            }
            PendingAction::SpawnTaskmgr => {
                close_start_menu();
                spawn_taskmgr();
            }
            PendingAction::SpawnFileManager => {
                close_start_menu();
                spawn_file_manager();
            }
            PendingAction::OpenRunDialog => {
                close_start_menu();
                if let Err(error) = crate::window::dialogs::open_run_dialog() {
                    crate::debug_warn!("GUIShell: could not open Run dialog: {}", error);
                }
            }
            PendingAction::ShowShutdownNotice => {
                close_start_menu();
                crate::window::dialogs::show_info(
                    "Shut Down",
                    "Shutdown is not available yet. Close the QEMU window to stop AgenticOS.",
                );
            }
            PendingAction::FocusWindow(frame_id) => {
                focus_window(frame_id);
            }
        }
    }
}

/// Signal the GUIShell process to wake up
///
/// Call this when window events occur that might need GUIShell attention.
pub fn signal_guishell() {
    let process_id = GUISHELL_STATE.lock().process_id;
    if let Some(pid) = process_id {
        crate::process::signal_process(pid, WakeEvents::WINDOW_EVENT);
    }
}

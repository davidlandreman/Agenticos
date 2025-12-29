//! GUIShell - Graphical shell with taskbar and Start menu
//!
//! This module manages the desktop environment with a taskbar at the bottom
//! of the screen, a Start button that opens a menu, and buttons for open windows.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::window::{self, WindowId, Rect, Window};
use crate::window::windows::{DesktopWindow, TaskbarWindow, Button, MenuWindow, FrameWindow};
use crate::window::windows::taskbar::{TASKBAR_HEIGHT, START_BUTTON_WIDTH, BUTTON_GAP, BUTTON_HEIGHT, BUTTON_Y_OFFSET, MAX_WINDOW_BUTTON_WIDTH};
use crate::window::windows::menu::MENU_ITEM_HEIGHT;

/// GUIShell state
pub struct GUIShellState {
    /// Desktop window ID
    pub desktop_id: Option<WindowId>,
    /// Taskbar window ID
    pub taskbar_id: Option<WindowId>,
    /// Start button ID
    pub start_button_id: Option<WindowId>,
    /// Current menu ID (if open)
    pub menu_id: Option<WindowId>,
    /// Tracked window buttons: (button_id, frame_id)
    pub window_buttons: Vec<(WindowId, WindowId)>,
    /// Whether the GUI shell is initialized
    pub initialized: bool,
    /// Deferred action to perform in next poll
    pub pending_action: Option<PendingAction>,
}

/// Actions that need to be deferred to avoid deadlocks
#[derive(Clone)]
pub enum PendingAction {
    ToggleStartMenu,
    SpawnTerminal,
    SpawnPainting,
    SpawnCalc,
    FocusWindow(WindowId),
}

impl GUIShellState {
    pub const fn new() -> Self {
        GUIShellState {
            desktop_id: None,
            taskbar_id: None,
            start_button_id: None,
            menu_id: None,
            window_buttons: Vec::new(),
            initialized: false,
            pending_action: None,
        }
    }
}

/// Queue a deferred action to be processed in the next poll
pub fn queue_action(action: PendingAction) {
    // Use lock() instead of try_lock() to ensure the action is always queued
    // try_lock() was silently dropping actions when the lock was briefly held
    let mut state = GUISHELL_STATE.lock();
    crate::debug_info!("GUIShell: Queuing action {:?}", core::mem::discriminant(&action));
    state.pending_action = Some(action);
}

/// Global GUIShell state
static GUISHELL_STATE: Mutex<GUIShellState> = Mutex::new(GUIShellState::new());

/// Initialize the GUIShell desktop environment
pub fn init_guishell() {
    let mut state = GUISHELL_STATE.lock();
    if state.initialized {
        return;
    }

    window::with_window_manager(|wm| {
        // Get screen dimensions
        let width = wm.graphics_device.width() as u32;
        let height = wm.graphics_device.height() as u32;

        // Create GUI screen
        let screen_id = wm.create_screen(window::ScreenMode::Gui);
        wm.switch_screen(screen_id);

        // Create desktop background window
        let desktop_id = wm.create_window(None);
        let desktop_window = Box::new(DesktopWindow::new(desktop_id, Rect::new(0, 0, width, height)));
        wm.set_window_impl(desktop_id, desktop_window);

        // Set desktop as the root window for the screen
        if let Some(screen) = wm.get_active_screen_mut() {
            screen.set_root_window(desktop_id);
        }

        state.desktop_id = Some(desktop_id);

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
        state.taskbar_id = Some(taskbar_id);

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

        // Update taskbar with start button ID
        // We need to get it as a TaskbarWindow to call set_start_button
        // But since we can't downcast, we'll track it in state instead
        state.start_button_id = Some(start_button_id);

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

        // Force a full screen repaint
        wm.force_full_repaint();

        crate::debug_info!("GUIShell: Desktop initialized (desktop={:?}, taskbar={:?}, start={:?})",
            desktop_id, taskbar_id, start_button_id);
    });

    state.initialized = true;
}

/// Show the Start menu
fn show_start_menu() {
    let mut state = GUISHELL_STATE.lock();

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

    window::with_window_manager(|wm| {
        // Get taskbar position
        let taskbar_bounds = wm.window_registry.get(&taskbar_id)
            .map(|w| w.bounds())
            .unwrap_or(Rect::new(0, 0, 0, 0));

        // Calculate menu dimensions
        let menu_items = 3; // Terminal, Painting, Calc
        let menu_width = 120u32;
        let menu_height = (menu_items * MENU_ITEM_HEIGHT as usize + 4) as u32;
        let menu_x = BUTTON_GAP as i32;
        let menu_y = taskbar_bounds.y - menu_height as i32;

        // Create menu window as child of desktop (so it's in the render hierarchy)
        let menu_id = wm.create_window(Some(desktop_id));
        let mut menu = MenuWindow::new_with_id(menu_id, Rect::new(menu_x, menu_y, menu_width, menu_height));
        menu.set_parent(Some(desktop_id));

        // Add menu items
        menu.add_item("Terminal");
        menu.add_item("Painting");
        menu.add_item("Calc");

        // Set up callback for menu selection
        // Use deferred actions to avoid deadlock
        menu.on_select(|index| {
            match index {
                0 => queue_action(PendingAction::SpawnTerminal),
                1 => queue_action(PendingAction::SpawnPainting),
                2 => queue_action(PendingAction::SpawnCalc),
                _ => {}
            }
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

        state.menu_id = Some(menu_id);

        crate::debug_info!("GUIShell: Start menu opened (menu={:?})", menu_id);
    });
}

/// Close the Start menu
fn close_start_menu() {
    let mut state = GUISHELL_STATE.lock();

    if let Some(menu_id) = state.menu_id.take() {
        let desktop_id = state.desktop_id;

        window::with_window_manager(|wm| {
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

/// Spawn a new terminal
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

/// Spawn the painting application
fn spawn_painting() {
    crate::debug_info!("GUIShell: Spawning painting...");
    if let Err(e) = crate::process::execute_command("painting", None) {
        crate::debug_warn!("GUIShell: Failed to spawn painting: {:?}", e);
    }
}

/// Spawn the calculator
fn spawn_calc() {
    crate::debug_info!("GUIShell: Spawning calc...");
    if let Err(e) = crate::process::execute_command("calc", None) {
        crate::debug_warn!("GUIShell: Failed to spawn calc: {:?}", e);
    }
}

/// Poll the GUIShell - updates taskbar buttons and handles events
pub fn poll() {
    // First, sync menu state with window manager
    // The window manager may have closed the menu via click-outside detection
    // without notifying us, so we need to check if our menu_id is still valid
    {
        let mut state = GUISHELL_STATE.lock();
        if let Some(menu_id) = state.menu_id {
            // Check if the menu window still exists
            let menu_exists = window::with_window_manager(|wm| {
                wm.window_registry.contains_key(&menu_id)
            }).unwrap_or(false);

            if !menu_exists {
                crate::debug_info!("GUIShell: Menu {:?} was destroyed externally, clearing state", menu_id);
                state.menu_id = None;
            }
        }
    }

    // Take any pending action
    let pending_action = {
        let mut state = GUISHELL_STATE.lock();

        if !state.initialized {
            return;
        }

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
            PendingAction::FocusWindow(frame_id) => {
                focus_window(frame_id);
            }
        }
    }

    // Sync window buttons with current frame windows
    sync_taskbar_buttons();
}

/// Toggle the Start menu (show if hidden, hide if shown)
fn toggle_start_menu() {
    let menu_open = GUISHELL_STATE.lock().menu_id.is_some();
    crate::debug_info!("GUIShell: toggle_start_menu called, menu_open={}", menu_open);
    if menu_open {
        close_start_menu();
    } else {
        show_start_menu();
    }
}

/// Sync taskbar buttons with current frame windows
fn sync_taskbar_buttons() {
    let state = GUISHELL_STATE.lock();
    let taskbar_id = match state.taskbar_id {
        Some(id) => id,
        None => return,
    };
    let desktop_id = match state.desktop_id {
        Some(id) => id,
        None => return,
    };
    let current_buttons: Vec<(WindowId, WindowId)> = state.window_buttons.clone();
    drop(state);

    // Get current frame windows
    let frame_windows = window::with_window_manager(|wm| {
        wm.get_frame_windows()
    }).unwrap_or_else(Vec::new);

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
    let mut state = GUISHELL_STATE.lock();
    let taskbar_id = match state.taskbar_id {
        Some(id) => id,
        None => return,
    };

    let button_id = window::with_window_manager(|wm| {
        // Create a new button
        let button_id = wm.create_window(Some(taskbar_id));

        // Calculate initial position (will be updated by layout)
        let button_count = state.window_buttons.len() as u32;
        let start_x = BUTTON_GAP + START_BUTTON_WIDTH + BUTTON_GAP;
        let x = start_x + button_count * (MAX_WINDOW_BUTTON_WIDTH + BUTTON_GAP);

        let bounds = Rect::new(
            x as i32,
            BUTTON_Y_OFFSET as i32,
            MAX_WINDOW_BUTTON_WIDTH.min(100),
            BUTTON_HEIGHT,
        );

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
        state.window_buttons.push((button_id, frame_id));
        crate::debug_trace!("GUIShell: Added window button {:?} for frame {:?}", button_id, frame_id);
    }
}

/// Remove a window button from the taskbar
fn remove_window_button(button_id: WindowId) {
    let mut state = GUISHELL_STATE.lock();
    let taskbar_id = match state.taskbar_id {
        Some(id) => id,
        None => return,
    };

    // Remove from state
    state.window_buttons.retain(|(bid, _)| *bid != button_id);

    window::with_window_manager(|wm| {
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
    let taskbar_id = state.taskbar_id;
    drop(state);

    if buttons.is_empty() {
        return;
    }

    window::with_window_manager(|wm| {
        // Get screen width for layout calculation
        let screen_width = wm.graphics_device.width() as u32;

        // Calculate button dimensions
        let start_x = BUTTON_GAP + START_BUTTON_WIDTH + BUTTON_GAP;
        let available_width = screen_width.saturating_sub(start_x + BUTTON_GAP);
        let button_count = buttons.len() as u32;
        let total_gaps = button_count.saturating_sub(1) * BUTTON_GAP;
        let available_for_buttons = available_width.saturating_sub(total_gaps);
        let button_width = (available_for_buttons / button_count).min(MAX_WINDOW_BUTTON_WIDTH);

        for (i, (button_id, _)) in buttons.iter().enumerate() {
            let x = start_x + (i as u32 * (button_width + BUTTON_GAP));
            let bounds = Rect::new(
                x as i32,
                BUTTON_Y_OFFSET as i32,
                button_width,
                BUTTON_HEIGHT,
            );

            if let Some(button) = wm.window_registry.get_mut(button_id) {
                button.set_bounds(bounds);
                button.invalidate();
            }
        }
    });
}

/// Focus a window (called when taskbar button is clicked)
fn focus_window(frame_id: WindowId) {
    window::with_window_manager(|wm| {
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

/// Check if the Start button was clicked and handle it
pub fn check_start_button_click() {
    // This would be called from the event loop if we detect the start button was clicked
    // For now, we use the button's on_click callback instead
}

/// Get the desktop window ID
pub fn get_desktop_id() -> Option<WindowId> {
    GUISHELL_STATE.lock().desktop_id
}

/// Get the taskbar window ID
pub fn get_taskbar_id() -> Option<WindowId> {
    GUISHELL_STATE.lock().taskbar_id
}

/// Check if GUIShell is initialized
pub fn is_initialized() -> bool {
    GUISHELL_STATE.lock().initialized
}

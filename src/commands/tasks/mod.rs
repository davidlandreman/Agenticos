//! Task Manager command - Windows Task Manager-style GUI application
//!
//! Displays running processes with metrics and allows killing processes
//! via a right-click context menu.

use crate::process::{BaseProcess, HasBaseProcess, RunnableProcess, ProcessId, ProcessState};
use crate::window::{self, Window, WindowId, Rect, Point};
use crate::window::windows::{ContainerWindow, FrameWindow, MultiColumnList, Column, MenuWindow};
use crate::graphics::color::Color;
use alloc::{vec, vec::Vec, string::String, boxed::Box, format};
use alloc::collections::BTreeMap;
use spin::Mutex;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Heap size for memory % calculation (100 MiB)
const TOTAL_HEAP_SIZE: usize = 100 * 1024 * 1024;

/// Unique ID for each task manager instance
static NEXT_TASKS_ID: AtomicUsize = AtomicUsize::new(1);

/// Task manager state for a single instance
struct TasksState {
    /// Frame window ID
    frame_id: Option<WindowId>,
    /// Content window ID
    content_id: Option<WindowId>,
    /// Process list window ID
    list_id: Option<WindowId>,
    /// Active context menu window ID
    context_menu_id: Option<WindowId>,
    /// Currently selected process PID (for context menu actions)
    selected_pid: Option<ProcessId>,
    /// Last refresh tick
    last_refresh_tick: u64,
    /// Whether task manager is running
    running: bool,
}

impl TasksState {
    fn new() -> Self {
        TasksState {
            frame_id: None,
            content_id: None,
            list_id: None,
            context_menu_id: None,
            selected_pid: None,
            last_refresh_tick: 0,
            running: true,
        }
    }
}

/// Global map of task manager states, keyed by instance ID
static TASKS_STATES: Mutex<BTreeMap<usize, TasksState>> = Mutex::new(BTreeMap::new());

/// Pending action for deferred execution (avoids deadlocks)
enum PendingAction {
    ShowContextMenu(usize, usize, Point), // (tasks_id, row_index, position)
    KillProcess(usize),                    // (tasks_id)
    CloseContextMenu(usize),               // (tasks_id)
}

/// Pending action queue
static PENDING_ACTION: Mutex<Option<PendingAction>> = Mutex::new(None);

/// Queue a pending action for deferred execution
fn queue_action(action: PendingAction) {
    let mut pending = PENDING_ACTION.lock();
    *pending = Some(action);
}

/// Process pending actions
fn process_pending_actions() {
    let action = {
        let mut pending = PENDING_ACTION.lock();
        pending.take()
    };

    if let Some(action) = action {
        match action {
            PendingAction::ShowContextMenu(tasks_id, row_index, position) => {
                show_context_menu(tasks_id, row_index, position);
            }
            PendingAction::KillProcess(tasks_id) => {
                kill_selected_process(tasks_id);
            }
            PendingAction::CloseContextMenu(tasks_id) => {
                close_context_menu(tasks_id);
            }
        }
    }
}

/// Show context menu at the given position
fn show_context_menu(tasks_id: usize, row_index: usize, position: Point) {
    // Close any existing context menu first
    close_context_menu(tasks_id);

    // Get the selected PID from the row
    let processes = crate::process::get_process_list();
    let selected_pid = processes.get(row_index).map(|p| p.pid);

    // Store the selected PID
    {
        let mut states = TASKS_STATES.lock();
        if let Some(state) = states.get_mut(&tasks_id) {
            state.selected_pid = selected_pid;
        }
    }

    let Some(selected_pid) = selected_pid else { return };

    // Don't allow killing the current process (the task manager itself)
    let current_pid = crate::process::SCHEDULER.lock().current();
    if current_pid == Some(selected_pid) {
        return; // Can't kill ourselves
    }

    // Create context menu
    let menu_id = window::with_window_manager(|wm| {
        let desktop_id = wm.get_active_screen()
            .and_then(|s| s.root_window)?;

        let menu_id = wm.create_window(Some(desktop_id));
        let menu_width = 120;
        let menu_height = 28; // Single item
        let mut menu = MenuWindow::new_with_id(
            menu_id,
            Rect::new(position.x, position.y, menu_width, menu_height),
        );
        menu.add_item("Kill Process");
        menu.set_parent(Some(desktop_id));

        // Set selection callback
        let local_tasks_id = tasks_id;
        menu.on_select(move |item_index| {
            if item_index == 0 {
                queue_action(PendingAction::KillProcess(local_tasks_id));
            }
            queue_action(PendingAction::CloseContextMenu(local_tasks_id));
        });

        wm.set_window_impl(menu_id, Box::new(menu));
        wm.set_active_menu(Some(menu_id));

        // Add to desktop's children
        if let Some(desktop) = wm.window_registry.get_mut(&desktop_id) {
            desktop.add_child(menu_id);
        }

        wm.force_full_repaint();

        Some(menu_id)
    });

    // Store menu ID
    if let Some(Some(menu_id)) = menu_id {
        let mut states = TASKS_STATES.lock();
        if let Some(state) = states.get_mut(&tasks_id) {
            state.context_menu_id = Some(menu_id);
        }
    }
}

/// Close context menu if open
fn close_context_menu(tasks_id: usize) {
    let menu_id = {
        let mut states = TASKS_STATES.lock();
        if let Some(state) = states.get_mut(&tasks_id) {
            state.context_menu_id.take()
        } else {
            None
        }
    };

    if let Some(menu_id) = menu_id {
        window::with_window_manager(|wm| {
            // Get parent before removing
            if let Some(menu) = wm.window_registry.get(&menu_id) {
                if let Some(parent_id) = menu.parent() {
                    if let Some(parent) = wm.window_registry.get_mut(&parent_id) {
                        parent.remove_child(menu_id);
                    }
                }
            }
            wm.window_registry.remove(&menu_id);
            wm.set_active_menu(None);
            wm.force_full_repaint();
        });
    }
}

/// Kill the selected process
fn kill_selected_process(tasks_id: usize) {
    let pid = {
        let states = TASKS_STATES.lock();
        states.get(&tasks_id).and_then(|s| s.selected_pid)
    };

    if let Some(pid) = pid {
        // Don't allow killing the current process
        let current_pid = crate::process::SCHEDULER.lock().current();
        if current_pid != Some(pid) {
            crate::process::terminate_process(pid);
        }
    }
}

/// Format process state as string
fn state_to_string(state: ProcessState) -> &'static str {
    match state {
        ProcessState::Ready => "Ready",
        ProcessState::Running => "Running",
        ProcessState::Blocked => "Blocked",
        ProcessState::Terminated => "Terminated",
    }
}

/// Format CPU time (ticks to seconds)
fn format_cpu_time(ticks: u64) -> String {
    // Timer runs at 100Hz, so 100 ticks = 1 second
    let seconds = ticks / 100;
    let tenths = (ticks % 100) / 10;
    format!("{}.{}s", seconds, tenths)
}

/// Format stack size in KB
fn format_stack_size(bytes: usize) -> String {
    format!("{} KB", bytes / 1024)
}

/// Format memory percentage
fn format_mem_percent(stack_size: usize) -> String {
    // stack_size is typically 64KB = 65536 bytes
    // TOTAL_HEAP_SIZE is 100MB = 104857600 bytes
    // Percentage = (stack_size * 100) / TOTAL_HEAP_SIZE
    let percent = (stack_size as u64 * 1000) / (TOTAL_HEAP_SIZE as u64); // tenths of percent
    format!("{}.{}%", percent / 10, percent % 10)
}

/// Refresh the process list
fn refresh_process_list(tasks_id: usize) {
    // Update CPU percentages first (using 50 ticks as elapsed time)
    crate::process::update_cpu_percentages(50);

    // Get process list
    let processes = crate::process::get_process_list();

    // Get list ID
    let (list_id, content_id) = {
        let states = TASKS_STATES.lock();
        if let Some(state) = states.get(&tasks_id) {
            (state.list_id, state.content_id)
        } else {
            return;
        }
    };

    let Some(list_id) = list_id else { return };
    let Some(content_id) = content_id else { return };

    // We need to recreate the list with new data since we can't downcast
    window::with_window_manager(|wm| {
        // Get current list bounds
        let list_bounds = if let Some(list) = wm.window_registry.get(&list_id) {
            list.bounds()
        } else {
            return;
        };

        // Remove old list from content's children
        if let Some(content) = wm.window_registry.get_mut(&content_id) {
            content.remove_child(list_id);
        }

        // Remove old list
        wm.window_registry.remove(&list_id);

        // Create new list with updated data
        let columns = vec![
            Column::new("PID", 50),
            Column::new("Name", 100),
            Column::new("State", 80),
            Column::new("CPU Time", 90),
            Column::new("Stack", 70),
            Column::new("CPU%", 55),
            Column::new("Mem%", 55),
        ];

        let mut new_list = MultiColumnList::new_with_id(list_id, list_bounds, columns);
        new_list.set_parent(Some(content_id));

        // Add rows for each process
        for process in &processes {
            let row = vec![
                format!("{}", process.pid),
                process.name.clone(),
                String::from(state_to_string(process.state)),
                format_cpu_time(process.total_runtime),
                format_stack_size(process.stack_size),
                format!("{}%", process.cpu_percentage),
                format_mem_percent(process.stack_size),
            ];
            new_list.add_row(row);
        }

        // Set right-click callback
        let local_tasks_id = tasks_id;
        new_list.on_right_click(move |row_index, position| {
            queue_action(PendingAction::ShowContextMenu(local_tasks_id, row_index, position));
        });

        // Register new list
        wm.set_window_impl(list_id, Box::new(new_list));

        // Add to content's children
        if let Some(content) = wm.window_registry.get_mut(&content_id) {
            content.add_child(list_id);
        }
    });
}

pub struct TasksProcess {
    base: BaseProcess,
    args: Vec<String>,
}

impl TasksProcess {
    pub fn new_with_args(args: Vec<String>) -> Self {
        Self {
            base: BaseProcess::new("tasks"),
            args,
        }
    }
}

impl HasBaseProcess for TasksProcess {
    fn base(&self) -> &BaseProcess {
        &self.base
    }

    fn base_mut(&mut self) -> &mut BaseProcess {
        &mut self.base
    }
}

impl RunnableProcess for TasksProcess {
    fn run(&mut self) {
        // Generate unique ID for this task manager instance
        let tasks_id = NEXT_TASKS_ID.fetch_add(1, Ordering::SeqCst);

        // Create state for this instance
        {
            let mut states = TASKS_STATES.lock();
            let mut state = TasksState::new();
            state.last_refresh_tick = crate::arch::x86_64::interrupts::get_timer_ticks();
            states.insert(tasks_id, state);
        }

        // Window dimensions
        let frame_width = 600;
        let frame_height = 400;

        // Offset each instance slightly
        let offset = ((tasks_id - 1) % 5) as i32 * 30;

        // Create frame + container + list window
        let result = window::with_window_manager(|wm| {
            // Get the desktop window
            let desktop_id = wm.get_active_screen()
                .and_then(|s| s.root_window)?;

            // Create frame window
            let frame_id = wm.create_window(Some(desktop_id));
            let mut frame = FrameWindow::new(frame_id, "Task Manager");
            frame.set_bounds(Rect::new(100 + offset, 50 + offset, frame_width, frame_height));
            frame.set_parent(Some(desktop_id));

            // Create container as content
            let content_id = wm.create_window(Some(frame_id));
            let content_bounds = frame.content_area();
            let mut content = ContainerWindow::new_with_id(content_id, content_bounds);
            content.set_background_color(Color::WHITE);
            content.set_parent(Some(frame_id));

            frame.set_content_window(content_id);

            // Create multi-column list
            let list_id = wm.create_window(Some(content_id));
            let padding = 5;
            let list_bounds = Rect::new(
                padding,
                padding,
                (content_bounds.width as i32 - padding * 2) as u32,
                (content_bounds.height as i32 - padding * 2) as u32,
            );

            let columns = vec![
                Column::new("PID", 50),
                Column::new("Name", 100),
                Column::new("State", 80),
                Column::new("CPU Time", 90),
                Column::new("Stack", 70),
                Column::new("CPU%", 55),
                Column::new("Mem%", 55),
            ];

            let mut list = MultiColumnList::new_with_id(list_id, list_bounds, columns);
            list.set_parent(Some(content_id));

            // Set right-click callback
            let local_tasks_id = tasks_id;
            list.on_right_click(move |row_index, position| {
                queue_action(PendingAction::ShowContextMenu(local_tasks_id, row_index, position));
            });

            // Register windows
            wm.set_window_impl(frame_id, Box::new(frame));
            wm.set_window_impl(content_id, Box::new(content));
            wm.set_window_impl(list_id, Box::new(list));

            // Set up parent-child relationships
            if let Some(desktop) = wm.window_registry.get_mut(&desktop_id) {
                desktop.add_child(frame_id);
            }
            if let Some(frame_win) = wm.window_registry.get_mut(&frame_id) {
                frame_win.add_child(content_id);
            }
            if let Some(content_win) = wm.window_registry.get_mut(&content_id) {
                content_win.add_child(list_id);
            }

            // Focus the window
            wm.focus_window(frame_id);
            if let Some(frame_win) = wm.window_registry.get_mut(&frame_id) {
                frame_win.set_focus(true);
            }

            Some((frame_id, content_id, list_id))
        });

        let (frame_id, content_id, list_id) = match result {
            Some(Some(r)) => r,
            _ => {
                crate::println!("Failed to create Task Manager window");
                return;
            }
        };

        // Store IDs in state
        {
            let mut states = TASKS_STATES.lock();
            if let Some(state) = states.get_mut(&tasks_id) {
                state.frame_id = Some(frame_id);
                state.content_id = Some(content_id);
                state.list_id = Some(list_id);
            }
        }

        // Initial refresh
        refresh_process_list(tasks_id);

        crate::println!("Task Manager started. Close window to exit.");

        // Main loop
        loop {
            let current_tick = crate::arch::x86_64::interrupts::get_timer_ticks();

            let (should_refresh, running) = {
                let states = TASKS_STATES.lock();
                if let Some(state) = states.get(&tasks_id) {
                    let elapsed = current_tick.saturating_sub(state.last_refresh_tick);
                    (elapsed >= 50, state.running) // Refresh every 50 ticks (500ms)
                } else {
                    break;
                }
            };

            if !running {
                break;
            }

            // Process any pending actions
            process_pending_actions();

            // Auto-refresh
            if should_refresh {
                refresh_process_list(tasks_id);

                // Update last refresh tick
                let mut states = TASKS_STATES.lock();
                if let Some(state) = states.get_mut(&tasks_id) {
                    state.last_refresh_tick = current_tick;
                }
            }

            // Small delay
            for _ in 0..50000 {
                core::hint::spin_loop();
            }

            // Allow preemption
            crate::process::yield_if_needed();

            // Check if window still exists
            let exists = window::with_window_manager(|wm| {
                wm.window_registry.contains_key(&frame_id)
            }).unwrap_or(false);

            if !exists {
                break;
            }
        }

        // Cleanup
        close_context_menu(tasks_id);
        {
            let mut states = TASKS_STATES.lock();
            states.remove(&tasks_id);
        }

        crate::println!("Task Manager closed.");
    }

    fn get_name(&self) -> &str {
        "tasks"
    }
}

pub fn create_tasks_process(args: Vec<String>) -> Box<dyn RunnableProcess> {
    Box::new(TasksProcess::new_with_args(args))
}

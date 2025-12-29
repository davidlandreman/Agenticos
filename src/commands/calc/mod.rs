//! Calculator command using GUI widgets

use crate::process::{BaseProcess, HasBaseProcess, RunnableProcess};
use crate::window::{self, Window, WindowId, Rect};
use crate::window::windows::{ContainerWindow, FrameWindow, Label, Button};
use crate::window::windows::label::TextAlign;
use crate::graphics::color::Color;
use alloc::{vec::Vec, string::String, boxed::Box, format};
use alloc::collections::BTreeMap;
use spin::Mutex;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Unique ID for each calculator instance
static NEXT_CALC_ID: AtomicUsize = AtomicUsize::new(1);

/// Calculator state for a single instance
struct CalcState {
    /// Current display value
    display: String,
    /// Accumulator for calculations
    accumulator: f64,
    /// Pending operation
    pending_op: Option<Op>,
    /// Clear display on next digit input
    clear_on_next: bool,
    /// Display window ID for updates
    display_id: Option<WindowId>,
    /// Whether calculator is running
    running: bool,
}

/// Calculator operations
#[derive(Clone, Copy)]
enum Op {
    Add,
    Sub,
    Mul,
    Div,
}

impl CalcState {
    fn new() -> Self {
        CalcState {
            display: String::from("0"),
            accumulator: 0.0,
            pending_op: None,
            clear_on_next: false,
            display_id: None,
            running: true,
        }
    }

    fn reset(&mut self) {
        self.display = String::from("0");
        self.accumulator = 0.0;
        self.pending_op = None;
        self.clear_on_next = false;
    }
}

/// Global map of calculator states, keyed by calculator ID
static CALC_STATES: Mutex<BTreeMap<usize, CalcState>> = Mutex::new(BTreeMap::new());

/// Handle digit button press for a specific calculator
fn handle_digit(calc_id: usize, digit: char) {
    let mut states = CALC_STATES.lock();
    if let Some(state) = states.get_mut(&calc_id) {
        if state.clear_on_next {
            state.display.clear();
            state.clear_on_next = false;
        }

        // Replace leading zero
        if state.display == "0" && digit != '.' {
            state.display.clear();
        }

        // Prevent multiple decimal points
        if digit == '.' && state.display.contains('.') {
            return;
        }

        // Limit display length
        if state.display.len() < 12 {
            state.display.push(digit);
        }
    }
}

/// Handle operator button press for a specific calculator
fn handle_operator(calc_id: usize, op: Op) {
    let mut states = CALC_STATES.lock();
    if let Some(state) = states.get_mut(&calc_id) {
        // Evaluate pending operation first
        if state.pending_op.is_some() && !state.clear_on_next {
            evaluate_pending(state);
        }

        // Parse current display as number
        let current: f64 = state.display.parse().unwrap_or(0.0);
        state.accumulator = current;
        state.pending_op = Some(op);
        state.clear_on_next = true;
    }
}

/// Handle equals button press for a specific calculator
fn handle_equals(calc_id: usize) {
    let mut states = CALC_STATES.lock();
    if let Some(state) = states.get_mut(&calc_id) {
        evaluate_pending(state);
        state.pending_op = None;
    }
}

/// Handle clear button press for a specific calculator
fn handle_clear(calc_id: usize) {
    let mut states = CALC_STATES.lock();
    if let Some(state) = states.get_mut(&calc_id) {
        state.reset();
    }
}

/// Evaluate pending operation
fn evaluate_pending(state: &mut CalcState) {
    if let Some(op) = state.pending_op {
        let current: f64 = state.display.parse().unwrap_or(0.0);

        let result = match op {
            Op::Add => state.accumulator + current,
            Op::Sub => state.accumulator - current,
            Op::Mul => state.accumulator * current,
            Op::Div => {
                if current != 0.0 {
                    state.accumulator / current
                } else {
                    f64::INFINITY
                }
            }
        };

        // Format result
        state.display = if result.is_infinite() || result.is_nan() {
            String::from("Error")
        } else {
            // Check if result is effectively an integer
            let truncated = result as i64;
            let is_integer = (result - truncated as f64).abs() < 1e-10;
            let abs_result = if result < 0.0 { -result } else { result };

            if is_integer && abs_result < 1e10 {
                // Integer result
                format!("{}", truncated)
            } else {
                // Float result, limit decimal places
                let formatted = format!("{:.6}", result);
                // Trim trailing zeros
                let trimmed = formatted.trim_end_matches('0').trim_end_matches('.');
                String::from(trimmed)
            }
        };

        state.accumulator = result;
        state.clear_on_next = true;
    }
}

pub struct CalcProcess {
    base: BaseProcess,
    args: Vec<String>,
}

impl CalcProcess {
    pub fn new_with_args(args: Vec<String>) -> Self {
        Self {
            base: BaseProcess::new("calc"),
            args,
        }
    }
}

impl HasBaseProcess for CalcProcess {
    fn base(&self) -> &BaseProcess {
        &self.base
    }

    fn base_mut(&mut self) -> &mut BaseProcess {
        &mut self.base
    }
}

impl RunnableProcess for CalcProcess {
    fn run(&mut self) {
        // Generate unique ID for this calculator instance
        let calc_id = NEXT_CALC_ID.fetch_add(1, Ordering::SeqCst);

        // Create state for this calculator
        {
            let mut states = CALC_STATES.lock();
            states.insert(calc_id, CalcState::new());
        }

        // Calculator dimensions
        let frame_width = 220;
        let frame_height = 280;
        let button_width = 45;
        let button_height = 35;
        let button_margin = 5;
        let display_height = 40;
        let padding = 10;

        // Offset each calculator slightly so they don't stack exactly
        let offset = ((calc_id - 1) % 5) as i32 * 30;

        // Create frame + container window
        let result = window::with_window_manager(|wm| {
            // Get the desktop window (root of the screen)
            let desktop_id = wm.get_active_screen()
                .and_then(|s| s.root_window)?;

            // Create frame window
            let frame_id = wm.create_window(Some(desktop_id));
            let mut frame = FrameWindow::new(frame_id, "Calculator");
            frame.set_bounds(Rect::new(150 + offset, 80 + offset, frame_width, frame_height));
            frame.set_parent(Some(desktop_id));

            // Create container as content
            let content_id = wm.create_window(Some(frame_id));
            let content_bounds = frame.content_area();
            let mut content = ContainerWindow::new_with_id(content_id, content_bounds);
            content.set_background_color(Color::new(60, 60, 60));
            content.set_parent(Some(frame_id));

            frame.set_content_window(content_id);

            // Create display label
            let display_id = wm.create_window(Some(content_id));
            let display_bounds = Rect::new(
                padding as i32,
                padding as i32,
                (content_bounds.width as i32 - padding as i32 * 2) as u32,
                display_height,
            );
            let mut display = Label::new_with_id(display_id, display_bounds, "0");
            display.set_background(Some(Color::new(40, 40, 40)));
            display.set_color(Color::WHITE);
            display.set_align(TextAlign::Right);
            display.set_parent(Some(content_id));

            // Store display ID in this calculator's state
            {
                let mut states = CALC_STATES.lock();
                if let Some(state) = states.get_mut(&calc_id) {
                    state.display_id = Some(display_id);
                }
            }

            // Button layout: 4 columns x 4 rows
            // Row 0: 7 8 9 /
            // Row 1: 4 5 6 *
            // Row 2: 1 2 3 -
            // Row 3: C 0 = +

            let button_labels = [
                ["7", "8", "9", "/"],
                ["4", "5", "6", "*"],
                ["1", "2", "3", "-"],
                ["C", "0", "=", "+"],
            ];

            let buttons_start_y = padding + display_height as usize + padding;
            let mut button_ids = Vec::new();

            for (row, row_labels) in button_labels.iter().enumerate() {
                for (col, &label) in row_labels.iter().enumerate() {
                    let btn_x = padding + col * (button_width + button_margin);
                    let btn_y = buttons_start_y + row * (button_height + button_margin);

                    let btn_id = wm.create_window(Some(content_id));
                    let btn_bounds = Rect::new(
                        btn_x as i32,
                        btn_y as i32,
                        button_width as u32,
                        button_height as u32,
                    );

                    let mut button = Button::new_with_id(btn_id, btn_bounds, label);
                    button.set_parent(Some(content_id));

                    // Set button colors based on type
                    let is_digit = label.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false);
                    let is_operator = matches!(label, "+" | "-" | "*" | "/");
                    let is_equals = label == "=";
                    let is_clear = label == "C";

                    if is_operator {
                        button.set_bg_color(Color::new(255, 159, 10)); // Orange
                        button.set_text_color(Color::WHITE);
                    } else if is_equals {
                        button.set_bg_color(Color::new(50, 200, 50)); // Green
                        button.set_text_color(Color::WHITE);
                    } else if is_clear {
                        button.set_bg_color(Color::new(200, 50, 50)); // Red
                        button.set_text_color(Color::WHITE);
                    } else if is_digit {
                        button.set_bg_color(Color::new(80, 80, 80));
                        button.set_text_color(Color::WHITE);
                    }

                    // Set up callback based on button type
                    // Each callback captures the calc_id for this specific calculator
                    let label_owned = String::from(label);
                    button.on_click(move || {
                        match label_owned.as_str() {
                            "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" => {
                                handle_digit(calc_id, label_owned.chars().next().unwrap());
                            }
                            "+" => handle_operator(calc_id, Op::Add),
                            "-" => handle_operator(calc_id, Op::Sub),
                            "*" => handle_operator(calc_id, Op::Mul),
                            "/" => handle_operator(calc_id, Op::Div),
                            "=" => handle_equals(calc_id),
                            "C" => handle_clear(calc_id),
                            _ => {}
                        }
                    });

                    wm.set_window_impl(btn_id, Box::new(button));
                    button_ids.push(btn_id);
                }
            }

            // Register windows
            wm.set_window_impl(frame_id, Box::new(frame));
            wm.set_window_impl(content_id, Box::new(content));
            wm.set_window_impl(display_id, Box::new(display));

            // Add frame to desktop's children
            if let Some(desktop) = wm.window_registry.get_mut(&desktop_id) {
                desktop.add_child(frame_id);
            }

            // Add content to frame's children
            if let Some(frame_win) = wm.window_registry.get_mut(&frame_id) {
                frame_win.add_child(content_id);
            }

            // Add display and buttons to content's children
            if let Some(content_win) = wm.window_registry.get_mut(&content_id) {
                content_win.add_child(display_id);
                for &btn_id in &button_ids {
                    content_win.add_child(btn_id);
                }
            }

            // Focus the calculator window
            wm.focus_window(frame_id);
            if let Some(frame_win) = wm.window_registry.get_mut(&frame_id) {
                frame_win.set_focus(true);
            }

            Some((frame_id, content_id, display_id, button_ids))
        });

        let (frame_id, content_id, display_id, _button_ids) = match result {
            Some(Some(r)) => r,
            _ => {
                crate::println!("Failed to create calculator window");
                return;
            }
        };

        crate::println!("Calculator started. Close window to exit.");

        // Main loop: update display and check if window still exists
        let mut last_display = String::new();

        loop {
            // Get current display value for this calculator
            let (display_text, running) = {
                let states = CALC_STATES.lock();
                if let Some(state) = states.get(&calc_id) {
                    (state.display.clone(), state.running)
                } else {
                    break;
                }
            };

            if !running {
                break;
            }

            // Update display label if changed
            if display_text != last_display {
                last_display = display_text.clone();

                window::with_window_manager(|wm| {
                    // Remove old label and create new one with updated text
                    if let Some(content_win) = wm.window_registry.get_mut(&content_id) {
                        content_win.remove_child(display_id);
                    }
                    wm.window_registry.remove(&display_id);

                    // Get content bounds
                    let content_bounds = if let Some(content) = wm.window_registry.get(&content_id) {
                        content.bounds()
                    } else {
                        return;
                    };

                    // Create new display label with updated text
                    let display_bounds = Rect::new(
                        10,
                        10,
                        (content_bounds.width as i32 - 20) as u32,
                        40,
                    );
                    let mut display = Label::new_with_id(display_id, display_bounds, &display_text);
                    display.set_background(Some(Color::new(40, 40, 40)));
                    display.set_color(Color::WHITE);
                    display.set_align(TextAlign::Right);
                    display.set_parent(Some(content_id));

                    wm.set_window_impl(display_id, Box::new(display));

                    if let Some(content_win) = wm.window_registry.get_mut(&content_id) {
                        content_win.add_child(display_id);
                    }
                });
            }

            // Small delay
            for _ in 0..50000 {
                core::hint::spin_loop();
            }

            // Allow preemption
            crate::process::yield_if_needed();

            // Check if window still exists
            let exists = window::with_window_manager(|wm| {
                wm.window_registry.contains_key(&content_id)
            }).unwrap_or(false);

            if !exists {
                break;
            }
        }

        // Clean up this calculator's state
        {
            let mut states = CALC_STATES.lock();
            states.remove(&calc_id);
        }

        crate::println!("Calculator closed.");
    }

    fn get_name(&self) -> &str {
        "calc"
    }
}

pub fn create_calc_process(args: Vec<String>) -> Box<dyn RunnableProcess> {
    Box::new(CalcProcess::new_with_args(args))
}

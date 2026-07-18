#![no_std]
#![no_main]

//! Standalone ring-3 calculator.
//!
//! A native GUI ELF port of the old kernel `src/commands/calc/` widget app.
//! It owns an ordinary [`CalcState`], draws its 4x4 button grid straight into
//! a [`gui::Canvas`], and blocks in [`gui::next_event`] rather than busy-poll.
//! Every process gets its own independent state, so two calculators never
//! interfere.

extern crate alloc;

use alloc::format;
use alloc::string::String;

use gui::{
    Canvas, Window, FONT_CELL_WIDTH, FONT_LINE_HEIGHT, GUI_EVENT_CLOSE, GUI_EVENT_FOCUS_CHANGE,
    GUI_EVENT_KEY, GUI_EVENT_MOUSE, GUI_EVENT_RESIZE, GUI_MOUSE_DOWN,
};

// ---------------------------------------------------------------------------
// Layout constants
// ---------------------------------------------------------------------------

const PADDING: i32 = 10;
const DISPLAY_HEIGHT: i32 = 40;
const BUTTON_W: i32 = 45;
const BUTTON_H: i32 = 35;
const GAP: i32 = 5;

const GRID_W: i32 = 4 * BUTTON_W + 3 * GAP; // 195
const GRID_H: i32 = 4 * BUTTON_H + 3 * GAP; // 155

const CLIENT_W: u32 = (2 * PADDING + GRID_W) as u32; // 215
const CLIENT_H: u32 = (3 * PADDING + DISPLAY_HEIGHT + GRID_H) as u32; // 225

const MAX_DISPLAY_LEN: usize = 12;

// Colors are little-endian XRGB8888 (0x00RRGGBB).
const COLOR_PANEL: u32 = 0x3C3C3C; // dark window background (60,60,60)
const COLOR_WELL: u32 = 0x282828; // display well (40,40,40)
const COLOR_WHITE: u32 = 0xFFFFFF;
const COLOR_DIGIT: u32 = 0x505050; // dark digit button (80,80,80)
const COLOR_OPERATOR: u32 = 0xFF9F0A; // orange
const COLOR_EQUALS: u32 = 0x32C832; // green
const COLOR_CLEAR: u32 = 0xC83232; // red
const COLOR_BEVEL_LIGHT: u32 = 0xA0A0A0;
const COLOR_BEVEL_DARK: u32 = 0x202020;

// ---------------------------------------------------------------------------
// Calculation model
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Op {
    Add,
    Sub,
    Mul,
    Div,
}

/// Independent per-instance calculator state. Holds no window handles or
/// callbacks so multiple app processes stay fully isolated.
struct CalcState {
    display: String,
    accumulator: f64,
    pending_op: Option<Op>,
    clear_on_next: bool,
}

impl CalcState {
    fn new() -> Self {
        Self {
            display: String::from("0"),
            accumulator: 0.0,
            pending_op: None,
            clear_on_next: false,
        }
    }

    fn input_digit(&mut self, digit: char) {
        if self.clear_on_next {
            self.display.clear();
            self.clear_on_next = false;
        }

        // Replace a lone leading zero, but keep it for `0.`.
        if self.display == "0" && digit != '.' {
            self.display.clear();
        }

        // Only one decimal point.
        if digit == '.' && self.display.contains('.') {
            return;
        }

        if self.display.len() < MAX_DISPLAY_LEN {
            self.display.push(digit);
        }
    }

    fn set_operator(&mut self, op: Op) {
        // Evaluate an unconsumed pending operation first (left-to-right).
        if self.pending_op.is_some() && !self.clear_on_next {
            self.evaluate_pending();
        }
        let current: f64 = self.display.parse().unwrap_or(0.0);
        self.accumulator = current;
        self.pending_op = Some(op);
        self.clear_on_next = true;
    }

    fn equals(&mut self) {
        self.evaluate_pending();
        self.pending_op = None;
    }

    fn clear(&mut self) {
        self.display = String::from("0");
        self.accumulator = 0.0;
        self.pending_op = None;
        self.clear_on_next = false;
    }

    fn evaluate_pending(&mut self) {
        let Some(op) = self.pending_op else {
            return;
        };
        let current: f64 = self.display.parse().unwrap_or(0.0);
        let result = match op {
            Op::Add => self.accumulator + current,
            Op::Sub => self.accumulator - current,
            Op::Mul => self.accumulator * current,
            Op::Div => {
                if current != 0.0 {
                    self.accumulator / current
                } else {
                    f64::INFINITY
                }
            }
        };

        self.display = format_result(result);
        self.accumulator = result;
        self.clear_on_next = true;
    }
}

/// Format a result the way the original kernel calculator did: integers show
/// without a fractional part, other finite values keep at most six fractional
/// digits with trailing zeroes trimmed, and non-finite results show `Error`.
fn format_result(result: f64) -> String {
    if result.is_infinite() || result.is_nan() {
        return String::from("Error");
    }
    let truncated = result as i64;
    let is_integer = (result - truncated as f64).abs() < 1e-10;
    let abs_result = if result < 0.0 { -result } else { result };
    if is_integer && abs_result < 1e10 {
        format!("{}", truncated)
    } else {
        let formatted = format!("{:.6}", result);
        let trimmed = formatted.trim_end_matches('0').trim_end_matches('.');
        String::from(trimmed)
    }
}

// ---------------------------------------------------------------------------
// Button descriptor table
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Action {
    Digit(char),
    Op(Op),
    Equals,
    Clear,
}

/// The classic 4x4 grid, in row-major order. Each button's row/column is its
/// index in this table (`row = index / 4`, `col = index % 4`):
///   7 8 9 /
///   4 5 6 *
///   1 2 3 -
///   C 0 = +
const BUTTONS: &[(&str, Action)] = &[
    ("7", Action::Digit('7')),
    ("8", Action::Digit('8')),
    ("9", Action::Digit('9')),
    ("/", Action::Op(Op::Div)),
    ("4", Action::Digit('4')),
    ("5", Action::Digit('5')),
    ("6", Action::Digit('6')),
    ("*", Action::Op(Op::Mul)),
    ("1", Action::Digit('1')),
    ("2", Action::Digit('2')),
    ("3", Action::Digit('3')),
    ("-", Action::Op(Op::Sub)),
    ("C", Action::Clear),
    ("0", Action::Digit('0')),
    ("=", Action::Equals),
    ("+", Action::Op(Op::Add)),
];

/// Grid `(row, col)` for a button at `index` in [`BUTTONS`].
fn grid_pos(index: usize) -> (i32, i32) {
    ((index / 4) as i32, (index % 4) as i32)
}

fn button_color(action: &Action) -> u32 {
    match action {
        Action::Op(_) => COLOR_OPERATOR,
        Action::Equals => COLOR_EQUALS,
        Action::Clear => COLOR_CLEAR,
        Action::Digit(_) => COLOR_DIGIT,
    }
}

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

/// Horizontal origin of the grid/display, centered within the canvas but never
/// left of the padding. Uses saturating math so tiny windows don't underflow.
fn grid_origin_x(canvas_width: u32) -> i32 {
    let width = canvas_width as i32;
    ((width - GRID_W) / 2).max(PADDING)
}

fn buttons_top() -> i32 {
    2 * PADDING + DISPLAY_HEIGHT
}

/// Pixel rect `(x, y, w, h)` for the button at `row`/`col`.
fn button_rect(origin_x: i32, row: i32, col: i32) -> (i32, i32, i32, i32) {
    let x = origin_x + col * (BUTTON_W + GAP);
    let y = buttons_top() + row * (BUTTON_H + GAP);
    (x, y, BUTTON_W, BUTTON_H)
}

fn point_in_rect(px: i32, py: i32, rect: (i32, i32, i32, i32)) -> bool {
    let (x, y, w, h) = rect;
    px >= x && px < x + w && py >= y && py < y + h
}

// ---------------------------------------------------------------------------
// Application
// ---------------------------------------------------------------------------

struct Calculator {
    window: Window,
    state: CalcState,
    focused: bool,
}

impl Calculator {
    fn new() -> Result<Self, i64> {
        Ok(Self {
            window: Window::new(CLIENT_W, CLIENT_H, "Calculator")?,
            state: CalcState::new(),
            focused: true,
        })
    }

    fn run(&mut self) -> i64 {
        self.render();
        loop {
            let event = match gui::next_event() {
                Ok(event) => event,
                Err(error) => return error,
            };
            if event.kind == gui::GUI_EVENT_THEME_CHANGED {
                // Calculator client styling is intentionally app-owned; the
                // surrounding frame is repainted by the kernel.
                continue;
            }
            if event.window != self.window.handle() {
                continue;
            }
            match event.kind {
                GUI_EVENT_CLOSE => return 0,
                GUI_EVENT_FOCUS_CHANGE => self.focused = event.payload[0] != 0,
                GUI_EVENT_RESIZE => self.window.resize(event.payload[0], event.payload[1]),
                GUI_EVENT_KEY if event.payload[3] != 0 => self.handle_key(event.payload),
                GUI_EVENT_MOUSE if event.payload[3] == GUI_MOUSE_DOWN => {
                    self.handle_mouse(event.payload)
                }
                _ => continue,
            }
            self.render();
        }
    }

    fn handle_key(&mut self, payload: [u32; 6]) {
        let key = payload[0];
        let character = char::from_u32(payload[1]).unwrap_or('\0');
        match key {
            runtime::KEY_ENTER => self.activate(Action::Equals),
            runtime::KEY_ESCAPE => self.activate(Action::Clear),
            _ => match character {
                '0'..='9' | '.' => self.state.input_digit(character),
                '+' => self.state.set_operator(Op::Add),
                '-' => self.state.set_operator(Op::Sub),
                '*' => self.state.set_operator(Op::Mul),
                '/' => self.state.set_operator(Op::Div),
                '=' => self.state.equals(),
                'c' | 'C' => self.state.clear(),
                _ => {}
            },
        }
    }

    fn handle_mouse(&mut self, payload: [u32; 6]) {
        let x = payload[0] as i32;
        let y = payload[1] as i32;
        let origin_x = grid_origin_x(self.window.canvas().width());
        for (index, &(_, action)) in BUTTONS.iter().enumerate() {
            let (row, col) = grid_pos(index);
            let rect = button_rect(origin_x, row, col);
            if !self.button_fits(rect) {
                continue;
            }
            if point_in_rect(x, y, rect) {
                self.activate(action);
                return;
            }
        }
    }

    fn activate(&mut self, action: Action) {
        match action {
            Action::Digit(character) => self.state.input_digit(character),
            Action::Op(op) => self.state.set_operator(op),
            Action::Equals => self.state.equals(),
            Action::Clear => self.state.clear(),
        }
    }

    /// A button is drawable/clickable only if it fits inside the canvas.
    fn button_fits(&self, rect: (i32, i32, i32, i32)) -> bool {
        let (x, y, w, h) = rect;
        let canvas = self.window.canvas();
        x >= 0 && y >= 0 && x + w <= canvas.width() as i32 && y + h <= canvas.height() as i32
    }

    fn render(&mut self) {
        let origin_x = grid_origin_x(self.window.canvas().width());
        let display = self.state.display.clone();
        let canvas = self.window.canvas_mut();
        canvas.clear(COLOR_PANEL);

        // Display well and right-aligned value.
        let well_w = GRID_W as u32;
        canvas.fill_rect(origin_x, PADDING, well_w, DISPLAY_HEIGHT as u32, COLOR_WELL);
        let text_w = display.chars().count() as i32 * FONT_CELL_WIDTH;
        let text_x = (origin_x + GRID_W - 6 - text_w).max(origin_x + 4);
        let text_y = PADDING + (DISPLAY_HEIGHT - FONT_LINE_HEIGHT) / 2;
        canvas.draw_text(text_x, text_y, &display, COLOR_WHITE);

        // Buttons.
        for (index, &(label, action)) in BUTTONS.iter().enumerate() {
            let (row, col) = grid_pos(index);
            let rect = button_rect(origin_x, row, col);
            let (x, y, w, h) = rect;
            if x < 0 || y < 0 || x + w > canvas.width() as i32 || y + h > canvas.height() as i32 {
                continue;
            }
            draw_button(canvas, rect, label, button_color(&action));
        }

        let _ = self.window.present();
    }
}

fn draw_button(canvas: &mut Canvas, rect: (i32, i32, i32, i32), label: &str, color: u32) {
    let (x, y, w, h) = rect;
    canvas.fill_rect(x, y, w as u32, h as u32, color);
    // Simple bevel so button edges read under either desktop theme.
    canvas.horizontal_line(x, y, w as u32, COLOR_BEVEL_LIGHT);
    canvas.vertical_line(x, y, h as u32, COLOR_BEVEL_LIGHT);
    canvas.horizontal_line(x, y + h - 1, w as u32, COLOR_BEVEL_DARK);
    canvas.vertical_line(x + w - 1, y, h as u32, COLOR_BEVEL_DARK);

    let text_w = label.chars().count() as i32 * FONT_CELL_WIDTH;
    let label_x = x + (w - text_w) / 2;
    let label_y = y + (h - FONT_LINE_HEIGHT) / 2;
    canvas.draw_text(label_x, label_y, label, COLOR_WHITE);
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        "mov rdi, rsp",
        "and rsp, -16",
        "call {}",
        "ud2",
        sym calc_main,
    );
}

unsafe extern "C" fn calc_main(_stack: *const u64) -> ! {
    let code = match Calculator::new() {
        Ok(mut app) => app.run(),
        Err(error) => error,
    };
    runtime::exit(if code == 0 { 0 } else { 1 })
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe { runtime::exit(127) }
}

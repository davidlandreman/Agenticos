//! `PAINTING.ELF` — a ring-3 GUI demo: four colored rectangles bouncing
//! around a black canvas.
//!
//! This is the ring-3 port of the former kernel-side `src/commands/painting`
//! app. It keeps the same behavior (a passive, self-driven animation), but
//! runs entirely in ring 3 over the GUI ABI: `gui::Window` for the surface,
//! `Canvas::fill_rect` for the shapes, and a poll-and-sleep frame loop.
//!
//! Unlike notepad/guidemo — which block in `gui::next_event()` and only
//! redraw on input — this app drives its own clock: each iteration drains
//! pending input non-blocking (`gui::try_next_event`), advances the shapes,
//! renders, and sleeps one frame via `runtime::nanosleep`. There is no
//! frame-tick GUI event, so the sleep is what keeps the loop from spinning
//! the CPU under the single-core scheduler.

#![no_std]
#![no_main]

use gui::{Window, GUI_EVENT_CLOSE, GUI_EVENT_RESIZE};

const CANVAS_WIDTH: u32 = 400;
const CANVAS_HEIGHT: u32 = 300;

/// ~60 FPS. Long enough that the loop never busy-spins the single core,
/// short enough to look smooth.
const FRAME_NANOS: i64 = 16_000_000;

const BLACK: u32 = 0x000000;
const RED: u32 = 0xFF0000;
const GREEN: u32 = 0x00FF00;
const BLUE: u32 = 0x0000FF;
const YELLOW: u32 = 0xFFFF00;

/// A rectangle moving at a fixed pixels-per-frame velocity, reflecting off
/// the canvas edges. Movement is integer per-frame because the frame period
/// is now fixed (unlike the kernel version, which normalized against the PIT
/// tick rate); the initial velocities are the kernel app's intended
/// 60-FPS-per-frame deltas.
struct Shape {
    x: i32,
    y: i32,
    dx: i32,
    dy: i32,
    width: u32,
    height: u32,
    color: u32,
}

impl Shape {
    const fn new(x: i32, y: i32, dx: i32, dy: i32, width: u32, height: u32, color: u32) -> Self {
        Self {
            x,
            y,
            dx,
            dy,
            width,
            height,
            color,
        }
    }

    fn advance(&mut self, canvas_width: u32, canvas_height: u32) {
        self.x += self.dx;
        self.y += self.dy;

        let max_x = canvas_width.saturating_sub(self.width) as i32;
        let max_y = canvas_height.saturating_sub(self.height) as i32;

        if self.x <= 0 {
            self.x = 0;
            self.dx = self.dx.abs();
        } else if self.x >= max_x {
            self.x = max_x;
            self.dx = -self.dx.abs();
        }

        if self.y <= 0 {
            self.y = 0;
            self.dy = self.dy.abs();
        } else if self.y >= max_y {
            self.y = max_y;
            self.dy = -self.dy.abs();
        }
    }
}

fn initial_shapes() -> [Shape; 4] {
    [
        Shape::new(50, 50, 3, 2, 60, 40, RED),
        Shape::new(150, 80, -2, 3, 50, 50, GREEN),
        Shape::new(100, 150, 4, -2, 70, 35, BLUE),
        Shape::new(200, 100, -3, -3, 45, 45, YELLOW),
    ]
}

fn render(window: &mut Window, shapes: &[Shape]) {
    let canvas = window.canvas_mut();
    canvas.clear(BLACK);
    for shape in shapes {
        canvas.fill_rect(shape.x, shape.y, shape.width, shape.height, shape.color);
    }
    let _ = window.present();
}

#[no_mangle]
pub unsafe extern "C" fn _start() -> ! {
    let mut window = match Window::new(CANVAS_WIDTH, CANVAS_HEIGHT, "Painting") {
        Ok(window) => window,
        Err(_) => runtime::exit(1),
    };

    let mut shapes = initial_shapes();
    let mut canvas_width = CANVAS_WIDTH;
    let mut canvas_height = CANVAS_HEIGHT;
    let frame = runtime::Timespec {
        tv_sec: 0,
        tv_nsec: FRAME_NANOS,
    };

    loop {
        // Drain all pending input without blocking; we only care about
        // close and resize.
        loop {
            match gui::try_next_event() {
                Ok(Some(event)) => match event.kind {
                    GUI_EVENT_CLOSE => {
                        window.destroy();
                        runtime::exit(0);
                    }
                    GUI_EVENT_RESIZE => {
                        canvas_width = event.payload[0].max(1);
                        canvas_height = event.payload[1].max(1);
                        window.resize(canvas_width, canvas_height);
                    }
                    _ => {}
                },
                Ok(None) => break,
                Err(_) => {
                    window.destroy();
                    runtime::exit(2);
                }
            }
        }

        for shape in &mut shapes {
            shape.advance(canvas_width, canvas_height);
        }
        render(&mut window, &shapes);

        runtime::nanosleep(&frame, None);
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe { runtime::exit(127) }
}

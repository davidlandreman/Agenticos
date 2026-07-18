use alloc::{boxed::Box, string::String, vec, vec::Vec};
use spin::Mutex;

use crate::arch::x86_64::interrupt_guard::InterruptGuard;
use crate::graphics::color::Color;
use crate::lib::arc::Arc;
use crate::process::RunnableProcess;
use crate::window::windows::{base::WindowBase, FrameWindow};
use crate::window::{self, Event, EventResult, GraphicsDevice, Rect, Window, WindowId};

const FIXED_ONE: i64 = 1 << 16;
const TIMER_HZ: i64 = 100;
const INTENDED_FRAMES_PER_SECOND: i64 = 60;
const ANIMATION_SLEEP_TICKS: u64 = 2;
const MAX_ELAPSED_TICKS: u64 = 5;

#[derive(Clone)]
struct BouncingShape {
    x_fixed: i64,
    y_fixed: i64,
    velocity_x_per_tick: i64,
    velocity_y_per_tick: i64,
    width: u32,
    height: u32,
    color: Color,
}

impl BouncingShape {
    fn new(
        x: i32,
        y: i32,
        dx_per_frame: i32,
        dy_per_frame: i32,
        width: u32,
        height: u32,
        color: Color,
    ) -> Self {
        Self {
            x_fixed: x as i64 * FIXED_ONE,
            y_fixed: y as i64 * FIXED_ONE,
            velocity_x_per_tick: dx_per_frame as i64
                * INTENDED_FRAMES_PER_SECOND
                * FIXED_ONE
                / TIMER_HZ,
            velocity_y_per_tick: dy_per_frame as i64
                * INTENDED_FRAMES_PER_SECOND
                * FIXED_ONE
                / TIMER_HZ,
            width,
            height,
            color,
        }
    }

    fn x(&self) -> i32 {
        (self.x_fixed / FIXED_ONE) as i32
    }

    fn y(&self) -> i32 {
        (self.y_fixed / FIXED_ONE) as i32
    }

    fn bounds(&self) -> Rect {
        Rect::new(self.x(), self.y(), self.width, self.height)
    }

    fn advance(&mut self, elapsed_ticks: u64, canvas_width: u32, canvas_height: u32) {
        let elapsed = elapsed_ticks.min(MAX_ELAPSED_TICKS) as i64;
        self.x_fixed += self.velocity_x_per_tick * elapsed;
        self.y_fixed += self.velocity_y_per_tick * elapsed;

        let max_x = canvas_width.saturating_sub(self.width) as i64 * FIXED_ONE;
        let max_y = canvas_height.saturating_sub(self.height) as i64 * FIXED_ONE;

        if self.x_fixed <= 0 {
            self.x_fixed = 0;
            self.velocity_x_per_tick = self.velocity_x_per_tick.abs();
        } else if self.x_fixed >= max_x {
            self.x_fixed = max_x;
            self.velocity_x_per_tick = -self.velocity_x_per_tick.abs();
        }

        if self.y_fixed <= 0 {
            self.y_fixed = 0;
            self.velocity_y_per_tick = self.velocity_y_per_tick.abs();
        } else if self.y_fixed >= max_y {
            self.y_fixed = max_y;
            self.velocity_y_per_tick = -self.velocity_y_per_tick.abs();
        }
    }
}

struct PaintingState {
    shapes: Vec<BouncingShape>,
    canvas_width: u32,
    canvas_height: u32,
    pending_dirty: Option<Rect>,
}

impl PaintingState {
    fn new(canvas_width: u32, canvas_height: u32) -> Self {
        Self {
            shapes: vec![
                BouncingShape::new(50, 50, 3, 2, 60, 40, Color::RED),
                BouncingShape::new(150, 80, -2, 3, 50, 50, Color::GREEN),
                BouncingShape::new(100, 150, 4, -2, 70, 35, Color::BLUE),
                BouncingShape::new(200, 100, -3, -3, 45, 45, Color::YELLOW),
            ],
            canvas_width,
            canvas_height,
            pending_dirty: None,
        }
    }

    fn advance(&mut self, elapsed_ticks: u64) {
        let mut frame_dirty: Option<Rect> = None;

        for shape in &mut self.shapes {
            let old_bounds = shape.bounds();
            shape.advance(elapsed_ticks, self.canvas_width, self.canvas_height);
            let shape_dirty = old_bounds.union(&shape.bounds());
            frame_dirty = Some(match frame_dirty {
                Some(existing) => existing.union(&shape_dirty),
                None => shape_dirty,
            });
        }

        if let Some(frame_dirty) = frame_dirty {
            self.pending_dirty = Some(match self.pending_dirty {
                Some(existing) => existing.union(&frame_dirty),
                None => frame_dirty,
            });
        }
    }
}

struct PaintingCanvasWindow {
    base: WindowBase,
    state: Arc<Mutex<PaintingState>>,
}

impl PaintingCanvasWindow {
    fn new(id: WindowId, bounds: Rect, state: Arc<Mutex<PaintingState>>) -> Self {
        Self {
            base: WindowBase::new_with_id(id, bounds),
            state,
        }
    }
}

impl Window for PaintingCanvasWindow {
    fn base(&self) -> &WindowBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut WindowBase {
        &mut self.base
    }

    fn set_bounds(&mut self, bounds: Rect) {
        self.base.set_bounds(bounds);
        let mut state = self.state.lock();
        state.canvas_width = bounds.width;
        state.canvas_height = bounds.height;
    }

    fn paint(&mut self, device: &mut dyn GraphicsDevice) {
        if !self.visible() {
            return;
        }

        let bounds = self.bounds();
        let shapes = {
            let mut state = self.state.lock();
            state.pending_dirty = None;
            state.shapes.clone()
        };

        // The compositor supplies the clip. Repainting the background and all
        // shapes is therefore correct both for full and incremental frames.
        device.fill_rect(bounds.x, bounds.y, bounds.width, bounds.height, Color::BLACK);
        for shape in &shapes {
            device.fill_rect(
                bounds.x + shape.x(),
                bounds.y + shape.y(),
                shape.width,
                shape.height,
                shape.color,
            );
        }

        self.base.clear_needs_repaint();
    }

    fn handle_event(&mut self, _event: Event) -> EventResult {
        EventResult::Propagate
    }

    fn dirty_rect_hint(&self) -> Option<Rect> {
        self.state.lock().pending_dirty
    }
}

pub struct PaintingProcess;

impl PaintingProcess {
    pub fn new_with_args(_args: Vec<String>) -> Self {
        Self
    }
}

impl RunnableProcess for PaintingProcess {
    fn run(&mut self) {
        let result = window::with_window_manager(|wm| {
            let desktop_id = wm.get_active_screen().and_then(|screen| screen.root_window)?;

            let frame_id = wm.create_window(Some(desktop_id));
            let mut frame = FrameWindow::new(frame_id, "Painting");
            frame.set_bounds(Rect::new(200, 100, 400, 300));
            frame.set_parent(Some(desktop_id));

            let content_id = wm.create_window(Some(frame_id));
            let content_bounds = frame.content_area();
            let state = Arc::new(Mutex::new(PaintingState::new(
                content_bounds.width,
                content_bounds.height,
            )));
            let mut content = PaintingCanvasWindow::new(content_id, content_bounds, state.clone());
            content.set_parent(Some(frame_id));

            frame.set_content_window(content_id);
            wm.set_window_impl(frame_id, Box::new(frame));
            wm.set_window_impl(content_id, Box::new(content));

            if let Some(desktop) = wm.window_registry.get_mut(&desktop_id) {
                desktop.add_child(frame_id);
            }

            wm.focus_window(frame_id);
            if let Some(frame) = wm.window_registry.get_mut(&frame_id) {
                frame.set_focus(true);
            }

            Some((content_id, state))
        });

        let (content_id, state) = match result {
            Some(Some(result)) => result,
            _ => {
                crate::println!("Failed to create painting window");
                return;
            }
        };

        crate::println!("Painting started. Close window to stop.");
        let mut last_tick = crate::arch::x86_64::interrupts::get_timer_ticks();

        loop {
            crate::process::sleep_ticks(ANIMATION_SLEEP_TICKS);

            let current_tick = crate::arch::x86_64::interrupts::get_timer_ticks();
            let elapsed_ticks = current_tick.saturating_sub(last_tick).max(1);
            last_tick = current_tick;

            // A timer interrupt can preempt a process. Keep it disabled while
            // holding shared painting state so the compositor can never spin
            // on a lock owned by a preempted process.
            {
                let _guard = InterruptGuard::disable();
                state.lock().advance(elapsed_ticks);
            }

            let running = window::with_window_manager(|wm| {
                if let Some(content) = wm.window_registry.get_mut(&content_id) {
                    content.invalidate();
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false);

            if !running {
                break;
            }
        }

        crate::println!("Painting stopped.");
    }

    fn get_name(&self) -> &str {
        "painting"
    }
}

pub fn create_painting_process(args: Vec<String>) -> Box<dyn RunnableProcess> {
    Box::new(PaintingProcess::new_with_args(args))
}

#[cfg(feature = "test")]
fn rect_contains_rect(outer: Rect, inner: Rect) -> bool {
    outer.x <= inner.x
        && outer.y <= inner.y
        && outer.right() >= inner.right()
        && outer.bottom() >= inner.bottom()
}

#[cfg(feature = "test")]
fn test_animation_damage_contains_old_and_new_shape_bounds() {
    let mut state = PaintingState::new(396, 272);
    let old_shapes = state.shapes.clone();
    state.advance(2);
    let dirty = state.pending_dirty.expect("animation should create damage");

    for (old, new) in old_shapes.iter().zip(state.shapes.iter()) {
        assert!(rect_contains_rect(dirty, old.bounds()));
        assert!(rect_contains_rect(dirty, new.bounds()));
    }
}

#[cfg(feature = "test")]
fn test_animation_damage_accumulates_until_paint() {
    let mut state = PaintingState::new(396, 272);
    state.advance(2);
    let first = state.pending_dirty.expect("first update should create damage");
    state.advance(2);
    let accumulated = state.pending_dirty.expect("second update should retain damage");

    assert!(rect_contains_rect(accumulated, first));
}

#[cfg(feature = "test")]
fn test_delayed_animation_stays_inside_canvas() {
    let mut state = PaintingState::new(396, 272);
    for _ in 0..500 {
        state.advance(100);
        for shape in &state.shapes {
            assert!(shape.x() >= 0);
            assert!(shape.y() >= 0);
            assert!(shape.x() + shape.width as i32 <= state.canvas_width as i32);
            assert!(shape.y() + shape.height as i32 <= state.canvas_height as i32);
        }
    }
}

#[cfg(feature = "test")]
pub fn get_tests() -> &'static [&'static dyn crate::lib::test_utils::Testable] {
    &[
        &test_animation_damage_contains_old_and_new_shape_bounds,
        &test_animation_damage_accumulates_until_paint,
        &test_delayed_animation_stays_inside_canvas,
    ]
}

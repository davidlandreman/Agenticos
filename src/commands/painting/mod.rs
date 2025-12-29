use crate::process::{BaseProcess, HasBaseProcess, RunnableProcess};
use crate::window::{self, Window, WindowId, Rect};
use crate::window::windows::{ContainerWindow, FrameWindow};
use crate::graphics::color::Color;
use alloc::{vec::Vec, string::String, boxed::Box, vec};

struct BouncingShape {
    x: i32,
    y: i32,
    dx: i32,
    dy: i32,
    width: u32,
    height: u32,
    color: Color,
}

pub struct PaintingProcess {
    base: BaseProcess,
    args: Vec<String>,
}

impl PaintingProcess {
    pub fn new_with_args(args: Vec<String>) -> Self {
        Self {
            base: BaseProcess::new("painting"),
            args,
        }
    }
}

impl HasBaseProcess for PaintingProcess {
    fn base(&self) -> &BaseProcess {
        &self.base
    }

    fn base_mut(&mut self) -> &mut BaseProcess {
        &mut self.base
    }
}

impl RunnableProcess for PaintingProcess {
    fn run(&mut self) {
        // Create bouncing shapes with different colors and velocities
        let mut shapes = vec![
            BouncingShape {
                x: 50,
                y: 50,
                dx: 3,
                dy: 2,
                width: 60,
                height: 40,
                color: Color::RED,
            },
            BouncingShape {
                x: 150,
                y: 80,
                dx: -2,
                dy: 3,
                width: 50,
                height: 50,
                color: Color::GREEN,
            },
            BouncingShape {
                x: 100,
                y: 150,
                dx: 4,
                dy: -2,
                width: 70,
                height: 35,
                color: Color::BLUE,
            },
            BouncingShape {
                x: 200,
                y: 100,
                dx: -3,
                dy: -3,
                width: 45,
                height: 45,
                color: Color::YELLOW,
            },
        ];

        // Create frame + container window
        let result = window::with_window_manager(|wm| {
            // Get the desktop window (root of the screen)
            let desktop_id = wm.get_active_screen()
                .and_then(|s| s.root_window)?;

            // Create frame window
            let frame_id = wm.create_window(Some(desktop_id));
            let mut frame = FrameWindow::new(frame_id, "Painting");
            frame.set_bounds(Rect::new(200, 100, 400, 300));
            frame.set_parent(Some(desktop_id));

            // Create container as content
            let content_id = wm.create_window(Some(frame_id));
            let content_bounds = frame.content_area();
            let mut content = ContainerWindow::new_with_id(content_id, content_bounds);
            content.set_background_color(Color::BLACK);
            content.set_parent(Some(frame_id));

            frame.set_content_window(content_id);

            // Register windows
            wm.set_window_impl(frame_id, Box::new(frame));
            wm.set_window_impl(content_id, Box::new(content));

            // Add frame to desktop's children
            if let Some(desktop) = wm.window_registry.get_mut(&desktop_id) {
                desktop.add_child(frame_id);
            }

            // Focus the painting window
            wm.focus_window(frame_id);
            if let Some(frame) = wm.window_registry.get_mut(&frame_id) {
                frame.set_focus(true);
            }

            Some((frame_id, content_id, content_bounds))
        });

        let (frame_id, content_id, content_bounds) = match result {
            Some(Some(r)) => r,
            _ => {
                crate::println!("Failed to create painting window");
                return;
            }
        };

        crate::println!("Painting started. Close window to stop.");

        // Animation loop
        let mut running = true;
        let mut frame_count = 0u64;

        while running {
            // Update shape positions
            for shape in &mut shapes {
                shape.x += shape.dx;
                shape.y += shape.dy;

                // Bounce off edges
                if shape.x <= 0 || shape.x + shape.width as i32 >= content_bounds.width as i32 {
                    shape.dx = -shape.dx;
                    shape.x = shape.x.clamp(0, content_bounds.width as i32 - shape.width as i32);
                }
                if shape.y <= 0 || shape.y + shape.height as i32 >= content_bounds.height as i32 {
                    shape.dy = -shape.dy;
                    shape.y = shape.y.clamp(0, content_bounds.height as i32 - shape.height as i32);
                }
            }

            // Draw directly to screen
            window::with_window_manager(|wm| {
                // Get content window's absolute position
                if let Some(content) = wm.window_registry.get(&content_id) {
                    let abs_bounds = content.bounds();

                    // Calculate absolute position by traversing parent chain
                    let (abs_x, abs_y) = get_absolute_position(wm, content_id);

                    // Clear background (black)
                    wm.graphics_device.fill_rect(
                        abs_x,
                        abs_y,
                        content_bounds.width as usize,
                        content_bounds.height as usize,
                        Color::BLACK,
                    );

                    // Draw shapes at their current positions
                    for shape in &shapes {
                        wm.graphics_device.fill_rect(
                            abs_x + shape.x as usize,
                            abs_y + shape.y as usize,
                            shape.width as usize,
                            shape.height as usize,
                            shape.color,
                        );
                    }

                    wm.graphics_device.flush();
                }
            });

            // Small delay to control animation speed (~60fps target)
            // Busy wait since we don't have proper sleep
            for _ in 0..100000 {
                core::hint::spin_loop();
            }

            // Allow preemption - check if scheduler wants to switch to another process
            crate::process::yield_if_needed();

            // Check if window still exists (user might have closed it)
            running = window::with_window_manager(|wm| {
                wm.window_registry.contains_key(&content_id)
            }).unwrap_or(false);

            frame_count += 1;
        }

        crate::println!("Painting stopped.");
    }

    fn get_name(&self) -> &str {
        "painting"
    }
}

/// Get absolute screen position of a window by traversing parent chain
fn get_absolute_position(wm: &window::WindowManager, window_id: WindowId) -> (usize, usize) {
    let mut x = 0i32;
    let mut y = 0i32;
    let mut current_id = Some(window_id);

    while let Some(id) = current_id {
        if let Some(window) = wm.window_registry.get(&id) {
            let bounds = window.bounds();
            x += bounds.x;
            y += bounds.y;
            current_id = window.parent();
        } else {
            break;
        }
    }

    (x.max(0) as usize, y.max(0) as usize)
}

pub fn create_painting_process(args: Vec<String>) -> Box<dyn RunnableProcess> {
    Box::new(PaintingProcess::new_with_args(args))
}

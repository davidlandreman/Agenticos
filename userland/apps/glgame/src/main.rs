//! `GLGAME.ELF` — a small colored-geometry 3D game rendered through VirGL.

#![no_std]
#![no_main]

use gl::{Context, MatrixMode, Primitive};
use gui::{Window, GUI_EVENT_CLOSE, GUI_EVENT_FOCUS_CHANGE, GUI_EVENT_KEY, GUI_EVENT_RESIZE};

const INITIAL_WIDTH: u32 = 800;
const INITIAL_HEIGHT: u32 = 600;
const FRAME_NANOS: i64 = 16_000_000;
const PICKUPS: [(f32, f32); 6] = [
    (-4.5, -3.5),
    (0.0, -4.5),
    (4.5, -2.5),
    (-3.0, 2.5),
    (1.5, 3.5),
    (4.5, 4.0),
];

#[derive(Default)]
struct Keys {
    left: bool,
    right: bool,
    up: bool,
    down: bool,
}

struct Game {
    player_x: f32,
    player_z: f32,
    angle: f32,
    collected: [bool; PICKUPS.len()],
    score: u32,
}

impl Game {
    const fn new() -> Self {
        Self {
            player_x: 0.0,
            player_z: 0.0,
            angle: 0.0,
            collected: [false; PICKUPS.len()],
            score: 0,
        }
    }

    fn reset(&mut self) {
        *self = Self::new();
    }

    fn update(&mut self, keys: &Keys, delta: f32) {
        let speed = 4.0 * delta;
        if keys.left {
            self.player_x -= speed;
        }
        if keys.right {
            self.player_x += speed;
        }
        if keys.up {
            self.player_z -= speed;
        }
        if keys.down {
            self.player_z += speed;
        }
        self.player_x = self.player_x.clamp(-5.5, 5.5);
        self.player_z = self.player_z.clamp(-5.5, 5.5);
        self.angle = (self.angle + 70.0 * delta) % 360.0;
        for (index, &(x, z)) in PICKUPS.iter().enumerate() {
            if self.collected[index] {
                continue;
            }
            let dx = self.player_x - x;
            let dz = self.player_z - z;
            if dx * dx + dz * dz < 0.8 {
                self.collected[index] = true;
                self.score += 1;
            }
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn _start() -> ! {
    let mut window = match Window::new(INITIAL_WIDTH, INITIAL_HEIGHT, "GL Arena") {
        Ok(window) => window,
        Err(_) => runtime::exit(1),
    };
    let mut gl = match Context::new(window.handle()) {
        Ok(gl) => gl,
        Err(_) => unsupported_loop(&mut window),
    };
    let mut game = Game::new();
    let mut keys = Keys::default();
    let mut focused = true;
    let mut previous = monotonic_seconds();
    let frame = runtime::Timespec {
        tv_sec: 0,
        tv_nsec: FRAME_NANOS,
    };

    'game: loop {
        loop {
            match gui::try_next_event() {
                Ok(Some(event)) if event.window == window.handle() => match event.kind {
                    GUI_EVENT_CLOSE => break 'game,
                    GUI_EVENT_FOCUS_CHANGE => focused = event.payload[0] != 0,
                    GUI_EVENT_RESIZE => {
                        window.resize(event.payload[0].max(1), event.payload[1].max(1));
                        if gl.resize().is_err() {
                            break 'game;
                        }
                    }
                    GUI_EVENT_KEY => {
                        let pressed = event.payload[3] != 0;
                        match event.payload[0] {
                            1 | runtime::KEY_LEFT => keys.left = pressed,   // A
                            4 | runtime::KEY_RIGHT => keys.right = pressed, // D
                            23 | runtime::KEY_UP => keys.up = pressed,      // W
                            19 | runtime::KEY_DOWN => keys.down = pressed,  // S
                            18 if pressed => game.reset(),                  // R
                            _ => {}
                        }
                    }
                    _ => {}
                },
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(_) => break 'game,
            }
        }

        let now = monotonic_seconds();
        let delta = (now - previous).clamp(0.0, 0.05) as f32;
        previous = now;
        if focused {
            game.update(&keys, delta);
            render(&mut gl, &game);
            let _ = gl.swap_buffers();
        }
        runtime::nanosleep(&frame, None);
    }

    gl.destroy();
    window.destroy();
    runtime::exit(0)
}

fn unsupported_loop(window: &mut Window) -> ! {
    draw_requirement(window);
    loop {
        let event = match gui::next_event() {
            Ok(event) => event,
            Err(_) => unsafe { runtime::exit(2) },
        };
        if event.window != window.handle() {
            continue;
        }
        match event.kind {
            GUI_EVENT_CLOSE => {
                window.destroy();
                unsafe { runtime::exit(0) }
            }
            GUI_EVENT_RESIZE => {
                window.resize(event.payload[0].max(1), event.payload[1].max(1));
                draw_requirement(window);
            }
            _ => {}
        }
    }
}

fn draw_requirement(window: &mut Window) {
    let canvas = window.canvas_mut();
    canvas.clear(0x101820);
    canvas.draw_text(
        24,
        28,
        "GL Arena requires qualified strict VirGL.",
        0xFFFFFF,
    );
    canvas.draw_text(24, 48, "Launch with AGENTICOS_COMPOSITOR=gpu", 0x80C0FF);
    canvas.draw_text(24, 64, "and AGENTICOS_GPU_STRICT=1.", 0x80C0FF);
    let _ = window.present();
}

fn monotonic_seconds() -> f64 {
    let mut time = runtime::Timespec::default();
    if runtime::clock_gettime(runtime::CLOCK_MONOTONIC, &mut time) < 0 {
        return 0.0;
    }
    time.tv_sec as f64 + time.tv_nsec as f64 / 1_000_000_000.0
}

fn render(gl: &mut Context, game: &Game) {
    let (width, height) = gl.dimensions();
    gl.begin_frame();
    gl.viewport(0, 0, width, height);
    gl.clear_color(0.025, 0.04, 0.09, 1.0);
    gl.depth_test(true);
    gl.cull_back_faces(false);

    gl.matrix_mode(MatrixMode::Projection);
    gl.load_identity();
    gl.perspective(55.0, width as f32 / height.max(1) as f32, 0.1, 80.0);
    gl.matrix_mode(MatrixMode::ModelView);
    gl.load_identity();
    gl.translate(0.0, -4.5, -16.0);
    gl.rotate_x(24.0);

    // The floor is an ordered background pass. Keeping it out of the depth
    // fallback prevents its arena-sized triangles from sorting over a moving
    // object on hosts whose VirGL capset has no depth attachment format.
    gl.depth_test(false);
    draw_floor(gl);
    gl.depth_test(true);
    gl.push_matrix();
    gl.translate(game.player_x, 0.0, game.player_z);
    draw_cube(gl, 0.65, [0.15, 0.72, 1.0, 1.0]);
    gl.pop_matrix();

    for (index, &(x, z)) in PICKUPS.iter().enumerate() {
        if game.collected[index] {
            continue;
        }
        gl.push_matrix();
        gl.translate(x, 0.1, z);
        gl.rotate_y(game.angle + index as f32 * 31.0);
        draw_crystal(gl, 0.5, [1.0, 0.72, 0.12, 1.0]);
        gl.pop_matrix();
    }

    draw_score(gl, width, height, game.score);
}

fn draw_floor(gl: &mut Context) {
    gl.color(0.08, 0.12, 0.18, 1.0);
    gl.begin(Primitive::Quads);
    gl.vertex(-6.0, -0.7, -6.0);
    gl.vertex(6.0, -0.7, -6.0);
    gl.vertex(6.0, -0.7, 6.0);
    gl.vertex(-6.0, -0.7, 6.0);
    gl.end();
    gl.color(0.12, 0.28, 0.38, 1.0);
    for step in -6..=6 {
        let value = step as f32;
        thin_quad(gl, -6.0, value, 6.0, value + 0.025);
        thin_quad(gl, value, -6.0, value + 0.025, 6.0);
    }
}

fn thin_quad(gl: &mut Context, x0: f32, z0: f32, x1: f32, z1: f32) {
    gl.begin(Primitive::Quads);
    gl.vertex(x0, -0.69, z0);
    gl.vertex(x1, -0.69, z0);
    gl.vertex(x1, -0.69, z1);
    gl.vertex(x0, -0.69, z1);
    gl.end();
}

fn draw_cube(gl: &mut Context, half: f32, color: [f32; 4]) {
    gl.color(color[0], color[1], color[2], color[3]);
    let faces = [
        [
            [-1.0, -1.0, 1.0],
            [1.0, -1.0, 1.0],
            [1.0, 1.0, 1.0],
            [-1.0, 1.0, 1.0],
        ],
        [
            [1.0, -1.0, -1.0],
            [-1.0, -1.0, -1.0],
            [-1.0, 1.0, -1.0],
            [1.0, 1.0, -1.0],
        ],
        [
            [-1.0, 1.0, 1.0],
            [1.0, 1.0, 1.0],
            [1.0, 1.0, -1.0],
            [-1.0, 1.0, -1.0],
        ],
        [
            [-1.0, -1.0, -1.0],
            [1.0, -1.0, -1.0],
            [1.0, -1.0, 1.0],
            [-1.0, -1.0, 1.0],
        ],
        [
            [1.0, -1.0, 1.0],
            [1.0, -1.0, -1.0],
            [1.0, 1.0, -1.0],
            [1.0, 1.0, 1.0],
        ],
        [
            [-1.0, -1.0, -1.0],
            [-1.0, -1.0, 1.0],
            [-1.0, 1.0, 1.0],
            [-1.0, 1.0, -1.0],
        ],
    ];
    gl.begin(Primitive::Quads);
    for face in faces {
        for vertex in face {
            gl.vertex(vertex[0] * half, vertex[1] * half, vertex[2] * half);
        }
    }
    gl.end();
}

fn draw_crystal(gl: &mut Context, size: f32, color: [f32; 4]) {
    gl.color(color[0], color[1], color[2], color[3]);
    let top = [0.0, size * 1.5, 0.0];
    let bottom = [0.0, -size * 1.5, 0.0];
    let ring = [
        [size, 0.0, 0.0],
        [0.0, 0.0, size],
        [-size, 0.0, 0.0],
        [0.0, 0.0, -size],
    ];
    gl.begin(Primitive::Triangles);
    for index in 0..4 {
        let next = (index + 1) % 4;
        for vertex in [
            top,
            ring[index],
            ring[next],
            bottom,
            ring[next],
            ring[index],
        ] {
            gl.vertex(vertex[0], vertex[1], vertex[2]);
        }
    }
    gl.end();
}

fn draw_score(gl: &mut Context, width: u32, height: u32, score: u32) {
    const SEGMENTS: [[bool; 7]; 10] = [
        [true, true, true, true, true, true, false],
        [false, true, true, false, false, false, false],
        [true, true, false, true, true, false, true],
        [true, true, true, true, false, false, true],
        [false, true, true, false, false, true, true],
        [true, false, true, true, false, true, true],
        [true, false, true, true, true, true, true],
        [true, true, true, false, false, false, false],
        [true, true, true, true, true, true, true],
        [true, true, true, true, false, true, true],
    ];
    gl.matrix_mode(MatrixMode::Projection);
    gl.push_matrix();
    gl.load_identity();
    gl.orthographic(0.0, width as f32, height as f32, 0.0, -1.0, 1.0);
    gl.matrix_mode(MatrixMode::ModelView);
    gl.push_matrix();
    gl.load_identity();
    gl.depth_test(false);
    gl.color(0.95, 0.8, 0.2, 1.0);
    let digit = (score % 10) as usize;
    let x = 24.0;
    let y = 24.0;
    let segments = [
        (x + 4.0, y, 20.0, 4.0),
        (x + 24.0, y + 4.0, 4.0, 20.0),
        (x + 24.0, y + 28.0, 4.0, 20.0),
        (x + 4.0, y + 48.0, 20.0, 4.0),
        (x, y + 28.0, 4.0, 20.0),
        (x, y + 4.0, 4.0, 20.0),
        (x + 4.0, y + 24.0, 20.0, 4.0),
    ];
    for (enabled, &(sx, sy, sw, sh)) in SEGMENTS[digit].iter().zip(segments.iter()) {
        if *enabled {
            screen_quad(gl, sx, sy, sw, sh);
        }
    }
    gl.depth_test(true);
    gl.pop_matrix();
    gl.matrix_mode(MatrixMode::Projection);
    gl.pop_matrix();
    gl.matrix_mode(MatrixMode::ModelView);
}

fn screen_quad(gl: &mut Context, x: f32, y: f32, width: f32, height: f32) {
    gl.begin(Primitive::Quads);
    gl.vertex(x, y, 0.0);
    gl.vertex(x + width, y, 0.0);
    gl.vertex(x + width, y + height, 0.0);
    gl.vertex(x, y + height, 0.0);
    gl.end();
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe { runtime::exit(127) }
}

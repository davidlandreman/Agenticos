#![no_std]
#![no_main]

use gui::{Window, GUI_EVENT_CLOSE, GUI_EVENT_KEY, GUI_EVENT_MOUSE, GUI_EVENT_RESIZE};

#[no_mangle]
pub unsafe extern "C" fn _start() -> ! {
    let mut window = match Window::new(480, 320, "Ring-3 GUI Demo") {
        Ok(window) => window,
        Err(_) => runtime::exit(1),
    };
    let mut background = 0x203050;
    let (mut mouse_x, mut mouse_y) = (240i32, 160i32);
    render(&mut window, background, mouse_x, mouse_y);
    loop {
        let event = match gui::next_event() {
            Ok(event) => event,
            Err(_) => runtime::exit(2),
        };
        match event.kind {
            GUI_EVENT_CLOSE => break,
            GUI_EVENT_KEY if event.payload[3] != 0 => {
                background = background.rotate_left(5) & 0xFFFFFF;
            }
            GUI_EVENT_MOUSE => {
                mouse_x = event.payload[0] as i32;
                mouse_y = event.payload[1] as i32;
            }
            GUI_EVENT_RESIZE => window.resize(event.payload[0], event.payload[1]),
            _ => continue,
        }
        render(&mut window, background, mouse_x, mouse_y);
    }
    window.destroy();
    runtime::exit(0)
}

fn render(window: &mut Window, background: u32, x: i32, y: i32) {
    let canvas = window.canvas_mut();
    canvas.clear(background);
    canvas.fill_rect(x - 20, y - 20, 40, 40, 0xF0B030);
    canvas.rect(x - 20, y - 20, 40, 40, 0xFFFFFF);
    canvas.draw_text(
        16,
        16,
        "GUIDEMO.ELF - move the mouse or press a key",
        0xFFFFFF,
    );
    let _ = window.present();
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe { runtime::exit(127) }
}

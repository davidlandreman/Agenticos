#![no_std]
#![no_main]

extern crate alloc;

use dialogs::{Buttons, ColorPicker, DialogStatus, MessageBox, MessageChoice, Modal, ModalOutcome};
use gui::{Window, GUI_EVENT_CLOSE, GUI_EVENT_KEY, GUI_EVENT_MOUSE, GUI_EVENT_RESIZE};

/// Why the reference client has a modal open (so its outcome routes correctly).
#[derive(Clone, Copy)]
enum ModalPurpose {
    Color,
    Confirm,
}

#[no_mangle]
pub unsafe extern "C" fn _start() -> ! {
    let mut window = match Window::new(480, 320, "Ring-3 GUI Demo") {
        Ok(window) => window,
        Err(_) => runtime::exit(1),
    };
    let mut background = 0x203050u32;
    let (mut mouse_x, mut mouse_y) = (240i32, 160i32);
    let mut modal: Option<Modal> = None;
    let mut purpose = ModalPurpose::Color;
    render(&mut window, background, mouse_x, mouse_y);
    loop {
        let event = match gui::next_event() {
            Ok(event) => event,
            Err(_) => runtime::exit(2),
        };
        if event.window == window.handle() {
            match event.kind {
                GUI_EVENT_CLOSE => break,
                // While a modal is open, ignore key/mouse to the main window.
                GUI_EVENT_KEY if event.payload[3] != 0 && modal.is_none() => {
                    let character = char::from_u32(event.payload[1]).unwrap_or('\0');
                    match character.to_ascii_lowercase() {
                        'c' => {
                            if let Ok(dialog) = ColorPicker::new(background) {
                                purpose = ModalPurpose::Color;
                                modal = Some(Modal::Color(dialog));
                            }
                        }
                        'm' => {
                            if let Ok(dialog) = MessageBox::new(
                                "Confirm",
                                "Exit the GUI demo?\nYes exits, No dismisses.",
                                Buttons::YesNo,
                            ) {
                                purpose = ModalPurpose::Confirm;
                                modal = Some(Modal::Message(dialog));
                            }
                        }
                        _ => background = background.rotate_left(5) & 0xFFFFFF,
                    }
                    render(&mut window, background, mouse_x, mouse_y);
                }
                GUI_EVENT_MOUSE if modal.is_none() => {
                    mouse_x = event.payload[0] as i32;
                    mouse_y = event.payload[1] as i32;
                    render(&mut window, background, mouse_x, mouse_y);
                }
                GUI_EVENT_RESIZE => {
                    window.resize(event.payload[0], event.payload[1]);
                    render(&mut window, background, mouse_x, mouse_y);
                }
                _ => {}
            }
        } else if let Some(dialog) = modal.as_mut() {
            if event.window == dialog.window_handle() {
                if let DialogStatus::Done(outcome) = dialog.handle_event(&event) {
                    modal = None;
                    match (purpose, outcome) {
                        (ModalPurpose::Color, Some(ModalOutcome::Color(color))) => {
                            background = color
                        }
                        (ModalPurpose::Confirm, Some(ModalOutcome::Choice(MessageChoice::Yes))) => {
                            break
                        }
                        _ => {}
                    }
                    render(&mut window, background, mouse_x, mouse_y);
                }
            }
        }
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
    canvas.draw_text(16, 32, "c: color picker    m: message box", 0xFFFFFF);
    let _ = window.present();
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe { runtime::exit(127) }
}

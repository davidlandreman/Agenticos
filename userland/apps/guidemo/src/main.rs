#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;

use dialogs::{
    Buttons, ColorPicker, DialogStatus, FileDialog, MessageBox, MessageChoice, Modal, ModalOutcome,
};
use gui::{
    theme, Button, ButtonState, ListView, TextField, Window, GUI_EVENT_CLOSE, GUI_EVENT_KEY,
    GUI_EVENT_MOUSE, GUI_EVENT_RESIZE, GUI_MOUSE_DOWN, GUI_MOUSE_SCROLL,
};

/// Why the reference client has a modal open (so its outcome routes correctly).
#[derive(Clone, Copy)]
enum ModalPurpose {
    Color,
    Confirm,
    Open,
}

/// Reference client: renders every toolkit widget in every state under the
/// active theme (Classic/Aero from `/etc/theme`), plus the common dialogs.
struct Demo {
    window: Window,
    swatch: u32,
    field: TextField,
    field_focused: bool,
    list: ListView,
    buttons: [Button; 4],
    modal: Option<Modal>,
    purpose: ModalPurpose,
}

const BUTTON_STATES: [ButtonState; 4] = [
    ButtonState::Normal,
    ButtonState::Hot,
    ButtonState::Pressed,
    ButtonState::Disabled,
];

impl Demo {
    fn new() -> Result<Self, i64> {
        let window = Window::new(520, 400, "Ring-3 GUI Demo")?;
        let mut list = ListView::new(16, 240, 220, 120);
        list.set_rows(
            (1..=12)
                .map(|index| alloc::format!("List row {index}"))
                .collect::<Vec<_>>(),
        );
        list.selected = Some(1);
        let mut demo = Self {
            window,
            swatch: 0x203050,
            field: TextField::new(16, 196, 220, 24, "Editable text"),
            field_focused: true,
            list,
            buttons: [
                Button::new("Normal", 16, 60, 100, 28),
                Button::new("Default", 128, 60, 100, 28),
                Button::new("Pressed", 240, 60, 100, 28),
                Button::new("Disabled", 352, 60, 100, 28),
            ],
            modal: None,
            purpose: ModalPurpose::Color,
        };
        demo.render();
        Ok(demo)
    }

    fn render(&mut self) {
        let palette = theme::palette();
        let theme_name = match theme::current() {
            theme::Theme::Classic => "classic",
            theme::Theme::Aero => "aero",
            theme::Theme::Futurism => "futurism",
        };
        let header = alloc::format!("GUIDEMO.ELF - theme: {theme_name}");
        let swatch = self.swatch;
        {
            let canvas = self.window.canvas_mut();
            canvas.clear(palette.content_bg);
            canvas.draw_text(16, 16, &header, palette.text);
            canvas.draw_text(
                16,
                32,
                "c: color    m: message    o: open file",
                palette.disabled_text,
            );

            canvas.draw_text(16, 48, "Buttons:", palette.text);
        }
        for (button, state) in self.buttons.iter().zip(BUTTON_STATES) {
            button.draw_state(self.window.canvas_mut(), state);
        }
        {
            let canvas = self.window.canvas_mut();
            canvas.draw_text(16, 104, "Selection:", palette.text);
            theme::draw_selection(canvas, 96, 100, 120, 16);
            canvas.draw_text(100, 104, "selected text", palette.selection_text);

            canvas.draw_text(16, 128, "Menu surface:", palette.text);
            theme::draw_menu_surface(canvas, 128, 124, 140, 56);
            canvas.draw_text(136, 132, "Menu item", palette.text);
            theme::draw_selection(canvas, 132, 146, 132, 16);
            canvas.draw_text(136, 150, "Hovered item", palette.selection_text);

            canvas.draw_text(16, 184, "Text field:", palette.text);
        }
        let focused = self.field_focused;
        self.field.draw(self.window.canvas_mut(), focused);
        {
            let canvas = self.window.canvas_mut();
            canvas.draw_text(16, 228, "List view:", palette.text);
        }
        self.list.draw(self.window.canvas_mut());
        {
            let canvas = self.window.canvas_mut();
            // Color swatch chosen via the picker (app-owned accent, not themed).
            canvas.draw_text(260, 228, "Picked color:", palette.text);
            canvas.fill_rect(260, 240, 64, 40, swatch);
            canvas.rect(260, 240, 64, 40, palette.border);
        }
        let _ = self.window.present();
    }

    fn handle_main(&mut self, event: &runtime::GuiEvent) -> bool {
        match event.kind {
            GUI_EVENT_CLOSE => return true,
            GUI_EVENT_KEY if event.payload[3] != 0 && self.modal.is_none() => {
                let key = event.payload[0];
                let character = char::from_u32(event.payload[1]).unwrap_or('\0');
                match character.to_ascii_lowercase() {
                    'c' if !self.field_focused => {
                        if let Ok(dialog) = ColorPicker::new(self.swatch) {
                            self.purpose = ModalPurpose::Color;
                            self.modal = Some(Modal::Color(dialog));
                        }
                    }
                    'm' if !self.field_focused => {
                        if let Ok(dialog) = MessageBox::new(
                            "Confirm",
                            "Exit the GUI demo?\nYes exits, No dismisses.",
                            Buttons::YesNo,
                        ) {
                            self.purpose = ModalPurpose::Confirm;
                            self.modal = Some(Modal::Message(dialog));
                        }
                    }
                    'o' if !self.field_focused => {
                        if let Ok(dialog) = FileDialog::open("/host/") {
                            self.purpose = ModalPurpose::Open;
                            self.modal = Some(Modal::File(dialog));
                        }
                    }
                    _ if self.field_focused => {
                        self.field.key(key, character);
                        self.render();
                    }
                    _ => {}
                }
                self.render();
            }
            GUI_EVENT_MOUSE if self.modal.is_none() => {
                let x = event.payload[0] as i32;
                let y = event.payload[1] as i32;
                match event.payload[3] {
                    GUI_MOUSE_DOWN => {
                        self.field_focused = self.field.hit(x, y);
                        if self.field_focused {
                            self.field.click(x);
                        }
                        self.list.click(x, y);
                        self.render();
                    }
                    GUI_MOUSE_SCROLL if self.list.hit(x, y) => {
                        self.list.scroll(event.payload[5] as i32);
                        self.render();
                    }
                    _ => {}
                }
            }
            GUI_EVENT_RESIZE => {
                self.window.resize(event.payload[0], event.payload[1]);
                self.render();
            }
            _ => {}
        }
        false
    }

    fn handle_modal(&mut self, event: &runtime::GuiEvent) -> bool {
        let Some(dialog) = self.modal.as_mut() else {
            return false;
        };
        if event.window != dialog.window_handle() {
            return false;
        }
        if let DialogStatus::Done(outcome) = dialog.handle_event(event) {
            self.modal = None;
            match (self.purpose, outcome) {
                (ModalPurpose::Color, Some(ModalOutcome::Color(color))) => self.swatch = color,
                (ModalPurpose::Confirm, Some(ModalOutcome::Choice(MessageChoice::Yes))) => {
                    return true
                }
                (ModalPurpose::Open, Some(ModalOutcome::Path(path))) => {
                    self.field.set_text(&path);
                }
                _ => {}
            }
            self.render();
        }
        false
    }
}

#[no_mangle]
pub unsafe extern "C" fn _start() -> ! {
    let mut demo = match Demo::new() {
        Ok(demo) => demo,
        Err(_) => runtime::exit(1),
    };
    loop {
        let event = match gui::next_event() {
            Ok(event) => event,
            Err(_) => runtime::exit(2),
        };
        if event.kind == gui::GUI_EVENT_THEME_CHANGED {
            demo.render();
            if let Some(modal) = demo.modal.as_mut() {
                modal.refresh_theme();
            }
            continue;
        }
        let exit = if event.window == demo.window.handle() {
            demo.handle_main(&event)
        } else {
            demo.handle_modal(&event)
        };
        if exit {
            break;
        }
    }
    demo.window.destroy();
    runtime::exit(0)
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe { runtime::exit(127) }
}

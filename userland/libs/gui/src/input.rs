use gui_core::{ControlInput, KeyInput, Modifiers, MouseButtons, PointerInput, PointerKind};

fn modifiers(bits: u32) -> Modifiers {
    Modifiers {
        shift: bits & 1 != 0,
        ctrl: bits & 2 != 0,
        alt: bits & 4 != 0,
    }
}

/// Decode the fixed GUI ABI payload into a typed control input.
pub fn decode_control_input(event: &runtime::GuiEvent) -> Option<ControlInput> {
    match event.kind {
        runtime::GUI_EVENT_KEY => Some(ControlInput::Key(KeyInput {
            key: event.payload[0],
            character: char::from_u32(event.payload[1]).unwrap_or('\0'),
            modifiers: modifiers(event.payload[2]),
            pressed: event.payload[3] != 0,
        })),
        runtime::GUI_EVENT_MOUSE => {
            let packed = event.payload[2];
            let kind = match event.payload[3] {
                runtime::GUI_MOUSE_MOVE => PointerKind::Move,
                runtime::GUI_MOUSE_DOWN => PointerKind::Down,
                runtime::GUI_MOUSE_UP => PointerKind::Up,
                runtime::GUI_MOUSE_SCROLL => PointerKind::Scroll {
                    delta_x: event.payload[4] as i32,
                    delta_y: event.payload[5] as i32,
                },
                _ => return None,
            };
            Some(ControlInput::Pointer(PointerInput {
                x: event.payload[0] as i32,
                y: event.payload[1] as i32,
                buttons: MouseButtons {
                    left: packed & 1 != 0,
                    right: packed & 2 != 0,
                    middle: packed & 4 != 0,
                },
                modifiers: modifiers(packed >> 8),
                kind,
                timestamp: if matches!(kind, PointerKind::Down | PointerKind::Up) {
                    event.payload[4] as u64 | ((event.payload[5] as u64) << 32)
                } else {
                    0
                },
            }))
        }
        _ => None,
    }
}

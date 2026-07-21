//! Non-blocking kernel Run dialog used by the Start menu.

use alloc::boxed::Box;
use alloc::string::String;
use spin::Mutex;

use crate::graphics::color::Color;
use crate::window::windows::dialog::{
    clear_dialog_state, close_dialog_with_result, get_dialog_id, get_dialog_result, is_dialog_open,
    set_dialog_state, DialogResult,
};
use crate::window::windows::{Button, ContainerWindow, FrameWindow, Label, TextInput};
use crate::window::{with_window_manager, Rect, Window, WindowId};

const DIALOG_WIDTH: u32 = 440;
const DIALOG_HEIGHT: u32 = 180;
const INPUT_LIMIT: usize = 256;

static RUN_DIALOG_ID: Mutex<Option<WindowId>> = Mutex::new(None);
static RUN_INPUT: Mutex<String> = Mutex::new(String::new());

fn submit_current_input() {
    let command = RUN_INPUT.lock().clone();
    if !command.trim().is_empty() {
        close_dialog_with_result(DialogResult::Text(command));
    }
}

fn cancel() {
    close_dialog_with_result(DialogResult::Cancel);
}

/// Open the Run dialog without blocking GUIShell. Completion is returned by
/// [`poll_run_dialog`].
pub fn open_run_dialog() -> Result<WindowId, &'static str> {
    if get_dialog_id().is_some() || RUN_DIALOG_ID.lock().is_some() {
        return Err("another modal dialog is already active");
    }
    RUN_INPUT.lock().clear();

    let frame_id = with_window_manager(|wm| {
        let (screen_width, screen_height) = wm.screen_dimensions();
        let dialog_x = (screen_width as i32 - DIALOG_WIDTH as i32) / 2;
        let dialog_y = (screen_height as i32 - DIALOG_HEIGHT as i32) / 2;
        let desktop_id = wm
            .get_active_screen()
            .and_then(|screen| screen.root_window)
            .ok_or("no desktop window")?;

        let frame_id = wm.create_window(Some(desktop_id));
        let mut frame = FrameWindow::new(frame_id, "Run");
        frame.set_resizable(false);
        frame.set_bounds(Rect::new(dialog_x, dialog_y, DIALOG_WIDTH, DIALOG_HEIGHT));
        frame.set_parent(Some(desktop_id));
        let content = frame.content_area();

        let container_id = wm.create_window(Some(frame_id));
        let mut container = ContainerWindow::new_with_id(container_id, content);
        container.set_parent(Some(frame_id));
        container.set_background_color(Color::new(192, 192, 192));
        frame.set_content_window(container_id);

        let line_one_id = wm.create_window(Some(container_id));
        let mut line_one = Label::new_with_id(
            line_one_id,
            Rect::new(12, 10, content.width.saturating_sub(24), 20),
            "Type the name of a program or command,",
        );
        line_one.set_parent(Some(container_id));

        let line_two_id = wm.create_window(Some(container_id));
        let mut line_two = Label::new_with_id(
            line_two_id,
            Rect::new(12, 30, content.width.saturating_sub(24), 20),
            "and AgenticOS will open it for you.",
        );
        line_two.set_parent(Some(container_id));

        let input_id = wm.create_window(Some(container_id));
        let mut input = TextInput::new_with_id(
            input_id,
            Rect::new(12, 60, content.width.saturating_sub(24), 26),
        );
        input.set_parent(Some(container_id));
        input.set_max_length(Some(INPUT_LIMIT));
        input.on_change(|text| {
            *RUN_INPUT.lock() = String::from(text);
        });
        input.on_submit(|_| submit_current_input());
        input.on_cancel(cancel);

        let cancel_id = wm.create_window(Some(container_id));
        let mut cancel_button = Button::new_with_id(
            cancel_id,
            Rect::new(content.width as i32 - 92, 100, 80, 28),
            "Cancel",
        );
        cancel_button.set_parent(Some(container_id));
        cancel_button.on_click(cancel);

        let ok_id = wm.create_window(Some(container_id));
        let mut ok_button = Button::new_with_id(
            ok_id,
            Rect::new(content.width as i32 - 180, 100, 80, 28),
            "OK",
        );
        ok_button.set_parent(Some(container_id));
        ok_button.on_click(submit_current_input);

        wm.set_window_impl(frame_id, Box::new(frame));
        wm.set_window_impl(container_id, Box::new(container));
        wm.set_window_impl(line_one_id, Box::new(line_one));
        wm.set_window_impl(line_two_id, Box::new(line_two));
        wm.set_window_impl(input_id, Box::new(input));
        wm.set_window_impl(ok_id, Box::new(ok_button));
        wm.set_window_impl(cancel_id, Box::new(cancel_button));

        wm.bring_to_front(frame_id);
        wm.set_modal_dialog(Some(frame_id));
        wm.focus_window(input_id);
        wm.force_full_repaint();
        Ok(frame_id)
    })
    .ok_or("window manager is not initialized")??;

    set_dialog_state(frame_id);
    *RUN_DIALOG_ID.lock() = Some(frame_id);
    Ok(frame_id)
}

/// Poll the Run dialog. `None` means it is still open (or was never opened),
/// `Some(Some(command))` is a submission, and `Some(None)` is cancellation.
pub fn poll_run_dialog() -> Option<Option<String>> {
    let frame_id = (*RUN_DIALOG_ID.lock())?;

    if is_dialog_open() {
        let exists =
            with_window_manager(|wm| wm.window_registry.contains_key(&frame_id)).unwrap_or(false);
        if exists {
            return None;
        }
        close_dialog_with_result(DialogResult::Cancel);
    }

    let result = get_dialog_result();
    with_window_manager(|wm| {
        wm.set_modal_dialog(None);
        if wm.window_registry.contains_key(&frame_id) {
            wm.destroy_window(frame_id);
        }
        wm.force_full_repaint();
    });
    clear_dialog_state();
    *RUN_DIALOG_ID.lock() = None;
    RUN_INPUT.lock().clear();

    match result {
        Some(DialogResult::Text(command)) => Some(Some(command)),
        _ => Some(None),
    }
}

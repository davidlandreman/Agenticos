use alloc::string::String;
use spin::Mutex;

use crate::graphics::surface::{Surface, SurfaceDesc};
use crate::window::event::{
    Event, KeyCode, KeyModifiers, KeyboardEvent, MouseButtons, MouseEvent, MouseEventType,
};
use crate::window::renderer::SurfaceCanvas;
use crate::window::theme::{self, ThemeKind};
use crate::window::windows::start_menu::{
    StartMenuAction, StartMenuItem, StartMenuWindow, START_MENU_BANNER_WIDTH,
    START_MENU_PROGRAMS_WIDTH, START_MENU_PROGRAM_ITEMS, START_MENU_PROGRAM_ROW_HEIGHT,
    START_MENU_ROOT_ITEMS, START_MENU_ROOT_ROW_HEIGHT, START_MENU_ROOT_WIDTH,
    START_MENU_SEPARATOR_HEIGHT,
};
use crate::window::windows::TextInput;
use crate::window::{Point, Rect, Window, WindowId};

static LAST_ACTION: Mutex<Option<StartMenuAction>> = Mutex::new(None);
static LAST_TEXT: Mutex<Option<String>> = Mutex::new(None);
static SUBMITTED: Mutex<Option<String>> = Mutex::new(None);
static CANCELLED: Mutex<bool> = Mutex::new(false);

fn mouse(event_type: MouseEventType, point: Point) -> Event {
    Event::Mouse(MouseEvent {
        event_type,
        position: point,
        global_position: point,
        buttons: MouseButtons {
            left: matches!(event_type, MouseEventType::ButtonDown),
            ..MouseButtons::default()
        },
        modifiers: KeyModifiers::default(),
    })
}

fn key(key_code: KeyCode) -> Event {
    Event::Keyboard(KeyboardEvent {
        key_code,
        pressed: true,
        modifiers: KeyModifiers::default(),
    })
}

fn root_row_center(index: usize) -> Point {
    let mut y = 2i32;
    for item in &START_MENU_ROOT_ITEMS[..index] {
        y += match item {
            StartMenuItem::Separator => START_MENU_SEPARATOR_HEIGHT,
            _ => START_MENU_ROOT_ROW_HEIGHT,
        } as i32;
    }
    let height = match START_MENU_ROOT_ITEMS[index] {
        StartMenuItem::Separator => START_MENU_SEPARATOR_HEIGHT,
        _ => START_MENU_ROOT_ROW_HEIGHT,
    };
    Point::new(START_MENU_BANNER_WIDTH as i32 + 12, y + height as i32 / 2)
}

fn program_row_center(index: usize) -> Point {
    Point::new(
        START_MENU_ROOT_WIDTH as i32 + 12,
        2 + index as i32 * START_MENU_PROGRAM_ROW_HEIGHT as i32
            + START_MENU_PROGRAM_ROW_HEIGHT as i32 / 2,
    )
}

fn test_menu_model_order_and_geometry() {
    assert_eq!(START_MENU_ROOT_ITEMS.len(), 6);
    assert!(matches!(
        START_MENU_ROOT_ITEMS[0],
        StartMenuItem::Submenu { label: "Programs" }
    ));
    assert!(matches!(
        START_MENU_ROOT_ITEMS[1],
        StartMenuItem::Disabled { label: "Documents" }
    ));
    assert!(matches!(
        START_MENU_ROOT_ITEMS[2],
        StartMenuItem::Action {
            label: "Settings",
            action: StartMenuAction::Settings
        }
    ));
    assert!(matches!(START_MENU_ROOT_ITEMS[4], StartMenuItem::Separator));
    assert!(matches!(
        START_MENU_ROOT_ITEMS[5],
        StartMenuItem::Action {
            label: "Shut Down...",
            action: StartMenuAction::ShutDown
        }
    ));
    assert_eq!(START_MENU_PROGRAM_ITEMS.len(), 7);
    assert!(matches!(
        START_MENU_PROGRAM_ITEMS[6],
        StartMenuItem::Action {
            label: "Task Manager",
            action: StartMenuAction::TaskManager
        }
    ));
    assert_eq!(StartMenuWindow::root_height(), 172);
    assert_eq!(
        StartMenuWindow::maximum_width(),
        START_MENU_ROOT_WIDTH + START_MENU_PROGRAMS_WIDTH - 2
    );
}

fn test_programs_flyout_hover_and_bounds() {
    let mut menu = StartMenuWindow::new_with_id(WindowId(8100), Point::new(0, 0));
    assert!(!menu.programs_open());
    assert_eq!(menu.bounds().width, START_MENU_ROOT_WIDTH);

    menu.handle_event(mouse(MouseEventType::Move, root_row_center(0)));
    assert!(menu.programs_open());
    assert_eq!(menu.bounds().width, StartMenuWindow::maximum_width());

    menu.handle_event(mouse(MouseEventType::Move, program_row_center(1)));
    assert!(menu.programs_open());

    menu.handle_event(mouse(MouseEventType::Move, root_row_center(1)));
    assert!(!menu.programs_open());
    assert_eq!(menu.bounds().width, START_MENU_ROOT_WIDTH);
}

fn test_enabled_and_disabled_dispatch() {
    *LAST_ACTION.lock() = None;
    let mut menu = StartMenuWindow::new_with_id(WindowId(8101), Point::new(0, 0));
    menu.on_select(|action| *LAST_ACTION.lock() = Some(action));

    menu.handle_event(mouse(MouseEventType::Move, root_row_center(0)));
    let calc = program_row_center(4);
    menu.handle_event(mouse(MouseEventType::ButtonDown, calc));
    menu.handle_event(mouse(MouseEventType::ButtonUp, calc));
    assert_eq!(*LAST_ACTION.lock(), Some(StartMenuAction::Calc));

    *LAST_ACTION.lock() = None;
    let gl_arena = program_row_center(5);
    menu.handle_event(mouse(MouseEventType::ButtonDown, gl_arena));
    menu.handle_event(mouse(MouseEventType::ButtonUp, gl_arena));
    assert_eq!(*LAST_ACTION.lock(), Some(StartMenuAction::GlGame));

    *LAST_ACTION.lock() = None;
    let documents = root_row_center(1);
    menu.handle_event(mouse(MouseEventType::ButtonDown, documents));
    menu.handle_event(mouse(MouseEventType::ButtonUp, documents));
    assert_eq!(*LAST_ACTION.lock(), None);

    let settings = root_row_center(2);
    menu.handle_event(mouse(MouseEventType::ButtonDown, settings));
    menu.handle_event(mouse(MouseEventType::ButtonUp, settings));
    assert_eq!(*LAST_ACTION.lock(), Some(StartMenuAction::Settings));

    let shutdown = root_row_center(5);
    menu.handle_event(mouse(MouseEventType::ButtonDown, shutdown));
    menu.handle_event(mouse(MouseEventType::ButtonUp, shutdown));
    assert_eq!(*LAST_ACTION.lock(), Some(StartMenuAction::ShutDown));
}

fn test_classic_start_menu_key_pixels() {
    let previous_theme = theme::active();
    theme::activate(ThemeKind::Classic);
    let mut menu = StartMenuWindow::new_with_id(WindowId(8102), Point::new(0, 0));
    let height = StartMenuWindow::root_height();
    let mut surface = Surface::new(SurfaceDesc::new(START_MENU_ROOT_WIDTH, height)).unwrap();
    {
        let mut canvas = SurfaceCanvas::new(
            &mut surface,
            (0, 0),
            (START_MENU_ROOT_WIDTH as usize, height as usize),
        );
        menu.paint(&mut canvas);
    }

    assert_eq!(surface.pixel(0, 0).unwrap().to_rgba(), (255, 255, 255, 255));
    assert_eq!(
        surface
            .pixel(START_MENU_ROOT_WIDTH - 1, height - 1)
            .unwrap()
            .to_rgba(),
        (0, 0, 0, 255)
    );
    assert_eq!(surface.pixel(3, 3).unwrap().to_rgba(), (0, 0, 128, 255));

    let mut banner_foreground = false;
    for y in 8..height - 5 {
        for x in 5..START_MENU_BANNER_WIDTH - 3 {
            if surface.pixel(x, y).unwrap().to_rgba() != (0, 0, 128, 255) {
                banner_foreground = true;
                break;
            }
        }
        if banner_foreground {
            break;
        }
    }
    assert!(banner_foreground, "rotated AgenticOS label should paint");

    assert_eq!(
        surface.pixel(34, 133).unwrap().to_rgba(),
        (128, 128, 128, 255)
    );
    assert_eq!(
        surface.pixel(34, 134).unwrap().to_rgba(),
        (255, 255, 255, 255)
    );

    menu.handle_event(mouse(MouseEventType::Move, root_row_center(0)));
    let mut hover_surface =
        Surface::new(SurfaceDesc::new(StartMenuWindow::maximum_width(), height)).unwrap();
    let hover_width = hover_surface.width();
    let hover_height = hover_surface.height();
    {
        let mut canvas = SurfaceCanvas::new(
            &mut hover_surface,
            (0, 0),
            (hover_width as usize, hover_height as usize),
        );
        menu.paint(&mut canvas);
    }
    assert_eq!(
        hover_surface.pixel(31, 3).unwrap().to_rgba(),
        (0, 0, 128, 255)
    );
    theme::activate(previous_theme);
}

fn test_aero_start_menu_uses_control_palette() {
    let previous_theme = theme::active();
    theme::activate(ThemeKind::Aero);
    let mut menu = StartMenuWindow::new_with_id(WindowId(8104), Point::new(0, 0));
    let height = StartMenuWindow::root_height();
    let mut surface = Surface::new(SurfaceDesc::new(START_MENU_ROOT_WIDTH, height)).unwrap();
    {
        let mut canvas = SurfaceCanvas::new(
            &mut surface,
            (0, 0),
            (START_MENU_ROOT_WIDTH as usize, height as usize),
        );
        menu.paint(&mut canvas);
    }

    menu.handle_event(mouse(MouseEventType::Move, root_row_center(0)));
    let mut hover_surface =
        Surface::new(SurfaceDesc::new(StartMenuWindow::maximum_width(), height)).unwrap();
    let hover_width = hover_surface.width();
    let hover_height = hover_surface.height();
    {
        let mut canvas = SurfaceCanvas::new(
            &mut hover_surface,
            (0, 0),
            (hover_width as usize, hover_height as usize),
        );
        menu.paint(&mut canvas);
    }
    theme::activate(previous_theme);

    assert_eq!(surface.pixel(0, 0).unwrap().to_rgba(), (151, 151, 151, 255));
    assert_eq!(
        surface.pixel(30, 3).unwrap().to_rgba(),
        (240, 240, 240, 255)
    );
    assert_eq!(surface.pixel(3, 3).unwrap().to_rgba(), (203, 232, 246, 255));
    assert_eq!(
        hover_surface.pixel(31, 3).unwrap().to_rgba(),
        (203, 232, 246, 255)
    );
}

fn test_text_input_callbacks_and_utf8_limit() {
    *LAST_TEXT.lock() = None;
    *SUBMITTED.lock() = None;
    *CANCELLED.lock() = false;
    let mut input = TextInput::new_with_id(WindowId(8103), Rect::new(0, 0, 160, 24));
    input.set_max_length(Some(3));
    input.on_change(|text| *LAST_TEXT.lock() = Some(String::from(text)));
    input.on_submit(|text| *SUBMITTED.lock() = Some(String::from(text)));
    input.on_cancel(|| *CANCELLED.lock() = true);

    input.set_text("éé");
    assert_eq!(input.text(), "é");
    assert_eq!(LAST_TEXT.lock().as_deref(), Some("é"));
    input.handle_event(key(KeyCode::Enter));
    assert_eq!(SUBMITTED.lock().as_deref(), Some("é"));
    input.handle_event(key(KeyCode::Escape));
    assert!(*CANCELLED.lock());
}

fn test_run_command_is_one_zsh_argument() {
    let command = "notepad '/data/my note.txt'";
    let argv = crate::commands::guishell::run_command_argv(command);
    assert_eq!(argv[0], crate::userland::process_service::ZSH_HOST_PATH);
    assert_eq!(argv[1], "-c");
    assert_eq!(argv[2], command);
}

pub fn get_tests() -> &'static [&'static dyn crate::lib::test_utils::Testable] {
    &[
        &test_menu_model_order_and_geometry,
        &test_programs_flyout_hover_and_bounds,
        &test_enabled_and_disabled_dispatch,
        &test_classic_start_menu_key_pixels,
        &test_aero_start_menu_uses_control_palette,
        &test_text_input_callbacks_and_utf8_limit,
        &test_run_command_is_one_zsh_argument,
    ]
}

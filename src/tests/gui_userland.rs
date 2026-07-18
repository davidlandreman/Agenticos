use crate::arch::x86_64::syscall::SyscallArgs;
use crate::userland::abi::{UserVaBounds, EAGAIN, EFAULT, EINVAL, ENOENT};
use crate::userland::gui::{
    self, GuiEvent, GuiWindowRecord, GUI_EVENT_KEY, GUI_EVENT_MOUSE, GUI_EVENT_QUEUE_CAPACITY,
    GUI_MOUSE_MOVE, GUI_NONBLOCK,
};
use crate::window::event::{
    Event, KeyCode, KeyModifiers, KeyboardEvent, MouseButtons, MouseEvent, MouseEventType,
};
use crate::window::{Point, WindowId};

fn test_gui_event_layout_and_encoding() {
    assert_eq!(core::mem::size_of::<GuiEvent>(), 32);
    let event = gui::encode_window_event(
        7,
        &Event::Keyboard(KeyboardEvent {
            key_code: KeyCode::A,
            pressed: true,
            modifiers: KeyModifiers {
                shift: true,
                ctrl: true,
                alt: false,
                meta: false,
            },
        }),
    )
    .expect("keyboard event must encode");
    assert_eq!(event.kind, GUI_EVENT_KEY);
    assert_eq!(event.window, 7);
    assert_eq!(event.payload[0], 1);
    assert_eq!(event.payload[1], 'A' as u32);
    assert_eq!(event.payload[2], 3);
    assert_eq!(event.payload[3], 1);
}

fn test_gui_mouse_button_encodes_timestamp_and_modifiers() {
    let before = crate::arch::x86_64::interrupts::get_timer_ticks();
    let event = gui::encode_window_event(
        9,
        &Event::Mouse(MouseEvent {
            event_type: MouseEventType::ButtonDown,
            position: Point::new(12, 34),
            global_position: Point::new(20, 40),
            buttons: MouseButtons {
                left: true,
                right: false,
                middle: false,
            },
            modifiers: KeyModifiers {
                shift: true,
                ctrl: true,
                alt: false,
                meta: false,
            },
        }),
    )
    .expect("mouse event must encode");
    let after = crate::arch::x86_64::interrupts::get_timer_ticks();
    let timestamp = event.payload[4] as u64 | ((event.payload[5] as u64) << 32);
    assert_eq!(event.payload[0], 12);
    assert_eq!(event.payload[1], 34);
    assert_eq!(event.payload[2] & 0xff, 1);
    assert_eq!(event.payload[2] >> 8, 3);
    assert_eq!(event.payload[3], crate::userland::gui::GUI_MOUSE_DOWN);
    assert!(timestamp >= before && timestamp <= after);
}

fn test_gui_queue_coalesces_mouse_moves() {
    gui::reset_for_test();
    let pid = 42;
    let first = GuiEvent {
        kind: GUI_EVENT_MOUSE,
        window: 1,
        payload: [10, 20, 0, GUI_MOUSE_MOVE, 0, 0],
    };
    let second = GuiEvent {
        kind: GUI_EVENT_MOUSE,
        window: 1,
        payload: [30, 40, 0, GUI_MOUSE_MOVE, 0, 0],
    };
    gui::enqueue_event(pid, first);
    gui::enqueue_event(pid, second);
    assert_eq!(gui::event_count_for_test(pid), 1);
    assert_eq!(gui::pop_event(pid), Some(second));
}

fn test_gui_queue_drops_oldest_at_capacity() {
    gui::reset_for_test();
    let pid = 43;
    for index in 0..GUI_EVENT_QUEUE_CAPACITY + 2 {
        gui::enqueue_event(
            pid,
            GuiEvent {
                kind: 4,
                window: index as u32,
                payload: [0; 6],
            },
        );
    }
    assert_eq!(gui::event_count_for_test(pid), GUI_EVENT_QUEUE_CAPACITY);
    assert_eq!(gui::pop_event(pid).unwrap().window, 2);
}

fn test_gui_cleanup_releases_pid_state() {
    gui::reset_for_test();
    let pid = 44;
    let handle = gui::allocate_handle(pid).expect("handle");
    gui::register_window(
        pid,
        handle,
        GuiWindowRecord {
            frame_id: WindowId::new(),
            surface_id: WindowId::new(),
        },
    )
    .expect("register");
    assert_eq!(gui::window_count_for_test(pid), 1);
    gui::cleanup_process(pid);
    assert_eq!(gui::window_count_for_test(pid), 0);
}

fn test_gui_next_event_nonblocking_and_bad_pointer() {
    gui::reset_for_test();
    let mut event = GuiEvent::default();
    let pointer = &mut event as *mut GuiEvent as u64;
    crate::userland::abi::set_user_va_bounds(UserVaBounds {
        start: pointer,
        end: pointer + core::mem::size_of::<GuiEvent>() as u64,
    });
    let mut args = SyscallArgs::default();
    args.rdi = pointer;
    args.rsi = core::mem::size_of::<GuiEvent>() as u64;
    args.rdx = GUI_NONBLOCK;
    assert_eq!(
        crate::userland::gui_syscalls::gui_next_event_handler(&mut args),
        EAGAIN
    );
    crate::userland::abi::clear_user_va_bounds();
    assert_eq!(
        crate::userland::gui_syscalls::gui_next_event_handler(&mut args),
        EFAULT
    );
}

fn test_gui_syscall_argument_errors() {
    let mut create = SyscallArgs::default();
    assert_eq!(
        crate::userland::gui_syscalls::gui_win_create_handler(&mut create),
        EINVAL
    );
    create.rdi = 100;
    create.rsi = 100;
    create.rdx = 1;
    create.r10 = 1;
    assert_eq!(
        crate::userland::gui_syscalls::gui_win_create_handler(&mut create),
        EFAULT
    );
    let mut destroy = SyscallArgs::default();
    destroy.rdi = 999;
    assert_eq!(
        crate::userland::gui_syscalls::gui_win_destroy_handler(&mut destroy),
        ENOENT
    );
}

fn test_gui_create_destroy_lifecycle() {
    gui::reset_for_test();
    // Test boot initializes the window manager but deliberately skips the
    // GUIShell desktop. Install a minimal root for the syscall lifecycle and
    // remove it afterward so the global manager returns to its prior state.
    let temporary_root = crate::window::with_window_manager(|wm| {
        if wm
            .get_active_screen()
            .and_then(|screen| screen.root_window)
            .is_some()
        {
            return None;
        }
        let root_id = wm.create_window(None);
        let (width, height) = wm.screen_dimensions();
        wm.set_window_impl(
            root_id,
            alloc::boxed::Box::new(crate::window::windows::DesktopWindow::new(
                root_id,
                crate::window::Rect::new(0, 0, width, height),
            )),
        );
        wm.get_active_screen_mut()
            .expect("active test screen")
            .set_root_window(root_id);
        Some(root_id)
    })
    .flatten();

    let mut create = SyscallArgs::default();
    create.rdi = 320;
    create.rsi = 200;
    let handle = crate::userland::gui_syscalls::gui_win_create_handler(&mut create);
    assert!(handle >= 1, "create failed with errno {}", handle);
    assert_eq!(
        gui::window_count_for_test(crate::userland::gui_syscalls::TEST_GUI_CALLER_PID),
        1
    );

    let title = b"Host - File Manager";
    let title_pointer = title.as_ptr() as u64;
    crate::userland::abi::set_user_va_bounds(UserVaBounds {
        start: title_pointer,
        end: title_pointer + title.len() as u64,
    });
    let mut set_title = SyscallArgs::default();
    set_title.rdi = handle as u64;
    set_title.rsi = title_pointer;
    set_title.rdx = title.len() as u64;
    assert_eq!(
        crate::userland::gui_syscalls::gui_win_set_title_handler(&mut set_title),
        0
    );
    crate::userland::abi::clear_user_va_bounds();
    let record = gui::window_record(
        crate::userland::gui_syscalls::TEST_GUI_CALLER_PID,
        handle as u32,
    )
    .expect("window record");
    let actual_title = crate::window::with_window_manager(|wm| {
        wm.window_registry
            .get(&record.frame_id)
            .and_then(|window| window.window_title())
            .map(alloc::string::String::from)
    })
    .flatten();
    assert_eq!(actual_title.as_deref(), Some("Host - File Manager"));

    let mut destroy = SyscallArgs::default();
    destroy.rdi = handle as u64;
    assert_eq!(
        crate::userland::gui_syscalls::gui_win_destroy_handler(&mut destroy),
        0
    );
    assert_eq!(
        gui::window_count_for_test(crate::userland::gui_syscalls::TEST_GUI_CALLER_PID),
        0
    );
    gui::reset_for_test();
    if let Some(root_id) = temporary_root {
        let _ = crate::window::with_window_manager(|wm| {
            wm.destroy_window(root_id);
            wm.get_active_screen_mut()
                .expect("active test screen")
                .root_window = None;
        });
    }
}

pub fn get_tests() -> &'static [&'static dyn crate::lib::test_utils::Testable] {
    &[
        &test_gui_event_layout_and_encoding,
        &test_gui_mouse_button_encodes_timestamp_and_modifiers,
        &test_gui_queue_coalesces_mouse_moves,
        &test_gui_queue_drops_oldest_at_capacity,
        &test_gui_cleanup_releases_pid_state,
        &test_gui_next_event_nonblocking_and_bad_pointer,
        &test_gui_syscall_argument_errors,
        &test_gui_create_destroy_lifecycle,
    ]
}

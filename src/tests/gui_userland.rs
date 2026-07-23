use crate::arch::x86_64::syscall::SyscallArgs;
use crate::graphics::composition::{ClientGlDraw, ClientGlVertex, CLIENT_GL_DRAW_DEPTH_TEST};
use crate::userland::abi::{UserVaBounds, EAGAIN, EFAULT, EINVAL, ENOENT, ENOSYS};
use crate::userland::fdtable::{FdSlot, FdTable, GuiEventHandle};
use crate::userland::gui::{
    self, GuiEvent, GuiWindowRecord, GUI_EVENT_KEY, GUI_EVENT_MOUSE, GUI_EVENT_QUEUE_CAPACITY,
    GUI_EVENT_SETTINGS_CHANGED, GUI_EVENT_THEME_CHANGED, GUI_MOUSE_MOVE, GUI_NONBLOCK,
};
use crate::window::event::{
    Event, KeyCode, KeyModifiers, KeyboardEvent, MouseButtons, MouseEvent, MouseEventType,
};
use crate::window::{Point, WindowId};

fn append_wire<T: Copy>(packet: &mut alloc::vec::Vec<u8>, value: &T) {
    // SAFETY: `value` is initialized and remains alive for the duration of
    // the copy. The wire structs contain only integer and floating fields.
    let bytes = unsafe {
        core::slice::from_raw_parts((value as *const T).cast::<u8>(), core::mem::size_of::<T>())
    };
    packet.extend_from_slice(bytes);
}

fn gl_packet(
    mut header: crate::userland::gui_gl::GlFrameHeader,
    draws: &[ClientGlDraw],
    vertices: &[ClientGlVertex],
) -> alloc::vec::Vec<u8> {
    let header_bytes = core::mem::size_of::<crate::userland::gui_gl::GlFrameHeader>();
    header.draw_count = draws.len() as u32;
    header.vertex_count = vertices.len() as u32;
    header.draw_offset = header_bytes as u32;
    header.vertex_offset = (header_bytes + core::mem::size_of_val(draws)) as u32;
    header.byte_len =
        (header_bytes + core::mem::size_of_val(draws) + core::mem::size_of_val(vertices)) as u32;
    let mut packet = alloc::vec::Vec::with_capacity(header.byte_len as usize);
    append_wire(&mut packet, &header);
    for draw in draws {
        append_wire(&mut packet, draw);
    }
    for vertex in vertices {
        append_wire(&mut packet, vertex);
    }
    packet
}

fn valid_gl_packet() -> alloc::vec::Vec<u8> {
    let header = crate::userland::gui_gl::GlFrameHeader {
        magic: crate::userland::gui_gl::GL_ABI_MAGIC,
        version: crate::userland::gui_gl::GL_ABI_VERSION,
        width: 640,
        height: 480,
        viewport_width: 640,
        viewport_height: 480,
        clear_color: [0.1, 0.2, 0.3, 1.0],
        clear_depth: 1.0,
        ..Default::default()
    };
    let draws = [ClientGlDraw {
        first_vertex: 0,
        vertex_count: 3,
        flags: CLIENT_GL_DRAW_DEPTH_TEST,
        reserved: 0,
    }];
    let vertices = [
        ClientGlVertex {
            position: [-0.5, -0.5, 0.0, 1.0],
            color: [1.0, 0.0, 0.0, 1.0],
        },
        ClientGlVertex {
            position: [0.5, -0.5, 0.0, 1.0],
            color: [0.0, 1.0, 0.0, 1.0],
        },
        ClientGlVertex {
            position: [0.0, 0.5, 0.0, 1.0],
            color: [0.0, 0.0, 1.0, 1.0],
        },
    ];
    gl_packet(header, &draws, &vertices)
}

fn test_gui_gl_packet_validation_accepts_canonical_triangle() {
    let frame = crate::userland::gui_gl::validate_packet_for_test(&valid_gl_packet())
        .expect("canonical packet must validate");
    assert_eq!(frame.width, 640);
    assert_eq!(frame.height, 480);
    assert_eq!(frame.draws.len(), 1);
    assert_eq!(frame.vertices.len(), 3);
    assert_eq!(frame.serial, 0);
}

fn test_gui_gl_packet_validation_rejects_untrusted_fields() {
    let mut bad_magic = valid_gl_packet();
    bad_magic[0] ^= 0xff;
    assert_eq!(
        crate::userland::gui_gl::validate_packet_for_test(&bad_magic).unwrap_err(),
        EINVAL
    );

    let mut bad_offset = valid_gl_packet();
    let offset = 72usize;
    bad_offset[offset..offset + 4].copy_from_slice(&0u32.to_ne_bytes());
    assert_eq!(
        crate::userland::gui_gl::validate_packet_for_test(&bad_offset).unwrap_err(),
        EINVAL
    );

    let mut nan_vertex = valid_gl_packet();
    let vertex_offset = core::mem::size_of::<crate::userland::gui_gl::GlFrameHeader>()
        + core::mem::size_of::<ClientGlDraw>();
    nan_vertex[vertex_offset..vertex_offset + 4].copy_from_slice(&f32::NAN.to_ne_bytes());
    assert_eq!(
        crate::userland::gui_gl::validate_packet_for_test(&nan_vertex).unwrap_err(),
        EINVAL
    );
}

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

fn test_gui_event_descriptor_open_flags_and_fork_ownership() {
    const O_NONBLOCK: u64 = 0x800;
    const O_CLOEXEC: u64 = 0x80000;
    let mut invalid = SyscallArgs::default();
    invalid.rdi = 1;
    assert_eq!(
        crate::userland::gui_syscalls::gui_event_open_handler(&mut invalid),
        EINVAL
    );

    let mut args = SyscallArgs::default();
    args.rdi = O_NONBLOCK | O_CLOEXEC;
    let fd = crate::userland::gui_syscalls::gui_event_open_handler(&mut args);
    assert!(fd >= 3, "GUI event open failed with {fd}");
    crate::userland::lifecycle::with_active_user(|process| {
        match process.fd_table.get(fd as i32).expect("event fd") {
            FdSlot::GuiEvents { handle, cloexec } => {
                assert_eq!(
                    handle.owner_pid(),
                    crate::userland::gui_syscalls::TEST_GUI_CALLER_PID
                );
                assert!(handle.nonblocking());
                assert!(*cloexec);
            }
            _ => panic!("wrong descriptor type"),
        }
        process.fd_table.close(fd as i32).expect("close event fd");
    });

    let handle = GuiEventHandle::new(77, false);
    let mut table = FdTable::new();
    let original = table
        .alloc(FdSlot::GuiEvents {
            handle: handle.clone(),
            cloexec: false,
        })
        .expect("allocate event fd");
    let duplicate = table.dup(original).expect("duplicate event fd");
    handle.set_nonblocking(true);
    match table.get(duplicate).expect("duplicated event fd") {
        FdSlot::GuiEvents { handle, .. } => assert!(handle.nonblocking()),
        _ => panic!("wrong duplicate type"),
    }
    assert!(table.fork_clone().get(original).is_none());
    assert!(table.fork_clone().get(duplicate).is_none());
}

fn test_gui_event_descriptor_readiness_tracks_queue() {
    gui::reset_for_test();
    let pid = 78;
    assert!(!gui::has_events(pid));
    gui::enqueue_event(
        pid,
        GuiEvent {
            kind: GUI_EVENT_KEY,
            window: 4,
            payload: [1, 'a' as u32, 0, 1, 0, 0],
        },
    );
    assert!(gui::has_events(pid));
    assert!(gui::pop_event(pid).is_some());
    assert!(!gui::has_events(pid));
}

fn test_theme_broadcast_targets_gui_owners_and_coalesces() {
    gui::reset_for_test();
    let pid = 45;
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
    gui::broadcast_theme_changed(
        crate::window::theme::ThemeKind::Aero,
        crate::window::theme::ThemeRequest::Aero,
    );
    gui::broadcast_theme_changed(
        crate::window::theme::ThemeKind::Classic,
        crate::window::theme::ThemeRequest::Classic,
    );
    assert_eq!(gui::event_count_for_test(pid), 1);
    let event = gui::pop_event(pid).expect("theme event");
    assert_eq!(event.kind, GUI_EVENT_THEME_CHANGED);
    assert_eq!(event.window, 0);
    assert_eq!(event.payload[0], 1);
    assert_eq!(event.payload[1], 1);

    // Futurism broadcasts as code 3 (ring-3 apps decode 3 => Futurism).
    gui::broadcast_theme_changed(
        crate::window::theme::ThemeKind::Futurism,
        crate::window::theme::ThemeRequest::Futurism,
    );
    let event = gui::pop_event(pid).expect("futurism theme event");
    assert_eq!(event.payload[0], 3);
    assert_eq!(event.payload[1], 3);
    gui::reset_for_test();
}

fn test_settings_broadcast_targets_gui_owners_and_coalesces() {
    gui::reset_for_test();
    let pid = 46;
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
    gui::broadcast_settings_changed();
    gui::broadcast_settings_changed();
    assert_eq!(gui::event_count_for_test(pid), 1);
    let event = gui::pop_event(pid).expect("settings event");
    assert_eq!(event.kind, GUI_EVENT_SETTINGS_CHANGED);
    assert_eq!(event.window, 0);
    gui::reset_for_test();
}

fn test_gui_cleanup_releases_pid_state() {
    gui::reset_for_test();
    let pid = 44;
    let handle = gui::allocate_handle(pid).expect("handle");
    let surface_id = WindowId::new();
    gui::register_window(
        pid,
        handle,
        GuiWindowRecord {
            frame_id: WindowId::new(),
            surface_id,
        },
    )
    .expect("register");
    let _master = crate::terminal::pty::install_for_terminal(surface_id, 24, 80);
    assert!(crate::terminal::pty::is_active_for_terminal(surface_id));
    assert_eq!(gui::window_count_for_test(pid), 1);
    gui::cleanup_process(pid);
    assert_eq!(gui::window_count_for_test(pid), 0);
    assert!(!crate::terminal::pty::is_active_for_terminal(surface_id));
}

fn test_retired_terminal_spawn_syscall_is_enosys() {
    let mut args = SyscallArgs::default();
    args.rax = 5018;
    assert_eq!(crate::userland::abi::syscall_dispatch(&mut args), ENOSYS);
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
    let mut too_narrow = SyscallArgs::default();
    too_narrow.rdi = crate::window::theme::minimum_resizable_client_width() as u64 - 1;
    too_narrow.rsi = 100;
    assert_eq!(
        crate::userland::gui_syscalls::gui_win_create_handler(&mut too_narrow),
        EINVAL
    );
    let mut unknown_flags = SyscallArgs::default();
    unknown_flags.rdi = 100;
    unknown_flags.rsi = 100;
    // Bit 3 is outside the defined flag set (fixed-size | undecorated | panel).
    unknown_flags.r8 = 1 << 3;
    assert_eq!(
        crate::userland::gui_syscalls::gui_win_create_handler(&mut unknown_flags),
        EINVAL
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
    // Test boot initializes the window manager but deliberately skips the    // desktop shell. Install a minimal root for the syscall lifecycle and
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
    let resizable = crate::window::with_window_manager(|wm| {
        wm.window_registry
            .get(&record.frame_id)
            .and_then(|window| window.as_frame_window())
            .is_some_and(|frame| frame.is_resizable())
    })
    .unwrap_or(false);
    assert!(
        resizable,
        "flags=0 must remain backward-compatible/resizable"
    );
    let actual_title = crate::window::with_window_manager(|wm| {
        wm.window_registry
            .get(&record.frame_id)
            .and_then(|window| window.window_title())
            .map(alloc::string::String::from)
    })
    .flatten();
    assert_eq!(actual_title.as_deref(), Some("Host - File Manager"));

    for cursor_kind in 0..=2 {
        let mut set_cursor = SyscallArgs::default();
        set_cursor.rdi = handle as u64;
        set_cursor.rsi = cursor_kind;
        assert_eq!(
            crate::userland::gui_syscalls::gui_win_set_cursor_handler(&mut set_cursor),
            0
        );
    }
    let mut invalid_cursor = SyscallArgs::default();
    invalid_cursor.rdi = handle as u64;
    invalid_cursor.rsi = 99;
    assert_eq!(
        crate::userland::gui_syscalls::gui_win_set_cursor_handler(&mut invalid_cursor),
        EINVAL
    );
    let mut missing_cursor = SyscallArgs::default();
    missing_cursor.rdi = u32::MAX as u64;
    missing_cursor.rsi = crate::window::CursorIcon::Arrow as u64;
    assert_eq!(
        crate::userland::gui_syscalls::gui_win_set_cursor_handler(&mut missing_cursor),
        ENOENT
    );
    let mut actual_cursor = None;
    let _ = crate::window::with_window_manager(|wm| {
        wm.with_window_mut(record.surface_id, |window| {
            actual_cursor = window
                .as_remote_surface_mut()
                .map(|surface| surface.cursor_icon());
        });
    });
    assert_eq!(actual_cursor, Some(crate::window::CursorIcon::Text));

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

    let mut create_fixed = SyscallArgs::default();
    create_fixed.rdi = 80;
    create_fixed.rsi = 100;
    create_fixed.r8 = gui::GUI_WINDOW_FIXED_SIZE;
    let fixed_handle = crate::userland::gui_syscalls::gui_win_create_handler(&mut create_fixed);
    assert!(
        fixed_handle >= 1,
        "fixed create failed with errno {}",
        fixed_handle
    );
    let fixed_record = gui::window_record(
        crate::userland::gui_syscalls::TEST_GUI_CALLER_PID,
        fixed_handle as u32,
    )
    .expect("fixed window record");
    let fixed_is_resizable = crate::window::with_window_manager(|wm| {
        wm.window_registry
            .get(&fixed_record.frame_id)
            .and_then(|window| window.as_frame_window())
            .is_some_and(|frame| frame.is_resizable())
    })
    .unwrap_or(true);
    assert!(!fixed_is_resizable);
    let mut destroy_fixed = SyscallArgs::default();
    destroy_fixed.rdi = fixed_handle as u64;
    assert_eq!(
        crate::userland::gui_syscalls::gui_win_destroy_handler(&mut destroy_fixed),
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

fn test_desktop_shell_registration() {
    use crate::userland::abi::EEXIST;
    gui::reset_for_test();
    let a = 4242u32;
    let b = 4343u32;
    assert!(!gui::is_desktop_shell(a));
    assert_eq!(gui::register_desktop_shell(a), Ok(()));
    assert!(gui::is_desktop_shell(a));
    // Idempotent for the current holder.
    assert_eq!(gui::register_desktop_shell(a), Ok(()));
    // A different live shell is rejected while the seat is held.
    assert_eq!(gui::register_desktop_shell(b), Err(EEXIST));
    assert!(!gui::is_desktop_shell(b));
    // Process exit releases the role; the seat reopens.
    gui::cleanup_process(a);
    assert!(!gui::is_desktop_shell(a));
    assert_eq!(gui::register_desktop_shell(b), Ok(()));
    assert!(gui::is_desktop_shell(b));
    gui::reset_for_test();
    assert!(!gui::is_desktop_shell(b));
}

fn test_shell_syscalls_require_registration() {
    use crate::userland::abi::EPERM;
    gui::reset_for_test();
    let pid = crate::userland::gui_syscalls::TEST_GUI_CALLER_PID;
    assert!(!gui::is_desktop_shell(pid));

    // Chrome-surface creation is refused for non-shell callers.
    let mut undecorated = SyscallArgs::default();
    undecorated.rdi = 200;
    undecorated.rsi = 40;
    undecorated.r8 = gui::GUI_WINDOW_UNDECORATED;
    assert_eq!(
        crate::userland::gui_syscalls::gui_win_create_handler(&mut undecorated),
        EPERM
    );
    let mut panel = SyscallArgs::default();
    panel.rdi = 200;
    panel.rsi = 40;
    panel.r8 = gui::GUI_WINDOW_PANEL;
    assert_eq!(
        crate::userland::gui_syscalls::gui_win_create_handler(&mut panel),
        EPERM
    );

    // The gui_shell_* syscalls are likewise refused.
    let mut list = SyscallArgs::default();
    list.rsi = 4096;
    assert_eq!(
        crate::userland::gui_syscalls::gui_shell_list_windows_handler(&mut list),
        EPERM
    );
    let mut action = SyscallArgs::default();
    action.rdi = 1;
    assert_eq!(
        crate::userland::gui_syscalls::gui_shell_window_action_handler(&mut action),
        EPERM
    );
    gui::reset_for_test();
}

fn test_shell_window_list_and_action() {
    gui::reset_for_test();
    let pid = crate::userland::gui_syscalls::TEST_GUI_CALLER_PID;

    // Install a temporary desktop root (test boot skips the desktop shell).
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

    // Create an ordinary application frame.
    let mut create = SyscallArgs::default();
    create.rdi = 320;
    create.rsi = 200;
    let handle = crate::userland::gui_syscalls::gui_win_create_handler(&mut create);
    assert!(handle >= 1, "create failed with errno {}", handle);
    let record = gui::window_record(pid, handle as u32).expect("window record");
    let frame_id = record.frame_id;

    // Claim the shell role, then the frame must appear in the list at state 0.
    assert_eq!(gui::register_desktop_shell(pid), Ok(()));
    let listed =
        crate::window::with_window_manager(|wm| wm.shell_window_list()).unwrap_or_default();
    let entry = listed
        .iter()
        .find(|(id, _, _)| *id == frame_id)
        .expect("frame present in shell list");
    assert_eq!(entry.1.as_str(), "AgenticOS Application");
    assert_eq!(entry.2, 0);

    // The list syscall copies the same record into a user buffer.
    let mut buffer = [0u8; core::mem::size_of::<gui::ShellWindowRecord>() * 4];
    let base = buffer.as_mut_ptr() as u64;
    crate::userland::abi::set_user_va_bounds(UserVaBounds {
        start: base,
        end: base + buffer.len() as u64,
    });
    let mut list = SyscallArgs::default();
    list.rdi = base;
    list.rsi = buffer.len() as u64;
    let count = crate::userland::gui_syscalls::gui_shell_list_windows_handler(&mut list);
    crate::userland::abi::clear_user_va_bounds();
    assert!(count >= 1, "list returned errno {}", count);
    let first = &buffer[..core::mem::size_of::<gui::ShellWindowRecord>()];
    let reported_id = u64::from_le_bytes(first[0..8].try_into().unwrap());
    let reported_state = u32::from_le_bytes(first[8..12].try_into().unwrap());
    assert_eq!(reported_id, frame_id.0 as u64);
    assert_eq!(reported_state, 0);

    // Minimize/activate cycle updates the reported state.
    assert!(
        crate::window::with_window_manager(|wm| wm.shell_window_action(frame_id, 1))
            .unwrap_or(false)
    );
    let minimized = crate::window::with_window_manager(|wm| wm.shell_window_list())
        .unwrap_or_default()
        .into_iter()
        .find(|(id, _, _)| *id == frame_id)
        .map(|(_, _, state)| state);
    assert_eq!(minimized, Some(1));
    assert!(
        crate::window::with_window_manager(|wm| wm.shell_window_action(frame_id, 0))
            .unwrap_or(false)
    );

    // Close removes the frame entirely.
    assert!(
        crate::window::with_window_manager(|wm| wm.shell_window_action(frame_id, 4))
            .unwrap_or(false)
    );
    let after_close =
        crate::window::with_window_manager(|wm| wm.shell_window_list()).unwrap_or_default();
    assert!(after_close.iter().all(|(id, _, _)| *id != frame_id));

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
        &test_desktop_shell_registration,
        &test_shell_syscalls_require_registration,
        &test_shell_window_list_and_action,
        &test_gui_event_layout_and_encoding,
        &test_gui_mouse_button_encodes_timestamp_and_modifiers,
        &test_gui_queue_coalesces_mouse_moves,
        &test_gui_queue_drops_oldest_at_capacity,
        &test_gui_event_descriptor_open_flags_and_fork_ownership,
        &test_gui_event_descriptor_readiness_tracks_queue,
        &test_theme_broadcast_targets_gui_owners_and_coalesces,
        &test_settings_broadcast_targets_gui_owners_and_coalesces,
        &test_gui_cleanup_releases_pid_state,
        &test_retired_terminal_spawn_syscall_is_enosys,
        &test_gui_next_event_nonblocking_and_bad_pointer,
        &test_gui_syscall_argument_errors,
        &test_gui_create_destroy_lifecycle,
        &test_gui_gl_packet_validation_accepts_canonical_triangle,
        &test_gui_gl_packet_validation_rejects_untrusted_fields,
    ]
}

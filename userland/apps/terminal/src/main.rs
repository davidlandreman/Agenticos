//! TERMINAL.ELF — the ring-3 terminal emulator.
//!
//! Linux keeps the pty + line discipline in the kernel and the terminal
//! *emulator* (xterm, Terminal.app) in userland; this is that split for
//! AgenticOS. The kernel owns `src/terminal/pty.rs`; this app owns the VT
//! parser, screen grid, scrollback, caret, key encoding, and glyph rendering
//! (via the `vte` and `termgrid` crates ported from the kernel).
//!
//! Flow: create a GUI window → `pty_open` the master for that window (which
//! also binds our `terminal_id`) → `fork`+`execve` zsh on the slave (inherited
//! through `terminal_id`) → poll the master fd and GUI events, parse output
//! into the `Screen`, encode keystrokes back to the master, and present the
//! rendered cells. The compositor no longer pumps this terminal — this app is
//! its own data pump.
//!
//! v1 scope: fixed 80×24 window (the `Screen` has no resize yet) and
//! full-surface presents. The kernel line discipline covers cooked-mode
//! editing (echo/VERASE/VKILL), VEOF end-of-file, and ISIG (^C/^\) for
//! canonical-mode children; zsh itself drives the tty raw. Shift+PgUp/PgDn
//! scroll local history. Live resize is the documented follow-up.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

use runtime::{
    clock_gettime, exit, fork, gui_next_event, gui_win_create, gui_win_destroy, gui_win_present,
    gui_win_set_title, kill, nanosleep, pty_open, read, wait4, write, GuiEvent, Timespec,
    CLOCK_MONOTONIC, GUI_EVENT_CLOSE, GUI_EVENT_FOCUS_CHANGE, GUI_EVENT_KEY, GUI_NONBLOCK,
    GUI_WINDOW_FIXED_SIZE, PTY_OPEN_CLOEXEC,
};
use termgrid::{render, RenderParams, TermFont, DEFAULT_FONT_PX};
use vte::caret::blink_on_at;
use vte::{decode_key_code, keys, KeyCode, KeyModifiers, Screen, Vte};

const ROWS: u16 = 24;
const COLS: u16 = 80;
/// Terminal content-well background (matches the kernel default `#202020`).
const WELL_BG: u32 = 0x0020_2020;
const WNOHANG: u32 = 1;
const SIGTERM: i32 = 15;
const SIGKILL: i32 = 9;
/// Frame pacing: ~16 ms between polls.
const FRAME_NS: i64 = 16_000_000;

// Null-terminated C strings for the child's execve.
const ZSH_PATH: &[u8] = b"/host/ZSH.ELF\0";
const ENV_PATH: &[u8] = b"PATH=/bin:/host\0";
const ENV_HOME: &[u8] = b"HOME=/root\0";
const ENV_USER: &[u8] = b"USER=root\0";
const ENV_LOGNAME: &[u8] = b"LOGNAME=root\0";
const ENV_SHELL: &[u8] = b"SHELL=/bin/zsh\0";
const ENV_TERM: &[u8] = b"TERM=xterm-256color\0";
const ENV_COLORTERM: &[u8] = b"COLORTERM=truecolor\0";
const ENV_LANG: &[u8] = b"LANG=C.UTF-8\0";

/// Terminate the child shell on window close. Interactive zsh ignores
/// SIGTERM, so give it a short grace and then SIGKILL (unblockable),
/// reaping either way so no zombie outlives the emulator.
fn shutdown_child(child_pid: i32) {
    kill(child_pid, SIGTERM);
    let grace = Timespec { tv_sec: 0, tv_nsec: 20_000_000 };
    for _ in 0..10 {
        if wait4(child_pid, None, WNOHANG) > 0 {
            return;
        }
        nanosleep(&grace, None);
    }
    kill(child_pid, SIGKILL);
    wait4(child_pid, None, 0);
}

fn monotonic_ms() -> u64 {
    let mut ts = Timespec { tv_sec: 0, tv_nsec: 0 };
    if clock_gettime(CLOCK_MONOTONIC, &mut ts) < 0 {
        return 0;
    }
    (ts.tv_sec as u64) * 1000 + (ts.tv_nsec as u64) / 1_000_000
}

/// Replace this process image with zsh on the pty slave. Never returns on
/// success; exits the child on failure.
fn exec_shell() -> ! {
    let argv: [*const u8; 2] = [ZSH_PATH.as_ptr(), core::ptr::null()];
    let envp: [*const u8; 9] = [
        ENV_PATH.as_ptr(),
        ENV_HOME.as_ptr(),
        ENV_USER.as_ptr(),
        ENV_LOGNAME.as_ptr(),
        ENV_SHELL.as_ptr(),
        ENV_TERM.as_ptr(),
        ENV_COLORTERM.as_ptr(),
        ENV_LANG.as_ptr(),
        core::ptr::null(),
    ];
    runtime::execve(ZSH_PATH, &argv, &envp);
    // execve only returns on failure.
    unsafe { exit(127) }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Cell metrics first, so we can size the window to an exact 80×24 grid.
    let mut font = TermFont::new(DEFAULT_FONT_PX);
    let cell_w = font.cell_width() as usize;
    let line_h = font.line_height() as usize;
    let width_px = COLS as usize * cell_w;
    let height_px = ROWS as usize * line_h;

    let handle = gui_win_create(
        width_px as u32,
        height_px as u32,
        "Terminal",
        GUI_WINDOW_FIXED_SIZE,
    );
    if handle < 0 {
        unsafe { exit(1) }
    }
    let handle = handle as u32;

    // Open the pty master for this window. This also binds our terminal_id so
    // the forked child inherits the slave. CLOEXEC keeps the master out of the
    // child after execve.
    let master_fd = pty_open(handle, ROWS, COLS, PTY_OPEN_CLOEXEC);
    if master_fd < 0 {
        gui_win_destroy(handle);
        unsafe { exit(1) }
    }
    let master_fd = master_fd as i32;

    let child_pid = match fork() {
        0 => exec_shell(),
        pid if pid < 0 => {
            gui_win_destroy(handle);
            unsafe { exit(1) }
        }
        pid => pid as i32,
    };

    run(handle, master_fd, child_pid, &mut font, width_px, height_px)
}

fn run(
    handle: u32,
    master_fd: i32,
    child_pid: i32,
    font: &mut TermFont,
    width_px: usize,
    height_px: usize,
) -> ! {
    let mut screen = Screen::new(ROWS as usize, COLS as usize);
    let mut vte = Vte::new();
    let mut fb: Vec<u32> = vec![WELL_BG; width_px * height_px];
    let mut read_buf = [0u8; 4096];

    let mut focused = true;
    let mut dirty = true;
    let mut last_caret_on = false;

    loop {
        // 1. Drain GUI events (non-blocking). `gui_next_event` returns 0 when
        // it copied an event, and -EAGAIN once the queue is empty.
        let mut event = GuiEvent::default();
        while gui_next_event(&mut event, GUI_NONBLOCK) == 0 {
            match event.kind {
                GUI_EVENT_KEY => {
                    // payload: [keycode, char, mods, pressed, ..]
                    if event.payload[3] == 1 {
                        let key = decode_key_code(event.payload[0]);
                        let mods = KeyModifiers::from_payload(event.payload[2]);
                        if mods.shift && (key == KeyCode::PageUp || key == KeyCode::PageDown) {
                            // Local scrollback view, matching the kernel
                            // terminal's Shift+PgUp/PgDn binding.
                            let page = ROWS as isize - 1;
                            let delta = if key == KeyCode::PageUp { page } else { -page };
                            screen.scroll_view(delta);
                            dirty = true;
                        } else {
                            let bytes = keys::encode_keystroke(key, mods);
                            if !bytes.is_empty() {
                                if screen.view_offset() != 0 {
                                    // Typing snaps the view back to live.
                                    screen.scroll_view(-(screen.view_offset() as isize));
                                    dirty = true;
                                }
                                write(master_fd, &bytes);
                            }
                        }
                    }
                }
                GUI_EVENT_FOCUS_CHANGE => {
                    focused = event.payload[0] != 0;
                    dirty = true;
                }
                GUI_EVENT_CLOSE => {
                    shutdown_child(child_pid);
                    gui_win_destroy(handle);
                    unsafe { exit(0) }
                }
                _ => {}
            }
            event = GuiEvent::default();
        }

        // 2. Drain shell output from the pty master (non-blocking).
        loop {
            let n = read(master_fd, &mut read_buf);
            if n <= 0 {
                break;
            }
            for &byte in &read_buf[..n as usize] {
                vte.advance(byte, &mut screen);
            }
            dirty = true;
            if (n as usize) < read_buf.len() {
                break;
            }
        }

        // 3. Push any answerback replies (DSR/DA) back to the shell.
        let replies = screen.take_replies();
        if !replies.is_empty() {
            write(master_fd, &replies);
        }

        // 4. Window title from OSC 0/2.
        if let Some(title) = screen.take_title() {
            gui_win_set_title(handle, &title);
        }

        // 5. Repaint if the grid changed or the caret blink phase flipped.
        let caret_on = blink_on_at(monotonic_ms());
        if dirty || caret_on != last_caret_on {
            let params = RenderParams {
                width_px,
                height_px,
                default_bg: WELL_BG,
                focused,
                caret_on,
            };
            render(font, &screen, &mut fb, &params);
            gui_win_present(handle, &fb, width_px as u32, height_px as u32);
            dirty = false;
            last_caret_on = caret_on;
        }

        // 6. Reap the shell; exit when it does.
        if wait4(child_pid, None, WNOHANG) > 0 {
            gui_win_destroy(handle);
            unsafe { exit(0) }
        }

        let req = Timespec { tv_sec: 0, tv_nsec: FRAME_NS };
        nanosleep(&req, None);
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe { exit(1) }
}

//! Per-process TTY state — termios + window size.
//!
//! Phase 3: we now claim that `fd 0/1/2` are TTYs. zsh's line editor
//! (`zle`) discovers this via `tcgetattr` (i.e. `ioctl(TCGETS)`) and
//! flips the terminal into raw mode so it can do its own redraw on
//! every keystroke.
//!
//! Lifetime: installed by `enter_user_mode_with` before iretq, cleared
//! by `release_active_image`. Single user process at a time (D5) →
//! one global Termios is enough.
//!
//! This module deliberately stays input-side. Output processing (`OPOST`,
//! `ONLCR`) is a future improvement; today the kernel emits `"\n"` itself
//! and that's good enough for the zsh-style line editor.

use spin::Mutex;

/// Linux `NCCS` for `struct termios` — number of control-character slots.
pub const NCCS: usize = 19;

// ---- c_iflag bits ----
pub const ICRNL: u32 = 0o000400;
pub const IXON: u32 = 0o002000;

// ---- c_oflag bits ----
pub const OPOST: u32 = 0o000001;
pub const ONLCR: u32 = 0o000004;

// ---- c_lflag bits ----
pub const ISIG: u32 = 0o000001;
pub const ICANON: u32 = 0o000002;
pub const ECHO: u32 = 0o000010;
pub const ECHOE: u32 = 0o000020;
pub const ECHOK: u32 = 0o000040;
pub const IEXTEN: u32 = 0o100000;

// ---- c_cc indices (Linux x86-64) ----
pub const VINTR: usize = 0; // Ctrl-C → SIGINT (Phase 5)
pub const VQUIT: usize = 1; // Ctrl-\ → SIGQUIT
pub const VERASE: usize = 2; // backspace
pub const VKILL: usize = 3; // line erase
pub const VEOF: usize = 4; // Ctrl-D
pub const VTIME: usize = 5;
pub const VMIN: usize = 6;
pub const VSUSP: usize = 10; // Ctrl-Z

/// Linux `struct termios` (x86-64). 36 bytes — `c_line` is one byte
/// followed immediately by `c_cc[19]`. Speed is encoded in `c_cflag`
/// bits; we don't model it.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Termios {
    pub c_iflag: u32,
    pub c_oflag: u32,
    pub c_cflag: u32,
    pub c_lflag: u32,
    pub c_line: u8,
    pub c_cc: [u8; NCCS],
}
const _SIZE_CHECK: () = assert!(core::mem::size_of::<Termios>() == 36);

impl Termios {
    /// Linux's "sane" defaults for a freshly opened TTY: line-buffered
    /// canonical mode with echo on, ICRNL translating CR to LF on
    /// input, and the conventional control characters.
    pub const fn default_tty() -> Self {
        let mut cc = [0u8; NCCS];
        cc[VINTR] = 0x03;
        cc[VQUIT] = 0x1C;
        cc[VERASE] = 0x7F;
        cc[VKILL] = 0x15;
        cc[VEOF] = 0x04;
        cc[VTIME] = 0;
        cc[VMIN] = 1;
        cc[VSUSP] = 0x1A;
        Self {
            c_iflag: ICRNL | IXON,
            c_oflag: OPOST | ONLCR,
            c_cflag: 0,
            c_lflag: ICANON | ECHO | ECHOE | ECHOK | ISIG | IEXTEN,
            c_line: 0,
            c_cc: cc,
        }
    }
}

/// Linux `struct winsize` for `TIOCGWINSZ`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Winsize {
    pub ws_row: u16,
    pub ws_col: u16,
    pub ws_xpixel: u16,
    pub ws_ypixel: u16,
}

static TERMIOS: Mutex<Termios> = Mutex::new(Termios::default_tty());

pub fn install_default() {
    *TERMIOS.lock() = Termios::default_tty();
}

pub fn snapshot() -> Termios {
    *TERMIOS.lock()
}

pub fn set(t: Termios) {
    *TERMIOS.lock() = t;
}

pub fn is_canonical() -> bool {
    (TERMIOS.lock().c_lflag & ICANON) != 0
}

pub fn is_echo() -> bool {
    (TERMIOS.lock().c_lflag & ECHO) != 0
}

pub fn icrnl() -> bool {
    (TERMIOS.lock().c_iflag & ICRNL) != 0
}

/// Default winsize for a focused terminal. zsh consults this to decide
/// where to wrap. 80x24 is the universally-safe default; the value
/// could be derived from the focused TerminalWindow's grid in a
/// follow-up.
pub fn winsize() -> Winsize {
    Winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

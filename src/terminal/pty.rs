//! PTY pair — pseudo-terminal master/slave for ring-3 processes.
//!
//! Replaces three previously-independent surfaces:
//!
//! - `userland::stdin` (per-WindowId byte queues feeding the slave's
//!   `read(0)`).
//! - `window::terminal`'s per-WindowId output buffers (slave's
//!   `write(1/2)` queueing into `String`s for the compositor to drain).
//! - `userland::tty`'s global `static TERMIOS` and 80×24 hardcoded
//!   `Winsize`.
//!
//! One [`PtyInner`] now owns all of those: a `slave_queue` for input,
//! a `master_queue` for output, a per-pty `Termios`, and a `Winsize`
//! that the [`PtyMaster`] updates when the host grid changes.
//!
//! A pty pair is registered per `WindowId`. Processes find their pty
//! through their existing `Process.terminal_id` field — the model is
//! the same as before, just with everything that used to live in three
//! mutexes consolidated under one `Arc<Mutex<PtyInner>>`. Multi-process
//! per-pty (fork) is handled the same way the WindowId queues handled
//! it: every process with the same `terminal_id` shares the pty.
//!
//! Line discipline lives here. Today the in-pty discipline is minimal —
//! the [`PtyMaster::push_input`] path leaves canonical-mode line
//! buffering, echo, and signal generation to the caller (which today
//! is `TerminalWindow`). That coupling will be tightened in a follow-up
//! once U9 lets the screen render echoes through the parser.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;
use spin::Mutex;

use crate::lib::arc::Arc;
use crate::window::WindowId;

use super::config;

// ---------------------------------------------------------------------
// Termios / Winsize
// ---------------------------------------------------------------------
//
// Layout matches Linux x86-64 `struct termios` and `struct winsize`.
// We previously inherited this from `userland::tty` — moving it here
// makes the pty the single owner of tty state, but the bit constants
// stay byte-compatible with what zsh and BusyBox expect.

pub const NCCS: usize = 19;

// c_iflag bits
pub const ICRNL: u32 = 0o000400;
pub const IXON: u32 = 0o002000;

// c_oflag bits
pub const OPOST: u32 = 0o000001;
pub const ONLCR: u32 = 0o000004;

// c_lflag bits
pub const ISIG: u32 = 0o000001;
pub const ICANON: u32 = 0o000002;
pub const ECHO: u32 = 0o000010;
pub const ECHOE: u32 = 0o000020;
pub const ECHOK: u32 = 0o000040;
pub const IEXTEN: u32 = 0o100000;

// c_cc indices (Linux x86-64)
pub const VINTR: usize = 0;
pub const VQUIT: usize = 1;
pub const VERASE: usize = 2;
pub const VKILL: usize = 3;
pub const VEOF: usize = 4;
pub const VTIME: usize = 5;
pub const VMIN: usize = 6;
pub const VSUSP: usize = 10;

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
const _SIZE_CHECK_TERMIOS: () = assert!(core::mem::size_of::<Termios>() == 36);

impl Termios {
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

    pub fn is_canonical(&self) -> bool {
        (self.c_lflag & ICANON) != 0
    }

    pub fn is_echo(&self) -> bool {
        (self.c_lflag & ECHO) != 0
    }
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Winsize {
    pub ws_row: u16,
    pub ws_col: u16,
    pub ws_xpixel: u16,
    pub ws_ypixel: u16,
}

impl Winsize {
    pub const fn new(rows: u16, cols: u16) -> Self {
        Self {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }
    }
}

// ---------------------------------------------------------------------
// PtyInner
// ---------------------------------------------------------------------

/// Soft cap on queued bytes (each direction). Bounds a runaway producer
/// from holding the kernel heap hostage.
const MAX_QUEUED_BYTES: usize = 64 * 1024;

pub struct PtyInner {
    /// Bytes ready for the slave's `read(0)`.
    slave_queue: VecDeque<u8>,

    /// Bytes the slave wrote to `1`/`2`, awaiting Vte parsing by the
    /// host's renderer. The compositor drains this through
    /// `PtyMaster::drain_output` once per frame.
    master_queue: VecDeque<u8>,

    pub termios: Termios,
    pub winsize: Winsize,

    /// `WindowId` of the hosting terminal window. Held for the
    /// SIGWINCH-on-resize path and for waker keys.
    terminal_id: WindowId,
}

impl PtyInner {
    fn new(terminal_id: WindowId, rows: u16, cols: u16) -> Self {
        Self {
            slave_queue: VecDeque::new(),
            master_queue: VecDeque::new(),
            termios: Termios::default_tty(),
            winsize: Winsize::new(rows, cols),
            terminal_id,
        }
    }

    /// Push raw bytes onto the slave's input queue. The caller is
    /// responsible for any line-discipline transformation (today's
    /// TerminalWindow does its own ICANON / ECHO handling). Returns
    /// the number of bytes accepted; remainder dropped on overflow.
    pub fn push_slave_input(&mut self, bytes: &[u8]) -> usize {
        let room = MAX_QUEUED_BYTES.saturating_sub(self.slave_queue.len());
        let take = core::cmp::min(room, bytes.len());
        self.slave_queue.extend(bytes[..take].iter().copied());
        take
    }

    /// Drain bytes from the slave's input queue into `dst`. Returns
    /// the number copied.
    pub fn slave_read(&mut self, dst: &mut [u8]) -> usize {
        let n = core::cmp::min(dst.len(), self.slave_queue.len());
        for slot in dst.iter_mut().take(n) {
            *slot = self.slave_queue.pop_front().unwrap();
        }
        n
    }

    /// Number of bytes ready for the slave to read.
    pub fn slave_readable(&self) -> usize {
        self.slave_queue.len()
    }

    /// Slave called `write` — append bytes to the master's output
    /// queue. Applies output post-processing per termios:
    /// `OPOST | ONLCR` translates each bare `\n` into `\r\n` (mirrors
    /// the Linux TTY layer — without this, programs that emit `\n`
    /// to end a line render as a staircase because LF alone moves
    /// down a row but doesn't return the column to zero).
    ///
    /// Returns the number of *input* bytes consumed (not the number
    /// of bytes produced — expansion to `\r\n` doesn't change the
    /// caller's accounting).
    pub fn slave_write(&mut self, bytes: &[u8]) -> usize {
        let opost = (self.termios.c_oflag & OPOST) != 0;
        let onlcr = opost && (self.termios.c_oflag & ONLCR) != 0;
        let mut consumed = 0;
        for &b in bytes {
            if self.master_queue.len() >= MAX_QUEUED_BYTES {
                break;
            }
            if onlcr && b == b'\n' {
                // Expand to CR + LF. Honor the cap mid-expansion: if
                // only room for the CR, fit it in; the next call may
                // get the LF (the slave will retry the write).
                self.master_queue.push_back(b'\r');
                if self.master_queue.len() < MAX_QUEUED_BYTES {
                    self.master_queue.push_back(b'\n');
                    consumed += 1;
                } else {
                    // Couldn't fit the LF — stop short of consuming
                    // this input byte. POSIX-friendly partial-write.
                    break;
                }
            } else {
                self.master_queue.push_back(b);
                consumed += 1;
            }
        }
        consumed
    }

    /// Drain all bytes the slave wrote — the compositor calls this
    /// once per frame and feeds the result into the Vte parser. Returns
    /// an empty `Vec` when there's nothing to drain.
    pub fn drain_master_output(&mut self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.master_queue.len());
        while let Some(b) = self.master_queue.pop_front() {
            out.push(b);
        }
        out
    }
}

// ---------------------------------------------------------------------
// Master / Slave handles
// ---------------------------------------------------------------------

/// The master end. Held by the host (TerminalWindow). All operations
/// require briefly locking the inner mutex.
#[derive(Clone)]
pub struct PtyMaster {
    inner: Arc<Mutex<PtyInner>>,
}

impl PtyMaster {
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn terminal_id(&self) -> WindowId {
        self.inner.lock().terminal_id
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut PtyInner) -> R) -> R {
        f(&mut self.inner.lock())
    }

    /// Push input bytes to the slave's read queue. Returns true if any
    /// bytes were enqueued (the caller wakes blocked readers).
    pub fn push_input(&self, bytes: &[u8]) -> bool {
        let pushed = self.with(|p| p.push_slave_input(bytes));
        pushed > 0
    }

    /// Drain pending slave→master output. Cheap when empty.
    pub fn drain_output(&self) -> Vec<u8> {
        self.with(|p| p.drain_master_output())
    }

    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn termios(&self) -> Termios {
        self.with(|p| p.termios)
    }

    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn set_termios(&self, t: Termios) {
        self.with(|p| p.termios = t);
    }

    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn winsize(&self) -> Winsize {
        self.with(|p| p.winsize)
    }

    /// Update the winsize if it changed. Returns true on actual change
    /// so the caller can raise SIGWINCH on the foreground process.
    pub fn set_winsize(&self, ws: Winsize) -> bool {
        self.with(|p| {
            if p.winsize == ws {
                false
            } else {
                p.winsize = ws;
                true
            }
        })
    }
}

/// The slave end. Held by ring-3 processes through their fd 0/1/2.
/// Since standard streams are represented as `FdSlot::Stdin/Stdout/Stderr`
/// sentinels and not as full Arc<Slave> handles, slave lookup goes
/// through `pty_for_terminal` keyed by `Process.terminal_id` — the
/// same model that `userland::stdin` used.
#[derive(Clone)]
pub struct PtySlave {
    inner: Arc<Mutex<PtyInner>>,
}

impl PtySlave {
    pub fn with<R>(&self, f: impl FnOnce(&mut PtyInner) -> R) -> R {
        f(&mut self.inner.lock())
    }

    pub fn read(&self, dst: &mut [u8]) -> usize {
        self.with(|p| p.slave_read(dst))
    }

    pub fn readable(&self) -> usize {
        self.with(|p| p.slave_readable())
    }

    pub fn write(&self, src: &[u8]) -> usize {
        self.with(|p| p.slave_write(src))
    }

    pub fn termios(&self) -> Termios {
        self.with(|p| p.termios)
    }

    pub fn set_termios(&self, t: Termios) {
        self.with(|p| p.termios = t);
    }

    pub fn winsize(&self) -> Winsize {
        self.with(|p| p.winsize)
    }
}

// ---------------------------------------------------------------------
// Registry — per-WindowId pty
// ---------------------------------------------------------------------

static REGISTRY: Mutex<BTreeMap<Option<WindowId>, Arc<Mutex<PtyInner>>>> =
    Mutex::new(BTreeMap::new());

/// Allocate a pty for `terminal_id` with the given grid size. Returns
/// the master end. Idempotent: re-registering returns the existing
/// pty.
pub fn install_for_terminal(terminal_id: WindowId, rows: u16, cols: u16) -> PtyMaster {
    let mut reg = REGISTRY.lock();
    let inner = reg
        .entry(Some(terminal_id))
        .or_insert_with(|| Arc::new(Mutex::new(PtyInner::new(terminal_id, rows, cols))));
    PtyMaster {
        inner: inner.clone(),
    }
}

/// Tear down the pty for `terminal_id`. Slaves with cached handles
/// continue to operate on a now-orphaned inner; they'll simply have
/// nothing waking them and their reads will return 0.
pub fn clear_for_terminal(terminal_id: WindowId) {
    REGISTRY.lock().remove(&Some(terminal_id));
}

/// True iff a pty exists for `terminal_id`.
pub fn is_active_for_terminal(terminal_id: WindowId) -> bool {
    REGISTRY.lock().contains_key(&Some(terminal_id))
}

/// Look up the master for a known terminal. Returns `None` when no pty
/// is registered for that id.
pub fn master_for_terminal(terminal_id: WindowId) -> Option<PtyMaster> {
    REGISTRY
        .lock()
        .get(&Some(terminal_id))
        .map(|inner| PtyMaster {
            inner: inner.clone(),
        })
}

/// Look up the slave handle for a terminal id. Same Arc as the master.
pub fn slave_for_terminal(terminal_id: WindowId) -> Option<PtySlave> {
    REGISTRY
        .lock()
        .get(&Some(terminal_id))
        .map(|inner| PtySlave {
            inner: inner.clone(),
        })
}

/// Install the `None`-keyed legacy queue, used by tests and by ring-3
/// processes that don't have a terminal yet. The defaults are 80×24.
pub fn install_legacy() {
    REGISTRY.lock().entry(None).or_insert_with(|| {
        Arc::new(Mutex::new(PtyInner::new(
            // The None-keyed pty isn't tied to any window — store a
            // placeholder id (which the keyboard waker path won't
            // ever fire on, since input only flows through a real
            // terminal).
            crate::window::WindowId(0),
            config::DEFAULT_ROWS,
            config::DEFAULT_COLS,
        )))
    });
}

pub fn clear_legacy() {
    REGISTRY.lock().remove(&None);
}

pub fn is_active_legacy() -> bool {
    REGISTRY.lock().contains_key(&None)
}

/// Slave handle for the legacy `None`-keyed pty. Used by tests.
pub fn legacy_slave() -> Option<PtySlave> {
    REGISTRY.lock().get(&None).map(|inner| PtySlave {
        inner: inner.clone(),
    })
}

/// Slave handle for the legacy `None`-keyed pty. Used by tests.
pub fn legacy_master() -> Option<PtyMaster> {
    REGISTRY.lock().get(&None).map(|inner| PtyMaster {
        inner: inner.clone(),
    })
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(feature = "test")]
pub fn get_tests() -> &'static [&'static dyn crate::lib::test_utils::Testable] {
    &[
        &tests::test_termios_size_matches_linux,
        &tests::test_termios_default_canonical_echo,
        &tests::test_master_slave_share_state,
        &tests::test_push_input_drains_via_slave_read,
        &tests::test_slave_write_drains_via_master,
        &tests::test_set_winsize_returns_change_flag,
        &tests::test_registry_install_and_clear,
        &tests::test_queue_caps_at_max,
        &tests::test_per_pty_termios_independent,
        &tests::test_slave_write_translates_lf_to_crlf_under_onlcr,
        &tests::test_slave_write_passes_through_when_opost_off,
        &tests::test_slave_write_preserves_existing_cr,
    ]
}

#[cfg(feature = "test")]
mod tests {
    use super::*;

    fn fresh_master() -> PtyMaster {
        let id = WindowId::new();
        install_for_terminal(id, 24, 80)
    }

    pub(super) fn test_termios_size_matches_linux() {
        assert_eq!(core::mem::size_of::<Termios>(), 36);
    }

    pub(super) fn test_termios_default_canonical_echo() {
        let t = Termios::default_tty();
        assert!(t.is_canonical());
        assert!(t.is_echo());
        assert_eq!(t.c_cc[VINTR], 0x03);
        assert_eq!(t.c_cc[VEOF], 0x04);
    }

    pub(super) fn test_master_slave_share_state() {
        let id = WindowId::new();
        let m = install_for_terminal(id, 24, 80);
        let s = slave_for_terminal(id).unwrap();
        let mut t = m.termios();
        t.c_lflag &= !ICANON;
        m.set_termios(t);
        // Slave sees master's change because they share PtyInner.
        assert!(!s.termios().is_canonical());
        clear_for_terminal(id);
    }

    pub(super) fn test_push_input_drains_via_slave_read() {
        let m = fresh_master();
        let id = m.terminal_id();
        let s = slave_for_terminal(id).unwrap();
        assert!(m.push_input(b"hello"));
        let mut buf = [0u8; 16];
        let n = s.read(&mut buf);
        assert_eq!(n, 5);
        assert_eq!(&buf[..5], b"hello");
        assert_eq!(s.read(&mut buf), 0);
        clear_for_terminal(id);
    }

    pub(super) fn test_slave_write_drains_via_master() {
        let m = fresh_master();
        let id = m.terminal_id();
        let s = slave_for_terminal(id).unwrap();
        s.write(b"world");
        let drained = m.drain_output();
        assert_eq!(&drained[..], b"world");
        // Second drain is empty.
        assert!(m.drain_output().is_empty());
        clear_for_terminal(id);
    }

    pub(super) fn test_set_winsize_returns_change_flag() {
        let m = fresh_master();
        let id = m.terminal_id();
        let ws_same = m.winsize();
        assert!(!m.set_winsize(ws_same));
        let new_ws = Winsize::new(40, 100);
        assert!(m.set_winsize(new_ws));
        assert_eq!(m.winsize().ws_row, 40);
        clear_for_terminal(id);
    }

    pub(super) fn test_registry_install_and_clear() {
        let id = WindowId::new();
        assert!(!is_active_for_terminal(id));
        install_for_terminal(id, 24, 80);
        assert!(is_active_for_terminal(id));
        clear_for_terminal(id);
        assert!(!is_active_for_terminal(id));
    }

    pub(super) fn test_queue_caps_at_max() {
        let m = fresh_master();
        let id = m.terminal_id();
        let s = slave_for_terminal(id).unwrap();
        // Push 2x the cap; only MAX_QUEUED_BYTES should land.
        let blob = alloc::vec![b'x'; MAX_QUEUED_BYTES * 2];
        m.push_input(&blob);
        // Drain in chunks to count.
        let mut total = 0usize;
        let mut buf = [0u8; 4096];
        loop {
            let n = s.read(&mut buf);
            if n == 0 {
                break;
            }
            total += n;
        }
        assert_eq!(total, MAX_QUEUED_BYTES);
        clear_for_terminal(id);
    }

    pub(super) fn test_slave_write_translates_lf_to_crlf_under_onlcr() {
        // Default termios has both OPOST and ONLCR. A bare `\n` from
        // the slave must appear as `\r\n` on the master side, otherwise
        // BusyBox / zsh output staircases.
        let m = fresh_master();
        let id = m.terminal_id();
        let s = slave_for_terminal(id).unwrap();
        s.write(b"hi\nbye\n");
        let drained = m.drain_output();
        assert_eq!(&drained[..], b"hi\r\nbye\r\n");
        clear_for_terminal(id);
    }

    pub(super) fn test_slave_write_passes_through_when_opost_off() {
        let m = fresh_master();
        let id = m.terminal_id();
        let mut t = m.termios();
        t.c_oflag &= !OPOST;
        m.set_termios(t);
        let s = slave_for_terminal(id).unwrap();
        s.write(b"hi\nbye\n");
        let drained = m.drain_output();
        assert_eq!(&drained[..], b"hi\nbye\n");
        clear_for_terminal(id);
    }

    pub(super) fn test_slave_write_preserves_existing_cr() {
        // A `\r\n` already in the stream should still emit `\r\n` —
        // ONLCR translates `\n` to `\r\n`, but a preceding `\r` is just
        // additional output we don't suppress (matches Linux TTY).
        let m = fresh_master();
        let id = m.terminal_id();
        let s = slave_for_terminal(id).unwrap();
        s.write(b"hi\r\n");
        let drained = m.drain_output();
        assert_eq!(&drained[..], b"hi\r\r\n");
        clear_for_terminal(id);
    }

    pub(super) fn test_per_pty_termios_independent() {
        let m1 = fresh_master();
        let m2 = fresh_master();
        let id1 = m1.terminal_id();
        let id2 = m2.terminal_id();
        let mut t = m1.termios();
        t.c_lflag &= !ICANON;
        m1.set_termios(t);
        assert!(!m1.termios().is_canonical());
        // m2 is untouched.
        assert!(m2.termios().is_canonical());
        clear_for_terminal(id1);
        clear_for_terminal(id2);
    }
}

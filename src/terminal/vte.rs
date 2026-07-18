//! ANSI / VT100 / xterm escape-sequence parser.
//!
//! Implements Paul Williams' DEC state machine
//! (<https://vt100.net/emu/dec_ansi_parser>) — the same machine used by
//! xterm, libvte, and Alacritty. Bytes are fed in via [`Vte::advance`]
//! and dispatched through a [`Perform`] callback.
//!
//! Scope: enough of the VT100 + xterm subset for vi/vim, less, htop,
//! agnoster-themed zsh. C0 controls in GROUND, CSI dispatch (with up to
//! 32 numeric params, two intermediates, and a "private" prefix like
//! `?`), OSC strings (BEL- or ST-terminated), and ESC dispatch. UTF-8
//! reassembly happens inside the parser so [`Perform::print`] always
//! receives a complete `char`.
//!
//! What we deliberately skip: DCS strings (consumed and dropped — we
//! don't use device control), sub-parameters with `:` (collapsed onto
//! `;`), and 8-bit C1 controls (we always go through ESC).

use alloc::vec::Vec;

/// Maximum number of numeric parameters accepted in a single CSI
/// sequence. xterm permits 16; SGR sequences with truecolor occasionally
/// reach 11 (`38;2;r;g;b` plus reset etc.), so 32 leaves headroom.
pub const MAX_PARAMS: usize = 32;

/// Maximum number of intermediate bytes (0x20..0x2F) in a single CSI
/// sequence. The VT500-series spec allows two; DECSCUSR's space-q is
/// the only one we actually parse today.
pub const MAX_INTERMEDIATES: usize = 2;

/// Hard cap on OSC string length. xterm uses 8192; titles longer than
/// that are extremely rare and would just be truncated by any terminal.
const MAX_OSC_BYTES: usize = 8192;

/// Replacement codepoint emitted when invalid UTF-8 is encountered.
const REPLACEMENT_CHAR: char = '\u{FFFD}';

/// Callback receiving parsed events. All methods have empty default
/// bodies so consumers only override the ones they care about.
#[allow(unused_variables)]
pub trait Perform {
    /// A printable character (after UTF-8 reassembly).
    fn print(&mut self, c: char) {}

    /// A C0 control byte (BS, HT, LF, CR, BEL, …) executed in GROUND.
    fn execute(&mut self, byte: u8) {}

    /// A CSI sequence terminated by `final_byte` (0x40..0x7E).
    /// `intermediates` is up to two bytes from the 0x20..0x2F range.
    /// `ignore` is true if the sequence overflowed our parameter or
    /// intermediate buffers (the spec says to silently absorb such
    /// sequences).
    fn csi_dispatch(&mut self, params: &[u16], intermediates: &[u8], ignore: bool, final_byte: u8) {
    }

    /// An OSC sequence — the entire payload between `ESC ]` and the
    /// terminator. `bell_terminated` distinguishes BEL termination from
    /// ST (`ESC \`); most consumers don't care.
    fn osc_dispatch(&mut self, data: &[u8], bell_terminated: bool) {}

    /// A non-CSI escape sequence — `ESC` followed by optional
    /// intermediates and a final byte 0x30..0x7E.
    fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {}
}

/// Parser states, named after the DEC state diagram. `SosPmApcString`
/// and `DcsPassthrough` are absorbed and discarded — we don't dispatch
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    EscapeIntermediate,
    CsiEntry,
    CsiParam,
    CsiIntermediate,
    CsiIgnore,
    OscString,
    DcsEntry,
    #[expect(dead_code, reason = "intentional kernel API surface")]
    DcsParam,
    #[expect(dead_code, reason = "intentional kernel API surface")]
    DcsIntermediate,
    #[expect(dead_code, reason = "intentional kernel API surface")]
    DcsPassthrough,
    #[expect(dead_code, reason = "intentional kernel API surface")]
    DcsIgnore,
    SosPmApcString,
}

/// The parser. Persistent across calls to [`Vte::advance`]; one instance
/// per terminal.
pub struct Vte {
    state: State,

    // CSI / DCS parameter accumulator. `current_param` is `None` when no
    // digit has yet been seen for the next slot — distinguishes
    // "explicit zero" from "omitted" only via convention (we collapse
    // both to 0; consumers default to 1 where it matters).
    params: [u16; MAX_PARAMS],
    num_params: usize,
    current_param: Option<u16>,

    // Intermediates 0x20..0x2F.
    intermediates: [u8; MAX_INTERMEDIATES],
    num_intermediates: usize,

    // Private-marker prefix byte (one of `?`, `>`, `<`, `=`) at start of
    // CSI params. Stored separately so a DEC private sequence like
    // `CSI ? 25 h` is dispatched with intermediates = [], params = [25],
    // private = Some(b'?'), final = b'h'.
    private_marker: Option<u8>,

    // Whether the sequence has overflowed any buffer. CSI_IGNORE absorbs
    // until the final byte and dispatches with `ignore = true`.
    ignore: bool,

    // UTF-8 reassembly buffer.
    utf8_buf: [u8; 4],
    utf8_len: usize,
    utf8_expected: usize,

    // OSC payload accumulator.
    osc_buf: Vec<u8>,
    osc_overflow: bool,
}

impl Vte {
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            params: [0; MAX_PARAMS],
            num_params: 0,
            current_param: None,
            intermediates: [0; MAX_INTERMEDIATES],
            num_intermediates: 0,
            private_marker: None,
            ignore: false,
            utf8_buf: [0; 4],
            utf8_len: 0,
            utf8_expected: 0,
            osc_buf: Vec::new(),
            osc_overflow: false,
        }
    }

    /// Reset to GROUND, clearing all in-flight state. Use after a
    /// parser-corrupting external event (we don't expect any today; the
    /// state machine self-recovers from any sequence of bytes).

    /// Feed a byte. Dispatches zero or one [`Perform`] callback before
    /// returning.
    pub fn advance<P: Perform>(&mut self, byte: u8, perform: &mut P) {
        // Anywhere transitions from the Williams diagram: certain bytes
        // are valid in any state and short-circuit the per-state logic.
        match byte {
            // CAN / SUB → cancel current sequence, return to GROUND.
            0x18 | 0x1A => {
                if self.state == State::OscString {
                    // OSC payloads accept any byte; CAN/SUB inside OSC
                    // is just data. (xterm differs here but ours is
                    // simpler and safer.)
                    self.osc_push(byte);
                    return;
                }
                self.clear_sequence();
                self.state = State::Ground;
                if byte == 0x1A {
                    perform.execute(byte);
                }
                return;
            }
            // ESC always restarts a sequence, except inside OSC where
            // it may be the first half of an ST terminator (`ESC \`).
            0x1B => {
                if self.state == State::OscString {
                    // OSC's terminator handling lives in the OscString
                    // branch below — but we need to peek ahead one byte
                    // for `\`. The Williams approach: leave OSC, enter
                    // an intermediate "OSC ESC" state. We collapse that
                    // into a flag.
                    self.state = State::Escape; // placeholder; will be
                                                // overridden by the next byte
                    self.osc_end(perform, /*bell_terminated=*/ false);
                    self.clear_sequence();
                    return;
                }
                self.clear_sequence();
                self.state = State::Escape;
                return;
            }
            _ => {}
        }

        match self.state {
            State::Ground => self.ground(byte, perform),
            State::Escape => self.escape(byte, perform),
            State::EscapeIntermediate => self.escape_intermediate(byte, perform),
            State::CsiEntry => self.csi_entry(byte, perform),
            State::CsiParam => self.csi_param(byte, perform),
            State::CsiIntermediate => self.csi_intermediate(byte, perform),
            State::CsiIgnore => self.csi_ignore(byte, perform),
            State::OscString => self.osc_string(byte, perform),
            State::DcsEntry
            | State::DcsParam
            | State::DcsIntermediate
            | State::DcsPassthrough
            | State::DcsIgnore => self.dcs(byte),
            State::SosPmApcString => self.sos_pm_apc(byte),
        }
    }

    // ---- per-state handlers ----

    fn ground<P: Perform>(&mut self, byte: u8, perform: &mut P) {
        match byte {
            // C0 controls.
            0x00..=0x06 | 0x0E..=0x17 | 0x19 | 0x1C..=0x1F => perform.execute(byte),
            0x07..=0x0D => perform.execute(byte),
            // Printable ASCII fast path.
            0x20..=0x7E => {
                self.utf8_len = 0;
                self.utf8_expected = 0;
                perform.print(byte as char);
            }
            // DEL — ignored.
            0x7F => {}
            // UTF-8 continuation or multi-byte start.
            _ => self.utf8_byte(byte, perform),
        }
    }

    fn utf8_byte<P: Perform>(&mut self, byte: u8, perform: &mut P) {
        if self.utf8_len == 0 {
            // Start of a multi-byte sequence.
            self.utf8_expected = if byte & 0xE0 == 0xC0 {
                2
            } else if byte & 0xF0 == 0xE0 {
                3
            } else if byte & 0xF8 == 0xF0 {
                4
            } else {
                // Continuation byte without a leader, or invalid start.
                perform.print(REPLACEMENT_CHAR);
                return;
            };
            self.utf8_buf[0] = byte;
            self.utf8_len = 1;
        } else {
            // Continuation byte.
            if byte & 0xC0 != 0x80 {
                // Invalid: not a continuation.
                self.utf8_len = 0;
                self.utf8_expected = 0;
                perform.print(REPLACEMENT_CHAR);
                // Re-feed the offending byte through GROUND so a stray
                // 0x80..=0xFF after a malformed sequence doesn't break
                // alignment.
                self.ground(byte, perform);
                return;
            }
            self.utf8_buf[self.utf8_len] = byte;
            self.utf8_len += 1;
        }

        if self.utf8_len == self.utf8_expected {
            let result = core::str::from_utf8(&self.utf8_buf[..self.utf8_len])
                .ok()
                .and_then(|s| s.chars().next());
            self.utf8_len = 0;
            self.utf8_expected = 0;
            match result {
                Some(c) => perform.print(c),
                None => perform.print(REPLACEMENT_CHAR),
            }
        }
    }

    fn escape<P: Perform>(&mut self, byte: u8, perform: &mut P) {
        match byte {
            // Intermediate.
            0x20..=0x2F => {
                self.collect_intermediate(byte);
                self.state = State::EscapeIntermediate;
            }
            // CSI.
            b'[' => {
                self.clear_sequence();
                self.state = State::CsiEntry;
            }
            // OSC.
            b']' => {
                self.osc_buf.clear();
                self.osc_overflow = false;
                self.state = State::OscString;
            }
            // DCS.
            b'P' => {
                self.clear_sequence();
                self.state = State::DcsEntry;
            }
            // SOS / PM / APC — absorb until ST.
            b'X' | b'^' | b'_' => {
                self.state = State::SosPmApcString;
            }
            // ESC final byte.
            0x30..=0x4F | 0x51..=0x57 | 0x59 | 0x5A | 0x5C | 0x60..=0x7E => {
                let intermediates = &self.intermediates[..self.num_intermediates];
                perform.esc_dispatch(intermediates, self.ignore, byte);
                self.clear_sequence();
                self.state = State::Ground;
            }
            // C0 controls execute in-place.
            0x00..=0x17 | 0x19 | 0x1C..=0x1F => perform.execute(byte),
            _ => {
                // Unknown — back to GROUND.
                self.clear_sequence();
                self.state = State::Ground;
            }
        }
    }

    fn escape_intermediate<P: Perform>(&mut self, byte: u8, perform: &mut P) {
        match byte {
            0x20..=0x2F => self.collect_intermediate(byte),
            0x30..=0x7E => {
                let intermediates = &self.intermediates[..self.num_intermediates];
                perform.esc_dispatch(intermediates, self.ignore, byte);
                self.clear_sequence();
                self.state = State::Ground;
            }
            0x00..=0x17 | 0x19 | 0x1C..=0x1F => perform.execute(byte),
            _ => {
                self.clear_sequence();
                self.state = State::Ground;
            }
        }
    }

    fn csi_entry<P: Perform>(&mut self, byte: u8, perform: &mut P) {
        match byte {
            // Private-marker prefix (one byte: ?, >, <, =).
            0x3C..=0x3F => {
                self.private_marker = Some(byte);
                self.state = State::CsiParam;
            }
            0x30..=0x39 => {
                // Digit.
                self.csi_param(byte, perform);
                self.state = State::CsiParam;
            }
            b';' | b':' => {
                // Empty parameter, then separator.
                self.push_param(0);
                self.state = State::CsiParam;
            }
            0x20..=0x2F => {
                self.collect_intermediate(byte);
                self.state = State::CsiIntermediate;
            }
            0x40..=0x7E => {
                // Final byte without params.
                self.dispatch_csi(perform, byte);
            }
            0x00..=0x17 | 0x19 | 0x1C..=0x1F => perform.execute(byte),
            _ => self.state = State::CsiIgnore,
        }
    }

    fn csi_param<P: Perform>(&mut self, byte: u8, perform: &mut P) {
        match byte {
            0x30..=0x39 => {
                // Accumulate digit into current param.
                let digit = (byte - b'0') as u16;
                let new_val = self
                    .current_param
                    .unwrap_or(0)
                    .saturating_mul(10)
                    .saturating_add(digit);
                self.current_param = Some(new_val);
            }
            b';' | b':' => {
                // End current param, start next slot.
                let p = self.current_param.take().unwrap_or(0);
                self.push_param(p);
            }
            0x20..=0x2F => {
                // Flush pending param then move to intermediate state.
                if let Some(p) = self.current_param.take() {
                    self.push_param(p);
                }
                self.collect_intermediate(byte);
                self.state = State::CsiIntermediate;
            }
            0x40..=0x7E => {
                // Final.
                if let Some(p) = self.current_param.take() {
                    self.push_param(p);
                }
                self.dispatch_csi(perform, byte);
            }
            0x00..=0x17 | 0x19 | 0x1C..=0x1F => perform.execute(byte),
            _ => self.state = State::CsiIgnore,
        }
    }

    fn csi_intermediate<P: Perform>(&mut self, byte: u8, perform: &mut P) {
        match byte {
            0x20..=0x2F => self.collect_intermediate(byte),
            0x40..=0x7E => self.dispatch_csi(perform, byte),
            0x00..=0x17 | 0x19 | 0x1C..=0x1F => perform.execute(byte),
            0x30..=0x3F => {
                // Per the spec, parameter / private bytes after
                // intermediates → ignore the rest of the sequence.
                self.state = State::CsiIgnore;
            }
            _ => self.state = State::CsiIgnore,
        }
    }

    fn csi_ignore<P: Perform>(&mut self, byte: u8, _perform: &mut P) {
        if (0x40..=0x7E).contains(&byte) {
            // Final byte — absorb without dispatch.
            self.clear_sequence();
            self.state = State::Ground;
        }
        // Else stay in CsiIgnore.
    }

    fn osc_string<P: Perform>(&mut self, byte: u8, perform: &mut P) {
        match byte {
            0x07 => {
                // BEL terminator.
                self.osc_end(perform, true);
                self.state = State::Ground;
            }
            _ => self.osc_push(byte),
        }
    }

    fn osc_push(&mut self, byte: u8) {
        if self.osc_buf.len() < MAX_OSC_BYTES {
            self.osc_buf.push(byte);
        } else {
            self.osc_overflow = true;
        }
    }

    fn osc_end<P: Perform>(&mut self, perform: &mut P, bell_terminated: bool) {
        if !self.osc_overflow {
            perform.osc_dispatch(&self.osc_buf, bell_terminated);
        }
        self.osc_buf.clear();
        self.osc_overflow = false;
    }

    fn dcs(&mut self, _byte: u8) {
        // DCS not dispatched; absorb until ST (`ESC \`). The
        // anywhere-transition on ESC handles termination — when ESC
        // arrives we'll fall through to Escape state and immediately
        // dispatch the trailing `\` as an ESC final. The `\` dispatch
        // is harmless; consumers won't have anything mapped to it.
        //
        // Bytes inside DCS are just absorbed; nothing else to do.
    }

    fn sos_pm_apc(&mut self, _byte: u8) {
        // Same handling as DCS — absorb until ESC.
    }

    // ---- helpers ----

    fn collect_intermediate(&mut self, byte: u8) {
        if self.num_intermediates < MAX_INTERMEDIATES {
            self.intermediates[self.num_intermediates] = byte;
            self.num_intermediates += 1;
        } else {
            self.ignore = true;
        }
    }

    fn push_param(&mut self, p: u16) {
        if self.num_params < MAX_PARAMS {
            self.params[self.num_params] = p;
            self.num_params += 1;
        } else {
            self.ignore = true;
        }
    }

    fn dispatch_csi<P: Perform>(&mut self, perform: &mut P, final_byte: u8) {
        // If there's a private marker, encode it as a "virtual"
        // intermediate at slot 0 so consumers see it. We always reserve
        // the first intermediate slot for the private marker, with the
        // declared intermediates following. This keeps the dispatch
        // signature simple while exposing the marker.
        //
        // To avoid surprising callers that pass real intermediates with
        // no private marker, we only prepend when `private_marker` is
        // set, and then the marker lives in `intermediates[0]`.
        let params = &self.params[..self.num_params];
        if let Some(marker) = self.private_marker {
            // Build a temporary intermediates array including marker.
            let mut tmp = [0u8; MAX_INTERMEDIATES + 1];
            tmp[0] = marker;
            for i in 0..self.num_intermediates {
                tmp[i + 1] = self.intermediates[i];
            }
            let slice = &tmp[..self.num_intermediates + 1];
            perform.csi_dispatch(params, slice, self.ignore, final_byte);
        } else {
            let intermediates = &self.intermediates[..self.num_intermediates];
            perform.csi_dispatch(params, intermediates, self.ignore, final_byte);
        }
        self.clear_sequence();
        self.state = State::Ground;
    }

    fn clear_sequence(&mut self) {
        self.num_params = 0;
        self.current_param = None;
        self.num_intermediates = 0;
        self.private_marker = None;
        self.ignore = false;
    }
}

impl Default for Vte {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(feature = "test")]
pub fn get_tests() -> &'static [&'static dyn crate::lib::test_utils::Testable] {
    &[
        &tests::test_print_ascii,
        &tests::test_execute_c0,
        &tests::test_csi_cursor_up_no_params,
        &tests::test_csi_cursor_to_row_col,
        &tests::test_csi_sgr_multi_params,
        &tests::test_csi_private_dec_mode_set,
        &tests::test_csi_with_intermediate_space_q,
        &tests::test_csi_overflow_ignored,
        &tests::test_csi_can_aborts,
        &tests::test_esc_dispatch,
        &tests::test_osc_bel_terminated,
        &tests::test_osc_st_terminated,
        &tests::test_utf8_multibyte_print,
        &tests::test_utf8_invalid_emits_replacement,
        &tests::test_csi_empty_param_is_zero,
        &tests::test_csi_recovers_after_garbage,
    ]
}

#[cfg(feature = "test")]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    /// Records every dispatched event for later inspection.
    #[derive(Default)]
    struct Recorder {
        events: Vec<Event>,
    }

    #[derive(Debug, PartialEq, Eq)]
    enum Event {
        Print(char),
        Execute(u8),
        Csi {
            params: Vec<u16>,
            intermediates: Vec<u8>,
            ignore: bool,
            final_byte: u8,
        },
        Osc {
            data: Vec<u8>,
            bel: bool,
        },
        Esc {
            intermediates: Vec<u8>,
            ignore: bool,
            byte: u8,
        },
    }

    impl Perform for Recorder {
        fn print(&mut self, c: char) {
            self.events.push(Event::Print(c));
        }
        fn execute(&mut self, byte: u8) {
            self.events.push(Event::Execute(byte));
        }
        fn csi_dispatch(
            &mut self,
            params: &[u16],
            intermediates: &[u8],
            ignore: bool,
            final_byte: u8,
        ) {
            self.events.push(Event::Csi {
                params: params.to_vec(),
                intermediates: intermediates.to_vec(),
                ignore,
                final_byte,
            });
        }
        fn osc_dispatch(&mut self, data: &[u8], bell_terminated: bool) {
            self.events.push(Event::Osc {
                data: data.to_vec(),
                bel: bell_terminated,
            });
        }
        fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {
            self.events.push(Event::Esc {
                intermediates: intermediates.to_vec(),
                ignore,
                byte,
            });
        }
    }

    fn run(bytes: &[u8]) -> Vec<Event> {
        let mut vte = Vte::new();
        let mut r = Recorder::default();
        for &b in bytes {
            vte.advance(b, &mut r);
        }
        r.events
    }

    pub(super) fn test_print_ascii() {
        let evts = run(b"hi");
        assert_eq!(evts, alloc::vec![Event::Print('h'), Event::Print('i')]);
    }

    pub(super) fn test_execute_c0() {
        let evts = run(b"a\nb");
        assert_eq!(
            evts,
            alloc::vec![Event::Print('a'), Event::Execute(b'\n'), Event::Print('b')]
        );
    }

    pub(super) fn test_csi_cursor_up_no_params() {
        // ESC [ A — cursor up, no params.
        let evts = run(b"\x1b[A");
        assert_eq!(
            evts,
            alloc::vec![Event::Csi {
                params: alloc::vec![],
                intermediates: alloc::vec![],
                ignore: false,
                final_byte: b'A',
            }]
        );
    }

    pub(super) fn test_csi_cursor_to_row_col() {
        // ESC [ 5 ; 12 H
        let evts = run(b"\x1b[5;12H");
        assert_eq!(
            evts,
            alloc::vec![Event::Csi {
                params: alloc::vec![5, 12],
                intermediates: alloc::vec![],
                ignore: false,
                final_byte: b'H',
            }]
        );
    }

    pub(super) fn test_csi_sgr_multi_params() {
        // SGR 38;2;255;100;50 — truecolor foreground.
        let evts = run(b"\x1b[38;2;255;100;50m");
        assert_eq!(
            evts,
            alloc::vec![Event::Csi {
                params: alloc::vec![38, 2, 255, 100, 50],
                intermediates: alloc::vec![],
                ignore: false,
                final_byte: b'm',
            }]
        );
    }

    pub(super) fn test_csi_private_dec_mode_set() {
        // ESC [ ? 25 h — show cursor.
        let evts = run(b"\x1b[?25h");
        assert_eq!(
            evts,
            alloc::vec![Event::Csi {
                params: alloc::vec![25],
                intermediates: alloc::vec![b'?'],
                ignore: false,
                final_byte: b'h',
            }]
        );
    }

    pub(super) fn test_csi_with_intermediate_space_q() {
        // DECSCUSR: ESC [ 2 SP q — block cursor.
        let evts = run(b"\x1b[2 q");
        assert_eq!(
            evts,
            alloc::vec![Event::Csi {
                params: alloc::vec![2],
                intermediates: alloc::vec![b' '],
                ignore: false,
                final_byte: b'q',
            }]
        );
    }

    pub(super) fn test_csi_overflow_ignored() {
        // MAX_PARAMS = 32. Build 33 params; final should dispatch with
        // ignore = true.
        let mut bytes = alloc::vec![0x1b, b'['];
        for i in 0..33 {
            if i > 0 {
                bytes.push(b';');
            }
            bytes.push(b'1');
        }
        bytes.push(b'm');
        let evts = run(&bytes);
        assert_eq!(evts.len(), 1);
        match &evts[0] {
            Event::Csi {
                ignore, final_byte, ..
            } => {
                assert!(ignore, "expected ignore flag on overflow");
                assert_eq!(*final_byte, b'm');
            }
            other => panic!("expected csi, got {:?}", other),
        }
    }

    pub(super) fn test_csi_can_aborts() {
        // ESC [ 1 ; CAN (0x18) — cancels, no dispatch. Then "X" prints.
        let evts = run(b"\x1b[1;\x18X");
        assert_eq!(evts, alloc::vec![Event::Print('X')]);
    }

    pub(super) fn test_esc_dispatch() {
        // ESC 7 — save cursor.
        let evts = run(b"\x1b7");
        assert_eq!(
            evts,
            alloc::vec![Event::Esc {
                intermediates: alloc::vec![],
                ignore: false,
                byte: b'7',
            }]
        );
    }

    pub(super) fn test_osc_bel_terminated() {
        // OSC 0 ; hello BEL — set window title.
        let evts = run(b"\x1b]0;hello\x07");
        assert_eq!(
            evts,
            alloc::vec![Event::Osc {
                data: b"0;hello".to_vec(),
                bel: true,
            }]
        );
    }

    pub(super) fn test_osc_st_terminated() {
        // OSC 0 ; hi ESC \ — ST terminator. The ST is dispatched as
        // ESC ` \ ` esc_dispatch in our implementation; we accept that.
        let evts = run(b"\x1b]0;hi\x1b\\");
        // First event: OSC payload "0;hi" with bel = false.
        assert!(matches!(
            evts.first(),
            Some(Event::Osc { data, bel: false }) if data == b"0;hi"
        ));
    }

    pub(super) fn test_utf8_multibyte_print() {
        // U+00E9 (é): C3 A9
        let evts = run(&[0xC3, 0xA9]);
        assert_eq!(evts, alloc::vec![Event::Print('é')]);
        // U+E0A0 (Powerline branch glyph): EE 82 A0
        let evts = run(&[0xEE, 0x82, 0xA0]);
        assert_eq!(evts, alloc::vec![Event::Print('\u{E0A0}')]);
    }

    pub(super) fn test_utf8_invalid_emits_replacement() {
        // Continuation byte without a leader.
        let evts = run(&[0x80]);
        assert_eq!(evts, alloc::vec![Event::Print('\u{FFFD}')]);
    }

    pub(super) fn test_csi_empty_param_is_zero() {
        // ESC [ ; 5 H — first param empty = 0.
        let evts = run(b"\x1b[;5H");
        assert_eq!(
            evts,
            alloc::vec![Event::Csi {
                params: alloc::vec![0, 5],
                intermediates: alloc::vec![],
                ignore: false,
                final_byte: b'H',
            }]
        );
    }

    pub(super) fn test_csi_recovers_after_garbage() {
        // Garbage CSI, then valid sequence.
        let evts = run(b"\x1b[1;\x1b[2J");
        // The first ESC inside CSI cancels and restarts; we should see
        // exactly one CSI dispatch for [2J.
        let csi_events: alloc::vec::Vec<&Event> = evts
            .iter()
            .filter(|e| matches!(e, Event::Csi { .. }))
            .collect();
        assert_eq!(csi_events.len(), 1);
        match csi_events[0] {
            Event::Csi {
                params, final_byte, ..
            } => {
                assert_eq!(params.as_slice(), &[2]);
                assert_eq!(*final_byte, b'J');
            }
            _ => unreachable!(),
        }
    }
}

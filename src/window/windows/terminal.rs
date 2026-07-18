//! Terminal window with input handling capabilities

use super::base::WindowBase;
use super::text::TextWindow;
use crate::window::event::KeyCode;
use crate::window::{keyboard::keycode_to_char, Event, EventResult, GraphicsDevice, Rect, Window};
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::mem;

/// Callback type for when input is received
pub type InputCallback = Box<dyn FnMut(String) + Send>;

/// A terminal window that can handle keyboard input. Output (from
/// ring-3 `write(1/2)` and kernel `write_to_terminal_id`) is parsed
/// through `vte` into `screen`, which is the source of truth for
/// what's displayed. `text_window` is then re-synced from `screen`
/// only when the Screen has actually changed (otherwise the per-frame
/// sync forces a permanent dirty flag + full repaint that starves the
/// compositor).
pub struct TerminalWindow {
    /// The underlying text window (renderer).
    text_window: TextWindow,
    /// VT100/xterm escape-sequence parser.
    vte: crate::terminal::vte::Vte,
    /// Character grid + cursor + SGR pen + scrollback + alt-screen.
    screen: crate::terminal::screen::Screen,
    /// Set whenever the Screen is mutated (parsed bytes drained from
    /// the pty, local echoes through `feed_bytes`, scrollback view
    /// changes). Consumed + cleared by `process_terminal_output` after
    /// it syncs the TextWindow.
    screen_dirty: bool,
    /// Current input buffer
    input_buffer: String,
    /// Callback for when Enter is pressed
    input_callback: Option<InputCallback>,
    /// Command history
    history: Vec<String>,
    /// Current position in history
    history_index: usize,
    /// Starting column of current input line
    input_start_col: usize,
    /// Starting row of current input line
    input_start_row: usize,
}

impl TerminalWindow {
    /// Create a new terminal window with a specific ID
    pub fn new_with_id(id: crate::window::WindowId, bounds: Rect) -> Self {
        let mut text_window = TextWindow::new_with_id(id, bounds);

        // Write initial text to show the terminal is ready
        text_window.write_str("AgenticOS Terminal Ready\n");
        text_window.write_str("Initializing...\n\n");

        // Log cursor position and buffer state
        let (col, row) = text_window.cursor_position();
        crate::debug_info!("Initial text written, cursor at ({}, {})", col, row);

        // Force invalidation to ensure initial paint
        text_window.invalidate();

        crate::debug_info!(
            "TerminalWindow created with id={:?}, bounds: {:?}",
            id,
            bounds
        );

        // Initialize the Vte + Screen pair sized to the TextWindow's
        // grid. The Screen is the source of truth for what's displayed;
        // TextWindow is synced from it each prepare_for_render.
        let (rows, cols) = match text_window.grid_size_opt() {
            Some((r, c)) => (r as usize, c as usize),
            None => (
                crate::terminal::config::DEFAULT_ROWS as usize,
                crate::terminal::config::DEFAULT_COLS as usize,
            ),
        };
        let screen = crate::terminal::screen::Screen::new(rows.max(1), cols.max(1));
        let vte = crate::terminal::vte::Vte::new();

        TerminalWindow {
            text_window,
            vte,
            screen,
            // Force one initial sync so the dark-grey terminal
            // background paints before any input arrives.
            screen_dirty: true,
            input_buffer: String::new(),
            input_callback: None,
            history: Vec::new(),
            history_index: 0,
            input_start_col: 0,
            input_start_row: 0,
        }
    }

    /// Set callback for when Enter is pressed

    /// Write output to the terminal. Routes through the Vte parser so
    /// escape sequences in the bytes (colors, cursor moves) take effect.

    /// Write a line to the terminal. Appends `\r\n` so the cursor wraps
    /// to column 0 on the next row regardless of OPOST state.

    /// Push bytes through the parser into the Screen. Used by the local
    /// echo paths (`handle_enter`, `handle_backspace`, key echo in
    /// canonical mode) and by `write`/`write_line` so every visible
    /// change flows through one channel.
    fn feed_bytes(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        for &b in bytes {
            self.vte.advance(b, &mut self.screen);
        }
        self.screen_dirty = true;
    }

    /// Clear the current input line (canonical-mode editor scratchpad).
    fn clear_input_line(&mut self) {
        // Reposition the screen cursor to the input start, then erase
        // to end of line — preserves SGR state on the cells.
        self.feed_bytes(b"\x1b[");
        push_u16_decimal(
            &mut self.vte,
            &mut self.screen,
            (self.input_start_row + 1) as u16,
        );
        self.feed_bytes(b";");
        push_u16_decimal(
            &mut self.vte,
            &mut self.screen,
            (self.input_start_col + 1) as u16,
        );
        self.feed_bytes(b"H\x1b[K");
    }

    /// Replace the current input with new text (history navigation).
    fn replace_input(&mut self, new_input: &str) {
        self.clear_input_line();
        self.input_buffer.clear();
        self.input_buffer.push_str(new_input);
        self.feed_bytes(new_input.as_bytes());
    }

    /// Process any pending console output

    /// Drain pending slave-output through the Vte parser into the
    /// Screen, then push any DSR / device-attribute reply bytes the
    /// Screen produced back into the pty's slave input. Sync the
    /// Screen's visible viewport down to the TextWindow **only when
    /// bytes were parsed or replies were sent** — running the full
    /// `rows × cols` sync every frame regardless of state forces a
    /// permanent dirty flag, full repaint per frame, and on a fast
    /// producer (vi redraws) starves the compositor to the point the
    /// mouse can't move.
    fn process_terminal_output(&mut self) {
        let terminal_id = self.text_window.id();

        let outputs = crate::window::terminal::take_terminal_output(terminal_id);
        if !outputs.is_empty() {
            for s in outputs {
                for byte in s.as_bytes() {
                    self.vte.advance(*byte, &mut self.screen);
                }
            }
            self.screen_dirty = true;
        }

        // Forward DSR / device-attribute replies the parser produced
        // back into the slave's input queue. push_bytes_for_terminal
        // (not the lower-level push_input) so a blocked `read(0)`
        // wakes.
        let replies = self.screen.take_replies();
        if !replies.is_empty() {
            crate::userland::stdin::push_bytes_for_terminal(terminal_id, &replies);
        }

        if !self.screen_dirty {
            return;
        }
        self.sync_text_window_from_screen();
        self.screen_dirty = false;

        let (col, row) = self.text_window.cursor_position();
        self.input_start_col = col;
        self.input_start_row = row;
    }

    /// Copy the Screen's visible viewport into the TextWindow's grid
    /// and move the TextWindow's cursor to match the Screen's caret.
    /// Cell attributes (bold/italic/underline) are not yet rendered —
    /// follow-up after U9 ships.
    fn sync_text_window_from_screen(&mut self) {
        use crate::terminal::colors::resolve;

        let rows = self.screen.rows().min(self.text_window_rows());
        let cols = self.screen.cols().min(self.text_window_cols());

        for row in 0..rows {
            let cells = self.screen.visible_row(row);
            for col in 0..cols {
                let cell = cells[col];
                let fg = resolve(cell.fg, /*is_foreground=*/ true);
                let bg = resolve(cell.bg, /*is_foreground=*/ false);
                self.text_window.set_cell(row, col, cell.ch, fg, bg);
            }
        }

        let caret = self.screen.caret();
        let cx = caret.col.min(cols.saturating_sub(1));
        let cy = caret.row.min(rows.saturating_sub(1));
        self.text_window.set_cursor_position(cx, cy);
    }

    fn text_window_rows(&self) -> usize {
        self.text_window
            .grid_size_opt()
            .map(|(r, _)| r as usize)
            .unwrap_or(0)
    }

    fn text_window_cols(&self) -> usize {
        self.text_window
            .grid_size_opt()
            .map(|(_, c)| c as usize)
            .unwrap_or(0)
    }

    /// Handle a backspace key press
    fn handle_backspace(&mut self) {
        if !self.input_buffer.is_empty() {
            self.input_buffer.pop();
            // Standard echoed-erase sequence: BS, space, BS — moves
            // back one column, erases the glyph, leaves cursor on the
            // erased cell.
            self.feed_bytes(b"\x08 \x08");
        }
    }

    /// Handle enter key press
    fn handle_enter(&mut self) {
        let input = mem::take(&mut self.input_buffer);
        crate::debug_info!(
            "Terminal: Enter pressed, input='{}' (len={})",
            input,
            input.len()
        );
        self.feed_bytes(b"\r\n");

        // Add to history if not empty
        if !input.is_empty() {
            self.history.push(input.clone());
            self.history_index = self.history.len();
        }

        // Call callback if set
        if let Some(ref mut callback) = self.input_callback {
            callback(input.clone());
        }

        // zsh (ring-3) is always the terminal's shell after boot, so
        // every typed line goes into the user stdin queue. Before the
        // userland subsystem is active (e.g., during boot before
        // init_guishell_desktop has spawned zsh), input is silently
        // dropped — the terminal window doesn't actually exist yet in
        // that window so this branch is effectively unreachable in
        // production.
        let tid = self.text_window.id();
        if crate::userland::stdin::is_active_for_terminal(tid) {
            crate::userland::stdin::push_bytes_for_terminal(tid, input.as_bytes());
            crate::userland::stdin::push_bytes_for_terminal(tid, b"\n");
        } else {
            crate::debug_warn!(
                "TerminalWindow::handle_enter: no userland stdin active for terminal {:?}; dropping line",
                tid,
            );
        }

        // Update input start position for next input
        let (col, row) = self.text_window.cursor_position();
        self.input_start_col = col;
        self.input_start_row = row;
    }

    /// Handle up arrow (previous history)
    fn handle_up_arrow(&mut self) {
        if self.history_index > 0 {
            self.history_index -= 1;
            let history_entry = self.history[self.history_index].clone();
            self.replace_input(&history_entry);
        }
    }

    /// Handle down arrow (next history)
    fn handle_down_arrow(&mut self) {
        if self.history_index < self.history.len() {
            self.history_index += 1;
            if self.history_index == self.history.len() {
                self.replace_input("");
            } else {
                let history_entry = self.history[self.history_index].clone();
                self.replace_input(&history_entry);
            }
        }
    }
}

// Implement Window trait by delegating to text_window
impl Window for TerminalWindow {
    fn base(&self) -> &WindowBase {
        self.text_window.base()
    }

    fn base_mut(&mut self) -> &mut WindowBase {
        self.text_window.base_mut()
    }

    // TextWindow's custom set_bounds reallocates its grid buffer; route
    // through it instead of touching WindowBase directly. After the
    // grid is resized, push the new winsize to the pty so TIOCGWINSZ
    // returns truth and the foreground process receives SIGWINCH (vi
    // / less rely on this to redraw).
    fn set_bounds(&mut self, bounds: Rect) {
        self.text_window.set_bounds(bounds);
        if let Some((rows, cols)) = self.text_window.grid_size_opt() {
            crate::window::terminal::sync_terminal_winsize(self.text_window.id(), rows, cols);
        }
    }

    fn set_bounds_no_invalidate(&mut self, bounds: Rect) {
        self.text_window.set_bounds_no_invalidate(bounds);
        if let Some((rows, cols)) = self.text_window.grid_size_opt() {
            crate::window::terminal::sync_terminal_winsize(self.text_window.id(), rows, cols);
        }
    }

    /// Forward TextWindow's narrow per-cell hint so the compositor's
    /// dirty-rect tracker only marks the changed cells (plus the cursor
    /// cell), not the entire terminal-content bounds. Without this, the
    /// desktop's per-region wallpaper blit covers the whole TextWindow
    /// area and TextWindow's incremental paint can't restore it — typed
    /// characters would appear alone on wallpaper.
    fn dirty_rect_hint(&self) -> Option<Rect> {
        self.text_window.dirty_rect_hint()
    }

    /// Drain pending output buffers BEFORE the compositor consults dirty
    /// state. If we deferred this work to `paint()` (as the pre-Phase-B
    /// code did), `mark_dirty_for_invalidated_windows` would call
    /// `dirty_rect_hint` against an empty `dirty_cells`, mark the full
    /// terminal bounds dirty, and the desktop's per-region wallpaper
    /// blit would overwrite the rest of the terminal — leaving older
    /// output as wallpaper after the incremental paint redrew only the
    /// freshly-arrived cells.
    fn prepare_for_render(&mut self) {
        self.process_terminal_output();
        if self.text_window.needs_repaint() {
            self.text_window.process_console_output();
        }
    }

    fn paint(&mut self, device: &mut dyn GraphicsDevice) {
        crate::debug_trace!("TerminalWindow::paint called");

        // Paint the text window
        self.text_window.paint(device);

        // After painting, sync our input position with the text window's cursor
        // This is important because the TextWindow may have processed console output
        let (col, row) = self.text_window.cursor_position();

        // Only update input position if we're not in the middle of typing
        // (cursor should be after the prompt and any input)
        if self.input_buffer.is_empty() {
            self.input_start_col = col;
            self.input_start_row = row;
            crate::debug_trace!("Updated input position to ({}, {})", col, row);
        }
    }

    fn handle_event(&mut self, event: Event) -> EventResult {
        // Mouse-wheel scrolling drives the Screen's scrollback view.
        // Skip when alt-screen is active (TUIs own the wheel — vi /
        // less interpret it differently); the Screen ignores
        // `scroll_view` calls in that mode anyway.
        if let Event::Mouse(m) = &event {
            if let crate::window::event::MouseEventType::Scroll { delta_y, .. } = m.event_type {
                if delta_y != 0 {
                    self.screen.scroll_view(-(delta_y as isize));
                    self.sync_text_window_from_screen();
                    self.screen_dirty = false;
                    self.text_window.invalidate();
                    return EventResult::Handled;
                }
            }
        }

        // Shift+PgUp / Shift+PgDn — manual scrollback paging. PgUp
        // alone goes to the slave (vi / less need it); shift escapes
        // to scroll the host's view.
        if let Event::Keyboard(kbd) = &event {
            if kbd.pressed && kbd.modifiers.shift && !kbd.modifiers.ctrl && !kbd.modifiers.alt {
                let lines: isize = self
                    .text_window
                    .grid_size_opt()
                    .map(|(r, _)| r as isize)
                    .unwrap_or(24);
                match kbd.key_code {
                    KeyCode::PageUp => {
                        self.screen.scroll_view(lines.saturating_sub(2));
                        self.sync_text_window_from_screen();
                        self.screen_dirty = false;
                        self.text_window.invalidate();
                        return EventResult::Handled;
                    }
                    KeyCode::PageDown => {
                        self.screen.scroll_view(-(lines.saturating_sub(2)));
                        self.sync_text_window_from_screen();
                        self.screen_dirty = false;
                        self.text_window.invalidate();
                        return EventResult::Handled;
                    }
                    _ => {}
                }
            }
        }

        match event {
            Event::Keyboard(kbd_event) if kbd_event.pressed => {
                // Phase 3: when a user process is active and has put the
                // tty into raw mode (ICANON off), bypass the terminal's
                // line editor and forward each keystroke as raw bytes to
                // user stdin. zsh's zle does this so it can paint its
                // own prompt and handle history/completion.
                let tid = self.text_window.id();
                if crate::userland::stdin::is_active_for_terminal(tid)
                    && !crate::userland::tty::is_canonical_for_terminal(tid)
                {
                    let bytes =
                        encode_keystroke_for_raw_mode(kbd_event.key_code, kbd_event.modifiers);
                    if !bytes.is_empty() {
                        crate::userland::stdin::push_bytes_for_terminal(tid, &bytes);
                        if crate::userland::tty::is_echo_for_terminal(tid) {
                            // ECHO in raw mode: echo only printable bytes
                            // (skip control characters / escape sequences,
                            // which the user app will redraw itself).
                            for &b in &bytes {
                                if (0x20..0x7F).contains(&b) {
                                    self.feed_bytes(&[b]);
                                }
                            }
                        }
                    }
                    return EventResult::Handled;
                }

                match kbd_event.key_code {
                    KeyCode::Enter => {
                        self.handle_enter();
                        EventResult::Handled
                    }
                    KeyCode::Backspace => {
                        self.handle_backspace();
                        EventResult::Handled
                    }
                    KeyCode::Up => {
                        self.handle_up_arrow();
                        EventResult::Handled
                    }
                    KeyCode::Down => {
                        self.handle_down_arrow();
                        EventResult::Handled
                    }
                    _ => {
                        crate::debug_trace!("Terminal handling key: {:?}", kbd_event.key_code);
                        if let Some(ch) = keycode_to_char(kbd_event.key_code, kbd_event.modifiers) {
                            crate::debug_trace!("Converted to character: '{}'", ch);

                            // Log current cursor position
                            let (cur_col, cur_row) = self.text_window.cursor_position();
                            crate::debug_trace!(
                                "Current cursor at ({}, {}), input starts at ({}, {})",
                                cur_col,
                                cur_row,
                                self.input_start_col,
                                self.input_start_row
                            );

                            // Phase 3: when a user is active and ECHO is
                            // off in canonical mode (e.g. password
                            // prompt), accept the byte but don't paint it.
                            self.input_buffer.push(ch);
                            let echo_to_screen =
                                !crate::userland::stdin::is_active_for_terminal(tid)
                                    || crate::userland::tty::is_echo_for_terminal(tid);
                            if echo_to_screen {
                                let mut buf = [0u8; 4];
                                let s = ch.encode_utf8(&mut buf);
                                self.feed_bytes(s.as_bytes());
                            }

                            EventResult::Handled
                        } else {
                            crate::debug_trace!("No character mapping for key");
                            EventResult::Ignored
                        }
                    }
                }
            }
            _ => self.text_window.handle_event(event),
        }
    }

    fn can_focus(&self) -> bool {
        self.text_window.can_focus()
    }

    fn grid_size(&self) -> Option<(u16, u16)> {
        self.text_window.grid_size_opt()
    }
}

/// Feed the decimal ASCII representation of `v` through a Vte parser
/// into a Screen. Used by `clear_input_line` to build a CSI sequence
/// without intermediate allocations.
fn push_u16_decimal(
    vte: &mut crate::terminal::vte::Vte,
    screen: &mut crate::terminal::screen::Screen,
    mut v: u16,
) {
    if v == 0 {
        vte.advance(b'0', screen);
        return;
    }
    let mut buf = [0u8; 5];
    let mut n = 0;
    while v > 0 {
        buf[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    for i in (0..n).rev() {
        vte.advance(buf[i], screen);
    }
}

/// Translate a keyboard event into the raw bytes a Linux TTY would
/// deliver in raw mode. Used only when a user process is active and
/// has cleared `ICANON` via `tcsetattr`. Delegates to
/// [`crate::terminal::keys::encode_keystroke`], which covers F1–F12,
/// PgUp/PgDn, Insert, Shift/Ctrl/Alt+arrow, and back-tab in addition
/// to the basic VT100 set.
fn encode_keystroke_for_raw_mode(
    key: crate::window::event::KeyCode,
    modifiers: crate::window::event::KeyModifiers,
) -> Vec<u8> {
    crate::terminal::keys::encode_keystroke(key, modifiers)
}

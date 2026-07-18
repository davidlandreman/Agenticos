//! Task Manager — standalone ring-3 GUI application.
//!
//! Three tabs over a 1 Hz /proc sampler: Processes (sortable list,
//! End Task via `kill(2)`), Performance (CPU/memory history graphs +
//! stat tiles), and Network (RX/TX throughput + socket table). The
//! loop drains GUI events with `GUI_NONBLOCK`, samples once per
//! second, repaints only when dirty, and `nanosleep`s 100 ms between
//! iterations so the monitor stays honest on its own CPU column.

#![no_std]
#![no_main]

extern crate alloc;

mod sampler;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use dialogs::{DialogStatus, MessageBox, MessageChoice};
use gui::{
    decode_control_input, Button, Column, ColumnListEvent, ColumnListView, ColumnRow, ControlInput,
    PointerKind, TabBar, TimeSeriesGraph, Window, COLOR_ACCENT2, COLOR_BORDER, COLOR_HIGHLIGHT,
    COLOR_PANEL, COLOR_TEXT, COLOR_TEXT_DIM, COLOR_WHITE, GUI_EVENT_CLOSE, GUI_EVENT_KEY,
    GUI_EVENT_MOUSE, GUI_EVENT_RESIZE,
};
use runtime::GuiEvent;
use sampler::Snapshot;

const SIGTERM: i32 = 15;
const SIGKILL: i32 = 9;

/// Loop iterations (100 ms sleeps) between /proc samples.
const ITERS_PER_SAMPLE: u32 = 10;
/// Samples a TERM'd process may survive before the force-kill offer.
const FORCE_KILL_AFTER_SAMPLES: u8 = 2;

const STATUS_H: u32 = 22;
const TAB_PROCESSES: usize = 0;
const TAB_PERFORMANCE: usize = 1;
const TAB_NETWORK: usize = 2;

/// Key-space tag for kernel-thread rows (never collides with PIDs).
const KTHREAD_KEY: u64 = 1 << 32;

enum ModalPurpose {
    ConfirmEnd(u32),
    ConfirmForce(u32),
}

struct TaskMgr {
    window: Window,
    tabs: TabBar,
    proc_list: ColumnListView,
    end_task: Button,
    cpu_graph: TimeSeriesGraph,
    mem_graph: TimeSeriesGraph,
    net_graph: TimeSeriesGraph,
    socket_list: ColumnListView,
    modal: Option<(MessageBox, ModalPurpose)>,
    prev: Option<Snapshot>,
    snap: Snapshot,
    /// Derived once per sample from consecutive snapshots.
    cpu_pct10: u64,
    rx_rate_bps: u64,
    tx_rate_bps: u64,
    pending_kill: Option<(u32, u8)>,
    iters: u32,
    dirty: bool,
    exit: bool,
}

fn fmt_pct10(tenths: u64) -> String {
    format!("{}.{}", tenths / 10, tenths % 10)
}

fn fmt_mmss(ticks: u64) -> String {
    let secs = ticks / 100;
    format!("{}:{:02}", secs / 60, secs % 60)
}

fn fmt_uptime(ticks: u64) -> String {
    let secs = ticks / 100;
    format!("{}:{:02}:{:02}", secs / 3600, (secs / 60) % 60, secs % 60)
}

fn fmt_kb(kb: u64) -> String {
    if kb >= 10 * 1024 {
        format!("{} MB", kb / 1024)
    } else {
        format!("{} KB", kb)
    }
}

fn fmt_rate(bps: u64) -> String {
    if bps >= 1024 * 1024 {
        format!(
            "{}.{} MB/s",
            bps / (1024 * 1024),
            (bps % (1024 * 1024)) * 10 / (1024 * 1024)
        )
    } else if bps >= 1024 {
        format!("{}.{} KB/s", bps / 1024, (bps % 1024) * 10 / 1024)
    } else {
        format!("{} B/s", bps)
    }
}

fn state_label(state: char) -> &'static str {
    match state {
        'R' => "Running",
        'Z' => "Zombie",
        _ => "Waiting",
    }
}

impl TaskMgr {
    fn new() -> Result<Self, i64> {
        let window = Window::new(640, 480, "Task Manager")?;
        let tabs = TabBar::new(0, 0, 640, &["Processes", "Performance", "Network"]);
        let proc_columns = alloc::vec![
            Column::numeric("PID", 56),
            Column::new("Name", 152),
            Column::new("State", 88),
            Column::numeric("CPU %", 64),
            Column::numeric("Time", 72),
            Column::numeric("Mem", 88),
        ];
        let socket_columns = alloc::vec![
            Column::numeric("Id", 40),
            Column::new("Proto", 56),
            Column::new("State", 104),
            Column::new("Local", 176),
            Column::new("Remote", 176),
        ];
        let mut app = Self {
            window,
            tabs,
            proc_list: ColumnListView::new(0, 0, 10, 10, proc_columns),
            end_task: Button::new("End Task", 0, 0, 96, 24),
            cpu_graph: TimeSeriesGraph::new(0, 0, 10, 10, 120, Some(100.0)),
            mem_graph: TimeSeriesGraph::new(0, 0, 10, 10, 120, None),
            net_graph: TimeSeriesGraph::new(0, 0, 10, 10, 120, None),
            socket_list: ColumnListView::new(0, 0, 10, 10, socket_columns),
            modal: None,
            prev: None,
            snap: Snapshot::default(),
            cpu_pct10: 0,
            rx_rate_bps: 0,
            tx_rate_bps: 0,
            pending_kill: None,
            iters: ITERS_PER_SAMPLE, // sample immediately on first loop
            dirty: true,
            exit: false,
        };
        // Default: busiest processes first.
        app.proc_list.sort_col = 3;
        app.proc_list.sort_desc = true;
        // Memory graph pins to MemTotal once known (first sample).
        app.layout();
        Ok(app)
    }

    /// Recompute widget geometry from the current canvas size.
    fn layout(&mut self) {
        let w = self.window.canvas().width().max(420);
        let h = self.window.canvas().height().max(300);
        let content_y = TabBar::HEIGHT as i32;
        let content_h = h.saturating_sub(TabBar::HEIGHT + STATUS_H);

        self.tabs.w = w;

        // Processes tab: list + action row.
        let action_h = 36;
        self.proc_list.x = 8;
        self.proc_list.y = content_y + 8;
        self.proc_list.w = w - 16;
        self.proc_list.h = content_h.saturating_sub(16 + action_h);
        self.end_task.x = w as i32 - 8 - self.end_task.w as i32;
        self.end_task.y = content_y + content_h as i32 - action_h as i32 + 4;

        // Performance tab: two stacked graphs + tile block below.
        let tiles_h = 76;
        let graph_h = ((content_h.saturating_sub(24 + 12 + tiles_h)) / 2).max(60);
        self.cpu_graph.x = 12;
        self.cpu_graph.y = content_y + 12;
        self.cpu_graph.w = w - 24;
        self.cpu_graph.h = graph_h;
        self.mem_graph.x = 12;
        self.mem_graph.y = self.cpu_graph.y + graph_h as i32 + 12;
        self.mem_graph.w = w - 24;
        self.mem_graph.h = graph_h;

        // Network tab: throughput graph + totals line + socket table.
        self.net_graph.x = 12;
        self.net_graph.y = content_y + 12;
        self.net_graph.w = w - 24;
        self.net_graph.h = 120.min(content_h / 2).max(60);
        let sockets_y = self.net_graph.y + self.net_graph.h as i32 + 30;
        self.socket_list.x = 8;
        self.socket_list.y = sockets_y;
        self.socket_list.w = w - 16;
        self.socket_list.h = (content_y + content_h as i32 - 8 - sockets_y)
            .max(ColumnListView::HEADER_HEIGHT as i32 + 16) as u32;
    }

    // ----------------------------------------------------------
    // Sampling
    // ----------------------------------------------------------

    fn take_sample(&mut self) {
        let snap = sampler::sample();
        let dticks = self
            .prev
            .as_ref()
            .map(|p| snap.uptime_ticks.saturating_sub(p.uptime_ticks))
            .unwrap_or(0);

        // Aggregate CPU rate.
        self.cpu_pct10 = if dticks > 0 {
            let prev = self.prev.as_ref().unwrap();
            let busy =
                (snap.cpu_user + snap.cpu_system).saturating_sub(prev.cpu_user + prev.cpu_system);
            (busy * 1000 / dticks).min(1000)
        } else {
            0
        };

        // Network rates (bytes/sec).
        if dticks > 0 {
            let prev = self.prev.as_ref().unwrap();
            self.rx_rate_bps = snap.rx_bytes.saturating_sub(prev.rx_bytes) * 100 / dticks;
            self.tx_rate_bps = snap.tx_bytes.saturating_sub(prev.tx_bytes) * 100 / dticks;
        }

        // Per-row CPU rates need the previous per-pid/tid tick counts.
        let prev_proc: Vec<(u32, u64)> = self
            .prev
            .as_ref()
            .map(|p| p.procs.iter().map(|r| (r.pid, r.utime_ticks)).collect())
            .unwrap_or_default();
        let prev_kthread: Vec<(u32, u64)> = self
            .prev
            .as_ref()
            .map(|p| {
                p.kthreads
                    .iter()
                    .map(|r| (r.tid, r.runtime_ticks))
                    .collect()
            })
            .unwrap_or_default();
        let row_pct10 = |prev_map: &[(u32, u64)], id: u32, now: u64| -> u64 {
            if dticks == 0 {
                return 0;
            }
            let before = prev_map
                .iter()
                .find(|(pid, _)| *pid == id)
                .map(|(_, t)| *t)
                .unwrap_or(now);
            (now.saturating_sub(before) * 1000 / dticks).min(1000)
        };

        let mut rows: Vec<ColumnRow> = Vec::new();
        for p in &snap.procs {
            rows.push(ColumnRow::new(
                p.pid as u64,
                alloc::vec![
                    format!("{}", p.pid),
                    p.comm.clone(),
                    String::from(state_label(p.state)),
                    fmt_pct10(row_pct10(&prev_proc, p.pid, p.utime_ticks)),
                    fmt_mmss(p.utime_ticks),
                    fmt_kb(p.rss_pages * 4),
                ],
            ));
        }
        for k in &snap.kthreads {
            let mut row = ColumnRow::new(
                KTHREAD_KEY | k.tid as u64,
                alloc::vec![
                    format!("{}", k.tid),
                    format!("[{}]", k.name),
                    {
                        let mut s = k.state.clone();
                        if let Some(first) = s.get_mut(0..1) {
                            first.make_ascii_uppercase();
                        }
                        s
                    },
                    fmt_pct10(row_pct10(&prev_kthread, k.tid, k.runtime_ticks)),
                    fmt_mmss(k.runtime_ticks),
                    fmt_kb(k.stack_bytes / 1024),
                ],
            );
            row.dim = true;
            rows.push(row);
        }
        self.proc_list.set_rows(rows);

        // Graphs.
        self.cpu_graph.push((self.cpu_pct10 as f32) / 10.0, None);
        let used_mb = snap.mem_total_kb.saturating_sub(snap.mem_free_kb) / 1024;
        self.mem_graph.fixed_max = Some(((snap.mem_total_kb / 1024) as f32).max(1.0));
        self.mem_graph.push(used_mb as f32, None);
        self.net_graph.push(
            self.rx_rate_bps as f32 / 1024.0,
            Some(self.tx_rate_bps as f32 / 1024.0),
        );

        // Socket table.
        let socket_rows: Vec<ColumnRow> = snap
            .sockets
            .iter()
            .map(|s| {
                ColumnRow::new(
                    s.id,
                    alloc::vec![
                        format!("{}", s.id),
                        s.proto.clone(),
                        s.state.clone(),
                        s.local.clone(),
                        s.remote.clone(),
                    ],
                )
            })
            .collect();
        self.socket_list.set_rows(socket_rows);

        self.prev = Some(core::mem::replace(&mut self.snap, snap));
        self.advance_kill_escalation();
        self.dirty = true;
    }

    /// After End Task sent SIGTERM, watch whether the target actually
    /// died; offer SIGKILL once it has survived a couple of samples.
    fn advance_kill_escalation(&mut self) {
        let Some((pid, samples)) = self.pending_kill else {
            return;
        };
        let alive = self.snap.procs.iter().any(|p| p.pid == pid);
        if !alive {
            self.pending_kill = None;
            return;
        }
        let samples = samples + 1;
        self.pending_kill = Some((pid, samples));
        if samples >= FORCE_KILL_AFTER_SAMPLES && self.modal.is_none() {
            let text = format!(
                "PID {} did not exit after SIGTERM.\nForce kill with SIGKILL?",
                pid
            );
            if let Ok(msgbox) = MessageBox::confirm("Not Responding", &text) {
                self.modal = Some((msgbox, ModalPurpose::ConfirmForce(pid)));
            }
            self.pending_kill = None;
        }
    }

    // ----------------------------------------------------------
    // Input
    // ----------------------------------------------------------

    fn selected_target(&self) -> Option<(u32, String)> {
        let row = self.proc_list.selected_row()?;
        if row.dim {
            return None; // kernel threads are view-only
        }
        let name = row.cells.get(1).cloned().unwrap_or_default();
        Some((row.key as u32, name))
    }

    fn request_end_task(&mut self) {
        if self.modal.is_some() {
            return;
        }
        let Some((pid, name)) = self.selected_target() else {
            return;
        };
        let text = format!("End process {} (PID {})?", name, pid);
        if let Ok(msgbox) = MessageBox::confirm("End Task", &text) {
            self.modal = Some((msgbox, ModalPurpose::ConfirmEnd(pid)));
            self.dirty = true;
        }
    }

    fn on_modal_done(&mut self, purpose: ModalPurpose, choice: Option<MessageChoice>) {
        let confirmed = matches!(choice, Some(MessageChoice::Yes));
        match purpose {
            ModalPurpose::ConfirmEnd(pid) if confirmed => {
                runtime::kill(pid as i32, SIGTERM);
                self.pending_kill = Some((pid, 0));
            }
            ModalPurpose::ConfirmForce(pid) if confirmed => {
                runtime::kill(pid as i32, SIGKILL);
            }
            _ => {}
        }
        self.dirty = true;
    }

    fn handle_key(&mut self, payload: [u32; 6]) {
        let key = payload[0];
        match key {
            runtime::KEY_TAB => {
                self.tabs.cycle();
                self.dirty = true;
            }
            runtime::KEY_DELETE if self.tabs.active == TAB_PROCESSES => {
                self.request_end_task();
            }
            _ => {
                let list = match self.tabs.active {
                    TAB_PROCESSES => &mut self.proc_list,
                    TAB_NETWORK => &mut self.socket_list,
                    _ => return,
                };
                match list.key(key) {
                    ColumnListEvent::None => {}
                    ColumnListEvent::Activated(_) if self.tabs.active == TAB_PROCESSES => {
                        self.dirty = true;
                        self.request_end_task();
                    }
                    _ => self.dirty = true,
                }
            }
        }
    }

    fn handle_mouse(&mut self, event: &GuiEvent) {
        let Some(ControlInput::Pointer(input)) = decode_control_input(event) else {
            return;
        };
        if matches!(input.kind, PointerKind::Down) {
            if let Some(tab) = self.tabs.hit(input.x, input.y) {
                if tab != self.tabs.active {
                    self.tabs.active = tab;
                    self.dirty = true;
                }
                return;
            }
        }
        if self.tabs.active == TAB_PROCESSES {
            let enabled = self
                .proc_list
                .selected_row()
                .map(|row| !row.dim)
                .unwrap_or(false);
            self.end_task.set_enabled(enabled);
            let response = self.end_task.handle_pointer(input);
            if response.action == Some(gui::ButtonAction::Activated) {
                self.request_end_task();
                return;
            }
            if response.consumed {
                self.dirty |= response.repaint;
                return;
            }
        }
        let response = match self.tabs.active {
            TAB_PROCESSES => self.proc_list.handle_pointer(input),
            TAB_NETWORK => self.socket_list.handle_pointer(input),
            _ => return,
        };
        if response.repaint || response.consumed {
            self.dirty = true;
        }
        if let Some(ColumnListEvent::Activated(_)) = response.action {
            if self.tabs.active == TAB_PROCESSES {
                self.request_end_task();
            }
        }
    }

    fn route(&mut self, event: GuiEvent) {
        if gui::theme::apply_system_event(&event) {
            if let Some((modal, _)) = self.modal.as_mut() {
                modal.refresh_theme();
            }
            self.dirty = true;
            return;
        }
        if let Some((modal, _)) = self.modal.as_mut() {
            if event.window == modal.window_handle() {
                if let DialogStatus::Done(choice) = modal.handle_event(&event) {
                    let (_, purpose) = self.modal.take().unwrap();
                    self.on_modal_done(purpose, choice);
                }
                return;
            }
        }
        if event.window != self.window.handle() {
            return;
        }
        match event.kind {
            GUI_EVENT_CLOSE => self.exit = true,
            GUI_EVENT_RESIZE => {
                self.window.resize(event.payload[0], event.payload[1]);
                self.layout();
                self.dirty = true;
            }
            GUI_EVENT_KEY if event.payload[3] != 0 && self.modal.is_none() => {
                self.handle_key(event.payload);
            }
            GUI_EVENT_MOUSE if self.modal.is_none() => {
                self.handle_mouse(&event);
            }
            _ => {}
        }
    }

    // ----------------------------------------------------------
    // Rendering
    // ----------------------------------------------------------

    fn render(&mut self) {
        // Snapshot everything the canvas closure needs (borrow split).
        let active = self.tabs.active;
        let canvas_h = self.window.canvas().height();
        let canvas_w = self.window.canvas().width();
        let status = format!(
            "Processes: {}   CPU: {}%   Mem: {}%   Up: {}",
            self.snap.procs.len(),
            fmt_pct10(self.cpu_pct10),
            if self.snap.mem_total_kb > 0 {
                (self.snap.mem_total_kb - self.snap.mem_free_kb) * 100 / self.snap.mem_total_kb
            } else {
                0
            },
            fmt_uptime(self.snap.uptime_ticks),
        );

        let canvas = self.window.canvas_mut();
        canvas.clear(COLOR_WHITE);
        match active {
            TAB_PROCESSES => {
                self.proc_list.draw(canvas);
                let enabled = self
                    .proc_list
                    .selected_row()
                    .map(|r| !r.dim)
                    .unwrap_or(false);
                self.end_task.set_enabled(enabled);
                self.end_task.draw_control(canvas, false);
                if !enabled {
                    canvas.draw_text(
                        self.proc_list.x,
                        self.end_task.y + 8,
                        "Select a process to end it",
                        COLOR_TEXT_DIM,
                    );
                }
            }
            TAB_PERFORMANCE => {
                let cpu_label = format!("{}%", fmt_pct10(self.cpu_pct10));
                self.cpu_graph.draw(canvas, "CPU", &cpu_label);
                let used_mb = self.snap.mem_total_kb.saturating_sub(self.snap.mem_free_kb) / 1024;
                let mem_label = format!("{} / {} MB", used_mb, self.snap.mem_total_kb / 1024);
                self.mem_graph.draw(canvas, "Memory", &mem_label);
                let tiles_y = self.mem_graph.y + self.mem_graph.h as i32 + 12;
                let lines = [
                    format!("Uptime            {}", fmt_uptime(self.snap.uptime_ticks)),
                    format!(
                        "Processes         {} ring-3, {} kernel threads",
                        self.snap.procs.len(),
                        self.snap.kthreads.len()
                    ),
                    format!(
                        "Kernel heap       {} / {}",
                        fmt_kb(self.snap.heap_used_kb),
                        fmt_kb(self.snap.heap_total_kb)
                    ),
                    format!(
                        "Physical memory   {} / {}",
                        fmt_kb(self.snap.mem_total_kb - self.snap.mem_free_kb),
                        fmt_kb(self.snap.mem_total_kb)
                    ),
                    format!("Sockets           {}", self.snap.sockets.len()),
                ];
                for (i, line) in lines.iter().enumerate() {
                    canvas.draw_text(16, tiles_y + i as i32 * 14, line, COLOR_TEXT);
                }
            }
            TAB_NETWORK => {
                let label = format!(
                    "RX {}  TX {}",
                    fmt_rate(self.rx_rate_bps),
                    fmt_rate(self.tx_rate_bps)
                );
                self.net_graph.draw(canvas, "Throughput (KB/s)", &label);
                let totals_y = self.net_graph.y + self.net_graph.h as i32 + 8;
                // Series legend + since-boot totals.
                canvas.fill_rect(16, totals_y + 2, 8, 8, COLOR_HIGHLIGHT);
                canvas.fill_rect(90, totals_y + 2, 8, 8, COLOR_ACCENT2);
                let totals = format!(
                    "RX        TX        total {} in / {} out ({} / {} packets)",
                    fmt_kb(self.snap.rx_bytes / 1024),
                    fmt_kb(self.snap.tx_bytes / 1024),
                    self.snap.rx_packets,
                    self.snap.tx_packets,
                );
                canvas.draw_text(28, totals_y + 2, &totals, COLOR_TEXT);
                self.socket_list.draw(canvas);
            }
            _ => {}
        }
        self.tabs.draw(canvas);
        // Status strip.
        let strip_y = canvas_h as i32 - STATUS_H as i32;
        canvas.fill_rect(0, strip_y, canvas_w, STATUS_H, COLOR_PANEL);
        canvas.horizontal_line(0, strip_y, canvas_w, COLOR_BORDER);
        canvas.draw_text(8, strip_y + 7, &status, COLOR_TEXT);
        let _ = self.window.present();
        self.dirty = false;
    }

    // ----------------------------------------------------------
    // Main loop
    // ----------------------------------------------------------

    fn run(&mut self) -> i64 {
        loop {
            // Drain the event queue without blocking.
            loop {
                let mut event = GuiEvent::default();
                let r = runtime::gui_next_event(&mut event, runtime::GUI_NONBLOCK);
                if r != 0 {
                    break;
                }
                self.route(event);
                if self.exit {
                    return 0;
                }
            }

            self.iters += 1;
            if self.iters >= ITERS_PER_SAMPLE {
                self.iters = 0;
                self.take_sample();
            }
            if self.dirty {
                self.render();
            }

            let delay = runtime::Timespec {
                tv_sec: 0,
                tv_nsec: 100_000_000,
            };
            runtime::nanosleep(&delay, None);
        }
    }
}

#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        "mov rdi, rsp",
        "and rsp, -16",
        "call {}",
        "ud2",
        sym taskmgr_main,
    );
}

unsafe extern "C" fn taskmgr_main(_stack: *const u64) -> ! {
    let code = match TaskMgr::new() {
        Ok(mut app) => app.run(),
        Err(error) => error,
    };
    runtime::exit(if code == 0 { 0 } else { 1 })
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe { runtime::exit(127) }
}

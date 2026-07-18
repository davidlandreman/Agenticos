//! Synthetic read-only `/proc` namespace.
//!
//! Modeled on the `/bin` synthesis pattern (`bin_namespace.rs`): pure
//! kernel generation, no backing files. Content for a file is
//! generated **once at `open()`** into a heap buffer owned by the fd
//! (`FdSlot::VirtualFile`), so no kernel lock is ever held across a
//! user `read()` and every open sees one consistent snapshot.
//!
//! Two tiers of files:
//!
//! - **Linux-shaped** (`uptime`, `meminfo`, `stat`, `loadavg`,
//!   `net/dev`, `/proc/<pid>/{stat,status,cmdline,statm}`) — minimal
//!   but well-formed subsets scoped to what BusyBox `ps`/`top` parse.
//!   Only real ring-3 processes appear as `/proc/<pid>`; kernel
//!   threads never masquerade with fake PIDs.
//! - **AgenticOS extensions** (`/proc/agenticos/{kthreads,gui,
//!   sockets}`) — line-oriented tab-separated tables with no Linux
//!   format constraints.
//!
//! Lock discipline: each generator takes at most one subsystem lock at
//! a time (`PROCESS_TABLE`, `SCHEDULER`, `NETWORK` via
//! `net::…` helpers) and returns owned data. The per-process RSS count
//! walks page tables via `with_memory_mapper` *inside* the
//! `PROCESS_TABLE` critical section — safe because the mapper is not a
//! lock (`get_mapper` is a plain static) and the InterruptMutex masks
//! preemption on the single core, so no one can mutate user page
//! tables mid-walk.

use crate::userland::lifecycle::{ExitKind, KERNEL_PID, PROCESS_TABLE};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// A resolved `/proc` node, generated at open time.
pub enum ProcNode {
    /// Regular file: full snapshot of its content.
    File(Vec<u8>),
    /// Directory: `(name, is_dir)` listing, not including `.`/`..`.
    Dir(Vec<(String, bool)>),
}

/// Structural classification without content generation (for
/// `stat`/`access`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ProcNodeKind {
    Dir,
    File,
}

/// `/proc` prefix guard. Runs on normalized paths only (see
/// `resolve_user_path`), mirroring `etc::is_managed_path`'s
/// component-bounded check so `/procfoo` never matches.
pub fn is_proc_path(path: &str) -> bool {
    path == "/proc" || path.starts_with("/proc/")
}

/// Names of the static top-level `/proc` files.
const TOP_FILES: &[&str] = &["loadavg", "meminfo", "stat", "uptime"];
/// Names of the `/proc/agenticos` extension files.
const AGENTICOS_FILES: &[&str] = &["gui", "kthreads", "sockets"];
/// Per-PID directory entries.
const PID_FILES: &[&str] = &["cmdline", "stat", "statm", "status"];

/// Resolve `/proc/self` to the calling process's PID and split `path`
/// into components after `/proc`. Returns `None` for non-proc paths.
fn components(path: &str) -> Option<Vec<String>> {
    let rest = path.strip_prefix("/proc")?;
    let mut out = Vec::new();
    for part in rest.split('/') {
        if part.is_empty() {
            continue;
        }
        if out.is_empty() && part == "self" {
            out.push(format!("{}", crate::userland::lifecycle::current_pid()));
        } else {
            out.push(String::from(part));
        }
    }
    Some(out)
}

/// Bounded PID parse: plain decimal, no sign, no leading zeros (except
/// "0" itself, which is never a live ring-3 PID).
fn parse_pid(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > 10 || (s.len() > 1 && s.starts_with('0')) {
        return None;
    }
    if !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<u32>().ok()
}

fn pid_is_live(pid: u32) -> bool {
    if pid == KERNEL_PID {
        return false;
    }
    let g = PROCESS_TABLE.lock();
    g.by_pid.contains_key(&pid) && g.thread_groups.get(&pid).copied().unwrap_or(pid) == pid
}

/// Classify a normalized `/proc` path without generating content.
pub fn classify(path: &str) -> Option<ProcNodeKind> {
    let parts = components(path)?;
    match parts.len() {
        0 => Some(ProcNodeKind::Dir),
        1 => {
            let p = parts[0].as_str();
            if TOP_FILES.contains(&p) {
                Some(ProcNodeKind::File)
            } else if p == "net" || p == "agenticos" {
                Some(ProcNodeKind::Dir)
            } else {
                let pid = parse_pid(p)?;
                pid_is_live(pid).then_some(ProcNodeKind::Dir)
            }
        }
        2 => match (parts[0].as_str(), parts[1].as_str()) {
            ("net", "dev") => Some(ProcNodeKind::File),
            ("agenticos", f) if AGENTICOS_FILES.contains(&f) => Some(ProcNodeKind::File),
            (p, f) if PID_FILES.contains(&f) => {
                let pid = parse_pid(p)?;
                pid_is_live(pid).then_some(ProcNodeKind::File)
            }
            _ => None,
        },
        _ => None,
    }
}

/// Resolve + generate the node at `path`. `None` → ENOENT.
pub fn open_node(path: &str) -> Option<ProcNode> {
    let parts = components(path)?;
    match parts.len() {
        0 => Some(ProcNode::Dir(root_listing())),
        1 => match parts[0].as_str() {
            "uptime" => Some(ProcNode::File(gen_uptime())),
            "meminfo" => Some(ProcNode::File(gen_meminfo())),
            "stat" => Some(ProcNode::File(gen_stat())),
            "loadavg" => Some(ProcNode::File(gen_loadavg())),
            "net" => Some(ProcNode::Dir(alloc::vec![(String::from("dev"), false)])),
            "agenticos" => Some(ProcNode::Dir(
                AGENTICOS_FILES
                    .iter()
                    .map(|f| (String::from(*f), false))
                    .collect(),
            )),
            p => {
                let pid = parse_pid(p)?;
                pid_is_live(pid).then(|| {
                    ProcNode::Dir(
                        PID_FILES
                            .iter()
                            .map(|f| (String::from(*f), false))
                            .collect(),
                    )
                })
            }
        },
        2 => match (parts[0].as_str(), parts[1].as_str()) {
            ("net", "dev") => Some(ProcNode::File(gen_net_dev())),
            ("agenticos", "kthreads") => Some(ProcNode::File(gen_kthreads())),
            ("agenticos", "gui") => Some(ProcNode::File(gen_gui())),
            ("agenticos", "sockets") => Some(ProcNode::File(gen_sockets())),
            (p, file) if PID_FILES.contains(&file) => {
                let pid = parse_pid(p)?;
                let snap = ring3_snapshot(pid)?;
                let content = match file {
                    "stat" => gen_pid_stat(&snap),
                    "status" => gen_pid_status(&snap),
                    "cmdline" => gen_pid_cmdline(&snap),
                    "statm" => gen_pid_statm(&snap),
                    _ => unreachable!(),
                };
                Some(ProcNode::File(content))
            }
            _ => None,
        },
        _ => None,
    }
}

fn root_listing() -> Vec<(String, bool)> {
    let mut entries: Vec<(String, bool)> = TOP_FILES
        .iter()
        .map(|f| (String::from(*f), false))
        .collect();
    entries.push((String::from("agenticos"), true));
    entries.push((String::from("net"), true));
    entries.push((String::from("self"), true));
    let pids: Vec<u32> = {
        let g = PROCESS_TABLE.lock();
        g.by_pid
            .keys()
            .copied()
            .filter(|&pid| {
                pid != KERNEL_PID && g.thread_groups.get(&pid).copied().unwrap_or(pid) == pid
            })
            .collect()
    };
    for pid in pids {
        entries.push((format!("{}", pid), true));
    }
    entries
}

// ---------------------------------------------------------------
// Per-process snapshots
// ---------------------------------------------------------------

/// Owned view of one ring-3 process. Scheduler state is captured before the
/// `PROCESS_TABLE` critical section so the subsystem locks never nest.
pub struct Ring3Snapshot {
    pub pid: u32,
    pub ppid: u32,
    /// Linux state char: `R`unning/runnable, `S`leeping, `Z`ombie.
    pub state: char,
    /// Short command name (basename of argv[0], ≤ 15 chars).
    pub comm: String,
    /// Retained argv (possibly truncated at retention time).
    pub cmdline: Vec<String>,
    pub utime_ticks: u64,
    /// Total VMA span in bytes (VSZ).
    pub vsize_bytes: u64,
    /// Resident 4 KiB pages (RSS).
    pub rss_pages: u64,
    pub threads: usize,
}

fn comm_of(exe_path: &Option<String>, cmdline: &[String]) -> String {
    let raw = cmdline
        .first()
        .map(String::as_str)
        .or(exe_path.as_deref())
        .unwrap_or("?");
    let base = raw.rsplit('/').next().unwrap_or(raw);
    base.chars().take(15).collect()
}

fn snapshot_one(
    pid: u32,
    p: &crate::userland::lifecycle::Process,
    run_state: Option<crate::process::entity::RunState>,
    threads: usize,
    utime_ticks: u64,
) -> Ring3Snapshot {
    let state = if p.exit_kind != ExitKind::None {
        'Z'
    } else if matches!(
        run_state,
        Some(crate::process::entity::RunState::Ready | crate::process::entity::RunState::Running)
    ) {
        'R'
    } else {
        'S'
    };
    let vsize_bytes: u64 = p
        .address_space
        .as_ref()
        .map(|a| {
            a.vmas()
                .as_slice()
                .iter()
                .map(|v| v.end.saturating_sub(v.start))
                .sum()
        })
        .unwrap_or(0);
    let rss_pages = p
        .address_space
        .as_ref()
        .and_then(|a| {
            let l4 = a.l4_frame();
            crate::mm::memory::with_memory_mapper(|m| m.count_user_resident_pages(l4))
        })
        .unwrap_or(0);
    Ring3Snapshot {
        pid,
        ppid: p.parent_pid,
        state,
        comm: comm_of(&p.exe_path, &p.cmdline),
        cmdline: p.cmdline.clone(),
        utime_ticks,
        vsize_bytes,
        rss_pages,
        threads,
    }
}

/// Snapshot one live ring-3 process. `None` if the PID is absent or
/// the kernel sentinel.
pub fn ring3_snapshot(pid: u32) -> Option<Ring3Snapshot> {
    if pid == KERNEL_PID {
        return None;
    }
    let scheduler = crate::process::scheduler::SCHEDULER.lock();
    let g = PROCESS_TABLE.lock();
    if g.thread_groups.get(&pid).copied().unwrap_or(pid) != pid {
        return None;
    }
    let p = g.by_pid.get(&pid)?;
    let members = g
        .by_pid
        .keys()
        .copied()
        .filter(|tid| g.thread_groups.get(tid).copied().unwrap_or(*tid) == pid)
        .collect::<Vec<_>>();
    let run_state = members.iter().find_map(|tid| {
        let state = scheduler.entity_state(crate::process::entity::EntityId::UserProcess(*tid));
        matches!(
            state,
            Some(
                crate::process::entity::RunState::Ready | crate::process::entity::RunState::Running
            )
        )
        .then_some(state)
        .flatten()
    });
    let utime_ticks = members
        .iter()
        .filter_map(|tid| g.by_pid.get(tid))
        .fold(0u64, |sum, task| sum.saturating_add(task.utime_ticks));
    Some(snapshot_one(pid, p, run_state, members.len(), utime_ticks))
}

// ---------------------------------------------------------------
// Generators
// ---------------------------------------------------------------

fn cpu_time_snapshots() -> Vec<(usize, crate::arch::x86_64::percpu::CpuTimeSnapshot)> {
    (0..crate::arch::x86_64::smp::online_cpu_count())
        .filter_map(|cpu| {
            crate::arch::x86_64::percpu::cpu_time_snapshot(cpu).map(|times| (cpu, times))
        })
        .collect()
}

fn gen_uptime() -> Vec<u8> {
    let ticks = crate::arch::x86_64::interrupts::get_timer_ticks();
    // Linux reports the sum of idle time across all online CPUs, so this
    // value may exceed wall-clock uptime on an SMP system.
    let idle = cpu_time_snapshots()
        .iter()
        .fold(0u64, |sum, (_, times)| sum.saturating_add(times.idle));
    // 100 Hz: tick count / 100 = seconds, remainder = centiseconds.
    format!(
        "{}.{:02} {}.{:02}\n",
        ticks / 100,
        ticks % 100,
        idle / 100,
        idle % 100
    )
    .into_bytes()
}

fn gen_loadavg() -> Vec<u8> {
    let running = crate::process::scheduler::SCHEDULER
        .lock()
        .runnable_user_count();
    let (total, last_pid) = {
        let g = PROCESS_TABLE.lock();
        let leaders = g
            .by_pid
            .keys()
            .copied()
            .filter(|pid| g.thread_groups.get(pid).copied().unwrap_or(*pid) == *pid);
        let total = leaders.clone().count().saturating_sub(1); // minus sentinel
        let last_pid = leaders.max().unwrap_or(0);
        (total, last_pid)
    };
    // No load-average bookkeeping — report zeros plus the honest
    // running/total counts Linux puts in fields 4 and 5.
    format!("0.00 0.00 0.00 {}/{} {}\n", running, total, last_pid).into_bytes()
}

fn gen_meminfo() -> Vec<u8> {
    let frames = crate::mm::memory::with_memory_mapper(|m| m.frame_stats());
    let heap = crate::mm::heap::stats();
    let (total_kb, free_kb) = frames
        .map(|f| (f.total_usable * 4, f.free * 4))
        .unwrap_or((0, 0));
    let (heap_total_kb, heap_used_kb) = heap
        .map(|h| ((h.size / 1024) as u64, (h.used / 1024) as u64))
        .unwrap_or((0, 0));
    let mut out = String::new();
    out.push_str(&format!("MemTotal:       {:>8} kB\n", total_kb));
    out.push_str(&format!("MemFree:        {:>8} kB\n", free_kb));
    out.push_str(&format!("MemAvailable:   {:>8} kB\n", free_kb));
    out.push_str("Buffers:               0 kB\n");
    out.push_str("Cached:                0 kB\n");
    out.push_str("SwapTotal:             0 kB\n");
    out.push_str("SwapFree:              0 kB\n");
    // AgenticOS extension lines — harmless to Linux parsers.
    out.push_str(&format!("KernelHeapTotal:{:>8} kB\n", heap_total_kb));
    out.push_str(&format!("KernelHeapUsed: {:>8} kB\n", heap_used_kb));
    out.into_bytes()
}

fn gen_stat() -> Vec<u8> {
    let cpu_times = cpu_time_snapshots();
    let (user, system, idle) =
        cpu_times
            .iter()
            .fold((0u64, 0u64, 0u64), |(user, system, idle), (_, times)| {
                (
                    user.saturating_add(times.user),
                    system.saturating_add(times.system),
                    idle.saturating_add(times.idle),
                )
            });
    let processes = {
        let g = PROCESS_TABLE.lock();
        g.by_pid
            .keys()
            .filter(|tid| g.thread_groups.get(tid).copied().unwrap_or(**tid) == **tid)
            .count()
            .saturating_sub(1)
    };
    let mut out = String::new();
    // Every local scheduling timer runs at 100 Hz, so these counters are
    // USER_HZ jiffies directly. Aggregate fields are derived from this exact
    // local snapshot and therefore equal the sum of the cpuN rows below.
    out.push_str(&format!(
        "cpu  {} 0 {} {} 0 0 0 0 0 0\n",
        user, system, idle
    ));
    for (cpu, times) in cpu_times {
        out.push_str(&format!(
            "cpu{} {} 0 {} {} 0 0 0 0 0 0\n",
            cpu, times.user, times.system, times.idle
        ));
    }
    out.push_str("btime 0\n");
    out.push_str(&format!("processes {}\n", processes));
    out.push_str("procs_running 1\n");
    out.push_str("procs_blocked 0\n");
    out.into_bytes()
}

fn gen_net_dev() -> Vec<u8> {
    let mut out = String::from(
        "Inter-|   Receive                                                |  Transmit\n \
         face |bytes    packets errs drop fifo frame compressed multicast|bytes    \
         packets errs drop fifo colls carrier compressed\n",
    );
    if let Some(c) = crate::net::counters() {
        out.push_str(&format!(
            "  eth0: {:>7} {:>7} {:>4} {:>4} {:>4} {:>5} {:>10} {:>9} {:>8} {:>7} {:>4} {:>4} {:>4} {:>5} {:>7} {:>10}\n",
            c.rx_bytes,
            c.rx_frames,
            0,
            c.rx_drops,
            0,
            0,
            0,
            0,
            c.tx_bytes,
            c.tx_frames,
            0,
            c.tx_drops,
            0,
            0,
            0,
            0,
        ));
    }
    out.into_bytes()
}

fn gen_kthreads() -> Vec<u8> {
    let mut out = String::from("tid\tname\tstate\truntime_ticks\tstack_bytes\n");
    let list = crate::process::scheduler::SCHEDULER
        .try_lock()
        .map(|s| s.get_process_list())
        .unwrap_or_default();
    for info in list {
        let state = match info.state {
            crate::process::ProcessState::Ready => "ready",
            crate::process::ProcessState::Running => "running",
            crate::process::ProcessState::Blocked => "blocked",
            crate::process::ProcessState::Terminated => "terminated",
        };
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\n",
            info.pid, info.name, state, info.total_runtime, info.stack_size
        ));
    }
    out.into_bytes()
}

fn gen_gui() -> Vec<u8> {
    let mut out = String::from("pid\twindows\tqueued_events\n");
    for (pid, windows, events) in crate::userland::gui::ownership_snapshot() {
        out.push_str(&format!("{}\t{}\t{}\n", pid, windows, events));
    }
    out.into_bytes()
}

fn gen_sockets() -> Vec<u8> {
    let mut out = String::from("id\tproto\tstate\tlocal\tremote\n");
    for s in crate::net::socket_snapshot() {
        let l = s.local;
        let local = format!(
            "{}.{}.{}.{}:{}",
            l.address[0], l.address[1], l.address[2], l.address[3], l.port
        );
        let remote = match s.remote {
            Some(r) => format!(
                "{}.{}.{}.{}:{}",
                r.address[0], r.address[1], r.address[2], r.address[3], r.port
            ),
            None => String::from("-"),
        };
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\n",
            s.id, s.proto, s.state, local, remote
        ));
    }
    out.into_bytes()
}

fn gen_pid_stat(s: &Ring3Snapshot) -> Vec<u8> {
    // Linux /proc/<pid>/stat: 52 fields. Everything we don't track is
    // zero. Field map (1-based): 1 pid, 2 (comm), 3 state, 4 ppid,
    // 14 utime, 18 priority, 19 nice, 20 num_threads, 23 vsize,
    // 24 rss. BusyBox ps sscanf-parses through field 24.
    let mut out = format!("{} ({}) {} {}", s.pid, s.comm, s.state, s.ppid);
    // fields 5..=13: pgrp session tty_nr tpgid flags minflt cminflt
    // majflt cmajflt
    out.push_str(" 0 0 0 0 0 0 0 0 0");
    // 14 utime, 15 stime, 16 cutime, 17 cstime
    out.push_str(&format!(" {} 0 0 0", s.utime_ticks));
    // 18 priority, 19 nice, 20 num_threads, 21 itrealvalue, 22 starttime
    out.push_str(&format!(" 20 0 {} 0 0", s.threads));
    // 23 vsize (bytes), 24 rss (pages)
    out.push_str(&format!(" {} {}", s.vsize_bytes, s.rss_pages));
    // 25..=52: rsslim … exit_code
    for _ in 25..=52 {
        out.push_str(" 0");
    }
    out.push('\n');
    out.into_bytes()
}

fn gen_pid_status(s: &Ring3Snapshot) -> Vec<u8> {
    let state_line = match s.state {
        'R' => "R (running)",
        'Z' => "Z (zombie)",
        _ => "S (sleeping)",
    };
    let mut out = String::new();
    out.push_str(&format!("Name:\t{}\n", s.comm));
    out.push_str(&format!("State:\t{}\n", state_line));
    out.push_str(&format!("Pid:\t{}\n", s.pid));
    out.push_str(&format!("PPid:\t{}\n", s.ppid));
    out.push_str("Uid:\t0\t0\t0\t0\n");
    out.push_str("Gid:\t0\t0\t0\t0\n");
    out.push_str(&format!("VmSize:\t{:>8} kB\n", s.vsize_bytes / 1024));
    out.push_str(&format!("VmRSS:\t{:>8} kB\n", s.rss_pages * 4));
    out.push_str(&format!("Threads:\t{}\n", s.threads));
    out.into_bytes()
}

fn gen_pid_cmdline(s: &Ring3Snapshot) -> Vec<u8> {
    let mut out = Vec::new();
    for arg in &s.cmdline {
        out.extend_from_slice(arg.as_bytes());
        out.push(0);
    }
    out
}

fn gen_pid_statm(s: &Ring3Snapshot) -> Vec<u8> {
    // size resident shared text lib data dt — in pages.
    format!("{} {} 0 0 0 0 0\n", s.vsize_bytes / 4096, s.rss_pages).into_bytes()
}

/// Fill the Linux `struct sysinfo` fields BusyBox `free`/`uptime`
/// consume. Returns `(uptime_secs, totalram, freeram, sharedram,
/// procs)` with `mem_unit = 1` semantics (bytes).
pub fn sysinfo_snapshot() -> (i64, u64, u64, u64, u16) {
    let ticks = crate::arch::x86_64::interrupts::get_timer_ticks();
    let frames = crate::mm::memory::with_memory_mapper(|m| m.frame_stats());
    let (total, free, shared) = frames
        .map(|f| (f.total_usable * 4096, f.free * 4096, f.shared * 4096))
        .unwrap_or((0, 0, 0));
    let procs = {
        let g = PROCESS_TABLE.lock();
        g.by_pid
            .keys()
            .filter(|tid| g.thread_groups.get(tid).copied().unwrap_or(**tid) == **tid)
            .count()
            .saturating_sub(1) as u16
    };
    ((ticks / 100) as i64, total, free, shared, procs)
}

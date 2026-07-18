//! /proc reader for the task manager.
//!
//! The only module that knows /proc file formats. Reads every source
//! into an owned [`Snapshot`]; rate quantities (CPU %, bytes/sec) are
//! derived by the caller from two consecutive snapshots.

use alloc::string::String;
use alloc::vec::Vec;

/// One ring-3 process from `/proc/<pid>/stat`.
pub struct ProcRow {
    pub pid: u32,
    pub comm: String,
    pub state: char,
    pub utime_ticks: u64,
    pub rss_pages: u64,
}

/// One kernel thread from `/proc/agenticos/kthreads`.
pub struct KthreadRow {
    pub tid: u32,
    pub name: String,
    pub state: String,
    pub runtime_ticks: u64,
    pub stack_bytes: u64,
}

/// One socket from `/proc/agenticos/sockets`.
pub struct SocketRow {
    pub id: u64,
    pub proto: String,
    pub state: String,
    pub local: String,
    pub remote: String,
}

/// Monotonic USER_HZ counters from one `/proc/stat` CPU row.
#[derive(Clone, Copy, Default)]
pub struct CpuTimes {
    pub id: u32,
    pub user: u64,
    pub system: u64,
    pub idle: u64,
}

#[derive(Default)]
pub struct Snapshot {
    /// Monotonic 100 Hz ticks since boot.
    pub uptime_ticks: u64,
    pub cpu_total: CpuTimes,
    pub cpus: Vec<CpuTimes>,
    pub mem_total_kb: u64,
    pub mem_free_kb: u64,
    pub heap_total_kb: u64,
    pub heap_used_kb: u64,
    pub procs: Vec<ProcRow>,
    pub kthreads: Vec<KthreadRow>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_packets: u64,
    pub tx_packets: u64,
    pub sockets: Vec<SocketRow>,
}

/// Read a file to EOF through open/read/close. Returns an empty vec on
/// any failure — a missing /proc file degrades to an empty panel, not
/// a crash.
fn read_file(path: &str) -> Vec<u8> {
    let cpath = gui::c_path(path);
    let fd = runtime::openat(runtime::AT_FDCWD, &cpath, runtime::O_RDONLY, 0);
    if fd < 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        let n = runtime::read(fd as i32, &mut buf);
        if n <= 0 {
            break;
        }
        out.extend_from_slice(&buf[..n as usize]);
    }
    runtime::close(fd as i32);
    out
}

fn read_text(path: &str) -> String {
    String::from_utf8(read_file(path)).unwrap_or_default()
}

fn parse_u64(s: &str) -> u64 {
    s.trim().parse().unwrap_or(0)
}

/// Second whitespace-separated token of the line starting with `key`.
fn keyed_value(text: &str, key: &str) -> u64 {
    text.lines()
        .find(|l| l.starts_with(key))
        .and_then(|l| l.split_whitespace().nth(1))
        .map(parse_u64)
        .unwrap_or(0)
}

fn parse_uptime(text: &str) -> u64 {
    // "<secs>.<centis> <idle>.<centis>\n"
    let first = text.split_whitespace().next().unwrap_or("0.0");
    let mut parts = first.split('.');
    let secs = parse_u64(parts.next().unwrap_or("0"));
    let centis = parse_u64(parts.next().unwrap_or("0"));
    secs * 100 + centis
}

fn parse_pid_stat(text: &str) -> Option<ProcRow> {
    // "<pid> (<comm>) <state> <ppid> ... utime=field14 ... vsize=23 rss=24"
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let pid = parse_u64(&text[..open]) as u32;
    let comm = String::from(&text[open + 1..close]);
    let rest: Vec<&str> = text[close + 1..].split_whitespace().collect();
    let state = rest.first()?.chars().next().unwrap_or('?');
    Some(ProcRow {
        pid,
        comm,
        state,
        utime_ticks: rest.get(11).map(|s| parse_u64(s)).unwrap_or(0),
        rss_pages: rest.get(21).map(|s| parse_u64(s)).unwrap_or(0),
    })
}

fn parse_kthreads(text: &str) -> Vec<KthreadRow> {
    text.lines()
        .skip(1) // header
        .filter_map(|line| {
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() < 5 {
                return None;
            }
            Some(KthreadRow {
                tid: parse_u64(f[0]) as u32,
                name: String::from(f[1]),
                state: String::from(f[2]),
                runtime_ticks: parse_u64(f[3]),
                stack_bytes: parse_u64(f[4]),
            })
        })
        .collect()
}

fn parse_sockets(text: &str) -> Vec<SocketRow> {
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() < 5 {
                return None;
            }
            Some(SocketRow {
                id: parse_u64(f[0]),
                proto: String::from(f[1]),
                state: String::from(f[2]),
                local: String::from(f[3]),
                remote: String::from(f[4]),
            })
        })
        .collect()
}

fn parse_net_dev(snap: &mut Snapshot, text: &str) {
    for line in text.lines() {
        let Some(colon) = line.find(':') else {
            continue;
        };
        if !line[..colon].trim().starts_with("eth") {
            continue;
        }
        let f: Vec<&str> = line[colon + 1..].split_whitespace().collect();
        if f.len() >= 12 {
            snap.rx_bytes = parse_u64(f[0]);
            snap.rx_packets = parse_u64(f[1]);
            snap.tx_bytes = parse_u64(f[8]);
            snap.tx_packets = parse_u64(f[9]);
        }
        return;
    }
}

fn parse_cpu_stat(snap: &mut Snapshot, text: &str) {
    let mut aggregate_seen = false;
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let Some(label) = fields.next() else {
            continue;
        };
        if label != "cpu"
            && !label
                .strip_prefix("cpu")
                .is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
        {
            continue;
        }

        let user = fields.next().map(parse_u64).unwrap_or(0);
        let nice = fields.next().map(parse_u64).unwrap_or(0);
        let system = fields.next().map(parse_u64).unwrap_or(0);
        let idle = fields.next().map(parse_u64).unwrap_or(0);
        let mut times = CpuTimes {
            id: 0,
            user: user.saturating_add(nice),
            system,
            idle,
        };
        if label == "cpu" {
            snap.cpu_total = times;
            aggregate_seen = true;
        } else {
            times.id = parse_u64(label.trim_start_matches("cpu")) as u32;
            snap.cpus.push(times);
        }
    }

    // Older kernels exposed only the aggregate row. Keep a useful one-graph
    // fallback instead of leaving the Performance tab empty.
    if !aggregate_seen && !snap.cpus.is_empty() {
        snap.cpu_total = snap
            .cpus
            .iter()
            .fold(CpuTimes::default(), |total, cpu| CpuTimes {
                id: 0,
                user: total.user.saturating_add(cpu.user),
                system: total.system.saturating_add(cpu.system),
                idle: total.idle.saturating_add(cpu.idle),
            });
    } else if snap.cpus.is_empty() && aggregate_seen {
        snap.cpus.push(snap.cpu_total);
    }
}

/// Take one full snapshot of every /proc source the UI consumes.
pub fn sample() -> Snapshot {
    let mut snap = Snapshot {
        uptime_ticks: parse_uptime(&read_text("/proc/uptime")),
        ..Snapshot::default()
    };

    let stat = read_text("/proc/stat");
    parse_cpu_stat(&mut snap, &stat);

    let meminfo = read_text("/proc/meminfo");
    snap.mem_total_kb = keyed_value(&meminfo, "MemTotal:");
    snap.mem_free_kb = keyed_value(&meminfo, "MemFree:");
    snap.heap_total_kb = keyed_value(&meminfo, "KernelHeapTotal:");
    snap.heap_used_kb = keyed_value(&meminfo, "KernelHeapUsed:");

    // Enumerate /proc for numeric (PID) directories.
    if let Ok(entries) = gui::list_dir("/proc") {
        for entry in entries {
            if !entry.is_dir || !entry.name.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            let mut path = String::from("/proc/");
            path.push_str(&entry.name);
            path.push_str("/stat");
            if let Some(row) = parse_pid_stat(&read_text(&path)) {
                snap.procs.push(row);
            }
        }
    }

    snap.kthreads = parse_kthreads(&read_text("/proc/agenticos/kthreads"));
    parse_net_dev(&mut snap, &read_text("/proc/net/dev"));
    snap.sockets = parse_sockets(&read_text("/proc/agenticos/sockets"));
    snap
}

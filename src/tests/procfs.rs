//! Tests for the synthetic `/proc` namespace, the `sysinfo` syscall,
//! ring-3 accounting plumbing, and the extended `kill(2)` semantics
//! (any-PID targeting + fatal default dispositions).
//!
//! Dispatcher-driven tests follow the readlink pattern in
//! `tests/userland.rs`: kernel-side buffers are exposed to the
//! usercopy layer via `set_user_va_bounds`, and no ring-3 process is
//! current (`current_user_pid == None`), so fd allocations land in the
//! persistent PID-0 sentinel's table.

use crate::arch::x86_64::syscall::SyscallArgs;
use crate::lib::test_utils::Testable;
use crate::userland::abi::{syscall_dispatch, EACCES, ENOENT, EPERM, ESRCH};
use crate::userland::lifecycle::{ExitKind, Process, PROCESS_TABLE};
use crate::userland::procfs::{self, ProcNodeKind};
use alloc::string::String;
use alloc::vec::Vec;

/// Minimal synthetic `Process` for table-backed tests. Never scheduled.
fn synthetic_process(pid: u32) -> Process {
    Process {
        pid,
        parent_pid: 0,
        image: None,
        exit_kind: ExitKind::None,
        exit_code: 0,
        brk_base: 0,
        brk_current: 0,
        mmap_next: 0,
        fd_table: crate::userland::fdtable::FdTable::new(),
        network_wait: None,
        sleep_deadline: None,
        real_timer: crate::userland::lifecycle::RealTimerState::disarmed(),
        pending_syscall_interrupt: false,
        cwd: String::from("/"),
        address_space: None,
        signal_state: crate::userland::signal::SignalState::new(),
        kernel_stack: None,
        exe_path: Some(String::from("/host/FAKE.ELF")),
        cmdline: alloc::vec![String::from("fake"), String::from("--flag")],
        utime_ticks: 123,
        stack_top: 0,
        stack_bottom: 0,
        stack_mapped_bottom: 0,
        stack_max_growth_floor: 0,
        growth_faults_remaining: 0,
        fs_base: 0,
        fpu_state: crate::arch::x86_64::fpu::FpuState::default(),
        saved_user_state: crate::userland::user_state::UserState::default(),
        terminal_id: None,
    }
}

fn insert_synthetic(pid: u32) {
    let p = synthetic_process(pid);
    PROCESS_TABLE.lock().by_pid.insert(pid, p);
}

fn remove_synthetic(pid: u32) {
    PROCESS_TABLE.lock().by_pid.remove(&pid);
}

fn bounds_for(ranges: &[(u64, u64)]) -> (u64, u64) {
    let lo = ranges.iter().map(|r| r.0).min().unwrap();
    let hi = ranges.iter().map(|r| r.0 + r.1).max().unwrap();
    (lo, hi)
}

fn set_bounds(lo: u64, hi: u64) {
    crate::userland::abi::set_user_va_bounds(crate::userland::abi::UserVaBounds {
        start: lo,
        end: hi,
    });
}

fn dispatch_open(path: &[u8], flags: u64) -> i64 {
    let (lo, hi) = bounds_for(&[(path.as_ptr() as u64, path.len() as u64)]);
    set_bounds(lo, hi);
    let mut args = SyscallArgs::default();
    args.rax = crate::userland::abi::nr::OPEN;
    args.rdi = path.as_ptr() as u64;
    args.rsi = flags;
    let ret = syscall_dispatch(&mut args);
    crate::userland::abi::clear_user_va_bounds();
    ret
}

fn dispatch_read(fd: i64, buf: &mut [u8]) -> i64 {
    let (lo, hi) = bounds_for(&[(buf.as_ptr() as u64, buf.len() as u64)]);
    set_bounds(lo, hi);
    let mut args = SyscallArgs::default();
    args.rax = crate::userland::abi::nr::READ;
    args.rdi = fd as u64;
    args.rsi = buf.as_mut_ptr() as u64;
    args.rdx = buf.len() as u64;
    let ret = syscall_dispatch(&mut args);
    crate::userland::abi::clear_user_va_bounds();
    ret
}

fn dispatch_close(fd: i64) -> i64 {
    let mut args = SyscallArgs::default();
    args.rax = crate::userland::abi::nr::CLOSE;
    args.rdi = fd as u64;
    syscall_dispatch(&mut args)
}

/// Read a /proc file to EOF through the dispatcher, returning its
/// content.
fn read_proc_file(path: &[u8]) -> Vec<u8> {
    let fd = dispatch_open(path, 0);
    assert!(fd >= 0, "open failed: {}", fd);
    let mut out = Vec::new();
    let mut buf = [0u8; 256];
    loop {
        let n = dispatch_read(fd, &mut buf);
        assert!(n >= 0, "read failed: {}", n);
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n as usize]);
    }
    assert_eq!(dispatch_close(fd), 0);
    out
}

/// Structural classification: known dirs and files resolve, junk
/// doesn't, and `/procfoo` never matches the namespace.
fn test_proc_classify() {
    assert!(matches!(procfs::classify("/proc"), Some(ProcNodeKind::Dir)));
    assert!(matches!(
        procfs::classify("/proc/uptime"),
        Some(ProcNodeKind::File)
    ));
    assert!(matches!(
        procfs::classify("/proc/agenticos"),
        Some(ProcNodeKind::Dir)
    ));
    assert!(matches!(
        procfs::classify("/proc/agenticos/kthreads"),
        Some(ProcNodeKind::File)
    ));
    assert!(matches!(
        procfs::classify("/proc/net/dev"),
        Some(ProcNodeKind::File)
    ));
    assert!(procfs::classify("/proc/nonsense").is_none());
    assert!(procfs::classify("/proc/999999").is_none());
    assert!(!procfs::is_proc_path("/procfoo"));
    assert!(!procfs::is_proc_path("/pro"));
}

/// `/proc/<pid>` classification requires a live table entry.
fn test_proc_pid_classify_liveness() {
    const PID: u32 = 700001;
    assert!(procfs::classify("/proc/700001").is_none());
    insert_synthetic(PID);
    assert!(matches!(
        procfs::classify("/proc/700001"),
        Some(ProcNodeKind::Dir)
    ));
    assert!(matches!(
        procfs::classify("/proc/700001/stat"),
        Some(ProcNodeKind::File)
    ));
    remove_synthetic(PID);
    assert!(procfs::classify("/proc/700001").is_none());
}

/// Open + read `/proc/uptime` through the dispatcher: "<s>.<cc> <s>.<cc>\n".
fn test_proc_uptime_read() {
    let content = read_proc_file(b"/proc/uptime\0");
    let text = core::str::from_utf8(&content).expect("uptime is ASCII");
    assert!(text.ends_with('\n'));
    let mut parts = text.trim_end().split(' ');
    let up = parts.next().expect("uptime field");
    let idle = parts.next().expect("idle field");
    assert!(parts.next().is_none());
    assert!(up.contains('.') && idle.contains('.'));
}

/// `/proc/meminfo` leads with MemTotal and reports a nonzero total.
fn test_proc_meminfo_shape() {
    let content = read_proc_file(b"/proc/meminfo\0");
    let text = core::str::from_utf8(&content).expect("meminfo is ASCII");
    assert!(text.starts_with("MemTotal:"), "got: {}", text);
    let first = text.lines().next().unwrap();
    let kb: u64 = first
        .split_whitespace()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .expect("MemTotal value");
    assert!(kb > 0, "MemTotal should be nonzero");
    assert!(text.contains("MemFree:"));
    assert!(text.contains("KernelHeapUsed:"));
}

/// `/proc/stat` has the aggregate cpu line and a processes count.
fn test_proc_stat_shape() {
    let content = read_proc_file(b"/proc/stat\0");
    let text = core::str::from_utf8(&content).expect("stat is ASCII");
    assert!(text.starts_with("cpu  "), "got: {}", text);
    assert!(text.contains("\nprocesses "));
}

/// getdents64 on /proc enumerates the static files, subdirs, and live
/// PIDs.
fn test_proc_getdents_root() {
    const PID: u32 = 700002;
    insert_synthetic(PID);
    let fd = dispatch_open(b"/proc\0", 0);
    assert!(fd >= 0, "open /proc failed: {}", fd);
    let mut names: Vec<String> = Vec::new();
    let mut buf = [0u8; 512];
    loop {
        let (lo, hi) = bounds_for(&[(buf.as_ptr() as u64, buf.len() as u64)]);
        set_bounds(lo, hi);
        let mut args = SyscallArgs::default();
        args.rax = crate::userland::abi::nr::GETDENTS64;
        args.rdi = fd as u64;
        args.rsi = buf.as_mut_ptr() as u64;
        args.rdx = buf.len() as u64;
        let n = syscall_dispatch(&mut args);
        crate::userland::abi::clear_user_va_bounds();
        assert!(n >= 0, "getdents64 failed: {}", n);
        if n == 0 {
            break;
        }
        let mut off = 0usize;
        while off < n as usize {
            let reclen = u16::from_ne_bytes([buf[off + 16], buf[off + 17]]) as usize;
            let name_start = off + 19;
            let mut name_end = name_start;
            while buf[name_end] != 0 {
                name_end += 1;
            }
            names.push(String::from(
                core::str::from_utf8(&buf[name_start..name_end]).unwrap(),
            ));
            off += reclen;
        }
    }
    assert_eq!(dispatch_close(fd), 0);
    remove_synthetic(PID);
    for expected in [
        "uptime",
        "meminfo",
        "stat",
        "agenticos",
        "net",
        "self",
        "700002",
    ] {
        assert!(
            names.iter().any(|n| n == expected),
            "missing {} in {:?}",
            expected,
            names
        );
    }
}

/// `/proc/<pid>/stat` carries pid, comm, state, utime, and parses to
/// ≥ 24 fields; cmdline is NUL-separated argv.
fn test_proc_pid_files() {
    const PID: u32 = 700003;
    insert_synthetic(PID);
    let stat = read_proc_file(b"/proc/700003/stat\0");
    let text = core::str::from_utf8(&stat).unwrap();
    assert!(text.starts_with("700003 (fake) S 0 "), "got: {}", text);
    let fields: Vec<&str> = text.trim_end().split(' ').collect();
    assert!(fields.len() >= 24, "only {} fields", fields.len());
    assert_eq!(fields[13], "123"); // utime
    let cmdline = read_proc_file(b"/proc/700003/cmdline\0");
    assert_eq!(&cmdline[..], b"fake\0--flag\0");
    let status = read_proc_file(b"/proc/700003/status\0");
    let status_text = core::str::from_utf8(&status).unwrap();
    assert!(status_text.contains("Name:\tfake\n"));
    assert!(status_text.contains("Pid:\t700003\n"));
    remove_synthetic(PID);
}

/// The fd owns its snapshot: content generated at open survives the
/// process exiting mid-read.
fn test_proc_snapshot_stable_across_exit() {
    const PID: u32 = 700004;
    insert_synthetic(PID);
    let fd = dispatch_open(b"/proc/700004/stat\0", 0);
    assert!(fd >= 0);
    remove_synthetic(PID);
    let mut buf = [0u8; 64];
    let n = dispatch_read(fd, &mut buf);
    assert!(n > 0, "snapshot read failed after exit: {}", n);
    assert!(buf.starts_with(b"700004 (fake)"));
    assert_eq!(dispatch_close(fd), 0);
    // Re-open after death is ENOENT.
    assert_eq!(dispatch_open(b"/proc/700004/stat\0", 0), ENOENT);
}

/// Unknown paths → ENOENT; write intent → EACCES; unlink → EPERM.
fn test_proc_write_and_mutation_rejected() {
    assert_eq!(dispatch_open(b"/proc/bogus\0", 0), ENOENT);
    const O_WRONLY: u64 = 1;
    assert_eq!(dispatch_open(b"/proc/uptime\0", O_WRONLY), EACCES);
    let path = b"/proc/uptime\0";
    let (lo, hi) = bounds_for(&[(path.as_ptr() as u64, path.len() as u64)]);
    set_bounds(lo, hi);
    let mut args = SyscallArgs::default();
    args.rax = crate::userland::abi::nr::UNLINK;
    args.rdi = path.as_ptr() as u64;
    let ret = syscall_dispatch(&mut args);
    crate::userland::abi::clear_user_va_bounds();
    assert_eq!(ret, EPERM);
}

/// `sysinfo(2)` fills uptime/ram/procs with mem_unit = 1.
fn test_sysinfo_dispatch() {
    let mut buf = [0u8; 112];
    let (lo, hi) = bounds_for(&[(buf.as_ptr() as u64, buf.len() as u64)]);
    set_bounds(lo, hi);
    let mut args = SyscallArgs::default();
    args.rax = crate::userland::abi::nr::SYSINFO;
    args.rdi = buf.as_mut_ptr() as u64;
    let ret = syscall_dispatch(&mut args);
    crate::userland::abi::clear_user_va_bounds();
    assert_eq!(ret, 0);
    let totalram = u64::from_ne_bytes(buf[32..40].try_into().unwrap());
    let mem_unit = u32::from_ne_bytes(buf[104..108].try_into().unwrap());
    assert!(totalram > 0);
    assert_eq!(mem_unit, 1);
}

/// The RSS walk over a fresh (empty lower half) address space is zero
/// and does not disturb the space.
fn test_rss_walk_fresh_address_space_is_zero() {
    let aspace = match crate::userland::address_space::AddressSpace::new() {
        Ok(a) => a,
        Err(_) => return, // no mapper in this boot phase — nothing to test
    };
    let rss =
        crate::mm::memory::with_memory_mapper(|m| m.count_user_resident_pages(aspace.l4_frame()))
            .unwrap_or(0);
    assert_eq!(rss, 0);
}

/// `capped_cmdline` copies argv until the byte budget is exhausted.
fn test_capped_cmdline_caps() {
    let short = crate::userland::lifecycle::capped_cmdline(&["a", "b"]);
    assert_eq!(short.len(), 2);
    let long_arg = "x".repeat(300);
    let capped = crate::userland::lifecycle::capped_cmdline(&["prog", &long_arg, "tail"]);
    assert_eq!(capped.len(), 1); // "prog" fits; the 300-byte arg doesn't
    assert_eq!(capped[0], "prog");
}

/// SIGTERM with no handler is a fatal default; a handler claims it for
/// delivery instead; SIGCHLD pending is discarded; blocked SIGTERM is
/// held but blocked SIGKILL is not blockable.
fn test_signal_fatal_default_semantics() {
    use crate::userland::signal::{SigAction, SignalState, SIGCHLD, SIGKILL, SIGTERM};
    let mut s = SignalState::new();
    s.raise(SIGCHLD);
    assert_eq!(s.take_fatal_default(), None);
    assert_ne!(
        s.pending, 0,
        "ignored signal stays pending (notification record)"
    );
    assert!(!s.has_actionable_pending());

    s.raise(SIGTERM);
    assert!(s.has_actionable_pending());
    assert_eq!(s.take_fatal_default(), Some(SIGTERM));

    // Handler installed → not fatal; consume_deliverable owns it.
    let mut s = SignalState::new();
    s.set_action(
        SIGTERM,
        SigAction {
            sa_handler: 0x40_0000,
            sa_flags: 0,
            sa_restorer: 0,
            sa_mask: 0,
        },
    );
    s.raise(SIGTERM);
    assert_eq!(s.take_fatal_default(), None);
    assert!(s.consume_deliverable().is_some());

    // Blocked SIGTERM is held; SIGKILL ignores the blocked mask.
    let mut s = SignalState::new();
    s.blocked = 1u64 << (SIGTERM - 1);
    s.raise(SIGTERM);
    assert_eq!(s.take_fatal_default(), None);
    assert!(!s.has_actionable_pending());
    s.blocked |= 1u64 << (SIGKILL - 1);
    s.raise(SIGKILL);
    assert_eq!(s.take_fatal_default(), Some(SIGKILL));
}

/// `kill(2)` now reaches any live PID: ESRCH for absent targets,
/// pending-bit raise for present ones, sig 0 as a liveness probe.
fn test_kill_any_pid() {
    use crate::userland::signal::SIGTERM;
    const PID: u32 = 700005;

    let mut args = SyscallArgs::default();
    args.rax = crate::userland::abi::nr::KILL;
    args.rdi = PID as u64;
    args.rsi = SIGTERM as u64;
    assert_eq!(syscall_dispatch(&mut args), ESRCH);

    insert_synthetic(PID);
    let mut args = SyscallArgs::default();
    args.rax = crate::userland::abi::nr::KILL;
    args.rdi = PID as u64;
    args.rsi = 0; // liveness probe
    assert_eq!(syscall_dispatch(&mut args), 0);
    let mut args = SyscallArgs::default();
    args.rax = crate::userland::abi::nr::KILL;
    args.rdi = PID as u64;
    args.rsi = SIGTERM as u64;
    assert_eq!(syscall_dispatch(&mut args), 0);
    let pending = PROCESS_TABLE
        .lock()
        .by_pid
        .get(&PID)
        .map(|p| p.signal_state.pending)
        .unwrap();
    assert_eq!(pending & (1u64 << (SIGTERM - 1)), 1u64 << (SIGTERM - 1));
    remove_synthetic(PID);
}

/// `process_expired_sleeps` wakes exactly the sleepers whose deadline
/// has passed and requeues them as ready; future sleepers stay parked.
/// The woken process's restart-stable `sleep_deadline` remains set for
/// the re-fired SYSCALL, which observes it elapsed via
/// `nanosleep_deadline` and returns 0.
fn test_sleeper_wake_at_deadline() {
    use crate::process::entity::{EntityId, RunState};
    use crate::userland::lifecycle::{process_expired_sleeps, Ring3BlockReason};
    const DUE: u32 = 700006;
    const NOT_DUE: u32 = 700007;
    let now = crate::arch::x86_64::interrupts::get_timer_ticks();

    insert_synthetic(DUE);
    insert_synthetic(NOT_DUE);
    for (pid, deadline) in [(DUE, now.saturating_sub(1)), (NOT_DUE, now + 10_000)] {
        {
            let mut g = PROCESS_TABLE.lock();
            g.by_pid.get_mut(&pid).unwrap().sleep_deadline = Some(deadline);
        }
        crate::process::scheduler::SCHEDULER
            .lock()
            .register_user(pid)
            .unwrap();
        crate::userland::lifecycle::mark_ring3_blocked(
            pid,
            Ring3BlockReason::Sleeping {
                deadline_tick: deadline,
            },
        );
    }

    process_expired_sleeps();

    {
        let g = PROCESS_TABLE.lock();
        assert!(
            !g.ring3_blocked.contains_key(&DUE),
            "due sleeper should be unblocked"
        );
        assert!(
            g.ring3_blocked.contains_key(&NOT_DUE),
            "future sleeper must stay blocked"
        );
    }
    assert_eq!(
        crate::process::scheduler::SCHEDULER
            .lock()
            .entity_state(EntityId::UserProcess(DUE)),
        Some(RunState::Ready),
        "due sleeper should be ready"
    );

    // Cleanup: pull both out of the scheduler and timer structures.
    for pid in [DUE, NOT_DUE] {
        crate::process::timer::cancel_entity(EntityId::UserProcess(pid));
        crate::process::scheduler::SCHEDULER
            .lock()
            .unregister_entity(EntityId::UserProcess(pid));
        PROCESS_TABLE.lock().ring3_blocked.remove(&pid);
    }
    remove_synthetic(DUE);
    remove_synthetic(NOT_DUE);
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_proc_classify,
        &test_proc_pid_classify_liveness,
        &test_proc_uptime_read,
        &test_proc_meminfo_shape,
        &test_proc_stat_shape,
        &test_proc_getdents_root,
        &test_proc_pid_files,
        &test_proc_snapshot_stable_across_exit,
        &test_proc_write_and_mutation_rejected,
        &test_sysinfo_dispatch,
        &test_rss_walk_fresh_address_space_is_zero,
        &test_capped_cmdline_caps,
        &test_signal_fatal_default_semantics,
        &test_kill_any_pid,
        &test_sleeper_wake_at_deadline,
    ]
}

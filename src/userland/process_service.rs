//! Non-blocking kernel-to-user launch service and detached-process reaper.
//!
//! Kernel callers enqueue owned launch requests and return immediately. One
//! persistent kernel thread performs the CR3-sensitive ELF setup transaction,
//! publishes the resulting ring-3 PID, and later tears that process down after
//! its scheduler entity has stopped. Fork children are deliberately absent
//! from this registry: their user parent retains POSIX `wait4` ownership.

use crate::arch::x86_64::interrupt_guard::InterruptMutex;
use crate::process::BlockReason;
use crate::userland::lifecycle::ExitKind;
use crate::window::WindowId;
use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const MAX_PENDING_LAUNCHES: usize = 32;

#[cfg(feature = "test")]
pub const ZSH_HOST_PATH: &str = "/host/ZSH.ELF";

/// Baseline environment for kernel-launched user programs.
pub const DEFAULT_USER_ENV: [&str; 8] = [
    "PATH=/bin:/host",
    "HOME=/root",
    "USER=root",
    "LOGNAME=root",
    "SHELL=/bin/zsh",
    "TERM=xterm-256color",
    "COLORTERM=truecolor",
    "LANG=C.UTF-8",
];

/// Stable identity returned before a launch request has a ring-3 PID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct LaunchId(pub u64);

/// Final result delivered outside every process/scheduler/service lock.
pub enum LaunchOutcome {
    Failed {
        #[expect(
            dead_code,
            reason = "launch identity is part of the completion contract"
        )]
        id: LaunchId,
        error: String,
    },
    Exited {
        #[expect(
            dead_code,
            reason = "launch identity is part of the completion contract"
        )]
        id: LaunchId,
        pid: u32,
        kind: ExitKind,
        code: i64,
    },
}

pub type CompletionHandler = Box<dyn FnOnce(LaunchOutcome) + Send>;

/// Owned description of a program the process service should start.
pub struct LaunchSpec {
    pub path: String,
    pub argv: Vec<String>,
    pub envp: Vec<String>,
    pub cwd: String,
    pub terminal_id: Option<WindowId>,
    pub completion: Option<CompletionHandler>,
}

impl LaunchSpec {
    pub fn new(path: &str, argv: &[&str], envp: &[&str]) -> Self {
        Self {
            path: String::from(path),
            argv: argv.iter().map(|value| String::from(*value)).collect(),
            envp: envp.iter().map(|value| String::from(*value)).collect(),
            cwd: String::from("/host"),
            terminal_id: None,
            completion: None,
        }
    }

    #[cfg_attr(
        not(feature = "test"),
        expect(dead_code, reason = "explicit-terminal launch regression API")
    )]
    pub fn with_terminal(mut self, terminal_id: WindowId) -> Self {
        self.terminal_id = Some(terminal_id);
        self
    }

    #[cfg_attr(
        not(feature = "test"),
        expect(dead_code, reason = "LaunchSpec builder retained for launchers/tests")
    )]
    pub fn with_cwd(mut self, cwd: &str) -> Self {
        self.cwd = String::from(cwd);
        self
    }

    pub fn on_complete(mut self, completion: CompletionHandler) -> Self {
        self.completion = Some(completion);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitError {
    NotStarted,
    QueueFull,
}

impl fmt::Display for SubmitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotStarted => write!(f, "process service is not started"),
            Self::QueueFull => write!(f, "process launch queue is full"),
        }
    }
}

struct LaunchRequest {
    id: LaunchId,
    path: String,
    argv: Vec<String>,
    envp: Vec<String>,
    cwd: String,
    terminal_id: Option<WindowId>,
}

struct LaunchRecord {
    pid: Option<u32>,
    completion: Option<CompletionHandler>,
}

struct ServiceState {
    initialized: bool,
    next_id: u64,
    queue: VecDeque<LaunchRequest>,
    records: BTreeMap<LaunchId, LaunchRecord>,
}

impl ServiceState {
    const fn new() -> Self {
        Self {
            initialized: false,
            next_id: 1,
            queue: VecDeque::new(),
            records: BTreeMap::new(),
        }
    }

    fn enqueue(&mut self, spec: LaunchSpec) -> Result<LaunchId, SubmitError> {
        if self.queue.len() >= MAX_PENDING_LAUNCHES {
            return Err(SubmitError::QueueFull);
        }
        let id = LaunchId(self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        self.records.insert(
            id,
            LaunchRecord {
                pid: None,
                completion: spec.completion,
            },
        );
        self.queue.push_back(LaunchRequest {
            id,
            path: spec.path,
            argv: spec.argv,
            envp: spec.envp,
            cwd: spec.cwd,
            terminal_id: spec.terminal_id,
        });
        Ok(id)
    }
}

static STATE: InterruptMutex<ServiceState> = InterruptMutex::new(ServiceState::new());
static SERVICE_PID: AtomicU64 = AtomicU64::new(0);
static WORK_PENDING: AtomicBool = AtomicBool::new(false);

/// Start the single persistent launch/reap worker.
pub fn start() {
    if SERVICE_PID.load(Ordering::Acquire) != 0 {
        return;
    }
    {
        let mut state = STATE.lock();
        if !state.initialized {
            state
                .queue
                .try_reserve(MAX_PENDING_LAUNCHES)
                .expect("process-service queue reservation failed");
            state.initialized = true;
        }
    }
    let pid = crate::process::spawn_process(String::from("process-service"), None, service_main);
    SERVICE_PID.store(pid as u64, Ordering::Release);
}

/// Enqueue a launch and return before ELF I/O or process setup begins.
pub fn submit(spec: LaunchSpec) -> Result<LaunchId, SubmitError> {
    let id = {
        let mut state = STATE.lock();
        if !state.initialized || SERVICE_PID.load(Ordering::Acquire) == 0 {
            return Err(SubmitError::NotStarted);
        }
        state.enqueue(spec)?
    };
    publish_work();
    Ok(id)
}

/// Called from the divergent ring-3 exit path. The Process entry remains the
/// durable work item, so this path performs no allocation.
pub fn notify_process_exit(pid: u32) {
    let _ = pid;
    // Every exit may orphan descendants even when this exact PID remains
    // owned by a live user's wait4 path.
    publish_work();
}

fn publish_work() {
    WORK_PENDING.store(true, Ordering::Release);
    let pid = SERVICE_PID.load(Ordering::Acquire) as u32;
    if pid != 0 {
        crate::process::scheduler::SCHEDULER.lock().wake(pid);
    }
}

fn service_main() {
    loop {
        WORK_PENDING.store(false, Ordering::Release);

        while adopt_one_orphan() {}
        while crate::userland::lifecycle::reap_one_dead_thread() {}
        while reap_one() {}

        // Keep the service-state guard out of `process_request`: that path
        // publishes the PID by taking STATE again.
        // A lock temporary in the `if let` scrutinee otherwise lives through
        // the branch and self-deadlocks with interrupts disabled.
        let request = { STATE.lock().queue.pop_front() };
        if let Some(request) = request {
            process_request(request);
            crate::process::yield_current();
            continue;
        }

        let _ = crate::process::park_current_if(BlockReason::WaitingForProcessWork, || {
            !WORK_PENDING.swap(false, Ordering::AcqRel)
        });
    }
}

fn adopt_one_orphan() -> bool {
    let Some(pid) = crate::userland::lifecycle::adopt_one_orphan_for_kernel_reaper() else {
        return false;
    };
    let mut state = STATE.lock();
    let id = LaunchId(state.next_id);
    state.next_id = state.next_id.saturating_add(1);
    state.records.insert(
        id,
        LaunchRecord {
            pid: Some(pid),
            completion: None,
        },
    );
    crate::debug_info!(
        "process-service: adopted orphan ring-3 pid={} as {:?}",
        pid,
        id
    );
    true
}

fn process_request(request: LaunchRequest) {
    let argv_refs: Vec<&str> = request.argv.iter().map(String::as_str).collect();
    let envp_refs: Vec<&str> = request.envp.iter().map(String::as_str).collect();
    let prepared = crate::userland::launcher::prepare_user_binary_unstarted(
        &request.path,
        &argv_refs,
        &envp_refs,
        request.terminal_id,
    );

    match prepared {
        Ok(pid) => {
            let _ = crate::userland::lifecycle::with_process(pid, |process| {
                process.cwd = request.cwd;
            });
            let published = {
                let mut state = STATE.lock();
                match state.records.get_mut(&request.id) {
                    Some(record) => {
                        record.pid = Some(pid);
                        true
                    }
                    None => false,
                }
            };
            if !published {
                drop(crate::userland::lifecycle::remove_process(pid));
                return;
            }
            crate::userland::lifecycle::mark_ring3_ready(pid);
            crate::debug_info!(
                "process-service: launch {:?} registered ring-3 pid={} ({})",
                request.id,
                pid,
                request.path
            );
        }
        Err(error) => {
            let completion = STATE
                .lock()
                .records
                .remove(&request.id)
                .and_then(|mut record| record.completion.take());
            crate::debug_error!(
                "process-service: launch {:?} failed for {}: {}",
                request.id,
                request.path,
                error
            );
            if let Some(completion) = completion {
                completion(LaunchOutcome::Failed {
                    id: request.id,
                    error,
                });
            }
        }
    }
}

/// Reap one exited or externally-removed detached process. Returns whether a
/// record was consumed so the service can drain all available work.
fn reap_one() -> bool {
    let candidate = {
        let state = STATE.lock();
        state.records.iter().find_map(|(id, record)| {
            let pid = record.pid?;
            let status = crate::userland::lifecycle::with_process(pid, |process| {
                if matches!(process.exit_kind, ExitKind::None) {
                    None
                } else {
                    Some((process.exit_kind, process.exit_code))
                }
            });
            status.flatten().map(|(kind, code)| (*id, pid, kind, code))
        })
    };
    let Some((id, pid, kind, code)) = candidate else {
        return false;
    };

    let process = crate::userland::lifecycle::remove_process(pid);
    drop(process);
    let completion = STATE
        .lock()
        .records
        .remove(&id)
        .and_then(|mut record| record.completion.take());
    if let Some(completion) = completion {
        completion(LaunchOutcome::Exited {
            id,
            pid,
            kind,
            code,
        });
    }
    true
}

#[cfg(feature = "test")]
fn test_queue_is_bounded_and_ids_are_stable() {
    let mut state = ServiceState::new();
    state.initialized = true;
    for index in 0..MAX_PENDING_LAUNCHES {
        let spec = LaunchSpec::new("/host/TEST.ELF", &["test"], &[]);
        let id = state.enqueue(spec).expect("queue slot");
        assert_eq!(id, LaunchId(index as u64 + 1));
    }
    let overflow = state.enqueue(LaunchSpec::new("/host/TEST.ELF", &["test"], &[]));
    assert_eq!(overflow, Err(SubmitError::QueueFull));
    assert_eq!(state.queue.len(), MAX_PENDING_LAUNCHES);
    assert_eq!(state.records.len(), MAX_PENDING_LAUNCHES);
}

#[cfg(feature = "test")]
fn test_launch_spec_owns_inputs_and_terminal() {
    let terminal = WindowId(0x1234);
    let spec = LaunchSpec::new("/host/APP.ELF", &["app", "--flag"], &["PATH=/bin"])
        .with_terminal(terminal)
        .with_cwd("/data");
    assert_eq!(spec.path, "/host/APP.ELF");
    assert_eq!(spec.argv, ["app", "--flag"]);
    assert_eq!(spec.envp, ["PATH=/bin"]);
    assert_eq!(spec.cwd, "/data");
    assert_eq!(spec.terminal_id, Some(terminal));
}

#[cfg(feature = "test")]
pub fn process_service_tests() -> &'static [&'static dyn crate::lib::test_utils::Testable] {
    &[
        &test_queue_is_bounded_and_ids_are_stable,
        &test_launch_spec_owns_inputs_and_terminal,
    ]
}

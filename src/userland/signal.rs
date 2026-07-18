//! Per-process signal state.
//!
//! Phase 5 PR-B foundation. The kernel tracks each process's signal
//! actions (handlers), blocked-signal mask, and pending-signal bitmap.
//! `rt_sigaction`, `rt_sigprocmask`, and `kill` operate on this state.
//!
//! **Actual handler invocation is not in this PR.** When a signal is
//! pending, nothing happens — the handler isn't called and the
//! default action isn't taken. zsh and similar shells can run on top
//! of this because they don't depend on async-signal delivery in our
//! synchronous-fork model: `waitpid` replaces SIGCHLD-driven reaping,
//! and zle handles Ctrl-C as a stdin byte rather than as a signal.
//! Real delivery (signal frames on user stack + sigreturn) lands in
//! PR-B2.

/// Number of signals tracked. Linux defines 64 (1..=64); we store
/// actions[0] unused so the index matches the signal number directly.
/// The actions table is therefore sized `NSIG + 1` to make index `NSIG`
/// (SIGRTMAX) valid.
pub const NSIG: usize = 64;

// ---- well-known signal numbers (subset Linux exposes) ----
#[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
pub const SIGHUP: i32 = 1;
pub const SIGILL: i32 = 4;
pub const SIGFPE: i32 = 8;
pub const SIGKILL: i32 = 9;
#[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
pub const SIGUSR1: i32 = 10;
pub const SIGSEGV: i32 = 11;
#[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
pub const SIGUSR2: i32 = 12;
pub const SIGCHLD: i32 = 17;
pub const SIGBUS: i32 = 7;
pub const SIGSTOP: i32 = 19;
pub const SIGWINCH: i32 = 28;

// ---- well-known sa_handler sentinels ----
pub const SIG_DFL: u64 = 0;
pub const SIG_IGN: u64 = 1;

// ---- rt_sigprocmask `how` arg ----
pub const SIG_BLOCK: i32 = 0;
pub const SIG_UNBLOCK: i32 = 1;
pub const SIG_SETMASK: i32 = 2;

/// Linux x86-64 `struct sigaction` layout. 32 bytes.
#[repr(C)]
#[derive(Default, Clone, Copy, Debug)]
pub struct SigAction {
    /// Function pointer to the handler, or `SIG_DFL` (0) / `SIG_IGN` (1).
    pub sa_handler: u64,
    /// `SA_RESTART`, `SA_SIGINFO`, `SA_RESTORER`, etc.
    pub sa_flags: u64,
    /// Address of a small user-mode trampoline that calls
    /// `rt_sigreturn`. musl always sets this; we'll need it once
    /// PR-B2 wires actual delivery.
    pub sa_restorer: u64,
    /// Signal mask applied while the handler runs. Single u64 covers
    /// the 64 possible signals.
    pub sa_mask: u64,
}
const _SIGACTION_SIZE: () = assert!(core::mem::size_of::<SigAction>() == 32);

/// Per-process signal state. The 64-entry actions table is heap-
/// allocated (`Option<Box<…>>`) to keep the inline `Process` size
/// small — moves through `swap_current_process` would otherwise push
/// a 2 KiB blob through the kernel stack on every fork.
pub struct SignalState {
    /// Lazy actions table. `None` means "all actions default to
    /// `SIG_DFL`"; the table is materialized on first `set_action` /
    /// `action` call that actually needs custom dispositions.
    pub actions: Option<alloc::boxed::Box<[SigAction; NSIG + 1]>>,
    /// Blocked-signal mask. Bit `i-1` set means signal `i` is
    /// blocked from delivery (not from being recorded as pending).
    pub blocked: u64,
    /// Pending-signal bitmap. Set by `kill`, by VINTR/VQUIT in the
    /// terminal layer, and by child exit (SIGCHLD).
    pub pending: u64,
}

impl SignalState {
    pub const fn new() -> Self {
        Self {
            actions: None,
            blocked: 0,
            pending: 0,
        }
    }

    fn ensure_actions(&mut self) -> &mut [SigAction; NSIG + 1] {
        if self.actions.is_none() {
            self.actions = Some(alloc::boxed::Box::new([SigAction {
                sa_handler: SIG_DFL,
                sa_flags: 0,
                sa_restorer: 0,
                sa_mask: 0,
            }; NSIG + 1]));
        }
        self.actions.as_deref_mut().unwrap()
    }

    /// Clone the signal state (for fork). Custom rather than `derive`
    /// because we want to deep-copy the heap-backed actions table.
    pub fn fork_clone(&self) -> Self {
        Self {
            actions: self.actions.as_ref().map(|a| alloc::boxed::Box::new(**a)),
            blocked: self.blocked,
            pending: 0, // POSIX: pending signals not inherited across fork
        }
    }

    /// Mark `sig` as pending. Silently ignores invalid signal numbers
    /// rather than panicking — kernel-side callers should rarely hit
    /// invalid sig numbers, but defensive treatment matches Linux.
    pub fn raise(&mut self, sig: i32) {
        if sig < 1 || (sig as usize) > NSIG {
            return;
        }
        self.pending |= 1u64 << (sig - 1);
    }



    /// Set the action for `sig`, returning the previous one. SIGKILL
    /// and SIGSTOP cannot have their disposition changed (POSIX
    /// guarantee), so attempts return the existing action unchanged.
    pub fn set_action(&mut self, sig: i32, action: SigAction) -> Option<SigAction> {
        if sig < 1 || (sig as usize) > NSIG {
            return None;
        }
        if sig == SIGKILL || sig == SIGSTOP {
            return Some(self.action(sig).unwrap_or_default());
        }
        let table = self.ensure_actions();
        let prev = table[sig as usize];
        table[sig as usize] = action;
        Some(prev)
    }

    pub fn action(&self, sig: i32) -> Option<SigAction> {
        if sig < 1 || (sig as usize) > NSIG {
            return None;
        }
        match &self.actions {
            Some(table) => Some(table[sig as usize]),
            None => Some(SigAction::default()),
        }
    }

    /// Pick the lowest pending signal that's both unblocked *and* has
    /// a real user-space handler installed (i.e. action is neither
    /// `SIG_DFL` nor `SIG_IGN`). Returns `(signum, action)` and clears
    /// the pending bit. Default-action signals (SIG_DFL) are left
    /// pending — PR-B2 only delivers explicit handlers; default
    /// dispositions land alongside `kill -INT` semantics in a future
    /// PR.
    pub fn consume_deliverable(&mut self) -> Option<(i32, SigAction)> {
        let unblocked = self.pending & !self.blocked;
        let mut bits = unblocked;
        while bits != 0 {
            let lowest = bits.trailing_zeros() as i32 + 1;
            let mask = 1u64 << (lowest - 1);
            bits &= !mask;
            if let Some(act) = self.action(lowest) {
                if act.sa_handler == SIG_DFL || act.sa_handler == SIG_IGN {
                    continue;
                }
                self.pending &= !mask;
                return Some((lowest, act));
            }
        }
        None
    }
}

impl Default for SignalState {
    fn default() -> Self {
        Self::new()
    }
}

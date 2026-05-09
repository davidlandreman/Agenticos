//! Name-keyed syscall ABI registry (D4).
//!
//! Two responsibilities:
//!
//! 1. **Static-slot syscall table.** `register_syscall(name, handler)` reserves
//!    one of `MAX_SYSCALLS` numeric IDs for a (name, handler) pair. The U6
//!    ELF loader will resolve user-side import names against this table; the
//!    syscall dispatcher (called from the naked `int 0x80` stub in
//!    `arch::x86_64::syscall`) looks up handlers by ID.
//!
//! 2. **Active user-VA bounds.** Until U7 lands a real "active user PCB"
//!    notion, the print syscall needs *some* way to know which VA window is
//!    legal for a user-pointer argument. We expose a static `Option<UserVaBounds>`
//!    that U6/U7 will populate at user-mode entry and clear at exit. Today's
//!    tests drive the static directly to exercise the dispatcher.

use core::sync::atomic::{AtomicUsize, Ordering};

use spin::Mutex;
use x86_64::VirtAddr;

use crate::arch::x86_64::syscall::SyscallArgs;

/// Maximum number of syscalls that can be registered. The trampoline page is
/// 4 KiB and each stub is ~9 bytes, so 64 fits comfortably.
pub const MAX_SYSCALLS: usize = 64;

/// Negative i64 sentinel for "syscall ID out of range / unregistered."
/// Following the Linux convention of negative-errno-style return values.
pub const ENOSYS: i64 = -38;

/// Negative i64 sentinel for "bad pointer" — the Linux convention for `EFAULT`.
pub const EFAULT: i64 = -14;

/// One entry in the syscall registry.
#[derive(Clone, Copy)]
pub struct SyscallEntry {
    pub name: &'static str,
    pub handler: SyscallHandler,
}

/// Raw handler signature. Receives the saved user GP registers and returns the
/// value that will end up in user RAX. Handlers must NOT panic — they run in
/// interrupt-gate context with IF cleared, so a panic would either deadlock
/// the system (panic handler tries to acquire the serial lock under another
/// pending IRQ) or trip the panic-handler-in-interrupt rule.
pub type SyscallHandler = fn(&mut SyscallArgs) -> i64;

static SYSCALL_TABLE: Mutex<[Option<SyscallEntry>; MAX_SYSCALLS]> =
    Mutex::new([None; MAX_SYSCALLS]);

/// Number of registered syscalls. Always equals the lowest unused slot index.
/// The trampoline page builder reads this to size the page contents.
static REGISTERED_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Active user-VA bounds (inclusive lower, exclusive upper). Populated by U7
/// before `iretq`-to-ring-3, cleared on exit. Print-syscall pointer validation
/// reads this static; if it is `None` the print path rejects the call (no
/// active user process means no valid user pointers).
///
/// Tests drive this directly via `set_user_va_bounds` / `clear_user_va_bounds`
/// to exercise the dispatcher without spinning up a full user process.
#[derive(Debug, Clone, Copy)]
pub struct UserVaBounds {
    pub start: u64,
    pub end: u64,
}

static USER_VA_BOUNDS: Mutex<Option<UserVaBounds>> = Mutex::new(None);

/// Last `exit_handler` exit code — placeholder until U7 wires the real
/// long-jump. Visible to tests so they can assert `exit(42)` recorded `42`.
pub static LAST_EXIT_CODE: Mutex<Option<i64>> = Mutex::new(None);

/// Register a syscall by name. Returns the assigned numeric ID, or `Err` if
/// the table is full or the name is a duplicate.
///
/// Idempotent on duplicate-name registration only insofar as the second call
/// fails fast — the first registrar wins. Callers should treat a duplicate
/// as a programmer error (two subsystems both trying to claim `print`).
pub fn register_syscall(
    name: &'static str,
    handler: SyscallHandler,
) -> Result<usize, RegisterError> {
    let mut table = SYSCALL_TABLE.lock();
    // Reject duplicates.
    for entry in table.iter().flatten() {
        if entry.name == name {
            return Err(RegisterError::DuplicateName);
        }
    }
    // Find the first empty slot.
    for (id, slot) in table.iter_mut().enumerate() {
        if slot.is_none() {
            *slot = Some(SyscallEntry { name, handler });
            // Update the registered-count high-water mark. Since we always
            // fill the lowest empty slot and never deregister, this is also
            // simply `id + 1` whenever it grows.
            let prev = REGISTERED_COUNT.load(Ordering::Relaxed);
            if id + 1 > prev {
                REGISTERED_COUNT.store(id + 1, Ordering::Relaxed);
            }
            return Ok(id);
        }
    }
    Err(RegisterError::TableFull)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterError {
    TableFull,
    DuplicateName,
}

/// Look up a syscall by name. Returns the assigned numeric ID. Used by the
/// trampoline-page builder so it can synthesize stubs in the same order the
/// IDs were assigned.
pub fn syscall_id(name: &str) -> Option<usize> {
    let table = SYSCALL_TABLE.lock();
    for (id, entry) in table.iter().enumerate() {
        if let Some(e) = entry {
            if e.name == name {
                return Some(id);
            }
        }
    }
    None
}

/// Number of currently registered syscalls (the trampoline page emits one
/// stub per registered syscall).
pub fn registered_count() -> usize {
    REGISTERED_COUNT.load(Ordering::Relaxed)
}

/// Snapshot the registered (name, id) pairs. Used by the trampoline-page
/// builder. Returns up to `MAX_SYSCALLS` `(name, id)` entries.
pub fn snapshot_registry() -> [(Option<&'static str>, usize); MAX_SYSCALLS] {
    let table = SYSCALL_TABLE.lock();
    let mut out: [(Option<&'static str>, usize); MAX_SYSCALLS] =
        [(None, 0); MAX_SYSCALLS];
    for (id, entry) in table.iter().enumerate() {
        if let Some(e) = entry {
            out[id] = (Some(e.name), id);
        }
    }
    out
}

/// Set the active user-VA bounds. U7 calls this before entering ring 3.
pub fn set_user_va_bounds(bounds: UserVaBounds) {
    *USER_VA_BOUNDS.lock() = Some(bounds);
}

/// Clear the active user-VA bounds. U7 calls this on user-process exit.
pub fn clear_user_va_bounds() {
    *USER_VA_BOUNDS.lock() = None;
}

/// Read the active user-VA bounds, if any. The print-syscall pointer-validation
/// helper consumes this.
pub fn user_va_bounds() -> Option<UserVaBounds> {
    *USER_VA_BOUNDS.lock()
}

/// Validate that a user-supplied `(ptr, len)` slice lies entirely within the
/// active user-VA bounds. **Defends S2** of the doc-review findings: the
/// addition `ptr + len` is performed with `checked_add` to defeat integer
/// wraparound near the top of the address space. A `len` of 0 is valid and
/// returns `Ok(())` regardless of `ptr`.
pub fn validate_user_slice(ptr: u64, len: u64) -> Result<(), i64> {
    if len == 0 {
        return Ok(());
    }
    let bounds = user_va_bounds().ok_or(EFAULT)?;
    let end = ptr.checked_add(len).ok_or(EFAULT)?;
    if ptr < bounds.start || end > bounds.end {
        return Err(EFAULT);
    }
    // Also reject obviously-kernel addresses defensively (the bounds check
    // above already covers this, but if a buggy U7 somehow sets bounds
    // overlapping kernel space, this would still catch it).
    if VirtAddr::try_new(ptr).is_err() || VirtAddr::try_new(end).is_err() {
        return Err(EFAULT);
    }
    Ok(())
}

/// Central syscall dispatcher. Called from the naked `int 0x80` entry stub
/// (via `syscall_dispatch_entry` in `arch::x86_64::syscall`). Looks up the
/// syscall ID in `SYSCALL_TABLE`; routes to the registered handler or
/// returns `ENOSYS` if the ID is unregistered or out of range.
pub fn syscall_dispatch(args: &mut SyscallArgs) -> i64 {
    let id = args.rax as usize;
    if id >= MAX_SYSCALLS {
        return ENOSYS;
    }
    let handler = {
        let table = SYSCALL_TABLE.lock();
        table[id].map(|e| e.handler)
    };
    match handler {
        Some(h) => h(args),
        None => ENOSYS,
    }
}

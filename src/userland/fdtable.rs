//! Per-process file-descriptor table for ring-3 user processes.
//!
//! Phase 2 of the path toward a real shell. Slots 0/1/2 are pinned to
//! stdin/stdout/stderr (which route through the per-process stdin queue
//! and the kernel `print!` pipeline respectively). Slots 3..N hold
//! `Arc<File>` handles produced by `openat`. There is at most one user
//! process at a time (D5), so the table lives directly on `ActiveUser`.
//!
//! Closing a slot drops the `Arc<File>`; the underlying FAT cluster-walk
//! state is released automatically.

use crate::fs::file_handle::{Directory, File};
use crate::lib::arc::Arc;
use crate::net::socket::SocketHandle;
use crate::userland::epoll::EpollInstance;
use crate::userland::eventfd::EventFd;
use crate::userland::local_stream::LocalStreamEndpoint;
use crate::userland::pipe::{PipeReadHandle, PipeWriteHandle};
use core::sync::atomic::{AtomicBool, Ordering};

/// Maximum file descriptors per process. Bounded to keep the table size
/// in `ActiveUser` small and predictable. zsh during a basic interactive
/// session opens ~10–15 fds; 64 leaves headroom for GCC's driver →
/// collect2 → ld fd-inheritance chain on top of that.
pub const FD_TABLE_SIZE: usize = 64;

pub const STDIN_FD: i32 = 0;
pub const STDOUT_FD: i32 = 1;
pub const STDERR_FD: i32 = 2;

/// Shared open-file state for a process's GUI event queue. `dup` shares this
/// object (and therefore `O_NONBLOCK`) just like a Linux open-file
/// description. The queue itself remains owned by `userland::gui` and keyed
/// by `owner_pid`.
pub struct GuiEventHandle {
    owner_pid: u32,
    nonblocking: AtomicBool,
}

impl GuiEventHandle {
    pub fn new(owner_pid: u32, nonblocking: bool) -> Arc<Self> {
        Arc::new(Self {
            owner_pid,
            nonblocking: AtomicBool::new(nonblocking),
        })
    }

    pub fn owner_pid(&self) -> u32 {
        self.owner_pid
    }

    pub fn nonblocking(&self) -> bool {
        self.nonblocking.load(Ordering::Acquire)
    }

    pub fn set_nonblocking(&self, value: bool) {
        self.nonblocking.store(value, Ordering::Release);
    }
}

/// One file-descriptor slot. The standard streams are sentinel variants
/// because they don't have an `Arc<File>` backing — `read(0)` consults
/// `userland::stdin` and `write(1|2)` goes through `print!`.
#[derive(Clone)]
pub enum FdSlot {
    Stdin,
    Stdout,
    Stderr,
    /// An opened file. `cloexec` is recorded for future `execve` (Phase 4)
    /// — today it has no effect, since there's no exec.
    File {
        handle: Arc<File>,
        /// Linux open-file status flags returned by `fcntl(F_GETFL)`.
        /// Access mode is immutable; `O_APPEND`/`O_NONBLOCK` may change.
        status_flags: u32,
        cloexec: bool,
    },
    /// An opened directory. `cursor` is the per-fd index into the
    /// directory's snapshotted entries — advanced by `getdents64`,
    /// independent across `dup`'d fds (a deviation from POSIX, which
    /// shares the offset across dup'd fds; fine for read-only zsh-like
    /// usage where job control isn't doing anything fancy here).
    Directory {
        handle: Arc<Directory>,
        cursor: usize,
        cloexec: bool,
    },
    /// Read end of an anonymous pipe (Phase 5 PR-A). `PipeReadHandle`
    /// has custom Clone/Drop that maintains the pipe's reader count.
    PipeRead(PipeReadHandle, bool /* cloexec */),
    /// Write end of an anonymous pipe.
    PipeWrite(PipeWriteHandle, bool /* cloexec */),
    /// The virtual `/bin` directory backed by the BusyBox applet list.
    /// `cursor` is the per-fd index into `bin_namespace::APPLETS`;
    /// `getdents64` reads and advances it. See
    /// `crate::userland::bin_namespace` for the namespace model.
    VirtualBinDir {
        cursor: usize,
        cloexec: bool,
    },
    /// A socket open-file description. Status flags live in the network
    /// registry entry so dup/fork share them; cloexec remains per descriptor.
    Socket {
        handle: Arc<SocketHandle>,
        cloexec: bool,
    },
    /// A synthesized read-only file (the `/proc` namespace). `data` is
    /// the full content snapshot generated at `open()`; `cursor` is the
    /// per-fd read offset. `Arc` keeps dup/fork clones cheap; the
    /// buffer frees when the last fd drops.
    VirtualFile {
        data: Arc<alloc::vec::Vec<u8>>,
        path: Arc<alloc::string::String>,
        cursor: usize,
        cloexec: bool,
    },
    /// A synthesized directory (the `/proc` namespace). `entries` is
    /// the `(name, is_dir)` listing snapshot from `open()` — `.`/`..`
    /// are synthesized by `getdents64`, not stored.
    VirtualDir {
        entries: Arc<alloc::vec::Vec<(alloc::string::String, bool)>>,
        path: Arc<alloc::string::String>,
        cursor: usize,
        cloexec: bool,
    },
    /// Synthetic `/dev` directory. Its entries are `null` and `urandom`.
    VirtualDevDir {
        cursor: usize,
        cloexec: bool,
    },
    /// Dynamic cryptographic random character device.
    Urandom {
        cloexec: bool,
    },
    /// `/dev/null`: reads return EOF, writes are discarded.
    DevNull {
        cloexec: bool,
    },
    /// Selectable view of the owning process's fixed-size GUI event queue.
    GuiEvents {
        handle: Arc<GuiEventHandle>,
        cloexec: bool,
    },
    /// Linux event counter used by libuv's cross-thread async wake path.
    EventFd {
        handle: Arc<EventFd>,
        cloexec: bool,
    },
    /// Bounded epoll interest set. The instance is an open-file description:
    /// dup/fork share registrations, while close-on-exec stays per fd.
    Epoll {
        handle: Arc<EpollInstance>,
        cloexec: bool,
    },
    LocalStream {
        handle: Arc<LocalStreamEndpoint>,
        cloexec: bool,
    },
    /// The master end of a pty, owned by a ring-3 terminal emulator
    /// (`TERMINAL.ELF`). Reads drain the slave's output; writes push into the
    /// slave's input. The slave side is reached by the child process through
    /// its inherited `terminal_id` (the existing sentinel-stdio model).
    PtyMaster {
        master: crate::terminal::pty::PtyMaster,
        cloexec: bool,
    },
}

impl FdSlot {
    /// The per-descriptor FD_CLOEXEC bit. The standard-stream sentinels
    /// never carry it.
    pub fn is_cloexec(&self) -> bool {
        match self {
            Self::Stdin | Self::Stdout | Self::Stderr => false,
            Self::File { cloexec, .. }
            | Self::Directory { cloexec, .. }
            | Self::VirtualBinDir { cloexec, .. }
            | Self::Socket { cloexec, .. }
            | Self::VirtualFile { cloexec, .. }
            | Self::VirtualDir { cloexec, .. }
            | Self::VirtualDevDir { cloexec, .. }
            | Self::Urandom { cloexec }
            | Self::DevNull { cloexec }
            | Self::GuiEvents { cloexec, .. }
            | Self::EventFd { cloexec, .. }
            | Self::Epoll { cloexec, .. }
            | Self::LocalStream { cloexec, .. }
            | Self::PtyMaster { cloexec, .. } => *cloexec,
            Self::PipeRead(_, cloexec) | Self::PipeWrite(_, cloexec) => *cloexec,
        }
    }

    /// Change the per-descriptor close-on-exec bit without affecting the
    /// shared open-file description. The three legacy standard-stream
    /// sentinels cannot carry descriptor flags and therefore remain clear.
    fn set_cloexec(&mut self, value: bool) {
        match self {
            Self::Stdin | Self::Stdout | Self::Stderr => {}
            Self::File { cloexec, .. }
            | Self::Directory { cloexec, .. }
            | Self::VirtualBinDir { cloexec, .. }
            | Self::Socket { cloexec, .. }
            | Self::VirtualFile { cloexec, .. }
            | Self::VirtualDir { cloexec, .. }
            | Self::VirtualDevDir { cloexec, .. }
            | Self::Urandom { cloexec }
            | Self::DevNull { cloexec }
            | Self::GuiEvents { cloexec, .. }
            | Self::EventFd { cloexec, .. }
            | Self::Epoll { cloexec, .. }
            | Self::LocalStream { cloexec, .. }
            | Self::PtyMaster { cloexec, .. } => *cloexec = value,
            Self::PipeRead(_, cloexec) | Self::PipeWrite(_, cloexec) => *cloexec = value,
        }
    }

    /// Whether two descriptor slots refer to the same Linux open-file
    /// description. Per-descriptor close-on-exec bits are intentionally
    /// ignored.
    pub fn same_open_description(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Stdin, Self::Stdin)
            | (Self::Stdout, Self::Stdout)
            | (Self::Stderr, Self::Stderr)
            | (Self::Urandom { .. }, Self::Urandom { .. })
            | (Self::DevNull { .. }, Self::DevNull { .. }) => true,
            (Self::File { handle: left, .. }, Self::File { handle: right, .. }) => {
                Arc::ptr_eq(left, right)
            }
            (Self::Directory { handle: left, .. }, Self::Directory { handle: right, .. }) => {
                Arc::ptr_eq(left, right)
            }
            (Self::PipeRead(left, _), Self::PipeRead(right, _)) => {
                left.same_open_description(right)
            }
            (Self::PipeWrite(left, _), Self::PipeWrite(right, _)) => {
                left.same_open_description(right)
            }
            (Self::Socket { handle: left, .. }, Self::Socket { handle: right, .. }) => {
                Arc::ptr_eq(left, right)
            }
            (Self::VirtualFile { data: left, .. }, Self::VirtualFile { data: right, .. }) => {
                Arc::ptr_eq(left, right)
            }
            (Self::VirtualDir { entries: left, .. }, Self::VirtualDir { entries: right, .. }) => {
                Arc::ptr_eq(left, right)
            }
            (Self::GuiEvents { handle: left, .. }, Self::GuiEvents { handle: right, .. }) => {
                Arc::ptr_eq(left, right)
            }
            (Self::EventFd { handle: left, .. }, Self::EventFd { handle: right, .. }) => {
                Arc::ptr_eq(left, right)
            }
            (Self::Epoll { handle: left, .. }, Self::Epoll { handle: right, .. }) => {
                Arc::ptr_eq(left, right)
            }
            (Self::LocalStream { handle: left, .. }, Self::LocalStream { handle: right, .. }) => {
                Arc::ptr_eq(left, right)
            }
            (Self::PtyMaster { master: left, .. }, Self::PtyMaster { master: right, .. }) => {
                left.same_master(right)
            }
            (Self::VirtualBinDir { .. }, Self::VirtualBinDir { .. })
            | (Self::VirtualDevDir { .. }, Self::VirtualDevDir { .. }) => false,
            _ => false,
        }
    }
}

#[derive(Clone)]
pub struct FdTable {
    slots: [Option<FdSlot>; FD_TABLE_SIZE],
}

impl FdTable {
    pub const fn new() -> Self {
        const NONE: Option<FdSlot> = None;
        Self {
            slots: [NONE; FD_TABLE_SIZE],
        }
    }

    /// Pin slots 0/1/2 to the standard streams. Called by
    /// `enter_user_mode_with` before iretq so the binary's libc finds
    /// fd 0/1/2 already allocated.
    pub fn install_default_streams(&mut self) {
        self.slots[STDIN_FD as usize] = Some(FdSlot::Stdin);
        self.slots[STDOUT_FD as usize] = Some(FdSlot::Stdout);
        self.slots[STDERR_FD as usize] = Some(FdSlot::Stderr);
    }

    /// Drop every slot. Called by `release_active_image` after the
    /// long-jump returns from ring 3.
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn clear(&mut self) {
        for slot in self.slots.iter_mut() {
            *slot = None;
        }
    }

    pub fn get(&self, fd: i32) -> Option<&FdSlot> {
        if fd < 0 || (fd as usize) >= FD_TABLE_SIZE {
            return None;
        }
        self.slots[fd as usize].as_ref()
    }

    /// Mutable access to the slot at `fd`. Used by `getdents64` to
    /// advance the per-fd cursor.
    pub fn get_mut(&mut self, fd: i32) -> Option<&mut FdSlot> {
        if fd < 0 || (fd as usize) >= FD_TABLE_SIZE {
            return None;
        }
        self.slots[fd as usize].as_mut()
    }

    /// Remove every slot whose FD_CLOEXEC bit is set and return ownership to
    /// the caller. The caller must drop the returned slots only after it has
    /// released `PROCESS_TABLE`: pipe-endpoint `Drop` wakes readers by taking
    /// that lock, and musl posix_spawn relies on the resulting EOF wake as its
    /// successful-exec notification.
    pub fn take_cloexec(&mut self) -> alloc::vec::Vec<FdSlot> {
        let mut removed = alloc::vec::Vec::new();
        for slot in self.slots.iter_mut() {
            if slot.as_ref().is_some_and(FdSlot::is_cloexec) {
                removed.push(slot.take().expect("CLOEXEC slot disappeared"));
            }
        }
        removed
    }

    /// Insert `slot` at the lowest available index ≥ 3 and return the
    /// fd. Returns `None` when the table is full.
    pub fn alloc(&mut self, slot: FdSlot) -> Option<i32> {
        for i in 3..FD_TABLE_SIZE {
            if self.slots[i].is_none() {
                self.slots[i] = Some(slot);
                return Some(i as i32);
            }
        }
        None
    }

    /// Clone descriptors for `fork`. GUI event queues are process-owned and
    /// cannot safely be consumed by a child, so those descriptor numbers are
    /// intentionally left closed in the child.
    pub fn fork_clone(&self) -> Self {
        let mut cloned = Self::new();
        for (index, slot) in self.slots.iter().enumerate() {
            cloned.slots[index] = match slot {
                Some(FdSlot::GuiEvents { .. }) => None,
                Some(slot) => Some(slot.clone()),
                None => None,
            };
        }
        cloned
    }

    /// Remove the slot at `fd`. Returns `Err(EBADF)` if the slot is
    /// already empty or the fd is out of range.
    pub fn close(&mut self, fd: i32) -> Result<(), i64> {
        use crate::userland::abi::EBADF;
        if fd < 0 || (fd as usize) >= FD_TABLE_SIZE {
            return Err(EBADF);
        }
        if self.slots[fd as usize].take().is_none() {
            return Err(EBADF);
        }
        Ok(())
    }

    pub fn contains_open_description(&self, description: &FdSlot) -> bool {
        self.slots
            .iter()
            .flatten()
            .any(|slot| slot.same_open_description(description))
    }

    pub fn epoll_instances(&self) -> alloc::vec::Vec<Arc<EpollInstance>> {
        let mut instances = alloc::vec::Vec::new();
        for slot in self.slots.iter().flatten() {
            let FdSlot::Epoll { handle, .. } = slot else {
                continue;
            };
            if !instances.iter().any(|old| Arc::ptr_eq(old, handle)) {
                instances.push(handle.clone());
            }
        }
        instances
    }

    /// `dup(fd)` — clone the slot into the lowest free index ≥ 3.
    pub fn dup(&mut self, fd: i32) -> Option<i32> {
        let mut slot = self.get(fd)?.clone();
        // POSIX: descriptor duplication clears FD_CLOEXEC on the new fd.
        slot.set_cloexec(false);
        self.alloc(slot)
    }

    /// `dup2(oldfd, newfd)` — close newfd if open, then place a clone
    /// of oldfd's slot at newfd. Returns the new fd.
    pub fn dup2(&mut self, oldfd: i32, newfd: i32) -> Option<i32> {
        if newfd < 0 || (newfd as usize) >= FD_TABLE_SIZE {
            return None;
        }
        if oldfd == newfd {
            // POSIX: dup2(fd, fd) is a no-op iff fd is valid.
            return self.get(oldfd).map(|_| newfd);
        }
        let mut slot = self.get(oldfd)?.clone();
        // POSIX: dup2 clears FD_CLOEXEC when it creates a distinct fd.
        slot.set_cloexec(false);
        self.slots[newfd as usize] = Some(slot);
        Some(newfd)
    }

    /// `fcntl(F_DUPFD*)`: duplicate `fd` into the lowest free descriptor at
    /// or above `minimum`, choosing the new descriptor's FD_CLOEXEC value.
    pub fn dup_from(&mut self, fd: i32, minimum: i32, cloexec: bool) -> Option<i32> {
        if minimum < 0 || minimum as usize >= FD_TABLE_SIZE {
            return None;
        }
        let mut slot = self.get(fd)?.clone();
        slot.set_cloexec(cloexec);
        for index in minimum as usize..FD_TABLE_SIZE {
            if self.slots[index].is_none() {
                self.slots[index] = Some(slot);
                return Some(index as i32);
            }
        }
        None
    }

    /// Set/clear the FD_CLOEXEC flag for a file or directory slot.
    /// No-op for stream slots (they have no per-fd flags).
    pub fn set_cloexec(&mut self, fd: i32, cloexec: bool) -> Result<(), i64> {
        use crate::userland::abi::EBADF;
        let slot = self
            .slots
            .get_mut(fd as usize)
            .and_then(|s| s.as_mut())
            .ok_or(EBADF)?;
        slot.set_cloexec(cloexec);
        Ok(())
    }

    pub fn cloexec(&self, fd: i32) -> Result<bool, i64> {
        use crate::userland::abi::EBADF;
        let slot = self.get(fd).ok_or(EBADF)?;
        Ok(match slot {
            FdSlot::File { cloexec, .. } | FdSlot::Directory { cloexec, .. } => *cloexec,
            FdSlot::PipeRead(_, ce) | FdSlot::PipeWrite(_, ce) => *ce,
            FdSlot::VirtualBinDir { cloexec, .. } => *cloexec,
            FdSlot::Socket { cloexec, .. } => *cloexec,
            FdSlot::VirtualFile { cloexec, .. } => *cloexec,
            FdSlot::VirtualDir { cloexec, .. } => *cloexec,
            FdSlot::VirtualDevDir { cloexec, .. } => *cloexec,
            FdSlot::Urandom { cloexec } => *cloexec,
            FdSlot::DevNull { cloexec } => *cloexec,
            FdSlot::GuiEvents { cloexec, .. } => *cloexec,
            FdSlot::EventFd { cloexec, .. } | FdSlot::Epoll { cloexec, .. } => *cloexec,
            FdSlot::LocalStream { cloexec, .. } => *cloexec,
            _ => false,
        })
    }
}

impl Default for FdTable {
    fn default() -> Self {
        Self::new()
    }
}

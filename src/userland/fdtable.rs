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
use crate::userland::pipe::{PipeReadHandle, PipeWriteHandle};

/// Maximum file descriptors per process. Bounded to keep the table size
/// in `ActiveUser` small and predictable. zsh during a basic interactive
/// session opens ~10–15 fds; 32 is comfortable headroom.
pub const FD_TABLE_SIZE: usize = 32;

pub const STDIN_FD: i32 = 0;
pub const STDOUT_FD: i32 = 1;
pub const STDERR_FD: i32 = 2;

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
}

impl FdSlot {}

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

    /// `dup(fd)` — clone the slot into the lowest free index ≥ 3.
    pub fn dup(&mut self, fd: i32) -> Option<i32> {
        let slot = self.get(fd)?.clone();
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
        let slot = self.get(oldfd)?.clone();
        self.slots[newfd as usize] = Some(slot);
        Some(newfd)
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
        match slot {
            FdSlot::File { cloexec: ce, .. } => *ce = cloexec,
            FdSlot::Directory { cloexec: ce, .. } => *ce = cloexec,
            FdSlot::PipeRead(_, ce) | FdSlot::PipeWrite(_, ce) => *ce = cloexec,
            FdSlot::VirtualBinDir { cloexec: ce, .. } => *ce = cloexec,
            FdSlot::Socket { cloexec: ce, .. } => *ce = cloexec,
            FdSlot::VirtualFile { cloexec: ce, .. } => *ce = cloexec,
            FdSlot::VirtualDir { cloexec: ce, .. } => *ce = cloexec,
            _ => {}
        }
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
            _ => false,
        })
    }
}

impl Default for FdTable {
    fn default() -> Self {
        Self::new()
    }
}

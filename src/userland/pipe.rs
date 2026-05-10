//! Anonymous pipes — kernel ring buffer + reader/writer fd handles.
//!
//! Phase 5 PR-A. A pipe is a single ring buffer with two ends. The
//! read end and the write end each get their own fd in the calling
//! process's FD table, and `dup`/`dup2`/`fork` clone those fds while
//! sharing the underlying buffer.
//!
//! Behavior follows POSIX:
//! - `read(read_fd, …)` drains as many bytes as are available, up to
//!   `count`. Returns `0` (EOF) when the buffer is empty *and* no
//!   writer fds remain. Returns `-EAGAIN` when empty but writers
//!   exist (POSIX would block; we don't have a real scheduler yet so
//!   for now we fail-fast — see `read_handler`).
//! - `write(write_fd, …)` accepts as many bytes as fit in the
//!   remaining capacity. Returns `-EPIPE` when the buffer is full
//!   and no reader fds remain.
//! - `close` drops the fd; when the last reader closes, writers see
//!   `-EPIPE` on the next write; when the last writer closes,
//!   readers see EOF.
//!
//! With our synchronous-fork model this is enough for short-output
//! pipelines (`echo foo | cat`): the parent forks the writer, the
//! writer's output fits in the 4 KiB buffer, the writer exits, then
//! the parent forks the reader which drains the buffer. Pipelines
//! that need both halves running concurrently wait for Phase 5 PR-B.

use alloc::collections::VecDeque;
use core::sync::atomic::{AtomicU32, Ordering};
use spin::Mutex;

use crate::lib::arc::Arc;

/// Capacity of a pipe's ring buffer. Linux's default is 64 KiB; we
/// pick a smaller value because the buffer is per-pipe and our heap
/// is bounded.
pub const PIPE_CAPACITY: usize = 4096;

/// Underlying pipe shared between read and write handles.
pub struct Pipe {
    inner: Mutex<VecDeque<u8>>,
    /// Number of read-side fds referencing this pipe.
    reader_count: AtomicU32,
    /// Number of write-side fds referencing this pipe.
    writer_count: AtomicU32,
}

impl Pipe {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(VecDeque::with_capacity(PIPE_CAPACITY)),
            reader_count: AtomicU32::new(0),
            writer_count: AtomicU32::new(0),
        })
    }

    pub fn readers(&self) -> u32 {
        self.reader_count.load(Ordering::Acquire)
    }

    pub fn writers(&self) -> u32 {
        self.writer_count.load(Ordering::Acquire)
    }

    /// Try to push as many bytes as fit into the ring buffer. Returns
    /// the count actually written (may be 0 if buffer is full).
    pub fn write(&self, src: &[u8]) -> usize {
        let mut buf = self.inner.lock();
        let room = PIPE_CAPACITY.saturating_sub(buf.len());
        let take = core::cmp::min(room, src.len());
        for &b in &src[..take] {
            buf.push_back(b);
        }
        take
    }

    /// Pop up to `dst.len()` bytes from the buffer into `dst`.
    /// Returns the number of bytes copied.
    pub fn read(&self, dst: &mut [u8]) -> usize {
        let mut buf = self.inner.lock();
        let n = core::cmp::min(dst.len(), buf.len());
        for slot in dst.iter_mut().take(n) {
            *slot = buf.pop_front().unwrap();
        }
        n
    }

    /// Bytes currently buffered (test-facing).
    #[cfg(feature = "test")]
    pub fn len(&self) -> usize {
        self.inner.lock().len()
    }
}

/// Read end of a pipe. Cloning increments the pipe's reader count;
/// dropping decrements it. The pipe knows how many readers and
/// writers exist via these counts.
pub struct PipeReadHandle {
    pipe: Arc<Pipe>,
}

impl PipeReadHandle {
    /// Construct the first reader handle on a freshly-created pipe.
    /// Use this exactly once per pipe; subsequent fds clone via
    /// `Clone`.
    pub fn new(pipe: Arc<Pipe>) -> Self {
        pipe.reader_count.fetch_add(1, Ordering::Release);
        Self { pipe }
    }

    pub fn pipe(&self) -> &Arc<Pipe> {
        &self.pipe
    }
}

impl Clone for PipeReadHandle {
    fn clone(&self) -> Self {
        self.pipe.reader_count.fetch_add(1, Ordering::Release);
        Self { pipe: self.pipe.clone() }
    }
}

impl Drop for PipeReadHandle {
    fn drop(&mut self) {
        self.pipe.reader_count.fetch_sub(1, Ordering::Release);
    }
}

/// Write end of a pipe. Mirror of `PipeReadHandle`.
pub struct PipeWriteHandle {
    pipe: Arc<Pipe>,
}

impl PipeWriteHandle {
    pub fn new(pipe: Arc<Pipe>) -> Self {
        pipe.writer_count.fetch_add(1, Ordering::Release);
        Self { pipe }
    }

    pub fn pipe(&self) -> &Arc<Pipe> {
        &self.pipe
    }
}

impl Clone for PipeWriteHandle {
    fn clone(&self) -> Self {
        self.pipe.writer_count.fetch_add(1, Ordering::Release);
        Self { pipe: self.pipe.clone() }
    }
}

impl Drop for PipeWriteHandle {
    fn drop(&mut self) {
        self.pipe.writer_count.fetch_sub(1, Ordering::Release);
    }
}

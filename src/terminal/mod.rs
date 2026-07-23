//! Kernel pseudo-terminal service.
//!
//! The interactive emulator lives in ring 3 as `TERMINAL.ELF`; the kernel
//! retains only the PTY queues, termios, winsize, line discipline, and master
//! fd plumbing.

pub mod pty;

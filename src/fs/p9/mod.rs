//! 9P2000.L client filesystem over the virtio-9p transport.
//!
//! Backs the `/shared` mount: a host directory exported by QEMU's `local`
//! fsdev, safe for concurrent use by multiple simultaneously running
//! instances because the host kernel owns the real filesystem.

pub(crate) mod client;
mod filesystem;
pub(crate) mod protocol;

pub use filesystem::P9Filesystem;

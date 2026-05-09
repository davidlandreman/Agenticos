//! RPC transport for the kernel tool registry.
//!
//! Wire format: `[u32 LE header_len][JSON header][u32 LE binary_len][binary]`.
//! `framing` reads/writes whole frames over a `Com2` byte stream;
//! `dispatcher` is the kernel-process body that reads requests, calls the
//! registry, and writes responses.

pub mod dispatcher;
pub mod framing;

pub use dispatcher::run_dispatcher;

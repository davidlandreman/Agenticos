//! Overlay filesystem: merge an `upper` writable FS over a `lower`
//! read-only FS.
//!
//! Reads fall through upper → lower with whiteout shadowing. Writes
//! land in upper; a file that exists only in lower is copy-up'd into
//! upper before mutation. Deletes mark the upper side with a
//! `.wh.<name>` whiteout sentinel so the lower entry is invisible to
//! subsequent reads.
//!
//! Used at boot to mount `/` as `overlay(upper=tmpfs, lower=boot-FAT)`,
//! giving userland a writable root without touching the immutable
//! bootloader-built FAT image. Persistence of the upper layer is a
//! Phase D concern.

pub mod filesystem;

pub use filesystem::Overlay;

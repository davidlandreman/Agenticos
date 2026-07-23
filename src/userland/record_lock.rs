//! POSIX advisory record locks for `fcntl(F_GETLK/F_SETLK/F_SETLKW)`.
//!
//! These are **advisory** (they constrain only other `fcntl` lockers, never
//! `read`/`write`) and **process-associated** in the classic POSIX sense:
//! owned by the thread-group leader (TGID), released when the process closes
//! *any* descriptor referring to the file or when it exits. Open-file-
//! description locks (`F_OFD_*`) and mandatory locking are out of scope.
//!
//! ## Keying
//!
//! Locks are keyed on the file's **normalized absolute path**, not its inode.
//! tmpfs — which backs `/` and `/work`, where GNU Make's `--output-sync` sync
//! file lives — assigns `FileHandle.inode` a fresh per-open-handle id rather
//! than a stable per-file inode (`src/fs/tmpfs/mod.rs`), so inode identity
//! cannot serve as a cross-open lock key. Absolute paths are globally unique
//! across mounts and stable across independent `open()` calls, which is exactly
//! what the whole-file advisory-lock use case needs. The tradeoff is that
//! hard-link / rename aliasing is not tracked; no current consumer needs it.
//!
//! ## Blocking
//!
//! `F_SETLKW` parks the caller on
//! [`Ring3BlockReason::WaitingForFileLock`](crate::userland::lifecycle::Ring3BlockReason::WaitingForFileLock).
//! Every lock release calls [`wake_lock_waiters`], which bumps the global
//! readiness sequence; the readiness wake path (extended to match file-lock
//! waiters) re-readies every waiter, whose re-fired SYSCALL re-attempts the
//! lock and either succeeds or re-blocks. The sequence sampled before the
//! conflict check closes the scan-to-park lost-wake race via
//! `reconcile_readiness_after_block`, exactly like the pipe/poll waiters.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::arch::x86_64::interrupt_guard::InterruptMutex;
use crate::userland::abi::ENOLCK;

/// Cap the records tracked for one file. Whole-file locking (Make's case)
/// collapses to a single record; the bound only guards pathological
/// byte-range fragmentation.
const MAX_RECORDS_PER_FILE: usize = 256;
/// Cap the number of distinct files with live locks.
const MAX_FILES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockKind {
    Read,
    Write,
}

impl LockKind {
    /// Two locks conflict only if at least one is a write lock.
    fn incompatible_with(self, other: LockKind) -> bool {
        self == LockKind::Write || other == LockKind::Write
    }
}

/// A held byte range, `[start, end]` inclusive. `end == u64::MAX` encodes a
/// lock that runs to end-of-file (POSIX `l_len == 0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LockRange {
    pub start: u64,
    pub end: u64,
}

impl LockRange {
    fn overlaps(&self, other: &LockRange) -> bool {
        self.start <= other.end && other.start <= self.end
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LockRecord {
    range: LockRange,
    kind: LockKind,
    owner: u32,
}

/// A conflicting lock reported by `F_GETLK` / a failed `F_SETLK`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Conflict {
    pub range: LockRange,
    pub kind: LockKind,
    pub owner: u32,
}

static LOCKS: InterruptMutex<BTreeMap<String, Vec<LockRecord>>> =
    InterruptMutex::new(BTreeMap::new());

/// First record owned by a *different* TGID that overlaps `range` with an
/// incompatible kind. `None` means the requested lock is grantable.
fn first_conflict(
    records: &[LockRecord],
    range: LockRange,
    kind: LockKind,
    owner: u32,
) -> Option<Conflict> {
    records
        .iter()
        .find(|record| {
            record.owner != owner
                && record.kind.incompatible_with(kind)
                && record.range.overlaps(&range)
        })
        .map(|record| Conflict {
            range: record.range,
            kind: record.kind,
            owner: record.owner,
        })
}

/// Remove the owner's coverage of `range`, retaining the left/right remnants
/// of any straddling record with their original kind. Shared by `set` (which
/// then inserts a fresh record) and `unlock`.
fn clear_owner_range(records: &mut Vec<LockRecord>, range: LockRange, owner: u32) {
    let mut remnants: Vec<LockRecord> = Vec::new();
    records.retain(|record| {
        if record.owner != owner || !record.range.overlaps(&range) {
            return true;
        }
        // `record.range.start < range.start` implies `range.start >= 1`, so the
        // `- 1` cannot underflow; symmetrically for the right remnant `+ 1`.
        if record.range.start < range.start {
            remnants.push(LockRecord {
                range: LockRange {
                    start: record.range.start,
                    end: range.start - 1,
                },
                kind: record.kind,
                owner,
            });
        }
        if record.range.end > range.end {
            remnants.push(LockRecord {
                range: LockRange {
                    start: range.end + 1,
                    end: record.range.end,
                },
                kind: record.kind,
                owner,
            });
        }
        false
    });
    records.extend(remnants);
}

/// Merge adjacent or overlapping same-owner same-kind records so repeated
/// lock/unlock cycles cannot fragment a file's record list without bound.
/// Records from different owners (compatible read locks) legitimately overlap
/// and are never merged.
fn coalesce(records: &mut Vec<LockRecord>) {
    records.sort_by(|a, b| {
        a.owner
            .cmp(&b.owner)
            .then(a.range.start.cmp(&b.range.start))
    });
    let mut merged: Vec<LockRecord> = Vec::with_capacity(records.len());
    for record in records.drain(..) {
        if let Some(last) = merged.last_mut() {
            let touching = record.range.start <= last.range.end.saturating_add(1);
            if last.owner == record.owner && last.kind == record.kind && touching {
                last.range.end = last.range.end.max(record.range.end);
                continue;
            }
        }
        merged.push(record);
    }
    *records = merged;
}

/// Test whether `range`/`kind` could be locked by `owner`. Returns the
/// conflicting lock for the `F_GETLK` reply, or `None` when it is free.
pub fn test(key: &str, range: LockRange, kind: LockKind, owner: u32) -> Option<Conflict> {
    let locks = LOCKS.lock();
    locks
        .get(key)
        .and_then(|records| first_conflict(records, range, kind, owner))
}

/// Acquire `range`/`kind` for `owner`, or report the conflictor. On success the
/// owner's prior overlapping records are replaced with the new kind.
pub fn set(key: &str, range: LockRange, kind: LockKind, owner: u32) -> Result<(), Conflict> {
    let mut locks = LOCKS.lock();
    if let Some(records) = locks.get(key) {
        if let Some(conflict) = first_conflict(records, range, kind, owner) {
            return Err(conflict);
        }
    } else if locks.len() >= MAX_FILES {
        // Treat table exhaustion as a self-conflict-free failure the caller
        // maps to ENOLCK; encode it as a zero-owner sentinel the handler
        // never surfaces (see `set_errno`).
        return Err(Conflict {
            range,
            kind,
            owner: u32::MAX,
        });
    }
    let records = locks.entry(String::from(key)).or_default();
    clear_owner_range(records, range, owner);
    records.push(LockRecord { range, kind, owner });
    coalesce(records);
    if records.len() > MAX_RECORDS_PER_FILE {
        // Undo: drop the just-added coverage. The owner keeps whatever it held
        // before, and the caller sees ENOLCK.
        clear_owner_range(records, range, owner);
        if records.is_empty() {
            locks.remove(key);
        }
        return Err(Conflict {
            range,
            kind,
            owner: u32::MAX,
        });
    }
    Ok(())
}

/// Convenience wrapper mapping [`set`]'s table-exhaustion sentinel to `ENOLCK`
/// and any real conflict to `err_conflict`.
pub fn set_or_errno(
    key: &str,
    range: LockRange,
    kind: LockKind,
    owner: u32,
    err_conflict: i64,
) -> i64 {
    match set(key, range, kind, owner) {
        Ok(()) => 0,
        Err(conflict) if conflict.owner == u32::MAX => ENOLCK,
        Err(_) => err_conflict,
    }
}

/// Release `owner`'s coverage of `range` on one file.
pub fn unlock(key: &str, range: LockRange, owner: u32) {
    let mut locks = LOCKS.lock();
    let Some(records) = locks.get_mut(key) else {
        return;
    };
    clear_owner_range(records, range, owner);
    coalesce(records);
    if records.is_empty() {
        locks.remove(key);
    }
}

/// Release every lock `owner` holds on one file. Classic POSIX close-time
/// behavior: closing any descriptor to a file drops all the process's locks on
/// it, regardless of other open descriptors.
pub fn release_all(key: &str, owner: u32) {
    let mut locks = LOCKS.lock();
    let Some(records) = locks.get_mut(key) else {
        return;
    };
    records.retain(|record| record.owner != owner);
    if records.is_empty() {
        locks.remove(key);
    }
}

/// Release every lock `owner` holds on every file (process/group exit).
/// Returns whether anything was removed, so the caller can skip a wake.
pub fn release_owner(owner: u32) -> bool {
    let mut locks = LOCKS.lock();
    let mut removed = false;
    let mut empty_keys: Vec<String> = Vec::new();
    for (key, records) in locks.iter_mut() {
        let before = records.len();
        records.retain(|record| record.owner != owner);
        if records.len() != before {
            removed = true;
        }
        if records.is_empty() {
            empty_keys.push(key.clone());
        }
    }
    for key in empty_keys {
        locks.remove(&key);
    }
    removed
}

/// Wake every `F_SETLKW` waiter after a release. Bumping the readiness
/// sequence both wakes the waiters (the readiness wake path matches
/// `WaitingForFileLock`) and closes the scan-to-park race for a waiter that
/// sampled the sequence before publishing its blocked state.
pub fn wake_lock_waiters() {
    crate::userland::readiness::notify_changed();
}

#[cfg(feature = "test")]
pub fn record_lock_tests() -> &'static [&'static dyn crate::lib::test_utils::Testable] {
    &[
        &test_whole_file_write_conflict,
        &test_read_locks_compatible,
        &test_read_write_conflict,
        &test_same_owner_replaces,
        &test_unlock_splits_range,
        &test_getlk_reports_conflictor,
        &test_getlk_free_returns_none,
        &test_range_disjoint_no_conflict,
        &test_release_all_one_file,
        &test_release_owner_all_files,
        &test_coalesce_adjacent,
        &test_records_per_file_cap,
    ]
}

#[cfg(feature = "test")]
fn whole_file() -> LockRange {
    LockRange {
        start: 0,
        end: u64::MAX,
    }
}

#[cfg(feature = "test")]
fn reset(key: &str) {
    LOCKS.lock().remove(key);
}

#[cfg(feature = "test")]
fn test_whole_file_write_conflict() {
    let key = "/test/rl-wwc";
    reset(key);
    assert_eq!(set(key, whole_file(), LockKind::Write, 10), Ok(()));
    let conflict = set(key, whole_file(), LockKind::Write, 20).unwrap_err();
    assert_eq!(conflict.owner, 10);
    assert_eq!(conflict.kind, LockKind::Write);
    reset(key);
}

#[cfg(feature = "test")]
fn test_read_locks_compatible() {
    let key = "/test/rl-rr";
    reset(key);
    assert_eq!(set(key, whole_file(), LockKind::Read, 10), Ok(()));
    assert_eq!(set(key, whole_file(), LockKind::Read, 20), Ok(()));
    reset(key);
}

#[cfg(feature = "test")]
fn test_read_write_conflict() {
    let key = "/test/rl-rw";
    reset(key);
    assert_eq!(set(key, whole_file(), LockKind::Read, 10), Ok(()));
    let conflict = set(key, whole_file(), LockKind::Write, 20).unwrap_err();
    assert_eq!(conflict.owner, 10);
    // Same owner upgrading read->write never conflicts with itself.
    assert_eq!(set(key, whole_file(), LockKind::Write, 10), Ok(()));
    reset(key);
}

#[cfg(feature = "test")]
fn test_same_owner_replaces() {
    let key = "/test/rl-replace";
    reset(key);
    assert_eq!(set(key, whole_file(), LockKind::Write, 10), Ok(()));
    // A different owner can lock once 10 downgrades to a compatible read and
    // the ranges are read/read.
    assert_eq!(set(key, whole_file(), LockKind::Read, 10), Ok(()));
    assert_eq!(set(key, whole_file(), LockKind::Read, 20), Ok(()));
    reset(key);
}

#[cfg(feature = "test")]
fn test_unlock_splits_range() {
    let key = "/test/rl-split";
    reset(key);
    // Lock [0, 99], then unlock the middle [40, 59]; expect [0,39] and [60,99].
    assert_eq!(
        set(key, LockRange { start: 0, end: 99 }, LockKind::Write, 10),
        Ok(())
    );
    unlock(key, LockRange { start: 40, end: 59 }, 10);
    // Owner 20 can now take the hole but not the flanks.
    assert_eq!(
        set(key, LockRange { start: 40, end: 59 }, LockKind::Write, 20),
        Ok(())
    );
    assert!(set(key, LockRange { start: 0, end: 10 }, LockKind::Write, 20).is_err());
    assert!(set(key, LockRange { start: 90, end: 99 }, LockKind::Write, 20).is_err());
    reset(key);
}

#[cfg(feature = "test")]
fn test_getlk_reports_conflictor() {
    let key = "/test/rl-getlk";
    reset(key);
    assert_eq!(
        set(key, LockRange { start: 5, end: 15 }, LockKind::Write, 10),
        Ok(())
    );
    let conflict = test(key, LockRange { start: 0, end: 100 }, LockKind::Write, 20).unwrap();
    assert_eq!(conflict.owner, 10);
    assert_eq!(conflict.range.start, 5);
    assert_eq!(conflict.range.end, 15);
    reset(key);
}

#[cfg(feature = "test")]
fn test_getlk_free_returns_none() {
    let key = "/test/rl-free";
    reset(key);
    assert!(test(key, whole_file(), LockKind::Write, 20).is_none());
    // Own lock never conflicts with self.
    assert_eq!(set(key, whole_file(), LockKind::Write, 20), Ok(()));
    assert!(test(key, whole_file(), LockKind::Write, 20).is_none());
    reset(key);
}

#[cfg(feature = "test")]
fn test_range_disjoint_no_conflict() {
    let key = "/test/rl-disjoint";
    reset(key);
    assert_eq!(
        set(key, LockRange { start: 0, end: 9 }, LockKind::Write, 10),
        Ok(())
    );
    assert_eq!(
        set(key, LockRange { start: 10, end: 19 }, LockKind::Write, 20),
        Ok(())
    );
    reset(key);
}

#[cfg(feature = "test")]
fn test_release_all_one_file() {
    let key = "/test/rl-relall";
    reset(key);
    assert_eq!(set(key, whole_file(), LockKind::Write, 10), Ok(()));
    release_all(key, 10);
    assert_eq!(set(key, whole_file(), LockKind::Write, 20), Ok(()));
    reset(key);
}

#[cfg(feature = "test")]
fn test_release_owner_all_files() {
    let a = "/test/rl-owner-a";
    let b = "/test/rl-owner-b";
    reset(a);
    reset(b);
    assert_eq!(set(a, whole_file(), LockKind::Write, 10), Ok(()));
    assert_eq!(set(b, whole_file(), LockKind::Write, 10), Ok(()));
    assert!(release_owner(10));
    assert!(!release_owner(10));
    assert_eq!(set(a, whole_file(), LockKind::Write, 20), Ok(()));
    assert_eq!(set(b, whole_file(), LockKind::Write, 20), Ok(()));
    reset(a);
    reset(b);
}

#[cfg(feature = "test")]
fn test_coalesce_adjacent() {
    let key = "/test/rl-coalesce";
    reset(key);
    assert_eq!(
        set(key, LockRange { start: 0, end: 9 }, LockKind::Write, 10),
        Ok(())
    );
    assert_eq!(
        set(key, LockRange { start: 10, end: 19 }, LockKind::Write, 10),
        Ok(())
    );
    // Adjacent same-owner same-kind ranges must have merged into one record.
    assert_eq!(LOCKS.lock().get(key).map(Vec::len), Some(1));
    reset(key);
}

#[cfg(feature = "test")]
fn test_records_per_file_cap() {
    let key = "/test/rl-cap";
    reset(key);
    // Interleave owners so ranges never coalesce, forcing fragmentation until
    // the per-file cap trips ENOLCK-mapped failure.
    let mut hit_cap = false;
    for i in 0..(MAX_RECORDS_PER_FILE as u64 + 8) {
        let start = i * 4;
        let owner = if i % 2 == 0 { 10 } else { 20 };
        let range = LockRange {
            start,
            end: start + 1,
        };
        if set(key, range, LockKind::Write, owner).is_err() {
            hit_cap = true;
            break;
        }
    }
    assert!(hit_cap, "per-file record cap never tripped");
    reset(key);
}

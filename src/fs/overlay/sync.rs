//! Phase D U11 — overlay persistence via double-buffered binary blob.
//!
//! The original plan called for a directory-tree dump on `/data` with
//! atomic rename-into-place commit (Corrections C-4). That design
//! requires FAT mkdir + rename, which Phase C U10 deferred to a
//! follow-up. This module implements a simpler design that delivers
//! the same crash-safety guarantee using ONLY file create + write +
//! single-byte commit:
//!
//! ```text
//!   /data/overlay-state.0   double-buffered binary blob (slot 0)
//!   /data/overlay-state.1   double-buffered binary blob (slot 1)
//!   /data/overlay-state.ptr 1-byte pointer: ASCII '0' or '1'
//! ```
//!
//! Atomicity: writing the new blob to the inactive slot is non-atomic
//! (a crash mid-write leaves a partial file in that slot — but the
//! pointer still points at the OLD slot, so the partial file is never
//! read). The commit is a single-sector write of the pointer file
//! (1 byte → 1 cluster → 1 sector), which IS atomic at the disk
//! level: the sector either lands fully or not at all.
//!
//! On restore, the pointer is read, the indicated slot is loaded, its
//! CRC32 is validated. If the pointer is missing or invalid, both
//! slots are checked in turn; if neither passes CRC, the upper layer
//! starts empty (loud log, but boot continues).
//!
//! Format:
//! ```text
//!   [magic   4 bytes = b"AGOV"]
//!   [version 1 byte  = 1]
//!   [crc32   4 bytes over everything that follows]
//!   [entry_count u32 LE]
//!   foreach entry:
//!     [kind u8]              0 = file, 1 = whiteout, 2 = opaque-dir marker
//!     [path_len u16 LE]
//!     [path utf8]
//!     if file: [data_len u32 LE][data bytes]
//! ```

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::fs::filesystem::{FileMode, Filesystem, FilesystemError};
use crate::fs::tmpfs::filesystem::{DirBody, TmpNode, Tmpfs};
use spin::Mutex;

const MAGIC: &[u8; 4] = b"AGOV";
const VERSION: u8 = 1;
const KIND_FILE: u8 = 0;
const KIND_WHITEOUT: u8 = 1;
const KIND_OPAQUE: u8 = 2;

const SLOT0_PATH: &str = "/data/overlay-state.0";
const SLOT1_PATH: &str = "/data/overlay-state.1";
const PTR_PATH: &str = "/data/overlay-state.ptr";

// ---------- CRC32 (IEEE polynomial) ----------

/// Compute CRC32 (IEEE 802.3 polynomial) over `data`. Lifted from
/// the reference table-based implementation; ~30 lines, no deps. The
/// MANIFEST uses this as a single integrity checksum.
fn crc32_ieee(data: &[u8]) -> u32 {
    static TABLE: spin::Once<[u32; 256]> = spin::Once::new();
    let table = TABLE.call_once(|| {
        let mut t = [0u32; 256];
        for i in 0..256u32 {
            let mut c = i;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB88320 ^ (c >> 1) } else { c >> 1 };
            }
            t[i as usize] = c;
        }
        t
    });
    let mut crc: u32 = 0xFFFFFFFF;
    for &b in data {
        crc = table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFFFFFF
}

// ---------- Serialization ----------

#[derive(Debug, Clone)]
pub enum Entry {
    File { path: String, data: Vec<u8> },
    Whiteout { path: String },
    Opaque { dir_path: String },
}

/// Walk a tmpfs subtree rooted at `dir` (with the given path
/// prefix), pushing entries into `out`. Whiteout / opaque sentinels
/// (`.wh.*` / `.wh..wh..opq`) are emitted as their semantic Entry
/// variants so a fresh tmpfs hydrated from the dump reproduces the
/// overlay's whiteout state exactly.
fn walk_tmpfs_dir(dir: &DirBody, prefix: &str, out: &mut Vec<Entry>) {
    let children = dir.lock();
    for (name, node) in children.iter() {
        let mut full_path = String::with_capacity(prefix.len() + 1 + name.len());
        full_path.push_str(prefix);
        if !prefix.ends_with('/') {
            full_path.push('/');
        }
        full_path.push_str(name);

        if name == ".wh..wh..opq" {
            out.push(Entry::Opaque {
                dir_path: prefix.to_string(),
            });
            continue;
        }
        if let Some(real_name) = name.strip_prefix(".wh.") {
            // Whiteout marker: the real path being shadowed is
            // <prefix>/<real_name>.
            let mut wh_path = String::with_capacity(prefix.len() + 1 + real_name.len());
            wh_path.push_str(prefix);
            if !prefix.ends_with('/') {
                wh_path.push('/');
            }
            wh_path.push_str(real_name);
            out.push(Entry::Whiteout { path: wh_path });
            continue;
        }
        match node {
            TmpNode::File(body) => {
                let data = body.lock().clone();
                out.push(Entry::File {
                    path: full_path,
                    data,
                });
            }
            TmpNode::Dir(sub) => {
                // Recurse with the new prefix; directories themselves
                // are implicit (the file entries' parent paths
                // identify them).
                walk_tmpfs_dir(sub, &full_path, out);
            }
        }
    }
}

/// Serialize the overlay's upper-layer tmpfs into a binary blob with
/// MANIFEST + CRC32.
pub fn serialize_upper(upper: &Tmpfs) -> Vec<u8> {
    let mut entries = Vec::new();
    walk_tmpfs_dir(&upper.root_dir(), "", &mut entries);

    // Inner payload (everything that the CRC covers, except the CRC
    // and the header).
    let mut inner: Vec<u8> = Vec::new();
    inner.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for entry in &entries {
        match entry {
            Entry::File { path, data } => {
                inner.push(KIND_FILE);
                inner.extend_from_slice(&(path.len() as u16).to_le_bytes());
                inner.extend_from_slice(path.as_bytes());
                inner.extend_from_slice(&(data.len() as u32).to_le_bytes());
                inner.extend_from_slice(data);
            }
            Entry::Whiteout { path } => {
                inner.push(KIND_WHITEOUT);
                inner.extend_from_slice(&(path.len() as u16).to_le_bytes());
                inner.extend_from_slice(path.as_bytes());
            }
            Entry::Opaque { dir_path } => {
                inner.push(KIND_OPAQUE);
                inner.extend_from_slice(&(dir_path.len() as u16).to_le_bytes());
                inner.extend_from_slice(dir_path.as_bytes());
            }
        }
    }
    let crc = crc32_ieee(&inner);

    let mut out: Vec<u8> =
        Vec::with_capacity(MAGIC.len() + 1 + 4 + inner.len());
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&inner);
    out
}

/// Deserialize a blob, validating MAGIC, VERSION, and CRC32. Returns
/// the list of entries on success.
pub fn deserialize_blob(blob: &[u8]) -> Result<Vec<Entry>, &'static str> {
    if blob.len() < MAGIC.len() + 1 + 4 + 4 {
        return Err("blob too short for header");
    }
    if &blob[..4] != MAGIC {
        return Err("bad magic");
    }
    if blob[4] != VERSION {
        return Err("unsupported version");
    }
    let expected_crc = u32::from_le_bytes([blob[5], blob[6], blob[7], blob[8]]);
    let inner = &blob[9..];
    let computed_crc = crc32_ieee(inner);
    if computed_crc != expected_crc {
        return Err("crc mismatch");
    }

    let entry_count = u32::from_le_bytes([inner[0], inner[1], inner[2], inner[3]]);
    let mut p = 4usize;
    let mut entries = Vec::with_capacity(entry_count as usize);
    for _ in 0..entry_count {
        if p >= inner.len() {
            return Err("truncated mid-entry");
        }
        let kind = inner[p];
        p += 1;
        if p + 2 > inner.len() {
            return Err("truncated path_len");
        }
        let path_len = u16::from_le_bytes([inner[p], inner[p + 1]]) as usize;
        p += 2;
        if p + path_len > inner.len() {
            return Err("truncated path");
        }
        let path = match core::str::from_utf8(&inner[p..p + path_len]) {
            Ok(s) => s.to_string(),
            Err(_) => return Err("non-utf8 path"),
        };
        p += path_len;
        match kind {
            KIND_FILE => {
                if p + 4 > inner.len() {
                    return Err("truncated data_len");
                }
                let data_len = u32::from_le_bytes([inner[p], inner[p + 1], inner[p + 2], inner[p + 3]]) as usize;
                p += 4;
                if p + data_len > inner.len() {
                    return Err("truncated data");
                }
                let data = inner[p..p + data_len].to_vec();
                p += data_len;
                entries.push(Entry::File { path, data });
            }
            KIND_WHITEOUT => entries.push(Entry::Whiteout { path }),
            KIND_OPAQUE => entries.push(Entry::Opaque { dir_path: path }),
            _ => return Err("unknown entry kind"),
        }
    }
    Ok(entries)
}

// ---------- Flush + restore ----------

/// Find which of the two slots is currently authoritative. Reads
/// `/data/overlay-state.ptr` (a 1-byte file holding ASCII '0' or
/// '1'). Returns 0 if the file is missing or malformed.
fn read_pointer() -> u8 {
    match crate::fs::File::open_read(PTR_PATH) {
        Ok(f) => {
            let mut buf = [0u8; 1];
            let _ = f.read(&mut buf);
            if buf[0] == b'1' { 1 } else { 0 }
        }
        Err(_) => 0,
    }
}

fn write_pointer(slot: u8) -> Result<(), FilesystemError> {
    let f = crate::fs::File::create(PTR_PATH)
        .map_err(|_| FilesystemError::IoError)?;
    let byte = if slot == 1 { b"1" } else { b"0" };
    f.write(byte).map_err(|_| FilesystemError::IoError)?;
    Ok(())
}

fn slot_path(slot: u8) -> &'static str {
    if slot == 1 { SLOT1_PATH } else { SLOT0_PATH }
}

/// Flush the overlay's upper-layer tmpfs to `/data`. Writes to the
/// INACTIVE slot, then commits via a single-byte pointer flip.
pub fn flush_upper_to_disk(upper: &Tmpfs) -> Result<(), FilesystemError> {
    let blob = serialize_upper(upper);
    let current = read_pointer();
    let target = if current == 0 { 1u8 } else { 0u8 };
    let path = slot_path(target);

    // Write the new blob to the inactive slot.
    let f = crate::fs::File::create(path).map_err(|_| FilesystemError::IoError)?;
    let mut written = 0;
    while written < blob.len() {
        let n = f.write(&blob[written..]).map_err(|_| FilesystemError::IoError)?;
        if n == 0 {
            return Err(FilesystemError::DiskFull);
        }
        written += n;
    }
    drop(f);

    // Atomic commit: flip the pointer (single-byte write).
    write_pointer(target)?;
    Ok(())
}

/// Restore the upper-layer tmpfs from `/data` if a valid blob is
/// present. Walks the indicated slot's entries; on any corruption,
/// tries the other slot; on total failure, returns Ok(()) and leaves
/// `upper` untouched (caller sees an empty upper, which is the same
/// as a fresh boot).
pub fn restore_upper_from_disk(upper: &Tmpfs) -> Result<usize, FilesystemError> {
    // Try the current pointer first, then the other slot.
    let primary = read_pointer();
    let candidates = [primary, if primary == 0 { 1 } else { 0 }];

    for slot in candidates {
        let path = slot_path(slot);
        let entries = match load_slot(path) {
            Ok(e) => e,
            Err(reason) => {
                crate::debug_warn!(
                    "overlay restore: slot {} ({}) rejected: {}",
                    slot,
                    path,
                    reason
                );
                continue;
            }
        };
        crate::debug_info!(
            "overlay restore: loaded slot {} ({} entries)",
            slot,
            entries.len()
        );
        let count = entries.len();
        apply_entries(upper, entries);
        return Ok(count);
    }
    // Neither slot loadable — first boot or corruption. Continue
    // with empty upper.
    crate::debug_info!("overlay restore: no valid slot on /data; starting with empty upper");
    Ok(0)
}

fn load_slot(path: &str) -> Result<Vec<Entry>, &'static str> {
    let f = crate::fs::File::open_read(path).map_err(|_| "open failed")?;
    let blob = f.read_to_vec().map_err(|_| "read failed")?;
    deserialize_blob(&blob)
}

/// Apply deserialized entries to an empty tmpfs (or any tmpfs whose
/// current state should be overwritten). Creates parent directories
/// as needed via tmpfs mkdir.
fn apply_entries(upper: &Tmpfs, entries: Vec<Entry>) {
    for entry in entries {
        match entry {
            Entry::File { path, data } => {
                ensure_parents(upper, &path);
                if let Ok(mut handle) = upper.open(
                    &path,
                    FileMode {
                        read: true,
                        write: true,
                        append: false,
                        create: true,
                        truncate: true,
                    },
                ) {
                    let _ = upper.write(&mut handle, &data);
                    let _ = upper.close(&mut handle);
                }
            }
            Entry::Whiteout { path } => {
                // Re-create the .wh.<name> sentinel at the parent dir.
                let (parent, leaf) = split_parent(&path);
                let wh_path = if parent == "/" {
                    let mut s = String::from("/.wh.");
                    s.push_str(leaf);
                    s
                } else {
                    let mut s = String::with_capacity(parent.len() + 5 + leaf.len());
                    s.push_str(parent);
                    s.push_str("/.wh.");
                    s.push_str(leaf);
                    s
                };
                ensure_parents(upper, &wh_path);
                if let Ok(mut h) = upper.open(
                    &wh_path,
                    FileMode {
                        read: false,
                        write: true,
                        append: false,
                        create: true,
                        truncate: true,
                    },
                ) {
                    let _ = upper.close(&mut h);
                }
            }
            Entry::Opaque { dir_path } => {
                let opaque_path = if dir_path.is_empty() || dir_path == "/" {
                    String::from("/.wh..wh..opq")
                } else {
                    let mut s = String::with_capacity(dir_path.len() + 13);
                    s.push_str(&dir_path);
                    s.push_str("/.wh..wh..opq");
                    s
                };
                ensure_parents(upper, &opaque_path);
                if let Ok(mut h) = upper.open(
                    &opaque_path,
                    FileMode {
                        read: false,
                        write: true,
                        append: false,
                        create: true,
                        truncate: true,
                    },
                ) {
                    let _ = upper.close(&mut h);
                }
            }
        }
    }
}

fn ensure_parents(fs: &Tmpfs, path: &str) {
    let mut acc = String::from("/");
    for comp in path.split('/').filter(|s| !s.is_empty()) {
        // Stop at the LAST component (the file itself).
        // We need to look ahead — but it's easier to just collect.
        let _ = comp;
    }
    // Simpler: split into components, take all but the last, mkdir each.
    let comps: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if comps.len() <= 1 {
        return; // file is at root, no parents needed
    }
    for comp in &comps[..comps.len() - 1] {
        if acc != "/" {
            acc.push('/');
        }
        acc.push_str(comp);
        let _ = fs.mkdir(&acc); // ignore AlreadyExists
    }
}

fn split_parent(path: &str) -> (&str, &str) {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) => ("/", &trimmed[1..]),
        Some(i) => (&trimmed[..i], &trimmed[i + 1..]),
        None => ("/", trimmed),
    }
}

// ---------- Hook on the overlay ----------

use crate::fs::overlay::filesystem::Overlay;

/// Trait extension so Overlay can call into the persistence layer
/// without sync.rs taking a hard dep on Overlay internals. The
/// overlay's `Filesystem::sync` impl invokes this.
pub fn overlay_persistent_sync(overlay: &Overlay) -> Result<(), FilesystemError> {
    // The overlay holds &'static dyn Filesystem for upper. We need to
    // narrow it back to a concrete Tmpfs reference. Use the upper's
    // name() as a sanity check — it's "tmpfs" for the standard root
    // overlay; any non-tmpfs upper skips persistence (no-op).
    let upper_dyn = overlay.upper();
    if upper_dyn.name() != "tmpfs" {
        return Ok(());
    }
    // SAFETY: We control the construction site (vfs::mount_overlay_root)
    // and put a Tmpfs in the upper slot. The name() check is a
    // guard against future variants.
    let upper_ptr = upper_dyn as *const dyn Filesystem as *const Tmpfs;
    let upper: &Tmpfs = unsafe { &*upper_ptr };
    flush_upper_to_disk(upper)
}

// ---------- One-shot mutex to serialize sync calls ----------
// FAT writes are not reentrant-safe; serialize at this layer.
static SYNC_LOCK: Mutex<()> = Mutex::new(());

pub fn synchronized_flush(overlay: &Overlay) -> Result<(), FilesystemError> {
    let _g = SYNC_LOCK.lock();
    overlay_persistent_sync(overlay)
}

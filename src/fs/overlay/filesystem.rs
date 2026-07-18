//! `Filesystem` trait implementation for the overlay.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::fs::filesystem::{
    DirectoryEntry, DirectoryIterator, FileAttributes, FileHandle, FileMode, FileType, Filesystem,
    FilesystemError, FilesystemStats,
};

/// Maximum file size we will copy-up from lower into upper in a
/// single open(O_WRONLY)-on-lower-only-file. Bounds the heap-burst
/// risk identified in doc-review #A-2: a userland call to
/// `open("/HELLOCPP.ELF", O_WRONLY)` would otherwise read the whole
/// 5.79 MiB file into RAM. 64 KiB is comfortable for typical config
/// files; larger files surface `EFBIG` (mapped to `BufferTooSmall`
/// here, translated to `EFBIG` at the syscall boundary).
const MAX_COPY_UP_BYTES: u64 = 64 * 1024;

/// Whiteout sentinel prefix. A file named `.wh.<name>` in upper hides
/// `<name>` from the merged view.
const WHITEOUT_PREFIX: &str = ".wh.";

/// Opaque-directory sentinel. A file with this exact name in an upper
/// directory means "don't show lower contents at all". Used when the
/// user has rmdir'd a lower-backed directory and recreated it fresh.
const OPAQUE_MARKER: &str = ".wh..wh..opq";

/// Overlay handle-id encoding. The high bit distinguishes which layer
/// served the open: upper (1) or lower (0). The low 63 bits carry the
/// underlying FS's handle id. We need this because `Filesystem::read`
/// only sees the `FileHandle` and must route back to the right layer.
const HANDLE_LAYER_BIT: u64 = 1u64 << 63;
const HANDLE_ID_MASK: u64 = !HANDLE_LAYER_BIT;

fn encode_upper(id: u64) -> u64 {
    debug_assert!(
        id & HANDLE_LAYER_BIT == 0,
        "upper handle id collides with layer bit"
    );
    id | HANDLE_LAYER_BIT
}
fn encode_lower(id: u64) -> u64 {
    debug_assert!(
        id & HANDLE_LAYER_BIT == 0,
        "lower handle id collides with layer bit"
    );
    id
}
fn is_upper(h: u64) -> bool {
    h & HANDLE_LAYER_BIT != 0
}
fn raw_id(h: u64) -> u64 {
    h & HANDLE_ID_MASK
}

/// Pair an entry name with its whiteout name in the upper layer.
fn whiteout_name(leaf: &str) -> String {
    let mut s = String::with_capacity(WHITEOUT_PREFIX.len() + leaf.len());
    s.push_str(WHITEOUT_PREFIX);
    s.push_str(leaf);
    s
}

/// Strip a `.wh.` prefix, returning `Some(real_name)` if present and
/// not the opaque marker.
fn strip_whiteout(name: &str) -> Option<&str> {
    if name == OPAQUE_MARKER {
        return None;
    }
    name.strip_prefix(WHITEOUT_PREFIX)
}

/// Split a path into (parent, leaf). For `/foo` returns
/// (Some("/"), Some("foo")). For `/` returns (None, None).
fn split_parent(path: &str) -> (Option<&str>, Option<&str>) {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return (None, None);
    }
    match trimmed.rfind('/') {
        Some(0) => (Some("/"), Some(&trimmed[1..])),
        Some(i) => (Some(&trimmed[..i]), Some(&trimmed[i + 1..])),
        None => (Some("/"), Some(trimmed)),
    }
}

/// Join two path fragments. `parent` is treated as a directory.
fn join(parent: &str, leaf: &str) -> String {
    if parent == "/" {
        let mut s = String::with_capacity(1 + leaf.len());
        s.push('/');
        s.push_str(leaf);
        s
    } else {
        let mut s = String::with_capacity(parent.len() + 1 + leaf.len());
        s.push_str(parent);
        s.push('/');
        s.push_str(leaf);
        s
    }
}

pub struct Overlay {
    upper: &'static dyn Filesystem,
    lower: &'static dyn Filesystem,
}

impl Overlay {
    pub fn new(upper: &'static dyn Filesystem, lower: &'static dyn Filesystem) -> Self {
        Self { upper, lower }
    }

    pub fn upper(&self) -> &'static dyn Filesystem {
        self.upper
    }

    /// Has the parent directory of `path` been marked opaque in upper?
    fn parent_is_opaque(&self, path: &str) -> bool {
        let (parent, _) = split_parent(path);
        match parent {
            Some(p) => {
                let opaque = join(p, OPAQUE_MARKER);
                self.upper.stat(&opaque).is_ok()
            }
            None => false,
        }
    }

    /// Is `path` whiteouted in upper?
    fn is_whiteouted(&self, path: &str) -> bool {
        let (parent, leaf) = split_parent(path);
        match (parent, leaf) {
            (Some(p), Some(l)) => {
                let wh = join(p, &whiteout_name(l));
                self.upper.stat(&wh).is_ok()
            }
            _ => false,
        }
    }

    /// Remove any whiteout marker for `path` (called when re-creating
    /// a previously deleted lower-backed name).
    fn clear_whiteout(&self, path: &str) -> Result<(), FilesystemError> {
        let (parent, leaf) = split_parent(path);
        if let (Some(p), Some(l)) = (parent, leaf) {
            let wh = join(p, &whiteout_name(l));
            if self.upper.stat(&wh).is_ok() {
                self.upper.unlink(&wh)?;
            }
        }
        Ok(())
    }

    /// Ensure that `path`'s parent directory exists in upper. Creates
    /// it (and ancestors) as needed. For "/" this is a no-op.
    fn ensure_upper_parent(&self, path: &str) -> Result<(), FilesystemError> {
        let (parent, _) = split_parent(path);
        let parent = match parent {
            Some(p) => p,
            None => return Ok(()),
        };
        if parent == "/" {
            return Ok(());
        }
        self.mkdir_p_upper(parent)
    }

    /// `mkdir -p` in upper: create `path` and any missing ancestors.
    /// If a component already exists as a directory in upper, skip
    /// it. If a component exists as a file, fail. Lower-side
    /// directories are NOT consulted — copying them into upper is
    /// done implicitly here by issuing `mkdir` (overlay doesn't need
    /// to clone metadata since lower entries remain visible via the
    /// merged readdir).
    fn mkdir_p_upper(&self, path: &str) -> Result<(), FilesystemError> {
        let mut acc = String::from("/");
        for comp in path.split('/').filter(|s| !s.is_empty()) {
            if acc != "/" {
                acc.push('/');
            }
            acc.push_str(comp);
            match self.upper.stat(&acc) {
                Ok(e) if e.file_type == FileType::Directory => continue,
                Ok(_) => return Err(FilesystemError::NotADirectory),
                Err(FilesystemError::NotFound) => {
                    self.upper.mkdir(&acc)?;
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// Copy `path` from lower into upper, with a size cap. Returns
    /// `Ok(())` on success (file now present in upper), or
    /// `BufferTooSmall` for oversized files.
    fn copy_up(&self, path: &str) -> Result<(), FilesystemError> {
        let meta = self.lower.stat(path)?;
        if meta.file_type == FileType::Directory {
            return Err(FilesystemError::IsADirectory);
        }
        if meta.size > MAX_COPY_UP_BYTES {
            return Err(FilesystemError::BufferTooSmall);
        }
        self.ensure_upper_parent(path)?;
        // Clear any whiteout so the new upper file is visible.
        self.clear_whiteout(path)?;

        // Read from lower.
        let mut lower_handle = self.lower.open(path, FileMode::READ)?;
        let size = meta.size as usize;
        let mut buf = alloc::vec![0u8; size];
        let mut total = 0;
        while total < size {
            let n = self.lower.read(&mut lower_handle, &mut buf[total..])?;
            if n == 0 {
                break;
            }
            total += n;
        }
        let _ = self.lower.close(&mut lower_handle);
        buf.truncate(total);

        // Write into upper as a fresh create.
        let mut upper_handle = self.upper.open(
            path,
            FileMode {
                read: false,
                write: true,
                append: false,
                create: true,
                truncate: true,
            },
        )?;
        let mut written = 0;
        while written < buf.len() {
            let n = self.upper.write(&mut upper_handle, &buf[written..])?;
            if n == 0 {
                return Err(FilesystemError::IoError);
            }
            written += n;
        }
        let _ = self.upper.close(&mut upper_handle);
        Ok(())
    }

    /// Look up `path`, returning which layer answers and the
    /// underlying entry. Returns `NotFound` if upper whiteouts hide
    /// the lower entry.
    fn locate(&self, path: &str) -> Result<(Layer, DirectoryEntry), FilesystemError> {
        // Upper wins.
        match self.upper.stat(path) {
            Ok(e) => return Ok((Layer::Upper, e)),
            Err(FilesystemError::NotFound) => {}
            Err(e) => return Err(e),
        }
        if self.is_whiteouted(path) || self.parent_is_opaque(path) {
            return Err(FilesystemError::NotFound);
        }
        let e = self.lower.stat(path)?;
        Ok((Layer::Lower, e))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layer {
    Upper,
    Lower,
}

impl Filesystem for Overlay {
    fn name(&self) -> &str {
        "overlay"
    }

    fn is_read_only(&self) -> bool {
        self.upper.is_read_only()
    }

    fn stats(&self) -> Result<FilesystemStats, FilesystemError> {
        // Combine; tmpfs stats are mostly zero. Surface lower's
        // block_size as a reasonable hint.
        let lower_stats = self.lower.stats().unwrap_or(FilesystemStats {
            total_blocks: 0,
            free_blocks: 0,
            block_size: 1,
            total_inodes: 0,
            free_inodes: 0,
        });
        Ok(lower_stats)
    }

    fn read_dir(&self, _path: &str) -> Result<DirectoryIterator<'_>, FilesystemError> {
        Err(FilesystemError::UnsupportedOperation)
    }

    fn enumerate_dir(&self, path: &str) -> Result<Vec<DirectoryEntry>, FilesystemError> {
        // Path must resolve to a directory in at least one layer.
        let (layer, _meta) = self.locate(path).or_else(|e| match e {
            FilesystemError::NotFound if path == "/" => Ok((
                Layer::Lower,
                DirectoryEntry {
                    name: [0; 256],
                    name_len: 0,
                    file_type: FileType::Directory,
                    size: 0,
                    attributes: FileAttributes {
                        read_only: false,
                        hidden: false,
                        system: false,
                        archive: false,
                    },
                    created: 0,
                    modified: 0,
                    accessed: 0,
                },
            )),
            _ => Err(e),
        })?;
        let _ = layer;

        // Pull upper entries (if upper has the dir).
        let upper_entries = match self.upper.enumerate_dir(path) {
            Ok(e) => e,
            Err(FilesystemError::NotFound) => Vec::new(),
            Err(e) => return Err(e),
        };

        // Build a set of names that upper either owns or whiteouts.
        let mut whiteouted: alloc::collections::BTreeSet<String> =
            alloc::collections::BTreeSet::new();
        let mut owned: alloc::collections::BTreeSet<String> = alloc::collections::BTreeSet::new();
        let mut opaque = false;
        let mut merged: Vec<DirectoryEntry> = Vec::new();

        for e in &upper_entries {
            let name = e.name_str();
            if name == OPAQUE_MARKER {
                opaque = true;
                continue;
            }
            if let Some(real) = strip_whiteout(name) {
                whiteouted.insert(real.to_string());
                continue;
            }
            owned.insert(name.to_string());
            merged.push(e.clone());
        }

        if !opaque {
            let lower_entries = match self.lower.enumerate_dir(path) {
                Ok(e) => e,
                Err(FilesystemError::NotFound) => Vec::new(),
                Err(_) => Vec::new(),
            };
            for e in lower_entries {
                let name = e.name_str().to_string();
                if owned.contains(&name) || whiteouted.contains(&name) {
                    continue;
                }
                merged.push(e);
            }
        }

        Ok(merged)
    }

    fn stat(&self, path: &str) -> Result<DirectoryEntry, FilesystemError> {
        self.locate(path).map(|(_, e)| e)
    }

    fn open(&self, path: &str, mode: FileMode) -> Result<FileHandle, FilesystemError> {
        let want_write = mode.write || mode.create || mode.truncate || mode.append;

        // Look up via the merged view first.
        let upper_present = self.upper.stat(path).is_ok();
        let whiteouted = self.is_whiteouted(path) || self.parent_is_opaque(path);
        let lower_present = !whiteouted && self.lower.stat(path).is_ok();

        if !want_write {
            // Read-only open: upper > whiteout-block > lower.
            if upper_present {
                let h = self.upper.open(path, mode)?;
                return Ok(FileHandle {
                    inode: encode_upper(h.inode),
                    position: h.position,
                    size: h.size,
                    mode: h.mode,
                });
            }
            if whiteouted {
                return Err(FilesystemError::NotFound);
            }
            let h = self.lower.open(path, mode)?;
            return Ok(FileHandle {
                inode: encode_lower(h.inode),
                position: h.position,
                size: h.size,
                mode: h.mode,
            });
        }

        // Write-side open. If the file is only in lower, copy-up.
        if upper_present {
            self.ensure_upper_parent(path)?;
            let h = self.upper.open(path, mode)?;
            return Ok(FileHandle {
                inode: encode_upper(h.inode),
                position: h.position,
                size: h.size,
                mode: h.mode,
            });
        }
        if lower_present {
            self.copy_up(path)?;
            let h = self.upper.open(path, mode)?;
            return Ok(FileHandle {
                inode: encode_upper(h.inode),
                position: h.position,
                size: h.size,
                mode: h.mode,
            });
        }
        // Neither layer has it. If create is set, create in upper.
        if mode.create {
            self.ensure_upper_parent(path)?;
            self.clear_whiteout(path)?;
            let h = self.upper.open(path, mode)?;
            return Ok(FileHandle {
                inode: encode_upper(h.inode),
                position: h.position,
                size: h.size,
                mode: h.mode,
            });
        }
        Err(FilesystemError::NotFound)
    }

    fn close(&self, handle: &mut FileHandle) -> Result<(), FilesystemError> {
        let raw = raw_id(handle.inode);
        let mut inner = FileHandle {
            inode: raw,
            position: handle.position,
            size: handle.size,
            mode: handle.mode,
        };
        if is_upper(handle.inode) {
            self.upper.close(&mut inner)
        } else {
            self.lower.close(&mut inner)
        }
    }

    fn read(&self, handle: &mut FileHandle, buffer: &mut [u8]) -> Result<usize, FilesystemError> {
        let upper = is_upper(handle.inode);
        let mut inner = FileHandle {
            inode: raw_id(handle.inode),
            position: handle.position,
            size: handle.size,
            mode: handle.mode,
        };
        let n = if upper {
            self.upper.read(&mut inner, buffer)?
        } else {
            self.lower.read(&mut inner, buffer)?
        };
        handle.position = inner.position;
        handle.size = inner.size;
        Ok(n)
    }

    fn write(&self, handle: &mut FileHandle, buffer: &[u8]) -> Result<usize, FilesystemError> {
        if !is_upper(handle.inode) {
            return Err(FilesystemError::ReadOnly);
        }
        let mut inner = FileHandle {
            inode: raw_id(handle.inode),
            position: handle.position,
            size: handle.size,
            mode: handle.mode,
        };
        let n = self.upper.write(&mut inner, buffer)?;
        handle.position = inner.position;
        handle.size = inner.size;
        Ok(n)
    }

    fn seek(&self, handle: &mut FileHandle, position: u64) -> Result<u64, FilesystemError> {
        let upper = is_upper(handle.inode);
        let mut inner = FileHandle {
            inode: raw_id(handle.inode),
            position: handle.position,
            size: handle.size,
            mode: handle.mode,
        };
        let p = if upper {
            self.upper.seek(&mut inner, position)?
        } else {
            self.lower.seek(&mut inner, position)?
        };
        handle.position = inner.position;
        Ok(p)
    }

    fn truncate(&self, handle: &mut FileHandle, size: u64) -> Result<(), FilesystemError> {
        if !is_upper(handle.inode) {
            return Err(FilesystemError::ReadOnly);
        }
        let mut inner = FileHandle {
            inode: raw_id(handle.inode),
            position: handle.position,
            size: handle.size,
            mode: handle.mode,
        };
        self.upper.truncate(&mut inner, size)?;
        handle.position = inner.position;
        handle.size = inner.size;
        Ok(())
    }

    fn handle_metadata(
        &self,
        handle: &FileHandle,
    ) -> Result<crate::fs::filesystem::UnixMetadata, FilesystemError> {
        let inner = FileHandle {
            inode: raw_id(handle.inode),
            position: handle.position,
            size: handle.size,
            mode: handle.mode,
        };
        if is_upper(handle.inode) {
            self.upper.handle_metadata(&inner)
        } else {
            self.lower.handle_metadata(&inner)
        }
    }

    fn sync_handle(&self, handle: &FileHandle, data_only: bool) -> Result<(), FilesystemError> {
        let inner = FileHandle {
            inode: raw_id(handle.inode),
            position: handle.position,
            size: handle.size,
            mode: handle.mode,
        };
        if is_upper(handle.inode) {
            self.upper.sync_handle(&inner, data_only)
        } else {
            self.lower.sync_handle(&inner, data_only)
        }
    }

    fn mkdir(&self, path: &str) -> Result<(), FilesystemError> {
        // Check if anything is already at this path in the merged
        // view (other than a stale whiteout).
        if self.upper.stat(path).is_ok() {
            return Err(FilesystemError::AlreadyExists);
        }
        let lower_present = !self.is_whiteouted(path)
            && !self.parent_is_opaque(path)
            && self.lower.stat(path).is_ok();
        if lower_present {
            return Err(FilesystemError::AlreadyExists);
        }
        self.ensure_upper_parent(path)?;
        self.clear_whiteout(path)?;
        self.upper.mkdir(path)
    }

    fn unlink(&self, path: &str) -> Result<(), FilesystemError> {
        let upper_present = self.upper.stat(path).is_ok();
        let lower_present = self.lower.stat(path).is_ok();
        let whiteouted = self.is_whiteouted(path) || self.parent_is_opaque(path);

        if !upper_present && (whiteouted || !lower_present) {
            return Err(FilesystemError::NotFound);
        }

        if upper_present {
            self.upper.unlink(path)?;
        }
        // If lower has the same name, lay down a whiteout so it stays
        // invisible.
        if lower_present && !whiteouted {
            let (parent, leaf) = split_parent(path);
            if let (Some(p), Some(l)) = (parent, leaf) {
                self.ensure_upper_parent(path)?;
                let wh = join(p, &whiteout_name(l));
                let mut h = self.upper.open(
                    &wh,
                    FileMode {
                        read: false,
                        write: true,
                        append: false,
                        create: true,
                        truncate: true,
                    },
                )?;
                let _ = self.upper.close(&mut h);
            }
        }
        Ok(())
    }

    fn rmdir(&self, path: &str) -> Result<(), FilesystemError> {
        // The merged dir must be empty.
        let entries = self.enumerate_dir(path)?;
        if !entries.is_empty() {
            return Err(FilesystemError::NotEmpty);
        }
        let upper_present = self.upper.stat(path).is_ok();
        let lower_present = self.lower.stat(path).is_ok()
            && !self.is_whiteouted(path)
            && !self.parent_is_opaque(path);
        if upper_present {
            // Best effort: empty merged ≠ empty upper because lower
            // contents that were whiteouted still count in upper. So
            // unlink whiteouts inside this dir before rmdir.
            let upper_inner = self.upper.enumerate_dir(path).unwrap_or_default();
            for e in upper_inner {
                let name = e.name_str();
                if name == OPAQUE_MARKER || name.starts_with(WHITEOUT_PREFIX) {
                    let p = join(path, name);
                    let _ = self.upper.unlink(&p);
                }
            }
            self.upper.rmdir(path)?;
        }
        if lower_present {
            // Lay down a whiteout AND an opaque marker so re-creates
            // start fresh.
            let (parent, leaf) = split_parent(path);
            if let (Some(p), Some(l)) = (parent, leaf) {
                self.ensure_upper_parent(path)?;
                let wh = join(p, &whiteout_name(l));
                let mut h = self.upper.open(
                    &wh,
                    FileMode {
                        read: false,
                        write: true,
                        append: false,
                        create: true,
                        truncate: true,
                    },
                )?;
                let _ = self.upper.close(&mut h);
            }
        }
        Ok(())
    }

    fn rename(&self, old_path: &str, new_path: &str) -> Result<(), FilesystemError> {
        // Resolve source: must exist in merged view.
        let (src_layer, _) = self.locate(old_path)?;
        if let Layer::Lower = src_layer {
            // Copy-up the source first so the rename happens entirely
            // in upper.
            self.copy_up(old_path)?;
        }
        self.ensure_upper_parent(new_path)?;
        self.clear_whiteout(new_path)?;
        self.upper.rename(old_path, new_path)?;
        // If lower has the source name, lay a whiteout so it doesn't
        // resurface.
        if self.lower.stat(old_path).is_ok() && !self.is_whiteouted(old_path) {
            let (parent, leaf) = split_parent(old_path);
            if let (Some(p), Some(l)) = (parent, leaf) {
                let wh = join(p, &whiteout_name(l));
                let mut h = self.upper.open(
                    &wh,
                    FileMode {
                        read: false,
                        write: true,
                        append: false,
                        create: true,
                        truncate: true,
                    },
                )?;
                let _ = self.upper.close(&mut h);
            }
        }
        Ok(())
    }

    fn sync(&self) -> Result<(), FilesystemError> {
        // Persist the upper layer to /data when /data is writable.
        // Errors from the persistence path (no /data mount, /data
        // read-only, /data full) are logged but not propagated —
        // sync() shouldn't fail on the in-RAM side.
        if let Err(e) = crate::fs::overlay::sync::synchronized_flush(self) {
            crate::debug_warn!("overlay sync: flush to /data failed: {:?}", e);
        }
        self.upper.sync()
    }
}

#[cfg(feature = "test")]
mod tests {
    use super::*;
    use crate::fs::tmpfs::Tmpfs;
    use crate::lib::test_utils::Testable;
    use spin::Mutex;

    // Tests build their own upper+lower fixtures. The Filesystem
    // trait requires &'static dyn Filesystem for the overlay
    // constructor; we stash fixtures in module-local statics. Tests
    // run serially under the kernel runner so the singleton pattern
    // is safe.
    static FIXTURE: Mutex<Option<&'static Tmpfs>> = Mutex::new(None);
    static FIXTURE2: Mutex<Option<&'static Tmpfs>> = Mutex::new(None);

    fn leak_tmpfs() -> &'static Tmpfs {
        let boxed = alloc::boxed::Box::new(Tmpfs::new());
        alloc::boxed::Box::leak(boxed)
    }

    fn make_fixture() -> (&'static Tmpfs, &'static Tmpfs) {
        let mut a = FIXTURE.lock();
        let mut b = FIXTURE2.lock();
        if a.is_none() {
            *a = Some(leak_tmpfs());
            *b = Some(leak_tmpfs());
        }
        // Wipe both fixtures by re-leaking. (Cheap; test fixtures
        // aren't size-critical and run serially.)
        let upper = leak_tmpfs();
        let lower = leak_tmpfs();
        *a = Some(upper);
        *b = Some(lower);
        (upper, lower)
    }

    /// Ensure parent directories of `path` exist, recursively. Test
    /// helper — production code uses `Overlay::ensure_upper_parent`.
    fn ensure_parents(fs: &dyn Filesystem, path: &str) {
        let trimmed = path.trim_end_matches('/');
        if let Some(idx) = trimmed.rfind('/') {
            let parent = &trimmed[..idx];
            if parent.is_empty() {
                return;
            }
            ensure_parents(fs, parent);
            let _ = fs.mkdir(parent);
        }
    }

    fn write_file(fs: &dyn Filesystem, path: &str, content: &[u8]) {
        ensure_parents(fs, path);
        let mut h = fs
            .open(
                path,
                FileMode {
                    read: true,
                    write: true,
                    append: false,
                    create: true,
                    truncate: true,
                },
            )
            .expect("open create");
        let mut total = 0;
        while total < content.len() {
            let n = fs.write(&mut h, &content[total..]).expect("write");
            assert!(n > 0);
            total += n;
        }
        fs.close(&mut h).expect("close");
    }

    fn read_file(fs: &dyn Filesystem, path: &str) -> Vec<u8> {
        let mut h = fs.open(path, FileMode::READ).expect("open read");
        let mut out = Vec::new();
        let mut buf = [0u8; 256];
        loop {
            let n = fs.read(&mut h, &mut buf).expect("read");
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
        }
        fs.close(&mut h).expect("close");
        out
    }

    fn test_overlay_reads_lower_passthrough() {
        let (upper, lower) = make_fixture();
        write_file(lower, "/etc/passwd", b"root:x:0:0");
        let o = Overlay::new(upper, lower);
        let content = read_file(&o, "/etc/passwd");
        assert_eq!(content, b"root:x:0:0");
    }

    fn test_overlay_write_triggers_copy_up() {
        let (upper, lower) = make_fixture();
        write_file(lower, "/etc/foo", b"original");
        let o = Overlay::new(upper, lower);

        // Open for write — overlay should copy-up.
        let mut h = o
            .open(
                "/etc/foo",
                FileMode {
                    read: true,
                    write: true,
                    append: false,
                    create: false,
                    truncate: true,
                },
            )
            .expect("open write");
        o.write(&mut h, b"modified").expect("write");
        o.close(&mut h).expect("close");

        // Overlay read returns the new content.
        assert_eq!(read_file(&o, "/etc/foo"), b"modified");
        // Lower file is untouched.
        assert_eq!(read_file(lower, "/etc/foo"), b"original");
    }

    fn test_overlay_unlink_lower_only_creates_whiteout() {
        let (upper, lower) = make_fixture();
        write_file(lower, "/etc/foo", b"lower");
        let o = Overlay::new(upper, lower);
        o.unlink("/etc/foo").expect("unlink");
        assert!(matches!(o.stat("/etc/foo"), Err(FilesystemError::NotFound)));
        // Whiteout exists in upper.
        assert!(upper.stat("/etc/.wh.foo").is_ok());
        // Lower still has the file.
        assert!(lower.stat("/etc/foo").is_ok());
    }

    fn test_overlay_recreate_after_whiteout() {
        let (upper, lower) = make_fixture();
        write_file(lower, "/etc/foo", b"lower");
        let o = Overlay::new(upper, lower);
        o.unlink("/etc/foo").expect("unlink");
        write_file(&o, "/etc/foo", b"fresh");
        assert_eq!(read_file(&o, "/etc/foo"), b"fresh");
        // Whiteout should be gone.
        assert!(matches!(
            upper.stat("/etc/.wh.foo"),
            Err(FilesystemError::NotFound)
        ));
    }

    fn test_overlay_mkdir_only_in_upper() {
        let (upper, lower) = make_fixture();
        let o = Overlay::new(upper, lower);
        o.mkdir("/var").expect("mkdir");
        assert!(upper.stat("/var").is_ok());
        assert!(matches!(lower.stat("/var"), Err(FilesystemError::NotFound)));
    }

    fn test_overlay_readdir_merges_layers() {
        let (upper, lower) = make_fixture();
        write_file(lower, "/etc/a", b"a");
        write_file(lower, "/etc/b", b"b");
        let o = Overlay::new(upper, lower);
        // Add a file only in upper.
        write_file(&o, "/etc/c", b"c");

        let entries = o.enumerate_dir("/etc").expect("readdir");
        let mut names: alloc::vec::Vec<String> =
            entries.iter().map(|e| e.name_str().to_string()).collect();
        names.sort();
        assert_eq!(
            names,
            alloc::vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    fn test_overlay_readdir_skips_whiteouts() {
        let (upper, lower) = make_fixture();
        write_file(lower, "/etc/a", b"a");
        write_file(lower, "/etc/b", b"b");
        let o = Overlay::new(upper, lower);
        o.unlink("/etc/a").expect("unlink a");

        let entries = o.enumerate_dir("/etc").expect("readdir");
        let mut names: alloc::vec::Vec<String> =
            entries.iter().map(|e| e.name_str().to_string()).collect();
        names.sort();
        assert_eq!(names, alloc::vec!["b".to_string()]);
    }

    fn test_overlay_copy_up_size_cap() {
        let (upper, lower) = make_fixture();
        // Write a file larger than MAX_COPY_UP_BYTES.
        let big = alloc::vec![0xABu8; (MAX_COPY_UP_BYTES + 1) as usize];
        write_file(lower, "/big", &big);
        let o = Overlay::new(upper, lower);
        // Read-only open works.
        let _h = o.open("/big", FileMode::READ).expect("open read");
        // Write-side open exceeds the cap.
        assert!(matches!(
            o.open(
                "/big",
                FileMode {
                    read: false,
                    write: true,
                    append: false,
                    create: false,
                    truncate: true,
                },
            ),
            Err(FilesystemError::BufferTooSmall)
        ));
    }

    fn test_overlay_rename_within_upper() {
        let (upper, lower) = make_fixture();
        let o = Overlay::new(upper, lower);
        write_file(&o, "/a", b"data");
        o.rename("/a", "/b").expect("rename");
        assert!(matches!(o.stat("/a"), Err(FilesystemError::NotFound)));
        assert_eq!(read_file(&o, "/b"), b"data");
    }

    fn test_overlay_rename_lower_only_copies_up() {
        let (upper, lower) = make_fixture();
        write_file(lower, "/a", b"data");
        let o = Overlay::new(upper, lower);
        o.rename("/a", "/b").expect("rename cross-copy-up");
        assert!(matches!(o.stat("/a"), Err(FilesystemError::NotFound)));
        assert_eq!(read_file(&o, "/b"), b"data");
        // Lower still has /a, but it's whiteouted in the merged view.
        assert!(lower.stat("/a").is_ok());
    }

    fn test_overlay_is_read_only_reflects_upper() {
        let (upper, lower) = make_fixture();
        let o = Overlay::new(upper, lower);
        assert!(!o.is_read_only());
    }

    pub fn get_tests() -> &'static [&'static dyn Testable] {
        &[
            &test_overlay_reads_lower_passthrough,
            &test_overlay_write_triggers_copy_up,
            &test_overlay_unlink_lower_only_creates_whiteout,
            &test_overlay_recreate_after_whiteout,
            &test_overlay_mkdir_only_in_upper,
            &test_overlay_readdir_merges_layers,
            &test_overlay_readdir_skips_whiteouts,
            &test_overlay_copy_up_size_cap,
            &test_overlay_rename_within_upper,
            &test_overlay_rename_lower_only_copies_up,
            &test_overlay_is_read_only_reflects_upper,
        ]
    }
}

#[cfg(feature = "test")]
pub use tests::get_tests as overlay_tests;

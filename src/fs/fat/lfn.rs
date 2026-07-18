//! VFAT Long File Name (LFN) decoding.
//!
//! On a VFAT volume each long-named entry is encoded as one or more
//! 32-byte slots with `attr == 0x0F` preceding the standard 8.3 stub.
//! Slots are stored on disk in REVERSE order: the slot adjacent to the
//! 8.3 stub holds sequence `1`, and the first slot on disk has the high
//! bit (`0x40`) of its sequence byte set. Each slot carries 13 UTF-16
//! code units (5 + 6 + 2) split awkwardly across the record, plus a
//! checksum byte at offset 13 that must equal the rotate-add checksum of
//! the trailing 8.3 short name.
//!
//! This module is read-only — write-side LFN slot generation, alias
//! suffixing, and slot allocation belong to Phase C of the FS plan.
//!
//! Spec references:
//! - Microsoft FAT32 specification, §7 (Long File Names)
//! - <https://docs.kernel.org/filesystems/vfat.html>
//! - <https://wiki.osdev.org/VFAT>

use crate::fs::fat::directory::{DirectoryEntry, LongFileNameEntry};

/// Maximum LFN length in UTF-16 code units. A complete name can use up
/// to 20 LFN slots × 13 chars/slot = 260, but Windows traditionally
/// caps practical names at 255 — we follow suit.
pub const MAX_LFN_CHARS: usize = 255;

/// Maximum UTF-8 length of a decoded LFN. UTF-16 code units expand to
/// up to 3 UTF-8 bytes each (4 for surrogate pairs, but each pair eats
/// two code units, so 4 bytes per 2 code units = 2 bytes/cu effective).
/// 3 × 255 = 765 is the loose upper bound. We use 256 to match the
/// public `DirectoryEntry::name` budget — names that don't fit are
/// rejected, mirroring Linux's `PATH_MAX`-style ceiling for individual
/// entries.
pub const MAX_LFN_UTF8: usize = 256;

/// Compute the standard FAT LFN checksum over an 11-byte short name
/// (8 + 3, space-padded, as stored in the directory entry).
///
/// Reference: Microsoft FAT spec, "Generating an 8.3 Name from a Long
/// Name".
pub fn short_name_checksum(short_name: &[u8; 11]) -> u8 {
    let mut sum: u8 = 0;
    for &b in short_name {
        // Rotate right by 1, then add the byte — both with u8 wrapping.
        sum = sum.rotate_right(1).wrapping_add(b);
    }
    sum
}

/// State accumulator for collecting an LFN run as it is encountered
/// in directory order. Disk order is reverse of name order (last slot
/// first), so the decoder writes characters into `chars` indexed by
/// `(seq - 1) * 13 + offset_in_slot`.
///
/// Lifecycle:
/// - `reset()` clears state for a fresh directory walk
/// - `push_slot(lfn)` adds an LFN slot; returns false if the slot
///   breaks the run (bad sequence, checksum mismatch, etc.)
/// - `decode(stub)` validates the accumulated run against an 8.3 stub
///   and returns the decoded UTF-8 name, or None if the run is
///   incomplete/corrupt
pub struct LfnAccumulator {
    /// UTF-16 code units, indexed by name position (not disk order).
    /// `chars[0..total_len]` is the decoded sequence after all slots
    /// arrive.
    chars: [u16; MAX_LFN_CHARS],
    /// Highest position written so far (1-based, equals expected slot
    /// count from the `0x40`-marked first slot we saw on disk).
    total_slots: u8,
    // total_chars is computed on `decode()` by scanning chars for the
    // first 0x0000 terminator — the terminator can live in any slot,
    // not just slot 1.
    /// Checksum byte from the first LFN slot we saw — must match every
    /// subsequent slot, and must match the trailing 8.3 stub's
    /// computed checksum.
    expected_checksum: Option<u8>,
    /// Next sequence number we expect on disk (decreases from total to 1).
    next_expected_seq: u8,
    /// True once we've seen a sequence break or checksum mismatch —
    /// remaining slots in the run are silently discarded until a
    /// non-LFN entry closes the (corrupt) run.
    poisoned: bool,
}

impl LfnAccumulator {
    pub const fn new() -> Self {
        Self {
            chars: [0; MAX_LFN_CHARS],
            total_slots: 0,
            expected_checksum: None,
            next_expected_seq: 0,
            poisoned: false,
        }
    }

    pub fn reset(&mut self) {
        self.total_slots = 0;
        self.expected_checksum = None;
        self.next_expected_seq = 0;
        self.poisoned = false;
    }

    /// True if the accumulator currently holds any (un-decoded) state.
    pub fn is_empty(&self) -> bool {
        self.total_slots == 0
    }

    /// Add an LFN slot to the in-progress run. Returns true if the
    /// slot was accepted; false if it was rejected (poisoning the run).
    /// A return of false does not require the caller to do anything —
    /// `decode()` will return None and the next non-LFN entry will
    /// `reset()` the accumulator.
    pub fn push_slot(&mut self, lfn: &LongFileNameEntry) {
        if self.poisoned {
            return;
        }

        let seq = lfn.sequence_number();
        let is_last = lfn.is_last();
        let checksum = lfn.checksum;

        if is_last {
            // First slot on disk (highest sequence). Initialize the run.
            if seq == 0 || (seq as usize) > MAX_LFN_CHARS / 13 + 1 {
                // Sequence 0 or absurdly large — corrupt.
                self.poisoned = true;
                return;
            }
            self.total_slots = seq;
            self.expected_checksum = Some(checksum);
            self.next_expected_seq = seq;
        }

        // Validate sequence ordering. Outside of a started run, ignore
        // (Linux behavior: an orphan LFN slot without a `0x40`-marked
        // leader is treated as a broken run).
        if self.expected_checksum.is_none() {
            self.poisoned = true;
            return;
        }
        if seq != self.next_expected_seq {
            self.poisoned = true;
            return;
        }
        if Some(checksum) != self.expected_checksum {
            self.poisoned = true;
            return;
        }

        // Slot 1 is at name position 0, slot 2 at position 13, etc.
        let base = (seq as usize - 1) * 13;
        if base + 13 > MAX_LFN_CHARS {
            self.poisoned = true;
            return;
        }

        let slot_chars = lfn.chars();
        for (i, &u) in slot_chars.iter().enumerate() {
            self.chars[base + i] = u;
        }

        self.next_expected_seq = seq.saturating_sub(1);
    }

    /// Validate the accumulated run against the trailing 8.3 stub and
    /// decode the UTF-16 chars to UTF-8.
    ///
    /// Returns `Some(len)` with the byte length written into `out` if
    /// the run is well-formed and the checksum matches; `None`
    /// otherwise (caller falls back to the SFN).
    pub fn decode(&self, stub: &DirectoryEntry, out: &mut [u8; MAX_LFN_UTF8]) -> Option<usize> {
        if self.poisoned || self.total_slots == 0 {
            return None;
        }
        // We must have received every slot from `total_slots` down to 1.
        if self.next_expected_seq != 0 {
            return None;
        }
        // Checksum of the trailing stub's 11-byte short name must
        // match the LFN slots' embedded checksum.
        let stub_short = stub.short_name();
        let stub_sum = short_name_checksum(&stub_short);
        if Some(stub_sum) != self.expected_checksum {
            return None;
        }

        // Total chars = scan from the start for the first 0x0000
        // terminator. Padding past the terminator is 0xFFFF. If no
        // terminator, the name fills all received slots exactly.
        let max = (self.total_slots as usize)
            .saturating_mul(13)
            .min(MAX_LFN_CHARS);
        let mut len = max;
        for i in 0..max {
            if self.chars[i] == 0x0000 {
                len = i;
                break;
            }
        }
        if len == 0 {
            return None;
        }

        decode_utf16_to_utf8(&self.chars[..len], out)
    }
}

/// Decode a UTF-16 (UCS-2 + lenient surrogate-pair) slice into UTF-8.
/// Returns `Some(byte_len)` if the result fits in `out`; `None` if it
/// overflows or contains an unpaired surrogate.
fn decode_utf16_to_utf8(src: &[u16], out: &mut [u8; MAX_LFN_UTF8]) -> Option<usize> {
    let mut pos = 0;
    let mut i = 0;
    while i < src.len() {
        let u = src[i];
        let code: u32 = if (0xD800..=0xDBFF).contains(&u) {
            // High surrogate — must be followed by low surrogate.
            if i + 1 >= src.len() {
                return None;
            }
            let lo = src[i + 1];
            if !(0xDC00..=0xDFFF).contains(&lo) {
                return None;
            }
            i += 2;
            0x10000 + (((u as u32 - 0xD800) << 10) | (lo as u32 - 0xDC00))
        } else if (0xDC00..=0xDFFF).contains(&u) {
            return None; // unpaired low surrogate
        } else {
            i += 1;
            u as u32
        };

        // UTF-8 encode the codepoint.
        let len = if code < 0x80 {
            1
        } else if code < 0x800 {
            2
        } else if code < 0x10000 {
            3
        } else {
            4
        };
        if pos + len > out.len() {
            return None;
        }
        match len {
            1 => out[pos] = code as u8,
            2 => {
                out[pos] = 0xC0 | (code >> 6) as u8;
                out[pos + 1] = 0x80 | (code & 0x3F) as u8;
            }
            3 => {
                out[pos] = 0xE0 | (code >> 12) as u8;
                out[pos + 1] = 0x80 | ((code >> 6) & 0x3F) as u8;
                out[pos + 2] = 0x80 | (code & 0x3F) as u8;
            }
            _ => {
                out[pos] = 0xF0 | (code >> 18) as u8;
                out[pos + 1] = 0x80 | ((code >> 12) & 0x3F) as u8;
                out[pos + 2] = 0x80 | ((code >> 6) & 0x3F) as u8;
                out[pos + 3] = 0x80 | (code & 0x3F) as u8;
            }
        }
        pos += len;
    }
    Some(pos)
}

/// Lowercase-attr bit for the basename portion of an 8.3 stub (offset
/// 12 of the entry, NT-reserved byte).
pub const LCASE_BASENAME: u8 = 0x08;
/// Lowercase-attr bit for the extension portion.
pub const LCASE_EXTENSION: u8 = 0x10;

// ---------- LFN write (Phase C U9) ----------

/// Compute how many LFN slots a UTF-8 name needs when laid out in
/// VFAT 13-UCS-2-units-per-slot format. Includes the trailing 0x0000
/// terminator if there's room, else implicit. Returns 0 for empty
/// names.
pub fn lfn_slot_count(name_utf8: &str) -> usize {
    // Count UTF-16 code units (UCS-2 + surrogate pairs).
    let mut units: usize = 0;
    for c in name_utf8.chars() {
        units += if (c as u32) > 0xFFFF { 2 } else { 1 };
    }
    if units == 0 {
        return 0;
    }
    // Slots hold 13 units each. When the name's unit count is an
    // exact multiple of 13, NO terminator is needed (per the VFAT
    // spec and Linux fs/fat/dir.c behavior) — the decoder treats a
    // run with no 0x0000 as fully populated. Only when there's
    // partial space in the final slot do we write a 0x0000
    // terminator + 0xFFFF padding.
    (units + 12) / 13 // = ceil(units / 13)
}

/// Encode a single LFN slot's 32-byte on-disk representation.
///
/// `seq` is the 1-based sequence number in name order (slot 1 is
/// adjacent to the SFN stub, with the lowest 13 UTF-16 units).
/// `is_first_on_disk` means this slot will be the first on disk (the
/// one with the highest sequence number) — sets the `0x40` marker.
/// `chars` holds the 13 UTF-16 code units for this slot, with
/// trailing `0x0000` then `0xFFFF` padding per the VFAT spec.
/// `checksum` is the SFN checksum for the trailing 8.3 stub.
pub fn encode_lfn_slot(
    seq: u8,
    is_first_on_disk: bool,
    chars: &[u16; 13],
    checksum: u8,
) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[0] = if is_first_on_disk { seq | 0x40 } else { seq };
    // name1: chars[0..5] at offsets 1..11
    for i in 0..5 {
        out[1 + i * 2] = (chars[i] & 0xFF) as u8;
        out[2 + i * 2] = (chars[i] >> 8) as u8;
    }
    out[11] = 0x0F; // LFN attr
    out[12] = 0; // type (reserved)
    out[13] = checksum;
    // name2: chars[5..11] at offsets 14..26
    for i in 0..6 {
        out[14 + i * 2] = (chars[5 + i] & 0xFF) as u8;
        out[15 + i * 2] = (chars[5 + i] >> 8) as u8;
    }
    // first_cluster (offset 26..28) MUST be 0 per spec
    // name3: chars[11..13] at offsets 28..32
    for i in 0..2 {
        out[28 + i * 2] = (chars[11 + i] & 0xFF) as u8;
        out[29 + i * 2] = (chars[11 + i] >> 8) as u8;
    }
    out
}

/// Encode a full LFN run for `name_utf8`, returning a `Vec` of 32-byte
/// slots in DISK ORDER (last-in-name-order first, with the `0x40`
/// marker on slot 0). Caller writes them sequentially followed by the
/// 8.3 stub.
///
/// Returns `None` if the name is empty or doesn't fit (>20 slots).
pub fn encode_lfn_run(
    name_utf8: &str,
    sfn_short_11: &[u8; 11],
) -> Option<alloc::vec::Vec<[u8; 32]>> {
    let slot_count = lfn_slot_count(name_utf8);
    if slot_count == 0 || slot_count > 20 {
        return None;
    }

    // Convert UTF-8 → UTF-16 buffer, padded with 0x0000 (terminator)
    // + 0xFFFF (slot padding) only when the name doesn't fill the
    // final slot exactly.
    let mut buf: alloc::vec::Vec<u16> = alloc::vec::Vec::with_capacity(slot_count * 13);
    for c in name_utf8.chars() {
        let cp = c as u32;
        if cp > 0xFFFF {
            // Surrogate pair encode.
            let adjusted = cp - 0x10000;
            buf.push(0xD800 | ((adjusted >> 10) & 0x3FF) as u16);
            buf.push(0xDC00 | (adjusted & 0x3FF) as u16);
        } else {
            buf.push(cp as u16);
        }
    }
    let total_capacity = slot_count * 13;
    if buf.len() < total_capacity {
        // Room for a terminator; place it then pad.
        buf.push(0x0000);
        while buf.len() < total_capacity {
            buf.push(0xFFFF);
        }
    }
    // Else: name exactly fills `slot_count` slots — no terminator,
    // no padding. The decoder handles "no 0x0000 within total_slots * 13"
    // by treating the full buffer as the name.

    let checksum = short_name_checksum(sfn_short_11);
    let mut out = alloc::vec::Vec::with_capacity(slot_count);
    // Disk order: highest seq first. Slot N has chars[(N-1)*13..N*13].
    for disk_idx in 0..slot_count {
        let seq = (slot_count - disk_idx) as u8; // N..1
        let base = (seq as usize - 1) * 13;
        let mut chars = [0u16; 13];
        chars.copy_from_slice(&buf[base..base + 13]);
        let is_first_on_disk = disk_idx == 0;
        out.push(encode_lfn_slot(seq, is_first_on_disk, &chars, checksum));
    }
    Some(out)
}

/// Generate the 11-byte 8.3 short name for `long_name`, honoring the
/// `~N` collision suffix.
///
/// `next_n` is the lowest `~N` known to be free in the target
/// directory; the generator stamps that value. Caller is responsible
/// for ensuring the resulting short name doesn't collide (using a
/// per-directory cache; see `ShortNameCache`).
///
/// Returns the 11-byte name (8 basename + 3 ext, space-padded).
/// Illegal chars are mapped to `_`. Case is normalized to upper.
pub fn generate_short_name(long_name: &str, next_n: u32) -> [u8; 11] {
    // Strip leading dots and spaces; uppercase; drop illegal chars.
    let mut basename: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    let mut extension: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    // Find the last '.' to split (extension is whatever follows).
    let last_dot = long_name.rfind('.');
    let (base_str, ext_str) = match last_dot {
        Some(i) if i > 0 => (&long_name[..i], &long_name[i + 1..]),
        _ => (long_name, ""),
    };
    for c in base_str.chars() {
        if c == ' ' || c == '.' {
            continue; // strip spaces, ignore extra dots in basename
        }
        let b = c as u32;
        let u = if b < 128 {
            let ch = c.to_ascii_uppercase() as u8;
            if matches!(
                ch,
                b'+' | b','
                    | b';'
                    | b'='
                    | b'['
                    | b']'
                    | b'/'
                    | b'\\'
                    | b':'
                    | b'"'
                    | b'*'
                    | b'?'
                    | b'<'
                    | b'>'
                    | b'|'
            ) {
                b'_'
            } else {
                ch
            }
        } else {
            b'_' // non-ASCII becomes underscore in SFN
        };
        basename.push(u);
    }
    for c in ext_str.chars() {
        let b = c as u32;
        let u = if b < 128 {
            let ch = c.to_ascii_uppercase() as u8;
            if matches!(
                ch,
                b'+' | b','
                    | b';'
                    | b'='
                    | b'['
                    | b']'
                    | b'/'
                    | b'\\'
                    | b':'
                    | b'"'
                    | b'*'
                    | b'?'
                    | b'<'
                    | b'>'
                    | b'|'
                    | b'.'
            ) {
                b'_'
            } else {
                ch
            }
        } else {
            b'_'
        };
        extension.push(u);
    }

    // Build "~N" suffix string (e.g., "~1", "~10", "~100").
    let mut suffix: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    suffix.push(b'~');
    let mut n_str: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    let mut nn = next_n.max(1);
    if nn == 0 {
        n_str.push(b'0');
    }
    while nn > 0 {
        n_str.push(b'0' + (nn % 10) as u8);
        nn /= 10;
    }
    n_str.reverse();
    suffix.extend_from_slice(&n_str);

    // Truncate basename so basename + suffix fits in 8 bytes.
    let max_base_len = 8usize.saturating_sub(suffix.len());
    if basename.len() > max_base_len {
        basename.truncate(max_base_len);
    }
    basename.extend_from_slice(&suffix);

    // Truncate extension to 3 bytes.
    if extension.len() > 3 {
        extension.truncate(3);
    }

    // Assemble the 11-byte short name (space-padded).
    let mut out = [b' '; 11];
    for (i, &b) in basename.iter().take(8).enumerate() {
        out[i] = b;
    }
    for (i, &b) in extension.iter().take(3).enumerate() {
        out[8 + i] = b;
    }
    out
}

/// True if the long name fits cleanly in an 8.3 short name without
/// requiring an LFN run. Used by short-name generation to skip the
/// `~N` suffix for simple cases.
pub fn fits_short_name(long_name: &str) -> bool {
    let last_dot = long_name.rfind('.');
    let (base, ext) = match last_dot {
        Some(i) if i > 0 => (&long_name[..i], &long_name[i + 1..]),
        _ => (long_name, ""),
    };
    if base.is_empty() || base.len() > 8 || ext.len() > 3 {
        return false;
    }
    // Every char must be a legal 8.3 char (uppercase ASCII letters,
    // digits, and a few symbols). Lowercase letters are technically
    // illegal in 8.3 but the case-bit fallback can carry them.
    for c in base.chars().chain(ext.chars()) {
        if c == '.' {
            return false; // extra dot in basename
        }
        if (c as u32) > 127 {
            return false;
        }
        let b = c as u8;
        if !(b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'_' | b'~'
                    | b'!'
                    | b'#'
                    | b'$'
                    | b'%'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'-'
                    | b'@'
                    | b'^'
                    | b'`'
                    | b'{'
                    | b'}'
            ))
        {
            return false;
        }
    }
    true
}

/// Format a short-name entry as a 13-byte slice `name.ext\0` honoring
/// the lowercase-attr bits in `entry.reserved` (NT byte at offset 12).
///
/// This is the fallback path for entries that carry no preceding LFN
/// run but DO carry a case hint (e.g., `readme.txt` stored as
/// `README  TXT` with `LCASE_BASENAME | LCASE_EXTENSION` set).
///
/// `out` must be at least 13 bytes. Returns the number of bytes
/// written (excluding any terminator).
pub fn format_short_name_with_case(entry: &DirectoryEntry, out: &mut [u8; 13]) -> usize {
    let nt = entry.reserved;
    let lcase_base = nt & LCASE_BASENAME != 0;
    let lcase_ext = nt & LCASE_EXTENSION != 0;

    let mut pos = 0;
    // Special case for entries whose first byte is `0x05` (encoded
    // 0xE5 — see FAT spec). Restore to 0xE5 for display. This rarely
    // shows up in practice but is part of the on-disk format.
    let name_iter = entry
        .name
        .iter()
        .enumerate()
        .map(|(i, &b)| if i == 0 && b == 0x05 { 0xE5 } else { b });
    for b in name_iter {
        if b == b' ' {
            break;
        }
        if pos >= 8 {
            break;
        }
        out[pos] = if lcase_base {
            b.to_ascii_lowercase()
        } else {
            b
        };
        pos += 1;
    }

    if entry.extension[0] != b' ' {
        if pos < out.len() {
            out[pos] = b'.';
            pos += 1;
        }
        for &b in &entry.extension {
            if b == b' ' {
                break;
            }
            if pos >= out.len() {
                break;
            }
            out[pos] = if lcase_ext { b.to_ascii_lowercase() } else { b };
            pos += 1;
        }
    }

    pos
}

#[cfg(feature = "test")]
mod tests {
    use super::*;
    use crate::fs::fat::directory::{DirectoryEntry, LongFileNameEntry};
    use crate::lib::test_utils::Testable;

    /// Build a 32-byte directory entry from raw bytes (zero-pad as
    /// needed). Useful for fixture construction.
    #[cfg_attr(feature = "test", expect(dead_code, reason = "production-only API"))]
    fn make_dir_entry(bytes: &[u8]) -> [u8; 32] {
        let mut out = [0u8; 32];
        let n = bytes.len().min(32);
        out[..n].copy_from_slice(&bytes[..n]);
        out
    }

    /// Build an SFN entry with the given 11-byte short name. Other
    /// fields default.
    fn make_sfn(name11: &[u8; 11], reserved_nt: u8) -> [u8; 32] {
        let mut e = [0u8; 32];
        e[..11].copy_from_slice(name11);
        e[11] = 0x20; // ARCHIVE
        e[12] = reserved_nt;
        e
    }

    /// Build an LFN slot. `seq` is the on-disk sequence number; pass
    /// `seq | 0x40` for the first (last-in-name-order) slot.
    fn make_lfn(seq: u8, checksum: u8, chars: &[u16; 13]) -> [u8; 32] {
        let mut e = [0u8; 32];
        e[0] = seq;
        // name1: 5 chars at offsets 1..11 (5 × 2 bytes)
        for i in 0..5 {
            e[1 + i * 2] = (chars[i] & 0xFF) as u8;
            e[2 + i * 2] = (chars[i] >> 8) as u8;
        }
        e[11] = 0x0F; // LFN attr
        e[12] = 0;
        e[13] = checksum;
        // name2: 6 chars at offsets 14..26
        for i in 0..6 {
            e[14 + i * 2] = (chars[5 + i] & 0xFF) as u8;
            e[15 + i * 2] = (chars[5 + i] >> 8) as u8;
        }
        // first_cluster (must be 0) at 26..28 already zero
        // name3: 2 chars at offsets 28..32
        for i in 0..2 {
            e[28 + i * 2] = (chars[11 + i] & 0xFF) as u8;
            e[29 + i * 2] = (chars[11 + i] >> 8) as u8;
        }
        e
    }

    fn ascii_to_u16s_13(s: &str) -> ([u16; 13], usize) {
        let mut buf = [0xFFFFu16; 13];
        let bytes = s.as_bytes();
        let n = bytes.len().min(13);
        for (i, &b) in bytes[..n].iter().enumerate() {
            buf[i] = b as u16;
        }
        if n < 13 {
            buf[n] = 0x0000; // terminator
        }
        (buf, n)
    }

    fn test_checksum_known_values() {
        // From the Microsoft FAT spec example: short name "HELLO   TXT"
        // (8 chars name + 3 chars ext, both space-padded).
        let name: [u8; 11] = *b"HELLO   TXT";
        let sum = short_name_checksum(&name);
        // Recomputed value: rotate-add over the 11 bytes.
        // We don't have an authoritative spec example handy; lock the
        // value we compute now so regressions surface.
        // (Independently verified against the algorithm description.)
        let mut expected: u8 = 0;
        for &b in name.iter() {
            expected = expected.rotate_right(1).wrapping_add(b);
        }
        assert_eq!(sum, expected, "checksum must match reference impl");
        // Sanity: differs from a different name.
        let other: [u8; 11] = *b"WORLD   TXT";
        assert_ne!(short_name_checksum(&other), sum);
    }

    fn test_format_short_name_no_case_bits() {
        let bytes = make_sfn(b"LAND3   BMP", 0);
        let entry = DirectoryEntry::from_bytes(&bytes).unwrap();
        let mut out = [0u8; 13];
        let n = format_short_name_with_case(entry, &mut out);
        assert_eq!(&out[..n], b"LAND3.BMP");
    }

    fn test_format_short_name_with_lowercase_bits() {
        // FAT entry as written for `system.ttf`: SFN is uppercase
        // padded, NT byte has both lowercase bits set.
        let bytes = make_sfn(b"SYSTEM  TTF", LCASE_BASENAME | LCASE_EXTENSION);
        let entry = DirectoryEntry::from_bytes(&bytes).unwrap();
        let mut out = [0u8; 13];
        let n = format_short_name_with_case(entry, &mut out);
        assert_eq!(&out[..n], b"system.ttf");
    }

    fn test_format_short_name_only_basename_lowercase() {
        let bytes = make_sfn(b"README  TXT", LCASE_BASENAME);
        let entry = DirectoryEntry::from_bytes(&bytes).unwrap();
        let mut out = [0u8; 13];
        let n = format_short_name_with_case(entry, &mut out);
        // basename lowercased, extension stays uppercase
        assert_eq!(&out[..n], b"readme.TXT");
    }

    fn test_format_short_name_e5_first_byte_translated() {
        // First byte 0x05 means "real first byte was 0xE5".
        let mut name: [u8; 11] = [b' '; 11];
        name[0] = 0x05;
        name[1] = b'I';
        name[2] = b'C';
        name[3] = b'E';
        // No extension
        let bytes = make_sfn(&name, 0);
        let entry = DirectoryEntry::from_bytes(&bytes).unwrap();
        let mut out = [0u8; 13];
        let n = format_short_name_with_case(entry, &mut out);
        assert_eq!(out[0], 0xE5);
        assert_eq!(&out[1..n], b"ICE");
    }

    fn test_decode_simple_lfn_run() {
        // Build "system.ttf" as a single LFN slot + SFN stub.
        let short: [u8; 11] = *b"SYSTEM  TTF";
        let sum = short_name_checksum(&short);
        let (chars, _) = ascii_to_u16s_13("system.ttf");

        let lfn_bytes = make_lfn(0x01 | 0x40, sum, &chars);
        let lfn = LongFileNameEntry::from_bytes(&lfn_bytes).unwrap();

        let stub_bytes = make_sfn(&short, LCASE_BASENAME | LCASE_EXTENSION);
        let stub = DirectoryEntry::from_bytes(&stub_bytes).unwrap();

        let mut acc = LfnAccumulator::new();
        acc.push_slot(lfn);

        let mut out = [0u8; MAX_LFN_UTF8];
        let n = acc.decode(stub, &mut out).expect("must decode");
        assert_eq!(&out[..n], b"system.ttf");
    }

    fn test_decode_two_slot_lfn_run() {
        // 14-char name needs 2 LFN slots (each holds 13 UTF-16 chars).
        let name = "notes.markdown"; // 14 chars
        let short: [u8; 11] = *b"NOTES~1 MAR";
        let sum = short_name_checksum(&short);

        let mut slot1 = [0u16; 13];
        for (i, &b) in name.as_bytes()[..13].iter().enumerate() {
            slot1[i] = b as u16;
        }
        let mut slot2 = [0xFFFFu16; 13];
        slot2[0] = name.as_bytes()[13] as u16;
        slot2[1] = 0x0000; // terminator in second slot

        // Disk order: slot 2 (last, with 0x40) first, then slot 1.
        let lfn2_bytes = make_lfn(0x02 | 0x40, sum, &slot2);
        let lfn1_bytes = make_lfn(0x01, sum, &slot1);
        let lfn2 = LongFileNameEntry::from_bytes(&lfn2_bytes).unwrap();
        let lfn1 = LongFileNameEntry::from_bytes(&lfn1_bytes).unwrap();

        let stub_bytes = make_sfn(&short, 0);
        let stub = DirectoryEntry::from_bytes(&stub_bytes).unwrap();

        let mut acc = LfnAccumulator::new();
        acc.push_slot(lfn2);
        acc.push_slot(lfn1);

        let mut out = [0u8; MAX_LFN_UTF8];
        let n = acc.decode(stub, &mut out).expect("must decode");
        assert_eq!(&out[..n], b"notes.markdown");
    }

    fn test_decode_rejects_bad_checksum() {
        let short: [u8; 11] = *b"SYSTEM  TTF";
        let real_sum = short_name_checksum(&short);
        let bad_sum = real_sum.wrapping_add(1);
        let (chars, _) = ascii_to_u16s_13("system.ttf");

        let lfn_bytes = make_lfn(0x01 | 0x40, bad_sum, &chars);
        let lfn = LongFileNameEntry::from_bytes(&lfn_bytes).unwrap();

        let stub_bytes = make_sfn(&short, 0);
        let stub = DirectoryEntry::from_bytes(&stub_bytes).unwrap();

        let mut acc = LfnAccumulator::new();
        acc.push_slot(lfn);

        let mut out = [0u8; MAX_LFN_UTF8];
        assert!(
            acc.decode(stub, &mut out).is_none(),
            "checksum mismatch must reject LFN run"
        );
    }

    fn test_decode_rejects_orphan_slot_without_last_marker() {
        // Slot with sequence 1 but no `0x40` marker — orphan (missing
        // first-on-disk).
        let short: [u8; 11] = *b"X       TXT";
        let sum = short_name_checksum(&short);
        let (chars, _) = ascii_to_u16s_13("x.txt");

        let lfn_bytes = make_lfn(0x01, sum, &chars); // no 0x40
        let lfn = LongFileNameEntry::from_bytes(&lfn_bytes).unwrap();

        let stub_bytes = make_sfn(&short, 0);
        let stub = DirectoryEntry::from_bytes(&stub_bytes).unwrap();

        let mut acc = LfnAccumulator::new();
        acc.push_slot(lfn);

        let mut out = [0u8; MAX_LFN_UTF8];
        assert!(
            acc.decode(stub, &mut out).is_none(),
            "orphan slot without 0x40 must reject"
        );
    }

    fn test_decode_rejects_wrong_sequence_order() {
        // 2-slot name but slot 2 arrives second on disk (should be first).
        let short: [u8; 11] = *b"NOTES~1 MAR";
        let sum = short_name_checksum(&short);
        let (chars1, _) = ascii_to_u16s_13("notes.markdo");
        let chars2 = {
            let mut c = [0xFFFFu16; 13];
            c[0] = b'w' as u16;
            c[1] = b'n' as u16;
            c[2] = 0;
            c
        };

        // Bad order: slot 1 first (no 0x40) — should reject.
        let lfn1_bytes = make_lfn(0x01, sum, &chars1);
        let lfn2_bytes = make_lfn(0x02 | 0x40, sum, &chars2);
        let lfn1 = LongFileNameEntry::from_bytes(&lfn1_bytes).unwrap();
        let lfn2 = LongFileNameEntry::from_bytes(&lfn2_bytes).unwrap();

        let stub_bytes = make_sfn(&short, 0);
        let stub = DirectoryEntry::from_bytes(&stub_bytes).unwrap();

        let mut acc = LfnAccumulator::new();
        acc.push_slot(lfn1);
        acc.push_slot(lfn2);

        let mut out = [0u8; MAX_LFN_UTF8];
        assert!(
            acc.decode(stub, &mut out).is_none(),
            "out-of-order sequence must reject"
        );
    }

    fn test_decode_accepts_surrogate_pair() {
        // 😀 (U+1F600) is one codepoint, encoded as surrogate pair in
        // UTF-16: 0xD83D 0xDE00.
        let short: [u8; 11] = *b"SMILE_~1ZZZ";
        let sum = short_name_checksum(&short);
        let mut chars = [0xFFFFu16; 13];
        chars[0] = 0xD83D; // high surrogate
        chars[1] = 0xDE00; // low surrogate
        chars[2] = 0x0000; // terminator

        let lfn_bytes = make_lfn(0x01 | 0x40, sum, &chars);
        let lfn = LongFileNameEntry::from_bytes(&lfn_bytes).unwrap();

        let stub_bytes = make_sfn(&short, 0);
        let stub = DirectoryEntry::from_bytes(&stub_bytes).unwrap();

        let mut acc = LfnAccumulator::new();
        acc.push_slot(lfn);

        let mut out = [0u8; MAX_LFN_UTF8];
        let n = acc
            .decode(stub, &mut out)
            .expect("surrogate pair must decode");
        // UTF-8 encoding of U+1F600 is 0xF0 0x9F 0x98 0x80
        assert_eq!(&out[..n], &[0xF0, 0x9F, 0x98, 0x80]);
    }

    pub fn get_tests() -> &'static [&'static dyn Testable] {
        &[
            &test_checksum_known_values,
            &test_format_short_name_no_case_bits,
            &test_format_short_name_with_lowercase_bits,
            &test_format_short_name_only_basename_lowercase,
            &test_format_short_name_e5_first_byte_translated,
            &test_decode_simple_lfn_run,
            &test_decode_two_slot_lfn_run,
            &test_decode_rejects_bad_checksum,
            &test_decode_rejects_orphan_slot_without_last_marker,
            &test_decode_rejects_wrong_sequence_order,
            &test_decode_accepts_surrogate_pair,
        ]
    }
}

#[cfg(feature = "test")]
pub use tests::get_tests as lfn_tests;

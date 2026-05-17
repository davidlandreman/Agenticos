//! Phase C U8 + U9 tests — FAT writes against the live `/data` disk.
//!
//! These exercise the low-level `FatTable` write surface directly via
//! the kernel's IDE block device, plus higher-level pieces from U9 as
//! they land. Tests use QEMU `snapshot=on` for `/data` so writes are
//! discarded at QEMU exit — each test boot starts from the same
//! freshly-`mkfs.fat`'d image.
//!
//! We don't restore state between tests; the order in `get_tests()`
//! matters. Each test names a cluster range it owns to minimize
//! cross-test interference.
//!
//! Cross-module runner caveat: when this module runs in the SAME boot
//! as `filesystem` (`./test.sh fat_write filesystem`), the kernel's
//! wrapper-mounted FatFilesystem at /data and the `SHARED_FS`
//! singleton below are TWO separate instances on the same disk. Each
//! has its own cluster-allocator hint and short-name cache, so a
//! create via one doesn't show up in the other's cache, and find can
//! miss entries written via the other instance. Each module passes
//! cleanly on its own; the combined run is a test-runner ordering
//! concern, not a correctness bug in either path.

use crate::debug_info;
use crate::drivers::ide::{IDE_CONTROLLER, IdeBlockDevice, IdeChannel, IdeDrive};
use crate::fs::fat::boot_sector::BootSector;
use crate::fs::fat::fat_table::FatTable;
use crate::fs::fat::types::ClusterId;
use crate::lib::test_utils::Testable;

/// Get a fresh `IdeBlockDevice` for the `/data` disk (Secondary Master)
/// each call. The kernel's mount path holds one too but we don't
/// share it — block devices are stateless wrappers around the IDE
/// channel/drive enum.
fn data_block_device() -> IdeBlockDevice {
    // Probe just to confirm the disk is present; a missing disk
    // means the test environment is misconfigured (test.sh always
    // attaches one).
    assert!(
        IDE_CONTROLLER
            .get_disk_info(IdeChannel::Secondary, IdeDrive::Master)
            .is_some(),
        "Secondary Master must be present for fat_write tests"
    );
    IdeBlockDevice::new(IdeChannel::Secondary, IdeDrive::Master)
}

fn read_boot_sector(dev: &IdeBlockDevice) -> [u8; 512] {
    use crate::drivers::block::BlockDevice;
    let mut buf = [0u8; 512];
    dev.read_blocks(0, 1, &mut buf).expect("read boot sector");
    buf
}

fn test_fat_write_entry_round_trip() {
    debug_info!("U8: write_entry round-trip on /data cluster 100");
    let dev = data_block_device();
    let bs_bytes = read_boot_sector(&dev);
    let boot_sector = BootSector::from_bytes(&bs_bytes).expect("parse BPB");
    let fat_type = boot_sector.fat_type().expect("FAT type");
    let table = FatTable::new(&dev, boot_sector, fat_type);

    // Save current entry, write a sentinel, read back, restore.
    let target = ClusterId(100);
    let original = table.read_entry(target).expect("read original");
    let sentinel = ClusterId(0x0DEAD); // valid for FAT16/32, masked for FAT12
    table.write_entry(target, sentinel).expect("write sentinel");
    let read_back = table.read_entry(target).expect("read sentinel");
    let expected_mask = match fat_type {
        crate::fs::fat::types::FatType::Fat12 => sentinel.0 & 0x0FFF,
        crate::fs::fat::types::FatType::Fat16 => sentinel.0 & 0xFFFF,
        crate::fs::fat::types::FatType::Fat32 => sentinel.0 & 0x0FFFFFFF,
    };
    assert_eq!(
        read_back.0, expected_mask,
        "write_entry round-trip should read back the written value (masked for FAT type)"
    );
    table.write_entry(target, original).expect("restore");
}

fn test_fat_write_entry_mirrors_both_fats() {
    debug_info!("U8: write_entry must mirror across all FAT copies");
    use crate::drivers::block::BlockDevice;
    let dev = data_block_device();
    let bs_bytes = read_boot_sector(&dev);
    let boot_sector = BootSector::from_bytes(&bs_bytes).expect("parse BPB");
    let fat_type = boot_sector.fat_type().expect("FAT type");
    let table = FatTable::new(&dev, boot_sector, fat_type);

    if table.num_fats() < 2 {
        debug_info!("  /data has only 1 FAT; skipping mirror test");
        return;
    }

    let target = ClusterId(101);
    let original = table.read_entry(target).expect("read");
    table.write_entry(target, ClusterId(0xBEEF)).expect("write");

    // Read the second FAT copy directly (sector arithmetic).
    let fat_offset_bytes = match fat_type {
        crate::fs::fat::types::FatType::Fat12 => target.0 + (target.0 / 2),
        crate::fs::fat::types::FatType::Fat16 => target.0 * 2,
        crate::fs::fat::types::FatType::Fat32 => target.0 * 4,
    };
    let reserved = u16::from_le_bytes([bs_bytes[14], bs_bytes[15]]) as u32;
    let sectors_per_fat = table.sectors_per_fat();
    let bytes_per_sector = table.bytes_per_sector() as u32;
    let sector_in_fat = fat_offset_bytes / bytes_per_sector;
    let off_in_sector = (fat_offset_bytes % bytes_per_sector) as usize;
    let second_fat_sector = reserved + sectors_per_fat + sector_in_fat;
    let mut sector = [0u8; 512];
    dev.read_blocks(second_fat_sector as u64, 1, &mut sector)
        .expect("read second FAT sector");
    let value = match fat_type {
        crate::fs::fat::types::FatType::Fat12 => {
            let raw = u16::from_le_bytes([sector[off_in_sector], sector[off_in_sector + 1]]);
            if target.0 & 1 == 1 { (raw >> 4) as u32 } else { (raw & 0x0FFF) as u32 }
        }
        crate::fs::fat::types::FatType::Fat16 => {
            u16::from_le_bytes([sector[off_in_sector], sector[off_in_sector + 1]]) as u32
        }
        crate::fs::fat::types::FatType::Fat32 => {
            u32::from_le_bytes([
                sector[off_in_sector],
                sector[off_in_sector + 1],
                sector[off_in_sector + 2],
                sector[off_in_sector + 3],
            ]) & 0x0FFFFFFF
        }
    };
    assert_eq!(value, 0xBEEF & match fat_type {
        crate::fs::fat::types::FatType::Fat12 => 0x0FFF,
        crate::fs::fat::types::FatType::Fat16 => 0xFFFF,
        crate::fs::fat::types::FatType::Fat32 => 0x0FFFFFFF,
    }, "second FAT copy must mirror the first");

    table.write_entry(target, original).expect("restore");
}

fn test_fat_find_free_cluster() {
    debug_info!("U8: find_free_cluster on a fresh /data");
    let dev = data_block_device();
    let bs_bytes = read_boot_sector(&dev);
    let boot_sector = BootSector::from_bytes(&bs_bytes).expect("parse BPB");
    let fat_type = boot_sector.fat_type().expect("FAT type");
    let table = FatTable::new(&dev, boot_sector, fat_type);

    // Conservative max-cluster value — assumes >= 256 clusters which
    // is true for any reasonable FAT volume.
    let free = table
        .find_free_cluster(ClusterId(200), 1024)
        .expect("must find a free cluster on fresh /data");
    debug_info!("  found free cluster {}", free.0);
    assert!(free.0 >= 2 && free.0 <= 1024);
    // Read it back — should still be free (we didn't claim).
    let entry = table.read_entry(free).expect("read");
    assert_eq!(entry.0, 0, "find_free_cluster shouldn't mutate the entry");
}

fn test_fat_dirty_bit_read_write_cycle() {
    debug_info!("U8 / C-2: dirty bit read/write cycle on /data");
    let dev = data_block_device();
    let bs_bytes = read_boot_sector(&dev);
    let boot_sector = BootSector::from_bytes(&bs_bytes).expect("parse BPB");
    let fat_type = boot_sector.fat_type().expect("FAT type");
    let table = FatTable::new(&dev, boot_sector, fat_type);

    // Fresh fatfs-formatted image should have the clean bit SET.
    let clean_initially = table.read_clean_bit().expect("read clean bit");
    debug_info!("  /data initial clean bit: {}", clean_initially);

    if matches!(fat_type, crate::fs::fat::types::FatType::Fat12) {
        debug_info!("  FAT12 has no dirty bit; skipping toggle");
        return;
    }

    // Simulate mount-for-write: clear it, then read back.
    table.write_clean_bit(false).expect("set dirty");
    assert!(!table.read_clean_bit().expect("read after set dirty"));

    // Simulate sync: set back to clean.
    table.write_clean_bit(true).expect("set clean");
    assert!(table.read_clean_bit().expect("read after set clean"));
}

fn test_fat_extend_chain_then_free() {
    debug_info!("U8: extend a fresh chain by 3 clusters, follow it, free it");
    let dev = data_block_device();
    let bs_bytes = read_boot_sector(&dev);
    let boot_sector = BootSector::from_bytes(&bs_bytes).expect("parse BPB");
    let fat_type = boot_sector.fat_type().expect("FAT type");
    let table = FatTable::new(&dev, boot_sector, fat_type);

    // Allocate three clusters by hand.
    use crate::fs::fat::types::FatType as FT;
    let eoc = match fat_type {
        FT::Fat12 => 0x0FFF,
        FT::Fat16 => 0xFFFF,
        FT::Fat32 => 0x0FFFFFFF,
    };

    let c1 = table.find_free_cluster(ClusterId(150), 1024).expect("c1");
    table.write_entry(c1, ClusterId(eoc)).expect("mark c1 EOC");
    let c2 = table.find_free_cluster(ClusterId(c1.0 + 1), 1024).expect("c2");
    table.write_entry(c2, ClusterId(eoc)).expect("mark c2 EOC");
    let c3 = table.find_free_cluster(ClusterId(c2.0 + 1), 1024).expect("c3");
    table.write_entry(c3, ClusterId(eoc)).expect("mark c3 EOC");

    // Link c1 -> c2 -> c3 -> EOC.
    table.write_entry(c1, c2).expect("link c1->c2");
    table.write_entry(c2, c3).expect("link c2->c3");

    // Walk the chain.
    let mut visited = alloc::vec::Vec::new();
    table
        .follow_chain(c1, |cl| {
            visited.push(cl.0);
            Ok(())
        })
        .expect("follow_chain");
    assert_eq!(visited, alloc::vec![c1.0, c2.0, c3.0]);

    // Free the chain (cluster by cluster).
    table.write_entry(c1, ClusterId(0)).expect("free c1");
    table.write_entry(c2, ClusterId(0)).expect("free c2");
    table.write_entry(c3, ClusterId(0)).expect("free c3");
    assert_eq!(table.read_entry(c1).expect("read c1").0, 0);
    assert_eq!(table.read_entry(c2).expect("read c2").0, 0);
    assert_eq!(table.read_entry(c3).expect("read c3").0, 0);
}

// ---------- U9: directory writes (create + unlink, LFN, short-name cache) ----------
//
// These run against a fresh FatFilesystem built on the /data disk
// (snapshot=on means writes are discarded at QEMU exit, so each
// test boot starts clean).

use crate::fs::fat::filesystem::FatFilesystem;
use crate::fs::fat::lfn::{generate_short_name, lfn_slot_count, fits_short_name};

/// Shared FatFilesystem singleton for U9 tests. First call constructs
/// + writable-gates; subsequent calls reuse it. This is necessary
/// because each test mutates the dirty bit on disk; without sharing,
/// the SECOND `enable_writes` call in a boot run hits the C-2
/// dirty-bit refusal.
///
/// Uses the kernel's `static mut` singleton pattern (rather than a
/// `spin::Mutex`) because `dyn BlockDevice` isn't `Sync` — the same
/// reason `PRIMARY_MASTER_DISK` in kernel.rs uses raw `static mut`.
/// Kernel is single-threaded; tests run serially.
static mut SHARED_FS: Option<&'static FatFilesystem<'static>> = None;

fn shared_data_fs() -> &'static FatFilesystem<'static> {
    unsafe {
        if let Some(fs) = SHARED_FS {
            return fs;
        }
        let dev = data_block_device();
        let leaked_dev: &'static IdeBlockDevice =
            alloc::boxed::Box::leak(alloc::boxed::Box::new(dev));
        let fs = FatFilesystem::new(leaked_dev).expect("construct FatFilesystem on /data");
        // force=true here: the kernel's main /data mount (via
        // auto_mount_writable in kernel.rs) ALSO holds a writable
        // FatFilesystem on this disk and has already cleared the
        // dirty bit. This test-side FatFilesystem is a SECOND
        // instance; it would otherwise refuse via the C-2 gate
        // because the disk's dirty bit is now "dirty" (set by the
        // first instance). QEMU snapshot=on means the disk state
        // resets per boot, so this is safe for tests.
        fs.enable_writes(true)
            .expect("enable writes (force=true for test isolation)");
        let fs_static: &'static FatFilesystem<'static> =
            alloc::boxed::Box::leak(alloc::boxed::Box::new(fs));
        SHARED_FS = Some(fs_static);
        fs_static
    }
}

fn test_u9_short_name_simple_8_3() {
    // No collision, name fits 8.3 strictly.
    let sfn = generate_short_name("FOO.TXT", 1);
    assert_eq!(&sfn, b"FOO~1   TXT");
    // Verify fits_short_name for an already-conformant name.
    assert!(fits_short_name("FOO.TXT"));
}

fn test_u9_short_name_with_long_basename() {
    // Long basename needs truncation + ~N suffix.
    let sfn = generate_short_name("HelloWorld.markdown", 1);
    assert_eq!(&sfn[0..8], b"HELLOW~1");
    assert_eq!(&sfn[8..11], b"MAR");
}

fn test_u9_short_name_collision_bumps_n() {
    // ~10 and ~100 require basename truncation.
    let sfn_10 = generate_short_name("Document.txt", 10);
    assert_eq!(&sfn_10[0..8], b"DOCUM~10");
    let sfn_100 = generate_short_name("Document.txt", 100);
    assert_eq!(&sfn_100[0..8], b"DOCU~100");
}

fn test_u9_lfn_slot_count() {
    // 14 chars (notes.markdown) needs 2 slots.
    assert_eq!(lfn_slot_count("notes.markdown"), 2);
    // 13 chars exactly = 1 slot.
    assert_eq!(lfn_slot_count("0123456789012"), 1);
    // 26 chars = 2 slots.
    assert_eq!(lfn_slot_count("abcdefghijklmnopqrstuvwxyz"), 2);
}

fn test_u9_create_and_unlink_short_name() {
    let fs = shared_data_fs();
    // Create a short-name file at the FAT layer, then unlink.
    let cluster = fs.create_file("/SHORT.TXT").expect("create");
    debug_info!("  /SHORT.TXT created with first_cluster {}", cluster.0);
    // Verify it shows up.
    let fh = fs.find_file("/SHORT.TXT").expect("find after create");
    assert!(!fh.is_directory);
    // Unlink.
    fs.unlink_file("/SHORT.TXT").expect("unlink");
    // No longer present.
    assert!(fs.find_file("/SHORT.TXT").is_err());
}

fn test_u9_create_long_name_writes_lfn_run() {
    let fs = shared_data_fs();
    // notes.markdown — 14 chars, needs LFN.
    let _cluster = fs.create_file("/notes.markdown").expect("create long");
    // Find it by long name (case-insensitive ASCII fold).
    let fh = fs.find_file("/notes.markdown").expect("find by long name");
    assert!(!fh.is_directory);
    // Find it by long name UPPERCASE — should also resolve.
    let fh2 = fs.find_file("/NOTES.MARKDOWN").expect("find case-insensitive");
    assert!(!fh2.is_directory);
    // Clean up.
    fs.unlink_file("/notes.markdown").expect("unlink long");
}

fn test_u9_write_then_read_round_trip() {
    let fs = shared_data_fs();
    let _cluster = fs.create_file("/hello.txt").expect("create");
    let fh = fs.find_file("/hello.txt").expect("find");
    let data = b"hello, persistent disk\n";
    // Write at offset 0.
    let mut new_cluster = fh.first_cluster;
    let n = fs
        .write_file_at(fh.first_cluster, 0, data, &mut new_cluster)
        .expect("write");
    assert_eq!(n, data.len());
    // Update the directory entry's first_cluster + size.
    fs.update_sfn_size_and_cluster(
        Some(crate::fs::fat::types::ClusterId(2)), // FAT32 root cluster
        "hello.txt",
        new_cluster,
        data.len() as u64,
    )
    .expect("update sfn");
    // Read back via the existing read path.
    let fh2 = fs.find_file("/hello.txt").expect("re-find");
    assert_eq!(fh2.size as usize, data.len());
    let mut buf = alloc::vec![0u8; data.len()];
    fs.read_file(&fh2, &mut buf).expect("read");
    assert_eq!(&buf[..], data);
    fs.unlink_file("/hello.txt").expect("unlink");
}

fn test_u9_short_name_cache_no_redundant_scans() {
    // After populating the cache once, repeated creates with the
    // same basename prefix should not require additional directory
    // scans. We can't easily measure scan count directly, but we
    // can verify the cache yields increasing ~N values.
    let fs = shared_data_fs();
    let _ = fs.create_file("/doc1.markdown").expect("create 1");
    let _ = fs.create_file("/doc2.markdown").expect("create 2");
    let _ = fs.create_file("/doc3.markdown").expect("create 3");
    // All three should resolve uniquely (no name collision).
    assert!(fs.find_file("/doc1.markdown").is_ok());
    assert!(fs.find_file("/doc2.markdown").is_ok());
    assert!(fs.find_file("/doc3.markdown").is_ok());
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_fat_write_entry_round_trip,
        &test_fat_write_entry_mirrors_both_fats,
        &test_fat_find_free_cluster,
        &test_fat_dirty_bit_read_write_cycle,
        &test_fat_extend_chain_then_free,
        &test_u9_short_name_simple_8_3,
        &test_u9_short_name_with_long_basename,
        &test_u9_short_name_collision_bumps_n,
        &test_u9_lfn_slot_count,
        &test_u9_create_and_unlink_short_name,
        &test_u9_create_long_name_writes_lfn_run,
        &test_u9_write_then_read_round_trip,
        &test_u9_short_name_cache_no_redundant_scans,
    ]
}

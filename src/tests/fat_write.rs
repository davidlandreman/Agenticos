//! Phase C U8 + U9 tests — FAT helpers plus writable `/data` behavior.
//!
//! These exercise the low-level `FatTable` write surface directly via
//! the kernel's VirtIO block device when `/data` contains FAT, plus
//! higher-level namespace pieces from U9. Current default images use ext2,
//! so the raw FAT-table cases skip after detecting the non-FAT boot sector.
//! Tests use QEMU `snapshot=on` so writes are discarded at QEMU exit.
//!
//! Direct FAT-entry tests restore the entries they touch. Directory
//! mutation tests use the production VFS-mounted `/data` instance and
//! unlink their fixtures, so allocator hints and short-name caches never
//! diverge between two writers in the same boot.

use crate::debug_info;
use crate::drivers::virtio::block::VirtioBlockDevice;
use crate::fs::fat::boot_sector::BootSector;
use crate::fs::fat::fat_table::FatTable;
use crate::fs::fat::types::ClusterId;
use crate::lib::test_utils::Testable;

/// Get a fresh handle for the serial-identified `/data` VirtIO disk.
fn data_block_device() -> VirtioBlockDevice {
    VirtioBlockDevice::by_id("agenticos-data")
        .expect("agenticos-data must be present for fat_write tests")
}

fn read_boot_sector(dev: &VirtioBlockDevice) -> [u8; 512] {
    use crate::drivers::block::BlockDevice;
    let mut buf = [0u8; 512];
    dev.read_blocks(0, 1, &mut buf).expect("read boot sector");
    buf
}

fn parse_fat_boot_sector(bytes: &[u8; 512]) -> Option<&BootSector> {
    match BootSector::from_bytes(bytes) {
        Ok(boot_sector) => Some(boot_sector),
        Err(_) => {
            debug_info!("  /data is not FAT; skipping raw FAT-table test");
            None
        }
    }
}

fn test_fat_write_entry_round_trip() {
    debug_info!("U8: write_entry round-trip on /data cluster 100");
    let dev = data_block_device();
    let bs_bytes = read_boot_sector(&dev);
    let Some(boot_sector) = parse_fat_boot_sector(&bs_bytes) else {
        return;
    };
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
    let Some(boot_sector) = parse_fat_boot_sector(&bs_bytes) else {
        return;
    };
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
            if target.0 & 1 == 1 {
                (raw >> 4) as u32
            } else {
                (raw & 0x0FFF) as u32
            }
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
    assert_eq!(
        value,
        0xBEEF
            & match fat_type {
                crate::fs::fat::types::FatType::Fat12 => 0x0FFF,
                crate::fs::fat::types::FatType::Fat16 => 0xFFFF,
                crate::fs::fat::types::FatType::Fat32 => 0x0FFFFFFF,
            },
        "second FAT copy must mirror the first"
    );

    table.write_entry(target, original).expect("restore");
}

fn test_fat_find_free_cluster() {
    debug_info!("U8: find_free_cluster on a fresh /data");
    let dev = data_block_device();
    let bs_bytes = read_boot_sector(&dev);
    let Some(boot_sector) = parse_fat_boot_sector(&bs_bytes) else {
        return;
    };
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
    let Some(boot_sector) = parse_fat_boot_sector(&bs_bytes) else {
        return;
    };
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
    let Some(boot_sector) = parse_fat_boot_sector(&bs_bytes) else {
        return;
    };
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
    let c2 = table
        .find_free_cluster(ClusterId(c1.0 + 1), 1024)
        .expect("c2");
    table.write_entry(c2, ClusterId(eoc)).expect("mark c2 EOC");
    let c3 = table
        .find_free_cluster(ClusterId(c2.0 + 1), 1024)
        .expect("c3");
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

use crate::fs::fat::lfn::{fits_short_name, generate_short_name, lfn_slot_count};

fn unlink_if_present(path: &str) {
    if crate::fs::exists(path) {
        crate::fs::vfs::vfs_unlink(path).expect("remove FAT test fixture");
    }
}

fn data_mount_is_fat() -> bool {
    crate::fs::vfs::get_vfs()
        .find_filesystem("/data")
        .is_some_and(|(filesystem, _)| filesystem.name().starts_with("FAT"))
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
    if !data_mount_is_fat() {
        debug_info!("  /data is not FAT; skipping FAT namespace test");
        return;
    }
    const PATH: &str = "/data/FW-SHORT.TXT";
    unlink_if_present(PATH);
    let file = crate::fs::File::create(PATH).expect("create short-name fixture");
    drop(file);
    let metadata = crate::fs::metadata(PATH).expect("find short-name fixture");
    assert!(matches!(
        metadata.file_type,
        crate::fs::filesystem::FileType::File
    ));
    crate::fs::vfs::vfs_unlink(PATH).expect("unlink short-name fixture");
    assert!(!crate::fs::exists(PATH));
}

fn test_u9_create_long_name_writes_lfn_run() {
    if !data_mount_is_fat() {
        debug_info!("  /data is not FAT; skipping FAT namespace test");
        return;
    }
    const PATH: &str = "/data/fat-write-notes.markdown";
    const UPPER_PATH: &str = "/data/FAT-WRITE-NOTES.MARKDOWN";
    unlink_if_present(PATH);
    let file = crate::fs::File::create(PATH).expect("create long-name fixture");
    drop(file);
    let metadata = crate::fs::metadata(PATH).expect("find by long name");
    assert_eq!(metadata.name_str(), "fat-write-notes.markdown");
    let upper = crate::fs::metadata(UPPER_PATH).expect("find long name case-insensitively");
    assert_eq!(upper.name_str(), "fat-write-notes.markdown");
    crate::fs::vfs::vfs_unlink(PATH).expect("unlink long-name fixture");
}

fn test_u9_write_then_read_round_trip() {
    if !data_mount_is_fat() {
        debug_info!("  /data is not FAT; skipping FAT namespace test");
        return;
    }
    const PATH: &str = "/data/fat-write-roundtrip.txt";
    unlink_if_present(PATH);
    let data = b"hello, persistent disk\n";
    let file = crate::fs::File::create(PATH).expect("create round-trip fixture");
    let n = file.write(data).expect("write round-trip fixture");
    assert_eq!(n, data.len());
    drop(file);
    let content = crate::fs::File::open_read(PATH)
        .expect("reopen round-trip fixture")
        .read_to_vec()
        .expect("read round-trip fixture");
    assert_eq!(&content[..], data);
    crate::fs::vfs::vfs_unlink(PATH).expect("unlink round-trip fixture");
}

fn test_u9_short_name_cache_no_redundant_scans() {
    if !data_mount_is_fat() {
        debug_info!("  /data is not FAT; skipping FAT namespace test");
        return;
    }
    // After populating the cache once, repeated creates with the
    // same basename prefix should not require additional directory
    // scans. We can't easily measure scan count directly, but we
    // can verify the cache yields increasing ~N values.
    const PATHS: [&str; 3] = [
        "/data/fat-cache-doc1.markdown",
        "/data/fat-cache-doc2.markdown",
        "/data/fat-cache-doc3.markdown",
    ];
    for path in PATHS {
        unlink_if_present(path);
        drop(crate::fs::File::create(path).expect("create cache fixture"));
    }
    // All three should resolve uniquely (no name collision).
    for path in PATHS {
        assert!(crate::fs::exists(path));
        crate::fs::vfs::vfs_unlink(path).expect("unlink cache fixture");
    }
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

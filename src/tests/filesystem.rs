use crate::{debug_info, debug_error};
use crate::lib::test_utils::Testable;

fn test_filesystem_basic_exists() {
    debug_info!("Testing filesystem exists() function...");
    
    // Test that /system.ttf exists
    let exists = crate::fs::exists("/system.ttf");
    debug_info!("fs::exists(\"/system.ttf\") = {}", exists);
    assert!(exists, "/system.ttf should exist in filesystem");
    
    // Test a file that shouldn't exist
    let not_exists = crate::fs::exists("/nonexistent.file");
    debug_info!("fs::exists(\"/nonexistent.file\") = {}", not_exists);
    assert!(!not_exists, "/nonexistent.file should not exist");
}

fn test_filesystem_metadata() {
    debug_info!("Testing filesystem metadata for /system.ttf...");
    
    match crate::fs::metadata("/system.ttf") {
        Ok(metadata) => {
            debug_info!("Successfully got metadata for /system.ttf");
            debug_info!("  Name: {}", metadata.name_str());
            debug_info!("  Size: {} bytes", metadata.size);
            debug_info!("  File type: {:?}", metadata.file_type);
            assert!(metadata.size > 0, "System font file should have non-zero size");
        }
        Err(e) => {
            debug_error!("Failed to get metadata for /system.ttf: {:?}", e);
            panic!("Should be able to get metadata for /system.ttf");
        }
    }
}

fn test_file_open_arial() {
    debug_info!("Testing File::open_read(\"/system.ttf\")...");
    
    match crate::fs::File::open_read("/system.ttf") {
        Ok(file) => {
            debug_info!("Successfully opened /system.ttf");
            debug_info!("  Path: {}", file.path());
            debug_info!("  Size: {} bytes", file.size());
            debug_info!("  Position: {}", file.position());
            assert!(file.size() > 0, "Font file should have non-zero size");
            debug_info!("File open test passed!");
        }
        Err(e) => {
            debug_error!("Failed to open /system.ttf: {:?}", e);
            panic!("Should be able to open /system.ttf for reading");
        }
    }
}

fn test_file_read_arial_header() {
    debug_info!("Testing reading first few bytes of /system.ttf...");
    
    match crate::fs::File::open_read("/system.ttf") {
        Ok(file) => {
            debug_info!("File opened, attempting to read header...");
            
            let mut header = [0u8; 16];
            match file.read(&mut header) {
                Ok(bytes_read) => {
                    debug_info!("Successfully read {} bytes from /system.ttf", bytes_read);
                    debug_info!("Header bytes: {:02x?}", &header[..bytes_read]);
                    
                    // TTF files should start with version info
                    if bytes_read >= 4 {
                        let version = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
                        debug_info!("TTF version: 0x{:08x}", version);
                        // Should be 0x00010000 (1.0) or 0x74727565 ('true')
                        assert!(version == 0x00010000 || version == 0x74727565, 
                               "Should be valid TTF version, got 0x{:08x}", version);
                    }
                    
                    assert!(bytes_read > 0, "Should read at least some bytes");
                    debug_info!("File read test passed!");
                }
                Err(e) => {
                    debug_error!("Failed to read from /system.ttf: {:?}", e);
                    panic!("Should be able to read from /system.ttf");
                }
            }
        }
        Err(e) => {
            debug_error!("Failed to open /system.ttf: {:?}", e);
            panic!("Should be able to open /system.ttf for reading");
        }
    }
}

fn test_file_read_full_arial() {
    debug_info!("Testing reading entire /system.ttf file...");
    
    match crate::fs::File::open_read("/system.ttf") {
        Ok(file) => {
            let size = file.size();
            debug_info!("File size: {} bytes, attempting full read...", size);
            
            // Read entire file
            let mut buffer = alloc::vec![0u8; size as usize];
            match file.read(&mut buffer) {
                Ok(bytes_read) => {
                    debug_info!("Successfully read {} of {} bytes", bytes_read, size);
                    assert_eq!(bytes_read, size as usize, "Should read entire file");
                    
                    // Verify TTF structure
                    if bytes_read >= 12 {
                        let version = u32::from_be_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]);
                        let num_tables = u16::from_be_bytes([buffer[4], buffer[5]]);
                        debug_info!("TTF version: 0x{:08x}, tables: {}", version, num_tables);
                        assert!(num_tables > 0 && num_tables < 100, "Should have reasonable number of tables");
                    }
                    
                    debug_info!("Full file read test passed!");
                }
                Err(e) => {
                    debug_error!("Failed to read full file: {:?}", e);
                    panic!("Should be able to read entire file");
                }
            }
        }
        Err(e) => {
            debug_error!("Failed to open /system.ttf: {:?}", e);
            panic!("Should be able to open /system.ttf for reading");
        }
    }
}

// --- Host folder mount tests (vvfat-backed /host) -----------------------
//
// These rely on:
//   - U2 wiring the vvfat -drive into build.sh / test.sh
//   - U1 staging host_share/HELLO.TXT as the addressable seed fixture
//   - U4 detecting Primary Slave and mounting at /host
//
// HELLO.TXT is uppercase 8.3 by construction so the FAT driver can address
// it by exact name without relying on vvfat's LFN-alias heuristics.

fn test_host_mount_present() {
    debug_info!("Testing /host mount is present in VFS mount list...");

    let vfs = crate::fs::vfs::get_vfs();
    let mut found = false;
    for mount in vfs.list_mounts() {
        debug_info!("  mount: {} ({})", mount.path, mount.filesystem.name());
        if mount.path == "/host" {
            found = true;
        }
    }

    assert!(found, "/host mount should be present in vfs.list_mounts()");
    debug_info!("/host mount present test passed!");
}

fn test_host_mount_can_open_seed_file() {
    debug_info!("Testing read of /host/HELLO.TXT seed file...");
    // The seed fixture must be uppercase 8.3 (the kernel's FAT driver does
    // not parse VFAT long-filename entries). If AGENTICOS_HOST_SHARE is
    // overridden to a folder without HELLO.TXT this test fails loudly so a
    // misconfigured CI surface is obvious.

    match crate::fs::File::open_read("/host/HELLO.TXT") {
        Ok(file) => {
            debug_info!("Opened /host/HELLO.TXT, size = {} bytes", file.size());
            assert!(file.size() > 0, "/host/HELLO.TXT should be non-empty");

            match file.read_to_string() {
                Ok(content) => {
                    debug_info!("/host/HELLO.TXT content: {:?}", content);
                    assert!(!content.is_empty(), "Seed file content should not be empty");
                }
                Err(e) => {
                    debug_error!("Failed to read /host/HELLO.TXT: {:?}", e);
                    panic!("Should be able to read /host/HELLO.TXT as string");
                }
            }
        }
        Err(e) => {
            debug_error!("Failed to open /host/HELLO.TXT: {:?}", e);
            panic!("Should be able to open /host/HELLO.TXT (vvfat-backed seed fixture)");
        }
    }
    debug_info!("Host seed-file open test passed!");
}

fn test_host_mount_does_not_break_root() {
    debug_info!("Testing root mount still works after host mount is wired...");
    // Regression check for the U3 multi-mount refactor: reading a known root
    // file must still succeed once a second FAT mount is in the slot array.

    assert!(crate::fs::exists("/system.ttf"), "/system.ttf should still exist on root");

    match crate::fs::File::open_read("/system.ttf") {
        Ok(file) => {
            assert!(file.size() > 0, "/system.ttf should still have non-zero size");
            debug_info!("/system.ttf still readable from root mount");
        }
        Err(e) => {
            debug_error!("Failed to open /system.ttf after host mount: {:?}", e);
            panic!("Root mount regression: /system.ttf should still open");
        }
    }
    debug_info!("Root-mount regression test passed!");
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_filesystem_basic_exists,
        &test_filesystem_metadata,
        &test_file_open_arial,
        &test_file_read_arial_header,
        &test_file_read_full_arial,
        &test_host_mount_present,
        &test_host_mount_can_open_seed_file,
        &test_host_mount_does_not_break_root,
    ]
}
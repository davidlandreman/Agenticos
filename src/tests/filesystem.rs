use crate::{debug_info, debug_error};
use crate::lib::test_utils::Testable;

fn test_filesystem_basic_exists() {
    debug_info!("Testing filesystem exists() function...");
    
    // Test that /arial.ttf exists
    let exists = crate::fs::exists("/arial.ttf");
    debug_info!("fs::exists(\"/arial.ttf\") = {}", exists);
    assert!(exists, "/arial.ttf should exist in filesystem");
    
    // Test a file that shouldn't exist
    let not_exists = crate::fs::exists("/nonexistent.file");
    debug_info!("fs::exists(\"/nonexistent.file\") = {}", not_exists);
    assert!(!not_exists, "/nonexistent.file should not exist");
}

fn test_filesystem_metadata() {
    debug_info!("Testing filesystem metadata for /arial.ttf...");
    
    match crate::fs::metadata("/arial.ttf") {
        Ok(metadata) => {
            debug_info!("Successfully got metadata for /arial.ttf");
            debug_info!("  Name: {}", metadata.name_str());
            debug_info!("  Size: {} bytes", metadata.size);
            debug_info!("  File type: {:?}", metadata.file_type);
            assert!(metadata.size > 0, "Arial font file should have non-zero size");
        }
        Err(e) => {
            debug_error!("Failed to get metadata for /arial.ttf: {:?}", e);
            panic!("Should be able to get metadata for /arial.ttf");
        }
    }
}

fn test_file_open_arial() {
    debug_info!("Testing File::open_read(\"/arial.ttf\")...");
    
    match crate::fs::File::open_read("/arial.ttf") {
        Ok(file) => {
            debug_info!("Successfully opened /arial.ttf");
            debug_info!("  Path: {}", file.path());
            debug_info!("  Size: {} bytes", file.size());
            debug_info!("  Position: {}", file.position());
            assert!(file.size() > 0, "Font file should have non-zero size");
            debug_info!("File open test passed!");
        }
        Err(e) => {
            debug_error!("Failed to open /arial.ttf: {:?}", e);
            panic!("Should be able to open /arial.ttf for reading");
        }
    }
}

fn test_file_read_arial_header() {
    debug_info!("Testing reading first few bytes of /arial.ttf...");
    
    match crate::fs::File::open_read("/arial.ttf") {
        Ok(file) => {
            debug_info!("File opened, attempting to read header...");
            
            let mut header = [0u8; 16];
            match file.read(&mut header) {
                Ok(bytes_read) => {
                    debug_info!("Successfully read {} bytes from /arial.ttf", bytes_read);
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
                    debug_error!("Failed to read from /arial.ttf: {:?}", e);
                    panic!("Should be able to read from /arial.ttf");
                }
            }
        }
        Err(e) => {
            debug_error!("Failed to open /arial.ttf: {:?}", e);
            panic!("Should be able to open /arial.ttf for reading");
        }
    }
}

fn test_file_read_full_arial() {
    debug_info!("Testing reading entire /arial.ttf file...");
    
    match crate::fs::File::open_read("/arial.ttf") {
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
            debug_error!("Failed to open /arial.ttf: {:?}", e);
            panic!("Should be able to open /arial.ttf for reading");
        }
    }
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_filesystem_basic_exists,
        &test_filesystem_metadata,
        &test_file_open_arial,
        &test_file_read_arial_header,
        &test_file_read_full_arial,
    ]
}
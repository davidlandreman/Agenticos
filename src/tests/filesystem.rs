use crate::lib::test_utils::Testable;
use crate::{debug_error, debug_info};

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
            assert!(
                metadata.size > 0,
                "System font file should have non-zero size"
            );
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
                        let version =
                            u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
                        debug_info!("TTF version: 0x{:08x}", version);
                        // Should be 0x00010000 (1.0) or 0x74727565 ('true')
                        assert!(
                            version == 0x00010000 || version == 0x74727565,
                            "Should be valid TTF version, got 0x{:08x}",
                            version
                        );
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
                        let version =
                            u32::from_be_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]);
                        let num_tables = u16::from_be_bytes([buffer[4], buffer[5]]);
                        debug_info!("TTF version: 0x{:08x}, tables: {}", version, num_tables);
                        assert!(
                            num_tables > 0 && num_tables < 100,
                            "Should have reasonable number of tables"
                        );
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

/// Exercise the production wait path: a spawned kernel thread submits block
/// I/O, parks its PCB, and resumes only after the PCI completion wake is
/// drained into the scheduler.
fn test_virtio_block_wakes_kernel_thread() {
    use core::sync::atomic::{AtomicU8, Ordering};
    static STATE: AtomicU8 = AtomicU8::new(0);

    STATE.store(0, Ordering::Release);
    crate::process::spawn_process(alloc::string::String::from("block-io-test"), None, || {
        let file = crate::fs::File::open_read("/system.ttf").expect("open from kernel thread");
        let mut header = [0u8; 16];
        let count = file.read(&mut header).expect("DMA read from kernel thread");
        assert_eq!(count, header.len());
        assert_eq!(&header[..4], &[0, 1, 0, 0]);
        STATE.store(1, Ordering::Release);
    });

    let deadline = crate::arch::x86_64::interrupts::get_timer_ticks().saturating_add(500);
    while STATE.load(Ordering::Acquire) == 0 {
        let _ = crate::process::drain_kernel_io_wakes();
        crate::process::try_run_scheduled_processes();
        assert!(
            crate::arch::x86_64::interrupts::get_timer_ticks() < deadline,
            "kernel thread did not resume from VirtIO block completion"
        );
        x86_64::instructions::hlt();
    }
}

fn test_zsh_image_read_is_coalesced() {
    let path = "/host/ZSH.ELF";
    if !crate::fs::exists(path) {
        return;
    }
    let file = crate::fs::File::open_read(path).expect("open staged zsh");
    let requests_before = crate::drivers::virtio::block::request_count();
    let ticks_before = crate::arch::x86_64::interrupts::get_timer_ticks();
    let bytes = file.read_to_vec().expect("read staged zsh");
    let requests = crate::drivers::virtio::block::request_count() - requests_before;
    let ticks = crate::arch::x86_64::interrupts::get_timer_ticks() - ticks_before;
    debug_info!(
        "[perf] coalesced zsh image read: {} bytes, {} requests, {} ticks",
        bytes.len(),
        requests,
        ticks
    );
    assert!(
        requests <= 40,
        "zsh image read fragmented into {} block requests",
        requests
    );
}

fn test_busybox_late_page_read_is_bounded() {
    let path = "/host/BB.ELF";
    if !crate::fs::exists(path) {
        return;
    }
    let file = crate::fs::File::open_read(path).expect("open staged BusyBox");
    let offset = (file.size() / 2) & !4095;
    let mut page = [0u8; 4096];
    let requests_before = crate::drivers::virtio::block::request_count();
    let bytes = file
        .read_at(offset, &mut page)
        .expect("read late BusyBox page");
    let requests = crate::drivers::virtio::block::request_count() - requests_before;
    debug_info!(
        "[perf] BusyBox page at {}: {} bytes, {} requests",
        offset,
        bytes,
        requests
    );
    assert_eq!(bytes, page.len());
    assert!(
        requests <= 6,
        "late BusyBox page fragmented into {} block requests",
        requests
    );
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

    assert!(
        crate::fs::exists("/system.ttf"),
        "/system.ttf should still exist on root"
    );

    match crate::fs::File::open_read("/system.ttf") {
        Ok(file) => {
            assert!(
                file.size() > 0,
                "/system.ttf should still have non-zero size"
            );
            debug_info!("/system.ttf still readable from root mount");
        }
        Err(e) => {
            debug_error!("Failed to open /system.ttf after host mount: {:?}", e);
            panic!("Root mount regression: /system.ttf should still open");
        }
    }
    debug_info!("Root-mount regression test passed!");
}

// --- read_to_vec uninit-read regression coverage (U3) -------------------
//
// `read_to_vec` was rewritten to read directly into uninitialized Vec
// capacity (Vec::with_capacity + from_raw_parts_mut + set_len) instead of
// pre-zeroing with Vec::resize. These tests pin the externally observable
// behavior: the returned bytes match what File::read produces into a
// pre-zeroed buffer, the length matches the file size, and short-read /
// zero-byte paths don't expose uninitialized memory.

fn test_read_to_vec_matches_explicit_read() {
    debug_info!("Testing read_to_vec returns the same bytes as a pre-zeroed read...");

    let file = crate::fs::File::open_read("/system.ttf").expect("open /system.ttf");
    let size = file.size() as usize;

    let via_read_to_vec = file.read_to_vec().expect("read_to_vec");
    assert_eq!(via_read_to_vec.len(), size, "length must equal file size");

    let file2 = crate::fs::File::open_read("/system.ttf").expect("re-open /system.ttf");
    let mut explicit = alloc::vec![0u8; size];
    let n = file2.read(&mut explicit).expect("explicit read");
    assert_eq!(n, size, "explicit read should fill the buffer");
    assert_eq!(via_read_to_vec, explicit, "byte-for-byte equal");

    // Spot-check the TTF magic to make sure we're reading file content,
    // not zeros from uninitialized memory that happens to look right.
    assert!(via_read_to_vec.len() >= 4, "TTF needs >= 4 bytes for magic");
    let magic = u32::from_be_bytes([
        via_read_to_vec[0],
        via_read_to_vec[1],
        via_read_to_vec[2],
        via_read_to_vec[3],
    ]);
    assert!(
        magic == 0x00010000 || magic == 0x74727565,
        "TTF magic check; got {:#010x}",
        magic
    );

    debug_info!("read_to_vec matches explicit read");
}

fn test_read_to_vec_length_matches_size_field() {
    // The set_len(bytes_read) call must match File::size() for a file the
    // FAT layer can fully deliver. A short read (bytes_read < size) would
    // shrink the Vec rather than expose uninit; this test pins the
    // happy-path length so a future regression in FAT's read contract
    // (returning a different short count) trips the guard.
    let file = crate::fs::File::open_read("/system.ttf").expect("open");
    let size = file.size() as usize;
    let bytes = file.read_to_vec().expect("read_to_vec");
    assert_eq!(bytes.len(), size);
}

// ---------- Diagnostic timing tests ----------
//
// These print `[perf]` lines so we can read FAT/IDE throughput numbers
// directly off the test serial output. The HELLOCPP test is the actual
// reproducer — it skips silently when the file isn't staged so unrelated
// CI runs aren't blocked, but if a developer stages it and the read hangs,
// these tests will hang too. That's deliberate: it pins the bug.

fn test_fat_read_throughput_system_ttf() {
    use crate::arch::x86_64::interrupts::get_timer_ticks;

    let file = crate::fs::File::open_read("/system.ttf").expect("open /system.ttf");
    let size = file.size();

    let t0 = get_timer_ticks();
    let bytes = file.read_to_vec().expect("read_to_vec");
    let t1 = get_timer_ticks();
    let elapsed = t1.saturating_sub(t0);

    let kib = (bytes.len() as u64) / 1024;
    let kib_per_sec = if elapsed > 0 {
        kib.saturating_mul(100) / elapsed
    } else {
        u64::MAX
    };

    debug_info!(
        "[perf] FAT read /system.ttf: {} bytes in {} ticks ({} ms); ~{} KiB/s",
        bytes.len(),
        elapsed,
        elapsed.saturating_mul(10),
        kib_per_sec,
    );

    assert_eq!(bytes.len(), size as usize);
    assert!(
        elapsed < 6000,
        "system.ttf read exceeded 60 s ({} ticks)",
        elapsed
    );
}

fn test_read_to_vec_vs_pre_zero_baseline() {
    use crate::arch::x86_64::interrupts::get_timer_ticks;

    let path = "/system.ttf";

    // New pattern: current `read_to_vec` (uninit-read + set_len).
    let f_new = crate::fs::File::open_read(path).expect("open new");
    let t0 = get_timer_ticks();
    let new_bytes = f_new.read_to_vec().expect("read_to_vec new");
    let t1 = get_timer_ticks();
    let new_ticks = t1.saturating_sub(t0);

    // Old pattern: Vec::with_capacity + Vec::resize(size, 0) + read.
    let f_old = crate::fs::File::open_read(path).expect("open old");
    let size = f_old.size() as usize;
    let mut buf = alloc::vec::Vec::with_capacity(size);
    let t2 = get_timer_ticks();
    buf.resize(size, 0u8);
    f_old.seek(0).expect("seek");
    let n = f_old.read(&mut buf).expect("read into pre-zeroed");
    let t3 = get_timer_ticks();
    let old_ticks = t3.saturating_sub(t2);
    buf.truncate(n);

    debug_info!(
        "[perf] read_to_vec {} bytes — new (uninit): {} ticks ({} ms); old (prezero): {} ticks ({} ms)",
        size,
        new_ticks,
        new_ticks.saturating_mul(10),
        old_ticks,
        old_ticks.saturating_mul(10),
    );

    assert_eq!(new_bytes, buf, "byte-for-byte equal");
}

fn test_fat_read_throughput_host_hellocpp() {
    use crate::arch::x86_64::interrupts::get_timer_ticks;

    let path = "/host/HELLOCPP.ELF";
    if !crate::fs::exists(path) {
        debug_info!("[perf] {} not present; skipping reproducer test", path);
        return;
    }

    let file = crate::fs::File::open_read(path).expect("open hellocpp");
    let size = file.size();
    debug_info!(
        "[perf] reading {} ({} bytes / {} MiB)...",
        path,
        size,
        size / (1024 * 1024)
    );

    let t0 = get_timer_ticks();
    let bytes = file.read_to_vec().expect("read_to_vec hellocpp");
    let t1 = get_timer_ticks();
    let elapsed = t1.saturating_sub(t0);

    let kib = (bytes.len() as u64) / 1024;
    let kib_per_sec = if elapsed > 0 {
        kib.saturating_mul(100) / elapsed
    } else {
        u64::MAX
    };

    debug_info!(
        "[perf] FAT read {}: {} bytes in {} ticks ({}.{:02}s); ~{} KiB/s",
        path,
        bytes.len(),
        elapsed,
        elapsed / 100,
        elapsed % 100,
        kib_per_sec,
    );

    assert_eq!(bytes.len(), size as usize);
    assert!(
        elapsed < 6000,
        "{} read exceeded 60 s ({} ticks); this is the multi-MiB stall reproducer",
        path,
        elapsed
    );
}

// Full end-to-end run path under test mode: read → load_elf → enter_user_mode
// → user code → exit_group. Skipped silently when /host/HELLOCPP.ELF isn't
// staged so unrelated CI runs aren't blocked.
//
// What this isolates: GUI/mouse/MCP are skipped under `feature = "test"`,
// so if this test passes the load+execute path is sound and any interactive-
// boot stall is a CPU-competition issue, not a load/execute bug. If this
// test hangs or fails, the bug is in the loader, lifecycle, syscall path,
// or the userland binary — not scheduling.
fn test_run_hellocpp_end_to_end() {
    use crate::arch::x86_64::interrupts::get_timer_ticks;
    use crate::userland::lifecycle::{with_active_user, ExitKind};

    let path = "/host/HELLOCPP.ELF";
    if !crate::fs::exists(path) {
        debug_info!("[perf] {} not present; skipping end-to-end test", path);
        return;
    }

    let t0 = get_timer_ticks();
    // Pass `--noecho` so HELLOCPP.ELF skips its stdin-read loop. There's
    // no terminal under test mode to feed bytes; the binary would otherwise
    // block forever inside `read(0, …)`.
    let argv = [path, "--noecho"];
    let result = crate::userland::launcher::launch_user_binary(path, &argv, &[]);
    crate::debug_info!("[perf] launch_user_binary returned {:?}", result);
    let t1 = get_timer_ticks();
    let elapsed = t1.saturating_sub(t0);

    // launcher::launch_user_binary consumes the active-user slot before
    // returning, so we can't inspect (kind, code) from outside. Instead
    // we rely on the debug_info traces inside the launcher and the
    // returned `(ExitKind, i64)` printed above.
    //
    // For the assertion, we re-check the active-user slot is now empty and
    // that user_active() is false — i.e. the run command cleaned up.
    let still_active = with_active_user(|au| au.image.is_some());
    assert!(
        !still_active,
        "active-user slot should be empty after run() returns"
    );

    let _ = ExitKind::None; // silence unused import on the success path

    debug_info!(
        "[perf] run /host/HELLOCPP.ELF end-to-end: {} ticks ({}.{:02}s)",
        elapsed,
        elapsed / 100,
        elapsed % 100,
    );

    assert!(
        elapsed < 6000,
        "/host/HELLOCPP.ELF end-to-end exceeded 60 s ({} ticks)",
        elapsed
    );
}

// --- VFAT long-filename integration coverage (U2) -----------------------
//
// The bundled bootloader's FAT writer (the `bootloader` 0.11 crate)
// emits VFAT LFN entries for every asset. Before U2 the kernel
// silently discarded them and returned only the SFN (e.g.
// `AGENTI~1.BMP` for `agentic-banner.bmp`). These tests pin the
// post-U2 behavior: enumeration surfaces the decoded long names and
// lookups resolve against them.

fn test_enumerate_root_contains_long_lowercase_names() {
    debug_info!("Enumerating / and looking for long/lowercase names...");

    let dir = match crate::fs::Directory::open("/") {
        Ok(d) => d,
        Err(e) => panic!("Directory::open(/) failed: {:?}", e),
    };
    let entries = dir.entries();

    let mut names = alloc::vec::Vec::new();
    for entry in &entries {
        names.push(alloc::string::ToString::to_string(entry.name_str()));
    }
    debug_info!("Root entries ({}): {:?}", names.len(), names);

    // `system.ttf` is in the assets/ source as lowercase 8.3 — the
    // bootloader emits an LFN that decodes to `system.ttf` AND sets
    // the lowercase-attr bits. Either path should surface lowercase.
    assert!(
        names.iter().any(|n| n == "system.ttf"),
        "expected lowercase `system.ttf` in root enumeration; got {:?}",
        names
    );

    // `agentic-banner.bmp` is too long for 8.3 (basename > 8 chars,
    // contains a hyphen). Before U2 the kernel saw `AGENTI~1.BMP`.
    assert!(
        names.iter().any(|n| n == "agentic-banner.bmp"),
        "expected `agentic-banner.bmp` in root enumeration; got {:?}",
        names
    );
}

fn test_stat_returns_long_name() {
    debug_info!("Stat /agentic-banner.bmp must return long name...");
    match crate::fs::metadata("/agentic-banner.bmp") {
        Ok(md) => {
            assert_eq!(md.name_str(), "agentic-banner.bmp");
            assert!(md.size > 0);
        }
        Err(e) => panic!("metadata(/agentic-banner.bmp) failed: {:?}", e),
    }
}

fn test_lookup_resolves_long_lowercase_name() {
    // Existing test_file_open_arial uses /system.ttf which already
    // worked via case-insensitive 8.3 lookup pre-U2. This one
    // exercises a name that REQUIRES LFN decoding (basename > 8
    // chars), so it fails outright without U2.
    debug_info!("Open /agentic-banner.bmp (LFN-only path)...");
    match crate::fs::File::open_read("/agentic-banner.bmp") {
        Ok(f) => {
            assert!(f.size() > 0);
        }
        Err(e) => panic!("open_read(/agentic-banner.bmp) failed: {:?}", e),
    }
}

fn test_lookup_case_insensitive_on_long_name() {
    // Path lookup should still be case-insensitive against the
    // decoded long name — `/AGENTIC-BANNER.BMP` should resolve to
    // `agentic-banner.bmp`.
    match crate::fs::File::open_read("/AGENTIC-BANNER.BMP") {
        Ok(f) => {
            assert!(f.size() > 0);
        }
        Err(e) => panic!("case-insensitive long-name lookup failed: {:?}", e),
    }
}

// --- U7 / Phase C: /data mount present (Secondary Master, whole-disk FAT32) -----

fn test_data_mount_present() {
    let vfs = crate::fs::vfs::get_vfs();
    let mut found = false;
    for mount in vfs.list_mounts() {
        if mount.path == "/data" {
            found = true;
            debug_info!(
                "  /data is mounted with filesystem: {}",
                mount.filesystem.name()
            );
            // U10 makes /data writable.
            assert!(
                !mount.filesystem.is_read_only(),
                "/data should be WRITABLE after Phase C U10"
            );
        }
    }
    assert!(found, "/data must be present in vfs.list_mounts()");
}

fn test_data_create_write_read_round_trip() {
    debug_info!("U10: end-to-end /data write via the public File API");
    let f = crate::fs::File::create("/data/u10-test.txt").expect("create");
    let n = f.write(b"hello from /data\n").expect("write");
    assert_eq!(n, "hello from /data\n".len());
    drop(f);

    let f2 = crate::fs::File::open_read("/data/u10-test.txt").expect("reopen");
    let content = f2.read_to_string().expect("read");
    assert_eq!(content, "hello from /data\n");
    drop(f2);
    crate::fs::vfs::vfs_unlink("/data/u10-test.txt").expect("cleanup u10-test.txt");
}

fn test_data_unlink() {
    debug_info!("U10: unlink on /data via the VFS");
    // Ensure a fresh file (the previous test may have left one).
    let f = crate::fs::File::create("/data/u10-unlink.txt").expect("create");
    f.write(b"x").expect("write");
    drop(f);
    assert!(crate::fs::exists("/data/u10-unlink.txt"));
    crate::fs::vfs::vfs_unlink("/data/u10-unlink.txt").expect("unlink");
    assert!(!crate::fs::exists("/data/u10-unlink.txt"));
}

fn test_data_write_larger_than_one_cluster() {
    // /data is FAT32 with 1 sector per cluster (512 bytes), so 5 KiB
    // forces multi-cluster chain extension. Exercises write_file_at's
    // FAT-link-then-write loop.
    debug_info!("U10: 5 KiB write to /data spans multiple clusters");
    let data: alloc::vec::Vec<u8> = (0..5 * 1024).map(|i| (i & 0xFF) as u8).collect();
    let f = crate::fs::File::create("/data/u10-big.bin").expect("create");
    let mut written = 0;
    while written < data.len() {
        let n = f.write(&data[written..]).expect("write");
        assert!(n > 0);
        written += n;
    }
    drop(f);

    let f2 = crate::fs::File::open_read("/data/u10-big.bin").expect("reopen");
    assert_eq!(f2.size() as usize, data.len());
    let content = f2.read_to_vec().expect("read");
    assert_eq!(content, data);
    drop(f2);
    crate::fs::vfs::vfs_unlink("/data/u10-big.bin").expect("cleanup u10-big.bin");
}

// --- U11 / Phase D: overlay persistence to /data ---------------------

fn test_u11_serialize_deserialize_round_trip() {
    use crate::fs::filesystem::{FileMode, Filesystem};
    use crate::fs::overlay::sync::{deserialize_blob, serialize_upper};
    use crate::fs::tmpfs::Tmpfs;

    let upper = Tmpfs::new();
    upper.mkdir("/etc").expect("mkdir /etc");
    // Put a file in.
    let mut h = upper
        .open(
            "/etc/hello",
            FileMode {
                read: true,
                write: true,
                append: false,
                create: true,
                truncate: true,
            },
        )
        .expect("open create");
    upper.write(&mut h, b"persistent\n").expect("write");
    upper.close(&mut h).expect("close");
    // Whiteout marker.
    let mut wh = upper
        .open(
            "/.wh.system.ttf",
            FileMode {
                read: false,
                write: true,
                append: false,
                create: true,
                truncate: true,
            },
        )
        .expect("create whiteout");
    upper.close(&mut wh).expect("close wh");

    let blob = serialize_upper(&upper);
    let entries = deserialize_blob(&blob).expect("deserialize");
    debug_info!(
        "  serialized {} bytes, {} entries",
        blob.len(),
        entries.len()
    );
    assert!(entries.len() >= 2);
}

fn test_u11_corrupted_blob_rejected() {
    use crate::fs::overlay::sync::deserialize_blob;
    let mut bad = alloc::vec::Vec::from(&b"XXXX\x01\x00\x00\x00\x00\x00"[..]);
    while bad.len() < 32 {
        bad.push(0);
    }
    assert!(deserialize_blob(&bad).is_err());
}

fn test_u11_flush_then_restore_on_live_data() {
    // End-to-end: write a file under `/`, sync (flushes to /data),
    // construct a fresh tmpfs, restore from /data — fresh tmpfs
    // should now contain the file.
    use crate::fs::filesystem::{FileMode, Filesystem};
    use crate::fs::overlay::sync::{flush_upper_to_disk, restore_upper_from_disk};
    use crate::fs::tmpfs::Tmpfs;

    // Get the live overlay's upper layer.
    let vfs = crate::fs::vfs::get_vfs();
    let root = vfs.find_filesystem("/").expect("/ resolvable").0;
    if root.name() != "overlay" {
        debug_info!("  / is not overlay; skipping");
        return;
    }
    let overlay_ptr = root as *const dyn Filesystem as *const crate::fs::overlay::Overlay;
    let overlay: &crate::fs::overlay::Overlay = unsafe { &*overlay_ptr };
    let upper_dyn = overlay.upper();
    let upper_ptr = upper_dyn as *const dyn Filesystem as *const Tmpfs;
    let upper: &Tmpfs = unsafe { &*upper_ptr };

    // Write a marker via the public File API (lands in the overlay
    // upper → tmpfs).
    let f = crate::fs::File::create("/u11-marker.txt").expect("create");
    f.write(b"survived a reboot\n").expect("write");
    drop(f);

    flush_upper_to_disk(upper).expect("flush");

    // Fresh tmpfs; restore from /data; verify our marker shows up.
    let fresh = Tmpfs::new();
    let count = restore_upper_from_disk(&fresh).expect("restore");
    debug_info!("  restore loaded {} entries", count);
    assert!(count >= 1);

    // The marker file should be present in the fresh tmpfs.
    let mut h = fresh
        .open("/u11-marker.txt", FileMode::READ)
        .expect("open marker in fresh tmpfs");
    let mut buf = [0u8; 32];
    let n = fresh.read(&mut h, &mut buf).expect("read");
    assert_eq!(&buf[..n], b"survived a reboot\n");
}

fn test_u11_pointer_flip_is_atomic() {
    // Two successive flushes should land in alternating slots so the
    // commit is single-byte atomic.
    use crate::fs::filesystem::Filesystem;
    use crate::fs::overlay::sync::flush_upper_to_disk;
    use crate::fs::tmpfs::Tmpfs;

    let vfs = crate::fs::vfs::get_vfs();
    let root = vfs.find_filesystem("/").expect("/ resolvable").0;
    if root.name() != "overlay" {
        return;
    }
    let overlay_ptr = root as *const dyn Filesystem as *const crate::fs::overlay::Overlay;
    let overlay: &crate::fs::overlay::Overlay = unsafe { &*overlay_ptr };
    let upper_dyn = overlay.upper();
    let upper_ptr = upper_dyn as *const dyn Filesystem as *const Tmpfs;
    let upper: &Tmpfs = unsafe { &*upper_ptr };

    flush_upper_to_disk(upper).expect("first flush");
    let ptr1 = crate::fs::File::open_read("/data/overlay-state.ptr")
        .expect("open ptr")
        .read_to_vec()
        .expect("read ptr");
    flush_upper_to_disk(upper).expect("second flush");
    let ptr2 = crate::fs::File::open_read("/data/overlay-state.ptr")
        .expect("open ptr")
        .read_to_vec()
        .expect("read ptr");
    // The pointer must have flipped.
    assert_ne!(ptr1, ptr2, "ptr must alternate slots on successive flushes");
    // And must be exactly 1 byte ('0' or '1').
    assert_eq!(ptr2.len(), 1);
    assert!(ptr2[0] == b'0' || ptr2[0] == b'1');

    // These blobs are fixtures for this test sequence, not ambient
    // state for later FAT modules in the same boot.
    for path in [
        "/data/overlay-state.ptr",
        "/data/overlay-state.0",
        "/data/overlay-state.1",
    ] {
        if crate::fs::exists(path) {
            crate::fs::vfs::vfs_unlink(path).expect("cleanup overlay-state fixture");
        }
    }
}

fn test_data_ext2_directory_mutations() {
    let filesystem = crate::fs::vfs::get_vfs()
        .find_filesystem("/data")
        .expect("/data resolvable")
        .0;
    assert_eq!(filesystem.name(), "ext2");
    let _ = crate::fs::vfs::vfs_unlink("/data/ext2-dir/renamed.txt");
    let _ = crate::fs::vfs::vfs_unlink("/data/ext2-dir/nested.txt");
    let _ = crate::fs::vfs::vfs_rmdir("/data/ext2-dir");
    crate::fs::vfs::vfs_mkdir("/data/ext2-dir").expect("mkdir on ext2");
    let file = crate::fs::File::create("/data/ext2-dir/nested.txt").expect("nested create");
    file.write(b"nested ext2 data").expect("nested write");
    drop(file);
    crate::fs::vfs::vfs_rename("/data/ext2-dir/nested.txt", "/data/ext2-dir/renamed.txt")
        .expect("rename on ext2");
    crate::fs::vfs::vfs_unlink("/data/ext2-dir/renamed.txt").expect("unlink nested");
    crate::fs::vfs::vfs_rmdir("/data/ext2-dir").expect("rmdir on ext2");
}

fn test_data_ext2_truncate_links_and_sparse_files() {
    for path in [
        "/data/ext2-target.txt",
        "/data/ext2-hardlink.txt",
        "/data/ext2-symlink.txt",
        "/data/ext2-sparse.bin",
    ] {
        let _ = crate::fs::vfs::vfs_unlink(path);
    }

    let target = crate::fs::File::create("/data/ext2-target.txt").expect("create target");
    target.write(b"abcdefgh").expect("write target");
    target.truncate(3).expect("truncate shrink");
    drop(target);
    assert_eq!(
        crate::fs::File::open_read("/data/ext2-target.txt")
            .expect("reopen target")
            .read_to_string()
            .expect("read target"),
        "abc"
    );

    crate::fs::vfs::vfs_link("/data/ext2-target.txt", "/data/ext2-hardlink.txt")
        .expect("hard link");
    let linked =
        crate::fs::vfs::vfs_unix_metadata("/data/ext2-target.txt").expect("linked metadata");
    assert_eq!(linked.links, 2);
    crate::fs::vfs::vfs_symlink("ext2-target.txt", "/data/ext2-symlink.txt").expect("symlink");
    assert_eq!(
        crate::fs::vfs::vfs_read_link("/data/ext2-symlink.txt").expect("readlink"),
        b"ext2-target.txt"
    );
    let symlink_metadata = crate::fs::vfs::vfs_symlink_metadata("/data/ext2-symlink.txt")
        .expect("lstat-style metadata");
    assert_eq!(symlink_metadata.mode & 0o170000, 0o120000);
    assert_eq!(symlink_metadata.size, b"ext2-target.txt".len() as u64);
    assert_eq!(symlink_metadata.blocks_512, 0);
    assert_eq!(
        crate::fs::File::open_read("/data/ext2-symlink.txt")
            .expect("follow symlink")
            .read_to_string()
            .expect("read symlink target"),
        "abc"
    );

    let sparse = crate::fs::File::create("/data/ext2-sparse.bin").expect("create sparse");
    sparse.truncate(16 * 1024).expect("sparse extend");
    let sparse_meta = sparse.metadata().expect("sparse metadata");
    assert_eq!(sparse_meta.size, 16 * 1024);
    assert_eq!(sparse_meta.blocks_512, 0);
    drop(sparse);
    let sparse_bytes = crate::fs::File::open_read("/data/ext2-sparse.bin")
        .expect("open sparse")
        .read_to_vec()
        .expect("read sparse");
    assert_eq!(sparse_bytes.len(), 16 * 1024);
    assert!(sparse_bytes.iter().all(|byte| *byte == 0));

    crate::fs::vfs::vfs_unlink("/data/ext2-target.txt").expect("unlink original");
    assert_eq!(
        crate::fs::File::open_read("/data/ext2-hardlink.txt")
            .expect("hard link survives")
            .read_to_string()
            .expect("read hard link"),
        "abc"
    );
    for path in [
        "/data/ext2-hardlink.txt",
        "/data/ext2-symlink.txt",
        "/data/ext2-sparse.bin",
    ] {
        crate::fs::vfs::vfs_unlink(path).expect("cleanup ext2 link fixture");
    }
    crate::fs::vfs::vfs_sync_all().expect("sync ext2 fixtures");
}

fn test_data_ext2_indirect_block_boundaries() {
    let file = crate::fs::File::create("/data/ext2-indirect.bin").expect("create indirect file");
    let block_size = file.metadata().expect("indirect metadata").block_size as u64;
    let fanout = block_size / 4;
    let locations = [
        (12 * block_size, 0x51u8),
        ((12 + fanout) * block_size, 0x62),
        ((12 + fanout + fanout * fanout) * block_size, 0x73),
    ];
    for (offset, marker) in locations {
        file.seek(offset).expect("seek into indirect range");
        file.write(&[marker]).expect("write indirect marker");
    }
    let metadata = file.metadata().expect("allocated-block metadata");
    assert_eq!(metadata.blocks_512, 9 * (block_size / 512));
    drop(file);

    let reopened = crate::fs::File::open_read("/data/ext2-indirect.bin").expect("reopen indirect");
    for (offset, marker) in locations {
        let mut byte = [0u8; 1];
        assert_eq!(reopened.read_at(offset, &mut byte).expect("read marker"), 1);
        assert_eq!(byte[0], marker);
    }
    drop(reopened);
    crate::fs::vfs::vfs_unlink("/data/ext2-indirect.bin").expect("unlink indirect file");
}

fn test_data_mount_root_dir_enumerable() {
    // Just-formatted FAT32 has an empty root dir (no entries beyond
    // the volume label, which is filtered out). Enumeration must
    // succeed cleanly.
    let entries = crate::fs::vfs::get_vfs()
        .find_filesystem("/data")
        .expect("/data resolvable")
        .0
        .enumerate_dir("/")
        .expect("enumerate /data");
    debug_info!("  /data contains {} entries", entries.len());
    // QEMU's snapshot=on layer means we don't see writes from earlier
    // test boots; assert empty OR small (just in case the volume label
    // surfaces in some edge case).
    assert!(entries.len() < 4, "/data should be near-empty post-mkfs");
}

// --- U6 / Phase B: writable / mount via overlay(tmpfs, FAT) ----------
//
// These tests exercise the real boot mount, where / is the overlay
// merged view. Reads must still fall through to the FAT lower, and
// writes must land in the tmpfs upper without disturbing lower.

fn test_overlay_root_lower_files_still_readable() {
    // /system.ttf lives only in lower (FAT). Reads must work.
    let file = crate::fs::File::open_read("/system.ttf").expect("open /system.ttf");
    assert!(file.size() > 0);
}

fn test_overlay_root_write_then_read() {
    // Create + write + read a fresh file at /; must not touch lower.
    let f = crate::fs::File::create("/u6-test.txt").expect("create /u6-test.txt");
    let n = f.write(b"hello overlay").expect("write");
    assert_eq!(n, b"hello overlay".len());
    drop(f);

    let f2 = crate::fs::File::open_read("/u6-test.txt").expect("open /u6-test.txt");
    let content = f2.read_to_string().expect("read_to_string");
    assert_eq!(content, "hello overlay");
}

fn test_overlay_root_mkdir_unlink_via_vfs() {
    // mkdir → file inside → unlink the file → rmdir the dir.
    crate::fs::vfs::vfs_mkdir("/u6-dir").expect("mkdir /u6-dir");

    let f = crate::fs::File::create("/u6-dir/inner.txt").expect("create nested");
    f.write(b"x").expect("write");
    drop(f);

    crate::fs::vfs::vfs_unlink("/u6-dir/inner.txt").expect("unlink");
    crate::fs::vfs::vfs_rmdir("/u6-dir").expect("rmdir");

    // Verify gone.
    assert!(!crate::fs::exists("/u6-dir"));
}

fn test_overlay_root_unlink_lower_creates_whiteout() {
    // Unlink a lower-only file. Subsequent stat must return NotFound,
    // but the lower FAT image is untouched (next test confirms by
    // remounting? Not possible mid-test — we just verify the merged
    // view).
    //
    // Use test.txt which is small and present in lower.
    assert!(
        crate::fs::exists("/test.txt"),
        "/test.txt must exist in lower"
    );
    crate::fs::vfs::vfs_unlink("/test.txt").expect("unlink /test.txt");
    assert!(!crate::fs::exists("/test.txt"));
    // Re-create with new content; whiteout should clear.
    let f = crate::fs::File::create("/test.txt").expect("recreate");
    f.write(b"fresh").expect("write");
    drop(f);
    let f2 = crate::fs::File::open_read("/test.txt").expect("open");
    let content = f2.read_to_string().expect("read");
    assert_eq!(content, "fresh");
}

// --- Seek-past-EOF with zero-fill (TinyCC plan U2) --------------------

/// Seek past EOF on the overlay tmpfs, then write: the gap reads back
/// as zeros and the size covers gap + payload. Linker-style writers
/// (and musl stdio fseek on update streams) rely on this.
fn test_seek_past_eof_tmpfs_zero_fill() {
    let f = crate::fs::File::create("/u2-gap.bin").expect("create");
    assert_eq!(f.write(b"abc").expect("head write"), 3);
    assert_eq!(f.seek(100).expect("seek past EOF"), 100);
    assert_eq!(f.write(b"xyz").expect("tail write"), 3);
    assert_eq!(f.size(), 103);
    drop(f);

    let content = crate::fs::File::open_read("/u2-gap.bin")
        .expect("reopen")
        .read_to_vec()
        .expect("read back");
    assert_eq!(content.len(), 103);
    assert_eq!(&content[..3], b"abc");
    assert!(
        content[3..100].iter().all(|&b| b == 0),
        "gap must read back as zeros"
    );
    assert_eq!(&content[100..], b"xyz");
    crate::fs::vfs::vfs_unlink("/u2-gap.bin").expect("cleanup");
}

/// Same on /data (ext2): the gap spans block boundaries and reads back
/// as zeros — ext2 stores it sparsely. (The FAT write path keeps its own
/// explicit gap zero-fill for FAT-writable configurations; the raw-FAT
/// cases in `fat_write` cover that surface when /data contains FAT.)
fn test_seek_past_eof_data_zero_fill() {
    let f = crate::fs::File::create("/data/u2-gap.bin").expect("create");
    assert_eq!(f.write(b"head").expect("head write"), 4);
    assert_eq!(f.seek(1500).expect("seek past EOF"), 1500);
    assert_eq!(f.write(b"tail").expect("tail write"), 4);
    assert_eq!(f.size(), 1504);
    drop(f);

    let content = crate::fs::File::open_read("/data/u2-gap.bin")
        .expect("reopen")
        .read_to_vec()
        .expect("read back");
    assert_eq!(content.len(), 1504);
    assert_eq!(&content[..4], b"head");
    assert!(
        content[4..1500].iter().all(|&b| b == 0),
        "gap must read back as zeros"
    );
    assert_eq!(&content[1500..], b"tail");
    crate::fs::vfs::vfs_unlink("/data/u2-gap.bin").expect("cleanup");
}

/// Read-only mounts keep the historical rejection: seeking past EOF on
/// a /host file still fails, and in-bounds seeks still work.
fn test_seek_past_eof_readonly_rejected() {
    let f = crate::fs::File::open_read("/host/NETTEST.ELF").expect("open /host fixture");
    let size = f.size();
    assert!(size > 0);
    assert!(
        f.seek(size + 10).is_err(),
        "seek past EOF on a read-only mount must fail"
    );
    assert_eq!(f.seek(size).expect("seek to EOF is in bounds"), size);
    assert_eq!(f.seek(0).expect("rewind"), 0);
}

/// /work is provisioned on the overlay tmpfs at every boot and is
/// writable: the conventional scratch directory for compiler output
/// (ring-3 processes start with cwd /host, which is read-only).
fn test_work_directory_provisioned_and_writable() {
    let meta = crate::fs::metadata("/work").expect("/work must exist after boot");
    assert_eq!(
        meta.file_type,
        crate::fs::filesystem::FileType::Directory,
        "/work must be a directory"
    );

    let f = crate::fs::File::create("/work/u3-probe.txt").expect("create in /work");
    assert_eq!(f.write(b"scratch").expect("write in /work"), 7);
    drop(f);
    let content = crate::fs::File::open_read("/work/u3-probe.txt")
        .expect("reopen")
        .read_to_string()
        .expect("read");
    assert_eq!(content, "scratch");
    crate::fs::vfs::vfs_unlink("/work/u3-probe.txt").expect("cleanup");
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_work_directory_provisioned_and_writable,
        &test_seek_past_eof_tmpfs_zero_fill,
        &test_seek_past_eof_data_zero_fill,
        &test_seek_past_eof_readonly_rejected,
        &test_filesystem_basic_exists,
        &test_filesystem_metadata,
        &test_file_open_arial,
        &test_file_read_arial_header,
        &test_file_read_full_arial,
        &test_virtio_block_wakes_kernel_thread,
        &test_zsh_image_read_is_coalesced,
        &test_busybox_late_page_read_is_bounded,
        &test_host_mount_present,
        &test_host_mount_can_open_seed_file,
        &test_host_mount_does_not_break_root,
        &test_read_to_vec_matches_explicit_read,
        &test_read_to_vec_length_matches_size_field,
        &test_fat_read_throughput_system_ttf,
        &test_read_to_vec_vs_pre_zero_baseline,
        &test_fat_read_throughput_host_hellocpp,
        &test_run_hellocpp_end_to_end,
        &test_enumerate_root_contains_long_lowercase_names,
        &test_stat_returns_long_name,
        &test_lookup_resolves_long_lowercase_name,
        &test_lookup_case_insensitive_on_long_name,
        &test_overlay_root_lower_files_still_readable,
        &test_overlay_root_write_then_read,
        &test_overlay_root_mkdir_unlink_via_vfs,
        &test_overlay_root_unlink_lower_creates_whiteout,
        &test_data_mount_present,
        &test_data_mount_root_dir_enumerable,
        &test_data_create_write_read_round_trip,
        &test_data_unlink,
        &test_data_write_larger_than_one_cluster,
        &test_data_ext2_directory_mutations,
        &test_data_ext2_truncate_links_and_sparse_files,
        &test_data_ext2_indirect_block_boundaries,
        &test_u11_serialize_deserialize_round_trip,
        &test_u11_corrupted_blob_rejected,
        &test_u11_flush_then_restore_on_live_data,
        &test_u11_pointer_flip_is_atomic,
    ]
}

//! Booted 9p `/shared` coverage: codec round-trips plus end-to-end
//! create/read/write/rename/unlink/symlink behavior against the per-run host
//! temp share `test.sh` attaches (pre-seeded with `fixture.txt`).
//!
//! Device-dependent tests self-skip when no virtio-9p device exists
//! (`AGENTICOS_SHARED=off`, or a QEMU built without the device), matching the
//! `crate::net::is_available()` convention. When the device IS present, a
//! missing or broken `/shared` mount is a hard failure — a regression there
//! must not hide behind the skip path.

use crate::debug_info;
use crate::fs::filesystem::{FileMode, FilesystemError};
use crate::fs::p9::protocol::{msg, WireReader, WireWriter, TAG};
use crate::fs::vfs::{
    get_vfs, vfs_mkdir, vfs_read_link, vfs_rename, vfs_rmdir, vfs_symlink, vfs_symlink_metadata,
    vfs_unix_metadata, vfs_unlink,
};
use crate::fs::File;
use crate::lib::test_utils::Testable;
use alloc::vec;
use alloc::vec::Vec;

const FIXTURE_CONTENT: &str = "agenticos 9p fixture\n";

fn share_device_present() -> bool {
    !crate::drivers::pci::find_virtio_9p_devices().is_empty()
}

/// Skip helper: true when the test should run. Prints the skip note once per
/// test so filtered runs make the reason visible.
fn share_available_or_skip(test: &str) -> bool {
    if share_device_present() {
        return true;
    }
    debug_info!("  [skip] {}: no virtio-9p device attached", test);
    false
}

fn test_p9_codec_round_trip() {
    let mut writer = WireWriter::request(msg::TWALK, TAG);
    writer.u32(7).u32(9).u16(2);
    writer.string("alpha").string("beta");
    let message = writer.finish();
    // size[4] type[1] tag[2] fid[4] newfid[4] nwname[2] 2*(len[2]+name)
    assert_eq!(message.len(), 7 + 4 + 4 + 2 + 2 + 5 + 2 + 4);
    let mut reader = WireReader::new(&message);
    assert_eq!(reader.u32().unwrap() as usize, message.len());
    assert_eq!(reader.u8().unwrap(), msg::TWALK);
    assert_eq!(reader.u16().unwrap(), TAG);
    assert_eq!(reader.u32().unwrap(), 7);
    assert_eq!(reader.u32().unwrap(), 9);
    assert_eq!(reader.u16().unwrap(), 2);
    assert_eq!(reader.string().unwrap(), "alpha");
    assert_eq!(reader.string().unwrap(), "beta");
    assert_eq!(reader.remaining(), 0);
}

fn test_p9_codec_rejects_truncation() {
    let mut writer = WireWriter::request(msg::TREAD, TAG);
    writer.u32(3).u64(0).u32(4096);
    let message = writer.finish();
    // A reader over a truncated message must error, never panic or read OOB.
    let mut reader = WireReader::new(&message[..message.len() - 3]);
    let _ = reader.u32();
    let _ = reader.u8();
    let _ = reader.u16();
    let _ = reader.u32();
    assert!(reader.u64().is_ok());
    assert_eq!(reader.u32(), Err(FilesystemError::IoError));
    // String length prefixes larger than the buffer are rejected too.
    let bogus = [5u8, 0, b'a'];
    assert!(WireReader::new(&bogus).string().is_err());
}

fn test_p9_shared_mounted() {
    if !share_available_or_skip("shared_mounted") {
        return;
    }
    let vfs = get_vfs();
    let (fs, relative) = vfs
        .find_filesystem("/shared")
        .expect("/shared must be mounted when the virtio-9p device is present");
    assert_eq!(fs.name(), "9p");
    assert_eq!(relative, "/");
    assert!(!fs.is_read_only());
}

fn test_p9_fixture_visible() {
    if !share_available_or_skip("fixture_visible") {
        return;
    }
    let file = File::open_read("/shared/fixture.txt").expect("open host-seeded fixture");
    let content = file.read_to_string().expect("read fixture");
    assert_eq!(content, FIXTURE_CONTENT);
    let metadata = vfs_unix_metadata("/shared/fixture.txt").expect("fixture metadata");
    assert_eq!(metadata.size as usize, FIXTURE_CONTENT.len());
    assert_eq!(metadata.mode & 0o170000, 0o100000, "regular file mode");
}

/// Exercise the production wait path rather than the boot/test-runner `hlt`
/// fallback: a spawned kernel thread must park and resume for each 9p RPC.
fn test_p9_wakes_kernel_thread() {
    use core::sync::atomic::{AtomicU8, Ordering};
    static STATE: AtomicU8 = AtomicU8::new(0);
    static PROGRESS: AtomicU8 = AtomicU8::new(0);

    if !share_available_or_skip("wakes_kernel_thread") {
        return;
    }
    STATE.store(0, Ordering::Release);
    PROGRESS.store(0, Ordering::Release);
    let pid =
        crate::process::spawn_process(alloc::string::String::from("p9-io-test"), None, || {
            PROGRESS.store(1, Ordering::Release);
            let file = File::open_read("/shared/fixture.txt").expect("open 9p from kernel thread");
            PROGRESS.store(2, Ordering::Release);
            let content = file.read_to_string().expect("read 9p from kernel thread");
            PROGRESS.store(3, Ordering::Release);
            assert_eq!(content, FIXTURE_CONTENT);
            drop(file);
            PROGRESS.store(4, Ordering::Release);
            STATE.store(1, Ordering::Release);
        });

    let deadline = crate::arch::x86_64::interrupts::get_timer_ticks().saturating_add(500);
    while STATE.load(Ordering::Acquire) == 0 {
        let _ = crate::process::drain_kernel_io_wakes();
        crate::process::try_run_scheduled_processes();
        if crate::arch::x86_64::interrupts::get_timer_ticks() >= deadline {
            let scheduler = crate::process::scheduler::SCHEDULER.lock();
            crate::debug_error!(
                "9p wake timeout: pid={} progress={} entity={:?} current={:?} ready={} pending_wakes={}",
                pid,
                PROGRESS.load(Ordering::Acquire),
                scheduler.entity_diagnostics_for_test(
                    crate::process::entity::EntityId::KernelThread(pid)
                ),
                scheduler.current_entity(),
                scheduler.ready_entity_count(),
                crate::process::pending_kernel_io_wakes_for_test(),
            );
            panic!("kernel thread did not resume from VirtIO 9p completion");
        }
        x86_64::instructions::hlt();
    }
}

/// Git preloads its index from several workers. Each worker can enter the 9p
/// backend while another is asleep in an RPC, so client serialization must
/// park contenders rather than spin in ring 0 behind the sleeping owner.
fn test_p9_concurrent_callers_park() {
    use core::sync::atomic::{AtomicUsize, Ordering};
    static DONE: AtomicUsize = AtomicUsize::new(0);
    static STARTED: AtomicUsize = AtomicUsize::new(0);

    if !share_available_or_skip("concurrent_callers_park") {
        return;
    }
    const CALLERS: usize = 4;
    DONE.store(0, Ordering::Release);
    STARTED.store(0, Ordering::Release);
    for _ in 0..CALLERS {
        crate::process::spawn_process(
            alloc::string::String::from("p9-concurrent-test"),
            None,
            || {
                STARTED.fetch_add(1, Ordering::AcqRel);
                let file = File::open_read("/shared/fixture.txt")
                    .expect("concurrent open from kernel thread");
                let content = file
                    .read_to_string()
                    .expect("concurrent read from kernel thread");
                assert_eq!(content, FIXTURE_CONTENT);
                drop(file);
                DONE.fetch_add(1, Ordering::AcqRel);
            },
        );
    }

    let deadline = crate::arch::x86_64::interrupts::get_timer_ticks().saturating_add(500);
    while DONE.load(Ordering::Acquire) != CALLERS {
        let _ = crate::process::drain_kernel_io_wakes();
        crate::process::try_run_scheduled_processes();
        if crate::arch::x86_64::interrupts::get_timer_ticks() >= deadline {
            panic!(
                "concurrent 9p callers stalled: started={} done={}",
                STARTED.load(Ordering::Acquire),
                DONE.load(Ordering::Acquire)
            );
        }
        x86_64::instructions::hlt();
    }
}

/// Ring-3 reproduction for the zsh/Git freeze: several forked BusyBox workers
/// hit `/shared` together on one process home's CPU.
fn test_p9_ring3_concurrent_callers_park() {
    if !share_available_or_skip("ring3_concurrent_callers_park") {
        return;
    }
    let path = crate::userland::bin_namespace::BB_HOST_PATH;
    assert!(
        crate::fs::exists(path),
        "mandatory fixture missing: {}",
        path
    );
    let command = "cat /shared/fixture.txt >/dev/null & \
                   cat /shared/fixture.txt >/dev/null & \
                   cat /shared/fixture.txt >/dev/null & \
                   cat /shared/fixture.txt >/dev/null & wait";
    let result = crate::userland::launcher::launch_user_binary(
        path,
        &["sh", "-c", command],
        &crate::userland::process_service::DEFAULT_USER_ENV,
    );
    crate::userland::abi::clear_user_va_bounds();
    let (kind, code) = result.unwrap_or_else(|error| panic!("BusyBox launch failed: {}", error));
    assert!(
        matches!(kind, crate::userland::lifecycle::ExitKind::Cooperative),
        "BusyBox concurrent 9p workers exited via {:?} ({})",
        kind,
        code
    );
    assert_eq!(code, 0, "BusyBox concurrent 9p workers failed");
}

/// Git mmaps regular-file input and drops the mapping during munmap. The VMA
/// owns an `Arc<File>`, whose last drop sends Tclunk and sleeps on 9p I/O; it
/// must therefore be destroyed only after PROCESS_TABLE has been unlocked.
fn test_p9_git_file_mmap_unmap() {
    if !share_available_or_skip("git_file_mmap_unmap") {
        return;
    }
    let path = crate::userland::bin_namespace::GIT_HOST_PATH;
    assert!(
        crate::fs::exists(path),
        "mandatory fixture missing: {}",
        path
    );
    let previous_trace = crate::userland::abi::is_trace_mode();
    crate::userland::abi::set_trace_mode(true);
    let result = crate::userland::launcher::launch_user_binary(
        path,
        &["git", "hash-object", "/shared/fixture.txt"],
        &crate::userland::process_service::DEFAULT_USER_ENV,
    );
    crate::userland::abi::set_trace_mode(previous_trace);
    crate::userland::abi::clear_user_va_bounds();
    let (kind, code) = result.unwrap_or_else(|error| panic!("git hash-object failed: {}", error));
    assert!(
        matches!(kind, crate::userland::lifecycle::ExitKind::Cooperative),
        "git hash-object exited via {:?} ({})",
        kind,
        code
    );
    assert_eq!(code, 0, "git hash-object failed");
}

/// Opt-in real-share reproducer used with
/// `AGENTICOS_TEST_SHARED_DIR=~/.agenticos/shared`. The hermetic test share
/// has no `make` checkout, so ordinary runs self-skip this workload.
fn test_p9_real_make_git_status() {
    if !crate::fs::exists("/shared/make/.git") {
        debug_info!("  [skip] real_make_git_status: /shared/make/.git absent");
        return;
    }
    let path = crate::userland::bin_namespace::GIT_HOST_PATH;
    // Exercise a cold and warm status. Attributes remain fresh on both; the
    // second pass verifies that reusable path fids remove walk/clunk traffic.
    // A completion path accidentally deferred to the 100 Hz PIT takes roughly
    // 40 seconds here, so keep the reproducer bounded below that regression.
    let previous_timeout = crate::process::set_inline_ring3_test_timeout_ticks(3_000);
    let previous_trace = crate::userland::abi::is_trace_mode();
    crate::userland::abi::set_trace_mode(true);
    for pass in 0..2 {
        let start_ticks = crate::arch::x86_64::interrupts::get_timer_ticks();
        let start_requests = crate::drivers::virtio::p9::request_count();
        let start_types =
            [110u8, 24, 120, 40, 12, 116].map(crate::drivers::virtio::p9::request_type_count);
        let result = crate::userland::launcher::launch_user_binary(
            path,
            &["git", "-C", "/shared/make", "status", "--porcelain=v1"],
            &crate::userland::process_service::DEFAULT_USER_ENV,
        );
        crate::userland::abi::clear_user_va_bounds();
        let elapsed_ticks = crate::arch::x86_64::interrupts::get_timer_ticks() - start_ticks;
        let requests = crate::drivers::virtio::p9::request_count() - start_requests;
        let request_types =
            [110u8, 24, 120, 40, 12, 116].map(crate::drivers::virtio::p9::request_type_count);
        let request_types =
            core::array::from_fn::<_, 6, _>(|index| request_types[index] - start_types[index]);
        let (kind, code) =
            result.unwrap_or_else(|error| panic!("git status launch failed: {}", error));
        debug_info!(
            "  real_make_git_status pass={} outcome: {:?} ({}) ticks={} p9_rpcs={} max_inflight={} walk/getattr/clunk/readdir/open/read={:?}",
            pass + 1,
            kind,
            code,
            elapsed_ticks,
            requests,
            crate::drivers::virtio::p9::max_requests_in_flight(),
            request_types
        );
        assert!(
            matches!(kind, crate::userland::lifecycle::ExitKind::Cooperative),
            "git status exited via {:?} ({})",
            kind,
            code
        );
        assert_eq!(code, 0, "git status failed");
    }

    // Exercise the actual interactive prompt path as well as Git directly.
    // It must render the real branch/status segment; the old `/shared`
    // approximation skipped status and displayed an `≈` marker instead.
    let start_ticks = crate::arch::x86_64::interrupts::get_timer_ticks();
    let start_requests = crate::drivers::virtio::p9::request_count();
    let zsh_path = crate::userland::process_service::ZSH_HOST_PATH;
    let prompt_command = r#"cd /shared/make; build_prompt; [[ "$PROMPT" == *$'\ue0a0'* ]] || exit 42; [[ "$PROMPT" != *'≈'* ]] || exit 43"#;
    let result = crate::userland::launcher::launch_user_binary(
        zsh_path,
        &["zsh", "+m", "-ic", prompt_command],
        &crate::userland::process_service::DEFAULT_USER_ENV,
    );
    crate::userland::abi::clear_user_va_bounds();
    let elapsed_ticks = crate::arch::x86_64::interrupts::get_timer_ticks() - start_ticks;
    let requests = crate::drivers::virtio::p9::request_count() - start_requests;
    let (kind, code) = result.unwrap_or_else(|error| panic!("zsh prompt launch failed: {}", error));
    debug_info!(
        "  real_make_git_prompt outcome: {:?} ({}) ticks={} p9_rpcs={}",
        kind,
        code,
        elapsed_ticks,
        requests
    );
    assert!(
        matches!(kind, crate::userland::lifecycle::ExitKind::Cooperative),
        "zsh prompt exited via {:?} ({})",
        kind,
        code
    );
    assert_eq!(code, 0, "zsh prompt did not render real Git status");
    crate::userland::abi::set_trace_mode(previous_trace);
    crate::process::set_inline_ring3_test_timeout_ticks(previous_timeout);
}

fn test_p9_create_write_read_back() {
    if !share_available_or_skip("create_write_read_back") {
        return;
    }
    let path = "/shared/p9-create-test.txt";
    {
        let file = File::create(path).expect("create file on /shared");
        let written = file.write(b"hello from ring 0\n").expect("write");
        assert_eq!(written, 18);
        file.sync(false).expect("fsync");
    }
    let file = File::open_read(path).expect("reopen");
    assert_eq!(
        file.read_to_string().expect("read back"),
        "hello from ring 0\n"
    );
    let metadata = vfs_unix_metadata(path).expect("metadata");
    assert_eq!(metadata.size, 18);
    drop(file);
    vfs_unlink(path).expect("unlink");
    assert!(File::open_read(path).is_err(), "unlinked file must be gone");
}

fn test_p9_truncate_and_append() {
    if !share_available_or_skip("truncate_and_append") {
        return;
    }
    let path = "/shared/p9-truncate-test.txt";
    let file = File::create(path).expect("create");
    file.write(b"0123456789").expect("seed");
    file.truncate(4).expect("truncate to 4");
    drop(file);
    let append = File::open(
        path,
        FileMode {
            read: true,
            write: true,
            append: true,
            create: false,
            truncate: false,
        },
    )
    .expect("open for append");
    append.write(b"XY").expect("append");
    drop(append);
    let content = File::open_read(path)
        .expect("reopen")
        .read_to_string()
        .expect("read");
    assert_eq!(content, "0123XY");
    vfs_unlink(path).expect("unlink");
}

fn test_p9_large_file_multi_chunk() {
    if !share_available_or_skip("large_file_multi_chunk") {
        return;
    }
    // 256 KiB: several msize-bounded Twrite/Tread RPCs per direction.
    let path = "/shared/p9-large-test.bin";
    let pattern: Vec<u8> = (0..256 * 1024)
        .map(|index: u32| (index.wrapping_mul(31).wrapping_add(7) & 0xFF) as u8)
        .collect();
    {
        let file = File::create(path).expect("create large");
        assert_eq!(file.write(&pattern).expect("write large"), pattern.len());
    }
    let file = File::open_read(path).expect("reopen large");
    let read_back = file.read_to_vec().expect("read large");
    assert_eq!(read_back.len(), pattern.len());
    assert_eq!(read_back, pattern, "large round-trip must be byte-exact");
    // Offset read across a chunk boundary.
    let mut window = vec![0u8; 64];
    let offset = 100_000u64;
    let read = file.read_at(offset, &mut window).expect("read_at");
    assert_eq!(read, 64);
    assert_eq!(window, pattern[offset as usize..offset as usize + 64]);
    drop(file);
    vfs_unlink(path).expect("unlink large");
}

fn test_p9_mkdir_enumerate_rmdir() {
    if !share_available_or_skip("mkdir_enumerate_rmdir") {
        return;
    }
    vfs_mkdir("/shared/p9-dir").expect("mkdir");
    assert_eq!(
        vfs_mkdir("/shared/p9-dir"),
        Err(FilesystemError::AlreadyExists),
        "duplicate mkdir must fail"
    );
    File::create("/shared/p9-dir/one.txt")
        .expect("create one")
        .write(b"1")
        .expect("write one");
    File::create("/shared/p9-dir/two.txt")
        .expect("create two")
        .write(b"22")
        .expect("write two");

    let directory = crate::fs::Directory::open("/shared/p9-dir").expect("open dir");
    let entries = directory.entries();
    assert_eq!(
        entries.len(),
        2,
        "directory must list exactly the two files"
    );
    let mut sizes = [0u64; 2];
    for entry in &entries {
        match entry.name_str() {
            "one.txt" => sizes[0] = entry.size,
            "two.txt" => sizes[1] = entry.size,
            other => panic!("unexpected directory entry: {}", other),
        }
    }
    assert_eq!(sizes, [1, 2], "enumerate_dir must carry real sizes");

    assert_eq!(
        vfs_rmdir("/shared/p9-dir"),
        Err(FilesystemError::NotEmpty),
        "rmdir of a populated directory must fail"
    );
    vfs_unlink("/shared/p9-dir/one.txt").expect("unlink one");
    vfs_unlink("/shared/p9-dir/two.txt").expect("unlink two");
    vfs_rmdir("/shared/p9-dir").expect("rmdir emptied dir");
}

fn test_p9_rename_within_and_across_dirs() {
    if !share_available_or_skip("rename") {
        return;
    }
    vfs_mkdir("/shared/p9-rename-dir").expect("mkdir");
    File::create("/shared/p9-rename-a.txt")
        .expect("create")
        .write(b"payload")
        .expect("write");
    vfs_rename("/shared/p9-rename-a.txt", "/shared/p9-rename-b.txt").expect("rename in dir");
    assert!(File::open_read("/shared/p9-rename-a.txt").is_err());
    vfs_rename(
        "/shared/p9-rename-b.txt",
        "/shared/p9-rename-dir/p9-rename-c.txt",
    )
    .expect("rename across directories");
    let content = File::open_read("/shared/p9-rename-dir/p9-rename-c.txt")
        .expect("open renamed")
        .read_to_string()
        .expect("read renamed");
    assert_eq!(content, "payload");
    vfs_unlink("/shared/p9-rename-dir/p9-rename-c.txt").expect("unlink");
    vfs_rmdir("/shared/p9-rename-dir").expect("rmdir");
}

fn test_p9_symlink_round_trip() {
    if !share_available_or_skip("symlink_round_trip") {
        return;
    }
    let link = "/shared/p9-fixture-link";
    vfs_symlink("fixture.txt", link).expect("symlink");
    let target = vfs_read_link(link).expect("readlink");
    assert_eq!(target.as_slice(), b"fixture.txt");
    let metadata = vfs_symlink_metadata(link).expect("lstat");
    assert_eq!(metadata.mode & 0o170000, 0o120000, "symlink mode");
    // Opening the link resolves to the fixture's content.
    let content = File::open_read(link)
        .expect("open through symlink")
        .read_to_string()
        .expect("read through symlink");
    assert_eq!(content, FIXTURE_CONTENT);
    vfs_unlink(link).expect("unlink symlink");
}

fn test_p9_missing_paths_error() {
    if !share_available_or_skip("missing_paths_error") {
        return;
    }
    assert!(File::open_read("/shared/definitely-not-here.txt").is_err());
    assert!(matches!(
        vfs_unix_metadata("/shared/definitely-not-here.txt"),
        Err(FilesystemError::NotFound)
    ));
    assert_eq!(
        vfs_unlink("/shared/definitely-not-here.txt"),
        Err(FilesystemError::NotFound)
    );
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_p9_codec_round_trip,
        &test_p9_codec_rejects_truncation,
        &test_p9_shared_mounted,
        &test_p9_fixture_visible,
        &test_p9_wakes_kernel_thread,
        &test_p9_concurrent_callers_park,
        &test_p9_ring3_concurrent_callers_park,
        &test_p9_git_file_mmap_unmap,
        &test_p9_real_make_git_status,
        &test_p9_create_write_read_back,
        &test_p9_truncate_and_append,
        &test_p9_large_file_multi_chunk,
        &test_p9_mkdir_enumerate_rmdir,
        &test_p9_rename_within_and_across_dirs,
        &test_p9_symlink_round_trip,
        &test_p9_missing_paths_error,
    ]
}

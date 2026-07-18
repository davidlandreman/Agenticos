use crate::arch::x86_64::{gdt, interrupts};
use crate::drivers::display::display;
use crate::drivers::ps2_controller;
use crate::lib::debug::{self, DebugLevel};
use crate::mm::memory;
use crate::window;
use crate::{debug_debug, debug_info, debug_warn};
use alloc::boxed::Box;
use bootloader_api::BootInfo;

pub fn init(boot_info: &'static mut BootInfo) {
    // Initialize debug subsystem
    debug::init();
    debug::set_debug_level(DebugLevel::Debug);

    debug_info!("=== AgenticOS Kernel Starting ===");
    debug_debug!("Boot info address: {:p}", boot_info);

    // Pull the test filter from QEMU fw_cfg before tests run. Pure port I/O,
    // safe pre-heap, silent on real hardware. No-op outside test builds.
    #[cfg(feature = "test")]
    crate::tests::filter::init();

    // Rendering policy is another small read-only fw_cfg input. It is parsed
    // before display/window-manager initialization and defaults to legacy.
    crate::window::renderer::init_boot_policy();

    // Enable SSE/SSE2 in CR0/CR4 before any code path that could end up in
    // ring 3 (loader → enter_user_mode). musl + libstdc++ binaries emit SSE2
    // in `__init_tls` before reaching `main`; without this the first SSE
    // instruction the user app issues `#UD`s.
    crate::arch::x86_64::fpu::enable_sse();

    // Install GDT + TSS before the IDT — exception handlers that reference
    // IST entries (e.g., #DF) consult the TSS at fault time, so it must be in
    // TR before any such fault can fire.
    debug_info!("[boot] gdt+idt");
    gdt::init();
    // Per-CPU SYSCALL scratch struct + GS_BASE/KERNEL_GS_BASE programming.
    // Both GS bases point at PERCPU so the first swapgs on SYSCALL entry is
    // a no-op regardless of MSR ordering.
    crate::arch::x86_64::syscall::init_percpu();
    // Program EFER.SCE + STAR/LSTAR/FMASK so user-mode `syscall` lands at
    // `syscall_fastpath_entry`. Must run after gdt::init() (so STAR sees
    // the right selectors) and after init_percpu() (so the first swapgs
    // hits a valid kernel GS).
    crate::arch::x86_64::syscall::init_syscall_msrs();
    interrupts::init_idt();
    ps2_controller::init();

    // Extract what we need from boot_info before borrowing it
    // Use a default offset if not provided by bootloader
    let physical_memory_offset = boot_info
        .physical_memory_offset
        .into_option()
        .unwrap_or(0x10000000000); // Default offset for identity mapping
    let rsdp_addr = boot_info.rsdp_addr.into_option();

    debug_info!("[boot] heap");
    unsafe {
        // Create a reference that will live for the entire program
        let memory_regions_ref: &'static _ = &*((&boot_info.memory_regions) as *const _);
        memory::init(memory_regions_ref, Some(physical_memory_offset));
        memory::init_heap(physical_memory_offset);
    }

    // Parse and install the system font. Must run after heap init (the TTF
    // rasterizer allocates) and before any window or TTY is constructed (the
    // grid layout reads the font's cell dimensions exactly once).
    debug_info!("[boot] fonts");
    crate::graphics::fonts::core_font::init_fonts();

    debug_info!("[boot] scheduler");
    crate::process::init_scheduler();

    // Linux ABI syscall surface (write/exit_group, plus the broader set in
    // U9) is dispatched directly from `userland::abi::syscall_dispatch` —
    // no per-syscall registration needed.

    debug_info!("[boot] ide+fs");
    crate::drivers::ide::IDE_CONTROLLER.initialize();
    init_filesystems();
    // Host-disk probe is small (one MBR read on the slave drive) and the
    // filesystem tests assert /host is mounted, so we keep it in test mode.
    try_mount_host_disk();
    try_mount_data_disk();
    // Phase D U11: now that /data is mounted (if present), restore
    // the overlay's upper-layer tmpfs from any persistent state.
    // No-op when /data is missing or has no prior sync.
    restore_overlay_upper_from_data();

    debug_info!("[boot] managed /etc");
    crate::userland::etc::init();

    // Networking publishes DHCP-owned resolver state into the now-mounted
    // root overlay. Production boot remains asynchronous and never waits for
    // a lease.
    debug_info!("[boot] network");
    crate::net::init();

    if let Some(rsdp_addr) = rsdp_addr {
        debug_debug!("RSDP address: 0x{:016x}", rsdp_addr);
    }
    memory::print_memory_info();

    debug_info!("[boot] display");
    let screen_dims = init_display(boot_info);

    // Mouse, GUIShell desktop, and MCP bridge are not exercised by any
    // in-kernel test. Skipping them under `feature = "test"` removes the
    // largest chunks of `./test.sh` startup latency: GUIShell paints the
    // wallpaper + initial render; the MCP bridge spawns a kernel process
    // and brings up COM2.
    #[cfg(not(feature = "test"))]
    {
        debug_info!("[boot] mouse");
        if let Some((width, height)) = screen_dims {
            crate::drivers::mouse::init_with_screen(width, height);
        } else {
            crate::drivers::mouse::init();
        }

        debug_info!("[boot] guishell");
        init_guishell_desktop();

        debug_info!("[boot] mcp-bridge");
        init_mcp_bridge();
    }

    #[cfg(feature = "test")]
    {
        let _ = screen_dims;
        debug_info!("[boot] test mode — skipping mouse, guishell, mcp");
    }

    debug_info!("[boot] init complete");
}

#[cfg(not(feature = "test"))]
fn init_guishell_desktop() {
    crate::commands::guishell::init_guishell();
    // Do an initial render so the desktop is visible immediately.
    window::render_frame();
}

#[cfg(not(feature = "test"))]
fn init_mcp_bridge() {
    use alloc::boxed::Box;
    use alloc::string::String;

    debug_info!("Initializing MCP tool registry...");

    crate::drivers::serial::init();
    crate::tools::init();

    if let Some(reg_lock) = crate::tools::registry() {
        let mut reg = reg_lock.lock();
        // screenshot is implemented but not registered in v1: byte-by-byte
        // UART transmission of a multi-MB framebuffer is prohibitively slow
        // (each byte is a vmexit). Re-enable once the transport swaps to
        // virtio-serial or IRQ-driven UART (deferred per plan).
        // reg.register(Box::new(crate::tools::screenshot::Screenshot));
        reg.register(Box::new(crate::tools::shell_run::ShellRun));
        reg.register(Box::new(crate::tools::send_input::SendInput));
        reg.register(Box::new(crate::tools::kernel_state::KernelState));
        debug_info!("Registered {} kernel tools", reg.enumerate().len());
    }

    // Synthetic terminal id used by shell_run to capture stdout. Must be
    // registered explicitly; otherwise write_to_terminal_id silently drops.
    crate::window::terminal::register_terminal(crate::tools::shell_run::RPC_TERMINAL_ID);

    // Spawn the dispatcher loop as a kernel process so long tool calls don't
    // stall the main event loop or input.
    crate::process::spawn_process(String::from("rpc-dispatcher"), None, || {
        crate::tools::rpc::run_dispatcher()
    });

    debug_info!("MCP dispatcher process spawned; serving on COM2");
}

fn init_display(boot_info: &'static mut BootInfo) -> Option<(u32, u32)> {
    let Some(framebuffer) = boot_info.framebuffer.as_mut() else {
        debug_warn!("No framebuffer available from bootloader");
        return None;
    };

    let width = framebuffer.info().width as u32;
    let height = framebuffer.info().height as u32;
    debug_debug!("Screen dimensions: {}x{}", width, height);

    let device: Box<dyn window::GraphicsDevice> = if display::USE_DOUBLE_BUFFER {
        Box::new(window::adapters::DoubleBufferedDevice::new_with_static_buffer(framebuffer))
    } else {
        Box::new(window::adapters::DirectFrameBufferDevice::new(framebuffer))
    };

    window::init_window_manager(device);
    Some((width, height))
}

// Static storage for IDE block devices and partition devices
static mut PRIMARY_MASTER_DISK: Option<crate::drivers::ide::IdeBlockDevice> = None;
static mut PARTITION_DEVICES: [Option<crate::fs::PartitionBlockDevice<'static>>; 4] =
    [None, None, None, None];

// Static storage for the host-share disk on Primary Slave (vvfat-backed when
// the user runs ./build.sh; absent otherwise). Kept in a separate slot/array
// so the host disk's partitions don't alias the root disk's PARTITION_DEVICES.
static mut PRIMARY_SLAVE_DISK: Option<crate::drivers::ide::IdeBlockDevice> = None;
static mut HOST_PARTITION_DEVICES: [Option<crate::fs::PartitionBlockDevice<'static>>; 4] =
    [None, None, None, None];

/// Secondary Master = the writable /data disk. Whole-disk FAT32 (no
/// MBR/partition table); the BPB lives at sector 0.
static mut SECONDARY_MASTER_DISK: Option<crate::drivers::ide::IdeBlockDevice> = None;

fn init_filesystems() {
    use crate::drivers::block::BlockDevice;
    use crate::drivers::ide::{IdeBlockDevice, IdeChannel, IdeDrive, IDE_CONTROLLER};
    use crate::fs::vfs::mount_overlay_root;
    use crate::fs::{detect_filesystem, read_partitions, PartitionBlockDevice};

    debug_info!("Detecting and mounting filesystems...");

    // Check primary master disk
    if let Some((model_bytes, sectors)) =
        IDE_CONTROLLER.get_disk_info(IdeChannel::Primary, IdeDrive::Master)
    {
        let size_mb = (sectors * 512) / (1024 * 1024);

        // Convert model bytes to string
        let model_len = model_bytes.iter().position(|&c| c == 0).unwrap_or(40);
        let model = core::str::from_utf8(&model_bytes[..model_len])
            .unwrap_or("Unknown")
            .trim();

        debug_info!("Found IDE disk: {} ({} MB)", model, size_mb);

        // Create block device for the disk and store it statically
        unsafe {
            PRIMARY_MASTER_DISK = Some(IdeBlockDevice::new(IdeChannel::Primary, IdeDrive::Master));
        }

        let primary_master = unsafe { (*&raw const PRIMARY_MASTER_DISK).as_ref().unwrap() };

        // Try to read the boot sector
        let mut boot_sector = [0u8; 512];
        match primary_master.read_blocks(0, 1, &mut boot_sector) {
            Ok(_) => {
                debug_info!("Successfully read boot sector");

                // Check for valid MBR signature
                if boot_sector[510] == 0x55 && boot_sector[511] == 0xAA {
                    debug_info!("Valid boot sector signature found");

                    // Try to read partition table
                    match read_partitions(primary_master) {
                        Ok(partitions) => {
                            let mut partition_num = 0;
                            let mut first_valid_partition = None;

                            // First pass: create partition devices and store them
                            for (i, partition) in partitions.iter().enumerate() {
                                if let Some(part) = partition {
                                    partition_num += 1;
                                    debug_info!(
                                        "Partition {}: Type={:?}, Start={}, Size={} sectors",
                                        i + 1,
                                        part.partition_type,
                                        part.start_lba,
                                        part.size_sectors
                                    );

                                    // Create a partition device and store it statically
                                    unsafe {
                                        PARTITION_DEVICES[i] =
                                            Some(PartitionBlockDevice::new(primary_master, part));
                                    }

                                    // Get a reference to the stored partition device
                                    let part_device =
                                        unsafe { PARTITION_DEVICES[i].as_ref().unwrap() };

                                    match detect_filesystem(part_device) {
                                        Ok(fs_type) => {
                                            debug_info!("  Detected filesystem: {:?}", fs_type);
                                            // Only consider FAT filesystems as valid for mounting
                                            use crate::fs::FilesystemType;
                                            if first_valid_partition.is_none()
                                                && matches!(
                                                    fs_type,
                                                    FilesystemType::Fat12
                                                        | FilesystemType::Fat16
                                                        | FilesystemType::Fat32
                                                )
                                            {
                                                first_valid_partition = Some(i);
                                            }
                                        }
                                        Err(_) => {
                                            debug_info!(
                                                "  Unknown filesystem on partition {}",
                                                i + 1
                                            );
                                        }
                                    }
                                }
                            }

                            // Mount the first valid partition as root, wrapped in
                            // an overlay(tmpfs over FAT) so userland sees a
                            // writable namespace without mutating the immutable
                            // boot image.
                            if let Some(part_idx) = first_valid_partition {
                                let part_device =
                                    unsafe { PARTITION_DEVICES[part_idx].as_ref().unwrap() };
                                match mount_overlay_root(part_device) {
                                    Ok(_) => {
                                        debug_info!(
                                            "Mounted overlay(tmpfs over FAT partition {}) at /",
                                            part_idx + 1
                                        );
                                    }
                                    Err(e) => {
                                        // Per doc-review #A-5: panic loudly rather than
                                        // silently downgrading to a read-only FAT mount.
                                        // Userland after Phase B expects / to be writable;
                                        // silent degradation would have zsh / scripts
                                        // appear to succeed then EROFS halfway through.
                                        panic!("FATAL: overlay root mount failed: {:?}", e);
                                    }
                                }
                            }

                            if partition_num == 0 {
                                debug_info!(
                                    "No partitions found, checking whole disk for filesystem"
                                );
                                // Try to detect filesystem on whole disk
                                match detect_filesystem(primary_master) {
                                    Ok(fs_type) => {
                                        debug_info!(
                                            "Detected filesystem on whole disk: {:?}",
                                            fs_type
                                        );
                                        // Only mount if it's a supported FAT filesystem
                                        use crate::fs::FilesystemType;
                                        if matches!(
                                            fs_type,
                                            FilesystemType::Fat12
                                                | FilesystemType::Fat16
                                                | FilesystemType::Fat32
                                        ) {
                                            match mount_overlay_root(primary_master) {
                                                Ok(_) => {
                                                    debug_info!("Mounted overlay(tmpfs over whole-disk FAT) at /");
                                                }
                                                Err(e) => {
                                                    panic!("FATAL: overlay root mount (whole disk) failed: {:?}", e);
                                                }
                                            }
                                        } else {
                                            debug_info!(
                                                "Filesystem type {:?} not supported for mounting",
                                                fs_type
                                            );
                                        }
                                    }
                                    Err(_) => {
                                        debug_info!("No filesystem detected on disk");
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            debug_warn!("Failed to read partition table: {}", e);
                        }
                    }
                } else {
                    debug_info!("No MBR signature found, checking whole disk");
                    // Try filesystem detection on whole disk anyway
                    match detect_filesystem(primary_master) {
                        Ok(fs_type) => {
                            debug_info!("Detected filesystem: {:?}", fs_type);
                            // Only mount if it's a supported FAT filesystem
                            use crate::fs::FilesystemType;
                            if matches!(
                                fs_type,
                                FilesystemType::Fat12
                                    | FilesystemType::Fat16
                                    | FilesystemType::Fat32
                            ) {
                                match mount_overlay_root(primary_master) {
                                    Ok(_) => {
                                        debug_info!("Mounted overlay(tmpfs over whole disk) at /");
                                    }
                                    Err(e) => {
                                        panic!(
                                            "FATAL: overlay root mount (no MBR) failed: {:?}",
                                            e
                                        );
                                    }
                                }
                            } else {
                                debug_info!(
                                    "Filesystem type {:?} not supported for mounting",
                                    fs_type
                                );
                            }
                        }
                        Err(_) => {
                            debug_info!("No filesystem detected");
                        }
                    }
                }
            }
            Err(e) => {
                debug_warn!("Failed to read boot sector: {}", e);
            }
        }
    } else {
        debug_info!("No IDE disk found on primary master");
    }

    debug_info!("Filesystem initialization complete");
}

/// Probe Primary IDE Slave and mount the first FAT partition there at `/host`.
///
/// This is the partition-table flow: vvfat synthesizes a real MBR with a single
/// FAT16 partition starting around LBA 63, so we read the boot sector, parse
/// the MBR, then `auto_mount` the FAT partition. Any failure (no drive, no MBR
/// signature, no FAT partition, mount error) logs and returns silently.
fn try_mount_host_disk() {
    use crate::drivers::block::BlockDevice;
    use crate::drivers::ide::{IdeBlockDevice, IdeChannel, IdeDrive, IDE_CONTROLLER};
    use crate::fs::vfs::auto_mount;
    use crate::fs::{detect_filesystem, read_partitions, FilesystemType, PartitionBlockDevice};

    let Some((model_bytes, sectors)) =
        IDE_CONTROLLER.get_disk_info(IdeChannel::Primary, IdeDrive::Slave)
    else {
        debug_info!("No IDE disk found on primary slave (host folder not mounted)");
        return;
    };

    let size_mb = (sectors * 512) / (1024 * 1024);
    let model_len = model_bytes.iter().position(|&c| c == 0).unwrap_or(40);
    let model = core::str::from_utf8(&model_bytes[..model_len])
        .unwrap_or("Unknown")
        .trim();
    debug_info!(
        "Found host IDE disk on primary slave: {} ({} MB)",
        model,
        size_mb
    );

    unsafe {
        PRIMARY_SLAVE_DISK = Some(IdeBlockDevice::new(IdeChannel::Primary, IdeDrive::Slave));
    }
    let host_disk = unsafe { (*&raw const PRIMARY_SLAVE_DISK).as_ref().unwrap() };

    let mut boot_sector = [0u8; 512];
    if let Err(e) = host_disk.read_blocks(0, 1, &mut boot_sector) {
        debug_warn!("Host disk: failed to read boot sector: {}", e);
        return;
    }

    if boot_sector[510] != 0x55 || boot_sector[511] != 0xAA {
        debug_info!("Host disk has no MBR signature; skipping host mount");
        return;
    }

    let partitions = match read_partitions(host_disk) {
        Ok(p) => p,
        Err(e) => {
            debug_warn!("Host disk: failed to read partition table: {}", e);
            return;
        }
    };

    for (i, partition) in partitions.iter().enumerate() {
        let Some(part) = partition else { continue };
        debug_info!(
            "Host partition {}: Type={:?}, Start={}, Size={} sectors",
            i + 1,
            part.partition_type,
            part.start_lba,
            part.size_sectors
        );

        unsafe {
            HOST_PARTITION_DEVICES[i] = Some(PartitionBlockDevice::new(host_disk, part));
        }
        let part_device = unsafe { HOST_PARTITION_DEVICES[i].as_ref().unwrap() };

        match detect_filesystem(part_device) {
            Ok(fs_type)
                if matches!(
                    fs_type,
                    FilesystemType::Fat12 | FilesystemType::Fat16 | FilesystemType::Fat32
                ) =>
            {
                debug_info!(
                    "Host partition {}: detected {:?}, mounting at /host",
                    i + 1,
                    fs_type
                );
                match auto_mount(part_device, "/host") {
                    Ok(_) => {
                        debug_info!("Host folder mounted at /host");
                        return;
                    }
                    Err(e) => {
                        debug_warn!("Failed to mount host partition {} at /host: {:?}", i + 1, e);
                        return;
                    }
                }
            }
            Ok(fs_type) => {
                debug_info!(
                    "Host partition {}: filesystem {:?} not supported",
                    i + 1,
                    fs_type
                );
            }
            Err(_) => {
                debug_info!("Host partition {}: filesystem detection failed", i + 1);
            }
        }
    }

    debug_info!("Host disk: no FAT partition found; /host not mounted");
}

/// Probe Secondary IDE Master and mount the whole-disk FAT32 image at
/// `/data`. Phase C U7 plumbing — this is the persistent writable
/// surface (Phase C U10 will flip it from read-only to writable once
/// the FAT writer lands).
///
/// The image is a bare FAT32 volume (no MBR), minted by `build.rs`
/// via `fatfs::format_volume`. Any failure (no drive, no BPB, mount
/// error) logs and returns silently — the kernel boots fine without
/// /data, just with no persistent writable mount.
fn try_mount_data_disk() {
    use crate::drivers::ide::{IdeBlockDevice, IdeChannel, IdeDrive, IDE_CONTROLLER};
    use crate::fs::filesystem::FilesystemError;
    use crate::fs::vfs::{auto_mount, auto_mount_writable};
    use crate::fs::{detect_filesystem, FilesystemType};

    let Some((model_bytes, sectors)) =
        IDE_CONTROLLER.get_disk_info(IdeChannel::Secondary, IdeDrive::Master)
    else {
        debug_info!("No IDE disk found on secondary master (no persistent /data)");
        return;
    };

    let size_mb = (sectors * 512) / (1024 * 1024);
    let model_len = model_bytes.iter().position(|&c| c == 0).unwrap_or(40);
    let model = core::str::from_utf8(&model_bytes[..model_len])
        .unwrap_or("Unknown")
        .trim();
    debug_info!(
        "Found data IDE disk on secondary master: {} ({} MB)",
        model,
        size_mb
    );

    unsafe {
        SECONDARY_MASTER_DISK = Some(IdeBlockDevice::new(IdeChannel::Secondary, IdeDrive::Master));
    }
    let data_disk = unsafe { (*&raw const SECONDARY_MASTER_DISK).as_ref().unwrap() };

    // Whole-disk FAT: no MBR, no partition table. Detect at sector 0.
    match detect_filesystem(data_disk) {
        Ok(fs_type)
            if matches!(
                fs_type,
                FilesystemType::Fat12 | FilesystemType::Fat16 | FilesystemType::Fat32
            ) =>
        {
            debug_info!(
                "Data disk: detected {:?}, mounting at /data WRITABLE",
                fs_type
            );
            // Try writable first; on dirty-bit refusal (C-2) fall back
            // to a read-only mount with a warning so userland still has
            // a /data mount to inspect.
            // TODO: plumb this through fw_cfg so production boots still
            // respect the dirty-bit gate. Forced to true for now so dev
            // workflow doesn't require `sync` before every Cmd-Q.
            match auto_mount_writable(data_disk, "/data", true) {
                Ok(_) => {
                    debug_info!("Data disk mounted writable at /data");
                }
                Err(FilesystemError::ReadOnly) => {
                    debug_warn!(
                        "Data disk: dirty-bit gate refused writable mount; falling back to read-only"
                    );
                    if let Err(e) = auto_mount(data_disk, "/data") {
                        debug_warn!("Read-only fallback also failed: {:?}", e);
                    }
                }
                Err(e) => {
                    debug_warn!("Failed to mount data disk at /data: {:?}", e);
                }
            }
        }
        Ok(fs_type) => {
            debug_info!("Data disk: filesystem {:?} not supported", fs_type);
        }
        Err(_) => {
            debug_info!("Data disk: filesystem detection failed (uninitialized?)");
        }
    }
}

/// Phase D U11: after /data is mounted, find the overlay mounted at
/// `/` and ask it to hydrate its upper-layer tmpfs from any prior
/// sync output. Silently no-ops when /data isn't writable (no
/// persistence target) or no prior state is present.
fn restore_overlay_upper_from_data() {
    use crate::fs::filesystem::Filesystem;
    use crate::fs::overlay::Overlay;
    use crate::fs::vfs::get_vfs;

    // Confirm /data is writable; otherwise restoring is moot since
    // future syncs won't be able to write either.
    if !get_vfs()
        .find_filesystem("/data")
        .map(|(fs, _)| !fs.is_read_only())
        .unwrap_or(false)
    {
        debug_info!("overlay restore: /data not writable; skipping persistence restore");
        return;
    }

    // Find the overlay at /.
    let root_mount = get_vfs()
        .list_mounts()
        .find(|m| m.path == "/" && m.filesystem.name() == "overlay");
    let Some(mount) = root_mount else {
        debug_info!("overlay restore: / is not an overlay; skipping");
        return;
    };

    // Narrow the trait object to a concrete Overlay reference. We
    // built this mount ourselves in vfs::mount_overlay_root, so the
    // name-based guard + downcast is sound.
    let overlay_ptr = mount.filesystem as *const dyn Filesystem as *const Overlay;
    let overlay: &Overlay = unsafe { &*overlay_ptr };
    let upper_dyn = overlay.upper();
    if upper_dyn.name() != "tmpfs" {
        debug_info!("overlay restore: upper isn't tmpfs; skipping");
        return;
    }
    let upper_ptr = upper_dyn as *const dyn Filesystem as *const crate::fs::tmpfs::Tmpfs;
    let upper: &crate::fs::tmpfs::Tmpfs = unsafe { &*upper_ptr };

    match crate::fs::overlay::sync::restore_upper_from_disk(upper) {
        Ok(n) => debug_info!(
            "overlay restore: hydrated upper with {} entries from /data",
            n
        ),
        Err(e) => debug_warn!("overlay restore: failed: {:?}", e),
    }
}

pub fn run() -> ! {
    debug_info!("Kernel initialization complete.");

    // Legacy GUI app launchers (painting, calc, explorer) are invoked via
    // `GLAUNCH.ELF`; notepad and the task manager are standalone ring-3 GUI
    // ELFs. File-utility commands are BusyBox applets. zsh drives the
    // synthetic /bin namespace.

    // Force an initial render to display the desktop
    window::render_frame();

    // Start the GUIShell background process (handles taskbar + start menu)
    debug_info!("Spawning GUIShell background process...");
    crate::commands::guishell::spawn_guishell_process();

    // U10: spawn the compositor kernel thread that owns input
    // processing + terminal output + rendering. Pre-U10 the kernel
    // main loop did this inline; with multi-ring-3 scheduling
    // (U5-U8) a busy ring-3 process would otherwise monopolize the
    // CPU and freeze the desktop. The compositor gets a regular
    // round-robin slice from the kernel-thread scheduler, so input
    // and rendering keep progressing alongside ring-3 work.
    debug_info!("Spawning compositor kernel thread...");
    crate::process::spawn_process(
        alloc::string::String::from("compositor"),
        None,
        crate::window::compositor::run,
    );

    // Main kernel loop (U10: pure scheduler housekeeping + idle).
    debug_info!("Entering idle loop...");

    loop {
        // === PREEMPTION HANDLING ===
        // Check if timer requested a context switch
        if interrupts::check_and_clear_preemption() {
            crate::process::handle_preemption();
        }

        // === WATCHDOG HANDLING ===
        // Check if timer detected a hung process that needs to be killed
        {
            use crate::arch::x86_64::preemption::WATCHDOG_KILL_PID;
            use crate::process::ProcessId;
            use core::sync::atomic::Ordering;

            let kill_pid_raw = WATCHDOG_KILL_PID.swap(0, Ordering::AcqRel);
            if kill_pid_raw != 0 {
                let pid = kill_pid_raw as ProcessId;

                // Get process name before killing it (for the alert dialog)
                let process_name = {
                    let sched = crate::process::scheduler::SCHEDULER.lock();
                    sched
                        .get_process(pid)
                        .map(|pcb| pcb.name.clone())
                        .unwrap_or_else(|| alloc::string::String::from("Unknown"))
                };

                debug_warn!(
                    "Watchdog: Terminating hung process {:?} '{}'",
                    pid,
                    process_name
                );

                // Terminate the process
                crate::process::terminate_process(pid);

                // Show alert dialog in a separate process (non-blocking)
                // Can't use blocking dialog from kernel loop because the kernel
                // loop IS the event processing loop - would deadlock
                let message = alloc::format!(
                    "Process '{}' was terminated because it stopped responding.",
                    process_name
                );
                crate::process::spawn_process(
                    alloc::string::String::from("watchdog-alert"),
                    None,
                    move || {
                        crate::window::dialogs::show_error("Process Terminated", &message);
                    },
                );
            }
        }

        crate::userland::lifecycle::process_due_real_timers();
        crate::userland::lifecycle::wake_ring3_due_sleepers();

        // === PROCESS SCHEDULING ===
        // Run any ready processes (GUIShell, spawned commands, etc.)
        // Processes that call sleep_ticks/sleep_until_event return here
        crate::process::try_run_scheduled_processes();

        // U8: dispatch any ring-3 process that's Ready. Saves the
        // kernel main loop's context into KERNEL_CONTEXT and diverges
        // into resume_ring3; control returns here when the ring-3
        // process yields back via yield_to_kernel_main_loop (which
        // switch_to_context's KERNEL_CONTEXT). Only fires when no
        // ring-3 process is currently loaded (the loaded one is
        // either running or running its kernel-side syscall handler).
        if crate::userland::lifecycle::current_user_pid().is_none() {
            if let Some(pid) = crate::userland::lifecycle::pop_next_ring3() {
                unsafe {
                    crate::userland::switch::save_kernel_and_resume_ring3(
                        pid,
                        &raw mut crate::arch::x86_64::preemption::KERNEL_CONTEXT,
                    );
                }
                // Resumed via KERNEL_CONTEXT restoration. Continue
                // the main-loop iteration below.
            }
        }

        // U10: input + terminal output + render moved to the
        // `compositor` kernel thread (spawned at boot). Main loop
        // is now pure scheduler housekeeping + idle.

        // === IDLE ===
        // Halt CPU until next interrupt (keyboard, mouse, timer).
        // Timer fires at 100 Hz, waking processes from sleep_queue.
        x86_64::instructions::hlt();
    }
}

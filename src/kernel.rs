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
    // The minimal crash core and CPU0 recorder are static and usable before
    // the heap, GS, mapper, or any production lock exists.
    crate::diagnostics::early_init();

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

    // Capture bootloader handoff values before subsystem initialization.
    let physical_memory_offset = boot_info
        .physical_memory_offset
        .into_option()
        .unwrap_or(0x10000000000);
    let rsdp_addr = boot_info.rsdp_addr.into_option();

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
    crate::diagnostics::percpu_init();
    // Program EFER.SCE + STAR/LSTAR/FMASK so user-mode `syscall` lands at
    // `syscall_fastpath_entry`. Must run after gdt::init() (so STAR sees
    // the right selectors) and after init_percpu() (so the first swapgs
    // hits a valid kernel GS).
    crate::arch::x86_64::syscall::init_syscall_msrs();
    // Load exception gates before initializing the demand-backed heap. The
    // first allocator metadata write faults in the first heap page.
    interrupts::init_idt();

    debug_info!("[boot] heap");
    unsafe {
        // Create a reference that will live for the entire program
        let memory_regions_ref: &'static _ = &*((&boot_info.memory_regions) as *const _);
        memory::init(memory_regions_ref, Some(physical_memory_offset));
        memory::init_heap(physical_memory_offset);
    }

    crate::arch::x86_64::acpi::init(rsdp_addr);
    // APIC MMIO mapping requires the live memory mapper and MADT topology.
    interrupts::init_interrupt_controllers();
    crate::time::init();
    ps2_controller::init();

    // Parse and install the system font. Must run after heap init (the TTF
    // rasterizer allocates) and before any window or TTY is constructed (the
    // grid layout reads the font's cell dimensions exactly once).
    debug_info!("[boot] fonts");
    crate::graphics::fonts::core_font::init_fonts();

    debug_info!("[boot] scheduler");
    crate::process::init_scheduler();
    crate::diagnostics::shadow_init();
    crate::process::timer::init();
    crate::process::timer::start_service();
    crate::arch::x86_64::smp::init();
    #[cfg(feature = "test")]
    crate::diagnostics::maybe_inject_crash();

    debug_info!("[boot] entropy");
    crate::random::init();

    // Linux ABI syscall surface (write/exit_group, plus the broader set in
    // U9) is dispatched directly from `userland::abi::syscall_dispatch` —
    // no per-syscall registration needed.

    debug_info!("[boot] virtio-blk+fs");
    crate::drivers::virtio::block::init();
    init_filesystems();
    // Host-disk probe is small (one MBR read on the slave drive) and the
    // filesystem tests assert /host is mounted, so we keep it in test mode.
    try_mount_host_disk();
    try_mount_data_disk();
    try_mount_legacy_data_disk();
    try_mount_shared();
    // Phase D U11: now that /data is mounted (if present), restore
    // the overlay's upper-layer tmpfs from any persistent state.
    // No-op when /data is missing or has no prior sync.
    restore_overlay_upper_from_data();

    // Load persistent system preferences before display initialization so the
    // window manager can resolve its initial theme from the saved request.
    crate::system_control::init();

    // Writable scratch directory on the overlay tmpfs. Idempotent:
    // a hydrated overlay state that already contains /work (or files
    // under it) must not fail boot. Ring-3 processes start with cwd
    // /host (read-only), so this is the conventional place for
    // compiler output and other build products.
    match crate::fs::vfs::vfs_mkdir("/work") {
        Ok(()) => {}
        Err(crate::fs::filesystem::FilesystemError::AlreadyExists) => {}
        Err(e) => debug_info!("[boot] /work provisioning failed: {:?}", e),
    }

    // DEFAULT_USER_ENV advertises HOME=/root. Keep it writable on the
    // overlay so normal userland applications (including Links) can create
    // their own dot-directories without baking mutable state into /etc.
    match crate::fs::vfs::vfs_mkdir("/root") {
        Ok(()) => {}
        Err(crate::fs::filesystem::FilesystemError::AlreadyExists) => {}
        Err(e) => debug_info!("[boot] /root provisioning failed: {:?}", e),
    }

    // POSIX temp-file directory. GCC's driver (and anything else relying
    // on mkstemp/choose_tmpdir defaults) writes scratch files here with
    // no TMPDIR convention. Overlay-backed like /work.
    match crate::fs::vfs::vfs_mkdir("/tmp") {
        Ok(()) => {}
        Err(crate::fs::filesystem::FilesystemError::AlreadyExists) => {}
        Err(e) => debug_info!("[boot] /tmp provisioning failed: {:?}", e),
    }

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

    // Publish the resolved frame/control theme for ring-3 GUI apps. Must run
    // after display + window-manager init (which finalizes the Classic/Aero
    // selection, including renderer fallbacks) and after managed-/etc init;
    // both hold at this point, and no ring-3 process exists yet.
    crate::userland::etc::publish_theme(crate::window::theme::active());

    // Mouse, GUIShell desktop, and host bridges are not exercised by any
    // in-kernel test. Skipping them under `feature = "test"` removes the
    // largest chunks of `./test.sh` startup latency: GUIShell paints the
    // wallpaper + initial render; the MCP bridge spawns a kernel process,
    // while the two host bridges bring up COM2 and COM3.
    #[cfg(not(feature = "test"))]
    {
        debug_info!("[boot] mouse");
        if let Some((width, height)) = screen_dims {
            crate::drivers::mouse::init_with_screen(width, height);
        } else {
            crate::drivers::mouse::init();
        }

        if ring3_desktop_shell_requested() {
            debug_info!("[boot] desktop root (ring-3 shell mode)");
            crate::commands::guishell::init_desktop_root_only();
            window::render_frame();
        } else {
            debug_info!("[boot] guishell");
            init_guishell_desktop();
        }

        debug_info!("[boot] mcp-bridge");
        init_mcp_bridge();

        debug_info!("[boot] host-clipboard");
        crate::clipboard::init();
    }

    #[cfg(feature = "test")]
    {
        let _ = screen_dims;
        debug_info!("[boot] test mode — skipping mouse, guishell, host bridges");
    }

    debug_info!("[boot] init complete");
    crate::arch::x86_64::smp::release_aps();
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

// Static storage for VirtIO block devices and partition devices. The BSP
// populates these slots before AP dispatch is enabled; afterwards their
// structure is immutable and runtime device access is serialized by the
// driver/filesystem locks.
static mut ROOT_DISK: Option<crate::drivers::virtio::block::VirtioBlockDevice> = None;
static mut PARTITION_DEVICES: [Option<crate::fs::PartitionBlockDevice<'static>>; 4] =
    [None, None, None, None];

// Static storage for the serial-identified host-share disk (vvfat-backed when
// the user runs ./build.sh; absent otherwise). Kept in a separate slot/array
// so the host disk's partitions don't alias the root disk's PARTITION_DEVICES.
static mut HOST_DISK: Option<crate::drivers::virtio::block::VirtioBlockDevice> = None;
static mut HOST_PARTITION_DEVICES: [Option<crate::fs::PartitionBlockDevice<'static>>; 4] =
    [None, None, None, None];

/// The `agenticos-data` device is the writable whole-disk /data filesystem.
static mut DATA_DISK: Option<crate::drivers::virtio::block::VirtioBlockDevice> = None;
/// Optional previous FAT data image, attached read-only for migration.
static mut LEGACY_DATA_DISK: Option<crate::drivers::virtio::block::VirtioBlockDevice> = None;

fn init_filesystems() {
    use crate::drivers::block::BlockDevice;
    use crate::drivers::virtio::block::VirtioBlockDevice;
    use crate::fs::vfs::mount_overlay_root;
    use crate::fs::{detect_filesystem, read_partitions, PartitionBlockDevice};

    debug_info!("Detecting and mounting filesystems...");

    if let Some(root) =
        VirtioBlockDevice::by_id("agenticos-root").or_else(|| VirtioBlockDevice::by_index(0))
    {
        let sectors = root.total_blocks();
        let size_mb = (sectors * 512) / (1024 * 1024);
        debug_info!("Found VirtIO root disk: {} ({} MB)", root.name(), size_mb);

        // Create block device for the disk and store it statically
        unsafe {
            ROOT_DISK = Some(root);
        }
        let primary_master = unsafe { (*&raw const ROOT_DISK).as_ref().unwrap() };

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
        debug_info!("No VirtIO root disk found");
    }

    debug_info!("Filesystem initialization complete");
}

/// Probe the `agenticos-host` VirtIO disk and mount its first FAT partition at `/host`.
///
/// This is the partition-table flow: vvfat synthesizes a real MBR with a single
/// FAT16 partition starting around LBA 63, so we read the boot sector, parse
/// the MBR, then `auto_mount` the FAT partition. Any failure (no drive, no MBR
/// signature, no FAT partition, mount error) logs and returns silently.
fn try_mount_host_disk() {
    use crate::drivers::block::BlockDevice;
    use crate::drivers::virtio::block::VirtioBlockDevice;
    use crate::fs::vfs::auto_mount;
    use crate::fs::{detect_filesystem, read_partitions, FilesystemType, PartitionBlockDevice};

    let Some(host) =
        VirtioBlockDevice::by_id("agenticos-host").or_else(|| VirtioBlockDevice::by_index(1))
    else {
        debug_info!("No VirtIO host disk found (host folder not mounted)");
        return;
    };
    debug_info!(
        "Found VirtIO host disk: {} ({} MB)",
        host.name(),
        host.capacity() / 1024 / 1024
    );

    unsafe {
        HOST_DISK = Some(host);
    }
    let host_disk = unsafe { (*&raw const HOST_DISK).as_ref().unwrap() };

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

fn force_dirty_mount_requested() -> bool {
    let mut value = [0u8; 8];
    let Some(length) =
        crate::drivers::fw_cfg::read_file("opt/agenticos/force_dirty_mount", &mut value)
    else {
        return false;
    };
    matches!(core::str::from_utf8(&value[..length]), Ok("1"))
}

/// Whether the desktop shell should run as the ring-3 `DESKTOP.ELF` process
/// rather than the in-kernel `guishell`. The ring-3 shell is now the default;
/// only an explicit `AGENTICOS_SHELL=ring0` (fw_cfg `opt/agenticos/shell`)
/// selects the legacy in-kernel shell.
fn ring3_desktop_shell_requested() -> bool {
    let mut value = [0u8; 8];
    let Some(length) = crate::drivers::fw_cfg::read_file("opt/agenticos/shell", &mut value) else {
        return true;
    };
    !matches!(core::str::from_utf8(&value[..length]), Ok("ring0"))
}

/// Probe the `agenticos-data` VirtIO device and mount its whole-disk ext2/FAT image at
/// `/data`. Failures are non-fatal so the kernel can still boot without a
/// persistent disk.
fn try_mount_data_disk() {
    use crate::drivers::block::BlockDevice;
    use crate::drivers::virtio::block::VirtioBlockDevice;
    use crate::fs::filesystem::FilesystemError;
    use crate::fs::vfs::{auto_mount, auto_mount_writable};
    use crate::fs::{detect_filesystem, FilesystemType};

    let Some(data) =
        VirtioBlockDevice::by_id("agenticos-data").or_else(|| VirtioBlockDevice::by_index(2))
    else {
        debug_info!("No VirtIO data disk found (no persistent /data)");
        return;
    };
    debug_info!(
        "Found VirtIO data disk: {} ({} MB)",
        data.name(),
        data.capacity() / 1024 / 1024
    );

    unsafe {
        DATA_DISK = Some(data);
    }
    let data_disk = unsafe { (*&raw const DATA_DISK).as_ref().unwrap() };

    // Whole-disk filesystem: no MBR or partition table.
    match detect_filesystem(data_disk) {
        Ok(fs_type)
            if matches!(
                fs_type,
                FilesystemType::Fat12
                    | FilesystemType::Fat16
                    | FilesystemType::Fat32
                    | FilesystemType::Ext2
            ) =>
        {
            debug_info!(
                "Data disk: detected {:?}, mounting at /data WRITABLE",
                fs_type
            );
            // Try writable first; on dirty-bit refusal (C-2) fall back
            // to a read-only mount with a warning so userland still has
            // a /data mount to inspect.
            let force_dirty = force_dirty_mount_requested();
            if force_dirty {
                debug_warn!("Data disk: forced dirty writable mount override is active");
            }
            match auto_mount_writable(data_disk, "/data", force_dirty) {
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

/// Probe the `agenticos-shared` virtio-9p device and mount the host share at
/// `/shared`. Absence is normal (share disabled, or QEMU without the
/// device); every failure path leaves boot identical to today.
fn try_mount_shared() {
    use crate::drivers::virtio::p9::P9Transport;

    let Some(transport) = P9Transport::discover_by_tag("agenticos-shared") else {
        debug_info!("shared: no virtio-9p device; /shared not mounted");
        return;
    };
    match crate::fs::p9::P9Filesystem::new(transport) {
        Ok(filesystem) => match crate::fs::vfs::mount_p9(filesystem, "/shared") {
            Ok(()) => debug_info!("Mounted 9p host share at /shared"),
            Err(error) => debug_warn!("shared: mount failed: {:?}", error),
        },
        Err(error) => debug_warn!("shared: 9p handshake failed: {:?}", error),
    }
}

/// Mount an explicitly supplied legacy FAT VirtIO image at
/// `/legacy-data`. `auto_mount` always creates a read-only FAT wrapper, making
/// this a safe migration source for `cp -a /legacy-data/. /data/`.
fn try_mount_legacy_data_disk() {
    use crate::drivers::block::BlockDevice;
    use crate::drivers::virtio::block::VirtioBlockDevice;
    use crate::fs::vfs::auto_mount;
    use crate::fs::{detect_filesystem, FilesystemType};

    let Some(legacy) =
        VirtioBlockDevice::by_id("agenticos-legacy").or_else(|| VirtioBlockDevice::by_index(3))
    else {
        return;
    };
    let sectors = legacy.total_blocks();
    unsafe {
        LEGACY_DATA_DISK = Some(legacy);
    }
    let disk = unsafe { (*&raw const LEGACY_DATA_DISK).as_ref().unwrap() };
    match detect_filesystem(disk) {
        Ok(kind)
            if matches!(
                kind,
                FilesystemType::Fat12 | FilesystemType::Fat16 | FilesystemType::Fat32
            ) =>
        {
            match auto_mount(disk, "/legacy-data") {
                Ok(_) => debug_info!(
                    "Mounted legacy FAT data image read-only at /legacy-data ({} MB)",
                    (sectors * 512) / (1024 * 1024)
                ),
                Err(error) => debug_warn!("Failed to mount /legacy-data: {:?}", error),
            }
        }
        Ok(kind) => debug_warn!("Legacy data disk is {:?}, expected FAT", kind),
        Err(error) => debug_warn!("Legacy data disk detection failed: {:?}", error),
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
    let overlay_ptr = {
        let vfs = get_vfs();
        let result = vfs
            .list_mounts()
            .find(|m| m.path == "/" && m.filesystem.name() == "overlay")
            .map(|mount| mount.filesystem as *const dyn Filesystem as *const Overlay);
        result
    };
    let Some(overlay_ptr) = overlay_ptr else {
        debug_info!("overlay restore: / is not an overlay; skipping");
        return;
    };

    // Narrow the trait object to a concrete Overlay reference. We
    // built this mount ourselves in vfs::mount_overlay_root, so the
    // name-based guard + downcast is sound.
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

    // Every GUI app is a standalone ring-3 ELF now (File Manager, Calc,
    // Notepad, Painting, GL Arena, Task Manager); the `GLAUNCH.ELF`
    // launcher list is empty. File-utility commands are BusyBox applets.
    // zsh drives the synthetic /bin namespace.

    // Force an initial render to display the desktop
    window::render_frame();

    // One persistent worker owns kernel-requested ring-3 launch setup and
    // detached-process teardown. Start it before GUIShell can submit actions.
    debug_info!("Spawning user process service...");
    crate::userland::process_service::start();

    // Start the desktop shell. The default in-kernel GUIShell owns the taskbar
    // and Start menu directly; AGENTICOS_SHELL=ring3 instead launches the
    // ring-3 DESKTOP.ELF, which claims the shell role and drives the taskbar
    // through the desktop-shell protocol syscalls.
    if ring3_desktop_shell_requested() {
        debug_info!("Launching ring-3 desktop shell (DESKTOP.ELF)...");
        crate::commands::guishell::spawn_ring3_desktop_shell();
    } else {
        debug_info!("Spawning GUIShell background process...");
        crate::commands::guishell::spawn_guishell_process();
    }

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

        let _ = crate::process::drain_kernel_io_wakes();
        // Backstop for signal wakes dropped by `wake_ring3_for_signal`'s
        // `try_lock` under SMP PROCESS_TABLE contention (e.g. a SIGCHLD reaper
        // or `kill` target parked in an interruptible syscall). Runs here with
        // interrupts enabled and no locks held, so its blocking scheduler lock
        // is safe; the timer ISR path is deliberately not used for it.
        let _ = crate::userland::lifecycle::retry_dropped_signal_wakes();

        // Dispatch one kernel-thread or user-process entity from the single
        // fair queue. Direct cross-privilege switches keep normal execution
        // out of this main-loop fallback after the first dispatch.
        crate::process::try_run_scheduled_processes();

        // U10: input + terminal output + render moved to the
        // `compositor` kernel thread (spawned at boot). Main loop
        // is now pure scheduler housekeeping + idle.

        // === IDLE ===
        // Close the completion-vs-halt race: with interrupts disabled, check
        // once more for a block wake and any unified-scheduler work. If none
        // exists, STI+HLT atomically waits for the PCI completion (or another
        // interrupt) without imposing a 10 ms PIT tick on every request.
        x86_64::instructions::interrupts::disable();
        let io_woke = crate::process::drain_kernel_io_wakes();
        // Recover any signal wake dropped since the housekeeping pass above,
        // before committing to STI+HLT — otherwise a parked signal-pending
        // process could sleep until the next unrelated interrupt.
        let signal_woke = crate::userland::lifecycle::retry_dropped_signal_wakes();
        let scheduler_ready = crate::process::scheduler::SCHEDULER
            .try_lock()
            .map(|scheduler| scheduler.ready_entity_count() != 0)
            .unwrap_or(true);
        if io_woke || signal_woke || scheduler_ready {
            x86_64::instructions::interrupts::enable();
            continue;
        }

        // Publish the idle state before the final queue recheck. A remote
        // enqueue either observes this flag and sends an IPI, or is observed
        // by the recheck before STI+HLT. The timer accounting path also uses
        // the flag to distinguish real idle time from kernel housekeeping.
        crate::arch::x86_64::percpu::set_idle_interruptible(true);
        if crate::process::scheduler::SCHEDULER
            .lock()
            .ready_entity_count()
            != 0
        {
            crate::arch::x86_64::percpu::set_idle_interruptible(false);
            x86_64::instructions::interrupts::enable();
            continue;
        }
        x86_64::instructions::interrupts::enable_and_hlt();
        crate::arch::x86_64::percpu::set_idle_interruptible(false);
    }
}

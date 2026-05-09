use bootloader_api::BootInfo;
use crate::lib::debug::{self, DebugLevel};
use crate::{debug_info, debug_debug, debug_warn};
use crate::arch::x86_64::{gdt, interrupts};
use crate::mm::memory;
use crate::drivers::display::display;
use crate::drivers::ps2_controller;
use crate::window;
use alloc::boxed::Box;

pub fn init(boot_info: &'static mut BootInfo) {
    // Initialize debug subsystem
    debug::init();
    debug::set_debug_level(DebugLevel::Debug);

    debug_info!("=== AgenticOS Kernel Starting ===");
    debug_debug!("Boot info address: {:p}", boot_info);

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
    let physical_memory_offset = boot_info.physical_memory_offset.into_option()
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
    crate::process::spawn_process(
        String::from("rpc-dispatcher"),
        None,
        || crate::tools::rpc::run_dispatcher(),
    );

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
static mut PARTITION_DEVICES: [Option<crate::fs::PartitionBlockDevice<'static>>; 4] = [None, None, None, None];

// Static storage for the host-share disk on Primary Slave (vvfat-backed when
// the user runs ./build.sh; absent otherwise). Kept in a separate slot/array
// so the host disk's partitions don't alias the root disk's PARTITION_DEVICES.
static mut PRIMARY_SLAVE_DISK: Option<crate::drivers::ide::IdeBlockDevice> = None;
static mut HOST_PARTITION_DEVICES: [Option<crate::fs::PartitionBlockDevice<'static>>; 4] = [None, None, None, None];

fn init_filesystems() {
    use crate::drivers::ide::{IDE_CONTROLLER, IdeChannel, IdeDrive, IdeBlockDevice};
    use crate::drivers::block::BlockDevice;
    use crate::fs::{detect_filesystem, read_partitions, PartitionBlockDevice};
    use crate::fs::vfs::auto_mount;
    
    debug_info!("Detecting and mounting filesystems...");
    
    // Check primary master disk
    if let Some((model_bytes, sectors)) = IDE_CONTROLLER.get_disk_info(IdeChannel::Primary, IdeDrive::Master) {
        let size_mb = (sectors * 512) / (1024 * 1024);
        
        // Convert model bytes to string
        let model_len = model_bytes.iter().position(|&c| c == 0).unwrap_or(40);
        let model = core::str::from_utf8(&model_bytes[..model_len]).unwrap_or("Unknown").trim();
        
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
                                    debug_info!("Partition {}: Type={:?}, Start={}, Size={} sectors", 
                                        i + 1, part.partition_type, part.start_lba, part.size_sectors);
                                    
                                    // Create a partition device and store it statically
                                    unsafe {
                                        PARTITION_DEVICES[i] = Some(PartitionBlockDevice::new(primary_master, part));
                                    }
                                    
                                    // Get a reference to the stored partition device
                                    let part_device = unsafe { PARTITION_DEVICES[i].as_ref().unwrap() };
                                    
                                    match detect_filesystem(part_device) {
                                        Ok(fs_type) => {
                                            debug_info!("  Detected filesystem: {:?}", fs_type);
                                            // Only consider FAT filesystems as valid for mounting
                                            use crate::fs::FilesystemType;
                                            if first_valid_partition.is_none() && 
                                               matches!(fs_type, FilesystemType::Fat12 | FilesystemType::Fat16 | FilesystemType::Fat32) {
                                                first_valid_partition = Some(i);
                                            }
                                        }
                                        Err(_) => {
                                            debug_info!("  Unknown filesystem on partition {}", i + 1);
                                        }
                                    }
                                }
                            }
                            
                            // Mount the first valid partition as root
                            if let Some(part_idx) = first_valid_partition {
                                let part_device = unsafe { PARTITION_DEVICES[part_idx].as_ref().unwrap() };
                                match auto_mount(part_device, "/") {
                                    Ok(_) => {
                                        debug_info!("Mounted partition {} as root filesystem", part_idx + 1);
                                    }
                                    Err(e) => {
                                        debug_warn!("Failed to mount partition {}: {:?}", part_idx + 1, e);
                                    }
                                }
                            }
                            
                            if partition_num == 0 {
                                debug_info!("No partitions found, checking whole disk for filesystem");
                                // Try to detect filesystem on whole disk
                                match detect_filesystem(primary_master) {
                                    Ok(fs_type) => {
                                        debug_info!("Detected filesystem on whole disk: {:?}", fs_type);
                                        // Only mount if it's a supported FAT filesystem
                                        use crate::fs::FilesystemType;
                                        if matches!(fs_type, FilesystemType::Fat12 | FilesystemType::Fat16 | FilesystemType::Fat32) {
                                            match auto_mount(primary_master, "/") {
                                                Ok(_) => {
                                                    debug_info!("Mounted whole disk as root filesystem");
                                                }
                                                Err(e) => {
                                                    debug_warn!("Failed to mount disk: {:?}", e);
                                                }
                                            }
                                        } else {
                                            debug_info!("Filesystem type {:?} not supported for mounting", fs_type);
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
                            if matches!(fs_type, FilesystemType::Fat12 | FilesystemType::Fat16 | FilesystemType::Fat32) {
                                match auto_mount(primary_master, "/") {
                                    Ok(_) => {
                                        debug_info!("Mounted disk as root filesystem");
                                    }
                                    Err(e) => {
                                        debug_warn!("Failed to mount: {:?}", e);
                                    }
                                }
                            } else {
                                debug_info!("Filesystem type {:?} not supported for mounting", fs_type);
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
    use crate::drivers::ide::{IDE_CONTROLLER, IdeChannel, IdeDrive, IdeBlockDevice};
    use crate::drivers::block::BlockDevice;
    use crate::fs::{detect_filesystem, read_partitions, PartitionBlockDevice, FilesystemType};
    use crate::fs::vfs::auto_mount;

    let Some((model_bytes, sectors)) = IDE_CONTROLLER.get_disk_info(IdeChannel::Primary, IdeDrive::Slave) else {
        debug_info!("No IDE disk found on primary slave (host folder not mounted)");
        return;
    };

    let size_mb = (sectors * 512) / (1024 * 1024);
    let model_len = model_bytes.iter().position(|&c| c == 0).unwrap_or(40);
    let model = core::str::from_utf8(&model_bytes[..model_len]).unwrap_or("Unknown").trim();
    debug_info!("Found host IDE disk on primary slave: {} ({} MB)", model, size_mb);

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
            i + 1, part.partition_type, part.start_lba, part.size_sectors
        );

        unsafe {
            HOST_PARTITION_DEVICES[i] = Some(PartitionBlockDevice::new(host_disk, part));
        }
        let part_device = unsafe { HOST_PARTITION_DEVICES[i].as_ref().unwrap() };

        match detect_filesystem(part_device) {
            Ok(fs_type) if matches!(fs_type, FilesystemType::Fat12 | FilesystemType::Fat16 | FilesystemType::Fat32) => {
                debug_info!("Host partition {}: detected {:?}, mounting at /host", i + 1, fs_type);
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
                debug_info!("Host partition {}: filesystem {:?} not supported", i + 1, fs_type);
            }
            Err(_) => {
                debug_info!("Host partition {}: filesystem detection failed", i + 1);
            }
        }
    }

    debug_info!("Host disk: no FAT partition found; /host not mounted");
}


pub fn run() -> ! {
    debug_info!("Kernel initialization complete.");

    // Register available commands with the process manager
    debug_info!("Registering commands...");
    crate::process::register_command("dir", crate::commands::dir::create_dir_process);
    crate::process::register_command("cat", crate::commands::cat::create_cat_process);
    crate::process::register_command("echo", crate::commands::echo::create_echo_process);
    crate::process::register_command("head", crate::commands::head::create_head_process);
    crate::process::register_command("tail", crate::commands::tail::create_tail_process);
    crate::process::register_command("wc", crate::commands::wc::create_wc_process);
    crate::process::register_command("touch", crate::commands::touch::create_touch_process);
    crate::process::register_command("hexdump", crate::commands::hexdump::create_hexdump_process);
    crate::process::register_command("time", crate::commands::time::create_time_process);
    crate::process::register_command("grep", crate::commands::grep::create_grep_process);
    crate::process::register_command("pwd", crate::commands::pwd::create_pwd_process);
    crate::process::register_command("ls", crate::commands::ls::create_ls_process);
    crate::process::register_command("painting", crate::commands::painting::create_painting_process);
    crate::process::register_command("calc", crate::commands::calc::create_calc_process);
    crate::process::register_command("notepad", crate::commands::notepad::create_notepad_process);
    crate::process::register_command("tasks", crate::commands::tasks::create_tasks_process);
    crate::process::register_command("run", crate::commands::run::create_run_process);
    debug_info!("All {} commands registered successfully.", 17);

    // Force an initial render to display the desktop
    window::render_frame();

    // Start the GUIShell background process (handles taskbar + start menu)
    debug_info!("Spawning GUIShell background process...");
    crate::commands::guishell::spawn_guishell_process();

    // Create input processor for event handling
    // This processes raw scancodes/mouse bytes into typed events
    let mut input_processor = crate::input::InputProcessor::new(1280, 720);
    debug_info!("Input processor initialized");

    // Main kernel loop
    debug_info!("Entering idle loop with window rendering...");
    let using_virtio = crate::drivers::mouse::is_virtio_tablet();
    if using_virtio {
        debug_info!("VirtIO tablet active - mouse will not grab QEMU window");
    }

    loop {
        // === PREEMPTION HANDLING ===
        // Check if timer requested a context switch
        if interrupts::check_and_clear_preemption() {
            crate::process::handle_preemption();
        }

        // === WATCHDOG HANDLING ===
        // Check if timer detected a hung process that needs to be killed
        {
            use core::sync::atomic::Ordering;
            use crate::arch::x86_64::preemption::WATCHDOG_KILL_PID;
            use crate::process::ProcessId;

            let kill_pid_raw = WATCHDOG_KILL_PID.swap(0, Ordering::AcqRel);
            if kill_pid_raw != 0 {
                let pid = kill_pid_raw as ProcessId;

                // Get process name before killing it (for the alert dialog)
                let process_name = {
                    let sched = crate::process::scheduler::SCHEDULER.lock();
                    sched.get_process(pid)
                        .map(|pcb| pcb.name.clone())
                        .unwrap_or_else(|| alloc::string::String::from("Unknown"))
                };

                debug_warn!("Watchdog: Terminating hung process {:?} '{}'", pid, process_name);

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

        // === PROCESS SCHEDULING ===
        // Run any ready processes (GUIShell, spawned commands, etc.)
        // Processes that call sleep_ticks/sleep_until_event return here
        crate::process::try_run_scheduled_processes();

        // === INPUT PROCESSING ===
        // VirtIO tablet (seamless mouse in QEMU) requires polling
        if using_virtio {
            crate::drivers::mouse::poll();
            if let Some(event) = input_processor.check_virtio_tablet() {
                window::process_event(event);  // Signals processes on input
            }
        }

        // Process keyboard/mouse events from interrupt-driven queue
        // Each event signals relevant sleeping processes
        for event in input_processor.process_pending(&crate::input::INPUT_QUEUE) {
            window::process_event(event);
        }

        // === SHELL PROCESSING ===
        // Poll shells that have pending work (cooperative multitasking)
        // Only processes shells with has_pending_work() == true
        let exited_terminals = crate::commands::shell::shell_process::poll_all_shells();
        for terminal_id in exited_terminals {
            crate::window::terminal_factory::close_terminal(terminal_id);
        }

        // === RENDERING ===
        // Process pending terminal output (early exit if none)
        window::process_terminal_output();
        // Render frame (early exit if compositor has no dirty regions)
        window::render_frame();

        // === IDLE ===
        // Halt CPU until next interrupt (keyboard, mouse, timer)
        // Timer fires at 100Hz, waking processes from sleep_queue
        x86_64::instructions::hlt();
    }
}


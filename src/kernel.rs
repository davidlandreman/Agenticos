use bootloader_api::BootInfo;
use crate::lib::debug::{self, DebugLevel};
use crate::{debug_info, debug_debug, debug_warn};
use crate::arch::x86_64::interrupts;
use crate::mm::memory;
use crate::drivers::display::{display, text_buffer, double_buffered_text};
use crate::drivers::ps2_controller;
use crate::process::{Process, ShellProcess};

pub fn init(boot_info: &'static mut BootInfo) {
    // Initialize debug subsystem
    debug::init();
    debug::set_debug_level(DebugLevel::Debug);
    
    debug_info!("=== AgenticOS Kernel Starting ===");
    debug_info!("Kernel entry point reached successfully!");
    debug_debug!("Boot info address: {:p}", boot_info);

    // Initialize interrupt descriptor table
    interrupts::init_idt();
    
    // Initialize PS/2 controller configuration for keyboard
    ps2_controller::init();
    
    // Extract what we need from boot_info before borrowing it
    // Use a default offset if not provided by bootloader
    let physical_memory_offset = boot_info.physical_memory_offset.into_option()
        .unwrap_or(0x10000000000); // Default offset for identity mapping
    let rsdp_addr = boot_info.rsdp_addr.into_option();
    
    // Initialize memory and heap (this will borrow memory_regions for 'static)
    unsafe {
        // Create a reference that will live for the entire program
        let memory_regions_ref: &'static _ = &*((&boot_info.memory_regions) as *const _);
        memory::init(memory_regions_ref, Some(physical_memory_offset));
        memory::init_heap(physical_memory_offset);
    }
    debug_info!("Heap initialized successfully!");
    
    // Initialize IDE controller and detect drives
    debug_info!("Initializing IDE controller...");
    crate::drivers::ide::IDE_CONTROLLER.initialize();
    
    // Initialize disk drives and mount filesystems
    init_filesystems();
    
    // Print memory information
    memory::print_memory_info();
    
    // Print boot information
    if let Some(rsdp_addr) = rsdp_addr {
        debug_debug!("RSDP address: 0x{:016x}", rsdp_addr);
    }
    
    // Initialize display - this can still mutably borrow boot_info
    init_display(boot_info);
    
    // Initialize mouse driver
    crate::drivers::mouse::init();
    
}

fn init_display(boot_info: &'static mut BootInfo) {
    debug_info!("Checking for framebuffer...");
    
    if let Some(framebuffer) = boot_info.framebuffer.as_mut() {
        if display::USE_DOUBLE_BUFFER {
            debug_info!("Framebuffer found! Initializing double buffered text...");
            double_buffered_text::init(framebuffer);
            debug_info!("Double buffered text initialized successfully!");
        } else {
            debug_info!("Framebuffer found! Initializing text buffer...");
            text_buffer::init(framebuffer);
            debug_info!("Text buffer initialized successfully!");
        }
        
    } else {
        debug_warn!("No framebuffer available from bootloader");
    }
}

// Static storage for IDE block devices and partition devices
static mut PRIMARY_MASTER_DISK: Option<crate::drivers::ide::IdeBlockDevice> = None;
static mut PARTITION_DEVICES: [Option<crate::fs::PartitionBlockDevice<'static>>; 4] = [None, None, None, None];

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
        
        let primary_master = unsafe { PRIMARY_MASTER_DISK.as_ref().unwrap() };
        
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
    
    // TODO: Check other IDE channels if needed (primary slave, secondary master/slave)
    
    debug_info!("Filesystem initialization complete");
}


pub fn run() -> ! {
    debug_info!("Kernel initialization complete.");

    // Run shell process
    let mut shell_process = ShellProcess::new();
    debug_info!("Running shell process (PID: {})", shell_process.get_id());
    shell_process.run();

    debug_info!("Entering idle loop with mouse cursor...");
    
    // Main kernel loop
    loop {
        // Process any pending keyboard input (outside of interrupt context)
        crate::drivers::keyboard::process_pending_input();
        
        // Draw mouse cursor if double buffering is enabled
        if display::USE_DOUBLE_BUFFER {
            double_buffered_text::with_buffer(|buffer| {
                // Draw the mouse cursor
                crate::graphics::mouse_cursor::draw_mouse_cursor(buffer);
                // Swap buffers to show the cursor
                buffer.swap_buffers();
            });
        }
        
        x86_64::instructions::hlt();
    }
}


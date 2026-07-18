# AgenticOS Implementation Plan

## Overview
AgenticOS is a Rust-based operating system targeting Intel x86-64 architecture. This phased plan follows the proven path from the "Writing an OS in Rust" tutorial while establishing a foundation for future agent-based computing capabilities.

## Progress Summary

### ✅ Completed Phases

#### Phase 1: Bare Metal Foundation ✓
- **Status**: Complete
- Configured `no_std` environment with panic handler
- Established kernel entry point with bootloader integration
- QEMU testing environment fully operational
- Build system with `build.sh` script for easy development

#### Phase 2: Basic I/O System ✓
- **Status**: Complete (Exceeded original scope)
- **VGA Alternative**: Implemented modern framebuffer support instead of VGA
  - Single and double buffering modes
  - Full RGB color support
  - TrueType font rendering (8bpp AA via `ttf-parser` + `ab_glyph_rasterizer`); embedded 8x8 bitmap kept as parse-failure fallback
- **Serial Port**: Debug logging system via QEMU serial
  - Hierarchical log levels (Error, Warn, Info, Debug, Trace)
  - Macro-based interface with compile-time filtering
  - Runtime level configuration

#### Phase 3: Testing Infrastructure ✓
- **Status**: Partially Complete
- Custom test framework for `no_std` environment
- QEMU-based testing functional
- Unit tests for core components
- **TODO**: Expand test coverage, add CI/CD pipeline

#### Phase 4: Interrupt Handling ✓
- **Status**: Complete
- **4.1 CPU Exception Handling**: ✓ IDT setup with basic handlers
- **4.2 Double Fault Protection**: ✓ Handler implemented
- **4.3 Hardware Interrupts**: ✓ Timer, keyboard, and mouse interrupts working
  - PIC 8259 initialized and configured
  - IRQ1 (keyboard) and IRQ12 (mouse) handlers implemented
  - Timer interrupt (IRQ0) for system tick

#### Phase 5: Memory Management ✓
- **Status**: Complete
- **5.1 Physical Memory Management**: ✓ 
  - Memory map parsing from bootloader
  - Memory region tracking and statistics
  - Frame allocator implementation (`BootInfoFrameAllocator`)
- **5.2 Paging**: ✓ Complete
  - Virtual memory with `OffsetPageTable`
  - Page fault handler with demand paging
  - Memory mapper for address translation
- **5.3 Heap Allocation**: ✓ Complete
  - 100 MiB heap at virtual address `0x_4444_4444_0000`
  - `linked_list_allocator` backend
  - Global allocator enables `Vec`, `String`, etc.
  - Comprehensive heap tests

#### Process Foundation ✓
- **Status**: Initial Implementation Complete
- Basic process abstraction layer created
- Process trait with `get_id()`, `get_name()`, `run()` methods
- Simple PID allocation (sequential from 1)
- Shell process runs as PID 1 during initialization

#### Input Device Support ✓
- **Status**: Complete
- **VirtIO Tablet** (Primary):
  - Seamless mouse input in QEMU (no cursor grabbing)
  - PCI bus enumeration for device discovery
  - Absolute positioning with screen coordinate scaling
  - Requires `-device virtio-tablet-pci` QEMU flag
- **PS/2 Controller** (Fallback): Shared initialization for keyboard and mouse
- **Keyboard Driver**:
  - Full PS/2 keyboard support with scancode set 2
  - Circular buffer for scancode queuing
  - Make/break code processing
- **Mouse Driver**:
  - Complete PS/2 mouse support with 3-byte packet processing
  - Position tracking with screen boundary clamping
  - Three-button support (left, right, middle)
  - Hardware cursor rendering with double buffer integration
  - Classic arrow cursor with background save/restore
- **Input Processing Pipeline**:
  - Lock-free SPSC queue (256 entries) for interrupt safety
  - `InputProcessor` converts raw events to typed events
  - State machines for keyboard and mouse packet processing
- Foundation ready for future scheduling/threading

#### Window System ✓
- **Status**: Core Implementation Complete (ongoing overhaul)
- **Window Hierarchy**: Parent-child relationships with coordinate transformations
- **Event System**: Keyboard and mouse event routing through window tree
- **Graphics Adapters**: Direct framebuffer and double-buffered modes
- **Window Types**: Desktop, Frame, Container, Text, Terminal windows
- **GUI Desktop**: Boots into graphical mode with windowed terminal
- **Implementation Phases**:
  - Phase 1-3: Complete (core infrastructure, basic windows, graphics integration)
  - Phase 4: Partial (UI controls, focus management done)
  - Phase 5: Future (drag/drop, resizing, menus)
- See `docs/window_system_design.md` for detailed architecture

#### Graphics and Image Support ✓
- **Status**: Partially Complete
- **BMP Support**: Full Windows bitmap format support
  - 4/8/16/24/32-bit color depths
  - Palette handling for indexed colors
  - Bottom-up and top-down image formats
- **PNG Support**: Basic implementation (in progress)
  - PNG header and IHDR chunk parsing
  - Color type validation (Grayscale, RGB, Palette, Alpha variants)
  - Bit depth validation
  - **TODO**: DEFLATE decompression for IDAT chunks
  - **TODO**: PNG filtering algorithms
  - **TODO**: Additional chunk support (PLTE, tRNS, etc.)
- **Image Rendering**: Integration with double-buffered display
  - Direct framebuffer image drawing
  - Cursor positioning after image display

#### Storage and Filesystem Support ✓
- **Status**: Initial Implementation Complete
- **Block Device Layer**: 
  - Generic `BlockDevice` trait for all storage devices
  - Full IDE/ATA PIO mode driver with LBA28/48 support
  - Automatic drive detection and identification
  - Support for up to 4 IDE drives
- **Filesystem Abstraction**:
  - Generic `Filesystem` trait for all filesystem implementations
  - Automatic filesystem type detection
  - Virtual Filesystem (VFS) layer for mount management
  - MBR partition table support with up to 4 primary partitions
  - Virtual block devices for individual partitions
- **Arc-based File Handle API**:
  - Modern file API using Arc for shared ownership
  - Thread-safe file and directory operations
  - Automatic resource cleanup
  - Directory enumeration with `enumerate_dir()` method
- **FAT Filesystem**:
  - Complete FAT12/16/32 read support
  - BIOS Parameter Block (BPB) parsing
  - FAT table operations and cluster chain following
  - Directory entry parsing (8.3 filenames)
  - Root directory listing with actual filesystem entries
  - File reading capabilities
  - Integration with shell for filesystem exploration
- **Ext2 Filesystem**:
  - Linux-compatible 1/2/4 KiB block formats and 128–256 byte inodes
  - Writable files/directories, sparse files, truncate, rename, hard links, and symlinks
  - Direct plus single/double/triple-indirect block allocation
  - Real Unix inode/mode/link/block metadata through stat/fstat/lstat
- **Future Filesystem Support**: ext3/ext4 journaling and NTFS remain deferred

#### Basic IPv4 Network Stack ✓
- **Status**: Complete (bounded first release)
- Modern VirtIO-net PCI driver with page-safe DMA pools and checked tokenized queues
- Pinned `no_std` smoltcp 0.12 for Ethernet/ARP/IPv4/DHCPv4/ICMP/UDP/TCP
- Kernel-owned DHCP address/default-route configuration and atomic managed `/etc/resolv.conf`
- Bounded TCP/UDP/ICMP registry with shared socket FD lifetime and deferred close
- Linux x86-64 `AF_INET` socket subset, blocking/nonblocking restart, and poll readiness
- Hermetic restricted-QEMU static-musl resolver fixture plus BusyBox IPv4 `ping`, `nc`, `nslookup`, and HTTP `wget`
- Poll-driven single interface; IPv6, offloads, and NIC IRQs remain future work

#### Cryptographic Randomness and Linux Random Interfaces ✓
- **Status**: Complete
- Host-`/dev/urandom`-backed modern VirtIO RNG for QEMU, with x86-64 RDRAND fallback
- One fail-closed kernel broker for process `AT_RANDOM`, `getrandom(2)`, and read-only `/dev/urandom`
- Cryptographic smoltcp seed and randomized ephemeral-port starting point
- Finite VirtIO completion waits with failed queues quarantined and DMA retained
- Links2 consumes the entropy path through pinned static OpenSSL; BusyBox `wget` remains HTTP-only

#### Links2 HTTPS ✓
- **Status**: Complete
- Pinned static OpenSSL 3.5.7 with TLS 1.2 minimum and no runtime modules/configuration
- Pinned Mozilla CA snapshot imported into `/etc/ssl/cert.pem` only with valid RTC time
- Strict-by-default chain, validity, DNS-name, and numeric-IP verification with SNI
- Hermetic restricted-QEMU positive and negative certificate coverage

### 🔄 Recent Architectural Improvements

#### Code Organization Refactor (Completed)
- **Modular Structure**: Reorganized codebase into clear modules:
  - `arch/` - Architecture-specific code (x86_64)
  - `drivers/` - Device drivers (display)
  - `graphics/` - Graphics subsystem and fonts
  - `lib/` - Core libraries (debug)
  - `mm/` - Memory management
  - `process/` - Process management abstractions
- **Simplified Entry Point**: Reduced main.rs to < 25 lines
- **Centralized Initialization**: All boot logic in `kernel.rs`
- **Improved Maintainability**: Clear separation of concerns

#### Graphics Subsystem Evolution
- **Modern Framebuffer**: Replaced VGA with framebuffer support
- **Performance Optimizations**: 
  - Double buffering with 8MB static buffer
  - Fast memory operations with `ptr::copy()`
  - Efficient scrolling without redraw
- **Rich Font Support**: Multiple font formats and sizes
- **Graphics Primitives**: Lines, rectangles, circles, polygons

## Upcoming Phases

### Phase 6: Multitasking Foundation (Weeks 13-16)
- **6.1 Async/Await Infrastructure**
  - Implement Future trait
  - Create async runtime basics
  - Build executor foundation
- **6.2 Cooperative Multitasking**
  - Task scheduler implementation
  - Task switching mechanism
  - Inter-task communication
- **6.3 Keyboard Task**
  - Async keyboard driver
  - Non-blocking input system

### Phase 7: Advanced Features (Weeks 17-20)
- **7.1 File System Enhancements** ✓ (Partially Complete)
  - ✓ Filesystem abstraction layer implemented
  - ✓ FAT12/16/32 read support
  - ✓ Partition table support
  - ✓ Writable tmpfs, overlay, FAT file writes, and ext2 namespace mutations
  - ✓ VFAT long filename reads
  - ✓ ext2 implementation (ext3/ext4-only features are rejected)
- **7.2 Process Management**
  - Process abstraction
  - Process isolation
  - IPC mechanisms
- **7.3 System Calls**
  - Syscall interface design
  - User/kernel boundary
  - Basic system call implementation

### Phase 8: AgenticOS Specific Features (Weeks 21-24)
- **8.1 Agent Runtime**
  - Agent execution model
  - Lifecycle management
  - Communication protocols
- **8.2 Resource Management**
  - Resource quotas
  - Agent sandboxing
  - Performance monitoring
- **8.3 Network Stack** ✓ (basic IPv4 slice complete)
  - ✓ Modern VirtIO-net, DHCPv4, ICMP, UDP, and TCP
  - ✓ Ring-3 Linux socket ABI and selected BusyBox applets
  - ✓ DHCP-backed DNS through kernel-managed `/etc` and the musl resolver
  - **TODO**: IPv6/TLS and interrupt-driven receive
  - **TODO**: Networked agent communication

## Development Guidelines

### Build Commands
```bash
# Build and run (recommended)
./build.sh

# Clean build
./build.sh -c

# Build only
./build.sh -n

# Run tests
cargo test

# Quick compilation check
cargo check

# Format code
cargo fmt

# Run linter
cargo clippy
```

### Current Project Structure
```
agenticos/
├── src/
│   ├── main.rs              # Minimal entry point
│   ├── kernel.rs            # Kernel initialization
│   ├── panic.rs             # Panic handler
│   ├── bootloader_config.rs # Bootloader configuration
│   ├── arch/
│   │   └── x86_64/
│   │       └── interrupts.rs
│   ├── commands/            # Shell commands (13 implemented)
│   │   ├── shell/           # Main system shell
│   │   ├── cat.rs, head.rs, tail.rs  # File viewing
│   │   ├── ls.rs, dir.rs    # Directory listing
│   │   └── ...              # Other commands
│   ├── drivers/
│   │   ├── display/
│   │   │   ├── display.rs
│   │   │   ├── frame_buffer.rs
│   │   │   ├── text_buffer.rs
│   │   │   ├── double_buffer.rs
│   │   │   └── double_buffered_text.rs
│   │   ├── virtio/          # VirtIO device drivers
│   │   │   ├── mod.rs
│   │   │   ├── common.rs    # Virtqueue implementation
│   │   │   └── input.rs     # VirtIO tablet for seamless mouse
│   │   ├── pci.rs           # PCI bus enumeration
│   │   ├── keyboard.rs
│   │   ├── mouse.rs
│   │   ├── ps2_controller.rs
│   │   ├── block.rs
│   │   └── ide.rs
│   ├── fs/
│   │   ├── mod.rs
│   │   ├── filesystem.rs
│   │   ├── file_handle.rs   # Arc-based file API
│   │   ├── fs_manager.rs    # High-level filesystem API
│   │   ├── partition.rs
│   │   ├── vfs.rs
│   │   └── fat/
│   │       └── ...          # FAT12/16/32 implementation
│   ├── graphics/
│   │   ├── color.rs
│   │   ├── core_text.rs
│   │   ├── core_gfx.rs
│   │   ├── mouse_cursor.rs
│   │   ├── compositor.rs    # Dirty rectangle tracking
│   │   ├── framebuffer.rs   # Region save/restore
│   │   ├── render.rs        # RenderTarget abstraction
│   │   ├── images/
│   │   │   └── ...          # BMP, PNG support
│   │   └── fonts/
│   │       └── ...          # Multiple font formats
│   ├── input/               # Input processing pipeline
│   │   ├── mod.rs           # InputProcessor
│   │   ├── queue.rs         # Lock-free SPSC queue
│   │   ├── keyboard_driver.rs
│   │   └── mouse_driver.rs
│   ├── lib/
│   │   ├── debug.rs
│   │   ├── arc.rs           # Custom Arc/Weak
│   │   └── test_utils.rs
│   ├── mm/
│   │   ├── memory.rs
│   │   ├── frame_allocator.rs
│   │   ├── heap.rs
│   │   └── paging.rs
│   ├── process/
│   │   ├── mod.rs
│   │   ├── process.rs
│   │   └── manager.rs
│   ├── stdlib/              # Standard library extensions
│   │   ├── io.rs            # Read/Write traits
│   │   └── waker.rs
│   ├── tests/               # Kernel tests
│   │   └── ...
│   └── window/              # Window system
│       ├── mod.rs
│       ├── types.rs, event.rs
│       ├── manager.rs, screen.rs
│       ├── cursor.rs, keyboard.rs
│       ├── adapters/        # GraphicsDevice implementations
│       └── windows/         # Window types
├── assets/              # Font and image files
├── docs/                # Design documentation
│   ├── window_system_design.md
│   └── shell_window_integration.md
├── .cargo/
│   └── config.toml
├── rust-toolchain.toml
├── build.sh
└── test.sh
```

### Key Dependencies
- `bootloader` - UEFI bootloader with framebuffer support
- `x86_64` - CPU architecture support
- `spin` - Spinlock implementation for synchronization
- `qemu_print` - Debug output to QEMU serial port

### Testing Strategy
- Unit tests for individual components
- Integration tests for system behavior
- QEMU-based testing for hardware interaction
- Debug logging for development diagnostics

## Success Metrics

### ✅ Achieved
- [x] Boots successfully in QEMU
- [x] Displays text with multiple colors
- [x] Handles debug output via serial port
- [x] Manages physical memory regions
- [x] Handles CPU exceptions
- [x] Supports multiple font formats
- [x] Implements graphics primitives
- [x] Double buffering for performance
- [x] BMP image format support
- [x] Mouse and keyboard input handling
- [x] Basic process abstraction
- [x] IDE/ATA disk driver with auto-detection
- [x] Block device abstraction layer
- [x] Filesystem abstraction with type detection
- [x] MBR partition table support
- [x] FAT12/16/32 filesystem read support
- [x] Virtual filesystem (VFS) layer
- [x] VirtIO tablet support for seamless mouse in QEMU
- [x] PCI bus enumeration and device discovery
- [x] Lock-free input processing pipeline (SPSC queue)
- [x] Window system with hierarchical management
- [x] GUI desktop with windowed terminal
- [x] Basic IPv4 network stack with DHCP, ICMP, UDP, TCP, and ring-3 sockets

### ⏳ In Progress
- [ ] PNG image format support (decompression needed)
- [x] Virtual memory with paging ✓
- [x] Heap allocation support ✓
- [ ] Async/await infrastructure
- [ ] Multitasking support
- [ ] Filesystem write support
- [ ] Long filename support

### 📋 Future Goals
- [ ] Additional filesystem implementations (ext2/3/4, NTFS)
- [ ] Process management
- [ ] System call interface
- [ ] Agent execution environment
- [x] Basic IPv4 network stack
- [x] DHCP-backed IPv4 DNS resolution
- [ ] IPv6 and networked agent protocols
- [ ] Full agent-based computing platform

## Technical Debt and Future Improvements

### Graphics Subsystem
- Consider hardware acceleration support
- Implement proper clipping algorithms
- Add dirty region tracking for efficiency
- Support for multiple display resolutions
- Complete PNG support with DEFLATE decompression
- Add support for additional image formats (JPEG, GIF)
- Implement image scaling and transformation

### Memory Management
- ✓ Complete paging implementation (DONE)
- ✓ Implement efficient heap allocator (DONE - using linked_list_allocator)
- Add memory protection features (per-process isolation)
- Implement copy-on-write optimization
- Add memory-mapped file support
- Support for NUMA architectures (future)

### Architecture
- Consider microkernel design elements
- Evaluate real-time capabilities
- Plan for multi-core support
- Design security architecture

## Resources
- [Writing an OS in Rust](https://os.phil-opp.com/)
- [Intel SDM](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html)
- [OSDev Wiki](https://wiki.osdev.org/)
- [Rust Embedded Book](https://docs.rust-embedded.org/book/)

# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

AgenticOS is a Rust-based operating system targeting Intel x86-64 architecture. This project implements a bare-metal OS from scratch, following established OS development practices while preparing for agent-based computing capabilities.

## Common Commands

### Build and Run
- `./build.sh` - Build kernel, create disk images, and run in QEMU (recommended)
- `./build.sh -c` - Clean build (removes all artifacts first)
- `./build.sh -n` - Build only, don't run QEMU
- `./build.sh -h` - Show help and usage
- `cargo build` - Build the kernel only (won't create disk images)
- `cargo build --release` - Build optimized release version

### Testing
- `./test.sh` - Run kernel tests in QEMU with automatic exit
- `cargo build --features test` - Build kernel with test features enabled
- Tests run automatically on kernel boot when built with test feature
- QEMU exits with success/failure code based on test results

### Code Quality
- `cargo fmt` - Format code according to Rust standards
- `cargo clippy` - Run the Rust linter for code improvements
- `cargo check` - Quick compilation check without producing binaries (preferred for validating code changes)

## Project Structure

The project follows a modular architecture with clear separation of concerns:

### Core Files
- `src/main.rs` - Minimal kernel entry point (< 25 lines)
- `src/kernel.rs` - Kernel initialization and boot sequence
- `src/panic.rs` - Custom panic handler

### Module Organization
- `src/arch/` - Architecture-specific code
  - `x86_64/` - Intel x86-64 specific implementations
    - `interrupts.rs` - Interrupt handling and IDT

- `src/drivers/` - Hardware drivers
  - `display/` - Display and framebuffer drivers
    - `display.rs` - Unified display interface
    - `frame_buffer.rs` - Low-level framebuffer abstraction
    - `text_buffer.rs` - Direct framebuffer text rendering
    - `double_buffer.rs` - Double buffering implementation
    - `double_buffered_text.rs` - Text rendering with double buffering
  - `keyboard.rs` - PS/2 keyboard driver with scancode processing
  - `mouse.rs` - PS/2 mouse driver with packet processing
  - `ps2_controller.rs` - PS/2 controller initialization for keyboard and mouse
  - `block.rs` - Block device trait and abstractions
  - `ide.rs` - IDE/ATA disk driver with LBA support

- `src/graphics/` - Graphics subsystem
  - `color.rs` - Color definitions and utilities
  - `core_text.rs` - Text rendering engine
  - `core_gfx.rs` - Graphics primitives (lines, circles, etc.)
  - `mouse_cursor.rs` - Mouse cursor rendering with background save/restore
  - `fonts/` - Font rendering systems
    - `core_font.rs` - Unified font interface
    - `embedded_font.rs` - Built-in bitmap fonts
    - `vfnt.rs` - VFNT font format support
    - `truetype_font.rs` - TrueType font support
    - `font_data.rs` - Font data definitions

- `src/fs/` - Filesystem layer
  - `mod.rs` - Module exports
  - `filesystem.rs` - Generic filesystem trait and detection
  - `partition.rs` - MBR partition table support
  - `vfs.rs` - Virtual filesystem layer
  - `fat/` - FAT filesystem implementation
    - `filesystem.rs` - FAT filesystem operations
    - `boot_sector.rs` - BIOS Parameter Block parsing
    - `fat_table.rs` - FAT table and cluster operations
    - `directory.rs` - Directory entry handling
    - `types.rs` - FAT-specific types

- `src/lib/` - Core libraries and utilities
  - `debug.rs` - Debug logging system with macros
  - `arc.rs` - Atomic reference counting (Arc/Weak) implementation

- `src/mm/` - Memory management
  - `memory.rs` - Physical memory manager with heap initialization
  - `frame_allocator.rs` - Physical frame allocator using bootloader memory map
  - `heap.rs` - Dynamic memory allocator (100 MiB heap with linked-list allocator)
  - `paging.rs` - Virtual memory paging with demand paging support

- `src/process/` - Core process management primitives
  - `process.rs` - Process trait and PID allocation
  - `mod.rs` - Module exports

- `src/commands/` - Specific command implementations
  - `shell/` - System shell command
    - `mod.rs` - Shell process implementation
  - `mod.rs` - Command exports

### Configuration Files
- `Cargo.toml` - Project manifest with OS-specific dependencies
- `rust-toolchain.toml` - Specifies nightly Rust with required components
- `.cargo/config.toml` - Build configuration and target settings

### Documentation
- `IMPLEMENTATION_PLAN.md` - Phased development roadmap
- `ARCHITECTURE.md` - Detailed architecture documentation
- `CLAUDE.md` - This file, AI assistant guidance

## OS Development Specifics

### Key Attributes
- `#![no_std]` - No standard library (bare metal)
- `#![no_main]` - Custom entry point instead of main()
- `#[no_mangle]` - Preserve function names for bootloader
- `#[panic_handler]` - Custom panic handling

### no_std Environment Restrictions

**CRITICAL: This is a `no_std` environment - the Rust standard library is NOT available.**

#### What this means:
- **NO `std::*` imports** - Only `core::*` and `alloc::*` are available
- **LIMITED heap allocation** - Heap is now available via custom allocator (see Memory Management section)
- **`Vec<T>` and `String` NOW AVAILABLE** - Through the `alloc` crate after heap initialization
- **NO `HashMap` from std** - Use `alloc::collections::BTreeMap` or implement custom data structures
- **NO file I/O, threads, or network** - These require OS support we haven't implemented

#### Heap Allocation Support (NEW):
With the heap allocator now implemented, dynamic allocation is available:
```rust
// NOW SUPPORTED - Vec and String from alloc crate
use alloc::vec::Vec;
use alloc::string::String;

pub fn example() {
    let mut v = Vec::new();
    v.push(42);
    
    let s = String::from("Hello, heap!");
}

// Still use static slices when heap isn't needed
pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[&test1, &test2]
}
```

**Important**: The heap allocator must be initialized before using any `alloc` features. This happens automatically during kernel initialization.

### Arc (Atomic Reference Counting)

The kernel now includes a custom `Arc<T>` implementation for thread-safe shared ownership:

```rust
use crate::lib::arc::{Arc, Weak};

// Create shared data
let data = Arc::new(vec![1, 2, 3, 4, 5]);
let data_clone = data.clone();

// Both references point to the same data
assert_eq!(data[0], 1);
assert_eq!(data_clone[0], 1);
assert_eq!(Arc::strong_count(&data), 2);

// Create weak references
let weak = Arc::downgrade(&data);
assert!(weak.upgrade().is_some());
```

**Key features:**
- Thread-safe atomic reference counting
- Support for weak references to break cycles
- Compatible with `!Sized` types
- Integrated with the kernel's heap allocator
- Memory efficient with proper cleanup

### Testing Approach
- Custom test framework for `no_std` environment
- QEMU integration for hardware testing
- Serial port output for debugging

### Important Resources
- Implementation plan: `IMPLEMENTATION_PLAN.md`
- Architecture documentation: `architecture.md`
- Tutorial reference: https://os.phil-opp.com/

## Memory Management

### Overview
The kernel features a sophisticated memory management system with virtual memory, paging, and dynamic heap allocation:

- **Physical Memory**: Managed by `BootInfoFrameAllocator` using bootloader-provided memory map
- **Virtual Memory**: Page table management with 4KB pages and demand paging
- **Heap Allocation**: 100 MiB heap using `linked_list_allocator` backend
- **Page Fault Handling**: Automatic page allocation for heap memory access

### Components

#### Frame Allocator (`frame_allocator.rs`)
- Allocates physical 4KB memory frames from usable memory regions
- Filters bootloader memory map to only use "Usable" regions
- Skips frame 0 for safety (null pointer protection)
- Provides frames to the virtual memory mapper

#### Heap Allocator (`heap.rs`)
- **Location**: Virtual address `0x_4444_4444_0000`
- **Size**: 100 MiB (configurable)
- **Backend**: `linked_list_allocator` crate v0.10
- **Features**:
  - Global allocator enables `alloc` crate collections (Vec, String, etc.)
  - Demand paging - pages mapped only when accessed
  - Proper OOM (out-of-memory) handling

#### Virtual Memory (`paging.rs`)
- `MemoryMapper` provides unified page table access
- `OffsetPageTable` for virtual-to-physical translations
- Page fault integration for demand paging
- Special handling for physical memory region access

### Usage Examples
```rust
// After heap initialization, these work:
use alloc::{vec, vec::Vec, string::String};

let mut numbers = vec![1, 2, 3, 4, 5];
numbers.push(6);

let message = String::from("Hello from the heap!");

// Large allocations are supported
let large_buffer = vec![0u8; 1024 * 1024]; // 1 MB allocation
```

### Page Fault Handling
When heap memory is accessed before being mapped:
1. Page fault occurs with the unmapped address
2. Handler allocates a new physical frame
3. Maps the virtual page to the physical frame
4. Execution continues transparently

### Debugging
- Page faults log detailed information via `debug_info!`
- Memory regions displayed during boot
- Heap test suite validates allocator functionality

## Testing Framework

The project includes a custom unit testing framework that runs tests directly in the kernel:

### Architecture
- **Test Runner**: Custom test runner based on os.phil-opp.com patterns
- **Test Utilities**: Located in `src/lib/test_utils.rs`
  - `Testable` trait for test functions
  - `test_runner()` function to execute tests
  - QEMU exit functions for test completion
- **Panic Handler**: Different behavior in test mode (exits QEMU with failure)

### Writing Tests
Tests are organized in the `src/tests/` directory by topic:
- `basic.rs` - Basic functionality and sanity tests
- `memory.rs` - Memory management tests
- `heap.rs` - Heap allocator and dynamic memory tests
- `arc.rs` - Arc and Weak reference counting tests
- `display.rs` - Display and graphics tests
- `interrupts.rs` - Interrupt handler tests

To add a new test:
1. Add the test function to the appropriate module
2. Add it to the module's `get_tests()` function (returns `&'static [&'static dyn Testable]`)
3. Tests will automatically run when using `./test.sh`

Example test:
```rust
fn test_example() {
    assert_eq!(2 + 2, 4);
}

// In the test module, return tests as a static slice:
pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_example,
        // other tests...
    ]
}
```

### Running Tests
1. Run `./test.sh` to build and execute tests
2. Tests run automatically during kernel boot
3. QEMU exits with appropriate status code:
   - Exit code 33 (0x10 << 1 | 1) = Success
   - Exit code 35 (0x11 << 1 | 1) = Failure

### Test Output
- Tests print to serial debug output
- Each test shows its name and [ok] on success
- Failed tests trigger panic handler and QEMU exit

## Graphics and Display Subsystem

### Double Buffering Implementation
The framebuffer display system supports both single and double buffering modes, controlled by the `USE_DOUBLE_BUFFER` flag in `src/drivers/display/display.rs`.

**Key Learnings:**
1. **Direct framebuffer access is slow** - Writing pixel-by-pixel to framebuffer memory has poor performance due to slow memory access
2. **Double buffering improves performance** - Writing to a fast memory buffer first, then copying to framebuffer in one operation is much faster
3. **Memory operations are efficient** - Using `ptr::copy()` for buffer swapping and scrolling is far superior to pixel-by-pixel operations
4. **Static allocation works well** - Using an 8MB static buffer avoids heap allocation complexities in the kernel
5. **Unified interfaces simplify code** - The `display.rs` module provides a clean abstraction over different rendering implementations

**Performance Considerations:**
- Single buffering: Each pixel write goes directly to slow framebuffer memory
- Double buffering: Pixel writes go to fast RAM, then bulk copy to framebuffer
- Scrolling: Memory copy operations (`ptr::copy`) are much faster than redrawing

### Image Support
- **BMP format**: Full Windows bitmap support with palette handling (4/8/16/24/32-bit)
- **Located in**: `src/graphics/images/`
- **Usage**: `BmpImage::from_bytes()` with `include_bytes!()` for compile-time embedding
- **Drawing**: Via `display::with_double_buffer()` and `buffer.draw_image()`

**Current Limitations:**
- Graphics concepts are becoming complex and somewhat murky
- The relationship between different display modules needs clarification
- Font rendering and graphics primitives could benefit from better organization
- Future work should revisit and reorganize the graphics subsystem architecture

## Process Abstraction

### Overview
The kernel now includes a basic process abstraction layer as a foundation for future threading and scheduling capabilities. This initial implementation provides:

- **Process trait**: Defines the interface for all processes with `get_id()`, `get_name()`, and `run()` methods
- **PID allocation**: Simple sequential process ID allocation starting from 1
- **Shell process**: The kernel's boot messages and initial system interface extracted into a `ShellProcess`

### Current Implementation
- `src/process/process.rs`: Core process abstractions
  - `Process` trait defining the process interface
  - `ProcessId` type alias for u32
  - `allocate_pid()` function for sequential PID allocation
  
- `src/commands/shell/mod.rs`: System shell command
  - Implements the `Process` trait as `ShellProcess`
  - Displays welcome message, memory statistics, and system tests
  - Demonstrates color support, scrolling, and tab handling
  - Runs as PID 1 during kernel initialization
  - Foundation for future interactive shell capabilities

### Architecture Note
The separation between `/process` and `/commands` provides a clean distinction:
- `/process` contains only the fundamental primitives and traits for process management
- `/commands` contains specific implementations of processes that users can run
- This structure allows for easy addition of new commands while keeping the core process abstraction minimal

### Usage Example
```rust
// In kernel.rs during kernel initialization
use crate::commands::ShellProcess;

let mut shell_process = ShellProcess::new();
debug_info!("Running shell process (PID: {})", shell_process.get_id());
shell_process.run();
```

### Future Considerations
- This is a foundation for future threading/scheduling implementation
- No actual concurrent execution yet - processes run synchronously
- Ready for extension with process states, scheduling, and context switching

## Mouse Support

### Overview
The kernel now includes full PS/2 mouse support with hardware cursor rendering:

- **PS/2 Controller**: Shared controller initialization for both keyboard and mouse devices
- **Mouse Driver**: Handles PS/2 mouse packets, tracks position and button states
- **Hardware Cursor**: Rendered directly to the framebuffer with the double buffer system
- **Interrupt-driven**: Mouse events are processed via IRQ12 interrupts

### Implementation Details

#### PS/2 Controller (`ps2_controller.rs`)
- Initializes the PS/2 controller for both keyboard and mouse
- Enables interrupts for both devices (IRQ1 for keyboard, IRQ12 for mouse)
- Configures the controller with proper settings for both devices

#### Mouse Driver (`mouse.rs`)
- Processes 3-byte PS/2 mouse packets
- Validates packet integrity (bit 3 of first byte must be set)
- Tracks mouse position with screen boundary clamping (0-1279, 0-719)
- Handles all three mouse buttons (left, right, middle)
- Provides `get_state()` function for cursor position queries

#### Mouse Cursor Rendering (`mouse_cursor.rs`)
- Classic arrow cursor design (12x12 pixels)
- Integrates directly with `DoubleBufferedFrameBuffer`
- Background save/restore for clean cursor movement
- Global cursor instance managed via lazy_static
- Drawn in the kernel idle loop when mouse position changes

### Usage
The mouse is automatically initialized during kernel boot and the cursor appears on screen. Mouse movement and button clicks are tracked and logged (movement at debug level, button changes at info level).

## Filesystem Support

### Overview
The kernel includes a filesystem abstraction layer with FAT12/16/32 support:

- **Block devices**: Generic `BlockDevice` trait with IDE/ATA driver
- **Partitions**: MBR partition table support  
- **VFS layer**: Mount management and filesystem detection
- **FAT filesystem**: Read-only FAT12/16/32 implementation with 8.3 filenames

### Key Components
- `src/drivers/block.rs` - Block device abstraction
- `src/drivers/ide.rs` - IDE disk driver
- `src/fs/filesystem.rs` - Filesystem trait
- `src/fs/vfs.rs` - Virtual filesystem layer
- `src/fs/fat/` - FAT implementation
- `src/fs/file_handle.rs` - Arc-based file and directory handles
- `src/fs/fs_manager.rs` - High-level filesystem API

### Arc-based File Handle API

The filesystem now provides modern Arc-based file handles that eliminate lifetime issues and unsafe transmutation:

#### File Operations
```rust
use crate::fs::{File, FileResult};
use crate::lib::arc::Arc;

// Open a file for reading
let file: Arc<File> = File::open_read("/TEST.TXT")?;

// Read entire file as string
let content = file.read_to_string()?;

// Read into buffer
let mut buffer = [0u8; 1024];
let bytes_read = file.read(&mut buffer)?;

// Get file information
println!("Path: {}", file.path());
println!("Size: {} bytes", file.size());
println!("Position: {}", file.position());

// Seek within file
file.seek(100)?;

// Share file handle safely
let file_clone = file.clone();
assert!(file.is_open() && file_clone.is_open());
```

#### Directory Operations
```rust
use crate::fs::{Directory, DirectoryEntry};

// Open a directory
let dir: Arc<Directory> = Directory::open("/")?;

// Get all entries
let entries = dir.entries();
for entry in &entries {
    println!("{} - {} bytes", entry.name_str(), entry.size);
}

// Iterate through entries
let mut dir = Directory::open("/assets")?;
while let Some(entry) = dir.read_entry() {
    match entry.file_type {
        FileType::File => println!("📄 {}", entry.name_str()),
        FileType::Directory => println!("📁 {}", entry.name_str()),
        _ => println!("❓ {}", entry.name_str()),
    }
}
```

#### Key Features
- **Shared Ownership**: Arc enables multiple references to the same file handle
- **Automatic Cleanup**: Files are automatically closed when all references are dropped
- **Memory Safe**: No unsafe lifetime transmutation or manual memory management
- **Thread Safe**: Arc provides atomic reference counting for future multi-threading support
- **Error Handling**: Consistent `FileResult<T>` return types with detailed error information

#### Convenience Functions
```rust
use crate::fs;

// Quick file operations
if fs::exists("/config.txt") {
    let content = fs::read_file_to_string("/config.txt")?;
    fs::write_string("/output.txt", &content)?;
}

// Create new files
let new_file = fs::create_file("/data.bin")?;
new_file.write(b"Binary data")?;

// Process files line by line
fs::for_each_line("/log.txt", |line| {
    println!("Log: {}", line);
    Ok(())
})?;
```

### Filesystem Implementation Notes

#### Directory Enumeration
The filesystem trait includes an `enumerate_dir` method that provides efficient directory listing:
- FAT filesystem overrides this to directly read directory entries from disk
- Returns a `Vec<DirectoryEntry>` for easy iteration
- Supports both root directory and subdirectory enumeration (FAT currently only supports root)

#### Virtual Filesystem (VFS)
- Manages filesystem mounts at specific paths
- Automatically detects filesystem types (FAT12/16/32, ext2/3/4, NTFS)
- Routes file operations to appropriate filesystem implementation
- Currently supports single root mount point

The shell automatically detects and displays filesystem information during boot, and provides filesystem exploration capabilities using the Arc-based API.
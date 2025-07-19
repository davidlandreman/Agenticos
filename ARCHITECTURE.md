# AgenticOS Architecture

This document describes the architecture and design decisions of AgenticOS components.

## Overall Architecture

AgenticOS is a monolithic kernel operating system targeting x86-64. While the eventual goal is to support "agentic" computing capabilities, the current implementation focuses on fundamental OS features. All code runs in kernel space (ring 0) with no user/kernel separation.

**Design Philosophy:**
- Modular organization within monolithic structure
- Clear separation of concerns between subsystems
- Minimal use of unsafe Rust code
- Static memory allocation where possible to avoid heap fragmentation
- Performance over flexibility in critical paths

## Current State Summary

### What Works Well
- **Memory Management**: Robust heap with demand paging
- **Display System**: Fast double-buffered graphics
- **Filesystem**: Clean Arc-based API for file access
- **Input Devices**: Full keyboard and mouse support
- **Command System**: Easy to extend with new commands
- **Build System**: Simple and effective

### What Needs Work
- **No Multitasking**: Everything is synchronous
- **Graphics Architecture**: Organic growth led to complexity
- **Global State**: Over-reliance on statics
- **Test Coverage**: Minimal testing
- **Error Handling**: Inconsistent approaches
- **Documentation**: Some areas lack clarity

### Not Yet Implemented
- **Agent Runtime**: The core vision
- **User Mode**: Everything runs privileged
- **Networking**: No network stack
- **Write Support**: Filesystem is read-only
- **Advanced Features**: No threads, IPC, etc.

### Module Organization

```
src/
├── main.rs              # Minimal entry point (< 25 lines)
├── kernel.rs            # Kernel initialization and main loop
├── panic.rs             # Panic handling
├── arch/                # Architecture-specific (x86_64 only)
├── drivers/             # Hardware device drivers
├── fs/                  # Filesystem layer (FAT read-only)
├── graphics/            # Graphics and font rendering
├── lib/                 # Core utilities (Arc, debug, test)
├── mm/                  # Memory management (heap, paging)
├── process/             # Process abstractions (no scheduling)
├── commands/            # Shell commands (13 implemented)
└── tests/               # Kernel test suite
```

### Key Architectural Decisions

1. **No Standard Library**: Uses `#![no_std]` requiring custom implementations
2. **Static Allocation**: 8MB display buffer, pre-allocated structures
3. **Synchronous Execution**: No async/await or threading
4. **Global State**: Heavy use of `static mut` and `lazy_static`
5. **Trait-Based Abstractions**: BlockDevice, Filesystem, Process, Font

### Testing Framework

AgenticOS includes a custom unit testing framework that runs tests directly in the kernel environment:

- **Test Runner**: Located in `src/lib/test_utils.rs`, provides the `Testable` trait and test execution
- **Test Modules**: Tests are organized in `src/tests/` with submodules for different test categories
- **Panic Handler**: Different behavior in test mode - exits QEMU with failure code
- **QEMU Integration**: Tests run in QEMU and automatically exit with success/failure status

Run tests with `./test.sh` which builds the kernel with test features and executes in QEMU.

## Entry Point and Initialization

### Simplified Entry Point

The `main.rs` file has been reduced to a minimal entry point (< 25 lines) that simply:
1. Declares the kernel entry point using `bootloader_api`
2. Calls `kernel::init()` for initialization
3. Calls `kernel::run()` for the main kernel loop

### Kernel Initialization (`kernel.rs`)

The initialization sequence is now centralized in `kernel.rs`:
1. **Debug subsystem** - Initialize logging first for diagnostics
2. **Interrupts** - Set up interrupt descriptor table
3. **Memory manager** - Initialize physical memory management
4. **Display subsystem** - Initialize framebuffer and text rendering
5. **Shell process** - Run the shell process to show system information and provide user interface

## Architecture-Specific Code (`arch/`)

### x86_64 Support

All architecture-specific code is isolated in `src/arch/x86_64/`:
- **interrupts.rs** - IDT setup, exception handlers, interrupt management
- Future: GDT, paging, CPU-specific features

This separation allows for potential future ports to other architectures.

## Device Drivers (`drivers/`)

### Display Driver Architecture

The display subsystem has evolved organically and now shows architectural complexity:

1. **Display Interface** (`display.rs`)
   - Controls `USE_DOUBLE_BUFFER` flag for performance tuning
   - Provides `println!` and `print!` macros
   - Routes to appropriate buffer implementation

2. **Buffer Implementations**
   - **FrameBuffer**: Direct framebuffer memory access (slow)
   - **DoubleBuffer**: 8MB static buffer with fast bulk copy
   - **TextBuffer**: Direct text rendering
   - **DoubleBufferedText**: Text via double buffer

3. **Performance Characteristics**
   - Single buffering: ~50-100ms per screen clear (poor)
   - Double buffering: ~5-10ms per screen clear (good)
   - Scrolling: Memory move instead of redraw

**Architectural Issues:**
- Multiple overlapping abstractions
- Unclear separation between display/graphics/text
- Tight coupling with global state

### Input Device Drivers

#### PS/2 Controller (`ps2_controller.rs`)
- Initializes the PS/2 controller for both keyboard and mouse
- Configures controller settings and enables interrupts
- Manages the shared hardware interface for both devices

#### Keyboard Driver (`keyboard.rs`)
- Handles PS/2 keyboard scancodes via IRQ1
- Maintains a circular buffer for scancode queuing
- Supports scancode set 2 with proper key mapping
- Processes both make and break codes

#### Mouse Driver (`mouse.rs`)
- Processes 3-byte PS/2 mouse packets via IRQ12
- Validates packet integrity (bit 3 check)
- Tracks absolute cursor position with boundary clamping
- Monitors all three button states
- Provides position/button state queries via `get_state()`


## Graphics Subsystem (`graphics/`)

The graphics subsystem provides rendering capabilities but suffers from organic growth and unclear boundaries.

### Current Components

1. **Graphics Primitives** (`core_gfx.rs`)
   - Bresenham line drawing
   - Circle rendering (outline and filled)
   - Rectangle and polygon support
   - Direct pixel manipulation

2. **Text Rendering** (`core_text.rs`)
   - Multi-line text with alignment
   - Font-agnostic interface
   - Background color support
   - Works with any font implementation

3. **Mouse Cursor** (`mouse_cursor.rs`)
   - 12x12 arrow cursor
   - Background save/restore
   - Tightly coupled to double buffer

4. **Image Support**
   - **BMP**: Full support including palettes
   - **PNG**: Header parsing only (no decompression)

### Architectural Problems

The graphics subsystem has several issues:
- **Unclear Layering**: Display, graphics, and text modules overlap
- **Tight Coupling**: Components directly reference each other
- **Mixed Abstractions**: Low-level pixel ops mixed with high-level rendering
- **Global State**: Heavy reliance on static instances

**Recommendation**: Future refactoring should establish clear layers:
1. Framebuffer access layer
2. Primitive drawing layer  
3. Text/font layer
4. Image/sprite layer
5. Composite/widget layer

### Font System (`graphics/fonts/`)

The font system supports multiple font formats through a unified interface:

1. **Unified Font Trait** (`core_font.rs`)
   - Common interface for all font types
   - Font selection and fallback logic
   - Static font instances for kernel use

2. **Font Implementations**
   - **embedded_font.rs** - Built-in 8x8 bitmap fonts
   - **vfnt.rs** - VFNT format (vector fonts)
   - **truetype_font.rs** - TrueType font support
   - **font_data.rs** - Raw font data storage

## Memory Management (`mm/`)

### Overview

AgenticOS features a sophisticated memory management system with physical memory management, virtual memory paging, and dynamic heap allocation. The system enables the kernel to use dynamic data structures through the `alloc` crate.

### Physical Memory Manager (`memory.rs`)

The memory manager initializes and manages the memory subsystem:
- Parses memory map from bootloader
- Categorizes memory (usable, reserved, bootloader)
- Provides memory statistics
- Initializes frame allocator and heap
- Sets up virtual memory mapping

### Frame Allocator (`frame_allocator.rs`)

The `BootInfoFrameAllocator` manages physical memory frames:
- Allocates 4KB frames from usable memory regions
- Filters bootloader memory map for safe regions
- Skips frame 0 to catch null pointer dereferences
- Implements `FrameAllocator<Size4KiB>` trait from x86_64 crate
- Provides frames for virtual memory operations

### Heap Allocator (`heap.rs`)

Dynamic memory allocation with these characteristics:
- **Virtual Address**: `0x_4444_4444_0000`
- **Size**: 100 MiB (configurable via `HEAP_SIZE`)
- **Backend**: `linked_list_allocator` crate (v0.10)
- **Features**:
  - Global allocator enables `Vec`, `String`, and other `alloc` types
  - Demand paging - memory mapped only when accessed
  - OOM handling with proper error reporting
  - Zero-initialized pages for security

### Virtual Memory (`paging.rs`)

Page table management and virtual memory operations:
- `MemoryMapper` provides centralized page table access
- Uses `OffsetPageTable` for virtual-to-physical translations
- Integrates with page fault handler for demand paging
- Special handling for physical memory region access
- Global mapper instance for interrupt handlers

### Demand Paging Implementation

The heap uses demand paging to allocate physical memory only when accessed:

1. **Initial State**: Heap virtual address space (100 MiB) is reserved but not mapped
2. **First Access**: Triggers page fault with unmapped address
3. **Page Fault Handler**: 
   - Validates address is in heap range (0x_4444_4444_0000 - 0x_4444_4AAA_8FFF)
   - Allocates a physical frame from frame allocator
   - Maps virtual page to physical frame
   - Returns to retry the instruction
4. **Result**: Memory allocated on-demand, reducing initial footprint

This approach means the 100 MiB heap doesn't consume physical memory until actually used.

## Core Libraries (`lib/`)

### Debug Subsystem (`debug.rs`)

The debug system provides structured logging for kernel debugging:

#### Log Levels
1. **Error** (0) - Critical errors and panics
2. **Warn** (1) - Warning conditions
3. **Info** (2) - Informational messages (default)
4. **Debug** (3) - Debug information
5. **Trace** (4) - Detailed execution traces

#### Macro System
- `debug_error!` - Critical errors
- `debug_warn!` - Warnings
- `debug_info!` - General information
- `debug_debug!` - Debug details
- `debug_trace!` - Execution traces

#### Features
- Zero-cost when messages filtered out
- QEMU serial output backend
- Runtime level configuration
- Formatted output with prefixes


## Block Device Layer (`drivers/block.rs`)

The block device layer provides a unified interface for all block storage devices:

### BlockDevice Trait
- **read_blocks()** - Read blocks from device
- **write_blocks()** - Write blocks to device
- **block_size()** - Get device block size (typically 512 bytes)
- **total_blocks()** - Get total number of blocks
- **capacity()** - Calculate total capacity in bytes
- **is_read_only()** - Check if device is read-only
- **flush()** - Flush pending writes

### IDE Driver Implementation
- Full IDE/ATA PIO mode driver (`drivers/ide.rs`)
- Supports up to 4 drives (primary/secondary × master/slave)
- LBA28/LBA48 addressing support
- Automatic drive detection and identification
- Implements `BlockDevice` trait through `IdeBlockDevice` wrapper

## Filesystem Layer (`fs/`)

### Architecture Overview

The filesystem layer provides read-only access to FAT-formatted disks:

1. **Filesystem Trait** (`filesystem.rs`)
   - Generic interface for filesystem implementations
   - Standard operations (open, read, enumerate_dir)
   - Currently only FAT is implemented

2. **Arc-based File API** (`file_handle.rs`)
   - Uses custom Arc implementation for shared ownership
   - Eliminates lifetime issues common in OS development
   - Clean API without callbacks or unsafe transmutation
   - Automatic cleanup when last reference dropped

3. **VFS Layer** (`vfs.rs`)
   - Single global mount point (no multi-mount yet)
   - Filesystem type detection
   - Routes operations to FAT implementation

4. **Limitations**
   - Read-only (no write support)
   - 8.3 filenames only
   - No subdirectory navigation
   - FAT12/16/32 only

### FAT Filesystem Implementation (`fs/fat/`)

Complete FAT12/16/32 filesystem support with read-only operations:

1. **Boot Sector Parsing** (`boot_sector.rs`)
   - BIOS Parameter Block (BPB) parsing
   - FAT type detection based on cluster count
   - Validation and error checking

2. **FAT Table Operations** (`fat_table.rs`)
   - Cluster chain following
   - FAT entry reading for all FAT types
   - Bad cluster detection
   - End-of-chain detection

3. **Directory Support** (`directory.rs`)
   - Short filename (8.3) support
   - Directory entry parsing
   - File attribute handling
   - Directory iteration

4. **Filesystem Operations** (`filesystem.rs`)
   - File reading
   - Directory listing with `enumerate_dir()` method
   - Root directory support for FAT12/16
   - Cluster chain support for FAT32

### Filesystem Detection

The system can automatically detect filesystem types:
- Checks for MBR partition tables
- Reads partition boot sectors
- Identifies FAT12/16/32 by signatures
- Extensible for future filesystem support (ext2/3/4, NTFS)

### Usage Example

```rust
// Arc-based file operations
let file = fs::File::open_read("/test.txt")?;
let content = file.read_to_string()?;

// Directory operations
let dir = fs::Directory::open("/")?;
for entry in dir.entries() {
    println!("{} - {} bytes", entry.name_str(), entry.size);
}
```

## Process Management (`process/`)

### Current Implementation

The "process management" system is currently just a command dispatcher:

1. **Process Traits** (`process.rs`)
   - `Process` and `BaseProcess` define interfaces
   - Sequential PID allocation (no reuse)
   - No actual process control blocks or state

2. **Command Manager** (`manager.rs`)
   - Registry mapping command names to factories
   - Synchronous command execution
   - Simple argument parsing

3. **What's Missing**
   - No CPU context saving/switching
   - No process scheduling
   - No memory isolation
   - No concurrent execution
   - No process lifecycle (create/suspend/terminate)

### Design Limitations

The current design is sufficient for a single-user command-line system but lacks fundamental process management features:
- Everything runs in kernel mode
- Commands block the entire system
- No protection between commands
- No resource limits or accounting

This is intentional for simplicity but must be completely redesigned for true multitasking.

## Panic Handling

The panic handler (`panic.rs`) provides:
- Debug output via serial port
- Visual indication on screen (red text)
- Kernel halt in infinite loop
- Panic message display

## Future Architecture Priorities

### Immediate Needs (Technical Debt)

1. **Graphics Refactoring**
   - Establish clear abstraction layers
   - Reduce coupling between modules
   - Consistent naming and organization

2. **Error Handling**
   - Replace panics with proper Results
   - Consistent error types across subsystems
   - Better error propagation and reporting

3. **Test Coverage**
   - Expand test suite significantly
   - Integration tests for subsystems
   - Performance benchmarks

### Medium-term Goals

1. **True Process Management**
   - CPU context switching
   - Process scheduling (round-robin to start)
   - Basic memory isolation
   - User/kernel mode separation

2. **Filesystem Write Support**
   - FAT write operations
   - Long filename support
   - Subdirectory navigation

3. **Better Memory Management**
   - Per-process memory spaces
   - Copy-on-write pages
   - Memory-mapped files

### Long-term Vision (Agentic Features)

1. **Agent Runtime**
   - WebAssembly or similar sandboxing
   - Resource limits and quotas
   - Inter-agent communication

2. **Networking**
   - Basic network stack
   - Agent-to-agent protocols
   - Remote agent execution

3. **Distributed Features**
   - Agent migration
   - Distributed state management
   - Consensus protocols

### Design Principles

1. **Incremental Progress** - Small, working improvements
2. **Maintainability** - Clean, documented code
3. **Performance** - Measure and optimize bottlenecks
4. **Correctness** - Extensive testing and validation
5. **Simplicity** - Don't over-engineer solutions
# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

AgenticOS is a Rust-based operating system targeting Intel x86-64 architecture. This project implements a bare-metal OS from scratch with the eventual goal of supporting agent-based computing capabilities.

**Current State**: The OS has a solid foundation with memory management, filesystem support, display/graphics, and basic process management. A window system has been implemented that provides hierarchical window management, event routing, and mouse support. The OS now boots into a GUI desktop with a blue background and a windowed terminal application. However, the "Agentic" aspects (agent runtime, advanced process management) are not yet implemented.

## Common Commands

### Build and Run
- `./build.sh` - Build kernel in release mode, create disk images, and run in QEMU (recommended)
- `./build.sh -c` - Clean build (removes all artifacts first)
- `./build.sh -d` - Build in debug mode (larger kernel, slower boot, more symbols)
- `./build.sh -n` - Build only, don't run QEMU
- `./build.sh -h` - Show help and usage
- `cargo build` - Build the kernel only (won't create disk images)
- `cargo build --release` - Build optimized release version

**QEMU Configuration**: The build script runs QEMU with 128M RAM, serial output, VirtIO tablet for seamless mouse, and isa-debug-exit for test integration.

### Testing
- `./test.sh` - Run kernel tests in QEMU with automatic exit
- `cargo build --features test` - Build kernel with test features enabled
- Tests run automatically on kernel boot when built with test feature
- QEMU exits with success/failure code based on test results
- **Note**: Test coverage is limited and should be expanded

### Code Quality
- `cargo fmt` - Format code according to Rust standards
- `cargo clippy` - Run the Rust linter for code improvements
- `cargo check` - Quick compilation check without producing binaries (preferred for validating code changes)

## Project Structure

The project follows a modular monolithic kernel design with clear separation of concerns. All code runs in kernel space (ring 0) with no user/kernel boundary yet.

### Core Files
- `src/main.rs` - Minimal kernel entry point (< 25 lines)
- `src/kernel.rs` - Kernel initialization and boot sequence
- `src/panic.rs` - Custom panic handler for kernel panics

### Module Organization
- `src/arch/` - Architecture-specific code
  - `x86_64/` - Intel x86-64 specific implementations
    - `interrupts.rs` - Interrupt handling and IDT

- `src/drivers/` - Hardware drivers
  - `display/` - Display and framebuffer drivers (uses modern framebuffer, NOT VGA)
    - `display.rs` - Unified display interface (controls single/double buffering)
    - `frame_buffer.rs` - Low-level framebuffer abstraction
    - `text_buffer.rs` - Direct framebuffer text rendering
    - `double_buffer.rs` - 8MB static buffer for performance
    - `double_buffered_text.rs` - Text rendering with double buffering
  - `virtio/` - VirtIO device drivers
    - `mod.rs` - VirtIO module initialization
    - `common.rs` - VirtIO device abstraction and Virtqueue implementation
    - `input.rs` - VirtIO tablet device for absolute mouse positioning (seamless in QEMU)
  - `pci.rs` - PCI bus enumeration, configuration space access, BAR reading
  - `keyboard.rs` - PS/2 keyboard driver with scancode set 2 support
  - `mouse.rs` - PS/2 mouse driver with 3-button support (fallback when VirtIO unavailable)
  - `ps2_controller.rs` - Shared PS/2 controller for keyboard/mouse
  - `block.rs` - Block device trait for storage abstraction
  - `ide.rs` - IDE/ATA PIO mode driver (supports 4 drives)

- `src/input/` - Input processing pipeline (lock-free architecture)
  - `mod.rs` - Central `InputProcessor` for event conversion and routing
  - `queue.rs` - Lock-free SPSC (Single-Producer Single-Consumer) ring buffer (256 entries)
  - `keyboard_driver.rs` - PS/2 scancode set 2 to KeyCode conversion with state machine
  - `mouse_driver.rs` - PS/2 mouse packet state machine

- `src/graphics/` - Graphics subsystem
  - `color.rs` - RGB color definitions and predefined colors
  - `core_text.rs` - Font-agnostic text rendering
  - `core_gfx.rs` - Graphics primitives (Bresenham lines, circles, polygons)
  - `mouse_cursor.rs` - 12x12 hardware cursor with background save/restore
  - `compositor.rs` - Dirty rectangle tracking and cursor overlay management
  - `framebuffer.rs` - Region save/restore abstraction (`SavedRegion`, `RegionCapableBuffer` trait)
  - `render.rs` - `RenderTarget` abstraction for efficient row-based drawing
  - `fonts/` - Multiple font format support
    - `core_font.rs` - Unified font trait and selection
    - `embedded_font.rs` - Built-in 8x8 bitmap fonts
    - `vfnt.rs` - VFNT vector font format
    - `truetype_font.rs` - TrueType font parsing and rendering
    - `font_data.rs` - Raw font data storage
  - `images/` - Image format support
    - `bmp.rs` - Full Windows BMP support (4/8/16/24/32-bit)
    - `png.rs` - PNG header parsing only (no decompression yet)

- `src/fs/` - Filesystem layer (read-only currently)
  - `filesystem.rs` - Generic filesystem trait
  - `partition.rs` - MBR partition table parsing
  - `vfs.rs` - Virtual filesystem with mount management
  - `file_handle.rs` - Arc-based file API (modern design)
  - `fs_manager.rs` - High-level filesystem operations
  - `fat/` - FAT12/16/32 implementation
    - `filesystem.rs` - FAT operations (8.3 filenames only)
    - `boot_sector.rs` - BIOS Parameter Block parsing
    - `fat_table.rs` - Cluster chain following
    - `directory.rs` - Directory entry parsing
    - `types.rs` - FAT-specific types

- `src/lib/` - Core libraries and utilities
  - `debug.rs` - 5-level debug logging (error/warn/info/debug/trace)
  - `arc.rs` - Custom Arc/Weak implementation for kernel use
  - `test_utils.rs` - Testing framework for no_std environment

- `src/stdlib/` - Standard library extensions for no_std
  - `io.rs` - Read/Write traits, `StdinBuffer` for buffered input
  - `waker.rs` - Async waker support for future use

- `src/mm/` - Memory management (fully implemented)
  - `memory.rs` - Memory subsystem initialization
  - `frame_allocator.rs` - 4KB frame allocation from bootloader map
  - `heap.rs` - 100 MiB heap at 0x_4444_4444_0000 (linked-list allocator)
  - `paging.rs` - Virtual memory with demand paging

- `src/process/` - Process management (basic foundation only)
  - `process.rs` - Process/BaseProcess traits, PID allocation
  - `manager.rs` - Command registry and execution

- `src/window/` - Window system implementation
  - `mod.rs` - Window system initialization and global functions
  - `types.rs` - Core types (WindowId, ScreenId, Rect, Point, ColorDepth)
  - `event.rs` - Event system (keyboard, mouse, window events)
  - `graphics.rs` - GraphicsDevice trait for rendering abstraction
  - `manager.rs` - WindowManager for coordinating windows and screens with parent-child coordinate transformations
  - `screen.rs` - Screen abstraction for virtual displays
  - `console.rs` - Console output buffer for print macro integration
  - `terminal.rs` - Terminal window support
  - `cursor.rs` - CursorRenderer with background save/restore for clean movement
  - `keyboard.rs` - PS/2 scancode set 2 to KeyCode conversion for window events
  - `adapters/` - GraphicsDevice implementations
    - `direct_framebuffer.rs` - Direct framebuffer writes (fast, used for mouse)
    - `double_buffered.rs` - Double buffered rendering (smooth but slower)
  - `windows/` - Window implementations
    - `base.rs` - Base window functionality with parent-child tracking
    - `container.rs` - Container window that can hold children
    - `text.rs` - Text grid window for terminal output
    - `terminal.rs` - Terminal window with input handling
    - `frame.rs` - Frame window with title bar and borders
    - `desktop.rs` - Desktop background window

- `src/commands/` - Shell commands (13 implemented)
  - `shell/` - Main system shell
  - `dir.rs`, `ls.rs` - Directory listing
  - `cat.rs`, `head.rs`, `tail.rs` - File viewing
  - `echo.rs`, `wc.rs`, `grep.rs` - Text processing
  - `touch.rs` - File creation
  - `hexdump.rs` - Binary viewing
  - `time.rs` - System time
  - `pwd.rs` - Working directory

- `src/tests/` - Test modules
  - `basic.rs`, `memory.rs`, `heap.rs`, `arc.rs` - Core tests
  - `display.rs`, `interrupts.rs` - Hardware tests

### Configuration Files
- `Cargo.toml` - Project manifest with OS-specific dependencies
- `rust-toolchain.toml` - Specifies nightly Rust with required components
- `.cargo/config.toml` - Build configuration and target settings
- `x86_64-agenticos.json` - Custom target specification

### Documentation
- `IMPLEMENTATION_PLAN.md` - Phased development roadmap
- `ARCHITECTURE.md` - Detailed architecture documentation
- `CLAUDE.md` - This file, AI assistant guidance
- `docs/window_system_design.md` - Window system architecture and implementation status
- `docs/shell_window_integration.md` - Shell/terminal window integration design

## Known Issues and Technical Debt

### Current Limitations
1. **No Multitasking** - Everything runs synchronously in kernel space
2. **Read-Only Filesystem** - No write support implemented
3. **8.3 Filenames Only** - No long filename support
4. **Limited Test Coverage** - Many subsystems lack comprehensive tests
5. **Global State** - Heavy use of `static mut` and `lazy_static`
6. **No User Space** - Everything runs in ring 0 (kernel mode)
7. **Constant Window Repainting** - TextWindow constantly repaints even when no changes occur, causing performance issues

### Areas Needing Refactoring
1. **Graphics Subsystem** - Complex relationships between display modules
2. **Error Handling** - Inconsistent use of panic! vs Result
3. **Command System** - Could benefit from better parsing/validation
4. **Mouse Integration** - Cursor rendering tightly coupled to display

### Performance Considerations
- Direct framebuffer writes are faster for mouse cursor than double buffering
- Double buffering's 3.5MB copies on every frame are expensive for simple updates
- Memory operations (ptr::copy) are much faster than pixel-by-pixel
- Static allocation avoids heap fragmentation in critical paths
- Window system uses smart rendering - only updates when mouse moves or content changes
- **Text rendering optimization**: TextWindow uses incremental updates (dirty cell tracking) to avoid redrawing all characters on each keypress
- **Window manager optimization**: Only clears screen on full redraw (`needs_redraw`), not on every paint
- **GUI Desktop**: Default boot mode is now GUI with windowed terminal, providing modern desktop experience
- **Coordinate Transformation**: Parent-child window relationships properly handle coordinate offsets during rendering

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

### Architecture Overview
The display system uses a **modern framebuffer** (NOT VGA text mode) provided by the bootloader. The system is organized in layers but the relationships between modules have become complex and need refactoring.

### Double Buffering Implementation
Double buffering is controlled by the `USE_DOUBLE_BUFFER` flag in `src/drivers/display/display.rs`:
- **Enabled (default)**: 8MB static buffer, fast performance
- **Disabled**: Direct framebuffer writes, slower but simpler

**Performance Insights:**
1. **Framebuffer memory is slow** - Direct pixel writes have high latency
2. **Bulk copies are fast** - `ptr::copy()` for buffer swapping is efficient
3. **Scrolling optimization** - Memory move instead of redrawing saves cycles
4. **Static allocation** - Avoids heap fragmentation for critical path

### Graphics Capabilities
- **Primitives**: Lines (Bresenham), circles, rectangles, polygons
- **Text**: Multiple fonts (bitmap, VFNT, TrueType) with alignment
- **Images**: Full BMP support, partial PNG (headers only)
- **Mouse**: Hardware cursor with background save/restore
- **Colors**: RGB support with predefined palette

### Current Architecture Issues
The graphics subsystem has grown organically and now suffers from:
- Unclear module boundaries (display vs graphics vs fonts)
- Tight coupling between components
- Mixed abstraction levels
- Inconsistent naming conventions

**Recommendation**: Next major refactor should establish clear layers:
1. Raw framebuffer access
2. Drawing primitives
3. Text/font rendering
4. Image loading/display
5. Composite operations (windows, widgets)

## Process Management

### Current State
AgenticOS has a basic process abstraction but **no actual multitasking**. All "processes" run synchronously in kernel space. This is a foundation for future work, not a complete implementation.

### What's Implemented
- **Process traits**: `Process` and `BaseProcess` define the interface
- **PID allocation**: Sequential IDs starting from 1 (no reuse)
- **Command registry**: Maps command names to factory functions
- **Shell integration**: Unknown commands routed to process manager

### What's NOT Implemented
- **No scheduling** - Processes run to completion
- **No context switching** - No saved CPU state
- **No isolation** - All code shares kernel memory
- **No concurrency** - Single execution thread
- **No IPC** - No inter-process communication

### Command System
The current "process" system is really just a command dispatcher:
```rust
// Commands are registered at boot
register_command("ls", create_ls_process);

// Shell executes them synchronously
execute_command("ls /home")?;  // Runs to completion
```

This provides a clean way to add commands but is not true process management.

### How Commands Work

Commands are registered at boot and executed through the process manager:

1. **Registration**: Each command registers a factory function
   ```rust
   register_command("ls", create_ls_process);
   ```

2. **Execution**: Shell routes commands to process manager
   ```rust
   execute_command("ls /home")?;
   ```

3. **Implementation**: Commands implement `RunnableProcess`
   ```rust
   pub trait RunnableProcess {
       fn run(&mut self);
       fn get_name(&self) -> &str;
   }

### Adding New Commands

To add a new command, follow this pattern:

1. **Create command file**: `src/commands/mycommand/mod.rs`
   ```rust
   use crate::process::{BaseProcess, HasBaseProcess, RunnableProcess};
   
   pub struct MyCommandProcess {
       pub base: BaseProcess,
       args: Vec<String>,
   }
   
   // Factory function for registration
   pub fn create_mycommand_process(args: Vec<String>) -> Box<dyn RunnableProcess> {
       Box::new(MyCommandProcess::new_with_args(args))
   }
   ```

2. **Implement traits**: Minimal boilerplate required
   ```rust
   impl RunnableProcess for MyCommandProcess {
       fn run(&mut self) { /* your code here */ }
       fn get_name(&self) -> &str { self.base.get_name() }
   }
   ```

3. **Register in kernel.rs**:
   ```rust
   register_command("mycommand", create_mycommand_process);
   ```

4. **Export from commands/mod.rs**:
   ```rust
   pub mod mycommand;
   ```

That's it! The command is now available in the shell.

## Window System

### Overview
The window system provides a modern GUI framework with hierarchical window management:

- **Window Hierarchy**: Parent-child relationships with proper event propagation and coordinate transformations
- **Multiple Screens**: Support for virtual screens (though only one physical display)
- **Event System**: Keyboard and mouse events routed through window tree
- **Double Buffering**: Smooth rendering with configurable buffering modes
- **Mouse Support**: Hardware cursor with click and drag capabilities
- **GUI Desktop**: Boots directly into graphical mode with blue desktop background

### Window Types
- **DesktopWindow**: Blue background for the desktop (0, 50, 100 RGB)
- **FrameWindow**: Windows with title bars and borders (uses WindowBase for parent tracking)
  - Active windows have blue borders/title bar
  - Inactive windows have grey borders/title bar
  - Title bar height: 24 pixels, border width: 2 pixels
- **TextWindow**: Grid-based text rendering for terminals
  - Uses 8x8 bitmap font by default
  - Tracks dirty cells for incremental updates
  - Dark grey background (32, 32, 32 RGB)
- **TerminalWindow**: Interactive terminal with command input
  - Wraps TextWindow with input handling
  - Manages command history and cursor
- **ContainerWindow**: Generic container for child windows

### Default Desktop Layout
The system boots into a GUI desktop with:
- Blue desktop background covering the entire screen
- Terminal window in a frame at position (100, 50)
- Terminal frame size: 800x600 pixels (or smaller if screen is small)
- Terminal displays "AgenticOS Terminal" in the title bar

### Implementation Details
- All windows now use `WindowBase` for consistent parent-child tracking
- The `render_window_tree_with_offset` method handles coordinate transformations
- Child windows are positioned relative to their parent's coordinate system
- Window bounds are temporarily adjusted during rendering for proper positioning

### Implementation Status
Based on the phased development approach:
- **Phase 1 (Core Infrastructure)**: Complete - Window trait, types, event system
- **Phase 2 (Basic Windows)**: Complete - ContainerWindow, TextWindow, event routing
- **Phase 3 (Graphics Integration)**: Complete - GraphicsDevice adapters, clipping
- **Phase 4 (UI Controls)**: Partial - FrameWindow done, focus management working, mouse interaction complete
- **Phase 5 (Advanced Features)**: Future - Drag/drop, window resizing, menus

See `docs/window_system_design.md` for detailed architecture documentation.

## Mouse Support

### Overview
The kernel supports multiple mouse input methods with automatic fallback:

- **VirtIO Tablet (Primary)**: Seamless absolute positioning in QEMU - no cursor grabbing
- **PS/2 Mouse (Fallback)**: Traditional relative positioning when VirtIO unavailable
- **Hardware Cursor**: Rendered via window system with background save/restore
- **Interrupt-driven**: Events processed via IRQ12 (PS/2) or VirtIO interrupts

### Input Method Selection
During initialization, the kernel:
1. Scans PCI bus for VirtIO tablet device
2. If found, initializes VirtIO tablet with screen dimensions for coordinate scaling
3. If not found, falls back to PS/2 mouse via IRQ12

### Implementation Details

#### VirtIO Tablet (`drivers/virtio/input.rs`)
- Provides absolute positioning (seamless mouse in QEMU)
- Scales tablet coordinates to screen resolution via `init_with_screen()`
- Uses PCI bus enumeration to detect device
- Requires `-device virtio-tablet-pci` QEMU flag

#### PS/2 Controller (`ps2_controller.rs`)
- Initializes the PS/2 controller for both keyboard and mouse
- Enables interrupts for both devices (IRQ1 for keyboard, IRQ12 for mouse)
- Configures the controller with proper settings for both devices

#### PS/2 Mouse Driver (`mouse.rs`)
- Processes 3-byte PS/2 mouse packets (fallback mode)
- Validates packet integrity (bit 3 of first byte must be set)
- Tracks mouse position with screen boundary clamping
- Handles all three mouse buttons (left, right, middle)
- Provides `get_state()` function for cursor position queries

#### Mouse Cursor Rendering (`window/cursor.rs`, `graphics/mouse_cursor.rs`)
- Classic arrow cursor design (12x12 pixels)
- Window system's CursorRenderer handles drawing
- Background save/restore for clean cursor movement
- Drawn during render loop when mouse position changes

### Usage
The mouse is automatically initialized during kernel boot. VirtIO tablet provides seamless mouse experience in QEMU (cursor moves freely between host and guest). PS/2 mode requires QEMU to grab the mouse.

## Input Processing Pipeline

### Overview
AgenticOS uses a sophisticated three-layer input processing architecture designed for interrupt safety and clean event routing:

1. **Hardware Layer**: Interrupt handlers push raw events to lock-free queue
2. **Processing Layer**: `InputProcessor` converts raw scancodes to typed events
3. **Event Layer**: Typed events routed to window system

### Lock-Free Event Queue (`src/input/queue.rs`)
- SPSC (Single-Producer Single-Consumer) ring buffer design
- 256-entry capacity (power of 2 for efficient modulo)
- Atomic operations with Release/Acquire ordering
- **Critical**: Prevents interrupt handler blocking (try_lock was causing issues)

```rust
// Interrupt handler (producer) - never blocks
pub fn push(&self, event: RawInputEvent) -> bool {
    // Uses atomic compare_exchange, returns false if full
}

// Main loop (consumer) - processes events
pub fn pop(&self) -> Option<RawInputEvent> {
    // Uses atomic load/store with proper ordering
}
```

### Keyboard Processing (`src/input/keyboard_driver.rs`)
- State machine for PS/2 scancode set 2
- Handles extended scancodes (0xE0 prefix)
- Tracks modifier state (Shift, Ctrl, Alt)
- Converts to `KeyCode` enum for window system

### Mouse Processing (`src/input/mouse_driver.rs`)
- State machine for 3-byte PS/2 packets
- Validates packet integrity before processing
- Produces `MouseEvent` with position delta and button state

### Integration with Window System
The `InputProcessor` is called from the kernel's idle loop:
```rust
// In kernel main loop
input::process_pending_events();  // Drains queue, routes to windows
window::render_frame();           // Renders any changes
```

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

### Arc-based File API

The filesystem uses Arc (atomic reference counting) for safe file handle sharing:

```rust
use crate::fs::File;
use crate::lib::arc::Arc;

// Open and read files
let file: Arc<File> = File::open_read("/TEST.TXT")?;
let content = file.read_to_string()?;

// Clone handles safely
let file2 = file.clone();  // Both share same file
```

**Key Benefits:**
- No lifetime issues or unsafe code
- Automatic cleanup when last reference dropped
- Ready for future multi-threading
- Clean API without callbacks

**Limitations:**
- Read-only (no write support)
- 8.3 filenames only
- FAT filesystem only
- No subdirectory support yet


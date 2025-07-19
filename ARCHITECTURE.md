# AgenticOS Architecture

This document describes the architecture and design decisions of AgenticOS components.

## Overall Architecture

AgenticOS follows a modular monolithic kernel design with clear separation between architecture-specific code, device drivers, and core kernel services. The kernel is organized into distinct modules that communicate through well-defined interfaces.

### Module Organization

```
src/
├── main.rs              # Minimal entry point
├── kernel.rs            # Kernel initialization
├── panic.rs             # Panic handling
├── arch/                # Architecture-specific
├── drivers/             # Device drivers
├── graphics/            # Graphics subsystem
├── lib/                 # Core libraries
└── mm/                  # Memory management
```

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
5. **Boot messages** - Display system information

## Architecture-Specific Code (`arch/`)

### x86_64 Support

All architecture-specific code is isolated in `src/arch/x86_64/`:
- **interrupts.rs** - IDT setup, exception handlers, interrupt management
- Future: GDT, paging, CPU-specific features

This separation allows for potential future ports to other architectures.

## Device Drivers (`drivers/`)

### Display Driver Architecture

The display subsystem (`src/drivers/display/`) provides multiple layers:

1. **Unified Interface** (`display.rs`)
   - Provides consistent API regardless of buffering mode
   - Runtime selection between single/double buffering
   - Exports print macros for kernel-wide use

2. **Framebuffer Abstraction** (`frame_buffer.rs`)
   - Low-level pixel manipulation
   - Direct memory access to framebuffer
   - Format conversion for different pixel formats

3. **Text Rendering**
   - **text_buffer.rs** - Direct rendering to framebuffer
   - **double_buffered_text.rs** - Renders to memory buffer first
   - Both implement scrolling, color support, and font rendering

4. **Double Buffering** (`double_buffer.rs`)
   - 8MB static buffer allocation
   - Fast memory-to-memory operations
   - Significant performance improvement over direct writes

## Graphics Subsystem (`graphics/`)

### Core Graphics Components

1. **Color Management** (`color.rs`)
   - RGB color representation
   - Predefined color constants
   - Color conversion utilities

2. **Graphics Primitives** (`core_gfx.rs`)
   - Line drawing (Bresenham's algorithm)
   - Rectangle and circle rendering
   - Triangle and polygon support
   - Both outline and filled variants

3. **Text Rendering Engine** (`core_text.rs`)
   - Font-agnostic text rendering
   - Multi-line text support
   - Text alignment (left, center, right)
   - Background color support

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

### Physical Memory Manager (`memory.rs`)

The memory manager tracks physical memory regions:
- Parses memory map from bootloader
- Categorizes memory (usable, reserved, bootloader)
- Provides memory statistics
- Foundation for future heap allocator

Key features:
- Static allocation (no heap required)
- Fixed-size region tracking
- Memory region iteration
- Statistics gathering

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

## Display Subsystem Architecture

### Performance Optimizations

1. **Double Buffering Benefits**
   - Reduces framebuffer access latency
   - Enables bulk memory operations
   - Smoother visual updates
   - Configurable via `USE_DOUBLE_BUFFER` flag

2. **Memory Operations**
   - Uses `ptr::copy()` for fast memory moves
   - Optimized scrolling without redrawing
   - Efficient buffer swapping

### Current Limitations

1. **Static Buffer Size** - 8MB hardcoded limit
2. **Single Font Size** - No dynamic font scaling
3. **Limited Graphics** - Basic primitives only
4. **No Hardware Acceleration** - Pure software rendering

## Panic Handling

The panic handler (`panic.rs`) provides:
- Debug output via serial port
- Visual indication on screen (red text)
- Kernel halt in infinite loop
- Panic message display

## Future Architecture Enhancements

### Planned Improvements

1. **Memory Management**
   - Heap allocator implementation
   - Virtual memory/paging
   - Memory protection

2. **Process Management**
   - Task/thread abstraction
   - Scheduler implementation
   - Inter-process communication

3. **File System**
   - VFS layer
   - Basic file system driver
   - Device file abstraction

4. **Networking**
   - Network driver framework
   - TCP/IP stack
   - Socket abstraction

5. **Agent Support**
   - Agent execution environment
   - Resource isolation
   - Communication protocols

### Design Principles

1. **Modularity** - Clear separation of concerns
2. **Safety** - Leverage Rust's type system
3. **Performance** - Optimize critical paths
4. **Simplicity** - Avoid over-engineering
5. **Extensibility** - Easy to add new features
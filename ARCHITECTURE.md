# AgenticOS Architecture

This document describes the architecture and design decisions of AgenticOS components.

## Debug Subsystem

The debug subsystem provides a structured logging interface for kernel debugging, built as an abstraction layer over QEMU's serial output capabilities.

### Overview

The debug system (`src/debug.rs`) wraps the external `qemu_print` crate to provide:
- Hierarchical log levels
- Formatted output with level prefixes
- Runtime-configurable verbosity
- Zero-cost abstractions when messages are filtered

### Log Levels

The system defines five log levels in ascending order of verbosity:

1. **Error** (0) - Critical errors and panics
2. **Warn** (1) - Warning conditions
3. **Info** (2) - Informational messages (default)
4. **Debug** (3) - Debug information
5. **Trace** (4) - Detailed execution traces

### Architecture Details

#### Static Configuration
- Debug level stored in static mutable variable for performance
- No heap allocation required
- Compile-time macro expansion for zero-cost filtering

#### Macro System
Each log level has a corresponding macro:
- `debug_error!` - Always visible errors
- `debug_warn!` - Warnings when level ≥ Warn
- `debug_info!` - Standard informational output
- `debug_debug!` - Detailed debug information
- `debug_trace!` - Verbose execution traces

Additional utility macros:
- `debug_print!` - Raw output without newline
- `debug_println!` - Raw output with newline

#### Message Format
```
[LEVEL] message content
```
Example output:
```
[INFO ] === AgenticOS Kernel Starting ===
[DEBUG] Boot info address: 0xdeadbeef
[TRACE] print_hello: Starting VGA buffer initialization
```

### Usage Guidelines

1. **Initialization**: Call `debug::init()` early in kernel startup
2. **Level Selection**:
   - Use `Error` for unrecoverable conditions
   - Use `Warn` for recoverable issues
   - Use `Info` for major state changes
   - Use `Debug` for detailed state information
   - Use `Trace` for function entry/exit and detailed flow
3. **Performance**: Messages below current level are eliminated at compile time

### Integration Points

The debug system is integrated throughout the kernel:
- `main.rs` - Kernel initialization and boot sequence
- `memory.rs` - Memory manager operations
- `vga_buffer.rs` - Display driver operations

### Future Enhancements

Potential improvements for the debug subsystem:
- Multiple output targets (serial, framebuffer, network)
- Structured logging with key-value pairs
- Log buffering for crash analysis
- Per-module log level configuration
- Timestamp support when timer interrupts are available
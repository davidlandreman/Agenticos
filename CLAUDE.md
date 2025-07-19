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
- `cargo test` - Run all tests
- `cargo test <test_name>` - Run a specific test

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

- `src/graphics/` - Graphics subsystem
  - `color.rs` - Color definitions and utilities
  - `core_text.rs` - Text rendering engine
  - `core_gfx.rs` - Graphics primitives (lines, circles, etc.)
  - `fonts/` - Font rendering systems
    - `core_font.rs` - Unified font interface
    - `embedded_font.rs` - Built-in bitmap fonts
    - `vfnt.rs` - VFNT font format support
    - `truetype_font.rs` - TrueType font support
    - `font_data.rs` - Font data definitions

- `src/lib/` - Core libraries and utilities
  - `debug.rs` - Debug logging system with macros

- `src/mm/` - Memory management
  - `memory.rs` - Physical memory manager

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

### Testing Approach
- Custom test framework for `no_std` environment
- QEMU integration for hardware testing
- Serial port output for debugging

### Important Resources
- Implementation plan: `IMPLEMENTATION_PLAN.md`
- Architecture documentation: `architecture.md`
- Tutorial reference: https://os.phil-opp.com/

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

**Current Limitations:**
- Graphics concepts are becoming complex and somewhat murky
- The relationship between different display modules needs clarification
- Font rendering and graphics primitives could benefit from better organization
- Future work should revisit and reorganize the graphics subsystem architecture
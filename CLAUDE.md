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
- `cargo check` - Quick compilation check without producing binaries

## Project Structure

The project structure for OS development:
- `src/main.rs` - Kernel entry point with `#![no_std]` and `#![no_main]`
- `src/debug.rs` - Debug subsystem for structured kernel logging
- `src/memory.rs` - Memory management implementation
- `src/vga_buffer.rs` - VGA text mode display driver
- `Cargo.toml` - Project manifest with OS-specific dependencies
- `rust-toolchain.toml` - Specifies nightly Rust with required components
- `.cargo/config.toml` - Build configuration and target settings
- `target/` - Build artifacts (gitignored)
- `IMPLEMENTATION_PLAN.md` - Phased development roadmap
- `ARCHITECTURE.md` - Detailed architecture documentation for kernel subsystems

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
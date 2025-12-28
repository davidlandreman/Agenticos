# AgenticOS

A Rust-based operating system for x86-64 architecture, built from scratch with the goal of supporting agent-based computing.

## Current Status

AgenticOS boots into a GUI desktop with a windowed terminal application. The OS has working memory management, filesystem support, graphics, and input handling.

### Implemented Features

- **GUI Desktop**: Boots directly into graphical mode with a blue desktop background
- **Window System**: Hierarchical window management with mouse support
- **Terminal**: Windowed terminal with shell and 13 built-in commands
- **Memory Management**: Virtual memory with paging and 100 MiB heap
- **Filesystem**: Read-only FAT12/16/32 with VFS layer
- **Input**: VirtIO tablet (seamless in QEMU) with PS/2 fallback
- **Graphics**: Framebuffer with double buffering, multiple fonts, BMP images

### Not Yet Implemented

- Multitasking / async execution
- Filesystem write support
- User space / process isolation
- Networking
- Agent runtime

## Building

### Prerequisites

- Rust nightly toolchain (managed automatically via `rust-toolchain.toml`)
- QEMU for x86-64 (`qemu-system-x86_64`)

### Build and Run

```bash
# Build and run in QEMU (recommended)
./build.sh

# Clean build
./build.sh -c

# Debug build (larger kernel, more symbols)
./build.sh -d

# Build only, don't run QEMU
./build.sh -n
```

### Testing

```bash
# Run kernel tests
./test.sh
```

## Shell Commands

Once booted, the terminal supports these commands:

| Command | Description |
|---------|-------------|
| `ls`, `dir` | List directory contents |
| `cat` | Display file contents |
| `head` | Show first lines of a file |
| `tail` | Show last lines of a file |
| `grep` | Search for patterns in files |
| `wc` | Count lines, words, characters |
| `echo` | Print text |
| `pwd` | Print working directory |
| `hexdump` | Display file in hex format |
| `time` | Show system time |
| `touch` | Create empty file (limited) |

## Project Structure

```
agenticos/
├── src/
│   ├── main.rs              # Kernel entry point
│   ├── kernel.rs            # Boot sequence
│   ├── arch/x86_64/         # x86-64 specific code
│   ├── drivers/             # Hardware drivers
│   │   ├── display/         # Framebuffer display
│   │   ├── virtio/          # VirtIO devices
│   │   └── ...              # PS/2, IDE, PCI
│   ├── fs/                  # Filesystem (FAT, VFS)
│   ├── graphics/            # Graphics primitives, fonts
│   ├── input/               # Input processing pipeline
│   ├── mm/                  # Memory management
│   ├── window/              # Window system
│   └── commands/            # Shell commands
├── assets/                  # Fonts and images
├── docs/                    # Design documentation
├── build.sh                 # Build script
└── test.sh                  # Test runner
```

## Architecture

AgenticOS is a **modular monolithic kernel** - all code runs in kernel space (ring 0) but is organized into distinct modules. Key architectural decisions:

- **No standard library**: Uses `#![no_std]` with custom allocator
- **Framebuffer graphics**: Modern pixel-based display (not VGA text mode)
- **Lock-free input**: SPSC queue prevents interrupt handler blocking
- **VirtIO first**: Uses VirtIO tablet for seamless QEMU mouse, falls back to PS/2

## Documentation

- [`CLAUDE.md`](CLAUDE.md) - Detailed development guide
- [`IMPLEMENTATION_PLAN.md`](IMPLEMENTATION_PLAN.md) - Development roadmap
- [`docs/window_system_design.md`](docs/window_system_design.md) - Window system architecture
- [`docs/shell_window_integration.md`](docs/shell_window_integration.md) - Shell integration design

## Development

```bash
# Quick compile check
cargo check

# Format code
cargo fmt

# Run linter
cargo clippy
```

## Resources

This project draws inspiration and guidance from:

- [Writing an OS in Rust](https://os.phil-opp.com/) by Philipp Oppermann
- [OSDev Wiki](https://wiki.osdev.org/)
- [Intel Software Developer Manuals](https://www.intel.com/sdm)

## License

This project is for educational and experimental purposes.

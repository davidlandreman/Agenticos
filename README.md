# AgenticOS

A Rust-based operating system for x86-64 architecture, built from scratch with the goal of supporting agent-based computing.

## Current Status

AgenticOS boots into a GUI desktop with ring-3 zsh terminals. It has working memory management, writable overlay/data filesystems, preemptive process scheduling, a Linux static-musl ABI, graphics/input, and a basic IPv4 network stack.

### Implemented Features

- **GUI Desktop**: Boots directly into graphical mode with a blue desktop background
- **Window System**: Hierarchical window management with mouse support
- **Terminal**: Windowed terminals running static-musl zsh and BusyBox applets
- **POSIX threads**: musl pthread creation/join, per-thread TLS/TIDs,
  mutexes, condition variables, and detached cleanup through clone + futexes
- **Memory Management**: Virtual memory, demand paging, per-process address spaces, and heap allocation
- **Filesystem**: FAT12/16/32 plus a Linux-compatible writable ext2 `/data`, with persistent overlay writes
- **Input**: VirtIO tablet (seamless in QEMU) with PS/2 fallback
- **Graphics**: Framebuffer/retained composition, qualified VirGL acceleration,
  and a bounded OpenGL-style ring-3 client path
- **3D Game**: `GL Arena` runs as `GLGAME.ELF` inside a normal movable,
  clipped, resizable desktop window
- **Networking**: Modern VirtIO-net, DHCPv4, DHCP-backed DNS resolution, ICMP, UDP, TCP, Linux socket FDs, and BusyBox `ping`/`nc`/`nslookup`/HTTP `wget`
- **Cryptographic randomness**: Host-backed modern VirtIO RNG in QEMU with
  x86-64 RDRAND fallback, feeding `AT_RANDOM`, `getrandom(2)`,
  `/dev/urandom`, and network seeds
- **Text browser**: Static-musl Links 2.30 (`links` or `links2`) with interactive
  text/native-GUI IPv4 HTTP and HTTPS, DNS, TLS 1.2+, and strict certificate
  chain/hostname validation against a pinned Mozilla trust store

### Not Yet Implemented

- Fine-grained SMP scheduling, user TLB shootdown, and a general async runtime
- IPv6 and interrupt-driven network I/O
- Agent runtime

## Building

### Prerequisites

- Rust nightly toolchain (managed automatically via `rust-toolchain.toml`)
- QEMU for x86-64 (`qemu-system-x86_64`)
- e2fsprogs (`mke2fs` and `e2fsck`; `brew install e2fsprogs` on macOS)

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

# Boot without a NIC
AGENTICOS_NETWORK=off ./build.sh
```

`GL Arena` requires the qualified strict VirGL path. On a provisioned macOS
host, launch it with the pinned QEMU described in
`docs/macos-virgl-qualification.md`, then choose Start → Programs → GL Arena
or run `glgame` from zsh.

### Testing

```bash
# Run kernel tests
./test.sh

# Focused hermetic network coverage
./test.sh --skip-userland network network_userland
```

Interactive QEMU uses user-mode NAT and normally leases `10.0.2.15` with
gateway/host alias `10.0.2.2`; DHCP-provided DNS servers populate the managed
`/etc/resolv.conf`. Tests use `restrict=on` plus repository-owned local
forwarding endpoints and deterministic `/etc/hosts` aliases, so they cannot
reach the host LAN or public Internet.

### Host Folder Mount

Files placed in `host_share/` at the repo root are exposed inside the running OS at `/host`, read-only. This is the easiest way to stage fixtures, sample images, or seed config files without rebuilding the bundled BIOS image.

```bash
# Default: ./host_share/ is mounted at /host
./build.sh

# Override with any folder on disk
AGENTICOS_HOST_SHARE=/path/to/folder ./build.sh
```

Inside the guest:

```
> ls /host
HELLO.TXT  HOST.TXT
> cat /host/HELLO.TXT
Hello from the host!
```

**Caveats** (inherent to the QEMU vvfat mechanism this uses):

- The directory listing is **snapshotted at QEMU start**. Adding or removing a file on the host while the guest is running will not be reflected until the next boot. File contents do update live, but new files do not appear.
- `/host` is intentionally mounted read-only. Persistent writable storage lives at `/data`.
- `host_share/` is gitignored except for the seed files. **Do not drop secrets, `.env` files, or credentials there** — the guest has no kernel/user boundary, so anything in `/host` is fully readable to anything running in the OS.

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
│   │   └── ...              # PS/2, VirtIO block, PCI
│   ├── fs/                  # Filesystem (ext2, FAT, tmpfs, overlay, VFS)
│   ├── graphics/            # Graphics primitives, fonts
│   ├── input/               # Input processing pipeline
│   ├── mm/                  # Memory management
│   ├── net/                 # IPv4 stack and socket registry
│   ├── userland/            # Ring-3 Linux ABI and ELF process platform
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
- **Bounded IPv4 stack**: Polling VirtIO-net and smoltcp behind an AgenticOS-owned Linux socket ABI

## Parallel Development with Conductor

This repo is wired up for [conductor.build](https://www.conductor.build) so you can run multiple branches in parallel without build artifacts or QEMU instances colliding. The compound-engineering plugin (`/ce-plan`, `/ce-work`, `/ce-code-review`, …) is enabled in every workspace by default.

See [`docs/conductor-workflow.md`](docs/conductor-workflow.md) for setup, isolation guarantees, and how to extend the configuration.

## Documentation

- [`CLAUDE.md`](CLAUDE.md) - Detailed development guide
- [`IMPLEMENTATION_PLAN.md`](IMPLEMENTATION_PLAN.md) - Development roadmap
- [`docs/conductor-workflow.md`](docs/conductor-workflow.md) - Parallel development with Conductor
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

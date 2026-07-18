# `src/drivers/` — Hardware Drivers

PCI bus, VirtIO block storage, PS/2 keyboard and mouse, VirtIO input/network/GPU, and the framebuffer display driver.

## Key files

- `pci.rs` — PCI bus enumeration, configuration space access, BAR reading.
- `block.rs` — `BlockDevice` trait used by `src/fs/`.
- `ps2_controller.rs` — shared PS/2 controller setup; enables IRQ1 (keyboard) and IRQ12 (mouse).
- `src/input/keyboard_driver.rs` — PS/2 keyboard state machine (scancode set 2).
- `mouse.rs` — PS/2 mouse driver, fallback when VirtIO tablet is absent.
- `mouse_old.rs` — legacy mouse code; **triage before relying on it**. If dead, remove in a separate PR; if intentional, document why.
- `virtio/mod.rs` — VirtIO module init.
- `virtio/common.rs` — modern VirtIO PCI feature negotiation, page-safe DMA storage, tokenized queue ownership, and descriptor chains.
- `virtio/block.rs` — interrupt-driven VirtIO-blk. Requests retain owned DMA bounce pages, split transfers at 128 sectors, and wake exact kernel/ring-3 waiters from PCI INTx completion.
- `virtio/gpu/` — VirtIO 1.3 GPU wire layouts, checked control/cursor queues,
  guest-backed 2D resources, display discovery/events, exact damage transfer +
  flush, scanout lifecycle, and the VirGL transport for capsets, contexts, 3D
  resources/transfers, submissions, fences, and deterministic TGSI command
  encoding. The production VirGL engine owns the device exclusively and opens
  only after clear, alpha/readback, and repeated-lifecycle gates pass.
- `virtio/input.rs` — VirtIO tablet (absolute pointing, seamless mouse in QEMU).
- `virtio/net.rs` — polling modern VirtIO-net device, bounded RX/TX DMA pools, and smoltcp Ethernet adapter.
- `virtio/p9.rs` — polling modern virtio-9p transport (device type 9, ID
  `0x1049`). Carries whole 9P2000.L messages for the `/shared` client in
  `src/fs/p9/`; identity is the config-space `mount_tag` (`agenticos-shared`),
  read under the config-generation loop. One request in flight, serialized by
  the client's lock; timeout/malformed completions quarantine the channel.
- `virtio/rng.rs` — polling modern VirtIO entropy device. Completion waits are
  finite; a timed-out or malformed queue is quarantined while its DMA storage
  remains owned by the driver.
- `display/` — framebuffer driver. `display.rs` controls single/double buffering (the `USE_DOUBLE_BUFFER` flag lives here even though graphics primitives live in `src/graphics/`). `frame_buffer.rs` is the low-level abstraction; `text_buffer.rs` and `double_buffered_text.rs` handle text rendering; `double_buffer.rs` provides the 8 MiB static back buffer.

## VirtIO block completion (load-bearing)

Block requests use modern device ID `0x1042`, require `VIRTIO_F_VERSION_1`, and identify root/host/data disks through `VIRTIO_BLK_T_GET_ID`. Each descriptor chain is header + DMA data pages + status byte. The request owns every page until its used-ring entry arrives; callers never expose stack or user virtual addresses to DMA.

The shared PCI INTx handler reads the VirtIO ISR, reclaims every used descriptor, and wakes the request's exact waiter. Ring-3 storage waits save both the in-progress kernel continuation and the live FS_BASE/FPU image so partially completed syscalls and page faults resume without re-firing or corrupting user SSE state. Any inline wait that can become `KERNEL_CONTEXT` must return from `hlt` with IF enabled; otherwise the eventual ring-3 exit restores an interrupt-disabled kernel caller. Early boot waits with `hlt`. Do not hold ordinary spin locks or the memory-mapper lock across a `BlockDevice` call.

## Mouse input-method selection

During boot, the kernel:

1. Scans the PCI bus for a VirtIO tablet device.
2. **If present**: initializes the VirtIO tablet via `init_with_screen()`, which scales tablet coordinates to the screen resolution. Provides absolute positioning — seamless cursor movement between QEMU host and guest.
3. **If absent**: falls back to PS/2 mouse on IRQ12 (relative positioning; QEMU grabs the mouse).

The VirtIO tablet requires `-device virtio-tablet-pci` in the QEMU command line. `./build.sh` includes this flag.

## VirtIO-GPU selection

Modern GPU PCI device type 16 is discovered through cached PCI enumeration.
Only explicitly understood features are negotiated. `scripts/qemu-compositor.sh`
probes the exact `AGENTICOS_QEMU_BIN`: retained mode can request `virtio-vga`
for 2D scanout, while GL device names are used only for `gpu`/`auto`. Plain 2D
scanout is disabled by default on macOS because QEMU 11.0.1's Cocoa frontend
can show a black window for an otherwise valid scanout; set
`AGENTICOS_QEMU_2D=on` only for diagnostics. Presence of `virtio-vga-gl` alone
never makes the kernel report acceleration; the guest qualification gates and
successful `VirglCompositionEngine` construction do.

## VirtIO DMA and network invariants

Virtqueue submissions use physical addresses plus caller tokens; completion
returns the token only after validating the device-written descriptor ID and
length. Translation failure is an error—never substitute a virtual address.
Ring and packet DMA objects are page-aligned/page-contained and remain owned
until completion. Publish/consume boundaries use release/acquire fences.

The network driver accepts only modern device ID `0x1041`, requires
`VIRTIO_F_VERSION_1`, and optionally negotiates `VIRTIO_NET_F_MAC`. It does not
enable checksum/GSO, merged RX buffers, multiqueue, or a control queue. The
modern non-merged header is 12 bytes (including `num_buffers`), followed by an
Ethernet frame no larger than 1514 bytes. Malformed completions and pool
exhaustion increment counters and drop work rather than panic.

## PS/2 mouse packet format

Mouse driver reads 3-byte packets from the controller. **Bit 3 of byte 0 must always be set** (sync bit) — packets failing this check are dropped to recover from sync loss. The driver tracks position with screen-boundary clamping and exposes button state for left / right / middle.

## Cross-references

- Raw input events go to a lock-free queue consumed by `src/input/` — see `src/input/CLAUDE.md`.
- Mouse cursor *rendering* (background save/restore, cursor sprite) is owned by the window system, NOT here — see `src/window/CLAUDE.md`.
- Block storage feeds `src/fs/` — see `src/fs/CLAUDE.md`.
- VirtIO-net feeds the protocol and socket layer in `src/net/` — see `src/net/CLAUDE.md`.
- The `USE_DOUBLE_BUFFER` flag in `display/display.rs` is mentioned by `src/graphics/CLAUDE.md` because flipping it affects graphics behavior; the flag itself lives here.

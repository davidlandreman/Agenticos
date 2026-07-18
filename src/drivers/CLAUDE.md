# `src/drivers/` — Hardware Drivers

PCI bus, IDE/ATA storage, PS/2 keyboard and mouse, VirtIO devices (tablet for seamless mouse), and the framebuffer display driver.

## Key files

- `pci.rs` — PCI bus enumeration, configuration space access, BAR reading.
- `ide.rs` — IDE/ATA PIO-mode storage driver. Supports up to 4 drives.
- `block.rs` — `BlockDevice` trait used by `src/fs/`.
- `ps2_controller.rs` — shared PS/2 controller setup; enables IRQ1 (keyboard) and IRQ12 (mouse).
- `keyboard.rs` — PS/2 keyboard driver (scancode set 2).
- `mouse.rs` — PS/2 mouse driver, fallback when VirtIO tablet is absent.
- `mouse_old.rs` — legacy mouse code; **triage before relying on it**. If dead, remove in a separate PR; if intentional, document why.
- `virtio/mod.rs` — VirtIO module init.
- `virtio/common.rs` — VirtIO device abstraction and Virtqueue.
- `virtio/gpu/` — VirtIO 1.3 GPU wire layouts, checked control/cursor queues,
  guest-backed 2D resources, display discovery/events, exact damage transfer +
  flush, and scanout lifecycle. This is a presenter, not an accelerated
  composition engine.
- `virtio/input.rs` — VirtIO tablet (absolute pointing, seamless mouse in QEMU).
- `display/` — framebuffer driver. `display.rs` controls single/double buffering (the `USE_DOUBLE_BUFFER` flag lives here even though graphics primitives live in `src/graphics/`). `frame_buffer.rs` is the low-level abstraction; `text_buffer.rs` and `double_buffered_text.rs` handle text rendering; `double_buffer.rs` provides the 8 MiB static back buffer.

## IDE PIO atomicity (load-bearing)

`IdeController::read_sectors` and `write_sectors` disable interrupts (via `InterruptGuard::disable()`) for the entire transaction — drive-select writes, `wait_ready`, `wait_drq`, the 256-word data-port loop, and the trailing `wait_ready`. PIO requires the CPU to read the data port immediately when DRQ becomes set; if the scheduler preempts the caller mid-read, the DRQ window slips and subsequent `wait_drq` calls time out (status `0x58` = DRDY|DSC|DRQ — set just after we gave up spinning).

This matters under interactive boot, not test mode. Test mode has no GUIShell competing for slices, so the run process completes IDE reads uninterrupted. Interactive boot has GUIShell + compositor running between every preemption tick; without the IRQ-disable, multi-MiB FAT reads timeout consistently. RAII guard ensures IRQs are restored on every exit path including `?`-propagated errors.

The transaction is bounded (at most 128 sectors / 64 KiB per call), so the IRQ-disabled window is small enough to be acceptable for the rest of the system.

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
never makes the kernel report acceleration.

## PS/2 mouse packet format

Mouse driver reads 3-byte packets from the controller. **Bit 3 of byte 0 must always be set** (sync bit) — packets failing this check are dropped to recover from sync loss. The driver tracks position with screen-boundary clamping and exposes button state for left / right / middle.

## Cross-references

- Raw input events go to a lock-free queue consumed by `src/input/` — see `src/input/CLAUDE.md`.
- Mouse cursor *rendering* (background save/restore, cursor sprite) is owned by the window system, NOT here — see `src/window/CLAUDE.md`.
- Block storage feeds `src/fs/` — see `src/fs/CLAUDE.md`.
- The `USE_DOUBLE_BUFFER` flag in `display/display.rs` is mentioned by `src/graphics/CLAUDE.md` because flipping it affects graphics behavior; the flag itself lives here.

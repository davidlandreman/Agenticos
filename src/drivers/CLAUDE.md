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
- `virtio/input.rs` — VirtIO tablet (absolute pointing, seamless mouse in QEMU).
- `display/` — framebuffer driver. `display.rs` controls single/double buffering (the `USE_DOUBLE_BUFFER` flag lives here even though graphics primitives live in `src/graphics/`). `frame_buffer.rs` is the low-level abstraction; `text_buffer.rs` and `double_buffered_text.rs` handle text rendering; `double_buffer.rs` provides the 8 MiB static back buffer.

## Mouse input-method selection

During boot, the kernel:

1. Scans the PCI bus for a VirtIO tablet device.
2. **If present**: initializes the VirtIO tablet via `init_with_screen()`, which scales tablet coordinates to the screen resolution. Provides absolute positioning — seamless cursor movement between QEMU host and guest.
3. **If absent**: falls back to PS/2 mouse on IRQ12 (relative positioning; QEMU grabs the mouse).

The VirtIO tablet requires `-device virtio-tablet-pci` in the QEMU command line. `./build.sh` includes this flag.

## PS/2 mouse packet format

Mouse driver reads 3-byte packets from the controller. **Bit 3 of byte 0 must always be set** (sync bit) — packets failing this check are dropped to recover from sync loss. The driver tracks position with screen-boundary clamping and exposes button state for left / right / middle.

## Cross-references

- Raw input events go to a lock-free queue consumed by `src/input/` — see `src/input/CLAUDE.md`.
- Mouse cursor *rendering* (background save/restore, cursor sprite) is owned by the window system, NOT here — see `src/window/CLAUDE.md`.
- Block storage feeds `src/fs/` — see `src/fs/CLAUDE.md`.
- The `USE_DOUBLE_BUFFER` flag in `display/display.rs` is mentioned by `src/graphics/CLAUDE.md` because flipping it affects graphics behavior; the flag itself lives here.

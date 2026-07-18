---
title: "feat: add a Rust AgenticOS GUI driver to Links2"
type: feat
status: active
date: 2026-07-18
depth: large
related_docs:
  - docs/plans/2026-07-18-007-feat-links2-userland-browser-plan.md
  - docs/plans/2026-07-18-001-feat-ring3-gui-platform-notepad-and-userland-unification-plan.md
  - docs/plans/2026-07-18-006-feat-cryptographic-entropy-and-random-interfaces-plan.md
  - src/userland/CLAUDE.md
  - src/window/CLAUDE.md
  - userland/apps/links2/README.md
  - userland/README.md
---

# feat: add a Rust AgenticOS GUI driver to Links2

## Summary

Turn the shipped Links 2.30 text browser into an optional native AgenticOS GUI
browser by implementing a new Links graphics driver whose platform logic is
written in Rust. Links remains the browser engine and continues to own HTML
layout, graphical browser chrome, menus, forms, history, bookmarks, downloads,
networking, and image composition. A deliberately thin C adapter exposes the
exact `struct graphics_driver` vtable expected by the pinned upstream source;
the Rust library owns the XRGB8888 backing surface, clipping, bitmap blits,
scrolling, GUI syscalls, input translation, dirty tracking, and presentation.

The same static, non-PIE `/host/LINKS.ELF` supports both modes:

```sh
links http://agenticos-http.test:8081/                # existing text mode
links2 -g http://agenticos-http.test:8081/            # native GUI mode
links2 -g -driver agenticos http://10.0.2.101:8081/   # explicit driver
```

Start -> Programs -> Web Browser launches the explicit graphics-mode argv and
does not need a terminal. Existing shell behavior remains compatible: `links`
without `-g` is still text mode, and zsh waits for a shell-launched GUI browser
until its window closes.

This plan does not turn Links into a modern Chromium-class browser. The result
is upstream Links' graphical browser in an ordinary AgenticOS window, still
IPv4/HTTP-only and without JavaScript or TLS. PNG, GIF, XBM, the built-in Links
font, mouse input, keyboard navigation, forms, resize, title updates, and gzip
HTTP content are in scope.

Upstream references:

- [Links 2.30 download and graphics build instructions](https://links.twibright.com/download.php)
- [Links user documentation](https://links.twibright.com/user_en.html)
- [Links graphics-driver development documentation](https://links.twibright.com/doc/links_doc_en.html)
- [zlib 1.3.2 release and checksum](https://www.zlib.net/)
- [libpng 1.6.58 release and checksums](https://www.libpng.org/pub/png/libpng.html)

---

## Current state and feasibility findings

### Existing AgenticOS pieces

- `LINKS.ELF` is already a committed, prebuilt-managed, static-musl Links 2.30
  binary. `/bin/links` and `/bin/links2`, HTTP, DNS, `select(2)`, nonblocking
  helper pipes, writable `/root/.links`, and deterministic QEMU coverage have
  landed.
- Ring-3 GUI syscalls create framed windows, copy-present full XRGB8888 client
  surfaces, deliver keyboard/mouse/resize/close/focus events, update titles,
  and clean up every window owned by an exiting PID.
- The compositor already handles ordinary ring-3 CPU surfaces under both the
  Classic and Aero server-side frame themes. The browser does not need VirGL,
  a framebuffer device, X11, SDL, or a second window system.
- The Links process already idles in a correct mixed-fd `select(2)` loop. Its
  driver can register a readable GUI-event fd with Links' existing
  `set_handlers` mechanism once the kernel provides such a descriptor.

### Confirmed upstream driver contract

The pinned Links source defines a stable, direct `struct graphics_driver`
callback table. The relevant callbacks cover:

- driver/device initialization and shutdown;
- bitmap allocate/register/update/unregister/draw;
- color conversion, filled rectangles, horizontal/vertical lines, clipping,
  and scrolling;
- flush, window-title update, and optional clipboard/shell hooks;
- device callbacks back into Links for redraw, resize, keyboard, mouse, and
  close behavior.

Links' window-system drivers keep the browser engine single-threaded. They
register their display descriptor with Links' central selector and invoke the
device's callback handlers while draining events. The AgenticOS driver should
follow that model instead of introducing a Rust executor or a second event
loop.

For a 32-bit little-endian buffer whose bytes are `B, G, R, 0`, Links' depth
code is `4 | (24 << 3) == 196`. That is the exact in-memory representation of
AgenticOS GUI ABI v1's `u32 0x00RRGGBB`, so neither solid colors nor decoded
bitmaps need a channel-swizzle pass.

### Confirmed platform gaps

1. **GUI events are not selectable.** `gui_next_event` either returns one
   event or blocks the whole process. Links must wait on GUI events, DNS/helper
   pipes, timers, and network sockets together, so a timer-poll workaround
   would either add arbitrary input latency or wake the browser continuously.
2. **Graphics-mode Links requires libpng.** Its internal fallback font is
   embedded as PNG glyph data, and upstream configure rejects graphics mode
   without libpng even if page images are disabled. The current text build has
   no image-library toolchain.
3. **Links draws incrementally; AgenticOS presents whole surfaces.** A naive
   syscall per primitive would be unusable. The driver needs a persistent CPU
   back buffer, dirty tracking, and one coalesced present at Links' flush
   boundary. Partial-present ABI work should be justified by measurements,
   not made a prerequisite.
4. **The existing Rust runtime cannot be linked wholesale into musl.** It owns
   a `brk` allocator intended for standalone Rust ELFs. Running that allocator
   beside musl malloc in one process would create two independent owners of
   the process break. The driver must be `no_std`/no-`alloc` and use exactly
   one allocator, through C allocation functions supplied by the musl/Links
   side.

### Mixed-language build proof

A planning spike built a `#![no_std]`, panic-abort Rust `staticlib` for
`x86_64-unknown-none` with the repository's pinned nightly and linked it into a
static-musl, non-PIE x86-64 `ET_EXEC`. The link completed without duplicate
runtime or compiler-builtin symbols. The spike is under gitignored `.context`
and is not part of the deliverable.

This proves the toolchain shape, not the production ABI. The implementation
still needs an explicit C/Rust header, layout assertions, undefined-symbol
checks, and an AgenticOS execution test.

---

## Goals

1. Add an upstream-style `agenticos` graphics driver to the pinned Links 2.30
   build while keeping the browser core and graphical UX upstream-owned.
2. Put platform behavior in a `no_std`, no-`alloc` Rust static library and
   keep the C adapter limited to Links struct construction, vtable
   registration, and calls through Links-owned handler pointers.
3. Add a pollable, per-process GUI-event descriptor that works with `read`,
   `select`, `poll`, dup/close, process cleanup, and Links' existing event loop
   without idle polling.
4. Render into one resizable XRGB8888 CPU surface with correct clipping,
   overlap-safe scrolling, decoded bitmap blits, title updates, batched flush,
   and deterministic failure handling.
5. Translate AgenticOS key, modifier, pointer, button, drag, wheel, resize,
   focus, theme, and close events into Links' graphics-device callbacks.
6. Reproducibly cross-build pinned zlib and libpng dependencies, enable Links'
   built-in graphical font plus PNG/GIF/XBM decoding and gzip HTTP content,
   then refresh the committed `LINKS.ELF`.
7. Add a first-class Web Browser Start-menu launch while preserving text-mode
   command behavior.
8. Prove graphics startup, rendering, navigation, forms, images, resize,
   downloads, idle blocking, cleanup, and text-mode regressions in bounded
   tests.

## Non-goals

- HTTPS/TLS, CA roots, hostname verification, or trusted-time policy. Entropy
  exists, but the TLS trust stack remains separate work.
- JavaScript, Chromium/WebKit compatibility, new CSS features, media, audio,
  WebSockets, or browser extensions.
- IPv6, AF_UNIX single-instance support, pthreads, or libevent.
- X11, Wayland, SDL, `/dev/fb0`, direct VirtIO-GPU access, or a VirGL client
  surface. This is a normal CPU-backed ring-3 GUI window.
- Replacing Links' own menus, toolbar, dialogs, form widgets, or page layout
  with `userland/libs/gui` widgets. Mixing two widget/event models in one
  content surface would create conflicting focus, clipping, and redraw rules.
- Multiple browser windows from one Links process in v1. Links receives
  `GD_ONLY_1_WINDOW`; tabs/new-window support can be reconsidered after the
  single-device lifecycle is proven.
- System clipboard integration, drag-and-drop, OS-wide URL handlers, desktop
  shortcuts, or native common file dialogs.
- FreeType/fontconfig or host-installed fonts. The first GUI build uses Links'
  pinned built-in font so a stock checkout is hermetic.
- JPEG, TIFF, WebP, AVIF, SVG-via-librsvg, Brotli, Zstd, bzip2, or LZMA in the
  first GUI merge. PNG/GIF/XBM are enough to prove the graphics path; extra
  decoders should be small, separately reviewable follow-ups.
- A partial-present syscall in the initial correctness path. Add one only if
  measured full-surface copy/upload cost misses the performance acceptance
  bar after flush coalescing.

---

## Architecture decisions

### AD1 — Links remains the engine and UX owner

Do not wrap text-mode output in a native toolbar or reimplement HTML layout in
Rust. Build Links with graphics enabled and implement the driver interface it
already uses on X, DirectFB, and other window systems. This retains upstream
navigation, forms, history, bookmarks, downloads, image caching, graphical
menus, and redraw logic while minimizing the fork from 2.30.

The AgenticOS server remains responsible for the top-level frame, taskbar
entry, focus, move, resize, close request, and active Classic/Aero decoration.
Links paints everything inside the content well.

### AD2 — use a narrow C vtable adapter around Rust platform logic

`struct graphics_driver` and `struct graphics_device` are upstream C layouts
with function pointers that Links initializes after device creation. Define
the global `agenticos_driver` and allocate/free those Links structs in a small
`agenticos.c` adapter compiled with the pinned source. Do not mirror the full
driver struct in Rust.

The adapter may:

- construct the vtable and `graphics_device`;
- read/write the public `size`, `clip`, `driver_data`, and handler fields;
- register/unregister the GUI fd with Links `set_handlers`;
- translate a Rust-returned neutral event into one call to the appropriate
  Links handler;
- use Links allocation/error helpers where their ownership is required.

All surface manipulation, bitmap memory, dirty state, GUI syscall calls, and
AgenticOS event decoding live in Rust. Keep the exported C ABI small and use
opaque Rust-owned handles rather than sharing internal Rust structs.

### AD3 — use one allocator: musl/Links allocation reached through C ABI

The Rust driver is `#![no_std]`, does not import `alloc`, and has no global
allocator. It calls C functions for checked `malloc`/`calloc`/`realloc`/`free`
or asks the adapter to allocate Links-owned objects. Rust performs every
dimension/stride/byte-length overflow check before calling an allocator or
forming a slice.

The Rust `staticlib` supplies a panic handler that exits the browser process
through a minimal syscall or calls a C fatal hook. It must never spin forever
with a live GUI window. Normal allocation failure returns a status that Links
can handle or closes only this process; it never panics in a draw callback.

### AD4 — add a selectable GUI-event fd, not a timer poll

Add private syscall 5011:

```text
gui_event_open(flags) -> fd | -errno
```

Accepted flags are `O_NONBLOCK | O_CLOEXEC`. The returned descriptor is bound
to the creating PID's existing bounded GUI queue. `read(fd, ...)` returns one
or more whole 32-byte `GuiEvent` records, never a partial record. A buffer
smaller than one event is `EINVAL`; an empty nonblocking read is `EAGAIN`;
blocking read reuses `WaitingForGuiEvent`.

`select`/`poll` report readable exactly when the bound queue is nonempty.
Enqueue wakes both a legacy process blocked directly in `gui_next_event` and
the same PID when it is parked in the shared mixed-fd wait used by
`select`/`poll`. Reading through the fd and calling `gui_next_event` consume
the same queue; applications choose one interface and must not mix consumers.

Represent the slot explicitly as `FdSlot::GuiEvents { owner_pid, ... }` (or a
small shared open-file handle if status flags require it). Audit every
exhaustive fd match: `read`, `write`, `fstat`, `fcntl`, `lseek`, `select`,
`poll`, `/proc/<pid>/fd` naming, dup, close, exec, fork, and teardown. A fork
child must not gain authority to consume its parent's window events; either
omit PID-bound GUI descriptors from the child's fd-table clone or make all
operations except close fail ownership validation. Test the chosen rule.

The existing syscall 5003 and native Rust GUI apps remain source- and
behavior-compatible.

### AD5 — use one software XRGB surface and coalesce presents

Each v1 device owns:

- one AgenticOS window handle;
- one GUI-event fd;
- width, height, stride, and a checked musl-allocated XRGB8888 pixel buffer;
- current Links clip rectangle;
- previous pointer-button state for transition decoding;
- dirty bounds plus a `present_pending` flag;
- counters for draw calls, flushes, full-present bytes, and failures in debug
  or render-stat builds.

All Links drawing callbacks modify this buffer. `fill_area`, lines,
`draw_bitmap`, and `scroll` clip before pointer arithmetic and union their
destination into the dirty bounds. The driver's `flush` performs at most one
`gui_win_present` for the current surface, then clears dirty state only after
success.

Links already calls `flush` at text/image and terminal redraw boundaries. If
some primitive paths require deferred batching, register one Links bottom
half, like its X/DirectFB drivers, rather than presenting from every callback.
No separate thread is introduced.

The initial kernel syscall still copies a full surface. Measure 640x480,
800x600, and 1024x768 page loads, scrolling, and resize. If p95 input-to-frame
latency or copy/upload traffic is unacceptable, follow with a versioned
damage-present syscall whose bounds are validated against the current content
surface. Do not silently change syscall 5002 semantics.

### AD6 — use native Links bitmap layout without conversion

Advertise Links depth 196 and four-byte rows. `get_empty_bitmap` allocates a
checked `width * 4 * height` buffer and fills `skip = width * 4`.
`register_bitmap`/`commit_strip` are no-ops for a client-memory bitmap;
`prepare_strip` returns the checked row pointer; `unregister_bitmap` frees it.

`draw_bitmap` copies only the clipped source rectangle into the device
surface. It must handle negative destinations, partial right/bottom overlap,
large/malformed dimensions, and arbitrary valid `skip`. Scrolling uses
direction-aware row order or `memmove` so source/destination overlap is safe.
It returns the Links result that requests redraw of newly exposed regions.

### AD7 — pin and privately build the minimum image stack

Build dependencies beneath `userland/apps/links2/build/deps`; never discover
host Homebrew libraries or trust host `pkg-config` output.

```text
zlib 1.3.2
  https://zlib.net/fossils/zlib-1.3.2.tar.gz
  SHA256 bb329a0a2cd0274d05519d61c667c062e06990d72e125ee2dfa8de64f0119d16

libpng 1.6.58
  https://download.sourceforge.net/libpng/libpng-1.6.58.tar.xz
  SHA256 28eb403f51f0f7405249132cecfe82ea5c0ef97f1b32c5a65828814ae0d34775
```

Cross-build static archives with the same musl toolchain, install into a
private prefix, and pass explicit include/library paths to Links configure.
Disable FreeType and every optional decoder named in Non-goals. Enable zlib
HTTP compression because zlib is already a required libpng dependency, and
add deterministic gzip-response coverage.

The dependency hashes and versions live in the Links Makefile beside the
existing Links archive pin. `distclean` removes extracted dependency sources
and outputs but never committed prebuilts.

### AD8 — preserve text mode and make GUI launch explicit

The rebuilt executable has graphics compiled in, but upstream `-g` remains the
mode switch. With `agenticos` as the only compiled graphics driver,
`links2 -g` auto-selects it; `-driver agenticos` stays supported and is used in
tests and Start-menu argv so intent is unambiguous.

Add `Web Browser` to the Programs fly-out with a repository-owned SVG icon.
The launch request is:

```text
path: /host/LINKS.ELF
argv: ["links2", "-g", "-driver", "agenticos", "-no-connect"]
env:  DEFAULT_USER_ENV
cwd:  /host
terminal_id: None
```

`-no-connect` skips the optional AF_UNIX single-instance rendezvous that
AgenticOS does not implement. A shell-launched instance may use the same flag
manually; neither path requires a Links-specific environment variable.

### AD9 — keep the HTTP-only boundary visible in the GUI

Compiling graphics and zlib must not accidentally enable SSL autodetection.
Continue passing `--without-ssl`, assert the configure summary and linked
symbols, and test that an `https://` URL produces a bounded unsupported/error
path rather than an insecure connection.

Documentation and the Start-menu description should call the application
`Web Browser (HTTP)` where space permits. The feature is complete without
claiming general public-web compatibility.

---

## Runtime flow

```text
Links 2.30 C browser engine
  HTML/layout/UI/image cache/network/select
               |
               | struct graphics_driver callbacks
               v
thin agenticos.c adapter
  Links structs + handler calls + set_handlers(gui_fd)
               |
               | narrow extern "C" ABI
               v
no_std/no-alloc Rust driver staticlib
  XRGB surface + raster/clip/scroll + input map + dirty/flush
               |
               | syscalls 5001/5002/5004/5005/5011 + read(gui_fd)
               v
AgenticOS ring-3 GUI platform
  PID event queue + RemoteSurface + framed compositor window
```

Event path:

```text
window manager event
  -> enqueue GuiEvent for browser PID
  -> gui fd becomes readable and wakes Links select(2)
  -> agenticos.c fd callback asks Rust to drain/map events
  -> adapter invokes dev->{keyboard,mouse,resize,redraw}_handler
  -> Links mutates browser state and draws through Rust callbacks
  -> coalesced driver flush -> one gui_win_present
```

Resize ordering is important:

1. validate the new nonzero dimensions and total bytes;
2. allocate a complete replacement surface before releasing the old one;
3. on success, update Rust state and the C `graphics_device.size/clip`;
4. call Links' resize handler, which redraws for the new viewport;
5. flush the complete new frame;
6. on allocation failure, preserve the old valid allocation and request a
   clean browser close/error instead of publishing mismatched dimensions.

---

## C/Rust ABI contract

Add one checked header under the Links app source, owned by this repository.
Use fixed-width integers, explicit return codes, and opaque pointers. Avoid C
`long`, Rust `usize`, Rust enums, bool layout, or ownership of a pointer that
is not stated in the header.

A representative boundary is:

```c
struct agui_context;

struct agui_mapped_event {
    uint32_t kind;
    int32_t x;
    int32_t y;
    int32_t code;
    uint32_t modifiers;
    uint32_t buttons;
    int32_t wheel_x;
    int32_t wheel_y;
};

int agui_driver_init(void);
struct agui_context *agui_device_create(uint32_t width, uint32_t height,
                                        const uint8_t *title, size_t title_len);
void agui_device_destroy(struct agui_context *ctx);
int agui_event_fd(const struct agui_context *ctx);
int agui_next_mapped_event(struct agui_context *ctx,
                           struct agui_mapped_event *event);
int agui_resize(struct agui_context *ctx, uint32_t width, uint32_t height);
int agui_flush(struct agui_context *ctx);
```

The final set may group raster parameters differently, but keep these
invariants:

- Rust never dereferences a `graphics_device` or calls a Links function
  pointer.
- C never dereferences Rust driver state.
- every shared struct has C `_Static_assert` and Rust size/alignment/offset
  assertions;
- return values distinguish success, queue drained, recoverable allocation
  failure, invalid input, and fatal GUI ownership loss;
- title bytes are UTF-8 and length-bounded; no implicit NUL scan crosses FFI;
- bitmap and device allocations have one documented allocator/free owner;
- exported Rust symbols use `#[no_mangle] extern "C"` and unwind is impossible.

---

## Input mapping

### Keyboard

Process key-down events only. Positive Unicode/ASCII payload characters pass
through when present. Map AgenticOS special key codes to Links `KBD_*` values:
Enter, Backspace, Tab, Escape, arrows, Insert/Delete, Home/End, Page Up/Down,
F1-F12, and close. Map modifier bits to `KBD_SHIFT`, `KBD_CTRL`, and `KBD_ALT`;
Meta remains unsupported. Preserve `Ctrl-C` as Links input in graphics mode,
not terminal SIGINT.

Unit-test every mapping and unknown/release behavior. The current AgenticOS
GUI key event only carries the character derived from keycode + Shift, so full
IME/composed-Unicode input is not promised by this plan.

### Pointer

Links needs the changed button on both press and release, while AgenticOS
events contain the current button mask. Track the previous left/right/middle
mask in the Rust context and compute transitions:

- down: `current & !previous`;
- up: `previous & !current`;
- motion with a held button: `B_DRAG` for the deterministic priority
  left -> middle -> right;
- motion without a held button: `B_MOVE`;
- vertical/horizontal wheel deltas: the corresponding Links wheel codes.

Clamp coordinates to the current content size before calling Links. Preserve
all wheel steps up to a small per-event cap so a malformed delta cannot loop
unboundedly. Motion remains coalescible in the kernel queue.

### Window events

- resize follows the replacement-surface ordering above;
- close maps to `KBD_CLOSE` and lets Links perform normal shutdown/config save;
- focus changes update driver state; do not redraw unless Links needs it;
- theme/settings broadcasts trigger at most one full redraw. Server-side frame
  theme changes are already live; the Links-owned content skin remains
  upstream and is not recolored to imitate AgenticOS controls.

---

## Implementation units

### M0 — lock the mixed build and upstream patch surface

**Goal:** Replace planning assumptions with a reproducible local prototype
before changing a public kernel ABI.

**Work:**

1. Add the production Rust staticlib skeleton and compile it with the pinned
   nightly for `x86_64-unknown-none`, panic abort, `core` +
   `compiler_builtins`, no `alloc`, and no standalone linker script.
2. Link a tiny C smoke object plus the Rust archive with
   `x86_64-linux-musl-gcc -static -no-pie`; assert x86-64 `ET_EXEC` and no
   dynamic interpreter.
3. Extract the pinned Links source and record the exact 2.30 layouts/constants
   used by the adapter. Add compile-time layout/value assertions near the C
   shim rather than copying undocumented offsets into Rust.
4. Prove an `agenticos` stub driver is listed by `links -g -driver help` (or
   the equivalent error listing) and reaches init without X/SDL/framebuffer
   libraries.
5. Run `nm -u`/`readelf` checks on the archive and final ELF. Reject pthread,
   X11, dynamic-loader, or Rust allocator/runtime surprises.

**Exit bar:** a static graphics-enabled Links binary selects the stub
`agenticos` driver and fails only at the intentionally unimplemented window
creation boundary.

### M1 — add GUI event descriptors

**Likely files:**

- `src/userland/abi.rs`
- `src/userland/gui.rs`
- `src/userland/gui_syscalls.rs`
- `src/userland/fdtable.rs`
- `src/userland/syscalls.rs`
- `src/userland/lifecycle.rs`
- `userland/runtime/src/lib.rs`
- `src/tests/gui_userland.rs`
- `src/userland/CLAUDE.md`

**Work:**

- dispatch syscall 5011 and mirror its constant in the small userland ABI;
- allocate the PID-bound fd atomically or return `EMFILE` without leaked
  state;
- implement whole-record `read`, nonblocking/blocking behavior, readiness,
  wakeup, fstat/fcntl/dup/close/fork/exec policy, proc-fd naming, and cleanup;
- keep queue capacity and mouse-motion coalescing unchanged;
- share queue consumption between syscall 5003 and the fd without duplicating
  events;
- document lock order: never hold the GUI-state lock while acquiring the
  process table or window manager; compute the targeted wake after releasing
  GUI state.

**Tests:**

- empty fd is not readable; one enqueued event becomes readable;
- short read is `EINVAL`, bad pointer is `EFAULT`, nonblocking empty read is
  `EAGAIN`, and a valid read returns exactly the expected 32 bytes;
- multiple queued events drain in order and readability clears at empty;
- motion coalescing and queue overflow behavior match syscall 5003;
- `select` over a GUI fd + pipe + socket wakes for each source independently;
- blocking GUI read wakes without spinning;
- dup observes one shared queue; close of one duplicate leaves another valid;
- the selected fork ownership rule prevents a DNS child from consuming parent
  GUI events;
- process fault/exit removes windows, queue state, and all descriptor state;
- existing native GUI apps using `gui_next_event` still pass unchanged.

**Exit bar:** a fixture process sleeps inside `select` indefinitely, wakes for
one synthetic window event, reads it, and returns to sleep with no PIT-rate
polling.

### M2 — implement and test the Rust software driver core

**New files (suggested):**

```text
userland/apps/links2/driver-rs/
  Cargo.toml
  .cargo/config.toml
  src/lib.rs
  src/abi.rs
  src/surface.rs
  src/input.rs
  src/tests.rs
userland/apps/links2/driver/
  agenticos_links_gui.h
  agenticos.c
```

If syscall wrappers are extracted from `runtime`, add a tiny no-allocation
`userland/libs/gui-abi` rlib and make `runtime` re-export it. Do not make the
driver depend on the allocator/startup portions of `runtime`.

**Work:**

- implement checked musl-backed context/bitmap allocation and destruction;
- implement depth-196 colors, clip normalization, fill, hline, vline, bitmap
  strip handling/blit, overlap-safe scroll, dirty union, and coalesced flush;
- wrap window create/present/destroy/title/event-open syscalls;
- map events into the neutral C ABI, including button transition state;
- make shutdown idempotent for partial init and ensure all failure paths close
  fds, destroy the window if owned, and free buffers exactly once;
- keep unsafe slice formation in small reviewed helpers with a stated
  allocation/length invariant.

**Host tests:**

- color values prove red/green/blue byte order;
- every primitive clips at all four edges and is a no-op for empty clips;
- bitmap blit handles negative origin, padded valid stride, and partial edges;
- scroll is correct for left/right/up/down and overlapping source/destination;
- dimension multiplication rejects zero, overflow, and the GUI maximum;
- dirty bounds union and successful/failed flush state transitions are exact;
- resize swaps only after allocation success and never double-frees;
- all key, modifier, button, drag, and wheel mappings are table-driven;
- malformed/unknown events are ignored without changing state.

**Exit bar:** host tests prove the raster/input core, and an AgenticOS fixture
creates a window, draws a known color test pattern through the Rust archive,
reads one key/mouse event through fd 5011, presents, and exits cleanly.

### M3 — register the Links driver and complete callbacks

**Files:**

- `userland/apps/links2/driver/agenticos.c`
- `userland/apps/links2/patches/0002-register-agenticos-driver.patch`
- `userland/apps/links2/Makefile`

**Work:**

- patch `drivers.c` to register `agenticos_driver` under
  `GRDRV_AGENTICOS`;
- patch the shipped configure/configure.in and Makefile templates just enough
  to support `--with-agenticos`, report the driver, compile `agenticos.c`, and
  avoid requiring any host graphics backend;
- fill every `graphics_driver` field explicitly and set depth 196 plus
  `GD_UNICODE_KEYS | GD_ONLY_1_WINDOW | GD_NO_OS_SHELL | GD_NO_LIBEVENT`;
- use no-op/NULL callbacks deliberately for palette, real colors, shell exec,
  clipboard, block/unblock, AF_UNIX name, and margins;
- create the device with a conservative default content size (800x600,
  clamped to the current display if a query is available) and accept Links'
  standard `WIDTHxHEIGHT` driver parameter;
- register the event fd with `set_handlers`, drain it to `EAGAIN`, invoke Links
  handlers outside Rust, and unregister before destroying the context;
- map `set_title` to syscall 5005 with the existing 256-byte kernel limit;
- make `after_fork` remove or invalidate inherited display handling in the DNS
  helper child without destroying the parent's window.

**Tests:**

- the driver appears exactly once and is the only auto-selectable graphics
  backend;
- init/shutdown, partial-init failure, and repeated 100-cycle device lifetime
  leave no GUI ownership or fd leaks;
- `-driver agenticos`, auto-selection under `-g`, invalid mode, and text mode
  choose the expected paths;
- every vtable callback is non-NULL only when its contract is implemented.

**Exit bar:** `links2 -g -driver agenticos file:///host/<fixture>.html` opens a
framed, resizable window, draws Links' own chrome and built-in font, responds
to keyboard/mouse input, and closes normally.

### M4 — build the pinned image stack and refresh packaging

**Files:**

- `userland/apps/links2/Makefile`
- `userland/apps/links2/README.md`
- `userland/apps/links2/.gitignore`
- `userland/prebuilt/LINKS.ELF` (generated and committed)
- `userland/prebuilt/README.md`
- `userland/apps.manifest.sh` comments/toolchain metadata if needed
- `userland/refresh-prebuilt.sh` documentation if needed

**Work:**

- add hash-verified fetch/extract/build/install targets for zlib 1.3.2 and
  libpng 1.6.58;
- make dependency and driver sources inputs to the extracted/built stamps so a
  source change cannot reuse stale output;
- build Links with graphics/agenticos/zlib enabled and TLS/IPv6/libevent/GPM/
  FreeType/other decoders disabled;
- assert the configure summary includes only `AGENTICOS`, internal fonts,
  PNG/GIF/XBM, and zlib compression;
- build the Rust staticlib before final Links link with the repository-pinned
  nightly; normal stock builds still copy the committed ELF without any host
  toolchain;
- strip, validate, measure, and refresh `userland/prebuilt/LINKS.ELF` through
  the existing manifest flow;
- update toolchain documentation: source refresh now requires musl C tools,
  Rust nightly/rust-src, tar/xz, and ordinary host build utilities.

**Acceptance:**

- two clean rebuilds produce byte-identical or explained deterministic
  outputs;
- hashes fail closed and no host `/opt/homebrew` include/library path enters
  build logs or ELF strings;
- final ELF is x86-64 static `ET_EXEC`, below the loader's 16 MiB input cap,
  and has no interpreter/shared-library dependency;
- `links -dump` HTTP/DNS tests from the text-browser port still pass;
- gzip HTTP content decodes correctly;
- a page containing built-in text, PNG, GIF, and XBM paints expected distinct
  regions without decoder crashes.

### M5 — complete browser interaction and lifecycle behavior

**Fixture page:**

- deterministic title and colored background;
- headings, paragraphs, relative links, UTF-8 text within the current key/font
  capability, and a long scroll region;
- GET and POST forms with text, checkbox/radio, select, and submit controls;
- local PNG/GIF/XBM assets with known dimensions/colors;
- a gzip response, redirect, slow/chunked response, and downloadable payload;
- an HTTPS link that exercises the documented unsupported path.

**Automated QEMU coverage:**

1. Launch graphics mode against the repository-owned HTTP fixture.
2. Wait for one GUI window with a title derived from the page.
3. Verify the remote surface contains non-background pixels and known color
   probes for chrome/text/image regions (or a deterministic surface hash where
   stable).
4. Inject key and pointer events to activate a relative link; verify title or
   content changes.
5. Focus a form field, type, submit, and verify the fixture receives expected
   data.
6. Scroll with keyboard and wheel; verify content changes and no corruption.
7. Resize small -> large -> small and verify matching presents, full redraw,
   valid clipping, and bounded allocations.
8. Download to `/work`, verify bytes, and keep the GUI responsive during the
   slow response.
9. Deliver close, wait for exit 0, and assert GUI ownership/fds/memory return
   to baseline.
10. Repeat with `AGENTICOS_NETWORK=off`; show a bounded recoverable error and
    permit normal close.

Avoid screenshot-only assertions for core correctness. Expose a test-only
read-only pixel snapshot/probe on `RemoteSurface` if needed; do not add a
production window-readback syscall.

**Manual acceptance:**

- Start-menu launch opens one 800x600 browser without a terminal;
- shell text mode still behaves exactly as before;
- shell `links2 -g` opens the same GUI and zsh resumes after close;
- URL entry, links, Back/Forward, menus, search, forms, bookmarks, history,
  image display, and a `/work` download are usable;
- title bar, focus, dragging, resizing, taskbar activation, Classic/Aero frame
  change, and close behavior match other ring-3 apps;
- another terminal, Task Manager, and network activity remain responsive while
  a page loads or the browser is idle;
- config/bookmark changes beneath `/root/.links` reload after restart and
  after overlay `sync` when persistence is requested.

### M6 — add Web Browser launch integration

**Files:**

- `assets/icons/start/web-browser.svg` (new)
- `src/window/windows/start_menu.rs`
- `src/commands/guishell/mod.rs`
- related Start-menu/action tests

**Work:**

- add a typed `WebBrowser` action and pending action;
- extend the icon array and Programs model without index drift;
- generalize the direct GUI app helper to accept a fixed argv slice, or add a
  browser-specific launcher using `LaunchSpec`;
- launch the explicit argv in AD8 and report preparation/exit failures through
  the existing Start error/log path;
- update Programs popup width/height and hit-testing tests for the extra row.

Do not add `/bin/browser` or a second ELF. `/bin/links` and `/bin/links2`
remain the canonical command names.

### M7 — performance qualification, regression, and documentation

**Measure:**

- driver draw calls, dirty area, flushes, presents, and bytes per navigation;
- `gui_win_present` usercopy bytes and compositor surface upload bytes;
- p50/p95 event-to-present time during typing, link activation, scroll, and
  resize at 640x480, 800x600, and 1024x768;
- idle scheduler/Task Manager CPU behavior for a static page and an open menu;
- resident memory before page load, after image-heavy fixture, after cache
  clear, and after process exit;
- 100 open/load/close cycles and 20 resize cycles without fd/window/heap growth.

**Performance bar:**

- idle browser remains blocked when no fd/timer is ready; no periodic GUI poll;
- one Links flush causes at most one full-surface present;
- pointer/key interaction remains visibly responsive under QEMU while network
  and compositor workers run;
- no unbounded present queue, bitmap cache, GUI queue, or fd growth;
- if full presents dominate and miss the interaction bar, stop and implement a
  separately versioned damage-present follow-up before claiming performance,
  without destabilizing the correctness merge.

**Regression commands:**

```sh
cargo fmt --check
cargo check
./test.sh --skip-userland gui_userland userland
./test.sh --skip-userland network network_userland
./test.sh links2 network_userland
./test.sh --skip-userland
./build.sh -n
REBUILD_LINKS2=1 ./build.sh -n
```

Also run the Rust driver host tests and the C/Rust link smoke through a
documented Makefile target.

**Documentation:**

- root `CLAUDE.md` and `README.md` current state;
- `src/userland/CLAUDE.md` syscall 5011/fd semantics;
- `src/window/CLAUDE.md` browser use of `RemoteSurface` if behavior changes;
- `userland/README.md`, `userland/apps/links2/README.md`, and
  `userland/prebuilt/README.md` build/run/capability boundary;
- this plan's `status: implemented` only after the full done criteria pass.

---

## Dependency order

```text
M0 mixed Rust+C+Links proof
  +--> M1 selectable GUI-event fd
  +--> M2 Rust raster/input core
  +--> M4 zlib/libpng private build

M1 + M2
  -> M3 registered live Links driver

M3 + M4
  -> M5 browser integration/lifecycle
      -> M6 Start-menu launch
          -> M7 qualification/regression/docs
```

M1, M2, and dependency build work may proceed independently after M0 freezes
the ABI. Do not commit/advertise the refreshed graphics-capable prebuilt until
the event fd and live driver work; otherwise `-g` ships a selectable but
wedged mode.

---

## Required invariants

1. Links remains single-threaded in the parent; the existing fork helper is
   still used instead of pthreads.
2. Exactly one allocator owns C/Rust heap memory in `LINKS.ELF`; the standalone
   Rust userland allocator is never linked or initialized.
3. Rust never depends on the layout of `struct graphics_driver` or invokes a
   Links function pointer.
4. Every draw/scroll/resize multiplication and pointer range is checked before
   allocation or slice formation.
5. The device surface format is always little-endian XRGB8888/depth 196; no
   implicit host-format autodetection exists.
6. GUI-event readiness is edge-independent: as long as the queue is nonempty,
   the fd remains readable. No event can be stranded by a lost wake.
7. A fork helper child cannot read or destroy the parent's GUI window/event
   capability.
8. One flush produces zero presents when clean and at most one when dirty.
   Failed present keeps recoverable dirty state or terminates cleanly; it never
   marks unseen pixels clean.
9. Resize publishes no surface whose allocation dimensions disagree with the
   server-side content well.
10. Close/process fault frees the event fd, browser surface, registered
    bitmaps, Links device, and kernel GUI ownership exactly once.
11. Text mode, `-dump`, HTTP/DNS, config, and the committed-prebuilt workflow
    continue to work from a stock checkout.
12. Graphics/zlib work does not enable TLS, IPv6, JavaScript, host graphics
    libraries, or unpinned decoders.

---

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| GUI queue changes between readiness sample and park | Links sleeps forever with queued input | Use the existing restart/recheck blocking discipline, targeted wake after queue unlock, and a race-focused select test. |
| Rust and musl both manage `brk` | Heap corruption in normal browsing | No `alloc`/global allocator in the driver; all dynamic memory goes through one C allocator; inspect linked symbols. |
| C/Rust ABI drifts from the header | Corrupt events or opaque state | Fixed-width fields, opaque pointers, C/Rust layout assertions, no shared Links structs, link smoke in CI. |
| DNS fork child inherits GUI state | Child consumes input or destroys parent window | Explicit `after_fork` cleanup plus owner-bound fd semantics/fork test. |
| Full presents are too expensive under TCG | Laggy typing/scrolling and high upload traffic | Dirty/coalesced flush first, measure, then add a separate partial-present ABI only with evidence. |
| Resize allocation fails after server frame changes | Buffer/dimension mismatch or OOB write | Allocate replacement first; on failure retain old valid state and trigger clean browser error/close. |
| Wrong depth/channel order | Red/blue swap in UI and images | Depth 196 constant assertion and exact color-probe tests before browser integration. |
| Scroll copies overlap in the wrong direction | Page corruption while scrolling | Direction-aware/memmove implementation with four-direction overlap tests. |
| Button-up lacks an explicit changed-button field | Stuck selections/drag state | Track prior mask and derive transitions; test chorded buttons and coalesced motion. |
| Upstream configure silently finds host X/libpng | Non-reproducible or dynamic binary | Disable every host backend, private prefixes, no host pkg-config, configure-summary and ELF assertions. |
| Untrusted PNG triggers decoder defect | Browser process memory corruption | Pin current libpng 1.6.58, keep decoder in ring 3, add malformed-image smoke/watchdog, update dependency through reviewed rebuilds. |
| Graphics prebuilt exceeds loader cap | Stock build cannot launch it | Size gate before refresh; keep optional decoders/fonts out; raise loader cap only in a separately justified memory plan. |
| Start launch has no terminal for diagnostics | Silent failure | Process-service completion handler logs and opens existing Start error dialog; automated startup test covers argv/env/cwd. |
| Users assume HTTPS because the app is graphical | Confusing failures or unsafe pressure | Keep HTTP label/docs, explicit unsupported HTTPS test, and no SSL autodetection. |

---

## Done criteria

- One committed `LINKS.ELF` remains static, non-PIE, reproducibly built from
  pinned Links/zlib/libpng sources, and staged by a stock checkout.
- `links`/`links2` without `-g` retain the existing text browser and all
  HTTP/DNS dump tests.
- `links2 -g` and `-driver agenticos` open one ordinary resizable AgenticOS
  window and render Links' graphical chrome, built-in font, PNG/GIF/XBM page
  images, and HTTP/gzip content with correct colors.
- The Rust library owns platform raster/input/present behavior; the C adapter
  is limited to the documented Links vtable/handler boundary and contains no
  independent renderer.
- GUI input joins Links' existing `select(2)` loop through syscall 5011 with no
  idle polling, lost wake, or parent/child capability leak.
- Keyboard navigation, pointer clicks/drags/wheel, links, forms, Back/Forward,
  menus, search, bookmarks/history, resize, title updates, and downloads work
  against deterministic fixtures.
- Start -> Programs -> Web Browser launches graphics mode without a terminal;
  shell launch waits and resumes normally after close.
- Repeated close, process fault, DNS fork, network-off, allocation-failure, and
  resize paths leave fd, GUI ownership, and memory counts at baseline.
- Measured flush/present behavior meets the interaction/idle bounds or a
  separately versioned partial-present prerequisite lands before the feature
  is called performant.
- Project documentation states the exact capability boundary: graphical
  IPv4 HTTP Links, no HTTPS, JavaScript, IPv6, system clipboard, or modern-web
  compatibility.

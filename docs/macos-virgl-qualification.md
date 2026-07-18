# macOS VirGL qualification

AgenticOS keeps stock Homebrew QEMU as the default. The optional GPU path uses
an explicitly selected, checksum-qualified custom keg and never changes the
`qemu-system-x86_64` symlink on `PATH`.

## Host gate

Installing the custom tap is an explicit developer action; it is intentionally
not part of Conductor setup:

```sh
brew tap startergo/qemu-virgl-kosmickrisp
brew install startergo/qemu-virgl-kosmickrisp/qemu
```

Homebrew 6 refuses that install when `homebrew/core/qemu` is already present
because both formulae are named `qemu`. Do not unlink stock QEMU to work around
that check. A side-by-side provision must pour and relocate the checksum-pinned
bottle into the exact versioned keg while leaving `opt/qemu` unchanged.

Qualify the exact keg before requesting the GPU compositor:

```sh
QEMU_VIRGL_PREFIX="$(brew --cellar qemu)/1.0.27"
AGENTICOS_QEMU_BIN="$QEMU_VIRGL_PREFIX/bin/qemu-system-x86_64" \
AGENTICOS_QEMU_GL=es \
./scripts/qemu-virgl-preflight.sh
```

The verifier is read-only apart from its report. It requires the pinned
`1.0.27` arm64 bottle, checks the expected bottle receipt and dependency
versions, resolves the QEMU and virglrenderer Mach-O dependency lists,
requires `virtio-vga-gl` and TCG, and briefly starts a paused,
diskless Cocoa GL VM. A successful run writes
`.context/qemu-virgl-qualification.json`.

`gl=es` is the supported ANGLE-to-Metal path. Use `AGENTICOS_QEMU_GL=core`
only as a diagnostic comparison. Injecting HVF through
`AGENTICOS_QEMU_ACCEL` or `AGENTICOS_QEMU_EXTRA_ARGS` is rejected because an
x86-64 guest on Apple Silicon must use TCG.

An explicit `AGENTICOS_COMPOSITOR=gpu` launch runs this preflight first and is
refused when it fails. `auto` remains allowed to launch retained CPU and emits
one host fallback reason. Stock QEMU continues to support ordinary legacy and
retained-CPU launches:

```sh
AGENTICOS_COMPOSITOR=retained AGENTICOS_THEME=aero ./build.sh
```

## CPU baseline telemetry

Enable structured retained-frame counters with `AGENTICOS_RENDER_STATS=1`:

```sh
AGENTICOS_COMPOSITOR=retained \
AGENTICOS_THEME=aero \
AGENTICOS_RENDER_STATS=1 \
AGENTICOS_QEMU_BIN="$QEMU_VIRGL_PREFIX/bin/qemu-system-x86_64" \
./build.sh
```

Each rendered frame reports frame count, rasterized windows/pixels, upload
bytes, composed layers, output damage, presentations, backdrop copy/pass
counts and pixels, the fixed blur scratch working set, and guest cycle buckets
for surface rasterization, texture staging, composition, backdrop blur, fence
wait, and presentation. No line is emitted for idle iterations. Cursor-only or
unchanged movement must report zero retained-surface raster and upload work.

Use the same QEMU binary, 1280x720 resolution, and Aero scene for retained CPU
and future GPU measurements. Capture idle, cursor-only, unchanged-window drag,
terminal scroll, focus change, and popup open/close. Host GPU measurements are
not comparable to stock-QEMU CPU measurements because QEMU build differences
would contaminate the result.

## Current qualified state

The repository now exposes a production `VirglCompositionEngine` after the
pinned host passes preflight and the guest passes deterministic clear,
premultiplied-alpha/readback, lifecycle, and production backdrop-blur gates.
The dedicated integration runner repeats lifecycle creation and teardown 100
times. The production fixtures compare ordered, clipped, translucent layers
and masked/stacked/partial-damage blur with the CPU reference. They also render
an asymmetric logical GL client into a content-well layer, pin its top-left
orientation and clipping, and verify depth ordering when a hardware depth
format exists (or painter ordering when the capset has none). A listed GL device
or advertised feature bit alone still cannot enable GPU mode.

Run the complete hardware-backed gate with:

```sh
AGENTICOS_QEMU_BIN="$QEMU_VIRGL_PREFIX/bin/qemu-system-x86_64" \
AGENTICOS_QEMU_GL=es \
./scripts/test-virgl-integration.sh
```

The qualified production path accelerates ordered textured-quad composition,
translation, clipping/scissor, layer opacity, premultiplied source-over, and
radius-6 Aero backdrop blur. Blur uses persistent render-target/sampler scratch
textures, an output-to-scratch copy, separable ping-pong passes, and a
three-sampler masked combine over the source, blurred snapshot, and preserved
sharp snapshot. Effective source alpha blends sharp into blurred before
source-over, so the window's translucent shadow gutter fades smoothly back to
the unblurred desktop. Transparent frame pixels are discarded so rounded
corners preserve the unblurred output; opaque client pixels replace the blur.
The completed render target remains on the host GPU and is presented as a
scanout-bound VirGL texture; ordinary frames do not transfer it back to guest
RAM and do not touch the boot framebuffer. A 64x64 VirtIO hardware cursor makes
pointer-only movement independent of 3D composition. Explicit deterministic
readback remains available to the hardware oracle and future screenshots.

The hardware runner validates the actual Cocoa GL presenter rather than QMP
`screendump`: QMP sees only QEMU's legacy CPU display surface and is black
during native texture scanout. Qualification requires Cocoa to enter scanout
mode, borrow a nonzero VirGL texture, and complete its texture blit with no GL
error. Broader performance rollout measurements remain follow-up work.

Strict VirGL can use GPU-blurred Aero chrome:

```sh
AGENTICOS_QEMU_BIN="$QEMU_VIRGL_PREFIX/bin/qemu-system-x86_64" \
AGENTICOS_QEMU_GL=es \
AGENTICOS_COMPOSITOR=gpu \
AGENTICOS_GPU_STRICT=1 \
AGENTICOS_THEME=aero \
./build.sh
```

An explicit Aero request stays on qualified VirGL and renders blurred glass;
`auto` also selects Aero after the full production blur gate succeeds.

The custom bottle has no QEMU `user` (slirp) network backend. When
networking is on and the selected QEMU lacks `user`, `build.sh` starts a
machine-less stock-QEMU bridge (`scripts/qemu-slirp-bridge.sh`) that joins
its own slirp NAT and a unix stream listener on one hub, and attaches the
guest's virtio-net through `-netdev stream` on that socket. The guest sees
the ordinary slirp addressing (10.0.2.0/24, gateway 10.0.2.2, DNS 10.0.2.3),
so VirGL launches keep DHCP, DNS, and outbound IPv4. Pin the helper with
`AGENTICOS_QEMU_NET_HELPER_BIN`, move the socket with
`AGENTICOS_SLIRP_SOCK`, or set `AGENTICOS_NETWORK=off` for an offline boot.

The currently pinned ANGLE capset reports no supported depth-attachment
format. `GLGAME.ELF` therefore uses the bounded frontend depth-order fallback
reported by `gui_gl_get_info`; this affects only client geometry ordering and
does not move the final game texture out of VirGL.

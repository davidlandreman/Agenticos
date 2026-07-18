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
bytes, composed layers, output damage, presentations, and guest cycle buckets
for surface rasterization, texture staging, composition, backdrop blur, fence
wait, and presentation. No line is emitted for idle iterations. Cursor-only
or unchanged movement must report zero retained-surface raster and upload
work.

Use the same QEMU binary, 1280x720 resolution, and Aero scene for retained CPU
and future GPU measurements. Capture idle, cursor-only, unchanged-window drag,
terminal scroll, focus change, and popup open/close. Host GPU measurements are
not comparable to stock-QEMU CPU measurements because QEMU build differences
would contaminate the result.

## Current qualified state

The repository now exposes a production `VirglCompositionEngine` after the
pinned host passes preflight and the guest passes deterministic clear,
premultiplied-alpha/readback, and lifecycle gates. The dedicated integration
runner repeats lifecycle creation and teardown 100 times. The production
fixture compares ordered, clipped, translucent layers with the CPU reference
within one channel value. It also renders an asymmetric logical GL client into
a content-well layer, pins its top-left orientation and clipping, and verifies
depth ordering when a hardware depth format exists (or painter ordering when
the capset has none). A listed GL device or advertised feature bit alone still
cannot enable GPU mode.

Run the complete hardware-backed gate with:

```sh
AGENTICOS_QEMU_BIN="$QEMU_VIRGL_PREFIX/bin/qemu-system-x86_64" \
AGENTICOS_QEMU_GL=es \
./scripts/test-virgl-integration.sh
```

The qualified production path accelerates ordered textured-quad composition,
translation, clipping/scissor, layer opacity, and premultiplied source-over.
The completed render target remains on the host GPU and is presented as a
scanout-bound VirGL texture; ordinary frames do not transfer it back to guest
RAM and do not touch the boot framebuffer. A 64x64 VirtIO hardware cursor makes
pointer-only movement independent of 3D composition. Explicit deterministic
readback remains available to the hardware oracle and future screenshots.

The hardware runner validates the actual Cocoa GL presenter rather than QMP
`screendump`: QMP sees only QEMU's legacy CPU display surface and is black
during native texture scanout. Qualification requires Cocoa to enter scanout
mode, borrow a nonzero VirGL texture, and complete its texture blit with no GL
error. Damage-only persistent input textures, performance rollout
measurements, and GPU Aero backdrop blur remain follow-up work.

Strict VirGL can use Aero chrome and translucency while GPU backdrop blur
remains a follow-up:

```sh
AGENTICOS_QEMU_BIN="$QEMU_VIRGL_PREFIX/bin/qemu-system-x86_64" \
AGENTICOS_QEMU_GL=es \
AGENTICOS_COMPOSITOR=gpu \
AGENTICOS_GPU_STRICT=1 \
AGENTICOS_THEME=aero \
AGENTICOS_NETWORK=off \
./build.sh
```

An explicit Aero request stays on VirGL and renders sharp translucent glass;
`auto` continues to choose Classic for VirGL until GPU blur is implemented.
The custom bottle has no QEMU `user` network backend, so the workspace launch
disables networking; this is independent of VirGL.

The currently pinned ANGLE capset reports no supported depth-attachment
format. `GLGAME.ELF` therefore uses the bounded frontend depth-order fallback
reported by `gui_gl_get_info`; this affects only client geometry ordering and
does not move the final game texture out of VirGL.

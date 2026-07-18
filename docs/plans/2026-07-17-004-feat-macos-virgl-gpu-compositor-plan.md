---
title: "feat: Prove and integrate macOS VirGL acceleration for the Aero compositor"
status: planned
created: 2026-07-17
plan_type: feat
depth: deep
related_docs:
  - docs/plans/2026-07-17-002-feat-optional-retained-gpu-compositor-plan.md
  - docs/plans/2026-07-17-003-feat-aero-glass-window-theme-plan.md
  - src/drivers/CLAUDE.md
  - src/graphics/CLAUDE.md
  - src/window/CLAUDE.md
---

# feat: Prove and integrate macOS VirGL acceleration for the Aero compositor

## Outcome

Make `AGENTICOS_COMPOSITOR=gpu` a real, capability-gated renderer on macOS and
use it to accelerate the retained compositor's composition and Aero backdrop
blur. Existing widgets continue to rasterize on the guest CPU. The supported
host is an explicit custom QEMU build with VirGL and a Cocoa OpenGL frontend;
stock Homebrew QEMU remains installed, remains the default, and continues to
run the legacy and retained-CPU paths.

This is the unfinished Phase 4 of the optional compositor plan. It is split
into a narrow transport/render/readback proof and a later production engine so
we can stop without exposing a fake GPU mode if the VirGL command stream or the
macOS host backend is not reliable.

## Why this is the next performance experiment

The Aero path is currently entirely guest-CPU rendered:

1. Widgets and frame chrome rasterize into retained ARGB surfaces.
2. `CpuCompositionEngine` blends every damaged layer.
3. `BackdropSample` copies and runs three box-blur passes in guest RAM.
4. The boot-framebuffer or VirtIO-GPU 2D presenter displays the completed
   pixels.

VirtIO-GPU 2D changes only step 4 and is not acceleration. It cannot make Aero
composition or blur faster. A VirGL engine can move steps 2 and 3 to the host
GPU while preserving the retained surfaces and CPU fallback. It will not make
widget rasterization or x86-64 TCG execution faster, so the work begins with
stage-level measurements and ends with the same measurements.

## Current verified host and repository state

Verified on 2026-07-17:

- Host: Apple Silicon (`arm64`), macOS 26.5.2.
- Installed QEMU: Homebrew QEMU 11.0.1 at
  `/opt/homebrew/bin/qemu-system-x86_64`.
- The installed x86-64 emulator exposes TCG only. It advertises
  `virtio-gpu-pci` and `virtio-vga`, but not `virtio-vga-gl` or
  `virtio-gpu-gl`.
- Retained CPU composition, Aero blur, VirtIO-GPU feature negotiation, control
  queue transport, 2D resources, scanout, and boot-framebuffer fallback exist.
- `VirtioGpu::virgl_advertised()` can inspect feature bit 0, but capset
  discovery, contexts, 3D resources, submit, fences, and readback do not exist.
- Renderer selection deliberately passes `gpu_available=false`; strict GPU
  mode therefore fails instead of claiming acceleration.

## Required custom QEMU for macOS

The first supported candidate is the custom
[`startergo/homebrew-qemu-virgl-kosmickrisp`](https://github.com/startergo/homebrew-qemu-virgl-kosmickrisp)
build, not stock Homebrew QEMU:

- Release: `v1.0.27`, published 2026-01-14.
- Bottle: `qemu-1.0.27.arm64_sequoia.bottle.tar.gz`.
- Bottle SHA-256:
  `a2eaeed6f7b52661436052b413f596785c5e14e2e1b65cd5509713fcfc164566`.
- Reported upstream QEMU commit:
  `cf3e71d8fc8ba681266759bb6cb2e45a45983e3e`.
- Runtime stack recorded by the bottle: virglrenderer 1.0.33, ANGLE 1.0.15,
  and libepoxy 1.0.4.
- Required launch shape:
  `-display cocoa,gl=es -device virtio-vga-gl`.
- Diagnostic comparison:
  `-display cocoa,gl=core -device virtio-vga-gl`.

`gl=es` is the primary path because the custom QEMU routes GLES through ANGLE
to Metal. `gl=core` exercises Apple's desktop OpenGL and is useful only to
separate an ANGLE-specific failure from a guest protocol failure. The tap's
Venus/KosmicKrisp path is not part of this implementation.

The bottle targets arm64 macOS Sequoia and the current host is newer. The
bottle also relies on Homebrew relocation and versioned dynamic dependencies;
extracting it directly is not a supported installation. Host qualification
must therefore prove that the relocated binary starts and renders on this
machine before any guest implementation is called successful.

The custom QEMU must coexist with stock QEMU. No script may unlink or replace
`/opt/homebrew/bin/qemu-system-x86_64`. Developers select the custom binary
through `AGENTICOS_QEMU_BIN` using its fully qualified keg path. Installation
is an explicit developer action and is not added to Conductor setup.

If v1.0.27 cannot run on macOS 26 because of bottle or dependency drift, do not
fall back to the tap's moving-master source build. Fork the formula, pin QEMU
to the recorded upstream commit, pin the two macOS texture/resolution patches
and dependency versions, and publish a new checksum-qualified AgenticOS
candidate. That is a host-toolchain repair, not permission to loosen the guest
capability gate.

## Goals

- Qualify one exact custom QEMU/VirGL/Cocoa build on the current Mac without
  disturbing stock QEMU.
- Add the minimum VirtIO-GPU 3D transport required to enumerate VirGL capsets,
  create a context and resources, submit work, fence it, and read pixels back.
- Prove a deterministic clear, then textured premultiplied-alpha quads, before
  wiring the window system to the GPU.
- Add a production `VirglCompositionEngine` that caches textures and uploads
  only surface damage.
- Implement Aero `BackdropSample` on the GPU with bounded offscreen blur
  passes, since alpha-only composition does not solve the reported Aero cost.
- Preserve guest CPU surface copies and fall back atomically to retained CPU
  composition after any GPU failure.
- Measure CPU retained versus VirGL on the same host, resolution, scene, and
  QEMU binary.

## Non-goals

- Exposing OpenGL, Vulkan, Mesa, DRM, or VirGL to ring-3 applications.
- Porting Mesa or implementing a general Gallium driver.
- Using Venus, gfxstream, rutabaga, or a bespoke Metal device in this spike.
- GPU-rasterizing widgets, fonts, wallpaper, or Aero frame chrome.
- Enabling x86-64 HVF on Apple Silicon. AgenticOS remains an x86-64 TCG guest.
- Making the custom QEMU or GPU compositor the default before the correctness
  and performance gates pass.

## Work sequence

### M0 — Add a reproducible performance baseline

Instrument retained rendering into separately reported stages:

- surface rasterization;
- texture/staging upload;
- layer composition;
- backdrop blur;
- fence wait;
- presentation.

Record idle, cursor-only, unchanged-window drag, terminal scroll, focus change,
and popup open/close at 1280x720 with Aero enabled. Record frame count, damaged
pixels, windows rasterized, upload bytes, output pixels, and stage cycles. Use
the same stock/custom QEMU binary for CPU-versus-GPU comparisons so QEMU build
differences are not mistaken for renderer improvements.

Acceptance:

- Idle produces no composition or presentation work.
- Moving an unchanged frame reports zero surface rasterization and zero upload
  bytes in the CPU baseline.
- The log distinguishes guest work from host fence/present latency.

### M1 — Qualify and pin the macOS host toolchain

Add `scripts/qemu-virgl-preflight.sh` as a read-only verifier. Given one exact
binary, it must:

1. print host OS/architecture and the resolved binary path;
2. verify the expected release/build metadata and record `--version`;
3. verify Mach-O dependencies resolve, including virglrenderer, ANGLE, and
   libepoxy;
4. require `virtio-vga-gl` in `-device help`;
5. require Cocoa `gl=es` to parse and initialize;
6. verify x86-64 uses TCG and reject an injected `-accel hvf` policy;
7. emit a machine-readable qualification record under `.context/`, including
   the bottle checksum and dependency versions.

Extend `scripts/qemu-compositor.sh` to call this stricter preflight only for an
explicit `gpu` request. `auto` may fall back to retained CPU with one reason.
`build.sh` must continue resolving and launching the same binary it probed.

Run a host-only GL smoke test before involving AgenticOS. A minimal Linux guest
is acceptable for this one host qualification step, but it is not evidence
that the AgenticOS guest driver works.

Stop/go gate 1: the custom binary starts repeatedly with Cocoa `gl=es`, exposes
`virtio-vga-gl`, and shows no loader/renderer errors. If this fails, repair or
repin the host toolchain before writing the production guest engine.

### M2 — Implement VirtIO-GPU VirGL transport

Add `src/drivers/virtio/gpu/virgl.rs` and extend protocol definitions with
compile-time layout assertions for:

- `GET_CAPSET_INFO` and `GET_CAPSET`;
- `CTX_CREATE` and `CTX_DESTROY`;
- `RESOURCE_CREATE_3D`, context attach/detach, and resource unref;
- 3D transfers to/from host;
- `SUBMIT_3D`;
- fence IDs, fenced responses, bounded completion, and error mapping.

Enumerate all advertised capsets and allow only a pinned VirGL/VirGL2 capset
version. Store the raw capset blob for diagnostics, but parse only the fields
needed by the compositor. Every command-stream constant and structure must
record its source repository, commit/tag, license, and any local deviation.
Prefer the virglrenderer/Mesa protocol revision matching the qualified host
over copying current `master` definitions.

Keep transport independent from scene composition. Unit tests should validate
wire bytes, response lengths/types, overflow handling, context/resource
lifetime, fence mismatch, timeout, and teardown after each partial failure.

### M3 — Pass a two-step render/readback spike

The guest integration fixture runs in strict GPU mode and has two increments:

1. Create a context and render target, clear to a known RGBA color, fence,
   transfer from host, and compare exact readback pixels.
2. Upload a 2x2 premultiplied texture and render overlapping opaque and
   half-alpha quads with scissor and source-over blending. Compare selected
   pixels with `CpuCompositionEngine` within a documented one-channel
   tolerance.

Repeat creation, submission, readback, and destruction at least 100 times to
catch stale handles and resource lifetime errors. Run first with Cocoa
`gl=es`, then with `gl=core` as a diagnostic. Repeat the same guest fixture on
Linux QEMU plus upstream virglrenderer as an independent protocol check.

Add a dedicated integration runner because the normal `test.sh -display none`
path cannot be assumed to create the patched Cocoa GL context. The runner must
still use serial assertions and `isa-debug-exit`; visual inspection is not the
test oracle.

Stop/go gate 2: both fixtures pass repeatedly with clean teardown and no QEMU,
ANGLE, or virglrenderer errors. If this needs a substantial Mesa/Gallium port,
stop. Keep `gpu` unavailable and retain CPU Aero as the product path.

### M4 — Refactor renderer ownership for a real GPU engine

The current retained renderer owns a concrete `CpuCompositionEngine`, and its
`CompositionEngine` contract assumes every engine exposes a CPU `Surface` via
`output()`/`output_mut()`. That is not a valid contract for a GPU render target.

Before adding the engine:

- make retained engine ownership an enum or a backend-neutral owner;
- separate `compose`, `present`, and optional `readback` contracts;
- keep one `VirtioGpu` device owner—do not let the 2D presenter and VirGL engine
  discover and initialize the same PCI function twice;
- keep canonical CPU surfaces alive in GPU mode for immediate fallback;
- install the candidate engine only after capset discovery and the smoke test;
- make renderer capabilities reflect the initialized engine, not the QEMU
  device name.

Strict GPU initialization fails before building the desktop. `auto` tears down
the failed candidate and constructs retained CPU. Runtime failure preserves
the current scene, recomposes it on CPU, and uses the best remaining presenter.

### M5 — Implement `VirglCompositionEngine`

Implement only the compositor feature set:

- one host texture per retained surface;
- damage-only texture upload;
- one output render target;
- ordered textured quads;
- translation, clip/scissor, and layer opacity;
- premultiplied source-over blending;
- damage-scissored clears/draws;
- bounded fences and structured device-loss errors;
- deterministic readback for tests and screenshots.

Movement, z-order, and opacity changes must submit new composition work without
rerasterizing or reuploading unchanged surfaces. Keep resource destruction
idempotent so partially created engines can always fall back.

Acceptance:

- Opaque desktop and alpha fixtures match the CPU oracle.
- Moving an unchanged window reports zero widget pixels and zero texture bytes
  uploaded.
- Injected submit rejection, timeout, or context loss returns to retained CPU
  without a black frame.

### M6 — Move Aero backdrop blur to the GPU

Implement `LayerEffect::BackdropSample` rather than silently dropping it:

1. compose layers below the glass region into the output target;
2. copy the damage-expanded backdrop into a reusable offscreen texture;
3. run horizontal and vertical blur passes (or a documented three-box
   equivalent matching the CPU reference) through ping-pong targets;
4. blend the translucent Aero surface over the blurred backdrop;
5. restrict work to effect-expanded damage and reuse targets between frames.

Start with the current radius 4 behavior and CPU-reference pixels. Exact
matching is required for opaque regions; blurred/alpha pixels get a small,
documented tolerance. If GPU blur is unavailable, the engine must report an
unsupported capability during initialization so Aero selects/falls back
coherently; it must not render unblurred glass while claiming full support.

### M7 — Measure, document, and decide rollout

Repeat M0 on the qualified custom QEMU with both retained CPU and VirGL.
Capture median and p95 stage costs plus guest/host CPU utilization for the Aero
drag and terminal-scroll cases.

Initial rollout gates:

- correctness fixtures and the full booted test suite pass;
- no full-surface upload on move/z-order/opacity-only frames;
- no idle GPU submissions;
- Aero composition+blur stage cost improves materially (target at least 2x at
  1280x720 on the qualification host), or the result documents why the
  remaining bottleneck is widget rasterization/TCG;
- strict mode never falls back silently;
- stock QEMU still runs legacy and retained CPU unchanged.

After the gates pass, document the exact custom QEMU as an optional supported
developer toolchain. Keep `legacy`/retained CPU as default until the custom
host has soaked; changing the default is a separate decision.

## Expected file changes

```text
scripts/qemu-virgl-preflight.sh
scripts/qemu-compositor.sh
build.sh
test.sh or scripts/test-virgl-integration.sh

src/drivers/virtio/gpu/protocol.rs
src/drivers/virtio/gpu/virgl.rs
src/drivers/virtio/gpu/mod.rs
src/graphics/composition/mod.rs
src/graphics/composition/virgl/{mod,commands,shaders}.rs
src/window/renderer/retained.rs
src/window/renderer/mod.rs
src/window/manager.rs
src/tests/virtio_gpu_protocol.rs
src/tests/virgl_commands.rs
src/tests/virgl_integration.rs
```

## Validation commands

```sh
# Stock QEMU remains the ordinary path.
AGENTICOS_COMPOSITOR=retained AGENTICOS_THEME=aero ./build.sh

# Exact custom binary; never depend on whichever qemu is first on PATH.
QEMU_VIRGL_PREFIX="$(brew --prefix startergo/qemu-virgl-kosmickrisp/qemu)"
AGENTICOS_QEMU_BIN="$QEMU_VIRGL_PREFIX/bin/qemu-system-x86_64" \
AGENTICOS_QEMU_GL=es \
./scripts/qemu-virgl-preflight.sh

AGENTICOS_QEMU_BIN="$QEMU_VIRGL_PREFIX/bin/qemu-system-x86_64" \
AGENTICOS_QEMU_GL=es \
AGENTICOS_COMPOSITOR=gpu \
AGENTICOS_GPU_STRICT=1 \
AGENTICOS_THEME=classic \
./build.sh

cargo fmt -- --check
cargo check
./test.sh virtio_gpu_protocol virgl_commands compositor_selection composition_cpu
./scripts/test-virgl-integration.sh
./test.sh
```

## Implementation checkpoint — 2026-07-18

M1-M3 are qualified on the pinned macOS `gl=es` ANGLE/Metal stack. The guest
has bounded VirGL transport, a minimal protocol-derived encoder, exact clear
and alpha readback fixtures, and 100-cycle teardown coverage. M4's sole-device
ownership and strict/non-strict fallback policy are installed. M5 has a real
production engine for ordered textured quads, opacity, translation, scissor,
premultiplied source-over, fencing, and deterministic CPU readback; its
production scene matches the CPU oracle within one channel value.

The local Conductor launch is enabled for strict VirGL with Classic. It is an
explicit developer opt-in, not a repository-wide default. M5's persistent
per-surface/damage-only upload optimization and M6 GPU backdrop blur are still
open, as are M7 performance measurements and rollout gates. Aero therefore
selects Classic coherently on VirGL instead of silently omitting blur.

## Primary risks and decisions

| Risk | Decision |
|---|---|
| The Sequoia bottle does not run on macOS 26 | Repin a reproducible custom build; do not use moving QEMU master |
| Device name exists but VirGL is unusable | Require guest feature + capset + render/readback proof |
| VirGL encoder scope expands toward Mesa | Stop at the spike gate; do not expose GPU mode |
| GPU alpha works but Aero remains slow | GPU blur is a required milestone; stage metrics identify remaining CPU raster/TCG cost |
| 2D presenter and 3D engine fight over the device | Introduce one VirtIO-GPU owner before engine integration |
| GPU engine cannot provide a CPU output surface | Split compose/present/readback contracts before implementation |
| Runtime failure produces a black window | Retain CPU surfaces and boot/2D presenter until a GPU frame succeeds |
| Custom QEMU replaces the normal developer install | Require an explicit fully qualified `AGENTICOS_QEMU_BIN`; never unlink stock QEMU in scripts |

## References

- QEMU VirtIO-GPU modes:
  https://www.qemu.org/docs/master/system/devices/virtio/virtio-gpu.html
- VirtIO 1.3 GPU device specification:
  https://docs.oasis-open.org/virtio/virtio/v1.3/virtio-v1.3.html#x1-3700007
- startergo custom macOS QEMU/VirGL tap:
  https://github.com/startergo/homebrew-qemu-virgl-kosmickrisp
- pinned custom QEMU release:
  https://github.com/startergo/homebrew-qemu-virgl-kosmickrisp/releases/tag/v1.0.27
- pinned formula and macOS patches:
  https://github.com/startergo/homebrew-qemu-virgl-kosmickrisp/tree/v1.0.27

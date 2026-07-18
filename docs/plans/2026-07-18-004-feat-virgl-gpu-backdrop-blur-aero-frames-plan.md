---
title: "feat: Add VirGL backdrop blur to Aero window frames"
status: implemented
created: 2026-07-18
plan_type: feat
depth: deep
related_docs:
  - docs/plans/2026-07-17-003-feat-aero-glass-window-theme-plan.md
  - docs/plans/2026-07-17-004-feat-macos-virgl-gpu-compositor-plan.md
  - docs/plans/2026-07-18-001-perf-virgl-direct-scanout-plan.md
  - docs/plans/2026-07-18-003-perf-virgl-persistent-surface-textures-plan.md
  - docs/macos-virgl-qualification.md
  - src/drivers/CLAUDE.md
  - src/graphics/CLAUDE.md
  - src/window/CLAUDE.md
---

# feat: Add VirGL backdrop blur to Aero window frames

## Implementation result

Implemented on 2026-07-18. VirGL now retains one full-output backdrop snapshot
plus two full-output blur scratch resources, and executes the shared three-box
radius decomposition through bounded
horizontal/vertical TGSI shaders, and combines source plus blurred backdrop
through an effective-alpha mask with transparent discard. The preserved sharp
snapshot is mixed with the blurred snapshot so soft effect edges fade instead
of ending abruptly. Construction qualifies that production path against the
CPU compositor before VirGL becomes available. Aero radius 6 is selected
automatically for qualified VirGL, telemetry reports copy/pass work and scratch
bytes, and the hardware-backed integration oracle covers masked, opaque,
stacked, partial-damage, and unsupported-radius cases.

## Outcome

Make `LayerEffect::BackdropSample { radius }` a real VirGL compositor effect
and use it for Aero window-frame glass. A qualified VirGL frame must snapshot
the already-composed backdrop, run the same three-box separable blur specified
by the CPU reference compositor, and combine the translucent frame surface over
that blurred result without reading the scanout target while writing it.

After this work:

- explicit `AGENTICOS_THEME=aero` on VirGL renders blurred glass rather than
  sharp translucency;
- `AGENTICOS_THEME=auto` selects Aero for a qualified VirGL backend;
- Classic and effect-free VirGL frames keep their current cached-texture,
  damage-scissored, single-submission, direct-scanout path;
- production frames still perform no GPU-to-guest readback;
- unsupported blur radii or any blur resource/shader/submit failure take the
  existing strict panic or non-strict retained-CPU fallback path rather than
  silently dropping the effect.

This is a composition change, not GPU widget rasterization. Aero chrome, text,
corners, shadows, and client pixels remain canonical guest CPU surfaces.

## Current state

The backend-neutral and theme-side contracts already exist:

- `FrameWindow::new` assigns `LayerEffect::BackdropSample { radius: 4 }` when
  the active theme is Aero (`src/window/windows/frame.rs`).
- `WindowManager::render_retained` copies each root window's compositor
  properties into its scene layer and expands composition damage where
  backdrop changes can affect a glass layer (`src/window/manager.rs`).
- `CpuCompositionEngine` is the pixel reference. For every damaged work
  region it snapshots the lower output, partitions the requested radius across
  three box passes, performs horizontal and vertical sliding-window blurs, and
  uses the blurred pixel only behind a nonzero, non-opaque source pixel
  (`src/graphics/composition/cpu.rs`).
- The VirGL compositor already owns a persistent scanout render target, one
  persistent texture/sampler view per retained surface, a growable vertex
  resource, fixed blend/rasterizer/shader state, damage-scissored draws, one
  fenced submission, and direct scanout (`src/graphics/composition/virgl.rs`).

The missing behavior is specific and currently silent: `PreparedLayer` does
not carry `Layer.effect`, so VirGL draws an Aero layer through the ordinary
source-over shader and produces sharp translucent glass. Theme `auto` chooses
Classic for `RendererKind::Virgl` to avoid advertising that as complete Aero.
The macOS qualification guide documents both limitations.

## Goals

1. Implement radius-4 Aero backdrop blur entirely in the VirGL command stream.
2. Preserve correct z-order when two or more glass windows overlap.
3. Preserve transparent frame corners/margins, opaque client pixels, layer
   opacity, clipping, translation, and premultiplied source-over semantics.
4. Restrict copies and blur draws to effect-expanded damage; do not blur the
   whole screen for a small terminal or drag update.
5. Allocate blur resources and shader/state objects once per engine and reuse
   them until teardown.
6. Keep one fenced 3D submission and direct scanout per damaged frame.
7. Qualify the exact copy/blur/multi-sampler shader path before the renderer is
   allowed to report itself as VirGL-capable for automatic Aero selection.
8. Compare deterministic GPU readback against the CPU reference with a narrow,
   blur-only channel tolerance.
9. Add telemetry that separates semantic layer draws from resource copies and
   blur passes without pretending guest cycle counters are host GPU timestamps.

## Non-goals

- GPU-rasterizing widgets, glyphs, shadows, rounded corners, or client apps.
- Exposing VirGL, OpenGL, Mesa, DRM, or shader APIs to ring-3 processes.
- Runtime theme switching or a settings UI.
- A general compositor render graph or arbitrary programmable effects.
- Compute shaders, mip-chain blur, Vulkan/Venus, or a Metal-specific path.
- Pipelining multiple in-flight frames or removing the current fenced
  submission.
- Changing the CPU blur's visible contract as part of the GPU implementation.
- Making the GPU compositor the global default; this only changes `theme=auto`
  after VirGL has already been explicitly selected and qualified.

## Design decisions

### 1. Keep `LayerEffect` as the single backend-neutral contract

Move the Aero radius into a named theme constant and expose a small
theme-owned helper such as `theme::frame_effect()`:

```rust
pub const AERO_BACKDROP_RADIUS: u16 = 4;

pub const fn frame_effect_for(kind: ThemeKind) -> LayerEffect {
    match kind {
        ThemeKind::Classic => LayerEffect::None,
        ThemeKind::Aero => LayerEffect::BackdropSample {
            radius: AERO_BACKDROP_RADIUS,
        },
    }
}
```

`FrameWindow` uses that helper when setting `CompositorProperties`. The theme
still paints only pixels; it does not depend on VirGL types or issue GPU work.
This makes the Aero-frame/effect relationship testable without duplicating the
radius in theme, compositor, and integration fixtures.

Extract the three-box radius partition into a shared pure helper used by both
composition engines:

```text
total radius 4 -> box radii [1, 1, 2]
each nonzero box -> horizontal pass, then vertical pass
```

The GPU implementation initially supports a bounded radius range that includes
the production radius and test fixtures. Choose the bound from the maximum
generated TGSI instruction count and command-stream budget, document it as
`MAX_GPU_BACKDROP_RADIUS`, and return `CompositionError::UnsupportedEffect`
outside it. Never clamp a requested radius or treat an unsupported effect as
`None`.

### 2. Snapshot the current output instead of recomposing lower layers

When an effect layer is reached in z-order, the output render target already
contains every lower layer, including the results of any earlier glass layer.
Copy that region into a scratch texture with classic VirGL
`RESOURCE_COPY_REGION`. The command carries source/destination resource IDs,
origins, and a 3D extent; add only this bounded command to
`VirglCommandEncoder`.

This choice is load-bearing for overlapping Aero windows. Re-rendering the
lower scene into a scratch target would either duplicate all lower layer draws
or require recursive effect evaluation. Copying the current output naturally
captures the exact result at that z-position and keeps the ordinary scene walk
as the source of ordering truth.

The command layout must be copied from the same pinned virglrenderer protocol
revision already cited by `commands.rs`, with the source commit and MIT license
recorded beside the encoder constant. The current upstream layout is a
13-dword payload after the command header; unit tests assert every field.

### 3. Use two persistent full-output ping-pong targets

Create two scratch `PIPE_TEXTURE_2D` resources with both render-target and
sampler-view binds. Each matches the output dimensions and format
`B8G8R8A8_UNORM` and owns:

- one `VirglResource`;
- one render-target surface handle;
- one persistent sampler-view handle.

Allocate them during `VirglCompositionEngine::new`, before the engine is
reported as available. This makes successful VirGL selection a truthful blur
capability gate and lets `theme=auto` choose Aero without risking a predictable
first-frame allocation failure. Use checked byte arithmetic and unwind both
resources, views, surfaces, output, and context in reverse order after every
partial initialization failure.

Two full-output BGRA targets add `2 * width * height * 4` bytes of guest
backing (about 7 MiB at 1280x720). Keep this separate from the 48 MiB canonical
surface-texture cache and report it as fixed blur working-set bytes. Reject an
output whose checked scratch allocation cannot fit; do not overcommit the
kernel heap.

Full-sized resources avoid per-damage allocation and host object churn. The
copy and every blur pass remain scissored to the clipped work rectangle, so a
small damage update does not process the full allocation.

### 4. Match the CPU blur with separable TGSI passes

For a damaged rectangle, compute the same clipped blur work rectangle used by
the CPU contract: inflate by the scene's total visible backdrop halo, clipped
to output bounds. The work halo ensures that stale scratch texels outside the
copied region cannot influence final pixels inside the original damage. At
physical output edges, the existing clamp-to-edge sampler provides the CPU
edge behavior.

For each effect layer intersecting that damage rectangle:

1. `RESOURCE_COPY_REGION` copies the current output work rectangle into
   scratch A at identical coordinates.
2. For each nonzero radius in the shared three-box partition:
   - bind scratch B as framebuffer and draw the work rectangle through a
     horizontal averaging shader sampling A;
   - bind scratch A as framebuffer and draw it through the matching vertical
     shader sampling B.
3. Scratch A again contains the final blurred backdrop.
4. Restore the output framebuffer and draw the effect layer only inside
   `damage ∩ layer bounds ∩ layer clip`.

Generate/cache bounded TGSI blur shader variants by `(axis, box_radius)`. The
shader embeds the output texel step, samples `2r + 1` taps, sums premultiplied
RGBA, and multiplies by the reciprocal tap count. Shader handles, links, and
objects persist across frames and are destroyed before their resources and
context. The radius bound must keep every generated shader and the complete
frame below `MAX_ENCODER_DWORDS`.

Rendering each horizontal+vertical pair back into scratch A means the number
of live ping-pong textures stays at two for every supported radius.

### 5. Combine the source and blurred backdrop without target feedback

The ordinary source-over draw is insufficient because the backdrop is now a
texture rather than the currently bound output target. Sampling the output
while it is also the framebuffer is forbidden and host-dependent.

Add an effect fragment shader with three sampler views:

- slot 0: the canonical layer texture;
- slot 1: the final blurred scratch texture.
- slot 2: the preserved sharp backdrop snapshot.

The source and backdrop textures use different coordinates: slot 0 uses the
layer's existing source UV, while slots 1 and 2 use absolute output UV because
each scratch texture is full-screen. Extend the vertex shader with a second
generic output derived from the NDC position
(`output_uv = position.xy * 0.5 + 0.5`). This avoids expanding the 32-byte
vertex format or baking per-damage coordinates into shader objects. Ordinary
composition keeps sampling only the original source-UV generic; blur and
effect shaders consume the derived output-UV generic.

It applies layer opacity to all premultiplied source channels, then computes:

```text
effect_backdrop = sharp_backdrop * (1 - source_alpha)
                + blurred_backdrop * source_alpha
result = source + effect_backdrop * (1 - source_alpha)
```

Using effective source alpha as the effect mask gives translucent decoration
edges a continuous transition from blurred to sharp backdrop. This is what
lets the window shadow fade out without leaving a rectangular blur cutoff.

The draw uses replace blending because the shader has already performed
source-over. A fully transparent source pixel must discard the fragment so the
unblurred output remains untouched. This preserves transparent rounded corners
and decoration margins. An opaque source naturally writes the source exactly,
so the client area does not expose the blur. The runtime blur smoke explicitly
qualifies the discard path; no host is accepted merely because the shader
compiled.

Extend the encoder from the current one-view convenience methods to checked
contiguous fragment sampler-view/state binding. Existing one-view callers use
the generalized method, and teardown unbinds every live slot before destroying
views.

### 6. Integrate effects into the existing damage-scissored scene walk

Extend `PreparedLayer` with its `LayerEffect`. Keep the current outer loop of
damage rectangles and inner z-ordered layer loop. Normal layers retain their
existing shader/blend path. Effect layers perform the snapshot/pass/combine
sequence exactly where their normal draw would have occurred.

Retain `WindowManager::expand_backdrop_damage` as the backend-neutral rule
that turns a backdrop change into composition/presentation damage on nearby
glass. Extract a shared scene halo helper so CPU and VirGL do not independently
sum visible effect radii. Add tests for:

- damage just outside a glass layer affecting the nearest glass pixel;
- two overlapping glass layers, where the upper layer samples the completed
  lower layer;
- disjoint damage regions remaining bounded;
- output-edge clipping and radius overflow.

Scratch-resource writes never become scanout damage. `present_direct` flushes
only the final expanded output damage already returned by the manager.

### 7. Preserve resource caching and transactional surface damage

Blur does not change canonical surface identity or upload ownership:

- surface textures remain keyed only by `SurfaceId` and `SurfaceDesc`;
- movement, z-order, opacity, focus, and effect-only changes upload zero
  canonical pixel bytes;
- surface damage is acknowledged only after the complete copy/blur/compose
  submission succeeds;
- blur scratch textures are not inserted into the surface cache and do not
  consume dynamic `SurfaceId` entries;
- no blur resource, view, surface, or shader is created/destroyed on a steady
  frame.

If encoding, copying, drawing, submission, fence wait, or later presentation
fails, return an error before acknowledging surface damage. Strict mode keeps
its visible panic. Non-strict mode drops the sole GPU owner, fully recomposes
the canonical scene with `CpuCompositionEngine`, and retains the Aero theme.
If that CPU fallback also fails and the renderer drops to legacy, explicitly
downgrade the active theme to Classic before repainting because legacy cannot
paint or compose Aero alpha.

### 8. Qualify blur before enabling automatic Aero

Add a small `VirtioGpu::virgl_backdrop_blur_readback_smoke` beside the existing
clear and alpha gates. It should use the production encoder primitives and
prove all of these in one bounded context:

- resource-copy ordering from a rendered output into scratch;
- scratch resources used alternately as sampler and framebuffer;
- horizontal and vertical blur shaders;
- two simultaneous fragment sampler views;
- replace-blend effect composition;
- transparent-source discard;
- exact cleanup after success and after each injected partial failure.

Use a small impulse backdrop with transparent, translucent, and opaque source
pixels. Require unchanged transparent/opaque pixels and a spread impulse under
the translucent pixel. Compare the blurred channel values to the CPU helper
within the documented blur tolerance.

Call the smoke gate from `VirglCompositionEngine::new` before returning the
engine. Only after this gate lands should `ThemeRequest::Auto` select Aero for
both `RendererKind::RetainedCpu` and `RendererKind::Virgl`. Explicit Aero keeps
its current behavior on retained CPU; legacy still resolves to Classic.

### 9. Extend telemetry without mislabeling GPU time

Keep `composition_cycles` as guest time spent preparing ordinary composition
commands and `fence_wait_cycles` as the synchronous host completion wait. Use
`backdrop_blur_cycles` for guest command preparation/encoding attributable to
effect copies, passes, and combine draws—not as a claim of isolated GPU
execution time.

Add counters:

- `backdrop_copies`;
- `backdrop_copy_pixels`;
- `backdrop_blur_passes`;
- `backdrop_blur_pixels`;
- fixed `backdrop_scratch_bytes`;
- effect-layer draw count if `layers_composed` remains semantic rather than
  physical draw-call count.

The render-stats log and macOS qualification guide must define these counters.
An idle or hardware-cursor-only sample remains all zero.

## Implementation sequence

### M1 — Lock the shared blur contract and theme ownership

1. Add `AERO_BACKDROP_RADIUS` and `theme::frame_effect_for`.
2. Make `FrameWindow` use the theme helper instead of matching `ThemeKind`
   directly.
3. Extract/test the shared three-box radius partition and scene halo helpers.
4. Add `CompositionError::UnsupportedEffect` and require VirGL to match every
   visible effect explicitly.

Acceptance: Classic produces `LayerEffect::None`; Aero produces radius 6; CPU
blur tests are unchanged; an out-of-range GPU radius cannot render sharply.

### M2 — Extend the bounded VirGL encoder

1. Add `resource_copy_region` with checked IDs, coordinates, extents, and exact
   protocol dword tests.
2. Generalize fragment sampler-view and sampler-state binding to bounded
   slices and explicit start slots.
3. Add exact create/bind/unbind/destroy stream tests for bounded sampler views.
4. Keep the existing one-view alpha smoke and production path green.

Acceptance: `virtio_gpu_protocol` proves the byte/dword layout and rejects
zero IDs, empty extents, overflowing coordinates, excess sampler slots, and
encoder-capacity overflow.

### M3 — Prove the primitive chain in a hardware blur smoke

1. Build the tiny copy + ping-pong + alpha-masked combine readback fixture.
2. Compare its pixels with the CPU blur oracle.
3. Repeat enough times to expose stale handles and framebuffer/sampler hazards.
4. Inject cleanup failures at each resource/object creation boundary.

Stop/go gate: do not enable automatic Aero or modify the production engine
until the pinned Cocoa `gl=es` host passes repeatedly with no QEMU,
virglrenderer, ANGLE, or teardown error. Run the same fixture on a Linux VirGL
host when available as an independent protocol check.

### M4 — Add persistent production blur resources and shaders

1. Allocate the checked snapshot and two blur scratch resources during engine
   initialization.
2. Create their surfaces and sampler views with the fixed pipeline.
3. Generate/cache bounded axis/radius shader variants.
4. Add reverse-order partial-init and `Drop` cleanup.
5. Report fixed scratch bytes and pipeline object counts.

Acceptance: resources/objects are created once, survive steady frames, and are
destroyed before the output resource/context with no handle collision against
dynamic retained-surface sampler views.

### M5 — Execute `BackdropSample` in z-order

1. Carry effects into `PreparedLayer`.
2. Copy the current output work region when an effect layer is reached.
3. Encode the shared three-box horizontal/vertical passes.
4. Draw the source+sharp+blur combine shader with alpha masking and transparent
   discard.
5. Keep normal layers on the current fast path and retain one submission.
6. Acknowledge canonical surface damage only after the full submission.

Acceptance: uniform, impulse, transparent-corner, opaque-client, layer-opacity,
partial-damage, edge-clipping, and stacked-glass scenes match CPU semantics;
ordinary production frames still report zero readback bytes.

### M6 — Enable Aero selection and complete fallback behavior

1. Change theme `auto` to select Aero for qualified VirGL.
2. Keep explicit Aero and retained-CPU fallback behavior intact.
3. Downgrade to Classic only if runtime recovery reaches legacy.
4. Update theme/selection tests and renderer capability logs.

Acceptance: `gpu + auto` and `gpu + aero` show blurred Aero; `gpu + classic`
stays Classic; `legacy + aero` and `legacy + auto` remain Classic; injected
non-strict GPU failure produces CPU-blurred Aero without a black or sharp-glass
intermediate frame.

### M7 — Measure and document rollout

1. Update subsystem guidance and the macOS qualification guide.
2. Capture retained CPU versus VirGL Aero at 1280x720 for initial desktop,
   unchanged drag, terminal scroll, focus/z-order change, overlapping windows,
   and idle/cursor-only movement.
3. Report absolute p50/p95 total cycles, blur command-preparation cycles, fence
   wait, copy/pass pixels, uploads, object churn, submissions, and presents.
4. Keep Classic as an explicit control scene to catch non-effect regressions.

Acceptance:

- no resource/view/surface/shader churn after initialization;
- no canonical texture upload on unchanged movement, opacity, or z-order;
- one GPU submission and zero readback bytes per damaged production frame;
- blur copy/pass pixels are bounded by effect-expanded damage, not full output;
- idle and cursor-only movement submit no 3D blur work;
- Classic steady-frame counters and pixel output do not regress;
- VirGL Aero materially improves the CPU reference's blur-heavy frame cost on
  the pinned host, with absolute measurements recorded even if fence latency
  limits the total-frame gain.

## Test matrix

| Layer/behavior | Required oracle |
|---|---|
| Uniform opaque backdrop + translucent glass | GPU equals ordinary source-over within 1 channel value |
| Single bright impulse | Neighboring blurred pixels become nonzero; GPU within blur tolerance of CPU |
| Fully transparent frame pixel | Existing output is unchanged exactly |
| Fully opaque client pixel | Source pixel replaces backdrop exactly |
| Layer opacity | Effective premultiplied source and alpha match CPU |
| Partial backdrop update near glass | Glass changes using the current neighborhood |
| Damage far from glass | No copy or blur passes |
| Two overlapping glass windows | Upper blur samples the completed lower glass result |
| Output edge/corner | No out-of-bounds copy, scissor, or sampling |
| Clean window move | Zero texture upload; bounded blur work only at old/new frame damage |
| Runtime VirGL failure | Strict panic or full retained-CPU Aero recovery |
| Legacy fallback | Theme becomes Classic before repaint |
| Direct scanout | Zero ordinary readback and exact expanded flush regions |

Set the blurred/translucent tolerance from measured quantization in the
hardware fixture (expected to be a few channel values because each GPU render
target pass requantizes float output). Do not weaken the existing one-value
tolerance for ordinary alpha composition or opaque pixels.

## Expected file changes

```text
docs/plans/2026-07-18-004-feat-virgl-gpu-backdrop-blur-aero-frames-plan.md
docs/macos-virgl-qualification.md

src/drivers/virtio/gpu/virgl.rs
src/drivers/virtio/gpu/virgl/commands.rs
src/graphics/CLAUDE.md
src/graphics/composition/mod.rs
src/graphics/composition/cpu.rs
src/graphics/composition/virgl.rs
src/graphics/scene.rs
src/window/CLAUDE.md
src/window/manager.rs
src/window/theme/mod.rs
src/window/windows/frame.rs
src/tests/composition_cpu.rs
src/tests/compositor_selection.rs
src/tests/retained_scene.rs
src/tests/virgl_integration.rs
src/tests/virtio_gpu_protocol.rs
src/tests/window_theme.rs
```

No new VirtIO-GPU control-queue request is required. `RESOURCE_COPY_REGION` is
part of the submitted classic VirGL 3D command stream, so the transport and
wire structs in `protocol.rs` should remain unchanged.

## Validation commands

Run host-independent checks first:

```sh
cargo fmt -- --check
cargo check --features test
./test.sh --skip-userland composition_cpu retained_scene window_theme \
  compositor_selection virtio_gpu_protocol window_manager_render
```

Run the pinned hardware oracle and direct-scanout fixture:

```sh
AGENTICOS_QEMU_BIN=/opt/homebrew/Cellar/qemu/1.0.27/bin/qemu-system-x86_64 \
AGENTICOS_QEMU_GL=es \
./scripts/test-virgl-integration.sh
```

Exercise strict automatic Aero with telemetry:

```sh
AGENTICOS_QEMU_BIN=/opt/homebrew/Cellar/qemu/1.0.27/bin/qemu-system-x86_64 \
AGENTICOS_QEMU_GL=es \
AGENTICOS_COMPOSITOR=gpu \
AGENTICOS_GPU_STRICT=1 \
AGENTICOS_THEME=auto \
AGENTICOS_NETWORK=off \
AGENTICOS_RENDER_STATS=1 \
./build.sh
```

Repeat with `AGENTICOS_THEME=classic` as the no-effect control, then finish
with the full host-independent suite:

```sh
./test.sh --skip-userland
```

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| Sampling the output while rendering to it causes undefined feedback | Copy the current output into scratch before every effect and sample scratch only |
| Transparent rounded corners get filled with blurred pixels | Effect shader discards zero-effective-alpha fragments; qualify exact preservation in hardware |
| Upper glass ignores lower glass | Snapshot at the effect's exact z-order position rather than recomposing a subset of layers |
| Blur reads stale pixels outside a small copy | Inflate the work region by the shared total effect halo and draw final pixels only inside damage |
| Scratch resources consume too much guest heap | Checked eager allocation, explicit working-set telemetry, atomic VirGL initialization failure |
| Generated TGSI exceeds command limits for large radii | Bound radius/instruction count and return `UnsupportedEffect`; never clamp or silently disable |
| Multiple sampler bindings leak into ordinary draws | Explicitly bind the required slot set per shader family and clear all live slots before teardown |
| Float/UNORM requantization differs from CPU integer averaging | Narrow tolerance only for blurred/translucent channels; keep opaque and ordinary alpha gates strict |
| Blur makes clean movement upload frame textures again | Keep effect state out of texture identity and retain transactional damage acknowledgement |
| Auto selects Aero before the host supports the full path | Add a readback blur gate to engine construction before changing theme selection |
| Runtime fallback reaches alpha-incapable legacy with Aero still active | Switch the active theme to Classic before the forced legacy repaint |
| Counters claim to measure GPU blur execution independently | Count commands/pixels and attribute actual host execution to the existing fenced wait bucket |

## Completion criteria

This plan is complete when the pinned VirGL host passes a deterministic
copy/ping-pong/alpha-masked-combine qualification; production
`BackdropSample` matches CPU blur semantics for uniform, impulse, partial
damage, transparent, opaque, opacity, edge, and stacked-glass scenes; Aero
radius 6 is selected automatically on qualified VirGL; steady frames reuse all
surface and blur resources; ordinary frames perform zero readback; strict and
non-strict failures remain coherent; and the qualification guide records
before/after telemetry for CPU and GPU Aero at the same resolution and scene.

## Protocol reference

The encoder addition should be derived from the pinned virglrenderer
`virgl_protocol.h`. A browsable upstream code mirror documents
`VIRGL_CCMD_RESOURCE_COPY_REGION` and its 13 payload fields:

- https://android.googlesource.com/platform/external/virglrenderer/+/68429e8e1106d0861d9f9f180583bd8381b8bf96/src/virgl_protocol.h

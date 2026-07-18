---
title: "feat: extract painting from the kernel into a ring-3 ELF (PAINTING.ELF)"
type: feat
status: completed
date: 2026-07-18
---

# feat: extract painting from the kernel into a ring-3 ELF (PAINTING.ELF)

## Summary

Move the kernel-side `painting` app (`src/commands/painting/mod.rs`) out of ring 0
into a standalone ring-3 ELF, `PAINTING.ELF`, exactly the way `notepad` was moved in
`docs/plans/2026-07-18-001-feat-ring3-gui-platform-notepad-and-userland-unification-plan.md`.
The ring-3 GUI platform that migration built (syscalls `5001–5004`, the `RemoteSurface`
window type, the per-PID GUI event queue, the `userland/runtime` libc-lite, and the
`userland/libs/gui` toolkit) already exists and is proven by `NOTEPAD.ELF` and
`GUIDEMO.ELF`. This is therefore a **"Phase 3 again"** migration: a new
`userland/apps/painting/` workspace crate, one manifest row, launch rewiring, and deletion
of the kernel module — plus one genuinely new design decision (below).

The plan explicitly named `calc`, `painting`, `tasks`, and `explorer` as follow-up
migrations that reuse the platform, each growing the toolkit with the widgets it needs.
This does the `painting` one.

---

## The one real decision: what is "painting" today, and what should it become?

Despite the name, **the current kernel `painting` app is not a drawing program**. It is a
**passive bouncing-shapes animation demo**: four hardcoded colored rectangles bounce around
a black canvas (`src/commands/painting/mod.rs:17–131`). It takes **no keyboard or mouse
input** (`handle_event` returns `Propagate` and ignores everything, `mod.rs:197–199`). It is
driven by a free-running kernel animation loop: `RunnableProcess::run` sleeps 2 PIT ticks,
advances fixed-point positions, and calls `content.invalidate()` each frame
(`mod.rs:262–290`), using `get_timer_ticks` for tick-rate-normalized velocity.

This matters because the ring-3 event model is **blocking and input-driven**. The toolkit's
`gui::next_event()` calls `gui_next_event(&event, flags=0)` — it parks the process until an
input/resize/close event arrives (`userland/libs/gui/src/lib.rs:179–181`). `GUIDEMO.ELF`
and `NOTEPAD.ELF` only ever redraw *in response to an event*; neither animates on a timer.
There is **no frame-tick GUI event**. So a faithful port of the bouncing animation cannot
just reuse the notepad event loop — it needs a self-driven clock.

Two building blocks already exist to bridge that gap, but neither is wired for animation:
- `runtime::nanosleep(&Timespec, ...)` — a real sleep syscall (`userland/runtime/src/lib.rs:226`).
- `GUI_NONBLOCK = 1` flag for `gui_next_event`, returning `-EAGAIN` on an empty queue
  (`userland/runtime/src/lib.rs:27`) — but the toolkit's `next_event()` hardcodes `flags = 0`
  and does not expose a non-blocking variant.

### Recommended path: **Option A — faithful animated port**

Keep `painting` as the bouncing-shapes animation (smallest, most faithful "migrate it"
outcome; ~115 lines of pure fixed-point math port over verbatim with zero kernel coupling).
Drive it with a **poll-and-sleep frame loop** in ring 3:

```rust
loop {
    // Drain input non-blocking (only need to notice CLOSE / RESIZE).
    while let Some(ev) = gui::try_next_event()? {   // new: passes GUI_NONBLOCK
        match ev.kind { GUI_EVENT_CLOSE => break 'outer, GUI_EVENT_RESIZE => window.resize(..), _ => {} }
    }
    advance_shapes(&mut state, dt);
    render(&mut window);            // Canvas::fill_rect per shape + present()
    runtime::nanosleep(&FRAME, None);  // ~20 ms → ~50 FPS
}
```

This requires one small toolkit addition — `gui::try_next_event() -> Result<Option<GuiEvent>, i64>`
that passes `GUI_NONBLOCK` — and nothing new in the kernel. Frame timing comes from
`nanosleep` instead of `sleep_ticks` + `get_timer_ticks`; velocity is normalized against the
chosen frame period instead of the PIT rate.

### Alternative: **Option B — make it a real interactive paint program**

Turn `painting` into an actual drawing app (mouse draws pixels/brush strokes onto a canvas,
a color palette, clear, maybe Save-as-bitmap via `openat`/`write` like notepad). This fits
the existing blocking `next_event()` loop cleanly (redraw on mouse events — no timer needed),
and it makes the app's name honest. But it is a **new feature**, not a migration: it grows the
toolkit with palette/brush widgets and needs a bitmap file format decision. It is strictly
more work and more scope than the user asked for.

**Recommendation: Option A.** It is the literal "migrate paint out of the kernel similar to
notepad" request — same behavior, now in ring 3 — with the minimum new surface (one
non-blocking toolkit helper). Option B is a good *follow-up* once the extraction has landed.
The rest of this plan assumes Option A and notes where Option B would diverge.

> **Confirm with the requester before implementing** which of the two they want, since it
> changes the app body substantially (though not the launch/build/removal wiring, which is
> identical either way).

---

## Requirements

### R1 — the ring-3 app crate

- **R1.1.** New workspace member `userland/apps/painting/` with:
  - `Cargo.toml` — `[dependencies] runtime` (path `../../runtime`) + `gui` (path `../../libs/gui`);
    `[build-dependencies] userland-build-support`. Model on `userland/apps/notepad/Cargo.toml`.
  - `build.rs` — `userland_build_support::configure("painting");` (one line, exactly like notepad).
  - `src/main.rs` — `#![no_std] #![no_main]`, `_start` + `startup_from_stack`, panic handler
    calling `runtime::exit`, and the frame loop from Option A. Port `BouncingShape` /
    `PaintingState` fixed-point math from `src/commands/painting/mod.rs:17–131` verbatim;
    replace `device.fill_rect(..)` with `Canvas::fill_rect(..)` + `Window::present()`.
- **R1.2.** Register the crate in `userland/Cargo.toml` `members`.
- **R1.3.** Add a manifest row in `userland/apps.manifest.sh` (same shape as the `notepad`/`guidemo` rows):
  ```
  app_row painting apps/painting cargo PAINTING.ELF built-every-run rust-nightly target/x86_64-unknown-none/release/painting -
  ```
  `built-every-run`, `rust-nightly`, committed-prebuilt = `-`. It is a native no_std Rust app
  that builds in seconds with the toolchain the kernel already requires, so it is **not**
  prebuilt-managed (per `userland/prebuilt/README.md`). No `build.sh`/`test.sh` edits are
  needed — staging is manifest-driven via `userland/stage-lib.sh`.

### R2 — toolkit addition (Option A only)

- **R2.1.** Add `gui::try_next_event() -> Result<Option<GuiEvent>, i64>` to
  `userland/libs/gui/src/lib.rs`, passing `runtime::GUI_NONBLOCK` and mapping `-EAGAIN` → `Ok(None)`.
  Leave the existing blocking `next_event()` untouched. (Option B needs no toolkit change for
  the loop, but would add palette/brush primitives instead.)

### R3 — launch rewiring (mirror notepad exactly)

- **R3.1.** Start menu (`src/commands/guishell/mod.rs`): change `spawn_painting()` (`:322`) to call
  `terminal_factory::spawn_gui_user_app("/host/PAINTING.ELF", vec!["painting"])` instead of the
  in-kernel `gui_launch_table::spawn_by_name("painting")`. The `PendingAction::SpawnPainting`
  variant (`:46`) and its dispatch (`:641`) stay; only the body of `spawn_painting()` changes.
  This makes the Start-menu path round-trip through ring 3, like `spawn_notepad()` (`:336`).
- **R3.2.** `/bin` namespace (`src/userland/bin_namespace.rs`): move `"painting"` **out of**
  `GUI_APPLETS` (`:53`, keep it sorted) and give it a direct rewrite like notepad —
  add `PAINTING_HOST_PATH = "/host/PAINTING.ELF"` and a `DIRECT_APPLETS` entry (`:57`) so
  `/bin/painting` rewrites straight to the ELF with `argv[0]` preserved, instead of routing
  through `GLAUNCH.ELF` → `sys_gui_launch(5000)` → the kernel launch table.
- **R3.3.** Kernel launch table (`src/commands/gui_launch_table.rs`): remove the
  `"painting" => painting::create_painting_process` arm (`:38`) and the mirrored `"painting"`
  token in the `test_every_gui_applet_dispatches` match (`:84`).

### R4 — remove the kernel module

- **R4.1.** Delete `src/commands/painting/mod.rs` and drop `pub mod painting;` from
  `src/commands/mod.rs:5`.
- **R4.2.** Remove painting's entry from the kernel test registry (`painting::get_tests`,
  aggregated via `src/tests/mod.rs`; grep for `painting`). Port the damage/animation unit tests
  into the ring-3 crate if still meaningful, or drop them.

### R5 — tests and docs

- **R5.1.** Update the kernel unit tests that reference `/bin/painting` or the applet lists so
  they stay green after R3: `test_apply_bin_rewrite_dispatches_gui_app` (repoint from
  `/bin/painting` to another GUI applet, e.g. `/bin/calc`), `test_gui_applets_are_sorted`, the
  applet-count assertions, and the `getdents64` `/bin` listing test in `bin_namespace.rs`.
  Add a rewrite test for the new direct `/bin/painting → /host/PAINTING.ELF` path (mirror the
  notepad rewrite test).
- **R5.2.** Docs: update `src/commands/CLAUDE.md` (painting is no longer kernel-side — it now
  lists `calc`, `tasks`, `explorer`), the root `CLAUDE.md` subsystem index / current-state
  paragraph (mention `PAINTING.ELF` alongside `NOTEPAD.ELF`), and `userland/README.md` if it
  enumerates apps. This plan's `status:` flips to `completed` on landing.

---

## Step-by-step implementation order

Each step compiles and boots on its own; the app and the removal are separable so the tree is
never broken.

1. **Toolkit** (Option A): add `gui::try_next_event()` (R2.1). `cargo check` the workspace.
2. **New app**: create `userland/apps/painting/` (R1.1), register in `members` (R1.2), add the
   manifest row (R1.3). Build (`./build.sh -n`) and confirm `host_share/PAINTING.ELF` is staged
   and passes the ET_EXEC validation. At this point `/bin/painting` still points at the *kernel*
   app — that's fine; the ELF is shippable and can be smoke-tested by temporarily typing its
   host path, or by finishing step 3 first.
3. **Rewire launch** (R3): Start menu body, `bin_namespace` direct rewrite, remove the launch-table
   arm. Now both Start → Painting and `painting` in zsh spawn the ring-3 ELF.
4. **Remove kernel module** (R4): delete `src/commands/painting/mod.rs`, drop the `pub mod`,
   remove from the test registry. `cargo check` — the launch-table arm is already gone so there
   are no dangling references.
5. **Fix tests + docs** (R5). Run `./test.sh bin_namespace gui_launch_table` (plus any module
   that referenced painting) and the full `./test.sh`.
6. **Manual verification** (see below).

---

## Verification

- `./test.sh` — full kernel suite green, especially `bin_namespace` (applet sort/count/getdents,
  new painting rewrite) and `gui_launch_table` (sync test no longer expects `painting`).
- `cargo check` and `cargo clippy` clean; `cargo fmt`.
- **Boot and drive it** (the `verify`/`run` flow): boot the GUI desktop, then
  - Start → Painting opens a titled `PAINTING.ELF` window with the shapes bouncing;
  - closing the window (title-bar close → `GUI_EVENT_CLOSE`) tears the process down cleanly
    (per-PID window teardown on exit — confirm no leaked window / no scheduler stall);
  - in Terminal, `painting` launches the same ELF; `/bin/painting` resolves via the direct
    rewrite (check with `which painting` / that `ls /bin` still lists it);
  - dragging the window and (Option A) `nanosleep` pacing keep animation smooth and don't peg a
    core — verify CPU/scheduler behavior since the poll loop is self-driven, unlike the blocking
    notepad loop.

---

## Risks & notes

- **Busy-spin risk (Option A).** A non-blocking poll loop with too short a `nanosleep` (or a
  missing one) will spin ring-3 and starve other processes under the single-core preemptive
  scheduler. Pick a real frame period (~16–20 ms) and always `nanosleep` each iteration. This is
  the one behavior notepad/guidemo never exercised (they block), so it deserves explicit
  scheduler-behavior verification. If poll+sleep proves unsatisfying, the principled alternative
  is a **kernel-side frame/timer GUI event** (a periodic `GUI_EVENT_TICK` enqueued to subscribed
  windows) so the app can stay on the blocking `next_event()` loop — but that is a kernel ABI
  addition and out of scope here unless the poll loop misbehaves.
- **Full-surface present only.** The ring-3 ABI presents whole surfaces via copy-blit
  (`gui_win_present`, no damage rects), unlike the kernel app's `dirty_rect_hint` incremental
  repaint. For a 400×300 canvas this is negligible; no action needed.
- **Shared launch plumbing stays.** `gui_launch_table::spawn_by_name`, `GUI_APPLETS`, and the
  kernel `Window`/`FrameWindow`/`GraphicsDevice`/compositor model remain for `calc`, `tasks`,
  and `explorer` until they migrate too. This plan removes only painting's own arm/module.
- **Explorer "open in paint"** is *not* in scope (explorer only spawns notepad today,
  `explorer/mod.rs:864`). If Option B lands later, wiring explorer to open image files in paint
  would be a natural follow-up.

## Out of scope

- Migrating `calc`, `tasks`, `explorer` (separate follow-ups, same recipe).
- A kernel `GUI_EVENT_TICK`/timer-event ABI (only revisit if the poll+sleep loop is unsatisfactory).
- Option B's palette/brush toolkit widgets and bitmap Save format (only if the requester chooses B).

## Key reference files

- Migration exemplar (Phase 3): `docs/plans/2026-07-18-001-feat-ring3-gui-platform-notepad-and-userland-unification-plan.md`
- App template: `userland/apps/notepad/{Cargo.toml,build.rs,src/main.rs}`; animated analog: `userland/apps/guidemo/src/main.rs`
- Toolkit: `userland/libs/gui/src/lib.rs` — `Canvas`, `Window`, `next_event()` (:179)
- Runtime/ABI: `userland/runtime/src/lib.rs` — `nanosleep` (:226), `GUI_NONBLOCK` (:27), `GuiEvent` (:250)
- Manifest: `userland/apps.manifest.sh`; staging: `userland/stage-lib.sh`; workspace: `userland/Cargo.toml`
- Launch wiring: `src/window/terminal_factory.rs` (`spawn_gui_user_app` :262), `src/commands/guishell/mod.rs`,
  `src/userland/bin_namespace.rs`, `src/commands/gui_launch_table.rs`
- Current kernel app to remove: `src/commands/painting/mod.rs`

---

## Implementation notes (what actually landed)

Delivered as **Option A**. Two things diverged from the pre-implementation plan:

1. **`nanosleep` was a no-op stub, so a real blocking `nanosleep` was implemented.**
   The frame loop's throttle depends on `runtime::nanosleep`, but the kernel
   handler (`syscalls.rs::nanosleep_handler`) previously returned 0 immediately
   without sleeping — so Option A's loop busy-spun and animated at thousands of
   FPS (the exact busy-spin failure this plan flagged). Rather than the deferred
   `GUI_EVENT_TICK` fallback, the smaller and more general fix was to make
   `nanosleep` actually block: a new `Ring3BlockReason::Sleeping { deadline_tick }`,
   a restart-stable per-process `sleep_deadline` (`lifecycle::nanosleep_deadline`,
   mirroring `prepare_network_wait`), and a `process_expired_sleeps()` housekeeping
   wake pass wired next to `process_due_real_timers()` in the kernel main loop and
   the inline scheduler loop. The duration is rounded up to whole 100 Hz PIT ticks
   (≥1 tick for any positive request). Signal-interruption (`-EINTR` + remaining
   time) is not modeled — a woken-but-not-elapsed sleeper simply re-blocks. Bonus:
   this also fixes zsh's previously-silent `sleep`/`usleep` builtins. Unit tests:
   `test_expired_nanosleep_wakes_blocked_process`,
   `test_unexpired_nanosleep_stays_blocked` (userland_switch), and the rewritten
   `test_dispatch_nanosleep_returns_zero_without_blocking` (userland).

2. **The `nanosleep` wake pass had to run from the compositor, not the idle
   loop.** First cut wired `process_expired_sleeps()` only into the kernel main
   loop next to `process_due_real_timers()`. That looked right but the boxes
   froze after ~2 frames: under U10 the kernel main loop is the *idle* task and
   is starved once other kernel threads (the compositor) are always ready —
   instrumentation showed it ran ~once every 5 seconds, so a sleeping animation
   loop was almost never woken. Ring-3 *dispatch* is fast (the scheduler's
   `next_runnable` pops `ring3_ready` on every context switch), but the *wake*
   was the bottleneck. Fix: also call `process_expired_sleeps()` from the
   compositor kernel-thread loop (`window::compositor::run`), which is scheduled
   every round-robin revolution. Verified live: the loop then cycles at ~51 Hz
   (block→wake→present) with steadily advancing shape positions. (The main-loop
   and inline-dispatch call sites stay for the test/launcher paths.)

3. **Toolkit `try_next_event()`** was added exactly as R2.1 specified.

The kernel `Color::GREEN/BLUE/YELLOW` constants lost their only non-test user when
`src/commands/painting/` was deleted, so they picked up the same
`#[cfg_attr(not(feature = "test"), expect(dead_code, ...))]` guard `CYAN` already
carried (release builds `#![deny(dead_code)]`).

Verified: full `./test.sh` (780 tests) green; `PAINTING.ELF` builds (ET_EXEC,
~12.8 KB) and stages; Start → Painting launches the ring-3 ELF (titled
`RemoteSurface` window, 400×300 content, focused, taskbar button, no fault, stable
over time) with the loop now blocking in `nanosleep` between frames instead of
pegging the CPU. Note: the frame close/drag interaction could not be exercised
through the RPC `send_input` bridge (synthetic `process_event` injection does not
drive the compositor's drag/close interaction-state path); that is a test-harness
limitation, not an app behavior.

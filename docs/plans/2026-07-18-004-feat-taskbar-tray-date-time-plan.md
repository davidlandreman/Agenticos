---
title: "feat: taskbar notification tray with date and time"
type: feat
status: completed
date: 2026-07-18
---

# Taskbar notification tray with date and time

## Summary

Add a Windows 98-style recessed notification tray to the right edge of the
desktop taskbar. The first tray content is a two-line UTC clock:

```text
+--------------------------------------------------------------------------+
| Start | AgenticOS Terminal | Notepad                         | 15:42 UTC |
|                                                            | 2026-07-18 |
+--------------------------------------------------------------------------+
```

The tray is a dedicated child window of the taskbar, not text painted directly
by `GUIShell`. It owns its clock state, repaints only when the displayed minute
changes, and leaves room for future notification icons. Task-window button
layout treats the tray as reserved right-side space and never overlaps it.

AgenticOS currently has only the 100 Hz PIT uptime counter. Its
`CLOCK_REALTIME` and `gettimeofday` implementations intentionally report time
since boot as though the epoch began at boot. A real date display therefore
also requires a small CMOS RTC reader and a kernel clock anchor. This plan adds
that source once, uses PIT ticks to advance it without repeatedly polling CMOS,
and makes the existing realtime syscalls agree with the taskbar clock.

## Current state

- `src/window/windows/taskbar.rs` implements a 32-pixel-high taskbar background.
  It does not paint any right-side content. Its fields for Start/window-button
  bookkeeping are currently unused because `GUIShellState` owns that policy.
- `src/commands/guishell/mod.rs::init_guishell` creates the taskbar and Start
  button. `sync_taskbar_buttons` creates one child `Button` per frame window,
  and `update_button_layout` divides all width after Start among those buttons.
  There is no reserved right edge, so adding a tray without changing this math
  would let task buttons paint underneath it.
- The compositor calls every window's `prepare_for_render` before reading dirty
  state. This is the correct hook for a clock widget to notice a minute change
  and invalidate itself without forcing continuous repainting.
- The retained and legacy renderers both consume the same `Window::paint`
  implementation. A separate tray child therefore keeps renderer-specific
  behavior out of the feature.
- `src/arch/x86_64/interrupts.rs` exposes the monotonic 100 Hz
  `get_timer_ticks()`, but there is no CMOS/RTC driver.
- `src/userland/syscalls.rs` maps both `CLOCK_REALTIME` and `CLOCK_MONOTONIC` to
  `timer_ticks * 10 ms`; `gettimeofday` does the same. The source comments call
  out RTC/NTP as the missing dependency.
- QEMU launch commands do not state an RTC base explicitly. QEMU normally
  exposes a PC-compatible CMOS RTC, but the guest should declare whether it
  interprets that clock as UTC or host local time.
- `src/fs/fat/filesystem.rs` still writes zero FAT timestamps and has a comment
  saying there is no RTC. Wiring filesystem timestamps is a separate concern;
  only that stale explanation changes in this feature.

## Product decisions

### PD1 — UTC, ISO date, and 24-hour time for v1

Display `HH:MM UTC` over `YYYY-MM-DD` and launch QEMU with an explicit UTC RTC
base. AgenticOS has no timezone database, `/etc/localtime`, user locale, or
settings UI. Silently treating an RTC value as local time would make the tray
look convenient while producing an incorrect POSIX epoch for userland.

UTC keeps the taskbar and `CLOCK_REALTIME` semantically correct. Local timezone
selection and 12/24-hour/date-format preferences are follow-ups once there is a
settings/configuration owner.

### PD2 — a dedicated `TaskbarTrayWindow` child

Add `TaskbarTrayWindow` beside `TaskbarWindow` in
`src/window/windows/taskbar.rs`. `GUIShell` registers it as a taskbar child in
the same way it registers Start and window buttons.

This boundary gives the tray:

- independent invalidation and event behavior;
- local coordinates naturally anchored within the taskbar;
- a place for future notification-icon children;
- no need for a `Window` downcast or clock state in global `GUIShellState`.

The tray is non-focusable and propagates/ignores pointer input for now. A clock
popup, tooltip, calendar, and notification interactions are not part of v1.

### PD3 — initialize wall time once, then advance it with PIT ticks

Read a stable RTC snapshot once during boot and store:

- the validated Unix timestamp from that snapshot;
- the PIT tick observed at the same point;
- whether wall time is valid.

`utc_now()` derives current time as `boot_epoch + elapsed_ticks / 100`.
`realtime_ns()` also includes the sub-second PIT remainder. The tray therefore
does not perform CMOS port I/O during rendering, and RTC access remains isolated
to architecture initialization.

There is no periodic RTC resynchronization in this unit. PIT/RTC drift over a
long VM session is acceptable for v1; an NTP or periodic calibration policy
belongs in the future system-time work.

### PD4 — RTC failure must not block boot

The CMOS reader uses bounded waits/retries and validates every field. If the
update-in-progress bit never clears, two snapshots never agree, or the decoded
calendar is invalid, boot logs one warning and continues.

In that fallback:

- the tray renders `--:-- UTC` and `----------` placeholders;
- `CLOCK_REALTIME`/`gettimeofday` retain today's uptime-from-zero behavior;
- `CLOCK_MONOTONIC` is unaffected.

No RTC error may panic, spin forever, or prevent the desktop from appearing.

### PD5 — reserve tray width in one shared geometry helper

Put taskbar geometry constants and pure helpers in `taskbar.rs`:

- taskbar/start/button dimensions already defined there;
- tray outer width, height, gap, and right/bottom inset;
- `tray_bounds(taskbar_width)` for the right-anchored child rectangle;
- `window_button_bounds(taskbar_width, button_count, index)` (or an equivalent
  range helper) for the usable span between Start and tray.

`GUIShell` calls these helpers instead of maintaining a second copy of the
arithmetic. All subtraction is saturating. On a very narrow display or with
many frame windows, buttons may collapse to zero width rather than underflow or
cross into the tray; normal display sizes retain the existing maximum button
width.

## Technical design

### Clock ownership

```text
CMOS ports 0x70/0x71
        |
        v
arch::x86_64::rtc::read_datetime()  -- stable raw snapshot + BCD/12h decode
        |
        v
time::init()                        -- UTC calendar -> boot Unix epoch
        |                                  + boot PIT tick anchor
        +---------------------+-------------------------------+
                              |                               |
                              v                               v
                 TaskbarTrayWindow::prepare_for_render   Linux time syscalls
                 compare epoch-minute, invalidate        REALTIME/gettimeofday
```

The generic `time` module owns calendar validation/conversion and clock
semantics. The architecture module owns only CMOS register access and register
encoding. UI and syscall code consume the generic API and never read ports.

### RTC snapshot rules

Add `src/arch/x86_64/rtc.rs` using `x86_64::instructions::port::Port<u8>`:

1. Enter an interrupt-disabled scope so a snapshot cannot interleave with a
   future CMOS user on this single CPU.
2. Poll Status A's update-in-progress bit with a fixed upper bound.
3. Read seconds, minutes, hours, day, month, year, optional century, and Status
   B into a raw snapshot.
4. Wait for update-in-progress to clear again and take a second snapshot.
5. Accept only two identical snapshots; retry the pair a bounded number of
   times.
6. Decode BCD unless Status B says the registers are binary. Decode the 12-hour
   PM bit unless Status B says 24-hour mode.
7. Use the century register when valid; otherwise constrain the fallback to
   years 2000-2099, which matches the supported QEMU-era deployment.
8. Re-enable NMI before leaving the CMOS index port and return an error for any
   invalid or unstable result.

Keep raw-register decoding as a pure function so BCD/binary and 12/24-hour
cases can be tested without hardware.

### Generic kernel time API

Add `src/time.rs` and register it from `src/main.rs`. The public surface stays
small:

```rust
pub struct DateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

pub fn init();
pub fn monotonic_ns() -> u64;
pub fn wall_clock_ns() -> Option<u64>;
pub fn realtime_ns() -> u64;
pub fn utc_now() -> Option<DateTime>;
```

`init()` runs once after IDT/PIT initialization and before the desktop or any
ring-3 process starts. It reads the RTC, converts the calendar to Unix seconds,
and publishes the epoch/tick anchor through atomics. Calendar helpers cover
Gregorian leap years and checked day/month ranges. Arithmetic saturates rather
than wrapping at extreme uptimes.

`wall_clock_ns()` and `utc_now()` return `None` when no RTC anchor was installed,
so the UI can distinguish an unknown date from 1970. `realtime_ns()` wraps the
optional wall clock and falls back to `monotonic_ns()`, preserving the current
syscall contract in failure cases.

### Tray presentation and refresh

`TaskbarTrayWindow` owns:

- `WindowBase`;
- cached time/date strings;
- `last_displayed_minute: Option<u64>`.

Its `prepare_for_render` reads the cheap tick-derived clock and compares the
current Unix minute to the cached minute. Only a change formats new strings and
calls `invalidate()`. Seconds are intentionally omitted, so an idle desktop
does no clock rasterization between minute boundaries.

`paint` draws:

1. ButtonFace background matching the taskbar;
2. a recessed classic bevel (shadow on top/left, highlight on bottom/right);
3. centered `HH:MM UTC` and `YYYY-MM-DD` lines using the existing 11 px caption
   font so both lines fit inside the 32 px taskbar.

Text remains within the inner bevel after clipping at the smallest supported
normal geometry. The implementation clears repaint state through the normal
`Window` contract and is identical under legacy, retained CPU, and VirGL
composition.

### `GUIShell` wiring and button layout

During `init_guishell`, immediately after creating Start:

1. allocate a tray window ID;
2. construct the tray with `tray_bounds(width)` in taskbar-local coordinates;
3. set the taskbar as parent, register it, and add it to taskbar children;
4. invalidate it as part of the initial desktop paint.

`GUIShellState` may retain `tray_id` for diagnostics and teardown symmetry, but
it does not update the clock. The tray and buttons are geometrically disjoint,
so correctness does not depend on child insertion order.

Refactor `add_window_button` and `update_button_layout` to call the shared
button-range helper. Every relayout subtracts the fixed tray reservation before
dividing available width among frame buttons.

### Userland realtime consistency

Move the private monotonic calculation out of `src/userland/syscalls.rs` and
use the generic clock:

- `CLOCK_MONOTONIC` -> `crate::time::monotonic_ns()`;
- `CLOCK_REALTIME` -> `crate::time::realtime_ns()`;
- `gettimeofday` -> `crate::time::realtime_ns()`.

ITIMER_REAL, nanosleep, polling timeouts, scheduler sleeps, and watchdog logic
continue to use absolute PIT ticks. They are duration/deadline mechanisms and
must not become sensitive to RTC corrections.

FAT create/modify timestamps remain zero. The existing comment changes from
“no RTC” to “filesystem timestamp wiring is deferred” so the source accurately
describes the remaining gap without expanding this task into persistence
semantics.

## Implementation units

### U1. CMOS RTC reader and pure decoder

Files:

- Create `src/arch/x86_64/rtc.rs`.
- Modify `src/arch/x86_64/mod.rs`.
- Modify `src/arch/x86_64/CLAUDE.md`.

Implement bounded stable-snapshot reads, NMI/interrupt restoration, BCD and
binary modes, 12/24-hour conversion, century fallback, and field validation.
Expose only a decoded calendar result/error to the generic clock layer.

### U2. Kernel clock anchor and realtime syscall integration

Files:

- Create `src/time.rs`.
- Modify `src/main.rs`.
- Modify `src/kernel.rs`.
- Modify `src/userland/syscalls.rs`.
- Modify `src/fs/fat/filesystem.rs` (comment only).
- Modify `src/userland/CLAUDE.md`.

Add Gregorian calendar/Unix conversion, initialize the RTC/PIT anchor at boot,
and split realtime from monotonic syscall behavior. Keep an explicit no-RTC
fallback.

### U3. Taskbar tray widget and shared geometry

Files:

- Modify `src/window/windows/taskbar.rs`.
- Modify `src/window/windows/mod.rs`.
- Modify `src/commands/guishell/mod.rs`.

Add the recessed two-line tray widget, minute-boundary refresh, tray creation,
and a single source of truth for tray/task-button geometry. Preserve the current
Start button location and taskbar height.

### U4. QEMU RTC contract, tests, and documentation

Files:

- Modify `build.sh`.
- Modify `test.sh`.
- Create `src/tests/time.rs`.
- Create `src/tests/taskbar_tests.rs`.
- Modify `src/tests/mod.rs`.
- Modify `src/window/CLAUDE.md`.
- Modify root `CLAUDE.md` and `docs/window_system_design.md` where they describe
  the default desktop/time support.

Pass `-rtc base=utc` in interactive and test QEMU configurations. Add focused
clock/geometry tests and document the tray and realtime source.

## Automated verification

### Clock and RTC tests (`./test.sh time`)

1. BCD 24-hour snapshot decodes to the expected calendar.
2. Binary 24-hour snapshot decodes unchanged.
3. 12 AM, 12 PM, and a non-noon PM hour decode correctly.
4. A valid century register wins; missing/invalid century uses 2000-2099.
5. Invalid month/day/hour values are rejected.
6. Leap-day validation accepts 2000-02-29 and 2024-02-29, rejects 2100-02-29.
7. Calendar -> Unix seconds -> calendar round trips across epoch, leap day,
   year-end, and the current century.
8. PIT-derived realtime carries seconds/minutes/dates across midnight and
   saturates rather than wrapping.
9. No-RTC state leaves monotonic functional, makes `utc_now()` return `None`,
   and gives realtime the documented monotonic fallback.

The hardware port loop itself gets a QEMU smoke assertion that a snapshot
returns a valid date range; exact wall time is not asserted because tests can
cross a second/minute while running.

### Taskbar tests (`./test.sh taskbar`)

1. Tray bounds are right-anchored with the documented inset at common screen
   widths.
2. Start bounds, window-button range, and tray bounds are pairwise disjoint.
3. One and many task buttons divide only the middle span and respect
   `MAX_WINDOW_BUTTON_WIDTH`.
4. Very narrow widths and excessive button counts use saturating geometry and
   never underflow or cross the tray's left edge.
5. UTC formatting is fixed-width and produces `HH:MM UTC` / `YYYY-MM-DD`,
   including leading zeroes and midnight.
6. Repeated preparation within one minute does not invalidate again; advancing
   to the next minute does; a date rollover updates both lines.
7. Unknown wall time produces the two placeholder lines.

If the test graphics helper can be reused without coupling test modules, add a
key-pixel test for the recessed bevel and text staying inside the tray. Geometry
and refresh behavior are mandatory even if the font rasterization remains a
manual visual check.

### Commands

```sh
cargo fmt --check
cargo check
cargo clippy
./test.sh time taskbar
./test.sh userland
./test.sh
```

## Manual QEMU acceptance

Boot both principal renderer paths:

```sh
AGENTICOS_THEME=classic AGENTICOS_COMPOSITOR=legacy ./build.sh
AGENTICOS_THEME=classic AGENTICOS_COMPOSITOR=retained ./build.sh
```

Verify:

- A recessed tray is flush-right inside the taskbar and shows the current UTC
  time and date with no clipping.
- The displayed minute agrees with the host's UTC clock and rolls over without
  input.
- Opening Terminal, Notepad, Painting, and Calc adds task buttons only between
  Start and the tray; buttons resize but never cover the tray.
- Closing/focusing windows preserves tray placement and clock updates.
- Start menu, Run dialog, frame dragging, and mouse routing behave unchanged.
- With no other damage, the desktop remains idle between minute transitions;
  one tray update does not repaint continuously.
- Legacy and retained rendering show the same geometry and bevel direction.
- In zsh, `date -u` agrees with the taskbar, while sleep/ping/animation timing
  remains monotonic and unchanged.

## Risks and mitigations

- **CMOS can change during a multi-register read.** Wait out update-in-progress
  and require two equal snapshots before accepting data.
- **A broken/emulated RTC can leave update-in-progress stuck.** Every poll and
  retry is bounded; failure degrades to placeholders and the existing realtime
  fallback.
- **BCD, 12-hour mode, and the PM bit are easy to misdecode.** Keep decoding
  pure and cover the mode matrix with fixed snapshots.
- **CMOS index writes can accidentally leave NMI disabled.** Contain access in
  one helper and restore the NMI-enabled index state on every return path.
- **RTC timezone ambiguity can make POSIX epoch wrong.** Declare UTC explicitly
  in QEMU and label the tray `UTC`; defer local-time display until timezone
  configuration exists.
- **Task buttons can overlap or starve the tray on narrow screens.** Centralize
  all geometry in saturating pure helpers and test the degenerate widths.
- **A clock widget can defeat compositor idle behavior.** Cache the displayed
  epoch-minute and invalidate only on a minute transition; do not show seconds.
- **Wall-clock integration can accidentally alter duration timers.** Restrict
  RTC time to realtime/gettimeofday/UI. Keep scheduler, sleep, poll, watchdog,
  and interval-timer deadlines on PIT ticks.

## Out of scope / follow-ups

- Local timezone database/configuration, daylight-saving rules, locale-aware
  date ordering, and 12/24-hour user preferences.
- Notification icons, overflow handling, volume/network indicators, tooltips,
  a calendar popup, or clickable clock behavior.
- RTC writes, user-settable system time, `settimeofday`, NTP synchronization,
  drift calibration, or suspend/resume correction.
- FAT create/modify timestamps and `utimensat` persistence semantics.
- A full taskbar visual rewrite, quick-launch area, pressed/active task-button
  styling, auto-hide, multi-row taskbars, or multi-monitor placement.

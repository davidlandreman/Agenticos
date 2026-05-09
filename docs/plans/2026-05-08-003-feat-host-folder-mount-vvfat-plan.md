---
title: "feat: Mount a host folder via QEMU vvfat at /host"
type: feat
status: active
created: 2026-05-08
depth: standard
---

# feat: Mount a host folder via QEMU vvfat at /host

## Summary

Make a folder on the developer's macOS host visible inside AgenticOS at `/host`, read-only, with zero new driver code. We achieve this by attaching a second QEMU drive that uses QEMU's `vvfat` block driver: QEMU synthesizes a virtual FAT16 filesystem from a host directory, and the existing `src/fs/fat/` driver reads it like any other disk.

The bulk of the work is not the vvfat side — it's a small refactor of `src/fs/vfs.rs::auto_mount()` to support more than one mounted FAT instance, plus extending `src/kernel.rs::init_filesystems()` to scan the second IDE drive and auto-mount it at `/host`.

---

## Problem Frame

Today there is no way to get files from the developer's Mac into the running AgenticOS guest without rebuilding the bundled BIOS image. That is friction whenever someone wants to stage a new FAT-driver fixture, bundle a sample image for the painting app, or seed a fresh config file before boot. The repo has a clean VFS mount-point abstraction but only ever uses one mount. We want a second mount, populated from a host directory, picked up at QEMU start.

Important constraint inherent to this design: every iteration is **rebuild + reboot**. vvfat snapshots the directory listing when QEMU launches, so changes to the host folder while the guest is running are not reflected. This is acceptable for the staging-style workflows above; it is not a substitute for live edit-test loops. If live edits become important, the natural follow-up is virtio-9p (captured under Deferred).

## Scope

### In scope
- A new default host-share directory in the repo (`host_share/`) that is mounted at `/host` when `./build.sh` runs.
- An environment variable to override the host path (so users can point at any folder on disk).
- Refactor of single-FAT-instance assumptions in `src/fs/vfs.rs` so two FAT mounts (`/` and `/host`) coexist.
- Boot-time detection of the second IDE drive (Primary Slave) in `src/kernel.rs::init_filesystems()`.
- Docs in `README.md` and `src/fs/CLAUDE.md` covering the feature, the 8.3 / uppercase filename constraint, and the snapshot-at-boot directory-listing limitation.
- Tests that exercise mount presence and file reads from `/host`.

### Deferred to Follow-Up Work
- A `mount` shell command for runtime introspection of mounts (handy but not required for this feature).
- Subdirectory support inside `/host` — the FAT driver's "no subdirectory traversal" limitation (per `src/fs/CLAUDE.md`) applies here too. Files at the top of the host folder work; nested directories are a separate piece of work in `src/fs/fat/`.
- Long-filename / lowercase support: vvfat synthesizes 8.3 names with VFAT LFN entries; we will see only the 8.3 form until LFN parsing is added.
- Read-write support. vvfat's `fat:rw:` mode has known data-corruption bugs and the kernel FAT stack is read-only by design today; both sides would need to change. Out of scope.
- A live-sync mechanism such as virtio-9p that would fix the snapshot-at-boot directory-listing limitation. Captured here as the natural next step if the limitation bites.

### Out of scope (not part of this product direction)
- Mounting non-FAT host filesystems.
- Any host-side daemon, sync agent, or write-back mechanism.

---

## Key Technical Decisions

**D1. Use QEMU `vvfat` as the transport.** The alternative (writing a virtio-9p client) is real OS-dev work, ~500–800 lines of guest code minimum, plus a new `Filesystem` impl. vvfat is one QEMU flag and reuses every line of `src/fs/fat/`. We accept the snapshot-at-boot directory-listing constraint in exchange for the radically smaller footprint. virtio-9p is the future track when live directory updates become important.

**D2. Read-only only.** vvfat's `fat:rw:` mode has multi-year-old data-corruption bugs in QEMU (see external research notes) and the kernel FAT stack itself is read-only. Locking this in eliminates an entire class of risk.

**D3. Use Primary Slave for the host disk, not Secondary Master.** Both work in QEMU. Primary Slave keeps both disks on the same IRQ/channel pair the IDE driver already serves and minimizes new probing surface. The TODO at `src/kernel.rs:269` already calls out this slot.

**D4. Default host path is `host_share/` at the repo root, gitignored except for a `.gitkeep`.** Convention over configuration: contributors get the feature working without arguments. `AGENTICOS_HOST_SHARE` env var overrides. If the directory does not exist, `build.sh` creates it (empty) before launching QEMU so vvfat does not error.

**D5. Mount path is `/host` (uppercase `/HOST` is **not** required).** The VFS routes by exact prefix match on the mount path string; the case-sensitivity gotcha applies inside paths under a mount, not to the mount path itself.

**D6. Generalize `auto_mount`'s singleton storage to a fixed-size array of FAT instances rather than introducing dynamic allocation.** The kernel already uses static slots elsewhere (`PARTITION_DEVICES: [Option<...>; 4]` in `src/kernel.rs:115`). Match that style. Size = 4 slots (root + host + headroom).

**D7. Graceful absence.** If the second drive is not present (someone running QEMU manually without the vvfat flag, or `./build.sh` invoked with a flag we add to skip it), `init_filesystems()` logs and continues. `/host` simply does not appear in the mount list. Nothing else fails.

---

## High-Level Technical Design

This sketch illustrates the intended approach and is directional guidance for review, not implementation specification.

```
                            ┌──────────────────────────────┐
  Mac host directory  ─────▶│  QEMU vvfat block driver     │  synthesizes FAT16
  (e.g. ./host_share/)      │  (host-side, in QEMU itself) │  on the fly
                            └──────────────┬───────────────┘
                                           ▼
                            ┌──────────────────────────────┐
                            │  IDE Primary Slave (disk #2) │  appears as a normal
                            │  src/drivers/ide.rs          │  PIO ATA drive
                            └──────────────┬───────────────┘
                                           ▼
                            ┌──────────────────────────────┐
                            │  src/fs/fat/ (existing)      │  reads boot sector,
                            │  detects FAT16, parses BPB   │  walks FAT, reads dirs
                            └──────────────┬───────────────┘
                                           ▼
                            ┌──────────────────────────────┐
                            │  src/fs/vfs.rs               │  mount("/host", fat,
                            │  generalized auto_mount      │  device) into slot[1]
                            └──────────────┬───────────────┘
                                           ▼
                                  ls /host, cat /host/X.TXT
                                  routed via existing VFS
                                  longest-prefix lookup
```

The host path crosses **one** new boundary (a second QEMU `-drive` flag). Everything below the IDE driver is unchanged. The only kernel-side novelty is "we now need to mount more than one FAT at a time."

---

## Implementation Units

### U1. Add a default host-share directory to the repo

**Goal:** Establish `host_share/` as the convention so `./build.sh` works out of the box.

**Requirements:** D4.

**Dependencies:** none.

**Files:**
- `host_share/.gitkeep` — new, empty.
- `.gitignore` — add `host_share/*` and `!host_share/.gitkeep` and `!host_share/HELLO.TXT` and `!host_share/HOST.TXT`.
- `host_share/HELLO.TXT` — uppercase 8.3 seed fixture containing a one-line "Hello from the host" message. Used by U5 Test 2 as a known-good file the kernel can address by name. Must be uppercase 8.3 because the FAT driver does not parse LFN entries (per `src/fs/CLAUDE.md`); pure-uppercase 6.3 is the safe shape.
- `host_share/HOST.TXT` — short prose file describing what `host_share/` is for, that it is a development-only folder, that secrets do not belong here, and that filenames must be uppercase 8.3 to be visible from the guest. Replaces the README that a contributor would otherwise look for.

**Approach:** Tiny, mechanical. The two seed files live inside the share so they ship into the guest as smoke-test fixtures.

**Verification:** `git status` shows `host_share/.gitkeep`, `host_share/HELLO.TXT`, and `host_share/HOST.TXT` tracked; everything else under `host_share/` is ignored. `./build.sh -n` does not error if no other files are present.

**Test scenarios:** none — pure scaffolding, no behavior change. (Test expectation: none — repo scaffolding only.)

---

### U2. Wire vvfat into `build.sh` (and `test.sh`)

**Goal:** Add a second `-drive` to QEMU's command line that exposes `host_share/` (or the env-var override) as a virtual FAT16 disk on Primary Slave.

**Requirements:** D1, D2, D3, D4, D7.

**Dependencies:** U1.

**Files:**
- `build.sh` — modify the QEMU launch block (currently lines 87–95).
- `test.sh` — add the same `-drive` so tests run with the same topology as interactive boots; tests should not require a host folder to exist beyond an empty default.
- `README.md` — short "Host folder mount" subsection describing the feature, the env var, and the 8.3 / uppercase / snapshot-at-boot caveats.

**Approach:**
- Resolve host path: `HOST_SHARE="${AGENTICOS_HOST_SHARE:-$(pwd)/host_share}"`.
- `mkdir -p "$HOST_SHARE"` so vvfat does not fail when the folder is missing.
- Append to the qemu-system-x86_64 invocation:
  - `-drive file=fat:ro:"$HOST_SHARE",format=raw,if=ide,index=1` — `if=ide,index=1` puts it on Primary Slave. Use `ro:` (read-only) form, never `rw:`.
- Echo the resolved path before launching so the developer sees what was attached.

**Patterns to follow:** the existing `BIOS_IMAGE` env-var pattern in `build.sh:88`.

**Verification:** Boot the kernel; serial log shows the second IDE drive being detected (existing IDE probe logs in `src/kernel.rs::init_filesystems()` will fire once U3 lands). Running `./test.sh` continues to pass.

**Test scenarios:**
- `AGENTICOS_HOST_SHARE` unset, default folder present and empty → QEMU launches without error, second drive attached.
- `AGENTICOS_HOST_SHARE` set to a folder with a known file (`HELLO.TXT`) → QEMU launches, drive attached.
- `AGENTICOS_HOST_SHARE` set to a path that does not exist → `build.sh` creates an empty directory there before launching (or fails with a clear message; pick one and document it).
- (Test expectation: shell-level, exercised manually and by U5's in-kernel tests, which assume U2 is wired.)

---

### U3. Generalize `auto_mount` to support multiple FAT instances

**Goal:** Replace the single `MOUNTED_FAT_FS` / `MOUNTED_FAT_WRAPPER` `Option`s in `src/fs/vfs.rs` with fixed-size arrays so two (or more) FAT mounts can coexist.

**Requirements:** D6.

**Dependencies:** none (independent refactor — could land before or alongside U2).

**Files:**
- `src/fs/vfs.rs` — modify the static storage (currently lines 8–9) and the `auto_mount` body (lines 119–175).
- `src/fs/CLAUDE.md` — note the new "multiple FAT mounts supported" capability, and add a brief gotcha about uppercase 8.3 filenames now also applying to host-mounted folders.

**Approach:**
- Replace:
  ```
  static mut MOUNTED_FAT_FS: Option<FatFilesystem<'static>> = None;
  static mut MOUNTED_FAT_WRAPPER: Option<FatFilesystemWrapper<'static>> = None;
  ```
  with:
  ```
  static mut MOUNTED_FAT_FS: [Option<FatFilesystem<'static>>; 4] = [None, None, None, None];
  static mut MOUNTED_FAT_WRAPPER: [Option<FatFilesystemWrapper<'static>>; 4] = [None, None, None, None];
  ```
  Match the existing `[Option<...>; 4]` style in `src/kernel.rs:115`.
- `auto_mount` finds the next free slot, transmutes the lifetime as today, populates both arrays at the same index, then calls `vfs.mount(...)` with a stable reference into the wrapper slot.
- Return `FilesystemError::DiskFull` (or a more descriptive new variant if cheap) when all slots are taken — mirrors the existing error path in `vfs.mount`.
- Keep the unsafe transmute pattern that exists today; do not try to remove it as part of this unit. That is a separate piece of work.

**Patterns to follow:** existing `PARTITION_DEVICES` static-array pattern in `src/kernel.rs:115`.

**Verification:** With U2 and U3 both in place, the boot log shows two successful mounts: `/` and `/host`. With U3 alone, behavior is unchanged when only one mount happens (slot[0] is used).

**Test scenarios:**
- Mount one FAT — succeeds, slot[0] populated.
- Mount two FATs at distinct paths — both succeed, slots[0] and [1] populated, both findable via `vfs.find_filesystem()`.
- Mount four FATs — all succeed.
- Mount a fifth — returns the documented "no more FAT slots" error; existing four still work.
- Attempt to mount two FATs at the same path — second one returns `AlreadyExists` (existing behavior, must remain).
- Read a file through each of two distinct mounts — content matches the underlying device. (Covers the central concern: the per-mount wrapper references must not alias each other.)

---

### U4. Detect and mount the second IDE drive at `/host`

**Goal:** Extend `src/kernel.rs::init_filesystems()` to probe Primary Slave, find the FAT16 partition that vvfat exposes there, and mount it at `/host`.

**Requirements:** D3, D5, D7.

**Dependencies:** U3 (multi-mount support must exist first).

**Critical correction from initial draft:** vvfat does **not** present an unpartitioned disk. It synthesizes a real MBR (signature `0x55AA` at offset 510) with a single partition entry pointing to a FAT16 region starting around LBA 63. The boot sector's BPB lives *inside* that partition, not at LBA 0. This unit must therefore mirror the **partition-table branch** at `src/kernel.rs:153–202`, not the whole-disk-detect branch.

**Files:**
- `src/kernel.rs` — modify `init_filesystems()` (currently runs from line 117, with the relevant TODO at line 269). Add a static slot for the host IDE block device mirroring `PRIMARY_MASTER_DISK` at line 114, plus a second partition-devices array so the host disk's partitions don't collide with the root disk's `PARTITION_DEVICES[0..3]` at line 115.
- `src/kernel.rs` — add a private helper `try_mount_host_disk()` that runs the partition-table flow against Primary Slave and mounts the first FAT partition at `/host`. The existing root-disk code stays as-is; do not refactor both onto a shared helper in this unit (that's a tempting but separate cleanup).

**Approach:**
- Add new statics next to the existing ones:
  ```
  static mut PRIMARY_SLAVE_DISK: Option<IdeBlockDevice> = None;
  static mut HOST_PARTITION_DEVICES: [Option<PartitionBlockDevice<'static>>; 4] = [None, None, None, None];
  ```
  The separate `HOST_PARTITION_DEVICES` array is required because the existing `PARTITION_DEVICES` is owned by the root-disk code path and reusing it would alias references with different lifetimes against the same slots.
- After the existing primary-master block finishes (after line 268), attempt the same flow for `IdeChannel::Primary` / `IdeDrive::Slave`:
  1. `IDE_CONTROLLER.get_disk_info(Primary, Slave)` — bail quietly if the drive isn't there (D7).
  2. Construct an `IdeBlockDevice`, store it in `PRIMARY_SLAVE_DISK`, take a `'static` reference.
  3. Read the boot sector. Verify the `0x55AA` signature. (vvfat always provides one; if absent, log and bail.)
  4. Call `read_partitions(host_disk)`. Iterate the returned partitions, store each in `HOST_PARTITION_DEVICES[i]`, run `detect_filesystem` on each.
  5. Find the **first FAT12/16/32 partition** and call `auto_mount(host_part_device, "/host")`.
  6. Log clearly which partition was selected.
- On any failure (no drive, no MBR signature, no FAT partition, mount error) log at info/debug and continue. Do not panic, do not propagate.

**Patterns to follow:** the existing primary-master + partition-table branch at `src/kernel.rs:153–202`. The whole-disk-detect branch (lines 235–259) is **not** the right precedent for vvfat and is explicitly the wrong choice.

**Verification:** With U2 + U3 + U4 wired, booting the kernel produces serial log lines along the lines of:
- `Found IDE disk: QEMU HARDDISK ... ` (or whatever vvfat reports as model — log it for the implementer to observe)
- `Valid boot sector signature found` (on Primary Slave)
- `Partition 1: Type=Fat16, Start=63, Size=...`
- `Detected filesystem: Fat16`
- `Mounted FAT filesystem at /host`

When the second drive is absent the boot proceeds with a single root mount and a debug-level "No IDE disk found on primary slave" line.

**Test scenarios:**
- Second drive absent — boot completes; only `/` is mounted. (No regression on the existing single-disk path.)
- Second drive present, host folder empty (only the seed files from U1) — `/host` mounts; `vfs.list_mounts()` reports two entries with paths `/` and `/host`.
- Second drive present, host folder contains `HELLO.TXT` (uppercase 8.3) — `/host` mounts; reading `/host/HELLO.TXT` returns expected contents.
- Second drive present, host folder contains a file with a lowercase or long name — file is either invisible (current FAT driver ignores LFN) or visible only via its 8.3 alias; either way no panic, no boot failure. Document the observed behavior in `src/fs/CLAUDE.md` once seen.
- Second drive present, host folder contains a subdirectory — directory entry is visible at the top level but traversal fails per the current FAT limitation; does not crash.
- vvfat partition table sanity check: log of `Partition N: Type=Fat16, Start=...` shows a Fat16 partition exists. If this fails, the assumption that vvfat synthesizes an MBR has changed in the installed QEMU version and U4 needs revisiting.

---

### U5. In-kernel tests for the host mount

**Goal:** Lock in the boot-time invariants so future refactors do not silently break the host mount.

**Requirements:** verifies D5, D6, D7 indirectly.

**Dependencies:** U1, U2, U3, U4.

**Files:**
- `src/tests/filesystem.rs` — extend the existing topic-organized filesystem test module with three new test functions (registered via the same `get_tests()` already exported from this file). Per `src/tests/CLAUDE.md` the convention is topic files (`basic.rs`, `memory.rs`, `filesystem.rs`), not per-feature files, so the host-mount tests live alongside the existing FAT tests.
- `host_share/HELLO.TXT` (already created in U1) is the test fixture. Tests assume `/host/HELLO.TXT` exists when the default share is in use — uppercase 8.3 by construction so the FAT driver can address it by exact name without depending on vvfat's LFN-alias heuristics.

**Approach:**
- Test 1 — `host_mount_present`: call `vfs.list_mounts()`, assert at least one entry with path `"/host"`.
- Test 2 — `host_mount_can_open_seed_file`: open `/host/HELLO.TXT` (the seed file from U1) and assert it reads non-empty content. Document in a code comment that the fixture must be uppercase 8.3 and that overriding `AGENTICOS_HOST_SHARE` to a folder without `HELLO.TXT` will skip-or-fail this test (pick one and document it; the simpler choice is fail-loud so a misconfigured CI surface is obvious).
- Test 3 — `host_mount_does_not_break_root`: read a known file from `/` (whatever is already used by other tests) to confirm the multi-mount refactor in U3 did not regress the root mount.
- Tests run via `./test.sh` per `.claude/rules/testing-flow.md`. Exit code 33 = pass, 35 = fail.

**Patterns to follow:** existing test modules under `src/tests/`. See `src/tests/CLAUDE.md`.

**Verification:** `./test.sh` reports all three tests passing; exit code 33.

**Test scenarios:** the unit *is* the test scenarios. Each named test above is an enumeration entry.

---

## System-Wide Impact

- **Build flow.** `./build.sh` and `./test.sh` now require a host folder to exist (auto-created if missing). No new toolchain dependencies; vvfat ships with QEMU.
- **VFS lifetime/aliasing.** `src/fs/vfs.rs::auto_mount` was originally written assuming a single mount. The array refactor in U3 changes the static storage shape but keeps the same `&'static`-via-transmute pattern. Anyone touching this file in the future needs to know that mounted wrappers and devices must outlive the kernel.
- **Second partition-devices array.** U4 introduces `HOST_PARTITION_DEVICES` alongside the existing `PARTITION_DEVICES` rather than sharing one array. They could be unified later, but doing so now means rewriting the root-disk code path that the rest of the kernel already depends on; the duplicate-static cost is small (4 slots × `Option<PartitionBlockDevice>`) and isolates the host-mount work from regression risk on the root mount.
- **Filename surface area.** Users will encounter the 8.3 / uppercase / no-LFN limitation more visibly because they will be staging arbitrary files. Documented in `README.md` and `src/fs/CLAUDE.md` as part of U2 / U3.
- **Boot-snapshot semantics.** Because vvfat snapshots the directory listing at QEMU start, adding or removing a file on the host while the guest runs will not be reflected. This is the central tradeoff of choosing vvfat over virtio-9p; called out in docs.
- **Read-only invariant preserved.** No write paths added, no risk to existing data integrity assumptions.

## Risks

- **Lifetime transmute in `auto_mount` getting more dangerous.** The array refactor multiplies the existing unsafe-lifetime pattern by 4. Mitigation: keep the static-slot pattern strictly mechanical; do not start using `Box::leak` or the heap; any future cleanup of the transmute lives in a separate plan.
- **vvfat geometry surprises.** vvfat presents a fixed virtual size (default 504 MiB FAT16). The IDE driver in `src/drivers/ide.rs` should handle that fine — it already supports up to 4 drives — but if anything in the FAT detection path assumes specific geometry we will see it during U4. Plan-time mitigation: U4 verification step explicitly walks the boot log; if an unexpected detection failure shows up the fix lands in U4, not silently elsewhere.
- **Host folder accidentally containing sensitive files.** Because `host_share/` is gitignored, contributors might drop secrets there for testing and forget. Mitigation: a one-line note in `host_share/HOST.TXT` (created in U1) reminding readers the folder is gitignored and is a development tool only.
- **Snapshot-at-boot confusion.** New users will expect live updates. Mitigation: the README section is explicit about it, and we name the future track ("virtio-9p later") in Scope > Deferred.

## Verification

- `./build.sh` boots the kernel with two drives attached; serial log shows both mounts succeeding.
- `./test.sh` exits with code 33; new host-mount tests pass.
- Manual smoke: drop `HELLO.TXT` (uppercase 8.3) into `host_share/` on the Mac, rebuild, run `cat /host/HELLO.TXT` in the guest shell, see the expected contents.
- Negative path: temporarily remove the second `-drive` from `build.sh`, rebuild, confirm the kernel still boots cleanly with only `/` mounted.

# Crash diagnostics

AgenticOS always compiles a minimal, allocation-free crash capsule core and a
128-record per-CPU flight recorder. Rich modes are selected at launch:

```sh
AGENTICOS_DIAGNOSTICS=record ./build.sh
AGENTICOS_DIAGNOSTICS=strict ./test.sh diagnostics
```

`record` expands each CPU ring to 1,024 records and records scheduler-shadow
violations without stopping the guest. `strict` escalates the first scheduler
invariant violation into an invariant crash capsule. Ordinary launches remain
`minimal` and do not attach a host debugcon file.

Rich launches write `.context/crashes/<run-id>/manifest.json`, `capsule.bin`,
and `kernel.elf.ref`. Decode a completed stream with:

```sh
python3 tools/crash_decode.py .context/crashes/<run-id>/capsule.bin \
  --manifest .context/crashes/<run-id>/manifest.json \
  --elf target/x86_64-unknown-none/release/agenticos
```

The decoder validates header, payload, and per-section CRCs before producing
`report.json` and `report.md`. It treats unknown sections as forward-compatible,
labels duplicate or missing evidence explicitly, and will not trust symbols
unless the build ID and ELF hash match the manifest.

Run expected-fatal smoke cases with:

```sh
scripts/test-crash-diagnostics.sh panic
scripts/test-crash-diagnostics.sh missing-cpu
scripts/test-crash-diagnostics.sh sched-duplicate
```

The harness requires a non-success QEMU exit, a complete decodable capsule,
matching run/build identity, and no missing required section. Normal SMP=4
crashes must capture all four CPUs. `missing-cpu` deliberately withholds one
NMI acknowledgement and requires a bounded, valid partial capsule instead of
a hang. `sched-duplicate` requires strict mode to report `SCHED-001` as the
first invariant.

## Crash-path rules

- Never allocate, format text, touch the filesystem/display, or acquire a
  production lock after crash ownership is elected.
- The first owner writes the capsule; nested entrants only increment the
  secondary marker and halt/exit.
- The owner captures itself, broadcasts a panic NMI, and waits only for a
  bounded TSC/spin budget. Remote CPUs snapshot on their private panic IST and
  halt without taking production locks.
- Recorder hooks accept integers only. Place them beside the production commit
  they describe, not before it.
- A CPU trace slot is readable only when its release-published sequence remains
  stable across the copy.
- Decoder inferences remain separate from capsule facts. Missing evidence is
  not evidence that a subsystem was healthy.

Lazy file page-in now follows a private-frame commit protocol: allocate and
zero privately, perform an exact-length read, revalidate the L4/VMA, and then
install the present leaf. Signals stay pending while a kernel block-I/O
continuation is suspended; only its exact completed token may wake it.

## Scheduler shadow

The production scheduler enables one fixed-capacity, crash-readable shadow
namespace. Isolated `Scheduler` values used in unit tests do not join that
namespace. Each mutation publishes an odd transition epoch with a pending
operation/subject, then commits an even epoch. The capsule therefore shows
whether a crash interrupted a transition as well as the last committed state
of every observed entity.

Current invariant ownership is `0x01xx_xxxx` (`SCHED-*`). In record mode the
first violation remains latched for later inspection; in strict mode the same
first violation is the crash signature. Never clear or overwrite the latch to
make a later symptom appear first.

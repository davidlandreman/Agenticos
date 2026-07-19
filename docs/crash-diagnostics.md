# Crash diagnostics

AgenticOS always compiles a minimal, allocation-free crash capsule core and a
128-record per-CPU flight recorder. Rich modes are selected at launch:

```sh
AGENTICOS_DIAGNOSTICS=record ./build.sh
AGENTICOS_DIAGNOSTICS=strict ./test.sh diagnostics
```

`record` expands each CPU ring to 1,024 records. `strict` currently selects
the same recorder plus the strict policy bit; shadow domains can use
`diagnostics::shadow::latch` as their transition hooks land. Ordinary launches
remain `minimal` and do not attach a host debugcon file.

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

Run the real expected-fatal smoke case with
`scripts/test-crash-diagnostics.sh panic`. The harness requires a non-success
QEMU exit, a complete decodable capsule, matching run/build identity, and no
missing required section.

## Crash-path rules

- Never allocate, format text, touch the filesystem/display, or acquire a
  production lock after crash ownership is elected.
- The first owner writes the capsule; nested entrants only increment the
  secondary marker and halt/exit.
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

# `src/tests/` — Kernel Test Authoring

This folder holds in-kernel test modules that run under QEMU when the kernel is built with `--features test`. For *running* tests and exit-code semantics, see `.claude/rules/testing-flow.md`.

## Key files (organized by topic)

- `basic.rs` — sanity tests.
- `memory.rs` — memory subsystem tests.
- `vm.rs` — VMA ordering, splitting, protection coverage, gap reuse, and reserved-hole tests.
- `heap.rs` — heap allocator and dynamic allocation tests.
- `arc.rs` — `Arc` / `Weak` reference counting tests.
- `display.rs` — display and graphics tests.
- `interrupts.rs` — interrupt handler tests.
- `filesystem.rs` — filesystem tests.
- `compiler_compat.rs` — booted static-musl compatibility ladder. Its
  committed ET_EXEC inputs live under `userland/prebuilt/compiler-compat/`
  and are staged even when `test.sh --skip-userland` is used.
- `tcc.rs` — booted TinyCC end-to-end: launches the staged `/host/TCC.ELF`
  against `/host/sysroot`, compiles (including a test-authored source and a
  `.o` round-trip) into `/work`, and executes the fresh binaries through the
  production loader. Uses the committed `TCC.ELF` + `tcc-sysroot.tar.gz`
  prebuilts, staged even with `--skip-userland`.
- `binutils.rs` — booted GNU binutils end-to-end: launches all fourteen
  committed static tools, assembles/links/runs a new ELF, creates and links an
  archive, checks stable inspection output through zsh redirection, preserves
  timestamps during objcopy, transforms/strips an ELF, and stress-links more
  inputs than the 32-slot fd table.
- `network.rs` — Virtqueue ownership/error edges plus bounded registry and
  QEMU-local DHCP coverage.
- `git_userland.rs` — booted git end-to-end: version + `/etc/gitconfig`
  identity smoke, local init→commit→branch→merge→fsck round trip, local
  clone, and a dumb-protocol HTTP clone through the `GITRHTTP.ELF`
  transport helper against the committed `tools/git-fixture` repo served
  by `tools/net-test-http.py`.
- `network_userland.rs` — booted static-musl socket fixture and BusyBox
  numeric IPv4 `ping`, `nc`, and HTTP-only `wget` smokes, including
  zsh→fork/execve regressions for `ping` and `wget`. `test.sh` supplies
  restricted QEMU networking and repository-owned guest-forwarded services.
- `procfs.rs` — synthetic `/proc`, `sysinfo(2)`, process accounting/signal
  coverage, plus booted BusyBox `free`, `top -b`, and `reset` capability
  smokes that keep the committed multicall binary aligned with the kernel ABI.
- `entropy.rs` — asserts the default QEMU selects modern VirtIO RNG, requests
  distinct broker output, and covers the RDRAND CPUID decoder.
- `p9.rs` — 9P2000.L codec round-trips plus booted `/shared` coverage against
  the per-run host temp share `test.sh` attaches (fixture read, create/write/
  read-back, truncate+append, 256 KiB multi-chunk, enumerate with real sizes,
  rename, symlink resolution, error paths). Self-skips only when no virtio-9p
  device is attached; with the device present a broken mount is a hard failure.

## Adding a test

1. **Write the test function** in the appropriate topic module:
   ```rust
   fn test_example() {
       assert_eq!(2 + 2, 4);
   }
   ```

2. **Register it** in that module's `get_tests()`. The slice must be `&'static [&'static dyn Testable]` — heap allocation is not used here (some tests run before heap init):
   ```rust
   pub fn get_tests() -> &'static [&'static dyn Testable] {
       &[
           &test_example,
           // existing tests…
       ]
   }
   ```

3. The test runs automatically on the next `./test.sh` invocation.

## Adding a topic module

1. Create `src/tests/<topic>.rs` with a `pub fn get_tests() -> &'static [&'static dyn Testable]`.
2. Add `#[cfg(feature = "test")] pub mod <topic>;` near the top of `mod.rs`.
3. Add **one line** to the `MODULES` registry: `("<short_name>", <topic>::get_tests),`. The short name becomes the `<module>` half of `<module>::<fn>` filter matching, so keep it lowercase and stable.

## Filtering

The runner consults `filter.rs`, populated at boot from QEMU `fw_cfg`. Run a subset:

```sh
./test.sh arc                   # one module
./test.sh 'arc::test_weak*'     # glob within a module
./test.sh '*scroll*'            # substring across module::fn
./test.sh -l                    # list registered modules
```

When the filter matches zero tests the kernel exits with code 35 (failure) so a typo never silently "passes." Full syntax: `.claude/rules/testing-flow.md`.

## Output

Each test prints its name to serial and `[ok]` on success. Failure triggers the panic handler, which prints the failure and exits QEMU with the failure code (35).

## Conventions

- **Static slices, not `Vec`.** `get_tests()` returns `&'static [...]` — this isn't decorative; some tests run before the heap is up, so the slice must be available without allocation.
- **Topic-organized.** Add new test functions to the existing topic module that fits, or add a new topic file (then wire its `get_tests()` into the test runner).
- **Don't write infinite-loop tests.** A hang prevents QEMU from exiting; the harness reads no exit code and reports failure ambiguously.
- **Booted compatibility inputs are mandatory.** `compiler_compat`, `binutils`, and
  `network_userland` must fail, not skip, when a committed fixture is missing.
  Refresh binaries through their source directories and commit source plus ELF
  together. Networking waits must be PIT-deadline-bounded and must use only
  the restricted QEMU-local services configured by `test.sh`.

## Cross-references

- `Testable` trait and `test_runner()` live in `src/lib/test_utils.rs` — see `src/lib/CLAUDE.md`.
- Test invocation, exit codes, and panic-handler test-mode behavior: `.claude/rules/testing-flow.md`.

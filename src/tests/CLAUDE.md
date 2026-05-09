# `src/tests/` — Kernel Test Authoring

This folder holds in-kernel test modules that run under QEMU when the kernel is built with `--features test`. For *running* tests and exit-code semantics, see `.claude/rules/testing-flow.md`.

## Key files (organized by topic)

- `basic.rs` — sanity tests.
- `memory.rs` — memory subsystem tests.
- `heap.rs` — heap allocator and dynamic allocation tests.
- `arc.rs` — `Arc` / `Weak` reference counting tests.
- `display.rs` — display and graphics tests.
- `interrupts.rs` — interrupt handler tests.
- `filesystem.rs` — filesystem tests.

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

## Output

Each test prints its name to serial and `[ok]` on success. Failure triggers the panic handler, which prints the failure and exits QEMU with the failure code (35).

## Conventions

- **Static slices, not `Vec`.** `get_tests()` returns `&'static [...]` — this isn't decorative; some tests run before the heap is up, so the slice must be available without allocation.
- **Topic-organized.** Add new test functions to the existing topic module that fits, or add a new topic file (then wire its `get_tests()` into the test runner).
- **Don't write infinite-loop tests.** A hang prevents QEMU from exiting; the harness reads no exit code and reports failure ambiguously.

## Cross-references

- `Testable` trait and `test_runner()` live in `src/lib/test_utils.rs` — see `src/lib/CLAUDE.md`.
- Test invocation, exit codes, and panic-handler test-mode behavior: `.claude/rules/testing-flow.md`.

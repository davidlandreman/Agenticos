# Testing Flow

The kernel has a custom `no_std` test framework that runs tests directly inside the booted kernel under QEMU.

## Running tests

```sh
./test.sh
```

This builds with `--features test`, boots the kernel under QEMU with `isa-debug-exit` wired up, runs the tests, and exits QEMU with the result code:

| Exit code | Meaning |
|---|---|
| 33 (`0x10 << 1 \| 1`) | All tests passed |
| 35 (`0x11 << 1 \| 1`) | A test failed (test panicked, panic handler exited with this code) |

Other (non-zero) exit codes mean the kernel crashed before tests could complete.

## Output

Tests print to the serial port (visible on stdout when QEMU is invoked via `./test.sh`):

```
test_name [ok]
```

Failing tests trigger the panic handler, which prints the panic info to serial and exits QEMU with code 35.

## Authoring tests

For how to add a test (`Testable` trait, `get_tests()` registration, where files live), see `src/tests/CLAUDE.md`. For the panic-handler test-mode behavior, see `.claude/rules/panic-and-attributes.md`.

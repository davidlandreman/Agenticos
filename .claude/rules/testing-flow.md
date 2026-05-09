# Testing Flow

The kernel has a custom `no_std` test framework that runs tests directly inside the booted kernel under QEMU.

## Running tests

```sh
./test.sh                       # all tests
./test.sh arc                   # one module
./test.sh arc heap              # several modules
./test.sh 'arc::test_weak*'     # glob within a module
./test.sh '*scroll*'            # substring across module::fn
./test.sh -l                    # list available modules and exit
./test.sh --skip-userland       # skip userland prebuild (faster iteration)
```

Builds with `--features test`, boots the kernel under QEMU with `isa-debug-exit` wired up, runs the tests, and exits QEMU with the result code:

| Exit code | Meaning |
|---|---|
| 33 (`0x10 << 1 \| 1`) | Selected tests passed |
| 35 (`0x11 << 1 \| 1`) | A test failed, OR no tests matched the filter |

Other (non-zero) exit codes mean the kernel crashed before tests could complete.

## Filter mechanism

The filter string is delivered via QEMU `fw_cfg` (file `opt/agenticos/test_filter`) and read by the kernel at boot (`src/tests/filter.rs`). No rebuild is needed when the filter changes — only the QEMU command line.

Syntax: comma-separated patterns. Each pattern matches against `<module>` or `<module>::<fn>`, with `*` allowed at the start and/or end of a pattern. A pattern with no `*` must match exactly. An empty/unset filter runs everything.

## Output

Tests print to the serial port (visible on stdout when QEMU is invoked via `./test.sh`):

```
test_name [ok]
```

Failing tests trigger the panic handler, which prints the panic info to serial and exits QEMU with code 35.

## Authoring tests

For how to add a test (`Testable` trait, `get_tests()` registration, where files live), see `src/tests/CLAUDE.md`. For the panic-handler test-mode behavior, see `.claude/rules/panic-and-attributes.md`.

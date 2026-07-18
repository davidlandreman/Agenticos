# Static-musl network fixture

`NETTEST.ELF` is the mandatory booted integration fixture for AgenticOS's
Linux IPv4 socket ABI. It checks invalid-family errno behavior, UDP
nonblocking readiness, ITIMER_REAL/SIGALRM interruption of a blocking receive,
TCP nonblocking connect, endpoint and socket-option queries, partial stream
I/O, finite timeouts, and clean shutdown against the QEMU-local framed echo
command.

Refresh the committed static `ET_EXEC` binary after changing its source:

```sh
make -C userland/apps/network-test refresh
```

The build requires `x86_64-linux-musl-gcc` (override with `MUSL_CC`). Ordinary
test runs only stage `userland/prebuilt/network/NETTEST.ELF`; they never need a
cross-compiler or public network access.

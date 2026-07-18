# Hermetic TLS fixtures

These test-only certificates back the QEMU `guestfwd` HTTPS endpoint. The
trusted root is staged only by `test.sh`; private keys remain on the host and
are never copied into the guest share. The test RTC is fixed at
2026-07-19T12:00:00Z so expired and not-yet-valid behavior is deterministic.

Run `./generate.sh` from any directory to regenerate the fixtures. The files
contain no production secrets.

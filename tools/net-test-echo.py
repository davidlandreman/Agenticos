#!/usr/bin/env python3
"""One-connection framed echo command used by QEMU guestfwd."""

import struct
import sys


def read_exact(stream, length):
    data = bytearray()
    while len(data) < length:
        chunk = stream.read(length - len(data))
        if not chunk:
            raise EOFError("short framed input")
        data.extend(chunk)
    return bytes(data)


def main():
    try:
        header = read_exact(sys.stdin.buffer, 4)
    except EOFError:
        # BusyBox `nc -z` deliberately connects and closes without payload.
        return
    (length,) = struct.unpack("!I", header)
    if length > 64 * 1024:
        raise ValueError("frame too large")
    payload = read_exact(sys.stdin.buffer, length)
    sys.stdout.buffer.write(header)
    for offset in range(0, len(payload), 313):
        sys.stdout.buffer.write(payload[offset : offset + 313])
        sys.stdout.buffer.flush()


if __name__ == "__main__":
    main()

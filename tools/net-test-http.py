#!/usr/bin/env python3
"""Minimal one-request HTTP/1.0 server used by QEMU guestfwd tests."""

import sys


def main():
    request = bytearray()
    while b"\r\n\r\n" not in request and len(request) < 8192:
        chunk = sys.stdin.buffer.read(1)
        if not chunk:
            return
        request.extend(chunk)

    body = b"AgenticOS HTTP OK\n"
    response = (
        b"HTTP/1.0 200 OK\r\n"
        b"Content-Type: text/plain\r\n"
        + f"Content-Length: {len(body)}\r\n".encode("ascii")
        + b"Connection: close\r\n\r\n"
        + body
    )
    sys.stdout.buffer.write(response)
    sys.stdout.buffer.flush()


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Deterministic one-request HTTP/1.0 server used by QEMU guestfwd tests."""

import os
import sys


# Committed dumb-protocol git repository served under /repo.git/ for the
# in-guest `git clone http://…/repo.git` test. git's smart-HTTP probe
# (info/refs?service=git-upload-pack) gets the plain info/refs file back
# because the query string is stripped, so the client falls back to the
# dumb protocol and walks the loose objects — no CGI required.
GIT_FIXTURE_ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "git-fixture")


def _git_fixture_response(path: bytes):
    relative = path[len(b"/repo.git/"):].decode("ascii", "replace")
    if not relative or ".." in relative.split("/") or relative.startswith("/"):
        return None
    file_path = os.path.join(GIT_FIXTURE_ROOT, "repo.git", *relative.split("/"))
    if not os.path.isfile(file_path):
        return None
    with open(file_path, "rb") as handle:
        body = handle.read()
    return (b"text/plain; charset=utf-8", body)


PAGES = {
    b"/": (
        b"text/html; charset=utf-8",
        b"<!doctype html><html><head><title>AgenticOS browser fixture</title></head>"
        b"<body><h1>AgenticOS HTTP OK</h1>"
        b"<p>Text-mode browser fixture: caf\xc3\xa9.</p>"
        b"<a href='/second'>Relative second page</a>"
        b"<form action='/form'><input name='query'><input type='submit'></form>"
        b"</body></html>\n",
    ),
    b"/second": (
        b"text/html; charset=utf-8",
        b"<!doctype html><html><head><title>Second page</title></head>"
        b"<body><h1>AgenticOS second page</h1><a href='/'>Home</a></body></html>\n",
    ),
}


def main():
    request = bytearray()
    while b"\r\n\r\n" not in request and len(request) < 8192:
        chunk = sys.stdin.buffer.read(1)
        if not chunk:
            return
        request.extend(chunk)

    first_line = bytes(request).split(b"\r\n", 1)[0]
    fields = first_line.split()
    path = fields[1] if len(fields) >= 2 else b"/"
    path = path.split(b"?", 1)[0]
    if path.startswith(b"/repo.git/"):
        fixture = _git_fixture_response(path)
        content_type, body = fixture or (
            b"text/plain; charset=utf-8",
            b"AgenticOS fixture: not found\n",
        )
        status = b"200 OK" if fixture else b"404 Not Found"
        response = (
            b"HTTP/1.0 "
            + status
            + b"\r\nContent-Type: "
            + content_type
            + b"\r\n"
            + f"Content-Length: {len(body)}\r\n".encode("ascii")
            + b"Connection: close\r\n\r\n"
            + body
        )
        sys.stdout.buffer.write(response)
        sys.stdout.buffer.flush()
        return
    if path == b"/redirect":
        sys.stdout.buffer.write(
            b"HTTP/1.0 302 Found\r\nLocation: /second\r\nContent-Length: 0\r\n"
            b"Connection: close\r\n\r\n"
        )
        sys.stdout.buffer.flush()
        return

    content_type, body = PAGES.get(
        path,
        (b"text/plain; charset=utf-8", b"AgenticOS fixture: not found\n"),
    )
    status = b"200 OK" if path in PAGES else b"404 Not Found"
    response = (
        b"HTTP/1.0 "
        + status
        + b"\r\nContent-Type: "
        + content_type
        + b"\r\n"
        + f"Content-Length: {len(body)}\r\n".encode("ascii")
        + b"Connection: close\r\n\r\n"
        + body
    )
    sys.stdout.buffer.write(response)
    sys.stdout.buffer.flush()


if __name__ == "__main__":
    main()

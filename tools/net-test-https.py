#!/usr/bin/env python3
"""Bounded TLS-over-stdio server for QEMU guestfwd HTTPS tests."""

import os
import ssl
import sys


BASE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "tls-fixtures")


def context(cert_name, tls12_only=False):
    result = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    result.minimum_version = ssl.TLSVersion.TLSv1_2
    # QEMU starts a fresh command for each guestfwd connection, so there is no
    # shared ticket key or session cache across a redirect. Do not advertise
    # resumable sessions that the next fixture process cannot honor.
    result.options |= ssl.OP_NO_TICKET
    if hasattr(result, "num_tickets"):
        result.num_tickets = 0
    if tls12_only:
        result.maximum_version = ssl.TLSVersion.TLSv1_2
    result.load_cert_chain(
        os.path.join(BASE, cert_name + ".pem"),
        os.path.join(BASE, cert_name + ".key"),
    )
    return result


def flush(outgoing):
    while outgoing.pending:
        sys.stdout.buffer.write(outgoing.read())
    sys.stdout.buffer.flush()


def feed(incoming, outgoing):
    flush(outgoing)
    # guestfwd connects separate stdin/stdout pipes. os.read returns the bytes
    # currently delivered by QEMU instead of waiting for a full buffered read.
    data = os.read(sys.stdin.fileno(), 4096)
    if not data:
        incoming.write_eof()
        return False
    incoming.write(data)
    return True


def retry_read(operation, incoming, outgoing):
    while True:
        try:
            return operation()
        except ssl.SSLWantReadError:
            if not feed(incoming, outgoing):
                raise EOFError
        except ssl.SSLWantWriteError:
            flush(outgoing)


def main():
    contexts = {
        "valid.agenticos.test": context("valid"),
        "tls12.agenticos.test": context("valid", tls12_only=True),
        "expired.agenticos.test": context("expired"),
        "future.agenticos.test": context("future"),
        "untrusted.agenticos.test": context("untrusted"),
    }
    default = contexts["valid.agenticos.test"]

    def select_server(ssl_object, server_name, _initial_context):
        # mismatch.agenticos.test intentionally keeps the valid certificate.
        ssl_object.context = contexts.get(server_name, default)

    default.set_servername_callback(select_server)
    incoming = ssl.MemoryBIO()
    outgoing = ssl.MemoryBIO()
    session = default.wrap_bio(incoming, outgoing, server_side=True)

    try:
        retry_read(session.do_handshake, incoming, outgoing)
        request = bytearray()
        while b"\r\n\r\n" not in request and len(request) < 8192:
            chunk = retry_read(lambda: session.read(4096), incoming, outgoing)
            if not chunk:
                break
            request.extend(chunk)

        first_line = bytes(request).split(b"\r\n", 1)[0]
        fields = first_line.split()
        path = fields[1] if len(fields) >= 2 else b"/"
        if path == b"/redirect":
            response = (
                b"HTTP/1.0 302 Found\r\nLocation: /second\r\n"
                b"Content-Length: 0\r\nConnection: close\r\n\r\n"
            )
        else:
            marker = b"AgenticOS HTTPS second page" if path == b"/second" else b"AgenticOS HTTPS OK"
            body = b"<!doctype html><html><body><h1>" + marker + b"</h1></body></html>\n"
            response = (
                b"HTTP/1.0 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n"
                + f"Content-Length: {len(body)}\r\n".encode("ascii")
                + b"Connection: close\r\n\r\n"
                + body
            )
        retry_read(lambda: session.write(response), incoming, outgoing)
        flush(outgoing)
    except (EOFError, ssl.SSLError):
        # Rejected client handshakes are expected in the negative tests.
        flush(outgoing)


if __name__ == "__main__":
    main()

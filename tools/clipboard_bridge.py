#!/usr/bin/env python3
"""Bridge AgenticOS's COM3 protocol to the host's text clipboard."""

from __future__ import annotations

import argparse
import os
import shutil
import socket
import struct
import subprocess
import sys
import time
from dataclasses import dataclass


REQUEST_MAGIC = b"ACCB"
RESPONSE_MAGIC = b"ACBR"
VERSION = 1
OP_COPY = 1
OP_PASTE = 2
STATUS_OK = 0
STATUS_BAD_REQUEST = 1
STATUS_UNSUPPORTED = 2
STATUS_HOST_ERROR = 3
STATUS_TOO_LARGE = 4
STATUS_INVALID_TEXT = 5
MAX_TEXT_BYTES = 1024 * 1024
HEADER = struct.Struct("<4sBBI")


class BridgeError(Exception):
    def __init__(self, status: int, message: str) -> None:
        super().__init__(message)
        self.status = status
        self.message = message


@dataclass(frozen=True)
class ClipboardCommands:
    copy: tuple[str, ...]
    paste: tuple[str, ...]


class CommandClipboard:
    def __init__(self, commands: ClipboardCommands | None = None) -> None:
        self.commands = commands or detect_clipboard_commands()

    def copy(self, text: bytes) -> None:
        run = subprocess.run(
            self.commands.copy,
            input=text,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            timeout=10,
            check=False,
        )
        if run.returncode != 0:
            raise BridgeError(STATUS_HOST_ERROR, command_error("copy", run.stderr))

    def paste(self) -> bytes:
        run = subprocess.run(
            self.commands.paste,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=10,
            check=False,
        )
        if run.returncode != 0:
            raise BridgeError(STATUS_HOST_ERROR, command_error("paste", run.stderr))
        return run.stdout


def command_error(operation: str, stderr: bytes) -> str:
    detail = stderr.decode("utf-8", errors="replace").strip()
    return f"host clipboard {operation} failed" + (f": {detail}" if detail else "")


def detect_clipboard_commands() -> ClipboardCommands:
    if sys.platform == "darwin" and shutil.which("pbcopy") and shutil.which("pbpaste"):
        return ClipboardCommands(("pbcopy",), ("pbpaste",))
    if shutil.which("wl-copy") and shutil.which("wl-paste"):
        return ClipboardCommands(("wl-copy",), ("wl-paste", "--no-newline"))
    if shutil.which("xclip"):
        return ClipboardCommands(
            ("xclip", "-selection", "clipboard", "-in"),
            ("xclip", "-selection", "clipboard", "-out"),
        )
    raise BridgeError(
        STATUS_UNSUPPORTED,
        "no supported host text clipboard commands found",
    )


def validate_text(payload: bytes) -> None:
    if len(payload) > MAX_TEXT_BYTES:
        raise BridgeError(STATUS_TOO_LARGE, "text exceeds the 1 MiB limit")
    try:
        payload.decode("utf-8")
    except UnicodeDecodeError as error:
        raise BridgeError(STATUS_INVALID_TEXT, f"clipboard data is not UTF-8: {error}") from error


def handle_request(operation: int, payload: bytes, clipboard: object) -> bytes:
    if operation == OP_COPY:
        validate_text(payload)
        clipboard.copy(payload)
        return b""
    if operation == OP_PASTE:
        if payload:
            raise BridgeError(STATUS_BAD_REQUEST, "paste request must have an empty payload")
        text = clipboard.paste()
        validate_text(text)
        return text
    raise BridgeError(STATUS_BAD_REQUEST, f"unknown clipboard operation {operation}")


def read_exact(connection: socket.socket, length: int) -> bytes:
    data = bytearray()
    while len(data) < length:
        chunk = connection.recv(length - len(data))
        if not chunk:
            raise EOFError
        data.extend(chunk)
    return bytes(data)


def send_response(connection: socket.socket, status: int, payload: bytes = b"") -> None:
    connection.sendall(HEADER.pack(RESPONSE_MAGIC, VERSION, status, len(payload)))
    if payload:
        connection.sendall(payload)


def serve_connection(connection: socket.socket, clipboard: object) -> None:
    while True:
        raw_header = read_exact(connection, HEADER.size)
        magic, version, operation, payload_len = HEADER.unpack(raw_header)
        if payload_len > MAX_TEXT_BYTES:
            send_response(connection, STATUS_TOO_LARGE, b"request exceeds the 1 MiB limit")
            raise EOFError
        payload = read_exact(connection, payload_len)
        try:
            if magic != REQUEST_MAGIC or version != VERSION:
                raise BridgeError(STATUS_BAD_REQUEST, "invalid clipboard protocol header")
            response = handle_request(operation, payload, clipboard)
            send_response(connection, STATUS_OK, response)
        except BridgeError as error:
            message = error.message.encode("utf-8")[:MAX_TEXT_BYTES]
            send_response(connection, error.status, message)
        except (OSError, subprocess.SubprocessError) as error:
            send_response(connection, STATUS_HOST_ERROR, str(error).encode("utf-8"))


def connect_and_serve(socket_path: str, clipboard: object) -> None:
    while True:
        connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            connection.connect(socket_path)
            serve_connection(connection, clipboard)
        except (FileNotFoundError, ConnectionRefusedError):
            time.sleep(0.1)
            connection.close()
            continue
        except (EOFError, BrokenPipeError, ConnectionResetError):
            return
        finally:
            connection.close()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--socket",
        default=os.environ.get("AGENTICOS_CLIPBOARD_SOCK", "/tmp/agenticos-clipboard.sock"),
    )
    parser.add_argument("--check", action="store_true", help="validate host support and exit")
    args = parser.parse_args()
    try:
        clipboard = CommandClipboard()
    except BridgeError as error:
        print(f"clipboard bridge: {error.message}", file=sys.stderr)
        return 1
    if args.check:
        return 0
    connect_and_serve(args.socket, clipboard)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

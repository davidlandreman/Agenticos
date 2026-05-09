"""Unix-socket client for the AgenticOS kernel RPC chardev.

Wire format (mirrors src/tools/rpc/framing.rs):
    [u32 LE header_len][JSON header bytes][u32 LE binary_len][binary bytes]

JSON header request:  {"id": <int>, "name": "<tool>", "args": {...}}
JSON header response: {"id": <int>, "ok": <result>}
                      | {"id": <int>, "error": {"code": "...", "message": "..."}}
"""

from __future__ import annotations

import json
import os
import socket
import struct
import threading
import time
from dataclasses import dataclass
from typing import Any


DEFAULT_SOCKET = os.environ.get("AGENTICOS_RPC_SOCK", "/tmp/agenticos-rpc.sock")
CONNECT_RETRY_SECONDS = 0.25
CONNECT_TIMEOUT_SECONDS = 5.0
# Generous: a `screenshot` of a 1280×720×4bpp framebuffer pushes ~3.7 MiB
# byte-by-byte through QEMU's emulated UART (one vmexit per byte). Real-world
# completion is in the 30-90s range. v1 sets a wide ceiling; long-term fix is
# virtio-serial (deferred per plan).
CALL_TIMEOUT_SECONDS = 180.0


class TransportError(Exception):
    """Transport-level failure (cannot reach kernel, frame parse error, timeout)."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(f"{code}: {message}")
        self.code = code
        self.message = message


class ToolCallError(Exception):
    """Tool-level failure returned by the kernel registry as a structured error."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(f"{code}: {message}")
        self.code = code
        self.message = message


@dataclass
class FrameResponse:
    header: dict
    binary: bytes | None


class KernelClient:
    """Thread-safe client. Each `request` call is serialized through `_lock`
    because the kernel only handles one in-flight request at a time."""

    def __init__(self, socket_path: str = DEFAULT_SOCKET) -> None:
        self._socket_path = socket_path
        self._sock: socket.socket | None = None
        self._lock = threading.Lock()
        self._next_id = 1

    def connect(self, *, timeout: float = CONNECT_TIMEOUT_SECONDS) -> None:
        deadline = time.monotonic() + timeout
        last_err: Exception | None = None
        while time.monotonic() < deadline:
            try:
                s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                s.settimeout(CALL_TIMEOUT_SECONDS)
                s.connect(self._socket_path)
                self._sock = s
                return
            except (FileNotFoundError, ConnectionRefusedError) as e:
                last_err = e
                time.sleep(CONNECT_RETRY_SECONDS)
        raise TransportError(
            "kernel_unreachable",
            f"could not connect to {self._socket_path} within {timeout:.1f}s: {last_err}",
        )

    def request(self, name: str, args: dict | None = None) -> FrameResponse:
        if self._sock is None:
            raise TransportError("not_connected", "call connect() first")
        with self._lock:
            req_id = self._next_id
            self._next_id += 1
            header = {"id": req_id, "name": name, "args": args or {}}
            self._send_frame(json.dumps(header).encode("utf-8"), None)
            return self._recv_frame()

    def close(self) -> None:
        if self._sock is not None:
            try:
                self._sock.close()
            finally:
                self._sock = None

    # --- framing helpers -----------------------------------------------------

    def _send_frame(self, header: bytes, binary: bytes | None) -> None:
        assert self._sock is not None
        try:
            self._sock.sendall(struct.pack("<I", len(header)))
            self._sock.sendall(header)
            self._sock.sendall(struct.pack("<I", len(binary) if binary else 0))
            if binary:
                self._sock.sendall(binary)
        except (BrokenPipeError, ConnectionResetError, OSError) as e:
            raise TransportError("kernel_unreachable", str(e)) from e

    def _recv_exact(self, n: int) -> bytes:
        assert self._sock is not None
        buf = bytearray()
        while len(buf) < n:
            try:
                chunk = self._sock.recv(n - len(buf))
            except socket.timeout as e:
                raise TransportError("kernel_timeout", str(e)) from e
            except (ConnectionResetError, OSError) as e:
                raise TransportError("kernel_unreachable", str(e)) from e
            if not chunk:
                raise TransportError("kernel_unreachable", "kernel closed the connection")
            buf.extend(chunk)
        return bytes(buf)

    def _recv_frame(self) -> FrameResponse:
        header_len = struct.unpack("<I", self._recv_exact(4))[0]
        header_bytes = self._recv_exact(header_len)
        binary_len = struct.unpack("<I", self._recv_exact(4))[0]
        binary = self._recv_exact(binary_len) if binary_len else None
        try:
            header = json.loads(header_bytes.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError) as e:
            raise TransportError("frame_parse_error", str(e)) from e
        return FrameResponse(header=header, binary=binary)


def call_tool(client: KernelClient, name: str, args: dict | None = None) -> tuple[Any, bytes | None]:
    """Convenience wrapper: raises ToolCallError on `error` responses, returns
    (ok_value, binary) on success. Use directly when you don't care about the
    request id."""
    response = client.request(name, args)
    header = response.header
    if "error" in header:
        err = header["error"]
        raise ToolCallError(err.get("code", "tool_failed"), err.get("message", ""))
    return header.get("ok"), response.binary


class LazyKernelClient:
    """KernelClient that connects on demand and reconnects after transport
    errors. Lets the MCP bridge register tools at startup whether or not
    QEMU is running yet; subsequent calls reach the kernel as soon as it
    becomes available without restarting the bridge.

    Per-call connect timeout is short (1s) so a still-down kernel surfaces
    quickly to the MCP client rather than blocking the user."""

    def __init__(self, socket_path: str = DEFAULT_SOCKET, *, per_call_timeout: float = 1.0) -> None:
        self._socket_path = socket_path
        self._per_call_timeout = per_call_timeout
        self._client: KernelClient | None = None

    def call(self, name: str, args: dict | None = None) -> tuple[Any, bytes | None]:
        # No automatic retry on TransportError. A mid-call timeout leaves the
        # kernel still writing the (large) response into QEMU's chardev buffer;
        # reconnecting would consume those stale bytes as the next response and
        # desync the channel. Surface the failure once and let the caller (or
        # the user) decide whether to retry — and possibly restart QEMU first.
        try:
            return self._call_once(name, args)
        except TransportError:
            self._reset()
            raise

    def _call_once(self, name: str, args: dict | None) -> tuple[Any, bytes | None]:
        if self._client is None:
            client = KernelClient(self._socket_path)
            client.connect(timeout=self._per_call_timeout)
            self._client = client
        return call_tool(self._client, name, args)

    def _reset(self) -> None:
        if self._client is not None:
            try:
                self._client.close()
            except Exception:
                pass
            self._client = None

    def list_tools_or_none(self) -> list[dict] | None:
        """Best-effort startup discovery. Returns the kernel's tool list when
        reachable, otherwise None — caller falls back to the hardcoded v1
        list."""
        try:
            ok, _ = self.call("__list_tools__")
        except (TransportError, ToolCallError):
            return None
        return ok if isinstance(ok, list) else None

"""AgenticOS MCP bridge.

Launched as a subprocess by the MCP client (Claude). Connects to the running
kernel's RPC chardev (a unix socket QEMU exposes), and registers tools that
mirror the kernel's tool registry plus a bridge-native `read_serial`.

Tool surface is **stable across restarts** — the v1 tool list is registered
unconditionally, and each call connects to the kernel on demand. This means
booting QEMU after Claude does not require a Claude restart; the next tool
call simply succeeds. When the kernel is reachable at startup, descriptions
and schemas are pulled from the kernel; otherwise a fallback descriptor set
is used for the same tool names.

Run:
    uv run --project . bridge.py
or:
    python bridge.py

Env vars:
    AGENTICOS_RPC_SOCK   path to the kernel RPC unix socket (default /tmp/agenticos-rpc.sock)
    AGENTICOS_LOG_FILE   optional path to a file the bridge tails to back `read_serial`
"""

from __future__ import annotations

import io
import json
import sys
from typing import Any

try:
    from fastmcp import FastMCP
except ImportError:
    print(
        "fastmcp not installed. Run `uv sync` (or `pip install -r requirements.txt`)",
        file=sys.stderr,
    )
    raise

try:
    from PIL import Image
except ImportError:
    Image = None  # screenshot tool will surface a clear error

from kernel_client import LazyKernelClient, ToolCallError, TransportError
from serial_tail import SerialTail


# Hardcoded fallback descriptors used when the kernel isn't reachable at
# startup. The names must match the kernel-side `Tool::name()` registrations
# so that handlers dispatch correctly once the kernel comes up. Adding a
# sixth kernel tool means updating this list — small partial regression on
# R13's "advertise dynamically", justified by stable tool visibility.
# `screenshot` is intentionally excluded from v1: byte-by-byte UART transmission
# of a multi-MB framebuffer is prohibitively slow. Re-enable when the transport
# swaps to virtio-serial or IRQ-driven UART (deferred per plan).
V1_FALLBACK_TOOLS: list[dict] = [
    {
        "name": "kernel_state",
        "description": "structured snapshot of in-kernel state (windows, processes, heap)",
    },
    {
        "name": "send_input",
        "description": "synthesize keyboard and/or mouse events into the window event pipeline",
    },
    {
        "name": "shell_run",
        "description": "run a shell command (allowlisted text-only commands) and return stdout",
    },
]


def _format_transport_failure(prefix: str, e: TransportError) -> str:
    return (
        f"{prefix} failed: {e.code}: {e.message}\n"
        "If you just booted QEMU, retry. If QEMU isn't running, run `./build.sh` "
        "and try again — the bridge connects on demand."
    )


def _build_screenshot_handler(client: LazyKernelClient):
    def handler(args: dict | None = None) -> list[dict]:
        try:
            ok, binary = client.call("screenshot", args or {})
        except TransportError as e:
            return [{"type": "text", "text": _format_transport_failure("screenshot", e)}]
        except ToolCallError as e:
            return [{"type": "text", "text": f"screenshot failed: {e.code}: {e.message}"}]
        if Image is None:
            return [{"type": "text", "text": "Pillow not installed; cannot encode PNG"}]
        if binary is None:
            return [{"type": "text", "text": "screenshot returned no pixel data"}]
        meta = ok or {}
        width = int(meta.get("width", 0))
        height = int(meta.get("height", 0))
        stride = int(meta.get("stride", 0))
        bpp = int(meta.get("bytes_per_pixel", 4))
        pixel_format = str(meta.get("pixel_format", "bgr"))
        row_bytes = stride * bpp
        if row_bytes <= 0 or len(binary) < row_bytes * height:
            return [{"type": "text", "text": f"snapshot too small: {len(binary)} bytes for {width}x{height} stride={stride} bpp={bpp}"}]
        useful_row = width * bpp
        rows = bytearray()
        for y in range(height):
            start = y * row_bytes
            rows.extend(binary[start : start + useful_row])
        if bpp == 4:
            mode = "RGBA" if pixel_format == "rgb" else "BGRA"
        elif bpp == 3:
            mode = "RGB" if pixel_format == "rgb" else "BGR"
        else:
            return [{"type": "text", "text": f"unsupported bytes_per_pixel: {bpp}"}]
        img = Image.frombytes(mode, (width, height), bytes(rows))
        if mode == "BGRA":
            b, g, r, a = img.split()
            img = Image.merge("RGBA", (r, g, b, a))
        elif mode == "BGR":
            b, g, r = img.split()
            img = Image.merge("RGB", (r, g, b))
        buf = io.BytesIO()
        img.save(buf, "PNG")
        import base64
        return [
            {
                "type": "image",
                "data": base64.b64encode(buf.getvalue()).decode("ascii"),
                "mimeType": "image/png",
            }
        ]

    return handler


def _build_passthrough_handler(client: LazyKernelClient, name: str):
    def handler(args: dict | None = None) -> list[dict]:
        try:
            ok, _ = client.call(name, args or {})
        except TransportError as e:
            return [{"type": "text", "text": _format_transport_failure(name, e)}]
        except ToolCallError as e:
            return [{"type": "text", "text": f"{name} failed: {e.code}: {e.message}"}]
        return [{"type": "text", "text": json.dumps(ok, indent=2)}]

    return handler


def _build_read_serial_handler(tail: SerialTail):
    def handler(args: dict | None = None) -> list[dict]:
        args = args or {}
        since = args.get("since_seq")
        if since is not None:
            try:
                since = int(since)
            except (TypeError, ValueError):
                return [{"type": "text", "text": "since_seq must be an integer"}]
        max_bytes = int(args.get("max_bytes", 64 * 1024))
        result = tail.drain(since_seq=since, max_bytes=max_bytes)
        return [{"type": "text", "text": json.dumps(result, indent=2)}]

    return handler


def main() -> None:
    mcp = FastMCP("agenticos-bridge")
    tail = SerialTail()
    client = LazyKernelClient()

    # Best-effort dynamic discovery: prefer the kernel's live descriptor list
    # (R13) when reachable, fall back to the hardcoded v1 list otherwise.
    # Either way the SAME tool names are registered, so MCP clients see a
    # stable surface regardless of QEMU's lifecycle vs. Claude's.
    discovered = client.list_tools_or_none()
    if discovered is not None:
        kernel_tools = discovered
        print(
            f"agenticos-bridge: discovered {len(kernel_tools)} tools from kernel",
            file=sys.stderr,
        )
    else:
        kernel_tools = V1_FALLBACK_TOOLS
        print(
            "agenticos-bridge: kernel not reachable at startup; "
            f"registering {len(kernel_tools)} v1 fallback tools. "
            "Calls will connect on demand once QEMU is up.",
            file=sys.stderr,
        )

    for meta in kernel_tools:
        name = meta["name"]
        description = meta.get("description", "")
        if name == "screenshot":
            handler = _build_screenshot_handler(client)
        else:
            handler = _build_passthrough_handler(client, name)
        mcp.tool(name=name, description=description)(handler)

    # Bridge-native: drains the bridge-side serial ring (no kernel call).
    mcp.tool(
        name="read_serial",
        description="Drain bridge-buffered kernel log output (from -serial stdio)",
    )(_build_read_serial_handler(tail))

    mcp.run()


if __name__ == "__main__":
    main()

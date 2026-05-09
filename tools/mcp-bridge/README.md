# AgenticOS MCP Bridge

Host-side MCP server that exposes the kernel-resident tool registry to MCP
clients (e.g. Claude). Launched as a stdio subprocess by the MCP client;
talks to the kernel over a unix socket QEMU exposes.

## Running

1. Start the kernel under QEMU. The default `./build.sh` opens the RPC
   chardev at `/tmp/agenticos-rpc.sock` (overridable via
   `AGENTICOS_RPC_SOCK`) and chmods it to `0600`.

2. Optional — to make `read_serial` work end-to-end, capture QEMU's
   `-serial stdio` to a file the bridge can tail, and point the bridge at it:

   ```sh
   AGENTICOS_LOG_FILE=/tmp/agenticos.log ./build.sh 2>&1 | tee /tmp/agenticos.log
   ```

   `read_serial` is a bridge-native tool — the kernel does not buffer log
   output. If `AGENTICOS_LOG_FILE` is unset or the file does not exist when
   the bridge starts, `read_serial` returns empty data with a `dropped: 0`
   counter.

3. Claude Code in this repo discovers the bridge automatically via the
   project-local `.mcp.json` at the repo root. That config invokes
   `tools/mcp-bridge/run.sh`, which `cd`s to the bridge dir and runs `uv run
   bridge.py` (falling back to `python3 bridge.py` if `uv` is not on PATH).

   For other MCP clients, point them at `tools/mcp-bridge/run.sh` directly
   (no args), or invoke the bridge manually:

   ```sh
   uv --directory tools/mcp-bridge run bridge.py
   # or
   python3 tools/mcp-bridge/bridge.py
   ```

   You can override the socket path or log file via env vars:

   ```sh
   AGENTICOS_RPC_SOCK=/tmp/agenticos-rpc.sock \
   AGENTICOS_LOG_FILE=/tmp/agenticos.log \
       tools/mcp-bridge/run.sh
   ```

## What's exposed

The bridge advertises whatever the kernel registry returns from
`__list_tools__`, plus `read_serial` (bridge-native).

Kernel-resident tools (v1):

| Tool | Description |
|---|---|
| `screenshot` | Raw framebuffer snapshot encoded by the bridge as PNG |
| `shell_run` | Run an allowlisted argv-only shell command |
| `send_input` | Synthesize keyboard/mouse events (max 256 per call) |
| `kernel_state` | Snapshot of `windows`, `processes`, or `heap` |

Bridge-native tools:

| Tool | Description |
|---|---|
| `read_serial` | Drain bridge-buffered kernel log output |

## Architecture

- `kernel_client.py` — unix-socket client implementing length-prefixed
  framing (`[u32 LE header_len][JSON][u32 LE binary_len][binary]`).
- `serial_tail.py` — bridge-side ring buffer fed by tailing a log file the
  user redirected `-serial stdio` into.
- `bridge.py` — FastMCP stdio server. Discovers kernel tools at startup,
  registers each as an MCP tool, plus the bridge-native `read_serial`.

## Security

- The kernel exposes everything it can do over the RPC socket. Anyone who
  can connect to that socket can run shell commands, synthesize keystrokes,
  and read the framebuffer. `build.sh` `chmod 0600`s the socket so only the
  launching user can connect — do not relax this.
- `shell_run`'s allowlist (`ls`, `cat`, `pwd`, `echo`, `dir`, `touch`,
  `hexdump`) is a stability control — it prevents GUI commands and
  stdin-readers from blocking the dispatcher. It is **not** a security
  boundary. `cat /host/<anything>` is the documented happy path; anything
  reachable through `cat`/`hexdump` over the `/host` mount is exposed.
- The MCP transport is stdio — no TCP listener, no socket binding. The MCP
  client (Claude) is the sole peer.
- `send_input` synthesizes keystrokes indistinguishable from hardware
  input. Be aware when this bridge is connected.

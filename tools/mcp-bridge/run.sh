#!/bin/sh
# Wrapper that launches the AgenticOS MCP bridge from a stable path.
#
# `.mcp.json` at the repo root invokes this script. We `cd` to the bridge
# directory so relative imports (kernel_client, serial_tail) and the
# pyproject.toml resolve cleanly. uv is preferred for hermetic deps; fall
# back to system python3 if uv is unavailable.

set -e
cd "$(dirname "$0")"

if command -v uv >/dev/null 2>&1; then
    exec uv run --quiet bridge.py
fi

# Fallback: assume `fastmcp` and `pillow` are importable from the active
# python3 env.
exec python3 bridge.py

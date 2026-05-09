"""Bridge-side log buffer that backs `read_serial`.

The kernel writes its log output to `-serial stdio` via the existing
`qemu_print` path. The bridge does not modify the kernel; instead, when the
user wants `read_serial` to work end-to-end, they run QEMU with stdio teed
to a file (see README) and point the bridge at that file via
`AGENTICOS_LOG_FILE`.

This module exposes a small ring buffer with a sequence cursor so callers can
drain incrementally without losing or duplicating bytes.
"""

from __future__ import annotations

import os
import threading
from collections import deque


DEFAULT_LOG_FILE = os.environ.get("AGENTICOS_LOG_FILE")
RING_BYTES = 256 * 1024


class SerialTail:
    def __init__(self, path: str | None = DEFAULT_LOG_FILE, *, capacity: int = RING_BYTES) -> None:
        self._path = path
        self._capacity = capacity
        self._buf: bytearray = bytearray()
        self._head_seq = 0  # absolute seq of self._buf[0]
        self._next_seq = 0  # absolute seq of next byte that will be appended
        self._lock = threading.Lock()
        self._fp = None
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None
        if path:
            try:
                # Open for read; tail in a background thread so MCP calls
                # don't block on file I/O.
                self._fp = open(path, "rb")
                self._fp.seek(0, os.SEEK_END)
                self._thread = threading.Thread(target=self._reader_loop, name="serial-tail", daemon=True)
                self._thread.start()
            except FileNotFoundError:
                # Bridge starts before the user does ./build.sh. Fine — the
                # tail will simply be empty. Document the prerequisite in
                # the README.
                self._fp = None

    def stop(self) -> None:
        self._stop.set()

    def drain(self, since_seq: int | None = None, max_bytes: int = 64 * 1024) -> dict:
        """Return data from `since_seq` onward, capped at `max_bytes`. If
        `since_seq` is older than the oldest retained byte (ring overran),
        the response includes a `dropped` count and resumes from `head_seq`."""
        with self._lock:
            head = self._head_seq
            tail = self._next_seq
            if since_seq is None or since_seq < head:
                start = head
                dropped = 0 if since_seq is None else max(0, head - since_seq)
            else:
                start = min(since_seq, tail)
                dropped = 0
            available = tail - start
            take = min(available, max_bytes)
            offset = start - head
            data = bytes(self._buf[offset : offset + take])
            return {
                "data": data.decode("utf-8", errors="replace"),
                "next_seq": start + take,
                "dropped": dropped,
                "head_seq": head,
            }

    # --- internals -----------------------------------------------------------

    def _reader_loop(self) -> None:
        assert self._fp is not None
        while not self._stop.is_set():
            chunk = self._fp.read(4096)
            if not chunk:
                # No new data; brief sleep without holding the lock.
                import time
                time.sleep(0.1)
                continue
            with self._lock:
                self._buf.extend(chunk)
                self._next_seq += len(chunk)
                # Trim to capacity.
                overflow = len(self._buf) - self._capacity
                if overflow > 0:
                    del self._buf[:overflow]
                    self._head_seq += overflow

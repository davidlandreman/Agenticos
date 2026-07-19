#!/usr/bin/env python3
"""Run a command in its own process group and bound its wall-clock time."""

from __future__ import annotations

import argparse
import os
import signal
import subprocess
import sys


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--seconds", type=float, required=True)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command
    if command[:1] == ["--"]:
        command = command[1:]
    if args.seconds <= 0 or not command:
        parser.error("a positive timeout and command are required")

    process = subprocess.Popen(command, start_new_session=True)
    try:
        return process.wait(timeout=args.seconds)
    except subprocess.TimeoutExpired:
        print(
            f"command timed out after {args.seconds:g}s: {' '.join(command)}",
            file=sys.stderr,
        )
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            return process.wait()
        try:
            process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait()
        return 124


if __name__ == "__main__":
    raise SystemExit(main())

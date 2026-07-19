#!/usr/bin/env python3
"""Create the host half of an AgenticOS crash run identity."""

import argparse
import hashlib
import json
import os
import pathlib
import subprocess


def fnv1a64(value: str) -> int:
    result = 0xCBF29CE484222325
    for byte in value.encode():
        result ^= byte
        result = (result * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return result


def rotate_left(value: int, amount: int) -> int:
    return ((value << amount) | (value >> (64 - amount))) & 0xFFFFFFFFFFFFFFFF


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True, type=pathlib.Path)
    parser.add_argument("--kernel", required=True, type=pathlib.Path)
    parser.add_argument("--bios", required=True, type=pathlib.Path)
    parser.add_argument("--qemu", required=True, type=pathlib.Path)
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--mode", required=True)
    parser.add_argument("--memory", required=True)
    parser.add_argument("--smp", required=True, type=int)
    parser.add_argument("qemu_args", nargs="*")
    args = parser.parse_args()

    git_sha = os.environ.get("AGENTICOS_GIT_SHA", "unknown")
    dirty = os.environ.get("AGENTICOS_GIT_DIRTY", "unknown")
    rustc = os.environ.get("AGENTICOS_RUSTC_VERSION", "unknown")
    first = fnv1a64(git_sha) ^ rotate_left(fnv1a64(dirty), 17)
    second = fnv1a64(rustc)
    third = fnv1a64(args.mode) & 0xFFFFFFFF
    build_id = first.to_bytes(8, "little") + second.to_bytes(8, "little") + third.to_bytes(4, "little")
    qemu_version = subprocess.run(
        [args.qemu, "--version"], text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, check=False
    ).stdout.splitlines()[0]
    normalized = "\0".join(args.qemu_args).encode()
    document = {
        "schema": 1,
        "run_id": args.run_id.replace("-", "").lower(),
        "build_id": build_id.hex(),
        "git_sha": git_sha,
        "git_dirty": dirty,
        "rustc": rustc,
        "diagnostics": args.mode,
        "kernel_elf": str(args.kernel.resolve()),
        "kernel_elf_sha256": sha256(args.kernel),
        "bios_image_sha256": sha256(args.bios),
        "qemu": str(args.qemu.resolve()),
        "qemu_sha256": sha256(args.qemu),
        "qemu_version": qemu_version,
        "qemu_args_sha256": hashlib.sha256(normalized).hexdigest(),
        "memory": args.memory,
        "smp": args.smp,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(document, indent=2, sort_keys=True) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

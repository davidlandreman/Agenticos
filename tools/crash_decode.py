#!/usr/bin/env python3
"""Decode AgenticOS crash capsules without trusting guest-provided lengths."""

from __future__ import annotations

import argparse
import binascii
import hashlib
import json
import pathlib
import struct
import subprocess
from dataclasses import dataclass
from typing import Any

MAGIC = b"AGCRASH\0"
HEADER = struct.Struct("<8sHHIQ16s20sBBBBQII")
SECTION = struct.Struct("<HHIII")
SECTION_NAMES = {
    1: "run_metadata",
    2: "trigger",
    3: "cpu_snapshots",
    5: "trace_tail",
    6: "shadow_scheduler",
    10: "violation",
    11: "backtrace",
    12: "footer",
}
MAX_CAPSULE = 16 * 1024 * 1024
MAX_SECTIONS = 256


class DecodeError(ValueError):
    pass


@dataclass(frozen=True)
class Section:
    kind: int
    version: int
    flags: int
    payload: bytes


def _crc(data: bytes) -> int:
    return binascii.crc32(data) & 0xFFFFFFFF


def _u16_string(payload: bytes, offset: int) -> tuple[str, int]:
    if offset + 2 > len(payload):
        raise DecodeError("truncated metadata string length")
    length = struct.unpack_from("<H", payload, offset)[0]
    offset += 2
    end = offset + length
    if end > len(payload):
        raise DecodeError("truncated metadata string")
    return payload[offset:end].decode("utf-8", "replace"), end


def parse_capsule(blob: bytes, offset: int = 0) -> tuple[dict[str, Any], int]:
    if offset < 0 or offset + HEADER.size > len(blob):
        raise DecodeError("truncated capsule header")
    fields = HEADER.unpack_from(blob, offset)
    (
        magic,
        schema,
        header_len,
        total_len,
        flags,
        run_id,
        build_id,
        owner_cpu,
        online_mask,
        captured_mask,
        record_kind,
        sequence,
        payload_crc,
        header_crc,
    ) = fields
    if magic != MAGIC:
        raise DecodeError("bad capsule magic")
    if schema != 1:
        raise DecodeError(f"unsupported schema {schema}")
    if header_len != HEADER.size:
        raise DecodeError(f"unsupported header length {header_len}")
    if total_len < header_len or total_len > MAX_CAPSULE:
        raise DecodeError("invalid capsule total length")
    end = offset + total_len
    if end > len(blob):
        raise DecodeError("truncated capsule payload")
    header_bytes = bytearray(blob[offset : offset + header_len])
    header_bytes[76:80] = b"\0" * 4
    if _crc(header_bytes) != header_crc:
        raise DecodeError("header CRC mismatch")
    payload = blob[offset + header_len : end]
    if _crc(payload) != payload_crc:
        raise DecodeError("payload CRC mismatch")

    sections: list[Section] = []
    cursor = offset + header_len
    while cursor < end:
        if len(sections) >= MAX_SECTIONS:
            raise DecodeError("too many sections")
        if cursor + SECTION.size > end:
            raise DecodeError("truncated section header")
        kind, version, length, section_flags, checksum = SECTION.unpack_from(blob, cursor)
        cursor += SECTION.size
        section_end = cursor + length
        if section_end < cursor or section_end > end:
            raise DecodeError("invalid section length")
        section_payload = blob[cursor:section_end]
        if _crc(section_payload) != checksum:
            raise DecodeError(f"section {kind} CRC mismatch")
        sections.append(Section(kind, version, section_flags, section_payload))
        cursor = section_end

    duplicate_kinds = sorted(
        kind for kind in {section.kind for section in sections} if sum(s.kind == kind for s in sections) > 1
    )
    report: dict[str, Any] = {
        "schema": schema,
        "run": {
            "id": run_id.hex(),
            "build_id": build_id.hex(),
            "manifest_trusted": False,
        },
        "trigger": {
            "kind": {1: "fatal", 2: "invariant", 3: "user_incident"}.get(
                record_kind, f"unknown({record_kind})"
            ),
            "owner_cpu": owner_cpu,
        },
        "record_sequence": sequence,
        "flags": flags,
        "cpu_masks": {"online": online_mask, "captured": captured_mask},
        "sections": [],
        "missing": [],
        "inferences": [],
    }
    if duplicate_kinds:
        report["inferences"].append({"duplicate_sections": duplicate_kinds})
    for section in sections:
        name = SECTION_NAMES.get(section.kind, f"unknown_{section.kind}")
        report["sections"].append(
            {"kind": name, "id": section.kind, "version": section.version, "flags": section.flags}
        )
        _decode_section(report, section)
    required = {1, 2, 3, 5, 11, 12}
    if report["run"].get("personality") in ("record", "strict"):
        required.add(6)
    present = {section.kind for section in sections}
    report["missing"] = [SECTION_NAMES[kind] for kind in sorted(required - present)]
    trigger = report["trigger"]
    trigger["signature"] = _signature(trigger, report.get("violation"))
    return report, end


def _decode_section(report: dict[str, Any], section: Section) -> None:
    payload = section.payload
    if section.kind == 1:
        if len(payload) < 8:
            raise DecodeError("short run metadata")
        personality, _, _, features = struct.unpack_from("<BBHI", payload)
        cursor = 8
        values = []
        for _ in range(4):
            value, cursor = _u16_string(payload, cursor)
            values.append(value)
        if cursor != len(payload):
            raise DecodeError("trailing run metadata")
        report["run"].update(
            {
                "personality": {0: "minimal", 1: "record", 2: "strict"}.get(
                    personality, f"unknown({personality})"
                ),
                "feature_mask": features,
                "git_sha": values[0],
                "git_dirty": values[1],
                "rustc": values[2],
                "configured_personality": values[3],
            }
        )
    elif section.kind == 2:
        if len(payload) != 52:
            raise DecodeError("invalid trigger size")
        kind, vector, fidelity, _, reason, error, address, rip, file_hash, line, column = struct.unpack(
            "<BBBBQQQQQII", payload
        )
        if kind not in (1, 2, 3):
            raise DecodeError(f"invalid trigger kind {kind}")
        report["trigger"].update(
            {
                "vector": vector,
                "fidelity": fidelity,
                "reason_hash": f"0x{reason:016x}",
                "error_code": f"0x{error:x}",
                "fault_address": f"0x{address:x}",
                "rip": f"0x{rip:x}",
                "panic_location": {"file_hash": f"0x{file_hash:016x}", "line": line, "column": column},
            }
        )
    elif section.kind == 3:
        if len(payload) < 4:
            raise DecodeError("short CPU snapshot section")
        count, second, size = struct.unpack_from("<BBH", payload)
        keys = ("rip", "rsp", "rbp", "rflags", "cr0", "cr2", "cr3", "cr4", "fs_base", "gs_base", "current_pid")
        if size != 96:
            raise DecodeError("invalid CPU snapshot record size")
        if section.version == 1:
            if count != 1 or len(payload) != 4 + size:
                raise DecodeError("invalid v1 CPU snapshot layout")
            values = struct.unpack_from("<11QB7x", payload, 4)
            report["cpus"] = [{"cpu": second, **{key: f"0x{value:x}" for key, value in zip(keys, values[:11])}, "fidelity": values[11]}]
        elif section.version == 2:
            if len(payload) != 4 + count * size:
                raise DecodeError("invalid v2 CPU snapshot layout")
            cpus = []
            cursor = 4
            seen = set()
            for _ in range(count):
                cpu, fidelity, *values = struct.unpack_from("<BB6x11Q", payload, cursor)
                cursor += size
                if cpu in seen or cpu >= 8:
                    raise DecodeError("invalid or duplicate CPU snapshot ID")
                seen.add(cpu)
                cpus.append({"cpu": cpu, **{key: f"0x{value:x}" for key, value in zip(keys, values)}, "fidelity": fidelity})
            report["cpus"] = cpus
        else:
            raise DecodeError(f"unsupported CPU snapshot version {section.version}")
    elif section.kind == 10:
        if len(payload) != 64:
            raise DecodeError("invalid violation size")
        values = struct.unpack("<IBBBB7Q", payload)
        report["violation"] = {
            "id": values[0],
            "severity": values[1],
            "cpu": values[2],
            "mode": values[3],
            "domain": values[4],
            "epoch": values[5],
            "subject": values[6],
            "expected": [values[7], values[9]],
            "observed": [values[8], values[10]],
            "trace_sequence": values[11],
        }
    elif section.kind == 6:
        if len(payload) < 28:
            raise DecodeError("short scheduler shadow section")
        epoch, pending_operation, pending_subject, count = struct.unpack_from("<QQQI", payload)
        if len(payload) != 28 + count * 28:
            raise DecodeError("invalid scheduler shadow layout")
        entities = []
        cursor = 28
        for _ in range(count):
            key, state, cpu, affinity, generation, last_epoch, operation = struct.unpack_from(
                "<QBBBBQB7x", payload, cursor
            )
            cursor += 28
            entities.append(
                {
                    "key": f"0x{key:016x}",
                    "state": state,
                    "cpu": cpu,
                    "affinity": affinity,
                    "generation": generation,
                    "last_epoch": last_epoch,
                    "last_operation": operation,
                }
            )
        report.setdefault("shadow", {})["scheduler"] = {
            "epoch": epoch,
            "stable": not bool(section.flags & 1) and epoch % 2 == 0,
            "pending_operation": pending_operation,
            "pending_subject": f"0x{pending_subject:016x}",
            "entities": entities,
        }


def _signature(trigger: dict[str, Any], violation: dict[str, Any] | None) -> str:
    if violation:
        return f"INV-{violation['id']:08x}"
    return f"VEC-{trigger.get('vector', 255):02x}:{trigger.get('reason_hash', 'unknown')}"


def parse_stream(blob: bytes) -> list[dict[str, Any]]:
    reports = []
    cursor = 0
    while True:
        found = blob.find(MAGIC, cursor)
        if found < 0:
            break
        try:
            report, cursor = parse_capsule(blob, found)
        except DecodeError:
            cursor = found + 1
            continue
        reports.append(report)
    if not reports:
        raise DecodeError("no valid complete capsules found")
    return reports


def trust_manifest(report: dict[str, Any], manifest_path: pathlib.Path, elf: pathlib.Path | None) -> None:
    manifest = json.loads(manifest_path.read_text())
    trusted = manifest.get("build_id") == report["run"]["build_id"]
    if elf is not None:
        digest = hashlib.sha256(elf.read_bytes()).hexdigest()
        trusted = trusted and manifest.get("kernel_elf_sha256") == digest
    report["run"]["manifest_trusted"] = bool(trusted)
    if not trusted:
        report["inferences"].append("symbols_untrusted")


def render_markdown(report: dict[str, Any]) -> str:
    trigger = report["trigger"]
    lines = [
        "# AgenticOS crash report",
        "",
        f"- Signature: `{trigger['signature']}`",
        f"- Trigger: {trigger['kind']} on CPU {trigger['owner_cpu']}",
        f"- Build ID: `{report['run']['build_id']}`",
        f"- Captured CPUs: `0x{report['cpu_masks']['captured']:02x}` / online `0x{report['cpu_masks']['online']:02x}`",
    ]
    if report["missing"]:
        lines.extend(["", "Missing evidence: " + ", ".join(report["missing"])])
    return "\n".join(lines) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("capsule", type=pathlib.Path)
    parser.add_argument("--output-dir", type=pathlib.Path)
    parser.add_argument("--manifest", type=pathlib.Path)
    parser.add_argument("--elf", type=pathlib.Path)
    args = parser.parse_args()
    reports = parse_stream(args.capsule.read_bytes())
    output = args.output_dir or args.capsule.parent
    output.mkdir(parents=True, exist_ok=True)
    for report in reports:
        if args.manifest:
            trust_manifest(report, args.manifest, args.elf)
    primary = reports[0]
    (output / "report.json").write_text(json.dumps(primary, indent=2, sort_keys=True) + "\n")
    (output / "report.md").write_text(render_markdown(primary))
    if len(reports) > 1:
        incidents = output / "incidents"
        incidents.mkdir(exist_ok=True)
        for report in reports[1:]:
            (incidents / f"{report['record_sequence']:020d}.json").write_text(
                json.dumps(report, indent=2, sort_keys=True) + "\n"
            )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

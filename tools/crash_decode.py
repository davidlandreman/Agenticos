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
    7: "shadow_pager",
    8: "shadow_io",
    9: "shadow_continuation",
    10: "violation",
    11: "backtrace",
    12: "footer",
    13: "shadow_address_space",
    14: "shadow_stack",
    15: "shadow_memory",
    16: "shadow_locks",
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
        required.add(7)
        required.add(8)
        required.add(9)
        required.add(13)
        required.add(14)
        required.add(15)
        required.add(16)
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
    elif section.kind == 7:
        if len(payload) < 4:
            raise DecodeError("short pager shadow section")
        count = struct.unpack_from("<I", payload)[0]
        if len(payload) != 4 + count * 64:
            raise DecodeError("invalid pager shadow layout")
        transactions = []
        cursor = 4
        for _ in range(count):
            values = struct.unpack_from("<QQQQQIBBHIIQ", payload, cursor)
            cursor += 64
            transactions.append(
                {
                    "generation": values[0],
                    "l4": f"0x{values[1]:x}",
                    "vma_generation": values[2],
                    "page": f"0x{values[3]:x}",
                    "frame": f"0x{values[4]:x}",
                    "pid": values[5],
                    "state": values[6],
                    "terminal_reason": values[8],
                    "requested": values[9],
                    "actual": values[10],
                    "checksum": f"0x{values[11]:016x}",
                }
            )
        report.setdefault("shadow", {})["pager"] = {
            "stable": not bool(section.flags & 1),
            "transactions": transactions,
        }
    elif section.kind == 8:
        if len(payload) < 4:
            raise DecodeError("short I/O shadow section")
        count = struct.unpack_from("<I", payload)[0]
        if len(payload) != 4 + count * 40:
            raise DecodeError("invalid I/O shadow layout")
        requests = []
        cursor = 4
        for _ in range(count):
            values = struct.unpack_from("<QQIIIHHBB6x", payload, cursor)
            cursor += 40
            requests.append(
                {
                    "token": values[0],
                    "page_generation": values[1],
                    "pid": values[2],
                    "requested": values[3],
                    "actual": values[4],
                    "device": values[5],
                    "queue_head": values[6],
                    "state": values[7],
                    "status": values[8],
                }
            )
        report.setdefault("shadow", {})["io"] = {
            "stable": not bool(section.flags & 1),
            "requests": requests,
        }
    elif section.kind == 9:
        if len(payload) < 4:
            raise DecodeError("short continuation shadow section")
        count = struct.unpack_from("<I", payload)[0]
        if len(payload) != 4 + count * 72:
            raise DecodeError("invalid continuation shadow layout")
        continuations = []
        cursor = 4
        for _ in range(count):
            values = struct.unpack_from("<QQQQQQQQIBB2x", payload, cursor)
            cursor += 72
            continuations.append(
                {
                    "generation": values[0],
                    "token": values[1],
                    "stack_generation": values[2],
                    "rip": f"0x{values[3]:x}",
                    "rsp": f"0x{values[4]:x}",
                    "rflags": f"0x{values[5]:x}",
                    "stack_bottom": f"0x{values[6]:x}",
                    "stack_top": f"0x{values[7]:x}",
                    "pid": values[8],
                    "state": values[9],
                    "wake_pending_before_publish": bool(values[10] & 1),
                }
            )
        report.setdefault("shadow", {})["continuation"] = {
            "stable": not bool(section.flags & 1),
            "continuations": continuations,
        }
    elif section.kind == 13:
        if len(payload) < 4:
            raise DecodeError("short address-space shadow section")
        count = struct.unpack_from("<I", payload)[0]
        if len(payload) != 4 + count * 48:
            raise DecodeError("invalid address-space shadow layout")
        roots = []
        cursor = 4
        for _ in range(count):
            values = struct.unpack_from("<QQIHBBQQQ", payload, cursor)
            cursor += 48
            roots.append(
                {
                    "generation": values[0],
                    "l4": f"0x{values[1]:x}",
                    "owner_tgid": values[2],
                    "member_count": values[3],
                    "state": values[4],
                    "active_cpu_mask": values[5],
                    "vma_generation": values[6],
                    "last_epoch": values[7],
                }
            )
        report.setdefault("shadow", {})["address_space"] = {
            "stable": not bool(section.flags & 1),
            "roots": roots,
        }
    elif section.kind == 14:
        if len(payload) < 4:
            raise DecodeError("short stack shadow section")
        count = struct.unpack_from("<I", payload)[0]
        if len(payload) != 4 + count * 48:
            raise DecodeError("invalid stack shadow layout")
        stacks = []
        cursor = 4
        for _ in range(count):
            values = struct.unpack_from("<QQQIBBHQQ", payload, cursor)
            cursor += 48
            stacks.append(
                {
                    "generation": values[0],
                    "bottom": f"0x{values[1]:x}",
                    "top": f"0x{values[2]:x}",
                    "owner_pid": values[3],
                    "state": values[4],
                    "active_cpu": values[5],
                    "flags": values[6],
                    "last_rsp": f"0x{values[7]:x}",
                    "last_epoch": values[8],
                }
            )
        report.setdefault("shadow", {})["stack"] = {
            "stable": not bool(section.flags & 1),
            "stacks": stacks,
        }
    elif section.kind == 15:
        if len(payload) < 36:
            raise DecodeError("short memory shadow section")
        frame_count, mapping_capacity, mapping_count, max_probe, rejected, frame_size, sequence = struct.unpack_from(
            "<IIIIIIQ", payload
        )
        recent_count = struct.unpack_from("<I", payload, 32)[0]
        mapping_count_offset = 36 + recent_count * 28
        if frame_size != 24 or len(payload) < mapping_count_offset + 4:
            raise DecodeError("invalid memory shadow layout")
        frames = []
        cursor = 36
        for _ in range(recent_count):
            values = struct.unpack_from("<IIIHHHBBHHI", payload, cursor)
            cursor += 28
            frames.append(
                {
                    "index": values[0],
                    "allocation_generation": values[1],
                    "expected_refs": values[2],
                    "leaf_refs": values[3],
                    "page_table_refs": values[4],
                    "transient_refs": values[5],
                    "state": values[6],
                    "kind": values[7],
                    "last_alloc_site": values[8],
                    "last_release_site": values[9],
                    "last_epoch": values[10],
                }
            )
        recent_mapping_count = struct.unpack_from("<I", payload, mapping_count_offset)[0]
        cursor = mapping_count_offset + 4
        if len(payload) != cursor + recent_mapping_count * 40:
            raise DecodeError("invalid memory shadow layout")
        mappings = []
        for _ in range(recent_mapping_count):
            values = struct.unpack_from("<QQQIIIBBH", payload, cursor)
            cursor += 40
            mappings.append(
                {
                    "address_space_generation": values[0],
                    "virtual_page": f"0x{values[1]:x}",
                    "frame_address": f"0x{values[2]:x}",
                    "frame_generation": values[3],
                    "flags": f"0x{values[4]:x}",
                    "mapping_generation": values[5],
                    "state": values[6],
                    "probe_distance": values[7],
                }
            )
        report.setdefault("shadow", {})["memory"] = {
            "stable": not bool(section.flags & 1),
            "frame_count": frame_count,
            "mapping_capacity": mapping_capacity,
            "mapping_count": mapping_count,
            "max_probe_distance": max_probe,
            "rejected_insertions": rejected,
            "sequence": sequence,
            "recent_frames": frames,
            "recent_mappings": mappings,
        }
    elif section.kind == 16:
        if len(payload) < 4:
            raise DecodeError("short lock shadow section")
        count, reserved = struct.unpack_from("<HH", payload)
        if reserved != 0 or len(payload) != 4 + count * 48:
            raise DecodeError("invalid lock shadow layout")
        classes = []
        cursor = 4
        for _ in range(count):
            values = struct.unpack_from("<BBBBHHIQQQQHH", payload, cursor)
            cursor += 48
            classes.append(
                {
                    "class": values[0],
                    "owner_cpu": values[1],
                    "recursion_depth": values[2],
                    "waiters": values[4],
                    "owner_entity": values[6],
                    "acquire_site": f"0x{values[7]:016x}",
                    "acquire_tsc": values[8],
                    "acquisitions": values[9],
                    "failed_try_locks": values[10],
                    "order_edges": values[11],
                }
            )
        report.setdefault("shadow", {})["locks"] = {
            "stable": not bool(section.flags & 1),
            "classes": classes,
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

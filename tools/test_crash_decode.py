import binascii
import hashlib
import json
import pathlib
import struct
import sys
import tempfile
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).parent))
import crash_decode


def section(kind, payload=b"", version=1, flags=0):
    return struct.pack("<HHIII", kind, version, len(payload), flags, binascii.crc32(payload) & 0xFFFFFFFF) + payload


def capsule(sections=None, record_kind=1):
    payload = b"".join(sections or [section(99, b"future")])
    header = bytearray(
        struct.pack(
            "<8sHHIQ16s20sBBBBQII",
            crash_decode.MAGIC,
            1,
            80,
            80 + len(payload),
            0,
            bytes(16),
            bytes(range(20)),
            0,
            1,
            1,
            record_kind,
            1,
            binascii.crc32(payload) & 0xFFFFFFFF,
            0,
        )
    )
    struct.pack_into("<I", header, 76, binascii.crc32(header) & 0xFFFFFFFF)
    return bytes(header) + payload


class CrashDecodeTests(unittest.TestCase):
    def test_unknown_section_is_skipped(self):
        report, end = crash_decode.parse_capsule(capsule())
        self.assertEqual(end, len(capsule()))
        self.assertEqual(report["sections"][0]["kind"], "unknown_99")

    def test_bad_header_crc(self):
        data = bytearray(capsule())
        data[20] ^= 1
        with self.assertRaisesRegex(crash_decode.DecodeError, "header CRC"):
            crash_decode.parse_capsule(data)

    def test_bad_payload_crc(self):
        data = bytearray(capsule())
        data[-1] ^= 1
        with self.assertRaisesRegex(crash_decode.DecodeError, "payload CRC"):
            crash_decode.parse_capsule(data)

    def test_partial_is_rejected(self):
        with self.assertRaisesRegex(crash_decode.DecodeError, "truncated"):
            crash_decode.parse_capsule(capsule()[:-2])

    def test_invalid_enum(self):
        trigger = struct.pack("<BBBBQQQQQII", 9, 14, 2, 0, 0, 0, 0, 0, 0, 0, 0)
        with self.assertRaisesRegex(crash_decode.DecodeError, "invalid trigger kind"):
            crash_decode.parse_capsule(capsule([section(2, trigger)]))

    def test_duplicate_sections_are_explicit(self):
        report, _ = crash_decode.parse_capsule(capsule([section(99), section(99)]))
        self.assertEqual(report["inferences"], [{"duplicate_sections": [99]}])

    def test_stream_skips_partial_then_finds_valid(self):
        valid = capsule()
        reports = crash_decode.parse_stream(b"noise" + valid[:40] + b"junk" + valid)
        self.assertEqual(len(reports), 1)

    def test_v2_multi_cpu_snapshots(self):
        def cpu(cpu_id, fidelity, base):
            return struct.pack("<BB6x11Q", cpu_id, fidelity, *range(base, base + 11))

        payload = struct.pack("<BBH", 2, 0, 96) + cpu(0, 2, 10) + cpu(3, 2, 30)
        report, _ = crash_decode.parse_capsule(capsule([section(3, payload, version=2)]))
        self.assertEqual([entry["cpu"] for entry in report["cpus"]], [0, 3])
        self.assertEqual(report["cpus"][1]["cr3"], "0x24")

    def test_trace_footer_and_explicitly_unavailable_backtrace(self):
        record = struct.pack("<8Q", 7, 11, 13, 17, 19, 23, 29, 0x0100_0800)
        trace = struct.pack("<HHHHBBHQQQI", 1, 1024, 128, 0, 0, 0, 0, 8, 2, 1, 1) + record
        backtrace = struct.pack("<HBB", 0, 5, 0)
        footer = struct.pack("<QII", 0, 0x434F4D50, 0)
        report, _ = crash_decode.parse_capsule(
            capsule([section(5, trace), section(11, backtrace, flags=1), section(12, footer)])
        )
        self.assertEqual(report["trace"]["cpus"][0]["records"][0]["kind"], "lock_attempt")
        self.assertEqual(report["backtrace"]["unavailable_reason"], 5)
        self.assertTrue(report["footer"]["complete"])

    def test_trace_rejects_hostile_record_count(self):
        trace = struct.pack("<HHHHBBHQQQI", 1, 1024, 128, 0, 0, 0, 0, 8, 0, 0, 129)
        with self.assertRaisesRegex(crash_decode.DecodeError, "trace record count"):
            crash_decode.parse_capsule(capsule([section(5, trace)]))

    def test_missing_rich_sections_are_absent_evidence(self):
        metadata = struct.pack("<BBHI", 2, 0, 0, 3)
        for value in ("abc", "0", "rustc", "strict"):
            encoded = value.encode()
            metadata += struct.pack("<H", len(encoded)) + encoded
        report, _ = crash_decode.parse_capsule(capsule([section(1, metadata)]))
        self.assertIn("shadow_locks", report["missing"])
        self.assertIn("footer", report["missing"])

    def test_manifest_requires_run_build_mode_and_elf_identity(self):
        metadata = struct.pack("<BBHI", 2, 0, 0, 3)
        for value in ("abc", "0", "rustc", "strict"):
            encoded = value.encode()
            metadata += struct.pack("<H", len(encoded)) + encoded
        report, _ = crash_decode.parse_capsule(capsule([section(1, metadata)]))
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            elf = root / "kernel"
            elf.write_bytes(b"elf")
            manifest = root / "manifest.json"
            manifest.write_text(
                json.dumps(
                    {
                        "run_id": "00" * 16,
                        "build_id": bytes(range(20)).hex(),
                        "diagnostics": "strict",
                        "kernel_elf_sha256": hashlib.sha256(b"elf").hexdigest(),
                    }
                )
            )
            crash_decode.trust_manifest(report, manifest, elf)
            self.assertTrue(report["run"]["manifest_trusted"])
            self.assertTrue(report["run"]["symbols_trusted"])

            document = json.loads(manifest.read_text())
            document["run_id"] = "11" * 16
            manifest.write_text(json.dumps(document))
            crash_decode.trust_manifest(report, manifest, elf)
            self.assertFalse(report["run"]["manifest_trusted"])
            self.assertIn("run_id", report["inferences"][-1]["manifest_mismatches"])

            document["run_id"] = "00" * 16
            manifest.write_text(json.dumps(document))
            elf.write_bytes(b"different elf")
            crash_decode.trust_manifest(report, manifest, elf)
            self.assertTrue(report["run"]["manifest_trusted"])
            self.assertFalse(report["run"]["symbols_trusted"])
            self.assertIn(
                "kernel_elf_sha256", report["inferences"][-1]["manifest_mismatches"]
            )

    def test_pager_and_io_shadow_sections(self):
        pager_record = struct.pack(
            "<QQQQQIBBHIIQ",
            7,
            0x1000,
            3,
            0x4000,
            0x9000,
            42,
            4,
            0,
            0,
            4096,
            4096,
            0x1234,
        )
        io_record = struct.pack(
            "<QQIIIHHBB6x", 11, 7, 42, 4096, 4096, 1, 9, 5, 0
        )
        report, _ = crash_decode.parse_capsule(
            capsule(
                [
                    section(7, struct.pack("<I", 1) + pager_record),
                    section(8, struct.pack("<I", 1) + io_record),
                ]
            )
        )
        self.assertEqual(report["shadow"]["pager"]["transactions"][0]["page"], "0x4000")
        self.assertEqual(report["shadow"]["io"]["requests"][0]["token"], 11)

    def test_pager_shadow_rejects_bad_count(self):
        with self.assertRaisesRegex(crash_decode.DecodeError, "pager shadow layout"):
            crash_decode.parse_capsule(capsule([section(7, struct.pack("<I", 2))]))

    def test_continuation_shadow_section(self):
        continuation = struct.pack(
            "<QQQQQQQQIBB2x",
            5,
            17,
            9,
            0xFFFF800000001234,
            0xFFFF900000002000,
            0x202,
            0xFFFF900000001000,
            0xFFFF900000003000,
            42,
            3,
            1,
        )
        report, _ = crash_decode.parse_capsule(
            capsule([section(9, struct.pack("<I", 1) + continuation)])
        )
        saved = report["shadow"]["continuation"]["continuations"][0]
        self.assertEqual(saved["pid"], 42)
        self.assertEqual(saved["stack_generation"], 9)
        self.assertTrue(saved["wake_pending_before_publish"])

    def test_continuation_shadow_rejects_bad_count(self):
        with self.assertRaisesRegex(crash_decode.DecodeError, "continuation shadow layout"):
            crash_decode.parse_capsule(capsule([section(9, struct.pack("<I", 1))]))

    def test_address_space_and_stack_shadow_sections(self):
        root = struct.pack(
            "<QQIHBBQQQ", 3, 0x9000, 42, 2, 3, 1, 7, 11, 0
        )
        stack = struct.pack(
            "<QQQIBBHQQ", 5, 0x1000, 0x3000, 43, 3, 0, 0, 0x2FF0, 12
        )
        report, _ = crash_decode.parse_capsule(
            capsule(
                [
                    section(13, struct.pack("<I", 1) + root),
                    section(14, struct.pack("<I", 1) + stack),
                ]
            )
        )
        self.assertEqual(report["shadow"]["address_space"]["roots"][0]["owner_tgid"], 42)
        self.assertEqual(report["shadow"]["stack"]["stacks"][0]["owner_pid"], 43)

    def test_memory_shadow_section(self):
        frame = struct.pack(
            "<IIIHHHBBHHI", 17, 3, 2, 2, 0, 0, 2, 3, 0x1201, 0x1107, 99
        )
        mapping = struct.pack(
            "<QQQIIIBBH", 7, 0x4000, 0x9000, 3, 0x80000007, 12, 1, 2, 0
        )
        payload = (
            struct.pack("<IIIIIIQI", 65536, 65536, 21, 4, 0, 24, 18, 1)
            + frame
            + struct.pack("<I", 1)
            + mapping
        )
        report, _ = crash_decode.parse_capsule(capsule([section(15, payload)]))
        memory = report["shadow"]["memory"]
        self.assertEqual(memory["mapping_count"], 21)
        self.assertEqual(memory["recent_frames"][0]["allocation_generation"], 3)
        self.assertEqual(memory["recent_mappings"][0]["virtual_page"], "0x4000")

    def test_lock_shadow_section(self):
        lock = struct.pack(
            "<BBBBHHIQQQQHH",
            3,
            2,
            1,
            0,
            4,
            0,
            42,
            0x5678,
            0x1234,
            19,
            5,
            1 << 4,
            0,
        )
        report, _ = crash_decode.parse_capsule(
            capsule([section(16, struct.pack("<HH", 1, 0) + lock)])
        )
        locks = report["shadow"]["locks"]
        self.assertEqual(locks["classes"][0]["class"], 3)
        self.assertEqual(locks["classes"][0]["class_name"], "memory_mapper")
        self.assertEqual(locks["classes"][0]["owner_cpu"], 2)
        self.assertEqual(locks["classes"][0]["order_edges"], 1 << 4)
        self.assertEqual(locks["classes"][0]["order_edge_classes"], ["stack_allocator"])

    def test_lock_shadow_rejects_bad_count(self):
        with self.assertRaisesRegex(crash_decode.DecodeError, "lock shadow layout"):
            crash_decode.parse_capsule(
                capsule([section(16, struct.pack("<HH", 2, 0) + bytes(36))])
            )

    def test_cpu_handoff_shadow_section(self):
        cpu = struct.pack(
            "<BBBBIQ8QIIQ",
            2,
            3,
            0x1F,
            8,
            0,
            44,
            0x800000000000002A,
            0x12345000,
            9,
            0xFFFF800000008000,
            12,
            0x12345000,
            0xFFFF800000008000,
            0xFFFF800000008000,
            42,
            0,
            0,
        )
        report, _ = crash_decode.parse_capsule(
            capsule([section(17, struct.pack("<HH", 1, 96) + cpu)])
        )
        shadow = report["shadow"]["cpu"]
        self.assertTrue(shadow["stable"])
        self.assertEqual(shadow["cpus"][0]["phase_name"], "user_stable")
        self.assertEqual(shadow["cpus"][0]["observed_pid"], 42)
        self.assertEqual(shadow["cpus"][0]["last_operation_name"], "commit_user")

    def test_cpu_handoff_shadow_rejects_bad_record_size(self):
        with self.assertRaisesRegex(crash_decode.DecodeError, "CPU handoff shadow layout"):
            crash_decode.parse_capsule(
                capsule([section(17, struct.pack("<HH", 1, 95) + bytes(95))])
            )


if __name__ == "__main__":
    unittest.main()

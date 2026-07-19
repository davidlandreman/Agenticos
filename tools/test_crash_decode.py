import binascii
import pathlib
import struct
import sys
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


if __name__ == "__main__":
    unittest.main()

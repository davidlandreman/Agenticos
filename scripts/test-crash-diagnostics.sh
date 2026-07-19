#!/bin/bash
set -eu

CASE_NAME="${1:-panic}"
case "$CASE_NAME" in
    panic) MISSING_CPU=""; DIAGNOSTICS=record; INJECT=panic ;;
    missing-cpu) MISSING_CPU=3; DIAGNOSTICS=record; INJECT=panic ;;
    sched-duplicate) MISSING_CPU=""; DIAGNOSTICS=strict; INJECT=sched-duplicate ;;
    *) echo "supported crash cases: panic, missing-cpu, sched-duplicate" >&2; exit 2 ;;
esac

RUN_ID="$(python3 -c 'import uuid; print(uuid.uuid4().hex)')"
ARTIFACT_DIR="${AGENTICOS_CRASH_DIR:-$(pwd)/.context/crashes/$RUN_ID}"
mkdir -p "$ARTIFACT_DIR"

set +e
AGENTICOS_DIAGNOSTICS="$DIAGNOSTICS" \
AGENTICOS_RUN_ID="$RUN_ID" \
AGENTICOS_CRASH_DIR="$ARTIFACT_DIR" \
AGENTICOS_CRASH_INJECT="$INJECT" \
AGENTICOS_CRASH_MISSING_CPU="$MISSING_CPU" \
AGENTICOS_QEMU_SMP=4 \
./test.sh --skip-userland diagnostics
STATUS=$?
set -e

if [ "$STATUS" -eq 0 ]; then
    echo "expected injected crash, but tests passed" >&2
    exit 1
fi
python3 tools/crash_decode.py \
    "$ARTIFACT_DIR/capsule.bin" \
    --output-dir "$ARTIFACT_DIR" \
    --manifest "$ARTIFACT_DIR/manifest.json" \
    --elf target/x86_64-unknown-none/release/agenticos
python3 - "$ARTIFACT_DIR/report.json" "$CASE_NAME" <<'PY'
import json
import pathlib
import sys

report = json.loads(pathlib.Path(sys.argv[1]).read_text())
case = sys.argv[2]
expected_kind = "invariant" if case == "sched-duplicate" else "fatal"
assert report["trigger"]["kind"] == expected_kind, report
assert report["run"]["manifest_trusted"] is True, report
assert not report["missing"], report
assert report["cpu_masks"]["online"] == 0x0f, report
if case in ("panic", "sched-duplicate"):
    assert report["cpu_masks"]["captured"] == 0x0f, report
    assert len(report["cpus"]) == 4, report
    assert report["flags"] & 0x04 == 0, report
else:
    assert report["cpu_masks"]["captured"] == 0x07, report
    assert len(report["cpus"]) == 3, report
    assert report["flags"] & 0x04, report
if case == "sched-duplicate":
    assert report["violation"]["id"] == 0x01000001, report
    assert report["trigger"]["signature"] == "INV-01000001", report
print(f"validated {report['trigger']['signature']}: {sys.argv[1]}")
PY

#!/bin/bash
set -eu

CASE_NAME="${1:-panic}"
case "$CASE_NAME" in
    panic) MISSING_CPU=""; DIAGNOSTICS=record; INJECT=panic ;;
    missing-cpu) MISSING_CPU=3; DIAGNOSTICS=record; INJECT=panic ;;
    sched-duplicate) MISSING_CPU=""; DIAGNOSTICS=strict; INJECT=sched-duplicate ;;
    cont-signal-wake) MISSING_CPU=""; DIAGNOSTICS=strict; INJECT=cont-signal-wake ;;
    cont-invalid-stack) MISSING_CPU=""; DIAGNOSTICS=strict; INJECT=cont-invalid-stack ;;
    as-destroy-active) MISSING_CPU=""; DIAGNOSTICS=strict; INJECT=as-destroy-active ;;
    stack-retire-active) MISSING_CPU=""; DIAGNOSTICS=strict; INJECT=stack-retire-active ;;
    *) echo "supported crash cases: panic, missing-cpu, sched-duplicate, cont-signal-wake, cont-invalid-stack, as-destroy-active, stack-retire-active" >&2; exit 2 ;;
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
invariant_cases = (
    "sched-duplicate",
    "cont-signal-wake",
    "cont-invalid-stack",
    "as-destroy-active",
    "stack-retire-active",
)
expected_kind = "invariant" if case in invariant_cases else "fatal"
assert report["trigger"]["kind"] == expected_kind, report
assert report["run"]["manifest_trusted"] is True, report
assert not report["missing"], report
assert report["cpu_masks"]["online"] == 0x0f, report
if case == "panic" or case in invariant_cases:
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
if case == "cont-signal-wake":
    assert report["violation"]["id"] == 0x05000004, report
    assert report["trigger"]["signature"] == "INV-05000004", report
if case == "cont-invalid-stack":
    assert report["violation"]["id"] == 0x05000002, report
    assert report["trigger"]["signature"] == "INV-05000002", report
if case == "as-destroy-active":
    assert report["violation"]["id"] == 0x06000003, report
    assert report["trigger"]["signature"] == "INV-06000003", report
if case == "stack-retire-active":
    assert report["violation"]["id"] == 0x07000001, report
    assert report["trigger"]["signature"] == "INV-07000001", report
print(f"validated {report['trigger']['signature']}: {sys.argv[1]}")
PY

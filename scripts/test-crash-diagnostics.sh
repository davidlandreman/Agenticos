#!/bin/bash
set -eu

CASE_NAME="${1:-panic}"
case "$CASE_NAME" in
    panic) MISSING_CPU=""; DIAGNOSTICS=record; INJECT=panic ;;
    fatal-page-fault) MISSING_CPU=""; DIAGNOSTICS=record; INJECT=fatal-page-fault ;;
    missing-cpu) MISSING_CPU=3; DIAGNOSTICS=record; INJECT=panic ;;
    sched-duplicate) MISSING_CPU=""; DIAGNOSTICS=strict; INJECT=sched-duplicate ;;
    cont-signal-wake) MISSING_CPU=""; DIAGNOSTICS=strict; INJECT=cont-signal-wake ;;
    cont-invalid-stack) MISSING_CPU=""; DIAGNOSTICS=strict; INJECT=cont-invalid-stack ;;
    pager-short-read) MISSING_CPU=""; DIAGNOSTICS=strict; INJECT=pager-short-read ;;
    io-wrong-wake) MISSING_CPU=""; DIAGNOSTICS=strict; INJECT=io-wrong-wake ;;
    io-lost-wake) MISSING_CPU=""; DIAGNOSTICS=strict; INJECT=io-lost-wake ;;
    io-double-complete) MISSING_CPU=""; DIAGNOSTICS=strict; INJECT=io-double-complete ;;
    io-early-consume) MISSING_CPU=""; DIAGNOSTICS=strict; INJECT=io-early-consume ;;
    as-destroy-active) MISSING_CPU=""; DIAGNOSTICS=strict; INJECT=as-destroy-active ;;
    stack-retire-active) MISSING_CPU=""; DIAGNOSTICS=strict; INJECT=stack-retire-active ;;
    mm-double-release) MISSING_CPU=""; DIAGNOSTICS=strict; INJECT=mm-double-release ;;
    mm-wrong-unmap) MISSING_CPU=""; DIAGNOSTICS=strict; INJECT=mm-wrong-unmap ;;
    mm-wx) MISSING_CPU=""; DIAGNOSTICS=strict; INJECT=mm-wx ;;
    lock-recursion) MISSING_CPU=""; DIAGNOSTICS=strict; INJECT=lock-recursion ;;
    lock-wrong-owner) MISSING_CPU=""; DIAGNOSTICS=strict; INJECT=lock-wrong-owner ;;
    lock-wrong-context) MISSING_CPU=""; DIAGNOSTICS=strict; INJECT=lock-wrong-context ;;
    lock-cycle) MISSING_CPU=""; DIAGNOSTICS=strict; INJECT=lock-cycle ;;
    cpu-wrong-cr3) MISSING_CPU=""; DIAGNOSTICS=strict; INJECT=cpu-wrong-cr3 ;;
    cpu-wrong-order) MISSING_CPU=""; DIAGNOSTICS=strict; INJECT=cpu-wrong-order ;;
    cpu-wrong-pid) MISSING_CPU=""; DIAGNOSTICS=strict; INJECT=cpu-wrong-pid ;;
    cpu-kernel-cr3) MISSING_CPU=""; DIAGNOSTICS=strict; INJECT=cpu-kernel-cr3 ;;
    cpu-wrong-publication) MISSING_CPU=""; DIAGNOSTICS=strict; INJECT=cpu-wrong-publication ;;
    *) echo "supported crash cases: panic, fatal-page-fault, missing-cpu, sched-duplicate, cont-signal-wake, cont-invalid-stack, pager-short-read, io-wrong-wake, io-lost-wake, io-double-complete, io-early-consume, as-destroy-active, stack-retire-active, mm-double-release, mm-wrong-unmap, mm-wx, lock-recursion, lock-wrong-owner, lock-wrong-context, lock-cycle, cpu-wrong-cr3, cpu-wrong-order, cpu-wrong-pid, cpu-kernel-cr3, cpu-wrong-publication" >&2; exit 2 ;;
esac

RUN_ID="$(python3 -c 'import uuid; print(uuid.uuid4().hex)')"
ARTIFACT_DIR="${AGENTICOS_CRASH_DIR:-$(pwd)/.context/crashes/$RUN_ID}"
TIMEOUT_SECONDS="${AGENTICOS_CRASH_TIMEOUT_SECONDS:-180}"
mkdir -p "$ARTIFACT_DIR"

set +e
AGENTICOS_DIAGNOSTICS="$DIAGNOSTICS" \
AGENTICOS_RUN_ID="$RUN_ID" \
AGENTICOS_CRASH_DIR="$ARTIFACT_DIR" \
AGENTICOS_CRASH_INJECT="$INJECT" \
AGENTICOS_CRASH_MISSING_CPU="$MISSING_CPU" \
AGENTICOS_QEMU_SMP=4 \
python3 tools/run_with_timeout.py --seconds "$TIMEOUT_SECONDS" -- \
    ./test.sh --skip-userland diagnostics
STATUS=$?
set -e

if [ "$STATUS" -eq 124 ]; then
    echo "crash case $CASE_NAME exceeded ${TIMEOUT_SECONDS}s hard timeout" >&2
    exit 1
fi
if [ "$STATUS" -eq 0 ]; then
    echo "expected injected crash, but tests passed" >&2
    exit 1
fi
if [ ! -s "$ARTIFACT_DIR/capsule.bin" ] || [ ! -s "$ARTIFACT_DIR/manifest.json" ]; then
    echo "crash case $CASE_NAME exited without a complete artifact set" >&2
    exit 1
fi
cp target/x86_64-unknown-none/release/agenticos "$ARTIFACT_DIR/kernel.elf"
printf '%s\n' "$ARTIFACT_DIR/kernel.elf" > "$ARTIFACT_DIR/kernel.elf.ref"
python3 tools/crash_decode.py \
    "$ARTIFACT_DIR/capsule.bin" \
    --output-dir "$ARTIFACT_DIR" \
    --manifest "$ARTIFACT_DIR/manifest.json" \
    --elf "$ARTIFACT_DIR/kernel.elf"
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
    "pager-short-read",
    "io-wrong-wake",
    "io-lost-wake",
    "io-double-complete",
    "io-early-consume",
    "as-destroy-active",
    "stack-retire-active",
    "mm-double-release",
    "mm-wrong-unmap",
    "mm-wx",
    "lock-recursion",
    "lock-wrong-owner",
    "lock-wrong-context",
    "lock-cycle",
    "cpu-wrong-cr3",
    "cpu-wrong-order",
    "cpu-wrong-pid",
    "cpu-kernel-cr3",
    "cpu-wrong-publication",
)
expected_kind = "invariant" if case in invariant_cases else "fatal"
assert report["trigger"]["kind"] == expected_kind, report
assert report["run"]["manifest_trusted"] is True, report
assert report["run"]["symbols_trusted"] is True, report
assert not report["missing"], report
assert report["footer"]["complete"] is True, report
assert report["backtrace"]["frames"] or report["backtrace"]["unavailable_reason"] != 0, report
assert report["cpu_masks"]["online"] == 0x0f, report
if case in ("panic", "fatal-page-fault") or case in invariant_cases:
    assert report["cpu_masks"]["captured"] == 0x0f, report
    assert len(report["cpus"]) == 4, report
    assert report["flags"] & 0x04 == 0, report
else:
    assert report["cpu_masks"]["captured"] == 0x07, report
    assert len(report["cpus"]) == 3, report
    assert report["flags"] & 0x04, report
if case in ("panic", "missing-cpu"):
    assert report["trigger"]["signature"] == "VEC-ff:0xf3243b8bc636f3bd", report
if case == "fatal-page-fault":
    assert report["trigger"]["vector"] == 14, report
    assert report["trigger"]["fault_address"] == "0xfffff00000000000", report
if case == "sched-duplicate":
    assert report["violation"]["id"] == 0x01000001, report
    assert report["trigger"]["signature"] == "INV-01000001", report
if case == "cont-signal-wake":
    assert report["violation"]["id"] == 0x05000004, report
    assert report["trigger"]["signature"] == "INV-05000004", report
if case == "cont-invalid-stack":
    assert report["violation"]["id"] == 0x05000002, report
    assert report["trigger"]["signature"] == "INV-05000002", report
if case == "pager-short-read":
    assert report["violation"]["id"] == 0x03000004, report
    assert report["violation"]["expected"][0] == 4096, report
    assert report["violation"]["observed"][0] == 2048, report
    assert report["trigger"]["signature"] == "INV-03000004", report
if case == "io-wrong-wake":
    assert report["violation"]["id"] == 0x04000003, report
    assert report["trigger"]["signature"] == "INV-04000003", report
if case == "io-lost-wake":
    assert report["violation"]["id"] == 0x04000004, report
    assert report["trigger"]["signature"] == "INV-04000004", report
if case == "io-double-complete":
    assert report["violation"]["id"] == 0x04000001, report
    assert report["violation"]["expected"][0] == 1, report
    assert report["violation"]["observed"][0] == 2, report
    assert report["trigger"]["signature"] == "INV-04000001", report
if case == "io-early-consume":
    assert report["violation"]["id"] == 0x04000002, report
    assert report["violation"]["expected"][0] == 4, report
    assert report["violation"]["observed"][0] == 1, report
    assert report["trigger"]["signature"] == "INV-04000002", report
if case == "as-destroy-active":
    assert report["violation"]["id"] == 0x06000003, report
    assert report["trigger"]["signature"] == "INV-06000003", report
if case == "stack-retire-active":
    assert report["violation"]["id"] == 0x07000001, report
    assert report["trigger"]["signature"] == "INV-07000001", report
if case == "mm-double-release":
    assert report["violation"]["id"] == 0x08000002, report
    assert report["trigger"]["signature"] == "INV-08000002", report
if case == "mm-wrong-unmap":
    assert report["violation"]["id"] == 0x08000001, report
    assert report["trigger"]["signature"] == "INV-08000001", report
if case == "mm-wx":
    assert report["violation"]["id"] == 0x08000004, report
    assert report["trigger"]["signature"] == "INV-08000004", report
if case == "lock-recursion":
    assert report["violation"]["id"] == 0x09000002, report
    assert report["trigger"]["signature"] == "INV-09000002", report
if case == "lock-wrong-owner":
    assert report["violation"]["id"] == 0x09000001, report
    assert report["trigger"]["signature"] == "INV-09000001", report
if case == "lock-wrong-context":
    assert report["violation"]["id"] == 0x09000003, report
    assert report["trigger"]["signature"] == "INV-09000003", report
if case == "lock-cycle":
    assert report["violation"]["id"] == 0x09000004, report
    assert report["trigger"]["signature"] == "INV-09000004", report
if case == "cpu-wrong-cr3":
    assert report["violation"]["id"] == 0x02000002, report
    assert report["trigger"]["signature"] == "INV-02000002", report
if case == "cpu-wrong-order":
    assert report["violation"]["id"] == 0x02000005, report
    assert report["trigger"]["signature"] == "INV-02000005", report
if case == "cpu-wrong-pid":
    assert report["violation"]["id"] == 0x02000001, report
    assert report["trigger"]["signature"] == "INV-02000001", report
if case == "cpu-kernel-cr3":
    assert report["violation"]["id"] == 0x02000003, report
    assert report["trigger"]["signature"] == "INV-02000003", report
if case == "cpu-wrong-publication":
    assert report["violation"]["id"] == 0x02000004, report
    assert report["trigger"]["signature"] == "INV-02000004", report

io_expectations = {
    "cont-signal-wake": (0xFEED0001, ["submitted", "completed", "wake_queued", "generic_wake_rejected"]),
    "io-wrong-wake": (0xFEED0010, ["submitted", "completed", "wake_queued", "wrong_wake"]),
    "io-lost-wake": (0xFEED0011, ["submitted", "completed", "wake_lost"]),
    "io-double-complete": (0xFEED0012, ["submitted", "completed"]),
    "io-early-consume": (0xFEED0013, ["submitted"]),
}
if case in io_expectations:
    token, expected_phases = io_expectations[case]
    events = [
        record["operands"]
        for cpu in report["trace"]["cpus"]
        for record in cpu["records"]
        if record.get("operands", {}).get("token") == token
    ]
    assert [event["phase"] for event in events] == expected_phases, events
    assert all(event["page_generation"] != 0 for event in events), events
    if case == "cont-signal-wake":
        assert events[1]["status"] == 0 and events[1]["actual_bytes"] == 4096, events
    if case == "io-wrong-wake":
        assert events[-1]["awaited_token"] == token + 1, events
print(f"validated {report['trigger']['signature']}: {sys.argv[1]}")
PY

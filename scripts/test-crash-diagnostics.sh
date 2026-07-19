#!/bin/bash
set -eu

CASE_NAME="${1:-panic}"
if [ "$CASE_NAME" != panic ]; then
    echo "supported crash case: panic" >&2
    exit 2
fi

RUN_ID="$(python3 -c 'import uuid; print(uuid.uuid4().hex)')"
ARTIFACT_DIR="${AGENTICOS_CRASH_DIR:-$(pwd)/.context/crashes/$RUN_ID}"
mkdir -p "$ARTIFACT_DIR"

set +e
AGENTICOS_DIAGNOSTICS=record \
AGENTICOS_RUN_ID="$RUN_ID" \
AGENTICOS_CRASH_DIR="$ARTIFACT_DIR" \
AGENTICOS_CRASH_INJECT="$CASE_NAME" \
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
python3 - "$ARTIFACT_DIR/report.json" <<'PY'
import json
import pathlib
import sys

report = json.loads(pathlib.Path(sys.argv[1]).read_text())
assert report["trigger"]["kind"] == "fatal", report
assert report["run"]["manifest_trusted"] is True, report
assert not report["missing"], report
print(f"validated {report['trigger']['signature']}: {sys.argv[1]}")
PY

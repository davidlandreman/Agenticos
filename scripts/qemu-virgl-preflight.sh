#!/bin/bash
# Read-only qualification for the pinned macOS QEMU/VirGL candidate.
#
# This script never installs or relinks Homebrew formulae. Pass the exact
# qemu-system-x86_64 binary either as argv[1] or AGENTICOS_QEMU_BIN.

set -euo pipefail

EXPECTED_RELEASE="${AGENTICOS_QEMU_VIRGL_RELEASE:-1.0.27}"
EXPECTED_BOTTLE_SHA256="${AGENTICOS_QEMU_VIRGL_BOTTLE_SHA256:-a2eaeed6f7b52661436052b413f596785c5e14e2e1b65cd5509713fcfc164566}"
EXPECTED_QEMU_COMMIT="${AGENTICOS_QEMU_VIRGL_COMMIT:-cf3e71d8fc8ba681266759bb6cb2e45a45983e3e}"
EXPECTED_VIRGL_VERSION="${AGENTICOS_QEMU_VIRGLRENDERER_VERSION:-1.0.33}"
EXPECTED_ANGLE_VERSION="${AGENTICOS_QEMU_ANGLE_VERSION:-1.0.15}"
EXPECTED_EPOXY_VERSION="${AGENTICOS_QEMU_LIBEPOXY_VERSION:-1.0.4}"
GL_MODE="${AGENTICOS_QEMU_GL:-es}"
QEMU_INPUT="${1:-${AGENTICOS_QEMU_BIN:-}}"
RECORD_PATH="${AGENTICOS_QEMU_QUALIFICATION_RECORD:-.context/qemu-virgl-qualification.json}"

fail() {
    echo "VirGL preflight failed: $*" >&2
    exit 1
}

json_escape() {
    printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g; s/	/\\t/g'
}

resolve_binary() {
    local candidate=$1
    if [[ "$candidate" != /* ]]; then
        candidate=$(command -v "$candidate" 2>/dev/null || true)
    fi
    [[ -n "$candidate" && -x "$candidate" ]] || fail "binary is missing or not executable: ${1:-<unset>}"
    if command -v realpath >/dev/null 2>&1; then
        realpath "$candidate"
    else
        local directory
        directory=$(cd "$(dirname "$candidate")" && pwd -P)
        printf '%s/%s\n' "$directory" "$(basename "$candidate")"
    fi
}

resolve_macho_path() {
    local dependency=$1 loader=$2 executable_dir=$3 rpath resolved
    case "$dependency" in
        /System/Library/*|/usr/lib/*)
            # Current macOS releases keep many system images only in the dyld
            # shared cache, so a valid load command need not have a filesystem
            # entry at the same path.
            printf '%s\n' "$dependency"
            return
            ;;
        /*) [[ -e "$dependency" ]] && printf '%s\n' "$dependency"; return ;;
        @loader_path/*)
            resolved="$(dirname "$loader")/${dependency#@loader_path/}"
            [[ -e "$resolved" ]] && printf '%s\n' "$resolved"
            return
            ;;
        @executable_path/*)
            resolved="$executable_dir/${dependency#@executable_path/}"
            [[ -e "$resolved" ]] && printf '%s\n' "$resolved"
            return
            ;;
        @rpath/*)
            while IFS= read -r rpath; do
                rpath=${rpath//@loader_path/$(dirname "$loader")}
                rpath=${rpath//@executable_path/$executable_dir}
                resolved="$rpath/${dependency#@rpath/}"
                if [[ -e "$resolved" ]]; then
                    printf '%s\n' "$resolved"
                    return
                fi
            done < <(otool -l "$loader" | awk '/cmd LC_RPATH/{getline; getline; print $2}')
            ;;
    esac
}

resolve_rpath_library() {
    local loader=$1 executable_dir=$2 library=$3 rpath resolved
    while IFS= read -r rpath; do
        rpath=${rpath//@loader_path/$(dirname "$loader")}
        rpath=${rpath//@executable_path/$executable_dir}
        resolved="$rpath/$library"
        if [[ -e "$resolved" ]]; then
            printf '%s\n' "$resolved"
            return
        fi
    done < <(otool -l "$loader" | awk '/cmd LC_RPATH/{getline; getline; print $2}')
}

canonical_path() {
    if command -v realpath >/dev/null 2>&1; then
        realpath "$1"
    else
        printf '%s\n' "$1"
    fi
}

verify_macho_dependencies() {
    local image=$1 executable_dir=$2 dependency resolved image_id
    image_id=$(otool -D "$image" 2>/dev/null | sed -n '2p')
    while IFS= read -r dependency; do
        [[ -n "$dependency" ]] || continue
        [[ -n "$image_id" && "$dependency" == "$image_id" ]] && continue
        resolved=$(resolve_macho_path "$dependency" "$image" "$executable_dir" || true)
        [[ -n "$resolved" ]] || fail "unresolved Mach-O dependency from $image: $dependency"
        if [[ "$resolved" != /System/Library/* && "$resolved" != /usr/lib/* && ! -e "$resolved" ]]; then
            fail "unresolved Mach-O dependency from $image: $dependency"
        fi
    done < <(otool -L "$image" | awk 'NR > 1 {print $1}')
}

QEMU_BIN=$(resolve_binary "$QEMU_INPUT")
[[ "$(uname -s)" == Darwin ]] || fail "the qualified frontend is macOS Cocoa, host is $(uname -s)"
[[ "$(uname -m)" == arm64 ]] || fail "the pinned bottle is arm64, host is $(uname -m)"
[[ "$GL_MODE" == es || "$GL_MODE" == core ]] || fail "AGENTICOS_QEMU_GL must be es or core"
if [[ " ${AGENTICOS_QEMU_ACCEL:-} ${AGENTICOS_QEMU_EXTRA_ARGS:-} " == *"hvf"* ]]; then
    fail "x86-64-on-Apple-Silicon policy requires TCG; injected HVF was rejected"
fi

HOST_VERSION=$(sw_vers -productVersion)
QEMU_VERSION=$($QEMU_BIN --version | head -n 1)
QEMU_PREFIX=$(cd "$(dirname "$QEMU_BIN")/.." && pwd -P)
RECEIPT="$QEMU_PREFIX/INSTALL_RECEIPT.json"

echo "Host: macOS $HOST_VERSION ($(uname -m))"
echo "QEMU: $QEMU_BIN"
echo "Version: $QEMU_VERSION"
echo "Expected custom release: $EXPECTED_RELEASE (upstream $EXPECTED_QEMU_COMMIT)"

[[ "$QEMU_BIN" == */Cellar/qemu/"$EXPECTED_RELEASE"/bin/qemu-system-x86_64 ]] \
    || fail "expected the fully qualified custom keg .../Cellar/qemu/$EXPECTED_RELEASE/bin/qemu-system-x86_64"
[[ -r "$RECEIPT" ]] || fail "Homebrew installation receipt is missing: $RECEIPT"
grep -Eq '"poured_from_bottle"[[:space:]]*:[[:space:]]*true' "$RECEIPT" \
    || fail "release $EXPECTED_RELEASE was not poured from the checksum-qualified bottle"

DEVICE_HELP=$($QEMU_BIN -device help 2>&1)
grep -q 'virtio-vga-gl' <<<"$DEVICE_HELP" || fail "virtio-vga-gl is not advertised"
ACCEL_HELP=$($QEMU_BIN -accel help 2>&1)
grep -Eq '(^|[[:space:]])tcg($|[[:space:]])' <<<"$ACCEL_HELP" || fail "TCG accelerator is unavailable"

command -v otool >/dev/null 2>&1 || fail "otool is required to inspect Mach-O dependencies"
EXECUTABLE_DIR=$(dirname "$QEMU_BIN")
DEPENDENCY_REPORT=$(mktemp -t agenticos-virgl-deps.XXXXXX)
LAUNCH_LOG=$(mktemp -t agenticos-virgl-launch.XXXXXX)
QEMU_PID=""
cleanup() {
    if [[ -n "$QEMU_PID" ]] && kill -0 "$QEMU_PID" 2>/dev/null; then
        kill "$QEMU_PID" 2>/dev/null || true
        wait "$QEMU_PID" 2>/dev/null || true
    fi
    rm -f "$DEPENDENCY_REPORT" "$LAUNCH_LOG"
}
trap cleanup EXIT

otool -L "$QEMU_BIN" >"$DEPENDENCY_REPORT"
VIRGL_DEP=$(awk 'NR > 1 {print $1}' "$DEPENDENCY_REPORT" | grep -i 'virglrenderer' | head -n 1 || true)
[[ -n "$VIRGL_DEP" ]] || fail "QEMU is not linked to virglrenderer"
VIRGL_PATH=$(resolve_macho_path "$VIRGL_DEP" "$QEMU_BIN" "$EXECUTABLE_DIR" || true)
[[ -n "$VIRGL_PATH" && -r "$VIRGL_PATH" ]] || fail "cannot resolve virglrenderer dependency: $VIRGL_DEP"
VIRGL_PATH=$(canonical_path "$VIRGL_PATH")
otool -L "$VIRGL_PATH" >>"$DEPENDENCY_REPORT"

EPOXY_DEP=$(awk 'NR > 1 {print $1}' "$DEPENDENCY_REPORT" | grep -i 'epoxy' | head -n 1 || true)
[[ -n "$EPOXY_DEP" ]] || fail "libepoxy is absent from the QEMU/virglrenderer dependency graph"
EPOXY_PATH=$(resolve_macho_path "$EPOXY_DEP" "$VIRGL_PATH" "$EXECUTABLE_DIR" || true)
[[ -n "$EPOXY_PATH" && -r "$EPOXY_PATH" ]] || fail "cannot resolve libepoxy dependency: $EPOXY_DEP"
EPOXY_PATH=$(canonical_path "$EPOXY_PATH")

# libepoxy deliberately loads ANGLE with dlopen("libEGL.dylib") rather than a
# Mach-O load command. Resolve the same LC_RPATH search that dyld will use and
# require both libraries needed by the VirGL GLES path.
grep -aq 'libEGL\.dylib' "$EPOXY_PATH" \
    || fail "libepoxy does not advertise its ANGLE EGL runtime load"
ANGLE_PATH=$(resolve_rpath_library "$EPOXY_PATH" "$EXECUTABLE_DIR" libEGL.dylib || true)
ANGLE_GLES_PATH=$(resolve_rpath_library "$EPOXY_PATH" "$EXECUTABLE_DIR" libGLESv2.dylib || true)
[[ -n "$ANGLE_PATH" && -r "$ANGLE_PATH" ]] || fail "cannot resolve ANGLE libEGL.dylib through libepoxy LC_RPATH"
[[ -n "$ANGLE_GLES_PATH" && -r "$ANGLE_GLES_PATH" ]] || fail "cannot resolve ANGLE libGLESv2.dylib through libepoxy LC_RPATH"
ANGLE_PATH=$(canonical_path "$ANGLE_PATH")
ANGLE_GLES_PATH=$(canonical_path "$ANGLE_GLES_PATH")

verify_macho_dependencies "$QEMU_BIN" "$EXECUTABLE_DIR"
verify_macho_dependencies "$VIRGL_PATH" "$EXECUTABLE_DIR"
verify_macho_dependencies "$EPOXY_PATH" "$EXECUTABLE_DIR"
verify_macho_dependencies "$ANGLE_PATH" "$EXECUTABLE_DIR"
verify_macho_dependencies "$ANGLE_GLES_PATH" "$EXECUTABLE_DIR"
[[ "$VIRGL_PATH" == */Cellar/virglrenderer/"$EXPECTED_VIRGL_VERSION"/* ]] \
    || fail "expected virglrenderer $EXPECTED_VIRGL_VERSION, resolved $VIRGL_PATH"
[[ "$ANGLE_PATH" == */Cellar/angle/"$EXPECTED_ANGLE_VERSION"/* ]] \
    || fail "expected ANGLE $EXPECTED_ANGLE_VERSION, resolved $ANGLE_PATH"
[[ "$ANGLE_GLES_PATH" == */Cellar/angle/"$EXPECTED_ANGLE_VERSION"/* ]] \
    || fail "expected ANGLE GLES $EXPECTED_ANGLE_VERSION, resolved $ANGLE_GLES_PATH"
[[ "$EPOXY_PATH" == */Cellar/libepoxy/"$EXPECTED_EPOXY_VERSION"/* ]] \
    || fail "expected libepoxy $EXPECTED_EPOXY_VERSION, resolved $EPOXY_PATH"

echo "VirGL dependency: $VIRGL_PATH"
echo "ANGLE dependency: $ANGLE_PATH"
echo "ANGLE GLES dependency: $ANGLE_GLES_PATH"
echo "libepoxy dependency: $EPOXY_PATH"

# Keep the Cocoa frontend alive just long enough to prove that option parsing,
# GL context creation, and the virtio-vga-gl device initialize together. The
# VM is paused and has no disks; termination is deliberate and read-only.
if [[ "${AGENTICOS_QEMU_PREFLIGHT_SKIP_LAUNCH:-0}" != 1 ]]; then
    "$QEMU_BIN" \
        -machine q35 -accel tcg -nodefaults -S \
        -display "cocoa,gl=$GL_MODE" -device virtio-vga-gl \
        >"$LAUNCH_LOG" 2>&1 &
    QEMU_PID=$!
    sleep 2
    if ! kill -0 "$QEMU_PID" 2>/dev/null; then
        wait "$QEMU_PID" || true
        sed -n '1,120p' "$LAUNCH_LOG" >&2
        fail "Cocoa gl=$GL_MODE did not remain initialized"
    fi
    kill "$QEMU_PID" 2>/dev/null || true
    wait "$QEMU_PID" 2>/dev/null || true
    QEMU_PID=""
    if grep -Eiv 'NO_ERROR' "$LAUNCH_LOG" \
        | grep -Eiq 'failed to initialize|could not initialize|opengl is not available|egl.*error|loader.*error'; then
        sed -n '1,120p' "$LAUNCH_LOG" >&2
        fail "Cocoa gl=$GL_MODE reported a loader or renderer error"
    fi
fi

mkdir -p "$(dirname "$RECORD_PATH")"
QUALIFIED_AT=$(date -u +%Y-%m-%dT%H:%M:%SZ)
cat >"$RECORD_PATH" <<EOF
{
  "schema_version": 1,
  "status": "qualified",
  "qualified_at": "$(json_escape "$QUALIFIED_AT")",
  "host_os": "macOS",
  "host_version": "$(json_escape "$HOST_VERSION")",
  "host_arch": "$(uname -m)",
  "qemu_binary": "$(json_escape "$QEMU_BIN")",
  "qemu_version": "$(json_escape "$QEMU_VERSION")",
  "custom_release": "$(json_escape "$EXPECTED_RELEASE")",
  "upstream_qemu_commit": "$(json_escape "$EXPECTED_QEMU_COMMIT")",
  "bottle_sha256": "$(json_escape "$EXPECTED_BOTTLE_SHA256")",
  "display": "cocoa,gl=$(json_escape "$GL_MODE")",
  "accelerator": "tcg",
  "virglrenderer": "$(json_escape "$VIRGL_PATH")",
  "virglrenderer_version": "$(json_escape "$EXPECTED_VIRGL_VERSION")",
  "angle": "$(json_escape "$ANGLE_PATH")",
  "angle_gles": "$(json_escape "$ANGLE_GLES_PATH")",
  "angle_version": "$(json_escape "$EXPECTED_ANGLE_VERSION")",
  "libepoxy": "$(json_escape "$EPOXY_PATH")",
  "libepoxy_version": "$(json_escape "$EXPECTED_EPOXY_VERSION")"
}
EOF

echo "Qualified: cocoa,gl=$GL_MODE + virtio-vga-gl + TCG"
echo "Qualification record: $RECORD_PATH"

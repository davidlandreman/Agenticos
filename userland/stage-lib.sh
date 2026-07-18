# Shared manifest-driven userland build, validation, staging, and refresh.
# Required from caller: REPO_ROOT and HOST_SHARE_STAGE.

_userland_rows() {
    app_row() { printf '%s|%s|%s|%s|%s|%s|%s|%s\n' "$@"; }
    # shellcheck source=userland/apps.manifest.sh
    . "$REPO_ROOT/userland/apps.manifest.sh"
    unset -f app_row
}

_stage_atomic_copy() {
    local src=$1 dst=$2 tmp
    mkdir -p "${dst%/*}"
    tmp="${dst%/*}/.${dst##*/}.tmp.$$"
    cp "$src" "$tmp" && mv -f "$tmp" "$dst"
}

# Stage the committed global zsh configuration and pruned function library.
# The kernel imports this read-only source tree into its managed runtime /etc.
stage_zsh_config() {
    local source_dir="$REPO_ROOT/userland/zsh-config"
    local etc_dir="$HOST_SHARE_STAGE/ETC"
    local zsh_dir="$etc_dir/ZSH"
    local functions_dir="$zsh_dir/FUNCTIONS"
    local required source_file count=0

    for required in \
        zshrc \
        agnoster.zsh-theme \
        functions/promptinit \
        functions/colors \
        functions/add-zsh-hook \
        functions/is-at-least \
        functions/vcs_info
    do
        if [ ! -f "$source_dir/$required" ]; then
            echo "Missing required zsh config artifact: $source_dir/$required" >&2
            return 1
        fi
    done

    mkdir -p "$functions_dir"
    _stage_atomic_copy "$source_dir/zshrc" "$etc_dir/ZSHRC"
    _stage_atomic_copy "$source_dir/agnoster.zsh-theme" "$zsh_dir/agnoster.zsh-theme"

    local manifest_tmp="$zsh_dir/.FUNCTIONS.MANIFEST.tmp.$$"
    : > "$manifest_tmp"
    for source_file in "$source_dir"/functions/*; do
        [ -f "$source_file" ] || continue
        _stage_atomic_copy "$source_file" "$functions_dir/${source_file##*/}"
        printf '%s\n' "${source_file##*/}" >> "$manifest_tmp"
        count=$((count + 1))
    done
    if [ "$count" -eq 0 ]; then
        rm -f "$manifest_tmp"
        echo "No zsh functions found under $source_dir/functions" >&2
        return 1
    fi
    mv -f "$manifest_tmp" "$zsh_dir/FUNCTIONS.MANIFEST"
    echo "Staged runtime /etc zsh sources: zshrc, agnoster, and $count functions"
}

# Stage the reviewed Mozilla trust snapshot. The kernel, rather than userland,
# owns /etc and imports this read-only source as /etc/ssl/cert.pem at boot.
stage_ca_certificates() {
    local source="$REPO_ROOT/userland/ca-certificates/cacert.pem"
    local destination="$HOST_SHARE_STAGE/ETC/SSL/CERT.PEM"
    local expected="3ff344e30b9b1ed2971044eabb438a08f2e2245ddb5f8ab1a3ad8b63ab4eaf91"
    local actual

    [ -f "$source" ] || {
        echo "Missing committed CA bundle: $source" >&2
        return 1
    }
    actual=$(shasum -a 256 "$source" | awk '{print $1}')
    [ "$actual" = "$expected" ] || {
        echo "CA bundle digest mismatch: expected $expected, got $actual" >&2
        return 1
    }
    _stage_atomic_copy "$source" "$destination" || return 1
    echo "Staged Mozilla CA bundle: $destination ($(wc -c < "$destination" | tr -d ' ') bytes)"
}

# Publish only the public test root into test host shares. Server certificates
# and private keys stay outside the guest-visible tree and are used solely by
# the host-side guestfwd process.
stage_tls_test_root() {
    local source="$REPO_ROOT/tools/tls-fixtures/root.pem"
    local destination="$HOST_SHARE_STAGE/TLS/ROOT.PEM"
    [ -f "$source" ] || {
        echo "Missing hermetic TLS test root: $source" >&2
        return 1
    }
    _stage_atomic_copy "$source" "$destination" || return 1
    if find "$HOST_SHARE_STAGE/TLS" -type f -iname '*.key' | grep -q .; then
        echo "TLS private key leaked into guest-visible host share" >&2
        return 1
    fi
    echo "Staged hermetic TLS test root: $destination"
}

# Stage small source fixtures used by the booted GNU binutils integration
# tests. They are ordinary read-only guest inputs, not generated prebuilts.
stage_binutils_fixtures() {
    local source_dir="$REPO_ROOT/userland/apps/binutils/tests"
    local dest="$HOST_SHARE_STAGE/BINUTILS"
    [ -f "$source_dir/exit42.s" ] && [ -f "$source_dir/archive-main.s" ] && [ -f "$source_dir/symbols.c" ] || {
        echo "Missing GNU binutils test fixtures under $source_dir" >&2
        return 1
    }
    mkdir -p "$dest"
    _stage_atomic_copy "$source_dir/exit42.s" "$dest/EXIT42.S"
    _stage_atomic_copy "$source_dir/archive-main.s" "$dest/ARCHMAIN.S"
    _stage_atomic_copy "$source_dir/symbols.c" "$dest/SYMBOLS.C"
}

_readelf_command() {
    local candidate
    for candidate in "${MUSL_CC:-x86_64-linux-musl-gcc}" "${MUSL_GXX:-x86_64-linux-musl-g++}"; do
        case "$candidate" in
            *gcc) candidate="${candidate%gcc}readelf" ;;
            *g++) candidate="${candidate%g++}readelf" ;;
        esac
        if command -v "$candidate" >/dev/null 2>&1; then printf '%s\n' "$candidate"; return 0; fi
    done
    for candidate in readelf llvm-readelf; do
        if command -v "$candidate" >/dev/null 2>&1; then printf '%s\n' "$candidate"; return 0; fi
    done
    return 1
}

validate_exec_elf() {
    local binary=$1 readelf et_type
    [ -f "$binary" ] || { echo "Missing userland ELF: $binary" >&2; return 1; }
    readelf=$(_readelf_command) || {
        echo "No readelf available to validate $binary" >&2
        return 1
    }
    et_type=$($readelf -h "$binary" 2>/dev/null | awk '/Type:/ { print $2 }')
    if [ "$et_type" != EXEC ]; then
        echo "$binary is ${et_type:-unknown}, expected EXEC" >&2
        return 1
    fi
    if "$readelf" -l "$binary" 2>/dev/null | grep -q ' INTERP '; then
        echo "$binary contains PT_INTERP; expected a static executable" >&2
        return 1
    fi
    if "$readelf" -d "$binary" 2>/dev/null | grep -q '(NEEDED)'; then
        echo "$binary contains DT_NEEDED; expected no dynamic dependencies" >&2
        return 1
    fi
}

_toolchain_command() {
    case "$1" in
        rust-nightly) printf '%s\n' cargo ;;
        musl-cc) printf '%s\n' "${MUSL_CC:-x86_64-linux-musl-gcc}" ;;
        musl-cxx) printf '%s\n' "${MUSL_GXX:-x86_64-linux-musl-g++}" ;;
        *) return 1 ;;
    esac
}

_strip_command() {
    case "$1" in
        musl-cc) printf '%s\n' "${MUSL_CC:-x86_64-linux-musl-gcc}" | sed 's/gcc$/strip/' ;;
        musl-cxx) printf '%s\n' "${MUSL_GXX:-x86_64-linux-musl-g++}" | sed 's/g++$/strip/' ;;
        *) return 1 ;;
    esac
}

_refresh_prebuilt_copy() {
    local src=$1 dst=$2 toolchain=$3 strip tmp
    strip=$(_strip_command "$toolchain") || {
        echo "No strip mapping for refresh toolchain $toolchain" >&2
        return 1
    }
    command -v "$strip" >/dev/null 2>&1 || {
        echo "Required strip tool $strip not found" >&2
        return 1
    }
    mkdir -p "${dst%/*}"
    tmp="${dst%/*}/.${dst##*/}.tmp.$$"
    cp "$src" "$tmp" || return 1
    if ! "$strip" "$tmp"; then
        rm -f "$tmp"
        return 1
    fi
    mv -f "$tmp" "$dst"
}

_make_app() {
    local source=$1 toolchain=$2 command
    command=$(_toolchain_command "$toolchain") || return 1
    if ! command -v "$command" >/dev/null 2>&1; then
        echo "Optional userland toolchain $command not found; skipping $source"
        return 1
    fi
    make -C "$REPO_ROOT/userland/$source" \
        MUSL_CC="${MUSL_CC:-x86_64-linux-musl-gcc}" \
        MUSL_GXX="${MUSL_GXX:-x86_64-linux-musl-g++}"
}

_stage_one() {
    local name=$1 source=$2 build_kind=$3 staged_name=$4 ship_kind=$5 toolchain=$6 output=$7 prebuilt=$8 cargo_ok=$9
    local output_path="$REPO_ROOT/userland/$output" staged="$HOST_SHARE_STAGE/$staged_name"
    local prebuilt_path rebuild_var rebuild_value=0

    case "$ship_kind" in
        built-every-run)
            rm -f "$staged"
            if [ "$build_kind" = cargo ]; then
                [ "$cargo_ok" = 1 ] || return 1
            else
                _make_app "$source" "$toolchain" || return 1
            fi
            validate_exec_elf "$output_path" || return 1
            _stage_atomic_copy "$output_path" "$staged"
            ;;
        prebuilt-managed)
            prebuilt_path="$REPO_ROOT/userland/$prebuilt"
            rebuild_var="REBUILD_$(printf '%s' "$name" | tr '[:lower:]-' '[:upper:]_')"
            eval "rebuild_value=\${$rebuild_var:-0}"
            case "$name" in
                binutils-*)
                    if [ "${REBUILD_BINUTILS:-0}" = 1 ]; then rebuild_value=1; fi
                    ;;
            esac
            if [ "${REBUILD_USERLAND:-0}" = 1 ] || [ "$rebuild_value" = 1 ] || [ ! -f "$prebuilt_path" ]; then
                if _make_app "$source" "$toolchain" && validate_exec_elf "$output_path"; then
                    _stage_atomic_copy "$output_path" "$prebuilt_path" || return 1
                else
                    echo "Could not rebuild $name; trying committed prebuilt" >&2
                fi
            fi
            [ -f "$prebuilt_path" ] || return 1
            validate_exec_elf "$prebuilt_path" || return 1
            _stage_atomic_copy "$prebuilt_path" "$staged"
            ;;
        test-fixture)
            prebuilt_path="$REPO_ROOT/userland/$prebuilt"
            [ -f "$prebuilt_path" ] || { echo "Missing mandatory fixture: $prebuilt_path" >&2; return 1; }
            validate_exec_elf "$prebuilt_path" || return 1
            _stage_atomic_copy "$prebuilt_path" "$staged"
            ;;
    esac
    echo "Staged $staged ($(wc -c < "$staged" | tr -d ' ') bytes)"
}

# Stage the committed TCC sysroot tarball as an extracted tree under
# host_share/sysroot (guest: /host/sysroot). Prebuilt-managed like the
# ELF rows: normal builds extract the committed tarball; REBUILD_USERLAND=1
# / REBUILD_TCC=1 / a missing tarball trigger a rebuild through the tcc
# Makefile (which produces both the binary and the tarball). A content
# stamp keeps repeat stagings cheap.
stage_tcc_sysroot() {
    local tarball="$REPO_ROOT/userland/prebuilt/tcc-sysroot.tar.gz"
    local built="$REPO_ROOT/userland/apps/tcc/build/tcc-sysroot.tar.gz"
    local dest="$HOST_SHARE_STAGE/sysroot"
    local stamp="$dest/.staged.sha256" want have tmp

    if [ "${REBUILD_USERLAND:-0}" = 1 ] || [ "${REBUILD_TCC:-0}" = 1 ] || [ ! -f "$tarball" ]; then
        if _make_app apps/tcc musl-cc && [ -f "$built" ]; then
            mkdir -p "${tarball%/*}"
            cp "$built" "$tarball.tmp.$$" && mv -f "$tarball.tmp.$$" "$tarball"
        else
            echo "Could not rebuild tcc sysroot; trying committed tarball" >&2
        fi
    fi
    if [ ! -f "$tarball" ]; then
        echo "Warning: tcc sysroot tarball missing; /host/sysroot not staged" >&2
        return 1
    fi

    want=$(shasum -a 256 "$tarball" | awk '{print $1}')
    have=$(cat "$stamp" 2>/dev/null || true)
    if [ "$want" = "$have" ] && [ -d "$dest" ]; then
        return 0
    fi
    tmp="$HOST_SHARE_STAGE/.sysroot.tmp.$$"
    rm -rf "$tmp" "$dest"
    mkdir -p "$tmp"
    if ! tar xzf "$tarball" -C "$tmp"; then
        rm -rf "$tmp"
        echo "Warning: could not extract tcc sysroot tarball" >&2
        return 1
    fi
    printf '%s\n' "$want" > "$tmp/.staged.sha256"
    mv "$tmp" "$dest"
    echo "Staged $dest ($(find "$dest" -type f | wc -l | tr -d ' ') files)"
}

# stage_userland MODE SKIP_BUILT
# MODE is `build` or `test`; SKIP_BUILT=1 preserves prebuilt + fixture staging.
stage_userland() {
    local mode=$1 skip_built=${2:-0} cargo_ok=1
    if [ "$mode" = test ]; then
        stage_tls_test_root || return 1
    else
        rm -f "$HOST_SHARE_STAGE/TLS/ROOT.PEM"
        rmdir "$HOST_SHARE_STAGE/TLS" 2>/dev/null || true
    fi
    if [ "$skip_built" != 1 ]; then
        echo "Building Rust userland workspace..."
        cargo build --release --manifest-path "$REPO_ROOT/userland/Cargo.toml" || cargo_ok=0
    fi
    while IFS='|' read -r name source build_kind staged_name ship_kind toolchain output prebuilt; do
        [ "$mode" = test ] || [ "$ship_kind" != test-fixture ] || continue
        if [ "$skip_built" = 1 ] && [ "$ship_kind" = built-every-run ]; then continue; fi
        if ! _stage_one "$name" "$source" "$build_kind" "$staged_name" "$ship_kind" "$toolchain" "$output" "$prebuilt" "$cargo_ok"; then
            if [ "$ship_kind" = test-fixture ]; then return 1; fi
            echo "Warning: userland app $name was not staged" >&2
        fi
    done < <(_userland_rows)
    # The TCC sysroot rides alongside the TCC.ELF row: extracted tree, not
    # a single ELF, so it has its own staging helper. Soft-fail like other
    # non-fixture apps.
    stage_tcc_sysroot || echo "Warning: tcc sysroot was not staged" >&2
    stage_binutils_fixtures || echo "Warning: binutils fixtures were not staged" >&2
    return 0
}

refresh_manifest_prebuilts() {
    local name source build_kind staged_name ship_kind toolchain output prebuilt output_path prebuilt_path
    while IFS='|' read -r name source build_kind staged_name ship_kind toolchain output prebuilt; do
        case "$ship_kind" in prebuilt-managed|test-fixture) ;; *) continue ;; esac
        [ "$build_kind" = make ] || { echo "Refresh only supports make rows: $name" >&2; return 1; }
        _make_app "$source" "$toolchain" || return 1
        output_path="$REPO_ROOT/userland/$output"
        prebuilt_path="$REPO_ROOT/userland/$prebuilt"
        validate_exec_elf "$output_path" || return 1
        _refresh_prebuilt_copy "$output_path" "$prebuilt_path" "$toolchain" || return 1
        validate_exec_elf "$prebuilt_path" || return 1
        echo "Refreshed $prebuilt_path"
    done < <(_userland_rows)
    # The tcc row's _make_app above also produced the sysroot tarball;
    # refresh the committed copy alongside TCC.ELF. Hard-fail like the
    # ELF artifacts.
    local sysroot_built="$REPO_ROOT/userland/apps/tcc/build/tcc-sysroot.tar.gz"
    local sysroot_prebuilt="$REPO_ROOT/userland/prebuilt/tcc-sysroot.tar.gz"
    [ -f "$sysroot_built" ] || { echo "tcc sysroot tarball was not built" >&2; return 1; }
    cp "$sysroot_built" "$sysroot_prebuilt.tmp.$$" && mv -f "$sysroot_prebuilt.tmp.$$" "$sysroot_prebuilt" || return 1
    echo "Refreshed $sysroot_prebuilt"
}

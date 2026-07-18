#!/bin/bash
# QEMU compositor capability probing. Source this file, then call
# `agenticos_configure_qemu <binary>`; results are returned in
# AGENTICOS_QEMU_RENDER_ARGS and AGENTICOS_QEMU_FW_CFG_ARGS arrays.

agenticos_configure_qemu() {
    local qemu_bin=$1
    local compositor=${AGENTICOS_COMPOSITOR:-legacy}
    local strict=${AGENTICOS_GPU_STRICT:-0}
    local theme=${AGENTICOS_THEME:-auto}
    local virtio_2d=${AGENTICOS_QEMU_2D:-auto}
    local gl_mode=${AGENTICOS_QEMU_GL:-es}
    local device_help display_help host_os

    case "$compositor" in
        legacy|retained|gpu|auto) ;;
        *)
            echo "Invalid AGENTICOS_COMPOSITOR '$compositor' (expected legacy, retained, gpu, or auto)" >&2
            return 2
            ;;
    esac
    case "$strict" in 0|1) ;; *) echo "AGENTICOS_GPU_STRICT must be 0 or 1" >&2; return 2 ;; esac
    case "$theme" in classic|aero|auto) ;; *) echo "Invalid AGENTICOS_THEME '$theme' (expected classic, aero, or auto)" >&2; return 2 ;; esac
    case "$virtio_2d" in auto|on|off) ;; *) echo "AGENTICOS_QEMU_2D must be auto, on, or off" >&2; return 2 ;; esac
    case "$gl_mode" in es|core) ;; *) echo "AGENTICOS_QEMU_GL must be es or core" >&2; return 2 ;; esac

    host_os=$(uname -s)
    device_help=${AGENTICOS_QEMU_DEVICE_HELP_OVERRIDE:-$("$qemu_bin" -device help 2>&1)}
    display_help=${AGENTICOS_QEMU_DISPLAY_HELP_OVERRIDE:-$("$qemu_bin" -display cocoa,help 2>&1 || true)}

    AGENTICOS_QEMU_RENDER_ARGS=()
    AGENTICOS_QEMU_FW_CFG_ARGS=(
        -fw_cfg "name=opt/agenticos/compositor,string=$compositor"
        -fw_cfg "name=opt/agenticos/gpu_strict,string=$strict"
        -fw_cfg "name=opt/agenticos/theme,string=$theme"
    )

    local has_gl_device=0
    local has_2d_device=0
    local use_2d_device=0
    grep -q 'virtio-vga-gl' <<<"$device_help" && has_gl_device=1
    grep -q 'virtio-vga' <<<"$device_help" && has_2d_device=1
    if [[ "$virtio_2d" == on || ("$virtio_2d" == auto && "$host_os" != Darwin) ]]; then
        use_2d_device=$has_2d_device
    elif [[ "$virtio_2d" == auto && "$host_os" == Darwin && $has_2d_device -eq 1 ]]; then
        echo "Plain VirtIO 2D disabled: QEMU Cocoa can show a black scanout; presenter=boot-framebuffer (override with AGENTICOS_QEMU_2D=on)"
    fi

    # QEMU 11.0.1's macOS Cocoa frontend can leave its window solid black for
    # a valid plain VirtIO-GPU 2D scanout. QEMU's internal screendump still
    # contains the fully rendered frame, while the boot framebuffer displays
    # normally. Prefer the boot-framebuffer presenter on Darwin unless a
    # developer explicitly opts into VirtIO 2D for diagnostics.
    if [[ "$compositor" == retained && $use_2d_device -eq 1 ]]; then
        AGENTICOS_QEMU_RENDER_ARGS=(-vga none -device virtio-vga)
    fi

    if [[ "$compositor" == gpu || "$compositor" == auto ]]; then
        if [[ $has_gl_device -eq 1 ]]; then
            if [[ "$host_os" == Darwin ]]; then
                if grep -q 'gl' <<<"$display_help"; then
                    AGENTICOS_QEMU_RENDER_ARGS=(-vga none -display "cocoa,gl=$gl_mode" -device virtio-vga-gl)
                elif [[ "$compositor" == gpu && "$strict" == 1 ]]; then
                    echo "Selected QEMU does not advertise Cocoa GL mode" >&2
                    return 2
                fi
            else
                AGENTICOS_QEMU_RENDER_ARGS=(-vga none -display gtk,gl=on -device virtio-vga-gl)
            fi
        elif [[ "$compositor" == gpu && "$strict" == 1 ]]; then
            echo "Selected QEMU does not provide virtio-vga-gl; strict GPU launch refused" >&2
            return 2
        fi

        # A plain VirtIO VGA device is useful for the 2D presenter but is not
        # evidence of accelerated composition.
        if [[ ${#AGENTICOS_QEMU_RENDER_ARGS[@]} -eq 0 && $use_2d_device -eq 1 ]]; then
            AGENTICOS_QEMU_RENDER_ARGS=(-vga none -device virtio-vga)
        fi
    fi

    # On macOS, QEMU's cocoa backend blits the guest framebuffer at 1:1
    # physical pixels. On a Retina/HiDPI display the 1280x720 guest therefore
    # opens as a physically tiny window ("hard to see the OS"). zoom-to-fit
    # makes the window resizable with the guest image scaled to fill it, and
    # zoom-interpolation smooths the upscale. Opt out with AGENTICOS_QEMU_ZOOM=off;
    # start maximized to the display with AGENTICOS_QEMU_FULLSCREEN=1.
    if [[ "$host_os" == Darwin && "${AGENTICOS_QEMU_ZOOM:-on}" != off ]]; then
        _agenticos_apply_cocoa_zoom
    fi
}

# Fold zoom-to-fit (and optional full-screen) into the cocoa display args.
# If a `-display cocoa...` is already present (GL path) we extend it in place;
# otherwise (legacy path passes no -display at all) we add a cocoa default.
_agenticos_apply_cocoa_zoom() {
    local zoom_opts="zoom-to-fit=on,zoom-interpolation=on"
    [[ "${AGENTICOS_QEMU_FULLSCREEN:-0}" == 1 ]] && zoom_opts="full-screen=on,$zoom_opts"

    local i j
    for i in "${!AGENTICOS_QEMU_RENDER_ARGS[@]}"; do
        if [[ "${AGENTICOS_QEMU_RENDER_ARGS[$i]}" == "-display" ]]; then
            j=$((i + 1))
            if [[ "${AGENTICOS_QEMU_RENDER_ARGS[$j]}" == cocoa* ]]; then
                AGENTICOS_QEMU_RENDER_ARGS[$j]="${AGENTICOS_QEMU_RENDER_ARGS[$j]},$zoom_opts"
            fi
            return
        fi
    done

    AGENTICOS_QEMU_RENDER_ARGS+=(-display "cocoa,$zoom_opts")
}

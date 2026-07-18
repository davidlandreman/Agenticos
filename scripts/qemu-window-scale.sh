#!/bin/bash
# Enlarge the QEMU cocoa window to a fixed multiple of the size it opens at.
#
# QEMU 11's macOS cocoa backend has no initial-window-size / scale flag: with
# zoom-to-fit=on the window is resizable and scales its 1280x720 guest image to
# fill, but it still *opens* at the native 1:1 size (physically tiny on Retina).
# We wait for the window to appear, read the size cocoa chose, and multiply it
# by AGENTICOS_QEMU_SCALE so the guest renders at that scale from the start.
#
# Driving another app's window uses System Events, which requires the launching
# terminal to hold Accessibility permission (System Settings -> Privacy &
# Security -> Accessibility). Without it the AppleScript errors and we no-op,
# leaving the normal (unscaled) window -- so this is best-effort, never fatal.
#
# Usage: qemu-window-scale.sh <process-name> <scale>
# Intended to be launched in the background just before QEMU runs.

set -u

proc_name=${1:?process name required}
scale=${2:?scale required}

# Nothing to do for 1x (or non-numeric); avoid touching the window at all.
case "$scale" in
    ''|*[!0-9.]*) exit 0 ;;
esac
if awk "BEGIN{exit !($scale <= 1)}"; then
    exit 0
fi

osascript - "$proc_name" "$scale" >/dev/null 2>&1 <<'APPLESCRIPT'
on run argv
    set procName to item 1 of argv
    set scaleFactor to (item 2 of argv) as real
    tell application "System Events"
        -- Poll for up to ~10s (50 * 0.2s) for QEMU's window to appear.
        repeat 50 times
            if exists (process procName) then
                tell process procName
                    if (count of windows) > 0 then
                        set win to window 1
                        set {w, h} to size of win
                        set size of win to {(w * scaleFactor) as integer, (h * scaleFactor) as integer}
                        return
                    end if
                end tell
            end if
            delay 0.2
        end repeat
    end tell
end run
APPLESCRIPT

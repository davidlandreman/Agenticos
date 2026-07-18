# AgenticOS Settings

`CONTROL.ELF` is the standalone ring-3 Control Center launched by the root
Start-menu Settings row and by `/bin/control` or `/bin/settings`.

It provides a modern searchable sidebar with Home, Appearance, Desktop,
System, Network, and About pages. Appearance switches Automatic/Classic/Aero
live; Desktop chooses or restores a BMP wallpaper. Both use private syscall
5010 and persist to `/data/agenticos/settings.conf`, degrading visibly to
session-only behavior when persistent storage is unavailable.

The app owns only its client UI. The kernel remains authoritative for frame
theme metrics/effects, desktop wallpaper bytes, persistence, validation, and
theme broadcasts to other GUI processes.

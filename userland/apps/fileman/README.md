# File Manager

`FILEMAN.ELF` is AgenticOS's standalone ring-3 file manager. It replaces the
old kernel Explorer while preserving the `explorer [path]` shell command.

## Interaction

- Back, Forward, Up, Home, New Folder, and Refresh toolbar actions
- Places sidebar for Home, Root, Data, and Host
- Breadcrumb navigation, or `Ctrl+L` to enter an absolute or relative path
- Details and icon-grid views, sortable columns, and `Ctrl+F` filtering
- Click, Ctrl-click, Shift-click, keyboard selection, right-click menus, and
  true timestamp-based double-click activation
- `Ctrl+C`, `Ctrl+X`, `Ctrl+V`, `Ctrl+A`, `Ctrl+Shift+N`, `F2`, `F5`, Delete,
  Backspace, Alt+Left, and Alt+Right shortcuts

Text files open in Notepad. Executable ELF files launch as child processes and
are reaped without blocking the manager.

## Filesystem capabilities

- Overlay root: full file/folder operations with `sync()` persistence
- `/data`: persistent file create/copy/delete; its FAT backend does not expose
  directory creation or rename
- `/host` and `/bin`: read-only browsing and copying out

Folder copy is intentionally not offered yet. Delete is permanent and always
uses a confirmation dialog.

## Build

The app is a built-every-run Cargo workspace member and is staged as the
FAT-safe `/host/FILEMAN.ELF` by `userland/apps.manifest.sh`.

```sh
cargo build --release --manifest-path userland/Cargo.toml -p fileman
```

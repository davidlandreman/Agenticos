# Working with Conductor

AgenticOS is set up for [conductor.build](https://www.conductor.build) so you can run multiple branches in parallel — code review on one, planning on another, exploration on a third — without QEMU instances or build artifacts colliding.

This doc explains what's wired up, how it stays isolated, and how to extend it.

---

## What's wired up

| File | Purpose |
|---|---|
| `conductor.json` | Lifecycle config Conductor reads when opening the repo. |
| `.conductor/setup.sh` | Runs **once** when Conductor creates a workspace. Installs the pinned Rust toolchain, verifies QEMU, seeds personal Claude Code permissions, and warms the release build cache. |
| `.conductor/run.sh` | Runs every time you click **Run** in a workspace. Defaults to strict VirGL on the qualified QEMU 1.0.27 bottle with Futurism chrome and networking enabled, then delegates to `./build.sh`; explicit environment overrides still win. |
| `.conductor/archive.sh` | Runs before Conductor archives a workspace. Kills any QEMU process scoped to that workspace. |
| `.conductor/run.local.sh` | Optional, gitignored. Drop one in a workspace to override `run.sh` (e.g., add `-gdb` flags) without dirtying git. |

Conductor scripts always execute inside `$CONDUCTOR_WORKSPACE_PATH` and have access to:

| Variable | Meaning |
|---|---|
| `CONDUCTOR_WORKSPACE_NAME` | City-based unique workspace ID (e.g. `prague`). |
| `CONDUCTOR_WORKSPACE_PATH` | Absolute path to this workspace's worktree. |
| `CONDUCTOR_ROOT_PATH` | Absolute path to the main checkout. |
| `CONDUCTOR_DEFAULT_BRANCH` | The repo's default branch name. |
| `CONDUCTOR_PORT` | First port of a 10-port block reserved for this workspace (`CONDUCTOR_PORT`–`CONDUCTOR_PORT+9`). Currently unused by AgenticOS but reserved for future GDB / monitor sockets. |

---

## How parallel workspaces stay isolated

Conductor uses `git worktree` under the hood: each workspace is a separate working tree on its own branch, sharing the `.git` object store with the main checkout. The pieces that could collide and how AgenticOS handles each:

| Resource | Default risk | How it's isolated |
|---|---|---|
| Cargo `target/` directory | Two parallel `cargo build`s on a shared target dir corrupt each other. | Cargo resolves `target/` relative to the manifest dir, so each worktree gets its own automatically. `build.rs` derives every path from `CARGO_MANIFEST_DIR` (no absolute paths). |
| Bootloader disk image | `target/bootloader/bios.img` would collide. | Lives inside per-worktree `target/`, so naturally isolated. `build.sh` and `test.sh` honor an `AGENTICOS_BIOS_IMAGE` override if you want to point at a custom path. |
| QEMU process | Two QEMUs from the same workspace fight over stdio. | `runScriptMode: "nonconcurrent"` in `conductor.json` makes Conductor stop the prior QEMU before launching a new one in the same workspace. Different workspaces still run concurrently. |
| QEMU RPC socket | A machine-global `/tmp/agenticos-rpc.sock` would be unlinked by the next workspace to start. | `.conductor/run.sh` sets `AGENTICOS_RPC_SOCK` from `CONDUCTOR_WORKSPACE_NAME`; archive removes that workspace's socket. |
| QEMU clipboard socket | Parallel guests need independent COM3 host-clipboard channels. | `.conductor/run.sh` sets `AGENTICOS_CLIPBOARD_SOCK` from `CONDUCTOR_WORKSPACE_NAME`; archive removes that workspace's socket. |
| Cargo registry / git index (`~/.cargo`) | None — cargo handles concurrent reads safely. | Shared by design; second-workspace builds are fast. |
| Personal Claude Code permissions | `.claude/settings.local.json` is gitignored, so it isn't carried into a new worktree. | `.conductor/setup.sh` copies it from `$CONDUCTOR_ROOT_PATH` if present, otherwise creates an empty allowlist. Shared `.claude/settings.json` (plugin enablement + base permissions) is committed and inherited automatically. |

---

## First time using Conductor with this repo

1. Install Conductor from <https://www.conductor.build> and point it at your AgenticOS clone.
2. Conductor reads `conductor.json` and shows the three lifecycle hooks.
3. Create a workspace on any branch. Conductor creates a worktree and runs `setup.sh`.
4. Click **Run**. `run.sh` invokes `./build.sh`, which builds the kernel and launches QEMU.
5. Make changes; click **Run** again — Conductor kills the prior QEMU and relaunches.

The compound-engineering plugin is enabled in the committed `.claude/settings.json`, so slash commands like `/ce-plan`, `/ce-work`, and `/ce-code-review` are available in every workspace's agent terminal out of the box.

---

## Running multiple workspaces in parallel

The intended workflow:

```
workspace-A  (main)         → code review with /ce-code-review
workspace-B  (feat/foo)     → /ce-work executing a plan
workspace-C  (exp/bar)      → exploring a refactor with /ce-brainstorm
```

Each workspace has its own `target/`, its own `bios.img`, and (when Run is clicked) its own QEMU process. Hit Run in A and B simultaneously — both QEMUs come up and run side by side.

---

## Extending the configuration

**Adding `-gdb` or other QEMU flags for one workspace:** drop a `.conductor/run.local.sh` in that workspace. It's gitignored. `run.sh` `exec`s into it when present, so it fully replaces the default invocation.

**Switching to debug mode for a workspace:** create `.conductor/run.local.sh` with:

```sh
#!/usr/bin/env bash
exec ./build.sh -d
```

**Adding MCP servers per workspace:** Conductor reads `.mcp.json` at the repo root if present. AgenticOS does not ship one yet; add it the first time you wire up an MCP server.

**Customizing toolchain installation:** edit `.conductor/setup.sh`. The script runs `./build.sh -n` once to warm each workspace's independent Cargo and boot-image cache; expect initial workspace creation to take longer than later Run clicks.

---

## Limitations

- The reserved `CONDUCTOR_PORT` block is documented but not yet wired into QEMU. AgenticOS currently uses `-serial stdio` and `isa-debug-exit` (an x86 I/O port, not a host TCP port), so there is no real port collision today. When you add a GDB stub or telnet monitor, derive the host port from `CONDUCTOR_PORT`.
- `archive.sh` uses `pkill -f` to terminate QEMU, scoped by `$CONDUCTOR_WORKSPACE_PATH`. This is portable on macOS and Linux but assumes the QEMU command line still references the workspace path. If you change the disk image path to something outside the workspace, update `archive.sh`.
- Hypervisor capacity is host-bound. KVM / Hypervisor.framework allow many guests, but if you spin up a dozen workspaces and click Run on all of them, your laptop will not enjoy itself.

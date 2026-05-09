---
title: "feat: Onboard AgenticOS to conductor.build with worktree-safe builds and first-class CE plugin"
type: feat
status: active
created: 2026-05-08
plan_id: 2026-05-08-001
depth: standard
---

# feat: Onboard AgenticOS to conductor.build with worktree-safe builds and first-class CE plugin

## Summary

Set up AgenticOS so it can be opened in [conductor.build](https://www.conductor.build) and run productively across many parallel git worktrees. The work has three parts that must all land for the experience to be usable:

1. **Make the build worktree-portable.** Today `build.rs` hardcodes `/Users/david/Projects/agenticos/...` and the cargo runner / `build.sh` / `test.sh` all read from a single shared `target/bootloader/bios.img`. Two workspaces compiled in parallel will corrupt each other's images.
2. **Author Conductor configuration.** A committed `conductor.json` plus `.conductor/setup.sh`, `.conductor/run.sh`, and `.conductor/archive.sh` that use Conductor's `$CONDUCTOR_WORKSPACE_PATH` / `$CONDUCTOR_WORKSPACE_NAME` / `$CONDUCTOR_PORT` env vars to keep workspaces isolated.
3. **Make the compound-engineering plugin a first-class citizen.** The plugin is already enabled locally in `.claude/settings.json`, but `.claude/` is gitignored and untracked, so a fresh Conductor workspace would start without it. Commit the shared plugin enablement, keep personal permissions out of git, scaffold the CE doc directories (`docs/plans/`, `docs/brainstorms/`, `docs/solutions/`), and pre-allow Conductor's bash entry points so agents in a fresh workspace can run them without per-prompt approval.

The success bar is: I can open two Conductor workspaces on different branches, hit "Run" in each, and have two QEMU instances boot AgenticOS at the same time without colliding on disk image, target dir, or port — and the `/ce-plan`, `/ce-work`, `/ce-code-review` slash commands are available the moment a workspace opens.

---

## Problem Frame

AgenticOS today is built for a single working copy: `cargo build` writes to `./target/`, `build.rs` writes images into a hardcoded absolute path, and QEMU is launched against a single fixed file. The compound-engineering plugin enablement and cargo build configuration both live in directories the repo currently gitignores, which means a freshly created worktree (Conductor's primary mechanism) lacks both. The user wants to use Conductor to drive multiple branches in parallel — code review on one, planning on another, exploration on a third — with the CE plugin available as a first-class tool in every one of them.

The job is not to invent new OS features. It is to make the existing build, test, and AI-tooling stack survive being instantiated N times in parallel under Conductor's worktree model.

---

## Confirmed Scope

- `conductor.json` at repo root, committed, with `setup` / `run` / `archive` scripts and `runScriptMode: "nonconcurrent"`.
- `.conductor/setup.sh`, `.conductor/run.sh`, `.conductor/archive.sh` committed and executable.
- Build infrastructure changes so that `cargo build`, `./build.sh`, and `./test.sh` all work correctly inside a Conductor worktree without referencing absolute paths or sharing `target/` with another workspace.
- Compound-engineering plugin available immediately in every workspace: shared `.claude/settings.json` (with plugin enabled) committed; CE doc scaffolding (`docs/plans/`, `docs/brainstorms/`, `docs/solutions/`) created; minimum permissions pre-allowed so agents do not get blocked in fresh workspaces.
- README / `CLAUDE.md` updated with a short "Working with Conductor" section so a teammate can onboard.

## Inferred Bets

These are choices I made without explicit user confirmation. Call any of them out if they do not match intent:

- **Per-workspace `target/` directory** (rather than a shared root-level cache) for build isolation. Trades disk space for clean isolation; `cargo` registry/git caches in `~/.cargo/` remain shared and cached. Rationale: kernel target dir contains custom-target artifacts that are cheap to cache shared but risky to write concurrently — per-workspace is the safer default.
- **`runScriptMode: "nonconcurrent"`** so clicking Run in a workspace kills the previous QEMU in that same workspace before relaunching. Different workspaces still run concurrently; this only stops a single workspace from leaking QEMU processes.
- **Commit `.claude/settings.json`** (which only contains `enabledPlugins`) but keep `.claude/settings.local.json` gitignored. Personal permission lists stay personal; shared plugin enablement is shared.
- **Keep QEMU's serial in `stdio` for now** rather than rewiring to a `CONDUCTOR_PORT`-derived TCP socket. The current QEMU invocation does not bind any host TCP ports, so port collisions are not a real risk yet. We reserve `$CONDUCTOR_PORT` documentation for the future when a GDB stub or telnet monitor is added.
- **Treat `.cargo/config.toml` as required source.** Remove the `.cargo` entry from `.gitignore` and commit `.cargo/config.toml`. Without it, a fresh worktree's build target spec is lost.

## Deferred to Follow-Up Work

- **Shared sccache or cargo cache layer** across worktrees. Useful but not required for first-cut isolation.
- **GDB-stub integration** with `-gdb tcp:127.0.0.1:$((CONDUCTOR_PORT+1))`. We will document how to enable it but not wire it in this plan.
- **CI integration of the conductor scripts.** The scripts are designed for local Conductor; a separate plan can adapt them for `.github/workflows/` if needed.
- **MCP servers via `.mcp.json`.** Conductor reads `.mcp.json` if present; we will not add one until there is a specific MCP we want to ship.

## Out of Scope

- Anything kernel-side: no new OS features, no driver changes, no behavior changes inside `src/`.
- Replacing `build.sh` / `test.sh` with a different build tool (e.g., `just`, `make`).
- Rewriting the bootloader image generation pipeline (we adjust paths only).

---

## Requirements Traceability

This plan was authored solo; there is no upstream `*-requirements.md`. Requirements derive from the user's prompt and the conductor.build research digest.

- **R1.** Two parallel Conductor workspaces of AgenticOS can each run `./build.sh` to completion without corrupting each other's outputs.
- **R2.** Two parallel Conductor workspaces can each run `./test.sh` and observe pass/fail independently.
- **R3.** A freshly created Conductor workspace has the compound-engineering plugin enabled and `/ce-plan`, `/ce-work`, `/ce-code-review` available out of the box.
- **R4.** Clicking Run in Conductor on a workspace boots AgenticOS in QEMU using only that workspace's artifacts.
- **R5.** Archiving a workspace cleanly stops any running QEMU spawned by it.
- **R6.** The setup runs unattended (no manual approval prompts inside the workspace's first `setup` invocation).

---

## Output Structure

```text
agenticos/
├── conductor.json                 # NEW — committed, repo-root config
├── .conductor/
│   ├── setup.sh                   # NEW — runs once at workspace creation
│   ├── run.sh                     # NEW — runs on Conductor "Run" click
│   └── archive.sh                 # NEW — runs before workspace teardown
├── .cargo/
│   └── config.toml                # MODIFIED + now tracked (removed from .gitignore)
├── .claude/
│   ├── settings.json              # MODIFIED + now tracked (plugin enablement only)
│   └── settings.local.json        # remains gitignored — personal
├── .gitignore                     # MODIFIED — drop `.cargo` and `.claude` lines
├── build.rs                       # MODIFIED — kill absolute paths, derive from CARGO_TARGET_DIR
├── build.sh                       # MODIFIED — read image path from env var
├── test.sh                        # MODIFIED — read image path from env var
├── docs/
│   ├── plans/                     # NEW directory (this plan lives here)
│   ├── brainstorms/               # NEW empty directory + .gitkeep
│   ├── solutions/                 # NEW empty directory + .gitkeep
│   └── conductor-workflow.md      # NEW — short onboarding doc
├── CLAUDE.md                      # MODIFIED — add "Working with Conductor" section
└── README.md                      # MODIFIED — link to docs/conductor-workflow.md
```

---

## High-Level Technical Design

*This section illustrates the intended approach and is directional guidance for review, not implementation specification.*

### Workspace lifecycle under Conductor

```mermaid
sequenceDiagram
    participant U as User in Conductor UI
    participant C as Conductor app
    participant W as Worktree (.../boston-v1)
    participant Q as QEMU process

    U->>C: "New Workspace" on branch feat/foo
    C->>W: git worktree add (tracked files only)
    C->>W: setup.sh<br/>(materialize .cargo, ensure toolchain,<br/>print env summary)
    Note over W: workspace ready; agent has /ce-* commands
    U->>C: click Run
    C->>W: run.sh (CONDUCTOR_PORT, _NAME, _PATH set)
    W->>Q: qemu-system-x86_64 -drive file=$WORKSPACE/target/bootloader/bios.img
    U->>C: click Run again<br/>(or new code)
    C->>Q: SIGTERM (runScriptMode=nonconcurrent)
    C->>W: run.sh (relaunched)
    U->>C: Archive workspace
    C->>W: archive.sh (kill stragglers, leave artifacts)
    C->>W: git worktree remove
```

### Path resolution (the central change)

| Surface | Today | After this plan |
|---|---|---|
| `build.rs` target dir | `PathBuf::from("/Users/david/Projects/agenticos/target")` | `PathBuf::from(env::var("CARGO_TARGET_DIR").unwrap_or("target"))` resolved relative to `CARGO_MANIFEST_DIR` |
| `build.rs` assets dir | `PathBuf::from("/Users/david/Projects/agenticos/assets")` | `PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets")` |
| Cargo target dir | implicit `./target` | `./target` per-worktree (no override needed; cargo resolves from manifest dir) |
| QEMU image path in `build.sh` / `test.sh` | literal `target/bootloader/bios.img` | `${AGENTICOS_BIOS_IMAGE:-target/bootloader/bios.img}` so Conductor `run.sh` can override |

### Compound-engineering enablement matrix

| Concern | Mechanism | Tracked in git? |
|---|---|---|
| Plugin enabled (`compound-engineering@compound-engineering-plugin: true`) | `.claude/settings.json` | Yes (after this plan) |
| Personal permission allow-list | `.claude/settings.local.json` | No (stays gitignored) |
| Shared agent permissions needed in every workspace | additions to committed `.claude/settings.json` `permissions.allow` | Yes |
| CE doc directories (`docs/plans/`, `docs/brainstorms/`, `docs/solutions/`) | `.gitkeep` files | Yes |
| MCP servers | `.mcp.json` (deferred) | n/a |

---

## Implementation Units

### U1. Make build.rs and the build/test scripts worktree-portable

**Goal:** Eliminate hardcoded absolute paths and parameterize the disk-image location so two worktrees never read or write the same file.

**Requirements:** R1, R2, R4

**Dependencies:** none

**Files:**
- `build.rs` (modify)
- `build.sh` (modify)
- `test.sh` (modify)
- `.cargo/config.toml` (modify; the `runner` line)

**Approach:**
- In `build.rs`, replace the two `PathBuf::from("/Users/david/Projects/agenticos/...")` literals with paths resolved from `CARGO_MANIFEST_DIR` (always set by cargo) joined with `target` and `assets` respectively. Honor `CARGO_TARGET_DIR` if cargo set it. The `cargo:rerun-if-changed=` directive on line 8 should also become manifest-relative.
- In `build.sh` and `test.sh`, replace the literal `target/bootloader/bios.img` with `"${AGENTICOS_BIOS_IMAGE:-target/bootloader/bios.img}"` and emit the resolved value before invoking QEMU so logs are obvious.
- In `.cargo/config.toml`, leave the runner pointing at `target/bootloader/bios.img` (cargo resolves relative to the manifest dir, which is per-worktree). No env-var indirection needed in cargo runner.
- Do not change the kernel build's two-pass behavior. That remains.

**Patterns to follow:** none in-repo — this is the only build script.

**Test scenarios:**
- From a fresh clone, `./build.sh -n` produces `target/bootloader/bios.img` and `target/bootloader/uefi.img` with no warning about a missing kernel binary.
- From a second checkout (simulated worktree) at a different filesystem path, `./build.sh -n` succeeds independently and writes to its own `target/bootloader/`. Confirm the two `bios.img` files have different mtimes after running concurrently.
- Setting `AGENTICOS_BIOS_IMAGE=/tmp/explicit.img` and running `./build.sh` (with that file pre-built) launches QEMU against the explicit path.
- `./test.sh` exits with code 0 (tests passed) using only relative paths.
- `cargo run` (which invokes the `.cargo/config.toml` runner) launches QEMU successfully.

**Verification:** Two checkouts at different paths can each run `./build.sh -n` in parallel and produce non-overlapping `target/bootloader/bios.img` files.

---

### U2. Author conductor.json at repo root

**Goal:** Declare the Conductor lifecycle so the app knows how to set up, run, and tear down workspaces.

**Requirements:** R1, R4, R5, R6

**Dependencies:** U1 (run script needs the new env-var-aware build.sh), U3, U4, U5 (the scripts referenced from `conductor.json`)

**Files:**
- `conductor.json` (new)

**Approach:**
- Minimal shape per the Conductor docs:
  ```json
  {
    "scripts": {
      "setup": ".conductor/setup.sh",
      "run": ".conductor/run.sh",
      "archive": ".conductor/archive.sh"
    },
    "runScriptMode": "nonconcurrent"
  }
  ```
- Do not add `enterpriseDataPrivacy` (defaults are fine).
- Do not add an MCP block — Conductor reads `.mcp.json` separately (deferred).

**Test scenarios:**
- `jq . conductor.json` parses without error.
- The three referenced script paths exist and are executable.

**Verification:** Opening the repo in Conductor surfaces the three scripts as detected lifecycle hooks (visible in the Conductor UI).

---

### U3. Write .conductor/setup.sh

**Goal:** One-time per-workspace bootstrap. Materialize gitignored-but-required files, ensure the rust toolchain components are present, and print a short summary so the agent has obvious context.

**Requirements:** R1, R3, R6

**Dependencies:** U6 (committing `.cargo/config.toml` and `.claude/settings.json` removes the need for setup.sh to fabricate them, but setup.sh still handles toolchain + sanity)

**Files:**
- `.conductor/setup.sh` (new, executable)

**Approach:**
- Set `set -euo pipefail`.
- Echo `$CONDUCTOR_WORKSPACE_NAME`, `$CONDUCTOR_WORKSPACE_PATH`, `$CONDUCTOR_PORT` so the agent's first terminal output makes the isolation visible.
- Run `rustup show` to force the toolchain pinned by `rust-toolchain.toml` to install (`rustup` is idempotent and safe across parallel workspaces for component installs of the same versions).
- Verify `qemu-system-x86_64` is on `$PATH`; print a clear message and exit non-zero if missing.
- If `.claude/settings.local.json` does not exist (it is gitignored), copy a starter template from `$CONDUCTOR_ROOT_PATH/.claude/settings.local.json` if available, otherwise create a minimal one with `{ "permissions": { "allow": [], "deny": [] } }` — so personal permissions in the main checkout transfer to new workspaces by default.
- Do *not* run `cargo build` here — that should remain the user's first action. Setup must stay fast.

**Patterns to follow:** the conductor.build Phoenix and Laravel example scripts (cited in research), kept Rust-flavored.

**Test scenarios:**
- `bash -n .conductor/setup.sh` parses cleanly.
- Running `.conductor/setup.sh` from the main checkout (with all CONDUCTOR_* vars set to dummy values) succeeds, prints the summary, and creates `.claude/settings.local.json` if it was absent.
- Running with `qemu-system-x86_64` removed from PATH fails with a clear message and non-zero exit.
- Running with `rust-toolchain.toml` channel set to a non-installed version installs that channel (this confirms `rustup show` is doing its job).

**Verification:** Creating a new Conductor workspace shows a green setup status; the workspace's first terminal session has `cargo`, `rustup`, and `qemu-system-x86_64` on the path.

---

### U4. Write .conductor/run.sh

**Goal:** Build and launch QEMU for the current workspace, using only this workspace's artifacts.

**Requirements:** R1, R4

**Dependencies:** U1 (the env-var-aware build.sh)

**Files:**
- `.conductor/run.sh` (new, executable)

**Approach:**
- `set -euo pipefail` and `cd "$CONDUCTOR_WORKSPACE_PATH"`.
- Echo `$CONDUCTOR_WORKSPACE_NAME`, the resolved image path, and reserved port block (`$CONDUCTOR_PORT`–`$((CONDUCTOR_PORT+9))`) before invoking the build, for log clarity.
- Invoke `./build.sh` (which already runs the two-pass cargo build and then QEMU).
- Do not pass `-gdb` or any TCP ports yet — current `build.sh` uses `-serial stdio` and `isa-debug-exit`, neither of which contend across workspaces. The reserved port block is documented for future use only.
- Honor a one-line override hook: if `.conductor/run.local.sh` exists, exec it instead. This lets a workspace experiment with QEMU flags without dirtying git.

**Test scenarios:**
- `bash -n .conductor/run.sh` parses cleanly.
- Running it with all `CONDUCTOR_*` env vars set launches QEMU and AgenticOS reaches the desktop.
- Two workspaces clicking Run within seconds of each other both reach the desktop with no "image busy" or build-lock errors.
- Creating an empty `.conductor/run.local.sh` causes the override branch to fire (use `set -x` proof).

**Verification:** Clicking Run in the Conductor UI launches QEMU in a window/terminal Conductor renders.

---

### U5. Write .conductor/archive.sh

**Goal:** Clean teardown — kill any QEMU process this workspace spawned, leave persistent artifacts (Conductor itself removes the worktree).

**Requirements:** R5

**Dependencies:** U4 (mirror image)

**Files:**
- `.conductor/archive.sh` (new, executable)

**Approach:**
- `set -uo pipefail` (do *not* `-e`; archive must be best-effort).
- `pkill -f "qemu-system-x86_64.*$CONDUCTOR_WORKSPACE_PATH"` — match QEMU processes whose disk-image path lives inside this workspace. This avoids killing other workspaces' QEMUs.
- Do not delete `target/`. Conductor removes the worktree directory; rebuilds elsewhere are unaffected.
- Echo a one-line "archived: $CONDUCTOR_WORKSPACE_NAME" for the Conductor log.

**Test scenarios:**
- `bash -n .conductor/archive.sh` parses cleanly.
- With a fake long-running `qemu-system-x86_64` started against a path matching `$CONDUCTOR_WORKSPACE_PATH`, the script terminates it.
- A QEMU started against a different path is *not* killed by the script.
- Running with no QEMU processes alive exits cleanly with status 0.

**Verification:** Archiving a workspace from Conductor's UI returns no warnings; `pgrep -f qemu-system-x86_64.*<that-workspace>` returns nothing afterwards.

---

### U6. Make compound-engineering a first-class citizen of every workspace

**Goal:** Any freshly created Conductor workspace has the CE plugin loaded, the doc directories present, and the bash entry points pre-allowed for the agent.

**Requirements:** R3, R6

**Dependencies:** none (independent of U1–U5; can land in parallel)

**Files:**
- `.gitignore` (modify — drop `.cargo` and `.claude` lines)
- `.claude/settings.json` (modify — keep plugin enablement; add minimum shared `permissions.allow` covering Conductor's bash invocations)
- `.claude/settings.local.json` (leave gitignored; explicit `.gitignore` entry)
- `.cargo/config.toml` (now tracked; no content change — handled in U1)
- `docs/plans/.gitkeep` (new)
- `docs/brainstorms/.gitkeep` (new)
- `docs/solutions/.gitkeep` (new)

**Approach:**
- Edit `.gitignore`: remove `.cargo` and `.claude` lines. Add explicit `.claude/settings.local.json` entry (so personal permissions stay personal).
- Edit `.claude/settings.json` to keep `enabledPlugins: { "compound-engineering@compound-engineering-plugin": true }` and add a `permissions.allow` array containing the minimum set every workspace agent needs. Suggested shared allow-list, derived from current `settings.local.json` minus the personal/temporary entries:
  - `Bash(cargo:*)`
  - `Bash(rustc:*)`
  - `Bash(rustup component add:*)`
  - `Bash(./build.sh:*)`
  - `Bash(./test.sh:*)`
  - `Bash(./.conductor/setup.sh:*)`
  - `Bash(./.conductor/run.sh:*)`
  - `Bash(./.conductor/archive.sh:*)`
  - `Bash(ls:*)`, `Bash(find:*)`, `Bash(wc:*)`, `Bash(mkdir:*)`, `Bash(mv:*)`
- Stage `.cargo/config.toml` so it ships with every checkout/worktree.
- Create empty `.gitkeep` files in the three CE doc directories.
- Confirm `compound-engineering` plugin is loaded by checking `/ce-plan` shows up in the slash-command list inside a fresh workspace.

**Patterns to follow:** existing `.claude/settings.local.json` permission shape.

**Test scenarios:**
- After committing, `git ls-files .claude/settings.json .cargo/config.toml` returns both paths.
- `git ls-files .claude/settings.local.json` returns nothing.
- A fresh `git worktree add ../agenticos-test feat/test` produces a worktree that contains `.claude/settings.json`, `.cargo/config.toml`, and the three `docs/*/.gitkeep` files.
- Inside that worktree, `claude` (or whichever entry point Conductor uses) sees the compound-engineering plugin as enabled — slash commands `/ce-plan`, `/ce-work`, `/ce-code-review` are listed.
- The agent in a fresh workspace can invoke `./build.sh -n` without a permission prompt.

**Verification:** A new Conductor workspace boots with `/ce-plan` available; the agent runs `./build.sh -n` without an approval modal.

---

### U7. Document the conductor workflow

**Goal:** A teammate (or future me) opening this repo can onboard to Conductor in under five minutes.

**Requirements:** R6 (indirectly — discoverability)

**Dependencies:** U1–U6 (the doc describes the system they create)

**Files:**
- `docs/conductor-workflow.md` (new)
- `README.md` (modify — add a "Parallel development with Conductor" section linking to the doc)
- `CLAUDE.md` (modify — short subsection under "Common Commands" pointing at `docs/conductor-workflow.md` so the agent surfaces it when asked)

**Approach:**
- The new doc explains: what Conductor is, what `conductor.json` declares, what each `.conductor/*.sh` does, the env vars Conductor injects, how parallel workspaces stay isolated (per-worktree `target/`, per-workspace QEMU process), how to enable a per-workspace override via `.conductor/run.local.sh`, and how the compound-engineering plugin is wired in.
- README addition is short — link, two sentences.
- CLAUDE.md addition is one paragraph under Common Commands with a pointer.

**Test scenarios:**
- `Test expectation: none -- pure documentation; verified by reading.`

**Verification:** A reader unfamiliar with Conductor can take the repo and create a working second workspace using only the docs.

---

## Key Technical Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Build isolation strategy | Per-worktree `./target/` (no `CARGO_TARGET_DIR` override) | Workspaces are already filesystem-isolated; cargo resolves `target/` relative to manifest dir, so each worktree gets its own automatically. Simpler than a shared root-level cache. |
| Cargo registry/git cache | Shared via `~/.cargo/` (default) | rustup/cargo handle concurrent reads safely; rebuilds are dramatically faster on workspace #2. |
| `runScriptMode` | `"nonconcurrent"` | Prevents a single workspace from leaking multiple QEMU processes; different workspaces still run concurrently. |
| Tracking `.cargo/config.toml` | Yes — drop from `.gitignore` | The file contains build-target spec required for the kernel to compile; gitignoring it is a latent bug, not a feature. |
| Tracking `.claude/settings.json` | Yes — drop from `.gitignore` | Plugin enablement is shared team knowledge; personal perms stay in `settings.local.json`. |
| QEMU port wiring | Keep `-serial stdio` for now | Current QEMU invocation has no host TCP binding; no real port collision exists yet. `$CONDUCTOR_PORT` documented for future GDB/monitor use. |
| Disk image location | Per-workspace `target/bootloader/bios.img` (already isolated by per-worktree `target/`) | Falls out for free once U1 lands. |
| Setup script side effects | Toolchain check + permissions starter only; no `cargo build` | Setup must be fast; build is the user's first explicit action. |

---

## System-Wide Impact

- **Build pipeline.** `build.rs` shifts from absolute paths to manifest-relative; this affects every developer, not just Conductor users. Worth a pass under `cargo build` on the main checkout to confirm no regression.
- **Permissions UX.** Moving the minimum allow-list from personal `settings.local.json` to shared `settings.json` reduces approval friction for everyone, including non-Conductor users.
- **Repo onboarding.** New contributors who clone (with or without Conductor) now get `.cargo/config.toml` and `.claude/settings.json` for free; the README's "first build" instructions get simpler.
- **Existing main checkout.** After `.cargo/` and `.claude/` are removed from `.gitignore` and tracked, existing local files in the main checkout are preserved (git tracks the working-tree contents on the first add). No data loss.

---

## Risks and Mitigations

| Risk | Likelihood | Mitigation |
|---|---|---|
| `build.rs` change breaks the existing main-checkout build | Medium | U1's first verification step is `./build.sh -n` from the main checkout. |
| Shared `~/.cargo/registry` corruption under high concurrency | Low | Cargo handles registry locking correctly across processes. Documented as a future shared-cache concern. |
| Conductor injects env vars we miss | Low | Setup script echoes all `CONDUCTOR_*` vars on first run; surfacing surprises is cheap. |
| Committing `.claude/settings.json` exposes data | Low | The current file contains only `enabledPlugins`; explicit review before commit; personal permissions remain in gitignored `settings.local.json`. |
| Two QEMUs collide on a machine-global resource (KVM, hypervisor framework) | Low on macOS (Hypervisor.framework is multi-instance); medium on Linux/KVM | `runScriptMode: nonconcurrent` keeps at most one QEMU per workspace. Documented as a known limitation if a user runs >N workspaces simultaneously where N exceeds host VM capacity. |
| Plugin not actually present after worktree creation because Conductor skips dotdir copy | Low | `.claude/` is gitignored only by line we are removing; once tracked, git copies it into every worktree by definition. |

---

## Verification Strategy

After all units land, run this end-to-end exercise:

1. From the main checkout, `./build.sh -n` produces images. Existing flow preserved.
2. From the main checkout, `./test.sh` exits 0.
3. Open the repo in Conductor. Create workspace A on `main` and workspace B on a throwaway branch.
4. Confirm both workspaces' first terminal lists `/ce-plan` as a slash command.
5. Click Run in workspace A; confirm QEMU launches and AgenticOS reaches the desktop.
6. While A's QEMU is running, click Run in workspace B; confirm a *second* QEMU launches successfully — both run side by side.
7. Click Run in workspace A again; confirm A's prior QEMU is killed and a fresh one launches (`runScriptMode: nonconcurrent`); workspace B's QEMU is unaffected.
8. Archive workspace B; confirm only B's QEMU dies; A continues.
9. Have an agent in workspace A invoke `./build.sh -n` — it runs without an approval modal.

If all nine pass, R1–R6 are satisfied.

---

## Open Questions / Deferred to Implementation

- Exact wording of the README and CLAUDE.md additions — write during U7, not now.
- Whether to add a `--release` / `--debug` toggle to `.conductor/run.sh` via an env var — implementer's call; trivial to add.
- Whether `pkill -f` matching `$CONDUCTOR_WORKSPACE_PATH` is portable to Linux Conductor (macOS is the current shipping platform). Confirm at implementation time; fall back to a pidfile if needed.
- Whether `.conductor/run.local.sh` should be auto-gitignored. Recommend yes; confirm during U4 implementation.

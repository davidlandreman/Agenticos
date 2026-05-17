---
title: "feat: Commit prebuilt userland ELFs so fresh clones boot without the musl toolchain"
type: feat
status: active
date: 2026-05-16
---

# feat: Commit prebuilt userland ELFs so fresh clones boot without the musl toolchain

## Summary

Check the static-musl `ZSH.ELF` (and future Linux-port ELFs) into a new tracked `userland/prebuilt/` directory. Teach `build.sh` and `test.sh` to copy the committed binary into `host_share/` by default and only invoke the upstream build (which fetches tarballs and depends on the musl cross-toolchain) when the prebuilt is missing or a rebuild is explicitly requested via `--rebuild-userland` / `REBUILD_USERLAND=1`. A fresh clone with no toolchain installed and no network access can `./build.sh` and reach a working zsh prompt. `HELLO.ELF` (Rust, 8 KB, fast, no exotic toolchain) and `HELLOCPP.ELF` (small C++ wrapper, requires only the musl toolchain — no upstream source fetch) continue to build every run.

---

## Problem Frame

The current `build.sh` / `test.sh` flow assumes every developer can rebuild every userland app from source on every invocation. For `ZSH.ELF` that means:

- The `x86_64-linux-musl-cross` toolchain installed on the host.
- `userland/apps/zsh/Makefile` fetching zsh + ncurses tarballs from upstream on first build (cached under `userland/apps/zsh/build/tarballs/` after that).
- Several minutes of compile time.

For a teammate or CI job that only wants to boot the OS and verify `zsh` works, those preconditions are friction with no payoff. We already ship a binary blob to QEMU at runtime — committing the exact same blob into the repo costs ~1.5 MB and erases the toolchain dependency for the common path.

The shape of the problem will grow: the zsh port is the first of several planned "Linux executable" ports (bash, vim, coreutils-style apps). Each new port re-introduces the same toolchain + source-fetch tax. A prebuilt convention put in place now keeps that tax bounded to "developers who actually iterate on a userland app."

`HELLO.ELF` and `HELLOCPP.ELF` are explicitly out of scope: neither downloads upstream sources, both build quickly, and the Rust hello uses only the standard Rust toolchain that every kernel developer already has. Including them in the prebuilt mechanism would commit binaries that drift faster than they save anyone time.

---

## Requirements

- R1. A new tracked directory `userland/prebuilt/` holds canonical committed ELFs. The first entry is `ZSH.ELF`. Future Linux-port apps that pull upstream source SHOULD be added here as they land.
- R2. `build.sh` and `test.sh` default behavior for each prebuilt-managed app: if `userland/prebuilt/<NAME>.ELF` exists and no rebuild flag is set, copy it into `host_share/<NAME>.ELF` and skip the source build entirely. Do not probe for the musl toolchain. Do not invoke `make`. Do not emit "missing musl-gcc" noise.
- R3. Rebuild is triggered by any of: `--rebuild-userland` CLI flag, `REBUILD_USERLAND=1` env var, per-app `REBUILD_ZSH=1` env var, or `userland/prebuilt/<NAME>.ELF` not existing.
- R4. When a rebuild runs, on success it MUST refresh both `userland/prebuilt/<NAME>.ELF` AND `host_share/<NAME>.ELF` atomically (write-to-tmp, rename). On rebuild failure, fall back to the existing prebuilt if present and warn; if no prebuilt exists, warn and continue (kernel tests still pass via embedded fixtures).
- R5. A helper script `userland/refresh-prebuilt.sh` forces a rebuild of every prebuilt-managed app and updates `userland/prebuilt/`. This is the canonical pre-commit workflow when a developer changes upstream source, patches, or build flags for one of the prebuilt apps.
- R6. `HELLO.ELF` (Rust) and `HELLOCPP.ELF` (C++ wrapper) continue to build from source on every `build.sh` / `test.sh` invocation — no behavior change for those two. The probe-and-warn pattern around `MUSL_GXX` for HELLOCPP stays as-is.
- R7. Documentation updated in three places: `CLAUDE.md` (Build and Run section), `userland/README.md` (new "Prebuilt ELFs" section explaining the contract), and `userland/prebuilt/README.md` (operational guide — what's here, when to refresh, who's responsible). The guidance MUST clearly state: prebuilt ELFs are committed binaries that lag source until a developer runs `userland/refresh-prebuilt.sh` and commits the result alongside any source-side change.
- R8. `.gitignore` change limited to whatever is needed to ensure `userland/prebuilt/` is tracked. No changes to the existing `host_share/*` rules — staging stays gitignored.

---

## Non-goals

- **No automatic staleness detection.** The user explicitly opted out of a CI guard, SHA256 manifest, or timestamp comparison. Developers are trusted to run `userland/refresh-prebuilt.sh` when they touch a prebuilt app's source. This is documented as the convention, not enforced by tooling. (If drift becomes a real pain point we revisit; the cost of the lighter manifest option is preserved as a known-easy follow-up.)
- **No prebuilt entry for HELLO.ELF or HELLOCPP.ELF.** Per R6, these stay source-built.
- **No Git LFS.** ~1.5 MB for zsh is well below the threshold where LFS pays for itself. Revisit if total `userland/prebuilt/` size exceeds ~50 MB.
- **No change to the kernel-side loader, FAT layer, or staging path.** This is purely a host-side build-script and repo-layout change.

---

## Approach

### Directory layout

```
userland/
  prebuilt/
    README.md          # operational guide
    ZSH.ELF            # static-musl zsh, ET_EXEC, x86_64
    # future: BASH.ELF, VIM.ELF, ...
  apps/
    hello/             # Rust, source-built every run
    hello-cpp/         # C++, source-built every run
    zsh/               # source-built only on --rebuild-userland or missing prebuilt
```

`host_share/` keeps its current role: a staging directory that QEMU mounts. The build scripts always copy into `host_share/`; the question is only whether the source for that copy is `userland/prebuilt/` or a fresh build under `userland/apps/<name>/build/`.

### Build-script flow (per prebuilt-managed app)

Pseudocode used by both `build.sh` and `test.sh`:

```
APP=zsh
SRC_BUILD_OUTPUT=userland/apps/zsh/build/zsh
PREBUILT=userland/prebuilt/ZSH.ELF
STAGED=host_share/ZSH.ELF

want_rebuild = $REBUILD_USERLAND || $REBUILD_ZSH \
            || flag --rebuild-userland was passed \
            || ! -f $PREBUILT

if want_rebuild:
    if musl toolchain available:
        if make -C userland/apps/zsh succeeds AND readelf says ET_EXEC:
            atomic-replace $PREBUILT with $SRC_BUILD_OUTPUT
            atomic-replace $STAGED   with $SRC_BUILD_OUTPUT
            echo "Rebuilt + refreshed prebuilt + staged ZSH.ELF"
            return
        else:
            echo "Rebuild failed."
            # fall through to prebuilt fallback
    else:
        echo "musl toolchain not found; cannot rebuild ZSH."
        # fall through to prebuilt fallback

if -f $PREBUILT:
    atomic-replace $STAGED with $PREBUILT
    echo "Staged ZSH.ELF from userland/prebuilt/ (no rebuild)"
else:
    echo "WARNING: ZSH.ELF unavailable — no prebuilt and no rebuild."
    # kernel tests with embedded fixtures still pass; interactive zsh won't run
```

Key invariants:
- Atomic replace means `cp` to `<dir>/.<NAME>.tmp.$$` then `mv -f` — same pattern the current scripts already use for `host_share/` staging.
- Prebuilt refresh and host_share staging happen in the same successful-build branch so they can't diverge.
- The "missing musl toolchain" message only prints when a rebuild was actually requested. Default fresh-clone runs are silent on toolchain.

### CLI / env surface

Added to both `build.sh` and `test.sh`:

| Trigger | Effect |
|---|---|
| `--rebuild-userland` flag | Force rebuild of all prebuilt-managed apps |
| `REBUILD_USERLAND=1` env | Same, env form (useful for CI / Conductor `run.local.sh`) |
| `REBUILD_ZSH=1` env | Force rebuild of just zsh (useful while iterating on the zsh build) |
| `userland/prebuilt/ZSH.ELF` missing | Auto-rebuild (the fresh-clone-with-toolchain-available case) |

`--rebuild-userland` is independent of `--skip-userland` on `test.sh`. If both are passed, `--skip-userland` wins (matches the existing "skip everything userland" semantic).

### `userland/refresh-prebuilt.sh`

A one-screen shell script that:
1. Sets `REBUILD_USERLAND=1`.
2. Invokes the same per-app build blocks that `build.sh` uses (factored into a shared sourceable file, see next section).
3. Hard-fails on any build failure (no soft-fail — this is the explicit "I am committing new prebuilts" path).
4. Prints a `git status userland/prebuilt/` at the end so the developer sees what changed.

Recommended invocation in the workflow:

```sh
# After changing userland/apps/zsh/Makefile or patches:
./userland/refresh-prebuilt.sh
git add userland/prebuilt/ZSH.ELF userland/apps/zsh/Makefile
git commit -m "userland(zsh): bump build flags; refresh prebuilt"
```

### Shared build helpers

The current `build.sh` and `test.sh` duplicate the per-app build/stage blocks. This plan introduces a shared sourceable file `userland/prebuilt-lib.sh` that both scripts (and `refresh-prebuilt.sh`) source. It exports a single function per app — `stage_zsh`, plus future `stage_bash`, etc. — that encapsulates the decision tree above. `build.sh` and `test.sh` lose ~30 lines each; the refresh script becomes a thin loop over the stage functions.

The HELLO and HELLOCPP blocks are NOT moved into this shared file — they stay inline because their semantics (always rebuild from source) differ enough that sharing would obscure the difference. Worth flagging in the lib file's header comment.

### Documentation updates

- **`CLAUDE.md` → "Common Commands → Build and Run"**: add a bullet under `./build.sh` mentioning `--rebuild-userland`, and a one-paragraph note that ZSH.ELF (and future Linux ports) ship as prebuilt binaries under `userland/prebuilt/` checked into the repo. Cross-link `userland/prebuilt/README.md`.
- **`userland/README.md`**: new top-level section "Prebuilt ELFs" explaining:
  - What's in `userland/prebuilt/` and why (toolchain + source-fetch tax avoidance).
  - The committed-binary convention: source change without a refresh = stale binary; reviewer should flag.
  - The decision tree: which apps prebuild (zsh, future ports), which don't (hello, hello-cpp), and the criterion (does the app fetch upstream source? does the build take more than a few seconds?).
  - `./userland/refresh-prebuilt.sh` as the canonical refresh workflow.
- **`userland/prebuilt/README.md`** (new): operational guide. Per-app table listing source path, expected `readelf` type, approximate size, and the last-refresh commit (manually updated on refresh). One-line refresh instructions.

---

## Phasing

This is small enough to land as one PR, but the work splits cleanly:

- **P1. Mechanism.** Add `userland/prebuilt/` (empty + README), `userland/prebuilt-lib.sh`, `userland/refresh-prebuilt.sh`. Refactor zsh block in `build.sh` and `test.sh` to call the shared `stage_zsh` function. Add CLI flag parsing for `--rebuild-userland`. Verify that with no `userland/prebuilt/ZSH.ELF` present, behavior is identical to today (auto-rebuild).
- **P2. Commit the prebuilt.** Run `./userland/refresh-prebuilt.sh` on a clean checkout with the musl toolchain. Commit the resulting `userland/prebuilt/ZSH.ELF`. Verify `git clone && ./build.sh` on a machine WITHOUT the musl toolchain reaches a zsh prompt.
- **P3. Docs.** Update `CLAUDE.md`, `userland/README.md`, write `userland/prebuilt/README.md`. Add an entry to `docs/solutions/learnings/` only if a surprising issue surfaces during P1/P2.

P1 and P3 are independent; P2 depends on P1.

---

## Risks and mitigations

- **Stale binary in repo.** Developer changes `userland/apps/zsh/Makefile` (e.g. bumps zsh version) but forgets `refresh-prebuilt.sh`. Result: the committed binary lags the source until someone notices.
  - *Mitigation:* documented convention in `userland/README.md`; PR template (out of scope here) could add a checkbox; future lighter option = SHA256 manifest verified at build time.
- **Binary review opacity.** Reviewers can't meaningfully eyeball a 1.5 MB ELF diff.
  - *Mitigation:* `userland/prebuilt/README.md` maintains a manually-updated per-app last-refresh commit pointer so the reviewer can correlate the binary change with the source/Makefile change in the same PR. If they're out of sync, that's the smell.
- **Repo size growth as more ports land.** Each port adds 1–10 MB.
  - *Mitigation:* documented threshold (~50 MB total) for revisiting LFS. The `userland/prebuilt/README.md` table makes total size visible at a glance.
- **Reproducibility drift.** Two developers running `refresh-prebuilt.sh` on different musl versions produce different binaries. Subsequent refresh PRs show "this binary changed" with no source-side change.
  - *Mitigation:* documented in `userland/README.md` — refresh only happens deliberately, and a binary-only diff PR with no source change is OK as long as the commit message names the toolchain version. Long-term fix is pinning the toolchain (out of scope).
- **Atomic-replace race in `userland/prebuilt/`.** If two parallel `build.sh` invocations both decide to rebuild, they race on writing the prebuilt.
  - *Mitigation:* same tmp+rename pattern already in use for `host_share/` staging. The worst case is one writer's binary wins; both binaries are byte-identical for the same source, so this is a non-issue in practice. Conductor workspaces are git worktrees with separate file trees, so cross-workspace races don't occur.

---

## Open questions

- Should we adopt a per-app last-refresh manifest entry now (lightweight, manually maintained) or defer until we have a second prebuilt entry? Plan defers — one ELF doesn't need a manifest.

## Resolved decisions

- **`refresh-prebuilt.sh` does NOT auto-commit.** It prints `git status userland/prebuilt/` and exits. The developer stages and commits the refreshed binary alongside any source change. Rationale: keeps the script side-effect surface limited to the build outputs; commit hygiene (message wording, what else to include in the commit) stays with the developer.

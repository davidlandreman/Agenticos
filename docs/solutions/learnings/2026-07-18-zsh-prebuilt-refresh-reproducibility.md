---
title: Zsh prebuilt refresh reproducibility on macOS worktrees
date: 2026-07-18
related:
  - docs/plans/2026-07-18-001-feat-zsh-config-agnoster-powerline-plan.md
  - userland/apps/zsh/Makefile
tags: [zsh, userland, prebuilt, ncurses, reproducibility]
---

# Zsh prebuilt refresh reproducibility on macOS worktrees

Refreshing zsh 5.9 while adding global startup configuration exposed three
build-recipe assumptions that had aged poorly. All three are now encoded in
`userland/apps/zsh/Makefile` so a clean refresh needs only the documented musl
cross-toolchain. Booting the resulting prompt also exposed a separate syscall
compatibility gap in command-substitution output.

## Failure chain

1. `https://www.zsh.org/pub/zsh-5.9.tar.xz` began returning 404 after the
   release moved to upstream's archive. The pinned source now uses
   `https://www.zsh.org/pub/old/zsh-5.9.tar.xz` with the unchanged SHA256.
2. The extraction recipes used the upstream `configure` files as Make targets
   and touched them after unpacking. That made zsh's generated maintainer
   files look stale, so the ordinary build tried to run unavailable
   `autoheader`. Dedicated `.agenticos-extracted` stamps now track extraction
   without mutating release-tarball timestamps.
3. ncurses' broad `make install` also generates its terminfo database. The
   database is not staged or used by AgenticOS, and its install failed while
   writing an entry on the macOS worktree filesystem. The recipe now installs
   only `install.libs install.includes`, which are the artifacts zsh links.

## Prompt runtime compatibility

Agnoster builds prompt segments with zsh builtins inside command
substitutions. zsh emits that buffered output with `writev`, and the command
substitution connects stdout to a pipe. AgenticOS supported scalar pipe
writes, but its `writev` handler returned `ENOSYS` for pipe descriptors, so
the terminal filled with `prompt_segment:echo: write error: function not
implemented` instead of drawing a prompt.

The `writev` handler now applies normal partial-write and blocking semantics
to pipe-backed descriptors, with a direct vectored-write round-trip test.
Once that write completed, a production boot exposed a second boundary:
upstream Agnoster evaluates `$(build_prompt)` on every redraw, and the nested
zsh child/SIGCHLD path can fault in the current guest signal implementation.
The AgenticOS adaptation therefore assembles the same segments into a shell
variable from a `precmd` hook, without forking during prompt redraw. An
end-to-end zsh test sources the staged global rc, builds the prompt, asserts
that it contains the Powerline separator, and rejects a residual `$()` in
`PROMPT`.

## Verification pattern

After changing zsh build flags, verify all three layers rather than trusting a
successful `make` alone:

- `readelf -h` reports `EXEC`, and `readelf -l` has no `INTERP` segment.
- `file` reports a statically linked, stripped x86-64 ELF.
- Generated `config.h` defines `GLOBAL_ZSHRC` while the unused global rc files
  remain undefined.

The normal `./test.sh` boot then proves the committed ELF, staged zsh source
tree, runtime `/etc` import, and kernel syscall surface work together without
requiring the cross-toolchain on subsequent clones.

# AgenticOS zsh configuration

Committed, toolchain-independent files staged under the read-only `/host`
share by `build.sh` and `test.sh`, then imported into the kernel-managed
runtime `/etc` namespace during boot:

- `zshrc` becomes `/etc/zshrc`.
- `agnoster.zsh-theme` becomes `/etc/zsh/agnoster.zsh-theme`.
- `functions/*` become `/etc/zsh/functions/*` and are refreshed from the
  pinned zsh 5.9 tarball with `make -C userland/apps/zsh functions`.

`stage_zsh_config` also writes a function manifest so the kernel can import
the complete pruned library without relying on VFS directory enumeration.

The agnoster theme is vendored from oh-my-zsh commit
`ac5295678f3325de1a69f9e2a603d69573112d05` (the last pre-Terraform version)
with the `# AgenticOS:` adaptations documented inline: a fixed Powerline
separator under `C.UTF-8`, a git segment that stays silent until `GIT.ELF`
is on PATH, and in-process prompt assembly from a `precmd` hook. The last
change avoids forking a zsh child for every prompt redraw while preserving
Agnoster's segments and colors.

The git segment computes its dirty color and staged/unstaged markers from a
single `git status --porcelain` pass rather than oh-my-zsh's `parse_git_dirty`
(never vendored) and zsh's `vcs_info` (its git backend hits a guest-specific
parse error). The rendering is unchanged: green for a clean tree, yellow when
dirty (untracked files included), and `± ` / `✚` markers for tracked unstaged
and staged changes.

User overrides belong in `/root/.zshrc`. Zsh sources that file after the
global config; running `sync` persists it through the writable overlay.

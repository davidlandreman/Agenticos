# AgenticOS zsh configuration

Committed, toolchain-independent files staged into the guest's read-only
`/etc` namespace by `build.sh` and `test.sh`:

- `zshrc` becomes `/etc/zshrc`.
- `agnoster.zsh-theme` becomes `/etc/zsh/agnoster.zsh-theme`.
- `functions/*` become `/etc/zsh/functions/*` and are refreshed from the
  pinned zsh 5.9 tarball with `make -C userland/apps/zsh functions`.

The agnoster theme is vendored from oh-my-zsh commit
`ac5295678f3325de1a69f9e2a603d69573112d05` (the last pre-Terraform version)
with the `# AgenticOS:` adaptations documented inline: a fixed Powerline
separator under `C.UTF-8`, an explicit no-git guard comment, and in-process
prompt assembly from a `precmd` hook. The last change avoids forking a zsh
child for every prompt redraw while preserving Agnoster's segments and
colors.

User overrides belong in `/root/.zshrc`. Zsh sources that file after the
global config; running `sync` persists it through the writable overlay.

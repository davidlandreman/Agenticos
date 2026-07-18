---
title: "feat: zsh startup config + agnoster prompt with Powerline font"
status: planned
created: 2026-07-18
plan_type: feat
depth: deep
related_docs:
  - docs/plans/2026-05-09-003-feat-zsh-on-agenticos-plan.md
  - docs/plans/2026-05-24-001-feat-terminal-ansi-vt-pty-and-caret-plan.md
  - docs/solutions/learnings/2026-05-24-terminal-ansi-vt-pty-overhaul.md
  - src/terminal/CLAUDE.md
  - userland/apps/zsh/README.md
  - userland/prebuilt/README.md
---

# feat: zsh startup config + agnoster prompt with Powerline font

## Summary

Today the terminal boots ring-3 zsh with **no startup files at all**: the
prebuilt `ZSH.ELF` is configured with `--disable-etcdir --disable-zshrc`
(global rc compiled out), `$HOME=/root` is an empty tmpfs dir (no
`~/.zshrc`), the zsh function library is never installed (empty effective
`fpath`), and the bundled terminal font (JetBrains Mono) has no glyphs in
the Powerline private-use area U+E0A0–U+E0B3. The result is a bare
`hostname%` prompt and no way to customize the shell.

This plan makes zsh load a real config chain and ships the **agnoster**
theme as the default prompt, rendered correctly with Powerline separators:

1. **Font** — swap `assets/system.ttf` for a Powerline-patched JetBrains
   Mono (Nerd Font patch, subset to keep the kernel small). No renderer
   code changes needed: the TTF path already lazily rasterizes any
   codepoint the face contains (`src/graphics/fonts/ttf.rs`).
2. **zsh rebuild** — re-enable global rc sourcing (`/etc/zshrc`) in the
   prebuilt `ZSH.ELF` and commit the refreshed binary.
3. **Shipped config** — stage a default `/etc/zshrc`, a pruned zsh
   function library, and the agnoster theme file into `host_share/`
   (read-only `/host`, reachable as `/etc/...` via the existing kernel
   path rewrite). User overrides live in writable `/root/.zshrc`, which
   zsh already sources after `/etc/zshrc`.
4. **Env fix** — `LANG=C.UTF-8` in the terminal's default environment so
   zsh's multibyte prompt-width accounting treats the Powerline glyphs
   as characters, not bytes.

Everything else needed is already in place: the VT parser reassembles
UTF-8 into `char`s, terminal cells store full codepoints, SGR `38;5;N` /
`48;5;N` (agnoster's 256-color backgrounds) and truecolor are handled,
`TERM=xterm-256color` is set, and all file syscalls the startup path
needs (`open`/`read`/`stat`/`access`/`getdents64`) exist.

## Goals

1. Interactive zsh sources `/etc/zshrc` (shipped defaults) and then
   `/root/.zshrc` (user overrides, persisted across reboots via `sync`).
2. The default prompt is agnoster with correctly rendered Powerline
   separators (U+E0B0 etc.) and segment background colors.
3. A usable pruned function library on `fpath` (`promptinit`, `colors`,
   `add-zsh-hook`, `is-at-least`, plus whatever the theme autoloads), so
   the config can use standard zsh idioms.
4. Sane shipped defaults beyond the prompt: persistent history under
   `/root/.zsh_history`, `DEFAULT_USER=root` so agnoster hides the
   redundant `root@host` segment, a plain-prompt fallback if theme
   sourcing fails.
5. Fresh clones keep working with no musl cross-toolchain: the rebuilt
   `ZSH.ELF` and the staged config/function files are all committed
   artifacts, refreshed via `./userland/refresh-prebuilt.sh`.

## Non-goals

- **Completion system (`compinit`)** — the full completion function tree
  is hundreds of files; deferred. The pruned fndir layout makes it a
  drop-in follow-up.
- **git prompt segment content** — there is no `git` binary in the guest
  (BusyBox has no git applet), so agnoster's git segment stays silently
  empty (the theme guards on git's presence). Porting git is out of scope.
- **Login-shell rc chain** (`zprofile`/`zlogin`/`zlogout`) — the terminal
  launches zsh as an interactive non-login shell; those stay compiled out.
- **Wide-character (CJK/emoji) terminal cells** — Powerline separators are
  single-width; double-width rendering remains future work.
- **Bold/italic/underline rendering** — SGR attrs are parsed and stored
  but `TextWindow::set_cell` still ignores them; agnoster only needs
  colors. Unchanged here.

## Current state (verified)

- **Launch**: `src/window/terminal_factory.rs:200-242` — argv is
  `["/host/ZSH.ELF"]` (interactive non-login, rc files enabled at
  runtime), env is `TERMINAL_SHELL_ENV` (`terminal_factory.rs:205-214`):
  `PATH=/bin:/host`, `HOME=/root`, `TERM=xterm-256color`,
  `COLORTERM=truecolor`, `LANG=C`, … No `ZDOTDIR`, no `FPATH`.
- **Build**: `userland/apps/zsh/Makefile:127-132` configures zsh 5.9 with
  `--disable-dynamic` (all modules — zle, zsh/parameter, zsh/zutil, … —
  statically linked, good) but also `--disable-etcdir --disable-zshenv
  --disable-zshrc --disable-zlogin --disable-zprofile --disable-zlogout`,
  so **global startup files are compiled out**. Only `Src/zsh` is copied
  out; `make install.fns` never runs, so **no function library ships**.
- **Filesystems**: `/` is overlay(tmpfs, boot FAT) — nothing can be
  seeded into the tmpfs upper at build time, so `/root` starts empty.
  `/host` is read-only vvfat over `host_share/`, populated by plain
  `cp`/`mkdir` in `build.sh:94-111`. The kernel rewrites `/etc/...` →
  `/host/etc/...` (`src/userland/path.rs:86-97`), which is how musl finds
  `host_share/ETC/PASSWD` today. FAT LFN + subdirectories work; lookups
  inside a mount are case-insensitive.
- **Terminal**: UTF-8 → `char` reassembly in `src/terminal/vte.rs:233-277`;
  cells store `char` (`src/terminal/screen.rs:54-60`); SGR 256-color and
  truecolor in `screen.rs:624-678` + `src/terminal/colors.rs`. A codepoint
  the font lacks renders as blank (glyph lookup returns empty coverage).
- **Font**: `assets/system.ttf` is JetBrains Mono (274 KB), embedded via
  `include_bytes!` (`src/graphics/fonts/core_font.rs:83`), rasterized at
  14 px. ASCII is pre-rasterized; **all other codepoints are lazily
  rasterized and cached** (`src/graphics/fonts/ttf.rs:51,131,175`), so a
  font that *contains* U+E0A0–U+E0B3 needs zero code changes. The missing
  Powerline glyphs are already called out as open work in
  `src/terminal/CLAUDE.md` and the 2026-05-24 terminal learnings doc.

## Design

### D1. Font: Powerline-patched JetBrains Mono, subset, committed

Replace `assets/system.ttf` with **JetBrainsMono Nerd Font Mono**
(the Nerd Fonts patch of the exact typeface we already ship, OFL-1.1) so
the terminal's look and metrics stay effectively identical.

The full patched TTF is ~2.2 MiB (vs 274 KB today) because it carries
thousands of Nerd Font icons. The kernel embeds the font with
`include_bytes!`, so ship a **subset** instead:

- Subset = the current JetBrains Mono coverage **plus U+E0A0–U+E0B3**
  (Powerline: branch, LN, padlock, and the four separator arrows). This
  keeps the asset in the same size class as today.
- Produce it offline with `pyftsubset` (fonttools); commit the resulting
  TTF plus a `tools/make-system-font.sh` script that records the exact
  source URL, version, and subset command so the artifact is
  reproducible — same philosophy as `userland/refresh-prebuilt.sh`.
- Update `assets/system.ttf.LICENSE` (OFL-1.1 text + Nerd Fonts
  attribution).

Fallback consideration: if subsetting proves fiddly, "DejaVu Sans Mono
for Powerline" (~340 KB, Bitstream Vera license) works out of the box but
changes the typeface; treat it as plan B.

Renderer hardening (small, optional but cheap): `GraphicsDevice::draw_text`
(`src/window/graphics.rs:81-83`) skips glyphs the font lacks. Verify the
terminal's per-cell paint path draws each cell at `col * cell_width` (so
a missing glyph leaves a clean blank cell rather than shifting the line);
if any terminal path advances by glyph advance, fix it to fixed-cell
positioning while here.

### D2. zsh rebuild: enable `/etc/zshrc` only

`userland/apps/zsh/Makefile` configure changes:

- **Remove** `--disable-etcdir` and `--disable-zshrc` → the binary
  compiles in `GLOBAL_ZSHRC=/etc/zshrc`, sourced for interactive shells.
- **Keep** `--disable-zshenv --disable-zlogin --disable-zprofile
  --disable-zlogout`. `zshenv` would run for every `zsh -c` a script
  spawns (startup tax, and we have nothing to put there — env comes from
  `TERMINAL_SHELL_ENV`); the login trio is irrelevant for a non-login
  shell. Revisit `--enable-zshenv` only if a
  `command_not_found_handler`-for-scripts need materializes (see the
  BusyBox plan's Option B).
- No `--enable-fndir` baking: `fpath` is set by the shipped `/etc/zshrc`
  instead, so relocating the function tree never requires a zsh rebuild.

Then `./userland/refresh-prebuilt.sh` to rebuild and commit the new
`userland/prebuilt/ZSH.ELF` (verify `readelf` still reports `EXEC`,
static, stripped). Update the flag-rationale table in
`userland/apps/zsh/README.md`.

### D3. Shipped config, function library, and theme

New committed source tree **`userland/zsh-config/`** (plain text files,
no toolchain needed):

```
userland/zsh-config/
├── zshrc                  # becomes /etc/zshrc in the guest
├── agnoster.zsh-theme     # vendored theme (MIT, from oh-my-zsh), + LICENSE
└── functions/             # pruned zsh 5.9 function library
    ├── promptinit, colors, add-zsh-hook, is-at-least, …
    └── (exactly the autoloads the theme + zshrc need)
```

- **`zshrc` contents** (shipped defaults, deliberately small):
  - `fpath=(/etc/zsh/functions $fpath)` and `autoload -Uz` of the staged
    helpers.
  - History: `HISTFILE=/root/.zsh_history`, `HISTSIZE=5000`,
    `SAVEHIST=5000`, `setopt share_history hist_ignore_dups`. `/root` is
    the overlay upper — history persists across reboots once the user
    runs `sync` (existing overlay semantics; document this in the file).
  - Quality-of-life setopts: `interactive_comments`, `prompt_subst`.
  - `DEFAULT_USER=root` so agnoster suppresses the `root@host` segment.
  - Theme load with fallback:
    `source /etc/zsh/agnoster.zsh-theme 2>/dev/null || PS1='%F{green}%~%f %# '`
    — a broken/missing theme must never leave the user with a blank or
    error-spewing prompt.
  - Nothing user-hostile: zsh sources `$HOME/.zshrc` *after*
    `/etc/zshrc` automatically, so `/root/.zshrc` (user-created, synced
    to `/data`) can override everything.
- **`agnoster.zsh-theme`**: vendor the oh-my-zsh variant (MIT), with two
  minimal adaptations, each marked with an `# AgenticOS:` comment:
  guard the git segment on `(( $+commands[git] ))` if the vendored
  version doesn't already, and default `SEGMENT_SEPARATOR=$''`
  unconditionally (no locale sniffing). During implementation, audit the
  theme for `autoload` calls (`vcs_info` in some variants) and stage
  those function files (VCS_Info tree if needed) — the audit, not this
  plan, decides the final prune list.
- **Function library provenance**: files come from the zsh 5.9 tarball's
  `Functions/` tree (zsh's MIT-like license). Extend
  `userland/apps/zsh/Makefile` with a `functions` target that copies the
  prune list out of the extracted tarball into `userland/zsh-config/
  functions/`, and hook it into `refresh-prebuilt.sh` so the committed
  copies are refreshed alongside `ZSH.ELF` and never hand-edited (except
  via clearly-marked patches, if ever needed).

**Staging** (`build.sh`, next to the existing `ETC/PASSWD` block at
`build.sh:107-111`, ideally as a `stage_zsh_config` helper in
`userland/prebuilt-lib.sh`):

```bash
mkdir -p "$HOST_SHARE_STAGE/ETC/ZSH/FUNCTIONS"
cp userland/zsh-config/zshrc              "$HOST_SHARE_STAGE/ETC/ZSHRC"
cp userland/zsh-config/agnoster.zsh-theme "$HOST_SHARE_STAGE/ETC/ZSH/"
cp userland/zsh-config/functions/*        "$HOST_SHARE_STAGE/ETC/ZSH/FUNCTIONS/"
```

Guest view: `/etc/zshrc`, `/etc/zsh/agnoster.zsh-theme`,
`/etc/zsh/functions/*` — all via the existing `/etc → /host/etc` rewrite,
read-only, case-insensitive FAT lookup (same mechanism as `/etc/passwd`
today). `test.sh` shares the staging path, so tests see the same files.

### D4. Environment: `LANG=C.UTF-8`

Change `LANG=C` → `LANG=C.UTF-8` in `TERMINAL_SHELL_ENV`
(`src/window/terminal_factory.rs:213`). zsh was built with
`--enable-multibyte`; under musl, `C.UTF-8` gives `MB_CUR_MAX > 1` so
ZLE counts the three-byte U+E0B0 as one display column. Under `LANG=C`,
prompt-width math counts bytes and the line editor misplaces the cursor
after every prompt redraw. This is a one-line change but load-bearing.

### Alternatives considered

- **`ZDOTDIR=/host/...` pointing at a staged dotfile dir (no zsh
  rebuild)** — rejected: it hijacks the *user's* dotfile location for
  system defaults, so a user `.zshrc` would have to live inside the
  read-only image or the mechanism abandoned. The standard
  global-rc-then-user-rc layering is exactly what `/etc/zshrc` is for,
  and we own the binary's build.
- **Seeding `/root/.zshrc` into the overlay upper at boot** — rejected:
  first `sync` would freeze the seeded copy into `/data`, shadowing
  future shipped-default updates; kernel-side seeding code for a
  userland concern.
- **Full Nerd Font, no subset** — rejected by default (~2 MiB kernel
  growth for icons nothing renders); revisit if/when a UI wants the
  icon set.
- **Baking `--enable-fndir=/etc/zsh/functions`** — works, but setting
  `fpath` in the shipped zshrc does the same without coupling binary
  rebuilds to layout changes.

## Implementation steps

1. **Font swap** (independent of everything else, do first):
   produce the subset TTF + `tools/make-system-font.sh`, replace
   `assets/system.ttf` + `.LICENSE`. Add kernel test (`src/tests/`)
   asserting `get_default_font().glyph()` returns non-empty coverage for
   `'\u{E0A0}'` and `'\u{E0B0}'..='\u{E0B3}'`, and that `cell_width` /
   `line_height` stay sane (> 0, unchanged vs. a recorded expectation).
   Boot interactively; check desktop/terminal text looks unchanged and
   `printf '\xee\x82\xb0\n'` in zsh shows the arrow.
2. **zsh Makefile + prebuilt refresh**: configure-flag change (D2) +
   `functions` prune-list target (D3); run `./userland/refresh-prebuilt.sh`;
   commit `ZSH.ELF` and `userland/zsh-config/functions/` together with
   the Makefile change (repo rule for prebuilt-managed apps).
3. **Config + theme + staging**: write `userland/zsh-config/zshrc`,
   vendor `agnoster.zsh-theme` (+ license), add the staging block to
   `build.sh` / `prebuilt-lib.sh`.
4. **Env**: `LANG=C.UTF-8` in `terminal_factory.rs`.
5. **Tests**:
   - Font-glyph test from step 1.
   - VFS test: `stat("/etc/zshrc")` and `stat("/etc/zsh/functions/promptinit")`
     resolve through the rewrite + FAT LFN path (guards the staging
     against silent breakage; belongs next to the existing `/bin`
     namespace / path-rewrite tests).
   - Run the full `./test.sh` suite — the zsh rebuild and env change
     touch the interactive boot path that userland tests exercise.
6. **Manual verification** (`./build.sh`): Start → Terminal shows the
   agnoster prompt with colored segments and solid-arrow separators; cursor
   lands correctly after the prompt (tests D4); `echo $fpath`, `history`,
   creating `/root/.zshrc` with `PS1` override + re-opening a terminal all
   behave; a deliberately broken theme file still yields the fallback
   prompt.
7. **Docs**: update `CLAUDE.md` (Current State paragraph),
   `src/terminal/CLAUDE.md` (remove the Powerline item from "what's not
   yet here"), `userland/apps/zsh/README.md` (flag table),
   `userland/prebuilt/README.md` (zsh-config artifact), and add a
   learnings note if implementation surfaces surprises.

## Risks & open questions

- **Prompt-width correctness under musl `C.UTF-8`** (D4) is the most
  likely surprise: if ZLE still miscounts, symptoms are a cursor offset
  after the prompt. Debug order: confirm `MB_CUR_MAX` via a test binary,
  then check zsh's `MULTIBYTE` option state, before touching the
  terminal.
- **Which autoloads agnoster actually needs** (vcs_info vs. plain `git`
  calls) varies by variant — resolved by the vendoring audit in step 3;
  the prune list follows the audit.
- **vvfat directory size**: dozens of small function files in one FAT
  subdirectory is well within limits, but `test.sh` boots should confirm
  the vvfat mount still assembles quickly.
- **Startup latency**: sourcing zshrc + theme on a FAT-backed read path
  adds file reads at terminal open; expected negligible (few KB), but
  worth a serial-timestamp sanity check on first boot.
- **Missing-glyph advance behavior** (D1 hardening): only matters if
  some terminal paint path is advance-based; verify early in step 1.

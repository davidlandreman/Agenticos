# AI Context Conventions

This project uses a layered structure for AI-agent context (primarily Claude Code, since AgenticOS is a single-vendor Claude Code project today). The goal is that an agent working on any subsystem has the right context loaded — and only the right context.

## Three layers

1. **`.claude/rules/*.md`** — project-wide rules. Loaded eagerly at the start of every session. Use this for invariants that apply regardless of which folder is being touched: `no_std` discipline, panic-handler attributes, testing flow.
2. **`CLAUDE.md`** at the repo root — orientation. Loaded eagerly. Contains project overview, build / test / quality commands, top-level core files, configuration, cross-cutting known issues, important resources, and a directory index pointing to per-folder files. Target: ≤ ~200 lines.
3. **`src/<subsystem>/CLAUDE.md`** — subsystem context. Loaded *on demand* when Claude reads a file in that directory. Contains purpose, key-files index, conventions specific to the folder, and gotchas an agent editing those files needs to know.

The Claude Code memory documentation describes the loading semantics: root and ancestor `CLAUDE.md` files load eagerly at session start; subdirectory files load when files in that directory are read; `.claude/rules/*.md` loads eagerly. See <https://code.claude.com/docs/en/memory>.

## Folder-file shape

Each `src/<subsystem>/CLAUDE.md` follows a loose convention — not a strict template:

- One-line purpose statement at the top.
- **Key files** — short index of the important files in the folder with one-line descriptions.
- **Rules / Gotchas** — what an agent editing files in this folder must not get wrong (cross-references to other folder files allowed; cross-references to `.claude/rules/` allowed).
- **References** — relevant `docs/*.md` design documents and any external URLs.

No frontmatter. No required section ordering. Keep it concise — if a folder file grows past ~200 lines, split content into one of the existing `docs/` design files and reference it.

## What NOT to do

- **Do not use `paths:` / `globs:` frontmatter** in `.claude/rules/`. Path-scoped rules are intended to be lazy but currently have multiple open Claude Code bugs that cause the gating to misfire (issues [#16299](https://github.com/anthropics/claude-code/issues/16299), [#16853](https://github.com/anthropics/claude-code/issues/16853), [#21858](https://github.com/anthropics/claude-code/issues/21858), [#23478](https://github.com/anthropics/claude-code/issues/23478)). Rule files in this project load unconditionally; revisit if/when these issues close.
- **Do not use `@path` imports as a splitting mechanism**. They load eagerly, so they organize source text without reducing tokens. Folder files are the splitting tool.
- **Do not duplicate prose** between root, folder files, and rule files. Each topic has exactly one authoritative home. Cross-link instead. (One-line invariant reminders that prevent likely mistakes — e.g., "Arc here is the kernel's custom impl, not `alloc::sync::Arc`" — may appear in more than one place; the no-duplication rule applies to prose explanations, not to short safety reminders.)
- **Do not introduce `AGENTS.md`** unless multi-vendor agent tooling becomes a need. AgenticOS uses Claude Code exclusively today. If that changes, the root `CLAUDE.md` can become a thin wrapper that does `@AGENTS.md`.

## Adding a new subsystem

When you add a new top-level subsystem under `src/`:

1. Decide whether it has enough distinct context to warrant its own `CLAUDE.md`. The threshold: subsystems where a contributor needs roughly a day to absorb local conventions, gotchas, or cross-references. Thin subsystems (one or two files, generic Rust) are covered by the root index entry alone.
2. If yes, write `src/<new-subsystem>/CLAUDE.md` following the shape above.
3. Add an index entry in the root `CLAUDE.md` pointing to the new folder file.
4. If the subsystem introduces a new cross-cutting rule (something that applies anywhere in the codebase), add it to `.claude/rules/` instead.

## Adding a new project-wide rule

Add a new file under `.claude/rules/`. Keep it short (under ~50 lines), no frontmatter, and focused on *what to do / not do*. Reference the deeper context elsewhere ("see `src/mm/CLAUDE.md` for heap internals") rather than restating it.

# `src/commands/` — Kernel-side GUI App Launchers

Each subdirectory is a kernel-side GUI app implementing the
`RunnableProcess` trait. Six modules:

| Module    | What it is                                                                  |
|-----------|------------------------------------------------------------------------------|
| `painting`| Animated shapes demo window                                                 |
| `calc`    | Calculator app                                                              |
| `notepad` | Text editor                                                                 |
| `tasks`   | Task manager (introspects `crate::process::get_process_list`)                |
| `explorer`| File explorer (opens ELFs via `crate::userland::launcher::launch_user_binary`) |
| `guishell`| Desktop + taskbar + start menu manager                                      |

Plus one routing module:

- `gui_launch_table` — `spawn_by_name(name)` dispatches GUI applet names
  to the matching `RunnableProcess` factory. The `sys_gui_launch`
  syscall handler in `src/userland/syscalls.rs` calls this; the taskbar
  in `guishell/` calls this directly.

## How GUI apps get launched

```
zsh: painting                            (user types in ring-3 zsh)
 ├─ access("/bin/painting", X_OK)        → kernel: GUI_APPLETS contains "painting" → 0
 └─ execve("/bin/painting", ["painting"], envp)
      └─ kernel: apply_bin_rewrite() rewrites to
                 execve("/host/GLAUNCH.ELF", ["painting"], envp)
      → load + iretq to ring 3
 GUILAUNCH _start (ring 3):
   argv[0] = "painting"
   sys_gui_launch("painting")            (new AgenticOS syscall)
    └─ kernel: gui_launch_table::spawn_by_name("painting")
              → spawn_process("painting", None, || PaintingProcess::new(...).run())
              → 0 (success)
   exit(0)
```

The taskbar's "Painting" menu item shortcuts the ring-3 round-trip by
calling `gui_launch_table::spawn_by_name` directly from `guishell/`.

## Why "commands" if there's no command interpreter?

Historical name. Until 2026-05-16 this directory hosted the kernel's
hand-written shell + ~14 file-utility commands (`cat`, `ls`, `grep`,
…). Those were deleted when zsh became the default shell —
BusyBox's multicall ELF covers them all from ring 3. See
`docs/plans/2026-05-16-004-feat-zsh-default-terminal-and-gui-launchers-plan.md`.

## Adding a new GUI app

1. **Create the module** at `src/commands/myapp/mod.rs` implementing
   `RunnableProcess`:
   ```rust
   pub struct MyAppProcess { pub base: BaseProcess, args: Vec<String> }
   pub fn create_myapp_process(args: Vec<String>) -> Box<dyn RunnableProcess> {
       Box::new(MyAppProcess::new_with_args(args))
   }
   ```

2. **Register in `src/commands/mod.rs`**: `pub mod myapp;`

3. **Add a match arm in `src/commands/gui_launch_table.rs::spawn_by_name`**
   pointing at `create_myapp_process`.

4. **Add the name to `GUI_APPLETS` in `src/userland/bin_namespace.rs`**
   (sorted). A test asserts the two stay in sync.

5. **Optionally add a taskbar entry in `src/commands/guishell/mod.rs`.**

The app is now invocable from zsh as bare `myapp`, from any kernel
caller via `gui_launch_table::spawn_by_name("myapp")`, and (if added in
step 5) from the start menu.

## Cross-references

- Process / scheduler internals: `src/process/CLAUDE.md`.
- The `/bin/<applet>` virtual namespace + `sys_gui_launch` syscall:
  `src/userland/bin_namespace.rs`, `src/userland/syscalls.rs::gui_launch_handler`.
- `Vec<String>` / `Box<dyn …>` require heap init — see `.claude/rules/no-std.md`.

# `src/commands/` — Shell Commands

Each subdirectory is one shell command implementing the `RunnableProcess` trait, dispatched by the process manager (see `src/process/CLAUDE.md`).

## Current commands

18 command directories: `calc`, `cat`, `dir`, `echo`, `grep`, `guishell`, `head`, `hexdump`, `ls`, `notepad`, `painting`, `pwd`, `shell`, `tail`, `tasks`, `time`, `touch`, `wc`.

`shell/` is the main system shell that everything else dispatches through.

## How dispatch works

1. **Registration** at boot — each command exposes a factory function:
   ```rust
   register_command("ls", create_ls_process);
   ```
2. **Execution** — the shell parses input and routes the command name to the process manager:
   ```rust
   execute_command("ls /home")?;   // runs synchronously to completion
   ```
3. **Implementation** — each command implements `RunnableProcess`:
   ```rust
   pub trait RunnableProcess {
       fn run(&mut self);
       fn get_name(&self) -> &str;
   }
   ```

## Adding a new command

1. **Create the command module** at `src/commands/mycommand/mod.rs`:
   ```rust
   use crate::process::{BaseProcess, HasBaseProcess, RunnableProcess};

   pub struct MyCommandProcess {
       pub base: BaseProcess,
       args: Vec<String>,
   }

   pub fn create_mycommand_process(args: Vec<String>) -> Box<dyn RunnableProcess> {
       Box::new(MyCommandProcess::new_with_args(args))
   }
   ```

2. **Implement the traits** — minimal boilerplate:
   ```rust
   impl RunnableProcess for MyCommandProcess {
       fn run(&mut self) { /* your code */ }
       fn get_name(&self) -> &str { self.base.get_name() }
   }
   ```

3. **Register in `src/kernel.rs`**:
   ```rust
   register_command("mycommand", create_mycommand_process);
   ```

4. **Export from `src/commands/mod.rs`**:
   ```rust
   pub mod mycommand;
   ```

The command is now available in the shell.

## Cross-references

- Process / dispatcher internals: `src/process/CLAUDE.md`.
- `Vec<String>` / `Box<dyn …>` require heap init — see `.claude/rules/no-std.md`.

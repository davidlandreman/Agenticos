//! `shell_run` tool — execute a shell command synchronously and return its
//! captured stdout. v1 uses an argv-only allowlist; commands that need stdin
//! or open windows are rejected so the dispatcher process never blocks.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use serde::Deserialize;
use serde_json::json;
use spin::Mutex;

use crate::tools::{Tool, ToolError, ToolResult};
use crate::window::types::WindowId;

/// v1 allowlist: argv-only commands. No stdin readers, no GUI commands.
/// Widening this requires either piping stdin or running async with a
/// timeout — both are explicit non-goals for v1.
const ALLOWLIST: &[&str] = &[
    "ls", "cat", "pwd", "echo", "dir", "touch", "hexdump",
];

/// Synthetic `WindowId` used by the dispatcher to capture command output. The
/// kernel registers this once at boot via `crate::window::terminal::register_terminal`.
pub const RPC_TERMINAL_ID: WindowId = WindowId(usize::MAX);

/// Serializes concurrent `shell_run` calls so they don't collide on
/// `set_current_output_terminal`. The lock holds for the entire run; v1 has
/// only one in-flight RPC at a time anyway, so this is rarely contended.
static SHELL_RUN_LOCK: Mutex<()> = Mutex::new(());

#[derive(Deserialize)]
struct ShellRunArgs {
    command: String,
}

pub struct ShellRun;

impl Tool for ShellRun {
    fn name(&self) -> &'static str { "shell_run" }

    fn description(&self) -> &'static str {
        "run a shell command (allowlisted text-only commands) and return stdout"
    }

    fn schema(&self) -> &'static str {
        r#"{"type":"object","required":["command"],"properties":{"command":{"type":"string"}}}"#
    }

    fn call(&self, args_json: &str) -> Result<ToolResult, ToolError> {
        let args: ShellRunArgs = serde_json::from_str(args_json)
            .map_err(|e| ToolError::bad_args(format!("invalid args: {}", e)))?;

        let trimmed = args.command.trim();
        if trimmed.is_empty() {
            return Err(ToolError::bad_args("empty command"));
        }

        let head = trimmed.split_whitespace().next().unwrap_or("");
        if !ALLOWLIST.contains(&head) {
            return Err(ToolError::unsupported(format!(
                "command {:?} is not in v1 allowlist (allowed: {:?})",
                head, ALLOWLIST
            )));
        }

        let _guard = SHELL_RUN_LOCK.lock();

        // Save and restore the prior output terminal so the user's interactive
        // shell isn't disrupted if a `shell_run` lands while a real command is
        // routing.
        let prior = crate::window::terminal::get_current_output_terminal();
        crate::window::terminal::set_current_output_terminal(RPC_TERMINAL_ID);

        let exec_result = crate::process::execute_command_sync(trimmed);

        // Drain whatever the command produced before any other writes can
        // append.
        let output_lines: Vec<String> =
            crate::window::terminal::take_terminal_output(RPC_TERMINAL_ID);

        // Restore prior routing (or clear if none).
        match prior {
            Some(id) => crate::window::terminal::set_current_output_terminal(id),
            None => crate::window::terminal::clear_current_output_terminal(),
        }

        let stdout = output_lines.join("");

        let body = match exec_result {
            Ok(()) => json!({
                "stdout": stdout,
                "exit": "ok",
            }),
            Err(msg) => json!({
                "stdout": stdout,
                "exit": "error",
                "error": msg,
            }),
        };

        let json = serde_json::to_string(&body)
            .map_err(|e| ToolError::tool_failed(format!("serialize: {}", e)))?;
        Ok(ToolResult::json_only(json))
    }
}

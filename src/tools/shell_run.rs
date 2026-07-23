//! `shell_run` tool — historically executed a kernel-side shell command
//! synchronously and returned its captured stdout. The kernel-side
//! command interpreter was removed when zsh became the default
//! terminal shell (see
//! `docs/plans/2026-05-16-004-feat-zsh-default-terminal-and-gui-launchers-plan.md`).
//!
//! For now `shell_run` is a stub that returns "not supported". A future
//! revision can reimplement it with an ordinary pipe-backed ring-3 child.

use alloc::format;
use alloc::string::String;

use serde::Deserialize;
use serde_json::json;

use crate::tools::{Tool, ToolError, ToolResult};

#[derive(Deserialize)]
#[cfg_attr(feature = "test", expect(dead_code, reason = "production-only API"))]
struct ShellRunArgs {
    command: String,
}

#[cfg_attr(feature = "test", expect(dead_code, reason = "production-only API"))]
pub struct ShellRun;

impl Tool for ShellRun {
    fn name(&self) -> &'static str {
        "shell_run"
    }

    fn description(&self) -> &'static str {
        "(disabled) executed kernel-side shell commands; removed when zsh became the default shell"
    }

    fn schema(&self) -> &'static str {
        r#"{"type":"object","required":["command"],"properties":{"command":{"type":"string"}}}"#
    }

    fn call(&self, args_json: &str) -> Result<ToolResult, ToolError> {
        let args: ShellRunArgs = serde_json::from_str(args_json)
            .map_err(|e| ToolError::bad_args(format!("invalid args: {}", e)))?;
        let body = json!({
            "stdout": String::new(),
            "exit": "error",
            "error": format!(
                "shell_run is disabled: the kernel-side command interpreter was removed. \
                 Type {:?} into the on-screen zsh terminal instead.",
                args.command,
            ),
        });
        let s = serde_json::to_string(&body)
            .map_err(|e| ToolError::tool_failed(format!("serialize: {}", e)))?;
        Ok(ToolResult::json_only(s))
    }
}

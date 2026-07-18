//! `kernel_state` tool — structured snapshots of in-kernel state.
//!
//! Discriminator: `{"what": "windows" | "processes" | "heap" | "memory"}`. Adding a new
//! discriminator is additive (R5).

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::process::ProcessState;
use crate::tools::{Tool, ToolError, ToolResult};

#[derive(Deserialize)]
#[cfg_attr(feature = "test", expect(dead_code, reason = "production-only API"))]
struct KernelStateArgs<'a> {
    what: &'a str,
}

#[cfg_attr(feature = "test", expect(dead_code, reason = "production-only API"))]
pub struct KernelState;

impl Tool for KernelState {
    fn name(&self) -> &'static str {
        "kernel_state"
    }

    fn description(&self) -> &'static str {
        "structured snapshot of in-kernel state (windows, processes, heap, memory)"
    }

    fn schema(&self) -> &'static str {
        r#"{"type":"object","required":["what"],"properties":{"what":{"type":"string","enum":["windows","processes","heap","memory"]}}}"#
    }

    fn call(&self, args_json: &str) -> Result<ToolResult, ToolError> {
        let args: KernelStateArgs = serde_json::from_str(args_json)
            .map_err(|e| ToolError::bad_args(format!("invalid args: {}", e)))?;

        let value = match args.what {
            "windows" => snapshot_windows()?,
            "processes" => snapshot_processes(),
            "heap" => snapshot_heap()?,
            "memory" => snapshot_memory()?,
            other => {
                return Err(ToolError::bad_args(format!(
                    "unknown discriminator {:?}; expected windows | processes | heap | memory",
                    other
                )));
            }
        };

        let json = serde_json::to_string(&value)
            .map_err(|e| ToolError::tool_failed(format!("serialize: {}", e)))?;
        Ok(ToolResult::json_only(json))
    }
}

#[cfg_attr(feature = "test", expect(dead_code, reason = "production-only API"))]
fn snapshot_windows() -> Result<Value, ToolError> {
    let result = crate::window::with_window_manager(|wm| {
        let mut entries: Vec<Value> = Vec::with_capacity(wm.window_registry.len());
        for (id, w) in wm.window_registry.iter() {
            let bounds = w.bounds();
            entries.push(json!({
                "id": id.0 as u64,
                "parent": w.parent().map(|p| p.0 as u64),
                "title": w.window_title(),
                "bounds": {
                    "x": bounds.x,
                    "y": bounds.y,
                    "width": bounds.width,
                    "height": bounds.height,
                },
                "visible": w.visible(),
                "focused": w.has_focus(),
                "children": w.children().iter().map(|c| c.0 as u64).collect::<Vec<_>>(),
            }));
        }
        Value::Array(entries)
    });

    result.ok_or_else(|| ToolError::unsupported("window manager not initialized"))
}

#[cfg_attr(feature = "test", expect(dead_code, reason = "production-only API"))]
fn snapshot_processes() -> Value {
    let processes = crate::process::get_process_list();
    let entries: Vec<Value> = processes
        .iter()
        .map(|p| {
            json!({
                "pid": format!("{:?}", p.pid),
                "name": p.name,
                "state": process_state_name(&p.state),
                "total_runtime_ticks": p.total_runtime,
                "stack_size": p.stack_size,
                "cpu_percentage": p.cpu_percentage,
            })
        })
        .collect();
    json!({
        "processes": entries,
        "count": processes.len(),
    })
}

#[cfg_attr(feature = "test", expect(dead_code, reason = "production-only API"))]
fn snapshot_heap() -> Result<Value, ToolError> {
    let stats =
        crate::mm::heap::stats().ok_or_else(|| ToolError::unsupported("heap not initialized"))?;
    Ok(json!({
        "size": stats.size,
        "used": stats.used,
        "free": stats.free,
        "bottom": stats.bottom,
        "top": stats.top,
    }))
}

#[cfg_attr(feature = "test", expect(dead_code, reason = "production-only API"))]
fn snapshot_memory() -> Result<Value, ToolError> {
    let stats = crate::mm::memory::with_memory_mapper(|mapper| mapper.frame_stats())
        .ok_or_else(|| ToolError::unsupported("frame allocator not initialized"))?;
    Ok(json!({
        "total_usable_frames": stats.total_usable,
        "pinned_frames": stats.pinned,
        "allocated_frames": stats.allocated,
        "exclusive_frames": stats.exclusive(),
        "shared_frames": stats.shared,
        "free_frames": stats.free,
    }))
}

fn process_state_name(state: &ProcessState) -> String {
    match state {
        ProcessState::Ready => "ready",
        ProcessState::Running => "running",
        ProcessState::Blocked => "blocked",
        ProcessState::Terminated => "terminated",
    }
    .to_string()
}

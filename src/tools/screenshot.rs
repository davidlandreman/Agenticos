//! `screenshot` tool — capture the current framebuffer as raw bytes plus
//! metadata. The kernel does not encode PNG; the host bridge does.

use alloc::format;

use serde_json::json;

use crate::tools::{Tool, ToolError, ToolResult};

pub struct Screenshot;

impl Tool for Screenshot {
    fn name(&self) -> &'static str { "screenshot" }

    fn description(&self) -> &'static str {
        "raw framebuffer snapshot (host bridge encodes PNG)"
    }

    fn schema(&self) -> &'static str {
        r#"{"type":"object","properties":{}}"#
    }

    fn call(&self, _args_json: &str) -> Result<ToolResult, ToolError> {
        // Acquire the WM lock long enough to snapshot through the device's
        // own buffer mutex into an owned Vec, then drop both locks before
        // returning. `with_window_manager` disables interrupts; we don't want
        // it held during serial transmission of the result.
        let snapshot = crate::window::with_window_manager(|wm| wm.framebuffer_snapshot())
        .flatten()
        .ok_or_else(|| ToolError::unsupported(
            "framebuffer not initialized or adapter does not support snapshot",
        ))?;

        let meta = json!({
            "width": snapshot.width,
            "height": snapshot.height,
            "stride": snapshot.stride,
            "bytes_per_pixel": snapshot.bytes_per_pixel,
            "pixel_format": snapshot.pixel_format,
            "byte_len": snapshot.pixels.len(),
        });

        let json = serde_json::to_string(&meta)
            .map_err(|e| ToolError::tool_failed(format!("serialize meta: {}", e)))?;

        Ok(ToolResult::with_binary(json, snapshot.pixels))
    }
}

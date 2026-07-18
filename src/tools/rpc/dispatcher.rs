//! RPC dispatcher loop. Runs as a kernel process; reads request frames from
//! COM2, calls the tool registry, writes response frames back. Errors at any
//! layer return structured JSON error responses — the dispatcher must never
//! panic.

use alloc::format;
use alloc::string::String;
use core::fmt::Write;

use serde::Deserialize;

use crate::debug_warn;
use crate::drivers::serial;
use crate::tools::rpc::framing::{read_frame, write_frame, FrameError};
use crate::tools::{registry, ToolError, ToolResult};

#[derive(Deserialize)]
struct RpcRequest<'a> {
    id: Option<i64>,
    name: &'a str,
    /// Tool args; passed verbatim (re-serialized) to the tool's `call`.
    #[serde(default)]
    args: Option<&'a serde_json::value::RawValue>,
}

/// Special pseudo-tool name handled inline by the dispatcher; not registered.
const LIST_TOOLS: &str = "__list_tools__";

/// Dispatcher process body. Runs forever — never returns. Spawned via
/// `crate::process::spawn_process` during kernel boot.
#[cfg_attr(feature = "test", expect(dead_code, reason = "production-only API"))]
pub fn run_dispatcher() {
    let com = match serial::com2() {
        Some(c) => c,
        None => {
            debug_warn!("rpc: COM2 not initialized; dispatcher exiting");
            return;
        }
    };

    loop {
        match read_frame(com) {
            Ok(frame) => handle_frame(com, &frame.header),
            Err(FrameError::OversizeHeader) => {
                // Lost wire sync mid-header; the next frame may not parse.
                // Best we can do is write a structured error and keep going.
                write_err(com, None, "frame_too_large", "header exceeds MAX_HEADER");
            }
            Err(FrameError::OversizeBinary) => {
                write_err(com, None, "frame_too_large", "binary exceeds MAX_BINARY");
            }
        }
    }
}

fn handle_frame(com: &serial::Com2, header_bytes: &[u8]) {
    // Parse header. If it fails, we cannot recover the request id.
    let req: RpcRequest = match serde_json::from_slice(header_bytes) {
        Ok(r) => r,
        Err(e) => {
            write_err(
                com,
                None,
                "bad_request",
                &format!("malformed header: {}", e),
            );
            return;
        }
    };

    let id = req.id;

    // Inline pseudo-tool: enumerate the registry.
    if req.name == LIST_TOOLS {
        let body = render_list_tools();
        write_ok_raw(com, id, &body, None);
        return;
    }

    // Resolve args to a JSON string the tool can parse. RawValue gives us
    // verbatim bytes; default to "{}" when absent.
    let args_json: &str = req.args.map(|raw| raw.get()).unwrap_or("{}");

    let reg = match registry() {
        Some(r) => r,
        None => {
            write_err(
                com,
                id,
                "registry_uninitialized",
                "tool registry not initialized",
            );
            return;
        }
    };

    let result = {
        let guard = reg.lock();
        guard.call(req.name, args_json)
    };

    match result {
        Ok(ToolResult { json, binary }) => {
            write_ok_raw(com, id, &json, binary.as_deref());
        }
        Err(ToolError { code, message }) => {
            write_err(com, id, code, &message);
        }
    }
}

fn render_list_tools() -> String {
    let reg = match registry() {
        Some(r) => r,
        None => return String::from("[]"),
    };
    let descriptors = {
        let guard = reg.lock();
        guard.enumerate()
    };

    let mut s = String::with_capacity(descriptors.len() * 64);
    s.push('[');
    for (i, d) in descriptors.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push('{');
        s.push_str("\"name\":");
        push_json_string(&mut s, d.name);
        s.push_str(",\"description\":");
        push_json_string(&mut s, d.description);
        // schema is already a JSON document (per Tool::schema contract);
        // embed verbatim, not as a string.
        s.push_str(",\"schema\":");
        s.push_str(d.schema);
        s.push('}');
    }
    s.push(']');
    s
}

fn write_ok_raw(com: &serial::Com2, id: Option<i64>, ok_json: &str, binary: Option<&[u8]>) {
    let mut header = String::with_capacity(ok_json.len() + 32);
    header.push_str("{\"id\":");
    push_id(&mut header, id);
    header.push_str(",\"ok\":");
    header.push_str(ok_json);
    header.push('}');
    write_frame(com, header.as_bytes(), binary);
}

fn write_err(com: &serial::Com2, id: Option<i64>, code: &str, message: &str) {
    let mut header = String::with_capacity(message.len() + 64);
    header.push_str("{\"id\":");
    push_id(&mut header, id);
    header.push_str(",\"error\":{\"code\":");
    push_json_string(&mut header, code);
    header.push_str(",\"message\":");
    push_json_string(&mut header, message);
    header.push_str("}}");
    write_frame(com, header.as_bytes(), None);
}

fn push_id(s: &mut String, id: Option<i64>) {
    match id {
        Some(n) => {
            let _ = write!(s, "{}", n);
        }
        None => s.push_str("null"),
    }
}

/// Encode a `&str` as a JSON string literal, including surrounding quotes and
/// the canonical escape rules for control characters and `"` / `\`.
fn push_json_string(s: &mut String, value: &str) {
    s.push('"');
    for c in value.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            '\r' => s.push_str("\\r"),
            '\t' => s.push_str("\\t"),
            '\x08' => s.push_str("\\b"),
            '\x0c' => s.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(s, "\\u{:04x}", c as u32);
            }
            c => s.push(c),
        }
    }
    s.push('"');
}

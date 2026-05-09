//! Kernel-resident tool registry exposed to the host via MCP.
//!
//! Tools are pure functions over JSON args + optional binary blobs. The
//! registry is transport-agnostic: the serial dispatcher (`rpc::dispatcher`)
//! is one consumer, and any in-kernel caller can reach the same tools through
//! `registry().lock().call(name, args_json)`.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use spin::{Mutex, Once};

pub mod kernel_state;
pub mod rpc;
pub mod screenshot;
pub mod send_input;
pub mod shell_run;

/// A structured response from a tool.
#[derive(Debug)]
pub struct ToolResult {
    /// JSON-encoded result payload.
    pub json: String,
    /// Optional binary trailer (e.g., framebuffer bytes).
    pub binary: Option<Vec<u8>>,
}

impl ToolResult {
    pub fn json_only(json: String) -> Self {
        ToolResult {
            json,
            binary: None,
        }
    }

    pub fn with_binary(json: String, binary: Vec<u8>) -> Self {
        ToolResult {
            json,
            binary: Some(binary),
        }
    }
}

/// A structured error from a tool. Codes are stable strings the bridge can
/// pattern-match without parsing the human-readable message.
#[derive(Debug)]
pub struct ToolError {
    pub code: &'static str,
    pub message: String,
}

impl ToolError {
    pub fn unknown_tool(name: &str) -> Self {
        ToolError {
            code: "unknown_tool",
            message: format!("no tool registered with name {:?}", name),
        }
    }

    pub fn bad_args(message: impl Into<String>) -> Self {
        ToolError {
            code: "bad_args",
            message: message.into(),
        }
    }

    pub fn tool_failed(message: impl Into<String>) -> Self {
        ToolError {
            code: "tool_failed",
            message: message.into(),
        }
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        ToolError {
            code: "unsupported",
            message: message.into(),
        }
    }
}

/// A tool callable through the registry.
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    /// JSON-Schema-ish descriptor of the args shape. Stored verbatim and
    /// surfaced through `__list_tools__` so the host bridge can advertise it
    /// to MCP clients without a parallel source of truth.
    fn schema(&self) -> &'static str;
    fn call(&self, args_json: &str) -> Result<ToolResult, ToolError>;
}

/// Compact metadata for `__list_tools__` enumeration.
pub struct ToolDescriptor {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: &'static str,
}

/// The kernel's tool registry. Add tools at boot via `register`; call them via
/// `call` from the dispatcher or any in-kernel consumer.
pub struct ToolRegistry {
    tools: BTreeMap<&'static str, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        ToolRegistry {
            tools: BTreeMap::new(),
        }
    }

    /// Register a tool. Latest registration wins on name collision. Takes a
    /// `Box<dyn Tool>` so callers can write `Box::new(MyTool)` and let the
    /// compiler coerce — the custom kernel `Arc` lacks `CoerceUnsized`, so
    /// trait-object construction is awkward there. Mutex-protected access in
    /// the dispatcher means single ownership in the registry is sufficient.
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let name = tool.name();
        self.tools.insert(name, tool);
    }

    /// Return descriptors in stable (alphabetical-by-name) order. `BTreeMap`
    /// gives us the ordering for free; downstream consumers (the bridge,
    /// tests) can rely on it without an extra sort.
    pub fn enumerate(&self) -> Vec<ToolDescriptor> {
        self.tools
            .values()
            .map(|t| ToolDescriptor {
                name: t.name(),
                description: t.description(),
                schema: t.schema(),
            })
            .collect()
    }

    pub fn call(&self, name: &str, args_json: &str) -> Result<ToolResult, ToolError> {
        match self.tools.get(name) {
            Some(tool) => tool.call(args_json),
            None => Err(ToolError::unknown_tool(name)),
        }
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

static TOOL_REGISTRY: Once<Mutex<ToolRegistry>> = Once::new();

/// Initialize the global registry. Idempotent. Call once during kernel boot
/// (before tools are registered).
pub fn init() {
    TOOL_REGISTRY.call_once(|| Mutex::new(ToolRegistry::new()));
}

/// Get the global registry handle. Returns `None` if `init()` has not run.
pub fn registry() -> Option<&'static Mutex<ToolRegistry>> {
    TOOL_REGISTRY.get()
}
